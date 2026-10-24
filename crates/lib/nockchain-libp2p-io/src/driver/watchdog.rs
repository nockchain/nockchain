#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(target_os = "linux")]
use std::sync::Arc;

#[cfg(target_os = "linux")]
use tracing::{error, info, warn};

#[cfg(target_os = "linux")]
use crate::metrics::NockchainP2PMetrics;

/// Spawns a dedicated OS thread (not a tokio task) that watches the
/// libp2p-driver heartbeat counter and dumps `/proc/self/task/*/stack`
/// to disk when progress stalls or when the process receives SIGQUIT.
///
/// Using a plain std thread is deliberate: if the tokio runtime deadlocks
/// (the mode we are hunting after the 2026-04-17 LAX1 freeze), every
/// tokio-scheduled task freezes with it. An OS thread outside that
/// scheduler keeps running and can read `/proc/self/task/*/stack`, which
/// reports kernel-level thread states regardless of user-space progress.
///
/// Behaviour:
/// - Wakes every 5 s, reads two liveness signals, and tracks the
///   consecutive duration each has stalled:
///     * `heartbeat`, advanced every 5 s by a standalone tokio task.
///       Stalling means the entire tokio runtime is parked (4/17 shape).
///     * `poke_last_unix`, advanced after every successful TrafficCop
///       kernel-facing poke or peek completion. Stalling means the kernel
///       side wedged (4/18 shape) even if the runtime is fine.
/// - Writes a stack dump when either counter has been stalled for its
///   respective threshold (30 s for heartbeat, 180 s for TrafficCop
///   kernel-operation completion because legitimate big-block kernel work
///   can approach that duration), then re-dumps every 5 min on persistent
///   stall.
/// - Also dumps on SIGQUIT (`kill -QUIT <pid>`) for operator-triggered
///   snapshots before the automatic timeout.
///
/// Output path: `/var/log/nockchain/nockchain-stacks-<unix>-<reason>.txt`
/// if that directory exists, else
/// `/tmp/nockchain-stacks-<unix>-<reason>.txt`.
#[cfg(target_os = "linux")]
pub(super) fn spawn_deadlock_watchdog(
    heartbeat: Arc<AtomicU64>,
    poke_last_unix: Arc<AtomicU64>,
    metrics: Arc<NockchainP2PMetrics>,
) {
    use signal_hook::consts::SIGQUIT;
    use signal_hook::iterator::Signals;

    let signals = match Signals::new([SIGQUIT]) {
        Ok(s) => Some(s),
        Err(err) => {
            warn!(error = %err, "failed to install SIGQUIT watchdog hook");
            None
        }
    };
    let Err(err) = std::thread::Builder::new()
        .name(String::from("libp2p-watchdog"))
        .spawn(move || watchdog_loop(heartbeat, poke_last_unix, metrics, signals))
    else {
        return;
    };
    warn!(error = %err, "failed to spawn libp2p-watchdog thread");
}

#[cfg(target_os = "linux")]
fn watchdog_loop(
    heartbeat: Arc<AtomicU64>,
    poke_last_unix: Arc<AtomicU64>,
    metrics: Arc<NockchainP2PMetrics>,
    signals: Option<signal_hook::iterator::Signals>,
) {
    const TICK: std::time::Duration = std::time::Duration::from_secs(5);
    const HEARTBEAT_DUMP_AT_S: u64 = 30;
    const POKE_DUMP_AT_S: u64 = 180;
    const REDUMP_EVERY_S: u64 = 300;

    fn unix_now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default()
    }

    let mut signals = signals;
    let mut last_tick: u64 = heartbeat.load(Ordering::Relaxed);
    let mut hb_stall_s: u64 = 0;
    let mut hb_last_dump_s: u64 = 0;
    let mut poke_last_dump_s: u64 = 0;

    info!(
        target: "nockchain::watchdog",
        "libp2p watchdog started \
         (heartbeat poll 5s, hb-dump at {HEARTBEAT_DUMP_AT_S}s, \
         poke-dump at {POKE_DUMP_AT_S}s)"
    );
    loop {
        // Non-blocking drain of any SIGQUIT signals that have fired since
        // the last iteration, triggering an on-demand stack dump for each.
        if let Some(s) = signals.as_mut() {
            for sig in s.pending() {
                if sig == signal_hook::consts::SIGQUIT {
                    info!(
                        target: "nockchain::watchdog",
                        "SIGQUIT received, dumping thread stacks"
                    );
                    if dump_thread_stacks("sigquit").is_ok() {
                        metrics.watchdog_stack_dump_total.increment();
                    }
                }
            }
        }
        std::thread::sleep(TICK);

        // --- heartbeat stall check (catches full runtime freeze) ---
        let now_tick = heartbeat.load(Ordering::Relaxed);
        if now_tick == last_tick {
            hb_stall_s = hb_stall_s.saturating_add(TICK.as_secs());
            let should_dump = hb_stall_s == HEARTBEAT_DUMP_AT_S
                || (hb_stall_s > HEARTBEAT_DUMP_AT_S
                    && hb_stall_s.saturating_sub(hb_last_dump_s) >= REDUMP_EVERY_S);
            if should_dump {
                error!(
                    target: "nockchain::watchdog",
                    stall_seconds = hb_stall_s,
                    "libp2p heartbeat has not advanced, dumping thread stacks"
                );
                if dump_thread_stacks(&format!("hb-stall-{hb_stall_s}s")).is_ok() {
                    metrics.watchdog_stack_dump_total.increment();
                }
                hb_last_dump_s = hb_stall_s;
            }
        } else {
            if hb_stall_s >= HEARTBEAT_DUMP_AT_S {
                info!(
                    target: "nockchain::watchdog",
                    previous_stall_seconds = hb_stall_s,
                    "libp2p heartbeat recovered"
                );
            }
            last_tick = now_tick;
            hb_stall_s = 0;
            hb_last_dump_s = 0;
        }

        // --- kernel-operation stall check (catches the 2026-04-18 class) ---
        // Unlike the heartbeat, `poke_last_unix` is a unix timestamp bumped
        // by the traffic cop, so we compare against wall-clock age rather
        // than iterating a counter.
        let poke_age_s = unix_now().saturating_sub(poke_last_unix.load(Ordering::Relaxed));
        // Publish the lag gauge every iteration, letting Datadog graph and
        // alert on `max(poke_completion_lag_seconds) > 180`; this is the
        // external signal-of-record for the 2026-04-18 livelock class.
        let _ = metrics.poke_completion_lag_seconds.swap(poke_age_s as f64);
        if poke_age_s >= POKE_DUMP_AT_S {
            let should_dump = poke_last_dump_s == 0
                || poke_age_s.saturating_sub(poke_last_dump_s) >= REDUMP_EVERY_S;
            if should_dump {
                error!(
                    target: "nockchain::watchdog",
                    poke_stall_seconds = poke_age_s,
                    "no TrafficCop kernel operation has completed, suspected kernel-side livelock; dumping thread stacks"
                );
                if dump_thread_stacks(&format!("poke-stall-{poke_age_s}s")).is_ok() {
                    metrics.watchdog_stack_dump_total.increment();
                }
                poke_last_dump_s = poke_age_s;
            }
        } else if poke_last_dump_s > 0 {
            info!(
                target: "nockchain::watchdog",
                "TrafficCop kernel operations recovered"
            );
            poke_last_dump_s = 0;
        }
    }
}

/// Writes a snapshot of every thread's kernel stack (from
/// `/proc/self/task/<tid>/stack`) along with its state and context-switch
/// counters to a timestamped file. Returns the written path on success.
///
/// `/proc/self/task/*/stack` is read by the kernel from the thread's saved
/// kernel context and returns useful frames even when every user-space
/// thread is parked on a futex, matching the LAX1 freeze shape.
///
/// Caveat (learned on the 2026-04-18 LAX1 stall): on Linux 5.8+ reading
/// `/proc/<tid>/stack` requires `CAP_SYS_PTRACE` even for threads of the
/// same process. The nockchain service runs as a non-privileged user, so
/// the `stack` reads fail with EPERM and we fall back to
/// `/proc/<tid>/wchan` (a single symbol naming the kernel function the
/// thread is waiting in) and `/proc/<tid>/syscall` (current syscall
/// number + registers), both of which remain readable for same-pid
/// threads under the default ptrace-scope. That gives us "which thread
/// is stuck, and on what kernel primitive" even without full frame
/// addresses, enough to distinguish a futex deadlock from a running
/// kernel livelock, as we needed to on 2026-04-18.
#[cfg(target_os = "linux")]
fn dump_thread_stacks(reason: &str) -> std::io::Result<std::path::PathBuf> {
    use std::fs;
    use std::io::Write;

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let base_dir = ["/var/log/nockchain", "/tmp"]
        .iter()
        .find(|p| std::path::Path::new(p).is_dir())
        .copied()
        .unwrap_or("/tmp");
    let path = std::path::PathBuf::from(format!("{base_dir}/nockchain-stacks-{ts}-{reason}.txt"));
    let mut file = fs::File::create(&path)?;
    writeln!(
        file,
        "=== thread stack dump: reason={reason} unix_ts={ts} pid={} ===",
        std::process::id()
    )?;
    match fs::read_dir("/proc/self/task") {
        Ok(tasks) => {
            let mut stack_eperm = 0usize;
            let mut total = 0usize;
            for entry in tasks.flatten() {
                total += 1;
                let tid = entry.file_name().to_string_lossy().into_owned();
                let comm = fs::read_to_string(entry.path().join("comm"))
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let status_raw =
                    fs::read_to_string(entry.path().join("status")).unwrap_or_default();
                let status_line: String = status_raw
                    .lines()
                    .filter(|l| {
                        l.starts_with("State:")
                            || l.starts_with("voluntary_ctxt_switches:")
                            || l.starts_with("nonvoluntary_ctxt_switches:")
                    })
                    .collect::<Vec<_>>()
                    .join(" | ");

                // `wchan` is a single kernel-function symbol the thread is
                // waiting in; far more permission-permissive than `stack`
                // and usually readable by the owning user even without
                // CAP_SYS_PTRACE. Blank for running (R) threads.
                let wchan = fs::read_to_string(entry.path().join("wchan"))
                    .unwrap_or_default()
                    .trim()
                    .to_string();

                // `syscall` reports current syscall number + register args
                // when the thread is in a syscall. `-1` indicates "not
                // currently in a syscall" (running user-space code); we
                // suppress that common case to keep the dump readable.
                let syscall_raw =
                    fs::read_to_string(entry.path().join("syscall")).unwrap_or_default();
                let syscall_line = syscall_raw.lines().next().unwrap_or("").trim().to_string();

                let stack = match fs::read_to_string(entry.path().join("stack")) {
                    Ok(s) => s,
                    Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                        stack_eperm += 1;
                        String::from("<EPERM: needs CAP_SYS_PTRACE; see wchan/syscall above>")
                    }
                    Err(e) => format!("<err reading stack: {e}>"),
                };

                writeln!(file, "\n--- tid={tid} comm={comm} {status_line}")?;
                if !wchan.is_empty() && wchan != "0" {
                    writeln!(file, "wchan: {wchan}")?;
                }
                if !syscall_line.is_empty() && !syscall_line.starts_with("-1 ") {
                    writeln!(file, "syscall: {syscall_line}")?;
                }
                writeln!(file, "stack: {stack}")?;
            }
            if stack_eperm > 0 {
                writeln!(
                    file,
                    "\n=== note: {stack_eperm}/{total} stack reads denied \
                     by kernel; grant CAP_SYS_PTRACE to the service \
                     (AmbientCapabilities=CAP_SYS_PTRACE in the systemd \
                     unit) to capture full frame addresses next dump ==="
                )?;
            }
        }
        Err(e) => writeln!(file, "<err reading /proc/self/task: {e}>")?,
    }
    let _ = dump_fd_inventory(&mut file);
    let _ = dump_socket_queues(&mut file);
    let _ = dump_proc_file(&mut file, "io", "/proc/self/io");
    let _ = dump_proc_file(&mut file, "net/sockstat", "/proc/self/net/sockstat");
    let _ = dump_proc_file(&mut file, "net/sockstat6", "/proc/self/net/sockstat6");
    let _ = dump_proc_file(&mut file, "status (mem + threads)", "/proc/self/status");
    let _ = dump_proc_file(&mut file, "limits", "/proc/self/limits");
    info!(
        target: "nockchain::watchdog",
        reason,
        path = %path.display(),
        "thread stack dump written"
    );
    Ok(path)
}

/// Dump `/proc/self/fd/*` as an inventory of fd number → target symlink,
/// plus a by-type histogram (socket / pipe / eventfd / epoll / timerfd /
/// regular-file / device / anon_inode).
///
/// Motivation: on the 2026-04-18 LAX1 stall we noticed only 22 open FDs,
/// low for a process with 39 connected QUIC peers, but the bare count
/// doesn't tell us whether the FDs are QUIC sockets, leaked eventfds from
/// dropped tasks, or in-flight checkpoint-write handles. A typed
/// inventory distinguishes "waker-fd leak" from "stalled disk write" from
/// "nothing weird, runtime-internal" on inspection.
#[cfg(target_os = "linux")]
fn dump_fd_inventory(file: &mut std::fs::File) -> std::io::Result<()> {
    use std::collections::BTreeMap;
    use std::fs;
    use std::io::Write;

    writeln!(file, "\n=== open file descriptors (/proc/self/fd) ===")?;
    let entries = match fs::read_dir("/proc/self/fd") {
        Ok(e) => e,
        Err(e) => {
            writeln!(file, "<err: {e}>")?;
            return Ok(());
        }
    };
    let mut fds: Vec<(u32, String)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let num = name.parse::<u32>().unwrap_or(u32::MAX);
        let target = fs::read_link(entry.path())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|e| format!("<err: {e}>"));
        fds.push((num, target));
    }
    fds.sort_by_key(|&(n, _)| n);

    writeln!(file, "total: {}", fds.len())?;
    let mut by_type: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (_, t) in &fds {
        let bucket = if t.starts_with("socket:") {
            "socket"
        } else if t.starts_with("pipe:") {
            "pipe"
        } else if t.starts_with("anon_inode:[eventfd]") {
            "eventfd"
        } else if t.starts_with("anon_inode:[eventpoll]") {
            "epoll"
        } else if t.starts_with("anon_inode:[timerfd]") {
            "timerfd"
        } else if t.starts_with("anon_inode:[signalfd]") {
            "signalfd"
        } else if t.starts_with("anon_inode:") {
            "anon_inode-other"
        } else if t.starts_with("/dev/") {
            "device"
        } else if t == "<err: " || t.starts_with("<err") {
            "unreadable"
        } else {
            "regular-file"
        };
        *by_type.entry(bucket).or_default() += 1;
    }
    writeln!(file, "\nby type:")?;
    for (k, v) in &by_type {
        writeln!(file, "  {k:20} : {v}")?;
    }
    writeln!(file, "\ndetail:")?;
    for (n, t) in &fds {
        writeln!(file, "  fd={n:<4} -> {t}")?;
    }
    Ok(())
}

/// Per-socket Recv-Q / Send-Q from /proc/net/{tcp,tcp6,udp,udp6},
/// cross-referenced against the process's own socket inodes from
/// /proc/self/fd/*. Distinguishes "libp2p socket has backed-up packets"
/// (kernel-side buffer full because user-space stopped draining) from
/// "runtime-internal stall". We couldn't tell the difference on the
/// 2026-04-18 LAX1 stall, this lets the next one be unambiguous.
///
/// /proc/net/udp row format (columns):
///   sl local_address rem_address st tx_queue:rx_queue tr:tm->when
///   retrnsmt uid timeout inode ref pointer drops
/// Addresses are hex (IP little-endian, port big-endian). States for UDP
/// are 7=LISTEN (UNCONN in ss), TCP has the usual ESTABLISHED/etc.
#[cfg(target_os = "linux")]
fn dump_socket_queues(file: &mut std::fs::File) -> std::io::Result<()> {
    use std::collections::HashSet;
    use std::fs;
    use std::io::Write;

    writeln!(
        file,
        "\n=== per-socket Recv-Q / Send-Q (owned sockets only) ==="
    )?;

    // 1. Collect our socket inodes from /proc/self/fd/* (link targets of
    //    the form "socket:[NNNN]").
    let mut my_inodes: HashSet<u64> = HashSet::new();
    let mut fd_by_inode: std::collections::BTreeMap<u64, u32> = std::collections::BTreeMap::new();
    if let Ok(entries) = fs::read_dir("/proc/self/fd") {
        for entry in entries.flatten() {
            let fd_num = entry
                .file_name()
                .to_string_lossy()
                .parse::<u32>()
                .unwrap_or(u32::MAX);
            let Ok(target) = fs::read_link(entry.path()) else {
                continue;
            };
            let t = target.to_string_lossy();
            if let Some(inode_str) = t.strip_prefix("socket:[").and_then(|s| s.strip_suffix(']')) {
                if let Ok(inode) = inode_str.parse::<u64>() {
                    my_inodes.insert(inode);
                    fd_by_inode.insert(inode, fd_num);
                }
            }
        }
    }
    writeln!(file, "owned socket inodes: {}", my_inodes.len())?;
    if my_inodes.is_empty() {
        return Ok(());
    }

    // 2. Walk /proc/net/{tcp,tcp6,udp,udp6}, pick out rows whose inode
    //    column matches one we own.
    let tables = [
        ("tcp", "/proc/net/tcp"),
        ("tcp6", "/proc/net/tcp6"),
        ("udp", "/proc/net/udp"),
        ("udp6", "/proc/net/udp6"),
    ];
    writeln!(
        file,
        "\n{:5} {:6} {:>6} {:>6} {:40} {:40} fd",
        "proto", "state", "rx_q", "tx_q", "local", "peer"
    )?;
    for (proto, path) in tables {
        let Ok(body) = fs::read_to_string(path) else {
            continue;
        };
        for line in body.lines().skip(1) {
            // Column-count-stable layout; split_whitespace handles it.
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 10 {
                continue;
            }
            // cols[0] = "N:"    cols[1] = local_addr
            // cols[2] = rem_addr cols[3] = state cols[4] = tx_q:rx_q
            // ... cols[9] = inode (varies between tcp/udp; it's the
            // position after the fields we care about). Scan for an
            // inode that matches any of ours; keep the line if found.
            // The inode is the 10th field in tcp/udp layouts (0-indexed
            // cols[9]).
            let inode_str = cols[9];
            let Ok(inode) = inode_str.parse::<u64>() else {
                continue;
            };
            if !my_inodes.contains(&inode) {
                continue;
            }
            let local = cols[1];
            let peer = cols[2];
            let state_hex = cols[3];
            let qs: Vec<&str> = cols[4].split(':').collect();
            let tx_q_hex = qs.first().copied().unwrap_or("?");
            let rx_q_hex = qs.get(1).copied().unwrap_or("?");
            let tx_q = u64::from_str_radix(tx_q_hex, 16).unwrap_or(0);
            let rx_q = u64::from_str_radix(rx_q_hex, 16).unwrap_or(0);
            let fd = fd_by_inode
                .get(&inode)
                .map(|n| n.to_string())
                .unwrap_or_else(|| String::from("?"));
            writeln!(
                file,
                "{:5} {:6} {:>6} {:>6} {:40} {:40} {}",
                proto, state_hex, rx_q, tx_q, local, peer, fd
            )?;
        }
    }
    Ok(())
}

/// Copy a small `/proc/self/*` file into the dump, with a header.
/// Used for `io`, `net/sockstat`, `net/sockstat6`, `status`, `limits`.
#[cfg(target_os = "linux")]
fn dump_proc_file(file: &mut std::fs::File, label: &str, path: &str) -> std::io::Result<()> {
    use std::fs;
    use std::io::Write;
    writeln!(file, "\n=== {label} ({path}) ===")?;
    match fs::read_to_string(path) {
        Ok(s) => {
            // Elide /proc/self/status's giant "SigCgt:" etc. bitmask section;
            // keep the memory + thread + context fields that matter.
            if path == "/proc/self/status" {
                for line in s.lines() {
                    if line.starts_with("Vm")
                        || line.starts_with("Threads:")
                        || line.starts_with("State:")
                        || line.starts_with("voluntary_")
                        || line.starts_with("nonvoluntary_")
                        || line.starts_with("FDSize:")
                        || line.starts_with("RssAnon:")
                        || line.starts_with("RssFile:")
                        || line.starts_with("RssShmem:")
                    {
                        writeln!(file, "{line}")?;
                    }
                }
            } else {
                write!(file, "{s}")?;
            }
        }
        Err(e) => writeln!(file, "<err: {e}>")?,
    }
    Ok(())
}
