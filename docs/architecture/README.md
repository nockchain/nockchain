# Architecture

Nockchain runs a Hoon consensus kernel inside NockApp on NockVM. Rust drivers
connect the kernel to peers, gRPC, timers, metrics, and persistent storage.
External miners submit proofs; the node validates them independently. Wallets and
APIs construct transactions or expose derived chain state, not consensus rules.

## System boundaries

| Area | Responsibility | Documentation |
| --- | --- | --- |
| Protocol and consensus | Transaction validity, consensus transitions, upgrade gates | [Protocol and upgrades](../protocol/README.md) |
| Node runtime and networking | Kernel execution, peer communication, chain sync | [Node](../../crates/apps/nockchain/README.md), [NockApp](../../crates/lib/nockapp/README.md), [NockVM](../../crates/lib/nockvm/README.md) |
| Wallet and public API | Key management, transaction construction, service exposure | [Wallet](../../crates/apps/nockchain-wallet/README.md), [API](../../crates/apps/nockchain-api/README.md) |
| Bridge | Cross-chain contracts, runtime and operator policy | [Architecture](../../crates/bridge/bridge/docs/architecture.md), [runbook](../../crates/bridge/bridge/docs/node-runbook.md) |
| Tooling | Compiler/bootstrap workflows and packaging | [Hoon compiler](../../crates/tools/hoonc/README.md), [native compiler](../native-compiler/README.md), [Nockup](../../crates/tools/nockup/README.md) |

## Invariants

- The [protocol specifications](../protocol/README.md) define consensus behavior and
  activation conditions. Runbooks and crate documentation do not redefine them.
- Node implementations must resolve the same consensus inputs to the same outputs.
- Consensus changes require a versioned upgrade specification and coordinated
  activation; deployment policy is not an alternative consensus mechanism.
- Applications execute offchain as NockApps. General NockApp proof verification,
  privacy, and useful-work network designs remain proposals unless adopted by
  the protocol specifications.

For data structures, validation, fees, and cryptography, see the
[transaction-engine overview](tx-engine/00-overview.md).
