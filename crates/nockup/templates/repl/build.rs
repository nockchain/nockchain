use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=hoon/app/app.hoon");
    println!("cargo:rerun-if-changed=hoon/lib/lib.hoon");

    let out_dir = env::var("OUT_DIR").unwrap();
    let hoon_app_file = "hoon/app/app.hoon";

    if Path::new(hoon_app_file).exists() {
        // Compile Hoon to Nock (this is a placeholder - adjust based on actual hoonc usage)
        let output = Command::new("hoonc")
            .args(&[hoon_app_file, "--output", &format!("{}/app.nock", out_dir)])
            .output();

        match output {
            Ok(result) => {
                if !result.status.success() {
                    panic!(
                        "Failed to compile Hoon: {}",
                        String::from_utf8_lossy(&result.stderr)
                    );
                }
                println!("cargo:rustc-env=COMPILED_HOON_PATH={}/app.nock", out_dir);
            }
            Err(e) => {
                println!(
                    "cargo:warning=Could not run hoonc: {}. Skipping Hoon compilation.",
                    e
                );
            }
        }
    } else {
        println!(
            "cargo:warning=No Hoon file found at {}. Skipping Hoon compilation.",
            hoon_app_file
        );
    }
}
