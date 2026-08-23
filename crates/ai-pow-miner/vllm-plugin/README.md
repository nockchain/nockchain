# Nockchain vLLM AI-PoW Plugin

This package is derived from Pearl Research Labs' vLLM mining plugin. It keeps
Pearl's quantized NoisyGEMM integration and adds the typed Nockchain AI-PoW
control plane. Source provenance and license notices are in `NOTICE-PEARL`,
`LICENSE-PEARL-MIT`, and `LICENSE-PEARL-ISC`.

## Description

This package provides custom CUDA kernels for quantized matrix multiplication with noise, designed to be used as a plugin with [vLLM](https://github.com/vllm-project/vllm). It is optimized for performance in mining and other intensive computational tasks.

The core of the package is a `noisy_gemm` operation, which can be used as a drop-in replacement for standard GEMM operations in PyTorch models.

When `NOCKCHAIN_AI_POW_ENDPOINT` is set, the plugin obtains mining jobs from the
Nockchain gRPC bridge, reports CUDA work lifecycles to the mining-first
scheduler, and streams opened witnesses on a target hit. Without that variable,
the original Pearl Gateway behavior remains available.

## Installation

You can install the package directly from this directory using pip.

### Standard Installation

```bash
pip install .
```

### Development Installation

For development, you can install the package in editable mode with all development dependencies:

```bash
pip install -e ".[dev]"
```

This will install the package and additional tools for testing, linting, and formatting.

## Running Tests

To run the test suite, use `pytest`:

```bash
pytest
```

## Building the Docker Image

To build the Docker image, you need a GitHub personal access token with read access to the repository.

1.  Run the build command:

    ```bash
    docker build -t vllm_miner ../../ -f Dockerfile
    ```

## Running the Miner

To run the miner, use the following command. The container will start the `pearl-gateway` service and the miner application.

Make sure to update `HF_TOKEN` to a good value, and also set `PEARLD_RPC_URL` to a known node URL or otherwise set `MINER_NO_GATEWAY` to true.

```bash
docker run --rm -it --gpus all -p 8000:8000 -p 8337:8337 -p 8339:8339 -e MINER_NO_GATEWAY=0 -e PEARLD_RPC_URL=http://172.17.0.1:44107/ -e HF_TOKEN=<TOKEN HERE>   -v /.cache/huggingface:/root/.cache/huggingface   --shm-size 8g   vllm_miner:latest   pearl-ai/Llama-3.1-8B-Instruct-pearl   --host 0.0.0.0   --port 8000   --max-model-len 8192   --gpu-memory-utilization 0.9 --enforce-eager
```

## Pushing to a Registry

To build and push a multi-platform image to a container registry, you can use `docker buildx`:

```bash
docker buildx build ../../ -f Dockerfile --tag <your-registry>/vllm_miner:latest --push
```

Replace `<your-registry>` with your container registry's URL.
