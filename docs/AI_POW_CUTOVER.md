# Coordinated upgrade at height 170,000

Deploy matching node, consensus kernel, Rust verifier jets, AI and ZK miners,
and wallet artifacts before parent height **169,999**. This release selects
updated native AI-PoW admission and resets both ASERT lanes at candidate height
**170,000**. Existing bridge withdrawal processing remains disabled; this
upgrade does not include a bridge withdrawal rollout.

## Activation and compatibility

The candidate block's height selects the height-gated consensus rules. Blocks
below 170,000 retain their historical proof rules and targets during
synchronization, replay, and backward reorgs. Blocks at or above 170,000 use the
new native AI-PoW policy. Installation does not activate these height-gated
rules early. The policy aliases the existing Hardened proof program and trusted
setup; there is no AIR, proof encoding, or miner wire-format migration at this
boundary. Historical proof-rule boundaries at 126,000, 147,500, and 154,500
remain intact.

Separately, realnet canonical-history pins apply **immediately on upgrade**,
independently of 170,000. Matching nodes must pin every height 126,000–154,499
inclusive, for all proof kinds, to the same nockscan-sourced block IDs, while
retaining existing sparse checkpoints. The existing incoming-block checkpoint
admission machinery enforces these pins. This changes neither Legacy circuits,
keys, nor proof rules. Source provenance, generator checks, and
the fixed checksum are recorded in the
[protocol specification](protocol/upgrades/017-anthropos.md#immediate-canonical-history-policy).

The generated checkpoint data consists of 29 bounded literal chunks: 28 of
1,000 rows and a final 500 rows. Inert literals disable source-debug wrappers.
The root concatenates the chunks once, decodes them with one shared typed
`from-b58` gate, and builds the map with one z-map `gas`; it does not emit
per-row decoder expressions or use `^~` to constant-fold the entire map.
The root is imported outside the state-parameterized consensus core, with no
per-block construction or chain-history startup walk.

Release `hoonc` registers the existing production `zkvm-jetpack` hot state for
base58, Tip5, and z-map ordering/hash evaluation. Exact dispatch is unchanged;
unmatched batteries fall back to evaluation. Build a kernel directly with:

```sh
make assets/dumb.jam
```

Standard Hoon targets use this checkout's `target/release/hoonc` by default.
Make runs Cargo's build/freshness check once per invocation before invoking
the default compiler, without installing it globally or clearing compiler
data/PMA caches. The normal `make build-hoon` path uses the same compiler.
An explicit override, such as `make assets/dumb.jam HOONC=/path/to/hoonc`,
uses that command/path as-is and skips the automatic local compiler build.

Release qualification must not inherit an application build cache. Freeze the
checkout inputs and compiler, disable exported prewarm, and compare two serial
builds with separate empty working/data/system directories:

```sh
set -eu
repo=$PWD
qualification=$(mktemp -d)
env -u KERNEL_JAM_PATH CARGO_TARGET_DIR="$repo/target" \
  cargo build --release --locked -j 1 --bin hoonc
for build in a b; do
  mkdir "$qualification/$build"
  (
    cd "$qualification/$build"
    env -u HOONC_NOCK_STACK_SIZE -u KERNEL_JAM_PATH \
      HOONC_DISABLE_PREWARM=1 NOCKAPP_HOME="$qualification/$build/system" \
      "$repo/target/release/hoonc" --new --color never --stack-size normal \
      --data-dir "$qualification/$build/data" --output dumb.jam \
      "$repo/hoon/apps/dumbnet/outer.hoon" "$repo/hoon"
  ) >"$qualification/$build/build.log" 2>&1
  test -s "$qualification/$build/dumb.jam"
  grep -aq 'hoonc: build succeeded, sending out write effect' "$qualification/$build/build.log"
  grep -aq 'hoonc: output written successfully to' "$qualification/$build/build.log"
done
cmp "$qualification/a/dumb.jam" "$qualification/b/dumb.jam"
shasum -a 256 "$qualification/a/dumb.jam" "$qualification/b/dumb.jam"
```

Retain both logs and record source/compiler/minimal-bootstrap/Hoon-text
identities. Require fresh compiler initialization and application/dependency
compilation before any same-invocation reuse; exit zero alone is insufficient.
The minimal compiler bootstrap remains trusted, not regenerated.
Install only the matching candidate with a fresh modification time, then build
native consumers with `KERNEL_JAM_PATH` unset: a global dumb-kernel override
would also affect the separate miner kernel.

Both ASERT lanes re-anchor once using branch-local median-time-past of
predecessor 169,999. The 170,000 ZK reference is 100 billion Tip5 hashes/s;
its AI reference doubles from 10 to 20 ExaMAC/s. The historical 154,500 ZK reference of
56 billion Tip5 hashes/s and historical accumulated work are preserved.
The exact targets, intervals, and work conversion are in the
[protocol specification](protocol/upgrades/017-anthropos.md#subsequent-reset-at-height-170000).
Pearl transaction inclusion and nonce limits remain specified in
[Logos](protocol/upgrades/016-logos.md#pearl-transaction-commitment).

Separately, updated transaction validation applies **immediately at every
spending height**. Fund-recovery maturity instead uses the shared configured
phase height (production **170,000**), selected by the spending candidate
height. Below activation, recovery retains its previous maturity behavior;
at or above activation, it additionally requires the configured coinbase
minimum age, inclusively `now >= origin + minimum` (production **100**).
The origin comes from the authenticated balance input, not the witness.
The production-target requirement and 3-of-4 signatures remain unchanged.
Moving back below activation restores the previous recovery age policy.
Block validation uses the block candidate height; wallet and mempool validation
use the next-candidate height. Configured constants are honored, and ordinary
note behavior is unchanged. Transaction IDs and signature preimages are
unchanged.

Coordinate all transaction- and block-validation nodes when deploying the
update. Stored accepted state is not automatically rewritten by installation.
Do not assume older software is rollback-compatible with updated validation
or accepted state.

## Before activation

1. Record the release revision and build all distributed binaries and kernels
   from that revision. Deploy matching kernel, Rust verifier, node, miners, and
   wallet artifacts; check the live tip and finish before parent height 169,999.
2. Start upgraded nodes early enough for verifier setup initialization to finish.
   Missing or incompatible setup data is regenerated automatically before
   networking starts; matching data is reused. Keep setup caches and historical
   verifier setups for replay. No manual cache deletion or preparation is
   required. Initialization can require substantial CPU time and working memory;
   monitor completion and the deployment's configured memory budget.
3. Upgrade and restart both miner types before parent height 169,999. Confirm
   that they receive fresh candidates with the matching node software. AI mining
   effects carry candidate height; historical miners that omit height are not
   compatible with the current node interface.
4. Keep `withdrawal_processing_enabled = false` in the rendered production bridge
   configuration. Existing signed withdrawals, if any, require separate
   reconciliation before any future withdrawal rollout.

## Private gRPC operator and miner access

Provision `NOCKCHAIN_PRIVATE_GRPC_AUTH_TOKEN` for operator access and
`NOCKCHAIN_PRIVATE_GRPC_MINER_TOKEN` for restricted mining access on the node.
Each configured token must contain at least 32 characters, and the two tokens
must be distinct. Keep the operator token outside miner environments; remote
miners receive only the miner token.

The `nockchain`, `nockchain-api`, and `nockchain-bridge-sequencer` executables
validate effective private API configuration before consensus runtime
preparation, kernel boot, state writes, or peer identity creation. The sequencer
also validates before loading sequencer configuration or starting external
services. Missing operator credentials without an explicit opt-out, short or
equal role tokens, miner-only credentials, and non-loopback binding are startup
errors, not background driver failures. The validated immutable snapshot is
reused by the listener; environment changes cannot alter policy after
initialization.

The operator credential permits Peek, WatchEffects, and arbitrary Poke. The
restricted miner credential permits only the matching mining-candidate
WatchEffects filter and matching AI/ZK mined-proof Poke. It does not permit
Peek, other effect subscriptions, arbitrary configuration, payout changes, or
mining enable/disable operations. Restricted miners leave configuration and
payees under operator control, including on reconnect and shutdown; stopping
one worker does not disable the node's mining for other workers.

Start restricted miners and let them wait for a candidate. On the operator
host, configure mining with the operator credential and an approved payout PKH:

```sh
NOCKCHAIN_PRIVATE_GRPC_AUTH_TOKEN="$OPERATOR_TOKEN" \
  cargo run -p nockchain-mining-common --bin configure_mining -- \
  http://127.0.0.1:5555 ai "1,$APPROVED_PKH"
```

Use `zk` instead of `ai` for a ZK miner. Confirm the operator-approved payees on
the node before enabling either mining path. Miners without a miner token use
self-managed startup behavior and require operator authority; do not distribute
that authority to remote or untrusted workers.

The private gRPC listener binds only to loopback, even with configured bearer
tokens. Remote mining requires an authenticated, encrypted tunnel terminating
at the node's loopback endpoint. For example, on the miner host:

```sh
ssh -N -L 127.0.0.1:15555:127.0.0.1:5555 node-host
# Point the restricted miner at http://127.0.0.1:15555.
```

Keep the tunnel running before connecting miners and ensure the node's port
5555 is not exposed to the network. Containers need a tunnel or proxy in the
node's network namespace with the same loopback-only endpoint. Bearer HTTP
is not encrypted by itself; local harness forwarding is not a substitute for
an authenticated encrypted remote tunnel. Rotate miner credentials and restart
the node and workers together.

Without an operator token the listener refuses startup unless explicitly opted
out with `--allow-unauthenticated-private-grpc` or
`NOCKCHAIN_ALLOW_UNAUTHENTICATED_PRIVATE_GRPC=1`. Use that opt-out only on a
trusted single-user host or local harness where other users and services cannot
reach the loopback endpoint: tokenless clients then have operator authority.
A miner token requires an operator token. Opt-out does not bypass any configured
operator token; tokenless calls remain rejected when that token is configured.

Local single-user bridge launchers (`run-node-only.sh`,
`run-node-and-bridge.sh`, and `bridge-dev`) select the explicit loopback opt-out
only when none of the operator token, miner token, or opt-out environment
variables was supplied. Even an empty supplied value or a false opt-out is
preserved, not replaced by a development default. Authenticated bridge clients
and readiness probes use `NOCKCHAIN_PRIVATE_GRPC_AUTH_TOKEN`; a restricted miner
token cannot authorize bridge operations. Export the operator token in the
environment of bridge-only launches and the zellij server used by
`multi-bridge.sh`; secrets are not copied into bridge configs or runner scripts.
`bridge-dev` node readiness requires an authenticated private RPC and kernel
response, not just an open port.

## State loading and recovery

Loading migrates older state schemas, rebuilds required ASERT timestamp and
puzzle-lineage metadata, refreshes the mining candidate, and discards pending
headers, including when reloading the same schema version. Accepted pages and
retained raw transactions are preserved; retained transactions return to the
mempool. Orphaned claims are repaired, and discarded pages require fresh
admission if received again.

Current-schema ASERT reconstruction processes the retained suffix starting at
height 154,499 in parent-first order. Older schema migrations may traverse
earlier retained pages to derive missing metadata; neither path replays proofs.
The checkpoint policy does not automatically rewind or rewrite accepted pages.

New incoming blocks, genesis admission, pending pages, and locally generated
candidates continue to use the applicable validation rules.

Puzzle verification uses the kernel's Nock wrapper and registered verifier
primitives; it does not require a separate native jet for the wrapper.

Release validation must be assessed against the final build. This note does not
certify full mainnet proof replay, production setup capacity, GPU performance,
or deployment readiness.
