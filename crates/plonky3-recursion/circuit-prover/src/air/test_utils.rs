//! Single-AIR satisfaction helpers re-exported from [`p3_test_utils::air_satisfaction`].
//!
//! Kept as a thin shim so the `crate::air::test_utils::assert_air_satisfies` import path used
//! across the primitive AIR tests stays stable.

pub use p3_test_utils::air_satisfaction::{
    assert_air_rejects, assert_air_satisfies, check_air_satisfies,
};
