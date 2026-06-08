# Bridge Node Runbook (Runtime and Incident Operations)

Use this runbook after a node is already provisioned.
This is the canonical runtime and incident-response document for bridge operators.

## Scope

- In scope: start/restart procedures, runtime health, monitoring, and incident response.
- Out of scope: initial provisioning and key onboarding, which are documented in [`../OPERATOR-SETUP.md`](../OPERATOR-SETUP.md).

## Entry Conditions

- Node config is already provisioned via [`../OPERATOR-SETUP.md`](../OPERATOR-SETUP.md).
- If you are using script-driven environments, profile selection is done via [`../scripts/environments/README.md`](../scripts/environments/README.md).

## 1. Boot and Restart Procedure

1. `cd crates/bridge`.
2. Launch:

   ```bash
   cargo run -p bridge -- --config-path /abs/path/to/bridge-conf.toml
   ```

3. Optional flags:
   - `--new` wipes on-disk kernel state, use only when you intend to reset state.
   - `--start` sends `%start` on boot if the kernel is STOPPED.
4. Confirm startup logs include:
   - `bridge nockapp started`
   - `loaded config from ...`
   - `Base bridge and signer initialized successfully`
   - `connected to nockchain gRPC endpoint`
   - `starting bridge ingress gRPC server`

## 2. Routine Operations

### Watchers

- Base watcher (`src/ethereum.rs`) polls confirmed windows every 30s over the Base WebSocket provider. If logs show `base reorg detected` or repeated `failed to get block number after retries`, pause submissions and investigate chain health.
- Nock watcher (`src/nockchain.rs`) polls private nockapp (`heavy`, `heavy-n`) every 10s by default. Repeated `failed to fetch tip height` or `failed to fetch block at height` usually indicates private nockapp availability or decode-path issues.

### Runtime and Kernel

- `BridgeRuntime` throttles pending events to 1024. `dropping oldest pending event` indicates backlog pressure.
- `%commit-nock-deposits` effects are persisted to `deposit-queue.sqlite` by the commit driver.
- Signature gossip is handled by the signing cursor loop. Monitor `bridge.cursor` for `broadcast signature to peer`, `failed to broadcast signature to peer`, and nonce divergence alerts.

### Ingress

- gRPC ingress (from `ingress_listen_address`) serves:
  - `bridge.ingress.v1.BridgeIngress`
  - `bridge.status.v1.BridgeStatus`
  - `bridge.tui.v1.BridgeTui`
- For bridge-to-bridge operation, ingress handles `BroadcastSignature`, `BroadcastConfirmation`, and `BroadcastStop` (plus `HealthCheck` and `GetProposalStatus`).

### Status gRPC

- Status endpoint: `bridge.status.v1.BridgeStatus/GetStatus` on ingress gRPC.
- Replace `<INGRESS_ADDR>` with your configured `ingress_listen_address` (for example `127.0.0.1:8001` or `127.0.0.1:8002`).
- Inspect API:

  ```bash
  grpcurl -plaintext <INGRESS_ADDR> list
  grpcurl -plaintext <INGRESS_ADDR> describe bridge.status.v1.BridgeStatus
  ```

- Query:

  ```bash
  grpcurl -plaintext -format json -d '{}' <INGRESS_ADDR> bridge.status.v1.BridgeStatus/GetStatus
  ```

- `lastSubmittedDeposit` appears only after this process successfully submits a deposit and resets on restart.

## 3. Common Operational Tasks

| Task                         | Steps                                                                                                                                                                                                        |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Rotate a bridge node address | Use MessageInbox owner (Gnosis multisig) to call `updateBridgeNode(index, newNode)`, then update every operator config with the new `eth_pubkey`.                                                            |
| Update contract addresses    | Deploy new contracts, update `contracts/deployments.json` (if you rely on deployment lookup), and/or set explicit `inbox_contract_address` + `nock_contract_address` in each operator config before restart. |
| Rebuild the kernel           | Regenerate `assets/bridge.jam`, rebuild/release bridge binaries, then rolling-restart nodes. Use `--new` only when a state reset is intentionally required.                                             |
| Verify deposit submission    | Tail for `Deposit processed on MessageInbox` and cross-check tx hash in Base explorer.                                                                                                                       |

## 4. Incident Response

1. Base RPC degradation: repeated `failed to get block number after retries` or submission errors (`Transaction failed`, `Transaction reverted`) indicate provider/outage or contract-path issues. Verify provider health first, then restart only if the process does not recover.
2. Submission stalls: posting is strict on `lastDepositNonce + 1`. If chain nonce does not advance, inspect posting-loop errors and confirm no nonce epoch mismatch alerts.
3. Nonce divergence: alerts like `Nonce Divergence Suspected` (ingress/cursor) indicate peers disagree on proposal hash for the same deposit ID. Pause submissions and reconcile nonce-epoch config and deposit-log state across operators.
4. Signature starvation: if proposals stay in `collecting`, verify peer health (`HealthCheck`), ingress reachability, and that peer signatures are being broadcast/accepted.
5. STOP state: if the kernel enters STOPPED state, restart without `--new` and pass `--start` only after confirming it is safe to clear stop state.

## 5. Shutdown and Recovery

1. Send SIGINT and allow graceful shutdown.
2. Confirm Base watcher, nock watcher, runtime, ingress, and app tasks exit cleanly.
3. Restart without `--new` to resume from persisted noun state; add `--start` only when recovering from STOPPED kernel state.

## 6. Logging and Monitoring

Logs persist under `{data_dir}/logs/` (typically `~/.nockapp/bridge/logs/`).

Override log directory:

```bash
cargo run -p bridge -- --log-dir /var/log/bridge
```

Set verbosity with `RUST_LOG`:

```bash
RUST_LOG=info cargo run -p bridge
RUST_LOG=bridge::shared::runtime=debug,bridge::shared::base=trace cargo run -p bridge
RUST_LOG=debug cargo run -p bridge
```

Useful queries:

```bash
tail -f ~/.nockapp/bridge/logs/bridge.log
grep -r "ERROR\\|WARN" ~/.nockapp/bridge/logs/
grep "bridge.base.observer\\|bridge.posting\\|bridge.cursor" ~/.nockapp/bridge/logs/bridge.log
grep "Deposit processed on MessageInbox\\|posting proposal to BASE\\|failed to post deposit" ~/.nockapp/bridge/logs/bridge.log
```

Alert on:
- Base observer and posting-loop errors (`bridge.base.observer`, `bridge.posting`)
- Nock watcher connectivity/decode warnings (`bridge.nock-watcher`)
- Runtime dropped-event warnings (`bridge.runtime`)
- Ingress broadcast validation failures (`bridge.ingress`)

## Handoff

- Need initial provisioning or key/node assignment work: [`../OPERATOR-SETUP.md`](../OPERATOR-SETUP.md)
- Need to switch script environment profiles or update env vars: [`../scripts/environments/README.md`](../scripts/environments/README.md)
- Need local first boot workflow: [`../QUICKSTART.md`](../QUICKSTART.md)
