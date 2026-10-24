# Taproot-Inspired Lock Merkle Proofs (MAST)

## Bitcoin Taproot Recap

Bitcoin Taproot (BIP 341/342, activated November 2021) introduced **Merkelized Abstract Syntax Trees (MAST)**: a way to commit to multiple spending scripts in a Merkle tree, where only the executed script branch is revealed on-chain.

Key Taproot concepts:
- **Key-path spending**: If all parties agree, the output can be spent with a single Schnorr signature (no script revealed)
- **Script-path spending**: Reveal one script from the MAST tree + Merkle proof to root
- **Privacy**: Unexecuted scripts remain hidden behind Merkle hashes
- **Efficiency**: Only the relevant branch is included in the transaction

## Nockchain's Lock Tree

Nockchain implements an analogous pattern through its **lock tree** and **lock Merkle proof** system. A lock in V1 is a binary tree of spend conditions, where each leaf is a `SpendCondition` (a list of lock primitives that must all be satisfied — AND logic). Different branches of the tree represent alternative ways to spend (OR logic).

### The Lock Tree Structure

```
            Lock Root
           /         \
      Branch A      Branch B
      /     \          |
   Leaf 1  Leaf 2   Leaf 3
   [pkh]   [pkh,    [tim,
            tim]     hax]
```

Each leaf is a `SpendCondition`: a list of `LockPrimitive` values that are ANDed together. The tree itself provides OR semantics — any one leaf (branch) can authorize spending.

This maps directly to Taproot's MAST:

| Taproot | Nockchain |
|---|---|
| Script tree | Lock tree |
| Script leaf | SpendCondition (list of LockPrimitive) |
| Control block | LockMerkleProof |
| Script-path spend | Spend1 with LockMerkleProof |
| Key-path spend | No direct equivalent (always reveals a branch) |

Notable difference: Nockchain does not have a key-path equivalent. Every spend must reveal at least one branch of the lock tree. This is a deliberate simplification — there is no "cooperative spend" shortcut that bypasses the Merkle proof.

## LockMerkleProof: The Control Block

When spending a V1 note, the spender provides a `lock-merkle-proof` that reveals exactly one branch of the lock tree. The authoritative Hoon type (from `tx-engine-1.hoon`):

```hoon
+$  lock-merkle-proof
  $^  :: stub (3-tuple):
      [=spend-condition axis=@ =merk-proof:merkle]
      :: full (4-tuple with %full tag):
      [version=%full =spend-condition axis=@ =merk-proof:merkle]
```

Rust serialization mirror:

```rust
// crates/lib/nockchain-types/src/tx_engine/v1/tx.rs:339-344
pub enum LockMerkleProof {
    Full(LockMerkleProofFull),  // Post-Bythos: axis committed in hash
    Stub(LockMerkleProofStub),  // Pre-Bythos: axis not committed
}
```

### Stub Format (Pre-Bythos)

The stub is a 3-tuple `[spend-condition axis merk-proof]`. The axis is not committed in the hash.

### Full Format (Post-Bythos)

The full format is a 4-tuple `[%full spend-condition axis merk-proof]`. The `%full` tag distinguishes it from stub proofs via structural discrimination (a standard Hoon `$^` pattern).

The difference: Full proofs include the `axis` in the hashable commitment. In Stub proofs, the axis was replaced by a hardcoded placeholder hash, meaning the witness hash did not commit to *which branch* was being executed — a weakness fixed by Bythos.

## The Axis: Binary Tree Addressing

The `axis` field is a Nock-native concept for addressing positions in binary trees. In Nock, every value is a binary tree (a "noun"), and tree positions are addressed by axes:

```
        1 (root)
       / \
      2    3
     / \  / \
    4  5 6   7
```

- Axis 1: root
- Axis 2: left child
- Axis 3: right child
- Axis 4: left-left
- Axis 5: left-right
- etc.

When a lock tree has multiple branches, the axis tells the validator which leaf position the provided `SpendCondition` occupies. The Merkle proof then demonstrates that this leaf, at this axis, hashes up to the lock root committed in the note's Name.

## Merkle Proof Structure

```rust
// crates/lib/nockchain-types/src/tx_engine/v1/tx.rs:408-412
pub struct MerkleProof {
    pub root: Hash,        // The expected root hash (matches Name.first)
    pub path: Vec<Hash>,   // Sibling hashes from leaf to root
}
```

The proof contains:
1. **root**: the expected Merkle root (must match the lock root hash stored in the note's Name)
2. **path**: a list of sibling hashes, one per level from the leaf up to the root

Verification proceeds by:
1. Hash the spend condition (the revealed leaf)
2. At each level, combine with the sibling hash from the path
3. Check that the result equals the declared root
4. Check that the declared root matches the note's lock root (committed in `Name.first`)

## Hashable Construction

The hash commitment for a lock Merkle proof is computed differently for stub and full formats.

### Stub Hashable (Pre-Bythos)

From `docs/protocol/upgrades/012-bythos.md`:

```hoon
:+  hash+(hash:spend-condition spend-condition.form)
    hash+(from-b58:^hash '6mhCSwJQDvbkbiPAUNjetJtVoo1VLtEhmEYoU4hmdGd6ep1F6ayaV4A')
  (hashable-merk-proof merk-proof.form)
```

The second element is a **hardcoded hash** — a static placeholder that does not depend on the axis. This means two different branches of the same lock tree would produce the same witness hash if their spend conditions happened to hash identically.

### Full Hashable (Post-Bythos)

```hoon
:*  leaf+version.form
    hash+(hash:spend-condition spend-condition.form)
    leaf+axis.form
    (hashable-merk-proof merk-proof.form)
==
```

The full format includes `axis` as a leaf in the hashable tree, ensuring the witness hash commits to the specific branch being executed. This is a stronger security property — the witness is now bound to a particular position in the lock tree.

## Height-Gated Proof Format Selection

The proof format is determined by the note's origin page:

```hoon
=/  parent-lmp=lock-merkle-proof
  ?:  (gte origin-page.note bythos-phase)
    (build-lock-merkle-proof-full:lock parent-lock 1)
  (build-lock-merkle-proof-stub:lock parent-lock 1)
```

- Notes created **before Bythos** (origin page < 54000): use stub proofs
- Notes created **at/after Bythos** (origin page ≥ 54000): use full proofs

The validator accepts both formats but gates full proof acceptance on the Bythos activation height.

## Privacy Properties

Like Taproot, Nockchain's lock Merkle proofs provide **partial script privacy**:

- **Revealed**: The spend condition being exercised (one branch of the lock tree)
- **Hidden**: All other branches remain behind Merkle hashes
- **Observable**: The number of tree levels (depth) can hint at the number of branches, but the exact count remains ambiguous

For a lock tree with N branches:
- The spender reveals 1 spend condition + log2(N) sibling hashes
- The remaining N-1 conditions are hidden

This is the same privacy model as Bitcoin Taproot script-path spends.

## Comparison: Bitcoin Taproot vs Nockchain Lock Merkle Proofs

| Aspect | Bitcoin Taproot | Nockchain Lock Merkle Proof |
|---|---|---|
| Script tree | MAST of TapScript leaves | Binary tree of SpendConditions |
| Key-path spend | Yes (single Schnorr sig, no script revealed) | No (always reveal a branch) |
| Script-path spend | Control block + script + Merkle proof | LockMerkleProof (spend condition + axis + proof) |
| Branch addressing | Control block leaf version + script | Axis (Nock binary tree address) |
| Proof structure | Internal key + parity + Merkle path | Root hash + sibling hash path |
| Hash function | SHA-256 (tagged) | Tip5 |
| Privacy | Unrevealed scripts hidden | Unrevealed conditions hidden |
| Commitment strength | Axis committed via control block | Stub: axis not committed; Full: axis committed |
| Composability | TapScript opcodes | AND within SpendCondition, OR across branches |
| Activation | BIP 9 signaling soft fork | Height-gated hard cutover |
