# Nockchain Transaction Engine: Architecture Overview

## Purpose

The Nockchain transaction engine (tx-engine) is the consensus-critical subsystem that defines how value is created, transferred, and validated on the Nockchain network. It implements a UTXO-based model with distinct design inspirations drawn from three pillars of blockchain innovation:

- **Bitcoin Segregated Witness (SegWit)** — witness data separation from transaction structure
- **Bitcoin Taproot (BIP 341/342)** — Merkelized script trees with selective branch revelation
- **Extended UTXO (eUTXO)** — arbitrary data attached to UTXOs, as pioneered by Cardano

The engine is distinctive in that it spans two languages and runtimes: **Hoon** (running on the Nock VM) is the **source of truth** for all type definitions and consensus validation logic, while **Rust** provides serialization/deserialization of Hoon nouns for networking, APIs, and wallet applications.

## Design Lineage

| Nockchain Concept | Bitcoin Equivalent | Cardano eUTXO Equivalent |
|---|---|---|
| Note | UTXO (TxOut) | UTxO |
| Name (first, last) | Outpoint (txid:vout) | TxOutRef |
| Lock (v0: M-of-N keys) | P2SH / P2PKH | — |
| Lock tree (v1) | Taproot script tree (MAST) | — |
| LockMerkleProof | Taproot control block + script | — |
| Spend0 (legacy bridge) | SegWit P2SH-wrapped spend | — |
| Spend1 (witness-based) | SegWit native spend | — |
| Witness struct | SegWit witness field | Redeemer |
| NoteData | — | Datum |
| SpendCondition (AND list) | Script execution | Validator |
| Lock primitives (Pkh/Tim/Hax/Burn) | OP_CHECKSIG / OP_CLTV / OP_HASH / OP_RETURN | — |
| Seed | TxOut (output) | TxOut |
| Nicks / Nocks | Satoshis / BTC | Lovelace / ADA |
| Tip5 hash | SHA-256d | Blake2b |
| z-map / z-set | — | — (Nock-native) |

## Codebase Map

### Hoon: Source of Truth (Type Definitions + Validation)

```
hoon/common/
├── tx-engine.hoon      # Versioned facade: dispatches to tx-engine-0 or tx-engine-1
├── tx-engine-0.hoon    # V0 engine: type definitions, validation, balance updates (~2471 lines)
└── tx-engine-1.hoon    # V1 engine: type definitions, witness checks, lock merkle proofs,
                        # fee calculation, context-aware validation (~1969 lines)
```

### Rust: Serialization/Deserialization Mirror (for Networking & Applications)

```
crates/lib/nockchain-types/src/tx_engine/
├── common/
│   ├── mod.rs          # Hash, Name, Nicks, Version, Source, SchnorrPubkey/Signature, timelocks
│   └── page.rs         # Page (block), BlockId, CoinbaseSplit, PageMsg
├── v0/
│   ├── tx.rs           # V0 RawTx, Input, Spend, Seeds, Seed
│   └── note.rs         # NoteV0, Lock (M-of-N), Balance, Timelock
├── v1/
│   ├── tx.rs           # V1 RawTx, Spend enum (Legacy/Witness), Spend0, Spend1, Witness,
│   │                   # LockMerkleProof, SpendCondition, LockPrimitive, Pkh, Tim, Hax, Seeds, Seed
│   └── note.rs         # NoteV1, NoteData, NoteDataEntry, Balance, Note enum (V0/V1)
└── mod.rs              # Module root
```

### Protocol Specifications

```
docs/protocol/upgrades/
├── 009-legacy-segwit-cutover-initial.md  # V0→V1 split (block 37350)
├── 010-legacy-v1-phase-39000.md          # V1 phase finalization
├── 011-legacy-lmp-axis-hotfix.md         # Lock merkle proof axis fix
├── 012-bythos.md                         # LMP versioning + fee rebalancing (block 54000)
└── 013-nous.md                           # Networking upgrade (non-tx-engine)
```

## Version History

```
Genesis ──── Block 0
   │         V0 engine only: M-of-N multisig, sig-keyed coinbase
   │
Protocol 009 ── Block 37350 (v1-phase)
   │         Dual engine: V0 rejected, V1 required
   │         Witness separation (Spend0 bridge, Spend1 native)
   │         Lock trees with Merkle proofs
   │         Note data (eUTXO datum)
   │         Fee floor with word-count pricing
   │
Protocol 010 ── Block 39000
   │         V1-phase boundary finalized
   │
Protocol 012 (Bythos) ── Block 54000
   │         Lock merkle proof versioning (stub → full)
   │         Fee rebalancing: witness discount (1/4 rate)
   │         Context-aware mempool admission
   │
Present
```

## Analysis Index

| Document | Topic |
|---|---|
| [01 — UTXO Model](01-utxo-model.md) | Notes, Names, Locks, balances, and asset denomination |
| [02 — SegWit Witness Separation](02-segwit-witness-separation.md) | V1 witness separation, Spend0/Spend1, bridge spends |
| [03 — Taproot Lock Merkle Proofs](03-taproot-lock-merkle-proofs.md) | MAST-like lock trees, selective branch revelation |
| [04 — eUTXO Note Data](04-eutxo-note-data.md) | On-chain datum via NoteData |
| [05 — Lock Primitives](05-lock-primitives-script-model.md) | Pkh, Tim, Hax, Burn and composition model |
| [06 — Fee Structure](06-fee-structure.md) | SegWit-inspired witness weight discount |
| [07 — Validation Pipeline](07-transaction-validation-pipeline.md) | End-to-end transaction validation |
| [08 — Protocol Evolution](08-protocol-evolution.md) | Upgrade mechanics and history |
| [09 — Noun Encoding](09-noun-encoding-data-layer.md) | Rust↔Hoon bridge via noun serialization |
| [10 — Zoon Persistent Data Structures](10-zoon-persistent-data-structures.md) | Hash-ordered z-maps and z-sets (cryptographic treaps) |
| [11 — Tip5 Hash Function](11-tip5-hash-function.md) | Algebraic sponge hash over Goldilocks field |
| [12 — Schnorr Signatures & Cheetah Curve](12-schnorr-signatures-cheetah-curve.md) | STARK-friendly signatures over Fp^6 extension |
| [13 — Merkle Trees & Commitments](13-merkle-trees-and-commitments.md) | Merkle proof construction and hashable commitment trees |
| [14 — Goldilocks Field Arithmetic](14-goldilocks-field-arithmetic.md) | Base and extension field types (ztd one and two) |
| [15 — ZTD STARK Proof Stack](15-ztd-stark-proof-stack.md) | Full STARK prover hierarchy (ztd three through eight) |
| [16 — ZK PoW Proof Data Encoding](16-zk-pow-proof-data-encoding.md) | Proof transcript objects, packed polynomials, and canonical `%poly` encoding |
