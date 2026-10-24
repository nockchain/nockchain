//! Boot-time compact verifier-setup builder.
//!
//! Verifier contexts and key digests are derived from the canonical AIR and
//! recursive circuit shape, without executing a witness or producing a proof.
//! The per-block opened schedule is bound separately by the program-commitment
//! fold. Production startup validates all supported rule/shape keys before
//! installing the disk-paged contexts.

use std::io::Write;

use ai_pow::params::MatmulParams;
use ai_pow::pearl_compat::{
    derive_pearl_moe_work_commitments, pearl_bitcoin_double_sha256_raw, PearlAuxInclusionProof,
    PearlIncompleteBlockHeader, PearlMiningConfig, PearlMoeParams, PearlNockchainAux,
    PearlPeriodicPattern, PearlPublicProofParams, PEARL_MMA_INT7XINT7_TO_INT32,
    PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG,
};
use ai_pow::pearl_moe_routing::build_routing_data;
use ai_pow::synth::{synth_matrices, AI_POW_PROD_SYNTH_SEED};
use ai_pow::zk_bridge::PearlMoeCompactProveRun;
use ai_pow_miner::certificate_noun::{
    AiPowCertificateShape, AiProofNode, PearlMergeMoeArtifact, PearlMergePublicStatementShape,
};
use ai_pow_zk::proof_rules::ProofRules;
use ai_pow_zk::recursion::{
    compact_batch_verifier_key_digest_to_bytes, AiPowCompactVerifierSetupDescriptor,
};

use crate::{AiPowVerifierSetup, VerifierSetupShapeKey};
/// Error building the verifier setup.
#[derive(Debug)]
pub struct SetupError(pub String);

impl std::fmt::Display for SetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ai-pow verifier setup: {}", self.0)
    }
}
impl std::error::Error for SetupError {}

/// Start the process-owned safe-stack proof workers before any block is admitted.
/// A local thread-start failure must abort startup, not become invalid PoW.
/// Existing global or embedding pools are not used for proof execution.
/// Failed initialization is sticky; fix local resource limits and restart.
pub fn prepare_verifier_resources() -> Result<(), SetupError> {
    ai_pow_zk::runtime::prepare().map_err(|error| SetupError(error.to_string()))
}

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

fn shape_key_for_descriptor(
    descriptor: &AiPowCompactVerifierSetupDescriptor,
) -> Result<VerifierSetupShapeKey, SetupError> {
    let mut key = shape_key_for_zk_params(&descriptor.zk_params, descriptor.trace_height())?;
    key.rules = descriptor.rules;
    Ok(key)
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
/// An arbitrary fixed commitment for the setup-representative block. The setup is
/// proof-independent, so the specific block does not matter.
pub const REFERENCE_SETUP_COMMIT: [u8; 32] = [0x42u8; 32];

/// The reference (setup-representative) block: its prove run plus the pieces needed to assemble
/// its artifact noun (used by tests to exercise the jet against this exact block).
pub struct ReferenceBlock {
    pub run: PearlMoeCompactProveRun,
    pub statement: PearlMergePublicStatementShape,
    pub aux_inclusion: PearlAuxInclusionProof,
    pub moe_art: PearlMergeMoeArtifact,
    pub certificate: AiPowCertificateShape,
    pub commit: [u8; 32],
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
        // unverifiable. See `ai_pow_miner::reference::REFERENCE_NBITS`.
        nbits: ai_pow_miner::reference::REFERENCE_NBITS,
    };
    (
        header,
        PearlAuxInclusionProof {
            coinbase_tx,
            merkle_branch: Vec::new(),
        },
    )
}

/// The reference MoE-block inputs derived from a `(params, hw, e, top_k)` shape —
/// the synthesized matrices, work commitments, routing, opened tile indices, and
/// the block-statement scaffolding. Shared by [`prove_reference_moe_block`] (which
/// then proves + assembles) and [`reference_moe_trace_height`] (which only needs
/// the prove-inputs to predict the trace height), so the two can never disagree
/// about which bucket a shape lands in.
struct ReferenceMoeInputs {
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

/// The matrix-FREE part of the reference MoE inputs: the mining config, routing,
/// and opened tile indices — everything the trace height depends on. Kept separate
/// from the (large) synthesized matrices so [`reference_moe_trace_height`] can sweep
/// candidate shapes cheaply, while [`reference_moe_inputs`] adds the matrices +
/// commitments for the actual prove. Both derive the schedule identically.
struct ReferenceMoeSchedule {
    config: PearlMiningConfig,
    routing: ai_pow::pearl_moe_routing::RoutingData,
    inner: Vec<u32>,
    local_b: Vec<u32>,
    n_e: usize,
    m: usize,
}

fn reference_moe_schedule(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
) -> Result<ReferenceMoeSchedule, SetupError> {
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
    Ok(ReferenceMoeSchedule {
        config,
        routing,
        inner,
        local_b,
        n_e,
        m,
    })
}

fn reference_moe_inputs(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
    nock_commit: [u8; 32],
) -> Result<ReferenceMoeInputs, SetupError> {
    let ReferenceMoeSchedule {
        config,
        routing,
        inner,
        local_b,
        n_e,
        m,
    } = reference_moe_schedule(params, hw, e, top_k)?;

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

    Ok(ReferenceMoeInputs {
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

/// The Layer-0 trace height a reference MoE block at `(params, hw, e, top_k)` would
/// have — WITHOUT proving AND without synthesizing the (large) matrices. Lets
/// [`production_verifier_setup_buckets`] cheaply select one shape per trace-height
/// bucket. Equal to the height the full prove yields.
pub fn reference_moe_trace_height(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
) -> Result<usize, SetupError> {
    let s = reference_moe_schedule(params, hw, e, top_k)?;
    ai_pow::zk_bridge::pearl_moe_canonical_trace_height(
        params, &s.routing, 0, &s.inner, &s.local_b, s.n_e,
    )
    .map_err(err("moe reference trace height"))
}

/// Prove a single reference MoE block at the given shape. `hw` is the opened-tile
/// side (`h = w = hw`); `e`/`top_k` the MoE config. Panics-free (returns errors).
pub fn prove_reference_moe_block(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
    nock_commit: [u8; 32],
) -> Result<ReferenceBlock, SetupError> {
    prove_reference_moe_block_with_rules(params, hw, e, top_k, nock_commit, ProofRules::Hardened)
}

pub fn prove_reference_moe_block_with_rules(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
    nock_commit: [u8; 32],
    rules: ProofRules,
) -> Result<ReferenceBlock, SetupError> {
    let ReferenceMoeInputs {
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
    } = reference_moe_inputs(params, hw, e, top_k, nock_commit)?;

    let run = ai_pow::zk_bridge::prove_pearl_moe_compact_recursive_certificate_with_rules(
        params, &a, &b, &commitments.kappa, &commitments.h_a, &commitments.h_b, &routing, 0,
        &inner, &local_b, n_e, rules,
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
    #[cfg(test)]
    eprintln!(
        "reference MoE {rules:?} trace_height={} certificate_bytes={}",
        run.trace_height,
        cert_bytes.len()
    );
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

    Ok(ReferenceBlock {
        run,
        statement,
        aux_inclusion,
        moe_art,
        certificate,
        commit: nock_commit,
    })
}

/// Compile and preprocess the canonical recursive verifier for a MoE shape.
pub fn build_verifier_setup(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
) -> Result<AiPowVerifierSetup, SetupError> {
    build_verifier_setup_with_rules(params, hw, e, top_k, ProofRules::Hardened)
}

pub fn build_verifier_setup_with_rules(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
    rules: ProofRules,
) -> Result<AiPowVerifierSetup, SetupError> {
    let trace_height = reference_moe_trace_height(params, hw, e, top_k)?;
    build_verifier_setup_for_trace_height(params, trace_height, rules)
}

fn build_verifier_setup_for_trace_height(
    params: &MatmulParams,
    trace_height: usize,
    rules: ProofRules,
) -> Result<AiPowVerifierSetup, SetupError> {
    let zk_params = ai_pow::zk_bridge::zk_params_from_matmul(params);
    let context = ai_pow_zk::recursion::derive_compact_verifier_context_for_shape(
        &zk_params, trace_height, rules,
    )
    .map_err(err("derive canonical verifier context"))?;
    let digest = context
        .validate_setup_binding()
        .map_err(err("verifier setup binding"))?;
    let shape_key = shape_key_for_zk_params(&zk_params, trace_height)?;
    Ok(AiPowVerifierSetup {
        trace_height,
        sx_bound: shape_key.sx_bound,
        context,
        digest_bytes: ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(&digest)
            .to_vec(),
    })
}

/// Compile a verifier and retain only its shape and canonical key digest.
pub fn build_verifier_setup_descriptor(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
) -> Result<AiPowCompactVerifierSetupDescriptor, SetupError> {
    build_verifier_setup_descriptor_with_rules(params, hw, e, top_k, ProofRules::Hardened)
}

pub fn build_verifier_setup_descriptor_with_rules(
    params: &MatmulParams,
    hw: u32,
    e: usize,
    top_k: usize,
    rules: ProofRules,
) -> Result<AiPowCompactVerifierSetupDescriptor, SetupError> {
    let setup = build_verifier_setup_with_rules(params, hw, e, top_k, rules)?;
    descriptor_for_setup(params, &setup)
}

/// Compile a dense shape without constructing its matrices or a reference proof.
pub fn build_verifier_setup_descriptor_dense(
    params: &MatmulParams,
) -> Result<AiPowCompactVerifierSetupDescriptor, SetupError> {
    build_verifier_setup_descriptor_dense_with_rules(params, ProofRules::Hardened)
}

pub fn build_verifier_setup_descriptor_dense_with_rules(
    params: &MatmulParams,
    rules: ProofRules,
) -> Result<AiPowCompactVerifierSetupDescriptor, SetupError> {
    let trace_height = ai_pow::zk_bridge::pearl_dense_canonical_trace_height(params, 0, 0)
        .map_err(err("dense verifier trace height"))?;
    let setup = build_verifier_setup_for_trace_height(params, trace_height, rules)?;
    descriptor_for_setup(params, &setup)
}

fn descriptor_for_setup(
    params: &MatmulParams,
    setup: &AiPowVerifierSetup,
) -> Result<AiPowCompactVerifierSetupDescriptor, SetupError> {
    AiPowCompactVerifierSetupDescriptor::from_context(
        ai_pow::zk_bridge::zk_params_from_matmul(params),
        setup.trace_height,
        &setup.context,
    )
    .map_err(err("verifier setup descriptor"))
}

/// Reconstruct a canonical context and reject a descriptor with a different key.
pub fn rebuild_verifier_setup_from_descriptor(
    descriptor: AiPowCompactVerifierSetupDescriptor,
) -> Result<AiPowVerifierSetup, SetupError> {
    let shape_key = shape_key_for_descriptor(&descriptor)?;
    let digest_bytes = descriptor.verifier_key_digest_bytes.clone();
    let context = descriptor
        .derive_context()
        .map_err(err("derive verifier context from descriptor"))?;
    Ok(AiPowVerifierSetup {
        trace_height: shape_key.trace_height,
        sx_bound: shape_key.sx_bound,
        context,
        digest_bytes,
    })
}

const VERIFIER_SETUP_DESCRIPTOR_CACHE_MAGIC: &[u8; 8] = b"NCVPSHAP";
const VERIFIER_SETUP_DESCRIPTOR_CACHE_FORMAT_VERSION: u32 = 3;
const VERIFIER_SETUP_DESCRIPTOR_CACHE_HEADER_LEN: usize = VERIFIER_SETUP_DESCRIPTOR_CACHE_MAGIC
    .len()
    + std::mem::size_of::<u32>()
    + std::mem::size_of::<u64>()
    + 32;

/// Shape-only cache files are independent of the obsolete proof-bearing format.
pub fn verifier_setup_descriptor_cache_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("ai-pow").join(format!(
        "verifier-setup-shapes-v{VERIFIER_SETUP_DESCRIPTOR_CACHE_FORMAT_VERSION}.bin"
    ))
}

fn write_file_atomically_with_commit(
    path: &std::path::Path,
    bytes: &[u8],
    artifact: &str,
    commit: impl FnOnce(&std::path::Path, &std::path::Path) -> std::io::Result<()>,
) -> Result<(), SetupError> {
    let parent = path.parent().ok_or_else(|| {
        SetupError(format!(
            "{artifact} has no parent directory: {}",
            path.display()
        ))
    })?;
    std::fs::create_dir_all(parent)
        .map_err(|e| SetupError(format!("create {artifact} directory: {e:?}")))?;

    let file_name = path
        .file_name()
        .ok_or_else(|| SetupError(format!("{artifact} has no file name: {}", path.display())))?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| SetupError(format!("read {artifact} clock: {e:?}")))?
        .as_nanos();
    let mut temp_name = file_name.to_os_string();
    temp_name.push(format!(".{}.{}.tmp", std::process::id(), nonce));
    let temp_path = parent.join(temp_name);

    let result = (|| -> Result<(), SetupError> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|e| SetupError(format!("create temporary {artifact}: {e:?}")))?;
        file.write_all(bytes)
            .map_err(|e| SetupError(format!("write temporary {artifact}: {e:?}")))?;
        file.sync_all()
            .map_err(|e| SetupError(format!("sync temporary {artifact}: {e:?}")))?;
        drop(file);
        commit(&temp_path, path).map_err(|e| SetupError(format!("commit {artifact}: {e:?}")))?;
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|e| SetupError(format!("sync {artifact} directory: {e:?}")))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

fn write_file_atomically(
    path: &std::path::Path,
    bytes: &[u8],
    artifact: &str,
) -> Result<(), SetupError> {
    write_file_atomically_with_commit(path, bytes, artifact, |temporary, destination| {
        std::fs::rename(temporary, destination)
    })
}

fn write_descriptor_cache_atomically(
    path: &std::path::Path,
    bytes: &[u8],
) -> Result<(), SetupError> {
    write_file_atomically(path, bytes, "verifier-setup cache")
}

/// Serialize shape descriptors with a versioned, checksummed envelope.
pub fn save_verifier_setup_descriptors(
    path: &std::path::Path,
    descriptors: &[AiPowCompactVerifierSetupDescriptor],
) -> Result<(), SetupError> {
    let payload = bincode::serde::encode_to_vec(descriptors, bincode::config::standard())
        .map_err(err("serialize verifier-setup descriptors"))?;
    let payload_len = u64::try_from(payload.len())
        .map_err(|_| SetupError("verifier-setup cache payload exceeds u64".to_string()))?;
    let checksum = blake3::hash(&payload);
    let mut bytes = Vec::with_capacity(VERIFIER_SETUP_DESCRIPTOR_CACHE_HEADER_LEN + payload.len());
    bytes.extend_from_slice(VERIFIER_SETUP_DESCRIPTOR_CACHE_MAGIC);
    bytes.extend_from_slice(&VERIFIER_SETUP_DESCRIPTOR_CACHE_FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&payload_len.to_le_bytes());
    bytes.extend_from_slice(checksum.as_bytes());
    bytes.extend_from_slice(&payload);
    write_descriptor_cache_atomically(path, &bytes)
}

/// Decode the shape-only cache, rejecting incompatible or corrupt framing.
pub fn load_verifier_setup_descriptors(
    path: &std::path::Path,
) -> Result<Vec<AiPowCompactVerifierSetupDescriptor>, SetupError> {
    let bytes = std::fs::read(path).map_err(err("read verifier-setup cache"))?;
    if bytes.len() < VERIFIER_SETUP_DESCRIPTOR_CACHE_HEADER_LEN {
        return Err(SetupError(format!(
            "verifier-setup cache at {} is shorter than its header",
            path.display()
        )));
    }

    let mut offset = 0usize;
    let magic = &bytes[offset..offset + VERIFIER_SETUP_DESCRIPTOR_CACHE_MAGIC.len()];
    if magic != VERIFIER_SETUP_DESCRIPTOR_CACHE_MAGIC {
        return Err(SetupError(format!(
            "verifier-setup cache at {} has an unsupported format marker",
            path.display()
        )));
    }
    offset += VERIFIER_SETUP_DESCRIPTOR_CACHE_MAGIC.len();

    let version = u32::from_le_bytes(
        bytes[offset..offset + std::mem::size_of::<u32>()]
            .try_into()
            .expect("cache header length checked"),
    );
    offset += std::mem::size_of::<u32>();
    if version != VERIFIER_SETUP_DESCRIPTOR_CACHE_FORMAT_VERSION {
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

    let (descriptors, consumed): (Vec<AiPowCompactVerifierSetupDescriptor>, usize) =
        bincode::serde::decode_from_slice(payload, bincode::config::standard())
            .map_err(err("deserialize verifier-setup descriptors"))?;
    if consumed != payload.len() {
        return Err(SetupError(format!(
            "verifier-setup cache at {} has trailing descriptor bytes",
            path.display()
        )));
    }
    for descriptor in &descriptors {
        descriptor
            .validate_shape()
            .map_err(err("cached verifier shape"))?;
    }
    Ok(descriptors)
}

/// Reconstruct canonical contexts from a shape-descriptor cache.
pub fn load_verifier_setup_table(
    path: &std::path::Path,
) -> Result<Vec<AiPowVerifierSetup>, SetupError> {
    load_verifier_setup_descriptors(path)?
        .into_iter()
        .map(rebuild_verifier_setup_from_descriptor)
        .collect()
}

/// Env var to override the resident-context LRU cap (max heavy contexts kept in
/// memory at once). See [`verifier_cache_cap`].
pub const AI_POW_VERIFIER_CACHE_CAP_ENV: &str = "AI_POW_VERIFIER_CACHE_CAP";

/// Default resident-context LRU cap. The production default retains every supported
/// setup shape, so remote inputs cannot create an evict/reload loop on the consensus
/// thread unless an operator deliberately lowers the cap.
pub const AI_POW_VERIFIER_CACHE_CAP_DEFAULT: usize = 28;

/// Resolve the resident-context LRU cap from `AI_POW_VERIFIER_CACHE_CAP` (clamped to
/// `>= 1`), else the DoS-safe all-shape default.
pub fn verifier_cache_cap() -> usize {
    std::env::var(AI_POW_VERIFIER_CACHE_CAP_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .map(|c| c.max(1))
        .unwrap_or(AI_POW_VERIFIER_CACHE_CAP_DEFAULT)
}

fn load_and_validate_descriptors(
    path: &std::path::Path,
) -> Result<Vec<AiPowCompactVerifierSetupDescriptor>, SetupError> {
    let descriptors = load_verifier_setup_descriptors(path)?;
    crate::table_digest::verify_verifier_setup_descriptor_table_digest(&descriptors)?;
    Ok(descriptors)
}

/// Derive or load every production context before admitting blocks. Both table
/// fingerprints, every context binding, and all 28 native keys are mandatory.
/// Existing cache bytes are retained until a complete replacement is committed.
/// A fresh derivation mismatch is fatal. Repeated initialization returns `Ok(0)`.
pub fn install_or_build_verifier_setup(
    data_dir: &std::path::Path,
    buckets: &[VerifierSetupBucketShape],
) -> Result<usize, SetupError> {
    prepare_verifier_resources()?;
    if crate::ai_pow_verifier_setup_initialized() {
        return Ok(0);
    }
    let path = verifier_setup_descriptor_cache_path(data_dir);
    let cached = if path.exists() {
        match load_and_validate_descriptors(&path) {
            Ok(descriptors) => Some(descriptors),
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    "AI-PoW shape cache is unusable ({error}); deriving an atomic replacement",
                );
                None
            }
        }
    } else {
        None
    };
    let disk_buckets = match cached {
        Some(descriptors) => build_or_reuse_disk_contexts(data_dir, descriptors)?,
        None => {
            if buckets.is_empty() {
                return Err(SetupError(
                    "no usable verifier cache and no canonical bucket shapes".to_string(),
                ));
            }
            validate_production_bucket_shapes(buckets)?;
            let (descriptors, disk_buckets) = derive_disk_contexts(data_dir, buckets)?;
            save_verifier_setup_descriptors(&path, &descriptors)?;
            disk_buckets
        }
    };
    let n = disk_buckets.len();
    let cap = verifier_cache_cap();
    crate::init_ai_pow_verifier_setup_disk(disk_buckets, cap)
        .map_err(|()| SetupError("verifier table rejected or already initialized".to_string()))?;
    tracing::info!("AI-PoW verifier setup installed: {n} native contexts, at most {cap} resident",);
    Ok(n)
}

/// One compact context file, keyed by rule, canonical geometry and key digest.
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
    data_dir.join("ai-pow").join(format!(
        "ctx-v4-{:?}-2p{log2}-{sx}-{tag}.bin",
        shape_key.rules
    ))
}

/// The sidecar file holding the BLAKE3 checksum of a context file's bytes.
fn context_checksum_path(context_path: &std::path::Path) -> std::path::PathBuf {
    let mut p = context_path.as_os_str().to_owned();
    p.push(".blake3");
    std::path::PathBuf::from(p)
}

/// Serialize one built verifier setup to a synced temporary file and atomically
/// rename it into place. Its checksum sidecar is replaced separately after the
/// context commit; a missing sidecar is recoverable because page-in always hashes
/// the context bytes.
fn write_verifier_context_file(
    setup: &AiPowVerifierSetup,
    path: &std::path::Path,
) -> Result<[u8; 32], SetupError> {
    let bytes = bincode::serde::encode_to_vec(setup, bincode::config::standard())
        .map_err(err("serialize verifier context"))?;
    let checksum = *blake3::hash(&bytes).as_bytes();
    let sidecar = context_checksum_path(path);
    match std::fs::remove_file(&sidecar) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(SetupError(format!(
                "remove stale verifier context checksum: {error:?}"
            )));
        }
    }
    write_file_atomically(path, &bytes, "verifier context file")?;
    if let Err(error) = write_file_atomically(&sidecar, &checksum, "verifier context checksum") {
        tracing::warn!(
            path = %sidecar.display(),
            error = %error,
            "Verifier context committed without checksum sidecar; it will be rebuilt on load",
        );
    }
    Ok(checksum)
}

/// The expected checksum for an existing context file: read the 32-byte sidecar if
/// present, else compute it from the file bytes and write the sidecar (self-heal for
/// files written before checksums, or a lost sidecar). This never trusts the file — it
/// only records what is on disk; `page_in_bucket` re-reads + re-checks on every use.
pub(crate) fn context_file_checksum(path: &std::path::Path) -> Result<[u8; 32], SetupError> {
    let sidecar = context_checksum_path(path);
    if let Ok(bytes) = std::fs::read(&sidecar) {
        if let Ok(arr) = <[u8; 32]>::try_from(bytes.as_slice()) {
            return Ok(arr);
        }
    }
    let file = std::fs::read(path).map_err(err("read verifier context for checksum"))?;
    let checksum = *blake3::hash(&file).as_bytes();
    // Best-effort atomic sidecar write (page-in recomputes when it is absent).
    let _ = write_file_atomically(&sidecar, &checksum, "verifier context checksum");
    Ok(checksum)
}

/// Validate existing contexts at startup, or reconstruct missing ones from shape.
/// Every descriptor key is checked against a freshly recomputed native binding.
fn build_or_reuse_disk_contexts(
    data_dir: &std::path::Path,
    descriptors: Vec<AiPowCompactVerifierSetupDescriptor>,
) -> Result<Vec<crate::DiskBucket>, SetupError> {
    let mut disk_buckets = Vec::with_capacity(descriptors.len());
    for descriptor in descriptors {
        let shape_key = shape_key_for_descriptor(&descriptor)?;
        let committed_digest = descriptor.verifier_key_digest_bytes.clone();
        let ctx_path = verifier_context_file_path(data_dir, shape_key, &committed_digest);
        let checksum = if ctx_path.exists() {
            context_file_checksum(&ctx_path)?
        } else {
            let setup = rebuild_verifier_setup_from_descriptor(descriptor)?;
            write_verifier_context_file(&setup, &ctx_path)?
        };
        let bucket = crate::DiskBucket::new(shape_key, committed_digest, ctx_path, checksum);
        disk_buckets.push(bucket);
    }
    verify_disk_contexts(&disk_buckets)?;
    Ok(disk_buckets)
}

fn verify_disk_contexts(buckets: &[crate::DiskBucket]) -> Result<(), SetupError> {
    let mut native_fingerprints = Vec::with_capacity(buckets.len());
    for bucket in buckets {
        let native = crate::page_in_bucket(bucket).ok_or_else(|| {
            SetupError(format!(
                "cached verifier context at {:?} failed native binding",
                bucket.shape_key,
            ))
        })?;
        native_fingerprints.push((
            native.shape_key(),
            compact_batch_verifier_key_digest_to_bytes(native.context.verifier_key_digest()),
        ));
    }
    crate::table_digest::verify_production_fingerprints(&native_fingerprints)?;
    Ok(())
}

fn derive_disk_contexts(
    data_dir: &std::path::Path,
    buckets: &[VerifierSetupBucketShape],
) -> Result<
    (
        Vec<AiPowCompactVerifierSetupDescriptor>,
        Vec<crate::DiskBucket>,
    ),
    SetupError,
> {
    let mut descriptors = Vec::with_capacity(2 * buckets.len());
    let mut disk_buckets = Vec::with_capacity(2 * buckets.len());
    let mut native_fingerprints = Vec::with_capacity(2 * buckets.len());
    for (shape, rules) in buckets
        .iter()
        .flat_map(|shape| [ProofRules::Legacy, ProofRules::Hardened].map(|rules| (shape, rules)))
    {
        let setup = build_bucket_verifier_setup(shape, rules)?;
        let descriptor = descriptor_for_setup(&shape.params, &setup)?;
        let shape_key = setup.shape_key();
        if descriptors
            .iter()
            .any(|prior| shape_key_for_descriptor(prior).ok() == Some(shape_key))
        {
            return Err(SetupError(format!(
                "duplicate verifier shape {shape_key:?}"
            )));
        }
        native_fingerprints.push((
            shape_key,
            compact_batch_verifier_key_digest_to_bytes(setup.context.verifier_key_digest()),
        ));
        let ctx_path = verifier_context_file_path(data_dir, shape_key, &setup.digest_bytes);
        let checksum = write_verifier_context_file(&setup, &ctx_path)?;
        disk_buckets.push(crate::DiskBucket::new(
            shape_key, setup.digest_bytes, ctx_path, checksum,
        ));
        descriptors.push(descriptor);
    }
    crate::table_digest::verify_production_fingerprints(&native_fingerprints)?;
    Ok((descriptors, disk_buckets))
}

/// Test / bootstrap helper: serialize already-built `setups` to per-bucket context
/// files under `data_dir` and inject them disk-paged, mirroring what the boot
/// installer does after building — without generating or loading a shape cache. Each
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

/// Install an existing complete cache without generating missing contexts.
/// An absent cache is allowed for non-consensus tools; an invalid cache is not.
pub fn install_verifier_setup_from_cache(data_dir: &std::path::Path) -> Result<usize, SetupError> {
    prepare_verifier_resources()?;
    if crate::ai_pow_verifier_setup_initialized() {
        return Ok(0);
    }
    let path = verifier_setup_descriptor_cache_path(data_dir);
    if !path.exists() {
        return Ok(0);
    }
    let descriptors = load_and_validate_descriptors(&path)?;
    let mut disk_buckets = Vec::with_capacity(descriptors.len());
    for descriptor in descriptors {
        let key = shape_key_for_descriptor(&descriptor)?;
        let digest = descriptor.verifier_key_digest_bytes;
        let path = verifier_context_file_path(data_dir, key, &digest);
        let checksum = context_file_checksum(&path)?;
        let bucket = crate::DiskBucket::new(key, digest, path, checksum);
        disk_buckets.push(bucket);
    }
    verify_disk_contexts(&disk_buckets)?;
    let n = disk_buckets.len();
    crate::init_ai_pow_verifier_setup_disk(disk_buckets, verifier_cache_cap())
        .map_err(|()| SetupError("verifier table rejected or already initialized".to_string()))?;
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
    /// When `true`, derive the trace height from the dense strip schedule.
    /// `hw`/`e`/`top_k` are ignored. Dense buckets cover verifier shapes that no MoE
    /// shape reaches — the MoE routing scatters opened rows, inflating the Layer-0
    /// trace height above the dense budget for the same `(params, tile)`.
    pub dense: bool,
}

impl VerifierSetupBucketShape {
    pub(crate) fn trace_height(&self) -> Result<usize, SetupError> {
        if self.dense {
            ai_pow::zk_bridge::pearl_dense_canonical_trace_height(&self.params, 0, 0)
                .map_err(err("dense canonical height"))
        } else {
            reference_moe_trace_height(&self.params, self.hw, self.e, self.top_k)
        }
    }
}

pub(crate) fn production_verifier_setup_geometries(
) -> Result<Vec<(ai_pow_zk::params::ZkParams, usize)>, SetupError> {
    production_verifier_setup_buckets()
        .into_iter()
        .map(|shape| {
            Ok((
                ai_pow::zk_bridge::zk_params_from_matmul(&shape.params),
                shape.trace_height()?,
            ))
        })
        .collect()
}

fn validate_production_bucket_shapes(
    buckets: &[VerifierSetupBucketShape],
) -> Result<(), SetupError> {
    let expected = production_verifier_setup_geometries()?;
    let supplied = buckets
        .iter()
        .map(|shape| {
            Ok((
                ai_pow::zk_bridge::zk_params_from_matmul(&shape.params),
                shape.trace_height()?,
            ))
        })
        .collect::<Result<Vec<_>, SetupError>>()?;
    if supplied.len() != expected.len() || expected.iter().any(|shape| !supplied.contains(shape)) {
        return Err(SetupError(
            "setup requires all 14 canonical production geometries".to_string(),
        ));
    }
    Ok(())
}

fn build_bucket_verifier_setup(
    shape: &VerifierSetupBucketShape,
    rules: ProofRules,
) -> Result<AiPowVerifierSetup, SetupError> {
    build_verifier_setup_for_trace_height(&shape.params, shape.trace_height()?, rules)
}

/// Derive a shape table and compact native context files, rejecting duplicate keys.
pub fn build_and_cache_verifier_setup_descriptors(
    path: &std::path::Path,
    buckets: &[VerifierSetupBucketShape],
) -> Result<(), SetupError> {
    prepare_verifier_resources()?;
    let mut descriptors = Vec::with_capacity(2 * buckets.len());
    let mut keys = Vec::with_capacity(2 * buckets.len());
    for (shape, rules) in buckets
        .iter()
        .flat_map(|shape| [ProofRules::Legacy, ProofRules::Hardened].map(|rules| (shape, rules)))
    {
        let setup = build_bucket_verifier_setup(shape, rules)?;
        let key = setup.shape_key();
        if keys.contains(&key) {
            return Err(SetupError(format!("duplicate verifier shape {key:?}")));
        }
        keys.push(key);
        descriptors.push(descriptor_for_setup(&shape.params, &setup)?);
        let context_name =
            verifier_context_file_path(std::path::Path::new(""), key, &setup.digest_bytes);
        let context_path = path.with_file_name(context_name.file_name().expect("context filename"));
        write_verifier_context_file(&setup, &context_path)?;
    }
    save_verifier_setup_descriptors(path, &descriptors)
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
                if let Ok(th) = reference_moe_trace_height(&params, hw, E, TOP_K) {
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

#[cfg(test)]
mod atomic_write_tests {
    use super::write_file_atomically_with_commit;

    #[test]
    fn verifier_resources_are_ready_before_admission_and_reusable() {
        super::prepare_verifier_resources().expect("initialize worker pool");
        super::prepare_verifier_resources().expect("reuse worker pool");
    }

    #[test]
    fn failed_atomic_commit_preserves_existing_file() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let destination = directory.path().join("verifier-context.bin");
        std::fs::write(&destination, b"complete-old-context").expect("seed destination");

        let error = write_file_atomically_with_commit(
            &destination,
            b"complete-new-context",
            "test verifier context",
            |temporary, _| {
                assert_eq!(
                    std::fs::read(temporary).expect("temporary context should be readable"),
                    b"complete-new-context"
                );
                Err(std::io::Error::other("simulated interruption"))
            },
        )
        .expect_err("interrupted commit must fail");

        assert!(error.to_string().contains("simulated interruption"));
        assert_eq!(
            std::fs::read(&destination).expect("old context should remain readable"),
            b"complete-old-context"
        );
    }
}
