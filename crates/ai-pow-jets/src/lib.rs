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

pub mod setup;

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

/// Derive the 32-byte Nockchain block commitment from the kernel's
/// `block-commitment:page:t` **noun** exactly as the miner does
/// (`ai-pow-miner::derive_job_inputs`): `BLAKE3(jam(commitment-noun))`.
///
/// This is the soundness-critical representation binding: the kernel's commitment
/// is a tip5 5-`belt` digest (a structured noun), NOT a 32-byte atom, so the jet
/// canonicalizes it the same way the prover did. `nockvm::serialization::jam`
/// (here) and `NounSlab::jam` (the miner) are the same canonical jam, so the
/// BLAKE3 inputs — and thus the commitments — match.
pub fn commit_from_noun(stack: &mut nockvm::mem::NockStack, noun: Noun) -> [u8; 32] {
    let jammed = nockvm::serialization::jam(stack, noun);
    let space = stack.noun_space();
    let handle = jammed.in_space(&space);
    let full = handle.as_ne_bytes();
    // `as_ne_bytes` is word-padded; the miner hashes `NounSlab::jam()` which is the
    // canonical (trailing-zero-trimmed) jam. Trim to the same significant length so
    // BLAKE3 matches — a padding mismatch here would reject every valid block.
    let sig_len = full.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    *blake3::hash(&full[..sig_len]).as_bytes()
}

/// Verify a decoded `%ai-pow` block artifact given the resolved 32-byte block
/// commitment + target and an explicit setup. This is the jet's load-bearing
/// core, factored out so it is unit-testable without the boot cache.
///
/// Returns `Ok(true)` iff the block verifies, `Ok(false)` if it is well-formed but
/// invalid (bad proof / unmet difficulty / wrong commitment / tampered artifact),
/// and `Err(JetErr)` only if the artifact noun cannot even be slotted.
pub fn ai_pow_verify_core(
    space: &NounSpace,
    artifact_noun: Noun,
    commit: [u8; 32],
    target: [u8; 32],
    setup: &AiPowVerifierSetup,
) -> Result<bool, JetErr> {
    let limits = CertificateNounLimits::default();
    let artifact = match decode_ai_pow_pearl_merge_artifact_noun(artifact_noun, space, limits) {
        Ok(a) => a,
        // A malformed artifact noun is a rejected block, not a jet failure.
        Err(_) => return Ok(false),
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

/// The AI-PoW verify jet. Sample:
/// `[artifact=ai-pow-artifact commit=block-commitment:page:t target=@]`
/// — `commit` is the STRUCTURED commitment noun (canonicalized here via
/// `commit_from_noun`), `target` the `merge:bignum` LE atom the Hoon arm passes.
/// Result: loobean.
///
/// Requires [`init_ai_pow_verifier_setup`] to have run at boot; if not, it bails to
/// the (stubbed) Hoon arm — which, for a jet-required arm, surfaces the boot bug
/// rather than silently accepting.
pub fn ai_pow_verify_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    // sample = [artifact commit target]  ⇒  head=2, commit=6, target=7
    let sample = slot(subject, 6, &space)?;
    let artifact_noun = slot(sample, 2, &space)?;
    let commit_noun = slot(sample, 6, &space)?;
    let target_noun = slot(sample, 7, &space)?;
    let Some(target) = atom_to_32(target_noun, &space) else {
        return Ok(NO);
    };
    let Some(setup) = SETUP.get() else {
        // Setup not injected at boot — cannot verify; fall back (surfaces the bug).
        return Err(BAIL_FAIL);
    };
    // Canonicalize the structured commitment noun (mutates the stack via jam).
    let commit = commit_from_noun(&mut context.stack, commit_noun);
    let space = context.stack.noun_space();
    let verified = ai_pow_verify_core(&space, artifact_noun, commit, target, setup)?;
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
    use crate::setup::{prove_canonical_moe_block, CANONICAL_SETUP_COMMIT};
    use ai_pow::params::MatmulParams;
    use ai_pow_miner::certificate_noun::build_ai_pow_pearl_merge_moe_artifact_noun_from_node;
    use nockapp::noun::slab::NounSlab;
    use nockvm::noun::NounAllocator;

    /// Cue a jammed artifact into a fresh slab and return `(slab, root)`.
    fn cue_artifact(jammed: nockapp::Bytes) -> NounSlab {
        let mut slab: NounSlab = NounSlab::new();
        let root = slab.cue_into(jammed).expect("cue artifact");
        slab.set_root(root);
        slab
    }

    /// **Soundness KAT (fast, no proving): the commit representation binding.**
    /// The jet derives the 32-byte block commitment as `BLAKE3(jam(commit-noun))`
    /// via `nockvm::serialization::jam`; the miner (`derive_job_inputs`) uses
    /// `BLAKE3(NounSlab::jam(..))`. These must be byte-identical — including the
    /// trailing-zero trimming — or every valid block is rejected. This pins that.
    #[test]
    fn commit_from_noun_matches_miner_derivation() {
        use nockvm::mem::NockStack;
        use nockvm::noun::{D, T};
        for payload in [D(0), D(1), D(0xdead_beef_u64), D(0xff00_u64)] {
            // Miner path: build the noun in a NounSlab, hash its canonical jam.
            let mut slab: NounSlab = NounSlab::new();
            let s = T(&mut slab, &[D(1), D(2), D(3), payload]);
            slab.set_root(s);
            let miner = *blake3::hash(&slab.jam()).as_bytes();

            // Jet path: the same logical noun in a NockStack, via commit_from_noun.
            let mut stack = NockStack::new(8 << 20, 0);
            let k = T(&mut stack, &[D(1), D(2), D(3), payload]);
            let jet = commit_from_noun(&mut stack, k);

            assert_eq!(
                jet, miner,
                "jet BLAKE3(nockvm jam) must equal miner BLAKE3(NounSlab::jam)",
            );
        }
    }

    /// KAT (real proving, ~25s): a real MoE `%ai-pow` block artifact verifies
    /// through the jet CORE; a wrong commitment and an unmet difficulty are
    /// rejected (`Ok(false)`, not a jet error). Validates the artifact decode-from-
    /// noun + verify dispatch over the already-validated `verify_ai_pow_block_artifact`.
    #[test]
    #[ignore = "real MoE compact proof (~25s); opt-in"]
    fn ai_pow_verify_jet_core_accepts_real_block_and_rejects_tampering() {
        let params = MatmulParams {
            m: 64,
            k: 1024,
            n: 64,
            noise_rank: 64,
            tile: 8,
            spot_checks: 1,
            difficulty_bits: 0,
        };
        let block = prove_canonical_moe_block(&params, 8, 2, 1, CANONICAL_SETUP_COMMIT)
            .expect("prove canonical MoE block");

        let jammed = build_ai_pow_pearl_merge_moe_artifact_noun_from_node(
            &block.statement,
            &block.aux_inclusion,
            &block.moe_art,
            &block.certificate.zk_params,
            block.certificate.found_idx,
            block.certificate.trace_height,
            &block.certificate.commitments,
            &block.certificate.public_inputs,
            &block.certificate.certificate,
        )
        .expect("build MoE artifact noun")
        .jam();

        let digest_bytes = ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(
            &block.run.verifier_key_digest(),
        )
        .to_vec();
        let setup = AiPowVerifierSetup {
            context: block.run.verifier_context,
            digest_bytes,
        };
        let commit = block.commit;
        let loose_target = [0xffu8; 32];

        let slab = cue_artifact(jammed);
        let space = slab.noun_space();
        let root = unsafe { *slab.root() };

        assert!(
            matches!(ai_pow_verify_core(&space, root, commit, loose_target, &setup), Ok(true)),
            "real MoE block must verify through the jet core",
        );
        assert!(
            matches!(
                ai_pow_verify_core(&space, root, [0x99u8; 32], loose_target, &setup),
                Ok(false)
            ),
            "wrong block commitment must be rejected",
        );
        assert!(
            matches!(ai_pow_verify_core(&space, root, commit, [0u8; 32], &setup), Ok(false)),
            "unmet difficulty must be rejected",
        );
    }
}
