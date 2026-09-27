# Peer v3 loopback rehearsal

`scripts/peer-v3-loopback-rehearsal.py` runs two real fakenet nodes and a
separate `zk-pow-mine` process. It checks initial synchronization, authenticated
block gossip, and catch-up after restarting the receiving node with its
existing identity and state. Both nodes must expose the same proof-bearing
chain head, and the receiving node must report generation 3 peer traffic.
The gossip check also requires a new `libp2p/gossip` event in its local event
log. This run submits no transactions, so those events establish receipt of
block gossip during the connected stage. Head agreement is checked
independently; the event count does not identify the particular gossiped block.

This is a short run near genesis. It does **not** test consensus-rule activation
at height 154500, wallet transaction propagation, mixed-generation peers, or a
production rollout. Historical corpus cases separately check representation
preservation around real activation boundaries.

## Prerequisites

- Python 3, `grpcurl`, and `lsof` (macOS or Linux).
- `nockchain` and `zk-pow-mine` built from the current branch.
- Fresh `assets/dumb.jam` and `assets/miner.jam` compiled from the current Hoon
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

After generating both current assets:

```sh
KERNEL_JAM_PATH="$PWD/assets/dumb.jam" \
  cargo build --locked -p nockchain --bin nockchain
KERNEL_JAM_PATH="$PWD/assets/miner.jam" \
  cargo build --locked -p zk-pow-miner --bin zk-pow-mine
python3 scripts/peer-v3-loopback-rehearsal.py \
  --node-bin target/debug/nockchain \
  --miner-bin target/debug/zk-pow-mine
```

Use the corresponding paths when setting `CARGO_TARGET_DIR`. Pass
`--stage-timeout` to change the default three-minute bound per mining/sync
stage. `--work-dir` must be a new directory; otherwise the script creates a
temporary directory and prints its location.

Build the two binaries separately: their kernel crates use the same
`KERNEL_JAM_PATH` variable for different assets. Explicit paths also avoid a
shared Cargo target reusing a build-script path from another checkout. Before
starting either node, the rehearsal verifies that each executable contains
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
node/miner/proxy settings. `TRACY_DISABLE` prevents the default profiler's TCP
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
head, peer traffic, gossip-event count, identity preservation, binary hashes,
the Git revision, the Hoon source-tree digest and kernel-asset hashes present
at run time. These hashes document the inputs; rebuild the binaries after
changing either kernel asset.

An unsuccessful run reports its failed stage and retains diagnostics. A Python
syntax check or successful binary build is not a passing rehearsal.
