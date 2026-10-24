#[cfg(feature = "bazel_build")]
pub const FILE_DESCRIPTOR_SET: &[u8] = include_bytes!(env!("BRIDGE_DESCRIPTOR_BIN"));

#[cfg(not(feature = "bazel_build"))]
pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("bridge_descriptor");
