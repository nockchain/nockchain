#!/bin/bash
set -euo pipefail

# Create and submit a bridge-deposit spend from the local fakenet wallet.
#
# This script is intentionally barebones and hardcoded.
#
# Workflow:
# 1) boot/reset wallet with deterministic seed
# 2) display wallet notes for operator context
# 3) let create-tx planner select spendable notes in ascending order
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

# Transfer parameters are overridable so bridge-dev can reuse the same flow.
RECIPIENT="${BRIDGE_DEPOSIT_RECIPIENT:-0x15A3DF65662B0235CdF27B3A8dD0f35D41E8A5BE}"
AMOUNT="${BRIDGE_DEPOSIT_AMOUNT:-7000000000}"
FEE="${BRIDGE_DEPOSIT_FEE:-3457024}"
MIN_BRIDGE_DEPOSIT="${BRIDGE_MIN_DEPOSIT:-6553600000}"
BRIDGE_LOCK_ROOT="${BRIDGE_DEPOSIT_LOCK_ROOT:-}"
WALLET_SH="${BRIDGE_WALLET_SH:-./wallet.sh}"

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

echo "Reading wallet notes..."
LIST_NOTES_OUTPUT="$(run_wallet list-notes)"
printf '%s\n' "$LIST_NOTES_OUTPUT"

if [[ -n "$BRIDGE_LOCK_ROOT" ]]; then
    RECIPIENT_JSON="$(printf '{"kind":"bridge-deposit","root":"%s","evm-address":"%s","amount":%s}' "$BRIDGE_LOCK_ROOT" "$RECIPIENT" "$AMOUNT")"
else
    RECIPIENT_JSON="$(printf '{"kind":"bridge-deposit","evm-address":"%s","amount":%s}' "$RECIPIENT" "$AMOUNT")"
fi

echo "Creating bridge-deposit transaction with planner-selected ascending inputs..."
CREATE_OUTPUT="$(run_wallet create-tx --note-selection ascending --fee "$FEE" --recipient "$RECIPIENT_JSON")"
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
