# Gemma 4 inference optimization plan

## Objective

Increase Gemma 4 31B inference throughput on these production layouts:

- one H100 80 GB;
- one RTX PRO 6000 Blackwell 96 GB;
- two RTX 5090 32 GB devices.

Preserve the useful-work mining contract and the current Nockchain proof contract.
Do not change consensus parameters to improve an inference benchmark.

## Frozen consensus surface

This work must not change:

- `MatmulParams::GEMMA_4_31B_GATE_UP_FUSED`;
- the logical work limit `m <= 4096`;
- `k = 5376`, `n = 43008`, rank `128`, and tile size `16`;
- INT7 matrix range checks;
- candidate, extranonce, commitment, noise, ticket, jackpot, or target derivation;
- compact recursive certificate public inputs or noun encoding;
- Nockchain target or difficulty calculation;
- Hoon molds, commands, or verifier behavior.

Every optimized mining result must still pass the scalar Rust reconstruction and
the compact recursive verifier. The existing peak miner source and ABI remain
unchanged.

## Current execution facts

The checkpoint stores 16.955 GB of INT8 tensors that represent INT7 weights.
These tensors are the 120 gate/up projections and the 60 attention output
projections. They are 51 percent of the checkpoint. The checkpoint also stores
12.331 GB of 128 by 128 block-FP8 weights.

The current non-mining INT7 path uses Pearl's large-tile GEMM on H100. It uses
`torch._int_mm` on SM120 and pads decode batches of 1 through 16 tokens to 32
rows. The path applies scales in separate operations. The selected mining layer
pads a logical batch below 256 tokens to 256 rows.

The production launcher uses eager execution because the CuTeDSL INT7 quantizer
and the mining callback path are not safe for Torch compilation or CUDA graph
replay.

Gemma 4 has 50 sliding-attention layers with head dimension 256 and 10
full-attention layers with head dimension 512. Stable vLLM selects Triton for a
uniform backend when FA4 is not available.

## Implementation stages

### Stage 0: Controlled baseline

Measure the unmodified image on one host of each device class.

For each host:

1. Record the image digest, driver, CUDA runtime, clocks, power limit, topology,
   vLLM version, and selected kernels.
2. Run mining enabled and `MINER_NO_MINING=1` on the same host.
3. Measure concurrency 1, 8, and 32.
4. Measure a short prompt and an 8192-token prompt.
5. Use 32 warmup requests and at least 200 measured requests.
6. Require 256 output tokens with `ignore_eos=true`.
7. Capture one decode interval with a GPU kernel trace.

Microbenchmark these linear shapes at `m = 1, 2, 4, 8, 16, 32, 128, 256`:

- INT7 gate/up: `k = 5376`, `n = 21504` and `43008`;
- INT7 sliding output: `k = 8192`, `n = 5376`;
- INT7 full output: `k = 16384`, `n = 5376`;
- checkpoint block-FP8 QKV and down-projection shapes.

The baseline is complete only when the API output and mining winner still match
the current reference behavior.

### Stage 1: Fast non-mining INT7

Keep the existing INT7 weights, activation quantization, scales, and rounding.
Replace only the non-mining matrix multiplication implementation.

1. Pack or transpose non-mining weights during model loading.
2. Route non-mining INT7 linears through vLLM's CUTLASS scaled INT8 operation.
3. Keep the selected mineable gate/up projection on `NativeGemma4Session`.
4. Do not quantize activations to the full INT8 range. Preserve the INT7 result
   and scale from `quant_7bit`.
5. Add shape, dtype, scale, fused-projection, and tensor-parallel tests.
6. Compare every optimized output with the existing Pearl vanilla GEMM.

Acceptance:

- exact INT32 accumulator equality before output conversion;
- bit-equal BF16 output for deterministic fixtures;
- no change to selected mining matrix bytes;
- a measured serving improvement or removal of this stage.

### Stage 2: Graph-safe execution

Enable vLLM compilation without capturing consensus callbacks as one-time Python
side effects.

1. Register INT7 quantization as a Torch custom operation with a fake function.
2. Compile CuTeDSL quantization during model warmup, not during graph capture.
3. Expose the selected mining calculation as one explicit piecewise split
   boundary.
4. Keep winner polling, candidate validation, proof construction, and node
   submission outside CUDA graph replay.
5. Put candidate data and target data in stable buffers that update before each
   replay.
6. Start with piecewise CUDA graphs. Attempt a full graph only after the split
   path passes all lifecycle tests.

Acceptance:

- candidate replacement and cancellation still invalidate stale work;
- one graph replay produces one current-attempt mining calculation;
- callbacks run once per actual inference step, not once per graph capture;
- mining enabled and disabled outputs remain equal to eager mode;
- the server starts without `--enforce-eager`.

### Stage 3: Text-only model loading

Test the existing `Gemma4ForCausalLM` architecture directly.

1. Reuse the current language-model checkpoint names through vLLM's weight
   mapper.
2. Skip vision tower construction and weight loading.
3. Keep the tokenizer, chat template, vocabulary, embeddings, and language
   weights unchanged.
4. Compare output and selected mining matrices with `--language-model-only`.

Acceptance:

- at least 1.139 GB of checkpoint vision tensors do not enter device memory;
- text output and mining inputs are bit-equal;
- health, chat, tool, and reasoning surfaces remain operational.

### Stage 4: H100 attention and FP8 kernels

Use a separate experimental image. Do not replace stable vLLM until the upstream
Gemma FA4 fix is accepted or locally validated.

Compare:

1. Triton attention with BF16 KV cache;
2. language-only FA3 for 256-wide heads and FA4 for 512-wide heads;
3. calibrated FP8 KV cache with the mixed FA3 and FA4 path;
4. DeepGEMM-only block FP8;
5. FlashInfer for small batches and DeepGEMM for larger batches.

Package or precompile required SM90 FlashInfer kernels. Do not depend on a
runtime download.

Acceptance:

- exact greedy-output stability on fixed prompts;
- no model-quality regression outside the accepted evaluation band;
- no startup or first-request JIT compilation;
- a measured gain over Triton and DeepGEMM-only production baselines.

### Stage 5: SM120 kernels

Use a vLLM branch that contains the merged SM120 kernel work. Do not assume that
local `main` is a linear superset of stable vLLM.

Compare on RTX PRO 6000 and RTX 5090:

1. CUTLASS block FP8;
2. B12X 128 by 128 block FP8;
3. Triton attention;
4. FlashInfer XQA decode and its compatible prefill path;
5. eager and graph execution.

Force B12X only for supported linear families. Keep the INT7 path independent.

Acceptance:

- B12X passes every checkpoint shape and dtype check;
- attention passes repeated long-context and sliding-window comparisons;
- no first-request kernel compilation remains;
- the selected production backend wins end to end, not only in a GEMM test.

### Stage 6: Dual RTX 5090 topology

Compare tensor parallel size 2 with pipeline parallel size 2.

For pipeline parallelism:

1. Assign approximately 30 decoder layers to each device.
2. Let only the stage that owns the selected layer initialize mining state.
3. Let only one process own the inference bridge and candidate lifecycle.
4. Send stage outputs through the normal vLLM intermediate-tensor path.
5. Do not duplicate global ticket work on both devices.

Measure concurrency 1, 8, and 32. Record PCIe transfer and collective time.

Acceptance:

- tensor-parallel and pipeline-parallel text outputs match;
- selected mining matrices and winner reconstruction match;
- one candidate causes one mining lifecycle stream;
- the production topology has the better measured latency or throughput for its
  target workload.

### Stage 7: Hybrid NVFP4 feasibility

Do not make this artifact the default before the no-checkpoint stages complete.

A single RTX 5090 needs a smaller checkpoint. A hybrid text artifact can retain
all quantized layer-0 weights and convert the remaining linear weights to NVFP4.
The estimated text-weight footprint is 19.53 GB.

Requirements:

1. Start from the original BF16 Gemma 4 checkpoint when possible.
2. Preserve embeddings, norms, the complete layer-0 pre-mining path, and the
   selected INT7 gate/up bytes when byte-identical mining inputs are required.
3. Use a text-only config.
4. Calibrate NVFP4 activations and shared scales for fused projections.
5. Run the complete model-quality suite and mining KATs.

Nockchain consensus verifies miner-committed matrices and does not require one
full-model checkpoint. Pearl model certification can impose a separate model
identity requirement. Record that distinction before deployment.

Acceptance:

- the model and 8192-token KV cache fit one 32 GB RTX 5090 without CPU offload;
- output quality remains inside the approved band;
- the selected mining statement and proof remain valid;
- one-GPU cost, latency, and throughput beat the selected two-GPU deployment.

## End-to-end validation

Every production candidate must pass:

1. authenticated health and OpenAI chat completion;
2. repeated greedy output on all supported hardware;
3. selected-layer clean output against the scalar reference;
4. 1000-ticket scalar and device differential;
5. candidate replacement, disconnect, and stale-generation rejection;
6. forced target hit through winner opening and scalar validation;
7. compact recursive proof construction and consensus verification;
8. canonical `%ai-pow` submission to a private mock NockApp node;
9. node acknowledgement before reporting acceptance;
10. applicable CUDA memory, race, initialization, and synchronization checks.

## Commit and release gates

Commit each coherent stage only after its focused tests and hardware benchmark
pass. Remove a stage that does not improve end-to-end behavior.

The final release requires:

- a clean worktree;
- no changes in the frozen consensus surface;
- focused Rust and Python tests;
- shell, Ruff, formatting, and documentation checks;
- a successful production image workflow;
- an immutable image digest and matching revision label;
- updated production measurements;
- deletion of every validation GPU resource.
