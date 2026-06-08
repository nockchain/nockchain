use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::BigInt;
use diesel::sqlite::SqliteConnection;
use nockapp::kernel::boot::{default_boot_cli, setup_, NockStackSize, PmaSize, SetupResult};
use nockapp::nockapp::wire::{SystemWire, Wire};
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::NockApp;
use nockvm::mem::NOCK_STACK_SIZE_TINY;
use nockvm::noun::{NounAllocator, NounSpace, D, T};
use nockvm::offset::PmaOffsetWords;
use nockvm::pma::Pma;
use nockvm_macros::tas;
use tempfile::TempDir;

// Leave roughly one existing PMA allocation worth of free space plus this small headroom. Boot can re-preserve the loaded state, then the next event has too little free space unless online growth runs before the durable append path.
const LOW_FREE_FIXTURE_EXTRA_WORDS: u64 = 700;

#[derive(QueryableByName)]
struct I64ValueRow {
    #[diesel(sql_type = BigInt)]
    value: i64,
}

pub(crate) fn run_regression() -> Result<(), Box<dyn Error>> {
    nockvm::check_endian();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run())
}

async fn run() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let data_dir = temp.path().join("pma-event-preflight-growth-regression");
    let jam = load_test_jam()?;

    println!("stage 1: create durable event-1 fixture");
    let mut first = boot_app(&jam, &data_dir).await?;
    poke_inc(&mut first).await?;
    assert_counter_state(&mut first, 1).await?;
    stop_app(first).await?;
    assert_eq!(sqlite_max_event_num(&data_dir)?, 1);

    let active_pma = active_runtime_pma(&data_dir)?;
    let before_force = Pma::read_file_metadata(&active_pma)?;
    println!(
        "active PMA before filler: path={} data_words={} alloc_words={} free_words={}",
        active_pma.display(),
        before_force.data_words,
        before_force.alloc_words,
        before_force
            .data_words
            .saturating_sub(before_force.alloc_words)
    );
    let forced_free_words = before_force
        .alloc_words
        .checked_add(LOW_FREE_FIXTURE_EXTRA_WORDS)
        .ok_or_else(|| std::io::Error::other("forced free word calculation overflowed"))?;
    force_pma_free_words(&active_pma, forced_free_words)?;
    let forced = Pma::read_file_metadata(&active_pma)?;
    println!(
        "active PMA forced low-free before event: path={} data_words={} alloc_words={} free_words={} fixture_extra_words={}",
        active_pma.display(),
        forced.data_words,
        forced.alloc_words,
        forced.data_words.saturating_sub(forced.alloc_words),
        LOW_FREE_FIXTURE_EXTRA_WORDS
    );

    println!("stage 2: reboot low-free fixture and issue one accepted inc event");
    let mut second = boot_app(&jam, &data_dir).await?;
    assert_counter_state(&mut second, 1).await?;
    let sqlite_before = sqlite_max_event_num(&data_dir)?;
    let pma_before_event = active_runtime_pma(&data_dir)?;
    let pma_before_event_metadata = Pma::read_file_metadata(&pma_before_event)?;
    println!(
        "before event: sqlite_max={} active_pma={} data_words={} alloc_words={} free_words={}",
        sqlite_before,
        pma_before_event.display(),
        pma_before_event_metadata.data_words,
        pma_before_event_metadata.alloc_words,
        pma_before_event_metadata
            .data_words
            .saturating_sub(pma_before_event_metadata.alloc_words)
    );

    let poke_result = second.poke(SystemWire.to_wire(), inc_poke()).await;
    let sqlite_after = sqlite_max_event_num(&data_dir)?;
    match poke_result {
        Ok(_) => {
            assert_counter_state(&mut second, 2).await?;
            let pma_after_event = active_runtime_pma(&data_dir)?;
            let pma_after_event_metadata = Pma::read_file_metadata(&pma_after_event)?;
            println!(
                "after event: sqlite_max={} active_pma={} data_words={} alloc_words={} free_words={}",
                sqlite_after,
                pma_after_event.display(),
                pma_after_event_metadata.data_words,
                pma_after_event_metadata.alloc_words,
                pma_after_event_metadata
                    .data_words
                    .saturating_sub(pma_after_event_metadata.alloc_words)
            );
            if sqlite_after != sqlite_before + 1 {
                return Err(std::io::Error::other(format!(
                    "accepted event did not advance SQLite exactly once: before={sqlite_before} after={sqlite_after}"
                ))
                .into());
            }
            if pma_after_event_metadata.data_words <= pma_before_event_metadata.data_words {
                return Err(std::io::Error::other(format!(
                    "event succeeded but active PMA was not grown from low-free fixture: before_data_words={} after_data_words={}",
                    pma_before_event_metadata.data_words, pma_after_event_metadata.data_words
                ))
                .into());
            }
            stop_app(second).await?;
            println!("event-time PMA preflight grew PMA before accepting/logging the event");
            Ok(())
        }
        Err(err) => {
            let message = err.to_string();
            if sqlite_after > sqlite_before {
                return Err(std::io::Error::other(format!(
                    "event log advanced despite failed low-free PMA event: sqlite_before={sqlite_before} sqlite_after={sqlite_after} poke_error={message}"
                ))
                .into());
            }
            Err(std::io::Error::other(format!(
                "low-free PMA event was rejected before SQLite append; this is fail-safe but does not satisfy growth regression: sqlite_before={sqlite_before} sqlite_after={sqlite_after} poke_error={message}"
            ))
            .into())
        }
    }
}

fn load_test_jam() -> Result<Vec<u8>, Box<dyn Error>> {
    let mut possible_paths = Vec::new();
    if let Some(manifest_dir) = option_env!("CARGO_MANIFEST_DIR") {
        possible_paths.push(
            Path::new(manifest_dir)
                .join("test-jams")
                .join("test-ker.jam"),
        );
    }
    possible_paths.push(Path::new("open/crates/nockapp/test-jams").join("test-ker.jam"));
    possible_paths.push(Path::new("test-jams").join("test-ker.jam"));

    for path in &possible_paths {
        if let Ok(bytes) = fs::read(path) {
            return Ok(bytes);
        }
    }

    Err(std::io::Error::other(format!(
        "failed to read test-ker.jam from any candidate path: {:?}",
        possible_paths
    ))
    .into())
}

async fn boot_app(jam: &[u8], data_dir: &Path) -> Result<NockApp<NockJammer>, Box<dyn Error>> {
    let mut cli = default_boot_cli(false);
    cli.data_dir = Some(data_dir.to_path_buf());
    cli.pma_initial_size = Some(PmaSize::from_words(NOCK_STACK_SIZE_TINY));
    cli.stack_size = NockStackSize::Tiny;
    cli.gc_interval = None;
    cli.rotating_snapshot_interval_event_time = None;
    cli.disable_fsync = true;
    match setup_::<NockJammer>(jam, cli, &[], "pma-event-preflight-growth-regression", None).await?
    {
        SetupResult::App(app) => Ok(app),
        SetupResult::ExportedState => Err(std::io::Error::other("unexpected state export").into()),
    }
}

fn inc_poke() -> NounSlab {
    let mut slab = NounSlab::new();
    let space = NounSpace::empty();
    slab.copy_into(D(tas!(b"inc")), &space);
    slab
}

fn state_peek() -> NounSlab {
    let mut slab = NounSlab::new();
    let peek = T(&mut slab, &[D(tas!(b"state")), D(0)]);
    slab.set_root(peek);
    slab
}

async fn poke_inc(app: &mut NockApp<NockJammer>) -> Result<(), Box<dyn Error>> {
    app.poke(SystemWire.to_wire(), inc_poke()).await?;
    Ok(())
}

async fn assert_counter_state(
    app: &mut NockApp<NockJammer>,
    expected: u64,
) -> Result<(), Box<dyn Error>> {
    let exported = app.export().await?;
    if exported.event_num != expected {
        return Err(std::io::Error::other(format!(
            "event number mismatch: expected={expected} actual={}",
            exported.event_num
        ))
        .into());
    }
    let actual = app.peek(state_peek()).await?;
    let space = actual.noun_space();
    let root = unsafe { *actual.root() };
    let value = root
        .in_space(&space)
        .slot(7)?
        .as_cell()?
        .tail()
        .noun()
        .in_space(&space)
        .as_atom()?
        .as_u64()?;
    if value != expected {
        return Err(std::io::Error::other(format!(
            "counter state mismatch: expected={expected} actual={value}"
        ))
        .into());
    }
    Ok(())
}

async fn stop_app(mut app: NockApp<NockJammer>) -> Result<(), Box<dyn Error>> {
    let handle = app.get_handle();
    handle.exit.exit(0).await?;
    app.run().await?;
    Ok(())
}

fn sqlite_max_event_num(data_dir: &Path) -> Result<u64, Box<dyn Error>> {
    let path = data_dir.join("event-log.sqlite3");
    let mut conn = SqliteConnection::establish(
        path.to_str()
            .ok_or_else(|| std::io::Error::other(format!("non-utf8 sqlite path: {path:?}")))?,
    )?;
    let row = sql_query("SELECT COALESCE(MAX(event_num), 0) AS value FROM events")
        .get_result::<I64ValueRow>(&mut conn)?;
    Ok(u64::try_from(row.value)?)
}

fn force_pma_free_words(path: &Path, free_words: u64) -> Result<(), Box<dyn Error>> {
    let metadata = Pma::read_file_metadata(path)?;
    if metadata.data_words <= free_words {
        return Err(std::io::Error::other(format!(
            "PMA too small to force {free_words} free words: data_words={}",
            metadata.data_words
        ))
        .into());
    }
    let new_alloc_words = metadata.data_words - free_words;
    let mut pma = Pma::open(path.to_path_buf())?;
    pma.reset_to(PmaOffsetWords::from_words(new_alloc_words));
    pma.sync_trailer()?;
    pma.sync_file()?;
    Ok(())
}

fn active_runtime_pma(data_dir: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let pma_dir = data_dir.join("pma");
    let mut candidates = Vec::new();
    for idx in [0, 1] {
        let pma_path = pma_dir.join(format!("{idx}.pma"));
        let meta_path = pma_dir.join(format!("{idx}.meta"));
        if pma_path.exists() && meta_path.exists() {
            let modified = fs::metadata(&meta_path)
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            candidates.push((pma_path, modified));
        }
    }
    candidates.sort_by_key(|(_, modified)| *modified);
    candidates.pop().map(|(path, _)| path).ok_or_else(|| {
        std::io::Error::other(format!(
            "no meta-paired runtime PMA found in {}",
            pma_dir.display()
        ))
        .into()
    })
}
