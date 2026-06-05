use std::env;
use std::error::Error;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let dumb_jam = manifest_dir.join("../../assets/dumb.jam");

    println!("cargo:rustc-env=DUMB_JAM_PATH={}", dumb_jam.display());
    println!("cargo:rerun-if-changed={}", dumb_jam.display());
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("migrations").display()
    );

    Ok(())
}
