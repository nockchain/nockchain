//! X_STEP reduction chip — Pearl §4.5 per-stripe accumulator XOR
//! (HIGH-2.2 §4.C.4, the route-independent core).
//!
//! ## Property enforced
//!
//! For each active row, `X_STEP` is the int32-XOR of the whole
//! `t·t` stripe accumulator:
//!
//! ```text
//!   X_STEP = ⊕_{c < ACC_LEN} ACC[c]            (int32 XOR)
//! ```
//!
//! This is exactly `ai-pow::matmul::compute_tile_trace`'s
//! per-stripe `x_steps[step]` value (`⊕ c_blk` after stripe
//! `step`). It is the clean scalar interface between the matmul
//! accumulator and the (done, tested) `FoldChip`
//! (`M[slot] = rotl13(M[slot]) ⊕ X_STEP`).
//!
//! Per `2026-05-15_HIGH2_2_DESIGN.md` §4.C.7 this reduction is
//! **route-independent** (it does not depend on whether the
//! committed-matrix binding uses Route A/B/C), so it lands and is
//! tested standalone first, exactly like the FoldChip.
//!
//! ## Why a parity-with-bounded-quotient, not a lookup
//!
//! XOR is not field-native. For each output bit `i`, the XOR of
//! the `ACC_LEN` input bits at position `i` equals their integer
//! sum mod 2. We enforce
//!
//! ```text
//!   Σ_c ACC_BITS[c][i]  ==  X_STEP_BITS[i] + 2·Q[i]
//! ```
//!
//! with `X_STEP_BITS[i]` boolean and `Q[i]` range-bounded by a
//! `QBITS`-bit decomposition. Because `Σ_c ACC_BITS[c][i] ≤
//! ACC_LEN` and `X_STEP_BITS[i] ∈ {0,1}`, the bounded `Q[i]`
//! forces `X_STEP_BITS[i] = parity = XOR`. All constraints are
//! degree ≤ 2 and live in plain `p3-uni-stark` — no LogUp, no new
//! prover stack (the Route-C-friendly choice).

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
use p3_field::PrimeCharacteristicRing;
use p3_matrix::dense::RowMajorMatrix;

use crate::Val;

/// Accumulator cell count = `t·t` for `TEST_SMALL`
/// (`params.tile = 8 ⇒ 64`). A shorter logical stripe is
/// zero-padded: XOR with 0 is identity, so an `L`-cell stripe is
/// `L` real cells + `ACC_LEN−L` zero cells with the same result.
pub const ACC_LEN: usize = 64;
/// Bits to range-bound the per-output-bit parity quotient.
/// `Σ ≤ ACC_LEN = 64 ⇒ Q ≤ 32`; 7 bits (0..127) is a safe bound.
pub const QBITS: usize = 7;

/// Chip-local column offsets.
pub mod cols {
    use super::{ACC_LEN, QBITS};

    /// 1 = active reduction row, 0 = padding (all cells 0).
    pub const IS_ACTIVE: usize = 0;
    /// `ACC_LEN` accumulator cells (u32 reinterpretation of i32).
    pub const ACC: usize = IS_ACTIVE + 1;
    /// 32 LE bits per accumulator cell.
    pub const ACC_BITS: usize = ACC + ACC_LEN;
    pub const ACC_BITS_LEN: usize = ACC_LEN * 32;
    /// The reduced XOR scalar (u32).
    pub const XSTEP: usize = ACC_BITS + ACC_BITS_LEN;
    /// 32 LE bits of `XSTEP`.
    pub const XSTEP_BITS: usize = XSTEP + 1;
    pub const XSTEP_BITS_LEN: usize = 32;
    /// `QBITS` quotient bits per output bit position (32 of them).
    pub const Q_BITS: usize = XSTEP_BITS + XSTEP_BITS_LEN;
    pub const Q_BITS_LEN: usize = 32 * QBITS;
    pub const ROW_W: usize = Q_BITS + Q_BITS_LEN;
}

/// Zero-sized chip type.
#[derive(Debug, Default, Clone, Copy)]
pub struct XStepChip;

/// Column-offset bundle so the eval body runs both standalone and
/// (later) at composite-layout offsets.
#[derive(Copy, Clone, Debug)]
pub struct XStepOffsets {
    pub is_active: usize,
    pub acc: usize,
    pub acc_bits: usize,
    pub xstep: usize,
    pub xstep_bits: usize,
    pub q_bits: usize,
}

impl XStepChip {
    pub const LOCAL_OFFSETS: XStepOffsets = XStepOffsets {
        is_active: cols::IS_ACTIVE,
        acc: cols::ACC,
        acc_bits: cols::ACC_BITS,
        xstep: cols::XSTEP,
        xstep_bits: cols::XSTEP_BITS,
        q_bits: cols::Q_BITS,
    };

    /// Emit the reduction constraints at the given offsets.
    pub fn eval_at<AB: AirBuilder<F = Val>>(builder: &mut AB, off: &XStepOffsets) {
        let two = <AB::F as PrimeCharacteristicRing>::TWO;
        let main = builder.main();
        let cur = main.current_slice();

        builder.assert_bool(cur[off.is_active]);

        // Each ACC cell == Σ_i ACC_BITS[cell][i]·2^i (bits boolean).
        for c in 0..ACC_LEN {
            let mut acc_recon: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
            let mut pow: AB::F = <AB::F as PrimeCharacteristicRing>::ONE;
            for i in 0..32 {
                let bit = cur[off.acc_bits + c * 32 + i];
                builder.assert_bool(bit);
                acc_recon = acc_recon + bit.into() * pow;
                pow = pow * two;
            }
            builder.assert_eq(cur[off.acc + c].into(), acc_recon);
        }

        // X_STEP == Σ_i X_STEP_BITS[i]·2^i (bits boolean).
        let mut x_recon: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
        let mut powx: AB::F = <AB::F as PrimeCharacteristicRing>::ONE;
        for i in 0..32 {
            let bit = cur[off.xstep_bits + i];
            builder.assert_bool(bit);
            x_recon = x_recon + bit.into() * powx;
            powx = powx * two;
        }
        builder.assert_eq(cur[off.xstep].into(), x_recon);

        // Per output bit i: Σ_c ACC_BITS[c][i] == X_STEP_BITS[i] +
        // 2·Q[i], with Q[i] = Σ_b Q_BITS[i][b]·2^b range-bounded
        // (each Q bit boolean ⇒ Q[i] ∈ [0, 2^QBITS)). Since the
        // column sum ≤ ACC_LEN and X_STEP_BITS[i] ∈ {0,1}, this
        // forces X_STEP_BITS[i] = parity = XOR.
        for i in 0..32 {
            let mut col_sum: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
            for c in 0..ACC_LEN {
                col_sum = col_sum + cur[off.acc_bits + c * 32 + i].into();
            }
            let mut q: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
            let mut powq: AB::F = <AB::F as PrimeCharacteristicRing>::ONE;
            for b in 0..QBITS {
                let qbit = cur[off.q_bits + i * QBITS + b];
                builder.assert_bool(qbit);
                q = q + qbit.into() * powq;
                powq = powq * two;
            }
            let xbit = cur[off.xstep_bits + i];
            builder.assert_eq(col_sum, xbit.into() + q * two);
        }
    }
}

impl<F> BaseAir<F> for XStepChip {
    fn width(&self) -> usize {
        cols::ROW_W
    }
}

impl<AB: AirBuilder<F = Val>> Air<AB> for XStepChip {
    fn eval(&self, builder: &mut AB) {
        XStepChip::eval_at(builder, &XStepChip::LOCAL_OFFSETS);
    }
}

/// Reference: int32-XOR of an accumulator (matches
/// `ai-pow::matmul::compute_tile_trace`'s per-stripe `x`).
#[inline]
pub fn ref_xstep(acc: &[i32]) -> u32 {
    acc.iter().fold(0u32, |a, &c| a ^ c as u32)
}

/// Build a standalone trace. Each entry of `stripes` is one
/// stripe's accumulator (`≤ ACC_LEN` i32 cells; zero-padded). One
/// active row per stripe; padded to the next power of two (≥ 4).
pub fn build_trace(stripes: &[Vec<i32>]) -> RowMajorMatrix<Val> {
    use p3_field::integers::QuotientMap;

    assert!(!stripes.is_empty(), "stripes must be non-empty");
    let n = stripes.len().next_power_of_two().max(4);
    let mut flat = vec![Val::default(); n * cols::ROW_W];

    for (row_idx, stripe) in stripes.iter().enumerate() {
        assert!(stripe.len() <= ACC_LEN, "stripe exceeds ACC_LEN");
        let row = &mut flat[row_idx * cols::ROW_W..(row_idx + 1) * cols::ROW_W];
        row[cols::IS_ACTIVE] = <Val as QuotientMap<u64>>::from_int(1);

        let mut acc = [0u32; ACC_LEN];
        for (c, &v) in stripe.iter().enumerate() {
            acc[c] = v as u32;
        }
        let mut xstep = 0u32;
        for c in 0..ACC_LEN {
            row[cols::ACC + c] = <Val as QuotientMap<u64>>::from_int(acc[c] as u64);
            for i in 0..32 {
                row[cols::ACC_BITS + c * 32 + i] =
                    <Val as QuotientMap<u64>>::from_int(((acc[c] >> i) & 1) as u64);
            }
            xstep ^= acc[c];
        }
        row[cols::XSTEP] = <Val as QuotientMap<u64>>::from_int(xstep as u64);
        for i in 0..32 {
            row[cols::XSTEP_BITS + i] =
                <Val as QuotientMap<u64>>::from_int(((xstep >> i) & 1) as u64);
            // Q[i] = (Σ_c ACC_BITS[c][i] − X_STEP_BITS[i]) / 2.
            let col_sum: u32 = (0..ACC_LEN).map(|c| (acc[c] >> i) & 1).sum();
            let q = (col_sum - ((xstep >> i) & 1)) / 2;
            for b in 0..QBITS {
                row[cols::Q_BITS + i * QBITS + b] =
                    <Val as QuotientMap<u64>>::from_int(((q >> b) & 1) as u64);
            }
        }
    }
    RowMajorMatrix::new(flat, cols::ROW_W)
}

/// Read the proven `X_STEP` of each active row, in order.
pub fn xsteps(trace: &RowMajorMatrix<Val>) -> Vec<u32> {
    use p3_field::PrimeField64;
    let h = trace.values.len() / cols::ROW_W;
    (0..h)
        .filter(|&r| trace.values[r * cols::ROW_W + cols::IS_ACTIVE].as_canonical_u64() == 1)
        .map(|r| trace.values[r * cols::ROW_W + cols::XSTEP].as_canonical_u64() as u32)
        .collect()
}

#[cfg(test)]
mod tests {
    //! Self-contained (ai-pow-zk must not depend on ai-pow). The
    //! cross-crate parity vs `compute_tile_trace`'s real
    //! per-stripe `x_steps` is asserted from the ai-pow side
    //! (zk_bridge, `zk` feature).

    use p3_field::integers::QuotientMap;
    use p3_field::PrimeField64;
    use p3_uni_stark::{prove, verify};

    use super::*;
    use crate::circuit::{build_stark_config, AiPowStarkConfig, CircuitConfig};
    use crate::params::ZkParams;

    fn cfg() -> AiPowStarkConfig {
        build_stark_config(
            &ZkParams {
                m: 8,
                k: 16,
                n: 8,
                noise_rank: 2,
                tile: 2,
                difficulty_bits: 0,
            },
            &CircuitConfig::TEST_PEARL,
        )
    }

    fn lcg(seed: u64, n: usize) -> Vec<i32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (s >> 32) as i32
            })
            .collect()
    }

    /// Proven `X_STEP` equals the reference XOR for a spread of
    /// stripe shapes: full ACC_LEN, short (zero-padded), single
    /// cell, all-zero, and the all-bits-set worst case (column
    /// sum hits ACC_LEN, exercising the Q bound).
    #[test]
    fn xstep_matches_reference_and_verifies() {
        let c = cfg();
        let stripes: Vec<Vec<i32>> = vec![
            lcg(1, ACC_LEN),
            lcg(2, ACC_LEN),
            lcg(3, 7),            // short ⇒ zero-padded
            vec![0x7FFF_FFFF],    // single cell
            vec![0; ACC_LEN],     // all zero ⇒ X_STEP 0
            vec![-1i32; ACC_LEN], // every bit set in every cell
        ];
        for s in &stripes {
            assert_eq!(
                xsteps(&build_trace(std::slice::from_ref(s)))[0],
                ref_xstep(s),
                "build_trace X_STEP must equal ref_xstep"
            );
        }
        let trace = build_trace(&stripes);
        let proof = prove::<AiPowStarkConfig, _>(&c, &XStepChip, trace, &[]);
        verify::<AiPowStarkConfig, _>(&c, &XStepChip, &proof, &[])
            .expect("honest X_STEP reduction must verify");
    }

    /// Independent re-derivation (XOR via per-bit parity) to guard
    /// against a shared bug between `build_trace` and the chip.
    #[test]
    fn xstep_matches_manual_bit_parity() {
        let s = lcg(42, ACC_LEN);
        let mut bits = [0u32; 32];
        for &v in &s {
            for i in 0..32 {
                bits[i] ^= (v as u32 >> i) & 1;
            }
        }
        let want: u32 = (0..32).map(|i| bits[i] << i).sum();
        assert_eq!(xsteps(&build_trace(&[s]))[0], want);
    }

    fn honest() -> RowMajorMatrix<Val> {
        build_trace(&[lcg(7, ACC_LEN), lcg(8, ACC_LEN), lcg(9, 30)])
    }

    #[test]
    fn rejects_tampered_xstep() {
        let c = cfg();
        let mut t = honest();
        t.values[cols::XSTEP] = <Val as QuotientMap<u64>>::from_int(0x1234);
        let p = prove::<AiPowStarkConfig, _>(&c, &XStepChip, t, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&c, &XStepChip, &p, &[]).is_err(),
            "tampered X_STEP must reject"
        );
    }

    #[test]
    fn rejects_tampered_acc_cell_without_bits() {
        let c = cfg();
        let mut t = honest();
        // Change ACC[5] but not its bits ⇒ 32-bit recon fails.
        t.values[cols::ACC + 5] = <Val as QuotientMap<u64>>::from_int(0xDEAD);
        let p = prove::<AiPowStarkConfig, _>(&c, &XStepChip, t, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&c, &XStepChip, &p, &[]).is_err(),
            "ACC/ACC_BITS mismatch must reject"
        );
    }

    #[test]
    fn rejects_flipped_xstep_bit_with_unbounded_q_attempt() {
        let c = cfg();
        let mut t = honest();
        // Flip X_STEP_BITS[0]; a malicious prover would need a
        // fractional/over-range Q to satisfy the parity eq — the
        // QBITS bound forbids it. (Leave XSTEP value as-is too ⇒
        // also breaks recon; either way must reject.)
        let cur = t.values[cols::XSTEP_BITS].as_canonical_u64();
        t.values[cols::XSTEP_BITS] = <Val as QuotientMap<u64>>::from_int(1 - cur);
        let p = prove::<AiPowStarkConfig, _>(&c, &XStepChip, t, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&c, &XStepChip, &p, &[]).is_err(),
            "flipped parity bit must reject (bounded Q)"
        );
    }

    #[test]
    fn rejects_out_of_range_q_bit() {
        let c = cfg();
        let mut t = honest();
        // Set a Q bit to 2 (non-boolean) ⇒ booleanity rejects.
        t.values[cols::Q_BITS] = <Val as QuotientMap<u64>>::from_int(2);
        let p = prove::<AiPowStarkConfig, _>(&c, &XStepChip, t, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&c, &XStepChip, &p, &[]).is_err(),
            "non-boolean Q bit must reject"
        );
    }
}
