#!/usr/bin/env bash
# Format or check only root-owned workspace packages. Package-scoped formatting
# avoids formatting dependency sources outside the root workspace.
# Both write and --check modes support macOS system Bash 3.2 with nounset.

set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

case "${1:-}" in
  "") ;;
  --check) ;;
  *)
    printf 'usage: %s [--check]\n' "$0" >&2
    exit 2
    ;;
esac

PACKAGES=()
while IFS= read -r package; do
  PACKAGES+=("$package")
done < <(
  cargo metadata --no-deps --format-version 1 \
    | python3 -c '
import json, sys
m = json.load(sys.stdin)
member_ids = set(m["workspace_members"])
packages = [p["name"] for p in m["packages"] if p["id"] in member_ids]
for name in sorted(packages):
    print(name)
'
)

if ((${#PACKAGES[@]} == 0)); then
  printf 'no workspace packages found\n' >&2
  exit 1
fi

printf 'Formatting %s root workspace package(s).\n' "${#PACKAGES[@]}"
for package in "${PACKAGES[@]}"; do
  # Bash 3.2 with nounset cannot expand an empty argument array.
  if [[ "${1:-}" == --check ]]; then
    cargo fmt --package "$package" --check
  else
    cargo fmt --package "$package"
  fi
done
