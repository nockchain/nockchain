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

echo "Starting nockchain miner with $num_threads mining threads:"

nockchain --mining-pkh ${MINING_PKH} --mine --num-threads $num_threads
