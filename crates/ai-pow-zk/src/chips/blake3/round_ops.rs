//! Constraint primitives for the BLAKE3 round AIR.
//!
//! **Pearl ISC.** This file is derived from Pearl source code
//! (Copyright (c) 2025-2026 Pearl Research Labs; 2015-2016 The Decred
//! developers); see `crates/ai-pow-zk/LICENSE-PEARL` for the full
//! permission notice.
//!
//! Port of `Pearl zk-pow chip/blake3/blake3_air.rs`,
//! specifically the underlying constraint helpers
//! (`add3_unchecked`, `add2_unchecked`, `xor_32_shift_if`,
//! `xor_32`). These are the building blocks Pearl's `half_g`
//! function composes; the full G-function port lands in a
//! follow-up commit.
//!
//! All constraints here are designed to stay within Pearl's
//! degree-3 budget (`pearl_stark.rs:208-210` pins
//! `constraint_degree = 3`). At our `CircuitConfig::TEST_PEARL`
//! profile (`log_blowup = 2`) the quotient-degree budget is 4 −
//! exactly the headroom Pearl plans for.
//!
//! ## Why "unchecked" sums?
//!
//! `add3_unchecked(res, a, b, c)` asserts `res ∈ {a+b+c,
//! a+b+c−2^32, a+b+c−2^33}` rather than `res = (a+b+c) mod 2^32`.
//! This stays degree 3 (a single cubic polynomial in `diff`).
//! The trace generator + range checks elsewhere bound `res` to a
//! valid u32; the constraint itself only verifies the *additive
//! relation* up to a 2^32 wrap.
//!
//! ## XOR-shift via bits
//!
//! `xor_32_shift_if(res, a, b, is_activated, shift)` asserts that
//! `res = a XOR (b <<< shift)` when `is_activated = 1`, given
//! `b` as a 32-bit decomposition. This implicitly range-checks
//! `b` (each bit constrained boolean) and produces `res` as the
//! packed u32 obtained by re-assembling the XORed shifted bits.

use p3_air::AirBuilder;
use p3_field::PrimeCharacteristicRing;

/// Constrain `res ∈ {a+b+c, a+b+c − 2^32, a+b+c − 2^33}` when
/// `is_activated = 1`; vacuous when `is_activated = 0`. Mirrors
/// Pearl's `add3_unchecked` (`blake3_air.rs:286-302`).
///
/// Ungated, the cubic `diff · (diff − 2^32) · (diff − 2^33) = 0`
/// is degree 3. Gated, it's `is_activated · diff · (diff − 2^32)
/// · (diff − 2^33) = 0` — degree 4. Pass `AB::Expr::ONE` to recover
/// the original degree-3 unconditional form when the caller has
/// no row-level gating to apply.
pub fn add3_unchecked<AB: AirBuilder>(
    builder: &mut AB,
    res: AB::Expr,
    a: AB::Expr,
    b: AB::Expr,
    c: AB::Expr,
    is_activated: AB::Expr,
) {
    let sum = a + b + c;
    let two_pow_32 = <AB::F as PrimeCharacteristicRing>::from_u64(1u64 << 32);
    let two_pow_33 = <AB::F as PrimeCharacteristicRing>::from_u64(1u64 << 33);
    let diff: AB::Expr = sum - res;
    let diff_1: AB::Expr = diff.clone() - two_pow_32;
    let diff_2: AB::Expr = diff.clone() - two_pow_33;
    builder.assert_zero(is_activated * diff * diff_1 * diff_2);
}

/// Constrain `res ∈ {a+b, a+b − 2^32}` when `is_activated = 1`;
/// vacuous when `is_activated = 0`. Mirrors Pearl's
/// `add2_unchecked` (`blake3_air.rs:304-317`).
///
/// Gated form is degree 3 (1 + 2). Pass `AB::Expr::ONE` to recover
/// the degree-2 unconditional form.
pub fn add2_unchecked<AB: AirBuilder>(
    builder: &mut AB,
    res: AB::Expr,
    a: AB::Expr,
    b: AB::Expr,
    is_activated: AB::Expr,
) {
    let sum = a + b;
    let two_pow_32 = <AB::F as PrimeCharacteristicRing>::from_u64(1u64 << 32);
    let diff: AB::Expr = sum - res;
    let diff_1: AB::Expr = diff.clone() - two_pow_32;
    builder.assert_zero(is_activated * diff * diff_1);
}

/// XOR-and-rotate constraint: `res = a XOR (b <<< shift)` when
/// `is_activated = 1`, given `b` as 32 boolean bits.
///
/// Mirrors Pearl's `xor_32_shift_if` (`blake3_air.rs:319-342`).
/// Asserts:
///   * each `b[i]` is boolean (unconditional, so b is range-checked
///     for u32 regardless of `is_activated`);
///   * `res = polyval(xor_bits, 2)` where
///     `xor_bits[i] = a[i] XOR b[(i + 32 − shift) mod 32]`.
///
/// XOR is computed via the boolean identity `x XOR y = x + y −
/// 2xy`.
pub fn xor_32_shift_if<AB: AirBuilder>(
    builder: &mut AB,
    res: AB::Expr,
    a: &[AB::Expr],
    b: &[AB::Var],
    is_activated: AB::Expr,
    shift: usize,
) {
    assert!(shift < 32, "xor_32_shift_if: shift must be < 32");
    assert_eq!(a.len(), 32);
    assert_eq!(b.len(), 32);

    // Boolean-check every b-bit unconditionally — implicit u32 range
    // check for `b`.
    for &bit in b.iter() {
        builder.assert_bool(bit);
    }

    // Pack the XOR'd, shifted bits back into a single u32 value.
    // `b[(i + 32 - shift) mod 32]` is the bit that lands at output
    // position i after a left rotation by `shift`.
    let two = <AB::F as PrimeCharacteristicRing>::TWO;
    let mut acc: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
    let mut pow: AB::F = <AB::F as PrimeCharacteristicRing>::ONE;
    for i in 0..32 {
        let a_bit: AB::Expr = a[i].clone();
        let b_src = b[(i + 32 - shift) % 32];
        // x XOR y = x + y - 2xy, valid when x, y are boolean.
        let two_ab = AB::Expr::from(b_src) * a_bit.clone() * two.clone();
        let xor_bit: AB::Expr = a_bit + AB::Expr::from(b_src) - two_ab;
        acc = acc + xor_bit * pow.clone();
        pow = pow * two.clone();
    }
    // `is_activated * (res - acc) = 0` ⇒ when activated, res equals
    // the recomposed XOR.
    let diff: AB::Expr = res - acc;
    builder.assert_zero(is_activated * diff);
}

/// Direct 32-bit XOR (no shift, no gating): returns
/// `polyval(a XOR b, 2)`. Mirrors Pearl's `xor_32`
/// (`blake3_air.rs:344-356`).
///
/// Both `a` and `b` must already be 32-bit decompositions (the
/// caller is responsible for the boolean-checks). Useful in the
/// finalization-row XOR where both operands are already
/// constrained bit columns.
pub fn xor_32_packed<AB: AirBuilder>(builder: &mut AB, a: &[AB::Var], b: &[AB::Var]) -> AB::Expr {
    assert_eq!(a.len(), 32);
    assert_eq!(b.len(), 32);
    let _ = builder; // current implementation needs no constraints
    let two = <AB::F as PrimeCharacteristicRing>::TWO;
    let mut acc: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
    let mut pow: AB::F = <AB::F as PrimeCharacteristicRing>::ONE;
    for i in 0..32 {
        let two_ab: AB::Expr = AB::Expr::from(a[i]) * b[i] * two.clone();
        let xor_bit: AB::Expr = AB::Expr::from(a[i]) + AB::Expr::from(b[i]) - two_ab;
        acc = acc + xor_bit * pow.clone();
        pow = pow * two.clone();
    }
    acc
}

#[cfg(test)]
mod tests {
    //! These tests exercise each constraint primitive in isolation
    //! by wrapping it in a thin AIR over a small custom trace.
    //!
    //! Layout in the test trace (per row, 64 cols):
    //! ```text
    //!   col 0..32  : 32 boolean bits of operand B
    //!   col 32     : packed u32 operand A
    //!   col 33     : packed u32 result RES
    //!   col 34..66 : 32 boolean bits of operand C (xor_32_packed only)
    //!   col 66..68 : reserved
    //! ```
    //! Tests use plenty of headroom in the column layout to avoid
    //! collisions between primitives.
    use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
    use p3_field::integers::QuotientMap;
    use p3_matrix::dense::RowMajorMatrix;
    use p3_uni_stark::{prove, verify};

    use super::*;
    use crate::circuit::{build_stark_config, AiPowStarkConfig, CircuitConfig};
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

    const W: usize = 96;

    // --------------------------------------------------------------
    //  add3_unchecked test wrapper.
    //
    //  Trace cols [0..3]: a, b, c (input operands as u32 values).
    //  Trace col   [3]:   res (the prover's claim of a+b+c mod something).
    //
    //  Constraint: add3_unchecked(res, a, b, c) — must hold per row.
    // --------------------------------------------------------------
    #[derive(Debug, Default)]
    struct Add3Air;
    impl<F> BaseAir<F> for Add3Air {
        fn width(&self) -> usize {
            W
        }
    }
    impl<AB: AirBuilder> Air<AB> for Add3Air {
        fn eval(&self, builder: &mut AB) {
            let main = builder.main();
            let cur = main.current_slice();
            let a = AB::Expr::from(cur[0]);
            let b = AB::Expr::from(cur[1]);
            let c = AB::Expr::from(cur[2]);
            let res = AB::Expr::from(cur[3]);
            add3_unchecked(
                builder,
                res,
                a,
                b,
                c,
                <AB::Expr as PrimeCharacteristicRing>::ONE,
            );
        }
    }

    fn make_add3_trace(rows: &[(u32, u32, u32, u64)]) -> RowMajorMatrix<Val> {
        let n = rows.len().next_power_of_two().max(4);
        let mut flat = vec![Val::default(); n * W];
        for (i, &(a, b, c, res)) in rows.iter().enumerate() {
            flat[i * W] = <Val as QuotientMap<u32>>::from_int(a);
            flat[i * W + 1] = <Val as QuotientMap<u32>>::from_int(b);
            flat[i * W + 2] = <Val as QuotientMap<u32>>::from_int(c);
            flat[i * W + 3] = <Val as QuotientMap<u64>>::from_int(res);
        }
        // Fill padding rows with (0, 0, 0, 0) — all zeros satisfy the
        // constraint trivially (diff = 0, factor zero).
        RowMajorMatrix::new(flat, W)
    }

    #[test]
    fn add3_unchecked_accepts_no_wrap() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        // a + b + c = 3 + 5 + 7 = 15, no wrap.
        let trace = make_add3_trace(&[(3, 5, 7, 15), (0, 0, 0, 0), (1, 2, 3, 6), (10, 20, 30, 60)]);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &Add3Air, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &Add3Air, &proof, &[]).expect("clean add must verify");
    }

    #[test]
    fn add3_unchecked_accepts_wrap_2_pow_32() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        // a + b + c = (2^31) + (2^31) + 5 = 2^32 + 5 → wrap to 5.
        // diff = (sum - res) = 2^32, so diff - 2^32 = 0 satisfies.
        let sum = (1u64 << 32) + 5;
        let trace = make_add3_trace(&[
            (1u32 << 31, 1u32 << 31, 5, sum - (1u64 << 32)),
            (0, 0, 0, 0),
            (0, 0, 0, 0),
            (0, 0, 0, 0),
        ]);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &Add3Air, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &Add3Air, &proof, &[])
            .expect("2^32-wrapped add must verify");
    }

    #[test]
    fn add3_unchecked_accepts_wrap_2_pow_33() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        // 3 × (2^32 - 1) ≈ 2^33 - 3 = sum, res = sum - 2^33 = -3 mod something.
        // Easier: pick a, b, c such that a + b + c = 2^33 + 0 → res = 0,
        // diff = 2^33, diff - 2^33 = 0.
        let trace = make_add3_trace(&[
            // a = 2^32 - 1, b = 2^32 - 1, c = 2, sum = 2^33 → res = 0.
            (u32::MAX, u32::MAX, 2, 0),
            (0, 0, 0, 0),
            (0, 0, 0, 0),
            (0, 0, 0, 0),
        ]);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &Add3Air, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &Add3Air, &proof, &[])
            .expect("2^33-wrapped add must verify");
    }

    #[test]
    fn add3_unchecked_rejects_off_by_one() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        // Claim res = 16 when the real sum is 15.
        let trace = make_add3_trace(&[(3, 5, 7, 16), (0, 0, 0, 0), (0, 0, 0, 0), (0, 0, 0, 0)]);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &Add3Air, trace, &[]);
        let r = verify::<AiPowStarkConfig, _>(&cfg, &Add3Air, &proof, &[]);
        assert!(r.is_err(), "off-by-one add must reject; got {r:?}");
    }

    #[test]
    fn add3_unchecked_rejects_unrelated_value() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let trace = make_add3_trace(&[(3, 5, 7, 100), (0, 0, 0, 0), (0, 0, 0, 0), (0, 0, 0, 0)]);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &Add3Air, trace, &[]);
        assert!(verify::<AiPowStarkConfig, _>(&cfg, &Add3Air, &proof, &[]).is_err());
    }

    // --------------------------------------------------------------
    //  add2_unchecked test wrapper.
    // --------------------------------------------------------------
    #[derive(Debug, Default)]
    struct Add2Air;
    impl<F> BaseAir<F> for Add2Air {
        fn width(&self) -> usize {
            W
        }
    }
    impl<AB: AirBuilder> Air<AB> for Add2Air {
        fn eval(&self, builder: &mut AB) {
            let main = builder.main();
            let cur = main.current_slice();
            let a = AB::Expr::from(cur[0]);
            let b = AB::Expr::from(cur[1]);
            let res = AB::Expr::from(cur[2]);
            add2_unchecked(
                builder,
                res,
                a,
                b,
                <AB::Expr as PrimeCharacteristicRing>::ONE,
            );
        }
    }

    fn make_add2_trace(rows: &[(u32, u32, u64)]) -> RowMajorMatrix<Val> {
        let n = rows.len().next_power_of_two().max(4);
        let mut flat = vec![Val::default(); n * W];
        for (i, &(a, b, res)) in rows.iter().enumerate() {
            flat[i * W] = <Val as QuotientMap<u32>>::from_int(a);
            flat[i * W + 1] = <Val as QuotientMap<u32>>::from_int(b);
            flat[i * W + 2] = <Val as QuotientMap<u64>>::from_int(res);
        }
        RowMajorMatrix::new(flat, W)
    }

    #[test]
    fn add2_unchecked_accepts_no_wrap() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let trace = make_add2_trace(&[(3, 5, 8), (0, 0, 0), (10, 20, 30), (42, 58, 100)]);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &Add2Air, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &Add2Air, &proof, &[]).expect("clean add must verify");
    }

    #[test]
    fn add2_unchecked_accepts_wrap_2_pow_32() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        // a + b = 2^32 + 5 → res = 5, diff = 2^32.
        let trace =
            make_add2_trace(&[(1u32 << 31, (1u32 << 31) + 5, 5), (0, 0, 0), (0, 0, 0), (0, 0, 0)]);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &Add2Air, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &Add2Air, &proof, &[])
            .expect("2^32-wrapped add must verify");
    }

    #[test]
    fn add2_unchecked_rejects_wrong_sum() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let trace = make_add2_trace(&[(3, 5, 9), (0, 0, 0), (0, 0, 0), (0, 0, 0)]);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &Add2Air, trace, &[]);
        assert!(verify::<AiPowStarkConfig, _>(&cfg, &Add2Air, &proof, &[]).is_err());
    }

    // --------------------------------------------------------------
    //  xor_32_shift_if test wrapper.
    //
    //  Trace cols [0..32]: 32 bool bits of operand B.
    //  Trace col   [32]:   packed u32 operand A.
    //  Trace col   [33]:   packed u32 result RES.
    //  is_activated = 1 (constant) — gating not exercised here.
    // --------------------------------------------------------------
    #[derive(Debug)]
    struct XorShiftAir<const SHIFT: usize>;
    impl<F, const S: usize> BaseAir<F> for XorShiftAir<S> {
        fn width(&self) -> usize {
            W
        }
    }
    impl<AB: AirBuilder, const S: usize> Air<AB> for XorShiftAir<S> {
        fn eval(&self, builder: &mut AB) {
            let main = builder.main();
            let cur = main.current_slice();
            let b_bits: Vec<AB::Var> = (0..32).map(|i| cur[i]).collect();
            // a as 32-bit decomposition via a single packed cell:
            // we need a as 32 individual Expr values. Trace col 32
            // is the packed `a` value; for this test we re-bit-
            // decompose `a` into trace cols 34..66.
            let a_bits: Vec<AB::Expr> = (0..32).map(|i| AB::Expr::from(cur[34 + i])).collect();
            // Bool-check a's bit columns.
            for i in 0..32 {
                builder.assert_bool(cur[34 + i]);
            }
            // Also constrain that polyval(a_bits, 2) = cur[32] so the
            // test's `a` column matches its bit decomposition.
            let two = <AB::F as PrimeCharacteristicRing>::TWO;
            let mut acc: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
            let mut pow: AB::F = <AB::F as PrimeCharacteristicRing>::ONE;
            for i in 0..32 {
                acc = acc + cur[34 + i] * pow.clone();
                pow = pow * two.clone();
            }
            builder.assert_eq(AB::Expr::from(cur[32]), acc);

            let res = AB::Expr::from(cur[33]);
            let is_activated = <AB::Expr as PrimeCharacteristicRing>::ONE;
            xor_32_shift_if(builder, res, &a_bits, &b_bits, is_activated, S);
        }
    }

    fn make_xor_shift_trace(rows: &[(u32, u32, u32)]) -> RowMajorMatrix<Val> {
        let n = rows.len().next_power_of_two().max(4);
        let mut flat = vec![Val::default(); n * W];
        for (i, &(a, b, res)) in rows.iter().enumerate() {
            // B bits at cols 0..32.
            for bit in 0..32 {
                flat[i * W + bit] = <Val as QuotientMap<u32>>::from_int((b >> bit) & 1);
            }
            // A packed at col 32.
            flat[i * W + 32] = <Val as QuotientMap<u32>>::from_int(a);
            // RES at col 33.
            flat[i * W + 33] = <Val as QuotientMap<u32>>::from_int(res);
            // A bits at cols 34..66.
            for bit in 0..32 {
                flat[i * W + 34 + bit] = <Val as QuotientMap<u32>>::from_int((a >> bit) & 1);
            }
        }
        RowMajorMatrix::new(flat, W)
    }

    #[test]
    fn xor_32_shift_if_zero_shift_matches_plain_xor() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        // res = a XOR (b <<< 0) = a XOR b.
        let trace = make_xor_shift_trace(&[
            (0x12345678, 0xABCDEF01, 0x12345678 ^ 0xABCDEF01),
            (0, 0, 0),
            (u32::MAX, 0, u32::MAX),
            (u32::MAX, u32::MAX, 0),
        ]);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &XorShiftAir::<0>, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &XorShiftAir::<0>, &proof, &[])
            .expect("zero-shift XOR must verify");
    }

    #[test]
    fn xor_32_shift_if_rotate_16_matches_pearl_g_rotation() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        // The G function's first XOR uses shift=16. Pearl's rotation
        // direction is left-rotate: (b <<< 16) = b.rotate_left(16).
        let b: u32 = 0x12345678;
        let a: u32 = 0xABCDEF01;
        let expected = a ^ b.rotate_left(16);
        let trace = make_xor_shift_trace(&[
            (a, b, expected),
            (0, 0, 0),
            (
                0xDEADBEEF,
                0x00112233,
                0xDEADBEEF ^ 0x00112233u32.rotate_left(16),
            ),
            (0, 0, 0),
        ]);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &XorShiftAir::<16>, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &XorShiftAir::<16>, &proof, &[])
            .expect("rotate-16 XOR must verify");
    }

    #[test]
    fn xor_32_shift_if_rejects_wrong_result() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        // Claim res = 0 when the actual XOR result is non-zero.
        let trace = make_xor_shift_trace(&[
            (0x12345678, 0xABCDEF01, 0xDEADBEEF), // wrong
            (0, 0, 0),
            (0, 0, 0),
            (0, 0, 0),
        ]);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &XorShiftAir::<0>, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &XorShiftAir::<0>, &proof, &[]).is_err(),
            "wrong XOR result must reject"
        );
    }

    #[test]
    fn xor_32_shift_if_rejects_non_boolean_bit() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = make_xor_shift_trace(&[
            (0x12345678, 0xABCDEF01, 0x12345678 ^ 0xABCDEF01),
            (0, 0, 0),
            (0, 0, 0),
            (0, 0, 0),
        ]);
        // Put a non-boolean value in one of B's bit columns.
        trace.values[0] = <Val as QuotientMap<u32>>::from_int(2);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &XorShiftAir::<0>, trace, &[]);
        assert!(verify::<AiPowStarkConfig, _>(&cfg, &XorShiftAir::<0>, &proof, &[]).is_err());
    }

    #[test]
    fn xor_32_shift_if_rotate_8_matches_pearl_g_rotation() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        // G's third XOR uses shift=8.
        let b: u32 = 0x12345678;
        let a: u32 = 0xFEDCBA98;
        let expected = a ^ b.rotate_left(8);
        let trace = make_xor_shift_trace(&[(a, b, expected), (0, 0, 0), (0, 0, 0), (0, 0, 0)]);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &XorShiftAir::<8>, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &XorShiftAir::<8>, &proof, &[])
            .expect("rotate-8 XOR must verify");
    }

    #[test]
    fn xor_32_shift_if_rotate_12_matches_pearl_g_rotation() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        // G's second XOR uses shift=12.
        let b: u32 = 0x12345678;
        let a: u32 = 0x76543210;
        let expected = a ^ b.rotate_left(12);
        let trace = make_xor_shift_trace(&[(a, b, expected), (0, 0, 0), (0, 0, 0), (0, 0, 0)]);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &XorShiftAir::<12>, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &XorShiftAir::<12>, &proof, &[])
            .expect("rotate-12 XOR must verify");
    }

    #[test]
    fn xor_32_shift_if_rotate_7_matches_pearl_g_rotation() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        // G's fourth XOR uses shift=7.
        let b: u32 = 0x12345678;
        let a: u32 = 0xCAFEBABE;
        let expected = a ^ b.rotate_left(7);
        let trace = make_xor_shift_trace(&[(a, b, expected), (0, 0, 0), (0, 0, 0), (0, 0, 0)]);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &XorShiftAir::<7>, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &XorShiftAir::<7>, &proof, &[])
            .expect("rotate-7 XOR must verify");
    }
}
