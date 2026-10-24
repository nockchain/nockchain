# `nockchain`

`nockchain` is the full-node host and command-line binary. It boots the Nockchain Hoon kernel as a NockApp, installs consensus jets, persists kernel state, and connects the kernel to libp2p, timers, gRPC, metrics, and operator configuration.

## Place in the system

```text
                     nockchain process
  +--------------------------------------------------+
  | Hoon consensus kernel on NockVM                  |
  |   ^        ^              ^                      |
  |   |        |              |                      |
  | libp2p   private/public   timer and persistence  |
  | driver      gRPC          drivers                |
  |                                                  |
  | native STARK, crypto, and mandatory AI verify jets|
  +--------------------------------------------------+
             ^                         ^
       peer network              external miners
```

The Hoon kernel owns chain state and consensus decisions. Rust owns process lifecycle, storage integration, networking, resource controls, and native acceleration. Protocol authority is indexed in [`../../../docs/protocol/README.md`](../../../docs/protocol/README.md); crate code and this README are implementation guidance.

## Maintained invariants

- All external blocks and transactions enter the kernel's normal validation path. Network receipt, gRPC delivery, or miner origin never grants validity.
- Kernel events are processed through the NockApp runtime and their durable state is associated with the ordered event log/checkpoint lifecycle.
- Mainnet constants and fakenet overrides are parsed as complete, internally consistent configurations. Consensus-coupled activation values cannot be independently overridden into contradictory boundaries.
- The node and `roswell` install the same consensus jet registry. A mandatory jet must resolve to the intended Hoon hint and cannot silently fall back to different behavior.
- AI verifier setup is complete and digest-validated before AI verification is available. Local setup faults fail startup or verification loudly rather than changing block acceptance.
- Peer and IP penalties are transport policy. They may limit repeated abuse but do not alter fork choice or make an invalid block valid.
- The private gRPC endpoint is privileged. Public gRPC is enabled only through the explicit API configuration.

## Consensus and cryptographic dependencies

Chain safety depends on deterministic Hoon execution, canonical noun serialization, Nockchain's transaction and block rules, ASERT and accumulated-work fork choice, the Nock ZKVM/STARK verifier, cryptographic jets matching their Hoon specifications, and mandatory AI-PoW verification when that puzzle is active.

Rust process behavior must be deterministic wherever it feeds consensus. Local concerns such as cache residency, peer selection, scheduling, and metrics may vary only when they cannot affect the accepted chain.

The Logos dual-puzzle implementation and its remaining independent cryptographic-review gate are described in [`../../../docs/protocol/upgrades/016-logos.md`](../../../docs/protocol/upgrades/016-logos.md) and the [`ai-pow` audit](../../lib/ai-pow/docs/2026-07-17_DUAL_PUZZLE_CONSENSUS_AUDIT.md).

## Build and run

From the repository root:

```sh
make install-nockchain
nockchain --help
cargo test -p nockchain
```

A changed Hoon kernel must be compiled to its `.jam` before rebuilding this crate; otherwise the binary embeds stale kernel code.

## Operations

Run these examples from the repository root unless a separate data directory is
shown. See the [root setup instructions](../../../README.md#setup) for dependencies
and installation. Before launch, read the [current upgrade requirements](../../../docs/protocol/README.md)
and use a fresh working directory without `.data.nockchain` from testing.

### Mining and keys

```sh
nockchain-wallet keygen
nockchain-wallet export-keys
```

Key generation prints private material and a seed phrase; protect terminal output
and the exported `keys.export`. Restore with `nockchain-wallet import-keys --file
keys.export`. Set `MINING_PKH=<v1-base58-address>` in `.env`, then run:

```sh
bash scripts/run_nockchain_miner.sh
```

A pkh is a public-key hash; v1 pkhs use base58. Multiple miners may use the same
pkh. Coinbase outputs are spendable by the configured `MINING_PKH`. To change the
recipient, update `.env`; generate a new key only if you want a new address. See
the [wallet guide](../nockchain-wallet/README.md) for watch-only identifiers and
key management. Check notes with `nockchain-wallet list-notes-by-address <address>`.

### Local fakenet

Use separate working directories for each instance so their checkpoint/state
files cannot collide. From the repository root:

```sh
mkdir fakenet-hub fakenet-node
cp .env fakenet-hub/
cp .env fakenet-node/
(cd fakenet-hub && bash ../scripts/run_nockchain_node_fakenet.sh)
# In a second terminal:
(cd fakenet-node && bash ../scripts/run_nockchain_miner_fakenet.sh)
```

The hub binds a fixed multiaddr; the miner script uses it as the initial peer.
Additional miner instances need their own working directories. Do not reuse this
state for mainnet.

### Networking and diagnostics

Open the configured P2P UDP port in the firewall and forward it through NAT when
necessary. Bind to an address available on the host, for example:

```sh
nockchain --bind /ip4/0.0.0.0/udp/<peer-port>/quic-v1
```

If peers do not connect, check the bind address, peer IDs, firewall, NAT/port
forwarding, and logs. An outgoing dial failure can mean an offline peer, a
connection limit, or an IP block; a failed dial or handshake timeout does not
necessarily mean the node has no other connections.

- Mining activity includes `%mining-on` log entries.
- In `block <id> added to validated blocks at <height>`, the final number is the block height.
- For startup failures, check port availability, `.env`, existing node state, and filesystem permissions. Preserve state before changing it; use the [PMA recovery guide](../../../docs/pma/DURABILITY-OPERATIONS.md).
- For mining failures, check `MINING_PKH`, the `--mine` flag, connected peers, and available resources.
- For wallet failures, check imported keys, endpoint/socket connectivity, and file permissions.

Set logging with `RUST_LOG`, either in the shell or in `.env` when using the scripts:

```sh
RUST_LOG=info nockchain
RUST_LOG=error nockchain
RUST_LOG=nockchain_libp2p_io=info,nockchain=warn nockchain
```

Levels, most to least verbose: `trace`, `debug`, `info`, `warn`, `error`.
