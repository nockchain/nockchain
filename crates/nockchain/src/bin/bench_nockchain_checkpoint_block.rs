#![allow(clippy::result_large_err)]

use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chaff::Chaff;
use clap::{ColorChoice, Parser};
use kernels_open_dumb::KERNEL as NOCKCHAIN_KERNEL;
use libp2p::PeerId;
use nockapp::kernel::boot::{self, NockStackSize, PmaSize, TraceOpts};
use nockapp::kernel::form::{Kernel, PmaCopyDetail, PmaTimingSample};
use nockapp::noun::slab::NounSlab;
use nockapp::save::{CheckpointBootstrapReader, SaveableCheckpoint};
use nockapp::utils::make_tas;
use nockapp::wire::Wire;
use nockapp::{AtomExt, NockApp, NockAppError};
use nockchain_libp2p_io::driver::Libp2pWire;
use nockchain_libp2p_io::tip5_util::tip5_hash_to_base58_stack;
use nockchain_math::noun_ext::NounMathExtHandle;
use nockchain_math::structs::HoonMapIter;
use nockvm::noun::{Atom, Noun, NounAllocator, NounSpace, D, SIG};
use nockvm_macros::tas;
use tempfile::{Builder, TempDir};
use tracing::{info, warn};
use zkvm_jetpack::hot::produce_prover_hot_state;

#[derive(Parser, Debug)]
#[command(
    name = "bench-nockchain-checkpoint-block",
    about = "Bench booting from checkpoint state and poking a single block."
)]
struct BenchArgs {
    #[arg(long, default_value = "../replay-checkpoints-51094")]
    target_checkpoint_dir: PathBuf,
    #[arg(long, default_value = "../nockchain-api-checkpoints-backup")]
    source_checkpoint_dir: PathBuf,
    #[arg(long, default_value_t = 51095)]
    block_height: u64,
    #[arg(long)]
    previous_height: Option<u64>,
    #[arg(long, default_value_t = true)]
    preload_raw_txs: bool,
    #[arg(long, value_enum, default_value_t = NockStackSize::Medium)]
    stack_size: NockStackSize,
    #[arg(long)]
    scratch_root: Option<PathBuf>,
}

enum ScratchDir {
    Temp(TempDir),
    Persistent(PathBuf),
}

impl ScratchDir {
    fn path(&self) -> &Path {
        match self {
            Self::Temp(temp) => temp.path(),
            Self::Persistent(path) => path.as_path(),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    nockvm::check_endian();
    std::env::set_var("NOCKAPP_DISABLE_METRICS", "1");
    std::env::set_var("GNORT_DISABLE", "1");
    std::env::set_var("NOCK_PMA_TIMING", "1");
    std::env::set_var("NOCK_PMA_TIMING_DETAIL", "1");

    let args = BenchArgs::parse();
    let previous_height = args
        .previous_height
        .or_else(|| args.block_height.checked_sub(1))
        .ok_or("block height must be greater than zero")?;

    let base_cli = boot::Cli {
        new: false,
        trace_opts: TraceOpts::default(),
        gc_interval: None,
        rotating_snapshot_interval_event_time: None,
        ephemeral: false,
        color: ColorChoice::Auto,
        state_jam: None,
        bootstrap_from_chkjam: None,
        export_state_jam: None,
        stack_size: args.stack_size,
        pma_initial_size: Some(PmaSize::from_words(args.stack_size.stack_words())),
        pma_reserved_size: None,
        data_dir: None,
        event_log_path: None,
        disable_fsync: false,
    };
    boot::init_default_tracing(&base_cli);

    let source_scratch = prepare_scratch_dir(
        &args.source_checkpoint_dir,
        "bench-source",
        args.scratch_root.as_deref(),
    )?;
    let target_scratch = prepare_scratch_dir(
        &args.target_checkpoint_dir,
        "bench-target",
        args.scratch_root.as_deref(),
    )?;

    info!(
        "bench: source_checkpoint_dir={} source_scratch={}",
        args.source_checkpoint_dir.display(),
        source_scratch.path().display()
    );
    info!(
        "bench: target_checkpoint_dir={} target_scratch={}",
        args.target_checkpoint_dir.display(),
        target_scratch.path().display()
    );

    let source_boot_start = Instant::now();
    info!("bench: loading source checkpoint into in-memory helper kernel");
    let mut source = load_source_peer_from_checkpoint(
        source_scratch.path().join("checkpoints"),
        args.stack_size,
    )
    .await?;
    let source_boot = source_boot_start.elapsed();
    let _ = source.take_pma_timing_samples_detailed();

    info!(
        "bench: extracting block at height={} from source peer",
        args.block_height
    );
    let (block_id, fact_poke) = extract_block_fact(&mut source, args.block_height).await?;
    info!(
        "bench: extracted block height={} block_id={}",
        args.block_height, block_id
    );
    let raw_tx_facts = if args.preload_raw_txs {
        info!(
            "bench: collecting raw transactions for block_id={} from source peer",
            block_id
        );
        let raw_txs = extract_block_raw_tx_facts(&mut source, &block_id).await?;
        info!(
            "bench: collected {} raw transaction fact poke(s) for preload",
            raw_txs.len()
        );
        raw_txs
    } else {
        Vec::new()
    };

    let target_boot_start = Instant::now();
    info!("bench: booting target peer for measured poke");
    let mut target = boot_peer(
        "bench-target",
        target_scratch.path().to_path_buf(),
        args.stack_size,
    )
    .await?;
    let target_boot = target_boot_start.elapsed();
    let boot_samples = target
        .take_pma_timing_samples_detailed()
        .unwrap_or_default();
    if !boot_samples.is_empty() {
        info!(
            "bench: discarded {} PMA timing sample(s) recorded during target boot",
            boot_samples.len()
        );
    }

    if let Some(mut page) = peek_page(&mut target, previous_height).await? {
        let prior_block_id = page_block_id(&mut page)?;
        info!(
            "bench: target before poke has height={} block_id={}",
            previous_height, prior_block_id
        );
    } else {
        warn!(
            "bench: target before poke did not return a block at height={}",
            previous_height
        );
    }

    if let Some(mut page) = peek_page(&mut target, args.block_height).await? {
        let existing_block_id = page_block_id(&mut page)?;
        return Err(format!(
            "target already has height {} with block_id {}; checkpoint input is not the expected pre-{} state",
            args.block_height, existing_block_id, args.block_height
        )
        .into());
    }

    if !raw_tx_facts.is_empty() {
        info!(
            "bench: preloading {} raw transaction(s) into target before measured block poke",
            raw_tx_facts.len()
        );
        let preload_start = Instant::now();
        for fact in raw_tx_facts {
            let _ = target
                .poke(Libp2pWire::Gossip(PeerId::random()).to_wire(), fact)
                .await?;
        }
        info!(
            "bench: preload_raw_txs_ms={:.3}",
            ms(preload_start.elapsed())
        );
        let preload_samples = target
            .take_pma_timing_samples_detailed()
            .unwrap_or_default();
        if !preload_samples.is_empty() {
            info!(
                "bench: discarded {} PMA timing sample(s) recorded during raw-tx preload",
                preload_samples.len()
            );
        }
    }

    let poke_start = Instant::now();
    let effects = target
        .poke(Libp2pWire::Gossip(PeerId::random()).to_wire(), fact_poke)
        .await?;
    let poke_wall = poke_start.elapsed();

    let poke_sample = target
        .take_pma_timing_samples_detailed()
        .and_then(|samples| samples.into_iter().last());
    let mut after_page = peek_page(&mut target, args.block_height)
        .await?
        .ok_or_else(|| {
            format!(
                "target still has no block at height {} after poke",
                args.block_height
            )
        })?;
    let after_block_id = page_block_id(&mut after_page)?;
    if after_block_id != block_id {
        return Err(format!(
            "target height {} resolved to block_id {} after poke, expected {}",
            args.block_height, after_block_id, block_id
        )
        .into());
    }

    info!(
        "bench: source_boot_ms={:.3} target_boot_ms={:.3} poke_wall_ms={:.3} effects={}",
        ms(source_boot),
        ms(target_boot),
        ms(poke_wall),
        effects.len()
    );
    info!(
        "bench: target now has height={} block_id={}",
        args.block_height, after_block_id
    );
    if let Some(sample) = poke_sample {
        report_pma_sample(sample);
    } else {
        warn!("bench: no PMA timing sample recorded for the measured poke");
    }

    Ok(())
}

fn prepare_scratch_dir(
    checkpoint_input: &Path,
    label: &str,
    scratch_root: Option<&Path>,
) -> Result<ScratchDir, Box<dyn Error>> {
    let checkpoints_dir = resolve_checkpoints_dir(checkpoint_input)?;
    let scratch = match scratch_root {
        Some(root) => {
            std::fs::create_dir_all(root)?;
            let path = root.join(format!("{label}-{}", unique_suffix()));
            std::fs::create_dir(&path)?;
            ScratchDir::Persistent(path)
        }
        None => ScratchDir::Temp(Builder::new().prefix(label).tempdir()?),
    };

    symlink_checkpoints_dir(&checkpoints_dir, &scratch.path().join("checkpoints"))?;
    Ok(scratch)
}

fn resolve_checkpoints_dir(path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let dir = if path.join("checkpoints").is_dir() {
        path.join("checkpoints")
    } else {
        path.to_path_buf()
    };

    if !dir.is_dir() {
        return Err(format!(
            "checkpoint path {} is not a directory and does not contain checkpoints/",
            path.display()
        )
        .into());
    }
    for file in ["0.chkjam", "1.chkjam"] {
        if !dir.join(file).exists() {
            warn!(
                "bench: checkpoint directory {} is missing {}",
                dir.display(),
                file
            );
        }
    }
    Ok(dir.canonicalize()?)
}

#[cfg(unix)]
fn symlink_checkpoints_dir(source: &Path, link: &Path) -> Result<(), Box<dyn Error>> {
    std::os::unix::fs::symlink(source, link)?;
    Ok(())
}

#[cfg(not(unix))]
fn symlink_checkpoints_dir(_source: &Path, _link: &Path) -> Result<(), Box<dyn Error>> {
    Err("checkpoint benchmark scratch dirs require unix symlinks".into())
}

async fn boot_peer(
    name: &str,
    data_dir: PathBuf,
    stack_size: NockStackSize,
) -> Result<NockApp<Chaff>, Box<dyn Error>> {
    let cli = boot::Cli {
        new: false,
        trace_opts: TraceOpts::default(),
        gc_interval: None,
        rotating_snapshot_interval_event_time: None,
        ephemeral: false,
        color: ColorChoice::Auto,
        state_jam: None,
        bootstrap_from_chkjam: None,
        export_state_jam: None,
        stack_size,
        pma_initial_size: Some(PmaSize::from_words(stack_size.stack_words())),
        pma_reserved_size: None,
        data_dir: Some(data_dir),
        event_log_path: None,
        disable_fsync: false,
    };
    let hot_state = produce_prover_hot_state();
    boot::setup::<Chaff>(NOCKCHAIN_KERNEL, cli, hot_state.as_slice(), name, None).await
}

async fn load_source_peer_from_checkpoint(
    checkpoints_dir: PathBuf,
    stack_size: NockStackSize,
) -> Result<NockApp<Chaff>, Box<dyn Error>> {
    let kernel = load_source_kernel_from_checkpoint(checkpoints_dir, stack_size).await?;
    let kernel_f =
        move |_| async move { Ok::<Kernel<SaveableCheckpoint>, nockapp::CrownError>(kernel) };
    Ok(NockApp::new(kernel_f).await?)
}

async fn load_source_kernel_from_checkpoint(
    checkpoints_dir: PathBuf,
    stack_size: NockStackSize,
) -> Result<Kernel<SaveableCheckpoint>, Box<dyn Error>> {
    let checkpoint = CheckpointBootstrapReader::<Chaff>::new(checkpoints_dir.clone())
        .load_latest(None)
        .await?
        .ok_or_else(|| {
            format!(
                "no checkpoint found in source checkpoint dir {}",
                checkpoints_dir.display()
            )
        })?;
    let hot_state = produce_prover_hot_state();
    let test_jets =
        boot::parse_test_jets(std::env::var("NOCK_TEST_JETS").unwrap_or_default().as_str());
    let kernel_bytes = Vec::from(NOCKCHAIN_KERNEL);
    let mut checkpoint = Some(checkpoint);
    let kernel: Kernel<SaveableCheckpoint> = match stack_size {
        NockStackSize::Tiny => {
            Kernel::load_with_hot_state_tiny(
                &kernel_bytes,
                checkpoint.take(),
                hot_state.as_slice(),
                test_jets.clone(),
                TraceOpts::default(),
                None,
            )
            .await?
        }
        NockStackSize::Small => {
            Kernel::load_with_hot_state_small(
                &kernel_bytes,
                checkpoint.take(),
                hot_state.as_slice(),
                test_jets.clone(),
                TraceOpts::default(),
                None,
            )
            .await?
        }
        NockStackSize::Normal => {
            Kernel::load_with_hot_state(
                &kernel_bytes,
                checkpoint.take(),
                hot_state.as_slice(),
                test_jets.clone(),
                TraceOpts::default(),
                None,
            )
            .await?
        }
        NockStackSize::Medium => {
            Kernel::load_with_hot_state_medium(
                &kernel_bytes,
                checkpoint.take(),
                hot_state.as_slice(),
                test_jets.clone(),
                TraceOpts::default(),
                None,
            )
            .await?
        }
        NockStackSize::Large => {
            Kernel::load_with_hot_state_large(
                &kernel_bytes,
                checkpoint.take(),
                hot_state.as_slice(),
                test_jets.clone(),
                TraceOpts::default(),
                None,
            )
            .await?
        }
        NockStackSize::Huge => {
            Kernel::load_with_hot_state_huge(
                &kernel_bytes,
                checkpoint.take(),
                hot_state.as_slice(),
                test_jets,
                TraceOpts::default(),
                None,
            )
            .await?
        }
    };
    Ok(kernel)
}

async fn extract_block_fact(
    app: &mut NockApp<Chaff>,
    block_height: u64,
) -> Result<(String, NounSlab), Box<dyn Error>> {
    let mut page = peek_page(app, block_height).await?.ok_or_else(|| {
        format!(
            "source peer did not return a block at height {}",
            block_height
        )
    })?;
    let block_id = page_block_id(&mut page)?;
    Ok((block_id, make_fact_from_payload("heard-block", &page)))
}

async fn extract_block_raw_tx_facts(
    app: &mut NockApp<Chaff>,
    block_id: &str,
) -> Result<Vec<NounSlab>, Box<dyn Error>> {
    let mut txs_map = peek_block_transactions(app, block_id)
        .await?
        .ok_or_else(|| {
            format!(
                "source peer returned no transactions for block {}",
                block_id
            )
        })?;
    let tx_ids = tx_ids_from_map(&mut txs_map)?;
    let mut facts = Vec::with_capacity(tx_ids.len());
    for tx_id in tx_ids {
        let raw_tx = peek_raw_transaction(app, &tx_id)
            .await?
            .ok_or_else(|| format!("source peer returned no raw transaction for tx {}", tx_id))?;
        facts.push(make_fact_from_payload("heard-tx", &raw_tx));
    }
    Ok(facts)
}

async fn peek_page(
    app: &mut NockApp<Chaff>,
    block_height: u64,
) -> Result<Option<NounSlab>, Box<dyn Error>> {
    Ok(app.peek_handle(make_heavy_n_path(block_height)).await?)
}

async fn peek_block_transactions(
    app: &mut NockApp<Chaff>,
    block_id: &str,
) -> Result<Option<NounSlab>, Box<dyn Error>> {
    let path = make_string_path("block-transactions", block_id)?;
    Ok(app.peek_handle(path).await?)
}

async fn peek_raw_transaction(
    app: &mut NockApp<Chaff>,
    tx_id: &str,
) -> Result<Option<NounSlab>, Box<dyn Error>> {
    let path = make_string_path("raw-transaction", tx_id)?;
    Ok(app.peek_handle(path).await?)
}

fn make_heavy_n_path(block_height: u64) -> NounSlab {
    let mut slab = NounSlab::new();
    let path = nockvm::noun::T(&mut slab, &[D(tas!(b"heavy-n")), D(block_height), SIG]);
    slab.set_root(path);
    slab
}

fn make_string_path(tag: &str, value: &str) -> Result<NounSlab, Box<dyn Error>> {
    let mut slab = NounSlab::new();
    let tag_noun = make_tas(&mut slab, tag).as_noun();
    let value_noun = Atom::from_value(&mut slab, value.as_bytes())?.as_noun();
    let path = nockvm::noun::T(&mut slab, &[tag_noun, value_noun, SIG]);
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

fn report_pma_sample(sample: PmaTimingSample) {
    info!(
        "bench: poke_event_ms={:.3} poke_pma_copy_ms={:.3} poke_total_ms={:.3}",
        ms(sample.event),
        ms(sample.pma_copy),
        ms(sample.event + sample.pma_copy)
    );
    if let Some(detail) = sample.detail {
        report_pma_detail(detail);
    }
}

fn report_pma_detail(detail: PmaCopyDetail) {
    report_segment("warm", detail.warm);
    report_segment("test_jets", detail.test_jets);
    report_segment("hot", detail.hot);
    report_segment("cache", detail.cache);
    report_segment("cold", detail.cold);
    report_segment("arvo", detail.arvo);
}

fn report_segment(label: &str, segment: nockapp::kernel::form::PmaCopySegment) {
    info!(
        "bench: poke_pma_{}_ms={:.3} poke_pma_{}_alloc_mib={:.3}",
        label,
        ms(segment.elapsed),
        label,
        mib(segment.alloc_words)
    );
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn mib(words: usize) -> f64 {
    (words as f64 * std::mem::size_of::<u64>() as f64) / (1024.0 * 1024.0)
}

fn unique_suffix() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}-{}", now.as_secs(), now.subsec_nanos())
}
