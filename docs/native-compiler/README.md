# Native Hoon compiler notes

These notes are for open-source native compiler work. Paths are relative to the
repository root. Parser details belong here when they affect compiler artifact
parity; parser-only notes belong in `docs/native-parser/`.

## Scope

Primary code and reference inputs:

- `honk` at `crates/tools/honk/`
- `crates/tools/hoonc/hoon/hoon-138.hoon`
- `hatch` at `crates/tools/hatch/`
- `hoon/common/hoon.hoon`

The native compiler goal is byte-for-byte artifact parity with the canonical Hoon
compiler for agreed open workloads, while preserving deterministic output and
practical compile time.

## Parity policy

- Byte-for-byte artifact equality is the acceptance criterion for compiler parity targets for now.
- Structural JAM comparison is diagnostic only unless a test explicitly says otherwise.
- Source spots and `dbug` metadata are part of the artifact. Do not normalize or delete them to hide differences; fix the parser or compiler behavior that created the mismatch.
- Prefer general semantic fixes over file-, line-, or workload-specific exceptions.
- Bazel artifact tests are authoritative for checked-in parity. Ad hoc local CLI compiles are useful for profiling and debugging, but local path metadata or undeclared directory state can make them unsuitable as final parity evidence.

Useful validation targets and suites:

```bash
cargo test --release -p hatch --lib
cargo test --release -p honk
bazel test //crates/tools/honk/test-assets:hoon_138_arbitrary_parity_test
bazel test //assets/native:kernel_parity_test
```

Performance design notes:

- [`ARENA-HOON-IR.md`](ARENA-HOON-IR.md) describes the scope-local `HoonId` graph, nested lowering arenas, lifetime invariants, and acceptance gates.
- [`ARENA-FORMULA-IR.md`](ARENA-FORMULA-IR.md) describes the end-to-end `FormulaId` graph, explicit formula-as-data boundaries, exactness rules, and measured validation.
- [`ARENA-SEMINOUN-IR.md`](ARENA-SEMINOUN-IR.md) describes canonical complete-value identity, the native seminoun lattice, exact Musk cache keys, and the measured 2.668× compile-throughput result.

Ignored tests do not validate parity in normal Cargo runs. If an ignored heavy
parity test is the evidence for a change, run it explicitly with `--ignored` and
record that fact in the change notes.

## Semantic notes to preserve

- Nock axis handling must support arbitrary-size atoms in axis-sensitive forms. Rejecting or truncating large axes is a compiler semantics bug.
- `/*` data imports must use the canonical `$octs` type asset at `crates/tools/honk/assets/hoonc-octs-type-138.jam`; the byte length matches the canonical `(met 3 fil)` behavior.
- Caches and memo tables are accelerators only. Cache hits and misses must be observationally identical.
- Recursive type operations that depend on active `%hold` expansion state must include that state, or an equivalent guard signature, in any memoization rule.

## Parser notes that belong here

Parser source-range behavior feeds directly into compiler artifact parity because spots become artifact data. Notes about native parser span expansion, doc-comment anchoring, and `dbug` spot parity should stay in this directory unless they are strictly about parser behavior independent of compiler output.

## Cache and allocation safety

- Boundary memos must distinguish active `%rest`/`%hold` fan scope, vet mode,
  and any in-progress arm/placeholder context that affects the result. Structural
  input equality alone cannot justify a hit. See the
  [cache context matrix](PHASE0-CACHE-MATRIX.md).
- Cross-call `miss` memo persistence is not sound for entry/kernel compiles;
  a per-call recursion memo and a persistent memo have different lifetimes.
- Copy nouns into the interpreter's arena before evaluation, and copy escaping
  results out before reclaiming that arena. Branded handles express this boundary;
  they do not by themselves validate address-keyed cache lifetimes.
- Reclamation must preserve every escaping noun and invalidate or re-key every
  cache that can retain reclaimed addresses. Checkpoint rewind alone cannot bound
  a recursive mint while caches and lazy resolvers still reference its allocation.
- Chunking a top-level `=>` chain does not bound the largest core within a layer.
  Per-arm reclamation can also increase retained copies and cache recomputation;
  measure peak memory and byte parity rather than assuming frame pops reduce RSS.
  See [chunked-mint limits](PHASE0-CHUNKED-DECISION.md) and
  [arena provenance](PHASE0-PROVENANCE-DESIGN.md).
- Embedded-prelude compilation does not prove native prelude parity. Exercise the
  native arbitrary-prelude target separately; do not replace byte equality with
  structural or directory-hash-only comparison to hide a compiler difference.
