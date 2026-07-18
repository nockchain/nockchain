# `nockchain-types`

`nockchain-types` provides Rust mirrors of Nockchain consensus data: blockchain constants, transaction-engine objects, pages/blocks, notes, locks, signatures, and bridge/Ethereum-facing types.

## Place in the system

Hoon remains the protocol authority. This crate lets Rust services, wallets, tests, networking code, and miners construct and inspect the same noun shapes without duplicating slot arithmetic at every call site.

- `blockchain_constants` mirrors the kernel configuration and mainnet/fakenet defaults.
- `tx_engine` mirrors transaction, page, note, lock, and proof objects.
- `eth` contains Base/Ethereum bridge-facing representations.

## Maintained invariants

- `NounEncode` and `NounDecode` preserve the exact Hoon mold, list shape, optional encoding, field order, atom endianness, and version discriminator.
- Versioned objects decode through an explicit version branch. Unknown or malformed variants fail rather than falling back to a nearby shape.
- Mainnet defaults match the corresponding Hoon constants. Fakenet overrides are explicit and must preserve coupled consensus boundaries.
- Block proof variants remain distinguishable. ZK-PoW and AI-PoW targets, artifacts, and activation data cannot be inferred from a shared untagged payload.
- Numeric conversion is checked at Rust boundaries; oversized nouns do not truncate into consensus values.
- Rust convenience methods do not create an alternative validation path. A successfully decoded object still requires kernel validation.

## Consensus and cryptographic dependencies

Consensus safety relies on exact Rust/Hoon representation parity, canonical JAM encoding, Tip5/hash parity, and consistent interpretation of activation heights, targets, fees, locks, and signatures. Cryptographic validity is delegated to the relevant verifier and signature crates; these types only preserve the values being verified.

Any wire-shape or constant change is a protocol change when it affects accepted nouns or computed hashes. It requires round-trip tests against the Hoon mold and coverage at every caller that persists, hashes, or transmits the object.

## Validation

```sh
cargo test -p nockchain-types
cargo check -p nockchain-types
```
