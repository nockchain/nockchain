# Bridge Withdrawals

Status: Active
Owner: Nockchain Maintainers
Last Reviewed: 2026-04-01
Canonical/Legacy: Canonical bridge withdrawal protocol and implementation spec

## Scope

This document specifies the implementation work required to enable bridge withdrawals (Base -> Nockchain) across:

1. Rust bridge crate (`open/crates/bridge`)
2. Hoon bridge kernel (`open/hoon/apps/bridge/bridge.hoon`, plus `base.hoon`, `nock.hoon`, `types.hoon`)

This spec focuses on:

1. The current implemented withdrawal protocol shape
2. The intended steady-state coordination model
3. Remaining hardening gaps and policy choices
4. How to validate correctness

## Current Implementation Status

### Kernel status (Hoon)

1. Nock-side withdrawal tx detection is now wired (no intentional hard-stop):
   - `open/hoon/apps/bridge/nock.hoon`
   - arm: `++ process-nock-txs`
   - branch: `is-bridge-withdrawal-tx`
   - current behavior: detects packed `%bridge-w` note-data, parses withdrawal metadata, and builds `withdrawal-settlement` entries (non-matching/malformed outputs are skipped, not fatal)

2. Nock-side settlement processing no longer intentionally rejects withdrawals:
   - `open/hoon/apps/bridge/nock.hoon`
   - arm: `++ nockchain-process-withdrawal-settlements`
   - current behavior: reconciles settlements against tracked withdrawals by counterpart identity and destination, enforces `0 < settled_amount < burned_amount`, emits hold when referenced `as_of` base hash is unknown, stops on irreconcilable counterpart issues or out-of-bounds settlement amounts, and clears matched unsettled entries

3. Base-side withdrawal proposal arm is now implemented:
   - `open/hoon/apps/bridge/base.hoon`
   - arm: `++ base-propose-withdrawals`
   - current behavior: returns `(list nock-withdrawal-request)` which is included in `%base-block-withdrawals-pending`

4. Withdrawal data/effect surface was updated:
   - `open/hoon/apps/bridge/types.hoon`
   - `withdrawal` now uses `dest=nock-lock-root` and no fee field
   - `withdrawal-settlement` now includes `base-batch-end` (including hashable encoding) and no `nock-tx-fee`
   - effects now include `%base-block-withdrawals-pending`,
     `%base-block-withdrawals-committed`,
     `%withdrawal-proposal-built`, `%withdrawal-tx-signed`, `%grpc`, and
     `%stop`
   - `%create-withdrawal-tx` exists as the dedicated poke/cause for asking the
     bridge app tx-builder to construct a withdrawal proposal from explicit
     inputs / withdrawal data
   - `%sign-tx` exists as the dedicated poke/cause for asking Rust-side
     withdrawal coordination to sign the transaction inside a
     `withdrawal-proposal`
   - current behavior: `++ set-blockchain-constants` stores the connected
     node's tx-engine constants, `++ evaluate-create-withdrawal-tx` builds a
     full `withdrawal-proposal`, and `++ evaluate-sign-tx` signs that
     proposal; there is no dedicated `%withdrawal-terminal` effect surface
     today

### Runtime status (Rust)

1. Core runtime now wires both deposit and withdrawal execution:
   - `open/crates/bridge/src/main.rs`
   - `open/crates/bridge/src/shared/runtime.rs`
   - `open/crates/bridge/src/withdrawal/runtime.rs`
   - active loops now include withdrawal assembly, signing, submission, and
     sequencer confirmation polling alongside the existing deposit path

2. Rust effect surface now decodes the live withdrawal kernel effects:
   - `open/crates/bridge/src/shared/types.rs`
   - withdrawal-related variants:
    `BridgeEffectVariant::BaseBlockWithdrawalsPending(PendingBaseBlockCommit)`,
     `BridgeEffectVariant::WithdrawalProposalBuilt(WithdrawalProposalData)`,
     and `BridgeEffectVariant::WithdrawalTxSigned(WithdrawalProposalData)`
   - legacy `BridgeEffectVariant::CreateWithdrawalTxs(...)` remains decodable but is not live execution work
   - there is no `BridgeEffectVariant::WithdrawalTerminal(...)` today

3. Legacy runtime local effect queue was removed:
   - `open/crates/bridge/src/shared/runtime.rs`
   - `BridgeRuntime::process_effect` and `send_effect` no longer exist
   - effect consumption is now driver-specific (e.g., stop/deposit drivers)

4. Live effect handling now includes the withdrawal execution driver:
   - `open/crates/bridge/src/deposit/log.rs`
   - `open/crates/bridge/src/withdrawal/assembly.rs`
   - `open/crates/bridge/src/main.rs` registers both
     `create_commit_nock_deposits_driver` and
     `create_withdrawal_execution_driver`
   - `open/crates/bridge/src/withdrawal/guard.rs` still exists as historical
     milestone-1 scaffolding, but it is not the production path

5. Ingress now serves both deposit and withdrawal coordination:
   - `open/crates/bridge/proto/bridge_ingress.proto`
   - `open/crates/bridge/src/shared/ingress.rs`
   - deposit proposal cache remains deposit-specific, but withdrawals use
     `WithdrawalProposalTransport` and dedicated ingress RPCs for proposal,
     canonicalization, and signed-proposal broadcast

6. Withdrawal proposal validation and tracking belong to Rust:
   - there is no kernel proposal-validation cause in the intended design
   - Rust owns proposal-envelope validation against withdrawal identity,
     snapshot/selected-note inputs, epoch legality, replay/equivocation rules,
     and durable per-withdrawal proposal tracking
7. Base burn events are observed and minimum-gated before they become tracked
   withdrawals:
   - `open/crates/bridge/src/ethereum.rs`
   - `process_nock_log` decodes `BurnForWithdrawal`
   - generated local `Withdrawal` currently has `dest: None`
   - `open/hoon/apps/bridge/base.hoon` now rejects burns at or below the
     configured minimum before they materialize into withdrawal state

8. Withdrawal-specific coordination state is live:
   - a durable withdrawal state-store record of prepared / peer-canonical /
     authorized / submitted / confirmed withdrawals exists in Rust
   - append-only withdrawal lifecycle storage and sequencer-owned reserved-note
     projections now exist
   - the main bridge binary now performs a boot-time
     `blockchain-constants` handshake against the connected private Nockchain
     node rather than guessing mainnet/fakenet tx-engine constants
   - if bridge kernel state does not already contain `blockchain-constants`,
     the handshake result is written there for the withdrawal tx-builder seam
   - if bridge kernel state already contains `blockchain-constants`, the
     bridge validates the connected private node against that existing kernel
     value during boot and any mismatch is a hard stop
   - because the current bridge state shape is still undeployed, this
     constants-bearing state was batched into the existing live design without
     introducing another bridge-state version bump
   - the ingress-side withdrawal proposal transport remains in the main bridge
     process, while the sequencer gRPC surface is now hosted on the Nockchain
     API node process
   - bridge nodes register derived `withdrawal_nonce` ordering with the
     sequencer and ask the sequencer before signing, authorizing, or
     submitting when submission-lifecycle truth matters
   - the production `%withdrawal-proposal-built` path now persists and
     broadcasts built proposals to peers
   - the API-node-hosted sequencer now owns first confirmation polling against
     the colocated public Nockchain API, while the main bridge process keeps
     the confirmed-block watcher for kernel reconciliation and note-snapshot
     refresh
   - `%create-withdrawal-tx` and `%sign-tx` are now real kernel execution
     seams rather than stubs

### Contracts and dependencies

1. Contracts support burn-side initiation:
   - `open/crates/bridge/contracts/Nock.sol`: `burn(amount, lockRoot)` emits `BurnForWithdrawal`
   - `open/crates/bridge/contracts/MessageInbox.sol`: `notifyBurn` + `withdrawalsEnabled` gate

2. No immediate `Cargo.toml` dependency gap identified for baseline implementation:
   - `open/crates/bridge/Cargo.toml`
   - nockapp gRPC client dependencies already present

## Target Withdrawal Flow

1. User burns wrapped NOCK on Base (`Nock.sol::burn`), event emitted with `lockRoot`.
2. Rust Base observer ingests burn, emits `%base-blocks` cause to kernel.
3. Kernel stores unsettled withdrawals keyed by `(as_of base hash, base_event_id)`.
   Base-side kernel processing now enforces the withdrawal minimum first; only
   burns strictly above the configured minimum become withdrawals. We are
   targetting a minimum of 10,000 NOCKS.
4. Kernel emits `%base-block-withdrawals-pending` with the Base batch identity and derived `nock-withdrawal-request` payloads.
5. Runtime persists those requests idempotently, sends `%base-block-withdrawals-committed`, and then treats each request as a single-withdrawal coordination unit. `base-batch-end` remains part of withdrawal metadata, but settlement coordination is per-withdrawal rather than multi-withdrawal batching.
6. For a given withdrawal id `(as_of, base_event_id)`, a deterministic epoch
   leader selects a pinned balance snapshot `{height, block_id}` and runs a
   withdrawal-specific planner over the bridge-owned note pool. That planner
   takes the gross burned amount as input and solves for:
   - selected input notes
   - final fee
   - net disbursed amount on Nockchain
   The generic create-tx planner remains the net-gift interface; withdrawals
   use a separate top-level entrypoint on top of the same internal
   selection/fee/conservation engine.
7. The leader pokes `%create-withdrawal-tx` with the withdrawal id, epoch,
   pinned snapshot metadata, computed net disbursed amount, gross burned
   amount, and selected input note names.
8. The kernel bridge tx-builder emits `%withdrawal-proposal-built`, carrying
   the full `withdrawal-proposal`, including:
   - the exact wallet `transaction` and its unique `name`
   - `amount` as the net Nockchain payout
   - `burned_amount` as the originating gross Base burn
9. The kernel tx-builder reads `blockchain-constants` from bridge state, which
   were stored there earlier by the boot-time private-node handshake.
10. Rust persists that proposal envelope and broadcasts it to peers from the
    production `%withdrawal-proposal-built` execution path.
11. Peers validate the proposal against kernel state and local bridge-owned note
    / tx-builder state, durably persist the exact envelope, and commit to at
    most one proposal hash for that `(withdrawal_id, epoch)`.
12. Once a threshold of peers has committed to the same exact proposal hash,
    that proposal becomes the peer-canonical candidate for that withdrawal and
    epoch.
    - The peer commit message should be domain-separated as:
      `keccak256("bridge.withdrawal.commit.v1" || withdrawal_id || epoch || proposal_hash)`.
    - Canonicalization transport now carries a commit certificate over that
      exact tuple.
    - Peer transport verifies commit signatures against configured bridge
      operator Ethereum addresses before accepting canonicalization gossip.
13. Peer canonicalization alone is not enough to make a withdrawal submit-ready.
    The sequencer gRPC service may only authorize a proposal after it has the
    full required witness signatures merged into the transaction, and it
    requires a valid threshold commit certificate for the peer-canonical
    proposal. `authorized` means "fully signed and submit-ready", not merely
    "chosen by the sequencer".
14. Only the sequencer gRPC service may finalize and submit a withdrawal tx.
    That service should run only on the designated Nockchain API node started
    with `--enable-withdrawal-sequencer`. It maintains the authoritative submitted /
    in-flight withdrawal set, orders withdrawals by derived
    `withdrawal_nonce`, and must not authorize the same withdrawal twice.
15. Pre-canonical timeout splits into two cases:
    - if a withdrawal is only `Assembling` when the assembly timeout elapses,
      the sequencer advances the shared pre-canonical handoff on the same
      epoch and the next handoff owner may retry that same epoch
    - if a withdrawal is already `Prepared` when the timeout elapses, the
      built but still provisional proposal is expired and the next epoch
      leader may assemble a replacement candidate tx for the same withdrawal
      id
16. The sequencer durably records the authorization / submission /
    confirmation lifecycle and owns the global in-flight gate for withdrawals.
    Only one withdrawal may be
    sequencer-authorized / submitted / unconfirmed at a time.
17. If the sequencer is unavailable, withdrawals pause. There is no automatic
    submission failover.
18. Authoritative progress is chain-observable, not gossip-observable. The
    bridge should distinguish:
    - diagnostic accepted state via `tx-accepted`
    - confirmed inclusion via observed settlement in a confirmed block
    A local "submitted" event is advisory only; sequencer authorization is the
    required precondition for submission, not a consensus fact on its own.
19. Withdrawal execution must fail closed if the bridge cannot fetch full
    `blockchain-constants` from the connected private Nockchain node. There is
    no fallback "guess fakenet vs mainnet" path for fee estimation or tx
    validity.
20. On startup, the bridge peeks `blockchain-constants` from bridge kernel
    state first. If the kernel has no value yet, the bridge fetches constants
    from the connected private Nockchain node and seeds kernel state. If the
    kernel already has a value, the bridge compares the connected node against
    that existing kernel value and stops on mismatch rather than overwriting it.
21. The sequencer service observes transaction-to-block inclusion directly from
    the colocated public Nockchain API and durably records confirmation for
    the authorized withdrawal before clearing its authoritative in-flight
    record. The bridge's confirmed-block watcher remains relevant for kernel
    reconciliation and note-snapshot refresh, not for first confirmation
    ownership.
22. The sequencer observes block inclusion directly from the colocated public
    Nockchain API, records confirmation for the authorized withdrawal, and
    clears the matching reserved-input / in-flight state.
23. Kernel independently reconciles settlement with the counterpart withdrawal
    on the confirmed Nock block stream using counterpart identity, destination
    equality, and the basic bound `0 < settled_amount < burned_amount`, then
    clears unsettled withdrawal state. Exact fee correctness remains a Rust
    proposal-validation concern. There is no dedicated `%withdrawal-terminal`
    effect on the current Rust bridge surface.
24. If settlement references an unknown `as_of` base hash, hold logic blocks
    advancement until that base hash is ingested. If `as_of` is known but
    counterpart data is inconsistent/missing, processing stops.

## Coordination Model

1. The coordination unit is one withdrawal, keyed by `(as_of, base_event_id)`. Once admitted, a withdrawal remains live until confirmed. Withdrawal settlement is not coordinated as a multi-withdrawal batch.
2. Pre-canonical failover has two recovery paths:
   - `Assembling` failover: if local assembly stalls before a proposal is
     built, the sequencer advances pre-canonical handoff on the same epoch and
     the next handoff owner retries that same epoch
   - `Prepared` expiry: if a built but still provisional proposal fails to
     become canonical before timeout, the operator expires that attempt and
     the next epoch leader assembles a replacement tx
3. A proposal becomes peer-canonical only after a threshold of bridge nodes has:
   - validated the same exact proposal hash
   - durably persisted the proposal envelope locally
   - committed to that proposal for the given `(withdrawal_id, epoch)`
4. Peer canonicalization is not enough to create a submit-ready withdrawal. A withdrawal may only advance past peer-canonical state when the sequencer durably authorizes a fully signed proposal.
5. `authorized` means the sequencer has one exact proposal hash / transaction name for the withdrawal and that proposal now carries the full required witness signatures for chain-valid submission.
6. "Submitted" is not a consensus fact. It is a local event recorded in the withdrawal state store only after sequencer authorization. The protocol must not require all nodes to agree on whether a submit RPC was attempted.
7. Proposal assembly, peer canonicalization, submission, timeout, and supersession tracking live in runtime attempt machinery. They are not kernel withdrawal states.
8. There is one sequencer gRPC service for withdrawals. That sequencer service
   owns the authoritative submitted / in-flight withdrawal set and the durable
   confirmation record for authorized withdrawals.
9. Only one withdrawal may be sequencer-authorized / submitted / unconfirmed at a time.
10. If the sequencer is unavailable, withdrawals stop. There is no automatic submission failover.
11. Because there is no chain-enforced withdrawal nullifier today, peer threshold agreement alone is not a sufficient duplicate-withdrawal safety mechanism. The sequencer is part of the authorization boundary for withdrawals.

## Bridge-Owned Note Snapshot

1. The withdrawal pipeline does not depend on the standalone `nockchain-wallet` application.
2. The bridge runtime uses the Rust note selector and bridge-owned tx-builder flow, with bridge-owned note state fetched from the private nockchain API.
3. The bridge maintains a confirmed note snapshot for the bridge-controlled spend authority / note pool.
4. This confirmed snapshot should refresh whenever the bridge observes a newly confirmed nockchain block.
5. In practice, snapshot freshness is bounded by:
   - nockchain block production cadence
   - configured nockchain confirmation depth
   - nock watcher poll interval
6. The safe Nockchain tip for note selection is `snapshot_height - nockchain_confirmation_depth`, saturated at zero.
7. A cached confirmed snapshot may be reused across multiple withdrawal assemblies, provided the planner filters out notes newer than the safe Nockchain tip and subtracts currently reserved input notes before selecting new inputs.
8. The runtime may also trigger an on-demand snapshot refresh before assembly if its cached confirmed snapshot is stale.

## Reservation Lifecycle

1. Input note reservations are sequencer-owned and must be persisted durably.
2. `Assembling` and `Prepared` are pre-canonical local operator states only;
   they do not create reserved-input rows on their own.
3. A proposal that becomes peer-canonical creates the reservation for its
   selected note names in the sequencer-owned live reserved-note set.
4. Canonical reservations prevent later assemblies from reusing inputs that
   belong to a peer-canonical / authorized / mempool-accepted withdrawal tx.
5. If `Assembling` times out before any proposal is built, the operator
   releases the local assembly lock and the sequencer advances same-epoch
   pre-canonical handoff. No reservation release is needed because none
   existed yet.
6. If `Prepared` times out before canonicalization, the operator expires that
   built-but-provisional attempt and the next epoch must be assembled. No
   canonical reservation release is needed because reservations still begin
   only at peer canonicalization.
7. If a sequencer-authorized tx fails to confirm, the withdrawal remains live.
   Recovery and replacement-attempt policy belongs to the runtime attempt
   machinery for that same still-unconfirmed withdrawal.
8. Canonical reservations are released only when the sequencer records
   confirmed settlement for the authorized withdrawal; proposer handoff and
   authorized submit failures do not release them.
9. A local submit RPC attempt is not enough to release or modify reservations.
10. Reservation release remains sequencer-owned. The kernel does not currently
    emit a dedicated withdrawal terminal/release effect, and it should never
    emit raw note names as part of settlement reconciliation.

## Persistence and Tables

### Source of Truth

1. The source of truth is append-only.
2. Reserved input notes must be recorded in the append-only log, not only in mutable current-state projections.
3. Mutable state exists only for fast lookup and operational convenience.
4. On startup, the sequencer rebuilds its current views from the append-only log, then reconciles against observed chain state.
5. This spec cares about the logical state that must exist, not the exact SQL schema. The implementation may use multiple physical tables to realize these views.

### Required Logical State

1. Append-only withdrawal lifecycle log.
   - One entry per lifecycle event.
   - Each entry must identify the withdrawal, epoch, proposal hash, transaction name, event type, and any snapshot / confirmation data needed to rebuild current state.
   - Input note names referenced by an event must also be durably recorded as part of the append-only log.
2. Current reserved-note set.
   - Pure live exclusion set used by planning.
   - Answers only: which input notes are blocked right now, and by which withdrawal attempt?
   - Must be rebuildable from the append-only log.
3. Current withdrawal state-store projection.
   - One current record per tracked withdrawal under sequencer control.
   - In bridge-node code, "live withdrawal" specifically means an active
     operator attempt in `Assembling`, `Prepared`, `PeerCanonical`,
     `Authorized`, or `MempoolAccepted`; `Pending` is tracked-but-not-live, and
     `Confirmed` is terminal.
   - Tracks the current epoch, the current assembly / prepared / peer-canonical / authorized / submitted / confirmed phase, and the live peer-canonical / authorized candidate hashes when applicable.
   - Also acts as the live local assembly-lock record; there is no separate staged-request queue.
   - Must be rebuildable from the append-only log.
4. Optional additional projections.
   - Implementations may keep extra projection tables for convenience, compaction, or operator queries.
   - Those are implementation details, not protocol requirements.

### Required Schemas

1. Append-only withdrawal lifecycle log.
   - Required fields:
     - `withdrawal_id = (as_of, base_event_id)`
     - `epoch`
     - `proposal_hash`
     - `transaction_name`
     - `event_type`
     - `created_at`
     - `snapshot_height` nullable
     - `snapshot_block_id` nullable
     - `confirmed_height` nullable
     - `confirmed_block_id` nullable
     - `transaction_jam` nullable
     - `input_note_names[]`
   - This may be realized as one physical table or as a header table plus child rows for input note names.
2. Live reserved-note set.
   - Required fields:
     - `note_name`
     - `withdrawal_id = (as_of, base_event_id)`
     - `epoch`
     - `proposal_hash`
     - `created_at`
3. Live withdrawal state-store projection.
   - Required fields:
     - `withdrawal_id = (as_of, base_event_id)`
     - `withdrawal_nonce`
     - `current_epoch`
     - `state`
     - `proposal_hash` nullable
     - `peer_commit_certificate` nullable
     - `authorized_transaction_name` nullable
     - `created_at`
     - `updated_at`
   - `proposal_hash` is the canonical proposal for the current withdrawal epoch;
     the row `state` describes whether it is peer-canonical, authorized,
     submitted, or confirmed.
   - The current-state projection must be able to represent at least:
     - pending tracked request before it becomes live
     - local assembly in progress
     - prepared but not yet peer-canonical
     - peer-canonical
     - authorized
     - mempool accepted / submitted
     - confirmed before the live projection is cleared or terminalized
   - `authorized_*` fields must refer to the fully signed, submit-ready
     proposal chosen by the sequencer, not merely the current peer-canonical
     candidate.

### Event Types

1. Expected event types include:
   - `proposal_signed`
   - `proposal_canonicalized`
   - `proposer_turn_expired`
   - `proposal_authorized`
   - `proposal_rejected`
   - `proposal_expired`
   - `proposal_superseded`
   - `tx_submitted`
   - `tx_seen_mempool_accepted`
   - `tx_confirmed`

### Append Log Schematics

1. Local assembly attempt:
   - derive stageable work from kernel `unsettled-withdrawals` plus the current live withdrawal projection
   - mark the chosen withdrawal as holding the local assembly lock before note selection / `%create-withdrawal-tx`
   - persist the built proposal locally as `prepared`
   - do not create reserved-note rows yet
   - update the local live withdrawal state-store projection to `prepared`
2. Canonicalization:
   - append `proposal_canonicalized`
   - append the peer-canonical commit certificate alongside the canonicalized
     proposal event
   - insert the selected notes into the live reserved-note set
   - update the live withdrawal state-store projection with the peer-canonical candidate
3. Sequencer authorization:
   - append `proposal_authorized`
   - update the live withdrawal state-store projection with the only authorized proposal hash / transaction name for that withdrawal
   - `proposal_authorized` means the proposal is fully signed and submit-ready
   - do not authorize a second proposal for the same withdrawal while any authorized or submitted state remains live
4. Local rollback before canonicalization:
   - if the state is only `assembling`, advance sequencer-owned pre-canonical
     handoff on the same epoch, release the local assembly lock, and do not
     touch the live reserved-note set
   - if the state is `prepared`, append `proposal_rejected`,
     `proposal_expired`, or `proposal_superseded`, clear the local active
     attempt, and advance to the next epoch for replacement assembly
5. Submission:
   - append `tx_submitted`
   - on mempool acceptance, transition the current withdrawal state to submitted / in-flight
   - if bounded submission fails, keep the withdrawal `authorized` with failure metadata
   - if the public Nockchain gRPC node is unavailable before a submit attempt
     starts, return a deferred response without journaling or updating submit
     metadata
   - do not release reservations
6. Chain-observed progress:
   - append `tx_seen_mempool_accepted` as observed for diagnostics
   - do not release reservations
7. Confirmed observation:
   - when confirmed settlement is observed for an authorized withdrawal,
     append `tx_confirmed`
   - remove the selected notes from the live reserved-note set
   - remove or terminalize the current in-flight withdrawal record
8. Kernel-side settlement reconciliation:
   - after confirmed Nock settlement is observed, kernel clears the matching
     unsettled withdrawal from its own state after counterpart identity,
     destination, and basic amount-bound reconciliation
   - there is no dedicated terminal effect on the current Rust bridge surface,
     so confirmed-path cross-checks rely on sequencer confirmation records plus
     kernel state reconciliation

### Modification Rules

1. Live mutable lifecycle state must never be edited on its own during normal runtime.
2. Lifecycle state changes must happen in the same transaction that appends the corresponding lifecycle event row(s).
3. Retry metadata may be refreshed without a lifecycle event when no transaction was submitted and no lifecycle state changed.
4. The only exception is startup rebuild, where live mutable state may be reconstructed by replaying the append-only log.
5. The live reserved-note set is a pure "currently blocking note names" view.
6. The live reserved-note set must not be treated as a history or audit log.
7. History, audit, restart recovery, and reservation provenance must come from the append-only journal.
8. Runtime sequencer projection mutations must use the store's journaled write
   helper. The helper appends every required remote journal/local history event
   before invoking the SQLite projection mutation closure.
9. The sequencer mirrors each lifecycle event to R2 / S3-compatible durable
   object storage before advancing the local SQLite projection when
   `[sequencer_journal].enabled` is true. This switch defaults to true, so
   production configs fail closed unless durable object storage is configured or
   the operator explicitly disables the mirror.
10. The target configuration is R2-first and named as an object store. The sink
   needs `endpoint`, `bucket`, `region`, `prefix`, `journal_id`,
   `access_key_id`, `secret_access_key`, and a public
   `[sequencer_journal].verifier_address` for the dedicated journal signing
   key. The private signing key must come from the vault-backed
   `WITHDRAWAL_SEQUENCER_JOURNAL_SIGNING_KEY` environment variable. The
   sequencer CLI/env values override bridge config:
   `--sequencer-journal-object-store-endpoint`,
   `--sequencer-journal-object-store-bucket`,
   `--sequencer-journal-object-store-region`,
   `--sequencer-journal-object-store-prefix`,
   `--sequencer-journal-id`,
   `--sequencer-journal-object-store-access-key-id`, and
   `--sequencer-journal-object-store-secret-access-key`. Cloudflare R2 should
   use its S3-compatible endpoint with region `auto`. Prefer env vars for
   credentials in deployed environments.
11. If the R2 / S3-compatible mirror is enabled and a lifecycle event cannot be written
    remotely, the corresponding local sequencer state transition must abort.
    Remote replay/rebuild from the mirrored objects remains a separate recovery
    path. `bridge-dev` generated configs explicitly set
    `[sequencer_journal].enabled = false`.
12. Journal retention is indefinite until sequencer checkpoint pruning lands.
    Do not configure R2 / S3 lifecycle expiration, prefix cleanup, or ad-hoc
    deletion for journal event objects. Current startup recovery requires the
    local cursor's exact remote event object to remain readable; older objects
    can only be removed after checkpoint recovery is implemented and deployed.

### Reservation Queries, Deletion, and Truncation

1. The planner should treat the live spendable set as:
   `spendable_notes = confirmed_snapshot(origin_page <= safe_nockchain_tip) - reserved_inputs`
2. When a note stops blocking planning, it should be removed from the live reserved-note set.
3. Removing a note from the live reserved-note set does not delete its reservation history.
4. Reservation history remains in the append-only journal forever unless it is safely compacted.
5. Safe truncation requires a terminal summary / checkpoint for a withdrawal before detailed event rows are deleted.
6. Detailed event rows must not be truncated for any withdrawal that:
   - is not terminal
   - still owns live reservations
   - may still be resubmitted
   - has not been checkpointed into a terminal summary
7. A practical future compaction path is:
   - write a compact terminal summary row for a finished withdrawal
   - confirm that no reservations or live attempts remain
   - archive or delete the detailed hot-path event rows for that withdrawal
   - optionally run SQLite `VACUUM`
8. Correctness is more important than aggressive truncation. It is acceptable to keep the full append-only withdrawal history and defer compaction.

## Sequencer Recovery and Safe Withdrawal Liquidity

### Summary

1. Sequencer SQLite is a rebuildable projection, not the source of truth.
2. Recovery uses three independent data sources:
   - Base logs for canonical withdrawal burn facts and withdrawal ordering
   - a remote sequencer journal for sequencer-only decisions and exact retry artifacts
   - current Nockchain balance snapshots filtered by the bridge node before
     planning or proposal validation
3. The remote journal should be R2-first in operator-facing configuration and
   documentation. The implementation may use the S3-compatible API because
   Cloudflare R2 and AWS S3 share that protocol surface, but the product framing
   should be "R2 / S3-compatible object store", not AWS S3 by default.
4. Production journal mode is fail-closed. If a required remote journal write,
   read, or continuity check fails, startup or the corresponding mutation must
   fail before any local projection is advanced or any transaction is submitted.
5. The exact recovery target is resume, not merely safe shutdown. A wiped
   sequencer DB should be reconstructable from the remote journal plus Base /
   Nockchain reconciliation, except for explicitly fatal operator-intervention
   cases.
6. Current implementation status: the sequencer can mirror lifecycle events to
   remote object storage before local SQLite projection mutation, assign ordered
   journal sequence numbers from a local SQLite cursor, and list/get ordered
   R2 / S3-compatible journal objects. Startup verifies that the local cursor
   names a real remote event, verifies that the cursor event's projection is
   already present in SQLite, then replays every remote successor into SQLite.
   Base activity replay and Base / Nockchain reconciliation are still deferred
   follow-up phases.

### Remote Sequencer Journal

1. The remote sequencer journal records sequencer-only decisions and artifacts
   that cannot be recovered from chain data.
2. It must be written before the local SQLite projection mutation or external
   transaction submission that depends on that decision.
3. The journal is a global ordered stream. Replay sorts by sequence, verifies no
   gaps, and validates hash continuity.
4. Object key shape:

```text
<r2_prefix>/v1/journals/<journal_id>/events/<sequence_20_digits>-<event_id>.json
```

5. Example:

```text
withdrawal-sequencer/v1/journals/base-84532-bridge-<bridge-id>/events/00000000000000000427-b3_<hash>.json
```

6. `journal_id` must bind the stream to the deployment. It should include, or
   be derived from, Base chain id plus stable bridge identity such as the Nock
   token / inbox address and active bridge lock root.
7. The sequencer is expected to be a single writer. During normal operation it
   assigns the next sequence from `sequencer_journal_cursor`
   `(last_sequence, last_event_id)`, appends the ordered object, then applies
   the local projection and advances the cursor in the same SQLite transaction.
   If remote append succeeds but local projection fails, the cursor remains at
   the prior event and startup recovery can replay the appended successor. On
   startup, the sequencer fetches the exact object named by the local
   non-genesis cursor, verifies its event hash, verifies that SQLite reflects
   that cursor event, then follows `first_after` successors until the remote tail
   is reached. If the cursor object is missing, local SQLite may be ahead of the
   durable log or restored from an incompatible backup and startup fails closed.
8. Journal records are event-only. They do not carry a SQLite projection
   snapshot. Replay must apply each event through a replay-only projector that
   does not append another remote journal record.
9. Appends are ordered only. The object-store PUT uses create-only semantics so
   an already-existing sequence key is treated as a hard failure rather than an
   overwrite.
10. The implementation uses the official AWS Rust S3 SDK (`aws-sdk-s3`) with a
    configurable endpoint, path-style addressing, and R2-compatible checksum
    settings. The bridge does not own custom SigV4 signing or S3 XML parsing.

### Model A Journal Cursor Semantics

1. Model A means the local journal cursor is the exact projection frontier: it
   names the last remote journal event whose SQLite projection is durably
   applied.
2. The write path is `append remote journal event` followed by `apply SQLite
   projection and advance cursor in one SQLite transaction`. Inside SQLite, the
   projection and cursor update are atomic: the cursor cannot advance unless the
   projection commits, and a projection failure rolls back the cursor update.
3. A crash after remote append but before the SQLite transaction commits is a
   recoverable remote/local gap, not a cursor/projection split. The remote
   journal may be ahead, but the SQLite cursor remains at the prior applied
   event. Startup begins from that unchanged cursor and replays the remote
   successor event.
4. A non-genesis cursor must point to a real remote object, and that cursor
   event must be exactly reflected in SQLite before replay begins. Startup
   verifies the row state, withdrawal nonce, epoch, proposal hash, authorization
   artifacts, submit metadata, and reserved-input projection that correspond to
   the cursor event type.
5. A missing or genesis cursor is only valid with an empty replay-owned
   projection. If `sequencer_withdrawals` or `withdrawal_reserved_inputs` has
   rows, recovery refuses to treat the database as fresh.
6. Replay starts at the first successor after the cursor and advances one event
   at a time. Each replayed event is projected and cursor-advanced atomically.
7. If replay sees local projection state already past the event being applied,
   recovery fails closed. That state means the cursor no longer describes the
   exact frontier and the system cannot distinguish a recoverable gap from
   local corruption.
8. Model A deliberately does not treat replay as "skip if already applied" over
   later lifecycle state. Exact resume is preferred over permissive recovery.
9. If a future durable event does not leave enough distinct SQLite state to
   verify exact cursor projection, add explicit last-applied journal metadata to
   the projection instead of weakening the cursor model.

### Sequencer Journal Record Schema

```json
{
  "schema_version": 1,
  "journal_id": "base-84532-bridge-<bridge-id>",
  "sequence": 427,
  "event_id": "b3_<hash_of_canonical_record_without_event_id>",
  "previous_event_id": "b3_<previous_event_id_or_genesis_for_sequence_1>",
  "created_at_unix_ms": 1776890000123,
  "event_type": "proposal_authorized",
  "withdrawal": {
    "as_of": "<base-batch-hash-hex>",
    "base_event_id": "<base-event-id-hex>",
    "withdrawal_nonce": 12,
    "epoch": 3
  },
  "base": {
    "base_batch_end": 40519900,
    "turn_started_base_height": 40520000,
    "last_submit_attempt_base_height": null
  },
  "nockchain": {
    "snapshot_height": 12345,
    "snapshot_block_id": "<tip5-hex-or-null>",
    "safe_tip_height_observed_by_writer": 12245
  },
  "proposal": {
    "proposal_hash": "<proposal-hash>",
    "transaction_name": "<base58-raw-tx-id-or-null>",
    "transaction_jam": "<hex-or-null>",
    "selected_inputs": [
      {
        "first": "<40-byte-hex>",
        "last": "<40-byte-hex>"
      }
    ],
    "commit_certificate": "<hex-or-null>",
    "signer_node_id": null
  },
  "submission": {
    "submitted_raw_tx_id": "<base58-raw-tx-id-or-null>",
    "authorized_raw_tx": "<hex-or-null>",
    "submit_attempt_count": 0,
    "last_submit_error": null
  },
  "confirmation": {
    "included_height": null,
    "included_block_id": null,
    "confirmed_height": null,
    "confirmed_block_id": null
  }
}
```

### Required Remote Journal Event Types

1. `withdrawal_ordered`
   - Records that the sequencer accepted the canonical `(withdrawal_id,
     withdrawal_nonce)` ordering pair.
2. `proposal_canonicalized`
   - Records canonical proposal hash, commit certificate if present, selected
     inputs, and snapshot metadata.
3. `proposal_authorized`
   - Records the sequencer-authorized fully signed proposal, structured
     transaction, submitted raw tx id, and exact authorized raw tx bytes.
4. `tx_submitted`
   - Records that the sequencer attempted to submit the authorized raw tx.
5. `tx_seen_mempool_accepted`
   - Records node/mempool acceptance or accepted observation.
6. `mempool_retry_attempted`
    - Records retry metadata for an existing mempool-accepted row without
      changing lifecycle state.
7. `tx_confirmed`
    - Records confirmed inclusion and confirmation height/block id. Replay uses
      this event to clear live reservations in the local projection.

### Local Debug-Only Events

1. `proposal_signed`, `precanonical_handoff`, and `proposer_turn_expired` remain
   valid local `withdrawal_submission_events` rows.
2. These events are intentionally omitted from the remote journal in the
   minimal exact-resume event set unless later recovery work proves they are
   required.
3. Replay may conservatively reset proposer-turn timers and handoff indexes
   from the durable recovery-critical events instead of requiring exact debug
   event replay.

### Raw Tx Recovery

1. `authorized_raw_tx` is recovery-critical. It must be journaled for every
   authorized or submitted withdrawal before submission.
2. `authorized_transaction_jam` remains useful typed state for debugging and
   future typed recovery workflows, but it is not the retry authority.
3. Nockchain block data can prove inclusion or confirmation for txs that made
   it into blocks. It cannot recover an authorized raw tx that was never
   included.
4. If an in-flight unconfirmed withdrawal lacks `authorized_raw_tx` in both
   SQLite and the remote journal, recovery fails closed unless the transaction
   is already confirmed.
5. Replay must validate that the journaled submitted raw tx id matches the raw
   tx bytes. A mismatch is a fail-closed recovery error.

### Base Activity Index

1. Base logs are canonical for withdrawal burn facts and withdrawal ordering.
2. The Base activity index is a remote journaled optimization for fast recovery,
   not the source of truth.
3. Recovery verifies indexed Base activity against Base RPC and then scans a
   recent tail from `last_indexed_block - safety_overlap` to the current
   confirmed Base tip.
4. Current Rust withdrawal ordering is `base_batch_end` then `base_event_id`.
   This is not raw EVM log order. `base_event_id` is computed as
   `keccak256(tx_hash || log_index)`. This spec keeps the existing ordering and
   names it explicitly as `base_batch_end_then_base_event_id`.
5. If the protocol later wants true Base log ordering, that is a separate
   ordering migration.

### Base Activity Record Schema

```json
{
  "schema_version": 1,
  "record_type": "base_withdrawal_burn",
  "record_id": "b3_<canonical-record-hash>",
  "indexed_at_unix_ms": 1776890000123,
  "base": {
    "chain_id": 84532,
    "nock_token_address": "0x...",
    "block_number": 40520376,
    "block_hash": "0x...",
    "parent_hash": "0x...",
    "tx_hash": "0x...",
    "tx_index": 12,
    "log_index": 3,
    "event_signature": "BurnForWithdrawal(address,uint256,bytes32)"
  },
  "burn": {
    "base_event_id": "0x...",
    "burner": "0x...",
    "recipient_lock_root": "<tip5-hex-or-b58>",
    "gross_burned_amount_dec": "1064937081909179687500"
  },
  "bridge_batch": {
    "base_blocks_hash": "<base-batch-hash>",
    "base_batch_first_height": 40520300,
    "base_batch_last_height": 40520399,
    "base_batch_end": 40520399
  },
  "ordering": {
    "ordering_scheme": "base_batch_end_then_base_event_id",
    "withdrawal_nonce": 12
  },
  "verification": {
    "topics_hash": "0x...",
    "data_hash": "0x..."
  }
}
```

### Base Reconciliation

1. Fetch the confirmed Base tip using the configured Base confirmation depth.
2. Load Base activity index records from R2 / S3-compatible object storage.
3. Compute the required Base catch-up height from the remote journal and Base
   activity index:
   - maximum journaled `base_batch_end`
   - maximum indexed Base block number
   - maximum journaled `turn_started_base_height`
   - maximum journaled `last_submit_attempt_base_height`
4. If the local confirmed Base tip is behind that required height, recovery
   must wait for Base RPC catch-up before serving sequencer RPCs, authorization,
   submission, retry, or handoff work.
5. While local Base is behind the remote journal / index cursor, recovery must
   not reject journaled sequencer withdrawals as missing Base burns and must not
   fire Base-height retry or handoff timers from the stale local view.
6. If the local confirmed Base tip is ahead of the remote journal / index
   cursor, no special state machine is required. Recovery verifies indexed
   burns, tail-scans from the overlap window to the confirmed tip, and recovers
   newly discovered burns as `Pending` / unsequenced work.
7. Verify every indexed record against Base RPC:
   - block number and block hash
   - tx hash and log index
   - event signature and contract address
   - burner
   - gross burned amount
   - recipient lock root
   - computed `base_event_id`
8. Tail scan from `last_indexed_block - safety_overlap` through the current
   confirmed Base tip to find recent burns that are not yet indexed.
9. Build the canonical withdrawal burn set keyed by `(as_of, base_event_id)`.
10. Recompute expected withdrawal nonces using
   `base_batch_end_then_base_event_id`.
11. Reconcile every journaled sequencer withdrawal against the canonical Base
   burn set:
   - no matching Base burn: fail recovery
   - amount mismatch: fail recovery
   - destination lock-root mismatch: fail recovery
   - nonce / ordering mismatch: fail recovery
12. If a canonical Base burn exists with no sequencer state, recover it as
   `Pending` / unsequenced work.
13. The Base activity index may accelerate recovery, but a bad index must never
   override confirmed Base RPC data.

### Nockchain Reconciliation

1. Nockchain reconciliation answers:
   - whether in-flight txs are confirmed, included-but-not-deep-enough,
     mempool-accepted, or missing
   - which bridge-owned notes are safe to spend for new proposals
   - whether any journaled / projected state is impossible relative to observed
     Nockchain state
2. Inputs:
   - sequencer projection rebuilt from the remote journal and Base
     reconciliation
   - `authorized_transaction_name` / submitted raw tx id
   - `authorized_raw_tx` for every unconfirmed in-flight withdrawal
   - public Nockchain tx-included-block lookup
   - public Nockchain current tip height
   - current bridge note balance snapshot
3. The safe Nockchain tip is:

```text
safe_nock_tip = max(0, current_nock_tip - nockchain_confirmation_depth)
```

4. The balance API may continue returning the freshest/current snapshot. Safety
   filtering belongs in the bridge snapshot service and proposal validation
   path.
5. Proposal construction must use only notes with
   `origin_height <= safe_nock_tip`, after subtracting live reserved inputs.
6. Incoming peer proposals must be accepted only when every selected input is
   safe against the validator's current safe tip. If an input is currently
   unsafe, reject or defer that proposal artifact without advancing lifecycle
   state and without treating the withdrawal as terminal.
7. If the local public Nockchain API tip is behind the maximum journaled
   Nockchain snapshot / inclusion / confirmation height, startup must wait for
   catch-up before serving withdrawal assembly or submission. Remaining behind
   for too long risks spending down older safe liquidity while newer balances
   are invisible.
8. While local Nockchain is behind the remote journal cursor, recovery must not
   reject journaled selected inputs as unsafe or treat journaled txs as missing.
   Those checks are meaningful only after the local API has caught up to the
   journaled Nockchain view.
9. If local Nockchain is ahead of the remote journal cursor, no special state
   machine is required. Recovery uses the current tip to confirm sufficiently
   buried in-flight txs, refreshes the current balance snapshot, filters by
   safe origin, and continues normal reconciliation.
10. Deposits are less concerning because they have a long delay before use.
   Refund or change notes produced by recent withdrawals are the main unsafe
   liquidity risk: they must not be selected until their origin is buried by
   the confirmation window.
11. If there are not enough safe-origin notes for the next pending withdrawal,
   leave the withdrawal pending and retry later.
12. The "latch" for unsafe-origin inputs is Nockchain tip advancement, not a
    durable rejection state. A proposal that is unsafe at height `T` may become
    valid at height `T + nockchain_confirmation_depth` if all other proposal
    checks still pass. Local assembly idles and releases its assembly lock when
    only unsafe-origin liquidity is available; later assembly ticks can select
    the same now-safe notes after the snapshot/tip catches up.
13. `safe_tip_height_observed_by_writer` is audit metadata. Validation must use
    the receiving validator's current safe tip, so a peer is not forced to
    permanently reject a proposal solely because the proposer first observed
    the input before it was buried.

### Nockchain Startup Reconciliation Algorithm

1. Fetch current Nockchain tip and compute `safe_nock_tip`.
2. For each `Authorized` row:
   - require `authorized_raw_tx` if the withdrawal has ever been submitted or
     may need retry
   - query tx inclusion by submitted raw tx id if one exists
   - if included and depth is satisfied, mark `Confirmed`
   - if included but not deep enough, leave in-flight
   - if not included, leave `Authorized` for bounded retry logic
3. For each `MempoolAccepted` row:
   - query tx inclusion by `authorized_transaction_name`
   - if included at height `H` and `current_tip - H >=
     nockchain_confirmation_depth`, mark `Confirmed`
   - if included but insufficient depth, leave `MempoolAccepted`
   - if not included, leave `MempoolAccepted`; orphan retry can resubmit using
     `authorized_raw_tx`
4. For each `Confirmed` row:
   - if the tx is still observable as confirmed, recovery continues
   - if the tx is not visible, fail closed or alert by default; do not silently
     roll back confirmed state without an explicit deep-reorg repair mode
5. Refresh current balance snapshot.
6. Filter bridge-owned notes to safe-origin notes only.
7. Verify each active reserved input is either:
   - present and safe in the filtered balance snapshot
   - or already consumed by a confirmed withdrawal
8. Active reserved input conflicts across withdrawals are fail-closed recovery
   errors.

### Startup Recovery Order

1. Open SQLite and ensure schema.
2. Configure R2 / S3-compatible journal in fail-closed mode unless explicitly
   disabled for development.
3. Before serving sequencer RPCs or spawning confirmation / retry loops:
   - load the local `sequencer_journal_cursor`
   - if the local cursor is genesis, probe the first remote journal object
   - if the local cursor is non-genesis, fetch the exact remote object named by
     `(last_sequence, last_event_id)` and verify its event hash
   - fail closed if the cursor object is missing or unreadable, because local
     SQLite may be ahead of the durable log
   - treat missing old event objects as operator error until checkpoint-based
     journal pruning is implemented
   - verify the cursor event is exactly reflected in SQLite before replay starts
   - replay each `first_after` successor in order until the remote tail is
     reached
   - fail closed on sequence gaps, event hash mismatches, `previous_event_id`
     mismatches, projection/cursor mismatches, or projection state that is
     already past the event being replayed
   - replay rebuilds `sequencer_withdrawals` and `withdrawal_reserved_inputs`;
     local `withdrawal_submission_events` remain diagnostic history rather than
     the remote recovery source of truth
4. Reconcile Base:
   - verify the Base activity index against Base RPC
   - tail scan recent confirmed Base blocks
   - recover burns with no sequencer state as pending
   - fail on mismatches against journaled sequencer withdrawals
5. Reconcile Nockchain:
   - wait for local public API catch-up if needed
   - confirm sufficiently buried in-flight txs
   - preserve unconfirmed in-flight rows with exact raw tx retry artifacts
   - rebuild safe liquidity view from current snapshot filtered by safe origin
6. Only after all recovery phases pass should the sequencer serve RPC,
   confirmation polling, orphan retry, or new authorization/submission work.

### Kernel Projection Cursor

1. SQLite tables derived from bridge kernel Base/Nock hashchains are
   projections, not independent sources of truth.
2. Each kernel-derived projection database stores a `kernel_projection_cursor`
   row for `bridge_kernel` with Base/Nock next heights and optional tip hashes.
3. Missing cursor plus existing kernel-derived rows is fail-closed.
4. Missing withdrawal cursor plus empty rows waits for the configured
   `withdrawal_activation_nock_next_height` cutoff. Once the Nock frontier
   reaches that cutoff, the withdrawal projection initializes at the current
   full Base/Nock kernel position. Earlier Base withdrawal burns are ignored.
5. A cursor ahead of the observed kernel Base/Nock position is fail-closed.
6. Future replay phases must advance the cursor in the same SQLite transaction
   as the projection writes covered by that cursor.

### Updated Implementation Plan

1. Terminology and configuration:
   - rename operator-facing config/docs from S3-specific language to
     R2 / S3-compatible object-store language
   - keep R2 as the primary documented target
   - keep compatibility aliases only where useful for rollout
   - keep `bridge-dev` explicitly disabled unless local object-store settings
     are configured
2. Journal completeness:
   - extend journal records with `sequence`, `event_id`,
     `previous_event_id`, `created_at_unix_ms`, deployment-bound
     `journal_id`, and event context blocks
   - add missing event types for withdrawal ordering and mempool retry metadata
   - journal `withdrawal_nonce`, handoff fields, submit-attempt metadata,
     selected input origins, `transaction_jam`, submitted raw tx id, and
     `authorized_raw_tx`
3. R2 / object-store read path:
   - extend the journal abstraction with append, list, and get
   - use `aws-sdk-s3` for S3-compatible `ListObjectsV2` pagination and `GET`
   - validate decoded records before mutating SQLite
4. Replay projector:
   - apply journal events to projection state without writing new remote events
   - keep replay exact-frontier: a cursor event must already be projected, and
     successors are applied one at a time
   - reuse the local recovery cursor: last sequence and last event id
   - fail if local SQLite projection is non-empty with no usable cursor, if the
     cursor object is absent from the remote stream, or if the projection is
     detectably ahead of the event being replayed
5. Base reconciliation:
   - add `BaseActivityIndex` / `BaseActivityStore`
   - add local verified Base burn projection, e.g. `sequencer_base_burns`
   - verify indexed records against Base RPC
   - scan the recent overlap tail and insert missing burns
   - recover unmatched burns as pending withdrawals
6. Nockchain reconciliation:
   - add startup reconciliation over `Authorized`, `MempoolAccepted`, and
     `Confirmed` rows
   - require raw tx bytes for unconfirmed in-flight withdrawals
   - use tx-included-block lookup plus current tip to mark confirmations
   - reject or defer currently unsafe-origin selected inputs in proposal
     validation without advancing lifecycle state
   - keep assembly pending when safe-origin liquidity is insufficient
7. Tests:
   - DB wipe rebuilds sequencer state from remote journal
   - partial DB catches up from journal cursor
   - remote append succeeds but local SQLite mutation fails; reboot applies the
     remote event
   - replay preserves `authorized_raw_tx` byte-for-byte
   - replay restores and clears reserved inputs correctly
   - replay does not re-append remote objects
   - replay rejects projection/cursor skew instead of skipping over later local
     lifecycle state
   - malformed, missing, duplicated, gapped, or hash-discontinuous remote
     records fail safely
   - Base activity index verifies exact logs and tail scan finds missing recent
     burns
   - Base burn with no sequencer state recovers as pending
   - journaled sequencer withdrawal with mismatched Base amount, destination, or
     ordering fails recovery
   - proposal construction excludes notes with `origin_height > safe_nock_tip`
   - refund / change note is ignored before confirmation depth and accepted
     after confirmation depth
   - incoming proposal with an input that is unsafe at validation height is not
     persisted, but the same otherwise-valid proposal can be accepted after the
     local safe tip advances past that input origin
   - insufficient safe notes leaves withdrawal pending and retries later
   - incoming proposal using unsafe/recent note is rejected
   - missing authorized raw tx for unconfirmed in-flight withdrawal fails safe
   - journal write failure prevents mutation/submission in production mode

## Design Commitments and Remaining Hardening

### A) Kernel behavior and remaining hardening (Hoon)

1. Keep withdrawal proposal handling out of the kernel.
   - do not introduce a kernel withdrawal proposal-validation cause
   - do not persist `withdrawal-proposals` in kernel state
   - do not validate proposal identity, epoch legality, replay, or
     equivocation in the kernel
   - do not validate proposal envelope fields against tracked withdrawal
     metadata in the kernel
   - the kernel should remain withdrawal-level only: qualifying burns create
     live/unconfirmed withdrawals, and confirmed settlements clear them
2. Harden Base-side proposal generation in `++ base-propose-withdrawals` (already emitting `nock-withdrawal-request`) with queue semantics suitable for single-flight assembly/canonicalization.
3. Harden withdrawal parsing path in `++ process-nock-txs` (already creating `withdrawal-settlement`) and close remaining schema/validation gaps.
4. Finalize `++ nockchain-process-withdrawal-settlements` behavior in `open/hoon/apps/bridge/nock.hoon`:
   - reconcile settlement against tracked unsettled withdrawals
   - apply hold/stop semantics for out-of-order or inconsistent settlement
   - tolerate sequencer retries of the same authorized tx without treating them as a second withdrawal
5. The dedicated `create-withdrawal-tx` poke/cause is the bridge-side tx
   builder seam.
   - it includes `epoch` and pinned
     `withdrawal-snapshot` in addition to explicit withdrawal metadata plus
     selected inputs
   - it lets Rust ask the bridge app tx-builder to build one full
     `withdrawal-proposal`, not just a raw tx
   - successful construction comes back out via `%withdrawal-proposal-built`,
     carrying that full proposal
   - it remains separate from `%base-block-withdrawals-pending`, which stages
     Base burn withdrawal intent until Rust persists and acks it
6. `%sign-tx` is the dedicated poke/cause for transaction signing.
   - input should be the full `withdrawal-proposal`
   - Rust uses it after proposal construction / authorization, not as part of
     kernel withdrawal-state transitions
   - signing remains distinct from tx construction and from confirmed terminal
     effects
7. Kernel settlement reconciliation is currently state-only.
   - on confirmed settlement reconciliation from the confirmed Nock block
     stream, kernel clears the unsettled withdrawal directly
   - there is no dedicated `%withdrawal-terminal` effect today
   - if a future terminal effect is added, it should identify the confirmed
     withdrawal and settlement outcome only; it must not include local note
     names
8. Normalize and finalize withdrawal metadata encoding:
   - new packed key path: `%bridge-w`
   - includes `base-block-hash`, `beid`, `base-batch-end`, `lock-root`
9. Finalize amount semantics in kernel and Rust models now that dedicated
   withdrawal fee fields were removed from molds.
   - gross burned amount remains the source-of-truth input from Base
   - net disbursed amount is the amount actually paid to the Nockchain
     recipient
   - kernel settlement reconciliation enforces only
     `0 < settled_amount < gross_burned_amount`
  - exact economic correctness
    (`gross_burned_amount = withdrawal_fee + final_fee + net_disbursed_amount`)
    belongs in the Rust proposal validation path owned by
    `WithdrawalProposalRegistry`
   - do not overload one public planner request type to mean both "gross burn"
     and "net gift"

### B) Rust bridge crate shape and remaining hardening

1. Withdrawal proposal transport and validation live in Rust:
   - ingress RPC and message types exist for withdrawal proposal, canonical
     proposal, and signed-proposal broadcast
   - proposal envelope must include withdrawal id, epoch, pinned snapshot
     metadata, selected input note names, the built `transaction`, net
     disbursed amount, and gross burned amount
   - decode and route directly to Rust withdrawal coordination logic rather
     than via a kernel cause
   - validate the proposal envelope in Rust by checking:
     - whether the withdrawal exists
     - whether the proposal's `burned_amount`, recipient, and batch metadata
       match the tracked withdrawal
     - same-epoch replay vs equivocation
     - contiguous epoch legality
     - future exact amount validation before persistence/canonicalization by
       independently checking
       `burned_amount = withdrawal_fee + final_fee + amount`
   - also validate pinned snapshot, selected notes, and transaction identity
     as part of the Rust-owned proposal envelope checks
2. The split withdrawal tx-builder seam is live:
   - Rust selects candidate inputs and pinned snapshot metadata, then pokes the
     widened `%create-withdrawal-tx`
   - decode `%withdrawal-proposal-built`, carrying the constructed full
     `withdrawal-proposal`
   - keep this builder seam implementable before durable submission is enabled
   - make the withdrawal builder use a withdrawal-specific planning API that
     takes gross burned amount as input and computes net disbursed amount plus
     fee before calling `%create-withdrawal-tx`
   - keep the existing generic `plan_create_tx` path as the net-gift planner;
     share the lower-level selection/fee/refund engine rather than forcing one
     top-level amount field to serve both meanings
3. The signing seam is live:
   - Rust pokes `%sign-tx` with the full `withdrawal-proposal`
   - successful signing should come back out via `%withdrawal-tx-signed`,
     carrying the proposal envelope with signed transaction data
   - signing is driven from Rust-side coordination and remains separate from
     kernel withdrawal-state transitions
4. The withdrawal sequencing service is live:
   - implement the durable sequencing service core and schema first
   - append-only local log of proposal/canonicalization/submission/confirmation lifecycle
   - authoritative tracking in the withdrawal state store of peer-canonical / authorized /
     submitted withdrawals
   - host the sequencer gRPC surface on the Nockchain API node process rather
     than in a separate bridge-side binary
   - have that API-node-hosted sequencer poll the colocated public Nockchain
     API for confirmation and clear the in-flight set there
   - reserve input notes by note `Name` at peer canonicalization and keep them
     reserved through authorization / submission until confirmation
   - drive local tx-status reconciliation using `tx-accepted` as a diagnostic
     mempool/accepted signal and confirmed settlement observations as the
     release signal
   - confirmation and reservation release are sequencer-owned; kernel
     settlement reconciliation is parallel and does not currently arrive as a
     dedicated Rust terminal effect
   - make the sequencer the only submission authority for withdrawals
   - make bridge nodes ask the sequencer for `authorized` / `submitted` /
     `confirmed` truth instead of reconstructing that lifecycle locally
5. The withdrawal execution driver is live:
   - consume `BridgeEffectVariant::BaseBlockWithdrawalsPending`
   - allow at most one withdrawal in assembly/canonicalization at a time for the bridge-controlled spend authority / note pool
   - make the sequencer gRPC service the only component that finalizes and submits an authorized raw tx using nockapp public nockchain gRPC client (`wallet_send_transaction`)
   - include separate assembly timeout and sequencer submission timeout behavior
   - if the sequencer is unavailable, stop withdrawal progress rather than failing over to peer submission
6. Add proposal broadcast/signature workflow keyed off per-withdrawal epochs rather than deposit ids or withdrawal batches, with sequencer authorization required before a peer-canonical candidate becomes submit-ready.
   - built proposals must be broadcast from the live `%withdrawal-proposal-built`
     execution path
   - canonicalization gossip must carry a threshold commit certificate
   - authorization must reject peer-canonical proposals that lack that
     certificate
7. Runtime/main wiring now registers the withdrawal execution driver, ingress
   transport, and withdrawal runtime loops in `open/crates/bridge/src/main.rs`.
8. Bridge note snapshot handling should continue to:
   - use a normalized bridge-note balance snapshot for private-node sync as well as public sync
   - refresh the confirmed bridge-note snapshot whenever a newly confirmed nockchain block is observed
   - support on-demand refresh before assembly when the cached snapshot is stale
   - compute `spendable_notes = confirmed_snapshot(origin_page <= safe_nockchain_tip) - reserved_inputs`
   - prevent reuse of notes that belong to peer-canonical / authorized /
     mempool-accepted but unconfirmed withdrawal txs
9. Durable withdrawal storage must continue to provide:
   - an append-only withdrawal lifecycle log
   - the live reserved-note set and current withdrawal state-store projection
   - startup rebuild of live mutable state from the append-only log
   - transactional reserved-note writes alongside the corresponding canonical /
     confirmed lifecycle event append
10. Optional but recommended:
   - enrich `process_nock_log` output in `open/crates/bridge/src/ethereum.rs` to propagate destination/lock-root mapping into runtime observability structures, or explicitly document that kernel state is canonical and Rust-side `dest` remains advisory-only.

### C) Type/protocol shape

1. `open/crates/bridge/proto/bridge_ingress.proto`
   - includes withdrawal proposal request/response
   - proposal payload is a full withdrawal proposal envelope, not just a bare
     transaction body
   - includes withdrawal id, epoch, proposal hash / transaction name, pinned
     snapshot metadata, selected input note names, and typed payload
   - includes sequencer RPCs for register / handoff / canonical / signed /
     reserved-inputs / authorize / submit / status / confirmed-record updates
2. `open/crates/bridge/src/shared/types.rs`
   - keep `NockWithdrawalRequestKernelData` as the kernel-emitted withdrawal intent payload
   - include `%withdrawal-proposal-built`, carrying a full `withdrawal-proposal`
   - include `%withdrawal-tx-signed`, carrying a full signed `withdrawal-proposal`
   - include a separate withdrawal proposal envelope type for peer coordination
   - there is no terminal withdrawal effect variant today
   - ensure serialized field order matches Hoon `nock-withdrawal-request` (`base_event_id`, `recipient`, `amount`, `base_batch_end`, `as_of`)
3. `open/hoon/apps/bridge/types.hoon`
   - keep withdrawal molds aligned with new shapes (`nock-lock-root`, `base-batch-end`, no dedicated withdrawal fee field)
   - widen `%create-withdrawal-tx` so bridge-side tx building includes
     `epoch` and pinned `withdrawal-snapshot`, not just withdrawal metadata and
     selected inputs
   - add a `%withdrawal-proposal-built` effect mold carrying a full
     `withdrawal-proposal`
   - add a `%withdrawal-tx-signed` effect mold carrying a full signed
     `withdrawal-proposal`
   - there is no `%withdrawal-terminal` effect mold today
   - do not add a withdrawal proposal-validation cause to the kernel molds

## Invariants and Validation Rules

1. Withdrawal identity must be deterministic and replay-safe:
   - key by counterpart `(as_of, base_event_id)` with canonical encoding
   - this identity is the coordination key for proposal epochs as well as settlement reconciliation
2. Canonicalization identity must be deterministic:
   - proposal hash is computed over the exact proposal envelope / exact typed `transaction`
   - transaction identity inside that envelope comes from `transaction.name`, not a separate `tx_id`
   - honest peers must commit to at most one proposal hash per `(withdrawal_id, epoch)`
3. Settlement must match counterpart withdrawal on:
   - destination lock root / recipient
   - the basic bound `0 < settled_amount < burned_amount`
   - exact gross/net/fee correctness in Rust proposal validation rather than
     kernel settlement reconciliation
4. Authoritative protocol facts are chain-visible, not submit-attempt-visible:
   - peer-canonicalization is not sufficient for submission
   - sequencer authorization is required before a withdrawal may be submitted
   - `tx-accepted` is a diagnostic signal that the tx reached node-accepted /
     mempool-visible state, not that it was included in a block
   - confirmed settlement in a block, observed through sequencer confirmation
     polling, is the confirmation signal for inclusion and reservation release
   - kernel counterpart reconciliation happens independently on the confirmed
     Nock block stream; there is no dedicated terminal withdrawal effect on
     the current Rust surface
   - local "submitted" events never make a tx canonical
   - the sequencer owns the authoritative submitted / in-flight withdrawal set
   - the sequencer durably records confirmation for authorized withdrawals
   - bridge nodes ask the sequencer for submit/confirm truth before retrying,
     re-authorizing, or re-submitting local attempts
5. Input notes reserved by a canonical or in-flight tx must not be reused:
   - reserve by note `Name`
   - planner spendable set is `safe-origin confirmed normalized snapshot - sequencer-reported reserved inputs`
   - reservations begin at peer canonicalization, not at local assembly
   - reservations are released only after the sequencer records confirmed
     settlement for the authorized withdrawal
6. There is no automatic submission failover:
   - after sequencer authorization, only the sequencer may submit or retry the same tx
   - if the sequencer is unavailable, withdrawals pause
7. Unknown counterpart policy:
   - if `as_of` base hash is unknown: set hold and wait for counterpart chain progress
   - if `as_of` is known but counterpart event/state is missing: stop
8. Any irreconcilable mismatch is a stop condition.
9. No silent divergence:
   - kernel withdrawal-state failures remain explicitly one of ignore, hold, or stop
   - Rust proposal-validation failures must be durably recorded and surfaced,
     not hidden as kernel state transitions
10. At most one withdrawal may hold the assembly/canonicalization lock at a time for the bridge-controlled spend authority / note pool.
11. At most one withdrawal may be sequencer-authorized / submitted / unconfirmed at a time.

## Testing Plan

### Kernel tests

1. Burn event enters `unsettled-withdrawals`.
2. Valid fee-bearing settlement with `0 < settled_amount < burned_amount`
   clears the corresponding unsettled entry.
3. Settlement-before-counterpart sets hold and later resolves.
4. Settlement with known `as_of` but missing counterpart triggers stop.
5. Settlement with missing unsettled withdrawal triggers stop.
6. Settlement with destination mismatch triggers stop.
7. Settlement with `settled_amount <= 0` or `settled_amount >= burned_amount`
   triggers stop.
8. Duplicate/replay settlement does not corrupt state.

### Rust tests

1. Ingress decodes withdrawal proposal and hands it directly to Rust
   withdrawal coordination logic.
2. Rust proposal validation rejects unknown withdrawals, mismatched withdrawal
   metadata, illegal epochs, replay with mismatched envelope, same-epoch
   equivocation, and future economically inconsistent fee decomposition.
3. Rust proposal broadcast driver emits full withdrawal proposal envelopes to peers correctly.
4. Canonicalization is reached only when threshold peers persist and commit to
   the same proposal hash, and the resulting canonicalized broadcast carries a
   valid threshold commit certificate.
5. Nock tx submission driver handles:
   - success ack
   - retryable errors
   - non-retryable errors
6. Only the sequencer may authorize and submit a peer-canonical withdrawal candidate.
7. Input notes used by peer-canonical / authorized / mempool-accepted but
   unconfirmed withdrawal txs are filtered from later planning snapshots.
8. Canonical reservations are released when the sequencer records confirmed
   settlement for the authorized withdrawal, not via a withdrawal-level
   abandon/fail outcome.
9. Sequencer confirmation tracking records confirmed withdrawals and clears the authoritative in-flight set without double-recording the same withdrawal.
10. Sequencer restart preserves the authoritative in-flight withdrawal set and confirmed-withdrawal record, and does not re-authorize the same withdrawal twice.
11. Sequencer loss pauses withdrawal progress rather than failing over submission to peers.
12. End-to-end wiring test:
   - kernel emits `%base-block-withdrawals-pending`
   - peers converge on one canonical candidate
   - sequencer authorizes and submits that candidate
   - sequencer records confirmation for that authorized withdrawal
   - kernel later reconciles the confirmed settlement in its own state while
     the sequencer confirms that the matching reserved inputs are no longer
     live
   - no panic/regression in existing deposit path

### Multi-node integration

1. 5-node single-withdrawal proposal and threshold signature convergence with sequencer authorization.
2. Scheduled assembler offline before proposal timeout:
   - if the withdrawal is only `Assembling`, the next handoff owner retries the same epoch
   - if the withdrawal is already `Prepared`, the next epoch leader assembles a replacement candidate tx
3. After peer canonicalization, only the sequencer authorizes and submits the candidate tx.
4. Sequencer restart mid-flight preserves reservations, the in-flight withdrawal set, and the confirmed-withdrawal record, then resumes safely.
5. Sequencer unavailability pauses withdrawal progress rather than failing over submission to peers.
6. Conflicting later proposal for an already authorized withdrawal is treated as an invariant violation / stop.
7. Simulated out-of-order Base/Nock arrival with hold release.

## Remaining Policy Questions

1. Fee model:
   - exact formula for withdrawal fee deduction and where it is applied
2. Hold metadata requirements:
   - whether base block height must be embedded in tx metadata for deterministic unblock behavior
3. Whether single-flight policy should remain permanent, or only the current intended design

## Remaining Hardening Themes

1. Kernel hardening
   - keep kernel settlement reconciliation scoped to identity plus basic amount
     bounds while moving exact fee validation into Rust proposal acceptance
   - keep hold/stop behavior deterministic
2. Runtime and integration hardening
   - continue exercising multi-node proposal convergence, assembly failover,
     sequencer restart recovery, and reservation integrity
   - keep one-withdrawal-at-a-time assembly / submission behavior explicit
3. Production hardening
   - metrics, alerts, operational docs, replay abuse tests

## Acceptance Criteria

1. No `TODO`/hard-stop paths remain for withdrawal causes/effects in kernel.
2. Bridge nodes can process Base burns into canonicalized, submitted, and finalized per-withdrawal Nock settlements.
3. If the scheduled assembler is offline before canonicalization, timeout handling is unambiguous:
   - `Assembling` rotates same-epoch pre-canonical handoff
   - `Prepared` expires the built attempt and advances replacement assembly to the next epoch
4. If a tx becomes peer-canonical, only the sequencer may authorize and submit it.
5. Out-of-order chain arrival is handled via deterministic hold behavior.
6. Submitted-but-unconfirmed withdrawal inputs are not reused for later withdrawal planning.
7. Existing deposit pipeline remains unchanged and green.
8. Integration tests cover nominal, sequencer-stop, restart, and note-reservation scenarios.
