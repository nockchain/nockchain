# nockchain-bench Snapshot Boot Research

Date: 2026-05-26

Status: research-only findings. No implementation plan or code changes are included here.

## Goal

`nockchain-bench` currently boots benchmark runs from checkpoints. We want it to also boot from PMA snapshots anywhere it currently accepts or materializes a checkpoint. Checkpoint support can remain. Event-log replay is explicitly out of scope for this first snapshot path; loading directly from a snapshot is sufficient.

This document records codebase facts to feed a separate spec-writing pass.

## Executive Summary

PMA snapshots are not checkpoint-shaped artifacts. Checkpoints are state-jam-style inputs that `nockchain-bench` decodes into `LoadState` and imports into a fresh replay PMA. PMA snapshots are a copied PMA slab plus a bincode manifest; production recovery verifies the manifest and slab, copies the snapshot PMA to the operative PMA path, synthesizes PMA metadata from the manifest, opens the PMA as existing state, then loads the PMA-rooted kernel state.

The most efficient design is therefore not to convert snapshots into checkpoints. It is to add a small public `nockapp` helper that restores a snapshot PMA and manifest into a bench replay PMA location and returns or constructs a `PmaConfig` with `open_existing = true` and `restore_manifest = Some(manifest)`. `nockchain-bench` can then generalize its checkpoint inputs into a boot-source model with `checkpoint` and `snapshot` variants.

The runtime boot change is probably small. The surrounding schema, Docker input staging, sweep, provenance, and `bench_pages` changes are the larger surface area.

## Key Runtime Facts

`nockchain-bench` checkpoint boot is state import:

- `crates/nockchain-bench/src/speed_of_light/checkpoint.rs:76` decodes checkpoint input as `JammedCheckpointV2`.
- `crates/nockchain-bench/src/speed_of_light/pma_replay.rs:46` converts `SaveableCheckpoint` into `LoadState`, dropping cold state.
- `crates/nockchain-bench/src/speed_of_light/pma_replay.rs:75` boots a fresh PMA kernel and then calls `kernel.import(load_state)`.
- `crates/nockchain-bench/src/speed_of_light/pma_replay.rs:34` uses fresh replay PMA slabs and disables production snapshot restore behavior.
- `crates/nockapp/src/kernel/form.rs:234` defines `LoadState` as `ker_hash`, `event_num`, and `kernel_state`.

Production snapshot boot is PMA restore:

- `crates/nockapp/src/snapshot.rs:23` defines snapshot manifest magic/version: `SNAPMAN1`, version `2`.
- `crates/nockapp/src/snapshot.rs:54` defines `SnapshotManifest`, including `ker_hash`, `event_num`, `pma_words`, `alloc_words`, `kernel_root_raw`, `cold_offset`, `used_blake3`, optional `structure_blake3`, and checksum.
- `crates/nockapp/src/snapshot.rs:291` verifies a manifest and PMA file.
- `crates/nockapp/src/snapshot.rs:485` restores a verified snapshot by copying the snapshot PMA into the operative PMA path and returning the manifest.
- `crates/nockapp/src/kernel/boot.rs:2025` attempts snapshot recovery before checkpoint fallback when PMA is missing, invalid, or out of sync.
- `crates/nockapp/src/kernel/boot.rs:2099` returns snapshot recovery as `checkpoint: None`, `pma_open_existing: true`, and `snapshot_manifest: Some(manifest)`.
- `crates/nockapp/src/kernel/boot.rs:2668` feeds that manifest into `PmaConfig.restore_manifest`.
- `crates/nockapp/src/kernel/form.rs:837` synthesizes PMA metadata from the restore manifest.
- `crates/nockapp/src/kernel/form.rs:1876` loads PMA state from metadata when there is no checkpoint.
- `crates/nockapp/src/kernel/form.rs:1963` rebuilds cold state fresh for PMA-state restore.

Current access boundaries block `nockchain-bench` from using this directly:

- `crates/nockapp/src/lib.rs:20` declares `snapshot` as `pub(crate)`.
- `crates/nockapp/src/kernel/form.rs:240` declares `PmaConfig.restore_manifest` as `pub(crate)`.

## Likely nockapp Helper Boundary

The smallest useful `nockapp` API would expose direct snapshot replay setup without exposing all snapshot internals. A possible shape:

```rust
PmaConfig::for_snapshot_replay(
    snapshot_pma_path,
    snapshot_manifest_path,
    replay_pma_0,
    replay_pma_1,
    words,
    gc_interval,
    fsync_enabled,
)
```

Internally this helper would:

1. Verify the source `.manifest` and `.pma`.
2. Copy or restore the source PMA into the per-run writable replay PMA path.
3. Read the manifest.
4. Return a `PmaConfig` with `open_existing = true`, `create_snapshots = false`, and `restore_manifest = Some(manifest)`.

This mirrors production recovery but removes event-log discovery and event replay. It also avoids direct-jamming a large PMA snapshot into `LoadState`, which would likely be slower and less representative.

One efficiency caveat: `crates/nockapp/src/snapshot.rs:887` currently uses `fs::copy` for snapshot file copy and then syncs the destination. A future optimization could consider reflinks where available, but the first spec should treat a per-run writable copy as the correctness baseline.

## Current Checkpoint-Coupled Surfaces

### CLI And Command Flow

- `crates/nockchain-bench/src/main.rs:101`: `sol extract --checkpoint`.
- `crates/nockchain-bench/src/main.rs:192`: `sol quick-read-bench --checkpoint`.
- `crates/nockchain-bench/src/main.rs:277`: trusted `sol bench --checkpoint` read shorthand.
- `crates/nockchain-bench/src/main.rs:489`: hidden `sol quick-read-once --checkpoint`.
- `crates/nockchain-bench/src/commands/sol.rs:47`: `QuickReadBenchOptions` stores checkpoint path.
- `crates/nockchain-bench/src/commands/sol.rs:168`: quick-read validates/canonicalizes checkpoint path.
- `crates/nockchain-bench/src/commands/sol.rs:252`: trusted source exclusivity validation.
- `crates/nockchain-bench/src/commands/sol.rs:595`: trusted `--checkpoint` becomes `RequestedOrchestrate::GeneratedRead`.
- `crates/nockchain-bench/src/commands/sol.rs:991`: extract passes checkpoint path into `ExtractorConfig`.

### Runtime Boot Paths

- `crates/nockchain-bench/src/speed_of_light/kernel_utils.rs:60`: `init_nockapp` accepts optional checkpoint.
- `crates/nockchain-bench/src/speed_of_light/kernel_utils.rs:78`: `init_checkpoint_backed_nockapp`.
- `crates/nockchain-bench/src/speed_of_light/peek_bench.rs:83`: quick-read config stores `checkpoint_path`.
- `crates/nockchain-bench/src/speed_of_light/peek_bench.rs:334`: quick-read boots from checkpoint.
- `crates/nockchain-bench/src/speed_of_light/bench.rs:75`: fixture replay config has optional checkpoint path.
- `crates/nockchain-bench/src/speed_of_light/bench.rs:217`: fixture replay loads checkpoint.
- `crates/nockchain-bench/src/speed_of_light/extractor.rs:106`: extractor config has checkpoint path.
- `crates/nockchain-bench/src/speed_of_light/extractor.rs:151`: extractor loads checkpoint and boots from it.

### Fixture And Archive Artifacts

`.soltest` is checkpoint-specific:

- `crates/nockchain-bench/src/speed_of_light/fixture.rs:17`: fixture magic/layout.
- `crates/nockchain-bench/src/speed_of_light/fixture.rs:31`: `SolFixtureCheckpointKind`.
- `crates/nockchain-bench/src/speed_of_light/fixture.rs:39`: `SolFixtureManifest` checkpoint fields.
- `crates/nockchain-bench/src/speed_of_light/fixture.rs:55`: `SolFixtureFile` includes `checkpoint_bytes`.
- `crates/nockchain-bench/src/speed_of_light/fixture.rs:90`: fixture writer writes checkpoint section.
- `crates/nockchain-bench/src/speed_of_light/fixture.rs:131`: fixture reader reads checkpoint section.
- `crates/nockchain-bench/src/commands/sol.rs:1187`: fixture inspect renders checkpoint metadata.

`.solarch` has a latent checkpoint provenance field:

- `crates/nockchain-bench/src/speed_of_light/archive.rs:234`: `ArchiveMetadata`.
- `crates/nockchain-bench/src/speed_of_light/archive.rs:250`: `source_checkpoint_hash`.
- Current extraction appears to use `SolArchiveWriter::new()` rather than setting this field.

Recommendation for first spec: do not force snapshots into `.soltest` first. Support snapshot boot in plans and direct/trusted read paths, then decide separately whether a fixture layout v5 should embed snapshot PMA artifacts.

### Plan, Trusted Run, And Docker Schemas

- `crates/nockchain-bench/src/speed_of_light/orchestrator.rs:23`: quick-orchestrate plan uses `checkpoint` and `kernel`.
- `crates/nockchain-bench/src/speed_of_light/orchestrator.rs:341`: plan validation resolves checkpoint paths.
- `crates/nockchain-bench/src/speed_of_light/orchestrator.rs:521`: results serialize `boot.checkpoint`.
- `crates/nockchain-bench/src/speed_of_light/orchestrate_plan.rs:47`: `OrchestratePlanInput` has top-level `checkpoint`.
- `crates/nockchain-bench/src/speed_of_light/orchestrate_plan.rs:75`: `GeneratedReadOptions` has `checkpoint_path`.
- `crates/nockchain-bench/src/speed_of_light/orchestrate_plan.rs:203`: `TrustedPlan`.
- `crates/nockchain-bench/src/speed_of_light/orchestrate_plan.rs:213`: `TrustedPlanBoot` has `checkpoint_input_id`.
- `crates/nockchain-bench/src/speed_of_light/orchestrate_plan.rs:225`: `ResolvedInput` and `InputRole`.
- `crates/nockchain-bench/src/speed_of_light/orchestrate_execute.rs:75`: run result boot stores `checkpoint_input_id`.
- `crates/nockchain-bench/src/speed_of_light/orchestrate_execute.rs:633`: trusted execution resolves checkpoint input.
- `crates/nockchain-bench/src/speed_of_light/harness/case.rs:97`: `RequestedOrchestrate::GeneratedRead` stores `checkpoint_path`.
- `crates/nockchain-bench/src/speed_of_light/harness/orchestrate.rs:420`: trusted setup builds fixture, plan, or checkpoint read shorthand.
- `crates/nockchain-bench/src/speed_of_light/harness/docker.rs:1183`: Docker rewrites checkpoint inputs.
- `crates/nockchain-bench/src/speed_of_light/harness/docker.rs:1246`: Docker stages checkpoint input files.
- `crates/nockchain-bench/src/speed_of_light/harness/docker.rs:1269`: Docker mounts inputs read-only.

Snapshot inputs are at least two files, and maybe best modeled as one logical boot input with multiple paths:

- PMA slab path.
- Manifest path.
- Kernel path remains separate, as today.
- No event log required for the bench-only path.

### Sweeps And Provenance

- `crates/nockchain-bench/src/speed_of_light/harness/sweep.rs:318`: sweep base accepts checkpoint.
- `crates/nockchain-bench/src/speed_of_light/harness/sweep.rs:433`: checkpoint axis switches to generated read.
- `crates/nockchain-bench/src/speed_of_light/harness/sweep.rs:685`: read axes require checkpoint/read base.
- `crates/nockchain-bench/src/speed_of_light/harness/sweep.rs:1810`: comparisons treat runtime/boot/work-dir provenance as invariants.
- `crates/nockchain-bench/src/speed_of_light/harness/provenance.rs:61`: generic provenance already has `boot_source` and `boot_event_num`.
- `crates/nockchain-bench/src/speed_of_light/harness/provenance.rs:97`: `PmaReplayProvenance::checkpoint` hard-codes checkpoint source.

Provenance should preserve the useful generic fields but formalize values:

- `boot_source = "checkpoint"` for checkpoint boot.
- `boot_source = "snapshot"` for PMA snapshot boot.
- `boot_event_num` from checkpoint event or snapshot manifest event.

### bench_pages

`bench_pages` already reads some generic boot fields, but still assumes checkpoint naming:

- `scripts/bench_pages/src/bench_pages/manifest.py:14`: case context extractors.
- `scripts/bench_pages/src/bench_pages/manifest.py:348`: provenance boot context extraction.
- `scripts/bench_pages/src/bench_pages/render.py:178`: boot tooltips.
- `scripts/bench_pages/src/bench_pages/render.py:677`: readable plan uses `checkpoint_input_id`.
- Existing tests assert `"checkpoint"` in `test_manifest.py`, `test_loader.py`, and `test_render.py`.

The spec should include `bench_pages` schema/display updates so pages can render both checkpoint and snapshot boot sources.

## Suggested Schema Direction

Because this is the first release and backward compatibility is not required, the spec can choose clean breaking names:

- Replace `checkpoint` fields in public benchmark schemas with `boot` or `boot_source`.
- Replace `checkpoint_input_id` with `boot_input_id`.
- Replace `InputRole::Checkpoint` with either:
  - `InputRole::Boot`, carrying a boot kind in the boot object, or
  - distinct `InputRole::Checkpoint` and `InputRole::Snapshot` roles.
- Replace `GeneratedRead { checkpoint_path }` with `GeneratedRead { boot: BootInputRequest }`.
- Keep checkpoint-specific names inside `.soltest` unless and until `.soltest` v5 embeds snapshots.

A rough boot input enum:

```json
{
  "boot": {
    "type": "snapshot",
    "pma": "path/to/snapshot.pma",
    "manifest": "path/to/snapshot.manifest"
  },
  "kernel": "path/to/kernel.jam",
  "steps": []
}
```

Checkpoint variant:

```json
{
  "boot": {
    "type": "checkpoint",
    "checkpoint": "path/to/checkpoint.chkjam"
  },
  "kernel": "path/to/kernel.jam",
  "steps": []
}
```

The exact field names should be set by the spec, but the important fact is that snapshot should be modeled as one logical boot source rather than as unrelated free-floating input files.

## Testing And Smoke Strategy

Existing tests to extend:

- `crates/nockchain-bench/src/main.rs:1019`: quick-read checkpoint CLI parse test.
- `crates/nockchain-bench/src/main.rs:1200`: quick-orchestrate plan/profile CLI parse test.
- `crates/nockchain-bench/src/main.rs:1474`: Docker trusted bench parsing.
- `crates/nockchain-bench/src/main.rs:1571`: trusted read shorthand.
- `crates/nockchain-bench/src/speed_of_light/orchestrate_plan.rs`: plan input, deterministic input ids, signatures, and generated-read tests.
- `crates/nockchain-bench/src/speed_of_light/harness/docker.rs:2060`: read-only trusted input mounts.
- `crates/nockchain-bench/src/speed_of_light/harness/docker.rs:2121`: Docker plan rewrite.
- `crates/nockchain-bench/src/speed_of_light/harness/provenance.rs`: PMA provenance tests need snapshot coverage.
- `scripts/bench_pages/tests`: boot-source display and manifest extraction.

Minimal smoke commands after implementation should cover:

1. Legacy checkpoint fixture inspection still works.
2. Native snapshot quick-read or quick-orchestrate.
3. Trusted native snapshot bench with at least 3 measured runs.
4. Trusted Docker snapshot bench with read-only staged inputs.
5. One small sweep including a snapshot boot case.
6. `bench_pages` rendering over that sweep.

## Snapshot Test Data Requirements

A useful minimal snapshot bundle should contain:

- `kernel.jam`
- `snapshot.pma`
- `snapshot.manifest`
- optional `archive.solarch` for one `poke_archive_block` replay smoke
- metadata for event number, expected height, and hashes

Existing local snapshot-shaped files were observed under `/shared/nockchain/tmp/statejam-exp-v0-full-{chkjam,statejam}-20260416/data/pma/epoch.pma`, but they are around 17G and do not appear to be clean minimal bench bundles. Treat them as layout examples, not ideal smoke fixtures.

Docs that describe the PMA snapshot model:

- `docs/pma/DURABILITY-OPERATIONS.md`
- `docs/pma/DESIGN.md`

## Risks And Open Questions

Source snapshot PMAs must remain read-only. Each run should materialize into a per-run writable replay PMA directory before boot. This is especially important for Docker trusted runs.

Docker input handling currently assumes single file inputs under `/bench/input/files/{input_id}.{ext}`. Snapshot boot needs multi-file logical inputs and Docker-visible host paths. Avoid `/tmp` paths that are not shared with Docker Desktop.

`docker-tmpfs` may be unsuitable for large snapshot PMAs with an `8g` memory limit. The spec should decide whether snapshot runs are allowed with `docker-tmpfs`, should warn, or should require a disk-backed work-dir mode for large snapshots.

Event logs should not be required for the initial bench snapshot path. Production snapshot discovery uses SQLite, but the bench path should accept explicit PMA and manifest paths and do no post-snapshot event replay.

The word "snapshot" is already used in some checkpoint-related names, such as checkpoint directory selection. The spec should distinguish PMA snapshots from checkpoint snapshots clearly in CLI help and type names.

## Recommended Spec Focus

The next spec should answer these questions:

1. What is the public boot-source JSON schema for checkpoint and snapshot inputs?
2. What CLI flags expose snapshot boot without making the checkpoint shorthand confusing?
3. What exact `nockapp` helper should be added, and what should remain private?
4. How are snapshot PMA and manifest staged for native and Docker trusted runs?
5. Which checkpoint names should be broken before first release versus preserved inside legacy `.soltest`?
6. What provenance fields and `bench_pages` labels prove a run booted from snapshot?
7. What minimal snapshot fixture/bundle will be created for smoke tests?
