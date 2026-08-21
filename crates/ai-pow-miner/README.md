# `ai-pow-miner`

`ai-pow-miner` is the external miner for Nockchain's `%ai-pow` puzzle. It searches Pearl-compatible dense and grouped-GEMM ticket attempts, creates a compact recursive certificate only after a target hit, and submits the canonical block artifact to a node.

The `ai-pow-mine` binary is enabled by the `node` feature. The default library build keeps the ticket loop available without the NockApp and gRPC dependency tree.

## Place in the system

```text
nockchain kernel --%mine-ai effect--> ai-pow-mine
       ^                                  |
       |                                  +-- Pearl-style work attempt
       |                                  +-- Nockchain target check
       |                                  +-- compact certificate on hit
       +-------- %ai-pow poke ------------+
                         \
                          +-- optional Pearl Gateway submission
```

`nockchain-mining-common` supplies the private gRPC client and candidate decoding. `ai-pow` owns the work statement and Pearl compatibility. `ai-pow-zk` proves it. `ai-pow-jets` and the Hoon kernel independently verify every submitted block.

## Modes

- **Node-connected mining:** watches `%mine-ai` effects and submits `%ai-pow` commands through the private NockApp gRPC service.
- **Canonical CPU mode:** constructs valid Nockchain certificates without a Pearl Gateway; intended for fakenet and integration verification rather than competitive throughput.
- **Pearl merge mining:** evaluates one Pearl-compatible work instance and may submit the same hit to Pearl and Nockchain when their independent targets are met.

## Maintained invariants

- A candidate supplies the block commitment, AI target, and puzzle variant. The miner never chooses consensus target or fork-choice weight.
- Every extranonce is upstream of `kappa`, matrix commitments, noise, noised matrices, tile state, and jackpot. A new mining attempt rebuilds nonce-bound state; a nonce-only hash loop is forbidden.
- A recursive certificate is generated only after the ticket's jackpot satisfies the Nockchain target. The node repeats the target and proof checks.
- The certificate and opaque nonce envelope commit to the same Pearl transcript and Nockchain block commitment.
- Pearl auxiliary inclusion contains exactly one Nockchain commitment, preventing one Pearl proof from authorizing multiple Nockchain blocks.
- Dense and MoE artifacts use explicit, canonical variants. Routing, expert-local dimensions, opened schedules, and matrix commitments remain proof-bound.
- Candidate replacement cancels stale work. A proof for an old commitment cannot validate against a new block.
- Hoon sees only the versioned `%ai-pow` artifact and opaque `[len data]` nonce. Pearl gateway metadata never becomes a consensus-kernel concept.

## Trust boundaries

The miner is untrusted from consensus's perspective. Successful local verification is an optimization and diagnostic; only the node's Hoon rules plus mandatory Rust verify jet admit a block. Private gRPC gives kernel-level poke access and should be bound to a trusted local interface.

Merge mining shares a mineable work unit, not a proof system or chain target. Pearl and Nockchain retain independent acceptance, targets, block commitments, and submission paths.

## Soundness dependencies

The miner relies on `ai-pow` for Pearl byte compatibility and attempt binding, and on `ai-pow-zk` for certificate construction. Its security-sensitive obligations are canonical serialization, exact statement construction, fresh per-attempt work, and refusal to substitute prover-controlled setup or public parameters. Cryptographic acceptance properties are documented in [`../ai-pow-zk/docs/SECURITY.md`](../ai-pow-zk/docs/SECURITY.md).

## GPU container configuration

Build the Linux/amd64 production image:

```sh
docker buildx build \
  --platform linux/amd64 \
  -f docker/Dockerfile.ai-pow-miner-gpu \
  -t ai-pow-miner-gpu .
```

The image mines to `2nFsk7KTv9Fm5zMU3ckWAM4p9eLhUSVeVEKUoPFkfzehyjuzmpXAN8j` by default. Set `MINING_PKH` to direct rewards to a different v1 mining public-key hash. `NODE_ADDR` is required.

```sh
docker run --rm --gpus all \
  -e NODE_ADDR=http://node.example:5555 \
  -e MINING_PKH=<v1-mining-pkh> \
  ai-pow-miner-gpu
```

The image uses up to eight visible CUDA devices, canonical mode, and batches of 32,768 attempts per device by default. Set `CUDA_DEVICES` to `all` or a comma-separated ordinal list such as `0,1,2,3`; set `CANONICAL` or `GPU_BATCH_ATTEMPTS` to override the other values. Non-canonical mode also requires `PEARL_GATEWAY`.

## Reusable inference image

`docker/Dockerfile.ai-pow-inference` builds the pinned Pearl workspace and Rust
bridge in disposable CUDA 13 stages. The runtime contains stable vLLM 0.27.1,
the Pearl and Nockchain Python wheels, the release bridge, health and benchmark
tools, and the supervised launcher. It does not contain build toolchains.

The bridge and in-process inference library contain exact `sm_90a` code for
H100 and exact `sm_120a` code for RTX PRO 6000 Blackwell and RTX 5090. All
three device classes execute the same candidate-bound noising, mining GEMM,
exact clean-output reconstruction, FP32 scaling, and BF16 rounding contract.
H100 and RTX PRO 6000 use one GPU. RTX 5090 uses two tensor-parallel GPUs.

The complete one-command deployment, application API examples, health probes,
security requirements, configuration reference, and benchmark procedure are in
[`gemma4-production-deployment.md`](../../docs/ai-pow-integration/gemma4-production-deployment.md).

```sh
docker buildx build \
  --platform linux/amd64 \
  -f docker/Dockerfile.ai-pow-inference \
  -t ghcr.io/nockchain/nockchain-ai-pow-inference:local .
```

Model weights remain on a reusable volume rather than in the container layer:

```sh
MODEL_PATH=/workspace/models/Gemma-4-31B-it-pearl \
  ai-pow-inference-seed-model
ai-pow-inference-run
```

Production mining also requires the node's private gRPC address and one reward
public-key hash:

```sh
MODEL_PATH=/workspace/models/Gemma-4-31B-it-pearl \
NOCKCHAIN_NODE_ADDR=http://node.example:5555 \
MINING_PKH=<v1-mining-pkh> \
AI_POW_REQUIRE_MINING=1 \
  ai-pow-inference-run
```

`NOCKCHAIN_AI_POW_ENDPOINT` is the loopback endpoint between vLLM and the Rust
bridge. It is not the node endpoint. If `NOCKCHAIN_NODE_ADDR` is absent, the
OpenAI inference API remains available, but the bridge uses a zero-target
diagnostic job and cannot submit mining rewards.

The bridge subscribes to real `%mine-ai` candidates. A native winner is
streamed once, scalar-rechecked against the active generation, proved with the
compact recursive prover, encoded as the canonical `%ai-pow` noun, and
submitted to the node. No-hit inference sends no tensor data over gRPC.

The `AI-PoW inference image` GitHub workflow publishes commit and branch tags to
`ghcr.io/nockchain/nockchain-ai-pow-inference`. Rebuilding uses registry-independent
GitHub Actions caches for the Python, Rust, and CUDA dependency layers.

Runpod needs at least 80 GB of container disk for the unpacked runtime. Use one
80 GB H100 or RTX PRO 6000, or two 32 GB RTX 5090 devices:

```sh
runpodctl pod create \
  --image ghcr.io/nockchain/nockchain-ai-pow-inference:la-gemma4 \
  --gpu-id "NVIDIA H100 PCIe" \
  --gpu-count 1 \
  --container-disk-in-gb 80 \
  --volume-in-gb 60 \
  --ports 22/tcp \
  --docker-args "sleep infinity"
```

The launcher uses the visible GPU count as its tensor-parallel size and defaults
GPU memory utilization to `0.62`. Override either value only for a measured
deployment-specific reason.

The CUDA 13 image requires NVIDIA driver 580.126.09 or newer on RTX 5090.
Driver 570 can run a CUDA 12.8 build, but the CUDA forward-compatibility package
does not support GeForce devices.

Keep each HTTP server on loopback and use SSH port forwarding from the laptop.
`scripts/compare-gemma4-openai.py` sends the same greedy request to each local
tunnel and rejects unstable, cross-device-different, or unexpected output.

## Validation

```sh
cargo test -p ai-pow-miner
cargo test -p ai-pow-miner --all-features
cargo run --release -p ai-pow-miner --features node --bin ai-pow-mine -- --help
```
