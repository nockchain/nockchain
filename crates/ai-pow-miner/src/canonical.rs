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

use ai_pow::params::MatmulParams;
use ai_pow::pearl_compat::{
    derive_pearl_work_commitments, pearl_bitcoin_double_sha256_raw, PearlAuxInclusionProof,
    PearlIncompleteBlockHeader, PearlMiningConfig, PearlMoeParams, PearlNockchainAux,
    PearlPeriodicPattern, PearlPublicProofParams, PEARL_MMA_INT7XINT7_TO_INT32,
    PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG,
};
use ai_pow::pearl_moe_routing::build_routing_data;
use ai_pow::synth::{synth_matrices, AI_POW_PROD_SYNTH_SEED};
use ai_pow::zk_bridge::{
    prove_pearl_moe_compact_recursive_certificate_with_seed, PearlMoeCompactProveRun,
};
use ai_pow_zk::recursion::AiPowCompactVerifierSetupSeed;

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

/// The canonical (setup-shaped) block: its prove run plus the pieces needed to
/// assemble its `%ai-pow` artifact noun.
pub struct CanonicalBlock {
    pub run: PearlMoeCompactProveRun,
    pub statement: PearlMergePublicStatementShape,
    pub aux_inclusion: PearlAuxInclusionProof,
    pub moe_art: PearlMergeMoeArtifact,
    pub certificate: AiPowCertificateShape,
    pub commit: [u8; 32],
    pub seed: AiPowCompactVerifierSetupSeed,
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

fn setup_aux_inclusion(
    aux_commitment: &[u8; 32],
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
        timestamp: 0x6677_8899,
        nbits: 0x207f_ffff,
    };
    (
        header,
        PearlAuxInclusionProof {
            coinbase_tx,
            merkle_branch: Vec::new(),
        },
    )
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
    n: usize,
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
    n: usize,
}

fn canonical_moe_schedule(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
) -> Result<CanonicalMoeSchedule, CanonicalProveError> {
    let m = params.m as usize;
    let n = params.n as usize;
    if e == 0 || n % e != 0 {
        return Err(CanonicalProveError(format!("n={n} not divisible by e={e}")));
    }
    let n_e = n / e;
    let config = PearlMiningConfig {
        common_dim: params.k,
        rank: params.noise_rank as u16,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: setup_pattern(hw),
        cols_pattern: setup_pattern(hw),
        reserved: PearlMiningConfig::moe_trailer(e as u16, top_k as u16),
    };
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
        n,
    })
}

fn canonical_moe_inputs(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
    nock_commit: [u8; 32],
) -> Result<CanonicalMoeInputs, CanonicalProveError> {
    let CanonicalMoeSchedule {
        config,
        routing,
        inner,
        local_b,
        n_e,
        m,
        n,
    } = canonical_moe_schedule(params, hw, e, top_k)?;

    let (a, b) = synth_matrices(AI_POW_PROD_SYNTH_SEED, params);
    let aux = setup_aux(nock_commit);
    let aux_commitment = aux.commitment().map_err(err("aux commitment"))?;
    let (header, aux_inclusion) = setup_aux_inclusion(&aux_commitment);
    let mu = config.to_bytes().map_err(err("config bytes"))?;
    let commitments = derive_pearl_work_commitments(&header.to_bytes(), &mu, &a, &b);

    Ok(CanonicalMoeInputs {
        a,
        b,
        commitments,
        routing,
        inner,
        local_b,
        n_e,
        m,
        n,
        config,
        header,
        aux,
        aux_commitment,
        aux_inclusion,
    })
}

/// Prove a single canonical MoE block at the given shape, bound to `nock_commit`
/// (the node's block commitment). `hw` is the opened-tile side; `e`/`top_k` the
/// MoE config. Deterministic given `nock_commit`; ~25-30s on CPU for the small
/// shape. Returns errors (panics-free).
pub fn prove_canonical_moe_block(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
    nock_commit: [u8; 32],
) -> Result<CanonicalBlock, CanonicalProveError> {
    let CanonicalMoeInputs {
        a,
        b,
        commitments,
        routing,
        inner,
        local_b,
        n_e,
        m,
        n,
        config,
        header,
        aux,
        aux_commitment,
        aux_inclusion,
    } = canonical_moe_inputs(params, hw, e, top_k, nock_commit)?;

    let (run, seed) = prove_pearl_moe_compact_recursive_certificate_with_seed(
        params,
        &a,
        &b,
        &commitments.kappa,
        &commitments.h_a,
        &commitments.h_b,
        &routing,
        0,
        &inner,
        &local_b,
        n_e,
    )
    .map_err(err("prove"))?;

    let public = PearlPublicProofParams {
        block_header: header,
        mining_config: config,
        hash_a: commitments.h_a,
        hash_b: commitments.h_b,
        hash_jackpot: run.ticket.jackpot_hash,
        m: m as u32,
        n: n as u32,
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
        ai_pow_zk::recursion::encode_compact_batch_recursive_certificate(&run.compact_cert)
            .map_err(err("encode cert"))?;
    let certificate = AiPowCertificateShape {
        version: 1,
        zk_params: run.zk_params,
        found_idx: 0,
        trace_height: run.trace_height,
        commitments: run.commitments,
        public_inputs: run.pis.clone(),
        certificate: AiProofNode::Bytes(cert_bytes),
    };
    let moe_art = PearlMergeMoeArtifact {
        moe: PearlMoeParams {
            expert_idx: 0,
            routing_offsets: routing.routing_offsets.clone(),
            hash_routing: run.ticket.commitment.routing_root,
            outer_indices: run.ticket.outer_indices.clone(),
        },
        routing_data: routing.routing_data.clone(),
    };

    Ok(CanonicalBlock {
        run,
        statement,
        aux_inclusion,
        moe_art,
        certificate,
        commit: nock_commit,
        seed,
    })
}
