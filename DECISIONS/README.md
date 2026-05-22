# DECISIONS

Status: Active
Owner: Nockchain Maintainers
Last Reviewed: 2026-02-20
Canonical/Legacy: Canonical (ADR index for durable technical decisions)

`DECISIONS/` stores Architecture Decision Records (ADRs), the durable "why" behind major choices.

## Status Lifecycle

`proposed -> accepted -> superseded`

- `proposed`: Draft decision under review.
- `accepted`: Decision is active guidance.
- `superseded`: Replaced by a newer ADR.

## Naming Convention

- Filename: `NNNN-short-kebab.md` (zero-padded integer + concise slug).
- Title line: `# ADR NNNN: <Decision Title>`.
- Sequence is append-only, do not renumber older ADRs.

## Supersession Policy

1. Never delete or rewrite the core rationale in accepted ADRs.
2. To replace a decision, create a new ADR that references `Supersedes: ADR NNNN`.
3. Mark the older ADR as `superseded` and point `Superseded by` to the new ADR.
4. Update the index table below in the same change.

## ADR Index

| ADR  | Title                           | Status   | Date       | Supersedes | File                                                                                   |
| ---- | ------------------------------- | -------- | ---------- | ---------- | -------------------------------------------------------------------------------------- |
| 0001 | Doc Spine And Authority         | accepted | 2026-02-18 | none       | [`0001-doc-spine-and-authority.md`](./0001-doc-spine-and-authority.md)                 |
| 0002 | Protocol-First Canonicalization | accepted | 2026-02-18 | none       | [`0002-protocol-first-canonicalization.md`](./0002-protocol-first-canonicalization.md) |
| 0003 | Merge Master Into Long-Running Branches | accepted | 2026-05-03 | none       | [`0003-merge-master-into-long-running-branches.md`](./0003-merge-master-into-long-running-branches.md) |

## Add A New ADR

1. Copy [`TEMPLATE.md`](./TEMPLATE.md) to `DECISIONS/NNNN-short-kebab.md`.
2. Fill in context, decision, alternatives, consequences, and rollout sections.
3. Add the ADR to the index table above.

## Related Spine Docs

- [`../START_HERE.md`](../START_HERE.md)
- [`../PROTOCOL.md`](../PROTOCOL.md)
- [`../ARCHITECTURE.md`](../ARCHITECTURE.md)
- [`../WORKFLOWS.md`](../WORKFLOWS.md)
