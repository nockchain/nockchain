#[cfg(feature = "bazel_build")]
pub static KERNEL: &[u8] = include_bytes!(env!("NOCKCHAIN_PEEK_JAM_PATH"));

#[cfg(not(feature = "bazel_build"))]
pub const KERNEL: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/peek.jam"
));
