#!/bin/bash
set -e

# Auto-detect GPUs and set CUDA_VISIBLE_DEVICES if not already set
if [ -z "$CUDA_VISIBLE_DEVICES" ]; then
    GPU_COUNT=$(nvidia-smi -L | wc -l)
    if [ "$GPU_COUNT" -gt 1 ]; then
        CUDA_VISIBLE_DEVICES=$(seq -s, 0 $((GPU_COUNT - 1)))
        export CUDA_VISIBLE_DEVICES
        echo "Auto-detected $GPU_COUNT GPUs, setting CUDA_VISIBLE_DEVICES=$CUDA_VISIBLE_DEVICES"
    fi
fi

echo "Starting pearl-gateway..."
pearl-gateway start &
PEARL_PID=$!

# Wait until the gateway's miner RPC socket is ready (the gateway exposes a Unix
# socket, not an HTTP endpoint), failing fast if the gateway dies during startup.
GATEWAY_SOCKET="${MINER_RPC_SOCKET_PATH:-/tmp/pearlgw.sock}"
echo "Waiting for pearl-gateway socket at $GATEWAY_SOCKET ..."
for _ in $(seq 1 30); do
    if [ -S "$GATEWAY_SOCKET" ]; then
        break
    fi
    if ! kill -0 "$PEARL_PID" 2>/dev/null; then
        echo "pearl-gateway exited before becoming ready" >&2
        exit 1
    fi
    sleep 1
done
if [ ! -S "$GATEWAY_SOCKET" ]; then
    echo "Timed out waiting for pearl-gateway socket at $GATEWAY_SOCKET" >&2
    exit 1
fi

echo "Starting vllm serve with args: $@"
exec vllm serve "$@"
