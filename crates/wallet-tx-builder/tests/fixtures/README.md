This directory stores checked-in fixtures used by `wallet-tx-builder` tests.

Compatibility note
- If `tx_engine` noun formats change without a version bump, regenerate `note_data_fixtures.jam`.
- Otherwise note-data decoding and word-count tests can drift from the real chain encoding.

Fixture map
- `open/crates/wallet-tx-builder/tests/fixtures/note_data_fixtures.jam`
  - generator script: `closed/hoon/scripts/fixtures/v1/generate-note-data-fixtures.hoon`
  - payload source: `closed/hoon/tests/wallet/mod/note-data-fixtures.hoon`
  - regeneration command:
```bash
make open/crates/wallet-tx-builder/tests/fixtures/note_data_fixtures.jam
```
- `open/crates/wallet-tx-builder/tests/fixtures/withdrawal_tx_fixtures.jam`
  - generator script: `closed/hoon/scripts/fixtures/v1/generate-withdrawal-tx-fixtures.hoon`
  - payload source: `closed/hoon/tests/wallet/mod/withdrawal-tx-fixtures.hoon`
  - regeneration command:
```bash
make open/crates/wallet-tx-builder/tests/fixtures/withdrawal_tx_fixtures.jam
```
