# Native Hoon Compiler (`honk`)

Rust native compiler for Hoon-to-Nock compilation and byte-for-byte parity with `hoonc`, principally verified with `hoon-138` and the project-specific kernels.

Canonical compiler reference file:

- `open/crates/hoonc/hoon/hoon-138.hoon`

Native compiler notes:

- `../../../docs/native-compiler/README.md`
- `../../../docs/native-compiler/architecture.md`
- `../../../docs/native-compiler/parity-policy.md`
- `../../../docs/native-compiler/semantic-invariants.md`
- `../../../docs/native-compiler/debugging.md`
- `../../../docs/native-compiler/performance.md`

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

The compiler consumes the AST from `open/crates/hatch/`. Parser choices can change compiler artifacts when they affect source spots, docs, `dbug` wrappers, or AST shape. Parser behavior that exists for compiler artifact parity is therefore documented with native compiler notes, not only parser notes.

Relevant docs:

- `../../../docs/native-compiler/parity-policy.md`
- `../../../docs/native-compiler/semantic-invariants.md`

## Output modes

The compiler supports several artifact shapes:

- `standard`: canonical evaluated output shape for normal builds.
- `arbitrary`: a kickable trap for arbitrary Hoon output.
- `dynock`: `[type (trap nock)]` with a stable minimal type header.
- `dynock-typed`: `[inferred-type (trap nock)]` retaining the inferred type.

The library-level `Compiled` type exposes helpers for these shapes. The CLI and Bazel rules select the mode through flags or per-entry batch manifest rows.

## Bazel integration

Native Hoon Bazel rules live in `open/rules_hoon/native_hoon.bzl`.

- `native_hoon_library` compiles one Hoon entry to one JAM artifact.
- `native_hoon_batch` compiles multiple entries in one native compiler process with a shared prelude/context. Batch entries are compiled serially inside that process; the tradeoff is cache/context reuse rather than Bazel-level parallelism between entries.

Open parity targets are defined under `open/crates/honk/test-assets/` and `open/assets/native/`.

## Parity policy

Native compiler parity is byte-for-byte artifact parity unless a test explicitly states a weaker diagnostic comparison. Source spots and `dbug` metadata are part of the artifact and should not be ignored to hide differences.

Useful open validation commands:

```bash
cargo test --release -p hatch --lib
bazel test //open/crates/honk:honk_tests --test_output=errors
bazel test //:compiler_parity_tests --test_output=errors
```

When changing compiler performance code, run parity first and profile second. When changing parser spot behavior for compiler parity, run parser unit tests and at least one byte-for-byte compiler artifact parity target.

## CLI examples

Build the native compiler binary:

```bash
cargo build --release -p honk --bin honk
```

Compile one entry in arbitrary mode:

```bash
target/release/honk \
  --new \
  --arbitrary \
  --output out.jam \
  --prelude open/hoon/common/hoon.hoon \
  open/crates/hoonc/hoon/hoon-138.hoon \
  open
```

Diff two JAM artifacts structurally:

```bash
cargo build --release -p honk-tools --bin jam-diff
target/release/jam-diff left.jam right.jam
```
