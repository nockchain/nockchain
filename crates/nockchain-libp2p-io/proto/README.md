# Peer protocol v3 schemas

The binary messages in `nockchain.peer.v3` define the external peer boundary.
The [Ecclesia specification](../../../changelog/protocol/018-ecclesia.md)
defines framing, validation, commitments, and rollout. Generated Prost structs
are unvalidated wire values; checked types in [`src/v3`](../src/v3) own their
conversion into consensus nouns.

## Schema policy

[`build.rs`](../build.rs) compiles the schemas with `protoc`, checks the resulting
descriptors using [`build_support/schema.rs`](../build_support/schema.rs), and
then generates Rust. Both Cargo and Bazel run this policy. Descriptors are
trusted build artifacts; peers cannot supply or select descriptors.

The build rejects:

- packages and imports outside `nockchain.peer.v3`;
- `Any`, dynamic descriptors, and other external message types;
- maps, floating-point fields, legacy groups, `required`, and extensions;
- RPC service declarations, since libp2p defines the transport;
- singular scalars without `optional`, unless they are `oneof` members;
- enums whose first value is not zero with an `_UNSPECIFIED` name.

Message fields already track presence. Repeated fields represent collections;
their constructors define whether emptiness, ordering, and duplicates are
allowed. Required fields, supported enum values, byte lengths, field-element
ranges, and relationships between fields are checked by the domain conversion.
Adding a field requires its validation and noun conversion in the same change.

Standard protobuf merge behavior applies: later scalar values replace earlier
ones, singular submessages merge, repeated fields concatenate, and `oneof`
members use standard protobuf selection semantics. Prost discards unknown
fields. Validation uses the completed decoded object. Do not forward original
bytes to another parser while assuming they have already been validated.

For collections that must reject duplicate keys, use repeated entries and
check uniqueness before constructing a map or set. If conflicting alternatives
must be rejected, use individually present fields and check mutual exclusion;
an ordinary `oneof` does not preserve every alternative seen on the wire.

Every `bytes` field needs a format contract. There is no opaque JAM payload on
the v3 wire. Arbitrary consensus noun fields use `NounValue`, whose completed
node table is checked before constructing nouns. Binary protobuf serialization
is not canonical and must not become a cryptographic commitment format.

## Build and validation

Cargo requires `protoc` on `PATH` or a `PROTOC` binary path, as does the existing
gRPC schema build. Run these from the repository root:

```sh
cargo test -p nockchain-libp2p-io --test peer_v3_schema
cargo test -p nockchain-libp2p-io --lib v3::
bazel test //crates/nockchain-libp2p-io:peer_v3_schema_test
```

The [historical corpus](../tests/fixtures/peer_v3/PROVENANCE.md) pins real pages
and transactions across both consensus versions and all six proof variants.
Its tests preserve original JAM through the checked types and framed codec,
verify block inclusion, and exercise reconstructed authenticated gossip.

Code generation writes `nockchain.peer.v3.rs` and `peer_v3_descriptor.bin` into
the build output directory. These files are not checked in. The descriptor
tests exercise the production schema and prohibited schema changes.

[`buf.yaml`](../buf.yaml) adds standard naming lint and `FILE` compatibility
checks. Buf is optional developer tooling; the build-time policy does not
depend on it. From this crate's directory, run:

```sh
buf lint
buf breaking --against /path/to/reviewed-peer-v3.binpb
```

Create the baseline image with `buf build -o /path/to/reviewed-peer-v3.binpb`
from the reviewed release checkout. A baseline must contain v3; the initial
v3 addition has no prior v3 schema to compare against. The compatibility check
is an explicit maintenance step, not something the descriptor policy can infer
from a single revision.

Never reuse field numbers or enum values. Reserve the names and numbers of
removed fields and enum values, and review deletions against the compatibility
policy. Passing a wire-compatibility check does not establish semantic
compatibility: older nodes cannot enforce a new requirement carried only by a
new unknown field. Such changes require an explicit version and rollout rule.
