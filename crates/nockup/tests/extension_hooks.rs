//! Integration tests for the three downstream-facing extension hooks
//! introduced in `extension-hooks`:
//!
//! * `template_git` / `template_path` in `nockapp.toml`
//! * `nockup-<subcmd>` plugin discovery
//!
//! The `NOCKUP_REGISTRY_URL` env-var override has its own test binary
//! (`tests/registry_override.rs`) because it touches the process-global
//! `ONLINE_REGISTRY` cache and would race with the other tests.

use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

/// Helper: initialize a real git repo at `path` with the given files,
/// commit, return the commit hash.
fn init_git_repo(path: &Path, files: &[(&str, &str)]) -> String {
    StdCommand::new("git")
        .arg("init")
        .arg("--quiet")
        .arg("-b")
        .arg("main")
        .current_dir(path)
        .status()
        .expect("git init");
    StdCommand::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(path)
        .status()
        .expect("git config email");
    StdCommand::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(path)
        .status()
        .expect("git config name");
    for (relpath, content) in files {
        let p = path.join(relpath);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, content).unwrap();
    }
    StdCommand::new("git")
        .args(["add", "-A"])
        .current_dir(path)
        .status()
        .expect("git add");
    StdCommand::new("git")
        .args(["commit", "--quiet", "-m", "init"])
        .current_dir(path)
        .status()
        .expect("git commit");
    let out = StdCommand::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(path)
        .output()
        .expect("git rev-parse");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn template_git_file_url_resolves() {
    // Set up: a temp git repo with `templates/demo/{Cargo.toml, hoon/app/app.hoon}`.
    let repo = TempDir::new().unwrap();
    let _commit = init_git_repo(
        repo.path(),
        &[
            ("templates/demo/Cargo.toml", "name = \"{{project_name}}\"\n"),
            ("templates/demo/hoon/app/app.hoon", "::  marker\n"),
        ],
    );

    // Set up: a working dir with a nockapp.toml pointing at the file:// URL.
    let workdir = TempDir::new().unwrap();
    let manifest = format!(
        "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\ntemplate = \"demo\"\ntemplate_git = \"file://{}\"\ntemplate_path = \"templates\"\n",
        repo.path().display()
    );
    fs::write(workdir.path().join("nockapp.toml"), manifest).unwrap();

    Command::cargo_bin("nockup")
        .unwrap()
        .arg("project")
        .arg("init")
        .current_dir(workdir.path())
        // Isolate the templates cache so the test doesn't pollute ~/.nockup.
        .env("HOME", workdir.path())
        .assert()
        .success();

    // Assert the scaffold landed under <workdir>/my-app/
    let scaffold = workdir.path().join("my-app");
    assert!(scaffold.join("Cargo.toml").exists());
    assert!(scaffold.join("hoon/app/app.hoon").exists());
    let cargo = fs::read_to_string(scaffold.join("Cargo.toml")).unwrap();
    assert!(
        cargo.contains("name = \"my-app\""),
        "handlebars rendered project_name: got {cargo:?}"
    );
}

#[test]
fn template_git_falls_back_when_unset() {
    // No template_git, no cache populated → init must error with the
    // existing "Template not found" message and not panic.
    let workdir = TempDir::new().unwrap();
    fs::write(
        workdir.path().join("nockapp.toml"),
        "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\ntemplate = \"nonexistent-template\"\n",
    )
    .unwrap();

    Command::cargo_bin("nockup")
        .unwrap()
        .arg("project")
        .arg("init")
        .current_dir(workdir.path())
        .env("HOME", workdir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn plugin_discovery_executes_path_binary() {
    // Drop a `nockup-greet` shell script into a tempdir, prepend that dir
    // to PATH, invoke `nockup greet hello`, assert the binary ran and
    // its exit code propagated.
    let plugin_dir = TempDir::new().unwrap();
    let script = plugin_dir.path().join("nockup-greet");
    fs::write(
        &script,
        "#!/bin/sh\necho \"plugin says: $1\"\nexit 0\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();
    }

    let path_var = format!(
        "{}:{}",
        plugin_dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    Command::cargo_bin("nockup")
        .unwrap()
        .arg("greet")
        .arg("hello")
        .env("PATH", &path_var)
        .assert()
        .success()
        .stdout(predicate::str::contains("plugin says: hello"));
}

#[test]
fn plugin_discovery_missing_binary_errors() {
    // Empty PATH → plugin lookup fails with the structured "unknown
    // command" message, exit code non-zero.
    Command::cargo_bin("nockup")
        .unwrap()
        .arg("nonexistent-cmd")
        .env("PATH", "/nonexistent")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown command"));
}
