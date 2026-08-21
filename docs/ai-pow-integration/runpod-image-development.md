# Runpod image development

Use the direct publisher for development images. GitHub Actions is not in the development loop.

The workflow has two build modes:

- A **plugin overlay** installs the current `vllm-miner` Python source on a complete base image. Use this mode for normal edits under `crates/ai-pow-miner/vllm-plugin/src`.
- A **complete image** rebuilds the Rust bridge, Pearl workspace, Python environment, native kernels, and runtime image. Use this mode when the base environment is not valid for the change.

## Prerequisites

Docker must have Buildx and registry access. Log in before the first push:

```sh
printf '%s' "$GHCR_TOKEN" | docker login ghcr.io \
  --username "$GHCR_USER" \
  --password-stdin
```

The token must have permission to write `ghcr.io/nockchain/nockchain-ai-pow-inference`. Runpod must have registry credentials when the package is private.

Run all publisher commands from the Nockchain repository root.

## Fast Python source loop

Start from a complete image that has the required CUDA, vLLM, Pearl, Rust bridge, and package dependencies. The branch image is suitable when it contains the required base environment:

```sh
BASE_IMAGE=ghcr.io/nockchain/nockchain-ai-pow-inference:la-gemma4
DEV_TAG=dev-la-gemma4-01

scripts/publish-ai-pow-inference-image.sh \
  --base "$BASE_IMAGE" \
  --tag "$DEV_TAG"
```

The publisher builds `docker/Dockerfile.ai-pow-inference-plugin` and pushes:

```text
ghcr.io/nockchain/nockchain-ai-pow-inference:dev-la-gemma4-01
```

The overlay installs the local `vllm-miner` package with `--no-deps`. The registry reuses all base-image layers and uploads only content that is not present. Give each Runpod deployment a new tag to prevent reuse of a cached mutable tag.

Repeat these steps for each Python source revision:

1. Edit files under `crates/ai-pow-miner/vllm-plugin/src`.
2. Select a new development tag.
3. Publish the overlay with the same complete base image.
4. Create the replacement Runpod pod with the new image tag.
5. Confirm the GPU model with `nvidia-smi` after login.

Publishing an image does not change a running pod. Let an active workload finish before replacing its pod.

## Complete image build

Use a complete build after changes to any of these inputs:

- Rust source or Cargo dependencies
- CUDA code or native kernels
- `pyproject.toml` dependencies
- Cargo or Python lock files
- `docker/Dockerfile.ai-pow-inference`
- Container entrypoints or runtime tools
- The pinned Pearl revision or base-image versions

Run the publisher without `--base`. Set `BUILDX_BUILDER` to a builder name from `docker buildx ls`:

```sh
BUILDX_BUILDER=native-amd64
scripts/publish-ai-pow-inference-image.sh \
  --builder "$BUILDX_BUILDER" \
  --tag full-la-gemma4-01
```

`--builder` is optional. A persistent native Linux/amd64 builder is the fastest choice for complete builds. It avoids amd64 emulation on Apple Silicon and retains BuildKit cache mounts between builds.

The complete build also imports and exports this registry cache:

```text
ghcr.io/nockchain/nockchain-ai-pow-inference:buildcache
```

The selected local Buildx builder remains usable when no remote native builder is available. Its first complete amd64 build is slower because Docker Desktop uses emulation. Later builds reuse its local cache.

GitHub Actions materializes the expensive dependency targets before assembling
the runtime image. It stores three registry-backed BuildKit caches:

```text
ghcr.io/nockchain/nockchain-ai-pow-inference:buildcache-python
ghcr.io/nockchain/nockchain-ai-pow-inference:buildcache-rust
ghcr.io/nockchain/nockchain-ai-pow-inference:buildcache-runtime
```

The Python cache contains the pinned Pearl dependency wheels, including the
long CUDA extension build. The vLLM-miner source wheel is a later, inexpensive
layer. The Rust cache contains the bridge, benchmark, and native CUDA library.
A runtime assembly failure therefore does not discard either expensive cache.
Changing only Python source, launcher scripts, documentation, or final-image
configuration reuses the dependency caches.

Inspect available builders with:

```sh
docker buildx ls
```

Preview either publisher command without building or pushing:

```sh
scripts/publish-ai-pow-inference-image.sh \
  --dry-run \
  --base "$BASE_IMAGE" \
  --tag "$DEV_TAG"
```

## Start the Runpod pod

Use [`gemma4-production-deployment.md`](gemma4-production-deployment.md) for
production application configuration, API authentication, health checks,
monitoring, graceful shutdown, and capacity benchmarks. This document covers
image development and Runpod validation only.

Keep model weights on the Runpod volume at `/workspace/models`. The volume survives pod replacement and prevents repeated model downloads.

Create a pod with the published development image:

```sh
runpodctl pod create \
  --image "ghcr.io/nockchain/nockchain-ai-pow-inference:${DEV_TAG}" \
  --gpu-id "NVIDIA H100 PCIe" \
  --gpu-count 1 \
  --container-disk-in-gb 80 \
  --volume-in-gb 60 \
  --ports 22/tcp \
  --docker-args "sleep infinity"
```

The launcher uses the visible GPU count as its tensor-parallel size and defaults
GPU memory utilization to `0.62`. Thus H100 and RTX PRO 6000 use one rank, and a
two-device RTX 5090 pod uses two ranks.

The complete image is large. For GraphQL deployments, set `minDownload` to at
least 500 Mbit/s. A low-bandwidth host can spend most of its rental time pulling
the image.

Record the pod ID from the create result. Do not repeat a create command after a timeout until `runpodctl pod list` proves that no matching pod exists.

Get the direct SSH endpoint and key information from Runpod:

```sh
runpodctl ssh info <pod-id>
```

Use the reported command exactly. Add a PTY and host-key handling for an interactive session:

```sh
ssh -tt -i <reported-key> root@<reported-ip> -p <reported-port> \
  -o StrictHostKeyChecking=accept-new
```

If the direct endpoint is not ready, use the complete Basic SSH command from the Runpod **Connect** tab. Preserve its opaque `<pod-id>-<routing-token>@ssh.runpod.io` username.

After login, verify the allocated GPU and start the inference service:

```sh
nvidia-smi
MODEL_PATH=/workspace/models/Gemma-4-31B-it-pearl \
  ai-pow-inference-seed-model
MODEL_PATH=/workspace/models/Gemma-4-31B-it-pearl \
NOCKCHAIN_NODE_ADDR=http://<node-private-grpc-host>:5555 \
MINING_PKH=<v1-mining-pkh> \
  ai-pow-inference-run
```

`NOCKCHAIN_NODE_ADDR` and `MINING_PKH` enable production candidate
subscription, recursive proof construction, and node submission. Omit both
only for a zero-target inference diagnostic. The OpenAI-compatible server
listens on port 8000; keep it on the pod and use an SSH tunnel for remote
clients.

For RTX 5090, select a host with NVIDIA driver 580.126.09 or newer. The
published image uses CUDA 13. The CUDA forward-compatibility package rejects
GeForce on driver 570 with status 804.

Run `ai-pow-inference-seed-model` only when the model volume does not contain the required weights.

## Verify the published reference

Inspect the registry manifest before creating the pod:

```sh
docker buildx imagetools inspect \
  "ghcr.io/nockchain/nockchain-ai-pow-inference:${DEV_TAG}"
```

The manifest must include `linux/amd64`. Runpod cannot use a local Docker image that was built without `--push`; use the publisher so the registry contains the image.
