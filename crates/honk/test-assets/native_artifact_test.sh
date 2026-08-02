#!/usr/bin/env bash
set -euo pipefail

root="${TEST_SRCDIR}/${TEST_WORKSPACE}"

if [[ "$#" -eq 0 ]]; then
  echo "usage: $0 <generated-artifact> [...]" >&2
  exit 2
fi

for relative in "$@"; do
  artifact="${root}/${relative}"
  if [[ ! -s "${artifact}" ]]; then
    echo "missing or empty generated artifact: ${relative}" >&2
    exit 1
  fi
done
