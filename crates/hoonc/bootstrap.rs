// Bazel-specific bootstrap file for hoonc
// This is used to provide the correct paths for include_bytes! in the Bazel build

#[cfg(bazel_build)]
pub const KERNEL_JAM: &[u8] = include_bytes!("bootstrap/hoonc-prewarm.jam");
#[cfg(bazel_build)]
pub const KERNEL_JAM_MINIMAL: &[u8] = include_bytes!("bootstrap/hoonc.jam");
#[cfg(bazel_build)]
pub const HOON_TXT: &[u8] = include_bytes!("../hoon/hoon-138.hoon");
