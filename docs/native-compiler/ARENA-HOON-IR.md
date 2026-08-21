# Arena-indexed Hoon compiler IR

## Outcome

Honk compiles parsed and compiler-generated Hoon through a scope-local graph indexed by `HoonId(u32)`. `mint` and `play` enter through an ID, direct `%pair` and `%tsgr` recursion follows compact child-ID edges, and every other recursive compiler boundary resolves to an ID before dispatch. The parser AST remains the canonical source payload and the historical Hoon serializer remains the canonical noun encoder, so this changes compiler identity and memo layout without introducing a second parser or output representation.

## Representation

Each arena has one dense entry vector and one address-to-ID ingress map. An entry holds the read-only source node, its spot-sensitive structural signature, inline child IDs for the hot canonical binary forms, its lazily materialized canonical noun, and its cached `open` result. The former stable-pointer set, pointer-to-signature map, pointer-to-noun map, and stable-node `open_cache` probes collapse into one ingress lookup followed by indexed loads. IDs are post-order, so children precede parents and related metadata stays close in memory.

`Sig64` constructs the graph in one compositional traversal. It records every Hoon node reached directly or through helper payloads such as `Spec` and `Type`, records the `%pair` and `%tsgr` edges used by direct ID recursion, and computes every node signature once. Registration assigns dense IDs in a first pass and resolves those hot child addresses to IDs in a second pass. Collecting generic edges for every Hoon form was measured and rejected because the extra per-node work cost compiler throughput without serving the current dispatch path.

## Generated lowerings

Generated Hoon used to be excluded from the borrowed-root sidecar. Every recursive call on such a temporary could therefore rescan its subtree for a signature and miss the stable-node noun cache. The arena implementation treats a generated lowering as a shorter nested scope: it suspends the parsed parent arena, registers the lowering and all descendants, compiles under IDs, drops the temporary graph, and restores the parent. This is a LIFO stack of arenas rather than an unbounded persistent cache.

## Lifetime and unwind safety

Arena entries borrow parser or lowering nodes by stable address but never mutate them. `HoonAstScope` ties the arena to the root borrow and restores the previous arena in `Drop`, including during panic unwinding. A nested call on a node already present in the current graph does not push another arena. A call on a generated or decoded root pushes exactly one graph, and its descendants reuse it. This prevents stale source addresses from surviving either normal return or a caught unwind.

## Correctness boundary

Structural signatures include `%dbug` spots because emitted formulas can retain `%spot` hints. Canonical noun materialization still calls `hoon_to_noun_with_cache`, which delegates every encoding decision to the historical serializer and merely records completed descendants into dense noun slots. Exact decoded-hold lookup remains separate because it is keyed by the noun identity discovered from type data. Structural compiler caches retain copied signatures rather than scope-local IDs, so no ID escapes its arena.

## Acceptance gates

The implementation must pass focused graph, nested-lowering, noun-materialization, and unwind tests; the complete Honk and Hatch suites; formatting and diff checks; exact SHA-256 comparisons for the production kernels; and balanced release benchmarks against the untouched interned-AST-DAG parent. A stage that is byte-correct but negative in compiler throughput is not part of the accepted performance stack.

## Release-build performance

Measurements used `wx-workstation`, release LTO, one pinned CPU, identical source inputs, adjacent controls from the exact interned-AST-DAG parent `93ca81d8`, and exact output hashes. The gain is compiler throughput, calculated as `control-midpoint / candidate - 1`.

| Kernel | Parent bracket | Arena compiler | Throughput gain | Parent/candidate maximum RSS | Exact output SHA-256 |
| --- | ---: | ---: | ---: | ---: | --- |
| Dumbnet | 84.04/85.25s | 84.63s | +0.02% | 6.75/6.19 GB | `2b5b0f77937f5162ed0e6c8f8ebd2651761576798f79dfa95a079d490423bb01` |
| Wallet | 89.09/89.54s | 85.82s | +4.07% | 7.58/6.92 GB | `7f6fc956b87f660f7371bd2caf955f4d2610103bd5a05c1c20e0fce66fbfa474` |
| Roswell | 140.00/140.57s | 130.32s | +7.65% | 10.81/9.61 GB | `612025fc58d84dcd226c4b7c0b49602a4c86a430683d6b22b66a3e109815a842` |

The arena conversion therefore pays off increasingly with compiler workload size while remaining throughput-neutral on Dumbnet. Peak memory falls by 8.4% on Dumbnet, 8.7% on Wallet, and 11.1% on Roswell.

## Whole-graph PGO

A fresh clean PGO pipeline built an instrumented whole dependency graph, trained on Wallet and Dumbnet, merged both profiles, built the optimized graph, and verified byte-identical Dumbnet output in 590.22s. Pinned final measurements were 75.52s and 6.18 GB for Dumbnet, 76.70s and 6.91 GB for Wallet, and 114.98s and 9.61 GB for Roswell; all three artifacts had the exact hashes in the release table. Compared with the previously recorded AST-DAG PGO measurements of 72.50s, 77.56s, and 121.83s, those runs are -4.00%, +1.12%, and +5.96% in throughput respectively. That historical comparison is informative rather than an adjacent bracket: it confirms that the large-workload arena gain survives PGO, while Dumbnet remains sensitive to profile-driven code layout. The accepted source-level result is therefore based on the adjacent release brackets above rather than claiming a universal PGO gain.

## Rejected variants

The first arena prototype stored generic child edges in a per-node hash map. It reduced Dumbnet peak memory by about 8.2% but regressed throughput by 1.97%, so it was discarded. Replacing that map with a hash-free generic traversal stack reduced the regression to 0.50% on Dumbnet and 0.11% on Wallet but still failed the performance gate. Specializing storage and direct recursion to the measured-hot `%pair` and `%tsgr` forms produced the accepted result above.

Direct arena recursion for `%wtcl` and `%wtdt` was byte-exact but measured +0.30% on Wallet and -0.18% on Roswell, both inside noise. Extending the same mechanism to `%dbug`, `%note`, and `%tscm` measured +0.22% on Wallet but -0.50% on Roswell. Both extensions were removed from the final source because neither provided a reproducible positive yield.

System-wide sampling with `samply` was unavailable on the benchmark host because its kernel has `perf_event_paranoid=4` and the account has no passwordless privilege to change it. The campaign did not alter host security settings; it used the compiler's built-in phase timing, isolated pinned-CPU wall time, peak RSS, and exact artifact comparison instead.

## Validation result

The benchmarked compiler source tree is `e28528fb7c9b2098354c204d89df8b51a56720b7`, shared by the local accepted implementation and the remotely built benchmark binary. The complete Honk library suite passed all 136 tests and the complete Hatch library suite passed all 276 tests. `cargo check -p honk --lib`, `cargo fmt --check`, and `git diff --check` passed. All six production artifacts are byte-identical to the interned-AST-DAG parent. Miner compiled in 13.87s with SHA-256 `1a7615c7af9df85066c9981a30e3bfe128dd11f7e1760df182014a7cbb3aa418`, Peek in 70.27s with `9af1f26217fc3226f190bece929da7ac181b427fd721e42081faa731657fe094`, and Bridge in 85.80s with `6304ae91b546e0fe39d3be6e558d4710278904151c86406e37e4a903fcd8e7c3`; the remaining exact hashes appear in the performance table above.
