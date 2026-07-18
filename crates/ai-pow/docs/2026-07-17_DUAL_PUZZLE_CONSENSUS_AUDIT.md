# Dual-Puzzle Consensus and Pearl-Compatibility Security Audit

Date: 2026-07-17
Scope: Logos (`0.1.15`), including ZK-PoW/AI-PoW fork choice, independent
ASERT, AI-PoW verification, Pearl merge mining, miner submission, verifier
setup, kernel migration, and the network boundary that admits proof-bearing
blocks.

## Release verdict

No known internally reproducible consensus, work-accounting, merge-mining,
per-attempt-work, deterministic-validation, or remotely triggerable
resource-amplification defect remains in the audited implementation.

The cryptographic verdict remains conditional on an independent audit of the
composite AIR and recursive STARK implementation. That review has not occurred.
This branch therefore has strong internal evidence but MUST NOT be represented
as independently audited or unconditionally production-ready.

## Consensus invariants

1. A `%ai-pow` attempt binds its extranonce before `κ`, matrix commitments,
   noise seeds, noised matmul, tile state, and jackpot. Changing the attempt
   requires fresh noised matmul inference.
2. A certificate binds one block commitment, one target, one canonical opened
   schedule, and one jackpot. A proof cannot be replayed onto another candidate
   or target.
3. One Pearl coinbase contains exactly one Nockchain aux commitment. One Pearl
   proof-of-work cannot authorize two Nockchain block commitments.
4. AI targets are at most `2^256 - 1`; every admitted target retains meaningful
   probability in the 256-bit BLAKE3 jackpot domain.
5. `compute-work-ai(T) == compute-work(T * 2^64)` exactly. An AI target `T`
   and ZK target `T * 2^64` contribute identical expected-work weight.
6. Target and accumulated work are verifier-derived. A block cannot claim an
   easier target or inflated heaviness.
7. Each branch stores its own ZK and AI puzzle counts, heads, and anchors.
   Retargeting depends only on the candidate parent's same-puzzle lineage, not
   block arrival order or the competing puzzle's cadence.
8. Both puzzles use the same global median-time-past clock and max-future bound.
9. AI admission, the AI ASERT phase, the dynamic AI anchor, and the post-AI ZK
   re-anchor share one activation boundary.
10. Missing verifier setup, setup corruption, or a local verifier fault is
    `%fail`/shutdown. Malformed or forged remote input is deterministic `NO`.

## Finding catalogue

| ID | Severity | Exploit or failure | Resolution |
|---|---:|---|---|
| F01 | Critical | The pre-fix attempt context cached nonce-independent noised matmul state, allowing a miner to grind a nonce/hash loop without one fresh inference per attempt. | The extranonce is upstream of `κ`, `H_A/H_B`, `s_A/s_B`, noise, matmul, and jackpot. The miner rebuilds that state per attempt; verifier and circuit bind the same transcript. The maintained invariant and its adversarial coverage are documented in [`SECURITY.md`](SECURITY.md#per-attempt-work). |
| F02 | High | MoE routing validation omitted Pearl's `top_k < experts` and per-expert span bounds, admitting over-routings Pearl rejects and shapes the difficulty model did not price. | Enforce the Pearl bounds before proof verification; adversarial over-routing KATs reject. The maintained invariant is documented in [`SECURITY.md`](SECURITY.md#moe-binding). |
| F03 | Critical | MoE local columns could bleed across expert boundaries, producing fork/grinding divergence from Pearl. | Clamp every expert-local column to `n_e`; malformed and boundary KATs cover the rejection. The maintained invariant is documented in [`SECURITY.md`](SECURITY.md#moe-binding). |
| F04 | Critical | `MAT_UNPACK` accepted a wider value range than Pearl, creating an acceptance-set split. | Route the plain operand through Pearl's int7 `[-64, 64]` range constraint and LogUp frequency checks; range-boundary and recursive-verification KATs reject divergent values. |
| F05 | High | Certificate decoding bounded each atom and node count but not cumulative atom bytes, permitting heap amplification. | `CertificateNounLimits` charges a 64 MiB total atom-byte budget before allocation. Oversize, depth, count, list, nonce, and routing cases reject. |
| F06 | Critical | Aux inclusion used substring containment, so one Pearl coinbase could carry two Nockchain aux tags and reuse one PoW across two commitments. | `verify_pearl_aux_inclusion` requires exactly one tag and exact commitment equality on dense and MoE accept paths (`29ef1eb9`). |
| F07 | High | Pearl serializes MoE `n` as per-expert columns while the circuit uses total columns; the mixed convention rejected valid expert counts and emitted non-Pearl statements. | Preserve `n_e` on the wire and derive total columns with checked multiplication only at the circuit boundary (`6e4cb9cf`). |
| F08 | Critical | Shared/global puzzle lineage state made ASERT depend on fork arrival order and opposite-puzzle gaps, enabling deterministic target disagreement and chain splits. | Persist branch-local counts, heads, and anchors in kernel state 12; follow only the candidate parent's lineage; fail closed when migration cannot reconstruct it (`fe713761`). |
| F09 | Critical | AI ASERT could produce a target outside the 256-bit jackpot domain, where every digest wins and mining becomes free. | Cap all AI ASERT and anchor-bootstrap outputs at `2^256 - 1`; use the equal-weight `2^227` anchor; add saturation KATs (`b202193f`). |
| F10 | High | The global median-time walker dereferenced genesis's nonexistent parent during fresh initialization. | Terminate the walk at genesis and retain one shared median for both puzzles (`b202193f`). |
| F11 | High | An attacker-controlled `trace_height` could cycle more buckets than the LRU cap and force synchronous disk reloads on the consensus thread. | The default cap is all seven supported buckets, pinned by a bucket-coverage KAT. Each bucket can page in at most once; lowering the cap is an explicit trusted-network-only tradeoff (`93e69112`). |
| F12 | High | `catch_unwind` becomes ineffective under `panic=abort`, turning a crafted verifier panic into node termination. | The verifier crate refuses `panic=abort` builds; decode and recursion remain unwind-contained (`68527922`). |
| F13 | High, latent | The test-only checkpoint verifier accepted the prover-embedded Layer-0 program rather than the verifier-derived canonical program. | Require exact canonical-program equality, thread the independently rebuilt program through bridge verification, and reject a mismatched-program KAT (`ffb69903`). |
| F14 | High | Fakenet could configure AI admission and AI ASERT at different heights, splitting admission, anchor, and post-AI ZK cutovers. | Derive one effective boundary and reject conflicting explicit values (`a23ec55b`). |
| F15 | High | Invalid-proof senders were blocked only by peer ID, so peer-ID rotation from one endpoint could repeatedly buy a full cryptographic verification. | `%failed-pow-check` now records Strong abuse against the authenticated connection address and escalates repeated IP behavior; other upgrade-skew liar reasons remain Weak (`852c899e`). |
| F16 | High | Candidate emission and validation used different puzzle targets, and new-heaviest handling did not consistently emit the AI variant. | Build one candidate, derive its AI-targeted variant, emit `%mine-zk` and `%mine-ai`, and reconstruct the identical variant in `do-pow` (`270fb1ad`, `fe713761`). |
| F17 | High | AI and ZK coinbase construction/validation could disagree about the recipient of 20% of a block's new issuance. | Dispatch the recipient from the proven puzzle type in both candidate construction and `check-fund-split`; reject wrong recipient/amount/count (`fff135c3`). |
| F18 | Medium | Stale “MoE fail-closed,” staged verifier, fixed-anchor, 300-second, and deprecated proof-version narratives contradicted live code and could guide a later unsafe change. | Remove obsolete aliases and rollout branches; document the live compact MoE path, mandatory verifier, dynamic anchors, global clock, and 250s/375s regime (`652c2b58`, `3b78b8ee`, `5faf48a7`). |

## Adversarial vectors with no defect found

- **Fork-choice normalization:** algebraic identity and monotonic KATs cover
  equal work at paired targets and less work for easier targets.
- **Forged target/heaviness:** `check-target` and `check-heaviness` recompute both
  values from the validated parent and proven puzzle type.
- **Replay/malleability:** block commitment → aux commitment → Pearl merkle root
  → `κ` → noise seeds → opened schedule/program fold → certificate. Changing any
  bound component rejects.
- **Degenerate matrices:** miner-chosen matrices do not bypass work because
  commitment-keyed noise is applied before every noised matmul and the AIR binds
  the result.
- **Difficulty gaming:** the verifier enforces parameter floors/ceilings,
  checked `h*w*dot` pricing, trace-height equality, tile scheduling, and the
  zero `difficulty_bits` policy.
- **MoE proof binding:** routing root, offsets, expert-local schedule, jackpot,
  cumsum, public inputs, verifier-key digest, and canonical program fold are
  bound. Adversarial routing, column-boundary, schedule, transcript, and
  difficulty vectors found no bypass.
- **Pearl byte compatibility:** dense and MoE commitment, noise, ticket, tile,
  jackpot, aux, plain-proof, and routing encodings are checked against Pearl
  formulas, reference fixtures, and merge-mining KATs.
- **Setup determinism:** little-endian startup enforcement, a committed v0 setup
  digest, cross-process regeneration, checksum-verified bucket files, and
  verifier-owned lookup prevent silent setup divergence.
- **Decode/crash behavior:** cumulative allocation limits, depth/count/list
  limits, trace-height cap, `catch_unwind`, and the panic-strategy build guard
  turn hostile artifacts into bounded deterministic rejection.
- **Timestamp manipulation:** both puzzle ASERTs consume the same BIP113-style
  median-of-11 and max-future rule, so puzzle-specific clocks cannot diverge.
- **Invalid-proof flooding:** gossip/IP token buckets, peer blocking, objective
  failed-PoW address escalation, bounded decode, and all-bucket residency bound
  the per-endpoint amplification. Distributed valid-looking proof verification
  remains the unavoidable cost of a public proof-validating network.

## Verification evidence

- `target/release/roswell test-dumb`: complete dumbnet suite passed after
  rebuilding `assets/roswell.jam`.
- Independent ASERT KATs passed for ZK-heavy and AI-heavy mixed chains,
  fork-locality, lineage gaps, activation anchoring, target dispatch, target
  saturation, equal-work accumulation, and global median time.
- `crates/nockchain/tests/ai_pow_accept_e2e.rs`: the real kernel and mandatory
  jet admitted a valid compact AI block and rejected a wrong-commitment block.
- `cargo test -p ai-pow --features zk --test pearl_compat_fixtures`: 78 passed.
- `cargo test -p ai-pow-miner --lib`: 19 passed.
- AI-PoW all-feature regression gate: 956 passed.
- `cargo test -p nockchain`: 25 passed.
- `cargo test -p nockchain-libp2p-io --lib`: 348 passed, 13 ignored.
- `cargo test --workspace --all-targets`: passed after repairing one unrelated
  stale wallet assertion exposed by the gate.
- Workspace clippy without incompatible mutually-exclusive nockvm features
  passed with `-D warnings`; all-feature AI-PoW clippy passed separately.
- Pearl merge-mining compatibility suite: dense/MoE success paths and aux,
  target, offset, routing, jackpot, nonce, metadata, size, malformed-envelope,
  and one-PoW/two-commitment rejection paths passed.
- `scripts/fakenet-dual-pow-smoke.sh` drove a live node through the exact
  `AI@1 -> ZK@2 -> AI@3` acceptance sequence. All three blocks extended one
  linear heaviest chain; the run emitted no unmatched jet-hint registration.

### Production proof benchmark

`scripts/benchmark-ai-pow-production.sh` reproducibly compiles one release test
binary with `RUSTFLAGS=-C target-cpu=native`, then executes each ignored
production proof fixture in its own `/usr/bin/time -l` process. This isolates
peak resident memory per proof. Prover wall time and serialized compact
certificate size come from the proving path; CPU is direct-process
`user + system`, so it includes fixture and test-process overhead.

Results on Darwin 25.5.0 arm64, Apple M2 Max, with 12-way proving parallelism:

| Fixture | Compact certificate | Prover wall | Process CPU | Peak RSS |
|---|---:|---:|---:|---:|
| Dense: `m=512, k=1024, n=512, r=64, tile=8` | 125,056 B (122.12 KiB) | 28.100 s | 302.52 s | 5,947,998,208 B (5.54 GiB) |
| Canonical MoE miner: `m=64, k=1024, n_e=64, E=2, top_k=1, r=64, tile=8` | 125,764 B (122.82 KiB) | 28.196 s | 299.51 s | 5,956,386,816 B (5.55 GiB) |

The canonical MoE pre-proof attempt path measured 1.6836 ms per attempt
(594 attempts/s) over 200 attempts. Proof generation dominates a successful
attempt's latency. The multi-core CPU totals are expected to exceed wall time.

Other ignored real-proof/setup tests remain opt-in because they generate
multi-gigabyte contexts or take minutes.

## Remaining assurance gate

An independent cryptographic review must cover:

1. composite AIR constraint completeness, including selector exclusivity,
   LogUp bus multiplicities, matrix/noise binding, and selective openings;
2. Fiat–Shamir transcript ordering and challenge observation;
3. FRI/profile soundness and degree-adaptive parameter selection;
4. the P0/D6 canonical-program fold and compact recursion verifier;
5. verifier-key/setup digest coverage of every preprocessed value used during
   acceptance.

This is an assurance prerequisite, not a known exploitable code finding. Until
it is completed, every statement that a certificate proves the committed
inference remains conditional on the internal circuit analysis above.
