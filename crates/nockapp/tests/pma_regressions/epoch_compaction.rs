use std::error::Error;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::{BigInt, Text};
use diesel::sqlite::SqliteConnection;
use nockapp::kernel::boot::{default_boot_cli, setup_, NockStackSize, PmaSize, SetupResult};
use nockapp::nockapp::wire::{SystemWire, Wire};
use nockapp::noun::slab::{slab_equality, NockJammer, NounSlab};
use nockapp::NockApp;
use nockvm::noun::{NounSpace, D};
use nockvm_macros::tas;
use tempfile::TempDir;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

#[derive(QueryableByName)]
struct IntegerRow {
    #[diesel(sql_type = BigInt)]
    value: i64,
}

#[derive(Debug, QueryableByName)]
struct SnapshotRow {
    #[diesel(sql_type = BigInt)]
    snapshot_id: i64,
    #[diesel(sql_type = Text)]
    kind: String,
    #[diesel(sql_type = Text)]
    pma_path: String,
    #[diesel(sql_type = Text)]
    manifest_path: String,
    #[diesel(sql_type = BigInt)]
    event_num: i64,
}

struct SnapshotBackup {
    manifest: Vec<u8>,
    pma_prefix: [u8; 8],
}

pub(crate) fn run_regression() -> TestResult {
    nockvm::check_endian();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> TestResult {
    let temp = TempDir::new()?;
    let data_dir = temp.path().join("epoch-compaction");
    let jam = load_test_jam()?;
    let mut app = boot_app(&jam, &data_dir).await?;
    let checkpoint = app.checkpoint().await?;
    let expired_checkpoint = checkpoint.to_jammed_checkpoint::<NockJammer>().encode()?;
    poke_inc(&mut app).await?;
    stop_app(app).await?;

    println!("stage 1: two durable compute-driven epoch compactions across restarts");
    for first_odd in [1, 7] {
        if first_odd > 1 {
            let mut app = boot_app(&jam, &data_dir).await?;
            poke_inc(&mut app).await?;
            stop_app(app).await?;
        }
        for odd_event in [first_odd, first_odd + 2, first_odd + 4] {
            // Only modify timing while the node is stopped. Event jobs and
            // state remain genuine accepted pokes through the public API.
            set_event_compute_seconds(&data_dir, odd_event, 1)?;
            let mut app = boot_app(&jam, &data_dir).await?;
            poke_inc(&mut app).await?;
            assert_eq!(app.export().await?.event_num, odd_event + 1);
            if odd_event < first_odd + 4 {
                poke_inc(&mut app).await?;
            }
            stop_app(app).await?;
        }
        assert_compacted_history(&data_dir, first_odd + 3, first_odd + 5, first_odd + 5)?;
    }

    // Leave a real event after the newest rotation, so all three anchors must
    // replay at least one retained event to reach the identical accepted state.
    let mut app = boot_app(&jam, &data_dir).await?;
    poke_inc(&mut app).await?;
    let expected = app.export().await?;
    assert_eq!(expected.event_num, 13);
    stop_app(app).await?;
    assert_compacted_history(&data_dir, 10, 12, 13)?;
    let snapshots = ready_snapshots(&data_dir)?;
    let original_artifacts: Vec<_> = snapshots
        .iter()
        .map(|snapshot| {
            let mut pma_prefix = [0; 8];
            fs::File::open(&snapshot.pma_path)?.read_exact(&mut pma_prefix)?;
            Ok::<_, std::io::Error>(SnapshotBackup {
                manifest: fs::read(&snapshot.manifest_path)?,
                pma_prefix,
            })
        })
        .collect::<Result<_, _>>()?;
    fs::write(data_dir.join("checkpoints/0.chkjam"), expired_checkpoint)?;

    println!("stage 2: recover independently from the epoch and each retained rotation");
    for chosen in &snapshots {
        restore_snapshot_registry(&data_dir, &snapshots, &original_artifacts)?;
        for other in &snapshots {
            if other.snapshot_id != chosen.snapshot_id {
                corrupt_snapshot(other)?;
            }
        }
        select_active_snapshot(&data_dir, chosen.snapshot_id)?;
        clear_runtime_pma(&data_dir)?;
        let recovered = boot_app(&jam, &data_dir).await?;
        let actual = recovered.export().await?;
        assert_eq!(
            actual.event_num, expected.event_num,
            "wrong head using {chosen:?}"
        );
        assert_eq!(actual.ker_hash, expected.ker_hash);
        assert!(
            slab_equality(&actual.kernel_state, &expected.kernel_state),
            "recovered noun differs using {chosen:?}"
        );
        assert_eq!(
            meta_value(&data_dir, "active_snapshot_id")?,
            chosen.snapshot_id
        );
        stop_app(recovered).await?;
        println!(
            "recovered head 13 using {} at event {}",
            chosen.kind, chosen.event_num
        );
    }

    println!("stage 3: expired checkpoint and fresh state cannot replace a compacted base");
    restore_snapshot_registry(&data_dir, &snapshots, &original_artifacts)?;
    for snapshot in &snapshots {
        corrupt_snapshot(snapshot)?;
    }
    clear_runtime_pma(&data_dir)?;
    assert_boot_refused(
        &jam, &data_dir, "no valid boot base at or after compacted event log replay floor 10",
    )
    .await?;
    for path in fs::read_dir(data_dir.join("checkpoints"))? {
        fs::remove_file(path?.path())?;
    }
    // Restore the registry before the next failure injection so boot cleanup
    // does not quarantine the failed artifacts needed by the suffix-gap case.
    restore_snapshot_registry(&data_dir, &snapshots, &original_artifacts)?;
    for snapshot in &snapshots {
        corrupt_snapshot(snapshot)?;
    }
    assert_boot_refused(
        &jam, &data_dir, "no valid boot base at or after compacted event log replay floor 10",
    )
    .await?;

    println!("stage 4: a genuine gap in the retained suffix still fails closed");
    restore_snapshot_registry(&data_dir, &snapshots, &original_artifacts)?;
    let epoch = snapshots
        .iter()
        .find(|snapshot| snapshot.kind == "epoch")
        .unwrap();
    select_active_snapshot(&data_dir, epoch.snapshot_id)?;
    sql_query("DELETE FROM events WHERE event_num = 11")
        .execute(&mut sqlite_connection(&data_dir)?)?;
    assert_boot_refused(&jam, &data_dir, "event log continuity check failed").await?;
    println!(
        "two-cycle epoch compaction and independent recovery matrix passed with fsync enabled"
    );
    Ok(())
}

async fn boot_app(jam: &[u8], data_dir: &Path) -> TestResult<NockApp<NockJammer>> {
    let mut cli = default_boot_cli(false);
    cli.data_dir = Some(data_dir.to_path_buf());
    cli.stack_size = NockStackSize::Tiny;
    cli.pma_initial_size = Some(PmaSize::from_words(NockStackSize::Tiny.stack_words()));
    cli.gc_interval = None;
    cli.rotating_snapshot_interval_event_time = Some(1);
    cli.epoch_compaction_interval_event_time = Some(3);
    cli.disable_fsync = false;
    match setup_::<NockJammer>(jam, cli, &[], "pma-epoch-compaction-regression", None).await? {
        SetupResult::App(app) => Ok(app),
        SetupResult::ExportedState => Err(std::io::Error::other("unexpected state export").into()),
    }
}

async fn poke_inc(app: &mut NockApp<NockJammer>) -> TestResult {
    let mut cause = NounSlab::new();
    cause.copy_into(D(tas!(b"inc")), &NounSpace::empty());
    app.poke(SystemWire.to_wire(), cause).await?;
    Ok(())
}

async fn stop_app(mut app: NockApp<NockJammer>) -> TestResult {
    app.get_handle().exit.exit(0).await?;
    app.run().await?;
    Ok(())
}

async fn assert_boot_refused(jam: &[u8], data_dir: &Path, expected_error: &str) -> TestResult {
    match boot_app(jam, data_dir).await {
        Ok(app) => {
            let state = app.export().await?;
            stop_app(app).await?;
            Err(std::io::Error::other(format!(
                "boot unexpectedly recovered event {} instead of refusing: {expected_error}",
                state.event_num
            ))
            .into())
        }
        Err(error) => {
            assert!(
                error.to_string().contains(expected_error),
                "unexpected error: {error}"
            );
            Ok(())
        }
    }
}

fn sqlite_connection(data_dir: &Path) -> TestResult<SqliteConnection> {
    let path = data_dir.join("event-log.sqlite3");
    Ok(SqliteConnection::establish(path.to_str().ok_or_else(
        || std::io::Error::other(format!("non-UTF8 event log path: {path:?}")),
    )?)?)
}

fn meta_value(data_dir: &Path, key: &str) -> TestResult<i64> {
    let row = sql_query(
        "SELECT COALESCE((SELECT CAST(value AS INTEGER) FROM meta WHERE key = ?), 0) AS value",
    )
    .bind::<Text, _>(key)
    .get_result::<IntegerRow>(&mut sqlite_connection(data_dir)?)?;
    Ok(row.value)
}

fn set_event_compute_seconds(data_dir: &Path, event_num: u64, seconds: i64) -> TestResult {
    assert_eq!(
        sql_query("UPDATE events SET event_processing_duration_us = ? WHERE event_num = ?")
            .bind::<BigInt, _>(seconds * 1_000_000)
            .bind::<BigInt, _>(i64::try_from(event_num)?)
            .execute(&mut sqlite_connection(data_dir)?)?,
        1
    );
    Ok(())
}

fn ready_snapshots(data_dir: &Path) -> TestResult<Vec<SnapshotRow>> {
    Ok(sql_query("SELECT snapshot_id, kind, pma_path, manifest_path, event_num FROM snapshots WHERE state = 'ready' ORDER BY kind, event_num")
        .load(&mut sqlite_connection(data_dir)?)?)
}

fn assert_compacted_history(
    data_dir: &Path,
    floor: u64,
    compaction_head: u64,
    head: u64,
) -> TestResult {
    assert_eq!(meta_value(data_dir, "replay_floor")?, floor as i64);
    assert_eq!(
        meta_value(data_dir, "compaction_event_num")?,
        compaction_head as i64
    );
    assert_eq!(meta_value(data_dir, "reclamation_pending")?, 0);
    let mut conn = sqlite_connection(data_dir)?;
    let events = sql_query("SELECT event_num AS value FROM events ORDER BY event_num")
        .load::<IntegerRow>(&mut conn)?;
    let actual_events: Vec<_> = events.into_iter().map(|row| row.value).collect();
    let expected_events: Vec<_> = ((floor + 1)..=head).map(|event| event as i64).collect();
    assert!(!actual_events.is_empty());
    assert_eq!(
        actual_events, expected_events,
        "retained suffix must be contiguous through accepted head"
    );
    let snapshots = ready_snapshots(data_dir)?;
    assert_eq!(
        snapshots.len(),
        3,
        "retain exactly one epoch and two rotations"
    );
    assert_eq!(
        snapshots
            .iter()
            .map(|snapshot| (snapshot.kind.as_str(), snapshot.event_num))
            .collect::<Vec<_>>(),
        vec![
            ("epoch", floor as i64),
            ("rotating", floor as i64),
            ("rotating", compaction_head as i64)
        ]
    );
    for snapshot in snapshots {
        assert!(Path::new(&snapshot.pma_path).is_file());
        assert!(Path::new(&snapshot.manifest_path).is_file());
    }
    let retired = sql_query("SELECT snapshot_id, kind, pma_path, manifest_path, event_num FROM snapshots WHERE state = 'retired'")
        .load::<SnapshotRow>(&mut conn)?;
    assert!(!retired.is_empty());
    for snapshot in retired {
        assert!(
            !Path::new(&snapshot.pma_path).exists(),
            "retired PMA not reclaimed: {snapshot:?}"
        );
        assert!(
            !Path::new(&snapshot.manifest_path).exists(),
            "retired manifest not reclaimed: {snapshot:?}"
        );
    }
    println!("compaction retained floor={floor}, compaction_head={compaction_head}, accepted_head={head}");
    Ok(())
}

fn select_active_snapshot(data_dir: &Path, snapshot_id: i64) -> TestResult {
    sql_query("INSERT INTO meta (key, value) VALUES ('active_snapshot_id', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value")
        .bind::<BigInt, _>(snapshot_id)
        .execute(&mut sqlite_connection(data_dir)?)?;
    Ok(())
}

fn restore_snapshot_registry(
    data_dir: &Path,
    snapshots: &[SnapshotRow],
    backups: &[SnapshotBackup],
) -> TestResult {
    let mut conn = sqlite_connection(data_dir)?;
    for (snapshot, backup) in snapshots.iter().zip(backups) {
        fs::write(&snapshot.manifest_path, &backup.manifest)?;
        fs::OpenOptions::new()
            .write(true)
            .open(&snapshot.pma_path)?
            .write_all(&backup.pma_prefix)?;
        sql_query("UPDATE snapshots SET state = 'ready' WHERE snapshot_id = ?")
            .bind::<BigInt, _>(snapshot.snapshot_id)
            .execute(&mut conn)?;
    }
    Ok(())
}

fn corrupt_snapshot(snapshot: &SnapshotRow) -> TestResult {
    fs::write(&snapshot.manifest_path, b"unusable recovery anchor")?;
    // Damage the PMA too: a supposedly independent epoch must remain usable
    // even if its source rotation's data is corrupted (including hard links).
    fs::OpenOptions::new()
        .write(true)
        .open(&snapshot.pma_path)?
        .write_all(&[0xa5; 8])?;
    Ok(())
}

fn clear_runtime_pma(data_dir: &Path) -> TestResult {
    for filename in ["0.pma", "1.pma", "0.meta", "1.meta"] {
        let path = data_dir.join("pma").join(filename);
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn load_test_jam() -> TestResult<Vec<u8>> {
    let mut candidates = Vec::new();
    if let Some(manifest_dir) = option_env!("CARGO_MANIFEST_DIR") {
        candidates.push(Path::new(manifest_dir).join("test-jams/test-ker.jam"));
    }
    candidates.push(PathBuf::from("open/crates/nockapp/test-jams/test-ker.jam"));
    candidates.push(PathBuf::from("test-jams/test-ker.jam"));
    candidates
        .iter()
        .find_map(|path| fs::read(path).ok())
        .ok_or_else(|| {
            std::io::Error::other(format!("test kernel not found at {candidates:?}")).into()
        })
}
