#!/usr/bin/env bash
# Profile / benchmark the ai-pow → ai-pow-zk F1 harness.
#
# Usage:
#   scripts/profile_f1.sh build                 # release build only
#   scripts/profile_f1.sh run   [ITERS]         # run + print metrics line
#   scripts/profile_f1.sh samply [ITERS]        # samply CPU profile (Firefox Profiler)
#   scripts/profile_f1.sh rss   [ITERS]         # peak resident-set-size
#   scripts/profile_f1.sh all   [ITERS]         # run + rss + samply
#
# ITERS (default 1) sets F1_ITERS — the number of prove+verify
# iterations. Use 3-5 for samply so the prover dominates the
# sample population; use 1 for a quick metrics read.
#
# Env passthrough: F1_SEED (matrix synth seed).
#
# See crates/ai-pow-zk/docs/2026-05-15_PROFILING.md for how to read the output.

set -euo pipefail

CRATE=ai-pow
EXAMPLE=f1_harness
BIN="target/release/examples/${EXAMPLE}"
CMD="${1:-run}"
ITERS="${2:-1}"

cd "$(git rev-parse --show-toplevel)"

build() {
  echo ">> cargo build -p ${CRATE} --release --features zk --example ${EXAMPLE}"
  cargo build -p "${CRATE}" --release --features zk --example "${EXAMPLE}"
}

need_bin() { [[ -x "${BIN}" ]] || build; }

case "${CMD}" in
  build)
    build
    ;;

  run)
    need_bin
    F1_ITERS="${ITERS}" "${BIN}"
    ;;

  samply)
    need_bin
    command -v samply >/dev/null || {
      echo "samply not found. Install: cargo install samply" >&2
      exit 1
    }
    OUT="f1_profile_$(date +%Y%m%d_%H%M%S).json.gz"
    echo ">> samply record (ITERS=${ITERS}) -> ${OUT}"
    # --save-only: write the profile without auto-opening the
    # browser (drop it to open the Firefox Profiler immediately).
    F1_ITERS="${ITERS}" samply record --save-only -o "${OUT}" "${BIN}"
    echo ">> wrote ${OUT}"
    echo "   View: samply load ${OUT}   (opens Firefox Profiler)"
    ;;

  rss)
    need_bin
    echo ">> peak resident-set-size (ITERS=${ITERS})"
    case "$(uname -s)" in
      Darwin)
        # macOS /usr/bin/time -l prints "maximum resident set size" in BYTES.
        F1_ITERS="${ITERS}" /usr/bin/time -l "${BIN}" 2>&1 \
          | awk '/maximum resident set size/ {printf "peak_rss_bytes=%s (%.1f MiB)\n",$1,$1/1048576}'
        ;;
      Linux)
        # GNU time -v prints "Maximum resident set size (kbytes)".
        F1_ITERS="${ITERS}" /usr/bin/time -v "${BIN}" 2>&1 \
          | awk -F': ' '/Maximum resident set size/ {printf "peak_rss_kb=%s (%.1f MiB)\n",$2,$2/1024}'
        ;;
      *)
        echo "unsupported OS for rss recipe; run under your platform profiler" >&2
        exit 1
        ;;
    esac
    ;;

  all)
    need_bin
    F1_ITERS="${ITERS}" "${BIN}"
    "$0" rss "${ITERS}"
    "$0" samply "${ITERS}"
    ;;

  *)
    echo "unknown command: ${CMD}" >&2
    sed -n '2,18p' "$0"
    exit 2
    ;;
esac
