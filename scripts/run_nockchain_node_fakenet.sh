#!/bin/bash
source .env
export RUST_LOG
export MINIMAL_LOG_FORMAT
export MINING_PUBKEY
nockchain --fakenet --grpc-address http://127.0.0.1:5555 --bind /ip4/127.0.0.1/udp/3006/quic-v1
