use std::path::{Path, PathBuf};

use futures::FutureExt;
use tokio::process::Command;

use super::artifacts::{read_run_artifacts, write_verdict};
use super::execute::{CpuProfileArtifact, CpuProfileExecutionKind};
use super::summary::{Validity, Verdict};
use super::{
    CpuProfilerKind, HarnessError, DEFAULT_THROUGHPUT_CV_THRESHOLD, VERDICT_SCHEMA_VERSION,
};

const BYTEHOUND_PROFILE: &str = "bytehound";
const CPU_PROFILE_SYMBOL_DIR: &str = "symbols";
const CPU_PROFILE_SYMBOL_BINARY: &str = "symbols/nockchain-bench";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuProfilerLaunchRequest {
    pub profiler_kind: CpuProfilerKind,
    pub sample_rate_hz: u32,
    pub execution_kind: CpuProfileExecutionKind,
    pub case_root: PathBuf,
    pub output_relative_path: PathBuf,
    pub symbol_dir_relative_path: PathBuf,
    pub symbol_binary_relative_path: PathBuf,
    pub profiled_run_dir: PathBuf,
    pub profiled_command: Vec<String>,
}

impl CpuProfilerLaunchRequest {
    pub fn output_path(&self) -> PathBuf {
        self.case_root.join(&self.output_relative_path)
    }

    pub fn symbol_dir_path(&self) -> PathBuf {
        self.case_root.join(&self.symbol_dir_relative_path)
    }

    pub fn symbol_binary_path(&self) -> PathBuf {
        self.case_root.join(&self.symbol_binary_relative_path)
    }

    pub fn artifact(&self) -> CpuProfileArtifact {
        CpuProfileArtifact {
            profiler_kind: self.profiler_kind,
            sample_rate_hz: self.sample_rate_hz,
            execution_kind: self.execution_kind.clone(),
            profiled_command: self.profiled_command.clone(),
            output_relative_path: self.output_relative_path.clone(),
            symbol_dir_relative_path: self.symbol_dir_relative_path.clone(),
            symbol_binary_relative_path: self.symbol_binary_relative_path.clone(),
        }
    }
}

pub fn cpu_profile_symbol_dir_relative_path() -> PathBuf {
    PathBuf::from(CPU_PROFILE_SYMBOL_DIR)
}

pub fn cpu_profile_symbol_binary_relative_path() -> PathBuf {
    PathBuf::from(CPU_PROFILE_SYMBOL_BINARY)
}

pub(super) fn invalidate_verdict_for_cpu_profiling_failure(
    output_root: &Path,
    error: &HarnessError,
) -> Result<(), HarnessError> {
    std::fs::create_dir_all(output_root)?;
    write_verdict(
        output_root,
        &Verdict {
            schema_version: VERDICT_SCHEMA_VERSION.to_string(),
            allow_debug_benchmark: false,
            allow_version_skew: false,
            allow_degraded_cold: false,
            cv_threshold: DEFAULT_THROUGHPUT_CV_THRESHOLD,
            validity: Validity::Invalid {
                reasons: vec![format!("cpu profiling failed: {error}")],
            },
        },
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalCommand {
    pub program: String,
    pub args: Vec<String>,
}

pub fn build_samply_record_command(
    sample_rate_hz: u32,
    output_path: &Path,
    profiled_command: &[String],
) -> Result<ExternalCommand, HarnessError> {
    if profiled_command.is_empty() {
        return Err(HarnessError::InvalidRequestedCase(
            "profiled command must not be empty".to_string(),
        ));
    }

    let mut args = vec![
        "record".to_string(),
        "--save-only".to_string(),
        "--rate".to_string(),
        sample_rate_hz.to_string(),
        "--output".to_string(),
        output_path.to_string_lossy().to_string(),
        "--".to_string(),
    ];
    args.extend(profiled_command.iter().cloned());

    Ok(ExternalCommand {
        program: "samply".to_string(),
        args,
    })
}

pub fn build_run_once_command(
    binary: &str,
    resolved_case_path: &str,
    run_dir: &str,
    run_id: &str,
) -> Vec<String> {
    vec![
        binary.to_string(),
        "sol".to_string(),
        "run-once".to_string(),
        "--resolved-case".to_string(),
        resolved_case_path.to_string(),
        "--run-dir".to_string(),
        run_dir.to_string(),
        "--run-id".to_string(),
        run_id.to_string(),
    ]
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn bytehound_binary_path(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join("target")
        .join(BYTEHOUND_PROFILE)
        .join("nockchain-bench")
}

fn current_build_profile_from_exe(current_exe: &Path) -> Option<&str> {
    let parent = current_exe.parent()?;
    if parent.file_name().and_then(|name| name.to_str()) == Some("deps") {
        parent.parent()?.file_name()?.to_str()
    } else {
        parent.file_name()?.to_str()
    }
}

pub(crate) fn choose_samply_profiled_binary_path(
    workspace_root: &Path,
    current_exe: &Path,
) -> PathBuf {
    match current_build_profile_from_exe(current_exe) {
        Some(BYTEHOUND_PROFILE) | None => current_exe.to_path_buf(),
        Some(_) => bytehound_binary_path(workspace_root),
    }
}

fn command_failure_detail(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return stdout;
    }

    format!("exit status {}", output.status)
}

fn run_checked_command(
    program: &Path,
    args: &[&str],
    current_dir: &Path,
    description: &str,
) -> Result<(), HarnessError> {
    let output = std::process::Command::new(program)
        .args(args)
        .current_dir(current_dir)
        .output()
        .map_err(|error| map_spawn_error(&program.to_string_lossy(), error))?;

    if output.status.success() {
        return Ok(());
    }

    Err(HarnessError::CommandFailure(format!(
        "{description} failed: {}",
        command_failure_detail(&output)
    )))
}

pub fn ensure_samply_profiled_binary(current_exe: &Path) -> Result<PathBuf, HarnessError> {
    let workspace_root = workspace_root();
    let profiled_binary = choose_samply_profiled_binary_path(&workspace_root, current_exe);
    if profiled_binary == current_exe {
        return Ok(profiled_binary);
    }

    eprintln!(
        "CPU profiling requested with samply; building `nockchain-bench` with `cargo build --profile bytehound -p nockchain-bench` for full debug symbols"
    );
    let cargo = std::env::var_os("CARGO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cargo"));
    run_checked_command(
        &cargo,
        &["build", "--profile", BYTEHOUND_PROFILE, "-p", "nockchain-bench"],
        &workspace_root,
        "auto-building bytehound profiling binary",
    )?;

    if !profiled_binary.is_file() {
        return Err(HarnessError::CommandFailure(format!(
            "expected bytehound profiling binary at {} after auto-build",
            profiled_binary.display()
        )));
    }

    Ok(profiled_binary)
}

pub trait CpuProfilerLauncher {
    fn preflight<'a>(
        &'a self,
        _request: &'a CpuProfilerLaunchRequest,
    ) -> futures::future::BoxFuture<'a, Result<(), HarnessError>> {
        async { Ok(()) }.boxed()
    }

    fn launch<'a>(
        &'a mut self,
        request: &'a CpuProfilerLaunchRequest,
    ) -> futures::future::BoxFuture<'a, Result<CpuProfileArtifact, HarnessError>>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemCpuProfilerLauncher;

fn map_spawn_error(program: &str, error: std::io::Error) -> HarnessError {
    if error.kind() == std::io::ErrorKind::NotFound {
        HarnessError::CommandFailure(format!("{program} is not installed or not on PATH"))
    } else {
        HarnessError::Io(error)
    }
}

fn current_perf_event_paranoid_value() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        super::read_trimmed_file("/proc/sys/kernel/perf_event_paranoid")
    }

    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

pub async fn preflight_samply_profiler() -> Result<(), HarnessError> {
    validate_samply_perf_preconditions(current_perf_event_paranoid_value().as_deref())?;
    let output = match Command::new("samply").arg("--help").output().await {
        Ok(output) => output,
        Err(error) => return Err(map_spawn_error("samply", error)),
    };
    if !output.status.success() {
        return Err(HarnessError::CommandFailure(format!(
            "samply --help failed: {}",
            command_failure_detail(&output)
        )));
    }

    Ok(())
}

pub(crate) fn validate_samply_perf_preconditions(
    perf_event_paranoid: Option<&str>,
) -> Result<(), HarnessError> {
    if let Some(value) = perf_event_paranoid {
        if let Some(error) =
            perf_event_paranoid_error(value).map_err(HarnessError::CommandFailure)?
        {
            return Err(HarnessError::CommandFailure(error));
        }
    }
    Ok(())
}

impl CpuProfilerLauncher for SystemCpuProfilerLauncher {
    fn preflight<'a>(
        &'a self,
        request: &'a CpuProfilerLaunchRequest,
    ) -> futures::future::BoxFuture<'a, Result<(), HarnessError>> {
        async move {
            match request.profiler_kind {
                CpuProfilerKind::Samply => {
                    preflight_samply_profiler().await?;
                }
            }
            Ok(())
        }
        .boxed()
    }

    fn launch<'a>(
        &'a mut self,
        request: &'a CpuProfilerLaunchRequest,
    ) -> futures::future::BoxFuture<'a, Result<CpuProfileArtifact, HarnessError>> {
        async move {
            let output_path = request.output_path();
            if let Some(parent) = output_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let command = match request.profiler_kind {
                CpuProfilerKind::Samply => {
                    validate_samply_perf_preconditions(
                        current_perf_event_paranoid_value().as_deref(),
                    )?;
                    build_samply_record_command(
                        request.sample_rate_hz, &output_path, &request.profiled_command,
                    )?
                }
            };

            run_samply_record_command(&command, &output_path).await?;
            validate_profiled_run(&request.profiled_run_dir)?;
            persist_local_symbol_binary(request)?;

            Ok(request.artifact())
        }
        .boxed()
    }
}

fn persist_local_symbol_binary(request: &CpuProfilerLaunchRequest) -> Result<(), HarnessError> {
    let source = request
        .profiled_command
        .first()
        .ok_or_else(|| {
            HarnessError::InvalidRequestedCase(
                "profiled command must include the binary path".to_string(),
            )
        })?
        .clone();
    let source = PathBuf::from(source);
    if !source.is_file() {
        return Err(HarnessError::CommandFailure(format!(
            "profiled binary is missing at {}",
            source.display()
        )));
    }

    std::fs::create_dir_all(request.symbol_dir_path())?;
    std::fs::copy(&source, request.symbol_binary_path())?;
    Ok(())
}

pub async fn run_samply_record_command(
    command: &ExternalCommand,
    output_path: &Path,
) -> Result<(), HarnessError> {
    let output = match Command::new(&command.program)
        .args(&command.args)
        .output()
        .await
    {
        Ok(output) => output,
        Err(error) => return Err(map_spawn_error(&command.program, error)),
    };
    if !output.status.success() {
        return Err(HarnessError::CommandFailure(format!(
            "{} {} failed: {}",
            command.program,
            command.args.join(" "),
            augment_perf_permission_guidance(&command_failure_detail(&output))
        )));
    }
    if !output_path.exists() {
        return Err(HarnessError::CommandFailure(format!(
            "profiler succeeded but output artifact is missing at {}",
            output_path.display()
        )));
    }

    Ok(())
}

pub(crate) fn validate_profiled_run(run_dir: &Path) -> Result<(), HarnessError> {
    let completed = read_run_artifacts(run_dir).map_err(|error| {
        HarnessError::CommandFailure(format!(
            "profiled run did not produce readable artifacts at {}: {error}",
            run_dir.display()
        ))
    })?;

    if completed.record.success {
        return Ok(());
    }

    Err(HarnessError::CommandFailure(format!(
        "profiled run failed: {}",
        completed
            .record
            .error
            .unwrap_or_else(|| "run failed".to_string())
    )))
}

fn perf_event_paranoid_error(value: &str) -> Result<Option<String>, String> {
    let parsed = value.parse::<i32>().map_err(|_| {
        format!(
            "CPU profiling preflight found kernel.perf_event_paranoid but its value was unparseable: {value}"
        )
    })?;
    Ok((parsed > 1).then(|| {
        format!(
            "CPU profiling requires kernel.perf_event_paranoid <= 1 for unprivileged profiling; current value is {parsed}"
        )
    }))
}

pub(crate) fn augment_perf_permission_guidance(detail: &str) -> String {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("mmap failed") {
        return format!(
            "{detail}; on high-core Linux hosts, try limiting CPU affinity for profiling with `taskset` (for example `taskset -c 0-3 ...`). This is often sufficient for single-threaded workloads such as a single NockVM replay"
        );
    }
    if lower.contains("operation not permitted")
        || lower.contains("permission denied")
        || lower.contains("perf_event_open")
    {
        format!(
            "{detail}; ensure kernel.perf_event_paranoid <= 1 and the profiler has permission to use perf events"
        )
    } else {
        detail.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};

    use tempfile::tempdir;

    use super::{
        augment_perf_permission_guidance, build_run_once_command, build_samply_record_command,
        choose_samply_profiled_binary_path, cpu_profile_symbol_binary_relative_path,
        cpu_profile_symbol_dir_relative_path, map_spawn_error, perf_event_paranoid_error,
        validate_samply_perf_preconditions, CpuProfilerLaunchRequest, CpuProfilerLauncher,
        SystemCpuProfilerLauncher,
    };
    use crate::speed_of_light::harness::{CpuProfileExecutionKind, CpuProfilerKind, HarnessError};

    #[test]
    fn system_profiler_reports_missing_samply_as_command_failure() {
        let command = build_samply_record_command(
            1_000,
            &PathBuf::from("profiles/samply-profile.json.gz"),
            &["/bin/true".to_string()],
        )
        .expect("build samply command");
        let error = map_spawn_error(
            &command.program,
            std::io::Error::new(std::io::ErrorKind::NotFound, "missing samply"),
        );
        match error {
            HarnessError::CommandFailure(message) => {
                assert!(message.contains("samply"));
            }
            other => panic!("expected explicit command failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn system_profiler_reports_profiled_replay_failure_as_command_failure() {
        let _guard = env_lock().lock().expect("env lock");
        let tempdir = tempdir().expect("tempdir");
        let bin_dir = tempdir.path().join("bin");
        std::fs::create_dir_all(&bin_dir).expect("bin dir");

        let samply_path = bin_dir.join("samply");
        std::fs::write(
            &samply_path,
            r#"#!/bin/sh
output=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output)
      output="$2"
      shift 2
      ;;
    --)
      shift
      break
      ;;
    *)
      shift
      ;;
  esac
done
mkdir -p "$(dirname "$output")"
printf 'profile\n' > "$output"
"$@"
"#,
        )
        .expect("write fake samply");
        set_executable(&samply_path);

        let profiled_command = bin_dir.join("profiled-run");
        std::fs::write(
            &profiled_command,
            r#"#!/bin/sh
run_dir="$1"
mkdir -p "$run_dir"
cat <<'EOF' > "$run_dir/result.json"
{
  "run_id": "profile",
  "success": false,
  "error": "replay failed under profiling",
  "blocks_poked": 0,
  "failed_pokes": 0,
  "init_time_secs": 0.0,
  "total_replay_time_secs": 0.0,
  "throughput_blocks_per_second": 0.0,
  "average_block_time_ms": 0.0,
  "peak_process_rss_bytes": null,
  "minor_faults_total": null,
  "major_faults_total": null
}
EOF
: > "$run_dir/stdout.log"
printf 'replay failed under profiling\n' > "$run_dir/stderr.log"
exit 0
"#,
        )
        .expect("write profiled command");
        set_executable(&profiled_command);

        let _path_guard = PathEnvGuard::prepend(&bin_dir);

        let case_root = tempdir.path().join("case");
        let profile_run_dir = case_root.join("profile-run");
        let request = CpuProfilerLaunchRequest {
            profiler_kind: CpuProfilerKind::Samply,
            sample_rate_hz: 1_000,
            execution_kind: CpuProfileExecutionKind::Native,
            case_root: case_root.clone(),
            output_relative_path: PathBuf::from("profiles/samply-profile.json.gz"),
            symbol_dir_relative_path: cpu_profile_symbol_dir_relative_path(),
            symbol_binary_relative_path: cpu_profile_symbol_binary_relative_path(),
            profiled_run_dir: profile_run_dir.clone(),
            profiled_command: vec![
                profiled_command.to_string_lossy().to_string(),
                profile_run_dir.to_string_lossy().to_string(),
            ],
        };

        let error = SystemCpuProfilerLauncher
            .launch(&request)
            .await
            .expect_err("failed profiled replay should fail profiling");

        match error {
            HarnessError::CommandFailure(message) => {
                assert!(!message.trim().is_empty());
            }
            other => panic!("expected command failure, got {other:?}"),
        }
    }

    #[test]
    fn build_run_once_command_targets_hidden_cli_entrypoint() {
        let command = build_run_once_command(
            "/tmp/nockchain-bench", "/tmp/resolved_case.json", "/tmp/profile-run", "profile",
        );

        assert_eq!(
            command,
            vec![
                "/tmp/nockchain-bench", "sol", "run-once", "--resolved-case",
                "/tmp/resolved_case.json", "--run-dir", "/tmp/profile-run", "--run-id", "profile",
            ]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
        );
    }

    #[test]
    fn release_binary_switches_samply_to_bytehound_output() {
        let workspace_root = PathBuf::from("/repo");
        let release_binary = workspace_root.join("target/release/nockchain-bench");

        assert_eq!(
            choose_samply_profiled_binary_path(&workspace_root, &release_binary),
            workspace_root.join("target/bytehound/nockchain-bench")
        );
    }

    #[test]
    fn bytehound_binary_is_reused_for_samply() {
        let workspace_root = PathBuf::from("/repo");
        let bytehound_binary = workspace_root.join("target/bytehound/nockchain-bench");

        assert_eq!(
            choose_samply_profiled_binary_path(&workspace_root, &bytehound_binary),
            bytehound_binary
        );
    }

    #[test]
    fn perf_event_paranoid_above_one_is_rejected() {
        let error = perf_event_paranoid_error("2")
            .expect("parse should succeed")
            .expect("should reject");
        assert!(error.contains("perf_event_paranoid <= 1"));
        assert!(error.contains("current value is 2"));
        assert!(perf_event_paranoid_error("1")
            .expect("parse should succeed")
            .is_none());
    }

    #[test]
    fn samply_perf_preflight_rejects_perf_event_paranoid_above_one() {
        let error = validate_samply_perf_preconditions(Some("2"))
            .expect_err("high perf_event_paranoid should fail preflight");
        assert!(error.to_string().contains("perf_event_paranoid <= 1"));
        assert!(error.to_string().contains("current value is 2"));
    }

    #[test]
    fn samply_perf_preflight_rejects_unparseable_perf_event_paranoid() {
        let error = validate_samply_perf_preconditions(Some("not-a-number"))
            .expect_err("unparseable perf_event_paranoid should fail preflight");
        assert!(error.to_string().contains("perf_event_paranoid"));
        assert!(error.to_string().contains("unparseable"));
        assert!(error.to_string().contains("not-a-number"));
    }

    #[test]
    fn samply_perf_preflight_allows_supported_or_missing_values() {
        validate_samply_perf_preconditions(Some("1")).expect("supported value");
        validate_samply_perf_preconditions(None).expect("missing kernel setting should not fail");
    }

    #[test]
    fn perf_permission_failures_gain_operator_guidance() {
        let message =
            augment_perf_permission_guidance("perf_event_open failed: Operation not permitted");
        assert!(message.contains("Operation not permitted"));
        assert!(message.contains("perf_event_paranoid"));
    }

    #[test]
    fn mmap_failures_gain_taskset_guidance() {
        let message = augment_perf_permission_guidance("Failed to start profiling: mmap failed");
        assert!(message.contains("mmap failed"));
        assert!(message.contains("taskset"));
        assert!(message.contains("single-threaded"));
    }

    fn env_lock() -> &'static Mutex<()> {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        ENV_LOCK.get_or_init(|| Mutex::new(()))
    }

    struct PathEnvGuard {
        previous: Option<std::ffi::OsString>,
    }

    impl PathEnvGuard {
        fn prepend(bin_dir: &std::path::Path) -> Self {
            let previous = std::env::var_os("PATH");
            let mut combined = bin_dir.as_os_str().to_os_string();
            if let Some(previous_path) = &previous {
                combined.push(":");
                combined.push(previous_path);
            }
            std::env::set_var("PATH", combined);
            Self { previous }
        }
    }

    impl Drop for PathEnvGuard {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(previous) => std::env::set_var("PATH", previous),
                None => std::env::remove_var("PATH"),
            }
        }
    }

    fn set_executable(path: &std::path::Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(path, permissions).expect("set permissions");
        }
    }
}
