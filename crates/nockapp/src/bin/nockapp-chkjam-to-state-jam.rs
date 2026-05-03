use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Parser;
use nockapp::export::ExportedState;
use nockapp::kernel::form::LoadState;
use nockapp::noun::slab::NockJammer;
use nockapp::save::{SaveableCheckpoint, Saver};
use tempfile::TempDir;
use tokio::fs;

#[derive(Debug, Parser)]
#[command(
    name = "nockapp-chkjam-to-state-jam",
    about = "Convert checkpoint jam file(s) into state jam file(s) compatible with --state-jam"
)]
struct Cli {
    /// Input checkpoint jam file(s) or checkpoint directories containing 0.chkjam/1.chkjam
    #[arg(long = "input", required = true)]
    inputs: Vec<PathBuf>,

    /// Output file path (single input only)
    #[arg(long, conflicts_with = "output_dir")]
    output: Option<PathBuf>,

    /// Output directory for generated state jam files
    #[arg(long)]
    output_dir: Option<PathBuf>,

    /// Suffix for generated files when using --output-dir
    #[arg(long, default_value = ".state.jam")]
    suffix: String,

    /// Overwrite existing output files
    #[arg(long, default_value_t = false)]
    overwrite: bool,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.output.is_some() && cli.inputs.len() != 1 {
        bail!("--output only supports exactly one --input");
    }

    let default_output_dir =
        std::env::current_dir().context("failed to resolve current directory")?;
    let output_dir = cli.output_dir.clone().unwrap_or(default_output_dir);

    if cli.output.is_none() {
        fs::create_dir_all(&output_dir)
            .await
            .with_context(|| format!("failed to create output dir {}", output_dir.display()))?;
    }

    for input in &cli.inputs {
        let checkpoint = load_checkpoint(input).await?;
        let output_path = match &cli.output {
            Some(path) => path.clone(),
            None => output_dir.join(output_name_for(input, &cli.suffix)),
        };

        if output_path.exists() && !cli.overwrite {
            bail!(
                "output already exists (pass --overwrite): {}",
                output_path.display()
            );
        }

        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).await.with_context(|| {
                format!(
                    "failed to create output parent directory {}",
                    parent.display()
                )
            })?;
        }

        let load_state = LoadState {
            ker_hash: checkpoint.ker_hash,
            event_num: checkpoint.event_num,
            kernel_state: checkpoint.state,
        };
        let exported = ExportedState::from_loadstate(load_state);
        let event_num = exported.event_num;
        let ker_hash = exported.ker_hash;
        let encoded = exported
            .encode()
            .context("failed to encode exported state")?;

        // Verify output can be decoded back into a load state before writing.
        let decoded = ExportedState::decode(&encoded).context("failed to decode output bytes")?;
        let _ = decoded.to_loadstate().map_err(|err| {
            anyhow::anyhow!("decoded output did not produce a valid load state: {err}")
        })?;

        fs::write(&output_path, encoded)
            .await
            .with_context(|| format!("failed to write {}", output_path.display()))?;

        println!(
            "wrote {} (input={} event_num={} ker_hash={})",
            output_path.display(),
            input.display(),
            event_num,
            ker_hash,
        );
    }

    Ok(())
}

async fn load_checkpoint(input: &Path) -> Result<SaveableCheckpoint> {
    if input.is_dir() {
        let path = input.to_path_buf();
        let (_, checkpoint) = Saver::<NockJammer>::try_load::<SaveableCheckpoint>(&path, None)
            .await
            .with_context(|| format!("failed to load checkpoint directory {}", input.display()))?;
        return checkpoint.with_context(|| format!("no checkpoint found in {}", input.display()));
    }

    if !input.exists() {
        bail!("input does not exist: {}", input.display());
    }
    if !input.is_file() {
        bail!("input must be a file or directory: {}", input.display());
    }

    let temp = TempDir::new().context("failed to create temporary directory")?;
    let checkpoint_dir = temp.path().join("checkpoints");
    fs::create_dir_all(&checkpoint_dir)
        .await
        .context("failed to create temporary checkpoints directory")?;

    let copied = checkpoint_dir.join("0.chkjam");
    fs::copy(input, &copied)
        .await
        .with_context(|| format!("failed to copy {} to {}", input.display(), copied.display()))?;

    let (_, checkpoint) =
        Saver::<NockJammer>::try_load::<SaveableCheckpoint>(&checkpoint_dir, None)
            .await
            .with_context(|| {
                format!(
                    "failed to decode checkpoint from temporary copy of {}",
                    input.display()
                )
            })?;

    checkpoint.with_context(|| format!("no checkpoint found in {}", input.display()))
}

fn output_name_for(input: &Path, suffix: &str) -> String {
    let base = if input.is_dir() {
        input
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("checkpoint")
            .to_owned()
    } else {
        input
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("checkpoint")
            .to_owned()
    };

    format!("{base}{suffix}")
}
