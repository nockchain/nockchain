#![allow(clippy::unwrap_used)]
//! Bridge hold tests exercised via Roswell kernel.

#[cfg(feature = "bazel_build")]
use bridge_roswell_harness::run_roswell_test;
#[cfg(not(feature = "bazel_build"))]
mod roswell_harness;
#[cfg(not(feature = "bazel_build"))]
use self::roswell_harness::run_roswell_test;

#[tokio::test]
async fn test_hold_suite() {
    run_roswell_test("test-hold")
        .await
        .expect("roswell hold tests failed");
}
