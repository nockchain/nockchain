# bridge-dev Scenario Tests

The ignored scenarios in `scenarios.rs` exercise bridge-dev against a fresh Tenderly VNET and
release-built bridge binaries. They are intentionally opt-in because each test provisions remote
state and can take several minutes.

Run them from the workspace root with credentials in the process environment:

```sh
cargo build --release -p bridge -p nockchain-bridge-sequencer -p nockchain-wallet
BRIDGE_DEV_RUN_E2E=1 \
TENDERLY_ACCESS_KEY=... \
TENDERLY_ACCOUNT_ID=... \
TENDERLY_PROJECT_SLUG=... \
TENDERLY_TEST_PRIVATE_KEY=... \
cargo test -p bridge-dev --test scenarios -- --ignored --test-threads=1
```

The test harness sets `BRIDGE_DEV_TEST_RUN_ROOT` and `BRIDGE_DEV_PORT_OFFSET` for every command, so
it does not use the normal `open/crates/bridge/test_run_data` state or default ports. The generated
VNET env file is written under the test run root. Set `BRIDGE_DEV_E2E_PORT_OFFSET` if you need a
specific offset for your machine.

By default the scenarios also use the faster fakenet genesis and difficulty override
(`BRIDGE_DEV_FAKENET_GENESIS_JAM`, `BRIDGE_DEV_FAKENET_POW_LEN`,
`BRIDGE_DEV_FAKENET_LOG_DIFFICULTY`), a larger Base observer catch-up chunk
(`BRIDGE_DEV_BASE_BLOCKS_CHUNK`), and a shorter Nock observer poll interval
(`BRIDGE_NOCK_OBSERVER_POLL_MILLIS`). They also set
`BRIDGE_DEV_BRIDGE_SAVE_INTERVAL_MILLIS=1000` so restart scenarios do not wait on the normal
two-minute checkpoint cadence. Set those variables yourself before invoking the tests to exercise
the normal bridge-dev parameters.

Covered scenarios:

- Fresh VNET boot reaches `status --bridges --sequencer`.
- Deposit happy path waits for enough matured fakenet coinbase notes, then reaches submitted and
  successful observations on all bridge nodes.
- Deposit recovery while all bridge processes are down submits the Base event during downtime, then
  restarts every bridge and verifies the deposit is replayed to success on every node.
- All-bridge process restart after a successful deposit verifies the same successful deposit is
  still visible after every bridge process restarts.
- Multiple deposits verifies nonce ordering and all-node visibility across successive deposits.
- Withdrawal happy path first seeds bridge-owned Nockchain liquidity with a deposit observed by all
  bridge nodes, then mints, burns, advances enough Base blocks to fill the observer chunk, and
  verifies one withdrawal reaches pending, ready, submitted, and executed phases with stable
  proposal and authorized transaction artifacts.
- Withdrawal restart recovery waits until the withdrawal is ready, restarts all bridge processes,
  then verifies the same withdrawal reaches submitted and executed with stable artifacts.
- Withdrawal sequencer R2 recovery enables the durable sequencer journal, waits until a withdrawal
  is ready, stops the colocated sequencer/node process, deletes only the sequencer SQLite cache,
  restarts the process, and verifies the same withdrawal submits and executes from journal replay.
- Withdrawal bridge downtime recovery stops every bridge process while the Base burn is submitted,
  restarts them, then verifies the withdrawal is replayed and reaches executed with stable
  artifacts.
- Two-node degraded withdrawal stops two bridge processes, verifies the remaining quorum executes
  the withdrawal, then restarts the stopped processes and checks the cluster returns to idle.
- Two-node degraded deposit confirms the target bridge processes stop, the remaining nodes complete
  the deposit, and restarted nodes catch up to the same successful deposit.

The R2-backed sequencer recovery scenario also requires:

```sh
BRIDGE_R2_RUN_E2E=1
BRIDGE_R2_TEST_URL=https://<account>.r2.cloudflarestorage.com/<bucket>
BRIDGE_R2_TEST_TOKEN=... # or BRIDGE_R2_TEST_ACCESS_KEY_ID/BRIDGE_R2_TEST_SECRET_ACCESS_KEY
```
