#[cfg(not(feature = "bazel_build"))]
pub static KERNEL: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/wal.jam"));

#[cfg(feature = "bazel_build")]
pub static KERNEL: &[u8] = include_bytes!(env!("WALLET_JAM_PATH"));
