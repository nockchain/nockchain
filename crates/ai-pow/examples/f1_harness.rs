//! F1 end-to-end harness — real `ai-pow` solve → `ai-pow-zk`
//! SNARK, instrumented.
//!
//! This is the substrate the GAP_AUDIT recommends: a genuine
//! cross-crate path (real matrices → real κ → SNARK binds the
//! chunk-Merkle commitment → real difficulty target →
//! `composite_verify_pow_pinned_logup`) that doubles as the
//! profiling / benchmarking fixture. It is deterministic and
//! emits one machine-readable metrics line.
//!
//! ## What it exercises (today)
//!
//! - `ai-pow` real solve: `mine` at `TEST_SMALL` (difficulty_bits
//!   = 0 ⇒ a tile always clears), proving the puzzle path runs.
//! - `BlockContext` → `h_a_chunk` / `h_b_chunk` (M52 step 5).
//! - `ai-pow-zk` matrix-hash A/B + C1 key-pins + C4 jackpot-hash
//!   into a `CompositeTrace`.
//! - **HIGH-2.2 §4.C Route A**: `composite_prove_pinned_logup` +
//!   `composite_verify_pow_pinned_logup` (batch-stark — CRIT-1
//!   program-pin **and** the `noised_packed`/range LogUp) against
//!   the real `difficulty_target` (C2).
//! - Hard assertions (non-vacuous): `HASH_A`/`HASH_B` byte-equal
//!   `BlockContext::h_a_chunk`/`h_b_chunk` (C3); `JOB_KEY`/
//!   `COMMITMENT_HASH` == the block's κ / nonce-bound jackpot key (C1);
//!   `HASH_JACKPOT` is the non-zero keyed digest (C4).
//!
//! ## What it does NOT yet exercise (tracked separately — §4.A)
//!
//! No real matmul rows are placed, so the `noised_packed`
//! *matmul-input* binding has no queries to bind (the LogUp
//! still balances; CRIT-1 + the C1/C3/C4 PI bindings are live
//! and asserted). Placing the real solved tile's matmul→fold
//! chain (so `JACKPOT_MSG` = the real `TileState M` and the
//! `noised_packed` binding bites) is HIGH-2.2 §4.A — see
//! `crates/ai-pow-zk/docs/2026-05-15_HIGH2_2_DESIGN.md`.
//!
//! ## Running
//!
//!   cargo run -p ai-pow --release --features zk --example f1_harness
//!
//! Env knobs:
//!   F1_SEED   — matrix synth seed (default "f1-harness-v1")
//!   F1_ITERS  — prove+verify iterations for profiling (default 1)
//!
//! See `crates/ai-pow-zk/docs/2026-05-15_PROFILING.md` for samply / peak-RSS
//! recipes that wrap this binary.

#[cfg(not(feature = "zk"))]
fn main() {
    eprintln!("f1_harness requires the `zk` feature: cargo run -p ai-pow --release --features zk --example f1_harness");
    std::process::exit(2);
}

#[cfg(feature = "zk")]
fn main() {
    use std::time::Instant;

    use ai_pow::params::MatmulParams;
    use ai_pow::prover::{mine, BlockContext, ProverOptions};
    use ai_pow::synth::synth_matrices;
    use ai_pow::tile_hash::difficulty_target;
    use ai_pow_zk::composite_proof::{
        build_config, composite_prove_pinned_logup, composite_verify_pow_pinned_logup,
    };
    use ai_pow_zk::{CircuitConfig, CompositePublicInputs, CompositeTrace, ZkParams};

    let seed = std::env::var("F1_SEED").unwrap_or_else(|_| "f1-harness-v1".to_string());
    let iters: u32 = std::env::var("F1_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    let params = MatmulParams::TEST_SMALL;
    let (a, b) = synth_matrices(seed.as_bytes(), &params);
    let block_commitment = b"f1-harness-block";
    let nonce = b"f1-harness-nonce";

    // 1. Real ai-pow solve (difficulty_bits = 0 ⇒ always finds a tile).
    let t = Instant::now();
    let plain = mine(
        block_commitment,
        nonce,
        &a,
        &b,
        &params,
        ProverOptions::default(),
    )
    .expect("mine must not error")
    .expect("difficulty_bits=0 ⇒ a tile always clears");
    let mine_ms = t.elapsed().as_millis();

    // 2. Per-attempt context: chunk-Merkle commitments + nonce-bound κ.
    let ctx =
        BlockContext::build(block_commitment, nonce, &a, &b, &params).expect("BlockContext build");

    // Helpers: 32-byte → 8 LE u32 words.
    let words = |b: &[u8; 32]| -> [u32; 8] {
        core::array::from_fn(|i| {
            u32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]])
        })
    };

    // Build the composite trace exactly as the F1 bridge does:
    // matrix-hash A/B (C3) + key-pin rows for JOB_KEY=κ and
    // COMMITMENT_HASH=pow_key_for_nonce(s_a, nonce) (C1) + final
    // jackpot-hash block (C4: HASH_JACKPOT = BLAKE3(JACKPOT_MSG,
    // key=pow_key)). Encapsulated as a closure so the per-iteration prove
    // loop rebuilds an identical trace.
    let a_bytes: Vec<u8> = a.iter().map(|&v| v as u8).collect();
    let b_bytes: Vec<u8> = b.iter().map(|&v| v as u8).collect();
    let kappa_w = words(ctx.kappa());
    let pow_key = ctx.pow_key();
    let pow_key_w = words(&pow_key);
    let build_trace = || -> CompositeTrace {
        let mut tr = CompositeTrace::baseline_min();
        let h = tr.height();
        let (n1, _) = tr.place_matrix_hash_a(0, &a_bytes, ctx.kappa());
        let (mh_end, _) = tr.place_matrix_hash_b(n1, &b_bytes, ctx.kappa());
        tr.place_key_pin_row(mh_end + 1, false, &kappa_w); // JOB_KEY = κ
        tr.place_key_pin_row(mh_end + 2, true, &pow_key_w); // COMMITMENT_HASH = pow_key
        tr.place_jackpot_hash_block(h - 8, &[0u32; 16], &pow_key_w); // C4
        tr
    };

    let t = Instant::now();
    let trace = build_trace();
    let trace_gen_ms = t.elapsed().as_millis();

    // Derive PIs and assert the non-vacuous C1 + C3 bindings:
    // HASH_A/HASH_B == BlockContext chunk commitments, and
    // JOB_KEY/COMMITMENT_HASH == the real block's κ / bound pow_key.
    let pis = CompositePublicInputs::derive_from_trace(&trace);
    assert_eq!(
        pis.hash_a,
        words(ctx.h_a_chunk()),
        "C3: SNARK HASH_A PI must byte-equal BlockContext.h_a_chunk"
    );
    assert_eq!(
        pis.hash_b,
        words(ctx.h_b_chunk()),
        "C3: SNARK HASH_B PI must byte-equal BlockContext.h_b_chunk"
    );
    assert_eq!(
        pis.job_key, kappa_w,
        "C1: SNARK JOB_KEY PI must equal the block's κ"
    );
    assert_eq!(
        pis.commitment_hash, pow_key_w,
        "C1: SNARK COMMITMENT_HASH PI must equal the block's nonce-bound pow_key"
    );
    assert_ne!(
        pis.hash_jackpot, [0u32; 8],
        "C4: SNARK HASH_JACKPOT must be the non-vacuous keyed digest"
    );

    // 5. Prove + PoW-verify (C2: against the real difficulty target).
    let zk_params = ZkParams {
        m: params.m,
        k: params.k,
        n: params.n,
        noise_rank: params.noise_rank,
        tile: params.tile,
        difficulty_bits: params.difficulty_bits,
    };
    let cfg = build_config(&zk_params, &CircuitConfig::TEST_PEARL);
    let target = difficulty_target(&params);

    let mut prove_ms_total = 0u128;
    let mut verify_ms_total = 0u128;
    let mut proof_bytes = 0usize;
    for _ in 0..iters {
        let trace_i = build_trace();
        let t = Instant::now();
        let (proof, program) = composite_prove_pinned_logup(&cfg, trace_i, &pis);
        prove_ms_total += t.elapsed().as_millis();

        let t = Instant::now();
        composite_verify_pow_pinned_logup(&cfg, &program, &proof, &pis, &target)
            .expect("pinned+LogUp STARK valid + HASH_JACKPOT clears target");
        verify_ms_total += t.elapsed().as_millis();

        proof_bytes = bincode::serde::encode_to_vec(&proof, bincode::config::standard())
            .expect("encode")
            .len();
    }

    println!(
        "f1_harness shape=TEST_SMALL seed={seed} iters={iters} \
         mine_ms={mine_ms} trace_gen_ms={trace_gen_ms} \
         prove_ms={} verify_ms={} proof_bytes={proof_bytes} \
         num_pis={} c1_job_key_ok=true c1_commitment_ok=true \
         c3_hash_ab_ok=true c4_hash_jackpot_ok=true",
        prove_ms_total / iters as u128,
        verify_ms_total / iters as u128,
        pis.to_vec().len(),
    );
    let _ = &plain; // mine result kept alive for the solve assertion
}
