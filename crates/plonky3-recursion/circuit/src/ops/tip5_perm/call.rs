//! User-facing call structs for adding Tip5 permutation rows (C2.3).
//!
//! - [`Tip5PermCall`] mirrors `Poseidon1PermCallBase` (the D=1,
//!   non-merkle base path), specialised to the recursive Tip5 sponge
//!   geometry: 16 base-field input slots, 10 rate output exposure
//!   flags. Used by the in-circuit challenger and the standalone
//!   batch-STARK CTL gate.
//! - [`Tip5PermCallMmcs`] mirrors `Poseidon1PermCall` (the merkle /
//!   MMCS-path variant: `Vec` input slots, `mmcs_bit`,
//!   `mmcs_index_sum`). Consumed by the `PermConfig`-generic
//!   `add_perm` dispatch so the in-circuit MMCS (`add_mmcs_verify`)
//!   routes Tip5 through the Tip5 NPO with rate-10 sibling-compress /
//!   merkle-swap, bit-for-bit with native
//!   `MerkleTreeMmcs<Goldilocks, _, PaddingFreeSponge<Tip5Perm,16,10,5>,
//!   TruncatedPermutation<Tip5Perm,2,5,16>, …>`.

use alloc::vec;
use alloc::vec::Vec;

use crate::ops::tip5_perm::config::Tip5Config;
use crate::types::ExprId;

/// User-facing arguments for adding a Tip5 perm row (one 5-round Tip5
/// permutation), D=1 base field. Mirrors `Poseidon1PermCallBase`.
pub struct Tip5PermCall {
    /// Tip5 configuration for this permutation row (always D=1
    /// Goldilocks, width 16, rate 10).
    pub config: Tip5Config,
    /// Flag indicating whether a new sponge chain is started (no
    /// previous-state carry).
    pub new_start: bool,
    /// Optional CTL exposure for each of the 16 input elements. If
    /// `None`, the element is not exposed via CTL (and is taken from
    /// the chain carry or zero).
    pub inputs: [Option<ExprId>; 16],
    /// Output exposure flags for the rate elements (first RATE=10).
    /// When `out_ctl[i]` is true, output[i] is CTL-verified.
    pub out_ctl: [bool; 10],
    /// Whether to return all 16 output elements (for challenger use).
    /// When true, outputs 10-15 are also allocated and returned, but
    /// NOT CTL-verified (capacity elements, constrained only by the
    /// Tip5 permutation itself).
    pub return_all_outputs: bool,
}

impl Default for Tip5PermCall {
    fn default() -> Self {
        Self {
            config: Tip5Config::GOLDILOCKS_W16,
            new_start: false,
            inputs: [None; 16],
            out_ctl: [false; 10],
            return_all_outputs: false,
        }
    }
}

/// User-facing arguments for adding a Tip5 perm row in the merkle /
/// MMCS-path layout. Faithful mirror of `Poseidon1PermCall`, Tip5
/// D=1 numbers (width_ext = 16, rate_ext = 10, capacity_ext = 6).
///
/// The `PermConfig`-generic `add_perm` builds this from the
/// hash-agnostic `PermCall`; `add_mmcs_verify` is the only caller and
/// always supplies `width_ext (16)` input limb slots followed by the
/// `mmcs_index_sum` and `mmcs_bit` tail slots (Poseidon1 D=1 layout).
pub struct Tip5PermCallMmcs {
    /// Tip5 configuration for this permutation row (always D=1
    /// Goldilocks, width 16, rate 10).
    pub config: Tip5Config,
    /// Flag indicating whether a new sponge / Merkle chain is started.
    pub new_start: bool,
    /// Flag indicating whether we are verifying a Merkle path.
    pub merkle_path: bool,
    /// MMCS direction bit input (base field, boolean). Required when
    /// `merkle_path = true`.
    pub mmcs_bit: Option<ExprId>,
    /// Optional CTL exposure for each input slot. `None` means the
    /// slot is not exposed via CTL (its value comes from the chain
    /// carry / sibling private data / zero per the executor).
    pub inputs: Vec<Option<ExprId>>,
    /// Output exposure flags for rate limbs (CTL-verified). When
    /// `out_ctl[i]` is true, output limb `i` is allocated + exposed.
    pub out_ctl: Vec<bool>,
    /// Whether to return all 16 output elements. When true, outputs
    /// 10-15 (capacity) are also allocated and returned, but NOT
    /// CTL-verified.
    pub return_all_outputs: bool,
    /// Optional MMCS index accumulator value to expose. Always `None`
    /// for the in-circuit MMCS (`add_mmcs_verify`).
    pub mmcs_index_sum: Option<ExprId>,
}

impl Default for Tip5PermCallMmcs {
    fn default() -> Self {
        let config = Tip5Config::GOLDILOCKS_W16;
        Self {
            config,
            new_start: false,
            merkle_path: false,
            mmcs_bit: None,
            inputs: vec![None; config.width_ext()],
            out_ctl: vec![false; config.rate_ext()],
            return_all_outputs: false,
            mmcs_index_sum: None,
        }
    }
}
