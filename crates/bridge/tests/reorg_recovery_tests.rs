#![allow(clippy::unwrap_used)]
//! Real-kernel proof of the post-confirmation history-divergence boundary.

#[cfg(feature = "bazel_build")]
use bridge_roswell_harness::run_roswell_test;
#[cfg(not(feature = "bazel_build"))]
mod roswell_harness;
#[cfg(not(feature = "bazel_build"))]
use self::roswell_harness::run_roswell_test;

#[tokio::test]
async fn hoon_post_confirmation_reorg_fail_stop_suite() {
    run_roswell_test("test-reorg-recovery")
        .await
        .expect("Roswell reorg recovery boundary tests failed");
}
