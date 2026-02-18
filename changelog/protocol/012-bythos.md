+++
version = "0.1.11"
status = "final"
consensus_critical = true

activation_height = 54000
published = "2026-01-19"
activation_target = "2026-03-01"

authors = ["@nockchain-core"]
reviewers = ["@nockchain-core"]

supersedes = "0.1.10"
superseded_by = ""
+++

# Bythos

Versioned lock merkle proofs and fee rebalancing.

## Summary

Bythos adds a `version` field to `lock-merkle-proof` so the axis is included in witness hashes. It also rebalances fees by lowering `base-fee` to 2^14 and charging inputs at 1/4 the output rate via `input-fee-divisor`.

## Motivation

### Lock Merkle Proof Versioning

The original `lock-merkle-proof` hashable did not include the `axis` field. It used a hardcoded hash as a placeholder. This meant the witness hash did not commit to which branch of the lock script was being executed.

The new `full` structure includes axis in the hashable. The old `stub` structure is kept so existing proofs still decode.

### Fee Structure

The old fee calculation charged the same rate for all transaction data. But outputs create new UTXOs that persist until spent, while inputs just reference existing UTXOs and are consumed immediately. Outputs cost more to store.

`input-fee-divisor` lets inputs be charged at a fraction of the output rate (default: 1/4).

## Technical Specification

### Lock Merkle Proof Versioning

#### Type Definitions

**Stub (legacy format):**
```
$lock-merkle-proof-stub
  [=spend-condition axis=@ =merk-proof:merkle]
```

**Full (new format):**
```
$lock-merkle-proof-full
  [version=%full =spend-condition axis=@ =merk-proof:merkle]
```

**Union type:**
```
$lock-merkle-proof
  $^([=spend-condition axis=@ =merk-proof:merkle]
     [version=%full =spend-condition axis=@ =merk-proof:merkle])
```

Discrimination is by structure: 3-tuple → stub, 4-tuple with `%full` tag → full.

#### Hashable Changes

**Stub hashable** (unchanged from v0.1.10):
```hoon
:+  hash+(hash:spend-condition spend-condition.form)
    hash+(from-b58:^hash '6mhCSwJQDvbkbiPAUNjetJtVoo1VLtEhmEYoU4hmdGd6ep1F6ayaV4A')
  (hashable-merk-proof merk-proof.form)
```

Note: The second element is a hardcoded hash that does not commit to axis.

**Full hashable** (new):
```hoon
:*  leaf+version.form
    hash+(hash:spend-condition spend-condition.form)
    leaf+axis.form
    (hashable-merk-proof merk-proof.form)
==
```

The full version includes `axis` as a leaf in the hashable tree.

#### Activation Gating

Lock merkle proof format is determined by the note's origin page:

- **Before bythos-phase**: Wallet builds stub proofs, nodes accept stub proofs only
- **At/after bythos-phase**: Wallet builds full proofs, nodes accept both formats

```hoon
=/  parent-lmp=lock-merkle-proof
  ?:  (gte origin-page.note bythos-phase)
    (build-lock-merkle-proof-full:lock parent-lock 1)
  (build-lock-merkle-proof-stub:lock parent-lock 1)
```

The `check-context` arm validates that full proofs are only used at/after bythos-phase:

```hoon
++  check
  |=  [=form lock=hash]
  ^-  ?
  =/  bythos-ok=?
    ?:  ?=([%full * * *] lmp.witness.form)
      (gte now.form bythos-phase.form)
    %.y
  =/  sc=spend-condition
    ?:  ?=([%full * * *] lmp.witness.form)
      =+  [ver sc ax mp]=lmp.witness.form
      sc
    =+  [sc ax mp]=lmp.witness.form
    sc
  ?&
      bythos-ok
      (check:lock-merkle-proof lmp.witness.form lock)
      %+  levy  sc
      |=  p=lock-primitive
      ^-  ?
      ?-  -.p
        %tim  (check:tim +.p form)
        %hax  (check:hax +.p form)
        %pkh  (check:pkh +.p form)
        %brn  %|
      ==
  ==
```

#### Rust Types

```rust
#[derive(NounEncode, NounDecode)]
pub struct LockMerkleProofStub {
    pub spend_condition: SpendCondition,
    pub axis: u64,
    pub proof: MerkleProof,
}

#[derive(NounEncode, NounDecode)]
pub struct LockMerkleProofFull {
    pub version: u64,  // tas!(b"full")
    pub spend_condition: SpendCondition,
    pub axis: u64,
    pub proof: MerkleProof,
}

#[derive(NounEncode, NounDecode)]
#[noun(untagged)]
pub enum LockMerkleProof {
    Full(LockMerkleProofFull),  // Tried first (4-tuple)
    Stub(LockMerkleProofStub),  // Fallback (3-tuple)
}
```

### Fee Structure Changes

#### Blockchain Constants

New field added to `blockchain-constants`:

```hoon
$:  v1-phase=@
    bythos-phase=@           :: NEW - activation height for this upgrade
    data=[max-size=@ min-fee=@]
    base-fee=@
    input-fee-divisor=@      :: NEW
    blockchain-constants:v0
==
```

**Default values:**
| Constant | Mainnet | Fakenet |
|----------|---------|---------|
| `bythos-phase` | 54,000 | 54,000 |
| `base-fee` | 16384 (2^14) | 128 |
| `input-fee-divisor` | 4 | 4 |

Note: Previous `base-fee` was 32768 (2^15).

**Rust struct changes:**
```rust
pub struct BlockchainConstants {
    // ... existing fields ...
    pub bythos_phase: u64,      // NEW
    pub input_fee_divisor: u64, // NEW
}
```

#### Fee Calculation

The `calculate-min-fee` arm now takes a page number and separates seed (output) and witness (input) fees. The input discount only applies at or after `bythos-phase`:

```hoon
++  calculate-min-fee
  |=  [sps=form page-num=page-number]
  ^-  coins
  =/  seed-word-count=@  (count-seed-words sps)
  =/  witness-word-count=@
    %+  roll  ~(tap z-by sps)
    |=  [[nam=nname sp=spend] acc=@]
    (add acc (count-witness-words:spend-v1 sp))
  ::  inputs pay discounted fee only at/after bythos activation
  =/  witness-divisor=@
    ?:  (gte page-num bythos-phase)
      input-fee-divisor
    1
  ::  outputs (seeds) pay full base-fee per word
  =/  seed-fee=coins  (mul seed-word-count base-fee)
  ::  inputs (witnesses) pay base-fee / witness-divisor per word
  =/  witness-fee=coins  (div (mul witness-word-count base-fee) witness-divisor)
  =/  word-fee=coins  (add seed-fee witness-fee)
  (max word-fee min-fee.data)
```

**Formula (at/after bythos-phase):**
```
min_fee = max(
  (seed_words * base_fee) + (witness_words * base_fee / input_fee_divisor),
  min_fee_constant
)
```

**Formula (before bythos-phase):**
```
min_fee = max(
  (seed_words + witness_words) * base_fee,
  min_fee_constant
)
```

Note-data accounting for `seed_words` is aggregated per lock root: all note-data maps
for outputs that share the same lock root are merged first, and the resulting map's
leaf count is charged once. This avoids double-charging identical note-data across
multiple outputs to the same lock.

#### Note-Data Size Validation

The `max-size` validation for note-data is now performed per-output (after merging by lock-root) in `validate-with-context`, rather than per-seed. This matches how outputs are actually constructed: seeds with the same lock-root have their note-data merged into a single output.

```hoon
++  note-data-exceeds-max
  |=  [sps=form max=@]
  ^-  ?
  %+  lien  ~(tap z-by (note-data-by-lock-root sps))
  |=  [key=hash note-data=(z-map @tas *)]
  =/  data-size=@
    %-  num-of-leaves:shape
    %-  ~(rep z-by note-data)
    |=  [[k=@tas v=*] tree=*]
    [k v tree]
  (gth data-size max)
```

### Mempool Admission Context Validation

Bythos also tightens mempool admission for v1 transactions. In dumbnet's `heard-tx`
path, nodes now run context-aware validation before adding a transaction to the
mempool.

**Before:** admission used `validate:raw-tx` (structural/signature checks) plus UTXO
presence checks.

**After:** v1 transactions must also pass `validate-with-context:spends` using the
current chain context:

- current heaviest balance (`get-cur-balance:con`)
- current heaviest height (`get-cur-height:con`)
- note-data size limit (`max-size.data.constants.k`)
- activation height for bythos gating (`bythos-phase.constants.k`)

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

This makes mempool policy match block-validation rules for v1 spends at receipt
time. Transactions that are structurally valid but context-invalid (for example:
timelock not yet satisfied, invalid lock witness in current context, full LMP before
`bythos-phase`, or note-data policy violations) are dropped immediately instead of
circulating in mempools until block processing rejects them.

### Lock Root Simplification

The `nname` computation for notes now directly hashes the lock instead of building an intermediate `lock-merkle-proof`:

**Before:**
```hoon
=/  lmp=lock-merkle-proof  (build-lock-merkle-proof:lock lk 1)
=/  root=hash  root.merk-proof.lmp
(new-v1:nname [root [parent %.y]])
```

**After:**
```hoon
=/  root=hash  (hash:lock lk)
(new-v1:nname [root [parent %.y]])
```

This is semantically equivalent but removes unnecessary intermediate computation.

### API Changes

Bythos updates the gRPC v2 wire format for lock merkle proofs:

- `LockMerkleProof` now includes optional `lmp_version` (field `4`)
- `lmp_version = %full` denotes full/versioned proofs
- omitted `lmp_version` denotes legacy stub proofs
- non-`%full` `lmp_version` values are rejected

This keeps pre-Bythos payloads decodable while allowing explicit full-proof encoding post-activation.

## Activation

- **Height**: 54,000
- **Coordination**: None. Upgrade nodes before activation height.

## Migration

### Requirements

- Software version: 0.1.1+
- All nodes must upgrade before activation height

### Configuration

No mandatory configuration changes are required.

Optional controls used during rollout and testing:

- node: `--fakenet-bythos-phase` (override activation height on fakenet)
- wallet: `--allow-low-fee` (unsafe testing flag to bypass min-fee enforcement)

### Data Migration

No data migration is required. The node will handle both stub and full lock merkle proof formats.

### Steps

1. Stop the node
2. Update to version 0.1.1 or later
3. Restart the node

### Rollback

Rollback is only safe before activation height. After activation, downgrading will fork or reject valid blocks.

## Backward Compatibility

### Breaking Changes

This is a **consensus-critical** upgrade. After activation:

- Nodes running pre-0.1.1 software may reject valid transactions that use the new `lock-merkle-proof-full` format
- Fee calculations will differ between old and new nodes for the same transaction
- The `blockchain-constants` noun structure has a new field, which will cause decoding failures on old nodes

### Transaction Compatibility

- Transactions created with old software (using stub proofs) will remain valid
- Transactions created with new software use stub proofs before activation and full proofs at/after `bythos-phase`
- Wallets should be updated to 0.1.1+ to calculate fees correctly

### Network Partition Risk

Nodes that do not upgrade before activation height may:
- Fork onto an incompatible chain
- Reject valid blocks
- Have their transactions rejected by upgraded nodes

**All node operators must upgrade before the activation height.**

## Security Considerations

Including `axis` in the witness hash strengthens commitment to the executed lock branch. No new cryptographic primitives are introduced.

## Operational Impact

Fee estimates and miner revenue dynamics will change due to the new `base-fee` and input discount. 

## Testing and Validation

Recommended validation:
- Decode/encode both stub and full `lock-merkle-proof` formats
- Verify witness hashes differ when `axis` differs
- Check fee calculations match the new formula for representative transactions
- Confirm `nname` outputs are unchanged by the lock root simplification
- Verify full proofs are rejected at heights before `bythos-phase`
- Verify stub proofs remain accepted before and after `bythos-phase`
- Verify full proofs are accepted at heights at/after `bythos-phase`
- Verify note-data size validation is per-output (merged by lock-root), not per-seed
- Verify input fee discount only applies at/after `bythos-phase`
- Verify mempool admission rejects v1 transactions that fail `validate-with-context` at receipt

## Reference Implementation

TBD (link to GitHub PR once available).
