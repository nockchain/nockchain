#![allow(clippy::items_after_test_module)]
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono;
use clap::{Args, ColorChoice, Parser, ValueEnum};
use nockvm::jets::hot::HotEntry;
use nockvm::noun::Atom;
use nockvm::pma::Pma;
use nockvm::trace::{IntervalFilter, KeywordFilter, TraceFilter, TraceInfo, TracingBackend};
use tokio::fs;
use tracing::{debug, info, warn, Level, Subscriber};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
#[cfg(feature = "tracing-tracy")]
use tracing_subscriber::Layer as _;
use tracing_subscriber::{fmt, EnvFilter};

use crate::event_log::{EventLogConfig, ReadySnapshotRecord, ReplayLogEntry};
use crate::export::ExportedState;
use crate::kernel::form::{inspect_existing_pma, ExistingPmaStatus, Kernel, PmaConfig};
use crate::metrics::NockAppMetrics;
use crate::noun::slab::{Jammer, NounSlab};
use crate::save::{CheckpointBootstrapReader, SaveableCheckpoint};
use crate::snapshot::{cleanup_snapshot_artifacts, restore_verified_snapshot, SnapshotManifest};
use crate::utils::error::{CrownError, ExternalError};
use crate::utils::{
    durability, NOCK_STACK_SIZE, NOCK_STACK_SIZE_HUGE, NOCK_STACK_SIZE_LARGE,
    NOCK_STACK_SIZE_MEDIUM, NOCK_STACK_SIZE_SMALL, NOCK_STACK_SIZE_TINY,
};
use crate::{default_data_dir, AtomExt, NockApp};

pub const DEFAULT_GC_INTERVAL_SECS: u64 = 60 * 60;
const DEFAULT_GC_INTERVAL_SECS_STR: &str = "3600";
const DEFAULT_ROTATING_SNAPSHOT_INTERVAL_EVENT_TIME_SECS: u64 = 15 * 60;
const DEFAULT_ROTATING_SNAPSHOT_INTERVAL_EVENT_TIME_SECS_STR: &str = "900";

const DEFAULT_LOG_FILTER: &str = "info";
const NOCK_PMA_INITIAL_WORDS_FOR_REGRESSION_ENV: &str = "NOCK_PMA_INITIAL_WORDS_FOR_REGRESSION";
const DEFAULT_PMA_INITIAL_MIN_BYTES: usize = 256 * 1024 * 1024;
const DEFAULT_PMA_INITIAL_KERNEL_MULTIPLIER: usize = 16;
const WORD_BYTES: usize = std::mem::size_of::<u64>();

#[derive(Debug)]
enum BootSource {
    Pma { path: PathBuf, event_num: u64 },
    Snapshot { path: PathBuf, event_num: u64 },
    Checkpoint { path: PathBuf, event_num: u64 },
    Fresh,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PmaSize {
    words: usize,
}

impl PmaSize {
    pub const fn from_words(words: usize) -> Self {
        Self { words }
    }

    pub const fn from_bytes_ceil(bytes: usize) -> Self {
        Self::from_words(bytes.div_ceil(WORD_BYTES))
    }

    pub const fn words(self) -> usize {
        self.words
    }
}

fn default_pma_initial_words(kernel_jam_bytes: usize) -> usize {
    let target_bytes = kernel_jam_bytes
        .saturating_mul(DEFAULT_PMA_INITIAL_KERNEL_MULTIPLIER)
        .max(DEFAULT_PMA_INITIAL_MIN_BYTES);
    let words = PmaSize::from_bytes_ceil(target_bytes).words().max(1);
    words.checked_next_power_of_two().unwrap_or(words)
}

fn pma_initial_words_for_boot(configured_size: Option<PmaSize>, kernel_jam_bytes: usize) -> usize {
    let configured_words = configured_size
        .map(PmaSize::words)
        .unwrap_or_else(|| default_pma_initial_words(kernel_jam_bytes));
    let Some(value) = std::env::var_os(NOCK_PMA_INITIAL_WORDS_FOR_REGRESSION_ENV) else {
        return configured_words;
    };
    match value.to_string_lossy().parse::<usize>() {
        Ok(words) if words > 0 => {
            warn!(
                configured_words,
                override_words = words,
                env = NOCK_PMA_INITIAL_WORDS_FOR_REGRESSION_ENV,
                "using regression PMA initial-size override"
            );
            words
        }
        _ => {
            warn!(
                configured_words,
                env = NOCK_PMA_INITIAL_WORDS_FOR_REGRESSION_ENV,
                value = %value.to_string_lossy(),
                "ignoring invalid regression PMA initial-size override"
            );
            configured_words
        }
    }
}

fn parse_pma_size(input: &str) -> Result<PmaSize, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("PMA size cannot be empty".to_string());
    }
    let normalized = trimmed.replace('_', "");
    let split_at = normalized
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(normalized.len());
    if split_at == 0 {
        return Err(format!("PMA size '{trimmed}' is missing a numeric value"));
    }
    let value = normalized[..split_at]
        .parse::<u128>()
        .map_err(|err| format!("invalid PMA size '{trimmed}': {err}"))?;
    let suffix = normalized[split_at..].trim().to_ascii_lowercase();
    let multiplier = match suffix.as_str() {
        "" | "b" | "byte" | "bytes" => 1u128,
        "k" | "kb" | "kib" => 1024u128,
        "m" | "mb" | "mib" => 1024u128.pow(2),
        "g" | "gb" | "gib" => 1024u128.pow(3),
        "t" | "tb" | "tib" => 1024u128.pow(4),
        _ => {
            return Err(format!(
                "invalid PMA size suffix '{suffix}' in '{trimmed}'; use bytes, KiB, MiB, GiB, or TiB"
            ));
        }
    };
    let bytes = value
        .checked_mul(multiplier)
        .ok_or_else(|| format!("PMA size '{trimmed}' overflowed"))?;
    let bytes = usize::try_from(bytes)
        .map_err(|_| format!("PMA size '{trimmed}' exceeds this platform's address size"))?;
    let size = PmaSize::from_bytes_ceil(bytes);
    if size.words() == 0 {
        return Err("PMA size must be at least one word".to_string());
    }
    Ok(size)
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum NockStackSize {
    Tiny,
    Small,
    Normal,
    Medium,
    Large,
    Huge,
}

impl NockStackSize {
    pub const fn stack_words(self) -> usize {
        match self {
            Self::Tiny => NOCK_STACK_SIZE_TINY,
            Self::Small => NOCK_STACK_SIZE_SMALL,
            Self::Normal => NOCK_STACK_SIZE,
            Self::Medium => NOCK_STACK_SIZE_MEDIUM,
            Self::Large => NOCK_STACK_SIZE_LARGE,
            Self::Huge => NOCK_STACK_SIZE_HUGE,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum TraceMode {
    Tracing,
}

/// Trace options for NockApp
#[derive(Args, Clone, Debug, Default)]
pub struct TraceOpts {
    /// Enable nock interpreter tracing (integrates with Tracy profiler)
    #[arg(long = "trace", help = "Enable nock interpreter tracing")]
    pub mode: Option<TraceMode>,

    #[arg(long, requires = "mode")]
    pub keyword_filter: Option<String>,

    #[arg(long, requires = "mode")]
    pub interval_filter: Option<usize>,
}

impl From<TraceOpts> for Option<TraceInfo> {
    fn from(trace_opts: TraceOpts) -> Self {
        let keyword_filter = trace_opts
            .keyword_filter
            .map(|v| v.split(",").map(String::from).collect::<Vec<String>>())
            .map(|keywords| KeywordFilter { keywords });
        let interval_filter = trace_opts
            .interval_filter
            .map(|interval| IntervalFilter { interval, cnt: 0 });

        let filter = match (keyword_filter, interval_filter) {
            (Some(a), Some(b)) => Some(a.or(b).boxed()),
            (Some(a), _) => Some(a.boxed()),
            (_, Some(b)) => Some(b.boxed()),
            (None, None) => None,
        };

        trace_opts.mode.map(|_mode| TraceInfo {
            backend: Box::new(TracingBackend::new()),
            filter,
        })
    }
}

#[derive(Parser, Debug, Clone)]
#[command(about = "boot a nockapp", author, version, color = ColorChoice::Auto)]
pub struct Cli {
    #[arg(
        long,
        help = "Start with a fresh data directory, aborting if the target already contains data",
        default_value = "false"
    )]
    pub new: bool,

    #[command(flatten)]
    pub trace_opts: TraceOpts,

    #[arg(
        long,
        help = "Set the PMA GC interval (in seconds). Use 'none' or '0' to disable PMA GC.",
        default_value = DEFAULT_GC_INTERVAL_SECS_STR,
        value_parser = parse_optional_u64
    )]
    pub gc_interval: Option<u64>,

    #[arg(
        long,
        help = "Set the rotating snapshot interval in cumulative event-processing seconds. Use 'none' or '0' to disable rotating snapshots.",
        default_value = DEFAULT_ROTATING_SNAPSHOT_INTERVAL_EVENT_TIME_SECS_STR,
        value_parser = parse_optional_u64
    )]
    pub rotating_snapshot_interval_event_time: Option<u64>,

    #[arg(
        long,
        help = "Run with in-memory NockStack state only, disabling PMA durability, event logs, snapshots, and GC."
    )]
    pub ephemeral: bool,

    #[arg(long, help = "Control colored output", value_enum, default_value_t = ColorChoice::Auto)]
    pub color: ColorChoice,

    #[arg(
        long,
        requires = "new",
        help = "Path to a jam file containing existing kernel state. Supports both JammedCheckpoint and ExportedState formats."
    )]
    pub state_jam: Option<String>,
    #[arg(
        long,
        help = "Path to a jammed checkpoint (.chkjam) to bootstrap from. Copies into the checkpoints dir as 0.chkjam before boot. Conflicts with --state-jam and --export-state-jam.",
        conflicts_with_all = &["state_jam", "export_state_jam"]
    )]
    pub bootstrap_from_chkjam: Option<String>,

    #[arg(
        long,
        help = "Path to export the kernel state as a jam file in the ExportedState format."
    )]
    pub export_state_jam: Option<String>,

    #[arg(
        long,
        help = "Nock stack size to use",
        value_enum,
        default_value_t = NockStackSize::Normal
    )]
    pub stack_size: NockStackSize,

    #[arg(
        long,
        help = "Initial file-backed PMA capacity. Accepts byte units like 256MiB or 4GiB. Defaults to auto: round_up_power_of_two(max(256MiB, kernel_jam_bytes * 16)).",
        value_parser = parse_pma_size
    )]
    pub pma_initial_size: Option<PmaSize>,

    #[arg(
        long,
        help = "Maximum virtual PMA reservation. Accepts byte units like 64GiB or 1TiB. Defaults to 1TiB, or NOCK_PMA_RESERVED_WORDS when set.",
        value_parser = parse_pma_size
    )]
    pub pma_reserved_size: Option<PmaSize>,
    #[arg(
        long,
        help = "Override the full data directory for this nockapp instance (expects the directory that contains checkpoints/)"
    )]
    pub data_dir: Option<PathBuf>,
    #[arg(long, help = "Override the SQLite event-log path")]
    pub event_log_path: Option<PathBuf>,

    #[arg(
        long,
        help = "Disable all fsync/fdatasync calls (including SQLite FULL-sync durability)"
    )]
    pub disable_fsync: bool,
}

impl Cli {
    fn normalized_gc_interval(&self) -> Option<Duration> {
        self.gc_interval.and_then(|value| {
            if value == 0 {
                None
            } else {
                Some(Duration::from_secs(value))
            }
        })
    }

    fn normalized_rotating_snapshot_interval_event_time(&self) -> Option<Duration> {
        self.rotating_snapshot_interval_event_time
            .and_then(|value| {
                if value == 0 {
                    None
                } else {
                    Some(Duration::from_secs(value))
                }
            })
    }
}

fn parse_optional_u64(input: &str) -> Result<u64, String> {
    let trimmed = input.trim();

    if trimmed.eq_ignore_ascii_case("none") {
        Ok(0)
    } else {
        let value = trimmed
            .parse::<u64>()
            .map_err(|e| format!("Invalid value '{trimmed}': {e}"))?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::Duration;

    use clap::Parser;
    use nockvm::jets::util::slot;
    use nockvm::noun::{CellHandle, NounAllocator, NounSpace, D, T};
    use nockvm_macros::tas;
    use rusqlite::Connection;
    use tempfile::TempDir;

    use super::{
        default_boot_cli, default_pma_initial_words, export_kernel_state, parse_optional_u64,
        parse_pma_size, select_boot_state, setup_, BootEventLogPolicy, BootSelection, SetupResult,
        DEFAULT_GC_INTERVAL_SECS,
    };
    use crate::metrics::NockAppMetrics;
    use crate::nockapp::wire::{wire_to_noun, SystemWire, Wire};
    use crate::noun::slab::{slab_equality, NockJammer, NounSlab};
    use crate::save::SaveableCheckpoint;
    use crate::snapshot::SnapshotManifest;
    use crate::NockApp;

    fn load_test_jam_bytes() -> Vec<u8> {
        let possible_paths = [
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("test-jams")
                .join("test-ker.jam"),
            Path::new("open/crates/nockapp/test-jams").join("test-ker.jam"),
            Path::new("test-jams").join("test-ker.jam"),
        ];

        possible_paths
            .iter()
            .find_map(|path| fs::read(path).ok())
            .expect("read test kernel")
    }

    fn inc_poke() -> NounSlab {
        let mut slab = NounSlab::new();
        let space = NounSpace::empty();
        slab.copy_into(D(tas!(b"inc")), &space);
        slab
    }

    fn state_peek() -> NounSlab {
        let mut slab = NounSlab::new();
        let peek = T(&mut slab, &[D(tas!(b"state")), D(0)]);
        slab.set_root(peek);
        slab
    }

    fn atom_slab(value: u64) -> NounSlab {
        let mut slab = NounSlab::new();
        let space = NounSpace::empty();
        slab.copy_into(D(value), &space);
        slab
    }

    async fn assert_counter_state(app: &mut NockApp<NockJammer>, expected: u64) {
        assert_eq!(
            app.kernel.serf.event_number.load(Ordering::SeqCst),
            expected,
            "booted event number must match the committed SQLite boundary"
        );
        let mut actual = app.kernel.peek(state_peek()).await.expect("peek state");
        let actual_space = actual.noun_space();
        actual.modify_noun(|root| {
            let cell = slot(root, 7, &actual_space)
                .expect("peek state result cell")
                .as_cell()
                .expect("peek state result payload cell");
            CellHandle::new(cell, &actual_space).tail().noun()
        });
        let expected_slab = atom_slab(expected);
        assert!(
            slab_equality(&actual, &expected_slab),
            "counter state did not match committed event boundary: actual={actual:?}, expected={expected_slab:?}"
        );
    }

    fn logged_poke_job_jam(event_num: u64, cause: u64) -> Vec<u8> {
        let mut slab = NounSlab::<NockJammer>::new();
        let base_wire = wire_to_noun(&mut slab, &SystemWire.to_wire());
        let wire = T(&mut slab, &[D(tas!(b"poke")), base_wire]);
        let job = T(&mut slab, &[D(event_num), wire, D(0), D(0), D(0), D(cause)]);
        slab.set_root(job);
        slab.jam().to_vec()
    }

    fn durable_test_boot_cli(new: bool) -> super::Cli {
        let mut cli = default_boot_cli(new);
        cli.gc_interval = None;
        cli.rotating_snapshot_interval_event_time = None;
        cli.disable_fsync = true;
        cli
    }

    async fn setup_test_app(data_dir: &Path) -> NockApp<NockJammer> {
        try_setup_test_app(data_dir, None)
            .await
            .expect("setup boot test app")
    }

    async fn setup_test_app_from_chkjam(
        data_dir: &Path,
        chkjam_path: &Path,
    ) -> NockApp<NockJammer> {
        let jam = load_test_jam_bytes();
        let mut cli = durable_test_boot_cli(false);
        cli.data_dir = Some(data_dir.to_path_buf());
        cli.bootstrap_from_chkjam = Some(chkjam_path.to_string_lossy().into_owned());
        match setup_::<NockJammer>(&jam, cli, &[], "boot-test", None)
            .await
            .expect("setup boot test app from checkpoint")
        {
            SetupResult::App(app) => app,
            SetupResult::ExportedState => panic!("unexpected export"),
        }
    }

    async fn try_setup_test_app(
        data_dir: &Path,
        rotating_snapshot_interval_event_time_secs: Option<u64>,
    ) -> Result<NockApp<NockJammer>, Box<dyn std::error::Error>> {
        let jam = load_test_jam_bytes();
        let mut cli = durable_test_boot_cli(false);
        cli.data_dir = Some(data_dir.to_path_buf());
        cli.rotating_snapshot_interval_event_time = rotating_snapshot_interval_event_time_secs;
        Ok(
            match setup_::<NockJammer>(&jam, cli, &[], "boot-test", None).await? {
                SetupResult::App(app) => app,
                SetupResult::ExportedState => panic!("unexpected export"),
            },
        )
    }

    async fn try_setup_test_app_with_gc_interval(
        data_dir: &Path,
        gc_interval_secs: Option<u64>,
    ) -> Result<NockApp<NockJammer>, Box<dyn std::error::Error>> {
        let jam = load_test_jam_bytes();
        let mut cli = durable_test_boot_cli(false);
        cli.data_dir = Some(data_dir.to_path_buf());
        cli.gc_interval = gc_interval_secs;
        Ok(
            match setup_::<NockJammer>(&jam, cli, &[], "boot-test", None).await? {
                SetupResult::App(app) => app,
                SetupResult::ExportedState => panic!("unexpected export"),
            },
        )
    }

    async fn write_checkpoint_bootstrap_fixture(app: &NockApp<NockJammer>, data_dir: &Path) {
        let checkpoint: SaveableCheckpoint =
            app.kernel.checkpoint().await.expect("checkpoint saveable");
        let jammed = checkpoint.to_jammed_checkpoint::<NockJammer>();
        let bytes = jammed.encode().expect("encode checkpoint");
        fs::write(data_dir.join("checkpoints").join("0.chkjam"), bytes)
            .expect("write checkpoint fixture");
    }

    async fn poke_inc(app: &NockApp<NockJammer>) {
        app.kernel
            .poke(SystemWire.to_wire(), inc_poke())
            .await
            .expect("poke inc");
    }

    async fn wait_for_serf_idle(app: &NockApp<NockJammer>) {
        app.export().await.expect("export barrier");
    }

    async fn stop_app(app: &mut NockApp<NockJammer>) {
        app.kernel.serf.stop().await.expect("stop kernel");
    }

    fn clear_pma_files(data_dir: &Path) {
        let pma_dir = data_dir.join("pma");
        for file_name in ["0.pma", "1.pma", "0.meta", "1.meta"] {
            let path = pma_dir.join(file_name);
            if path.exists() {
                fs::remove_file(path).expect("remove PMA artifact");
            }
        }
    }

    fn copy_runtime_pma_files(from_dir: &Path, to_dir: &Path) {
        fs::create_dir_all(to_dir).expect("create PMA backup dir");
        for file_name in ["0.pma", "1.pma", "0.meta", "1.meta"] {
            let from = from_dir.join(file_name);
            if from.exists() {
                fs::copy(&from, to_dir.join(file_name)).expect("copy PMA artifact");
            }
        }
    }

    fn restore_runtime_pma_files(data_dir: &Path, backup_dir: &Path) {
        clear_pma_files(data_dir);
        let pma_dir = data_dir.join("pma");
        for file_name in ["0.pma", "1.pma", "0.meta", "1.meta"] {
            let backup = backup_dir.join(file_name);
            if backup.exists() {
                fs::copy(&backup, pma_dir.join(file_name)).expect("restore PMA artifact");
            }
        }
    }

    fn ready_snapshot_count(data_dir: &Path) -> i64 {
        let conn =
            Connection::open(data_dir.join("event-log.sqlite3")).expect("open event log sqlite");
        conn.query_row(
            "SELECT COUNT(1) FROM snapshots WHERE state = 'ready'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("count ready snapshots")
    }

    fn ready_rotating_snapshots(data_dir: &Path) -> Vec<(i64, String, String, i64)> {
        let conn =
            Connection::open(data_dir.join("event-log.sqlite3")).expect("open event log sqlite");
        let mut stmt = conn
            .prepare(
                "SELECT snapshot_id, pma_path, manifest_path, event_num FROM snapshots WHERE state = 'ready' AND kind = 'rotating' ORDER BY timestamp_tag DESC",
            )
            .expect("prepare rotating snapshots query");
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .expect("query rotating snapshots");
        rows.collect::<Result<Vec<_>, _>>()
            .expect("collect rotating snapshots")
    }

    fn ready_snapshots(data_dir: &Path) -> Vec<(i64, String, String, String, i64)> {
        let conn =
            Connection::open(data_dir.join("event-log.sqlite3")).expect("open event log sqlite");
        let mut stmt = conn
            .prepare(
                "SELECT snapshot_id, kind, pma_path, manifest_path, event_num FROM snapshots WHERE state = 'ready' ORDER BY event_num DESC, snapshot_id DESC",
            )
            .expect("prepare ready snapshots query");
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .expect("query ready snapshots");
        rows.collect::<Result<Vec<_>, _>>()
            .expect("collect ready snapshots")
    }

    fn retain_only_snapshot_for_test(data_dir: &Path, snapshot_id: i64) {
        let conn =
            Connection::open(data_dir.join("event-log.sqlite3")).expect("open event log sqlite");
        let stale_paths = {
            let mut stmt = conn
                .prepare("SELECT pma_path, manifest_path FROM snapshots WHERE snapshot_id != ?1")
                .expect("prepare stale snapshot path query");
            let rows = stmt
                .query_map([snapshot_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .expect("query stale snapshot paths");
            rows.collect::<Result<Vec<_>, _>>()
                .expect("collect stale snapshot paths")
        };
        for (pma_path, manifest_path) in stale_paths {
            for path in [PathBuf::from(pma_path), PathBuf::from(manifest_path)] {
                if path.exists() {
                    fs::remove_file(path).expect("remove stale snapshot artifact");
                }
            }
        }
        conn.execute(
            "DELETE FROM snapshots WHERE snapshot_id != ?1",
            [snapshot_id],
        )
        .expect("delete stale snapshots");
        set_active_snapshot_id_for_test(data_dir, snapshot_id);
    }

    fn retired_rotating_snapshot_count(data_dir: &Path) -> i64 {
        let conn =
            Connection::open(data_dir.join("event-log.sqlite3")).expect("open event log sqlite");
        conn.query_row(
            "SELECT COUNT(1) FROM snapshots WHERE state = 'retired' AND kind = 'rotating'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("count retired rotating snapshots")
    }

    fn active_snapshot_id_for_test(data_dir: &Path) -> Option<i64> {
        let conn =
            Connection::open(data_dir.join("event-log.sqlite3")).expect("open event log sqlite");
        conn.query_row(
            "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'active_snapshot_id'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .ok()
    }

    fn set_active_snapshot_id_for_test(data_dir: &Path, snapshot_id: i64) {
        let conn =
            Connection::open(data_dir.join("event-log.sqlite3")).expect("open event log sqlite");
        conn.execute(
            r#"
INSERT INTO meta (key, value)
VALUES ('active_snapshot_id', ?1)
ON CONFLICT(key) DO UPDATE SET value = excluded.value
"#,
            [snapshot_id],
        )
        .expect("set active snapshot id");
    }

    fn snapshot_state_for_test(data_dir: &Path, snapshot_id: i64) -> String {
        let conn =
            Connection::open(data_dir.join("event-log.sqlite3")).expect("open event log sqlite");
        conn.query_row(
            "SELECT state FROM snapshots WHERE snapshot_id = ?1",
            [snapshot_id],
            |row| row.get::<_, String>(0),
        )
        .expect("snapshot state")
    }

    fn clear_ready_snapshots_for_test(data_dir: &Path) {
        let conn =
            Connection::open(data_dir.join("event-log.sqlite3")).expect("open event log sqlite");
        conn.execute("DELETE FROM snapshots", [])
            .expect("delete snapshots");
        conn.execute("DELETE FROM meta WHERE key = 'active_snapshot_id'", [])
            .expect("delete active snapshot id");
    }

    fn max_event_num_for_test(data_dir: &Path) -> Option<i64> {
        let conn =
            Connection::open(data_dir.join("event-log.sqlite3")).expect("open event log sqlite");
        conn.query_row("SELECT max(event_num) FROM events", [], |row| {
            row.get::<_, Option<i64>>(0)
        })
        .expect("read max event num")
    }

    fn insert_event_job_for_test(data_dir: &Path, event_num: u64, job_jam: Vec<u8>) {
        let conn =
            Connection::open(data_dir.join("event-log.sqlite3")).expect("open event log sqlite");
        conn.execute(
            r#"
INSERT INTO events (
  event_num,
  job_jam,
  wire_source,
  wire_version,
  wire_tags_json,
  cause_hash,
  job_hash,
  event_processing_duration_us,
  created_at_ms
) VALUES (?1, ?2, 'sys', 1, '[]', ?3, ?4, 0, 42)
"#,
            (
                i64::try_from(event_num).expect("event num fits in sqlite"),
                job_jam,
                vec![0_u8; 32],
                vec![1_u8; 32],
            ),
        )
        .expect("insert event job");
    }

    fn rewrite_snapshot_manifest_ker_hash(manifest_path: &Path, ker_hash: [u8; 32]) {
        let manifest =
            SnapshotManifest::read_from_path(manifest_path).expect("read snapshot manifest");
        let rewritten = SnapshotManifest::new(
            manifest.kind,
            manifest.timestamp_tag.clone(),
            blake3::Hash::from_bytes(ker_hash),
            manifest.event_num,
            manifest.pma_words,
            manifest.alloc_words,
            manifest.kernel_root_raw,
            manifest.cold_offset,
            blake3::Hash::from_bytes(manifest.used_blake3),
            manifest.structure_blake3.map(blake3::Hash::from_bytes),
            manifest.created_at_ms,
        )
        .expect("rewrite snapshot manifest");
        rewritten
            .write_to_path(manifest_path)
            .expect("write rewritten snapshot manifest");
    }

    fn set_event_processing_duration_for_test(data_dir: &Path, event_num: u64, duration: Duration) {
        let conn =
            Connection::open(data_dir.join("event-log.sqlite3")).expect("open event log sqlite");
        let duration_us =
            i64::try_from(duration.as_micros()).expect("event processing duration fits in i64");
        conn.execute(
            "UPDATE events SET event_processing_duration_us = ?1 WHERE event_num = ?2",
            (
                duration_us,
                i64::try_from(event_num).expect("event num fits in i64"),
            ),
        )
        .expect("update event processing duration");
    }

    #[test]
    fn parse_optional_u64_none_variants() {
        assert_eq!(parse_optional_u64("none").unwrap(), 0);
        assert_eq!(parse_optional_u64("NoNe").unwrap(), 0);
        assert_eq!(parse_optional_u64("0").unwrap(), 0);
        assert_eq!(parse_optional_u64(" 0 ").unwrap(), 0);
    }

    #[test]
    fn parse_optional_u64_positive_values() {
        assert_eq!(parse_optional_u64("1").unwrap(), 1);
        assert_eq!(parse_optional_u64(" 900 ").unwrap(), 900);
    }

    #[test]
    fn parse_optional_u64_rejects_invalid() {
        assert!(parse_optional_u64("abc").is_err());
    }

    #[test]
    fn parse_pma_size_accepts_binary_byte_units() {
        assert_eq!(parse_pma_size("1").unwrap().words(), 1);
        assert_eq!(parse_pma_size("8").unwrap().words(), 1);
        assert_eq!(parse_pma_size("256MiB").unwrap().words(), 32 * 1024 * 1024);
        assert_eq!(parse_pma_size("1GiB").unwrap().words(), 128 * 1024 * 1024);
        assert_eq!(
            parse_pma_size("1_TiB").unwrap().words(),
            128 * 1024 * 1024 * 1024
        );
    }

    #[test]
    fn default_pma_initial_size_uses_kernel_jam_size_floor_and_power_of_two() {
        assert_eq!(
            default_pma_initial_words(12 * 1024 * 1024),
            32 * 1024 * 1024
        );
        assert_eq!(
            default_pma_initial_words(18 * 1024 * 1024),
            64 * 1024 * 1024
        );
    }

    #[test]
    fn pma_size_cli_decouples_from_stack_size() {
        let parsed = super::Cli::try_parse_from([
            "boot-test", "--stack-size", "large", "--pma-initial-size", "512MiB",
            "--pma-reserved-size", "2TiB",
        ])
        .expect("parse PMA sizing cli");
        assert!(matches!(parsed.stack_size, super::NockStackSize::Large));
        assert_eq!(parsed.pma_initial_size.unwrap().words(), 64 * 1024 * 1024);
        assert_eq!(
            parsed.pma_reserved_size.unwrap().words(),
            256 * 1024 * 1024 * 1024
        );
    }

    #[test]
    fn normalized_rotating_snapshot_interval_event_time_filters_zero() {
        let mut cli = super::default_boot_cli(false);
        cli.rotating_snapshot_interval_event_time = Some(0);
        assert_eq!(cli.normalized_rotating_snapshot_interval_event_time(), None);

        cli.rotating_snapshot_interval_event_time = Some(5);
        assert_eq!(
            cli.normalized_rotating_snapshot_interval_event_time(),
            Some(Duration::from_secs(5))
        );
    }

    #[test]
    fn normalized_gc_interval_defaults_enabled_and_filters_zero() {
        let mut cli = super::default_boot_cli(false);
        assert_eq!(
            cli.normalized_gc_interval(),
            Some(Duration::from_secs(DEFAULT_GC_INTERVAL_SECS))
        );

        cli.gc_interval = Some(0);
        assert_eq!(cli.normalized_gc_interval(), None);

        cli.gc_interval = None;
        assert_eq!(cli.normalized_gc_interval(), None);
    }

    #[test]
    fn gc_interval_cli_defaults_enabled_and_allows_disable() {
        let parsed = super::Cli::try_parse_from(["boot-test"]).expect("parse default cli");
        assert_eq!(
            parsed.normalized_gc_interval(),
            Some(Duration::from_secs(DEFAULT_GC_INTERVAL_SECS))
        );

        let parsed = super::Cli::try_parse_from(["boot-test", "--gc-interval", "none"])
            .expect("parse disabled gc interval");
        assert_eq!(parsed.normalized_gc_interval(), None);

        let parsed = super::Cli::try_parse_from(["boot-test", "--gc-interval", "0"])
            .expect("parse zero gc interval");
        assert_eq!(parsed.normalized_gc_interval(), None);

        let parsed = super::Cli::try_parse_from(["boot-test", "--gc-interval", "1"])
            .expect("parse one-second gc interval");
        assert_eq!(
            parsed.normalized_gc_interval(),
            Some(Duration::from_secs(1))
        );
    }

    #[test]
    fn ephemeral_test_boot_cli_disables_durable_side_effects() {
        let cli = super::ephemeral_test_boot_cli(true);
        assert!(cli.new);
        assert!(cli.ephemeral);
        assert_eq!(cli.gc_interval, None);
        assert_eq!(cli.rotating_snapshot_interval_event_time, None);
        assert!(cli.disable_fsync);
    }

    #[test]
    fn durable_test_boot_cli_keeps_pma_with_fast_io_defaults() {
        let cli = durable_test_boot_cli(false);
        assert!(!cli.new);
        assert!(!cli.ephemeral);
        assert_eq!(cli.gc_interval, None);
        assert_eq!(cli.rotating_snapshot_interval_event_time, None);
        assert!(cli.disable_fsync);
    }

    #[test]
    fn state_jam_cli_requires_new() {
        let err =
            super::Cli::try_parse_from(["boot-test", "--state-jam", "/tmp/state.jam"]).unwrap_err();
        assert!(
            err.to_string().contains("--new"),
            "expected clap error to mention --new, got: {err}"
        );

        let parsed =
            super::Cli::try_parse_from(["boot-test", "--new", "--state-jam", "/tmp/state.jam"])
                .expect("parse with --new");
        assert!(parsed.new);
        assert_eq!(parsed.state_jam.as_deref(), Some("/tmp/state.jam"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn setup_rejects_state_jam_without_new_for_programmatic_callers() {
        let temp = TempDir::new().expect("tempdir");
        let mut cli = super::default_boot_cli(false);
        cli.state_jam = Some(temp.path().join("state.jam").display().to_string());

        let err =
            match setup_::<NockJammer>(&[], cli, &[], "boot-test", Some(temp.path().to_path_buf()))
                .await
            {
                Ok(_) => panic!("setup should reject state_jam without --new"),
                Err(err) => err,
            };

        assert!(
            err.to_string().contains("--state-jam requires --new"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn export_state_jam_creates_parent_dir() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("export-state-jam-data");
        let export_path = temp
            .path()
            .join("nested")
            .join("backup")
            .join("kernel.state.jam");

        let mut app = setup_test_app(&data_dir).await;
        wait_for_serf_idle(&app).await;

        export_kernel_state::<NockJammer, _>(
            &app.kernel,
            export_path.to_str().expect("export path string"),
        )
        .await
        .expect("export state jam");

        assert!(export_path.exists());
        assert!(export_path.parent().expect("export parent").exists());

        stop_app(&mut app).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn setup_rejects_new_when_data_dir_is_nonempty() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("existing-data-dir");
        let checkpoints_dir = data_dir.join("checkpoints");
        let checkpoint_path = checkpoints_dir.join("existing.chkjam");
        fs::create_dir_all(&checkpoints_dir).expect("create checkpoints dir");
        fs::write(&checkpoint_path, b"keep").expect("write existing checkpoint");

        let jam = load_test_jam_bytes();
        let mut cli = default_boot_cli(true);
        cli.data_dir = Some(data_dir.clone());

        let err = match setup_::<NockJammer>(&jam, cli, &[], "boot-test", None).await {
            Ok(_) => panic!("setup should reject --new for a non-empty data dir"),
            Err(err) => err,
        };

        assert!(
            err.to_string()
                .contains("--new requires an empty data directory"),
            "unexpected error: {err}"
        );
        assert_eq!(
            fs::read(&checkpoint_path).expect("read existing checkpoint"),
            b"keep"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn setup_allows_new_for_empty_data_dir() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("fresh-empty-dir");
        fs::create_dir_all(&data_dir).expect("create empty data dir");

        let jam = load_test_jam_bytes();
        let mut cli = durable_test_boot_cli(true);
        cli.data_dir = Some(data_dir.clone());

        let mut app = match setup_::<NockJammer>(&jam, cli, &[], "boot-test", None)
            .await
            .expect("setup should allow --new for an empty data dir")
        {
            SetupResult::App(app) => app,
            SetupResult::ExportedState => panic!("unexpected export"),
        };

        assert!(data_dir.join("checkpoints").exists());
        assert!(data_dir.join("pma").exists());
        stop_app(&mut app).await;
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn bootstraps_pma_from_checkpoint_once() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("bootstrapped-pma");

        let mut first = setup_test_app(&data_dir).await;
        poke_inc(&first).await;
        write_checkpoint_bootstrap_fixture(&first, &data_dir).await;
        stop_app(&mut first).await;
        drop(first);
        assert_eq!(ready_snapshot_count(&data_dir), 1);
        fs::remove_file(data_dir.join("pma").join("epoch.pma")).expect("remove epoch pma");
        fs::remove_file(data_dir.join("pma").join("epoch.manifest"))
            .expect("remove epoch manifest");
        clear_ready_snapshots_for_test(&data_dir);
        clear_pma_files(&data_dir);

        let mut second = setup_test_app(&data_dir).await;
        assert_eq!(second.kernel.serf.event_number.load(Ordering::SeqCst), 1);
        assert!(
            data_dir.join("pma").join("0.meta").exists()
                || data_dir.join("pma").join("1.meta").exists()
        );
        assert!(data_dir.join("pma").join("epoch.pma").exists());
        assert!(data_dir.join("pma").join("epoch.manifest").exists());
        assert_eq!(ready_snapshot_count(&data_dir), 1);
        poke_inc(&second).await;
        assert_eq!(second.kernel.serf.event_number.load(Ordering::SeqCst), 2);
        stop_app(&mut second).await;
        drop(second);
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn bootstraps_explicit_checkpoint_copy_into_empty_event_log() {
        let temp = TempDir::new().expect("tempdir");
        let source_data_dir = temp.path().join("checkpoint-source");
        let copied_data_dir = temp.path().join("checkpoint-copy-empty-event-log");

        let mut first = setup_test_app(&source_data_dir).await;
        poke_inc(&first).await;
        assert_counter_state(&mut first, 1).await;
        write_checkpoint_bootstrap_fixture(&first, &source_data_dir).await;
        stop_app(&mut first).await;
        drop(first);

        fs::create_dir_all(copied_data_dir.join("checkpoints")).expect("create checkpoint dir");
        fs::copy(
            source_data_dir.join("checkpoints").join("0.chkjam"),
            copied_data_dir.join("checkpoints").join("0.chkjam"),
        )
        .expect("copy checkpoint into fresh data dir");

        let implicit_err = match try_setup_test_app(&copied_data_dir, None).await {
            Ok(mut app) => {
                stop_app(&mut app).await;
                panic!("implicit checkpoint bootstrap unexpectedly succeeded")
            }
            Err(err) => err,
        };
        assert!(
            implicit_err
                .to_string()
                .contains("SQLite event log is empty or missing"),
            "unexpected implicit checkpoint bootstrap error: {implicit_err}"
        );

        let mut second = setup_test_app_from_chkjam(
            &copied_data_dir,
            &source_data_dir.join("checkpoints").join("0.chkjam"),
        )
        .await;
        assert_counter_state(&mut second, 1).await;
        assert_eq!(max_event_num_for_test(&copied_data_dir), None);
        stop_app(&mut second).await;
        drop(second);

        let mut third = setup_test_app(&copied_data_dir).await;
        assert_counter_state(&mut third, 1).await;
        assert_eq!(max_event_num_for_test(&copied_data_dir), None);
        poke_inc(&third).await;
        assert_counter_state(&mut third, 2).await;
        assert_eq!(max_event_num_for_test(&copied_data_dir), Some(2));
        stop_app(&mut third).await;
        drop(third);
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn valid_pma_skips_corrupt_checkpoint_files() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("skip-corrupt-checkpoint");

        let mut first = setup_test_app(&data_dir).await;
        poke_inc(&first).await;
        write_checkpoint_bootstrap_fixture(&first, &data_dir).await;
        stop_app(&mut first).await;
        drop(first);
        clear_pma_files(&data_dir);

        let mut second = setup_test_app(&data_dir).await;
        poke_inc(&second).await;
        assert_eq!(second.kernel.serf.event_number.load(Ordering::SeqCst), 2);
        stop_app(&mut second).await;
        drop(second);

        let checkpoints_dir = data_dir.join("checkpoints");
        fs::write(checkpoints_dir.join("0.chkjam"), b"corrupt checkpoint 0").expect("corrupt chk0");
        fs::write(checkpoints_dir.join("1.chkjam"), b"corrupt checkpoint 1").expect("corrupt chk1");

        let boot_selection: BootSelection = select_boot_state::<NockJammer>(
            &data_dir.join("checkpoints"),
            &load_test_jam_bytes(),
            &data_dir.join("event-log.sqlite3"),
            &data_dir.join("pma").join("0.pma"),
            &data_dir.join("pma").join("1.pma"),
            Arc::new(NockAppMetrics::default()),
            BootEventLogPolicy {
                preexisting: true,
                allow_empty_bootstrap: false,
            },
        )
        .await
        .expect("select boot state");
        assert!(boot_selection.checkpoint.is_none());
        assert!(boot_selection.pma_open_existing);
        assert!(boot_selection.snapshot_manifest.is_none());
        assert!(boot_selection.replay_jobs.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn valid_pma_with_unopenable_event_log_fails_closed() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("valid-pma-unopenable-event-log");

        let mut first = setup_test_app(&data_dir).await;
        poke_inc(&first).await;
        assert_counter_state(&mut first, 1).await;
        stop_app(&mut first).await;
        drop(first);

        let event_log_path = data_dir.join("event-log.sqlite3");
        fs::remove_file(&event_log_path).expect("remove event log sqlite");
        fs::create_dir(&event_log_path).expect("replace event log sqlite with directory");

        let result = select_boot_state::<NockJammer>(
            &data_dir.join("checkpoints"),
            &load_test_jam_bytes(),
            &event_log_path,
            &data_dir.join("pma").join("0.pma"),
            &data_dir.join("pma").join("1.pma"),
            Arc::new(NockAppMetrics::default()),
            BootEventLogPolicy {
                preexisting: true,
                allow_empty_bootstrap: false,
            },
        )
        .await;

        match result {
            Ok(_) => panic!("boot selection trusted PMA even though SQLite could not be opened"),
            Err(err) => assert!(
                err.to_string().contains("event log"),
                "unexpected boot selection error: {err}"
            ),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn pma_ahead_of_event_log_recovers_to_sqlite_boundary() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("pma-ahead-of-event-log");

        let mut first = setup_test_app(&data_dir).await;
        poke_inc(&first).await;
        poke_inc(&first).await;
        assert_eq!(first.kernel.serf.event_number.load(Ordering::SeqCst), 2);
        stop_app(&mut first).await;
        drop(first);

        let conn =
            Connection::open(data_dir.join("event-log.sqlite3")).expect("open event log sqlite");
        conn.execute("DELETE FROM events WHERE event_num = 2", [])
            .expect("delete event 2");
        drop(conn);
        assert_eq!(max_event_num_for_test(&data_dir), Some(1));

        let mut second = setup_test_app(&data_dir).await;
        assert_counter_state(&mut second, 1).await;
        stop_app(&mut second).await;
        drop(second);
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn pma_lagging_event_log_replays_to_sqlite_boundary() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("pma-lagging-event-log");
        let pma_dir = data_dir.join("pma");
        let event_1_pma_backup = temp.path().join("event-1-pma-backup");

        let mut first = setup_test_app(&data_dir).await;
        poke_inc(&first).await;
        assert_eq!(first.kernel.serf.event_number.load(Ordering::SeqCst), 1);
        stop_app(&mut first).await;
        drop(first);
        copy_runtime_pma_files(&pma_dir, &event_1_pma_backup);

        let mut second = setup_test_app(&data_dir).await;
        assert_eq!(second.kernel.serf.event_number.load(Ordering::SeqCst), 1);
        poke_inc(&second).await;
        assert_eq!(second.kernel.serf.event_number.load(Ordering::SeqCst), 2);
        stop_app(&mut second).await;
        drop(second);

        assert_eq!(max_event_num_for_test(&data_dir), Some(2));
        restore_runtime_pma_files(&data_dir, &event_1_pma_backup);

        let mut third = setup_test_app(&data_dir).await;
        assert_counter_state(&mut third, 2).await;
        stop_app(&mut third).await;
        drop(third);
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn checkpoint_behind_event_log_replays_to_sqlite_boundary() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("checkpoint-behind-event-log");

        let mut first = setup_test_app(&data_dir).await;
        poke_inc(&first).await;
        assert_counter_state(&mut first, 1).await;
        write_checkpoint_bootstrap_fixture(&first, &data_dir).await;
        stop_app(&mut first).await;
        drop(first);

        let mut second = setup_test_app(&data_dir).await;
        assert_counter_state(&mut second, 1).await;
        poke_inc(&second).await;
        assert_counter_state(&mut second, 2).await;
        stop_app(&mut second).await;
        drop(second);

        assert_eq!(max_event_num_for_test(&data_dir), Some(2));
        clear_ready_snapshots_for_test(&data_dir);
        clear_pma_files(&data_dir);

        let mut third = setup_test_app(&data_dir).await;
        assert_counter_state(&mut third, 2).await;
        stop_app(&mut third).await;
        drop(third);
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn checkpoint_ahead_of_event_log_recovers_to_sqlite_boundary() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("checkpoint-ahead-event-log");

        let mut first = setup_test_app(&data_dir).await;
        poke_inc(&first).await;
        assert_counter_state(&mut first, 1).await;
        stop_app(&mut first).await;
        drop(first);

        let mut second = setup_test_app(&data_dir).await;
        assert_counter_state(&mut second, 1).await;
        poke_inc(&second).await;
        assert_counter_state(&mut second, 2).await;
        write_checkpoint_bootstrap_fixture(&second, &data_dir).await;
        stop_app(&mut second).await;
        drop(second);

        let conn =
            Connection::open(data_dir.join("event-log.sqlite3")).expect("open event log sqlite");
        conn.execute("DELETE FROM events WHERE event_num = 2", [])
            .expect("delete uncommitted event");
        drop(conn);
        assert_eq!(max_event_num_for_test(&data_dir), Some(1));
        clear_ready_snapshots_for_test(&data_dir);
        clear_pma_files(&data_dir);

        let mut third = setup_test_app(&data_dir).await;
        assert_counter_state(&mut third, 1).await;
        stop_app(&mut third).await;
        drop(third);
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn fresh_boot_with_committed_event_log_replays_from_zero() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("fresh-replay-from-event-log");

        let mut first = setup_test_app(&data_dir).await;
        stop_app(&mut first).await;
        drop(first);

        clear_ready_snapshots_for_test(&data_dir);
        clear_pma_files(&data_dir);
        insert_event_job_for_test(&data_dir, 1, logged_poke_job_jam(1, tas!(b"inc")));

        let mut second = setup_test_app(&data_dir).await;
        assert_counter_state(&mut second, 1).await;
        stop_app(&mut second).await;
        drop(second);
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn replay_rejected_logged_event_fails_instead_of_synthesizing_crud() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("replay-rejected-logged-event");

        let mut first = setup_test_app(&data_dir).await;
        stop_app(&mut first).await;
        drop(first);

        insert_event_job_for_test(&data_dir, 1, logged_poke_job_jam(1, tas!(b"bad")));
        clear_pma_files(&data_dir);

        let err = match try_setup_test_app(&data_dir, None).await {
            Ok(_) => panic!("replay should fail instead of synthesizing a replacement event"),
            Err(err) => err,
        };

        let err_text = err.to_string();
        assert!(
            err_text.contains("event replay after snapshot restore")
                || err_text.to_ascii_lowercase().contains("kernel error"),
            "unexpected replay error: {err}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn snapshot_kernel_hash_mismatch_loads_state_like_checkpoint() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("snapshot-kernel-hash-mismatch");

        let mut first = setup_test_app(&data_dir).await;
        poke_inc(&first).await;
        assert_counter_state(&mut first, 1).await;
        stop_app(&mut first).await;
        drop(first);
        set_event_processing_duration_for_test(&data_dir, 1, Duration::from_secs(1));
        clear_pma_files(&data_dir);

        let mut second = try_setup_test_app(&data_dir, Some(1))
            .await
            .expect("setup app for rotating snapshot");
        assert_counter_state(&mut second, 1).await;
        poke_inc(&second).await;
        wait_for_serf_idle(&second).await;
        assert_counter_state(&mut second, 2).await;
        stop_app(&mut second).await;
        drop(second);

        let rotating = ready_rotating_snapshots(&data_dir);
        assert!(!rotating.is_empty(), "expected a rotating snapshot");
        let (snapshot_id, _pma_path, manifest_path, event_num) = &rotating[0];
        assert_eq!(*event_num, 2);
        retain_only_snapshot_for_test(&data_dir, *snapshot_id);
        rewrite_snapshot_manifest_ker_hash(Path::new(manifest_path), [0x42; 32]);
        clear_pma_files(&data_dir);

        let ready = ready_snapshots(&data_dir);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].0, *snapshot_id);

        let mut third = setup_test_app(&data_dir).await;
        assert_counter_state(&mut third, 2).await;
        stop_app(&mut third).await;
        drop(third);
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn restores_epoch_snapshot_when_pma_is_missing() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("restore-epoch-snapshot");

        let mut first = setup_test_app(&data_dir).await;
        poke_inc(&first).await;
        stop_app(&mut first).await;
        drop(first);
        clear_pma_files(&data_dir);

        let mut second = setup_test_app(&data_dir).await;
        assert_eq!(second.kernel.serf.event_number.load(Ordering::SeqCst), 1);
        stop_app(&mut second).await;
        drop(second);

        clear_pma_files(&data_dir);
        let checkpoints_dir = data_dir.join("checkpoints");
        fs::write(checkpoints_dir.join("0.chkjam"), b"corrupt checkpoint 0").expect("corrupt chk0");
        fs::write(checkpoints_dir.join("1.chkjam"), b"corrupt checkpoint 1").expect("corrupt chk1");

        let mut third = setup_test_app(&data_dir).await;
        assert_eq!(third.kernel.serf.event_number.load(Ordering::SeqCst), 1);
        assert!(data_dir.join("pma").join("0.meta").exists());
        stop_app(&mut third).await;
        drop(third);
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn shutdown_flush_rewrites_missing_active_pma_metadata() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("shutdown-rewrites-pma-meta");

        let mut first = setup_test_app(&data_dir).await;
        poke_inc(&first).await;
        for meta_name in ["0.meta", "1.meta"] {
            let meta_path = data_dir.join("pma").join(meta_name);
            if meta_path.exists() {
                fs::remove_file(&meta_path).expect("remove active meta before shutdown");
            }
            assert!(!meta_path.exists());
        }

        stop_app(&mut first).await;
        drop(first);

        assert!(
            data_dir.join("pma").join("0.meta").exists()
                || data_dir.join("pma").join("1.meta").exists()
        );
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn pma_gc_switches_slabs_and_rebuilds_runtime_state() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("pma-gc-slab-switch");
        fs::create_dir_all(&data_dir).expect("create data dir");

        let mut first = try_setup_test_app_with_gc_interval(&data_dir, Some(1))
            .await
            .expect("setup with forced GC interval");
        tokio::time::sleep(Duration::from_secs(1) + Duration::from_millis(50)).await;
        poke_inc(&first).await;
        wait_for_serf_idle(&first).await;

        assert!(
            data_dir.join("pma").join("1.meta").exists(),
            "GC should publish metadata for the alternate slab"
        );
        assert!(
            !data_dir.join("pma").join("0.meta").exists(),
            "GC should invalidate the old active slab metadata before switching slabs"
        );
        assert_counter_state(&mut first, 1).await;
        stop_app(&mut first).await;

        let mut second = setup_test_app(&data_dir).await;
        assert_counter_state(&mut second, 1).await;
        stop_app(&mut second).await;
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn replays_logged_events_after_snapshot_restore() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("replay-after-snapshot-restore");

        let mut first = setup_test_app(&data_dir).await;
        poke_inc(&first).await;
        stop_app(&mut first).await;
        drop(first);
        clear_pma_files(&data_dir);

        let mut second = setup_test_app(&data_dir).await;
        assert_eq!(second.kernel.serf.event_number.load(Ordering::SeqCst), 1);
        poke_inc(&second).await;
        assert_eq!(second.kernel.serf.event_number.load(Ordering::SeqCst), 2);
        stop_app(&mut second).await;
        drop(second);

        clear_pma_files(&data_dir);
        let checkpoints_dir = data_dir.join("checkpoints");
        fs::write(checkpoints_dir.join("0.chkjam"), b"corrupt checkpoint 0").expect("corrupt chk0");
        fs::write(checkpoints_dir.join("1.chkjam"), b"corrupt checkpoint 1").expect("corrupt chk1");

        let mut third = setup_test_app(&data_dir).await;
        assert_eq!(third.kernel.serf.event_number.load(Ordering::SeqCst), 2);
        stop_app(&mut third).await;
        drop(third);
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn refuses_boot_on_event_log_gap_after_snapshot() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("gap-after-snapshot");

        let mut first = setup_test_app(&data_dir).await;
        poke_inc(&first).await;
        stop_app(&mut first).await;
        drop(first);
        clear_pma_files(&data_dir);

        let mut second = setup_test_app(&data_dir).await;
        poke_inc(&second).await;
        poke_inc(&second).await;
        stop_app(&mut second).await;
        drop(second);

        let conn =
            Connection::open(data_dir.join("event-log.sqlite3")).expect("open event log sqlite");
        conn.execute("DELETE FROM events WHERE event_num = 2", [])
            .expect("delete event");

        clear_pma_files(&data_dir);
        let checkpoints_dir = data_dir.join("checkpoints");
        fs::write(checkpoints_dir.join("0.chkjam"), b"corrupt checkpoint 0").expect("corrupt chk0");
        fs::write(checkpoints_dir.join("1.chkjam"), b"corrupt checkpoint 1").expect("corrupt chk1");

        let err = match try_setup_test_app(&data_dir, None).await {
            Ok(_) => panic!("boot should fail on continuity gap"),
            Err(err) => err,
        };
        assert!(err
            .to_string()
            .contains("event log continuity check failed"));
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn rotates_snapshots_and_retires_oldest() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("rotating-snapshot-retention");

        let mut first = setup_test_app(&data_dir).await;
        poke_inc(&first).await;
        wait_for_serf_idle(&first).await;
        stop_app(&mut first).await;
        drop(first);
        set_event_processing_duration_for_test(&data_dir, 1, Duration::from_secs(1));
        clear_pma_files(&data_dir);

        let mut second = try_setup_test_app(&data_dir, Some(1))
            .await
            .expect("setup rotating snapshot retention app");
        poke_inc(&second).await;
        poke_inc(&second).await;
        wait_for_serf_idle(&second).await;
        stop_app(&mut second).await;
        drop(second);
        set_event_processing_duration_for_test(&data_dir, 3, Duration::from_secs(1));
        clear_pma_files(&data_dir);

        let mut third = try_setup_test_app(&data_dir, Some(1))
            .await
            .expect("setup rotating snapshot retention app after event 3");
        poke_inc(&third).await;
        poke_inc(&third).await;
        wait_for_serf_idle(&third).await;
        stop_app(&mut third).await;
        drop(third);
        set_event_processing_duration_for_test(&data_dir, 5, Duration::from_secs(1));
        clear_pma_files(&data_dir);

        let mut fourth = try_setup_test_app(&data_dir, Some(1))
            .await
            .expect("setup rotating snapshot retention app after event 5");
        poke_inc(&fourth).await;
        wait_for_serf_idle(&fourth).await;
        stop_app(&mut fourth).await;
        drop(fourth);

        let rotating = ready_rotating_snapshots(&data_dir);
        assert_eq!(rotating.len(), 2);
        assert_eq!(rotating[0].3, 6);
        assert_eq!(rotating[1].3, 4);
        assert_eq!(retired_rotating_snapshot_count(&data_dir), 1);
        for (_, pma_path, manifest_path, _) in rotating {
            assert!(Path::new(&pma_path).exists());
            assert!(Path::new(&manifest_path).exists());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn falls_back_from_corrupt_newest_rotating_snapshot() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("rotating-fallback");

        let mut first = setup_test_app(&data_dir).await;
        poke_inc(&first).await;
        wait_for_serf_idle(&first).await;
        stop_app(&mut first).await;
        drop(first);
        set_event_processing_duration_for_test(&data_dir, 1, Duration::from_secs(1));
        clear_pma_files(&data_dir);

        let mut second = try_setup_test_app(&data_dir, Some(1))
            .await
            .expect("setup rotating fallback app");
        poke_inc(&second).await;
        poke_inc(&second).await;
        wait_for_serf_idle(&second).await;
        stop_app(&mut second).await;
        drop(second);
        set_event_processing_duration_for_test(&data_dir, 3, Duration::from_secs(1));
        clear_pma_files(&data_dir);

        let mut third = try_setup_test_app(&data_dir, Some(1))
            .await
            .expect("setup rotating fallback app after event 3");
        poke_inc(&third).await;
        wait_for_serf_idle(&third).await;
        assert_eq!(third.kernel.serf.event_number.load(Ordering::SeqCst), 4);
        stop_app(&mut third).await;
        drop(third);

        let rotating = ready_rotating_snapshots(&data_dir);
        assert_eq!(rotating.len(), 2);
        fs::write(&rotating[0].1, b"corrupt newest rotating snapshot")
            .expect("corrupt newest rotating pma");

        clear_pma_files(&data_dir);
        let checkpoints_dir = data_dir.join("checkpoints");
        fs::write(checkpoints_dir.join("0.chkjam"), b"corrupt checkpoint 0").expect("corrupt chk0");
        fs::write(checkpoints_dir.join("1.chkjam"), b"corrupt checkpoint 1").expect("corrupt chk1");

        let mut fourth = setup_test_app(&data_dir).await;
        assert_eq!(fourth.kernel.serf.event_number.load(Ordering::SeqCst), 4);
        stop_app(&mut fourth).await;
        drop(fourth);
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn falls_back_from_manifest_only_corruption() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("manifest-only-fallback");

        let mut first = setup_test_app(&data_dir).await;
        poke_inc(&first).await;
        wait_for_serf_idle(&first).await;
        stop_app(&mut first).await;
        drop(first);
        set_event_processing_duration_for_test(&data_dir, 1, Duration::from_secs(1));
        clear_pma_files(&data_dir);

        let mut second = try_setup_test_app(&data_dir, Some(1))
            .await
            .expect("setup manifest-only fallback app");
        poke_inc(&second).await;
        poke_inc(&second).await;
        wait_for_serf_idle(&second).await;
        stop_app(&mut second).await;
        drop(second);
        set_event_processing_duration_for_test(&data_dir, 3, Duration::from_secs(1));
        clear_pma_files(&data_dir);

        let mut third = try_setup_test_app(&data_dir, Some(1))
            .await
            .expect("setup manifest-only fallback app after event 3");
        poke_inc(&third).await;
        wait_for_serf_idle(&third).await;
        assert_eq!(third.kernel.serf.event_number.load(Ordering::SeqCst), 4);
        stop_app(&mut third).await;
        drop(third);

        let rotating = ready_rotating_snapshots(&data_dir);
        assert_eq!(rotating.len(), 2);
        fs::write(&rotating[0].2, b"corrupt newest rotating manifest")
            .expect("corrupt newest rotating manifest");

        clear_pma_files(&data_dir);
        let checkpoints_dir = data_dir.join("checkpoints");
        fs::write(checkpoints_dir.join("0.chkjam"), b"corrupt checkpoint 0").expect("corrupt chk0");
        fs::write(checkpoints_dir.join("1.chkjam"), b"corrupt checkpoint 1").expect("corrupt chk1");

        let mut fourth = setup_test_app(&data_dir).await;
        assert_eq!(fourth.kernel.serf.event_number.load(Ordering::SeqCst), 4);
        stop_app(&mut fourth).await;
        drop(fourth);
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn honors_active_snapshot_selection_before_ordering() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("active-snapshot-selection");

        let mut first = setup_test_app(&data_dir).await;
        poke_inc(&first).await;
        wait_for_serf_idle(&first).await;
        stop_app(&mut first).await;
        drop(first);
        set_event_processing_duration_for_test(&data_dir, 1, Duration::from_secs(1));
        clear_pma_files(&data_dir);

        let mut second = try_setup_test_app(&data_dir, Some(1))
            .await
            .expect("setup active snapshot selection app");
        poke_inc(&second).await;
        poke_inc(&second).await;
        wait_for_serf_idle(&second).await;
        stop_app(&mut second).await;
        drop(second);
        set_event_processing_duration_for_test(&data_dir, 3, Duration::from_secs(1));
        clear_pma_files(&data_dir);

        let mut third = try_setup_test_app(&data_dir, Some(1))
            .await
            .expect("setup active snapshot selection app after event 3");
        poke_inc(&third).await;
        wait_for_serf_idle(&third).await;
        assert_eq!(third.kernel.serf.event_number.load(Ordering::SeqCst), 4);
        stop_app(&mut third).await;
        drop(third);

        let rotating = ready_rotating_snapshots(&data_dir);
        assert_eq!(rotating.len(), 2);
        let older_snapshot_id = rotating[1].0;
        let newer_snapshot_id = rotating[0].0;
        set_active_snapshot_id_for_test(&data_dir, older_snapshot_id);
        fs::write(&rotating[1].1, b"corrupt active rotating snapshot")
            .expect("corrupt active rotating pma");

        clear_pma_files(&data_dir);
        let checkpoints_dir = data_dir.join("checkpoints");
        fs::write(checkpoints_dir.join("0.chkjam"), b"corrupt checkpoint 0").expect("corrupt chk0");
        fs::write(checkpoints_dir.join("1.chkjam"), b"corrupt checkpoint 1").expect("corrupt chk1");

        let mut fourth = setup_test_app(&data_dir).await;
        assert_eq!(fourth.kernel.serf.event_number.load(Ordering::SeqCst), 4);
        stop_app(&mut fourth).await;
        drop(fourth);

        assert_eq!(
            snapshot_state_for_test(&data_dir, older_snapshot_id),
            "failed"
        );
        assert_eq!(
            active_snapshot_id_for_test(&data_dir),
            Some(newer_snapshot_id)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn moves_orphan_snapshot_files_to_corrupted_pma() {
        let temp = TempDir::new().expect("tempdir");
        let data_dir = temp.path().join("orphan-snapshot-cleanup");

        let mut first = setup_test_app(&data_dir).await;
        poke_inc(&first).await;
        wait_for_serf_idle(&first).await;
        stop_app(&mut first).await;
        drop(first);
        set_event_processing_duration_for_test(&data_dir, 1, Duration::from_secs(1));
        clear_pma_files(&data_dir);

        let mut second = try_setup_test_app(&data_dir, Some(1))
            .await
            .expect("setup orphan cleanup app");
        poke_inc(&second).await;
        assert_eq!(second.kernel.serf.event_number.load(Ordering::SeqCst), 2);
        stop_app(&mut second).await;
        drop(second);

        let pma_dir = data_dir.join("pma");
        let orphan_pma = pma_dir.join("snap-orphan.pma");
        let orphan_manifest = pma_dir.join("snap-orphan.manifest");
        let orphan_pma_tmp = pma_dir.join("snap-orphan.pma.tmp");
        let orphan_manifest_tmp = pma_dir.join("snap-orphan.manifest.tmp");
        fs::write(&orphan_pma, b"orphan pma").expect("write orphan pma");
        fs::write(&orphan_manifest, b"orphan manifest").expect("write orphan manifest");
        fs::write(&orphan_pma_tmp, b"orphan pma tmp").expect("write orphan pma tmp");
        fs::write(&orphan_manifest_tmp, b"orphan manifest tmp").expect("write orphan manifest tmp");

        let boot_selection: BootSelection = select_boot_state::<NockJammer>(
            &data_dir.join("checkpoints"),
            &load_test_jam_bytes(),
            &data_dir.join("event-log.sqlite3"),
            &data_dir.join("pma").join("0.pma"),
            &data_dir.join("pma").join("1.pma"),
            Arc::new(NockAppMetrics::default()),
            BootEventLogPolicy {
                preexisting: true,
                allow_empty_bootstrap: false,
            },
        )
        .await
        .expect("select boot state");
        assert!(boot_selection.checkpoint.is_none());
        assert!(boot_selection.pma_open_existing);
        assert!(boot_selection.snapshot_manifest.is_none());
        assert!(boot_selection.replay_jobs.is_empty());

        let corrupted_dir = pma_dir.join("corrupted_pma");
        assert!(!orphan_pma.exists());
        assert!(!orphan_manifest.exists());
        assert!(!orphan_pma_tmp.exists());
        assert!(!orphan_manifest_tmp.exists());
        assert!(corrupted_dir.join("snap-orphan.pma").exists());
        assert!(corrupted_dir.join("snap-orphan.manifest").exists());
        assert!(corrupted_dir.join("snap-orphan.pma.tmp").exists());
        assert!(corrupted_dir.join("snap-orphan.manifest.tmp").exists());
    }
}

/// Result of setting up a NockApp
#[allow(clippy::large_enum_variant)]
pub enum SetupResult<J> {
    /// A fully initialized NockApp
    App(NockApp<J>),
    /// State was exported successfully
    ExportedState,
}

struct BootSelection {
    checkpoint: Option<SaveableCheckpoint>,
    pma_open_existing: bool,
    snapshot_manifest: Option<SnapshotManifest>,
    replay_jobs: Vec<ReplayLogEntry>,
}

#[derive(Clone, Copy)]
struct BootEventLogPolicy {
    preexisting: bool,
    allow_empty_bootstrap: bool,
}

fn order_snapshot_candidates(
    active_snapshot_id: Option<i64>,
    ready_snapshots: Vec<ReadySnapshotRecord>,
) -> Vec<ReadySnapshotRecord> {
    if let Some(active_snapshot_id) = active_snapshot_id {
        if let Some(active_idx) = ready_snapshots
            .iter()
            .position(|snapshot| snapshot.snapshot_id == active_snapshot_id)
        {
            let mut ordered = Vec::with_capacity(ready_snapshots.len());
            ordered.push(ready_snapshots[active_idx].clone());
            ordered.extend(
                ready_snapshots
                    .into_iter()
                    .filter(|snapshot| snapshot.snapshot_id != active_snapshot_id),
            );
            return ordered;
        }
    }
    ready_snapshots
}

async fn select_boot_state<J: Jammer>(
    jams_dir: &Path,
    kernel_bytes: &[u8],
    event_log_path: &Path,
    pma_path_0: &Path,
    pma_path_1: &Path,
    metrics: Arc<NockAppMetrics>,
    event_log_policy: BootEventLogPolicy,
) -> Result<BootSelection, CrownError<ExternalError>> {
    let expected_ker_hash = {
        let mut hasher = blake3::Hasher::new();
        hasher.update(kernel_bytes);
        hasher.finalize()
    };
    let existing_pma = Some(inspect_existing_pma(pma_path_0, pma_path_1, kernel_bytes));
    let mut recovery_event_log = match crate::event_log::EventLog::open(EventLogConfig {
        path: event_log_path.to_path_buf(),
    }) {
        Ok(mut event_log) => {
            let cleanup_start = std::time::Instant::now();
            if let Err(err) = cleanup_snapshot_artifacts(
                &mut event_log,
                pma_path_0
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new(".")),
            ) {
                metrics.snapshot_cleanup_failures.increment();
                warn!("snapshot cleanup failed during boot: {err}");
            } else {
                metrics
                    .snapshot_cleanup
                    .add_timing(&cleanup_start.elapsed());
            }
            Some(event_log)
        }
        Err(err) => {
            if !matches!(existing_pma.as_ref(), Some(ExistingPmaStatus::Missing)) {
                return Err(CrownError::Unknown(format!(
                    "event log could not be opened while PMA artifacts exist: {err}"
                )));
            }
            warn!("recovery skipped because event log could not be opened: {err}");
            None
        }
    };

    if let Some(event_log) = recovery_event_log.as_mut() {
        if let Err(err) = event_log.quick_check() {
            return Err(CrownError::Unknown(format!(
                "event log quick_check failed during snapshot recovery: {err}"
            )));
        }
    }
    let event_log_max = recovery_event_log
        .as_mut()
        .map(|event_log| {
            event_log
                .max_event_num()
                .map_err(|err| {
                    CrownError::Unknown(format!(
                        "failed to read max event number from event log: {err}"
                    ))
                })
                .map(|max| max.unwrap_or(0))
        })
        .transpose()?
        .unwrap_or(0);

    if let Some(ExistingPmaStatus::Valid { path, event_num }) = existing_pma.as_ref() {
        if event_log_max == 0 && *event_num > 0 {
            if !event_log_policy.preexisting && !event_log_policy.allow_empty_bootstrap {
                return Err(CrownError::Unknown(format!(
                    "refusing to boot PMA event_num={} because the SQLite event log was missing; restore the event log or use explicit bootstrap/discard intent",
                    event_num
                )));
            }
            let boot_source = BootSource::Pma {
                path: path.clone(),
                event_num: *event_num,
            };
            match &boot_source {
                BootSource::Pma { path, event_num } => {
                    info!(
                        "Boot source: PMA path={} event_num={} event_log_max=0 empty_event_log_bootstrap=true",
                        path.display(),
                        event_num
                    );
                }
                _ => unreachable!(),
            }
            return Ok(BootSelection {
                checkpoint: None,
                pma_open_existing: true,
                snapshot_manifest: None,
                replay_jobs: Vec::new(),
            });
        }
        match event_num.cmp(&event_log_max) {
            std::cmp::Ordering::Equal => {
                let boot_source = BootSource::Pma {
                    path: path.clone(),
                    event_num: *event_num,
                };
                match &boot_source {
                    BootSource::Pma { path, event_num } => {
                        info!(
                            "Boot source: PMA path={} event_num={}",
                            path.display(),
                            event_num
                        );
                    }
                    _ => unreachable!(),
                }
                return Ok(BootSelection {
                    checkpoint: None,
                    pma_open_existing: true,
                    snapshot_manifest: None,
                    replay_jobs: Vec::new(),
                });
            }
            std::cmp::Ordering::Greater => {
                warn!(
                    "Ignoring PMA at {} event_num={} because it is ahead of event log max {}",
                    path.display(),
                    event_num,
                    event_log_max
                );
            }
            std::cmp::Ordering::Less => {
                warn!(
                    "Ignoring PMA at {} event_num={} because it is behind event log max {}",
                    path.display(),
                    event_num,
                    event_log_max
                );
            }
        }
    }

    match existing_pma.as_ref() {
        Some(ExistingPmaStatus::Invalid { path, reason }) => {
            warn!("Ignoring invalid PMA at {}: {}", path.display(), reason);
        }
        Some(ExistingPmaStatus::Missing) => {
            info!("No valid PMA found; checking snapshot and checkpoint recovery");
        }
        None => {}
        Some(ExistingPmaStatus::Valid { .. }) => {}
    }

    if let Some(event_log) = recovery_event_log.as_mut() {
        let active_snapshot_id = event_log.active_snapshot_id().map_err(|err| {
            CrownError::Unknown(format!(
                "failed to read active_snapshot_id from event log: {err}"
            ))
        })?;
        let ready_snapshots = event_log.list_ready_snapshots().map_err(|err| {
            CrownError::Unknown(format!(
                "failed to list ready snapshots from event log: {err}"
            ))
        })?;
        for snapshot in order_snapshot_candidates(active_snapshot_id, ready_snapshots) {
            if snapshot.event_num > event_log_max {
                warn!(
                    "Snapshot {} event_num={} is ahead of event log max {}; marking failed",
                    snapshot.pma_path, snapshot.event_num, event_log_max
                );
                let _ = event_log.mark_snapshot_failed(snapshot.snapshot_id);
                continue;
            }
            let replay_entries =
                event_log
                    .replay_events_after(snapshot.event_num)
                    .map_err(|err| {
                        CrownError::Unknown(format!(
                    "event log continuity check failed from snapshot {} event_num={}: {err}",
                    snapshot.pma_path, snapshot.event_num
                ))
                    })?;
            let replay_reaches_event_log_max = replay_entries
                .last()
                .map(|entry| entry.event_num)
                .unwrap_or(snapshot.event_num)
                == event_log_max;
            if !replay_reaches_event_log_max {
                warn!(
                    "Snapshot {} event_num={} does not replay to event log max {}; marking failed",
                    snapshot.pma_path, snapshot.event_num, event_log_max
                );
                let _ = event_log.mark_snapshot_failed(snapshot.snapshot_id);
                continue;
            }
            let verify_start = std::time::Instant::now();
            match restore_verified_snapshot(&snapshot, pma_path_0) {
                Ok(manifest) => {
                    metrics.snapshot_verify.add_timing(&verify_start.elapsed());
                    if manifest.ker_hash != *expected_ker_hash.as_bytes() {
                        warn!(
                            "Snapshot {} kernel hash mismatch; loading snapshot state into current kernel",
                            snapshot.pma_path
                        );
                    }
                    let _ = event_log.set_active_snapshot_id(snapshot.snapshot_id);
                    for stale in [
                        pma_path_1.to_path_buf(),
                        pma_path_0.with_extension("meta"),
                        pma_path_1.with_extension("meta"),
                    ] {
                        if stale.exists() {
                            let _ = std::fs::remove_file(&stale);
                        }
                    }
                    let boot_source = BootSource::Snapshot {
                        path: PathBuf::from(&snapshot.pma_path),
                        event_num: snapshot.event_num,
                    };
                    match &boot_source {
                        BootSource::Snapshot { path, event_num } => info!(
                            "Boot source: snapshot path={} event_num={}",
                            path.display(),
                            event_num
                        ),
                        _ => unreachable!(),
                    }
                    return Ok(BootSelection {
                        checkpoint: None,
                        pma_open_existing: true,
                        snapshot_manifest: Some(manifest),
                        replay_jobs: replay_entries,
                    });
                }
                Err(err) => {
                    metrics.snapshot_verify_failures.increment();
                    warn!(
                        "Snapshot restore failed for {}: {}; marking snapshot failed",
                        snapshot.pma_path, err
                    );
                    let _ = event_log.mark_snapshot_failed(snapshot.snapshot_id);
                }
            }
        }
    }

    let checkpoint_reader = CheckpointBootstrapReader::<J>::new(jams_dir.to_path_buf());
    let checkpoint_candidate = checkpoint_reader
        .load_latest_state_only_with_summary(None)
        .await
        .map_err(|err| match existing_pma.as_ref() {
            Some(ExistingPmaStatus::Invalid { path, reason }) => CrownError::Unknown(format!(
                "checkpoint bootstrap inspection failed after PMA validation failed for {}: {} ({err})",
                path.display(),
                reason
            )),
            Some(ExistingPmaStatus::Missing) => {
                CrownError::Unknown(format!("checkpoint bootstrap inspection failed: {err}"))
            }
            None => CrownError::Unknown(format!("checkpoint bootstrap inspection failed: {err}")),
            Some(ExistingPmaStatus::Valid { .. }) => {
                CrownError::Unknown(format!("checkpoint bootstrap inspection failed after PMA was rejected against event log: {err}"))
            }
        })?;

    if let Some((checkpoint, summary)) = checkpoint_candidate {
        let checkpoint_bootstraps_empty_event_log = recovery_event_log.is_some()
            && event_log_max == 0
            && event_log_policy.allow_empty_bootstrap;
        if recovery_event_log.is_some()
            && event_log_max == 0
            && summary.event_num > 0
            && !event_log_policy.allow_empty_bootstrap
        {
            return Err(CrownError::Unknown(format!(
                "refusing to boot checkpoint {} event_num={} because the SQLite event log is empty or missing; use --new/--bootstrap-from-chkjam for explicit checkpoint bootstrap",
                summary.path.display(),
                summary.event_num
            )));
        }
        if summary.event_num <= event_log_max || checkpoint_bootstraps_empty_event_log {
            let replay_entries = if checkpoint_bootstraps_empty_event_log {
                Vec::new()
            } else if let Some(event_log) = recovery_event_log.as_mut() {
                event_log
                    .replay_events_after(summary.event_num)
                    .map_err(|err| {
                        CrownError::Unknown(format!(
                            "event log continuity check failed from checkpoint {} event_num={}: {err}",
                            summary.path.display(),
                            summary.event_num
                        ))
                    })?
            } else {
                Vec::new()
            };
            let replay_reaches_event_log_max = replay_entries
                .last()
                .map(|entry| entry.event_num)
                .unwrap_or(summary.event_num)
                == event_log_max
                || checkpoint_bootstraps_empty_event_log;
            if replay_reaches_event_log_max {
                let boot_source = BootSource::Checkpoint {
                    path: summary.path.clone(),
                    event_num: summary.event_num,
                };
                match &boot_source {
                    BootSource::Checkpoint { path, event_num } => {
                        info!(
                            "Boot source: checkpoint path={} event_num={} event_log_max={} empty_event_log_bootstrap={} checkpoint_cold_state=empty",
                            path.display(),
                            event_num,
                            event_log_max,
                            checkpoint_bootstraps_empty_event_log
                        );
                    }
                    _ => unreachable!(),
                }
                return Ok(BootSelection {
                    checkpoint: Some(checkpoint),
                    pma_open_existing: false,
                    snapshot_manifest: None,
                    replay_jobs: replay_entries,
                });
            }
            warn!(
                "Checkpoint {} event_num={} did not replay to event log max {}; ignoring",
                summary.path.display(),
                summary.event_num,
                event_log_max
            );
        } else {
            warn!(
                "Ignoring checkpoint {} event_num={} because it is ahead of event log max {}",
                summary.path.display(),
                summary.event_num,
                event_log_max
            );
        }
    }

    if event_log_max > 0 {
        if let Some(event_log) = recovery_event_log.as_mut() {
            let replay_entries = event_log.replay_events_after(0).map_err(|err| {
                CrownError::Unknown(format!(
                    "event log continuity check failed from fresh boot event_num=0: {err}"
                ))
            })?;
            let replay_reaches_event_log_max = replay_entries
                .last()
                .map(|entry| entry.event_num)
                .unwrap_or(0)
                == event_log_max;
            if replay_reaches_event_log_max {
                let boot_source = BootSource::Fresh;
                match boot_source {
                    BootSource::Fresh => {
                        info!("Boot source: fresh kernel state with event-log replay")
                    }
                    _ => unreachable!(),
                }
                return Ok(BootSelection {
                    checkpoint: None,
                    pma_open_existing: false,
                    snapshot_manifest: None,
                    replay_jobs: replay_entries,
                });
            }
        }
        return Err(CrownError::Unknown(format!(
            "no valid boot base can recover event log max {event_log_max}"
        )));
    }

    let boot_source = BootSource::Fresh;
    match boot_source {
        BootSource::Fresh => info!("Boot source: fresh kernel state"),
        _ => unreachable!(),
    }
    Ok(BootSelection {
        checkpoint: None,
        pma_open_existing: false,
        snapshot_manifest: None,
        replay_jobs: Vec::new(),
    })
}

pub fn default_boot_cli(new: bool) -> Cli {
    Cli {
        gc_interval: Some(DEFAULT_GC_INTERVAL_SECS),
        rotating_snapshot_interval_event_time: Some(
            DEFAULT_ROTATING_SNAPSHOT_INTERVAL_EVENT_TIME_SECS,
        ),
        ephemeral: false,
        new,
        trace_opts: Default::default(),
        color: ColorChoice::Auto,
        state_jam: None,
        bootstrap_from_chkjam: None,
        export_state_jam: None,
        stack_size: NockStackSize::Normal,
        pma_initial_size: None,
        pma_reserved_size: None,
        data_dir: None,
        event_log_path: None,
        disable_fsync: false,
    }
}

pub fn ephemeral_test_boot_cli(new: bool) -> Cli {
    let mut cli = default_boot_cli(new);
    cli.ephemeral = true;
    cli.gc_interval = None;
    cli.rotating_snapshot_interval_event_time = None;
    cli.disable_fsync = true;
    cli
}

fn dir_has_entries(path: &std::path::Path) -> std::io::Result<bool> {
    if !path.exists() {
        return Ok(false);
    }

    if !path.is_dir() {
        return Ok(true);
    }

    let mut entries = std::fs::read_dir(path)?;
    Ok(entries.next().transpose()?.is_some())
}

fn event_log_sidecar_paths(event_log_path: &std::path::Path) -> [PathBuf; 3] {
    [
        event_log_path.to_path_buf(),
        PathBuf::from(format!("{}-wal", event_log_path.display())),
        PathBuf::from(format!("{}-shm", event_log_path.display())),
    ]
}

/// A minimal event formatter for development mode
struct MinimalFormatter;

impl<S, N> FormatEvent<S, N> for MinimalFormatter
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        let level = *event.metadata().level();
        let level_str = match level {
            Level::TRACE => "\x1B[36mT\x1B[0m",
            Level::DEBUG => "\x1B[34mD\x1B[0m",
            Level::INFO => "\x1B[32mI\x1B[0m",
            Level::WARN => "\x1B[33mW\x1B[0m",
            Level::ERROR => "\x1B[31mE\x1B[0m",
        };

        // Get level color code for potential use with slogger
        let level_color = match level {
            Level::TRACE => "\x1B[36m", // Cyan
            Level::DEBUG => "\x1B[34m", // Blue
            Level::INFO => "\x1B[32m",  // Green
            Level::WARN => "\x1B[33m",  // Yellow
            Level::ERROR => "\x1B[31m", // Red
        };

        write!(writer, "{} ", level_str)?;

        // simple, shorter timestamp (HH:mm:ss)
        let now = chrono::Local::now();
        let time_str = now.format("%H:%M:%S").to_string();
        write!(writer, "\x1B[38;5;246m({time_str})\x1B[0m ")?;

        let target = event.metadata().target();

        // Special handling for slogger
        if target == "slogger" {
            // For slogger, omit the target prefix and color the message with the log level color
            // this mimics the behavior of slogging in urbit
            write!(writer, "{}", level_color)?;
            ctx.field_format().format_fields(writer.by_ref(), event)?;
            write!(writer, "\x1B[0m")?;

            return writeln!(writer);
        }

        let simplified_target = if target.contains("::") {
            // Just take the last component of the module path
            let parts: Vec<&str> = target.split("::").collect();
            if parts.len() > 1 {
                // If we have a structure like "a::b::c::d", just take "c::d"
                // but prefix it with the first two characters of the first part
                // i.e, nockapp::kernel::boot -> [cr] kernel::boot
                if parts.len() > 2 {
                    format!(
                        "[{}] {}::{}",
                        parts[0].chars().take(2).collect::<String>(),
                        parts[parts.len() - 2],
                        parts[parts.len() - 1]
                    )
                } else {
                    parts
                        .last()
                        .unwrap_or_else(|| {
                            panic!(
                                "Panicked at {}:{} (git sha: {:?})",
                                file!(),
                                line!(),
                                option_env!("GIT_SHA")
                            )
                        })
                        .to_string()
                }
            } else {
                target.to_string()
            }
        } else {
            target.to_string()
        };

        // Write the simplified target in grey and italics
        write!(writer, "\x1B[3;90m{}\x1B[0m: ", simplified_target)?;

        // Write the fields (the actual log message)
        ctx.field_format().format_fields(writer.by_ref(), event)?;

        writeln!(writer)
    }
}

fn init_with_default_filter<T: Subscriber + Send + Sync + for<'a> LookupSpan<'a>>(reg: T) {
    let filter = EnvFilter::new(
        std::env::var("RUST_LOG").unwrap_or_else(|_| DEFAULT_LOG_FILTER.to_string()),
    );

    let reg = reg.with(filter);

    #[cfg(feature = "tracing-tracy")]
    if std::env::var("TRACY_DISABLE").is_err() {
        let tracy = tracing_tracy::TracyLayer::default();
        let only_nockcode = std::env::var("TRACY_ONLY_NOCKCODE").is_ok();
        if only_nockcode {
            let nockcode_filter =
                tracing_subscriber::filter::filter_fn(move |meta| meta.target() == "nockcode");
            reg.with(tracy.with_filter(nockcode_filter)).init();
        } else {
            reg.with(tracy).init();
        }
        debug!("Tracy tracing is enabled");
        return;
    } else {
        debug!("Tracy tracing is disabled");
    }
    reg.init();
}

/// Initialize tracing with appropriate configuration based on CLI arguments.
pub fn init_default_tracing(cli: &Cli) {
    let use_ansi = cli.color == ColorChoice::Auto || cli.color == ColorChoice::Always;

    // Build and initialize the subscriber
    // If RUST_LOG is set and MINIMAL_LOG_FORMAT is unset, we will do production-grade logging.
    // Otherwise we will do more minimal logging suitable for an interactive terminal.
    if std::env::var("MINIMAL_LOG_FORMAT").is_ok() || std::env::var("RUST_LOG").is_err() {
        let fmt_layer = fmt::layer()
            .with_ansi(use_ansi)
            .event_format(MinimalFormatter);

        init_with_default_filter(tracing_subscriber::registry().with(fmt_layer));
    } else {
        init_with_default_filter(
            tracing_subscriber::registry().with(
                fmt::layer()
                    .with_ansi(use_ansi)
                    .with_target(true)
                    .with_level(true),
            ),
        );
    }
}

pub async fn setup<J: Jammer + Send + 'static>(
    jam: &[u8],
    cli: Cli,
    hot_state: &[HotEntry],
    name: &str,
    data_root: Option<PathBuf>,
) -> Result<NockApp<J>, Box<dyn std::error::Error>> {
    let result = setup_(jam, cli, hot_state, name, data_root).await?;
    match result {
        SetupResult::App(app) => Ok(app),
        SetupResult::ExportedState => {
            info!("Exiting after successful state export");
            std::process::exit(0);
        }
    }
}

pub async fn setup_<J: Jammer + Send + 'static>(
    jam: &[u8],
    mut cli: Cli,
    hot_state: &[HotEntry],
    name: &str,
    data_root: Option<PathBuf>,
) -> Result<SetupResult<J>, Box<dyn std::error::Error>> {
    if cli.state_jam.is_some() && !cli.new {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "--state-jam requires --new",
        )));
    }
    if cli.ephemeral && cli.bootstrap_from_chkjam.is_some() {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "--bootstrap-from-chkjam requires durable PMA boot",
        )));
    }
    if cli.ephemeral {
        cli.gc_interval = None;
        cli.rotating_snapshot_interval_event_time = None;
        cli.disable_fsync = true;
    }
    durability::set_fsync_disabled(cli.disable_fsync);
    let nock_test_jets_env = std::env::var("NOCK_TEST_JETS").unwrap_or_default();
    let test_jets = parse_test_jets(nock_test_jets_env.as_str());
    let ephemeral = cli.ephemeral;
    let data_dir = if let Some(explicit_dir) = cli.data_dir.clone() {
        explicit_dir
    } else if let Some(root) = data_root {
        root.join(name)
    } else {
        default_data_dir(name)
    };
    let pma_dir = data_dir.join("pma");
    let jams_dir = data_dir.join("checkpoints");
    let event_log_path = cli
        .event_log_path
        .clone()
        .unwrap_or_else(|| data_dir.join("event-log.sqlite3"));

    if cli.new && !ephemeral {
        if dir_has_entries(&data_dir)? {
            warn!(
                path = %data_dir.display(),
                "Refusing --new because the target data directory already contains data or setup artifacts; use a fresh path or remove it manually"
            );
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!(
                    "--new requires an empty data directory, found existing contents at {}",
                    data_dir.display()
                ),
            )));
        }

        for path in event_log_sidecar_paths(&event_log_path) {
            if path.exists() {
                warn!(
                    path = %path.display(),
                    "Refusing --new because the target event-log path already exists; use a fresh path or remove it manually"
                );
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!(
                        "--new requires an unused event-log path, found existing file at {}",
                        path.display()
                    ),
                )));
            }
        }
    }

    if !ephemeral && !jams_dir.exists() {
        std::fs::create_dir_all(&jams_dir)?;
        debug!("Created jams directory: {:?}", jams_dir);
    }

    if !ephemeral && !pma_dir.exists() {
        std::fs::create_dir_all(&pma_dir)?;
        debug!("Created pma directory: {:?}", pma_dir);
    }

    if !ephemeral {
        if let Some(chkjam_path) = cli.bootstrap_from_chkjam.as_deref() {
            let src = PathBuf::from(chkjam_path);
            if !src.exists() {
                return Err(format!("bootstrap chkjam not found: {}", src.display()).into());
            }
            let dst = jams_dir.join("0.chkjam");
            std::fs::copy(&src, &dst)?;
            let dst_alt = jams_dir.join("1.chkjam");
            if dst_alt.exists() {
                std::fs::remove_file(&dst_alt)?;
            }
            info!(
                "Bootstrapping from checkpoint: {} -> {}",
                src.display(),
                dst.display()
            );
        }
    }

    info!("kernel: starting");
    debug!("kernel: pma directory: {:?}", pma_dir);
    debug!("kernel: snapshots directory: {:?}", jams_dir);
    debug!("kernel: event-log path: {:?}", event_log_path);
    info!("NockApp boot cli: {:?}", cli);
    if cli.disable_fsync {
        warn!("All fsync/fdatasync durability calls are disabled");
    }
    let gc_interval = if ephemeral {
        None
    } else {
        cli.normalized_gc_interval()
    };
    let rotating_snapshot_interval_event_time = if ephemeral {
        None
    } else {
        cli.normalized_rotating_snapshot_interval_event_time()
    };
    if let Some(interval) = gc_interval {
        info!("PMA GC interval duration: {:?}", interval);
    } else {
        info!("PMA GC interval disabled");
    }
    if let Some(interval) = rotating_snapshot_interval_event_time {
        info!("Rotating snapshot interval event time: {:?}", interval);
    } else {
        info!("Rotating snapshots disabled");
    }
    if ephemeral {
        info!("Ephemeral NockStack active; PMA durability, event log, snapshots, and GC disabled");
    } else {
        info!("PMA durability active");
    }
    let pma_path_0 = pma_dir.join("0.pma");
    let pma_path_1 = pma_dir.join("1.pma");
    let stack_size = cli.stack_size;
    let pma_initial_words = pma_initial_words_for_boot(cli.pma_initial_size, jam.len());
    let pma_reserved_words = Some(
        cli.pma_reserved_size
            .map(PmaSize::words)
            .unwrap_or_else(|| Pma::default_reserved_words_for_capacity(pma_initial_words))
            .max(pma_initial_words),
    );
    let trace_opts = cli.trace_opts.clone();
    let event_log_path_for_kernel = event_log_path.clone();
    let event_log_preexisting = event_log_sidecar_paths(&event_log_path)
        .iter()
        .any(|path| path.exists());
    let allow_empty_event_log_bootstrap = cli.new || cli.bootstrap_from_chkjam.is_some();
    if !ephemeral {
        info!(
            kernel_jam_bytes = jam.len(),
            pma_initial_words,
            pma_reserved_words = ?pma_reserved_words,
            "PMA sizing selected"
        );
    }
    let kernel_f = move |metrics: Arc<NockAppMetrics>| async move {
        let boot_selection = if ephemeral {
            BootSelection {
                checkpoint: None,
                pma_open_existing: false,
                snapshot_manifest: None,
                replay_jobs: Vec::new(),
            }
        } else {
            select_boot_state::<J>(
                &jams_dir,
                jam,
                &event_log_path,
                &pma_path_0,
                &pma_path_1,
                metrics.clone(),
                BootEventLogPolicy {
                    preexisting: event_log_preexisting,
                    allow_empty_bootstrap: allow_empty_event_log_bootstrap,
                },
            )
            .await?
        };
        let mut checkpoint = boot_selection.checkpoint;
        let pma_open_existing = boot_selection.pma_open_existing;
        let snapshot_manifest = boot_selection.snapshot_manifest.clone();
        let replay_jobs = boot_selection.replay_jobs;
        let pma_config = |_nock_stack_words| {
            if ephemeral {
                None
            } else {
                Some(PmaConfig {
                    path_0: pma_path_0.clone(),
                    path_1: pma_path_1.clone(),
                    words: pma_initial_words,
                    reserved_words: pma_reserved_words,
                    open_existing: pma_open_existing,
                    create_snapshots: true,
                    rotating_snapshot_interval_event_time,
                    restore_manifest: snapshot_manifest.clone(),
                    gc_interval,
                })
            }
        };
        let event_log_config = if ephemeral {
            None
        } else {
            Some(EventLogConfig {
                path: event_log_path_for_kernel.clone(),
            })
        };
        let kernel: Kernel<SaveableCheckpoint> = match stack_size {
            NockStackSize::Tiny => {
                Kernel::load_with_hot_state_tiny_with_event_log(
                    jam,
                    checkpoint.take(),
                    hot_state,
                    test_jets,
                    trace_opts.clone(),
                    pma_config(NOCK_STACK_SIZE_TINY),
                    event_log_config.clone(),
                )
                .await?
            }
            NockStackSize::Small => {
                Kernel::load_with_hot_state_small_with_event_log(
                    jam,
                    checkpoint.take(),
                    hot_state,
                    test_jets,
                    trace_opts.clone(),
                    pma_config(NOCK_STACK_SIZE_SMALL),
                    event_log_config.clone(),
                )
                .await?
            }
            NockStackSize::Normal => {
                Kernel::load_with_hot_state_with_event_log(
                    jam,
                    checkpoint.take(),
                    hot_state,
                    test_jets,
                    trace_opts.clone(),
                    pma_config(NOCK_STACK_SIZE),
                    event_log_config.clone(),
                )
                .await?
            }
            NockStackSize::Medium => {
                Kernel::load_with_hot_state_medium_with_event_log(
                    jam,
                    checkpoint.take(),
                    hot_state,
                    test_jets,
                    trace_opts.clone(),
                    pma_config(NOCK_STACK_SIZE_MEDIUM),
                    event_log_config.clone(),
                )
                .await?
            }
            NockStackSize::Large => {
                Kernel::load_with_hot_state_large_with_event_log(
                    jam,
                    checkpoint.take(),
                    hot_state,
                    test_jets,
                    trace_opts.clone(),
                    pma_config(NOCK_STACK_SIZE_LARGE),
                    event_log_config.clone(),
                )
                .await?
            }
            NockStackSize::Huge => {
                Kernel::load_with_hot_state_huge_with_event_log(
                    jam,
                    checkpoint.take(),
                    hot_state,
                    test_jets,
                    trace_opts,
                    pma_config(NOCK_STACK_SIZE_HUGE),
                    event_log_config,
                )
                .await?
            }
        };
        if !replay_jobs.is_empty() {
            let replay_start = std::time::Instant::now();
            let replay_job_count = replay_jobs.len();
            info!(
                jobs = replay_job_count,
                "event replay after snapshot restore start"
            );
            if let Err(err) = kernel.replay_event_jobs(replay_jobs).await {
                metrics.replay_failures.increment();
                warn!(
                    jobs = replay_job_count,
                    elapsed_ms = replay_start.elapsed().as_secs_f64() * 1000.0,
                    error = %err,
                    "event replay after snapshot restore failed"
                );
                return Err(err);
            }
            for _ in 0..replay_job_count {
                metrics.replay_events.increment();
            }
            let elapsed = replay_start.elapsed();
            metrics.replay_apply.add_timing(&elapsed);
            info!(
                jobs = replay_job_count,
                elapsed_ms = elapsed.as_secs_f64() * 1000.0,
                "event replay after snapshot restore done"
            );
        }
        let res: Result<Kernel<SaveableCheckpoint>, CrownError<ExternalError>> = Ok(kernel);
        res
    };

    let app: NockApp<J> = NockApp::new(kernel_f).await?;

    if let Some(export_path) = cli.export_state_jam.clone() {
        export_kernel_state::<J, _>(&app.kernel, &export_path).await?;
        return Ok(SetupResult::ExportedState);
    }

    if let Some(import_path) = cli.state_jam.clone() {
        import_kernel_state::<J, _>(&app.kernel, &import_path).await?;
    }

    Ok(SetupResult::App(app))
}

/// Exports the kernel state to a jam file at the specified path
async fn export_kernel_state<J: Jammer, C>(
    kernel: &Kernel<C>,
    export_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    info!(
        path = %export_path,
        jammer = std::any::type_name::<J>(),
        "state jam export start"
    );
    if let Some(parent) = Path::new(export_path).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).await?;
        }
    }
    let kernel_state = kernel.export().await?;
    let exported_state = ExportedState::from_loadstate::<J>(kernel_state);
    let state_bytes = exported_state.encode()?;
    info!(
        path = %export_path,
        bytes = state_bytes.len(),
        "state jam write start"
    );
    fs::write(export_path, state_bytes).await?;
    info!(
        path = %export_path,
        jammer = std::any::type_name::<J>(),
        "state jam export done"
    );
    Ok(())
}

/// Imports the kernel state from a jam file at the specified path
async fn import_kernel_state<J: Jammer, C>(
    kernel: &Kernel<C>,
    import_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    info!(
        path = %import_path,
        jammer = std::any::type_name::<J>(),
        "state jam import start"
    );
    let state_bytes = fs::read(import_path).await?;
    let exported_state = ExportedState::decode(&state_bytes)?;
    let kernel_state = exported_state.to_loadstate::<J>()?;
    kernel.import(kernel_state).await?;
    info!(
        path = %import_path,
        jammer = std::any::type_name::<J>(),
        "state jam import done"
    );
    Ok(())
}

pub fn parse_test_jets(jets: &str) -> Vec<NounSlab> {
    let mut test_jets = Vec::new();
    for jet in jets.split(',') {
        if jet.is_empty() {
            continue;
        }
        let mut slab = NounSlab::new();
        let mut path = nockvm::noun::D(0);
        for el in jet.split('/') {
            let ver_split: Vec<&str> = el.split('.').collect();
            if ver_split.len() == 2 {
                let sym_atom = Atom::from_value(&mut slab, ver_split[0])
                    .expect("Could not construct symbol atom")
                    .as_noun();
                let ver_atom = Atom::from_value(
                    &mut slab,
                    ver_split[1]
                        .parse::<u64>()
                        .expect("Could not parse cold path version"),
                )
                .expect("Could not construct version atom")
                .as_noun();
                let path_el = nockvm::noun::T(&mut slab, &[sym_atom, ver_atom]);
                path = nockvm::noun::T(&mut slab, &[path_el, path]);
            } else if el.is_empty() {
                continue;
            } else {
                let el_atom = Atom::from_value(&mut slab, el)
                    .expect("Could not construct element atom")
                    .as_noun();
                path = nockvm::noun::T(&mut slab, &[el_atom, path]);
            }
        }
        slab.set_root(path);
        test_jets.push(slab);
    }
    test_jets
}
