//! `ai-pow-miner` - block-mining driver for the `ai-pow` PoW.
//!
//! Wraps [`ai_pow`] with:
//! * The production-oriented NockApp driver, [`run`], which performs
//!   Pearl-format-compatible Nockchain `%ai-pow` submission. The miner searches
//!   Pearl-style ticket attempts and constructs the recursive certificate only
//!   after the Nockchain target is hit. The selected noun encoder packages the
//!   compact final-layer batch-STARK certificate as canonical bytes inside the
//!   `%ai-pow` artifact. The large checkpoint certificate remains a regression
//!   path.
//! * Pearl Gateway proof-submission plumbing for Pearl-side hits. This remains
//!   Rust-only miner metadata; it is not part of the Hoon `%ai-pow` artifact.
//! * A Rust-owned opaque nonce envelope for `%ai-pow` artifacts. Hoon sees the
//!   nonce only as `[len data]`; Pearl-format transcript details remain Rust
//!   metadata and must not become Hoon kernel concepts.
//!
//! The bare crate (no features) has no async / NockApp dep, which is useful for
//! benchmarks and fuzz harnesses. The connected node driver lives behind
//! `feature = "node"`.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::field_reassign_with_default))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Caller-supplied 256-bit chain difficulty bound. Compared
/// little-endian (`ai_pow::tile_hash::hash_le_target` semantics).
pub type DifficultyTarget = [u8; 32];

/// Exact dense production benchmark/miner fixture admitted by the named dense route.
pub const DENSE_PRODUCTION_PARAMS: ai_pow::params::MatmulParams = ai_pow::params::MatmulParams {
    m: 512,
    k: 1024,
    n: 512,
    noise_rank: 64,
    tile: 8,
    spot_checks: 1,
    difficulty_bits: 0,
};

/// Dense Pearl V3 profile for the RTX 5090 production search kernel.
pub const PEAK_PRODUCTION_PARAMS: ai_pow::params::MatmulParams = ai_pow::params::MatmulParams {
    m: 4096,
    k: 8192,
    n: 32768,
    noise_rank: 512,
    tile: 16,
    spot_checks: 1,
    difficulty_bits: 0,
};

/// Return the dense RTX 5090 production profile.
pub const fn peak_production_params() -> ai_pow::params::MatmulParams {
    PEAK_PRODUCTION_PARAMS
}

/// Return the exact dense production fixture parameters.
pub const fn dense_production_params() -> ai_pow::params::MatmulParams {
    DENSE_PRODUCTION_PARAMS
}

/// Snapshot for progress callbacks and the final solution.
#[derive(Clone, Debug, Default)]
pub struct MiningStats {
    /// Count of fully evaluated Pearl-compatible matmul ticket attempts.
    pub matmul_attempts_tried: u64,
    pub elapsed: Duration,
}

impl MiningStats {
    pub fn matmul_attempt_rate_per_sec(&self) -> f64 {
        let s = self.elapsed.as_secs_f64();
        if s > 0.0 {
            (self.matmul_attempts_tried as f64) / s
        } else {
            0.0
        }
    }
}

/// Cooperative cancellation. Clone freely; checked at every ticket boundary.
#[derive(Clone, Default)]
pub struct MiningCancel(Arc<AtomicBool>);

impl MiningCancel {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Pearl-compatible merge-mining ticket loop.
pub mod pearl_mining;

/// Bounded CPU and accelerator ticket-search backend contract.
pub mod search;

/// Gemma 4 31B checkpoint validation and native fused gate/up operand mapping.
pub mod gemma4;

/// RTX 5090 dense ticket-search session and benchmark contract.
#[cfg(feature = "gpu")]
pub mod peak;

/// Native Gemma 4 fused gate/up CUDA session and benchmark contract.
#[cfg(feature = "gpu")]
pub mod gemma4_cuda;

/// Pearl Gateway `submitPlainProof` artifact construction.
///
/// Internal by design: Hoon and external Nockchain callers submit only the
/// recursive `%ai-pow` certificate. Pearl `PlainProof` is a Gateway wire detail
/// built by the miner when a Pearl target hits.
// Only the `node` run-loop (gateway submission) consumes this module; its own
// tests exercise it directly. Gate it so a default (no-`node`) build does not
// flag the whole module as dead code.
#[cfg(any(feature = "node", test))]
pub(crate) mod pearl_plain_proof;

/// Wire vocabulary (`AiPowMinerWire`, `SOURCE = "ai-pow-miner"`). Behind
/// the `node` feature because it implements `nockapp::wire::Wire`.
#[cfg(feature = "node")]
pub mod wire;

/// Noun encoder for recursive AI-PoW certificates, including the selected
/// compact final-layer batch-STARK artifact.
#[cfg(feature = "node")]
pub mod certificate_noun;

/// Gateway-free canonical AI-PoW block proving for a self-contained CPU miner
/// (see [`run::run_canonical`]). Copies the jets-free canonical prover so the
/// standalone miner can prove a valid `%ai-pow` block bound to the node's block
/// commitment without an external Pearl Gateway.
#[cfg(feature = "node")]
pub mod canonical;

/// Out-of-process node-connecting run loop ([`run::run`]) - the production
/// entry point used by the `ai-pow-mine` binary. Behind the `node` feature
/// because it pulls in the gRPC + nockapp dep tree.
#[cfg(feature = "node")]
pub mod run;

/// Shared CLI arguments and node-miner configuration. Backend binaries flatten
/// the same work arguments and choose their search implementation explicitly.
#[cfg(feature = "node")]
pub mod cli;
/// CUDA search implementations. `--gpu --canonical` selects the production
/// dense route. Gateway mode retains the generic CUDA backend.
#[cfg(all(feature = "node", feature = "gpu"))]
pub mod gpu;

/// Test-only synthetic nockchain targets sized so the factor-adjusted
/// product fits the 256-bit band: fixtures previously used `[0xff; 32]`,
/// which the fail-closed adjusted-target multiply rejects as over-band.
#[cfg(test)]
pub(crate) fn easy_nock_target() -> [u8; 32] {
    // 2^240 − 1: fits fixture factors up to 2^16 (the standard
    // 8×8×1024 test config), yielding adjusted target 2^256 − 2^16.
    let mut t = [0u8; 32];
    t[..30].fill(0xff);
    t
}

/// Largest target whose factor-adjusted product fits the 256-bit band for
/// `config` (floor((2^256 − 1) / (h·w·dot))), for fixtures with
/// non-standard pattern shapes.
#[cfg(test)]
pub(crate) fn easy_target_for(config: &ai_pow::pearl_compat::PearlMiningConfig) -> [u8; 32] {
    let factor = u64::from(config.rows_pattern.size().unwrap())
        * u64::from(config.cols_pattern.size().unwrap())
        * config.dot_product_length().unwrap() as u64;
    assert!(factor > 0);
    // Floor-divide [0xff; 32] by the factor (fits in u64 limb math for any
    // envelope-valid factor).
    let mut out = [0u8; 32];
    let mut rem = 0u64;
    for i in (0..32).rev() {
        let acc = (rem << 8) | 0xffu64;
        out[i] = (acc / factor) as u8;
        rem = acc % factor;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mining_cancel_works() {
        let c = MiningCancel::new();
        assert!(!c.is_cancelled());
        let c2 = c.clone();
        c.cancel();
        assert!(c2.is_cancelled());
    }
}
