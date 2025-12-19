#[cfg(feature = "bazel_build")]
pub static KERNEL: &[u8] = include_bytes!(env!("BRIDGE_JAM_PATH"));

#[cfg(not(feature = "bazel_build"))]
pub static KERNEL: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/bridge.jam"
));
