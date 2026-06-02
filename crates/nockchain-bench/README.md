# nockchain-bench

`nockchain-bench` is the Nockchain speed-of-light benchmarking crate. It
contains the CLI and library code used to extract replay archives, inspect
fixtures, run quick local experiments, run trusted native and Docker benchmark
cases, execute trusted sweeps, and publish static `bench_pages` reports.

The current implementation uses PMA replay as the normal `nockchain-bench`
runtime. The crate consumes existing `.soltest`, checkpoint, PMA snapshot,
kernel, `.solarch`, and orchestrate-plan inputs.

Use release builds and release binaries unless you are explicitly debugging
build-profile behavior.

```bash
cargo build -p nockchain-bench --release
./target/release/nockchain-bench sol --help
```

## TL;DR

The problem: SOL benchmark numbers are easy to distort with ad hoc commands,
dirty Docker environments, hidden fixture changes, checkpoint drift, and
missing provenance.

The solution: `nockchain-bench` has two CLI surfaces:

| Surface | Use it for | Evidence quality |
| --- | --- | --- |
| `sol quick-*` | local investigation, profiling, and debugging | not trusted evidence |
| `sol bench` / `sol sweep` | measured native or Docker benchmark records | trusted artifact tree with verdicts |

Supporting scripts handle reporting and local image construction:

| Script | Use it for | Output |
| --- | --- | --- |
| `scripts/bench_pages` | local or GitHub Pages reporting over sweep artifacts | visual report, raw artifact browser |
| `scripts/build_nockchain_bench_image.sh` | Docker benchmark image construction | local benchmark image |

Recommended local Docker smoke:

```bash
mkdir -p ./tmp/docker-bench-smoke
./target/release/nockchain-bench sol bench \
  --fixture ./fixtures/first-100-derived-checkpoint-no-mempool.soltest \
  --output ./tmp/docker-bench-smoke \
  --docker-build-tag nockchain-bench:local \
  --memory-limit 8g \
  --work-dir-mode docker-tmpfs \
  --warmup-runs 0 \
  --measured-runs 3 \
  --cooldown-secs 0
```

## Feature Map

| Feature | Command or tool | Notes |
| --- | --- | --- |
| Extract replay archive | `sol extract` | checkpoint + kernel to `.solarch` |
| Inspect archive mempool | `sol inspect` | stale mempool snapshot inspection |
| Inspect fixture | `sol fixture inspect` | hashes, ranges, checkpoint kind, payload sizes |
| Quick replay benchmark | `sol quick-bench` | fixture-backed inner loop |
| Quick read benchmark | `sol quick-read-bench` | checkpoint- or snapshot-backed `%heavy-n` peek loop |
| Quick orchestration | `sol quick-orchestrate` | ordered poke/peek plan, not trusted evidence |
| Trusted benchmark | `sol bench` | native or Docker measured runs |
| Trusted sweep | `sol sweep` | matrix of trusted benchmark cases |
| Docker preflight | `sol validate` | validates image, cgroup, and resource realization |
| Hidden run wrapper | `sol run-once` | machine entrypoint for trusted harness |
| Hidden read wrapper | `sol quick-read-once` | machine entrypoint for profiled read reruns |
| Hidden identity | `sol binary-identity` | host/container version comparison |
| Hidden Docker probe | `sol validate-probe` | container-side Docker validation |
| Docker image build | `scripts/build_nockchain_bench_image.sh` | standard or profiling image |
| Static reports | `scripts/bench_pages publish-sweep` | local Pages tree or push workflow |

## Artifact Types

| Extension | Meaning | Producer | Consumer |
| --- | --- | --- | --- |
| `.chkjam` | checkpoint snapshot | node runtime or existing fixture source | `sol extract`, read/orchestrate plans |
| `.pma` + manifest | verified PMA snapshot boot pair | existing nockapp snapshot export | `sol quick-read-bench`, `sol bench`, `sol extract` |
| `.solarch` | extracted accepted-block archive plus raw transaction replay payloads | `sol extract` | inspection, replay, and fixture tooling |
| `.soltest` | fixture bundle: checkpoint + archive + kernel | existing fixture source or out-of-tree generation | `sol quick-bench`, `sol bench`, replay sweeps |
| orchestrate plan JSON | boot source/kernel plus ordered operations | operator or sweep shorthand | `sol quick-orchestrate`, `sol bench`, `sol sweep` |
| sweep artifact tree | trusted benchmark record | `sol sweep` | `scripts/bench_pages` |

PMA replay consumes one boot source for the selected workflow: a `.soltest`
fixture, a checkpoint, or a snapshot PMA plus manifest. Commands that boot a
runtime still take a kernel path, defaulting to `assets/dumb.jam` where the CLI
exposes a default. Snapshot boot copies the source PMA into a per-run replay
PMA before opening it, so source snapshot files remain read-only benchmark
inputs.

## Build

Build and test the release binary directly:

```bash
cargo build -p nockchain-bench --release
cargo test -p nockchain-bench --release
```

Replay, read, and orchestrate commands expose `--fsync on|off`. The default is
`on`. For benchmark evidence, prefer fsync on unless the experiment is
explicitly about disabling durability.

## Archive And Fixture Workflow

### Extract `.solarch`

`sol extract` boots a checkpoint or PMA snapshot with a jammed kernel and
records accepted blocks into a replay archive.

```bash
./target/release/nockchain-bench sol extract \
  --checkpoint ./checkpoints/0.chkjam \
  --kernel ./assets/dumb.jam \
  --start-height 0 \
  --end-height 1000 \
  --output ./tmp/first-1001.solarch
```

Snapshot extraction uses the same archive format, but boots from an existing
PMA/manifest pair:

```bash
./target/release/nockchain-bench sol extract \
  --snapshot-pma ./snapshots/before-window/snapshot.pma \
  --snapshot-manifest ./snapshots/before-window/snapshot.manifest \
  --kernel ./snapshots/before-window/kernel.jam \
  --start-height 10014 \
  --blocks 100 \
  --output ./tmp/blocks-10014-10113-with-txs.solarch
```

Rules:

- `--start-height` is inclusive.
- `--end-height` is inclusive and overrides `--blocks`.
- If `--end-height` is omitted, `--blocks` controls how many accepted blocks to
  extract from `--start-height`.
- Per-block raw transaction payloads are included and are required for complete
  replay of transaction-bearing blocks.
- `--include-mempool` preserves mempool snapshots when available. Mempool
  snapshots are diagnostic only; they are not replay payloads.

### Inspect `.soltest`

```bash
./target/release/nockchain-bench sol fixture inspect \
  --fixture ./fixtures/first-100-derived-checkpoint-no-mempool.soltest
```

Inspect output includes checkpoint kind, embedded height/event, archive replay
range, raw transaction counts, mempool presence, hashes, and embedded payload
sizes.

`sol inspect` is a stale-mempool diagnostic view. It also reports archive
raw transaction counts, but mempool snapshots should not be used as the source
of replay transactions.

## Quick Commands

Quick commands are for local investigation. They are useful, but they are not
published benchmark evidence.

### `sol quick-bench`

```bash
./target/release/nockchain-bench sol quick-bench \
  --fixture ./fixtures/first-100-derived-checkpoint-no-mempool.soltest \
  --blocks 10 \
  --profile-memory \
  --profile-output ./tmp/quick-bench-memory.json
```

Supported investigation knobs include memory profiling, page-fault burst
thresholds, `--fsync on|off`, and an optional extra `samply` CPU profiling
pass:

```bash
./target/release/nockchain-bench sol quick-bench \
  --fixture ./fixtures/first-100-derived-checkpoint-no-mempool.soltest \
  --cpu-profiler samply \
  --cpu-profile-output ./tmp/quick-bench-profile.json.gz
```

### `sol quick-read-bench`

Use this for checkpoint- or snapshot-backed `%heavy-n` peek investigation
without building a fixture.

```bash
./target/release/nockchain-bench sol quick-read-bench \
  --checkpoint ./checkpoints/0.chkjam \
  --kernel ./assets/dumb.jam \
  --start-height 1 \
  --count 100 \
  --profile-output ./tmp/quick-read-summary.json
```

Snapshot boot uses an explicit PMA/manifest pair instead of `--checkpoint`:

```bash
./target/release/nockchain-bench sol quick-read-bench \
  --snapshot-pma ./snapshots/first-100/snapshot.pma \
  --snapshot-manifest ./snapshots/first-100/snapshot.manifest \
  --kernel ./snapshots/first-100/kernel.jam \
  --start-height 0 \
  --count 1 \
  --dry-run
```

Range rules:

- `--start-height` is inclusive.
- `--count N` peeks `N` heights from `--start-height`.
- `--end-height N` is inclusive and conflicts with `--count`.
- `--dry-run` boots, resolves the range, and exits before peeking.

### `sol quick-orchestrate`

Use this when you need one shared runtime and a hand-authored ordered plan.

```bash
./target/release/nockchain-bench sol quick-orchestrate \
  --plan ./tmp/plan.json \
  --profile-output ./tmp/orchestrate-summary.json
```

`--cold-mode strict|soft` controls how cold verification failures are handled
for quick runs. `--fsync on|off` controls PMA durability.

## Trusted Benchmarks

Use `sol bench` for auditable single-case measurements.

```bash
mkdir -p ./tmp/native-bench-example
./target/release/nockchain-bench sol bench \
  --fixture ./fixtures/first-100-derived-checkpoint-no-mempool.soltest \
  --output ./tmp/native-bench-example \
  --warmup-runs 0 \
  --measured-runs 3 \
  --cooldown-secs 0
```

Trusted protocol:

- Use a release binary unless intentionally passing `--allow-debug-benchmark`.
- `--output` must exist and be empty.
- Trusted measured runs require `--measured-runs >= 3`.
- `summary.json`, `verdict.json`, `resolved_case.json`, `provenance.json`, and
  per-run directories are the record of truth.
- `--cv-threshold` controls the maximum coefficient of variation before the
  primary throughput verdict becomes `Partial`.

### Docker Trusted Bench

```bash
mkdir -p ./tmp/docker-bench-example
./target/release/nockchain-bench sol bench \
  --fixture ./fixtures/first-100-derived-checkpoint-no-mempool.soltest \
  --output ./tmp/docker-bench-example \
  --docker-build-tag nockchain-bench:local \
  --memory-limit 8g \
  --work-dir-mode docker-tmpfs \
  --warmup-runs 0 \
  --measured-runs 3 \
  --cooldown-secs 0
```

Docker mode requires exactly one of `--docker-image` or `--docker-build-tag`,
plus `--memory-limit` and `--work-dir-mode`.

Work directory modes:

| Mode | Meaning |
| --- | --- |
| `host-bind` | host directory mounted into the container |
| `docker-volume` | Docker-managed volume |
| `docker-tmpfs` | tmpfs-backed container work directory |

Docker trusted provenance records host/container binary identity, image source,
resolved image id/digest, container id, Docker engine facts, and realized cgroup
limits where available. By default, host/container version or commit skew makes
the run invalid. Use `--allow-version-skew` only when that drift is intentional.

Some Docker Desktop setups require an explicit socket path:

```bash
DOCKER_HOST=unix:///path/to/docker.sock \
  ./target/release/nockchain-bench sol bench ...
```

### Docker Validation

`sol validate` checks Docker image/resource realization without replay.

```bash
mkdir -p ./tmp/docker-validate-example
./target/release/nockchain-bench sol validate \
  --fixture ./fixtures/first-100-derived-checkpoint-no-mempool.soltest \
  --output ./tmp/docker-validate-example \
  --docker-build-tag nockchain-bench:local \
  --memory-limit 8g \
  --work-dir-mode docker-tmpfs
```

It writes the requested/resolved scaffold, validation evidence, raw Docker
facts, and a sibling validation cache.

## Orchestrate Plans

Trusted `sol bench` accepts three mutually exclusive workload modes. Read
shorthand accepts exactly one boot source: either `--checkpoint` or the
`--snapshot-pma`/`--snapshot-manifest` pair.

| Input mode | CLI fields | Use case |
| --- | --- | --- |
| Fixture replay shorthand | `--fixture`, `--blocks` | poke archive blocks from `.soltest` |
| Read shorthand | `--checkpoint` or `--snapshot-pma` + `--snapshot-manifest`, plus `--kernel`, `--start-height`, `--count` or `--end-height`, `--peek-mode` | generated peek plans |
| Explicit plan | `--plan` | exact ordered poke/peek/cold operations |

Native snapshot read example:

```bash
mkdir -p ./tmp/native-snapshot-read
./target/release/nockchain-bench sol bench \
  --snapshot-pma ./snapshots/first-100/snapshot.pma \
  --snapshot-manifest ./snapshots/first-100/snapshot.manifest \
  --kernel ./snapshots/first-100/kernel.jam \
  --start-height 0 \
  --count 1 \
  --output ./tmp/native-snapshot-read \
  --warmup-runs 0 \
  --measured-runs 3 \
  --cooldown-secs 0
```

Explicit plan example:

```json
{
  "schema_version": "orchestrate-plan/v2",
  "boot": {
    "type": "checkpoint",
    "checkpoint": "./checkpoints/full.chkjam"
  },
  "kernel": "./assets/dumb.jam",
  "steps": [
    {
      "type": "force_cold",
      "label": "force-cold-before-read",
      "tolerance_pages": 100,
      "max_attempts": 3
    },
    {
      "type": "peek_height",
      "height": 1,
      "label": "expected-cold-001",
      "cache_expectation": "cold"
    },
    {
      "type": "peek_height",
      "height": 1,
      "label": "expected-warm-repeat-001",
      "cache_expectation": "warm"
    }
  ]
}
```

For explicit snapshot-backed plans, use the same tagged `boot` shape:

```json
{
  "schema_version": "orchestrate-plan/v2",
  "boot": {
    "type": "snapshot",
    "pma": "./snapshots/first-100/snapshot.pma",
    "manifest": "./snapshots/first-100/snapshot.manifest"
  },
  "kernel": "./snapshots/first-100/kernel.jam",
  "steps": [
    { "type": "peek_height", "height": 0, "cache_expectation": "warm" }
  ]
}
```

Normalized trusted plans use `trusted-plan/v2`. Run records use
`run-result/v2`. Trusted read artifacts record snapshot boot inputs as
`snapshot-pma-0` and `snapshot-manifest-0`, and provenance records
`boot_source: "snapshot"` with the snapshot manifest event number.

Plan step types:

| Step | Fields | Meaning |
| --- | --- | --- |
| `poke_archive_block` | `height`, archive input from fixture/inventory | replay one archive block |
| `poke_archive_range` | `start_height`, `end_height`, optional `label_prefix` | expands to block pokes |
| `peek_height` | `height`, optional `label`, optional `cache_expectation` | issue one `%heavy-n` peek |
| `peek_height_range` | `start_height`, `end_height`, `peek_mode`, optional `cache_expectation` | expands to peeks |
| `force_cold` | optional `cold_target`, `tolerance_pages`, `max_attempts` | request page-cache eviction before later peeks |
| `peek_height_cold` | `height`, optional cold fields | explicit cold peek shorthand |

`poke_archive_block` is complete block replay: it prebuilds the block fact,
pokes `%heard-block`, then pokes each archived raw transaction for that block as
`%heard-tx`. Per-step artifacts retain block poke duration, raw transaction poke
duration, slab prebuild duration, prebuild RSS range, raw transaction slab
count, raw transaction payload bytes, and `raw_tx_pokes_completed`.

`cache_expectation` is a reporting hint for downstream consumers such as
`bench_pages`. Valid values are:

| Value | Meaning |
| --- | --- |
| `cold` | the following peek is expected to observe cold cache behavior |
| `warm` | the following peek is expected to reuse resident data |
| `ambient` | no cold guarantee, but not intended as a warmed repeat |
| `unknown` | intentionally unspecified |

Plans that omit `cache_expectation` infer cold context for peeks after
`force_cold` until the next operation that invalidates that context. Plans
that explicitly set `"cache_expectation": "unknown"` remain unknown.

Cold evidence is a warning/verdict dimension, not just a hard pass/fail. A
partial pageout is recorded as degraded cold evidence so the report can show
what happened without failing to produce benchmark artifacts.

## Sweep Matrices

`sol sweep` expands a JSON matrix into trusted `sol bench` cases.

```json
{
  "benchmark": "sol-orchestrate",
  "base": {
    "fixture": "./fixtures/first-100-derived-checkpoint-no-mempool.soltest",
    "warmup_runs": 0,
    "measured_runs": 3,
    "cooldown_secs": 0,
    "profile_memory": true,
    "profile_interval_ms": 500,
    "mode": {
      "docker": {
        "image": {
          "auto_build": {
            "tag": "nockchain-bench:local"
          }
        },
        "memory_limit": "8g",
        "work_dir_mode": "DockerTmpfs",
        "allow_version_skew": false
      }
    }
  },
  "axes": {
    "memory_limit": ["8g", "16g"]
  }
}
```

Run it:

```bash
mkdir -p ./tmp/live-sol-sweep
./target/release/nockchain-bench sol sweep \
  --matrix ./tmp/matrix.json \
  --output ./tmp/live-sol-sweep \
  --comparison-markdown
```

`benchmark` is currently `sol-orchestrate`.

`base` fields include:

| Field | Meaning |
| --- | --- |
| `fixture` | `.soltest` for replay shorthand |
| `plan` | explicit orchestrate plan path |
| `checkpoint`, `kernel`, `start_height`, `count`, `end_height`, `peek_mode` | read shorthand |
| `blocks` | prefix replay count for fixture replay; `0` means full fixture window |
| `profile_memory`, `profile_interval_ms` | memory and page-fault sampling |
| `threads` | logical metadata axis |
| `warmup_runs`, `measured_runs`, `cooldown_secs` | repetition schedule |
| `cv_threshold` | primary throughput stability policy |
| `label` | human label persisted into artifacts |
| `fsync` | PMA replay durability mode; defaults on |
| `mode.native` or `mode.docker` | execution backend |

Supported axes include replay/read fields, `fixture`, `plan`, Docker image and
resource fields, `work_dir_mode`, `allow_version_skew`, and PMA `fsync`.

Fixture-axis variation is allowed and is treated as expected axis variation.
Different fixture hashes, checkpoint hashes, archive windows, and kernel hashes
do not make the comparison invalid merely because `fixture` is an axis.

Sweep scheduling flags:

```bash
./target/release/nockchain-bench sol sweep --matrix ./tmp/matrix.json --output ./tmp/out
./target/release/nockchain-bench sol sweep --matrix ./tmp/matrix.json --output ./tmp/out --interleave
./target/release/nockchain-bench sol sweep --matrix ./tmp/matrix.json --output ./tmp/out --randomize-order
```

## Memory, Fault, and CPU Profiling

Memory profiling:

- `--profile-memory` enables memory timeline sampling.
- `--profile-interval-ms` controls sample cadence.
- Docker trusted runs also record cgroup memory realization.
- Page-fault totals and per-step deltas appear when the platform and command
  surface can collect them.

CPU profiling:

- `sol quick-bench` supports `--cpu-profiler samply`.
- `sol quick-read-bench` supports an extra profiled `quick-read-once` pass.
- `sol sweep` supports trusted CPU profiling per case.
- Direct `sol bench` intentionally does not expose CPU profiling flags.
- Linux hosts generally require `kernel.perf_event_paranoid <= 1`.
- Docker profiling requires an image containing both `nockchain-bench` and
  `samply`, plus container perf permissions.

Build images:

```bash
scripts/build_nockchain_bench_image.sh \
  --variant standard \
  --tag nockchain-bench:local

scripts/build_nockchain_bench_image.sh \
  --variant profiling \
  --tag nockchain-bench:local-samply
```

Build from an existing binary:

```bash
scripts/build_nockchain_bench_image.sh \
  --variant standard \
  --tag nockchain-bench:pma-local \
  --binary /path/to/pma-checkout/target/release/nockchain-bench \
  --skip-cargo-build
```

## Bench Pages Reports

Publish a completed sweep locally:

```bash
uv run --project scripts/bench_pages publish-sweep \
  --sweep-root ./tmp/live-sol-sweep \
  --output-dir ./tmp/live-sol-pages \
  --replace \
  --no-publish-ghcr
```

Publish to GitHub Pages/GHCR:

```bash
uv run --project scripts/bench_pages publish-sweep \
  --sweep-root ./tmp/live-sol-sweep \
  --push
```

Reports include:

- sweep status and verdict
- plan quick summary with block ranges and cache expectations
- operation health
- cross-case comparison
- typed peek throughput (`Cold`, `Warm`, `Ambient`, `Unknown`) when plan hints
  are present
- run-spread strip charts
- case workspace
- evidence and artifact browsers
- optional profile links
- raw transaction replay panels for transaction-bearing runs

Trusted replay metrics emit `block_pokes_per_second` for archive-block steps
and `raw_tx_pokes_per_second` for transaction payload pokes.

When `runs/*/steps.ndjson` is present, `bench_pages` streams it into compact
per-run and per-case raw transaction summaries. The rendered report shows raw
tx throughput, raw tx poke progress, slab prebuild timing, payload bytes, and
prebuild RSS range. Failed raw-tx step samples are bounded in `manifest.json`
and HTML with an omitted-count summary; the full `steps.ndjson` evidence stays
published in the artifact tree and bundle.

The report intentionally does not publish PMA work files as page artifacts.
Those files can be large and are runtime scratch state, not benchmark evidence.

## PMA Runtime Notes

PMA replay is the normal runtime for current master. Trusted replay, read, and
orchestrate benchmark paths use PMA boot internally and record PMA provenance in
the artifact tree.

Build a Docker image from the current release binary:

```bash
DOCKER_HOST=unix:///path/to/docker.sock \
  scripts/build_nockchain_bench_image.sh \
  --variant standard \
  --tag nockchain-bench:pma-local \
  --binary /path/to/pma-checkout/target/release/nockchain-bench \
  --skip-cargo-build
```

Run a trusted Docker sweep:

```bash
DOCKER_HOST=unix:///path/to/docker.sock \
NOCKCHAIN_BENCH_COLD_TARGET=pma_replay_nockstack \
  ./target/release/nockchain-bench \
  sol sweep \
  --matrix /path/to/matrix.json \
  --output /path/to/sweep-artifacts \
  --comparison-markdown
```

Docker Desktop only makes configured host paths visible to containers. Put
Docker-consumed checkpoints, kernels, plans, matrices, and output roots under a
host path Docker can mount, or adjust Docker Desktop file-sharing settings.

Runtime notes:

- Prefer fsync on for PMA benchmark evidence.

## `--blocks` Semantics

`--blocks N` is prefix replay, not arbitrary slicing.

- `--blocks 0` replays the full fixture archive window.
- `--blocks N` replays the first `N` accepted blocks from that fixture window.
- To benchmark another range, build another fixture or use an explicit
  orchestrate/read plan.

## Verdicts And CV

Trusted verdicts are policy outcomes over successful artifact generation.

| Verdict | Meaning |
| --- | --- |
| `Valid` | requested evidence completed within policy |
| `Partial` | artifacts exist, but stability or policy exceptions were observed |
| `Invalid` | evidence should not be used for trusted comparison |

Throughput CV is the coefficient of variation: standard deviation divided by
mean across measured runs. The default threshold is `0.10`; a primary
throughput CV above that marks the case/sweep partial unless the threshold is
changed.

## Troubleshooting

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| Docker unavailable from harness | in-process client cannot find Docker Desktop socket | set `DOCKER_HOST=unix:///path/to/docker.sock` |
| Docker cannot see inputs | Docker Desktop file sharing excludes the input path | move inputs/output under a host path Docker can mount or update Docker Desktop file sharing |
| trusted Docker run invalid | host/container binary identity drift | rebuild image from current release binary or pass `--allow-version-skew` only intentionally |
| output directory rejected | trusted output is missing or not empty | create an empty directory before running |
| `samply` fails on Linux | perf permissions too strict | lower `kernel.perf_event_paranoid` or run with appropriate privileges |
| page tree unexpectedly huge | runtime scratch/PMA files included as artifacts | keep only trusted sweep artifacts in bench_pages publication |
| raw transaction panel missing | sweep has no raw-tx step evidence or no `steps.ndjson` files | regenerate archives with `sol extract` and publish a sweep that preserves run step artifacts |

## Limitations

- Direct `sol bench` does not expose CPU profiling flags.
- Existing `.soltest` fixture replay remains supported when the embedded archive
  includes the raw transaction payloads needed for complete replay.
- `--include-mempool` support exists but should be treated cautiously unless
  the fixture is independently inspected; mempool snapshots are diagnostic, not
  replay payloads.
- Mixed native and Docker cases in a single sweep are not the primary supported
  comparison shape; prefer one backend per sweep.
- Docker cgroup evidence depends on what the local Docker engine exposes.
