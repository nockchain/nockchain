use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::docker::parse_memory_limit;
use super::docker_image::{
    resolve_requested_image_ref, DockerImageSource, DockerImageVariant, ResolvedDockerImage,
};
use super::{
    is_release_build, HarnessError, REQUESTED_CASE_SCHEMA_VERSION, RESOLVED_CASE_SCHEMA_VERSION,
};
use crate::speed_of_light::fixture::{read_fixture_file, SolFixtureManifest};
use crate::speed_of_light::{
    BootSourceInput, InputRole, PeekMode, ReadRangeResolution, ResolvedInput,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkDirMode {
    HostBind,
    DockerVolume,
    DockerTmpfs,
}

impl WorkDirMode {
    pub fn provenance_label(&self) -> &'static str {
        match self {
            Self::HostBind => "host_bind",
            Self::DockerVolume => "docker_volume",
            Self::DockerTmpfs => "docker_tmpfs",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionRequest {
    Native,
    Docker {
        image: DockerImageSource,
        memory_limit: String,
        cpuset: Option<String>,
        cpu_quota: Option<i64>,
        cpu_period: Option<i64>,
        work_dir_mode: WorkDirMode,
        allow_version_skew: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DockerResolvedConfig {
    pub image: ResolvedDockerImage,
    pub requested_memory_limit_bytes: u64,
    pub cpuset: Option<String>,
    pub cpu_quota: Option<i64>,
    pub cpu_period: Option<i64>,
    pub work_dir_mode: WorkDirMode,
    pub allow_version_skew: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestedCase {
    #[serde(default = "requested_case_schema_version")]
    pub schema_version: String,
    #[serde(default = "default_benchmark")]
    pub benchmark: String,
    pub label: Option<String>,
    #[serde(default = "default_requested_orchestrate")]
    pub orchestrate: RequestedOrchestrate,
    #[serde(default, skip_serializing)]
    pub fixture_path: PathBuf,
    #[serde(default, skip_serializing)]
    pub blocks: u64,
    #[serde(default, skip_serializing)]
    pub skip_genesis: bool,
    pub profile_memory: bool,
    pub profile_interval_ms: u64,
    #[serde(default = "default_fsync_enabled")]
    #[serde(
        serialize_with = "serialize_fsync_bool",
        deserialize_with = "deserialize_fsync_bool"
    )]
    pub fsync: bool,
    pub execution: ExecutionRequest,
    pub threads: u32,
    pub warmup_runs: u32,
    pub measured_runs: u32,
    pub cooldown_secs: u64,
    #[serde(default)]
    pub allow_debug_benchmark: bool,
    #[serde(default)]
    pub allow_version_skew: bool,
    #[serde(default)]
    pub allow_degraded_cold: bool,
    #[serde(default)]
    pub cv_threshold: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum RequestedOrchestrate {
    PlanFile {
        plan_path: PathBuf,
    },
    GeneratedReplay {
        fixture_path: PathBuf,
        blocks: Option<u64>,
        skip_genesis: bool,
    },
    GeneratedRead {
        boot: BootSourceInput,
        kernel_path: PathBuf,
        start_height: u64,
        end_height: Option<u64>,
        count: Option<u64>,
        peek_mode: PeekMode,
    },
}

pub const DEFAULT_FSYNC_ENABLED: bool = true;

pub const fn default_fsync_enabled() -> bool {
    DEFAULT_FSYNC_ENABLED
}

pub const fn fsync_mode_label(enabled: bool) -> &'static str {
    if enabled {
        "on"
    } else {
        "off"
    }
}

fn serialize_fsync_bool<S>(value: &bool, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(fsync_mode_label(*value))
}

fn deserialize_fsync_bool<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    match value.as_str() {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(serde::de::Error::custom("fsync must be \"on\" or \"off\"")),
    }
}

impl RequestedCase {
    pub fn native(fixture_path: PathBuf) -> Self {
        Self {
            schema_version: REQUESTED_CASE_SCHEMA_VERSION.to_string(),
            benchmark: "sol-orchestrate".to_string(),
            label: None,
            orchestrate: RequestedOrchestrate::GeneratedReplay {
                fixture_path: fixture_path.clone(),
                blocks: Some(0),
                skip_genesis: false,
            },
            fixture_path,
            blocks: 0,
            skip_genesis: false,
            profile_memory: false,
            profile_interval_ms: 500,
            fsync: default_fsync_enabled(),
            execution: ExecutionRequest::Native,
            threads: 1,
            warmup_runs: 1,
            measured_runs: 5,
            cooldown_secs: 10,
            allow_debug_benchmark: false,
            allow_version_skew: false,
            allow_degraded_cold: false,
            cv_threshold: None,
        }
    }

    pub fn fsync_enabled(&self) -> bool {
        self.fsync
    }

    pub fn set_fsync_enabled(&mut self, enabled: bool) {
        self.fsync = enabled;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryIdentity {
    pub version: String,
    pub build_profile: String,
    pub git_commit: Option<String>,
}

fn compiled_build_profile() -> &'static str {
    option_env!("NOCKCHAIN_BENCH_BUILD_PROFILE")
        .filter(|profile| !profile.trim().is_empty())
        .unwrap_or_else(|| {
            if is_release_build() {
                "release"
            } else {
                "debug"
            }
        })
}

pub fn current_binary_identity() -> BinaryIdentity {
    BinaryIdentity {
        version: env!("CARGO_PKG_VERSION").to_string(),
        build_profile: compiled_build_profile().to_string(),
        git_commit: option_env!("NOCKCHAIN_BENCH_GIT_COMMIT")
            .map(str::trim)
            .filter(|commit| !commit.is_empty())
            .map(str::to_string),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionConfig {
    pub gc_drop_threshold_mib: u64,
    pub page_fault_minor_burst_threshold: u64,
    pub page_fault_major_burst_threshold: u64,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            gc_drop_threshold_mib: 64,
            page_fault_minor_burst_threshold: 50_000,
            page_fault_major_burst_threshold: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedCase {
    #[serde(default = "resolved_case_schema_version")]
    pub schema_version: String,
    pub requested: RequestedCase,
    #[serde(default = "default_benchmark")]
    pub benchmark: String,
    #[serde(default = "default_resolved_orchestrate")]
    pub orchestrate: ResolvedOrchestrate,
    #[serde(default, skip_serializing)]
    pub absolute_fixture_path: PathBuf,
    #[serde(default, skip_serializing)]
    pub fixture_sha256_hex: String,
    #[serde(default = "default_fixture_manifest", skip_serializing)]
    pub fixture_manifest: SolFixtureManifest,
    pub execution_config: ExecutionConfig,
    pub binary: BinaryIdentity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docker: Option<DockerResolvedConfig>,
}

fn requested_case_schema_version() -> String {
    REQUESTED_CASE_SCHEMA_VERSION.to_string()
}

fn resolved_case_schema_version() -> String {
    RESOLVED_CASE_SCHEMA_VERSION.to_string()
}

fn default_benchmark() -> String {
    "sol-orchestrate".to_string()
}

fn default_requested_orchestrate() -> RequestedOrchestrate {
    RequestedOrchestrate::GeneratedReplay {
        fixture_path: PathBuf::new(),
        blocks: Some(0),
        skip_genesis: false,
    }
}

fn default_resolved_orchestrate() -> ResolvedOrchestrate {
    ResolvedOrchestrate {
        source_kind: "generated_replay".to_string(),
        source_plan_path: None,
        source_plan_sha256_hex: None,
        normalized_plan_sha256_hex: None,
        trusted_plan_relative_path: PathBuf::from("trusted_plan.json"),
        inputs: Vec::new(),
        step_count: 0,
        step_signature_sha256_hex: None,
        read_range_resolution: None,
        contains_cold_steps: false,
    }
}

fn default_fixture_manifest() -> SolFixtureManifest {
    SolFixtureManifest {
        source_archive_path: String::new(),
        source_archive_event_num: None,
        checkpoint_kind: crate::speed_of_light::SolFixtureCheckpointKind::Derived,
        checkpoint_height: crate::speed_of_light::SolHeight::ZERO,
        checkpoint_event_num: 0,
        archive_start_height: crate::speed_of_light::SolHeight::ZERO,
        archive_end_height: crate::speed_of_light::SolHeight::ZERO,
        include_mempool: false,
        chunk_size: 0,
        kernel_hash_hex: String::new(),
        checkpoint_hash_hex: String::new(),
        archive_hash_hex: String::new(),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedOrchestrate {
    pub source_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_plan_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_plan_sha256_hex: Option<String>,
    pub normalized_plan_sha256_hex: Option<String>,
    pub trusted_plan_relative_path: PathBuf,
    pub inputs: Vec<ResolvedInput>,
    pub step_count: usize,
    pub step_signature_sha256_hex: Option<String>,
    pub read_range_resolution: Option<ReadRangeResolution>,
    pub contains_cold_steps: bool,
}

pub fn resolve_requested_case(requested: &RequestedCase) -> Result<ResolvedCase, HarnessError> {
    validate_requested_case(requested)?;

    let (absolute_fixture_path, fixture_sha256_hex, fixture_manifest) = match &requested.orchestrate
    {
        RequestedOrchestrate::GeneratedReplay { fixture_path, .. } => {
            let absolute_fixture_path = canonicalize_path(fixture_path)?;
            let fixture = read_fixture_file(&absolute_fixture_path)?;
            let fixture_sha256_hex = sha256_hex_for_file(&absolute_fixture_path)?;
            (absolute_fixture_path, fixture_sha256_hex, fixture.manifest)
        }
        RequestedOrchestrate::PlanFile { .. } | RequestedOrchestrate::GeneratedRead { .. } => {
            (PathBuf::new(), String::new(), default_fixture_manifest())
        }
    };

    Ok(ResolvedCase {
        schema_version: RESOLVED_CASE_SCHEMA_VERSION.to_string(),
        requested: requested.clone(),
        benchmark: "sol-orchestrate".to_string(),
        orchestrate: placeholder_resolved_orchestrate(requested),
        absolute_fixture_path,
        fixture_sha256_hex,
        fixture_manifest,
        execution_config: ExecutionConfig::default(),
        binary: current_binary_identity(),
        docker: resolve_docker_execution(&requested.execution)?,
    })
}

impl ResolvedOrchestrate {
    pub fn for_requested(requested: &RequestedCase) -> Self {
        placeholder_resolved_orchestrate(requested)
    }
}

fn placeholder_resolved_orchestrate(requested: &RequestedCase) -> ResolvedOrchestrate {
    let source_kind = match requested.orchestrate {
        RequestedOrchestrate::PlanFile { .. } => "plan_file",
        RequestedOrchestrate::GeneratedReplay { .. } => "generated_replay",
        RequestedOrchestrate::GeneratedRead { .. } => "generated_read",
    }
    .to_string();

    let mut inputs = Vec::new();
    match &requested.orchestrate {
        RequestedOrchestrate::GeneratedReplay { .. } | RequestedOrchestrate::PlanFile { .. } => {}
        RequestedOrchestrate::GeneratedRead {
            boot, kernel_path, ..
        } => {
            match boot {
                BootSourceInput::Checkpoint { checkpoint } => {
                    inputs.push(ResolvedInput {
                        input_id: "checkpoint-0".to_string(),
                        role: InputRole::Checkpoint,
                        absolute_path: checkpoint.clone(),
                        sha256_hex: String::new(),
                        size_bytes: 0,
                        container_path: None,
                    });
                }
                BootSourceInput::Snapshot { pma, manifest } => {
                    inputs.push(ResolvedInput {
                        input_id: "snapshot-pma-0".to_string(),
                        role: InputRole::SnapshotPma,
                        absolute_path: pma.clone(),
                        sha256_hex: String::new(),
                        size_bytes: 0,
                        container_path: None,
                    });
                    inputs.push(ResolvedInput {
                        input_id: "snapshot-manifest-0".to_string(),
                        role: InputRole::SnapshotManifest,
                        absolute_path: manifest.clone(),
                        sha256_hex: String::new(),
                        size_bytes: 0,
                        container_path: None,
                    });
                }
            }
            inputs.push(ResolvedInput {
                input_id: "kernel-0".to_string(),
                role: InputRole::Kernel,
                absolute_path: kernel_path.clone(),
                sha256_hex: String::new(),
                size_bytes: 0,
                container_path: None,
            });
        }
    }

    ResolvedOrchestrate {
        source_kind,
        source_plan_path: None,
        source_plan_sha256_hex: None,
        normalized_plan_sha256_hex: None,
        trusted_plan_relative_path: PathBuf::from("trusted_plan.json"),
        inputs,
        step_count: 0,
        step_signature_sha256_hex: None,
        read_range_resolution: None,
        contains_cold_steps: false,
    }
}

fn validate_requested_case(requested: &RequestedCase) -> Result<(), HarnessError> {
    if requested.measured_runs < 3 {
        return Err(HarnessError::InvalidRequestedCase(
            "trusted runs require at least 3 measured runs".to_string(),
        ));
    }

    if requested.schema_version != REQUESTED_CASE_SCHEMA_VERSION {
        return Err(HarnessError::InvalidRequestedCase(format!(
            "requested case schema_version must be {REQUESTED_CASE_SCHEMA_VERSION}"
        )));
    }

    if requested.benchmark != "sol-orchestrate" {
        return Err(HarnessError::InvalidRequestedCase(
            "trusted SOL benchmark kind must be sol-orchestrate".to_string(),
        ));
    }

    if requested.threads == 0 {
        return Err(HarnessError::InvalidRequestedCase(
            "--threads must be at least 1".to_string(),
        ));
    }

    if requested
        .cv_threshold
        .is_some_and(|threshold| !threshold.is_finite() || threshold < 0.0)
    {
        return Err(HarnessError::InvalidRequestedCase(
            "--cv-threshold must be a finite non-negative value".to_string(),
        ));
    }

    validate_execution_request(&requested.execution)?;

    Ok(())
}

fn validate_execution_request(execution: &ExecutionRequest) -> Result<(), HarnessError> {
    let ExecutionRequest::Docker {
        image,
        memory_limit,
        cpuset,
        cpu_quota,
        cpu_period,
        ..
    } = execution
    else {
        return Ok(());
    };

    match image {
        DockerImageSource::Provided { reference } if reference.trim().is_empty() => {
            return Err(HarnessError::InvalidRequestedCase(
                "Docker execution requires a non-empty provided image ref".to_string(),
            ));
        }
        DockerImageSource::AutoBuild { tag } if tag.trim().is_empty() => {
            return Err(HarnessError::InvalidRequestedCase(
                "Docker execution requires a non-empty auto-build image tag".to_string(),
            ));
        }
        _ => {}
    }

    if parse_memory_limit(memory_limit) <= 0 {
        return Err(HarnessError::InvalidRequestedCase(
            "Docker execution requires a positive memory limit".to_string(),
        ));
    }

    if cpuset
        .as_ref()
        .is_some_and(|cpuset| cpuset.trim().is_empty())
    {
        return Err(HarnessError::InvalidRequestedCase(
            "Docker execution requires a non-empty cpuset when provided".to_string(),
        ));
    }

    if cpu_quota.is_some_and(|value| value <= 0) {
        return Err(HarnessError::InvalidRequestedCase(
            "Docker execution requires a positive cpu_quota when provided".to_string(),
        ));
    }

    if cpu_period.is_some_and(|value| value <= 0) {
        return Err(HarnessError::InvalidRequestedCase(
            "Docker execution requires a positive cpu_period when provided".to_string(),
        ));
    }

    Ok(())
}

fn resolve_docker_execution(
    execution: &ExecutionRequest,
) -> Result<Option<DockerResolvedConfig>, HarnessError> {
    let ExecutionRequest::Docker {
        image,
        memory_limit,
        cpuset,
        cpu_quota,
        cpu_period,
        work_dir_mode,
        allow_version_skew,
    } = execution
    else {
        return Ok(None);
    };
    let requested_ref = resolve_requested_image_ref(image, DockerImageVariant::Standard)?;

    Ok(Some(DockerResolvedConfig {
        image: ResolvedDockerImage {
            source: image.clone(),
            variant: DockerImageVariant::Standard,
            requested_ref,
            resolved_ref: String::new(),
            immutable_identity: String::new(),
            image_id: String::new(),
        },
        requested_memory_limit_bytes: parse_memory_limit(memory_limit) as u64,
        cpuset: cpuset.clone(),
        cpu_quota: *cpu_quota,
        cpu_period: *cpu_period,
        work_dir_mode: work_dir_mode.clone(),
        allow_version_skew: *allow_version_skew,
    }))
}

fn canonicalize_path(path: &Path) -> Result<PathBuf, HarnessError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn sha256_hex_for_file(path: &Path) -> Result<String, HarnessError> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;
    use tempfile::tempdir;

    use super::{
        compiled_build_profile, current_binary_identity, resolve_requested_case, DockerImageSource,
        ExecutionRequest, RequestedCase, RequestedOrchestrate, ResolvedCase, ResolvedOrchestrate,
        WorkDirMode,
    };
    use crate::speed_of_light::fixture::{write_fixture_file, SolFixtureFile, SolFixtureManifest};
    use crate::speed_of_light::types::SolHeight;
    use crate::speed_of_light::{BootSourceInput, InputRole};

    #[test]
    fn docker_requested_case_resolves_docker_execution() {
        let tempdir = tempdir().expect("tempdir");
        let fixture_path = tempdir.path().join("fixture.soltest");
        write_fixture_file(
            &fixture_path,
            &SolFixtureFile {
                manifest: SolFixtureManifest {
                    source_archive_path: "archive.solarch".to_string(),
                    source_archive_event_num: Some(1),
                    checkpoint_kind: crate::speed_of_light::SolFixtureCheckpointKind::Derived,
                    checkpoint_height: SolHeight(1),
                    checkpoint_event_num: 1,
                    archive_start_height: SolHeight(2),
                    archive_end_height: SolHeight(3),
                    include_mempool: false,
                    chunk_size: 8,
                    kernel_hash_hex: "kernel".to_string(),
                    checkpoint_hash_hex: "checkpoint".to_string(),
                    archive_hash_hex: "archive".to_string(),
                },
                checkpoint_bytes: vec![1, 2, 3],
                archive_bytes: vec![4, 5, 6],
                kernel_bytes: vec![7, 8, 9],
            },
        )
        .expect("write fixture");

        let requested = RequestedCase {
            execution: ExecutionRequest::Docker {
                image: DockerImageSource::AutoBuild {
                    tag: "nockchain-bench:test".to_string(),
                },
                memory_limit: "2g".to_string(),
                cpuset: Some("0-3".to_string()),
                cpu_quota: Some(200_000),
                cpu_period: Some(100_000),
                work_dir_mode: WorkDirMode::DockerVolume,
                allow_version_skew: true,
            },
            measured_runs: 3,
            cooldown_secs: 0,
            warmup_runs: 1,
            fixture_path: PathBuf::from(&fixture_path),
            ..RequestedCase::native(PathBuf::from(&fixture_path))
        };

        let resolved = resolve_requested_case(&requested).expect("resolve requested case");

        let docker = resolved.docker.expect("docker execution details");
        assert_eq!(
            docker.image.source,
            DockerImageSource::AutoBuild {
                tag: "nockchain-bench:test".to_string()
            }
        );
        assert_eq!(docker.image.requested_ref, "nockchain-bench:test");
        assert_eq!(docker.requested_memory_limit_bytes, 2 * 1024 * 1024 * 1024);
        assert_eq!(docker.work_dir_mode, WorkDirMode::DockerVolume);
        assert!(docker.allow_version_skew);
    }

    #[test]
    fn generated_read_requested_case_records_snapshot_boot_inputs() {
        let requested = RequestedCase {
            orchestrate: RequestedOrchestrate::GeneratedRead {
                boot: BootSourceInput::Snapshot {
                    pma: PathBuf::from("/tmp/snapshot.pma"),
                    manifest: PathBuf::from("/tmp/snapshot.manifest"),
                },
                kernel_path: PathBuf::from("/tmp/kernel.jam"),
                start_height: 0,
                end_height: None,
                count: Some(1),
                peek_mode: crate::speed_of_light::PeekMode::Warm,
            },
            ..RequestedCase::native(PathBuf::from("/tmp/fixture.soltest"))
        };

        let resolved = ResolvedOrchestrate::for_requested(&requested);
        let inputs: Vec<_> = resolved
            .inputs
            .iter()
            .map(|input| (input.input_id.as_str(), input.role))
            .collect();

        assert_eq!(
            inputs,
            vec![
                ("snapshot-pma-0", InputRole::SnapshotPma),
                ("snapshot-manifest-0", InputRole::SnapshotManifest),
                ("kernel-0", InputRole::Kernel),
            ]
        );
    }

    #[test]
    fn docker_requested_case_deserializes_structured_image_source() {
        let requested = serde_json::from_value::<RequestedCase>(json!({
            "benchmark": "sol-replay",
            "label": null,
            "fixture_path": "fixture.soltest",
            "blocks": 0,
            "skip_genesis": false,
            "profile_memory": false,
            "profile_interval_ms": 500,
            "execution": {
                "Docker": {
                    "image": {
                        "provided": {
                            "ref": "ghcr.io/org/nockchain-bench@sha256:abc"
                        }
                    },
                    "memory_limit": "8g",
                    "cpuset": null,
                    "cpu_quota": null,
                    "cpu_period": null,
                    "work_dir_mode": "DockerTmpfs",
                    "allow_version_skew": false
                }
            },
            "threads": 1,
            "warmup_runs": 0,
            "measured_runs": 3,
            "cooldown_secs": 0
        }))
        .expect("deserialize requested case");

        let execution = serde_json::to_value(&requested)
            .expect("serialize requested case")
            .get("execution")
            .cloned()
            .expect("execution field");
        assert_eq!(
            execution,
            json!({
                "Docker": {
                    "image": {
                        "provided": {
                            "ref": "ghcr.io/org/nockchain-bench@sha256:abc"
                        }
                    },
                    "memory_limit": "8g",
                    "cpuset": null,
                    "cpu_quota": null,
                    "cpu_period": null,
                    "work_dir_mode": "DockerTmpfs",
                    "allow_version_skew": false
                }
            })
        );
    }

    #[test]
    fn requested_case_defaults_fsync_on_when_field_is_missing() {
        let requested = serde_json::from_value::<RequestedCase>(json!({
            "benchmark": "sol-replay",
            "label": null,
            "fixture_path": "fixture.soltest",
            "blocks": 0,
            "skip_genesis": false,
            "profile_memory": false,
            "profile_interval_ms": 500,
            "execution": "Native",
            "threads": 1,
            "warmup_runs": 0,
            "measured_runs": 3,
            "cooldown_secs": 0
        }))
        .expect("deserialize requested case");

        assert!(requested.fsync);
    }

    #[test]
    fn docker_requested_case_deserializes_frozen_resolved_image() {
        let resolved = serde_json::from_value::<ResolvedCase>(json!({
            "schema_version": "1",
            "requested": {
                "benchmark": "sol-replay",
                "label": null,
                "fixture_path": "fixture.soltest",
                "blocks": 0,
                "skip_genesis": false,
                "profile_memory": false,
                "profile_interval_ms": 500,
                "execution": {
                    "Docker": {
                        "image": {
                            "auto_build": {
                                "tag": "nockchain-bench:local"
                            }
                        },
                        "memory_limit": "8g",
                        "cpuset": null,
                        "cpu_quota": null,
                        "cpu_period": null,
                        "work_dir_mode": "DockerTmpfs",
                        "allow_version_skew": false
                    }
                },
                "threads": 1,
                "warmup_runs": 0,
                "measured_runs": 3,
                "cooldown_secs": 0
            },
            "absolute_fixture_path": "/tmp/fixture.soltest",
            "fixture_sha256_hex": "abc123",
            "fixture_manifest": {
                "source_archive_path": "archive.solarch",
                "source_archive_event_num": 1,
                "checkpoint_kind": "derived",
                "checkpoint_height": 1,
                "checkpoint_event_num": 1,
                "archive_start_height": 2,
                "archive_end_height": 3,
                "include_mempool": false,
                "chunk_size": 8,
                "kernel_hash_hex": "kernel",
                "checkpoint_hash_hex": "checkpoint",
                "archive_hash_hex": "archive"
            },
            "execution_config": {
                "gc_drop_threshold_mib": 64,
                "page_fault_minor_burst_threshold": 50000,
                "page_fault_major_burst_threshold": 1
            },
            "binary": {
                "version": "0.1.0",
                "build_profile": "release",
                "git_commit": null
            },
            "docker": {
                "image": {
                    "source": {
                        "auto_build": {
                            "tag": "nockchain-bench:local"
                        }
                    },
                    "variant": "Standard",
                    "requested_ref": "nockchain-bench:local",
                    "resolved_ref": "sha256:deadbeef",
                    "immutable_identity": "sha256:deadbeef",
                    "image_id": "sha256:deadbeef"
                },
                "requested_memory_limit_bytes": 8589934592u64,
                "cpuset": null,
                "cpu_quota": null,
                "cpu_period": null,
                "work_dir_mode": "DockerTmpfs",
                "allow_version_skew": false
            }
        }))
        .expect("deserialize resolved case");

        let docker = serde_json::to_value(&resolved)
            .expect("serialize resolved case")
            .get("docker")
            .cloned()
            .expect("docker config");
        assert_eq!(
            docker.get("image"),
            Some(&json!({
                "source": {
                    "auto_build": {
                        "tag": "nockchain-bench:local"
                    }
                },
                "variant": "Standard",
                "requested_ref": "nockchain-bench:local",
                "resolved_ref": "sha256:deadbeef",
                "immutable_identity": "sha256:deadbeef",
                "image_id": "sha256:deadbeef"
            }))
        );
    }

    #[test]
    fn current_binary_identity_uses_compiled_git_commit() {
        let identity = current_binary_identity();
        assert_eq!(identity.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(identity.build_profile, compiled_build_profile());
    }

    #[test]
    fn docker_requested_case_rejects_invalid_docker_memory_limit() {
        let requested = RequestedCase {
            execution: ExecutionRequest::Docker {
                image: DockerImageSource::AutoBuild {
                    tag: "nockchain-bench:test".to_string(),
                },
                memory_limit: "0".to_string(),
                cpuset: None,
                cpu_quota: None,
                cpu_period: None,
                work_dir_mode: WorkDirMode::HostBind,
                allow_version_skew: false,
            },
            measured_runs: 3,
            cooldown_secs: 0,
            ..RequestedCase::native(PathBuf::from("fixture.soltest"))
        };

        let error = resolve_requested_case(&requested).expect_err("invalid memory limit");
        assert!(error.to_string().contains("memory limit"));
    }
}
