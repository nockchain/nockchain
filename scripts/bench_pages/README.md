# Bench Pages Publisher

`scripts/bench_pages` publishes static SOL benchmark sweep reports from an
existing `nockchain-bench sol sweep` artifact tree. The publisher is a reader
and presenter: benchmark execution, archive validation, snapshot inspection,
and replay correctness remain owned by `nockchain-bench`.

## Usage

Publish a completed sweep to a local site tree:

```bash
uv run --project scripts/bench_pages publish-sweep \
  --sweep-root ./tmp/live-sol-sweep \
  --output-dir ./tmp/live-sol-pages \
  --replace \
  --no-publish-ghcr
```

Publish to the configured Pages branch, and optionally GHCR for Docker sweeps:

```bash
uv run --project scripts/bench_pages publish-sweep \
  --sweep-root ./tmp/live-sol-sweep \
  --push
```

`--output-dir` is for local inspection and does not touch `gh-pages`.
Without `--output-dir`, the command stages a Pages worktree, limits hosted
artifact size, and pushes only when `--push` is present.

## Snapshot Boot Context

Pages display boot context from trusted artifacts only. They do not open checkpoint files, PMA snapshots, snapshot manifests, `.soltest`, or `.solarch` inputs during publication.

Supported display fields:

- `provenance.json`: `runtime_flavor`, `boot_source`, `boot_event_num`, `pma_work_dir_mode`
- `trusted_plan.json` or nested `resolved_case.json.trusted_plan`: `boot.source.type`, snapshot input ids, checkpoint input id, event number, and kernel input id
- `resolved_case.json` and `provenance.json`: fixture identity and input identity fields used by the case workspace

Missing boot fields are treated as absent display context.

## Raw Transaction Replay Context

Raw transaction replay display uses two artifact layers:

- `summary.json`: high-level rates such as `raw_tx_pokes_per_second`
- `runs/*/steps.ndjson`: per-step raw transaction evidence

The publisher streams `steps.ndjson` when present and stores compact derived
summaries in `manifest.json`. Full step rows stay in the published artifact
tree and artifact bundle. Missing `steps.ndjson` means no per-step transaction
panel for that run. A present malformed `steps.ndjson` is rejected because
silently ignoring it would make a transaction replay run look like a
no-transaction run.

Recognized per-step fields include:

- `raw_tx_pokes_completed`
- `block_poke_duration_ms`
- `raw_tx_poke_duration_ms`
- `slab_prebuild_duration_ms`
- `block_slab_prebuild_duration_ms`
- `raw_tx_slab_prebuild_duration_ms`
- `raw_tx_slabs_prebuilt`
- `raw_tx_payload_bytes_prebuilt`
- `slab_prebuild_start_rss_bytes`
- `slab_prebuild_peak_rss_bytes`

Known-zero values are evidence. For example, `raw_tx_pokes_completed: 0` after successful prebuild is rendered as `0`, not as missing data.

Failure progress is intentionally bounded in `manifest.json` and HTML. The
summary keeps `error_step_count`, renders sampled head/tail failure rows with
`run_id`, and records how many rows were omitted. The complete failure evidence
remains available in the published `steps.ndjson` artifacts.

## Report Contents

Current reports include:

- sweep verdict, completion state, runtime, boot source, fixture, and PMA work
  directory summaries
- readable plan context for checkpoint and snapshot boot sources
- operation health and typed peek throughput columns when cache expectation
  hints are present
- block replay throughput and raw transaction throughput (`Raw tx/s`)
- a Raw Transaction Replay panel with poke counts, slab counts, payload bytes,
  prebuild timing, prebuild RSS range, and bounded failure samples
- case workspaces, artifact browser links, and optional CPU profile links

## Local Verification

Run the publisher test suite with:

```bash
uv run --project scripts/bench_pages python -m unittest discover -s scripts/bench_pages/tests -v
```

For manual inspection, publish a fixture to a temporary output directory:

```bash
uv run --project scripts/bench_pages publish-sweep \
  --sweep-root scripts/bench_pages/tests/fixtures/raw_tx_snapshot_minimal \
  --output-dir /tmp/bench-pages-raw-tx
```
