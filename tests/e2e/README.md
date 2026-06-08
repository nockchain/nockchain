Nockchain end-to-end scenarios live in `tests/e2e/scenarios`.

Run a scenario locally:

```bash
cargo build -p nockchain --release
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/smoke.yaml
```

Two-node sync example:

```bash
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/two_nodes_sync.yaml
```

Additional scenarios:

```bash
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/upgrade_activation.yaml
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/mixed_version.yaml
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/partition_reorg.yaml
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/invalid_tx_rejected.yaml
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/wallet_smoke.yaml
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/transaction_lifecycle.yaml
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/double_spend_rejected.yaml
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_gen2_partition_reorg.yaml
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_gen2_multi_sender.yaml
```

Nous rollout scenarios:

```bash
# Shipped defaults (gen1 only) on both nodes
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_shipped_default.yaml

# Gen2 send enabled on both nodes
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_gen2_enabled.yaml

# Mixed: gen2-enabled sender plus explicit accept-only peer
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_mixed_generation.yaml

# Restart/rejoin after gen2 session
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_rollback.yaml

# Full transaction lifecycle with gen2 enabled on all peers
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_gen2_tx_lifecycle.yaml

# Adversarial transaction coverage with gen2 enabled on all peers
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_gen2_double_spend.yaml
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_gen2_invalid_tx.yaml

# Fail-fast wallet/block-stuffer smoke before the long soak
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_gen2_block_stuffer_preflight.yaml

# 4-node bounded recurring-gate soak to height 100+ with wallet traffic
# This should finish with a per-scenario report.json; use long-haul for 2h+ stress.
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_gen2_soak.yaml

# 7-node long-haul gen2 soak intended for 2h+ operator runs
./scripts/run_nous_long_haul_testnet.sh

# 4-node all-gen2 network with three concurrent sender wallets over three rounds
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_gen2_multi_sender.yaml

# Rolling per-node restart while enabling gen2 send
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_gen2_rolling_restart.yaml

# Rolling per-node rollback from gen2 send to gen1
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_gen2_rolling_rollback.yaml
# Gen2-enabled new node + pre-Nous old node (requires NOCKCHAIN_BIN_OLD)
NOCKCHAIN_BIN_OLD=/path/to/old NOCKCHAIN_BIN_NEW=target/release/nockchain \
  cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_old_new_fallback.yaml

# Gen2-enabled partition/fork/heal plus post-reorg transaction confirmation
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_gen2_partition_reorg.yaml
```

## Nous Generation Inventory

The Nous rollout defines three node configurations:

| Generation | Config | Protocol Behavior |
|---|---|---|
| **Pre-Nous** (old) | No gen2 code | gen1 only (`/nockchain-1-req-res`) |
| **Nous-default** | `accept=false, send=false` | gen1 only (`/nockchain-1-req-res`) |
| **Nous-accept** | `accept=true, send=false` | Accepts gen2 inbound, sends gen1 outbound |
| **Nous-enabled** | `accept=true, send=true` | Prefers gen2 outbound, falls back to gen1 |

### Rollout Test Matrix

These rollout-matrix scenarios are block-sync probes. Because outbound
`BlockByHeight` remains gen1-only during the staged rollout, the expected
transport for these sync exchanges is gen1 even when both nodes have
`req_res_gen2_send_enabled=true`.

| Requester | Responder | Expected Protocol | Scenario |
|---|---|---|---|
| Nous-default | Nous-default | gen1 | `nous_shipped_default.yaml` |
| Nous-enabled | Nous-enabled | gen1 (block-sync exclusion) | `nous_gen2_enabled.yaml` |
| Nous-enabled | Nous-accept | gen1 (block-sync exclusion) | `nous_mixed_generation.yaml` |
| Nous-enabled | Pre-Nous | gen1 (fallback) | `nous_old_new_fallback.yaml` |
| Nous-enabled | restart | gen1 (block-sync exclusion) | `nous_rollback.yaml` |

Additional rollout-load coverage:
- `nous_gen2_block_stuffer_preflight.yaml` smoke-tests the wallet/block-stuffer path before committing to the long soak.
- `nous_gen2_multi_sender.yaml` exercises three independent sender wallets submitting concurrent transactions across a 4-node all-gen2 network.
- `nous_gen2_rolling_restart.yaml` exercises staged per-node restarts while progressively enabling gen2 send.
- `nous_gen2_rolling_rollback.yaml` exercises staged per-node rollback from gen2 send to gen1.
- `run_nous_long_haul_testnet.sh` drives a separate 7-node, 2h+ long-haul gen2 soak without stretching the recurring validation suite into a multi-hour gate.

For steady-state enabled<->enabled transaction-path coverage, also run
`nous_gen2_tx_lifecycle.yaml`, `nous_gen2_double_spend.yaml`, and
`nous_gen2_invalid_tx.yaml`. The comprehensive operator suite in
`scripts/run_testnet_full_validation.sh` includes those three scenarios,
the block-stuffer preflight, the gen2 soak, the multi-sender run, the
rolling-restart and rolling-rollback drills, and both reorg scenarios.
The long-haul soak remains separate via `run_nous_long_haul_testnet.sh`.

The "old" binary must support `--bind-public-grpc-addr` for the runner to control it.
Protocol-ordering rollback (disabling `send_enabled` mid-session) is verified at the
crate level in `req_res_gen2_rollback_reverts_outbound_to_gen1`; the scenario schema
does not currently support changing env vars between stop/start cycles.

Notes:
- By default, `nockchain-e2e` uses `target/release/nockchain` when `--nockchain-bin` is not set.
- Use `--release=false` or `--nockchain-bin target/debug/nockchain` if you need a debug binary.
- Wallet scenarios require `nockchain-wallet` (use `--wallet-bin` or build `target/release/nockchain-wallet`).
- `upgrade_activation.yaml` and `mixed_version.yaml` require `NOCKCHAIN_BIN_OLD` and `NOCKCHAIN_BIN_NEW` to point at old/new binaries (or docker images in `--docker` mode).

Docker mode:
- Use `--docker` to run nodes in testcontainers with `--docker-image <name:tag>` (defaults to `NOCKCHAIN_E2E_IMAGE` or `nockchain-e2e:latest`).
- Example: `cargo run -p nockchain-e2e -- run tests/e2e/scenarios/smoke.yaml --docker --docker-image nockchain-e2e:ci`.
