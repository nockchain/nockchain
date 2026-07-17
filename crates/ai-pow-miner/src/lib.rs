//! `ai-pow-miner` - block-mining driver for the `ai-pow` PoW.
//!
//! Wraps [`ai_pow`] with:
//! * The production-oriented NockApp driver, [`run`], which performs
//!   Pearl-format-compatible Nockchain `%ai-pow` submission. The miner searches
//!   Pearl-style ticket attempts and constructs the recursive certificate only
//!   after the Nockchain target is hit. The selected noun encoder packages the
//!   compact final-layer batch-STARK certificate as canonical bytes inside the
//!   `%ai-pow` artifact. The large checkpoint certificate remains a regression
//!   path; native terminal compression has been removed from this API.
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
