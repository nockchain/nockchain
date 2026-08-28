# Nockchain vLLM AI-PoW Plugin

This package is derived from Pearl Research Labs' vLLM mining plugin. Source
provenance and license notices are in `NOTICE-PEARL`, `LICENSE-PEARL-MIT`, and
`LICENSE-PEARL-ISC`.

The package admits the fixed dense Gemma 4 gate/up projection. A configured
`NOCKCHAIN_AI_POW_ENDPOINT` supplies canonical mining jobs and receives opened
witnesses. Without that endpoint, the same quantized layers use the vanilla
inference GEMM and do not mine.

The production package is built by `docker/Dockerfile.ai-pow-inference` against
the pinned Pearl revision and `uv.lock` in this directory.

## Generate the Python gRPC bindings

Python bindings come from the canonical schema in `nockapp-grpc-proto`:

```sh
uv run --with grpcio-tools==1.73.1 --with protobuf==6.33.5 python \
  crates/ai-pow-miner/vllm-plugin/generate_proto.py \
  crates/nockapp-grpc-proto/proto/nockchain/ai_pow/v1/inference_mining.proto \
  crates/ai-pow-miner/vllm-plugin/src/vllm_miner/proto
```

The generated files are build artifacts and are not checked in.

## Test

Run the interface suite in the pinned image environment:

```sh
pytest crates/ai-pow-miner/vllm-plugin/tests
```
