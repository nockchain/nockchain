# Native Hoon Compiler (`honk`)

Rust native compiler for Hoon-to-Nock compilation and byte-for-byte parity with `hoonc`, principally verified with `hoon-138` and the project-specific kernels.

Canonical compiler reference file:

- `crates/hoonc/hoon/hoon-138.hoon`

Native compiler notes:

- `../../docs/native-compiler/README.md`
- `../../docs/native-compiler/architecture.md`
- `../../docs/native-compiler/parity-policy.md`
- `../../docs/native-compiler/semantic-invariants.md`
- `../../docs/native-compiler/debugging.md`
- `../../docs/native-compiler/performance.md`

## High-level architecture

The crate has two closely related jobs:

1. Provide a library API for compiling a parsed `Hoon` AST into a type and Nock formula.
2. Provide a CLI that parses files, resolves imports, builds a canonical compilation subject, and emits JAM artifacts in the same shape as the canonical compiler.

Important source areas:

| Path | Role |
| --- | --- |
| `src/lib.rs` | Public `Compiler` API and `Compiled` output wrapper. |
| `src/native/mod.rs` | Native compiler facade around the semantic engine. |
| `src/native/ut/` | Native implementation of the Hoon `++ut` typechecker/compiler semantics. |
| `src/native/formula.rs` | Nock formula constructors and formula-level optimizations. |
| `src/native/noun.rs` | Noun conversion helpers. |
| `src/pipeline.rs` | Native parser integration and import resolution helpers. |
| `src/bin/honk.rs` | CLI used by Bazel native Hoon rules. |
| `src/arm_map.rs` | Arm-name-to-axis extraction from compiled core types used by parity and arm-axis validation. |
| `src/artifact.rs` | Import, export, verification, and structural diffing for Nockasm kernel artifacts. |
| `src/build_cache.rs` | Atomic content-addressed storage for persistent native build products. |
| `../honk-tools/` | Standalone JAM/asset diagnostics such as `jam-diff` and `extract-hoonc-octs-type`. |
| `test-assets/` | Minimal open compiler fixtures and Bazel parity targets. |

## Compile flow

For a direct AST compile through the library API:

1. Caller provides a `hatch::ast::hoon::Hoon` AST.
2. `Compiler` delegates to `NativeCompiler`.
3. `NativeCompiler` creates a noun slab and initial subject/goal types.
4. `Ut::mint` compiles the AST under a subject type and goal type.
5. The result is a pair of inferred type noun and generated Nock formula.
6. `Compiled` can JAM the formula, the type, arbitrary-mode output, or dynock output depending on caller needs.

For a file/artifact build through the CLI or Bazel path:

1. Parse the prelude and entry source with the native parser.
2. Resolve Hoon imports and data imports under the dependency directory.
3. Build the prelude subject and wrapper gates needed for canonical artifact shapes.
4. Compile the requested entry using the native semantic engine.
5. Apply the requested output mode (`standard`, `arbitrary`, `dynock`, or `dynock-typed`).
6. JAM the final noun and write the artifact.

## Semantic engine

`src/native/ut/` is the core of the compiler. It implements the type and compile operations corresponding to Hoon `++ut`, including:

- `mint`: infer/check type and emit Nock formula.
- `play`: infer type without emitting a formula.
- `nest`: subtype checking.
- `find`, `fond`, `fine`, `fire`: wing and arm resolution.
- `peek`, `repo`, `feel`, `lose`, `gain`-like narrowing, `fuse`, `crop`, and subject update helpers.
- Constant-folding helpers used by `^~` and related paths.

The implementation uses caches heavily for repeated type operations. Caches are correctness-preserving accelerators only: a cache hit and a cache miss must produce identical compiler output.

## Parser/compiler boundary

The compiler consumes the AST from `crates/hatch/`. Parser choices can change compiler artifacts when they affect source spots, docs, `dbug` wrappers, or AST shape. Parser behavior that exists for compiler artifact parity is therefore documented with native compiler notes, not only parser notes.

Relevant docs:

- `../../docs/native-compiler/parity-policy.md`
- `../../docs/native-compiler/semantic-invariants.md`

## Output modes

The compiler supports several artifact shapes:

- `standard`: canonical evaluated output shape for normal builds.
- `arbitrary`: a kickable trap for arbitrary Hoon output.
- `dynock`: `[type (trap nock)]` with a stable minimal type header.
- `dynock-typed`: `[inferred-type (trap nock)]` retaining the inferred type.

The library-level `Compiled` type exposes helpers for these shapes. The CLI and Bazel rules select the mode through flags or per-entry batch manifest rows.

## Bazel integration

Native Hoon Bazel rules live in `rules_hoon/hoon.bzl` and are exported from
`rules_hoon/defs.bzl`.

- `honk_library` compiles one Hoon entry to one native JAM artifact.
- `hoon_library` builds the matching hoonc reference artifact.

Strict parity targets are defined under `crates/honk/test-assets/` and
`assets/native/`.

## Parity policy

Native compiler parity is byte-for-byte artifact parity unless a test explicitly states a weaker diagnostic comparison. Source spots and `dbug` metadata are part of the artifact and should not be ignored to hide differences.

Useful validation commands:

```bash
cargo test --release -p hatch --lib
cargo test --release -p honk
bazel test //crates/honk/test-assets:hoon_138_arbitrary_parity_test --test_output=errors
bazel test //assets/native:kernel_parity_test --test_output=errors
```

When changing compiler performance code, run parity first and profile second. When changing parser spot behavior for compiler parity, run parser unit tests and at least one byte-for-byte compiler artifact parity target.

## CLI examples

Build the native compiler binary:

```bash
cargo build --release -p honk --bin honk
```

Build a profile-guided optimized compiler:

```bash
just build-honk-pgo
target/honk-pgo/honk --help
```

This recipe uses vanilla Rust PGO across honk's complete target dependency graph. It builds an instrumented compiler, trains it on the Wallet and Dumbnet kernels, merges the resulting profiles with the `llvm-profdata` from the active Rust toolchain, builds the optimized compiler, and checks that its Dumbnet JAM is byte-identical to the instrumented compiler's output. The final binary, merged profile, and source/toolchain identity are written under `target/honk-pgo/`. Install the matching LLVM tools first with `rustup component add llvm-tools-preview` if the active toolchain does not already include them.

Compile one entry in arbitrary mode:

```bash
target/release/honk \
  --new \
  --arbitrary \
  --output out.jam \
  --prelude hoon/common/hoon.hoon \
  crates/hoonc/hoon/hoon-138.hoon \
  .
```

Diff two JAM artifacts structurally:

```bash
cargo build --release -p honk-tools --bin jam-diff
target/release/jam-diff left.jam right.jam
```

## Persistent incremental builds

Pass `--cache-dir` to persist compiled dependency vases and entry products as
compact Nockasm DAG bundles. Cache keys are Merkle hashes over the source,
ordered Hoon and data imports, import faces, logical debug path, prelude and
subject identities, compiler ABI, and semantic flags. File timestamps and
absolute checkout paths are not inputs.

```bash
target/release/honk \
  --cache-dir target/honk-cache \
  --output out.jam \
  --prelude hoon/common/hoon.hoon \
  hoon/apps/dumbnet/outer.hoon hoon
```

`--new` bypasses cache reads and atomically repopulates the same
content-addressed objects. Cache writes use a temporary file, `fsync`, and
rename; a missing, truncated, noncanonical, or hash-mismatched object is a
cache miss and is repaired by the successful build. The compiler does not
persist `Ut`'s semantic memo tables because those depend on mutable compilation
state and are not safe across builds.

Inspect or age out the cache with:

```bash
target/release/honk cache stats --cache-dir target/honk-cache
target/release/honk cache gc --cache-dir target/honk-cache --max-age-days 30
```

## Nockasm artifact inspection

Any valid kernel JAM can be imported, whether it came from `hoonc` or `honk`:

```bash
target/release/honk nockasm export kernel.jam --output kernel.nockasm
target/release/honk nockasm verify kernel.nockasm
target/release/honk nockasm diff hoonc.jam honk.jam --output compiler.diff
```

`graph.ndag` is the authoritative compact, lossless noun DAG. `manifest.json`
records source, graph, root, and canonical-JAM hashes. `tree/` is a
content-addressed debug view split into 256 files; its 128-bit display IDs and
sorted records stay stable when unrelated nodes move. The specialized diff
skips equal subgraphs by full BLAKE3 hash and emits small changed subtrees as
named-op Nockasm fragments. Large mismatched subtrees are represented by an
axis and full hash rather than expanded into an unmanageable file.

The exported tree is for review and diagnostics, not the build-cache hot path.
Persistent builds read the compact bundle directly.
