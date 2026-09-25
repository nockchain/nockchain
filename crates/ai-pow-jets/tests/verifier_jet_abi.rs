//! Real Hoon -> jet acceptance across the rule boundary. Compile
//! hoon/tests/ai-pow-jet-abi.hoon and set AI_POW_VERIFY_GATE_JAM to its output.

use ai_pow::difficulty::{attempt_wins, shape_work_factor_for, AI_POW_MAX_CONSENSUS_TARGET};
use ai_pow::params::MatmulParams;
use ai_pow_jets::setup::{build_verifier_setup_with_rules, prove_reference_moe_block_with_rules};
use ai_pow_jets::{init_ai_pow_verifier_setup, produce_ai_pow_hot_state};
use ai_pow_miner::certificate_noun::build_ai_pow_pearl_merge_moe_artifact_noun_from_node_with_rules;
use ai_pow_miner::reference::evaluate_reference_moe_jackpot;
use ai_pow_zk::proof_rules::ProofRules;
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
    let factor = shape_work_factor_for(8, 8, params.k, params.noise_rank).unwrap();
    let (attempt, commit) = (0..4096)
        .find_map(|attempt| {
            let hash = *blake3::hash(&commitment(attempt).jam()).as_bytes();
            let jackpot = evaluate_reference_moe_jackpot(&params, 8, 2, 1, hash, 0).unwrap();
            attempt_wins(&jackpot, &target, factor)
                .unwrap()
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

    // A missing v2 binding must execute the !! fallback, never return acceptance
    // or a normal block rejection under a mismatched kernel/binary deployment.
    let mut missing = context(false);
    let gate = load_gate(&mut missing, &gate_jam);
    let sample = T(&mut missing.stack, &[D(154_500), D(0), D(0), D(0)]);
    assert!(slam(&mut missing, gate, sample).is_err());

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
        artifact.cue_into(artifact_jam).unwrap();
        let artifact = artifact.copy_to_stack(&mut ctx.stack);
        let commit = commitment(attempt).copy_to_stack(&mut ctx.stack);
        let target = Atom::from_value(&mut ctx.stack, target.as_slice())
            .unwrap()
            .as_noun();
        for height in [153_500, 154_499, 154_500, 154_501, 154_499] {
            let sample = T(&mut ctx.stack, &[D(height), artifact, commit, target]);
            let result = slam(&mut ctx, gate, sample).expect("Hoon must reach the verifier jet");
            assert_eq!(
                result.as_direct().unwrap().data(),
                u64::from(rules != ProofRules::at_height(height)),
                "proof {rules:?} at height {height}"
            );
        }
    }
}
