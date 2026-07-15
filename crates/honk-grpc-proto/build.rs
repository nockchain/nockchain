#![allow(dead_code)]

use std::env;
use std::path::PathBuf;

fn ensure_protoc() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-env-changed=PROTOC");
    println!("cargo:rerun-if-env-changed=PATH");
    if let Some(protoc) = env::var_os("PROTOC") {
        let path = PathBuf::from(protoc);
        if !path.is_file() {
            return Err(format!("PROTOC is set but not a file: {}", path.display()).into());
        }
        return Ok(());
    }
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    env::set_var("PROTOC", protoc);
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    ensure_protoc()?;
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let proto_files: Vec<_> = glob::glob("proto/**/*.proto")?
        .filter_map(Result::ok)
        .collect();
    for proto_file in &proto_files {
        println!("cargo:rerun-if-changed={}", proto_file.display());
    }
    tonic_prost_build::configure()
        .file_descriptor_set_path(out_dir.join("honk_descriptor.bin"))
        .compile_protos(&proto_files, &[PathBuf::from("proto")])?;
    Ok(())
}
