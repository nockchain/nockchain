# `roswell`

`roswell` is the executable harness for Nockchain Hoon tests, proof conformance, and kernel/jet integration. It boots `assets/roswell.jam` as an ephemeral or persistent NockApp and exposes commands for test suites, puzzle proofs, snapshots, stream windows, and proof verification.

## Place in the system

Roswell is not a full node and does not participate in peer consensus. It provides a controlled host in which the public Hoon test kernel runs with the same proof and AI-PoW jet registry used by `nockchain`.

This makes it the boundary test for three layers at once:

1. Hoon source compiled into `roswell.jam`;
2. NockVM execution and noun effects; and
3. native jet registration and equivalence.

## Maintained invariants

- `produce_full_hot_state` is the shared consensus jet set: Nock ZKVM prover/verifier jets plus the mandatory AI-PoW verifier jet.
- Hoon effects are decoded into explicit file and exit actions. A suite succeeds only when the kernel reports successful completion.
- Proof commands validate puzzle length and proof version before constructing kernel nouns.
- Generated proof files, snapshots, and stream windows preserve canonical JAM encoding.
- Ephemeral test runs do not reuse persistent state unless requested.
- A clean Hoon compile is not sufficient evidence that a jet hint resolves. Runtime tests must execute the hinted arm and observe the jetted behavior.

## Soundness role

Roswell supplies conformance evidence; it is not a cryptographic assumption. Its value depends on using the same jam, hot-state entries, noun encodings, and verifier setup rules as production. Tests that accidentally run an unjetted Hoon fallback or a stale embedded jam do not validate the production path.

Cryptographic acceptance remains dependent on the Nock proof system, AI composite AIR/recursion, hashes, and canonical statement binding described by their owning crates.

## Build and run

```sh
make assets/roswell.jam
cargo run --release -p roswell -- --new --ephemeral run-suite
cargo test -p roswell
```

Whenever a `.hoon` source used by Roswell changes, rebuild `assets/roswell.jam` before rebuilding or running the Rust binary.
