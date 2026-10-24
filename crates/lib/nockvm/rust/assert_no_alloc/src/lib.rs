//! Minimal `assert_no_alloc` shim so that `cargo check --workspace` succeeds
//! when nockvm's `check_*` features pull in `assert_no_alloc` as a dependency.
//!
//! The upstream `assert_no_alloc` crate is only vendored for Bazel builds; this
//! workspace-local shim satisfies Cargo's resolver without introducing a
//! crates.io dependency.  It is intentionally a no-op.

pub fn permit_alloc<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    f()
}
