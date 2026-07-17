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
| One-PoW-binds-two-commitments (N5) | FIXED this audit (exactly-one aux-tag enforced + tests) |
| MoE live-vs-gated (§1.3/§1.6) | RESOLVED this audit (live via compact path, routing-bound; stale comments reconciled) |
| Cache-thrash DoS via `trace_height` (§4.4) | MITIGATED this audit (operator CLI knob); code page-in-off-thread residual |
| Liar `%failed-pow-check` ban is peer-id/Weak, not IP/Strong (§4.3) | DEFERRED — current ban deemed sufficient; high-risk networking surgery |
| Checkpoint (non-compact) verify skips program bind (§1.4) | LATENT — production uses compact (bound); not consensus-wired |
| External ZK/circuit soundness audit (§1.1) | OPEN — external; everything above is conditional on it |

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
- **N5 — one PoW must not bind two commitments** (`ai-pow/src/pearl_compat.rs`,
  commit 29ef1eb9). `verify_pearl_aux_inclusion` now requires the
  `NOCKCHAIN-AI-POW-AUX` tag to occur **exactly once** in the verified coinbase (was a
  substring `contains` check), so a merge-miner cannot embed two aux commitments under
  one Pearl PoW and mint two same-height forks from one unit of work. Covered by
  `pearl_aux_inclusion_rejects_double_tag_n5` and
  `pearl_aux_inclusion_rejects_tag_without_room_for_commitment`.
- **§4.4 — verifier cache-thrash DoS knob** (`nockchain` CLI, commits 29ef1eb9 +
  9c495d88). `--ai-pow-verifier-cache-cap` (env `AI_POW_VERIFIER_CACHE_CAP`) lets an
  operator pin all trace-height buckets resident (cap ≥ 7) to neutralize the
  attacker-controlled page-in thrash. Default kept low (2) to bound RSS; raise it if
  the vector is exercised. Moving the page-in off the consensus thread remains a
  code-level residual (below).
- **§1.3 / §1.6 — MoE is live via the compact path; stale "fail-closed" comments
  reconciled** (`ai-pow/src/pearl_compat.rs` + test, commit 652c2b58). MoE ai-pow
  blocks ARE accepted, through `verify_pearl_moe_compact_recursive_certificate` +
  `verify_pearl_moe_compatible_work`, which bind the routing commitment (demonstrated
  by the live dual chain). The stale docstring/error-string on the *dense*-prover
  config guard (which correctly refuses MoE as a caller-routing error) was a
  soundness-maintenance hazard — it read as though no MoE block could ever be accepted —
  and is now corrected. MoE's *deep* circuit soundness remains conditional on §1.1.

## OPEN — the production residual (prioritized)

1. **§1.1 external ZK/circuit audit (HIGHEST).** The recursion + composite-AIR
   soundness is ASSUMED, not verified — the one place a false-accept could live.
   Every "SOUND" verdict on the crypto is conditional on this. Requires a dedicated
   external circuit + recursion audit before an adversarial mainnet.

2. **§4.4 cache-thrash — code residual (LOW).** The operator knob above closes the
   practical DoS. The remaining code-level improvement is to move the ~0.6 s
   `trace_height` page-in off the serf/consensus thread (or pin all buckets resident
   for validators by default), so an under-provisioned operator who leaves the cap low
   is not exposed. Not consensus-soundness; a latency/liveness hardening.

3. **§1.4 checkpoint program-bind (LATENT, defense-in-depth).** The non-compact
   checkpoint `verify_recursive_certificate` does not assert the canonical-program
   equality. Production AI-PoW uses the *compact* path, which DOES bind the program via
   the D6/P0 opened-schedule fold, and the checkpoint path is not consensus-wired — so
   this is not currently reachable on the accept path. Add the equality check (or a
   `debug_assert` + doc that it is test-only) before ever wiring the checkpoint path
   into consensus.

4. **§4.3 liar-ban strength — DEFERRED (accepted risk).** A `%failed-pow-check` liar
   currently earns a peer-id-scoped (Weak) ban: a session-lived libp2p `allow_block_list`
   block on the peer-id + fail2ban logging, with the IP-exclusion ceiling at
   `IP_EXTENDED_EXCLUSION`/`MAX_AUTO_EXCLUSION` = 6 h. Escalating *specifically*
   `%failed-pow-check` to an IP-level (Strong) exclusion would require reason-parsing
   the liar effect and resolving each tracked peer's address inside the driver's ban
   path — invasive surgery on load-bearing networking code with real false-positive
   risk (NAT-shared IPs). **Decision (operator):** the current ~6 h ban is deemed
   sufficient; defer the IP-level escalation to a dedicated, separately-validated
   networking effort. A prototype of the targeted change (Strong only for
   `%failed-pow-check`, Weak preserved for all other liar reasons that can arise from
   honest soft-fork ruleset skew) was written and reverted this session; see the commit
   history if revisited.

## Not re-audited (verify the fixes, per the prior corpus)

Byte-level Pearl commitment/tile/jackpot parity; the 27-angle MoE adversarial suite
(6 issues fixed); replay binding; setup-digest determinism; grinding/difficulty
binding; decode DoS bounds; the compact opened-schedule binding (D6/P0). See
`2026-07-16_AI_POW_PRODUCTION_HARDENING.md` and the docs it cross-references.
