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
- one active CTA per SM with a two-CTA-per-SM launch grid;
- no out-of-range winner under adversarial targets.

Runpod validation has zero findings from `memcheck`, `racecheck`, `initcheck`,
and `synccheck` on adjacent range searches across persistent CTA tiles. The
`sm_120` kernel has 248 registers per thread, no stack frame, and no spills.

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

### Stage 5 evidence

The selected shape is $4096 \times 32768 \times 8192$ with rank 512, a
$256 \times 128 \times 64$ CTA, and two pipeline stages. After five seconds of
warmup, the median of 21 searches is:

- 3.158 ms kernel time;
- 3.184 ms wall time;
- 166.025 million complete tickets/s;
- 348.179 TMAC/s;
- 696.359 TOPS.

A 60-second power-limited run produces 334.740 TMAC/s and 159.617 million
tickets/s. The matching raw GEMM is 368.1 TMAC/s. The complete search keeps
94.6% of the five-second raw rate and 90.9% under sustained power load.

The 1,000-ticket Rust differential compares all transcript words from the
device debug kernel and brackets every fused search hash with its exact target
and little-endian predecessor. Each vector runs three fused device repetitions.
First, last, CTA-boundary, maximum-target, zero-target, and adjacent range cases
pass.

## Stage 6: Production pipeline integration

The production GPU path uses the dense `AIP1` artifact. One synthetic Pearl
header binds the Nockchain block commitment and one extranonce. Its complete
tile grid is one prepared template. A miss advances the extranonce and creates
a new template. The host keeps the winning template until proof construction
finishes.

The worker uses this order:

1. derive the header, commitments, noise seeds, and noised matrices;
2. search every valid dense tile on the selected GPUs;
3. select the lowest global tile ordinal;
4. recompute the winner with the scalar evaluator;
5. check the Nockchain target again;
6. build and verify the compact recursive certificate;
7. encode the existing dense `AIP1` artifact;
8. submit the `%ai-pow` block.

The certificate binds the header, configuration, matrix commitments, winning
row and column strips, jackpot, and Nockchain auxiliary commitment. CUDA state
does not enter the certificate or noun wire.

The production `--gpu --canonical` mode selects the peak backend. The Pearl
Gateway mode keeps its existing generic backend because Gateway work controls
the puzzle shape. CPU canonical mode remains available for diagnosis.

Gate: one peak GPU winner builds a compact recursive certificate that the
production verifier accepts.

The profile satisfies the consensus parameter envelope. Its first and last
dense tiles both require a $2^{17}$ Layer-0 trace. The production verifier table
contains the matching `sx_bound=true` setup key.

## Stage 7: Prepared-template throughput

Measure the complete template cycle. Include header derivation, keyed matrix
commitments, noise expansion, noised matrix construction, device transfer, and
the search kernel. Repeated searches of one prepared template are not a valid
production throughput measurement.

Move transcript preparation to the GPU when host preparation or PCIe transfer
keeps the complete cycle below 80% of the measured search-kernel rate. Keep the
original matrices resident on each device. Derive the keyed commitments, noise
factors, and noised matrices for each new header on the device. Return the
commitments and noise seeds that the host needs for scalar validation and proof
construction.

Gate: the production miner reports complete-template ticket and TMAC rates for
at least 60 seconds.

## Stage 8: Multi-GPU production backend

Each device owns one session and stream. All devices prepare the same template.
For each ordered batch, partition the tile ordinals into adjacent, disjoint
ranges. Each device returns its local minimum. The host waits for every active
device and returns the lowest global ordinal.

A device initialization, preparation, launch, synchronization, or result error
is fatal. The backend does not retry the range on the CPU or on another GPU.
Candidate replacement stops new launches, waits for the current launches, and
destroys every stale session before it prepares the next template.

Gate on every available device count:

- exact range coverage with no gaps or overlaps;
- global lowest winner with the maximum target;
- no winner with the zero target;
- scalar recheck of the selected global winner;
- cancellation after candidate replacement;
- throughput scaling against one device.

## Stage 9: Runpod production flow

Build a CUDA 12.8 or newer Linux/amd64 image for `sm_120`. Start RTX 5090 pods
with only `NODE_ADDR=http://23.252.122.18:5556` and the production mining key.
Do not start or connect to a fakenet.

Verify:

1. GPU enumeration and peak-backend startup;
2. one persistent session per selected device;
3. complete-template progress accounting;
4. connection to the mainnet API node;
5. an accepted `%ai-pow` block in the node logs;
6. candidate replacement during a launch;
7. a full container stop and restart with the same configuration;
8. no CPU fallback after an injected CUDA failure.

Gate: the node accepts a block before and after the restart.

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
