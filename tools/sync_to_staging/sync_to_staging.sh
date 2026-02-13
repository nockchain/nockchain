#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFIG_PATH="${SCRIPT_DIR}/copy.bara.sky"

COPYBARA_VERSION="${COPYBARA_VERSION:-20250224}"
COPYBARA_SHA256="${COPYBARA_SHA256:-5938f1db447c20ff9859828f3d52ce1b04dbe1da18195337f3efae5e65f1d969}"
COPYBARA_JAR="${COPYBARA_JAR:-/tmp/copybara_deploy.jar}"

if [[ ! -f "${COPYBARA_JAR}" ]]; then
  curl -fsSL \
    "https://github.com/google/copybara/releases/download/v${COPYBARA_VERSION}/copybara_deploy.jar" \
    -o "${COPYBARA_JAR}"
fi

if command -v sha256sum >/dev/null 2>&1; then
  echo "${COPYBARA_SHA256}  ${COPYBARA_JAR}" | sha256sum -c -
elif command -v shasum >/dev/null 2>&1; then
  echo "${COPYBARA_SHA256}  ${COPYBARA_JAR}" | shasum -a 256 -c -
else
  echo "sha256sum or shasum is required to verify copybara_deploy.jar" >&2
  exit 1
fi

if ! command -v java >/dev/null 2>&1; then
  echo "java is required to run Copybara" >&2
  exit 1
fi

ORIGIN_DIR="${1:-$(pwd)}"

exec java -jar "${COPYBARA_JAR}" migrate \
  --force \
  --git-destination-non-fast-forward \
  "${CONFIG_PATH}" \
  sync_to_staging \
  "${ORIGIN_DIR}"
