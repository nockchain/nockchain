//! Tests for nonce-bound attempts: `mine_block` must produce the same proof
//! bytes as `mine` at the same nonce, and each nonce must rebuild the
//! commitment/noise/matmul-derived state rather than reusing work.

use ai_pow::params::MatmulParams;
use ai_pow::prover::{mine, mine_block, ProverOptions};
use ai_pow::synth::synth_matrices;
use ai_pow::verifier::verify;

#[test]
fn mine_and_mine_block_agree_at_same_nonce() {
    let params = MatmulParams::TEST_SMALL;
    let (a, b) = synth_matrices(b"ab-seed", &params);
    let single = mine(b"hdr", b"nce-1", &a, &b, &params, ProverOptions::default())
        .unwrap()
        .unwrap();
    let block_one = mine_block(
        b"hdr",
        std::iter::once(b"nce-1" as &[u8]),
        &a,
        &b,
        &params,
        ProverOptions::default(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        single, block_one,
        "mine and mine_block must be byte-equivalent"
    );
    verify(b"hdr", b"nce-1", &params, &block_one).unwrap();
}

#[test]
fn mine_block_returns_first_satisfying_nonce() {
    let params = MatmulParams::TEST_SMALL;
    let (a, b) = synth_matrices(b"ab-seed", &params);
    // With difficulty_bits = 0 every nonce produces a proof, so the first
    // nonce wins. mine_block should return the same proof as mine on it.
    let nonces: Vec<&[u8]> = vec![b"a", b"b", b"c"];
    let block_proof = mine_block(b"hdr", nonces, &a, &b, &params, ProverOptions::default())
        .unwrap()
        .unwrap();
    let single = mine(b"hdr", b"a", &a, &b, &params, ProverOptions::default())
        .unwrap()
        .unwrap();
    assert_eq!(block_proof, single);
}

#[test]
fn mine_block_returns_none_when_no_nonce_satisfies() {
    let mut params = MatmulParams::TEST_SMALL;
    params.difficulty_bits = 400; // impossible
    let (a, b) = synth_matrices(b"ab-seed", &params);
    let r = mine_block(
        b"hdr",
        [b"a" as &[u8], b"b", b"c"],
        &a,
        &b,
        &params,
        ProverOptions::default(),
    )
    .unwrap();
    assert!(r.is_none());
}

#[test]
fn mine_block_preserves_per_nonce_diversity() {
    let params = MatmulParams::TEST_SMALL;
    let (a, b) = synth_matrices(b"ab-seed", &params);
    let p_a = mine(b"hdr", b"a", &a, &b, &params, ProverOptions::default())
        .unwrap()
        .unwrap();
    let p_b = mine(b"hdr", b"b", &a, &b, &params, ProverOptions::default())
        .unwrap()
        .unwrap();
    assert_ne!(p_a, p_b);
    // The nonce is part of the Pearl attempt state, so commitments are
    // re-keyed before noise and matmul.
    assert_ne!(p_a.h_a, p_b.h_a);
    assert_ne!(p_a.h_b, p_b.h_b);
}

#[test]
fn attempt_commitments_are_block_scoped() {
    let params = MatmulParams::TEST_SMALL;
    let (a, b) = synth_matrices(b"ab-seed", &params);
    let p_a = mine(b"hdr-A", b"nce", &a, &b, &params, ProverOptions::default())
        .unwrap()
        .unwrap();
    let p_b = mine(b"hdr-B", b"nce", &a, &b, &params, ProverOptions::default())
        .unwrap()
        .unwrap();
    assert_ne!(p_a, p_b);
    // κ depends on the attempt state, including block_commitment, so H_A and
    // H_B differ between blocks even though A and B are identical.
    assert_ne!(p_a.h_a, p_b.h_a);
    assert_ne!(p_a.h_b, p_b.h_b);
}
