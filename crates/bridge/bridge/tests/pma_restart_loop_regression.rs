//! Regression reproducer for bridge restart loops that repeatedly commit startup pokes.
//!
//! This test mirrors the bridge-node-a failure mode: the process boots from the same PMA,
//! sends the durable startup pokes, then exits before making durable forward progress. The
//! repeated accepted pokes should be enough to show bump-allocation growth across restarts.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Once;

use bridge::shared::types::{
    AtomBytes, BridgeCause, BridgeConstants, NodeConfig, NodeInfo, SchnorrSecretKey, Tip5Hash,
};
use kernels_open_bridge::KERNEL;
use nockapp::kernel::boot::{default_boot_cli, setup_, NockStackSize, PmaSize, SetupResult};
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::one_punch::OnePunchWire;
use nockapp::wire::Wire;
use nockapp::NockApp;
use nockchain_math::belt::Belt;
use nockchain_types::tx_engine::common::Hash as NockPkh;
use nockvm::pma::{Pma, PmaFileMetadata};
use noun_serde::NounEncode;
use tempfile::TempDir;
use zkvm_jetpack::hot::produce_prover_hot_state;

const RESTART_CYCLES: usize = 3;
const INITIAL_PMA_WORDS: usize = 33_554_432; // 256 MiB, matching bridge production sizing.
const RESERVED_PMA_WORDS: usize = 268_435_456; // 2 GiB cap for the local reproducer.
const PREFLIGHT_FREE_WORDS: usize = 30_000_000; // Force early growth without large files.

struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: impl ToString) -> Self {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value.to_string());
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            std::env::set_var(self.key, previous);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

#[tokio::test]
#[ignore = "regression reproducer: intentionally exercises repeated durable bridge startup pokes"]
async fn bridge_restart_loop_startup_pokes_grow_pma_regression() -> Result<(), Box<dyn Error>> {
    init_test_tracing();
    nockvm::check_endian();
    let _preflight_guard =
        EnvGuard::set("NOCK_PMA_EVENT_PREFLIGHT_FREE_WORDS", PREFLIGHT_FREE_WORDS);
    let _reserved_guard = EnvGuard::set("NOCK_PMA_RESERVED_WORDS", RESERVED_PMA_WORDS);

    let temp = TempDir::new()?;
    let data_dir = temp.path().join("bridge-restart-loop-pma-regression");
    let mut samples = Vec::new();

    for cycle in 0..RESTART_CYCLES {
        let mut app = boot_bridge_app(&data_dir).await?;
        let before = largest_runtime_pma(&data_dir)?;
        send_bridge_startup_pokes(&mut app).await?;
        let after = largest_runtime_pma(&data_dir)?;
        println!(
            "restart cycle {cycle}: before data_words={} alloc_words={} after data_words={} alloc_words={} delta_alloc_mib={:.3}",
            before.data_words,
            before.alloc_words,
            after.data_words,
            after.alloc_words,
            words_to_mib(after.alloc_words.saturating_sub(before.alloc_words))
        );
        samples.push(after);
        stop_app(app).await?;
    }

    let first = samples
        .first()
        .ok_or_else(|| std::io::Error::other("missing first PMA sample"))?;
    let last = samples
        .last()
        .ok_or_else(|| std::io::Error::other("missing last PMA sample"))?;
    let delta_words = last.alloc_words.saturating_sub(first.alloc_words);

    assert!(
        delta_words > 0,
        "restart-loop startup pokes should reproduce PMA bump growth: first_alloc={} last_alloc={}",
        first.alloc_words,
        last.alloc_words
    );
    assert!(
        samples
            .windows(2)
            .all(|pair| pair[1].alloc_words > pair[0].alloc_words),
        "PMA alloc_words should increase after every simulated restart: {:?}",
        samples
            .iter()
            .map(|metadata| metadata.alloc_words)
            .collect::<Vec<_>>()
    );
    assert!(
        samples
            .iter()
            .any(|metadata| metadata.data_words > INITIAL_PMA_WORDS as u64),
        "accelerated regression fixture should trigger PMA capacity growth: {:?}",
        samples
            .iter()
            .map(|metadata| metadata.data_words)
            .collect::<Vec<_>>()
    );

    println!(
        "restart-loop PMA growth reproduced: cycles={} delta_words={} delta_mib={:.3}",
        RESTART_CYCLES,
        delta_words,
        words_to_mib(delta_words)
    );
    Ok(())
}

fn init_test_tracing() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    });
}

async fn boot_bridge_app(data_dir: &Path) -> Result<NockApp<NockJammer>, Box<dyn Error>> {
    let mut cli = default_boot_cli(false);
    cli.data_dir = Some(data_dir.to_path_buf());
    cli.stack_size = NockStackSize::Medium;
    cli.pma_initial_size = Some(PmaSize::from_words(INITIAL_PMA_WORDS));
    cli.pma_reserved_size = Some(PmaSize::from_words(RESERVED_PMA_WORDS));
    cli.gc_interval = None;
    cli.rotating_snapshot_interval_event_time = None;
    cli.disable_fsync = true;

    match setup_::<NockJammer>(
        KERNEL,
        cli,
        &produce_prover_hot_state(),
        "bridge-restart-loop-pma-regression",
        None,
    )
    .await?
    {
        SetupResult::App(app) => Ok(app),
        SetupResult::ExportedState => Err(std::io::Error::other("unexpected state export").into()),
    }
}

async fn send_bridge_startup_pokes(app: &mut NockApp<NockJammer>) -> Result<(), Box<dyn Error>> {
    poke_bridge_cause(app, BridgeCause::start()).await?;
    poke_bridge_cause(app, BridgeCause::cfg_load(Some(test_node_config()))).await?;
    poke_bridge_cause(app, BridgeCause::set_constants(test_bridge_constants())).await?;
    Ok(())
}

async fn poke_bridge_cause(
    app: &mut NockApp<NockJammer>,
    cause: BridgeCause,
) -> Result<(), Box<dyn Error>> {
    let mut slab = NounSlab::new();
    let noun = cause.to_noun(&mut slab);
    slab.set_root(noun);
    app.poke(OnePunchWire::Poke.to_wire(), slab).await?;
    Ok(())
}

async fn stop_app(mut app: NockApp<NockJammer>) -> Result<(), Box<dyn Error>> {
    let handle = app.get_handle();
    handle.exit.exit(0).await?;
    app.run().await?;
    Ok(())
}

fn largest_runtime_pma(data_dir: &Path) -> Result<PmaFileMetadata, Box<dyn Error>> {
    let pma_dir = data_dir.join("pma");
    let mut best: Option<(PathBuf, PmaFileMetadata)> = None;
    for idx in 0..=1 {
        let path = pma_dir.join(format!("{idx}.pma"));
        if !path.exists() {
            continue;
        }
        let metadata = Pma::read_file_metadata(&path)?;
        match &best {
            Some((_best_path, best_metadata))
                if best_metadata.alloc_words >= metadata.alloc_words => {}
            _ => best = Some((path, metadata)),
        }
    }
    best.map(|(_path, metadata)| metadata).ok_or_else(|| {
        std::io::Error::other(format!(
            "no runtime PMA files found in {}",
            pma_dir.display()
        ))
        .into()
    })
}

fn test_bridge_constants() -> BridgeConstants {
    BridgeConstants {
        base_blocks_chunk: 1,
        base_start_height: 40,
        nockchain_start_height: 0,
        minimum_event_nocks: 1,
        ..BridgeConstants::default()
    }
}

fn test_node_config() -> NodeConfig {
    NodeConfig {
        node_id: 0,
        nodes: vec![
            test_node_info(0, "2222222222222222222222222222222222222222222222222222"),
            test_node_info(1, "3333333333333333333333333333333333333333333333333333"),
            test_node_info(2, "4444444444444444444444444444444444444444444444444444"),
            test_node_info(3, "5555555555555555555555555555555555555555555555555555"),
            test_node_info(4, "6666666666666666666666666666666666666666666666666666"),
        ],
        bridge_lock_root: Tip5Hash::from_base58(
            "AcsPkuhXQoGeEsF91yynpm1kcW17PQ2Z1MEozgx7YnDPkZwrtzLuuqd",
        )
        .expect("bridge lock root"),
        my_eth_key: AtomBytes(vec![0xf0; 32]),
        my_nock_key: SchnorrSecretKey([Belt(1); 8]),
    }
}

fn test_node_info(id: u8, pkh_b58: &str) -> NodeInfo {
    NodeInfo {
        ip: format!("127.0.0.1:80{id:02}"),
        eth_pubkey: AtomBytes(vec![id; 20]),
        nock_pkh: NockPkh::from_base58(pkh_b58).expect("valid test pkh"),
    }
}

fn words_to_mib(words: u64) -> f64 {
    (words as f64 * 8.0) / (1024.0 * 1024.0)
}
