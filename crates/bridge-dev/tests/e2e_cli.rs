use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};
use std::{fs, thread};

use anyhow::{anyhow, Context, Result};
use bridge_dev::artifacts::{
    BRIDGE_BIN_ENV, BRIDGE_JAM_ENV, CTL_BIN_ENV, FAKENET_GENESIS_ENV, NODE_BIN_ENV, ROSWELL_JAM_ENV,
};
use bridge_dev::e2e::E2eReport;
use tempfile::TempDir;

#[test]
fn help_exposes_stable_withdrawal_command() -> Result<()> {
    let output = Command::new(bridge_dev_bin())
        .args(["e2e", "withdrawal", "--help"])
        .output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    for option in [
        "--base", "--client", "--seed", "--build", "--artifacts", "--archive-rpc-url",
        "--keep-artifacts", "--run-root", "--timeout-secs",
    ] {
        assert!(stdout.contains(option), "help missing {option}");
    }
    Ok(())
}

#[test]
fn success_prints_stable_keys_and_writes_report() -> Result<()> {
    let fixture = CliFixture::new()?;
    let output = fixture.run("success", 11)?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = report_from_output(&output)?;
    assert_eq!(report.status, "passed");
    assert_eq!(report.steps_executed, 1);
    assert!(report.shutdown_attempted);
    assert!(report.run_dir.join("provisioned").is_file());
    assert!(report.run_dir.join("executed").is_file());
    assert!(report.run_dir.join("shutdown").is_file());
    Ok(())
}

#[test]
fn setup_assertion_shutdown_and_zero_step_fail_nonzero_with_reports() -> Result<()> {
    for (plan, expected) in [
        ("provision-failure", "provision"),
        ("assertion-failure", "execute"),
        ("shutdown-failure", "shutdown"),
        ("zero-steps", "zero steps"),
    ] {
        let fixture = CliFixture::new()?;
        let output = fixture.run(plan, 12)?;
        assert!(!output.status.success(), "{plan} unexpectedly succeeded");
        let report = report_from_output(&output)?;
        assert_eq!(report.status, "failed");
        assert!(report.shutdown_attempted);
        assert!(report.errors.iter().any(|error| error.contains(expected)));
        assert!(report.run_dir.join("shutdown").is_file() || plan == "shutdown-failure");
    }
    Ok(())
}

#[test]
fn concurrent_runs_never_share_run_directories() -> Result<()> {
    let fixture = CliFixture::new()?;
    let mut first = fixture.command("success", 21);
    let mut second = fixture.command("success", 22);
    first.stdout(Stdio::piped()).stderr(Stdio::piped());
    second.stdout(Stdio::piped()).stderr(Stdio::piped());
    let first = first.spawn()?;
    let second = second.spawn()?;
    let first_output = first.wait_with_output()?;
    let second_output = second.wait_with_output()?;
    assert!(first_output.status.success());
    assert!(second_output.status.success());
    let first_report = report_from_output(&first_output)?;
    let second_report = report_from_output(&second_output)?;
    assert_ne!(first_report.run_id, second_report.run_id);
    assert_ne!(first_report.run_dir, second_report.run_dir);
    Ok(())
}

#[test]
fn sigint_cancels_execution_and_still_runs_shutdown() -> Result<()> {
    let fixture = CliFixture::new()?;
    let mut command = fixture.command("wait", 31);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let run_dir = loop {
        if let Some(run_dir) = newest_run_dir(&fixture.run_root)? {
            if run_dir.join("provisioned").is_file() {
                break run_dir;
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            return Err(anyhow!("E2E child did not reach provisioned state"));
        }
        thread::sleep(Duration::from_millis(25));
    };
    let pid = child.id();
    let signal = Command::new("/bin/kill")
        .args(["-INT", &pid.to_string()])
        .status()?;
    assert!(signal.success());
    let output = child.wait_with_output()?;
    assert!(!output.status.success());
    let report = report_from_output(&output)?;
    assert_eq!(report.run_dir, run_dir);
    assert!(report
        .errors
        .iter()
        .any(|error| error.contains("cancelled")));
    assert!(report.shutdown_attempted);
    assert!(report.run_dir.join("shutdown").is_file());
    Ok(())
}

struct CliFixture {
    _tempdir: TempDir,
    run_root: PathBuf,
    bridge: PathBuf,
    node: PathBuf,
    ctl: PathBuf,
    bridge_jam: PathBuf,
    roswell_jam: PathBuf,
    fakenet: PathBuf,
}

impl CliFixture {
    fn new() -> Result<Self> {
        let tempdir = TempDir::new()?;
        let root = tempdir.path();
        let run_root = root.join("runs");
        fs::create_dir_all(&run_root)?;
        let bridge = root.join("bridge");
        let node = root.join("node");
        let ctl = root.join("ctl");
        let bridge_jam = root.join("bridge.jam");
        let roswell_jam = root.join("roswell.jam");
        let fakenet = root.join("fakenet.jam");
        for path in [&bridge, &node, &ctl] {
            write_binary(path)?;
        }
        for path in [&bridge_jam, &roswell_jam, &fakenet] {
            fs::write(path, [1u8; 32])?;
        }
        Ok(Self {
            _tempdir: tempdir,
            run_root,
            bridge,
            node,
            ctl,
            bridge_jam,
            roswell_jam,
            fakenet,
        })
    }

    fn command(&self, plan: &str, seed: u64) -> Command {
        let mut command = Command::new(bridge_dev_bin());
        command
            .args([
                "e2e",
                "withdrawal",
                "--base",
                "hermetic",
                "--client",
                "rust-reference",
                "--seed",
                &seed.to_string(),
                "--keep-artifacts",
                "--run-root",
                self.run_root.to_str().unwrap_or_default(),
                "--timeout-secs",
                "30",
            ])
            .env("BRIDGE_DEV_E2E_SCRIPTED_PLAN", plan)
            .env(BRIDGE_BIN_ENV, &self.bridge)
            .env(NODE_BIN_ENV, &self.node)
            .env(CTL_BIN_ENV, &self.ctl)
            .env(BRIDGE_JAM_ENV, &self.bridge_jam)
            .env(ROSWELL_JAM_ENV, &self.roswell_jam)
            .env(FAKENET_GENESIS_ENV, &self.fakenet);
        command
    }

    fn run(&self, plan: &str, seed: u64) -> Result<Output> {
        Ok(self.command(plan, seed).output()?)
    }
}

fn report_from_output(output: &Output) -> Result<E2eReport> {
    let stdout = String::from_utf8(output.stdout.clone())?;
    for key in ["run_id", "run_dir", "environment", "seed", "report"] {
        assert!(
            stdout
                .lines()
                .any(|line| line.starts_with(&format!("{key}="))),
            "stdout missing {key}: {stdout}"
        );
    }
    let report_path = stdout
        .lines()
        .find_map(|line| line.strip_prefix("report="))
        .map(PathBuf::from)
        .context("stdout missing report path")?;
    Ok(serde_json::from_str(&fs::read_to_string(report_path)?)?)
}

fn newest_run_dir(root: &Path) -> Result<Option<PathBuf>> {
    let mut entries = fs::read_dir(root)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    entries.sort();
    Ok(entries.pop())
}

fn bridge_dev_bin() -> PathBuf {
    option_env!("CARGO_BIN_EXE_bridge-dev")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/bridge-dev")
        })
}

fn write_binary(path: &Path) -> Result<()> {
    let mut header = [0u8; 32];
    #[cfg(target_arch = "aarch64")]
    {
        header[..4].copy_from_slice(&0xfeedfacfu32.to_le_bytes());
        header[4..8].copy_from_slice(&0x0100_000cu32.to_le_bytes());
    }
    #[cfg(target_arch = "x86_64")]
    {
        header[..4].copy_from_slice(&0xfeedfacfu32.to_le_bytes());
        header[4..8].copy_from_slice(&0x0100_0007u32.to_le_bytes());
    }
    fs::write(path, header)?;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}
