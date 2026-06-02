use std::path::Path;
use std::process::Command;

#[test]
fn bytehound_binary_identity_reports_bytehound_profile() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());

    let build = Command::new(&cargo)
        .args(["build", "-p", "nockchain-bench", "--profile", "bytehound"])
        .current_dir(&repo_root)
        .output()
        .expect("build bytehound binary");
    assert!(build.status.success(), "{build:?}");

    let binary = repo_root.join("target/bytehound/nockchain-bench");
    let output = Command::new(&binary)
        .args(["sol", "binary-identity"])
        .current_dir(&repo_root)
        .output()
        .expect("run bytehound binary-identity");
    assert!(output.status.success(), "{output:?}");

    let identity: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse binary identity json");
    assert_eq!(identity["build_profile"], "bytehound");
}
