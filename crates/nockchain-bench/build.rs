use std::env;
use std::path::Path;
use std::process::Command;

#[path = "build_support.rs"]
mod build_support;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    configure_release_binary_layout();
    println!(
        "cargo:rustc-env=NOCKCHAIN_BENCH_BUILD_PROFILE={}",
        cargo_build_profile()
    );
    if let Some(head_path) = git_path("HEAD") {
        let head_ref_path = head_ref().as_deref().and_then(git_path_ref);
        for path in build_support::tracked_git_watch_paths(
            &head_path,
            &git_path("packed-refs").unwrap_or_default(),
            head_ref_path.as_deref(),
        ) {
            if !path.is_empty() {
                println!("cargo:rerun-if-changed={path}");
            }
        }
    }

    let git_commit = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|commit| commit.trim().to_string())
        .filter(|commit| !commit.is_empty())
        .unwrap_or_default();

    println!("cargo:rustc-env=NOCKCHAIN_BENCH_GIT_COMMIT={git_commit}");
}

fn configure_release_binary_layout() {
    let profile = env::var("PROFILE").unwrap_or_default();
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    for link_arg in build_support::release_binary_link_args(&profile, &target_os, &target_env) {
        println!("cargo:rustc-link-arg-bin=nockchain-bench={link_arg}");
    }
}

fn cargo_build_profile() -> String {
    env::var("OUT_DIR")
        .ok()
        .and_then(|out_dir| {
            Path::new(&out_dir)
                .ancestors()
                .nth(3)
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .filter(|profile| !profile.trim().is_empty())
        .or_else(|| {
            env::var("PROFILE")
                .ok()
                .filter(|profile| !profile.trim().is_empty())
        })
        .unwrap_or_default()
}

fn git_path(path: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-path", path])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let resolved_path = String::from_utf8(output.stdout).ok()?;
    let resolved_path = resolved_path.trim();
    if resolved_path.is_empty() {
        None
    } else {
        Some(resolved_path.to_string())
    }
}

fn head_ref() -> Option<String> {
    let output = Command::new("git")
        .args(["symbolic-ref", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let head_ref = String::from_utf8(output.stdout).ok()?;
    let head_ref = head_ref.trim();
    if head_ref.is_empty() {
        None
    } else {
        Some(head_ref.to_string())
    }
}

fn git_path_ref(path: &str) -> Option<String> {
    git_path(path)
}
