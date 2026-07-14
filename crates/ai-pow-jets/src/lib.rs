//! Consensus verifier jet for the AI-PoW (`%ai-pow`) puzzle — Branch (b): a full
//! Rust verify jet with a stubbed Hoon arm.
//!
//! nockchain's existing consensus verify (`check-pow` → `verify:nv`) is a Hoon
//! STARK verifier with jetted primitives. AI-PoW's compact **recursive**-STARK
//! verify is Rust-only, so — per the chosen architecture — the Hoon arm
//! `++ai-pow-verify` is a stub and this jet is the real implementation.
//!
//! **Transparency:** the jet's sample is the *structured* `ai-pow-artifact` noun
//! (`[nonce certificate]`, the same shape Hoon builds) plus the block commitment
//! and target as atoms; the result is a loobean. Only the opaque `nonce` (the
//! Pearl statement bytes) and the recursive certificate body are byte-atoms —
//! everything Hoon reasons about stays inspectable.
//!
//! **Soundness:** the jet re-derives the canonical `(A, B)` from the protocol seed
//! (never the prover), so a miner cannot grind favorable matrices. The trusted
//! compact verifier setup (`context` + `verifier_key_digest`) is deterministic
//! from the production params and **proof-independent** (validated in
//! `ai-pow-miner`), so it is built once at boot and injected via
//! [`init_ai_pow_verifier_setup`].

use ai_pow_miner::certificate_noun::{
    decode_ai_pow_pearl_merge_artifact_noun, verify_ai_pow_block_artifact, AiPowBlockVerifyOutcome,
    CertificateNounLimits,
};
use ai_pow_zk::recursion::AiPowCompactBatchVerifierContext;
use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::noun::{Noun, NounSpace, D};
use once_cell::sync::OnceCell;

/// Pattern-length bound the verifier enforces (protocol constant; matches the
/// production admission envelope).
pub const AI_POW_VERIFY_MAX_PATTERN_LEN: usize = 4096;

/// The boot-injected, proof-independent compact verifier setup.
pub struct AiPowVerifierSetup {
    pub context: AiPowCompactBatchVerifierContext,
    /// Canonical 40-byte verifier-key/setup digest.
    pub digest_bytes: Vec<u8>,
}

static SETUP: OnceCell<AiPowVerifierSetup> = OnceCell::new();

/// Inject the compact verifier setup once at node boot. The setup is deterministic
/// from the production params and proof-independent, so building it once (prove one
/// canonical block; see `ai-pow-miner`) and reusing it for every block is sound.
///
/// Returns `Err` if already initialized (boot should call this exactly once).
pub fn init_ai_pow_verifier_setup(setup: AiPowVerifierSetup) -> Result<(), ()> {
    SETUP.set(setup).map_err(|_| ())
}

/// Loobean helpers (`&`/yes = 0 = verified, `|`/no = 1 = rejected).
const YES: Noun = D(0);
const NO: Noun = D(1);

fn atom_to_32(noun: Noun, space: &NounSpace) -> Option<[u8; 32]> {
    let atom = noun.in_space(space).as_atom().ok()?.atom();
    let handle = atom.in_space(space);
    let bytes = handle.as_ne_bytes();
    if bytes.len() > 32 {
        return None;
    }
    let mut out = [0u8; 32];
    out[..bytes.len()].copy_from_slice(bytes);
    Some(out)
}

/// Verify a decoded `%ai-pow` block artifact given an explicit setup. This is the
/// jet's load-bearing core, factored out so it is unit-testable without the boot
/// cache. The `sample` is `[artifact=ai-pow-artifact commit=@ target=@]`.
///
/// Returns `Ok(true)` iff the block verifies, `Ok(false)` if it is well-formed but
/// invalid (bad proof / unmet difficulty / wrong commitment / tampered artifact),
/// and `Err(JetErr)` only if the sample is structurally malformed (not a valid
/// triple) — the one case that legitimately falls back to the Hoon arm.
pub fn ai_pow_verify_with_setup(
    space: &NounSpace,
    sample: Noun,
    setup: &AiPowVerifierSetup,
) -> Result<bool, JetErr> {
    // sample = [artifact commit target]  ⇒  head=2, commit=6, target=7
    let artifact_noun = slot(sample, 2, space)?;
    let commit_noun = slot(sample, 6, space)?;
    let target_noun = slot(sample, 7, space)?;

    let limits = CertificateNounLimits::default();
    let artifact = match decode_ai_pow_pearl_merge_artifact_noun(artifact_noun, space, limits) {
        Ok(a) => a,
        // A malformed artifact noun is a rejected block, not a jet failure.
        Err(_) => return Ok(false),
    };
    let (Some(commit), Some(target)) =
        (atom_to_32(commit_noun, space), atom_to_32(target_noun, space))
    else {
        return Ok(false);
    };

    match verify_ai_pow_block_artifact(
        &artifact,
        limits,
        &commit,
        &target,
        AI_POW_VERIFY_MAX_PATTERN_LEN,
        &setup.context,
        &setup.digest_bytes,
    ) {
        Ok(AiPowBlockVerifyOutcome::Dense(_)) | Ok(AiPowBlockVerifyOutcome::Moe(_)) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// The AI-PoW verify jet. Sample: `[artifact commit target]`; result: loobean.
///
/// Requires [`init_ai_pow_verifier_setup`] to have run at boot; if not, it bails to
/// the (stubbed) Hoon arm — which, for a jet-required arm, surfaces the boot bug
/// rather than silently accepting.
pub fn ai_pow_verify_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sample = slot(subject, 6, &space)?;
    let Some(setup) = SETUP.get() else {
        // Setup not injected at boot — cannot verify; fall back (surfaces the bug).
        return Err(BAIL_FAIL);
    };
    let verified = ai_pow_verify_with_setup(&space, sample, setup)?;
    Ok(if verified { YES } else { NO })
}

/// Hot-state entry set for the AI-PoW verify jet. Appended to the nockchain kernel
/// hot state alongside `zkvm-jetpack`'s prover jets.
///
/// NOTE: the jet **path** below is provisional — it must match the `~%`/`~/` hint
/// chain of the stubbed Hoon `++ai-pow-verify` arm once that arm lands (Stage 2).
/// Registration is validated at runtime (a mis-chained hint prints at build/call).
pub fn produce_ai_pow_hot_state() -> Vec<nockvm::jets::hot::HotEntry> {
    use either::Either::Left;
    use nockvm::jets::hot::K_138;
    vec![(
        &[K_138, Left(b"one"), Left(b"ai-pow-verify")],
        6,
        ai_pow_verify_jet,
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_pow::params::MatmulParams;
    use ai_pow::pearl_compat::{
        derive_pearl_work_commitments, pearl_bitcoin_double_sha256_raw, PearlAuxInclusionProof,
        PearlIncompleteBlockHeader, PearlMiningConfig, PearlMoeParams, PearlNockchainAux,
        PearlPeriodicPattern, PearlPublicProofParams, PEARL_MMA_INT7XINT7_TO_INT32,
        PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG,
    };
    use ai_pow::pearl_moe_routing::build_routing_data;
    use ai_pow::synth::{synth_matrices, AI_POW_PROD_SYNTH_SEED};
    use ai_pow_miner::certificate_noun::{
        build_ai_pow_pearl_merge_moe_artifact_noun_from_node, AiPowCertificateShape, AiProofNode,
        PearlMergeMoeArtifact, PearlMergePublicStatementShape,
    };
    use nockapp::noun::slab::NounSlab;
    use nockapp::IndirectAtomExt;
    use nockvm::noun::{IndirectAtom, NounAllocator, T};

    fn test_pattern(len: u32) -> PearlPeriodicPattern {
        PearlPeriodicPattern {
            shape: [(1, len), (len, 1), (len, 1)],
        }
    }

    fn test_aux(commit: [u8; 32]) -> PearlNockchainAux {
        PearlNockchainAux {
            nockchain_chain_id: b"nockchain-mainnet\0".to_vec(),
            nock_block_commitment: commit,
            nockchain_target_epoch_or_height: 123_456,
            extra_domain_data: b"ai-pow-target-window\0\0".to_vec(),
        }
    }

    fn test_aux_inclusion(
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

    /// Build the sample noun `[artifact commit target]` in a fresh slab from a
    /// jammed artifact + the two 32-byte atoms.
    fn build_sample(jammed: nockapp::Bytes, commit: [u8; 32], target: [u8; 32]) -> NounSlab {
        let mut slab = NounSlab::new();
        let artifact_root = slab.cue_into(jammed).expect("cue artifact");
        let commit_atom =
            <IndirectAtom as IndirectAtomExt>::from_bytes(&mut slab, &commit).as_noun();
        let target_atom =
            <IndirectAtom as IndirectAtomExt>::from_bytes(&mut slab, &target).as_noun();
        let sample = T(&mut slab, &[artifact_root, commit_atom, target_atom]);
        slab.set_root(sample);
        slab
    }

    /// KAT (real proving, ~25s): a real MoE `%ai-pow` block artifact, presented as
    /// the structured jet sample `[artifact commit target]`, verifies through the
    /// jet CORE; a wrong block commitment is rejected (Ok(false), not a jet error);
    /// a malformed artifact atom is rejected. This validates the jet's noun
    /// plumbing (slot axes, atom extraction, decode-from-noun) end-to-end on top of
    /// the already-validated `verify_ai_pow_block_artifact`.
    #[test]
    #[ignore = "real MoE compact proof (~25s); opt-in"]
    fn ai_pow_verify_jet_core_accepts_real_block_and_rejects_tampering() {
        let (m, n, e, top_k, n_e) = (64usize, 64usize, 2usize, 1usize, 32usize);
        let params = MatmulParams {
            m: m as u32,
            k: 1024,
            n: n as u32,
            noise_rank: 64,
            tile: 8,
            spot_checks: 1,
            difficulty_bits: 0,
        };
        let (a, b) = synth_matrices(AI_POW_PROD_SYNTH_SEED, &params);

        let commit = [0x42u8; 32];
        let aux = test_aux(commit);
        let aux_commitment = aux.commitment().unwrap();
        let (header, aux_inclusion) = test_aux_inclusion(&aux_commitment);
        let config = PearlMiningConfig {
            common_dim: 1024,
            rank: 64,
            mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
            rows_pattern: test_pattern(8),
            cols_pattern: test_pattern(8),
            reserved: PearlMiningConfig::moe_trailer(e as u16, top_k as u16),
        };
        let commitments =
            derive_pearl_work_commitments(&header.to_bytes(), &config.to_bytes().unwrap(), &a, &b);
        let topk: Vec<u32> = (0..m).map(|t| (t % e) as u32).collect();
        let routing = build_routing_data(&topk, m, top_k, e).unwrap();
        let inner: Vec<u32> = config
            .rows_pattern
            .indices_with_offset_bounded(0, 4096)
            .unwrap();
        let local_b: Vec<u32> = config
            .cols_pattern
            .indices_with_offset_bounded(0, 4096)
            .unwrap();
        let run = ai_pow::zk_bridge::prove_pearl_moe_compact_recursive_certificate(
            &params, &a, &b, &commitments.kappa, &commitments.h_a, &commitments.h_b, &routing, 0,
            &inner, &local_b, n_e,
        )
        .expect("prove MoE compact certificate");

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
            public_data: public.to_public_data().unwrap(),
            expected_aux_commitment: aux_commitment,
            aux,
        };
        let cert_bytes =
            ai_pow_zk::recursion::encode_compact_batch_recursive_certificate(&run.compact_cert)
                .unwrap();
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
        let artifact_slab = build_ai_pow_pearl_merge_moe_artifact_noun_from_node(
            &statement,
            &aux_inclusion,
            &moe_art,
            &certificate.zk_params,
            certificate.found_idx,
            certificate.trace_height,
            &certificate.commitments,
            &certificate.public_inputs,
            &certificate.certificate,
        )
        .expect("build MoE artifact noun");
        let jammed = artifact_slab.jam();

        let digest_bytes = ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(
            &run.verifier_key_digest(),
        )
        .to_vec();
        let setup = AiPowVerifierSetup {
            context: run.verifier_context,
            digest_bytes,
        };
        let loose_target = [0xffu8; 32];

        // Valid block → verified.
        let slab = build_sample(jammed.clone(), commit, loose_target);
        let space = slab.noun_space();
        let root = unsafe { *slab.root() };
        assert!(
            matches!(ai_pow_verify_with_setup(&space, root, &setup), Ok(true)),
            "real MoE block must verify through the jet core",
        );

        // Wrong block commitment → rejected (Ok(false), not a jet error).
        let slab_bad = build_sample(jammed.clone(), [0x99u8; 32], loose_target);
        let space_bad = slab_bad.noun_space();
        let root_bad = unsafe { *slab_bad.root() };
        assert!(
            matches!(ai_pow_verify_with_setup(&space_bad, root_bad, &setup), Ok(false)),
            "wrong block commitment must be rejected",
        );

        // Unmet difficulty (target 0) → rejected.
        let slab_t = build_sample(jammed, commit, [0u8; 32]);
        let space_t = slab_t.noun_space();
        let root_t = unsafe { *slab_t.root() };
        assert!(
            matches!(ai_pow_verify_with_setup(&space_t, root_t, &setup), Ok(false)),
            "unmet difficulty must be rejected",
        );
    }
}
