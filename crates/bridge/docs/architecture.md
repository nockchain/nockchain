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

The deployed Base (mainnet) wrapped-NOCK ERC-20 token — the `Nock.sol` contract above — is at [`0x9B5E262cF9bb04869ab40b19AF91D2dc85761722`](https://basescan.org/address/0x9B5E262cF9bb04869ab40b19AF91D2dc85761722); a bridge deposit mints this token on Base. Wallet-side deposits are built via `create-tx --bridge-deposit <nocks> --to-evm-address <0x...>` (see the [wallet README](../../nockchain-wallet/README.md#bridge-deposits)). The bridge enforces a minimum deposit of 100,000 nocks (6,553,600,000 nicks) and charges a 0.3% fee on the deposited amount.

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

## Bridge State-Machine Safety Invariants

These invariants are enforced by the Hoon kernel unless a responsibility is
explicitly assigned to the Rust runtime or an on-chain contract.

### Canonical input and cursor invariants

1. Observers feed only blocks past their configured confirmation depth. The
   kernel then requires one canonical continuation: the next Nockchain height
   and parent must match exactly, and every Base batch must start at the next
   Base height, contain the configured number of blocks, and preserve parent
   linkage.
2. A source-chain hash is the hash of the complete cooked `nock-block` or
   `base-blocks` value. Settlement lookup never substitutes an RPC block ID,
   transaction ID, or height for that hash.
3. A parent mismatch, skipped/replayed cursor, malformed batch, or contradictory
   finalized history is a STOP condition. The kernel never selects an alternate
   branch.
4. Bridge constants and source-chain configuration are consensus inputs for the
   bridge nodes. Nodes must use the same start heights, Base batch size, bridge
   lock root, contract, and event filters.

### Counterpart and settlement identity invariants

1. A deposit settlement binds its Base event ID and map key to one Nockchain
   source tuple: `(as_of, nock_height, note_name, recipient, amount)`.
   `as_of` must identify a stored Nock block at `nock_height`; that block must
   contain `note_name`; the corresponding unsettled deposit must exist; and
   recipient and amount must match.
2. A withdrawal settlement binds its Nock note-name map key to one Base source
   tuple: `(as_of, base_batch_end, base_event_id, lock_root)`.
   `as_of` must identify a stored Base batch ending at `base_batch_end`; that
   batch must contain `base_event_id`; the corresponding unsettled withdrawal
   must exist; and the lock root must match.
3. Kernel withdrawal reconciliation enforces the current amount safety bound
   `0 < settled_amount < burned_amount`. Exact fee and net-amount policy belongs
   to Rust proposal validation and the sequencer; the kernel does not claim to
   prove that economic policy.
4. Embedded IDs must equal their map keys. A known `as_of` with a missing
   counterpart, a consumed unsettled entry, a height mismatch, or a content
   mismatch is a STOP condition, never an ignored event.
5. A Base batch may settle each Nock note at most once, and a Nock block may
   settle each Base event ID at most once, even when the duplicate settlements
   use different outer hashes or settlement note names.

### Deferred cross-chain dependency invariants

1. An unknown `as_of` is deferred only when its claimed source position has not
   been consumed: a deposit settlement requires
   `nock_height >= nock_hashchain_next_height`, and a withdrawal settlement
   requires `base_batch_end >= base_hashchain_next_height`.
2. Deferred settlements are globally unique by value-release counterpart,
   independent of their outer hash bucket: one deferred deposit settlement per
   Nock note name and one deferred withdrawal settlement per Base event ID.
   A new or persisted deferred entry also must not name an already-unsettled
   counterpart tracked under a different hash.
3. When a source block arrives, any deferred entry that names one of its
   counterpart events under a different `as_of` stops processing before a new
   proposal or persistence effect is emitted. If the claimed height or batch
   end is reached without the claimed hash, processing also stops.
4. A matching deferred entry is validated against the full identity above,
   removes the unsettled counterpart, and is then deleted. A valid
   settlement-first and counterpart-first history therefore converges to the
   same kernel settlement state.
5. Deferred maps survive restart and `%start`. Legacy `base-hold` and
   `nock-hold` fields exist only for state migration and are cleared on start;
   new cross-chain dependencies do not create global holds.

### Atomicity, effects, and failure invariants

1. Base batches are fully validated before
   `%base-block-withdrawals-pending` is emitted. This includes every deposit
   settlement and every deferred withdrawal that could suppress a proposal, so
   invalid input cannot create durable Rust withdrawal work.
2. A Base batch is staged until Rust durably persists its exact withdrawal
   list and acknowledges `(blocks_hash, first_height, last_height)`. While that
   commit is pending, neither Base nor Nock input advances. Only the matching
   acknowledgement applies deposit settlements, clears the pending record, and
   commits the Base hashchain.
3. Nock block processing is atomic in the kernel. Any settlement failure
   returns the pre-block state and emits only STOP; deposit proposal effects are
   constructed after the entire block succeeds.
4. STOP is fail-closed and persistent. Operators must investigate the
   contradictory chain data, corrupted state, or signer-authorized malformed
   settlement rather than skipping it.
5. The two source chains otherwise progress independently. If a counterpart is
   observed before its already-existing remote settlement, a proposal may have
   reached durable Rust storage before that settlement is observed locally.
   Runtime queues and submission paths must therefore remain idempotent and use
   confirmed chain state as authority. Settlement-first replay is suppressed by
   the deferred-map rules above.

### Authority and finality assumptions

1. `MessageInbox` accepts `DepositProcessed` only through its configured bridge
   signature threshold and enforces transaction replay and nonce ordering.
   Nock withdrawal settlements spend bridge-controlled notes. Deferred-map
   growth is therefore reachable only through bridge-authorized settlement
   events, not arbitrary public payloads; unexpected growth is an operational
   signal of signer compromise, malformed authorized data, or state corruption.
2. Kernel validation is defense in depth and remains mandatory even for
   authorized settlements. Threshold authorization does not permit a settlement
   to bind to the wrong source hash, height, event, recipient, or amount.
3. The Hoon `deposit-settlement` mold does not retain the Nock transaction ID
   carried by `DepositProcessed`; transaction-ID replay protection is therefore
   an explicit `MessageInbox` and Rust-runtime responsibility. Conversely,
   Nockchain has no withdrawal nullifier keyed by the Base event ID, so the
   sequencer's one-authorized-submission rule remains part of the withdrawal
   safety boundary.
4. The bridge assumes observer confirmation depth makes source inputs final. A
   deeper reorganization is not reconciled silently: its resulting cursor or
   parent contradiction stops the bridge.

## Reference Files

- `crates/bridge/src/main.rs`
- `crates/bridge/src/runtime.rs`
- `crates/bridge/src/ethereum.rs`
- `crates/bridge/src/nockchain.rs`
- `crates/bridge/src/ingress.rs`
- `crates/bridge/src/deposit_log.rs`
- `crates/bridge/src/stop.rs`
- `crates/bridge/src/types.rs`
