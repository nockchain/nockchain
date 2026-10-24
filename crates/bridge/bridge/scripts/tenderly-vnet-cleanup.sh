#!/usr/bin/env bash
set -euo pipefail

# Cleanup Tenderly virtual testnets for bridge manual-testing workflows.
#
# Default behavior:
#   - list vnets for the configured Tenderly project
#   - match names by prefix (default: bridge-vnet)
#   - keep the newest N matching vnets (default: 3)
#   - delete older matches, falling back to stop if delete fails
#
# Explicit target modes:
#   --current              cleanup TENDERLY_VNET_ID from the current shell or env file
#   --id <vnet-id>         cleanup one specific vnet id (repeatable)
#   --old                  cleanup by prefix/keep even when explicit ids are provided
#
# Examples:
#   ./tenderly-vnet-cleanup.sh
#   ./tenderly-vnet-cleanup.sh --keep 0
#   ./tenderly-vnet-cleanup.sh --current
#   ./tenderly-vnet-cleanup.sh --current --env-file scripts/environments/virtual-testnet.generated.env
#   ./tenderly-vnet-cleanup.sh --id abc123 --mode stop
#   ./tenderly-vnet-cleanup.sh --id abc123 --old --keep 2

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEFAULT_ENV_FILE="$SCRIPT_DIR/environments/virtual-testnet.generated.env"
API_BASE_URL="https://api.tenderly.co/api/v1"

PREFIX="${PREFIX:-bridge-vnet}"
KEEP="${KEEP:-3}"
MODE="${MODE:-delete}" # delete | stop
DRY_RUN="false"
DO_CURRENT="false"
DO_OLD="false"
ENV_FILE="$DEFAULT_ENV_FILE"
EXPLICIT_TARGETS=()

export TENDERLY_ACCOUNT_ID="${TENDERLY_ACCOUNT_ID:-nockchain}"
export TENDERLY_PROJECT_SLUG="${TENDERLY_PROJECT_SLUG:-bridge}"

usage() {
    cat <<'EOF'
Usage: tenderly-vnet-cleanup.sh [options]

Default behavior:
  Cleanup old vnets matching --prefix, keeping the newest --keep.

Options:
  --prefix PREFIX         Match vnets by display-name/name/slug prefix (default: bridge-vnet).
  --keep N               Keep newest N matching vnets when doing prefix cleanup (default: 3).
  --mode delete|stop     Delete vnets or stop them (default: delete).
  --current              Cleanup TENDERLY_VNET_ID from the current shell or --env-file.
  --id VNET_ID           Cleanup an explicit vnet id (repeatable).
  --old                  Also perform prefix cleanup when using --current/--id.
  --env-file PATH        Env file to source for --current (default: scripts/environments/virtual-testnet.generated.env).
  --dry-run              Print actions without mutating remote state.
  -h, --help             Show this help.

Required env:
  TENDERLY_ACCESS_KEY

Optional env:
  TENDERLY_ACCOUNT_ID    Default: nockchain
  TENDERLY_PROJECT_SLUG  Default: bridge
  TENDERLY_VNET_ID       Used by --current if already exported
EOF
}

log() { printf '[tenderly-vnet-cleanup] %s\n' "$*"; }
warn() { printf '[tenderly-vnet-cleanup] WARN: %s\n' "$*" >&2; }
die() { printf '[tenderly-vnet-cleanup] ERROR: %s\n' "$*" >&2; exit 1; }

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || die "Missing required command: $1"
}

require_env() {
    local name="$1"
    [[ -n "${!name:-}" ]] || die "$name is required"
}

is_true() {
    [[ "$1" == "true" || "$1" == "1" || "$1" == "yes" ]]
}

api_request() {
    local method="$1"
    local path="$2"
    local body="${3:-}"
    local url="$API_BASE_URL/$path"
    local resp code payload

    if [[ -n "$body" ]]; then
        resp="$(curl -sS -X "$method" "$url" \
            -H "Accept: application/json" \
            -H "Content-Type: application/json" \
            -H "X-Access-Key: $TENDERLY_ACCESS_KEY" \
            -d "$body" \
            -w $'\n%{http_code}')"
    else
        resp="$(curl -sS -X "$method" "$url" \
            -H "Accept: application/json" \
            -H "X-Access-Key: $TENDERLY_ACCESS_KEY" \
            -w $'\n%{http_code}')"
    fi

    code="${resp##*$'\n'}"
    payload="${resp%$'\n'*}"

    if (( code < 200 || code >= 300 )); then
        echo "$payload" >&2
        return 1
    fi

    echo "$payload"
}

load_env_file() {
    local path="$1"
    [[ -f "$path" ]] || die "Env file not found: $path"
    # shellcheck disable=SC1090
    source "$path"
}

cleanup_vnet_by_id() {
    local id="$1"
    local path="account/$TENDERLY_ACCOUNT_ID/project/$TENDERLY_PROJECT_SLUG/vnets/$id"

    if is_true "$DRY_RUN"; then
        log "[dry-run] Would $MODE vnet: $id"
        return
    fi

    if [[ "$MODE" == "delete" ]]; then
        if api_request DELETE "$path" >/dev/null 2>&1; then
            log "Deleted vnet: $id"
            return
        fi
        warn "Delete failed for $id, trying stop fallback"
    fi

    if api_request PATCH "$path" '{"status":"stopped"}' >/dev/null 2>&1; then
        log "Stopped vnet: $id"
        return
    fi

    warn "Failed to cleanup vnet: $id"
}

cleanup_explicit_targets() {
    local id
    for id in "${EXPLICIT_TARGETS[@]}"; do
        [[ -z "$id" ]] && continue
        cleanup_vnet_by_id "$id"
    done
}

cleanup_old_vnets() {
    local list_json ids

    log "Cleaning old vnets with prefix '$PREFIX' (keep newest $KEEP, mode=$MODE)..."
    list_json="$(api_request GET "account/$TENDERLY_ACCOUNT_ID/project/$TENDERLY_PROJECT_SLUG/vnets")" \
        || die "Failed to list vnets"

    ids="$(
        echo "$list_json" | jq -r \
            --arg prefix "$PREFIX" \
            --argjson keep "$KEEP" '
            def items:
                if type == "array" then .
                elif (.virtual_networks? | type) == "array" then .virtual_networks
                elif (.results? | type) == "array" then .results
                elif (.vnets? | type) == "array" then .vnets
                elif (.data? | type) == "array" then .data
                else [] end;

            [items[]
                | {
                    id: (.id // .vnet_id // .virtual_network_id // empty),
                    name: (.display_name // .name // .slug // ""),
                    epoch: (
                        (.created_at // .createdAt // .created // .inserted_at // .updated_at // 0)
                        | if type == "string" then (fromdateiso8601? // tonumber? // 0)
                          elif type == "number" then .
                          else 0 end
                    )
                }
                | select(.id != "" and (.name | startswith($prefix)))
            ]
            | sort_by(.epoch)
            | reverse
            | .[$keep:][]?.id
            '
    )"

    if [[ -z "$ids" ]]; then
        log "No old matching vnets to cleanup."
        return
    fi

    while IFS= read -r id; do
        [[ -z "$id" ]] && continue
        cleanup_vnet_by_id "$id"
    done <<< "$ids"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --prefix)
            PREFIX="$2"
            shift 2
            ;;
        --keep)
            KEEP="$2"
            shift 2
            ;;
        --mode)
            MODE="$2"
            shift 2
            ;;
        --current)
            DO_CURRENT="true"
            shift
            ;;
        --id)
            EXPLICIT_TARGETS+=("$2")
            shift 2
            ;;
        --old)
            DO_OLD="true"
            shift
            ;;
        --env-file)
            ENV_FILE="$2"
            shift 2
            ;;
        --dry-run)
            DRY_RUN="true"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "Unknown option: $1 (use --help)"
            ;;
    esac
done

[[ "$MODE" == "delete" || "$MODE" == "stop" ]] || die "--mode must be delete or stop"
[[ "$KEEP" =~ ^[0-9]+$ ]] || die "--keep must be a non-negative integer"

require_cmd curl
require_cmd jq
require_env TENDERLY_ACCESS_KEY
require_env TENDERLY_ACCOUNT_ID
require_env TENDERLY_PROJECT_SLUG

if is_true "$DO_CURRENT"; then
    if [[ -z "${TENDERLY_VNET_ID:-}" ]]; then
        load_env_file "$ENV_FILE"
    fi
    [[ -n "${TENDERLY_VNET_ID:-}" ]] || die "--current requested but TENDERLY_VNET_ID is unset"
    EXPLICIT_TARGETS+=("$TENDERLY_VNET_ID")
fi

if (( ${#EXPLICIT_TARGETS[@]} == 0 )) && ! is_true "$DO_OLD"; then
    DO_OLD="true"
fi

if (( ${#EXPLICIT_TARGETS[@]} > 0 )); then
    log "Cleaning explicit vnet targets: ${EXPLICIT_TARGETS[*]}"
    cleanup_explicit_targets
fi

if is_true "$DO_OLD"; then
    cleanup_old_vnets
fi

log "Done."
