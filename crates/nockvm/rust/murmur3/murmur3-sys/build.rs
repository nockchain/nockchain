fn main() {
    let out_dir = std::env::var("OUT_DIR").expect("No out dir");

    // If LIBCLANG_SO_PATH is set (by Bazel), derive LIBCLANG_PATH from it
    // This is needed for hermetic Nix builds where libclang is in the Nix store
    if let Ok(libclang_so_path) = std::env::var("LIBCLANG_SO_PATH") {
        if let Some(parent) = std::path::Path::new(&libclang_so_path).parent() {
            std::env::set_var("LIBCLANG_PATH", parent);
        }
    }

    // Compile the C library
    cc::Build::new().file("murmur3.c").compile("murmur3");

    // Generate Rust bindings
    let bindings = bindgen::Builder::default()
        .header("murmur3.h")
        .generate()
        .expect("Unable to generate bindings");

    bindings
        .write_to_file(std::path::Path::new(&out_dir).join("bindings.rs"))
        .expect("Couldn't write bindings!");
}
