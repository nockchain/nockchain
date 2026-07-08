//! §4.C.2 / Phase-A3.0 — off-circuit Pearl low-rank-noise
//! reference (the **canonical spec** the in-circuit B2 sub-AIR,
//! A3.1/A3.2, must reproduce).
//!
//! Bit-for-bit mirror of `ai-pow::prng` + `ai-pow::matmul`'s
//! `BlockNoise` for the *single value* a verifier needs:
//!
//! ```text
//! a_prime[i,l] = A[i,l] + E[i,l],   E[i,l] = E_L[i,pp_l] − E_L[i,pm_l]
//! b_prime[l,j] = B[l,j] + F[l,j],   F[l,j] = F_R[j,pp_l] − F_R[j,pm_l]
//! ```
//!
//! with `E_L[i,·] = fill_uniform_row(SEED_A, s_a, i, r)` (`r`
//! signed-6-bit, BLAKE3(s_a)-keyed), `(pp_l,pm_l) =
//! perm_pair(SEED_A, s_a, l, r)` (per-**column** distinct pair),
//! and the `F`/`s_b` side symmetric. `ai-pow-zk` must **not**
//! depend on `ai-pow` (dep cycle), so this re-derives from the
//! same `blake3` primitive; the decisive cross-crate
//! byte-equivalence KAT (`noise_ref` == `ai-pow::BlockNoise`)
//! lives on the `ai-pow` side (it may depend on `ai-pow-zk`).
//!
//! Pure / off-circuit — no AIR, no trace. De-risks the spec
//! before any sub-AIR work (the P-B.2.0 KAT-first discipline).

use blake3::Hasher;

/// `b"A_tensor"` zero-padded to 32 — Pearl `SEED_LABEL_A`
/// (`ai-pow::prng::SEED_LABEL_A`).
pub const SEED_LABEL_A: [u8; 32] = pad_label(*b"A_tensor");
/// `b"B_tensor"` zero-padded to 32 — Pearl `SEED_LABEL_B`.
pub const SEED_LABEL_B: [u8; 32] = pad_label(*b"B_tensor");

const PEARL_DIGEST_SIZE: usize = 32;
const PEARL_PAIRS_PER_HASH: usize = 8;

const fn pad_label(label: [u8; 8]) -> [u8; 32] {
    let mut r = [0u8; 32];
    let mut i = 0;
    while i < 8 {
        r[i] = label[i];
        i += 1;
    }
    r
}

/// Pearl `get_random_hash` — BLAKE3-keyed hash of a 64-byte
/// message: `(1+index) as i32` LE at `prepend*4`, `seed` at
/// `[32..64]`, key = the per-block noise seed (`s_a`/`s_b`).
pub fn pearl_random_hash(
    index: usize,
    seed: &[u8; 32],
    key: &[u8; 32],
    prepend: usize,
) -> [u8; 32] {
    let mut message = [0u8; 64];
    let prepend_value = (1 + index) as i32;
    message[prepend * 4..prepend * 4 + 4].copy_from_slice(&prepend_value.to_le_bytes());
    message[32..64].copy_from_slice(seed);
    *Hasher::new_keyed(key)
        .update(&message)
        .finalize()
        .as_bytes()
}

/// `(b & 0x3F) - 32` ∈ `[-32, 31]` (Pearl `E_L`/`F_R` 6-bit).
#[inline]
pub fn byte_to_i6(b: u8) -> i8 {
    ((b & 0x3F) as i32 - 32) as i8
}

#[inline]
fn mul_hi_u32(a: u32, b: u32) -> u32 {
    ((a as u64 * b as u64) >> 32) as u32
}

/// Pearl `fill_uniform_row`: row `row_idx` (length `num_cols`,
/// values `[-32,31]`) of the flat 6-bit uniform stream — used
/// for an `E_L` row (`seed=SEED_A,key=s_a`) or `F_R` column
/// (`seed=SEED_B,key=s_b`).
pub fn fill_uniform_row(seed: &[u8; 32], key: &[u8; 32], row_idx: u32, num_cols: u32) -> Vec<i8> {
    let num_cols = num_cols as usize;
    let mut out = vec![0i8; num_cols];
    let start = row_idx as usize * num_cols;
    let end = start + num_cols;
    let first_block = start / PEARL_DIGEST_SIZE;
    let last_block = end.div_ceil(PEARL_DIGEST_SIZE);
    for block in first_block..last_block {
        let hash = pearl_random_hash(block, seed, key, 0);
        let block_start = block * PEARL_DIGEST_SIZE;
        let lo = start.saturating_sub(block_start).min(PEARL_DIGEST_SIZE);
        let hi = (end - block_start).min(PEARL_DIGEST_SIZE);
        for k in lo..hi {
            out[block_start + k - start] = byte_to_i6(hash[k]);
        }
    }
    out
}

/// Pearl `pearl_permutation_pair`: the per-index distinct
/// `(p_plus, p_minus)` ∈ `[0,r)` (`r` a power of two). For an
/// `E_R` column (`seed=SEED_A,key=s_a`) / `F_L` row
/// (`seed=SEED_B,key=s_b`).
pub fn perm_pair(seed: &[u8; 32], key: &[u8; 32], j: u32, r: u32) -> (u32, u32) {
    debug_assert!(r >= 2 && r.is_power_of_two());
    let chunk_idx = j as usize / PEARL_PAIRS_PER_HASH;
    let slot = j as usize % PEARL_PAIRS_PER_HASH;
    let hash = pearl_random_hash(chunk_idx, seed, key, 1);
    let off = slot * 4;
    let rnd = u32::from_le_bytes([hash[off], hash[off + 1], hash[off + 2], hash[off + 3]]);
    let mask = r - 1;
    let first = rnd & mask;
    let second = first ^ (1 + mul_hi_u32(mask, rnd));
    (first, second)
}

/// `E[i,l] = E_L[i,pp_l] − E_L[i,pm_l]` — the per-element `A`
/// noise (`a_prime[i,l] = A[i,l] + e_value(...)`). `s_a` is the
/// C1-pinned `COMMITMENT_HASH`.
pub fn e_value(s_a: &[u8; 32], i: u32, l: u32, r: u32) -> i8 {
    let e_l = fill_uniform_row(&SEED_LABEL_A, s_a, i, r);
    let (pp, pm) = perm_pair(&SEED_LABEL_A, s_a, l, r);
    e_l[pp as usize].wrapping_sub(e_l[pm as usize])
}

/// `F[l,j] = F_R[j,pp_l] − F_R[j,pm_l]` — the per-element `B`
/// noise (`b_prime[l,j] = B[l,j] + f_value(...)`, B col-major).
pub fn f_value(s_b: &[u8; 32], l: u32, j: u32, r: u32) -> i8 {
    let f_r = fill_uniform_row(&SEED_LABEL_B, s_b, j, r);
    let (pp, pm) = perm_pair(&SEED_LABEL_B, s_b, l, r);
    f_r[pp as usize].wrapping_sub(f_r[pm as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_labels_match_pearl() {
        assert_eq!(&SEED_LABEL_A[..8], b"A_tensor");
        assert_eq!(SEED_LABEL_A[8..], [0u8; 24]);
        assert_eq!(&SEED_LABEL_B[..8], b"B_tensor");
    }

    #[test]
    fn i6_range_and_uniform_row_bounds() {
        for b in 0..=255u8 {
            let v = byte_to_i6(b);
            assert!((-32..=31).contains(&v), "i6 {v} out of [-32,31]");
        }
        let s = [0x5Au8; 32];
        for r in [32u32, 64, 128, 1024] {
            for i in 0..4 {
                let row = fill_uniform_row(&SEED_LABEL_A, &s, i, r);
                assert_eq!(row.len() as u32, r);
                assert!(row.iter().all(|&v| (-32..=31).contains(&v)));
            }
        }
    }

    #[test]
    fn perm_pairs_distinct_and_in_range() {
        let s = [0xA5u8; 32];
        for &r in &[2u32, 32, 64, 256, 1024] {
            for j in 0..(3 * PEARL_PAIRS_PER_HASH as u32 + 1) {
                let (pp, pm) = perm_pair(&SEED_LABEL_A, &s, j, r);
                assert!(pp < r && pm < r, "pair ({pp},{pm}) out of [0,{r})");
                assert_ne!(pp, pm, "pair must be distinct @ j={j}, r={r}");
            }
        }
    }

    /// `e_value` is exactly the select-subtract of the `E_L`
    /// row at the per-column pair — the relation the in-circuit
    /// B2 sub-AIR must reproduce.
    #[test]
    fn e_value_is_el_select_subtract() {
        let s_a = [0x37u8; 32];
        let r = 64u32;
        for i in [0u32, 1, 5, 17] {
            let e_l = fill_uniform_row(&SEED_LABEL_A, &s_a, i, r);
            for l in [0u32, 1, 7, 8, 63, 100] {
                let (pp, pm) = perm_pair(&SEED_LABEL_A, &s_a, l, r);
                let want = e_l[pp as usize].wrapping_sub(e_l[pm as usize]);
                assert_eq!(e_value(&s_a, i, l, r), want);
            }
        }
        // F side symmetric (col-major F_R, perm keyed by s_b).
        let s_b = [0x9Cu8; 32];
        let f_r = fill_uniform_row(&SEED_LABEL_B, &s_b, 3, r);
        let (pp, pm) = perm_pair(&SEED_LABEL_B, &s_b, 9, r);
        assert_eq!(
            f_value(&s_b, 9, 3, r),
            f_r[pp as usize].wrapping_sub(f_r[pm as usize])
        );
    }
}
