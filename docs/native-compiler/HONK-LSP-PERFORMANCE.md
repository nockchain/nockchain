# Honk LSP performance harness

The LSP benchmark measures the editor paths separately from artifact-producing
Honk builds. It uses the real compiler actor, source-overlay invalidation,
semantic snapshot cache, and in-memory LSP protocol adapter. It never changes
compiler semantics and does not write source files outside temporary fixtures.

Run the standard baseline with:

```sh
just honk-lsp-performance
```

Run a short correctness smoke test with:

```sh
just honk-lsp-performance-smoke
```

The standard run uses three warmups and twenty measured samples, plus 256
invalidating root edits for the sustained-memory scenario. Pass custom settings
directly when more samples are needed:

```sh
cargo bench -p honk-lsp --bench lsp_performance -- \
  --samples 200 --warmups 5 --sustained-checks 512
```

## Scenarios

- `compiler_epoch_startup`: construction of a fresh compiler epoch with open
  editor documents already installed.
- `startup_to_first_check`: epoch construction plus the first artifact-free
  check.
- `first_check`: the first check after epoch construction.
- `unchanged_check`: repeated detached root-check cache hits.
- `same_content_update_check`: a new editor version with identical content,
  including overlay reconciliation and the resulting check.
- `root_edit_check`: alternating valid root contents while dependencies remain
  unchanged.
- `dependency_edit_check`: alternating a transitive dependency while preserving
  an unrelated cached dependency.
- `semantic_changed_snapshot`: full parsing and side-table construction for a
  changed generated document.
- `semantic_cached_snapshot`, `semantic_cached_hover`, and
  `semantic_cached_completion`: noun-free editor cache-hit paths.
- `lsp_startup_to_first_diagnostic`: initialization, an unsaved malformed open,
  compiler startup, checking, and diagnostic publication through the LSP
  adapter.
- `lsp_*_with_background_miner_check`: actual hover and completion request
  latency while the configured Miner entry is scheduled on the compiler worker.
- `sustained_root_edit_check`: repeated invalidating checks, with current RSS
  recorded before and after the sequence and process peak RSS recorded at the
  end.

The LSP adapter scenarios use `Connection::memory`, so they include JSON value
construction, protocol dispatch, worker scheduling, and response publication,
but exclude operating-system process launch and stdio framing. Compiler work,
cache behavior, and worker concurrency are otherwise the shipping paths.

## Results and interpretation

Every invocation creates a new directory under
`target/honk-lsp-performance/<run-id>/` containing:

- `DEFINE.md`, which records the scenario contract and scope;
- `fingerprint.json`, which records the git state, host, toolchain, and run
  parameters;
- `results.json`, including every raw sample and cache/memory invariants; and
- `BASELINE.md`, a compact p50/p95/p99/max report.

Twenty samples are the minimum useful engineering baseline, not enough for a
precise p95 or p99 claim. The report marks p95 advisory below 200 samples and
p99 conservative below 2,000 samples. Compare absolute latency only between
runs with matching host/toolchain fingerprints. Treat same-host p95 drift up to
10% as noise, investigate drift above 10%, and escalate drift above 20% or
three consecutive regressions above 10%.

The harness has no universal hard latency gate yet. Establish the first quiet,
same-host baseline, then add CI comparison against ratios rather than copying
one workstation's absolute milliseconds to unrelated runners.
