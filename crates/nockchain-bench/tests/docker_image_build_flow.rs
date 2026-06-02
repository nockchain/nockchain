use std::fs;
use std::path::Path;
use std::process::Command;

use nockchain_bench::speed_of_light::harness::docker_image::{
    docker_auto_build_command, DockerImageVariant,
};

const PLACEHOLDER_BINARY: &[u8] = b"placeholder";

fn script_path() -> &'static str {
    "../../scripts/build_nockchain_bench_image.sh"
}

fn prepend_path(dir: &std::path::Path) -> std::ffi::OsString {
    let mut combined = std::ffi::OsString::new();
    combined.push(dir.as_os_str());
    combined.push(":");
    combined.push(std::env::var_os("PATH").expect("PATH"));
    combined
}

fn write_placeholder(path: &Path) {
    fs::write(path, PLACEHOLDER_BINARY).expect("write placeholder binary");
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).expect("write fake command");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path)
            .expect("fake command metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod fake command");
    }
}

#[test]
fn build_image_script_help_mentions_standard_and_profiling_variants() {
    let output = Command::new(script_path())
        .arg("--help")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--tag"));
    assert!(stdout.contains("--variant"));
    assert!(stdout.contains("standard"));
    assert!(stdout.contains("profiling"));
}

#[test]
fn profiling_variant_requires_samply_or_explicit_override() {
    let empty_path = tempfile::tempdir().expect("tempdir");
    let binary_dir = tempfile::tempdir().expect("binary tempdir");
    let binary_path = binary_dir.path().join("nockchain-bench");
    write_placeholder(&binary_path);

    let output = Command::new(script_path())
        .args([
            "--variant",
            "profiling",
            "--tag",
            "example:test",
            "--dry-run",
            "--skip-cargo-build",
            "--binary",
            binary_path.to_str().expect("binary path utf-8"),
        ])
        .env("PATH", empty_path.path())
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run script");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("samply"));
}

#[test]
fn standard_variant_completes_successfully_with_mocked_docker() {
    let bin_dir = tempfile::tempdir().expect("bin tempdir");
    let docker_path = bin_dir.path().join("docker");
    write_executable(
        &docker_path, "#!/bin/sh\nprintf 'docker %s\\n' \"$*\" > \"$MOCK_DOCKER_LOG\"\n",
    );

    let binary_dir = tempfile::tempdir().expect("binary tempdir");
    let binary_path = binary_dir.path().join("nockchain-bench");
    write_placeholder(&binary_path);

    let log_path = binary_dir.path().join("docker.log");
    let output = Command::new(script_path())
        .args([
            "--variant",
            "standard",
            "--tag",
            "example:test",
            "--skip-cargo-build",
            "--binary",
            binary_path.to_str().expect("binary path utf-8"),
        ])
        .env("PATH", prepend_path(bin_dir.path()))
        .env("MOCK_DOCKER_LOG", &log_path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run script");

    assert!(output.status.success(), "{output:?}");
    let docker_log = fs::read_to_string(log_path).expect("read docker log");
    assert!(docker_log.contains("build -t example:test"));
}

#[test]
fn profiling_variant_stages_samply_and_uses_profiling_dockerfile() {
    let bin_dir = tempfile::tempdir().expect("bin tempdir");
    let docker_path = bin_dir.path().join("docker");
    write_executable(
        &docker_path,
        "#!/bin/sh\nfor last do :; done\ncontext=\"$last\"\n[ -f \"$context/samply\" ] || { echo 'missing staged samply' >&2; exit 1; }\nprintf 'docker %s\\n' \"$*\" > \"$MOCK_DOCKER_LOG\"\n",
    );

    let binary_dir = tempfile::tempdir().expect("binary tempdir");
    let binary_path = binary_dir.path().join("nockchain-bench");
    let samply_path = binary_dir.path().join("samply");
    write_placeholder(&binary_path);
    write_placeholder(&samply_path);

    let log_path = binary_dir.path().join("docker.log");
    let output = Command::new(script_path())
        .args([
            "--variant",
            "profiling",
            "--tag",
            "example:test",
            "--skip-cargo-build",
            "--binary",
            binary_path.to_str().expect("binary path utf-8"),
            "--samply-bin",
            samply_path.to_str().expect("samply path utf-8"),
        ])
        .env("PATH", prepend_path(bin_dir.path()))
        .env("MOCK_DOCKER_LOG", &log_path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run script");

    assert!(output.status.success(), "{output:?}");
    let docker_log = fs::read_to_string(log_path).expect("read docker log");
    assert!(docker_log.contains("Dockerfile.profiling"));
}

#[test]
fn profiling_variant_builds_bytehound_binary_by_default() {
    let bin_dir = tempfile::tempdir().expect("bin tempdir");
    let cargo_path = bin_dir.path().join("cargo");
    let docker_path = bin_dir.path().join("docker");
    write_executable(
        &cargo_path, "#!/bin/sh\nprintf 'cargo %s\\n' \"$*\" > \"$MOCK_CARGO_LOG\"\n",
    );
    write_executable(
        &docker_path, "#!/bin/sh\nprintf 'docker %s\\n' \"$*\" > \"$MOCK_DOCKER_LOG\"\n",
    );

    let binary_dir = tempfile::tempdir().expect("binary tempdir");
    let binary_path = binary_dir.path().join("nockchain-bench");
    let samply_path = binary_dir.path().join("samply");
    write_placeholder(&binary_path);
    write_placeholder(&samply_path);

    let cargo_log = binary_dir.path().join("cargo.log");
    let docker_log = binary_dir.path().join("docker.log");
    let output = Command::new(script_path())
        .args([
            "--variant",
            "profiling",
            "--tag",
            "example:test",
            "--binary",
            binary_path.to_str().expect("binary path utf-8"),
            "--samply-bin",
            samply_path.to_str().expect("samply path utf-8"),
        ])
        .env("CARGO", &cargo_path)
        .env("PATH", prepend_path(bin_dir.path()))
        .env("MOCK_CARGO_LOG", &cargo_log)
        .env("MOCK_DOCKER_LOG", &docker_log)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run script");

    assert!(output.status.success(), "{output:?}");
    let cargo_log = fs::read_to_string(cargo_log).expect("read cargo log");
    assert!(cargo_log.contains("build -p nockchain-bench --profile bytehound"));
}

#[test]
fn standard_auto_build_packages_invoking_binary_without_rebuilding() {
    let current_exe = Path::new("/tmp/pma-phase3-docker-check/target/release/nockchain-bench");

    let command = docker_auto_build_command(
        "nockchain-bench:local",
        DockerImageVariant::Standard,
        current_exe,
    );

    assert_eq!(
        command.args,
        vec![
            "--variant", "standard", "--tag", "nockchain-bench:local", "--binary",
            "/tmp/pma-phase3-docker-check/target/release/nockchain-bench", "--skip-cargo-build",
        ]
    );
}
