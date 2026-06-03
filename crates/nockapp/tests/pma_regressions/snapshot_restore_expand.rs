use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::{BigInt, Text};
use diesel::sqlite::SqliteConnection;
use nockapp::kernel::boot::{default_boot_cli, setup_, NockStackSize, PmaSize, SetupResult};
use nockapp::nockapp::wire::{SystemWire, Wire};
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::NockApp;
use nockvm::mem::{NOCK_STACK_SIZE_SMALL, NOCK_STACK_SIZE_TINY};
use nockvm::noun::{NounAllocator, NounSpace, D, T};
use nockvm::pma::Pma;
use nockvm_macros::tas;
use tempfile::TempDir;

use crate::pma_regressions::pma_meta::PmaPersistMetadataForTest;

#[derive(QueryableByName)]
struct I64ValueRow {
    #[diesel(sql_type = BigInt)]
    value: i64,
}

#[derive(QueryableByName)]
struct SnapshotRow {
    #[diesel(sql_type = Text)]
    pma_path: String,
    #[diesel(sql_type = Text)]
    manifest_path: String,
    #[diesel(sql_type = BigInt)]
    event_num: i64,
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
    let data_dir = temp.path().join("pma-snapshot-restore-expand-regression");
    let jam = load_test_jam()?;

    println!("stage 1: create tiny-PMA snapshot plus two committed events");
    let mut first = boot_app(&jam, &data_dir, NockStackSize::Tiny).await?;
    poke_inc(&mut first).await?;
    poke_inc(&mut first).await?;
    assert_counter_state(&mut first, 2).await?;
    stop_app(first).await?;

    assert_eq!(sqlite_max_event_num(&data_dir)?, 2);
    assert_eq!(max_runtime_pma_meta_event(&data_dir)?, 2);

    let snapshot = newest_ready_snapshot(&data_dir)?;
    if snapshot.event_num >= sqlite_max_event_num(&data_dir)? {
        return Err(std::io::Error::other(format!(
            "snapshot must be a replay base behind SQLite max: snapshot_event={} sqlite_max={}",
            snapshot.event_num,
            sqlite_max_event_num(&data_dir)?
        ))
        .into());
    }
    let snapshot_pma_metadata = Pma::read_file_metadata(&snapshot.pma_path)?;
    let snapshot_pma = Pma::open(snapshot.pma_path.clone())?;
    let snapshot_runtime_metadata = snapshot_pma.file_metadata()?;
    println!(
        "ready snapshot selected: pma={} manifest={} event_num={} data_words={} alloc_words={} free_words={}",
        snapshot.pma_path.display(),
        snapshot.manifest_path.display(),
        snapshot.event_num,
        snapshot_pma_metadata.data_words,
        snapshot_pma_metadata.alloc_words,
        snapshot_pma_metadata
            .data_words
            .saturating_sub(snapshot_pma_metadata.alloc_words)
    );
    if snapshot_pma_metadata.data_words != NOCK_STACK_SIZE_TINY as u64 {
        return Err(std::io::Error::other(format!(
            "snapshot fixture should have tiny PMA size: expected={} actual={}",
            NOCK_STACK_SIZE_TINY, snapshot_pma_metadata.data_words
        ))
        .into());
    }
    if snapshot_runtime_metadata.reserved_words <= snapshot_runtime_metadata.capacity_words {
        return Err(std::io::Error::other(format!(
            "snapshot PMA fixture should have a reservation larger than current capacity: capacity_words={} reserved_words={}",
            snapshot_runtime_metadata.capacity_words, snapshot_runtime_metadata.reserved_words
        ))
        .into());
    }
    let reserved_data_bytes = snapshot_runtime_metadata.reserved_words.saturating_mul(8);
    if snapshot_runtime_metadata.apparent_file_bytes >= reserved_data_bytes {
        return Err(std::io::Error::other(format!(
            "snapshot copied reserved maximum instead of current capacity: apparent_file_bytes={} reserved_data_bytes={}",
            snapshot_runtime_metadata.apparent_file_bytes, reserved_data_bytes
        ))
        .into());
    }

    clear_runtime_pma_files(&data_dir)?;
    write_corrupt_checkpoint_pair(&data_dir)?;
    println!(
        "stage 2: restore from snapshot into configured small PMA and replay to SQLite max={}",
        sqlite_max_event_num(&data_dir)?
    );

    let mut second = boot_app(&jam, &data_dir, NockStackSize::Small).await?;
    assert_counter_state(&mut second, 2).await?;
    assert_eq!(sqlite_max_event_num(&data_dir)?, 2);
    assert_eq!(max_runtime_pma_meta_event(&data_dir)?, 2);

    let active_pma = active_runtime_pma(&data_dir)?;
    let active_metadata = Pma::read_file_metadata(&active_pma)?;
    println!(
        "restored active PMA: path={} data_words={} alloc_words={} free_words={}",
        active_pma.display(),
        active_metadata.data_words,
        active_metadata.alloc_words,
        active_metadata
            .data_words
            .saturating_sub(active_metadata.alloc_words)
    );
    if active_metadata.data_words < NOCK_STACK_SIZE_SMALL as u64 {
        return Err(std::io::Error::other(format!(
            "snapshot recovery replayed to SQLite max but did not restore into configured larger PMA: required_min_words={} actual_words={}",
            NOCK_STACK_SIZE_SMALL, active_metadata.data_words
        ))
        .into());
    }

    stop_app(second).await?;
    println!("snapshot restore expanded PMA and replayed to SQLite max");
    Ok(())
}

#[derive(Debug)]
struct ReadySnapshotForTest {
    pma_path: PathBuf,
    manifest_path: PathBuf,
    event_num: u64,
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

async fn boot_app(
    jam: &[u8],
    data_dir: &Path,
    stack_size: NockStackSize,
) -> Result<NockApp<NockJammer>, Box<dyn Error>> {
    let mut cli = default_boot_cli(false);
    cli.data_dir = Some(data_dir.to_path_buf());
    cli.pma_initial_size = Some(PmaSize::from_words(stack_size.stack_words()));
    cli.stack_size = stack_size;
    cli.gc_interval = None;
    cli.rotating_snapshot_interval_event_time = None;
    cli.disable_fsync = true;
    match setup_::<NockJammer>(
        jam,
        cli,
        &[],
        "pma-snapshot-restore-expand-regression",
        None,
    )
    .await?
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

fn sqlite_connection(data_dir: &Path) -> Result<SqliteConnection, Box<dyn Error>> {
    let path = data_dir.join("event-log.sqlite3");
    Ok(SqliteConnection::establish(path.to_str().ok_or_else(
        || std::io::Error::other(format!("non-utf8 sqlite path: {path:?}")),
    )?)?)
}

fn sqlite_max_event_num(data_dir: &Path) -> Result<u64, Box<dyn Error>> {
    let mut conn = sqlite_connection(data_dir)?;
    let row = sql_query("SELECT COALESCE(MAX(event_num), 0) AS value FROM events")
        .get_result::<I64ValueRow>(&mut conn)?;
    Ok(u64::try_from(row.value)?)
}

fn newest_ready_snapshot(data_dir: &Path) -> Result<ReadySnapshotForTest, Box<dyn Error>> {
    let mut conn = sqlite_connection(data_dir)?;
    let row = sql_query(
        "SELECT pma_path, manifest_path, event_num FROM snapshots WHERE state = 'ready' ORDER BY event_num DESC, snapshot_id DESC LIMIT 1",
    )
    .get_result::<SnapshotRow>(&mut conn)?;
    let pma_path = PathBuf::from(row.pma_path);
    let manifest_path = PathBuf::from(row.manifest_path);
    if !pma_path.exists() || !manifest_path.exists() {
        return Err(std::io::Error::other(format!(
            "ready snapshot artifacts missing: pma_exists={} manifest_exists={} pma={} manifest={}",
            pma_path.exists(),
            manifest_path.exists(),
            pma_path.display(),
            manifest_path.display()
        ))
        .into());
    }
    Ok(ReadySnapshotForTest {
        pma_path,
        manifest_path,
        event_num: u64::try_from(row.event_num)?,
    })
}

fn max_runtime_pma_meta_event(data_dir: &Path) -> Result<u64, Box<dyn Error>> {
    let pma_dir = data_dir.join("pma");
    let mut max_event = None;
    for idx in [0, 1] {
        let meta_path = pma_dir.join(format!("{idx}.meta"));
        if !meta_path.exists() {
            continue;
        }
        let meta = PmaPersistMetadataForTest::load(&meta_path)?;
        max_event = Some(max_event.map_or(meta.event_num, |event: u64| event.max(meta.event_num)));
    }
    max_event.ok_or_else(|| {
        std::io::Error::other(format!(
            "no runtime PMA metadata found in {}",
            pma_dir.display()
        ))
        .into()
    })
}

fn active_runtime_pma(data_dir: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let pma_dir = data_dir.join("pma");
    let mut candidates = Vec::new();
    for idx in [0, 1] {
        let pma_path = pma_dir.join(format!("{idx}.pma"));
        let meta_path = pma_dir.join(format!("{idx}.meta"));
        if pma_path.exists() && meta_path.exists() {
            let meta = PmaPersistMetadataForTest::load(&meta_path)?;
            let modified = fs::metadata(&meta_path)
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            candidates.push((pma_path, meta.event_num, modified));
        }
    }
    candidates.sort_by_key(|(_, event_num, modified)| (*event_num, *modified));
    candidates.pop().map(|(path, _, _)| path).ok_or_else(|| {
        std::io::Error::other(format!(
            "no meta-paired runtime PMA found in {}",
            pma_dir.display()
        ))
        .into()
    })
}

fn clear_runtime_pma_files(data_dir: &Path) -> Result<(), Box<dyn Error>> {
    let pma_dir = data_dir.join("pma");
    for file_name in ["0.pma", "1.pma", "0.meta", "1.meta"] {
        let path = pma_dir.join(file_name);
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn write_corrupt_checkpoint_pair(data_dir: &Path) -> Result<(), Box<dyn Error>> {
    let checkpoints_dir = data_dir.join("checkpoints");
    fs::create_dir_all(&checkpoints_dir)?;
    for file_name in ["0.chkjam", "1.chkjam"] {
        fs::write(
            checkpoints_dir.join(file_name),
            b"corrupt checkpoint fixture",
        )?;
    }
    Ok(())
}
