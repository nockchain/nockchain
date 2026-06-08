Golden JAM fixtures for `nockchain-types`.

Status: Active  
Owner: Nockchain Maintainers  
Last Reviewed: 2026-02-20  
Canonical/Legacy: Legacy (crate-level test fixture reference; canonical docs spine starts at [`START_HERE.md`](../../../START_HERE.md))

## Layout

- `jams/v0/`: v0 fixtures (`balance.jam`, `early-balance.jam`, `note.jam`, `raw-tx.jam`, `timelock.jam`)
- `jams/v1/`: v1 fixtures (`note.jam`, `raw-tx.jam`, sig-hash oracles)

## Where These Fixtures Are Used

- `tests/balance_from_peek_v0.rs` and `tests/balance_from_peek_v1.rs` decode `jams/v0/early-balance.jam`, validate expected fields, and assert noun roundtrips.
- `tests/raw_tx_from_jam_v0.rs` and `tests/raw_tx_from_jam_v1.rs` decode `raw-tx.jam` / `note.jam` fixtures and assert structural invariants + noun roundtrips.
- `tests/spend1_signature_verification_v1.rs` decodes `raw-tx.jam` plus the `%1` sig-hash oracle fixture and checks Rust `Spend1::sig_hash_digest()` parity against the Hoon-generated oracle.
- `src/tx_engine/v0/note.rs` unit tests load `jams/v0/{balance,note,timelock}.jam`.

## Updating Fixtures

There is currently no in-tree `dump_balance_peek` example binary in this repo.  
To refresh coverage:

1. Capture new jam blobs with your external tooling/workflow.
2. Place files in the versioned `jams/v0` or `jams/v1` directory.
3. Update assertions in the matching tests when fixture structure changes.

These tests use fixed `include_bytes!` paths, so fixture files must exist at compile time.

## Compatibility note
- If you change `tx_engine` encoding/decoding or noun shape without a version bump, regenerate these fixtures.
- Otherwise tests may fail for the wrong reason, or pass against stale data that no longer matches current consensus encoding.

## Regeneration Flow
`hoon-closed` regeneration flow
- Run all commands from the repo root.
- `out.jam` is the compiled generator artifact, not the kicked generator result.
- To save the actual fixture payload, pass `--out-dir /tmp/nockchain-fixtures` and copy the emitted script-stem `.jam` from there.

## Fixture map
- `open/crates/nockchain-types/jams/v0/raw-tx.jam`
  - generator: `closed/hoon/scripts/fixtures/v0/generate-raw-tx.hoon`
  - command:
```bash
cargo run --profile release --bin hoon-closed -- \
  closed/hoon/scripts/fixtures/v0/generate-raw-tx.hoon \
  closed/hoon \
  --out-dir /tmp/nockchain-fixtures
cp /tmp/nockchain-fixtures/generate-raw-tx.jam open/crates/nockchain-types/jams/v0/raw-tx.jam
```
- `open/crates/nockchain-types/jams/v0/note.jam`
  - generator: `closed/hoon/scripts/fixtures/v0/generate-note.hoon`
  - command:
```bash
cargo run --profile release --bin hoon-closed -- \
  closed/hoon/scripts/fixtures/v0/generate-note.hoon \
  closed/hoon \
  --out-dir /tmp/nockchain-fixtures
cp /tmp/nockchain-fixtures/generate-note.jam open/crates/nockchain-types/jams/v0/note.jam
```
- `open/crates/nockchain-types/jams/v0/balance.jam`
  - generator: `closed/hoon/scripts/fixtures/v0/generate-balance.hoon`
  - command:
```bash
cargo run --profile release --bin hoon-closed -- \
  closed/hoon/scripts/fixtures/v0/generate-balance.hoon \
  closed/hoon \
  --out-dir /tmp/nockchain-fixtures
cp /tmp/nockchain-fixtures/generate-balance.jam open/crates/nockchain-types/jams/v0/balance.jam
```
- `open/crates/nockchain-types/jams/v0/timelock.jam`
  - generator: `closed/hoon/scripts/fixtures/v0/generate-timelock.hoon`
  - command:
```bash
cargo run --profile release --bin hoon-closed -- \
  closed/hoon/scripts/fixtures/v0/generate-timelock.hoon \
  closed/hoon \
  --out-dir /tmp/nockchain-fixtures
cp /tmp/nockchain-fixtures/generate-timelock.jam open/crates/nockchain-types/jams/v0/timelock.jam
```
- `open/crates/nockchain-types/jams/v1/raw-tx.jam`
  - generator: `closed/hoon/scripts/fixtures/v1/generate-v1-raw-tx.hoon`
  - command:
```bash
cargo run --profile release --bin hoon-closed -- \
  closed/hoon/scripts/fixtures/v1/generate-v1-raw-tx.hoon \
  closed/hoon \
  --out-dir /tmp/nockchain-fixtures
cp /tmp/nockchain-fixtures/generate-v1-raw-tx.jam open/crates/nockchain-types/jams/v1/raw-tx.jam
```
- `open/crates/nockchain-types/jams/v1/note.jam`
  - generator: `closed/hoon/scripts/fixtures/v1/generate-v1-note.hoon`
  - command:
```bash
cargo run --profile release --bin hoon-closed -- \
  closed/hoon/scripts/fixtures/v1/generate-v1-note.hoon \
  closed/hoon \
  --out-dir /tmp/nockchain-fixtures
cp /tmp/nockchain-fixtures/generate-v1-note.jam open/crates/nockchain-types/jams/v1/note.jam
```
- `open/crates/nockchain-types/jams/v1/raw-tx-word-count-oracle.jam`
  - generator: `closed/hoon/scripts/fixtures/v1/generate-v1-raw-tx-word-count-golden.hoon`
  - command:
```bash
cargo run --profile release --bin hoon-closed -- \
  closed/hoon/scripts/fixtures/v1/generate-v1-raw-tx-word-count-golden.hoon \
  closed/hoon \
  --out-dir /tmp/nockchain-fixtures
cp /tmp/nockchain-fixtures/generate-v1-raw-tx-word-count-golden.jam open/crates/nockchain-types/jams/v1/raw-tx-word-count-oracle.jam
```
- `open/crates/nockchain-types/jams/v1/raw-tx-spend1-sig-hash-oracle.jam`
  - generator: `closed/hoon/scripts/fixtures/v1/generate-v1-raw-tx-spend1-sig-hash-oracle.hoon`
  - command:
```bash
cargo run --profile release --bin hoon-closed -- \
  closed/hoon/scripts/fixtures/v1/generate-v1-raw-tx-spend1-sig-hash-oracle.hoon \
  closed/hoon \
  --out-dir /tmp/nockchain-fixtures
cp /tmp/nockchain-fixtures/generate-v1-raw-tx-spend1-sig-hash-oracle.jam open/crates/nockchain-types/jams/v1/raw-tx-spend1-sig-hash-oracle.jam
```

## Captured fixture (no direct Hoon script)
- `open/crates/nockchain-types/jams/v0/early-balance.jam`
  - source: captured `%balance-by-pubkey` peek payload.
  - capture command:
```bash
cargo run -p nockapp-grpc --example dump_balance_peek -- \
  http://127.0.0.1:50051 \
  <PUBKEY_B58> \
  /tmp
```
