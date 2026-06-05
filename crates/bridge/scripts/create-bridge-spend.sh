#!/bin/bash
set -euo pipefail

# Create and submit a bridge-deposit spend from the local fakenet wallet.
#
# This script is intentionally barebones and hardcoded.
#
# Workflow:
# 1) boot/reset wallet with deterministic seed
# 2) parse list-notes output and select enough notes for amount + fee
# 3) create bridge-deposit tx
# 4) submit tx to the local node

die() {
    echo "Error: $*" >&2
    exit 1
}

is_uint() {
    [[ "$1" =~ ^[0-9]+$ ]]
}

run_wallet() {
    "$WALLET_SH" --color never "$@"
}

# Hardcoded transfer parameters and local server.
RECIPIENT="0x15A3DF65662B0235CdF27B3A8dD0f35D41E8A5BE"
AMOUNT="7000000000"
FEE="3457024"
MIN_BRIDGE_DEPOSIT="6553600000"
WALLET_SH="./wallet.sh"

[[ -x "$WALLET_SH" ]] || die "wallet script not executable: $WALLET_SH"
is_uint "$AMOUNT" || die "amount must be an unsigned integer"
is_uint "$FEE" || die "fee must be an unsigned integer"
is_uint "$MIN_BRIDGE_DEPOSIT" || die "min bridge deposit must be an unsigned integer"

RECIPIENT_HEX="${RECIPIENT#0x}"
[[ "$RECIPIENT_HEX" =~ ^[0-9a-fA-F]{40}$ ]] || die "recipient must be a 20-byte hex address"
RECIPIENT="0x${RECIPIENT_HEX}"

if (( AMOUNT < MIN_BRIDGE_DEPOSIT )); then
    die "amount ${AMOUNT} is below minimum bridge deposit ${MIN_BRIDGE_DEPOSIT}"
fi

TOTAL_REQUIRED=$((AMOUNT + FEE))

echo "== Bridge Spend Parameters =="
echo "Recipient: ${RECIPIENT}"
echo "Amount:    ${AMOUNT} nicks"
echo "Fee:       ${FEE} nicks"
echo "Required:  ${TOTAL_REQUIRED} nicks (amount + fee)"
echo "Wallet mode: wallet.sh defaults"
echo ""

echo "Resetting wallet state via --new..."
run_wallet --new show-balance >/dev/null

echo "Reading spendable notes..."
LIST_NOTES_OUTPUT="$(run_wallet list-notes)"
printf '%s\n' "$LIST_NOTES_OUTPUT"

SANITIZED_NOTES="$(
    printf '%s\n' "$LIST_NOTES_OUTPUT" \
    | sed -E 's/\x1B\[[0-9;]*[A-Za-z]//g' \
    | tr -cd '\11\12\15\40-\176'
)"

NOTE_ROWS_RAW="$(
    printf '%s\n' "$SANITIZED_NOTES" \
    | awk '
        BEGIN {
            collecting_name = 0
            have_name = 0
            name = ""
        }
        /^- Name: \[/ {
            line = $0
            sub(/^- Name: \[/, "", line)
            gsub(/^[[:space:]]+|[[:space:]]+$/, "", line)
            name = line
            if (line ~ /\]$/) {
                sub(/\]$/, "", name)
                have_name = 1
                collecting_name = 0
            } else {
                collecting_name = 1
            }
            next
        }
        collecting_name {
            line = $0
            gsub(/^[[:space:]]+|[[:space:]]+$/, "", line)
            if (line ~ /\]$/) {
                sub(/\]$/, "", line)
                name = name " " line
                collecting_name = 0
                have_name = 1
            } else {
                name = name " " line
            }
            next
        }
        /^- Assets \(nicks\): / {
            if (!have_name) {
                next
            }
            assets = $0
            sub(/^- Assets \(nicks\): /, "", assets)
            gsub(/[^0-9]/, "", assets)
            if (assets != "") {
                gsub(/[[:space:]]+/, " ", name)
                gsub(/^[[:space:]]+|[[:space:]]+$/, "", name)
                printf "%s|[%s]\n", assets, name
            }
            name = ""
            have_name = 0
        }
    ' \
    | sort -t'|' -k1,1n
)"

[[ -n "$NOTE_ROWS_RAW" ]] || die "no spendable notes parsed from list-notes output"

SELECTED_NAMES=""
SELECTED_TOTAL=0
SELECTED_COUNT=0

while IFS= read -r row; do
    [[ -n "$row" ]] || continue
    assets="${row%%|*}"
    note_name="${row#*|}"
    is_uint "$assets" || continue
    if [[ -n "$SELECTED_NAMES" ]]; then
        SELECTED_NAMES+=","
    fi
    SELECTED_NAMES+="$note_name"
    SELECTED_TOTAL=$((SELECTED_TOTAL + assets))
    SELECTED_COUNT=$((SELECTED_COUNT + 1))
    if (( SELECTED_TOTAL >= TOTAL_REQUIRED )); then
        break
    fi
done <<< "$NOTE_ROWS_RAW"

if (( SELECTED_TOTAL < TOTAL_REQUIRED )); then
    die "failed to select enough notes: selected ${SELECTED_TOTAL}, need ${TOTAL_REQUIRED}"
fi

echo "Selected ${SELECTED_COUNT} note(s) totaling ${SELECTED_TOTAL} nicks"
echo "Selected names: ${SELECTED_NAMES}"

RECIPIENT_JSON="$(printf '{"kind":"bridge-deposit","evm-address":"%s","amount":%s}' "$RECIPIENT" "$AMOUNT")"

echo "Creating bridge-deposit transaction..."
CREATE_OUTPUT="$(run_wallet create-tx --names "$SELECTED_NAMES" --fee "$FEE" --recipient "$RECIPIENT_JSON")"
printf '%s\n' "$CREATE_OUTPUT"

TX_FILE="$(printf '%s\n' "$CREATE_OUTPUT" | grep -Eo '\./txs/[A-Za-z0-9]+\.tx' | tail -n1 || true)"
if [[ -z "$TX_FILE" ]]; then
    TX_NAME="$(printf '%s\n' "$CREATE_OUTPUT" | sed -n 's/^- Name: //p' | head -n1)"
    if [[ -n "$TX_NAME" ]]; then
        TX_FILE="./txs/${TX_NAME}.tx"
    fi
fi
[[ -n "$TX_FILE" ]] || die "failed to parse tx file path from create-tx output"
[[ -f "$TX_FILE" ]] || die "tx file does not exist: $TX_FILE"

echo "Submitting transaction..."
SEND_OUTPUT="$(run_wallet send-tx "$TX_FILE")"
printf '%s\n' "$SEND_OUTPUT"

TX_ID="$(printf '%s\n' "$SEND_OUTPUT" | sed -n 's/.*Validation for TX \([A-Za-z0-9]\+\) passed.*/\1/p' | head -n1)"
if [[ -n "$TX_ID" ]]; then
    echo "Submitted TX ID: ${TX_ID}"
fi

echo "Done. Transaction file: ${TX_FILE}"
