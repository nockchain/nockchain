# Open Test Issues For The Compact Recursive Pipeline

This document tracks test work that is still needed after the Plonky3 pin and
AI-PoW API cleanup. It is intentionally scoped to issues that affect the
production compact recursive proof path.

## Current Validation

The current dependency pin is Plonky3 commit
`83ec3062e42a0b556798d22fa0ed0ee09c81c5e1`. This is newer than the previous
`9fc13fa223665268f3bec16ddd43d4b592cd57a1` pin and keeps the LogUp/batch-STARK
API shape used by the vendored recursion workspace. The former local
`p3-batch-stark` patch was removed; Layer 0 now uses upstream
`p3-batch-stark` at this same pin.

Validated so far:

- `RUSTFLAGS="-C target-cpu=native" cargo check -p ai-pow-zk --features recursion`
- `RUSTFLAGS="-C target-cpu=native" cargo check -p ai-pow --features zk`
- `RUSTFLAGS="-C target-cpu=native" cargo check -p ai-pow-miner --features node`
- `cargo test -p ai-pow-zk --features recursion`

The published Plonky3 `v0.5.3` tag was checked during the bump. It is not a
drop-in target for this branch: its lookup and `BaseAir` APIs do not match the
vendored recursion code. Current Plonky3 `main` was also checked, but it uses
`MaybeUninit::assume_init_ref`, which this workspace toolchain does not
currently expose as stable. Move past `83ec3062` only with a toolchain bump or
an upstream-compatible patch.

## Tests Still Needed

1. **Release compact-proof regression after the Plonky3 bump**

   Run the ignored production-size route in release/native mode and record proof
   bytes and wall time:

   ```sh
   RUSTFLAGS="-C target-cpu=native" cargo test -p ai-pow-miner --release --features node \
     real_compact_pearl_merge_artifact_jam_size_for_selected_route -- --ignored --nocapture
   ```

   This should confirm the compact recursive certificate remains within the
   relaxed `~150kb` artifact budget and close to the `~30s` proving target after
   the Plonky3 dependency movement.

2. **Full compact verify round trip against verifier-owned context**

   Add or run a release test that builds a compact Pearl-compatible artifact,
   cues it through the noun boundary, verifies it with verifier-owned setup, and
   rejects the same artifact under a wrong verifier-key digest. This is the most
   important end-to-end soundness regression for the current public API.

3. **Prover-cache equivalence and stale-cache rejection**

   Exercise `prove_pearl_merge_compact_recursive_certificate_with_prover_cache`
   and `AiPowCompactRecursiveCertificateRun::into_prover_cache` across at least
   two attempts with identical shape. The test should prove that warmed-cache
   output verifies and that stale L1 metadata or changed FRI shape rejects before
   proof acceptance.

4. **Serial/parallel proof-byte equivalence**

   The default proof path enables Plonky3/Rayon parallelism. Add a focused
   regression that proves the same small chain-verified statement with default
   features and with `--no-default-features` where practical, then compares the
   encoded compact certificate bytes or at least the verifier result and setup
   digest. This guards the claim that parallelism changes only runtime.

5. **Dependency-forward compatibility check**

   Track the next Plonky3 commit after `83ec3062` that builds on the workspace
   Rust toolchain. When the toolchain supports the newer `MaybeUninit` API, test
   current Plonky3 `main` again and rerun the release compact-proof regression
   before advancing the pin.

6. **Miner submission boundary regression**

   Add a run-loop-level test around `PearlMergeSubmissionConfig::new_compact_recursive`
   proving that the recursive certificate builder runs only after a Nockchain
   target hit and that synthetic proof injection remains test-only. This protects
   the simplified constructor API.
