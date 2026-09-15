# Withdrawal DApp

Status: Normative for the public withdrawal API
Owner: Nockchain Maintainers
Last Reviewed: 2026-08-28
Canonical/Legacy: DApp-facing product and API contract derived from `bridge-withdrawals.md`

Source of truth: [`bridge-withdrawals.md`](bridge-withdrawals.md)

## Purpose

The withdrawal DApp burns wrapped NOCK on Base, requests settlement to a
Nockchain lock root, and displays authoritative progress until Nockchain
settlement is confirmed. The DApp owns request preparation, Base transaction
submission, local recovery, and presentation. The bridge runtime and sequencer
own proposal construction, authorization, Nockchain submission, reservation,
and settlement reconciliation.

## Launch Gate

Base-to-Nockchain withdrawal remains disabled for users. Enabling the frontend
flag alone is not a valid launch procedure. Launch requires all of the following
at one certified revision:

1. an immutable public Iris SDK release with the canonical codec and policy;
2. matching contract, sequencer, signer, protocol, policy, and SDK identities;
3. enabled operator admission and on-chain `MessageInbox.withdrawalsEnabled`;
4. fresh Base and Nockchain observations;
5. positive reservation-aware quotes;
6. receipt and burn-log verification;
7. durable browser persistence and authoritative history recovery;
8. production monitoring, compensation procedures, and recovery drills;
9. real-browser nominal, failure, reload, and reorg coverage.

The ordinary 68-byte generated-ABI burn path is unsupported. Official clients
submit only `WithdrawalWireV1`.

## Withdrawal Identity

A bridge withdrawal is keyed by:

```text
withdrawal_id = (as_of, base_event_id)
```

- `as_of` is the canonical Base block reference used by the bridge.
- `base_event_id` is the unique burn-log identity derived from transaction hash
  and log index.

The public UI uses `base_event_id` as soon as the Base receipt is verified. The
full `withdrawal_id` may remain absent until the backend has authoritative
kernel context. Display the Base transaction hash and timestamps alongside the
internal identifier.

## Canonical Burn Transaction

The DApp must not submit a withdrawal through a generated
`burn(uint256,bytes32)` contract helper. The five-limb Nockchain destination
requires the 116-byte `WithdrawalWireV1` input.

| Byte range | Size | Meaning |
|---|---:|---|
| `0..4` | 4 | `burn(uint256,bytes32)` selector |
| `4..36` | 32 | wrapped-NOCK amount, unsigned big-endian |
| `36..68` | 32 | withdrawal commitment passed as ABI `lockRoot` |
| `68..76` | 8 | ASCII `NOCKWD1!` |
| `76..116` | 40 | five unsigned 64-bit big-endian Tip5 limbs |

The commitment is:

```text
keccak256(
  "nock-withdrawal-calldata-v1"
  || nock_token_address[20]
  || burner_address[20]
  || amount_be[32]
  || lock_root_limbs_be[40]
)
```

The five limbs are the destination. A 32-byte digest is not a substitute. Each
limb must satisfy the Tip5 field bound.

Before opening the wallet, the DApp must:

1. resolve the configured Base deployment for the expected chain;
2. verify the connected account and chain after any wallet switch;
3. reject contract and delegated-account burners;
4. parse the amount without JavaScript floating-point conversion;
5. parse a direct lock root or derive one from a v1 PKH address;
6. obtain a current positive quote for the exact gross amount and destination;
7. construct the 116-byte input with the pinned Iris SDK;
8. decode the constructed bytes and compare every field with current state;
9. simulate and estimate gas using those exact bytes;
10. submit those same bytes through a chain-bound Base client.

Any account, chain, token, amount, destination, policy, or quote change
invalidates the prepared transaction.

## Amount Policy and Quote

`withdrawal-policy-v1` defines the amount contract:

- inclusive gross minimum: `100,000 NOCK`;
- exact conversion: `65,536` nicks per NOCK;
- Base token scale: `10^16` base units per NOCK;
- Base units per nick: `152,587,890,625`;
- bridge fee: `195` nicks per started NOCK;
- maximum nick amount: `u64::MAX`;
- net payout must remain positive after bridge and Nockchain transaction fees.

The frontend consumes these values from the pinned Iris SDK and requires the
backend readiness document to match them exactly. It does not maintain a second
policy table.

The quote endpoint uses the sequencer's current private Nockchain snapshot and
reservation state. A quote contains gross amount, bridge fee, transaction fee,
net payout, snapshot height and block, observation time, and revision. The DApp
blocks confirmation when the quote is unavailable, stale, for another amount,
or has a non-positive payout. The displayed payout remains an estimate until
settlement is confirmed.

## User Flow

1. Connect a Base wallet.
2. Enter the gross amount and Nockchain destination.
3. Validate deployment readiness, policy, destination, storage, and quote.
4. Review the exact gross burn, fees, estimated payout, destination, and Base
   network.
5. Sign one Base transaction containing the canonical 116-byte input.
6. Persist the submitted transaction identity before waiting for its receipt.
7. Verify the mined transaction and exactly one matching
   `BurnForWithdrawal` log.
8. Persist the Base block, log index, and `base_event_id`.
9. Poll authoritative history until the withdrawal is confirmed or requires
   support.

A wallet rejection creates no record. An unknown submitted transaction remains
tracked and must not be retried automatically.

## Public States

| State | Meaning |
|---|---|
| `draft` | Client-only form state; no Base transaction identity exists. |
| `awaiting_base` | A Base transaction identity exists, but no eligible canonical burn is authoritative yet. |
| `withdrawal_pending` | An eligible canonical burn is indexed; confirmed Nockchain settlement is not yet reconciled. |
| `confirmed` | Durable Nockchain settlement has been reconciled with the withdrawal. |
| `delayed` | A time/readiness overlay on pending state with support guidance. |
| `failure` | Authoritative facts report malformed input, below-policy admission, reorg invalidation, or inconsistency. |

Base confirmation, peer agreement, sequencer authorization, and mempool
acceptance do not produce `confirmed`.

A higher authoritative revision may move a record out of `confirmed` or
`failure` after a reorg or reconciliation. Clients must apply higher revisions
and reject lower revisions. A reorg regression clears the prior Nockchain
transaction, block, payout, and confirmation timestamp before presenting the
new state.

The public API does not expose proposal hashes, epochs, signers, selected
inputs, reservations, raw transactions, handoff indices, or internal hold/stop
state.

## UI Requirements

### Withdrawal form

Display and validate:

- connected Base account and network;
- gross wrapped-NOCK amount;
- direct lock root or v1 PKH destination;
- bridge fee, Nockchain transaction fee, and estimated payout;
- deployment readiness and the first blocking reason.

The confirmation action remains disabled until readiness, policy, storage,
quote, chain, account, and destination checks all pass.

### Base submission

Display the submitted and replacement transaction hashes, receipt status, Base
block, and verified burn-log identity. A submitted transaction with an unknown
receipt remains a support state. Do not recommend another burn.

### Withdrawal status

Display:

- Base transaction and `base_event_id`;
- gross amount and estimated or confirmed payout;
- destination;
- public state and authoritative revision;
- recovery reason and invalidated block when present;
- Nockchain transaction and block only for current terminal proof;
- support guidance for delayed, unavailable, or inconsistent states.

### History

Merge local records with the authoritative burner history. Local submitted
records remain visible during public API outages. The UI supports multiple
active withdrawals and never evicts a tracked record to make room for another
burn.

## Browser Persistence and Recovery

The browser stores at most 50 versioned withdrawal records per local store.
Before submission it verifies that storage can be read and written and that
capacity remains. Reaching capacity blocks a new burn instead of deleting an
existing record.

A record contains:

- account, chain, Nock token, and MessageInbox identity;
- submitted and current Base transaction hashes;
- amount, destination, commitment, and canonical calldata;
- receipt block, block hash, log index, and `base_event_id`;
- estimated and confirmed payouts;
- authoritative revision and recovery generation;
- invalidated block, prior status, and recovery reason;
- lifecycle history and timestamps.

Receipt replacement callbacks are serialized and persisted before receipt
processing continues. If persistence fails after submission, the DApp enters a
support state and preserves the known transaction identity.

On reload, account change, or network change, the DApp reloads records for the
exact account and deployment. Corrupt or unsupported data is never overwritten
silently. Confirmed and failed records continue polling because a higher
revision may supersede them.

## Public Query Service

The normative read-only service is
`bridge.ingress.v1.WithdrawalPublicQuery` in
`crates/bridge/proto/bridge_ingress.proto`.

| Method | Purpose |
|---|---|
| `ResolveBaseWithdrawal` | Resolve deployment, Base transaction, and optional log index. |
| `GetWithdrawal` | Lookup by Base locator, `base_event_id`, or full `withdrawal_id`. |
| `ListWithdrawalsByBurner` | Return bounded, stable history for a Base burner. |
| `GetWithdrawalReadiness` | Return deployment readiness and safe blocking reasons. |
| `GetWithdrawalQuote` | Return a reservation-aware payout quote. |

The public listener routes only `WithdrawalPublicQuery`. It does not expose
`WithdrawalSequencer`, `BridgeIngress`, reflection for private services, or a
generic proxy. Mutation RPCs remain on the authenticated private listener.

Every request is bound to Base chain ID, Nock token address, policy ID, and
protocol ID. Unknown or partial deployment identities fail closed. Log index
zero is valid. Omitting a log index for a transaction with multiple matching
logs returns `AMBIGUOUS_LOG`; the server does not guess.

History is ordered newest first by:

```text
(base_block_number, log_index, base_event_id)
```

The default page size is 20 and the maximum is 100. Opaque cursors bind the
deployment, burner, snapshot revision, and last sort key. Invalid,
cross-deployment, cross-burner, expired, or reorg-superseded cursors are
rejected.

## Browser HTTP Adapter

The optional production adapter binds `--withdrawal-public-http-addr` and
exposes `GET` and `OPTIONS` on `/withdrawal-status`:

| Query | Response |
|---|---|
| none | readiness schema 1 |
| `base_event_id=...` | status schema 2 |
| `history=1&account=...&limit=...` | burner history schema 1 |
| `quote=1&gross_amount_nicks=...&destination_lock_root=...` | quote schema 1 |

Configure the MessageInbox, Iris version, and request limit with the matching
`--withdrawal-public-http-*` arguments. Signer roster and threshold come from
the validated sequencer configuration and are not separately overrideable.

`WITHDRAWAL_PUBLIC_HTTP_ALLOWED_ORIGINS` is an exact origin allowlist.
Production origins require HTTPS; explicit loopback HTTP is accepted for local
verification. Wildcards, credentials, paths, fragments, and non-loopback plain
HTTP are rejected. Responses use `Cache-Control: no-store`; unknown origins
receive `403`; rate-limited clients receive `429` before bridge-state work.
Internal errors are logged server-side and redacted in public responses.

## Readiness Contract

Burn submission is enabled only when all of these facts match the pinned
frontend deployment:

1. Base chain ID and Nock token address;
2. MessageInbox address, `Nock.inbox()`, and `MessageInbox.nock()`;
3. signer PKH set and threshold;
4. `WithdrawalWireV1`, `withdrawal-policy-v1`, and Iris SDK version;
5. complete numeric policy values;
6. operator admission enabled;
7. on-chain withdrawal gate observed enabled;
8. fresh Base observation and current reconciliation frontier;
9. available Nockchain state for a positive quote.

The Base watcher clears the observed contract gate when the configured
MessageInbox differs from `Nock.inbox()`. Stale or unavailable observations
make readiness and quotes unavailable.

## Privacy and Abuse Controls

Burner history derives from public-chain facts but remains bounded by per-IP
request limits, page limits, response-size limits, and request deadlines.
Destination and account data must not be added to analytics or logs beyond the
identifiers required for operations. Public errors must not include private
endpoints, credentials, raw backend errors, node identities, IAM details,
reserved inputs, or transaction bytes.

## Operator-Only State

Operator tooling may display assembly, peer-canonical, authorized, submitted,
mempool-accepted, held, stopped, sequencer-unavailable, reservation, and
recovery state. The public DApp normalizes those facts into the public states
above.

## Acceptance Criteria

1. The frontend cannot initiate an ordinary 68-byte ABI burn.
2. One user confirmation produces at most one canonical Base burn.
3. Submitted and replacement transaction identities survive reload and errors.
4. Storage failure blocks submission or produces a support state without
   suggesting a duplicate burn.
5. Only a current positive quote enables confirmation.
6. Contract and delegated-account burners fail before submission.
7. Public deployment, signer, policy, protocol, SDK, and gate mismatches fail
   closed.
8. Authoritative higher revisions can regress local terminal state safely.
9. Confirmed UI state requires a current Nockchain transaction, block, payout,
   and terminal proof.
10. Local and authoritative history remain accessible across reload and public
    API outages.
11. Nominal, real-service failure, mock failure, and reorg lifecycle browser
    suites pass against the production adapter.
