# The UTXO Model: Notes, Names, and Balances

## Overview

Nockchain uses an unspent transaction output (UTXO) model, where all on-chain value exists as discrete, immutable "notes" that are created by transactions and consumed (spent) exactly once. This is the same fundamental model used by Bitcoin, in contrast to Ethereum's account-based model.

## Notes = UTXOs

A **note** is Nockchain's UTXO. It represents a discrete unit of value locked under specific spending conditions. Notes exist in two versions, reflecting the protocol's evolution.

All note types are defined authoritatively in Hoon (`hoon/common/tx-engine-0.hoon` and `tx-engine-1.hoon`). The Rust structs in `crates/lib/nockchain-types/` are serialization mirrors that encode/decode Hoon nouns for networking and applications.

### NoteV0 (Genesis)

The Hoon type definition (authoritative):

```hoon
::  from hoon/common/tx-engine-0.hoon (++nnote, line 1621)
+$  form
  $:  $:  version=%0
        origin-page=page-number
        =timelock
      ==
    name=nname
    =sig             :: M-of-N multisig (Hoon type is ++sig, Rust mirrors as Lock)
    =source
    assets=coins
  ==
```

Rust serialization mirror:

```rust
// crates/lib/nockchain-types/src/tx_engine/v0/note.rs:99-102
pub struct NoteV0 {
    pub head: NoteHead,    // version, origin_page, timelock
    pub tail: NoteTail,    // name, lock, source, assets
}
```

A V0 note carries:
- **version**: always `V0` (encoded as `0`)
- **origin_page**: the block height at which this note was added to the balance
- **timelock**: optional absolute/relative constraints on when the note can be spent
- **name**: the unique identifier (see below)
- **sig**: M-of-N multisig condition (Hoon `++sig` type: `m` + z-set of Schnorr public keys; Rust mirrors as `Lock`)
- **source**: provenance tracking (hash + coinbase flag)
- **assets**: value in nicks

### NoteV1 (Post-SegWit)

Hoon type definition (authoritative, from `tx-engine-1.hoon`):

```hoon
+$  nnote-1
  $:  version=%1
      origin-page=page-number
      name=nname
      note-data=(z-map @tas *)
      assets=coins
  ==
```

Rust serialization mirror:

```rust
// crates/lib/nockchain-types/src/tx_engine/v1/note.rs:58-65
pub struct NoteV1 {
    pub version: Version,       // always V1
    pub origin_page: BlockHeight,
    pub name: Name,
    pub note_data: NoteData,    // NEW: arbitrary key-value data (eUTXO datum)
    pub assets: Nicks,
}
```

V1 notes replace the explicit `Lock` and `Source` fields with:
- **note_data**: a key-value map of arbitrary data (the eUTXO-inspired datum)
- The lock information is now embedded in the `Name` itself (via the lock root hash)

The lock is no longer stored in the note directly — instead, the note's `Name` commits to the lock root, and the spender must provide a `LockMerkleProof` in the witness to demonstrate they know a valid spending path.

## Name: UTXO Identifier

A **Name** (`nname` in Hoon) uniquely identifies a note within the UTXO set. Defined in Hoon as a pair of hashes plus a null terminator:

```hoon
+$  nname  [first=hash last=hash ~]
```

Rust serialization mirror:

```rust
// crates/lib/nockchain-types/src/tx_engine/common/mod.rs:266-270
pub struct Name {
    pub first: Hash,    // lock root hash
    pub last: Hash,     // source-derived hash (parent hash of transaction)
    null: usize,        // always 0 (Hoon noun alignment)
}
```

### Comparison with Bitcoin's Outpoint

| Property | Bitcoin Outpoint | Nockchain Name |
|---|---|---|
| Structure | `(txid, vout_index)` | `(lock_root_hash, source_last_hash)` |
| References | Transaction + output index | Lock commitment + source commitment |
| Size | 36 bytes (32 + 4) | 80 bytes (2 × Tip5 hash) |
| Commits to lock? | No (lock is in the output) | Yes (first = lock root hash) |

The key difference is that a Nockchain Name **commits to the spending conditions** in its identity. The `first` field is the hash of the lock tree root, meaning the UTXO's identity is intrinsically bound to how it can be spent. This is closer to Bitcoin's P2SH/P2WSH model (where the address commits to the script hash) than to bare P2PK.

The `last` field derives from the source transaction's hash, ensuring uniqueness even when two outputs share the same lock.

## Lock: Spending Conditions

### V0 Lock (Simple M-of-N)

Authoritative Hoon type (`tx-engine-0.hoon:1389`, named `++sig`):

```hoon
+$  form
  $~  [m=1 pubkeys=*(z-set schnorr-pubkey)]
  [m=@udD pubkeys=(z-set schnorr-pubkey)]
```

Rust serialization mirror:

```rust
// crates/lib/nockchain-types/src/tx_engine/v0/note.rs:39
pub struct Lock {
    pub keys_required: u64,         // M (Hoon: m=@udD, max 255)
    pub pubkeys: Vec<SchnorrPubkey>, // N public keys (Hoon: z-set)
}
```

This is analogous to Bitcoin's `OP_CHECKMULTISIG` — a simple threshold scheme requiring M of N Schnorr signatures. The Hoon type uses a `z-set` (Tip5-ordered persistent set) for pubkeys, ensuring deterministic ordering; the Rust mirror serializes this as a `Vec`.

### V1 Lock (Lock Tree)

In V1, the lock concept was generalized to a **tree of spend conditions** (see [03-taproot-lock-merkle-proofs.md](03-taproot-lock-merkle-proofs.md) and [05-lock-primitives-script-model.md](05-lock-primitives-script-model.md)). The lock is no longer stored on the note; instead, the note's Name commits to the lock root hash, and spenders provide a Merkle proof to a specific branch.

## Asset Denomination

```rust
// crates/lib/nockchain-types/src/tx_engine/common/mod.rs:82
pub struct Nicks(pub usize);
```

Nockchain uses a two-tier denomination:
- **Nicks**: the base unit (analogous to satoshis)
- **Nocks**: the display unit (analogous to bitcoin), where `1 nock = 65536 nicks` (2^16)

The choice of 2^16 as the subdivision factor (vs Bitcoin's 10^8) reflects Nockchain's preference for power-of-two arithmetic, consistent with the Nock VM's binary tree orientation.

## Balance: The UTXO Set

The full UTXO set is called the **balance** — a persistent sorted map (`z-map`) from Names to Notes:

```rust
// crates/lib/nockchain-types/src/tx_engine/v1/note.rs:23
pub struct Balance(pub Vec<(Name, Note)>);
```

Where `Note` is a version-discriminated enum:

```rust
// crates/lib/nockchain-types/src/tx_engine/v1/note.rs:53-56
pub enum Note {
    V0(NoteV0),
    V1(NoteV1),
}
```

Discrimination is structural: V0 notes have a cell (pair) as their head element; V1 notes have an atom (the version number). This allows the balance to contain both V0 and V1 notes simultaneously during the transition period.

## Source: Provenance Tracking

```rust
// crates/lib/nockchain-types/src/tx_engine/common/mod.rs:134-137
pub struct Source {
    pub hash: Hash,
    pub is_coinbase: bool,
}
```

Every note tracks its origin: whether it was created by a coinbase (mining reward) or a regular transaction, plus a hash linking to the originating transaction. This is used in the Name computation and for validation rules that may treat coinbase outputs differently (e.g., maturation requirements).

## Transaction Structure

### V0 Transaction

```rust
// crates/lib/nockchain-types/src/tx_engine/v0/tx.rs:95-100
pub struct RawTx {
    pub id: TxId,
    pub inputs: Inputs,               // z-map of (Name → Input)
    pub timelock_range: TimelockRangeAbsolute,
    pub total_fees: Nicks,
}
```

V0 transactions embed the full note in each input (`Input { note, spend }`), and the spend carries an optional signature directly.

### V1 Transaction

```rust
// crates/lib/nockchain-types/src/tx_engine/v1/tx.rs:19-23
pub struct RawTx {
    pub version: Version,
    pub id: TxId,
    pub spends: Spends,               // z-map of (Name → Spend)
}
```

V1 transactions are leaner: they reference notes by Name rather than embedding them, and carry a version tag. The `Spends` map associates each consumed note (by Name) with its spending proof. See [02-segwit-witness-separation.md](02-segwit-witness-separation.md) for the Spend structure.

## Key Design Observations

1. **Identity-committed locks**: Unlike Bitcoin where an outpoint is `(txid, index)` and the lock script lives in the output, Nockchain bakes the lock hash into the Name itself. This means you cannot construct a valid Name without knowing the lock, providing an inherent binding between identity and spending authority.

2. **Structural version discrimination**: Rather than using explicit version tags for note discrimination, the system uses structural patterns (cell head vs atom head). This is idiomatic Hoon — types are discriminated by shape, not by tags.

3. **Persistent balanced trees**: The balance (UTXO set) is stored as a z-map (a persistent balanced binary tree with Tip5-based ordering), which allows efficient functional updates — inserting and removing notes without mutating the existing tree.
