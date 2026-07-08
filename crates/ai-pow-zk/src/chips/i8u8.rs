//! Signed↔unsigned 8-bit conversion table chip.
//!
//! **Pearl ISC.** This file is derived from Pearl source code
//! (Copyright (c) 2025-2026 Pearl Research Labs; 2015-2016 The Decred
//! developers); see `crates/ai-pow-zk/LICENSE-PEARL` for the full
//! permission notice.
//!
//! Port of `Pearl zk-pow chip/i8u8.rs`. The table
//! enumerates all 256 valid `(i8, u8)` pairs related by the two's-
//! complement convention `u8 = i8 + 128` (or equivalently `u8 =
//! (i8 as u8)`).
//!
//! ## Pearl's encoding
//!
//! Both `i8` and `u8` are packed into a single column ([`I8U8_TABLE`])
//! as `pack = signed * 256 + unsigned`:
//!
//! ```text
//!   signed  unsigned  pack
//!   ──────  ────────  ─────
//!   -128    128       -32640
//!   -127    129       -32383   (delta +257)
//!    ...
//!   -1      255       -1
//!    0      0         0        (delta +1  ← sign boundary)
//!    1      1         257
//!    ...
//!    127    127       32639
//! ```
//!
//! An auxiliary boolean column ([`I8U8_AUX`]) tracks the sign:
//! `AUX = 0` for `signed < 0`, `AUX = 1` for `signed ≥ 0`. The
//! transition `0 → 1` happens exactly once, at the sign boundary.
//!
//! ## Constraints
//!
//! Pearl encodes the traversal logic such that the *only* sequence
//! consistent with the constraints is the canonical 256-row table.
//! Translated to Plonky3 `AirBuilder`:
//!
//! ```text
//!   AUX[i] ∈ {0, 1}                                    (boolean)
//!   AUX[0] == 0
//!   AUX[N-1] == 1
//!   (AUX[i+1] - AUX[i]) ∈ {0, 1}                      (monotonic)
//!   AUX[i+1] - AUX[i] == 1  ⇒  pack[i] == -1           (boundary only at signed=-1)
//!   pack[0]   == -128 * 256 + 128 = -32640
//!   pack[N-1] == 127 * 256 + 127  =  32639
//!   total_delta == 257  OR  pack_delta == aux_delta    (step 257 or 1 at boundary)
//!     where total_delta = pack_delta + 256 * aux_delta
//! ```
//!
//! Together: starting at the minimum pack value, every transition
//! is +257 (advances both signed and unsigned by 1) except for one
//! transition with delta = +1 (at the sign boundary, where unsigned
//! wraps from 255 to 0).
//!
//! ## Why all 256 pairs are forced
//!
//! `pack[N-1] - pack[0] = 32639 - (-32640) = 65279`. The total
//! across 255 transitions is `254 × 257 + 1 = 65279`. So if the
//! constraints fire, exactly 254 transitions are +257 (every i8
//! advances 1 step at a time) and one transition is +1 (the sign
//! boundary). The only `(signed, unsigned)` sequence with that
//! shape and `pack[0] = -32640` is the canonical one.

use p3_air::{AirBuilder, WindowAccess};
use p3_field::PrimeCharacteristicRing;

use crate::composite_layout::{I8U8_AUX, I8U8_TABLE};

/// Inclusive size of the I8U8 table.
pub const I8U8_TABLE_SIZE: usize = 256;

/// Boundary `pack` value: `signed = -1, unsigned = 255` → `-1`.
const PACK_BOUNDARY: i32 = -1;
/// `pack` at row 0: `signed = -128, unsigned = 128`.
const PACK_FIRST: i32 = -128 * 256 + 128;
/// `pack` at row 255: `signed = 127, unsigned = 127`.
const PACK_LAST: i32 = 127 * 256 + 127;
/// Standard transition step (advances both signed and unsigned by 1).
const PACK_STEP: i32 = 257;

#[derive(Debug, Default, Clone, Copy)]
pub struct I8U8Chip;

impl I8U8Chip {
    pub const fn new() -> Self {
        Self
    }

    pub fn eval<AB: AirBuilder>(&self, builder: &mut AB) {
        let main = builder.main();
        let cur = main.current_slice();
        let nxt = main.next_slice();

        let pack: AB::Var = cur[I8U8_TABLE];
        let aux: AB::Var = cur[I8U8_AUX];
        let pack_next: AB::Var = nxt[I8U8_TABLE];
        let aux_next: AB::Var = nxt[I8U8_AUX];

        let zero: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
        let one: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ONE;

        // 1. AUX[i] is boolean.
        builder.assert_bool(aux);

        // 2. AUX at first / last row.
        builder.when_first_row().assert_zero(aux);
        builder.when_last_row().assert_eq(aux, one.clone());

        // 3. AUX transition is boolean (delta ∈ {0, 1}). Encoded as
        //    delta * (delta - 1) = 0 → delta * delta - delta = 0.
        let aux_delta: AB::Expr = AB::Expr::from(aux_next) - AB::Expr::from(aux);
        builder
            .when_transition()
            .assert_zero(aux_delta.clone() * (aux_delta.clone() - one.clone()));

        // 4. AUX delta == 1 ⇒ pack[cur] == -1 (sign boundary). Encoded
        //    as aux_delta * (pack + 1) = 0 (since pack must be -1, i.e.
        //    pack + 1 = 0, when aux_delta = 1).
        let boundary_const = <AB::Expr as PrimeCharacteristicRing>::from_i32(PACK_BOUNDARY);
        builder
            .when_transition()
            .assert_zero(aux_delta.clone() * (AB::Expr::from(pack) - boundary_const));

        // 5. pack at first / last row.
        let pack_first = <AB::F as PrimeCharacteristicRing>::from_i32(PACK_FIRST);
        let pack_last = <AB::F as PrimeCharacteristicRing>::from_i32(PACK_LAST);
        builder.when_first_row().assert_eq(pack, pack_first);
        builder.when_last_row().assert_eq(pack, pack_last);

        // 6. Per-transition step rule:
        //      total_delta = pack_delta + 256 * aux_delta
        //      EITHER total_delta == 257 (standard step)
        //      OR     pack_delta == aux_delta (boundary: both deltas are 1)
        //    Encoded as the product = 0.
        let pack_delta: AB::Expr = AB::Expr::from(pack_next) - AB::Expr::from(pack);
        let two_fifty_six = <AB::Expr as PrimeCharacteristicRing>::from_i32(256);
        let two_fifty_seven = <AB::Expr as PrimeCharacteristicRing>::from_i32(PACK_STEP);
        let total_delta = pack_delta.clone() + two_fifty_six * aux_delta.clone();
        let delta_delta = pack_delta.clone() - aux_delta;
        builder
            .when_transition()
            .assert_zero(delta_delta * (total_delta - two_fifty_seven));

        let _ = zero; // silence unused if no other zero ref
    }

    /// Fill the I8U8_TABLE / I8U8_AUX cells of `row` for table row
    /// `row_idx ∈ 0..256`. Past row 255 the values stay at the last
    /// entry (Pearl's padding convention).
    pub fn fill_row(&self, row_idx: usize, row: &mut [crate::Val]) {
        use p3_field::integers::QuotientMap;
        let i = row_idx.min(I8U8_TABLE_SIZE - 1);
        let signed = (i as i32) - 128;
        let unsigned = signed.rem_euclid(256);
        let pack = signed * 256 + unsigned;
        let aux: i32 = if signed >= 0 { 1 } else { 0 };
        row[I8U8_TABLE] = <crate::Val as QuotientMap<i32>>::from_int(pack);
        row[I8U8_AUX] = <crate::Val as QuotientMap<i32>>::from_int(aux);
    }
}

#[cfg(test)]
mod tests {
    use p3_air::{Air, AirBuilder, BaseAir};
    use p3_field::integers::QuotientMap;
    use p3_field::PrimeField64;
    use p3_matrix::dense::RowMajorMatrix;
    use p3_uni_stark::{prove, verify};

    use super::*;
    use crate::circuit::{build_stark_config, AiPowStarkConfig, CircuitConfig};
    use crate::composite_layout::{I8U8_AUX, I8U8_TABLE, TOTAL_TRACE_WIDTH};
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
    struct I8U8OnlyAir;

    impl<F> BaseAir<F> for I8U8OnlyAir {
        fn width(&self) -> usize {
            TOTAL_TRACE_WIDTH
        }
    }

    impl<AB: AirBuilder> Air<AB> for I8U8OnlyAir {
        fn eval(&self, builder: &mut AB) {
            I8U8Chip::new().eval(builder);
        }
    }

    fn build_valid_table() -> RowMajorMatrix<Val> {
        let chip = I8U8Chip::new();
        let rows = I8U8_TABLE_SIZE; // 256, power of two
        let mut flat = vec![Val::default(); rows * TOTAL_TRACE_WIDTH];
        for r in 0..rows {
            let row = &mut flat[r * TOTAL_TRACE_WIDTH..(r + 1) * TOTAL_TRACE_WIDTH];
            chip.fill_row(r, row);
        }
        RowMajorMatrix::new(flat, TOTAL_TRACE_WIDTH)
    }

    #[test]
    fn i8u8_table_size_is_256() {
        assert_eq!(I8U8_TABLE_SIZE, 256);
    }

    /// Reference: every row encodes the right (signed, unsigned) pair.
    #[test]
    fn fill_row_encodes_pearl_pack() {
        let chip = I8U8Chip::new();
        let mut row = vec![Val::default(); TOTAL_TRACE_WIDTH];

        // Row 0: signed=-128, unsigned=128, pack = -32640, aux=0.
        chip.fill_row(0, &mut row);
        let pack: Val = <Val as QuotientMap<i32>>::from_int(-32640);
        assert_eq!(row[I8U8_TABLE], pack);
        assert_eq!(row[I8U8_AUX].as_canonical_u64(), 0);

        // Row 128: signed=0, unsigned=0, pack=0, aux=1.
        chip.fill_row(128, &mut row);
        assert_eq!(row[I8U8_TABLE].as_canonical_u64(), 0);
        assert_eq!(row[I8U8_AUX].as_canonical_u64(), 1);

        // Row 127 (boundary): signed=-1, unsigned=255, pack=-1, aux=0.
        chip.fill_row(127, &mut row);
        let m1: Val = <Val as QuotientMap<i32>>::from_int(-1);
        assert_eq!(row[I8U8_TABLE], m1);
        assert_eq!(row[I8U8_AUX].as_canonical_u64(), 0);

        // Row 255: signed=127, unsigned=127, pack=32639, aux=1.
        chip.fill_row(255, &mut row);
        assert_eq!(row[I8U8_TABLE].as_canonical_u64(), 32639);
        assert_eq!(row[I8U8_AUX].as_canonical_u64(), 1);
    }

    #[test]
    fn prove_and_verify_valid_i8u8_table() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let air = I8U8OnlyAir;
        let trace = build_valid_table();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &air, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &air, &proof, &[])
            .expect("valid I8U8 table must verify");
    }

    /// AUX[0] must be 0.
    #[test]
    fn rejects_aux_first_row_nonzero() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let air = I8U8OnlyAir;
        let mut trace = build_valid_table();
        trace.values[I8U8_AUX] = <Val as QuotientMap<i32>>::from_int(1);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &air, trace, &[]);
        assert!(verify::<AiPowStarkConfig, _>(&cfg, &air, &proof, &[]).is_err());
    }

    /// AUX[N-1] must be 1.
    #[test]
    fn rejects_aux_last_row_zero() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let air = I8U8OnlyAir;
        let mut trace = build_valid_table();
        let last = (I8U8_TABLE_SIZE - 1) * TOTAL_TRACE_WIDTH + I8U8_AUX;
        trace.values[last] = <Val as QuotientMap<i32>>::from_int(0);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &air, trace, &[]);
        assert!(verify::<AiPowStarkConfig, _>(&cfg, &air, &proof, &[]).is_err());
    }

    /// Pack at row 0 must equal PACK_FIRST = -32640.
    #[test]
    fn rejects_wrong_first_pack() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let air = I8U8OnlyAir;
        let mut trace = build_valid_table();
        trace.values[I8U8_TABLE] = <Val as QuotientMap<i32>>::from_int(0);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &air, trace, &[]);
        assert!(verify::<AiPowStarkConfig, _>(&cfg, &air, &proof, &[]).is_err());
    }

    /// Pack at row N-1 must equal PACK_LAST = 32639.
    #[test]
    fn rejects_wrong_last_pack() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let air = I8U8OnlyAir;
        let mut trace = build_valid_table();
        let last = (I8U8_TABLE_SIZE - 1) * TOTAL_TRACE_WIDTH + I8U8_TABLE;
        trace.values[last] = <Val as QuotientMap<i32>>::from_int(0);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &air, trace, &[]);
        assert!(verify::<AiPowStarkConfig, _>(&cfg, &air, &proof, &[]).is_err());
    }

    /// Non-boolean AUX rejects.
    #[test]
    fn rejects_non_boolean_aux() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let air = I8U8OnlyAir;
        let mut trace = build_valid_table();
        // Put 2 in AUX at row 50.
        let r = 50 * TOTAL_TRACE_WIDTH + I8U8_AUX;
        trace.values[r] = <Val as QuotientMap<i32>>::from_int(2);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &air, trace, &[]);
        assert!(verify::<AiPowStarkConfig, _>(&cfg, &air, &proof, &[]).is_err());
    }

    /// AUX delta of 1 (sign boundary transition) must only happen
    /// at the row where pack == -1. Test by moving the boundary
    /// transition to the wrong row.
    #[test]
    fn rejects_aux_transition_off_boundary() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let air = I8U8OnlyAir;
        let mut trace = build_valid_table();
        // Swap AUX values at rows 50 and 51: original 0, 0 → 0, 1.
        // pack at row 50 isn't -1, so the boundary constraint fires.
        let r50_aux = 50 * TOTAL_TRACE_WIDTH + I8U8_AUX;
        let r51_aux = 51 * TOTAL_TRACE_WIDTH + I8U8_AUX;
        trace.values[r50_aux] = <Val as QuotientMap<i32>>::from_int(0);
        trace.values[r51_aux] = <Val as QuotientMap<i32>>::from_int(1);
        // (Now AUX has shape 0..0, 1 at row 51, then 0 again at 52
        // because we didn't touch later rows — that *also* violates
        // the monotonicity. Either rejection is fine; both come
        // from the same chip.)
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &air, trace, &[]);
        assert!(verify::<AiPowStarkConfig, _>(&cfg, &air, &proof, &[]).is_err());
    }

    /// AUX has shape 0…01…1 — once it flips to 1 it never returns
    /// to 0. Test by inserting a 0 after the boundary.
    #[test]
    fn rejects_aux_non_monotonic() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let air = I8U8OnlyAir;
        let mut trace = build_valid_table();
        // Row 200 normally has AUX = 1. Set it to 0 → 0 → 1 reversal
        // → AUX delta from row 199 to row 200 is -1 (not boolean).
        let r = 200 * TOTAL_TRACE_WIDTH + I8U8_AUX;
        trace.values[r] = <Val as QuotientMap<i32>>::from_int(0);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &air, trace, &[]);
        assert!(verify::<AiPowStarkConfig, _>(&cfg, &air, &proof, &[]).is_err());
    }

    /// Wrong intermediate pack value (e.g. broken step). Replace
    /// pack at row 100 with a different value → either total_delta
    /// from 99→100 is wrong (not 257) or 100→101 is wrong.
    #[test]
    fn rejects_wrong_intermediate_pack() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let air = I8U8OnlyAir;
        let mut trace = build_valid_table();
        let r = 100 * TOTAL_TRACE_WIDTH + I8U8_TABLE;
        // Was -128*256+128 + 100*257 = -32640 + 25700 = -6940. Set
        // to something off by 10.
        trace.values[r] = <Val as QuotientMap<i32>>::from_int(-6930);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &air, trace, &[]);
        assert!(verify::<AiPowStarkConfig, _>(&cfg, &air, &proof, &[]).is_err());
    }
}
