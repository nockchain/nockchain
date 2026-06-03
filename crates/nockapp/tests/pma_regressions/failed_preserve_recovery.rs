use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::BigInt;
use diesel::sqlite::SqliteConnection;
use nockapp::kernel::boot::{default_boot_cli, setup_, NockStackSize, PmaSize, SetupResult};
use nockapp::nockapp::wire::{SystemWire, Wire};
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::NockApp;
use nockvm::mem::NOCK_STACK_SIZE_SMALL;
use nockvm::noun::{NounAllocator, NounSpace, D, T};
use nockvm::offset::PmaOffsetWords;
use nockvm::pma::Pma;
use nockvm_macros::tas;
use tempfile::TempDir;

use crate::pma_regressions::pma_meta::PmaPersistMetadataForTest;

const CHILD_ENV: &str = "NOCK_PMA_FAILED_PRESERVE_CHILD";
const DATA_DIR_ENV: &str = "NOCK_PMA_FAILED_PRESERVE_DATA_DIR";
const DISABLE_RESIZE_ENV: &str = "NOCK_PMA_DISABLE_RESIZE_FOR_REGRESSION";
const CHILD_TEST_NAME: &str = "pma_failed_preserve_recovery_regression";
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
    if env::var_os(CHILD_ENV).is_some() {
        let data_dir = env::var_os(DATA_DIR_ENV)
            .map(PathBuf::from)
            .ok_or_else(|| std::io::Error::other(format!("{DATA_DIR_ENV} not set")))?;
        return runtime.block_on(child_no_resize_boot_attempt(&data_dir));
    }
    runtime.block_on(run_parent())
}

async fn run_parent() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let data_dir = temp.path().join("pma-failed-preserve-recovery-regression");
    let jam = load_test_jam()?;

    println!("stage 1: create durable event-1 active PMA fixture");
    let mut first = boot_app(&jam, &data_dir, NockStackSize::Tiny).await?;
    poke_inc(&mut first).await?;
    assert_counter_state(&mut first, 1).await?;
    stop_app(first).await?;
    assert_eq!(sqlite_max_event_num(&data_dir)?, 1);
    assert_eq!(max_runtime_pma_meta_event(&data_dir)?, 1);

    let active_pma = active_runtime_pma(&data_dir)?;
    let before_force = Pma::read_file_metadata(&active_pma)?;
    println!(
        "active PMA before low-free fixture: path={} data_words={} alloc_words={} free_words={}",
        active_pma.display(),
        before_force.data_words,
        before_force.alloc_words,
        before_force
            .data_words
            .saturating_sub(before_force.alloc_words)
    );

    let forced_free_words = before_force.alloc_words;
    force_pma_free_words(&active_pma, forced_free_words)?;
    let forced = Pma::read_file_metadata(&active_pma)?;
    println!(
        "active PMA forced for partial preserve failure: path={} data_words={} alloc_words={} free_words={}",
        active_pma.display(),
        forced.data_words,
        forced.alloc_words,
        forced.data_words.saturating_sub(forced.alloc_words)
    );

    println!("stage 2: run failing no-resize boot attempt in child process");
    let child_output = Command::new(env::current_exe()?)
        .arg("--exact")
        .arg(CHILD_TEST_NAME)
        .arg("--nocapture")
        .env(CHILD_ENV, "1")
        .env(DATA_DIR_ENV, &data_dir)
        .env(DISABLE_RESIZE_ENV, "1")
        .output()?;
    let child_stdout = String::from_utf8_lossy(&child_output.stdout);
    let child_stderr = String::from_utf8_lossy(&child_output.stderr);
    if !child_stdout.trim().is_empty() {
        println!("child stdout:\n{child_stdout}");
    }
    if !child_stderr.trim().is_empty() {
        println!("child stderr:\n{child_stderr}");
    }
    let combined = format!("{child_stdout}\n{child_stderr}");
    if child_output.status.success() {
        return Err(std::io::Error::other(
            "no-resize child boot unexpectedly succeeded; this regression requires a failing first boot attempt",
        )
        .into());
    }
    if !combined.contains("PMA is full") {
        return Err(std::io::Error::other(format!(
            "no-resize child boot failed for the wrong reason: status={} output={combined}",
            child_output.status
        ))
        .into());
    }
    println!("child reproduced initialization preserve PMA OOM");

    let after_child = Pma::read_file_metadata(&active_pma)?;
    println!(
        "active PMA after failed child boot: path={} data_words={} alloc_words={} free_words={} alloc_delta={}",
        active_pma.display(),
        after_child.data_words,
        after_child.alloc_words,
        after_child.data_words.saturating_sub(after_child.alloc_words),
        after_child.alloc_words.saturating_sub(forced.alloc_words)
    );
    assert_eq!(sqlite_max_event_num(&data_dir)?, 1);
    assert_eq!(max_runtime_pma_meta_event(&data_dir)?, 1);

    println!("stage 3: fixed production boot must grow or recover after failed preserve");
    match boot_app(&jam, &data_dir, NockStackSize::Small).await {
        Ok(mut recovered) => {
            assert_counter_state(&mut recovered, 1).await?;
            assert_eq!(sqlite_max_event_num(&data_dir)?, 1);
            assert_eq!(max_runtime_pma_meta_event(&data_dir)?, 1);
            let recovered_pma = active_runtime_pma(&data_dir)?;
            let recovered_metadata = Pma::read_file_metadata(&recovered_pma)?;
            println!(
                "recovered active PMA: path={} data_words={} alloc_words={} free_words={}",
                recovered_pma.display(),
                recovered_metadata.data_words,
                recovered_metadata.alloc_words,
                recovered_metadata
                    .data_words
                    .saturating_sub(recovered_metadata.alloc_words)
            );
            if recovered_metadata.data_words < NOCK_STACK_SIZE_SMALL as u64 {
                return Err(std::io::Error::other(format!(
                    "recovery boot succeeded but active PMA was not grown to configured small size: required_min_words={} actual_words={}",
                    NOCK_STACK_SIZE_SMALL, recovered_metadata.data_words
                ))
                .into());
            }
            stop_app(recovered).await?;
            println!("failed initialization preserve did not poison later recovery");
            Ok(())
        }
        Err(err) => Err(std::io::Error::other(format!(
            "production boot did not recover after failed initialization preserve: {err}"
        ))
        .into()),
    }
}

async fn child_no_resize_boot_attempt(data_dir: &Path) -> Result<(), Box<dyn Error>> {
    let jam = load_test_jam()?;
    match boot_app(&jam, data_dir, NockStackSize::Tiny).await {
        Ok(app) => {
            let _ = stop_app(app).await;
            Err(
                std::io::Error::other("deliberate no-resize child boot unexpectedly succeeded")
                    .into(),
            )
        }
        Err(err) => Err(err),
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
        "pma-failed-preserve-recovery-regression",
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
