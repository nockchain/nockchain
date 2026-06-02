# nockchain-bench Snapshot Boot Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement snapshot boot support in `nockchain-bench` anywhere checkpoint boot is currently accepted, while preserving checkpoint support.

**Architecture:** Add one bench-side boot-source model that normalizes checkpoint and explicit snapshot inputs into trusted plan inputs, then dispatches runtime setup through checkpoint or snapshot replay helpers. Snapshot replay remains behind a narrow `nockapp` API that verifies a `snapshot.pma` plus `snapshot.manifest` pair, copies the PMA into a writable per-run replay PMA, and opens it as existing PMA state with restore metadata. Keep `.soltest` checkpoint-shaped for this release.

**Tech Stack:** Rust workspace crates `nockapp` and `nockchain-bench`, Clap, Serde JSON, Docker harness staging, Python `scripts/bench_pages`, release-mode smoke tests.

---

## Inputs

Primary spec:

- `docs/superpowers/plans/2026-05-26-nockchain-bench-snapshot-boot-spec.md`

Research note:

- `docs/superpowers/plans/2026-05-26-nockchain-bench-snapshot-research.md`

Reference snapshot test bundle:

```text
/shared/nockchain/snapshots/first-100-v0-full-checkpoint-no-mempool/
  archive.solarch
  kernel.jam
  snapshot.manifest
  snapshot.pma
```

Use the reference bundle for local smoke commands throughout this plan.

## Guardrails

- Do not change `.soltest` layout or add snapshot sections to fixtures.
- Do not add event-log replay for explicit snapshot boot.
- Do not expose `nockapp::snapshot::SnapshotManifest` or broad snapshot internals publicly.
- Do not optimize snapshot PMA copies with reflinks or reuse in this first implementation.
- Do not preserve compatibility with old benchmark JSON schema shapes.
- Remove `RunBoot::default()` instead of inventing an arbitrary enum default.
- Keep `ResolvedOrchestrate.source_kind` orthogonal to `boot_source`.
- Use release builds and release binaries for smoke commands.

## File Map

### nockapp Snapshot Boundary

- Modify `crates/nockapp/src/snapshot.rs`
  - Factor production snapshot restore into an explicit path-based helper.
  - Keep helper crate-private.
- Modify `crates/nockapp/src/kernel/form.rs`
  - Add public snapshot replay inspection and PMA config helpers.
  - Keep `SnapshotManifest` hidden behind returned summary/config types.
- Modify `crates/nockapp/src/kernel/boot.rs`
  - Add or extend tests only if helper behavior is easiest to validate through boot.

### Bench Boot Model and CLI

- Modify `crates/nockchain-bench/src/main.rs`
  - Add Clap fields/tests for snapshot flags where CLI parsing currently lives.
- Modify `crates/nockchain-bench/src/commands/sol.rs`
  - Add `--snapshot-pma` and `--snapshot-manifest` to `extract`, `quick-read-bench`, `bench`, and hidden `quick-read-once`.
  - Enforce source exclusivity for the snapshot pair.
- Create or modify `crates/nockchain-bench/src/speed_of_light/boot_source.rs`
  - Centralize `BootSourceInput`, normalized trusted boot source, validation, hashing, and path canonicalization if no equivalent module already exists.
- Modify `crates/nockchain-bench/src/speed_of_light/mod.rs`
  - Export the new boot-source module.

### Bench Runtime

- Modify `crates/nockchain-bench/src/speed_of_light/kernel_utils.rs`
  - Add `init_boot_source_backed_nockapp`.
  - Keep `init_nockapp` as low-level checkpoint/fresh helper during migration.
- Modify `crates/nockchain-bench/src/speed_of_light/pma_replay.rs`
  - Add snapshot replay PMA config and `init_snapshot_replay_nockapp`.
- Modify checkpoint callers:
  - `crates/nockchain-bench/src/speed_of_light/peek_bench.rs`
  - `crates/nockchain-bench/src/speed_of_light/bench.rs`
  - `crates/nockchain-bench/src/speed_of_light/extractor.rs`
  - `crates/nockchain-bench/src/speed_of_light/orchestrator.rs` only where runtime dispatch can change without changing trusted-plan or run-result schemas.

### Trusted Harness, Docker, Sweep, Provenance

- Modify `crates/nockchain-bench/src/speed_of_light/harness/case.rs`
- Modify `crates/nockchain-bench/src/speed_of_light/orchestrate_plan.rs`
- Modify `crates/nockchain-bench/src/speed_of_light/harness/orchestrate.rs`
- Modify `crates/nockchain-bench/src/speed_of_light/harness/native.rs`
- Modify `crates/nockchain-bench/src/speed_of_light/harness/docker.rs`
- Modify `crates/nockchain-bench/src/speed_of_light/harness/provenance.rs`
- Modify `crates/nockchain-bench/src/speed_of_light/harness/sweep.rs`

### Bench Pages and Docs

- Modify `scripts/bench_pages/src/bench_pages/loader.py`
- Modify `scripts/bench_pages/src/bench_pages/render.py`
- Modify `scripts/bench_pages/src/bench_pages/manifest.py`
- Modify tests under `scripts/bench_pages/tests/`
- Modify `crates/nockchain-bench/specs/bench-harness-spec.md`

## Task 1: nockapp Snapshot Replay API

**Files:**

- Modify: `crates/nockapp/src/snapshot.rs`
- Modify: `crates/nockapp/src/kernel/form.rs`
- Test: `crates/nockapp/src/kernel/form.rs`
- Test: `crates/nockapp/src/snapshot.rs`

- [ ] **Step 1: Add failing tests for replay helper shape**

Add focused tests that create a small real snapshot using existing nockapp test helpers, then assert:

- `inspect_snapshot_replay_source(snapshot.pma, snapshot.manifest)` returns `event_num`.
- missing manifest fails.
- missing PMA fails.
- manifest checksum/verification failure fails.
- manifest/PMA `pma_words` mismatch fails.
- manifest/PMA `alloc_words` mismatch fails.
- used-prefix hash mismatch fails.
- `PmaConfig::for_snapshot_replay(...)` copies the source PMA into the requested replay path.
- returned `PmaConfig` has `open_existing = true`, `create_snapshots = false`, `rotating_snapshot_interval_event_time = None`, and restore metadata populated.
- stale `0.meta`, `1.pma`, and `1.meta` in the replay directory are removed or replaced according to existing production restore semantics.

Place tests near the PMA/snapshot tests already in `crates/nockapp/src/kernel/form.rs` unless a private helper test in `snapshot.rs` is simpler.

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test -p nockapp --release snapshot_replay
```

Expected: fails because the new public functions do not exist.

- [ ] **Step 3: Factor explicit-path snapshot restore**

In `crates/nockapp/src/snapshot.rs`, factor the current production restore into:

```rust
pub(crate) fn restore_verified_snapshot_from_paths(
    manifest_path: &Path,
    pma_path: &Path,
    operative_pma_path: &Path,
) -> Result<SnapshotManifest, SnapshotRestoreError>
```

Keep existing production `restore_verified_snapshot(&ReadySnapshotRecord, &Path)` as a thin wrapper that calls the new path helper. The path helper should perform the same Full verification production restore already performs; do not add a mode parameter to this private helper.

- [ ] **Step 4: Add public helper types and functions**

In `crates/nockapp/src/kernel/form.rs`, add public API matching the spec:

- `SnapshotReplayInfo { event_num: u64, pma_words: u64, alloc_words: u64 }`.
- `SnapshotReplayConfigError`.
- `inspect_snapshot_replay_source(snapshot_pma_path, snapshot_manifest_path)`.
- `PmaConfig::for_snapshot_replay(snapshot_pma_path, snapshot_manifest_path, replay_pma_0, replay_pma_1, words, gc_interval, fsync_enabled)`.

Pin verification modes:

- `inspect_snapshot_replay_source`: `SnapshotVerifyMode::Fast`.
- `for_snapshot_replay`: `SnapshotVerifyMode::Full`.

Make the process-global fsync side effect explicit in code comments and implementation:

```rust
durability::set_fsync_disabled(!fsync_enabled);
```

- [ ] **Step 5: Run focused tests**

Run:

```bash
cargo test -p nockapp --release snapshot_replay
```

Expected: pass.

- [ ] **Step 6: Run broader nockapp snapshot tests**

Run:

```bash
cargo test -p nockapp --release snapshot
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add crates/nockapp/src/snapshot.rs crates/nockapp/src/kernel/form.rs crates/nockapp/src/kernel/boot.rs
git commit -m "feat(nockapp): expose snapshot replay PMA helper"
```

## Task 2: Bench Boot Source Model

**Files:**

- Create or modify: `crates/nockchain-bench/src/speed_of_light/boot_source.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/mod.rs`
- Modify tests in nearby Rust modules as appropriate.

- [ ] **Step 1: Add failing tests for schema parsing and normalization**

Add tests for:

- authored checkpoint JSON:

```json
{"type":"checkpoint","checkpoint":"checkpoint.chkjam"}
```

- authored snapshot JSON:

```json
{"type":"snapshot","pma":"snapshot.pma","manifest":"snapshot.manifest"}
```

- trusted checkpoint source maps to `checkpoint-0`.
- trusted snapshot source maps to `snapshot-pma-0` and `snapshot-manifest-0`.
- snapshot input IDs are stable even when source file extensions are missing or unusual.
- manifest/PMA inspection populates snapshot `event_num`.

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test -p nockchain-bench --release boot_source
```

Expected: fails because the module/types do not exist.

- [ ] **Step 3: Implement boot source types**

Add the v2 types from the spec:

- `BootSourceInput`
- `TrustedBootSource`
- `BootSourceKind`
- resolved/normalized summary with `event_num`

Derive `Serialize`, `Deserialize`, `Debug`, `Clone`, `PartialEq`, `Eq` where useful for tests.

- [ ] **Step 4: Implement normalization**

Implement normalization that:

- canonicalizes paths.
- rejects incomplete snapshot pairs.
- calls checkpoint inspection for checkpoint event metadata.
- calls `nockapp::kernel::form::inspect_snapshot_replay_source` for snapshot event metadata.
- records file hashes for both snapshot files in trusted plan hashing inputs.
- includes snapshot `event_num` in canonical trusted plan hashing.

- [ ] **Step 5: Run focused tests**

Run:

```bash
cargo test -p nockchain-bench --release boot_source
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/nockchain-bench/src/speed_of_light/boot_source.rs crates/nockchain-bench/src/speed_of_light/mod.rs
git commit -m "feat(bench): add boot source model"
```

## Task 3: CLI Flags and Source Validation

**Files:**

- Modify: `crates/nockchain-bench/src/main.rs`
- Modify: `crates/nockchain-bench/src/commands/sol.rs`
- Test: existing CLI parse tests in those files.

- [ ] **Step 1: Add failing Clap tests**

Add parse/validation tests for:

- `sol quick-read-bench --snapshot-pma ... --snapshot-manifest ... --kernel ...`
- `sol bench --snapshot-pma ... --snapshot-manifest ... --kernel ...`
- `sol extract --snapshot-pma ... --snapshot-manifest ... --kernel ...`
- hidden `quick-read-once` if it has direct checkpoint flags today.
- `--snapshot-pma` without `--snapshot-manifest` fails.
- `--snapshot-manifest` without `--snapshot-pma` fails.
- snapshot pair conflicts with `--checkpoint`.

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test -p nockchain-bench --release commands::sol
```

Expected: fails before flags exist.

- [ ] **Step 3: Add snapshot flags**

Add:

```rust
#[arg(long, requires = "snapshot_manifest", conflicts_with = "checkpoint")]
snapshot_pma: Option<PathBuf>,

#[arg(long, requires = "snapshot_pma", conflicts_with = "checkpoint")]
snapshot_manifest: Option<PathBuf>,
```

Adjust exact field names to match existing Clap style.

- [ ] **Step 4: Update source exclusivity**

For `sol bench`, count the snapshot pair as one workload source:

```rust
let snapshot_pair = snapshot_pma.is_some() && snapshot_manifest.is_some();
let source_count = [
    plan.is_some(),
    fixture.is_some(),
    checkpoint.is_some(),
    snapshot_pair,
]
.into_iter()
.filter(|present| *present)
.count();
```

Let Clap handle incomplete snapshot pairs before this counter runs.

- [ ] **Step 5: Keep dry-run semantics explicit**

Ensure snapshot dry-run still resolves and boots enough to verify/copy the PMA, then skips measured reads.

- [ ] **Step 6: Run CLI tests**

Run:

```bash
cargo test -p nockchain-bench --release commands::sol
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add crates/nockchain-bench/src/main.rs crates/nockchain-bench/src/commands/sol.rs
git commit -m "feat(bench): add snapshot boot CLI flags"
```

## Task 4: Runtime Dispatcher and Snapshot Replay Init

**Files:**

- Modify: `crates/nockchain-bench/src/speed_of_light/kernel_utils.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/pma_replay.rs`
- Modify callers:
  - `crates/nockchain-bench/src/commands/sol.rs`
  - `crates/nockchain-bench/src/speed_of_light/peek_bench.rs`
  - `crates/nockchain-bench/src/speed_of_light/bench.rs`
  - `crates/nockchain-bench/src/speed_of_light/extractor.rs`
  - `crates/nockchain-bench/src/speed_of_light/orchestrator.rs` only if runtime dispatch needs a compile-compatible adapter.

Task boundary: do not change trusted-plan schemas, run-result schemas, schema constants, or `orchestrate_plan.rs` / `orchestrate_execute.rs` in this task. If `orchestrator.rs` needs changes here, limit them to runtime dispatch that compiles against the old checkpoint-shaped trusted plan. Schema migration happens in Task 5.

- [ ] **Step 1: Add failing runtime tests**

Add focused tests that use the reference snapshot bundle where possible or small generated nockapp snapshots where unit tests need speed:

- snapshot replay config creates `work_dir/replay-pma/0.pma`.
- snapshot path does not call `kernel.import`.
- checkpoint path still calls existing import behavior.
- init timing wrapper includes snapshot verify/copy/open when invoked by measured setup.

- [ ] **Step 2: Run focused tests and verify failure**

Run:

```bash
cargo test -p nockchain-bench --release pma_replay kernel_utils
```

Expected: fails before dispatcher exists.

- [ ] **Step 3: Add snapshot replay PMA config**

In `pma_replay.rs`, add:

- `SnapshotReplayInfo`
- `snapshot_replay_pma_config(work_dir, snapshot_pma, snapshot_manifest, fsync)`
- `init_snapshot_replay_nockapp(...)`

The helper must:

- create `work_dir/replay-pma`.
- pass `replay-pma/0.pma` and `replay-pma/1.pma` to `PmaConfig::for_snapshot_replay`.
- pass explicit `words` and `gc_interval` through to `PmaConfig::for_snapshot_replay`, matching checkpoint replay sizing behavior.
- build hot state with the existing `produce_prover_hot_state()` path.
- copy and verify inside the init path, not during plan parsing.
- open PMA existing state.
- not import checkpoint state.

- [ ] **Step 4: Add boot-source dispatcher**

In `kernel_utils.rs`, add:

```rust
init_boot_source_backed_nockapp(boot_source, kernel, work_dir, fsync, ...)
```

Dispatch:

- checkpoint -> existing checkpoint-backed init.
- snapshot -> new snapshot replay init.

Keep `init_nockapp` as the low-level helper. Migrate user-facing paths to the dispatcher.

- [ ] **Step 5: Update quick-read and extraction callers**

Update all paths that currently accept `--checkpoint` to build a `BootSourceInput` and call the dispatcher:

- CLI config construction in `crates/nockchain-bench/src/commands/sol.rs`.
- quick read
- quick read once
- extraction
- simple bench path

Do not migrate trusted orchestrator schema consumption in this task. Keep any orchestrator edits compile-compatible with the pre-v2 trusted plan; Task 5 performs the full trusted-plan/run-result shape swap.

Task 4 may leave quick-read profile JSON in an interim shape while proving snapshot runtime setup works. Task 5 finalizes the public quick-read profile schema.

- [ ] **Step 6: Run focused tests**

Run:

```bash
cargo test -p nockchain-bench --release pma_replay kernel_utils
```

Expected: pass.

- [ ] **Step 7: Run native dry-run smoke**

Build the release binary first so the smoke does not accidentally run a stale binary:

```bash
cargo build -p nockchain-bench --release
```

Run:

```bash
/shared/nockchain/target/release/nockchain-bench sol quick-read-bench \
  --snapshot-pma /shared/nockchain/snapshots/first-100-v0-full-checkpoint-no-mempool/snapshot.pma \
  --snapshot-manifest /shared/nockchain/snapshots/first-100-v0-full-checkpoint-no-mempool/snapshot.manifest \
  --kernel /shared/nockchain/snapshots/first-100-v0-full-checkpoint-no-mempool/kernel.jam \
  --start-height 1 \
  --count 1 \
  --dry-run
```

Expected: setup succeeds and measured peek loop is skipped.

- [ ] **Step 8: Commit**

```bash
git add crates/nockchain-bench/src/commands/sol.rs crates/nockchain-bench/src/speed_of_light/kernel_utils.rs crates/nockchain-bench/src/speed_of_light/pma_replay.rs crates/nockchain-bench/src/speed_of_light/peek_bench.rs crates/nockchain-bench/src/speed_of_light/bench.rs crates/nockchain-bench/src/speed_of_light/extractor.rs crates/nockchain-bench/src/speed_of_light/orchestrator.rs
git commit -m "feat(bench): boot runtime from checkpoint or snapshot"
```

## Task 5: Trusted Plan and Run Schema v2

**Files:**

- Modify: `crates/nockchain-bench/src/speed_of_light/orchestrate_plan.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/orchestrate_execute.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/orchestrator.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/peek_bench.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/bench.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/harness/case.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/harness/mod.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/mod.rs` if schema constant re-exports need updates.
- Modify: `crates/nockchain-bench/src/main.rs` if existing CLI/JSON tests assert the old quick-read, quick-orchestrate, or run-result shape.

- [ ] **Step 1: Add failing schema tests**

Add tests for:

- trusted plan v2 checkpoint shape.
- trusted plan v2 snapshot shape.
- run result `boot.source` checkpoint shape.
- run result `boot.source` snapshot shape.
- explicit `RunBoot` construction from `TrustedBootSource`; removing `Default` is enforced by compilation.
- quick-read profile uses raw `BootSourceInput`.
- quick-orchestrate wire uses `RunBoot`-shaped boot object.

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test -p nockchain-bench --release orchestrate_plan orchestrate_execute harness::case
cargo test -p nockchain-bench --release orchestrator
cargo test -p nockchain-bench --release quick_read
```

Expected: fails on old schema shapes.

- [ ] **Step 3: Bump schema constants**

Bump all constants named in the spec:

- `ORCHESTRATE_PLAN_INPUT_SCHEMA_VERSION` in `crates/nockchain-bench/src/speed_of_light/orchestrate_plan.rs`.
- `TRUSTED_PLAN_SCHEMA_VERSION` in `crates/nockchain-bench/src/speed_of_light/orchestrate_plan.rs`.
- `RUN_RESULT_SCHEMA_VERSION` in `crates/nockchain-bench/src/speed_of_light/orchestrate_execute.rs`.
- `REQUESTED_CASE_SCHEMA_VERSION` in `crates/nockchain-bench/src/speed_of_light/harness/mod.rs`.
- `RESOLVED_CASE_SCHEMA_VERSION` in `crates/nockchain-bench/src/speed_of_light/harness/mod.rs`.
- `PROVENANCE_SCHEMA_VERSION` in `crates/nockchain-bench/src/speed_of_light/harness/mod.rs`.

Check `crates/nockchain-bench/src/speed_of_light/mod.rs` for re-exports or public references that need no value change but may need imports adjusted.

Confirm `QuickOrchestrateResultsWire` still has no independent schema constant. If a constant appears during implementation, bump it and update the spec with the exact name.

- [ ] **Step 4: Replace checkpoint fields with boot source**

Replace trusted plan `checkpoint_input_id` with:

```rust
source: TrustedBootSource
```

Update quick-orchestrate wire output in `crates/nockchain-bench/src/speed_of_light/orchestrator.rs`:

- replace `QuickOrchestrateResults.checkpoint_path` with boot-source data.
- replace `BootWire { checkpoint, kernel, fsync, init_time_secs }` with `BootWire { source, kernel, fsync, init_time_secs }`.
- update compact JSON emission.
- update printed summary from `Checkpoint:` to `Boot: checkpoint` or `Boot: snapshot`, plus the relevant source paths and kernel.

Update quick-read profile emission in `peek_bench.rs` and any related `bench.rs` run-record/profile code so checkpoint and snapshot profile JSON use the raw `BootSourceInput` shape described in the spec.

For run records, replace:

```rust
checkpoint_input_id: String
```

with:

```rust
source: TrustedBootSource
```

- [ ] **Step 5: Add snapshot input roles**

In `crates/nockchain-bench/src/speed_of_light/orchestrate_plan.rs`, add these variants next to the existing `InputRole` and `ResolvedInput` definitions:

- `SnapshotPma`
- `SnapshotManifest`

Stable input IDs:

- `snapshot-pma-0`
- `snapshot-manifest-0`

Do not move `InputRole` into `boot_source.rs`; downstream trusted plan JSON serialization already belongs to `orchestrate_plan.rs`.

If the variants are not constructed until Task 6, add a narrow `#[allow(dead_code)]` note or an immediate unit test that constructs them so the Task 5 commit remains warning-clean under the repo's lint settings.

Update the `InputRole` to string/serde match arms in `orchestrate_plan.rs` and any other exhaustive `InputRole` matches at the same time as adding the variants.

- [ ] **Step 6: Remove `RunBoot::default()`**

Remove `Default` from `RunBoot`. Replace call sites in `orchestrate_execute.rs` with explicit construction from the resolved `TrustedBootSource`, or carry `Option<TrustedBootSource>` until populated. Do not silently choose checkpoint as a default.

Known locations from the spec:

- `crates/nockchain-bench/src/speed_of_light/orchestrate_execute.rs:75-85`
- call sites around `:526`
- call sites around `:1661`

- [ ] **Step 7: Update plan hashing**

Ensure trusted plan canonical hashing includes:

- snapshot PMA file hash.
- snapshot manifest file hash.
- snapshot `event_num`.
- existing checkpoint and kernel hashes.

- [ ] **Step 8: Preserve orthogonal source fields**

Keep:

- `ResolvedOrchestrate.source_kind`: workload origin (`generated_read`, `generated_replay`, `plan_file`).
- `ResolvedOrchestrate.boot_source`: runtime boot kind (`checkpoint`, `snapshot`).

- [ ] **Step 9: Run schema tests**

Run:

```bash
cargo test -p nockchain-bench --release orchestrate_plan orchestrate_execute harness::case
cargo test -p nockchain-bench --release orchestrator
cargo test -p nockchain-bench --release quick_read
```

Expected: pass.

- [ ] **Step 10: Commit**

```bash
git add crates/nockchain-bench/src/main.rs crates/nockchain-bench/src/speed_of_light/orchestrate_plan.rs crates/nockchain-bench/src/speed_of_light/orchestrate_execute.rs crates/nockchain-bench/src/speed_of_light/orchestrator.rs crates/nockchain-bench/src/speed_of_light/peek_bench.rs crates/nockchain-bench/src/speed_of_light/bench.rs crates/nockchain-bench/src/speed_of_light/harness/case.rs crates/nockchain-bench/src/speed_of_light/harness/mod.rs crates/nockchain-bench/src/speed_of_light/mod.rs
git commit -m "feat(bench): use boot source schema v2"
```

## Task 6: Native and Docker Input Staging

**Files:**

- Modify: `crates/nockchain-bench/src/speed_of_light/harness/native.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/harness/docker.rs`
- Modify: `crates/nockchain-bench/src/commands/sol.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/orchestrate_plan.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/harness/orchestrate.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/harness/case.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/harness/sweep.rs`

- [ ] **Step 1: Add failing staging tests**

Add tests for:

- native snapshot inputs are copied or referenced as read-only source inputs.
- Docker trusted inputs mount both snapshot files read-only.
- container paths are role-derived and stable:
  - `/bench/input/files/snapshot-pma-0.pma`
  - `/bench/input/files/snapshot-manifest-0.manifest`
- missing or odd file extensions still stage with `.pma` and `.manifest` role-derived extensions.
- `docker-tmpfs` large snapshot warning triggers using parsed bytes from the existing memory-limit parser.

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test -p nockchain-bench --release harness
cargo test -p nockchain-bench --release orchestrate_plan
```

Expected: fails before native/Docker staging uses the snapshot roles.

- [ ] **Step 3: Use snapshot input roles in staging**

Use the roles added in Task 5:

- `SnapshotPma`
- `SnapshotManifest`

Stable input IDs:

- `snapshot-pma-0`
- `snapshot-manifest-0`

Do not create a new `harness/input.rs` file.

- [ ] **Step 4: Migrate generated-read requested cases**

Change `RequestedOrchestrate::GeneratedRead` in `crates/nockchain-bench/src/speed_of_light/harness/case.rs` from a checkpoint-only field to a boot source field:

- replace `checkpoint_path` with `boot: BootSourceInput` or the local equivalent.
- update serialization tests for requested/resolved case v2.
- update `placeholder_resolved_orchestrate` so checkpoint boot emits the checkpoint input and snapshot boot emits both `snapshot-pma-0` and `snapshot-manifest-0`.
- keep `source_kind = "generated_read"` unchanged.

- [ ] **Step 5: Migrate generated-read planning options and call sites**

Decision: keep `GeneratedReadOptions` checkpoint-only through Task 5. Migrate it in Task 6 together with `RequestedOrchestrate::GeneratedRead` so commit boundaries stay compile-clean.

In `crates/nockchain-bench/src/speed_of_light/orchestrate_plan.rs`, replace `GeneratedReadOptions.checkpoint_path` with `boot: BootSourceInput` or the local equivalent.

Update all constructors/destructures in the same commit:

- `crates/nockchain-bench/src/commands/sol.rs`: wire `--checkpoint` or the `--snapshot-pma`/`--snapshot-manifest` pair into the generated-read `boot` field, and update printed plan summaries from `Checkpoint:` to `Boot: checkpoint` / `Boot: snapshot`.
- `crates/nockchain-bench/src/speed_of_light/harness/orchestrate.rs`: construct `GeneratedReadOptions { boot, kernel_path, ... }` and stage snapshot input records for read shorthand.
- `crates/nockchain-bench/src/speed_of_light/harness/sweep.rs`: update `SweepBaseCase::into_requested_case` construction of `RequestedOrchestrate::GeneratedRead` to pass `boot`.

- [ ] **Step 6: Update native staging**

For generated read with snapshot boot:

- canonicalize snapshot PMA, manifest, and kernel.
- materialize planning-time replay in `output/input/read-tip-work/replay-pma`.
- exclude planning-time copy from any run `init_time_secs`.
- delete `output/input/read-tip-work` immediately after trusted plan and resolved metadata are emitted. Deletion is best-effort on failure paths and must not gate successful trusted-plan emission.
- do not reuse planning PMA for measured runs.

- [ ] **Step 7: Update Docker staging**

Update Docker input rewrite to set `container_path` for every snapshot input. Mount both source files read-only and copy the PMA into per-run replay PMA inside the container before opening it.

Update generated-read shorthand staging in `crates/nockchain-bench/src/speed_of_light/harness/orchestrate.rs`, including the checkpoint input construction area, so snapshot read shorthand emits both snapshot input records.

- [ ] **Step 8: Add docker-tmpfs warning**

Warn, do not fail, when:

- boot source is snapshot.
- work-dir mode is `docker-tmpfs`.
- source snapshot PMA size exceeds 50 percent of parsed `--memory-limit`.

Use the existing memory-limit parser; do not compare raw strings.

Emit this through the existing requested-case validation or resolve warning path that operators already see during `sol bench`. Do not add a new bench_pages warning schema for this release unless the existing warning plumbing already exposes one.

- [ ] **Step 9: Run staging tests**

Run:

```bash
cargo test -p nockchain-bench --release harness
cargo test -p nockchain-bench --release orchestrate_plan
```

Expected: pass.

- [ ] **Step 10: Commit**

```bash
git add crates/nockchain-bench/src/commands/sol.rs crates/nockchain-bench/src/speed_of_light/harness/native.rs crates/nockchain-bench/src/speed_of_light/harness/docker.rs crates/nockchain-bench/src/speed_of_light/harness/orchestrate.rs crates/nockchain-bench/src/speed_of_light/orchestrate_plan.rs crates/nockchain-bench/src/speed_of_light/harness/case.rs crates/nockchain-bench/src/speed_of_light/harness/sweep.rs
git commit -m "feat(bench): stage snapshot inputs for native and docker"
```

## Task 7: Sweep Snapshot Base and Axis

**Files:**

- Modify: `crates/nockchain-bench/src/speed_of_light/harness/sweep.rs`

- [ ] **Step 1: Add failing sweep tests**

Add tests for:

- static sweep base shape:

```json
{
  "snapshot": {
    "pma": "snapshot-a.pma",
    "manifest": "snapshot-a.manifest"
  }
}
```

- single atomic `snapshot` object axis with cells:

```json
{
  "pma": "snapshot-a.pma",
  "manifest": "snapshot-a.manifest"
}
```

- rejecting independent `snapshot.pma` and `snapshot.manifest` axes for this release.
- snapshot pair co-varies as one cell.
- invariant equality compares the object cell, not individual path strings.
- label fallback uses manifest stem. Plan-derived collision guidance: if stems collide, use parent directory plus stem; if still colliding, append zero-based axis index.

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test -p nockchain-bench --release harness::sweep
```

Expected: fails before atomic snapshot axis exists.

- [ ] **Step 3: Implement snapshot base parsing**

Add `snapshot: Option<SnapshotPair>` or equivalent to `SweepBaseCase` and to the mirrored `SweepBaseCaseSerde` deserialize helper. Update the manual `Deserialize` impl to copy the parsed snapshot pair into `SweepBaseCase`.

Include `snapshot_pair` in the exactly-one-source counter in `SweepBaseCase::into_requested_case`, alongside plan/fixture/checkpoint. The base snapshot pair must flow into the requested case as the static boot source.

- [ ] **Step 4: Implement atomic axis parsing**

Extend sweep parsing to recognize a `snapshot` axis whose values are JSON objects with `pma` and `manifest` strings. Do not use the existing one-path `path_value(axis, value)` path for this object without adapting it deliberately.

- [ ] **Step 5: Implement labels and invariants**

Labels:

- explicit case label wins.
- otherwise manifest stem.
- on collision, parent directory plus stem.
- if still colliding, append zero-based axis index.

Invariants compare the normalized object pair.

- [ ] **Step 6: Run sweep tests**

Run:

```bash
cargo test -p nockchain-bench --release harness::sweep
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add crates/nockchain-bench/src/speed_of_light/harness/sweep.rs
git commit -m "feat(bench): add snapshot sweep base and axis"
```

## Task 8: Provenance and bench_pages

**Files:**

- Modify: `crates/nockchain-bench/src/speed_of_light/harness/provenance.rs`
- Modify: `scripts/bench_pages/src/bench_pages/loader.py`
- Modify: `scripts/bench_pages/src/bench_pages/render.py`
- Modify: `scripts/bench_pages/src/bench_pages/manifest.py`
- Modify: `scripts/bench_pages/tests/test_manifest.py`
- Modify: `scripts/bench_pages/tests/test_loader.py`
- Modify: `scripts/bench_pages/tests/test_render.py`

- [ ] **Step 1: Add failing provenance tests**

Add tests for:

- checkpoint provenance reports `boot_source = "checkpoint"` and checkpoint event.
- snapshot provenance reports `boot_source = "snapshot"` and manifest event.
- `phase2_pma_provenance` reads from resolved boot fields, not fixture checkpoint metadata.

- [ ] **Step 2: Add failing bench_pages tests**

Rewrite checkpoint-only assertions into v2 boot-shape tests:

- checkpoint run renders `Boot from checkpoint checkpoint-0 using kernel-0`.
- snapshot run renders `Boot from snapshot snapshot-pma-0 + snapshot-manifest-0 using kernel-0`.
- loader accepts wrapped `RunBoot` shape.
- readable boot line handles unknown/future boot source gracefully.

- [ ] **Step 3: Run tests and verify failure**

Run:

```bash
cargo test -p nockchain-bench --release harness::provenance
uv run pytest scripts/bench_pages/tests
```

Expected: fails before provenance/pages update.

- [ ] **Step 4: Update provenance**

Add or update constructors:

```rust
PmaProvenance::checkpoint(boot_event_num)
PmaProvenance::snapshot(boot_event_num)
```

Populate `boot_source` and `boot_event_num` from resolved boot source.

- [ ] **Step 5: Update bench_pages loader/rendering**

Update readable boot rendering to branch on:

```python
source = boot.get("source", {})
match source.get("type"):
    case "checkpoint":
        ...
    case "snapshot":
        ...
    case None:
        return f"Boot source unknown using {kernel_id}"
    case _:
        return f"Boot source unknown using {kernel_id}"
```

Update manifest/provenance extraction in `scripts/bench_pages/src/bench_pages/manifest.py` so v2 provenance contributes the correct `boot_source` and `boot_event_num`; rendering alone is not enough.

Keep checkpoint rendering explicit and add snapshot rendering rather than replacing checkpoint coverage.

- [ ] **Step 6: Run tests**

Run:

```bash
cargo test -p nockchain-bench --release harness::provenance
uv run pytest scripts/bench_pages/tests
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add crates/nockchain-bench/src/speed_of_light/harness/provenance.rs scripts/bench_pages/src/bench_pages/loader.py scripts/bench_pages/src/bench_pages/render.py scripts/bench_pages/src/bench_pages/manifest.py scripts/bench_pages/tests
git commit -m "feat(bench-pages): render snapshot boot sources"
```

## Task 9: Fixture Non-Changes and Harness Docs

**Files:**

- Modify only if needed: `crates/nockchain-bench/src/speed_of_light/fixture.rs`
- Modify: `crates/nockchain-bench/specs/bench-harness-spec.md`

- [ ] **Step 1: Add fixture regression test if missing**

Ensure tests assert:

- `.soltest` layout remains v4.
- fixture inspect still uses checkpoint wording.
- extracted fixture output still includes `checkpoint.chkjam`, `archive.solarch`, and `kernel.jam`.
- no snapshot sections are added.

- [ ] **Step 2: Run fixture tests**

Run:

```bash
cargo test -p nockchain-bench --release fixture
```

Expected: pass or fail only for intentional wording updates.

- [ ] **Step 3: Update bench harness spec**

Add an explicit snapshot boot section to `crates/nockchain-bench/specs/bench-harness-spec.md` covering:

- public `BootSourceInput`.
- trusted `TrustedBootSource`.
- run `RunBoot` shape.
- Docker staging of both snapshot files.
- `.soltest` non-goals.
- event-log replay non-goal.
- reference bundle path for local smoke testing. This is an implementation-plan requirement so future operators know which local artifact the smoke commands target.

- [ ] **Step 4: Run docs-adjacent tests**

Run:

```bash
cargo test -p nockchain-bench --release fixture
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/nockchain-bench/specs/bench-harness-spec.md crates/nockchain-bench/src/speed_of_light/fixture.rs
git commit -m "docs(bench): document snapshot boot harness schema"
```

## Task 10: End-to-End Smoke and Fixups

**Files:**

- Any files touched by failures.

- [ ] **Step 1: Build release binary**

Run:

```bash
cargo build -p nockchain-bench --release
```

Expected: pass.

- [ ] **Step 2: Run nockapp snapshot tests**

Run:

```bash
cargo test -p nockapp --release snapshot
```

Expected: pass.

- [ ] **Step 3: Run nockchain-bench focused tests**

Run:

```bash
cargo test -p nockchain-bench --release boot_source
cargo test -p nockchain-bench --release commands::sol
cargo test -p nockchain-bench --release harness
cargo test -p nockchain-bench --release orchestrate
```

Expected: pass.

- [ ] **Step 4: Run bench_pages tests**

Run:

```bash
uv run pytest scripts/bench_pages/tests
```

Expected: pass.

- [ ] **Step 5: Run checkpoint fixture inspect smoke**

Run:

```bash
/shared/nockchain/target/release/nockchain-bench sol fixture inspect \
  /shared/nockchain/fixtures/first-100-v0-derived-checkpoint-no-mempool.soltest
```

Expected: existing checkpoint fixture output remains sensible.

- [ ] **Step 6: Run native snapshot quick-read dry-run**

Run:

```bash
/shared/nockchain/target/release/nockchain-bench sol quick-read-bench \
  --snapshot-pma /shared/nockchain/snapshots/first-100-v0-full-checkpoint-no-mempool/snapshot.pma \
  --snapshot-manifest /shared/nockchain/snapshots/first-100-v0-full-checkpoint-no-mempool/snapshot.manifest \
  --kernel /shared/nockchain/snapshots/first-100-v0-full-checkpoint-no-mempool/kernel.jam \
  --start-height 1 \
  --count 1 \
  --dry-run
```

Expected: verifies and copies the 513 MiB snapshot PMA, boots, resolves setup, then skips measured read loop.

- [ ] **Step 7: Run trusted native snapshot bench**

Ensure output directory does not exist or is empty before running.

Run:

```bash
mkdir -p /shared/nockchain/tmp/snapshot-bench-native
/shared/nockchain/target/release/nockchain-bench sol bench \
  --snapshot-pma /shared/nockchain/snapshots/first-100-v0-full-checkpoint-no-mempool/snapshot.pma \
  --snapshot-manifest /shared/nockchain/snapshots/first-100-v0-full-checkpoint-no-mempool/snapshot.manifest \
  --kernel /shared/nockchain/snapshots/first-100-v0-full-checkpoint-no-mempool/kernel.jam \
  --start-height 1 \
  --count 1 \
  --output /shared/nockchain/tmp/snapshot-bench-native \
  --warmup-runs 0 \
  --measured-runs 3 \
  --cooldown-secs 0
```

Expected: trusted plan and run results show `boot.source.type = "snapshot"`.

- [ ] **Step 8: Run trusted Docker snapshot bench**

Ensure output directory does not exist or is empty before running.

Run:

```bash
/shared/nockchain/target/release/nockchain-bench sol bench \
  --snapshot-pma /shared/nockchain/snapshots/first-100-v0-full-checkpoint-no-mempool/snapshot.pma \
  --snapshot-manifest /shared/nockchain/snapshots/first-100-v0-full-checkpoint-no-mempool/snapshot.manifest \
  --kernel /shared/nockchain/snapshots/first-100-v0-full-checkpoint-no-mempool/kernel.jam \
  --start-height 1 \
  --count 1 \
  --output /shared/nockchain/tmp/snapshot-bench-docker \
  --docker-build-tag nockchain-bench:local \
  --memory-limit 8g \
  --work-dir-mode docker-volume \
  --warmup-runs 0 \
  --measured-runs 3 \
  --cooldown-secs 0
```

If Docker is not reachable through the default client environment on this machine, rerun with the local Docker Desktop socket:

```bash
DOCKER_HOST=unix:///home/drbeefsupreme/.docker/desktop/docker.sock \
/shared/nockchain/target/release/nockchain-bench sol bench ...
```

Expected: Docker mounts snapshot PMA and manifest read-only, copies the PMA per run, and succeeds.

- [ ] **Step 9: Render one bench_pages output**

Run the existing bench_pages command used in this repo against a snapshot run output. If no single command exists, invoke the tested loader/render path used by `scripts/bench_pages/tests`.

Expected: rendered page shows snapshot boot line.

- [ ] **Step 10: Commit fixups**

If smoke revealed fixes:

```bash
git add <fixed files>
git commit -m "test(bench): verify snapshot boot release path"
```

If no code changes were required after Task 9, skip this commit.

## Final Verification Checklist

- [ ] `cargo test -p nockapp --release snapshot`
- [ ] `cargo test -p nockchain-bench --release boot_source`
- [ ] `cargo test -p nockchain-bench --release commands::sol`
- [ ] `cargo test -p nockchain-bench --release harness`
- [ ] `cargo test -p nockchain-bench --release orchestrate`
- [ ] `uv run pytest scripts/bench_pages/tests`
- [ ] native snapshot quick-read dry-run using the reference bundle
- [ ] trusted native snapshot bench using the reference bundle
- [ ] trusted Docker snapshot bench using the reference bundle
- [ ] checkpoint fixture inspect still works

## Expected Commit Series

1. `feat(nockapp): expose snapshot replay PMA helper`
2. `feat(bench): add boot source model`
3. `feat(bench): add snapshot boot CLI flags`
4. `feat(bench): boot runtime from checkpoint or snapshot`
5. `feat(bench): use boot source schema v2`
6. `feat(bench): stage snapshot inputs for native and docker`
7. `feat(bench): add snapshot sweep base and axis`
8. `feat(bench-pages): render snapshot boot sources`
9. `docs(bench): document snapshot boot harness schema`
10. Optional: `test(bench): verify snapshot boot release path`
