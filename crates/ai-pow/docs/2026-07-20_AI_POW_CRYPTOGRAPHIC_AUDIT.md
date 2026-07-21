# AI-PoW cryptographic audit

Date: 2026-07-20
Scope: consensus-reachable AI-PoW puzzle at `logos-integration-squashed` commit `76918aae1d4b52795a78141851286120da730c3d`.

## Release verdict

**NO-SHIP for adversarial blockchain deployment without independent cryptographic review.**

No internally reproducible cryptographic bypass was found in the production AI-PoW route. The audited source supports the core claim that a valid `%ai-pow` block artifact binds one block commitment, one target, one Pearl-compatible work transcript, one verifier-derived Layer-0 program, and one compact recursive certificate under the committed verifier-setup table.

The release verdict remains **NO-SHIP** because the remaining claims are assurance-critical and not replaceable by this internal source audit:

1. the configured proof margin is a nominal FRI query-parameter ledger, not a proven probability bound for this AIR/LogUp/recursive stack; recorded maintainer acceptance and independent review are still required;
2. the composite AIR, LogUp multiplicities, compact recursion, and setup digest coverage still require independent adversarial review;
3. production-route compact-certificate mutation evidence contains ignored opt-in proof tests and lower-level always-on tests, not a cheap always-on mutation matrix through every full block-artifact boundary.

This report is internal source-audit evidence. It is not third-party sign-off.

## Audited system freeze

| Item | Audited value |
|---|---|
| Repository HEAD | `76918aae1d4b52795a78141851286120da730c3d` |
| Branch | `logos-integration-squashed` |
| Worktree at freeze | clean |
| Plonky3 revision | `11cc5849a1b57a2f520d6edc608b9e516517d841` for `p3-*` 0.6.2 crates (`Cargo.lock:7524-7526`) |
| Plonky3-recursion revision | `a82732eec8228e8459fdc5be4b8ff5dcadbd75e3` for `p3-recursion`, `p3-circuit`, and related crates (`Cargo.lock:7624-7626`) |
| Planned Pearl source revision | `08e1eb83123faba0bcdb6b36825fac9868ec929b`; not present in the local Pearl checkout, so exact planned-commit comparison used raw GitHub source |
| Local Pearl source checkout | `/Users/loganallen/Dev/ai-pow/pearl` at `f8804af6d0a4d951f0e8576e170c0ca28f304d9d`, with one untracked `.DS_Store` |
| Non-source Pearl path | `/Users/loganallen/Dev/Gemma-4-31B-it-pearl`, a HuggingFace model checkout, not Pearl source |
| v0 verifier-setup table digest | `1bffbf7c8a390aec6f04a58e30de2ce4b2fca71488e48347b6602a0776019471` (`crates/ai-pow-jets/src/table_digest.rs:54-61`) |
| Admitted setup shape keys | `(8192,true)`, `(16384,false)`, `(16384,true)`, `(32768,false)`, `(32768,true)`, `(65536,false)`, `(65536,true)`, `(131072,false)`, `(131072,true)`, `(262144,false)`, `(262144,true)`, `(524288,false)`, `(524288,true)` |

The selected production route is Hoon `%ai-pow` artifact → mandatory `~/ %ai-pow-verify` jet → `ai_pow_jets::ai_pow_verify_core` → `ai_pow_miner::certificate_noun::verify_ai_pow_block_artifact` → compact recursive certificate verification. Raw Layer-0 proofs, `MatmulProof`, Pearl plain proofs, and non-compact checkpoint constructors are diagnostic or regression paths, not consensus block artifacts (`hoon/common/pow.hoon:5-23`; `crates/ai-pow-jets/src/lib.rs:522-574`; `crates/ai-pow-miner/src/certificate_noun.rs:1-12`, `2889-2960`).

## Core invariants supported by the audit

1. **Attempt binding.** The attempt nonce is upstream of `κ`, matrix commitments, noise seeds, noised matmul, tile state, and jackpot. Reusing work across a different nonce, block commitment, matrix root, or target rejects before acceptance (`crates/ai-pow/src/fiat_shamir.rs:62-173`; `crates/ai-pow/src/pearl_compat.rs:2890-2917`; `crates/ai-pow-miner/src/certificate_noun.rs:1971-2055`, `2374-2448`).
2. **Pearl-compatible transcript.** Dense transcript order, endian conventions, keyed matrix commitments, PRNG labels, tile loop, MoE public data layout, target comparison, and aux commitment binding are locally fixture-backed and were compared against Pearl `08e1eb83123faba0bcdb6b36825fac9868ec929b` source files: [`proof_utils.rs`](https://github.com/pearl-research-labs/pearl/blob/08e1eb83123faba0bcdb6b36825fac9868ec929b/zk-pow/src/api/proof_utils.rs), [`sanity_checks.rs`](https://github.com/pearl-research-labs/pearl/blob/08e1eb83123faba0bcdb6b36825fac9868ec929b/zk-pow/src/api/sanity_checks.rs), [`mine.rs`](https://github.com/pearl-research-labs/pearl/blob/08e1eb83123faba0bcdb6b36825fac9868ec929b/zk-pow/src/ffi/mine.rs), and [`build_routing_data.cu`](https://github.com/pearl-research-labs/pearl/blob/08e1eb83123faba0bcdb6b36825fac9868ec929b/miner/pearl-gemm/csrc/moe/build_routing_data.cu). Nockchain is narrower where it caps routing data, differs from Pearl CUDA miner limits, or permits duplicate non-opened top-k routing entries that do not reduce opened-tile work (`crates/ai-pow/src/pearl_compat.rs:243-290`, `518-640`, `731-940`, `1226-1248`, `1473-1524`, `1935-1944`, `2388-2468`, `2890-2917`).
3. **Committed-matrix policy.** Arbitrary miner-committed matrices are permitted. The proof statement binds the submitted `H_A/H_B` and keyed jackpot work; it does not prove model provenance, usefulness, or uniqueness (`crates/ai-pow-miner/src/certificate_noun.rs:2899-2945`).
4. **Layer-0 program pin.** Production verification uses `CompositeFullAirWithLookupsPinned`, and the verifier rebuilds the canonical program from trusted parameters and schedule data. `PROGRAM_COLS` are unconditionally pinned to preprocessed verifier data on every row (`crates/ai-pow-zk/src/composite_full_air.rs:120-145`, `277-302`; `crates/ai-pow-zk/src/composite_proof.rs:412-433`; `crates/ai-pow/src/zk_bridge.rs:2655-2679`).
5. **Public-input ownership.** `HASH_A`, `HASH_B`, `JOB_KEY`, and `COMMITMENT_HASH` are verifier/bridge-owned. `HASH_JACKPOT` is proof-produced inside Layer 0, then equality-bound at the production artifact boundary to the authenticated Pearl statement and target-checked. `CUMSUM_TILE` and matrix-free `JACKPOT_MSG` remain proof outputs constrained inside the AIR, not externally verifier-owned values (`crates/ai-pow-zk/src/composite_public.rs:61-94`, `170-249`; `crates/ai-pow-zk/src/composite_full_air.rs:716-749`, `826-831`; `crates/ai-pow/src/zk_bridge.rs:2610-2648`; `crates/ai-pow-miner/src/certificate_noun.rs:2015-2021`, `2058-2063`, `2411-2420`, `2638-2641`).
6. **Matrix/noise binding.** C3 binds matrix bytes to BLAKE3 message words; `InputChip`, i8/u8 LogUp, `BUS_NOISED_PACKED`, and positioned chunk IDs bind noised matmul operands back to the committed strip roots (`crates/ai-pow-zk/src/composite_full_air.rs:603-655`, `779-839`; `crates/ai-pow-zk/src/composite_full_air_with_lookups.rs:324-495`; `crates/ai-pow-zk/src/composite_trace.rs:90-121`).
7. **Fold/jackpot binding.** SX and R-b predecessor keystones bind fold input to the selected stripe/reducer state, and the final row binds `JACKPOT_MSG` to `FOLD_STATE` before keyed BLAKE3 target checking (`crates/ai-pow-zk/src/composite_full_air.rs:303-389`; `crates/ai-pow/src/zk_bridge.rs:2645-2648`).
8. **Setup authority.** Nockchain strict boot installs a seed table matching `AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST` or shuts down. Verification only pages committed buckets after checksum, shape-key, and digest validation; remote proofs cannot trigger setup generation (`crates/nockchain/src/main.rs:61-78`; `crates/ai-pow-jets/src/setup.rs:430-586`; `crates/ai-pow-jets/src/lib.rs:241-354`).
9. **Malformed input behavior.** Once an artifact noun reaches the jet, malformed artifacts, non-atom targets, unsupported setup shapes, verifier errors, and recursion panics reject as Hoon loobean `NO`; oversized target atoms saturate to the 256-bit maximum, and certificate/target mismatches reject in the work check. Missing/corrupt local setup reaches `%fail`, and strict boot setup-install failure is fatal before block processing. Whole-block/network ingestion before `+check-pow` is outside this source-audit boundary (`crates/ai-pow-jets/src/lib.rs:23-37`, `454-460`, `488-578`; `crates/nockchain/src/main.rs:61-78`; `hoon/common/tx-engine-1.hoon:90-129`).

## Proof-system margin

Production proof acceptance has three proof layers:

| Layer | Production profile | Query-bit accounting |
|---|---|---:|
| L0 composite batch-STARK, small trace buckets | `log_blowup=4`, `num_queries=15`, proof-system PoW `0` | 60 |
| L0 composite batch-STARK, larger trace buckets | `log_blowup=2`, `num_queries=30`, proof-system PoW `0` | 60 |
| L1 recursive-verifier outer proof | `log_blowup=3`, `num_queries=20`, proof-system PoW `0` | 60 |
| L2 compact final proof | `log_blowup=5`, `num_queries=12`, proof-system PoW `0` | 60 |

The source-derived per-layer expression is

$$s_i = \text{log\_blowup}_i \times \text{num\_queries}_i + \text{commit\_pow}_i + \text{query\_pow}_i.$$

All production proof-system PoW terms are zero. These 60-bit entries are nominal query-parameter values. If independent review establishes that each accepted proof layer has error probability at most `2^-60` for this concrete AIR/LogUp/batch/recursive stack, then a union over three layers gives the following nominal erosion ledger:

| Scope | Nominal query-parameter ledger |
|---|---:|
| One accepted certificate | `60 - log2(3) = 58.415037` bits |
| `10^6` adversarial certificates | `60 - log2(3 * 10^6) = 38.483469` bits |
| Ten years at 150 s/block (`2,103,840` accepted blocks) | `60 - log2(3 * 2,103,840) = 37.410444` bits |

These figures are repeated-submission union-accounting numbers, not attack costs and not a proven probability upper bound. Plonky3 treats the simple `log_blowup * num_queries + query_pow` product as a conjectured estimate; batching, with-replacement query collisions, duplicate indices, AIR degree/count terms, and lookup composition remain theorem obligations for independent review.

This recomputes the release ledger’s “about 58.4 bits” entry as a nominal one-certificate figure (`crates/ai-pow/docs/2026-07-18_PRODUCTION_RELEASE_ASSURANCE.md:45-47`). The regression test floors the same accounting by subtracting two bits and expecting 58 (`crates/ai-pow-zk/src/recursion.rs:3197-3208`).

No production verifier setting below the 60-bit per-layer claim was found:

- Layer 0 proof and verification derive `CircuitConfig::for_layer0_trace(trace_height)` from verifier-bound trace height (`crates/ai-pow/src/zk_bridge.rs:572-604`, `2639-2688`, `3301-3333`).
- The Layer 1 circuit verifies Layer 0 with `FriVerifierParams::with_mmcs` and the profile query count (`crates/ai-pow-zk/src/recursion.rs:984-1039`).
- Layer 2 compact verification validates verifier-owned setup binding and exact `compact_batch_l2_fri_shape()` equality before verifying the compact proof body (`crates/ai-pow-zk/src/recursion.rs:262-298`, `2519-2533`).
- The test-only Plonky3-recursion arithmetic constructor that can disable MMCS was not found in AI-PoW production callsites.

A proven end-to-end Plonky3 soundness upper bound is not derivable from source constants alone. The local Plonky3 security model requires actual AIR constraint counts, max degree, max out-of-domain combination, collision-resistance terms, lookup/batch composition treatment, and theorem-hypothesis acceptance for this recursive stack. AI-PoW security docs already state that configured FRI query bits are not by themselves a complete end-to-end proof (`crates/ai-pow-zk/docs/SECURITY.md:11-22`).

## Finding and assurance ledger

| ID | Classification | Severity | Finding | Disposition |
|---|---|---:|---|---|
| CA-01 | Non-finding | N/A | No production shortcut was found for replay across different `sigma`/`mu`/roots, nonce/header attempts, parameter overflow, or one-work/two-chain replay. Same-transcript Pearl-valid offset scanning is permitted and still requires evaluating the explicit ticket/tile attempt; low-rank reassociation and stripe omission were rejected by order-dependent fold/XOR operation analysis, not solely by KAT coverage. | Transcript, target, aux, and matrix-policy KATs passed; low-rank/stripe conclusions remain analytical audit evidence under the BLAKE3 random-oracle/PRF model. |
| CA-02 | Non-finding | N/A | Arbitrary committed matrices are valid by design. The proof statement prices and proves committed work, not model provenance or usefulness. | Keep public claims scoped to committed work. Do not market canonical model inference as consensus-proven. |
| CA-03 | Non-finding | N/A | No production Route-A unowned selector/store/lookup gap was found. Selector suppression is blocked by pinned `PROGRAM_COLS`; matrix substitution is blocked by C3/Input/i8u8/noised-packed routing; fold/jackpot forgery is blocked by SX/R-b and final-boundary keystones. | Route-A, LogUp, and precheck gates passed in this audit pass. |
| CA-04 | Assurance gap | Release-gating | The 58.4-bit figure is a nominal three-layer FRI query-parameter ledger, not a full theorem-instantiated STARK soundness proof for AI-PoW’s concrete AIR/LogUp/recursive system. | Keep the release ledger open pending recorded maintainer acceptance of this residual margin and independent cryptographic review. |
| CA-05 | Assurance gap | Medium | Always-on tests do not provide a cheap full mutation matrix through `verify_ai_pow_block_artifact` for every compact certificate field. Some deepest compact-route checks are ignored opt-in proof tests or lower-level always-on tests. | Treat full compact-route mutation coverage as release evidence only after the opt-in commands run at the audited HEAD. |
| CA-06 | Assurance gap | Low | Direct `IRANGE7P1_FREQ` and `IRANGE8_FREQ` table-frequency mutation tests were not found. Out-of-range query and property tests cover those buses, and other bus frequencies have explicit mutation tests. | Add direct frequency tamper KATs if this report becomes a release evidence baseline. |
| CA-07 | Documentation divergence | Informational | `BUS_STARK_ROW_IDX` is listed in `ALL_BUSES` but is not emitted as a standalone interaction; `cv_routing` embeds `STARK_ROW_IDX` directly in its key. | Do not cite `BUS_STARK_ROW_IDX` as a separate LogUp channel. |
| CA-08 | Documentation divergence | Informational | Some Hoon/Rust comments still describe verifier re-derivation of canonical `(A,B)` matrices. Production compact verification intentionally accepts arbitrary miner-committed matrices bound by `H_A/H_B`. | Update stale comments before public release materials rely on them. |
| CA-09 | Compatibility/provenance gap | Informational | The exact planned Pearl source commit was not present in the local Pearl source checkout. Raw GitHub source at the planned commit was used for exact comparison. Known MoE qualifications remain: Nockchain carries/caps routing data in the artifact, Pearl CUDA routing caps `num_experts <= 256` while verifier-side formats allow more, and duplicate non-opened top-k routing entries are not globally rejected by either inspected verifier path. | Record Pearl upstream commit provenance in fixture/source metadata before release sign-off and keep MoE compatibility claims scoped to the compared `08e1eb...` source files. |
| CA-10 | Policy risk | Informational | Parser saturation only begins when the raw target atom is `>=2^256`, but jackpot difficulty compares against `target * h * w * dot` with saturating 256-bit multiplication. Every jackpot wins whenever that adjusted product reaches `2^256-1`, even if the raw base target is lower. | Keep AI ASERT/admission policy factor-aware: either explicitly accept all-winning adjusted-target regimes or cap base targets so `target * h * w * dot < 2^256-1` for admitted shapes. The target parser KAT pins exact raw-atom parsing and oversized saturation only (`crates/ai-pow-jets/src/lib.rs:914-940`; `crates/ai-pow/src/pearl_compat.rs:1201-1225`, `1380-1422`). |

## Verification evidence from this audit pass

Commands below ran in the worktree based on the audited HEAD. The target-parser KAT is the only code added during this audit pass.

| Area | Command | Result | Raw output |
|---|---|---|---|
| Setup shape coverage | `cargo test --locked -p ai-pow-jets production_verifier_setup_buckets_cover_the_capped_band -- --nocapture`; `cargo test --locked -p ai-pow-jets print_production_bucket_shapes -- --nocapture` | pass; 13 admitted shape keys printed | `artifact://4` |
| Pearl transcript and shortcut KATs | `cargo test --locked -p ai-pow --features zk --test pearl_compat_fixtures -- --nocapture`; `cargo test --locked -p ai-pow --test pearl_merge_compat -- --nocapture`; `cargo test --locked -p ai-pow --test pearl_moe_routing_binding -- --nocapture`; `cargo test --locked -p ai-pow --test pearl_moe_tile -- --nocapture`; `cargo test --locked -p ai-pow --test pearl_moe_work_precheck -- --nocapture`; `cargo test --locked -p ai-pow --test soundness_sim -- --nocapture` | pass; 11 + 53 + 21 + 3 + 4 + 3 tests | `artifact://18` |
| Jet malformed artifact and target parser KATs | `cargo test --locked -p ai-pow-jets target_atom_to_32_saturates_only_oversized_targets -- --nocapture`; `cargo test --locked -p ai-pow-jets malformed_ai_pow_artifact_is_rejected_at_decode -- --nocapture` | pass; 1 + 1 tests | `artifact://44` |
| Route-A, LogUp, compact accounting, miner precheck | `cargo test --locked -p ai-pow-zk --features recursion routea_ -- --nocapture`; `cargo test --locked -p ai-pow-zk --lib composite_full_air_with_lookups::tests -- --nocapture`; `cargo test --locked -p ai-pow-zk --features recursion compact_batch_ -- --nocapture`; `cargo test --locked -p ai-pow-miner --features node pearl_merge_artifact_precheck_rejects_replay_and_certificate_mismatch -- --nocapture` | pass; 13 + 47 + 8 + 1 tests | `artifact://46` |

Source-inspection subaudits also covered Layer-0 LogUp/selector wiring, mutation coverage, proof-margin derivation, and consensus verifier boundary behavior. Those subaudits ran no project-wide suites and produced no code changes.

## Residual gates before any ship recommendation

1. Independent cryptographic review of `CompositeFullAirWithLookupsPinned`, every LogUp bus multiplicity, C3/Input/i8u8/noised-packed matrix binding, SX/R-b fold keystones, and final jackpot hash binding.
2. Independent review of the L0/L1/L2 FRI parameterization against the exact Plonky3 and Plonky3-recursion revisions, including AIR-shape, batching, duplicate-query, lookup-composition, and theorem-hypothesis terms.
3. Recorded maintainer acceptance of the nominal `58.415` one-certificate ledger and its repeated-submission erosion before any production security claim relies on it.
4. Full production-route compact-certificate adversarial matrix through `verify_ai_pow_block_artifact` / `ai_pow_verify_core`: wrong target, nonce, `found_idx`, trace height, verifier-key digest, compact proof bytes, MoE routing, outer indices, roots, public inputs, and setup digest.
5. Live Hoon/jet admission gates at the audited HEAD after serial jam rebuild where needed: valid `%ai-pow` block admission and malformed `%ai-pow` artifact rejection through `crates/nockchain/tests/ai_pow_accept_e2e.rs`.
6. Whole-block/network ingestion resource review before the noun reaches `+check-pow`, including page-size accounting and hostile block transport limits.
7. Opt-in ignored proof gates at the audited HEAD for compact recursive round trips and production-size compact artifacts, with raw logs attached to the release ledger.
8. Stale public/durable comments corrected wherever they imply canonical `(A,B)` matrices or overclaim a standalone `BUS_STARK_ROW_IDX` channel.
9. Pearl fixture/source provenance recorded at the exact upstream commit used for future compatibility claims.

Until those gates close, the maintained release ledger’s `NO-SHIP` status remains the correct release state.
