#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/nockup-template-smoke.sh [--keep-temp] [--offline] [--skip-runtime] [--skip-grpc-runtime]

Generates every bundled Nockup template from the current checkout, patches
generated Nockchain git dependencies to this local checkout, builds each
generated project with `nockup project build`, and runs smoke checks for the
generated apps.

Options:
  --keep-temp           Leave the temporary workspace in place for inspection.
  --offline             Run Cargo in offline mode to catch unexpected network access.
  --skip-runtime        Only generate and build templates.
  --skip-grpc-runtime   Skip the gRPC listen/talk runtime smoke.
  -h, --help            Show this help.
USAGE
}

KEEP_TEMP=0
OFFLINE=0
RUN_RUNTIME=1
RUN_GRPC_RUNTIME=1

while [ "$#" -gt 0 ]; do
  case "$1" in
    --keep-temp)
      KEEP_TEMP=1
      ;;
    --offline)
      OFFLINE=1
      ;;
    --skip-runtime)
      RUN_RUNTIME=0
      ;;
    --skip-grpc-runtime)
      RUN_GRPC_RUNTIME=0
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "${SCRIPT_DIR}/.." && pwd)"
ORIGINAL_HOME="${HOME:?HOME must be set}"
TEMPLATE_NAMES="basic repl http-server http-static grpc"
NOCKCHAIN_GIT_URL="https://github.com/nockchain/nockchain.git"

WORK_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/nockup-template-smoke.XXXXXX")"
SMOKE_HOME="${WORK_ROOT}/home"
SMOKE_TARGET="${WORK_ROOT}/target"
SMOKE_WORKSPACE="${WORK_ROOT}/workspace"
SMOKE_LOGS="${WORK_ROOT}/logs"
PIDS=""

export CARGO_HOME="${CARGO_HOME:-${ORIGINAL_HOME}/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-${ORIGINAL_HOME}/.rustup}"
export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-nightly-2025-11-26}"
export CARGO_TARGET_DIR="${SMOKE_TARGET}"
export HOME="${SMOKE_HOME}"
export NO_COLOR=1
export PATH="${SMOKE_TARGET}/release:${SMOKE_TARGET}/debug:${CARGO_HOME}/bin:${PATH}"

if [ "${OFFLINE}" -eq 1 ]; then
  export CARGO_NET_OFFLINE=true
fi

cleanup() {
  status="$1"

  for pid in ${PIDS}; do
    kill "${pid}" >/dev/null 2>&1 || true
    wait "${pid}" >/dev/null 2>&1 || true
  done

  if [ "${status}" -eq 0 ] && [ "${KEEP_TEMP}" -eq 0 ]; then
    rm -rf "${WORK_ROOT}"
  else
    echo "kept temp workspace: ${WORK_ROOT}"
  fi

  exit "${status}"
}
trap 'cleanup $?' EXIT

log() {
  printf '\n==> %s\n' "$*"
}

need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "required command not found: $1" >&2
    exit 1
  fi
}

toml_escape() {
  value="$1"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  printf '%s' "${value}"
}

template_project_name() {
  printf 'smoke-%s' "$1"
}

write_manifest() {
  template="$1"
  project_name="$(template_project_name "${template}")"

  cat > nockapp.toml <<EOF
[package]
name = "${project_name}"
version = "0.1.0"
description = "Nockup smoke test for ${template}"
authors = ["Nockup Smoke <smoke@example.com>"]
template = "${template}"
EOF
}

local_crate_path() {
  case "$1" in
    nockapp)
      printf '%s/crates/nockapp' "${REPO_ROOT}"
      ;;
    nockapp-grpc)
      printf '%s/crates/nockapp-grpc' "${REPO_ROOT}"
      ;;
    nockvm)
      printf '%s/crates/nockvm/rust/nockvm' "${REPO_ROOT}"
      ;;
    nockvm_macros)
      printf '%s/crates/nockvm/rust/nockvm_macros' "${REPO_ROOT}"
      ;;
    noun-serde)
      printf '%s/crates/noun-serde' "${REPO_ROOT}"
      ;;
    noun-serde-derive)
      printf '%s/crates/noun-serde-derive' "${REPO_ROOT}"
      ;;
    *)
      echo "no local path mapping for crate: $1" >&2
      exit 1
      ;;
  esac
}

local_dependency_line() {
  crate="$1"
  path="$(toml_escape "$(local_crate_path "${crate}")")"
  printf '%s = { path = "%s" }\n' "${crate}" "${path}"
}

rewrite_local_nockchain_deps() {
  cargo_toml="$1"
  tmp_file="${cargo_toml}.tmp"

  while IFS= read -r line || [ -n "${line}" ]; do
    case "${line}" in
      nockapp\ =\ \{\ git\ =\ \"${NOCKCHAIN_GIT_URL}\"*)
        local_dependency_line nockapp
        ;;
      nockapp-grpc\ =\ \{\ git\ =\ \"${NOCKCHAIN_GIT_URL}\"*)
        local_dependency_line nockapp-grpc
        ;;
      nockvm\ =\ \{\ git\ =\ \"${NOCKCHAIN_GIT_URL}\"*)
        local_dependency_line nockvm
        ;;
      nockvm_macros\ =\ \{\ git\ =\ \"${NOCKCHAIN_GIT_URL}\"*)
        local_dependency_line nockvm_macros
        ;;
      noun-serde\ =\ \{\ git\ =\ \"${NOCKCHAIN_GIT_URL}\"*)
        local_dependency_line noun-serde
        ;;
      noun-serde-derive\ =\ \{\ git\ =\ \"${NOCKCHAIN_GIT_URL}\"*)
        local_dependency_line noun-serde-derive
        ;;
      *)
        printf '%s\n' "${line}"
        ;;
    esac
  done <"${cargo_toml}" >"${tmp_file}"

  mv "${tmp_file}" "${cargo_toml}"

  if grep -q "${NOCKCHAIN_GIT_URL}" "${cargo_toml}"; then
    echo "unrewritten Nockchain git dependency found in ${cargo_toml}" >&2
    exit 1
  fi
}

port_is_listening() {
  port="$1"

  if command -v nc >/dev/null 2>&1 && nc -z 127.0.0.1 "${port}" >/dev/null 2>&1; then
    return 0
  fi

  if command -v lsof >/dev/null 2>&1 && lsof -nP -iTCP:"${port}" -sTCP:LISTEN >/dev/null 2>&1; then
    return 0
  fi

  return 1
}

wait_for_http() {
  url="$1"
  output_file="$2"

  for _ in $(seq 1 90); do
    if curl -fsS "${url}" >"${output_file}" 2>"${output_file}.err"; then
      return 0
    fi
    sleep 1
  done

  return 1
}

wait_for_port() {
  port="$1"

  for _ in $(seq 1 90); do
    if port_is_listening "${port}"; then
      return 0
    fi
    sleep 1
  done

  return 1
}

wait_for_port_free() {
  port="$1"

  for _ in $(seq 1 30); do
    if ! port_is_listening "${port}"; then
      return 0
    fi
    sleep 1
  done

  return 1
}

stop_background() {
  pid="$1"
  port="${2:-}"

  kill "${pid}" >/dev/null 2>&1 || true
  wait "${pid}" >/dev/null 2>&1 || true

  if [ -n "${port}" ] && command -v lsof >/dev/null 2>&1; then
    listener_pids="$(lsof -tiTCP:"${port}" -sTCP:LISTEN 2>/dev/null || true)"
    for listener_pid in ${listener_pids}; do
      kill "${listener_pid}" >/dev/null 2>&1 || true
    done
  fi

  if [ -n "${port}" ]; then
    wait_for_port_free "${port}"
  fi
}

require_port_free() {
  port="$1"

  if port_is_listening "${port}"; then
    echo "port ${port} is already in use; stop that process or skip the runtime smoke" >&2
    exit 1
  fi
}

run_nockup_project() {
  template="$1"
  project="$(template_project_name "${template}")"

  cd "${SMOKE_WORKSPACE}/${template}"
  "${SMOKE_TARGET}/debug/nockup" project build "${project}" \
    >"${SMOKE_LOGS}/build-${template}.log" 2>&1
}

run_basic() {
  log "running basic template"
  cd "${SMOKE_WORKSPACE}/basic"
  "${SMOKE_TARGET}/debug/nockup" project run smoke-basic \
    >"${SMOKE_LOGS}/run-basic.log" 2>&1
}

run_repl() {
  log "running repl template"
  cd "${SMOKE_WORKSPACE}/repl"
  printf 'quit\n' | "${SMOKE_TARGET}/debug/nockup" project run smoke-repl \
    >"${SMOKE_LOGS}/run-repl.log" 2>&1
}

run_http_server() {
  log "running http-server template"
  require_port_free 8080

  cd "${SMOKE_WORKSPACE}/http-server"
  "${SMOKE_TARGET}/debug/nockup" project run smoke-http-server \
    >"${SMOKE_LOGS}/run-http-server.log" 2>&1 &
  pid="$!"
  PIDS="${PIDS} ${pid}"

  wait_for_http "http://127.0.0.1:8080/" "${SMOKE_LOGS}/http-server-get.body"
  grep -q "Count: 0" "${SMOKE_LOGS}/http-server-get.body"

  curl -fsS -X POST "http://127.0.0.1:8080/increment" \
    >"${SMOKE_LOGS}/http-server-post.body" 2>"${SMOKE_LOGS}/http-server-post.err"
  grep -q "Count: 1" "${SMOKE_LOGS}/http-server-post.body"

  stop_background "${pid}" 8080
}

run_http_static() {
  log "running http-static template"
  require_port_free 8080

  cd "${SMOKE_WORKSPACE}/http-static"
  "${SMOKE_TARGET}/debug/nockup" project run smoke-http-static \
    >"${SMOKE_LOGS}/run-http-static.log" 2>&1 &
  pid="$!"
  PIDS="${PIDS} ${pid}"

  wait_for_http "http://127.0.0.1:8080/" "${SMOKE_LOGS}/http-static-get.body"
  grep -q "Hello NockApp!" "${SMOKE_LOGS}/http-static-get.body"

  stop_background "${pid}" 8080
}

run_grpc() {
  log "running grpc template"
  require_port_free 5555

  cd "${SMOKE_WORKSPACE}/grpc/smoke-grpc"
  RUST_LOG=debug cargo run --release --bin listen >"${SMOKE_LOGS}/grpc-listen.log" 2>&1 &
  pid="$!"
  PIDS="${PIDS} ${pid}"

  wait_for_port 5555
  RUST_LOG=debug cargo run --release --bin talk >"${SMOKE_LOGS}/grpc-talk.log" 2>&1
  sleep 2
  grep -a -q "Received peek" "${SMOKE_LOGS}/grpc-listen.log"

  stop_background "${pid}" 5555
}

need_cmd cargo
need_cmd curl
need_cmd git

mkdir -p "${SMOKE_HOME}/.nockup/templates" "${SMOKE_WORKSPACE}" "${SMOKE_LOGS}"

log "building local nockup and hoonc"
cd "${REPO_ROOT}"
cargo build -p nockup >"${SMOKE_LOGS}/cargo-build-nockup.log" 2>&1
cargo build --release -p hoonc >"${SMOKE_LOGS}/cargo-build-hoonc.log" 2>&1

log "seeding local template cache"
cp -R "${REPO_ROOT}/crates/nockup/templates/." "${SMOKE_HOME}/.nockup/templates/"

for template in ${TEMPLATE_NAMES}; do
  log "generating ${template}"
  mkdir -p "${SMOKE_WORKSPACE}/${template}"
  cd "${SMOKE_WORKSPACE}/${template}"
  write_manifest "${template}"
  "${SMOKE_TARGET}/debug/nockup" project init >"${SMOKE_LOGS}/init-${template}.log" 2>&1
  rewrite_local_nockchain_deps "$(template_project_name "${template}")/Cargo.toml"
done

for template in ${TEMPLATE_NAMES}; do
  log "building ${template}"
  run_nockup_project "${template}"
done

if [ "${RUN_RUNTIME}" -eq 1 ]; then
  run_basic
  run_repl
  run_http_server
  run_http_static

  if [ "${RUN_GRPC_RUNTIME}" -eq 1 ]; then
    run_grpc
  fi
fi

log "nockup template smoke PASS"
if [ "${KEEP_TEMP}" -eq 1 ]; then
  echo "logs: ${SMOKE_LOGS}"
else
  echo "temporary workspace will be removed; pass --keep-temp to inspect logs"
fi
