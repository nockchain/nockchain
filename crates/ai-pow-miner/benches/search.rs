//! Release-mode baseline for scalar AI-PoW ticket search.
//!
//! The benchmark measures the actual search evaluators, not certificate work.
//! Canonical attempts use the production-compatible fixed MoE route. The dense
//! measurement evaluates all 4,096 `DENSE_PRODUCTION_PARAMS` offset pairs with
//! impossible Pearl and Nockchain thresholds, so it cannot terminate early.

#![allow(clippy::unwrap_used)] // benchmark setup uses fixed valid fixtures

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ai_pow::params::MatmulParams;
use ai_pow::pearl_compat::{
    prepare_pearl_pattern_job, PearlIncompleteBlockHeader, PearlMiningConfig, PearlPeriodicPattern,
    PEARL_MINING_CONFIG_RESERVED_SIZE, PEARL_MMA_INT7XINT7_TO_INT32,
};
use ai_pow::synth::{synth_matrices, AI_POW_PROD_SYNTH_SEED};
use ai_pow_miner::canonical::PreparedCanonicalMoeTemplate;
#[cfg(feature = "gpu")]
use ai_pow_miner::gpu::GpuSearchBackend;
use ai_pow_miner::search::{CpuSearchBackend, SearchBackend, SearchBatch};
use ai_pow_miner::DENSE_PRODUCTION_PARAMS;

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

fn print_measurement(name: &str, attempts: u64, workers: usize, measurement: Measurement) {
    let elapsed_s = measurement.elapsed.as_secs_f64();
    let attempts_per_s = (attempts as f64) / elapsed_s;
    println!(
        "{name}: attempts={attempts} elapsed_ms={:.3} attempts_per_s={attempts_per_s:.3} allocations={} workers={workers}",
        elapsed_s * 1_000.0,
        measurement.allocations,
    );
}

fn main() {
    let canonical_attempts = std::env::var("AI_POW_SEARCH_BENCH_CANONICAL_ATTEMPTS")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|&attempts| attempts > 0)
        .unwrap_or(256);
    let workers = CpuSearchBackend::default_worker_count();
    let backend = CpuSearchBackend::new(workers).expect("dedicated CPU search backend");
    let canonical = canonical_params();
    let canonical_template = Arc::new(
        PreparedCanonicalMoeTemplate::new(&canonical, 8, 2, 1, [0x5a; 32])
            .expect("canonical template"),
    );
    backend
        .search_canonical(
            Arc::clone(&canonical_template),
            SearchBatch::new(0, 1, [0; 32]).expect("canonical warmup batch"),
        )
        .expect("canonical dedicated warmup");
    let mut canonical_prepare_scratch = canonical_template.scratch();
    let canonical_prepare_measurement = measure(|| {
        for extranonce in 0..canonical_attempts {
            std::hint::black_box(
                canonical_template.prepare_attempt(extranonce, &mut canonical_prepare_scratch),
            );
        }
    });
    print_measurement(
        "canonical_prepare_scalar",
        u64::from(canonical_attempts),
        1,
        canonical_prepare_measurement,
    );
    let mut canonical_scratch = canonical_template.scratch();
    let canonical_measurement = measure(|| {
        for extranonce in 0..canonical_attempts {
            std::hint::black_box(
                canonical_template
                    .evaluate(extranonce, &mut canonical_scratch)
                    .jackpot_hash,
            );
        }
    });
    print_measurement(
        "canonical_prepared_scalar",
        u64::from(canonical_attempts),
        1,
        canonical_measurement,
    );
    let canonical_parallel_measurement = measure(|| {
        std::hint::black_box(
            backend
                .search_canonical(
                    Arc::clone(&canonical_template),
                    SearchBatch::new(0, u64::from(canonical_attempts), [0; 32])
                        .expect("canonical batch"),
                )
                .expect("canonical dedicated search"),
        );
    });
    print_measurement(
        "canonical_prepared_dedicated",
        u64::from(canonical_attempts),
        workers,
        canonical_parallel_measurement,
    );

    #[cfg(feature = "gpu")]
    {
        let gpu_backend =
            GpuSearchBackend::new(0, u64::from(canonical_attempts)).expect("CUDA search backend");
        gpu_backend
            .search_canonical(
                Arc::clone(&canonical_template),
                SearchBatch::new(0, 1, [0; 32]).expect("CUDA warmup batch"),
            )
            .expect("CUDA warmup");
        let canonical_gpu_measurement = measure(|| {
            std::hint::black_box(
                gpu_backend
                    .search_canonical(
                        Arc::clone(&canonical_template),
                        SearchBatch::new(0, u64::from(canonical_attempts), [0; 32])
                            .expect("CUDA canonical batch"),
                    )
                    .expect("CUDA canonical search"),
            );
        });
        print_measurement(
            "canonical_prepared_cuda",
            u64::from(canonical_attempts),
            1,
            canonical_gpu_measurement,
        );
    }

    let header = dense_header();
    let config = dense_config();
    let (a, b) = synth_matrices(AI_POW_PROD_SYNTH_SEED, &DENSE_PRODUCTION_PARAMS);
    let dense_prepared = Arc::new(
        prepare_pearl_pattern_job(&header, &config, &DENSE_PRODUCTION_PARAMS, &a, &b, 8)
            .expect("prepared dense Pearl job"),
    );
    backend
        .search_dense(
            Arc::clone(&dense_prepared),
            SearchBatch::new(0, 1, [0; 32]).expect("dense warmup batch"),
        )
        .expect("dense dedicated warmup");
    let dense_attempts = u64::try_from(
        dense_prepared
            .row_offsets()
            .len()
            .checked_mul(dense_prepared.col_offsets().len())
            .expect("dense attempt count"),
    )
    .expect("dense attempt count fits u64");
    let mut dense_scratch = dense_prepared.scratch();
    let dense_measurement = measure(|| {
        for &t_rows in dense_prepared.row_offsets() {
            for &t_cols in dense_prepared.col_offsets() {
                std::hint::black_box(
                    dense_prepared
                        .evaluate(t_rows, t_cols, &mut dense_scratch)
                        .expect("prepared dense ticket")
                        .jackpot_hash,
                );
            }
        }
    });
    print_measurement(
        "dense_prepared_scalar_full_sweep", dense_attempts, 1, dense_measurement,
    );
    let dense_parallel_measurement = measure(|| {
        std::hint::black_box(
            backend
                .search_dense(
                    Arc::clone(&dense_prepared),
                    SearchBatch::new(0, dense_attempts, [0; 32]).expect("dense batch"),
                )
                .expect("dense dedicated search"),
        );
    });
    print_measurement(
        "dense_prepared_dedicated_full_sweep", dense_attempts, workers, dense_parallel_measurement,
    );
}
