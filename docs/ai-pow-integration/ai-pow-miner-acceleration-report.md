# AI-PoW Miner Acceleration Report

## Status

`ai-pow-miner` now has a prepared, parallel CPU ticket-search path for both canonical MoE and dense Pearl Gateway work. CPU remains the default search backend and the authority for ticket validation and recursive-certificate handoff.

CUDA support is isolated but not implemented as a usable search backend. `ai-pow-miner-cuda` owns the CUDA process context, stream, device selection, `sm_80` capability policy, and shared command-line surface. Its backend returns an explicit unavailable error because no device algorithms or validated kernels have been admitted. GPU support must not be advertised until the device pipeline passes the hardware gates below.

## Goal and non-negotiable invariants

The work accelerates ticket search without changing the active Pearl-compatible transcript, target semantics, cancellation, stale-work handling, noun wire format, consensus behavior, or recursive-certificate construction.

- A canonical attempt recomputes every extranonce-dependent transcript value, including `kappa`, matrix commitments, noise seeds, noised strips, tile state, and jackpot. It never rehashes a previous ticket or reuses transcript-dependent data across extranonces.
- A dense Pearl job prepares only fixed-header invariants once. Every backend result is reconstructed with `evaluate_pearl_merge_checked_ticket_attempt` before Gateway submission or recursive-proof construction.
- A canonical backend result is recomputed with the checked scalar oracle before `prove_canonical_moe_block_at_for_miner` runs.
- Batch search returns the lowest successful input ordinal. Attempt accounting, bounds, deadlines, cancellation, and stale-generation invalidation apply before and after every bounded batch.
- A backend may lose hashrate or report an error; it may not cause an invalid submission. There is no silent CPU fallback for a requested CUDA backend.
- CUDA dependencies remain outside the root workspace. Normal CPU users and CI do not require an NVIDIA driver, CUDA Toolkit, nightly CUDA codegen, or `libcuda`.

## Execution plan

### Stage 0 — byte-exact baseline

Freeze canonical and dense Pearl search vectors that record inputs, transcript commitments, opened/noised strips, stripe state, jackpot, adjusted targets, and winner ordinal. Cross-check them against the scalar implementation, Pearl reference behavior, and applicable upstream fixtures. Measure release throughput and allocation counts before optimization.

### Stage 1 — prepared evaluators

Separate proof-producing traces from search-only tile-state evaluation. Prepare dense fixed-header state once. Prepare canonical proof-independent state once while retaining mandatory per-extranonce transcript derivation. Preserve public oracle and prover wrappers as compatibility surfaces.

### Stage 2 — deterministic parallel CPU search

Use a miner-owned `SearchBackend` and an ordered batch scheduler. Give a dedicated CPU worker pool persistent scratch. Parallelize tickets rather than hash-tree internals. Recheck every backend winner with the scalar checked oracle before proof or submission.

### Stage 3 — CUDA isolation

Keep `crates/ai-pow-miner-cuda` explicitly excluded from the root workspace. Share node-gated CLI parsing with the CPU binary. Target portable `sm_80` PTX and reject devices below compute capability 8.0. Keep one persistent CUDA context, stream, allocation set, and work-fingerprint-keyed session per process.

### Stage 4 — canonical MoE CUDA

First implement a scalar device kernel that emits every canonical KAT intermediate. Then replace only the tile inner loop with signed INT8 MMA while preserving wrapping `i32` accumulation and the 64 logical tile cells. Device-side winner reduction returns only the lowest candidate; host scalar recomputation remains mandatory.

### Stage 5 — dense Pearl CUDA

Upload prepared dense-job invariants once per fixed Gateway header. Assign one CTA to each ticket ordinal, reduce to the lowest hit, reconstruct the exact checked ticket on CPU, classify the hit, and discard stale session output before any submission.

### Stage 6 — hardware validation and end-to-end proof

Run scalar/prepared/parallel CPU/scalar CUDA/MMA CUDA differentials, randomized admitted-shape coverage, all CUDA sanitizer modes, release throughput measurements, canonical fakenet acceptance, and dense Gateway stale-work scenarios. CUDA support is complete only after a real NVIDIA host accepts GPU-found and CPU-rechecked work end to end.

## Current CPU evidence

Release benchmark on this Apple M2 Max, using `RUSTFLAGS='-C target-cpu=native' cargo bench -p ai-pow-miner --bench search --features node`:

| Workload | Workers | Attempts | Throughput | Allocations | Speedup over prepared scalar |
|---|---:|---:|---:|---:|---:|
| Canonical prepared scalar | 1 | 256 | 3,517.369 attempts/s | 0 | 1.00x |
| Canonical prepared dedicated backend | 12 | 256 | 27,219.202 attempts/s | 2 | 7.74x |
| Dense prepared scalar full sweep | 1 | 4,096 | 394,514.743 attempts/s | 0 | 1.00x |
| Dense prepared dedicated backend full sweep | 12 | 4,096 | 2,674,065.611 attempts/s | 2 | 6.78x |

The two allocations in each dedicated-backend measurement are batch-level synchronization overhead, not per-ticket evaluator allocation. The prepared scalar evaluator itself measured zero allocations for both workloads.

Additional checks passed:

```text
RUSTFLAGS='-C target-cpu=native' cargo test -p ai-pow-miner --features node
162 passed; 9 ignored

cargo fmt --check -p ai-pow-miner
cargo +nightly-2026-04-03 fmt --manifest-path crates/ai-pow-miner-cuda/Cargo.toml --check
```

The root `Cargo.lock` has no `cuda-oxide`, `cuda-core`, `cuda-device`, or `cuda-bindings` entry.

## CUDA prerequisite and current blocker

CUDA compilation was attempted through the isolated package. `cuda-bindings` stopped before compiling package source because this Apple Silicon host has no CUDA Toolkit installation and therefore no `cuda.h`; it also has no NVIDIA device. This is an environmental blocker, not evidence that a CUDA kernel is correct.

A continuation host requires all of the following:

- Linux with an NVIDIA `sm_80` or newer GPU.
- CUDA Toolkit 12.x or newer, with `cuda.h`, `nvcc`, and a driver compatible with the selected GPU.
- The pinned `nightly-2026-04-03` toolchain with `rust-src`, `rustc-dev`, and `llvm-tools`.
- Clang/libclang and `cargo oxide` required by the pinned cuda-oxide revision.

The isolated package defaults to portable `sm_80` PTX in `crates/ai-pow-miner-cuda/.cargo/cuda-oxide.toml`. The driver may JIT that PTX only on later compatible NVIDIA devices; it does not make non-NVIDIA or pre-Ampere hardware supported.

## Proposed next steps

1. Provision the required CUDA validation host and record GPU model, driver, Toolkit, cuda-oxide revision, and compiler-toolchain versions with the validation output.
2. Build the isolated CUDA package for `sm_80`. If cuda-oxide fails, reduce the fault to a minimal reproducer in `../cuda-oxide`; do not introduce unsafe PTX or alter the pinned revision without a validated fix.
3. Port and KAT-test device BLAKE3 compression/tree reduction, noise PRNG, wrapping INT8 accumulation, fold, target comparison, and atomic minimum-winner selection. Compare raw little-endian bytes for every intermediate.
4. Implement the scalar canonical CUDA KAT kernel before any MMA optimization. Differential it against the frozen canonical vectors and the CPU prepared evaluator.
5. Add signed INT8 MMA only to the canonical tile inner loop. Preserve exact wrapping and fold semantics; retain the scalar device kernel as a test oracle only.
6. Implement dense CUDA from the prepared dense job, including fingerprint-keyed uploads, attempt-range caps, stale-output invalidation, checked CPU reconstruction, and hit classification.
7. Run `memcheck`, `racecheck`, `initcheck`, and `synccheck`; then measure canonical and dense performance on one `sm_80` GPU and one Ada-or-newer GPU. Set `--gpu-batch-attempts` from a measured sub-100 ms launch bound on the slowest supported validation device.
8. Run the canonical fakenet proof-acceptance flow and deterministic Gateway stale-work flow. Record accepted block and submission logs, KAT counts, sanitizer results, throughput, speedups, transfer share, and cancellation latency.

## Source ownership

- `crates/ai-pow/src/matmul.rs` defines tile-state recurrence and wrapping behavior.
- `crates/ai-pow/src/pearl_compat.rs`, `prng.rs`, and `commit.rs` define the byte-exact transcript and device-port inputs.
- `crates/ai-pow-miner/src/canonical.rs` owns canonical preparation, scalar validation, and the proof handoff.
- `crates/ai-pow-miner/src/pearl_mining.rs` owns dense attempt ordering, accounting, cancellation, and CPU-default behavior.
- `crates/ai-pow-miner/src/search.rs` defines the backend and scheduler contract.
- `crates/ai-pow-miner/src/run.rs` injects the backend into the node driver and preserves generation safety.
- `crates/ai-pow-miner-cuda/` owns the isolated CUDA CLI, runtime session, future device modules, and GPU-only tests.
