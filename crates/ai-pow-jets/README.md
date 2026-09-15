# `ai-pow-jets`

`ai-pow-jets` is the native consensus verifier for the `%ai-pow` block puzzle. The Hoon kernel owns puzzle dispatch, target calculation, and block admission; its `%ai-pow-verify` arm is intentionally a stub whose mandatory jet is implemented here.

## Place in the system

```text
candidate block + AI artifact + AI target
                    |
             Hoon check-pow
                    |
        mandatory %ai-pow-verify jet
                    |
 bounded noun decode and statement derivation
                    |
 verifier-owned setup selected by trace height
                    |
       compact recursive-STARK verification
                    |
        loobean YES or deterministic NO
```

The jet is registered in both `nockchain` and `roswell`. `ai-pow-miner` defines the artifact noun and verifier precheck, `ai-pow-zk` verifies the compact certificate, and `ai-pow` defines the Pearl-compatible work statement.

## Maintained invariants

- Hoon supplies the structured block commitment and consensus-computed target. The jet canonical-jams the commitment exactly as the miner does.
- Attacker-controlled nouns are decoded under depth, node-count, atom-size, and cumulative-byte limits before expensive verification.
- The public statement, canonical opened schedule, trace height, and setup bucket are derived from block-bound data. They are not accepted from the prover.
- The certificate's verifier-key digest must match the verifier-owned setup digest.
- Every reachable production trace height has one committed setup bucket. Missing buckets in the committed table make a block invalid; failure to load an existing local bucket is a node fault, not a consensus vote.
- Targets are interpreted as little-endian 256-bit values. A larger Hoon target is saturated to `2^256 - 1`, which is acceptance-equivalent for a 256-bit jackpot.
- Decode and recursive-verifier panics are caught and converted to deterministic rejection. The crate refuses to compile with `panic = "abort"` because that would make a malformed block process-fatal.
- The mandatory jet has no independent Hoon acceptance fallback. A node without the registered jet cannot validate `%ai-pow` blocks.

## Soundness and security dependencies

Consensus safety relies on:

- the `ai-pow` statement and the `ai-pow-zk` composite AIR being complete;
- FRI and Fiat-Shamir soundness for the selected proof profiles;
- collision and preimage resistance of BLAKE3 and the assumed security of Tip5 where each is used;
- canonical JAM and certificate encodings;
- setup generation being deterministic and the committed verifier-key digests covering every verifier-used parameter;
- the Hoon kernel independently enforcing activation, puzzle-specific ASERT, exact target equality, fork-choice work, and coinbase rules.

This crate reduces malformed-input and setup-divergence risks; it does not replace an independent cryptographic review of the composite AIR and recursion stack. See [`../ai-pow/docs/2026-07-17_DUAL_PUZZLE_CONSENSUS_AUDIT.md`](../ai-pow/docs/2026-07-17_DUAL_PUZZLE_CONSENSUS_AUDIT.md).

## Verifier setup

Production nodes build and validate the complete setup table at boot, store heavy contexts on disk, and retain a bounded LRU in memory. Verification may page in a prebuilt context but never rebuilds a circuit in response to an untrusted block. See [`docs/VERIFIER_SETUP.md`](docs/VERIFIER_SETUP.md).

## Validation

```sh
cargo test -p ai-pow-jets
cargo test -p ai-pow-jets --all-features
cargo test -p roswell
```

Hoon or jet-registration changes additionally require rebuilding `assets/roswell.jam` before rebuilding and running `roswell`.
