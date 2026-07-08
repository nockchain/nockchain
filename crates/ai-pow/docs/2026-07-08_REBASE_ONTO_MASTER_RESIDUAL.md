# AI-PoW rebase onto nockchain master — checkpoint + porting residual

**Date:** 2026-07-08
**Branch:** `claude/ai-pow-integration-squashed`
**State:** the 543-commit `claude/ai-pow-integration-squash` branch was squashed into
one commit and rebased onto `origin/master` (`6444e5c7`). The **squash + mechanical
rebase are complete** (all 9 explicit merge conflicts resolved). The branch **does
NOT build yet** — it predates several major master refactors that require a
follow-on port (below). The original branch `claude/ai-pow-integration-squash` is
untouched and builds against the old master; use it as the known-good reference.

## Resolved in this checkpoint (the mechanical rebase)

| file | resolution |
|---|---|
| `.gitignore` | union of both sides + `proptest-regressions/` |
| `Cargo.toml` (workspace) | union of members (master's crates + ai-pow crates) + both dep entries + `exclude` |
| `crates/nockchain/Cargo.toml` | keep master's `[lints]` + branch's `nockchain-mining-common` dev-dep |
| `crates/nockchain/src/bin/bench_nockchain_kernel.rs` | **master's** (per user) |
| `crates/nockchain-math/src/tip5/{mod,hash}.rs` | **master's** (per user — see port item 1) |
| `hoon/apps/dumbnet/lib/miner.hoon` | master's jet hints + branch's AI-PoW semantics (derived-state door, `pow-artifact:t`) |
| `hoon/apps/dumbnet/lib/consensus.hoon` | master's `~% %consensus` hint + branch's derived-state door + `height-to-proof-version-legacy` rename |
| `hoon/apps/dumbnet/inner.hoon` | branch's poke `$` arm (src-split `%zk-pow-miner`/`%ai-pow-miner`, libp2p case, `%mine-zk`); outer `~/ %poke` hint kept; inner `%poke-helpers` hint dropped |
| `Cargo.lock` | branch's, then reconciled by `cargo metadata` for the merged member set |

## The port residual (why it doesn't build) — all "take master's + adapt ai-pow"

The branch predates these master refactors; each must be reconciled. Build
iteratively (`cargo build` → fix the surfaced crate → repeat).

1. **tip5 / zkvm-prover (`f5b7f3cb`, "zkvm: expose open prover jets + math
   helpers", 54 files).** Master rewrote `nockchain-math` tip5 to a **Montgomery
   domain** with helpers `hash_ten_cell`, `hash_belts_slice`, `MONT_ONE`, and
   overhauled the whole `zkvm-jetpack` prover; the branch is on the **pre-f5b7f3cb**
   tip5 (normal domain; its own API `hash_varlen`, `tip5_calc_digest`,
   `tip5_montify_vecbelt`, `permute_5round`, `create_init_sponge_*`). tip5 is now
   **master's**. **Port:** adapt `ai-pow-zk` (and any `ai-pow`/`ai-pow-miner` tip5
   use) to master's tip5 hash API, and **add `permute_5round` + `NUM_ROUNDS_5ROUND`
   to master's tip5** (the reduced-round variant the recursion needs), implemented
   in master's Montgomery domain. Verify byte-identical Tip5 output (KATs) since
   this is the consensus hash. **Largest item.**

2. **ASERT difficulty (`blockchain_constants.rs`).** Master added `asert_*` fields
   to `BlockchainConstants` (ASERT difficulty adjust); the branch added
   `zk_asert_post_ai` / `ai_pow_activation_height` / `ai_asert`. git's auto-merge is
   **textually clean but semantically broken** (struct missing fields its own parser
   references — current build error). **Port:** take **master's** `BlockchainConstants`
   (with ASERT fields + noun parse/build), then re-add the branch's AI-PoW fields
   adapted to master's struct/parser. Consensus-critical (difficulty + AI
   activation height) — validate against master's ASERT tests + the ai-pow
   activation logic.

3. **Nous (`013-nous`, libp2p req-res gen2).** `inner.hoon` took the branch's poke
   `$` arm (which already had the libp2p `%peer-id` case). **Verify** master's Nous
   gen2 handlers (batched transport, block+tx bundles, catch-up range) — which
   auto-merged elsewhere in `inner.hoon`/`libp2p-io` — are actually reachable
   through the branch's dispatch, or graft the missing gen2 wire cases.

4. **Further divergences.** Expect more per-crate breakage as items 1–3 unblock
   the build (the branch is ~36 master commits behind, incl. bridge/bazel). Resolve
   each by the same rule: take master's version of shared/consensus code, re-apply
   or adapt the AI-PoW delta.

5. **Kernel jams (after Rust compiles).** The dumbnet hoon changed
   (`inner`/`consensus`/`miner`), so per `hoon-jam-builds`: rebuild the affected
   `.jam` (dumb, miner) **before** the Rust binaries that embed them, or the hoon
   changes silently don't take effect.

6. **Validation gate (per R1, consensus-critical).** Full workspace build +
   `ai-pow-zk` (405+), `ai-pow`, `ai-pow-miner` suites + the recursion round-trips
   (`noncontiguous`/`real_moe`) + master's consensus/ASERT/Nous tests. Do **not**
   treat the rebase as done until this is green.

## Recommended approach for the follow-up

Because item 1 (tip5/zkvm) is a large, consensus-hash-critical adaptation, drive it
in validated stages: first make `nockchain-math`/`zkvm-jetpack` (master's) + the
ai-pow tip5 shim compile with byte-identical Tip5 KATs, then item 2 (ASERT), then
the build-iterate loop, then jams + the full gate. Keep `claude/ai-pow-integration-
squash` as the reference for the branch's original (old-master) behavior.
