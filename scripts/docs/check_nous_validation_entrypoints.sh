#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
OPEN_ROOT="$(CDPATH= cd -- "${SCRIPT_DIR}/../.." && pwd)"
cd "$OPEN_ROOT"

SPEC_FILE="changelog/protocol/013-nous.md"
failures=0

require_pattern() {
    label="$1"
    pattern="$2"

    if ! rg -q "$pattern" "$SPEC_FILE"; then
        printf '  - %s: missing `%s`\n' "$SPEC_FILE" "$label"
        failures=1
    fi
}

if [ ! -f "$SPEC_FILE" ]; then
    printf '  - %s: file not found\n' "$SPEC_FILE"
    printf 'nous validation entrypoint check: FAIL\n'
    exit 1
fi

require_pattern "gen2 protocol ID" '/nockchain-2-req-res'
require_pattern "gen2 full support" 'full inbound and outbound support'
require_pattern "gen1 registration removed" 'Generation 1 is not registered'
require_pattern "legacy gossip removed" 'unauthenticated legacy `Gossip` wire shape'
require_pattern "authenticated gossip schema" 'AuthenticatedGossip'
require_pattern "gen2 request schema" 'BatchRequest'
require_pattern "accept flag removal" 'req_res_gen2_accept_enabled.*removed'
require_pattern "send flag removal" 'req_res_gen2_send_enabled.*removed'
require_pattern "no runtime rollback" 'no runtime rollback flag'

if [ "$failures" -ne 0 ]; then
    printf 'nous validation entrypoint check: FAIL\n'
    exit 1
fi

printf 'nous validation entrypoint check: PASS\n'
