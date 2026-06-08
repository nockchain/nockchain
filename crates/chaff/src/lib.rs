mod jammer;

pub use jammer::{Chaff, CueError};

#[cfg(test)]
include!("legacy_tests.rs");
