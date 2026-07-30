# Arena-indexed native type IR

## Outcome

Honk’s live type graph is now owned by the per-compile `Context` and every canonical node receives a dense `TypeId(u32)`. Recursive type algebra still dereferences a one-word handle when it needs the node payload, while identity-sensitive caches, cycle guards, fork keys, and structural hashes use compact IDs instead of allocation addresses. A handle clone is an unconditional word copy and its drop is a no-op; the hot path no longer performs reference-count mutations or tagged cold/hot discrimination.

## Representation

`TypeTable` owns stable `Box<TypeSlot<Type>>` allocations for the complete compiler context. A slot contains its dense ID and node payload, while `TypeRef<Type>` contains only the non-null slot pointer. The table interns children before parents, shallow-hashes canonical child IDs, confirms collisions with exact shallow equality, and returns the existing handle for structurally identical nodes. Context caches use `u32` identities, fork cache keys use vectors of IDs, and reachable-hold memoization uses the same ID namespace.

This is intentionally an arena-owned DAG rather than a `Vec<Type>` indexed directly by ID. Type constructors and recursive compiler code need cheap `Deref` without threading the table through every call, and stable boxed slots provide that property while removing `Rc` accounting. The dense ID remains the canonical cache identity; the pointer is only the payload handle.

## Lifetime and public boundary

Arena handles are internal to the crate-private type-IR module and the compiler’s `mint` and `play` entry points do not expose them publicly. The owning `Context` outlives every live type-algebra operation and cache that can dereference a handle. Dropping the arena needs no graph walk because child handles have no destructor behavior.

The optional public round-trip and interning-statistics oracles use a separate independently owned `BoundaryType` tree. This preserves the old diagnostic behavior without forcing every live handle to carry a tag and conditionally manage an `Rc`. The cold tree is decoded with owned leaves, round-trips through the historical noun encoder, and can be interned into a temporary table for the same unshared-versus-distinct statistics as before.

## Correctness boundary

The type noun remains the serialization oracle. No type constructor, normalization rule, fork witness, carried leaf, or emitted noun shape changes. IDs are local to one compiler context and never become serialized data. Pointer equality remains exact canonical-node equality, while cache keys use the slot’s immutable ID. Fork option materialization is deliberately excluded from interner identity just as before.

## Performance result

Measurements use the release-LTO compiler on `wx-workstation`, one pinned CPU, the exact arena-Hoon parent binary as control, adjacent control/candidate/control runs, and SHA-256 equality for every output. Throughput gain is `control midpoint / candidate - 1`; maximum RSS is reported from `/usr/bin/time -v`.

| Kernel | Parent bracket | Arena type compiler | Throughput gain | Parent/candidate maximum RSS | Exact output SHA-256 |
| --- | ---: | ---: | ---: | ---: | --- |
| Wallet | 94.05/93.30s | 92.23s | +1.57% | 6.91/6.85 GB | `7f6fc956b87f660f7371bd2caf955f4d2610103bd5a05c1c20e0fce66fbfa474` |
| Roswell | 137.64/136.20s | 132.75s | +3.14% | 9.61/9.43 GB | `612025fc58d84dcd226c4b7c0b49602a4c86a430683d6b22b66a3e109815a842` |
| Dumbnet | 91.25/91.62s | 89.89s | +1.72% | 6.19/6.17 GB | `2b5b0f77937f5162ed0e6c8f8ebd2651761576798f79dfa95a079d490423bb01` |

The gain grows with type-algebra workload size: Roswell improves most, while the smaller Dumbnet compiler still gains rather than paying an arena setup penalty. Peak RSS falls by 0.82% on Wallet, 1.85% on Roswell, and 0.43% on Dumbnet.

## Rejected variants

Replacing the type interner’s `DefaultHasher` with the existing byte-wise `FastHasher` was byte-exact but decisively slower on Wallet: 104.94s against a 95.10/95.85s parent bracket, a 9.02% throughput regression. Carried leaf hashes write byte slices, so the simple FNV-style byte loop costs more than it saves on dense child IDs. The experiment remains visible in history and is reverted from the accepted source.

Applying `FastHasher` only to integer-keyed context caches avoided the leaf-byte problem but still failed the cross-workload gate. Wallet measured 91.33s against an 89.60/93.13s bracket, +0.04% and therefore noise; Roswell measured 134.21s against 133.67/131.96s, a 1.04% throughput regression. This variant is also reverted.

An early tagged handle supported both context-owned slots and standalone `Rc` trees in one word. It established the dense-ID architecture and measured 97.77s against a 96.51/103.06s Wallet bracket, +2.06%, with exact output and lower RSS; Roswell was neutral at 142.62s against 145.79/139.04s, -0.14%. The final representation separates the cold decoder so the live handle does not branch on every clone, drop, dereference, or ID read.

## Validation

The acceptance gate is the complete Honk library suite, the complete Hatch library suite, compiler integration and rejection tests, formatting and diff checks, exact release artifact hashes for Dumbnet, Wallet, Miner, Peek, Bridge, and Roswell, and balanced benchmarks on Wallet, Roswell, and Dumbnet. A byte-correct variant that fails the throughput gate is not accepted.

Final production code commit `7cb4214b5be59ec3f5b63ba640741c435e322a0d` passed all 137 Honk library tests, all 276 Hatch library tests, all 70 compiler-mint integration tests, both compiler acceptance/rejection probes, `cargo check -p honk --all-targets`, formatting, and diff checks. A real Miner compile with both `HONK_NATIVE_PARITY=1` and `HONK_IR_ROUNDTRIP=1` completed successfully and emitted the known Miner hash. The final compiler independently rebuilt all six production artifacts with exact known hashes: Dumbnet `2b5b0f77937f5162ed0e6c8f8ebd2651761576798f79dfa95a079d490423bb01`, Wallet `7f6fc956b87f660f7371bd2caf955f4d2610103bd5a05c1c20e0fce66fbfa474`, Miner `1a7615c7af9df85066c9981a30e3bfe128dd11f7e1760df182014a7cbb3aa418`, Peek `9af1f26217fc3226f190bece929da7ac181b427fd721e42081faa731657fe094`, Bridge `6304ae91b546e0fe39d3be6e558d4710278904151c86406e37e4a903fcd8e7c3`, and Roswell `612025fc58d84dcd226c4b7c0b49602a4c86a430683d6b22b66a3e109815a842`.
