//! End-to-end tests at LLM-shaped, non-power-of-two-tile-count parameters.
//!
//! Production LLM dimensions are huge (Gemma 4 31B FFN tile count at
//! the current tile=8 preset is 512 × 2688 = 1,376,256; padded depth
//! 21) so we exercise small rectangular shapes that share the
//! relevant structural properties.

use ai_pow::params::MatmulParams;
use ai_pow::prover::{mine, ProverOptions};
use ai_pow::synth::synth_matrices;
use ai_pow::verifier::verify;

fn rect_a() -> MatmulParams {
    // (8 row tiles) * (12 col tiles) = 96 tiles, padded to 128.
    MatmulParams {
        m: 64,
        k: 80,
        n: 96,
        noise_rank: 4,
        tile: 8,
        spot_checks: 4,
        difficulty_bits: 0,
    }
}

fn rect_b() -> MatmulParams {
    MatmulParams {
        m: 96,
        k: 80,
        n: 80,
        noise_rank: 4,
        tile: 8,
        spot_checks: 4,
        difficulty_bits: 0,
    }
}

#[test]
fn rectangle_round_trip_a() {
    let params = rect_a();
    let (a, b) = synth_matrices(b"ab-seed", &params);
    let proof = mine(b"hdr", b"nce", &a, &b, &params, ProverOptions::default())
        .unwrap()
        .unwrap();
    verify(b"hdr", b"nce", &params, &proof).unwrap();
}

#[test]
fn rectangle_round_trip_b() {
    let params = rect_b();
    let (a, b) = synth_matrices(b"ab-seed", &params);
    let proof = mine(b"hdr", b"nce", &a, &b, &params, ProverOptions::default())
        .unwrap()
        .unwrap();
    verify(b"hdr", b"nce", &params, &proof).unwrap();
}

#[test]
fn swapping_m_and_n_changes_proof() {
    let pa = rect_a();
    let pb = MatmulParams {
        m: pa.n,
        n: pa.m,
        ..pa
    };
    let (a_a, b_a) = synth_matrices(b"ab-a", &pa);
    let (a_b, b_b) = synth_matrices(b"ab-b", &pb);
    let pa_proof = mine(b"hdr", b"nce", &a_a, &b_a, &pa, ProverOptions::default())
        .unwrap()
        .unwrap();
    let pb_proof = mine(b"hdr", b"nce", &a_b, &b_b, &pb, ProverOptions::default())
        .unwrap()
        .unwrap();
    assert_ne!(pa_proof, pb_proof);
    let cross = verify(b"hdr", b"nce", &pb, &pa_proof);
    assert!(cross.is_err());
}

#[test]
fn merkle_path_length_is_padded_depth() {
    let params = rect_a();
    let (a, b) = synth_matrices(b"ab-seed", &params);
    let proof = mine(b"hdr", b"nce", &a, &b, &params, ProverOptions::default())
        .unwrap()
        .unwrap();
    let padded_depth = params.num_tiles().next_power_of_two().trailing_zeros() as usize;
    assert_eq!(proof.found.m_path.len(), padded_depth);
    for opening in &proof.spot {
        assert_eq!(opening.m_path.len(), padded_depth);
    }
    // Row paths reach h_a (over m leaves) and col paths reach h_b (over n leaves).
    let m_depth = (params.m as u64).next_power_of_two().trailing_zeros() as usize;
    let n_depth = (params.n as u64).next_power_of_two().trailing_zeros() as usize;
    for p in &proof.found.a_row_paths {
        assert_eq!(p.len(), m_depth);
    }
    for p in &proof.found.b_col_paths {
        assert_eq!(p.len(), n_depth);
    }
}

#[test]
fn llm_profiles_validate() {
    MatmulParams::GEMMA_4_31B_FFN.validate().unwrap();
    MatmulParams::QWEN_3_6_27B_FFN.validate().unwrap();
    MatmulParams::llm_ffn(5376, 21504, 4096).validate().unwrap();
    MatmulParams::llm_ffn(5120, 17408, 4096).validate().unwrap();

    // GEMMA at tile=8: row_tiles=512, col_tiles=2688
    // ⇒ num_tiles=1,376,256, padded to 2^21=2,097,152.
    assert_eq!(
        MatmulParams::GEMMA_4_31B_FFN
            .num_tiles_padded()
            .trailing_zeros(),
        21,
    );
}
