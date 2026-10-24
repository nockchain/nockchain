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
//!   * a post-activation node emits a `%mine-ai` candidate;
//!   * a structurally valid certificate bound to the wrong commitment is rejected;
//!   * a valid `%ai-pow` block is admitted through `do-pow -> heard-block`;
//!   * replaying that accepted certificate after the tip advances cannot prevent the
//!     current `%mine-zk` candidate from being admitted.
//!
//! The other adversarial cases are covered at the jet level (`ai-pow-jets::jet_tests`),
//! where they can be tested without a full kernel boot: over-cap trace-height reject,
//! unmet-difficulty reject, commit-noun binding, and malformed/undecodable-artifact
//! reject (`malformed_ai_pow_artifact_is_rejected_at_decode`).
//!
//! Real reference blocks supply the certificates. The verifier context is derived
//! independently from the canonical shape, serialized to disk, and injected
//! disk-paged. The jet pages it in during the first `check-pow` (read and
//! deserialize, no rebuild). This test is marked `#[ignore]`.
//!
//! `ai_pow_hardening_boundary_and_restart` instead uses the real height 154500
//! and a compiled sparse prerequisite state. It admits real Legacy AI at 154499,
//! rejects a real Hardened certificate that misses the immutable AI reset target,
//! and admits real ZK blocks at 154500/154501 across current-kernel state reloads.
//! A permissive threshold is used ONLY in an independent native-jet proof check:
//! it is never substituted for the target in the kernel. This is not positive
//! Hardened AI admission or a validation of the fixture's synthetic prehistory.
//!
//! Compile `hoon/tests/ai-pow-hardening-state.hoon` with `hoonc --arbitrary` and set
//! `AI_POW_HARDENING_STATE_JAM` to its absolute output path. Optionally set
//! `AI_POW_DUMB_KERNEL_JAM` to a freshly compiled production dumb kernel;
//! otherwise the test uses `kernels_open_dumb::KERNEL`. Run this test alone:
//! `cargo test -p nockchain --release --test ai_pow_accept_e2e
//! ai_pow_hardening_boundary_and_restart -- --ignored --exact --nocapture`.
//!
//! `ai_pow_170000_boundary_restart_and_reorg` uses
//! `hoon/tests/ai-pow-170000-state.hoon` via `AI_POW_170000_STATE_JAM`.
//! Its bounded prerequisite consists of accepted pages with synthetic artifacts.
//! Initial import, ordinary exports, and every reload retain enabled PoW.
//! The initial session finishes init before submitting current proofs.
//! A custom-network 169998 pin permits real Hardened AI admission and a
//! shorter/heavier sibling ZK reorganization. The AI reset target is production
//! exact; ZK uses the new 100e9*500 calibration with the fixture's wider
//! max-target numerator, not mainnet difficulty. Real custom-network ZK proofs
//! continue through 170000/170001 and restart. Genuine AI certificates miss
//! the live 170000/170001 targets; this is not positive post-reset AI admission,
//! proof-validated prehistory, production-difficulty ZK mining, or mainnet replay.
//! Run only this ignored test in a fresh process with a freshly built kernel.
//! Set `AI_POW_DUMB_KERNEL_JAM` to the final production kernel and run:
//! `cargo test -p nockchain --release --test ai_pow_accept_e2e
//! ai_pow_170000_boundary_restart_and_reorg -- --ignored --exact --nocapture`.

#![allow(clippy::unwrap_used)] // integration test: unwrap is acceptable
use std::path::Path;

use ai_pow::params::MatmulParams;
use ai_pow_jets::setup::{
    build_verifier_setup_with_rules, install_verifier_setup_disk_from_setups,
    prove_reference_moe_block_with_rules, ReferenceBlock,
};
use ai_pow_jets::{ai_pow_verifier_setup_initialized, ai_pow_verify_jet, produce_ai_pow_hot_state};
use ai_pow_miner::certificate_noun::{
    build_ai_pow_pearl_merge_moe_artifact_noun_from_node,
    build_ai_pow_pearl_merge_moe_artifact_noun_from_node_with_rules,
};
use ai_pow_miner::reference::{
    evaluate_reference_moe_jackpot, prove_reference_moe_block_at_with_rules,
};
use ai_pow_zk::proof_rules::{ProofRules, VerificationRules};
use chaff::Chaff;
use nockapp::export::ExportedState;
use nockapp::kernel::boot::{self, NockStackSize};
use nockapp::kernel::form::LoadState;
use nockapp::noun::slab::NounSlab;
use nockapp::utils::{create_context, make_tas};
use nockapp::wire::{SystemWire, Wire};
use nockapp::{AtomExt, NockApp};
use nockchain::setup::{self, heard_fake_genesis_block, SetupCommand, FAKENET_GENESIS_MESSAGE};
use nockchain_math::belt::Belt;
use nockchain_math::crypto::cheetah::A_GEN;
use nockchain_math::noun_ext::NounMathExtHandle;
use nockchain_mining_common::{MiningCandidate, MiningCandidateKind};
use nockchain_types::tx_engine::common::page::{BigNum, Page};
use nockchain_types::tx_engine::common::{Hash, SchnorrPubkey};
use nockchain_types::{fakenet_blockchain_constants, AsertParams, BlockchainConstants, Seconds};
use nockvm::interpreter::Context;
use nockvm::jets::cold::Cold;
use nockvm::jets::hot::URBIT_HOT_STATE;
use nockvm::jets::util::kick;
use nockvm::jets::JetDispatchMode;
use nockvm::mem::NockStack;
use nockvm::noun::{Atom, NounAllocator, D, T};
use nockvm_macros::tas;
use noun_serde::NounDecode;
use zk_pow_miner::worker::{build_candidate_poke, random_nonce};
use zk_pow_miner::{MineResult, SerfWorker, Worker};

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
    let v0 = Atom::from_value(&mut slab, pk.as_bytes())
        .unwrap()
        .as_noun();
    let v1 = Atom::from_value(&mut slab, pkh.as_bytes())
        .unwrap()
        .as_noun();
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

fn malformed_ai_pow_artifact_poke() -> NounSlab {
    let mut slab = NounSlab::new();
    let art = T(&mut slab, &[D(tas!(b"ai-pow")), D(0), D(0)]);
    let payload = T(&mut slab, &[D(tas!(b"command")), D(tas!(b"pow")), art]);
    slab.set_root(payload);
    slab
}

fn short_ai_pow_artifact_poke() -> NounSlab {
    let mut slab = NounSlab::new();
    let art = T(&mut slab, &[D(tas!(b"ai-pow")), D(0)]);
    let payload = T(&mut slab, &[D(tas!(b"command")), D(tas!(b"pow")), art]);
    slab.set_root(payload);
    slab
}

/// Build the `[%ai-pow nonce cert]` artifact noun for a proved canonical block.
fn artifact_for_block(block: &ReferenceBlock) -> NounSlab {
    build_ai_pow_pearl_merge_moe_artifact_noun_from_node(
        &block.statement, &block.aux_inclusion, &block.moe_art, &block.certificate.zk_params,
        block.certificate.found_idx, block.certificate.trace_height,
        &block.certificate.commitments, &block.certificate.public_inputs,
        &block.certificate.certificate,
    )
    .expect("build MoE artifact noun")
}

/// Same as [`artifact_for_block`] for a block proved by the MINER crate's
/// canonical path. The two crates keep separate copies of the canonical block
/// builder (ai-pow-jets depends on ai-pow-miner, so the dependency cannot run the
/// other way), and only the miner's takes an extranonce -- which grinding needs.
fn artifact_for_miner_block(block: &ai_pow_miner::reference::ReferenceBlock) -> NounSlab {
    build_ai_pow_pearl_merge_moe_artifact_noun_from_node(
        &block.statement, &block.aux_inclusion, &block.moe_art, &block.certificate.zk_params,
        block.certificate.found_idx, block.certificate.trace_height,
        &block.certificate.commitments, &block.certificate.public_inputs,
        &block.certificate.certificate,
    )
    .expect("build MoE artifact noun")
}

fn candidate_from_effects(
    effects: Vec<NounSlab>,
    expected_kind: MiningCandidateKind,
) -> MiningCandidate {
    effects
        .into_iter()
        .filter_map(|effect| MiningCandidate::from_effect_slab(effect).ok().flatten())
        .find(|candidate| candidate.kind == expected_kind)
        .unwrap_or_else(|| panic!("kernel emitted no {expected_kind:?} mining candidate"))
}

async fn mine_zk_candidate(candidate: &MiningCandidate) -> NounSlab {
    assert_eq!(
        candidate.kind,
        MiningCandidateKind::Zk,
        "the ZK miner requires a %mine-zk candidate"
    );
    let worker = SerfWorker::spawn(0, zkvm_jetpack::hot::produce_prover_hot_state())
        .await
        .expect("spawn ZK miner");
    let mut nonce = random_nonce();
    let command = loop {
        match worker
            .mine_attempt(build_candidate_poke(candidate, nonce))
            .await
            .expect("mine ZK candidate")
        {
            MineResult::Success { poke_slab, .. } => break poke_slab,
            MineResult::Retry { next_nonce } => nonce = next_nonce,
        }
    };
    worker.cancel();
    command
}

async fn drive_genesis(app: &mut NockApp<Chaff>) {
    drive_genesis_with_activation(app, 1).await
}

async fn drive_genesis_with_activation(app: &mut NockApp<Chaff>, ai_pow_activation_height: u64) {
    drive_genesis_with_activation_and_zk_target(
        app,
        ai_pow_activation_height,
        ibig::UBig::from(1u64) << 291,
    )
    .await
}

async fn drive_genesis_with_activation_and_zk_target(
    app: &mut NockApp<Chaff>,
    ai_pow_activation_height: u64,
    zk_target: ibig::UBig,
) {
    // Fakenet constants; AI-PoW activates at `ai_pow_activation_height` (genesis is
    // height 0), and a 1s candidate-update interval so a poke shortly after
    // enable-mining re-emits the candidate.
    // The AI ASERT must be the thing that sets an AI block's target. Left at the
    // mainnet defaults, `phase.zk-asert` is 65,500, so a height-1 AI block is
    // BELOW the ASERT phase and inherits the epoch target -- on a fresh chain
    // that is the genesis target (~2^318), far outside the domain in which the
    // verifier can scale a target by the tile shape factor. Such a block is
    // unminable regardless of work, so the test would be asserting admission of
    // a block no configuration can produce.
    //
    // Bring all the phases down to 1 so the AI ASERT governs from the first
    // block, and anchor it at the loosest target consensus can emit so a
    // canonical-shape jackpot is findable in a short grind. The kernel asserts
    // the phase orderings (see +load), so these have to move together.
    let asert = |phase: u64, anchor_height: u64, anchor_target_atom: ibig::UBig| AsertParams {
        phase,
        anchor_height,
        anchor_target_atom,
        ideal_block_time: 250,
        half_life: 43_200,
        anchor_min_timestamp: 0,
    };
    let max_minable_ai_target = (ibig::UBig::from(1u64) << 232) - ibig::UBig::from(1u64);
    let constants = fakenet_blockchain_constants(2, 1)
        .with_ai_pow_activation_height(ai_pow_activation_height)
        .with_zk_asert(asert(1, 0, zk_target.clone()))
        .with_zk_asert_post_ai(asert(1, 0, zk_target))
        .with_ai_asert(asert(1, 1, max_minable_ai_target))
        .with_update_candidate_timestamp_interval(Seconds(1));
    setup::poke(app, SetupCommand::PokeFakenetConstants(Box::new(constants)))
        .await
        .expect("set-constants");
    setup::poke(
        app,
        SetupCommand::PokeSetGenesisSeal(FAKENET_GENESIS_MESSAGE.to_string()),
    )
    .await
    .expect("set-genesis-seal");
    setup::poke(app, SetupCommand::PokeSetBtcData)
        .await
        .expect("btc-data");
    app.poke(SystemWire.to_wire(), born_poke())
        .await
        .expect("born");
    app.poke(
        SystemWire.to_wire(),
        heard_fake_genesis_block(None).unwrap(),
    )
    .await
    .expect("heard genesis");
}

#[tokio::test]
#[ignore = "boots the dumb kernel + proves one ai-pow block (~30s); opt-in"]
async fn ai_pow_valid_block_is_admitted() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut cli = boot::ephemeral_test_boot_cli(true);
    cli.data_dir = Some(tmp.path().to_path_buf());
    cli.stack_size = NockStackSize::Large;
    let mut hot = zkvm_jetpack::hot::produce_prover_hot_state();
    hot.extend(produce_ai_pow_hot_state());
    let mut app = boot::setup::<Chaff>(
        kernels_open_dumb::KERNEL,
        cli,
        hot.as_slice(),
        "nockchain",
        None,
    )
    .await
    .expect("boot dumb kernel");

    let max_zk_target = fakenet_blockchain_constants(2, 1).max_target_atom;
    drive_genesis_with_activation_and_zk_target(&mut app, 1, max_zk_target).await;
    // Genesis (height 0) must be admitted.
    assert!(
        app.peek_handle(heaviest_block_path())
            .await
            .unwrap()
            .is_some(),
        "genesis must be admitted",
    );

    // Set a mining key + enable mining so the kernel builds the height-1 candidate
    // (do-enable-mining -> heard-new-block). The candidate's commitment is read below
    // from the %mine effect it re-emits after the update interval.
    app.poke(SystemWire.to_wire(), set_mining_key_poke())
        .await
        .expect("set-mining-key");
    app.poke(SystemWire.to_wire(), enable_mining_poke())
        .await
        .expect("enable-mining");
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
    let bad_block =
        prove_reference_moe_block_with_rules(&params, 8, 2, 1, [0x99u8; 32], ProofRules::Legacy)
            .expect("prove wrong-commit block");
    let bad_artifact = artifact_for_block(&bad_block);
    // Inject the setup DISK-PAGED (production path): build the context, serialize it to
    // disk, and register it — the jet PAGES it in from disk during the first
    // `check-pow` (read + deserialize, no rebuild) and caches it.
    let vsetup = build_verifier_setup_with_rules(&params, 8, 2, 1, ProofRules::Legacy)
        .expect("derive context from the canonical shape");
    install_verifier_setup_disk_from_setups(vec![vsetup], tmp.path(), 2)
        .expect("inject disk-paged setup");
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
    let effs = app
        .poke(SystemWire.to_wire(), timer_poke())
        .await
        .expect("timer");
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
    assert_eq!(candidate.candidate_height, Some(1));
    let rules = ProofRules::at_height(candidate.candidate_height.unwrap());
    assert_eq!(rules, ProofRules::Legacy);
    let commit32: [u8; 32] = *blake3::hash(&candidate.block_header.jam()).as_bytes();

    // GRIND against the target the node actually handed out, using the same
    // predicate consensus applies: the jackpot clears `target * shape work
    // factor`, not the bare target. Proving a fixed extranonce instead would
    // only pass when the target admits essentially every jackpot, which no
    // legal target does -- at the loosest one consensus can emit the canonical
    // shape still needs ~2^8 attempts.
    let target = ai_pow_miner::run::decode_chain_target_bignum(&candidate.target)
        .expect("decode candidate target");
    let threshold = ai_pow_miner::run::reference_grind_threshold(&target).expect("grind threshold");
    let mut winning_extranonce = None;
    for extranonce in 0u32..100_000 {
        let jackpot = evaluate_reference_moe_jackpot(&params, 8, 2, 1, commit32, extranonce)
            .expect("grind attempt");
        if ai_pow::tile_hash::hash_le_target(&jackpot, &threshold) {
            winning_extranonce = Some(extranonce);
            break;
        }
    }
    let extranonce = winning_extranonce.expect(
        "no jackpot cleared the candidate target within the grind budget; the test          constants must admit a findable solution",
    );
    eprintln!("[positive] jackpot found at extranonce {extranonce}; proving (~30s)");

    let block =
        prove_reference_moe_block_at_with_rules(&params, 8, 2, 1, commit32, extranonce, rules)
            .expect("prove ai-pow block");
    let artifact = artifact_for_miner_block(&block);
    let post_ai_effects = app
        .poke(SystemWire.to_wire(), pow_poke_from_artifact(&artifact))
        .await
        .expect("poke %pow %ai-pow");
    assert!(
        app.peek_handle(heavy_n_path(1)).await.unwrap().is_some(),
        "a valid %ai-pow block must be admitted through do-pow -> heard-block",
    );
    eprintln!(
        "[positive] valid %ai-pow block ADMITTED at height 1 (commit {})",
        hex(&commit32)
    );

    // Replay the accepted height-1 certificate after the AI block advances the
    // tip. Rejection must leave ZK mining usable. The ordinary one-second
    // timestamp refresh may still emit replacement work after the rejection.
    let mut zk_candidate = candidate_from_effects(post_ai_effects, MiningCandidateKind::Zk);
    let stale_effects = app
        .poke(SystemWire.to_wire(), pow_poke_from_artifact(&artifact))
        .await
        .expect("poke stale %pow %ai-pow");
    if let Some(refreshed) = stale_effects
        .into_iter()
        .filter_map(|effect| MiningCandidate::from_effect_slab(effect).ok().flatten())
        .find(|candidate| candidate.kind == MiningCandidateKind::Zk)
    {
        eprintln!(
            "[negative] stale AI rejected; ZK commitment refreshed: {}",
            refreshed.block_header.jam() != zk_candidate.block_header.jam()
        );
        zk_candidate = refreshed;
    }
    assert!(
        app.peek_handle(heavy_n_path(2)).await.unwrap().is_none(),
        "a stale AI certificate must not admit height 2",
    );

    let zk_poke = mine_zk_candidate(&zk_candidate).await;
    app.poke(SystemWire.to_wire(), zk_poke)
        .await
        .expect("poke height-2 %pow %dumb-zkpow");
    assert!(
        app.peek_handle(heavy_n_path(2)).await.unwrap().is_some(),
        "a stale AI certificate must not block the current ZK candidate",
    );
}

#[tokio::test]
#[ignore = "boots the dumb kernel (~5s); opt-in"]
async fn malformed_ai_pow_artifact_is_rejected_without_admission() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut cli = boot::ephemeral_test_boot_cli(true);
    cli.data_dir = Some(tmp.path().to_path_buf());
    cli.stack_size = NockStackSize::Large;
    let mut hot = zkvm_jetpack::hot::produce_prover_hot_state();
    hot.extend(produce_ai_pow_hot_state());
    let mut app = boot::setup::<Chaff>(
        kernels_open_dumb::KERNEL,
        cli,
        hot.as_slice(),
        "nockchain",
        None,
    )
    .await
    .expect("boot dumb kernel");

    drive_genesis(&mut app).await;
    app.poke(SystemWire.to_wire(), set_mining_key_poke())
        .await
        .expect("set-mining-key");
    app.poke(SystemWire.to_wire(), enable_mining_poke())
        .await
        .expect("enable-mining");

    for (label, poke) in [
        (
            "undecodable nonce/certificate atoms",
            malformed_ai_pow_artifact_poke(),
        ),
        ("short ai-pow tuple", short_ai_pow_artifact_poke()),
    ] {
        app.poke(SystemWire.to_wire(), poke)
            .await
            .unwrap_or_else(|err| panic!("poke malformed %ai-pow ({label}): {err}"));
        assert!(
            app.peek_handle(heavy_n_path(1)).await.unwrap().is_none(),
            "a malformed %ai-pow artifact ({label}) must not admit height 1",
        );
    }
}

/// Consensus safety BELOW activation: `do-mine` must emit ONLY the legacy
/// `%mine-zk` candidate, never a `%mine-ai` one, while the candidate height is
/// below `ai-pow-activation-height`. A node that mined an AI block pre-activation
/// would produce a version-4 artifact that every node — upgraded or not — rejects
/// via `proof-version-valid-at-height`; refusing to emit the AI candidate at all
/// keeps a pre-activation node's mining effort on valid work and its behavior
/// identical to a pre-Logos node. Fast: no proving — only the candidate KIND is
/// inspected.
#[tokio::test]
#[ignore = "boots the dumb kernel (~5s); opt-in"]
async fn no_ai_candidate_below_activation() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut cli = boot::ephemeral_test_boot_cli(true);
    cli.data_dir = Some(tmp.path().to_path_buf());
    cli.stack_size = NockStackSize::Large;
    let mut hot = zkvm_jetpack::hot::produce_prover_hot_state();
    hot.extend(produce_ai_pow_hot_state());
    let mut app = boot::setup::<Chaff>(
        kernels_open_dumb::KERNEL,
        cli,
        hot.as_slice(),
        "nockchain",
        None,
    )
    .await
    .expect("boot dumb kernel");

    // AI-PoW activation set far above the height-1 candidate this node builds.
    drive_genesis_with_activation(&mut app, 100).await;
    assert!(
        app.peek_handle(heaviest_block_path())
            .await
            .unwrap()
            .is_some(),
        "genesis must be admitted",
    );

    app.poke(SystemWire.to_wire(), set_mining_key_poke())
        .await
        .expect("set-mining-key");
    app.poke(SystemWire.to_wire(), enable_mining_poke())
        .await
        .expect("enable-mining");

    // Re-emit the height-1 candidate after the 1s update interval.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let effs = app
        .poke(SystemWire.to_wire(), timer_poke())
        .await
        .expect("timer");
    let candidates: Vec<MiningCandidate> = effs
        .into_iter()
        .filter_map(|s| MiningCandidate::from_effect_slab(s).ok().flatten())
        .collect();
    assert!(
        !candidates.is_empty(),
        "the kernel must emit a mining candidate at height 1",
    );
    assert!(
        candidates.iter().all(|c| c.kind == MiningCandidateKind::Zk),
        "below AI-PoW activation the node must emit only %mine-zk candidates, never \
         %mine-ai (got {:?})",
        candidates.iter().map(|c| c.kind).collect::<Vec<_>>(),
    );
    eprintln!(
        "[pre-activation] {} candidate(s) emitted at height 1, all %mine-zk",
        candidates.len()
    );
}

fn boundary_context() -> Context {
    let mut stack = NockStack::new(32 << 20, 0);
    let cold = Cold::new(&mut stack);
    let mut hot = URBIT_HOT_STATE.to_vec();
    hot.extend(zkvm_jetpack::hot::produce_prover_hot_state());
    hot.extend(produce_ai_pow_hot_state());
    create_context(stack, &hot, cold, None, vec![], JetDispatchMode::Exact)
}

fn write_boundary_seed(kernel: &[u8], destination: &Path) {
    write_boundary_seed_from_env(kernel, destination, "AI_POW_HARDENING_STATE_JAM");
}

fn write_boundary_seed_from_env(kernel: &[u8], destination: &Path, variable: &str) {
    let fixture = std::env::var_os(variable)
        .unwrap_or_else(|| panic!("compile the sparse state helper and set {variable}"));
    let fixture = Path::new(&fixture);
    assert!(fixture.is_absolute(), "fixture jam path must be absolute");
    let mut slab: NounSlab = NounSlab::new();
    slab.cue_into(std::fs::read(fixture).expect("read seed trap").into())
        .expect("cue seed trap");
    let mut context = boundary_context();
    let trap = slab.copy_to_stack(&mut context.stack);
    let state = kick(&mut context, trap, D(2)).expect("evaluate sparse prerequisite state");
    let state = NounSlab::from_noun(state, &context.stack.noun_space());
    let exported = ExportedState::from_loadstate::<Chaff>(LoadState {
        ker_hash: blake3::hash(kernel),
        event_num: 0,
        kernel_state: state,
    });
    std::fs::write(
        destination,
        exported.encode().expect("encode prerequisite state"),
    )
    .expect("write prerequisite state");
}

async fn boot_boundary_state(kernel: &[u8], root: &Path, state: &Path) -> NockApp<Chaff> {
    let mut cli = boot::ephemeral_test_boot_cli(true);
    cli.data_dir = Some(root.to_path_buf());
    cli.state_jam = Some(state.to_str().expect("state path UTF-8").to_owned());
    cli.stack_size = NockStackSize::Large;
    let mut hot = zkvm_jetpack::hot::produce_prover_hot_state();
    hot.extend(produce_ai_pow_hot_state());
    let mut app = boot::setup::<Chaff>(kernel, cli, &hot, "nockchain", None)
        .await
        .expect("import state through current production kernel +load");

    let mut path = NounSlab::new();
    let tag = make_tas(&mut path, "constants").as_noun();
    let root = T(&mut path, &[tag, SIG]);
    path.set_root(root);
    let constants = app
        .peek_handle(path)
        .await
        .unwrap()
        .expect("active constants");
    // BlockchainConstants intentionally omits this legacy flag from its Rust
    // API. Check its persisted noun slot too: v1 field 6, then v0 field 9.
    let space = constants.noun_space();
    let v0 = nockvm::jets::util::slot(unsafe { *constants.root() }, 126, &space).unwrap();
    let check_pow = nockvm::jets::util::slot(v0, 1022, &space).unwrap();
    assert_eq!(
        check_pow.as_direct().unwrap().data(),
        0,
        "the imported state's explicit proof-checking mode must be retained"
    );
    let constants =
        BlockchainConstants::from_noun(unsafe { constants.root() }, &constants.noun_space())
            .expect("decode active constants");
    assert_eq!(constants.max_target_atom, ibig::UBig::from(1u64) << 384);
    app
}

async fn boundary_candidates(app: &mut NockApp<Chaff>, height: u64) -> Vec<MiningCandidate> {
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let candidates: Vec<_> = app
        .poke(SystemWire.to_wire(), timer_poke())
        .await
        .expect("refresh current mining candidates")
        .into_iter()
        .filter_map(|effect| MiningCandidate::from_effect_slab(effect).ok().flatten())
        .collect();
    for kind in [MiningCandidateKind::Ai, MiningCandidateKind::Zk] {
        let candidate = candidates
            .iter()
            .find(|candidate| candidate.kind == kind)
            .unwrap_or_else(|| panic!("no {kind:?} candidate at {height}"));
        if kind == MiningCandidateKind::Ai {
            assert_eq!(candidate.candidate_height, Some(height));
        } else {
            // The production ZK effect has no height field. Its selected proof
            // version must nevertheless be the real post-147500 version.
            assert_eq!(
                unsafe { candidate.version.root() }
                    .as_direct()
                    .unwrap()
                    .data(),
                5
            );
        }
    }
    candidates
}

fn boundary_artifact(block: &ai_pow_miner::reference::ReferenceBlock) -> NounSlab {
    build_ai_pow_pearl_merge_moe_artifact_noun_from_node_with_rules(
        &block.statement, &block.aux_inclusion, &block.moe_art, &block.certificate.zk_params,
        block.certificate.found_idx, block.certificate.trace_height,
        &block.certificate.commitments, &block.certificate.public_inputs,
        &block.certificate.certificate, block.rules,
    )
    .expect("encode rule-explicit MoE artifact")
}

fn grind_boundary_proof(
    candidate: &MiningCandidate,
    threshold: &[u8; 32],
    rules: ProofRules,
) -> ai_pow_miner::reference::ReferenceBlock {
    let params = test_params();
    let commit = *blake3::hash(&candidate.block_header.jam()).as_bytes();
    let extranonce = (0..100_000)
        .find(|&extranonce| {
            let jackpot = evaluate_reference_moe_jackpot(&params, 8, 2, 1, commit, extranonce)
                .expect("compute candidate-bound jackpot");
            ai_pow::tile_hash::hash_le_target(&jackpot, threshold)
        })
        .expect("find jackpot at the explicitly supplied test threshold");
    prove_reference_moe_block_at_with_rules(&params, 8, 2, 1, commit, extranonce, rules)
        .expect("prove genuine candidate-bound MoE certificate")
}

fn boundary_jet_accepts(
    artifact: &NounSlab,
    commitment: &NounSlab,
    target: &[u8; 32],
    rules: ProofRules,
) -> bool {
    boundary_jet_accepts_with_verification_rules(artifact, commitment, target, rules.into())
}

fn boundary_jet_accepts_with_verification_rules(
    artifact: &NounSlab,
    commitment: &NounSlab,
    target: &[u8; 32],
    rules: VerificationRules,
) -> bool {
    let mut context = boundary_context();
    let artifact = artifact.clone().copy_to_stack(&mut context.stack);
    let commitment = commitment.clone().copy_to_stack(&mut context.stack);
    let target = Atom::from_value(&mut context.stack, target.as_slice())
        .unwrap()
        .as_noun();
    let rules = match rules {
        VerificationRules::Legacy => D(tas!(b"legacy")),
        VerificationRules::Hardened => D(tas!(b"hardened")),
        VerificationRules::ConsensusBatch => D(tas!(b"consrule")),
    };
    let sample = T(&mut context.stack, &[rules, artifact, commitment, target]);
    let subject = T(&mut context.stack, &[D(0), sample, D(0)]);
    ai_pow_verify_jet(&mut context, subject)
        .expect("production verifier jet must execute, not bail")
        .as_direct()
        .expect("jet loobean")
        .data()
        == 0
}

async fn export_boundary_state(app: &NockApp<Chaff>, destination: &Path) {
    let exported = ExportedState::from_loadstate::<Chaff>(
        app.export().await.expect("export accepted chain state"),
    );
    std::fs::write(
        destination,
        exported.encode().expect("encode exported state"),
    )
    .expect("write current-kernel export");
}

/// Real verification at literal height 154500, with honest limitations:
/// sparse prehistory is fabricated; 154499 onward is not. The AI reset target
/// is fixed at roughly 2^256 / (10^19 * 214), before ASERT adjustment. A small
/// 2^16-work-factor proof would need roughly 3.27e16 jackpot trials. We prove
/// the certificate under Hardened rules, demonstrate its cryptographic
/// validity independently, and require the kernel to reject its insufficient
/// work. Real ZK blocks exercise admission and accepted-state reload.
#[tokio::test]
#[ignore = "compiled sparse state + genuine Legacy/Hardened AI and ZK proofs; fresh process only"]
async fn ai_pow_hardening_boundary_and_restart() {
    assert!(
        !ai_pow_verifier_setup_initialized(),
        "run only this test in a fresh process"
    );
    let tmp = tempfile::TempDir::new().unwrap();
    let kernel = std::env::var_os("AI_POW_DUMB_KERNEL_JAM")
        .map(|path| std::fs::read(path).expect("read current production dumb kernel"))
        .unwrap_or_else(|| kernels_open_dumb::KERNEL.to_vec());
    eprintln!("[boundary] production kernel {}", blake3::hash(&kernel));
    let seed_path = tmp.path().join("sparse-prerequisite.jam");
    write_boundary_seed(&kernel, &seed_path);
    let mut app = boot_boundary_state(&kernel, &tmp.path().join("initial"), &seed_path).await;
    assert!(app
        .peek_handle(heavy_n_path(154_498))
        .await
        .unwrap()
        .is_some());
    assert!(app
        .peek_handle(heavy_n_path(154_499))
        .await
        .unwrap()
        .is_none());
    app.poke(SystemWire.to_wire(), set_mining_key_poke())
        .await
        .unwrap();
    app.poke(SystemWire.to_wire(), enable_mining_poke())
        .await
        .unwrap();

    let setups = [ProofRules::Legacy, ProofRules::Hardened]
        .into_iter()
        .map(|rules| {
            build_verifier_setup_with_rules(&test_params(), 8, 2, 1, rules)
                .expect("build verifier-owned rule-specific setup")
        })
        .collect();
    // One resident bucket forces real disk paging when the rule version changes.
    install_verifier_setup_disk_from_setups(setups, tmp.path(), 1)
        .expect("install production disk-paged verifier setups");

    let candidates = boundary_candidates(&mut app, 154_499).await;
    let legacy_candidate = candidates
        .iter()
        .find(|c| c.kind == MiningCandidateKind::Ai)
        .unwrap();
    assert_eq!(ProofRules::at_height(154_499), ProofRules::Legacy);
    let legacy_target =
        ai_pow_miner::run::decode_chain_target_bignum(&legacy_candidate.target).unwrap();
    let legacy_threshold = ai_pow_miner::run::reference_grind_threshold(&legacy_target).unwrap();
    let legacy = grind_boundary_proof(legacy_candidate, &legacy_threshold, ProofRules::Legacy);
    let legacy_artifact = boundary_artifact(&legacy);
    app.poke(
        SystemWire.to_wire(),
        pow_poke_from_artifact(&legacy_artifact),
    )
    .await
    .expect("submit real Legacy AI block at 154499");
    let retained_legacy = app
        .peek_handle(heavy_n_path(154_499))
        .await
        .unwrap()
        .expect("real Legacy AI block admitted")
        .jam();

    // A Legacy certificate for the CURRENT height-154500 commitment is valid
    // under Legacy rules at a permissive threshold, but not under Hardened.
    // This isolates the version control independently of the strict live target.
    let permissive_target = ai_pow::difficulty::AI_POW_MAX_CONSENSUS_TARGET;
    let factor = ai_pow::difficulty::shape_work_factor_for(8, 8, 1024, 64).unwrap();
    let permissive_threshold =
        ai_pow::difficulty::effective_jackpot_threshold(&permissive_target, factor).unwrap();
    let candidates = boundary_candidates(&mut app, 154_500).await;
    let candidate = candidates
        .iter()
        .find(|c| c.kind == MiningCandidateKind::Ai)
        .unwrap();
    assert_eq!(
        ProofRules::at_height(candidate.candidate_height.unwrap()),
        ProofRules::Hardened
    );
    let wrong_version = grind_boundary_proof(candidate, &permissive_threshold, ProofRules::Legacy);
    let wrong_version = boundary_artifact(&wrong_version);
    assert!(boundary_jet_accepts(
        &wrong_version,
        &candidate.block_header,
        &permissive_target,
        ProofRules::Legacy
    ));
    assert!(!boundary_jet_accepts(
        &wrong_version,
        &candidate.block_header,
        &permissive_target,
        ProofRules::Hardened
    ));
    app.poke(SystemWire.to_wire(), pow_poke_from_artifact(&wrong_version))
        .await
        .expect("submit wrong-rule certificate at actual boundary");
    assert!(app
        .peek_handle(heavy_n_path(154_500))
        .await
        .unwrap()
        .is_none());

    // Refresh after the negative submission; no poke occurs between reading this
    // commitment, proving it and submitting it. The commitment includes the
    // actual live AI target; only the independent jet oracle uses a looser target.
    let candidates = boundary_candidates(&mut app, 154_500).await;
    let candidate = candidates
        .iter()
        .find(|c| c.kind == MiningCandidateKind::Ai)
        .unwrap();
    let target = ai_pow_miner::run::decode_chain_target_bignum(&candidate.target).unwrap();
    let threshold = ai_pow_miner::run::reference_grind_threshold(&target).unwrap();
    let hardened = grind_boundary_proof(candidate, &permissive_threshold, ProofRules::Hardened);
    assert!(
        !ai_pow::tile_hash::hash_le_target(&hardened.jackpot_hash, &threshold),
        "this negative control must really miss the kernel's fixed reset target"
    );
    let hardened_artifact = boundary_artifact(&hardened);
    assert!(
        boundary_jet_accepts(
            &hardened_artifact,
            &candidate.block_header,
            &permissive_target,
            ProofRules::Hardened
        ),
        "the bound Hardened certificate must be cryptographically valid"
    );
    assert!(
        !boundary_jet_accepts(
            &hardened_artifact,
            &legacy_candidate.block_header,
            &permissive_target,
            ProofRules::Hardened
        ),
        "wrong commitment must fail independently of difficulty"
    );
    assert!(
        !boundary_jet_accepts(
            &hardened_artifact,
            &candidate.block_header,
            &permissive_target,
            ProofRules::Legacy
        ),
        "Hardened proof must not verify under the Legacy setup"
    );
    assert!(
        !boundary_jet_accepts(
            &hardened_artifact,
            &candidate.block_header,
            &target,
            ProofRules::Hardened
        ),
        "the production jet must reject this proof at the kernel's actual target"
    );
    app.poke(
        SystemWire.to_wire(),
        pow_poke_from_artifact(&hardened_artifact),
    )
    .await
    .expect("submit genuine Hardened certificate at actual reset target");
    assert!(
        app.peek_handle(heavy_n_path(154_500))
            .await
            .unwrap()
            .is_none(),
        "valid cryptography does not waive the live chain difficulty"
    );
    eprintln!("[boundary] genuine Hardened proof rejected at actual target (little-endian) {}; not AI admission", hex(&target));
    let candidates = boundary_candidates(&mut app, 154_500).await;
    let current = candidates
        .iter()
        .find(|c| c.kind == MiningCandidateKind::Ai)
        .unwrap();
    assert_ne!(
        *blake3::hash(&current.block_header.jam()).as_bytes(),
        hardened.commit,
        "timer refresh must make the old certificate a wrong-commit control"
    );
    app.poke(
        SystemWire.to_wire(),
        pow_poke_from_artifact(&hardened_artifact),
    )
    .await
    .expect("submit stale Hardened commitment");
    assert!(
        app.peek_handle(heavy_n_path(154_500))
            .await
            .unwrap()
            .is_none(),
        "a wrong-commit Hardened certificate must not admit a block"
    );

    let mut retained = vec![(154_499, retained_legacy)];
    for height in [154_500, 154_501] {
        let candidates = boundary_candidates(&mut app, height).await;
        let zk = candidates
            .iter()
            .find(|c| c.kind == MiningCandidateKind::Zk)
            .unwrap();
        let poke = mine_zk_candidate(zk).await;
        app.poke(SystemWire.to_wire(), poke)
            .await
            .expect("submit real boundary ZK proof");
        retained.push((
            height,
            app.peek_handle(heavy_n_path(height))
                .await
                .unwrap()
                .expect("real ZK block admitted")
                .jam(),
        ));

        let export = tmp.path().join(format!("accepted-{height}.jam"));
        export_boundary_state(&app, &export).await;
        drop(app);
        app = boot_boundary_state(
            &kernel,
            &tmp.path().join(format!("restart-{height}")),
            &export,
        )
        .await;
        for (accepted_height, page) in &retained {
            assert_eq!(
                app.peek_handle(heavy_n_path(*accepted_height))
                    .await
                    .unwrap()
                    .expect("accepted proof-bearing page survives current-kernel load")
                    .jam(),
                *page,
                "restart must retain the exact admitted page at {accepted_height}",
            );
        }
        // Mining state is restored by +load; do not repair it by setting a key
        // or enabling mining again. The next iteration SOLVES this next height.
        let candidates = boundary_candidates(&mut app, height + 1).await;
        assert!(candidates
            .iter()
            .filter(|c| c.kind == MiningCandidateKind::Ai)
            .all(|c| ProofRules::at_height(c.candidate_height.unwrap()) == ProofRules::Hardened));
        eprintln!("[boundary] real ZK height {height} retained after current-kernel export/load");
    }
}

fn constants_path() -> NounSlab {
    let mut slab = NounSlab::new();
    let tag = make_tas(&mut slab, "constants").as_noun();
    let root = T(&mut slab, &[tag, SIG]);
    slab.set_root(root);
    slab
}

fn boundary_target_value(target: &NounSlab) -> ibig::UBig {
    let value = BigNum::from_noun(unsafe { target.root() }, &target.noun_space())
        .expect("decode the complete lane target, without AI u256 saturation");
    ibig::UBig::from_le_bytes(&value.0.to_bytes_le())
}

/// The widened custom-network ZK domain admits every genuine V5 digest.
/// Require that property and exactly one attempt, rather than silently starting
/// an unbounded production-difficulty search. STARK construction remains real.
async fn mine_boundary_smoke_zk(candidate: &MiningCandidate) -> NounSlab {
    assert_eq!(candidate.kind, MiningCandidateKind::Zk);
    assert_eq!(candidate.pow_len, 64, "retain the production puzzle length");
    let prime = ibig::UBig::from(18_446_744_069_414_584_321u64);
    let prime_squared = &prime * &prime;
    let digest_ceiling = &prime_squared * &prime_squared * prime - ibig::UBig::from(1u64);
    assert!(
        boundary_target_value(&candidate.target) >= digest_ceiling,
        "this bounded smoke requires the disclosed custom-network target domain"
    );
    let worker = SerfWorker::spawn(0, zkvm_jetpack::hot::produce_prover_hot_state())
        .await
        .expect("spawn real ZK prover");
    let result = worker
        .mine_attempt(build_candidate_poke(candidate, random_nonce()))
        .await
        .expect("construct a real V5 proof");
    worker.cancel();
    match result {
        MineResult::Success { poke_slab, .. } => poke_slab,
        MineResult::Retry { .. } => {
            panic!("a digest below the domain ceiling must clear this target")
        }
    }
}

fn assert_170000_reset_candidates(candidates: &[MiningCandidate], fixture_max_target: &ibig::UBig) {
    let zk = candidates
        .iter()
        .find(|c| c.kind == MiningCandidateKind::Zk)
        .unwrap();
    let ai = candidates
        .iter()
        .find(|c| c.kind == MiningCandidateKind::Ai)
        .unwrap();
    let zk_target = boundary_target_value(&zk.target);
    let ai_target = ai_pow_miner::run::decode_chain_target_bignum(&ai.target).unwrap();
    assert_eq!(
        zk_target,
        fixture_max_target / ibig::UBig::from(100_000_000_000u64 * 500),
        "use the 170000 ZK calibration with the disclosed custom-network numerator"
    );
    assert_eq!(
        ibig::UBig::from_le_bytes(&ai_target),
        (ibig::UBig::from(1u64) << 256)
            / (ibig::UBig::from(20_000_000_000_000_000_000u128) * ibig::UBig::from(214u64)),
        "the AI reset must use 20 ExaMAC/s without changing the earlier pin"
    );
}

/// Current proof-bearing admissions and an actual shorter/heavier reorg.
/// The bounded prerequisite has synthetic artifacts. Initial import and every
/// reload retain enabled PoW while preserving accepted pages and derived targets.
/// AI keeps its production reset target. ZK uses the new 100e9 hashes/s reference
/// and unchanged 500-second cadence with a wider maximum target, not mainnet difficulty.
#[tokio::test]
#[ignore = "compiled bounded prerequisite + real AI/ZK proofs; fresh process only"]
async fn ai_pow_170000_boundary_restart_and_reorg() {
    assert!(
        !ai_pow_verifier_setup_initialized(),
        "run only this test in a fresh process"
    );
    // This isolated real-proof test needs kernel/verifier rejection diagnostics.
    // RUST_LOG selects the existing boot logger's targets and verbosity.
    boot::init_default_tracing(&boot::ephemeral_test_boot_cli(true));
    let tmp = tempfile::TempDir::new().unwrap();
    let kernel = std::env::var_os("AI_POW_DUMB_KERNEL_JAM")
        .map(|path| std::fs::read(path).expect("read final production dumb kernel"))
        .unwrap_or_else(|| kernels_open_dumb::KERNEL.to_vec());
    eprintln!(
        "[170000] kernel {}; prerequisite proofs are not validated",
        blake3::hash(&kernel)
    );
    let seed = tmp.path().join("header-prerequisite.jam");
    write_boundary_seed_from_env(&kernel, &seed, "AI_POW_170000_STATE_JAM");
    let mut app = boot_boundary_state(&kernel, &tmp.path().join("initial"), &seed).await;
    assert!(app
        .peek_handle(heavy_n_path(169_997))
        .await
        .unwrap()
        .is_some());
    assert!(app
        .peek_handle(heavy_n_path(169_998))
        .await
        .unwrap()
        .is_none());
    let fixture_constants = app.peek_handle(constants_path()).await.unwrap().unwrap();
    let constants = BlockchainConstants::from_noun(
        unsafe { fixture_constants.root() },
        &fixture_constants.noun_space(),
    )
    .unwrap();
    assert_eq!(constants.zk_asert_post_ai.phase, 169_998);
    assert_eq!(constants.ai_asert.phase, 169_998);
    app.poke(SystemWire.to_wire(), born_poke())
        .await
        .expect("finish fixture initialization before current proof submission");
    app.poke(SystemWire.to_wire(), set_mining_key_poke())
        .await
        .unwrap();
    app.poke(SystemWire.to_wire(), enable_mining_poke())
        .await
        .unwrap();
    let fork_seed = tmp.path().join("fork-parent.jam");
    export_boundary_state(&app, &fork_seed).await;
    let setups = [ProofRules::Legacy, ProofRules::Hardened]
        .into_iter()
        .map(|rules| {
            build_verifier_setup_with_rules(&test_params(), 8, 2, 1, rules)
                .expect("build verifier-owned setup")
        })
        .collect();
    install_verifier_setup_disk_from_setups(setups, tmp.path(), 1).unwrap();
    let first = boundary_candidates(&mut app, 169_998).await;
    let ai = first
        .iter()
        .find(|c| c.kind == MiningCandidateKind::Ai)
        .unwrap();
    let target = ai_pow_miner::run::decode_chain_target_bignum(&ai.target).unwrap();
    let threshold = ai_pow_miner::run::reference_grind_threshold(&target).unwrap();
    let first = grind_boundary_proof(ai, &threshold, ProofRules::Hardened);
    let artifact = boundary_artifact(&first);
    let oracle = boundary_jet_accepts_with_verification_rules(
        &artifact,
        &ai.block_header,
        &target,
        VerificationRules::at_height(169_998),
    );
    eprintln!("[169998] actual-target verifier={oracle} rules={:?} trace-height={} commitment={} target-le={} jackpot-le={}",
        first.rules, first.certificate.trace_height, hex(&first.commit), hex(&target), hex(&first.jackpot_hash));
    assert!(oracle, "candidate-bound honest proof must pass the production jet at the actual candidate target before node submission; see verifier tracing");
    let effects = app
        .poke(SystemWire.to_wire(), pow_poke_from_artifact(&artifact))
        .await
        .expect("submit real AI block immediately above the fork parent");
    let summary = boundary_effect_summary(&effects);
    eprintln!("[169998] submission effects: {summary}");
    let mut retained = vec![(169_998, app.peek_handle(heavy_n_path(169_998)).await.unwrap()
        .unwrap_or_else(|| panic!("real AI block not admitted at custom preceding pin; actual-target verifier passed; effects: {summary}; see kernel rejection tracing"))
        .jam())];

    let before = boundary_candidates(&mut app, 169_999).await;
    let ai = before
        .iter()
        .find(|c| c.kind == MiningCandidateKind::Ai)
        .unwrap();
    let zk = before
        .iter()
        .find(|c| c.kind == MiningCandidateKind::Zk)
        .unwrap();
    assert_eq!(
        VerificationRules::at_height(169_999),
        VerificationRules::Hardened
    );
    let ai_target = ai_pow_miner::run::decode_chain_target_bignum(&ai.target).unwrap();
    let zk_target = boundary_target_value(&zk.target);
    assert_eq!(
        ibig::UBig::from_le_bytes(&ai_target),
        (ibig::UBig::from(1u64) << 232) - ibig::UBig::from(1u64)
    );
    assert_eq!(zk_target, ibig::UBig::from(1u64) << 320);
    let threshold = ai_pow_miner::run::reference_grind_threshold(&ai_target).unwrap();
    let block = grind_boundary_proof(ai, &threshold, ProofRules::at_height(169_999));
    let artifact = boundary_artifact(&block);
    app.poke(SystemWire.to_wire(), pow_poke_from_artifact(&artifact))
        .await
        .unwrap();
    retained.push((
        169_999,
        app.peek_handle(heavy_n_path(169_999))
            .await
            .unwrap()
            .expect("real pre-reset Hardened AI admitted")
            .jam(),
    ));

    let boundary = boundary_candidates(&mut app, 170_000).await;
    assert_170000_reset_candidates(&boundary, &constants.max_target_atom);
    assert_eq!(
        VerificationRules::at_height(170_000),
        VerificationRules::ConsensusBatch
    );
    let ai = boundary
        .iter()
        .find(|c| c.kind == MiningCandidateKind::Ai)
        .unwrap();
    let target = ai_pow_miner::run::decode_chain_target_bignum(&ai.target).unwrap();
    let live_threshold = ai_pow_miner::run::reference_grind_threshold(&target).unwrap();
    let permissive = ai_pow::difficulty::AI_POW_MAX_CONSENSUS_TARGET;
    let permissive_threshold = ai_pow_miner::run::reference_grind_threshold(&permissive).unwrap();
    let negative = grind_boundary_proof(ai, &permissive_threshold, ProofRules::Hardened);
    assert!(
        !ai_pow::tile_hash::hash_le_target(&negative.jackpot_hash, &live_threshold),
        "the target-negative control must actually miss the immutable live target"
    );
    let negative = boundary_artifact(&negative);
    assert!(
        boundary_jet_accepts_with_verification_rules(
            &negative,
            &ai.block_header,
            &permissive,
            VerificationRules::ConsensusBatch,
        ),
        "genuine proof must pass the production admission tag at an independent oracle threshold"
    );
    assert!(!boundary_jet_accepts_with_verification_rules(
        &negative,
        &ai.block_header,
        &target,
        VerificationRules::ConsensusBatch,
    ));
    assert!(
        !boundary_jet_accepts_with_verification_rules(
            &negative,
            &before
                .iter()
                .find(|c| c.kind == MiningCandidateKind::Ai)
                .unwrap()
                .block_header,
            &permissive,
            VerificationRules::ConsensusBatch,
        ),
        "commitment binding must fail independently of live difficulty"
    );
    assert!(!boundary_jet_accepts(
        &negative,
        &ai.block_header,
        &permissive,
        ProofRules::Legacy
    ));
    for poke in [
        pow_poke_from_artifact(&negative),
        malformed_ai_pow_artifact_poke(),
        short_ai_pow_artifact_poke(),
        pow_poke_from_artifact(&artifact),
    ] {
        app.poke(SystemWire.to_wire(), poke)
            .await
            .expect("submit AI rejection control");
        assert!(app
            .peek_handle(heavy_n_path(170_000))
            .await
            .unwrap()
            .is_none());
    }

    for height in [170_000, 170_001] {
        let mut candidates = boundary_candidates(&mut app, height).await;
        if height == 170_000 {
            assert_170000_reset_candidates(&candidates, &constants.max_target_atom);
        }
        assert_eq!(
            VerificationRules::at_height(height),
            VerificationRules::ConsensusBatch
        );
        if height == 170_001 {
            let ai = candidates
                .iter()
                .find(|c| c.kind == MiningCandidateKind::Ai)
                .unwrap();
            let continued_target =
                ai_pow_miner::run::decode_chain_target_bignum(&ai.target).unwrap();
            assert_eq!(
                continued_target, target,
                "interleaved ZK must not advance the AI lineage"
            );
            let continued = grind_boundary_proof(ai, &permissive_threshold, ProofRules::Hardened);
            let threshold =
                ai_pow_miner::run::reference_grind_threshold(&continued_target).unwrap();
            assert!(!ai_pow::tile_hash::hash_le_target(
                &continued.jackpot_hash, &threshold
            ));
            let continued = boundary_artifact(&continued);
            assert!(boundary_jet_accepts_with_verification_rules(
                &continued,
                &ai.block_header,
                &permissive,
                VerificationRules::at_height(height),
            ));
            assert!(!boundary_jet_accepts_with_verification_rules(
                &continued,
                &ai.block_header,
                &continued_target,
                VerificationRules::at_height(height),
            ));
            app.poke(SystemWire.to_wire(), pow_poke_from_artifact(&continued))
                .await
                .expect("submit genuine post-reset AI target-negative control");
            assert!(app
                .peek_handle(heavy_n_path(height))
                .await
                .unwrap()
                .is_none());
            candidates = boundary_candidates(&mut app, height).await;
        }
        let zk = candidates
            .iter()
            .find(|c| c.kind == MiningCandidateKind::Zk)
            .unwrap();
        let poke = mine_boundary_smoke_zk(zk).await;
        app.poke(SystemWire.to_wire(), poke)
            .await
            .expect("submit real post-reset ZK proof");
        retained.push((
            height,
            app.peek_handle(heavy_n_path(height))
                .await
                .unwrap()
                .expect("proof-bearing ZK block admitted with PoW enabled")
                .jam(),
        ));
        let next = boundary_candidates(&mut app, height + 1).await;
        let export = tmp.path().join(format!("accepted-{height}.jam"));
        export_boundary_state(&app, &export).await;
        drop(app);
        app = boot_boundary_state(
            &kernel,
            &tmp.path().join(format!("restart-{height}")),
            &export,
        )
        .await;
        for (accepted_height, page) in &retained {
            assert_eq!(
                app.peek_handle(heavy_n_path(*accepted_height))
                    .await
                    .unwrap()
                    .unwrap()
                    .jam(),
                *page
            );
        }
        let resumed = boundary_candidates(&mut app, height + 1).await;
        for kind in [MiningCandidateKind::Ai, MiningCandidateKind::Zk] {
            assert_eq!(
                next.iter().find(|c| c.kind == kind).unwrap().target.jam(),
                resumed
                    .iter()
                    .find(|c| c.kind == kind)
                    .unwrap()
                    .target
                    .jam(),
                "restart must reconstruct the same lane target after {height}",
            );
        }
    }

    // Both branches share exactly the same imported parent. The sibling's real
    // ZK work at the custom preceding pin outweighs two AI + two reset ZK pages.
    // No saved tip/selector is edited to simulate this change of chain.
    let mut sibling = boot_boundary_state(&kernel, &tmp.path().join("sibling"), &fork_seed).await;
    let candidates = boundary_candidates(&mut sibling, 169_998).await;
    let zk = candidates
        .iter()
        .find(|c| c.kind == MiningCandidateKind::Zk)
        .unwrap();
    let poke = mine_boundary_smoke_zk(zk).await;
    sibling.poke(SystemWire.to_wire(), poke).await.unwrap();
    let sibling_slab = sibling
        .peek_handle(heavy_n_path(169_998))
        .await
        .unwrap()
        .expect("real sibling ZK block admitted");
    let sibling_work = Page::from_noun(unsafe { sibling_slab.root() }, &sibling_slab.noun_space())
        .unwrap()
        .accumulated_work;
    let old_tip = app
        .peek_handle(heaviest_block_path())
        .await
        .unwrap()
        .unwrap();
    let old_work = Page::from_noun(unsafe { old_tip.root() }, &old_tip.noun_space())
        .unwrap()
        .accumulated_work;
    assert!(
        sibling_work.0 > old_work.0,
        "the submitted shorter branch must actually have more work"
    );
    let sibling_page = sibling_slab.jam();
    drop(sibling);
    let heard = heard_fake_genesis_block(Some(sibling_page.to_vec())).unwrap();
    app.poke(SystemWire.to_wire(), heard)
        .await
        .expect("hear real shorter/heavier sibling");
    assert_eq!(
        app.peek_handle(heaviest_block_path())
            .await
            .unwrap()
            .unwrap()
            .jam(),
        sibling_page,
        "kernel must select the actual shorter/heavier branch"
    );
    assert_eq!(
        app.peek_handle(heavy_n_path(169_998))
            .await
            .unwrap()
            .unwrap()
            .jam(),
        sibling_page
    );
    for height in [169_999, 170_000, 170_001] {
        assert!(
            app.peek_handle(heavy_n_path(height))
                .await
                .unwrap()
                .is_none(),
            "heaviest-chain index must drop the abandoned suffix at {height}"
        );
    }
    let rewound = boundary_candidates(&mut app, 169_999).await;
    assert_eq!(
        VerificationRules::at_height(169_999),
        VerificationRules::Hardened
    );
    let ai = rewound
        .iter()
        .find(|c| c.kind == MiningCandidateKind::Ai)
        .unwrap();
    assert_eq!(
        boundary_target_value(&ai.target),
        (ibig::UBig::from(1u64) << 232) - ibig::UBig::from(1u64)
    );
    let export = tmp.path().join("reorganized.jam");
    export_boundary_state(&app, &export).await;
    drop(app);
    let mut app =
        boot_boundary_state(&kernel, &tmp.path().join("restart-reorganized"), &export).await;
    assert_eq!(
        app.peek_handle(heaviest_block_path())
            .await
            .unwrap()
            .unwrap()
            .jam(),
        sibling_page
    );
    let candidates = boundary_candidates(&mut app, 169_999).await;
    let ai = candidates
        .iter()
        .find(|c| c.kind == MiningCandidateKind::Ai)
        .unwrap();
    let target = ai_pow_miner::run::decode_chain_target_bignum(&ai.target).unwrap();
    let threshold = ai_pow_miner::run::reference_grind_threshold(&target).unwrap();
    let resumed = grind_boundary_proof(ai, &threshold, ProofRules::at_height(169_999));
    app.poke(
        SystemWire.to_wire(),
        pow_poke_from_artifact(&boundary_artifact(&resumed)),
    )
    .await
    .expect("submit real AI block below upgrade after backwards reorg and reload");
    assert!(app
        .peek_handle(heavy_n_path(169_999))
        .await
        .unwrap()
        .is_some());
    let candidates = boundary_candidates(&mut app, 170_000).await;
    assert_170000_reset_candidates(&candidates, &constants.max_target_atom);
    let zk = candidates
        .iter()
        .find(|c| c.kind == MiningCandidateKind::Zk)
        .unwrap();
    let poke = mine_boundary_smoke_zk(zk).await;
    app.poke(SystemWire.to_wire(), poke).await.unwrap();
    assert!(
        app.peek_handle(heavy_n_path(170_000))
            .await
            .unwrap()
            .is_some(),
        "real ZK admission must resume after backwards reorg and reload"
    );
    eprintln!("[170000] real custom-network proofs, production-exact AI reset target, new 100e9 hashes/s ZK calibration with custom max-target {}, target rejection, proof-enabled accepted-state reload and shorter/heavier reorg; no mainnet-difficulty proof wins or proof-validated prehistory", constants.max_target_atom);
}

/// Keep the kernel's semantic rejection reason in a failed admission assertion.
fn boundary_effect_summary(effects: &[NounSlab]) -> String {
    use std::fmt::Write as _;
    if effects.is_empty() {
        return "none; do-pow may reject before heard-block (see kernel tracing)".to_owned();
    }
    let mut summary = String::new();
    for effect in effects {
        if !summary.is_empty() {
            summary.push_str(", ");
        }
        let space = effect.noun_space();
        let root = unsafe { *effect.root() }.in_space(&space);
        let cell = root.as_cell().expect("tagged kernel effect");
        let tag = cell.head().as_atom().expect("effect term");
        let tag = String::from_utf8_lossy(tag.as_ne_bytes());
        let tag = tag.trim_end_matches('\0');
        summary.push_str(tag);
        if tag == "liar-block-id" {
            let fields = root.uncell::<3>().expect("liar-block-id effect tuple");
            let cause = fields[2].as_atom().expect("semantic rejection term");
            let cause = String::from_utf8_lossy(cause.as_ne_bytes());
            write!(summary, ":{}", cause.trim_end_matches('\0')).unwrap();
        }
    }
    summary
}
