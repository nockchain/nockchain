//! Canonical AI-PoW difficulty representation.
//!
//! Three distinct 256-bit quantities are easy to confuse; consensus depends on
//! never confusing them. This module is the single place any of them is
//! computed, so a producer and a consumer cannot drift apart.
//!
//! ```text
//!   T  consensus target      page.target for an %ai-pow block. Prices ONE
//!                            MAC-equivalent of matmul work. Emitted by
//!                            +compute-target-ai-asert, capped at
//!                            AI_POW_MAX_CONSENSUS_TARGET.
//!
//!   F  shape work factor     h · w · dot_product_length. MAC-equivalents the
//!                            miner spends producing ONE jackpot candidate.
//!                            Miner-chosen within the Pearl envelope, so
//!                            F ∈ [MIN_SHAPE_WORK_FACTOR, MAX_SHAPE_WORK_FACTOR].
//!
//!   Θ  effective threshold   T · F. The value the 256-bit BLAKE3 jackpot is
//!                            actually compared against. THIS is the
//!                            acceptance threshold — never T.
//! ```
//!
//! # Invariants
//!
//! **D1 — one predicate.** Every accept decision, on the verifier side and in
//! every miner, is [`attempt_wins`]. A miner that tests the jackpot against `T`
//! instead of `Θ` does `F` times more work per block than consensus asks for;
//! at the envelope maximum that is a 2^24 error. Enforced by
//! `miner_and_verifier_share_one_predicate` and by there being no other
//! implementation of the comparison.
//!
//! **D2 — unit of work.** Expected MAC-equivalents to find a block is
//! `2^256 / T`, *independent of the shape the miner picked*: the miner needs
//! `2^256 / Θ` attempts and each costs `F`, and `(2^256/Θ)·F = 2^256/T`. This
//! is what makes `+compute-work-ai` (which credits `≈ 2^256/T`) a meaningful
//! fork-choice weight. Enforced by `expected_work_is_shape_invariant`.
//!
//! **D3 — representable domain.** `Θ` is computed in 256 bits and the
//! computation is fail-closed, so a target that overflows when scaled is not
//! "easy", it is *unminable*: every block at that target is rejected, and
//! because the AI ASERT only advances when an AI block is accepted, the puzzle
//! would never recover. Consensus must therefore never emit a target above
//! [`AI_POW_MAX_CONSENSUS_TARGET`] — the largest `T` for which `T · F` fits for
//! *every* admissible shape. Enforced by `max_consensus_target_never_overflows`
//! and pinned against the Hoon `max-ai-target-atom` constant.

use crate::params::PEARL_HW_MAX;

/// Largest `dot_product_length` the Pearl envelope admits.
///
/// `dot_product_length = k − (k mod r)` so it never exceeds `k`, and
/// `envelope_check_dims` caps `k` at `2^16`.
pub const MAX_DOT_PRODUCT_LENGTH: u128 = 1 << 16;

/// Largest shape work factor `F` the Pearl envelope admits: `h·w ≤ 256`
/// ([`PEARL_HW_MAX`]) times [`MAX_DOT_PRODUCT_LENGTH`].
pub const MAX_SHAPE_WORK_FACTOR: u128 = PEARL_HW_MAX as u128 * MAX_DOT_PRODUCT_LENGTH;

/// Smallest shape work factor `F` the Pearl envelope admits: `h·w ≥ 32`
/// (`PEARL_HW_MIN`) times the smallest admissible `dot_product_length`
/// (`k ≥ 1024`, and `r | k` is not required so `dot` can be as low as
/// `k − (k mod r) = 1024` at `r = 32`).
pub const MIN_SHAPE_WORK_FACTOR: u128 = crate::params::PEARL_HW_MIN as u128 * 1024;

/// The largest consensus target `T` that consensus may emit for an `%ai-pow`
/// block: `floor((2^256 − 1) / MAX_SHAPE_WORK_FACTOR)`.
///
/// Above this, `T · F` leaves the 256-bit domain for some admissible shape and
/// [`effective_jackpot_threshold`] fail-closes, which rejects the block. The
/// Hoon `max-ai-target-atom` MUST equal this value; see the invariant note on
/// this module and `ai_pow_max_consensus_target_matches_hoon_constant`.
pub const AI_POW_MAX_CONSENSUS_TARGET: [u8; 32] = {
    // (2^256 − 1) / 2^24 == 2^232 − 1, i.e. 29 bytes of 0xff.
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 29 {
        out[i] = 0xff;
        i += 1;
    }
    out
};

/// Why a shape or target is not admissible. Kept separate from
/// `PearlCompatError` so this module has no upward dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DifficultyError {
    /// `rank == 0`, so `dot_product_length` is undefined.
    ZeroRank,
    /// `h`, `w`, or `dot_product_length` is zero, or their product overflowed.
    DegenerateShape,
    /// `T · F` does not fit in 256 bits. Fail-closed: never widen to "accept
    /// everything".
    ThresholdOverflow,
}

/// MAC-equivalents one jackpot attempt costs for this tile shape: `h · w · dot`.
///
/// `dot_product_length` is Pearl's `k − (k mod r)` (`PearlMiningConfig::
/// dot_product_length`); it is passed in rather than recomputed so the one
/// definition in `pearl_compat` stays authoritative.
pub fn shape_work_factor(h: u32, w: u32, dot_product_length: u64) -> Result<u128, DifficultyError> {
    if h == 0 || w == 0 || dot_product_length == 0 {
        return Err(DifficultyError::DegenerateShape);
    }
    u128::from(h)
        .checked_mul(u128::from(w))
        .and_then(|hw| hw.checked_mul(u128::from(dot_product_length)))
        .ok_or(DifficultyError::DegenerateShape)
}

/// Pearl's `dot_product_length`: `k − (k mod r)`.
pub fn dot_product_length(common_dim: u32, rank: u32) -> Result<u64, DifficultyError> {
    if rank == 0 {
        return Err(DifficultyError::ZeroRank);
    }
    let k = u64::from(common_dim);
    Ok(k - k % u64::from(rank))
}

/// `F` for a `(h, w, k, r)` shape, in one call.
pub fn shape_work_factor_for(
    h: u32,
    w: u32,
    common_dim: u32,
    rank: u32,
) -> Result<u128, DifficultyError> {
    shape_work_factor(h, w, dot_product_length(common_dim, rank)?)
}

/// The effective jackpot threshold `Θ = T · F`, fail-closed on overflow.
///
/// This is the ONLY place the consensus target is scaled by the tile shape.
pub fn effective_jackpot_threshold(
    consensus_target: &[u8; 32],
    shape_work_factor: u128,
) -> Result<[u8; 32], DifficultyError> {
    u256_le_mul_u128(consensus_target, shape_work_factor).ok_or(DifficultyError::ThresholdOverflow)
}

/// The canonical AI-PoW accept predicate: does this jackpot clear the target
/// for this tile shape?
///
/// Every miner and the consensus verifier MUST go through this. See invariant
/// D1 on this module.
pub fn attempt_wins(
    jackpot: &[u8; 32],
    consensus_target: &[u8; 32],
    shape_work_factor: u128,
) -> Result<bool, DifficultyError> {
    let threshold = effective_jackpot_threshold(consensus_target, shape_work_factor)?;
    Ok(crate::tile_hash::hash_le_target(jackpot, &threshold))
}

/// 256-bit little-endian × u128, fail-closed: `None` when the product exceeds
/// the 256-bit band. Callers turn that into an error — an over-wide threshold
/// must never silently become accept-everything.
pub(crate) fn u256_le_mul_u128(value: &[u8; 32], factor: u128) -> Option<[u8; 32]> {
    if factor == 0 || value.iter().all(|&b| b == 0) {
        return Some([0u8; 32]);
    }

    let mut value_limbs = [0u32; 8];
    for i in 0..8 {
        value_limbs[i] = u32::from_le_bytes([
            value[i * 4],
            value[i * 4 + 1],
            value[i * 4 + 2],
            value[i * 4 + 3],
        ]);
    }
    let mut factor_limbs = [0u32; 4];
    let factor_bytes = factor.to_le_bytes();
    for i in 0..4 {
        factor_limbs[i] = u32::from_le_bytes([
            factor_bytes[i * 4],
            factor_bytes[i * 4 + 1],
            factor_bytes[i * 4 + 2],
            factor_bytes[i * 4 + 3],
        ]);
    }

    let mut acc = [0u128; 12];
    for i in 0..8 {
        for j in 0..4 {
            acc[i + j] += u128::from(value_limbs[i]) * u128::from(factor_limbs[j]);
        }
    }

    let mut out_limbs = [0u32; 8];
    let mut carry = 0u128;
    for i in 0..12 {
        let total = acc[i] + carry;
        if i < 8 {
            out_limbs[i] = (total & 0xffff_ffff) as u32;
            carry = total >> 32;
        } else if total != 0 {
            return None;
        }
    }
    if carry != 0 {
        return None;
    }

    let mut out = [0u8; 32];
    for i in 0..8 {
        out[i * 4..i * 4 + 4].copy_from_slice(&out_limbs[i].to_le_bytes());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_u256(bytes: &[u8; 32]) -> num_bigint::BigUint {
        num_bigint::BigUint::from_bytes_le(bytes)
    }

    /// Every admissible `(h, w, k, r)` in the Pearl envelope, coarsely swept.
    fn envelope_shapes() -> Vec<(u32, u32, u32, u32)> {
        let mut out = Vec::new();
        for r in [32u32, 64, 128, 256, 512, 1024] {
            for k in [1024u32, 4096, 16384, 65536] {
                // Pearl envelope: 16r <= k <= 4r^2, k <= 2^16.
                if k < 16 * r || u64::from(k) > 4 * u64::from(r) * u64::from(r) {
                    continue;
                }
                for h in [2u32, 4, 8, 16] {
                    for w in [2u32, 4, 8, 16] {
                        let hw = u64::from(h) * u64::from(w);
                        if !(crate::params::PEARL_HW_MIN..=PEARL_HW_MAX).contains(&hw) {
                            continue;
                        }
                        out.push((h, w, k, r));
                    }
                }
            }
        }
        assert!(!out.is_empty());
        out
    }

    /// **D3.** The advertised maximum consensus target scales without
    /// overflowing for EVERY admissible shape. This is the property that keeps
    /// a loose AI target minable instead of permanently dead.
    #[test]
    fn max_consensus_target_never_overflows() {
        for (h, w, k, r) in envelope_shapes() {
            let f = shape_work_factor_for(h, w, k, r).expect("admissible shape");
            assert!(
                f <= MAX_SHAPE_WORK_FACTOR,
                "shape ({h},{w},{k},{r}) factor {f} exceeds the advertised maximum"
            );
            effective_jackpot_threshold(&AI_POW_MAX_CONSENSUS_TARGET, f).unwrap_or_else(|e| {
                panic!("max consensus target must scale for shape ({h},{w},{k},{r}): {e:?}")
            });
        }
    }

    /// The bound is TIGHT: one above it overflows at the envelope maximum, so
    /// the constant is not accidentally conservative in a way that hides drift.
    #[test]
    fn max_consensus_target_is_the_tight_bound() {
        let mut one_above = AI_POW_MAX_CONSENSUS_TARGET;
        // 2^232 − 1 + 1 == 2^232: clear the low 29 bytes, set bit 232.
        for b in one_above.iter_mut().take(29) {
            *b = 0;
        }
        one_above[29] = 0x01;
        assert_eq!(
            effective_jackpot_threshold(&one_above, MAX_SHAPE_WORK_FACTOR),
            Err(DifficultyError::ThresholdOverflow),
        );
        assert_eq!(
            to_u256(&AI_POW_MAX_CONSENSUS_TARGET),
            (num_bigint::BigUint::from(1u8) << 232u32) - 1u8,
            "AI_POW_MAX_CONSENSUS_TARGET must be exactly 2^232 - 1"
        );
    }

    /// **D2 — unit of work.** Expected MAC-equivalents per block is
    /// `2^256 / T` no matter which admissible tile shape the miner picks. If
    /// this ever fails, `+compute-work-ai` has stopped measuring a real cost
    /// and cross-puzzle fork choice is meaningless.
    #[test]
    fn expected_work_is_shape_invariant() {
        let two_256 = num_bigint::BigUint::from(1u8) << 256u32;
        // A target well inside the domain so no shape saturates.
        let mut target = [0u8; 32];
        target[24] = 0x01; // 2^192
        let expected_macs = &two_256 / to_u256(&target);

        for (h, w, k, r) in envelope_shapes() {
            let f = shape_work_factor_for(h, w, k, r).expect("admissible shape");
            let theta = effective_jackpot_threshold(&target, f).expect("in domain");
            let attempts = &two_256 / to_u256(&theta);
            let macs = attempts * f;
            // Integer division makes this exact here (target and F are powers
            // of two for the swept shapes with k a power of two).
            assert_eq!(
                macs, expected_macs,
                "shape ({h},{w},{k},{r}) must cost 2^256/T MAC-equivalents per block"
            );
        }
    }

    /// A jackpot in the band `(T, T·F]` is a REAL win under the consensus rule
    /// and would be discarded by any miner that compares against `T`. This
    /// pins the exact gap that a shipped miner once had.
    #[test]
    fn band_between_raw_target_and_effective_threshold_is_a_win() {
        let mut target = [0u8; 32];
        target[24] = 0x01; // 2^192
        let f = shape_work_factor_for(8, 8, 1024, 64).expect("canonical shape");
        assert_eq!(f, 1 << 16);
        let theta = effective_jackpot_threshold(&target, f).expect("in domain");

        // 2^192 + 1: above T, far below Θ = 2^208.
        let mut jackpot = target;
        jackpot[0] = 0x01;

        assert!(!crate::tile_hash::hash_le_target(&jackpot, &target));
        assert!(crate::tile_hash::hash_le_target(&jackpot, &theta));
        assert!(attempt_wins(&jackpot, &target, f).expect("in domain"));
    }

    #[test]
    fn dot_product_length_is_pearl_floor() {
        assert_eq!(dot_product_length(1024, 64).unwrap(), 1024);
        assert_eq!(dot_product_length(4096, 64).unwrap(), 4096);
        // k not a multiple of r floors down.
        assert_eq!(dot_product_length(1000, 64).unwrap(), 960);
        assert_eq!(dot_product_length(1024, 0), Err(DifficultyError::ZeroRank));
    }

    #[test]
    fn degenerate_shapes_are_rejected_not_silently_unit() {
        assert_eq!(
            shape_work_factor(0, 8, 1024),
            Err(DifficultyError::DegenerateShape)
        );
        assert_eq!(
            shape_work_factor(8, 0, 1024),
            Err(DifficultyError::DegenerateShape)
        );
        assert_eq!(
            shape_work_factor(8, 8, 0),
            Err(DifficultyError::DegenerateShape)
        );
    }

    #[test]
    fn zero_target_admits_nothing_and_zero_factor_is_zero() {
        let zero = [0u8; 32];
        assert_eq!(effective_jackpot_threshold(&zero, 1 << 16).unwrap(), zero);
        // Only an all-zero jackpot "clears" a zero threshold; a real BLAKE3
        // digest never is.
        assert!(attempt_wins(&zero, &zero, 1 << 16).unwrap());
        let mut one = [0u8; 32];
        one[0] = 1;
        assert!(!attempt_wins(&one, &zero, 1 << 16).unwrap());
    }
}
