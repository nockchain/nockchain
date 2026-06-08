#![allow(clippy::result_large_err)]
//! Profiling harness for the dumbnet kernel's *block and transaction
//! verification* path. Unlike `vet_chain_signatures` (read-only peeks), this
//! binary actually drives the consensus validation logic so it can be profiled
//! with `samply` to find optimization targets.
//!
//! ## Why a replay is required
//!
//! Re-poking a historical block into a node that already has it is a no-op:
//! `heard-block` short-circuits at `check-duplicate-block` before any of the
//! expensive work (`check-digest` -> `validate-page-without-txs` -> `check-pow`
//! -> `process-block-with-txs`). Likewise `heard-tx` short-circuits at
//! `inputs-in-heaviest-balance` / `inputs-spent` before `validate:raw-tx`
//! because a historical tx's inputs are already spent at the tip. So to exercise
//! the full validation pipeline for a block at height N the node must sit at
//! height N-1 *without* block N.
//!
//! This harness therefore boots a fresh "target" node, initializes it to the
//! same mainnet genesis as the synced "source" (state-jam) node, and replays
//! real blocks (and their raw transactions) into the target in order. Each block
//! is validated against the parent state the target just built -- digest checks,
//! PoW (STARK) verification, and transaction signature verification all run for
//! real.
//!
//! ## Usage
//!
//! ```text
//! samply record -- target/release/bench_dumb_validation \
//!     --state-jam /path/to/state.jam --start 1 --end 2000
//! ```
//!
//! Blocks `1..start` are replayed as warmup to position the target; the
//! `start..=end` window is what you care about in the profile (its wall time is
//! reported separately). `--skip-pow` flips the `check-pow` flag in the replayed
//! constants so the STARK verifier is bypassed -- useful for isolating
//! transaction/hashing cost from PoW cost.

use std::error::Error;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::Parser;
use kernels_open_dumb::KERNEL as NOCKCHAIN_KERNEL;
use libp2p::PeerId;
use nockapp::kernel::boot::{self, NockStackSize, PmaSize, TraceOpts};
use nockapp::nockapp::export::ExportedState;
use nockapp::noun::slab::NounSlab;
use nockapp::utils::make_tas;
use nockapp::wire::{SystemWire, Wire};
use nockapp::{AtomExt, NockApp, NockAppError};
use nockchain::setup::{self, REALNET_GENESIS_MESSAGE};
use nockchain_libp2p_io::driver::Libp2pWire;
use nockchain_libp2p_io::tip5_util::tip5_hash_to_base58_stack;
use nockchain_math::noun_ext::NounMathExtHandle;
use nockchain_math::structs::HoonMapIter;
use nockchain_types::BlockchainConstants;
use nockvm::noun::{Atom, Noun, NounAllocator, NounSpace, D, SIG, T};
use nockvm_macros::tas;
use noun_serde::NounDecode;
use tempfile::Builder;
use tracing::{info, warn};
use zkvm_jetpack::hot::produce_prover_hot_state;

type Chaff = chaff::Chaff;

#[derive(Parser, Debug)]
#[command(
    name = "bench-dumb-validation",
    about = "Replay real blocks into a fresh node to profile full block + tx verification."
)]
struct Args {
    /// Path to the source state jam (a synced mainnet kernel state) to read
    /// blocks and raw transactions from.
    #[arg(long)]
    state_jam: PathBuf,
    /// First height of the measured window (inclusive). Blocks `1..start` are
    /// still replayed (warmup) to position the target at `start-1`.
    #[arg(long, default_value_t = 1)]
    start: u64,
    /// Last height to replay/measure (inclusive).
    #[arg(long, default_value_t = 2000)]
    end: u64,
    /// Flip the `check-pow` flag off in the replayed constants to bypass the
    /// STARK verifier (isolates tx/hash cost from PoW cost). Only applies when
    /// the target boots fresh (ignored with `--target-state-jam`).
    #[arg(long, default_value_t = false)]
    skip_pow: bool,
    /// Boot the target from this previously-exported state jam instead of a
    /// fresh genesis, skipping the warmup replay. Pair with a prior
    /// `--export-state-jam` run to profile a late range without replaying the
    /// whole chain each time. In this mode only the measured window
    /// `start..=end` is extracted and replayed, so the checkpoint must be at
    /// height `start-1` (e.g. export at 38999, then `--start 39000`). Any block
    /// already present in the imported state is skipped automatically.
    #[arg(long)]
    target_state_jam: Option<PathBuf>,
    /// After replaying to `--end`, export the target's kernel state to this path
    /// so a later run can `--target-state-jam` it and skip the warmup.
    #[arg(long)]
    export_state_jam: Option<String>,
    /// Stack/PMA size for the source node (needs a large arena for a big jam).
    #[arg(long, value_enum, default_value_t = NockStackSize::Huge)]
    source_stack_size: NockStackSize,
    /// Stack/PMA size for the target node (grows as it replays).
    #[arg(long, value_enum, default_value_t = NockStackSize::Large)]
    target_stack_size: NockStackSize,
    /// Log a progress line every N heights.
    #[arg(long, default_value_t = 100)]
    progress_every: u64,
}

/// A block ready to be replayed: the `heard-block` fact plus the `heard-tx`
/// facts for every raw transaction the block references, all self-contained
/// (so the source node can be dropped before replay begins).
struct ReplayBlock {
    height: u64,
    block_id: String,
    block_fact: NounSlab,
    tx_facts: Vec<NounSlab>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    nockvm::check_endian();
    std::env::set_var("NOCKAPP_DISABLE_METRICS", "1");
    std::env::set_var("GNORT_DISABLE", "1");

    let args = Args::parse();
    if args.start == 0 {
        return Err("--start must be >= 1 (height 0 is the genesis block)".into());
    }
    if args.end < args.start {
        return Err("--end must be >= --start".into());
    }

    boot::init_default_tracing(&base_cli(NockStackSize::Huge));

    // A fresh target must replay the whole prefix (1..start) as warmup to
    // position itself at start-1, so it needs genesis + every block 1..=end.
    // When booting from a checkpoint the prefix is already in the imported
    // state, so only the measured window start..=end is extracted/replayed --
    // the checkpoint must be at height start-1.
    let from_jam = args.target_state_jam.is_some();
    let extract_from = if from_jam { args.start } else { 1 };

    // ---- phase 1: extract the needed blocks (and, if fresh, genesis) ----
    let (constants, genesis, blocks) = {
        info!(
            "bench: booting source node from state jam {} (loads the full chain state)",
            args.state_jam.display()
        );
        let mut source = boot_source(&args).await?;
        info!(
            "bench: source boot complete; extracting blocks {}..={}",
            extract_from, args.end
        );

        // Constants + genesis are only needed to initialize a fresh target.
        let (constants, genesis) = if from_jam {
            (None, None)
        } else {
            let mut constants = peek_constants(&mut source).await?;
            if args.skip_pow {
                info!("bench: --skip-pow set; disabling check-pow flag in replayed constants");
                constants.check_pow_flag = false;
            }
            let genesis = extract_block(&mut source, 0)
                .await?
                .ok_or("source node has no genesis block (height 0); cannot initialize target")?;
            (Some(constants), Some(genesis))
        };

        let mut blocks = Vec::new();
        for height in extract_from..=args.end {
            match extract_block(&mut source, height).await? {
                Some(block) => blocks.push(block),
                None => {
                    warn!(
                        "bench: source has no block at height {}; stopping extraction here",
                        height
                    );
                    break;
                }
            }
            if args.progress_every > 0 && height.is_multiple_of(args.progress_every) {
                info!("bench: extracted blocks through height {}", height);
            }
        }
        info!("bench: extracted {} block(s) from source", blocks.len());
        (constants, genesis, blocks)
        // source dropped here, freeing its arena before the target grows
    };

    // ---- phase 2: boot target + (if fresh) initialize to mainnet genesis ----
    let scratch = Builder::new().prefix("bench-dumb-validation").tempdir()?;
    match args.target_state_jam.as_ref() {
        Some(path) => info!(
            "bench: booting target from state jam {} (skipping genesis init + warmup)",
            path.display()
        ),
        None => info!("bench: booting fresh target node and initializing to mainnet genesis"),
    }
    let mut target = boot_target(&args, scratch.path().to_path_buf()).await?;

    match (from_jam, constants, genesis) {
        (false, Some(constants), Some(genesis)) => {
            // set-constants (source's exact constants, optionally pow-off),
            // genesis seal, btc-data, born -- the same init a fresh mainnet
            // node performs.
            setup::poke(
                &mut target,
                setup::SetupCommand::PokeFakenetConstants(Box::new(constants)),
            )
            .await?;
            setup::poke(
                &mut target,
                setup::SetupCommand::PokeSetGenesisSeal(REALNET_GENESIS_MESSAGE.to_string()),
            )
            .await?;
            setup::poke(&mut target, setup::SetupCommand::PokeSetBtcData).await?;
            target.poke(SystemWire.to_wire(), make_born_poke()).await?;

            // genesis block
            target
                .poke(
                    Libp2pWire::Gossip(PeerId::random()).to_wire(),
                    genesis.block_fact,
                )
                .await?;
            if peek_page(&mut target, 0).await?.is_none() {
                return Err(
                    "target did not accept the genesis block; init sequence is wrong".into(),
                );
            }
            info!(
                "bench: target accepted genesis (block_id={})",
                genesis.block_id
            );
        }
        _ => {
            // Booted from a checkpoint: confirm it contains the parent of the
            // first measured block, otherwise the replay can't position itself.
            if peek_page(&mut target, args.start - 1).await?.is_none() {
                warn!(
                    "bench: imported state has no block at height {} (the parent of the first \
                     replayed block {}); blocks will fail to validate. The checkpoint passed to \
                     --target-state-jam should be at height start-1.",
                    args.start - 1,
                    args.start
                );
            }
        }
    }

    // ---- phase 3: replay blocks, measuring the start..=end window ----
    let mut warmup_wall = Duration::ZERO;
    let mut measured_wall = Duration::ZERO;
    let mut measured_blocks = 0u64;
    let mut measured_txs = 0u64;
    let mut skipped_present = 0u64;
    let mut rejected = 0u64;

    for block in blocks {
        let height = block.height;
        let measured = height >= args.start;

        // When booting from an imported state, the prefix is already validated;
        // skip any block the target already has (cheap peek vs. re-validation).
        if from_jam {
            if let Some(mut page) = peek_page(&mut target, height).await? {
                if page_block_id(&mut page)? == block.block_id {
                    skipped_present += 1;
                    continue;
                }
            }
        }

        let tx_count = block.tx_facts.len() as u64;
        let start = Instant::now();
        for tx_fact in block.tx_facts {
            target
                .poke(Libp2pWire::Gossip(PeerId::random()).to_wire(), tx_fact)
                .await?;
        }
        let result = target
            .poke(
                Libp2pWire::Gossip(PeerId::random()).to_wire(),
                block.block_fact,
            )
            .await;
        let elapsed = start.elapsed();

        match result {
            Ok(_) => {}
            Err(e) => {
                rejected += 1;
                warn!("bench: poke for block at height {} errored: {}", height, e);
            }
        }

        // Confirm the target actually accepted (validated) the block.
        let accepted = match peek_page(&mut target, height).await? {
            Some(mut page) => page_block_id(&mut page)? == block.block_id,
            None => false,
        };
        if !accepted {
            rejected += 1;
            warn!(
                "bench: block at height {} (block_id={}) did NOT validate into the target",
                height, block.block_id
            );
        }

        if measured {
            measured_wall += elapsed;
            measured_blocks += 1;
            measured_txs += tx_count;
        } else {
            warmup_wall += elapsed;
        }

        if args.progress_every > 0 && height.is_multiple_of(args.progress_every) {
            info!(
                "bench: replayed height={} (measured_blocks={} measured_ms={:.1} rejected={})",
                height,
                measured_blocks,
                ms(measured_wall),
                rejected
            );
        }
    }
    info!("================ bench: replay complete ================");
    if skipped_present > 0 {
        info!("blocks skipped (in imported state): {}", skipped_present);
    }
    info!(
        "warmup blocks (1..{})      : wall {:.1} ms",
        args.start,
        ms(warmup_wall)
    );
    info!(
        "measured blocks ({}..={})   : {} block(s), {} tx(s), wall {:.1} ms",
        args.start,
        args.end,
        measured_blocks,
        measured_txs,
        ms(measured_wall)
    );
    if measured_blocks > 0 {
        info!(
            "mean per measured block    : {:.3} ms",
            ms(measured_wall) / measured_blocks as f64
        );
    }
    info!("blocks that did NOT validate: {}", rejected);
    if rejected != 0 {
        warn!(
            "bench: {} block(s) failed to validate -- profile numbers may not reflect full \
             validation. Check constants/genesis-seal match the source chain.",
            rejected
        );
    }

    if let Some(export_path) = args.export_state_jam.as_deref() {
        export_target(&target, export_path).await?;
    }

    Ok(())
}

fn base_cli(stack_size: NockStackSize) -> boot::Cli {
    boot::Cli {
        new: true,
        trace_opts: TraceOpts::default(),
        gc_interval: None,
        rotating_snapshot_interval_event_time: None,
        ephemeral: false,
        color: clap::ColorChoice::Auto,
        state_jam: None,
        bootstrap_from_chkjam: None,
        export_state_jam: None,
        stack_size,
        pma_initial_size: Some(PmaSize::from_words(stack_size.stack_words())),
        pma_reserved_size: None,
        data_dir: None,
        event_log_path: None,
        disable_fsync: true,
    }
}

async fn boot_source(args: &Args) -> Result<NockApp<Chaff>, Box<dyn Error>> {
    let scratch = Builder::new().prefix("bench-dumb-source").tempdir()?;
    let cli = boot::Cli {
        state_jam: Some(args.state_jam.to_string_lossy().into_owned()),
        data_dir: Some(scratch.path().to_path_buf()),
        ..base_cli(args.source_stack_size)
    };
    // Keep the tempdir alive for the duration of the boot by leaking it; the OS
    // reclaims it on exit. (Source is read-only and short-lived.)
    std::mem::forget(scratch);
    let hot_state = produce_prover_hot_state();
    boot::setup::<Chaff>(
        NOCKCHAIN_KERNEL,
        cli,
        hot_state.as_slice(),
        "bench-source",
        None,
    )
    .await
}

async fn boot_target(args: &Args, data_dir: PathBuf) -> Result<NockApp<Chaff>, Box<dyn Error>> {
    let cli = boot::Cli {
        state_jam: args
            .target_state_jam
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        data_dir: Some(data_dir),
        ..base_cli(args.target_stack_size)
    };
    let hot_state = produce_prover_hot_state();
    boot::setup::<Chaff>(
        NOCKCHAIN_KERNEL,
        cli,
        hot_state.as_slice(),
        "bench-target",
        None,
    )
    .await
}

/// Export the target's current kernel state to a jam file (same format
/// `--state-jam` / `--target-state-jam` import).
async fn export_target(app: &NockApp<Chaff>, path: &str) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let load_state = app.export().await?;
    let exported = ExportedState::from_loadstate::<Chaff>(load_state);
    let bytes = exported.encode()?;
    std::fs::write(path, &bytes)?;
    info!(
        "bench: exported target state to {} ({} bytes)",
        path,
        bytes.len()
    );
    Ok(())
}

async fn peek_constants(app: &mut NockApp<Chaff>) -> Result<BlockchainConstants, Box<dyn Error>> {
    let slab = app
        .peek_handle(make_simple_path("blockchain-constants"))
        .await?
        .ok_or("peek [%blockchain-constants ~] returned ~")?;
    let noun = unsafe { slab.root() };
    let space = slab.noun_space();
    Ok(BlockchainConstants::from_noun(
        &noun.in_space(&space).noun(),
        &space,
    )?)
}

/// Peek a block at `height` and build its `heard-block` fact plus the
/// `heard-tx` facts for every raw transaction it references.
async fn extract_block(
    app: &mut NockApp<Chaff>,
    height: u64,
) -> Result<Option<ReplayBlock>, Box<dyn Error>> {
    let Some(mut page) = peek_page(app, height).await? else {
        return Ok(None);
    };
    let block_id = page_block_id(&mut page)?;
    let block_fact = make_fact_from_payload("heard-block", &page);

    let mut tx_facts = Vec::new();
    if let Some(mut txs_map) = peek_block_transactions(app, &block_id).await? {
        for tx_id in tx_ids_from_map(&mut txs_map)? {
            if let Some(raw_tx) = peek_raw_transaction(app, &tx_id).await? {
                tx_facts.push(make_fact_from_payload("heard-tx", &raw_tx));
            } else {
                warn!(
                    "bench: block {} (height {}) references tx {} but source has no raw tx; \
                     block may fail to validate",
                    block_id, height, tx_id
                );
            }
        }
    }

    Ok(Some(ReplayBlock {
        height,
        block_id,
        block_fact,
        tx_facts,
    }))
}

fn make_born_poke() -> NounSlab {
    let mut slab = NounSlab::new();
    let born = T(&mut slab, &[D(tas!(b"command")), D(tas!(b"born")), D(0)]);
    slab.set_root(born);
    slab
}

// ---- peek helpers (mirrored from vet_chain_signatures / checkpoint bench) ----

async fn peek_page(
    app: &mut NockApp<Chaff>,
    height: u64,
) -> Result<Option<NounSlab>, Box<dyn Error>> {
    Ok(app.peek_handle(make_heavy_n_path(height)).await?)
}

async fn peek_block_transactions(
    app: &mut NockApp<Chaff>,
    block_id: &str,
) -> Result<Option<NounSlab>, Box<dyn Error>> {
    Ok(app
        .peek_handle(make_string_path("block-transactions", block_id)?)
        .await?)
}

async fn peek_raw_transaction(
    app: &mut NockApp<Chaff>,
    tx_id: &str,
) -> Result<Option<NounSlab>, Box<dyn Error>> {
    Ok(app
        .peek_handle(make_string_path("raw-transaction", tx_id)?)
        .await?)
}

fn make_simple_path(tag: &str) -> NounSlab {
    let mut slab = NounSlab::new();
    let tag_noun = make_tas(&mut slab, tag).as_noun();
    let path = T(&mut slab, &[tag_noun, SIG]);
    slab.set_root(path);
    slab
}

fn make_heavy_n_path(height: u64) -> NounSlab {
    let mut slab = NounSlab::new();
    let path = T(&mut slab, &[D(tas!(b"heavy-n")), D(height), SIG]);
    slab.set_root(path);
    slab
}

fn make_string_path(tag: &str, value: &str) -> Result<NounSlab, Box<dyn Error>> {
    let mut slab = NounSlab::new();
    let tag_noun = make_tas(&mut slab, tag).as_noun();
    let value_noun = Atom::from_value(&mut slab, value.as_bytes())?.as_noun();
    let path = T(&mut slab, &[tag_noun, value_noun, SIG]);
    slab.set_root(path);
    Ok(slab)
}

fn make_fact_from_payload(tag: &str, payload: &NounSlab) -> NounSlab {
    let mut heard = NounSlab::new();
    heard.copy_from_slab(payload);
    let tag_noun = make_tas(&mut heard, tag).as_noun();
    heard.modify(|payload_noun| vec![tag_noun, payload_noun]);

    let mut fact = NounSlab::new();
    fact.copy_from_slab(&heard);
    fact.modify(|heard_noun| vec![D(tas!(b"fact")), D(0), heard_noun]);
    fact
}

#[allow(clippy::result_large_err)]
fn tx_ids_from_map(txs_map: &mut NounSlab) -> Result<Vec<String>, NockAppError> {
    let noun = unsafe { txs_map.root() };
    let space = txs_map.noun_space();
    if let Ok(atom) = noun.in_space(&space).as_atom() {
        if atom.as_u64()? == 0 {
            return Ok(Vec::new());
        }
    }
    let mut tx_ids = Vec::new();
    for entry in HoonMapIter::new(&noun.in_space(&space)) {
        if !entry.is_cell() {
            continue;
        }
        let [tx_id, _] = entry.uncell()?;
        tx_ids.push(tip5_hash_to_base58_stack(txs_map, tx_id.noun(), &space)?);
    }
    Ok(tx_ids)
}

#[allow(clippy::result_large_err)]
fn page_block_id(page: &mut NounSlab) -> Result<String, NockAppError> {
    let noun = unsafe { page.root() };
    let space = page.noun_space();
    let block_id = block_id_from_page(*noun, &space)?;
    tip5_hash_to_base58_stack(page, block_id, &space)
}

#[allow(clippy::result_large_err)]
fn block_id_from_page(page: Noun, space: &NounSpace) -> Result<Noun, NockAppError> {
    let page_cell = page.in_space(space).as_cell()?;
    match page_cell.head().as_atom() {
        Ok(version_atom) => {
            let version = version_atom.as_u64()?;
            if version == 1 {
                Ok(page_cell.tail().as_cell()?.head().noun())
            } else {
                Err(NockAppError::OtherError(format!(
                    "unsupported page version {}",
                    version
                )))
            }
        }
        Err(_) => Ok(page_cell.head().noun()),
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
