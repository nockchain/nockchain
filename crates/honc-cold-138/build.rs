#[path = "../../../scripts/jam_asset_build.rs"]
mod jam_asset_build;

fn main() {
    println!("cargo:rerun-if-env-changed=HONC_COLD_138_ALLOW_MISSING");

    let missing = if std::env::var_os("HONC_COLD_138_ALLOW_MISSING").is_some() {
        jam_asset_build::MissingAsset::EmptyPlaceholder {
            placeholder_name: "honc-cold-138.missing.jam",
            warning: "honc-cold-138.jam is missing; using an empty compile-time placeholder for bootstrap. Run `make honk-cargo-build` or `bazel build //open/assets:honc_cold_138` to generate the cached cold-state asset.",
        }
    } else {
        jam_asset_build::MissingAsset::Fail
    };

    jam_asset_build::configure(
        "HONC_COLD_138_JAM_PATH", "open/assets/honc-cold-138.jam", missing,
    );
}
