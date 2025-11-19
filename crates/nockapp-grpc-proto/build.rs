use std::env;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Rerun if any file in the proto directory changes

    // Get the output directory
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);

    // Use glob pattern to compile all .proto files
    let proto_files: Vec<_> = glob::glob("proto/**/*.proto")?
        .filter_map(Result::ok)
        .collect();

    for proto_file in proto_files.clone() {
        eprintln!("cargo:rerun-if-changed={}", proto_file.display());
        let path_string = proto_file
            .to_str()
            .expect("Couldn't convert proto_file path to string");
        println!("cargo:rerun-if-changed={path_string}");
    }
    let include_dirs = ["proto"].map(PathBuf::from);
    tonic_prost_build::configure()
        .file_descriptor_set_path(out_dir.join("nockapp_descriptor.bin"))
        .compile_protos(&proto_files, &include_dirs)?;

    Ok(())
}
