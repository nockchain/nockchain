use std::env;
use std::error::Error;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let assets_dir = manifest_dir.join("../../assets");

    println!(
        "cargo:rustc-env=DUMB_JAM_PATH={}",
        assets_dir.join("dumb.jam").display()
    );
    println!(
        "cargo:rustc-env=WALLET_JAM_PATH={}",
        assets_dir.join("wal.jam").display()
    );
    println!(
        "cargo:rustc-env=MINER_JAM_PATH={}",
        assets_dir.join("miner.jam").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        assets_dir.join("dumb.jam").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        assets_dir.join("wal.jam").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        assets_dir.join("miner.jam").display()
    );

    Ok(())
}
