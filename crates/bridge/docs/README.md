# Bridge Documentation Index

Status: Active
Owner: Nockchain Maintainers
Last Reviewed: 2026-03-12
Canonical/Legacy: Legacy (bridge subsystem index; canonical docs spine starts at [`START_HERE.md`](../../../START_HERE.md))

Use this as the bridge docs landing page. Start with architecture, then drill
into setup, operations, and governance guides.

1. [`architecture.md`](./architecture.md) – Component map, event/effect flow,
   and critical pathways between Nockchain, Base, and the Hoon kernel.
2. [`sans-io-architecture.md`](./sans-io-architecture.md) – Canonical sans-IO
   execution architecture (planners, ports, loop shells, deterministic
   testability).
3. [`signatures.md`](./signatures.md) – Canonical signature formats, hashing,
   verification rules, and recommended validation steps.
4. [`node-runbook.md`](./node-runbook.md) – Operational playbook for bridge node
   provisioning, monitoring, logging, and incident response.
5. [`governance.md`](./governance.md) – Upgrade procedures for contracts, the
   kernel jam, and the Rust runtime, plus multisig responsibilities.
6. [`release-punchlist.md`](./release-punchlist.md) – Mainnet launch checklist
   for network-specific config, contract wiring, and R2 journal setup.
7. [`../OPERATOR-SETUP.md`](../OPERATOR-SETUP.md) – Provisioning and initial
   host/bootstrap setup for new bridge operators.
8. [`../QUICKSTART.md`](../QUICKSTART.md) – Build/test/run quickstart for local
   bridge development and first boot.
9. [`bridge-withdrawals.md`](./bridge-withdrawals.md) – Canonical bridge
   withdrawal protocol and implementation spec.
10. [`withdrawal-dapp.md`](./withdrawal-dapp.md) – DApp/frontend-oriented
   withdrawal product and implementation guide derived from the canonical
   withdrawal spec.
11. [`../draft/README.md`](../draft/README.md) – Withdrawal milestone plans
   and dated progress log.
12. [`../draft/specs/test-harness.md`](../draft/specs/test-harness.md) –
   Deterministic test harness design for bridge runtime/kernel tests and the
   kernel-to-signing seam.

When adding new docs, update this index so we have a single place to point future readers.
