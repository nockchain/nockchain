use std::env;
use std::process::Command;

use chrono::{DateTime, Utc};

fn main() {
    // Get build timestamp
    let now: DateTime<Utc> = Utc::now();
    let build_timestamp = now.format("%Y.%-m.%-d..%-H.%-M.%-S").to_string();
    println!("cargo:rustc-env=BUILD_TIMESTAMP={}", build_timestamp);

    // Get git commit hash (short)
    let git_hash = get_git_hash().unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=GIT_HASH={}", git_hash);

    // Get build user (from environment)
    let build_user = env::var("USER")
        .or_else(|_| env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=BUILD_USER={}", build_user);

    // Get build host
    let build_host = get_hostname().unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=BUILD_HOST={}", build_host);

    // Create version string like your example: "0.1.0"
    let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.1.0".to_string());
    let full_version = format!("{}", version);
    println!("cargo:rustc-env=FULL_VERSION={}", full_version);

    // Tell cargo to re-run if git state changes
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");
}

fn get_git_hash() -> Option<String> {
    let output = Command::new("git")
        .args(&["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

fn get_hostname() -> Option<String> {
    // Try different methods to get hostname
    Command::new("hostname")
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
            } else {
                None
            }
        })
        .or_else(|| {
            // Fallback to environment variables
            env::var("HOSTNAME")
                .or_else(|_| env::var("COMPUTERNAME"))
                .ok()
        })
}
