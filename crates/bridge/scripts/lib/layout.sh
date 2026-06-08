#!/usr/bin/env bash

# Shared path discovery for bridge helper scripts.
#
# The monorepo layout is:
#   <workspace>/open/crates/bridge
#
# The public repository layout is:
#   <workspace>/crates/bridge
#
# Keep all script entrypoints in terms of these resolved paths instead of
# spelling either layout directly.

bridge_resolve_layout() {
    BRIDGE_SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[1]}")" && pwd)"
    BRIDGE_DIR="$(cd "$BRIDGE_SCRIPT_DIR/.." && pwd)"
    BRIDGE_CRATES_DIR="$(cd "$BRIDGE_DIR/.." && pwd)"
    BRIDGE_SOURCE_ROOT="$(cd "$BRIDGE_CRATES_DIR/.." && pwd)"

    if [[ "$(basename "$BRIDGE_SOURCE_ROOT")" == "open" && -f "$(dirname "$BRIDGE_SOURCE_ROOT")/Cargo.toml" ]]; then
        BRIDGE_WORKSPACE_ROOT="$(cd "$BRIDGE_SOURCE_ROOT/.." && pwd)"
    else
        BRIDGE_WORKSPACE_ROOT="$BRIDGE_SOURCE_ROOT"
    fi

    BRIDGE_BIN_DIR="$BRIDGE_WORKSPACE_ROOT/target/release"
}
