use std::path::PathBuf;
use std::process::Command;

use serde::{Deserialize, Serialize};

use super::case::{BinaryIdentity, RequestedOrchestrate, ResolvedCase, WorkDirMode};
use super::docker_image::DockerImageSource;
use super::{read_trimmed_file, unix_timestamp_ms, PROVENANCE_SCHEMA_VERSION};
use crate::speed_of_light::boot_source::ResolvedBootSource;
use crate::speed_of_light::fixture::SolFixtureManifest;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostIdentity {
    pub hostname: Option<String>,
    pub os: String,
    pub arch: String,
    pub kernel: Option<String>,
    pub cpu_count: usize,
    pub total_memory_bytes: Option<u64>,
    pub cpu_model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitIdentity {
    pub commit: Option<String>,
    pub branch: Option<String>,
    pub commit_date: Option<String>,
    pub dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostEnvSnapshot {
    pub current_dir: Option<PathBuf>,
    pub shell: Option<String>,
    pub user: Option<String>,
    pub hostname_env: Option<String>,
    pub rust_log: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendRuntimeFacts {
    Native,
    Docker {
        host_binary: BinaryIdentity,
        container_binary: BinaryIdentity,
        image_source: DockerImageSource,
        requested_image_ref: String,
        resolved_image_ref: String,
        image_digest: String,
        container_id: String,
        docker_engine_version: String,
        docker_context: String,
        cgroup_version: String,
        storage_driver: String,
        realized_memory_max: u64,
        realized_memory_current: u64,
        realized_cpuset: Option<String>,
        realized_cpu_max: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub schema_version: String,
    pub capture_timestamp_ms: u128,
    pub host: HostIdentity,
    pub git: Option<GitIdentity>,
    pub backend: BackendRuntimeFacts,
    pub allow_debug_benchmark: bool,
    pub allow_version_skew: bool,
    pub allow_degraded_cold: bool,
    pub cv_threshold: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_flavor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boot_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boot_event_num: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pma_work_dir_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pma_fsync_mode: Option<String>,
    pub binary: BinaryIdentity,
    pub fixture_path: PathBuf,
    pub fixture_sha256_hex: String,
    pub fixture_manifest: SolFixtureManifest,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PmaReplayProvenance {
    pub runtime_flavor: Option<String>,
    pub boot_source: Option<String>,
    pub boot_event_num: Option<u64>,
    pub pma_work_dir_mode: Option<String>,
    pub pma_fsync_mode: Option<String>,
}

impl PmaReplayProvenance {
    pub(crate) fn checkpoint(boot_event_num: u64) -> Self {
        Self {
            runtime_flavor: Some("pma".to_string()),
            boot_source: Some("checkpoint".to_string()),
            boot_event_num: Some(boot_event_num),
            pma_work_dir_mode: None,
            pma_fsync_mode: None,
        }
    }

    pub(crate) fn snapshot(boot_event_num: u64) -> Self {
        Self {
            runtime_flavor: Some("pma".to_string()),
            boot_source: Some("snapshot".to_string()),
            boot_event_num: Some(boot_event_num),
            pma_work_dir_mode: None,
            pma_fsync_mode: None,
        }
    }

    pub(crate) fn with_work_dir_mode(mut self, work_dir_mode: &WorkDirMode) -> Self {
        self.pma_work_dir_mode = Some(work_dir_mode.provenance_label().to_string());
        self
    }

    #[allow(dead_code)]
    pub(crate) fn with_fsync_mode(mut self, fsync_enabled: bool) -> Self {
        self.pma_fsync_mode = Some(super::case::fsync_mode_label(fsync_enabled).to_string());
        self
    }

    fn is_absent(&self) -> bool {
        self.runtime_flavor.is_none()
            && self.boot_source.is_none()
            && self.boot_event_num.is_none()
            && self.pma_work_dir_mode.is_none()
            && self.pma_fsync_mode.is_none()
    }
}

impl Provenance {
    pub(crate) fn pma_replay_provenance(&self) -> Option<PmaReplayProvenance> {
        let pma = PmaReplayProvenance {
            runtime_flavor: self.runtime_flavor.clone(),
            boot_source: self.boot_source.clone(),
            boot_event_num: self.boot_event_num,
            pma_work_dir_mode: self.pma_work_dir_mode.clone(),
            pma_fsync_mode: self.pma_fsync_mode.clone(),
        };
        (!pma.is_absent()).then_some(pma)
    }

    fn set_pma_replay_provenance(&mut self, pma: Option<PmaReplayProvenance>) {
        let pma = pma.unwrap_or_default();
        self.runtime_flavor = pma.runtime_flavor;
        self.boot_source = pma.boot_source;
        self.boot_event_num = pma.boot_event_num;
        self.pma_work_dir_mode = pma.pma_work_dir_mode;
        self.pma_fsync_mode = pma.pma_fsync_mode;
    }

    #[cfg(test)]
    pub(crate) fn with_pma_replay_provenance(mut self, pma: PmaReplayProvenance) -> Self {
        self.set_pma_replay_provenance(Some(pma));
        self
    }
}

pub fn build_provenance(
    resolved: &ResolvedCase,
    backend: BackendRuntimeFacts,
    docker_pma_proven: bool,
) -> Provenance {
    let mut provenance = Provenance {
        schema_version: PROVENANCE_SCHEMA_VERSION.to_string(),
        capture_timestamp_ms: unix_timestamp_ms(),
        host: capture_host_identity(),
        git: capture_git_identity(),
        backend,
        allow_debug_benchmark: resolved.requested.allow_debug_benchmark,
        allow_version_skew: resolved.requested.allow_version_skew,
        allow_degraded_cold: resolved.requested.allow_degraded_cold,
        cv_threshold: resolved.requested.cv_threshold,
        runtime_flavor: None,
        boot_source: None,
        boot_event_num: None,
        pma_work_dir_mode: None,
        pma_fsync_mode: None,
        binary: resolved.binary.clone(),
        fixture_path: resolved.absolute_fixture_path.clone(),
        fixture_sha256_hex: resolved.fixture_sha256_hex.clone(),
        fixture_manifest: resolved.fixture_manifest.clone(),
    };
    provenance.set_pma_replay_provenance(phase2_pma_provenance(
        resolved, &provenance.backend, docker_pma_proven,
    ));
    provenance
}

pub fn capture_native_provenance(resolved: &ResolvedCase) -> Provenance {
    build_provenance(resolved, BackendRuntimeFacts::Native, false)
}

pub fn capture_host_env() -> HostEnvSnapshot {
    HostEnvSnapshot {
        current_dir: std::env::current_dir().ok(),
        shell: std::env::var("SHELL").ok(),
        user: std::env::var("USER").ok(),
        hostname_env: std::env::var("HOSTNAME").ok(),
        rust_log: std::env::var("RUST_LOG").ok(),
    }
}

fn phase2_pma_provenance(
    resolved: &ResolvedCase,
    backend: &BackendRuntimeFacts,
    docker_pma_proven: bool,
) -> Option<PmaReplayProvenance> {
    let base = requested_read_pma_provenance(resolved).unwrap_or_else(|| {
        PmaReplayProvenance::checkpoint(resolved.fixture_manifest.checkpoint_event_num)
    });
    if matches!(backend, BackendRuntimeFacts::Native) {
        Some(base.with_fsync_mode(resolved.requested.fsync))
    } else if matches!(backend, BackendRuntimeFacts::Docker { .. }) && docker_pma_proven {
        Some(
            base.with_work_dir_mode(
                &resolved
                    .docker
                    .as_ref()
                    .expect("docker PMA provenance requires resolved Docker config")
                    .work_dir_mode,
            )
            .with_fsync_mode(resolved.requested.fsync),
        )
    } else {
        None
    }
}

fn requested_read_pma_provenance(resolved: &ResolvedCase) -> Option<PmaReplayProvenance> {
    let RequestedOrchestrate::GeneratedRead { boot, .. } = &resolved.requested.orchestrate else {
        return None;
    };
    match boot.clone().resolve().ok()? {
        ResolvedBootSource::Checkpoint {
            event_num: Some(event_num),
            ..
        } => Some(PmaReplayProvenance::checkpoint(event_num)),
        ResolvedBootSource::Checkpoint {
            event_num: None, ..
        } => None,
        ResolvedBootSource::Snapshot { event_num, .. } => {
            Some(PmaReplayProvenance::snapshot(event_num))
        }
    }
}

fn capture_host_identity() -> HostIdentity {
    HostIdentity {
        hostname: read_trimmed_file("/proc/sys/kernel/hostname")
            .or_else(|| std::env::var("HOSTNAME").ok()),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        kernel: read_trimmed_file("/proc/sys/kernel/osrelease"),
        cpu_count: std::thread::available_parallelism()
            .map(|parallelism| parallelism.get())
            .unwrap_or(1),
        total_memory_bytes: read_total_memory_bytes(),
        cpu_model: read_cpu_model(),
    }
}

fn capture_git_identity() -> Option<GitIdentity> {
    let commit = git_stdout(["rev-parse", "HEAD"]);
    let branch = git_stdout(["rev-parse", "--abbrev-ref", "HEAD"]);
    let commit_date = git_stdout(["log", "-1", "--format=%cI", "HEAD"]);
    let dirty = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()
        .ok()
        .map(|output| !String::from_utf8_lossy(&output.stdout).trim().is_empty())
        .unwrap_or(false);

    if commit.is_none() && branch.is_none() {
        None
    } else {
        Some(GitIdentity {
            commit,
            branch,
            commit_date,
            dirty,
        })
    }
}

fn git_stdout<const N: usize>(args: [&str; N]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn read_total_memory_bytes() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in meminfo.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            let kb = rest.split_whitespace().next()?.parse::<u64>().ok()?;
            return Some(kb.saturating_mul(1024));
        }
    }
    None
}

fn read_cpu_model() -> Option<String> {
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    for line in cpuinfo.lines() {
        if let Some(model) = line
            .split(':')
            .nth(1)
            .filter(|_| line.starts_with("model name"))
        {
            return Some(model.trim().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{build_provenance, BackendRuntimeFacts, PmaReplayProvenance};
    use crate::speed_of_light::fixture::SolFixtureManifest;
    use crate::speed_of_light::harness::case::{
        BinaryIdentity, DockerResolvedConfig, ExecutionConfig, ExecutionRequest, RequestedCase,
        RequestedOrchestrate, ResolvedCase, ResolvedOrchestrate, WorkDirMode,
    };
    use crate::speed_of_light::harness::docker_image::{
        DockerImageSource, DockerImageVariant, ResolvedDockerImage,
    };
    use crate::speed_of_light::harness::RESOLVED_CASE_SCHEMA_VERSION;
    use crate::speed_of_light::types::SolHeight;
    use crate::speed_of_light::{BootSourceInput, PeekMode};

    const REFERENCE_SNAPSHOT_DIR: &str =
        "/shared/nockchain/snapshots/first-100-v0-full-checkpoint-no-mempool";

    fn test_resolved_case() -> ResolvedCase {
        let requested = RequestedCase::native(PathBuf::from("fixture.soltest"));
        ResolvedCase {
            schema_version: RESOLVED_CASE_SCHEMA_VERSION.to_string(),
            benchmark: "sol-orchestrate".to_string(),
            orchestrate: ResolvedOrchestrate::for_requested(&requested),
            requested,
            absolute_fixture_path: PathBuf::from("/tmp/fixture.soltest"),
            fixture_sha256_hex: "fixture-sha".to_string(),
            fixture_manifest: SolFixtureManifest {
                source_archive_path: "archive.solarch".to_string(),
                source_archive_event_num: Some(12_000),
                checkpoint_kind: crate::speed_of_light::SolFixtureCheckpointKind::Derived,
                checkpoint_height: SolHeight(11_999),
                checkpoint_event_num: 12_000,
                archive_start_height: SolHeight(12_000),
                archive_end_height: SolHeight(12_099),
                include_mempool: false,
                chunk_size: 8,
                kernel_hash_hex: "kernel".to_string(),
                checkpoint_hash_hex: "checkpoint".to_string(),
                archive_hash_hex: "archive".to_string(),
            },
            execution_config: ExecutionConfig::default(),
            binary: BinaryIdentity {
                version: "0.1.0".to_string(),
                build_profile: "release".to_string(),
                git_commit: None,
            },
            docker: None,
        }
    }

    fn test_docker_resolved_case() -> ResolvedCase {
        let mut resolved = test_resolved_case();
        resolved.requested.execution = ExecutionRequest::Docker {
            image: DockerImageSource::AutoBuild {
                tag: "nockchain-bench:test".to_string(),
            },
            memory_limit: "8g".to_string(),
            cpuset: None,
            cpu_quota: None,
            cpu_period: None,
            work_dir_mode: WorkDirMode::DockerTmpfs,
            allow_version_skew: false,
        };
        resolved.docker = Some(DockerResolvedConfig {
            image: ResolvedDockerImage {
                source: DockerImageSource::AutoBuild {
                    tag: "nockchain-bench:test".to_string(),
                },
                variant: DockerImageVariant::Standard,
                requested_ref: "nockchain-bench:test".to_string(),
                resolved_ref: "sha256:test".to_string(),
                immutable_identity: "sha256:test".to_string(),
                image_id: "sha256:test".to_string(),
            },
            requested_memory_limit_bytes: 8 * 1024 * 1024 * 1024,
            cpuset: None,
            cpu_quota: None,
            cpu_period: None,
            work_dir_mode: WorkDirMode::DockerTmpfs,
            allow_version_skew: false,
        });
        resolved
    }

    fn test_snapshot_read_resolved_case() -> ResolvedCase {
        let dir = PathBuf::from(REFERENCE_SNAPSHOT_DIR);
        let mut resolved = test_resolved_case();
        resolved.requested.orchestrate = RequestedOrchestrate::GeneratedRead {
            boot: BootSourceInput::Snapshot {
                pma: dir.join("snapshot.pma"),
                manifest: dir.join("snapshot.manifest"),
            },
            kernel_path: dir.join("kernel.jam"),
            start_height: 0,
            end_height: None,
            count: Some(1),
            peek_mode: PeekMode::Warm,
        };
        resolved.orchestrate = ResolvedOrchestrate::for_requested(&resolved.requested);
        resolved
    }

    fn test_docker_backend() -> BackendRuntimeFacts {
        BackendRuntimeFacts::Docker {
            host_binary: BinaryIdentity {
                version: "0.1.0".to_string(),
                build_profile: "release".to_string(),
                git_commit: Some("host".to_string()),
            },
            container_binary: BinaryIdentity {
                version: "0.1.0".to_string(),
                build_profile: "release".to_string(),
                git_commit: Some("host".to_string()),
            },
            image_source: DockerImageSource::AutoBuild {
                tag: "nockchain-bench:test".to_string(),
            },
            requested_image_ref: "nockchain-bench:test".to_string(),
            resolved_image_ref: "sha256:test".to_string(),
            image_digest: "sha256:test".to_string(),
            container_id: "container-id".to_string(),
            docker_engine_version: "29.1.3".to_string(),
            docker_context: "desktop-linux".to_string(),
            cgroup_version: "2".to_string(),
            storage_driver: "overlayfs".to_string(),
            realized_memory_max: 8 * 1024 * 1024 * 1024,
            realized_memory_current: 512,
            realized_cpuset: Some("0-3".to_string()),
            realized_cpu_max: Some("max 100000".to_string()),
        }
    }

    #[test]
    fn build_provenance_populates_pma_replay_fields() {
        let resolved = test_resolved_case();
        let provenance = build_provenance(&resolved, BackendRuntimeFacts::Native, false);
        assert_eq!(provenance.backend, BackendRuntimeFacts::Native);
        assert_eq!(provenance.runtime_flavor.as_deref(), Some("pma"));
        assert_eq!(provenance.boot_source.as_deref(), Some("checkpoint"));
        assert_eq!(
            provenance.boot_event_num,
            Some(resolved.fixture_manifest.checkpoint_event_num)
        );
        assert_eq!(provenance.pma_work_dir_mode, None);
        assert_eq!(provenance.pma_fsync_mode.as_deref(), Some("on"));
        assert_eq!(
            provenance.pma_replay_provenance(),
            Some(
                PmaReplayProvenance::checkpoint(resolved.fixture_manifest.checkpoint_event_num,)
                    .with_fsync_mode(resolved.requested.fsync)
            )
        );

        let json = serde_json::to_value(&provenance).expect("serialize provenance");
        assert_eq!(json.get("backend"), Some(&serde_json::json!("Native")));
        assert_eq!(json.get("runtime_flavor"), Some(&serde_json::json!("pma")));
        assert_eq!(
            json.get("boot_source"),
            Some(&serde_json::json!("checkpoint"))
        );
        assert_eq!(
            json.get("boot_event_num"),
            Some(&serde_json::json!(
                resolved.fixture_manifest.checkpoint_event_num
            ))
        );
        assert_eq!(json.get("pma_fsync_mode"), Some(&serde_json::json!("on")));
        assert!(json.get("pma_work_dir_mode").is_none());
    }

    #[test]
    fn build_provenance_populates_pma_replay_fields_for_docker() {
        let resolved = test_docker_resolved_case();
        let provenance = build_provenance(&resolved, test_docker_backend(), true);

        assert_eq!(provenance.runtime_flavor.as_deref(), Some("pma"));
        assert_eq!(provenance.boot_source.as_deref(), Some("checkpoint"));
        assert_eq!(
            provenance.boot_event_num,
            Some(resolved.fixture_manifest.checkpoint_event_num)
        );
        assert_eq!(
            provenance.pma_work_dir_mode.as_deref(),
            Some("docker_tmpfs")
        );
        assert_eq!(provenance.pma_fsync_mode.as_deref(), Some("on"));
        assert_eq!(
            provenance.pma_replay_provenance(),
            Some(
                PmaReplayProvenance::checkpoint(resolved.fixture_manifest.checkpoint_event_num,)
                    .with_work_dir_mode(&WorkDirMode::DockerTmpfs)
                    .with_fsync_mode(resolved.requested.fsync)
            )
        );

        let json = serde_json::to_value(&provenance).expect("serialize provenance");
        assert_eq!(json.get("runtime_flavor"), Some(&serde_json::json!("pma")));
        assert_eq!(
            json.get("boot_source"),
            Some(&serde_json::json!("checkpoint"))
        );
        assert_eq!(
            json.get("boot_event_num"),
            Some(&serde_json::json!(
                resolved.fixture_manifest.checkpoint_event_num
            ))
        );
        assert_eq!(
            json.get("pma_work_dir_mode"),
            Some(&serde_json::json!("docker_tmpfs"))
        );
        assert_eq!(json.get("pma_fsync_mode"), Some(&serde_json::json!("on")));
    }

    #[test]
    fn build_provenance_uses_snapshot_read_boot_source() {
        let resolved = test_snapshot_read_resolved_case();
        let provenance = build_provenance(&resolved, BackendRuntimeFacts::Native, false);

        assert_eq!(provenance.runtime_flavor.as_deref(), Some("pma"));
        assert_eq!(provenance.boot_source.as_deref(), Some("snapshot"));
        assert_eq!(provenance.boot_event_num, Some(5));
        assert_eq!(provenance.pma_fsync_mode.as_deref(), Some("on"));
    }
}
