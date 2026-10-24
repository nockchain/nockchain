# Tip5: The Sponge Hash Function

## Overview

Tip5 is the cryptographic hash function used throughout Nockchain for commitments, Merkle trees, data structure ordering, and transaction IDs. It belongs to the family of **algebraic sponge hashes** (alongside Poseidon, Neptune, and Rescue) — hash functions designed to be efficient both natively and inside zero-knowledge proof circuits.

The authoritative implementation is in Hoon (`hoon/common/ztd/three.hoon`), with a Rust jet in `crates/lib/nockchain-math/src/tip5/`.

## Design Parameters

| Parameter | Value | Meaning |
|---|---|---|
| Field | Goldilocks, p = 2^64 − 2^32 + 1 | Each state element is a 64-bit field element |
| State size | 16 elements | Total permutation width |
| Capacity | 6 elements | Security margin (never directly exposed) |
| Rate | 10 elements | Input absorbed per permutation |
| Rounds | 7 | Number of permutation rounds |
| Digest length | 5 elements (320 bits) | Output size |
| S-box (first 4) | Lookup table (Fermat map) | x → x^(p−2) via 256-byte table |
| S-box (last 12) | 7th power map | x → x^7 in 4 multiplications |

## Sponge Construction

Tip5 uses the standard sponge construction:

```
Input: [a₀, a₁, ..., aₙ]

1. Pad input: append [1, 0, 0, ...] to reach a multiple of RATE (10)
2. Convert to Montgomery form (montify each element)
3. Initialize sponge state (16 elements)
4. For each RATE-sized chunk:
   a. XOR chunk into the rate portion of the state
   b. Apply permutation (7 rounds)
5. Extract first 5 elements as digest
6. Convert back from Montgomery form
```

Two initialization modes exist:
- **Variable-length** (`hash-varlen`): for arbitrary-length inputs
- **Fixed-length** (`hash-10`): optimized for exactly 10 elements (one rate block)

## Permutation Round Structure

Each of the 7 rounds applies three layers:

### 1. S-box Layer

The S-box provides non-linearity. It applies two different maps depending on position:

```hoon
++  sbox-layer
  ~/  %sbox-layer
  |=  =state
  ?>  =((lent state) state-size)
  %+  weld
    (turn (scag num-split-and-lookup state) split-and-lookup)  :: first 4
  %+  turn  (slag num-split-and-lookup state)                   :: last 12
  |=  m=melt
  =/  sq  (bmul m m)      :: x²
  =/  qu  (bmul sq sq)    :: x⁴
  :(bmul m sq qu)         :: x⁷ = x · x² · x⁴
```

- **First 4 elements**: `split-and-lookup` — decomposes the element into bytes, applies a 256-byte lookup table (implementing the Fermat map x → x^(p−2) ≡ x^(−1)), then recombines
- **Last 12 elements**: 7th power map — computes x^7 using 4 field multiplications (x → x² → x⁴ → x·x²·x⁴)

The hybrid S-box design is a performance optimization: the lookup-table-based Fermat map is faster for the first few elements, while the power map avoids the table dependency for the remaining elements.

### 2. MDS Layer

Linear mixing via a 16×16 circulant MDS (Maximum Distance Separable) matrix:

```hoon
++  mds-cyclomul-m
  ~/  %mds-cyclomul-m
  |=  v=(list @)
  ^-  (list @)
  %+  turn  mds-matrix
  |=  row=(list @)
  (mod (inner-product row v) p)
```

The MDS matrix ensures maximum diffusion — every output element depends on every input element. The circulant structure allows optimization via the inner product computation.

### 3. Round Constant Addition

Each round adds a unique set of 16 precomputed constants:

```hoon
++  round
  ~/  %round
  |=  [sponge=tip5-state round-index=@]
  =.  sponge  (mds-cyclomul-m (sbox-layer sponge))
  %^  zip  sponge  (range state-size)
  |=  [b=belt i=@]
  (badd b (snag (add (mul round-index state-size) i) round-constants))
```

Total round constants: 7 rounds × 16 elements = 112 precomputed Goldilocks field elements.

## Montgomery Representation

All permutation arithmetic operates in **Montgomery space** for efficiency:

- `melt`: a field element in Montgomery form (multiplied by R = 2^64 mod p)
- `montify`: convert belt → melt (multiply by R mod p)
- `mont-reduction`: convert melt → belt (multiply by R^(-1) mod p)
- `montiply`: Montgomery multiplication — `a * b * R^(-1) mod p` in a single operation

Montgomery multiplication avoids expensive division-based modular reduction by using a shift-based reduction, roughly 4× faster for repeated multiplications.

## Hash Variants

### hash-10: Fixed 10-Element Input

```hoon
++  hash-10
  ~/  %hash-10
  |=  input=(list belt)
  ^-  (list belt)
  ?>  =((lent input) rate)
  ?>  (levy input based)
  =.  input   (turn input montify)
  =/  sponge  (init-tip5-state %fixed)
  =.  sponge  (permutation (weld input (slag rate sponge)))
  (turn (scag digest-length sponge) mont-reduction)
```

Optimized path for exactly 10 elements. Used by `double-tip` in zoon (hashes two 5-element digests together).

### hash-varlen: Variable-Length Input

Pads input, absorbs in rate-sized chunks, returns 5-element digest. Used for hashing arbitrary-length data.

### hash-noun-varlen: Noun Hashing

Hashes an arbitrary Nock noun by extracting its **leaf sequence** (atoms at the leaves of the binary tree) and **Dyck word** (the tree structure encoding), then feeding both through the sponge. This ensures that structurally different nouns produce different hashes even if they contain the same atoms.

### hash-hashable: Commitment Hashing

Hashes a tagged `hashable` tree structure. The `hashable` type is:

```hoon
+$  hashable
  $%  [%leaf p=@]               :: raw value
      [%hash p=noun-digest]     :: pre-computed hash
      [%list p=(list hashable)] :: list of hashables
  ==
```

Transaction engine types define `++hashable` arms that build these trees, which `hash-hashable` then traverses to produce deterministic commitments. For example, the lock-merkle-proof-full hashable:

```hoon
:*  leaf+version.form
    hash+(hash:spend-condition spend-condition.form)
    leaf+axis.form
    (hashable-merk-proof merk-proof.form)
==
```

### Tog: Deterministic PRNG

The `tog` interface provides deterministic pseudorandom generation from a sponge state:

- `belts`: generate N random belt (field element) values
- `felt`: generate a random felt (extension field element)
- `indices`: generate random indices for spot checks

Used in the STARK proof system for Fiat-Shamir challenges.

## Rust Jet Implementation

The Rust jet mirrors the Hoon implementation exactly:

**Constants** (`crates/lib/nockchain-math/src/tip5/mod.rs`):
- `LOOKUP_TABLE`: 256-byte S-box for the Fermat map
- `ROUND_CONSTANTS`: 112 `u64` values (7 rounds × 16 state elements)
- `MDS_MATRIX_MONT`: 16×16 matrix in Montgomery representation

**Functions** (`crates/lib/nockchain-math/src/tip5/hash.rs`):
- `permute(sponge: &mut [u64; 16])`: applies 7 rounds in-place
- `hash_varlen(input: &[u64]) -> [u64; 5]`: variable-length hash
- `hash_10(input: &[u64; 10]) -> [u64; 5]`: fixed 10-element hash
- `hash_noun_varlen(noun: &Noun) -> [u64; 5]`: noun hash

## Usage in the Transaction Engine

Tip5 is used at every layer of the tx-engine:

| Usage | Function | Input |
|---|---|---|
| Transaction ID | `hash-hashable` | Hashable tree of spends |
| Lock root | `hash:lock` | Lock tree structure |
| Note Name.first | `hash:lock` | Lock root of the spending conditions |
| Note Name.last | derived | Source hash (parent transaction) |
| Merkle proof siblings | `hash-ten-cell` | Pair of sibling hashes |
| z-map/z-set ordering | `hash-noun-varlen` | Element nouns (via `tip` in zoon) |
| z-map/z-set priority | `hash-ten-cell` + `hash-noun-varlen` | Double-hash (via `double-tip`) |
| PKH commitment | `hash` | Public key → public key hash |
| Hax commitment | `hash` | Preimage → commitment hash |
| Block commitment | `hash-hashable` | Page/block contents |

## Comparison with Other Blockchain Hash Functions

| Hash | Blockchain | Field | Design | ZK-Friendly |
|---|---|---|---|---|
| SHA-256d | Bitcoin | Binary (256-bit) | Merkle-Damgård | No |
| Keccak-256 | Ethereum | Binary (256-bit) | Sponge (binary) | No |
| Poseidon | Mina, Filecoin | Various prime fields | Algebraic sponge | Yes |
| Rescue-Prime | Various | Various prime fields | Algebraic sponge | Yes |
| Tip5 | Nockchain | Goldilocks (64-bit) | Algebraic sponge | Yes |

### Why Tip5 Matters for STARK Compatibility

Traditional hash functions like SHA-256 and Keccak operate on bits/bytes. Verifying them inside a STARK circuit requires encoding binary operations as algebraic constraints — extremely expensive (thousands of constraints per hash).

Tip5 operates natively on Goldilocks field elements — the same field used for STARK arithmetic. This means:
- Hashing inside a STARK circuit requires only ~7 × 16 = 112 field operations per permutation
- No binary-to-field conversion overhead
- The S-box, MDS matrix, and round constants all operate in the native field
- Merkle proof verification in-circuit is practical, enabling efficient recursive proofs
