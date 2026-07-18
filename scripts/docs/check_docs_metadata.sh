#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
OPEN_ROOT="$(CDPATH= cd -- "${SCRIPT_DIR}/../.." && pwd)"
cd "$OPEN_ROOT"

# Keep this list intentionally explicit for high signal checks.
TARGET_FILES='
ARCHITECTURE.md
DECISIONS/README.md
PROTOCOL.md
README.md
WORKFLOWS.md
changelog/protocol/SPECIFICATION.md
crates/hoonc/README.md
crates/nockapp/README.md
crates/nockchain-api/README.md
crates/nockchain-explorer-tui/README.md
crates/nockchain-wallet/README.md
crates/nockup/README.md
crates/nockvm/DEVELOPERS.md
crates/nockvm/README.md
'

# Optional extra files for local validation drills.
if [ -n "${DOCS_METADATA_EXTRA_FILES:-}" ]; then
    TARGET_FILES="${TARGET_FILES}
${DOCS_METADATA_EXTRA_FILES}
"
fi

failures=0
checked=0

check_field_in_header() {
    file="$1"
    field_name="$2"
    pattern="$3"

    first_match_line="$(rg -n --no-heading "$pattern" "$file" | cut -d: -f1 | head -n1 || true)"
    if [ -z "$first_match_line" ]; then
        printf '  - %s: missing `%s` header field\n' "$file" "$field_name"
        return 1
    fi

    if [ "$first_match_line" -gt 20 ]; then
        printf '  - %s: `%s` is outside the header block (first 20 lines)\n' "$file" "$field_name"
        return 1
    fi

    return 0
}

check_required_section() {
    file="$1"
    heading="$2"

    if ! rg -q "^## ${heading}$" "$file"; then
        printf '  - %s: missing required section heading `## %s`\n' "$file" "$heading"
        return 1
    fi

    return 0
}

check_required_pattern() {
    file="$1"
    label="$2"
    pattern="$3"

    if ! rg -q "$pattern" "$file"; then
        printf '  - %s: missing required pattern `%s`\n' "$file" "$label"
        return 1
    fi

    return 0
}

for file in $TARGET_FILES; do
    if [ ! -f "$file" ]; then
        printf '  - %s: file not found\n' "$file"
        failures=1
        continue
    fi

    checked=$((checked + 1))

    # Status and authority change how a reader should rely on a document.
    # Ownership labels and review dates do not, and quickly become stale.
    if ! check_field_in_header "$file" "Status" '^Status:[[:space:]]+.+$'; then
        failures=1
    fi
    if ! check_field_in_header "$file" "Canonical/Legacy" '^Canonical/Legacy:[[:space:]]+.+$'; then
        failures=1
    fi

    canonical_line="$(rg --no-heading '^Canonical/Legacy:[[:space:]]+.+$' "$file" | head -n1 || true)"
    if printf '%s' "$canonical_line" | rg -q 'Tier[[:space:]]+1'; then
        if ! check_required_section "$file" "Canonical Scope"; then
            failures=1
        fi
        if ! check_required_section "$file" "Failure Modes And Limits"; then
            failures=1
        fi
        if ! check_required_section "$file" "Verification Contract"; then
            failures=1
        fi
        if ! check_required_pattern "$file" "NOT canonical boundary line" '^This document is NOT canonical for:$'; then
            failures=1
        fi
    fi
done

if [ "$failures" -ne 0 ]; then
    printf 'docs metadata check: FAIL\n'
    exit 1
fi

printf 'docs metadata check: PASS (%s files)\n' "$checked"
