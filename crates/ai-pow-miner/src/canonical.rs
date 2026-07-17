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
    compute_pearl_moe_ticket, derive_pearl_work_commitments, pearl_bitcoin_double_sha256_raw,
    PearlAuxInclusionProof, PearlIncompleteBlockHeader, PearlMiningConfig, PearlMoeParams,
    PearlNockchainAux, PearlPeriodicPattern, PearlPublicProofParams, PEARL_MMA_INT7XINT7_TO_INT32,
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

/// Base header timestamp; `extranonce == 0` reproduces the exact block the boot
/// verifier-setup builder and `ai_pow_accept_e2e` prove (byte-stable).
const CANONICAL_BASE_TIMESTAMP: u32 = 0x6677_8899;

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
    extranonce: u32,
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
    let (header, aux_inclusion) = setup_aux_inclusion(&aux_commitment, extranonce);
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
        ..
    } = canonical_moe_inputs(params, hw, e, top_k, nock_commit, extranonce)?;
    // Mirror the prover's ticket call exactly (expert 0, dot_product_len == k).
    compute_pearl_moe_ticket(
        &commitments.kappa,
        &commitments.h_a,
        &commitments.h_b,
        &a,
        &b,
        &routing,
        0,
        &inner,
        &local_b,
        n_e,
        params.k as usize,
        params.noise_rank as usize,
        params.k as usize,
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
    } = canonical_moe_inputs(params, hw, e, top_k, nock_commit, extranonce)?;

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
            block.run.ticket.jackpot_hash, j1,
            "certified jackpot must equal the cheap grind jackpot for the same extranonce"
        );

        // extranonce 0 back-compat: same wrapper result as prove_canonical_moe_block.
        let b0a = prove_canonical_moe_block(&params, 8, 2, 1, commit).expect("prove wrapper");
        let b0b = prove_canonical_moe_block_at(&params, 8, 2, 1, commit, 0).expect("prove at 0");
        assert_eq!(b0a.run.ticket.jackpot_hash, b0b.run.ticket.jackpot_hash);
        assert_eq!(b0a.run.ticket.jackpot_hash, j0);

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
    #[test]
    fn canonical_jackpot_is_pure_function_of_tile_and_seed() {
        let params = canonical_params();
        let commit = [0x77u8; 32];
        for xn in [0u32, 1, 5, 100] {
            let t = evaluate_canonical_moe_ticket(&params, 8, 2, 1, commit, xn).expect("ticket");
            let recomputed =
                ai_pow::pearl_compat::pearl_jackpot_hash(&t.tile_state, &t.s_a);
            assert_eq!(
                recomputed, t.jackpot_hash,
                "extranonce {xn}: jackpot must equal keyed_hash(tile_state, s_a)"
            );
        }
    }
}
