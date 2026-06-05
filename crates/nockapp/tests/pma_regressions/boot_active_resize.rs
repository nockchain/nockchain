use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use nockapp::kernel::boot::{default_boot_cli, setup_, NockStackSize, PmaSize, SetupResult};
use nockapp::nockapp::wire::{SystemWire, Wire};
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::NockApp;
use nockvm::noun::{NounSpace, D};
use nockvm::offset::PmaOffsetWords;
use nockvm::pma::Pma;
use nockvm_macros::tas;
use tempfile::TempDir;

const FORCED_FREE_WORDS: u64 = 2;

pub(crate) fn run_regression() -> Result<(), Box<dyn Error>> {
    nockvm::check_endian();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run())
}

async fn run() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let data_dir = temp.path().join("pma-boot-active-resize-regression");
    let jam = load_test_jam()?;

    println!("stage 1: create valid active tiny PMA at event 1");
    let mut first = boot_app(&jam, &data_dir, NockStackSize::Tiny).await?;
    poke_inc(&mut first).await?;
    let event_num = first.export().await?.event_num;
    assert_eq!(event_num, 1, "fixture must be at event 1 before PMA fill");
    stop_app(first).await?;

    let pma_path = data_dir.join("pma").join("0.pma");
    let meta_path = data_dir.join("pma").join("0.meta");
    if !pma_path.exists() || !meta_path.exists() {
        return Err(std::io::Error::other(format!(
            "expected active 0.pma/0.meta after first boot, got pma_exists={} meta_exists={}",
            pma_path.exists(),
            meta_path.exists()
        ))
        .into());
    }

    force_pma_free_words(&pma_path, FORCED_FREE_WORDS)?;
    let filled = Pma::read_file_metadata(&pma_path)?;
    println!(
        "fixture active PMA forced near full: path={} data_words={} alloc_words={} free_words={}",
        pma_path.display(),
        filled.data_words,
        filled.alloc_words,
        filled.data_words.saturating_sub(filled.alloc_words)
    );
    assert_eq!(
        filled.data_words.saturating_sub(filled.alloc_words),
        FORCED_FREE_WORDS,
        "fixture must reproduce production free-space condition"
    );

    println!("stage 2: reboot with larger configured stack/PMA minimum");
    match boot_app(&jam, &data_dir, NockStackSize::Small).await {
        Ok(second) => {
            let event_num = second.export().await?.event_num;
            assert_eq!(
                event_num, 1,
                "boot must preserve the committed event boundary after active PMA resize"
            );
            let active = largest_meta_paired_pma(&data_dir)?;
            let active_metadata = Pma::read_file_metadata(&active)?;
            println!(
                "post-boot active PMA candidate: path={} data_words={} alloc_words={} free_words={}",
                active.display(),
                active_metadata.data_words,
                active_metadata.alloc_words,
                active_metadata
                    .data_words
                    .saturating_sub(active_metadata.alloc_words)
            );
            if active_metadata.data_words <= filled.data_words {
                return Err(std::io::Error::other(format!(
                    "boot succeeded but active PMA was not grown: before_data_words={} after_data_words={}",
                    filled.data_words, active_metadata.data_words
                ))
                .into());
            }
            stop_app(second).await?;
            println!("active full PMA was resized by production boot before Serf initialization");
            Ok(())
        }
        Err(err) => {
            let message = err.to_string();
            if message.contains("PMA is full") {
                eprintln!(
                    "reproduced pre-fix production failure: active valid PMA selected with only {FORCED_FREE_WORDS} free words, then Serf initialization hit PMA OOM"
                );
            }
            Err(std::io::Error::other(format!(
                "production boot did not resize active full PMA before Serf initialization: {message}"
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
    match setup_::<NockJammer>(jam, cli, &[], "pma-boot-active-resize-regression", None).await? {
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

async fn poke_inc(app: &mut NockApp<NockJammer>) -> Result<(), Box<dyn Error>> {
    app.poke(SystemWire.to_wire(), inc_poke()).await?;
    Ok(())
}

async fn stop_app(mut app: NockApp<NockJammer>) -> Result<(), Box<dyn Error>> {
    let handle = app.get_handle();
    handle.exit.exit(0).await?;
    app.run().await?;
    Ok(())
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

fn largest_meta_paired_pma(data_dir: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let pma_dir = data_dir.join("pma");
    let mut best: Option<(PathBuf, u64)> = None;
    for idx in [0, 1] {
        let pma_path = pma_dir.join(format!("{idx}.pma"));
        let meta_path = pma_dir.join(format!("{idx}.meta"));
        if !pma_path.exists() || !meta_path.exists() {
            continue;
        }
        let metadata = Pma::read_file_metadata(&pma_path)?;
        match &best {
            Some((_, best_words)) if *best_words >= metadata.data_words => {}
            _ => best = Some((pma_path, metadata.data_words)),
        }
    }
    best.map(|(path, _)| path).ok_or_else(|| {
        std::io::Error::other(format!(
            "no meta-paired runtime PMA found in {}",
            pma_dir.display()
        ))
        .into()
    })
}
