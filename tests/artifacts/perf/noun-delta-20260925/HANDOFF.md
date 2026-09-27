# PMA storage work handoff

Branch: `codex/pma-storage-efficiency`. Draft PR: <https://github.com/nockchain/nockchain/pull/207>.

The runtime changes through `639240265d1298d4aef36b350772faa994173058` preserve sparse snapshot copies and compact SQLite event history behind the older retained rotating snapshot. The compaction threshold uses accepted-event compute time and must strictly exceed snapshot rotation's threshold. Epoch advancement copies a verified existing snapshot; it does not replay pokes to construct an arbitrary epoch. See the PR description for completed runtime tests and remaining production-copy validation.

This directory contains the separate noun-delta benchmark, tests and recorded measurements. It is an independent Cargo workspace, with no runtime integration. Start with [REPORT.md](REPORT.md) and [README.md](README.md). Historical server status and measurements are from September 25, 2026; they are not a current health check.

## Continue locally

From the repository root:

```sh
cargo +nightly-2026-04-03 test --locked --manifest-path tests/artifacts/perf/noun-delta-20260925/Cargo.toml
cargo +nightly-2026-04-03 test --locked --all-features --manifest-path tests/artifacts/perf/noun-delta-20260925/Cargo.toml
```

Synthetic tests need no node data or server access. PMAs, SQLite files, generated deltas and compiled binaries are not in Git. `build-*.sh` records the original container setup (`/source`, `/build-target`, `/usr/local/cargo`). `run-*.sh` records the privileged Linux/systemd setup and session-specific paths; adapt those deliberately before running elsewhere. `verify-backup.py` also needs the original server inventory and read-only backup mount.

## Findings and proposed next work

- Prepared Merkle comparison against an epoch-plus-delta reconstruction took 243 ms median and produced a 35.94 MiB next delta. Preparing the current PMA index first took 210 seconds. This chain experiment used 128-bit fingerprints.
- Exact interning with portable 256-bit fingerprints took 164 seconds for the base and 160 seconds for the target, producing a 79.07 MiB delta. Peak RSS through full verification was 69.23 GiB. Host file caches were warm; whole-run comparisons do not isolate algorithm speedups.
- Reconstructed-state index extension is already incremental: 57 ms for fingerprints and 28 ms for lookup entries, without reading base nouns. The 4.50 MiB incremental fingerprint sidecar is a calculated format size; its production loader is unimplemented.
- Live PMA metadata maintenance remains unimplemented. A possible later design caches `(PMA generation, offset) -> canonical ID`, indexes lazily from a published root, carries existing associations through GC, and appends new durable dictionary/index records. It needs generation, lifetime, publication and retention rules before deployment.

The current recommendation is to finish validation of the existing-format storage PR, then evaluate a separate optional snapshot codec. That recommendation is not an implemented change or an instruction to start a production test.

A conservative codec experiment would first capture the existing full snapshot, asynchronously encode each target directly against one pinned full epoch, and retain every full snapshot during evaluation. Recovery would materialize an ordinary PMA. Semantic reconstruction needs a PMA writer, correct footer/root offsets, a new verified manifest preserving event/kernel identity, actual isolated boot/recovery tests, and crash-safe publication with dependency-aware retention. The benchmark currently proves noun-value reconstruction, not bootable PMA restoration. It reads saved kernel state at Arvo axis 6, not the full live VM subject. Avoid treating hashes or the virtual graph as a completed restore implementation.

Byte-exact compression would preserve the original manifest and has a smaller correctness surface, but ordinary lossless compression has not been measured. The fixed-block delta control retained more than 99% of target size. Runtime IDs and GC-aware metadata can remain separate from the first codec experiment.

## Server data and stopped work

On `nockcloud-control`, the independent stopped-copy backup is:

```text
/opt/nockcloud/backups/pma-storage-efficiency-20260925T1635Z/data
```

All 40 original files were copied independently. Full read-only SQLite integrity verification returned `ok`; evidence is in `artifacts/backup/`. The original production service was restarted on the same image after copying. The raw candidate was subsequently halted at the user's request during its initial SQLite integrity scan, before compaction. It has no completed large-data compaction/restart result. At the last September 25 check, the candidate and benchmark processes were stopped and production health was `SERVING`.

Benchmark outputs remain separately under:

```text
/opt/nockcloud/backups/noun-delta-bench-20260925
```

Preserve the fallback data copy for later PR testing. Use a separate working copy for any mutating test, and recheck server state before continuing. No server process was restarted or test resumed for this Git handoff.
