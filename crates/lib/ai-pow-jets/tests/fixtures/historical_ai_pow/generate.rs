//! Copy this file into `crates/ai-pow-jets/tests/` at the public revision below.
//! That revision has equivalent legacy implementation source for regeneration.
//! The fixture is an honest, target-winning proof; no proof fields are modified.

use std::path::PathBuf;
use std::process::Command;

use ai_pow::difficulty::{attempt_wins, shape_work_factor_for, AI_POW_MAX_CONSENSUS_TARGET};
use ai_pow::params::MatmulParams;
use ai_pow_jets::setup::prove_canonical_moe_block;
use ai_pow_jets::AI_POW_VERIFY_MAX_PATTERN_LEN;
use ai_pow_miner::canonical::evaluate_canonical_moe_jackpot;
use ai_pow_miner::certificate_noun::{
    build_ai_pow_pearl_merge_moe_artifact_noun_from_node, verify_ai_pow_block_artifact_jam,
    AiPowBlockVerifyOutcome, CertificateNounLimits,
};
use ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes;

#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

const SOURCE_EQUIVALENT_PUBLIC_REVISION: &str = "cbd9298f96584b14ab93074ecdf32d0ec212e50e";

#[test]
fn generate_honest_legacy_fixture() {
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("read source commit");
    assert!(head.status.success());
    assert_eq!(
        String::from_utf8(head.stdout).unwrap().trim(),
        SOURCE_EQUIVALENT_PUBLIC_REVISION
    );
    let output_dir = PathBuf::from(
        std::env::var_os("HISTORICAL_AI_POW_FIXTURE_DIR").expect("set fixture output directory"),
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
    let target = AI_POW_MAX_CONSENSUS_TARGET;
    let work_factor = shape_work_factor_for(8, 8, params.k, params.noise_rank).unwrap();
    let commit = (0u64..4096)
        .find_map(|attempt| {
            let mut commit = [0u8; 32];
            commit[..8].copy_from_slice(&attempt.to_le_bytes());
            let jackpot = evaluate_canonical_moe_jackpot(&params, 8, 2, 1, commit, 0)
                .expect("evaluate honest legacy MoE ticket");
            attempt_wins(&jackpot, &target, work_factor)
                .expect("valid target")
                .then_some(commit)
        })
        .expect("find a target-winning honest commitment");
    let block = prove_canonical_moe_block(&params, 8, 2, 1, commit).expect("prove legacy block");
    let artifact = build_ai_pow_pearl_merge_moe_artifact_noun_from_node(
        &block.statement, &block.aux_inclusion, &block.moe_art, &block.certificate.zk_params,
        block.certificate.found_idx, block.certificate.trace_height,
        &block.certificate.commitments, &block.certificate.public_inputs,
        &block.certificate.certificate,
    )
    .expect("encode legacy artifact")
    .jam();
    let digest = compact_batch_verifier_key_digest_to_bytes(&block.run.verifier_key_digest());
    let outcome = verify_ai_pow_block_artifact_jam(
        &artifact,
        CertificateNounLimits::default(),
        &commit,
        &target,
        AI_POW_VERIFY_MAX_PATTERN_LEN,
        &block.run.verifier_context,
        &digest,
    )
    .expect("legacy verifier must accept the fixture before it is saved");
    assert!(matches!(outcome, AiPowBlockVerifyOutcome::Moe(_)));

    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::write(output_dir.join("legacy-moe.jam"), &artifact).unwrap();
    std::fs::write(output_dir.join("commit.bin"), commit).unwrap();
    std::fs::write(output_dir.join("verifier-key-digest.bin"), &digest).unwrap();
    let rustc = Command::new("rustc").arg("-Vv").output().unwrap();
    assert!(rustc.status.success());
    let provenance = format!(
        "source_commit={SOURCE_EQUIVALENT_PUBLIC_REVISION}\nartifact_bytes={}\nartifact_blake3={}\ncommit_blake3={}\nverifier_key_digest_blake3={}\ntrace_height={}\nlegacy_verification=accepted\n{}",
        artifact.len(), blake3::hash(&artifact), blake3::hash(&commit), blake3::hash(&digest),
        block.run.trace_height, String::from_utf8(rustc.stdout).unwrap(),
    );
    std::fs::write(output_dir.join("provenance.txt"), &provenance).unwrap();
    println!("{provenance}");
}
