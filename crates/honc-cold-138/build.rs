#[path = "../../scripts/jam_asset_build.rs"]
mod jam_asset_build;

fn main() {
    let missing = jam_asset_build::MissingAsset::EmptyPlaceholder {
        placeholder_name: "honc-cold-138.missing.jam",
        warning: "honc-cold-138.jam is missing; using an empty compile-time placeholder for bootstrap. Run `just build-honk-assets`, `make build-honk-assets`, or `bazel build //assets:honc_cold_138` to generate the cached cold-state asset.",
    };

    jam_asset_build::configure(
        "HONC_COLD_138_JAM_PATH", "open/assets/honc-cold-138.jam", missing,
    );
}
