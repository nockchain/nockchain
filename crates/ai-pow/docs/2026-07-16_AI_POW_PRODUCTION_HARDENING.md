# AI-PoW Production Hardening & Threat Model

Status: WORKING DRAFT (2026-07-16). This document enumerates every angle needed to
take the AI-PoW (`%ai-pow`) consensus puzzle into a fully adversarial open-internet
deployment. Each item has a **STATUS**: `DONE` (implemented + validated), `SOUND`
(argued sound, tests in place), `ASSUMED` (relied upon but not independently audited),
`OPEN` (known gap, work required), or `RESIDUAL` (accepted risk with rationale).

## 0. Threat model

**Adversaries.**
- **Profit-maximizing miners** — extract maximum block reward for minimum work: forge
  a proof, grind favorable puzzle parameters, claim a cheaper puzzle than performed,
  replay/precompute across blocks, or drift difficulty down.
- **Value extractors** — anything that games incentives (selfish mining, difficulty
  manipulation, MEV-adjacent behavior) — largely out of AI-PoW's specific scope but
  noted where the puzzle interacts.
- **Spammers / vandals** — no economic goal beyond degrading the network: flood
  invalid/malformed blocks, exhaust CPU/memory/disk, crash nodes, force forks.

**Environment.** Open internet, gossip, no trusted peers. Every input reaching the
verifier (`%ai-pow` artifacts, block headers, targets) is attacker-controlled.

**Assets to protect.**
1. **Consensus integrity** — no forged blocks accepted; no honest block wrongly
   rejected; all honest nodes agree (no forks from verifier divergence).
2. **Node availability** — a node cannot be cheaply crashed, stalled, or exhausted.
3. **Work == reward fairness** — a block's accepted difficulty reflects real work.

---

## 1. Consensus soundness — the verifier (the foundation)

Everything rests on the compact recursive-STARK verifier being SOUND: it must accept a
certificate only if the prover actually performed the committed matmul work and found a
jackpot meeting the target, bound to the block. A soundness break here = free money.

- **1.1 Recursion STARK soundness (no forgery) — THE FOUNDATION, ASSUMED SOUND,
  UNAUDITED.** The whole anti-grind/anti-forgery argument reduces to (a) the composite
  AIR fully constraining matmul correctness, the selector-gated PI bindings
  (`composite_public.rs:19-91`), and the anti-grind linchpin that the exact matrix bytes
  fed to the matmul equal the bytes hashed into `H_A/H_B` via the `noised_packed` LogUp
  bus (`composite_lookups.rs:105-185`); and (b) the vendored Plonky3 batch-STARK + LogUp
  + FRI + preprocessed-commitment being sound with a Fiat-Shamir transcript binding all
  60 public values AND the L0 program commitment (folded `recursion.rs:461-481`, verified
  `recursion.rs:2305-2326`). The *plumbing* is confirmed (buses exist, PIs + program
  commitment are folded); AIR constraint COMPLETENESS and p3 soundness are NOT verified.
  STATUS: **ASSUMED — needs a dedicated external circuit + recursion ZK audit. This is
  where a real false-accept would live.**
- **1.2 Anti-grind constraint completeness.** The "miner-chosen matrices are safe even
  for degenerate `A=B=0`" claim rests on 1.1 (commitment-keyed noise `A'=A+E` forces the
  full `h·w·dot` matmul; `s_A` keyed by `H_A/H_B`). STATUS: `ASSUMED` (rests on 1.1).
- **1.3 MoE (`e>0`) is admitted LIVE despite "fail-closed until M4" comments.** The jet
  accepts MoE if the proof verifies (`certificate_noun.rs:3264-3272` →
  `...compact_moe...:2472`); the MoE verify looks complete (routing-consistency binding,
  per-expert column clamp, routing-spliced seed recompute + PI binding, D6 program fold).
  But pervasive comments still say MoE "stays fail-closed until M4" and there is NO gate
  actually stopping it. Its soundness rests on `verify_pearl_moe_routing_binding` (not
  deep-audited) + 1.1. STATUS: **OPEN — make an explicit decision: gate MoE off until
  audited, or confirm the routing binding + accept it as live.**
- **1.4 Latent footgun — the non-compact verify path skips program binding.**
  `verify_decoded_ai_pow_pearl_merge_artifact_with_context_and_limits`
  (`certificate_noun.rs:2277-2294`) calls `verify_recursive_certificate` WITHOUT
  `l0_program_matches` — "a malicious prover can embed a program that opened a
  prover-favorable strip" (`recursion.rs:132-146`). It is `#[doc(hidden)]` and NOT
  consensus-wired (the jet uses only compact paths), but it is a public API. STATUS:
  **OPEN — `#[cfg(test)]`-gate it or add the canonical-program equality check** so it can
  never be wired in by mistake.
- **1.5 Replay / malleability — MITIGATED.** The block commitment is bound transitively
  into the proof: `nock_block_commitment → aux.commitment (==checked) → Pearl header
  merkle_root → kappa=BLAKE3(header‖config) → s_A/s_B → commitment-keyed noise + jackpot
  key`, and `kappa,s_A,s_B` are folded into the L0 program commitment; the jackpot is
  bound (`pis.hash_jackpot == public_params.hash_jackpot`, the value the difficulty gate
  tests). Replaying A's proof for block B changes the commitment → different program/PIs
  → reject. STATUS: `SOUND` (cited checks + the wrong-commit KAT + accept e2e).
- **1.6 Stale docstrings misdescribe the LIVE security model — soundness-maintenance
  hazard.** Several docstrings still assert the REMOVED synth-pin model ("re-derives the
  canonical matrices from the protocol seed", `certificate_noun.rs:3178-3198`,
  `hoon/common/pow.hoon:9-11`), and "this function must not be reached by block
  acceptance" / "fail-closed until M4" where the code is now live+bound. STATUS: **OPEN
  doc debt — correct before external audit** (an auditor or a future dev could "fix"
  toward a stale comment and reopen a hole).

---

## 2. Determinism & the committed consensus parameter

The verifier-setup table is a consensus parameter pinned by a committed BLAKE3 digest
`AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST` (`e7eef3f4…`). Every node must build a
byte-identical verifier or it either forks (if the mismatch is undetected) or halts (if
detected). We detect: boot re-derives the digest and refuses to run on a mismatch.

- **2.1 Little-endian enforced.** `nockvm::check_endian()` panics on any non-LE target
  at startup, so byte-order is not a cross-machine variable. STATUS: `DONE`.
- **2.2 Run-to-run determinism (per-process randomness ruled out).** The vendored
  circuit crate uses `hashbrown::HashMap::new()` (randomized hasher, different seed per
  process). Two full-table generations in **separate processes** produced the identical
  digest — empirically proving hashmap iteration order (and any per-process randomness)
  does NOT feed the digest. STATUS: `DONE`.
- **2.3 Deterministic by construction (given LE-only) — VERIFIED.** A rigorous trace of
  the entire path establishes it: the digest reduces to
  `Tip5-sponge(postcard(route_params_const) ++ postcard(metadata) ++ postcard(fri_shape))`
  — `route_params`/`fri_shape` are compile-time constants, and `metadata` OMITS the proof
  body (proof-independent), carrying only the preprocessed commitment + shape. Every
  `HashMap`/`HashSet` in the path was enumerated and shown to be keyed-lookup/membership
  ONLY (output ordering always comes from `Vec`/fixed-slice/enum sequences, never map
  iteration); the DFT is a structurally order-independent NTT and the commit is a
  tree-structured Merkle MMCS; there is NO float, NO order-sensitive parallel reduction,
  NO uninitialized/padding bytes hashed (postcard is field-by-field), NO pointer/capacity
  or time/env/pid input. Fixed synth seed, `is_zk()==0`, deterministic Fiat-Shamir (which
  does not even reach the digest). The one order-dependent serialization (`serde_npo_map`)
  is provably OFF the consensus-digest path (it only feeds the on-disk file's local
  sidecar checksum) and is emptied by `into_verifier_only()` anyway. STATUS: `SOUND` —
  deterministic across machines/architectures by construction given LE.
- **2.4 Cross-ARCH CI (belt-and-suspenders).** Because the digest is integer/field
  arithmetic + NTT + Merkle over LE constants (all architecture-independent), cross-arch
  reproduction is a theoretical certainty, not an open risk. A CI job that regenerates
  the table on x86_64 AND aarch64 and asserts both reproduce `e7eef3f4…` is still
  recommended as a regression tripwire (it would also catch a future vendored-crate bump
  that accidentally introduced nondeterminism). STATUS: `RECOMMENDED` (was `OPEN`;
  downgraded by the determinism audit). A mismatch is fail-loud regardless (a divergent
  node shuts down at the boot digest check — no silent consensus split).
- **2.5 Digest versioning / upgrade.** The domain tag carries `v0`; any change to the
  setup format, bucket set, or verifier bumps the version + re-pins the constant.
  STATUS: `DONE` (mechanism); a documented upgrade runbook is OPEN.
- **2.6 The digest binds what verification uses.** The committed digest is the
  verifier-key digest, which commits (via `metadata.stark_common`) to the preprocessed
  Merkle root the verifier authenticates openings against — so a divergent tree cannot
  bypass into an accept, only cause a reject. STATUS: `SOUND` (adversarial review
  confirmed).

---

## 3. Grinding / difficulty gaming (miner value extraction)

Audited. The value-extraction vectors are **provably mitigated with cited verifier
checks** — CONDITIONAL on the §1.1 ZK-soundness foundation holding.

- **3.1 Miner-chosen matrices/params — difficulty binding. MITIGATED.** Target =
  `base × (h·w·dot)` (`pearl_compat.rs:1078-1103`); a bigger factor eases the target but
  costs proportionally more MACs per opened tile, so expected work is invariant. The
  verifier (not just the miner) enforces consensus floors/ceilings via `sanity_check →
  envelope_check_dims` (`pearl_compat.rs:976-1076`, reached dense `:2238` / MoE `:1767`):
  `k ≥ 1024`, `k ≥ 16r`, `r` pow2 in `[32,1024]`, `h·w ∈ [32,256]`, `k ≤ 2^16`,
  `dot%8==0`, `dot = k − k%rank`; `difficulty_bits` is PINNED to 0
  (`certificate_noun.rs:2732`), `tile==h`, square tiles. The opened schedule (fixing
  `h,w`) and `k` are bound into the L0 program commitment, so a weak shape can't be
  declared for the relaxation while a cheaper tile is proven. STATUS: `SOUND` (cited).
  Residual (informational): the per-candidate BLAKE3 overhead is not in `h·w·dot`, a
  minor ASIC-shape optimization, not a reward-for-no-work break.
- **3.2 trace_height / setup selection — MITIGATED.** The verifier IGNORES the cert's
  `trace_height` and RECOMPUTES the required one, requiring exact equality
  (`certificate_noun.rs:2769-2784`; MoE `zk_bridge.rs:2032-2033`), with a `2^22` backstop
  and the jet's `2^19` cap. A too-small height that slips past makes
  `canonical_program_for_strip_schedule` assert → panic → caught → `NO`. STATUS: `SOUND`.
- **3.3 ASERT interaction — MITIGATED (conditional).** AI blocks use their own
  `compute-target-ai-asert` over same-puzzle ancestors and the block's `target` must
  equal the consensus-computed target (miner cannot supply it). ASERT assumes constant
  work/block; the `h·w·dot` factor is what makes each AI block ≈ constant work, so the
  assumption holds conditional on §1.1/§3.1. STATUS: `SOUND` (conditional). See §9 for
  the cross-puzzle interaction.
- **3.4 Precomputation / amortization — NO ADVANTAGE.** `kappa/s_A/s_B` are per-block
  (bound to the commitment), so tiles/noise cannot be reused; degenerate matrices give
  no shortcut; no sub-linear nonce grind. STATUS: `SOUND` (rests on §1.1).
- **3.5 Target-multiply saturation (bootstrap-only).** `u256_le_mul_u128_saturating`
  saturates at 2^256; only reachable at genesis / deep-crash difficulty (base target
  `> ~2^232`). No explicit consensus rejection of a saturated adjusted target. STATUS:
  OPEN (LOW) — optionally reject a saturated result.

---

## 4. DoS / resource exhaustion

Audited. **Well-hardened against crashes, false-accepts, and unbounded
allocation/recursion.** The residual exposure is *resource-amplification of valid
rejects* — an attacker referencing a real (public) parent block-id cheaply satisfies
every consensus pre-check, then varies the certificate bytes to force full verifies.

- **4.1 Cheap-reject-first ordering — CORRECT but insufficient.** The jet checks the
  `trace_height ≤ 2^19` cap and decodes before the STARK (lib.rs:469-479); the verify
  runs cheap deterministic checks before the STARK. BUT the difficulty precheck is
  attacker-satisfiable — `hash_jackpot` is only *proven* real by the STARK, so
  `hash_jackpot=[0;32]` passes the cheap gate (`pearl_compat.rs:1782`) and only the STARK
  rejects; for `%ai-pow` the cheap work check is DEFERRED to `check-pow`
  (`consensus.hoon:615-632`). STATUS: OPEN — no cheap admission cost before the STARK.
- **4.2 Decode bounds — SOLID.** `CertificateNounLimits` (depth 256, 1M nodes/list
  items, 64 MiB cumulative atom bytes, nonce ≤ ~100 KB, MoE routing ≤ ~25K entries)
  bound every recursion and every `with_capacity` (each length cross-checked before
  alloc). No unbounded blow-up. STATUS: `SOUND`.
- **4.3 Distinct-invalid-block spam — OPEN.** `%liar-block-id %failed-pow-check` only
  stops re-processing the SAME digest; distinct crafted blocks each force a fresh
  decode+page-in+verify. Gates: a per-IP gossip token bucket (2/s + 120 burst,
  `inbound.rs:288`) and a POST-verify per-peer-id ban (`LiarBlockId → BlockPeer`,
  `driver.rs:1739`) — but the ban is `Weak` severity (no IP exclusion) and rotatable
  (free new peer-id/IP). STATUS: OPEN — make `%failed-pow-check` Strong/IP-level, and/or
  add a cheap pre-verify admission cost.
- **4.4 Cache-thrash — OPEN, the strongest amplifier, INTRODUCED by disk-paging.**
  `trace_height` is fully attacker-controlled (a cert field, bounded only ≤ 2^19) and the
  ~0.6 s disk page-in runs SYNCHRONOUSLY on the single serf thread (lib.rs:486) BEFORE the
  commit derivation and STARK. With the default LRU cap 2 and 7 buckets, an attacker
  cycling 3+ heights forces evict+reload every block → a ~0.6 s serf stall each (a few
  hundred network bytes → 0.6 s disk read + multi-GB churn), blocking ALL consensus
  progress. This is the direct cost of the RSS optimization. STATUS: **OPEN — must fix
  before adversarial mainnet.** Options: (a) move page-in OFF the serf thread + rate-limit
  it (the real fix, preserves low RSS); (b) for validator/consensus nodes set
  `AI_POW_VERIFIER_CACHE_CAP ≥ 7` (pin all — trades the RSS win for thrash-immunity);
  (c) gate the cert `trace_height` before touching disk. **Immediate mitigation: document
  cap=7 for validators.**
- **4.5 Memory / OOM — SOUND.** Serial verification (single serf thread) → at most one
  verify + one page-in live at once; LRU ≤ cap contexts + a ~2× transient during page-in;
  jemalloc reclaims (required). Pokes queue on the serf (bounded by the token bucket +
  inflight backpressure). No unbounded stacking. STATUS: `SOUND` with a large-footprint
  caveat.
- **4.6 Panic-hook log spam + `panic=abort` residual — OPEN.** `catch_unwind` converts a
  panic to `NO` but does NOT suppress the default panic hook — a crafted panic-block
  floods stderr (log-fill DoS); no production path installs a hook. STATUS: OPEN — install
  a scoped/rate-limited panic hook. **More severe RESIDUAL:** the no-panic guarantee
  depends on `panic="unwind"` (currently default, NOT pinned); a future `panic="abort"`
  makes `catch_unwind` a no-op and a crafted-panic block CRASHES the node. **Pin
  `panic=unwind` with a build-time assertion.**
- **4.7 Block-size gate is after `check-pow`** (`consensus.hoon:718`), so a large
  (cue-bounded) cert forces a full verify before size rejection. STATUS: minor.

---

## 5. Verifier robustness (no crash / no panic)

- **5.1 The verify jet cannot panic the node.** The two attacker-controlled steps
  (decode, recursion verify) are wrapped in `catch_unwind` → a panic on crafted input
  is a deterministic reject (`NO`), never a node crash. The LRU eviction `remove(0)` is
  guarded; the page-in deserialize is wrapped. STATUS: `DONE` (release profile is
  `panic=unwind`, so `catch_unwind` is effective). RESIDUAL: relies on `panic=unwind`
  staying set and on `commit_from_noun`'s `jam` being panic-free over a bounded
  Hoon-constructed 5-belt commitment noun (audited low-risk, not wrapped because it
  mutates the interpreter stack).
- **5.2 Whole-node panic surface.** OPEN — this hardening covers the verify jet; a
  broader node-wide panic audit (other jets, the kernel driver) is out of scope here
  but recommended.

---

## 6. Reject-direction / liveness / fork safety

- **6.1 Deterministic-invalid → `NO` → liar.** A block whose `trace_height` has no
  committed bucket (attacker-craftable) is a deterministic invalid block → `NO`, which
  flows to `heard-block`'s `%liar-block-id %failed-pow-check` so it is marked and cannot
  be re-spammed. STATUS: `DONE`.
- **6.2 Non-deterministic fault → `%fail`.** A genuine per-node fault (uninjected
  table, missing/corrupt/bit-rotten context file) → `BAIL_FAIL` (`%fail`, the correct
  non-deterministic mote) so a broken node halts rather than wrongly rejecting valid
  blocks (which would fork it off). STATUS: `DONE`.
- **6.3 On-disk integrity.** Each context file has a BLAKE3 sidecar checksum
  re-verified on every page-in; bit-rot → `LoadFailed` → `%fail`, not a silent reject.
  STATUS: `DONE`.

---

## 7. Boot / setup / operational

- **7.1 Generate-or-shutdown boot.** On first boot a node generates the 7-bucket seed
  table (or shuts down on failure), then builds contexts to disk. STATUS: `DONE`.
- **7.2 Corrupt / format-incompatible cache recovery.** A bad seed cache is deleted +
  regenerated; a bad build is fatal. STATUS: `DONE`.
- **7.3 Disk-paged residency + jemalloc.** Contexts live on disk (~8 GB), paged into a
  bounded LRU; jemalloc is REQUIRED so freed page-outs return to the OS (verified).
  STATUS: `DONE`. RESIDUAL: ~8 GB disk requirement; first-boot build cost (~1–2 min).
- **7.4 Activation / upgrade path.** `%ai-pow` activates at a height; state loads
  "like a checkpoint" across the kernel change. STATUS: `SOUND` (validated earlier);
  a documented activation/upgrade runbook is OPEN.

---

## 8. Production-readiness checklist

| # | Item | Status |
|---|------|--------|
| 1.1 | Recursion STARK / composite-AIR soundness | **OPEN — external ZK audit required** |
| 1.3 | MoE live vs fail-closed decision | **OPEN — decide/gate** |
| 1.4 | Non-compact verify footgun (`#[cfg(test)]`-gate) | **OPEN** (small) |
| 1.6 | Correct stale synth-pin / fail-closed docstrings | **OPEN** (doc debt) |
| 2.3 | Digest deterministic by construction | **SOUND (verified)** |
| 2.4 | Cross-arch digest reproduction in CI | RECOMMENDED (tripwire) |
| 3.x | Difficulty↔param binding, trace_height, replay | **SOUND** (cond. on 1.1) |
| 3.5 | Reject saturated adjusted target (bootstrap) | OPEN (low) |
| 4.1 | Cheap admission cost before the STARK | **OPEN** |
| 4.3 | Distinct-invalid-block spam (IP-level ban) | **OPEN** |
| 4.4 | Cache-thrash page-in off-serf / cap=7 validators | **OPEN — critical** |
| 4.6 | Panic-hook log-spam + pin `panic=unwind` | **OPEN** |
| 5.1 | Verifier cannot panic the node | **DONE** |
| 6.x | Deterministic-reject vs %fail + checksums | **DONE** |
| 7.x | Boot / recovery / disk-paged / jemalloc | **DONE** |
| 9.2 | Cross-puzzle heaviness comparability | pending §9 audit |
| 9.4 | AI-PoW feature completeness vs ZK | pending §9 audit |
| 9.5 | Side-by-side dual-puzzle test | **OPEN — build** |

The `DONE` items are this session's landed + validated work. `SOUND` items are argued
sound with cited checks/tests. The `OPEN` items are the concrete gaps to close before an
adversarial mainnet — the load-bearing ones are **1.1 (external ZK audit of the verifier
soundness)** and **4.4 (the disk-paging cache-thrash DoS)**.

---

## 9. Dual-puzzle consensus (ZK-PoW + AI-PoW side by side)

Post-activation the chain runs BOTH puzzles at once: ZK-PoW (proof version %2,
`%mine-zk`) and AI-PoW (%3, `%mine-ai`). Blocks of either kind extend the same chain,
each with its OWN ASERT retargeting; fork choice compares one accumulated-work number
across the mix. This is the highest-risk consensus interaction and needs explicit tests.

- **9.1 Per-puzzle ASERT (design).** Target validation dispatches by puzzle type
  (consensus.hoon:647-666): a block's expected target is computed by
  `compute-target-zk-asert` or `compute-target-ai-asert` against its **same-type
  parent** (`find-same-type-ancestor`, so each puzzle retargets only against its own
  block series). STATUS: `SOUND` (structure) — _pending §-dual-puzzle audit for
  oscillation/interaction, bootstrap at activation (first AI block has no AI-parent →
  `degenerate` fallback to the immediate parent), and epoch/target-store handling_.
- **9.2 Cross-puzzle heaviness comparability — THE key risk.** Fork choice sums
  `compute-work(target)=max_target/(target+1)` (tx-engine-0.hoon:736) over all blocks
  regardless of puzzle. For this to be secure, an AI block and a ZK block of equivalent
  real cost must contribute comparable work. The concern: the AI jet checks the jackpot
  against `target × difficulty_adjustment_factor (h·w·dot)` (pearl_compat.rs), so the
  stored block `target` (which feeds `compute-work`) may not be on the same scale as the
  effective work — potentially over/under-crediting AI blocks vs ZK, which an attacker
  could exploit to build a "heavier" chain more cheaply. STATUS: `OPEN` — _pending
  §-dual-puzzle audit: is `compute-work(ai_target)` a faithful, ZK-comparable work
  measure given the factor?_ This must be resolved + tested before mainnet.
- **9.3 Version gating / selection.** `pow-artifact-to-proof-version` discriminates the
  puzzle from the persisted pow artifact (independent of the block's version field);
  `proof-version-valid-at-height` accepts %2 or %3 post-activation; `do-pow` admits
  whichever a miner solves first. STATUS: `SOUND` (validated: emit + accept e2e +
  roswell) — _confirm no version-field/artifact-version disagreement edge case_.
- **9.4 Feature completeness — AI-PoW vs ZK-PoW.** AI-PoW must participate in ALL the
  same consensus paths as ZK-PoW (heaviness, per-puzzle ASERT, epoch/target store,
  genesis, validation, coinbase/reward, gossip, candidate emission). STATUS: _pending
  §-dual-puzzle audit for any stubbed/missing `%mine-ai`/`%ai-pow` arms_.
- **9.5 Test coverage — side-by-side.** STATUS: `OPEN`. Existing tests cover ASERT and
  accumulated-work in isolation, but there is (to confirm) NO test that runs a MIXED
  ZK+AI chain and checks per-puzzle retargeting + cross-puzzle heaviness accumulation +
  heaviest-block selection end to end. **Build one** (consensus-level, constructing
  mixed-version pages so it need not prove real puzzles): assert each puzzle's target
  retargets toward its interval from its own block series, that accumulated-work sums
  correctly across the mix, and that the heavier mixed chain wins fork choice.

## 10. Node operator experience

- **10.1 First-boot cost + messaging.** First boot generates the seed table (~5 min,
  logged) then builds contexts to disk (~1-2 min, logged); subsequent boots ~3 s.
  jemalloc required; ~8 GB context files + ~39 MB seeds on disk. STATUS: `DONE`
  (messaging) — _confirm the disk-space + RAM requirements are documented for operators_.
- **10.2 Failure modes are legible.** Corrupt cache → auto delete+regenerate (logged);
  divergent build → fatal with a clear message; corrupt context file → `%fail` (logged);
  determinism mismatch → fatal at the digest check. STATUS: `DONE` (all log clearly).
- **10.3 Config knobs.** `AI_POW_VERIFIER_CACHE_CAP` (RSS vs page-in tradeoff). STATUS:
  `DONE` — _document the tradeoff + a recommended default for validators vs light nodes_.
- **10.4 Running a miner.** Mining is split into external binaries (node emits
  `%mine-zk`/`%mine-ai`, external miners consume + submit). STATUS: _pending — confirm
  the two-puzzle miner UX (which puzzle to mine, how to choose) is documented + ergonomic_.
- **10.5 Observability.** Operators need to distinguish "block invalid" from "my setup
  is broken" (the `NO` vs `%fail` split helps) and to see per-puzzle difficulty/heaviness.
  STATUS: OPEN — _recommend metrics: per-puzzle target, page-in rate, verify latency,
  liar-block rate_.
