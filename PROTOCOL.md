# PROTOCOL

Status: Active
Owner: Nockchain Maintainers
Last Reviewed: 2026-02-19
Canonical/Legacy: Canonical (protocol authority entrypoint for the nockchain repository)

This is the canonical protocol index for the nockchain repository.
If protocol guidance conflicts with workflow or crate docs, this page and the linked upgrade specs win.

## Read Order

1. [`PROTOCOL.md`](./PROTOCOL.md) (authority and index).
2. [`changelog/protocol/SPECIFICATION.md`](./changelog/protocol/SPECIFICATION.md) (required spec format and lifecycle).
3. The specific upgrade spec from the index below.

## Current Release Track

- Next scheduled activation: [`012-bythos.md`](./changelog/protocol/012-bythos.md), version `0.1.11`, target date `2026-03-01`, activation height `54000`.

## Upgrade Index

Legend:

- `activation_height = 0` means there is no consensus height trigger recorded (historical gap or rollout-gated activation coordinated operationally).
- Status lifecycle: normal path is `draft -> final -> activated`; replacement path (for plans withdrawn before activation) is `draft -> final -> superseded`.
- `superseded` means "not the active deployment target". Activated historical upgrades may still carry `superseded_by` in frontmatter as release-track lineage metadata.

| Seq | Codename                                 | Version | Status     | Activation Height | Activation Target | Spec                                                                                                  |
| --- | ---------------------------------------- | ------- | ---------- | ----------------- | ----------------- | ----------------------------------------------------------------------------------------------------- |
| 012 | Bythos                                   | 0.1.11  | final      | 54000             | 2026-03-01        | [`012-bythos.md`](./changelog/protocol/012-bythos.md)                                                 |
| 011 | Legacy 011 - LMP Axis Hotfix             | 0.1.10  | activated  | 0                 | -                 | [`011-legacy-lmp-axis-hotfix.md`](./changelog/protocol/011-legacy-lmp-axis-hotfix.md)                 |
| 010 | Legacy 010 - V1 Phase 39000              | 0.1.9   | activated  | 39000             | -                 | [`010-legacy-v1-phase-39000.md`](./changelog/protocol/010-legacy-v1-phase-39000.md)                   |
| 009 | Legacy 009 - Segwit Cutover Initial      | 0.1.8   | superseded | 37350             | -                 | [`009-legacy-segwit-cutover-initial.md`](./changelog/protocol/009-legacy-segwit-cutover-initial.md)   |
| 008 | Legacy 008 - Checkpoint 16128            | 0.1.7   | activated  | 16128             | -                 | [`008-legacy-checkpoint-16128.md`](./changelog/protocol/008-legacy-checkpoint-16128.md)               |
| 007 | Legacy 007 - Proof Version 2 at 12000    | 0.1.6   | activated  | 12000             | -                 | [`007-legacy-proof-v2-12000.md`](./changelog/protocol/007-legacy-proof-v2-12000.md)                   |
| 006 | Legacy 006 - Proof Version 1 at 6750     | 0.1.5   | activated  | 6750              | -                 | [`006-legacy-proof-v1-6750.md`](./changelog/protocol/006-legacy-proof-v1-6750.md)                     |
| 005 | Legacy 005 - Checkpoint 4032             | 0.1.4   | activated  | 4032              | -                 | [`005-legacy-checkpoint-4032.md`](./changelog/protocol/005-legacy-checkpoint-4032.md)                 |
| 004 | Legacy 004 - Sign Output Source          | 0.1.3   | activated  | 0                 | -                 | [`004-legacy-sign-output-source.md`](./changelog/protocol/004-legacy-sign-output-source.md)           |
| 003 | Legacy 003 - Checkpoint 2448             | 0.1.2   | activated  | 2448              | -                 | [`003-legacy-checkpoint-2448.md`](./changelog/protocol/003-legacy-checkpoint-2448.md)                 |
| 002 | Legacy 002 - Checkpoints Genesis and 720 | 0.1.1   | activated  | 720               | -                 | [`002-legacy-checkpoints-genesis-720.md`](./changelog/protocol/002-legacy-checkpoints-genesis-720.md) |
| 001 | Legacy 001 - Checkpoint 144              | 0.1.0   | activated  | 144               | -                 | [`001-legacy-checkpoint-144.md`](./changelog/protocol/001-legacy-checkpoint-144.md)                   |

## Maintenance Rule

When adding or updating any file in `changelog/protocol/`, update this index in the same change.

## Related Spine Docs

- [`START_HERE.md`](./START_HERE.md)
- [`ARCHITECTURE.md`](./ARCHITECTURE.md)
- [`WORKFLOWS.md`](./WORKFLOWS.md)
- [`DECISIONS/README.md`](./DECISIONS/README.md)
