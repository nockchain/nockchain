# Wire conformance discrepancies

There are no accepted differences between Prost and the Google protobuf
reference for the known semantics of these 23 vectors. All expectations are
ordinary passing assertions; none use skip or expected-failure handling.

The peer domain deliberately imposes requirements beyond protobuf syntax.
Protobuf can successfully decode an absent operation, an absent required domain
field, or an unknown enum number. The corresponding `domain.verdict` is
`reject`; this is application validation, not a protobuf parsing discrepancy.

The checked peer boundary preserves known meaning and constructs canonical
internal nouns. It does not promise to retain unknown protobuf fields when
writing a checked message again. The reference comparison therefore examines
known semantic objects, not serialized output bytes. These limits are recorded
in [COVERAGE.md](COVERAGE.md).

Reviewed: 2026-09-26.
