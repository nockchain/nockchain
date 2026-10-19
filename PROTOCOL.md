# PROTOCOL

Owner: Nockchain Maintainers
Last Reviewed: 2026-09-22
Canonical/Legacy: Canonical (protocol authority entrypoint for the nockchain repository)

This is the canonical protocol index for the nockchain repository.
If protocol guidance conflicts with workflow or crate docs, this page and the linked upgrade specs win.

## Read Order

1. [`PROTOCOL.md`](./PROTOCOL.md) (authority and index).
2. [`changelog/protocol/SPECIFICATION.md`](./changelog/protocol/SPECIFICATION.md) (required spec format and process).
3. The specific upgrade spec from the index below.

## Current Release Track

- Current coordinated upgrade at candidate height `170000`: [subsequent reset and native AI-PoW admission specification](./changelog/protocol/017-anthropos.md#subsequent-reset-at-height-170000). Both ASERT lanes re-anchor once to branch-local predecessor `169999` MTP; deploy matching artifacts before that parent height. Updated transaction validation applies immediately at all spending heights, independently of the height-gated rules.
- Most recently activated upgrade, ZK-PoW `%5` and 70/30 puzzle allocation: [`017-anthropos.md`](./changelog/protocol/017-anthropos.md), version `0.1.17`, activation height `147500`.
- Activated dual proof-of-work upgrade: [`016-logos.md`](./changelog/protocol/016-logos.md), version `0.1.16`, activation height `126000`.
- Pearl merge-mining clarification in Logos: transaction-bearing Pearl blocks admit coinbase Merkle paths of up to 32 siblings; see its transaction commitment and compatibility sections before rollout.
- Coordinated node, kernel, Rust verifier, miner, and wallet deployment: [operator guidance](./docs/AI_POW_CUTOVER.md), including setup retention, recovery, and restricted mining access. Existing bridge withdrawals remain disabled.

## Upgrade Index

Legend:

- `activation_height = 0` means there is no consensus height trigger recorded (historical gap or rollout-gated activation coordinated operationally).

| Seq | Codename                                 | Version | Activation Height                        | Activation Target | Spec                                                                                                  |
| --- | ---------------------------------------- | ------- | ---------------------------------------- | ----------------- | ----------------------------------------------------------------------------------------------------- |
| 017 | Anthropos                                | 0.1.17  | 147500                                   | -                 | [`017-anthropos.md`](./changelog/protocol/017-anthropos.md)                                           |
| 016 | Logos                                    | 0.1.16  | 126000                                   | -                 | [`016-logos.md`](./changelog/protocol/016-logos.md)                                                   |
| 015 | Zoe                                      | 0.1.15  | 119400                                   | 2026-08-06        | [`015-zoe.md`](./changelog/protocol/015-zoe.md)                                                       |
| 014 | Aletheia                                 | 0.1.14  | 65500                                    | -                 | [`014-aletheia.md`](./changelog/protocol/014-aletheia.md)                                             |
| 013 | Nous                                     | 1.0.0   | 0                                        | 2026-Q2           | [`013-nous.md`](./changelog/protocol/013-nous.md)                                                     |
| 012 | Bythos                                   | 0.1.11  | 54000                                    | 2026-03-01        | [`012-bythos.md`](./changelog/protocol/012-bythos.md)                                                 |
| 011 | Legacy 011 - LMP Axis Hotfix             | 0.1.10  | 0                                        | -                 | [`011-legacy-lmp-axis-hotfix.md`](./changelog/protocol/011-legacy-lmp-axis-hotfix.md)                 |
| 010 | Legacy 010 - V1 Phase 39000              | 0.1.9   | 39000                                    | -                 | [`010-legacy-v1-phase-39000.md`](./changelog/protocol/010-legacy-v1-phase-39000.md)                   |
| 009 | Legacy 009 - Segwit Cutover Initial      | 0.1.8   | 37350 (never activated; replaced by 010) | -                 | [`009-legacy-segwit-cutover-initial.md`](./changelog/protocol/009-legacy-segwit-cutover-initial.md)   |
| 008 | Legacy 008 - Checkpoint 16128            | 0.1.7   | 16128                                    | -                 | [`008-legacy-checkpoint-16128.md`](./changelog/protocol/008-legacy-checkpoint-16128.md)               |
| 007 | Legacy 007 - Proof Version 2 at 12000    | 0.1.6   | 12000                                    | -                 | [`007-legacy-proof-v2-12000.md`](./changelog/protocol/007-legacy-proof-v2-12000.md)                   |
| 006 | Legacy 006 - Proof Version 1 at 6750     | 0.1.5   | 6750                                     | -                 | [`006-legacy-proof-v1-6750.md`](./changelog/protocol/006-legacy-proof-v1-6750.md)                     |
| 005 | Legacy 005 - Checkpoint 4032             | 0.1.4   | 4032                                     | -                 | [`005-legacy-checkpoint-4032.md`](./changelog/protocol/005-legacy-checkpoint-4032.md)                 |
| 004 | Legacy 004 - Sign Output Source          | 0.1.3   | 0                                        | -                 | [`004-legacy-sign-output-source.md`](./changelog/protocol/004-legacy-sign-output-source.md)           |
| 003 | Legacy 003 - Checkpoint 2448             | 0.1.2   | 2448                                     | -                 | [`003-legacy-checkpoint-2448.md`](./changelog/protocol/003-legacy-checkpoint-2448.md)                 |
| 002 | Legacy 002 - Checkpoints Genesis and 720 | 0.1.1   | 720                                      | -                 | [`002-legacy-checkpoints-genesis-720.md`](./changelog/protocol/002-legacy-checkpoints-genesis-720.md) |
| 001 | Legacy 001 - Checkpoint 144              | 0.1.0   | 144                                      | -                 | [`001-legacy-checkpoint-144.md`](./changelog/protocol/001-legacy-checkpoint-144.md)                   |

## Maintenance Rule

When adding or updating any file in `changelog/protocol/`, update this index in the same change.

## Related Spine Docs

- [`START_HERE.md`](./START_HERE.md)
- [`ARCHITECTURE.md`](./ARCHITECTURE.md)
- [`WORKFLOWS.md`](./WORKFLOWS.md)
- [`DECISIONS/README.md`](./DECISIONS/README.md)
