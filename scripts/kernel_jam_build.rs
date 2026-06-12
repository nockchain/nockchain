#[path = "jam_asset_build.rs"]
mod jam_asset_build;

pub fn configure(relative_asset_path: &'static str) {
    jam_asset_build::configure(
        "KERNEL_JAM_PATH",
        relative_asset_path,
        jam_asset_build::MissingAsset::Fail,
    );
}
