#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
OPEN_ROOT="$(CDPATH= cd -- "${SCRIPT_DIR}/../.." && pwd)"
cd "$OPEN_ROOT"

SPEC_FILE="changelog/protocol/013-nous.md"
failures=0

require_pattern() {
    label="$1"
    pattern="$2"

    if ! rg -q "$pattern" "$SPEC_FILE"; then
        printf '  - %s: missing `%s`\n' "$SPEC_FILE" "$label"
        failures=1
    fi
}

if [ ! -f "$SPEC_FILE" ]; then
    printf '  - %s: file not found\n' "$SPEC_FILE"
    printf 'nous validation entrypoint check: FAIL\n'
    exit 1
fi

require_pattern "run_testnet_full_validation entrypoint" 'scripts/run_testnet_full_validation\.sh'
require_pattern "run_testnet_gen2_validation entrypoint" 'scripts/run_testnet_gen2_validation\.sh'
require_pattern "run_nous_mixed_generation_e2e entrypoint" 'scripts/run_nous_mixed_generation_e2e\.sh'
require_pattern "validator json output path" 'scripts/validate_req_res_gen2_rollout\.rs --json-out target/test-logs/req_res_gen2_rollout_readiness/latest\.json'
require_pattern "canonical recurring readiness gate wording" 'canonical recurring readiness gate'
require_pattern "one-time pre-testnet validation wording" 'one-time pre-testnet validation'
require_pattern "full validation report output" 'target/test-logs/testnet_full_validation/<timestamp>/report\.json'
require_pattern "embedded old/new fallback proof summary wording" 'embedded `old_new_fallback_proof` summary'
require_pattern "first-checkpoint fallback proof gate wording" 'pass `--require-old-new-fallback`.*recurring report.*fails'
require_pattern "focused staged-send drill wording" 'narrower staged-send drill'
require_pattern "gen2 staged-send scenario reference" 'nous_testnet_gen2_send'

# Output-path guards for remaining canonical validation entrypoints
require_pattern "gen2 staged-send compose/consensus.log output" 'testnet_gen2_validation/<timestamp>/compose/consensus\.log'
require_pattern "gen2 staged-send scenario-run/ output" 'testnet_gen2_validation/<timestamp>/scenario-run/'
require_pattern "cargo-only reducer benchmark sidecar output" 'target/benchmarks/req_res_gen2/latest\.json'
require_pattern "mixed-generation proof log output" 'nous_mixed_generation/<timestamp>\.log'

# Guard: operator guide must use the canonical --json-out validator flag.
OPERATOR_GUIDE="../docs/NOUS-OPERATOR-GUIDE.md"
if [ -f "$OPERATOR_GUIDE" ]; then
    if ! rg -q 'scripts/validate_req_res_gen2_rollout\.rs.*--json-out' "$OPERATOR_GUIDE"; then
        printf '  - %s: missing canonical --json-out validator flag\n' "$OPERATOR_GUIDE"
        failures=1
    fi
    # Detect bare one-line validator invocations without --json-out.
    if rg -q 'validate_req_res_gen2_rollout\.rs\s*$' "$OPERATOR_GUIDE"; then
        printf '  - %s: bare validator invocation without --json-out detected\n' "$OPERATOR_GUIDE"
        failures=1
    fi
fi

if [ "$failures" -ne 0 ]; then
    printf 'nous validation entrypoint check: FAIL\n'
    exit 1
fi

printf 'nous validation entrypoint check: PASS\n'
