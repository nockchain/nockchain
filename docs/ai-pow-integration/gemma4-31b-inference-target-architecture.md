# Gemma 4 31B AI-PoW Target Architecture

## Decision

Use one native INT7 GEMM for the Gemma 4 MLP gate and up projections. Both
projections consume the same quantized activations, so their output channels can
be concatenated without changing inference:

$$
A_{T \times 5376}
\begin{bmatrix}B_{\mathrm{gate}} & B_{\mathrm{up}}\end{bmatrix}_{5376 \times 43008}
=
\begin{bmatrix}C_{\mathrm{gate}} & C_{\mathrm{up}}\end{bmatrix}_{T \times 43008}.
$$

The production mining profile is:

| Dimension | Value |
|---|---:|
| Rows (`m`) | 4,096 |
| Common dimension (`k`) | 5,376 |
| Columns (`n`) | 43,008 |
| Noise rank (`r`) | 128 |
| Ticket tile | 16 × 16 |
| Spot checks | 1 |
| Local difficulty bits | 0 |

This shape performs no padded MACs in the common or output dimensions. A batch
with fewer than 4,096 token rows pads only absent rows. Inference backpressure
favors full batches because mining throughput has priority over latency.

## Checkpoint contract

The target profile is the Pearl checkpoint `pearl-ai/Gemma-4-31B-it-pearl`:

- architecture `Gemma4ForConditionalGeneration`;
- text model type `gemma4_text`;
- 60 decoder layers;
- hidden size 5,376;
- intermediate size 21,504;
- Pearl mixed-precision quantization version `0.15.0.1`;
- INT7 per-channel gate and up weights;
- dynamic per-token INT7 gate and up activations;
- FP8 Q, K, V, and MLP down projections outside the mineable group.

For decoder layer `L`, the fused matrix concatenates these safetensors:

- `model.language_model.layers.L.mlp.gate_proj.weight`;
- `model.language_model.layers.L.mlp.up_proj.weight`.

Each tensor has type `I8` and shape `[21504, 5376]`. Safetensors row-major
`[out, in]` bytes are Pearl column-major `[in, out]` bytes without a transpose.
Gate columns precede up columns. The inference result splits at column 21,504,
and the per-channel scales concatenate in the same order.

`ai_pow_miner::gemma4::Gemma4Checkpoint` validates the model configuration and
bounded safetensors header before CUDA allocation. It rejects wrong tensor names,
shapes, ranges, or values outside `[-64, 64]`. The safetensors layout digest is a
runtime identity, not a weight commitment. Pearl V3 attempt commitments bind the
fused matrix bytes.

## Consensus boundary

The profile is `MatmulParams::GEMMA_4_31B_GATE_UP_FUSED`. It passes the production
parameter envelope:

- `128 | 5376`;
- `16·128 ≤ 5376 ≤ 4·128²`;
- `k/r = 42 ≤ STRIPE_MAX`;
- `16·16 = PEARL_HW_MAX`;
- `m` and `n` are divisible by the ticket side;
- the ticket count is 688,128, below the `u32` address limit.

The first and last dense tickets both select verifier setup key
`(trace_height=2^17, sx_bound=true)`. The production verifier table contains this
key. The existing dense `AIP1` artifact, shape-adjusted target, compact recursive
certificate, noun format, and Hoon interface remain unchanged.

Consensus commits to the fused INT7 matrix and its ticket. It does not identify
Gemma, a decoder layer, an inference request, or dequantization scales. Model
admission remains operator policy. A consensus model registry would require a
separate versioned protocol.

## CUDA kernel

The Gemma kernel is a separate compilation unit and C ABI. The RTX 5090 peak
kernel remains unchanged.

The CUDA object contains separate `sm_90a` and `sm_120a` code. The `sm_90a`
variant targets NVIDIA H100. The `sm_120a` variant targets RTX PRO 6000
Blackwell and RTX 5090. CUDA selects an exact code object for the active device;
the runtime does not depend on PTX just-in-time compilation. Each variant must
pass the same scalar differential, sanitizer, output, and consensus checks.
Throughput and launch geometry are measured and tuned for each device class.

The Gemma specialization uses:

- a 256 × 128 × 64 CTA;
- 16 × 16 signed INT8 Tensor Core tickets;
- `k=5376`, which is 84 K tiles;
- `r=128`, which is two K tiles per transcript cadence;
- 42 transcript cadences;
- the canonical 16-slot recurrence
  `M[s] = rotl13(M[s]) XOR x`, where `s = cadence mod 16`;
- device-side keyed BLAKE3 and little-endian target comparison;
- `atomicMin` on the canonical row-major ticket ordinal.

The device returns only the lowest winning ordinal and jackpot in the no-output
path. Rust reconstructs the offsets, recomputes the complete ticket with the
scalar implementation, checks the target again, and constructs the existing
compact certificate. A device mismatch is fatal for the selected backend.

The source session keeps the fused model weight and current activation matrix
resident. Each candidate-bound transcript rebuilds `kappa`, commitments, noise,
noised matrices, and ticket states. A header change never reuses attempt-bound
state.

## Noising and clean-output contract

Let `A` contain token rows and let `B` contain output-channel rows. Pearl noise
uses rank-128 factors:

```text
A' = A + E_AL × E_ARᵀ
B' = B + E_BR × E_BLᵀ
```

The mining accumulator is `D = A' × B'ᵀ`. The same execution derives two
low-rank intermediates:

```text
X = A  × E_BL
Y = B' × E_AR
```

The clean integer output is:

```text
A × Bᵀ = D - E_AL × Yᵀ - X × E_BRᵀ
```

All three products use signed INT8 inputs and INT32 accumulation. The
subtractions are exact INT32 operations. Each clean element is multiplied by
its per-token and per-channel FP32 scales and converted to BF16 with
round-to-nearest-even. This order is the cross-device output contract.

The output-capable kernel writes only logical token rows. It splits gate and up
at output column 21,504. The no-output mining specialization has no output
pointer, denoising work, scale loads, or output stores.

## Architecture backends

The runtime selects an exact backend from the CUDA compute capability:

| Device class | Code object | Main inference GEMM |
|---|---|---|
| NVIDIA H100 | `sm_90a` | WGMMA and TMA |
| RTX PRO 6000 Blackwell | `sm_120a` | Blackwell Tensor Core MMA |
| RTX 5090 | `sm_120a` | Blackwell Tensor Core MMA |

Both backends implement the same noising, transcript, clean-output, scaling,
and winner contract. Unsupported capabilities fail before model serving. The
runtime does not JIT a PTX fallback.

Only the fused gate/up projection is consensus mining work. Other INT7 linear
layers use an inference-only INT8 GEMM and do not create work notifications.

## Execution planes

The inference process owns tokenization, transformer state, KV cache, scales,
activation quantization, activation functions, and model scheduling. It loads
the native kernel in-process and passes contiguous tensor pointers on the
current PyTorch CUDA stream. Device matrices never enter protobuf messages.

The miner service owns the Nockchain candidate, target, scheduler generation,
scalar winner validation, proof, and submission. The local control channel
carries bounded candidate and lifecycle metadata.

An immutable operand generation contains:

- checkpoint layout digest;
- decoder layer;
- logical and padded token counts;
- fused gate/up weight identity;
- activation and scale pointers;
- tensor-parallel rank and global column range;
- monotonically increasing generation number.

Candidate replacement and operand replacement are independent generations.
Existing stale-candidate checks remain authoritative for block submission.

## RTX 5090 tensor parallelism

Two RTX 5090 devices hold contiguous output-column shards. Each rank computes
its local BF16 output and searches only its global column interval.

Both ranks derive one canonical work statement:

1. Hash local contiguous `B` chunks with their global BLAKE3 chunk counters.
2. Merge the ordered chunk chaining values into the canonical full-`B` root.
3. Broadcast the full commitments and derive one pair of noise seeds with
   global `n=43008`.
4. Generate `E_BR` with the rank's global output-column offset.
5. Convert local winners to global row-major ordinals.
6. Reduce the lowest global winner across ranks before scalar validation.

No rank-local commitment, local `n`, or local ordinal can enter a certificate.

## Mining-performance rule

The existing peak kernel is the regression baseline and remains available under
its current selector. The Gemma target ships only if:

- the peak kernel's binary behavior and sustained throughput do not regress;
- Gemma mining-only throughput is measured as complete-ticket TMAC/s;
- inference-loaded Gemma throughput remains within the accepted non-regression
  band or uses an additional output device;
- no-hit search performs no allocation;
- launch and candidate-cancellation latency stay bounded;
- every device winner matches the scalar Rust oracle.

The fused Gemma sweep performs 947,040,288,768 useful MACs, 86.1% of the current
peak sweep, with no common/output zero padding. Normalized TMAC/s, not raw ticket
count, is the performance comparison.

## RTX 5090 mining evidence

Hardware validation uses one RTX 5090 (`sm_120`, 170 SMs, 32,607 MiB),
driver 580.126.09, CUDA 12.8.93, and a 600 W power limit.

The kernel uses 230 registers per thread, one active 256-thread CTA per SM,
8,192 bytes of static shared memory, and 49,152 bytes of dynamic shared memory.
`cuobjdump` reports zero stack and zero local memory for the search, debug,
commitment, seed, noise, and noising kernels.

The scalar/device differential passes 1,000 deterministic tickets, including
first, last, CTA-boundary, exact-target, and predecessor-target cases. The source
session matches CPU commitments, noise, tile state, and jackpot across four
candidate extranonces. `memcheck`, `racecheck`, `initcheck`, and `synccheck`
report zero findings.

After 100 warmup launches, 20,000 measured full-grid launches produce:

| Kernel | Median launch | Tickets/s | Complete-ticket TMAC/s |
|---|---:|---:|---:|
| Gemma fused gate/up | 2.825 ms | 243,544,431 | 335.179 |
| Existing peak baseline | 3.206 ms | 163,550,520 | 342.990 |

The isolated Gemma kernel retains 97.72% of the peak kernel's normalized mining
rate while every common/output MAC remains useful inference. The separate
compilation unit leaves the existing peak source and ABI unchanged. Inference
output materialization remains outside this mining-only measurement.

## H100 and RTX PRO 6000 mining evidence

The exact-SASS image contains `sm_90a` and `sm_120a` code objects. Validation
uses driver 580.159.04 on one 310 W H100 PCIe with 114 SMs and one 600 W RTX
PRO 6000 Blackwell Server Edition with 188 SMs.

After 100 warmup launches, 20 measured full-grid launches produce:

| Device | Median launch | Tickets/s | Complete-ticket TMAC/s |
|---|---:|---:|---:|
| H100 PCIe | 4.836 ms | 142,294,215 | 195.833 |
| RTX PRO 6000 Blackwell | 2.807 ms | 245,136,655 | 337.371 |

Both code objects pass the 1,000-ticket scalar differential and the complete
source-session transcript differential. `memcheck`, `racecheck`, `initcheck`,
and `synccheck` report zero findings on both devices.

The H100 model-serving path returns deterministic Gemma 4 output while the
idle miner uses the same `sm_90a` consensus kernel. Its idle mining rate returns
to 99.28% of the pre-request rate after eight inference requests. The common
warp-MMA kernel is the H100 correctness baseline; an H100 WGMMA specialization
is required before its throughput is final.

Pearl GEMM uses Hopper WGMMA and TMA kernels for model-serving matrix
multiplication. Compiling those kernels as `sm_120a` is not valid: the first
noising launch fails on RTX 5090. Blackwell model serving therefore requires a
native `sm_120a` inference GEMM path. The consensus mining code object itself
works on RTX 5090 and RTX PRO 6000 Blackwell.

## Failure behavior

- A checkpoint mismatch fails before CUDA allocation.
- A non-INT7 activation fails before template preparation.
- An unsupported device or geometry fails without CPU fallback.
- A transcript, tile-state, jackpot, or winner mismatch disables the Gemma
  backend.
- A new candidate cancels and drains stale work before proof construction.
- A recursive certificate is built only after a scalar-validated target hit.

## Hardware validation gate

Runpod validation records:

1. GPU model, topology, driver, CUDA Toolkit, and compiler flags;
2. kernel registers, shared memory, occupancy, stack, and spills;
3. first, last, CTA-boundary, and randomized scalar/device differentials;
4. all 16 transcript words and jackpot bytes across the 42-cadence recurrence;
5. maximum-target, zero-target, adjacent-range, and lowest-winner behavior;
6. `memcheck`, `racecheck`, `initcheck`, and `synccheck`;
7. 60-second peak and Gemma mining-only TMAC/s;
8. inference-output equality and mining-loaded throughput;
9. compact certificate verification through the existing consensus entry point.

Correctness failures stop performance work. Runpod model-serving tuning starts
only after byte equality and the peak non-regression gate pass.
