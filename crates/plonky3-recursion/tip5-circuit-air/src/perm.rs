//! `Tip5Perm` — a `p3_symmetric` permutation adapter over the
//! in-crate, KAT-anchored, bit-for-bit twin of
//! `nockchain_math::tip5::permute_5round` (C2.3 / M-S4).
//!
//! This is the *single in-workspace* native-reference Tip5
//! permutation. The recursion workspace cannot depend on `ai-pow-zk`
//! (separate excluded workspace), so the native
//! `DuplexChallenger<Goldilocks, Tip5Perm, 16, 10>` /
//! `PaddingFreeSponge<Tip5Perm,16,10,5>` /
//! `TruncatedPermutation<Tip5Perm,2,5,16>` /
//! `MerkleTreeMmcs<Goldilocks, _, …>` used in the bit-for-bit
//! validation gates instantiate over **this** type.
//!
//! It is a thin, faithful adapter: it canonical-`u64` round-trips the
//! Goldilocks state through [`crate::tip5_spec::permute`] (the
//! recursive 5-round Tip5, frozen against the committed golden KAT),
//! so `Tip5Perm.permute(state)` is — by construction —
//! the exact permutation the in-circuit Tip5 NPO witnesses.

use p3_field::{PrimeCharacteristicRing, PrimeField64};
use p3_goldilocks::Goldilocks;
use p3_symmetric::{CryptographicPermutation, Permutation};

use crate::tip5_spec::{STATE_SIZE, permute};

/// `Permutation<[Goldilocks; 16]>` over recursive 5-round Tip5
/// (`crate::tip5_spec::permute`, the in-crate bit-for-bit twin of
/// `nockchain_math::tip5::permute_5round`).
///
/// Public so the recursion crate's tests can build the native
/// `DuplexChallenger` / `PaddingFreeSponge` / `TruncatedPermutation` /
/// `MerkleTreeMmcs` reference oracles over the exact same permutation
/// the in-circuit Tip5 challenger / MMCS reconstructs.
#[derive(Clone, Copy, Debug, Default)]
pub struct Tip5Perm;

impl Permutation<[Goldilocks; STATE_SIZE]> for Tip5Perm {
    fn permute(&self, input: [Goldilocks; STATE_SIZE]) -> [Goldilocks; STATE_SIZE] {
        let mut s: [u64; STATE_SIZE] =
            core::array::from_fn(|i| PrimeField64::as_canonical_u64(&input[i]));
        permute(&mut s);
        core::array::from_fn(|i| Goldilocks::from_u64(s[i]))
    }

    fn permute_mut(&self, input: &mut [Goldilocks; STATE_SIZE]) {
        *input = Permutation::permute(self, *input);
    }
}

impl CryptographicPermutation<[Goldilocks; STATE_SIZE]> for Tip5Perm {}

// =====================================================================
//  Packed-Goldilocks variant.
//
//  Plonky3's `DuplexChallenger: GrindingChallenger` (exercised inside
//  FRI's PoW phase, even at `pow_bits = 0`) and `MerkleTreeMmcs`'s
//  commit step bound the permutation over BOTH scalar and packed lanes:
//
//      P: CryptographicPermutation<[Goldilocks; WIDTH]>
//       + CryptographicPermutation<[<Goldilocks as Field>::Packing; WIDTH]>
//
//  So a native `TwoAdicFriPcs<Goldilocks, …, MerkleTreeMmcs<Goldilocks,
//  Goldilocks, PaddingFreeSponge<Tip5Perm,…>, …>, …>::prove` (the exact
//  ai-pow-zk Layer-0 PCS) requires the packed impl. This is a verbatim
//  faithful mirror of ai-pow-zk's
//  `crates/ai-pow-zk/src/circuit.rs::packed_perm`: unpack lane-by-lane,
//  run the *same* scalar `crate::tip5_spec::permute` on each lane (each
//  SIMD lane is an independent Goldilocks element), repack. It is
//  therefore bit-for-bit identical to the scalar `Permutation` impl
//  above — the in-circuit Tip5 NPO witness is unchanged.
//
//  The concrete packed types are named directly (not via the
//  `<Goldilocks as Field>::Packing` projection) so rustc's coherence
//  checker can disambiguate across cfg variants — identical reasoning
//  to ai-pow-zk.

#[cfg(target_arch = "aarch64")]
mod packed_perm {
    use p3_field::PackedValue;
    use p3_goldilocks::PackedGoldilocksNeon;

    use super::*;

    impl Permutation<[PackedGoldilocksNeon; STATE_SIZE]> for Tip5Perm {
        fn permute_mut(&self, input: &mut [PackedGoldilocksNeon; STATE_SIZE]) {
            let lanes = <PackedGoldilocksNeon as PackedValue>::WIDTH;
            for lane in 0..lanes {
                let mut state = [0u64; STATE_SIZE];
                for i in 0..STATE_SIZE {
                    state[i] = PrimeField64::as_canonical_u64(&input[i].as_slice()[lane]);
                }
                permute(&mut state);
                for i in 0..STATE_SIZE {
                    input[i].as_slice_mut()[lane] = Goldilocks::from_u64(state[i]);
                }
            }
        }
    }

    impl CryptographicPermutation<[PackedGoldilocksNeon; STATE_SIZE]> for Tip5Perm {}
}

#[cfg(all(target_arch = "x86_64", target_feature = "avx512f"))]
mod packed_perm {
    use p3_field::PackedValue;
    use p3_goldilocks::PackedGoldilocksAVX512;

    use super::*;

    impl Permutation<[PackedGoldilocksAVX512; STATE_SIZE]> for Tip5Perm {
        fn permute_mut(&self, input: &mut [PackedGoldilocksAVX512; STATE_SIZE]) {
            let lanes = <PackedGoldilocksAVX512 as PackedValue>::WIDTH;
            for lane in 0..lanes {
                let mut state = [0u64; STATE_SIZE];
                for i in 0..STATE_SIZE {
                    state[i] = PrimeField64::as_canonical_u64(&input[i].as_slice()[lane]);
                }
                permute(&mut state);
                for i in 0..STATE_SIZE {
                    input[i].as_slice_mut()[lane] = Goldilocks::from_u64(state[i]);
                }
            }
        }
    }

    impl CryptographicPermutation<[PackedGoldilocksAVX512; STATE_SIZE]> for Tip5Perm {}
}

#[cfg(all(
    target_arch = "x86_64",
    target_feature = "avx2",
    not(target_feature = "avx512f")
))]
mod packed_perm {
    use p3_field::PackedValue;
    use p3_goldilocks::PackedGoldilocksAVX2;

    use super::*;

    impl Permutation<[PackedGoldilocksAVX2; STATE_SIZE]> for Tip5Perm {
        fn permute_mut(&self, input: &mut [PackedGoldilocksAVX2; STATE_SIZE]) {
            let lanes = <PackedGoldilocksAVX2 as PackedValue>::WIDTH;
            for lane in 0..lanes {
                let mut state = [0u64; STATE_SIZE];
                for i in 0..STATE_SIZE {
                    state[i] = PrimeField64::as_canonical_u64(&input[i].as_slice()[lane]);
                }
                permute(&mut state);
                for i in 0..STATE_SIZE {
                    input[i].as_slice_mut()[lane] = Goldilocks::from_u64(state[i]);
                }
            }
        }
    }

    impl CryptographicPermutation<[PackedGoldilocksAVX2; STATE_SIZE]> for Tip5Perm {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Tip5Perm` is exactly `tip5_spec::permute` lifted to
    /// `[Goldilocks;16]` (canonical-u64 round-trip is the identity on
    /// canonical inputs).
    #[test]
    fn perm_matches_spec_permute() {
        let mut raw: [u64; STATE_SIZE] = core::array::from_fn(|i| (i as u64) * 0x9e37_79b9 + 1);
        let lifted: [Goldilocks; STATE_SIZE] =
            core::array::from_fn(|i| Goldilocks::from_u64(raw[i]));

        let via_perm = Tip5Perm.permute(lifted);
        permute(&mut raw);
        let via_spec: [Goldilocks; STATE_SIZE] =
            core::array::from_fn(|i| Goldilocks::from_u64(raw[i]));

        assert_eq!(via_perm, via_spec);
    }

    #[test]
    fn permute_mut_agrees_with_permute() {
        let lifted: [Goldilocks; STATE_SIZE] =
            core::array::from_fn(|i| Goldilocks::from_u64(0xdead_beef ^ (i as u64)));
        let mut m = lifted;
        Tip5Perm.permute_mut(&mut m);
        assert_eq!(m, Tip5Perm.permute(lifted));
    }

    /// De-risk: the packed (SIMD) adapter must be **bit-for-bit
    /// identical** to the scalar `Permutation` on every lane, so the
    /// native MMCS / challenger transcript built over the packed type
    /// equals the one the in-circuit Tip5 NPO reconstructs.
    #[cfg(target_arch = "aarch64")]
    #[test]
    fn packed_perm_matches_scalar_lane_by_lane() {
        use p3_field::PackedValue;
        use p3_goldilocks::PackedGoldilocksNeon;

        let lanes = <PackedGoldilocksNeon as PackedValue>::WIDTH;
        let lane_state = |l: usize| -> [Goldilocks; STATE_SIZE] {
            core::array::from_fn(|i| {
                Goldilocks::from_u64(
                    ((l as u64) << 40)
                        ^ (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
                        ^ 0x1234_5678,
                )
            })
        };

        // Build the packed input from the per-lane scalar states.
        let mut packed: [PackedGoldilocksNeon; STATE_SIZE] =
            core::array::from_fn(|_| PackedGoldilocksNeon::from(Goldilocks::ZERO));
        for lane in 0..lanes {
            let st = lane_state(lane);
            for i in 0..STATE_SIZE {
                packed[i].as_slice_mut()[lane] = st[i];
            }
        }

        <Tip5Perm as Permutation<[PackedGoldilocksNeon; STATE_SIZE]>>::permute_mut(
            &Tip5Perm,
            &mut packed,
        );

        for lane in 0..lanes {
            let expected = Tip5Perm.permute(lane_state(lane));
            for i in 0..STATE_SIZE {
                assert_eq!(
                    packed[i].as_slice()[lane],
                    expected[i],
                    "packed lane {lane} limb {i} diverged from scalar Tip5Perm"
                );
            }
        }
    }
}
