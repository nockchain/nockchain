# Extended UTXO: Note Data as On-Chain Datum

## Cardano eUTXO Recap

Cardano's extended UTXO (eUTXO) model extends Bitcoin's UTXO model with three additions:

1. **Datum**: Arbitrary data attached to each UTXO, available to validator scripts
2. **Redeemer**: Data provided by the spender to the validator when consuming a UTXO
3. **Script context**: The full transaction context available to validator scripts

The datum allows UTXOs to carry state, enabling stateful smart contracts within the UTXO paradigm — each UTXO becomes a "mini-state-machine" rather than just a value container.

## Nockchain's Note Data

V1 notes carry a `note-data` field — a z-map (persistent sorted map) from `@tas` symbol keys to arbitrary nouns. The authoritative Hoon definition:

```hoon
::  from hoon/common/tx-engine-1.hoon
::  note-data is a z-map of @tas keys to arbitrary noun values
note-data=(z-map @tas *)
```

Rust serialization mirror (represents the z-map as a list of key-value pairs with jammed noun blobs):

```rust
// crates/lib/nockchain-types/src/tx_engine/v1/note.rs:98-99
pub struct NoteData(pub Vec<NoteDataEntry>);

// crates/lib/nockchain-types/src/tx_engine/v1/note.rs:115-120
pub struct NoteDataEntry {
    pub key: String,       // @tas (Hoon "cord" / symbol string)
    pub blob: bytes::Bytes, // Jammed noun (arbitrary serialized Nock data)
}
```

### How Note Data Attaches to UTXOs

Note data flows from transaction outputs (seeds) to the notes they create:

```rust
// crates/lib/nockchain-types/src/tx_engine/v1/tx.rs:158-165
pub struct Seed {
    pub output_source: Option<Source>,
    pub lock_root: Hash,
    pub note_data: NoteData,    // Data for the new UTXO
    pub gift: Nicks,
    pub parent_hash: Hash,
}
```

When a transaction is applied to the balance:
1. Each seed creates a new V1 note
2. The seed's `note_data` becomes the note's `note_data`
3. The resulting `NoteV1` carries this data for its entire lifetime

```rust
// crates/lib/nockchain-types/src/tx_engine/v1/note.rs:58-65
pub struct NoteV1 {
    pub version: Version,
    pub origin_page: BlockHeight,
    pub name: Name,
    pub note_data: NoteData,    // Carried from the creating seed
    pub assets: Nicks,
}
```

### Noun Encoding

Note data entries use Hoon's `@tas` (text atom symbol) for keys and **jammed nouns** for values. A jammed noun is a compressed binary serialization of an arbitrary Nock value — it can represent any data structure expressible in Nock:

```rust
// crates/lib/nockchain-types/src/tx_engine/v1/note.rs:128-145
impl NounEncode for NoteData {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        self.0.iter().fold(D(0), |map, entry| {
            let mut key = make_tas(allocator, &entry.key).as_noun();
            // Cue (decompress) the jammed blob back into a noun
            let mut slab: NounSlab<NockJammer> = NounSlab::new();
            slab.cue_into(entry.blob.clone()).expect("failed to cue blob");
            let mut value = unsafe {
                let &root = slab.root();
                allocator.copy_into(root)
            };
            zmap::z_map_put(allocator, &map, &mut key, &mut value, &DefaultTipHasher)
                .expect("failed to encode note-data entry")
        })
    }
}
```

The data is stored on-chain as a z-map (persistent sorted map) of `(@tas, *)` — string keys to arbitrary nouns. This makes it a fully general key-value store per UTXO.

## Lock-Root Aggregation

A distinctive feature of Nockchain's note data model: **outputs (seeds) sharing the same lock root have their note-data merged**.

From `hoon/common/tx-engine-1.hoon`:

```hoon
++  note-data-by-lock-root
  |=  sps=form
  ^-  (z-mip ^hash @tas *)
  =/  all-seeds=(list seed)  ...
  =/  by-lock-root=(z-mip ^hash @tas *)
    %+  roll  all-seeds
    |=  [sed=seed acc=(z-mip ^hash @tas *)]
    =/  key=hash  lock-root.sed
    =/  existing=(unit (z-map @tas *))
      (~(get z-by acc) key)
    ?~  existing
      (~(put z-by acc) key note-data.sed)
    =/  merged=(z-map @tas *)
      (~(uni z-by u.existing) note-data.sed)
    (~(put z-by acc) key merged)
  by-lock-root
```

This means:
- If a transaction has three outputs to the same lock root, each with note-data `{a: 1}`, `{b: 2}`, `{c: 3}`, they merge into one note with `{a: 1, b: 2, c: 3}`
- Fee calculation charges once for the merged data, not three times
- Size validation (`max-size`) applies to the merged result

This is an unusual design choice with no direct equivalent in Bitcoin or Cardano. It enables efficient batch data attachment to a single spending authority.

## Size Limits

Bythos (Protocol 012) introduced per-output size validation for note data:

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

The size is measured by counting the number of leaves in the noun tree representation of the note-data map. This is checked per-output (after merging by lock-root), not per-seed.

The `max-size` limit is configured in `blockchain-constants.data.max-size` and provides protection against bloating the UTXO set with arbitrarily large datum.

## Comparison: Cardano Datum vs Nockchain Note Data

| Aspect | Cardano eUTXO Datum | Nockchain NoteData |
|---|---|---|
| Data type | Arbitrary Plutus Data (CBOR) | Arbitrary jammed nouns (key-value map) |
| Structure | Single datum per UTxO | Key-value map (`@tas → *`) per note |
| Attachment | Per output | Per output, merged by lock root |
| Size limits | Protocol parameter (max tx size) | `max-size` per merged output |
| Validator access | Full datum available to script | Note data available to Hoon validation |
| Inline vs reference | Both supported (Vasil) | Always inline |
| Hash commitment | Datum hash in output (pre-Vasil) | Note data included in note structure |
| Fee impact | Contributes to transaction size/fee | Contributes to seed word count for fee |
| Use cases | DeFi state, NFT metadata, oracle data | TBD (protocol-level data attachment) |

## Key Differences from Full eUTXO

Nockchain's note data is inspired by eUTXO but is **not a full implementation** of the Cardano model:

1. **No validator execution per UTXO**: Cardano runs Plutus validators when UTXOs are consumed; Nockchain's lock primitives (`Pkh`, `Tim`, `Hax`, `Burn`) are fixed opcodes, not arbitrary scripts. The note data is not consumed by a user-defined validator.

2. **No redeemer concept**: In Cardano, the spender provides a "redeemer" argument to the validator. Nockchain's closest equivalent is the `Witness` struct, but its fields are purpose-specific (signatures, preimages), not general-purpose redeemer data.

3. **No script context**: Cardano validators receive the full transaction context (all inputs, outputs, validity range, etc.). Nockchain's lock primitives check individual conditions without access to the full transaction.

4. **Key-value structure**: Unlike Cardano's single datum blob, Nockchain uses a structured key-value map, which provides native namespacing and avoids the need for application-level datum parsing.

The note data is best understood as a **data attachment mechanism** — a way to carry metadata alongside value — rather than a full programmable state machine. It provides the substrate for future extensions where lock primitives or validators could inspect note data during spending.

## Potential Applications

The note data mechanism enables:
- **Token metadata**: Attaching token names, symbols, or URIs to value-carrying notes
- **On-chain state**: Carrying application state that transitions with UTXO spending
- **Provenance tracking**: Recording the history or classification of value flows
- **Cross-chain data**: Carrying data needed for bridge or interoperability proofs
