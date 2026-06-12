#!/usr/bin/env bash
set -euo pipefail

root="${TEST_SRCDIR}/${TEST_WORKSPACE}"
failures=0

usage() {
  echo "usage: $0 <hoonc-runfile> <native-runfile> [<hoonc-runfile> <native-runfile> ...]" >&2
}

hash_file() {
  local file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file" | awk '{print $1}'
  else
    openssl dgst -sha256 "$file" | awk '{print $NF}'
  fi
}

compare_artifact() {
  local hoonc_rel="$1"
  local native_rel="$2"
  local hoonc="${root}/${hoonc_rel}"
  local native="${root}/${native_rel}"

  if [[ ! -f "${hoonc}" ]]; then
    echo "missing generated hoonc artifact: ${hoonc_rel}" >&2
    failures=$((failures + 1))
    return
  fi
  if [[ ! -f "${native}" ]]; then
    echo "missing native artifact: ${native_rel}" >&2
    failures=$((failures + 1))
    return
  fi

  if ! cmp -s "${native}" "${hoonc}"; then
    echo "compiler artifact mismatch: ${hoonc_rel} vs ${native_rel}" >&2
    echo "  hoonc  $(hash_file "${hoonc}")" >&2
    echo "  native $(hash_file "${native}")" >&2
    failures=$((failures + 1))
  fi
}

if [[ "$#" -eq 0 || $(( $# % 2 )) -ne 0 ]]; then
  usage
  exit 2
fi

while [[ "$#" -gt 0 ]]; do
  compare_artifact "$1" "$2"
  shift 2
done

if [[ "${failures}" -ne 0 ]]; then
  echo "${failures} compiler parity comparison(s) failed" >&2
  exit 1
fi
