//! Per-row BLAKE3 instruction descriptor.
//!
//! **Pearl ISC.** This file is derived from Pearl source code
//! (Copyright (c) 2025-2026 Pearl Research Labs; 2015-2016 The Decred
//! developers); see `crates/ai-pow-zk/LICENSE-PEARL` for the full
//! permission notice.
//!
//! Port of `Pearl zk-pow chip/blake3/logic.rs`. Defines
//! the high-level "what should this row of the BLAKE3 chip do" type
//! that Phase 8's trace generator + constraint evaluator consume.
//!
//! A full BLAKE3 hash spans 8 consecutive trace rows (rounds 1..=7
//! plus a final feed-forward XOR row counted as round 8). The
//! per-row descriptor carries enough information to populate the
//! trace and gate the right constraints.

use super::compress::Blake3Tweak;

/// Where this BLAKE3 row's message buffer comes from. Mirrors
/// Pearl's `MessageDataType` (`logic.rs:13-25`).
#[derive(Clone, Debug, Copy, Default)]
pub enum MessageDataType {
    /// Load a dword from the noised-matrix table at this row's
    /// `MAT_ID`. Phase 9's matmul chip writes the same byte stream;
    /// the BLAKE3 chip reads it for h_a / h_b leaf hashing.
    Matrix { dword_offset: usize },
    /// Load from auxiliary data (separate from the matrix RAM):
    /// either an auxiliary message word or a chaining value.
    Auxiliary { kind: AuxKind, dword_idx: usize },
    /// Load 4 dwords from a previous BLAKE3 row's `CV_OUT` (used to
    /// chain multi-block hashes via the BLAKE3 message permutation).
    PreviousCv { source_row_idx: usize },
    /// Load from the jackpot tile-state buffer (h_a finalisation
    /// uses this to consume the tile-state Merkle leaf).
    Jackpot,
    /// No data loading this round. The default for rounds 2..=7
    /// that just permute the message buffer.
    #[default]
    None,
}

/// Which sub-stream of auxiliary data to load from.
#[derive(Clone, Debug, Copy)]
pub enum AuxKind {
    /// 64-byte message segment, 8 dwords.
    Msg { aux_msg_idx: usize },
    /// 32-byte CV, 4 dwords.
    Cv { aux_cv_idx: usize },
}

/// Per-row logic descriptor. Mirrors Pearl's `BlakeRoundLogic`
/// (`logic.rs:27-42`).
#[derive(Clone, Debug, Copy)]
pub struct BlakeRoundLogic {
    /// What to load into `BLAKE3_MSG_BUFFER` this round.
    pub data_source: MessageDataType,
    /// Set only on round 1 of each BLAKE3 hash — the per-hash tweak
    /// (counter, block_len, flags). Defaulted to `None` for rounds
    /// 2..=7.
    pub blake3_tweak: Option<Blake3Tweak>,
    /// Round index within a BLAKE3 compression, 1-indexed:
    /// `1..=8` (8 = feed-forward XOR finalization).
    pub round_idx: usize,
    /// Optional: a previous STARK row to read CV_OUT from (CV
    /// routing lookup). `None` if this round uses JOB_KEY or
    /// COMMITMENT_HASH as the CV instead.
    pub idx_of_row_whence_to_read_cv: Option<usize>,
    /// This row produces HASH_A (matrix-A keyed hash output).
    /// Asserts CV_OUT == public_inputs.HASH_A on round 8.
    pub is_hash_a: bool,
    /// This row produces HASH_B.
    pub is_hash_b: bool,
    /// This row produces HASH_JACKPOT (the tile-state-hash output
    /// that gets compared against the difficulty target outside the
    /// circuit).
    pub is_hash_jackpot: bool,
    /// CV is the chain-pinned `COMMITMENT_HASH` (a.k.a. `s_A` in
    /// the M10.1b notation). Otherwise CV is `JOB_KEY` (= κ).
    pub cv_is_commitment: bool,
}

impl Default for BlakeRoundLogic {
    fn default() -> Self {
        Self {
            data_source: MessageDataType::None,
            blake3_tweak: None,
            // round 1 is the most permissive choice — it admits any
            // tweak (others would need specific predecessors).
            round_idx: 1,
            idx_of_row_whence_to_read_cv: None,
            is_hash_a: false,
            is_hash_b: false,
            is_hash_jackpot: false,
            cv_is_commitment: false,
        }
    }
}

impl BlakeRoundLogic {
    /// Should this row use `JOB_KEY` (= κ) as its CV input? Pearl's
    /// logic: yes when neither a CV-routing index nor commitment-
    /// hash is set, OR when the data source is a previous-CV row
    /// (the previous row's CV_OUT is already a hash output, so the
    /// CV input for the new compression is the original κ).
    pub fn is_use_job_key(&self) -> bool {
        (self.idx_of_row_whence_to_read_cv.is_none()
            || matches!(self.data_source, MessageDataType::PreviousCv { .. }))
            && !self.cv_is_commitment
    }

    /// Should this row use `COMMITMENT_HASH` as its CV input?
    pub fn is_use_commitment_hash(&self) -> bool {
        self.cv_is_commitment
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_round_logic_uses_job_key() {
        let l = BlakeRoundLogic::default();
        assert!(l.is_use_job_key());
        assert!(!l.is_use_commitment_hash());
    }

    #[test]
    fn cv_is_commitment_switches_cv_source() {
        let l = BlakeRoundLogic {
            cv_is_commitment: true,
            ..BlakeRoundLogic::default()
        };
        assert!(!l.is_use_job_key());
        assert!(l.is_use_commitment_hash());
    }

    #[test]
    fn previous_cv_with_routing_index_uses_job_key() {
        let l = BlakeRoundLogic {
            data_source: MessageDataType::PreviousCv { source_row_idx: 17 },
            idx_of_row_whence_to_read_cv: Some(17),
            ..BlakeRoundLogic::default()
        };
        // Even though we set a CV-routing index, the data source is
        // a previous CV → caller wants JOB_KEY as the *CV input*
        // (the previous CV is being loaded as the *message*).
        assert!(l.is_use_job_key());
    }

    #[test]
    fn cv_routing_without_previous_cv_does_not_use_job_key() {
        let l = BlakeRoundLogic {
            data_source: MessageDataType::Matrix { dword_offset: 0 },
            idx_of_row_whence_to_read_cv: Some(5),
            ..BlakeRoundLogic::default()
        };
        // CV-routing index set + data source isn't a previous CV →
        // the row reads CV from row 5 via lookup, NOT JOB_KEY.
        assert!(!l.is_use_job_key());
        assert!(!l.is_use_commitment_hash());
    }

    #[test]
    fn round_idx_default_is_one() {
        let l = BlakeRoundLogic::default();
        assert_eq!(l.round_idx, 1);
    }

    #[test]
    fn message_data_type_default_is_none() {
        let m = MessageDataType::default();
        assert!(matches!(m, MessageDataType::None));
    }
}
