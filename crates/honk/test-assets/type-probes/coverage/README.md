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
| Unit tests: lines | 72.1% | 94.8% |
| Unit tests: branches | 55.9% (2,530 of 5,736 missed) | 89.2% (648 of 6,016 missed) |
| Parity corpus: lines | 65.9% | 76.1% |
| Parity corpus: branches | 51.8% (2,552 of 5,292 missed) | 66.1% (1,883 of 5,556 missed) |

The unit tests reach every line and branch outcome the parity corpus reaches.
No ledger entry is left as "not yet covered": every remaining gap has a
reason. Most of the parity gap is build tooling, CLI and `HONK_MEMO_VERIFY`
paths that cannot change compiled output, internal forms no source program produces (`%hand`,
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

## Mutation testing

Coverage shows which code runs; `cargo mutants` shows whether the tests would
notice if it changed. A run over all of `crates/honk/src` (3,891 mutants,
79 minutes at 32 jobs on a 128-core machine):

```
cargo mutants -p honk -j 32 --timeout 180 \
  -C=--config=profile.dev.opt-level=1 -C=--config=profile.dev.debug=0 \
  -C=--config=profile.dev.lto=false -C=--config=profile.dev.codegen-units=256 \
  -C=--config=profile.dev.incremental=true \
  -- --lib --bins --tests -- \
  --skip c6_cli_dynamic_wrapper_dumps_with_a_minted_prelude_formula \
  --skip c6_cli_wrapper_asset_dumps_agree
```

The profile overrides turn a mutant's rebuild from minutes (the dev profile
is opt-level 3 with thin LTO) into about 8 seconds. The two skipped tests mint
the whole prelude and take about 70 seconds each. For a pull request,
`--in-diff` limits the run to changed lines.

The tests caught 81% of the mutants that compile. Each of the 477 survivors
was then rebuilt and checked against the hoonc-checked parity corpus, under
`HONK_MEMO_VERIFY`, and (for cache keys) against the hoon-138 self-mint:

- 6 are caught by the two skipped tests, and 58 by the parity corpus or the
  self-mint, so for those 58 the Bazel parity job is what guards them.
- 78 delete a rune arm in `mint_inner`, `play_inner`, or `mull_inner` whose
  catch-all path (hatch's `open`) produces the same result.
- 121 change a cache key, signature, lookup, or store with no wrong cache hit
  and no change to the self-mint; 36 more matter only on a mug or hash
  collision; 38 are `HONK_MEMO_VERIFY` tooling or stack sizing.
- 1 (the `(Stop | Wait, None)` arm of `blow_ktsg`) makes the `^~` fold cache
  give wrong hits under `HONK_MEMO_VERIFY` without changing any output.
- 79 are in the CLI driver and pipeline plumbing (timing, tracing, `fsync`,
  artifact listings), including standard-build paths that nothing reaches.
- 8 were gaps that tests now catch: `c2_cnts_mixed_depth_edits`,
  `c3_crop_fork_of_cells`, and `c3_wthx_noun_cell_skin` (each checked against
  hoonc before it was added), the identical-source batch test in
  `tests/cov_c6_cli.rs`, and the collision tests
  `c1_lazy_resolver_bucket_needs_prefix_and_tomes_map` and
  `c4_noun_eq_compares_pairs_past_the_pair_set_threshold`.
- 52 are elsewhere in the type checker: 11 are fast paths whose fallback
  computes the same thing, and the rest (core minting, `burp`, `mull`, wet
  `redo`, fork sets, `twin`) are not yet classified.
