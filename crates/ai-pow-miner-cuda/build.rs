use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=csrc/ai_pow_gemm.cu");
    println!("cargo:rerun-if-changed=csrc/ai_pow_gemm.h");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    let object = out_dir.join("ai_pow_gemm.o");
    let library = out_dir.join("libai_pow_gemm.a");
    let arch = env::var("AI_POW_CUDA_ARCH").unwrap_or_else(|_| "compute_89".to_owned());
    let code = env::var("AI_POW_CUDA_CODE").unwrap_or_else(|_| "compute_89".to_owned());

    let status = Command::new("nvcc")
        .args([
            "-std=c++17",
            "-O3",
            "--use_fast_math",
            "-Xcompiler",
            "-fPIC",
            "-gencode",
            &format!("arch={arch},code={code}"),
            "-c",
            "csrc/ai_pow_gemm.cu",
            "-o",
        ])
        .arg(&object)
        .status()
        .expect("nvcc must be installed for the gpu feature");
    assert!(status.success(), "nvcc failed to compile AI-PoW CUDA GEMM");

    let status = Command::new("ar")
        .args(["crus"])
        .arg(&library)
        .arg(&object)
        .status()
        .expect("ar must be installed for the gpu feature");
    assert!(status.success(), "ar failed to archive AI-PoW CUDA GEMM");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=ai_pow_gemm");
    println!("cargo:rustc-link-lib=dylib=cudart");
    if let Ok(cuda_home) = env::var("CUDA_HOME") {
        println!("cargo:rustc-link-search=native={cuda_home}/lib64");
    } else {
        println!("cargo:rustc-link-search=native=/usr/local/cuda/lib64");
    }
}
