#!/usr/bin/env bash
# Deterministic branch-local ASERT regression gate for the Logos dual-puzzle chain.
# The live miner smoke covers proof-bearing acceptance; these Hoon tests control
# timestamps and puzzle ordering so both retarget directions are exact and repeatable.

set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

make assets/roswell.jam
cargo build --release -p roswell --bin roswell

LOG_DIR="$(mktemp -d -t dual-pow-retarget.XXXXXX)"
trap 'rm -rf "$LOG_DIR"' EXIT

TEST_PREFIXES=(
    test-tandem-asert
    test-ai-asert-ignores-interleaved-zk
    test-zk-asert-ignores-interleaved-ai
    test-build-ai-candidate-retargets
    test-ai-block-accepted-post-asert
)

for prefix in "${TEST_PREFIXES[@]}"; do
    echo "[run  ] $prefix"
    target/release/roswell --new --ephemeral run-test "$prefix" \
        2>&1 | tee "$LOG_DIR/$prefix.log"
    if grep -q "could not match parent battery at given axis" "$LOG_DIR/$prefix.log"; then
        echo "[fail ] dangling jet-hint ancestry while running $prefix" >&2
        exit 1
    fi
    echo "[ok   ] $prefix"
done

echo "[ok   ] independent ZK/AI ASERT lineage and post-ASERT acceptance"
