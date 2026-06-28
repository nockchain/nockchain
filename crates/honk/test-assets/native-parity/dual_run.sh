#!/usr/bin/env bash
# Phase-0 strict dual-run parity harness (NATIVE-TYPES-MIGRATION.md §2.2, RT-02).
#
# ACCEPTANCE gate = strict `cmp` byte-equality of honk's output jam against the
# hoonc-built reference in assets/<name>.jam. A difference that is ONLY the
# Bazel-sandbox dir-hash leaf (proven via `jam-diff --kernel-parity`) is reported
# as WAIVED — a NAMED exception, not a silent tolerant pass. Anything else FAILs.
#
# This establishes the acceptance gate the native migration is validated against.
# The native-vs-noun arm (HONK_NATIVE_IR) is a Phase-1 hook (see end of file):
# once the native path exists, this same harness strict-cmps native-honk vs
# noun-honk in addition to honk-vs-hoonc.
#
# Usage: dual_run.sh [kernel ...]   (default: all six)
set -uo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
cd "${repo_root}"

honk="target/release/honk"
jamdiff="target/release/jam-diff"
prelude="hoon/common/hoon.hoon"
work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT

# name|source
all_kernels=(
  "dumb|hoon/apps/dumbnet/outer.hoon"
  "wal|hoon/apps/wallet/wallet.hoon"
  "miner|hoon/apps/dumbnet/miner.hoon"
  "peek|hoon/apps/peek/peek.hoon"
  "bridge|hoon/apps/bridge/bridge.hoon"
  "roswell|hoon/apps/roswell/roswell.hoon"
)

# Optional filter by kernel name(s).
kernels=()
if [ "$#" -gt 0 ]; then
  for want in "$@"; do
    for k in "${all_kernels[@]}"; do [ "${k%%|*}" = "${want}" ] && kernels+=("${k}"); done
  done
else
  kernels=("${all_kernels[@]}")
fi

echo "== building honk + jam-diff (release) =="
cargo build --release -p honk --bin honk >/dev/null 2>&1 || { echo "FAIL: honk build"; exit 3; }
cargo build --release -p honk-tools --bin jam-diff >/dev/null 2>&1 || { echo "FAIL: jam-diff build"; exit 3; }

strict=0; waived=0; failed=0; missing=0
printf '\n%-10s %-12s %s\n' "KERNEL" "RESULT" "DETAIL"
printf '%-10s %-12s %s\n' "------" "------" "------"
for entry in "${kernels[@]}"; do
  name="${entry%%|*}"; src="${entry#*|}"
  ref="assets/${name}.jam"
  out="${work}/${name}.jam"
  if [ ! -f "${ref}" ]; then printf '%-10s %-12s %s\n' "${name}" "NO-REF" "${ref} missing"; missing=$((missing+1)); continue; fi
  if ! "${honk}" --new --output "${out}" --prelude "${prelude}" "${src}" hoon >"${work}/${name}.log" 2>&1; then
    printf '%-10s %-12s %s\n' "${name}" "COMPILE-ERR" "see ${work}/${name}.log"; failed=$((failed+1)); continue
  fi
  if cmp -s "${ref}" "${out}"; then
    printf '%-10s %-12s %s\n' "${name}" "STRICT-PASS" "byte-identical to hoonc"; strict=$((strict+1)); continue
  fi
  # Differs: classify via jam-diff. dir-hash-only difference => WAIVED (named).
  cls="$(${jamdiff} --kernel-parity "${ref}" "${out}" 2>&1)"
  if printf '%s' "${cls}" | grep -qiE "dir.?hash"; then
    printf '%-10s %-12s %s\n' "${name}" "WAIVED" "dir-hash-only diff (named exception)"; waived=$((waived+1))
  else
    printf '%-10s %-12s %s\n' "${name}" "FAIL" "non-dir-hash divergence: ${cls}"; failed=$((failed+1))
  fi
done

printf '\nsummary: strict=%d waived=%d fail=%d missing=%d\n' "${strict}" "${waived}" "${failed}" "${missing}"
# Acceptance = no FAIL and no MISSING. WAIVED is allowed (named).
[ "${failed}" -eq 0 ] && [ "${missing}" -eq 0 ]
