//! Boot-time compact verifier-setup builder (Branch b, Stage 2).
//!
//! The compact verifier `context` + `verifier_key_digest` are deterministic from
//! the puzzle SHAPE (params / trace-height) and **proof-independent** (validated in
//! `ai-pow-miner::moe_compact_verifier_setup_is_proof_independent`). So a consensus
//! node builds them ONCE at boot by proving a single canonical block, then injects
//! the result via [`crate::init_ai_pow_verifier_setup`] and reuses it to verify
//! every same-shape `%ai-pow` block. The per-block opened schedule is bound
//! separately by the P0/D6 program-commitment fold — not by the setup — so one
//! setup serves all blocks of the shape (dense and MoE alike).

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
use ai_pow_miner::certificate_noun::{
    AiPowCertificateShape, AiProofNode, PearlMergeMoeArtifact, PearlMergePublicStatementShape,
};
use ai_pow_zk::recursion::AiPowCompactVerifierSetupSeed;

use crate::AiPowVerifierSetup;

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
    n: usize,
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
    n: usize,
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
        n,
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
        n,
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
    Ok(AiPowVerifierSetup {
        trace_height,
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

/// Rebuild the full boot verifier setup from a cached seed WITHOUT proving — the
/// boot-time counterpart of [`build_verifier_setup_seed`]. Rebuilds the compact
/// verifier context (circuit compile + Merkle commit; seconds, no FRI proving) and
/// pairs it with the trace height + the cached verifier-key digest. The result is
/// byte-for-byte equivalent to the [`build_verifier_setup`] (direct-context) form,
/// validated in `moe_verifier_setup_seed_roundtrip_rebuilds_working_setup`.
pub fn rebuild_verifier_setup_from_seed(
    seed: AiPowCompactVerifierSetupSeed,
) -> Result<AiPowVerifierSetup, SetupError> {
    let trace_height = seed.trace_height();
    let digest_bytes = seed.verifier_key_digest_bytes.clone();
    let context = seed
        .rebuild_context()
        .map_err(err("rebuild verifier context from seed"))?;
    Ok(AiPowVerifierSetup {
        trace_height,
        context,
        digest_bytes,
    })
}

/// The sane cache path inside the nockapp data dir for the boot verifier-setup
/// seed table. Kept under an `ai-pow/` subdirectory so we do not litter the data
/// dir with loose files — it sits alongside the other per-node persistence.
pub fn verifier_setup_seed_cache_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("ai-pow").join("verifier-setup-seeds.bin")
}

/// Serialize a seed table to `path` (creating parent dirs as needed). The cached
/// artifact is small — the seeds (KB-MB/bucket), NOT the rebuilt ~866 MB contexts.
pub fn save_verifier_setup_seeds(
    path: &std::path::Path,
    seeds: &[AiPowCompactVerifierSetupSeed],
) -> Result<(), SetupError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(err("create verifier-setup cache dir"))?;
    }
    let bytes = bincode::serde::encode_to_vec(seeds, bincode::config::standard())
        .map_err(err("serialize verifier-setup seeds"))?;
    std::fs::write(path, bytes).map_err(err("write verifier-setup cache"))?;
    Ok(())
}

/// Load a seed table from `path` — the inverse of [`save_verifier_setup_seeds`].
pub fn load_verifier_setup_seeds(
    path: &std::path::Path,
) -> Result<Vec<AiPowCompactVerifierSetupSeed>, SetupError> {
    let bytes = std::fs::read(path).map_err(err("read verifier-setup cache"))?;
    let (seeds, _) = bincode::serde::decode_from_slice(&bytes, bincode::config::standard())
        .map_err(err("deserialize verifier-setup seeds"))?;
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

/// Default resident-context LRU cap. A node verifies at the current difficulty's
/// trace height (stable — ASERT adjusts slowly) and admits blocks in height order,
/// so the honest working set is ~1–2 heights; 2 avoids thrashing on a height shift
/// while keeping standing RSS to ~2 contexts instead of all 7.
///
/// DoS knob: `trace_height` is an attacker-controlled certificate field, and a cache
/// miss triggers a ~0.6 s synchronous context page-in on the single serf/consensus
/// thread — paid even for INVALID blocks (the page-in precedes the cert reject). An
/// attacker cycling ≥ cap+1 of the 7 valid buckets (2^13..2^19) forces an
/// evict+reload each block, stalling consensus. If that thrash is observed, raise the
/// cap to 7 (all buckets resident, bounded RSS, no page-in) via
/// `--ai-pow-verifier-cache-cap` or the `AI_POW_VERIFIER_CACHE_CAP` env var. Kept low
/// by default so the disk-paging RSS optimization holds for the common case.
pub const AI_POW_VERIFIER_CACHE_CAP_DEFAULT: usize = 2;

/// Resolve the resident-context LRU cap from `AI_POW_VERIFIER_CACHE_CAP` (clamped to
/// `>= 1`), else the default. Raising it trades RSS for fewer on-demand rebuilds;
/// `>= 7` keeps every bucket resident once built (≈ the old all-resident behavior,
/// minus the upfront boot rebuild).
pub fn verifier_cache_cap() -> usize {
    std::env::var(AI_POW_VERIFIER_CACHE_CAP_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .map(|c| c.max(1))
        .unwrap_or(AI_POW_VERIFIER_CACHE_CAP_DEFAULT)
}

/// Load the cached SEEDS and validate them against the committed **v0** consensus
/// digest WITHOUT rebuilding (lazy boot). Any failure — a missing/unreadable file, a
/// corrupt or format-incompatible cache (bincode decode error), or a mismatch against
/// the pinned consensus constant — is returned as `Err`. The boot installer treats a
/// present-but-invalid cache as corrupt and regenerates from scratch.
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
/// [`crate::init_ai_pow_verifier_setup_disk`] / [`verifier_cache_cap`]), so a verify
/// never rebuilds — at most a ~0.6 s page-in from disk — and standing RSS is a bounded
/// working set.
///
/// - **Cache present and valid:** load seeds + validate digest + build/reuse contexts.
/// - **Cache present but corrupt / format-incompatible / digest-mismatched:** DELETE
///   it and regenerate from scratch, rather than run against an unusable/divergent
///   cache.
/// - **Cache absent (or just deleted):** GENERATE it (one real compact proof per
///   `buckets` entry — a one-time ~5-minute boot delay), cache it, then load +
///   validate + inject.
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
                    "AI-PoW verifier-setup cache at {} is unusable ({e}); deleting and \
                     regenerating from scratch",
                    path.display(),
                );
                std::fs::remove_file(&path)
                    .map_err(err("delete corrupt/incompatible verifier-setup cache"))?;
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
                 step and takes about 5 minutes; the result is cached, so subsequent boots are \
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
        "AI-PoW verifier-setup installed (disk-paged): {n} bucket(s); contexts paged from disk, up \
         to {cap} resident at once.",
    );
    Ok(n)
}

/// The on-disk file for one bucket's serialized verifier context. The committed
/// verifier-key digest is baked into the filename so a seed/table change (⇒ new
/// digest) yields a new filename and a rebuild, and a stale file is never mistaken
/// for the current one.
pub fn verifier_context_file_path(
    data_dir: &std::path::Path,
    trace_height: usize,
    committed_digest: &[u8],
) -> std::path::PathBuf {
    let log2 = (trace_height as u64).trailing_zeros();
    let tag: String = committed_digest
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect();
    data_dir
        .join("ai-pow")
        .join(format!("ctx-2p{log2}-{tag}.bin"))
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
        let h = seed.trace_height();
        let committed_digest = seed.verifier_key_digest_bytes.clone();
        let ctx_path = verifier_context_file_path(data_dir, h, &committed_digest);
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
            // Digest gate: the built context must match its seed's committed digest.
            let digest = ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(
                setup.context.verifier_key_digest(),
            );
            if digest.as_slice() != committed_digest.as_slice()
                || setup.digest_bytes != committed_digest
            {
                return Err(SetupError(format!(
                    "built verifier context for trace_height {h} does not match its committed \
                     digest — refusing to run a divergent verifier"
                )));
            }
            let ck = write_verifier_context_file(&setup, &ctx_path)?;
            built += 1;
            ck
        };
        disk_buckets.push(crate::DiskBucket::new(
            h, committed_digest, ctx_path, checksum,
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
        let h = setup.trace_height;
        let digest = setup.digest_bytes.clone();
        let ctx_path = verifier_context_file_path(data_dir, h, &digest);
        let checksum = write_verifier_context_file(setup, &ctx_path)?;
        disk_buckets.push(crate::DiskBucket::new(h, digest, ctx_path, checksum));
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
            let h = seed.trace_height();
            let digest = seed.verifier_key_digest_bytes.clone();
            let ctx_path = verifier_context_file_path(data_dir, h, &digest);
            // Register-only (never paged in by a non-consensus tool): read the sidecar
            // checksum if a node already built the file, else a zero placeholder.
            let checksum = context_file_checksum(&ctx_path).unwrap_or([0u8; 32]);
            crate::DiskBucket::new(h, digest, ctx_path, checksum)
        })
        .collect();
    crate::init_ai_pow_verifier_setup_disk(disk_buckets, verifier_cache_cap()).map_err(|()| {
        SetupError(
            "verifier-setup table rejected (empty / duplicate buckets) or already initialized"
                .to_string(),
        )
    })?;
    Ok(n)
}

/// One production trace-height bucket: the puzzle shape that lands in it. The boot
/// table has one entry per reachable Pearl trace height (shapes sharing a height
/// share a setup — the setup is height-keyed, not shape-keyed).
#[derive(Clone, Copy, Debug)]
pub struct VerifierSetupBucketShape {
    pub params: MatmulParams,
    pub hw: u32,
    pub e: usize,
    pub top_k: usize,
}

/// OFFLINE (expensive — one real compact proof per bucket): build the seed table
/// for the given bucket shapes and cache it to `path`. Run this once (offline / on
/// first boot); subsequent boots call [`load_verifier_setup_table`] and rebuild in
/// seconds. Rejects duplicate trace-height buckets (each cert must resolve to
/// exactly one setup), matching [`crate::init_ai_pow_verifier_setup`]'s admission.
pub fn build_and_cache_verifier_setup_seeds(
    path: &std::path::Path,
    buckets: &[VerifierSetupBucketShape],
) -> Result<(), SetupError> {
    let mut seeds: Vec<AiPowCompactVerifierSetupSeed> = Vec::with_capacity(buckets.len());
    for b in buckets {
        let seed = build_verifier_setup_seed(&b.params, b.hw, b.e, b.top_k)?;
        let h = seed.trace_height();
        if seeds.iter().any(|s| s.trace_height() == h) {
            return Err(SetupError(format!(
                "duplicate trace-height bucket {h} in verifier-setup table"
            )));
        }
        seeds.push(seed);
    }
    save_verifier_setup_seeds(path, &seeds)
}

/// The production trace-height bucket set the boot generator must cover: one
/// canonical MoE shape per reachable Layer-0 trace-height bucket (the §4.8 envelope
/// heights 2^13..2^20). Derived by sweeping consensus-valid MoE shapes and keeping,
/// for each distinct trace height (computed WITHOUT proving via
/// [`canonical_moe_trace_height`]), the first (cheapest) representative. One setup
/// per height covers BOTH dense and MoE blocks (the setup is height-keyed and
/// schedule-independent), so this MoE-derived table serves the whole accept-band.
///
/// The height climbs with the opened tile side `hw`, `k`, and `num_stripes = k/r`;
/// `m = n = e·hw` is the minimal MoE-valid width (each of `e` experts gets exactly
/// `hw` rows/cols) and `m,n` do not affect the trace height. Coverage of the full
/// 2^13..2^20 band is asserted cheaply (no proving) in
/// `production_verifier_setup_buckets_cover_the_envelope`.
pub fn production_verifier_setup_buckets() -> Vec<VerifierSetupBucketShape> {
    use std::collections::BTreeMap;
    const E: usize = 2;
    const TOP_K: usize = 1;
    let mut by_bucket: BTreeMap<usize, VerifierSetupBucketShape> = BTreeMap::new();
    // Prefer the SMALLEST opened tile `hw` that reaches each height (less opened work
    // ⇒ less proving memory/time). The setup is height-keyed and schedule-independent,
    // so one representative per height covers blocks mined at any valid shape of that
    // height (dense or MoE, any tile).
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
                    // Consensus caps the accept-band at AI_POW_MAX_TRACE_HEIGHT
                    // (2^19); the top-of-envelope 2^20 setup is not built (large-RAM
                    // only), and blocks above the cap are rejected. So the table is
                    // exactly the seven feasible buckets 2^13..2^19.
                    if th > ai_pow::params::AI_POW_MAX_TRACE_HEIGHT {
                        continue;
                    }
                    by_bucket.entry(th).or_insert(VerifierSetupBucketShape {
                        params,
                        hw,
                        e: E,
                        top_k: TOP_K,
                    });
                }
            }
        }
    }
    by_bucket.into_values().collect()
}
