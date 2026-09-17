use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use clap::Parser;
use zk_pow_miner::cuda::{device_count, CudaSession};
use zk_pow_miner::v5::V5Job;

#[derive(Debug, Parser)]
#[command(
    name = "zk-pow-cuda-bench",
    about = "Validate and benchmark the exact Nockchain V5 ZK-PoW CUDA search kernel"
)]
struct Args {
    /// Comma-delimited CUDA device ordinals. Empty means every visible device.
    #[arg(long, value_delimiter = ',')]
    devices: Vec<u32>,

    /// Nonces evaluated in each kernel launch per device.
    #[arg(long, default_value = "262144")]
    batch_size: u64,

    /// Benchmark duration after correctness validation.
    #[arg(long, default_value = "10")]
    seconds: u64,

    /// V5 puzzle product length.
    #[arg(long, default_value = "64")]
    pow_len: u64,

    /// CUDA blocks launched per streaming multiprocessor.
    #[arg(long, default_value = "12")]
    blocks_per_sm: u32,

    /// CUDA threads per block. Must be a multiple of 32.
    #[arg(long, default_value = "512")]
    threads_per_block: u32,
}

#[derive(Debug)]
struct DeviceResult {
    device: u32,
    name: String,
    attempts: u64,
    batches: u64,
    wall_seconds: f64,
    kernel_seconds: f64,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.batch_size == 0 {
        bail!("--batch-size must be nonzero");
    }
    let devices = if args.devices.is_empty() {
        (0..device_count()?).collect::<Vec<_>>()
    } else {
        args.devices.clone()
    };
    if devices.is_empty() {
        bail!("no CUDA devices selected");
    }
    let mut unique = devices.clone();
    unique.sort_unstable();
    unique.dedup();
    if unique.len() != devices.len() {
        bail!("--devices contains a duplicate ordinal");
    }

    let barrier = Arc::new(Barrier::new(devices.len()));
    let mut handles = Vec::with_capacity(devices.len());
    for device in devices {
        let barrier = Arc::clone(&barrier);
        let duration = Duration::from_secs(args.seconds);
        let batch_size = args.batch_size;
        let blocks_per_sm = args.blocks_per_sm;
        let threads_per_block = args.threads_per_block;
        let job = V5Job::new([1, 2, 3, 4, 5], [0; 5], args.pow_len)?;
        handles.push(thread::spawn(move || -> Result<DeviceResult, String> {
            let mut session = CudaSession::with_launch_config(
                device,
                blocks_per_sm,
                threads_per_block,
            )
            .map_err(|error| error.to_string())?;
            let name = session.name().to_owned();
            let mut nonce = [u64::from(device), 7, 11, 13, 17];
            let gpu_digest = session
                .digest(&job, nonce)
                .map_err(|error| error.to_string())?;
            let cpu_digest = job.digest(nonce);
            if gpu_digest != cpu_digest {
                return Err(format!(
                    "device {device} ({name}) digest mismatch: GPU {gpu_digest:?}, CPU {cpu_digest:?}"
                ));
            }

            barrier.wait();
            let started = Instant::now();
            let deadline = started + duration;
            let mut attempts = 0u64;
            let mut batches = 0u64;
            let mut kernel_seconds = 0.0f64;
            loop {
                let result = session
                    .search(&job, nonce, batch_size)
                    .map_err(|error| error.to_string())?;
                if result.found {
                    return Err(format!(
                        "device {device} found an unexpected all-zero digest at {:?}",
                        result.next_nonce
                    ));
                }
                nonce = result.next_nonce;
                attempts = attempts.saturating_add(result.attempts);
                batches += 1;
                kernel_seconds += f64::from(result.kernel_ms) / 1_000.0;
                if Instant::now() >= deadline {
                    break;
                }
            }
            Ok(DeviceResult {
                device,
                name,
                attempts,
                batches,
                wall_seconds: started.elapsed().as_secs_f64(),
                kernel_seconds,
            })
        }));
    }

    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        results.push(
            handle
                .join()
                .map_err(|_| anyhow!("CUDA benchmark thread panicked"))?
                .map_err(anyhow::Error::msg)?,
        );
    }
    results.sort_unstable_by_key(|result| result.device);

    let mut aggregate_attempts = 0u64;
    let mut aggregate_rate = 0.0f64;
    for result in &results {
        let wall_rate = result.attempts as f64 / result.wall_seconds;
        let kernel_rate = result.attempts as f64 / result.kernel_seconds;
        aggregate_attempts = aggregate_attempts.saturating_add(result.attempts);
        aggregate_rate += wall_rate;
        println!(
            "device={} name={:?} attempts={} batches={} wall_s={:.3} kernel_s={:.3} wall_hps={:.3} kernel_hps={:.3}",
            result.device,
            result.name,
            result.attempts,
            result.batches,
            result.wall_seconds,
            result.kernel_seconds,
            wall_rate,
            kernel_rate,
        );
    }
    println!(
        "aggregate devices={} attempts={} wall_hps={:.3}",
        results.len(),
        aggregate_attempts,
        aggregate_rate,
    );
    Ok(())
}
