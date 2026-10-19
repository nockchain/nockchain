//! Real Hoon -> jet acceptance across the production rule boundaries.
//! Compile hoon/tests/ai-pow-jet-abi.hoon for AI_POW_VERIFY_GATE_JAM or
//! hoon/tests/ai-pow-consensus-batch-jet-abi.hoon for
//! AI_POW_CONSENSUS_BATCH_VERIFY_GATE_JAM. Run each ignored test in a fresh process.
//! The consensus-batch gate runs `consensus_batch_rejects_frozen_skew_and_preserves_honest_work`.

use ai_pow::difficulty::{attempt_wins, shape_work_factor_for, AI_POW_MAX_CONSENSUS_TARGET};
use ai_pow::params::MatmulParams;
use ai_pow_jets::setup::{build_verifier_setup_with_rules, prove_reference_moe_block_with_rules};
use ai_pow_jets::{init_ai_pow_verifier_setup, produce_ai_pow_hot_state};
use ai_pow_miner::certificate_noun::build_ai_pow_pearl_merge_moe_artifact_noun_from_node_with_rules;
use ai_pow_miner::reference::evaluate_reference_moe_jackpot;
use ai_pow_zk::proof_rules::{ProofRules, VerificationRules};
use nockapp::noun::slab::NounSlab;
use nockapp::utils::create_context;
use nockapp::AtomExt;
use nockvm::interpreter::Context;
use nockvm::jets::cold::Cold;
use nockvm::jets::hot::URBIT_HOT_STATE;
use nockvm::jets::util::{kick, slam};
use nockvm::jets::JetDispatchMode;
use nockvm::mem::NockStack;
use nockvm::noun::{Atom, Noun, D, T};

#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[path = "fixtures/pre_batch_two_moe/mod.rs"]
mod frozen_skew;

const RULE_SELECTION_HEIGHTS: &[u64] = &[
    126_000, 147_500, 153_500, 154_499, 154_500, 154_501, 169_999, 170_000, 170_001, 169_999,
    154_499, 154_500,
];

fn context(with_verifier: bool) -> Context {
    let mut stack = NockStack::new(32 << 20, 0);
    let cold = Cold::new(&mut stack);
    let mut hot = URBIT_HOT_STATE.to_vec();
    if with_verifier {
        hot.extend(produce_ai_pow_hot_state());
    }
    create_context(stack, &hot, cold, None, vec![], JetDispatchMode::Exact)
}

fn load_gate(context: &mut Context, jam: &[u8]) -> Noun {
    let mut slab: NounSlab = NounSlab::new();
    slab.cue_into(jam.to_vec().into())
        .expect("cue compiled gate");
    let trap = slab.copy_to_stack(&mut context.stack);
    kick(context, trap, D(2)).expect("evaluate compiled gate")
}

fn commitment(attempt: u64) -> NounSlab {
    let mut slab = NounSlab::new();
    let root = T(&mut slab, &[D(1), D(2), D(3), D(4), D(attempt)]);
    slab.set_root(root);
    slab
}

#[test]
#[ignore = "requires compiled Hoon gate and two honest MoE proofs; run in a fresh process"]
fn one_jet_preserves_height_selected_honest_proofs() {
    let path = std::env::var("AI_POW_VERIFY_GATE_JAM")
        .expect("compile hoon/tests/ai-pow-jet-abi.hoon and set AI_POW_VERIFY_GATE_JAM");
    let gate_jam = std::fs::read(path).expect("read compiled Hoon gate");
    let params = MatmulParams {
        m: 64,
        k: 1024,
        n: 64,
        noise_rank: 64,
        tile: 8,
        spot_checks: 1,
        difficulty_bits: 0,
    };
    let target = AI_POW_MAX_CONSENSUS_TARGET;
    let factor = shape_work_factor_for(8, 8, params.k, params.noise_rank)
        .expect("fixture shape should have a valid work factor");
    let (attempt, commit) = (0..4096)
        .find_map(|attempt| {
            let hash = *blake3::hash(&commitment(attempt).jam()).as_bytes();
            let jackpot = evaluate_reference_moe_jackpot(&params, 8, 2, 1, hash, 0)
                .expect("fixture parameters should produce a reference MoE jackpot");
            attempt_wins(&jackpot, &target, factor)
                .expect("fixture jackpot and target should support a winning-ticket check")
                .then_some((attempt, hash))
        })
        .expect("find an honest winning ticket bound to a structured commitment");
    let setups = [ProofRules::Legacy, ProofRules::Hardened]
        .into_iter()
        .map(|rules| {
            build_verifier_setup_with_rules(&params, 8, 2, 1, rules).expect("verifier-owned setup")
        })
        .collect();
    init_ai_pow_verifier_setup(setups).expect("fresh process-global setup");

    // The mandatory jet has no interpreting fallback under either active rule.
    for height in [154_500, 170_000] {
        let mut missing = context(false);
        let gate = load_gate(&mut missing, &gate_jam);
        let sample = T(&mut missing.stack, &[D(height), D(0), D(0), D(0)]);
        assert!(slam(&mut missing, gate, sample).is_err());
    }

    for rules in [ProofRules::Legacy, ProofRules::Hardened] {
        let block = prove_reference_moe_block_with_rules(&params, 8, 2, 1, commit, rules)
            .expect("prove honest block using selected rules");
        let artifact = build_ai_pow_pearl_merge_moe_artifact_noun_from_node_with_rules(
            &block.statement, &block.aux_inclusion, &block.moe_art, &block.certificate.zk_params,
            block.certificate.found_idx, block.certificate.trace_height,
            &block.certificate.commitments, &block.certificate.public_inputs,
            &block.certificate.certificate, rules,
        )
        .expect("encode honest artifact");
        let artifact_jam = artifact.jam();
        let mut ctx = context(true);
        let gate = load_gate(&mut ctx, &gate_jam);
        let mut artifact: NounSlab = NounSlab::new();
        artifact
            .cue_into(artifact_jam)
            .expect("honest artifact jam should decode into a noun");
        let artifact = artifact.copy_to_stack(&mut ctx.stack);
        let commit = commitment(attempt).copy_to_stack(&mut ctx.stack);
        let target = Atom::from_value(&mut ctx.stack, target.as_slice())
            .expect("consensus target bytes should form an atom")
            .as_noun();
        for &height in RULE_SELECTION_HEIGHTS {
            let sample = T(&mut ctx.stack, &[D(height), artifact, commit, target]);
            let result = slam(&mut ctx, gate, sample).expect("Hoon must reach the verifier jet");
            assert_eq!(
                result
                    .as_direct()
                    .expect("verifier jet result should be a direct boolean atom")
                    .data(),
                u64::from(rules != VerificationRules::at_height(height).proof_rules()),
                "proof {rules:?} at height {height}"
            );
        }
    }
}

#[test]
#[ignore = "requires compiled consensus-batch Hoon gate and real MoE proofs; run in a fresh process"]
fn consensus_batch_rejects_frozen_skew_and_preserves_honest_work() {
    use ai_pow::pearl_compat::PearlCompatError;
    use ai_pow_miner::certificate_noun::{
        decode_ai_pow_pearl_merge_artifact_jam_with_rules,
        verify_ai_pow_block_artifact_jam_with_verification_rules,
        verify_ai_pow_block_artifact_with_verification_rules, CertificateNounError,
        CertificateNounLimits,
    };

    let gate_jam = std::fs::read(
        std::env::var("AI_POW_CONSENSUS_BATCH_VERIFY_GATE_JAM")
            .expect("compile hoon/tests/ai-pow-consensus-batch-jet-abi.hoon"),
    )
    .expect("read compiled consensus-batch Hoon gate");
    let params = MatmulParams {
        m: 64,
        k: 1024,
        n: 64,
        noise_rank: 64,
        tile: 8,
        spot_checks: 1,
        difficulty_bits: 0,
    };
    let target = AI_POW_MAX_CONSENSUS_TARGET;
    let factor = shape_work_factor_for(8, 8, params.k, params.noise_rank)
        .expect("fixture shape should have a valid work factor");
    let (attempt, commit) = (0..4096)
        .find_map(|attempt| {
            let hash = *blake3::hash(&commitment(attempt).jam()).as_bytes();
            let jackpot = evaluate_reference_moe_jackpot(&params, 8, 2, 1, hash, 0)
                .expect("fixture parameters should produce a reference MoE jackpot");
            attempt_wins(&jackpot, &target, factor)
                .expect("fixture jackpot and target should support a winning-ticket check")
                .then_some((attempt, hash))
        })
        .expect("find honest winning work");
    let block =
        prove_reference_moe_block_with_rules(&params, 8, 2, 1, commit, ProofRules::Hardened)
            .expect("prove honest MoE work");
    let setup = build_verifier_setup_with_rules(&params, 8, 2, 1, ProofRules::Hardened)
        .expect("verifier-owned honest setup");
    let frozen = frozen_skew::attest(&setup);
    assert_eq!(
        &commit,
        frozen_skew::COMMIT,
        "both proofs use the same structured commitment"
    );
    assert_eq!(
        &block.statement.aux.nock_block_commitment,
        frozen_skew::COMMIT
    );
    assert_eq!(block.certificate.found_idx, frozen.certificate.found_idx);
    assert_eq!(block.certificate.trace_height, setup.trace_height);
    assert_eq!(
        block
            .run
            .verifier_context
            .validate_setup_binding()
            .expect("honest prover setup binding"),
        setup
            .context
            .validate_setup_binding()
            .expect("independent verifier setup binding"),
    );
    assert_eq!(
        block.run.verifier_context.verifier_key_digest(),
        setup.context.verifier_key_digest(),
    );
    assert_eq!(
        bincode::serde::encode_to_vec(&block.run.verifier_context, bincode::config::standard())
            .expect("serialize honest prover context"),
        bincode::serde::encode_to_vec(&setup.context, bincode::config::standard())
            .expect("serialize independently rebuilt verifier context"),
        "honest and frozen work must use the same full pinned context",
    );
    let honest = build_ai_pow_pearl_merge_moe_artifact_noun_from_node_with_rules(
        &block.statement,
        &block.aux_inclusion,
        &block.moe_art,
        &block.certificate.zk_params,
        0,
        block.certificate.trace_height,
        &block.certificate.commitments,
        &block.certificate.public_inputs,
        &block.certificate.certificate,
        ProofRules::Hardened,
    )
    .expect("honest hardened block should encode as an artifact noun")
    .jam();

    for (bytes, canonical) in [(honest.as_ref(), true), (frozen_skew::ARTIFACT, false)] {
        let artifact = decode_ai_pow_pearl_merge_artifact_jam_with_rules(
            bytes,
            CertificateNounLimits::default(),
            ProofRules::Hardened,
        )
        .expect("Hardened artifact jam should decode with Hardened rules");
        for &height in RULE_SELECTION_HEIGHTS {
            let rules = VerificationRules::at_height(height);
            let expected = rules.proof_rules() == ProofRules::Hardened
                && (canonical || rules != VerificationRules::ConsensusBatch);
            let jam_result = verify_ai_pow_block_artifact_jam_with_verification_rules(
                bytes,
                CertificateNounLimits::default(),
                &commit,
                &target,
                4096,
                &setup.context,
                &setup.digest_bytes,
                rules,
            );
            let shape_result = verify_ai_pow_block_artifact_with_verification_rules(
                &artifact,
                CertificateNounLimits::default(),
                &commit,
                &target,
                4096,
                &setup.context,
                &setup.digest_bytes,
                rules,
            );
            assert_eq!(
                jam_result.is_ok(),
                expected,
                "honest proof height={height} rules={rules:?}: {jam_result:?}"
            );
            assert_eq!(
                shape_result.is_ok(),
                expected,
                "honest proof height={height} rules={rules:?}: {shape_result:?}"
            );
            if !canonical && rules == VerificationRules::ConsensusBatch {
                for result in [jam_result, shape_result] {
                    assert!(matches!(
                        result,
                        Err(CertificateNounError::PearlMergeStatement(
                            PearlCompatError::MoeRoutingMultiplicityMismatch {
                                token: 0,
                                expected: 1,
                                actual: 2,
                            }
                        ))
                    ));
                }
            }
        }
    }
    init_ai_pow_verifier_setup(vec![setup]).expect("fresh process-global setup");
    for (bytes, canonical) in [(honest.as_ref(), true), (frozen_skew::ARTIFACT, false)] {
        let mut ctx = context(true);
        let gate = load_gate(&mut ctx, &gate_jam);
        let mut slab: NounSlab = NounSlab::new();
        slab.cue_into(bytes.to_vec().into())
            .expect("Hardened artifact jam should decode into a noun");
        let artifact = slab.copy_to_stack(&mut ctx.stack);
        let commit = commitment(attempt).copy_to_stack(&mut ctx.stack);
        let target = Atom::from_value(&mut ctx.stack, target.as_slice())
            .expect("consensus target bytes should form an atom")
            .as_noun();
        for height in frozen_skew::HEIGHTS {
            let rules = VerificationRules::at_height(height);
            let expected = rules.proof_rules() == ProofRules::Hardened
                && (canonical || rules != VerificationRules::ConsensusBatch);
            let sample = T(&mut ctx.stack, &[D(height), artifact, commit, target]);
            let result = slam(&mut ctx, gate, sample).expect("Hoon must reach the verifier jet");
            assert_eq!(
                result
                    .as_direct()
                    .expect("consensus-batch verifier jet result should be a direct boolean atom")
                    .data(),
                u64::from(!expected),
                "canonical={canonical} proof height={height}"
            );
        }
    }
    for bytes in [honest.as_ref(), frozen_skew::ARTIFACT] {
        let mut missing = context(false);
        let gate = load_gate(&mut missing, &gate_jam);
        let mut slab: NounSlab = NounSlab::new();
        slab.cue_into(bytes.to_vec().into())
            .expect("decode real proof without verifier jet");
        let artifact = slab.copy_to_stack(&mut missing.stack);
        let commit = commitment(attempt).copy_to_stack(&mut missing.stack);
        let target = Atom::from_value(&mut missing.stack, target.as_slice())
            .expect("target atom")
            .as_noun();
        let sample = T(&mut missing.stack, &[D(170_000), artifact, commit, target]);
        assert!(
            slam(&mut missing, gate, sample).is_err(),
            "the verifier jet is mandatory even with a real artifact and valid sample"
        );
    }
}
