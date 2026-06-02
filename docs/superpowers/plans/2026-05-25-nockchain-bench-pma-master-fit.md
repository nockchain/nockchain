# nockchain-bench PMA Master Fit Plan

## First-Release Decision

This branch is the first `nockchain-bench` release line. Do not preserve
compatibility with artifacts, CLI surfaces, or feature flags created only for
the old PMA-less `master` straddle.

Current `master` includes PMA, so `nockchain-bench` should look as though it
was built directly for that runtime:

- PMA replay is the only runtime path.
- `pma-runtime-compat` is deleted after all cfg sites are removed.
- `runtime_compat.rs` is renamed to the PMA replay module name used by callers.
- Validation artifacts do not include PMA-compat feature booleans.
- Requested cases do not include checkpoint-cadence knobs.
- Run records do not include checkpoint-cadence metrics.
- Unsupported checkpoint-production stubs are removed from the CLI.
- The canonical spec is `crates/nockchain-bench/specs/bench-harness-spec.md`.
  Older root-level harness spec snapshots are removed.

## Execution Order

1. Prove the starting branch builds with PMA enabled.
2. Remove `#[cfg(feature = "pma-runtime-compat")]` and
   `#[cfg(not(feature = "pma-runtime-compat"))]` sites while the feature entry
   still exists, so every intermediate state can build.
3. Rename `runtime_compat.rs` after enumerating all callers with
   `git grep -l runtime_compat` or `rg runtime_compat`.
4. Remove feature declarations from `Cargo.toml` only after `rg
   'pma-runtime-compat|cfg\\(feature = "pma-runtime-compat"\\)'` is empty.
5. Remove old checkpoint production and checkpoint cadence surfaces:
   `sol checkpoint`, `sol fixture build`, `enable_checkpointing`,
   `checkpoint_every_blocks`, checkpoint recovery tolerance fields, and
   checkpoint cadence run metrics.
6. Remove first-release artifact schema cruft:
   `ValidationProbeResult::pma_runtime_compat`,
   `ValidationRecord::observed_pma_runtime_compat`, and any gating based on
   those fields. Bump the validation probe version because the schema changed.
7. Update README/spec docs to describe only the current release contract.
8. Run release verification and smoke tests.

## File Map

- `Cargo.toml`
- `crates/nockchain-bench/Cargo.toml`
- `crates/nockchain-bench/src/main.rs`
- `crates/nockchain-bench/src/commands/mod.rs`
- `crates/nockchain-bench/src/commands/sol.rs`
- `crates/nockchain-bench/src/speed_of_light/bench.rs`
- `crates/nockchain-bench/src/speed_of_light/noun_compat.rs`
- `crates/nockchain-bench/src/speed_of_light/pma_replay.rs`
- `crates/nockchain-bench/src/speed_of_light/extractor.rs`
- `crates/nockchain-bench/src/speed_of_light/poke.rs`
- `crates/nockchain-bench/src/speed_of_light/harness/case.rs`
- `crates/nockchain-bench/src/speed_of_light/harness/execute.rs`
- `crates/nockchain-bench/src/speed_of_light/harness/validate.rs`
- `crates/nockchain-bench/src/speed_of_light/harness/docker.rs`
- `crates/nockchain-bench/src/speed_of_light/harness/native.rs`
- `crates/nockchain-bench/src/speed_of_light/harness/orchestrate.rs`
- `crates/nockchain-bench/src/speed_of_light/harness/sweep.rs`
- `crates/nockchain-bench/src/speed_of_light/harness/artifacts.rs`
- `crates/nockchain-bench/src/speed_of_light/harness/profiler.rs`
- `crates/nockchain-bench/src/speed_of_light/orchestrate_plan.rs`
- `crates/nockchain-bench/tests/binary_identity_build_profile.rs`
- `crates/nockchain-bench/README.md`
- `crates/nockchain-bench/specs/bench-harness-spec.md`

## Verification

Required release checks:

```bash
cargo fmt --check
cargo check -p nockchain-bench --release
cargo test -p nockchain-bench --release
cargo build -p nockchain-bench --release
```

Focused checks worth keeping during the cleanup:

```bash
cargo test -p nockchain-bench --release validation
cargo test -p nockchain-bench --release checkpoint
cargo test -p nockchain-bench --release fsync
cargo test -p nockchain-bench --release quick_orchestrate_
cargo test -p nockchain-bench --release docker_image_build_flow
```

Smoke checks:

- Native quick bench against an existing `.soltest` fixture.
- Docker trusted `sol bench` with at least three measured runs.
- `rg` audit for removed terms:

```bash
rg 'pma-runtime-compat|observed_pma_runtime_compat|pma_runtime_compat|enable_checkpointing|checkpoint_every_blocks|checkpoint_recovery|sol checkpoint|sol fixture build' crates/nockchain-bench
```
