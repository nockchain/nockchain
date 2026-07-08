//! Public inputs for the M10.1c composite proof.
//!
//! Pins the cells the composite AIR exposes externally. The
//! verifier checks these against the prover's claim via the
//! [`CompositeFullAir`] / [`CompositeFullAirWithLookups`]
//! constraints on the trace's last row, plus selector-gated
//! per-row constraints for `HASH_A` / `HASH_B`.
//!
//! ## Layout (60 field elements)
//!
//! ```text
//!   index 0..4   : final CUMSUM_TILE (4 i32 cells, signed —
//!                  encoded canonically into Goldilocks; the
//!                  trace generator's `fill_cumsum_passthrough`
//!                  guarantees every row from the chain end
//!                  through the last row holds this value)
//!   index 4..20  : final JACKPOT_MSG (16 u32 cells, threaded
//!                  by `fill_jackpot_passthrough`)
//!   index 20..28 : HASH_A — 8 u32 words encoding the 256-bit
//!                  `BLAKE3-keyed(pad(A), κ)` matrix commitment.
//!                  Bound to the row where `IS_HASH_A = 1` via
//!                  `IS_HASH_A · (CV_OUT[i] − PI_HASH_A[i]) = 0`.
//!   index 28..36 : HASH_B — 8 u32 words for matrix B.
//!   index 36..44 : JOB_KEY — κ bound to BLAKE3 key input rows.
//!   index 44..52 : COMMITMENT_HASH — jackpot key. Native Nockchain
//!                  AI-PoW uses `pow_key_for_nonce(s_A, nonce)`;
//!                  Pearl-compatible merge mining uses Pearl's `s_A`.
//!   index 52..60 : HASH_JACKPOT — jackpot digest checked against target.
//! ```
//!
//! ## Deferred from PIs
//!
//! - **Final CV_OUT.** Per-row only meaningful on finalize rows
//!   (where `IS_LAST_ROUND = 1`). Without a trace-side mechanism
//!   to thread the "current" CV through to the last row, binding
//!   it as a PI would always read zero on most traces. Add when a
//!   downstream protocol needs the final hash output.

use p3_field::integers::QuotientMap;
use p3_field::PrimeField64;
use serde::{Deserialize, Serialize};

use crate::composite_layout::{
    CUMSUM_TILE_START, CV_IN_LEN, CV_IN_START, CV_OUT_LEN, CV_OUT_START, IS_HASH_A, IS_HASH_B,
    IS_HASH_JACKPOT, IS_USE_COMMITMENT_HASH, IS_USE_JOB_KEY, JACKPOT_MSG_START, JACKPOT_SIZE,
    TOTAL_TRACE_WIDTH,
};
use crate::composite_trace::CompositeTrace;
use crate::Val;

/// Number of field elements in the public-input vector.
///
/// Layout (Pearl Layer-0 STARK canonical set + our M10.1c
/// cumsum/jackpot passthrough kept for backward-compat):
/// `cumsum(4) + jackpot(16) + hash_a(8) + hash_b(8) +
///  job_key(8) + commitment_hash(8) + hash_jackpot(8)` = 60.
///
/// `job_key`, `commitment_hash`, `hash_jackpot` mirror Pearl's
/// `pearl_layout::{JOB_KEY, COMMITMENT_HASH, HASH_JACKPOT}`
/// (`pearl_circuit.rs:12-22`). They tie the proof to the
/// chain-pinned block-header key + noise seed and bind the
/// tile-state keyed hash — without them the SNARK proves correct
/// sub-computations but not a *proof of work*.
pub const NUM_PUBLIC_VALUES: usize = 4 + JACKPOT_SIZE + 5 * CV_OUT_LEN; // 60

/// PI layout offsets (within the `Vec<Val>` of length
/// [`NUM_PUBLIC_VALUES`]).
pub const PI_CUMSUM_OFFSET: usize = 0;
pub const PI_CUMSUM_LEN: usize = 4; // TILE_H × TILE_H
pub const PI_JACKPOT_OFFSET: usize = PI_CUMSUM_OFFSET + PI_CUMSUM_LEN;
pub const PI_JACKPOT_LEN: usize = JACKPOT_SIZE;
pub const PI_HASH_A_OFFSET: usize = PI_JACKPOT_OFFSET + PI_JACKPOT_LEN;
pub const PI_HASH_A_LEN: usize = CV_OUT_LEN; // 8 u32 words
pub const PI_HASH_B_OFFSET: usize = PI_HASH_A_OFFSET + PI_HASH_A_LEN;
pub const PI_HASH_B_LEN: usize = CV_OUT_LEN;
/// Pearl `JOB_KEY` = BLAKE3(block-header ‖ mining-config) = κ.
/// Bound to `CV_IN` on rows where `IS_USE_JOB_KEY = 1`.
pub const PI_JOB_KEY_OFFSET: usize = PI_HASH_B_OFFSET + PI_HASH_B_LEN;
pub const PI_JOB_KEY_LEN: usize = CV_IN_LEN;
/// Legacy Pearl `COMMITMENT_HASH` public-input slot. Native Nockchain AI-PoW
/// carries the nonce-derived jackpot key `pow_key_for_nonce(s_a, nonce)` here;
/// Pearl-compatible merge mining carries Pearl's `s_A` directly. The value is
/// bound to `CV_IN` on rows where `IS_USE_COMMITMENT_HASH = 1`.
pub const PI_COMMITMENT_HASH_OFFSET: usize = PI_JOB_KEY_OFFSET + PI_JOB_KEY_LEN;
pub const PI_COMMITMENT_HASH_LEN: usize = CV_IN_LEN;
/// Pearl `HASH_JACKPOT` = BLAKE3(JACKPOT_MSG, key=COMMITMENT_HASH)
/// — the tile-state keyed hash compared against the difficulty
/// target. Bound to `CV_OUT` on the row where
/// `IS_HASH_JACKPOT = 1`.
pub const PI_HASH_JACKPOT_OFFSET: usize = PI_COMMITMENT_HASH_OFFSET + PI_COMMITMENT_HASH_LEN;
pub const PI_HASH_JACKPOT_LEN: usize = CV_OUT_LEN;

/// Typed view of the public inputs the composite AIR commits to.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CompositePublicInputs {
    /// Final value of `CUMSUM_TILE[0..4]` (signed 32-bit ints).
    pub cumsum: [i32; PI_CUMSUM_LEN],
    /// Final value of `JACKPOT_MSG[0..16]` (32-bit values).
    pub jackpot: [u32; JACKPOT_SIZE],
    /// BLAKE3 keyed-hash of `pad(A_row_major)` — Pearl §4.3
    /// matrix-A commitment. 8 u32 words = 256 bits. Bound to the
    /// row where `IS_HASH_A = 1`.
    pub hash_a: [u32; PI_HASH_A_LEN],
    /// BLAKE3 keyed-hash of `pad(B_col_major)` — Pearl §4.3
    /// matrix-B commitment.
    pub hash_b: [u32; PI_HASH_B_LEN],
    /// Pearl `JOB_KEY` (κ): chain-pinned BLAKE3(block-header ‖
    /// mining-config). Bound to `CV_IN` on `IS_USE_JOB_KEY` rows.
    pub job_key: [u32; PI_JOB_KEY_LEN],
    /// Legacy Pearl `COMMITMENT_HASH` slot. Native Nockchain AI-PoW uses it
    /// for the nonce-derived jackpot key; Pearl-compatible merge mining uses
    /// it for Pearl's `s_A`. Bound to `CV_IN` on `IS_USE_COMMITMENT_HASH`
    /// rows.
    pub commitment_hash: [u32; PI_COMMITMENT_HASH_LEN],
    /// `HASH_JACKPOT` = BLAKE3(JACKPOT_MSG, key=COMMITMENT_HASH slot). This
    /// is the tile-state keyed hash that the difficulty target is compared
    /// against. Bound to `CV_OUT` on the `IS_HASH_JACKPOT` row.
    pub hash_jackpot: [u32; PI_HASH_JACKPOT_LEN],
}

impl Default for CompositePublicInputs {
    fn default() -> Self {
        Self::zero()
    }
}

impl CompositePublicInputs {
    /// All-zero PIs. Matches a baseline trace (no chip activity).
    pub const fn zero() -> Self {
        Self {
            cumsum: [0; PI_CUMSUM_LEN],
            jackpot: [0; JACKPOT_SIZE],
            hash_a: [0; PI_HASH_A_LEN],
            hash_b: [0; PI_HASH_B_LEN],
            job_key: [0; PI_JOB_KEY_LEN],
            commitment_hash: [0; PI_COMMITMENT_HASH_LEN],
            hash_jackpot: [0; PI_HASH_JACKPOT_LEN],
        }
    }

    /// Serialise to the `Vec<Val>` shape `prove_batch` /
    /// `verify_batch` expect.
    pub fn to_vec(&self) -> Vec<Val> {
        let mut out = Vec::with_capacity(NUM_PUBLIC_VALUES);
        for &v in &self.cumsum {
            out.push(<Val as QuotientMap<i64>>::from_int(v as i64));
        }
        for &v in &self.jackpot {
            out.push(<Val as QuotientMap<u64>>::from_int(v as u64));
        }
        for &v in &self.hash_a {
            out.push(<Val as QuotientMap<u64>>::from_int(v as u64));
        }
        for &v in &self.hash_b {
            out.push(<Val as QuotientMap<u64>>::from_int(v as u64));
        }
        for &v in &self.job_key {
            out.push(<Val as QuotientMap<u64>>::from_int(v as u64));
        }
        for &v in &self.commitment_hash {
            out.push(<Val as QuotientMap<u64>>::from_int(v as u64));
        }
        for &v in &self.hash_jackpot {
            out.push(<Val as QuotientMap<u64>>::from_int(v as u64));
        }
        debug_assert_eq!(out.len(), NUM_PUBLIC_VALUES);
        out
    }

    /// Read the trace and derive the PI values it would commit to.
    /// CUMSUM_TILE and JACKPOT_MSG come from the last row (after
    /// `fill_*_passthrough`); HASH_A and HASH_B are read from the
    /// CV_OUT cells of the row where `IS_HASH_A` / `IS_HASH_B`
    /// is set. If no such row exists (baseline trace), the hash
    /// PI fields are zero.
    pub fn derive_from_trace(trace: &CompositeTrace) -> Self {
        Self::derive_from_matrix(&trace.matrix)
    }

    /// Variant for callers that hold the trace as a
    /// `RowMajorMatrix<Val>` directly (e.g. test code in
    /// `composite_full_air` and `composite_trace`).
    pub fn derive_from_matrix(matrix: &p3_matrix::dense::RowMajorMatrix<Val>) -> Self {
        let n = matrix.values.len() / TOTAL_TRACE_WIDTH;
        let last_base = (n - 1) * TOTAL_TRACE_WIDTH;

        let cumsum: [i32; PI_CUMSUM_LEN] = core::array::from_fn(|i| {
            let raw = matrix.values[last_base + CUMSUM_TILE_START + i].as_canonical_u64();
            goldilocks_to_i32(raw)
        });
        let jackpot: [u32; JACKPOT_SIZE] = core::array::from_fn(|i| {
            matrix.values[last_base + JACKPOT_MSG_START + i].as_canonical_u64() as u32
        });

        let hash_a = read_cv_at_selector(matrix, n, IS_HASH_A, CV_OUT_START);
        let hash_b = read_cv_at_selector(matrix, n, IS_HASH_B, CV_OUT_START);
        let hash_jackpot = read_cv_at_selector(matrix, n, IS_HASH_JACKPOT, CV_OUT_START);
        let job_key = read_cv_at_selector(matrix, n, IS_USE_JOB_KEY, CV_IN_START);
        let commitment_hash = read_cv_at_selector(matrix, n, IS_USE_COMMITMENT_HASH, CV_IN_START);

        Self {
            cumsum,
            jackpot,
            hash_a,
            hash_b,
            job_key,
            commitment_hash,
            hash_jackpot,
        }
    }
}

/// Scan the trace for the (at most one) row where `selector_col`
/// equals 1, and read 8 CV words starting at `cv_start` from that
/// row (`CV_OUT_START` for hash outputs, `CV_IN_START` for the
/// JOB_KEY / COMMITMENT_HASH key inputs). Returns zeros if no
/// such row is found (baseline trace).
///
/// The selector-gated AIR constraint enforces the bound value
/// equals the PI on every firing row; the trace generator places
/// exactly one such row per matrix in production.
fn read_cv_at_selector(
    matrix: &p3_matrix::dense::RowMajorMatrix<Val>,
    n: usize,
    selector_col: usize,
    cv_start: usize,
) -> [u32; CV_OUT_LEN] {
    for r in 0..n {
        let base = r * TOTAL_TRACE_WIDTH;
        if matrix.values[base + selector_col].as_canonical_u64() == 1 {
            return core::array::from_fn(|i| {
                matrix.values[base + cv_start + i].as_canonical_u64() as u32
            });
        }
    }
    [0; CV_OUT_LEN]
}

/// Map a Goldilocks raw value back to i32 (preserving the two's-
/// complement representation that `from_int(i32)` uses). Used
/// when extracting signed cumsum cells.
fn goldilocks_to_i32(raw: u64) -> i32 {
    const GOLDILOCKS_P: u64 = 0xFFFF_FFFF_0000_0001;
    if raw > GOLDILOCKS_P / 2 {
        let signed = raw as i128 - GOLDILOCKS_P as i128;
        signed as i32
    } else {
        raw as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_round_trips() {
        let pis = CompositePublicInputs::zero();
        let v = pis.to_vec();
        assert_eq!(v.len(), NUM_PUBLIC_VALUES);
        for x in v {
            assert_eq!(x, Val::default());
        }
    }

    #[test]
    fn derive_from_baseline_trace_is_zero() {
        let trace = CompositeTrace::baseline_min();
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        assert_eq!(pis, CompositePublicInputs::zero());
    }

    #[test]
    fn derive_picks_up_threaded_cumsum() {
        let mut trace = CompositeTrace::baseline_min();
        let cumsum = [1, -2, 3, -4];
        trace.fill_cumsum_passthrough(0, &cumsum);
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        assert_eq!(pis.cumsum, cumsum);
        assert_eq!(pis.jackpot, [0u32; JACKPOT_SIZE]);
    }

    #[test]
    fn derive_picks_up_threaded_jackpot() {
        let mut trace = CompositeTrace::baseline_min();
        let jp: [u32; JACKPOT_SIZE] = core::array::from_fn(|i| (i as u32 + 1) * 0xCAFE);
        trace.fill_jackpot_passthrough(0, &jp);
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        assert_eq!(pis.cumsum, [0; 4]);
        assert_eq!(pis.jackpot, jp);
    }

    #[test]
    fn to_vec_layout_is_stable() {
        let pis = CompositePublicInputs {
            cumsum: [10, 20, 30, 40],
            jackpot: core::array::from_fn(|i| 100 + i as u32),
            ..CompositePublicInputs::zero()
        };
        let v = pis.to_vec();
        use p3_field::PrimeField64;
        for i in 0..PI_CUMSUM_LEN {
            assert_eq!(
                v[PI_CUMSUM_OFFSET + i].as_canonical_u64(),
                pis.cumsum[i] as u64
            );
        }
        for i in 0..PI_JACKPOT_LEN {
            assert_eq!(
                v[PI_JACKPOT_OFFSET + i].as_canonical_u64(),
                pis.jackpot[i] as u64
            );
        }
    }

    #[test]
    fn negative_cumsum_round_trips() {
        let pis = CompositePublicInputs {
            cumsum: [-1, -1000, i32::MIN, i32::MAX],
            ..CompositePublicInputs::zero()
        };
        let v = pis.to_vec();
        use p3_field::PrimeField64;
        // -1 in Goldilocks = p - 1.
        assert_eq!(v[0].as_canonical_u64(), 0xFFFF_FFFF_0000_0001u64 - 1);
        // Round-trip via goldilocks_to_i32.
        assert_eq!(goldilocks_to_i32(v[0].as_canonical_u64()), -1);
        assert_eq!(goldilocks_to_i32(v[1].as_canonical_u64()), -1000);
        assert_eq!(goldilocks_to_i32(v[2].as_canonical_u64()), i32::MIN);
        assert_eq!(goldilocks_to_i32(v[3].as_canonical_u64()), i32::MAX);
    }

    #[test]
    fn num_public_values_includes_hash_slots() {
        // 4 cumsum + 16 jackpot + 8 hash_a + 8 hash_b
        // + 8 job_key + 8 commitment_hash + 8 hash_jackpot = 60.
        assert_eq!(NUM_PUBLIC_VALUES, 60);
        assert_eq!(PI_HASH_A_OFFSET, 20);
        assert_eq!(PI_HASH_B_OFFSET, 28);
        assert_eq!(PI_JOB_KEY_OFFSET, 36);
        assert_eq!(PI_COMMITMENT_HASH_OFFSET, 44);
        assert_eq!(PI_HASH_JACKPOT_OFFSET, 52);
        assert_eq!(PI_HASH_A_LEN, 8);
        assert_eq!(PI_HASH_B_LEN, 8);
        assert_eq!(PI_HASH_JACKPOT_LEN, 8);
    }

    #[test]
    fn hash_a_b_round_trip_via_to_vec() {
        let pis = CompositePublicInputs {
            hash_a: [
                0x01020304, 0x05060708, 0x090A0B0C, 0x0D0E0F10, 0xDEADBEEF, 0xFEEDFACE, 0xCAFEBABE,
                0x12345678,
            ],
            hash_b: [
                0x11111111, 0x22222222, 0x33333333, 0x44444444, 0x55555555, 0x66666666, 0x77777777,
                0x88888888,
            ],
            ..CompositePublicInputs::zero()
        };
        let v = pis.to_vec();
        use p3_field::PrimeField64;
        for i in 0..PI_HASH_A_LEN {
            assert_eq!(
                v[PI_HASH_A_OFFSET + i].as_canonical_u64(),
                pis.hash_a[i] as u64,
                "hash_a[{i}]"
            );
            assert_eq!(
                v[PI_HASH_B_OFFSET + i].as_canonical_u64(),
                pis.hash_b[i] as u64,
                "hash_b[{i}]"
            );
        }
    }

    #[test]
    fn derive_picks_up_hash_a_at_selector_row() {
        // Manually plant IS_HASH_A=1 + CV_OUT pattern on row 3
        // and check derive_from_matrix surfaces it.
        let trace = CompositeTrace::baseline_min();
        let n = trace.matrix.values.len() / TOTAL_TRACE_WIDTH;
        let mut planted = trace.clone();
        let r = 3usize.min(n - 1);
        let base = r * TOTAL_TRACE_WIDTH;
        use p3_field::integers::QuotientMap;
        planted.matrix.values[base + IS_HASH_A] = <Val as QuotientMap<u64>>::from_int(1);
        let expected: [u32; CV_OUT_LEN] = core::array::from_fn(|i| (i as u32 + 1) * 0x0F0F0F);
        for i in 0..CV_OUT_LEN {
            planted.matrix.values[base + CV_OUT_START + i] =
                <Val as QuotientMap<u64>>::from_int(expected[i] as u64);
        }
        let pis = CompositePublicInputs::derive_from_matrix(&planted.matrix);
        assert_eq!(pis.hash_a, expected);
        assert_eq!(pis.hash_b, [0; PI_HASH_B_LEN], "no IS_HASH_B set");
    }
}
