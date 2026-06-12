#!/usr/bin/env bash
set -euo pipefail

root="${TEST_SRCDIR}/${TEST_WORKSPACE}"
honk="${root}/open/crates/honk/honk"
prelude="${root}/open/hoon/common/hoon.hoon"
deps_dir="${root}/open/hoon"
expected_cold="${root}/open/assets/honc-cold-138.jam"
dynamic_dir="${TEST_TMPDIR}/dynamic-native-wrappers"
native_dir="${TEST_TMPDIR}/constructed-native-wrappers"

"${honk}" --dump-wrapper-assets "${dynamic_dir}" --prelude "${prelude}" "${deps_dir}"
"${honk}" --dump-native-wrapper-assets "${native_dir}" --prelude "${prelude}" "${deps_dir}"

failures=0
for asset in \
  constant-vase-battery.jam \
  data-vase-battery.jam \
  dir-hash-vase-battery.jam \
  empty-trap-vase.jam \
  eval-vase-battery.jam \
  label-vase-battery.jam \
  slat-battery.jam \
  shot-battery.jam \
  swet-gun-battery.jam \
  value-trap-arbitrary-battery.jam \
  value-trap-standard-battery.jam
 do
  if ! cmp -s "${dynamic_dir}/${asset}" "${native_dir}/${asset}"; then
    echo "native wrapper constructor parity mismatch: ${asset}" >&2
    failures=$((failures + 1))
  fi
done

if ! cmp -s "${expected_cold}" "${dynamic_dir}/honc-cold-138.jam"; then
  echo "native cold-state generated-asset parity mismatch: honc-cold-138.jam" >&2
  failures=$((failures + 1))
fi

if [[ "${failures}" -ne 0 ]]; then
  echo "${failures} native wrapper/cold asset parity comparison(s) failed" >&2
  exit 1
fi
