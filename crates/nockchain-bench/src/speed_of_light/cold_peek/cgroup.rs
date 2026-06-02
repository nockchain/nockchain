use std::ffi::CStr;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{fs, io};

use super::vma::{read_nockstack_vmas, read_pma_vmas, resident_pages, Vma};
use super::{
    ColdEvidenceDetails, ColdForceResult, ColdInitError, ColdOperationsAudit, ColdReclaimAudit,
    ColdStepError, ColdStepOptions, ColdTargetKind, ColdVmaAudit, OffendingVmaResidency,
};

#[derive(Debug)]
struct LeafCgroup {
    parent: PathBuf,
    leaf: PathBuf,
    pid: u32,
}

impl LeafCgroup {
    fn new(parent: PathBuf, leaf: PathBuf, pid: u32) -> Self {
        Self { parent, leaf, pid }
    }

    fn reclaim_path(&self) -> PathBuf {
        self.leaf.join("memory.reclaim")
    }

    fn cleanup(&self) {
        let _ = fs::write(self.parent.join("cgroup.procs"), format!("{}\n", self.pid));
        let _ = fs::remove_dir(&self.leaf);
    }
}

impl Drop for LeafCgroup {
    fn drop(&mut self) {
        self.cleanup();
    }
}

#[derive(Debug)]
pub struct ColdRuntime {
    leaf: LeafCgroup,
    cold_mode: crate::speed_of_light::ColdMode,
    target: Option<ColdTarget>,
}

#[derive(Debug, Clone)]
struct ColdTarget {
    kind: ColdTargetKind,
    components: Vec<ColdTargetComponent>,
}

#[derive(Debug, Clone)]
struct ColdTargetComponent {
    vmas: Vec<Vma>,
    sync_before_reclaim: bool,
    pageout_before_reclaim: bool,
    reclaim_swappiness: Option<u8>,
}

impl ColdTarget {
    fn pma_replay(vmas: Vec<Vma>, fsync: bool) -> Self {
        Self {
            kind: ColdTargetKind::PmaReplay,
            components: vec![ColdTargetComponent::pma_replay(vmas, fsync)],
        }
    }

    fn pma_replay_nockstack(pma_vmas: Vec<Vma>, nockstack_vmas: Vec<Vma>, fsync: bool) -> Self {
        Self {
            kind: ColdTargetKind::PmaReplayNockStack,
            components: vec![
                ColdTargetComponent::pma_replay(pma_vmas, fsync),
                ColdTargetComponent::nockstack(nockstack_vmas),
            ],
        }
    }

    fn nockstack(vmas: Vec<Vma>) -> Self {
        Self {
            kind: ColdTargetKind::NockStack,
            components: vec![ColdTargetComponent::nockstack(vmas)],
        }
    }

    fn verify_vmas(&self) -> Vec<Vma> {
        self.components
            .iter()
            .flat_map(|component| component.vmas.iter().cloned())
            .collect()
    }

    #[cfg(test)]
    fn reclaim_swappinesses(&self) -> Vec<Option<u8>> {
        self.components
            .iter()
            .map(|component| component.reclaim_swappiness)
            .collect()
    }

    #[cfg(test)]
    fn primary_component(&self) -> &ColdTargetComponent {
        self.components.first().expect("target has component")
    }

    #[cfg(test)]
    fn component_count(&self) -> usize {
        self.components.len()
    }

    #[cfg(test)]
    fn kind(&self) -> ColdTargetKind {
        self.kind
    }

    #[cfg(test)]
    fn sync_before_reclaim(&self) -> bool {
        self.primary_component().sync_before_reclaim
    }

    #[cfg(test)]
    fn pageout_before_reclaim(&self) -> bool {
        self.primary_component().pageout_before_reclaim
    }

    #[cfg(test)]
    fn reclaim_swappiness(&self) -> Option<u8> {
        self.primary_component().reclaim_swappiness
    }
}

impl ColdTargetComponent {
    fn pma_replay(vmas: Vec<Vma>, fsync: bool) -> Self {
        Self {
            vmas,
            sync_before_reclaim: fsync,
            pageout_before_reclaim: true,
            reclaim_swappiness: Some(0),
        }
    }

    fn nockstack(vmas: Vec<Vma>) -> Self {
        Self {
            vmas,
            sync_before_reclaim: false,
            pageout_before_reclaim: true,
            reclaim_swappiness: Some(200),
        }
    }
}

impl ColdRuntime {
    pub fn startup_if_needed(
        has_cold_steps: bool,
        cold_mode: crate::speed_of_light::ColdMode,
    ) -> Result<Option<Self>, ColdInitError> {
        if !has_cold_steps {
            return Ok(None);
        }

        let parent = test_override_parent_path()
            .or_else(env_override_parent_path)
            .unwrap_or(own_cgroup_path()?);
        ensure_memory_delegated(&parent)?;
        sweep_empty_bench_leaves(&parent);

        let pid = std::process::id();
        let leaf = parent.join(bench_leaf_name(pid));
        fs::create_dir(&leaf).map_err(|source| ColdInitError::LeafCreateFailed {
            errno: source.raw_os_error().unwrap_or(libc::EIO),
            path: leaf.clone(),
        })?;

        probe_memory_reclaim(&leaf, &startup_reclaim_swappinesses()?)?;
        fs::write(leaf.join("cgroup.procs"), format!("{pid}\n"))
            .map_err(|source| classify_leaf_join_error(source, &leaf))?;

        Ok(Some(Self {
            leaf: LeafCgroup::new(parent, leaf, pid),
            cold_mode,
            target: None,
        }))
    }

    pub fn bind_after_boot(&mut self, work_dir: &Path, fsync: bool) -> Result<(), ColdInitError> {
        let target = bind_target_after_boot(work_dir, fsync)?;
        self.target = Some(target);
        Ok(())
    }

    pub fn force_cold(
        &mut self,
        options: ColdStepOptions,
    ) -> Result<ColdForceResult, ColdStepError> {
        let Some(target) = self.target.as_ref() else {
            return Err(ColdStepError::System(
                "cold runtime not bound to target VMAs after boot".to_string(),
            ));
        };

        let mut ops = LiveColdOps::new(self.leaf.reclaim_path());
        force_cold_with_ops(&mut ops, target, options, self.cold_mode)
    }
}

fn startup_reclaim_swappinesses() -> Result<Vec<Option<u8>>, ColdInitError> {
    Ok(match cold_target_selection()? {
        ColdTargetSelection::PmaReplay => vec![Some(0)],
        ColdTargetSelection::NockStack => vec![Some(200)],
        ColdTargetSelection::PmaReplayNockStack => vec![Some(0), Some(200)],
    })
}

fn bind_target_after_boot(work_dir: &Path, fsync: bool) -> Result<ColdTarget, ColdInitError> {
    match cold_target_selection()? {
        ColdTargetSelection::PmaReplay => {
            let vmas = read_pma_vmas(work_dir).map_err(|_| ColdInitError::NoPmaVmas)?;
            if vmas.is_empty() {
                return Err(ColdInitError::NoPmaVmas);
            }
            Ok(ColdTarget::pma_replay(vmas, fsync))
        }
        ColdTargetSelection::NockStack => bind_nockstack_target(),
        ColdTargetSelection::PmaReplayNockStack => {
            let pma_vmas = read_pma_vmas(work_dir).map_err(|_| ColdInitError::NoPmaVmas)?;
            if pma_vmas.is_empty() {
                return Err(ColdInitError::NoPmaVmas);
            }
            let nockstack_vmas =
                read_nockstack_vmas().map_err(|_| ColdInitError::NoNockStackVma)?;
            if nockstack_vmas.is_empty() {
                return Err(ColdInitError::NoNockStackVma);
            }
            Ok(ColdTarget::pma_replay_nockstack(
                pma_vmas, nockstack_vmas, fsync,
            ))
        }
    }
}

fn bind_nockstack_target() -> Result<ColdTarget, ColdInitError> {
    let vmas = read_nockstack_vmas().map_err(|_| ColdInitError::NoNockStackVma)?;
    if vmas.is_empty() {
        return Err(ColdInitError::NoNockStackVma);
    }
    Ok(ColdTarget::nockstack(vmas))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColdTargetSelection {
    PmaReplay,
    NockStack,
    PmaReplayNockStack,
}

fn cold_target_selection() -> Result<ColdTargetSelection, ColdInitError> {
    const ENV: &str = "NOCKCHAIN_BENCH_COLD_TARGET";
    match std::env::var(ENV) {
        Ok(value) => match value.as_str() {
            "pma_replay" => Ok(ColdTargetSelection::PmaReplay),
            "nockstack" => Ok(ColdTargetSelection::NockStack),
            "pma_replay_nockstack" | "pma_replay+nockstack" | "combined" => {
                Ok(ColdTargetSelection::PmaReplayNockStack)
            }
            _ => Err(ColdInitError::InvalidColdTargetOverride { value }),
        },
        Err(std::env::VarError::NotPresent) => Ok(ColdTargetSelection::PmaReplayNockStack),
        Err(std::env::VarError::NotUnicode(value)) => {
            Err(ColdInitError::InvalidColdTargetOverride {
                value: value.to_string_lossy().into_owned(),
            })
        }
    }
}

trait ColdOps {
    fn sync_vmas(&mut self, vmas: &[Vma]) -> Result<(), ColdStepError>;
    fn pageout_vmas(&mut self, vmas: &[Vma]) -> Result<(), ColdStepError>;
    fn reclaim(&mut self, bytes: u64, swappiness: Option<u8>) -> Result<(), ColdStepError>;
    fn verify(&mut self, vmas: &[Vma]) -> Result<ColdVerifySummary, ColdStepError>;
    fn cgroup_path(&self) -> Option<PathBuf> {
        None
    }
    fn memory_reclaim_writable(&self) -> Option<bool> {
        None
    }
    fn eagain_seen(&self) -> bool {
        false
    }
}

struct LiveColdOps {
    reclaim_path: PathBuf,
    eagain_seen: bool,
}

impl LiveColdOps {
    fn new(reclaim_path: PathBuf) -> Self {
        Self {
            reclaim_path,
            eagain_seen: false,
        }
    }
}

impl ColdOps for LiveColdOps {
    fn sync_vmas(&mut self, vmas: &[Vma]) -> Result<(), ColdStepError> {
        for vma in vmas {
            let ret =
                unsafe { libc::msync(vma.start as *mut libc::c_void, vma.len(), libc::MS_SYNC) };
            if ret != 0 {
                return Err(ColdStepError::System(format!(
                    "msync(MS_SYNC) failed for {}: {}",
                    vma.path.display(),
                    io::Error::last_os_error()
                )));
            }
        }
        Ok(())
    }

    fn pageout_vmas(&mut self, vmas: &[Vma]) -> Result<(), ColdStepError> {
        for vma in vmas {
            let ret = unsafe {
                libc::madvise(
                    vma.start as *mut libc::c_void,
                    vma.len(),
                    libc::MADV_PAGEOUT,
                )
            };
            if ret != 0 {
                return Err(ColdStepError::System(format!(
                    "madvise(MADV_PAGEOUT) failed for {}: {}",
                    vma.path.display(),
                    io::Error::last_os_error()
                )));
            }
        }
        Ok(())
    }

    fn reclaim(&mut self, bytes: u64, swappiness: Option<u8>) -> Result<(), ColdStepError> {
        let reclaim_request = memory_reclaim_payload(bytes, swappiness);
        match fs::write(&self.reclaim_path, reclaim_request) {
            Ok(()) => Ok(()),
            Err(source) if source.raw_os_error() == Some(libc::EAGAIN) => {
                self.eagain_seen = true;
                Ok(())
            }
            Err(source) => Err(ColdStepError::System(format!(
                "memory.reclaim failed for {}: {}",
                self.reclaim_path.display(),
                source
            ))),
        }
    }

    fn verify(&mut self, vmas: &[Vma]) -> Result<ColdVerifySummary, ColdStepError> {
        let mut resident = 0u64;
        let mut total = 0u64;
        let mut offending_vma = None;
        let mut vmas_after = Vec::with_capacity(vmas.len());

        for vma in vmas {
            let (vma_resident, vma_total) = resident_pages(vma).map_err(|source| {
                ColdStepError::System(format!(
                    "mincore verify failed for {}: {}",
                    vma.path.display(),
                    source
                ))
            })?;
            resident = resident.saturating_add(vma_resident as u64);
            total = total.saturating_add(vma_total as u64);
            if offending_vma.is_none() && vma_resident > 0 {
                offending_vma = Some(OffendingVmaResidency {
                    path: vma.path.clone(),
                    resident_pages: vma_resident as u64,
                    total_pages: vma_total as u64,
                });
            }
            vmas_after.push(ColdVmaAudit {
                start: vma.start,
                end: vma.end,
                path: vma.path.clone(),
                total_pages: vma_total as u64,
                resident_pages_after: vma_resident as u64,
            });
        }

        Ok(ColdVerifySummary {
            residency_pages_after: resident,
            residency_total_pages: total,
            offending_vma,
            vmas_after,
        })
    }

    fn cgroup_path(&self) -> Option<PathBuf> {
        self.reclaim_path.parent().map(Path::to_path_buf)
    }

    fn memory_reclaim_writable(&self) -> Option<bool> {
        Some(true)
    }

    fn eagain_seen(&self) -> bool {
        self.eagain_seen
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ColdVerifySummary {
    residency_pages_after: u64,
    residency_total_pages: u64,
    offending_vma: Option<OffendingVmaResidency>,
    vmas_after: Vec<ColdVmaAudit>,
}

fn force_cold_with_ops(
    ops: &mut impl ColdOps,
    target: &ColdTarget,
    options: ColdStepOptions,
    _cold_mode: crate::speed_of_light::ColdMode,
) -> Result<ColdForceResult, ColdStepError> {
    let max_attempts = options.max_attempts.max(1);
    for component in &target.components {
        if component.sync_before_reclaim {
            ops.sync_vmas(&component.vmas)?;
        }
    }
    for component in &target.components {
        if component.pageout_before_reclaim {
            ops.pageout_vmas(&component.vmas)?;
        }
    }

    let mut last_summary = ColdVerifySummary {
        residency_pages_after: 0,
        residency_total_pages: 0,
        offending_vma: None,
        vmas_after: Vec::new(),
    };
    let verify_vmas = target.verify_vmas();
    let reclaim_bytes_per_attempt = target
        .components
        .iter()
        .fold(0u64, |target_sum, component| {
            target_sum.saturating_add(component.vmas.iter().fold(0u64, |sum, vma| {
                sum.saturating_add(vma.len().try_into().unwrap_or(u64::MAX))
            }))
        });
    for attempt in 1..=max_attempts {
        for component in &target.components {
            let reclaim_bytes = component.vmas.iter().fold(0u64, |sum, vma| {
                sum.saturating_add(vma.len().try_into().unwrap_or(u64::MAX))
            });
            ops.reclaim(reclaim_bytes, component.reclaim_swappiness)?;
        }
        let summary = ops.verify(&verify_vmas)?;
        last_summary = summary;

        if last_summary.residency_pages_after <= options.tolerance_pages {
            return Ok(ColdForceResult {
                cold_target: target.kind,
                cold_verified: true,
                residency_pages_after: last_summary.residency_pages_after,
                residency_total_pages: last_summary.residency_total_pages,
                cold_attempts: attempt,
                degraded_reason: None,
                evidence: cold_evidence_details(
                    ops,
                    target,
                    &last_summary,
                    Some(reclaim_bytes_per_attempt.saturating_mul(attempt as u64)),
                    true,
                ),
            });
        }
    }

    Ok(ColdForceResult {
        cold_target: target.kind,
        cold_verified: false,
        residency_pages_after: last_summary.residency_pages_after,
        residency_total_pages: last_summary.residency_total_pages,
        cold_attempts: max_attempts,
        degraded_reason: Some("partial_pageout".to_string()),
        evidence: cold_evidence_details(
            ops,
            target,
            &last_summary,
            Some(reclaim_bytes_per_attempt.saturating_mul(max_attempts as u64)),
            false,
        ),
    })
}

fn cold_evidence_details(
    ops: &impl ColdOps,
    target: &ColdTarget,
    summary: &ColdVerifySummary,
    bytes_requested: Option<u64>,
    cold_verified: bool,
) -> ColdEvidenceDetails {
    ColdEvidenceDetails {
        reclaim: ColdReclaimAudit {
            cgroup_path: ops.cgroup_path(),
            memory_reclaim_writable: ops.memory_reclaim_writable(),
            swappiness_values: target
                .components
                .iter()
                .filter_map(|component| component.reclaim_swappiness)
                .map(|value| value.to_string())
                .collect(),
            bytes_requested,
            eagain_seen: ops.eagain_seen(),
        },
        vmas: summary.vmas_after.clone(),
        operations: ColdOperationsAudit {
            msync: if target
                .components
                .iter()
                .any(|component| component.sync_before_reclaim)
            {
                "ok".to_string()
            } else {
                "not_applicable".to_string()
            },
            madvise_pageout: if target
                .components
                .iter()
                .any(|component| component.pageout_before_reclaim)
            {
                "ok".to_string()
            } else {
                "not_applicable".to_string()
            },
            memory_reclaim: if cold_verified {
                "ok".to_string()
            } else {
                "unverified".to_string()
            },
            mincore: "ok".to_string(),
        },
    }
}

pub fn own_cgroup_path() -> Result<PathBuf, ColdInitError> {
    let contents =
        fs::read_to_string("/proc/self/cgroup").map_err(|_| ColdInitError::ReclaimUnsupported)?;
    own_cgroup_path_from_str(&contents)
}

fn own_cgroup_path_from_str(contents: &str) -> Result<PathBuf, ColdInitError> {
    for line in contents.lines() {
        let mut parts = line.splitn(3, ':');
        let hierarchy = parts.next();
        let _controllers = parts.next();
        let path = parts.next();
        if hierarchy == Some("0") {
            let relative = path.unwrap_or_default().trim_start_matches('/');
            return Ok(PathBuf::from("/sys/fs/cgroup").join(relative));
        }
    }
    Err(ColdInitError::ReclaimUnsupported)
}

fn env_override_parent_path() -> Option<PathBuf> {
    std::env::var_os("NOCKCHAIN_BENCH_COLD_CGROUP_PARENT").map(PathBuf::from)
}

pub fn parse_subtree_control_tokens(contents: &str) -> Vec<&str> {
    contents
        .split_whitespace()
        .map(|token| token.trim_start_matches(['+', '-']))
        .filter(|token| !token.is_empty())
        .collect()
}

fn subtree_control_has_controller(contents: &str, controller: &str) -> bool {
    parse_subtree_control_tokens(contents)
        .into_iter()
        .any(|token| token == controller)
}

fn ensure_memory_delegated(parent: &Path) -> Result<(), ColdInitError> {
    let contents = fs::read_to_string(parent.join("cgroup.subtree_control"))
        .map_err(|_| ColdInitError::NoDelegatedMemory)?;
    if subtree_control_has_controller(&contents, "memory") {
        Ok(())
    } else {
        Err(ColdInitError::NoDelegatedMemory)
    }
}

fn sweep_empty_bench_leaves(parent: &Path) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };

    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if !file_name.starts_with("bench-") {
            continue;
        }

        let cgroup_procs = entry.path().join("cgroup.procs");
        let Ok(contents) = fs::read_to_string(&cgroup_procs) else {
            continue;
        };
        if !contents.trim().is_empty() {
            continue;
        }

        let _ = fs::remove_dir(entry.path());
    }
}

fn memory_reclaim_payload(bytes: u64, swappiness: Option<u8>) -> String {
    match swappiness {
        Some(swappiness) => format!("{bytes} swappiness={swappiness}"),
        None => bytes.to_string(),
    }
}

fn probe_memory_reclaim(leaf: &Path, swappinesses: &[Option<u8>]) -> Result<(), ColdInitError> {
    if let Some(result) = test_override_probe_result() {
        return result;
    }

    let reclaim_path = leaf.join("memory.reclaim");
    fs::write(&reclaim_path, "0")
        .map_err(|source| classify_reclaim_probe_error(source, false, kernel_release_string()))?;
    for swappiness in swappinesses.iter().copied().flatten() {
        fs::write(&reclaim_path, memory_reclaim_payload(0, Some(swappiness))).map_err(
            |source| classify_reclaim_probe_error(source, true, kernel_release_string()),
        )?;
    }
    Ok(())
}

fn classify_reclaim_probe_error(
    source: io::Error,
    swappiness_probe: bool,
    kernel_release: String,
) -> ColdInitError {
    let errno = source.raw_os_error().unwrap_or(libc::EIO);
    classify_reclaim_probe_errno(errno, swappiness_probe, kernel_release)
}

fn classify_reclaim_probe_errno(
    errno: i32,
    swappiness_probe: bool,
    kernel_release: String,
) -> ColdInitError {
    if errno == libc::EINVAL && swappiness_probe {
        ColdInitError::SwappinessKeyUnsupported {
            found_kernel: kernel_release,
        }
    } else if errno == libc::ENOENT {
        ColdInitError::ReclaimUnsupported
    } else {
        ColdInitError::ReclaimProbeFailed { errno }
    }
}

fn classify_leaf_join_error(source: io::Error, leaf: &Path) -> ColdInitError {
    ColdInitError::LeafCreateFailed {
        errno: source.raw_os_error().unwrap_or(libc::EIO),
        path: leaf.to_path_buf(),
    }
}

fn bench_leaf_name(pid: u32) -> String {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    format!("bench-{pid}-{seed:08x}")
}

fn kernel_release_string() -> String {
    let mut uts = std::mem::MaybeUninit::<libc::utsname>::uninit();
    let ret = unsafe { libc::uname(uts.as_mut_ptr()) };
    if ret != 0 {
        return "unknown".to_string();
    }
    let uts = unsafe { uts.assume_init() };
    unsafe { CStr::from_ptr(uts.release.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
#[derive(Debug, Clone, Default)]
struct ColdInitTestOverrides {
    parent: Option<PathBuf>,
    probe_result: Option<Result<(), ColdInitError>>,
}

#[cfg(test)]
static COLD_INIT_TEST_OVERRIDES: LazyLock<Mutex<ColdInitTestOverrides>> =
    LazyLock::new(|| Mutex::new(ColdInitTestOverrides::default()));

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct ColdInitTestOverrideGuard {
    previous: ColdInitTestOverrides,
}

#[cfg(test)]
impl Drop for ColdInitTestOverrideGuard {
    fn drop(&mut self) {
        *COLD_INIT_TEST_OVERRIDES
            .lock()
            .expect("test overrides mutex") = self.previous.clone();
    }
}

#[cfg(test)]
pub(crate) fn set_test_cold_init_overrides(
    parent: Option<PathBuf>,
    probe_result: Option<Result<(), ColdInitError>>,
) -> ColdInitTestOverrideGuard {
    let mut overrides = COLD_INIT_TEST_OVERRIDES
        .lock()
        .expect("test overrides mutex");
    let previous = overrides.clone();
    *overrides = ColdInitTestOverrides {
        parent,
        probe_result,
    };
    ColdInitTestOverrideGuard { previous }
}

#[cfg(test)]
fn test_override_parent_path() -> Option<PathBuf> {
    COLD_INIT_TEST_OVERRIDES
        .lock()
        .expect("test overrides mutex")
        .parent
        .clone()
}

#[cfg(not(test))]
fn test_override_parent_path() -> Option<PathBuf> {
    None
}

#[cfg(test)]
fn test_override_probe_result() -> Option<Result<(), ColdInitError>> {
    COLD_INIT_TEST_OVERRIDES
        .lock()
        .expect("test overrides mutex")
        .probe_result
        .clone()
}

#[cfg(not(test))]
fn test_override_probe_result() -> Option<Result<(), ColdInitError>> {
    None
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::speed_of_light::ColdMode;

    #[derive(Default)]
    struct FakeColdOps {
        calls: Vec<&'static str>,
        reclaim_calls: Vec<(u64, Option<u8>)>,
        verify_results: Vec<ColdVerifySummary>,
    }

    impl FakeColdOps {
        fn new(verify_results: &[ColdVerifySummary]) -> Self {
            Self {
                calls: Vec::new(),
                reclaim_calls: Vec::new(),
                verify_results: verify_results.to_vec(),
            }
        }
    }

    impl ColdOps for FakeColdOps {
        fn sync_vmas(&mut self, _vmas: &[Vma]) -> Result<(), ColdStepError> {
            self.calls.push("msync");
            Ok(())
        }

        fn pageout_vmas(&mut self, _vmas: &[Vma]) -> Result<(), ColdStepError> {
            self.calls.push("pageout");
            Ok(())
        }

        fn reclaim(&mut self, bytes: u64, swappiness: Option<u8>) -> Result<(), ColdStepError> {
            self.calls.push("reclaim");
            self.reclaim_calls.push((bytes, swappiness));
            Ok(())
        }

        fn verify(&mut self, _vmas: &[Vma]) -> Result<ColdVerifySummary, ColdStepError> {
            self.calls.push("verify");
            Ok(self.verify_results.remove(0))
        }
    }

    fn verify_summary(
        residency_pages_after: u64,
        residency_total_pages: u64,
        offending_vma: Option<OffendingVmaResidency>,
    ) -> ColdVerifySummary {
        ColdVerifySummary {
            residency_pages_after,
            residency_total_pages,
            offending_vma,
            vmas_after: Vec::new(),
        }
    }

    #[test]
    fn own_cgroup_path_uses_v2_entry() {
        let path = own_cgroup_path_from_str("0::/user.slice/user-1000.slice/session.scope\n")
            .expect("cgroup path");
        assert_eq!(
            path,
            PathBuf::from("/sys/fs/cgroup/user.slice/user-1000.slice/session.scope")
        );
    }

    #[test]
    fn subtree_control_parser_keeps_exact_tokens() {
        let tokens = parse_subtree_control_tokens("+cpu +memory +io");
        assert_eq!(tokens, vec!["cpu", "memory", "io"]);
    }

    #[test]
    fn subtree_control_memory_detection_is_exact_token_match() {
        assert!(subtree_control_has_controller("+cpu +memory +io", "memory"));
        assert!(!subtree_control_has_controller(
            "+cpu +memoryswap +io", "memory"
        ));
        assert!(!subtree_control_has_controller(
            "+cpu +mem ory +io", "memory"
        ));
    }

    #[test]
    fn reclaim_probe_einval_on_swappiness_maps_to_specific_error() {
        let error = classify_reclaim_probe_errno(libc::EINVAL, true, "6.10.0-test".to_string());
        assert_eq!(
            error,
            ColdInitError::SwappinessKeyUnsupported {
                found_kernel: "6.10.0-test".to_string()
            }
        );
    }

    #[test]
    fn reclaim_probe_eacces_maps_to_generic_probe_failure() {
        let error = classify_reclaim_probe_errno(libc::EACCES, true, "6.11.0-test".to_string());
        assert_eq!(
            error,
            ColdInitError::ReclaimProbeFailed {
                errno: libc::EACCES
            }
        );
    }

    #[test]
    fn reclaim_payload_is_target_aware() {
        assert_eq!(memory_reclaim_payload(4096, Some(0)), "4096 swappiness=0");
        assert_eq!(
            memory_reclaim_payload(8192, Some(200)),
            "8192 swappiness=200"
        );
        assert_eq!(memory_reclaim_payload(16384, None), "16384");
    }

    #[test]
    fn reclaim_probe_uses_selected_target_swappiness_payload() {
        let temp_dir = tempdir().expect("temp dir");
        std::fs::write(temp_dir.path().join("memory.reclaim"), "").expect("memory.reclaim");

        probe_memory_reclaim(temp_dir.path(), &[Some(200)]).expect("probe");

        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("memory.reclaim"))
                .expect("read memory.reclaim"),
            "0 swappiness=200"
        );
    }

    #[test]
    fn leaf_join_failure_maps_to_leaf_create_failed() {
        let leaf = PathBuf::from("/sys/fs/cgroup/bench-123");
        let error = classify_leaf_join_error(io::Error::from_raw_os_error(libc::EACCES), &leaf);
        assert_eq!(
            error,
            ColdInitError::LeafCreateFailed {
                errno: libc::EACCES,
                path: leaf,
            }
        );
    }

    #[test]
    fn startup_if_needed_is_noop_for_warm_only_plans() {
        let runtime = ColdRuntime::startup_if_needed(false, ColdMode::Strict).expect("startup");
        assert!(runtime.is_none());
    }

    #[test]
    fn bind_after_boot_is_the_only_phase_that_reports_no_pma_vmas() {
        let temp_dir = tempdir().expect("temp dir");
        let mut runtime = ColdRuntime {
            leaf: LeafCgroup::new(
                temp_dir.path().join("parent"),
                temp_dir.path().join("leaf"),
                std::process::id(),
            ),
            cold_mode: ColdMode::Strict,
            target: None,
        };

        let error = runtime
            .bind_after_boot(temp_dir.path(), false)
            .expect_err("bind should fail without replay-pma VMAs");
        assert_eq!(error, ColdInitError::NoPmaVmas);
    }

    #[test]
    fn pma_target_pages_out_and_preserves_fsync_and_file_cache_reclaim_bias() {
        let vmas = vec![Vma {
            start: 0x1000,
            end: 0x2000,
            perms: "rw-s".to_string(),
            path: PathBuf::from("/tmp/work/replay-pma/slab-0.bin"),
        }];

        let target = ColdTarget::pma_replay(vmas, true);

        assert_eq!(target.kind(), ColdTargetKind::PmaReplay);
        assert!(target.sync_before_reclaim());
        assert!(target.pageout_before_reclaim());
        assert_eq!(target.reclaim_swappiness(), Some(0));
    }

    #[test]
    fn nockstack_target_skips_msync_and_uses_anon_reclaim_bias() {
        let vmas = vec![Vma {
            start: 0x1000,
            end: 0x2000,
            perms: "rw-p".to_string(),
            path: PathBuf::from("[anon:nockstack]"),
        }];

        let target = ColdTarget::nockstack(vmas);

        assert_eq!(target.kind(), ColdTargetKind::NockStack);
        assert!(!target.sync_before_reclaim());
        assert!(target.pageout_before_reclaim());
        assert_eq!(target.reclaim_swappiness(), Some(200));
    }

    #[test]
    fn combined_pma_nockstack_target_uses_separate_reclaim_biases() {
        let pma_vmas = vec![Vma {
            start: 0x1000,
            end: 0x3000,
            perms: "rw-s".to_string(),
            path: PathBuf::from("/tmp/work/replay-pma/slab-0.bin"),
        }];
        let nockstack_vmas = vec![Vma {
            start: 0x4000,
            end: 0x7000,
            perms: "rw-p".to_string(),
            path: PathBuf::from("[anon:nockstack]"),
        }];
        let target = ColdTarget::pma_replay_nockstack(pma_vmas, nockstack_vmas, true);
        let options = ColdStepOptions {
            tolerance_pages: 0,
            max_attempts: 1,
        };
        let mut ops = FakeColdOps::new(&[verify_summary(0, 5, None)]);

        let result = force_cold_with_ops(&mut ops, &target, options, ColdMode::Strict)
            .expect("combined cold target");

        assert!(result.cold_verified);
        assert_eq!(result.cold_target, ColdTargetKind::PmaReplayNockStack);
        assert_eq!(target.component_count(), 2);
        assert_eq!(target.reclaim_swappinesses(), vec![Some(0), Some(200)]);
        assert_eq!(
            ops.calls,
            vec!["msync", "pageout", "pageout", "reclaim", "reclaim", "verify"]
        );
        assert_eq!(ops.reclaim_calls, vec![(8192, Some(0)), (12288, Some(200))]);
    }

    #[test]
    fn pma_runtime_defaults_to_combined_pma_and_nockstack_target() {
        std::env::remove_var("NOCKCHAIN_BENCH_COLD_TARGET");

        let selection = cold_target_selection().expect("default cold target");

        assert_eq!(selection, ColdTargetSelection::PmaReplayNockStack);
        assert_eq!(
            startup_reclaim_swappinesses().unwrap(),
            vec![Some(0), Some(200)]
        );
    }

    #[test]
    fn force_cold_respects_fsync_setting() {
        let vmas = vec![Vma {
            start: 0x1000,
            end: 0x2000,
            perms: "rw-s".to_string(),
            path: PathBuf::from("/tmp/replay-pma/slab-0.bin"),
        }];
        let options = ColdStepOptions {
            tolerance_pages: 0,
            max_attempts: 3,
        };

        let with_fsync_target = ColdTarget::pma_replay(vmas.clone(), true);
        let mut with_fsync = FakeColdOps::new(&[verify_summary(0, 1, None)]);
        let result = force_cold_with_ops(
            &mut with_fsync,
            &with_fsync_target,
            options,
            ColdMode::Strict,
        )
        .expect("force cold with fsync");
        assert!(result.cold_verified);
        assert_eq!(
            with_fsync.calls,
            vec!["msync", "pageout", "reclaim", "verify"]
        );

        let without_fsync_target = ColdTarget::pma_replay(vmas, false);
        let mut without_fsync = FakeColdOps::new(&[verify_summary(0, 1, None)]);
        let result = force_cold_with_ops(
            &mut without_fsync,
            &without_fsync_target,
            options,
            ColdMode::Strict,
        )
        .expect("force cold without fsync");
        assert!(result.cold_verified);
        assert_eq!(without_fsync.calls, vec!["pageout", "reclaim", "verify"]);
    }

    #[test]
    fn nockstack_force_cold_pages_out_before_reclaim() {
        let vmas = vec![Vma {
            start: 0x1000,
            end: 0x2000,
            perms: "rw-p".to_string(),
            path: PathBuf::from("[anon:nockstack]"),
        }];
        let options = ColdStepOptions {
            tolerance_pages: 0,
            max_attempts: 1,
        };
        let target = ColdTarget::nockstack(vmas);
        let mut ops = FakeColdOps::new(&[verify_summary(0, 1, None)]);

        let result = force_cold_with_ops(&mut ops, &target, options, ColdMode::Strict)
            .expect("force cold with pageout");

        assert!(result.cold_verified);
        assert_eq!(ops.calls, vec!["pageout", "reclaim", "verify"]);
    }

    #[test]
    fn force_cold_retries_without_repeating_msync() {
        let vmas = vec![Vma {
            start: 0x1000,
            end: 0x3000,
            perms: "rw-s".to_string(),
            path: PathBuf::from("/tmp/replay-pma/slab-0.bin"),
        }];
        let options = ColdStepOptions {
            tolerance_pages: 0,
            max_attempts: 3,
        };
        let target = ColdTarget::pma_replay(vmas, true);
        let mut ops = FakeColdOps::new(&[
            verify_summary(
                3,
                8,
                Some(OffendingVmaResidency {
                    path: PathBuf::from("/tmp/replay-pma/slab-0.bin"),
                    resident_pages: 3,
                    total_pages: 8,
                }),
            ),
            verify_summary(0, 8, None),
        ]);

        let result = force_cold_with_ops(&mut ops, &target, options, ColdMode::Strict)
            .expect("force cold retries");

        assert!(result.cold_verified);
        assert_eq!(result.cold_attempts, 2);
        assert_eq!(
            ops.calls,
            vec!["msync", "pageout", "reclaim", "verify", "reclaim", "verify"]
        );
    }

    #[test]
    fn force_cold_soft_mode_returns_unverified_after_retry_budget() {
        let vmas = vec![Vma {
            start: 0x1000,
            end: 0x3000,
            perms: "rw-s".to_string(),
            path: PathBuf::from("/tmp/replay-pma/slab-0.bin"),
        }];
        let options = ColdStepOptions {
            tolerance_pages: 0,
            max_attempts: 2,
        };
        let target = ColdTarget::pma_replay(vmas, false);
        let mut ops = FakeColdOps::new(&[
            verify_summary(
                2,
                8,
                Some(OffendingVmaResidency {
                    path: PathBuf::from("/tmp/replay-pma/slab-0.bin"),
                    resident_pages: 2,
                    total_pages: 8,
                }),
            ),
            verify_summary(
                1,
                8,
                Some(OffendingVmaResidency {
                    path: PathBuf::from("/tmp/replay-pma/slab-0.bin"),
                    resident_pages: 1,
                    total_pages: 8,
                }),
            ),
        ]);

        let result = force_cold_with_ops(&mut ops, &target, options, ColdMode::Soft)
            .expect("soft mode should continue");

        assert!(!result.cold_verified);
        assert_eq!(result.residency_pages_after, 1);
        assert_eq!(result.residency_total_pages, 8);
        assert_eq!(result.cold_attempts, 2);
    }

    #[test]
    fn residency_over_tolerance_returns_partial_pageout_warning() {
        let vmas = vec![Vma {
            start: 0x1000,
            end: 0x3000,
            perms: "rw-s".to_string(),
            path: PathBuf::from("/tmp/replay-pma/slab-0.bin"),
        }];
        let options = ColdStepOptions {
            tolerance_pages: 0,
            max_attempts: 2,
        };
        let offending_vma = OffendingVmaResidency {
            path: PathBuf::from("/tmp/replay-pma/slab-0.bin"),
            resident_pages: 2,
            total_pages: 8,
        };
        let target = ColdTarget::pma_replay(vmas, false);
        let mut ops = FakeColdOps::new(&[
            verify_summary(2, 8, Some(offending_vma.clone())),
            verify_summary(2, 8, Some(offending_vma.clone())),
        ]);

        let result = force_cold_with_ops(&mut ops, &target, options, ColdMode::Strict)
            .expect("residency over tolerance should warn, not fail");

        assert!(!result.cold_verified);
        assert_eq!(result.degraded_reason.as_deref(), Some("partial_pageout"));
        assert_eq!(result.residency_pages_after, 2);
        assert_eq!(result.residency_total_pages, 8);
        assert_eq!(result.cold_attempts, 2);
        assert_eq!(result.evidence.reclaim.bytes_requested, Some(16_384));
        assert_eq!(result.evidence.operations.memory_reclaim, "unverified");
    }
}
