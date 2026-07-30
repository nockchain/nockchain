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
