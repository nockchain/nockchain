#!/bin/bash
source .env
export RUST_LOG
export MINIMAL_LOG_FORMAT
export MINING_PKH
nockchain --mine --fakenet --mining-pkh ${MINING_PKH} --peer /ip4/127.0.0.1/udp/3006/quic-v1  --no-default-peers
