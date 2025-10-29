use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use clap::Parser;
use hoonc::Error;
use nockapp::export::ExportedState;
use nockapp::kernel::boot::{self, default_boot_cli};
use nockapp::kernel::form::LoadState;
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::one_punch::OnePunchWire;
use nockapp::save::JammedCheckpoint;
use nockapp::wire::Wire;
use nockapp::{exit_driver, file_driver, AtomExt};
use nockvm::noun::{Atom, D, T};
use nockvm_macros::tas;
use tempfile::TempDir;

#[derive(Parser, Debug)]
struct Args {
    /// Output path for the prebaked hoonc kernel jam
    #[arg(long, default_value = "open/crates/hoonc/bootstrap/hoonc-prewarm.jam")]
    output: PathBuf,

    /// Optional base directory for hoonc data; defaults to a temporary directory
    #[arg(long)]
    data_dir: Option<PathBuf>,

    /// Leave the data directory intact instead of removing it on exit
    #[arg(long)]
    keep_data_dir: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let (temp_dir, base_data_dir) = prepare_data_dir(args.data_dir.clone())?;

    let mut boot_cli = default_boot_cli(true);
    boot_cli.new = true;

    let mut nockapp = boot::setup::<NockJammer>(
        hoonc::KERNEL_JAM,
        boot_cli,
        &[],
        "hoonc",
        Some(base_data_dir.clone()),
    )
    .await?;

    nockapp.add_io_driver(file_driver()).await;
    nockapp.add_io_driver(exit_driver()).await;

    run_boot_poke(&mut nockapp).await?;
    nockapp.save_blocking().await?;

    let checkpoints_dir = base_data_dir.join("hoonc").join("checkpoints");
    let latest_checkpoint = find_latest_checkpoint(&checkpoints_dir)?;
    let exported = checkpoint_to_exported_state(&latest_checkpoint)?;

    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&args.output, &exported)?;

    println!(
        "Wrote prewarmed kernel state from {} to {}",
        latest_checkpoint.display(),
        args.output.display()
    );

    if args.keep_data_dir {
        if let Some(dir) = temp_dir {
            let _ = dir.keep();
        }
    }

    Ok(())
}

fn prepare_data_dir(
    override_dir: Option<PathBuf>,
) -> Result<(Option<TempDir>, PathBuf), Box<dyn std::error::Error>> {
    if let Some(dir) = override_dir {
        fs::create_dir_all(&dir)?;
        return Ok((None, dir));
    }

    let temp_dir = TempDir::new()?;
    let path = temp_dir.path().to_path_buf();
    Ok((Some(temp_dir), path))
}

async fn run_boot_poke(nockapp: &mut nockapp::NockApp<NockJammer>) -> Result<(), Error> {
    let mut slab = NounSlab::new();
    let hoon_cord = Atom::from_value(&mut slab, hoonc::HOON_TXT)
        .expect("Failed to create hoon cord")
        .as_noun();
    let bootstrap_poke = T(&mut slab, &[D(tas!(b"boot")), hoon_cord]);
    slab.set_root(bootstrap_poke);

    nockapp
        .poke(OnePunchWire::Poke.to_wire(), slab)
        .await
        .map(|_| ())
        .map_err(|e| -> Error { Box::new(e) })
}

fn find_latest_checkpoint(dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut latest: Option<(SystemTime, PathBuf)> = None;

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let metadata = entry.metadata()?;
        let modified = metadata.modified()?;
        if latest
            .as_ref()
            .map(|(time, _)| modified > *time)
            .unwrap_or(true)
        {
            latest = Some((modified, entry.path()));
        }
    }

    latest
        .map(|(_, path)| path)
        .ok_or_else(|| format!("No checkpoint found in {}", dir.display()).into())
}

fn checkpoint_to_exported_state(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let bytes = fs::read(path)?;
    let checkpoint = JammedCheckpoint::decode_from_bytes(&bytes)
        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;

    let mut kernel_slab = NounSlab::<NockJammer>::new();
    let kernel_root = kernel_slab.cue_into(checkpoint.state_jam.0.clone())?;
    kernel_slab.set_root(kernel_root);

    let load_state = LoadState {
        ker_hash: checkpoint.ker_hash,
        event_num: checkpoint.event_num,
        kernel_state: kernel_slab,
    };
    let exported = ExportedState::from_loadstate(load_state);
    Ok(exported.encode()?)
}
