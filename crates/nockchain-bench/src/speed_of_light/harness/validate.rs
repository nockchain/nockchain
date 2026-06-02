use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::artifacts::write_validation;
use super::case::WorkDirMode;
use super::{current_cgroup_v2_path, parse_cgroup_numeric, HarnessError};

pub const VALIDATION_PROBE_VERSION: &str = "phase4-v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValidationStatus {
    Valid,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationCacheKey {
    pub docker_engine_version: String,
    pub cgroup_version: String,
    pub image_digest: String,
    pub memory_limit: String,
    pub cpuset: Option<String>,
    pub cpu_quota: Option<i64>,
    pub cpu_period: Option<i64>,
    pub work_dir_mode: WorkDirMode,
    pub probe_version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidationRecord {
    pub key: ValidationCacheKey,
    pub status: ValidationStatus,
    pub from_cache: bool,
    pub observed_probe_version: Option<String>,
    pub probe_version_matches: Option<bool>,
    pub container_started: bool,
    pub docker_reports_cgroup_v2: bool,
    pub memory_max_readable: bool,
    pub memory_current_readable: bool,
    pub memory_limit_matches: bool,
    pub allocation_sanity: bool,
    pub realized_memory_max_bytes: Option<u64>,
    pub allocation_request_bytes: Option<u64>,
    pub memory_current_before_bytes: Option<u64>,
    pub memory_current_peak_bytes: Option<u64>,
    pub memory_current_after_bytes: Option<u64>,
    pub recorded_cpu_max: Option<String>,
    pub recorded_cpuset: Option<String>,
    pub failure_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationProbeResult {
    pub probe_version: String,
    pub memory_max_readable: bool,
    pub memory_current_readable: bool,
    pub realized_memory_max_bytes: Option<u64>,
    pub memory_current_before_bytes: Option<u64>,
    pub memory_current_peak_bytes: Option<u64>,
    pub memory_current_after_bytes: Option<u64>,
    pub allocation_request_bytes: Option<u64>,
    pub recorded_cpu_max: Option<String>,
    pub recorded_cpuset: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BackendValidationOutcome {
    pma_replay_proven: bool,
}

impl BackendValidationOutcome {
    pub fn new(pma_replay_proven: bool) -> Self {
        Self { pma_replay_proven }
    }

    pub fn pma_replay_proven(&self) -> bool {
        self.pma_replay_proven
    }

    pub fn from_validation_record(record: &ValidationRecord) -> Self {
        Self {
            pma_replay_proven: record.status == ValidationStatus::Valid,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ValidationCacheFile {
    pub entries: Vec<ValidationRecord>,
}

impl ValidationRecord {
    pub fn cache_hit(record: &ValidationRecord) -> Self {
        let mut cached = record.clone();
        cached.from_cache = true;
        cached
    }
}

pub fn evaluate_validation_probe(
    key: ValidationCacheKey,
    container_started: bool,
    requested_memory_limit_bytes: u64,
    probe: &ValidationProbeResult,
) -> ValidationRecord {
    let docker_reports_cgroup_v2 = key.cgroup_version == "2";
    let probe_version_matches = probe.probe_version == key.probe_version;
    let memory_limit_matches =
        probe.realized_memory_max_bytes == Some(requested_memory_limit_bytes);
    let allocation_sanity = probe
        .allocation_request_bytes
        .zip(probe.memory_current_before_bytes)
        .zip(probe.memory_current_peak_bytes)
        .is_some_and(|((allocation_request_bytes, before), peak)| {
            allocation_within_tolerance(before, peak, allocation_request_bytes)
        });

    let status = if container_started
        && docker_reports_cgroup_v2
        && probe_version_matches
        && probe.memory_max_readable
        && probe.memory_current_readable
        && memory_limit_matches
        && allocation_sanity
    {
        ValidationStatus::Valid
    } else {
        ValidationStatus::Invalid
    };

    let failure_reason = (status == ValidationStatus::Invalid).then(|| {
        build_failure_reason(
            container_started,
            docker_reports_cgroup_v2,
            Some(probe_version_matches),
            probe.memory_max_readable,
            probe.memory_current_readable,
            memory_limit_matches,
            allocation_sanity,
        )
    });

    ValidationRecord {
        key,
        status,
        from_cache: false,
        observed_probe_version: Some(probe.probe_version.clone()),
        probe_version_matches: Some(probe_version_matches),
        container_started,
        docker_reports_cgroup_v2,
        memory_max_readable: probe.memory_max_readable,
        memory_current_readable: probe.memory_current_readable,
        memory_limit_matches,
        allocation_sanity,
        realized_memory_max_bytes: probe.realized_memory_max_bytes,
        allocation_request_bytes: probe.allocation_request_bytes,
        memory_current_before_bytes: probe.memory_current_before_bytes,
        memory_current_peak_bytes: probe.memory_current_peak_bytes,
        memory_current_after_bytes: probe.memory_current_after_bytes,
        recorded_cpu_max: probe.recorded_cpu_max.clone(),
        recorded_cpuset: probe.recorded_cpuset.clone(),
        failure_reason,
    }
}

pub fn run_validation_probe() -> Result<ValidationProbeResult, HarnessError> {
    let cgroup_root = current_cgroup_v2_path().unwrap_or_else(|| PathBuf::from("/sys/fs/cgroup"));
    let memory_max_raw = read_cgroup_file(&cgroup_root, "memory.max");
    let memory_current_before_raw = read_cgroup_file(&cgroup_root, "memory.current");

    let realized_memory_max_bytes = memory_max_raw.as_deref().and_then(parse_cgroup_numeric);
    let memory_current_before_bytes = memory_current_before_raw
        .as_deref()
        .and_then(parse_cgroup_numeric);
    let allocation_request_bytes = Some(select_allocation_request_bytes(realized_memory_max_bytes));

    let mut allocation = vec![0u8; allocation_request_bytes.unwrap_or(0) as usize];
    for page in allocation.iter_mut().step_by(4096) {
        *page = 1;
    }

    thread::sleep(Duration::from_millis(50));
    let memory_current_peak_bytes = read_cgroup_file(&cgroup_root, "memory.current")
        .as_deref()
        .and_then(parse_cgroup_numeric);

    drop(allocation);
    thread::sleep(Duration::from_millis(50));
    let memory_current_after_bytes = read_cgroup_file(&cgroup_root, "memory.current")
        .as_deref()
        .and_then(parse_cgroup_numeric);

    Ok(ValidationProbeResult {
        probe_version: VALIDATION_PROBE_VERSION.to_string(),
        memory_max_readable: memory_max_raw.is_some(),
        memory_current_readable: memory_current_before_raw.is_some(),
        realized_memory_max_bytes,
        memory_current_before_bytes,
        memory_current_peak_bytes,
        memory_current_after_bytes,
        allocation_request_bytes,
        recorded_cpu_max: read_cgroup_file(&cgroup_root, "cpu.max"),
        recorded_cpuset: read_cgroup_file(&cgroup_root, "cpuset.cpus.effective")
            .or_else(|| read_cgroup_file(&cgroup_root, "cpuset.cpus")),
    })
}

fn build_failure_reason(
    container_started: bool,
    docker_reports_cgroup_v2: bool,
    probe_version_matches: Option<bool>,
    memory_max_readable: bool,
    memory_current_readable: bool,
    memory_limit_matches: bool,
    allocation_sanity: bool,
) -> String {
    let mut reasons = Vec::new();
    if !container_started {
        reasons.push("container did not start");
    }
    if !docker_reports_cgroup_v2 {
        reasons.push("docker runtime is not cgroup v2");
    }
    if probe_version_matches == Some(false) {
        reasons.push("validate-probe version does not match host expectation");
    }
    if !memory_max_readable {
        reasons.push("memory.max is not readable");
    }
    if !memory_current_readable {
        reasons.push("memory.current is not readable");
    }
    if !memory_limit_matches {
        reasons.push("realized memory.max does not match requested limit");
    }
    if !allocation_sanity {
        reasons.push("allocation sanity probe fell outside tolerance");
    }
    reasons.join("; ")
}

fn allocation_within_tolerance(before: u64, peak: u64, requested: u64) -> bool {
    if requested == 0 || peak < before {
        return false;
    }
    let delta = peak - before;
    let lower = requested.saturating_mul(80) / 100;
    let upper = requested.saturating_mul(120) / 100;
    delta >= lower && delta <= upper
}

fn select_allocation_request_bytes(realized_memory_max_bytes: Option<u64>) -> u64 {
    let minimum = 8 * 1024 * 1024;
    let maximum = 64 * 1024 * 1024;
    match realized_memory_max_bytes {
        Some(limit) if limit > 0 => (limit / 8).clamp(minimum, maximum),
        _ => 32 * 1024 * 1024,
    }
}

fn read_cgroup_file(cgroup_root: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(cgroup_root.join(name))
        .ok()
        .map(|contents| contents.trim().to_string())
        .filter(|contents| !contents.is_empty())
}

pub fn validation_cache_path(output_root: &Path) -> std::path::PathBuf {
    output_root
        .parent()
        .unwrap_or(output_root)
        .join("validation_cache.json")
}

pub fn read_validation_cache(output_root: &Path) -> Result<ValidationCacheFile, HarnessError> {
    let path = validation_cache_path(output_root);
    if !path.exists() {
        return Ok(ValidationCacheFile::default());
    }
    let bytes = std::fs::read(path)?;
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(ValidationCacheFile::default());
    }
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn write_validation_cache(
    output_root: &Path,
    cache: &ValidationCacheFile,
) -> Result<(), HarnessError> {
    let path = validation_cache_path(output_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(cache)?)?;
    Ok(())
}

pub fn upsert_validation_cache_record(
    output_root: &Path,
    record: &ValidationRecord,
) -> Result<(), HarnessError> {
    let mut cache = read_validation_cache(output_root)?;
    if let Some(existing) = cache
        .entries
        .iter_mut()
        .find(|entry| entry.key == record.key)
    {
        *existing = record.clone();
    } else {
        cache.entries.push(record.clone());
    }
    write_validation_cache(output_root, &cache)
}

pub fn find_cached_validation(
    output_root: &Path,
    key: &ValidationCacheKey,
) -> Result<Option<ValidationRecord>, HarnessError> {
    let cache = read_validation_cache(output_root)?;
    Ok(cache
        .entries
        .into_iter()
        .find(|entry| &entry.key == key)
        .map(|entry| ValidationRecord::cache_hit(&entry)))
}

pub fn persist_validation_record(
    output_root: &Path,
    record: &ValidationRecord,
) -> Result<(), HarnessError> {
    write_validation(output_root, record)?;
    upsert_validation_cache_record(output_root, record)
}

pub fn read_validation_record(output_root: &Path) -> Result<ValidationRecord, HarnessError> {
    Ok(serde_json::from_slice(&std::fs::read(
        output_root.join("validation.json"),
    )?)?)
}

pub fn validate_cached_or_run<F>(
    output_root: &Path,
    key: ValidationCacheKey,
    container_started: bool,
    requested_memory_limit_bytes: u64,
    run_probe: F,
) -> Result<ValidationRecord, HarnessError>
where
    F: FnOnce() -> Result<ValidationProbeResult, HarnessError>,
{
    if let Some(cached) = find_cached_validation(output_root, &key)? {
        write_validation(output_root, &cached)?;
        if cached.status == ValidationStatus::Valid {
            return Ok(cached);
        }
        return Err(HarnessError::InvalidRequestedCase(
            cached
                .failure_reason
                .clone()
                .unwrap_or_else(|| "cached validation failed".to_string()),
        ));
    }

    let record = match run_probe() {
        Ok(probe) => {
            evaluate_validation_probe(key, container_started, requested_memory_limit_bytes, &probe)
        }
        Err(error) => invalid_validation_record(key, container_started, error.to_string()),
    };
    persist_validation_record(output_root, &record)?;

    if record.status == ValidationStatus::Valid {
        Ok(record)
    } else {
        Err(HarnessError::InvalidRequestedCase(
            record
                .failure_reason
                .clone()
                .unwrap_or_else(|| "validation failed".to_string()),
        ))
    }
}

fn invalid_validation_record(
    key: ValidationCacheKey,
    container_started: bool,
    failure_reason: String,
) -> ValidationRecord {
    let docker_reports_cgroup_v2 = key.cgroup_version == "2";
    ValidationRecord {
        key,
        status: ValidationStatus::Invalid,
        from_cache: false,
        observed_probe_version: None,
        probe_version_matches: None,
        container_started,
        docker_reports_cgroup_v2,
        memory_max_readable: false,
        memory_current_readable: false,
        memory_limit_matches: false,
        allocation_sanity: false,
        realized_memory_max_bytes: None,
        allocation_request_bytes: None,
        memory_current_before_bytes: None,
        memory_current_peak_bytes: None,
        memory_current_after_bytes: None,
        recorded_cpu_max: None,
        recorded_cpuset: None,
        failure_reason: Some(failure_reason),
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn sample_key() -> ValidationCacheKey {
        ValidationCacheKey {
            docker_engine_version: "28.0.1".to_string(),
            cgroup_version: "2".to_string(),
            image_digest: "sha256:abc".to_string(),
            memory_limit: "8g".to_string(),
            cpuset: Some("0-3".to_string()),
            cpu_quota: Some(200_000),
            cpu_period: Some(100_000),
            work_dir_mode: WorkDirMode::DockerTmpfs,
            probe_version: VALIDATION_PROBE_VERSION.to_string(),
        }
    }

    fn sample_record() -> ValidationRecord {
        ValidationRecord {
            key: sample_key(),
            status: ValidationStatus::Valid,
            from_cache: false,
            observed_probe_version: Some(VALIDATION_PROBE_VERSION.to_string()),
            probe_version_matches: Some(true),
            container_started: true,
            docker_reports_cgroup_v2: true,
            memory_max_readable: true,
            memory_current_readable: true,
            memory_limit_matches: true,
            allocation_sanity: true,
            realized_memory_max_bytes: Some(8 * 1024 * 1024 * 1024),
            allocation_request_bytes: Some(64 * 1024 * 1024),
            memory_current_before_bytes: Some(1_000),
            memory_current_peak_bytes: Some(65 * 1024 * 1024),
            memory_current_after_bytes: Some(2_000),
            recorded_cpu_max: Some("200000 100000".to_string()),
            recorded_cpuset: Some("0-3".to_string()),
            failure_reason: None,
        }
    }

    #[test]
    fn validation_cache_key_changes_with_tuple_fields() {
        let key = sample_key();
        let mut same = sample_key();
        assert_eq!(key, same);

        same.image_digest = "sha256:def".to_string();
        assert_ne!(key, same);
    }

    #[test]
    fn validation_cache_round_trips_records() {
        let tempdir = tempdir().expect("tempdir");
        let output_root = tempdir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");
        let record = sample_record();

        upsert_validation_cache_record(&output_root, &record).expect("write cache");
        let loaded = read_validation_cache(&output_root).expect("read cache");

        assert_eq!(loaded.entries, vec![record]);
    }

    #[test]
    fn validation_record_writes_root_artifact_and_cache_hit_sets_flag() {
        let tempdir = tempdir().expect("tempdir");
        let output_root = tempdir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");
        let record = sample_record();

        persist_validation_record(&output_root, &record).expect("persist validation");

        let written: ValidationRecord = serde_json::from_slice(
            &std::fs::read(output_root.join("validation.json")).expect("validation artifact"),
        )
        .expect("validation json");
        assert_eq!(written.status, ValidationStatus::Valid);
        assert!(!written.from_cache);

        let cached = find_cached_validation(&output_root, &record.key)
            .expect("read cache")
            .expect("cached record");
        assert!(cached.from_cache);
        assert_eq!(cached.status, ValidationStatus::Valid);
    }

    #[test]
    fn validation_probe_marks_mismatched_memory_limit_invalid() {
        let record = evaluate_validation_probe(
            sample_key(),
            true,
            8 * 1024 * 1024 * 1024,
            &ValidationProbeResult {
                probe_version: VALIDATION_PROBE_VERSION.to_string(),
                memory_max_readable: true,
                memory_current_readable: true,
                realized_memory_max_bytes: Some(4 * 1024 * 1024 * 1024),
                memory_current_before_bytes: Some(1_000),
                memory_current_peak_bytes: Some(70 * 1024 * 1024),
                memory_current_after_bytes: Some(2_000),
                allocation_request_bytes: Some(64 * 1024 * 1024),
                recorded_cpu_max: Some("200000 100000".to_string()),
                recorded_cpuset: Some("0-3".to_string()),
            },
        );

        assert_eq!(record.status, ValidationStatus::Invalid);
        assert!(!record.memory_limit_matches);
    }

    #[test]
    fn validation_probe_rejects_mismatched_probe_version() {
        let record = evaluate_validation_probe(
            sample_key(),
            true,
            8 * 1024 * 1024 * 1024,
            &ValidationProbeResult {
                probe_version: "phase3-v0".to_string(),
                memory_max_readable: true,
                memory_current_readable: true,
                realized_memory_max_bytes: Some(8 * 1024 * 1024 * 1024),
                memory_current_before_bytes: Some(1_000_000),
                memory_current_peak_bytes: Some(65 * 1024 * 1024),
                memory_current_after_bytes: Some(1_100_000),
                allocation_request_bytes: Some(64 * 1024 * 1024),
                recorded_cpu_max: None,
                recorded_cpuset: None,
            },
        );

        assert_eq!(record.status, ValidationStatus::Invalid);
        assert_eq!(record.probe_version_matches, Some(false));
        assert_eq!(record.observed_probe_version.as_deref(), Some("phase3-v0"));
    }

    #[test]
    fn validation_probe_requires_memory_files_to_be_readable() {
        let record = evaluate_validation_probe(
            sample_key(),
            true,
            8 * 1024 * 1024 * 1024,
            &ValidationProbeResult {
                probe_version: VALIDATION_PROBE_VERSION.to_string(),
                memory_max_readable: false,
                memory_current_readable: false,
                realized_memory_max_bytes: None,
                memory_current_before_bytes: None,
                memory_current_peak_bytes: None,
                memory_current_after_bytes: None,
                allocation_request_bytes: None,
                recorded_cpu_max: None,
                recorded_cpuset: None,
            },
        );

        assert_eq!(record.status, ValidationStatus::Invalid);
        assert!(!record.memory_max_readable);
        assert!(!record.memory_current_readable);
    }

    #[test]
    fn validation_probe_accepts_allocation_sanity_within_tolerance() {
        let allocation = 64 * 1024 * 1024;
        let record = evaluate_validation_probe(
            sample_key(),
            true,
            8 * 1024 * 1024 * 1024,
            &ValidationProbeResult {
                probe_version: VALIDATION_PROBE_VERSION.to_string(),
                memory_max_readable: true,
                memory_current_readable: true,
                realized_memory_max_bytes: Some(8 * 1024 * 1024 * 1024),
                memory_current_before_bytes: Some(1_000_000),
                memory_current_peak_bytes: Some(1_000_000 + allocation - 1024),
                memory_current_after_bytes: Some(1_100_000),
                allocation_request_bytes: Some(allocation),
                recorded_cpu_max: None,
                recorded_cpuset: None,
            },
        );

        assert_eq!(record.status, ValidationStatus::Valid);
        assert!(record.allocation_sanity);
    }

    #[test]
    fn validation_probe_rejects_allocation_sanity_outside_tolerance() {
        let allocation = 64 * 1024 * 1024;
        let record = evaluate_validation_probe(
            sample_key(),
            true,
            8 * 1024 * 1024 * 1024,
            &ValidationProbeResult {
                probe_version: VALIDATION_PROBE_VERSION.to_string(),
                memory_max_readable: true,
                memory_current_readable: true,
                realized_memory_max_bytes: Some(8 * 1024 * 1024 * 1024),
                memory_current_before_bytes: Some(1_000_000),
                memory_current_peak_bytes: Some(1_000_000 + (allocation / 2)),
                memory_current_after_bytes: Some(1_100_000),
                allocation_request_bytes: Some(allocation),
                recorded_cpu_max: None,
                recorded_cpuset: None,
            },
        );

        assert_eq!(record.status, ValidationStatus::Invalid);
        assert!(!record.allocation_sanity);
    }

    #[test]
    fn validate_cached_result_reused() {
        let tempdir = tempdir().expect("tempdir");
        let output_root = tempdir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");
        let record = sample_record();
        persist_validation_record(&output_root, &record).expect("persist record");

        let reused = validate_cached_or_run(&output_root, record.key.clone(), true, 1, || {
            panic!("cache hit should not run probe")
        })
        .expect("cached validation");

        assert!(reused.from_cache);
        assert_eq!(reused.status, ValidationStatus::Valid);
    }

    #[test]
    fn validate_failure_persists_artifact() {
        let tempdir = tempdir().expect("tempdir");
        let output_root = tempdir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");
        let key = sample_key();

        let error = validate_cached_or_run(&output_root, key, true, 1, || {
            Err(HarnessError::CommandFailure("probe failed".to_string()))
        })
        .expect_err("validation should fail");

        assert!(error.to_string().contains("probe failed"));
        let persisted = read_validation_record(&output_root).expect("validation artifact");
        assert_eq!(persisted.status, ValidationStatus::Invalid);
        assert!(persisted.container_started);
        assert!(persisted.docker_reports_cgroup_v2);
        assert_eq!(persisted.probe_version_matches, None);
        assert_eq!(
            persisted.failure_reason.as_deref(),
            Some("Command failure: probe failed")
        );
    }

    #[test]
    fn backend_validation_outcome_requires_valid_record() {
        let mut record = sample_record();
        assert!(BackendValidationOutcome::from_validation_record(&record).pma_replay_proven());

        record.status = ValidationStatus::Invalid;
        assert!(!BackendValidationOutcome::from_validation_record(&record).pma_replay_proven());
    }
}
