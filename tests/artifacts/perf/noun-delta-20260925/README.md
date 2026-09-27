# Read-only noun snapshot delta experiment

Standalone benchmark source and measurement artifacts from `nockcloud-control`, 2026-09-25. This is an experimental representation and benchmark, not a Nockchain runtime patch or production restore tool. Input PMAs must remain immutable for the entire run.

Read `REPORT.md` for the completed measurements. Raw results and build identities are under `artifacts/`. The original 256-bit baseline source is preserved in `artifacts/baseline-source/`; current source adds incremental chain caches and explicitly selectable hash variants.

Read [HANDOFF.md](HANDOFF.md) to continue on another computer, including the latest design recommendations, missing runtime/restore work and historical server backup status.

## Build and verify

This package has its own Cargo workspace and lockfile. The measured Linux toolchain/build configuration is recorded in `artifacts/linux-toolchain.txt` and `Cargo.toml`.

```sh
cargo test --locked
cargo test --locked --features one-shot-cell
cargo test --locked --features compact-hash
cargo test --locked --all-features
RUSTFLAGS='-C force-frame-pointers=yes' cargo build --release --locked --bins --all-features
```

`one-shot-cell` preserves the selected fingerprint algorithm exactly. `compact-hash` changes the fingerprint width and cell hash inputs, uses separate artifact magic, and is an explicit 128-bit experiment. Default fingerprints are 256 bits.

## Commands

```text
nockchain-noun-delta-bench inspect PMA MANIFEST_OR_META
nockchain-noun-delta-bench pilot PMA MANIFEST_OR_META MAX_UNIQUE_NODES
nockchain-noun-delta-bench bench A_PMA A_DESC B_PMA B_DESC OUTPUT_DIR [REPEATS]
sequence A_PMA A_DESC B_PMA B_DESC C_PMA C_DESC OUTPUT_DIR [REPEATS]
page_delta A_PMA A_DESC B_PMA B_DESC [CHUNK_BYTES ...]
hashcons A_PMA A_DESC B_PMA B_DESC OUTPUT_DIR [MAX_UNIQUE_PHYSICAL_NODES]
```

A pilot deliberately returns an error when its node budget is exhausted. A completed whole-state benchmark ends with a `complete` JSONL record. Warm iterations have a 60-second aggregate budget and report their actual count. New output paths are required; existing delta files are never overwritten.

`sequence` builds A/B indexes in parallel, writes and rereads A→B, mounts B virtually, derives its index and lookup from the epoch cache, then indexes C and measures C against reconstructed B. It writes B→C, reconstructs C through both deltas, and performs a complete independent atom/cell comparison. Prepared warm timings exclude full current-state indexing.

`page_delta` is a byte-layout control. It counts potential payload and checks a virtual prefix; it does not write a standalone payload. Its complete byte checksum and bounded noun-root smoke check are distinct from the full semantic comparison in the other commands.

No sidecar loader or NockVM integration is implemented. `write_to` and `write_incremental_to` index sizes are verified on fixtures, but large persisted indexes were not written or restored in the server benchmark. See `artifacts/runtime-integration-notes.md`.

## Server operation

All benchmark processes ran in a private network namespace with read-only input mounts, no access to the production directory, CPU/memory limits, and a 15-minute runtime ceiling. The leader continued using its existing production image. Shell scripts in this directory record the exact session-specific paths and systemd settings; adapt them deliberately before reuse.

The independent stopped data copy is at `/opt/nockcloud/backups/pma-storage-efficiency-20260925T1635Z/data`; it is separate from experiment outputs at `/opt/nockcloud/backups/noun-delta-bench-20260925`. A separate low-priority read-only SQLite integrity job finished successfully (`ok`) at 2026-09-25 18:39:53 UTC; its result is in `artifacts/backup/`. The benchmark never reads the event log.

The `hashcons` alternative interns normalized atom values and exact ordered canonical child pairs across A then B. It keeps portable 256-bit fingerprints, hashes cells only when a new canonical value is created, and releases each source traversal's raw-pointer memo. Its delta is actually written/reread and its reconstruction contains only the base dictionary and decoded new payloads. Both full physical traversals are explicit costs; this is separate from the prepared Merkle timings.
