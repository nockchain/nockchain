# Source spots and compiler parity

Native parser source spots become compiler artifact data. A spot mismatch can be the first byte-for-byte artifact difference even when generated formulas are otherwise equivalent.

## Policy

- Treat spot and `dbug` differences as parity failures.
- Fix the source-range rule that produced the difference; do not patch the comparator to ignore it.
- Keep range expansion structural. Avoid source-file or source-line special cases.
- When narrowing a source-range rule, preserve regressions for both the expanded case and nearby cases that must not expand.

## Current parser/compiler bridge rule of thumb

The native parser's `LineMap` may expand a rune span backward over doc-comment blocks when canonical compiler spots do the same. This is compiler-parity behavior, not merely parser presentation behavior, so the notes live here.

Important constraints:

- A doc block that is part of an arm body's canonical spot may be included in the arm body spot.
- Plain comments under ordinary arms should not be swept into following body runes just because they are adjacent.
- The caret-after-plus-header case is intentionally narrow: it should match the canonical doc-block shape without broadening ordinary arm comments.
- Parser helpers that detect an arm body start should prefer the first doc content line only when that is the canonical body anchor; otherwise they should return the actual body rune.

Relevant implementation area:

- `crates/hatch/src/utils.rs`

## Test expectations

For parser changes that affect compiler parity, run parser unit tests in release mode and at least one byte-for-byte compiler artifact parity target:

```bash
cargo test --release -p hatch --lib
bazel test //crates/honk/test-assets:hoon_138_arbitrary_parity_test
```
