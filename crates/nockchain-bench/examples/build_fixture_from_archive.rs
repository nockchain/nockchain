use std::fs::File;
use std::io::{self, Read};
use std::path::PathBuf;

use clap::Parser;
use nockchain_bench::speed_of_light::{
    checkpoint_event_num, slice_archive_file, write_fixture_file_from_paths, SolArchiveReader,
    SolFixtureCheckpointKind, SolFixtureManifest, SolHeight,
};
use sha2::{Digest, Sha256};

#[derive(Debug, Parser)]
struct Args {
    #[arg(long)]
    checkpoint: PathBuf,
    #[arg(long)]
    checkpoint_height: u64,
    #[arg(long)]
    kernel: PathBuf,
    #[arg(long)]
    source_archive: PathBuf,
    #[arg(long)]
    start_height: u64,
    #[arg(long, conflicts_with = "end_height")]
    count: Option<u64>,
    #[arg(long)]
    end_height: Option<u64>,
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    work_dir: PathBuf,
    #[arg(long, default_value_t = false)]
    include_mempool: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let end_height = match (args.count, args.end_height) {
        (Some(count), None) if count > 0 => args.start_height + count - 1,
        (None, Some(end_height)) => end_height,
        (Some(_), None) => return Err("--count must be greater than zero".into()),
        (None, None) => return Err("either --count or --end-height is required".into()),
        (Some(_), Some(_)) => unreachable!("clap conflict"),
    };
    if args.start_height > end_height {
        return Err("--start-height must be <= end height".into());
    }
    if args.start_height != args.checkpoint_height.saturating_add(1) {
        return Err(format!(
            "start height {} does not immediately follow checkpoint height {}",
            args.start_height, args.checkpoint_height
        )
        .into());
    }

    std::fs::create_dir_all(&args.work_dir)?;
    if let Some(parent) = args.output.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let slice_path = args.work_dir.join(format!(
        "archive-{}-{}.solarch",
        args.start_height, end_height
    ));
    let slice = slice_archive_file(
        &args.source_archive,
        &slice_path,
        SolHeight(args.start_height),
        SolHeight(end_height),
        args.include_mempool,
    )?;

    let reader = SolArchiveReader::from_file(&slice_path)?;
    if reader.block_count() != end_height - args.start_height + 1 {
        return Err(format!(
            "archive slice copied {} blocks, expected {}",
            reader.block_count(),
            end_height - args.start_height + 1
        )
        .into());
    }

    let manifest = SolFixtureManifest {
        source_archive_path: args.source_archive.display().to_string(),
        source_archive_event_num: None,
        checkpoint_kind: SolFixtureCheckpointKind::Full,
        checkpoint_height: SolHeight(args.checkpoint_height),
        checkpoint_event_num: checkpoint_event_num(&args.checkpoint)?,
        archive_start_height: SolHeight(args.start_height),
        archive_end_height: SolHeight(end_height),
        include_mempool: args.include_mempool,
        chunk_size: reader.block_count(),
        kernel_hash_hex: sha256_hex_for_file(&args.kernel)?,
        checkpoint_hash_hex: sha256_hex_for_file(&args.checkpoint)?,
        archive_hash_hex: sha256_hex_for_file(&slice_path)?,
    };

    write_fixture_file_from_paths(
        &args.output, &manifest, &args.checkpoint, &slice_path, &args.kernel,
    )?;

    println!("fixture={}", args.output.display());
    println!("checkpoint_height={}", args.checkpoint_height);
    println!("checkpoint_event_num={}", manifest.checkpoint_event_num);
    println!("archive_range={}..={}", args.start_height, end_height);
    println!("blocks={}", slice.block_count);
    println!("archive_slice={}", slice_path.display());
    Ok(())
}

fn sha256_hex_for_file(path: &PathBuf) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
