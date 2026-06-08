use std::error::Error;
use std::fs;
use std::path::Path;

use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::BigInt;
use diesel::sqlite::SqliteConnection;
use nockapp::kernel::boot::{default_boot_cli, setup_, NockStackSize, SetupResult};
use nockapp::nockapp::wire::{SystemWire, Wire};
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::NockApp;
use nockvm::noun::{NounAllocator, NounSpace, D, T};
use nockvm_macros::tas;
use tempfile::TempDir;

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

    continuous_event_log_replays_from_fresh_kernel(&jam, &temp.path().join("continuous-log"))
        .await?;
    gapped_event_log_fails_closed(&jam, &temp.path().join("gapped-log")).await?;

    println!("fresh event-log replay boundary regression passed");
    Ok(())
}

async fn continuous_event_log_replays_from_fresh_kernel(
    jam: &[u8],
    data_dir: &Path,
) -> Result<(), Box<dyn Error>> {
    println!("subcase continuous-log: remove PMA/snapshot/checkpoint bases and replay events 1..2 from fresh kernel");
    let mut first = boot_app(jam, data_dir).await?;
    poke_inc(&mut first).await?;
    poke_inc(&mut first).await?;
    assert_counter_state(&mut first, 2).await?;
    stop_app(first).await?;
    assert_eq!(sqlite_max_event_num(data_dir)?, 2);

    remove_all_non_event_log_boot_bases(data_dir)?;
    let mut recovered = boot_app(jam, data_dir).await?;
    assert_counter_state(&mut recovered, 2).await?;
    assert_eq!(sqlite_max_event_num(data_dir)?, 2);
    stop_app(recovered).await?;
    println!("subcase continuous-log passed: fresh replay reached SQLite max event 2");
    Ok(())
}

async fn gapped_event_log_fails_closed(jam: &[u8], data_dir: &Path) -> Result<(), Box<dyn Error>> {
    println!("subcase gapped-log: remove event 1 and require continuity failure instead of skipped replay");
    let mut first = boot_app(jam, data_dir).await?;
    poke_inc(&mut first).await?;
    poke_inc(&mut first).await?;
    assert_counter_state(&mut first, 2).await?;
    stop_app(first).await?;
    assert_eq!(sqlite_max_event_num(data_dir)?, 2);

    remove_all_non_event_log_boot_bases(data_dir)?;
    delete_sqlite_event(data_dir, 1)?;
    assert_eq!(sqlite_max_event_num(data_dir)?, 2);

    match boot_app(jam, data_dir).await {
        Ok(app) => {
            let event_num = app.export().await?.event_num;
            let _ = stop_app(app).await;
            Err(std::io::Error::other(format!(
                "fresh replay unexpectedly booted from a gapped SQLite log: booted_event={event_num}"
            ))
            .into())
        }
        Err(err) => {
            let text = err.to_string();
            if !text.contains("event log continuity")
                && !text.contains("EventSequenceGap")
                && !text.contains("event sequence gap")
            {
                return Err(std::io::Error::other(format!(
                    "gapped event log failed for the wrong reason: {text}"
                ))
                .into());
            }
            println!("subcase gapped-log passed: boot failed closed with continuity error: {text}");
            Ok(())
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
    cli.stack_size = NockStackSize::Tiny;
    cli.gc_interval = None;
    cli.rotating_snapshot_interval_event_time = None;
    cli.disable_fsync = true;
    match setup_::<NockJammer>(jam, cli, &[], "pma-fresh-replay-boundary-regression", None).await? {
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

fn remove_all_non_event_log_boot_bases(data_dir: &Path) -> Result<(), Box<dyn Error>> {
    clear_runtime_pma_files(data_dir)?;
    delete_snapshot_rows_and_artifacts(data_dir)?;
    remove_checkpoint_files(data_dir)?;
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

fn remove_checkpoint_files(data_dir: &Path) -> Result<(), Box<dyn Error>> {
    let checkpoint_dir = data_dir.join("checkpoints");
    for file_name in ["0.chkjam", "1.chkjam"] {
        let path = checkpoint_dir.join(file_name);
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}
