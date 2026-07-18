#!/usr/bin/env bash
# Measure production compact-certificate size, prover wall/CPU time, and peak RSS.
# Runs one dense Pearl-compatible proof and one canonical MoE miner proof.

set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

export RUSTFLAGS="${RUSTFLAGS:--C target-cpu=native}"

printf '== AI-PoW production proof benchmark ==\n'
printf 'host: %s %s\n' "$(uname -srm)" "$(sysctl -n machdep.cpu.brand_string 2>/dev/null || true)"
printf 'rustflags: %s\n' "$RUSTFLAGS"
printf 'fixtures:\n'
printf '  dense: m=512 k=1024 n=512 rank=64 tile=8\n'
printf '  MoE:   m=64 k=1024 n_e=64 total_n=128 rank=64 tile=8 experts=2 top_k=1\n\n'

TEST_BIN="$({
  cargo test --release -p ai-pow-miner --features node --lib --no-run --message-format=json
} | python3 -c '
import json, sys
matches = []
for line in sys.stdin:
    try:
        msg = json.loads(line)
    except json.JSONDecodeError:
        continue
    if msg.get("reason") != "compiler-artifact":
        continue
    target = msg.get("target", {})
    executable = msg.get("executable")
    if target.get("name") == "ai_pow_miner" and "lib" in target.get("kind", []) and executable:
        matches.append(executable)
if len(matches) != 1:
    raise SystemExit(f"expected one ai_pow_miner lib test binary, found {matches}")
print(matches[0])
')"

run_benchmark() {
  local label=$1
  local filter=$2
  printf '\n== %s ==\n' "$label"
  /usr/bin/time -l "$TEST_BIN" "$filter" --ignored --nocapture --test-threads=1 2>&1
}

run_benchmark \
  "dense production compact proof" \
  "real_compact_pearl_merge_prod_scale_m_size_and_latency"
run_benchmark \
  "canonical MoE miner proof" \
  "canonical_mining_costs"

printf '\nPeak RSS is the `maximum resident set size` emitted for each direct test process.\n'
printf 'CPU time is the `user` plus `system` time; prover wall time and proof bytes are emitted by the tests.\n'
