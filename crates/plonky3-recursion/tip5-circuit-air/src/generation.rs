//! Witness/trace generation for [`crate::Tip5PermAir`].
//!
//! Fills, per input, one row encoding the full 5-round Tip5 evaluation
//! exactly as the AIR constraints expect. Values are computed with the
//! native-faithful [`crate::tip5_spec`] arithmetic; the resulting
//! `(IN, ROUT[NUM_ROUNDS-1])` is asserted bit-identical to the committed golden
//! fixture by `air::tests::native_equiv_kat`.

use alloc::vec;
use alloc::vec::Vec;

use p3_goldilocks::Goldilocks;
use p3_matrix::dense::RowMajorMatrix;

use crate::air::{
    BBITS, CBITS, NBYTES, NS, QBITS, a_col, rout_col, sbox_in_col, tip5_perm_air_width,
};
use crate::tip5_spec::{
    LOOKUP_TABLE, NUM_ROUNDS, P_GOLDILOCKS, ROUND_CONSTANTS, STATE_SIZE, mds_matrix, rc_precomp,
};

// flat index helpers (re-derived; mirror air.rs — kept private there)
const BYTE_BLOCK: usize = BBITS + CBITS + QBITS;
const SPLIT_BLOCK: usize = NS * NBYTES * BYTE_BLOCK;
#[inline]
fn rb(r: usize) -> usize {
    STATE_SIZE + r * crate::air::ROUND_GROUP
}
#[inline]
fn bbit(r: usize, t: usize, k: usize, i: usize) -> usize {
    rb(r) + (t * NBYTES + k) * BYTE_BLOCK + i
}
#[inline]
fn cbit(r: usize, t: usize, k: usize, i: usize) -> usize {
    rb(r) + (t * NBYTES + k) * BYTE_BLOCK + BBITS + i
}
#[inline]
fn qbit(r: usize, t: usize, k: usize, i: usize) -> usize {
    rb(r) + (t * NBYTES + k) * BYTE_BLOCK + BBITS + CBITS + i
}
#[inline]
fn inv_col(r: usize, t: usize) -> usize {
    rb(r) + SPLIT_BLOCK + t
}
#[inline]
fn x2_col(r: usize, j: usize) -> usize {
    rb(r) + SPLIT_BLOCK + NS + (j - NS)
}
#[inline]
fn x3_col(r: usize, j: usize) -> usize {
    rb(r) + SPLIT_BLOCK + NS + (STATE_SIZE - NS) + (j - NS)
}

const P: u128 = P_GOLDILOCKS as u128;

#[inline]
fn modpow(mut base: u128, mut exp: u128) -> u128 {
    base %= P;
    let mut acc = 1u128;
    while exp > 0 {
        if exp & 1 == 1 {
            acc = acc * base % P;
        }
        base = base * base % P;
        exp >>= 1;
    }
    acc
}

/// Field inverse mod p (0 ↦ 0; only used for the §4.6 guard witness,
/// where the value is unconstrained when `g == 0`).
#[inline]
fn finv(a: u128) -> u128 {
    let a = a % P;
    if a == 0 { 0 } else { modpow(a, P - 2) }
}

#[inline]
fn fmul(a: u64, b: u64) -> u64 {
    ((a as u128) * (b as u128) % P) as u64
}
#[inline]
fn fadd(a: u64, b: u64) -> u64 {
    (((a as u128) + (b as u128)) % P) as u64
}

/// Test-support: like [`generate_trace_rows`] but row 0's round-0,
/// lane-0 byte decomposition is forced to `override0` instead of the
/// canonical `.to_le_bytes()`. The override is propagated *faithfully*
/// (its L-images, recompositions, and the entire downstream 5-round
/// permutation are recomputed from it) so **every** constraint stays
/// satisfied *except* the paper §4.6 canonical guard when `override0`
/// is a non-canonical (`≥ p`) alias — isolating exactly the forgery
/// vector that guard exists to block.
#[doc(hidden)]
#[allow(dead_code)]
pub fn generate_trace_rows_with_lane0_override(
    inputs: &[[u64; STATE_SIZE]],
    override0: Option<[u8; NBYTES]>,
) -> RowMajorMatrix<Goldilocks> {
    generate_inner(inputs, override0)
}

/// Generate the trace for `inputs` (one row per permutation, padded to
/// a power of two with the genuine zero-input permutation row).
pub fn generate_trace_rows(inputs: &[[u64; STATE_SIZE]]) -> RowMajorMatrix<Goldilocks> {
    generate_inner(inputs, None)
}

fn generate_inner(
    inputs: &[[u64; STATE_SIZE]],
    override0: Option<[u8; NBYTES]>,
) -> RowMajorMatrix<Goldilocks> {
    let width = tip5_perm_air_width();
    let n = inputs.len().max(1);
    let height = n.next_power_of_two();

    let mut values = vec![Goldilocks::new(0); height * width];
    let mds = mds_matrix();
    let zero_input = [0u64; STATE_SIZE];

    for row in 0..height {
        let inp = if row < inputs.len() {
            &inputs[row]
        } else {
            &zero_input
        };
        let base = row * width;
        let set = |values: &mut Vec<Goldilocks>, col: usize, v: u64| {
            values[base + col] = Goldilocks::new(v % P_GOLDILOCKS);
        };

        // input columns
        for lane in 0..STATE_SIZE {
            set(&mut values, lane, inp[lane]);
        }

        let mut state = *inp;
        for r in 0..NUM_ROUNDS {
            let sbox_in = state;
            let mut a = [0u64; STATE_SIZE];

            // split-and-lookup lanes
            for t in 0..NS {
                let bytes = match override0 {
                    Some(ob) if row == 0 && r == 0 && t == 0 => ob,
                    _ => sbox_in[t].to_le_bytes(),
                };
                let mut cbytes = [0u8; NBYTES];
                for k in 0..NBYTES {
                    let b = bytes[k] as u64;
                    let c = LOOKUP_TABLE[b as usize] as u64;
                    let cube = {
                        let u = b + 1;
                        u * u * u
                    };
                    debug_assert_eq!((cube - 1) % 257, c, "C2.0 identity must hold");
                    let q = (cube - 1) / 257;
                    cbytes[k] = c as u8;
                    for i in 0..BBITS {
                        set(&mut values, bbit(r, t, k, i), (b >> i) & 1);
                    }
                    for i in 0..CBITS {
                        set(&mut values, cbit(r, t, k, i), (c >> i) & 1);
                    }
                    for i in 0..QBITS {
                        set(&mut values, qbit(r, t, k, i), (q >> i) & 1);
                    }
                }
                a[t] = u64::from_le_bytes(cbytes);

                // §4.6 guard witness: inv = (H − (2^32−1))^{-1} (or 0 if zero)
                let high = (sbox_in[t] >> 32) & 0xffff_ffff;
                let g = ((high as u128) + P - ((1u128 << 32) - 1)) % P;
                set(&mut values, inv_col(r, t), finv(g) as u64);
            }

            // power lanes  x^7
            for j in NS..STATE_SIZE {
                let x = sbox_in[j];
                let x2 = fmul(x, x);
                let x3 = fmul(x2, x);
                a[j] = fmul(fmul(x3, x3), x);
                set(&mut values, x2_col(r, j), x2);
                set(&mut values, x3_col(r, j), x3);
            }

            // A columns
            for i in 0..STATE_SIZE {
                set(&mut values, a_col(r, i), a[i]);
            }

            // MDS + round constants → ROUT (= next round input)
            for i in 0..STATE_SIZE {
                let mut acc = 0u64;
                for j in 0..STATE_SIZE {
                    acc = fadd(acc, fmul(mds[i][j], a[j]));
                }
                let out = fadd(acc, rc_precomp(ROUND_CONSTANTS[r * STATE_SIZE + i]));
                set(&mut values, rout_col(r, i), out);
                state[i] = out;
            }
        }
        // sanity: round-0 sbox input columns line up with the input
        debug_assert_eq!(sbox_in_col(0, 0), 0);
    }

    RowMajorMatrix::new(values, width)
}
