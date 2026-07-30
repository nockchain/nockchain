# Native Hoon compiler notes

These notes are for open-source native compiler work. Keep repo paths in this
section under `open/`. Parser details belong here when they affect compiler
artifact parity; parser-only notes belong in `open/docs/native-parser/`.

## Scope

Primary code and reference inputs:

- `honk` at `open/crates/honk/`
- `open/crates/hoonc/hoon/hoon-138.hoon`
- `hatch` at `open/crates/hatch/`
- `open/hoon/common/hoon.hoon`

The native compiler goal is byte-for-byte artifact parity with the canonical Hoon
compiler for agreed open workloads, while preserving deterministic output and
practical compile time.

## Parity policy

- Byte-for-byte artifact equality is the acceptance criterion for compiler parity targets for now.
- Structural JAM comparison is diagnostic only unless a test explicitly says otherwise.
- Source spots and `dbug` metadata are part of the artifact. Do not normalize or delete them to hide differences; fix the parser or compiler behavior that created the mismatch.
- Prefer general semantic fixes over file-, line-, or workload-specific exceptions.
- Bazel artifact tests are authoritative for checked-in parity. Ad hoc local CLI compiles are useful for profiling and debugging, but local path metadata or undeclared directory state can make them unsuitable as final parity evidence.

Useful open validation targets and suites:

```bash
cargo test --release -p hatch --lib
cargo test --release -p honk
bazel test //open/crates/honk/test-assets:hoon_138_arbitrary_parity_test
bazel test //open/assets/native:kernel_parity_test
```

Performance design notes:

- [`ARENA-HOON-IR.md`](ARENA-HOON-IR.md) describes the scope-local `HoonId` graph, nested lowering arenas, lifetime invariants, and acceptance gates.

Ignored tests do not validate parity in normal Cargo runs. If an ignored heavy
parity test is the evidence for a change, run it explicitly with `--ignored` and
record that fact in the change notes.

## Semantic notes to preserve

- Nock axis handling must support arbitrary-size atoms in axis-sensitive forms. Rejecting or truncating large axes is a compiler semantics bug.
- `/*` data imports must use the canonical `$octs` type asset at `open/crates/honk/assets/hoonc-octs-type-138.jam`; the byte length matches the canonical `(met 3 fil)` behavior.
- Caches and memo tables are accelerators only. Cache hits and misses must be observationally identical.
- Recursive type operations that depend on active `%hold` expansion state must include that state, or an equivalent guard signature, in any memoization rule.

## Parser notes that belong here

Parser source-range behavior feeds directly into compiler artifact parity because spots become artifact data. Notes about native parser span expansion, doc-comment anchoring, and `dbug` spot parity should stay in this directory unless they are strictly about parser behavior independent of compiler output.
