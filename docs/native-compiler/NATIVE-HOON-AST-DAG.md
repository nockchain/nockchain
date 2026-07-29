# Interned native Hoon AST DAG

## Outcome

Honk now gives every node in a borrowed top-level Hoon AST a scope-bound native identity, computes spot-sensitive structural signatures compositionally in one traversal, and materializes each stable node's canonical noun at most once per compiler scope. The optimization does not change the parser AST, emitted noun, type semantics, or kernel artifact: it adds a sidecar DAG over the AST that already exists and preserves the historical serializer as the sole output authority.

## Design

- The outermost `mint` or `play` call opens an AST scope and registers every native descendant by address. Recursive calls share that scope, while generated lowering temporaries are not registered and continue through the ordinary uncached path.
- `Sig64` hashes each native Hoon node independently and feeds the completed child digest into its parent. One root walk therefore produces the spot-sensitive identity of every descendant instead of rescanning a subtree whenever `mint`, `play`, `mull`, or `open` reaches it.
- Stable node addresses index only sidecar data and are valid because the top-level borrowed AST outlives the recursive compiler call. Every address-keyed table is cleared before that borrow ends, preventing a later allocation at the same address from inheriting stale identity or noun state.
- `hoon_to_noun_with_cache` calls the existing canonical Hoon serializer and memoizes completed descendant nouns in scoped thread-local scratch storage. Nested `Spec`, `Tome`, and other helper paths still reach the same cache through `hoon_to_noun`, so each native descendant is serialized once even when its noun is first requested through a subtree.
- Materialized nouns remain valid for the scope because `Ut` owns one `NounSlab` for the compile. The thread-local map is drained into the `Ut` sidecar after a miss, clears on normal return or unwind, and retains only allocation capacity between calls.
- `mull` uses the cached 64-bit structural AST signature directly instead of materializing the Hoon noun solely to compute a 31-bit noun mug. If a Hoon shape cannot be structurally signed, the historical noun-and-mug path remains the fallback.
- Exact decoded-hold noun lookup remains confined to exact-lookup mode. This avoids adding a general hash-table probe to the normal path for nodes that are not part of the stable borrowed root.

## Rejected variants

The first implementation retained cloned native AST nodes, including compiler-generated temporaries. On Dumbnet it regressed from 83.72s to 104.79s and increased maximum resident memory from 9.42 GB to 20.67 GB, so it was discarded. A bounded identity/signature-only variant that disabled noun reuse took 100.69s at 6.72 GB, demonstrating that one-time noun materialization is the essential half of the design. A two-thread-local fast-path variant took 88.58s and was also rejected. The accepted implementation retains no AST clone, does not assign address identity to transient nodes, and reuses one scratch map.

## Release-build performance

Measurements used `wx-workstation`, the same release configuration and source inputs, adjacent untouched-parent controls from commit `56a09dad2b6dad360d65a07ced4992599470c885`, and exact SHA-256 output comparisons. The gain is compiler throughput, computed as `control_time / candidate_time - 1`.

| Kernel | Untouched-parent bracket | Candidate | Compiler-throughput gain | Candidate maximum RSS | Exact output SHA-256 |
| --- | ---: | ---: | ---: | ---: | --- |
| Dumbnet | 83.72/84.47s | 82.72s | +1.66% | 6.72 GB | `2b5b0f77937f5162ed0e6c8f8ebd2651761576798f79dfa95a079d490423bb01` |
| Wallet | 92.83/94.23s | 88.11s | +6.15% | 7.53 GB | `7f6fc956b87f660f7371bd2caf955f4d2610103bd5a05c1c20e0fce66fbfa474` |
| Roswell | 147.50/151.60s | 134.29s | +11.36% | 10.77 GB | `612025fc58d84dcd226c4b7c0b49602a4c86a430683d6b22b66a3e109815a842` |

The final release compiler also produced exact known artifacts for Miner in 15.88s, Peek in 81.25s, and Bridge in 94.02s. Their SHA-256 values were `1a7615c7af9df85066c9981a30e3bfe128dd11f7e1760df182014a7cbb3aa418`, `9af1f26217fc3226f190bece929da7ac181b427fd721e42081faa731657fe094`, and `6304ae91b546e0fe39d3be6e558d4710278904151c86406e37e4a903fcd8e7c3`, respectively.

## Whole-graph PGO performance

A fresh whole-graph PGO compiler was trained on Wallet and Dumbnet from the final runtime source. The adjacent comparisons below use the prior native-type-DAG PGO compiler as the control. Every candidate artifact is byte-identical to its controls.

| Kernel | Prior PGO bracket | AST-DAG PGO | Compiler-throughput gain | Prior/candidate maximum RSS |
| --- | ---: | ---: | ---: | ---: |
| Dumbnet | 72.16/72.34s | 72.50s | -0.35% | 9.45/6.70 GB |
| Wallet | 80.50/81.01s | 77.56s | +4.12% | 10.88/7.52 GB |
| Roswell | 127.82/127.41s | 121.83s | +4.75% | 17.05/10.73 GB |

The small Dumbnet PGO regression is specific to optimized code placement rather than the source algorithm: the same source is +1.66% in the balanced release comparison and reduces Dumbnet maximum RSS by about 29%. A second experiment weighted Dumbnet twice in the PGO training corpus. A same-load standard/weighted/standard comparison measured 89.09/88.30s around an 88.77s weighted run, a -0.08% result inside noise, so the extra weight was rejected and the reproducible Wallet+Dumbnet recipe remains unchanged.

## Correctness invariants

- Structural signatures include `%dbug` spots wherever compiled formulas can retain `%spot` hints, matching the prior cache partitioning requirement.
- Structurally equal AST clones receive the same signature; pointer identity only avoids recomputation inside one proven borrow lifetime.
- The exact historical Hoon serializer creates every cached noun. The optimization memoizes its results rather than introducing a second representation or encoder.
- Stable AST sidecars clear at the outermost compiler boundary, including error returns. Materialization scratch state clears through an unwind guard.
- Transient generated ASTs and unsupported signature shapes retain the old serialization and noun-mug behavior.
- Whole-kernel byte equality, rather than semantic equivalence alone, is the acceptance gate.

## Validation

The focused materialization regression verifies that a seven-node AST spanning a direct child and a Hoon nested inside `Spec` records every node and jams byte-identically to uncached serialization. The scope regression verifies complete descendant registration, nested-scope reuse, structural-clone signature equality, and complete address-cache clearing at the outermost return. The complete Honk library suite passed with 132 tests passing, zero failing, and one pre-existing ignored test; the complete Hatch library suite passed all 271 tests. `cargo fmt --check` and `git diff --check` passed. All six production kernels match the untouched parent byte for byte in both release and fresh PGO builds.
