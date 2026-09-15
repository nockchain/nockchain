# End-to-end native type DAG

## Outcome

Honk's recursive type algebra now traverses `%fork` members as native `Rc<Type>` children instead of repeatedly lowering the fork to a noun, walking its Hoon `%set` treap, and lifting every member back into the native interner. The exact original treap is retained as an immutable serialization witness, so typed-Dynock output and kernel JAM bytes remain unchanged.

The implementation is adaptive because unconditional retention was not a universal performance win. A fork's first traversal uses inline `SmallVec` storage and returns owned native children without reference-count churn. A second traversal promotes those children into the fork's `OnceCell<Vec<Rc<Type>>>`; every later consumer walks the cached slice directly. This preserves the end-to-end native DAG benefit for shared forks without making one-shot forks retain an allocation for the rest of the compile.

## Profile evidence

The Wallet+Dumbnet whole-graph PGO profile at source commit `f98a9884` recorded 2,330,316 entries into `fork_options_native`, 1,620,460 entries into the historical `fork_set_options`, 1,257,496 `cons_fork` calls, and 16,970,255 `live_to_noun` calls. Only 674,021 `cons_fork` executions reached the multi-member option loop, which identified empty/singleton construction and the fork noun boundary as the highest-EV remaining type-DAG work.

## Design

- `Type::Fork` owns the exact `Leaf` set witness plus adaptive native child state. The witness alone defines structural hash and equality; cache mutation cannot change interner identity.
- `visit_fork_set_members` walks the historical treap order with a bounded iterative stack. It calls a visitor directly instead of allocating an intermediate `Vec<Noun>`.
- The transient/cached iterator moves `Rc` children out of transient storage and clones only cached children whose ownership must stay with the DAG.
- Every native type consumer in `find`, `wet`, `nest`, `wrap_type`, `take`, `gain`, `lose`, `miss`, `fuse`, `crop`, `peek`, and reachable-leg analysis uses this single native fork boundary.
- `cons_fork` implements exact empty, void-singleton, and singleton collapses natively. Multi-member construction still delegates treap normalization and ordering to the historical noun implementation, preserving nested-fork flattening, duplicate removal, void removal, and RT-07 order exactly.

## Performance experiments

All measurements used one pinned CPU on `wx-workstation`, the same release settings, the same source tree, balanced adjacent controls, and exact output SHA checks.

| Variant | Roswell result versus smart-only | Wallet result versus smart-only | Dumbnet result versus smart-only | Decision |
| --- | ---: | ---: | ---: | --- |
| Eager native fork children | Rejected under an initially confounded run | Not advanced | Not advanced | Rejected: unnecessary retention and first-use work |
| Unconditional lazy `Rc<[Rc<Type>]>` | Approximately flat to slightly negative | Not advanced | Not advanced | Rejected: first-use copy plus retained allocation |
| Unconditional lazy `Vec<Rc<Type>>` | 152.98s versus 150.02/150.90s | Not advanced | Not advanced | Rejected |
| Adaptive native DAG with move-aware iteration | 153.69s versus 153.50/152.91s (-0.32%) | 97.00s versus 97.53/97.50s (+0.53%) | 87.58s versus 87.87/88.16s (+0.49%) | Accepted: positive on both PGO training kernels with negligible Roswell tradeoff |

The independent native smart-constructor change measured 151.70s against a 152.63/153.86s Roswell bracket, approximately +1.0%. Whole-graph PGO was then regenerated from the final source and compared against the previous PGO compiler with balanced prior/final/prior runs:

| Kernel | Previous PGO bracket | Native-DAG PGO | Compiler-throughput gain | Output SHA-256 |
| --- | ---: | ---: | ---: | --- |
| Roswell | 135.00/135.33s | 133.36s | +1.35% | `612025fc58d84dcd226c4b7c0b49602a4c86a430683d6b22b66a3e109815a842` |
| Wallet | 85.32/85.34s | 83.75s | +1.89% | `7f6fc956b87f660f7371bd2caf955f4d2610103bd5a05c1c20e0fce66fbfa474` |
| Dumbnet | 76.64/76.46s | 75.18s | +1.82% | `2b5b0f77937f5162ed0e6c8f8ebd2651761576798f79dfa95a079d490423bb01` |

## Correctness invariants

- `Type::to_noun` emits the retained set witness and never derives output from cached child order.
- Fork hash and equality ignore `options` and `options_seen`; both are pure cache state derived from the witness.
- Oracle `Type::from_noun` eagerly populates native children so standalone IR round trips exercise the complete shape.
- Live noun decoding remains lazy to avoid doing native work for forks that type algebra never traverses.
- A one-shot fork does not retain a child vector. The second traversal populates the cache, and subsequent traversals share the same slice.
- Empty, singleton, nested, duplicate, hinted, ambiguity, pruning, and branch-isolation behavior remains covered by the fork regression tests and whole-kernel parity gates.

## Validation

- The full Honk suite passed: 205 tests passed, zero failed, and one pre-existing test remained ignored.
- `cargo fmt --check`, `cargo check -p honk --lib`, and `git diff --check` passed.
- The four native fixtures (`core_chain`, `fork`, `loop_dec`, and `wet_turn`) were compiled in debug and non-debug modes by both the untouched `f98a9884` parent and the final compiler. All eight candidate artifacts were byte-identical to their parent artifacts.
- All six production kernels were compiled by both the untouched parent and the final compiler. Every candidate artifact was byte-identical to its parent artifact:

| Kernel | Exact shared SHA-256 |
| --- | --- |
| Dumbnet | `2b5b0f77937f5162ed0e6c8f8ebd2651761576798f79dfa95a079d490423bb01` |
| Wallet | `7f6fc956b87f660f7371bd2caf955f4d2610103bd5a05c1c20e0fce66fbfa474` |
| Miner | `1a7615c7af9df85066c9981a30e3bfe128dd11f7e1760df182014a7cbb3aa418` |
| Peek | `9af1f26217fc3226f190bece929da7ac181b427fd721e42081faa731657fe094` |
| Bridge | `6304ae91b546e0fe39d3be6e558d4710278904151c86406e37e4a903fcd8e7c3` |
| Roswell | `612025fc58d84dcd226c4b7c0b49602a4c86a430683d6b22b66a3e109815a842` |

- A fresh whole-graph PGO build trained on Wallet and Dumbnet completed, and its built-in Dumbnet comparison was byte-identical.
- `just native-goldens check` and `shadow_gate.sh` currently report differences, but the untouched `f98a9884` parent reports the identical differences against the checked-in files. Those references were already stale before this change, so they were not regenerated or silently accepted here; exact candidate-versus-parent comparisons replace them as the regression gate for this patch.
