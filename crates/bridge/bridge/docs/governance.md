# Bridge Upgrade & Governance Process

This document codifies how we upgrade the bridge stack—contracts, kernel, and
node software—and who is authorized to operate bridge infrastructure.
It is not a standalone source of chain-level protocol authority.

Protocol authority routing:

- Chain-level protocol index and activation authority: [`docs/protocol/README.md`](../../../../docs/protocol/README.md)
- Canonical protocol upgrade sources: [`docs/protocol/upgrades/`](../../../../docs/protocol/upgrades/)
- This document governs bridge subsystem operations and cannot override indexed protocol specs.

## Actors

- **Bridge multisig (Base)** – Owns `MessageInbox.sol` (UUPS proxy + owner) and
  `Nock.sol`. Minimum requirement: 3-of-5 signatures from the bridge node set or
  a dedicated Gnosis Safe controlled by the same operators.
- **Bridge node operators** – Five parties running the Rust runtime + Hoon
  kernel. They hold the Ethereum keys referenced in `bridge-conf.toml` and the
  Schnorr keys for nockchain signatures.
- **Bridge maintainers** – Engineers permitted to push new Hoon kernels,
  update the Rust crate, or publish contract releases.

## Contract Upgrade Process

1. **Author & review**
   - Implement changes under `crates/bridge/bridge/contracts`.
   - Update or add Foundry tests (`contracts/test/*.t.sol`).
   - Run `forge test` and capture gas snapshots.
2. **Deploy implementation**
   - Use Foundry scripts under `contracts/forge/*.s.sol` (and helper shell
     scripts under `contracts/scripts/` as needed) to deploy a new logic
     contract (do not upgrade yet).
   - Update `contracts/deployments.json` if operators rely on deployment-file
     address discovery.
3. **Multisig approval**
   - Prepare a `upgradeTo(newImplementation)` transaction for the UUPS proxy
     (`MessageInbox`).
   - Collect signatures from the bridge multisig signers.
   - Once executed, emit a memo referencing the git SHA and release notes.
4. **Post-upgrade verification**
   - Call `proxiableUUID()` to ensure the slot matches expectations.
   - Run a canary deposit using the devnet to verify signature validation still
     succeeds.
   - Broadcast the new implementation address to every node operator so they can
     update monitoring and config files.

`Nock.sol` is **not** upgradeable; governance is limited to `updateInbox` and
ownership transfers. Treat `updateInbox` as an emergency-only lever (for
example, if the inbox proxy is compromised) and require the same multisig quorum
before invoking it.

## Kernel & Runtime Releases

1. **Hoon kernel**
   - Modify `hoon/apps/bridge/bridge.hoon` and `hoon/apps/bridge/types.hoon`.
   - Regenerate `assets/bridge.jam` (`make assets/bridge.jam`).
   - Build and release a bridge binary from that commit (the runtime embeds
     `assets/bridge.jam` at build time).
   - Tag the release with git SHA and circulate the release artifact or commit.
   - Rolling-restart operators on the new build; use `--new` only when state
     reset is intentionally required.
2. **Rust runtime**
   - Land changes under `crates/bridge/bridge/src`.
   - Run `cargo test -p bridge --lib` and any integration suites touching the
     Base watcher or ingress server.
   - Produce a signed release binary (or instruct operators to build from the
     tagged commit).
   - Rolling restart the five nodes one at a time to maintain quorum.

## Emergency Procedures

- **Bridge node rotation** – If a node key leaks, use `MessageInbox.updateBridgeNode`
  to swap the compromised address, then distribute a fresh `bridge-conf.toml` to
  every operator so proposer selection stays deterministic.
- **Pause submissions** – There is no on-chain pause switch yet; coordinate a
  social halt by having all nodes stop calling `submitDeposit`. Because the
  kernel persists unsigned proposals, restarting later resumes the queue.
- **Signature threshold adjustment** – Changing the 3-of-5 constant requires a
  contract upgrade; follow the full process above and clearly document the new
  quorum.

## Change Logging

- Record every upgrade (contract logic, kernel jam, runtime release) in the
  bridge ops log with: datetime, git SHA / contract address, signer list, and
  validation steps performed.

## Reference

- `contracts/MessageInbox.sol` – upgradeable inbox (UUPS + Ownable).
- `contracts/Nock.sol` – non-upgradeable ERC-20 with `updateInbox`.
- `crates/bridge/bridge/src/main.rs` – ties together observers, ingress, kernel,
  and signing driver.
- `crates/bridge/bridge/docs/architecture.md` – holistic overview if you need
  to understand the blast radius of a change.
