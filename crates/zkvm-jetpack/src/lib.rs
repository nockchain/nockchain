#![feature(cold_path)]
// Allow unwrap in test code - standard practice for test assertions
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod form;
pub mod hot;
pub mod jets;
pub mod utils;
pub use nockchain_math::based;
