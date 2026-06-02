# Spec: Trustworthy SOL Benchmark Harness

## Status

This is the canonical first-release spec for `nockchain-bench`. For operator
commands and current examples, prefer `crates/nockchain-bench/README.md`; this
spec captures the harness contract behind those commands.

The harness keeps:
- one shared once-run engine
- one shared trusted orchestration contract
- backend adapters for native and Docker

Current implementation addendum for current PMA master:
- trusted sweep matrices use `benchmark: "sol-orchestrate"`
- trusted orchestrate/read plans may annotate peek steps with
  `cache_expectation: "cold" | "warm" | "ambient" | "unknown"`
- `cache_expectation` is a reporting hint for downstream consumers, not a
  separate runtime operation; explicit `unknown` remains unknown, while plans
  that omit the field infer cold context after `force_cold`
- `bench_pages` reports typed peek throughput columns only for cache
  expectation types present in the plan
- PMA replay is the normal runtime; no separate feature or checkout sync is
  required
- trusted replay is supported for existing `.soltest` fixtures and explicit
  orchestrate/read plans
- snapshot boot is supported for read shorthand, explicit orchestrate plans,
  trusted bench/sweep flows, and archive extraction through a PMA/manifest pair
- `.solarch` archives carry accepted blocks plus per-block raw transaction
  payloads; `poke_archive_block` replays the block fact and that block's raw
  transaction facts
- archive reads validate that transaction-bearing blocks include matching raw
  transaction payloads; there is no incomplete-replay override path
- trusted replay artifacts carry `block_pokes_per_second`,
  `raw_tx_pokes_per_second`, per-step raw transaction progress, slab prebuild
  timing, prebuild RSS range, raw transaction slab counts, and raw transaction
  payload byte counts
- `bench_pages` surfaces snapshot boot context and raw transaction replay
  summaries from trusted artifacts; it keeps full `steps.ndjson` evidence while
  bounding rendered failure samples
- PMA identity remains additive top-level provenance:
  `runtime_flavor`, `boot_source`, `boot_event_num`, and
  `pma_work_dir_mode`
- Docker operators use the existing CLI flags `--docker-build-tag` and
  `--docker-image`
- on the maintained Docker Desktop setup, the expected Docker context is
  `desktop-linux`, and host-side fallbacks may need
  `DOCKER_HOST=unix:///home/drbeefsupreme/.docker/desktop/docker.sock`

## 1. Purpose

Build a trustworthy benchmark harness for SOL replay workloads in
`nockchain-bench`.

The harness must make four things explicit and auditable:
- requested inputs
- resolved execution plan
- realized execution environment
- raw measurement evidence

Mining benchmarks are out of scope and are removed before any new harness work
begins.

## 2. Scope

This spec applies only to SOL replay benchmarking driven by `SolBenchRunner`
and the unified `.soltest` fixture format.

It does not apply to:
- mining benchmarks
- `MiningScenario`
- event correlation for mining logs
- Parquet export for mining stats
- the removed pre-harness sweep implementation

## 3. Design Axioms

1. One measured run equals one fully specified case.
2. One trustworthy comparison changes only declared axes.
3. Raw evidence is retained even when parsed summaries exist.
4. Requested configuration and realized provenance are separate artifacts.
5. Trusted Docker runs require validation.
6. Trusted results require release builds.
7. Sweep orchestration contains no measurement logic.
8. Human labels never drive logic.
9. Invalid runs are preserved, not discarded.
10. Prefix replay is a first-class supported mode; arbitrary in-fixture slicing
    is not required in v1.
11. Native and Docker trusted runs share one orchestration contract, not only
    one once-run engine.

## 4. Phase 0: Hard Deletion Boundary

Phase 0 is a clean break. No transitional stubs.

### 4.1 Delete Entire Subsystems

Delete these directories or modules entirely:
- `src/scenario/`
- `src/events/`
- `src/output/`
- `src/runner/`
- `src/commands/mining.rs`
- `src/speed_of_light/sweep.rs`

### 4.2 Delete CLI Surfaces

Remove these top-level commands from `src/main.rs`:
- `Run`
- `Attach`
- `Compare`
- `Analyze`

Remove the current `sol sweep` subcommand entirely. It will be replaced later.

### 4.3 Delete Mining-Specific Types And Re-exports

Remove:
- `MiningScenario`
- `MiningScenarioConfig`
- `MiningResult`
- `NockchainMode`
- `OutputFormat`
- all re-exports tied to mining, events, parquet, or the old runner

### 4.4 Cargo Dependency Cleanup

After Phase 0, remove dependencies that only supported deleted subsystems.
At minimum, reevaluate and likely remove:
- `arrow`
- `parquet`
- `chrono`

Keep Docker-related dependencies (`bollard`, `futures`) because the new SOL
harness will still need them.

### 4.5 Salvage Generic Helpers

Carry the generic Docker/cgroup v2 helpers salvaged from the deleted
`src/runner/docker.rs` into the new `speed_of_light::harness::docker` module:

- Docker connection logic
- `ContainerStats` and `from_docker_stats`
- `parse_memory_limit`
- `parse_proc_stat_faults`
- `calculate_cpu_percent`
- page fault reading via `docker exec` of `/proc/1/stat`

Do not preserve the old module structure or mining-era API surface.

### 4.6 Phase 0 Exit Criteria

After deletion:
- `cargo build -p nockchain-bench --release` passes
- `cargo test -p nockchain-bench --release` passes
- remaining CLI surface is SOL-focused plus `sample`

Remaining commands:
- `sample`
- `sol quick-bench`
- `sol extract`
- `sol inspect`
- `sol fixture inspect`

## 5. Module Layout

Create a new SOL-specific harness module tree under
`src/speed_of_light/harness/`.

```text
src/speed_of_light/harness/
├── mod.rs
├── case.rs
├── artifacts.rs
├── provenance.rs
├── summary.rs
├── execute.rs        # shared once-run execution contract
├── orchestrate.rs    # shared trusted orchestration pipeline      (Phase 2+)
├── native.rs         # native backend adapter
├── docker.rs         # Docker backend adapter and Docker helpers
├── validate.rs       # Docker validation gate and probe protocol
└── sweep.rs          # matrix expansion and orchestration only
```

Notes:
- `orchestrate.rs` is introduced at the Phase 2 boundary by extracting the
  orchestration loop from `native.rs`
- Phase 1 may keep orchestration inline in `native.rs` because native is the
  only backend at that point
- once Phase 2 begins, the shared orchestration logic becomes a first-class
  module and both backends must use it

## 6. Reuse From Existing Code

Reuse these existing SOL pieces:
- `SolBenchRunner`
- `SolBenchConfig`
- `SolBenchResults`
- `MemoryProfile`
- `ProcessMemoryProfiler`
- `SolScorecard`
- `SolFixtureManifest`
- fixture parsing and extraction helpers
- archive reader/writer
- checkpoint builder and extractor utilities
- `sampler::smaps`, `sampler::buckets`

Do not make mining-oriented abstractions part of the new design.

## 7. Benchmark Semantics

### 7.1 `--blocks`

In v1, `--blocks N` means:
- replay at most the first `N` replayable blocks from the fixture's archive
  window

It does not mean:
- slice the fixture file physically
- skip into the middle of the fixture window
- benchmark an arbitrary offset without building a different fixture

### 7.2 Fixture Handling

In v1:
- the fixture is treated as an immutable input blob
- full fixture extraction is acceptable
- fixture bind-mounting into Docker is acceptable
- partial extraction optimization is out of scope

## 8. Execution Architecture

The execution model has three layers:

1. one shared once-run engine
2. one shared trusted orchestrator
3. backend adapters for native and Docker

The orchestration contract is:
"run one resolved case N times under one backend, persist all artifacts, and
emit summary and verdict."

### 8.1 Shared Once-Run Engine

Introduce one shared execution entrypoint used by:
- native trusted runs
- Docker trusted runs
- `sol quick-bench`

This must be a library-level operation, not a parser of human-readable CLI
output.

The once-run engine owns only:
- one execution of one resolved case
- one run directory
- per-run results and per-run artifacts

It does not own:
- repetition policy
- cooldown
- summary math
- verdict policy
- validation
- backend lifecycle

### 8.2 Hidden `sol run-once`

A hidden/internal CLI is required for container execution, for example:

- `nockchain-bench sol run-once --resolved-case /bench/input/resolved_case.json --run-dir /bench/output/run-0`

`sol run-once` is a machine-oriented wrapper over the shared once-run engine.
It exists so the host-side Docker backend can invoke the same execution path
inside the container without parsing stdout or duplicating replay logic.

### 8.3 Shared Trusted Orchestrator

Trusted benchmarking must use one shared orchestration pipeline for both native
and Docker backends.

The orchestrator owns:
- output-root preparation
- `requested_case.json`
- `resolved_case.json`
- shared provenance assembly and writing
- warmup and measured run scheduling
- cooldown policy
- failure accounting
- `summary.json`
- `verdict.json`
- common cleanup/error-handling rules

The orchestrator does not own backend-specific runtime setup. It delegates that
to a backend adapter.

Implementation timing:
- Phase 1: orchestration may live inline in `native.rs`
- Phase 2: extract that logic into `orchestrate.rs` before adding Docker

### 8.4 Backend Adapter Contract

Each backend adapter must implement a contract equivalent to:

```rust
pub trait BenchBackend {
    async fn prepare(
        &mut self,
        resolved: &ResolvedCase,
        output_root: &Path,
    ) -> Result<()>;

    /// Called immediately after prepare() and before the first measured block.
    /// These facts are merged into provenance.json.
    fn capture_runtime_facts(&self) -> Result<BackendRuntimeFacts>;

    async fn execute_run(
        &mut self,
        resolved: &ResolvedCase,
        run_dir: &Path,
        run_id: &str,
    ) -> Result<CompletedRun>;

    async fn capture_raw_evidence(&self, raw_dir: &Path) -> Result<()>;

    async fn cleanup(&mut self) -> Result<()>;
}
```

The backend adapter must not own:
- summary math
- verdict policy
- repetition scheduling
- shared artifact naming

`capture_runtime_facts()` exists because `provenance.json` is defined as the
realized environment after setup and before the first measured block. It must
not depend on post-run container state.

`BackendRuntimeFacts` must distinguish native and Docker contributions:

```rust
pub enum BackendRuntimeFacts {
    Native,
    Docker {
        host_binary: BinaryIdentity,
        container_binary: BinaryIdentity,
        image_source: DockerImageSource,
        requested_image_ref: String,
        resolved_image_ref: String,
        image_digest: String,
        container_id: String,
        docker_engine_version: String,
        docker_context: String,
        cgroup_version: String,
        storage_driver: String,
        realized_memory_max: u64,
        realized_memory_current: u64,
        realized_cpuset: Option<String>,
        realized_cpu_max: Option<String>,
    },
}
```

The orchestrator merges `BackendRuntimeFacts` with shared fields and writes one
`provenance.json`.

### 8.5 Relationship To `sol bench`

`sol bench` is the public trusted interface.

There is one trusted command:
- `sol bench`

There are multiple backends behind it:
- native
- Docker

Direct `sol bench` remains the trusted warmup/measured execution surface. CPU
profiling is layered onto `sol quick-bench` for ad hoc single runs and onto
`sol sweep` for trusted per-case profiling; it is not a direct `sol bench`
flag surface in this design.

### 8.6 Relationship To `sol quick-bench`

`sol quick-bench` remains the quick ad hoc interface.

Recommended structure:
- factor replay execution into the shared once-run engine
- let `sol quick-bench` call that engine directly
- let `sol bench` call the trusted orchestrator
- let Docker use `sol run-once` inside the container

CPU profiling policy:
- `sol quick-bench` may run one extra profiled replay pass and write the raw
  profile to an operator-selected output path
- `sol sweep` may run one extra profiled replay pass per trusted case
- trusted measured-run statistics never include the extra profiled pass
- native profiling wraps the shared hidden `sol run-once` entrypoint with host
  `samply`
- Docker profiling wraps `sol run-once` with in-container `samply`
- native profiling should fail early on Linux when
  `kernel.perf_event_paranoid > 1`

## 9. Data Model

### 9.1 RequestedCase

Requested input only. No auto-captured facts.

```rust
pub struct RequestedCase {
    pub benchmark: String,                // "sol-orchestrate"
    pub label: Option<String>,

    pub fixture_path: PathBuf,
    pub blocks: u64,                      // 0 = all in fixture window
    pub skip_genesis: bool,

    pub profile_memory: bool,
    pub profile_interval_ms: u64,

    pub execution: ExecutionRequest,
    pub threads: u32,

    pub warmup_runs: u32,                 // default 1
    pub measured_runs: u32,               // default 5, minimum 3
    pub cooldown_secs: u64,               // default 10
}

pub enum ExecutionRequest {
    Native,
    Docker {
        image: DockerImageSource,
        memory_limit: String,
        cpuset: Option<String>,
        cpu_quota: Option<i64>,
        cpu_period: Option<i64>,
        work_dir_mode: WorkDirMode,
        allow_version_skew: bool,
    },
}

pub enum DockerImageSource {
    Provided { ref: String },
    AutoBuild { tag: String },
}

pub enum WorkDirMode {
    HostBind,
    DockerVolume,
    DockerTmpfs,
}
```

Additional runner-specific fields are allowed only under one of these rules:
- they are documented in the spec because they can affect benchmark behavior,
  artifact interpretation, or comparison invariants
- or they are explicitly namespaced as engine-internal diagnostics that do not
  affect trust semantics

The spec must remain authoritative for requested inputs that can materially
change replay behavior, summary interpretation, or trusted comparison.

### 9.2 ResolvedCase

Normalized execution plan after applying defaults and computing static inputs.

Includes:
- absolute paths
- parsed memory-limit bytes
- fixture SHA256
- embedded fixture manifest
- schema version
- build profile
- tool version and commit
- normalized backend configuration

Does not include runtime facts like container id or realized cgroup values.

### 9.3 Provenance

`provenance.json` records the realized environment after backend setup and
before the first measured block.

The orchestrator captures shared fields:
- schema version
- capture timestamp
- host identity
- git identity
- fixture identity
- binary identity

The backend contributes `BackendRuntimeFacts` immediately after `prepare()`.

For Docker mode this includes:
- `host_binary`
- `container_binary`
- image source, requested launch ref, and resolved immutable image identity
- container id
- Docker engine version
- Docker context
- cgroup version (`2` only; non-v2 environments are rejected)
- storage driver
- realized `memory.max` from `/sys/fs/cgroup/memory.max`
- realized `memory.current` snapshot from `/sys/fs/cgroup/memory.current`
- realized cpuset when the cpuset controller is exposed; otherwise `null`
- realized `cpu.max` from `/sys/fs/cgroup/cpu.max` when the CPU controller is
  exposed; otherwise `null`

Registry digests are preferred when available. When Docker reports no
`RepoDigests` for a local-only image, the harness falls back to the Docker
image ID and uses that value as both the launch identity and the trusted
immutable identity recorded in `backend.image_digest`.

The orchestrator merges shared fields and backend runtime facts and writes one
`provenance.json` before measured execution begins.

### 9.4 Version Skew Policy

Trusted Docker mode requires host/container binary version agreement by default.

- record host binary version/commit
- record container binary version/commit
- mark the run `Invalid` unless they match, or unless
  `--allow-version-skew` is set

### 9.5 Verdict

```rust
pub enum Validity {
    Valid,
    Partial { reasons: Vec<String> },
    Invalid { reasons: Vec<String> },
}
```

Examples:
- `Invalid`: requested memory limit does not match realized limit
- `Invalid`: debug build used without `--allow-debug-benchmark`
- `Invalid`: host/container binary mismatch without `--allow-version-skew`
- `Partial`: one measured repetition failed
- `Partial`: throughput CV exceeded threshold

## 10. Artifact Model

### 10.1 Single Trusted Run

```text
<output_root>/
├── schema_version.txt
├── requested_case.json
├── resolved_case.json
├── provenance.json
├── validation.json                 # Docker only, Phase 3+
├── verdict.json
├── summary.json
├── raw/
│   ├── host_env.json
│   ├── docker_inspect.json         # Docker only
│   ├── docker_info.json            # Docker only
│   └── container_env.json          # Docker only
├── runs/
│   ├── warmup-0/
│   │   ├── result.json
│   │   ├── profile.json
│   │   ├── block_timings.ndjson
│   │   ├── stdout.log
│   │   └── stderr.log
│   ├── run-0/
│   │   ├── result.json
│   │   ├── profile.json
│   │   ├── block_timings.ndjson
│   │   ├── container_samples.ndjson   # Docker only
│   │   ├── stdout.log
│   │   └── stderr.log
│   └── ...
```

Artifact ownership:
- orchestrator-owned:
  - `schema_version.txt`
  - `requested_case.json`
  - `resolved_case.json`
  - `provenance.json`
  - `summary.json`
  - `verdict.json`
- backend-owned additions:
  - backend-specific raw evidence under `raw/`
  - backend-specific per-run evidence such as `container_samples.ndjson`

Warmups are persisted but excluded from summary statistics.

### 10.2 Sweep Output

```text
<sweep_root>/
├── schema_version.txt
├── matrix.json
├── matrix_expanded.json
├── schedule.json
├── verdict.json
├── comparison.json
├── comparison.md                 # optional
└── cases/
    ├── case-000-memory_limit_4g/
    ├── case-001-memory_limit_8g/
    └── ...
```

## 11. Backend Models

### 11.1 Native Backend

The native backend runs the once-run engine directly on the host.

It is responsible for:
- host-side environment preparation
- `raw/host_env.json`
- `BackendRuntimeFacts::Native`
- direct invocation of the once-run engine

### 11.2 Docker Backend Core Rule

In Docker mode, the SOL replay workload must run inside the constrained
container, and trusted execution only supports cgroup v2.

### 11.3 What The Docker Backend Does On The Host

The Docker backend on the host:
- validates requested Docker parameters
- rejects Docker environments whose reported `CgroupVersion` is not `2` before
  creating or starting the benchmark container
- resolves image tag to digest
- captures Docker engine facts
- creates the container with requested resource limits
- bind-mounts fixture input read-only at `/bench/fixture.soltest`
- bind-mounts artifact output at `/bench/output/`
- configures work dir at `/bench/work/` per `work_dir_mode`
- polls container-level stats concurrently during execution
- gathers raw inspect/info evidence
- collects exit status and logs

### 11.4 What The Docker Backend Does In The Container

The container executes the shared once-run engine via hidden/internal CLI:

```bash
nockchain-bench sol run-once \
  --resolved-case /bench/input/resolved_case.json \
  --run-dir /bench/output/run-0
```

The trusted Docker path must not depend on `sol quick-bench` stdout.

### 11.5 Image Requirements

The image must contain:
- `nockchain-bench` release binary
- `samply` on `PATH` when Docker CPU profiling is requested

The repository tracks both image variants through
`scripts/build_nockchain_bench_image.sh`:

- `--variant standard` builds the standard image with `nockchain-bench`
- `--variant profiling` builds the profiling image, which adds `samply`

Minimal Dockerfile:

```dockerfile
FROM ubuntu:24.04
COPY target/release/nockchain-bench /usr/local/bin/nockchain-bench
ENTRYPOINT ["/usr/local/bin/nockchain-bench"]
```

If Docker CPU profiling is requested, use the tracked profiling variant so the
image adds `samply`; the container runtime must still permit perf sampling.

### 11.6 Work Directory Modes

Explicit and benchmark-relevant:
- `HostBind`
- `DockerVolume`
- `DockerTmpfs`

No silent default in trusted Docker mode.

## 12. Validation Gate

Trusted Docker runs require validation, but validation is a layer on top of the
Docker backend rather than part of the core orchestrator.

### 12.1 Required Checks

1. container starts successfully
2. Docker reports cgroup v2 for the runtime environment
3. realized `memory.max` matches requested limit
4. optional CPU controller files are recorded when exposed by the container
   cgroup; otherwise the corresponding provenance fields are `null`
5. a known allocation changes `memory.current` as expected (+/- 20%)
6. required cgroup v2 memory files are readable inside the runtime environment

### 12.2 Validation Mechanism

Preferred implementation:
- `nockchain-bench sol validate-probe` runs inside the container

### 12.3 OOM Policy

OOM testing is not a mandatory gate. Optional diagnostic mode only.

### 12.4 Validation Cache

Validation may be cached per tuple of:
- Docker engine version
- cgroup version (`2`)
- image digest
- memory limit
- cpuset
- cpu quota/period
- work dir mode
- validation probe version

### 12.5 Abort Behavior

If validation fails, the run aborts immediately with a clear error. Partial
`validation.json` is preserved.

## 13. Measurement Sources

Trust hierarchy, most trusted first:
1. `SolBenchResults`
2. `MemoryProfile`
3. cgroup v2 snapshots from inside the container
4. time-series container samples via Docker stats API

Never use as primary evidence:
- `/usr/bin/time docker ...`
- Docker client memory
- human summaries without machine artifacts

## 14. Summary Rules

`summary.json` must include raw values and dispersion metrics:
- `median`
- `min`
- `max`
- `mad`
- `stddev`
- `cv`
- `values`

Minimum metrics to summarize:
- throughput
- init time
- total replay time
- average block time
- failed pokes
- checkpoint count
- average checkpoint time
- peak process RSS
- peak container memory in Docker mode
- major/minor fault totals where available

Defaults:
- measured runs default to 5
- minimum measured runs is 3

If throughput `cv` exceeds the configured threshold (default 0.10), verdict is
`Partial`.

## 15. Sweep Semantics

### 15.1 Matrix Schema

The matrix always uses `axes` as a map of axis name to value list.

Without `--allow-multi-axis`, a matrix with more than one axis is an error.

### 15.2 Default Sweep Policy

Trusted sweeps are single-axis by default.

### 15.3 Scheduling

Allowed modes:
- sequential
- `--interleave`
- `--randomize-order`

Not allowed by default:
- concurrent measured execution

### 15.4 Cooldown

`cooldown_secs` applies between all runs.

### 15.5 Invariants

Across a trusted comparison, all non-axis fields must remain constant,
including:
- fixture SHA256 and manifest
- git commit and dirty state
- build profile
- execution mode
- image digest
- work dir mode (the `work_dir_mode` axis also suppresses the derived
  `pma_work_dir_mode` provenance invariant)
- additive PMA provenance identity (`runtime_flavor`, `boot_source`,
  `pma_work_dir_mode`)
- thread count
- CPU control policy
- host identity unless explicitly overridden
- host/container binary identity policy

## 16. CLI Surface

### 16.1 Keep

- `sample`
- `sol quick-bench`
- `sol extract`
- `sol inspect`
- `sol fixture inspect`

### 16.2 Add

- `sol bench`
- `sol sweep`
- `sol validate`
- `sol run-once`
- `sol validate-probe`

### 16.3 `sol quick-bench` Positioning

- `sol quick-bench` is for ad hoc single runs and inner-loop debugging only
- `sol quick-bench` is not reproducible benchmark evidence
- `sol bench` is for trustworthy measured runs
- `sol sweep` orchestrates over `sol bench`

### 16.4 Trusted Docker PMA Operator Notes

- trusted Docker PMA replay uses the same `sol bench` surface as standard
  trusted Docker replay
- choose exactly one of `--docker-build-tag` or `--docker-image`
- PMA Docker replay must preserve the invoking PMA-featured binary into the
  container image
- the output directory must already exist and be empty before the run
- trusted Docker runs still require host/container identity and version-skew
  checks unless explicitly overridden
- checkpoint production remains unsupported under PMA replay

## 17. Build and Release Policy

Trusted mode enforces release builds.

- record build profile in `resolved_case.json` and `provenance.json`
- refuse debug binaries unless `--allow-debug-benchmark` is set
- if overridden, include the reason in verdict

## 18. Failure Policy

Do not discard partial evidence.

If a run fails:
- preserve all artifacts collected so far
- emit `verdict.json` with the failure reason
- do not silently retry under another backend

If one of N measured repetitions fails:
- preserve its artifacts
- exclude it from summary statistics
- emit `Partial`

If backend setup fails:
- preserve any raw evidence already captured
- write a structured failure artifact when possible

## 19. Implementation Phases

### Phase 0: Delete Mining And Removed Harness

1. delete mining-specific subsystems and CLI surfaces
2. clean Cargo dependencies
3. salvage generic Docker helpers into the new harness area
4. preserve only SOL-focused commands

### Phase 1: Shared Once-Run Core + Native Trusted Runner

Phase 1 implements the once-run engine and the full orchestration loop, but the
orchestration may remain inline in `native.rs` because only one backend exists.

1. create the `speed_of_light::harness` module tree
2. define the case, provenance, summary, and verdict models
3. extract shared once-run execution into a library function
4. implement `sol bench` native mode with repetition loop and cooldown
5. refactor `sol quick-bench` to call the shared once-run engine
6. write the native trusted artifact tree
7. compute `summary.json`
8. compute `verdict.json`
9. enforce release-build policy

Exit criteria:
- native `sol bench` produces a complete valid artifact tree
- `sol quick-bench` still works as the quick path
- summary statistics are correct for 3+ measured runs
- the orchestration logic is structured so it can be extracted into
  `orchestrate.rs` at the Phase 2 boundary without rewriting the native
  backend

### Phase 2: Docker Backend On The Shared Orchestrator

Phase 2 begins with a refactoring gate before any new Docker code:

0. extract the orchestration loop from `native.rs` into `orchestrate.rs`
1. define the backend contract
2. convert native to a backend adapter using that contract
3. verify native artifacts remain semantically equivalent after refactoring

Then:

4. implement SOL-specific Docker backend logic in `harness::docker`
5. implement hidden `sol run-once`
6. add Docker execution to `ExecutionRequest`
7. add host/container provenance capture
8. add concurrent Docker stats API polling to `container_samples.ndjson`
9. add host/container version skew check
10. support explicit work dir modes
11. capture `raw/docker_inspect.json`, `raw/docker_info.json`, and
    `raw/container_env.json`

Exit criteria:
- Docker `sol bench` executes replay inside the container via `sol run-once`
- Docker and native trusted runs both use the same shared orchestrator contract
- full artifact tree is emitted with both process-level and container-level
  evidence
- version skew between host and container binary is detected
- native behavior and artifact semantics are preserved across the refactor

### Phase 3: Validation Gate

1. implement `sol validate`
2. implement `sol validate-probe`
3. implement memory-limit verification and allocation sanity probe
4. add validation caching by resource tuple
5. wire validation into Docker `sol bench`

### Phase 4: Sweep Rewrite

1. implement `axes` matrix schema and cartesian expansion
2. implement `sol sweep` as orchestration over `sol bench`
3. implement single-axis trusted sweep with invariant checking
4. add `--allow-multi-axis`, `--interleave`, and `--randomize-order`
5. generate `comparison.json` and optional `comparison.md`
6. generate sweep-level `verdict.json`

### Phase 5: Documentation And Follow-Through

1. document trusted benchmark protocol
2. document `sol quick-bench` vs `sol bench`
3. document `--blocks` prefix-replay semantics
4. document host/container version policy

## 20. Acceptance Criteria

The redesign is acceptable when all of these hold:

1. `MiningScenario` and related subsystems are gone.
2. No trusted SOL path depends on mining-era abstractions.
3. A trusted Docker run records both host and container binary identity.
4. A trusted Docker run proves whether the requested memory limit was realized.
5. A trusted comparison can be traced back to raw per-run artifacts.
6. `sol bench` native and Docker modes share one machine-oriented once-run
   execution contract.
7. `sol bench` native and Docker modes share one trusted orchestration
   contract via a backend adapter boundary.
8. `sol quick-bench` remains available as the quick path but is not the source
   of truth for trusted orchestration.
9. `--blocks N` is explicitly documented as prefix replay of the fixture
   window.
10. Sweeps use `axes` map schema and no longer rely on mining-era case naming.
11. `cargo build -p nockchain-bench --release` and
    `cargo test -p nockchain-bench --release` pass after each phase boundary.
