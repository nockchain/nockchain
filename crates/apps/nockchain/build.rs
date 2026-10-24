fn main() {
    // List of Bazel built-in stamping variables to embed
    let bazel_vars = [
        "BUILD_EMBED_LABEL", "BUILD_HOST", "BUILD_USER", "BUILD_TIMESTAMP",
        "FORMATTED_DATE",
        // You can add more built-in variables or your own STABLE_ variables as needed
    ];

    // Set cargo:rustc-env for each variable that exists
    for var in bazel_vars {
        let value = std::env::var(var).unwrap_or_else(|_| "unknown".to_string());
        println!("cargo:rustc-env={var}={value}");
    }
}
