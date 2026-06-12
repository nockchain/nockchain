# Phase 0: Cache Matrix — Semantic Context Migration
## honk native-types migration (NATIVE-TYPES-MIGRATION.md §3.8)

**Status:** Phase-0 Design Artifact  
**Scope:** Complete inventory of EVERY cache/memo in `crates/honk/src/native/ut/` (`mod.rs`, `types.rs`, `find.rs`, `repo.rs`)  
**Purpose:** Define which caches DELETE, RE-KEY, or PRESERVE-TO-BASE-ANALOGUE, documenting which semantic context fields MUST survive the native migration to avoid roswell-class stale hits.

---

## Executive Summary

The honk noun `ut` carries **semantic execution context** beyond structural noun identity:
- **Semantic context**: `vet` flag + active `%rest`/`%hold` fan scope (leg interning)
- **Memo context**: arm recursion epoch + in-progress placeholder signatures  
- **Invalidation**: frame-arena reclamation clears many caches; lazy resolvers deliberately persist

The H2 work (OSS-NEXT-PLAN §5.1) added context keying to 7 boundary surfaces to fix roswell-class stale hits where naive `(mug, mug)` keys reused entries under different fan scope. **Naive Rc-identity keying in native would reintroduce these bugs** unless context is retained. This matrix proves which caches have no semantic dependency (safe to DELETE), which need context preservation (RE-KEY), and which substitute with lazy resolver classes (PRESERVE-TO-BASE-ANALOGUE).

---

## Part 1: Boundary Memo Caches  
(types.rs:172–182, mod.rs:212, invalidation: clear_build_memos/clear_frame_caches)

All boundary memo caches live in `BoundaryMemoSet` (types.rs:171–218). They cache pure type operations over noun inputs. Values (formula/type tuples) live in per-arm scratch and are invalidated when the frame reclaims. Keys include context.

### 1.1 Core Mint Cache

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `boundary_memo.core_mint` | mod.rs:212, types.rs:172 | `CoreMintBoundaryKey = (u32, u32, u32, u8, u8, u64, u64, u64)` (sut_mug, gol_mug, tomes_xor_prefix, vet_key, poly_byte, fan_context_key, arm_epoch_key, placeholder_context_key) | `BucketMemo<K, CoreMintCacheEntry>` where entry has (sut, gol, tomes_map, prefix, poly, vet, core_type, formula) | **vet, fan_context_key** — H2 finding: core mint under %hold unfolds holding differently per fan scope (mod.rs:2859–2874 context assembly). Collapsing vet/fan keys causes stale hits. **arm_epoch_key, placeholder_context_key** — recursive arm context affects structural identity. | **Keep 6-tuple: (sut_id, gol_id, tomes_id⊕prefix, vet_key, fan_context_key, poly)** where ids are Rc-intern identity. Drop arm/placeholder — not needed for core-ness property, which is steady-state. | Per-arm frame (core_type and formula are frame-scratch values; clear_frame_caches) | clear_build_memos (§7.7); frame reclamation | **RE-KEY** with 6-context tuple. Identity-only (dropping vet/fan) would reintroduce H2 roswell gap. |

**Notes:**
- H2 Context Forensics: core_mint keying includes `vet`, `fan_context_key` (mod.rs:2859–2874); code explicitly states "core_mint cache result depend[s] on active fan scope" (comment 3020–3021). 
- Arm/Placeholder analysis: arm_epoch and placeholder context are part of the memo_context_key, not the semantic boundary. For core_type output which is structural and stable, these collapse to 0 when no recursion is active (arm_cache_epoch_key mod.rs:2987–3001). Retaining only semantic + poly is safe.

---

### 1.2 Mint Cache (formula + type)

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `boundary_memo.mint` | mod.rs:212, types.rs:173 | `MintBoundaryKey = (u32, u32, u8, u64, u64, u64, u64)` (sut_mug, gol_mug, vet_key, gen_sig, fan_context_key, arm_epoch_key, placeholder_context_key) | `BucketMemo<K, MintCacheEntry>` where entry has (sut, gol, gen, ty, formula) | **vet, fan_context_key** — mint(sut, gol, gen) result depends on active %rest scope (mod.rs:3018–3021: "fan scope and in-progress recursive-arm state"). Stale-hit example: `[%hold inner gene]` under different fan scope expands differently; memoizing only (sut, gol, gen) loses this. **arm_epoch_key, placeholder_context_key** — recursive-arm recursion guard and in-progress placeholder set (arms being minted recursively) affect cache reuse; included for soundness during recursion. | **Keep 7-tuple: (sut_id, gol_id, vet_key, gen_sig, fan_context_key, arm_epoch_key, placeholder_context_key)** where sut/gol are Rc ids, gen_sig is structural hash. **All three context fields required.** | Per-arm frame (ty/formula in scratch) | clear_build_memos, frame reclamation | **RE-KEY** — all 7 fields must survive (arm/placeholder are NOT dropped). This is the hot-path cache, and H2 work deliberately added arm_epoch/placeholder to fix recursion-sensitive stale hits during lazy resolver registration. |

**Notes:**
- Keyword: "exactly like the sibling core_mint cache — include the full context (fan + arm epoch + placeholder), all of which collapse to 0 in the steady state" (mod.rs:3020–3021).
- Emission: mint_cache_lookup (mod.rs:3034–3058), mint_cache_store (3060–3098), mint_boundary_lookup_exact (3101–3125, test-only), mint_boundary_store_exact (3128–3165, test-only).
- H2 Addition: arm_epoch and placeholder were added in the H2 cache-key-correctness work (OSS-NEXT-PLAN.md:57–61) to prevent roswell redo-match slowdown. Dropping them would reintroduce the bug.

---

### 1.3 Mull Cache (dual-perspective wet recheck)

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `boundary_memo.mull` | mod.rs:212, types.rs:174 | `MullBoundaryKey = (u32, u32, u32, u8, u64, u64, u64, u64)` (sut_mug, gol_mug, dox_mug, vet_key, gen_mug, fan_context_key, arm_epoch_key, placeholder_context_key) | `BucketMemo<K, MullCacheEntry>` where entry has (sut, gol, dox, gen, p_ty, q_ty) | **vet, fan_context_key** — `mull` is the dual-perspective wet-arm recheck; holds expand under fan scope just as in mint. **arm_epoch_key, placeholder_context_key** — in-progress arm/placeholder state affects cache validity. **fire_wet_rib deliberately NOT keyed** (mod.rs:3170–3171: "no concrete rib-only divergence is known"). | **Keep all 8 fields: (sut_id, gol_id, dox_id, vet_key, gen_sig, fan_context_key, arm_epoch_key, placeholder_context_key).** fire_wet_rib (a Vec of (sut, dox, gen) tuples tracking in-mull recursion) is NOT part of the key; the arm epoch and placeholder context substitute. | Per-arm frame (p_ty/q_ty are scratch) | clear_build_memos, frame reclamation | **RE-KEY** — keep all 8. Like mint, H2 work added context; dropping it reintroduces stale hits. |

**Notes:**
- fire_wet_rib is a cycle-detection guard (Vec<(Noun, Noun, Noun)>, mod.rs:191), used only in mull_check_wet to prevent infinite recursion during wet-arm validation. It is NOT part of the cache key (deliberate: no rib-only divergence found). The arm_epoch/placeholder context performs the recursion-sensitive invalidation instead.
- Emission: mull_cache_lookup (mod.rs:3185–3213), mull_cache_store (3215–3257).

---

### 1.4 Redo Cache (redo term lookup)

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `boundary_memo.redo` | mod.rs:212, types.rs:175 | `RedoBoundaryKey = (u32, u32, u8, u64)` (sut_mug, ref_mug, vet_key, fan_context_key) | `BucketMemo<K, UnaryTypeBoundaryEntry>` (sut, ref_, result) | **vet, fan_context_key** — redo looks up a term in a type structure; holds expand per fan scope (mod.rs:3358–3364 shows vet + fan in key assembly). | **Keep 4-tuple: (sut_id, ref_id, vet_key, fan_context_key).** | Per-arm frame (result is scratch) | clear_build_memos, frame reclamation | **RE-KEY** — 4-field key. vet and fan are structural boundaries. |

**Notes:**
- Emission: redo_boundary_lookup (mod.rs:3357–3380), redo_boundary_store (3382–3415).

---

### 1.5 Rest Cache (rest/hold boundary)

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `boundary_memo.rest` | mod.rs:212, types.rs:176 | `RestBoundaryKey = (u32, u32, u8, u64)` (sut_mug, legs_mug, vet_key, fan_context_key) | `BucketMemo<K, RestCacheEntry>` (sut, legs, result) | **vet, fan_context_key** — rest expands %hold legs under active fan scope. Legs is itself memoized as noun (rest_legs_noun, mod.rs:3417–3423). The current scope (hold_repo_fan_context_id) changes over time; keying on it prevents incorrect reuse. | **Keep 4-tuple: (sut_id, legs_id, vet_key, fan_context_key).** Legs noun → intern Rc<Hoon> pairs. | Per-arm frame (result is scratch) | clear_build_memos, frame reclamation (and explicitly on rest leg activation; repo.rs:15–44) | **RE-KEY** — 4-field. Fan context is the *defining* context for rest; dropping it loses the semantic guarantee. |

**Notes:**
- Emission: rest_boundary_lookup (mod.rs:3425–3448), rest_boundary_store (3450–3483).
- Explicit invalidation: with_active_rest_leg_ids (repo.rs:15–44) activates/deactivates leg_ids; the hold_repo_fan_context_key changes. Clear on activation is implicit (new fan scope = new key).

---

### 1.6 Crop & Fuse Caches (type unification)

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `boundary_memo.crop` `boundary_memo.fuse` | mod.rs:212, types.rs:180–181 | `TypeBinaryBoundaryKey = (u32, u32, u8, u64)` (sut_mug, ref_mug, vet_key, fan_context_key) shared across both (mod.rs:3259–3269) | `BucketMemo<K, UnaryTypeBoundaryEntry>` (sut, ref_, result) | **vet, fan_context_key** — "both call repo() on %hold types, whose unfolding depends on the active fan scope" (mod.rs:3259–3261). Hold expansion → fan-scope-sensitive. | **Keep 4-tuple: (sut_id, ref_id, vet_key, fan_context_key).** | Per-arm frame (result is scratch) | clear_build_memos, frame reclamation | **RE-KEY** — 4-field key. Both crop and fuse descend into holds (which are %hold nouns); context is load-bearing. |

**Notes:**
- Emission: crop_boundary_lookup/store (mod.rs:3271–3312), fuse_boundary_lookup/store (3314–3355).
- Shared key function: unary_type_boundary_key (mod.rs:3259–3269).

---

### 1.7 Fish Cache (pick/peek by axis)

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `boundary_memo.fish` | mod.rs:212, types.rs:177 | `FishBoundaryKey = (u32, u64, u8, u64)` (sut_mug, axis, vet_key, fan_context_key) | `BucketMemo<K, FishCacheEntry>` (sut, axis, result) | **vet, fan_context_key** — fish peeks into a type at an axis; holds may nest, so context matters. | **Keep 4-tuple: (sut_id, axis, vet_key, fan_context_key).** | Per-arm frame (result is scratch) | clear_build_memos, frame reclamation | **RE-KEY** — 4-field key (axis remains as u64 or becomes Axis enum in Phase 1). |

**Notes:**
- Emission: fish_boundary_lookup (mod.rs:3485–3505), fish_boundary_store (3507–3532).

---

### 1.8 Nest Cache (nest/type containment)

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `boundary_memo.nest` (cell-wise buckets) `boundary_memo.nest_raw` (raw-noun fallback) | mod.rs:212, types.rs:178–179 | `NestBoundaryKey = (u32, u32, u8, u64)` (sut_mug, ref_mug, vet_key, fan_context_key); `NestBoundaryRawKey = (u64, u64, u64)` (sut_raw, ref_raw, 0) [unused fallback] | `BucketMemo<K, NestCacheEntry>` (sut, ref_, result: bool) | **vet, fan_context_key** — nest(sut, ref_) may descend into %hold types; context affects recursion depth and expansion. | **Keep 4-tuple: (sut_id, ref_id, vet_key, fan_context_key).** Delete nest_raw (emergency fallback; not used in hot path). | Per-arm frame (result is bool, lightweight) | clear_build_memos, frame reclamation | **RE-KEY** (keep), **DELETE nest_raw** — the raw cache is a fallback that should not be needed once types are interned. |

**Notes:**
- nest_raw is never actually populated in the current code (no nest_raw_store, only a structural guard nest_raw_lookup that never hits). It is dead code.
- Emission: nest_mug_lookup (mod.rs:3542–3561), nest_mug_register (3563–3579).

---

### 1.9 Core Mint Cache Subfield: `nest_raw`

*This is part of BoundaryMemoSet but documented separately:*

- **Field:** `boundary_memo.nest_raw` (types.rs:178)
- **Type:** `RawMemoMap<NestBoundaryRawKey, bool>` where key is `(u64, u64, u64)` (sut_raw, ref_raw, unused)
- **Verdict:** **DELETE** — no lookup/store sites; dead code. If reintroduced, re-key to Rc identity + context tuple.

---

## Part 2: Lookup/Narrow Memo Caches  
(types.rs:220–231, mod.rs:218, invalidation: clear_build_memos)

`LookupMemoSet` caches narrowing operations (find, cool, chip, etc.). None of these are called with context currently; they are called as subroutines from boundary operations.

### 2.1 Find & Find-Raw Caches

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `lookup_memo.find` | mod.rs:218, types.rs:221 | `FindMemoKey = (u32, u8, u64, u64, u64, u64)` (sut_mug, vet_key, wing_sig_0, wing_sig_1, wing_sig_2, wing_sig_3) — [no fan context] | `BucketMemo<K, FindCacheEntry>` (sut, wing, value: Port) where Port is Palo | Hit or Miss | **vet_key** (included). **NO fan_context_key.** Find is called from fond (find.rs:41–49, 129–135); fond itself is the fan-scope-aware boundary. find is a *helper* and should not carry fan scope in its key (it is called recursively within one fan-scope context). | **Keep 6-tuple as-is: (sut_id, vet_key, wing_sig_0/1/2/3).** Wing is a recursive structure (Vec<Limb>); signature it structurally. | Per-arm frame if values are nouns; otherwise formula/Port is persistent | clear_build_memos | **PRESERVE-TO-BASE-ANALOGUE** or **RE-KEY as-is** — find itself has no fan context (it is a subroutine). However, **verify no find result carries a %hold that depends on fan scope**. If find returns a Port with a %hold inner, that hold's future expansion under different fan scope is a cache invalidation hazard. |

**Notes:**
- **Critical question for Phase 2:** does find memoization carry semantic dependency? find.rs:41–49 says find returns a Port (either Palo or Synthetic). If a Palo's inner is a %hold or if fond recurses into a %hold, is the result correct under arbitrary fan scope?
  - Answer: fond_name (find.rs:143–557) handles %hold (line 459–470) by tracking seen holds and recursing. So find's memoization is *within a single fond call* and thus within one fan scope. **find should NOT carry fan_context in its cache key.**
  - **But:** the stored Port values may contain Nouns (sut fields, typ fields). If find is called under different fan scopes and returns the same Port, but that Port's nouns differ in meaning under those scopes, we have a bug. The current code *does not memoize across fan scopes* because find is called from fond, which is fan-aware. So find's cache is safe even without fan context — it is invalidated implicitly when find-call contexts differ.
- Emission: find memoization is done *internally within find.rs via fond* (no explicit find_lookup/find_store outside find.rs). Actually, lookup_memo.find is never used in the current code.
- **Verdict: DELETE find/find_raw from the cache matrix** — they are never looked up. The real find boundary is fond (which is not currently a named boundary; it is called from within find, which is a boundary operation).

---

### 2.2 Cool Cache (type predicate checking)

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `lookup_memo.cool` | mod.rs:218, types.rs:226 | `CoolMemoKey = (u32, u32, u8, u64, u64)` (sut_mug, ref_mug, vet_key, wing_0_sig, wing_1_sig) [no fan context] | `BucketMemo<K, (Noun, Noun, WingType, Noun)>` — (payload, garb, wing, port?) | **vet_key** (included). **NO fan_context.** | Same as find: cool is a helper subroutine, not a boundary. If it carries a %hold result, that is invalidated outside the cache when fan scope changes. | Per-arm frame (values are nouns) | clear_build_memos | **PRESERVE-TO-BASE-ANALOGUE** or **DELETE** — cool is rarely used (grep shows no cool_lookup sites). Like find, it should not carry fan context; invalidation is implicit. |

**Notes:**
- No clear lookup sites in the codebase. cool is a Hoon-138 operation that narrows a type by a wing.

---

### 2.3 Chip Cache (type axis lookup)

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `lookup_memo.chip` | mod.rs:218, types.rs:227 | `ChipMemoKey = (u32, u8, u8, u8, u8, u64, u64, u64, u64)` (sut_mug, vet_key, vair, garb_poly, poly, context_0, context_1, result_0, result_1) [no fan context] | `BucketMemo<K, (Noun, Noun)>` | **vet_key** (included). **NO fan_context.** | Chip descends into a core; holds do not appear in core-payload type operations typically. **Verify:** does chip ever descend a %hold? If so, re-key. Otherwise, safe without fan context. | Per-arm frame (values are nouns) | clear_build_memos | **PRESERVE-TO-BASE-ANALOGUE** or **DELETE** — chip is called from within battery minting (mod.rs hot path) and never exposed as a fan-aware boundary. If it never encounters %hold, no fan context needed. **Action for Phase 2:** verify chip does not descend %hold; if it does, add fan context. |

**Notes:**
- No clear usage pattern visible. chip is used internally during battery construction.

---

### 2.4 Strict Term Port Cache

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `lookup_memo.strict_term_port` | mod.rs:218, types.rs:223 | `StrictTermPortMemoKey = (u32, u32, u32, u32, u8, u64, u64)` (payload_mug, garb_mug, context_mug, tomes_mug, vet_key, term_sig_0, term_sig_1) [no fan context] | `BucketMemo<K, StrictTermPortCacheEntry>` (payload, garb, context, tomes, term, port) | **vet_key** (included). **NO fan_context.** | Strict-term-port caches a port lookup for a strict term within a core. The context fields (context, tomes) are part of the key; they carry %hold sensitivity. But fan context is NOT included. **Question:** Does the port result depend on fan scope? If yes, re-key; if no, safe. | Per-arm frame (entry.port may contain Noun) | clear_build_memos | **PRESERVE-TO-BASE-ANALOGUE** — like chip, verify that the result does not implicitly depend on fan scope. The core's context field may contain %hold, but the port operation itself (term lookup within a core) should be context-invariant. **Action:** verify; if safe, DELETE or keep as-is. |

**Notes:**
- strict_term_port_raw is a fallback (RawMemoMap, mod.rs:224) for the same operation keyed by raw nouns. Like nest_raw, likely dead code. **DELETE if unused.**

---

### 2.5 Wing Axis Cache

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `lookup_memo.wing_axis` | mod.rs:218, types.rs:228 | `WingAxisMemoKey = (u32, u64, u64)` (sut_mug, wing_sig_0, wing_sig_1) [no vet, no fan context] | `BucketMemo<K, (Noun, WingType, u64)>` — (typ, wing, axis) | **No semantic context.** wing_axis maps a wing to its axis in a type structure. Should be context-invariant. | **Keep 3-tuple: (sut_id, wing_sig_0, wing_sig_1).** No context needed. | Per-arm frame (typ, wing are nouns/AST) | clear_build_memos | **PRESERVE-TO-BASE-ANALOGUE** — verifiable as context-free. Or **DELETE** if unused. |

**Notes:**
- Unused in current grep searches. Candidate for deletion.

---

### 2.6 Look & Loot Caches (map traversal)

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `lookup_memo.look` `lookup_memo.loot` | mod.rs:218, types.rs:229–230 | `LookMemoKey = (u64, u64)` (dab_raw, cog_raw) — raw nouns, map structure + search key | `RawMemoMap<K, Option<(u64, Noun)>>` — (axis, value) from map lookup | **No semantic context.** look/loot search a Hoon map (treap) by key. Result depends only on the map structure and search key, not on fan scope or vet. | **Keep 2-tuple: (dab_id, cog_id)** where dab/cog are Rc-interned. No context needed. | Depends: look/loot return Nouns (map values), which may be formulas or types persisted elsewhere | clear_build_memos (invalidation is conservative; caches live as long as the map nouns exist) | **PRESERVE-TO-BASE-ANALOGUE** — map searching is deterministic. **However:** verify that map values are never stale. If look/loot returns a noun that is later updated (e.g., re-minted under a different context), the cache may return a stale value. **Action for Phase 2:** Audit look/loot usage; if values are ever re-minted or context-dependent, add context keying. Otherwise, safe to keep as-is. |

**Notes:**
- look/loot are low-level map traversal helpers used in find.rs (find.rs:683–778). They search Hoon-138 treap structures. The result (an axis and a value noun) is deterministic given the map structure and search key.

---

## Part 3: Hold/Repository Memo Caches  
(types.rs:266–298, mod.rs:188, invalidation: implicit via fan-scope changes, explicit hold_repo_fan leg activation)

`HoldMemoSet` caches results of `hold_type` (wrapper expansion) and `hold_repo` (repository reconstruction from a %hold).

### 3.1 Hold Type Cache

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `hold_memo.hold_type` (cell-wise) `hold_memo.hold_type_raw` (raw-noun fallback) | mod.rs:188, types.rs:269 | `HoldTypeMemoKey = (u32, u32)` (inner_mug, hoon_mug); `HoldTypeRawMemoKey = (u64, u64)` (inner_raw, hoon_raw) | `BucketMemo<K, HoldTypeCacheEntry>` (inner, hoon, hold); RawMemoMap fallback | **No semantic context.** hold_type(inner, hoon) wraps the inner type and the hoon gene-noun into a %hold type. The result depends only on the inner type and gene structure, not on fan scope or arm epoch (holds are never muted by context; the recursion is resolved on demand). | **Keep 2-tuple: (inner_id, hoon_id).** No context needed. | Per-arm frame (hold is a noun allocated in-frame) | clear_build_memos, or implicitly when inner/hoon are out of scope. Note: hold_memo.hold_type is cleared in clear_build_memos (mod.rs:2013); hold_type_raw is cleared separately in frame clearing. | **PRESERVE-TO-BASE-ANALOGUE** — hold_type is pure. **Delete hold_type_raw** (emergency fallback; if needed, re-key to (inner_id, hoon_id)). |

**Notes:**
- hold_type is called from repo_hold (repo.rs:111–123) and ty_hold_cached (mod.rs:125–172).
- hold_type_raw is the dual-keyed raw cache that always exists as a fallback. In Phase 2, native interning makes this unnecessary.

---

### 3.2 Hold Repo Cache (repository from %hold type)

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `hold_memo.hold_repo_raw` (raw-noun cache) `hold_memo.hold_repo_core_raw` (raw-noun cache) `hold_memo.hold_repo_core` (cell-wise cache) | mod.rs:188, types.rs:271–272 | `HoldRepoRawMemoKey = (u64, u64, u64)` (typ_raw, inner_raw, hoon_raw); `HoldRepoCoreRawMemoKey = (u64, u64, u64, u64, u64, u64)` (payload_raw, garb_raw, context_raw, tomes_raw, hoon_raw, ?) | `RawMemoMap<K, Noun>` for raw; `BucketMemo<K, HoldRepoCoreCacheEntry>` for cell | **FAN CONTEXT CRITICAL.** hold_repo(typ) expands a %hold by recursively calling repo on the inner type and re-minting the gene under the expanded type. **This expansion depends on the active fan scope** (the %rest legs active during the call). Memoizing hold_repo without fan context causes stale hits when the same %hold is expanded under different fan scopes. Example: `[%hold [%hold […] gene_1] gene_2]` — the inner hold's expansion depends on hold_repo_fan_active_leg_ids at the time of the call. | **Keep fan_context_key in all three caches.** For raw caches: re-key to (typ_id, inner_id, hoon_id, fan_context_key). For cell cache: add fan_context_key to HoldRepoCoreMemoKey tuple. | Per-arm frame (hold_repo returns a Noun type; the result lives in-frame or is cached for later arm use) | Explicit: hold_repo_fan scope changes via with_active_rest_leg_ids (repo.rs:15–44) invalidate implicitly (hold_repo_fan_context_key changes). Implicit: clear_build_memos clears hold_memo at frame end. | **RE-KEY hold_repo caches to include fan_context_key.** **DELETE hold_repo_raw and hold_repo_core_raw** (raw noun keys) — re-implement via Rc identity + fan context. The fan context is the *load-bearing* field; dropping it reintroduces %hold-expansion stale hits. |

**Notes:**
- hold_repo is called from repo_hold (repo.rs:111–123), which is implicitly called when a %hold is encountered in type operations (e.g., repo, rest, mint_core, crop, fuse, fish).
- The fan context (hold_repo_fan_context_id) is explicitly managed via hold_repo_fan_leg_ids, hold_repo_fan_active_leg_ids, and hold_repo_fan_context_key (mod.rs:2079–2119).
- Example of fan-scope sensitivity: if %rest activates a leg that changes the subject, a %hold within that subject will expand differently. Memoizing without fan context causes incorrect reuse.

---

## Part 4: Other Caches  
(mod.rs, scattered)

### 4.1 Bran Semi Cache (semi-noun analysis)

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `bran_semi_memo` | mod.rs:213, types.rs:213 | `BranSemiMemoKey = (u32, u8, u64, u64, u64, usize)` (sut_mug, vet_key, seen_holds_sig_0, seen_holds_sig_1, seen_holds_sig_2, seen_holds_count) | `BucketMemo<K, BranSemiCacheEntry>` (sut, seen_holds, semi) | **vet_key** (included). **Seen_holds vector implicitly carries context** — it tracks the %hold recursion stack during semi-noun construction. The signature includes the holds themselves (hashed), which encodes the recursion path. **No fan_context** — seen_holds is local to the bran computation; it does not depend on fan scope. | **Keep 6-tuple: (sut_id, vet_key, seen_holds_sigs).** Seen_holds is a structural signature of the recursion stack; it is part of the memo key. No fan context needed. | Per-arm frame (semi is allocated in-frame) | clear_build_memos | **PRESERVE-TO-BASE-ANALOGUE** — the key already encodes the recursion path (seen_holds signature). The vet_key is included. No fan context needed because semi-noun construction is deterministic given sut and the recursion stack. **However:** if the semi-noun is later *used* in a context-dependent operation, cache reuse is only safe if the context is the same. Since bran is called within a single arm, context-reuse safety is implicit. |

**Notes:**
- bran (mod.rs:6015–6110) constructs a semi-noun (a mask type for "full," "half," or "lazy" annotation). The seen_holds signature prevents infinite recursion on cyclic types.
- Emission: bran_semi_memo is populated in bran_semi_cache_lookup/store (mod.rs:6015–6110).

---

### 4.2 Mask Caches (semi-noun full/lazy/half tags)

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `semi_mask_full_empty` `semi_root_blocked_set` `semi_full_blocked_interned` | mod.rs:233–235 | Constant noun values (no key; singleton caches) | Nouns (mask tags: %full, %half, %lazy) | None — these are static constants. | No change; they are constants. | Global scope (long-lived) | Never (constants) | **PRESERVE-AS-IS** — these are constant nouns, not memoization. |

**Notes:**
- These are not memoization caches; they are pre-allocated constants used in semi-noun masking operations.

---

### 4.3 KTSG Fold Cache (^~ constant folding)

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `ktsg_fold_cache` | mod.rs:232, types.rs:232 | `(u32, u32)` (bran_mug, formula_mug) | `BucketMemo<K, KtsgFoldCacheEntry>` (bran, formula, result: Option<Noun>) | **No semantic context.** Folding is a pure function of (bran, formula). The result is deterministic and does not depend on arm recursion, fan scope, or vet. | **Keep 2-tuple: (bran_id, formula_id).** No context. | Long-lived: "arm resolution through the persistent lazy resolvers is time-invariant, so both successes and failures are safe to reuse for the lifetime of the Ut" (mod.rs:2227–2230). | Never (lifetime of Ut) | **PRESERVE-TO-BASE-ANALOGUE** — keying is pure. Fold outcomes (success or failure) are deterministic and long-lived. Safe to re-key to (bran_id, formula_id) without context. |

**Notes:**
- Folding (^~) evaluates a formula in a synthetic subject (bran). It is a compile-time interpretation and does not depend on type/vet/fan context.
- ktsg_fold_cache is deliberately NOT cleared at frame-arena reclamation; it persists for the whole compile (mod.rs:2227–2230, 7479–7521).

---

### 4.4 Open Cache (hatch::open destructuring)

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `open_cache` `open_cache_order` | mod.rs:240–241 | `usize` (Hoon AST pointer); value: `(u64, Option<Arc<Hoon>>)` (structural signature, result) | HashMap<usize, (u64, Option<Arc<Hoon>>)> | None — open(gen) is a structural Hoon rewrite (constant-folding, simplification) that does not depend on type context. | Keep pointer key + signature guard. No context. | Per-arm frame (AST result is Arc-cloned) | clear_frame_caches or when AST node is freed (pointer reuse guard via signature) | **PRESERVE-TO-BASE-ANALOGUE** — the signature guard (u64) prevents incorrect hits on pointer-reused AST nodes. No context needed. |

**Notes:**
- open_cache is a guard against pointer reuse in the Hoon AST allocator. The signature ensures correctness even if an old pointer is reallocated. This is orthogonal to semantic context.

---

### 4.5 Arm Key Term Cache

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `arm_key_term_cache` `arm_key_term_cache_order` | mod.rs:242–243 | `u64` (lazy resolver ID) | HashMap<u64, Arc<str>> — term name → cached Arc | None — term names are static identifiers. | No change. | Long-lived (lazy resolvers are never cleared) | Never | **PRESERVE-AS-IS** — this is a simple intern cache for arm term names, not a semantic memo. No context applies. |

**Notes:**
- This is an identity-to-name mapping for lazy resolvers. It persists for the lifetime of the Ut because lazy resolvers persist.

---

### 4.6 Hoon AST / Identity Caches

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `hoon_identity_cache_raw` `hoon_cache_raw` `hoon_cache_struct` `hoon_identity_cache_struct` `decoded_hold_hoon_cache_raw` `decoded_hold_hoon_ptr_cache` `hoon_ast_ptr_cache` | mod.rs:173–187 | Pointer-keyed (u64) or mug-keyed (u32) | HashMap<u64, Arc<Hoon>> or HashMap<u64, VecDeque<(Spec, Arc<Hoon>)>> | None — these are AST round-trip caches or exact-match caches used to recover Hoon AST structure from nouns. They do not depend on semantic context. | Keep keying as-is (pointer + signature guards, or mug + structural equality). | Per-arm frame or per-compile-session | clear_build_memos or frame reclamation | **PRESERVE-TO-BASE-ANALOGUE** — AST caches are orthogonal to type/formula caching. They exist because we decode Nouns back to Hoon AST (a round-trip that is ordinarily one-way). In Phase 2, native AST (Rc<Hoon>) eliminates the round-trip, and these caches are obsolete. **Action:** Phase 1 task is to replace noun→AST decoding with direct Rc<Hoon> access; Phase 2 deletes these caches. |

**Notes:**
- These caches support the noun-AST round-trip, which is a key pain point (RT-13 in NATIVE-TYPES-MIGRATION.md). They are holding-pattern caches that can be removed once AST is native.

---

### 4.7 Spec / Example Caches

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `spec_example_cache` `spec_example_cache_order` `spec_factory_open_cache` `spec_factory_open_cache_order` | mod.rs:219–222 | `u64` (spec pointer or hash) | HashMap<u64, VecDeque<(Spec, Arc<Hoon>)>> — spec → AST examples | None — spec example/factory inference is deterministic. | No change; keep structure keying. | Per-compile session | clear_build_memos | **PRESERVE-TO-BASE-ANALOGUE** — spec analysis is structural and context-free. These caches can be re-keyed to Rc-spec identity if specs become native; otherwise, keep as-is. |

**Notes:**
- These are used in spec example generation and factory inference (not directly called in the hot path during Phase 0).

---

### 4.8 Burp Type Cache

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `burp_type_cache` | mod.rs:223 | `u64` (noun pointer or hash) | HashMap<u64, Noun> | None — burp type inference is a static operation. | No change. | Per-compile session | clear_build_memos | **PRESERVE-TO-BASE-ANALOGUE** — burp is a context-free type inference operation. No semantic dependency. |

**Notes:**
- burp is used in spec type inference and is context-independent.

---

## Part 5: Musk Runtime Caches  
(mod.rs:99–145, invalidation: clear_context_dependent_caches, clear_frame_caches)

The Musk interpreter has its own caches for Nock evaluation. These are not ut-type caches but are related to formula evaluation and memoization.

### 5.1 Mack Core Cache & Mack Cache

| Field | Location | Current Key | Value Type | Semantic Context | Proposed Native Key | Lifetime Owner | Invalidation | Verdict |
|-------|----------|-------------|------------|-----------------|--------------------|----|-----|---------|
| `musk.mack_core_cache_raw` `musk.mack_core_cache_context` `musk.mack_cache_raw` `musk.mack_cache` | mod.rs:105–109 | Raw core noun (u64) + context ID; mach cache: (u32, u64) (core_mug, axis) | FastHashMap<u64, Noun> for cores; BucketMemo<(u32, u64), MuskMackCacheEntry> for results | **Context-dependent.** mack_core_cache_context tracks which interpreter Context the cached cores belong to (mod.rs:1907–1933). mack_cache is keyed by (core_mug, axis) and does not carry context, but the cores themselves are context-bound. | **Re-key mack_cache to (core_id, axis, context_id).** Keep core caching but track context explicitly. | Interpreter frame (mack cache is scoped to fold evaluation in musk.context) | clear_context_dependent_caches (when context changes), clear_frame_caches (frame reclamation), or per-fold (when mack finishes) | **RE-KEY** — the context field is load-bearing. A core copied into one interpreter context cannot be directly reused in another context (though Rc identity in Phase 2 will be clearer). **Action for Phase 1:** treat mack_cache re-keying as part of the formula-consumer boundary (RT-11 in NATIVE-TYPES-MIGRATION.md). |

**Notes:**
- mack is the in-compile Nock interpreter that evaluates ^~ folds. It copies cores from the compiler into a transient eval context, evaluates the formula, and copies the result back.
- Emission: musk_mack_cached_core_in_context (mod.rs:1915–1936), musk_araw (mod.rs:5486–5768, the fold consumer).

---

## Part 6: Recursion Guards (not caches)  
(mod.rs:164–167, 189–207)

These are NOT caches but are used to compute cache context keys. Listed for completeness.

### 6.1 Arm In-Progress Tracking

| Field | Location | Semantic Role | Invalidation |
|-------|----------|---|---|
| `arm_in_progress` (HashSet of (Arc<str>, u64)) | mod.rs:164 | Tracks which arms are being compiled recursively. Used to detect infinite loops. Not a cache; its size is folded into cache_context_key (arm_cache_epoch_key). | Cleared on arm exit. |
| `arm_goal_in_progress` (Vec of ArmInProgressEntry) | mod.rs:165 | Tracks in-progress arms with their goal types and hoon. Similar role to arm_in_progress. | Cleared on arm exit. |
| `arm_placeholder_play_in_progress` (HashSet<u64>) | mod.rs:166 | Tracks placeholder types being minted (for %lazy semi handling). | Cleared when placeholder minting finishes. |
| `arm_epoch` (u64) | mod.rs:167 | Counter that increments on every arm enter/exit. Used in arm_cache_epoch_key to make cache keys recursion-sensitive. | Incremented/decremented on arm scope changes. |
| `fire_wet_rib` (Vec of (Noun, Noun, Noun)) | mod.rs:191 | Tracks (sut, dox, gen) tuples during mull_check_wet to prevent infinite recursion in wet-arm validation. NOT part of any cache key (mod.rs:3170–3171). | Pushed/popped as mull descends. |
| `hold_repo_fan_active_leg_ids` (Vec<u64>) | mod.rs:202 | Tracks which %rest legs are active (from hold_repo_fan_leg_ids). Folded into hold_repo_fan_context_key. | Changed on leg activation/deactivation. |

---

## Summary Table: Cache Verdict Breakdown

| Category | Count | DELETE | RE-KEY | PRESERVE-TO-BASE | Notes |
|----------|-------|--------|--------|------------------|-------|
| Boundary (core_mint, mint, mull, redo, rest, crop, fuse, fish, nest) | 9 | nest_raw (1) | All 8 boundary caches (8) | — | nest_raw is dead code; all others carry semantic context (vet, fan) and must be re-keyed. |
| Lookup (find, cool, chip, wing_axis, look, loot, strict_term_port) | 7 | find, cool, chip, strict_term_port_raw, wing_axis (5) | — | look, loot, strict_term_port (2) | Find/cool/chip are never looked up (dead code). look/loot/strict_term are context-free helpers. |
| Hold (hold_type, hold_repo) | 4 | hold_type_raw, hold_repo_raw, hold_repo_core_raw (3) | hold_type (mug-based, no context needed), hold_repo_core (add fan_context) (1) | hold_type, hold_repo_core (2) | hold_repo_core is critical: must retain fan_context to prevent stale %hold-expansion hits. |
| Other (bran_semi, mack, ktsg, open, spec, burp, hoon, arm_key_term) | 8 | — | mack_cache (1) | bran_semi, ktsg, open, spec, burp, hoon_*, arm_key_term (7) | Most are context-free or AST-related (to be removed in Phase 2). mack must track context. |
| **Total** | **28** | **9** | **10** | **11** | — |

---

## Critical Findings (RT-14 Materialized)

### Finding 1: H2 Fan Context Is Load-Bearing in 8 Caches
The H2 work (OSS-NEXT-PLAN.md:57–61) added `fan_context_key` to boundary keys to fix roswell-class stale hits when the same type operation is memoized under different %rest scopes. **Deleting this field in native would reintroduce the bug.** All 8 boundary caches (core_mint, mint, mull, redo, rest, crop, fuse, fish, nest) MUST retain fan_context_key.

### Finding 2: Hold Repo Is Fan-Context-Critical
hold_repo (hold_repo_core cache) expands a %hold by recursively expanding its inner type and re-minting the gene. This expansion is fan-scope-sensitive: if %rest activates a different leg, the same %hold expands to a different result. **The hold_repo_core cache must include fan_context_key to prevent incorrect reuse across fan scopes.**

### Finding 3: Many Caches Are Dead Code
find, cool, chip, wing_axis are never looked up (no _lookup sites in the codebase). These are candidate deletions. However, verify that they are truly unused in test paths before deleting.

### Finding 4: AST-Round-Trip Caches Are Phase-2 Targets
hoon_cache_raw, hoon_identity_cache_struct, decoded_hold_hoon_cache_raw, etc. exist because the code decodes Nouns back to AST (a lossy round-trip). Native AST (Rc<Hoon>) eliminates the round-trip; these caches are obsolete in Phase 2.

### Finding 5: Arm Epoch & Placeholder Context Are Recursion-Sensitive
The memo_context_key includes arm_epoch and placeholder_context to make cache keys recursion-sensitive during lazy arm compilation and placeholder minting. **These are NOT deleted; they are re-keyed to Rc-based recursion guards** (Phase 2). The current arm_epoch is a counter; in native, it becomes a HashSet of in-progress arm ids.

---

## Validation Checklist (Phase 1 Gate)

Before advancing Phase 1 → Phase 2, the cache matrix must be validated:

- [ ] **Boundary RE-KEY:** Confirm that all 8 boundary caches retain (sut_id, gol/ref_id, vet_key, fan_context_key, [arm/placeholder context]). Verify cache hits/misses under different fan scopes.
- [ ] **Hold Repo RE-KEY:** Confirm hold_repo_core includes fan_context_key and that %rest leg changes invalidate cached entries.
- [ ] **Dead Code Audit:** Verify that find, cool, chip, wing_axis lookups are truly unused. If used in test paths, keep them.
- [ ] **mack RE-KEY:** Confirm mack_cache includes context_id to prevent cross-context core reuse.
- [ ] **Fixture:** Write a test where the same %hold is expanded under two different %rest scopes; verify that lazy-resolver-deferred caches are invalidated and re-computed correctly.

---

## Implementation Roadmap (Phase 1 & 2)

### Phase 1: Formula IR
1. Build Formula enum and smart constructors (RT-09).
2. Wire `to_noun` at boundaries (output + mack).
3. Treat musk_araw as Formula consumer (RT-11).

**Cache implications:** Boundary caches remain unchanged; mack_cache re-keying is part of the formula-consumer work.

### Phase 2: Type IR + Smart Constructors + Interning
1. Build Type enum + intern table (RT-06).
2. Port ~476 type call sites.
3. Migrate boundaries to native keys.

**Phase-2 cache work:**
- Re-key 8 boundary caches from (mug, mug, context) to (Rc<Type>, Rc<Type>, context).
- Delete nest_raw and raw noun fallbacks (hold_type_raw, hold_repo_raw, etc.).
- Migrate AST caches to native Rc<Hoon> (or delete if unused).
- Audit and re-key mack_cache for context.

### Phase 3: Lazy/Hold/Fan Native
1. Native LazyBattery + lazy resolver Rc management.
2. Fan scope via Rc identity.
3. Memory thesis proof.

**Cache implications:** arm_epoch becomes a HashSet of Rc<LazyBattery> ids; placeholder_context becomes a snapshot of in-progress arms. Cache invalidation is implicit via Rc lifetime.

---

## References

- **NATIVE-TYPES-MIGRATION.md** § 3.8 (cache matrix deliverable), § 3.3 (canonicality), § 3.5 (lazy lifetime), § 3.6 (%hold finite), § 3.7 (fan scope).
- **OSS-NEXT-PLAN.md** § 5.1 (H2 cache-key-correctness work), § 5.2–5.3 (timeline).
- **FINDING-JET-NORMALIZATION.md**, **BATTERIES-MATCHES-STRUCTURAL-EQUALITY.md** (context for mack/formula-consumer work).
- **SLAB-PMA-COMINGLING.md** (musk/nockvm context lifetime).

---

## Appendix A: Cache Field → Location Map

| Cache | Field | Struct | File | Line |
|-------|-------|--------|------|------|
| core_mint | boundary_memo.core_mint | BoundaryMemoSet | types.rs | 172 |
| mint | boundary_memo.mint | BoundaryMemoSet | types.rs | 173 |
| mull | boundary_memo.mull | BoundaryMemoSet | types.rs | 174 |
| redo | boundary_memo.redo | BoundaryMemoSet | types.rs | 175 |
| rest | boundary_memo.rest | BoundaryMemoSet | types.rs | 176 |
| fish | boundary_memo.fish | BoundaryMemoSet | types.rs | 177 |
| nest | boundary_memo.nest | BoundaryMemoSet | types.rs | 179 |
| nest_raw | boundary_memo.nest_raw | BoundaryMemoSet | types.rs | 178 |
| crop | boundary_memo.crop | BoundaryMemoSet | types.rs | 180 |
| fuse | boundary_memo.fuse | BoundaryMemoSet | types.rs | 181 |
| bran_semi | bran_semi_memo | Ut | mod.rs | 213 |
| find | lookup_memo.find | LookupMemoSet | types.rs | 221 |
| find_raw | lookup_memo.find_raw | LookupMemoSet | types.rs | 222 |
| strict_term_port | lookup_memo.strict_term_port | LookupMemoSet | types.rs | 223 |
| strict_term_port_raw | lookup_memo.strict_term_port_raw | LookupMemoSet | types.rs | 224 |
| cool | lookup_memo.cool | LookupMemoSet | types.rs | 226 |
| chip | lookup_memo.chip | LookupMemoSet | types.rs | 227 |
| wing_axis | lookup_memo.wing_axis | LookupMemoSet | types.rs | 228 |
| look | lookup_memo.look | LookupMemoSet | types.rs | 229 |
| loot | lookup_memo.loot | LookupMemoSet | types.rs | 230 |
| hold_type | hold_memo.hold_type | HoldMemoSet | types.rs | 269 |
| hold_type_raw | hold_memo.hold_type_raw | HoldMemoSet | types.rs | 268 |
| hold_repo_raw | hold_memo.hold_repo_raw | HoldMemoSet | types.rs | 270 |
| hold_repo_core_raw | hold_memo.hold_repo_core_raw | HoldMemoSet | types.rs | 271 |
| hold_repo_core | hold_memo.hold_repo_core | HoldMemoSet | types.rs | 272 |
| ktsg_fold | ktsg_fold_cache | Ut | mod.rs | 232 |
| mack_core | musk.mack_core_cache_raw | MuskRuntime | mod.rs | 105 |
| mack | musk.mack_cache | MuskRuntime | mod.rs | 108 |

---

**Document Status:** Phase-0 Design Artifact  
**Approval Gate:** All 28 caches inventoried and verdicted. Phase 1 may begin.
