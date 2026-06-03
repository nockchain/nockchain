use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::BigInt;
use diesel::sqlite::SqliteConnection;
use nockapp::kernel::boot::{default_boot_cli, setup_, NockStackSize, SetupResult};
use nockapp::nockapp::wire::{SystemWire, Wire};
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::NockApp;
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

pub(crate) fn run_regression() -> Result<(), Box<dyn Error>> {
    nockvm::check_endian();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run())
}

async fn run() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let jam = load_test_jam()?;

    pma_ahead_subcase(&jam, &temp.path().join("pma-ahead")).await?;
    pma_behind_subcase(&jam, &temp.path().join("pma-behind")).await?;
    meta_stale_subcase(&jam, &temp.path().join("meta-stale")).await?;

    println!("PMA/SQLite boundary recovery matrix passed");
    Ok(())
}

async fn pma_ahead_subcase(jam: &[u8], data_dir: &Path) -> Result<(), Box<dyn Error>> {
    println!("subcase pma-ahead: create event-2 PMA, then delete SQLite event 2");
    let mut first = boot_app(jam, data_dir).await?;
    poke_inc(&mut first).await?;
    poke_inc(&mut first).await?;
    assert_counter_state(&mut first, 2).await?;
    stop_app(first).await?;
    assert_eq!(sqlite_max_event_num(data_dir)?, 2);
    assert_eq!(max_runtime_pma_meta_event(data_dir)?, 2);

    delete_sqlite_event(data_dir, 2)?;
    assert_eq!(sqlite_max_event_num(data_dir)?, 1);
    assert_eq!(max_runtime_pma_meta_event(data_dir)?, 2);
    print_active_pma_state("pma-ahead before recovery", data_dir)?;

    let mut recovered = boot_app(jam, data_dir).await?;
    assert_counter_state(&mut recovered, 1).await?;
    assert_eq!(sqlite_max_event_num(data_dir)?, 1);
    assert_eq!(max_runtime_pma_meta_event(data_dir)?, 1);
    stop_app(recovered).await?;
    println!("subcase pma-ahead passed: boot recovered to SQLite boundary event 1");
    Ok(())
}

async fn pma_behind_subcase(jam: &[u8], data_dir: &Path) -> Result<(), Box<dyn Error>> {
    println!("subcase pma-behind: restore event-1 runtime PMA while SQLite remains at event 2");
    let backup_dir = data_dir.with_extension("event-1-pma-backup");

    let mut first = boot_app(jam, data_dir).await?;
    poke_inc(&mut first).await?;
    assert_counter_state(&mut first, 1).await?;
    stop_app(first).await?;
    assert_eq!(sqlite_max_event_num(data_dir)?, 1);
    assert_eq!(max_runtime_pma_meta_event(data_dir)?, 1);
    copy_runtime_pma_files(&data_dir.join("pma"), &backup_dir)?;

    let mut second = boot_app(jam, data_dir).await?;
    assert_counter_state(&mut second, 1).await?;
    poke_inc(&mut second).await?;
    assert_counter_state(&mut second, 2).await?;
    stop_app(second).await?;
    assert_eq!(sqlite_max_event_num(data_dir)?, 2);
    assert_eq!(max_runtime_pma_meta_event(data_dir)?, 2);

    restore_runtime_pma_files(data_dir, &backup_dir)?;
    assert_eq!(sqlite_max_event_num(data_dir)?, 2);
    assert_eq!(max_runtime_pma_meta_event(data_dir)?, 1);
    print_active_pma_state("pma-behind before recovery", data_dir)?;

    let mut recovered = boot_app(jam, data_dir).await?;
    assert_counter_state(&mut recovered, 2).await?;
    assert_eq!(sqlite_max_event_num(data_dir)?, 2);
    assert_eq!(max_runtime_pma_meta_event(data_dir)?, 2);
    stop_app(recovered).await?;
    println!("subcase pma-behind passed: boot replayed to SQLite boundary event 2");
    Ok(())
}

async fn meta_stale_subcase(jam: &[u8], data_dir: &Path) -> Result<(), Box<dyn Error>> {
    println!(
        "subcase meta-stale: keep event-2 PMA file but replace active .meta with event-1 metadata"
    );
    let stale_meta_path = data_dir.with_extension("event-1-active.meta");

    let mut first = boot_app(jam, data_dir).await?;
    poke_inc(&mut first).await?;
    assert_counter_state(&mut first, 1).await?;
    stop_app(first).await?;
    let event_1_pma = active_runtime_pma(data_dir)?;
    let event_1_meta = event_1_pma.with_extension("meta");
    let event_1_metadata = Pma::read_file_metadata(&event_1_pma)?;
    fs::copy(&event_1_meta, &stale_meta_path)?;
    assert_eq!(sqlite_max_event_num(data_dir)?, 1);
    assert_eq!(max_runtime_pma_meta_event(data_dir)?, 1);

    let mut second = boot_app(jam, data_dir).await?;
    assert_counter_state(&mut second, 1).await?;
    poke_inc(&mut second).await?;
    assert_counter_state(&mut second, 2).await?;
    stop_app(second).await?;
    assert_eq!(sqlite_max_event_num(data_dir)?, 2);
    assert_eq!(max_runtime_pma_meta_event(data_dir)?, 2);

    let event_2_pma = active_runtime_pma(data_dir)?;
    let event_2_metadata = Pma::read_file_metadata(&event_2_pma)?;
    if event_2_metadata.alloc_words <= event_1_metadata.alloc_words {
        return Err(std::io::Error::other(format!(
            "meta-stale fixture did not advance the PMA trailer: event1_alloc={} event2_alloc={}",
            event_1_metadata.alloc_words, event_2_metadata.alloc_words
        ))
        .into());
    }
    fs::copy(&stale_meta_path, event_2_pma.with_extension("meta"))?;
    assert_eq!(sqlite_max_event_num(data_dir)?, 2);
    assert_eq!(max_runtime_pma_meta_event(data_dir)?, 1);
    print_active_pma_state("meta-stale before recovery", data_dir)?;

    let mut recovered = boot_app(jam, data_dir).await?;
    assert_counter_state(&mut recovered, 2).await?;
    assert_eq!(sqlite_max_event_num(data_dir)?, 2);
    assert_eq!(max_runtime_pma_meta_event(data_dir)?, 2);
    stop_app(recovered).await?;
    println!("subcase meta-stale passed: boot ignored stale .meta and replayed to SQLite boundary event 2");
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

async fn boot_app(jam: &[u8], data_dir: &Path) -> Result<NockApp<NockJammer>, Box<dyn Error>> {
    let mut cli = default_boot_cli(false);
    cli.data_dir = Some(data_dir.to_path_buf());
    cli.stack_size = NockStackSize::Tiny;
    cli.gc_interval = None;
    cli.rotating_snapshot_interval_event_time = None;
    cli.disable_fsync = true;
    match setup_::<NockJammer>(
        jam,
        cli,
        &[],
        "pma-sqlite-boundary-recovery-regression",
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

fn delete_sqlite_event(data_dir: &Path, event_num: u64) -> Result<(), Box<dyn Error>> {
    let mut conn = sqlite_connection(data_dir)?;
    sql_query("DELETE FROM events WHERE event_num = ?")
        .bind::<BigInt, _>(i64::try_from(event_num)?)
        .execute(&mut conn)?;
    Ok(())
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

fn copy_runtime_pma_files(from_dir: &Path, to_dir: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(to_dir)?;
    for file_name in ["0.pma", "1.pma", "0.meta", "1.meta"] {
        let from = from_dir.join(file_name);
        if from.exists() {
            fs::copy(&from, to_dir.join(file_name))?;
        }
    }
    Ok(())
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

fn restore_runtime_pma_files(data_dir: &Path, backup_dir: &Path) -> Result<(), Box<dyn Error>> {
    clear_runtime_pma_files(data_dir)?;
    let pma_dir = data_dir.join("pma");
    fs::create_dir_all(&pma_dir)?;
    for file_name in ["0.pma", "1.pma", "0.meta", "1.meta"] {
        let backup = backup_dir.join(file_name);
        if backup.exists() {
            fs::copy(&backup, pma_dir.join(file_name))?;
        }
    }
    Ok(())
}

fn print_active_pma_state(label: &str, data_dir: &Path) -> Result<(), Box<dyn Error>> {
    let active = active_runtime_pma(data_dir)?;
    let meta = PmaPersistMetadataForTest::load(&active.with_extension("meta"))?;
    let metadata = Pma::read_file_metadata(&active)?;
    println!(
        "{label}: sqlite_max={} active_pma={} meta_event={} data_words={} alloc_words={} free_words={}",
        sqlite_max_event_num(data_dir)?,
        active.display(),
        meta.event_num,
        metadata.data_words,
        metadata.alloc_words,
        metadata.data_words.saturating_sub(metadata.alloc_words)
    );
    Ok(())
}
