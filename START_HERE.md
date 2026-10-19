# START_HERE

This file defines the docs trust contract for humans and LLMs.
If documents conflict, follow this file and the canonical spine below.

## Read Order (First 5 Minutes)

1. Start here: `START_HERE.md` (this file).
2. Read canonical spine docs in this order:
   - Protocol authority: [`PROTOCOL.md`](./PROTOCOL.md)
   - Architecture boundaries/invariants: [`ARCHITECTURE.md`](./ARCHITECTURE.md)
   - Workflow golden paths: [`WORKFLOWS.md`](./WORKFLOWS.md)
   - Decision history index: [`DECISIONS/README.md`](./DECISIONS/README.md)
3. Then read scoped satellites linked by the spine:
   - Quickstart and operational commands: [`README.md`](./README.md)
   - Runtime interface and kernel integration: [`crates/nockapp/README.md`](./crates/nockapp/README.md)
   - Public API runtime/deployment guidance: [`crates/nockchain-api/README.md`](./crates/nockchain-api/README.md)
   - Wallet CLI behavior and usage: [`crates/nockchain-wallet/README.md`](./crates/nockchain-wallet/README.md)
   - Deeper protocol notes and historical context: [`docs/NOCKCHAIN.md`](../docs/NOCKCHAIN.md)

## Trust And Canonical Policy

- Tier 0 (canonical spine): this file, `PROTOCOL.md`, `ARCHITECTURE.md`, `WORKFLOWS.md`, `DECISIONS/README.md`.
- Tier 0 protocol rule authority: upgrade specs in `changelog/protocol/` as indexed by `PROTOCOL.md`.
- Tier 1 (scoped canonical satellites): docs directly linked by Tier 0 that declare `Canonical/Legacy: Canonical` for a bounded subsystem scope.
- Tier 2 (legacy or historical): docs marked `Canonical/Legacy: Legacy`, plus unpromoted docs outside Tier 0/Tier 1.
- Conflict rule: Tier 0 overrides Tier 1, Tier 1 overrides Tier 2.
- Isolation rule: do not trust a crate README in isolation as canonical protocol or architecture guidance.

## Demotions In This Revision

- `../docs/NOCKCHAIN.md` and `README.md` are no longer Tier 0; they are Tier 1 scoped satellites.

## Promotions In This Revision

- [`crates/nockapp/README.md`](./crates/nockapp/README.md): promoted to Tier 1 scoped canonical satellite for NockApp runtime interface usage.
- [`crates/nockchain-api/README.md`](./crates/nockchain-api/README.md): promoted to Tier 1 scoped canonical satellite for public API runtime/deployment guidance.
- [`crates/nockchain-wallet/README.md`](./crates/nockchain-wallet/README.md): promoted to Tier 1 scoped canonical satellite for wallet CLI behavior and operations.

## Promotion Gate (Mandatory)

Promoting a doc from Tier 2 to Tier 1 is not a relabel.
Promotion MUST include technical hardening in the same change:

1. Update this file's `Promotions In This Revision` section.
2. Update the promoted doc header to `Canonical/Legacy: Canonical` with explicit Tier 1 scope.
3. Add all of these sections to the promoted doc (exact headings):
   - `## Canonical Scope`
   - `## Failure Modes And Limits`
   - `## Verification Contract`
4. State what the doc is NOT authoritative for, and route those decisions to the canonical authority doc.
5. Record promotion rationale in PR/commit history (not in the promoted document body).
## Promotion Eligibility Criteria (Objective)

A doc is a Tier 1 promotion candidate only if all are true:

1. It is directly linked from a Tier 0 doc as an operational or interface dependency.
2. It defines a stable subsystem contract (runtime interface, operator workflow, or CLI/API behavior), not historical commentary.
3. Incorrect guidance would create material operational risk (safety, security, availability, or funds risk).
4. The scope boundary can be stated precisely, including what remains Tier 0 authority.
5. The document has a named owner and an explicit verification contract.

## Legacy / Non-Canonical Warning

Many repository docs predate this contract. They may still be useful, but they are not normative unless promoted by Tier 0.

If a doc is not in Tier 0 and is not directly referenced by Tier 0 as authoritative for a scope, treat it as historical context only.

## Maintenance Contract For Editors

When adding or promoting top-level docs, update this file in the same change:

1. Add the doc to read order and canonical tier.
2. State which older docs are demoted to Tier 2.
3. Keep links valid among `START_HERE.md`, `PROTOCOL.md`, `ARCHITECTURE.md`, `WORKFLOWS.md`, and `DECISIONS/README.md`.
