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

Nous gen2 scenarios:

```bash
# Mandatory gen2 baseline and restart/rejoin
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_gen2_enabled.yaml
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_rollback.yaml

# Full transaction lifecycle and adversarial transaction coverage
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_gen2_tx_lifecycle.yaml
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_gen2_double_spend.yaml
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_gen2_invalid_tx.yaml

# Fail-fast wallet/block-stuffer smoke and bounded soak
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_gen2_block_stuffer_preflight.yaml
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_gen2_soak.yaml

# Multi-sender, partition/reorg, and long-haul coverage
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_gen2_multi_sender.yaml
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/nous_gen2_partition_reorg.yaml
./scripts/run_nous_long_haul_testnet.sh
```

## Nous Protocol Inventory

All nodes register `/nockchain-2-req-res` as full inbound and outbound support.
Generation 1, accept-only/send-only modes, and protocol fallback are not
supported. Pre-Nous binaries cannot join a gen2-only network.

All block sync, transaction, gossip, restart, and recovery scenarios therefore
expect `gen2`. The former
`NOCKCHAIN_LIBP2P_REQ_RES_GEN2_ACCEPT_ENABLED` and
`NOCKCHAIN_LIBP2P_REQ_RES_GEN2_SEND_ENABLED` environment variables are removed.

Additional load coverage:
- `nous_gen2_block_stuffer_preflight.yaml` smoke-tests the wallet/block-stuffer path before the soak.
- `nous_gen2_multi_sender.yaml` exercises independent sender wallets on a four-node gen2 network.
- `nous_rollback.yaml` verifies restart and catch-up without changing protocol generation.
- `run_nous_long_haul_testnet.sh` drives a separate seven-node, two-hour-plus gen2 soak.

The comprehensive operator suite in `scripts/run_testnet_full_validation.sh`
uses only gen2-capable nodes and scenarios. The long-haul soak remains separate
via `run_nous_long_haul_testnet.sh`.

Notes:
- By default, `nockchain-e2e` uses `target/release/nockchain` when `--nockchain-bin` is not set.
- Use `--release=false` or `--nockchain-bin target/debug/nockchain` if you need a debug binary.
- Wallet scenarios require `nockchain-wallet` (use `--wallet-bin` or build `target/release/nockchain-wallet`).
- `upgrade_activation.yaml` and `mixed_version.yaml` require `NOCKCHAIN_BIN_OLD` and `NOCKCHAIN_BIN_NEW` to point at old/new binaries (or docker images in `--docker` mode).

Docker mode:
- Use `--docker` to run nodes in testcontainers with `--docker-image <name:tag>` (defaults to `NOCKCHAIN_E2E_IMAGE` or `nockchain-e2e:latest`).
- Example: `cargo run -p nockchain-e2e -- run tests/e2e/scenarios/smoke.yaml --docker --docker-image nockchain-e2e:ci`.
