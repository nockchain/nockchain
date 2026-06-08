#!/usr/bin/env bash
set -euo pipefail

ADDR="${1:-127.0.0.1:8002}"  # multi-bridge node 0; others: 8003..8006

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/layout.sh
source "$SCRIPT_DIR/lib/layout.sh"
bridge_resolve_layout
PROTO_DIR="$BRIDGE_DIR/proto"

if [[ ! -f "$PROTO_DIR/bridge_ingress.proto" ]]; then
  echo "error: can't find $PROTO_DIR/bridge_ingress.proto" >&2
  exit 1
fi

BASE_HASH="$(python3 - <<'PY' | base64
import sys; sys.stdout.buffer.write(b'\x11'*40)
PY
)"
NOCK_HASH="$(python3 - <<'PY' | base64
import sys; sys.stdout.buffer.write(b'\x22'*40)
PY
)"

grpcurl -plaintext \
  -import-path "$PROTO_DIR" \
  -proto bridge_ingress.proto \
  -d "{
    \"sender_node_id\": 1,
    \"reason\": \"smoke stop\",
    \"last_base_hash\": \"${BASE_HASH}\",
    \"last_base_height\": 123,
    \"last_nock_hash\": \"${NOCK_HASH}\",
    \"last_nock_height\": 456,
    \"timestamp\": $(date +%s)
  }" \
  "${ADDR}" bridge.ingress.v1.BridgeIngress/BroadcastStop
