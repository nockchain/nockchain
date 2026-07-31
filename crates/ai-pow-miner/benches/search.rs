//! Release-mode baseline for scalar AI-PoW ticket search.
//!
//! The benchmark measures the actual search evaluators, not certificate work.
//! Canonical attempts use the production-compatible fixed MoE route. The dense
//! measurement evaluates all 4,096 `DENSE_PRODUCTION_PARAMS` offset pairs with
//! impossible Pearl and Nockchain thresholds, so it cannot terminate early.

#![allow(clippy::unwrap_used)] // benchmark setup uses fixed valid fixtures

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ai_pow::params::MatmulParams;
use ai_pow::pearl_compat::{
    PearlIncompleteBlockHeader, PearlMiningConfig, PearlNockchainAux, PearlPeriodicPattern,
    PEARL_MINING_CONFIG_RESERVED_SIZE, PEARL_MMA_INT7XINT7_TO_INT32,
};
use ai_pow::synth::{synth_matrices, AI_POW_PROD_SYNTH_SEED};
use ai_pow_miner::canonical::evaluate_canonical_moe_jackpot;
use ai_pow_miner::pearl_mining::{
    run, PearlMergeMineOptions, PearlMergeMiningError, PearlMergeMiningJob,
};
use ai_pow_miner::{MiningCancel, DENSE_PRODUCTION_PARAMS};

struct CountingAllocator;

static COUNT_ALLOCATIONS: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNT_ALLOCATIONS.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if COUNT_ALLOCATIONS.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNT_ALLOCATIONS.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

struct Measurement {
    elapsed: Duration,
    allocations: u64,
}

fn measure(operation: impl FnOnce()) -> Measurement {
    ALLOCATIONS.store(0, Ordering::Relaxed);
    COUNT_ALLOCATIONS.store(true, Ordering::Release);
    let started = Instant::now();
    operation();
    let elapsed = started.elapsed();
    COUNT_ALLOCATIONS.store(false, Ordering::Release);
    Measurement {
        elapsed,
        allocations: ALLOCATIONS.load(Ordering::Relaxed),
    }
}

fn canonical_params() -> MatmulParams {
    MatmulParams {
        m: 64,
        k: 1024,
        n: 64,
        noise_rank: 64,
        tile: 8,
        spot_checks: 1,
        difficulty_bits: 0,
    }
}

fn pattern(length: u32) -> PearlPeriodicPattern {
    PearlPeriodicPattern {
        shape: [(1, length), (length, 1), (length, 1)],
    }
}

fn dense_config() -> PearlMiningConfig {
    PearlMiningConfig {
        common_dim: DENSE_PRODUCTION_PARAMS.k,
        rank: DENSE_PRODUCTION_PARAMS.noise_rank as u16,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: pattern(8),
        cols_pattern: pattern(8),
        reserved: [0; PEARL_MINING_CONFIG_RESERVED_SIZE],
    }
}

fn dense_header() -> PearlIncompleteBlockHeader {
    PearlIncompleteBlockHeader {
        version: 0x0102_0304,
        prev_block: [0x11; 32],
        merkle_root: [0x22; 32],
        timestamp: 0x6677_8899,
        nbits: 0x1d00_0000,
    }
}

fn dense_aux() -> PearlNockchainAux {
    PearlNockchainAux {
        nockchain_chain_id: b"nockchain-mainnet".to_vec(),
        nock_block_commitment: [0x42; 32],
        nockchain_target_epoch_or_height: 123_456,
        extra_domain_data: b"ai-pow-search-benchmark".to_vec(),
    }
}

fn print_measurement(name: &str, attempts: u64, measurement: Measurement) {
    let elapsed_s = measurement.elapsed.as_secs_f64();
    let attempts_per_s = (attempts as f64) / elapsed_s;
    println!(
        "{name}: attempts={attempts} elapsed_ms={:.3} attempts_per_s={attempts_per_s:.3} allocations={} workers=1",
        elapsed_s * 1_000.0,
        measurement.allocations,
    );
}

fn main() {
    let canonical_attempts = std::env::var("AI_POW_SEARCH_BENCH_CANONICAL_ATTEMPTS")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|&attempts| attempts > 0)
        .unwrap_or(16);
    let canonical = canonical_params();
    let canonical_measurement = measure(|| {
        for extranonce in 0..canonical_attempts {
            std::hint::black_box(
                evaluate_canonical_moe_jackpot(&canonical, 8, 2, 1, [0x5a; 32], extranonce)
                    .expect("canonical jackpot"),
            );
        }
    });
    print_measurement(
        "canonical_scalar",
        u64::from(canonical_attempts),
        canonical_measurement,
    );

    let header = dense_header();
    let config = dense_config();
    let (a, b) = synth_matrices(AI_POW_PROD_SYNTH_SEED, &DENSE_PRODUCTION_PARAMS);
    let dense_job = PearlMergeMiningJob {
        header: &header,
        config: &config,
        params: &DENSE_PRODUCTION_PARAMS,
        nockchain_target: [0; 32],
        a: &a,
        b: &b,
        max_pattern_len: 8,
        aux: dense_aux(),
    };
    let dense_measurement = measure(|| {
        let outcome = run(
            &dense_job,
            &PearlMergeMineOptions {
                progress_interval: None,
                ..PearlMergeMineOptions::default()
            },
            MiningCancel::new(),
        );
        assert!(
            matches!(outcome, Err(PearlMergeMiningError::AttemptSpaceExhausted)),
            "impossible targets must sweep the complete dense offset space"
        );
    });
    print_measurement("dense_scalar_full_sweep", 4_096, dense_measurement);
}
