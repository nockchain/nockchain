//! Adversarial tests: every check the verifier performs must reject the
//! corresponding tampering.

use ai_pow::params::{MatmulParams, ParamError, SPOT_CHECKS_MAX};
use ai_pow::proof::{MatmulProof, TileOpening};
use ai_pow::prover::{mine, ProverOptions};
use ai_pow::synth::synth_matrices;
use ai_pow::verifier::{verify, VerifyError};

fn fresh_proof() -> (MatmulParams, &'static [u8], &'static [u8], MatmulProof) {
    let params = MatmulParams::TEST_SMALL;
    let block = b"block-header-bytes" as &[u8];
    let nonce = b"nonce-1" as &[u8];
    let (a, b) = synth_matrices(b"ab-seed", &params);
    let proof = mine(block, nonce, &a, &b, &params, ProverOptions::default())
        .unwrap()
        .unwrap();
    (params, block, nonce, proof)
}

#[test]
fn reject_tampered_comm_m() {
    let (params, block, nonce, mut proof) = fresh_proof();
    proof.comm_m[0] ^= 1;
    let r = verify(block, nonce, &params, &proof);
    // Tampering comm_m breaks every Merkle path; the first one checked fails.
    assert!(matches!(
        r,
        Err(VerifyError::FoundMerkleMismatch | VerifyError::SpotMerkleMismatch)
    ));
}

#[test]
fn reject_tampered_params_tag() {
    let (params, block, nonce, mut proof) = fresh_proof();
    proof.params_tag[0] ^= 1;
    assert_eq!(
        verify(block, nonce, &params, &proof),
        Err(VerifyError::ParamsTagMismatch)
    );
}

#[test]
fn reject_tampered_h_a() {
    // Tampering H_A changes s_A, which changes the noise, which changes the
    // M state. So tile-path Merkle paths recover the wrong leaf and fail.
    // (The row-strip paths also fail since their root no longer matches.)
    let (params, block, nonce, mut proof) = fresh_proof();
    proof.h_a[0] ^= 1;
    let r = verify(block, nonce, &params, &proof);
    assert!(
        matches!(
            r,
            Err(VerifyError::FoundIndexMismatch)
                | Err(VerifyError::ARowMerkleMismatch)
                | Err(VerifyError::FoundMerkleMismatch)
                | Err(VerifyError::SpotMerkleMismatch)
        ),
        "got {r:?}"
    );
}

#[test]
fn reject_tampered_h_b() {
    let (params, block, nonce, mut proof) = fresh_proof();
    proof.h_b[0] ^= 1;
    let r = verify(block, nonce, &params, &proof);
    assert!(
        matches!(
            r,
            Err(VerifyError::FoundIndexMismatch)
                | Err(VerifyError::BColMerkleMismatch)
                | Err(VerifyError::FoundMerkleMismatch)
                | Err(VerifyError::SpotMerkleMismatch)
        ),
        "got {r:?}"
    );
}

#[test]
fn reject_tampered_chunk_seed_commitments() {
    let (params, block, nonce, mut proof) = fresh_proof();
    assert_ne!(proof.h_a_chunk, [0u8; 32]);
    assert_ne!(proof.h_b_chunk, [0u8; 32]);

    proof.h_a_chunk = [9u8; 32];
    let r = verify(block, nonce, &params, &proof);
    assert!(
        matches!(
            r,
            Err(VerifyError::FoundIndexMismatch)
                | Err(VerifyError::FoundMerkleMismatch)
                | Err(VerifyError::SpotMerkleMismatch)
        ),
        "got {r:?}"
    );

    let (_, _, _, mut proof) = fresh_proof();
    proof.h_b_chunk = [10u8; 32];
    let r = verify(block, nonce, &params, &proof);
    assert!(
        matches!(
            r,
            Err(VerifyError::FoundIndexMismatch)
                | Err(VerifyError::FoundMerkleMismatch)
                | Err(VerifyError::SpotMerkleMismatch)
        ),
        "got {r:?}"
    );
}

#[test]
fn reject_tampered_found_m_path() {
    let (params, block, nonce, mut proof) = fresh_proof();
    proof.found.m_path[0][0] ^= 1;
    assert_eq!(
        verify(block, nonce, &params, &proof),
        Err(VerifyError::FoundMerkleMismatch)
    );
}

#[test]
fn reject_found_out_of_range() {
    let (params, block, nonce, mut proof) = fresh_proof();
    proof.found.i = params.row_tiles();
    assert_eq!(
        verify(block, nonce, &params, &proof),
        Err(VerifyError::FoundOutOfRange)
    );
}

#[test]
fn reject_found_above_target() {
    // Build a proof under tight difficulty so most tiles fail the hardness
    // check, then replace `found` with a tile that doesn't pass.
    let mut params = MatmulParams::TEST_SMALL;
    params.difficulty_bits = 240;
    let block = b"block";
    let nonce = b"nonce";
    let (a, b) = synth_matrices(b"ab-seed", &params);
    let Some(proof) = mine(block, nonce, &a, &b, &params, ProverOptions::default()).unwrap() else {
        return;
    };
    let Some(alt) = proof
        .spot
        .iter()
        .find(|s| (s.i, s.j) != (proof.found.i, proof.found.j))
        .cloned()
    else {
        return;
    };
    let mut bad = proof.clone();
    bad.found = alt;
    let r = verify(block, nonce, &params, &bad);
    assert!(
        matches!(
            r,
            Err(VerifyError::FoundIndexMismatch)
                | Err(VerifyError::FoundAboveTarget)
                | Err(VerifyError::FoundMerkleMismatch)
        ),
        "got {r:?}"
    );
}

#[test]
fn reject_in_range_found_tile_substitution_before_merkle_or_target() {
    let (params, block, nonce, mut proof) = fresh_proof();
    let claimed = params.tile_index(proof.found.i, proof.found.j);
    let other = (claimed + 1) % params.num_tiles();
    let (i, j) = params.tile_coords(other);
    proof.found.i = i;
    proof.found.j = j;

    assert_eq!(
        verify(block, nonce, &params, &proof),
        Err(VerifyError::FoundIndexMismatch)
    );
}

#[test]
fn reject_wrong_spot_count() {
    let (params, block, nonce, mut proof) = fresh_proof();
    proof.spot.pop();
    assert_eq!(
        verify(block, nonce, &params, &proof),
        Err(VerifyError::SpotCountMismatch)
    );
}

#[test]
fn reject_wrong_spot_indices() {
    let (params, block, nonce, mut proof) = fresh_proof();
    let opening = &mut proof.spot[0];
    let other_idx = (params.tile_index(opening.i, opening.j) + 1) % params.num_tiles();
    let (i2, j2) = params.tile_coords(other_idx);
    opening.i = i2;
    opening.j = j2;
    assert_eq!(
        verify(block, nonce, &params, &proof),
        Err(VerifyError::SpotIndexMismatch)
    );
}

#[test]
fn reject_tampered_spot_m_path() {
    let (params, block, nonce, mut proof) = fresh_proof();
    proof.spot[0].m_path[0][0] ^= 1;
    assert_eq!(
        verify(block, nonce, &params, &proof),
        Err(VerifyError::SpotMerkleMismatch)
    );
}

#[test]
fn reject_spot_out_of_range() {
    let (params, block, nonce, mut proof) = fresh_proof();
    proof.spot[0].i = params.row_tiles();
    assert_eq!(
        verify(block, nonce, &params, &proof),
        Err(VerifyError::SpotOutOfRange)
    );
}

#[test]
fn reject_extra_opening() {
    let (params, block, nonce, mut proof) = fresh_proof();
    proof.spot.push(TileOpening {
        i: 0,
        j: 0,
        m_path: vec![],
        a_rows: vec![],
        b_cols: vec![],
        a_row_paths: vec![],
        b_col_paths: vec![],
    });
    assert_eq!(
        verify(block, nonce, &params, &proof),
        Err(VerifyError::SpotCountMismatch)
    );
}

#[test]
fn reject_tampered_a_row_bytes() {
    let (params, block, nonce, mut proof) = fresh_proof();
    // Flipping a byte of the A strip changes its leaf hash, which fails
    // its Merkle path to h_a.
    proof.found.a_rows[0] ^= 1;
    let r = verify(block, nonce, &params, &proof);
    assert!(
        matches!(r, Err(VerifyError::ARowMerkleMismatch)),
        "got {r:?}"
    );
}

#[test]
fn reject_tampered_b_col_bytes() {
    let (params, block, nonce, mut proof) = fresh_proof();
    proof.found.b_cols[0] ^= 1;
    let r = verify(block, nonce, &params, &proof);
    assert!(
        matches!(r, Err(VerifyError::BColMerkleMismatch)),
        "got {r:?}"
    );
}

#[test]
fn reject_tampered_a_row_path() {
    let (params, block, nonce, mut proof) = fresh_proof();
    proof.found.a_row_paths[0][0][0] ^= 1;
    let r = verify(block, nonce, &params, &proof);
    assert!(
        matches!(r, Err(VerifyError::ARowMerkleMismatch)),
        "got {r:?}"
    );
}

#[test]
fn reject_tampered_b_col_path() {
    let (params, block, nonce, mut proof) = fresh_proof();
    proof.found.b_col_paths[0][0][0] ^= 1;
    let r = verify(block, nonce, &params, &proof);
    assert!(
        matches!(r, Err(VerifyError::BColMerkleMismatch)),
        "got {r:?}"
    );
}

#[test]
fn reject_a_row_out_of_range() {
    let (params, block, nonce, mut proof) = fresh_proof();
    proof.found.a_rows[0] = 100; // > 64
    let r = verify(block, nonce, &params, &proof);
    assert_eq!(r, Err(VerifyError::AStripOutOfRange));
}

#[test]
fn reject_b_col_out_of_range() {
    let (params, block, nonce, mut proof) = fresh_proof();
    proof.found.b_cols[0] = -100; // < -64
    let r = verify(block, nonce, &params, &proof);
    assert_eq!(r, Err(VerifyError::BStripOutOfRange));
}

#[test]
fn reject_wrong_a_strip_length() {
    let (params, block, nonce, mut proof) = fresh_proof();
    proof.found.a_rows.pop();
    let r = verify(block, nonce, &params, &proof);
    assert!(
        matches!(r, Err(VerifyError::BadAStripLen { .. })),
        "got {r:?}"
    );
}

/// H2 (DoS audit): `verifier::verify` MUST reject a `params` whose
/// `spot_checks` exceeds the hard DoS cap (`SPOT_CHECKS_MAX = 256`)
/// *at validate time*, before entering the per-opening loop. The
/// pre-fix verifier would have looped `spot_checks` times re-hashing
/// up-to-2-MiB strips — a CPU-time DoS unbounded by the proof format
/// (it's the *params* that drive the loop count, not the proof).
#[test]
fn reject_spot_checks_above_dos_cap() {
    let (mut params, block, nonce, proof) = fresh_proof();
    params.spot_checks = SPOT_CHECKS_MAX + 1;
    let r = verify(block, nonce, &params, &proof);
    assert_eq!(
        r,
        Err(VerifyError::Params(ParamError::SpotChecksAboveDosCap)),
        "verify must reject crafted-spot_checks params at validate, not after the loop"
    );
}
