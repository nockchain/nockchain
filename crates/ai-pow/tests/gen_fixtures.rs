//! Temporary one-shot generator for `tests/fixtures/pearl.rs`.
//!
//! Run with:
//!
//!   cargo test -p ai-pow --test gen_fixtures -- --include-ignored --nocapture
//!
//! Emits Pearl reference outputs for fixed inputs as Rust `const`
//! declarations on stdout. Output is meant to be pasted into
//! `tests/fixtures/pearl.rs`. After the fixtures file is finalized this
//! generator can be regenerated whenever Pearl protocol changes upstream.
//!
//! ## License notice
//!
//! The reference functions defined below are derived line-for-line from
//! the Pearl source files
//!   * `Pearl zk-pow pearl_noise.rs`,
//!   * `Pearl zk-pow api/proof_utils.rs`,
//!   * `Pearl zk-pow ffi/mine.rs`, and
//!   * `Pearl pearl-blake3 merkle.rs`.
//!
//! Pearl is distributed under the ISC license
//! (Copyright (c) 2025-2026 Pearl Research Labs; Copyright (c) 2015-2016
//! The Decred developers). A verbatim copy of the Pearl license is
//! included at `crates/ai-pow/LICENSE-PEARL` and applies to the
//! reference functions in this file.
//!
//! Fixture coverage mirrors the section labels in `tests/fixtures/pearl.rs`:
//!
//!   * **Lock-in (already byte-identical)**: noise reconstruction
//!     (matvec_sparse_perm), tile loop, jackpot hash, difficulty target.
//!   * **Divergent (D1/D2/D3/D4)**: Pearl's PRNG byte stream
//!     (`get_random_hash`), Pearl's permutation pairs, Pearl's
//!     commitment-hash chain (`compute_commitment_hash`), Pearl's
//!     matrix-commitment Merkle root (`blake3_digest(padded, key)`).

#![allow(dead_code)]

use blake3::Hasher;

// ----------------------------------------------------------------------------
// Pearl reference functions (vendored from Pearl zk-pow at this commit).
// ----------------------------------------------------------------------------

const BLAKE3_DIGEST_SIZE: usize = 32;
const CHUNK_LEN: usize = 1024;

// Pearl `Pearl zk-pow pearl_noise.rs:12-16`:
//   const NOISE_RANGE: usize = 128;
//   const IDXS_PER_COL: usize = 2;
//   const UNIFORM_NOISE_RANGE: usize = NOISE_RANGE / IDXS_PER_COL;  // 64
//   const ZERO_POINT_TRANSLATION: i8 = (UNIFORM_NOISE_RANGE / 2) as i8;  // 32
//   const RANGE_MASK: u8 = (UNIFORM_NOISE_RANGE - 1) as u8;  // 63 = 0x3F
const RANGE_MASK: u8 = 0x3F;
const ZERO_POINT_TRANSLATION: i8 = 32;

fn blake3_digest(data: &[u8], key: Option<[u8; 32]>) -> [u8; 32] {
    let mut h = match key {
        Some(k) => Hasher::new_keyed(&k),
        None => Hasher::new(),
    };
    h.update(data);
    *h.finalize().as_bytes()
}

const fn padded_seed_label(label: [u8; 8]) -> [u8; 32] {
    let mut result = [0u8; 32];
    let mut i = 0;
    while i < label.len() {
        result[i] = label[i];
        i += 1;
    }
    result
}

const SEED_LABEL_A: [u8; 32] = padded_seed_label(*b"A_tensor");
const SEED_LABEL_B: [u8; 32] = padded_seed_label(*b"B_tensor");

fn get_random_hash(
    index: usize,
    seed: &[u8; 32],
    key: &[u8; 32],
    prepend_index: usize,
) -> [u8; 32] {
    let mut message = vec![0u8; 64];
    let prepend_value = (1 + index) as i32;
    message[prepend_index * 4..(prepend_index * 4 + 4)]
        .copy_from_slice(&prepend_value.to_le_bytes());
    message[32..64].copy_from_slice(seed);
    blake3_digest(&message, Some(*key))
}

fn generate_uniform_random_matrix(
    seed: &[u8; 32],
    key: &[u8; 32],
    row_indices: &[usize],
    num_cols: usize,
) -> Vec<Vec<i8>> {
    row_indices
        .iter()
        .map(|&row_idx| {
            let start_idx = row_idx * num_cols;
            (start_idx / BLAKE3_DIGEST_SIZE..(start_idx + num_cols).div_ceil(BLAKE3_DIGEST_SIZE))
                .flat_map(|block| {
                    get_random_hash(block, seed, key, 0)
                        .into_iter()
                        .enumerate()
                        .filter_map(move |(k, byte)| {
                            let idx = block * BLAKE3_DIGEST_SIZE + k;
                            (idx >= start_idx && idx < start_idx + num_cols)
                                .then(|| (byte & RANGE_MASK) as i8 - ZERO_POINT_TRANSLATION)
                        })
                })
                .collect()
        })
        .collect()
}

fn mul_hi_u32(a: u32, b: u32) -> u32 {
    ((a as u64 * b as u64) >> 32) as u32
}

fn generate_permutation_matrix(
    seed: &[u8; 32],
    key: &[u8; 32],
    k: usize,
    noise_rank: usize,
) -> Vec<[u32; 2]> {
    const BYTES_PER_LINE: usize = 4;
    const LINES_PER_HASH: usize = BLAKE3_DIGEST_SIZE / BYTES_PER_LINE;
    let rank_mask = (noise_rank - 1) as u32;
    let mut res = vec![[0u32; 2]; k];
    for (i, chunk) in res.chunks_mut(LINES_PER_HASH).enumerate() {
        let random_hash = get_random_hash(i, seed, key, 1);
        for (j, slot) in chunk.iter_mut().enumerate() {
            let random_uint32 = u32::from_le_bytes([
                random_hash[j * 4],
                random_hash[j * 4 + 1],
                random_hash[j * 4 + 2],
                random_hash[j * 4 + 3],
            ]);
            let first_idx = random_uint32 & rank_mask;
            let second_idx = first_idx ^ (1 + mul_hi_u32((noise_rank - 1) as u32, random_uint32));
            *slot = [first_idx, second_idx];
        }
    }
    res
}

fn matvec_sparse_perm(perm: &[[u32; 2]], vec: &[i8]) -> Vec<i8> {
    perm.iter()
        .map(|&[a, b]| (vec[a as usize] as i32 - vec[b as usize] as i32) as i8)
        .collect()
}

fn pearl_tile_loop(
    a_noised: &[Vec<i32>],
    b_noised_t: &[Vec<i32>],
    a_rows: &[usize],
    b_cols: &[usize],
    k: usize,
    rank: usize,
) -> [u32; 16] {
    let mut jackpot_tile: Vec<Vec<i32>> = vec![vec![0; b_cols.len()]; a_rows.len()];
    let mut jackpot = [0u32; 16];
    let mut ll = rank;
    while ll <= k {
        for (u, &a_idx) in a_rows.iter().enumerate() {
            for (v, &b_idx) in b_cols.iter().enumerate() {
                for l in (ll - rank)..ll {
                    jackpot_tile[u][v] += a_noised[a_idx][l] * b_noised_t[b_idx][l];
                }
            }
        }
        let xored = jackpot_tile
            .iter()
            .flatten()
            .fold(0u32, |acc, &x| acc ^ x as u32);
        let tid = (ll / rank - 1) % 16;
        jackpot[tid] = jackpot[tid].rotate_left(13) ^ xored;
        ll += rank;
    }
    jackpot
}

fn compute_jackpot_hash(jackpot: &[u32; 16], commitment_hash: [u8; 32]) -> [u8; 32] {
    let msg: [u8; 64] = std::array::from_fn(|i| jackpot[i / 4].to_le_bytes()[i % 4]);
    blake3_digest(&msg, Some(commitment_hash))
}

fn compute_commitment_hash(
    job_key: &[u8; 32],
    a_row_major: &[u8],
    b_col_major: &[u8],
) -> ([u8; 32], [u8; 32]) {
    let hash_a = blake3_digest(a_row_major, Some(*job_key));
    let hash_b = blake3_digest(b_col_major, Some(*job_key));
    let mut b_seed_input = [0u8; 64];
    b_seed_input[..32].copy_from_slice(job_key);
    b_seed_input[32..].copy_from_slice(&hash_b);
    let b_noise_seed = blake3_digest(&b_seed_input, None);
    let mut a_seed_input = [0u8; 64];
    a_seed_input[..32].copy_from_slice(&b_noise_seed);
    a_seed_input[32..].copy_from_slice(&hash_a);
    let a_noise_seed = blake3_digest(&a_seed_input, None);
    (b_noise_seed, a_noise_seed)
}

fn pad_to_chunk_boundary(data: &[u8]) -> Vec<u8> {
    let mut padded = data.to_vec();
    padded.resize(data.len().div_ceil(CHUNK_LEN) * CHUNK_LEN, 0);
    padded
}

// ----------------------------------------------------------------------------
// Print helpers (emit Rust source).
// ----------------------------------------------------------------------------

fn print_header(section: &str) {
    println!();
    println!("// =================================================================");
    println!("// {section}");
    println!("// =================================================================");
}

fn print_subheader(s: &str) {
    println!("// {s}");
}

fn print_i8_array(name: &str, data: &[i8]) {
    print!("pub const {name}: &[i8] = &[");
    for (i, &v) in data.iter().enumerate() {
        if i % 16 == 0 {
            print!("\n    ");
        }
        print!("{v}, ");
    }
    println!("\n];");
}

fn print_u8_array(name: &str, data: &[u8]) {
    print!("pub const {name}: &[u8] = &[");
    for (i, &v) in data.iter().enumerate() {
        if i % 16 == 0 {
            print!("\n    ");
        }
        print!("0x{v:02x}, ");
    }
    println!("\n];");
}

fn print_u32_pairs(name: &str, data: &[[u32; 2]]) {
    print!("pub const {name}: &[(u32, u32)] = &[");
    for (i, &[a, b]) in data.iter().enumerate() {
        if i % 4 == 0 {
            print!("\n    ");
        }
        print!("({a}, {b}), ");
    }
    println!("\n];");
}

fn print_u32_array(name: &str, data: &[u32]) {
    print!("pub const {name}: [u32; {}] = [", data.len());
    for (i, &v) in data.iter().enumerate() {
        if i % 4 == 0 {
            print!("\n    ");
        }
        print!("0x{v:08x}, ");
    }
    println!("\n];");
}

fn print_bytes32(name: &str, data: &[u8; 32]) {
    print!("pub const {name}: [u8; 32] = [");
    for (i, &v) in data.iter().enumerate() {
        if i % 8 == 0 {
            print!("\n    ");
        }
        print!("0x{v:02x}, ");
    }
    println!("\n];");
}

fn flat_i8(rows: &[Vec<i8>]) -> Vec<i8> {
    rows.iter().flat_map(|r| r.iter().copied()).collect()
}

// ----------------------------------------------------------------------------
// Fixture set.
// ----------------------------------------------------------------------------

#[test]
#[ignore = "fixture generator; run manually with --include-ignored --nocapture"]
fn emit_fixtures() {
    println!("// Auto-generated by `gen_fixtures.rs`. Do not edit by hand.");
    println!("// To regenerate: `cargo test -p ai-pow --test gen_fixtures -- \\");
    println!("//                  --include-ignored --nocapture > tests/pearl_fixtures.rs`");
    println!("//                (then prepend `#![allow(dead_code)]` and the module doc).");
    println!();

    // ========================================================================
    // SECTION 0: protocol constants we share with Pearl (lock-in).
    // ========================================================================
    print_header("SECTION 0 — Pearl protocol constants (lock-in)");
    println!("/// Pearl's `SEED_LABEL_A`: 32 bytes = `b\"A_tensor\"` zero-padded.");
    print_bytes32("PEARL_SEED_LABEL_A", &SEED_LABEL_A);
    println!("/// Pearl's `SEED_LABEL_B`: 32 bytes = `b\"B_tensor\"` zero-padded.");
    print_bytes32("PEARL_SEED_LABEL_B", &SEED_LABEL_B);
    println!("/// Pearl `JACKPOT_SIZE`: 16 × u32 slots in the rolling tile state.");
    println!("pub const PEARL_JACKPOT_SIZE: usize = 16;");
    println!("/// Pearl `LROT_PER_TILE`: 13-bit left rotation per stripe fold.");
    println!("pub const PEARL_LROT_PER_TILE: u32 = 13;");
    println!("/// Pearl `BLAKE3_DIGEST_SIZE`: 32-byte hash output.");
    println!("pub const PEARL_BLAKE3_DIGEST_SIZE: usize = 32;");
    println!("/// Pearl `BLAKE3 CHUNK_LEN`: 1024-byte Merkle chunk size.");
    println!("pub const PEARL_CHUNK_LEN: usize = 1024;");
    println!("/// Pearl uniform-noise sign translation: `(byte & 0x3F) - 32`.");
    println!("pub const PEARL_RANGE_MASK: u8 = 0x3F;");
    println!("pub const PEARL_ZERO_POINT_TRANSLATION: i8 = 32;");

    // ========================================================================
    // SECTION 1 (D3): Pearl PRNG byte streams — `get_random_hash`.
    // ========================================================================
    print_header("SECTION 1 — Pearl PRNG byte streams (D3, divergent)");
    println!("// Pearl's `get_random_hash(index, seed, key, prepend_index)` is:");
    println!("//   message = [0u8; 64]");
    println!("//   message[prepend_index * 4 .. prepend_index * 4 + 4] = (1 + index) as i32 LE");
    println!("//   message[32 .. 64] = seed");
    println!("//   BLAKE3-keyed(message, key)");
    let prng_key: [u8; 32] = [0x91; 32];
    print_bytes32("PEARL_PRNG_KEY", &prng_key);
    println!();
    for prepend_index in 0..2usize {
        for &idx in &[0usize, 1, 7, 31] {
            let h = get_random_hash(idx, &SEED_LABEL_A, &prng_key, prepend_index);
            let name = format!("PEARL_PRNG_A_PI{prepend_index}_I{idx:02}");
            print_bytes32(&name, &h);
        }
    }
    for prepend_index in 0..2usize {
        for &idx in &[0usize, 1, 7, 31] {
            let h = get_random_hash(idx, &SEED_LABEL_B, &prng_key, prepend_index);
            let name = format!("PEARL_PRNG_B_PI{prepend_index}_I{idx:02}");
            print_bytes32(&name, &h);
        }
    }

    // ========================================================================
    // SECTION 2 (D4): permutation pairs over several ranks.
    // ========================================================================
    print_header("SECTION 2 — Pearl permutation pairs (D4, divergent)");
    println!("// Pearl `generate_permutation_matrix(seed, key, k, rank)`:");
    println!("//   for chunk i, hash = get_random_hash(i, seed, key, prepend=1)");
    println!("//   for slot j in chunk: u32 = LE(hash[j*4 .. j*4+4])");
    println!("//     first  = u32 & (rank - 1)");
    println!("//     second = first XOR (1 + mul_hi(rank - 1, u32))");
    let perm_key: [u8; 32] = [0xA3; 32];
    print_bytes32("PEARL_PERM_KEY", &perm_key);
    for &rank in &[32usize, 64, 128] {
        let k = 64usize;
        let perm = generate_permutation_matrix(&SEED_LABEL_A, &perm_key, k, rank);
        let name = format!("PEARL_PERM_K{k}_R{rank}_LABEL_A");
        print_u32_pairs(&name, &perm);
    }

    // ========================================================================
    // SECTION 3 (D3): full noise factors generated via Pearl PRNG.
    //                 These are what Pearl produces internally; we'll match
    //                 them when D3 is closed.
    // ========================================================================
    print_header("SECTION 3 — Pearl uniform-noise matrices (D3, divergent)");
    let noise3_key: [u8; 32] = [0xC5; 32];
    let m3 = 4usize;
    let n3 = 4usize;
    let r3 = 32usize;
    print_bytes32("FIX3_NOISE_KEY", &noise3_key);
    println!("pub const FIX3_M: usize = {m3};");
    println!("pub const FIX3_N: usize = {n3};");
    println!("pub const FIX3_R: usize = {r3};");
    let a_rows3: Vec<usize> = (0..m3).collect();
    let b_cols3: Vec<usize> = (0..n3).collect();
    let e_l_3 = generate_uniform_random_matrix(&SEED_LABEL_A, &noise3_key, &a_rows3, r3);
    let f_r_3 = generate_uniform_random_matrix(&SEED_LABEL_B, &noise3_key, &b_cols3, r3);
    print_i8_array("FIX3_E_L", &flat_i8(&e_l_3));
    print_i8_array("FIX3_F_R", &flat_i8(&f_r_3));

    // ========================================================================
    // SECTION 4 (lock-in): noise reconstruction `matvec_sparse_perm`.
    //                      Inputs (E_L, E_R^T positions, F_R, F_L positions)
    //                      are chosen to *not* depend on Pearl's PRNG so the
    //                      arithmetic can be tested even while D3/D4 are open.
    // ========================================================================
    print_header("SECTION 4 — Noise reconstruction arithmetic (lock-in)");
    println!("// matvec_sparse_perm(perm, vec)[l] = vec[perm[l][0]] - vec[perm[l][1]]");
    println!("// Fixture: m = n = 4, k = 8, r = 4. E_L / F_R / positions hand-picked.");
    let m4 = 4usize;
    let n4 = 4usize;
    let k4 = 8usize;
    let r4 = 4usize;
    println!("pub const FIX4_M: usize = {m4};");
    println!("pub const FIX4_N: usize = {n4};");
    println!("pub const FIX4_K: usize = {k4};");
    println!("pub const FIX4_R: usize = {r4};");
    // E_L: 4 rows × 4 cols, each entry in [-32, 31].
    let e_l_4: Vec<Vec<i8>> = (0..m4)
        .map(|i| {
            (0..r4)
                .map(|p| (i as i32 * 5 + p as i32 * 3 - 16) as i8)
                .collect()
        })
        .collect();
    // E_R^T: k pairs in [0, r-1] distinct.
    let e_r_t_4: Vec<[u32; 2]> = (0..k4)
        .map(|l| [(l % r4) as u32, ((l + 1) % r4) as u32])
        .collect();
    // F_R: n cols × r rows.
    let f_r_4: Vec<Vec<i8>> = (0..n4)
        .map(|j| {
            (0..r4)
                .map(|p| (j as i32 * 7 + p as i32 * 2 - 16) as i8)
                .collect()
        })
        .collect();
    // F_L: k pairs in [0, r-1] distinct.
    let f_l_4: Vec<[u32; 2]> = (0..k4)
        .map(|l| [((l + 2) % r4) as u32, ((l + 3) % r4) as u32])
        .collect();
    print_i8_array("FIX4_E_L", &flat_i8(&e_l_4));
    print_u32_pairs("FIX4_E_R_T_POS", &e_r_t_4);
    print_i8_array("FIX4_F_R", &flat_i8(&f_r_4));
    print_u32_pairs("FIX4_F_L_POS", &f_l_4);
    let noise_a_4: Vec<Vec<i8>> = e_l_4
        .iter()
        .map(|row| matvec_sparse_perm(&e_r_t_4, row))
        .collect();
    let noise_b_t_4: Vec<Vec<i8>> = f_r_4
        .iter()
        .map(|col| matvec_sparse_perm(&f_l_4, col))
        .collect();
    print_i8_array("FIX4_NOISE_A", &flat_i8(&noise_a_4));
    print_i8_array("FIX4_NOISE_B_T", &flat_i8(&noise_b_t_4));

    // ========================================================================
    // SECTION 5 (lock-in): full tile loop.
    // ========================================================================
    print_header("SECTION 5 — Pearl tile loop (lock-in)");
    println!("// Pre-noised matrices (A + noise_A, B + noise_B), tile rows = cols 0..t.");
    let m5 = 8usize;
    let n5 = 8usize;
    let k5 = 64usize;
    let r5 = 32usize;
    let t5 = 4usize;
    println!("pub const FIX5_M: usize = {m5};");
    println!("pub const FIX5_N: usize = {n5};");
    println!("pub const FIX5_K: usize = {k5};");
    println!("pub const FIX5_R: usize = {r5};");
    println!("pub const FIX5_T: usize = {t5};");

    // Build deterministic A, B with entries in [-64, 64].
    let a5: Vec<Vec<i8>> = (0..m5)
        .map(|i| {
            (0..k5)
                .map(|l| ((i as i32 * 7 + l as i32 * 3) % 129 - 64) as i8)
                .collect()
        })
        .collect();
    let b_t_5: Vec<Vec<i8>> = (0..n5)
        .map(|j| {
            (0..k5)
                .map(|l| ((j as i32 * 5 + l as i32 * 2) % 129 - 64) as i8)
                .collect()
        })
        .collect();
    // Pearl-PRNG noise (using arbitrary seeds for the demo); these are part of
    // the lock-in because the arithmetic of the tile loop is independent of
    // how the noise was generated.
    let tile_a_seed: [u8; 32] = [0xD1; 32];
    let tile_b_seed: [u8; 32] = [0xD2; 32];
    print_bytes32("FIX5_A_NOISE_SEED", &tile_a_seed);
    print_bytes32("FIX5_B_NOISE_SEED", &tile_b_seed);
    let a_rows5: Vec<usize> = (0..m5).collect();
    let b_cols5: Vec<usize> = (0..n5).collect();
    let e_l_5 = generate_uniform_random_matrix(&SEED_LABEL_A, &tile_a_seed, &a_rows5, r5);
    let e_r_t_5 = generate_permutation_matrix(&SEED_LABEL_A, &tile_a_seed, k5, r5);
    let f_l_5 = generate_permutation_matrix(&SEED_LABEL_B, &tile_b_seed, k5, r5);
    let f_r_5 = generate_uniform_random_matrix(&SEED_LABEL_B, &tile_b_seed, &b_cols5, r5);
    let noise_a_5: Vec<Vec<i8>> = e_l_5
        .iter()
        .map(|row| matvec_sparse_perm(&e_r_t_5, row))
        .collect();
    let noise_b_t_5: Vec<Vec<i8>> = f_r_5
        .iter()
        .map(|col| matvec_sparse_perm(&f_l_5, col))
        .collect();
    let a_noised5: Vec<Vec<i32>> = a5
        .iter()
        .zip(&noise_a_5)
        .map(|(row, n_row)| {
            row.iter()
                .zip(n_row)
                .map(|(&a, &nz)| a as i32 + nz as i32)
                .collect()
        })
        .collect();
    let b_noised_t_5: Vec<Vec<i32>> = b_t_5
        .iter()
        .zip(&noise_b_t_5)
        .map(|(row, n_row)| {
            row.iter()
                .zip(n_row)
                .map(|(&b, &nz)| b as i32 + nz as i32)
                .collect()
        })
        .collect();
    let a_rows_t5: Vec<usize> = (0..t5).collect();
    let b_cols_t5: Vec<usize> = (0..t5).collect();
    let jackpot5 = pearl_tile_loop(&a_noised5, &b_noised_t_5, &a_rows_t5, &b_cols_t5, k5, r5);
    // Flatten pre-noised slices for our crate's slice API.
    let mut a_prime_flat: Vec<i8> = Vec::with_capacity(t5 * k5);
    for i in 0..t5 {
        for l in 0..k5 {
            a_prime_flat.push(a_noised5[i][l] as i8);
        }
    }
    let mut b_prime_flat: Vec<i8> = Vec::with_capacity(t5 * k5);
    for j in 0..t5 {
        for l in 0..k5 {
            b_prime_flat.push(b_noised_t_5[j][l] as i8);
        }
    }
    print_i8_array("FIX5_A_PRIME_ROWS", &a_prime_flat);
    print_i8_array("FIX5_B_PRIME_COLS", &b_prime_flat);
    print_u32_array("FIX5_JACKPOT", &jackpot5);

    // ========================================================================
    // SECTION 6 (lock-in): keyed jackpot hash.
    // ========================================================================
    print_header("SECTION 6 — Pearl jackpot hash (lock-in)");
    let hash6_key: [u8; 32] = [0x55; 32];
    let hash6 = compute_jackpot_hash(&jackpot5, hash6_key);
    print_bytes32("FIX6_KEY", &hash6_key);
    print_bytes32("FIX6_EXPECTED_HASH", &hash6);

    // ========================================================================
    // SECTION 7 (D1): Pearl commitment-hash chain.
    //   job_key, hash_a, hash_b, s_b, s_a from `compute_commitment_hash`.
    // ========================================================================
    print_header("SECTION 7 — Pearl commitment hash chain (D1, divergent)");
    let job_key7: [u8; 32] = [0x77; 32];
    print_bytes32("FIX7_JOB_KEY", &job_key7);
    // A flattened row-major and B^T flattened row-major (from fixture 5).
    let mut a_bytes7: Vec<u8> = Vec::with_capacity(m5 * k5);
    for i in 0..m5 {
        for l in 0..k5 {
            a_bytes7.push(a5[i][l] as u8);
        }
    }
    let mut b_t_bytes7: Vec<u8> = Vec::with_capacity(n5 * k5);
    for j in 0..n5 {
        for l in 0..k5 {
            b_t_bytes7.push(b_t_5[j][l] as u8);
        }
    }
    println!("pub const FIX7_M: usize = {m5};");
    println!("pub const FIX7_N: usize = {n5};");
    println!("pub const FIX7_K: usize = {k5};");
    print_u8_array("FIX7_A_BYTES_ROW_MAJOR", &a_bytes7);
    print_u8_array("FIX7_B_T_BYTES_ROW_MAJOR", &b_t_bytes7);
    let hash_a7 = blake3_digest(&a_bytes7, Some(job_key7));
    let hash_b7 = blake3_digest(&b_t_bytes7, Some(job_key7));
    let (s_b7, s_a7) = compute_commitment_hash(&job_key7, &a_bytes7, &b_t_bytes7);
    print_bytes32("FIX7_HASH_A", &hash_a7);
    print_bytes32("FIX7_HASH_B", &hash_b7);
    print_bytes32("FIX7_S_B", &s_b7);
    print_bytes32("FIX7_S_A", &s_a7);

    // ========================================================================
    // SECTION 8 (D2): Pearl matrix-commitment Merkle root.
    //   For chunk-aligned A/B^T bytes, the root is just
    //   `blake3_digest(padded, Some(job_key))`. We test with a non-chunk-
    //   aligned size so the padding path is exercised.
    // ========================================================================
    print_header("SECTION 8 — Pearl chunk-Merkle root (D2, divergent)");
    println!("// Hashes are blake3_digest(pad_to_chunk_boundary(raw), Some(key)).");
    println!("// `raw` = m * k bytes for A, n * k bytes for B^T.");
    println!();
    // Same fixture as section 7 (already 512 bytes, well below one 1024 chunk).
    let padded_a8 = pad_to_chunk_boundary(&a_bytes7);
    let padded_b8 = pad_to_chunk_boundary(&b_t_bytes7);
    println!("pub const FIX8_RAW_A_LEN: usize = {};", a_bytes7.len());
    println!("pub const FIX8_RAW_B_LEN: usize = {};", b_t_bytes7.len());
    println!("pub const FIX8_PADDED_A_LEN: usize = {};", padded_a8.len());
    println!("pub const FIX8_PADDED_B_LEN: usize = {};", padded_b8.len());
    let merkle_a8 = blake3_digest(&padded_a8, Some(job_key7));
    let merkle_b8 = blake3_digest(&padded_b8, Some(job_key7));
    print_bytes32("FIX8_MERKLE_ROOT_A", &merkle_a8);
    print_bytes32("FIX8_MERKLE_ROOT_B", &merkle_b8);

    // Also include a multi-chunk fixture: 3 KB of data should hit a real
    // BLAKE3 chunk-tree path.
    println!();
    println!("// 3 KB fixture (3 chunks worth) to exercise the BLAKE3 chunk-tree path.");
    let big_raw_a: Vec<u8> = (0..3000u32).map(|i| (i % 256) as u8).collect();
    let big_padded = pad_to_chunk_boundary(&big_raw_a);
    let big_merkle = blake3_digest(&big_padded, Some(job_key7));
    println!("pub const FIX8_BIG_RAW_LEN: usize = {};", big_raw_a.len());
    println!(
        "pub const FIX8_BIG_PADDED_LEN: usize = {};",
        big_padded.len()
    );
    print_u8_array("FIX8_BIG_RAW", &big_raw_a);
    print_bytes32("FIX8_BIG_MERKLE_ROOT", &big_merkle);

    // ========================================================================
    // SECTION 9 (lock-in): difficulty target & comparison.
    // ========================================================================
    print_header("SECTION 9 — Difficulty target (lock-in, little-endian)");
    println!("// Pearl interprets BLAKE3 output as U256::from_little_endian.");
    println!("// Our target_from_weight produces 2^(256 - b) * r * t^2 in 32-byte LE.");
    println!("// Hand-computed reference values for several (b, r, t) tuples.");
    let cases: &[(u32, u32, u32)] = &[
        (0, 4, 8),
        (8, 4, 8),
        (16, 32, 4),
        (128, 64, 128),
        (256, 4, 8),
        (256, 32, 4),
        (400, 4, 8),
    ];
    println!("/// (b, r, t, expected_LE_bytes)");
    println!("pub const FIX9_TARGETS: &[(u32, u32, u32, [u8; 32])] = &[");
    for &(b, r, t) in cases {
        let target = target_from_weight_ref(b, (r as u128) * (t as u128) * (t as u128));
        print!("    ({b}, {r}, {t}, [");
        for v in &target {
            print!("0x{v:02x},");
        }
        println!("]),");
    }
    println!("];");
}

// Reference implementation of our `target_from_weight`, kept self-contained
// inside the generator so we don't import private functions from the crate.
// Matches `crates/ai-pow/src/tile_hash.rs::target_from_weight` exactly.
fn target_from_weight_ref(b: u32, weight: u128) -> [u8; 32] {
    if weight == 0 {
        return [0u8; 32];
    }
    if b == 0 {
        return [0xFFu8; 32];
    }
    if b >= 256 + 128 {
        return [0u8; 32];
    }
    let shift = 256i32 - b as i32;
    let (hi, lo): (u128, u128) = if shift >= 128 {
        let s = (shift - 128) as u32;
        if s > 0 && (weight >> (128 - s)) != 0 {
            return [0xFFu8; 32];
        }
        let hi = if s == 128 { 0 } else { weight << s };
        (hi, 0u128)
    } else if shift > 0 {
        let s = shift as u32;
        let lo = if s == 0 { weight } else { weight << s };
        let hi = if s == 0 { 0 } else { weight >> (128 - s) };
        (hi, lo)
    } else {
        let s = (-shift) as u32;
        if s >= 128 {
            return [0u8; 32];
        }
        (0u128, weight >> s)
    };
    let mut out = [0u8; 32];
    out[..16].copy_from_slice(&lo.to_le_bytes());
    out[16..].copy_from_slice(&hi.to_le_bytes());
    out
}
