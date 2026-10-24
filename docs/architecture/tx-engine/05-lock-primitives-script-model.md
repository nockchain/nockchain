# Lock Primitives: A Composable Script Model

## Context: Programmable Spending

Bitcoin uses Script — a stack-based, non-Turing-complete language — to express spending conditions. Cardano uses Plutus validators — Haskell-based programs compiled to Plutus Core. Nockchain takes a third approach: a fixed set of **lock primitives** composed via AND/OR logic, with Merkle tree branch selection providing the OR.

## LockPrimitive: Authoritative Hoon Definition

Lock primitives are defined in Hoon (`tx-engine-1.hoon`) as a tagged union using `@tas` (text atom symbol) tags. The Hoon definition is the source of truth:

```hoon
+$  lock-primitive
  $%  [%pkh m=@ hashes=(z-set hash)]
      [%tim rel=timelock-range-relative abs=timelock-range-absolute]
      [%hax hashes=(z-set hash)]
      [%brn ~]
  ==
```

A `spend-condition` is a list of lock primitives (AND logic):

```hoon
+$  spend-condition  (list lock-primitive)
```

Rust serialization mirror:

```rust
// crates/lib/nockchain-types/src/tx_engine/v1/tx.rs:478-484
pub enum LockPrimitive {
    Pkh(Pkh),       // %pkh - Pay-to-public-key-hash (M-of-N Schnorr)
    Tim(LockTim),   // %tim - Timelock constraints
    Hax(Hax),       // %hax - Hash preimage verification
    Burn,           // %brn - Unspendable (proof-of-burn)
}
```

## Pkh: Pay-to-Public-Key-Hash

Defined in Hoon as `[%pkh m=@ hashes=(z-set hash)]`: a threshold `m` and a set of public key hashes.

Rust serialization mirror:

```rust
// crates/lib/nockchain-types/src/tx_engine/v1/tx.rs:534-539
pub struct Pkh {
    pub m: u64,            // Required signature count (threshold)
    pub hashes: Vec<Hash>, // z-set of public key hashes
}
```

Pkh is the primary authentication primitive. It requires M Schnorr signatures from a set of N public key hashes — a threshold multisig scheme.

### How Pkh Differs from Bitcoin

| Feature | Bitcoin P2PKH | Bitcoin P2TR Key-Path | Nockchain Pkh |
|---|---|---|---|
| Key type | ECDSA | Schnorr | Schnorr (Cheetah curve) |
| Hash stores | Single pubkey hash | Tweaked public key | Set of pubkey hashes |
| Multisig | Separate OP_CHECKMULTISIG | MuSig2 aggregation | Native M-of-N threshold |
| Reveals | Full public key on spend | Only aggregated key | Signing pubkeys + signatures (lock stores only hashes) |

The Pkh primitive uses **public key hashes** rather than public keys directly. The lock commits to `Hash(pubkey)`, and the witness provides `(pubkey, signature)` pairs. The validator:
1. Checks that `Hash(pubkey)` matches one of the committed hashes
2. Verifies the Schnorr signature against the pubkey and sig-hash
3. Counts that at least M valid signatures are provided

### Witness Satisfaction

The `PkhSignature` in the witness provides the satisfying data:

```rust
// crates/lib/nockchain-types/src/tx_engine/v1/tx.rs:296-301
pub struct PkhSignatureEntry {
    pub hash: Hash,                   // The pubkey hash being satisfied
    pub pubkey: SchnorrPubkey,        // The actual public key
    pub signature: SchnorrSignature,  // Schnorr signature
}
```

The Hoon validation (`tx-engine-1.hoon`, `check:pkh` at line 1656):

```hoon
++  check
  |=  [=form ctx=check-context]
  ?&  =(m.form ~(wyt z-by pkh.witness.ctx))           :: exactly M signatures provided
      =(~ (~(dif z-in ~(key z-by pkh.witness.ctx))    :: all pubkey hashes are in the
               h.form))                                ::   Pkh's permitted set
      %-  ~(rep z-by pkh.witness.ctx)                  :: each pubkey hashes to its
      |=  [[h=^hash pk=schnorr-pubkey sig=schnorr-signature] a=?]  :: claimed hash
      ?&  a  =(h (hash:schnorr-pubkey pk))  ==
      %-  batch-verify:affine:belt-schnorr:cheetah     :: all signatures are valid
      (signatures:pkh-signature pkh.witness.ctx sig-hash.ctx)
  ==
```

This checks four conditions (AND):
1. **Exactly** `m` signatures are provided (not "at least" — the count must match precisely)
2. All provided pubkey hashes are members of the Pkh's permitted hash set
3. Each provided public key hashes to its declared hash (binding pubkey to commitment)
4. All signatures pass batch Schnorr verification against the sig-hash via `batch-verify:affine:belt-schnorr:cheetah`

## Tim: Timelock Constraints

Defined in Hoon as `[%tim rel=timelock-range-relative abs=timelock-range-absolute]`. Each range is a pair of optional bounds (min, max).

Timelocks constrain *when* a note can be spent, with both relative and absolute bounds:

### Comparison with Bitcoin Timelocks

| Feature | Bitcoin CLTV (BIP 65) | Bitcoin CSV (BIP 112) | Nockchain Tim |
|---|---|---|---|
| Type | Absolute | Relative | Both combined |
| Granularity | Block height or timestamp | Block count or time | Block height only |
| Min constraint | ✓ (cannot spend before) | ✓ (must wait N blocks) | ✓ (min height/delta) |
| Max constraint | ✗ (no expiry in CLTV) | ✗ (no expiry in CSV) | ✓ (max height/delta) |
| Implementation | OP_CHECKLOCKTIMEVERIFY | OP_CHECKSEQUENCEVERIFY | Lock primitive |

Nockchain's Tim is more expressive than Bitcoin's individual timelock opcodes because it combines:
- **Absolute minimum**: like CLTV — "cannot spend before block X"
- **Absolute maximum**: "cannot spend after block Y" (not available in Bitcoin)
- **Relative minimum**: like CSV — "must wait N blocks after creation"
- **Relative maximum**: "must spend within N blocks of creation" (not available in Bitcoin)

The relative constraints use `origin_page` (the block where the note was created) as the reference point, analogous to CSV's reference to the block containing the spending transaction's input.

### Witness Satisfaction

The `tim` field in the Witness is currently reserved (always 0). Timelock validation is **context-based** — it checks the current block height against the constraints without requiring witness data.

The Hoon validation (`tx-engine-1.hoon`, `check:tim` at line 1741):

```hoon
++  check
  |=  [=form ctx=check-context]
  =/  rmin-ok=?  ?~(min.rel.form %.y (gte now.ctx (add since.ctx u.min.rel.form)))
  =/  rmax-ok=?  ?~(max.rel.form %.y (lte now.ctx (add since.ctx u.max.rel.form)))
  =/  amin-ok=?  ?~(min.abs.form %.y (gte now.ctx u.min.abs.form))
  =/  amax-ok=?  ?~(max.abs.form %.y (lte now.ctx u.max.abs.form))
  &(rmin-ok rmax-ok amin-ok amax-ok)
```

Where `now.ctx` is the current block height and `since.ctx` is the note's `origin_page`. Each constraint is optional (`unit`); absent constraints are vacuously satisfied.

## Hax: Hash Preimage Verification

Defined in Hoon as `[%hax hashes=(z-set hash)]`: a set of Tip5 hash commitments.

Hax requires the spender to reveal preimages whose hashes match the committed values. This is the foundation for:
- **Hash Time-Locked Contracts (HTLCs)**: Combined with Tim for atomic swaps
- **Payment channels**: Conditional payments based on secret revelation
- **Commit-reveal schemes**: On-chain commitment to off-chain data

### Comparison with Bitcoin Hash Locks

| Feature | Bitcoin OP_SHA256/OP_HASH160 | Nockchain Hax |
|---|---|---|
| Hash function | SHA-256, RIPEMD-160, SHA-1 | Tip5 |
| Multiple preimages | Via script composition | Native set of hash commitments |
| Preimage type | Raw bytes | Jammed nouns (arbitrary Nock data) |

### Witness Satisfaction

The `hax` field in the Witness provides preimage revelations:

```rust
// crates/lib/nockchain-types/src/tx_engine/v1/tx.rs:253-258
pub struct HaxPreimage {
    pub hash: Hash,
    pub value: bytes::Bytes,  // Jammed noun bytes
}
```

The Hoon validation (`tx-engine-1.hoon`, `check:hax` at line 1704):

```hoon
++  check
  |=  [=form ctx=check-context]
  %-  ~(all z-in form)
  |=  =^hash
  =/  preimage  (~(get z-by hax.witness.ctx) hash)
  ?~  preimage  %|
  =(hash (hash-noun u.preimage))
```

For each committed hash in the Hax set:
1. Look up the preimage in the witness `hax` map (`z-map hash *`)
2. If no preimage provided, fail
3. Hash the preimage via `hash-noun` (recursive hashable decomposition of the noun tree) and check it matches the committed hash

The `hash-noun` arm builds a hashable tree from the noun's structure — cells become pairs, atoms become leaves — then hashes via `hash-hashable:tip5`. This is distinct from `hash-noun-varlen` (used in zoon for tree ordering), which uses the Dyck word encoding.

In Hoon, preimage values are arbitrary nouns (`*`). The Rust serialization represents them as **jammed noun bytes** (`bytes::Bytes`) — Nock's binary serialization format — which are cued (decompressed) back to nouns during validation. This means preimages can be any structured Nock data (lists, trees, records), serialized to bytes for transport.

## Burn: Unspendable Output

```rust
LockPrimitive::Burn  // Tag: "brn", value: 0
```

Burn creates an **unconditionally unspendable** output. Any SpendCondition containing a Burn primitive can never be satisfied:

```hoon
%brn  %|   :: always fails
```

This is Nockchain's equivalent of Bitcoin's `OP_RETURN` — used for:
- **Proof-of-burn**: Permanently destroying value
- **Data anchoring**: Committing data to the chain without creating spendable outputs
- **Mining commitments**: Permanently locking value as a proof mechanism

## Protocol-Fund Recovery

Protocol-fund coinbase notes whose first name is `fund-note-firstname` use the
dedicated recovery rule in `check-context`. Their original lock wrapped the
fund's multisig lock root as a public-key hash, so recovery checks the revealed
multisig condition against `protocol-fund-address` instead of checking the
original Merkle path.

The revealed spend condition must hash to the pinned fund target, and its
primitives must verify against the spending transaction's signature hash. The
configured fund maturity rule still applies. Mempool validation and block
admission share this rule; ordinary lock-condition handling is unchanged.

## Composition Model

### AND Logic: Within a SpendCondition

A `SpendCondition` is a **list** of `LockPrimitive` values. All primitives in the list must be satisfied — AND logic:

```rust
// crates/lib/nockchain-types/src/tx_engine/v1/tx.rs:442-443
pub struct SpendCondition(pub Vec<LockPrimitive>);
```

Example: `[Pkh(1-of-2), Tim(min=100)]` means "provide 1 of 2 valid signatures AND the block height must be ≥ 100."

### OR Logic: Across Lock Tree Branches

Different branches of the lock Merkle tree provide OR semantics — any one branch can authorize spending. The spender chooses which branch to reveal via the `LockMerkleProof`.

Example lock tree:
```
         Root
        /    \
  Branch A   Branch B
  [Pkh(2-of-3)]  [Tim(height>1000), Hax(secret)]
```

This means: "Either provide 2-of-3 signatures, OR wait until block 1000 and reveal the secret."

### Expressiveness Summary

The AND/OR composition with four primitive types enables:

| Pattern | Primitives Used | Use Case |
|---|---|---|
| Simple pay | Pkh(1-of-1) | Standard payment |
| Multisig | Pkh(M-of-N) | Shared custody |
| Timelock + multisig | Pkh + Tim | Vesting schedule |
| HTLC | Pkh + Hax + Tim | Atomic swaps |
| Dead man's switch | Branch A: Pkh(owner) OR Branch B: Pkh(heir) + Tim(delay) | Inheritance |
| Proof-of-burn | Burn | Value destruction |
| Escrow with timeout | Branch A: Pkh(2-of-2) OR Branch B: Pkh(sender) + Tim(expiry) | Conditional escrow |

### What Cannot Be Expressed

The fixed primitive set means some Bitcoin Script / Plutus capabilities are not available:
- **Arithmetic conditions**: No equivalent to `OP_ADD`, `OP_LESSTHAN`
- **Introspection**: No access to transaction structure (inputs/outputs) within primitives
- **Loops**: Not possible (but also not possible in Bitcoin Script)
- **State machines**: Cannot inspect note data during validation (unlike Cardano validators)
- **Covenants**: Cannot constrain how outputs must be structured

This is a deliberate trade-off: simplicity and auditability over expressiveness. The four-primitive model covers the vast majority of practical spending patterns while remaining easy to formally verify.
