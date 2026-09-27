# Peer v3 protobuf wire conformance

The 23 fixed vectors in [vectors.json](vectors.json) test the known meaning of
tiny protobuf messages: values, explicit presence, selected oneof member, and
repeated-field order. Their largest protobuf body is 195 bytes. All 23 are
valid protobuf; the checked peer domain accepts 16 and rejects 7.

The independent Google protobuf runner and the Rust test consume the same
authored semantic expectations. Rust additionally sends every raw body through
the actual libp2p `ProtobufCodec` frame reader. Accepted cases must produce the
expected canonical driver noun and preserve known meaning through the frame
writer. Direct assertions also compare the checked proof, nonce, item IDs,
acknowledgement value, result status, and error classification with the authored
expectations; those checks do not rely on encoder/decoder symmetry. The required
categories below are asserted by the Rust harness.

| Requirement | Category | Fixed cases | Required behavior |
| --- | --- | ---: | --- |
| Singular scalar duplicates | `duplicate_scalar` | 3 | Last occurrence wins, including explicit zero and known enum following unknown enum. |
| Explicit presence | `presence` | 2 | An absent nonce or error classification parses but fails domain validation. |
| Singular embedded message duplicates | `singular_message_merge` | 1 | Occurrences merge their fields. |
| Repeated occurrence of the same message oneof member | `oneof_same_message` | 3 | Merge fields, including an empty occurrence and a split root batch. |
| Different oneof member | `oneof_different_member` | 1 | Replace the selected member. |
| Oneof switch back to an earlier member | `oneof_switchback` | 1 | Start that member afresh; its earlier fields do not reappear. |
| Repeated scalar oneof member | `oneof_scalar` | 2 | Last value wins, including zero and false. |
| Repeated numeric fields | `repeated_numeric` | 2 | Concatenate packed and unpacked fixed32/fixed64 segments in occurrence order; accept an empty packed segment. |
| Unknown fields | `unknown_fields` | 5 | Preserve recognized meaning at root and nested levels; unknown tags neither choose nor clear a known operation. Unknown-only operations fail domain validation. |
| Open enum values | `enum` | 3 | Preserve the protobuf enum value; domain validation accepts known classifications and rejects unknown or unspecified values. |

A separate Rust test solves EquiX for one request, manually reorders its known
protobuf fields, and adds harmless unknown fields at two levels. Framed decoding
must preserve its checked request, replay key, and sender/receiver-bound proof.
The fixed vectors' zero-filled proof fields test representation only; they do
not assert authenticated admission.

The minimal page in the numeric-field cases is a synthetic representation
fixture. It is not an accepted-chain block. Historical accepted pages and
transactions are covered separately by [the captured corpus](../peer_v3/COVERAGE.md).

This matrix covers the selected proto3 parsing rules, not every protobuf wire
rule or every consensus requirement. It does not cover malformed encodings,
resource limits, deprecated groups, unknown-field round-trip retention, or
consensus acceptance. Existing v3 unit tests cover framing failures, required
domain fields, structural validation, and protocol operation variants.
