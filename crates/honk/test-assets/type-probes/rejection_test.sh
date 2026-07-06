#!/usr/bin/env bash
# Rejection-parity test: an ill-typed probe must be rejected by BOTH
# compilers. The verdict is artifact absence — hoonc exits 0 even when its
# build fails, so exit codes prove nothing. A probe both compilers accept
# (or that only one rejects) fails the test.
#
# usage: rejection_test.sh <hoonc-runfile> <honk-runfile> <prelude-runfile> <probe-runfile>
set -u

if [[ "$#" -ne 4 ]]; then
  echo "usage: $0 <hoonc> <honk> <prelude> <probe>" >&2
  exit 2
fi

root="${TEST_SRCDIR}/${TEST_WORKSPACE}"
hoonc="${root}/$1"
honk="${root}/$2"
prelude="${root}/$3"
probe="${root}/$4"

for f in "$hoonc" "$honk" "$prelude" "$probe"; do
  [[ -e "$f" ]] || { echo "missing runfile: $f" >&2; exit 1; }
done

work="$(mktemp -d "${TEST_TMPDIR:-${TMPDIR:-/tmp}}/reject.XXXXXX")"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/empty-deps"
export HOME="$work"
cd "$work"

timeout 120 "$hoonc" --new --data-dir "$work/hoonc-data" --arbitrary \
  --output "$work/ref.jam" "$probe" "$work/empty-deps" \
  > "$work/hoonc.log" 2>&1 || true
# Guard against infra failures masquerading as rejections: hoonc must have
# at least reached its parse phase on the probe (a missing file or bad boot
# also yields no artifact, but proves nothing about type checking). A
# rejection during parse (e.g. empty =~) or a crash during compile (e.g.
# stack overflow on a divergent type) still counts as a verdict.
if ! grep -aq "parsing" "$work/hoonc.log"; then
  echo "hoonc never reached its parse phase — infra failure, not a verdict:" >&2
  tail -5 "$work/hoonc.log" >&2
  exit 1
fi

timeout 120 "$honk" --arbitrary --output "$work/nat.jam" \
  --prelude "$prelude" "$probe" "$work/empty-deps" \
  > "$work/honk.log" 2>&1 || true

fail=0
if [[ -f "$work/ref.jam" ]]; then
  echo "hoonc ACCEPTED rejection probe ${4}" >&2
  fail=1
fi
if [[ -f "$work/nat.jam" ]]; then
  echo "honk ACCEPTED rejection probe ${4}" >&2
  grep -a -m1 -E "compile failed|error" "$work/honk.log" >&2 || true
  fail=1
fi
if [[ "$fail" -ne 0 ]]; then
  exit 1
fi
