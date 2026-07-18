//! An AIR for the Poseidon2 table for recursion. Handles sponge operations and compressions.
//!
//! The active AI-PoW production path uses Tip5 commitments for the compact
//! recursive certificate. Poseidon2 support remains in the vendored recursion
//! substrate because generic recursive examples and retained unit tests use it.

#![no_std]

extern crate alloc;

mod air;
mod columns;
mod public_types;

pub use air::*;
pub use columns::*;
pub use public_types::*;
