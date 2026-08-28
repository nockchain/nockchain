# Gemma 4 production inference and AI-PoW

## Supported release

The production image serves `pearl-ai/Gemma-4-31B-it-pearl` with stable vLLM
0.27.1 and the Nockchain AI-PoW bridge. The image contains exact `sm_90a` code
for H100 and exact `sm_120a` code for RTX PRO 6000 Blackwell and RTX 5090.

Use one of these GPU layouts:

| GPU layout | Tensor parallel size | Minimum GPU memory |
|---|---:|---:|
| One H100 | 1 | 80 GB |
| One RTX PRO 6000 Blackwell | 1 | 96 GB |
| Two RTX 5090 | 2 | 32 GB per GPU |

RTX 5090 requires an NVIDIA driver from the 580 series for the CUDA 12.9 image.
Driver 570 cannot use the CUDA forward-compatibility package on GeForce.

## Runtime architecture

One container runs two cooperating services:

```text
production application
        |
        | OpenAI-compatible HTTP, port 8000
        v
vLLM 0.27.1 worker
        |
        | loopback gRPC, default 127.0.0.1:5590
        v
AI-PoW bridge -----------------> Nockchain private gRPC, default port 5555
        |                                  |
        | native CUDA                      | canonical %ai-pow submission
        v                                  v
inference output + mining work           node acknowledgement
```

The selected gate/up GEMM produces useful inference output and consensus mining
work. Normal requests do not send model tensors through gRPC. A rare target hit
streams the opened INT7 tensors to the bridge, then proof construction and node
submission continue on a background worker. The recursive proof does not hold
the customer response open.

## Required inputs

- An immutable production image tag.
- A model volume mounted at `/workspace/models`.
- Checkpoint revision `f1dfba688ce6343b0433de57ca4dc0f3d1c5baa5`, whose `model.safetensors` SHA-256 is pinned in the bridge.
- The node private gRPC URL.
- A v1 mining public-key hash.
- An API key if the HTTP listener is not loopback.

## Set the mining payout

Set `MINING_PKH` to the v1 mining public-key hash that receives this miner's
reward. The inference image has no default payout address:

```sh
export MINING_PKH=<V1_MINING_PKH>
```

For Compose, set the same value in the protected environment file:

```text
MINING_PKH=<V1_MINING_PKH>
```

The launcher passes this value to `ai-pow-inference-bridge --mining-pkh`. It
refuses production mining when `MINING_PKH` or `NOCKCHAIN_NODE_ADDR` is absent.
Use the same payout hash that you use with the standalone
`ai-pow-miner-gpu` image.

The startup log prints the node URL and selected `MINING_PKH` so the operator
can verify the payout configuration before traffic starts.

Keep model weights outside the image. Seed a new model volume once:

```sh
docker run --rm --gpus all \
  -v gemma-models:/workspace/models \
  -e MODEL_PATH=/workspace/models/Gemma-4-31B-it-pearl \
  <IMAGE> \
  ai-pow-inference-seed-model
```

The seed command downloads the fixed checkpoint revision and exits only after
the bridge validates the profile and the pinned SHA-256 weight digest.

Do not run the seed command when the volume already contains the complete
checkpoint.

## One-command production start

The safest same-host deployment uses host networking and a loopback HTTP
listener:

```sh
docker run --rm --gpus all --network host \
  --name nockchain-gemma \
  -v gemma-models:/workspace/models:ro \
  -e MODEL_PATH=/workspace/models/Gemma-4-31B-it-pearl \
  -e NOCKCHAIN_NODE_ADDR=http://127.0.0.1:5555 \
  -e MINING_PKH=<V1_MINING_PKH> \
  -e AI_POW_REQUIRE_MINING=1 \
  -e VLLM_HOST=127.0.0.1 \
  -e VLLM_PORT=8000 \
  -e VLLM_SERVED_MODEL_NAME=gemma-4-31b-it-pearl \
  <IMAGE> \
  ai-pow-inference-run
```

A container-network deployment must bind vLLM to all container interfaces. It
must also enable API authentication:

```sh
docker run --rm --gpus all \
  --name nockchain-gemma \
  -p 127.0.0.1:8000:8000 \
  -v gemma-models:/workspace/models:ro \
  -e MODEL_PATH=/workspace/models/Gemma-4-31B-it-pearl \
  -e NOCKCHAIN_NODE_ADDR=http://node.internal:5555 \
  -e MINING_PKH=<V1_MINING_PKH> \
  -e AI_POW_REQUIRE_MINING=1 \
  -e VLLM_HOST=0.0.0.0 \
  -e VLLM_API_KEY=<RANDOM_SECRET> \
  <IMAGE> \
  ai-pow-inference-run
```

For routine operation, copy
[`ai-pow-inference.env.example`](../../docker/ai-pow-inference.env.example) to
a protected deployment directory, replace its required values, and start with:

```sh
docker run --rm --gpus all \
  --name nockchain-gemma \
  --env-file /secure/nockchain-gemma.env \
  -p 127.0.0.1:8000:8000 \
  -v gemma-models:/workspace/models:ro \
  <IMAGE> \
  ai-pow-inference-run
```

The shipped Compose file provides the shortest managed deployment:

```sh
cp docker/ai-pow-inference.env.example /secure/nockchain-gemma.env
# Edit and protect /secure/nockchain-gemma.env before continuing.
export AI_POW_IMAGE=<IMMUTABLE_IMAGE>
export AI_POW_ENV_FILE=/secure/nockchain-gemma.env
docker compose -f docker/compose.ai-pow-inference.yml \
  --profile seed run --rm seed-model
docker compose -f docker/compose.ai-pow-inference.yml up -d inference
docker compose -f docker/compose.ai-pow-inference.yml ps
```


The launcher refuses a non-loopback listener without `VLLM_API_KEY`. Set
`VLLM_ALLOW_UNAUTHENTICATED=1` only behind an authenticated private proxy.

## Application integration

The stable model identifier is `gemma-4-31b-it-pearl`. Applications must not
use the model filesystem path as the API model name.

### cURL

```sh
curl http://127.0.0.1:8000/v1/chat/completions \
  -H "Authorization: Bearer ${VLLM_API_KEY}" \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "gemma-4-31b-it-pearl",
    "messages": [{"role": "user", "content": "Explain deterministic consensus."}],
    "temperature": 0,
    "max_tokens": 128
  }'
```

### Python OpenAI client

```python
import os
from openai import OpenAI

client = OpenAI(
    base_url="http://127.0.0.1:8000/v1",
    api_key=os.environ["VLLM_API_KEY"],
    timeout=120.0,
)
response = client.chat.completions.create(
    model="gemma-4-31b-it-pearl",
    messages=[{"role": "user", "content": "Explain deterministic consensus."}],
    temperature=0,
    max_tokens=128,
)
print(response.choices[0].message.content)
```

### Streaming

```python
stream = client.chat.completions.create(
    model="gemma-4-31b-it-pearl",
    messages=[{"role": "user", "content": "Explain deterministic consensus."}],
    temperature=0,
    max_tokens=128,
    stream=True,
)
for event in stream:
    content = event.choices[0].delta.content
    if content:
        print(content, end="", flush=True)
```

Use streaming when time to first token matters. Set an application timeout that
fits the requested output length. A 120-second timeout is a safe starting point
for interactive requests; batch jobs with long outputs need a larger timeout.

Production clients should reuse HTTP connections and cap their own concurrency
to the measured server capacity. Retry connection failures, HTTP 429, and HTTP
503 with bounded exponential backoff only before response streaming starts. Do
not replay a partially received stream. Treat HTTP 400 and HTTP 401 as permanent
request failures. Route traffic only after the readiness probe succeeds.

## Readiness and monitoring

The image contains a combined health probe:

```sh
ai-pow-inference-health --wait 600 --timeout 3
```

The command checks:

- bridge gRPC readiness;
- the vLLM `/health` endpoint;
- the expected model in `/v1/models`;
- node connectivity and a live rank-zero mining runtime when `AI_POW_REQUIRE_MINING=1`.

It prints one JSON object with API state, runtime lease counts, rank-zero and
mining-enabled ownership, and expiry counters. The image also defines a Docker
`HEALTHCHECK` with a 10-minute startup grace period.

Useful HTTP endpoints:

| Endpoint | Purpose |
|---|---|
| `/health` | vLLM process readiness |
| `/v1/models` | served model identity |
| `/metrics` | Prometheus metrics |
| `/v1/chat/completions` | OpenAI-compatible chat API |

A Kubernetes readiness probe can execute the shipped command:

```yaml
readinessProbe:
  exec:
    command: ["ai-pow-inference-health", "--timeout", "3"]
  periodSeconds: 10
  timeoutSeconds: 5
  failureThreshold: 3
  initialDelaySeconds: 30
```

Alert when the container is unhealthy, when `node_connected` is false in
production mode, or when the inference error rate increases. Scrape `/metrics`
for request queue, token, cache, and latency metrics.

## Shutdown and restart

`ai-pow-inference-run` supervises both child services. If vLLM or the bridge
exits, the launcher stops the other child and exits. `SIGTERM` stops vLLM, then
the bridge, and waits for both processes. Use a container restart policy such as
`unless-stopped` or a Kubernetes Deployment.

A recursive proof that is already running can delay graceful bridge shutdown.
Give the container a termination grace period of at least 180 seconds.

## Configuration reference

| Variable | Default | Production use |
|---|---|---|
| `MODEL_PATH` | `/workspace/models/Gemma-4-31B-it-pearl` | Mounted checkpoint directory |
| `NOCKCHAIN_NODE_ADDR` | unset | Node private gRPC URL |
| `MINING_PKH` | unset | Required v1 reward hash |
| `AI_POW_REQUIRE_MINING` | `0` | Set to `1` to make node connectivity part of readiness |
| `NOCKCHAIN_AI_POW_ENDPOINT` | `127.0.0.1:5590` | Internal bridge endpoint; keep on loopback |
| `NOCKCHAIN_GEMMA4_MINING_LAYER` | `0` | Selected mineable decoder layer |
| `VLLM_HOST` | `127.0.0.1` | HTTP bind address |
| `VLLM_PORT` | `8000` | HTTP port |
| `VLLM_API_KEY` | unset | Required for non-loopback HTTP |
| `VLLM_SERVED_MODEL_NAME` | `gemma-4-31b-it-pearl` | Stable API model identifier |
| `VLLM_TENSOR_PARALLEL_SIZE` | visible GPU count | Set only for an intentional topology |
| `VLLM_MAX_MODEL_LEN` | `8192` | Maximum context length |
| `VLLM_GPU_MEMORY_UTILIZATION` | `0.66` | Validated universal memory fraction |
| `VLLM_ENFORCE_EAGER` | `0` | Set to `1` only for the eager diagnostic fallback |
| `VLLM_COMPILATION_CONFIG` | compile with CUDA graph replay disabled | Keep `cudagraph_mode` at `0`; replay does not preserve inference output with the mineable operation |
| `VLLM_MAX_NUM_SEQS` | unset | Optional vLLM scheduler limit |
| `VLLM_MAX_NUM_BATCHED_TOKENS` | unset | Optional vLLM scheduler token limit |
| `VLLM_DISABLED_KERNELS` | selected by compute capability | Validated FP8 kernel exclusions; override only after device testing |

Extra command-line arguments after `ai-pow-inference-run` pass directly to
`vllm serve`.

## Benchmark procedure

Run the benchmark from the application host. This includes network and JSON
costs:

```sh
VLLM_API_KEY=<RANDOM_SECRET> ai-pow-inference-bench \
  --base-url http://127.0.0.1:8000 \
  --requests 100 \
  --concurrency 1 \
  --warmup 5 \
  --max-tokens 128 \
  --stream
```

Repeat with application concurrency:

```sh
VLLM_API_KEY=<RANDOM_SECRET> ai-pow-inference-bench \
  --base-url http://127.0.0.1:8000 \
  --requests 200 \
  --concurrency 8 \
  --warmup 10 \
  --max-tokens 128
```

The JSON report includes p50, p95, and p99 latency, streaming time to first
token, requests per second, output tokens per second, and total tokens per
second. Keep prompt text, output limit, concurrency, image digest, GPU, and
driver fixed when comparing releases.

Measure the isolated mining kernel inside the container:

```sh
ai-pow-gemma4-bench \
  --m 4096 \
  --n 43008 \
  --warmup-iterations 100 \
  --iterations 200
```

This reports launch time, tickets per second, and complete-ticket TMAC/s. The
OpenAI benchmark measures normal coexistence because idle mining runs whenever
no selected inference work is active.

## Image promotion gate

The image workflow resolves every base image and tool image by digest and uses
the checked-in Python lock. It publishes a candidate digest only after the Rust
suite and the Python-to-gRPC-to-Rust KAT pass. Separate H100, RTX PRO 6000
Blackwell, and dual RTX 5090 runners must then pass the native scalar
differential and varied-batch VRAM KAT.

Release tags are created from the tested digest. The digest carries maximum
BuildKit provenance and an SBOM and has a keyless Sigstore signature. Failed or
missing GPU jobs leave only the `candidate-<commit>` tag and cannot update a
branch, commit, or `latest` release tag.

## Validated production measurements

Use image
`ghcr.io/nockchain/nockchain-ai-pow-inference:sha-6fd3db13c211ef12ad5fab4b3610c8aa107b06fb`.
Its manifest digest is
`sha256:d256494c95dc3474cc02c77ad093f74b9110b0dbb6f38c2c9b361a1268a76376`.

The OpenAI benchmark used varied short prompts, 128 output tokens, a warm
server, and the production mineable layer. The table reports output tokens per
second. The isolated mining benchmark used 10 warmup launches and 30 measured
launches.

| GPU layout | Driver | c1 | c64 | c256 | Isolated mining |
|---|---:|---:|---:|---:|---:|
| H100 80 GB | 580.126.09 | 21.8 | 1201.0 | 1537.7 | 159.0 TMAC/s |
| RTX PRO 6000 Blackwell 96 GB | 595.91.07 | 14.3 | 798.8 | 1426.8 | 223.2 TMAC/s |
| Two RTX 5090 32 GB | 580.142 | 18.6 | 880.4 | 1131.9 | 366.0 TMAC/s |

The RTX 5090 mining value is for tensor-parallel rank zero. The follower rank
calculates only its local inference projection. Do not add device throughput
values.

## Security checklist

- Use an immutable image digest.
- Keep the node private gRPC endpoint on a private network.
- Keep the bridge endpoint on loopback.
- Require an API key for every non-loopback HTTP listener.
- Terminate TLS at a trusted reverse proxy or service mesh.
- Do not store the API key or mining reward configuration in the image.
- Mount the model volume read-only during serving.
- Restrict `/metrics` and `/v1/models` to the private application network.
- Rotate the API key without rebuilding the image.

## Troubleshooting

`AI-PoW production mining is not connected to the node`
: Check `NOCKCHAIN_NODE_ADDR`, node reachability, the reward hash, and private
  gRPC permissions.

`HTTP 401`
: Supply the same `VLLM_API_KEY` to the application.

`model is not served`
: Use `VLLM_SERVED_MODEL_NAME=gemma-4-31b-it-pearl` in both the server and
  application.

CUDA status 804 on RTX 5090
: Select a host with an NVIDIA driver from the 580 series. CUDA forward
  compatibility does not support GeForce driver 570.

Container is unhealthy during startup
: Model loading can take several minutes. Inspect container logs and confirm
  that the model volume contains a complete checkpoint.
