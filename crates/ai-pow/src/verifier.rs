//! Verifier: replication-style spot-check of an AI-PoW matmul proof.
//!
//! For each opened tile the verifier:
//!   1. Range-checks the supplied row/column strips of `A` and `B`.
//!   2. Recomputes each row/column leaf via `a_row_leaf_hash` /
//!      `b_col_leaf_hash` and confirms its Merkle path recovers the plain
//!      proof's diagnostic `h_a` / `h_b` opening roots. These roots are not
//!      production recursive commitments and are not used for nonce/noise
//!      derivation.
//!   3. Reconstructs `A' = A + E` and `B' = B + F` rows / columns from the
//!      supplied strips and the re-derived noise factors.
//!   4. Runs the iterative tile loop to recompute `M_{i,j}` (Pearl §4.5).
//!   5. Keyed-hashes `M` with `pow_key = derive_key("pow-key", s_A ‖ nonce)`
//!      and checks the path against `comm_m`. For the found tile, also
//!      checks the keyed hash is `<= 2^(256-b) · r · t^2`.

use thiserror::Error;

use crate::commit::{a_row_leaf_hash, b_col_leaf_hash, merkle_recover_root, MerkleError};
use crate::fiat_shamir::{
    attempt_tile_index, block_state, canonical_noise_seeds_from_matrix_commitments,
    challenge_indices, challenge_seed, commitment_key, pow_key_for_nonce,
};
use crate::matmul::compute_tile_from_slices;
use crate::params::{MatmulParams, ParamError};
use crate::prng;
use crate::proof::{MatmulProof, TileOpening};
use crate::prover::params_tag;
use crate::tile_hash::{difficulty_target, hash_le_target};

const INPUT_RANGE_MAX: i8 = 64;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum VerifyError {
    #[error("invalid params: {0}")]
    Params(#[from] ParamError),
    #[error("merkle: {0}")]
    Merkle(#[from] MerkleError),
    #[error("params tag in proof does not match expected")]
    ParamsTagMismatch,
    #[error("found tile coordinates out of range")]
    FoundOutOfRange,
    #[error("found tile index does not match verifier-derived attempt tile")]
    FoundIndexMismatch,
    #[error("found tile hardness check failed")]
    FoundAboveTarget,
    #[error("found tile Merkle path does not recover comm_m")]
    FoundMerkleMismatch,
    #[error("spot check count does not match params")]
    SpotCountMismatch,
    #[error("spot check tile index does not match Fiat-Shamir derivation")]
    SpotIndexMismatch,
    #[error("spot check tile coordinates out of range")]
    SpotOutOfRange,
    #[error("spot check Merkle path does not recover comm_m")]
    SpotMerkleMismatch,
    #[error("opening A-row strip length wrong (expected {expected}, got {actual})")]
    BadAStripLen { expected: usize, actual: usize },
    #[error("opening B-col strip length wrong (expected {expected}, got {actual})")]
    BadBStripLen { expected: usize, actual: usize },
    #[error("opening has wrong number of A-row paths (expected {expected}, got {actual})")]
    BadAPathCount { expected: usize, actual: usize },
    #[error("opening has wrong number of B-col paths (expected {expected}, got {actual})")]
    BadBPathCount { expected: usize, actual: usize },
    #[error("A-row strip value out of range [-64, 64]")]
    AStripOutOfRange,
    #[error("B-col strip value out of range [-64, 64]")]
    BStripOutOfRange,
    #[error("A-row strip does not authenticate against h_a")]
    ARowMerkleMismatch,
    #[error("B-col strip does not authenticate against h_b")]
    BColMerkleMismatch,
}

/// Non-production helper: verify a `MatmulProof` for the given block context
/// using `difficulty_target(params)`.
///
/// Consensus callers with an externally supplied chain target must use
/// [`verify_at_target`] instead. This wrapper is
/// retained under `ai_pow::verifier::verify` for tests and local tools whose
/// target is intentionally derived from `params.difficulty_bits`; it is not
/// re-exported from the crate root.
pub fn verify(
    block_commitment: &[u8],
    nonce: &[u8],
    params: &MatmulParams,
    proof: &MatmulProof,
) -> Result<(), VerifyError> {
    let target = difficulty_target(params);
    verify_at_target(block_commitment, nonce, params, &target, proof)
}

/// Verify a plain `MatmulProof` against an explicit 256-bit little-endian
/// target.
///
/// This is a low-level verifier primitive. It structurally validates `params`,
/// but it does not enforce the production consensus envelope; plain-proof
/// callers that need those extra checks must use [`verify_prod_at_target`].
/// Nockchain block acceptance must verify a recursive certificate artifact
/// instead of a plain proof. The current production candidate is the compact
/// final-layer batch-STARK certificate. The large batch-STARK checkpoint is
/// retained only for regression; native terminal compression was removed.
pub fn verify_at_target(
    block_commitment: &[u8],
    nonce: &[u8],
    params: &MatmulParams,
    target: &[u8; 32],
    proof: &MatmulProof,
) -> Result<(), VerifyError> {
    params.validate()?;
    let tag = params_tag(params);
    if tag != proof.params_tag {
        return Err(VerifyError::ParamsTagMismatch);
    }
    let state = block_state(block_commitment, nonce);
    let chal = challenge_seed(&state, &proof.comm_m, &tag);
    let num_tiles = params.num_tiles();
    let kappa = commitment_key(&state, &tag);
    let (s_a, s_b) =
        canonical_noise_seeds_from_matrix_commitments(&kappa, &proof.h_a_chunk, &proof.h_b_chunk);
    let expected_found = attempt_tile_index(&state, &tag, &s_a, num_tiles);

    if (proof.spot.len() as u32) != params.spot_checks {
        return Err(VerifyError::SpotCountMismatch);
    }

    precheck_opening(&proof.found, params, OpeningRole::Found)?;
    if params.tile_index(proof.found.i, proof.found.j) != expected_found {
        return Err(VerifyError::FoundIndexMismatch);
    }
    for opening in &proof.spot {
        precheck_opening(opening, params, OpeningRole::Spot)?;
    }

    let pow_key = pow_key_for_nonce(&s_a, nonce);
    let noise = VerifierNoise::new(s_a, s_b, params);

    let leaf_found = verify_opening(
        &proof.found,
        proof,
        params,
        &noise,
        &kappa,
        &pow_key,
        OpeningRole::Found,
    )?;
    if !hash_le_target(&leaf_found, target) {
        return Err(VerifyError::FoundAboveTarget);
    }

    let expected_indices = challenge_indices(&chal, params.spot_checks, num_tiles);
    for (k, opening) in proof.spot.iter().enumerate() {
        let claimed_idx = params.tile_index(opening.i, opening.j);
        if claimed_idx != expected_indices[k] {
            return Err(VerifyError::SpotIndexMismatch);
        }
        verify_opening(
            opening,
            proof,
            params,
            &noise,
            &kappa,
            &pow_key,
            OpeningRole::Spot,
        )?;
    }

    Ok(())
}

/// Verify a plain `MatmulProof` against an explicit target after enforcing the
/// production parameter envelope.
///
/// This is not a canonical Nockchain block verifier. It exists for diagnostics
/// and pre-ZKP checks before recursive certificate generation.
pub fn verify_prod_at_target(
    block_commitment: &[u8],
    nonce: &[u8],
    params: &MatmulParams,
    target: &[u8; 32],
    proof: &MatmulProof,
) -> Result<(), VerifyError> {
    params.validate_prod_envelope()?;
    verify_at_target(block_commitment, nonce, params, target, proof)
}

#[derive(Copy, Clone)]
enum OpeningRole {
    Found,
    Spot,
}

struct VerifierNoise {
    s_a: [u8; 32],
    s_b: [u8; 32],
    r: u32,
    e_r_pos: Vec<(u32, u32)>,
    f_l_pos: Vec<(u32, u32)>,
}

impl VerifierNoise {
    fn new(s_a: [u8; 32], s_b: [u8; 32], params: &MatmulParams) -> Self {
        let e_r_pos = (0..params.k)
            .map(|l| prng::e_r_col_positions(&s_a, l, params.noise_rank))
            .collect();
        let f_l_pos = (0..params.k)
            .map(|l| prng::f_l_row_positions(&s_b, l, params.noise_rank))
            .collect();
        Self {
            s_a,
            s_b,
            r: params.noise_rank,
            e_r_pos,
            f_l_pos,
        }
    }

    fn e_row_into(&self, i: u32, out: &mut [i8]) {
        let mut e_l_row = vec![0i8; self.r as usize];
        prng::expand_e_l_row(&self.s_a, i, self.r, &mut e_l_row);
        for (l, slot) in out.iter_mut().enumerate() {
            let (pp, pm) = self.e_r_pos[l];
            *slot = e_l_row[pp as usize] - e_l_row[pm as usize];
        }
    }

    fn f_col_into(&self, j: u32, out: &mut [i8]) {
        let mut f_r_col = vec![0i8; self.r as usize];
        prng::expand_f_r_col(&self.s_b, j, self.r, &mut f_r_col);
        for (l, slot) in out.iter_mut().enumerate() {
            let (pp, pm) = self.f_l_pos[l];
            *slot = f_r_col[pp as usize] - f_r_col[pm as usize];
        }
    }
}

fn precheck_opening(
    opening: &TileOpening,
    params: &MatmulParams,
    role: OpeningRole,
) -> Result<(), VerifyError> {
    let t = params.tile as usize;
    let k = params.k as usize;

    if opening.i >= params.row_tiles() || opening.j >= params.col_tiles() {
        return Err(match role {
            OpeningRole::Found => VerifyError::FoundOutOfRange,
            OpeningRole::Spot => VerifyError::SpotOutOfRange,
        });
    }
    if opening.a_rows.len() != t * k {
        return Err(VerifyError::BadAStripLen {
            expected: t * k,
            actual: opening.a_rows.len(),
        });
    }
    if opening.b_cols.len() != t * k {
        return Err(VerifyError::BadBStripLen {
            expected: t * k,
            actual: opening.b_cols.len(),
        });
    }
    if opening.a_row_paths.len() != t {
        return Err(VerifyError::BadAPathCount {
            expected: t,
            actual: opening.a_row_paths.len(),
        });
    }
    if opening.b_col_paths.len() != t {
        return Err(VerifyError::BadBPathCount {
            expected: t,
            actual: opening.b_col_paths.len(),
        });
    }
    for &v in &opening.a_rows {
        if v < -INPUT_RANGE_MAX || v > INPUT_RANGE_MAX {
            return Err(VerifyError::AStripOutOfRange);
        }
    }
    for &v in &opening.b_cols {
        if v < -INPUT_RANGE_MAX || v > INPUT_RANGE_MAX {
            return Err(VerifyError::BStripOutOfRange);
        }
    }
    Ok(())
}

/// Validate one tile opening end-to-end. Returns the keyed-hash leaf so the
/// caller can apply the per-role hardness check (only meaningful for the
/// `Found` opening).
fn verify_opening(
    opening: &TileOpening,
    proof: &MatmulProof,
    params: &MatmulParams,
    noise: &VerifierNoise,
    kappa: &[u8; 32],
    pow_key: &[u8; 32],
    role: OpeningRole,
) -> Result<[u8; 32], VerifyError> {
    let t = params.tile as usize;
    let k = params.k as usize;
    let m = params.m as usize;
    let n = params.n as usize;
    let num_tiles = params.num_tiles() as usize;

    // Range checks on (i, j).
    if opening.i >= params.row_tiles() || opening.j >= params.col_tiles() {
        return Err(match role {
            OpeningRole::Found => VerifyError::FoundOutOfRange,
            OpeningRole::Spot => VerifyError::SpotOutOfRange,
        });
    }

    // Strip shape checks.
    if opening.a_rows.len() != t * k {
        return Err(VerifyError::BadAStripLen {
            expected: t * k,
            actual: opening.a_rows.len(),
        });
    }
    if opening.b_cols.len() != t * k {
        return Err(VerifyError::BadBStripLen {
            expected: t * k,
            actual: opening.b_cols.len(),
        });
    }
    if opening.a_row_paths.len() != t {
        return Err(VerifyError::BadAPathCount {
            expected: t,
            actual: opening.a_row_paths.len(),
        });
    }
    if opening.b_col_paths.len() != t {
        return Err(VerifyError::BadBPathCount {
            expected: t,
            actual: opening.b_col_paths.len(),
        });
    }

    // Strip value range checks (Pearl §4.1).
    for &v in &opening.a_rows {
        if v < -INPUT_RANGE_MAX || v > INPUT_RANGE_MAX {
            return Err(VerifyError::AStripOutOfRange);
        }
    }
    for &v in &opening.b_cols {
        if v < -INPUT_RANGE_MAX || v > INPUT_RANGE_MAX {
            return Err(VerifyError::BStripOutOfRange);
        }
    }

    // Authenticate each row strip against h_a.
    let row0 = (opening.i * params.tile) as usize;
    let col0 = (opening.j * params.tile) as usize;
    for di in 0..t {
        let row = &opening.a_rows[di * k..(di + 1) * k];
        let leaf = a_row_leaf_hash(row, kappa);
        let recovered = merkle_recover_root(&leaf, row0 + di, &opening.a_row_paths[di], m)?;
        if recovered != proof.h_a {
            return Err(VerifyError::ARowMerkleMismatch);
        }
    }
    for dj in 0..t {
        let col = &opening.b_cols[dj * k..(dj + 1) * k];
        let leaf = b_col_leaf_hash(col, kappa);
        let recovered = merkle_recover_root(&leaf, col0 + dj, &opening.b_col_paths[dj], n)?;
        if recovered != proof.h_b {
            return Err(VerifyError::BColMerkleMismatch);
        }
    }

    // Reconstruct A' rows and B' cols by adding the noise.
    let mut a_prime_rows = vec![0i8; t * k];
    let mut e_buf = vec![0i8; k];
    for di in 0..t {
        let ri = row0 + di;
        noise.e_row_into(ri as u32, &mut e_buf);
        let a_row = &opening.a_rows[di * k..(di + 1) * k];
        for l in 0..k {
            a_prime_rows[di * k + l] = (a_row[l] as i16 + e_buf[l] as i16) as i8;
        }
    }
    let mut b_prime_cols = vec![0i8; t * k];
    let mut f_buf = vec![0i8; k];
    for dj in 0..t {
        let cj = col0 + dj;
        noise.f_col_into(cj as u32, &mut f_buf);
        let b_col = &opening.b_cols[dj * k..(dj + 1) * k];
        for l in 0..k {
            b_prime_cols[dj * k + l] = (b_col[l] as i16 + f_buf[l] as i16) as i8;
        }
    }

    // Iterative Pearl §4.5 tile loop.
    let m_state = compute_tile_from_slices(&a_prime_rows, &b_prime_cols, params);
    let leaf = m_state.keyed_hash(pow_key);

    let tile_idx = params.tile_index(opening.i, opening.j) as usize;
    let recovered = merkle_recover_root(&leaf, tile_idx, &opening.m_path, num_tiles)?;
    if recovered != proof.comm_m {
        return Err(match role {
            OpeningRole::Found => VerifyError::FoundMerkleMismatch,
            OpeningRole::Spot => VerifyError::SpotMerkleMismatch,
        });
    }
    Ok(leaf)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::*;
    use crate::matmul::BLOCK_NOISE_EXPAND_CALLS;

    fn empty_opening() -> TileOpening {
        TileOpening {
            i: 0,
            j: 0,
            m_path: Vec::new(),
            a_rows: Vec::new(),
            b_cols: Vec::new(),
            a_row_paths: Vec::new(),
            b_col_paths: Vec::new(),
        }
    }

    fn empty_proof(params: &MatmulParams) -> MatmulProof {
        MatmulProof {
            comm_m: [0u8; 32],
            params_tag: params_tag(params),
            h_a: [1u8; 32],
            h_b: [2u8; 32],
            h_a_chunk: [0u8; 32],
            h_b_chunk: [0u8; 32],
            found: empty_opening(),
            spot: Vec::new(),
        }
    }

    #[test]
    fn production_verifiers_reject_structural_test_params() {
        let params = MatmulParams::TEST_SMALL;
        params.validate().unwrap();
        assert!(params.validate_prod_envelope().is_err());
        let proof = empty_proof(&params);
        let nonce = b"diagnostic-nonce";

        assert!(matches!(
            verify_prod_at_target(b"param01-puzzle", nonce, &params, &[0xFFu8; 32], &proof,),
            Err(VerifyError::Params(_))
        ));
    }

    #[test]
    fn rejects_spot_count_before_full_noise_expansion() {
        let params = MatmulParams {
            m: 1 << 20,
            k: 64,
            n: 8,
            noise_rank: 4,
            tile: 8,
            spot_checks: 1,
            difficulty_bits: 0,
        };
        params.validate().unwrap();
        let proof = empty_proof(&params);

        BLOCK_NOISE_EXPAND_CALLS.store(0, Ordering::Relaxed);
        let res = verify_at_target(
            b"dos01-block", b"dos01-nonce", &params, &[0xFFu8; 32], &proof,
        );
        assert_eq!(res, Err(VerifyError::SpotCountMismatch));
        assert_eq!(
            BLOCK_NOISE_EXPAND_CALLS.load(Ordering::Relaxed),
            0,
            "verifier must reject cheap shape/count errors before full noise expansion"
        );
    }
}
