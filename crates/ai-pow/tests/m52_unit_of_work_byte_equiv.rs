//! M52 step 6 — end-to-end byte-equivalence test for the matrix
//! commitments produced by ai-pow's plain side vs. ai-pow-zk's
//! `place_matrix_hash_a` / `place_matrix_hash_b` inside the SNARK.
//!
//! This is the "unit of work" cross-check: the bytes BLAKE3 hashes
//! in ai-pow's `BlockContext::h_a_chunk` MUST equal what the SNARK
//! binds as public input `HASH_A`. Without this, merge-mining
//! between Pearl and Nockchain via shared plain proofs cannot work.
//!
//! Gated by the `zk` feature.

#![cfg(feature = "zk")]

use ai_pow::params::MatmulParams;
use ai_pow::prover::BlockContext;
use ai_pow::synth::synth_matrices;
use ai_pow_zk::CompositeTrace;

fn u32_words_from_bytes(b: &[u8; 32]) -> [u32; 8] {
    core::array::from_fn(|i| {
        u32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]])
    })
}

#[test]
fn h_a_chunk_matches_snark_place_matrix_hash_a() {
    let params = MatmulParams::TEST_SMALL;
    let (a, b) = synth_matrices(b"m52-seed-a", &params);
    let block_commitment = b"m52-block-header";
    let nonce = b"m52-nonce-a";

    let ctx =
        BlockContext::build(block_commitment, nonce, &a, &b, &params).expect("BlockContext build");

    // Plain-side h_a_chunk computed via matrix_commitment.
    let plain_h_a = *ctx.h_a_chunk();

    // SNARK-side equivalent: place_matrix_hash_a on the same bytes + κ.
    let a_bytes: Vec<u8> = a.iter().map(|&v| v as u8).collect();
    let mut trace = CompositeTrace::baseline_min();
    let (_next_row, snark_root_cv) = trace.place_matrix_hash_a(0, &a_bytes, ctx.kappa());

    let expected_words = u32_words_from_bytes(&plain_h_a);
    assert_eq!(
        snark_root_cv, expected_words,
        "ai-pow h_a_chunk must byte-equal ai-pow-zk place_matrix_hash_a"
    );
}

#[test]
fn h_b_chunk_matches_snark_place_matrix_hash_b() {
    let params = MatmulParams::TEST_SMALL;
    let (a, b) = synth_matrices(b"m52-seed-b", &params);
    let block_commitment = b"m52-block-header-b";
    let nonce = b"m52-nonce-b";

    let ctx =
        BlockContext::build(block_commitment, nonce, &a, &b, &params).expect("BlockContext build");

    let plain_h_b = *ctx.h_b_chunk();

    let b_bytes: Vec<u8> = b.iter().map(|&v| v as u8).collect();
    let mut trace = CompositeTrace::baseline_min();
    let (_, snark_root_cv) = trace.place_matrix_hash_b(0, &b_bytes, ctx.kappa());

    let expected_words = u32_words_from_bytes(&plain_h_b);
    assert_eq!(
        snark_root_cv, expected_words,
        "ai-pow h_b_chunk must byte-equal ai-pow-zk place_matrix_hash_b"
    );
}

#[test]
fn distinct_matrices_have_distinct_chunk_commitments() {
    // Sanity: two different seeds yield two different h_a_chunks.
    let params = MatmulParams::TEST_SMALL;
    let bc = b"m52-distinct-block";
    let nonce = b"m52-distinct-nonce";
    let (a1, b1) = synth_matrices(b"seed-1", &params);
    let (a2, b2) = synth_matrices(b"seed-2", &params);
    let ctx1 = BlockContext::build(bc, nonce, &a1, &b1, &params).unwrap();
    let ctx2 = BlockContext::build(bc, nonce, &a2, &b2, &params).unwrap();
    assert_ne!(ctx1.h_a_chunk(), ctx2.h_a_chunk());
    assert_ne!(ctx1.h_b_chunk(), ctx2.h_b_chunk());
}
