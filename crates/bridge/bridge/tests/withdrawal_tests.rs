#![allow(clippy::unwrap_used)]
//! Bridge withdrawal kernel tests exercised via Roswell kernel.

#[cfg(feature = "bazel_build")]
use bridge_roswell_harness::run_roswell_test;
#[cfg(not(feature = "bazel_build"))]
mod roswell_harness;
#[cfg(not(feature = "bazel_build"))]
use self::roswell_harness::run_roswell_test;

#[tokio::test]
async fn test_withdrawal_suite() {
    run_roswell_test("test-withdrawal")
        .await
        .expect("roswell withdrawal tests failed");
}
