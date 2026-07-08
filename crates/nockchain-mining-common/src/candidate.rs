//! Decoded form of a `%mine-zk` or `%mine-ai` effect emitted by the
//! node's kernel.
//!
//! The kernel-side Hoon emits effects shaped `[%mine-zk version commit
//! target pow-len]` (always) and `[%mine-ai version commit target
//! pow-len]` (post-AI-activation). `commit` is the kernel's
//! `block-commitment:page`, not a raw block header. Each miner subscribes via
//! WatchEffects with its own head filter (`b"mine-zk"` / `b"mine-ai"`).
//! This decoder is shape-symmetric: same field layout for both heads,
//! so the same struct holds either kind of candidate while preserving the
//! effect kind for puzzle-specific boundary checks.

use nockapp::noun::slab::NounSlab;
use nockchain_math::noun_ext::NounMathExtHandle;
use nockvm::noun::{Noun, NounAllocator};
use thiserror::Error;

/// A mining candidate: everything the miner needs to drive one
/// puzzle-nock attempt.
pub struct MiningCandidate {
    /// Which mining effect head produced this candidate.
    pub kind: MiningCandidateKind,
    /// Block-version atom from the candidate (chain consensus version).
    pub version: NounSlab,
    /// Kernel-emitted `block-commitment:page` noun for the candidate block.
    ///
    /// The field name is kept for compatibility with existing ZK miner code,
    /// but consumers should treat the noun as the candidate block commitment.
    pub block_header: NounSlab,
    /// PoW difficulty target.
    pub target: NounSlab,
    /// `pow-len` parameter (proof length in bytes).
    pub pow_len: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MiningCandidateKind {
    Zk,
    Ai,
}

#[derive(Debug, Error)]
pub enum CandidateDecodeError {
    #[error("effect noun is not a cell")]
    NotACell,
    #[error("effect head is not %mine-zk or %mine-ai")]
    NotMine,
    #[error("effect tail does not match the 4-tuple [version commit target pow-len]")]
    BadTuple,
    #[error("pow-len is not a u64 atom")]
    BadPowLen,
}

impl MiningCandidate {
    /// Decode a `[%mine-zk version commit target pow-len]` or
    /// `[%mine-ai version commit target pow-len]` effect noun (read
    /// from a `WatchEffects` stream) into a `MiningCandidate`. The
    /// caller's head_filter on the subscription decides which head it
    /// receives; this decoder accepts either.
    ///
    /// Returns `Ok(None)` for an effect whose head is neither
    /// `%mine-zk` nor `%mine-ai`.
    pub fn from_effect_slab(slab: NounSlab) -> Result<Option<Self>, CandidateDecodeError> {
        // SAFETY: `slab.root()` returns a valid Noun owned by the slab
        // for the lifetime of `slab`. We construct nested slabs via
        // `copy_into`, which is safe. Post-h-zoon: noun reads must be
        // explicitly bound to the slab's NounSpace via `in_space`.
        let root = unsafe { *slab.root() };
        let space = slab.noun_space();
        let effect_cell = root
            .in_space(&space)
            .as_cell()
            .map_err(|_| CandidateDecodeError::NotACell)?;
        let head = effect_cell.head();
        let kind = if head.eq_bytes("mine-zk") {
            MiningCandidateKind::Zk
        } else if head.eq_bytes("mine-ai") {
            MiningCandidateKind::Ai
        } else {
            return Ok(None);
        };
        let [version_h, commit_h, target_h, pow_len_h] = effect_cell
            .tail()
            .uncell::<4>()
            .map_err(|_| CandidateDecodeError::BadTuple)?;

        let pow_len = pow_len_h
            .as_atom()
            .map_err(|_| CandidateDecodeError::BadPowLen)?
            .as_u64()
            .map_err(|_| CandidateDecodeError::BadPowLen)?;

        Ok(Some(MiningCandidate {
            kind,
            version: noun_into_owned_slab(version_h.noun(), &space),
            block_header: noun_into_owned_slab(commit_h.noun(), &space),
            target: noun_into_owned_slab(target_h.noun(), &space),
            pow_len,
        }))
    }
}

/// Copy a noun into a fresh `NounSlab` with that noun as the root, so it
/// can be owned and moved independently of the source slab. Mirrors the
/// `header_slab / version_slab / target_slab` build pattern in the old
/// in-tree driver.
fn noun_into_owned_slab(noun: Noun, space: &nockvm::noun::NounSpace) -> NounSlab {
    let mut slab = NounSlab::new();
    let copied = slab.copy_into(noun, space);
    slab.set_root(copied);
    slab
}

#[cfg(test)]
mod tests {
    use nockvm::noun::{D, T};
    use nockvm_macros::tas;

    use super::*;

    /// Synthesize `[%mine-zk version commit target pow-len]` and round-trip
    /// it through `MiningCandidate::from_effect_slab`.
    #[test]
    fn from_effect_slab_decodes_mine_zk_tuple() {
        let mut slab = NounSlab::new();
        let head = D(tas!(b"mine-zk"));
        let version = D(0xAA);
        let commit = D(0xBBBB);
        let target = D(0xCCCC_CCCC);
        let pow_len = D(256);
        let root = T(&mut slab, &[head, version, commit, target, pow_len]);
        slab.set_root(root);

        let candidate = MiningCandidate::from_effect_slab(slab)
            .expect("decode")
            .expect("head is %mine-zk");

        assert_eq!(candidate.pow_len, 256);
        assert_eq!(candidate.kind, MiningCandidateKind::Zk);
        // The owned slabs round-trip the values. Post-h-zoon: atom
        // reads must be bound to a NounSpace via in_space.
        let version_space = candidate.version.noun_space();
        let v = unsafe { *candidate.version.root() }
            .in_space(&version_space)
            .as_atom()
            .expect("version atom")
            .as_u64()
            .expect("u64");
        assert_eq!(v, 0xAA);
        let header_space = candidate.block_header.noun_space();
        let h = unsafe { *candidate.block_header.root() }
            .in_space(&header_space)
            .as_atom()
            .expect("commit atom")
            .as_u64()
            .expect("u64");
        assert_eq!(h, 0xBBBB);
        let target_space = candidate.target.noun_space();
        let t = unsafe { *candidate.target.root() }
            .in_space(&target_space)
            .as_atom()
            .expect("target atom")
            .as_u64()
            .expect("u64");
        assert_eq!(t, 0xCCCC_CCCC);
    }

    #[test]
    fn from_effect_slab_decodes_mine_ai_tuple() {
        let mut slab = NounSlab::new();
        let head = D(tas!(b"mine-ai"));
        let root = T(&mut slab, &[head, D(0x11), D(0x22), D(0x33), D(64)]);
        slab.set_root(root);
        let candidate = MiningCandidate::from_effect_slab(slab)
            .expect("decode")
            .expect("head is %mine-ai");
        assert_eq!(candidate.kind, MiningCandidateKind::Ai);
        assert_eq!(candidate.pow_len, 64);
    }

    #[test]
    fn from_effect_slab_returns_none_for_other_heads() {
        let mut slab = NounSlab::new();
        let head = D(tas!(b"irrelvnt")); // 8-char tas! limit
        let root = T(&mut slab, &[head, D(0), D(0), D(0), D(0)]);
        slab.set_root(root);

        let res = MiningCandidate::from_effect_slab(slab).expect("decode");
        assert!(res.is_none(), "non-mine head should decode to None");
    }

    #[test]
    fn from_effect_slab_errors_on_bad_tuple() {
        // %mine-zk head but the tail is a single atom, not a 4-tuple.
        let mut slab = NounSlab::new();
        let head = D(tas!(b"mine-zk"));
        let root = T(&mut slab, &[head, D(0)]);
        slab.set_root(root);

        let res = MiningCandidate::from_effect_slab(slab);
        assert!(matches!(res, Err(CandidateDecodeError::BadTuple)));
    }
}
