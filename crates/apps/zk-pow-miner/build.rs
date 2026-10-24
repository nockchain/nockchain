use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=csrc/v5_cuda.cu");
    println!("cargo:rerun-if-changed=csrc/v5_cuda.h");
    println!("cargo:rerun-if-env-changed=ZK_POW_NVCC");
    println!("cargo:rerun-if-env-changed=ZK_POW_CUDA_ARCH");
    println!("cargo:rerun-if-env-changed=CUDA_PATH");

    if env::var_os("CARGO_FEATURE_CUDA").is_none() {
        return;
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let object = out_dir.join("v5_cuda.o");
    let archive = out_dir.join("libzk_pow_cuda.a");
    let nvcc = find_nvcc();
    let arch = env::var("ZK_POW_CUDA_ARCH").unwrap_or_else(|_| "native".to_owned());

    let mut command = Command::new(&nvcc);
    command.args([
        "-std=c++17", "-O3", "--threads", "0", "--compiler-options", "-fPIC", "-c",
        "csrc/v5_cuda.cu", "-o",
    ]);
    command.arg(&object);
    if arch == "native" {
        command.arg("-arch=native");
    } else {
        for architecture in arch
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
        {
            command.arg(format!(
                "-gencode=arch=compute_{architecture},code=sm_{architecture}"
            ));
        }
    }
    run(command, "nvcc");

    let mut ar = Command::new("ar");
    ar.args(["crus"]);
    ar.arg(&archive).arg(&object);
    run(ar, "ar");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=zk_pow_cuda");
    println!("cargo:rustc-link-lib=dylib=cudart");

    if let Some(cuda_path) = env::var_os("CUDA_PATH") {
        emit_cuda_search_paths(Path::new(&cuda_path));
    } else if let Some(parent) = Path::new(&nvcc).parent().and_then(Path::parent) {
        emit_cuda_search_paths(parent);
    } else {
        emit_cuda_search_paths(Path::new("/usr/local/cuda"));
    }
}

fn find_nvcc() -> std::ffi::OsString {
    if let Some(nvcc) = env::var_os("ZK_POW_NVCC") {
        return nvcc;
    }
    if let Some(cuda_path) = env::var_os("CUDA_PATH") {
        let nvcc = PathBuf::from(cuda_path).join("bin/nvcc");
        if nvcc.is_file() {
            return nvcc.into_os_string();
        }
    }
    let conventional = PathBuf::from("/usr/local/cuda/bin/nvcc");
    if conventional.is_file() {
        return conventional.into_os_string();
    }
    "nvcc".into()
}

fn emit_cuda_search_paths(cuda_path: &Path) {
    for directory in ["lib64", "lib"] {
        let path = cuda_path.join(directory);
        if path.exists() {
            println!("cargo:rustc-link-search=native={}", path.display());
        }
    }
}

fn run(mut command: Command, name: &str) {
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("failed to execute {name}: {error}"));
    assert!(status.success(), "{name} failed with status {status}");
}
