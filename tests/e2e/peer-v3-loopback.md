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

- Python 3 and `grpcurl`.
- `nockchain` and `zk-pow-mine` built from the current branch.
- Fresh `assets/dumb.jam` and `assets/miner.jam` compiled from the current Hoon
  sources before building those binaries. Do not silently reuse older compiled
  kernels. Use a fresh `hoonc --new --data-dir` directory for each asset; the
  Makefile's kernel asset targets describe the source entry points.
- Enough time and memory for mandatory AI verifier setup on first boot, even
  though this run mines ZK blocks. The default boot timeout is one hour.
  `--setup-cache` accepts an existing proof-independent `ai-pow` cache directory;
  it is copied into fresh node state and validated by node startup.

The older YAML scenarios still expect in-process mining flags. This rehearsal
uses the current separate-miner interface.

After generating both current assets:

```sh
cargo build -p nockchain --bin nockchain -p zk-pow-miner --bin zk-pow-mine
python3 scripts/peer-v3-loopback-rehearsal.py \
  --node-bin target/debug/nockchain \
  --miner-bin target/debug/zk-pow-mine
```

Use the corresponding paths when setting `CARGO_TARGET_DIR`. Pass
`--stage-timeout` to change the default three-minute bound per mining/sync
stage. `--work-dir` must be a new directory; otherwise the script creates a
temporary directory and prints its location.

## Isolation and evidence

Both nodes start with fresh consensus state, explicit `127.0.0.1` QUIC and
gRPC listeners, and `--no-default-peers`. The receiving node's only configured
peer is the other loopback node. No production snapshots or node identities
are used. Discovery can learn only from this isolated peer set; the node has
no separate switch for disabling Kademlia. Child environments omit inherited
node/miner/proxy settings and disable telemetry.

All owned child processes are stopped when the script finishes or fails. Logs
and `report.json` remain in the run directory. The report records each agreed
head, peer traffic, gossip-event count, identity preservation, binary hashes,
the Git revision, the Hoon source-tree digest and kernel-asset hashes present
at run time. These hashes document the inputs; rebuild the binaries after
changing either kernel asset.

An unsuccessful run reports its failed stage and retains diagnostics. A Python
syntax check or successful binary build is not a passing rehearsal.
