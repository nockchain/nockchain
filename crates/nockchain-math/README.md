# `nockchain-math`

`nockchain-math` contains shared field, polynomial, hash, cryptographic, noun, and persistent-data-structure primitives used by the node, wallet, proof systems, and Hoon jets.

## Place in the system

The crate provides Rust representations and operations for:

- Goldilocks-field belts and extension-field values;
- polynomials and structured proof data;
- Tip5 hashing and sponge operations;
- Cheetah-curve and related cryptographic helpers under `crypto`;
- noun conversion and ownership adapters; and
- `zoon` persistent collection types mirrored across Rust/Hoon boundaries.

It supplies primitives, not transaction policy, block validity, or fork choice.

## Maintained invariants

- Field operations use the exact modulus and canonical representation expected by Nockchain and its proof systems.
- Hash inputs, output widths, padding, endianness, and domain conventions match their Hoon and circuit counterparts.
- The canonical Nockchain Tip5 permutation is seven rounds. `permute_5round` is a separate paper-spec variant used only by the AI recursive-proof stack; it must never replace canonical hashing.
- Noun conversions preserve atom byte order and reject values that do not fit the destination type.
- Persistent structures preserve deterministic ordering and root/hash behavior wherever those values cross consensus boundaries.
- Optimized or jetted implementations must remain byte-for-byte equivalent to the reference semantics.

## Cryptographic dependencies

Security depends on the assumed properties of Tip5, BLAKE3 where downstream crates use it, Argon2 where invoked, the Cheetah curve and signature construction, and the algebraic soundness of the Goldilocks-field proof systems. This crate implements building blocks; safe composition, domain separation, key validation, transcript ordering, and protocol-level checks remain caller obligations.

Changes to field constants, Tip5 rounds/constants, serialization, elliptic-curve formulas, or hash-domain behavior can be consensus-breaking even when the Rust API still compiles. Such changes require cross-implementation known-answer tests against Hoon/circuit behavior and the protocol specification.

## Validation

```sh
cargo test -p nockchain-math
cargo check -p nockchain-math
```
