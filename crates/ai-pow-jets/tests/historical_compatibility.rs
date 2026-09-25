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
    AiPowBlockVerifyOutcome, CertificateNounLimits,
};
use ai_pow_zk::proof_rules::ProofRules;

#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

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

    // Verify through the jet's block-artifact path using the height-selected rules.
    let outcome = verify_ai_pow_block_artifact_jam_with_rules(
        LEGACY_ARTIFACT,
        CertificateNounLimits::default(),
        LEGACY_COMMIT,
        &AI_POW_MAX_CONSENSUS_TARGET,
        AI_POW_VERIFY_MAX_PATTERN_LEN,
        &setup.context,
        &setup.digest_bytes,
        ProofRules::at_height(154_499),
    )
    .expect("an honest legacy proof must remain verifiable for historical blocks");
    assert!(matches!(outcome, AiPowBlockVerifyOutcome::Moe(_)));
    for height in [154_500, 154_501] {
        assert!(
            verify_ai_pow_block_artifact_jam_with_rules(
                LEGACY_ARTIFACT,
                CertificateNounLimits::default(),
                LEGACY_COMMIT,
                &AI_POW_MAX_CONSENSUS_TARGET,
                AI_POW_VERIFY_MAX_PATTERN_LEN,
                &setup.context,
                &setup.digest_bytes,
                ProofRules::at_height(height),
            )
            .is_err(),
            "legacy proof must use the selected rules at height {height}"
        );
    }
}

#[test]
fn miners_produce_the_selected_proof_version() {
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
        for height in [153_500, 154_499, 154_500, 154_501, 154_499] {
            let selected = ProofRules::at_height(height);
            let result = verify_ai_pow_block_artifact_jam_with_rules(
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
                selected == rules,
                "proof rules {rules:?}, candidate height {height}: {result:?}"
            );
        }
        let rebuilt = ai_pow_jets::setup::rebuild_verifier_setup_from_seed(block.seed)
            .expect("rebuild versioned setup seed");
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
