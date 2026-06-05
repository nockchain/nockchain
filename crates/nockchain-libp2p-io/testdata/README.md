# Req-Res Conformance Vectors

This directory holds machine-readable CBOR conformance vectors for the libp2p request-response transport.

Current fixture:
- `req_res_gen1_cbor_vectors.json`

## Fixture goals

- Keep canonical bytes explicit and reviewable.
- Make serialization behavior executable in tests.
- Give third-party implementations a deterministic target.

## Schema summary

Top-level fields:
- `schema_version`
- `request_vectors`
- `response_vectors`
- `invalid_vectors`

`request_vectors` and `response_vectors` include:
- a stable `id`
- structured fields (`variant`, payload fields)
- canonical `cbor_hex`

`invalid_vectors` include:
- a stable `id`
- decode target (`request` or `response`)
- malformed `cbor_hex`
- optional `error_substring` assertion

## Validation

Vectors are executed by tests in:
- `open/crates/nockchain-libp2p-io/src/cbor_tests.rs`

Current entry points:
- `test_gen1_cbor_vector_schema_version`
- `test_gen1_request_cbor_vectors_roundtrip`
- `test_gen1_response_cbor_vectors_roundtrip`
- `test_gen1_invalid_cbor_vectors_fail_decode`
