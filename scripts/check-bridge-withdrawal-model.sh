#!/usr/bin/env bash
set -euo pipefail
set -C

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

QUINT_VERSION="0.32.0"
MAX_STEPS="${BRIDGE_MODEL_MAX_STEPS:-20}"
SIMULATION_SEED="${BRIDGE_MODEL_SEED:-424242}"
TIMEOUT_SECONDS="${BRIDGE_MODEL_TIMEOUT_SECONDS:-180}"
MODEL="spec/bridge-withdrawal.qnt"
TESTS="spec/bridge-withdrawal-tests.qnt"
MUTATIONS="spec/mutations/bridge-withdrawal-mutations.qnt"
RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)-$$"
OUTPUT_ROOT="target/bridge-model"
RUN_DIR="$OUTPUT_ROOT/$RUN_ID"

for command in node npx jq timeout; do
  if ! command -v "$command" >/dev/null 2>&1; then
    printf 'required command is unavailable: %s\n' "$command" >&2
    exit 2
  fi
done

if ! [[ "$MAX_STEPS" =~ ^[1-9][0-9]*$ ]] ||
   ! [[ "$TIMEOUT_SECONDS" =~ ^[1-9][0-9]*$ ]]; then
  printf 'bounds must be positive integers\n' >&2
  exit 2
fi

mkdir -p "$OUTPUT_ROOT"
mkdir "$RUN_DIR"

QUINT=(npx -y "@informalsystems/quint@$QUINT_VERSION")
set +e
actual_version="$(
  timeout "${TIMEOUT_SECONDS}s" "${QUINT[@]}" --version \
    2>"$RUN_DIR/version.log"
)"
version_status=$?
set -e
if [[ "$version_status" -ne 0 ]]; then
  printf 'unable to run Quint %s (log %s)\n' \
    "$QUINT_VERSION" "$RUN_DIR/version.log" >&2
  exit 2
fi
if [[ "$actual_version" != "$QUINT_VERSION" ]]; then
  printf 'Quint version drift: expected %s, observed %s\n' \
    "$QUINT_VERSION" "$actual_version" >&2
  exit 2
fi

run_checked() {
  local name="$1"
  shift
  local log="$RUN_DIR/$name.log"
  set +e
  timeout "${TIMEOUT_SECONDS}s" "$@" >"$log" 2>&1
  local status=$?
  set -e
  if [[ "$status" -ne 0 ]]; then
    if [[ "$status" -eq 124 || "$status" -eq 137 ]]; then
      printf 'model command timed out: %s (log %s)\n' "$name" "$log" >&2
    else
      printf 'model command failed: %s (log %s)\n' "$name" "$log" >&2
    fi
    exit 1
  fi
}

run_checked typecheck \
  "${QUINT[@]}" typecheck "$TESTS"
run_checked simulations \
  "${QUINT[@]}" test "$TESTS"
run_checked seeded-safety-simulation \
  "${QUINT[@]}" run "$MODEL" \
  --seed="$SIMULATION_SEED" \
  --max-samples=1 \
  --max-steps="$MAX_STEPS" \
  --invariant=allSafety \
  --out-itf="$RUN_DIR/simulation.itf.json"
run_checked bounded-safety \
  "${QUINT[@]}" verify "$MODEL" \
  --backend=tlc \
  --invariant=allSafety \
  --max-steps="$MAX_STEPS"
run_checked healthy-liveness \
  "${QUINT[@]}" verify "$MODEL" \
  --backend=tlc \
  --temporal=healthyEventuallyTerminal \
  --max-steps="$MAX_STEPS"
run_checked shallow-fork-liveness \
  "${QUINT[@]}" verify "$MODEL" \
  --backend=tlc \
  --temporal=shallowForkEventuallyResolves \
  --max-steps="$MAX_STEPS"

validate_itf() {
  local trace="$1"
  if ! jq -e \
    '."#meta".format == "ITF" and (.states | type == "array") and (.states | length >= 2)' \
    "$trace" >/dev/null; then
    printf 'counterexample is missing or malformed: %s\n' "$trace" >&2
    exit 1
  fi
}

expect_counterexample() {
  local slug="$1"
  local module="$2"
  local invariant="$3"
  local trace="$RUN_DIR/$slug.itf.json"
  local log="$RUN_DIR/$slug.log"

  set +e
  timeout "${TIMEOUT_SECONDS}s" "${QUINT[@]}" verify "$MUTATIONS" \
    --main="$module" \
    --step=mutationStep \
    --invariant="$invariant" \
    --max-steps="$MAX_STEPS" \
    --out-itf="$trace" >"$log" 2>&1
  local status=$?
  set -e

  if [[ "$status" -eq 0 ]]; then
    printf 'mutation did not produce a counterexample: %s\n' "$slug" >&2
    exit 1
  fi
  if [[ "$status" -eq 124 || "$status" -eq 137 ]]; then
    printf 'mutation check timed out: %s\n' "$slug" >&2
    exit 1
  fi
  validate_itf "$trace"
}

expect_counterexample \
  reservation-owner \
  reservation_owner_mutation \
  oneReservationOwnerInv
expect_counterexample \
  authorized-identity \
  authorized_identity_mutation \
  retryIdentityInv
expect_counterexample \
  authorized-base-fork \
  authorized_base_fork_mutation \
  authorizedRawIdentityInv
expect_counterexample \
  premature-second-withdrawal \
  premature_second_withdrawal_mutation \
  secondReservationOrderInv
expect_counterexample \
  compensation-payout \
  compensation_payout_mutation \
  compensationExcludesPayoutInv
expect_counterexample \
  premature-terminal \
  premature_terminal_mutation \
  terminalProofInv
expect_counterexample \
  skipped-deep-hold \
  skipped_deep_hold_mutation \
  unsafeForkHoldsInv

validate_itf "$RUN_DIR/simulation.itf.json"

SUMMARY="$RUN_DIR/summary.json"
jq -n \
  --arg schema_version "1" \
  --arg quint_version "$QUINT_VERSION" \
  --arg seed "$SIMULATION_SEED" \
  --arg max_steps "$MAX_STEPS" \
  --arg timeout_seconds "$TIMEOUT_SECONDS" \
  --arg run_dir "$RUN_DIR" \
  '{
    schema_version: ($schema_version | tonumber),
    quint_version: $quint_version,
    seed: $seed,
    max_steps: ($max_steps | tonumber),
    timeout_seconds: ($timeout_seconds | tonumber),
    run_dir: $run_dir,
    unmutated: {
      typecheck: "passed",
      simulations: "passed",
      safety: "passed",
      healthy_liveness: "passed",
      shallow_fork_liveness: "passed"
    },
    expected_counterexamples: [
      "reservation-owner",
      "authorized-identity",
      "authorized-base-fork",
      "premature-second-withdrawal",
      "compensation-payout",
      "premature-terminal",
      "skipped-deep-hold"
    ]
  }' >"$SUMMARY"

printf 'bridge-withdrawal-model-report=%s\n' "$SUMMARY"
