//! Parser for /proc/<pid>/smaps and /proc/<pid>/status
//!
//! Ported from scripts/compare_mem.rs

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SmapsError {
    #[error("Failed to read file {path}: {source}")]
    IoError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Process {pid} not found")]
    ProcessNotFound { pid: i32 },

    #[error("Permission denied reading {path}")]
    PermissionDenied { path: PathBuf },

    #[error("Invalid smaps format: {details}")]
    ParseError { details: String },
}

/// Memory totals from a mapping or rollup
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryTotals {
    pub size_kb: u64,
    pub rss_kb: u64,
    pub pss_kb: u64,
    pub shared_clean_kb: u64,
    pub shared_dirty_kb: u64,
    pub private_clean_kb: u64,
    pub private_dirty_kb: u64,
    pub swap_kb: u64,
    pub swap_pss_kb: u64,
}

impl MemoryTotals {
    /// Total private memory (clean + dirty)
    pub fn private_kb(&self) -> u64 {
        self.private_clean_kb + self.private_dirty_kb
    }

    /// Total shared memory (clean + dirty)
    pub fn shared_kb(&self) -> u64 {
        self.shared_clean_kb + self.shared_dirty_kb
    }

    /// Add another MemoryTotals to this one
    pub fn add(&mut self, other: &MemoryTotals) {
        self.size_kb += other.size_kb;
        self.rss_kb += other.rss_kb;
        self.pss_kb += other.pss_kb;
        self.shared_clean_kb += other.shared_clean_kb;
        self.shared_dirty_kb += other.shared_dirty_kb;
        self.private_clean_kb += other.private_clean_kb;
        self.private_dirty_kb += other.private_dirty_kb;
        self.swap_kb += other.swap_kb;
        self.swap_pss_kb += other.swap_pss_kb;
    }
}

/// Memory stats from /proc/<pid>/status
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcStatus {
    pub vm_rss_kb: u64,
    pub vm_size_kb: u64,
    pub vm_swap_kb: u64,
    pub rss_anon_kb: u64,
    pub rss_file_kb: u64,
    pub rss_shmem_kb: u64,
}

/// Information about a single memory mapping
#[derive(Clone, Debug)]
pub struct MappingInfo {
    /// Start address of the mapping
    pub start_addr: u64,
    /// End address of the mapping
    pub end_addr: u64,
    /// Permissions string (e.g., "rw-p")
    pub perms: String,
    /// File path if this is a file-backed mapping
    pub path: Option<String>,
    /// Memory totals for this mapping
    pub totals: MemoryTotals,
}

impl MappingInfo {
    /// Size of the mapping in bytes
    pub fn size_bytes(&self) -> u64 {
        self.end_addr - self.start_addr
    }

    /// Is this an anonymous mapping (no file backing)?
    pub fn is_anonymous(&self) -> bool {
        self.path.is_none()
            || self.path.as_ref().map(|p| p.is_empty()).unwrap_or(true)
            || self
                .path
                .as_ref()
                .map(|p| p.starts_with('['))
                .unwrap_or(false)
    }

    /// Is this the heap?
    pub fn is_heap(&self) -> bool {
        self.path.as_ref().map(|p| p == "[heap]").unwrap_or(false)
    }

    /// Is this a thread stack?
    pub fn is_stack(&self) -> bool {
        self.path
            .as_ref()
            .map(|p| p == "[stack]" || p.starts_with("[stack:"))
            .unwrap_or(false)
    }
}

/// Parser for /proc/<pid>/smaps and related files
pub struct SmapsParser {
    pid: i32,
    proc_base: PathBuf,
}

impl SmapsParser {
    /// Create a parser for the given PID
    pub fn new(pid: i32) -> Self {
        Self {
            pid,
            proc_base: PathBuf::from("/proc").join(pid.to_string()),
        }
    }

    /// Create a parser with a custom proc base path (for testing)
    pub fn with_proc_base(pid: i32, proc_base: PathBuf) -> Self {
        Self { pid, proc_base }
    }

    /// Check if the process exists
    pub fn process_exists(&self) -> bool {
        self.proc_base.exists()
    }

    /// Parse /proc/<pid>/status
    pub fn parse_status(&self) -> Result<ProcStatus, SmapsError> {
        let path = self.proc_base.join("status");
        let contents = self.read_file(&path)?;

        let mut status = ProcStatus::default();
        let map = parse_key_value_kb(&contents);

        status.vm_rss_kb = map.get("VmRSS").copied().unwrap_or(0);
        status.vm_size_kb = map.get("VmSize").copied().unwrap_or(0);
        status.vm_swap_kb = map.get("VmSwap").copied().unwrap_or(0);
        status.rss_anon_kb = map.get("RssAnon").copied().unwrap_or(0);
        status.rss_file_kb = map.get("RssFile").copied().unwrap_or(0);
        status.rss_shmem_kb = map.get("RssShmem").copied().unwrap_or(0);

        Ok(status)
    }

    /// Parse /proc/<pid>/smaps_rollup for overall memory totals
    pub fn parse_smaps_rollup(&self) -> Result<MemoryTotals, SmapsError> {
        let path = self.proc_base.join("smaps_rollup");
        let contents = self.read_file(&path)?;

        let mut totals = MemoryTotals::default();
        for line in contents.lines() {
            if let Some((key, val)) = parse_kb_line(line) {
                assign_mem_total(&mut totals, key, val);
            }
        }

        Ok(totals)
    }

    /// Parse /proc/<pid>/smaps and return all mappings
    pub fn parse_smaps(&self) -> Result<Vec<MappingInfo>, SmapsError> {
        let path = self.proc_base.join("smaps");
        let file = self.open_file(&path)?;
        let reader = BufReader::new(file);

        let mut mappings = Vec::new();
        let mut current_mapping: Option<MappingInfo> = None;

        for line in reader.lines() {
            let line = line.map_err(|e| SmapsError::IoError {
                path: path.clone(),
                source: e,
            })?;

            if let Some((start, end, perms, path_opt)) = parse_mapping_header(&line) {
                // Save previous mapping if any
                if let Some(mapping) = current_mapping.take() {
                    mappings.push(mapping);
                }

                // Start new mapping
                current_mapping = Some(MappingInfo {
                    start_addr: start,
                    end_addr: end,
                    perms,
                    path: path_opt,
                    totals: MemoryTotals::default(),
                });
            } else if let Some(ref mut mapping) = current_mapping {
                // Parse memory stat line
                if let Some((key, val)) = parse_kb_line(&line) {
                    assign_mem_total(&mut mapping.totals, key, val);
                }
            }
        }

        // Don't forget the last mapping
        if let Some(mapping) = current_mapping {
            mappings.push(mapping);
        }

        Ok(mappings)
    }

    /// Parse /proc/<pid>/stat for page fault counts
    pub fn parse_stat(&self) -> Result<(u64, u64), SmapsError> {
        let path = self.proc_base.join("stat");
        let contents = self.read_file(&path)?;

        // /proc/<pid>/stat format: pid (comm) state ... minflt ... majflt ...
        // Fields are space-separated, but comm can contain spaces and is in parens
        let stat_after_comm = contents
            .rfind(')')
            .map(|i| &contents[i + 1..])
            .unwrap_or(&contents);

        let fields: Vec<&str> = stat_after_comm.split_whitespace().collect();

        // After (comm), field indices are:
        // 0: state, 1: ppid, 2: pgrp, 3: session, 4: tty_nr, 5: tpgid,
        // 6: flags, 7: minflt, 8: cminflt, 9: majflt, 10: cmajflt
        let minflt = fields.get(7).and_then(|s| s.parse().ok()).unwrap_or(0);
        let majflt = fields.get(9).and_then(|s| s.parse().ok()).unwrap_or(0);

        Ok((minflt, majflt))
    }

    fn map_io_error(&self, path: &Path, e: std::io::Error) -> SmapsError {
        if e.kind() == std::io::ErrorKind::NotFound {
            SmapsError::ProcessNotFound { pid: self.pid }
        } else if e.kind() == std::io::ErrorKind::PermissionDenied {
            SmapsError::PermissionDenied {
                path: path.to_path_buf(),
            }
        } else {
            SmapsError::IoError {
                path: path.to_path_buf(),
                source: e,
            }
        }
    }

    fn read_file(&self, path: &Path) -> Result<String, SmapsError> {
        std::fs::read_to_string(path).map_err(|e| self.map_io_error(path, e))
    }

    fn open_file(&self, path: &Path) -> Result<File, SmapsError> {
        File::open(path).map_err(|e| self.map_io_error(path, e))
    }
}

/// Parse a mapping header line from smaps
/// Format: "start-end perms offset dev inode pathname"
fn parse_mapping_header(line: &str) -> Option<(u64, u64, String, Option<String>)> {
    let mut parts = line.split_whitespace();

    let range = parts.next()?;
    let (start_str, end_str) = range.split_once('-')?;

    let start = u64::from_str_radix(start_str, 16).ok()?;
    let end = u64::from_str_radix(end_str, 16).ok()?;

    let perms = parts.next()?.to_string();
    if perms.len() < 4 {
        return None;
    }

    // Skip offset, dev, inode
    parts.next()?; // offset
    parts.next()?; // dev
    parts.next()?; // inode

    // Remaining is the path (may be empty or contain spaces)
    let path: String = parts.collect::<Vec<_>>().join(" ");
    let path = if path.is_empty() {
        None
    } else {
        Some(path.trim_end_matches("(deleted)").trim().to_string())
    };

    Some((start, end, perms, path))
}

/// Parse a "Key: value kB" line
fn parse_kb_line(line: &str) -> Option<(&str, u64)> {
    let mut parts = line.split_whitespace();
    let key = parts.next()?.trim_end_matches(':');
    let val = parts.next()?.parse::<u64>().ok()?;
    Some((key, val))
}

/// Parse key-value pairs from status file
fn parse_key_value_kb(contents: &str) -> HashMap<&str, u64> {
    let mut map = HashMap::new();
    for line in contents.lines() {
        if let Some((key, val)) = parse_kb_line(line) {
            map.insert(key, val);
        }
    }
    map
}

/// Assign a memory value to the appropriate field in MemoryTotals
fn assign_mem_total(totals: &mut MemoryTotals, key: &str, val: u64) {
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

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn create_mock_proc(dir: &Path, pid: i32) -> PathBuf {
        let proc_dir = dir.join(pid.to_string());
        std::fs::create_dir_all(&proc_dir).unwrap();
        proc_dir
    }

    #[test]
    fn test_parse_mapping_header_basic() {
        let line = "7f1234560000-7f1234570000 rw-p 00000000 00:00 0";
        let result = parse_mapping_header(line);
        assert!(result.is_some());

        let (start, end, perms, path) = result.unwrap();
        assert_eq!(start, 0x7f1234560000);
        assert_eq!(end, 0x7f1234570000);
        assert_eq!(perms, "rw-p");
        assert!(path.is_none());
    }

    #[test]
    fn test_parse_mapping_header_with_path() {
        let line = "7f1234560000-7f1234570000 r--p 00000000 08:01 12345 /usr/lib/libc.so.6";
        let result = parse_mapping_header(line);
        assert!(result.is_some());

        let (start, end, perms, path) = result.unwrap();
        assert_eq!(start, 0x7f1234560000);
        assert_eq!(end, 0x7f1234570000);
        assert_eq!(perms, "r--p");
        assert_eq!(path, Some("/usr/lib/libc.so.6".to_string()));
    }

    #[test]
    fn test_parse_mapping_header_file_backed_mmap() {
        let line = "7f1234560000-7f1234570000 rw-s 00000000 00:05 12345 /data/.data.nockchain/state/arena.mmap";
        let result = parse_mapping_header(line);
        assert!(result.is_some());

        let (_, _, _, path) = result.unwrap();
        assert_eq!(
            path,
            Some("/data/.data.nockchain/state/arena.mmap".to_string())
        );
    }

    #[test]
    fn test_parse_mapping_header_heap() {
        let line = "55a1b2c3d000-55a1b2c4e000 rw-p 00000000 00:00 0 [heap]";
        let result = parse_mapping_header(line);
        assert!(result.is_some());

        let (_, _, _, path) = result.unwrap();
        assert_eq!(path, Some("[heap]".to_string()));
    }

    #[test]
    fn test_parse_kb_line() {
        assert_eq!(
            parse_kb_line("Size:               1234 kB"),
            Some(("Size", 1234))
        );
        assert_eq!(
            parse_kb_line("Rss:                 567 kB"),
            Some(("Rss", 567))
        );
        assert_eq!(
            parse_kb_line("VmRSS:            10240 kB"),
            Some(("VmRSS", 10240))
        );
        assert_eq!(parse_kb_line("Not a valid line"), None);
    }

    #[test]
    fn test_mapping_info_is_anonymous() {
        let anon = MappingInfo {
            start_addr: 0,
            end_addr: 4096,
            perms: "rw-p".to_string(),
            path: None,
            totals: MemoryTotals::default(),
        };
        assert!(anon.is_anonymous());

        let heap = MappingInfo {
            start_addr: 0,
            end_addr: 4096,
            perms: "rw-p".to_string(),
            path: Some("[heap]".to_string()),
            totals: MemoryTotals::default(),
        };
        assert!(heap.is_anonymous()); // [heap] counts as anonymous
        assert!(heap.is_heap());

        let file = MappingInfo {
            start_addr: 0,
            end_addr: 4096,
            perms: "r--p".to_string(),
            path: Some("/usr/lib/libc.so.6".to_string()),
            totals: MemoryTotals::default(),
        };
        assert!(!file.is_anonymous());
    }

    #[test]
    fn test_memory_totals_add() {
        let mut a = MemoryTotals {
            size_kb: 100,
            rss_kb: 50,
            pss_kb: 40,
            shared_clean_kb: 10,
            shared_dirty_kb: 5,
            private_clean_kb: 20,
            private_dirty_kb: 15,
            swap_kb: 0,
            swap_pss_kb: 0,
        };

        let b = MemoryTotals {
            size_kb: 200,
            rss_kb: 100,
            pss_kb: 80,
            shared_clean_kb: 20,
            shared_dirty_kb: 10,
            private_clean_kb: 40,
            private_dirty_kb: 30,
            swap_kb: 5,
            swap_pss_kb: 3,
        };

        a.add(&b);

        assert_eq!(a.size_kb, 300);
        assert_eq!(a.rss_kb, 150);
        assert_eq!(a.pss_kb, 120);
        assert_eq!(a.shared_clean_kb, 30);
        assert_eq!(a.shared_dirty_kb, 15);
        assert_eq!(a.private_clean_kb, 60);
        assert_eq!(a.private_dirty_kb, 45);
        assert_eq!(a.swap_kb, 5);
        assert_eq!(a.swap_pss_kb, 3);
    }

    #[test]
    fn test_parse_status_from_mock_file() {
        let dir = TempDir::new().unwrap();
        let proc_dir = create_mock_proc(dir.path(), 1234);

        let status_content = r#"Name:   nockchain
Umask:  0022
State:  S (sleeping)
VmPeak:     123456 kB
VmSize:     100000 kB
VmRSS:       50000 kB
RssAnon:     30000 kB
RssFile:     20000 kB
RssShmem:        0 kB
VmSwap:       1000 kB
"#;

        std::fs::write(proc_dir.join("status"), status_content).unwrap();

        let parser = SmapsParser::with_proc_base(1234, proc_dir);
        let status = parser.parse_status().unwrap();

        assert_eq!(status.vm_size_kb, 100000);
        assert_eq!(status.vm_rss_kb, 50000);
        assert_eq!(status.rss_anon_kb, 30000);
        assert_eq!(status.rss_file_kb, 20000);
        assert_eq!(status.vm_swap_kb, 1000);
    }

    #[test]
    fn test_parse_smaps_from_mock_file() {
        let dir = TempDir::new().unwrap();
        let proc_dir = create_mock_proc(dir.path(), 1234);

        let smaps_content = r#"7f0000000000-7f0000010000 rw-p 00000000 00:00 0
Size:                 64 kB
Rss:                  32 kB
Pss:                  32 kB
Shared_Clean:          0 kB
Shared_Dirty:          0 kB
Private_Clean:        16 kB
Private_Dirty:        16 kB
Swap:                  0 kB
SwapPss:               0 kB
7f0000010000-7f0000020000 r--p 00000000 08:01 12345 /usr/lib/libc.so.6
Size:                 64 kB
Rss:                  64 kB
Pss:                  32 kB
Shared_Clean:         64 kB
Shared_Dirty:          0 kB
Private_Clean:         0 kB
Private_Dirty:         0 kB
Swap:                  0 kB
SwapPss:               0 kB
7f0000020000-7f0000030000 rw-s 00000000 00:05 67890 /data/state/arena.mmap
Size:                 64 kB
Rss:                  48 kB
Pss:                  48 kB
Shared_Clean:          0 kB
Shared_Dirty:          0 kB
Private_Clean:        32 kB
Private_Dirty:        16 kB
Swap:                  0 kB
SwapPss:               0 kB
"#;

        std::fs::write(proc_dir.join("smaps"), smaps_content).unwrap();

        let parser = SmapsParser::with_proc_base(1234, proc_dir);
        let mappings = parser.parse_smaps().unwrap();

        assert_eq!(mappings.len(), 3);

        // First mapping: anonymous
        assert!(mappings[0].is_anonymous());
        assert_eq!(mappings[0].totals.size_kb, 64);
        assert_eq!(mappings[0].totals.rss_kb, 32);

        // Second mapping: libc
        assert!(!mappings[1].is_anonymous());
        assert_eq!(mappings[1].path, Some("/usr/lib/libc.so.6".to_string()));
        assert_eq!(mappings[1].totals.shared_clean_kb, 64);

        // Third mapping: file-backed mapping
        assert!(!mappings[2].is_anonymous());
        assert_eq!(mappings[2].totals.rss_kb, 48);
    }

    #[test]
    fn test_parse_smaps_rollup_from_mock_file() {
        let dir = TempDir::new().unwrap();
        let proc_dir = create_mock_proc(dir.path(), 1234);

        let rollup_content = r#"7f0000000000-7fffffffffff ---p 00000000 00:00 0 [rollup]
Rss:               102400 kB
Pss:                51200 kB
Shared_Clean:       20480 kB
Shared_Dirty:        1024 kB
Private_Clean:      40960 kB
Private_Dirty:      40960 kB
Swap:                   0 kB
SwapPss:                0 kB
"#;

        std::fs::write(proc_dir.join("smaps_rollup"), rollup_content).unwrap();

        let parser = SmapsParser::with_proc_base(1234, proc_dir);
        let totals = parser.parse_smaps_rollup().unwrap();

        assert_eq!(totals.rss_kb, 102400);
        assert_eq!(totals.pss_kb, 51200);
        assert_eq!(totals.shared_clean_kb, 20480);
        assert_eq!(totals.private_kb(), 40960 + 40960);
    }
}
