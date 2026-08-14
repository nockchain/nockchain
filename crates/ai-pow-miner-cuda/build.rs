use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=csrc/ai_pow_gemm.cu");
    println!("cargo:rerun-if-changed=csrc/ai_pow_v3.cu");
    println!("cargo:rerun-if-changed=csrc/ai_pow_v3_peak.cu");
    println!("cargo:rerun-if-changed=csrc/ai_pow_gemm.h");
    println!("cargo:rerun-if-changed=csrc/ai_pow_v3_peak.h");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    let library = out_dir.join("libai_pow_gemm.a");
    let arch = env::var("AI_POW_CUDA_ARCH").unwrap_or_else(|_| "compute_89".to_owned());
    let code = env::var("AI_POW_CUDA_CODE").unwrap_or_else(|_| "compute_89".to_owned());
    let mut objects = Vec::new();
    for source in ["ai_pow_gemm.cu", "ai_pow_v3.cu", "ai_pow_v3_peak.cu"] {
        let object = out_dir.join(format!("{source}.o"));
        let status = Command::new("nvcc")
            .args([
                "-std=c++17",
                "-O3",
                "-Xcompiler",
                "-fPIC",
                "-gencode",
                &format!("arch={arch},code={code}"),
                "-c",
                &format!("csrc/{source}"),
                "-o",
            ])
            .arg(&object)
            .status()
            .expect("nvcc must be installed for the gpu feature");
        assert!(status.success(), "nvcc failed to compile {source}");
        objects.push(object);
    }
    let status = Command::new("ar")
        .arg("crus")
        .arg(&library)
        .args(&objects)
        .status()
        .expect("ar must be installed for the gpu feature");
    assert!(status.success(), "ar failed to archive AI-PoW CUDA objects");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=ai_pow_gemm");
    println!("cargo:rustc-link-lib=dylib=cudart");
    if let Ok(cuda_home) = env::var("CUDA_HOME") {
        println!("cargo:rustc-link-search=native={cuda_home}/lib64");
    } else {
        println!("cargo:rustc-link-search=native=/usr/local/cuda/lib64");
    }
}
