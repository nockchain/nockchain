use std::error::Error;
use std::path::PathBuf;
use std::{env, fs, io};

fn collect_files(path: &std::path::Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    if path.is_file() {
        files.push(path.to_path_buf());
        return Ok(());
    }
    for entry in fs::read_dir(path)? {
        collect_files(&entry?.path(), files)?;
    }
    Ok(())
}

fn compiler_fingerprint(manifest_dir: &std::path::Path) -> io::Result<String> {
    let repository = manifest_dir.join("../..").canonicalize()?;
    let roots = [
        manifest_dir.join("src"),
        manifest_dir.join("build.rs"),
        manifest_dir.join("Cargo.toml"),
        manifest_dir.join("../hatch/src"),
        manifest_dir.join("../nockapp/src/noun"),
        manifest_dir.join("../nockvm/rust/nockvm/src"),
        repository.join("Cargo.lock"),
        repository.join("Cargo.toml"),
    ];
    let mut files = Vec::new();
    for root in &roots {
        println!("cargo:rerun-if-changed={}", root.display());
        collect_files(root, &mut files)?;
    }
    files.sort();
    let mut left = 0xcbf2_9ce4_8422_2325u64;
    let mut right = 0x8422_2325_cbf2_9ce4u64;
    for file in files {
        let relative = file.strip_prefix(&repository).unwrap_or(&file);
        for byte in relative.to_string_lossy().bytes().chain([0]) {
            left = (left ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
            right = (right ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
            right = right.rotate_left(7);
        }
        for byte in fs::read(&file)? {
            left = (left ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
            right = (right ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
            right = right.rotate_left(7);
        }
    }
    Ok(format!("{left:016x}{right:016x}"))
}

fn main() -> Result<(), Box<dyn Error>> {
    let manifest_dir = env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("CARGO_MANIFEST_DIR is not set"))?;
    let hoon_source = manifest_dir.join("../../hoon/common/hoon.hoon");
    let honc_type_asset = manifest_dir.join("assets/honc-type-138.jam");
    let honc_formula_asset = manifest_dir.join("assets/honc-formula-138.jam");
    let hoonc_octs_type_asset = manifest_dir.join("assets/hoonc-octs-type-138.jam");
    println!(
        "cargo:rustc-env=HONK_NATIVE_COMPILER_FINGERPRINT={}",
        compiler_fingerprint(&manifest_dir)?
    );

    // The canonical hoonc `$octs` type is a required parity input: data
    // imports (`/*`) must vase their bytes with hoonc's `$octs` hold, not a
    // local `[p=@ud q=@]` approximation. Refuse to build without it rather
    // than embedding an empty placeholder that silently degrades `/*` output.
    // Regenerate with `just hoonc-octs-type-138-asset` (or the Bazel target).
    // Bootstrap builds that genuinely lack the checked-in asset may point
    // HONK_HOONC_OCTS_TYPE_138_JAM_OVERRIDE at an alternate jam.
    println!("cargo:rerun-if-env-changed=HONK_HOONC_OCTS_TYPE_138_JAM_OVERRIDE");
    let hoonc_octs_type_asset_path = if let Some(override_path) =
        env::var_os("HONK_HOONC_OCTS_TYPE_138_JAM_OVERRIDE")
    {
        let path = PathBuf::from(override_path);
        if !path.is_file() {
            return Err(io::Error::other(format!(
                "HONK_HOONC_OCTS_TYPE_138_JAM_OVERRIDE points at a missing file: {}",
                path.display()
            ))
            .into());
        }
        path.canonicalize()?
    } else if hoonc_octs_type_asset.is_file() && fs::metadata(&hoonc_octs_type_asset)?.len() > 0 {
        hoonc_octs_type_asset.clone()
    } else {
        return Err(io::Error::other(
            "crates/honk/assets/hoonc-octs-type-138.jam is missing or empty; \
                 regenerate it with `just hoonc-octs-type-138-asset` (data imports \
                 require hoonc's canonical $octs type), or set \
                 HONK_HOONC_OCTS_TYPE_138_JAM_OVERRIDE to an alternate path",
        )
        .into());
    };

    for path in [&hoon_source, &honc_type_asset, &honc_formula_asset, &hoonc_octs_type_asset_path] {
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
