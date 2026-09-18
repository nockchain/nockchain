use std::env;
use std::error::Error;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use ai_pow_jets::setup::{
    build_verifier_setup_seed, build_verifier_setup_seed_dense, load_verifier_setup_seeds,
    production_verifier_setup_buckets, save_verifier_setup_seeds, verifier_setup_seed_cache_path,
};

#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

const SHARD_DIRECTORY: &str = "verifier-setup-seed-shards-v1";
const DEFAULT_MALLOC_CONF: &str = "background_thread:true,dirty_decay_ms:0,muzzy_decay_ms:0";

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let command = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(usage)?;

    match command.as_str() {
        "generate-all" => {
            let data_dir = required_path(&mut args, "DATA_DIR")?;
            reject_extra_args(&mut args)?;
            generate_all(&data_dir)
        }
        "generate-bucket" => {
            let index = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or("generate-bucket requires INDEX")?
                .parse::<usize>()?;
            let shard_dir = required_path(&mut args, "SHARD_DIR")?;
            reject_extra_args(&mut args)?;
            generate_bucket(index, &shard_dir)
        }
        "assemble" => {
            let data_dir = required_path(&mut args, "DATA_DIR")?;
            reject_extra_args(&mut args)?;
            assemble(&data_dir)
        }
        "help" | "--help" | "-h" => {
            println!("{}", usage());
            Ok(())
        }
        _ => Err(format!("unknown command: {command}\n{}", usage()).into()),
    }
}

fn required_path(
    args: &mut impl Iterator<Item = std::ffi::OsString>,
    name: &str,
) -> Result<PathBuf, Box<dyn Error>> {
    args.next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("missing {name}\n{}", usage()).into())
}

fn reject_extra_args(
    args: &mut impl Iterator<Item = std::ffi::OsString>,
) -> Result<(), Box<dyn Error>> {
    if args.next().is_some() {
        return Err(format!("unexpected extra argument\n{}", usage()).into());
    }
    Ok(())
}

fn usage() -> String {
    "usage:\n  ai-pow-setup-cache generate-all DATA_DIR\n  ai-pow-setup-cache generate-bucket INDEX SHARD_DIR\n  ai-pow-setup-cache assemble DATA_DIR".to_owned()
}

fn generate_all(data_dir: &Path) -> Result<(), Box<dyn Error>> {
    let bucket_count = production_verifier_setup_buckets().len();
    let shard_dir = data_dir.join("ai-pow").join(SHARD_DIRECTORY);
    std::fs::create_dir_all(&shard_dir)?;
    let executable = env::current_exe()?;

    for index in 0..bucket_count {
        let shard = shard_path(&shard_dir, index);
        if shard.exists() {
            let seeds = load_verifier_setup_seeds(&shard)?;
            if seeds.len() != 1 {
                return Err(format!(
                    "{} contains {} seeds; expected exactly one",
                    shard.display(),
                    seeds.len()
                )
                .into());
            }
            eprintln!("reusing verifier setup seed shard {index}/{bucket_count}");
            continue;
        }

        eprintln!("generating verifier setup seed shard {index}/{bucket_count}");
        let mut child = Command::new(&executable);
        child
            .arg("generate-bucket")
            .arg(index.to_string())
            .arg(&shard_dir);
        if env::var_os("MALLOC_CONF").is_none() {
            child.env("MALLOC_CONF", DEFAULT_MALLOC_CONF);
        }
        if env::var_os("RAYON_NUM_THREADS").is_none() {
            child.env("RAYON_NUM_THREADS", "1");
        }
        let status = child.status()?;
        if !status.success() {
            return Err(format!("verifier setup seed shard {index} exited with {status}").into());
        }
    }

    assemble(data_dir)
}

fn generate_bucket(index: usize, shard_dir: &Path) -> Result<(), Box<dyn Error>> {
    let buckets = production_verifier_setup_buckets();
    let bucket = buckets
        .get(index)
        .ok_or_else(|| format!("bucket index {index} out of range 0..{}", buckets.len()))?;
    std::fs::create_dir_all(shard_dir)?;

    let seed = if bucket.dense {
        build_verifier_setup_seed_dense(&bucket.params)?
    } else {
        build_verifier_setup_seed(&bucket.params, bucket.hw, bucket.e, bucket.top_k)?
    };
    let output = shard_path(shard_dir, index);
    save_verifier_setup_seeds(&output, &[seed])?;
    println!(
        "generated verifier setup seed shard at {}",
        output.display()
    );
    Ok(())
}

fn assemble(data_dir: &Path) -> Result<(), Box<dyn Error>> {
    let bucket_count = production_verifier_setup_buckets().len();
    let shard_dir = data_dir.join("ai-pow").join(SHARD_DIRECTORY);
    let mut seeds = Vec::with_capacity(bucket_count);
    for index in 0..bucket_count {
        let input = shard_path(&shard_dir, index);
        let mut shard = load_verifier_setup_seeds(&input)?;
        if shard.len() != 1 {
            return Err(format!(
                "{} contains {} seeds; expected exactly one",
                input.display(),
                shard.len()
            )
            .into());
        }
        seeds.push(shard.remove(0));
    }

    let digest = ai_pow_jets::table_digest::verify_verifier_setup_seed_table_digest(&seeds)?;
    let output = verifier_setup_seed_cache_path(data_dir);
    save_verifier_setup_seeds(&output, &seeds)?;
    let mut digest_hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut digest_hex, "{byte:02x}")?;
    }
    println!(
        "assembled {bucket_count} verifier setup seed shards at {} digest={digest_hex}",
        output.display()
    );
    Ok(())
}

fn shard_path(shard_dir: &Path, index: usize) -> PathBuf {
    shard_dir.join(format!("bucket-{index:02}.bin"))
}
