//! Cross-implementation tests against Pearl reference outputs, baked in
//! as constants in `tests/fixtures/pearl.rs` so no Pearl source is needed
//! at test time. The fixtures were generated once by `tests/gen_fixtures.rs`
//! (the vendored generator); re-run that test with `--include-ignored
//! --nocapture` if Pearl protocol changes upstream.
//!
//! Coverage maps to the section labels in `tests/fixtures/pearl.rs`:
//!
//!   * **S0** (lock-in) — Pearl protocol constants we share.
//!   * **S1** (D3) — `get_random_hash` byte stream. **#[ignore]**: our PRNG
//!     uses `derive_key`, not Pearl's keyed-`get_random_hash`. Remove the
//!     ignore when D3 is closed.
//!   * **S2** (D4) — permutation pairs. **#[ignore]**: our generator uses
//!     rejection sampling, not Pearl's XOR trick.
//!   * **S3** (D3) — uniform-noise rows. **#[ignore]** for the same reason
//!     as S1.
//!   * **S4** (lock-in) — `matvec_sparse_perm` arithmetic. Hand-picked
//!     inputs make this byte-equivalent today.
//!   * **S5** (lock-in) — full tile loop / `jackpot[16]`.
//!   * **S6** (lock-in) — `compute_jackpot_hash`.
//!   * **S7** (D1) — commitment-hash chain. **#[ignore]**: our chain uses
//!     `derive_key` with context strings.
//!   * **S8** (D2) — matrix-commitment chunk-Merkle root. **#[ignore]**:
//!     our commitment is a per-row Merkle.
//!   * **S9** (lock-in) — shape-aware difficulty target.
//!
//! When a divergence is closed in our crate, remove the corresponding
//! `#[ignore]` and the test flips to an enforced byte-equality assertion.

use ai_pow::commit::{matrix_commitment, pad_to_chunk_boundary, padded_chunk_len};
use ai_pow::fiat_shamir::{commitment_key, noise_seed_a, noise_seed_b};
use ai_pow::matmul::{compute_tile_from_slices, BlockNoise, TileState};
use ai_pow::params::MatmulParams;
use ai_pow::prng;
use ai_pow::tile_hash::{difficulty_target, hash_le_target};

#[path = "fixtures/pearl.rs"]
mod fix;

// ============================================================================
// S0 — protocol constants we share with Pearl (lock-in)
// ============================================================================

#[test]
fn s0_protocol_constants_match_pearl() {
    // Tile state geometry, rotation amount, BLAKE3 digest size, chunk size,
    // and noise sign-translation are protocol-level constants both sides
    // must agree on.
    assert_eq!(TileState::zero().0.len(), fix::PEARL_JACKPOT_SIZE);
    assert_eq!(fix::PEARL_LROT_PER_TILE, 13);
    assert_eq!(fix::PEARL_BLAKE3_DIGEST_SIZE, 32);
    assert_eq!(fix::PEARL_CHUNK_LEN, 1024);
    assert_eq!(fix::PEARL_RANGE_MASK, 0x3F);
    assert_eq!(fix::PEARL_ZERO_POINT_TRANSLATION, 32);

    // The `A_tensor` / `B_tensor` seed labels — useful to confirm we know
    // exactly what bytes Pearl's PRNG keys are mixed with.
    assert_eq!(&fix::PEARL_SEED_LABEL_A[..8], b"A_tensor");
    assert_eq!(&fix::PEARL_SEED_LABEL_A[8..], &[0u8; 24]);
    assert_eq!(&fix::PEARL_SEED_LABEL_B[..8], b"B_tensor");
    assert_eq!(&fix::PEARL_SEED_LABEL_B[8..], &[0u8; 24]);
}

// ============================================================================
// S1 (D3) — `get_random_hash` byte stream
// ============================================================================
//
// Pearl PRNG = BLAKE3-keyed(message=[index_LE @ prepend_index*4 ..; seed @
// 32..64], key=key). Our PRNG = derive_key(context).update(seed).update(idx).
// Different byte streams => flagged divergence (D3 in the fixture table).
// The fixtures are still useful even while divergent: once D3 is closed by
// switching our PRNG, removing `#[ignore]` will lock in byte-equivalence.

#[test]
fn s1_prng_byte_stream_matches_pearl() {
    // S1 (D3 closed): our `prng::pearl_random_hash` must match Pearl's
    // `get_random_hash` byte-for-byte on every (index, seed, key, pi).
    let cases: &[(usize, usize, &[u8; 32], &[u8; 32])] = &[
        (0, 0, &fix::PEARL_SEED_LABEL_A, &fix::PEARL_PRNG_A_PI0_I00),
        (1, 0, &fix::PEARL_SEED_LABEL_A, &fix::PEARL_PRNG_A_PI0_I01),
        (7, 0, &fix::PEARL_SEED_LABEL_A, &fix::PEARL_PRNG_A_PI0_I07),
        (31, 0, &fix::PEARL_SEED_LABEL_A, &fix::PEARL_PRNG_A_PI0_I31),
        (0, 1, &fix::PEARL_SEED_LABEL_A, &fix::PEARL_PRNG_A_PI1_I00),
        (1, 1, &fix::PEARL_SEED_LABEL_A, &fix::PEARL_PRNG_A_PI1_I01),
        (7, 1, &fix::PEARL_SEED_LABEL_A, &fix::PEARL_PRNG_A_PI1_I07),
        (31, 1, &fix::PEARL_SEED_LABEL_A, &fix::PEARL_PRNG_A_PI1_I31),
        (0, 0, &fix::PEARL_SEED_LABEL_B, &fix::PEARL_PRNG_B_PI0_I00),
        (1, 0, &fix::PEARL_SEED_LABEL_B, &fix::PEARL_PRNG_B_PI0_I01),
        (7, 0, &fix::PEARL_SEED_LABEL_B, &fix::PEARL_PRNG_B_PI0_I07),
        (31, 0, &fix::PEARL_SEED_LABEL_B, &fix::PEARL_PRNG_B_PI0_I31),
        (0, 1, &fix::PEARL_SEED_LABEL_B, &fix::PEARL_PRNG_B_PI1_I00),
        (1, 1, &fix::PEARL_SEED_LABEL_B, &fix::PEARL_PRNG_B_PI1_I01),
        (7, 1, &fix::PEARL_SEED_LABEL_B, &fix::PEARL_PRNG_B_PI1_I07),
        (31, 1, &fix::PEARL_SEED_LABEL_B, &fix::PEARL_PRNG_B_PI1_I31),
    ];
    for (idx, pi, seed, expected) in cases {
        let ours = prng::pearl_random_hash(*idx, seed, &fix::PEARL_PRNG_KEY, *pi);
        assert_eq!(&ours, *expected, "idx={idx} pi={pi}");
    }

    // Also verify our `SEED_LABEL_A`/`SEED_LABEL_B` constants byte-match
    // Pearl's: both implementations must agree on the seed labels.
    assert_eq!(prng::SEED_LABEL_A, fix::PEARL_SEED_LABEL_A);
    assert_eq!(prng::SEED_LABEL_B, fix::PEARL_SEED_LABEL_B);
}

// ============================================================================
// S2 (D4) — permutation pairs
// ============================================================================

#[test]
fn s2_permutation_pairs_match_pearl() {
    // S2 (D4 closed): `e_r_col_positions` and `f_l_row_positions` use
    // Pearl's deterministic XOR scheme keyed on (SEED_LABEL_*, noise_seed).
    let cases: &[(u32, &[(u32, u32)])] = &[
        (32, fix::PEARL_PERM_K64_R32_LABEL_A),
        (64, fix::PEARL_PERM_K64_R64_LABEL_A),
        (128, fix::PEARL_PERM_K64_R128_LABEL_A),
    ];
    for (rank, expected) in cases {
        for (l, &(pp, pm)) in expected.iter().enumerate() {
            let (our_pp, our_pm) = prng::e_r_col_positions(&fix::PEARL_PERM_KEY, l as u32, *rank);
            assert_eq!((our_pp, our_pm), (pp, pm), "rank={rank} l={l}");
        }
    }
}

// ============================================================================
// S3 (D3) — uniform-noise matrices
// ============================================================================

#[test]
fn s3_uniform_noise_matches_pearl() {
    // S3 (D3 closed): `expand_e_l_row` / `expand_f_r_col` produce Pearl's
    // `generate_uniform_random_matrix` byte stream.
    let m = fix::FIX3_M;
    let n = fix::FIX3_N;
    let r = fix::FIX3_R as u32;
    let mut e_l = vec![0i8; m * r as usize];
    for i in 0..m {
        let off = i * r as usize;
        prng::expand_e_l_row(
            &fix::FIX3_NOISE_KEY,
            i as u32,
            r,
            &mut e_l[off..off + r as usize],
        );
    }
    assert_eq!(e_l.as_slice(), fix::FIX3_E_L);
    let mut f_r = vec![0i8; n * r as usize];
    for j in 0..n {
        let off = j * r as usize;
        prng::expand_f_r_col(
            &fix::FIX3_NOISE_KEY,
            j as u32,
            r,
            &mut f_r[off..off + r as usize],
        );
    }
    assert_eq!(f_r.as_slice(), fix::FIX3_F_R);
}

// ============================================================================
// S4 (lock-in) — `matvec_sparse_perm` reconstruction
// ============================================================================

#[test]
fn s4_noise_reconstruction_matches_pearl() {
    // Build a `BlockNoise` directly from the hand-picked factors. Our
    // `e_row_into` / `f_col_into` must produce the same noise rows/cols
    // Pearl produces via `matvec_sparse_perm`.
    let params = MatmulParams {
        m: fix::FIX4_M as u32,
        k: fix::FIX4_K as u32,
        n: fix::FIX4_N as u32,
        noise_rank: fix::FIX4_R as u32,
        tile: fix::FIX4_K as u32, // doesn't affect this test
        spot_checks: 1,
        difficulty_bits: 0,
    };
    let e_r_pos: Vec<(u32, u32)> = fix::FIX4_E_R_T_POS.iter().copied().collect();
    let f_l_pos: Vec<(u32, u32)> = fix::FIX4_F_L_POS.iter().copied().collect();
    let noise = BlockNoise {
        m: params.m,
        k: params.k,
        n: params.n,
        r: params.noise_rank,
        e_l: fix::FIX4_E_L.to_vec(),
        e_r_pos,
        f_r: fix::FIX4_F_R.to_vec(),
        f_l_pos,
    };
    let k = params.k as usize;
    let mut e_row = vec![0i8; k];
    for i in 0..params.m {
        noise.e_row_into(i, &mut e_row);
        let expected = &fix::FIX4_NOISE_A[(i as usize) * k..(i as usize + 1) * k];
        assert_eq!(e_row.as_slice(), expected, "e_row({i})");
    }
    let mut f_col = vec![0i8; k];
    for j in 0..params.n {
        noise.f_col_into(j, &mut f_col);
        let expected = &fix::FIX4_NOISE_B_T[(j as usize) * k..(j as usize + 1) * k];
        assert_eq!(f_col.as_slice(), expected, "f_col({j})");
    }
}

// ============================================================================
// S5 (lock-in) — full tile loop / `jackpot[16]`
// ============================================================================

#[test]
fn s5_tile_loop_jackpot_matches_pearl() {
    // Our `compute_tile_from_slices` on the captured (A', B') tile strips
    // must produce the same 16 × u32 state Pearl's mining loop produces.
    let params = MatmulParams {
        m: fix::FIX5_M as u32,
        k: fix::FIX5_K as u32,
        n: fix::FIX5_N as u32,
        noise_rank: fix::FIX5_R as u32,
        tile: fix::FIX5_T as u32,
        spot_checks: 1,
        difficulty_bits: 0,
    };
    let state = compute_tile_from_slices(fix::FIX5_A_PRIME_ROWS, fix::FIX5_B_PRIME_COLS, &params);
    let our_u32: [u32; 16] = std::array::from_fn(|i| state.0[i] as u32);
    assert_eq!(our_u32, fix::FIX5_JACKPOT);
}

// ============================================================================
// S6 (lock-in) — `compute_jackpot_hash`
// ============================================================================

#[test]
fn s6_jackpot_hash_matches_pearl() {
    let state = TileState(std::array::from_fn(|i| fix::FIX5_JACKPOT[i] as i32));
    let h = state.keyed_hash(&fix::FIX6_KEY);
    assert_eq!(h, fix::FIX6_EXPECTED_HASH);
}

// ============================================================================
// S7 (D1) — commitment hash chain
// ============================================================================

#[test]
fn s7_commitment_chain_matches_pearl() {
    // S7 (D1 closed): our seed-derivation chain follows Pearl's unkeyed
    // BLAKE3 formulas. The fixture supplies (job_key, hash_a, hash_b) so
    // we don't depend on D2 (matrix commitment) being closed yet.
    let s_b = noise_seed_b(&fix::FIX7_JOB_KEY, &fix::FIX7_HASH_B);
    assert_eq!(s_b, fix::FIX7_S_B);
    let s_a = noise_seed_a(&s_b, &fix::FIX7_HASH_A);
    assert_eq!(s_a, fix::FIX7_S_A);

    // Also confirm `commitment_key` matches Pearl's `compute_job_key` shape:
    // feeding `(block_commitment, params_tag)` should produce a 32-byte
    // BLAKE3 of their flat concatenation. We can't byte-match the fixture's
    // FIX7_JOB_KEY (which was hand-picked; the flat-concat preimage isn't
    // recoverable), but we can verify the function is the same algorithm
    // by feeding two known parts and checking it equals raw BLAKE3.
    let block = b"some-block-commitment";
    let tag = [0xA5u8; 32];
    let ours = commitment_key(block, &tag);
    let mut hasher = blake3::Hasher::new();
    hasher.update(block);
    hasher.update(&tag);
    let expected = *hasher.finalize().as_bytes();
    assert_eq!(ours, expected);
}

// ============================================================================
// S8 (D2) — matrix-commitment Merkle root
// ============================================================================

#[test]
fn s8_matrix_merkle_root_matches_pearl() {
    // S8 (D2 closed): our `matrix_commitment` is Pearl's chunk-Merkle root,
    // computed as keyed BLAKE3 over the zero-padded matrix byte stream.

    // Single-chunk case: 512 bytes pad up to one full 1024-byte chunk.
    assert_eq!(fix::FIX8_RAW_A_LEN, 512);
    assert_eq!(fix::FIX8_PADDED_A_LEN, 1024);
    assert_eq!(
        padded_chunk_len(fix::FIX8_RAW_A_LEN),
        fix::FIX8_PADDED_A_LEN
    );
    let our_a = matrix_commitment(fix::FIX7_A_BYTES_ROW_MAJOR, &fix::FIX7_JOB_KEY);
    assert_eq!(our_a, fix::FIX8_MERKLE_ROOT_A);
    let our_b = matrix_commitment(fix::FIX7_B_T_BYTES_ROW_MAJOR, &fix::FIX7_JOB_KEY);
    assert_eq!(our_b, fix::FIX8_MERKLE_ROOT_B);

    // Multi-chunk case: 3000 bytes pad up to 3 × 1024 bytes, exercising
    // BLAKE3's internal chunk-tree construction.
    assert_eq!(fix::FIX8_BIG_RAW_LEN, 3000);
    assert_eq!(fix::FIX8_BIG_PADDED_LEN, 3072);
    assert_eq!(
        padded_chunk_len(fix::FIX8_BIG_RAW_LEN),
        fix::FIX8_BIG_PADDED_LEN
    );
    let our_big = matrix_commitment(fix::FIX8_BIG_RAW, &fix::FIX7_JOB_KEY);
    assert_eq!(our_big, fix::FIX8_BIG_MERKLE_ROOT);

    // Sanity: pad_to_chunk_boundary returns the right length.
    let padded = pad_to_chunk_boundary(fix::FIX8_BIG_RAW);
    assert_eq!(padded.len(), fix::FIX8_BIG_PADDED_LEN);
    assert_eq!(&padded[..fix::FIX8_BIG_RAW_LEN], fix::FIX8_BIG_RAW);
    assert!(padded[fix::FIX8_BIG_RAW_LEN..].iter().all(|&b| b == 0));
}

// ============================================================================
// S9 (lock-in) — shape-aware difficulty target
// ============================================================================

#[test]
fn s9_difficulty_target_le_bytes_match() {
    for &(b, r, t, expected) in fix::FIX9_TARGETS {
        let params = MatmulParams {
            m: t,
            k: 64.max(r * 2), // doesn't affect target, just needs to validate
            n: t,
            noise_rank: r,
            tile: t,
            spot_checks: 1,
            difficulty_bits: b,
        };
        let target = difficulty_target(&params);
        assert_eq!(
            target, expected,
            "difficulty_target mismatch at b={b} r={r} t={t}\n  ours:     {target:02x?}\n  expected: {expected:02x?}",
        );
    }
}

#[test]
fn s9_le_compare_matches_pearl_u256_semantics() {
    // Compare every fixture target against itself, a slightly smaller
    // hash, and a slightly larger one. The semantics must match
    // U256::from_little_endian(hash) <= U256::from_little_endian(target).
    for &(b, r, t, target) in fix::FIX9_TARGETS {
        if target == [0u8; 32] {
            // No hash can be <= 0 except an all-zero one.
            assert!(hash_le_target(&[0u8; 32], &target));
            let mut h = [0u8; 32];
            h[0] = 1;
            assert!(!hash_le_target(&h, &target));
            continue;
        }
        // hash == target should be true.
        assert!(
            hash_le_target(&target, &target),
            "b={b} r={r} t={t}: target should equal itself"
        );
        // smaller hash → true.
        let mut smaller = target;
        let mut idx = 0;
        while idx < 32 && smaller[idx] == 0 {
            idx += 1;
        }
        if idx < 32 {
            smaller[idx] -= 1;
            assert!(
                hash_le_target(&smaller, &target),
                "b={b} r={r} t={t}: smaller hash should pass"
            );
        }
        // Larger hash → false (unless target is already max).
        if target != [0xFFu8; 32] {
            let mut larger = target;
            let mut idx = 0;
            while idx < 32 && larger[idx] == 0xFF {
                larger[idx] = 0;
                idx += 1;
            }
            if idx < 32 {
                larger[idx] += 1;
                assert!(
                    !hash_le_target(&larger, &target),
                    "b={b} r={r} t={t}: larger hash should fail"
                );
            }
        }
    }
}
