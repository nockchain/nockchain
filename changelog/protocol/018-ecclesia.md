+++
version = "0.1.18"
status = "draft"
consensus_critical = false

activation_height = 0
published = "2026-09-25"
activation_target = ""

authors = []
reviewers = []

supersedes = "0.1.17"
superseded_by = ""
+++

# Ecclesia

## Summary

Ecclesia replaces the generation-2 CBOR and JAM peer encoding with binary
protobuf messages under `/nockchain-3-req-res`. It retains libp2p over QUIC,
requires checked peer-domain conversion before noun construction, and preserves
the existing consensus objects and validation rules.

## Motivation

Peer messages need an explicit external representation that can be validated
independently of the VM's general noun serialization. Generation 3 assigns
named fields and variants to peer operations, transactions, pages, and facts,
with complete conversion into the existing consensus representation.

This is a transport and representation change. Existing resource controls
remain in force; queueing, admission, and runtime optimization are separate
work.

## Technical Specification

### Registration and framing

Nodes MUST register `/nockchain-3-req-res` for full inbound and outbound
request-response support. Nodes MUST NOT register or fall back to
`/nockchain-2-req-res` or earlier generations. A failed negotiation with an older
peer is a protocol incompatibility.

Each request or response half-stream contains exactly:

```text
4-byte unsigned big-endian length | length bytes of protobuf | EOF
```

The length excludes its own four bytes. The existing request or response byte
cap applies to the complete frame, including those four bytes. The receiver
MUST enforce that cap before reading the body. It MUST read the complete
body and require EOF before interpreting the operation. A truncated prefix,
truncated body, trailing byte, second frame, or malformed protobuf is an error.
The sender closes its write side after its frame. Multiple concurrent libp2p
streams remain supported.

JSON, protobuf Text Format, CBOR, peer-supplied descriptors, and opaque JAM are
not v3 encodings. The trusted `.proto` sources in
[`nockchain.peer.v3`](../../crates/nockchain-libp2p-io/proto/nockchain/peer/v3)
define the binary message fields and numbers.

### Peer-domain conversion

The wire envelope represents batched requests, authenticated gossip, batched
results, and acknowledgments. Request items identify a supported data request.
Gossip carries typed block or transaction facts; elders facts are response-only.
Result envelopes carry typed facts and bundles of pages and transactions. The
request vocabulary and response-to-request matching remain the same as
generation 2.

Generated protobuf messages MUST be treated as unvalidated input. Before an
object can enter the driver, its checked conversion MUST validate:

- required field presence, including explicitly supplied zero or empty values;
- supported variants and enum values;
- fixed byte lengths, integer conversions, and field-element ranges;
- collection uniqueness and ordering where the consensus type requires them;
- consistency between discriminators, optional fields, and related fields;
- complete representation of every field used by the consensus noun.

Conversions MUST fail on invalid input instead of substituting defaults,
truncating values, discarding consensus fields, or using unchecked noun
construction. Authenticated connection state supplies the sending peer's
identity. Existing proof, signature, identity, and consensus checks remain
mandatory after representation validation.

Page version `0` reconstructs the untagged legacy page tuple and its public-key
coinbase map; version `1` reconstructs the `%1` page tuple and its hash-keyed
coinbase map. Other versions and mismatched coinbase variants are invalid.
Both versions retain the optional proof, transaction-ID set, full coinbase,
timestamp, epoch counter, target, accumulated work, height, and message.
Timestamp and epoch counter retain their arbitrary-precision atom values.
Target and accumulated work retain the exact `[%bn (list u32)]` chunks,
including trailing zero chunks. Message elements retain the full Goldilocks
field range. None of these fields may be projected through a narrower API
representation.

### Protobuf acceptance semantics

Singular scalars outside `oneof` MUST use explicit `optional` presence. Message
fields and `oneof` members already track presence. Required values are enforced
by domain conversion, without legacy protobuf `required` fields.

V3 uses standard protobuf decoding semantics. Repeated occurrences of a scalar
use the last value; singular message occurrences merge; repeated fields
concatenate; `oneof` occurrences follow standard protobuf selection and merge
behavior. Unknown fields are discarded by the Prost decoder. The complete
decoded object is the input to every subsequent validation and conversion.

An omitted operation, unsupported decoded variant, unknown enum value, or
unspecified enum value where an operation is required MUST fail validation.
Unknown fields alone do not grant an older implementation new capabilities or
new validation rules. A future mandatory semantic change MUST have an explicit
protocol compatibility decision.

V3 schemas MUST NOT use maps, `Any`, dynamic descriptors, external message
types, floating-point fields, groups, or extensions. Maps are represented as
repeated typed entries so duplicate-key rejection occurs before insertion.
Where all conflicting alternatives must remain visible to validation, the
schema uses separate present fields and an explicit mutual-exclusion check.

The implementation MUST NOT validate one parser's result and then pass the
original peer bytes to another parser as already validated input.

### Arbitrary consensus nouns

Consensus fields whose declared type permits arbitrary nouns use `NounValue`.
This is an explicit flat node table with atoms or pairs of child indices.
The final node is the root. Each cell references only earlier nodes, and every
node MUST be reachable from the root. Empty tables, forward references,
out-of-range references, unreachable nodes, missing cell children, and mixed
atom/cell forms MUST be rejected.

Atom bytes are unsigned little-endian. Zero uses an empty byte string; a nonzero
atom MUST NOT have a zero most-significant byte. An explicitly present empty
atom differs from an omitted atom. Hash limbs are required and MUST be less
than the Goldilocks modulus, `18446744069414584321`.

Only after checking the complete table may the implementation allocate and
connect the corresponding local nouns. The existing noun-position cap remains
in force. These structural checks do not establish proof validity or the
semantic validity of application-defined noun contents.

### Commitments and the internal adapter

The initial implementation adapts checked peer objects into the existing
driver's internal message types. It constructs local nouns, serializes those
nouns with the local canonical JAM encoder, and passes those locally generated
bytes to the existing internal routing path. Any remaining internal cue
operation therefore receives the adapter's output rather than the peer's raw
serialized noun bytes.

Before sending a local message, the outbound adapter checks that reconstruction
produces exactly its original canonical JAM bytes. It rejects a representation
that would change those bytes, including a noncanonical local treap, instead of
silently changing the sender's commitment preimage.

The EquiX preimage retains its existing structure and sender/receiver binding,
with generation-3 domain separation:

- batched requests: `nockchain:req-res:gen3:pow:v1`;
- authenticated gossip: `nockchain:req-res:gen3:gossip:pow:v1`.

Item order, item ID, nonce, request
variant, and the full locally reconstructed payload remain committed. Both
outbound proof construction and inbound proof verification MUST use that same
local reconstruction.

Protobuf bytes MUST NOT serve as a canonical representation for proof-of-work,
signatures, transaction identity, or block identity. Equivalent protobuf
encodings represent the same checked object and therefore the same commitment.
The consensus encodings, hashes, and validation rules continue to apply to the
reconstructed objects.

### Existing execution rules

Sender-bound proof-of-work, replay protection, admission checks, request/result
correlation, bundle ordering, and kernel validation remain required. A batch
request receives a batch result, while authenticated gossip receives an
acknowledgment. Bundled transactions are routed before their block fact.

The envelope constructor checks each bundle's transaction-ID coverage and
rejects duplicate or overlapping entries. Request-aware driver validation still
checks response correlation, requested heights, range order and continuity,
and parent linkage before routing facts to the kernel. Successful protobuf or
peer-domain decoding does not replace those checks.

Existing request, response, item, batch, inflight, queue, and noun bounds remain
in force. Existing configuration names containing `gen2` may remain for these
limits and scheduling features; their names do not enable generation-2 wire
negotiation.

## Activation

- **Height**: `0`; there is no consensus-height trigger.
- **Coordination**: deploy generation-3-capable software to the participating
  network as one coordinated transport cutover.
- **Target**: unset while the specification is draft. A deployment date and
  reviewers must be recorded before finalization.

## Migration

### Requirements

Nodes exchanging peer traffic need a release containing the Ecclesia v3
implementation. The specification version `0.1.18` does not itself identify a
published binary.

### Configuration

No per-peer generation selection or v2 compatibility flag is provided. Existing
byte, queue, inflight, bundle, and prefetch settings remain applicable.

### Data Migration

No persisted chain-state migration is required. Transaction, note, proof, and
block consensus identities retain their existing definitions.

### Steps

1. Complete review and interoperability validation before scheduling deployment.
2. Upgrade the nodes and bootstrap peers that must remain mutually reachable.
3. Confirm negotiation of `/nockchain-3-req-res` and successful authenticated
   gossip, batched requests, bundles, and range responses.
4. Monitor incompatible peers, decode failures, proof-of-work rejection,
   response validation, and synchronization progress during the cutover.

### Rollback

A node downgraded to v2 cannot exchange request-response traffic with v3-only
peers. Rollback therefore requires coordinated transport compatibility across
the participating peers; there is no runtime fallback flag. This transport
change introduces no chain-state rollback requirement.

## Backward Compatibility

The wire protocol is intentionally incompatible with generation 2. Older nodes
cannot negotiate peer requests with upgraded nodes and must upgrade to
participate in their peer network. Existing consensus-valid transactions and
blocks remain valid when represented by the v3 schema; this upgrade does not
change their consensus rules.

[`013-nous.md`](./013-nous.md) remains the historical generation-2 specification.
Its CBOR and raw-JAM wire requirements do not describe v3.

## Security Considerations

The parser schema is fixed at build time. Peers control field values but cannot
select executable schema logic. The boundary between generated messages and
checked domain types is mandatory even when protobuf decoding succeeds.

Flat noun tables separate structural representation checks from local noun
allocation. Precise scalar presence and fallible conversions prevent omitted
values, unsupported variants, and narrowing conversions from silently changing
an object's meaning. Complete peer types preserve all consensus fields across
the boundary.

Protobuf's merge and unknown-field semantics are part of the acceptance
contract. Applications must base security decisions on the validated object,
and must specify commitments independently of the original protobuf encoding.
Transport authentication does not replace application or consensus validation.

## Operational Impact

Operators must coordinate compatible node and bootstrap-peer deployments before
the cutover. A mixed v2/v3 network loses request-response connectivity across
the version boundary. Existing admission and runtime limits remain configured
and monitored; this upgrade makes no throughput or memory-use guarantee.

No wallet transaction-format migration or block-height activation is introduced.
After deployment, operators should confirm catch-up and gossip progress as well
as successful protocol negotiation.

## Testing and Validation

Required validation covers:

- production schema policy and rejection of prohibited schema edits;
- complete framing, truncated frames, trailing bytes, unsupported protocol IDs,
  and configured request/response limits;
- typed round trips for every request, response, transaction, and page variant;
- omitted fields, invalid scalar ranges, duplicate collection keys, and
  inconsistent variants;
- noun-table topology, reachability, atom representation, and noun round trips;
- standard protobuf duplicate/merge and unknown-field behavior;
- canonical local commitment reconstruction and sender/receiver/payload
  proof-of-work binding;
- live peer negotiation, gossip, batches, bundles, response correlation, and
  existing retry and admission behavior.

Passing schema checks alone does not establish conversion correctness or
interoperability. Run the crate's unit and integration suites and retain
cross-implementation fixtures before finalizing the specification.

## Reference Implementation

- [`proto` schemas and maintenance](../../crates/nockchain-libp2p-io/proto/README.md)
- [`src/v3`](../../crates/nockchain-libp2p-io/src/v3)
- [`behaviour.rs`](../../crates/nockchain-libp2p-io/src/behaviour.rs)
- [`config.rs`](../../crates/nockchain-libp2p-io/src/config.rs)
- [`messages.rs`](../../crates/nockchain-libp2p-io/src/messages.rs)
- [`peer_v3_schema.rs`](../../crates/nockchain-libp2p-io/tests/peer_v3_schema.rs)
