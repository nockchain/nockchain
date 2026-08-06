use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::backend::Backend;
use crate::sandbox::SandboxKind;

#[derive(Debug, Serialize, Deserialize)]
struct SandboxInput {
    code: String,
}

pub async fn run_isolated(
    kind: SandboxKind,
    backend: &Backend,
    code: String,
    max_calls: usize,
    max_result_bytes: usize,
    process_timeout: Duration,
    memory_limit_mib: u64,
) -> Result<Value> {
    let executable = std::env::current_exe().context("locate the nockchain-mcp executable")?;
    let mut command = Command::new(executable);
    command
        .arg("--internal-sandbox-kind")
        .arg(match kind {
            SandboxKind::Search => "search",
            SandboxKind::Execute => "execute",
        })
        .arg("--mode")
        .arg(match backend.mode {
            crate::catalog::ApiMode::Public => "public",
            crate::catalog::ApiMode::Private => "private",
        })
        .arg("--grpc-backend")
        .arg(&backend.endpoint)
        .arg("--grpc-timeout-ms")
        .arg(backend.timeout.as_millis().to_string())
        .arg("--max-calls")
        .arg(max_calls.to_string())
        .arg("--max-result-bytes")
        .arg(max_result_bytes.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    configure_resource_limits(&mut command, memory_limit_mib, process_timeout)?;

    let mut child = command.spawn().context("spawn the JavaScript sandbox")?;
    let input = serde_json::to_vec(&SandboxInput { code })?;
    let mut stdin = child.stdin.take().context("open sandbox stdin")?;
    stdin
        .write_all(&input)
        .await
        .context("write sandbox input")?;
    stdin.shutdown().await.context("close sandbox stdin")?;
    drop(stdin);

    let output = tokio::time::timeout(process_timeout, child.wait_with_output())
        .await
        .with_context(|| format!("sandbox exceeded the {process_timeout:?} wall-time limit"))??;
    if output.stdout.len() > max_result_bytes.saturating_add(64 * 1024) {
        bail!("sandbox output exceeded its configured limit")
    }
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "sandbox failed ({}): {}{}",
            output.status,
            stderr.trim(),
            if stdout.trim().is_empty() {
                String::new()
            } else {
                format!("; output: {}", stdout.trim())
            }
        )
    }
    serde_json::from_slice(&output.stdout).context("decode sandbox JSON output")
}

pub async fn run_internal_sandbox(
    kind: SandboxKind,
    backend: Backend,
    max_calls: usize,
    max_result_bytes: usize,
) -> Result<()> {
    let mut input = String::new();
    tokio::io::stdin()
        .read_to_string(&mut input)
        .await
        .context("read sandbox input")?;
    let input: SandboxInput = serde_json::from_str(&input).context("decode sandbox input")?;
    let result = crate::sandbox::run_code(kind, backend, &input.code, max_calls, max_result_bytes)?;
    let encoded = serde_json::to_vec(&result)?;
    if encoded.len() > max_result_bytes {
        bail!("sandbox result exceeded the configured byte limit")
    }
    let mut stdout = tokio::io::stdout();
    stdout.write_all(&encoded).await?;
    stdout.shutdown().await?;
    Ok(())
}

#[cfg(unix)]
fn configure_resource_limits(
    command: &mut Command,
    memory_limit_mib: u64,
    process_timeout: Duration,
) -> Result<()> {
    use std::os::unix::process::CommandExt;

    #[cfg(target_os = "linux")]
    let bytes = memory_limit_mib
        .checked_mul(1024 * 1024)
        .context("sandbox memory limit is too large")?;
    #[cfg(not(target_os = "linux"))]
    let _ = memory_limit_mib;
    let cpu_seconds = process_timeout.as_secs().clamp(1, 60);
    // SAFETY: pre_exec runs after fork and before exec. The closure only invokes the
    // async-signal-safe setrlimit syscall and constructs an io::Error from errno.
    unsafe {
        command.as_std_mut().pre_exec(move || {
            // Darwin rejects finite RLIMIT_AS values for this executable before dyld starts.
            // Linux accepts RLIMIT_AS and uses it as the sandbox's hard address-space bound.
            #[cfg(target_os = "linux")]
            set_limit(libc::RLIMIT_AS, bytes)?;
            set_limit(libc::RLIMIT_CPU, cpu_seconds)?;
            Ok(())
        });
    }
    Ok(())
}

#[cfg(unix)]
fn set_limit(resource: libc::c_int, limit: u64) -> std::io::Result<()> {
    let value = libc::rlimit {
        rlim_cur: limit,
        rlim_max: limit,
    };
    // SAFETY: value is a valid rlimit pointer for the duration of the syscall.
    if unsafe { libc::setrlimit(resource, &value) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn configure_resource_limits(
    _command: &mut Command,
    _memory_limit_mib: u64,
    _process_timeout: Duration,
) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::time::Duration;

    #[cfg(unix)]
    #[tokio::test]
    async fn configured_resource_limits_allow_a_child_to_start() {
        let mut command = tokio::process::Command::new("/usr/bin/true");
        super::configure_resource_limits(&mut command, 512, Duration::from_secs(30))
            .expect("configure limits");
        let status = command.status().await.expect("spawn limited child");
        assert!(status.success());
    }
}
