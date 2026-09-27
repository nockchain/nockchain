use std::error::Error;
use std::path::PathBuf;
use std::{env, fs};

#[path = "build_support/schema.rs"]
mod schema;

fn main() -> Result<(), Box<dyn Error>> {
    let proto_dir = PathBuf::from("proto/nockchain/peer/v3");
    println!("cargo:rerun-if-changed={}", proto_dir.display());
    println!("cargo:rerun-if-changed=build_support/schema.rs");
    println!("cargo:rerun-if-env-changed=PROTOC");
    println!("cargo:rerun-if-env-changed=PROTOC_INCLUDE");

    let mut protos = Vec::new();
    for entry in fs::read_dir(&proto_dir)? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "proto")
        {
            println!("cargo:rerun-if-changed={}", path.display());
            protos.push(path);
        }
    }
    protos.sort();
    if protos.is_empty() {
        return Err("peer protocol v3 has no protobuf schemas".into());
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is unset")?);
    let mut config = prost_build::Config::new();
    config.file_descriptor_set_path(out_dir.join("peer_v3_descriptor.bin"));
    let descriptors = config.load_fds(&protos, &["proto"])?;
    schema::validate_peer_schema(&descriptors)?;
    config.compile_fds(descriptors)?;
    Ok(())
}
