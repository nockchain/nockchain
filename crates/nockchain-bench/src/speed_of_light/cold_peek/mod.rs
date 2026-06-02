#[cfg(not(target_os = "linux"))]
use std::path::Path;
use std::path::PathBuf;

#[cfg(target_os = "linux")]
mod cgroup;
mod measure;
#[cfg(target_os = "linux")]
mod vma;

#[cfg(all(test, target_os = "linux"))]
pub(crate) use cgroup::set_test_cold_init_overrides;
#[cfg(target_os = "linux")]
pub use cgroup::{own_cgroup_path, parse_subtree_control_tokens, ColdRuntime};
pub use measure::{measure_peek, measure_sync, PeekMeasurement, StepMeasurement};
#[cfg(target_os = "linux")]
pub use vma::{
    page_size, parse_proc_maps, read_nockstack_vmas, read_pma_vmas, reduce_mincore_bitmap,
    resident_pages, select_nockstack_vmas_from_maps, Vma,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ColdTargetKind {
    #[serde(rename = "pma_replay")]
    PmaReplay,
    #[serde(rename = "pma_replay_nockstack")]
    PmaReplayNockStack,
    #[serde(rename = "nockstack")]
    NockStack,
    #[serde(rename = "unsupported")]
    Unsupported,
}

impl ColdTargetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PmaReplay => "pma_replay",
            Self::PmaReplayNockStack => "pma_replay_nockstack",
            Self::NockStack => "nockstack",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColdStepOptions {
    pub tolerance_pages: u64,
    pub max_attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffendingVmaResidency {
    pub path: PathBuf,
    pub resident_pages: u64,
    pub total_pages: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColdForceResult {
    pub cold_target: ColdTargetKind,
    pub cold_verified: bool,
    pub residency_pages_after: u64,
    pub residency_total_pages: u64,
    pub cold_attempts: u32,
    pub degraded_reason: Option<String>,
    pub evidence: ColdEvidenceDetails,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct ColdEvidenceDetails {
    pub reclaim: ColdReclaimAudit,
    pub vmas: Vec<ColdVmaAudit>,
    pub operations: ColdOperationsAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct ColdReclaimAudit {
    pub cgroup_path: Option<PathBuf>,
    pub memory_reclaim_writable: Option<bool>,
    pub swappiness_values: Vec<String>,
    pub bytes_requested: Option<u64>,
    pub eagain_seen: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ColdVmaAudit {
    pub start: usize,
    pub end: usize,
    pub path: PathBuf,
    pub total_pages: u64,
    pub resident_pages_after: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ColdOperationsAudit {
    pub msync: String,
    pub madvise_pageout: String,
    pub memory_reclaim: String,
    pub mincore: String,
}

impl Default for ColdOperationsAudit {
    fn default() -> Self {
        Self {
            msync: "not_recorded".to_string(),
            madvise_pageout: "not_recorded".to_string(),
            memory_reclaim: "not_recorded".to_string(),
            mincore: "not_recorded".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ColdStepError {
    #[error("cold verify failed after {cold_attempts} attempts: {message}")]
    VerifyFailed {
        cold_target: ColdTargetKind,
        residency_pages_after: u64,
        residency_total_pages: u64,
        tolerance_pages: u64,
        cold_attempts: u32,
        offending_vma: Option<OffendingVmaResidency>,
        message: String,
    },

    #[error("{0}")]
    System(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ColdInitError {
    #[error("cold peek requires cgroup v2 memory.reclaim support")]
    ReclaimUnsupported,

    #[error("cold peek requires memory.reclaim swappiness support; found kernel {found_kernel}")]
    SwappinessKeyUnsupported { found_kernel: String },

    #[error(
        "cold peek requires a delegated cgroup v2 parent with memory in cgroup.subtree_control"
    )]
    NoDelegatedMemory,

    #[error("failed to create cold peek leaf cgroup {path}: errno {errno}")]
    LeafCreateFailed { errno: i32, path: PathBuf },

    #[error("failed to probe memory.reclaim: errno {errno}")]
    ReclaimProbeFailed { errno: i32 },

    #[error("no PMA VMAs discovered under replay-pma")]
    NoPmaVmas,

    #[error("no strict medium-size NockStack VMA discovered in /proc/self/maps")]
    NoNockStackVma,

    #[error("invalid NOCKCHAIN_BENCH_COLD_TARGET value {value:?}; expected pma_replay, nockstack, or pma_replay_nockstack")]
    InvalidColdTargetOverride { value: String },
}

#[cfg(not(target_os = "linux"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColdRuntime;

#[cfg(not(target_os = "linux"))]
impl ColdRuntime {
    pub fn startup_if_needed(
        has_cold_steps: bool,
        _cold_mode: crate::speed_of_light::ColdMode,
    ) -> Result<Option<Self>, ColdInitError> {
        Ok(has_cold_steps.then_some(Self))
    }

    pub fn bind_after_boot(&mut self, _work_dir: &Path, _fsync: bool) -> Result<(), ColdInitError> {
        Ok(())
    }

    pub fn force_cold(
        &mut self,
        options: ColdStepOptions,
    ) -> Result<ColdForceResult, ColdStepError> {
        Ok(ColdForceResult {
            cold_target: ColdTargetKind::Unsupported,
            cold_verified: false,
            residency_pages_after: 0,
            residency_total_pages: 0,
            cold_attempts: options.max_attempts,
            degraded_reason: Some("macos_unsupported".to_string()),
            evidence: ColdEvidenceDetails::default(),
        })
    }
}
