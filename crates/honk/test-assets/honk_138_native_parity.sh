#!/usr/bin/env bash
#
# Arbitrary-build parity for hoon-138: does honk's NATIVE mint of the hoon-138
# prelude byte-match hoonc's arbitrary build of the same source?
#
# honk normally embeds hoonc's precompiled hoon-138 (formula+type). Setting
# HONK_NATIVE_PARITY=1 disables that substitution so honk mints the prelude
# itself — the build this test exercises.
#
# STATUS (verified 2026-06-28): this is a PASSING byte-parity gate. honk's
# native mint of the full hoon-138 prelude now COMPLETES with BOUNDED memory
# (~40 s in release) and the resulting artifact is BYTE-IDENTICAL to hoonc's
# arbitrary build (2,286,744 bytes, cmp-clean). The old "~4 GB/min linear
# growth / 49-min OOM / does not complete" behavior is RESOLVED — the memory
# work (mack-cache cap, frame arena, bottom-up interning) plus the faithful
# ++vast doc-anchoring port bounded it and reached parity. The honk side is
# still run under an RSS guard as a cheap backstop against pathological
# regressions; it is not expected to fire in normal operation.
#
# NOTE: the embedded hoonc prelude (formula+type) is still the default for the
# canonical build purely as a SPEED cache (~instant cue vs ~40 s mint), not a
# correctness crutch. Native mint is proven byte-identical here; it is simply
# not the build-path default.
#
# hoon/common/hoon.hoon is a symlink to crates/hoonc/hoon/hoon-138.hoon, so
# both compilers see identical source; any artifact diff is compiler behavior,
# not source drift.
set -uo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "${repo_root}"

honc="target/release/hoonc"
honk="target/release/honk"
entry="hoon/common/hoon.hoon"
deps="hoon"
work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT

# RSS backstop for the honk native mint: 75% of physical RAM. Native mint is
# bounded and completes well under this in normal operation; this only trips
# on a pathological memory regression, self-aborting with headroom instead of
# triggering the OS OOM killer.
mem_total_kb=$(( $(sysctl -n hw.memsize) / 1024 ))
mem_cap_kb=$(( mem_total_kb * 3 / 4 ))

echo "== hoonc arbitrary hoon-138 (reference) =="
"${honc}" --new --data-dir "${work}/hoonc-data" --arbitrary --output "${work}/ref.jam" "${entry}" "${deps}"
# hoonc writes the output to the basename in CWD; normalize location.
[ -f ref.jam ] && mv ref.jam "${work}/ref.jam"
if [ ! -f "${work}/ref.jam" ]; then
  echo "FAIL: hoonc did not produce a reference artifact" >&2
  exit 1
fi

echo "== honk NATIVE arbitrary hoon-138 (candidate, RSS-guarded) =="
HONK_NATIVE_PARITY=1 "${honk}" --arbitrary --output "${work}/native.jam" --prelude "${entry}" "${entry}" "${deps}" &
honk_pid=$!
start=$(date +%s)
while kill -0 "${honk_pid}" 2>/dev/null; do
  rss_kb=$(ps -o rss= -p "${honk_pid}" 2>/dev/null | tr -d ' ')
  [ -z "${rss_kb}" ] && break
  if [ "${rss_kb}" -gt "${mem_cap_kb}" ]; then
    elapsed=$(( $(date +%s) - start ))
    kill -9 "${honk_pid}" 2>/dev/null
    awk -v k="${rss_kb}" -v t="${elapsed}" 'BEGIN{printf "REGRESSION: honk native mint hit %.1f GB at %ds — exceeded the RSS backstop\n", k/1048576, t}' >&2
    echo "  Native mint is normally bounded and completes in ~40 s; this indicates a pathological memory regression, not expected behavior." >&2
    exit 2
  fi
  sleep 5
done
wait "${honk_pid}"
honk_rc=$?
if [ "${honk_rc}" -ne 0 ] || [ ! -f "${work}/native.jam" ]; then
  echo "FAIL: honk native mint exited ${honk_rc} without an artifact" >&2
  exit 1
fi

echo "== byte-exact compare =="
if cmp -s "${work}/native.jam" "${work}/ref.jam"; then
  echo "PASS (exact): honk native hoon-138 == hoonc arbitrary hoon-138"
  exit 0
else
  echo "FAIL: honk native hoon-138 differs from hoonc arbitrary hoon-138" >&2
  echo "  ref    $(shasum -a 256 "${work}/ref.jam" | awk '{print $1}')" >&2
  echo "  native $(shasum -a 256 "${work}/native.jam" | awk '{print $1}')" >&2
  exit 1
fi
