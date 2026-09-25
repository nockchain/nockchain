#!/usr/bin/env bash
# Merge cache objects emitted by the six sandboxed honk actions into the seed
# read by future Bazel builds. Run with `bazel run //assets/native:save_honk_cache`.
set -euo pipefail

workspace="${BUILD_WORKSPACE_DIRECTORY:?run this target with bazel run}"
saved=0
if [[ "$#" -eq 0 ]]; then
  echo "no kernel cache deltas were requested" >&2
  exit 1
fi

for kernel in "$@"; do
  cache="$workspace/.honk-cache/$kernel"
  delta="$workspace/bazel-bin/assets/native/${kernel}_native_cached_compile_cache_delta"
  if [[ ! -d "$delta" ]]; then
    echo "missing honk cache delta: $delta" >&2
    exit 1
  fi
  while IFS= read -r -d '' source; do
    relative="${source#"$delta/"}"
    destination="$cache/$relative"
    if [[ -f "$destination" ]] && cmp -s "$source" "$destination"; then
      continue
    fi
    mkdir -p "$(dirname "$destination")"
    temporary="$(mktemp "$destination.tmp.XXXXXXXX")"
    cp "$source" "$temporary"
    mv -f "$temporary" "$destination"
    saved=$((saved + 1))
  done < <(find "$delta" -type f -print0)
done

echo "Saved $saved honk cache objects under $workspace/.honk-cache"
