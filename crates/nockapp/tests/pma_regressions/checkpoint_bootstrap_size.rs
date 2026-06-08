use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::BigInt;
use diesel::sqlite::SqliteConnection;
use nockapp::kernel::boot::{default_boot_cli, setup_, NockStackSize, PmaSize, SetupResult};
use nockapp::nockapp::wire::{SystemWire, Wire};
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::save::SaveableCheckpoint;
use nockapp::NockApp;
use nockvm::mem::NOCK_STACK_SIZE_SMALL;
use nockvm::noun::{NounAllocator, NounSpace, D, T};
use nockvm::pma::Pma;
use nockvm_macros::tas;
use tempfile::TempDir;

use crate::pma_regressions::pma_meta::PmaPersistMetadataForTest;

const PMA_INITIAL_OVERRIDE_ENV: &str = "NOCK_PMA_INITIAL_WORDS_FOR_REGRESSION";
const PMA_RESERVED_WORDS_ENV: &str = "NOCK_PMA_RESERVED_WORDS";
const PMA_GROWTH_EVENTS_ENV: &str = "NOCK_PMA_GROWTH_EVENTS_PATH";
const LARGE_BOOTSTRAP_INITIAL_WORDS: usize = 1024;
const LARGE_BOOTSTRAP_RESERVED_WORDS: usize = 1 << 22;
const LARGE_BOOTSTRAP_MIN_GROWTH_EVENTS: usize = 2;

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
    let data_dir = temp.path().join("pma-checkpoint-bootstrap-size-regression");
    let jam = load_test_jam()?;

    println!("stage 1: boot tiny PMA, checkpoint event 1, then commit event 2");
    let mut app = boot_app(&jam, &data_dir, NockStackSize::Tiny).await?;
    poke_inc(&mut app).await?;
    assert_counter_state(&mut app, 1).await?;
    let checkpoint_path = write_checkpoint_from_export(&mut app, &data_dir).await?;
    println!(
        "wrote valid event-1 checkpoint at {}",
        checkpoint_path.display()
    );
    poke_inc(&mut app).await?;
    assert_counter_state(&mut app, 2).await?;
    stop_app(app).await?;

    assert_eq!(sqlite_max_event_num(&data_dir)?, 2);
    assert_eq!(max_runtime_pma_meta_event(&data_dir)?, 2);
    delete_snapshot_rows_and_artifacts(&data_dir)?;
    clear_runtime_pma_files(&data_dir)?;
    println!("stage 2: boot from checkpoint base with small PMA config and replay SQLite tail");

    let mut recovered = boot_app(&jam, &data_dir, NockStackSize::Small).await?;
    assert_counter_state(&mut recovered, 2).await?;
    assert_eq!(sqlite_max_event_num(&data_dir)?, 2);
    assert_eq!(max_runtime_pma_meta_event(&data_dir)?, 2);

    let active_pma = active_runtime_pma(&data_dir)?;
    let active_metadata = Pma::read_file_metadata(&active_pma)?;
    println!(
        "checkpoint-restored active PMA: path={} data_words={} alloc_words={} free_words={}",
        active_pma.display(),
        active_metadata.data_words,
        active_metadata.alloc_words,
        active_metadata
            .data_words
            .saturating_sub(active_metadata.alloc_words)
    );
    if active_metadata.data_words < NOCK_STACK_SIZE_SMALL as u64 {
        return Err(std::io::Error::other(format!(
            "checkpoint recovery replayed to SQLite max but did not create the configured larger PMA: required_min_words={} actual_words={}",
            NOCK_STACK_SIZE_SMALL, active_metadata.data_words
        ))
        .into());
    }

    stop_app(recovered).await?;
    println!("checkpoint recovery created configured larger PMA and replayed to SQLite max");

    run_large_checkpoint_bootstrap_growth_subcase(&jam).await?;
    Ok(())
}

async fn run_large_checkpoint_bootstrap_growth_subcase(jam: &[u8]) -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let data_dir = temp
        .path()
        .join("pma-checkpoint-large-bootstrap-growth-regression");
    println!("stage 3: checkpoint bootstrap from tiny initial PMA with large virtual reservation");

    let mut app = boot_app(jam, &data_dir, NockStackSize::Tiny).await?;
    poke_inc(&mut app).await?;
    assert_counter_state(&mut app, 1).await?;
    let checkpoint_path = write_checkpoint_from_export(&mut app, &data_dir).await?;
    poke_inc(&mut app).await?;
    assert_counter_state(&mut app, 2).await?;
    stop_app(app).await?;

    assert_eq!(sqlite_max_event_num(&data_dir)?, 2);
    delete_snapshot_rows_and_artifacts(&data_dir)?;
    clear_runtime_pma_files(&data_dir)?;

    let growth_events_path = data_dir.join("large-bootstrap-growth-events.log");
    let _initial_guard = EnvVarGuard::set(
        PMA_INITIAL_OVERRIDE_ENV,
        LARGE_BOOTSTRAP_INITIAL_WORDS.to_string(),
    );
    let _reserved_guard = EnvVarGuard::set(
        PMA_RESERVED_WORDS_ENV,
        LARGE_BOOTSTRAP_RESERVED_WORDS.to_string(),
    );
    let _events_guard = EnvVarGuard::set(
        PMA_GROWTH_EVENTS_ENV,
        growth_events_path.as_os_str().to_os_string(),
    );

    let mut recovered = boot_app(jam, &data_dir, NockStackSize::Tiny).await?;
    assert_counter_state(&mut recovered, 2).await?;
    assert_eq!(sqlite_max_event_num(&data_dir)?, 2);
    assert_eq!(max_runtime_pma_meta_event(&data_dir)?, 2);

    let active_pma = active_runtime_pma(&data_dir)?;
    let active_meta = PmaPersistMetadataForTest::load(&active_pma.with_extension("meta"))?;
    let metadata = Pma::read_file_metadata(&active_pma)?;
    if active_meta.pma_reserved_words != Some(metadata.reserved_words) {
        return Err(std::io::Error::other(format!(
            "PMA sidecar reservation did not match trailer: sidecar={:?} trailer={}",
            active_meta.pma_reserved_words, metadata.reserved_words
        ))
        .into());
    }
    let growth_events = read_growth_events(&growth_events_path)?;
    println!(
        "large checkpoint bootstrap active PMA: checkpoint={} path={} capacity_words={} alloc_words={} free_words={} reserved_words={} growth_events={}",
        checkpoint_path.display(),
        active_pma.display(),
        metadata.capacity_words,
        metadata.alloc_words,
        metadata.free_words,
        metadata.reserved_words,
        growth_events.len()
    );
    for event in &growth_events {
        println!("large checkpoint bootstrap growth event: {event}");
    }

    let ten_x_initial = u64::try_from(LARGE_BOOTSTRAP_INITIAL_WORDS * 10)?;
    if metadata.alloc_words <= ten_x_initial {
        return Err(std::io::Error::other(format!(
            "checkpoint bootstrap PMA allocation was not more than 10x the initial capacity: initial_words={} alloc_words={}",
            LARGE_BOOTSTRAP_INITIAL_WORDS, metadata.alloc_words
        ))
        .into());
    }
    if growth_events.len() < LARGE_BOOTSTRAP_MIN_GROWTH_EVENTS {
        return Err(std::io::Error::other(format!(
            "expected at least {LARGE_BOOTSTRAP_MIN_GROWTH_EVENTS} PMA growth events during large checkpoint bootstrap, got {}",
            growth_events.len()
        ))
        .into());
    }
    let reserved_bytes = u64::try_from(LARGE_BOOTSTRAP_RESERVED_WORDS)?
        .checked_mul(8)
        .ok_or_else(|| std::io::Error::other("reserved byte calculation overflowed"))?;
    if metadata.apparent_file_bytes >= reserved_bytes {
        return Err(std::io::Error::other(format!(
            "large checkpoint bootstrap materialized the reserved maximum: apparent_file_bytes={} reserved_bytes={reserved_bytes}",
            metadata.apparent_file_bytes
        ))
        .into());
    }

    stop_app(recovered).await?;
    println!(
        "checkpoint bootstrap grew a PMA payload more than 10x its initial capacity without retrying copy_to_pma"
    );
    Ok(())
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
        "pma-checkpoint-bootstrap-size-regression",
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

fn empty_cold_slab() -> NounSlab {
    let mut slab = NounSlab::new();
    let root = T(&mut slab, &[D(0), D(0), D(0)]);
    slab.set_root(root);
    slab
}

async fn write_checkpoint_from_export(
    app: &mut NockApp<NockJammer>,
    data_dir: &Path,
) -> Result<PathBuf, Box<dyn Error>> {
    let exported = app.export().await?;
    let checkpoint = SaveableCheckpoint {
        ker_hash: exported.ker_hash,
        event_num: exported.event_num,
        state: exported.kernel_state,
        cold: empty_cold_slab(),
    };
    let jammed = checkpoint.to_jammed_checkpoint::<NockJammer>();
    let bytes = jammed.encode()?;
    let checkpoint_dir = data_dir.join("checkpoints");
    fs::create_dir_all(&checkpoint_dir)?;
    let checkpoint_path = checkpoint_dir.join("0.chkjam");
    fs::write(&checkpoint_path, bytes)?;
    Ok(checkpoint_path)
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

fn read_growth_events(path: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    Ok(fs::read_to_string(path)?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl Into<OsString>) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value.into());
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            std::env::set_var(self.key, previous);
        } else {
            std::env::remove_var(self.key);
        }
    }
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

fn delete_snapshot_rows_and_artifacts(data_dir: &Path) -> Result<(), Box<dyn Error>> {
    let pma_dir = data_dir.join("pma");
    let mut conn = sqlite_connection(data_dir)?;
    sql_query("DELETE FROM snapshots").execute(&mut conn)?;
    sql_query("DELETE FROM meta WHERE key = 'active_snapshot_id'").execute(&mut conn)?;
    drop(conn);

    if !pma_dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&pma_dir)? {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name == "epoch.pma"
            || name == "epoch.manifest"
            || name == "epoch.pma.tmp"
            || name == "epoch.manifest.tmp"
            || name.starts_with("snap-")
        {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}
