# Gemma 4 31B AI-PoW Target Architecture

## Decision

Use Gemma 4 inference operands as the source matrices for the existing dense peak work profile. Keep the consensus statement, proof, target adjustment, certificate, and noun format unchanged.

The physical work shape stays fixed:

| Dimension | Value |
|---|---:|
| Rows (`m`) | 4,096 |
| Common dimension (`k`) | 8,192 |
| Columns (`n`) | 32,768 |
| Noise rank (`r`) | 512 |
| Ticket tile | 16 × 16 |

The selected Gemma 4 MLP operation has this logical INT7 shape:

$$A_{T\times 5376} B_{5376\times 21504} = C_{T\times 21504}, \quad 1 \le T \le 4096.$$

The target pads `A` with zero columns and zero rows. It pads each column of `B` with zero common-dimension values and adds zero output columns. The padded matrices have the existing peak shape. The logical top-left result is unchanged because every added term is zero.

This design keeps the measured peak CUDA geometry. A native `5376 × 21504` kernel would change the common-dimension cadence, transcript recurrence, launch geometry, and Tensor Core utilization. It would need a new performance program and could reduce mining rate.

## Checkpoint contract

The target profile is the Pearl checkpoint `pearl-ai/Gemma-4-31B-it-pearl` with these properties:

- architecture: `Gemma4ForConditionalGeneration`;
- text model type: `gemma4_text`;
- 60 decoder layers;
- hidden size: 5,376;
- intermediate size: 21,504;
- no MoE block;
- Pearl mixed-precision quantization version `0.15.0.1`;
- INT7, per-channel weights and dynamic per-token INT7 activations for the mineable group;
- FP8 Q, K, V, and MLP down projections outside the mineable group.

Each decoder layer has two mineable MLP weights:

- `model.language_model.layers.<L>.mlp.gate_proj.weight`;
- `model.language_model.layers.<L>.mlp.up_proj.weight`.

Each weight has safetensors type `I8` and shape `[21504, 5376]`. Safetensors row-major `[out, in]` bytes are Pearl column-major `[in, out]` bytes without a transpose. Padding extends each 5,376-byte output column to 8,192 bytes and extends the output count to 32,768.

`ai_pow_miner::gemma4::Gemma4Checkpoint` enforces this profile. It reads only `config.json` and the bounded safetensors header during registration. It reads one selected weight by file offset when it builds the persistent peak `B` matrix. It rejects a wrong model type, quantization scheme, layer schedule, INT7 tensor set, tensor shape, file range, or operand outside `[-64, 64]`.

The safetensors layout digest identifies names, shapes, and offsets for the runtime handshake. It is not a weight commitment. The existing attempt-specific Pearl commitments bind all padded matrix bytes.

## Consensus boundary

No consensus change is required.

The production verifier accepts miner-chosen matrices. It verifies these properties:

1. the dense Pearl V3 transcript;
2. the matrix commitments;
3. the opened noised tile and jackpot in the compact recursive certificate;
4. the Nockchain candidate commitment in the auxiliary inclusion;
5. the shape-adjusted Nockchain target;
6. the production parameter envelope and Layer-0 trace-height cap.

The padded target uses `PEAK_PRODUCTION_PARAMS`. This profile is already admitted and uses the existing dense `AIP1` artifact. Its Layer-0 trace uses the existing $2^{17}$ setup bucket.

Consensus does not know the name Gemma, the checkpoint digest, the layer number, the projection name, the activation scales, or the request. A miner can use other valid INT7 matrices. Model admission is operator policy in this architecture. Consensus enforcement of a model registry would require a candidate and artifact version change, a model-commitment rule, and an activation-source rule. It is a separate protocol change.

## Execution planes

### Inference plane

The inference process owns tokenization, transformer state, KV cache, activation quantization, per-token scales, per-channel weight scales, and model scheduling. It selects a gate or up projection and forms up to 4,096 INT7 activation rows.

Inference can wait. A bounded queue applies backpressure to inference requests. It never pauses the active mining search to accept a new request.

### Mining plane

The miner owns the Nockchain candidate, target, Pearl transcript, padded INT7 operands, noise, ticket search, winner validation, proof, and submission.

The selected padded `B` stays resident. Two padded `A` buffers permit one activation upload while the current immutable operand generation remains active. The miner changes the active operand generation only at a prepared-template boundary. A generation includes:

- checkpoint layout digest;
- layer and projection;
- logical token count;
- padded `A` and `B` bytes;
- quantization-scale handle for inference output;
- monotonically increasing target generation.

A new Nockchain candidate does not require new inference operands. It derives a new header, `kappa`, commitments, noise, and jackpot schedule from the current immutable operands. A stale candidate result cannot enter proof construction because the existing miner generation check remains authoritative.

### Output plane

The mining accumulation uses noised matrices. Inference needs the clean logical result. The output path therefore has two parts:

1. A separate peak kernel variant stores noised accumulators only for the logical `T × 21504` rectangle. The normal no-output kernel remains unchanged.
2. A correction worker reconstructs `A × B` from the stored noised result and the Pearl low-rank factors. It then applies the existing per-token and per-channel scales.

The correction worker runs outside the mining device when the hardware topology permits peer access. It may be slow. The inference request waits for it while the mining device continues no-output search on the same immutable operands.

The output variant must produce the same 16 ticket-state words and jackpot bytes as the no-output variant for every tile. Output materialization is not part of the consensus statement.

## Mining-performance rule

Mining throughput has priority over inference throughput.

The production mining device does not host the vLLM model, KV cache, attention kernels, sampling kernels, or low-rank correction kernels. Inference devices and mining devices use disjoint CUDA ordinals. The only mining-device additions are:

- an asynchronous activation copy into the inactive `A` buffer;
- one bounded logical-output store when an inference batch needs a result;
- a generation swap at a normal template boundary.

The normal search path calls the existing `ai_pow_v3_peak_kernel` symbol. It has no inference branch, output pointer, extra allocation, or extra synchronization.

A same-device output store is acceptable only if hardware measurement shows no mining-rate regression. The acceptance gate compares sustained complete-ticket TMAC/s under inference load with the same device and source matrices in mining-only mode. The target must remain within measurement noise and must not increase template-preparation latency or cancellation latency. If the gate fails, the output variant runs on an additional target device. Existing mining devices do not share inference work.

This rule has a direct hardware consequence: one finite GPU cannot perform extra visible work at zero cost unless the work uses otherwise idle execution or memory capacity. The implementation does not hide that cost in a lower reported inference rate or in partial ticket accounting.

## Local process boundary

Use a versioned Unix-domain control channel. Keep model traffic off the node gRPC and Hoon wire.

The control protocol needs these bounded messages:

1. `RegisterTarget`: layout digest, logical profile, layer, projection, and weight-scale metadata;
2. `ActivationBatch`: generation, token count, INT7 activation handle, and token-scale handle;
3. `OutputReady`: generation, logical shape, output handle, and completion event;
4. `RejectGeneration`: exact validation or capacity error.

Large tensors use CUDA IPC handles on a peer-capable host. The socket carries only fixed-size metadata and bounded strings. A pinned-host transport is diagnostic because PCIe readback can consume mining bandwidth. The miner validates every dimension and byte length before it opens a handle or allocates a buffer.

Loss of the inference process does not invalidate the current operand generation. The miner continues with the last validated Gemma operand snapshot. It does not substitute synthetic matrices after target registration. Inference reconnects with a new monotonic generation.

## Failure behavior

- A checkpoint mismatch fails target registration before CUDA allocation.
- A non-INT7 activation fails before template preparation.
- An invalid tensor handle or length rejects only the pending inference generation.
- A CUDA output mismatch disables the Gemma target path. It does not fall back to an unverified output.
- A GPU winner still passes the existing scalar Rust ticket recheck before proof construction.
- A candidate change can discard a mining hit but does not corrupt a clean inference result. The result is tied to the operand generation and matching noise factors, not to block acceptance.
- Queue saturation delays inference. It does not delay mining.

## Code ownership

- `crates/ai-pow-miner/src/gemma4.rs` owns checkpoint validation, tensor selection, INT7 validation, and deterministic peak padding.
- `crates/ai-pow-miner/src/peak.rs` owns the immutable operand generation and output-capable peak session API.
- `crates/ai-pow-miner-cuda/csrc/ai_pow_v3_peak.cu` owns mining accumulation and optional logical-output capture. The existing no-output kernel remains a separate symbol.
- The inference adapter owns vLLM hooks, scale transfer, correction, and result delivery.
- `crates/ai-pow-miner/src/run.rs` owns candidate generation, target checks, proof construction, and stale-work rejection.
- Consensus code remains model-agnostic.

## Hardware validation gate

Runpod work starts only after review of this boundary. The first hardware stage must record:

1. GPU model, topology, peer-access matrix, driver, CUDA Toolkit, and compiler flags;
2. exact checkpoint layout digest and selected tensor;
3. padded-operand equality against the logical Gemma INT7 matmul;
4. output/no-output transcript equality for first, last, and random tickets;
5. clean-output equality after low-rank correction and dequantization;
6. 60-second mining-only and inference-loaded complete-ticket TMAC/s;
7. template preparation, operand swap, output copy, and cancellation latency;
8. Compute Sanitizer results for both kernel variants;
9. compact proof verification through the existing consensus entry point.

Do not start model-quality or serving-throughput tuning until the mining non-regression and byte-equality gates pass.
