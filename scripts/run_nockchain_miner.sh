#!/bin/bash
source .env
export RUST_LOG
export MINIMAL_LOG_FORMAT
export MINING_PKH

get_cpu_count() {
    if [[ "$OSTYPE" == "darwin"* ]]; then
        # macOS
        sysctl -n hw.logicalcpu
    else
        # Linux (Ubuntu, etc.)
        nproc
    fi
}

# Get total CPU hyperthreads
total_threads=$(get_cpu_count)

# Subtract 2
threads=$((total_threads - 2))

# minimum 1
num_threads=$((threads > 1 ? threads : 1))

echo "Starting zk-pow-mine with $num_threads mining threads:"

# Mining runs in its own process. Start the node first
# (scripts/run_nockchain_node.sh) and wait for it to finish starting up;
# zk-pow-mine connects to its private gRPC port, 127.0.0.1:5555 by default.
zk-pow-mine --mining-pkh ${MINING_PKH} --num-threads $num_threads
