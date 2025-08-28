use std::env;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/nockapp.proto");

    // Get the output directory
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);

    // For tonic-build 0.14.x, use the compile method
    tonic_prost_build::configure()
        .file_descriptor_set_path(out_dir.join("nockapp_descriptor.bin"))
        .compile_protos(&["proto/nockapp.proto"], &["proto"])?;

    Ok(())
}
