#![allow(dead_code)]
use std::env;
use std::path::PathBuf;
use std::process::Command;

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
    match Command::new("protoc").arg("--version").status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!(
            "protoc from PATH exited with status {status}; set PROTOC to a valid binary path"
        )
        .into()),
        Err(_) => Err("PROTOC is not set and protoc was not found on PATH".into()),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Rerun if any file in the proto directory changes

    ensure_protoc()?;

    // Get the output directory
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);

    // Use glob pattern to compile all .proto files
    let proto_files: Vec<_> = glob::glob("proto/**/*.proto")?
        .filter_map(Result::ok)
        .collect();

    for proto_file in proto_files.clone() {
        eprintln!("cargo:rerun-if-changed={}", proto_file.display());
        let path_string = proto_file.to_str().ok_or("proto path is not valid UTF-8")?;
        println!("cargo:rerun-if-changed={path_string}");
    }
    let include_dirs = ["proto"].map(PathBuf::from);
    tonic_prost_build::configure()
        .file_descriptor_set_path(out_dir.join("nockapp_descriptor.bin"))
        .compile_protos(&proto_files, &include_dirs)?;

    Ok(())
}
