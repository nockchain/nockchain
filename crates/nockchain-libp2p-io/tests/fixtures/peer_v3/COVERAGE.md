# Historical corpus coverage

This corpus exercises preservation of real consensus objects through the
[Ecclesia v3 boundary](../../../../../changelog/protocol/018-ecclesia.md).
It is one part of protocol validation, not a claim of complete conformance.

## Captured variants

| Requirement | Required | Captured |
| --- | --- | --- |
| Page versions | Untagged v0 and tagged v1 | 14 v0, 17 v1 |
| Raw transaction versions | Untagged v0 and tagged v1 | 13 v0, 6 v1 |
| ZK proof versions | 0, 1, 2, 3, 5 | 4, 2, 16, 4, 2 pages respectively |
| AI proof artifact | `%ai-pow`, classified as version 4 | 3 pages |
| V1 spend variants | Legacy 0 and witness 1 | 2 legacy-spend transactions, 4 witness-spend transactions |
| Lock Merkle proof forms | Untagged stub and `%full` | Both forms in captured v1 transactions |
| Complete nonempty bundles | Page versions 0 and 1 | All transactions for the captured nonempty pages |
| Public checkpoints | Heights 144, 4032, 16128 | All three match the public constants |

The manifest's `required` lists are test assertions. Removing the last example
of any required variant fails the suite. Proof 4 is an AI artifact, not an
additional ZK proof-stream tag. The block at height 147500 contains an AI
artifact; ZK proof 5 is covered by heights 148868 and 148869.

## Assertions

| Specification requirement | Historical check |
| --- | --- |
| Complete page and transaction preservation | Exact source JAM after domain/protobuf conversion |
| Stable consensus identity | Independently decoded IDs; recomputed v1 transaction IDs |
| Original field representation | Existing transaction readers plus direct page tuple inspection |
| Complete block bundle coverage | Original page transaction sets matched to captured transaction IDs |
| One bounded frame per response | Actual codec write/read at the complete encoded frame size |
| Authenticated gossip binding | Reconstructed gossip for page/transaction v0/v1 with solved EquiX and peer-bound verification |
| Fixture integrity and provenance | SHA-256, size, checkpoint, height, variant, and inclusion metadata checked before conversion |

Transport wrappers are reconstructed by the tests. The historical noun is
the oracle; output from the conversion under test is never used to regenerate
that oracle.

## Remaining scope

- Existing structural tests supplement history with empty collections,
  optional-field cases, unsupported variants, duplicate keys, and malformed
  framing. Historical samples are not an exhaustive set of field combinations.
- The corpus does not rerun consensus proof/signature verification. The v0
  transaction checks use the pinned ID and full source noun; the harness has no
  independent v0 transaction-ID computation.
- Cross-runtime protobuf vectors, an exhaustive protobuf merge/unknown-field
  acceptance matrix, and full-node cutover rehearsals remain separate work.
- Operational admission and resource limits are outside this corpus's scope.

No intentionally divergent conversion is accepted; see
[DISCREPANCIES.md](DISCREPANCIES.md).
