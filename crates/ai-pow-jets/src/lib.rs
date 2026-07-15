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
    CertificateNounLimits, PearlMergeAiPowArtifactShape,
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

/// The boot-injected, proof-independent compact verifier setup for ONE trace
/// log-height bucket. The compact verifier setup (`context` + `digest`) is
/// deterministic from the trace height (the padded Layer-0 degree_bits), NOT the
/// full shape: many Pearl shapes share a bucket, and the Pearl envelope spans a
/// small, bounded set of buckets (degree_bits ≤ ~19). Supporting EVERY Pearl
/// combination therefore means a *table* of these, one per reachable bucket —
/// see [`init_ai_pow_verifier_setup`].
#[derive(serde::Serialize, serde::Deserialize)]
pub struct AiPowVerifierSetup {
    /// The Layer-0 trace height (power of two) this setup verifies. A cert whose
    /// `certificate.trace_height` equals this is verified with this setup.
    pub trace_height: usize,
    pub context: AiPowCompactBatchVerifierContext,
    /// Canonical 40-byte verifier-key/setup digest.
    pub digest_bytes: Vec<u8>,
}

/// The boot-injected verifier-setup TABLE — one [`AiPowVerifierSetup`] per Pearl
/// trace-height bucket. Keyed lookup is by the cert's `trace_height`.
static SETUP: OnceCell<Vec<AiPowVerifierSetup>> = OnceCell::new();

/// Resolve the setup for a given Layer-0 `trace_height` from the boot table.
/// Returns `None` if the table is uninjected or has no bucket for `trace_height`
/// (a full boot table covers every Pearl-envelope bucket, so a miss is a boot
/// config error, not a valid block).
pub fn ai_pow_verifier_setup_for(trace_height: usize) -> Option<&'static AiPowVerifierSetup> {
    SETUP.get()?.iter().find(|s| s.trace_height == trace_height)
}

/// Inject the compact verifier-setup TABLE once at node boot — one entry per
/// Pearl trace-height bucket. Each setup is deterministic from its trace height
/// and proof-independent (prove one canonical block per bucket; see
/// `ai-pow-miner` / `build_verifier_setup`), so building the table once and
/// reusing it for every block is sound. Rejects an empty table or duplicate
/// trace-height buckets.
///
/// Returns `Err` if already initialized (boot should call this exactly once) or
/// if the table is empty / has duplicate buckets.
pub fn init_ai_pow_verifier_setup(setups: Vec<AiPowVerifierSetup>) -> Result<(), ()> {
    let heights: Vec<usize> = setups.iter().map(|s| s.trace_height).collect();
    if !setup_table_heights_valid(&heights) {
        return Err(());
    }
    SETUP.set(setups).map_err(|_| ())
}

/// A verifier-setup table is well-formed iff it is non-empty and has no duplicate
/// trace-height bucket (each cert resolves to exactly one setup). Pure so the
/// admission rule is unit-testable without constructing real setups.
fn setup_table_heights_valid(heights: &[usize]) -> bool {
    if heights.is_empty() {
        return false;
    }
    for (i, &h) in heights.iter().enumerate() {
        if heights[..i].contains(&h) {
            return false; // duplicate bucket
        }
    }
    true
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

/// Verify an already-decoded `%ai-pow` block artifact given the resolved 32-byte
/// block commitment + target and an explicit setup. Factored out so it is
/// unit-testable without the boot cache. Returns `Ok(true)` iff the block verifies,
/// `Ok(false)` if it is well-formed but invalid.
pub fn ai_pow_verify_core(
    artifact: &PearlMergeAiPowArtifactShape,
    commit: [u8; 32],
    target: [u8; 32],
    setup: &AiPowVerifierSetup,
) -> Result<bool, JetErr> {
    let limits = CertificateNounLimits::default();
    match verify_ai_pow_block_artifact(
        artifact,
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
/// The artifact is decoded BEFORE the boot setup is required: a malformed/garbage
/// `%ai-pow` artifact is a rejected block (`NO`) that needs no setup — so a garbage
/// block cannot crash a node whose setup is still building, and the jet is
/// unit-testable end-to-end without a ~25 s setup prove. A *well-formed* artifact
/// with no setup injected does bail to the stubbed Hoon arm (`!!`) — the
/// jet-required design surfaces the boot bug rather than silently accepting.
pub fn ai_pow_verify_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    // sample = [artifact commit target]  ⇒  head=2, commit=6, target=7
    let sample = slot(subject, 6, &space)?;
    let artifact_noun = slot(sample, 2, &space)?;
    let commit_noun = slot(sample, 6, &space)?;
    let target_noun = slot(sample, 7, &space)?;

    // Decode first — garbage is a rejected block, not a jet failure, and needs no
    // setup. `PearlMergeAiPowArtifactShape` is an owned decode, so it survives the
    // stack mutation from `commit_from_noun` below.
    let limits = CertificateNounLimits::default();
    let artifact = match decode_ai_pow_pearl_merge_artifact_noun(artifact_noun, &space, limits) {
        Ok(a) => a,
        Err(_) => return Ok(NO),
    };
    let Some(target) = atom_to_32(target_noun, &space) else {
        return Ok(NO);
    };
    // Resolve the setup for THIS cert's trace-height bucket from the boot table.
    // A well-formed artifact whose bucket is absent (or the table uninjected)
    // falls back to the stubbed Hoon arm (`!!`) — surfaces an incomplete boot
    // table rather than silently rejecting a valid block. (A full table covers
    // every Pearl-envelope bucket; the decode already rejected malformed shapes.)
    let Some(setup) = ai_pow_verifier_setup_for(artifact.certificate.trace_height) else {
        return Err(BAIL_FAIL);
    };
    // Canonicalize the structured commitment noun (mutates the stack via jam).
    let commit = commit_from_noun(&mut context.stack, commit_noun);
    let verified = ai_pow_verify_core(&artifact, commit, target, setup)?;
    Ok(if verified { YES } else { NO })
}

/// Hot-state entry set for the AI-PoW verify jet. Appended to the nockchain kernel
/// hot state alongside `zkvm-jetpack`'s prover jets.
///
/// The Hoon `++ai-pow-verify` (`~/ %ai-pow-verify`) lives in the shared
/// `/common/pow` lib under a `~% %pow-lib ..ut ~` root (it cannot be a kernel
/// door arm — the `fort` mold fixes %dumb-inner to load/peek/poke — nor a
/// `|^`-nested arm, which fails cold registration). `..ut` resolves to the
/// hoon.hoon std-library prefix `[one two tri qua pen]` (confirmed by the
/// `%zeke`-anchored jets, e.g. cheetah `ser-a-pt`, which sit at
/// `[one two tri qua pen zeke ..]`). So `%pow-lib` sits at
/// `[one two tri qua pen pow-lib]` and the jetted arm at
/// `[one two tri qua pen pow-lib ai-pow-verify]`. Axis `1` is the `~/`-gate
/// convention (matches every base58 / ec-point `|=` jet). Runtime-validated by
/// the roswell `test-ai-pow-verify-jet-fires` unit test.
pub fn produce_ai_pow_hot_state() -> Vec<nockvm::jets::hot::HotEntry> {
    use either::Either::Left;
    use nockvm::jets::hot::K_138;
    vec![(
        &[
            K_138,
            Left(b"one"),
            Left(b"two"),
            Left(b"tri"),
            Left(b"qua"),
            Left(b"pen"),
            Left(b"pow-lib"),
            Left(b"ai-pow-verify"),
        ],
        1,
        ai_pow_verify_jet,
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The verifier-setup TABLE admission rule (supporting the full Pearl band):
    /// non-empty, one setup per trace-height bucket, no duplicates.
    #[test]
    fn setup_table_admission_rule() {
        assert!(!setup_table_heights_valid(&[]), "empty table rejected");
        assert!(setup_table_heights_valid(&[8192]), "single bucket ok");
        assert!(
            setup_table_heights_valid(&[8192, 16384, 32768]),
            "distinct buckets ok"
        );
        assert!(
            !setup_table_heights_valid(&[8192, 16384, 8192]),
            "duplicate bucket rejected (a cert must resolve to exactly one setup)"
        );
    }
}

#[cfg(test)]
mod jet_tests {
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
            trace_height: block.run.trace_height,
            context: block.run.verifier_context,
            digest_bytes,
        };
        let commit = block.commit;
        let loose_target = [0xffu8; 32];

        // Decode the artifact noun to the shape (what the jet does before verify).
        let slab = cue_artifact(jammed);
        let space = slab.noun_space();
        let root = unsafe { *slab.root() };
        let artifact = decode_ai_pow_pearl_merge_artifact_noun(
            root,
            &space,
            CertificateNounLimits::default(),
        )
        .expect("decode artifact noun");

        assert!(
            matches!(ai_pow_verify_core(&artifact, commit, loose_target, &setup), Ok(true)),
            "real MoE block must verify through the jet core",
        );
        assert!(
            matches!(
                ai_pow_verify_core(&artifact, [0x99u8; 32], loose_target, &setup),
                Ok(false)
            ),
            "wrong block commitment must be rejected",
        );
        assert!(
            matches!(ai_pow_verify_core(&artifact, commit, [0u8; 32], &setup), Ok(false)),
            "unmet difficulty must be rejected",
        );
    }

    /// KAT (real proving, ~25s): the ACCEPTANCE path with the block commitment
    /// derived exactly as the jet derives it in consensus — `commit_from_noun`
    /// (BLAKE3 of the nockvm jam) of a realistic block-commitment noun (a tip5
    /// 5-belt digest), NOT an arbitrary 32-byte constant. We prove a real cert
    /// against that noun-derived commit and confirm the jet-core ACCEPTS it, then
    /// confirm a different commitment noun (⇒ different commit) is rejected. This
    /// closes the `block-commitment noun → commit_from_noun → prove → verify=%.y`
    /// loop that the live +check-pow path exercises, with real proving — the
    /// acceptance-direction analog of `commit_from_noun_matches_miner_derivation`.
    #[test]
    #[ignore = "real MoE compact proof (~25s); opt-in"]
    fn jet_commit_from_noun_seeds_a_cert_the_core_accepts() {
        use nockvm::mem::NockStack;
        use nockvm::noun::{D, T};

        let params = MatmulParams {
            m: 64,
            k: 1024,
            n: 64,
            noise_rank: 64,
            tile: 8,
            spot_checks: 1,
            difficulty_bits: 0,
        };

        // A realistic block-commitment noun: a tip5 noun-digest is 5 belts.
        let mut stack = NockStack::new(8 << 20, 0);
        // Arbitrary belts (< 2^63 so they are valid direct atoms; the noun is
        // only jammed+hashed, so the exact values don't matter).
        let commit_noun = T(
            &mut stack,
            &[
                D(0x0123_4567_89ab_cdef),
                D(0x1122_3344_5566_7788),
                D(0x2233_4455_6677_8899),
                D(0x3344_5566_7788_99aa),
                D(0x4455_6677_8899_aabb),
            ],
        );
        // The jet's own commitment derivation (BLAKE3 of the nockvm jam).
        let commit = commit_from_noun(&mut stack, commit_noun);

        // Prove a real cert bound to that noun-derived commit (the miner's job).
        let block = prove_canonical_moe_block(&params, 8, 2, 1, commit)
            .expect("prove canonical MoE block for the noun-derived commit");
        assert_eq!(
            block.commit, commit,
            "the proved cert must commit to the jet-derived commitment",
        );

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
            trace_height: block.run.trace_height,
            context: block.run.verifier_context,
            digest_bytes,
        };

        let slab = cue_artifact(jammed);
        let space = slab.noun_space();
        let root = unsafe { *slab.root() };
        let artifact = decode_ai_pow_pearl_merge_artifact_noun(
            root,
            &space,
            CertificateNounLimits::default(),
        )
        .expect("decode artifact noun");

        let loose_target = [0xffu8; 32];
        // ACCEPT: the jet-derived commit matches the cert's commitment.
        assert!(
            matches!(ai_pow_verify_core(&artifact, commit, loose_target, &setup), Ok(true)),
            "real block must verify when the commit is derived from its commitment noun",
        );
        // REJECT: a different commitment noun yields a different commit.
        let mut stack2 = NockStack::new(8 << 20, 0);
        let other_noun = T(&mut stack2, &[D(1), D(2), D(3), D(4), D(5)]);
        let other_commit = commit_from_noun(&mut stack2, other_noun);
        assert_ne!(other_commit, commit, "distinct nouns ⇒ distinct commits");
        assert!(
            matches!(ai_pow_verify_core(&artifact, other_commit, loose_target, &setup), Ok(false)),
            "a block committed to a different noun must be rejected",
        );
    }

    /// KAT (real proving + rebuild, ~30s): the boot-setup SEED cache path — the
    /// C4 linchpin. Prove a real MoE block, serialize its SMALL rebuild seed,
    /// deserialize it, and rebuild the FULL verifier setup from it WITHOUT proving.
    /// The real block must verify through the jet CORE against the REBUILT
    /// (cached-seed) setup exactly as against the freshly-proved context, and a
    /// wrong commit must still be rejected. Also asserts the serialized seed is
    /// small (< 16 MiB) — the whole point of caching the seed, not the ~866 MB
    /// context. This proves a boot node can cache seeds and rebuild working setups.
    #[test]
    #[ignore = "real MoE compact proof + rebuild (~30s); opt-in"]
    fn moe_verifier_setup_seed_roundtrip_rebuilds_working_setup() {
        use crate::setup::{
            prove_canonical_moe_block, rebuild_verifier_setup_from_seed, CANONICAL_SETUP_COMMIT,
        };
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
        let commit = block.commit;

        // Serialize the SMALL seed; assert it is small (vs the ~866 MB context).
        let seed_bytes = bincode::serde::encode_to_vec(&block.seed, bincode::config::standard())
            .expect("serialize verifier-setup seed");
        assert!(
            seed_bytes.len() < 16 * 1024 * 1024,
            "cached seed must be small (< 16 MiB); got {} bytes",
            seed_bytes.len(),
        );

        // Build the block artifact noun (uses the freshly-proved cert; unchanged).
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

        // BOOT path: deserialize the seed and REBUILD the setup (no proving).
        let (seed2, _): (ai_pow_zk::recursion::AiPowCompactVerifierSetupSeed, _) =
            bincode::serde::decode_from_slice(&seed_bytes, bincode::config::standard())
                .expect("deserialize verifier-setup seed");
        let setup = rebuild_verifier_setup_from_seed(seed2).expect("rebuild setup from seed");
        assert_eq!(
            setup.trace_height, block.run.trace_height,
            "rebuilt setup trace height matches the proved cert",
        );
        let proved_digest = ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(
            &block.run.verifier_key_digest(),
        )
        .to_vec();
        assert_eq!(
            setup.digest_bytes, proved_digest,
            "rebuilt setup digest matches the proved cert digest",
        );

        let slab = cue_artifact(jammed);
        let space = slab.noun_space();
        let root = unsafe { *slab.root() };
        let artifact = decode_ai_pow_pearl_merge_artifact_noun(
            root,
            &space,
            CertificateNounLimits::default(),
        )
        .expect("decode artifact noun");

        let loose_target = [0xffu8; 32];
        assert!(
            matches!(ai_pow_verify_core(&artifact, commit, loose_target, &setup), Ok(true)),
            "real MoE block must verify against the REBUILT (cached-seed) setup",
        );
        assert!(
            matches!(
                ai_pow_verify_core(&artifact, [0x99u8; 32], loose_target, &setup),
                Ok(false)
            ),
            "wrong block commitment must still be rejected against the rebuilt setup",
        );

        // FILE cache path (C4b): save the seed to a data-dir-style cache file, load
        // + rebuild the TABLE from disk, and verify the SAME block against the
        // disk-loaded setup — the exact boot flow (cache in data dir → load →
        // rebuild → verify), end to end through a real file.
        let tmp_data_dir =
            std::env::temp_dir().join(format!("ai-pow-jets-seedcache-{}", std::process::id()));
        let cache_path = crate::setup::verifier_setup_seed_cache_path(&tmp_data_dir);
        crate::setup::save_verifier_setup_seeds(&cache_path, std::slice::from_ref(&block.seed))
            .expect("save seed cache to data-dir file");
        let table =
            crate::setup::load_verifier_setup_table(&cache_path).expect("load + rebuild seed table");
        let _ = std::fs::remove_dir_all(&tmp_data_dir);
        assert_eq!(table.len(), 1, "one-bucket table loaded from disk");
        assert_eq!(
            table[0].trace_height, block.run.trace_height,
            "disk-loaded setup trace height matches the proved cert",
        );
        assert!(
            matches!(ai_pow_verify_core(&artifact, commit, loose_target, &table[0]), Ok(true)),
            "real MoE block must verify against the DISK-loaded rebuilt setup",
        );
    }

    /// **DE-RISK — is the compact verifier setup shape-DEPENDENT?**
    /// Pearl admits a BAND of puzzle shapes; nockchain must verify all of them.
    /// A single embedded boot setup only suffices if the verifier-key digest is
    /// INVARIANT across shapes. This builds setups at several distinct shapes
    /// (varying k / hw / m,n — the axes that drive L0 trace height) and prints
    /// each digest, then asserts nothing (observational) so the run always shows
    /// the full table. If every digest is equal ⇒ one setup covers the band; if
    /// they differ ⇒ we need a per-shape setup table (or fixed-height padding).
    #[test]
    #[ignore = "builds several real compact proofs (~2-4 min); opt-in diagnostic"]
    fn digest_shape_dependence_probe() {
        use crate::setup::build_verifier_setup;
        // MoE routing needs m/e >= hw and n/e >= hw (each expert must have >= hw
        // rows/cols to fill the opened tile), so base at m=n=16, e=2, hw=8.
        let base = MatmulParams {
            m: 16, k: 1024, n: 16, noise_rank: 64, tile: 8, spot_checks: 1, difficulty_bits: 0,
        };
        // (label, params, hw, e, top_k). num_stripes = k/noise_rank; the pinned AIR
        // caps it at STRIPE_MAX=64. Span the band from num_stripes=8 to the max 64,
        // across k / rank / hw / m,n, to confirm ONE digest covers nockchain's whole
        // accept-band.
        let shapes: [(&str, MatmulParams, u32, usize, usize); 7] = [
            ("stripes16 base m16 k1024 r64 hw8", base, 8, 2, 1),
            ("stripes8 k512 r64", MatmulParams { k: 512, ..base }, 8, 2, 1),
            ("stripes16 k512 r32", MatmulParams { k: 512, noise_rank: 32, ..base }, 8, 2, 1),
            ("stripes32 k2048 r64", MatmulParams { k: 2048, ..base }, 8, 2, 1),
            ("stripes64 k2048 r32 (MAX)", MatmulParams { k: 2048, noise_rank: 32, ..base }, 8, 2, 1),
            ("stripes64 k4096 r64 (MAX)", MatmulParams { k: 4096, ..base }, 8, 2, 1),
            ("hw16 m32 n32", MatmulParams { m: 32, n: 32, ..base }, 16, 2, 1),
        ];
        let mut digests = Vec::new();
        for (label, params, hw, e, top_k) in shapes {
            match build_verifier_setup(&params, hw, e, top_k) {
                Ok(setup) => {
                    let hex: String =
                        setup.digest_bytes.iter().map(|b| format!("{b:02x}")).collect();
                    eprintln!("SHAPE-DIGEST [{label}] = {hex}");
                    digests.push((label, Some(hex)));
                }
                Err(e) => {
                    eprintln!("SHAPE-DIGEST [{label}] = BUILD-ERROR: {e}");
                    digests.push((label, None));
                }
            }
        }
        let distinct: std::collections::BTreeSet<_> =
            digests.iter().filter_map(|(_, d)| d.clone()).collect();
        eprintln!(
            "SHAPE-DIGEST SUMMARY: {} shapes built, {} DISTINCT digest(s) ⇒ {}",
            digests.iter().filter(|(_, d)| d.is_some()).count(),
            distinct.len(),
            if distinct.len() <= 1 {
                "SHAPE-INDEPENDENT (one setup covers all)"
            } else {
                "SHAPE-DEPENDENT (need a per-shape setup table)"
            },
        );
    }
}
