# Pearl V3 RTX 5090 Kernel Implementation Plan

## Scope

Add an opt-in dense search path named `peak`. Keep the existing generic and canonical V3 CUDA paths byte-for-byte unchanged until the peak path passes every gate.

The implementation uses the architecture in `pearl-v3-rtx5090-architecture.md` and the limits in `pearl-v3-rtx5090-roofline.md`.

## Stage 1: Independent kernel and harness

Add new files only:

- `crates/ai-pow-miner-cuda/csrc/ai_pow_v3_peak.cu`
- `crates/ai-pow-miner-cuda/csrc/ai_pow_v3_peak.h`
- `crates/ai-pow-miner-cuda/csrc/test_ai_pow_v3_peak.cu`

The kernel starts from the measured Pearl $256 \times 128 \times 64$ Tensor Core main loop. Remove Pearl process state, Gateway state, and nonce-generation code. Retain only full-grid GEMM, transcript construction, keyed BLAKE3, target comparison, and lowest-ordinal selection.

The harness must:

1. generate deterministic patterned `A'` and `B'`;
2. run a small shape that covers more than one CTA tile;
3. compute every ticket with an independent scalar oracle;
4. compare all 16 transcript words and jackpot bytes for selected tiles;
5. check maximum and zero targets;
6. repeat each vector three times.

Gate: the standalone CUDA executable prints exact equality for every vector. No Rust integration starts before this gate passes.

## Stage 2: Stable C ABI and persistent session

Add an opaque `AiPowCudaPeakSession` API:

- create from device ordinal, geometry, matrices, and `s_A`;
- search an ordinal range and target;
- capture one ticket transcript for tests;
- return timing counters for benchmarks;
- destroy all owned resources.

Validate all lengths and supported geometry before allocation. Use checked `size_t` products. Return CUDA status codes without process exit.

Gate: adjacent searches on one session, template replacement, maximum target, zero target, and boundary ordinals all match the standalone oracle. The no-hit path performs no allocation.

## Stage 3: Rust differential oracle

Expose immutable noised matrix slices from `PreparedPearlPatternJob`. This is an additive read-only API.

Add `PeakGpuSearchBackend` beside `GpuSearchBackend`. It accepts only the supported dense geometry. It never handles the canonical MoE path and never falls back to CPU search.

Add focused tests that compare:

- ordinal-to-offset mapping;
- all 16 `TileState` words;
- keyed jackpot bytes;
- lowest-winner selection;
- adjacent batch boundaries;
- session reuse and replacement;
- scalar rejection of a corrupted device result.

Gate: Rust scalar/device differential passes on RTX 5090 for 1,000 deterministic tickets, including first, last, and CTA-boundary tiles.

## Stage 4: Safety and determinism

Run these tools on the focused CUDA harness and Rust differential:

1. Compute Sanitizer `memcheck`;
2. Compute Sanitizer `racecheck`;
3. Compute Sanitizer `initcheck`;
4. Compute Sanitizer `synccheck`;
5. `cuobjdump -res-usage` or the equivalent `ptxas` report.

Gate:

- no sanitizer findings;
- three identical transcript sweeps;
- zero local stack and zero spills;
- at least two resident CTAs per SM;
- no out-of-range winner under adversarial targets.

## Stage 5: RTX 5090 shape and topology sweep

Build `sm_120` variants for:

- `m`: 4,096; 8,192; 16,384; 32,768;
- `n`: 32,768 and 57,344 where memory permits;
- CTA: $128 \times 128$ and $256 \times 128$;
- stages: 2 and 3;
- fixed `k=8192`, `r=512`, and tile 16.

For each valid variant, measure:

- matrix preparation and upload time;
- kernel time;
- total search wall time;
- raw GEMM TOPS;
- complete ticket TMAC/s and tickets/s;
- finalizer share;
- power, clock, temperature, registers, stack, and occupancy.

Select the smallest shape within 98% of the best complete-ticket rate. Reject a faster shape if one launch exceeds 100 ms.

Gate: at least 600 sustained TOPS, 300 TMAC/s, 140 million tickets/s, and 80% of same-session raw GEMM.

## Stage 6: Opt-in miner integration

Add `peak` as a separate backend selector in the CUDA miner CLI and Docker entrypoint. It must require the peak dense profile and must conflict with the existing canonical V3 selector.

The worker keeps the current order:

1. search;
2. scalar recheck;
3. target recheck;
4. recursive proof;
5. artifact encoding;
6. node submission.

Gate: one GPU winner builds and verifies the existing compact recursive certificate. The certificate and noun wire contain no CUDA-specific field.

## Stage 7: Runpod production flow

Build a CUDA 12.8 or newer Linux/amd64 image for `sm_120`. Start one RTX 5090 pod with only the documented miner arguments.

Verify:

1. GPU enumeration and peak-backend startup;
2. persistent template allocation;
3. steady no-hit mining and progress accounting;
4. fakenet node connection;
5. accepted `%ai-pow` block;
6. candidate replacement during a launch;
7. full pod stop and restart with the same configuration;
8. no CPU-search fallback after an injected CUDA failure.

Gate: the node logs an accepted block before and after the restart.

## Stage 8: Multi-GPU extension

Partition each ordered batch into contiguous device ranges. Preserve one session and stream per device. Reduce device results by global ordinal.

Gate on two, four, and eight devices:

- exact range coverage with no gaps or overlaps;
- global lowest winner under maximum target;
- zero-target miss;
- cancellation after a candidate replacement;
- scalar recheck of the selected global winner;
- throughput scaling recorded against one device.

## Stop rules

Stop performance work and correct the first failure when any condition occurs:

- one transcript word differs;
- one jackpot byte differs;
- the winner is not the lowest ordinal;
- a sanitizer reports an error;
- the hot kernel spills;
- an unsupported shape falls back to another backend;
- proof or node verification rejects a scalar-valid winner.

Do not tune around a failed correctness gate.

## Final evidence

Record the selected shape, compile flags, measured table, sanitizer results, scalar differential count, proof verification, accepted block timestamp, and image digest in the GPU miner goal document. Commit each validated stage separately.
