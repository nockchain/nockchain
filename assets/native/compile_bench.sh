#!/usr/bin/env bash
# Uncached compile benchmark: hoonc vs honk on the same build.
#
# Usage: compile_bench.sh <label> <mode> <hoonc> <honk> <entry> <deps-dir>
#   label:    human-readable name for the report (e.g. "dumb")
#   mode:     "kernel" (--new) or "arbitrary" (vase build / native self-mint)
#   hoonc:    workspace-rooted path to the hoonc binary
#   honk:     workspace-rooted path to the honk binary
#   entry:    workspace-rooted path to the entry .hoon file
#   deps-dir: workspace-rooted path to the hoon dependency tree
#
# Every iteration is a cold build: hoonc gets a freshly-created --data-dir and
# honk keeps no on-disk state, so nothing is reused between runs (the point is
# to measure uncached compilation). Iteration count comes from
# BENCH_ITERATIONS (default 1); pass it under Bazel with
#   bazel test --test_env=BENCH_ITERATIONS=3 --test_output=streamed <target>
# or run interactively with
#   bazel run <target>
set -euo pipefail

if [[ "$#" -ne 6 ]]; then
  echo "usage: $0 <label> <kernel|arbitrary> <hoonc> <honk> <entry> <deps-dir>" >&2
  exit 2
fi

label="$1"
mode="$2"
hoonc_rel="$3"
honk_rel="$4"
entry_rel="$5"
deps_rel="$6"

# Under `bazel test` the runfiles root is TEST_SRCDIR/TEST_WORKSPACE; under
# `bazel run` the working directory already is the runfiles workspace dir.
if [[ -n "${TEST_SRCDIR:-}" ]]; then
  root="${TEST_SRCDIR}/${TEST_WORKSPACE}"
else
  root="$(pwd)"
fi

hoonc="${root}/${hoonc_rel}"
honk="${root}/${honk_rel}"
entry="${root}/${entry_rel}"
deps="${root}/${deps_rel}"
prelude="${root}/${deps_rel}/common/hoon.hoon"

for f in "$hoonc" "$honk" "$entry" "$prelude"; do
  [[ -e "$f" ]] || { echo "missing runfile: $f" >&2; exit 1; }
done

iterations="${BENCH_ITERATIONS:-1}"
work="$(mktemp -d "${TEST_TMPDIR:-${TMPDIR:-/tmp}}/compile-bench.XXXXXX")"
trap 'rm -rf "$work"' EXIT
# hoonc writes a copy of the artifact to the output basename in CWD; run from
# the scratch dir so nothing lands in the runfiles tree.
cd "$work"

now_ms() {
  if command -v python3 >/dev/null 2>&1; then
    python3 -c 'import time; print(int(time.time() * 1000))'
  else
    echo $(($(date +%s) * 1000))
  fi
}

fmt_s() { # ms -> "123.4s"
  awk -v ms="$1" 'BEGIN { printf "%.1fs", ms / 1000 }'
}

run_timed() { # <log-file> <cmd...> -> echoes elapsed ms; fails loudly
  local log="$1"
  shift
  local t0 t1
  t0="$(now_ms)"
  if ! "$@" >"$log" 2>&1; then
    echo "command failed: $*" >&2
    tail -20 "$log" >&2
    exit 1
  fi
  t1="$(now_ms)"
  echo $((t1 - t0))
}

hoonc_best=""
honk_best=""

echo "== uncached compile benchmark: ${label} (${mode}) =="
echo "   entry: ${entry_rel}   iterations: ${iterations}"

for i in $(seq 1 "$iterations"); do
  rm -rf "$work/hoonc-data" "$work/ref.jam" "$work/native.jam"

  if [[ "$mode" == "arbitrary" ]]; then
    hoonc_ms="$(run_timed "$work/hoonc.log" \
      "$hoonc" --new --data-dir "$work/hoonc-data" --arbitrary \
      --output "$work/ref.jam" "$entry" "$deps")"
    honk_ms="$(run_timed "$work/honk.log" \
      env HONK_NATIVE_PARITY=1 "$honk" --arbitrary \
      --output "$work/native.jam" --prelude "$entry" "$entry" "$deps")"
  else
    hoonc_ms="$(run_timed "$work/hoonc.log" \
      "$hoonc" --new --data-dir "$work/hoonc-data" \
      --output "$work/ref.jam" "$entry" "$deps")"
    honk_ms="$(run_timed "$work/honk.log" \
      "$honk" --new --output "$work/native.jam" --prelude "$prelude" \
      "$entry" "$deps")"
  fi

  ratio="$(awk -v a="$hoonc_ms" -v b="$honk_ms" 'BEGIN { printf "%.2f", a / b }')"
  echo "   iteration ${i}:  hoonc $(fmt_s "$hoonc_ms")   honk $(fmt_s "$honk_ms")   (hoonc/honk ${ratio}x)"

  [[ -z "$hoonc_best" || "$hoonc_ms" -lt "$hoonc_best" ]] && hoonc_best="$hoonc_ms"
  [[ -z "$honk_best" || "$honk_ms" -lt "$honk_best" ]] && honk_best="$honk_ms"
done

ref_size="$(wc -c <"$work/ref.jam" | tr -d ' ')"
native_size="$(wc -c <"$work/native.jam" | tr -d ' ')"
if cmp -s "$work/ref.jam" "$work/native.jam"; then
  parity="byte-identical"
else
  parity="DIFFER (${ref_size} B vs ${native_size} B)"
fi

speed="$(awk -v a="$hoonc_best" -v b="$honk_best" 'BEGIN { printf "%.2f", a / b }')"
faster="$(awk -v a="$hoonc_best" -v b="$honk_best" 'BEGIN { print (b <= a) ? "honk" : "hoonc" }')"

echo "-- summary: ${label} (best of ${iterations}) --"
echo "   hoonc  $(fmt_s "$hoonc_best")   (artifact ${ref_size} B)"
echo "   honk   $(fmt_s "$honk_best")   (artifact ${native_size} B)"
echo "   hoonc/honk: ${speed}x  (${faster} is faster)"
echo "   artifacts: ${parity}"
