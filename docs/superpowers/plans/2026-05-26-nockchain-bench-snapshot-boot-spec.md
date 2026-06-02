# nockchain-bench Snapshot Boot Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `nockchain-bench` boot benchmark runs from PMA snapshots anywhere it currently boots from checkpoints, while keeping checkpoint boot support.

**Architecture:** Model checkpoint and snapshot as one public `BootSource` enum. Checkpoint boot keeps the existing state-import path; snapshot boot asks `nockapp` to verify and copy `snapshot.pma` into the per-run replay PMA, then opens the PMA as existing state with the snapshot manifest wired into `PmaConfig.restore_manifest`.

**Tech Stack:** Rust workspace crates `nockchain-bench` and `nockapp`, Clap CLI, Serde JSON schemas, Docker trusted harness staging, Python `bench_pages`.

---

## Scope

This is a first-release schema change. Do not preserve compatibility with old benchmark artifact schemas.

In scope:

- Direct checkpoint boot remains supported.
- Direct snapshot boot from explicit `snapshot.pma` plus `snapshot.manifest`.
- Quick read, quick orchestrate plans, trusted plans, trusted read shorthand, extraction, native runs, Docker runs, sweeps, provenance, and `bench_pages`.
- Per-run writable replay PMA copies for snapshot runs.

Out of scope:

- Event-log replay after snapshot restore.
- Discovering snapshots from event logs or snapshot directories.
- Embedding snapshots in `.soltest`.
- Reflink/copy-on-write optimization for large PMA copies.
- Backward-compatible JSON schema deserialization.

## Reference Test Bundle

Use the small full-checkpoint-derived snapshot bundle as the default local test input while implementing this plan:

```text
/shared/nockchain/snapshots/first-100-v0-full-checkpoint-no-mempool/
  archive.solarch
  kernel.jam
  snapshot.manifest
  snapshot.pma
```

This bundle was generated from `/shared/nockchain/fixtures/first-100-v0-full-checkpoint-no-mempool.soltest`. It preserves the fixture archive range `1..=100` and checkpoint event number `5`, and is small enough for routine native and Docker smoke runs while still exercising the multi-file snapshot path.

## Existing Facts To Preserve

- Checkpoint boot in `crates/nockchain-bench/src/speed_of_light/pma_replay.rs` decodes checkpoint state into `LoadState` and calls `kernel.import(load_state)`.
- Production snapshot boot in `nockapp` verifies a manifest/PMA pair, copies the snapshot PMA to the operative PMA path, synthesizes PMA metadata from the manifest, opens existing PMA state, and rebuilds cold state fresh.
- `crates/nockapp/src/snapshot.rs` and `PmaConfig.restore_manifest` are currently crate-private, so `nockchain-bench` needs a narrow public helper rather than direct access to snapshot internals.
- `.soltest` layout v4 is checkpoint-shaped and should stay that way for this release.

## Public Boot Source Schema

Add one public boot-source model in `nockchain-bench`; use it for CLI options after parsing, quick plans, trusted plans, run records, requested cases, resolved cases, and profile output.

### Human/Authored Plan Schema

Replace top-level `checkpoint` in `OrchestratePlanInput` and `QuickOrchestratePlan` with `boot`.

Checkpoint:

```json
{
  "schema_version": "orchestrate-plan/v2",
  "boot": {
    "type": "checkpoint",
    "checkpoint": "path/to/checkpoint.chkjam"
  },
  "kernel": "path/to/kernel.jam",
  "steps": []
}
```

Snapshot:

```json
{
  "schema_version": "orchestrate-plan/v2",
  "boot": {
    "type": "snapshot",
    "pma": "path/to/snapshot.pma",
    "manifest": "path/to/snapshot.manifest"
  },
  "kernel": "path/to/kernel.jam",
  "steps": []
}
```

Rust shape:

```rust
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BootSourceInput {
    Checkpoint { checkpoint: PathBuf },
    Snapshot { pma: PathBuf, manifest: PathBuf },
}
```

### Trusted Plan Schema

Bump `TRUSTED_PLAN_SCHEMA_VERSION` to `trusted-plan/v2`.

Replace `TrustedPlanBoot.checkpoint_input_id` with a tagged source object. Keep `kernel_input_id` and `fsync`.

Checkpoint:

```json
{
  "schema_version": "trusted-plan/v2",
  "boot": {
    "source": {
      "type": "checkpoint",
      "checkpoint_input_id": "checkpoint-0",
      "event_num": 12345
    },
    "kernel_input_id": "kernel-0",
    "fsync": "on"
  },
  "inputs": [
    {
      "input_id": "checkpoint-0",
      "role": "checkpoint",
      "absolute_path": "/abs/checkpoint.chkjam",
      "sha256_hex": "...",
      "size_bytes": 123
    }
  ],
  "steps": []
}
```

Snapshot:

```json
{
  "schema_version": "trusted-plan/v2",
  "boot": {
    "source": {
      "type": "snapshot",
      "pma_input_id": "snapshot-pma-0",
      "manifest_input_id": "snapshot-manifest-0",
      "event_num": 12345
    },
    "kernel_input_id": "kernel-0",
    "fsync": "on"
  },
  "inputs": [
    {
      "input_id": "snapshot-pma-0",
      "role": "snapshot_pma",
      "absolute_path": "/abs/snapshot.pma",
      "sha256_hex": "...",
      "size_bytes": 123
    },
    {
      "input_id": "snapshot-manifest-0",
      "role": "snapshot_manifest",
      "absolute_path": "/abs/snapshot.manifest",
      "sha256_hex": "...",
      "size_bytes": 123
    }
  ],
  "steps": []
}
```

Rust shape:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrustedPlanBoot {
    pub source: TrustedBootSource,
    pub kernel_input_id: String,
    #[serde(default = "default_fsync_enabled")]
    #[serde(serialize_with = "serialize_fsync_bool", deserialize_with = "deserialize_fsync_bool")]
    pub fsync: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TrustedBootSource {
    Checkpoint {
        checkpoint_input_id: String,
        event_num: Option<u64>,
    },
    Snapshot {
        pma_input_id: String,
        manifest_input_id: String,
        event_num: u64,
    },
}
```

`event_num` is required for snapshot because it is in the manifest. It may be `None` for checkpoint only if checkpoint metadata could not be read during normalization; generated read paths should populate it.

`normalized_plan_sha256_hex` must include the entire trusted `boot.source`, including `TrustedBootSource::Snapshot.event_num`. The field is redundant with the content-hashed manifest, but including it pins canonical hashing and keeps all visible trusted-plan boot metadata covered. `step_signature_sha256_hex` remains step-only and must not include boot source or kernel input IDs.

### Input Roles

Extend `InputRole`:

```rust
pub enum InputRole {
    Checkpoint,
    SnapshotPma,
    SnapshotManifest,
    Kernel,
    Archive,
    SourcePlan,
}
```

Input ID prefixes:

- `checkpoint-0`
- `snapshot-pma-0`
- `snapshot-manifest-0`
- `kernel-0`
- `archive-0`
- `source-plan-0`

Keep inputs file-level. The logical boot source lives in `boot.source`; snapshot is not modeled as two unrelated boot choices.

### Run Record Schema

Bump `RUN_RESULT_SCHEMA_VERSION` to `run-result/v2`.

Replace `RunBoot.checkpoint_input_id` with `RunBoot.source: TrustedBootSource`.

```json
{
  "boot": {
    "source": {
      "type": "snapshot",
      "pma_input_id": "snapshot-pma-0",
      "manifest_input_id": "snapshot-manifest-0",
      "event_num": 12345
    },
    "kernel_input_id": "kernel-0",
    "fsync": "on",
    "init_time_secs": 1.23
  }
}
```

Rust shape:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunBoot {
    pub source: TrustedBootSource,
    pub kernel_input_id: String,
    #[serde(serialize_with = "serialize_fsync_bool", deserialize_with = "deserialize_fsync_bool")]
    pub fsync: bool,
    pub init_time_secs: Option<f64>,
}
```

Remove `Default` from `RunBoot`. `TrustedBootSource` has no valid natural default. Replace `RunBoot::default()` and test/helper initializers that currently synthesize a checkpoint default with explicit construction from the normalized `TrustedPlanBoot.source`. If a builder needs an incomplete `RunRecord` before boot is known, pass the boot source into that builder instead of making `source` optional in the public artifact schema.

### Requested/Resolved Case Schema

Bump requested/resolved case schema versions. Change `RequestedOrchestrate::GeneratedRead` to:

```rust
GeneratedRead {
    boot: BootSourceInput,
    kernel_path: PathBuf,
    start_height: u64,
    end_height: Option<u64>,
    count: Option<u64>,
    peek_mode: PeekMode,
}
```

Add these fields to `ResolvedOrchestrate`:

```rust
pub boot_source: Option<String>,       // "checkpoint" or "snapshot"
pub boot_event_num: Option<u64>,
```

Populate them from the normalized `TrustedPlanBoot.source` in `resolve_trusted_plan_artifact`, not from `.soltest` defaults.

Keep `ResolvedOrchestrate.source_kind` unchanged and orthogonal. It continues to describe where the workload came from (`"generated_read"`, `"generated_replay"`, or `"plan_file"`). `boot_source` describes how runtime state is booted (`"checkpoint"` or `"snapshot"`).

### Schema Constants To Bump

Bump these schema constants in the same implementation series:

- `ORCHESTRATE_PLAN_INPUT_SCHEMA_VERSION`: `orchestrate-plan/v2`
- `TRUSTED_PLAN_SCHEMA_VERSION`: `trusted-plan/v2`
- `RUN_RESULT_SCHEMA_VERSION`: `run-result/v2`
- `REQUESTED_CASE_SCHEMA_VERSION`: `requested-case/v2`
- `RESOLVED_CASE_SCHEMA_VERSION`: `resolved-case/v2`
- `PROVENANCE_SCHEMA_VERSION`: `provenance/v2`

`STEP_RESULT_SCHEMA_VERSION` and `COLD_EVIDENCE_SCHEMA_VERSION` can remain unchanged unless their payloads are modified.

`QuickOrchestrateResultsWire` currently has no public `schema_version` constant or field. Do not introduce one solely for this snapshot work. Update its compact JSON tests for the new boot object shape, but let the shape change ride with the CLI/profile-output change.

## CLI Flag Shape

Use the same boot-source flag pair everywhere the user can directly request checkpoint-backed boot.

### `sol quick-read-bench`

Keep checkpoint support and add snapshot support:

```bash
nockchain-bench sol quick-read-bench \
  --checkpoint /path/to/checkpoint.chkjam \
  --kernel /path/to/kernel.jam \
  --start-height 100 \
  --count 10
```

```bash
nockchain-bench sol quick-read-bench \
  --snapshot-pma /path/to/snapshot.pma \
  --snapshot-manifest /path/to/snapshot.manifest \
  --kernel /path/to/kernel.jam \
  --start-height 100 \
  --count 10
```

Clap rules:

- `--checkpoint` conflicts with `--snapshot-pma` and `--snapshot-manifest`.
- `--snapshot-pma` requires `--snapshot-manifest`.
- `--snapshot-manifest` requires `--snapshot-pma`.
- Exactly one boot source is required.
- Existing `--end-height` versus `--count` conflict remains.

Profile JSON should replace `checkpoint_path` with:

```json
{
  "boot": {
    "type": "snapshot",
    "pma": "/abs/snapshot.pma",
    "manifest": "/abs/snapshot.manifest"
  },
  "kernel_path": "/abs/kernel.jam"
}
```

This quick-read profile shape is intentionally the raw `BootSourceInput` plus `kernel_path`. It is not the trusted-plan/run-record boot shape and is not consumed by `bench_pages`.

For checkpoint profile JSON:

```json
{
  "boot": {
    "type": "checkpoint",
    "checkpoint": "/abs/checkpoint.chkjam"
  },
  "kernel_path": "/abs/kernel.jam"
}
```

`--dry-run` semantics remain setup-only, not peek-only. A snapshot dry run still validates the snapshot source and copies it into the temporary replay PMA because boot and tip resolution are part of setup; it then skips the measured peek loop. This makes dry-run useful for proving that a snapshot can boot.

### `sol quick-orchestrate --plan`

No new command-line flags are needed. The plan file uses `boot` as shown in the public plan schema.

`QuickOrchestrateResultsWire.boot` should become:

```json
{
  "boot": {
    "source": {
      "type": "snapshot",
      "pma": "/abs/snapshot.pma",
      "manifest": "/abs/snapshot.manifest"
    },
    "kernel": "/abs/kernel.jam",
    "fsync": "on",
    "init_time_secs": 1.23
  },
  "steps": []
}
```

This quick-orchestrate result wire shape is intentionally closer to `RunBoot`: it includes source, kernel, fsync, and init timing. It is distinct from quick-read's ad hoc profile JSON because quick-orchestrate executes a plan-shaped workload and feeds the same concepts as trusted plan/run artifacts.

The printed summary should say `Boot: snapshot`, then print `Snapshot PMA`, `Snapshot manifest`, and `Kernel`. Checkpoint summaries should say `Boot: checkpoint` and print `Checkpoint` plus `Kernel`.

### Trusted Plan Files

`sol bench --plan plan.json` accepts `orchestrate-plan/v2`. It normalizes `boot` into `trusted-plan/v2`.

The trusted plan artifact stored at `trusted_plan.json` never stores raw boot paths in `boot`; it stores input IDs and records raw paths in `inputs`.

### `sol bench` Read Shorthand

Keep checkpoint shorthand and add snapshot shorthand.

Checkpoint:

```bash
nockchain-bench sol bench \
  --checkpoint /path/to/checkpoint.chkjam \
  --kernel /path/to/kernel.jam \
  --start-height 100 \
  --count 10 \
  --output /path/to/out \
  --measured-runs 3
```

Snapshot:

```bash
nockchain-bench sol bench \
  --snapshot-pma /path/to/snapshot.pma \
  --snapshot-manifest /path/to/snapshot.manifest \
  --kernel /path/to/kernel.jam \
  --start-height 100 \
  --count 10 \
  --output /path/to/out \
  --measured-runs 3
```

Validation:

- Exactly one workload source: `--plan`, `--fixture`, `--checkpoint`, or the complete snapshot pair.
- Count the complete snapshot pair as one source. A direct translation of the existing source counter should use a `snapshot_pair` boolean:

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

- Clap should reject `--snapshot-pma` without `--snapshot-manifest` and `--snapshot-manifest` without `--snapshot-pma` before source counting.
- Snapshot read shorthand has the same read-shorthand requirements as checkpoint: `--start-height` plus exactly one of `--end-height` or `--count`.
- Snapshot read shorthand cannot combine with `--blocks` or `--skip-genesis`.
- `--kernel` remains meaningful for checkpoint and snapshot read shorthand.
- Docker flags stay unchanged: `--docker-image` or `--docker-build-tag`, `--memory-limit`, and `--work-dir-mode`.

### Hidden `sol quick-read-once`

Update hidden `quick-read-once` consistently:

- Accept `--checkpoint` or `--snapshot-pma` plus `--snapshot-manifest`.
- Emit the same boot object in profile output.
- Keep it machine-oriented; no new public docs are required.

### `sol extract`

Although not called out in the CLI list, extraction currently boots from `--checkpoint` and is part of "anywhere it currently boots from checkpoints." Add the same snapshot pair:

```bash
nockchain-bench sol extract \
  --snapshot-pma /path/to/snapshot.pma \
  --snapshot-manifest /path/to/snapshot.manifest \
  --kernel /path/to/kernel.jam \
  --start-height 100 \
  --end-height 110 \
  --output /path/to/archive.solarch
```

`ExtractorConfig` should hold `boot: BootSourceInput` instead of `checkpoint_path: String`.

For this release, extraction from a snapshot does not populate `.solarch` `ArchiveMetadata.source_checkpoint_hash`. That field remains checkpoint-oriented and may stay empty for snapshot extraction. Do not rename or generalize `.solarch` metadata as part of snapshot boot.

## nockapp Helper Boundary

Do not make `nockchain-bench` depend on `nockapp::snapshot` internals. Add a narrow public API in `crates/nockapp/src/kernel/form.rs`, implemented by crate-private helpers in `crates/nockapp/src/snapshot.rs`.

### Public Types

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotReplayInfo {
    pub event_num: u64,
    pub pma_words: u64,
    pub alloc_words: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum SnapshotReplayConfigError {
    #[error("snapshot manifest error for {manifest_path}: {message}")]
    Manifest { manifest_path: PathBuf, message: String },

    #[error("snapshot verification failed for manifest {manifest_path} and PMA {pma_path}: {message}")]
    Verify { manifest_path: PathBuf, pma_path: PathBuf, message: String },

    #[error("failed to copy snapshot PMA from {source} to {destination}: {source_error}")]
    Copy {
        source: PathBuf,
        destination: PathBuf,
        source_error: std::io::Error,
    },

    #[error("failed to remove stale replay PMA artifact {path}: {source_error}")]
    RemoveStale {
        path: PathBuf,
        source_error: std::io::Error,
    },
}
```

Do not expose `SnapshotManifest`, `SnapshotKind`, `SnapshotVerifyError`, `ReadySnapshotRecord`, or event-log types.

### Public Functions

```rust
pub fn inspect_snapshot_replay_source(
    snapshot_pma_path: &Path,
    snapshot_manifest_path: &Path,
) -> Result<SnapshotReplayInfo, SnapshotReplayConfigError>;

impl PmaConfig {
    pub fn for_snapshot_replay(
        snapshot_pma_path: &Path,
        snapshot_manifest_path: &Path,
        replay_pma_0: PathBuf,
        replay_pma_1: PathBuf,
        words: usize,
        gc_interval: Option<Duration>,
        fsync_enabled: bool,
    ) -> Result<(Self, SnapshotReplayInfo), SnapshotReplayConfigError>;
}
```

`inspect_snapshot_replay_source` reads and checksum-validates the manifest, reads PMA file metadata, verifies manifest `pma_words` and `alloc_words` against the PMA metadata, and returns `event_num`. It should not copy the PMA and should not require event logs.

Use `SnapshotVerifyMode::Fast` for `inspect_snapshot_replay_source`: manifest checksum, PMA metadata, used-prefix hash, root raw, and cold-offset validation are enough for planning/provenance. Use `SnapshotVerifyMode::Full` only in `PmaConfig::for_snapshot_replay`, immediately before the snapshot PMA is copied into the writable replay PMA.

`PmaConfig::for_snapshot_replay` must:

1. Call `durability::set_fsync_disabled(!fsync_enabled)` just like `PmaConfig::for_replay`. This is a process-global side effect; do not hide it behind a helper whose name obscures the global setting.
2. Verify the source snapshot PMA and manifest using `SnapshotVerifyMode::Full`.
3. Copy the source PMA into `replay_pma_0` through the existing snapshot copy/replace path.
4. Remove stale `replay_pma_1`, `replay_pma_0.with_extension("meta")`, and `replay_pma_1.with_extension("meta")` if present.
5. Return a `PmaConfig` with:
   - `path_0 = replay_pma_0`
   - `path_1 = replay_pma_1`
   - `words = words`
   - `reserved_words = None`
   - `open_existing = true`
   - `create_snapshots = false`
   - `rotating_snapshot_interval_event_time = None`
   - `restore_manifest = Some(manifest)`
   - `gc_interval = gc_interval`

### Crate-Private Snapshot Refactor

In `crates/nockapp/src/snapshot.rs`, factor production restore into a path-based helper:

```rust
pub(crate) fn restore_verified_snapshot_from_paths(
    manifest_path: &Path,
    pma_path: &Path,
    operative_pma_path: &Path,
) -> Result<SnapshotManifest, SnapshotRestoreError>;
```

Then keep the existing event-log helper as a wrapper:

```rust
pub(crate) fn restore_verified_snapshot(
    record: &ReadySnapshotRecord,
    operative_pma_path: &Path,
) -> Result<SnapshotManifest, SnapshotRestoreError> {
    restore_verified_snapshot_from_paths(
        Path::new(&record.manifest_path),
        Path::new(&record.pma_path),
        operative_pma_path,
    )
}
```

This preserves production boot while giving `PmaConfig::for_snapshot_replay` a direct explicit-path entry point.

## nockchain-bench Runtime Integration

Add a focused boot-source module, for example:

- Create `crates/nockchain-bench/src/speed_of_light/boot_source.rs`
- Export it from `crates/nockchain-bench/src/speed_of_light/mod.rs`

Responsibilities:

- Define `BootSourceInput`.
- Define a canonicalized/resolved boot source if useful:

```rust
pub enum ResolvedBootSource {
    Checkpoint { checkpoint: PathBuf },
    Snapshot { pma: PathBuf, manifest: PathBuf },
}
```

- Provide helpers:
  - `canonicalize_boot_source(BootSourceInput) -> Result<ResolvedBootSource, ...>`
  - `boot_source_kind(&self) -> &'static str`
  - `boot_event_num(&self) -> Result<Option<u64>, ...>`
  - `input_paths(&self) -> Vec<(InputRole, PathBuf)>`
  - `to_quick_plan_boot_paths(...)` for trusted-plan execution if needed.

Update `crates/nockchain-bench/src/speed_of_light/kernel_utils.rs`:

```rust
pub async fn init_boot_source_backed_nockapp(
    boot: &ResolvedBootSource,
    kernel_path: &Path,
    work_dir: &PathBuf,
    fsync: bool,
) -> Result<NockApp, BootSourceBackedInitError>
```

Behavior:

- Checkpoint variant: load checkpoint and call existing `init_nockapp(..., Some(checkpoint), ...)`.
- Snapshot variant: call a new `pma_replay::init_snapshot_replay_nockapp(...)`.

Keep `init_nockapp` as the low-level checkpoint/fresh replay helper for existing internal callers during the transition, but route user-facing checkpoint/snapshot boot sites through `init_boot_source_backed_nockapp`. Keep `init_checkpoint_backed_nockapp` only as a thin checkpoint convenience wrapper or remove it after all callers migrate; do not leave some public workflows on checkpoint-only helpers.

Update `crates/nockchain-bench/src/speed_of_light/pma_replay.rs`:

- Keep `init_replay_nockapp` for checkpoint/fresh replay.
- Add `snapshot_replay_pma_config(work_dir, snapshot_pma, snapshot_manifest, fsync)` that prepares `work_dir/replay-pma`, passes `0.pma` and `1.pma` to `PmaConfig::for_snapshot_replay`, and returns `SnapshotReplayInfo`.
- Add `init_snapshot_replay_nockapp` that:
  - Loads kernel bytes.
  - Builds hot state.
  - Uses `PmaConfig::for_snapshot_replay`.
  - Calls `Kernel::load_with_hot_state_medium` with `checkpoint = None` and `Some(pma_config)`.
  - Does not call `kernel.import`.
  - Returns `NockApp` plus optional `SnapshotReplayInfo` only if callers need it at runtime; plan/provenance should normally use the earlier manifest metadata.

`init_time_secs` should include snapshot verification, PMA copy, PMA open, kernel load, and any tip peek performed by the measured setup path. The existing orchestrator pattern of starting the init clock immediately before `init_*_nockapp` is acceptable as long as snapshot copy happens inside that measured call. Do not add a separate copy timer to the public run schema in this release.

Update all checkpoint-backed callers to use `BootSourceInput` or `ResolvedBootSource`:

- `crates/nockchain-bench/src/speed_of_light/peek_bench.rs`
- `crates/nockchain-bench/src/speed_of_light/bench.rs`
- `crates/nockchain-bench/src/speed_of_light/extractor.rs`
- `crates/nockchain-bench/src/speed_of_light/orchestrator.rs`
- `crates/nockchain-bench/src/speed_of_light/orchestrate_execute.rs`
- `crates/nockchain-bench/src/speed_of_light/harness/orchestrate.rs`

## Native And Docker Input Staging

### Native

Native trusted runs should keep using absolute host paths in `ResolvedInput.absolute_path`.

For generated read shorthand:

1. Canonicalize `snapshot.pma`, `snapshot.manifest`, and `kernel`.
2. Resolve tip by booting from the snapshot into `output/input/read-tip-work/replay-pma`.
3. Build generated read plan with `boot: BootSourceInput::Snapshot`.
4. Normalize to trusted plan.
5. Execute each run by copying the source snapshot PMA into that run's `work_dir/replay-pma/0.pma`.

The source snapshot paths must always be read-only inputs. Never open the source PMA as the writable replay PMA.

Tip resolution for snapshot generated-read intentionally performs one planning-time snapshot materialization before measured runs. This cost is not part of any run's `init_time_secs`. Delete `output/input/read-tip-work` immediately after `trusted_plan.json` and resolved-case metadata have been emitted. Do not reuse the planning-time replay PMA for measured runs in this release, and do not add a `--keep-tip-work` flag unless a later debugging requirement asks for it.

### Docker

Docker trusted runs need file-level staging for both snapshot files.

Update `trusted_container_input_path` to produce stable paths:

- `snapshot-pma-0` with extension from source, usually `/bench/input/files/snapshot-pma-0.pma`
- `snapshot-manifest-0` with extension from source, usually `/bench/input/files/snapshot-manifest-0.manifest`

If a snapshot input lacks an extension, append a role-derived extension for container staging: `.pma` for `SnapshotPma` and `.manifest` for `SnapshotManifest`. Tests should assert stable role-derived names rather than depending on unusual source filenames.

Update `rewrite_trusted_inputs_for_container` to assign `container_path` to all `ResolvedInput` rows, including snapshot PMA and manifest.

Update `docker_create_args` tests to assert both snapshot files are mounted read-only:

```text
/host/snapshot.pma:/bench/input/files/snapshot-pma-0.pma:ro
/host/snapshot.manifest:/bench/input/files/snapshot-manifest-0.manifest:ro
```

Inside the container:

1. `trusted_plan.json` references snapshot input IDs.
2. `quick_plan_from_trusted` resolves IDs to `container_path`.
3. `QuickOrchestratePlan` contains `/bench/input/files/...` paths in its `boot`.
4. `PmaConfig::for_snapshot_replay` copies `/bench/input/files/snapshot-pma-0.pma` into `/bench/work/<run_id>/replay-pma/0.pma`.

Do not place the writable replay PMA under `/bench/input`; it is mounted read-only.

### Docker Work Directory Mode

Allow snapshot runs with all existing work-dir modes in the first implementation, but add an explicit warning when:

- boot source is `snapshot`
- work-dir mode is `docker_tmpfs`
- source PMA `size_bytes` is greater than 50% of Docker `memory_limit` parsed through the existing `parse_memory_limit` helper in `harness/docker.rs`

The warning should be attached to requested-case validation or printed during resolve, not hidden in Docker errors. Do not hard-fail `docker_tmpfs` yet because small snapshots may be valid.

## Provenance

Bump `PROVENANCE_SCHEMA_VERSION` to `provenance/v2`.

Keep generic fields:

- `runtime_flavor = "pma"`
- `boot_source = "checkpoint"` or `"snapshot"`
- `boot_event_num = checkpoint event or snapshot manifest event`
- `pma_work_dir_mode`
- `pma_fsync_mode`

Change `PmaReplayProvenance::checkpoint` into:

```rust
pub(crate) fn boot_source(kind: &'static str, boot_event_num: Option<u64>) -> Self
```

or add separate constructors:

```rust
pub(crate) fn checkpoint(boot_event_num: Option<u64>) -> Self
pub(crate) fn snapshot(boot_event_num: u64) -> Self
```

`phase2_pma_provenance` must use `resolved.orchestrate.boot_source` and `resolved.orchestrate.boot_event_num`, not `resolved.fixture_manifest.checkpoint_event_num`.

For `.soltest` generated replay, `resolve_trusted_plan_artifact` should set `boot_source = "checkpoint"` and `boot_event_num = generated.manifest.checkpoint_event_num`.

For plan-file and generated-read paths, derive provenance from `TrustedPlanBoot.source`.

Invariant comparison in `harness/sweep.rs` can keep treating `runtime_flavor`, `boot_source`, `boot_event_num`, and `pma_work_dir_mode` as invariants unless those fields are axes.

## bench_pages

Update Python readers/renderers for the new boot schema:

- `scripts/bench_pages/src/bench_pages/manifest.py`
- `scripts/bench_pages/src/bench_pages/render.py`
- Tests under `scripts/bench_pages/tests`

Required display behavior:

- Continue extracting `boot_source` and `boot_event_num` from provenance.
- In readable plan rendering, replace `checkpoint_input_id` lookup with tagged `boot.source`.
- Checkpoint line: `Boot from checkpoint checkpoint-0 using kernel-0`.
- Snapshot line: `Boot from snapshot snapshot-pma-0 + snapshot-manifest-0 using kernel-0`.
- If older local artifacts are accidentally loaded, render `Boot source unknown` rather than crashing. This is lenient display only; Rust schemas do not need compatibility.
- Rewrite existing tests that pin checkpoint-only strings to the v2 boot shape. Prefer checkpoint/snapshot parametrized fixtures in `test_manifest.py`, `test_loader.py`, and `test_render.py` so checkpoint coverage remains explicit and snapshot coverage is additive.

Suggested helper shape in `render.py`:

```python
def _readable_boot_line(boot: dict[str, Any]) -> str:
    source = boot.get("source") if isinstance(boot, dict) else {}
    kernel_id = boot.get("kernel_input_id") or "kernel"
    match source.get("type"):
        case "checkpoint":
            return f"Boot from checkpoint {source.get('checkpoint_input_id') or 'checkpoint'} using {kernel_id}"
        case "snapshot":
            pma = source.get("pma_input_id") or "snapshot-pma"
            manifest = source.get("manifest_input_id") or "snapshot-manifest"
            return f"Boot from snapshot {pma} + {manifest} using {kernel_id}"
        case _:
            return f"Boot source unknown using {kernel_id}"
```

## Keep `.soltest` Checkpoint-Specific

Do not change these for this release:

- `FIXTURE_LAYOUT_VERSION = 4`
- `SolFixtureCheckpointKind`
- `SolFixtureManifest.checkpoint_*` fields
- `SolFixtureFile.checkpoint_bytes`
- fixture read/write section order
- fixture inspect checkpoint wording

Generated replay from a `.soltest` should simply produce an `OrchestratePlanInput` with:

```json
"boot": {
  "type": "checkpoint",
  "checkpoint": "input/extracted/fixture-0/checkpoint.chkjam"
}
```

Do not add snapshot sections to `.soltest` and do not create a fixture v5 in this implementation.

## File Map

Primary Rust files:

- `crates/nockapp/src/snapshot.rs`
- `crates/nockapp/src/kernel/form.rs`
- `crates/nockchain-bench/src/main.rs`
- `crates/nockchain-bench/src/commands/sol.rs`
- `crates/nockchain-bench/src/speed_of_light/mod.rs`
- `crates/nockchain-bench/src/speed_of_light/boot_source.rs` (new)
- `crates/nockchain-bench/src/speed_of_light/checkpoint.rs`
- `crates/nockchain-bench/src/speed_of_light/pma_replay.rs`
- `crates/nockchain-bench/src/speed_of_light/kernel_utils.rs`
- `crates/nockchain-bench/src/speed_of_light/peek_bench.rs`
- `crates/nockchain-bench/src/speed_of_light/bench.rs`
- `crates/nockchain-bench/src/speed_of_light/extractor.rs`
- `crates/nockchain-bench/src/speed_of_light/orchestrator.rs`
- `crates/nockchain-bench/src/speed_of_light/orchestrate_plan.rs`
- `crates/nockchain-bench/src/speed_of_light/orchestrate_execute.rs`
- `crates/nockchain-bench/src/speed_of_light/harness/case.rs`
- `crates/nockchain-bench/src/speed_of_light/harness/orchestrate.rs`
- `crates/nockchain-bench/src/speed_of_light/harness/docker.rs`
- `crates/nockchain-bench/src/speed_of_light/harness/sweep.rs`
- `crates/nockchain-bench/src/speed_of_light/harness/provenance.rs`

Python/doc files:

- `scripts/bench_pages/src/bench_pages/manifest.py`
- `scripts/bench_pages/src/bench_pages/render.py`
- `scripts/bench_pages/tests/test_manifest.py`
- `scripts/bench_pages/tests/test_loader.py`
- `scripts/bench_pages/tests/test_render.py`
- `crates/nockchain-bench/specs/bench-harness-spec.md`

## Implementation Tasks

### Task 1: Add nockapp snapshot replay helper

**Files:**

- Modify: `crates/nockapp/src/snapshot.rs`
- Modify: `crates/nockapp/src/kernel/form.rs`

- [ ] Add `SnapshotReplayInfo` and `SnapshotReplayConfigError` as public types in `kernel/form.rs`.
- [ ] Factor `restore_verified_snapshot_from_paths` in `snapshot.rs`.
- [ ] Implement `inspect_snapshot_replay_source`.
- [ ] Implement `PmaConfig::for_snapshot_replay`.
- [ ] Add tests that prove:
  - manifest metadata can be inspected without event-log records
  - restore copies source PMA to replay `0.pma`
  - stale replay `1.pma` and meta files are removed
  - returned config has `open_existing = true`, `create_snapshots = false`, and private restore manifest populated
- [ ] Commit: `feat(nockapp): expose snapshot replay PMA config helper`

### Task 2: Introduce nockchain-bench BootSource model

**Files:**

- Create: `crates/nockchain-bench/src/speed_of_light/boot_source.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/mod.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/checkpoint.rs`

- [ ] Define `BootSourceInput` and `ResolvedBootSource`.
- [ ] Add canonicalization and display helpers.
- [ ] Add `boot_event_num` helper:
  - checkpoint uses `checkpoint_event_num`
  - snapshot uses `nockapp::kernel::form::inspect_snapshot_replay_source`
- [ ] Add file-level input-role expansion for trusted-plan inventory.
- [ ] Add unit tests for canonicalization, event metadata, and input-role expansion.
- [ ] Commit: `feat(bench): add boot source model`

### Task 3: Generalize runtime boot

**Files:**

- Modify: `crates/nockchain-bench/src/speed_of_light/pma_replay.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/kernel_utils.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/peek_bench.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/bench.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/extractor.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/orchestrator.rs`

- [ ] Add snapshot replay PMA config and kernel init path.
- [ ] Replace checkpoint-only config fields with `boot`.
- [ ] Keep checkpoint code path behavior unchanged.
- [ ] Ensure snapshot path does not call `kernel.import`.
- [ ] Ensure snapshot boot peeks tip using the same `peek_heaviest_chain_or_block` path.
- [ ] Add focused tests around `init_boot_source_backed_nockapp` dispatch where possible.
- [ ] Commit: `feat(bench): boot replay runtime from checkpoint or snapshot`

### Task 4: Update CLI and command plumbing

**Files:**

- Modify: `crates/nockchain-bench/src/main.rs`
- Modify: `crates/nockchain-bench/src/commands/sol.rs`

- [ ] Add `--snapshot-pma` and `--snapshot-manifest` to `extract`, `quick-read-bench`, `bench`, and hidden `quick-read-once`.
- [ ] Enforce mutual exclusivity and pair requirements.
- [ ] Update quick-read profile output boot schema.
- [ ] Update trusted `sol bench` source validation for the fourth source: snapshot pair.
- [ ] Update printed summaries.
- [ ] Extend existing Clap parse tests near the current quick-read, trusted read shorthand, and Docker trusted bench tests.
- [ ] Commit: `feat(bench): add snapshot boot CLI flags`

### Task 5: Update plan, trusted-plan, and run schemas

**Files:**

- Modify: `crates/nockchain-bench/src/speed_of_light/orchestrate_plan.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/orchestrate_execute.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/orchestrator.rs`

- [ ] Bump the enumerated schema constants from "Schema Constants To Bump" to v2.
- [ ] Replace `checkpoint` with `boot` in human-authored plans.
- [ ] Add `TrustedBootSource`.
- [ ] Extend `InputRole` and deterministic ID generation.
- [ ] Normalize checkpoint and snapshot boot sources into trusted input IDs.
- [ ] Include boot source in trusted plan hashes.
- [ ] Generate read plans from `BootSourceInput`.
- [ ] Convert trusted plans back into quick plans using container paths when present.
- [ ] Update run record boot schema, remove `Default` from `RunBoot`, and replace `RunBoot::default()` call sites with explicit construction from `TrustedPlanBoot.source`.
- [ ] Update tests for deterministic input IDs, normalized plan JSON, step signatures, generated read plans, and run record boot JSON.
- [ ] Commit: `feat(bench): normalize boot source in trusted plans`

### Task 6: Update harness, native, Docker, and sweeps

**Files:**

- Modify: `crates/nockchain-bench/src/speed_of_light/harness/case.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/harness/orchestrate.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/harness/docker.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/harness/sweep.rs`

- [ ] Change `RequestedOrchestrate::GeneratedRead` to carry `boot`.
- [ ] Populate placeholder resolved inputs for snapshot generated-read cases.
- [ ] Resolve plan artifacts and set `ResolvedOrchestrate.boot_source` and `boot_event_num`.
- [ ] Rewrite container paths for snapshot generated-read requested cases.
- [ ] Mount snapshot PMA and manifest read-only in Docker.
- [ ] Add docker-tmpfs large-snapshot warning.
- [ ] Add sweep base support:

```json
{
  "base": {
    "snapshot": {
      "pma": "snapshot.pma",
      "manifest": "snapshot.manifest"
    },
    "kernel": "kernel.jam",
    "start_height": 100,
    "count": 10
  }
}
```

- [ ] Add sweep axis support for a single atomic `snapshot` object axis. Do not add independent `snapshot.pma` and `snapshot.manifest` axes in this release because the pair must co-vary.

Snapshot axis cell schema:

```json
{
  "snapshot": [
    {
      "pma": "snapshot-a.pma",
      "manifest": "snapshot-a.manifest"
    },
    {
      "pma": "snapshot-b.pma",
      "manifest": "snapshot-b.manifest"
    }
  ]
}
```

The sweep engine should parse each cell as an object with exactly `pma` and `manifest` path fields, canonicalize both paths, and replace the generated-read boot source atomically. Equality/invariant semantics treat the canonicalized pair as one boot-source value. Labels should use the manifest file stem when no explicit case label is supplied; if two generated labels collide, prefix the parent directory name, and if they still collide, append the zero-based axis value index.

- [ ] Update read-axis error text from "checkpoint/read base" to "read boot base".
- [ ] Commit: `feat(bench): stage snapshot boot inputs in harness`

### Task 7: Update provenance and pages

**Files:**

- Modify: `crates/nockchain-bench/src/speed_of_light/harness/provenance.rs`
- Modify: `crates/nockchain-bench/src/speed_of_light/harness/sweep.rs`
- Modify: `scripts/bench_pages/src/bench_pages/manifest.py`
- Modify: `scripts/bench_pages/src/bench_pages/render.py`
- Modify: `scripts/bench_pages/tests/test_manifest.py`
- Modify: `scripts/bench_pages/tests/test_loader.py`
- Modify: `scripts/bench_pages/tests/test_render.py`

- [ ] Bump provenance schema.
- [ ] Derive PMA provenance from resolved boot source.
- [ ] Keep invariant checks for boot source and event number.
- [ ] Update readable plan rendering for tagged boot source.
- [ ] Rewrite existing checkpoint-only assertions to the v2 boot shape and add checkpoint/snapshot page-rendering test cases.
- [ ] Commit: `feat(bench-pages): render checkpoint and snapshot boot sources`

### Task 8: Keep `.soltest` checkpoint-only and update docs

**Files:**

- Modify only if needed: `crates/nockchain-bench/src/speed_of_light/fixture.rs`
- Modify: `crates/nockchain-bench/src/commands/sol.rs`
- Modify: `crates/nockchain-bench/specs/bench-harness-spec.md`

- [ ] Confirm no fixture layout changes are required.
- [ ] Ensure fixture inspect output is unchanged except for callers that now wrap the extracted checkpoint as a boot source.
- [ ] Document that `.soltest` embeds checkpoints only in this release.
- [ ] Add an explicit snapshot boot section to `crates/nockchain-bench/specs/bench-harness-spec.md` covering the `BootSourceInput`, trusted `TrustedBootSource`, Docker staging, and `.soltest` non-goals. Do not limit the docs update to a schema appendix.
- [ ] Commit: `docs(bench): document snapshot boot source schemas`

### Task 9: Verification and smoke

**Files:**

- No code files unless tests reveal issues.

- [ ] Run formatting:

```bash
cargo fmt --check
```

- [ ] Run nockapp helper tests:

```bash
cargo test -p nockapp --release snapshot
```

- [ ] Run nockchain-bench tests:

```bash
cargo test -p nockchain-bench --release
```

- [ ] Run bench pages tests:

```bash
uv run pytest scripts/bench_pages/tests
```

- [ ] Build release binary:

```bash
cargo build -p nockchain-bench --release
```

- [ ] Run checkpoint fixture inspect smoke:

```bash
/shared/nockchain/target/release/nockchain-bench sol fixture inspect \
  /shared/nockchain/fixtures/first-100-derived-checkpoint-no-mempool.soltest
```

- [ ] Run native snapshot quick-read smoke, using a small real snapshot bundle:

```bash
/shared/nockchain/target/release/nockchain-bench sol quick-read-bench \
  --snapshot-pma /shared/nockchain/snapshots/first-100-v0-full-checkpoint-no-mempool/snapshot.pma \
  --snapshot-manifest /shared/nockchain/snapshots/first-100-v0-full-checkpoint-no-mempool/snapshot.manifest \
  --kernel /shared/nockchain/snapshots/first-100-v0-full-checkpoint-no-mempool/kernel.jam \
  --start-height 1 \
  --count 1 \
  --dry-run
```

Note: snapshot `--dry-run` is not a cheap parse-only check. It still verifies and copies the snapshot PMA into the temporary replay PMA to prove setup can boot. The reference bundle's PMA is about 512 MiB, so the smoke is practical but still pays real disk IO.

- [ ] Run trusted native snapshot bench:

```bash
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

- [ ] Run trusted Docker snapshot bench with read-only staged inputs:

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

- [ ] Run one sweep with a snapshot boot base and render `bench_pages` over it.
- [ ] Commit: `test(bench): verify snapshot boot release path`

## Unit Test Checklist

Add or update tests in these areas:

- `crates/nockapp/src/kernel/form.rs`: public snapshot replay helper config and stale-file cleanup.
- `crates/nockapp/src/kernel/form.rs`: negative coverage for manifest/PMA `pma_words` or `alloc_words` mismatch and manifest checksum/verification failure.
- `crates/nockchain-bench/src/main.rs`: Clap parse tests for snapshot quick-read, trusted snapshot shorthand, Docker snapshot shorthand, and invalid combinations.
- `crates/nockchain-bench/src/commands/sol.rs`: source validation for plan/fixture/checkpoint/snapshot exclusivity.
- `crates/nockchain-bench/src/commands/sol.rs`: negative coverage for `--snapshot-pma` without `--snapshot-manifest`, `--snapshot-manifest` without `--snapshot-pma`, and snapshot pair combined with `--checkpoint`, `--fixture`, or `--plan`.
- `crates/nockchain-bench/src/speed_of_light/boot_source.rs`: canonicalization, event metadata, input roles.
- `crates/nockchain-bench/src/speed_of_light/orchestrate_plan.rs`: v2 input schema, deterministic IDs, trusted boot source JSON, generated read plan.
- `crates/nockchain-bench/src/speed_of_light/orchestrate_execute.rs`: `quick_plan_from_trusted` for snapshot container paths and run boot JSON.
- `crates/nockchain-bench/src/speed_of_light/harness/case.rs`: requested/resolved placeholder inputs for snapshot.
- `crates/nockchain-bench/src/speed_of_light/harness/docker.rs`: read-only mounts and containerized path rewrite for snapshot inputs.
- `crates/nockchain-bench/src/speed_of_light/harness/docker.rs`: docker-tmpfs large-snapshot warning trigger using `parse_memory_limit`.
- `crates/nockchain-bench/src/speed_of_light/harness/provenance.rs`: checkpoint and snapshot provenance.
- `crates/nockchain-bench/src/speed_of_light/harness/sweep.rs`: snapshot base and snapshot axis behavior.
- `scripts/bench_pages/tests`: manifest context and readable plan rendering for both boot kinds.

## Risks And Open Questions

### Large PMA Copies

Baseline correctness requires one writable PMA copy per run. A 17 GiB snapshot multiplied by warmup/measured runs can dominate runtime and disk IO. Do not optimize this away in the first implementation. Record copy time as part of init time, and document that snapshot benchmarks include snapshot materialization cost.

Future optimization: add reflink or copy-file-range support behind explicit detection, then record the copy strategy in provenance.

### Docker `docker-tmpfs`

`docker-tmpfs` can fail or distort results for large snapshots because the copied replay PMA consumes the container memory limit. First release behavior should warn when the source PMA is large relative to `--memory-limit`, but not ban the mode.

Recommended smoke mode for large snapshots is `docker-volume`; `host-bind` is also acceptable when host paths are Docker Desktop visible.

### Event-Log Replay Is Absent

Snapshot boot stops at the snapshot manifest event number. It does not replay event-log entries after the snapshot. This is intentional for first release.

Consequences:

- A snapshot from event N can only benchmark state visible at N.
- `--start-height`, `--end-height`, and `--count` must be selected against that snapshot's tip.
- The code should not silently search for or replay event logs.

### Snapshot Validity Versus Plan Hashing

Trusted plans hash snapshot PMA and manifest as file inputs. Runtime boot also verifies the PMA/manifest pair before copying. This is redundant but acceptable for first release because it gives reproducibility and correctness.

If hashing very large PMAs becomes too expensive, a future schema could record manifest hashes plus PMA used-prefix hashes instead of whole-file SHA-256. Do not make that change here.

It is acceptable for v1 to inspect/verify snapshot metadata during plan normalization and verify again inside `PmaConfig::for_snapshot_replay`. Do not introduce a public pre-verified manifest handle to avoid the second read in this release; that would widen the `nockapp` API surface.

Snapshot generated-read tip resolution also uses real snapshot materialization plus a heaviest-tip peek. Avoiding that pre-copy is a future optimization, not a v1 requirement.

### Kernel Hash Mismatch

Production snapshot restore warns on kernel hash mismatch but loads state into the current kernel. Snapshot replay should match that behavior through `nockapp`. `nockchain-bench` should not add stricter kernel-hash rejection unless a separate release requirement appears.

### Manifest API Surface

Keep the new `nockapp` API narrow. If implementation pressure suggests exposing `SnapshotManifest` publicly, stop and redesign the helper instead. `nockchain-bench` should not become coupled to snapshot manifest internals.

### `.soltest` Snapshot Embedding

Embedding snapshots into `.soltest` would need a fixture v5 layout, multi-file section metadata, and large-file handling. That is intentionally deferred.
