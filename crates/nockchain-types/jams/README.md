This directory holds golden JAM snapshots captured from the Hoon kernel via the `%balance-by-pubkey` peek.

How to generate
- Ensure your NockApp gRPC private interface is running locally (e.g. `http://127.0.0.1:50051`).
- Use the example helper to capture the peek noun for a pubkey:
  - cargo run -p nockapp-grpc --example dump_balance_peek -- http://127.0.0.1:50051 <PUBKEY_B58> open/crates/nockchain-types/jams/balance_peek/
- Commit the resulting `balance_by_pubkey_<PUBKEY_B58>.jam` files.

What the tests do
- The test `tests/balance_from_peek.rs` walks `jams/balance_peek/*.jam`, cues the noun, unwraps the inner `[height block-id (z-map nname nnote)]`, and verifies that our `Balance` noun-serde decoder can parse the map and that encoding the decoded `Balance` reproduces the original noun (isomorphic roundtrip).

Guidance for coverage
- Add a few peeks for wallets that exercise optional fields:
  - Notes with and without timelocks.
  - Absolute and relative timelock ranges.
  - Coinbases and non-coinbases.
  - Varied locks (different `keys_required`, differing pubkey cardinality).
- The test is skip-friendly: if no peek files exist, it reports and passes. This keeps CI stable, while still allowing high-fidelity coverage when peeks are provided.

