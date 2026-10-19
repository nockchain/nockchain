# Transaction Validation Pipeline

## Overview

Transaction validation in Nockchain is a multi-phase process implemented primarily in Hoon (`hoon/common/tx-engine-0.hoon` and `hoon/common/tx-engine-1.hoon`), which is the authoritative source of truth for all validation logic. The Rust layer handles deserialization and networking but defers consensus validation to the Nock VM executing the Hoon kernel.

Validation proceeds through three phases:
1. **Structural validation** — well-formedness of the transaction itself
2. **Witness verification** — cryptographic proof checking
3. **Context-aware validation** — checking against the current chain state

## Phase 1: Structural Validation

Structural validation checks that the transaction is well-formed without reference to the UTXO set or chain state.

### V0 Transactions

The V0 `validate` arm in `tx-engine-0.hoon` checks:
- Transaction ID (`tx-id`) matches the hash of the transaction body
- Inputs are non-empty
- Fee amounts are non-negative
- Timelock ranges are internally consistent (min ≤ max)
- Total fees match the sum of per-input fees

### V1 Transactions

The V1 `validate` arm in `tx-engine-1.hoon` checks:
- Version tag is `%1`
- Transaction ID matches the hash of the spends
- Spends map is non-empty
- Each spend is internally consistent:
  - Seeds (outputs) are non-empty
  - Fee is non-negative
  - Gift amounts sum correctly
  - Spend version tag is valid (`%0` or `%1`)

## Phase 2: Witness Verification

### V0: Direct Signature Verification

V0 spends carry signatures directly. Verification involves:
1. Computing the sig-hash from the spend's seeds and fee
2. For each (pubkey, signature) pair in the signature map:
   - Verify the Schnorr signature against the sig-hash
3. Check that at least `keys_required` valid signatures exist (M-of-N threshold)

### V1: Lock Primitive Satisfaction

V1 verification is more complex, with each lock primitive type checked independently.

#### Lock Merkle Proof Validation

The `check:lock-merkle-proof` arm verifies:
1. The spend condition hashes to the correct leaf
2. The Merkle path from leaf to root is valid (each level combines with the sibling hash)
3. The computed root matches the lock root in the note's Name
4. (Post-Bythos) For full proofs: the axis is committed in the hash and `now ≥ bythos-phase`

#### PKH Signature Check

The `check:pkh` arm verifies (all conditions must hold):
1. **Signature count**: The number of witness PKH entries equals `m` (the M-of-N threshold)
2. **Permissible keys**: The set of witness pubkey hashes is a subset of the committed hashes (checked via `z-in dif`)
3. **Hash binding**: Each `Hash(pubkey)` in the witness matches its committed hash (checked via `z-by rep`)
4. **Batch signature verification**: All Schnorr signatures are verified against the sig-hash in a single batch call via `batch-verify:affine:belt-schnorr:cheetah`

The sig-hash for V1 is computed over the spend's seeds and fee (excluding the witness), ensuring signatures don't circularly depend on themselves.

#### Timelock Check

The `check:tim` arm verifies:
1. Current page number (`now.ctx`) satisfies absolute constraints:
   - `now ≥ abs.min` (if set)
   - `now ≤ abs.max` (if set)
2. Current page number satisfies relative constraints (relative to `since.ctx`, the note's origin page):
   - `now ≥ since + rel.min` (if set)
   - `now ≤ since + rel.max` (if set)

No witness data is needed — timelocks are validated against the block context.

#### Hash Preimage Check

The `check:hax` arm verifies:
1. For each hash commitment in the Hax primitive
2. Look up the corresponding preimage in the witness `hax` map
3. Hash the preimage and verify it matches the committed hash

#### Burn Check

The `%brn` case always returns false — burn primitives can never be satisfied.

### The check-context Arm

V1 introduces a unified `check-context` structure that bundles all context needed for witness verification:

```hoon
++  check-context
  =<  form
  |%
  +$  form
    $:  now=page-number        :: current block height
        since=page-number      :: page height of the note (origin page)
        sig-hash=hash          :: sig-hash of the spend (for signature verification)
        =witness               :: the witness data being verified
        bythos-phase=page-number  :: height at which bythos activates
    ==
  ++  check
    |=  [=form lock=hash]
    ^-  ?
    ...
```

This arm validates a complete spend by:
1. Checking Bythos compatibility (full proofs only after bythos-phase; gates `%full` LMP format to `now >= bythos-phase`)
2. Extracting the spend condition from the lock merkle proof (handles both stub 3-tuple and full 4-tuple formats)
3. Binding the proof root to the parent note's first-name and verifying exact Merkle inclusion. A full proof must also select a complete authorization condition: axis 1 for a single-condition lock, or a balanced OR-tree leaf at the depth for arity 2, 4, 8, or 16, with the final (root-level) sibling equal to the hash of that arity tag.
4. Checking each lock primitive in the spend condition via `levy` (AND logic):
   - `%tim` → check timelocks against current height
   - `%hax` → check hash preimages
   - `%pkh` → check Schnorr signatures (with batch verification)
   - `%brn` → always fail (`%|`)

All checks must pass (AND logic within a spend condition).

Full-proof authorization boundaries are enforced as soon as the upgraded node
validates a spend, at every spending height; they have no activation-height
selector. The standalone full-proof checker and the production proof wrapper
share the same check. Existing lock and note hashes, proof encoding, and honest
proof builders are unchanged. An empty spend condition never authorizes a
spend, under either proof format, whether revealed as a leaf or a whole lock;
complete burn conditions remain valid Merkle leaves but cannot satisfy the burn
primitive. Bythos still independently gates use of the full-proof format, and
the existing seed parent-hash gate is unchanged. Historical validation applies
the same complete-authorization boundary rule at every spending height.

## Phase 3: Context-Aware Validation

Added by Bythos (Protocol 012), context-aware validation checks the transaction against the current chain state.

### validate-with-context

The `validate-with-context` arm in `tx-engine-1.hoon` takes:
- `balance`: the current UTXO set (z-map of Name → Note)
- `sps`: the transaction's spends
- `page-num`: current block height
- `max-size`: maximum note-data size
- `bythos-phase`: activation height for Bythos rules

It performs:

1. **Note-data size check**: Ensure no merged output exceeds `max-size`
2. **For each spend**:
   a. **UTXO existence**: The referenced note must exist in the balance
   b. **Version matching**:
      - V0 note (head is cell) → must use Spend0
      - V1 note (head is atom, version=1) → must use Spend1
   c. **Spend verification**:
      - Spend0: verify V0-style signature, check gifts and fees
      - Spend1: build check-context, verify lock, check gifts and fees
   d. **Gifts and fees**: Total output gifts + fee ≤ input note's assets

### Error Taxonomy

The validation pipeline returns named rejection reasons (Hoon `@tas` cords):

| Error | Meaning |
|---|---|
| `%v1-note-data-exceeds-max-size` | Merged note-data exceeds max-size limit |
| `%v1-input-missing` | Referenced UTXO not in balance |
| `%v1-spend-version-mismatch` | V0 note with Spend1, or V1 note with Spend0 |
| `%v1-note-version-mismatch` | V1 note has unexpected version number |
| `%v1-spend-0-verify-failed` | Spend0 signature verification failed |
| `%v1-spend-0-gifts-failed` | Spend0 output amounts don't balance |
| `%v1-spend-1-lock-failed` | Spend1 lock merkle proof / witness check failed |
| `%v1-spend-1-gifts-failed` | Spend1 output amounts don't balance |

## Mempool vs Block Validation

Bythos tightened the gap between mempool admission and block validation:

**Before Bythos**: Mempool admission used only structural validation (`validate:raw-tx`) plus UTXO presence checks. Context-invalid transactions could circulate in mempools until block processing rejected them.

**After Bythos**: V1 transactions in the mempool must also pass `validate-with-context` using current chain state:

```hoon
=/  ctx-valid=(reason:t ~)
  ?^  -.raw
    [%.y ~]
  %-  validate-with-context:spends:t
  :*  get-cur-balance:con
      spends.raw
      get-cur-height:con
      max-size.data.constants.k
      bythos-phase.constants.k
  ==
?.  ?=(%.y -.ctx-valid)
  ~>  %slog.[1 (cat 3 'heard-tx: Transaction context invalid: ' +.ctx-valid)]
  `k
```

This means transactions that are structurally valid but fail context checks (expired timelocks, invalid lock proofs, oversized note-data, missing UTXOs) are dropped immediately at receipt rather than propagated through the network.

## Validation Flow Summary

```
Transaction arrives
    │
    ├─ Phase 1: Structural Validation
    │   ├─ Version check
    │   ├─ ID integrity
    │   ├─ Non-empty inputs
    │   └─ Internal consistency
    │
    ├─ Phase 2: Witness Verification
    │   ├─ Lock Merkle proof validation
    │   ├─ PKH signature verification
    │   ├─ Timelock satisfaction
    │   ├─ Hash preimage verification
    │   └─ Burn rejection
    │
    └─ Phase 3: Context-Aware Validation (Bythos+)
        ├─ Note-data size limits
        ├─ UTXO existence
        ├─ Version matching
        ├─ Full witness+lock verification
        └─ Gift/fee balance check
```

Each phase can independently reject the transaction with a specific error code. A transaction must pass all three phases to be admitted to the mempool or included in a block.
