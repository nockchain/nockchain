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
bounded safetensors header. The bridge then hashes every `model.safetensors`
byte with SHA-256 and compares the result with the compiled checkpoint pin
before CUDA initialization. Wrong tensor metadata or weight content fails
startup. Pearl V3 attempt commitments separately bind each fused matrix.

## Consensus boundary

The miner-owned profile is `ai_pow_miner::gemma4::GEMMA4_NATIVE_PARAMS`. It
passes the unchanged production parameter envelope:

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
current PyTorch CUDA stream. A no-hit inference call sends only bounded
lifecycle metadata over gRPC. No activation, weight, or output tensor leaves
the device on the normal path.

The miner service owns the Nockchain candidate, target, scheduler generation,
scalar winner validation, proof, and submission. A target hit copies the
opened INT7 activation and fused weight matrices to host memory and streams
them in bounded chunks. This rare proof handoff does not affect no-hit
inference or mining throughput.

Each inference runtime has a five-second lease. Its heartbeat declares the
runtime's active work IDs. The bridge removes server work that is absent from
the heartbeat, so a lost completion response cannot pause idle mining. Lease
expiry deregisters the runtime, removes its remaining work, and wakes idle
mining. Status reports live ownership and both cleanup counters.

An immutable operand generation contains:

- checkpoint content digest;
- decoder layer;
- logical and padded token counts;
- fused gate/up weight identity;
- activation and scale pointers;
- tensor-parallel rank and global column range;
- monotonically increasing generation number.

Candidate replacement and operand replacement are independent generations.
Existing stale-candidate checks remain authoritative for block submission.

## Candidate, proof, and submission path

The production bridge connects to the node's private gRPC endpoint:

1. Set the reward public-key hash.
2. Subscribe to `%mine-ai` before mining is enabled.
3. Decode the version-4 candidate commitment, target, and `pow-len`.
4. Publish a new candidate generation and its canonical dense Pearl header to
   vLLM.
5. Invalidate the generation and install a zero target if the candidate stream
   disconnects.

The bridge adjusts the raw node target with its canonical Pearl mining
configuration. vLLM consumes that effective target and the serialized mining
configuration without local derivation for device comparison. A winner
submission contains the candidate generation,
header extranonce, opened row and column tile, rank-128 noise seeds, and the
full INT7 `A` and `Bᵀ` matrices. The bridge requires:

- `m` to be a nonzero multiple of 256 and no greater than 4,096;
- `k=5,376`, `n=43,008`, `r=128`, and a 16 × 16 contiguous tile;
- exact tensor byte counts and values in `[-64, 63]`;
- no routing tensor;
- a current candidate generation and a registered runtime.

The proof worker reconstructs the canonical header from the active Nockchain
commitment and submitted extranonce. It recomputes the complete scalar
transcript, both noise seeds, the selected tile, jackpot, and adjusted target
check. It then builds the compact recursive certificate and canonical
`[%command %pow %ai-pow ...]` noun. The node submission result controls the
gRPC response: `accepted=true` means that the node acknowledged the prepared
poke.

Only one recursive inference-winner proof can run at a time. Candidate
replacement does not interrupt a recursive prover. The completed result is
discarded unless its generation is still current. This prevents stale block
submission without detaching an unbounded proof task.

The idle CUDA session uses the same node job. It varies the canonical header
timestamp as an extranonce, applies the same shape-adjusted target, and sends a
winning witness through the same scalar, recursive, and submission path.

## RTX 5090 tensor parallelism

Two RTX 5090 devices hold contiguous output-channel shards. Each rank computes
its local BF16 output. Only rank zero executes the complete mineable
projection.

The ranks establish one canonical work statement:

1. All ranks contribute their local fused gate/up weight and scale shards.
2. The runtime restores canonical `[gate, up]` row order and caches the full
   matrix after the collective.
3. Rank zero derives the full-matrix commitments and noise, searches every
   global ticket, and converts a winner to its canonical row-major ordinal.
4. Rank zero returns its local gate/up output slice from the clean full-matrix
   result.
5. Each follower rank uses its cached local `(K, N)` weight layout for an
   inference-only INT8 GEMM.

Follower ranks do not emit work notifications, derive mining state, search
tickets, or submit winners. No rank-local commitment, local `n`, or local
ordinal can enter a certificate.

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

## Multi-device full-flow evidence

The runtime contains exact `sm_90a` and `sm_120a` code objects. The native
in-process API consumes the real quantized vLLM activation and weight pointers
on the active PyTorch CUDA stream. Its output is bit-equal to direct INT32 GEMM,
FP32 scale multiplication, and BF16 round-to-nearest-even on every tested
device.

After 100 warmup launches, 20 measured full-grid mining launches produce:

| Device | Median launch | Complete-ticket TMAC/s |
|---|---:|---:|
| H100 SXM, 132 SMs | 3.593 ms | 263.558 |
| RTX PRO 6000 Blackwell, 188 SMs | 2.654 ms | 356.902 |
| RTX 5090 device 0, 170 SMs | 2.815 ms | 336.380 |
| RTX 5090 device 1, 170 SMs | 2.838 ms | 333.713 |

The complete candidate preparation, mining GEMM, exact denoising, scaling, and
device-output path produces:

| Device | Tokens | Wall median | Mining GEMM | Clean output |
|---|---:|---:|---:|---:|
| H100 SXM | 256 | 8.986 ms | 0.339 ms | 4.579 ms |
| H100 SXM | 4,096 | 81.959 ms | 4.337 ms | 73.117 ms |
| RTX PRO 6000 Blackwell | 256 | 5.875 ms | 0.250 ms | 2.809 ms |
| RTX PRO 6000 Blackwell | 4,096 | 50.400 ms | 3.153 ms | 44.287 ms |
| RTX 5090 | 256 | 5.261 ms | 0.223 ms | 2.464 ms |
| RTX 5090 | 4,096 | 44.763 ms | 2.920 ms | 39.005 ms |

H100 and RTX PRO 6000 serve the model on one GPU. Two RTX 5090 devices gather
the selected gate/up weight shards in rank order and restore canonical
`[gate, up]` row order. Rank zero executes the full mining statement and
returns its local output shard. The follower executes only its local
inference projection.

Each device class passes the 1,000-ticket scalar differential, the complete
source-session transcript differential, and exact clean-output comparison.
`memcheck`, `racecheck`, `initcheck`, and `synccheck` report zero findings for
the device-pointer rebind and output path on all three classes.

At temperature zero, H100, RTX PRO 6000, and dual RTX 5090 return identical
responses across repeated exact-string, arithmetic, and factual OpenAI chat
requests.

## Production proof-flow evidence

The forced-hit bridge KAT starts a private mock NockApp node and the real
inference gRPC service. It publishes a version-4 `%mine-ai` candidate, runs the
native production-shape search, streams the 256 × 5,376 activation and
43,008 × 5,376 fused weight, reconstructs the scalar winner, builds the compact
recursive certificate, and requires a node-acknowledged canonical poke.

| Host | CUDA build | End-to-end KAT |
|---|---:|---:|
| H100 SXM | 13.0, `sm_90a` | 31.39 s |
| RTX PRO 6000 Blackwell | 13.0, `sm_120a` | 31.20 s |
| Dual RTX 5090 host, rank-zero full-matrix path | 12.8, `sm_120a` | 20.65 s |

The production image builds the same source with CUDA 12.9 and exact `sm_90a`
and `sm_120a` code objects. RTX 5090 requires a native driver compatible with
this toolkit; driver 570 returns status 804. The CUDA forward-compatibility
package does not support GeForce devices.

`compute-sanitizer --tool memcheck` reports zero errors for the complete source
transcript differential on H100 and RTX PRO 6000. The H100 full-grid
mining-only regression measures 262.028 complete-ticket TMAC/s, within 0.6% of
the 263.558 TMAC/s reference measurement.

## Failure behavior

- A checkpoint mismatch fails before CUDA allocation.
- A non-INT7 activation fails before template preparation.
- An unsupported device or geometry fails without CPU fallback.
- A transcript, tile-state, jackpot, or winner mismatch disables the Gemma
  backend.
- A candidate replacement rejects stale witnesses and discards an in-flight
  proof result before node submission.
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
