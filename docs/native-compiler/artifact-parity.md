# Native compiler artifact parity

## Exact equality target

For compiler parity targets, the native artifact and canonical compiler artifact must be byte-identical. The parity test for `hoon-138.hoon` arbitrary output compares artifacts with `cmp`, not hash-only or structural comparison.

Target files involved in the current `hoon-138.hoon` arbitrary parity path:

- `crates/hoonc/hoon/hoon-138.hoon`
- `hoon/common/hoon.hoon`
- `crates/honk/test-assets/BUILD.bazel`
- `crates/honk/test-assets/compiler_artifact_parity_test.sh`

`crates/hoonc/hoon/hoon-138.hoon` and `hoon/common/hoon.hoon` are the same source text. Keep that invariant in mind when diagnosing parity: a diff between artifacts is usually caused by compiler, parser, source spot, environment, or rule behavior rather than different Hoon source.

## Diagnostic workflow

1. Run the byte-for-byte parity test first:
   ```bash
   bazel test //crates/honk/test-assets:hoon_138_arbitrary_parity_test
   ```

2. If it fails, use JAM-aware structural diff tooling to find the first semantic or metadata difference.
3. If the first difference is a spot or `dbug` value, treat it as real artifact data. Do not add a comparison filter unless canonical compiler behavior also omits or normalizes that data.
4. After a fix, re-run the exact artifact parity test, then the relevant kernel parity suite:
   ```bash
   bazel test //assets/native:kernel_parity_test
   ```

## Guardrails

- Keep parity test scripts simple: resolve runfiles, verify files exist, compare exact bytes, and print hashes only as failure diagnostics.
- Avoid logic that knows about a specific source file, source line, or artifact name beyond Bazel rule wiring.
- Prefer adding a new focused parity target over broadening a script into a test harness with hidden normalization rules.
