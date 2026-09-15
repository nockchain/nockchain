# Pearl V3 AI-PoW Cutover Plan

## Scope

Logos AI-PoW uses Pearl certificate V3 salted noise seeds at Pearl commit `fc5ca65a1df0fad0140e74c3b52e71c4a0f99e90`. Every dense and MoE attempt, recursive certificate, verifier, canonical miner job, and Pearl Gateway request uses V3. Nockchain keeps its `%ai-pow` artifact, compact recursive certificate format, `%mine-ai` candidate version `%4`, and `AI_POW_CERT_VERSION = 1`.

## Stage 1: Reference vectors

1. Create a detached worktree from the pinned Pearl commit. Do not modify `./pearl`.
2. Run Pearl seed tests. Prove the pinned salt bytes equal the BLAKE3 domain strings.
3. Add fixed dense and MoE vectors. Include a Logos-sized MoE vector that distinguishes per-expert `n_e` from stacked B width and binds A before routing.
4. Remove accepted V2 fixture vectors. Keep V2 calculations only for negative tests.
5. Run Pearl seed tests and Logos fixture tests.
6. Commit the tested stage as `test(ai-pow): pin Pearl V3 salted-seed vectors`.

## Stage 2: Single V3 work statement

1. Add private root binding for raw 32-byte roots and authenticated little-endian dimensions. The binding uses a zeroed 64-byte input, root bytes at `0..32`, and dimension bytes at `32..36`.
2. Derive dense seeds from bound A and B roots. Derive MoE seeds from bound roots, per-expert `n_e`, and canonical routing data or a verified public routing root.
3. Delete public raw-root V2 seed helpers. Keep raw matrix commitments as authenticated public inputs.
4. Use unambiguous dense and MoE work-commitment constructors. Migrate prover, verifier, recursive bridge, miner, jets, and test callers.
5. Extend the independent `ai-pow-zk` MoE reference implementation. Compare full roots, routing commitment, and seeds across crates.
6. Keep the 60-element Layer-0 public-input layout, compact recursive certificate APIs, Nockchain wire versions, and verifier setup identifiers.
7. Add negative V2-equivalent proof rejection and dimension, root, routing, offset, and public-parameter mutation tests. Refresh the dense miner snapshot from the V3 transcript.
8. Update active protocol documentation. Preserve the historical V2 incident formula in the report.
9. Run focused crate tests, real dense and MoE recursive proofs, and setup-independence tests.
10. Commit the tested stage as `feat(ai-pow): adopt Pearl V3 salted noise seeds`.

## Stage 3: Gateway boundary

1. Require an inbound numeric `cert_version` field. Convert only wire value `3` into a zero-sized `PearlCertificateV3` marker.
2. Reject missing, malformed, V1, V2, and unknown certificate versions before search. Use a dedicated unsupported-version error.
3. Submit a resolved job, not loose header and target values. Revalidate the header binding and serialize the exact header, target, and numeric `cert_version: 3`.
4. Retain only Pearl's post-MoE bincode 1.3.3 `PlainProof` layout. Remove legacy V1 payload paths and feature gates.
5. Add unit tests for version rejection and exact successful wire values. Add the ignored real Pearl Gateway process test and Python reference fixture.
6. Run Gateway unit tests, Pearl Gateway Python tests, and the direct process integration.
7. Commit the tested stage as `feat(ai-pow-miner): require Pearl V3 gateway jobs`.

## Stage 4: Regression gates

1. Run focused KAT and wrong-derivation tests.
2. Run all affected Rust crates, the release workspace suite, real recursive proof tests, setup-independence, and Nockchain admission tests.
3. Run Roswell without rebuilding a jam unless a Hoon source file changes.
4. Run the pinned Pearl Rust and Go seed, wire-dispatch, and hardfork tests.
5. Run the direct Gateway integration, fakenet AI-PoW and dual-PoW smoke tests, and the production proof benchmark.
6. Run formatting, clippy, and stale-symbol checks. Inspect active certificate-version and formula uses.

## Invariants

- The Pearl V3 seed formula has no runtime or wire-selectable V1 or V2 path.
- `m` authenticates A. `n` authenticates dense B. `n_e` authenticates each MoE expert B width.
- The total stacked MoE B width is invalid at the seed-derivation boundary.
- The recursive verifier context remains proof-independent and shape-bound.
- A Gateway job cannot begin a search unless its certificate version is exactly 3.
- Pearl certificate version is a Gateway job property. It is not a Nockchain certificate-noun version.
