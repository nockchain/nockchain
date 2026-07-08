mod context;
mod executor;
mod npo;
mod op;

pub mod hash;
pub mod mmcs;
pub mod perm;
pub mod poseidon1_perm;
pub mod poseidon2_perm;
pub mod recompose;
pub mod tip5_perm;

pub use context::*;
pub use executor::*;
pub use npo::*;
pub use op::*;
pub use perm::{PermCall, PermConfig, perm_private_data};
pub use poseidon1_perm::{
    // Prover/AIR (trace access)
    Poseidon1CircuitRow,
    Poseidon1Config,
    Poseidon1Params,
    // Builder API
    Poseidon1PermCall,
    // Configuration
    Poseidon1PermPrivateData,
    Poseidon1Trace,
    generate_poseidon1_trace,
};
pub use poseidon2_perm::{
    // Preset configurations
    BabyBearD1Width16,
    GoldilocksD2Width8,
    KoalaBearD1Width16,
    // Prover/AIR (trace access)
    Poseidon2CircuitRow,
    Poseidon2Config,
    Poseidon2Params,
    // Builder API
    Poseidon2PermCall,
    // Configuration
    Poseidon2PermPrivateData,
    Poseidon2Trace,
    generate_poseidon2_trace,
};
pub use recompose::{
    RecomposeCircuitRow, RecomposeTrace, RecomposeTraceKind, generate_recompose_coeff_trace,
    generate_recompose_trace,
};
pub use tip5_perm::{
    // Prover/AIR (trace access)
    Tip5CircuitRow,
    // Configuration / NPO key (C2.2)
    Tip5Config,
    Tip5FieldId,
    Tip5Goldilocks,
    Tip5Params,
    // Builder API
    Tip5PermCall,
    // Configuration
    Tip5PermPrivateData,
    Tip5Trace,
    generate_tip5_trace,
};
