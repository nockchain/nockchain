//! Prover: derive the single eligible jackpot tile for this nonce-bound
//! attempt and check whether its keyed BLAKE3 hash of the 512-bit Pearl
//! tile state `M_{i,j}` falls below the shape-aware target
//! `2^(256 - b) · r · t^2` (Pearl §4.5).
//!
//! The miner supplies the input matrices `A` and `B`. Each nonce attempt
//! commits to them via a Pearl §4.3 Alg. 2-shaped chain:
//!  1. `κ = derive_key("kappa", block_state(block, nonce) ‖ params_tag)`
//!  2. `h_a_chunk` / `h_b_chunk` are nonce-keyed matrix commitments bound by
//!     the recursive ZK proof as `HASH_A` / `HASH_B`.
//!  3. `s_B = derive_key("s_b", κ ‖ h_b_chunk)`
//!  4. `s_A = derive_key("s_a", s_B ‖ h_a_chunk)`
//!
//! Plain `MatmulProof` diagnostics still compute `h_a` / `h_b` for their
//! own row/column spot openings, but those roots are not production recursive
//! commitments and are not part of the nonce/noise seed surface.
//!
//! The nonce is part of the per-attempt `sigma` before `κ`, `H_A`, `H_B`,
//! `s_A`, `s_B`, low-rank noise, and all tile states are computed. Minimal
//! work reuse across nonces is intentional; cache-friendly reuse of one matmul
//! result across many nonce hashes or many miner-selected tile hashes would be
//! a PoW soundness bug.

use thiserror::Error;

use crate::commit::{a_row_leaf_hash, b_col_leaf_hash, merkle_path, merkle_root, MerkleError};
use crate::fiat_shamir::{
    attempt_tile_index, block_state, canonical_noise_seeds_from_matrix_commitments,
    challenge_indices, challenge_seed, commitment_key, pow_key_for_nonce,
};
use crate::matmul::{compute_tile, BlockNoise, Matrices, TileState};
use crate::params::{MatmulParams, ParamError};
use crate::proof::{MatmulProof, TileOpening};
use crate::tile_hash::{difficulty_target, hash_le_target};

/// Pearl §4.1 input range: `|A|, |B| <= 64`.
const INPUT_RANGE_MAX: i8 = 64;

#[derive(Debug, Clone, Copy)]
pub struct ProverOptions {
    /// Deprecated no-op retained for API compatibility. Production mining uses
    /// one verifier-derived jackpot tile per nonce-bound attempt, so there is
    /// no miner-selected tile set to scan for a "best" hash.
    pub seek_best: bool,
}

impl Default for ProverOptions {
    fn default() -> Self {
        Self { seek_best: false }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MineError {
    #[error("invalid params: {0}")]
    Params(#[from] ParamError),
    #[error("merkle: {0}")]
    Merkle(#[from] MerkleError),
    #[error("A has wrong length: expected m*k = {expected}, got {actual}")]
    InputAShape { expected: usize, actual: usize },
    #[error("B has wrong length: expected n*k = {expected}, got {actual}")]
    InputBShape { expected: usize, actual: usize },
    #[error("input entry out of range [-64, 64]: matrix={matrix}, index={index}, value={value}")]
    InputOutOfRange {
        matrix: &'static str,
        index: usize,
        value: i8,
    },
    #[error("BlockContext was built for a different block commitment or nonce")]
    ContextAttemptMismatch,
}

/// Compute the 32-byte tag binding a `MatmulParams` instance into the
/// transcript. Both prover and verifier compute and compare this.
pub fn params_tag(p: &MatmulParams) -> [u8; 32] {
    crate::fiat_shamir::transcript(
        "matmul-params-v3",
        &[
            &p.m.to_le_bytes(),
            &p.k.to_le_bytes(),
            &p.n.to_le_bytes(),
            &p.noise_rank.to_le_bytes(),
            &p.tile.to_le_bytes(),
            &p.spot_checks.to_le_bytes(),
            &p.difficulty_bits.to_le_bytes(),
        ],
    )
}

/// Per-attempt computation: commitments to `A` and `B`, derived seeds,
/// noise factors, perturbed matrices, all `M_{i,j}` tile states, and the
/// leaves of `H_A` / `H_B` for opening-path extraction.
///
/// This is a nonce-bound attempt handle, not a reusable mining cache. Public
/// getters below exist for diagnostics, tests, and ZK bridge consistency checks.
/// Production mining must call [`mine`], [`mine_block`], or
/// [`mine_with_context_at_target`] with a context built for the exact nonce
/// being attempted; scanning alternate nonces over this context's `s_A` or tile
/// states would recreate the cheap-rehash grinding bug.
pub struct BlockContext<'a> {
    pub(crate) block_commitment: Vec<u8>,
    pub(crate) nonce: Vec<u8>,
    pub(crate) attempt_state: Vec<u8>,
    pub(crate) a: &'a [i8],
    pub(crate) b: &'a [i8],
    pub(crate) params: MatmulParams,
    pub(crate) tag: [u8; 32],
    pub(crate) kappa: [u8; 32],
    pub(crate) h_a: [u8; 32],
    pub(crate) h_b: [u8; 32],
    /// M52 step 5: chunk-Merkle BLAKE3 keyed-hash of `pad(A)` —
    /// the commitment shape the `ai-pow-zk` SNARK binds to as
    /// public input `HASH_A`. Distinct from `h_a` (per-row
    /// Merkle, used for spot-check opening) — both coexist so
    /// the plain-side spot-check protocol AND the SNARK PI
    /// binding work without conflict.
    pub(crate) h_a_chunk: [u8; 32],
    /// M52 step 5: chunk-Merkle commitment for matrix B
    /// (column-major bytes), analogous to `h_a_chunk`.
    pub(crate) h_b_chunk: [u8; 32],
    pub(crate) s_a: [u8; 32],
    pub(crate) s_b: [u8; 32],
    pub(crate) h_a_leaves: Vec<[u8; 32]>,
    pub(crate) h_b_leaves: Vec<[u8; 32]>,
    pub(crate) m_states: Vec<TileState>,
}

impl<'a> BlockContext<'a> {
    pub fn block_commitment(&self) -> &[u8] {
        &self.block_commitment
    }

    pub fn nonce(&self) -> &[u8] {
        &self.nonce
    }

    pub fn attempt_state(&self) -> &[u8] {
        &self.attempt_state
    }

    pub fn a(&self) -> &[i8] {
        self.a
    }

    pub fn b(&self) -> &[i8] {
        self.b
    }

    pub fn params(&self) -> &MatmulParams {
        &self.params
    }

    pub fn params_tag(&self) -> &[u8; 32] {
        &self.tag
    }

    pub fn kappa(&self) -> &[u8; 32] {
        &self.kappa
    }

    pub fn h_a(&self) -> &[u8; 32] {
        &self.h_a
    }

    pub fn h_b(&self) -> &[u8; 32] {
        &self.h_b
    }

    pub fn h_a_chunk(&self) -> &[u8; 32] {
        &self.h_a_chunk
    }

    pub fn h_b_chunk(&self) -> &[u8; 32] {
        &self.h_b_chunk
    }

    /// Diagnostic accessor for the nonce-bound `s_A` seed.
    ///
    /// Do not use this with a different nonce to derive alternate
    /// [`pow_key_for_nonce`] values over cached tile states.
    pub fn s_a(&self) -> &[u8; 32] {
        &self.s_a
    }

    /// Derived jackpot hash key for this exact nonce-bound attempt.
    ///
    /// Prefer this over combining [`Self::s_a`] with an arbitrary nonce in
    /// diagnostics. Production mining and proof construction use the context's
    /// own nonce so one cached context cannot be presented as many attempts.
    pub fn pow_key(&self) -> [u8; 32] {
        pow_key_for_nonce(&self.s_a, &self.nonce)
    }

    /// Diagnostic accessor for the nonce-bound `s_B` seed.
    pub fn s_b(&self) -> &[u8; 32] {
        &self.s_b
    }

    /// Diagnostic accessor for nonce-bound matmul tile states.
    ///
    /// These states are valid only for this context's `nonce`; production
    /// attempts must rebuild them for every explicit attempt nonce or
    /// Pearl-compatible ticket.
    pub fn tile_states(&self) -> &[TileState] {
        &self.m_states
    }

    /// Diagnostic accessor for one nonce-bound matmul tile state.
    pub fn tile_state(&self, idx: usize) -> Option<&TileState> {
        self.m_states.get(idx)
    }

    /// Validate inputs and run the full per-block precomputation. Returns
    /// `Err` only for parameter or shape problems.
    pub fn build(
        block_commitment: &[u8],
        nonce: &[u8],
        a: &'a [i8],
        b: &'a [i8],
        params: &MatmulParams,
    ) -> Result<Self, MineError> {
        params.validate()?;
        let m = params.m as usize;
        let k = params.k as usize;
        let n = params.n as usize;
        if a.len() != m * k {
            return Err(MineError::InputAShape {
                expected: m * k,
                actual: a.len(),
            });
        }
        if b.len() != n * k {
            return Err(MineError::InputBShape {
                expected: n * k,
                actual: b.len(),
            });
        }
        for (idx, &v) in a.iter().enumerate() {
            if v < -INPUT_RANGE_MAX || v > INPUT_RANGE_MAX {
                return Err(MineError::InputOutOfRange {
                    matrix: "A",
                    index: idx,
                    value: v,
                });
            }
        }
        for (idx, &v) in b.iter().enumerate() {
            if v < -INPUT_RANGE_MAX || v > INPUT_RANGE_MAX {
                return Err(MineError::InputOutOfRange {
                    matrix: "B",
                    index: idx,
                    value: v,
                });
            }
        }

        let tag = params_tag(params);
        let attempt_state = block_state(block_commitment, nonce);
        let kappa = commitment_key(&attempt_state, &tag);

        // Row leaves for H_A.
        let mut h_a_leaves = Vec::with_capacity(m);
        for i in 0..params.m {
            let off = (i as usize) * k;
            h_a_leaves.push(a_row_leaf_hash(&a[off..off + k], &kappa));
        }
        let h_a = merkle_root(&h_a_leaves)?;

        // Column leaves for H_B (B is column-major: col j at j*k..(j+1)*k).
        let mut h_b_leaves = Vec::with_capacity(n);
        for j in 0..params.n {
            let off = (j as usize) * k;
            h_b_leaves.push(b_col_leaf_hash(&b[off..off + k], &kappa));
        }
        let h_b = merkle_root(&h_b_leaves)?;

        // M52 step 5: chunk-Merkle commitments for SNARK PI binding.
        // BLAKE3 keyed-hash of pad(A_row_major) and pad(B_col_major).
        // i8 and u8 share layout; reinterpret without copying.
        let a_bytes: &[u8] =
            unsafe { core::slice::from_raw_parts(a.as_ptr() as *const u8, a.len()) };
        let b_bytes: &[u8] =
            unsafe { core::slice::from_raw_parts(b.as_ptr() as *const u8, b.len()) };
        let h_a_chunk = crate::commit::matrix_commitment(a_bytes, &kappa);
        let h_b_chunk = crate::commit::matrix_commitment(b_bytes, &kappa);

        let (s_a, s_b) =
            canonical_noise_seeds_from_matrix_commitments(&kappa, &h_a_chunk, &h_b_chunk);

        let noise = BlockNoise::expand(&s_a, &s_b, params);
        let matrices = Matrices::build(a, b, &noise, params);

        // Pre-compute every tile's M state for this single nonce-bound attempt.
        // Reusing these states across nonce values would be a PoW soundness bug.
        let num_tiles = params.num_tiles() as usize;
        let mut m_states = Vec::with_capacity(num_tiles);
        for tile_i in 0..params.row_tiles() {
            for tile_j in 0..params.col_tiles() {
                m_states.push(compute_tile(&matrices, params, tile_i, tile_j));
            }
        }

        Ok(BlockContext {
            block_commitment: block_commitment.to_vec(),
            nonce: nonce.to_vec(),
            attempt_state,
            a,
            b,
            params: *params,
            tag,
            kappa,
            h_a,
            h_b,
            h_a_chunk,
            h_b_chunk,
            s_a,
            s_b,
            h_a_leaves,
            h_b_leaves,
            m_states,
        })
    }
}

fn ensure_context_attempt(
    ctx: &BlockContext<'_>,
    block_commitment: &[u8],
    nonce: &[u8],
) -> Result<(), MineError> {
    if ctx.block_commitment == block_commitment && ctx.nonce == nonce {
        Ok(())
    } else {
        Err(MineError::ContextAttemptMismatch)
    }
}

/// Run one nonce attempt with caller-supplied `A` and `B`. Returns
/// `Ok(Some(proof))` if a tile satisfies the hardness target, `Ok(None)`
/// if no tile does, or `Err` for parameter / shape / range problems.
pub fn mine(
    block_commitment: &[u8],
    nonce: &[u8],
    a: &[i8],
    b: &[i8],
    params: &MatmulParams,
    opts: ProverOptions,
) -> Result<Option<MatmulProof>, MineError> {
    let ctx = BlockContext::build(block_commitment, nonce, a, b, params)?;
    mine_with_context(&ctx, opts)
}

/// Run the prover repeatedly over a sequence of nonces. Every nonce builds a
/// fresh attempt context, including fresh commitments, noise, matmul, and tile
/// states. Returns the first proof found or `Ok(None)` if no nonce in the
/// iterator satisfies the target.
pub fn mine_block<I, N>(
    block_commitment: &[u8],
    nonces: I,
    a: &[i8],
    b: &[i8],
    params: &MatmulParams,
    opts: ProverOptions,
) -> Result<Option<MatmulProof>, MineError>
where
    I: IntoIterator<Item = N>,
    N: AsRef<[u8]>,
{
    for nonce in nonces {
        let ctx = BlockContext::build(block_commitment, nonce.as_ref(), a, b, params)?;
        if let Some(proof) = mine_with_context(&ctx, opts)? {
            return Ok(Some(proof));
        }
    }
    Ok(None)
}

/// External-target variant of the per-nonce mining attempt. The
/// chain's difficulty bound is passed explicitly rather than derived
/// from `params.difficulty_bits` — needed by `ai-pow-miner` where
/// the chain supplies an arbitrary 32-byte target (which Pearl-style
/// `difficulty_bits` cannot express precisely).
///
/// Returns `Ok(Some(proof))` if a tile clears `target`, `Ok(None)`
/// otherwise.
///
/// **The `#[cfg(feature = "zk")]` SNARK side-effect** that
/// [`mine`] / [`mine_block`] perform (calling
/// `zk_bridge::prove_and_verify_for_block`) is **NOT** invoked here:
/// that bridge re-derives the target from `params` per MED-3 and
/// would mismatch any caller-supplied `target` that doesn't equal
/// `difficulty_target(params)`. Callers wanting the SNARK should
/// drive it themselves on the returned `found` tile after ensuring
/// `params` agrees with `target`.
pub fn mine_with_context_at_target(
    ctx: &BlockContext<'_>,
    block_commitment: &[u8],
    nonce: &[u8],
    target: &[u8; 32],
    opts: ProverOptions,
) -> Result<Option<MatmulProof>, MineError> {
    ensure_context_attempt(ctx, block_commitment, nonce)?;
    Ok(mine_inner(ctx, target, opts)?.map(|(p, _)| p))
}

fn mine_with_context(
    ctx: &BlockContext<'_>,
    opts: ProverOptions,
) -> Result<Option<MatmulProof>, MineError> {
    let target = difficulty_target(&ctx.params);
    let result = mine_inner(ctx, &target, opts)?;

    // Pearl-analog ZK wrapping (preserved from the original
    // mine_with_context). The bridge re-derives target from params
    // per MED-3, so we only run it on the params-derived-target
    // path; `mine_with_context_at_target` deliberately skips it.
    #[cfg(feature = "zk")]
    if let Some((_, found_idx)) = &result {
        // The selected-tile ZK side-effect is only full-matmul admissible when
        // the whole attempt has exactly one tile. Multi-tile production proofs
        // must go through the recursive certificate/full-matmul gate instead
        // of treating one selected tile as the whole work unit.
        if ctx.params.validate_prod_envelope().is_ok() && ctx.params.num_tiles() == 1 {
            let _zk = crate::zk_bridge::prove_and_verify_for_block(
                ctx, &ctx.params, &ctx.nonce, *found_idx,
            )
            .expect("F1 zk bridge: single-tile prove + pow-verify must succeed for a found tile");
        }
    }

    Ok(result.map(|(p, _)| p))
}

/// Shared inner per-nonce attempt — returns both the plain proof
/// and the winning linear tile index (the SNARK bridge consumes
/// `found_idx`).
fn mine_inner(
    ctx: &BlockContext<'_>,
    target: &[u8; 32],
    opts: ProverOptions,
) -> Result<Option<(MatmulProof, u32)>, MineError> {
    let params = &ctx.params;
    let pow_key = pow_key_for_nonce(&ctx.s_a, &ctx.nonce);

    // Final attempt hash: these M states are already nonce-bound because the
    // context's kappa/noise/matmul were built from block_state(block, nonce).
    let num_tiles = params.num_tiles() as usize;
    let mut leaves: Vec<[u8; 32]> = Vec::with_capacity(num_tiles);
    for state in &ctx.m_states {
        leaves.push(state.keyed_hash(&pow_key));
    }

    let comm_m = merkle_root(&leaves)?;
    let chal = challenge_seed(&ctx.attempt_state, &comm_m, &ctx.tag);

    let _ = opts;
    let found_idx =
        attempt_tile_index(&ctx.attempt_state, &ctx.tag, &ctx.s_a, num_tiles as u64) as u32;
    let h = &leaves[found_idx as usize];
    if !hash_le_target(h, target) {
        return Ok(None);
    }

    let spot_indices = challenge_indices(&chal, params.spot_checks, num_tiles as u64);

    let found_opening = build_tile_opening(ctx, &leaves, found_idx.into())?;
    let mut spot = Vec::with_capacity(spot_indices.len());
    for &idx in &spot_indices {
        spot.push(build_tile_opening(ctx, &leaves, idx)?);
    }

    let plain_proof = MatmulProof {
        comm_m,
        params_tag: ctx.tag,
        h_a: ctx.h_a,
        h_b: ctx.h_b,
        // Canonical seeds are derived from the nonce-keyed chunk commitments
        // that the recursive ZK proof binds as HASH_A/HASH_B. The plain
        // verifier still authenticates row/column roots only inside the plain
        // proof's spot openings, so this legacy artifact remains diagnostic,
        // not consensus.
        h_a_chunk: ctx.h_a_chunk,
        h_b_chunk: ctx.h_b_chunk,
        found: found_opening,
        spot,
    };

    // The Pearl-analog ZK wrapping that the previous monolithic
    // `mine_with_context` performed (`zk_bridge::prove_and_verify_
    // for_block`) is intentionally NOT done here. The bridge
    // re-derives the difficulty target from `params` per MED-3, so
    // running it against an arbitrary external `target` would
    // mismatch. `mine_with_context` (the params-derived-target
    // wrapper) drives the SNARK on the returned `found_idx`
    // exactly when that mismatch is impossible.
    Ok(Some((plain_proof, found_idx)))
}

fn build_tile_opening(
    ctx: &BlockContext<'_>,
    leaves: &[[u8; 32]],
    tile_idx: u64,
) -> Result<TileOpening, MineError> {
    let params = &ctx.params;
    let (i, j) = params.tile_coords(tile_idx);
    let t = params.tile as usize;
    let k = params.k as usize;
    let row0 = (i * params.tile) as usize;
    let col0 = (j * params.tile) as usize;

    let m_path = merkle_path(leaves, tile_idx as usize)?;

    // Row strips of A: t contiguous rows, each k entries.
    let mut a_rows = Vec::with_capacity(t * k);
    let mut a_row_paths = Vec::with_capacity(t);
    for di in 0..t {
        let global_row = row0 + di;
        let off = global_row * k;
        a_rows.extend_from_slice(&ctx.a[off..off + k]);
        a_row_paths.push(merkle_path(&ctx.h_a_leaves, global_row)?);
    }

    // Column strips of B: t contiguous columns, each k entries.
    let mut b_cols = Vec::with_capacity(t * k);
    let mut b_col_paths = Vec::with_capacity(t);
    for dj in 0..t {
        let global_col = col0 + dj;
        let off = global_col * k;
        b_cols.extend_from_slice(&ctx.b[off..off + k]);
        b_col_paths.push(merkle_path(&ctx.h_b_leaves, global_col)?);
    }

    Ok(TileOpening {
        i,
        j,
        m_path,
        a_rows,
        b_cols,
        a_row_paths,
        b_col_paths,
    })
}
