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
use nockapp::save::SaveableCheckpoint;
use nockapp::NockApp;
use nockvm::noun::{NounAllocator, NounSpace, D, T};
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

    sqlite_present_checkpoint_replays_to_newer_history(
        &jam,
        &temp.path().join("sqlite-present-replay"),
    )
    .await?;
    missing_sqlite_requires_explicit_checkpoint_bootstrap(
        &jam,
        &temp.path().join("missing-sqlite-normal"),
        &temp.path().join("explicit-bootstrap"),
    )
    .await?;

    println!("stale checkpoint refusal regression passed");
    Ok(())
}

async fn sqlite_present_checkpoint_replays_to_newer_history(
    jam: &[u8],
    data_dir: &Path,
) -> Result<(), Box<dyn Error>> {
    println!("subcase sqlite-present: checkpoint event 1 must replay to SQLite max event 2");
    let mut app = boot_app(jam, data_dir, false, None).await?;
    poke_inc(&mut app).await?;
    assert_counter_state(&mut app, 1).await?;
    let checkpoint_path = write_checkpoint_from_export(&mut app, data_dir).await?;
    println!(
        "wrote valid event-1 checkpoint at {}",
        checkpoint_path.display()
    );
    poke_inc(&mut app).await?;
    assert_counter_state(&mut app, 2).await?;
    stop_app(app).await?;

    assert_eq!(sqlite_max_event_num(data_dir)?, 2);
    assert_eq!(max_runtime_pma_meta_event(data_dir)?, 2);
    delete_snapshot_rows_and_artifacts(data_dir)?;
    clear_runtime_pma_files(data_dir)?;

    let mut recovered = boot_app(jam, data_dir, false, None).await?;
    assert_counter_state(&mut recovered, 2).await?;
    assert_eq!(sqlite_max_event_num(data_dir)?, 2);
    assert_eq!(max_runtime_pma_meta_event(data_dir)?, 2);
    stop_app(recovered).await?;
    println!("subcase sqlite-present passed: stale checkpoint was only used as a replay base");
    Ok(())
}

async fn missing_sqlite_requires_explicit_checkpoint_bootstrap(
    jam: &[u8],
    data_dir: &Path,
    explicit_bootstrap_dir: &Path,
) -> Result<(), Box<dyn Error>> {
    println!("subcase missing-sqlite: normal recovery must not silently use stale checkpoint");
    let mut app = boot_app(jam, data_dir, false, None).await?;
    poke_inc(&mut app).await?;
    assert_counter_state(&mut app, 1).await?;
    let checkpoint_path = write_checkpoint_from_export(&mut app, data_dir).await?;
    poke_inc(&mut app).await?;
    assert_counter_state(&mut app, 2).await?;
    stop_app(app).await?;

    assert_eq!(sqlite_max_event_num(data_dir)?, 2);
    assert_eq!(max_runtime_pma_meta_event(data_dir)?, 2);
    delete_snapshot_rows_and_artifacts(data_dir)?;
    clear_runtime_pma_files(data_dir)?;
    remove_event_log_files(data_dir)?;

    match boot_app(jam, data_dir, false, None).await {
        Ok(silently_recovered) => {
            let event_num = silently_recovered.export().await?.event_num;
            let _ = stop_app(silently_recovered).await;
            return Err(std::io::Error::other(format!(
                "normal recovery silently used a stale checkpoint after the SQLite event log was removed: booted_event={event_num}; expected fail-closed unless explicit bootstrap/discard intent is provided"
            ))
            .into());
        }
        Err(err) => {
            println!("normal recovery failed closed with missing SQLite event log: {err}");
        }
    }

    println!(
        "subcase explicit-bootstrap: the same checkpoint is allowed only in a fresh --new boot"
    );
    let mut explicit = boot_app(
        jam,
        explicit_bootstrap_dir,
        true,
        Some(checkpoint_path.clone()),
    )
    .await?;
    assert_counter_state(&mut explicit, 1).await?;
    stop_app(explicit).await?;
    println!("subcase missing-sqlite passed: stale checkpoint required explicit bootstrap intent");
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
    new: bool,
    bootstrap_from_chkjam: Option<PathBuf>,
) -> Result<NockApp<NockJammer>, Box<dyn Error>> {
    let mut cli = default_boot_cli(new);
    cli.data_dir = Some(data_dir.to_path_buf());
    cli.stack_size = NockStackSize::Tiny;
    cli.gc_interval = None;
    cli.rotating_snapshot_interval_event_time = None;
    cli.disable_fsync = true;
    cli.bootstrap_from_chkjam =
        bootstrap_from_chkjam.map(|path| path.to_string_lossy().into_owned());
    match setup_::<NockJammer>(
        jam,
        cli,
        &[],
        "pma-stale-checkpoint-refusal-regression",
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

fn remove_event_log_files(data_dir: &Path) -> Result<(), Box<dyn Error>> {
    let event_log = data_dir.join("event-log.sqlite3");
    for path in [
        event_log.clone(),
        PathBuf::from(format!("{}-wal", event_log.display())),
        PathBuf::from(format!("{}-shm", event_log.display())),
    ] {
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}
