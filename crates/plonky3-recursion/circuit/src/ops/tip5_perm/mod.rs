//! Tip5 permutation non-primitive operation (C2 / M-S4).
//!
//! Full NPO subsystem (C2.3), the faithful mechanical mirror of
//! `poseidon1_perm`'s D=1 path (base **and** merkle / MMCS) → the
//! recursive Tip5 variant (Goldilocks, D=1, width 16, rate 10,
//! capacity 6, digest 5, 5-round). The permutation *constraint system*
//! lives in the sibling
//! `p3-tip5-circuit-air` (`Tip5PermLookupAir`, KAT-anchored to
//! `nockchain_math::tip5::permute_5round`); the circuit-prover Tip5 table
//! (`p3_circuit_prover::batch_stark_prover::tip5`) wraps it and adds
//! the WitnessChecks CTL.

mod builder;
pub mod call;
pub(crate) mod config;
pub(crate) mod executor;
pub(crate) mod plugin;
pub mod state;
pub mod trace;

pub use call::{Tip5PermCall, Tip5PermCallMmcs};
pub use config::{Tip5Config, Tip5FieldId};
pub(crate) use config::Tip5PermExec;
pub(crate) use plugin::Tip5CircuitPlugin;
pub use state::Tip5PermPrivateData;
pub use trace::{Tip5CircuitRow, Tip5Goldilocks, Tip5Params, Tip5Trace, generate_tip5_trace};
