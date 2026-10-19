use ai_pow_jets::AiPowVerifierSetup;
use ai_pow_miner::certificate_noun::{
    ai_pow_compact_recursive_certificate_from_node,
    build_ai_pow_pearl_merge_moe_artifact_noun_from_node_with_rules,
    decode_ai_pow_pearl_merge_artifact_jam_with_rules, CertificateNounLimits,
    PearlMergeAiPowArtifactShape,
};
use ai_pow_zk::proof_rules::ProofRules;

pub const ARTIFACT: &[u8] = include_bytes!("hardened-skewed-moe.jam");
pub const COMMIT: &[u8; 32] = include_bytes!("hardened-skewed-commit.bin");
pub const KEY_DIGEST: &[u8; 40] = include_bytes!("hardened-skewed-key-digest.bin");
pub const HEIGHTS: [u64; 4] = [169_999, 170_000, 170_001, 169_999];

pub fn attest(setup: &AiPowVerifierSetup) -> PearlMergeAiPowArtifactShape {
    assert_eq!(ARTIFACT.len(), 147_436);
    assert_eq!(
        blake3::hash(ARTIFACT).to_hex().as_str(),
        "0a3145f0986ce415f2f59150efa23c49d0dde4485476c90c544ceecd18370eed",
        "the historical proof must remain immutable",
    );
    assert_eq!(setup.context.proof_rules(), ProofRules::Hardened);
    assert_eq!(setup.trace_height, 8192);
    assert_eq!(setup.digest_bytes.as_slice(), KEY_DIGEST);
    assert_eq!(
        &setup
            .context
            .validate_setup_binding()
            .expect("trusted setup binding"),
        setup.context.verifier_key_digest(),
    );
    let artifact = decode_ai_pow_pearl_merge_artifact_jam_with_rules(
        ARTIFACT,
        CertificateNounLimits::default(),
        ProofRules::Hardened,
    )
    .expect("decode frozen Hardened artifact");
    let certificate =
        ai_pow_compact_recursive_certificate_from_node(&artifact.certificate.certificate)
            .expect("decode frozen recursive proof");
    assert_eq!(
        certificate.verifier_key_digest(),
        setup.context.verifier_key_digest()
    );
    assert_eq!(artifact.certificate.trace_height, setup.trace_height);
    assert_eq!(artifact.certificate.found_idx, 0);
    assert_eq!(&artifact.statement.aux.nock_block_commitment, COMMIT);
    let moe = artifact.moe.as_ref().expect("frozen artifact is MoE");
    assert_eq!(moe.routing_data.len(), 64);
    assert_eq!(
        moe.routing_data.iter().filter(|&&token| token == 0).count(),
        2
    );
    assert_eq!(
        moe.routing_data.iter().filter(|&&token| token == 1).count(),
        0
    );
    // Re-encoding checks the complete opaque nonce, statement, aux inclusion,
    // routing and certificate together, rather than substituting a new nonce.
    let encoded = build_ai_pow_pearl_merge_moe_artifact_noun_from_node_with_rules(
        &artifact.statement,
        &artifact.aux_inclusion,
        moe,
        &artifact.certificate.zk_params,
        artifact.certificate.found_idx,
        artifact.certificate.trace_height,
        &artifact.certificate.commitments,
        &artifact.certificate.public_inputs,
        &artifact.certificate.certificate,
        ProofRules::Hardened,
    )
    .expect("re-encode frozen artifact")
    .jam();
    assert_eq!(encoded.as_ref(), ARTIFACT);
    artifact
}
