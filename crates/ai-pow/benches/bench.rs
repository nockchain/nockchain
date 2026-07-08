//! Criterion benches for the AI-PoW prover and verifier.
//!
//! These run at `MatmulParams::TEST_SMALL` by default for fast feedback in
//! CI; pass `BENCH_PROFILE=mid` or `BENCH_PROFILE=prod` in the environment
//! to bench larger profiles.

use std::env;
use std::time::Duration;

use ai_pow::params::MatmulParams;
use ai_pow::prover::{mine, mine_block, ProverOptions};
use ai_pow::synth::synth_matrices;
use ai_pow::verifier::verify;
use criterion::{criterion_group, criterion_main, Criterion};

fn pick_params() -> MatmulParams {
    match env::var("BENCH_PROFILE").as_deref() {
        Ok("prod") => MatmulParams::PROD,
        Ok("gemma4_31b_ffn") => MatmulParams::GEMMA_4_31B_FFN,
        Ok("qwen3_6_27b_ffn") => MatmulParams::QWEN_3_6_27B_FFN,
        Ok("mid") => MatmulParams {
            m: 256,
            k: 256,
            n: 256,
            noise_rank: 16,
            tile: 32,
            spot_checks: 16,
            difficulty_bits: 0,
        },
        _ => MatmulParams::TEST_SMALL,
    }
}

fn bench_prover(c: &mut Criterion) {
    let params = pick_params();
    let (a, b) = synth_matrices(b"bench-ab", &params);
    c.bench_function("prover.mine.one_attempt", |bencher| {
        bencher.iter(|| mine(b"hdr", b"nce", &a, &b, &params, ProverOptions::default()).unwrap())
    });
}

fn bench_verifier(c: &mut Criterion) {
    let params = pick_params();
    let (a, b) = synth_matrices(b"bench-ab", &params);
    let proof = mine(b"hdr", b"nce", &a, &b, &params, ProverOptions::default())
        .unwrap()
        .unwrap();
    c.bench_function("verifier.verify", |bencher| {
        bencher.iter(|| verify(b"hdr", b"nce", &params, &proof).unwrap())
    });
}

fn bench_mine_block_nonce_bound(c: &mut Criterion) {
    // Sweep 4 nonces with a difficulty so tight nothing satisfies. Each nonce
    // now rebuilds the attempt transcript, commitments, noise, matmul-derived
    // tile states, and final hashes; this must not measure final-hash-only
    // retry cost.
    let mut params = pick_params();
    params.difficulty_bits = 400;
    let (a, b) = synth_matrices(b"bench-ab", &params);
    c.bench_function("prover.mine_block.4_nonces_no_match", |bencher| {
        bencher.iter(|| {
            mine_block(
                b"hdr",
                [b"n1" as &[u8], b"n2", b"n3", b"n4"],
                &a,
                &b,
                &params,
                ProverOptions::default(),
            )
            .unwrap()
        })
    });
    c.bench_function("prover.mine.one_attempt_no_match", |bencher| {
        bencher.iter(|| mine(b"hdr", b"n1", &a, &b, &params, ProverOptions::default()).unwrap())
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(10)
        .measurement_time(Duration::from_secs(5));
    targets = bench_prover, bench_verifier, bench_mine_block_nonce_bound
}
criterion_main!(benches);
