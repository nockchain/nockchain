use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::Result;
use bridge_dev::artifacts::{
    ArtifactBuildCommand, ArtifactOverrides, ArtifactResolveOptions, ArtifactResolver,
    BinaryArchitecture,
};
use tempfile::TempDir;

#[test]
fn resolves_and_hashes_explicit_artifacts() -> Result<()> {
    let tempdir = TempDir::new()?;
    let fixture = ArtifactFixture::create(tempdir.path().join("explicit"), Layout::Cargo, true)?;
    let mut options = ArtifactResolveOptions::new(tempdir.path().join("unused"));
    options.require_ctl = true;
    options.overrides = fixture.overrides();

    let first = ArtifactResolver::resolve(&options)?;
    let second = ArtifactResolver::resolve(&options)?;
    assert_eq!(first, second);
    assert_eq!(first.bridge.sha256.len(), 64);
    assert_eq!(first.bridge.architecture, Some(host_architecture()));
    assert_eq!(first.bridge_jam.size_bytes, 32);
    assert_eq!(
        first.sequencer_ctl.as_ref().map(|file| &file.path),
        Some(&fixture.ctl)
    );
    assert!(first
        .environment_overrides()
        .iter()
        .all(|(_, value)| !value.is_empty()));
    Ok(())
}

#[test]
fn autodiscovers_cargo_and_bazel_layouts() -> Result<()> {
    for layout in [Layout::Cargo, Layout::Bazel] {
        let tempdir = TempDir::new()?;
        let fixture = ArtifactFixture::create(tempdir.path().join("workspace"), layout, false)?;
        let options = ArtifactResolveOptions::new(fixture.root.clone());
        let artifacts = ArtifactResolver::resolve(&options)?;
        assert_eq!(artifacts.bridge.path, fixture.bridge);
        assert_eq!(artifacts.node.path, fixture.node);
        assert!(artifacts.sequencer_ctl.is_none());
        assert_eq!(artifacts.fakenet_genesis_jam.path, fixture.fakenet);
    }
    Ok(())
}

#[test]
fn aggregates_missing_and_invalid_artifacts_with_one_remediation() -> Result<()> {
    let tempdir = TempDir::new()?;
    let root = tempdir.path().join("workspace");
    let release = root.join("target/release");
    fs::create_dir_all(&release)?;
    let bridge = release.join("bridge");
    write_binary(&bridge, opposite_architecture(), false)?;
    fs::create_dir_all(root.join("assets"))?;
    fs::write(root.join("assets/bridge.jam"), [])?;
    let mut options = ArtifactResolveOptions::new(root);
    options.require_ctl = true;

    let error = ArtifactResolver::resolve(&options).expect_err("invalid fixture must fail");
    let rendered = error.to_string();
    for label in [
        "bridge binary", "sequencer/node binary", "sequencer ctl binary", "bridge jam",
        "roswell jam", "fakenet genesis jam",
    ] {
        assert!(rendered.contains(label), "missing aggregate entry {label}");
    }
    assert!(rendered.contains("not executable") || rendered.contains("architecture"));
    assert!(rendered.contains("file is empty"));
    assert!(rendered.contains("remediation: bazel build"));
    Ok(())
}

#[test]
fn build_option_runs_one_command_then_resolves_outputs() -> Result<()> {
    let tempdir = TempDir::new()?;
    let root = tempdir.path().join("workspace");
    fs::create_dir_all(&root)?;
    let templates = tempdir.path().join("templates");
    fs::create_dir_all(&templates)?;
    let binary_template = templates.join("binary");
    let jam_template = templates.join("jam");
    write_binary(&binary_template, host_architecture(), true)?;
    fs::write(&jam_template, [7u8; 32])?;
    let builder = tempdir.path().join("build-artifacts.sh");
    fs::write(
        &builder,
        format!(
            "#!/bin/sh\nset -eu\nprintf run >> build-count\nmkdir -p target/release assets crates/nockchain/jams\ncp '{}' target/release/bridge\ncp '{}' target/release/nockchain-bridge-sequencer\ncp '{}' target/release/nockchain-bridge-sequencer-ctl\ncp '{}' assets/bridge.jam\ncp '{}' assets/roswell.jam\ncp '{}' crates/nockchain/jams/fakenet-genesis-pow-2-bex-1.jam\n",
            binary_template.display(),
            binary_template.display(),
            binary_template.display(),
            jam_template.display(),
            jam_template.display(),
            jam_template.display(),
        ),
    )?;
    make_executable(&builder)?;
    let mut options = ArtifactResolveOptions::new(root.clone());
    options.require_ctl = true;
    options.build = true;
    options.build_command = ArtifactBuildCommand {
        program: builder,
        args: Vec::new(),
    };

    let artifacts = ArtifactResolver::resolve(&options)?;
    assert!(artifacts.bridge.path.is_file());
    assert!(artifacts.sequencer_ctl.is_some());
    assert_eq!(fs::read_to_string(root.join("build-count"))?, "run");
    Ok(())
}

#[derive(Clone, Copy)]
enum Layout {
    Cargo,
    Bazel,
}

struct ArtifactFixture {
    root: PathBuf,
    bridge: PathBuf,
    node: PathBuf,
    ctl: PathBuf,
    bridge_jam: PathBuf,
    roswell_jam: PathBuf,
    fakenet: PathBuf,
}

impl ArtifactFixture {
    fn create(root: PathBuf, layout: Layout, include_ctl: bool) -> Result<Self> {
        let (bridge, node, ctl, bridge_jam, roswell_jam) = match layout {
            Layout::Cargo => (
                root.join("target/release/bridge"),
                root.join("target/release/nockchain-bridge-sequencer"),
                root.join("target/release/nockchain-bridge-sequencer-ctl"),
                root.join("assets/bridge.jam"),
                root.join("assets/roswell.jam"),
            ),
            Layout::Bazel => (
                root.join("bazel-bin/crates/bridge/bridge-bin"),
                root.join("bazel-bin/crates/nockchain-bridge-sequencer/nockchain-bridge-sequencer"),
                root.join(
                    "bazel-bin/crates/nockchain-bridge-sequencer/nockchain-bridge-sequencer-ctl",
                ),
                root.join("bazel-bin/assets/bridge.jam"),
                root.join("bazel-bin/assets/roswell.jam"),
            ),
        };
        let fakenet = root.join("crates/nockchain/jams/fakenet-genesis-pow-2-bex-1.jam");
        write_binary(&bridge, host_architecture(), true)?;
        write_binary(&node, host_architecture(), true)?;
        if include_ctl {
            write_binary(&ctl, host_architecture(), true)?;
        }
        write_jam(&bridge_jam, 1)?;
        write_jam(&roswell_jam, 2)?;
        write_jam(&fakenet, 3)?;
        Ok(Self {
            root,
            bridge,
            node,
            ctl,
            bridge_jam,
            roswell_jam,
            fakenet,
        })
    }

    fn overrides(&self) -> ArtifactOverrides {
        ArtifactOverrides {
            bridge: Some(self.bridge.clone()),
            node: Some(self.node.clone()),
            sequencer_ctl: Some(self.ctl.clone()),
            bridge_jam: Some(self.bridge_jam.clone()),
            roswell_jam: Some(self.roswell_jam.clone()),
            fakenet_genesis_jam: Some(self.fakenet.clone()),
        }
    }
}

fn write_jam(path: &Path, byte: u8) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, [byte; 32])?;
    Ok(())
}

fn write_binary(path: &Path, architecture: BinaryArchitecture, executable: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut header = [0u8; 32];
    match architecture {
        BinaryArchitecture::Universal => {
            header[..4].copy_from_slice(&0xcafebabeu32.to_be_bytes());
        }
        BinaryArchitecture::Arm64 => {
            header[..4].copy_from_slice(&0xfeedfacfu32.to_le_bytes());
            header[4..8].copy_from_slice(&0x0100_000cu32.to_le_bytes());
        }
        BinaryArchitecture::X86_64 => {
            header[..4].copy_from_slice(&0xfeedfacfu32.to_le_bytes());
            header[4..8].copy_from_slice(&0x0100_0007u32.to_le_bytes());
        }
    }
    fs::write(path, header)?;
    if executable {
        make_executable(path)?;
    }
    Ok(())
}

fn make_executable(path: &Path) -> Result<()> {
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

fn opposite_architecture() -> BinaryArchitecture {
    match host_architecture() {
        BinaryArchitecture::Arm64 => BinaryArchitecture::X86_64,
        BinaryArchitecture::X86_64 => BinaryArchitecture::Arm64,
        BinaryArchitecture::Universal => BinaryArchitecture::Arm64,
    }
}
