# Pull Request 164 Review Comments

Source: [PR 164: `feat(ai-pow): add Gemma 4 inference mining path`](https://github.com/nockchain/nockchain/pull/164)

Captured on 2026-08-28. The pull request had six inline review comments and two general discussion comments. The six review submissions had no separate review body.

Each comment has triage fields. Complete pending fields during triage. The quoted text is the original comment text.

## Inline review comments

### Different Python and Rust mining configurations

- Author: `@bitemyapp`
- Date: 2026-08-24 15:30:03 UTC
- Location: `crates/ai-pow-miner/vllm-plugin/src/vllm_miner/vllm_kernels.py:433`
- Source: [discussion_r3844965065](https://github.com/nockchain/nockchain/pull/164#discussion_r3844965065)
- Triage status: Implemented
- Disposition: Accept
- Owner:
- Notes: Inference protocol 2 carries the Rust-encoded 52-byte mining configuration and effective target. The Nockchain plugin consumes both fields without local adjustment. A Python-to-gRPC-to-Rust known-answer test checks the fixed Gemma 4 values.

> **Blocker — Python and Rust serialize different mining configurations.** `GPUMatmulConfigFactory.create()` uses the pinned Pearl defaults (`rows=[0,8]` and 64 sparse columns), while Rust reconstruction builds contiguous 16×16 patterns. That changes the 52-byte `mu`, target work factor, `kappa`, seeds, and jackpot; a hit from the real vLLM path should fail Rust seed reconstruction. Please make the Rust `MiningJob` carry the authoritative `mu` and effective target, have Python consume them verbatim, and add a Python → gRPC → Rust known-answer test.

### Unbounded native VRAM cache

- Author: `@bitemyapp`
- Date: 2026-08-24 15:30:04 UTC
- Location: `crates/ai-pow-miner/vllm-plugin/src/vllm_miner/vllm_kernels.py:105`
- Source: [discussion_r3844965258](https://github.com/nockchain/nockchain/pull/164#discussion_r3844965258)
- Triage status: Pending
- Disposition:
- Owner:
- Notes:

> **Blocker — this cache can retain roughly 13 GiB of native VRAM.** The key includes padded `m`, and the padding logic permits 16 shapes from 256 through 4096. Every retained session owns another full `B` workspace plus `m×n` INT32/BF16 buffers. Static allocation accounting is about 1.27 GiB for `m=4096` and 12.8 GiB across all shapes, before retained PyTorch tensors and normal output. Please use a bounded LRU with stream synchronization or a safely resizable workspace, and add a varied-batch steady-state VRAM test.

### Checkpoint validation is not active

- Author: `@bitemyapp`
- Date: 2026-08-24 15:30:06 UTC
- Location: `crates/ai-pow-miner/vllm-plugin/src/vllm_miner/nockchain_client.py:26`
- Source: [discussion_r3844965429](https://github.com/nockchain/nockchain/pull/164#discussion_r3844965429)
- Triage status: Pending
- Disposition:
- Owner:
- Notes:

> **Checkpoint validation is currently non-operative in production.** A missing environment value silently registers a zero layout digest; the bridge only length-checks and hashes it into the runtime ID, while `Gemma4Checkpoint::open` is referenced only by tests. The documented mismatch-before-CUDA property is therefore not enforced, and the layout digest is not a weight-content commitment. Either wire a fail-closed startup preflight with a pinned content digest, or remove/relabel the unused validation and identity path and correct the documentation.

### Automatic parity triggers are disabled

- Author: `@bitemyapp`
- Date: 2026-08-24 15:30:07 UTC
- Location: `.github/workflows/parity.yml:17`
- Source: [discussion_r3844965586](https://github.com/nockchain/nockchain/pull/164#discussion_r3844965586)
- Triage status: Pending
- Disposition:
- Owner:
- Notes:

> **Please restore the automatic parity triggers.** This changes the repository-wide compiler parity gate from push/PR execution to manual-only, which is unrelated to inference mining and removes protection for future changes across the tree. If runtime or cost is the concern, narrow the path filters or schedule the expensive jobs in a separate PR rather than disabling the gate here.

### Image promotion lacks reproducible inputs and acceptance gates

- Author: `@bitemyapp`
- Date: 2026-08-24 15:30:08 UTC
- Location: `.github/workflows/ai-pow-inference-image.yml:95`
- Source: [discussion_r3844965759](https://github.com/nockchain/nockchain/pull/164#discussion_r3844965759)
- Triage status: Pending
- Disposition:
- Owner:
- Notes:

> **Image promotion needs reproducible inputs and an encoded acceptance gate.** This workflow publishes with provenance disabled and runs no GPU or cross-language correctness tests. The Docker build also uses mutable base/tool tags, installs Rust through a live script, and resolves `uv lock` at build time without a checked-in lock; the tested image cited in the PR predates this head. Please pin image/tool digests and the dependency lock, enable provenance/SBOM and signing, and gate published tags on the cross-language KAT plus target-GPU tests.

### Active work lacks lease and crash cleanup

- Author: `@bitemyapp`
- Date: 2026-08-24 15:30:10 UTC
- Location: `crates/ai-pow-miner/src/inference.rs:178`
- Source: [discussion_r3844965912](https://github.com/nockchain/nockchain/pull/164#discussion_r3844965912)
- Triage status: Pending
- Disposition:
- Owner:
- Notes:

> **Active work has no lease or crash cleanup.** `STARTED` inserts this key indefinitely, and only an explicit `FINISHED` or `FAILED` removes it. A process crash or transient completion-RPC failure can therefore pause idle mining permanently; registered runtimes also never deregister. Please add a runtime heartbeat/lease and expiry, clean up its active work on timeout, and expose registered rank-zero/mining-enabled state in health checks. The bridge binary should also enforce loopback itself or authenticate non-loopback listeners.

## General discussion comments

### Production inference validation results

- Author: `@tacryt-socryp`
- Date: 2026-08-22 01:40:35 UTC
- Source: [issuecomment-5377155660](https://github.com/nockchain/nockchain/pull/164#issuecomment-5377155660)
- Triage status: Pending
- Disposition:
- Owner:
- Notes:

> Production inference validation is complete.
>
> - Published stable vLLM 0.27.1 with CUDA 12.9: `ghcr.io/nockchain/nockchain-ai-pow-inference:sha-9a6c2aa2d6501f654b05c1785e6cbb30bcfd5bb6`
> - Manifest: `sha256:270626aa364e48d381903ccc16ffab0c99cff9563989c87401b78a5f2403da2d`
> - Image revision label matches `9a6c2aa2d6501f654b05c1785e6cbb30bcfd5bb6`.
> - Final image workflow passed: https://github.com/nockchain/nockchain/actions/runs/32543081243
>
> | GPU layout | Driver | Interactive p50 TTFT | Interactive output tokens/s | Concurrency-8 output tokens/s | Isolated mining TMAC/s |
> |---|---:|---:|---:|---:|---:|
> | H100 SXM | 580.126.09 | 0.133 s | 15.501 | 34.863 | 258.970 |
> | RTX PRO 6000 Blackwell | 580.159.04 | 0.477 s | 3.662 | 27.800 | 345.884 |
> | Dual RTX 5090 | 580.65.06 | 0.170 s | 6.096 | 45.333 | 310.812 / 318.821 |
>
> The OpenAI measurements include scheduler coexistence with idle mining. H100 selects DeepGEMM. Blackwell excludes the unsupported DeepGEMM scale layout and selects CUTLASS. Dual RTX 5090 requires `VLLM_GPU_MEMORY_UTILIZATION=0.64` for the 8,192-token KV cache; this is the image default.
>
> All three layouts passed authenticated health checks and real Gemma chat completions. H100, RTX PRO 6000, and dual RTX 5090 returned stable repeated exact-string output. Driver 570 RTX 5090 hosts fail CUDA initialization with status 804 because CUDA forward compatibility does not support GeForce; use a driver from the 580 series.
>
> All Runpod validation resources are terminated.

### Suggested scope reductions

- Author: `@bitemyapp`
- Date: 2026-08-24 15:30:20 UTC
- Source: [issuecomment-5397498585](https://github.com/nockchain/nockchain/pull/164#issuecomment-5397498585)
- Triage status: Pending
- Disposition:
- Owner:
- Notes:

> Safe scope reductions that should not weaken correctness, security, or performance:
>
> - Keep the trust boundary in this repository: canonical Rust configuration and scalar reconstruction, bridge/proof integration, one protobuf schema, the CUDA kernel and thin wrapper, a minimal Gemma adapter, and cross-language known-answer tests.
> - Remove or defer the MoE serving modules and their tests. The fixed Gemma profile is dense; `moe_gemm_operators.py`, `pearl_moe_experts.py`, and `pearl_moe_method.py` add about 1,100 runtime lines before tests.
> - Trim the legacy noisy-gateway plugin path, retaining only the vanilla fallback and native Gemma path needed here. Generic Pearl/vLLM functionality can be separately versioned and pinned by commit and signed OCI digest.
> - Deduplicate the two identical 116-line protobuf schemas and generated Python bindings. Generate both languages from one canonical schema, removing roughly 450 checked-in lines.
> - Resolve the checkpoint module decisively: either wire a small fail-closed, content-authenticating preflight, or remove the unused loader/layout-only identity machinery and correct the docs.
> - Move the parity workflow change, image publishing, benchmarks, Runpod utilities, and long optimization notes to follow-up PRs. Extract the inference-node additions from `run.rs` into a dedicated module for reviewability.
>
> That should remove roughly 2,000–3,000 checked-in lines immediately, and more if the generic serving plugin is separately packaged. I would not try to shrink the CUDA kernel, scalar reconstruction, proof boundary, or the end-to-end tests; those are the parts worth keeping local and directly auditable.
