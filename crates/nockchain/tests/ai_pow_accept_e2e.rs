//! Positive end-to-end acceptance test for an AI-PoW (%ai-pow) block.
//!
//! Boots the real dumb consensus kernel in-process, drives the fakenet genesis
//! sequence, sets a low AI-PoW activation height, mines one candidate, proves a
//! REAL compact recursive certificate bound to that candidate's block commitment,
//! injects the matching verifier setup, and pokes the `%pow` `%ai-pow` submission.
//! `do-pow` verifies the certificate against the injected setup (via the mandatory
//! `++ai-pow-verify` jet) and, on success, admits the block through `+heard-block`.
//!
//! This test exercises the LIVE consensus kernel end to end and asserts:
//!   * the post-activation node emits a `%mine-ai` candidate (the AI work effect);
//!   * NEGATIVE — a real certificate bound to the WRONG block commitment is rejected
//!     by `do-pow` (the structurally-valid-but-invalid submission path); and
//!   * POSITIVE — a valid `%ai-pow` block is admitted through `do-pow -> heard-block`.
//!
//! The other adversarial cases are covered at the jet level (`ai-pow-jets::jet_tests`),
//! where they can be tested without a full kernel boot: over-cap trace-height reject,
//! unmet-difficulty reject, commit-noun binding, and malformed/undecodable-artifact
//! reject (`malformed_ai_pow_artifact_is_rejected_at_decode`).
//!
//! The single expensive step is proving one small MoE block (~30s); the setup is
//! rebuilt from the same proof's seed (no extra proving). Marked `#[ignore]`.

use ai_pow::params::MatmulParams;
use ai_pow_jets::setup::{prove_canonical_moe_block, rebuild_verifier_setup_from_seed, CanonicalBlock};
use ai_pow_jets::{
    ai_pow_verifier_setup_initialized, init_ai_pow_verifier_setup, produce_ai_pow_hot_state,
};
use ai_pow_miner::certificate_noun::build_ai_pow_pearl_merge_moe_artifact_noun_from_node;
use chaff::Chaff;
use nockapp::kernel::boot::{self, NockStackSize};
use nockapp::noun::slab::NounSlab;
use nockapp::utils::make_tas;
use nockapp::wire::{SystemWire, Wire};
use nockapp::{AtomExt, NockApp};
use nockchain::setup::{self, heard_fake_genesis_block, SetupCommand, FAKENET_GENESIS_MESSAGE};
use nockchain_types::Seconds;
use nockchain_math::belt::Belt;
use nockchain_math::crypto::cheetah::A_GEN;
use nockchain_mining_common::{MiningCandidate, MiningCandidateKind};
use nockchain_types::tx_engine::common::{Hash, SchnorrPubkey};
use nockchain_types::fakenet_blockchain_constants;
use nockvm::noun::{Atom, NounAllocator, D, T};
use nockvm_macros::tas;

const SIG: nockvm::noun::Noun = D(0);

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Small MoE puzzle shape — the miner-chosen matmul params for the test cert.
fn test_params() -> MatmulParams {
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

fn born_poke() -> NounSlab {
    let mut slab = NounSlab::new();
    let born = T(&mut slab, &[D(tas!(b"command")), D(tas!(b"born")), D(0)]);
    slab.set_root(born);
    slab
}

fn set_mining_key_poke() -> NounSlab {
    let mut slab = NounSlab::new();
    // A valid base58 schnorr pubkey (the curve generator A_GEN) and a valid base58
    // tip5 pkh. do-set-mining-key only requires both to decode; it does not check
    // pkh == hash(pubkey). (`tas!` only fits <=8-byte tags, so the >8-byte command
    // names use `make_tas`.)
    let pk = SchnorrPubkey(A_GEN).to_base58().expect("pubkey base58");
    let pkh = Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]).to_base58();
    let cmd = make_tas(&mut slab, "set-mining-key").as_noun();
    let v0 = Atom::from_value(&mut slab, pk.as_bytes()).unwrap().as_noun();
    let v1 = Atom::from_value(&mut slab, pkh.as_bytes()).unwrap().as_noun();
    let poke = T(&mut slab, &[D(tas!(b"command")), cmd, v0, v1]);
    slab.set_root(poke);
    slab
}

fn enable_mining_poke() -> NounSlab {
    let mut slab = NounSlab::new();
    let cmd = make_tas(&mut slab, "enable-mining").as_noun();
    let poke = T(&mut slab, &[D(tas!(b"command")), cmd, D(0)]);
    slab.set_root(poke);
    slab
}

fn timer_poke() -> NounSlab {
    let mut slab = NounSlab::new();
    let poke = T(&mut slab, &[D(tas!(b"command")), D(tas!(b"timer")), D(0)]);
    slab.set_root(poke);
    slab
}

fn heavy_n_path(height: u64) -> NounSlab {
    let mut slab = NounSlab::new();
    let path = T(&mut slab, &[D(tas!(b"heavy-n")), D(height), SIG]);
    slab.set_root(path);
    slab
}

fn heaviest_block_path() -> NounSlab {
    let mut slab = NounSlab::new();
    let tag = make_tas(&mut slab, "heaviest-block").as_noun();
    let path = T(&mut slab, &[tag, SIG]);
    slab.set_root(path);
    slab
}

/// Wrap the `[%ai-pow nonce cert]` artifact in a `[%command %pow ..]` poke,
/// mirroring `ai_pow_miner::run::build_ai_pow_pearl_merge_certificate_poke`.
fn pow_poke_from_artifact(artifact: &NounSlab) -> NounSlab {
    let artifact_space = artifact.noun_space();
    let mut slab = NounSlab::new();
    let art = slab.copy_into(unsafe { *artifact.root() }, &artifact_space);
    let payload = T(&mut slab, &[D(tas!(b"command")), D(tas!(b"pow")), art]);
    slab.set_root(payload);
    slab
}

/// Build the `[%ai-pow nonce cert]` artifact noun for a proved canonical block.
fn artifact_for_block(block: &CanonicalBlock) -> NounSlab {
    build_ai_pow_pearl_merge_moe_artifact_noun_from_node(
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
}

async fn drive_genesis(app: &mut NockApp<Chaff>) {
    // Fakenet constants with AI-PoW active from height 1 (genesis is height 0), and
    // a 1s candidate-update interval so a poke shortly after enable-mining re-emits
    // the candidate.
    let constants = fakenet_blockchain_constants(2, 1)
        .with_ai_pow_activation_height(1)
        .with_update_candidate_timestamp_interval(Seconds(1));
    setup::poke(app, SetupCommand::PokeFakenetConstants(Box::new(constants)))
        .await
        .expect("set-constants");
    setup::poke(app, SetupCommand::PokeSetGenesisSeal(FAKENET_GENESIS_MESSAGE.to_string()))
        .await
        .expect("set-genesis-seal");
    setup::poke(app, SetupCommand::PokeSetBtcData)
        .await
        .expect("btc-data");
    app.poke(SystemWire.to_wire(), born_poke())
        .await
        .expect("born");
    app.poke(SystemWire.to_wire(), heard_fake_genesis_block(None).unwrap())
        .await
        .expect("heard genesis");
}

#[tokio::test]
#[ignore = "boots the dumb kernel + proves one ai-pow block (~30s); opt-in"]
async fn ai_pow_valid_block_is_admitted() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut cli = boot::default_boot_cli(true);
    cli.data_dir = Some(tmp.path().to_path_buf());
    cli.stack_size = NockStackSize::Large;
    let mut hot = zkvm_jetpack::hot::produce_prover_hot_state();
    hot.extend(produce_ai_pow_hot_state());
    let mut app = boot::setup::<Chaff>(kernels_open_dumb::KERNEL, cli, hot.as_slice(), "nockchain", None)
        .await
        .expect("boot dumb kernel");

    drive_genesis(&mut app).await;
    // Genesis (height 0) must be admitted.
    assert!(
        app.peek_handle(heaviest_block_path()).await.unwrap().is_some(),
        "genesis must be admitted",
    );

    // Set a mining key + enable mining so the kernel builds the height-1 candidate
    // (do-enable-mining -> heard-new-block). The candidate's commitment is read below
    // from the %mine effect it re-emits after the update interval.
    app.poke(SystemWire.to_wire(), set_mining_key_poke()).await.expect("set-mining-key");
    app.poke(SystemWire.to_wire(), enable_mining_poke()).await.expect("enable-mining");
    assert!(
        !ai_pow_verifier_setup_initialized(),
        "run this test in a fresh process (it installs the process-global setup)",
    );

    let params = test_params();

    // ── NEGATIVE (done FIRST — a submission poke advances the candidate timestamp and
    // thus its commitment): a certificate bound to the WRONG commitment must be
    // REJECTED by do-pow. `check-pow` re-derives the candidate's real commitment and
    // the `0x99..`-bound cert fails the in-circuit binding, so the block is not
    // admitted. Its setup (same trace-height bucket) is injected once and reused below.
    let bad_block = prove_canonical_moe_block(&params, 8, 2, 1, [0x99u8; 32]).expect("prove wrong-commit block");
    let bad_artifact = artifact_for_block(&bad_block);
    let vsetup = rebuild_verifier_setup_from_seed(bad_block.seed).expect("rebuild setup");
    init_ai_pow_verifier_setup(vec![vsetup]).expect("inject setup");
    app.poke(SystemWire.to_wire(), pow_poke_from_artifact(&bad_artifact))
        .await
        .expect("poke wrong-commit %pow");
    assert!(
        app.peek_handle(heavy_n_path(1)).await.unwrap().is_none(),
        "a certificate bound to the wrong block commitment must be rejected by do-pow",
    );
    eprintln!("[negative] wrong-commit cert correctly rejected");

    // ── POSITIVE: read the CURRENT candidate commitment (fresh, after the negative
    // poke), prove a cert bound to it, and submit. No poke happens between the read and
    // the positive submission (only the ~30s prove), so the candidate — and its
    // commitment — is unchanged. do-pow verifies the cert against the injected setup
    // (via the ai-pow-verify jet) and admits the block through heard-block.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let effs = app.poke(SystemWire.to_wire(), timer_poke()).await.expect("timer");
    let candidate = effs
        .into_iter()
        .find_map(|s| MiningCandidate::from_effect_slab(s).ok().flatten())
        .expect("kernel emitted a %mine candidate");
    // Post-activation the node must emit an %mine-ai candidate (the AI-PoW work
    // effect). It is prepended ahead of the legacy %mine-zk effect, so the first
    // decoded candidate is the AI one.
    assert_eq!(
        candidate.kind,
        MiningCandidateKind::Ai,
        "post AI-PoW activation the node must emit a %mine-ai candidate",
    );
    let commit32: [u8; 32] = *blake3::hash(&candidate.block_header.jam()).as_bytes();

    let block = prove_canonical_moe_block(&params, 8, 2, 1, commit32).expect("prove ai-pow block");
    let artifact = artifact_for_block(&block);
    app.poke(SystemWire.to_wire(), pow_poke_from_artifact(&artifact))
        .await
        .expect("poke %pow %ai-pow");
    assert!(
        app.peek_handle(heavy_n_path(1)).await.unwrap().is_some(),
        "a valid %ai-pow block must be admitted through do-pow -> heard-block",
    );
    eprintln!("[positive] valid %ai-pow block ADMITTED at height 1 (commit {})", hex(&commit32));
}
