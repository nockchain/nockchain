# Arena-indexed seminoun and value IR

## Outcome

Honk now evaluates Musk's abstract Nock values in two compile-local native DAGs. `ValueArena` assigns exact structural `ValueId(u32)` identities to complete nouns, and `SemiArena` represents complete, blocked, half, and lazy seminoun states behind hash-consed `SemiId(u32)` identities. The compiler no longer rebuilds recursive seminoun mask nouns at every Nock step or compares reconstructed constant cores with repeated full noun equality walks.

## Representation and invariants

`ValueArena` imports a slab noun iteratively, canonicalizes direct atoms by value, collision-checks indirect atoms by exact bytes, and canonicalizes cells by their already-canonical child IDs. A raw-address table makes re-importing the same slab noun constant time, while structurally equal values at different addresses converge to one ID. Every entry retains the original noun used at semantic interpreter boundaries, so identity changes do not alter the value passed to NockVM or the final compiler artifact.

`SemiArena` stores `Complete(ValueId)`, `Blocked`, `Half { head, tail }`, and `Lazy { fragment, resolver_id }`. Complete values, half pairs, lazy fragments, and the blocked singleton are hash-consed. Fragment and mutation traverse arena edges directly; combining two complete values creates one slab cell and one canonical value node; lazy completion still calls the existing content-addressed arm resolver. Encoded seminouns carried in core-type leaves are imported once at the boundary, while core-type construction continues to emit the canonical noun encoding expected by the type system and artifact format.

The `musk_araw` memo is keyed by `(SemiId, formula_raw)`, constant-core Mack results are keyed by `(ValueId, axis)`, `%ktsg` folds are keyed by `(SemiId, FormulaId)`, and the native `bran` cache stores `SemiId`. These are exact identities rather than hashes: structurally unequal inputs cannot alias, and a cache hit is behaviorally identical to recomputation. Arbitrary-precision axes remain `BigUint` on the wide path.

## Performance evidence

The accepted parent is the formula-DAG compiler at runtime commit `62a309284800d4eeb30498064ba40c44b265834b`. The first isolated change, commit `912eb7eb`, added canonical complete-value identity and re-keyed the constant-core Mack cache. Its initial balanced Dumbnet gate measured 80.773 seconds parent versus 43.377 seconds candidate, an 86.214% compile-throughput gain, with all three pairs positive and every artifact byte-identical.

The second isolated change, commit `9916a2a8`, moved the full seminoun lattice and Musk evaluator to `SemiId`. Its exact cleaned stage-one/stage-two gate measured 43.607 seconds versus 31.067 seconds, a further 40.365% compile-throughput gain, again with all three pairs positive and every artifact byte-identical.

The final direct balanced parent/final gate on `wx-workstation` ran parent/final, final/parent, and parent/final. The pairs were 84.12/31.03, 84.04/31.69, and 83.75/31.71 seconds. Parent mean was 83.970 seconds and final mean was 31.477 seconds: 62.514% less wall time, 2.668× total throughput, or a 166.769% compile-throughput gain. Every one of the six outputs had the accepted Dumbnet SHA-256 `2b5b0f77937f5162ed0e6c8f8ebd2651761576798f79dfa95a079d490423bb01`.

The final exact-source release binary SHA-256 was `af3cb8e247c3e93cf6a77068ebebcdb63c051b14811eafdb47d3a5afb1fda0f4`. An exact-output Samply run collected 27,190 worker samples. `noun_eq`, which was 56.17% inclusive before value identity, fell to 3.02% self; native value import was 0.88% self. The new frontier is distributed across native compiler work: `Sig64::write_hoon` was 18.02% inclusive and 4.78% self, Musk was 20.20% inclusive, and seminoun completion was 18.72% inclusive. The latter two percentages overlap because completion occurs under Musk and no longer identify a single dominant primitive comparable to the removed equality walk.

## Correctness gates

Focused tests cover structurally equal values sharing one ID, semantically equal seminoun states sharing one ID, exact complete-value combination, fragment and mutation behavior, and importing a mixed complete/blocked encoded half seminoun. The final source passed 148 Honk library tests, six Honk binary tests, 70 compiler-mint integration tests, two compiler acceptance/rejection probes, 276 Hatch library tests, and 183 NockVM library tests with four pre-existing ignored NockVM tests. `cargo check -p honk --all-targets`, formatting, and whitespace validation passed.

The exact final binary independently rebuilt all six production kernels. Their SHA-256 values were Dumbnet `2b5b0f77937f5162ed0e6c8f8ebd2651761576798f79dfa95a079d490423bb01`, Wallet `7f6fc956b87f660f7371bd2caf955f4d2610103bd5a05c1c20e0fce66fbfa474`, Miner `1a7615c7af9df85066c9981a30e3bfe128dd11f7e1760df182014a7cbb3aa418`, Peek `9af1f26217fc3226f190bece929da7ac181b427fd721e42081faa731657fe094`, Bridge `6304ae91b546e0fe39d3be6e558d4710278904151c86406e37e4a903fcd8e7c3`, and Roswell `612025fc58d84dcd226c4b7c0b49602a4c86a430683d6b22b66a3e109815a842`.
