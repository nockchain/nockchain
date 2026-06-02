---
title: SOL Benchmark Snapshot Boot and V4 Raw Transaction Replay
date: 2026-05-29
category: architecture-patterns
module: nockchain-bench speed-of-light harness
problem_type: architecture_pattern
component: tooling
severity: high
applies_when:
  - "Adding checkpoint or snapshot boot support to SOL benchmark plans"
  - "Building V4 .solarch replay workloads that include blocks and raw transaction pokes"
  - "Publishing benchmark evidence where final tip advancement must be proven"
related_components:
  - nockapp PMA snapshot replay
  - speed-of-light orchestrator
  - trusted sweep artifacts
tags: [nockchain-bench, solarch, snapshot-boot, raw-tx-replay, trusted-bench, pma]
---

# SOL Benchmark Snapshot Boot and V4 Raw Transaction Replay

## Context

The work from `00fa2024` through `e56dc47f` turned the speed-of-light benchmark harness from a checkpoint-only replay path into a boot-source-aware harness that can read from checkpoints or PMA snapshots, then hardened V4 `.solarch` replay so blocks with transactions replay their raw transaction pokes and expose enough evidence for trusted benchmark rows.

The key pressure was that `.soltest` v4 is checkpoint-shaped: it embeds `checkpoint + archive + kernel`. Snapshot boot is a different artifact shape, because the trusted input is a PMA snapshot pair plus a kernel and an explicit plan. In the same arc, V4 archives gained raw transaction payload sections, which meant replay completeness, timing semantics, memory observability, and final-tip validation all had to become first-class evidence.

## Guidance

Model benchmark boot state as a typed boot source union, not as a checkpoint path with optional side channels. The durable shape is:

```rust
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BootSourceInput {
    Checkpoint { checkpoint: PathBuf },
    Snapshot { pma: PathBuf, manifest: PathBuf },
}
```

Use that union everywhere the benchmark needs provenance or trusted inputs: CLI parsing, generated read plans, trusted plan normalization, Docker path rewriting, requested/resolved cases, provenance, and run execution. Snapshot replay should verify the manifest/PMA pair, copy it into a fresh per-run PMA directory, and synthesize replay metadata before boot:

```rust
PmaConfig::for_snapshot_replay(
    snapshot_pma,
    snapshot_manifest,
    replay_pma_dir.join("0.pma"),
    replay_pma_dir.join("1.pma"),
    replay_pma_words(),
    None,
    fsync_enabled,
)
```

Keep `.soltest` v4 as the checkpoint fixture format. For snapshot-backed replay workloads, use an explicit `orchestrate-plan/v2` bundle instead of trying to force snapshot files into `.soltest`:

```json
{
  "schema_version": "orchestrate-plan/v2",
  "boot": {
    "type": "snapshot",
    "pma": "boot/snapshot.pma",
    "manifest": "boot/snapshot.manifest"
  },
  "kernel": "kernel.jam",
  "expected_final_tip": {
    "height": 10113,
    "hash": "AgKEWgUfHELk43FEc6bik81A4diabMouPUaRnKnx5725YsPhQ3X3vg9"
  },
  "steps": [
    {
      "type": "poke_archive_range",
      "archive": "inputs/archive-10014-10113.solarch",
      "start_height": 10014,
      "end_height": 10113
    }
  ]
}
```

For V4 raw transaction replay, select a replay window before execution and reject incomplete windows by default. V3 archives with transaction-bearing blocks and V4 archives where `tx_count > 0` but raw transaction payloads are unavailable must be treated as incomplete replay unless the caller explicitly opts in:

```rust
pub enum ReplayCompleteness {
    Complete,
    Incomplete { reason: String },
}
```

Do not let V4 support leak through V3-shaped APIs as `None` or a panic. Unsupported archive operations should be typed errors, so callers can distinguish "this V4 operation is unsupported" from "the block is missing".

When replaying a V4 block, prebuild both the block poke slab and raw transaction poke slabs, then poke the block followed by each raw transaction. The timings should preserve:

- block poke duration
- raw transaction poke duration
- total duration
- slab prebuild duration
- block and raw transaction slab prebuild durations
- raw transaction slabs built
- raw transaction payload bytes successfully prebuilt
- prebuild RSS start and peak
- raw transaction pokes completed so far, including known zero

Carry those fields through errors. Archive-read failures during prebuild, malformed raw transaction payloads, block-poke failures, and raw-tx-poke failures should still emit timing-carrying errors so `steps.ndjson`, quick results, trusted rows, and downstream summaries retain the partial evidence.

## Why This Matters

Checkpoint boot and snapshot boot are not the same artifact contract. A checkpoint can be embedded into `.soltest`; a snapshot is a PMA file plus a manifest that must be verified, copied into a writable replay PMA, and represented as two trusted inputs. Hiding both behind one path loses provenance and makes Docker mounting, schema evolution, and run reproducibility harder.

Raw transaction replay also changes what a "valid" SOL benchmark means. A block-only replay can advance through a height range while silently skipping transaction effects if the archive lacks raw transaction payloads. The harness now needs both replay completeness and final-tip validation:

- replay completeness proves the selected archive window has enough payload to replay the intended workload
- final-tip validation proves the runtime actually advanced to the expected height and hash

The session fixture work proved why this matters. A previous 100-block fixture used a checkpoint already at height `38393`, so replaying `10014..=10113` produced a valid-looking run while the observed final tip stayed at `38393`. Building a checkpoint immediately before the target window fixed the evidence: replaying from the real checkpoint tip `5628` to `10013`, then packaging `10014..=10113`, produced trusted runs where expected and observed final tips both matched `10113`.

## When to Apply

- Use `BootSourceInput` whenever a benchmark path can boot from either a checkpoint or PMA snapshot.
- Use `.soltest` for checkpoint-backed replay fixtures.
- Use explicit plan bundles for snapshot-backed replay workloads.
- Use `ReplayCompleteness` before trusted replay whenever archives may contain transaction-bearing blocks.
- Require final-tip validation for replay evidence, especially when constructing fixtures from historical archives.
- Preserve partial timing and progress metrics on V4 raw transaction failures; failed rows are still benchmark evidence.

## Examples

### Snapshot Read Smoke Shape

Snapshot-backed read benchmarks use the snapshot pair directly:

```bash
./target/release/nockchain-bench sol bench \
  --snapshot-pma snapshots/first-100-v0-full-checkpoint-no-mempool/snapshot.pma \
  --snapshot-manifest snapshots/first-100-v0-full-checkpoint-no-mempool/snapshot.manifest \
  --kernel snapshots/first-100-v0-full-checkpoint-no-mempool/kernel.jam \
  --start-height 0 \
  --count 1 \
  --output /tmp/nockchain-bench-snapshot-trusted-smoke \
  --warmup-runs 0 \
  --measured-runs 3 \
  --cooldown-secs 0
```

That path should record `boot_source=snapshot` and `boot_event_num` from the snapshot manifest in provenance.

### Checkpoint-Backed V4 Raw Transaction Fixture Evidence

For the V1 raw-transaction window, the reliable fixture construction path was:

1. Extract an earlier full checkpoint fixture and verify the live heaviest tip, not just the manifest height.
2. Replay the source V4 archive from the live tip through the desired pre-window height.
3. Write the new checkpoint only if final tip height matches the target.
4. Slice the V4 archive for the next 100 blocks.
5. Build a `.soltest` with the checkpoint immediately before the window.
6. Run trusted bench or sweep and inspect `final_tip_validation`.

The tracked commit range provides the architecture and harness behavior. A session-built artifact outside the tracked range verified the pattern with:

- checkpoint target: `10013`
- archive window: `10014..=10113`
- archive version: V4
- blocks: `100`
- archive raw txs: `25`
- trusted sweep verdict: `Valid`
- final tip expected and observed: `10113 AgKEWgUfHELk43FEc6bik81A4diabMouPUaRnKnx5725YsPhQ3X3vg9`

### Bench Pages Raw Transaction Surfacing

`bench_pages` now promotes the raw transaction evidence emitted by trusted replay. The publisher reads `runs/*/steps.ndjson` when present, keeps the full step rows as published artifacts, and derives compact manifest summaries for a case-level raw transaction replay panel from per-step fields such as slab counts, payload bytes, poke duration, prebuild duration, and RSS:

```json
{
  "raw_tx_pokes_completed": 2,
  "raw_tx_poke_duration_ms": 619.7732070000001,
  "raw_tx_slabs_prebuilt": 2,
  "raw_tx_payload_bytes_prebuilt": 46132
}
```

The page also adds `raw_tx_pokes_per_second` to the comparison/KPI surfaces when `summary.json` reports it. Missing `steps.ndjson` remains compatible with older sweeps, but a present malformed step file is rejected because otherwise a raw-transaction run could be misrepresented as having no transaction evidence.

Keep `bench_pages` as an artifact presenter. Snapshot boot context comes from provenance and trusted plan fields, and raw transaction replay context comes from summary and step artifacts. The publisher should not inspect `.solarch`, `.soltest`, checkpoint, snapshot PMA, or snapshot manifest files during page generation.

## Related

- Commit range documented: `00fa2024^..e56dc47f`
- Main modules: `crates/nockchain-bench/src/speed_of_light/boot_source.rs`, `pma_replay.rs`, `orchestrate_plan.rs`, `replay_window.rs`, `final_tip.rs`, `archive.rs`, `poke.rs`, `orchestrator.rs`, and `orchestrate_execute.rs`
- Plan: `docs/superpowers/plans/2026-05-26-nockchain-bench-snapshot-boot-implementation-plan.md`
- Existing `docs/solutions/` overlap: none; this repository did not have a solution-doc tree before this file
