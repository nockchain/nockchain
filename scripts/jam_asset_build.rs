use std::path::{Path, PathBuf};
use std::{env, fs};

#[allow(dead_code)]
pub enum MissingAsset {
    Fail,
    EmptyPlaceholder {
        placeholder_name: &'static str,
        warning: &'static str,
    },
}

pub fn configure(env_var: &str, relative_asset_path: &str, missing: MissingAsset) {
    println!("cargo:rerun-if-env-changed={env_var}");

    if let Some(path) = env::var_os(env_var) {
        let path = PathBuf::from(path);
        println!("cargo:rerun-if-changed={}", path.display());
        println!("cargo:rustc-env={env_var}={}", path.display());
        return;
    }

    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let repo_root = find_repo_root(&manifest_dir);
    let path = resolve_asset_path(&repo_root, relative_asset_path);

    println!("cargo:rerun-if-changed={}", path.display());

    if path.is_file() {
        println!("cargo:rustc-env={env_var}={}", path.display());
        return;
    }

    match missing {
        MissingAsset::Fail => panic!(
            "required jam asset is missing: {} (resolved from {relative_asset_path})",
            path.display()
        ),
        MissingAsset::EmptyPlaceholder {
            placeholder_name,
            warning,
        } => {
            println!("cargo:warning={warning}");
            let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
            let placeholder = out_dir.join(placeholder_name);
            fs::write(&placeholder, []).expect("write empty jam placeholder");
            println!("cargo:rustc-env={env_var}={}", placeholder.display());
        }
    }
}

fn find_repo_root(manifest_dir: &Path) -> PathBuf {
    for ancestor in manifest_dir.ancestors() {
        if ancestor.join("Cargo.lock").is_file() {
            return ancestor.to_path_buf();
        }
    }
    manifest_dir.to_path_buf()
}

fn resolve_asset_path(repo_root: &Path, relative_asset_path: &str) -> PathBuf {
    let direct = repo_root.join(relative_asset_path);
    if direct.exists() {
        return direct;
    }

    if let Some(stripped) = relative_asset_path.strip_prefix("open/") {
        let stripped = repo_root.join(stripped);
        if stripped.exists() || !repo_root.join("open").exists() {
            return stripped;
        }
    }

    direct
}
