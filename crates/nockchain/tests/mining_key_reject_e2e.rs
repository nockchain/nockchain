//! GHSA-hgg4-q235-p5m5 regression: malformed `set-mining-key(-advanced)`
//! pokes must be rejected per-request with the node alive.
//!
//! The command arms used to return `[%exit 1]` on malformed input — a
//! boot-time fatal semantics applied to an unauthenticated runtime poke,
//! soft machinery converts into a per-poke error; the kernel keeps running.

#![allow(clippy::unwrap_used)] // integration test: unwrap is acceptable
use chaff::Chaff;
use nockapp::kernel::boot::{self, NockStackSize};
use nockapp::noun::slab::NounSlab;
use nockapp::utils::make_tas;
use nockapp::wire::{SystemWire, Wire};
use nockapp::{AtomExt, NockApp};
use nockchain::setup::{self, heard_fake_genesis_block, SetupCommand, FAKENET_GENESIS_MESSAGE};
use nockchain_types::tx_engine::common::Hash;
use nockchain_types::{fakenet_blockchain_constants, Seconds};
use nockvm::noun::{Atom, D, T};
use nockvm_macros::tas;

fn born_poke() -> NounSlab {
    let mut slab = NounSlab::new();
    let born = T(&mut slab, &[D(tas!(b"command")), D(tas!(b"born")), D(0)]);
    slab.set_root(born);
    slab
}

/// The advisory's kill payload: three well-formed `[share, pkh]` v1
/// entries, where the kernel supports at most two.
fn overlong_advanced_poke() -> NounSlab {
    let mut slab = NounSlab::new();
    let pkh = Hash([0, 1, 2, 3, 4].map(nockchain_math::belt::Belt)).to_base58();
    let cmd = make_tas(&mut slab, "set-mining-key-advanced").as_noun();
    let entry = |slab: &mut NounSlab| {
        let pkh = Atom::from_value(slab, pkh.as_bytes()).unwrap().as_noun();
        T(slab, &[D(1), pkh])
    };
    let e1 = entry(&mut slab);
    let e2 = entry(&mut slab);
    let e3 = entry(&mut slab);
    let v1 = T(&mut slab, &[e1, e2, e3, D(0)]);
    let poke = T(&mut slab, &[D(tas!(b"command")), cmd, D(0), v1]);
    slab.set_root(poke);
    slab
}

/// Malformed base58 in the plain `set-mining-key` arm.
fn invalid_key_poke() -> NounSlab {
    let mut slab = NounSlab::new();
    let cmd = make_tas(&mut slab, "set-mining-key").as_noun();
    let bad = Atom::from_value(&mut slab, "not-base58!".as_bytes())
        .unwrap()
        .as_noun();
    let poke = T(&mut slab, &[D(tas!(b"command")), cmd, bad, bad]);
    slab.set_root(poke);
    slab
}

fn heaviest_block_path() -> NounSlab {
    let mut slab = NounSlab::new();
    let tag = make_tas(&mut slab, "heaviest-block").as_noun();
    let path = T(&mut slab, &[tag, D(0)]);
    slab.set_root(path);
    slab
}

#[tokio::test]
async fn malformed_mining_key_pokes_are_rejected_with_node_alive() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut cli = boot::default_boot_cli(true);
    cli.data_dir = Some(tmp.path().to_path_buf());
    cli.stack_size = NockStackSize::Large;
    let hot = zkvm_jetpack::hot::produce_prover_hot_state();
    let mut app: NockApp<Chaff> = boot::setup(
        kernels_open_dumb::KERNEL,
        cli,
        hot.as_slice(),
        "nockchain",
        None,
    )
    .await
    .expect("boot dumb kernel");

    let constants =
        fakenet_blockchain_constants(2, 1).with_update_candidate_timestamp_interval(Seconds(1));
    setup::poke(
        &mut app,
        SetupCommand::PokeFakenetConstants(Box::new(constants)),
    )
    .await
    .expect("set-constants");
    setup::poke(
        &mut app,
        SetupCommand::PokeSetGenesisSeal(FAKENET_GENESIS_MESSAGE.to_string()),
    )
    .await
    .expect("set-genesis-seal");
    setup::poke(&mut app, SetupCommand::PokeSetBtcData)
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
    // The advisory's kill payload: must produce no %exit effect (the old
    // arm returned `[%exit 1]`, which the exit driver turned into process
    // death) and leave the node responsive.
    let overlong_effects = app
        .poke(SystemWire.to_wire(), overlong_advanced_poke())
        .await
        .expect("the malformed poke must be absorbed, not kill the poke machinery");
    assert!(
        !has_exit_effect(&overlong_effects),
        "a rejected key config must not emit a process-exit effect",
    );
    assert!(
        app.peek_handle(heaviest_block_path())
            .await
            .unwrap()
            .is_some(),
        "the node must stay alive and responsive after the rejected poke",
    );

    // The plain arm's malformed base58 must be rejected the same way.
    let invalid_effects = app
        .poke(SystemWire.to_wire(), invalid_key_poke())
        .await
        .expect("the malformed poke must be absorbed, not kill the poke machinery");
    assert!(
        !has_exit_effect(&invalid_effects),
        "an invalid mining key must not emit a process-exit effect",
    );
    assert!(
        app.peek_handle(heaviest_block_path())
            .await
            .unwrap()
            .is_some(),
        "the node must stay alive after both rejected pokes",
    );
}

/// True when any effect slab is a `[%exit code]` effect — the old kill path.
fn has_exit_effect(effects: &[NounSlab]) -> bool {
    use nockvm::noun::NounAllocator;

    effects.iter().any(|slab| {
        let space = slab.noun_space();
        let root = unsafe { *slab.root() }.in_space(&space);
        root.as_cell()
            .map(|cell| {
                let head = cell.head().noun();
                head.in_space(&space)
                    .as_atom()
                    .ok()
                    .and_then(|atom| atom.as_u64().ok())
                    .map(|tag| tag == u64::from(tas!(b"exit")))
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    })
}
