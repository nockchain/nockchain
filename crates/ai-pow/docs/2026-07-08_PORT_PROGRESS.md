# AI-PoW port onto master — progress log

**Branch:** `claude/ai-pow-integration-squashed` (checkpoint `d40fcc26` on
`origin/master` 6444e5c7). Goal: full port — all crates + hoon kernels build, all
tests pass (incl. roswell). Driven in validated stages; this log updated as I go.

**Reconciliation principle:** for each subsystem the branch predates, keep the
version consistent with its *consumers*. tip5 → master's (master's zkvm-jetpack +
nockchain-math consume it). ASERT/consensus types → branch's (the consensus **hoon**
+ noun layout consume them). Verify each by building the consumer.

## Stages

- [x] S1. Foundational crates build: `nockchain-math` (tip5→master), `nockchain-types`
  (ASERT→branch's `AsertParams`; **added `NounDecode` for `BlockchainConstants`/
  `AsertParams`** — my branch only impl'd encode; master's bridge needs decode).
- [x] S2. `zkvm-jetpack` builds (master's f5b7f3cb consumer).
- [x] S3. ai-pow stack builds — the only tip5 adaptation needed was **adding
  `permute_5round` + `NUM_ROUNDS_5ROUND` to master's tip5** (5-round variant, master
  Montgomery domain, byte-identical to branch's since it's the same round fn).
- [x] S4. `nockchain` + kernel *crates* build (Rust). Fixed unused-import lint
  errors (deny-warnings) in `nockapp-grpc`, `nockchain/config.rs`.
- [~] S5. **CORRECTION (R1, no fake completion):** the earlier claim that the cargo
  build rebuilds the jams was WRONG. Every kernel crate is
  `include_bytes!(assets/X.jam)` — cargo re-embeds a **pre-built** jam; it never runs
  hoonc. The on-disk jams were stale (`dumb`/`miner` dated Jun 26, before this port's
  hoon reconciliation) or empty (`bridge`/`peek` = 0 bytes → the `bitvec TooLong`
  bridge-kernel boot panic). Rebuilding all jams via hoonc is the real validation of
  "hoon kernels build properly".

  **Stark reconciliation (soundness-critical, resolved with user guidance).** hoonc
  surfaced a broken auto-merge: `prover.hoon`/`nock-prover.hoon` were left as a
  master≈+1-line mix while other stark files were branch's → `mint-lost`. Root cause,
  established via merge-base diffs: the **branch barely touched the hoon stark**
  (prover.hoon +1 line, nock-prover.hoon +0, verifier.hoon +2) — that +1/+2 is the
  ai-pow **`%3` proof-version** (`proof-version` in `ztd/four.hoon` = `?(%3 %2 %1 %0)`;
  the zk prover/verifier `!!` on v3 since the AI proof is produced/verified in Rust).
  **Master** did the big refactor (prover 258+/52−) **and added the proof-snapshot /
  proof-stream family** (a coworker's genuinely-needed but untyped work). So neither
  side alone is right — the correct result is a **merge**: master's stark as baseline
  (keeps the refactor + proof-snapshot) **+** re-apply the branch's `%3`.
  - `prover.hoon`, `nock-prover.hoon`, `verifier.hoon`, `nock-verifier.hoon` ← master.
  - `%3` re-applied to the switches that dispatch the **full** `proof-version` (not the
    `prover-input`-narrowed `?(%0 %1 %2)` table switches): 2 in prover
    (assemble-continuation final, assemble-proof-stream construction) + 2 in verifier.
  - Per user directive, **added the missing `^-` return types** to nock-prover's
    proof-snapshot wrappers (`snapshot`→`proof-snapshot`,
    `make-proof-stream-window`→`proof-stream-window-result`, `assemble-proof-stream` /
    `assemble-proof-continuation`→`prove-result`) — the untyped versions inferred
    `(unit *)` and caused peek's `nest-fail`.
  - **dumb.jam rebuilds (19M)** with this merged stark → consensus/mining sound.
  - Jams rebuilt + verified: **dumb (19M), miner (17M), bridge (19M), wal (19M)**.
    `roswell` + `peek` still fail — see residuals.

### Kernel-jam residuals (need direction)

- **roswell.jam** — COMPILES ✅; hoon test suite partially runtime-correct.
  roswell is a master-only NockApp that compiles master's entire `hoon/tests/` suite
  (53 files; the branch had no `hoon/tests` dir). Divergence: branch's consensus AND
  miner doors added a `d=derived-state` slot (`|_ [c=consensus-state d=derived-state
  =blockchain-constants]`), but master's tests call them with only `[c bc]`. Fixed by
  threading `der` as a read-only 2nd arg at **386 `dcon`/`dmin` call sites across 8
  test files** (a subagent did the grind under strict guardrails — only `hoon/tests/`
  touched), plus regrouping the flat `asert-*` blockchain-constants fields into the
  `zk-asert` AsertParams struct (`asert-phase` → `phase.zk-asert`, matching production
  in `miner.hoon`). **`roswell.jam` now rebuilds fresh (25M).** So all 6 hoon kernels
  build.
  - **Runtime status — RESOLVED ✅.** `roswell test-dumb` → **257 OK, 0 CRASHED, 0
    FAILED**; `roswell test-wallet` → **109/0/0**; `roswell test-ci` (all groups:
    dumb/crypto/zoon/wallet/bridge/verifier) → **exit 0**. All independently verified.
  - Root cause of the 56 crashes was NOT the derived-state — it was `(need pow)` on
    powless test blocks: this branch removed the `check-pow-flag` testnet bypass in the
    pow checks, so test blocks now need a real/mock proof. Fixed within `hoon/tests/`
    only: (a) 44 lib-level tests get a minimal well-typed `%0` mock-pow that clears
    `check-target` at default difficulty (masks nothing — reject tests still reach
    their rejection reason); (b) 11 kernel-integration tests generate **real
    verifiable** STARK proofs with fast params (`pow-len=1`, `genesis-target=max`) so
    the kernel's real `check-pow`/`verify:nv` runs unweakened; (c) 3 `kernel-state-8`
    tests updated `%9`→`%11` (the branch's newest state after the 9→10→11 AI-PoW/ASERT
    upgrades). No consensus/derived/miner code touched; no soundness check weakened.
    The roswell **Rust** unit tests also pass 17/0.

**Net: all crates build, all 6 hoon kernels build + their hoon test suites pass, all
Rust test suites pass — including the ai-pow zk lib suite (161/0) after removing
`high2_2_attests_real_solved_tile` (see below).**

**On `high2_2_attests_real_solved_tile` (REMOVED):** it drove the dense path with
`require_prod_envelope=false` to prove ONE selected *non-zero* tile of a multi-tile
config — exactly the shape production deliberately fails-closed on
(`FullMatmulProofUnavailable`), since proving one tile ≠ the full multi-tile aggregate.
So it tested an unsound single-selected-tile shortcut, not any real path: its one
production-relevant assertion (jackpot byte-compat) is already covered for every real
path (Layer-0 composite ~`zk_bridge.rs:3803`, production recursion ~`4024`/`4089`,
single-tile statement ~`4238`; found_idx binding ~`4280`). Removed per user decision.
**Genuine multi-tile support = the full-matmul aggregate proof, a documented Pearl
production residual** (the code fails-closed on `num_tiles>1` pending "the future
full-matmul proof"; see the fail-closed rationale at `validate_canonical_recursive_
certificate_params`) — tracked separately, to be implemented per Pearl's model.
- **peek.jam** ✅ FIXED. `format-page` read `` `-.u.pow `` off the opaque
  `pow-artifact` (`*`), inferring `(unit *)` → `nest-fail`. Per user guidance (the
  pow-artifact must stay scrutable with a version tag), replicated consensus's
  `+pow-artifact-to-proof-version`: `[%ai-pow *]` → v3, else soft-cast to `proof` and
  read `version`. **peek.jam rebuilds (19M).** (The 0-byte on-disk peek.jam was a
  deliberate placeholder to keep the Rust crates compiling, not a failed build.)
- **high2_2_attests_real_solved_tile** (ai-pow zk test): pre-existing, non-production
  (see earlier section).
- [x] S6. **Full workspace `cargo build` — GREEN** (bridge, roswell, e2e, …).
  Benches: `bench_dumb_validation` adapted (branch PoW is unconditional; the legacy
  `check_pow_flag` was removed). `bench_nockchain_kernel` **deleted** — it drives the
  pre-split monolithic mining (`nockchain::mining::MiningWire`, `%mine`), which the
  branch's zk/ai mining split removed (branch had already deleted it). NOTE: this
  overrides the earlier "keep master's bench" call — master's version is
  fundamentally incompatible with the mining refactor and would need a full rewrite
  to branch's split-mining (`ZkPowMinerWire`, `%mine-zk`); re-add as a follow-up if
  the benchmark is needed.
- [~] S7. Tests. Green so far (12-core native):
  - `ai-pow-zk` lib **408/0** (validates `permute_5round` + the composite).
  - `ai-pow` **270/0**, `ai-pow-miner` **130/0**.
  - **recursion round-trips 2/0** (`real_moe` + `noncontiguous`) — the definitive
    check that `permute_5round` matches the in-circuit Tip5 (recursion soundness).
  - **`roswell` 17/0** (required).
  - `nockchain-types` **68/0** (ASERT + `BlockchainConstants` NounEncode/Decode
    round-trip), `nockchain` lib **12/0**.
  - Full `cargo test --workspace --no-fail-fast`: green **except one PRE-EXISTING
    failure** (see below).

### Pre-existing failure (NOT a port regression): `high2_2_attests_real_solved_tile`

`ai-pow zk_bridge::tests::high2_2_attests_real_solved_tile` (a `--features zk`
test not run by default-feature validation) fails with a `noised_packed` LogUp
`GlobalCumulativeMismatch` on tile **(0,5)** — a NON-zero dense tile proved via
`prove_and_verify_for_block_inner`. **Confirmed it fails IDENTICALLY on the
pre-port branch `claude/ai-pow-integration-squash`** (checked out + run), so the
port did not cause it. It is a pre-existing composite bug in the dense Layer-0
prove+verify for non-zero tiles. **Not production-critical:** production uses
`prove_and_verify_for_block` only for single-tile attempts (found_idx=0 → tile
(0,0), which passes); multi-tile production goes through the recursive certificate
(validated — recursion round-trips 2/0). tile (0,0) passes; only non-(0,0) tiles
via this dense test path fail.

**Residual (separate soundness investigation, per R1 — not rushed as a port
afterthought):** the `noised_packed` producer/consumer keying for a non-zero-column
dense tile in `prove_and_verify_for_block_inner` imbalances the bus. Fix requires
aligning the store (global column) and sweep (tile-local column) keys for non-zero
`tile_j`; validate with high2_2 + the composite suite. Since it is pre-existing and
in a non-production path, it does not block the port itself.

## Log

### S1 — nockchain-types (ASERT)
The rebase auto-merged `blockchain_constants.rs` into a broken state (master's flat
`asert_*` fields + branch's `AsertParams` structs coexisting; the flat-asert parser
referenced fields the merged struct lacked). The consensus **hoon** (`tx-engine.hoon`:
`phase.zk-asert`) uses branch's `AsertParams` model, and the flat `asert_*` fields
had no users outside the broken file → take **branch's** `blockchain_constants.rs`.
