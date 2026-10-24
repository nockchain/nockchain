//! Cross-version acceptance test. Never regenerate the legacy proof with current code.
//! See fixtures/historical_ai_pow/README.md for provenance and coverage limits.

use ai_pow::difficulty::AI_POW_MAX_CONSENSUS_TARGET;
use ai_pow::params::MatmulParams;
use ai_pow_jets::setup::{build_verifier_setup_with_rules, prove_reference_moe_block_with_rules};
use ai_pow_jets::AI_POW_VERIFY_MAX_PATTERN_LEN;
use ai_pow_miner::certificate_noun::{
    ai_pow_compact_recursive_certificate_from_node,
    build_ai_pow_pearl_merge_moe_artifact_noun_from_node_with_rules,
    decode_ai_pow_pearl_merge_artifact_jam_with_rules, verify_ai_pow_block_artifact_jam_with_rules,
    verify_ai_pow_block_artifact_jam_with_verification_rules, AiPowBlockVerifyOutcome,
    CertificateNounLimits,
};
use ai_pow_zk::proof_rules::{ProofRules, VerificationRules};

#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[path = "fixtures/pre_batch_two_moe/mod.rs"]
mod frozen_skew;

#[test]
fn preserves_proven_pre_consensus_batch_hardened_acceptance() {
    use ai_pow::pearl_compat::PearlCompatError;
    use ai_pow_miner::certificate_noun::{
        verify_ai_pow_block_artifact_with_verification_rules, CertificateNounError,
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
    let setup = build_verifier_setup_with_rules(&params, 8, 2, 1, ProofRules::Hardened)
        .expect("independently rebuild the trusted Hardened setup");
    let artifact = frozen_skew::attest(&setup);
    for height in [154_499, 154_500].into_iter().chain(frozen_skew::HEIGHTS) {
        let rules = VerificationRules::at_height(height);
        assert_eq!(
            rules == VerificationRules::ConsensusBatch,
            height >= 170_000
        );
        let jam_result = verify_ai_pow_block_artifact_jam_with_verification_rules(
            frozen_skew::ARTIFACT,
            CertificateNounLimits::default(),
            frozen_skew::COMMIT,
            &AI_POW_MAX_CONSENSUS_TARGET,
            AI_POW_VERIFY_MAX_PATTERN_LEN,
            &setup.context,
            &setup.digest_bytes,
            rules,
        );
        let shape_result = verify_ai_pow_block_artifact_with_verification_rules(
            &artifact,
            CertificateNounLimits::default(),
            frozen_skew::COMMIT,
            &AI_POW_MAX_CONSENSUS_TARGET,
            AI_POW_VERIFY_MAX_PATTERN_LEN,
            &setup.context,
            &setup.digest_bytes,
            rules,
        );
        for result in [jam_result, shape_result] {
            match rules {
                VerificationRules::Hardened => assert!(
                    matches!(result, Ok(AiPowBlockVerifyOutcome::Moe(_))),
                    "frozen proof height={height}: {result:?}",
                ),
                VerificationRules::ConsensusBatch => assert!(
                    matches!(
                        result,
                        Err(CertificateNounError::PearlMergeStatement(
                            PearlCompatError::MoeRoutingMultiplicityMismatch {
                                token: 0,
                                expected: 1,
                                actual: 2,
                            }
                        ))
                    ),
                    "frozen proof height={height}: {result:?}",
                ),
                VerificationRules::Legacy => assert!(result.is_err()),
            }
        }
    }
}

const LEGACY_ARTIFACT: &[u8] = include_bytes!("fixtures/historical_ai_pow/legacy-moe.jam");
const LEGACY_COMMIT: &[u8; 32] = include_bytes!("fixtures/historical_ai_pow/commit.bin");
const LEGACY_KEY_DIGEST: &[u8] =
    include_bytes!("fixtures/historical_ai_pow/verifier-key-digest.bin");

#[test]
fn accepts_honest_legacy_moe_artifact() {
    assert_eq!(
        blake3::hash(LEGACY_ARTIFACT).to_hex().as_str(),
        "ba1ca5b90d167ad33edf83d58d0c30d6dd21dfb4d9a14173d568f47a32322ac3",
        "preserve the exact legacy proof bytes",
    );
    assert_eq!(
        blake3::hash(LEGACY_COMMIT).to_hex().as_str(),
        "129c83ed1f1996881b36db2b1f3fd521907662ad5fbc8dc0810d15d85cb4a09c",
    );
    assert_eq!(
        blake3::hash(LEGACY_KEY_DIGEST).to_hex().as_str(),
        "00cc46b2a2b67c93289a7b9b5d84b17818cd7081901822487a0797cdecc8cf95",
    );
    let params = MatmulParams {
        m: 64,
        k: 1024,
        n: 64,
        noise_rank: 64,
        tile: 8,
        spot_checks: 1,
        difficulty_bits: 0,
    };
    // This is verifier-owned setup from the implementation under test, not setup
    // supplied by the frozen proof. The fixture generator verifies its original output.
    let setup = build_verifier_setup_with_rules(&params, 8, 2, 1, ProofRules::Legacy)
        .expect("build verifier-owned setup");
    assert_eq!(setup.trace_height, 8192);
    assert_eq!(setup.context.proof_rules(), ProofRules::Legacy);
    let artifact = decode_ai_pow_pearl_merge_artifact_jam_with_rules(
        LEGACY_ARTIFACT,
        CertificateNounLimits::default(),
        ProofRules::Legacy,
    )
    .expect("decode frozen honest artifact");
    let certificate =
        ai_pow_compact_recursive_certificate_from_node(&artifact.certificate.certificate)
            .expect("decode frozen compact certificate");
    assert_eq!(setup.digest_bytes.as_slice(), LEGACY_KEY_DIGEST);
    assert_eq!(
        certificate.verifier_key_digest(),
        setup.context.verifier_key_digest(),
        "the fixture must reach proof verification with the expected setup digest",
    );
    assert_eq!(artifact.certificate.trace_height, setup.trace_height);

    // Verify immutable bytes with the complete production admission selector.
    for height in [
        126_000, 147_500, 154_499, 154_500, 154_501, 169_999, 170_000, 170_001, 169_999, 154_499,
    ] {
        let rules = VerificationRules::at_height(height);
        let result = verify_ai_pow_block_artifact_jam_with_verification_rules(
            LEGACY_ARTIFACT,
            CertificateNounLimits::default(),
            LEGACY_COMMIT,
            &AI_POW_MAX_CONSENSUS_TARGET,
            AI_POW_VERIFY_MAX_PATTERN_LEN,
            &setup.context,
            &setup.digest_bytes,
            rules,
        );
        assert_eq!(
            result.is_ok(),
            rules.proof_rules() == ProofRules::Legacy,
            "legacy proof at height {height}, rules={rules:?}: {result:?}",
        );
        if rules.proof_rules() == ProofRules::Legacy {
            assert!(matches!(result, Ok(AiPowBlockVerifyOutcome::Moe(_))));
        }
    }
}

#[test]
fn miners_produce_the_selected_proof_version() {
    // Warm the existing global pool before entering an embedding caller's
    // deliberately small-stack local pool. Never assume sole ownership of
    // global initialization when the full integration suite runs together.
    rayon::broadcast(|_| ());
    let foreign = rayon::ThreadPoolBuilder::new()
        .stack_size(2 * 1024 * 1024)
        .num_threads(1)
        .build()
        .expect("embedding caller pool");
    foreign.install(miners_produce_the_selected_proof_version_from_foreign_pool);
}

#[inline(never)]
fn miners_produce_the_selected_proof_version_from_foreign_pool() {
    let params = MatmulParams {
        m: 64,
        k: 1024,
        n: 64,
        noise_rank: 64,
        tile: 8,
        spot_checks: 1,
        difficulty_bits: 0,
    };
    for rules in [ProofRules::Legacy, ProofRules::Hardened] {
        let block = prove_reference_moe_block_with_rules(&params, 8, 2, 1, *LEGACY_COMMIT, rules)
            .expect("prove honest version-selected block");
        let jammed = build_ai_pow_pearl_merge_moe_artifact_noun_from_node_with_rules(
            &block.statement, &block.aux_inclusion, &block.moe_art, &block.certificate.zk_params,
            block.certificate.found_idx, block.certificate.trace_height,
            &block.certificate.commitments, &block.certificate.public_inputs,
            &block.certificate.certificate, rules,
        )
        .expect("encode honest artifact")
        .jam();
        if rules == ProofRules::Legacy {
            assert!(
                jammed.as_ref() == LEGACY_ARTIFACT,
                "legacy generation must preserve the original artifact bytes"
            );
        }
        let digest = ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(
            &block.run.verifier_key_digest(),
        );
        assert_eq!(
            digest.as_slice() == LEGACY_KEY_DIGEST,
            rules == ProofRules::Legacy,
            "each proof version must use its own verifier key"
        );
        for height in [
            126_000, 147_500, 153_500, 154_499, 154_500, 154_501, 169_999, 170_000, 170_001,
            169_999, 154_499,
        ] {
            let selected = VerificationRules::at_height(height);
            let result = verify_ai_pow_block_artifact_jam_with_verification_rules(
                &jammed,
                CertificateNounLimits::default(),
                LEGACY_COMMIT,
                &AI_POW_MAX_CONSENSUS_TARGET,
                AI_POW_VERIFY_MAX_PATTERN_LEN,
                &block.run.verifier_context,
                &digest,
                selected,
            );
            assert_eq!(
                result.is_ok(),
                selected.proof_rules() == rules,
                "proof rules {rules:?}, candidate height {height}: {result:?}"
            );
        }
        let rebuilt = build_verifier_setup_with_rules(&params, 8, 2, 1, rules)
            .expect("derive version-selected setup from its canonical shape");
        assert_eq!(rebuilt.trace_height, block.run.trace_height);
        assert_eq!(rebuilt.context.proof_rules(), rules);
        assert_eq!(rebuilt.digest_bytes.as_slice(), digest.as_slice());
        assert_eq!(
            rebuilt
                .context
                .validate_setup_binding()
                .expect("canonical setup binding"),
            block
                .run
                .verifier_context
                .validate_setup_binding()
                .expect("genuine prover setup binding"),
        );
        assert_eq!(
            bincode::serde::encode_to_vec(rebuilt.context.metadata(), bincode::config::standard())
                .expect("serialize canonical setup metadata"),
            bincode::serde::encode_to_vec(
                block.run.verifier_context.metadata(),
                bincode::config::standard(),
            )
            .expect("serialize genuine prover setup metadata"),
            "shape-only setup must preserve the genuine prover's canonical metadata",
        );
        // Serialization omits in-memory lookup expressions; genuine acceptance
        // with both contexts complements this native-context byte comparison.
        assert_eq!(
            bincode::serde::encode_to_vec(&rebuilt.context, bincode::config::standard())
                .expect("serialize canonical verifier context"),
            bincode::serde::encode_to_vec(&block.run.verifier_context, bincode::config::standard())
                .expect("serialize prover verifier context"),
            "shape-derived and proof-derived serialized verifier contexts under {rules:?}",
        );
        verify_ai_pow_block_artifact_jam_with_rules(
            &jammed,
            CertificateNounLimits::default(),
            LEGACY_COMMIT,
            &AI_POW_MAX_CONSENSUS_TARGET,
            AI_POW_VERIFY_MAX_PATTERN_LEN,
            &rebuilt.context,
            &rebuilt.digest_bytes,
            rules,
        )
        .expect("version-selected proof verifies with rebuilt context");
    }
}
