use std::env;
use std::path::PathBuf;
use std::process::Command;

fn cuda_gencodes() -> Vec<String> {
    if let Ok(value) = env::var("AI_POW_CUDA_GENCODES") {
        let gencodes = value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                let (arch, code) = value.split_once(':').unwrap_or_else(|| {
                    panic!("invalid CUDA gencode {value:?}; expected ARCH:CODE")
                });
                assert!(
                    arch.starts_with("compute_")
                        && (code.starts_with("sm_") || code.starts_with("compute_")),
                    "invalid CUDA gencode {value:?}"
                );
                format!("arch={arch},code={code}")
            })
            .collect::<Vec<_>>();
        assert!(
            !gencodes.is_empty(),
            "AI_POW_CUDA_GENCODES must name at least one target"
        );
        return gencodes;
    }

    let arch = env::var("AI_POW_CUDA_ARCH").unwrap_or_else(|_| "compute_89".to_owned());
    let code = env::var("AI_POW_CUDA_CODE").unwrap_or_else(|_| "compute_89".to_owned());
    vec![format!("arch={arch},code={code}")]
}

fn main() {
    println!("cargo:rerun-if-changed=../ai-pow-miner-cuda/csrc/ai_pow_gemm.cu");
    println!("cargo:rerun-if-changed=../ai-pow-miner-cuda/csrc/ai_pow_v3.cu");
    println!("cargo:rerun-if-changed=../ai-pow-miner-cuda/csrc/ai_pow_v3_peak.cu");
    println!("cargo:rerun-if-changed=../ai-pow-miner-cuda/csrc/ai_pow_v3_gemma4.cu");
    println!("cargo:rerun-if-changed=../ai-pow-miner-cuda/csrc/ai_pow_gemm.h");
    println!("cargo:rerun-if-changed=../ai-pow-miner-cuda/csrc/ai_pow_v3_peak.h");
    println!("cargo:rerun-if-changed=../ai-pow-miner-cuda/csrc/ai_pow_v3_gemma4.h");
    println!("cargo:rerun-if-env-changed=AI_POW_CUDA_ARCH");
    println!("cargo:rerun-if-env-changed=AI_POW_CUDA_CODE");
    println!("cargo:rerun-if-env-changed=AI_POW_CUDA_GENCODES");
    println!("cargo:rerun-if-env-changed=AI_POW_CUDA_TOOLKIT");
    println!("cargo:rerun-if-env-changed=CUDA_HOME");
    if env::var_os("CARGO_FEATURE_GPU").is_none() {
        return;
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    let library = out_dir.join("libai_pow_gemm.a");
    let gencodes = cuda_gencodes();
    let mut objects = Vec::new();
    for source in ["ai_pow_gemm.cu", "ai_pow_v3.cu", "ai_pow_v3_peak.cu", "ai_pow_v3_gemma4.cu"] {
        let object = out_dir.join(format!("{source}.o"));
        let mut command = Command::new("nvcc");
        command.args(["-std=c++17", "-O3", "-Xcompiler", "-fPIC"]);
        for gencode in &gencodes {
            command.args(["-gencode", gencode]);
        }
        let status = command
            .args(["-c", &format!("../ai-pow-miner-cuda/csrc/{source}"), "-o"])
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
    let cuda_home = env::var("CUDA_HOME").unwrap_or_else(|_| "/usr/local/cuda".to_owned());
    println!("cargo:rustc-link-search=native={cuda_home}/lib64");
}
