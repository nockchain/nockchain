# SegWit-Inspired Witness Separation

## Bitcoin SegWit Recap

Bitcoin's Segregated Witness (BIP 141, activated August 2017) introduced a fundamental structural change: **signature (witness) data was moved out of the transaction body** into a separate witness structure. This achieved three goals:

1. **Transaction malleability fix**: Transaction IDs no longer depend on signatures, preventing third parties from modifying txids by tweaking signatures.
2. **Fee discount**: Witness data is counted at 1/4 weight, incentivizing efficient use of block space.
3. **Script versioning**: A new `witness_version` byte enabled future soft-fork upgrades (which led to Taproot).

## Nockchain's Witness Separation

Nockchain's V1 transaction engine (Protocol 009, activated at block 37350) implements an analogous separation. The design is documented as the "segwit cutover" in `docs/protocol/upgrades/009-legacy-segwit-cutover-initial.md`.

### V0 Model: Embedded Signatures

In V0, spending proofs were embedded directly in the input. The authoritative Hoon type definition:

```hoon
::  from hoon/common/tx-engine-0.hoon
++  input   [note=nnote =spend]
++  spend   $:  signature=(unit signature)
                =seeds
                fee=coins
            ==
```

Rust serialization mirror:

```rust
// crates/lib/nockchain-types/src/tx_engine/v0/tx.rs:51-54
pub struct Input {
    pub note: NoteV0,     // The full UTXO being spent (embedded!)
    pub spend: Spend,     // Contains signature directly
}
```

Problems with this model:
- The full note is embedded in each input, duplicating data already in the UTXO set
- Signatures are part of the core transaction structure, complicating ID computation
- No extensibility path for new authentication mechanisms

### V1 Model: Separated Witness

V1 introduces a clean separation. The authoritative Hoon type definitions:

```hoon
::  from hoon/common/tx-engine-1.hoon
++  spend
  $%  [%0 =signature =seeds fee=coins]
      [%1 =witness =seeds fee=coins]
  ==
```

Rust serialization mirror:

```rust
// crates/lib/nockchain-types/src/tx_engine/v1/tx.rs:90-94
pub enum Spend {
    Legacy(Spend0),    // Tag %0: bridge spend (v0 note → v1 output)
    Witness(Spend1),   // Tag %1: native v1 spend with separate witness
}
```

#### Spend0: Legacy Bridge

`Spend0` (tag `%0`) exists solely to allow v0 notes (created before the cutover) to be spent under the v1 engine. It carries a direct signature, just like v0, but produces v1 outputs. It is a **bridge mechanism** — once all v0 notes are spent, Spend0 becomes unnecessary.

This is analogous to Bitcoin's P2SH-wrapped SegWit outputs, which allowed SegWit transactions to be sent from non-SegWit-aware wallets during the transition.

#### Spend1: Native Witness

`Spend1` (tag `%1`) is the native V1 spend with a separated witness. The Hoon type:

```hoon
::  The witness type (authoritative definition from tx-engine-1.hoon)
++  witness
  $:  =lock-merkle-proof   :: which branch of the lock tree
      =pkh-signature        :: Schnorr signature proofs
      hax=(z-map hash *)    :: hash preimage reveals
      tim=@                 :: reserved (always 0)
  ==
```

Rust serialization mirror:

```rust
// crates/lib/nockchain-types/src/tx_engine/v1/tx.rs:167-174
pub struct Witness {
    pub lock_merkle_proof: LockMerkleProof,
    pub pkh_signature: PkhSignature,
    pub hax: Vec<HaxPreimage>,
    pub tim: usize,
}
```

Each witness field corresponds to a lock primitive type:
- `lock_merkle_proof` → proves which spend condition branch is being exercised
- `pkh_signature` → satisfies `Pkh` (public-key-hash) lock primitives
- `hax` → satisfies `Hax` (hash preimage) lock primitives
- `tim` → reserved for `Tim` (timelock) witness data (currently unused; timelocks are checked against block context)

## Structural Comparison

### V0 Transaction Layout

```
RawTx
├── id: TxId
├── inputs: z-map
│   └── (Name → Input)
│       ├── note: NoteV0          ← full UTXO embedded
│       └── spend
│           ├── signature         ← auth proof embedded in spend
│           ├── seeds             ← outputs
│           └── fee
├── timelock_range
└── total_fees
```

### V1 Transaction Layout

```
RawTx
├── version: 1
├── id: TxId
└── spends: z-map
    └── (Name → Spend)
        └── Spend::Witness(Spend1)
            ├── witness           ← SEPARATED authentication proof
            │   ├── lock_merkle_proof
            │   ├── pkh_signature
            │   ├── hax
            │   └── tim
            ├── seeds             ← outputs
            └── fee
```

Key structural differences:
1. **No embedded note**: V1 references notes by Name only; the validator looks them up in the balance
2. **Witness separated**: Authentication data has its own structured container
3. **Extensible witness**: The Witness struct has dedicated fields per lock primitive type, allowing independent evolution
4. **Version tagged**: The RawTx carries an explicit version number

## Height-Gated Activation

The cutover is enforced by a strict height-based rule matrix:

| Block Height | V0 Transactions | V1 Transactions | Coinbase Format |
|---|---|---|---|
| `< v1-phase` (37350) | Allowed | Rejected | V0 (sig-keyed) |
| `≥ v1-phase` (37350) | Rejected | Required | V1 (lock-hash-rooted) |

From `docs/protocol/upgrades/009-legacy-segwit-cutover-initial.md`:
> At `height >= v1-phase`: v1 raw transactions are required. v0 raw transactions are rejected (`%v0-tx-after-cutoff`).

This is a **hard cutover**, not a gradual transition. All nodes must agree on the same rules at the same height.

## Bridge Spending: V0 → V1 Transition

When a V0 note needs to be spent after the cutover, it uses `Spend0`:

1. The spender references the V0 note by its Name
2. Provides a V0-style signature (directly, not via witness)
3. The outputs (seeds) are V1-format, producing V1 notes

The Hoon validation logic checks spend-version compatibility:

```hoon
:: From hoon/common/tx-engine-1.hoon (validate-with-context)
::  v0 note must back a %0 spend
?:  ?=(@ -.note)  [%.n %v1-spend-version-mismatch]
```

- V0 notes (head is a cell) → must use Spend0
- V1 notes (head is an atom) → must use Spend1

This ensures that the more expressive V1 witness/lock system is only used with V1 notes that were created with lock tree commitments.

## Transaction ID Computation

In V1, the transaction ID (`TxId`) is computed as a hash of the full spend data including witnesses. This differs from Bitcoin SegWit, where the `txid` excludes witness data (and a separate `wtxid` includes it).

However, the **sig-hash** (the message that gets signed) excludes the witness:

```rust
// The sig_hash is computed over (seeds, fee) — not the witness
// This is analogous to Bitcoin's sighash, which signs the transaction
// structure without the signatures themselves
```

This means:
- The transaction ID is stable once all witnesses are attached
- The signing message doesn't include signatures (avoiding circular dependency)
- Unlike Bitcoin SegWit, there is no separate "witness txid" — just one ID

## Fee Implications

The witness separation enabled the Bythos upgrade (Protocol 012) to introduce **differential fee pricing** for witness vs non-witness data, directly analogous to Bitcoin SegWit's weight system:

- **Seed words** (outputs): charged at full `base_fee` per word
- **Witness words** (inputs): charged at `base_fee / input_fee_divisor` (1/4 rate)

See [06-fee-structure.md](06-fee-structure.md) for the full fee analysis.

## Comparison: Bitcoin SegWit vs Nockchain V1

| Aspect | Bitcoin SegWit | Nockchain V1 |
|---|---|---|
| Activation | BIP 9 signaling (soft fork) | Height-gated hard cutover |
| Witness location | Separate witness field in serialization | `Witness` struct in `Spend1` |
| Transaction ID | `txid` excludes witness; `wtxid` includes | Single ID includes all data |
| Signature exclusion | Witness stripped for txid | Witness excluded from sig-hash |
| Fee discount | 4:1 witness weight ratio | 4:1 input fee divisor (Bythos) |
| Legacy compatibility | P2SH-wrapped SegWit | Spend0 bridge spends |
| Script versioning | witness_version byte | Spend enum tag (0 or 1) |
| Extensibility | Led to Taproot (v1) | Led to lock trees + note data |
