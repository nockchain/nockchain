//! End-to-end mock-node integration test for the zk-pow-miner.
//!
//! Spins up a private `NockAppService` gRPC server backed by a hand-built
//! `NockAppHandle` (no real chain kernel), runs `zk_pow_miner::run::run_with_pool`
//! against it with a **real** `SerfWorker` pool (loaded with `assets/miner.jam`),
//! publishes one synthetic `%mine` effect with `target = 2^400` so any
//! digest passes, and asserts that within 60 seconds the mock observes a
//! `ZkPowMinerWire::Mined`-wire poke whose payload begins with
//! `[%command %pow …]` — proving the miner ran the real STARK and routed
//! the solution back over gRPC.
//!
//! This is the strongest end-to-end validation that doesn't depend on
//! `nockchain`'s fakenet bootstrap (which has a pre-existing
//! `BlockchainConstants` ↔ `blockchain-constants:v1` schema-mismatch bug;
//! see `scripts/fakenet-zk-pow-smoke.sh` header for details). It pairs with:
//!
//! - `crates/nockapp-grpc/src/tests.rs::watch_effects_round_trip_with_head_filter`
//!   (proves the node-side `WatchEffects` RPC works end-to-end)
//! - `crates/zk-pow-miner/src/worker.rs::tests::serf_worker_mines_trivial_target`
//!   (proves the SerfWorker runs the STARK at trivial target in ~1.3s)
//! - `crates/zk-pow-miner/src/run.rs::tests::run_loop_*`
//!   (proves the run loop wires NodeClient ↔ Pool correctly with stubs)
//!
//! Together those four tests fully cover the zk-pow-miner integration up
//! to the chain-bring-up boundary.
//!
//! Marked `#[ignore]` because spawning a real `SerfThread` + running the
//! STARK takes ~5–10 s. Run with:
//!
//!   cargo test -p zk-pow-miner --test end_to_end_mock_node --release -- --ignored

use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ibig::UBig;
use nockapp::driver::{IOAction, NockAppHandle, PokeResult};
use nockapp::nockapp::wire::Wire;
use nockapp::noun::slab::NounSlab;
use nockapp::noun::AtomExt;
use nockapp::NockAppExit;
use nockapp_grpc::services::private_nockapp::server::PrivateNockAppGrpcServer;
use nockchain_mining_common::MiningPkhConfig;
use nockvm::noun::{Atom, D, T};
use nockvm_macros::tas;
use once_cell::sync::Lazy;
use tokio::sync::{broadcast, mpsc, Mutex as TMutex};
use tokio_util::sync::CancellationToken;
use zk_pow_miner::pool::Pool;
use zk_pow_miner::run::{run_with_pool, MinerConfig};
use zk_pow_miner::wire::ZkPowMinerWire;
use zk_pow_miner::worker::{SerfWorker, Worker};

/// Singleton metrics — gnort's registry rejects double-registration.
static METRICS: Lazy<Arc<nockapp::nockapp::metrics::NockAppMetrics>> = Lazy::new(|| {
    Arc::new(
        nockapp::nockapp::metrics::NockAppMetrics::register(gnort::global_metrics_registry())
            .expect("register NockAppMetrics"),
    )
});

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn miner_finds_and_submits_block_against_mock_node() {
    // ── 1. Stand up a mock private NockAppService on an ephemeral port.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr: SocketAddr = listener.local_addr().expect("local_addr");
    drop(listener);
    let server_url = format!("http://{addr}");

    let (action_tx, mut action_rx) = mpsc::channel::<IOAction>(64);
    let (effect_tx, _seed_rx) = broadcast::channel::<NounSlab>(64);
    let effect_tx = Arc::new(effect_tx);
    let effect_rx_for_handle = effect_tx.subscribe();
    let (exit, _exit_rx) = NockAppExit::new();
    let handle = NockAppHandle {
        io_sender: action_tx,
        effect_sender: effect_tx.clone(),
        effect_receiver: TMutex::new(effect_rx_for_handle),
        metrics: METRICS.clone(),
        exit,
    };

    // Drain action channel; collect ZkPowMinerWire::Mined poke slabs.
    let mined_pokes: Arc<TMutex<Vec<NounSlab>>> = Arc::new(TMutex::new(Vec::new()));
    let pokes_observed = Arc::new(AtomicU64::new(0));
    let mined_clone = mined_pokes.clone();
    let pokes_clone = pokes_observed.clone();
    let action_drainer = tokio::spawn(async move {
        while let Some(action) = action_rx.recv().await {
            match action {
                IOAction::Poke {
                    wire,
                    ack_channel,
                    poke,
                    ..
                } => {
                    pokes_clone.fetch_add(1, Ordering::SeqCst);
                    eprintln!(
                        "[mock-node] poke received: source={} v={} tags={:?}",
                        wire.source, wire.version, wire.tags
                    );
                    let is_mined = wire.source == ZkPowMinerWire::SOURCE
                        && wire.tags.iter().any(|t| {
                            matches!(
                                t,
                                nockapp::wire::WireTag::String(s) if s == "mined"
                            )
                        });
                    if is_mined {
                        mined_clone.lock().await.push(poke);
                    }
                    let _ = ack_channel.send(PokeResult::Ack);
                }
                IOAction::Peek { .. } => {}
            }
        }
    });

    // Spawn the server.
    let server = PrivateNockAppGrpcServer::new(handle);
    let server_task = tokio::spawn(async move { server.serve(addr).await });

    // Give the server a moment.
    tokio::time::sleep(Duration::from_millis(150)).await;

    // ── 2. Build a real SerfWorker pool (1 worker; the STARK at pow-len=2
    // takes ~1.3 s wall-clock).
    eprintln!("[test] producing prover hot state + spawning SerfWorker ...");
    let hot_state = zkvm_jetpack::hot::produce_prover_hot_state();
    let worker = SerfWorker::spawn(0, hot_state)
        .await
        .expect("spawn SerfWorker");
    let workers: Vec<Arc<dyn Worker>> = vec![Arc::new(worker)];
    let pool = Pool::new(workers);
    eprintln!("[test] pool ready");

    // ── 3. Run the miner against the mock node.
    let cfg = MinerConfig {
        node_addr: server_url,
        mining_pkh_configs: vec![MiningPkhConfig {
            share: 1,
            pkh: "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV".to_string(),
        }],
        num_threads: 1,
        reconnect_backoff_initial: Duration::from_millis(50),
        reconnect_backoff_max: Duration::from_millis(200),
        reconnect_max_attempts: 5,
    };
    let shutdown = CancellationToken::new();
    let shutdown_clone = shutdown.clone();
    let miner_task = tokio::spawn(async move { run_with_pool(cfg, pool, shutdown_clone).await });

    // Brief pause for the miner to connect + configure + subscribe.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // ── 4. Publish a synthetic %mine effect with target = 2^400 (any
    // digest from the STARK passes).
    let mine_effect = build_synth_mine_effect(2);
    effect_tx
        .send(mine_effect)
        .expect("publish synthetic %mine effect");
    eprintln!("[test] published synthetic %mine effect; awaiting %mined poke ...");

    // ── 5. Poll for a %mined poke, up to 60s.
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut got_pokes = Vec::new();
    while Instant::now() < deadline {
        {
            let guard = mined_pokes.lock().await;
            if !guard.is_empty() {
                got_pokes = guard.clone();
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let total_pokes = pokes_observed.load(Ordering::SeqCst);
    eprintln!(
        "[test] poll complete: mined_pokes={} total_pokes={}",
        got_pokes.len(),
        total_pokes
    );

    // ── 6. Tear down.
    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), miner_task)
        .await
        .expect("miner task did not exit");
    server_task.abort();
    let _ = server_task.await;
    action_drainer.abort();
    let _ = action_drainer.await;

    // ── 7. Assertions.
    assert!(
        !got_pokes.is_empty(),
        "miner did not submit a %mined poke within 60s; total pokes observed = {total_pokes}"
    );
    // Validate the first %mined poke's shape: [%command %pow ...].
    use nockvm::noun::NounAllocator;
    let first = &got_pokes[0];
    let space = first.noun_space();
    let root = unsafe { *first.root() };
    let cell = root
        .in_space(&space)
        .as_cell()
        .expect("poke root is a cell");
    assert!(
        cell.head().eq_bytes("command"),
        "%mined poke head should be %command"
    );
    let tail = cell.tail().noun();
    let tail_cell = tail
        .in_space(&space)
        .as_cell()
        .expect("poke tail is a cell");
    assert!(
        tail_cell.head().eq_bytes("pow"),
        "%mined poke command should be %pow"
    );
    eprintln!("[test] PASS: %mined poke shape is [%command %pow ...]");
}

/// Build a synthetic `[%mine-zk version commit target pow-len]` effect with
/// version=%0, commit=[0 0 0 0 0], target=2^400 (trivial), and the
/// provided pow-len.
fn build_synth_mine_effect(pow_len: u64) -> NounSlab {
    let mut slab = NounSlab::new();
    let head = D(tas!(b"mine-zk"));
    let version = D(0);
    let commit = T(&mut slab, &[D(0), D(0), D(0), D(0), D(0)]);
    // target = 2^400 as [%bn list-of-u32-le]
    let target_value = UBig::from(1u64) << 400;
    let target = bignum_to_noun(&mut slab, &target_value);
    let plen = D(pow_len);
    let effect = T(&mut slab, &[head, version, commit, target, plen]);
    slab.set_root(effect);
    slab
}

/// `bignum_to_noun` mirrors the helper in
/// `crates/nockchain/tests/open_prover_bench.rs`: encodes a UBig as
/// `[%bn list-of-u32-belts-in-little-endian]`.
fn bignum_to_noun(slab: &mut NounSlab, value: &UBig) -> nockvm::noun::Noun {
    let mut list = D(0);
    let bytes = value.to_le_bytes();
    for chunk in bytes.chunks(4).rev() {
        let mut padded = [0u8; 4];
        padded[..chunk.len()].copy_from_slice(chunk);
        let chunk = u64::from(u32::from_le_bytes(padded));
        let atom = <Atom as AtomExt>::from_value(slab, chunk)
            .expect("atom")
            .as_noun();
        list = T(slab, &[atom, list]);
    }
    T(slab, &[D(tas!(b"bn")), list])
}
