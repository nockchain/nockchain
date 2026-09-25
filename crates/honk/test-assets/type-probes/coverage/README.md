# Coverage probes

These packages hold hoonc-parity probes written to close branch-coverage gaps
in `hatch` (the parser and desugarer) and `honk` (the compiler). Every `*.hoon`
at the top of a package is compiled by both hoonc and honk (`--arbitrary`,
empty deps) and the artifacts must be byte-identical; each package's
`coverage_parity_test` suite is part of
`//crates/honk/test-assets/type-probes:all_parity_test`, which CI's type probe
parity job runs.

| Package | Source range |
|---|---|
| `p1` | `crates/hatch/src/utils.rs` desugarer (`open`, the `+ax`/`+ap`/`+ah` ports) |
| `p2` | `utils.rs` lexing: tokens, floats, text, docs, `LineMap` |
| `p3` | `utils.rs` atom, path, date, and spec parsers |
| `p4` | `utils.rs` AST/noun conversion, `main.rs`, `runes/*.rs`, `ast/hoon.rs` |
| `c1`, `c2`, `c3` | `crates/honk/src/native/ut/mod.rs`, in thirds |
| `c4` | the other `ut` modules, `native/noun.rs`, `native/formula.rs` |
| `c5` | `crates/honk/src/native/ir/` |
| `c6` | `pipeline.rs`, the library entry points, and the CLI (also holds multi-file import and kernel pairings in its BUILD file) |
| `t1` | build cache, Nockasm artifacts, nasm bridge, arm maps (no probes; unit tests only) |
| `regressions` | probes for fixed hoonc divergences |

Conventions:

- `UNCOVERED.md` in each package lists every branch and line in its range that
  the unit tests or the parity corpus do not reach, with the reason.
- A probe that makes the compilers disagree goes in `<pkg>/divergent/` (its
  test is tagged manual) with a row in `../DIVERGENCES.md` until it is fixed.
  Once fixed, it moves to `regressions/`, or to `../reject/` and
  `REJECTION_PROBES` if both compilers now reject it.
- Unit tests for the same ranges live in `crates/hatch/src/cov/`,
  `crates/honk/src/native/ut/cov/`, `crates/honk/src/native/ir/cov_c5.rs`,
  `crates/honk/src/cov_c6.rs`, `crates/honk/src/cov_t1.rs`,
  `crates/honk/src/bin_tests/cov_c6_bin.rs`, and `crates/honk/tests/cov_*.rs`.

## Current coverage

Branch coverage of `crates/hatch/src` and `crates/honk/src`, excluding test
code. "Parity" counts only code exercised while compiling programs whose
output is checked byte-for-byte against hoonc.

| Measure | Before | Now |
|---|---|---|
| Unit tests: lines | 72.1% | 95.1% |
| Unit tests: branches | 55.9% (2,530 of 5,736 missed) | 89.7% (607 of 5,918 missed) |
| Parity corpus: lines | 65.9% | 76.8% |
| Parity corpus: branches | 51.8% (2,552 of 5,292 missed) | 66.9% (1,808 of 5,458 missed) |

The unit tests reach every line and branch outcome the parity corpus reaches.
No ledger entry is left as "not yet covered": every remaining gap has a
reason. Most of the parity gap is build tooling and CLI paths that cannot
change compiled output, internal forms no source program produces (`%hand`,
noun decoding, `HONK_IR_ROUNDTRIP` checks), library API the binary never
calls, dead or test-only code, and defensive checks.

## How it is measured

Coverage uses LLVM source-based instrumentation with branch coverage
(`-C instrument-coverage -Z coverage-options=branch`, on the pinned nightly)
applied only to the hatch and honk crates through a `RUSTC_WRAPPER`, then
`llvm-profdata merge` and `llvm-cov report --show-branch-summary` from the
toolchain's `llvm-tools`.

- Unit: `cargo test -p hatch -p honk` with the instrumented build, reporting
  over the test binaries and the `honk`/`hatch` executables (integration tests
  run the built binary).
- Parity: an instrumented release `honk` replays every hoonc-checked compile:
  the six kernels, the hoon-138 native self-mint, the curated type probes and
  compiler fixtures, the rejection corpus, the correctness regressions, every
  coverage package probe, and the `c6` import and kernel pairings, rejection
  trees, and dynock pairings. The `c6` batch genrule is not replayed.
