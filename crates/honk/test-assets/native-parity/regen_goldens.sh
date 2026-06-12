#!/usr/bin/env bash
# Phase-0 emitted-formula golden corpus (NATIVE-TYPES-MIGRATION.md Phase 0 / §2.1).
#
# Captures the CURRENT (noun) honk output jam for each expression fixture as a
# golden under goldens/. In Phase 1 the native Formula IR's `to_noun` must
# reproduce these byte-for-byte (strict `cmp`) — they are the reference for the
# de-risking first slice. Re-run to refresh after an intentional honk change
# (review the diff!). dbug on/off variants are captured because dbug is a
# phase-wide, byte-affecting input (RT-15).
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${here}/../../../.." && pwd)"
cd "${repo_root}"
honk="target/release/honk"
prelude="hoon/common/hoon.hoon"

cargo build --release -p honk --bin honk >/dev/null 2>&1 || { echo "FAIL: honk build"; exit 3; }

mkdir -p "${here}/goldens"
check="${1:-regen}"   # "regen" writes goldens; "check" strict-cmps against them
fail=0
for src in "${here}"/exprs/*.hoon; do
  base="$(basename "${src}" .hoon)"
  for mode in nodbug dbug; do
    flag="--no-dbug"; [ "${mode}" = dbug ] && flag="--dbug"
    # --arbitrary: these fixtures are bare expressions, not gate-shaped kernels,
    # so they use the arbitrary-expression output path (same as the native-parity
    # mint). End-to-end goldens; fine-grained per-construct formula parity is a
    # Phase-1 unit fixture (build noun formula, cmp to native to_noun).
    out="$(mktemp)"
    if ! "${honk}" --new --arbitrary ${flag} --output "${out}" --prelude "${prelude}" "${src}" hoon >/dev/null 2>&1; then
      echo "COMPILE-ERR ${base} (${mode})"; fail=$((fail+1)); rm -f "${out}"; continue
    fi
    golden="${here}/goldens/${base}.${mode}.jam"
    if [ "${check}" = check ]; then
      if cmp -s "${golden}" "${out}"; then echo "OK    ${base} (${mode})"
      else echo "DIFF  ${base} (${mode})  vs ${golden}"; fail=$((fail+1)); fi
    else
      cp "${out}" "${golden}"; echo "wrote ${base}.${mode}.jam ($(wc -c < "${golden}") bytes)"
    fi
    rm -f "${out}"
  done
done
[ "${fail}" -eq 0 ]
