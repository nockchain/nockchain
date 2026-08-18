//! Boot-time compact verifier-setup builder.
//!
//! The compact verifier `context` + `verifier_key_digest` are deterministic from
//! the puzzle SHAPE (params / trace-height) and **proof-independent** (validated in
//! `ai-pow-miner::moe_compact_verifier_setup_is_proof_independent`). So a consensus
//! node builds them ONCE at boot by proving a single canonical block, then injects
//! the result via [`crate::init_ai_pow_verifier_setup`] and reuses it to verify
//! every same-shape `%ai-pow` block. The per-block opened schedule is bound
//! separately by the program-commitment fold — not by the setup — so one
//! setup serves all blocks of the shape (dense and MoE alike).

use std::io::Write;

use ai_pow::params::MatmulParams;
use ai_pow::pearl_compat::{
    derive_pearl_moe_work_commitments, evaluate_pearl_merge_ticket_attempt,
    pearl_bitcoin_double_sha256_raw, PearlAuxInclusionProof, PearlIncompleteBlockHeader,
    PearlMiningConfig, PearlMoeParams, PearlNockchainAux, PearlPeriodicPattern,
    PearlPublicProofParams, PEARL_MMA_INT7XINT7_TO_INT32, PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG,
};
use ai_pow::pearl_moe_routing::build_routing_data;
use ai_pow::synth::{synth_matrices, AI_POW_PROD_SYNTH_SEED};
use ai_pow::zk_bridge::{
    prove_pearl_merge_compact_recursive_certificate_with_seed,
    prove_pearl_moe_compact_recursive_certificate_with_seed, AiPowCompactRecursiveCertificateRun,
    PearlMoeCompactProveRun,
};
use ai_pow_miner::certificate_noun::{
    AiPowCertificateShape, AiProofNode, PearlMergeMoeArtifact, PearlMergePublicStatementShape,
};
use ai_pow_zk::recursion::AiPowCompactVerifierSetupSeed;

use crate::{AiPowVerifierSetup, VerifierSetupShapeKey};
/// Error building the canonical verifier setup.
#[derive(Debug)]
pub struct SetupError(pub String);

impl std::fmt::Display for SetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ai-pow verifier setup: {}", self.0)
    }
}
impl std::error::Error for SetupError {}

fn err<E: std::fmt::Debug>(what: &str) -> impl FnOnce(E) -> SetupError + '_ {
    move |e| SetupError(format!("{what}: {e:?}"))
}

fn shape_key_for_zk_params(
    zk_params: &ai_pow_zk::ZkParams,
    trace_height: usize,
) -> Result<VerifierSetupShapeKey, SetupError> {
    VerifierSetupShapeKey::from_zk_params(zk_params, trace_height)
        .ok_or_else(|| SetupError("verifier setup shape has zero noise_rank".to_string()))
}

fn shape_key_for_seed(
    seed: &AiPowCompactVerifierSetupSeed,
) -> Result<VerifierSetupShapeKey, SetupError> {
    shape_key_for_zk_params(&seed.zk_params, seed.trace_height())
}

fn shape_key_for_params(
    params: &MatmulParams,
    trace_height: usize,
) -> Result<VerifierSetupShapeKey, SetupError> {
    if params.noise_rank == 0 {
        return Err(SetupError(
            "verifier setup shape has zero noise_rank".to_string(),
        ));
    }
    let num_stripes = params.k / params.noise_rank;
    Ok(VerifierSetupShapeKey::new(
        trace_height,
        (num_stripes as usize) <= ai_pow::params::STRIPE_MAX,
    ))
}
/// An arbitrary fixed commitment for the canonical setup block. The setup is
/// proof-independent, so the specific block does not matter.
pub const CANONICAL_SETUP_COMMIT: [u8; 32] = [0x42u8; 32];

/// The canonical (setup) block: its prove run plus the pieces needed to assemble
/// its artifact noun (used by tests to exercise the jet against this exact block).
pub struct CanonicalBlock {
    pub run: PearlMoeCompactProveRun,
    pub statement: PearlMergePublicStatementShape,
    pub aux_inclusion: PearlAuxInclusionProof,
    pub moe_art: PearlMergeMoeArtifact,
    pub certificate: AiPowCertificateShape,
    pub commit: [u8; 32],
    /// The SMALL, serializable rebuild seed for this block's trace-height bucket —
    /// the cacheable boot-setup input (see [`build_verifier_setup`]).
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
        // Shared with the miner rather than copied: this header feeds `sigma`
        // and therefore the whole work transcript, and a base target the accept
        // path cannot scale by the tile shape factor makes the block
        // unverifiable. See `ai_pow_miner::canonical::CANONICAL_NBITS`.
        nbits: ai_pow_miner::canonical::CANONICAL_NBITS,
    };
    (
        header,
        PearlAuxInclusionProof {
            coinbase_tx,
            merkle_branch: Vec::new(),
        },
    )
}

/// The canonical MoE-block inputs derived from a `(params, hw, e, top_k)` shape —
/// the synthesized matrices, work commitments, routing, opened tile indices, and
/// the block-statement scaffolding. Shared by [`prove_canonical_moe_block`] (which
/// then proves + assembles) and [`canonical_moe_trace_height`] (which only needs
/// the prove-inputs to predict the trace height), so the two can never disagree
/// about which bucket a shape lands in.
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

/// The matrix-FREE part of the canonical MoE inputs: the mining config, routing,
/// and opened tile indices — everything the trace height depends on. Kept separate
/// from the (large) synthesized matrices so [`canonical_moe_trace_height`] can sweep
/// candidate shapes cheaply, while [`canonical_moe_inputs`] adds the matrices +
/// commitments for the actual prove. Both derive the schedule identically.
struct CanonicalMoeSchedule {
    config: PearlMiningConfig,
    routing: ai_pow::pearl_moe_routing::RoutingData,
    inner: Vec<u32>,
    local_b: Vec<u32>,
    n_e: usize,
    m: usize,
}

fn canonical_moe_schedule(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
) -> Result<CanonicalMoeSchedule, SetupError> {
    let m = params.m as usize;
    let n = params.n as usize;
    if e == 0 || !n.is_multiple_of(e) {
        return Err(SetupError(format!("n={n} not divisible by e={e}")));
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
    })
}

fn canonical_moe_inputs(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
    nock_commit: [u8; 32],
) -> Result<CanonicalMoeInputs, SetupError> {
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
    let (header, aux_inclusion) = setup_aux_inclusion(&aux_commitment);
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

/// The Layer-0 trace height a canonical MoE block at `(params, hw, e, top_k)` would
/// have — WITHOUT proving AND without synthesizing the (large) matrices. Lets
/// [`production_verifier_setup_buckets`] cheaply select one shape per trace-height
/// bucket. Equal to the height the full prove yields.
pub fn canonical_moe_trace_height(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
) -> Result<usize, SetupError> {
    let s = canonical_moe_schedule(params, hw, e, top_k)?;
    ai_pow::zk_bridge::pearl_moe_canonical_trace_height(
        params, &s.routing, 0, &s.inner, &s.local_b, s.n_e,
    )
    .map_err(err("moe canonical trace height"))
}

/// Prove a single canonical MoE block at the given shape. `hw` is the opened-tile
/// side (`h = w = hw`); `e`/`top_k` the MoE config. Panics-free (returns errors).
pub fn prove_canonical_moe_block(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
    nock_commit: [u8; 32],
) -> Result<CanonicalBlock, SetupError> {
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
    } = canonical_moe_inputs(params, hw, e, top_k, nock_commit)?;

    let (run, seed) = prove_pearl_moe_compact_recursive_certificate_with_seed(
        params, &a, &b, &commitments.kappa, &commitments.h_a, &commitments.h_b, &routing, 0,
        &inner, &local_b, n_e,
    )
    .map_err(err("prove"))?;

    let public = PearlPublicProofParams {
        block_header: header,
        mining_config: config,
        hash_a: commitments.h_a,
        hash_b: commitments.h_b,
        hash_jackpot: run.ticket.jackpot_hash,
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

/// Build the boot verifier setup by proving one canonical block at the production
/// shape. Call once at node boot, then [`crate::init_ai_pow_verifier_setup`].
pub fn build_verifier_setup(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
) -> Result<AiPowVerifierSetup, SetupError> {
    let block = prove_canonical_moe_block(params, hw, e, top_k, CANONICAL_SETUP_COMMIT)?;
    let trace_height = block.run.trace_height;
    let digest_bytes = ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(
        &block.run.verifier_key_digest(),
    )
    .to_vec();
    let shape_key = shape_key_for_params(params, trace_height)?;
    Ok(AiPowVerifierSetup {
        trace_height,
        sx_bound: shape_key.sx_bound,
        context: block.run.verifier_context,
        digest_bytes,
    })
}

/// Build ONLY the small, cacheable rebuild seed for the boot verifier setup, by
/// proving one canonical MoE block at the given shape. The offline/boot table
/// builder calls this per trace-height bucket and serializes the seeds; the large
/// (~866 MB) verifier context that proving also produces is dropped here and
/// rebuilt at boot from the seed (see [`rebuild_verifier_setup_from_seed`]). This
/// is the size-practical form: a seed is KB-MB, so a full bucket table caches in
/// tens of MB rather than gigabytes.
pub fn build_verifier_setup_seed(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
) -> Result<AiPowCompactVerifierSetupSeed, SetupError> {
    Ok(prove_canonical_moe_block(params, hw, e, top_k, CANONICAL_SETUP_COMMIT)?.seed)
}
/// Build ONLY the small, cacheable rebuild seed for a DENSE boot verifier setup, by
/// proving one canonical dense block at the given shape. The dense counterpart of
/// [`build_verifier_setup_seed`]. Used for the `(2^13, false)` bucket that no MoE
/// shape reaches — the MoE routing scatters opened A rows, inflating the Layer-0
/// trace height above the dense budget for the same `(params, tile)`.
pub fn build_verifier_setup_seed_dense(
    params: &MatmulParams,
) -> Result<AiPowCompactVerifierSetupSeed, SetupError> {
    Ok(prove_canonical_dense_block(params, CANONICAL_SETUP_COMMIT)?.seed)
}

/// Prove a single canonical DENSE block at the given shape. Builds a
/// Pearl-compatible dense ticket attempt (synth matrices, aux, header), grinds
/// the aux height until the jackpot clears the max consensus target, then
/// proves the compact recursive certificate with seed capture. The dense
/// counterpart of [`prove_canonical_moe_block`].
fn prove_canonical_dense_block(
    params: &MatmulParams,
    nock_commit: [u8; 32],
) -> Result<CanonicalDenseBlock, SetupError> {
    let (a, b) = synth_matrices(AI_POW_PROD_SYNTH_SEED, params);
    let config = PearlMiningConfig {
        common_dim: params.k,
        rank: params.noise_rank as u16,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: setup_pattern(params.tile),
        cols_pattern: setup_pattern(params.tile),
        reserved: [0u8; ai_pow::pearl_compat::PEARL_MINING_CONFIG_RESERVED_SIZE],
    };
    let target = ai_pow::difficulty::AI_POW_MAX_CONSENSUS_TARGET;
    let factor = ai_pow::difficulty::shape_work_factor_for(
        params.tile, params.tile, params.k, params.noise_rank,
    )
    .map_err(err("dense shape work factor"))?;

    let max_pattern_len = crate::AI_POW_VERIFY_MAX_PATTERN_LEN;
    // Grind the aux height until the jackpot clears the max consensus target.
    // The max target scaled by the shape work factor still rejects ~99.6% of
    // jackpots (the adjusted threshold is ~2^248 for this shape), so a few
    // hundred attempts suffice.
    let mut attempt = None;
    for n in 0..100_000 {
        let aux = PearlNockchainAux {
            nockchain_chain_id: b"nockchain-mainnet\0".to_vec(),
            nock_block_commitment: nock_commit,
            nockchain_target_epoch_or_height: 123_456 + n,
            extra_domain_data: b"ai-pow-target-window\0\0".to_vec(),
        };
        let aux_commitment = aux.commitment().map_err(err("aux commitment"))?;
        let (header, _aux_inclusion) = setup_aux_inclusion(&aux_commitment);
        let ticket_attempt = evaluate_pearl_merge_ticket_attempt(
            &header, &config, params, 0, 0, &a, &b, &target, max_pattern_len, aux,
        )
        .map_err(err("evaluate dense ticket"))?;
        if ai_pow::difficulty::attempt_wins(&ticket_attempt.ticket.jackpot_hash, &target, factor)
            .map_err(err("attempt_wins"))?
        {
            attempt = Some(ticket_attempt);
            break;
        }
    }
    let attempt = attempt.ok_or_else(|| {
        SetupError(
            "dense canonical block: no jackpot cleared the target in 100k attempts".to_string(),
        )
    })?;

    let (run, seed) = prove_pearl_merge_compact_recursive_certificate_with_seed(
        &attempt, params, &a, &b, max_pattern_len,
    )
    .map_err(err("prove dense compact certificate with seed"))?;

    Ok(CanonicalDenseBlock { run, seed })
}
/// The canonical dense setup block: its prove run plus the boot-setup seed.
#[allow(dead_code)]
struct CanonicalDenseBlock {
    run: AiPowCompactRecursiveCertificateRun,
    seed: AiPowCompactVerifierSetupSeed,
}

/// Rebuild the full boot verifier setup from a cached seed WITHOUT proving — the
/// boot-time counterpart of [`build_verifier_setup_seed`]. Rebuilds the compact
/// verifier context (circuit compile + Merkle commit; seconds, no FRI proving) and
/// pairs it with the trace height + the cached verifier-key digest. The result is
/// byte-for-byte equivalent to the [`build_verifier_setup`] (direct-context) form,
/// validated in `moe_verifier_setup_seed_roundtrip_rebuilds_working_setup`.
pub fn rebuild_verifier_setup_from_seed(
    seed: AiPowCompactVerifierSetupSeed,
) -> Result<AiPowVerifierSetup, SetupError> {
    let shape_key = shape_key_for_seed(&seed)?;
    let digest_bytes = seed.verifier_key_digest_bytes.clone();
    let context = seed
        .rebuild_context()
        .map_err(err("rebuild verifier context from seed"))?;
    Ok(AiPowVerifierSetup {
        trace_height: shape_key.trace_height,
        sx_bound: shape_key.sx_bound,
        context,
        digest_bytes,
    })
}

/// The local seed-cache encoding. The consensus fingerprint is checked separately
/// after decoding; this version protects the cache's non-consensus bincode framing.
const VERIFIER_SETUP_SEED_CACHE_MAGIC: &[u8; 8] = b"NCVPSEED";
/// Bump this when the seed-table bincode encoding or configuration changes.
const VERIFIER_SETUP_SEED_CACHE_FORMAT_VERSION: u32 = 1;
const VERIFIER_SETUP_SEED_CACHE_HEADER_LEN: usize = VERIFIER_SETUP_SEED_CACHE_MAGIC.len()
    + std::mem::size_of::<u32>()
    + std::mem::size_of::<u64>()
    + 32;

/// The cache path is versioned so a changed seed serialization never destroys an
/// older cache before a replacement has been generated successfully.
pub fn verifier_setup_seed_cache_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("ai-pow").join(format!(
        "verifier-setup-seeds-v{VERIFIER_SETUP_SEED_CACHE_FORMAT_VERSION}.bin"
    ))
}

fn write_seed_cache_atomically(path: &std::path::Path, bytes: &[u8]) -> Result<(), SetupError> {
    let parent = path.parent().ok_or_else(|| {
        SetupError(format!(
            "verifier-setup cache has no parent: {}",
            path.display()
        ))
    })?;
    std::fs::create_dir_all(parent).map_err(err("create verifier-setup cache dir"))?;

    let file_name = path.file_name().ok_or_else(|| {
        SetupError(format!(
            "verifier-setup cache has no file name: {}",
            path.display()
        ))
    })?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(err("read verifier-setup cache clock"))?
        .as_nanos();
    let mut temp_name = file_name.to_os_string();
    temp_name.push(format!(".{}.{}.tmp", std::process::id(), nonce));
    let temp_path = parent.join(temp_name);

    let result = (|| -> Result<(), SetupError> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(err("create temporary verifier-setup cache"))?;
        file.write_all(bytes)
            .map_err(err("write temporary verifier-setup cache"))?;
        file.sync_all()
            .map_err(err("sync temporary verifier-setup cache"))?;
        std::fs::rename(&temp_path, path).map_err(err("replace verifier-setup cache"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

/// Serialize a seed table to `path` with a versioned, checksummed envelope. The
/// cached artifact is small — the seeds (KB-MB/bucket), NOT the rebuilt ~866 MB
/// contexts.
pub fn save_verifier_setup_seeds(
    path: &std::path::Path,
    seeds: &[AiPowCompactVerifierSetupSeed],
) -> Result<(), SetupError> {
    let payload = bincode::serde::encode_to_vec(seeds, bincode::config::standard())
        .map_err(err("serialize verifier-setup seeds"))?;
    let payload_len = u64::try_from(payload.len())
        .map_err(|_| SetupError("verifier-setup cache payload exceeds u64".to_string()))?;
    let checksum = blake3::hash(&payload);
    let mut bytes = Vec::with_capacity(VERIFIER_SETUP_SEED_CACHE_HEADER_LEN + payload.len());
    bytes.extend_from_slice(VERIFIER_SETUP_SEED_CACHE_MAGIC);
    bytes.extend_from_slice(&VERIFIER_SETUP_SEED_CACHE_FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&payload_len.to_le_bytes());
    bytes.extend_from_slice(checksum.as_bytes());
    bytes.extend_from_slice(&payload);
    write_seed_cache_atomically(path, &bytes)
}

/// Load a versioned seed table from `path` — the inverse of
/// [`save_verifier_setup_seeds`].
pub fn load_verifier_setup_seeds(
    path: &std::path::Path,
) -> Result<Vec<AiPowCompactVerifierSetupSeed>, SetupError> {
    let bytes = std::fs::read(path).map_err(err("read verifier-setup cache"))?;
    if bytes.len() < VERIFIER_SETUP_SEED_CACHE_HEADER_LEN {
        return Err(SetupError(format!(
            "verifier-setup cache at {} is shorter than its header",
            path.display()
        )));
    }

    let mut offset = 0usize;
    let magic = &bytes[offset..offset + VERIFIER_SETUP_SEED_CACHE_MAGIC.len()];
    if magic != VERIFIER_SETUP_SEED_CACHE_MAGIC {
        return Err(SetupError(format!(
            "verifier-setup cache at {} has an unsupported format marker",
            path.display()
        )));
    }
    offset += VERIFIER_SETUP_SEED_CACHE_MAGIC.len();

    let version = u32::from_le_bytes(
        bytes[offset..offset + std::mem::size_of::<u32>()]
            .try_into()
            .expect("cache header length checked"),
    );
    offset += std::mem::size_of::<u32>();
    if version != VERIFIER_SETUP_SEED_CACHE_FORMAT_VERSION {
        return Err(SetupError(format!(
            "verifier-setup cache at {} has unsupported format version {version}",
            path.display()
        )));
    }

    let payload_len = usize::try_from(u64::from_le_bytes(
        bytes[offset..offset + std::mem::size_of::<u64>()]
            .try_into()
            .expect("cache header length checked"),
    ))
    .map_err(|_| {
        SetupError("verifier-setup cache payload length does not fit usize".to_string())
    })?;
    offset += std::mem::size_of::<u64>();

    let expected_checksum: [u8; 32] = bytes[offset..offset + 32]
        .try_into()
        .expect("cache header length checked");
    offset += 32;

    let payload_end = offset.checked_add(payload_len).ok_or_else(|| {
        SetupError("verifier-setup cache payload length overflows usize".to_string())
    })?;
    if payload_end != bytes.len() {
        return Err(SetupError(format!(
            "verifier-setup cache at {} has an invalid payload length",
            path.display()
        )));
    }
    let payload = &bytes[offset..payload_end];
    if *blake3::hash(payload).as_bytes() != expected_checksum {
        return Err(SetupError(format!(
            "verifier-setup cache at {} failed its payload checksum",
            path.display()
        )));
    }

    let (seeds, consumed) = bincode::serde::decode_from_slice(payload, bincode::config::standard())
        .map_err(err("deserialize verifier-setup seeds"))?;
    if consumed != payload.len() {
        return Err(SetupError(format!(
            "verifier-setup cache at {} has trailing seed bytes",
            path.display()
        )));
    }
    Ok(seeds)
}

/// Load the cached seed table and REBUILD each seed into a full verifier setup
/// (circuit compile + Merkle commit; NO FRI proving). This is the boot-time path:
/// seconds per bucket, not the ~2 min of proving each would otherwise cost. The
/// resulting table is ready for [`crate::init_ai_pow_verifier_setup`].
pub fn load_verifier_setup_table(
    path: &std::path::Path,
) -> Result<Vec<AiPowVerifierSetup>, SetupError> {
    load_verifier_setup_seeds(path)?
        .into_iter()
        .map(rebuild_verifier_setup_from_seed)
        .collect()
}

/// Env var to override the resident-context LRU cap (max heavy contexts kept in
/// memory at once). See [`verifier_cache_cap`].
pub const AI_POW_VERIFIER_CACHE_CAP_ENV: &str = "AI_POW_VERIFIER_CACHE_CAP";

/// Default resident-context LRU cap. The production default retains every supported
/// setup shape, so remote inputs cannot create an evict/reload loop on the consensus
/// thread unless an operator deliberately lowers the cap.
pub const AI_POW_VERIFIER_CACHE_CAP_DEFAULT: usize = 14;

/// Resolve the resident-context LRU cap from `AI_POW_VERIFIER_CACHE_CAP` (clamped to
/// `>= 1`), else the DoS-safe all-shape default.
pub fn verifier_cache_cap() -> usize {
    std::env::var(AI_POW_VERIFIER_CACHE_CAP_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .map(|c| c.max(1))
        .unwrap_or(AI_POW_VERIFIER_CACHE_CAP_DEFAULT)
}

/// Load the cached SEEDS and validate them against the committed **v0** consensus
/// digest WITHOUT rebuilding (lazy boot). The cache envelope rejects corruption and
/// serialization mismatches before bincode decoding; the consensus digest then
/// rejects a decoded table with divergent verifier keys.
fn load_and_validate_seeds(
    path: &std::path::Path,
) -> Result<Vec<AiPowCompactVerifierSetupSeed>, SetupError> {
    let seeds = load_verifier_setup_seeds(path)?;
    crate::table_digest::verify_verifier_setup_seed_table_digest(&seeds)?;
    Ok(seeds)
}

/// BOOT installer: ensure the AI-PoW verifier-setup table is installed (LAZILY), or
/// FAIL.
///
/// The verifier-setup table is a CONSENSUS PARAMETER — every node must verify
/// `%ai-pow` blocks against byte-identical verifier keys. This installer pins that:
/// the SEEDS it ends up with (loaded or freshly generated) must hash to the committed
/// [`crate::table_digest::AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST`], or the node refuses
/// to run. It builds every bucket's context to disk AT THE OUTSET (first boot;
/// reused after) and injects them disk-paged (see
/// [`crate::init_ai_pow_verifier_setup_disk`]), so a verify never rebuilds — at most
/// a ~0.6 s page-in from disk — and standing RSS is a bounded working set.
///
/// - **Cache present and valid:** load seeds + validate digest + build/reuse contexts.
/// - **Cache present but corrupt / format-incompatible / digest-mismatched:** retain
///   it until a complete replacement is atomically written, rather than deleting
///   the only diagnostic artifact before regeneration.
/// - **Cache absent:** GENERATE it (one real compact proof per `buckets` entry — a
///   one-time ~15-minute boot delay), cache it, then load + validate + inject.
///
/// Returns the number of buckets installed. **Any failure is `Err` and is FATAL** —
/// the caller must shut the node down. A digest mismatch on a FRESHLY-GENERATED cache
/// is fatal and NOT retried. Idempotent (a second in-process boot returns `Ok(0)`).
pub fn install_or_build_verifier_setup(
    data_dir: &std::path::Path,
    buckets: &[VerifierSetupBucketShape],
) -> Result<usize, SetupError> {
    if crate::ai_pow_verifier_setup_initialized() {
        return Ok(0);
    }
    let path = verifier_setup_seed_cache_path(data_dir);

    // Fast path: a present, loadable, digest-matching seed cache (no rebuild).
    let mut seeds: Option<Vec<AiPowCompactVerifierSetupSeed>> = None;
    if path.exists() {
        match load_and_validate_seeds(&path) {
            Ok(s) => seeds = Some(s),
            Err(e) => {
                tracing::warn!(
                    "AI-PoW verifier-setup cache at {} is unusable ({e}); \
                     regenerating an atomic replacement",
                    path.display(),
                );
            }
        }
    }

    let seeds = match seeds {
        Some(s) => s,
        None => {
            if buckets.is_empty() {
                return Err(SetupError(
                    "no usable verifier-setup cache and no bucket shapes to generate one from"
                        .to_string(),
                ));
            }
            tracing::info!(
                "Generating the AI-PoW verifier-setup table ({} buckets). This is a one-time \
                 step and takes about 15 minutes; the result is cached, so subsequent boots are \
                 fast.",
                buckets.len(),
            );
            build_and_cache_verifier_setup_seeds(&path, buckets)?;
            // A digest mismatch HERE is fatal and NOT retried.
            load_and_validate_seeds(&path)?
        }
    };

    if seeds.is_empty() {
        return Err(SetupError(
            "verifier-setup seed table is empty after build/load".to_string(),
        ));
    }

    // Build (or reuse) every bucket's on-disk context AT THE OUTSET: first boot builds
    // all of them (a one-time cost); subsequent boots find the files and skip straight
    // to disk-paged residency. Because all contexts exist before any block is verified,
    // a verify NEVER triggers a ~12 s rebuild — at most a ~0.6 s page-in from disk.
    let disk_buckets = build_or_reuse_disk_contexts(data_dir, seeds)?;
    let n = disk_buckets.len();
    let cap = verifier_cache_cap();
    crate::init_ai_pow_verifier_setup_disk(disk_buckets, cap).map_err(|()| {
        SetupError(
            "verifier-setup table rejected (empty / duplicate buckets) or already initialized"
                .to_string(),
        )
    })?;
    tracing::info!(
        "AI-PoW verifier-setup installed (disk-paged): {n} bucket(s); contexts paged from disk, \
         up to {cap} resident at once.",
    );
    Ok(n)
}

/// The on-disk file for one bucket's serialized verifier context. The shape key and
/// committed verifier-key digest are baked into the filename so a seed/table change
/// yields a new filename and a stale file is never mistaken for the current one.
pub fn verifier_context_file_path(
    data_dir: &std::path::Path,
    shape_key: VerifierSetupShapeKey,
    committed_digest: &[u8],
) -> std::path::PathBuf {
    let log2 = (shape_key.trace_height as u64).trailing_zeros();
    let sx = if shape_key.sx_bound { "sx" } else { "rb" };
    let tag: String = committed_digest
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect();
    data_dir
        .join("ai-pow")
        .join(format!("ctx-2p{log2}-{sx}-{tag}.bin"))
}

/// The sidecar file holding the BLAKE3 checksum of a context file's bytes.
fn context_checksum_path(context_path: &std::path::Path) -> std::path::PathBuf {
    let mut p = context_path.as_os_str().to_owned();
    p.push(".blake3");
    std::path::PathBuf::from(p)
}

/// Serialize one built verifier setup to `path` for disk paging, and write a sidecar
/// BLAKE3 of the file bytes (re-checked on every page-in to catch on-disk bit-rot).
/// Returns the checksum.
fn write_verifier_context_file(
    setup: &AiPowVerifierSetup,
    path: &std::path::Path,
) -> Result<[u8; 32], SetupError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(err("create verifier-context dir"))?;
    }
    let bytes = bincode::serde::encode_to_vec(setup, bincode::config::standard())
        .map_err(err("serialize verifier context"))?;
    let checksum = *blake3::hash(&bytes).as_bytes();
    std::fs::write(path, bytes).map_err(err("write verifier context file"))?;
    std::fs::write(context_checksum_path(path), checksum)
        .map_err(err("write verifier context checksum"))?;
    Ok(checksum)
}

/// The expected checksum for an existing context file: read the 32-byte sidecar if
/// present, else compute it from the file bytes and write the sidecar (self-heal for
/// files written before checksums, or a lost sidecar). This never trusts the file — it
/// only records what is on disk; `page_in_bucket` re-reads + re-checks on every use.
fn context_file_checksum(path: &std::path::Path) -> Result<[u8; 32], SetupError> {
    let sidecar = context_checksum_path(path);
    if let Ok(bytes) = std::fs::read(&sidecar) {
        if let Ok(arr) = <[u8; 32]>::try_from(bytes.as_slice()) {
            return Ok(arr);
        }
    }
    let file = std::fs::read(path).map_err(err("read verifier context for checksum"))?;
    let checksum = *blake3::hash(&file).as_bytes();
    // Best-effort sidecar write (ignore failure: page_in will recompute if absent).
    let _ = std::fs::write(&sidecar, checksum);
    Ok(checksum)
}

/// Ensure every bucket has a valid on-disk context, building + serializing any that
/// are missing (consuming `seeds`), and return the disk buckets for injection. A
/// freshly-built context is digest-validated against its seed's committed digest
/// before it is written; a mismatch is fatal (a divergent verifier must not run).
fn build_or_reuse_disk_contexts(
    data_dir: &std::path::Path,
    seeds: Vec<AiPowCompactVerifierSetupSeed>,
) -> Result<Vec<crate::DiskBucket>, SetupError> {
    let mut disk_buckets: Vec<crate::DiskBucket> = Vec::with_capacity(seeds.len());
    let mut built = 0usize;
    let total = seeds.len();
    for seed in seeds {
        let shape_key = shape_key_for_seed(&seed)?;
        let committed_digest = seed.verifier_key_digest_bytes.clone();
        let ctx_path = verifier_context_file_path(data_dir, shape_key, &committed_digest);
        let checksum = if ctx_path.exists() {
            // Reuse: record the existing file's checksum (page_in re-verifies on use).
            context_file_checksum(&ctx_path)?
        } else {
            if built == 0 {
                tracing::info!(
                    "Building the AI-PoW verifier contexts to disk ({total} buckets, one-time; \
                     ~1–2 minutes). They are paged in from disk afterwards, never rebuilt.",
                );
            }
            let setup = rebuild_verifier_setup_from_seed(seed)?;
            // Digest gate: the built context must internally bind its setup metadata
            // and match its seed's committed digest.
            let recomputed_digest = setup.context.validate_setup_binding().map_err(|e| {
                SetupError(format!(
                    "built verifier context for {:?} failed setup binding validation: {e:?}",
                    shape_key,
                ))
            })?;
            let digest = ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(
                &recomputed_digest,
            );
            if digest.as_slice() != committed_digest.as_slice()
                || setup.digest_bytes != committed_digest
            {
                return Err(SetupError(format!(
                    "built verifier context for {:?} does not match its committed digest — \
                     refusing to run a divergent verifier",
                    shape_key,
                )));
            }
            let ck = write_verifier_context_file(&setup, &ctx_path)?;
            built += 1;
            ck
        };
        disk_buckets.push(crate::DiskBucket::new(
            shape_key, committed_digest, ctx_path, checksum,
        ));
    }
    if built > 0 {
        tracing::info!("Built {built} AI-PoW verifier context(s) to disk.");
    }
    Ok(disk_buckets)
}

/// Test / bootstrap helper: serialize already-built `setups` to per-bucket context
/// files under `data_dir` and inject them disk-paged, mirroring what the boot
/// installer does after building — without generating or loading a seed cache. Each
/// setup's own `digest_bytes` is the committed digest (re-checked on page-in).
pub fn install_verifier_setup_disk_from_setups(
    setups: Vec<AiPowVerifierSetup>,
    data_dir: &std::path::Path,
    cap: usize,
) -> Result<(), SetupError> {
    let mut disk_buckets: Vec<crate::DiskBucket> = Vec::with_capacity(setups.len());
    for setup in &setups {
        let shape_key = setup.shape_key();
        let recomputed_digest = setup.context.validate_setup_binding().map_err(|e| {
            SetupError(format!(
                "verifier context for {:?} failed setup binding validation: {e:?}",
                shape_key,
            ))
        })?;
        let digest =
            ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(&recomputed_digest)
                .to_vec();
        if setup.digest_bytes != digest {
            return Err(SetupError(format!(
                "verifier context for {:?} does not match its committed digest",
                shape_key,
            )));
        }
        let ctx_path = verifier_context_file_path(data_dir, shape_key, &digest);
        let checksum = write_verifier_context_file(setup, &ctx_path)?;
        disk_buckets.push(crate::DiskBucket::new(
            shape_key, digest, ctx_path, checksum,
        ));
    }
    crate::init_ai_pow_verifier_setup_disk(disk_buckets, cap)
        .map_err(|()| SetupError("disk-paged setup rejected or already initialized".to_string()))
}

/// Lenient loader for non-consensus tools (e.g. roswell): if the data-dir cache
/// exists, load + rebuild (no proving) + inject and return the bucket count;
/// otherwise return `Ok(0)` WITHOUT generating one. Unlike
/// [`install_or_build_verifier_setup`], this never proves at boot (so it never
/// stalls a tool/test harness) and never shuts down on a missing cache — it only
/// errors on a corrupt cache or rebuild failure. Idempotent.
pub fn install_verifier_setup_from_cache(data_dir: &std::path::Path) -> Result<usize, SetupError> {
    if crate::ai_pow_verifier_setup_initialized() {
        return Ok(0);
    }
    let path = verifier_setup_seed_cache_path(data_dir);
    if !path.exists() {
        return Ok(0);
    }
    // Lenient: register the disk-bucket paths WITHOUT building the heavy contexts and
    // WITHOUT the consensus-digest gate. A non-consensus tool never verifies a real
    // block, so it never pages a context in; if it did, page-in re-checks the digest
    // and a missing/wrong file fails safe.
    let seeds = load_verifier_setup_seeds(&path)?;
    let n = seeds.len();
    let disk_buckets: Vec<crate::DiskBucket> = seeds
        .iter()
        .map(|seed| {
            let shape_key = shape_key_for_seed(seed)?;
            let digest = seed.verifier_key_digest_bytes.clone();
            let ctx_path = verifier_context_file_path(data_dir, shape_key, &digest);
            // Register-only (never paged in by a non-consensus tool): read the sidecar
            // checksum if a node already built the file, else a zero placeholder.
            let checksum = context_file_checksum(&ctx_path).unwrap_or([0u8; 32]);
            Ok(crate::DiskBucket::new(
                shape_key, digest, ctx_path, checksum,
            ))
        })
        .collect::<Result<Vec<_>, SetupError>>()?;
    crate::init_ai_pow_verifier_setup_disk(disk_buckets, verifier_cache_cap()).map_err(|()| {
        SetupError(
            "verifier-setup table rejected (empty / duplicate buckets) or already initialized"
                .to_string(),
        )
    })?;
    Ok(n)
}

/// One production verifier setup shape: the puzzle shape that lands in it. The boot
/// table has one entry per reachable `(trace_height, sx_bound)` key.
#[derive(Clone, Copy, Debug)]
pub struct VerifierSetupBucketShape {
    pub params: MatmulParams,
    pub hw: u32,
    pub e: usize,
    pub top_k: usize,
    /// When `true`, the bucket is built by proving a DENSE canonical block (not MoE).
    /// `hw`/`e`/`top_k` are ignored. Dense buckets cover verifier shapes that no MoE
    /// shape reaches — the MoE routing scatters opened rows, inflating the Layer-0
    /// trace height above the dense budget for the same `(params, tile)`.
    pub dense: bool,
}

/// OFFLINE (expensive — one real compact proof per bucket): build the seed table
/// for the given bucket shapes and cache it to `path`. Run this once (offline / on
/// first boot); subsequent boots call [`load_verifier_setup_table`] and rebuild in
/// seconds. Rejects duplicate shape keys, matching
/// [`crate::init_ai_pow_verifier_setup`]'s admission.
pub fn build_and_cache_verifier_setup_seeds(
    path: &std::path::Path,
    buckets: &[VerifierSetupBucketShape],
) -> Result<(), SetupError> {
    let mut keys: Vec<VerifierSetupShapeKey> = Vec::with_capacity(buckets.len());
    let mut seeds: Vec<AiPowCompactVerifierSetupSeed> = Vec::with_capacity(buckets.len());
    for b in buckets {
        let seed = if b.dense {
            build_verifier_setup_seed_dense(&b.params)?
        } else {
            build_verifier_setup_seed(&b.params, b.hw, b.e, b.top_k)?
        };
        let key = shape_key_for_seed(&seed)?;
        if keys.contains(&key) {
            return Err(SetupError(format!(
                "duplicate verifier-setup shape key {:?} in verifier-setup table",
                key,
            )));
        }
        keys.push(key);
        seeds.push(seed);
    }
    save_verifier_setup_seeds(path, &seeds)
}

/// The production verifier setup bucket set: one canonical MoE shape per reachable
/// `(trace_height, sx_bound)` key. Derived by sweeping consensus-valid MoE shapes and
/// keeping, for each distinct key, the first representative.
///
/// The height climbs with the opened tile side `hw`, `k`, and `num_stripes = k/r`;
/// `m = n = e·hw` is the minimal MoE-valid width (each of `e` experts gets exactly
/// `hw` rows/cols) and `m,n` do not affect the trace height. Coverage of the full
/// capped 2^13..2^19 band is asserted cheaply (no proving) in
/// `production_verifier_setup_buckets_cover_the_capped_band`.
pub fn production_verifier_setup_buckets() -> Vec<VerifierSetupBucketShape> {
    use std::collections::BTreeMap;
    const E: usize = 2;
    const TOP_K: usize = 1;
    let mut by_bucket: BTreeMap<VerifierSetupShapeKey, VerifierSetupBucketShape> = BTreeMap::new();
    // Prefer the SMALLEST opened tile `hw` that reaches each key.
    for &hw in &[8u32, 12, 16, 24, 32, 48, 64, 96, 128] {
        let mn = E as u32 * hw;
        for &r in &[32u32, 64, 128, 256, 512, 1024] {
            for &num_stripes in &[16u32, 32, 48, 64, 96, 128, 192, 256, 384, 512] {
                let k = num_stripes * r;
                let params = MatmulParams {
                    m: mn,
                    k,
                    n: mn,
                    noise_rank: r,
                    tile: hw,
                    spot_checks: 1,
                    difficulty_bits: 0,
                };
                if params.validate_prod_envelope().is_err() {
                    continue;
                }
                if let Ok(th) = canonical_moe_trace_height(&params, hw, E, TOP_K) {
                    if th > ai_pow::params::AI_POW_MAX_TRACE_HEIGHT {
                        continue;
                    }
                    let Ok(key) = shape_key_for_params(&params, th) else {
                        continue;
                    };
                    by_bucket.entry(key).or_insert(VerifierSetupBucketShape {
                        params,
                        hw,
                        e: E,
                        top_k: TOP_K,
                        dense: false,
                    });
                }
            }
        }
    }
    // Dense bucket: covers the `(2^13, false)` key that no MoE shape reaches.
    // The MoE routing scatters opened A rows, inflating the strip-opening row
    // budget above the dense budget for the same shape, so the MoE sweep never
    // lands at `(8192, false)`. This dense shape — `m=n=tile=6, k=2112, r=32`
    // (num_stripes=66 > STRIPE_MAX=64) — is inside `validate_prod_envelope` and
    // `envelope_check_dims`, and its dense Layer-0 row budget (7970) rounds up
    // to trace_height 8192 with `sx_bound = false`.
    let dense_params = MatmulParams {
        m: 6,
        k: 2112,
        n: 6,
        noise_rank: 32,
        tile: 6,
        spot_checks: 1,
        difficulty_bits: 0,
    };
    // Defense-in-depth: reject if the shape is ever removed from the admission
    // envelope, rather than silently building a bucket for a shape consensus
    // no longer admits.
    if dense_params.validate_prod_envelope().is_ok() {
        let dense_th = ai_pow::zk_bridge::expected_layer0_rows_for_strip_schedule(
            &dense_params,
            &ai_pow_zk::canonical::StripIndexSchedule {
                a_indices: (0..6).collect(),
                b_indices: (0..6).collect(),
            },
        )
        .map_err(|e| SetupError(format!("dense bucket trace height: {e:?}")))
        .map(|b| b.required_trace_len())
        .unwrap_or(0);
        if dense_th > 0 && dense_th <= ai_pow::params::AI_POW_MAX_TRACE_HEIGHT {
            if let Ok(key) = shape_key_for_params(&dense_params, dense_th) {
                by_bucket.entry(key).or_insert(VerifierSetupBucketShape {
                    params: dense_params,
                    hw: 0,
                    e: 0,
                    top_k: 0,
                    dense: true,
                });
            }
        }
    }
    by_bucket.into_values().collect()
}
