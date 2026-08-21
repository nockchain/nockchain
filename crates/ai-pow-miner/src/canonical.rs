//! Gateway-free canonical AI-PoW block proving for the standalone miner.
//!
//! The production `--pearl-gateway` path fetches Pearl work from an external
//! gateway and proves a recursive certificate merging that Pearl proof. For a
//! self-contained fakenet run (no gateway), the miner instead proves a CANONICAL
//! MoE block directly on the CPU, bound to the node's block commitment. This is
//! the exact block the boot-time verifier-setup builder and the
//! `ai_pow_accept_e2e` integration test prove — the setup is height-keyed and
//! proof-independent, so a node's boot-installed production setup verifies it.
//!
//! These functions are copied from `ai-pow-jets::setup` (they use only `ai-pow`,
//! `ai-pow-zk`, and this crate's `certificate_noun` — nothing from `ai-pow-jets`),
//! because `ai-pow-jets` already depends on this crate, so this crate cannot
//! depend back on it. Keep them in sync with the jets copy (the node's setup
//! builder must prove the same shape it later verifies).

use std::sync::Arc;

use ai_pow::fiat_shamir::canonical_noise_seeds_moe;
use ai_pow::matmul::{
    compute_pattern_tile_state_from_slices, BlockNoise, PatternTileScratch, TileState,
};
use ai_pow::params::MatmulParams;
use ai_pow::pearl_compat::{
    compute_pearl_moe_ticket, derive_pearl_moe_work_commitments,
    evaluate_pearl_merge_checked_ticket_attempt, moe_expert_b_cols_from_local,
    pearl_bitcoin_double_sha256_raw, pearl_jackpot_hash, pearl_kappa, pearl_matrix_commitments,
    prepare_pearl_pattern_job, PearlAuxInclusionProof, PearlIncompleteBlockHeader,
    PearlMergeCheckedTicketAttempt, PearlMiningConfig, PearlMoeParams, PearlNockchainAux,
    PearlPeriodicPattern, PearlPublicProofParams, PearlWorkCommitments, PreparedPearlPatternJob,
    PEARL_MINING_CONFIG_RESERVED_SIZE, PEARL_MMA_INT7XINT7_TO_INT32,
    PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG,
};
use ai_pow::pearl_moe_routing::build_routing_data;
use ai_pow::synth::{synth_matrices, AI_POW_PROD_SYNTH_SEED};
use ai_pow::zk_bridge::{
    prove_pearl_merge_compact_recursive_certificate_checked,
    prove_pearl_moe_compact_recursive_certificate, AiPowCompactRecursiveCertificateRun,
    PearlMoeCompactProveRun,
};

use crate::certificate_noun::{
    AiPowCertificateShape, AiProofNode, PearlMergeMoeArtifact, PearlMergePublicStatementShape,
};

/// Error proving a canonical AI-PoW block.
#[derive(Debug)]
pub struct CanonicalProveError(pub String);

impl std::fmt::Display for CanonicalProveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "canonical ai-pow prove: {}", self.0)
    }
}
impl std::error::Error for CanonicalProveError {}

fn err<E: std::fmt::Debug>(what: &str) -> impl FnOnce(E) -> CanonicalProveError + '_ {
    move |e| CanonicalProveError(format!("{what}: {e:?}"))
}

/// The canonical submission block: the pieces needed to assemble its `%ai-pow`
/// artifact noun after proving.
pub struct CanonicalBlock {
    pub statement: PearlMergePublicStatementShape,
    pub aux_inclusion: PearlAuxInclusionProof,
    pub moe_art: PearlMergeMoeArtifact,
    pub certificate: AiPowCertificateShape,
    pub commit: [u8; 32],
    pub jackpot_hash: [u8; 32],
}

/// A proved dense canonical block that is ready for `AIP1` artifact encoding.
pub struct CanonicalDenseBlock {
    pub attempt: PearlMergeCheckedTicketAttempt,
    pub aux_inclusion: PearlAuxInclusionProof,
    pub a: Arc<Vec<i8>>,
    pub b: Arc<Vec<i8>>,
    pub run: AiPowCompactRecursiveCertificateRun,
    pub commit: [u8; 32],
    pub jackpot_hash: [u8; 32],
}

fn setup_pattern(len: u32) -> PearlPeriodicPattern {
    PearlPeriodicPattern {
        shape: [(1, len), (len, 1), (len, 1)],
    }
}

fn setup_aux(commit: [u8; 32]) -> PearlNockchainAux {
    PearlNockchainAux {
        nockchain_chain_id: b"nockchain-mainnet\0".to_vec(),
        nock_block_commitment: commit,
        nockchain_target_epoch_or_height: 123_456,
        extra_domain_data: b"ai-pow-target-window\0\0".to_vec(),
    }
}

/// Base header timestamp; `extranonce == 0` reproduces the exact block the boot
/// verifier-setup builder and `ai_pow_accept_e2e` prove (byte-stable).
const CANONICAL_BASE_TIMESTAMP: u32 = 0x6677_8899;

/// `nbits` for the synthetic Pearl header.
///
/// The gateway-free miner is not merge-mining a real Pearl chain, so this only
/// has to be a value the accept path can carry. The accept path scales the
/// Pearl target by the same tile shape factor it scales the Nockchain target by
/// (`pearl_adjusted_target`), in 256 bits, fail-closed — so a base target above
/// `2^232` makes the whole verify error out before the Nockchain difficulty
/// gate is ever reached, whatever the jackpot is.
///
/// `0x1d7fffff` decodes to `0x7fffff · 2^208` (~2^231): the loosest Pearl target
/// that still scales at the envelope-maximum factor `2^24`. Pearl's own target
/// is not gated on the Nockchain accept path — only the Nockchain target is —
/// so nothing is made harder by choosing a representable value here.
///
/// Regtest-max `0x207fffff` (~2^255) does NOT scale, and a header carrying it
/// cannot produce an acceptable block.
pub const CANONICAL_NBITS: u32 = 0x1d7f_ffff;

/// Build the synthetic Pearl header + aux-inclusion proof for one grind attempt.
///
/// `extranonce` varies ONLY the header `timestamp`, which feeds `sigma =
/// header.to_bytes()` and therefore `kappa` → the noised matmul → the jackpot —
/// so each extranonce is a fresh proof-of-work attempt. It does NOT touch the
/// coinbase (hence the `merkle_root`), so the aux inclusion that binds
/// `nock_commit` stays valid across the whole grind. The node's verifier
/// re-derives everything from the SUBMITTED header, so it accepts any winning
/// extranonce (it only re-checks aux inclusion + `jackpot <= target`).
fn setup_aux_inclusion(
    aux_commitment: &[u8; 32],
    extranonce: u32,
) -> (PearlIncompleteBlockHeader, PearlAuxInclusionProof) {
    let mut script = Vec::from([0x01u8, 0x00]);
    script.extend_from_slice(PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG);
    script.extend_from_slice(aux_commitment);
    let mut coinbase_tx = Vec::new();
    coinbase_tx.extend_from_slice(&1u32.to_le_bytes());
    coinbase_tx.push(1);
    coinbase_tx.extend_from_slice(&[0u8; 32]);
    coinbase_tx.extend_from_slice(&u32::MAX.to_le_bytes());
    coinbase_tx.push(script.len() as u8);
    coinbase_tx.extend_from_slice(&script);
    coinbase_tx.extend_from_slice(&u32::MAX.to_le_bytes());
    coinbase_tx.push(1);
    coinbase_tx.extend_from_slice(&0u64.to_le_bytes());
    coinbase_tx.push(1);
    coinbase_tx.push(0x51);
    coinbase_tx.extend_from_slice(&0u32.to_le_bytes());
    let mut merkle_root = pearl_bitcoin_double_sha256_raw(&coinbase_tx);
    merkle_root.reverse();
    let header = PearlIncompleteBlockHeader {
        version: 0x0102_0304,
        prev_block: [0x11; 32],
        merkle_root,
        timestamp: CANONICAL_BASE_TIMESTAMP.wrapping_add(extranonce),
        nbits: CANONICAL_NBITS,
    };
    (
        header,
        PearlAuxInclusionProof {
            coinbase_tx,
            merkle_branch: Vec::new(),
        },
    )
}

/// Return the candidate-bound dense mining header before inference tensors exist.
///
/// The header depends on the node commitment and extranonce, not on matrix
/// dimensions or contents. The proof path reconstructs the same header after it
/// receives the opened tensors.
pub fn canonical_dense_incomplete_header(
    nock_commit: [u8; 32],
    extranonce: u32,
) -> Result<[u8; ai_pow::pearl_compat::PEARL_INCOMPLETE_BLOCK_HEADER_SIZE], CanonicalProveError> {
    let aux = setup_aux(nock_commit);
    let aux_commitment = aux.commitment().map_err(err("dense aux commitment"))?;
    let (header, _) = setup_aux_inclusion(&aux_commitment, extranonce);
    Ok(header.to_bytes())
}

struct CanonicalMoeInputs {
    a: Vec<i8>,
    b: Vec<i8>,
    commitments: ai_pow::pearl_compat::PearlWorkCommitments,
    routing: ai_pow::pearl_moe_routing::RoutingData,
    inner: Vec<u32>,
    local_b: Vec<u32>,
    n_e: usize,
    m: usize,
    config: PearlMiningConfig,
    header: PearlIncompleteBlockHeader,
    aux: PearlNockchainAux,
    aux_commitment: [u8; 32],
    aux_inclusion: PearlAuxInclusionProof,
}

struct CanonicalMoeSchedule {
    config: PearlMiningConfig,
    routing: ai_pow::pearl_moe_routing::RoutingData,
    inner: Vec<u32>,
    local_b: Vec<u32>,
    n_e: usize,
    m: usize,
}

/// Immutable canonical MoE schedule and transcript-independent matrices.
///
/// An extranonce changes only the header timestamp. All resulting transcript
/// values—commitments, seeds, noised strips, tile state, and jackpot—are
/// recomputed for every evaluation.
pub struct PreparedCanonicalMoeTemplate {
    params: MatmulParams,
    config: PearlMiningConfig,
    a: Vec<i8>,
    b: Vec<i8>,
    routing: ai_pow::pearl_moe_routing::RoutingData,
    routing_data: Vec<u8>,
    routing_offsets: Vec<u8>,
    inner: Vec<u32>,
    local_b: Vec<u32>,
    n_e: usize,
    m: usize,
    outer_indices: Vec<u32>,
    b_cols_global: Vec<u32>,
    header: PearlIncompleteBlockHeader,
    aux: PearlNockchainAux,
    aux_commitment: [u8; 32],
    aux_inclusion: PearlAuxInclusionProof,
    mu: [u8; ai_pow::pearl_compat::PEARL_MINING_CONFIG_SIZE],
}

/// Immutable dense canonical inputs for one Nockchain block commitment.
///
/// Source matrices and auxiliary inclusion remain immutable across
/// attempt-bound search transcripts.
pub struct PreparedCanonicalDenseTemplate {
    params: MatmulParams,
    config: PearlMiningConfig,
    a: Arc<Vec<i8>>,
    b: Arc<Vec<i8>>,
    header: PearlIncompleteBlockHeader,
    aux: PearlNockchainAux,
    aux_inclusion: PearlAuxInclusionProof,
}

/// Host metadata for one dense search transcript prepared on the GPU.
pub struct PreparedCanonicalDenseSearch {
    extranonce: u32,
    header: PearlIncompleteBlockHeader,
    config: PearlMiningConfig,
    params: MatmulParams,
    sigma: [u8; ai_pow::pearl_compat::PEARL_INCOMPLETE_BLOCK_HEADER_SIZE],
    mu: [u8; ai_pow::pearl_compat::PEARL_MINING_CONFIG_SIZE],
    row_tickets: u64,
    col_tickets: u64,
}

/// Reusable mutable storage for one canonical template worker.
pub struct PreparedCanonicalMoeScratch {
    noise: BlockNoise,
    e_row: Vec<i8>,
    f_col: Vec<i8>,
    a_prime_rows: Vec<i8>,
    b_prime_cols: Vec<i8>,
    tile: PatternTileScratch,
}

/// Search-only values for one canonical extranonce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanonicalMoeSearchResult {
    pub commitments: PearlWorkCommitments,
    pub tile_state: TileState,
    pub jackpot_hash: [u8; 32],
}

impl PreparedCanonicalMoeTemplate {
    pub fn new(
        params: &MatmulParams,
        hw: u32,
        e: usize,
        top_k: usize,
        nock_commit: [u8; 32],
    ) -> Result<Self, CanonicalProveError> {
        let CanonicalMoeSchedule {
            config,
            routing,
            inner,
            local_b,
            n_e,
            m,
        } = canonical_moe_schedule(params, hw, e, top_k)?;
        let (a, b) = synth_matrices(AI_POW_PROD_SYNTH_SEED, params);
        let aux = setup_aux(nock_commit);
        let aux_commitment = aux.commitment().map_err(err("aux commitment"))?;
        let (header, aux_inclusion) = setup_aux_inclusion(&aux_commitment, 0);
        let mu = config.to_bytes().map_err(err("config bytes"))?;
        let outer_indices = routing
            .outer_indices(0, &inner)
            .map_err(err("outer indices"))?;
        let b_cols_global =
            moe_expert_b_cols_from_local(&local_b, 0, n_e).map_err(err("expert columns"))?;
        let routing_data = routing.routing_data_le_bytes();
        let routing_offsets = routing.routing_offsets_le_bytes();

        Ok(Self {
            params: *params,
            config,
            a,
            b,
            routing,
            routing_data,
            routing_offsets,
            inner,
            local_b,
            n_e,
            m,
            outer_indices,
            b_cols_global,
            header,
            aux,
            aux_commitment,
            aux_inclusion,
            mu,
        })
    }

    pub fn config(&self) -> PearlMiningConfig {
        self.config
    }

    pub fn aux(&self) -> &PearlNockchainAux {
        &self.aux
    }

    pub const fn aux_commitment(&self) -> &[u8; 32] {
        &self.aux_commitment
    }

    pub fn aux_inclusion(&self) -> &PearlAuxInclusionProof {
        &self.aux_inclusion
    }

    /// Fixed canonical inputs used to derive Pearl V3 commitments on an accelerator.
    ///
    /// The returned values are immutable for this template. Attempt-dependent
    /// values, including `kappa`, matrix commitments, seeds, and noised strips,
    /// must still be derived for every extranonce.
    pub fn gpu_inputs(
        &self,
    ) -> (
        &[i8],
        &[i8],
        &[u8],
        &[u8],
        &[u32],
        &[u32],
        [u8; ai_pow::pearl_compat::PEARL_INCOMPLETE_BLOCK_HEADER_SIZE],
        [u8; ai_pow::pearl_compat::PEARL_MINING_CONFIG_SIZE],
    ) {
        (
            &self.a,
            &self.b,
            &self.routing_data,
            &self.routing_offsets,
            &self.outer_indices,
            &self.b_cols_global,
            self.header.to_bytes(),
            self.mu,
        )
    }

    pub fn scratch(&self) -> PreparedCanonicalMoeScratch {
        let k = self.params.k as usize;
        PreparedCanonicalMoeScratch {
            noise: BlockNoise::for_params(&self.params),
            e_row: vec![0; k],
            f_col: vec![0; k],
            a_prime_rows: vec![0; self.outer_indices.len() * k],
            b_prime_cols: vec![0; self.b_cols_global.len() * k],
            tile: PatternTileScratch::new(self.outer_indices.len(), self.b_cols_global.len()),
        }
    }

    /// Whether a worker scratch allocation has this template's matrix shape.
    pub fn scratch_matches(&self, scratch: &PreparedCanonicalMoeScratch) -> bool {
        let k = self.params.k as usize;
        scratch.noise.m == self.params.m
            && scratch.noise.k == self.params.k
            && scratch.noise.n == self.params.n
            && scratch.noise.r == self.params.noise_rank
            && scratch.e_row.len() == k
            && scratch.f_col.len() == k
            && self
                .outer_indices
                .len()
                .checked_mul(k)
                .is_some_and(|len| scratch.a_prime_rows.len() == len)
            && self
                .b_cols_global
                .len()
                .checked_mul(k)
                .is_some_and(|len| scratch.b_prime_cols.len() == len)
            && scratch
                .tile
                .matches_dimensions(self.outer_indices.len(), self.b_cols_global.len())
    }

    pub fn header_for(&self, extranonce: u32) -> PearlIncompleteBlockHeader {
        PearlIncompleteBlockHeader {
            timestamp: self.header.timestamp.wrapping_add(extranonce),
            ..self.header
        }
    }

    /// Recompute attempt-dependent commitments and noised opened strips.
    ///
    /// Search backends can upload the resulting strips to an accelerator. The
    /// storage belongs to `scratch` and is overwritten by the next attempt.
    pub fn prepare_attempt(
        &self,
        extranonce: u32,
        scratch: &mut PreparedCanonicalMoeScratch,
    ) -> PearlWorkCommitments {
        let header = self.header_for(extranonce);
        let kappa = pearl_kappa(&header.to_bytes(), &self.mu);
        let (h_a, h_b) = pearl_matrix_commitments(&self.a, &self.b, &kappa);
        let (s_a, s_b, _) = canonical_noise_seeds_moe(
            &kappa, &h_a, &h_b, self.params.m, self.n_e as u32, &self.routing_data,
            &self.routing_offsets,
        );
        scratch.noise.refill(&s_a, &s_b, &self.params);

        let k = self.params.k as usize;
        for (slot, &row) in self.outer_indices.iter().enumerate() {
            scratch.noise.e_row_into(row, &mut scratch.e_row);
            let source = &self.a[row as usize * k..(row as usize + 1) * k];
            let destination = &mut scratch.a_prime_rows[slot * k..(slot + 1) * k];
            for ((out, &value), &noise) in destination.iter_mut().zip(source).zip(&scratch.e_row) {
                *out = (value as i16 + noise as i16) as i8;
            }
        }
        for (slot, &col) in self.b_cols_global.iter().enumerate() {
            scratch.noise.f_col_into(col, &mut scratch.f_col);
            let source = &self.b[col as usize * k..(col as usize + 1) * k];
            let destination = &mut scratch.b_prime_cols[slot * k..(slot + 1) * k];
            for ((out, &value), &noise) in destination.iter_mut().zip(source).zip(&scratch.f_col) {
                *out = (value as i16 + noise as i16) as i8;
            }
        }
        PearlWorkCommitments {
            kappa,
            h_a,
            h_b,
            s_a,
            s_b,
        }
    }

    /// Opened noised strips produced by [`Self::prepare_attempt`].
    pub fn prepared_strips<'a>(
        &self,
        scratch: &'a PreparedCanonicalMoeScratch,
    ) -> (&'a [i8], &'a [i8]) {
        (&scratch.a_prime_rows, &scratch.b_prime_cols)
    }

    /// Recompute the complete attempt-dependent canonical transcript in reusable storage.
    pub fn evaluate(
        &self,
        extranonce: u32,
        scratch: &mut PreparedCanonicalMoeScratch,
    ) -> CanonicalMoeSearchResult {
        let commitments = self.prepare_attempt(extranonce, scratch);
        let k = self.params.k as usize;
        let tile_state = compute_pattern_tile_state_from_slices(
            &scratch.a_prime_rows,
            &scratch.b_prime_cols,
            self.outer_indices.len(),
            self.b_cols_global.len(),
            k,
            self.params.noise_rank as usize,
            k,
            &mut scratch.tile,
        );
        CanonicalMoeSearchResult {
            commitments,
            tile_state,
            jackpot_hash: pearl_jackpot_hash(&tile_state, &commitments.s_a),
        }
    }

    pub fn schedule(
        &self,
    ) -> (
        &ai_pow::pearl_moe_routing::RoutingData,
        &[u32],
        &[u32],
        usize,
        usize,
    ) {
        (&self.routing, &self.inner, &self.local_b, self.n_e, self.m)
    }
}

impl PreparedCanonicalDenseTemplate {
    pub fn new(
        params: &MatmulParams,
        nock_commit: [u8; 32],
        a: Arc<Vec<i8>>,
        b: Arc<Vec<i8>>,
    ) -> Result<Self, CanonicalProveError> {
        params
            .validate_prod_envelope()
            .map_err(err("dense canonical parameter envelope"))?;
        if params.spot_checks != 1 || params.difficulty_bits != 0 {
            return Err(CanonicalProveError(
                "dense canonical proof requires spot_checks=1 and difficulty_bits=0".to_string(),
            ));
        }
        let expected_a = usize::try_from(params.m)
            .ok()
            .and_then(|m| m.checked_mul(params.k as usize))
            .ok_or_else(|| CanonicalProveError("dense A length overflow".to_string()))?;
        let expected_b = usize::try_from(params.n)
            .ok()
            .and_then(|n| n.checked_mul(params.k as usize))
            .ok_or_else(|| CanonicalProveError("dense B length overflow".to_string()))?;
        if a.len() != expected_a || b.len() != expected_b {
            return Err(CanonicalProveError(format!(
                "dense matrix lengths must be {expected_a} and {expected_b}, got {} and {}",
                a.len(),
                b.len()
            )));
        }
        let config = PearlMiningConfig {
            common_dim: params.k,
            rank: u16::try_from(params.noise_rank)
                .map_err(|_| CanonicalProveError("dense rank does not fit u16".to_string()))?,
            mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
            rows_pattern: setup_pattern(params.tile),
            cols_pattern: setup_pattern(params.tile),
            reserved: [0; PEARL_MINING_CONFIG_RESERVED_SIZE],
        };
        config.to_bytes().map_err(err("dense mining config"))?;
        let aux = setup_aux(nock_commit);
        let aux_commitment = aux.commitment().map_err(err("dense aux commitment"))?;
        let (header, aux_inclusion) = setup_aux_inclusion(&aux_commitment, 0);
        Ok(Self {
            params: *params,
            config,
            a,
            b,
            header,
            aux,
            aux_inclusion,
        })
    }

    pub const fn params(&self) -> MatmulParams {
        self.params
    }

    pub const fn config(&self) -> PearlMiningConfig {
        self.config
    }

    pub fn header_for(&self, extranonce: u32) -> PearlIncompleteBlockHeader {
        PearlIncompleteBlockHeader {
            timestamp: self.header.timestamp.wrapping_add(extranonce),
            ..self.header
        }
    }

    pub fn prepare_search(
        &self,
        extranonce: u32,
    ) -> Result<PreparedCanonicalDenseSearch, CanonicalProveError> {
        let header = self.header_for(extranonce);
        let row_tickets = u64::from(self.params.m / self.params.tile);
        let col_tickets = u64::from(self.params.n / self.params.tile);
        if row_tickets == 0 || col_tickets == 0 {
            return Err(CanonicalProveError(
                "dense search requires at least one complete tile".to_string(),
            ));
        }
        row_tickets
            .checked_mul(col_tickets)
            .ok_or_else(|| CanonicalProveError("dense ticket count overflow".to_string()))?;
        Ok(PreparedCanonicalDenseSearch {
            extranonce,
            header,
            config: self.config,
            params: self.params,
            sigma: header.to_bytes(),
            mu: self
                .config
                .to_bytes()
                .map_err(err("dense search mining config"))?,
            row_tickets,
            col_tickets,
        })
    }

    pub fn prepare(&self, extranonce: u32) -> Result<PreparedPearlPatternJob, CanonicalProveError> {
        prepare_pearl_pattern_job(
            &self.header_for(extranonce),
            &self.config,
            &self.params,
            &self.a,
            &self.b,
            self.params.tile as usize,
        )
        .map_err(err("prepare dense canonical template"))
    }

    pub fn checked_winner(
        &self,
        prepared: &PreparedPearlPatternJob,
        ordinal: u64,
        nockchain_target: &[u8; 32],
    ) -> Result<PearlMergeCheckedTicketAttempt, CanonicalProveError> {
        if prepared.params() != self.params || prepared.config() != self.config {
            return Err(CanonicalProveError(
                "dense winner belongs to a different prepared template".to_string(),
            ));
        }
        let (t_rows, t_cols) = prepared.offsets_at_ordinal(ordinal).ok_or_else(|| {
            CanonicalProveError("dense winner ordinal is out of range".to_string())
        })?;
        let attempt = evaluate_pearl_merge_checked_ticket_attempt(
            &prepared.header(),
            &self.config,
            &self.params,
            t_rows,
            t_cols,
            &self.a,
            &self.b,
            nockchain_target,
            self.params.tile as usize,
            self.aux.clone(),
        )
        .map_err(err("validate dense canonical winner"))?;
        attempt
            .public_params
            .check_nockchain_jackpot_target(nockchain_target)
            .map_err(err("dense canonical winner target"))?;
        Ok(attempt)
    }

    pub fn checked_search_winner(
        &self,
        prepared: &PreparedCanonicalDenseSearch,
        ordinal: u64,
        nockchain_target: &[u8; 32],
    ) -> Result<PearlMergeCheckedTicketAttempt, CanonicalProveError> {
        if prepared.params != self.params
            || prepared.config != self.config
            || prepared.header != self.header_for(prepared.extranonce)
        {
            return Err(CanonicalProveError(
                "dense winner belongs to a different search transcript".to_string(),
            ));
        }
        let (t_rows, t_cols) = prepared.offsets_at_ordinal(ordinal).ok_or_else(|| {
            CanonicalProveError("dense winner ordinal is out of range".to_string())
        })?;
        let attempt = evaluate_pearl_merge_checked_ticket_attempt(
            &prepared.header,
            &self.config,
            &self.params,
            t_rows,
            t_cols,
            &self.a,
            &self.b,
            nockchain_target,
            self.params.tile as usize,
            self.aux.clone(),
        )
        .map_err(err("validate dense canonical search winner"))?;
        attempt
            .public_params
            .check_nockchain_jackpot_target(nockchain_target)
            .map_err(err("dense canonical search winner target"))?;
        Ok(attempt)
    }

    pub fn prove(
        &self,
        attempt: PearlMergeCheckedTicketAttempt,
    ) -> Result<CanonicalDenseBlock, CanonicalProveError> {
        let run = prove_pearl_merge_compact_recursive_certificate_checked(
            &attempt, &self.params, &self.a, &self.b,
        )
        .map_err(err("prove dense canonical winner"))?;
        let jackpot_hash = attempt.ticket.jackpot_hash;
        Ok(CanonicalDenseBlock {
            attempt,
            aux_inclusion: self.aux_inclusion.clone(),
            a: Arc::clone(&self.a),
            b: Arc::clone(&self.b),
            run,
            commit: self.aux.nock_block_commitment,
            jackpot_hash,
        })
    }
}

impl PreparedCanonicalDenseSearch {
    pub const fn config(&self) -> PearlMiningConfig {
        self.config
    }

    pub const fn params(&self) -> MatmulParams {
        self.params
    }

    pub fn sigma(&self) -> &[u8] {
        &self.sigma
    }

    pub fn mu(&self) -> &[u8] {
        &self.mu
    }

    pub fn total_tickets(&self) -> u64 {
        self.row_tickets * self.col_tickets
    }

    pub fn offsets_at_ordinal(&self, ordinal: u64) -> Option<(u32, u32)> {
        if ordinal >= self.total_tickets() {
            return None;
        }
        let row = u32::try_from(ordinal / self.col_tickets).ok()?;
        let col = u32::try_from(ordinal % self.col_tickets).ok()?;
        Some((
            row.checked_mul(self.params.tile)?,
            col.checked_mul(self.params.tile)?,
        ))
    }
}

/// The Pearl mining config the canonical miner puts in every statement it
/// builds.
///
/// Public so the grind loop can derive its accept threshold
/// (`config.shape_work_factor()`) from the SAME object the statement carries
/// and the verifier re-parses. Deriving it from a parallel copy of
/// `(h, w, k, r)` is how a miner's predicate silently diverges from consensus.
pub fn canonical_mining_config(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
) -> PearlMiningConfig {
    PearlMiningConfig {
        common_dim: params.k,
        rank: params.noise_rank as u16,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: setup_pattern(hw),
        cols_pattern: setup_pattern(hw),
        reserved: PearlMiningConfig::moe_trailer(e as u16, top_k as u16),
    }
}

fn canonical_moe_schedule(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
) -> Result<CanonicalMoeSchedule, CanonicalProveError> {
    let m = params.m as usize;
    let n = params.n as usize;
    if e == 0 || !n.is_multiple_of(e) {
        return Err(CanonicalProveError(format!("n={n} not divisible by e={e}")));
    }
    let n_e = n / e;
    let config = canonical_mining_config(params, hw, e, top_k);
    let topk: Vec<u32> = (0..m).map(|t| (t % e) as u32).collect();
    let routing = build_routing_data(&topk, m, top_k, e).map_err(err("routing"))?;
    let inner = config
        .rows_pattern
        .indices_with_offset_bounded(0, 4096)
        .map_err(err("inner"))?;
    let local_b = config
        .cols_pattern
        .indices_with_offset_bounded(0, 4096)
        .map_err(err("local_b"))?;
    Ok(CanonicalMoeSchedule {
        config,
        routing,
        inner,
        local_b,
        n_e,
        m,
    })
}

fn canonical_moe_inputs(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
    nock_commit: [u8; 32],
    extranonce: u32,
) -> Result<CanonicalMoeInputs, CanonicalProveError> {
    let CanonicalMoeSchedule {
        config,
        routing,
        inner,
        local_b,
        n_e,
        m,
    } = canonical_moe_schedule(params, hw, e, top_k)?;

    let (a, b) = synth_matrices(AI_POW_PROD_SYNTH_SEED, params);
    let aux = setup_aux(nock_commit);
    let aux_commitment = aux.commitment().map_err(err("aux commitment"))?;
    let (header, aux_inclusion) = setup_aux_inclusion(&aux_commitment, extranonce);
    let mu = config.to_bytes().map_err(err("config bytes"))?;
    let n_e_u32 = u32::try_from(n_e).map_err(err("n_e"))?;
    let commitments = derive_pearl_moe_work_commitments(
        &header.to_bytes(),
        &mu,
        &a,
        &b,
        params.m,
        n_e_u32,
        &routing.routing_data_le_bytes(),
        &routing.routing_offsets_le_bytes(),
    );

    Ok(CanonicalMoeInputs {
        a,
        b,
        commitments,
        routing,
        inner,
        local_b,
        n_e,
        m,
        config,
        header,
        aux,
        aux_commitment,
        aux_inclusion,
    })
}

/// Cheap proof-of-work grind step: compute the full work ticket for one attempt
/// (`nock_commit`, `extranonce`) — the noised MoE tile matmul + BLAKE3 jackpot, and
/// NOT the ~25-30s recursive certificate. The returned ticket is byte-identical to
/// the one the matching [`prove_canonical_moe_block_at`] certifies (both route
/// through `compute_pearl_moe_ticket` with the same inputs), so a jackpot found
/// here is guaranteed to survive the certificate's `jackpot <= target` gate.
///
/// This is the per-attempt proof-of-work UNIT. The jackpot is
/// `keyed_hash(tile_state, s_a)` — a function of ONLY the tile matmul output and
/// the noise seed, both of which derive from `kappa = BLAKE3(sigma || mu)`. There
/// is no separate nonce: changing `extranonce` (the header timestamp inside
/// `sigma`) changes `kappa` → `s_a`/`s_b` → the noise → the tile matmul → the
/// jackpot. So a fresh jackpot trial is impossible without a fresh tile inference.
pub fn evaluate_canonical_moe_ticket(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
    nock_commit: [u8; 32],
    extranonce: u32,
) -> Result<ai_pow::pearl_compat::PearlMoeTicket, CanonicalProveError> {
    let CanonicalMoeInputs {
        a,
        b,
        commitments,
        routing,
        inner,
        local_b,
        n_e,
        m,
        ..
    } = canonical_moe_inputs(params, hw, e, top_k, nock_commit, extranonce)?;
    // Mirror the prover's ticket call exactly (expert 0, dot_product_len == k).
    compute_pearl_moe_ticket(
        &commitments.kappa, &commitments.h_a, &commitments.h_b, &a, &b, &routing, 0, &inner,
        &local_b, n_e, m as u32, params.k as usize, params.noise_rank as usize, params.k as usize,
    )
    .map_err(err("moe ticket"))
}

/// Cheap grind step returning only the jackpot hash (see
/// [`evaluate_canonical_moe_ticket`]).
pub fn evaluate_canonical_moe_jackpot(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
    nock_commit: [u8; 32],
    extranonce: u32,
) -> Result<[u8; 32], CanonicalProveError> {
    Ok(evaluate_canonical_moe_ticket(params, hw, e, top_k, nock_commit, extranonce)?.jackpot_hash)
}

/// Prove a single canonical MoE block bound to `nock_commit` at `extranonce == 0`.
/// Byte-stable back-compat wrapper (the boot setup builder / e2e prove this exact
/// block).
pub fn prove_canonical_moe_block(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
    nock_commit: [u8; 32],
) -> Result<CanonicalBlock, CanonicalProveError> {
    prove_canonical_moe_block_at(params, hw, e, top_k, nock_commit, 0)
}

/// Prove a single canonical MoE block at the given shape, bound to `nock_commit`
/// (the node's block commitment) and `extranonce` (the winning grind attempt — it
/// selects the header timestamp that made `jackpot <= target`). `hw` is the
/// opened-tile side; `e`/`top_k` the MoE config. ~25-30s on CPU for the small
/// shape. Returns errors (panics-free).
pub fn prove_canonical_moe_block_at(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
    nock_commit: [u8; 32],
    extranonce: u32,
) -> Result<CanonicalBlock, CanonicalProveError> {
    prove_canonical_moe_block_at_for_miner(params, hw, e, top_k, nock_commit, extranonce)
}

/// The `PearlPublicProofParams` the canonical miner will publish for this
/// attempt, WITHOUT paying the ~25-30s certificate cost.
///
/// `hash_jackpot` is left zero — every other field, and in particular the whole
/// `mining_config`, is byte-identical to what
/// [`prove_canonical_moe_block_at`] emits, so this is the statement the
/// consensus verifier re-parses. Exists so the grind loop's accept threshold
/// can be checked against the verifier's without proving.
pub fn canonical_public_params(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
    nock_commit: [u8; 32],
    extranonce: u32,
) -> Result<PearlPublicProofParams, CanonicalProveError> {
    let CanonicalMoeInputs {
        commitments,
        n_e,
        m,
        config,
        header,
        ..
    } = canonical_moe_inputs(params, hw, e, top_k, nock_commit, extranonce)?;
    Ok(PearlPublicProofParams {
        block_header: header,
        mining_config: config,
        hash_a: commitments.h_a,
        hash_b: commitments.h_b,
        hash_jackpot: [0u8; 32],
        m: m as u32,
        n: n_e as u32,
        t_rows: 0,
        t_cols: 0,
    })
}

/// The authenticated statement and MoE artifact the canonical miner will
/// publish for this attempt, WITHOUT paying the certificate cost.
///
/// Unlike [`canonical_public_params`] the returned `hash_jackpot` is the real
/// one for `(nock_commit, extranonce)`, so the pair is a complete MoE work
/// statement: it passes aux binding and `verify_pearl_moe_compatible_work`
/// against any target the jackpot clears. Only the recursive certificate is
/// absent, which is what makes it usable for testing the pre-proof gates.
pub fn canonical_moe_statement_parts(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
    nock_commit: [u8; 32],
    extranonce: u32,
) -> Result<(PearlPublicProofParams, PearlMergeMoeArtifact), CanonicalProveError> {
    let CanonicalMoeInputs {
        commitments,
        routing,
        n_e,
        m,
        config,
        header,
        ..
    } = canonical_moe_inputs(params, hw, e, top_k, nock_commit, extranonce)?;
    let ticket = evaluate_canonical_moe_ticket(params, hw, e, top_k, nock_commit, extranonce)?;
    let public = PearlPublicProofParams {
        block_header: header,
        mining_config: config,
        hash_a: commitments.h_a,
        hash_b: commitments.h_b,
        hash_jackpot: ticket.jackpot_hash,
        m: m as u32,
        n: n_e as u32,
        t_rows: 0,
        t_cols: 0,
    };
    let moe_art = PearlMergeMoeArtifact {
        moe: PearlMoeParams {
            expert_idx: 0,
            routing_offsets: routing.routing_offsets.clone(),
            hash_routing: ticket.commitment.routing_root,
            outer_indices: ticket.outer_indices.clone(),
        },
        routing_data: routing.routing_data.clone(),
    };
    Ok((public, moe_art))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prove_canonical_moe_block_at_for_miner(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
    nock_commit: [u8; 32],
    extranonce: u32,
) -> Result<CanonicalBlock, CanonicalProveError> {
    prove_canonical_moe_block_at_inner(params, hw, e, top_k, nock_commit, extranonce)
        .map(|(block, _)| block)
}

#[cfg(all(test, feature = "gpu"))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn prove_canonical_moe_block_at_with_verifier_context(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
    nock_commit: [u8; 32],
    extranonce: u32,
) -> Result<
    (
        CanonicalBlock,
        ai_pow_zk::recursion::AiPowCompactBatchVerifierContext,
    ),
    CanonicalProveError,
> {
    prove_canonical_moe_block_at_inner(params, hw, e, top_k, nock_commit, extranonce)
}

#[allow(clippy::too_many_arguments)]
fn prove_canonical_moe_block_at_inner(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
    nock_commit: [u8; 32],
    extranonce: u32,
) -> Result<
    (
        CanonicalBlock,
        ai_pow_zk::recursion::AiPowCompactBatchVerifierContext,
    ),
    CanonicalProveError,
> {
    let CanonicalMoeInputs {
        a,
        b,
        commitments,
        routing,
        inner,
        local_b,
        n_e,
        m,
        config,
        header,
        aux,
        aux_commitment,
        aux_inclusion,
    } = canonical_moe_inputs(params, hw, e, top_k, nock_commit, extranonce)?;

    let run = prove_pearl_moe_compact_recursive_certificate(
        params, &a, &b, &commitments.kappa, &commitments.h_a, &commitments.h_b, &routing, 0,
        &inner, &local_b, n_e,
    )
    .map_err(err("prove"))?;

    let PearlMoeCompactProveRun {
        compact_cert,
        verifier_context,
        pis,
        zk_params,
        trace_height,
        commitments: proof_commitments,
        ticket,
        prover_cache: _,
    } = run;

    let public = PearlPublicProofParams {
        block_header: header,
        mining_config: config,
        hash_a: commitments.h_a,
        hash_b: commitments.h_b,
        hash_jackpot: ticket.jackpot_hash,
        m: m as u32,
        n: n_e as u32,
        t_rows: 0,
        t_cols: 0,
    };
    let statement = PearlMergePublicStatementShape {
        block_header: header.to_bytes(),
        public_data: public.to_public_data().map_err(err("public data"))?,
        expected_aux_commitment: aux_commitment,
        aux,
    };
    let cert_bytes =
        ai_pow_zk::recursion::encode_compact_batch_recursive_certificate(&compact_cert)
            .map_err(err("encode cert"))?;
    let certificate = AiPowCertificateShape {
        version: 1,
        zk_params,
        found_idx: 0,
        trace_height,
        commitments: proof_commitments,
        public_inputs: pis,
        certificate: AiProofNode::Bytes(cert_bytes),
    };
    let moe_art = PearlMergeMoeArtifact {
        moe: PearlMoeParams {
            expert_idx: 0,
            routing_offsets: routing.routing_offsets.clone(),
            hash_routing: ticket.commitment.routing_root,
            outer_indices: ticket.outer_indices.clone(),
        },
        routing_data: routing.routing_data.clone(),
    };

    Ok((
        CanonicalBlock {
            statement,
            aux_inclusion,
            moe_art,
            certificate,
            commit: nock_commit,
            jackpot_hash: ticket.jackpot_hash,
        },
        verifier_context,
    ))
}

#[cfg(test)]
mod tests {
    use ai_pow::tile_hash::hash_le_target;

    use super::*;

    fn canonical_params() -> MatmulParams {
        MatmulParams {
            m: 64,
            k: 1024,
            n: 64,
            noise_rank: 64,
            tile: 8,
            spot_checks: 1,
            difficulty_bits: 0,
        }
    }

    fn dense_params() -> MatmulParams {
        MatmulParams {
            m: 16,
            k: 1024,
            n: 16,
            noise_rank: 64,
            tile: 8,
            spot_checks: 1,
            difficulty_bits: 0,
        }
    }

    #[test]
    fn dense_template_rebinds_every_extranonce_and_validates_the_winner() {
        let params = dense_params();
        let (a, b) = synth_matrices(AI_POW_PROD_SYNTH_SEED, &params);
        let template =
            PreparedCanonicalDenseTemplate::new(&params, [0x5a; 32], Arc::new(a), Arc::new(b))
                .expect("dense canonical template");
        let first = template.prepare(0).expect("first dense template");
        let second = template.prepare(1).expect("second dense template");

        assert_ne!(first.header(), second.header());
        assert_ne!(first.commitments(), second.commitments());
        assert_eq!(first.row_offsets(), &[0, 8]);
        assert_eq!(first.col_offsets(), &[0, 8]);

        let mut target = [0xff; 32];
        target[30] = 0;
        target[31] = 0;
        let checked = template
            .checked_winner(&first, 0, &target)
            .expect("scalar-checked dense winner");
        let scalar = first
            .evaluate(0, 0, &mut first.scratch())
            .expect("scalar ticket");
        assert_eq!(checked.ticket.jackpot_hash, scalar.jackpot_hash);
        assert_eq!(checked.aux.nock_block_commitment, [0x5a; 32]);
    }

    /// The cheap grind evaluator must be byte-identical to the jackpot the full
    /// certificate proves, for the SAME `(commit, extranonce)` — otherwise a hit
    /// found while grinding would not survive the node's `jackpot <= target` check
    /// after proving. Also: distinct extranonces are distinct PoW attempts (fresh
    /// jackpots), and `extranonce == 0` is the byte-stable back-compat block.
    /// Ignored (one prove ~25-30s); run with:
    ///   cargo test --release -p ai-pow-miner canonical_grind_jackpot_matches_prove -- --ignored --nocapture
    #[test]
    #[ignore]
    fn canonical_grind_jackpot_matches_prove() {
        let params = canonical_params();
        let commit = [0x5au8; 32];

        // Grind jackpots vary per extranonce (fresh attempts).
        let j0 = evaluate_canonical_moe_jackpot(&params, 8, 2, 1, commit, 0).expect("eval 0");
        let j1 = evaluate_canonical_moe_jackpot(&params, 8, 2, 1, commit, 1).expect("eval 1");
        let j2 = evaluate_canonical_moe_jackpot(&params, 8, 2, 1, commit, 2).expect("eval 2");
        assert_ne!(j0, j1, "extranonce 0 vs 1 must be distinct attempts");
        assert_ne!(j1, j2, "extranonce 1 vs 2 must be distinct attempts");
        // Deterministic per (commit, extranonce).
        let j1b = evaluate_canonical_moe_jackpot(&params, 8, 2, 1, commit, 1).expect("eval 1b");
        assert_eq!(j1, j1b, "grind eval must be deterministic");

        // Grind eval == certified jackpot, for a nonzero extranonce.
        let block = prove_canonical_moe_block_at(&params, 8, 2, 1, commit, 1).expect("prove 1");
        assert_eq!(
            block.jackpot_hash, j1,
            "certified jackpot must equal the cheap grind jackpot for the same extranonce"
        );

        // extranonce 0 back-compat: same wrapper result as prove_canonical_moe_block.
        let b0a = prove_canonical_moe_block(&params, 8, 2, 1, commit).expect("prove wrapper");
        let b0b = prove_canonical_moe_block_at(&params, 8, 2, 1, commit, 0).expect("prove at 0");
        assert_eq!(b0a.jackpot_hash, b0b.jackpot_hash);
        assert_eq!(b0a.jackpot_hash, j0);

        // Sanity: a trivial (all-ones) target is cleared; a zero target is not.
        assert!(hash_le_target(&j1, &[0xFFu8; 32]));
        assert!(!hash_le_target(&j1, &[0u8; 32]));
    }

    /// ANTI-REUSE INVARIANT (no skip-inference nonce): every extranonce that
    /// changes the jackpot must force a FRESH tile inference. We assert that
    /// distinct extranonces yield distinct noise seeds (`s_a`, `s_b`) AND distinct
    /// tile matmul outputs (`tile_state`) — not merely distinct final hashes. If
    /// the tile matmul output were reused across extranonces, a miner could grind
    /// cheap jackpot trials without redoing inference; that is exactly the
    /// forbidden shortcut. Cheap (no certificate), so not ignored.
    #[test]
    fn canonical_extranonce_forces_fresh_tile_inference() {
        let params = canonical_params();
        let commit = [0x33u8; 32];
        let n = 24u32;
        let mut seen_tiles = std::collections::HashSet::new();
        let mut seen_sa = std::collections::HashSet::new();
        let mut prev: Option<ai_pow::pearl_compat::PearlMoeTicket> = None;
        for xn in 0..n {
            let t = evaluate_canonical_moe_ticket(&params, 8, 2, 1, commit, xn).expect("ticket");
            // Serialize the tile matmul output to compare/collect.
            let tile_bytes = format!("{:?}", t.tile_state);
            assert!(
                seen_tiles.insert(tile_bytes),
                "extranonce {xn}: tile matmul output repeated — inference was NOT forced fresh"
            );
            assert!(
                seen_sa.insert(t.s_a),
                "extranonce {xn}: noise seed s_a repeated — kappa did not vary"
            );
            if let Some(p) = &prev {
                assert_ne!(p.s_b, t.s_b, "s_b must vary per extranonce");
                assert_ne!(
                    p.jackpot_hash, t.jackpot_hash,
                    "jackpot must vary per extranonce"
                );
            }
            prev = Some(t);
        }
        assert_eq!(seen_tiles.len(), n as usize);
        assert_eq!(seen_sa.len(), n as usize);
    }

    /// The jackpot is a pure function of the tile matmul output and the noise seed
    /// (`keyed_hash(tile_state, s_a)`), with NO separate nonce input. Recomputing
    /// the jackpot from the ticket's own `tile_state` + `s_a` must reproduce it
    /// exactly — proving there is no hidden degree of freedom that could change the
    /// jackpot without changing the matmul.
    ///
    /// PEARL MERGE-COMPAT LOCK: Pearl keys the jackpot with `s_A` DIRECTLY
    /// (`compute_jackpot_hash(jackpot, key=a_noise_seed)`, Pearl zk-pow
    /// proof_utils.rs:1411-1415). The native path's `pow_key_for_nonce(s_a, nonce)`
    /// folds an EXTRA nonce and must NOT appear on this path — it would both break
    /// Pearl merge-compat and reintroduce a skip-inference degree of freedom. We
    /// assert the canonical jackpot equals the s_A-keyed hash and does NOT equal the
    /// nonce-folded-key hash.
    #[test]
    fn canonical_jackpot_keyed_by_s_a_direct_not_nonce_folded() {
        let params = canonical_params();
        let commit = [0x77u8; 32];
        for xn in [0u32, 1, 5, 100] {
            let t = evaluate_canonical_moe_ticket(&params, 8, 2, 1, commit, xn).expect("ticket");
            // Pearl form: BLAKE3(M, key = s_A).
            let pearl_keyed = ai_pow::pearl_compat::pearl_jackpot_hash(&t.tile_state, &t.s_a);
            assert_eq!(
                pearl_keyed, t.jackpot_hash,
                "extranonce {xn}: jackpot must equal keyed_hash(tile_state, s_a) [Pearl s_A-direct]"
            );
            // Native form (forbidden here): BLAKE3(M, key = pow_key_for_nonce(s_a, nonce)).
            let nonce_folded_key =
                ai_pow::fiat_shamir::pow_key_for_nonce(&t.s_a, &xn.to_le_bytes());
            let nonce_folded_jackpot = t.tile_state.keyed_hash(&nonce_folded_key);
            assert_ne!(
                nonce_folded_jackpot, t.jackpot_hash,
                "extranonce {xn}: canonical jackpot must NOT use the nonce-folded native key"
            );
        }
    }

    #[test]
    ///
    /// The transcript is keyed by `kappa = BLAKE3(sigma || mu)` and `sigma`
    /// includes the header `nbits`, so these values move whenever
    /// `CANONICAL_NBITS` does. See that constant for why it is not regtest-max.
    fn canonical_moe_route_kat_snapshot() {
        let params = canonical_params();
        let ticket =
            evaluate_canonical_moe_ticket(&params, 8, 2, 1, [0x42u8; 32], 7).expect("ticket");

        assert_eq!(
            hex::encode(ticket.s_a),
            "02b9580688ca4e7f6ba6ff2c1db5ca44ed07f5702e762ae3616046e81547e741"
        );
        assert_eq!(
            hex::encode(ticket.s_b),
            "fdd75fc5ae96f72705f953adf145980386572e32add83e1f043d9cf9171f3fa6"
        );
        assert_eq!(
            hex::encode(ticket.commitment.routing_root),
            "eb588a0d5bda34a2710469ed2f5c93ab9f1ebb48b6eb48f0b70505a65ade6ed0"
        );
        assert_eq!(
            hex::encode(ticket.commitment.hash_offsets),
            "488cc3d6025e355f4f2dbaf42a9b2ed3b79fab763a06c125a4eaa2ff1e0780cb"
        );
        assert_eq!(
            hex::encode(ticket.commitment.hash_routing),
            "81885e78ac3c16270dab5b5d3a5b32f1c421f9d7d099ce0984989ba47cfb974e"
        );
        assert_eq!(
            hex::encode(ticket.commitment.hash_activations),
            "71f9a0f3e1680acfd1c8175cb0f503f66f15a7939fcc807a12affd0f01f1936b"
        );
        assert_eq!(ticket.outer_indices, vec![0, 2, 4, 6, 8, 10, 12, 14]);
        assert_eq!(ticket.b_cols_global, vec![0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(
            hex::encode(ticket.jackpot_hash),
            "2d4fb8ab46f60e365ac9889e99ae2124e558fa912c74f5b8f712b3921c7b96f0"
        );
    }

    /// **A canonical block must survive the node's work precheck.**
    ///
    /// The MoE accept path scales BOTH the Pearl target (from the header's
    /// `nbits`) and the Nockchain target by the tile shape work factor, in 256
    /// bits, fail-closed — and it does so before the difficulty gate. A header
    /// whose Pearl base target cannot be scaled therefore makes every block this
    /// miner produces unacceptable, no matter how much work went into it, and
    /// the failure looks like "AI mining just never lands a block".
    ///
    /// Pinned against the loosest target consensus can emit, so this holds for
    /// the whole admissible difficulty range rather than one convenient point.
    #[test]
    fn canonical_statement_survives_the_node_work_precheck() {
        let params = canonical_params();
        let (mut public, moe_art) =
            canonical_moe_statement_parts(&params, 8, 2, 1, [0x5a; 32], 0).expect("statement");

        // Pearl's own target is not gated here, but it must be REPRESENTABLE:
        // the accept path computes it with `?` before the Nockchain gate.
        public
            .pearl_adjusted_target()
            .expect("the canonical header's nbits must scale by the shape work factor");

        // The difficulty gate is not under test; zero the jackpot so the
        // precheck is reached deterministically at the loosest legal target.
        public.hash_jackpot = [0u8; 32];
        ai_pow::pearl_compat::verify_pearl_moe_compatible_work(
            &public,
            &moe_art.moe,
            &moe_art.routing_data,
            &ai_pow::difficulty::AI_POW_MAX_CONSENSUS_TARGET,
            4096,
        )
        .expect("a canonical statement must pass the node MoE work precheck");
    }

    #[test]
    fn canonical_search_kat_snapshot() {
        use ai_pow::matmul::{compute_pattern_tile_trace_from_slices, BlockNoise};

        let params = canonical_params();
        let commit = [0x42u8; 32];
        let extranonce = 7;
        let inputs =
            canonical_moe_inputs(&params, 8, 2, 1, commit, extranonce).expect("canonical inputs");
        let ticket = compute_pearl_moe_ticket(
            &inputs.commitments.kappa, &inputs.commitments.h_a, &inputs.commitments.h_b, &inputs.a,
            &inputs.b, &inputs.routing, 0, &inputs.inner, &inputs.local_b, inputs.n_e,
            inputs.m as u32, params.k as usize, params.noise_rank as usize, params.k as usize,
        )
        .expect("canonical ticket");
        let noise = BlockNoise::expand(&ticket.s_a, &ticket.s_b, &params);
        let mut a_prime = Vec::with_capacity(ticket.outer_indices.len() * params.k as usize);
        let mut e_row = vec![0i8; params.k as usize];
        for &row in &ticket.outer_indices {
            noise.e_row_into(row, &mut e_row);
            let offset = row as usize * params.k as usize;
            a_prime.extend(
                inputs.a[offset..offset + params.k as usize]
                    .iter()
                    .zip(&e_row)
                    .map(|(&a, &e)| (a as i16 + e as i16) as i8),
            );
        }
        let mut b_prime = Vec::with_capacity(ticket.b_cols_global.len() * params.k as usize);
        let mut f_col = vec![0i8; params.k as usize];
        for &col in &ticket.b_cols_global {
            noise.f_col_into(col, &mut f_col);
            let offset = col as usize * params.k as usize;
            b_prime.extend(
                inputs.b[offset..offset + params.k as usize]
                    .iter()
                    .zip(&f_col)
                    .map(|(&b, &f)| (b as i16 + f as i16) as i8),
            );
        }
        let trace = compute_pattern_tile_trace_from_slices(
            &a_prime,
            &b_prime,
            ticket.outer_indices.len(),
            ticket.b_cols_global.len(),
            params.k as usize,
            params.noise_rank as usize,
            params.k as usize,
        );
        let mut target = [0u8; 32];
        target[..28].fill(0xff);
        let public =
            canonical_public_params(&params, 8, 2, 1, commit, extranonce).expect("public params");

        assert_eq!(
            hex::encode(inputs.header.to_bytes()),
            "0403020111111111111111111111111111111111111111111111111111111111111111115dfa1ba6cad1f126ee44599121967ab8382909541fb115f6780c6fcbe3de59eca0887766ffff7f1d"
        );
        assert_eq!(
            hex::encode(inputs.config.to_bytes().expect("config bytes")),
            "00040000400000000007000000000007000000000200010000000000000000000000000000000000000000000000000000000000"
        );
        assert_eq!(
            hex::encode(inputs.commitments.kappa),
            "7b306ed3bb831b1db2a4f12bff99fcaf9b83bad2199ab1ee2c209486f8ab6bf2"
        );
        assert_eq!(
            hex::encode(inputs.commitments.h_a),
            "5322f08c4b6587587e954b7eec68cb210775222a9e8328c781a6f583233c1f07"
        );
        assert_eq!(
            hex::encode(inputs.commitments.h_b),
            "dc79f290caff0d668fe026e509814f45fe13c1af0dee8d45712fa375f958438b"
        );
        assert_eq!(
            hex::encode(ticket.s_a),
            "02b9580688ca4e7f6ba6ff2c1db5ca44ed07f5702e762ae3616046e81547e741"
        );
        assert_eq!(
            hex::encode(ticket.s_b),
            "fdd75fc5ae96f72705f953adf145980386572e32add83e1f043d9cf9171f3fa6"
        );
        assert_eq!(ticket.outer_indices, [0, 2, 4, 6, 8, 10, 12, 14]);
        assert_eq!(ticket.b_cols_global, [0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(
            hex::encode(
                blake3::hash(&a_prime.iter().map(|&value| value as u8).collect::<Vec<_>>())
                    .as_bytes()
            ),
            "a747529f3ed5735a58768b3d10497bc18783964aa209e49d86b0cc6b10b7f9e1"
        );
        assert_eq!(
            hex::encode(
                blake3::hash(&b_prime.iter().map(|&value| value as u8).collect::<Vec<_>>())
                    .as_bytes()
            ),
            "47361b7f13944b8f97cf815eb5c239c6da8d217460f1218a561e4c98e3ebdea7"
        );
        assert_eq!(
            trace.x_steps,
            [
                -28_929, 7_783, 94_310, 129_510, -114_585, 13_084, -18_021, -111_673, 243_592,
                21_367, 68_887, -165_211, -35_760, 4_711, 229_935, 57_423,
            ]
        );
        assert_eq!(
            trace.state,
            ai_pow::matmul::TileState([
                -28_929, 7_783, 94_310, 129_510, -114_585, 13_084, -18_021, -111_673, 243_592,
                21_367, 68_887, -165_211, -35_760, 4_711, 229_935, 57_423,
            ])
        );
        assert_eq!(
            hex::encode(ticket.jackpot_hash),
            "2d4fb8ab46f60e365ac9889e99ae2124e558fa912c74f5b8f712b3921c7b96f0"
        );
        assert_eq!(trace.state, ticket.tile_state);
        assert_eq!(
            hex::encode(public.pearl_adjusted_target().expect("Pearl target")),
            "00000000000000000000000000000000000000000000000000000000ffff7f00"
        );
        assert_eq!(
            hex::encode(
                public
                    .nockchain_adjusted_target(&target)
                    .expect("Nockchain target")
            ),
            "0000ffffffffffffffffffffffffffffffffffffffffffffffffffffffff0000"
        );
        assert_eq!(extranonce, 7);
    }

    #[test]
    fn prepared_canonical_template_matches_scalar_ticket_oracle() {
        let params = canonical_params();
        let commit = [0x42u8; 32];
        let template =
            PreparedCanonicalMoeTemplate::new(&params, 8, 2, 1, commit).expect("template");
        let mut scratch = template.scratch();
        let a_prime_rows = scratch.a_prime_rows.as_ptr();
        let b_prime_cols = scratch.b_prime_cols.as_ptr();

        for extranonce in [0, 1, 7, u32::MAX] {
            let inputs =
                canonical_moe_inputs(&params, 8, 2, 1, commit, extranonce).expect("inputs");
            let scalar = evaluate_canonical_moe_ticket(&params, 8, 2, 1, commit, extranonce)
                .expect("scalar");
            let prepared = template.evaluate(extranonce, &mut scratch);

            assert_eq!(prepared.commitments.kappa, inputs.commitments.kappa);
            assert_eq!(prepared.commitments.h_a, inputs.commitments.h_a);
            assert_eq!(prepared.commitments.h_b, inputs.commitments.h_b);
            assert_eq!(prepared.commitments.s_a, scalar.s_a);
            assert_eq!(prepared.commitments.s_b, scalar.s_b);
            assert_eq!(prepared.tile_state, scalar.tile_state);
            assert_eq!(prepared.jackpot_hash, scalar.jackpot_hash);
        }
        assert_eq!(scratch.a_prime_rows.as_ptr(), a_prime_rows);
        assert_eq!(scratch.b_prime_cols.as_ptr(), b_prime_cols);
        assert_eq!(
            template.header_for(u32::MAX).timestamp,
            CANONICAL_BASE_TIMESTAMP.wrapping_add(u32::MAX)
        );
    }
}
