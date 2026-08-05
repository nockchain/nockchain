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

The Hoon kernel owns chain state and consensus decisions. Rust owns process lifecycle, storage integration, networking, resource controls, and native acceleration. Protocol authority is indexed in [`../../PROTOCOL.md`](../../PROTOCOL.md); crate code and this README are implementation guidance.

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

The Logos dual-puzzle implementation and its remaining independent cryptographic-review gate are described in [`../../changelog/protocol/016-logos.md`](../../changelog/protocol/016-logos.md) and the [`ai-pow` audit](../ai-pow/docs/2026-07-17_DUAL_PUZZLE_CONSENSUS_AUDIT.md).

## Build and run

From the repository root:

```sh
make install-nockchain
nockchain --help
cargo test -p nockchain
```

A changed Hoon kernel must be compiled to its `.jam` before rebuilding this crate; otherwise the binary embeds stale kernel code.
