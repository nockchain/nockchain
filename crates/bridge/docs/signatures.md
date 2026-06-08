# Bridge Signature Format & Verification

Status: Active
Owner: Nockchain Maintainers
Last Reviewed: 2026-02-20
Canonical/Legacy: Legacy (bridge subsystem signing reference; canonical docs spine starts at [`START_HERE.md`](../../../START_HERE.md))

This guide describes how proposal hashes and Ethereum signatures are computed,
validated, gossiped, and submitted in the bridge runtime.

## Data Model

| Type                       | Location              | Purpose                                                                                                                        |
| -------------------------- | --------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `SignatureSet`             | `src/types.rs`        | Bundles `eth_signatures: Vec<ByteBuf>` and `nock_signatures: Vec<ByteBuf>` for submissions to Base or nockchain.               |
| `EthSignatureParts`        | `src/types.rs` | Canonical `(r, s, v)` with validation helpers and noun encoding so the kernel can reason about signatures.                     |
| `BaseCallSigData`          | `src/types.rs`     | Legacy cause payload that couples an `EthSignatureParts` with signed calldata for peer verification paths. |
| `NockDepositRequestData`  | `src/types.rs` | Structured signature request: `[tx-id, name, recipient, amount, block-height, as-of]` matching Hoon `nock-deposit-request`. Here `name` is a full `nockchain_types::v1::Name` (two Tip5 hashes: first & last). |

Signature validation and proposal hashing are driven from these shared types,
with `%commit-nock-deposits` as the kernel effect that feeds the runtime
signing pipeline.

## Message Hash Construction

`MessageInbox.submitDeposit` verifies signatures over:

```solidity
keccak256(abi.encodePacked(
    _encodeTip5(txId),      // 40 bytes
    _encodeTip5(nameFirst), // 40 bytes
    _encodeTip5(nameLast),  // 40 bytes
    recipient,              // 20 bytes
    amount,                 // 32-byte uint256
    blockHeight,            // 32-byte uint256
    _encodeTip5(asOf),      // 40 bytes
    depositNonce            // 32-byte uint256
));
```

In Rust (`compute_proposal_hash` in `src/types.rs`), the payload is built as:

1. Tip5 limbs encoded as **big-endian** `u64` bytes (`to_be_bytes`), matching Solidity `abi.encodePacked(uint64, ...)`.
2. `amount` converted from nicks to NOCK base units (`NOCK_BASE_PER_NICK`) before encoding as 32-byte `U256`.
3. `block_height` and `nonce` encoded as 32-byte `U256`.

Total packed payload length is 276 bytes before keccak256.

## Signing Flow

1. Kernel emits `%commit-nock-deposits` with nonce-free requests.
2. Runtime assigns nonce deterministically and builds `NockDepositRequestData`.
3. Runtime computes proposal hash and signs with `EthereumSigner` (`src/signing.rs`).
4. `EthereumSigner` uses `alloy::signers::local::PrivateKeySigner::sign_message`, so signatures are EIP-191 prefixed.
5. Runtime gossips signatures and the proposal cache aggregates signatures until the threshold is reached, at
  which point the posting loop calls `submitDeposit` on Base.

## Ingress Wire Format

Bridge signature gossip is handled by `bridge.ingress.v1.BridgeIngress` (`proto/bridge_ingress.proto`):

- `BroadcastSignature(SignatureBroadcast)`:
  - `deposit_id`: 120 bytes (`DepositId`: `as_of || name.first || name.last`)
  - `proposal_hash`: 32 bytes
  - `signature`: 65 bytes (`r || s || v`)
  - `signer_address`: 20 bytes
- `GetProposalStatus(ProposalStatusRequest)` for cache state.
- `BroadcastConfirmation(ConfirmationBroadcast)` after successful Base posting.

`ProposalRequest` / `SignatureRequest` messages exist in the proto but are not the active ingress RPC path.

## Current Kernel Effect Surface

The kernel now emits only:

- `%base-block-withdrawals-pending pending=pending-base-block-withdrawals`
- `%commit-nock-deposits reqs=(list nock-deposit-request)`
- `%grpc grpc-effect`
- `%stop reason=cord last=stop-info`

Deposit signature flow in this document is tied to `%commit-nock-deposits`.
`%base-block-withdrawals-pending` carries withdrawal intent metadata for the
pending Base batch and is not part of the ETH multisig deposit-signature path.

## Serialization Format

### Hoon Types

The `nock-deposit-request` type in `hoon/apps/bridge/types.hoon`:
```hoon
+$  nock-deposit-request
  [tx-id=tx-id:t name=nname:t recipient=base-addr amount=@ block-height=@ as-of=nock-hash]
```

The `commit-nock-deposits` effect variant:
```hoon
[%commit-nock-deposits reqs=(list nock-deposit-request)]
```

### Rust Types

`NockDepositRequestData` in `src/types.rs` matches the Hoon structure:
- `tx_id: Tip5Hash` - Encoded as 5 uint64 limbs (40 bytes) via `Tip5Hash::to_array()`
- `name: Name` - Includes `first` and `last`, each encoded like `tx_id` (5 limbs => 40 bytes each, total 80 bytes)
- `recipient: EthAddress` - Exactly 20 bytes
- `amount: u64` - Amount in nocks
- `block_height: u64` - Block height
- `as_of: Tip5Hash` - Same encoding as tx_id/name

The `compute_proposal_hash` function in `src/types.rs` encodes each Tip5Hash
field as its 5 uint64 limbs packed in little-endian order, matching the Solidity
`_encodeTip5` function.

### gRPC Wire Format

When broadcasting via `ProposalRequest`:
- `kind`: `ProposalKind::Base`
- `payload`: Protobuf-encoded `EthSignatureRequest` message
- `proposer`: Node identifier string
- `request_id`: Unique request identifier

When responding via `SignatureRequest`:
- `proposal_hash`: 32-byte keccak256 hash (computed from eth_signature_request fields)
- `signature`: 65-byte ECDSA signature (r || s || v)
- `signer`: Node identifier string
- `request_id`: Matches the original proposal request_id
- `eth_signature_request`: Optional structured fields for validation

The runtime never trusts an inbound signature blindly: `EthSignatureParts`
exposes `validate()` so we can reject zeroed limbs or malformed `v` before the
kernel even sees the data.

## On-Chain Verification

`MessageInbox` enforces:

1. At least `THRESHOLD` (= 3) unique bridge-node signatures.
2. Canonical signature constraints (`s <= secp256k1_n/2`, `v` in `{27, 28}`).
3. Replay protection via `processedDeposits[txIdHash]`.
4. Monotonic ordering via `depositNonce > lastDepositNonce`.

A failed signer recovery or non-node signer causes the submission to revert.

## Operational Checklist

Before gossiping/submitting:

- Validate signature length (65 bytes) and signer-address length (20 bytes).
- Ensure proposal hash matches the in-cache request for the same `deposit_id`.
- Ensure signer address is in configured bridge-node set.
- Keep signatures raw (`r || s || v`) when building `bytes[] ethSigs` for contract submission.

## Testing Pointers

- Collect at least three validated signatures and keep their ordering stable.
- Build `DepositSubmission` from canonical proposal fields and the assigned nonce
  (runtime path), then call `BaseBridge::submit_deposit`.
- Attach the signatures as `bytes[] ethSigs` exactly as returned by peers (no
  ABI re-encoding is needed; each entry is the raw 65-byte concatenation of
  `r || s || v`).

## Testing

- `src/signing.rs` ships an async test that covers the happy path for
  `EthereumSigner::sign_proposal`.
- Contract-level tests in `contracts/test/MessageInboxDepositTest.t.sol` (add
  more as we expand coverage) should cover malleability, threshold edges, and
  replay protection.
- When adding new signing backends, integrate them into the `commit-nock-deposits`
  driver and reuse the existing hash computation and noun encoding helpers.

### Test Catalog
- `src/signing.rs`: signer unit tests.
- `src/ingress.rs`: signature broadcast validation and cache tests.
- `src/proposal_cache.rs`: threshold and signature-aggregation behavior.
- `contracts/test/MessageInboxDepositTest.t.sol`: contract-side signature checks and replay/threshold behavior.
