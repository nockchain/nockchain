# Noun Encoding and the Nock Data Layer

## Architecture: Hoon as Source of Truth

A critical architectural point: **Hoon is the source of truth** for all type definitions and validation logic in the Nockchain transaction engine. The Rust type definitions in `crates/lib/nockchain-types/` are serialization/deserialization mirrors — they exist to encode and decode Hoon nouns for networking and application layers, but they do not define the canonical semantics.

This means:
- **Consensus validation runs in Nock**: The Hoon kernel executing on the Nock VM is what actually validates transactions, computes fees, checks witnesses, and updates balances
- **Rust types are wire-format adapters**: They serialize Rust structs into nouns (for sending to the Hoon kernel) and deserialize nouns back into Rust structs (for networking, APIs, and wallet logic)
- **The Hoon types are authoritative**: If the Rust types and Hoon types disagree, the Hoon definition is correct and the Rust code must be fixed

## Nock Nouns: The Universal Data Representation

Nock (the virtual machine underlying Hoon) has exactly one data type: the **noun**. A noun is either:
- An **atom**: an arbitrary-precision natural number (0, 1, 2, ..., 2^256, ...)
- A **cell**: an ordered pair of nouns `[left right]`

Every data structure in the system — transactions, notes, balances, blocks, proofs — is ultimately a noun: a binary tree of natural numbers. This is radically different from conventional type systems:

```
Transaction (as a noun):
[1 [hash...] [[name spend] [name spend] 0]]
 │    │              │
 ver  id          spends (z-map)
```

## NounEncode / NounDecode: The Rust↔Hoon Bridge

The Rust codebase uses `NounEncode` and `NounDecode` traits to bridge between typed Rust structs and untyped Nock nouns:

```rust
// Example from crates/lib/nockchain-types/src/tx_engine/v1/tx.rs:25-31
impl NounEncode for RawTx {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let version = self.version.to_noun(allocator);
        let id = self.id.to_noun(allocator);
        let spends = self.spends.to_noun(allocator);
        nockvm::noun::T(allocator, &[version, id, spends])
    }
}
```

This creates a noun cell tree: `[version [id spends]]` — matching the Hoon type definition:

```hoon
+$  form
  $:  version=%1
      id=tx-id
      =spends
  ==
```

### Structural Discrimination

Because nouns are untyped, version discrimination happens by **examining the shape** of the data. For example, the `Note` enum:

```rust
// crates/lib/nockchain-types/src/tx_engine/v1/note.rs:77-84
impl NounDecode for Note {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let hed = noun.as_cell()?.head();
        match hed.is_cell() {
            true => Ok(Note::V0(NoteV0::from_noun(noun)?)),   // head is cell → V0
            false => Ok(Note::V1(NoteV1::from_noun(noun)?)),   // head is atom → V1
        }
    }
}
```

V0 notes have a `NoteHead` (a cell) as their first element; V1 notes have a `Version` atom (1). The decoder probes the structure to determine the variant. This is idiomatic Hoon — `$^` and `$@` runes discriminate types by whether the head is a cell or atom.

Similarly, `LockMerkleProof` discriminates stub vs full:

```rust
// crates/lib/nockchain-types/src/tx_engine/v1/tx.rs:394-406
impl NounDecode for LockMerkleProof {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        if let Ok(full) = LockMerkleProofFull::from_noun(noun) {
            if full.version != nockvm_macros::tas!(b"full") {
                return Err(NounDecodeError::Custom(...));
            }
            return Ok(Self::Full(full));
        }
        Ok(Self::Stub(LockMerkleProofStub::from_noun(noun)?))
    }
}
```

Try the 4-tuple (Full) first; if it fails or the version tag isn't `%full`, fall back to the 3-tuple (Stub).

## Z-Maps and Z-Sets: Persistent Sorted Trees

Nockchain uses **z-maps** and **z-sets** — balanced binary trees ordered by Tip5 hash — for all collection types. These are the Nock-native equivalents of sorted maps and sets.

### Z-Map (Sorted Map)

Used for:
- **Spends**: `z-map(Name → Spend)` — associates each UTXO being consumed with its spend proof
- **Balance**: `z-map(Name → Note)` — the full UTXO set
- **PkhSignature**: `z-map(Hash → PkhSignatureEntry)` — signature proofs keyed by pubkey hash
- **NoteData**: `z-map(@tas → *)` — arbitrary key-value data on notes
- **Witness Hax**: `z-map(Hash → *)` — hash preimage reveals keyed by commitment hash

```rust
// Z-map put operation (from Spends encoding):
zmap::z_map_put(allocator, &acc, &mut key, &mut value, &DefaultTipHasher)
```

### Z-Set (Sorted Set)

Used for:
- **Seeds**: `z-set(Seed)` — the set of outputs in a spend
- **Pkh hashes**: `z-set(Hash)` — the set of authorized pubkey hashes
- **Hax hashes**: `z-set(Hash)` — the set of hash commitments

```rust
// Z-set put operation (from Seeds encoding):
zset::z_set_put(allocator, &acc, &mut value, &DefaultTipHasher)
```

Both structures use `DefaultTipHasher` (Tip5) for ordering, ensuring deterministic tree layout regardless of insertion order. This is essential for consensus — every node must produce the same noun representation for the same logical data.

## Tip5: The Hash Function

Nockchain uses **Tip5** as its universal hash function for:
- Merkle trees (lock merkle proofs, block commitments)
- Transaction IDs
- Note Names
- UTXO set ordering (z-map/z-set balancing)
- Public key hashing (Pkh)

Tip5 produces a 5-element tuple of field elements (`[Belt; 5]`), where each `Belt` is a `u64` reduced modulo a prime:

```rust
// crates/lib/nockchain-types/src/tx_engine/common/mod.rs:149-150
pub struct Hash(pub [Belt; 5]);
```

### Hashable Construction

Hoon defines `hashable` arms for each type that specify how to construct the hash input tree:

```hoon
++  hashable
  |=  =form
  ^-  hashable:tip5
  :*  leaf+version.form
      hash+(hash:spend-condition spend-condition.form)
      leaf+axis.form
      (hashable-merk-proof merk-proof.form)
  ==
```

The `hashable` type is a tagged tree of `leaf` (raw values) and `hash` (pre-computed hashes) nodes. The Tip5 hasher traverses this tree to produce a deterministic hash. This is defined in Hoon and executed in the Nock VM; the Rust side does not reimplement the hashing logic for consensus purposes.

## Jamming: Noun Serialization

**Jam** is Nock's native serialization format — it compresses an arbitrary noun into a byte sequence. **Cue** is the inverse (deserialization).

Jam is used for:
- **NoteData blobs**: Each note-data entry's value is a jammed noun
- **HaxPreimage values**: Hash preimages are jammed nouns
- **Wire format**: Nouns sent between nodes are jammed for transport

```rust
// Jamming a noun for storage:
let mut slab: NounSlab<NockJammer> = NounSlab::new();
slab.copy_into(raw_value);
let jam = slab.jam();  // → bytes::Bytes

// Cueing bytes back into a noun:
slab.cue_into(entry.blob.clone())?;
```

Jam achieves compression through structure sharing — if the same sub-noun appears multiple times, it's stored once and referenced by back-pointer. This is particularly efficient for the repetitive structures common in transaction data.

## Architectural Significance

### Why Nouns?

The noun-based architecture has several consequences:

1. **Language-independent consensus**: The Nock VM specification is ~300 words. Any implementation that correctly evaluates Nock programs will produce identical results. This makes consensus validation implementation-independent.

2. **Deterministic serialization**: Nouns have a canonical form — the same logical data always produces the same noun, which always produces the same jam bytes, which always produces the same hash. No sorting, normalization, or canonicalization heuristics needed.

3. **Unified type system**: Everything is a noun. There's no impedance mismatch between "transaction types" and "hash inputs" and "serialized bytes" — they're all the same thing at different levels of interpretation.

4. **Functional state updates**: Z-maps and z-sets are persistent (immutable) data structures. Updating the balance (UTXO set) produces a new tree that shares structure with the old one, enabling efficient versioning and rollback.

### Performance Considerations

The noun-based approach has trade-offs:

- **Encoding overhead**: Converting between Rust structs and nouns requires allocation and tree traversal. This is more expensive than zero-copy serialization formats like FlatBuffers.
- **Hash computation**: Tip5 hashing traverses the noun tree, which is more expensive than hashing a flat byte array. However, Tip5 is specifically designed for efficient operation within the Nock VM.
- **Memory layout**: Nouns are pointer-heavy binary trees, which are less cache-friendly than contiguous arrays. The Nock VM uses arena allocation to mitigate this.

These costs are accepted because the noun layer is the consensus boundary — it provides the deterministic execution guarantee that makes decentralized validation possible. Performance-critical paths (like signature verification) are accelerated by **jets** — optimized Rust implementations of commonly-used Nock functions that produce identical results to the pure Nock evaluation.
