# Fee Structure: SegWit-Inspired Weight Discounting

## Bitcoin SegWit Fee Model Recap

Bitcoin SegWit introduced a **virtual weight** system where witness data is discounted at a 4:1 ratio:

- Non-witness data: 4 weight units per byte
- Witness data: 1 weight unit per byte
- Block limit: 4,000,000 weight units (effectively ~1MB non-witness + ~3MB witness)

The rationale: witness data does not contribute to the UTXO set and is only needed for validation, so it should cost less than output data that persists until spent.

## Nockchain's Fee Model

Nockchain's fee model evolved through two phases, ultimately arriving at the same principle: **witness (input) data should cost less than output data**.

All fee logic is defined in Hoon (`hoon/common/tx-engine-1.hoon`), which is the authoritative source. The Rust types in `crates/lib/nockchain-types/` carry the `BlockchainConstants` structure for deserialization but do not implement fee calculation.

### Pre-Bythos Fee Model (Before Block 54000)

Before Bythos, fees were calculated uniformly:

```
min_fee = max(
  (seed_words + witness_words) * (2 * base_fee),
  min_fee_constant
)
```

Where:
- `base_fee = 2^15 = 32768` (configured, but doubled in practice)
- `min_fee = 256`
- All transaction words charged at the same rate

### Post-Bythos Fee Model (Block 54000+)

Bythos (Protocol 012) introduced differential pricing, directly analogous to SegWit's weight discount:

```
min_fee = max(
  (seed_words * base_fee) + (witness_words * base_fee / input_fee_divisor),
  min_fee_constant
)
```

Where:
- `base_fee = 2^14 = 16384` (halved from pre-Bythos effective rate)
- `input_fee_divisor = 4`
- `min_fee = 256`

The Hoon implementation from `tx-engine.hoon` (the versioned facade, line 916):

```hoon
++  calculate-min-fee
  |=  [sps=form page-num=page-number]
  ^-  coins
  =/  bythos-active=?  (gte page-num bythos-phase)
  ::  bythos halves base-fee at activation; pre-bythos uses legacy 2x rate
  =/  effective-base-fee=coins
    ?:(bythos-active base-fee (mul 2 base-fee))
  =/  seed-word-count=@  (count-seed-words [sps page-num])
  =/  witness-word-count=@  (count-witness-words [sps page-num])
  ::  inputs pay discounted fee only at/after bythos activation
  =/  witness-divisor=@  ?:(bythos-active input-fee-divisor 1)
  ::  outputs (seeds) pay full effective-base-fee per word
  =/  seed-fee=coins  (mul seed-word-count effective-base-fee)
  ::  inputs (witnesses) pay effective-base-fee / input-fee-divisor per word
  =/  witness-fee=coins  (div (mul witness-word-count effective-base-fee) witness-divisor)
  =/  word-fee=coins  (add seed-fee witness-fee)
  (max word-fee min-fee.data)
```

## Blockchain Constants

Fee parameters are part of the `blockchain-constants` structure defined in Hoon. The Rust side mirrors these for deserialization:

| Constant | Mainnet Value | Fakenet Value | Purpose |
|---|---|---|---|
| `base-fee` | 16384 (2^14) | 128 | Per-word fee rate for outputs |
| `input-fee-divisor` | 4 | 4 | Witness discount factor |
| `min-fee` | 256 | 256 | Absolute fee floor |
| `max-size` | (configured) | (configured) | Max note-data size per output |
| `bythos-phase` | 54000 | 54000 | Activation height for new fee model |

## Word Counting

Fees are denominated in "words" — the number of Nock noun tree nodes in the serialized transaction structure. This is Nock-native sizing: rather than counting bytes (as Bitcoin does), Nockchain counts the structural complexity of the noun representation.

Two separate word counts are computed:
- **Seed words**: counted from the outputs (seeds), representing data that creates new UTXOs
- **Witness words**: counted from the inputs (witnesses), representing authentication data consumed during validation

### Note-Data Accounting

Seed word counting includes note-data, but with an important optimization: **note-data maps for outputs sharing the same lock root are merged first**, and the merged map's leaf count is charged once.

From `tx-engine-1.hoon`:

```hoon
++  note-data-by-lock-root
  |=  sps=form
  ^-  (z-mip ^hash @tas *)
  ...
  :: merges note-data from all seeds with the same lock-root
```

This prevents double-charging when multiple outputs to the same lock root carry overlapping data.

## Comparison: Bitcoin SegWit Weight vs Nockchain Fee Formula

| Aspect | Bitcoin SegWit | Nockchain (Post-Bythos) |
|---|---|---|
| Measurement unit | Bytes → weight units | Noun words |
| Output data rate | 4 WU per byte | `base_fee` per word |
| Witness data rate | 1 WU per byte | `base_fee / 4` per word |
| Discount ratio | 4:1 | 4:1 (configurable via `input-fee-divisor`) |
| Fee floor | Dust relay minimum | `min_fee = 256` |
| Activation | Soft fork (BIP 141) | Height-gated (block 54000) |
| Rationale | Witness data doesn't grow UTXO set | Inputs are consumed; outputs persist |

The 4:1 discount ratio is identical to Bitcoin SegWit. The rationale is the same: outputs create new UTXOs that the network must store until spent, while input/witness data is validated once and discarded. Discounting witness data incentivizes spending (consuming UTXOs) over creating new ones.

## Fee Transition Mechanics

The Bythos upgrade handled the fee transition smoothly:

1. **Before block 54000**: `effective_base_fee = 2 * base_fee = 2 * 16384 = 32768`, `witness_divisor = 1`
   - Net effect: `(all_words) * 32768`
2. **At/after block 54000**: `effective_base_fee = base_fee = 16384`, `witness_divisor = 4`
   - Net effect: `(seed_words * 16384) + (witness_words * 4096)`

For a typical transaction where witness data is ~50% of total words:
- Pre-Bythos: `100 words * 32768 = 3,276,800`
- Post-Bythos: `50 * 16384 + 50 * 4096 = 819,200 + 204,800 = 1,024,000`
- ~69% fee reduction for typical transactions

This mirrors the practical effect of Bitcoin SegWit, where SegWit transactions paid significantly lower fees than legacy transactions.
