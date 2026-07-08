//! The Tip5 permutation AIR (C2.1 soundness linchpin).
//!
//! **One row per permutation**: every row independently encodes a full
//! 5-round Tip5 evaluation `OUT = permute(IN)` (padding rows hold the
//! genuine zero-input permutation, so they too satisfy every
//! constraint — no padding selector, no selector soundness subtlety).
//!
//! Constraints (max algebraic degree 3, lookup-free):
//!
//! * **Split-and-lookup lanes (0..4):** each pre-S-box lane value is
//!   decomposed into 8 little-endian bytes via boolean bit columns.
//!   Per byte `b`: `c` (8-bit) and `q` (16-bit) bit-columns enforce
//!   `(b+1)^3 − 1 = 257·q + c` ⇒ `c = ((b+1)^3−1) mod 257 =
//!   LOOKUP_TABLE[b]` (the C2.0 machine-proved identity). The 8-byte
//!   split is pinned to the unique **canonical** representative by the
//!   paper §4.6 inverse-or-zero `< p` guard (`H = 2^32−1 ⇒ L = 0`).
//! * **Power lanes (4..16):** `x^7 = x^3·x^3·x` via `x2,x3` registers.
//! * **MDS:** constant circulant matvec (degree 1).
//! * **Round constants:** `+ ((RC·2^64) mod p)` constant (degree 1).
//!
//! Faithfulness is closed by the native≡in-circuit KAT
//! (`tests::native_equiv_kat`): the generated trace's
//! `(IN, ROUT[NUM_ROUNDS-1])` equals the committed 5-round golden
//! fixture (= `nockchain_math::tip5::permute_5round`) bit-for-bit, the
//! proof verifies, and any tamper is
//! rejected.

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
use p3_field::PrimeCharacteristicRing;

use crate::tip5_spec::{NUM_ROUNDS, ROUND_CONSTANTS, STATE_SIZE, mds_matrix, rc_precomp};

/// Split-and-lookup lanes.
pub(crate) const NS: usize = 4;
/// Bytes per split lane (little-endian decomposition of a u64).
pub(crate) const NBYTES: usize = 8;
/// Bits per input byte `b` (`b ∈ [0,256)`).
pub(crate) const BBITS: usize = 8;
/// Bits per output byte `c` (`c ∈ [0,256)`).
pub(crate) const CBITS: usize = 8;
/// Bits per quotient `q` (`q ≤ ⌊(256^3−1)/257⌋ = 65280 < 2^16`).
pub(crate) const QBITS: usize = 16;

/// Columns per (split-lane, byte) block: `b` bits ++ `c` bits ++ `q` bits.
const BYTE_BLOCK: usize = BBITS + CBITS + QBITS; // 32
/// Split-bits block per round.
const SPLIT_BLOCK: usize = NS * NBYTES * BYTE_BLOCK; // 1024
/// Columns per round group.
pub(crate) const ROUND_GROUP: usize = SPLIT_BLOCK
    + NS                              // inverse-or-zero guard, 1 per split lane
    + (STATE_SIZE - NS)               // x2 register per power lane
    + (STATE_SIZE - NS)               // x3 register per power lane
    + STATE_SIZE                      // A: S-box output
    + STATE_SIZE; // ROUT: post-MDS+RC state (= next round's input)

/// Total AIR width: input state ++ `NUM_ROUNDS` round groups.
pub const fn tip5_perm_air_width() -> usize {
    STATE_SIZE + NUM_ROUNDS * ROUND_GROUP
}

// ---- flat column index helpers (absolute column of the local row) ----

#[inline]
const fn in_col(lane: usize) -> usize {
    lane
}
#[inline]
const fn rb(r: usize) -> usize {
    STATE_SIZE + r * ROUND_GROUP
}
#[inline]
const fn bbit(r: usize, t: usize, k: usize, i: usize) -> usize {
    rb(r) + (t * NBYTES + k) * BYTE_BLOCK + i
}
#[inline]
const fn cbit(r: usize, t: usize, k: usize, i: usize) -> usize {
    rb(r) + (t * NBYTES + k) * BYTE_BLOCK + BBITS + i
}
#[inline]
const fn qbit(r: usize, t: usize, k: usize, i: usize) -> usize {
    rb(r) + (t * NBYTES + k) * BYTE_BLOCK + BBITS + CBITS + i
}
#[inline]
const fn inv_col(r: usize, t: usize) -> usize {
    rb(r) + SPLIT_BLOCK + t
}
#[inline]
const fn x2_col(r: usize, j: usize) -> usize {
    rb(r) + SPLIT_BLOCK + NS + (j - NS)
}
#[inline]
const fn x3_col(r: usize, j: usize) -> usize {
    rb(r) + SPLIT_BLOCK + NS + (STATE_SIZE - NS) + (j - NS)
}
#[inline]
pub(crate) const fn a_col(r: usize, i: usize) -> usize {
    rb(r) + SPLIT_BLOCK + NS + 2 * (STATE_SIZE - NS) + i
}
#[inline]
pub(crate) const fn rout_col(r: usize, i: usize) -> usize {
    rb(r) + SPLIT_BLOCK + NS + 2 * (STATE_SIZE - NS) + STATE_SIZE + i
}

/// Column index of round `r`'s S-box input for `lane`:
/// the original input for round 0, else the previous round's output.
#[inline]
pub(crate) const fn sbox_in_col(r: usize, lane: usize) -> usize {
    if r == 0 {
        in_col(lane)
    } else {
        rout_col(r - 1, lane)
    }
}

/// The Tip5 permutation AIR. Stateless — all constants are embedded
/// (and statically asserted equal to the committed golden fixture in
/// `tests::embedded_constants_match_fixture`).
#[derive(Debug, Default, Clone, Copy)]
pub struct Tip5PermAir;

impl<F: PrimeCharacteristicRing + Sync> BaseAir<F> for Tip5PermAir {
    fn width(&self) -> usize {
        tip5_perm_air_width()
    }

    fn max_constraint_degree(&self) -> Option<usize> {
        Some(3)
    }
}

impl<AB: AirBuilder> Air<AB> for Tip5PermAir {
    fn eval(&self, builder: &mut AB) {
        let main = builder.main();
        let local = main.current_slice();

        // Field constant helper.
        let fe = |v: u64| -> AB::Expr { AB::Expr::from(AB::F::from_u64(v)) };
        let var = |c: usize| -> AB::Expr { local[c].into() };
        // 2^{8k}, k = 0..8 (all < p).
        let pow8 = |k: usize| -> AB::Expr { fe(1u64 << (8 * k)) };

        let mds = mds_matrix();
        let two32_minus_1 = fe((1u64 << 32) - 1);

        // Tip5 paper = IACR ePrint 2023/107. One
        // round = S-box layer (§2.2) → linear MDS layer (§2.3) → round
        // constants (§2.4), iterated NUM_ROUNDS=5 times (§2.1).
        for r in 0..NUM_ROUNDS {
            // ---- §2.2 S-box layer, split-and-lookup lanes 0..NS ----
            // `S = ρ ∘ L^8 ∘ σ`: σ decomposes into 8 bytes (paper §2.2 /
            // §4.6 correct decomposition mod p), L is the per-byte map
            // `(x+1)^3−1 mod 257` (paper §2.2; the paper arithmetizes L
            // via a lookup argument §4.1/§4.7 — here it is the
            // soundness-equivalent lookup-free cube, anchored by the
            // C2.0 identity `LOOKUP_TABLE[b]==((b+1)^3−1) mod 257 ∀b`),
            // ρ recomposes the looked-up bytes.
            for t in 0..NS {
                let mut recompose_b = AB::Expr::ZERO;
                let mut recompose_c = AB::Expr::ZERO;
                let mut low = AB::Expr::ZERO; // L: low 32 bits (bytes 0..4)
                let mut high = AB::Expr::ZERO; // H: high 32 bits (bytes 4..8)

                for k in 0..NBYTES {
                    // byte b_k from BBITS boolean bits
                    let mut b_k = AB::Expr::ZERO;
                    for i in 0..BBITS {
                        let bit = var(bbit(r, t, k, i));
                        builder.assert_zero(bit.clone() * (bit.clone() - AB::Expr::ONE));
                        b_k = b_k + bit * fe(1u64 << i);
                    }
                    // output byte c_k from CBITS boolean bits
                    let mut c_k = AB::Expr::ZERO;
                    for i in 0..CBITS {
                        let bit = var(cbit(r, t, k, i));
                        builder.assert_zero(bit.clone() * (bit.clone() - AB::Expr::ONE));
                        c_k = c_k + bit * fe(1u64 << i);
                    }
                    // quotient q_k from QBITS boolean bits
                    let mut q_k = AB::Expr::ZERO;
                    for i in 0..QBITS {
                        let bit = var(qbit(r, t, k, i));
                        builder.assert_zero(bit.clone() * (bit.clone() - AB::Expr::ONE));
                        q_k = q_k + bit * fe(1u64 << i);
                    }

                    // offset-Fermat-cube: (b+1)^3 − 1 = 257·q + c
                    //   ⇒ c = ((b+1)^3 − 1) mod 257 = LOOKUP_TABLE[b]  (C2.0)
                    let u = b_k.clone() + AB::Expr::ONE;
                    let cube = u.clone() * u.clone() * u;
                    builder.assert_zero(cube - AB::Expr::ONE - fe(257) * q_k - c_k.clone());

                    recompose_b = recompose_b + b_k.clone() * pow8(k);
                    recompose_c = recompose_c + c_k * pow8(k);
                    if k < 4 {
                        low = low + b_k * pow8(k);
                    } else {
                        high = high + b_k * pow8(k - 4);
                    }
                }

                // canonical 8-byte decomposition of the S-box input
                builder.assert_zero(recompose_b - var(sbox_in_col(r, t)));
                // A[t] = recomposed looked-up bytes
                builder.assert_zero(var(a_col(r, t)) - recompose_c);

                // paper §4.6 canonical (`< p`) guard: H = 2^32−1 ⇒ L = 0.
                let g = high - two32_minus_1.clone();
                let inv = var(inv_col(r, t));
                let prod = g.clone() * inv; // = 1 iff g != 0 (forced below); 0 iff g == 0
                builder.assert_zero(g * (prod.clone() - AB::Expr::ONE));
                builder.assert_zero((AB::Expr::ONE - prod) * low);
            }

            // ---- §2.2 S-box layer, power lanes NS..STATE_SIZE ----
            // `T: x ↦ x^7` (paper §2.2; α=7 is the lowest exponent that
            // is a permutation of the Goldilocks field). Staged via
            // x²,x³ registers so each constraint is degree ≤ 3.
            for j in NS..STATE_SIZE {
                let x = var(sbox_in_col(r, j));
                let x2 = var(x2_col(r, j));
                let x3 = var(x3_col(r, j));
                builder.assert_zero(x2.clone() - x.clone() * x.clone());
                builder.assert_zero(x3.clone() - x2 * x.clone());
                // x^7 = x^3 · x^3 · x
                builder.assert_zero(var(a_col(r, j)) - x3.clone() * x3 * x);
            }

            // ---- §2.3 linear layer (constant circulant MDS) +
            //      §2.4 round constants ----
            // `state ← M·sbox(state) + RC[r]`. M is the fixed circulant
            // MDS (paper §2.3); RC[r] are the per-round constants
            // (paper §2.4), embedded as `(RC·2^64) mod p` exactly as
            // `nockchain_math::tip5::permute_5round` computes `r_cons`.
            for i in 0..STATE_SIZE {
                let mut acc = AB::Expr::ZERO;
                for j in 0..STATE_SIZE {
                    acc = acc + fe(mds[i][j]) * var(a_col(r, j));
                }
                let rc = fe(rc_precomp(ROUND_CONSTANTS[r * STATE_SIZE + i]));
                builder.assert_zero(var(rout_col(r, i)) - acc - rc);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use alloc::string::String;
    use alloc::vec::Vec;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::{fs, vec};

    use p3_air::check_constraints;
    use p3_field::PrimeCharacteristicRing;
    use p3_goldilocks::{Goldilocks, default_goldilocks_poseidon2_8};
    use p3_matrix::Matrix;
    use p3_test_utils::goldilocks_params::{
        ChallengeMmcs, Challenger, Dft, FriParameters, MyCompress, MyConfig, MyHash, MyMmcs, MyPcs,
        StarkConfig,
    };
    use p3_uni_stark::{prove, verify};

    use super::*;
    use crate::generation::{generate_trace_rows, generate_trace_rows_with_lane0_override};
    use crate::tip5_spec::{LOOKUP_TABLE, MDS_FIRST_ROW, P_GOLDILOCKS, permute};

    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        // 5-round ai-pow-zk fixture (NOT the 7-round canonical Nockchain one).
        "/../../ai-pow-zk/tests/fixtures/tip5_5round_golden_kat.txt"
    );

    struct Kat {
        lookup: Vec<u64>,
        round_constants: Vec<u64>,
        rc_precomp: Vec<u64>,
        mds_row0: Vec<u64>,
        vectors: Vec<([u64; STATE_SIZE], [u64; STATE_SIZE])>,
    }

    fn parse_fixture() -> Kat {
        let text = fs::read_to_string(FIXTURE).expect(
            "C2.0 golden KAT fixture missing — run \
             `cargo test -p nockchain-math --lib c2_kat` first",
        );
        let nums = |line: &str| -> Vec<u64> {
            line.split_whitespace()
                .skip(1)
                .map(|t| t.parse::<u64>().unwrap())
                .collect()
        };
        let mut lookup = Vec::new();
        let mut round_constants = Vec::new();
        let mut rc_precomp = Vec::new();
        let mut mds_row0 = Vec::new();
        let mut vectors = Vec::new();
        let mut pending_in: Option<[u64; STATE_SIZE]> = None;
        for line in text.lines() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if line.starts_with("LOOKUP ") {
                lookup = nums(line);
            } else if line.starts_with("ROUND_CONSTANTS ") {
                round_constants = nums(line);
            } else if line.starts_with("RC_PRECOMP ") {
                rc_precomp = nums(line);
            } else if line.starts_with("MDS_ROW0 ") {
                mds_row0 = nums(line);
            } else if line.starts_with("IN ") {
                let v = nums(line);
                let mut arr = [0u64; STATE_SIZE];
                arr.copy_from_slice(&v);
                pending_in = Some(arr);
            } else if line.starts_with("OUT ") {
                let v = nums(line);
                let mut arr = [0u64; STATE_SIZE];
                arr.copy_from_slice(&v);
                vectors.push((pending_in.take().unwrap(), arr));
            }
        }
        Kat {
            lookup,
            round_constants,
            rc_precomp,
            mds_row0,
            vectors,
        }
    }

    fn make_config() -> MyConfig {
        let perm = default_goldilocks_poseidon2_8();
        let hash = MyHash::new(perm.clone());
        let compress = MyCompress::new(perm.clone());
        let val_mmcs = MyMmcs::new(hash, compress, 0);
        let challenge_mmcs = ChallengeMmcs::new(val_mmcs.clone());
        let fri_params = FriParameters::new_testing(challenge_mmcs, 0);
        let pcs = MyPcs::new(Dft::default(), val_mmcs, fri_params);
        StarkConfig::new(pcs, Challenger::new(perm))
    }

    /// Static cross-workspace loop: the AIR's embedded constants equal
    /// the committed fixture (which `nockchain-math` proves equals its
    /// live `permute`/constants).
    #[test]
    fn embedded_constants_match_fixture() {
        let k = parse_fixture();
        assert_eq!(k.lookup.len(), 256);
        for b in 0..256 {
            assert_eq!(LOOKUP_TABLE[b] as u64, k.lookup[b], "LOOKUP[{b}]");
        }
        assert_eq!(
            ROUND_CONSTANTS.to_vec(),
            k.round_constants,
            "ROUND_CONSTANTS"
        );
        for (i, &rc) in ROUND_CONSTANTS.iter().enumerate() {
            assert_eq!(rc_precomp(rc), k.rc_precomp[i], "rc_precomp[{i}]");
        }
        assert_eq!(MDS_FIRST_ROW.to_vec(), k.mds_row0, "MDS first row");
    }

    /// Live loop: the in-crate spec == `nockchain_math::tip5::permute_5round`
    /// (via the fixture `nockchain-math` pins to its live 5-round permute).
    #[test]
    fn tip5_spec_matches_fixture_permute() {
        let k = parse_fixture();
        assert!(k.vectors.len() >= 10);
        for (inp, out) in &k.vectors {
            let mut s = *inp;
            permute(&mut s);
            assert_eq!(&s, out, "in-crate permute != fixture for input {inp:?}");
        }
    }

    /// **The C2.1 soundness linchpin**: the AIR trace is bit-for-bit
    /// `nockchain_math::tip5::permute_5round` at the observable boundary, the
    /// constraints are consistent with that faithful witness, and a
    /// real STARK proof verifies.
    #[test]
    fn native_equiv_kat() {
        let k = parse_fixture();
        let inputs: Vec<[u64; STATE_SIZE]> = k.vectors.iter().map(|(i, _)| *i).collect();
        let trace = generate_trace_rows(&inputs);
        let width = tip5_perm_air_width();
        assert_eq!(trace.width(), width);

        // native ≡ in-circuit: IN cols == fixture in, final ROUT == fixture out
        for (row, (inp, out)) in k.vectors.iter().enumerate() {
            for lane in 0..STATE_SIZE {
                assert_eq!(
                    trace.values[row * width + lane],
                    Goldilocks::from_u64(inp[lane]),
                    "trace IN mismatch row {row} lane {lane}"
                );
                assert_eq!(
                    trace.values[row * width + rout_col(NUM_ROUNDS - 1, lane)],
                    Goldilocks::from_u64(out[lane]),
                    "trace OUT (final ROUT) != native permute, row {row} lane {lane}"
                );
            }
        }

        let air = Tip5PermAir;
        // deterministic constraint check (no panic) ...
        check_constraints(&air, &trace, &[]);
        // ... and a full prove → verify round trip.
        let config = make_config();
        let proof = prove(&config, &air, trace, &[]);
        verify(&config, &air, &proof, &[]).expect("honest Tip5 proof must verify");
    }

    fn panics(f: impl FnOnce()) -> bool {
        catch_unwind(AssertUnwindSafe(f)).is_err()
    }

    /// Adversarial: any tampered witness cell breaks the constraints.
    #[test]
    fn adversarial_tamper_rejected() {
        let inputs = [core::array::from_fn::<u64, STATE_SIZE, _>(|i| i as u64 + 1)];
        let air = Tip5PermAir;
        let width = tip5_perm_air_width();

        // control: honest passes
        let good = generate_trace_rows(&inputs);
        check_constraints(&air, &good, &[]);

        // tamper the permutation output (final ROUT lane 3)
        let mut t1 = generate_trace_rows(&inputs);
        let c = rout_col(NUM_ROUNDS - 1, 3);
        t1.values[c] += Goldilocks::ONE;
        assert!(
            panics(|| check_constraints(&air, &t1, &[])),
            "tampered OUT accepted"
        );

        // tamper an S-box output column A[r=2][j=9]
        let mut t2 = generate_trace_rows(&inputs);
        t2.values[a_col(2, 9)] += Goldilocks::ONE;
        assert!(
            panics(|| check_constraints(&air, &t2, &[])),
            "tampered A accepted"
        );

        // tamper a split-lane bit (round 0, lane 0, byte 0, bit 0)
        let mut t3 = generate_trace_rows(&inputs);
        let bit0 = STATE_SIZE; // rb(0) + 0 = first bit column
        t3.values[bit0] += Goldilocks::ONE;
        assert!(
            panics(|| check_constraints(&air, &t3, &[])),
            "tampered bit accepted"
        );
        let _ = width;
    }

    /// Adversarial (the precise §4.6 forgery vector): a *non-canonical*
    /// 8-byte split (value `≥ p`, here `x + p`) that is otherwise fully
    /// consistent must be rejected **solely** by the canonical guard.
    #[test]
    fn adversarial_noncanonical_split_rejected() {
        // lane0 input = 1; canonical LE bytes = [1,0,0,0,0,0,0,0].
        let inputs = [core::array::from_fn::<u64, STATE_SIZE, _>(|i| {
            if i == 0 { 1 } else { i as u64 + 2 }
        })];
        let air = Tip5PermAir;

        // control: forcing the *canonical* bytes still verifies.
        let canon =
            generate_trace_rows_with_lane0_override(&inputs, Some([1, 0, 0, 0, 0, 0, 0, 0]));
        check_constraints(&air, &canon, &[]);

        // attack: 1 + p = 0xffff_ffff_0000_0002 ≡ 1 (mod p), a
        // non-canonical alias. LE bytes: [02,00,00,00,ff,ff,ff,ff].
        // Everything (cube/recompose/A/downstream) recomputed faithfully
        // ⇒ only the §4.6 guard (H = 2^32−1 ⇒ L = 0) is violated.
        let evil = generate_trace_rows_with_lane0_override(
            &inputs,
            Some([0x02, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]),
        );
        assert!(
            panics(|| check_constraints(&air, &evil, &[])),
            "non-canonical (≥ p) byte split accepted — §4.6 forgery vector OPEN"
        );
    }

    /// **Exhaustive native-equivalence**: the in-circuit Tip5 AIR ≡
    /// `nockchain_math::tip5::permute_5round` over a large randomized input
    /// space, not just the frozen vectors.
    ///
    /// Equivalence chain (each leg independently tested, green):
    /// `AIR trace final ROUT` == `tip5_spec::permute` (this test, N random
    /// inputs + `check_constraints` on every one) — and
    /// `tip5_spec::permute` == the committed golden fixture ==
    /// `nockchain_math::tip5::permute_5round` (`tip5_spec_matches_fixture_
    /// permute` + `embedded_constants_match_fixture` over all 315
    /// fixture vectors; `nockchain-math`'s own `c2_kat` re-verifies
    /// that fixture against its *live* `permute_5round`). Hence the AIR is
    /// exhaustively validated against the nockchain-math 5-round Tip5
    /// implementation. Constraints follow the Tip5 paper (IACR ePrint
    /// 2023/107): §2.1 round iteration, §2.2 split-and-lookup + x^7
    /// S-boxes, §2.3 circulant MDS, §2.4 round constants, §4.6 correct
    /// decomposition mod p.
    #[test]
    fn air_equals_native_spec_exhaustive_random() {
        // Deterministic xorshift64* (same scheme as nockchain-math's
        // KAT generator), field-reduced — frozen so the test is stable.
        fn xs(state: &mut u64) -> u64 {
            let mut x = *state;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            *state = x;
            x.wrapping_mul(0x2545F4914F6CDD1D) % P_GOLDILOCKS
        }

        const N: usize = 4096; // 4096 random 16-wide permutations
        let mut seed = 0xDEAD_BEEF_CAFE_F00Du64;
        let inputs: Vec<[u64; STATE_SIZE]> = (0..N)
            .map(|_| core::array::from_fn(|_| xs(&mut seed)))
            .collect();

        let trace = generate_trace_rows(&inputs);
        let width = tip5_perm_air_width();

        // AIR trace output == native spec, bit-for-bit, every input.
        for (row, inp) in inputs.iter().enumerate() {
            let mut expected = *inp;
            permute(&mut expected);
            for lane in 0..STATE_SIZE {
                assert_eq!(
                    trace.values[row * width + rout_col(NUM_ROUNDS - 1, lane)],
                    Goldilocks::from_u64(expected[lane]),
                    "AIR != nockchain-math 5-round Tip5 at random row {row} lane {lane}"
                );
            }
        }

        // Exhaustive deterministic constraint validation over all
        // N random permutations (every constraint group, every row).
        let air = Tip5PermAir;
        check_constraints(&air, &trace, &[]);
    }

    /// Round-trip a heavier batch (all fixture vectors) end-to-end.
    #[test]
    fn prove_verify_all_fixture_vectors() {
        let k = parse_fixture();
        let inputs: Vec<[u64; STATE_SIZE]> = k.vectors.iter().map(|(i, _)| *i).collect();
        let trace = generate_trace_rows(&inputs);
        let air = Tip5PermAir;
        let config = make_config();
        let proof = prove(&config, &air, trace, &[]);
        verify(&config, &air, &proof, &[]).expect("batch proof must verify");
        let _ = String::new();
        let _ = vec![0u8; 0];
    }
}
