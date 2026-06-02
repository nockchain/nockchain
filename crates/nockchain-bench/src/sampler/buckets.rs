//! Memory attribution and bucketing
//!
//! Categorizes memory mappings into NockStack, heap/other, and file-backed buckets.

use serde::{Deserialize, Serialize};

use super::smaps::{MappingInfo, ProcStatus, SmapsParser};

/// Memory bucket categories
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MemoryBucket {
    /// NockStack: the large anonymous mapping used for Nock computation
    NockStack,
    /// Heap and other anonymous memory (jam buffers, slabs, etc.)
    HeapOther,
    /// Shared libraries and other file-backed mappings
    SharedLibs,
    /// Thread stacks
    ThreadStacks,
}

/// Configuration for memory attribution
#[derive(Debug, Clone)]
pub struct AttributionConfig {
    /// Expected NockStack size in bytes (used to identify the NockStack mapping)
    /// If None, will try to detect the largest anonymous rw-p mapping
    pub nockstack_size_bytes: Option<u64>,

    /// Tolerance for NockStack size matching (as a fraction, e.g., 0.1 = 10%)
    pub nockstack_size_tolerance: f64,
}

impl Default for AttributionConfig {
    fn default() -> Self {
        Self {
            nockstack_size_bytes: None,
            nockstack_size_tolerance: 0.05, // 5% tolerance
        }
    }
}

impl AttributionConfig {
    /// Create a config with a known NockStack size
    pub fn with_nockstack_size(size_bytes: u64) -> Self {
        Self {
            nockstack_size_bytes: Some(size_bytes),
            ..Default::default()
        }
    }
}

/// Attributed memory totals for each bucket
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryAttribution {
    /// Timestamp when this sample was taken (milliseconds since start)
    pub timestamp_ms: u64,

    /// Overall process stats from /proc/<pid>/status
    pub vm_rss_kb: u64,
    pub vm_size_kb: u64,
    pub rss_anon_kb: u64,
    pub rss_file_kb: u64,
    pub vm_swap_kb: u64,

    /// NockStack bucket
    pub nockstack_size_kb: u64,
    pub nockstack_rss_kb: u64,

    /// Heap and other anonymous memory
    pub heap_other_size_kb: u64,
    pub heap_other_rss_kb: u64,

    /// Shared libraries
    pub shared_libs_size_kb: u64,
    pub shared_libs_rss_kb: u64,

    /// Thread stacks
    pub thread_stacks_size_kb: u64,
    pub thread_stacks_rss_kb: u64,

    /// Page faults since process start
    pub minor_faults: u64,
    pub major_faults: u64,
}

impl MemoryAttribution {
    /// Total attributed RSS (should roughly match vm_rss_kb)
    pub fn total_attributed_rss_kb(&self) -> u64 {
        self.nockstack_rss_kb
            + self.heap_other_rss_kb
            + self.shared_libs_rss_kb
            + self.thread_stacks_rss_kb
    }
}

/// Attribute memory from parsed smaps data
pub struct MemoryAttributor {
    config: AttributionConfig,
}

impl MemoryAttributor {
    pub fn new(config: AttributionConfig) -> Self {
        Self { config }
    }

    /// Attribute memory for a process
    pub fn attribute(
        &self,
        status: &ProcStatus,
        mappings: &[MappingInfo],
        page_faults: (u64, u64),
        timestamp_ms: u64,
    ) -> MemoryAttribution {
        let mut result = MemoryAttribution {
            timestamp_ms,
            vm_rss_kb: status.vm_rss_kb,
            vm_size_kb: status.vm_size_kb,
            rss_anon_kb: status.rss_anon_kb,
            rss_file_kb: status.rss_file_kb,
            vm_swap_kb: status.vm_swap_kb,
            minor_faults: page_faults.0,
            major_faults: page_faults.1,
            ..Default::default()
        };

        // Find the NockStack mapping
        let nockstack_mapping = self.find_nockstack(mappings);

        for mapping in mappings {
            let bucket = self.classify_mapping(mapping, nockstack_mapping);

            match bucket {
                MemoryBucket::NockStack => {
                    result.nockstack_size_kb += mapping.totals.size_kb;
                    result.nockstack_rss_kb += mapping.totals.rss_kb;
                }
                MemoryBucket::HeapOther => {
                    result.heap_other_size_kb += mapping.totals.size_kb;
                    result.heap_other_rss_kb += mapping.totals.rss_kb;
                }
                MemoryBucket::SharedLibs => {
                    result.shared_libs_size_kb += mapping.totals.size_kb;
                    result.shared_libs_rss_kb += mapping.totals.rss_kb;
                }
                MemoryBucket::ThreadStacks => {
                    result.thread_stacks_size_kb += mapping.totals.size_kb;
                    result.thread_stacks_rss_kb += mapping.totals.rss_kb;
                }
            }
        }

        result
    }

    /// Find the NockStack mapping
    fn find_nockstack<'a>(&self, mappings: &'a [MappingInfo]) -> Option<&'a MappingInfo> {
        if let Some(expected_size) = self.config.nockstack_size_bytes {
            // Look for a mapping that matches the expected size
            let tolerance = expected_size as f64 * self.config.nockstack_size_tolerance;
            let min_size = expected_size as f64 - tolerance;
            let max_size = expected_size as f64 + tolerance;

            mappings.iter().find(|m| {
                let size = m.size_bytes() as f64;
                m.is_anonymous()
                    && m.perms.starts_with("rw")
                    && !m.is_heap()
                    && !m.is_stack()
                    && size >= min_size
                    && size <= max_size
            })
        } else {
            // Heuristic: find the largest anonymous rw-p mapping that isn't [heap] or [stack]
            mappings
                .iter()
                .filter(|m| {
                    m.is_anonymous()
                        && m.perms.starts_with("rw")
                        && !m.is_heap()
                        && !m.is_stack()
                        && m.size_bytes() > 100 * 1024 * 1024 // > 100 MiB
                })
                .max_by_key(|m| m.size_bytes())
        }
    }

    /// Classify a mapping into a bucket
    fn classify_mapping(
        &self,
        mapping: &MappingInfo,
        nockstack: Option<&MappingInfo>,
    ) -> MemoryBucket {
        // Check if this is the NockStack
        if let Some(ns) = nockstack {
            if mapping.start_addr == ns.start_addr && mapping.end_addr == ns.end_addr {
                return MemoryBucket::NockStack;
            }
        }

        // Check for thread stacks
        if mapping.is_stack() {
            return MemoryBucket::ThreadStacks;
        }

        // Check for shared libraries and other file-backed mappings
        if !mapping.is_anonymous() {
            return MemoryBucket::SharedLibs;
        }

        // Everything else is heap/other anonymous
        MemoryBucket::HeapOther
    }
}

/// Convenience function to sample memory attribution for a process
pub fn sample_process(
    pid: i32,
    config: &AttributionConfig,
    timestamp_ms: u64,
) -> Result<MemoryAttribution, super::smaps::SmapsError> {
    let parser = SmapsParser::new(pid);

    let status = parser.parse_status()?;
    let mappings = parser.parse_smaps()?;
    let page_faults = parser.parse_stat().unwrap_or((0, 0));

    let attributor = MemoryAttributor::new(config.clone());
    Ok(attributor.attribute(&status, &mappings, page_faults, timestamp_ms))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sampler::smaps::MemoryTotals;

    fn make_mapping(
        start: u64,
        size_kb: u64,
        perms: &str,
        path: Option<&str>,
        rss_kb: u64,
    ) -> MappingInfo {
        MappingInfo {
            start_addr: start,
            end_addr: start + size_kb * 1024,
            perms: perms.to_string(),
            path: path.map(String::from),
            totals: MemoryTotals {
                size_kb,
                rss_kb,
                ..Default::default()
            },
        }
    }

    #[test]
    fn test_classify_file_backed_mapping() {
        let config = AttributionConfig::default();
        let attributor = MemoryAttributor::new(config);

        let mmap_file = make_mapping(
            0x7f0000000000,
            1024 * 1024, // 1 GiB
            "rw-s",
            Some("/data/state/arena.mmap"),
            512 * 1024, // 512 MiB RSS
        );

        let bucket = attributor.classify_mapping(&mmap_file, None);
        assert_eq!(bucket, MemoryBucket::SharedLibs);
    }

    #[test]
    fn test_classify_heap_mapping() {
        let config = AttributionConfig::default();
        let attributor = MemoryAttributor::new(config);

        let heap = make_mapping(0x55a000000000, 1024, "rw-p", Some("[heap]"), 512);

        let bucket = attributor.classify_mapping(&heap, None);
        assert_eq!(bucket, MemoryBucket::HeapOther);
    }

    #[test]
    fn test_classify_shared_lib() {
        let config = AttributionConfig::default();
        let attributor = MemoryAttributor::new(config);

        let libc = make_mapping(
            0x7f1000000000,
            2048,
            "r--p",
            Some("/usr/lib/libc.so.6"),
            2048,
        );

        let bucket = attributor.classify_mapping(&libc, None);
        assert_eq!(bucket, MemoryBucket::SharedLibs);
    }

    #[test]
    fn test_classify_thread_stack() {
        let config = AttributionConfig::default();
        let attributor = MemoryAttributor::new(config);

        let stack = make_mapping(0x7ffc00000000, 132, "rw-p", Some("[stack]"), 32);

        let bucket = attributor.classify_mapping(&stack, None);
        assert_eq!(bucket, MemoryBucket::ThreadStacks);
    }

    #[test]
    fn test_find_nockstack_by_size() {
        let config = AttributionConfig::with_nockstack_size(2 * 1024 * 1024 * 1024); // 2 GiB
        let attributor = MemoryAttributor::new(config);

        let mappings = vec![
            make_mapping(0x100000000, 1024, "rw-p", Some("[heap]"), 512), // heap
            make_mapping(0x200000000, 2 * 1024 * 1024, "rw-p", None, 1024 * 1024), // NockStack (2 GiB)
            make_mapping(0x300000000, 512 * 1024, "rw-p", None, 256 * 1024),       // other anon
        ];

        let nockstack = attributor.find_nockstack(&mappings);
        assert!(nockstack.is_some());
        assert_eq!(nockstack.unwrap().start_addr, 0x200000000);
    }

    #[test]
    fn test_find_nockstack_heuristic() {
        let config = AttributionConfig::default(); // No expected size
        let attributor = MemoryAttributor::new(config);

        let mappings = vec![
            make_mapping(0x100000000, 1024, "rw-p", Some("[heap]"), 512), // heap - too small
            make_mapping(0x200000000, 2 * 1024 * 1024, "rw-p", None, 1024 * 1024), // 2 GiB - largest
            make_mapping(0x300000000, 512 * 1024, "rw-p", None, 256 * 1024),       // 512 MiB
        ];

        let nockstack = attributor.find_nockstack(&mappings);
        assert!(nockstack.is_some());
        // Should pick the largest one (2 GiB)
        assert_eq!(nockstack.unwrap().start_addr, 0x200000000);
    }

    #[test]
    fn test_full_attribution() {
        let config = AttributionConfig::with_nockstack_size(2 * 1024 * 1024 * 1024);
        let attributor = MemoryAttributor::new(config);

        let status = ProcStatus {
            vm_rss_kb: 3_000_000,
            vm_size_kb: 50_000_000,
            vm_swap_kb: 0,
            rss_anon_kb: 2_500_000,
            rss_file_kb: 500_000,
            rss_shmem_kb: 0,
        };

        let mappings = vec![
            // NockStack: 2 GiB mapped, 1.5 GiB RSS
            make_mapping(0x100000000, 2 * 1024 * 1024, "rw-p", None, 1_500_000),
            // File-backed mmap: 16 GiB mapped, 500 MiB RSS
            make_mapping(
                0x200000000,
                16 * 1024 * 1024,
                "rw-s",
                Some("/data/state/arena.mmap"),
                500_000,
            ),
            // Heap: 100 MiB mapped, 80 MiB RSS
            make_mapping(0x300000000, 100 * 1024, "rw-p", Some("[heap]"), 80_000),
            // Other anon: 200 MiB mapped, 150 MiB RSS
            make_mapping(0x400000000, 200 * 1024, "rw-p", None, 150_000),
            // Shared lib: 50 MiB mapped, 50 MiB RSS
            make_mapping(
                0x500000000,
                50 * 1024,
                "r--p",
                Some("/usr/lib/libc.so.6"),
                50_000,
            ),
            // Stack: 8 MiB mapped, 1 MiB RSS
            make_mapping(0x600000000, 8 * 1024, "rw-p", Some("[stack]"), 1_000),
        ];

        let result = attributor.attribute(&status, &mappings, (1000, 10), 5000);

        assert_eq!(result.timestamp_ms, 5000);
        assert_eq!(result.vm_rss_kb, 3_000_000);

        // NockStack
        assert_eq!(result.nockstack_size_kb, 2 * 1024 * 1024);
        assert_eq!(result.nockstack_rss_kb, 1_500_000);

        // Heap + other anon
        assert_eq!(result.heap_other_size_kb, 100 * 1024 + 200 * 1024);
        assert_eq!(result.heap_other_rss_kb, 80_000 + 150_000);

        // Shared libs + other file-backed mappings
        assert_eq!(result.shared_libs_size_kb, (16 * 1024 * 1024) + (50 * 1024));
        assert_eq!(result.shared_libs_rss_kb, 500_000 + 50_000);

        // Thread stacks
        assert_eq!(result.thread_stacks_size_kb, 8 * 1024);
        assert_eq!(result.thread_stacks_rss_kb, 1_000);

        // Page faults
        assert_eq!(result.minor_faults, 1000);
        assert_eq!(result.major_faults, 10);
    }
}
