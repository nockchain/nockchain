use std::error::Error;
use std::path::PathBuf;
use std::{env, fs, io};

fn main() -> Result<(), Box<dyn Error>> {
    let manifest_dir = env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("CARGO_MANIFEST_DIR is not set"))?;
    let hoon_source = manifest_dir.join("../../hoon/common/hoon.hoon");
    let honc_type_asset = manifest_dir.join("assets/honc-type-138.jam");
    let honc_formula_asset = manifest_dir.join("assets/honc-formula-138.jam");
    let hoonc_octs_type_asset = manifest_dir.join("assets/hoonc-octs-type-138.jam");

    let allow_missing_hoonc_octs = env_flag("HONK_HOONC_OCTS_TYPE_138_ALLOW_MISSING");
    let hoonc_octs_type_asset_path = if hoonc_octs_type_asset.exists() {
        hoonc_octs_type_asset.clone()
    } else if allow_missing_hoonc_octs {
        let out_dir = env::var_os("OUT_DIR")
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("OUT_DIR is not set"))?;
        let placeholder = out_dir.join("hoonc-octs-type-138.missing.jam");
        fs::write(&placeholder, [])?;
        println!(
            "cargo:warning=assets/hoonc-octs-type-138.jam is missing; using an empty compile-time placeholder because HONK_HOONC_OCTS_TYPE_138_ALLOW_MISSING is set. Data-import compilation will require `bazel build //open/crates/honk:hoonc_octs_type_138` or `make build-honk-assets`."
        );
        placeholder
    } else {
        return Err(io::Error::other(format!(
            "missing {}; run `make build-honk-assets` or set HONK_HOONC_OCTS_TYPE_138_ALLOW_MISSING=1 for bootstrap/cargo-only diagnostics that do not compile data imports",
            hoonc_octs_type_asset.display()
        ))
        .into());
    };

    for path in [&hoon_source, &honc_type_asset, &honc_formula_asset, &hoonc_octs_type_asset] {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!(
        "cargo:rustc-env=HONK_HOON_138_SOURCE={}",
        hoon_source.display()
    );
    println!(
        "cargo:rustc-env=HONK_HONC_TYPE_138_JAM={}",
        honc_type_asset.display()
    );
    println!(
        "cargo:rustc-env=HONK_HONC_FORMULA_138_JAM={}",
        honc_formula_asset.display()
    );
    println!(
        "cargo:rustc-env=HONK_HOONC_OCTS_TYPE_138_JAM={}",
        hoonc_octs_type_asset_path.display()
    );
    Ok(())
}

fn env_flag(name: &str) -> bool {
    env::var(name)
        .ok()
        .map(|value| {
            let value = value.trim().to_ascii_lowercase();
            !(value.is_empty() || value == "0" || value == "false" || value == "off")
        })
        .unwrap_or(false)
}
