#!/usr/bin/env bash
# Rejection-parity test for a multi-file c6 tree: both compilers must refuse
# to build the entry against the dependency root. Modeled on
# ../../rejection_test.sh; the verdict is artifact absence, since hoonc exits 0
# even when its build fails.
#
# usage: rejection_tree_test.sh <hoonc-runfile> <honk-runfile> <prelude-runfile> <entry-runfile> <deps-dir>
set -u

if [[ "$#" -ne 5 ]]; then
  echo "usage: $0 <hoonc> <honk> <prelude> <entry> <deps-dir>" >&2
  exit 2
fi

root="${TEST_SRCDIR}/${TEST_WORKSPACE}"
hoonc="${root}/$1"
honk="${root}/$2"
prelude="${root}/$3"
entry="${root}/$4"
deps="${root}/$5"

for f in "$hoonc" "$honk" "$prelude" "$entry" "$deps"; do
  [[ -e "$f" ]] || { echo "missing runfile: $f" >&2; exit 1; }
done

work="$(mktemp -d "${TEST_TMPDIR:-${TMPDIR:-/tmp}}/reject.XXXXXX")"
trap 'rm -rf "$work"' EXIT
export HOME="$work"
cd "$work"

timeout 120 "$hoonc" --new --data-dir "$work/hoonc-data" --arbitrary \
  --output "$work/ref.jam" "$entry" "$deps" \
  > "$work/hoonc.log" 2>&1 || true
# Guard against infra failures masquerading as rejections: hoonc must have
# loaded its kernel (it logs the kernel's build hash) before the build could
# fail. Some trees fail while hoonc reads the directory map (`+stab`), before
# it logs any parsing.
if ! grep -aqE "build-hash|parsing" "$work/hoonc.log"; then
  echo "hoonc never loaded its kernel — infra failure, not a verdict:" >&2
  tail -5 "$work/hoonc.log" >&2
  exit 1
fi

timeout 120 "$honk" --arbitrary --output "$work/nat.jam" \
  --prelude "$prelude" "$entry" "$deps" \
  > "$work/honk.log" 2>&1 || true

fail=0
if [[ -f "$work/ref.jam" ]]; then
  echo "hoonc ACCEPTED rejection tree ${4}" >&2
  fail=1
fi
if [[ -f "$work/nat.jam" ]]; then
  echo "honk ACCEPTED rejection tree ${4}" >&2
  fail=1
fi
if [[ "$fail" -ne 0 ]]; then
  exit 1
fi
grep -a -m1 -E "compile failed" "$work/honk.log" || true
