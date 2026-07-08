//! Input chip — ties unpacked matrix bytes / unpacked noise to the
//! packed `NOISED_PACKED` column read by the matmul + BLAKE3 chips.
//!
//! **Pearl ISC.** This file is derived from Pearl source code
//! (Copyright (c) 2025-2026 Pearl Research Labs; 2015-2016 The Decred
//! developers); see `crates/ai-pow-zk/LICENSE-PEARL` for the full
//! permission notice.
//!
//! Port of `Pearl zk-pow chip/input/constraints.rs`.
//!
//! ## Columns the chip reads
//!
//! Per row, the chip consumes (from `composite_layout`):
//!
//!   * `MAT_UNPACK[0..64]`     — 64 × i8 matrix bytes (range-checked by IRange8).
//!   * `UINT8_DATA[0..64]`     — 64 × u8 message-input bytes (range-checked by URange8).
//!   * `NOISE_PACKED_PREP`     — preprocessed noise, packed via base 129.
//!   * `NOISE_UNPACK[0..64]`   — 64 × i7+1 noise bytes (range-checked by IRange7P1).
//!   * `NOISED_PACKED[0..16]`  — 16 cells holding (matrix + noise) packed 4 i8/Goldilocks via base 256.
//!
//! ## Property enforced
//!
//! Two constraints fire on every row (no selector — they hold
//! identically):
//!
//! ```text
//!   NOISE_PACKED_PREP == polyval(NOISE_UNPACK, base=NOISE_PACKING_BASE=129)
//!
//!   for i in 0..NOISED_PACKED_LEN (= 16):
//!     NOISED_PACKED[i] == polyval(MAT_UNPACK[i*4..(i+1)*4], base=256)
//!                       + polyval(NOISE_UNPACK[i*4..(i+1)*4], base=256)
//! ```
//!
//! `UINT8_DATA` itself is not constrained here — its semantic
//! relation to `MAT_UNPACK` (under `IS_MSG_MAT`) is enforced by the
//! I8U8 conversion lookup (Phase 11).
//!
//! ## Why this is the cryptographic-binding pivot
//!
//! `NOISED_PACKED` is the canonical noised-matrix store that:
//!   * the matmul chip reads via `A_NOISED` / `B_NOISED` RAM lookups,
//!   * the BLAKE3 chip reads (after i8→u8 conversion) for h_a / h_b
//!     leaf-hashing rows.
//!
//! The input chip is what forces `NOISED_PACKED` to actually equal
//! `matrix + noise`, where `matrix` and `noise` are the unpacked
//! per-row bytes. Subsequent RAM lookups bind the matmul reads and
//! BLAKE3 leaf inputs to *the same* `NOISED_PACKED` row.

use p3_air::{AirBuilder, WindowAccess};
use p3_field::PrimeCharacteristicRing;

use crate::composite_layout::{
    BYTES_PER_GOLDILOCKS, MAT_UNPACK_LEN, MAT_UNPACK_START, NOISED_PACKED_LEN, NOISED_PACKED_START,
    NOISE_PACKED_PREP, NOISE_PACKED_PREP_LEN, NOISE_UNPACK_LEN, NOISE_UNPACK_START,
};

/// Noise-element range: `[-64, 64]` has 129 values, so the packing
/// base for `polyval` over noise bytes is 129.
pub const NOISE_PACKING_BASE: i32 = 129;

/// Packing base for matrix bytes (i8 → polyval with base 256).
pub const MATRIX_PACKING_BASE: i32 = 256;

#[derive(Debug, Default, Clone, Copy)]
pub struct InputChip;

impl InputChip {
    pub const fn new() -> Self {
        Self
    }

    /// Evaluate `polyval([x0, x1, ..., xn-1], base) = x0 + x1*base +
    /// x2*base^2 + … + x_{n-1} * base^{n-1}` over a slice of trace
    /// variables.
    fn polyval<AB: AirBuilder>(coeffs: &[AB::Var], base: i32) -> AB::Expr {
        let base_f = <AB::F as PrimeCharacteristicRing>::from_i32(base);
        let mut acc: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
        let mut pow: AB::F = <AB::F as PrimeCharacteristicRing>::ONE;
        for &c in coeffs {
            acc = acc + c * pow.clone();
            pow = pow * base_f.clone();
        }
        acc
    }

    pub fn eval<AB: AirBuilder>(&self, builder: &mut AB) {
        let main = builder.main();
        let cur = main.current_slice();

        // §4.C.2 c-exact (cx.2-mat-input/X1): the blocks are
        // widened (MAT_UNPACK/NOISE_UNPACK 8→64, NOISED_PACKED
        // 2→16, NOISE_PACKED_PREP 1→8). InputChip integrity now
        // covers all **8 sub-slices** of a (co-located) leaf
        // round-0 row's 64-byte block — *unconditionally* (not
        // gated). ZERO-BLAST: on every current trace the added
        // cells are 0, so each added sub-slice's eqn is
        // `0 == polyval(0,·) (+ polyval(0,·))` ⇒ holds, and the
        // CRIT-1 `NOISE_PACKED_PREP[1..8]` pin is `0==0`;
        // sub-slice 0 / cell 0..2 are the pre-cx.2 constraints
        // unchanged ⇒ byte-identical. Reuses M-S1's *exact*
        // polyval packing ×8 (no i8/u8 reconciliation).
        let mat_unpack: Vec<AB::Var> = (0..MAT_UNPACK_LEN)
            .map(|i| cur[MAT_UNPACK_START + i])
            .collect();
        let noise_unpack: Vec<AB::Var> = (0..NOISE_UNPACK_LEN)
            .map(|i| cur[NOISE_UNPACK_START + i])
            .collect();
        let noised_packed: Vec<AB::Var> = (0..NOISED_PACKED_LEN)
            .map(|i| cur[NOISED_PACKED_START + i])
            .collect();

        // 1. Per 8-byte sub-slice s: NOISE_PACKED_PREP[s] ==
        //    polyval(NOISE_UNPACK[8s..8s+8], base=129).
        const SUBSLICE: usize = 8;
        for s in 0..NOISE_PACKED_PREP_LEN {
            let chunk = &noise_unpack[s * SUBSLICE..s * SUBSLICE + SUBSLICE];
            let repacked = Self::polyval::<AB>(chunk, NOISE_PACKING_BASE);
            builder.assert_eq(cur[NOISE_PACKED_PREP + s], repacked);
        }

        // 2. For each NOISED_PACKED cell i (2 per sub-slice ×8 =
        //    16): NOISED_PACKED[i] == polyval(MAT_UNPACK[4i..],256)
        //    + polyval(NOISE_UNPACK[4i..],256).
        for i in 0..NOISED_PACKED_LEN {
            let mat_chunk = &mat_unpack[i * BYTES_PER_GOLDILOCKS..(i + 1) * BYTES_PER_GOLDILOCKS];
            let noise_chunk =
                &noise_unpack[i * BYTES_PER_GOLDILOCKS..(i + 1) * BYTES_PER_GOLDILOCKS];
            let mat_packed = Self::polyval::<AB>(mat_chunk, MATRIX_PACKING_BASE);
            let noise_packed = Self::polyval::<AB>(noise_chunk, MATRIX_PACKING_BASE);
            builder.assert_eq(noised_packed[i].clone(), mat_packed + noise_packed);
        }
    }

    /// Fill the input chip's trace cells for one legacy 8-byte
    /// matrix row. cx.2 rows that carry a full 64-byte block are
    /// filled by the composite trace co-location path; this helper
    /// keeps tests and pure store rows on the first sub-slice and
    /// leaves the remaining sub-slices at zero.
    /// `mat_bytes` and `noise_bytes` are i7 values in `[-64, 64]`.
    pub fn fill_row(&self, mat_bytes: &[i8; 8], noise_bytes: &[i8; 8], row: &mut [crate::Val]) {
        use p3_field::integers::QuotientMap;

        // MAT_UNPACK: legacy helpers write one 8-byte sub-slice.
        for i in 0..mat_bytes.len() {
            row[MAT_UNPACK_START + i] =
                <crate::Val as QuotientMap<i32>>::from_int(mat_bytes[i] as i32);
        }
        // NOISE_UNPACK: legacy helpers write one 8-byte sub-slice.
        for i in 0..noise_bytes.len() {
            row[NOISE_UNPACK_START + i] =
                <crate::Val as QuotientMap<i32>>::from_int(noise_bytes[i] as i32);
        }
        // NOISE_PACKED_PREP: polyval of NOISE_UNPACK at base 129.
        let mut noise_packed: i64 = 0;
        let mut pow: i64 = 1;
        for &b in noise_bytes {
            noise_packed += (b as i64) * pow;
            pow *= NOISE_PACKING_BASE as i64;
        }
        row[NOISE_PACKED_PREP] = <crate::Val as QuotientMap<i64>>::from_int(noise_packed);

        // NOISED_PACKED: per-chunk sum of polyval(mat_chunk, 256) +
        // polyval(noise_chunk, 256).
        for i in 0..(mat_bytes.len() / BYTES_PER_GOLDILOCKS) {
            let mut mat_packed: i64 = 0;
            let mut noise_p: i64 = 0;
            let mut p256: i64 = 1;
            for j in 0..BYTES_PER_GOLDILOCKS {
                let idx = i * BYTES_PER_GOLDILOCKS + j;
                mat_packed += (mat_bytes[idx] as i64) * p256;
                noise_p += (noise_bytes[idx] as i64) * p256;
                p256 *= MATRIX_PACKING_BASE as i64;
            }
            row[NOISED_PACKED_START + i] =
                <crate::Val as QuotientMap<i64>>::from_int(mat_packed + noise_p);
        }
    }
}

#[cfg(test)]
mod tests {
    use p3_air::{Air, AirBuilder, BaseAir};
    use p3_field::integers::QuotientMap;
    use p3_matrix::dense::RowMajorMatrix;
    use p3_uni_stark::{prove, verify};

    use super::*;
    use crate::circuit::{build_stark_config, AiPowStarkConfig, CircuitConfig};
    use crate::composite_layout::TOTAL_TRACE_WIDTH;
    use crate::params::ZkParams;
    use crate::Val;

    fn test_zk_params() -> ZkParams {
        ZkParams {
            m: 8,
            k: 16,
            n: 8,
            noise_rank: 2,
            tile: 2,
            difficulty_bits: 0,
        }
    }

    #[derive(Debug, Default)]
    struct InputOnlyAir;

    impl<F> BaseAir<F> for InputOnlyAir {
        fn width(&self) -> usize {
            TOTAL_TRACE_WIDTH
        }
    }

    impl<AB: AirBuilder> Air<AB> for InputOnlyAir {
        fn eval(&self, builder: &mut AB) {
            InputChip::new().eval(builder);
        }
    }

    /// Build a 16-row trace populated by the InputChip for the same
    /// (mat_bytes, noise_bytes) on every row. Constraint holds
    /// independently on each row, so a uniform fill is a valid trace.
    fn build_valid_trace_rows(rows: usize, mat: &[i8; 8], noise: &[i8; 8]) -> RowMajorMatrix<Val> {
        let chip = InputChip::new();
        let mut flat = vec![Val::default(); rows * TOTAL_TRACE_WIDTH];
        for r in 0..rows {
            let row = &mut flat[r * TOTAL_TRACE_WIDTH..(r + 1) * TOTAL_TRACE_WIDTH];
            chip.fill_row(mat, noise, row);
        }
        RowMajorMatrix::new(flat, TOTAL_TRACE_WIDTH)
    }

    #[test]
    fn noise_packing_base_is_129() {
        assert_eq!(NOISE_PACKING_BASE, 129);
    }

    #[test]
    fn matrix_packing_base_is_256() {
        assert_eq!(MATRIX_PACKING_BASE, 256);
    }

    /// Reference: fill_row writes correct values for a known input.
    /// Anchor with hand-computed values.
    #[test]
    fn fill_row_packs_correctly_simple() {
        let chip = InputChip::new();
        let mat: [i8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
        let noise: [i8; 8] = [0; 8];
        let mut row = vec![Val::default(); TOTAL_TRACE_WIDTH];
        chip.fill_row(&mat, &noise, &mut row);

        use p3_field::PrimeField64;
        // NOISE_PACKED_PREP = polyval([0,...,0], 129) = 0.
        assert_eq!(row[NOISE_PACKED_PREP].as_canonical_u64(), 0);
        // NOISED_PACKED[0] = polyval([1,2,3,4], 256) + 0
        //                  = 1 + 2*256 + 3*65536 + 4*16777216
        //                  = 1 + 512 + 196608 + 67108864 = 67305985
        assert_eq!(row[NOISED_PACKED_START].as_canonical_u64(), 67305985);
    }

    #[test]
    fn prove_and_verify_valid_input_trace() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let air = InputOnlyAir;
        let mat: [i8; 8] = [1, -1, 2, -2, 3, -3, 4, -4];
        let noise: [i8; 8] = [0, 1, 0, -1, 0, 1, 0, -1];
        let trace = build_valid_trace_rows(16, &mat, &noise);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &air, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &air, &proof, &[])
            .expect("valid input-chip trace must verify");
    }

    /// Property: NOISED_PACKED must equal pack(MAT) + pack(NOISE).
    /// Tamper one NOISED_PACKED cell → reject.
    #[test]
    fn rejects_wrong_noised_packed() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let air = InputOnlyAir;
        let mat: [i8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
        let noise: [i8; 8] = [0; 8];
        let mut trace = build_valid_trace_rows(16, &mat, &noise);
        // Change NOISED_PACKED[0] at row 5 to a wrong value.
        trace.values[5 * TOTAL_TRACE_WIDTH + NOISED_PACKED_START] =
            <Val as QuotientMap<i64>>::from_int(0);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &air, trace, &[]);
        assert!(verify::<AiPowStarkConfig, _>(&cfg, &air, &proof, &[]).is_err());
    }

    /// Property: NOISE_PACKED_PREP must equal polyval(NOISE_UNPACK,
    /// 129). Tamper → reject.
    #[test]
    fn rejects_wrong_noise_packed_prep() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let air = InputOnlyAir;
        let mat: [i8; 8] = [0; 8];
        let noise: [i8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
        let mut trace = build_valid_trace_rows(16, &mat, &noise);
        // Change NOISE_PACKED_PREP at row 3 to a wrong value.
        trace.values[3 * TOTAL_TRACE_WIDTH + NOISE_PACKED_PREP] =
            <Val as QuotientMap<i32>>::from_int(0);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &air, trace, &[]);
        assert!(verify::<AiPowStarkConfig, _>(&cfg, &air, &proof, &[]).is_err());
    }

    /// Property: noised = mat + noise per byte. Tamper a mat byte
    /// without updating NOISED_PACKED → reject.
    #[test]
    fn rejects_tampered_mat_byte() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let air = InputOnlyAir;
        let mat: [i8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
        let noise: [i8; 8] = [0; 8];
        let mut trace = build_valid_trace_rows(16, &mat, &noise);
        // Change MAT_UNPACK[2] at row 0 (was 3, set to 10). NOISED_PACKED
        // still reflects mat=[1,2,3,4] so the constraint fails.
        trace.values[MAT_UNPACK_START + 2] = <Val as QuotientMap<i32>>::from_int(10);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &air, trace, &[]);
        assert!(verify::<AiPowStarkConfig, _>(&cfg, &air, &proof, &[]).is_err());
    }

    /// Reference: an adversary trying to substitute different
    /// matrix bytes while keeping NOISED_PACKED unchanged is caught
    /// by the constraint that ties them together. This is the
    /// **core property** that makes the matmul ↔ BLAKE3 RAM-lookup
    /// linkage cryptographically meaningful.
    #[test]
    fn cannot_diverge_mat_from_noised_packed() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let air = InputOnlyAir;
        let mat: [i8; 8] = [10, 20, 30, 40, 50, 60, 70, 80];
        let noise: [i8; 8] = [-5; 8];
        let mut trace = build_valid_trace_rows(16, &mat, &noise);
        // Tamper: replace MAT_UNPACK[5] with a value off by 1 at
        // row 7. NOISED_PACKED still reflects original → fails.
        trace.values[7 * TOTAL_TRACE_WIDTH + MAT_UNPACK_START + 5] =
            <Val as QuotientMap<i32>>::from_int(61); // was 60
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &air, trace, &[]);
        assert!(verify::<AiPowStarkConfig, _>(&cfg, &air, &proof, &[]).is_err());
    }

    /// Reference: noise that includes -64 values still produces a
    /// valid trace (covers Pearl's full i7+1 noise range).
    #[test]
    fn handles_boundary_noise_values() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let air = InputOnlyAir;
        let mat: [i8; 8] = [0; 8];
        let noise: [i8; 8] = [-64, -63, -1, 0, 1, 62, 63, 64];
        let trace = build_valid_trace_rows(16, &mat, &noise);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &air, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &air, &proof, &[])
            .expect("boundary-noise trace must verify");
    }
}
