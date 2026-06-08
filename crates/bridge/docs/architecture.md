# Bridge Architecture Overview

## Scope

This document describes how the `crates/bridge` runtime composes:

- Base contracts (`MessageInbox.sol`, `Nock.sol`)
- The Hoon bridge kernel (`assets/bridge.jam`)
- Rust observers, gRPC services, and posting/signature loops

It is an implementation map for this crate, not chain-level protocol authority.

## Authority Boundaries

- Protocol activation and chain-level authority are external:
  [`PROTOCOL.md`](../../../PROTOCOL.md) and
  [`changelog/protocol/`](../../../changelog/protocol/).
- This document only describes bridge runtime behavior in this repository.

## Component Map

| Layer      | Component                                                      | Responsibility                                                                                                                                | Source   |
| ---------- | -------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- | -------- |
| On-chain   | `contracts/MessageInbox.sol`                                   | UUPS-upgradeable inbox that mints wrapped nock after seeing a 3-of-5 bridge signature set. Tracks node roster, burns, and prevents replay.    | Solidity |
| On-chain   | `contracts/Nock.sol`                                           | Non-upgradeable ERC-20 that lets users burn to exit back to Nockchain and forwards those burns to the inbox.                                  | Solidity |
| Kernel     | `open/hoon/apps/bridge/*.hoon` -> `open/assets/bridge.jam`     | Deterministic state machine for cross-chain hashchain state; stages Base withdrawals with `%base-block-withdrawals-pending`, then commits on ack. | Hoon     |
| Runtime    | `src/runtime.rs`                                               | Asynchronous event router and kernel poke/peek wrapper that feeds causes into the kernel.                                                     | Rust     |
| Observers  | `src/ethereum.rs`, `src/nockchain.rs`                          | Pull data from Base (via `alloy` WS) and from the private nockchain gRPC API, turn them into `BridgeEvent`s, and hand them to the runtime.   | Rust     |
| Interfaces | `src/ingress.rs`, `nockapp_grpc::driver::grpc_listener_driver` | gRPC entry points for peer coordination plus a listener driver for kernel `%grpc` effects.                                                    | Rust     |
| Signing    | `src/signing.rs`, `main.rs::run_signing_cursor_loop`           | Computes proposal hashes from the deposit log, signs them locally, and gossips signatures to peers.                                          | Rust     |

## Runtime Inbound Pipeline

`BridgeRuntime` currently accepts only chain events (`BridgeEvent::Chain`):

1. `BaseBridge::stream_base_events()` emits `ChainEvent::Base` batches.
2. `NockchainWatcher::run()` emits `ChainEvent::Nock` blocks.
3. `KernelCauseBuilder` converts those events into kernel pokes:
   - `base-blocks`
   - `nockchain-block`
4. `BridgeRuntime` sends those pokes to the installed nockapp driver.

Other kernel pokes are injected directly (not via `BridgeRuntime` events):

- `cfg-load` and `set-constants` at boot (`main.rs`)
- `%start` when `--start` is passed
- `%stop` from stop handling logic

## Kernel Effect Sinks

Kernel effects are decoded by IO drivers in `main.rs`:
| Cause tag           | Trigger                                          | Payload                                     | Status      |
| ------------------- | ------------------------------------------------ | ------------------------------------------- | ----------- |
| `base-blocks`       | Batch of Base deposits/withdrawals/settlements   | `Vec<RawBaseBlockEntry>`                    | Implemented |
| `nockchain-block`   | New nockchain page                               | `nockchain_types::tx_engine::common::Page`  | Implemented |
| `proposed-base-call`| Peer-delivered deposit proposal payload          | `ProposedBaseCallData`                      | Implemented |
| `base-call-sig`     | Peer signature routed through ingress            | `EthSignatureParts` + calldata              | Implemented |
| `cfg-load`          | Startup configuration                            | `NodeConfig` parsed from `bridge-conf.toml` | Implemented |
| `set-constants`     | Runtime/operator constants update                | `BridgeConstants`                           | Implemented |
| `stop` / `start`    | Operator or fault-triggered state transition     | `StopLastBlocks` / null tag                 | Implemented |

| Effect tag                                                                | Runtime handling                                                                                                      |
| ------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| `commit-nock-deposits`                                                    | Implemented by `create_commit_nock_deposits_driver`: persists requests to `deposit-queue.sqlite`.                     |
| `stop`                                                                    | Implemented by `create_stop_driver`: transitions local stop state and broadcasts stop to peers.                       |
| `grpc`                                                                    | Implemented by `grpc_listener_driver`: executes `%grpc` effect calls.                                                 |
| `base-call`, `assemble-base-call`, `nockchain-tx`, `propose-nockchain-tx` | Type-level support exists (`types.rs`), but no dedicated bridge IO driver is wired for these tags in `main.rs` today. |

## Signature and Posting Pipeline

Deposit submission is currently driven by deposit-log + cache loops:

1. Kernel emits `commit-nock-deposits`.
2. Commit driver persists requests to `deposit-queue.sqlite`.
3. `run_signing_cursor_loop` polls:
   - on-chain `lastDepositNonce`
   - local deposit queue
4. It signs candidates, inserts signatures into `ProposalCache`, and gossips via `BridgeIngress/BroadcastSignature`.
5. Ingress validates and stores peer signatures in the same cache.
6. `run_posting_loop` selects ready proposals, enforces proposer/failover logic, calls `BaseBridge::submit_deposit`, and then broadcasts `BroadcastConfirmation`.

This means signature collection and Base posting are coordinated by Rust loops around `ProposalCache`, not by a direct ingress-to-runtime event path.
| Effect tag               | Purpose                                                                            | Consumer                                        | Status      |
| ------------------------ | ---------------------------------------------------------------------------------- | ----------------------------------------------- | ----------- |
| `base-block-withdrawals-pending` | Emit a staged Base batch whose derived withdrawal requests must be durably persisted before Base hashchain commit. | withdrawal execution driver | Implemented |
| `commit-nock-deposits`   | Emit structured deposit requests for runtime persistence and signing.               | `create_commit_nock_deposits_driver` in `main.rs` | Implemented |
| `grpc`                   | Make gRPC calls: `peek` for queries, `call` for RPC invocations.                   | gRPC listener driver                            | Implemented |
| `stop`                   | Freeze processing in kernel and propagate stop to peers.                            | `create_stop_driver` in `main.rs`               | Implemented |

Note: when nodes encounter STOP, the kernel transitions to a STOPPED state and no longer processes
new pokes. Drivers in the Rust runtime will spin. If the process is restarted, the
kernel will still be STOPPED, however, the drivers will be restarted.

Effect handling is implemented directly by IO drivers registered in `main.rs`.
Right now we ship drivers for `%grpc`, `%commit-nock-deposits`, `%stop`, markdown
(debug UI), and exit handling. Deposit submission to `MessageInbox` is driven by
the runtime signing/posting loops after `%commit-nock-deposits` has been
persisted to the local deposit log.

## Critical Flows

### Deposit (Nockchain -> Base)

1. `NockchainWatcher` notices a deposit on the heavy chain and emits a
   `nockchain-block` cause.
2. The kernel updates its dual-hashchain state and emits a `commit-nock-deposits`
   effect containing a list of `nock-deposit-request` structures with fields:
   tx-id, name, recipient, amount, block-height, as-of.
3. The `create_commit_nock_deposits_driver` appends the requests to the local
   deposit log so nonce assignment is deterministic across restarts.
4. The signing cursor loop reads from the deposit log, assigns nonces, signs
   proposals, and gossips signatures to peers.
5. The proposal cache aggregates signatures per `DepositId`; once threshold is
   reached, the posting loop constructs `DepositSubmission` and calls
   `BaseBridge::submit_deposit`.
6. MessageInbox validates signatures and nonce ordering, then emits
   `DepositProcessed`, which the Base observer feeds back into kernel state.

### Withdrawal (Base -> Nockchain)

1. Users burn wrapped tokens through `Nock.sol::burn`, which emits
   `BurnForWithdrawal` and calls `MessageInbox.notifyBurn`.
2. `BaseBridge::stream_base_events` captures the burn event, packages it as
   `BaseWithdrawalEvent`, and emits a `base-blocks` cause so the kernel can
   queue settlement work.
3. The kernel emits `base-block-withdrawals-pending` with the batch identity and
   derived `nock-withdrawal-request` entries.
4. Rust persists those requests idempotently, sends
   `base-block-withdrawals-committed`, and the kernel commits the Base batch.

## Reference Files

- `crates/bridge/src/main.rs`
- `crates/bridge/src/runtime.rs`
- `crates/bridge/src/ethereum.rs`
- `crates/bridge/src/nockchain.rs`
- `crates/bridge/src/ingress.rs`
- `crates/bridge/src/deposit_log.rs`
- `crates/bridge/src/stop.rs`
- `crates/bridge/src/types.rs`
