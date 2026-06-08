#![allow(clippy::result_large_err)]
//! Audits Schnorr-signature encoding invariants across a synced chain. Boots a
//! peer from a state jam (a full kernel state) and checks every on-chain
//! signature for two invariants:
//!
//!   * scalar range: `0 < chal < g-order` and `0 < sig < g-order`.
//!   * limb canonicality: every 32-bit limb is `< 2^32`.
//!
//! Read-only: peeks never mutate the chain. The fast `raw-txs` mode reads the
//! whole raw-txs map in one peek; `block-walk` re-derives txs per block.

use std::error::Error;
use std::path::PathBuf;

use clap::Parser;
use ibig::UBig;
use kernels_open_dumb::KERNEL as NOCKCHAIN_KERNEL;
use nockapp::kernel::boot::{self, NockStackSize, PmaSize, TraceOpts};
use nockapp::noun::slab::NounSlab;
use nockapp::utils::make_tas;
use nockapp::{AtomExt, NockApp, NockAppError};
use nockchain_libp2p_io::tip5_util::tip5_hash_to_base58_stack;
use nockchain_math::belt::{based_check, Belt};
use nockchain_math::crypto::cheetah::G_ORDER;
use nockchain_math::noun_ext::NounMathExtHandle;
use nockchain_math::structs::HoonMapIter;
use nockchain_types::tx_engine::common::SchnorrSignature;
use nockchain_types::tx_engine::{v0, v1};
use nockvm::noun::{Atom, Noun, NounAllocator, NounSpace, D, SIG};
use nockvm_macros::tas;
use noun_serde::NounDecode;
use tempfile::Builder;
use tracing::{info, warn};
use zkvm_jetpack::hot::produce_prover_hot_state;

type Chaff = chaff::Chaff;

#[derive(Parser, Debug)]
#[command(
    name = "vet-chain-signatures",
    about = "Scan every on-chain Schnorr signature for scalar range and limb canonicality."
)]
struct Args {
    /// Path to the state jam (full kernel state) to boot from.
    #[arg(long)]
    state_jam: PathBuf,
    /// First block height to scan (inclusive). Only used in `--mode block-walk`.
    #[arg(long, default_value_t = 1)]
    start: u64,
    /// Last block height to scan (inclusive). Only used in `--mode block-walk`.
    #[arg(long, default_value_t = 41035)]
    end: u64,
    /// Scan mode: `raw-txs` (one peek of the whole raw-txs map; fast) or
    /// `block-walk` (peek every block + raw tx; authoritative-by-accepted-chain
    /// but slow).
    #[arg(long, default_value = "raw-txs")]
    mode: String,
    /// Stack/PMA size; a 4.6G state jam needs a large arena.
    #[arg(long, value_enum, default_value_t = NockStackSize::Huge)]
    stack_size: NockStackSize,
    /// Log a progress line every N heights/txs.
    #[arg(long, default_value_t = 5000)]
    progress_every: u64,
}

#[derive(Default)]
struct Stats {
    heights_scanned: u64,
    blocks_found: u64,
    blocks_missing: u64,
    raw_txs: u64,
    v0_txs: u64,
    v1_txs: u64,
    undecodable_txs: u64,
    signatures: u64,
    out_of_range: u64,
    non_canonical: u64,
    v1_pkh_sigs_valid: u64,
    v1_pkh_sigs_invalid: u64,
    // Any atom leaf anywhere in a raw-tx noun that is not a base-field element
    // (>= PRIME, or wider than u64). Every field `based:raw-tx` checks
    // (id/hashes/locks/gift/assets/sig-limbs) is already based, so the only
    // place a non-based leaf can hide is a note-data or hax *value* -- the
    // values `based:note-data` / `based-noun` now require to be based. Zero
    // here over the accepted chain proves that requirement rejects no tx.
    tx_leaves_scanned: u64,
    non_based_leaves: u64,
    txs_with_non_based: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    nockvm::check_endian();
    std::env::set_var("NOCKAPP_DISABLE_METRICS", "1");
    std::env::set_var("GNORT_DISABLE", "1");

    let args = Args::parse();
    let cli = boot::Cli {
        new: true,
        trace_opts: TraceOpts::default(),
        gc_interval: None,
        rotating_snapshot_interval_event_time: None,
        ephemeral: false,
        color: clap::ColorChoice::Auto,
        state_jam: Some(args.state_jam.to_string_lossy().into_owned()),
        bootstrap_from_chkjam: None,
        export_state_jam: None,
        stack_size: args.stack_size,
        pma_initial_size: Some(PmaSize::from_words(args.stack_size.stack_words())),
        pma_reserved_size: None,
        data_dir: None,
        event_log_path: None,
        disable_fsync: true,
    };
    boot::init_default_tracing(&cli);

    let scratch = Builder::new().prefix("vet-chain-sigs").tempdir()?;
    let cli = boot::Cli {
        data_dir: Some(scratch.path().to_path_buf()),
        ..cli
    };

    info!(
        "vet: booting peer from state jam {} (this loads the full chain state)",
        args.state_jam.display()
    );
    let hot_state = produce_prover_hot_state();
    let mut app: NockApp<Chaff> =
        boot::setup::<Chaff>(NOCKCHAIN_KERNEL, cli, hot_state.as_slice(), "vet", None).await?;
    info!(
        "vet: boot complete; scanning heights {}..={}",
        args.start, args.end
    );

    let g_order: &UBig = &G_ORDER;
    let two_32 = UBig::from(1u64 << 32);
    let zero = UBig::from(0u32);

    let mut stats = Stats::default();

    match args.mode.as_str() {
        "raw-txs" => {
            scan_raw_txs_map(
                &mut app, g_order, &two_32, &zero, args.progress_every, &mut stats,
            )
            .await?
        }
        "block-walk" => {
            scan_block_walk(
                &mut app,
                args.start,
                args.end,
                g_order,
                &two_32,
                &zero,
                args.progress_every.max(1),
                &mut stats,
            )
            .await?
        }
        other => return Err(format!("unknown --mode {other}; use raw-txs or block-walk").into()),
    }

    info!(
        "================ vet: scan complete (mode={}) ================",
        args.mode
    );
    info!("heights scanned        : {}", stats.heights_scanned);
    info!(
        "blocks found / missing : {} / {}",
        stats.blocks_found, stats.blocks_missing
    );
    info!(
        "raw txs (v0 / v1 / ?)  : {} ({} / {} / {})",
        stats.raw_txs, stats.v0_txs, stats.v1_txs, stats.undecodable_txs
    );
    info!("signatures checked     : {}", stats.signatures);
    info!("OUT-OF-RANGE scalars   : {}", stats.out_of_range);
    info!("NON-CANONICAL limbs    : {}", stats.non_canonical);
    if stats.out_of_range == 0 {
        info!("RESULT: every signature scalar is in range (0 < scalar < g-order).");
    } else {
        warn!(
            "RESULT: {} signature scalar(s) are OUT OF RANGE.",
            stats.out_of_range
        );
    }
    if stats.non_canonical == 0 {
        info!("RESULT: every 32-bit limb is canonical (< 2^32).");
    } else {
        warn!(
            "RESULT: {} signature(s) have NON-CANONICAL limbs.",
            stats.non_canonical
        );
    }
    info!(
        "v1 %pkh sigs verified (valid / invalid) : {} / {}",
        stats.v1_pkh_sigs_valid, stats.v1_pkh_sigs_invalid
    );
    if stats.v1_pkh_sigs_invalid == 0 {
        info!("RESULT: every v1 %pkh signature verifies via the shared verify_affine.");
    } else {
        warn!(
            "RESULT: {} v1 %pkh signature(s) did not verify (raw-txs may include \
             mempool-rejected txs; in block-walk mode this would be unexpected).",
            stats.v1_pkh_sigs_invalid
        );
    }

    info!(
        "raw-tx leaves scanned  : {} (non-based: {} across {} tx)",
        stats.tx_leaves_scanned, stats.non_based_leaves, stats.txs_with_non_based
    );
    if stats.non_based_leaves == 0 {
        info!(
            "RESULT: every raw-tx atom leaf is based; the note-data/hax value \
             basedness requirement rejects no tx in this scan."
        );
    } else {
        warn!(
            "RESULT: {} non-based leaf/leaves across {} tx(s); the value-basedness \
             requirement may reject real, already-accepted txs.",
            stats.non_based_leaves, stats.txs_with_non_based
        );
    }

    if stats.out_of_range != 0 || stats.non_canonical != 0 || stats.non_based_leaves != 0 {
        std::process::exit(1);
    }
    Ok(())
}

enum TxVersion {
    V0,
    V1,
}

/// Iterate the whole `raw-txs` map (one peek) and check every signature.
async fn scan_raw_txs_map(
    app: &mut NockApp<Chaff>,
    g_order: &UBig,
    two_32: &UBig,
    zero: &UBig,
    progress_every: u64,
    stats: &mut Stats,
) -> Result<(), Box<dyn Error>> {
    let Some(map) = app
        .peek_handle(make_simple_path("raw-transactions"))
        .await?
    else {
        return Err("peek [%raw-transactions ~] returned ~ (node has no raw-txs map)".into());
    };
    let noun = unsafe { map.root() };
    let space = map.noun_space();
    if let Ok(atom) = noun.in_space(&space).as_atom() {
        if atom.as_u64()? == 0 {
            warn!("vet: raw-txs map is empty");
            return Ok(());
        }
    }
    // Each entry: [tx-id [raw-tx heard-at]].
    for entry in HoonMapIter::new(&noun.in_space(&space)) {
        if !entry.is_cell() {
            continue;
        }
        let [_tx_id, value] = entry.uncell()?;
        let [raw_tx, _heard_at] = value.uncell()?;
        stats.raw_txs += 1;
        if progress_every > 0 && stats.raw_txs.is_multiple_of(progress_every) {
            info!(
                "vet: progress raw_txs={} sigs={} out_of_range={} non_canonical={}",
                stats.raw_txs, stats.signatures, stats.out_of_range, stats.non_canonical
            );
        }
        let mut sigs: Vec<SchnorrSignature> = Vec::new();
        let label = stats.raw_txs.to_string();
        scan_non_based_leaves(&raw_tx.noun(), &space, 0, &label, stats);
        match decode_signatures(&raw_tx.noun(), &space, &mut sigs, stats) {
            Ok(TxVersion::V0) => stats.v0_txs += 1,
            Ok(TxVersion::V1) => stats.v1_txs += 1,
            Err(_) => {
                stats.undecodable_txs += 1;
                continue;
            }
        }
        for sig in &sigs {
            stats.signatures += 1;
            check_scalar("chal", &sig.chal, 0, &label, g_order, two_32, zero, stats);
            check_scalar("sig", &sig.sig, 0, &label, g_order, two_32, zero, stats);
        }
    }
    Ok(())
}

/// Authoritative walk over the accepted chain: every tx in every block.
#[allow(clippy::too_many_arguments)]
async fn scan_block_walk(
    app: &mut NockApp<Chaff>,
    start: u64,
    end: u64,
    g_order: &UBig,
    two_32: &UBig,
    zero: &UBig,
    progress_every: u64,
    stats: &mut Stats,
) -> Result<(), Box<dyn Error>> {
    for height in start..=end {
        stats.heights_scanned += 1;
        if progress_every > 0 && height.is_multiple_of(progress_every) {
            info!(
                "vet: progress height={} blocks={} raw_txs={} sigs={} out_of_range={} non_canonical={}",
                height, stats.blocks_found, stats.raw_txs, stats.signatures, stats.out_of_range, stats.non_canonical
            );
        }
        let Some(mut page) = peek_page(app, height).await? else {
            stats.blocks_missing += 1;
            continue;
        };
        stats.blocks_found += 1;
        let block_id = page_block_id(&mut page)?;
        let Some(mut txs_map) = peek_block_transactions(app, &block_id).await? else {
            continue;
        };
        let tx_ids = tx_ids_from_map(&mut txs_map)?;
        for tx_id in tx_ids {
            let Some(raw_tx) = peek_raw_transaction(app, &tx_id).await? else {
                continue;
            };
            stats.raw_txs += 1;
            let space = raw_tx.noun_space();
            let root = unsafe { raw_tx.root() };
            let mut sigs: Vec<SchnorrSignature> = Vec::new();
            scan_non_based_leaves(&root.in_space(&space).noun(), &space, height, &tx_id, stats);
            match decode_signatures(&root.in_space(&space).noun(), &space, &mut sigs, stats) {
                Ok(TxVersion::V0) => stats.v0_txs += 1,
                Ok(TxVersion::V1) => stats.v1_txs += 1,
                Err(_) => {
                    stats.undecodable_txs += 1;
                    continue;
                }
            }
            for sig in &sigs {
                stats.signatures += 1;
                check_scalar(
                    "chal", &sig.chal, height, &tx_id, g_order, two_32, zero, stats,
                );
                check_scalar(
                    "sig", &sig.sig, height, &tx_id, g_order, two_32, zero, stats,
                );
            }
        }
    }
    Ok(())
}

/// Decode a raw tx noun (v1 then v0), append all embedded Schnorr signatures,
/// and — for v1 `%pkh` witness spends — actually verify each signature through
/// the same `verify_affine` the jet uses (sig-hash reconstruction + canonicality
/// + range + curve check), tallying valid/invalid into `stats`.
fn decode_signatures(
    noun: &Noun,
    space: &NounSpace,
    out: &mut Vec<SchnorrSignature>,
    stats: &mut Stats,
) -> Result<TxVersion, Box<dyn Error>> {
    // v1 raw-tx head is the version atom (1); v0 head is the id (a cell).
    if let Ok(tx) = v1::RawTx::from_noun(noun, space) {
        for (_, spend) in &tx.spends.0 {
            match spend {
                v1::Spend::Witness(s1) => {
                    for entry in &s1.witness.pkh_signature.0 {
                        out.push(entry.signature.clone());
                        match s1.verify_pkh_signature(entry) {
                            Ok(()) => stats.v1_pkh_sigs_valid += 1,
                            Err(_) => stats.v1_pkh_sigs_invalid += 1,
                        }
                    }
                }
                v1::Spend::Legacy(s0) => {
                    for (_pk, sig) in &s0.signature.0 {
                        out.push(sig.clone());
                    }
                }
            }
        }
        return Ok(TxVersion::V1);
    }

    let tx = v0::RawTx::from_noun(noun, space)?;
    for (_, input) in &tx.inputs.0 {
        if let Some(sig_map) = &input.spend.signature {
            for (_pk, sig) in &sig_map.0 {
                out.push(sig.clone());
            }
        }
    }
    Ok(TxVersion::V0)
}

#[allow(clippy::too_many_arguments)]
fn check_scalar(
    which: &str,
    limbs: &[Belt; 8],
    height: u64,
    tx_id: &str,
    g_order: &UBig,
    two_32: &UBig,
    zero: &UBig,
    stats: &mut Stats,
) {
    let mut scalar = UBig::from(0u32);
    let mut canonical = true;
    for (i, limb) in limbs.iter().enumerate() {
        if UBig::from(limb.0) >= *two_32 {
            canonical = false;
        }
        scalar += UBig::from(limb.0) << (32 * i);
    }
    // scalar range: 0 < scalar < g-order (matches Hoon +verify).
    if scalar <= *zero || scalar >= *g_order {
        stats.out_of_range += 1;
        warn!(
            "vet: OUT-OF-RANGE {} scalar at height {} tx {} (limbs={:?})",
            which,
            height,
            tx_id,
            limbs.iter().map(|b| b.0).collect::<Vec<_>>()
        );
    }
    // limb canonicality: every 32-bit limb < 2^32.
    if !canonical {
        stats.non_canonical += 1;
        warn!(
            "vet: NON-CANONICAL {} limb at height {} tx {} (limbs={:?})",
            which,
            height,
            tx_id,
            limbs.iter().map(|b| b.0).collect::<Vec<_>>()
        );
    }
}

/// Walk every atom leaf of a raw-tx noun and tally any that are not base-field
/// elements. This over-approximates the `based:raw-tx` note-data/hax value
/// checks (it inspects every leaf, not only the checked fields), so a zero
/// count is a sufficient proof that those checks reject no tx in this set; a
/// non-zero count localizes (via the warned example) where a non-based leaf
/// lives so it can be checked against the actual `based` cover.
fn scan_non_based_leaves(
    noun: &Noun,
    space: &NounSpace,
    height: u64,
    label: &str,
    stats: &mut Stats,
) {
    let mut found_in_tx = false;
    // Explicit work-stack: raw txs can be deep; avoid blowing the native stack.
    let mut stack: Vec<Noun> = vec![*noun];
    while let Some(n) = stack.pop() {
        let n = n.in_space(space);
        if let Ok(cell) = n.as_cell() {
            stack.push(cell.head().noun());
            stack.push(cell.tail().noun());
            continue;
        }
        stats.tx_leaves_scanned += 1;
        // A leaf is based iff it fits in u64 AND is < PRIME. An atom wider than
        // u64 is unconditionally >= PRIME, hence not based.
        let based = match n.as_atom() {
            Ok(atom) => match atom.as_u64() {
                Ok(v) => based_check(v),
                Err(_) => false,
            },
            Err(_) => false,
        };
        if !based {
            stats.non_based_leaves += 1;
            if !found_in_tx {
                found_in_tx = true;
                stats.txs_with_non_based += 1;
            }
            if stats.non_based_leaves <= 20 {
                let shown = n
                    .as_atom()
                    .ok()
                    .and_then(|a| a.as_u64().ok())
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "<atom wider than u64>".to_string());
                warn!(
                    "vet: NON-BASED leaf at height {} tx {} (atom={})",
                    height, label, shown
                );
            }
        }
    }
}

// ---- peek helpers (mirrored from bench_nockchain_checkpoint_block) ----

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
    let path = nockvm::noun::T(&mut slab, &[tag_noun, SIG]);
    slab.set_root(path);
    slab
}

fn make_heavy_n_path(height: u64) -> NounSlab {
    let mut slab = NounSlab::new();
    let path = nockvm::noun::T(&mut slab, &[D(tas!(b"heavy-n")), D(height), SIG]);
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
