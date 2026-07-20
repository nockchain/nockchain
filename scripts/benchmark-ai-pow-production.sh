#!/usr/bin/env bash
# Release gate for production compact-certificate size, prover wall time, and
# peak RSS. Each sample runs in a fresh test process.

set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

export RUSTFLAGS="${RUSTFLAGS:--C target-cpu=native}"

MAX_CERT_BYTES="${AI_POW_BENCH_MAX_CERT_BYTES:-150000}"
MAX_PROVE_SECONDS="${AI_POW_BENCH_MAX_PROVE_SECONDS:-30}"
PROTOCOL="${AI_POW_BENCH_PROTOCOL:-release}"
if [[ "$PROTOCOL" != "release" ]]; then
  printf 'unknown AI_POW_BENCH_PROTOCOL=%s (expected release)\n' "$PROTOCOL" >&2
  exit 2
fi
if [[ -n "${AI_POW_BENCH_SAMPLES+x}" ]]; then
  SAMPLES="$AI_POW_BENCH_SAMPLES"
else
  SAMPLES=3
fi
TIME_BIN="${TIME_BIN:-/usr/bin/time}"

case "$(uname -s)" in
  Darwin) TIME_ARGS=(-l) ;;
  *) TIME_ARGS=(-v) ;;
esac

printf '== AI-PoW production proof benchmark ==\n'
printf 'host: %s %s\n' "$(uname -srm)" "$(sysctl -n machdep.cpu.brand_string 2>/dev/null || true)"
printf 'rustflags: %s\n' "$RUSTFLAGS"
printf 'protocol: %s\n' "$PROTOCOL"
printf 'limits: proof_bytes < %s, prove_seconds < %s, samples_per_cell=%s\n' \
  "$MAX_CERT_BYTES" "$MAX_PROVE_SECONDS" "$SAMPLES"
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

parse_sample() {
  local label=$1
  local kind=$2
  local sample=$3
  local logfile=$4

  python3 - "$kind" "$MAX_CERT_BYTES" "$MAX_PROVE_SECONDS" "$sample" "$label" "$logfile" <<'PY'
import pathlib
import re
import sys

kind, max_bytes_s, max_seconds_s, sample, label, path = sys.argv[1:]
max_bytes = int(max_bytes_s)
max_seconds = float(max_seconds_s)
text = pathlib.Path(path).read_text(errors="replace")

if kind == "dense":
    m = re.search(r"compact_cert=(\d+) bytes.*?prove_wall_ms=(\d+)", text, re.S)
    if not m:
        raise SystemExit(f"{label} sample {sample}: missing dense compact_cert/prove_wall_ms line")
    proof_bytes = int(m.group(1))
    prove_seconds = int(m.group(2)) / 1000.0
elif kind == "moe":
    m = re.search(r"canonical MoE prove:\s*([0-9.]+)s\s+compact_cert_bytes=(\d+)", text)
    if not m:
        raise SystemExit(f"{label} sample {sample}: missing MoE prove line")
    prove_seconds = float(m.group(1))
    proof_bytes = int(m.group(2))
else:
    raise SystemExit(f"unknown benchmark kind: {kind}")

rss_bytes = None
darwin_rss = re.findall(r"^\s*(\d+)\s+maximum resident set size$", text, re.M)
linux_rss = re.findall(r"Maximum resident set size.*:\s*(\d+)", text)
if darwin_rss:
    rss_bytes = int(darwin_rss[-1])
elif linux_rss:
    rss_bytes = int(linux_rss[-1]) * 1024

errors = []
if proof_bytes >= max_bytes:
    errors.append(f"proof_bytes {proof_bytes} >= {max_bytes}")
if prove_seconds >= max_seconds:
    errors.append(f"prove_seconds {prove_seconds:.3f} >= {max_seconds:.3f}")

rss = "unknown" if rss_bytes is None else str(rss_bytes)
print(
    f"{label} sample {sample}: proof_bytes={proof_bytes} "
    f"prove_seconds={prove_seconds:.3f} peak_rss_bytes={rss}"
)

if errors:
    raise SystemExit(f"{label} sample {sample} failed release budget: " + "; ".join(errors))
PY
}

run_benchmark() {
  local label=$1
  local kind=$2
  local filter=$3

  printf '\n== %s ==\n' "$label"
  for sample in $(seq 1 "$SAMPLES"); do
    printf '\n-- sample %s/%s --\n' "$sample" "$SAMPLES"
    local logfile
    logfile="$(mktemp "${TMPDIR:-/tmp}/ai-pow-bench.XXXXXX")"
    set +e
    "$TIME_BIN" "${TIME_ARGS[@]}" "$TEST_BIN" "$filter" --ignored --nocapture --test-threads=1 \
      >"$logfile" 2>&1
    local status=$?
    set -e
    cat "$logfile"
    if (( status != 0 )); then
      rm -f "$logfile"
      return "$status"
    fi
    parse_sample "$label" "$kind" "$sample" "$logfile"
    rm -f "$logfile"
  done
}

run_benchmark \
  "dense production compact proof" \
  "dense" \
  "real_compact_pearl_merge_prod_scale_m_size_and_latency"
run_benchmark \
  "canonical MoE miner proof" \
  "moe" \
  "canonical_mining_costs"

printf '\nPASS: every sample met proof_bytes < %s and prove_seconds < %s.\n' \
  "$MAX_CERT_BYTES" "$MAX_PROVE_SECONDS"
printf 'Peak RSS is measured by %s %s for each direct test process.\n' \
  "$TIME_BIN" "${TIME_ARGS[*]}"
