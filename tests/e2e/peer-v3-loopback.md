# Peer v3 loopback rehearsal

`scripts/peer-v3-loopback-rehearsal.py` runs two real fakenet nodes and a
separate `zk-pow-mine` process. It checks initial synchronization, authenticated
block gossip, v1 transaction propagation and inclusion, and catch-up after restarting the receiving node with its
existing identity and state. Both nodes must expose the same proof-bearing
chain head, and the receiving node must report generation 3 peer traffic.
Head snapshots come directly from the kernel's private Peek through the
`peer_v3_head` Rust example. It queries `%heaviest-chain`, falling back to
`%heaviest-block` when needed. The public block-details RPC separately
checks the agreed height, block ID, and proof presence. After restart catch-up,
the script captures both kernel heads again and requires the same agreement.
The block-gossip check also requires a new `libp2p/gossip` event in its local
event log. No transactions have been submitted at that stage, so those events
establish receipt of block gossip. Head agreement is checked
independently; the event count does not identify the particular gossiped block.

This is a short run near genesis. It does **not** test consensus-rule activation
at height 154500, live legacy v0 transaction acceptance, mixed-generation peers, or a
production rollout. Historical corpus cases separately check representation
preservation around real activation boundaries.

## Transaction evidence

The run creates fresh miner and recipient wallets, mines to the miner wallet,
and makes a 1,000-nick payment with normal wallet note selection and fees.
The wallet and nodes use matching fakenet phase settings. The miner is stopped
throughout admission and gossip verification; the wallet submits only to A.
The `peer_v3_tx` helper reconstructs the signed raw transaction from the saved
wallet `.tx` file, including its witness data. It recomputes the submitted ID,
which can differ from the wallet filename, and the exact event cause hash.

| Check | Required evidence |
| --- | --- |
| Fresh transaction | Absent from both nodes' acceptance and pending queries before submission |
| Kernel admission | `TransactionAccepted` true on both nodes |
| Pending propagation | Exact ID in both private `%excluded-txs` sets while both consensus heads remain unchanged |
| Authenticated gossip | New receiver event with the exact signed-transaction cause hash and configured sender's gossip wire tags |
| v3 peer traffic | Receiver reports generation 3 and received bytes |
| Inclusion | After mining resumes, direct block details on both nodes contain that ID in the same proof-bearing block |
| Pending removal | Included ID absent from both `%excluded-txs` sets |

An RPC acknowledgment or generic gossip count is insufficient. The event
wire version is 1; it is distinct from peer protocol generation 3. The
transaction stages record their signed-artifact hash, cause hash, transaction
ID, matching event number, and inclusion block in `report.json`.

## Prerequisites

- Python 3, `grpcurl`, and `lsof` (macOS or Linux).
- `nockchain`, `zk-pow-mine`, `nockchain-wallet`, and the `peer_v3_head` and
  `peer_v3_tx` examples built from the current branch.
- Fresh `assets/dumb.jam`, `assets/miner.jam`, and `assets/wal.jam` compiled from the current Hoon
  sources before building those binaries. Do not silently reuse older compiled
  kernels. Use a fresh `hoonc --new --data-dir` directory for each asset; the
  Makefile's kernel asset targets describe the source entry points.
- Enough time and memory for mandatory AI verifier setup on first boot, even
  though this run mines ZK blocks. The default boot timeout is one hour.
  `--setup-cache` accepts an existing proof-independent `ai-pow` cache directory;
  only `verifier-setup-seeds-v2.bin` is imported into fresh node state. The file
  must exist before either node starts. Node startup validates the seed table
  and rebuilds its verifier contexts locally; foreign context files, checksum
  sidecars, and temporary files are not imported. The completed local cache is
  then reused by the second node on the same machine and binary.

The older YAML scenarios still expect in-process mining flags. This rehearsal
uses the current separate-miner interface.

After generating all three current assets:

```sh
KERNEL_JAM_PATH="$PWD/assets/dumb.jam" \
  cargo build --locked -p nockchain --bin nockchain
KERNEL_JAM_PATH="$PWD/assets/miner.jam" \
  cargo build --locked -p zk-pow-miner --bin zk-pow-mine
KERNEL_JAM_PATH="$PWD/assets/wal.jam" \
  cargo build --locked -p nockchain-wallet --bin nockchain-wallet
KERNEL_JAM_PATH="$PWD/assets/dumb.jam" \
  cargo build --locked -p nockchain-e2e --example peer_v3_head --example peer_v3_tx
python3 scripts/peer-v3-loopback-rehearsal.py \
  --node-bin target/debug/nockchain \
  --miner-bin target/debug/zk-pow-mine \
  --head-bin target/debug/examples/peer_v3_head \
  --wallet-bin target/debug/nockchain-wallet \
  --tx-bin target/debug/examples/peer_v3_tx
```

Use the corresponding paths when setting `CARGO_TARGET_DIR`. Pass
`--stage-timeout` to change the default three-minute bound per mining/sync
stage. `--work-dir` must be a new directory; otherwise the script creates a
temporary directory and prints its location.

Build the node, miner, and wallet separately: their kernel crates use the same
`KERNEL_JAM_PATH` variable for different assets. Explicit paths also avoid a
shared Cargo target reusing a build-script path from another checkout. Before
starting any wallet or node, the rehearsal verifies that each kernel-bearing executable contains
the complete current kernel asset, then records that check in its report.

An `ai-pow-jets` test executable built from the current branch can check a
candidate cache without regenerating seeds or rebuilding contexts. An older
binary may check a stale consensus digest. If needed,
`cargo test --locked -p ai-pow-jets --lib --no-run` builds it and prints its
path. For this check,
`AI_POW_SETUP_GENERATION_DIR` names the **parent** of the `ai-pow` directory;
`--setup-cache` instead names the `ai-pow` directory itself.

```sh
seed_root=/path/to/cache-parent
test -f "$seed_root/ai-pow/verifier-setup-seeds-v2.bin" &&
AI_POW_SETUP_GENERATION_DIR="$seed_root" \
  /path/to/ai_pow_jets-test-binary \
  --exact jet_tests::stable_cache_seeds_pass_lazy_boot_digest_check \
  --ignored --nocapture
```

Require one executed test, a passing result, and the explicit message
`stable cache seeds pass the lazy boot digest check`. An absent cache skips
the test successfully, and a wrong filter can run zero tests, so exit status
alone is insufficient. This check validates the cache format, complete
production shape set, and committed table digest. Normal node startup still
performs its own validation.

## Isolation and evidence

Both nodes start with fresh consensus state, explicit `127.0.0.1` QUIC and
gRPC listeners, and `--no-default-peers`. The receiving node's only configured
peer is the other loopback node. No production snapshots or node identities
are used. Discovery can learn only from this isolated peer set; the node has
no separate switch for disabling Kademlia. Child environments omit inherited
node/miner/wallet/proxy settings. Wallet commands use an explicit loopback
private client and a fresh `NOCKAPP_HOME` inside the run directory. Wallet
key-generation logs contain test private keys and seed phrases: keep these
artifacts local. Wallet directories are owner-only and process logs are
created with mode 0600. `TRACY_DISABLE` prevents the default profiler's TCP
listener and discovery broadcasts. Application metrics are disabled, and the
global Gnort registry's first emission is deferred beyond the run's bounded
stages and cleanup because that registry does not honor the disable setting.

On every wait poll, the script uses `lsof` with both an owned-PID filter and a
network-socket filter, verifies the returned PID, and checks all reported TCP
and UDP endpoints. A wildcard or non-loopback endpoint aborts the run and
triggers cleanup. This is a periodic check, not an operating-system network
sandbox. The report records the number of successful process socket checks.

All owned child processes are stopped when the script finishes or fails. Logs
and `report.json` remain in the run directory. The report records each agreed
head, final kernel-head snapshots, peer traffic, gossip-event count, identity
preservation, node/miner/wallet/helper binary hashes, the head-query source,
the Git revision, the Hoon source-tree digest and kernel-asset hashes present
at run time. These hashes document the inputs; rebuild the binaries after
changing either kernel asset.

An unsuccessful run reports its failed stage and retains diagnostics. A Python
syntax check or successful binary build is not a passing rehearsal.
