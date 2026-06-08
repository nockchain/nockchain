#!/usr/bin/env rust-script
//! ```cargo
//! [package]
//! edition = "2021"
//! ```
use std::collections::HashMap;
use std::convert::TryInto;
use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
struct ProcInfo {
    pid: i32,
    cmdline: String,
    comm: String,
    exe: Option<PathBuf>,
    cwd: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
struct MemTotals {
    size_kb: u64,
    rss_kb: u64,
    pss_kb: u64,
    shared_clean_kb: u64,
    shared_dirty_kb: u64,
    private_clean_kb: u64,
    private_dirty_kb: u64,
    swap_kb: u64,
    swap_pss_kb: u64,
}

#[derive(Clone, Debug, Default)]
struct StatusMem {
    vm_rss_kb: u64,
    vm_size_kb: u64,
    vm_swap_kb: u64,
    rss_anon_kb: u64,
    rss_file_kb: u64,
    rss_shmem_kb: u64,
}

#[derive(Clone, Debug, Default)]
struct ProcMemReport {
    status: StatusMem,
    smaps_rollup: Option<MemTotals>,
    pma_maps: MemTotals,
    pma_map_count: u64,
    pma_smaps_available: bool,
    pma_alloc_bytes: Option<u64>,
    checkpoint: Option<CheckpointInfo>,
}

#[derive(Clone, Debug)]
struct CheckpointInfo {
    checkpoints_dir: PathBuf,
    file_count: u64,
    total_bytes: u64,
    latest_path: Option<PathBuf>,
    latest_bytes: u64,
}

fn main() {
    if !Path::new("/proc").is_dir() {
        eprintln!("This script requires /proc (Linux).");
        std::process::exit(1);
    }

    let (pma_proc, base_proc) = match discover_procs() {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    };

    let pma_report = match collect_report(
        &pma_proc,
        env::var("NOCKCHAIN_PMA_DATA_DIR").ok().map(PathBuf::from),
    ) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("Failed to read memory stats for PMA process: {err}");
            std::process::exit(1);
        }
    };
    let base_report = match collect_report(
        &base_proc,
        env::var("NOCKCHAIN_BASE_DATA_DIR").ok().map(PathBuf::from),
    ) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("Failed to read memory stats for base process: {err}");
            std::process::exit(1);
        }
    };

    println!("PMA process:");
    print_proc(&pma_proc, &pma_report);
    println!();
    println!("Base process:");
    print_proc(&base_proc, &base_report);
    println!();
    print_comparison(&pma_report, &base_report);
    println!();
    print_summary(&pma_proc, &pma_report, &base_proc, &base_report);
}

fn discover_procs() -> Result<(ProcInfo, ProcInfo), String> {
    if let (Ok(pma_pid), Ok(base_pid)) = (
        env::var("NOCKCHAIN_PMA_PID"),
        env::var("NOCKCHAIN_BASE_PID"),
    ) {
        let pma_pid = parse_pid(&pma_pid)?;
        let base_pid = parse_pid(&base_pid)?;
        let pma_info = read_proc_info(pma_pid)?;
        let base_info = read_proc_info(base_pid)?;
        return Ok((pma_info, base_info));
    }

    let all = list_proc_infos()?;
    let candidates: Vec<ProcInfo> = all
        .into_iter()
        .filter(|info| is_nockchain_process(info))
        .collect();

    if candidates.len() < 2 {
        return Err(format!(
            "Expected at least 2 nockchain processes, found {}.\n\
             You can set NOCKCHAIN_PMA_PID and NOCKCHAIN_BASE_PID to override.",
            candidates.len()
        ));
    }

    let base_dir = env::var("NOCKCHAIN_BASE_DIR").ok().map(PathBuf::from);
    let repo_root_opt = env::current_dir().ok().and_then(find_repo_root);

    if let Some((pma, base)) = pick_by_cwd_hint(&candidates) {
        return Ok((pma, base));
    }

    if let Some(repo_root) = repo_root_opt.as_ref() {
        if let Some((pma, base)) = pick_by_repo_data_dir(&candidates, repo_root) {
            return Ok((pma, base));
        }
    }

    if let Some((pma, base)) = pick_by_data_dir_flag(&candidates) {
        return Ok((pma, base));
    }

    if let Some((pma, base)) = pick_by_pma_presence(&candidates, base_dir.as_ref()) {
        return Ok((pma, base));
    }

    if candidates.len() == 2 {
        let first_is_docker = is_docker_nockchain(&candidates[0]);
        let second_is_docker = is_docker_nockchain(&candidates[1]);
        if first_is_docker != second_is_docker {
            let (pma, base) = if first_is_docker {
                (candidates[0].clone(), candidates[1].clone())
            } else {
                (candidates[1].clone(), candidates[0].clone())
            };
            return Ok((pma, base));
        }
    }

    let repo_root = match repo_root_opt {
        Some(root) => root,
        None => {
            return Err(
                "Could not locate repo root (Cargo.toml + crates/). Run from repo root."
                    .to_string(),
            );
        }
    };

    let mut docker_candidates: Vec<ProcInfo> = candidates
        .iter()
        .cloned()
        .filter(|info| is_docker_nockchain(info))
        .collect();
    if docker_candidates.len() == 1 {
        let pma = docker_candidates.remove(0);
        let mut base_candidates: Vec<ProcInfo> = candidates
            .iter()
            .cloned()
            .filter(|info| info.pid != pma.pid)
            .collect();
        base_candidates = apply_base_dir_filter(&base_candidates, base_dir.as_ref());
        let base = choose_single(
            base_candidates,
            "base",
            "Set NOCKCHAIN_BASE_PID or NOCKCHAIN_BASE_DIR.",
        )?;
        return Ok((pma, base));
    }

    let mut with_pma: Vec<ProcInfo> = Vec::new();
    let mut without_pma: Vec<ProcInfo> = Vec::new();

    for info in candidates.iter().cloned() {
        if has_pma_mapping(info.pid).unwrap_or(false) {
            with_pma.push(info);
        } else {
            without_pma.push(info);
        }
    }

    if with_pma.len() == 1 && !without_pma.is_empty() {
        let pma = with_pma.remove(0);
        let base_candidates = apply_base_dir_filter(&without_pma, base_dir.as_ref());
        let base = choose_single(
            base_candidates,
            "base",
            "Set NOCKCHAIN_BASE_PID or NOCKCHAIN_BASE_DIR.",
        )?;
        return Ok((pma, base));
    }

    let mut container_candidates: Vec<ProcInfo> = Vec::new();
    let mut host_candidates: Vec<ProcInfo> = Vec::new();
    for info in candidates.iter().cloned() {
        if is_container_process(&info) {
            container_candidates.push(info);
        } else {
            host_candidates.push(info);
        }
    }
    if container_candidates.len() == 1 && !host_candidates.is_empty() {
        let pma = container_candidates.remove(0);
        let base_candidates = apply_base_dir_filter(&host_candidates, base_dir.as_ref());
        let base = choose_single(
            base_candidates,
            "base",
            "Set NOCKCHAIN_BASE_PID or NOCKCHAIN_BASE_DIR.",
        )?;
        return Ok((pma, base));
    }

    let mut pma_candidates: Vec<ProcInfo> = Vec::new();
    let mut base_candidates: Vec<ProcInfo> = Vec::new();

    for info in candidates {
        let is_pma = is_under_root(&info, &repo_root);
        let is_base = base_dir
            .as_ref()
            .map(|base| is_under_root(&info, base))
            .unwrap_or(false);

        if is_pma {
            pma_candidates.push(info);
        } else if is_base {
            base_candidates.push(info);
        } else {
            base_candidates.push(info);
        }
    }

    let pma = choose_single(
        pma_candidates,
        "PMA",
        "Set NOCKCHAIN_PMA_PID or run from PMA repo root.",
    )?;

    let base = choose_single(
        base_candidates,
        "base",
        "Set NOCKCHAIN_BASE_PID or NOCKCHAIN_BASE_DIR.",
    )?;

    Ok((pma, base))
}

fn pick_by_cwd_hint(candidates: &[ProcInfo]) -> Option<(ProcInfo, ProcInfo)> {
    let mut pma_candidates: Vec<ProcInfo> = candidates
        .iter()
        .cloned()
        .filter(|info| has_pma_hint_from_paths(info))
        .collect();
    if pma_candidates.len() == 1 && candidates.len() >= 2 {
        let pma = pma_candidates.remove(0);
        let mut base_candidates: Vec<ProcInfo> = candidates
            .iter()
            .cloned()
            .filter(|info| info.pid != pma.pid)
            .collect();
        if base_candidates.len() == 1 {
            return Some((pma, base_candidates.remove(0)));
        }
    }
    None
}

fn pick_by_repo_data_dir(
    candidates: &[ProcInfo],
    repo_root: &Path,
) -> Option<(ProcInfo, ProcInfo)> {
    let pma_data_dir = repo_root.join(".data.nockchain");
    let mut matches: Vec<ProcInfo> = candidates
        .iter()
        .cloned()
        .filter(|info| {
            resolve_data_dir(info, None)
                .map(|dir| dir == pma_data_dir)
                .unwrap_or(false)
        })
        .collect();
    if matches.len() == 1 && candidates.len() >= 2 {
        let pma = matches.remove(0);
        let mut base_candidates: Vec<ProcInfo> = candidates
            .iter()
            .cloned()
            .filter(|info| info.pid != pma.pid)
            .collect();
        if base_candidates.len() == 1 {
            return Some((pma, base_candidates.remove(0)));
        }
    }
    None
}

fn pick_by_data_dir_flag(candidates: &[ProcInfo]) -> Option<(ProcInfo, ProcInfo)> {
    let mut with_flag: Vec<ProcInfo> = candidates
        .iter()
        .cloned()
        .filter(|info| parse_data_dir_flag(&info.cmdline).is_some())
        .collect();
    let mut without_flag: Vec<ProcInfo> = candidates
        .iter()
        .cloned()
        .filter(|info| parse_data_dir_flag(&info.cmdline).is_none())
        .collect();
    if with_flag.len() == 1 && without_flag.len() == 1 {
        return Some((with_flag.remove(0), without_flag.remove(0)));
    }
    None
}

fn pick_by_pma_presence(
    candidates: &[ProcInfo],
    base_dir: Option<&PathBuf>,
) -> Option<(ProcInfo, ProcInfo)> {
    let mut with_pma = Vec::new();
    let mut without_pma = Vec::new();
    let mut unknown = Vec::new();

    for info in candidates.iter().cloned() {
        match detect_pma_presence(&info) {
            Some(true) => with_pma.push(info),
            Some(false) => without_pma.push(info),
            None => unknown.push(info),
        }
    }

    if with_pma.len() == 1 {
        let pma = with_pma.remove(0);
        let base_candidates = if !without_pma.is_empty() {
            apply_base_dir_filter(&without_pma, base_dir)
        } else {
            apply_base_dir_filter(&unknown, base_dir)
        };
        if let Ok(base) = choose_single(
            base_candidates,
            "base",
            "Set NOCKCHAIN_BASE_PID or NOCKCHAIN_BASE_DIR.",
        ) {
            return Some((pma, base));
        }
    }

    if with_pma.is_empty() && without_pma.len() == 1 && !unknown.is_empty() {
        let base = without_pma.remove(0);
        if let Ok(pma) = choose_single(
            apply_base_dir_filter(&unknown, base_dir),
            "PMA",
            "Set NOCKCHAIN_PMA_PID.",
        ) {
            return Some((pma, base));
        }
    }

    if with_pma.len() > 1 && without_pma.len() == 1 {
        let base = without_pma.remove(0);
        if let Ok(pma) = choose_single(
            with_pma,
            "PMA",
            "Set NOCKCHAIN_PMA_PID.",
        ) {
            return Some((pma, base));
        }
    }

    None
}

fn apply_base_dir_filter(
    candidates: &[ProcInfo],
    base_dir: Option<&PathBuf>,
) -> Vec<ProcInfo> {
    let mut candidates: Vec<ProcInfo> = candidates.to_vec();
    if let Some(base_dir) = base_dir {
        let filtered: Vec<ProcInfo> = candidates
            .iter()
            .cloned()
            .filter(|info| is_under_root(info, base_dir))
            .collect();
        if !filtered.is_empty() {
            candidates = filtered;
        }
    }
    candidates
}

fn choose_single(mut candidates: Vec<ProcInfo>, label: &str, hint: &str) -> Result<ProcInfo, String> {
    if candidates.is_empty() {
        return Err(format!("No candidate {label} process found. {hint}"));
    }
    if candidates.len() == 1 {
        return Ok(candidates.remove(0));
    }

    if label.eq_ignore_ascii_case("base") {
        let non_docker: Vec<ProcInfo> = candidates
            .iter()
            .cloned()
            .filter(|info| !is_docker_nockchain(info))
            .collect();
        if non_docker.len() == 1 {
            return Ok(non_docker[0].clone());
        }
    } else if label.eq_ignore_ascii_case("pma") {
        let docker: Vec<ProcInfo> = candidates
            .iter()
            .cloned()
            .filter(|info| is_docker_nockchain(info))
            .collect();
        if docker.len() == 1 {
            return Ok(docker[0].clone());
        }
    }

    candidates.retain(|info| info.cmdline.contains("--fast-sync"));
    if candidates.len() == 1 {
        return Ok(candidates.remove(0));
    }

    let mut msg = format!(
        "Ambiguous {label} process selection ({} candidates). {hint}\n",
        candidates.len()
    );
    for info in candidates {
        msg.push_str(&format!(
            "  pid={} comm={} cwd={} cmdline={}\n",
            info.pid,
            info.comm,
            info.cwd
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "<unknown>".to_string()),
            info.cmdline
        ));
    }
    Err(msg)
}

fn has_pma_mapping(pid: i32) -> Option<bool> {
    let smaps_path = PathBuf::from("/proc").join(pid.to_string()).join("smaps");
    if let Some(found) = scan_mapping_file(&smaps_path) {
        return Some(found);
    }
    let maps_path = PathBuf::from("/proc").join(pid.to_string()).join("maps");
    scan_mapping_file(&maps_path)
}

fn scan_mapping_file(path: &Path) -> Option<bool> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    for line in reader.lines().flatten() {
        if let Some(path_opt) = parse_smaps_header(&line) {
            if let Some(path) = path_opt {
                if is_pma_mapping(&path) {
                    return Some(true);
                }
            }
        }
    }
    Some(false)
}

fn has_pma_fd(pid: i32) -> Option<bool> {
    let fd_dir = PathBuf::from("/proc").join(pid.to_string()).join("fd");
    let entries = fs::read_dir(&fd_dir).ok()?;
    for entry in entries.flatten() {
        if let Ok(target) = fs::read_link(entry.path()) {
            let target = target.to_string_lossy();
            if is_pma_mapping(&target) {
                return Some(true);
            }
        }
    }
    Some(false)
}

fn detect_pma_presence(info: &ProcInfo) -> Option<bool> {
    if let Some(found) = has_pma_mapping(info.pid) {
        return Some(found);
    }
    if let Some(found) = has_pma_fd(info.pid) {
        return Some(found);
    }
    if has_pma_hint_from_paths(info) {
        return Some(true);
    }
    if let Some(has_dir) = data_dir_has_pma(info) {
        return Some(has_dir);
    }
    None
}

fn has_pma_hint_from_paths(info: &ProcInfo) -> bool {
    let needles = ["pma-nockchain", "pma_nockchain"];
    if let Some(cwd) = &info.cwd {
        let cwd = cwd.to_string_lossy();
        if needles.iter().any(|needle| cwd.contains(needle)) {
            return true;
        }
    }
    if let Some(exe) = &info.exe {
        let exe = exe.to_string_lossy();
        if needles.iter().any(|needle| exe.contains(needle)) {
            return true;
        }
    }
    false
}

fn data_dir_has_pma(info: &ProcInfo) -> Option<bool> {
    let data_dir = resolve_data_dir(info, None)?;
    let pma_dir = data_dir.join("pma");
    match fs::read_dir(&pma_dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("mmap") {
                    return Some(true);
                }
            }
            Some(false)
        }
        Err(err) => match err.kind() {
            std::io::ErrorKind::NotFound => Some(false),
            std::io::ErrorKind::PermissionDenied => None,
            _ => None,
        },
    }
}

fn is_container_process(info: &ProcInfo) -> bool {
    if is_docker_nockchain(info) {
        return true;
    }
    let path = PathBuf::from("/proc").join(info.pid.to_string()).join("cgroup");
    let contents = read_to_string(&path).unwrap_or_default();
    ["docker", "containerd", "kubepods", "libpod"]
        .iter()
        .any(|marker| contents.contains(marker))
}

fn is_docker_nockchain(info: &ProcInfo) -> bool {
    if info.cmdline.contains("/data/.data.nockchain")
        || info.cmdline.contains("/usr/local/bin/nockchain")
    {
        return true;
    }
    if let Some(exe) = &info.exe {
        if exe.ends_with("/usr/local/bin/nockchain") {
            return true;
        }
    }
    false
}

fn is_under_root(info: &ProcInfo, root: &Path) -> bool {
    let mut matches = false;
    if let Some(cwd) = &info.cwd {
        if cwd.starts_with(root) {
            matches = true;
        }
    }
    if let Some(exe) = &info.exe {
        if exe.starts_with(root) {
            matches = true;
        }
    }
    matches
}

fn parse_pid(pid: &str) -> Result<i32, String> {
    pid.parse::<i32>()
        .map_err(|_| format!("Invalid pid: {pid}"))
}

fn list_proc_infos() -> Result<Vec<ProcInfo>, String> {
    let mut infos = Vec::new();
    let entries = fs::read_dir("/proc").map_err(|e| e.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let name = match name.to_str() {
            Some(s) => s,
            None => continue,
        };
        let pid = match name.parse::<i32>() {
            Ok(pid) => pid,
            Err(_) => continue,
        };
        if let Ok(info) = read_proc_info(pid) {
            infos.push(info);
        }
    }
    Ok(infos)
}

fn read_proc_info(pid: i32) -> Result<ProcInfo, String> {
    let base = PathBuf::from("/proc").join(pid.to_string());
    let cmdline = read_cmdline(&base.join("cmdline")).unwrap_or_default();
    let comm = read_to_string(&base.join("comm")).unwrap_or_default();
    let exe = fs::read_link(&base.join("exe")).ok();
    let cwd = fs::read_link(&base.join("cwd")).ok();

    Ok(ProcInfo {
        pid,
        cmdline,
        comm: comm.trim().to_string(),
        exe,
        cwd,
    })
}

fn read_cmdline(path: &Path) -> Option<String> {
    let data = fs::read(path).ok()?;
    if data.is_empty() {
        return None;
    }
    let parts: Vec<OsString> = data
        .split(|b| *b == 0)
        .filter(|b| !b.is_empty())
        .map(|b| OsString::from(String::from_utf8_lossy(b).to_string()))
        .collect();
    if parts.is_empty() {
        return None;
    }
    let cmdline = parts
        .iter()
        .map(|s| s.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(" ");
    Some(cmdline)
}

fn read_to_string(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn is_nockchain_process(info: &ProcInfo) -> bool {
    if info.comm == "nockchain" {
        return true;
    }
    if let Some(exe) = &info.exe {
        if exe.ends_with("nockchain") {
            return true;
        }
    }
    info.cmdline
        .split_whitespace()
        .next()
        .map(|arg0| arg0.ends_with("nockchain") || arg0.ends_with("/nockchain"))
        .unwrap_or(false)
}

fn find_repo_root(start: PathBuf) -> Option<PathBuf> {
    let mut dir = start;
    loop {
        if dir.join("Cargo.toml").is_file() && dir.join("crates").is_dir() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn collect_report(
    info: &ProcInfo,
    data_dir_override: Option<PathBuf>,
) -> Result<ProcMemReport, String> {
    let base = PathBuf::from("/proc").join(info.pid.to_string());
    let status = parse_status(&base.join("status")).unwrap_or_default();
    let smaps_rollup = parse_smaps_rollup(&base.join("smaps_rollup")).ok();
    let (pma_maps, pma_map_count, pma_smaps_available) = parse_pma_smaps(&base.join("smaps"))?;
    let pma_alloc_bytes = pma_alloc_bytes(info, data_dir_override.as_ref());
    let checkpoint = checkpoint_info(info, data_dir_override);
    Ok(ProcMemReport {
        status,
        smaps_rollup,
        pma_maps,
        pma_map_count,
        pma_smaps_available,
        pma_alloc_bytes,
        checkpoint,
    })
}

fn parse_status(path: &Path) -> Option<StatusMem> {
    let contents = read_to_string(path)?;
    let mut map: HashMap<&str, u64> = HashMap::new();
    for line in contents.lines() {
        if let Some((key, val)) = parse_kb_line(line) {
            map.insert(key, val);
        }
    }
    Some(StatusMem {
        vm_rss_kb: *map.get("VmRSS").unwrap_or(&0),
        vm_size_kb: *map.get("VmSize").unwrap_or(&0),
        vm_swap_kb: *map.get("VmSwap").unwrap_or(&0),
        rss_anon_kb: *map.get("RssAnon").unwrap_or(&0),
        rss_file_kb: *map.get("RssFile").unwrap_or(&0),
        rss_shmem_kb: *map.get("RssShmem").unwrap_or(&0),
    })
}

fn parse_smaps_rollup(path: &Path) -> Result<MemTotals, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let reader = BufReader::new(file);
    let mut totals = MemTotals::default();
    for line in reader.lines() {
        let line = line.map_err(|e| e.to_string())?;
        if let Some((key, val)) = parse_kb_line(&line) {
            assign_mem_total(&mut totals, key, val);
        }
    }
    Ok(totals)
}

fn parse_pma_smaps(path: &Path) -> Result<(MemTotals, u64, bool), String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => {
            return Ok((MemTotals::default(), 0, false));
        }
    };
    let reader = BufReader::new(file);
    let mut totals = MemTotals::default();
    let mut count = 0u64;
    let mut current_is_pma = false;

    for line in reader.lines() {
        let line = line.map_err(|e| e.to_string())?;
        if let Some(path_opt) = parse_smaps_header(&line) {
            current_is_pma = path_opt
                .as_ref()
                .map(|path| is_pma_mapping(path))
                .unwrap_or(false);
            if current_is_pma {
                count += 1;
            }
            continue;
        }
        if current_is_pma {
            if let Some((key, val)) = parse_kb_line(&line) {
                assign_mem_total(&mut totals, key, val);
            }
        }
    }

    Ok((totals, count, true))
}

fn parse_smaps_header(line: &str) -> Option<Option<String>> {
    let mut parts = line.split_whitespace();
    let range = parts.next()?;
    if !range.contains('-') {
        return None;
    }
    let perms = parts.next()?;
    if perms.len() < 4 {
        return None;
    }
    parts.next()?;
    parts.next()?;
    parts.next()?;
    let path = parts
        .next()
        .map(|p| p.trim_end_matches("(deleted)").to_string());
    Some(path)
}

fn is_pma_mapping(path: &str) -> bool {
    let path = path.trim();
    (path.contains("/pma/") && path.ends_with(".mmap")) || (path.contains("pma-") && path.ends_with(".mmap"))
}

fn pma_alloc_bytes(info: &ProcInfo, data_dir_override: Option<&PathBuf>) -> Option<u64> {
    let data_dir = resolve_data_dir(info, data_dir_override.cloned())?;
    let pma_dir = data_dir.join("pma");
    if !pma_dir.is_dir() {
        return None;
    }
    let mut best: Option<(std::time::SystemTime, u64)> = None;
    let entries = fs::read_dir(&pma_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("mmap") {
            continue;
        }
        let meta = entry.metadata().ok()?;
        let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let alloc = read_pma_alloc_bytes(&path)?;
        let update = match best {
            None => true,
            Some((best_mtime, _)) => mtime >= best_mtime,
        };
        if update {
            best = Some((mtime, alloc));
        }
    }
    best.map(|(_, alloc)| alloc)
}

fn read_pma_alloc_bytes(path: &Path) -> Option<u64> {
    const PMA_MAGIC: u64 = u64::from_le_bytes(*b"NOCKPMA1");
    const PMA_VERSION: u64 = 1;
    const PMA_TRAILER_BYTES: usize = 32;
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len < PMA_TRAILER_BYTES as u64 {
        return None;
    }
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::End(-(PMA_TRAILER_BYTES as i64))).ok()?;
    let mut buf = [0u8; PMA_TRAILER_BYTES];
    file.read_exact(&mut buf).ok()?;
    let magic = u64::from_le_bytes(buf[0..8].try_into().ok()?);
    let version = u64::from_le_bytes(buf[8..16].try_into().ok()?);
    if magic != PMA_MAGIC || version != PMA_VERSION {
        return None;
    }
    let alloc_offset_words = u64::from_le_bytes(buf[24..32].try_into().ok()?);
    Some(alloc_offset_words.saturating_mul(8))
}

fn checkpoint_info(info: &ProcInfo, data_dir_override: Option<PathBuf>) -> Option<CheckpointInfo> {
    let data_dir = resolve_data_dir(info, data_dir_override)?;
    let checkpoints_dir = data_dir.join("checkpoints");
    if !checkpoints_dir.is_dir() {
        return None;
    }
    let mut file_count = 0u64;
    let mut total_bytes = 0u64;
    let mut latest_path: Option<PathBuf> = None;
    let mut latest_mtime = std::time::SystemTime::UNIX_EPOCH;
    let mut latest_bytes = 0u64;

    let entries = fs::read_dir(&checkpoints_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let meta = match entry.metadata() {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if !meta.is_file() {
            continue;
        }
        file_count += 1;
        let bytes = meta.len();
        total_bytes = total_bytes.saturating_add(bytes);
        let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if latest_path.is_none() || mtime >= latest_mtime {
            latest_path = Some(path);
            latest_mtime = mtime;
            latest_bytes = bytes;
        }
    }

    Some(CheckpointInfo {
        checkpoints_dir,
        file_count,
        total_bytes,
        latest_path,
        latest_bytes,
    })
}

fn resolve_data_dir(info: &ProcInfo, data_dir_override: Option<PathBuf>) -> Option<PathBuf> {
    let base = if let Some(override_dir) = data_dir_override {
        Some(override_dir)
    } else {
        parse_data_dir_flag(&info.cmdline).or_else(|| default_data_dir_from_cwd(info))
    };

    if base.is_none() {
        if is_container_process(info) {
            return probe_container_data_dir(info);
        }
        return None;
    }
    let base = base?;

    if base.is_absolute() {
        let rooted = proc_root(info).join(base.strip_prefix("/").unwrap_or(&base));
        if (is_container_process(info) && rooted.exists()) || (!base.exists() && rooted.exists()) {
            return Some(rooted);
        }
        if is_container_process(info) {
            if let Some(mapped) = map_container_path_to_host(info, &base) {
                if mapped.exists() {
                    return Some(mapped);
                }
            }
        }
        return Some(base);
    }
    info.cwd.as_ref().map(|cwd| cwd.join(base))
}

fn probe_container_data_dir(info: &ProcInfo) -> Option<PathBuf> {
    let root = proc_root(info);
    let candidates = [
        PathBuf::from("/data/.data.nockchain"),
        PathBuf::from("/.data.nockchain"),
        PathBuf::from("/root/.data.nockchain"),
    ];
    for path in candidates {
        let rooted = root.join(path.strip_prefix("/").unwrap_or(&path));
        if rooted.is_dir() {
            return Some(rooted);
        }
        if let Some(mapped) = map_container_path_to_host(info, &path) {
            if mapped.is_dir() {
                return Some(mapped);
            }
        }
    }
    None
}

fn map_container_path_to_host(info: &ProcInfo, container_path: &Path) -> Option<PathBuf> {
    let mounts_path = PathBuf::from("/proc")
        .join(info.pid.to_string())
        .join("mounts");
    let mounts = read_to_string(&mounts_path)?;
    let mut best: Option<(usize, PathBuf)> = None;
    for line in mounts.lines() {
        let mut parts = line.split_whitespace();
        let source = parts.next()?;
        let mount_point = parts.next()?;
        let mount_point = unescape_mount_field(mount_point);
        let source = unescape_mount_field(source);
        let mount_point_path = Path::new(&mount_point);
        if container_path.starts_with(mount_point_path) {
            let rel = container_path.strip_prefix(mount_point_path).unwrap_or(Path::new(""));
            let host_path = Path::new(&source).join(rel);
            let len = mount_point.len();
            let update = match &best {
                None => true,
                Some((best_len, _)) => len > *best_len,
            };
            if update {
                best = Some((len, host_path));
            }
        }
    }
    best.map(|(_, path)| path)
}

fn unescape_mount_field(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        let mut code = String::new();
        for _ in 0..3 {
            match chars.peek() {
                Some(c) if c.is_ascii_digit() => {
                    code.push(*c);
                    chars.next();
                }
                _ => break,
            }
        }
        if code.len() == 3 {
            if let Ok(oct) = u8::from_str_radix(&code, 8) {
                out.push(oct as char);
                continue;
            }
        }
        out.push('\\');
        out.push_str(&code);
    }
    out
}

fn proc_root(info: &ProcInfo) -> PathBuf {
    PathBuf::from("/proc")
        .join(info.pid.to_string())
        .join("root")
}

fn parse_data_dir_flag(cmdline: &str) -> Option<PathBuf> {
    let mut iter = cmdline.split_whitespace();
    while let Some(arg) = iter.next() {
        if arg == "--data-dir" {
            if let Some(value) = iter.next() {
                return Some(PathBuf::from(value));
            }
        } else if let Some(value) = arg.strip_prefix("--data-dir=") {
            return Some(PathBuf::from(value));
        }
    }
    None
}

fn default_data_dir_from_cwd(info: &ProcInfo) -> Option<PathBuf> {
    let cwd = info.cwd.as_ref()?;
    Some(cwd.join(".data.nockchain"))
}

fn parse_kb_line(line: &str) -> Option<(&str, u64)> {
    let mut parts = line.split_whitespace();
    let key = parts.next()?;
    let val = parts.next()?.parse::<u64>().ok()?;
    Some((key.trim_end_matches(':'), val))
}

fn assign_mem_total(totals: &mut MemTotals, key: &str, val: u64) {
    match key {
        "Size" => totals.size_kb += val,
        "Rss" => totals.rss_kb += val,
        "Pss" => totals.pss_kb += val,
        "Shared_Clean" => totals.shared_clean_kb += val,
        "Shared_Dirty" => totals.shared_dirty_kb += val,
        "Private_Clean" => totals.private_clean_kb += val,
        "Private_Dirty" => totals.private_dirty_kb += val,
        "Swap" => totals.swap_kb += val,
        "SwapPss" => totals.swap_pss_kb += val,
        _ => {}
    }
}

fn print_proc(info: &ProcInfo, report: &ProcMemReport) {
    println!("  pid: {}", info.pid);
    println!("  cmdline: {}", info.cmdline);
    if let Some(exe) = &info.exe {
        println!("  exe: {}", exe.display());
    }
    if let Some(cwd) = &info.cwd {
        println!("  cwd: {}", cwd.display());
    }
    println!(
        "  status: VmRSS={} VmSize={} VmSwap={} RssAnon={} RssFile={} RssShmem={}",
        fmt_mib(report.status.vm_rss_kb),
        fmt_mib(report.status.vm_size_kb),
        fmt_mib(report.status.vm_swap_kb),
        fmt_mib(report.status.rss_anon_kb),
        fmt_mib(report.status.rss_file_kb),
        fmt_mib(report.status.rss_shmem_kb),
    );
    if let Some(rollup) = &report.smaps_rollup {
        println!(
            "  smaps_rollup: Size={} Rss={} Pss={} Private={} Shared={} Swap={}",
            fmt_mib(rollup.size_kb),
            fmt_mib(rollup.rss_kb),
            fmt_mib(rollup.pss_kb),
            fmt_mib(rollup.private_clean_kb + rollup.private_dirty_kb),
            fmt_mib(rollup.shared_clean_kb + rollup.shared_dirty_kb),
            fmt_mib(rollup.swap_kb),
        );
    } else {
        println!("  smaps_rollup: unavailable");
    }
    if !report.pma_smaps_available {
        println!("  pma_maps: unavailable (permission denied)");
    } else if report.pma_map_count > 0 {
        let ratio = rss_ratio_str(report.pma_maps.rss_kb, report.pma_maps.size_kb);
        let alloc = report
            .pma_alloc_bytes
            .map(fmt_bytes)
            .unwrap_or_else(|| "unknown".to_string());
        println!(
            "  pma_maps: count={} Size={} Rss={} Pss={} Private={} Shared={} Swap={} rss_ratio={} alloc_offset={}",
            report.pma_map_count,
            fmt_mib(report.pma_maps.size_kb),
            fmt_mib(report.pma_maps.rss_kb),
            fmt_mib(report.pma_maps.pss_kb),
            fmt_mib(report.pma_maps.private_clean_kb + report.pma_maps.private_dirty_kb),
            fmt_mib(report.pma_maps.shared_clean_kb + report.pma_maps.shared_dirty_kb),
            fmt_mib(report.pma_maps.swap_kb),
            ratio,
            alloc,
        );
    } else {
        println!("  pma_maps: none detected");
    }

    if let Some(checkpoint) = &report.checkpoint {
        let latest_name = checkpoint
            .latest_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "<none>".to_string());
        println!(
            "  checkpoints: dir={} files={} total={} latest={} ({})",
            checkpoint.checkpoints_dir.display(),
            checkpoint.file_count,
            fmt_bytes(checkpoint.total_bytes),
            fmt_bytes(checkpoint.latest_bytes),
            latest_name
        );
    } else {
        println!("  checkpoints: unavailable");
    }
}

fn print_comparison(pma: &ProcMemReport, base: &ProcMemReport) {
    println!("Comparison:");
    struct Row {
        label: String,
        pma: String,
        base: String,
        delta: String,
    }
    let mut rows = Vec::new();
    rows.push(Row {
        label: "VmRSS".to_string(),
        pma: fmt_mib(pma.status.vm_rss_kb),
        base: fmt_mib(base.status.vm_rss_kb),
        delta: fmt_signed_mib(delta_kb_signed(
            pma.status.vm_rss_kb,
            base.status.vm_rss_kb,
        )),
    });
    rows.push(Row {
        label: "VmSize".to_string(),
        pma: fmt_mib(pma.status.vm_size_kb),
        base: fmt_mib(base.status.vm_size_kb),
        delta: fmt_signed_mib(delta_kb_signed(
            pma.status.vm_size_kb,
            base.status.vm_size_kb,
        )),
    });
    rows.push(Row {
        label: "RssAnon".to_string(),
        pma: fmt_mib(pma.status.rss_anon_kb),
        base: fmt_mib(base.status.rss_anon_kb),
        delta: fmt_signed_mib(delta_kb_signed(
            pma.status.rss_anon_kb,
            base.status.rss_anon_kb,
        )),
    });
    rows.push(Row {
        label: "RssFile".to_string(),
        pma: fmt_mib(pma.status.rss_file_kb),
        base: fmt_mib(base.status.rss_file_kb),
        delta: fmt_signed_mib(delta_kb_signed(
            pma.status.rss_file_kb,
            base.status.rss_file_kb,
        )),
    });
    rows.push(Row {
        label: "VmSwap".to_string(),
        pma: fmt_mib(pma.status.vm_swap_kb),
        base: fmt_mib(base.status.vm_swap_kb),
        delta: fmt_signed_mib(delta_kb_signed(
            pma.status.vm_swap_kb,
            base.status.vm_swap_kb,
        )),
    });

    if pma.pma_smaps_available && pma.pma_map_count > 0 {
        rows.push(Row {
            label: "PMA map size".to_string(),
            pma: fmt_mib(pma.pma_maps.size_kb),
            base: if base.pma_map_count > 0 {
                fmt_mib(base.pma_maps.size_kb)
            } else {
                "n/a".to_string()
            },
            delta: if base.pma_map_count > 0 {
                fmt_signed_mib(delta_kb_signed(
                    pma.pma_maps.size_kb,
                    base.pma_maps.size_kb,
                ))
            } else {
                "n/a".to_string()
            },
        });
        rows.push(Row {
            label: "PMA rss_ratio".to_string(),
            pma: rss_ratio_str(pma.pma_maps.rss_kb, pma.pma_maps.size_kb),
            base: if base.pma_map_count > 0 {
                rss_ratio_str(base.pma_maps.rss_kb, base.pma_maps.size_kb)
            } else {
                "n/a".to_string()
            },
            delta: "n/a".to_string(),
        });
    } else if !pma.pma_smaps_available {
        rows.push(Row {
            label: "PMA map size".to_string(),
            pma: "unavailable".to_string(),
            base: "n/a".to_string(),
            delta: "n/a".to_string(),
        });
        rows.push(Row {
            label: "PMA rss_ratio".to_string(),
            pma: "unavailable".to_string(),
            base: "n/a".to_string(),
            delta: "n/a".to_string(),
        });
    }

    let pma_alloc = pma
        .pma_alloc_bytes
        .map(fmt_bytes)
        .unwrap_or_else(|| "unknown".to_string());
    let base_alloc = base
        .pma_alloc_bytes
        .map(fmt_bytes)
        .unwrap_or_else(|| "n/a".to_string());
    rows.push(Row {
        label: "PMA alloc_offset".to_string(),
        pma: pma_alloc,
        base: base_alloc,
        delta: "n/a".to_string(),
    });

    match (&pma.checkpoint, &base.checkpoint) {
        (Some(pma_ck), Some(base_ck)) => {
            rows.push(Row {
                label: "Checkpoint latest".to_string(),
                pma: fmt_bytes(pma_ck.latest_bytes),
                base: fmt_bytes(base_ck.latest_bytes),
                delta: fmt_signed_bytes(delta_bytes_signed(
                    pma_ck.latest_bytes,
                    base_ck.latest_bytes,
                )),
            });
            rows.push(Row {
                label: "Checkpoint total".to_string(),
                pma: fmt_bytes(pma_ck.total_bytes),
                base: fmt_bytes(base_ck.total_bytes),
                delta: fmt_signed_bytes(delta_bytes_signed(
                    pma_ck.total_bytes,
                    base_ck.total_bytes,
                )),
            });
        }
        _ => {
            rows.push(Row {
                label: "Checkpoint latest".to_string(),
                pma: "n/a".to_string(),
                base: "n/a".to_string(),
                delta: "n/a".to_string(),
            });
            rows.push(Row {
                label: "Checkpoint total".to_string(),
                pma: "n/a".to_string(),
                base: "n/a".to_string(),
                delta: "n/a".to_string(),
            });
        }
    }

    let header_label = "Metric";
    let header_pma = "PMA";
    let header_base = "Base";
    let header_delta = "PMA - base";
    let mut w_label = header_label.len();
    let mut w_pma = header_pma.len();
    let mut w_base = header_base.len();
    let mut w_delta = header_delta.len();
    for row in &rows {
        w_label = w_label.max(row.label.len());
        w_pma = w_pma.max(row.pma.len());
        w_base = w_base.max(row.base.len());
        w_delta = w_delta.max(row.delta.len());
    }

    println!(
        "  {:<w1$}  {:>w2$}  {:>w3$}  {:>w4$}",
        header_label,
        header_pma,
        header_base,
        header_delta,
        w1 = w_label,
        w2 = w_pma,
        w3 = w_base,
        w4 = w_delta
    );
    println!(
        "  {:<w1$}  {:>w2$}  {:>w3$}  {:>w4$}",
        "-".repeat(w_label),
        "-".repeat(w_pma),
        "-".repeat(w_base),
        "-".repeat(w_delta),
        w1 = w_label,
        w2 = w_pma,
        w3 = w_base,
        w4 = w_delta
    );
    for row in rows {
        println!(
            "  {:<w1$}  {:>w2$}  {:>w3$}  {:>w4$}",
            row.label,
            row.pma,
            row.base,
            row.delta,
            w1 = w_label,
            w2 = w_pma,
            w3 = w_base,
            w4 = w_delta
        );
    }
}

fn print_summary(
    pma_proc: &ProcInfo,
    pma: &ProcMemReport,
    base_proc: &ProcInfo,
    base: &ProcMemReport,
) {
    println!("Summary:");
    let pma_maps_ok = pma.pma_smaps_available && pma.pma_map_count > 0;
    if !pma.pma_smaps_available {
        println!(
            "  PMA mapping info unavailable for pid {} (permission denied reading smaps).",
            pma_proc.pid
        );
        println!(
            "  Notes: run as a user with access to /proc/<pid>/smaps to see PMA RSS."
        );
    } else if pma.pma_map_count == 0 {
        println!(
            "  PMA mapping not detected for pid {}. PMA likely not enabled or mapping path unexpected.",
            pma_proc.pid
        );
    } else {
        let pma_size_mib = kb_to_mib(pma.pma_maps.size_kb);
        let pma_rss_mib = kb_to_mib(pma.pma_maps.rss_kb);
        println!(
            "  PMA mapping size {:.1} MiB, RSS {:.1} MiB (ratio {}).",
            pma_size_mib,
            pma_rss_mib,
            rss_ratio_str(pma.pma_maps.rss_kb, pma.pma_maps.size_kb)
        );
    }

    let rss_delta = delta_kb_signed(pma.status.vm_rss_kb, base.status.vm_rss_kb);
    println!(
        "  Total RSS delta (PMA - base): {}.",
        fmt_signed_mib(rss_delta)
    );

    let verdict = if pma_maps_ok {
        let pma_ratio = rss_ratio_value(pma.pma_maps.rss_kb, pma.pma_maps.size_kb);
        let pma_size_mib = kb_to_mib(pma.pma_maps.size_kb);
        let mut score = 0;
        if pma_size_mib > 256.0 {
            score += 1;
        }
        if pma_ratio < 0.9 {
            score += 1;
        }
        if pma.status.rss_file_kb > base.status.rss_file_kb {
            score += 1;
        }
        match score {
            3 => "likely",
            2 => "somewhat likely",
            1 => "inconclusive",
            _ => "unlikely",
        }
    } else {
        "inconclusive"
    };

    println!(
        "  Likelihood PMA paging is working correctly: {}.",
        verdict
    );
    if pma_maps_ok {
        println!(
            "  Notes: PMA paging is best-effort. If ratio ~= 1.0, the PMA may be small/hot or no memory pressure."
        );
    }
    println!(
        "  Processes: PMA pid {} vs base pid {}.",
        pma_proc.pid, base_proc.pid
    );
}

fn fmt_mib(kb: u64) -> String {
    format!("{:.1} MiB", kb_to_mib(kb))
}

fn fmt_bytes(bytes: u64) -> String {
    let mib = (bytes as f64) / (1024.0 * 1024.0);
    if mib >= 1024.0 {
        format!("{:.2} GiB", mib / 1024.0)
    } else {
        format!("{:.1} MiB", mib)
    }
}

fn kb_to_mib(kb: u64) -> f64 {
    (kb as f64) / 1024.0
}

fn delta_kb_signed(a: u64, b: u64) -> i64 {
    (a as i64) - (b as i64)
}

fn delta_bytes_signed(a: u64, b: u64) -> i64 {
    (a as i64) - (b as i64)
}

fn fmt_signed_mib(kb_delta: i64) -> String {
    let sign = if kb_delta < 0 { "-" } else { "+" };
    let value = (kb_delta.abs() as f64) / 1024.0;
    format!("{}{:.*} MiB", sign, 1, value)
}

fn fmt_signed_bytes(bytes_delta: i64) -> String {
    let sign = if bytes_delta < 0 { "-" } else { "+" };
    let value = bytes_delta.abs() as u64;
    format!("{}{}", sign, fmt_bytes(value))
}

fn rss_ratio_str(rss_kb: u64, size_kb: u64) -> String {
    if size_kb == 0 {
        return "n/a".to_string();
    }
    format!("{:.3}", (rss_kb as f64) / (size_kb as f64))
}

fn rss_ratio_value(rss_kb: u64, size_kb: u64) -> f64 {
    if size_kb == 0 {
        return 0.0;
    }
    (rss_kb as f64) / (size_kb as f64)
}
