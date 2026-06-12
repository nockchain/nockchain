# Phase-0 native-parity harness

Supports the native-types migration (`docs/native-compiler/NATIVE-TYPES-MIGRATION.md`).

## `dual_run.sh [kernel ...]`  (just `native-parity-dual`)
The **acceptance gate** (§2.2 / RT-02): compiles each kernel with honk and
**strict `cmp`s** the output jam against the hoonc-built reference in
`assets/<name>.jam`. A difference that is *only* the Bazel sandbox dir-hash leaf
(proven via `jam-diff --kernel-parity`) is reported **WAIVED** — a named
exception, not a silent tolerant pass. Anything else FAILs. Exit nonzero on any
FAIL/MISSING.

The native-vs-noun arm (`HONK_NATIVE_IR`) is a **Phase-1 hook**: once the native
Formula/Type IR path exists, this harness also strict-cmps native-honk vs
noun-honk, not just honk-vs-hoonc.

## `regen_goldens.sh [regen|check]`  (just `native-goldens`)
Captures (`regen`) / verifies (`check`) the emitted-formula golden corpus for the
`exprs/*.hoon` fixtures, in both `--no-dbug` and `--dbug` variants (dbug is a
byte-affecting, phase-wide input — RT-15). Goldens are end-to-end (`--arbitrary`,
so they carry the prelude) and **regenerable / gitignored**. Fine-grained
per-construct formula parity (build noun formula, `cmp` to native `to_noun`) is a
Phase-1 **unit** fixture, not a CLI golden.

The real committed end-to-end goldens are the kernel references in `assets/`.
