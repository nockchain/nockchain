//! Trace generation for [`crate::Tip5PermLookupAir`].
//!
//! Builds the single main trace (`256` verifier-table rows ++ `5P`
//! round rows ++ inert padding) and the matching preprocessed L-table
//! plus round selectors, witnessing the recursive 5-round Tip5 from
//! [`crate::tip5_spec`]. `(first-round IN, final-round OUT)` of every
//! five-row permutation window equals `nockchain_math::tip5::permute_5round`
//! bit-for-bit (asserted by the `air_lookup::tests` native-equivalence gate).
//!
//! **Parallelism (2026-05-21).** The `P` permutation rows are
//! filled in parallel via `par_chunks_exact_mut + par_fold_reduce`
//! when the `parallel` feature is on:
//!
//! - **Row content** is a deterministic function of the row's input
//!   `inp` + the immutable constants (MDS matrix, LOOKUP_TABLE,
//!   ROUND_CONSTANTS). No cross-row data flow ⇒ rows can be
//!   computed in any order without contention.
//! - **`mult[TABLE_ROWS]`** (the byte-frequency counter into the
//!   L-table) is the only shared state. Each thread maintains a
//!   thread-local `[u64; TABLE_ROWS]` accumulator and the closure
//!   returns it; the reduce step is element-wise integer sum
//!   (associative + commutative), so the final array is identical
//!   regardless of how Rayon chunks/orders the work.
//!
//! **Bit-identity guarantee:** the resulting `(main_trace, prep)`
//! is byte-for-byte identical to the serial path's output for any
//! input — both the trace cells (per-row, deterministic) and the
//! L-table multiplicities (sum reduction is order-invariant). The
//! `air_lookup::tests` native-equivalence + the C2 Tip5 AIR KAT
//! gates remain the binding correctness checks; parallel just
//! re-orders the work.

use alloc::vec;
use alloc::vec::Vec;

use p3_goldilocks::Goldilocks;
use p3_matrix::dense::RowMajorMatrix;
use p3_maybe_rayon::prelude::*;

use crate::air_lookup::{NBYTES, NS, PREP_WIDTH, TABLE_ROWS, rout_col, tip5_lookup_air_width};
use crate::tip5_spec::{
    LOOKUP_TABLE, NUM_ROUNDS, P_GOLDILOCKS, ROUND_CONSTANTS, STATE_SIZE, mds_matrix, rc_precomp,
};

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

// flat main-trace column indices (mirror air_lookup.rs)
const C_TMULT: usize = 0;
const C_TIN: usize = 1;
const C_IN: usize = C_TIN + STATE_SIZE;
const SPLIT_BC: usize = NS * 2 * NBYTES;
const C_SPLIT: usize = C_IN + STATE_SIZE;
const C_INV: usize = C_SPLIT + SPLIT_BC;
#[inline]
fn b_col(t: usize, k: usize) -> usize {
    C_SPLIT + t * (2 * NBYTES) + k
}
#[inline]
fn c_col(t: usize, k: usize) -> usize {
    C_SPLIT + t * (2 * NBYTES) + NBYTES + k
}
#[inline]
fn inv_col(t: usize) -> usize {
    C_INV + t
}
#[inline]
fn set_row(row: &mut [Goldilocks], col: usize, v: u64) {
    row[col] = Goldilocks::new(v % P_GOLDILOCKS);
}

/// Fill a single permutation row in-place from `inp`, accumulating
/// byte-key frequencies into `mult`.
///
/// Pure function of `inp` + (`mds`, constants); writes to `row` only
/// and increments `mult` only. Safe to call from any thread on any
/// disjoint `(row, mult)` pair.
#[inline]
fn fill_perm_rows(
    rows: &mut [Goldilocks],
    inp: &[u64; STATE_SIZE],
    mds: &[[u64; STATE_SIZE]; STATE_SIZE],
    mult: &mut [u64; TABLE_ROWS],
) {
    let mut state = *inp;
    for r in 0..NUM_ROUNDS {
        let row = &mut rows[r * tip5_lookup_air_width()..(r + 1) * tip5_lookup_air_width()];
        for lane in 0..STATE_SIZE {
            set_row(row, C_TIN + lane, inp[lane]);
            set_row(row, C_IN + lane, state[lane]);
        }
        let sbox_in = state;
        let mut a = [0u64; STATE_SIZE];
        for t in 0..NS {
            let bytes = sbox_in[t].to_le_bytes();
            let mut cb = [0u8; NBYTES];
            for k in 0..NBYTES {
                let b = bytes[k];
                let c = LOOKUP_TABLE[b as usize];
                mult[b as usize] += 1;
                cb[k] = c;
                set_row(row, b_col(t, k), b as u64);
                set_row(row, c_col(t, k), c as u64);
            }
            a[t] = u64::from_le_bytes(cb);
            let high = (sbox_in[t] >> 32) & 0xffff_ffff;
            let g = ((high as u128) + P - ((1u128 << 32) - 1)) % P;
            set_row(row, inv_col(t), finv(g) as u64);
        }
        for j in NS..STATE_SIZE {
            let x = sbox_in[j];
            let x2 = fmul(x, x);
            let x3 = fmul(x2, x);
            a[j] = fmul(fmul(x3, x3), x);
        }
        for i in 0..STATE_SIZE {
            let mut acc = 0u64;
            for (j, &aj) in a.iter().enumerate() {
                acc = fadd(acc, fmul(mds[i][j], aj));
            }
            let out = fadd(acc, rc_precomp(ROUND_CONSTANTS[r * STATE_SIZE + i]));
            set_row(row, rout_col(i), out);
            state[i] = out;
        }
    }
}

/// Generate `(main_trace, preprocessed_flat)` for `inputs`.
///
/// `preprocessed_flat` is row-major `PREP_WIDTH`-wide, height = main
/// height: `[IS_TABLE, TIN, TOUT]`, the verifier-fixed
/// `(i, LOOKUP_TABLE[i])` on the first `TABLE_ROWS` rows.
///
/// Parallel by default (Rayon over the P permutation rows + sum
/// reduction over thread-local mult counters); falls back to serial
/// when the `parallel` feature is off. Output is bit-identical to
/// the serial path for any input (see module doc-comment).
pub fn generate_lookup_trace(
    inputs: &[[u64; STATE_SIZE]],
) -> (RowMajorMatrix<Goldilocks>, Vec<Goldilocks>) {
    let width = tip5_lookup_air_width();
    let p = inputs.len();
    let round_rows = p * NUM_ROUNDS;
    let height = (TABLE_ROWS + round_rows).max(1).next_power_of_two();

    let mut main = vec![Goldilocks::new(0); height * width];
    let mut prep = vec![Goldilocks::new(0); height * PREP_WIDTH];
    let mds = mds_matrix();

    // ---- preprocessed verifier-fixed L-table on rows [0, 256) ----
    // Serial fill: only TABLE_ROWS=256 rows, ~negligible compared
    // to the heavy permutation work below.
    for (i, tbl) in LOOKUP_TABLE.iter().enumerate() {
        prep[i * PREP_WIDTH] = Goldilocks::new(1); // IS_TABLE
        prep[i * PREP_WIDTH + 1] = Goldilocks::new(i as u64); // TIN
        prep[i * PREP_WIDTH + 2] = Goldilocks::new(*tbl as u64); // TOUT
    }
    for pi in 0..p {
        for round in 0..NUM_ROUNDS {
            let row = TABLE_ROWS + pi * NUM_ROUNDS + round;
            let base = row * PREP_WIDTH;
            prep[base + 3] = Goldilocks::new(1); // IS_ROUND
            prep[base + 4 + round] = Goldilocks::new(1);
        }
    }

    // ---- permutation rows [TABLE_ROWS, TABLE_ROWS + p) — PARALLEL ----
    //
    // Each row's columns are a deterministic function of its input
    // and constants ⇒ no cross-row data flow ⇒ Rayon can fan over
    // rows freely. The byte-frequency multiplicities `mult[0..256]`
    // are the only shared state; we accumulate per-thread via
    // `par_fold_reduce` and combine with element-wise sum (which is
    // associative + commutative ⇒ order-independent).
    //
    // `par_fold_reduce` (from `p3_maybe_rayon::prelude::SharedExt`)
    // gives identical numerical output in both parallel (`fold` per
    // chunk + `reduce` across chunks) and serial (single `fold` over
    // all items) modes — the trace produced here is BIT-IDENTICAL
    // to what a single-threaded loop would produce, modulo
    // reorderings that don't affect the output because each row's
    // content is independent and `mult` is summed via an
    // associative+commutative operation.
    let perm_start = TABLE_ROWS * width;
    let perm_end = (TABLE_ROWS + round_rows) * width;
    let perm_slice = &mut main[perm_start..perm_end];
    let mult: [u64; TABLE_ROWS] = perm_slice
        .par_chunks_exact_mut(NUM_ROUNDS * width)
        .zip(inputs.par_iter())
        .par_fold_reduce(
            || [0u64; TABLE_ROWS],
            |mut local_mult, (rows, inp)| {
                fill_perm_rows(rows, inp, &mds, &mut local_mult);
                local_mult
            },
            |mut a, b| {
                for i in 0..TABLE_ROWS {
                    a[i] += b[i];
                }
                a
            },
        );

    // ---- table-row multiplicities (rows [0, TABLE_ROWS)) ----
    // Serial fill: only TABLE_ROWS=256 rows, fast.
    for (i, &m) in mult.iter().enumerate() {
        let base = i * width;
        // KIND already 0; TMULT = how many perm-row byte queries hit i
        set_row(&mut main[base..base + width], C_TMULT, m % P_GOLDILOCKS);
    }

    (RowMajorMatrix::new(main, width), prep)
}
