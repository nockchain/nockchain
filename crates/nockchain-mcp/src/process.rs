use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::Result;
use serde_json::Value;
use tokio::process::Command;

use crate::backend::Backend;
use crate::sandbox::SandboxKind;

#[allow(clippy::too_many_arguments)]
pub async fn run_isolated(
    kind: SandboxKind,
    backend: &Backend,
    code: String,
    rust_script: &Path,
    max_calls: usize,
    max_result_bytes: usize,
    process_timeout: Duration,
    memory_limit_mib: u64,
) -> Result<Value> {
    crate::sandbox::run_code(
        kind,
        backend.clone(),
        &code,
        rust_script,
        max_calls,
        max_result_bytes,
        process_timeout,
        memory_limit_mib,
    )
    .await
}

pub(crate) fn rust_script_command(
    executable: &Path,
    memory_limit_mib: u64,
    process_timeout: Duration,
) -> Result<Command> {
    let mut command = Command::new(executable);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    configure_resource_limits(&mut command, memory_limit_mib, process_timeout)?;
    Ok(command)
}

pub(crate) async fn terminate_process_group(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // The child starts a fresh session in configure_resource_limits, so its PID is also the
        // process-group ID. Killing the group includes rust-script, Cargo/rustc, and the script.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
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
    // SAFETY: pre_exec runs after fork and before exec. setsid and setrlimit are
    // async-signal-safe syscalls, and the closure does not touch shared process state.
    unsafe {
        command.as_std_mut().pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            // Darwin rejects finite RLIMIT_AS values before dyld starts. Linux accepts it and
            // applies the address-space bound to rust-script and all of its descendants.
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
    use std::path::Path;
    #[cfg(unix)]
    use std::time::Duration;

    #[cfg(unix)]
    #[tokio::test]
    async fn configured_resource_limits_allow_a_child_to_start() {
        let mut command =
            super::rust_script_command(Path::new("/usr/bin/true"), 512, Duration::from_secs(30))
                .expect("configure limits");
        let status = command.status().await.expect("spawn limited child");
        assert!(status.success());
    }
}
