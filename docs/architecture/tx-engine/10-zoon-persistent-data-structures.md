# Zoon: Hash-Ordered Persistent Trees

## Overview

Zoon (`hoon/common/zoon.hoon`, ~700 lines) is Nockchain's vendored balanced tree library, adapted from Hoon's standard `by`/`in` containers. It provides cryptographically-ordered persistent (immutable) collections — z-maps and z-sets — that use Tip5 hashing for deterministic element ordering. These are the data structures that hold the UTXO set, transaction spends, seeds, signatures, and every other collection in the transaction engine.

Zoon explicitly deprecates the standard Hoon containers:

```hoon
::  /lib/zoon: vendored types from hoon.hoon
+|  %no-by-in
++  by  %do-not-use
++  in  %do-not-use
++  ju  %do-not-use
++  ja  %do-not-use
++  bi  %do-not-use
```

This ensures the entire codebase uses Tip5-ordered trees, which is critical for consensus — all nodes must produce identical noun representations for the same logical data.

## Import Chain

```
zoon.hoon
  └─ imports zeke.hoon
       └─ re-exports ztd/eight.hoon
            └─ ... (full STARK stack, including Tip5)
```

Zoon imports `zeke.hoon` (a 1-line re-export of `ztd/eight.hoon`) to get access to the Tip5 hash function. The jet hint `~% %zoon ..stark-engine-jet-hook:z ~` registers the entire zoon library for JIT acceleration by the Nock VM.

## z-map: Persistent Sorted Map

```hoon
++  z-map
  |$  [key value]
  $|  (tree (pair key value))
  |=(a=(tree (pair)) ?:(=(~ a) & ~(apt z-by a)))
```

A z-map is a balanced binary tree where each node is a `[key value]` pair. The tree maintains two invariants:

1. **gor-tip ordering**: Keys are ordered by their Tip5 hash (primary key comparison via `gor-tip`)
2. **mor-tip heap property**: Parent nodes have higher `mor-tip` (double-hash) priority than children

Together, these make the tree a **treap** (tree + heap) — the combination of hash-based ordering and hash-based priority ensures a unique tree shape for any set of keys, regardless of insertion order.

### z-by: The z-map Engine

The `z-by` core provides the full API, all jet-hinted with `~/`:

| Arm | Purpose | Complexity |
|---|---|---|
| `get` | Look up value by key | O(log n) |
| `put` | Insert or update key-value pair | O(log n) |
| `del` | Remove key from map | O(log n) |
| `has` | Check key existence | O(log n) |
| `gas` | Batch insert from list | O(k log n) |
| `uni` | Union (merge two maps, right-biased) | O(n + m) |
| `int` | Intersection of two maps | O(n + m) |
| `dif` | Difference (remove keys in other map) | O(n + m) |
| `bif` | Split map at a key | O(log n) |
| `tap` | Convert to list (in-order traversal) | O(n) |
| `rep` | Fold/reduce over all entries | O(n) |
| `run` | Apply gate to all values | O(n) |
| `urn` | Apply gate to all key-value pairs | O(n) |
| `wyt` | Count entries | O(n) |
| `key` | Extract z-set of keys | O(n) |
| `val` | Extract list of values | O(n) |
| `apt` | Validate tree invariants | O(n) |
| `dig` | Find axis of key in tree | O(log n) |
| `jab` | Update value at key via gate | O(log n) |
| `mar` | Conditional put/delete | O(log n) |
| `uno` | General union with merge function | O(n + m) |

### The `put` Algorithm

The `put` arm illustrates how the treap invariants work:

```hoon
++  put
  ~/  %put
  |*  [b=* c=*]
  |-  ^+  a
  ?~  a
    [[b c] ~ ~]
  ?:  =(b p.n.a)
    ?:  =(c q.n.a)  a
    a(n [b c])
  ?:  (gor-tip b p.n.a)
    =+  d=$(a l.a)
    ?>  ?=(^ d)
    ?:  (mor-tip p.n.a p.n.d)
      a(l d)
    d(r a(l r.d))
  =+  d=$(a r.a)
  ?>  ?=(^ d)
  ?:  (mor-tip p.n.a p.n.d)
    a(r d)
  d(l a(r l.d))
```

1. If tree is empty: create leaf `[[key value] ~ ~]`
2. If key matches: update value
3. Recurse left or right based on `gor-tip` comparison
4. After insertion, check `mor-tip` heap property; if violated, rotate

## z-set: Persistent Sorted Set

```hoon
++  z-set
  |$  [item]
  $|  (tree item)
  |=(a=(tree) ?:(=(~ a) & ~(apt z-in a)))
```

A z-set is a z-map with keys only (no values). The `z-in` engine provides a parallel API: `put`, `has`, `del`, `dif`, `int`, `uni`, `bif`, `tap`, `wyt`, `all`, `any`, `run`, `gas`, `rep`, `dig`.

## z-mip and z-jug: Nested Collections

Zoon also provides two higher-order collection types:

**z-mip** (map of maps):
```hoon
++  z-mip  |$  [kex key value]  (z-map kex (z-map key value))
```

Used in the tx-engine for `note-data-by-lock-root`: a map from lock-root hash to note-data maps.

**z-jug** (map of sets):
```hoon
++  z-jug  |$  [key value]  (z-map key (z-set value))
```

A key-to-set-of-values mapping, with `z-ju` engine providing `put`, `get`, `has`, `del`, `gas`.

## Ordering Functions: The Cryptographic Heart

The ordering functions define the tree layout. All comparisons ultimately derive from Tip5 hashing.

### tip: Primary Hash

```hoon
++  tip
  |=  a=*
  ^-  noun-digest:tip5:z
  (hash-noun-varlen:tip5:z a)
```

Computes the Tip5 hash of an arbitrary noun. This is a 5-element digest `[Belt; 5]`.

### double-tip: Secondary Hash

```hoon
++  double-tip
  |=  a=*
  ^-  noun-digest:tip5:z
  =/  one  (tip a)
  (hash-ten-cell:tip5:z one one)
```

Hashes the Tip5 digest with itself, producing a secondary hash used for the heap property.

### dor-tip: Canonical Depth-First Ordering

```hoon
++  dor-tip
  ~/  %dor-tip
  |=  [a=* b=*]
  ^-  ?
  ?:  =(a b)  &
  ?.  ?=(@ a)
    ?:  ?=(@ b)  |
    ?:  =(-.a -.b)  $(a +.a, b +.b)
    $(a -.a, b -.b)
  ?.  ?=(@ b)  &
  (lth a b)
```

A deterministic fallback ordering: atoms before cells, then left-to-right comparison. Used when Tip5 hashes collide (astronomically unlikely but handled for correctness).

### gor-tip: Primary Key Ordering

```hoon
++  gor-tip
  ~/  %gor-tip
  |=  [a=* b=*]
  ^-  ?
  =+  [c=(tip a) d=(tip b)]
  ?:  =(c d)  (dor-tip a b)
  (lth-tip c d)
```

Compare by Tip5 hash; fall back to `dor-tip` on collision. This is the **BST property** — determines left vs right in the tree.

### mor-tip: Heap Priority Ordering

```hoon
++  mor-tip
  ~/  %mor-tip
  |=  [a=* b=*]
  ^-  ?
  =+  [c=(double-tip a) d=(double-tip b)]
  ?:  =(c d)  (dor-tip a b)
  (lth-tip c d)
```

Compare by double-Tip5 hash; fall back to `dor-tip` on collision. This is the **heap property** — determines parent vs child.

Using two independent hash functions (Tip5 and double-Tip5) for BST ordering and heap priority ensures that the tree shape is deterministic and unique for any set of elements — a cryptographic treap.

## Why Hash-Based Ordering Matters for Consensus

In a decentralized system, every node must independently arrive at the same UTXO set representation. If tree layout depended on insertion order, different nodes processing the same transactions in different orders would produce different trees — same logical data, different nouns, different hashes.

Tip5-based ordering guarantees:
1. **Deterministic layout**: Same elements → same tree → same noun → same hash
2. **No sorting needed**: Elements self-organize via their hashes during insertion
3. **Merkle-like properties**: The tree root hash effectively commits to the entire set's contents
4. **Efficient verification**: Two nodes can compare z-maps by comparing root hashes

## Rust Jet Implementations

The ordering functions and z-map/z-set operations are jetted in Rust for performance:

| Hoon | Rust File | Key Functions |
|---|---|---|
| `tip`, `double-tip`, `gor-tip`, `mor-tip`, `dor-tip`, `lth-tip` | `crates/lib/nockchain-math/src/zoon/common.rs` | `TipHasher` trait, `DefaultTipHasher` |
| `z-map put` | `crates/lib/nockchain-math/src/zoon/zmap.rs` | `z_map_put()`, `z_map_rep()` |
| `z-set put/bif/dif` | `crates/lib/nockchain-math/src/zoon/zset.rs` | `z_set_put()`, `z_set_bif()`, `z_set_dif()` |

The `TipHasher` trait abstracts the hash function:

```rust
pub trait TipHasher {
    fn hash_noun_varlen<A: NounAllocator>(
        &self, stack: &mut A, a: Noun,
    ) -> Result<[u64; 5], JetErr>;
    fn hash_ten_cell(&self, ten: [u64; 10]) -> Result<[u64; 5], JetErr>;
}
```

Note that `hash_ten_cell` takes a single `[u64; 10]` array (the two 5-element digests concatenated), not two separate `[u64; 5]` arrays. `DefaultTipHasher` uses Tip5, but the trait allows alternative hashers for testing.

## Usage in the Transaction Engine

Every collection type in the tx-engine is a z-map or z-set:

| Collection | Type | Keys | Values |
|---|---|---|---|
| Spends | z-map | Name (UTXO ID) | Spend (proof) |
| Balance (UTXO set) | z-map | Name | Note |
| Seeds (outputs) | z-set | Seed | — |
| PkhSignature | z-map | Hash (pubkey hash) | PkhSignatureEntry |
| NoteData | z-map | @tas (string key) | * (arbitrary noun) |
| Hax (hash commitments) | z-set | Hash | — |
| Pkh hashes | z-set | Hash | — |
| note-data-by-lock-root | z-mip | Hash (lock root) | @tas → * |

## Comparison with Other Blockchains

| Aspect | Bitcoin | Cardano | Nockchain |
|---|---|---|---|
| UTXO set structure | LevelDB flat index | Haskell `Data.Map` | z-map (Tip5-ordered treap) |
| Ordering | None (hash-indexed) | Ord instance | Cryptographic (Tip5 hash) |
| Persistence | Copy-on-write DB | Persistent maps | Structural sharing (immutable trees) |
| Deterministic layout | N/A (DB internal) | Yes (Ord-based) | Yes (hash-based, unique treap) |
| Merkle commitment | Separate Merkle tree | Separate Merkle root | Implicit in tree root hash |
| Collection operations | DB queries | Standard Haskell | z-by/z-in engines (30+ arms each) |

The key differentiator: Nockchain's UTXO set is itself a cryptographically-ordered tree, not a separate database with a Merkle root bolt-on. The tree structure *is* the commitment.
