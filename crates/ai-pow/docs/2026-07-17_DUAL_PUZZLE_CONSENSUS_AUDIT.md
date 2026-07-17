# Dual-Puzzle (ZK-PoW + AI-PoW) Consensus Threat-Vector Audit

Date: 2026-07-17
Scope: consensus soundness of the dual-puzzle chain (both a ZK miner and an AI miner
producing blocks on one chain) for a production livenet. Supersedes the dual-puzzle
sections (§9) of `2026-07-16_AI_POW_PRODUCTION_HARDENING.md`, several of whose
residuals have since landed.

## Context: what changed since the 2026-07-16 hardening doc

The 07-16 doc concluded "AI-PoW is wired at the type/dispatch level but NOT
consensus-complete" (§9) — AI blocks unacceptable post-ASERT, AI ASERT degenerate,
target saturation, no side-by-side tests. **Most of §9 has since landed** and is now
demonstrated live: a fakenet node with both `zk-pow-mine` and `ai-pow-mine --canonical`
attached advanced a single chain to height 9 with BOTH puzzles winning heights
(7 ZK : 3 AI, zero errors over 12 min), each block carrying its own ASERT target
(ZK `2^313`, AI `2^244` in that run). See `[[fakenet-dual-miner-tuning]]`.

## Verdict summary

| Vector | Verdict |
|---|---|
| Equal-weight heaviness (`compute-work-ai`) | SOUND (exact integer identity) |
| `check-target` (forged-easy-target) | SOUND (deterministically re-derived + checked) |
| `check-heaviness` (work inflation) | SOUND (re-derived from validated target) |
| AI work-meets-target (`ai-pow-verify` jet) | SOUND (cert bound to commitment+target, fail-closed) |
| `check-timestamp` / time-warp | SOUND (BIP113 median-of-11 + max-future; shared global median) |
| Puzzle-type ↔ proof binding | SOUND (`proof-version-valid-at-height` + per-type verify) |
| Arbitrary miner matrices | SOUND (per-nonce noise forces fresh matmul; difficulty binds work) |
| 256-bit AI-target saturation (§9.3) | FIXED (kernel already bex 227; Rust default corrected this audit) |
| verify-jet crash DoS under panic=abort (§4.6) | FIXED this audit (build guard) |
| Cache-thrash DoS via `trace_height` (§4.4) | OPEN — mitigation documented below |
| One-PoW-binds-two-commitments (N5) | OPEN — needs a consensus decision; mitigation ready |
| Checkpoint (non-compact) verify skips program bind (§1.4) | LATENT — production uses compact (bound); not consensus-wired |
| External ZK/circuit soundness audit (§1.1) | OPEN — external; everything above is conditional on it |
| MoE live-vs-gated (§1.3) | OPEN — decision + routing-binding audit |

## SOUND — analyzed this audit

**Equal-weight heaviness.** `compute-work(T) = max-target-atom/(T+1)` (expected ZK
attempts in the ~2^320 space); `compute-work-ai(T) = max-target-atom/(T·2^64+1) =
2^256/T` (expected AI attempts in the 256-bit space). `compute-work-ai(T) ==
compute-work(T·2^64)` is an exact integer identity, so at AI-target T and ZK-target
T·2^64 the two puzzles have equal solve probability and their work sums meaningfully.
Work rises as the target tightens, so there is no cheap-weight coin-hopping (mining
the easier puzzle yields *less* work per block). `block-compute-work` (consensus.hoon)
dispatches by the block's proven puzzle-type; `check-heaviness` requires
`accumulated-work == parent + block-compute-work`, and `check-target` requires the
block's target to equal the ASERT-recomputed target — both deterministic, so a
forged easy target or inflated work is rejected (`%page-target-invalid` /
`%page-heaviness-invalid`).

**AI PoW verification is fail-closed.** `do-pow` (local-miner path) reconstructs the
candidate via `build-ai-candidate` and runs `check-pow`; the gossip `heard-block`
path re-validates independently. `check-pow`'s `%ai-pow` branch runs the
`ai-pow-verify` jet, which binds the certificate to `(block-commitment, target)`,
enforces `jackpot ≤ target`, and is panic-safe (`catch_unwind`). A stale or forged
cert is rejected.

**Time-warp.** `check-timestamp` enforces `timestamp ≥ parent-median-of-11` and
`≤ now + max-future`. Both puzzles read the same global median, so timestamp
manipulation cannot create cross-puzzle difficulty asymmetry; the per-puzzle
difficulty difference comes only from the deterministic subchain block-count.

## FIXED this audit

- **§9.3 — Rust AI anchor `2^291` → `2^227`** (`blockchain_constants.rs`, commit
  68527922). The AI jackpot is 256-bit, so an anchor `≥ 2^256` is trivially cleared
  (no PoW at the anchor). The Hoon consensus default is already the correct `bex 227`;
  the Rust default was stale (fakenet-only, CLI-overridable, so not mainnet-live, but
  a no-override fakenet got trivial AI PoW). Now matches the kernel.
- **§4.6 — `panic=unwind` build guard** (`ai-pow-jets`, commit 68527922). The verify
  jet's no-crash guarantee relies on `catch_unwind`; `panic=abort` makes it a no-op,
  so one crafted `%ai-pow` block would abort the node. A `compile_error!` now refuses
  to build the consensus verifier under `panic=abort`.

## OPEN — the production residual (prioritized)

1. **§1.1 external ZK/circuit audit (HIGHEST).** The recursion + composite-AIR
   soundness is ASSUMED, not verified — the one place a false-accept could live.
   Every "SOUND" verdict on the crypto is conditional on this. Requires a dedicated
   external circuit + recursion audit before an adversarial mainnet.

2. **§4.4 cache-thrash DoS.** `trace_height` is attacker-controlled (cert field, ≤2^19)
   and a cache miss triggers a ~0.6 s synchronous disk page-in on the serf thread; an
   attacker cycling ≥3 heights stalls consensus. **Mitigation (operator):** set the
   verifier cache cap ≥ 7 (all production buckets 2^13..2^19 resident) —
   `AI_POW_VERIFIER_CACHE_CAP`. **Code fix (next):** move page-in off the consensus
   thread or pin all buckets resident for validators.

3. **N5 one-PoW-binds-two-commitments.** A merge-miner who knows two
   `nock_block_commitment`s can embed both aux tags in one coinbase, so one Pearl PoW
   satisfies aux-inclusion for both → two same-height forks from one unit of work.
   **Mitigation (ready):** enforce exactly one `NOCKCHAIN-AI-POW-AUX` tag occurrence
   in the verified coinbase. Needs a consensus decision + Hoon/verify change + test.

4. **§1.3 MoE live-vs-gated.** The verify jet accepts MoE if the proof verifies, but
   its routing binding is not deep-audited and code comments still say "fail-closed."
   Decide: gate MoE off until audited, or confirm `verify_pearl_moe_routing_binding`
   and mark it live. Reconcile the stale comments (§1.6).

5. **§4.3 / §1.4 hardening.** Make `%failed-pow-check` an IP-level (Strong) ban, not a
   rotatable weak one (§4.3). Add the canonical-program equality check to the
   checkpoint `verify_recursive_certificate` or keep it off the consensus path (§1.4;
   the production *compact* path already binds it via the D6/P0 fold).

## Not re-audited (verify the fixes, per the prior corpus)

Byte-level Pearl commitment/tile/jackpot parity; the 27-angle MoE adversarial suite
(6 issues fixed); replay binding; setup-digest determinism; grinding/difficulty
binding; decode DoS bounds; the compact opened-schedule binding (D6/P0). See
`2026-07-16_AI_POW_PRODUCTION_HARDENING.md` and the docs it cross-references.
