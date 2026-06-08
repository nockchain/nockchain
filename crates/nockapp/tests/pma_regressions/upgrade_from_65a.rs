use std::error::Error;
use std::fs;
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::BigInt;
use diesel::sqlite::SqliteConnection;
use nockapp::kernel::boot::{default_boot_cli, setup_, NockStackSize, PmaSize, SetupResult};
use nockapp::nockapp::wire::{SystemWire, Wire};
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::NockApp;
use nockvm::noun::{NounAllocator, NounSpace, D, T};
use nockvm::pma::Pma;
use nockvm_macros::tas;
use tempfile::TempDir;

use crate::pma_regressions::pma_meta::PmaPersistMetadataForTest;

const PMA_MAGIC: u64 = u64::from_le_bytes(*b"NOCKPMA1");
const PMA_VERSION_V1: u64 = 1;
const PMA_LEGACY_TRAILER_BYTES: u64 = 32;

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
    let data_dir = temp.path().join("pma-upgrade-from-65a-regression");
    let jam = load_test_jam()?;
    let configured_reserved_words = NockStackSize::Small.stack_words();

    println!("stage 1: create current PMA, then rewrite it as a 65a-style PMA fixture");
    let mut first = boot_app(&jam, &data_dir, configured_reserved_words).await?;
    poke_inc(&mut first).await?;
    poke_inc(&mut first).await?;
    assert_counter_state(&mut first, 2).await?;
    stop_app(first).await?;
    assert_eq!(sqlite_max_event_num(&data_dir)?, 2);

    let active_pma = active_runtime_pma(&data_dir)?;
    let active_meta_path = active_pma.with_extension("meta");
    let current_metadata = Pma::read_file_metadata(&active_pma)?;
    let current_meta = PmaPersistMetadataForTest::load(&active_meta_path)?;
    if current_meta.event_num != 2 {
        return Err(std::io::Error::other(format!(
            "expected event-2 sidecar before downgrade, got {}",
            current_meta.event_num
        ))
        .into());
    }

    downgrade_pma_file_to_65a(
        &active_pma, current_metadata.data_words, current_metadata.alloc_words,
    )?;
    current_meta.save_v4_to_path(&active_meta_path)?;

    let legacy_metadata = Pma::read_file_metadata(&active_pma)?;
    if legacy_metadata.version != PMA_VERSION_V1 {
        return Err(std::io::Error::other(format!(
            "downgraded fixture should read as PMA v1, got version {}",
            legacy_metadata.version
        ))
        .into());
    }
    if legacy_metadata.data_words != current_metadata.data_words
        || legacy_metadata.alloc_words != current_metadata.alloc_words
        || legacy_metadata.reserved_words != legacy_metadata.data_words
    {
        return Err(std::io::Error::other(format!(
            "65a fixture metadata mismatch: legacy={legacy_metadata:?} current={current_metadata:?}"
        ))
        .into());
    }
    let legacy_meta = PmaPersistMetadataForTest::load(&active_meta_path)?;
    if legacy_meta.event_num != 2 || legacy_meta.pma_reserved_words.is_some() {
        return Err(std::io::Error::other(format!(
            "65a fixture sidecar mismatch: event_num={} reserved={:?}",
            legacy_meta.event_num, legacy_meta.pma_reserved_words
        ))
        .into());
    }

    println!("stage 2: boot current code from the 65a-style PMA and continue event processing");
    let mut upgraded = boot_app(&jam, &data_dir, configured_reserved_words).await?;
    assert_counter_state(&mut upgraded, 2).await?;
    poke_inc(&mut upgraded).await?;
    assert_counter_state(&mut upgraded, 3).await?;
    stop_app(upgraded).await?;
    assert_eq!(sqlite_max_event_num(&data_dir)?, 3);

    let upgraded_pma = active_runtime_pma(&data_dir)?;
    let upgraded_metadata = Pma::read_file_metadata(&upgraded_pma)?;
    if upgraded_metadata.version != 2 {
        return Err(std::io::Error::other(format!(
            "upgraded PMA should be v2 after boot, got version {}",
            upgraded_metadata.version
        ))
        .into());
    }
    if upgraded_metadata.data_words < current_metadata.data_words {
        return Err(std::io::Error::other(format!(
            "upgrade shrank PMA capacity: before={} after={}",
            current_metadata.data_words, upgraded_metadata.data_words
        ))
        .into());
    }
    if upgraded_metadata.reserved_words != u64::try_from(configured_reserved_words)? {
        return Err(std::io::Error::other(format!(
            "upgrade did not apply configured reservation: expected={} actual={}",
            configured_reserved_words, upgraded_metadata.reserved_words
        ))
        .into());
    }
    let upgraded_meta = PmaPersistMetadataForTest::load(&upgraded_pma.with_extension("meta"))?;
    if upgraded_meta.event_num != 3
        || upgraded_meta.pma_reserved_words != Some(upgraded_metadata.reserved_words)
    {
        return Err(std::io::Error::other(format!(
            "upgraded sidecar mismatch: event_num={} reserved={:?} trailer_reserved={}",
            upgraded_meta.event_num,
            upgraded_meta.pma_reserved_words,
            upgraded_metadata.reserved_words
        ))
        .into());
    }

    println!("65a PMA fixture upgraded, preserved state, and accepted a new event");
    Ok(())
}

fn downgrade_pma_file_to_65a(
    path: &Path,
    data_words: u64,
    alloc_words: u64,
) -> Result<(), Box<dyn Error>> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let data_bytes = data_words
        .checked_mul(8)
        .ok_or_else(|| std::io::Error::other("PMA data byte length overflowed"))?;
    file.set_len(data_bytes + PMA_LEGACY_TRAILER_BYTES)?;
    file.seek(SeekFrom::Start(data_bytes))?;
    file.write_all(&PMA_MAGIC.to_le_bytes())?;
    file.write_all(&PMA_VERSION_V1.to_le_bytes())?;
    file.write_all(&data_words.to_le_bytes())?;
    file.write_all(&alloc_words.to_le_bytes())?;
    file.sync_all()?;
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
    pma_reserved_words: usize,
) -> Result<NockApp<NockJammer>, Box<dyn Error>> {
    let mut cli = default_boot_cli(false);
    cli.data_dir = Some(data_dir.to_path_buf());
    cli.pma_initial_size = Some(PmaSize::from_words(NockStackSize::Tiny.stack_words()));
    cli.pma_reserved_size = Some(PmaSize::from_words(pma_reserved_words));
    cli.stack_size = NockStackSize::Tiny;
    cli.gc_interval = None;
    cli.rotating_snapshot_interval_event_time = None;
    cli.disable_fsync = true;
    match setup_::<NockJammer>(jam, cli, &[], "pma-upgrade-from-65a-regression", None).await? {
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
    let result = app.peek(state_peek()).await?;
    let space = result.noun_space();
    let root = unsafe { *result.root() };
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
        || std::io::Error::other(format!("invalid sqlite path: {}", path.display())),
    )?)?)
}

fn sqlite_max_event_num(data_dir: &Path) -> Result<u64, Box<dyn Error>> {
    let mut conn = sqlite_connection(data_dir)?;
    let row = sql_query("SELECT COALESCE(MAX(event_num), 0) AS value FROM events")
        .get_result::<I64ValueRow>(&mut conn)?;
    Ok(u64::try_from(row.value)?)
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
