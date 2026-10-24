use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let jam_path = manifest_dir.join("../../../../assets/roswell.jam");

    println!("cargo:rerun-if-env-changed=KERNEL_JAM_PATH");
    println!("cargo:rerun-if-changed={}", jam_path.display());

    if env::var_os("KERNEL_JAM_PATH").is_none() {
        println!("cargo:rustc-env=KERNEL_JAM_PATH={}", jam_path.display());
    }
}
