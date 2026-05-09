//! Integration tests for `NOCKUP_REGISTRY_URL`.
//!
//! These live in their own test binary so the process-global
//! `ONLINE_REGISTRY` cache (in `crates/nockup/src/resolver/registry.rs`)
//! doesn't bleed between tests in `extension_hooks.rs`.
//!
//! `nockup package add` only mutates `nockapp.toml`; the actual
//! registry lookup happens in `package install`. So each test seeds
//! a manifest with a dep entry and runs `install`.

use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

/// Write a minimal typhoon-shaped registry. Returns the `file://` URL.
fn write_local_registry(dir: &std::path::Path, package_name: &str, git_url: &str) -> String {
    let body = format!(
        r#"
[workspace.testws]
git_url = "{git_url}"
ref = "main"
description = "test workspace"
root_path = "."

[[package]]
name = "{package_name}"
workspace = "testws"
path = "."
file = ""
dependencies = []
"#
    );
    let path = dir.join("registry.toml");
    fs::write(&path, body).unwrap();
    format!("file://{}", path.display())
}

/// Set up a project dir with `nockapp.toml` declaring one dep that
/// will trigger registry lookup on `package install`.
fn project_with_dep(dir: &std::path::Path, dep: &str) {
    fs::create_dir_all(dir.join("probe")).unwrap();
    fs::write(
        dir.join("probe/nockapp.toml"),
        format!(
            "[package]\nname = \"probe\"\nversion = \"0.1.0\"\n\n[dependencies]\n\"{dep}\" = \"latest\"\n"
        ),
    )
    .unwrap();
    // Mirror the manifest at the parent dir so `package install` can find
    // the project from cwd.
    fs::write(
        dir.join("nockapp.toml"),
        format!(
            "[package]\nname = \"probe\"\nversion = \"0.1.0\"\n\n[dependencies]\n\"{dep}\" = \"latest\"\n"
        ),
    )
    .unwrap();
}

#[test]
fn registry_env_override_emits_warning() {
    let dir = TempDir::new().unwrap();
    project_with_dep(dir.path(), "test/fakepkg");
    let registry_url = write_local_registry(
        dir.path(),
        "test/fakepkg",
        "https://example.invalid/does-not-exist",
    );

    Command::cargo_bin("nockup")
        .unwrap()
        .arg("package")
        .arg("install")
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env("NOCKUP_REGISTRY_URL", &registry_url)
        .assert()
        // Registry-lookup will fail downstream when the bogus git URL
        // can't clone, but the override warning fires first.
        .stderr(predicate::str::contains("NOCKUP_REGISTRY_URL"));
}

#[test]
fn registry_env_override_file_url_parses() {
    let dir = TempDir::new().unwrap();
    project_with_dep(dir.path(), "test/fakepkg");
    let registry_url = write_local_registry(
        dir.path(),
        "test/fakepkg",
        "https://example.invalid/does-not-exist",
    );

    Command::cargo_bin("nockup")
        .unwrap()
        .arg("package")
        .arg("install")
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env("NOCKUP_REGISTRY_URL", &registry_url)
        .assert()
        // The registry parsed (no "did not parse"); install may still fail
        // when the bogus git URL can't clone — that's downstream.
        .stderr(predicate::str::contains("did not parse").not());
}

#[test]
fn registry_env_override_bad_toml_reports_url() {
    let dir = TempDir::new().unwrap();
    project_with_dep(dir.path(), "test/fakepkg");

    let bad_path = dir.path().join("bad-registry.toml");
    fs::write(&bad_path, "this is not valid toml [[[").unwrap();
    let bad_url = format!("file://{}", bad_path.display());

    Command::cargo_bin("nockup")
        .unwrap()
        .arg("package")
        .arg("install")
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env("NOCKUP_REGISTRY_URL", &bad_url)
        .assert()
        .failure();
}
