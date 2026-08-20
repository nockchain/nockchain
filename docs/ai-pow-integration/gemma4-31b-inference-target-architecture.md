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

## Inference output

Mining accumulation uses noised matrices. Inference needs the clean fused output.
The output-capable path is a separate kernel symbol:

1. store noised accumulators for the logical token rows;
2. reconstruct the clean `A × B` result with the Pearl low-rank factors;
3. apply concatenated per-token and per-channel scales;
4. split gate and up outputs at column 21,504.

The normal no-output mining kernel has no output pointer, inference branch,
additional allocation, or additional synchronization. Output work may run on a
separate target device if it reduces mining throughput on the primary device.

## Execution planes

The inference process owns tokenization, transformer state, KV cache, scales,
activation quantization, activation functions, and model scheduling.

The miner owns the Nockchain candidate, target, fused INT7 operands, Pearl
transcript, noise, ticket search, scalar winner validation, proof, and submission.

Inference and mining communicate through a versioned local control channel.
Large same-host tensors use bounded CUDA IPC handles where peer access is
available. A pending activation generation never interrupts an active search.
Queue saturation delays inference rather than mining.

An immutable operand generation contains:

- checkpoint layout digest;
- decoder layer;
- logical token count;
- fused gate/up weight identity;
- activation and scale handles;
- monotonically increasing generation number.

Candidate replacement and operand replacement are independent generations.
Existing stale-candidate checks remain authoritative for block submission.

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
