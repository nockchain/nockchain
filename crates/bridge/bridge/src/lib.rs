// Allow unwrap in unit-test-only code paths; production code is still linted with `-D clippy::unwrap_used`.
#![cfg_attr(test, allow(clippy::unwrap_used))]

#[cfg(all(feature = "snmalloc", feature = "malloc"))]
compile_error!("features `snmalloc` and `malloc` are mutually exclusive");

pub mod core;
pub mod deposit;
pub mod observability;
pub mod shared;
pub mod withdrawal;
