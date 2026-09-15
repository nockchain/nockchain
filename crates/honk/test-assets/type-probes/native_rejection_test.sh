#!/usr/bin/env bash
# Native-only rejection test for inputs on which the reference compiler does
# not terminate within a useful test budget.
set -u

if [[ "$#" -ne 3 ]]; then
  echo "usage: $0 <honk> <prelude> <probe>" >&2
  exit 2
fi

root="${TEST_SRCDIR}/${TEST_WORKSPACE}"
honk="${root}/$1"
prelude="${root}/$2"
probe="${root}/$3"

for f in "$honk" "$prelude" "$probe"; do
  [[ -e "$f" ]] || { echo "missing runfile: $f" >&2; exit 1; }
done

work="$(mktemp -d "${TEST_TMPDIR:-${TMPDIR:-/tmp}}/native-reject.XXXXXX")"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/empty-deps"

timeout 30 "$honk" --arbitrary --output "$work/output.jam" \
  --prelude "$prelude" "$probe" "$work/empty-deps" \
  > "$work/honk.log" 2>&1
status=$?

if [[ "$status" -eq 124 ]]; then
  echo "honk timed out instead of rejecting the probe" >&2
  exit 1
fi
if [[ -f "$work/output.jam" ]]; then
  echo "honk accepted native rejection probe $3" >&2
  exit 1
fi
if ! grep -aq "musk-loop" "$work/honk.log"; then
  echo "honk rejected without the expected musk-loop diagnostic:" >&2
  tail -10 "$work/honk.log" >&2
  exit 1
fi
