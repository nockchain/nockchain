#!/usr/bin/env bash
# Fast regression gate for the native-types ATOMIC FLIP.
#
# Once producers RETURN native unconditionally (the flip), the HONK_NATIVE_TYPES
# flag no longer changes output, so on/off comparison is meaningless. Instead we
# compare each fixture's compiled output against a FIXED pre-flip baseline
# (flip-baselines/*.jam — the hoonc-parity output captured before the flip began).
# Byte-identical output is the invariant the whole flip must preserve.
#
# These fixtures are tiny (~1-2s each), so this is the routine per-step gate.
# Regenerate baselines ONLY intentionally (REGEN=1) when output legitimately
# changes — which during the flip it must NOT.
#
# Usage: crates/honk/test-assets/native-parity/shadow_gate.sh [honk_binary]
#        REGEN=1 crates/honk/test-assets/native-parity/shadow_gate.sh   # rebuild baselines
set -u
HONK="${1:-target/release/honk}"
PRELUDE="hoon/common/hoon.hoon"
DIR="crates/honk/test-assets/native-parity/exprs"
BASE="crates/honk/test-assets/native-parity/flip-baselines"
FIXTURES="core_chain fork loop_dec wet_turn"
TMP="${TMPDIR:-/tmp}"
ok=1
for f in $FIXTURES; do
  src="$DIR/$f.hoon"
  if [ "${REGEN:-0}" = 1 ]; then
    timeout 120 "$HONK" --new --arbitrary --output "$BASE/$f.jam" --prelude "$PRELUDE" "$src" hoon >/dev/null 2>&1 \
      && echo "  $f: baseline regenerated" || { echo "  $f: REGEN FAILED"; ok=0; }
    continue
  fi
  out="$TMP/sg_$f.jam"; log="$TMP/sg_$f.log"
  timeout 120 "$HONK" --new --arbitrary --output "$out" --prelude "$PRELUDE" "$src" hoon > "$log" 2>&1
  if grep -qiE "panic|native shadow mismatch" "$log"; then echo "  $f: PANIC/ASSERT"; ok=0
  elif cmp -s "$BASE/$f.jam" "$out"; then echo "  $f: matches baseline OK"
  else echo "  $f: REGRESSED vs baseline"; ok=0; fi
done
if [ "$ok" = 1 ]; then echo "flip gate: PASS"; else echo "flip gate: FAIL"; exit 1; fi
