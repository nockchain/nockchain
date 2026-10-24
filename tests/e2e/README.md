Nockchain end-to-end scenarios live in `tests/e2e/scenarios`.

Run a scenario locally:

```bash
cargo build -p nockchain -p nockchain-wallet --release
cargo build -p zk-pow-miner --bin zk-pow-mine --release --no-default-features
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
- By default, `nockchain-e2e` uses `target/release/nockchain` and `target/release/zk-pow-mine`. Use `--nockchain-bin` and `--miner-bin` for explicit host executable paths.
- `--release=false` selects both debug binaries. When overriding just one path, explicitly select the other path if it is not a release build.
- Wallet scenarios require `nockchain-wallet` (use `--wallet-bin` or build `target/release/nockchain-wallet`).
- `upgrade_activation.yaml` and `mixed_version.yaml` require `NOCKCHAIN_BIN_OLD` and `NOCKCHAIN_BIN_NEW` to identify the actual old/new node executables (or images in Docker mode). Both must support the current public/private gRPC and standalone-miner protocol; an incompatible old binary is an error, not a substitution.

## Standalone mining and lifecycle

- `mine: true` starts one real standalone CPU `zk-pow-mine` child per node. The node itself receives no embedded-mining or payout flags. A configured `mining_pkh` is harmless when `mine: false`; it is used when mining is explicitly started.
- Set `mining_threads` on a node to choose its miner thread count (default one). `extra_args` remain node arguments, not miner arguments.
- Process mode supports a node's `miner_binary` override. Docker mode supports separate `miner_image` and `miner_binary` overrides; the latter is the executable path **inside the miner image**, not a host path.
- Startup waits for the node's actual private kernel RPC before provisioning both the existing v0 recipient and the requested v1 payout, then starts the miner. Fresh mandatory verifier setup precedes RPC availability and can take several minutes; the bounded startup wait is 1800 seconds. Scenario `wait_for_grpc`/height timeouts apply after that startup stage.
- `set_mining_pkh` provisions the current operator-controlled keys and starts/replaces the owned miner without restarting the node. `set_mining_enabled: false` and `disable_mining` stop the owned worker before disabling kernel mining; the payout is retained for a later enable. Enabling starts a real worker if needed.
- Node restart/upgrade stops the miner before the node and preserves the requested mining state. `upgrade_miner` selects an independent artifact through `miner_binaries` (host executable paths in process mode, image references in Docker mode); `version: current` selects the runner's miner artifact, never its node artifact.
- Normal child teardown does not globally disable mining merely because a miner exits. Explicit mining-stop actions do. Unexpected owned-child exits fail waits/actions, and failed startup cleans up the children it started.
- Each node work directory retains `stdout.log`/`stderr.log`; its miner has separate `miner/stdout.log`/`miner/stderr.log`. Restarted children archive previous logs under their own `logs/launch-NNN/` directories, while current-launch assertions read only the current node logs. Docker logs are captured before owned-container removal.

Private operator credentials are taken from the node's configured environment, or the inherited environment when not overridden, and passed explicitly to its private clients and children. Set `NOCKCHAIN_PRIVATE_GRPC_AUTH_TOKEN` for authenticated control. If `NOCKCHAIN_PRIVATE_GRPC_MINER_TOKEN` is configured, the miner receives that restricted credential, not the operator token; the manager provisions keys and enables mining as the operator. Tokens travel in child environments and private-client metadata, not command arguments. `set_node_env` changes take effect when the node is restarted. The existing explicit unauthenticated loopback fixture opt-out is preserved when no credentials are configured; it is not a deployment recommendation.

## Docker mode

Use a local Docker daemon and Docker CLI. Build **Linux binaries for that daemon's architecture** before building these images; host macOS executables cannot be copied into a Linux runtime image.

```bash
docker build -f tools/docker/Dockerfile -t nockchain-e2e:latest .
docker build -f tools/docker/Dockerfile.zk-pow-miner -t zk-pow-miner-e2e:latest .
cargo run -p nockchain-e2e -- run tests/e2e/scenarios/two_nodes_sync.yaml \
  --docker --docker-image nockchain-e2e:latest \
  --docker-miner-image zk-pow-miner-e2e:latest
```

- Node image selection defaults to `NOCKCHAIN_E2E_IMAGE` or `nockchain-e2e:latest`; miner image selection defaults to `NOCKCHAIN_E2E_MINER_IMAGE` or `zk-pow-miner-e2e:latest`.
- `--docker-miner-bin` defaults to `/usr/local/bin/zk-pow-mine`. The supplied miner recipe contains the real CPU executable and uses SIGINT for shutdown.
- Nodes retain the existing isolated bridge network and private-gRPC relay (`alpine/socat:1.8.0.3` by default). The relay exposes the node's loopback private service through a **127.0.0.1-only host port**; it does not bypass service authentication. Miner containers share their node's network namespace and connect directly to its internal loopback service.
- This is a local, trusted-container fixture: other containers attached to its bridge can reach the relay. Provision operator/restricted credentials when isolation from those callers is required. Public gRPC/P2P publication and peer rewriting retain their existing semantics. Relay diagnostics are retained with node logs.

## Genuine lifecycle checks

These opt-in tests require real binaries/images and retain child artifacts in a printed temporary work directory. They exercise authenticated private RPC, accepted v0/v1 CPU-mined heights, pause/resume, restart, independent miner replacement, prompt crash detection, log retention, and cleanup; they do not use a fake miner.

```bash
NOCKCHAIN_BIN_NEW="$PWD/target/release/nockchain" \
NOCKCHAIN_MINER_BIN="$PWD/target/release/zk-pow-mine" \
cargo test -p nockchain-e2e --lib genuine_process_miner_lifecycle \
  -- --ignored --nocapture --test-threads=1

NOCKCHAIN_BIN_NEW="$PWD/target/release/nockchain" \
cargo test -p nockchain-e2e --lib genuine_missing_miner_startup_releases_owned_children \
  -- --ignored --nocapture --test-threads=1

NOCKCHAIN_E2E_IMAGE=nockchain-e2e:latest \
NOCKCHAIN_E2E_MINER_IMAGE=zk-pow-miner-e2e:latest \
cargo test -p nockchain-e2e --lib genuine_docker_miner_lifecycle \
  -- --ignored --nocapture --test-threads=1
```

The missing-miner startup check requires an actual executable-not-found failure and checks that ownership and listening ports are released before final shutdown. The Docker lifecycle check also inspects the actual private host-port binding.

The controller exposes typed `PrivateRpcError` categories for transport and gRPC status, local-client failures, NockApp poke NACKs (`PokeFailed`), other RPC rejections, and deadlines/cancellation/decode failures. SDK errors are adapted at this boundary without changing SDK `Send`/`Sync` contracts. A poke acknowledgement reports successful outer-kernel processing, not acceptance of a mining configuration: an inner rejection can be recovered as a successful error-effect poke. The lifecycle checks therefore require actual accepted blocks to establish mining progress.
