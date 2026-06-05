#!/usr/bin/env bash
set -euo pipefail

# Create a Tenderly Base Sepolia virtual testnet, fund addresses via tenderly_setBalance,
# deploy bridge contracts (MessageInbox + Nock), and optionally clean up old vnets.
#
# Required env vars:
#   TENDERLY_ACCESS_KEY
#   TENDERLY_ACCOUNT_ID
#   TENDERLY_PROJECT_SLUG
#
# Required for deploy/fund:
#   TENDERLY_PRIVATE_KEY (or TENDERLY_TEST_PRIVATE_KEY for disposable VNet runs)
#   TENDERLY_PUBLIC_ADDRESS (optional; derived from TENDERLY_PRIVATE_KEY when unset)
#   BRIDGE_NODE_0..BRIDGE_NODE_4 (or BRIDGE_NODE_KEY_0..BRIDGE_NODE_KEY_4)
#   If BRIDGE_NODE_* are unset, defaults are used from test-bridge-keys.env.example
#
# Optional:
#   NOCK_NAME (default: Nock)
#   NOCK_SYMBOL (default: NOCK)
#
# Quickstart (from the bridge scripts directory):
#   export TENDERLY_ACCESS_KEY="..."
#   export TENDERLY_ACCOUNT_ID="..."
#   export TENDERLY_PROJECT_SLUG="..."
#   export TENDERLY_PRIVATE_KEY="0x..." # or rely on TENDERLY_TEST_PRIVATE_KEY for disposable VNet runs
#   export BRIDGE_NODE_0="0x..."
#   export BRIDGE_NODE_1="0x..."
#   export BRIDGE_NODE_2="0x..."
#   export BRIDGE_NODE_3="0x..."
#   export BRIDGE_NODE_4="0x..."
#   ./tenderly-vnet-deploy.sh --dry-run
#   ./tenderly-vnet-deploy.sh --cleanup-old --cleanup-prefix bridge-vnet --cleanup-keep 3
#   source environments/virtual-testnet.generated.env
#
# Examples:
#   ./tenderly-vnet-deploy.sh
#   ./tenderly-vnet-deploy.sh --name bridge-e2e --fund-eth 25
#   ./tenderly-vnet-deploy.sh --cleanup-old --cleanup-prefix bridge-vnet --cleanup-keep 2
#   ./tenderly-vnet-deploy.sh --cleanup-only --cleanup-prefix bridge-vnet --cleanup-mode delete

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BRIDGE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTRACTS_DIR="$BRIDGE_DIR/contracts"
DEPLOY_SCRIPT="$CONTRACTS_DIR/scripts/deploy_tenderly.sh"
API_BASE_URL="https://api.tenderly.co/api/v1"

VNET_NAME_PREFIX="bridge-vnet"
VNET_NAME=""
FORK_NETWORK_ID="84532" # Base Sepolia
CHAIN_ID="84532"        # Base Sepolia
FORK_BLOCK_NUMBER="latest"
STATE_SYNC="true"
PUBLIC_EXPLORER="true"
VERIFICATION_VISIBILITY="src"

FUND_AMOUNT_ETH="10"
DO_FUND="true"
DO_DEPLOY="true"
INSTALL_DEPS="true"

CLEANUP_OLD="false"
CLEANUP_ONLY="false"
CLEANUP_PREFIX=""
CLEANUP_KEEP="3"
CLEANUP_MODE="delete" # delete | stop

OUTPUT_ENV_PATH="$SCRIPT_DIR/environments/virtual-testnet.generated.env"
DEPLOY_TARGET_NETWORK=""
DEPLOYMENTS_PATH=""

DRY_RUN="false"

EXTRA_FUND_ADDRS=()
EXTRA_DEPLOY_ARGS=()

VNET_ID=""
ADMIN_RPC_URL=""
PUBLIC_RPC_URL=""
PUBLIC_WS_URL=""
VNET_BASE_START_HEIGHT=""
DEPLOYER_ADDRESS=""
INBOX_CONTRACT_ADDRESS=""
NOCK_CONTRACT_ADDRESS=""

DEFAULT_BRIDGE_NODE_0="0x2c7536E3605D9C16a7a3D7b1898e529396a65c23"
DEFAULT_BRIDGE_NODE_1="0x0EE156f080d9cB3BaA3C0DB53D07f13D69CEf4C9"
DEFAULT_BRIDGE_NODE_2="0x274BD645de480C325D618c60c661F11275eB77F1"
DEFAULT_BRIDGE_NODE_3="0x6dc59eb20f7928935c47A391e35545a2CEC51013"
DEFAULT_BRIDGE_NODE_4="0xcaB10dA05fC0aDBb7e91Eadc30f224bcDF601375"
DEFAULT_TENDERLY_TEST_PRIVATE_KEY="0xf9dca69398d5030c8f57a92cb69e4930caf448aefc493357dcafda87747f6098"

usage() {
    cat <<'EOF'
Usage: tenderly-vnet-deploy.sh [options]

Core options:
  --name NAME                   Display name for the new vnet.
  --prefix PREFIX               Prefix for generated vnet name/cleanup matching (default: bridge-vnet).
  --fork-network-id ID          Tenderly fork network id (default: 84532 for Base Sepolia).
  --chain-id ID                 Chain id for the virtual network (default: 84532).
  --fork-block N|latest         Fork block number (default: latest).
  --no-fund                     Skip tenderly_setBalance funding.
  --fund-eth AMOUNT             ETH amount per funded address (default: 10).
  --fund-address ADDRESS        Extra address to fund (repeatable).
  --no-deploy                   Skip contract deploy.
  --no-install-deps             Do not auto-run `make deps` in contracts/ if libs missing.
  --deploy-target-network NAME  DEPLOY_TARGET_NETWORK override.
  --deployments-path PATH       DEPLOYMENTS_PATH override.
  --deploy-arg ARG              Extra arg forwarded to deploy_tenderly.sh (repeatable).
  --output-env PATH             Generated env file path (default: scripts/environments/virtual-testnet.generated.env).

Cleanup options:
  --cleanup-old                 Cleanup old vnets after provisioning.
  --cleanup-only                Only cleanup old vnets (skip create/fund/deploy).
  --cleanup-prefix PREFIX       Prefix used to match old vnets (default: --prefix value).
  --cleanup-keep N              Keep newest N matching vnets (default: 3).
  --cleanup-mode delete|stop    Delete vnets or stop them (default: delete).

Other:
  --dry-run                     Print planned actions without mutating remote state.
  -h, --help                    Show this help.
EOF
}

log() { printf '[tenderly-vnet] %s\n' "$*"; }
warn() { printf '[tenderly-vnet] WARN: %s\n' "$*" >&2; }
die() { printf '[tenderly-vnet] ERROR: %s\n' "$*" >&2; exit 1; }

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || die "Missing required command: $1"
}

get_var() {
    eval "printf '%s' \"\${$1:-}\""
}

set_var() {
    local key="$1"
    local value="$2"
    eval "$key=\"\$value\""
    export "$key"
}

require_env() {
    local name="$1"
    [[ -n "$(get_var "$name")" ]] || die "$name is required"
}

is_true() {
    [[ "$1" == "true" || "$1" == "1" || "$1" == "yes" ]]
}

sanitize_slug() {
    local s="$1"
    s="$(echo "$s" | tr '[:upper:]' '[:lower:]')"
    s="$(echo "$s" | sed -E 's/[^a-z0-9-]+/-/g; s/^-+//; s/-+$//; s/-+/-/g')"
    printf '%s' "$s"
}

to_ws_url() {
    local url="$1"
    url="${url/https:\/\//wss://}"
    url="${url/http:\/\//ws://}"
    printf '%s' "$url"
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

rpc_request() {
    local rpc_url="$1"
    local payload="$2"
    curl -sS -X POST "$rpc_url" \
        -H "Content-Type: application/json" \
        -d "$payload"
}

resolve_vnet_start_height() {
    local resp="$1"
    local block_resp block_hex parsed_height

    VNET_BASE_START_HEIGHT="$(
        echo "$resp" | jq -r '
            [
                .fork_config.block_number?,
                .forkConfig.blockNumber?,
                .virtual_network.fork_config.block_number?,
                .virtual_network.forkConfig.blockNumber?,
                .fork.block_number?,
                .fork.blockNumber?,
                .virtual_network.fork.block_number?,
                .virtual_network.fork.blockNumber?,
                .forked_from.block_number?,
                .forked_from.blockNumber?,
                .virtual_network.forked_from.block_number?,
                .virtual_network.forked_from.blockNumber?
            ]
            | map(
                if type == "number" then tostring
                elif type == "string" then .
                else ""
                end
            )
            | map(select(test("^[0-9]+$")))
            | .[0] // empty
        '
    )"

    if [[ -z "$VNET_BASE_START_HEIGHT" && "$FORK_BLOCK_NUMBER" =~ ^[0-9]+$ ]]; then
        VNET_BASE_START_HEIGHT="$FORK_BLOCK_NUMBER"
    fi

    if [[ -z "$VNET_BASE_START_HEIGHT" && -n "$PUBLIC_RPC_URL" ]]; then
        if block_resp="$(
            rpc_request "$PUBLIC_RPC_URL" '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'
        )"; then
            block_hex="$(echo "$block_resp" | jq -r '.result // empty')"
            if [[ "$block_hex" =~ ^0x[0-9a-fA-F]+$ ]]; then
                parsed_height="$((16#${block_hex#0x}))"
                VNET_BASE_START_HEIGHT="$parsed_height"
            else
                warn "Unable to parse eth_blockNumber result when resolving VNet start height: $block_hex"
            fi
        else
            warn "eth_blockNumber request failed while resolving VNet start height"
        fi
    fi

    [[ -n "$VNET_BASE_START_HEIGHT" ]] || die "Unable to resolve VNet base start height from Tenderly response or RPC"
}

resolve_bridge_nodes() {
    local i addr key_var key
    require_cmd cast

    for i in 0 1 2 3 4; do
        addr="$(get_var "BRIDGE_NODE_${i}")"
        if [[ -z "$addr" ]]; then
            key_var="BRIDGE_NODE_KEY_${i}"
            key="$(get_var "$key_var")"
            if [[ -n "$key" ]]; then
                addr="$(cast wallet address --private-key "$key")"
                set_var "BRIDGE_NODE_${i}" "$addr"
            fi
        fi
        if [[ -z "$addr" ]]; then
            addr="$(get_var "DEFAULT_BRIDGE_NODE_${i}")"
            set_var "BRIDGE_NODE_${i}" "$addr"
        fi
    done
}

ensure_contract_deps() {
    if [[ -d "$CONTRACTS_DIR/lib/forge-std" && -d "$CONTRACTS_DIR/lib/openzeppelin-contracts" ]]; then
        return
    fi
    if ! is_true "$INSTALL_DEPS"; then
        die "Contract deps missing in $CONTRACTS_DIR/lib. Re-run with default install behavior."
    fi
    log "Installing contract dependencies (make -C contracts deps)..."
    make -C "$CONTRACTS_DIR" deps
}

create_vnet() {
    local now name slug payload resp
    now="$(date +%Y%m%d-%H%M%S)"

    if [[ -z "$VNET_NAME" ]]; then
        name="${VNET_NAME_PREFIX}-${now}"
    else
        name="$VNET_NAME"
    fi
    VNET_NAME="$name"
    slug="$(sanitize_slug "$name")-$(date +%s)"

    payload="$(jq -n \
        --arg slug "$slug" \
        --arg display_name "$name" \
        --argjson network_id "$FORK_NETWORK_ID" \
        --arg block_number "$FORK_BLOCK_NUMBER" \
        --argjson chain_id "$CHAIN_ID" \
        --argjson sync_enabled "$STATE_SYNC" \
        --argjson explorer_enabled "$PUBLIC_EXPLORER" \
        --arg verification_visibility "$VERIFICATION_VISIBILITY" \
        '{
            slug: $slug,
            display_name: $display_name,
            fork_config: {
                network_id: $network_id,
                block_number: $block_number
            },
            virtual_network_config: {
                chain_config: {
                    chain_id: $chain_id
                }
            },
            sync_state_config: {
                enabled: $sync_enabled
            },
            explorer_page_config: {
                enabled: $explorer_enabled,
                verification_visibility: $verification_visibility
            }
        }')"

    if is_true "$DRY_RUN"; then
        log "[dry-run] Would create vnet: $name"
        log "[dry-run] Payload: $payload"
        return
    fi

    log "Creating Tenderly vnet '$name'..."
    resp="$(api_request POST "account/$TENDERLY_ACCOUNT_ID/project/$TENDERLY_PROJECT_SLUG/vnets" "$payload")" \
        || die "Failed to create vnet via Tenderly API"

    VNET_ID="$(echo "$resp" | jq -r '.id // .virtual_network.id // empty')"
    [[ -n "$VNET_ID" ]] || die "Tenderly API response missing vnet id: $resp"

    ADMIN_RPC_URL="$(
        echo "$resp" | jq -r '
            def rpc_entries: ([.rpcs[]?] + [.virtual_network.rpcs[]?]);
            (
                [rpc_entries[] | select(((.name // "") | ascii_downcase | test("admin"))) | .url] +
                [.admin_rpc_url?, .adminRpcUrl?, .virtual_network.admin_rpc_url?, .virtual_network.adminRpcUrl?]
            )
            | map(select(type == "string" and . != "" and (startswith("https://") or startswith("http://"))))
            | .[0] // empty
        '
    )"
    if [[ -z "$ADMIN_RPC_URL" ]]; then
        ADMIN_RPC_URL="$(echo "$resp" | jq -r '.rpcs[0].url // empty')"
    fi

    PUBLIC_RPC_URL="$(
        echo "$resp" | jq -r '
            def rpc_entries: ([.rpcs[]?] + [.virtual_network.rpcs[]?]);
            (
                [rpc_entries[] | select(((.name // "") | ascii_downcase | test("public"))) | .url] +
                [.public_rpc_url?, .publicRpcUrl?, .virtual_network.public_rpc_url?, .virtual_network.publicRpcUrl?]
            )
            | map(select(type == "string" and . != "" and (startswith("https://") or startswith("http://"))))
            | .[0] // empty
        '
    )"
    if [[ -z "$PUBLIC_RPC_URL" ]]; then
        PUBLIC_RPC_URL="$(echo "$resp" | jq -r '[.rpcs[]?.url, .virtual_network.rpcs[]?.url] | map(select(type == "string" and . != "" and (startswith("https://") or startswith("http://")))) | .[0] // empty')"
    fi

    [[ -n "$ADMIN_RPC_URL" ]] || die "Unable to determine admin RPC URL from Tenderly response"
    [[ -n "$PUBLIC_RPC_URL" ]] || die "Unable to determine public RPC URL from Tenderly response"

    PUBLIC_WS_URL="$(
        echo "$resp" | jq -r '
            def rpc_entries: ([.rpcs[]?] + [.virtual_network.rpcs[]?]);
            (
                [rpc_entries[]
                    | select(((.name // "") | ascii_downcase | test("public")))
                    | .ws_url?, .wsUrl?, .websocket_url?, .websocketUrl?, .websocket_rpc_url?, .websocketRpcUrl?, .url?
                ] +
                [
                    .public_ws_url?, .publicWsUrl?, .public_websocket_url?, .publicWebsocketUrl?,
                    .public_websocket_rpc_url?, .publicWebsocketRpcUrl?, .public_rpc_ws_url?, .publicRpcWsUrl?,
                    .ws_rpc_url?, .wsRpcUrl?, .websocket_url?, .websocketUrl?,
                    .virtual_network.public_ws_url?, .virtual_network.publicWsUrl?,
                    .virtual_network.public_websocket_url?, .virtual_network.publicWebsocketUrl?,
                    .virtual_network.public_websocket_rpc_url?, .virtual_network.publicWebsocketRpcUrl?,
                    .virtual_network.ws_rpc_url?, .virtual_network.wsRpcUrl?,
                    .virtual_network.websocket_url?, .virtual_network.websocketUrl?
                ]
            )
            | map(select(type == "string" and . != "" and (startswith("wss://") or startswith("ws://"))))
            | .[0] // empty
        '
    )"
    if [[ -z "$PUBLIC_WS_URL" ]]; then
        PUBLIC_WS_URL="$(to_ws_url "$PUBLIC_RPC_URL")"
    fi
    resolve_vnet_start_height "$resp"

    log "Created vnet id: $VNET_ID"
    log "Admin RPC:  $ADMIN_RPC_URL"
    log "Public RPC: $PUBLIC_RPC_URL"
    log "Public WS:  $PUBLIC_WS_URL"
    log "Base start height: $VNET_BASE_START_HEIGHT"
}

fund_vnet() {
    local payload resp err amount_wei amount_hex addr_json
    local addrs=()
    local i extra_addr

    require_cmd cast

    DEPLOYER_ADDRESS="$(cast wallet address --private-key "$TENDERLY_PRIVATE_KEY")"
    addrs+=("$DEPLOYER_ADDRESS")
    for i in 0 1 2 3 4; do
        addrs+=("$(get_var "BRIDGE_NODE_${i}")")
    done
    # Use :- form to avoid nounset errors on older bash when optional arrays are empty.
    for extra_addr in "${EXTRA_FUND_ADDRS[@]:-}"; do
        [[ -z "$extra_addr" ]] && continue
        addrs+=("$extra_addr")
    done

    addr_json="$(
        printf '%s\n' "${addrs[@]}" \
        | awk 'NF && !seen[tolower($0)]++' \
        | jq -Rsc 'split("\n") | map(select(length > 0))'
    )"

    amount_wei="$(cast to-wei "$FUND_AMOUNT_ETH" ether)"
    amount_hex="$(cast to-hex "$amount_wei")"

    payload="$(jq -n \
        --argjson addrs "$addr_json" \
        --arg amount "$amount_hex" \
        '{jsonrpc:"2.0", method:"tenderly_setBalance", params:[$addrs, $amount], id:1}')"

    if is_true "$DRY_RUN"; then
        log "[dry-run] Would fund addresses with tenderly_setBalance amount=${FUND_AMOUNT_ETH} ETH"
        log "[dry-run] Addresses: $(echo "$addr_json" | jq -c '.')"
        return
    fi

    log "Funding deployer/bridge accounts with ${FUND_AMOUNT_ETH} ETH each via tenderly_setBalance..."
    resp="$(rpc_request "$ADMIN_RPC_URL" "$payload")"
    err="$(echo "$resp" | jq -r '.error.message // empty')"
    if [[ -n "$err" ]]; then
        die "Funding failed: $err"
    fi

    log "Funding complete."
}

deploy_contracts() {
    local network_name deploy_path
    local deploy_cmd
    local extra_arg

    require_cmd cast
    require_cmd forge
    [[ -x "$DEPLOY_SCRIPT" ]] || die "Missing deploy script: $DEPLOY_SCRIPT"

    DEPLOYER_ADDRESS="$(cast wallet address --private-key "$TENDERLY_PRIVATE_KEY")"

    export NOCK_NAME="${NOCK_NAME:-Nock}"
    export NOCK_SYMBOL="${NOCK_SYMBOL:-NOCK}"

    if [[ -z "$DEPLOY_TARGET_NETWORK" ]]; then
        if [[ -n "$VNET_ID" ]]; then
            network_name="tenderly-vnet-${VNET_ID}"
        else
            network_name="tenderly-vnet-$(date +%Y%m%d-%H%M%S)"
        fi
    else
        network_name="$DEPLOY_TARGET_NETWORK"
    fi

    if [[ -z "$DEPLOYMENTS_PATH" ]]; then
        deploy_path="$CONTRACTS_DIR/deployments/${network_name}.json"
    else
        deploy_path="$DEPLOYMENTS_PATH"
    fi

    export TENDERLY_RPC_URL="$ADMIN_RPC_URL"
    export DEPLOY_TARGET_NETWORK="$network_name"
    export DEPLOYMENTS_PATH="$deploy_path"
    export DEPLOYER_ADDRESS="$DEPLOYER_ADDRESS"

    if is_true "$DRY_RUN"; then
        log "[dry-run] Would deploy contracts via $DEPLOY_SCRIPT"
        log "[dry-run] DEPLOY_TARGET_NETWORK=$DEPLOY_TARGET_NETWORK"
        log "[dry-run] DEPLOYMENTS_PATH=$DEPLOYMENTS_PATH"
        return
    fi

    ensure_contract_deps

    log "Deploying MessageInbox + Nock..."
    deploy_cmd=("$DEPLOY_SCRIPT")
    # Use :- form to avoid nounset errors on older bash when optional arrays are empty.
    for extra_arg in "${EXTRA_DEPLOY_ARGS[@]:-}"; do
        [[ -z "$extra_arg" ]] && continue
        deploy_cmd+=("$extra_arg")
    done
    "${deploy_cmd[@]}"

    [[ -f "$DEPLOYMENTS_PATH" ]] || die "Deploy did not create deployment file: $DEPLOYMENTS_PATH"

    INBOX_CONTRACT_ADDRESS="$(jq -r '.messageInboxProxy // empty' "$DEPLOYMENTS_PATH")"
    NOCK_CONTRACT_ADDRESS="$(jq -r '.nock // empty' "$DEPLOYMENTS_PATH")"
    [[ -n "$INBOX_CONTRACT_ADDRESS" ]] || die "Deployment file missing messageInboxProxy"
    [[ -n "$NOCK_CONTRACT_ADDRESS" ]] || die "Deployment file missing nock"

    log "Deployed MessageInbox: $INBOX_CONTRACT_ADDRESS"
    log "Deployed Nock:         $NOCK_CONTRACT_ADDRESS"
}

cleanup_vnet_by_id() {
    local id="$1"
    local path="account/$TENDERLY_ACCOUNT_ID/project/$TENDERLY_PROJECT_SLUG/vnets/$id"

    if is_true "$DRY_RUN"; then
        log "[dry-run] Would $CLEANUP_MODE vnet: $id"
        return
    fi

    if [[ "$CLEANUP_MODE" == "delete" ]]; then
        if api_request DELETE "$path" >/dev/null 2>&1; then
            log "Deleted old vnet: $id"
            return
        fi
        warn "Delete failed for $id, trying stop fallback"
    fi

    if api_request PATCH "$path" '{"status":"stopped"}' >/dev/null 2>&1; then
        log "Stopped old vnet: $id"
        return
    fi

    warn "Failed to cleanup vnet: $id"
}

cleanup_old_vnets() {
    local prefix list_json ids
    prefix="$CLEANUP_PREFIX"
    if [[ -z "$prefix" ]]; then
        prefix="$VNET_NAME_PREFIX"
    fi

    log "Cleaning old vnets with prefix '$prefix' (keep newest $CLEANUP_KEEP, mode=$CLEANUP_MODE)..."
    list_json="$(api_request GET "account/$TENDERLY_ACCOUNT_ID/project/$TENDERLY_PROJECT_SLUG/vnets")" \
        || die "Failed to list vnets"

    ids="$(
        echo "$list_json" | jq -r \
            --arg prefix "$prefix" \
            --argjson keep "$CLEANUP_KEEP" '
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

write_env_file() {
    local out="$OUTPUT_ENV_PATH"

    if [[ -z "$PUBLIC_WS_URL" ]]; then
        PUBLIC_WS_URL="$(to_ws_url "$PUBLIC_RPC_URL")"
    fi
    [[ -n "$VNET_BASE_START_HEIGHT" ]] || die "VNet base start height missing; cannot write env file"

    if is_true "$DRY_RUN"; then
        log "[dry-run] Would write env file: $out"
        return
    fi

    mkdir -p "$(dirname "$out")"
    cat > "$out" <<EOF
# Auto-generated by tenderly-vnet-deploy.sh on $(date -u +"%Y-%m-%dT%H:%M:%SZ")
export BRIDGE_ENV="virtual-testnet"
export TENDERLY_VNET_ID="${VNET_ID}"
export TENDERLY_RPC_URL="${ADMIN_RPC_URL}"
export TENDERLY_PUBLIC_ADDRESS="${TENDERLY_PUBLIC_ADDRESS}"
export BASE_RPC_URL="${PUBLIC_RPC_URL}"
export BASE_WS_URL="${PUBLIC_WS_URL}"
export BASE_START_HEIGHT="${VNET_BASE_START_HEIGHT}"
export INBOX_CONTRACT_ADDRESS="${INBOX_CONTRACT_ADDRESS}"
export NOCK_CONTRACT_ADDRESS="${NOCK_CONTRACT_ADDRESS}"
EOF
    log "Wrote environment file: $out"
}

mkdir -p ../contracts/deployments

while [[ $# -gt 0 ]]; do
    case "$1" in
        --name)
            VNET_NAME="$2"
            shift 2
            ;;
        --prefix)
            VNET_NAME_PREFIX="$2"
            shift 2
            ;;
        --fork-network-id)
            FORK_NETWORK_ID="$2"
            shift 2
            ;;
        --chain-id)
            CHAIN_ID="$2"
            shift 2
            ;;
        --fork-block)
            FORK_BLOCK_NUMBER="$2"
            shift 2
            ;;
        --fund-eth)
            FUND_AMOUNT_ETH="$2"
            shift 2
            ;;
        --fund-address)
            EXTRA_FUND_ADDRS+=("$2")
            shift 2
            ;;
        --no-fund)
            DO_FUND="false"
            shift
            ;;
        --no-deploy)
            DO_DEPLOY="false"
            shift
            ;;
        --no-install-deps)
            INSTALL_DEPS="false"
            shift
            ;;
        --deploy-target-network)
            DEPLOY_TARGET_NETWORK="$2"
            shift 2
            ;;
        --deployments-path)
            DEPLOYMENTS_PATH="$2"
            shift 2
            ;;
        --deploy-arg)
            EXTRA_DEPLOY_ARGS+=("$2")
            shift 2
            ;;
        --output-env)
            OUTPUT_ENV_PATH="$2"
            shift 2
            ;;
        --cleanup-old)
            CLEANUP_OLD="true"
            shift
            ;;
        --cleanup-only)
            CLEANUP_ONLY="true"
            CLEANUP_OLD="true"
            DO_FUND="false"
            DO_DEPLOY="false"
            shift
            ;;
        --cleanup-prefix)
            CLEANUP_PREFIX="$2"
            shift 2
            ;;
        --cleanup-keep)
            CLEANUP_KEEP="$2"
            shift 2
            ;;
        --cleanup-mode)
            CLEANUP_MODE="$2"
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

[[ "$CLEANUP_MODE" == "delete" || "$CLEANUP_MODE" == "stop" ]] || die "--cleanup-mode must be delete or stop"
[[ "$STATE_SYNC" == "true" || "$STATE_SYNC" == "false" ]] || die "STATE_SYNC must be true/false"
[[ "$PUBLIC_EXPLORER" == "true" || "$PUBLIC_EXPLORER" == "false" ]] || die "PUBLIC_EXPLORER must be true/false"
[[ "$CLEANUP_KEEP" =~ ^[0-9]+$ ]] || die "--cleanup-keep must be a non-negative integer"

require_cmd curl
require_cmd jq

require_env TENDERLY_ACCESS_KEY
require_env TENDERLY_ACCOUNT_ID
require_env TENDERLY_PROJECT_SLUG

if ! is_true "$CLEANUP_ONLY"; then
    if [[ -z "${TENDERLY_PRIVATE_KEY:-}" ]]; then
        export TENDERLY_PRIVATE_KEY="${TENDERLY_TEST_PRIVATE_KEY:-$DEFAULT_TENDERLY_TEST_PRIVATE_KEY}"
    fi
    require_env TENDERLY_PRIVATE_KEY
    if [[ -z "${TENDERLY_PUBLIC_ADDRESS:-}" ]]; then
        require_cmd cast
        export TENDERLY_PUBLIC_ADDRESS="$(cast wallet address --private-key "$TENDERLY_PRIVATE_KEY")"
    fi
    if is_true "$DO_FUND" || is_true "$DO_DEPLOY"; then
        resolve_bridge_nodes
        require_env BRIDGE_NODE_0
        require_env BRIDGE_NODE_1
        require_env BRIDGE_NODE_2
        require_env BRIDGE_NODE_3
        require_env BRIDGE_NODE_4
    fi

    create_vnet

    if is_true "$DO_FUND"; then
        fund_vnet
    elif is_true "$DO_DEPLOY"; then
        DEPLOYER_ADDRESS="$(cast wallet address --private-key "$TENDERLY_PRIVATE_KEY")"
    fi

    if is_true "$DO_DEPLOY"; then
        deploy_contracts
    fi

    write_env_file
fi

if is_true "$CLEANUP_OLD"; then
    cleanup_old_vnets
fi

log "Done."
if [[ -n "$VNET_ID" ]]; then
    log "VNet ID: $VNET_ID"
fi
if [[ -n "$INBOX_CONTRACT_ADDRESS" && -n "$NOCK_CONTRACT_ADDRESS" ]]; then
    log "MessageInbox: $INBOX_CONTRACT_ADDRESS"
    log "Nock token:   $NOCK_CONTRACT_ADDRESS"
fi
if [[ -f "$OUTPUT_ENV_PATH" ]]; then
    log "Source env: source $OUTPUT_ENV_PATH"
fi
