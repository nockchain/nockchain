#!/bin/bash
source .env
export RUST_LOG
export MINIMAL_LOG_FORMAT
export MINING_PKH
# Connects to the private gRPC port set in run_nockchain_node_fakenet.sh.
zk-pow-mine --node-addr http://127.0.0.1:5556 --mining-pkh ${MINING_PKH}
