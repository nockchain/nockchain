use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use bridge_dev::scenario::{ProcessTarget, ScenarioHarness, PORT_OFFSET_ENV, TEST_RUN_ROOT_ENV};
use tempfile::TempDir;

#[test]
fn command_shape_and_environment_match_existing_contract() -> Result<()> {
    let fixture = HarnessFixture::new("command-shape", false)?;
    let mut harness = fixture.harness()?;
    harness.extend_env_overrides([("BRIDGE_SCENARIO_MARKER".to_owned(), "present".to_owned())]);
    let command = harness.command(&["status", "--bridges", "--sequencer"]);

    assert_eq!(command.get_program(), fixture.binary.as_os_str());
    assert_eq!(
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        ["status", "--bridges", "--sequencer"]
    );
    let environment = command
        .get_envs()
        .filter_map(|(key, value)| {
            Some((
                key.to_string_lossy().into_owned(),
                value?.to_string_lossy().into_owned(),
            ))
        })
        .collect::<Vec<_>>();
    assert!(environment
        .iter()
        .any(|(key, value)| key == TEST_RUN_ROOT_ENV
            && value == harness.paths().run_root.to_string_lossy().as_ref()));
    assert!(environment.iter().any(|(key, _)| key == PORT_OFFSET_ENV));
    assert!(environment
        .iter()
        .any(|(key, value)| key == "BRIDGE_SCENARIO_MARKER" && value == "present"));
    Ok(())
}

#[test]
fn process_target_commands_use_existing_cli_shape() -> Result<()> {
    let fixture = HarnessFixture::new("targets", false)?;
    let harness = fixture.harness()?;
    harness.start_targets(&[ProcessTarget::Node, ProcessTarget::Bridge(3)])?;
    harness.stop_targets(&[ProcessTarget::Bridge(3)])?;

    let commands = fs::read_to_string(harness.paths().run_root.join("commands.log"))?;
    assert!(commands.lines().any(|line| line == "start node bridge-3"));
    assert!(commands.lines().any(|line| line == "stop bridge-3"));
    assert!(harness.start_targets(&[]).is_err());
    assert!(ProcessTarget::Bridge(5).name().is_err());
    Ok(())
}

#[test]
fn partial_startup_stops_child_and_preserves_failure_artifacts() -> Result<()> {
    let fixture = HarnessFixture::new("partial-startup", true)?;
    let mut harness = fixture.harness()?;
    let paths = harness.paths();
    let error = harness
        .spawn_cluster(&["up", "--fresh", "--start"], Duration::from_millis(300))
        .expect_err("status failure must abort startup");
    assert!(format!("{error:#}").contains("<unavailable>"));

    drop(harness);

    assert!(
        paths.run_root.is_dir(),
        "failure run root was not preserved"
    );
    assert!(paths.run_root.join("stopped").is_file());
    assert!(paths.up_stdout.is_file());
    assert!(paths.up_stderr.is_file());
    Ok(())
}

#[test]
fn explicit_stop_terminates_child_without_preserving_live_process() -> Result<()> {
    let fixture = HarnessFixture::new("explicit-stop", false)?;
    let mut harness = fixture.harness()?;
    harness.spawn_cluster(&["up", "--start"], Duration::from_secs(3))?;
    let paths = harness.paths();
    harness.stop();

    assert!(paths.run_root.join("stopped").is_file());
    let status = harness.run_status_command_allowing_endpoint_failures(&["status"])?;
    assert!(status.contains("processes:"));
    Ok(())
}

struct HarnessFixture {
    _tempdir: TempDir,
    workspace: PathBuf,
    binary: PathBuf,
}

impl HarnessFixture {
    fn new(name: &str, fail_status: bool) -> Result<Self> {
        let tempdir = TempDir::new()?;
        let workspace = tempdir.path().join(name);
        let release = workspace.join("target/release");
        fs::create_dir_all(&release)?;
        for binary in ["bridge", "nockchain-bridge-sequencer", "nockchain-wallet"] {
            fs::write(release.join(binary), b"fixture")?;
        }
        if fail_status {
            fs::write(workspace.join("fail-status"), b"1")?;
        }
        let binary = workspace.join("bridge-dev-fixture.sh");
        fs::write(&binary, fake_bridge_dev_script())?;
        let mut permissions = fs::metadata(&binary)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&binary, permissions)?;
        Ok(Self {
            _tempdir: tempdir,
            workspace,
            binary,
        })
    }

    fn harness(&self) -> Result<ScenarioHarness> {
        ScenarioHarness::with_binary(
            "scenario-harness-lifecycle",
            self.workspace.clone(),
            self.binary.clone(),
        )
    }
}

fn fake_bridge_dev_script() -> &'static [u8] {
    br##"#!/bin/sh
set -eu
root="${BRIDGE_DEV_TEST_RUN_ROOT:?}"
mkdir -p "$root"
printf '%s\n' "$*" >> "$root/commands.log"
case "${1:-}" in
  up)
    printf '%s\n' "$$" > "$root/fake-up.pid"
    trap 'printf stopped > "$root/stopped"; exit 0' TERM INT
    printf '\033[31mstarting fixture cluster\033[0m\n'
    printf '\377' >&2
    while :; do sleep 1; done
    ;;
  down)
    if [ -f "$root/fake-up.pid" ]; then
      kill -TERM "$(cat "$root/fake-up.pid")" 2>/dev/null || true
    fi
    ;;
  status)
    if [ -f "$PWD/fail-status" ]; then
      exit 1
    fi
    cat <<'STATUS'
processes:
  node running pid=1
  bridge-0 running pid=2
  bridge-1 running pid=3
  bridge-2 running pid=4
  bridge-3 running pid=5
  bridge-4 running pid=6
bridge_streams:
  bridge-0 running_state=Running base_height=1 nock_height=2 nockchain_api=Connected batch_status=idle unhealthy_peers=0
sequencer_status:
  reserved_inputs=0 next_pending=none
STATUS
    ;;
  start|stop)
    ;;
  *)
    exit 1
    ;;
esac
"##
}
