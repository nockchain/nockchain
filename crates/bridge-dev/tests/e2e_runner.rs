use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::Result;
use bridge_dev::artifacts::{
    ArtifactOverrides, ArtifactResolveOptions, ArtifactResolver, BinaryArchitecture,
};
use bridge_dev::e2e::{
    E2eBaseMode, E2eClientMode, E2eRunConfig, E2eRunner, E2eRunnerError, ScriptedE2eExecutor,
    ScriptedPlan,
};
use bridge_dev::evidence::WithdrawalEvidenceCapsuleV1;
use tempfile::TempDir;
use tokio::sync::watch;

#[tokio::test]
async fn report_write_failure_is_non_success_after_shutdown() -> Result<()> {
    let tempdir = TempDir::new()?;
    let artifacts = create_artifacts(tempdir.path())?;
    let artifact_manifest = tempdir.path().join("artifacts.json");
    fs::write(&artifact_manifest, serde_json::to_vec_pretty(&artifacts)?)?;
    let report_directory = tempdir.path().join("report-is-a-directory");
    fs::create_dir_all(&report_directory)?;
    let run_root = tempdir.path().join("runs");
    let config = E2eRunConfig {
        workspace_root: tempdir.path().to_path_buf(),
        run_root: Some(run_root.clone()),
        artifact_manifest: Some(artifact_manifest),
        report_path: Some(report_directory),
        build_artifacts: false,
        require_ctl: true,
        keep_artifacts: true,
        timeout: std::time::Duration::from_secs(10),
        base: E2eBaseMode::Hermetic,
        archive_rpc_url: None,
        iris_artifact: None,
        client: E2eClientMode::RustReference,
        seed: 1,
    };
    let (_sender, receiver) = watch::channel(false);
    let mut executor = ScriptedE2eExecutor::new(ScriptedPlan::Success);
    let error = E2eRunner::run(config, &mut executor, receiver)
        .await
        .expect_err("directory report path must fail");
    assert!(matches!(error, E2eRunnerError::ReportWrite { .. }));
    let run_dir = fs::read_dir(run_root)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .next()
        .expect("runner did not allocate a run directory");
    assert!(run_dir.join("shutdown").is_file());
    Ok(())
}

#[tokio::test]
async fn artifact_resolution_failure_still_writes_safe_partial_evidence() -> Result<()> {
    let tempdir = TempDir::new()?;
    let run_root = tempdir.path().join("runs");
    let config = E2eRunConfig {
        workspace_root: tempdir.path().to_path_buf(),
        run_root: Some(run_root.clone()),
        artifact_manifest: Some(tempdir.path().join("missing-artifacts.json")),
        report_path: None,
        build_artifacts: false,
        require_ctl: true,
        keep_artifacts: true,
        timeout: std::time::Duration::from_secs(10),
        base: E2eBaseMode::Hermetic,
        archive_rpc_url: None,
        iris_artifact: None,
        client: E2eClientMode::RustReference,
        seed: 2,
    };
    let (_sender, receiver) = watch::channel(false);
    let mut executor = ScriptedE2eExecutor::new(ScriptedPlan::Success);
    assert!(E2eRunner::run(config, &mut executor, receiver)
        .await
        .is_err());
    let run_dir = fs::read_dir(run_root)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .next()
        .expect("runner did not allocate a failure run directory");
    let report_path = run_dir.join("safe-evidence/report.json");
    let capsule = WithdrawalEvidenceCapsuleV1::from_json(&fs::read_to_string(report_path)?)?;
    assert_eq!(
        capsule.run.status,
        bridge_dev::evidence::EvidenceRunStatus::Failed
    );
    assert!(run_dir.join("safe-evidence/artifact-index.json").is_file());
    Ok(())
}

fn create_artifacts(root: &Path) -> Result<bridge_dev::artifacts::E2eArtifacts> {
    let bridge = root.join("bridge");
    let node = root.join("node");
    let miner = root.join("miner");
    let wallet = root.join("wallet");
    let ctl = root.join("ctl");
    let bridge_jam = root.join("bridge.jam");
    let roswell_jam = root.join("roswell.jam");
    let fakenet = root.join("fakenet.jam");
    for binary in [&bridge, &node, &miner, &wallet, &ctl] {
        write_binary(binary)?;
    }
    for jam in [&bridge_jam, &roswell_jam, &fakenet] {
        fs::write(jam, [1u8; 32])?;
    }
    let mut options = ArtifactResolveOptions::new(root.to_path_buf());
    options.require_ctl = true;
    options.overrides = ArtifactOverrides {
        bridge: Some(bridge),
        miner: Some(miner),
        wallet: Some(wallet),
        node: Some(node),
        sequencer_ctl: Some(ctl),
        bridge_jam: Some(bridge_jam),
        roswell_jam: Some(roswell_jam),
        fakenet_genesis_jam: Some(fakenet),
    };
    Ok(ArtifactResolver::resolve(&options)?)
}

fn write_binary(path: &Path) -> Result<()> {
    let mut header = [0u8; 32];
    let architecture = host_architecture();
    header[..4].copy_from_slice(&0xfeedfacfu32.to_le_bytes());
    let cpu = match architecture {
        BinaryArchitecture::Arm64 => 0x0100_000cu32,
        BinaryArchitecture::X86_64 => 0x0100_0007u32,
        BinaryArchitecture::Universal => 0,
    };
    header[4..8].copy_from_slice(&cpu.to_le_bytes());
    fs::write(path, header)?;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(target_arch = "aarch64")]
fn host_architecture() -> BinaryArchitecture {
    BinaryArchitecture::Arm64
}

#[cfg(target_arch = "x86_64")]
fn host_architecture() -> BinaryArchitecture {
    BinaryArchitecture::X86_64
}
