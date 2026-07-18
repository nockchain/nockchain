//! Poseidon1 permutation non-primitive operation (one Poseidon1 call per row).
//!
//! Supports standard hashing, sponge chaining (`new_start`), and Merkle-path
//! verification (`merkle_path`/`mmcs_bit`) with leaf-index accumulation
//! (`mmcs_index_sum`).

mod builder;
pub mod call;
pub(crate) mod config;
pub(crate) mod executor;
pub(crate) mod plugin;
pub mod state;
pub mod trace;

pub use call::{Poseidon1PermCall, Poseidon1PermCallBase};
pub use config::Poseidon1Config;
pub(crate) use config::Poseidon1PermExec;
pub(crate) use plugin::Poseidon1CircuitPlugin;
pub use state::Poseidon1PermPrivateData;
pub use trace::{
    BabyBearD1Width16, BabyBearD4Width16, GoldilocksD2Width8, KoalaBearD1Width16,
    KoalaBearD4Width16, Poseidon1CircuitRow, Poseidon1Params, Poseidon1Trace,
    generate_poseidon1_trace,
};
