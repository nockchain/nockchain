# Atomic-flip execution tracker (native-types migration)

> UPDATE 2026-06-28: RESOLVED — the flip landed and the memory goal is MET. Native
> hoon-138 mint now COMPLETES (~40 s release) with BOUNDED RSS and is
> byte-identical to hoonc (both 2,286,744 B, `cmp` clean); the OOM / ~61 GB
> non-convergence / "do not ship Phase 1 alone" regressions tracked below are all
> resolved (frame arena + mack-cache cap + bottom-up interning + the ++vast
> doc-anchoring port). Enforced by `crates/honk/tests/native_parity_138.rs`
> (release-only gate, default build vs `HONK_NATIVE_PARITY=1`, byte-compared);
> shadow_gate still PASS, compiler_mint 69/0. The dated journey below is preserved
> intact as the record of how the regression was diagnosed and bounded.

Resumption anchor for the atomic replace (the FLIP): types become native
`Rc<Type>` as the working representation of `mint`/`play`; nouns are materialized
only at boundaries via `Type::to_noun`. This survives context resets — **update
the STATE section every turn.**

## Strategy: monotonic native-region expansion (compiles at every step)

Flip producers one connected step at a time to RETURN `Rc<Type>` (the type slot;
the formula slot stays `Noun`). At the current boundary, convert:
- incoming noun types → native via `intern::native_of(noun, space)` (a not-yet-
  flipped caller still hands us a noun `sut`);
- outgoing native types → noun via `native.to_noun(slab)` (a not-yet-flipped
  caller still expects a noun).

Each step compiles (boundary conversions bridge the gap) and the native region
grows; the boundary (and thus the transient noun (re)builds) shrinks. The memory
win grows monotonically — internal deepened subjects become shared `Rc`. At
completion the boundary is the OUTPUT only, and noun type construction is gone.

VALIDATION each step: `crates/honk/test-assets/native-parity/shadow_gate.sh`
(fast fixtures, byte-identical output). Full kernel byte-parity at completion.
Do NOT run full-kernel flag-on as a routine gate (O(n^2) until flipped).

## Conventions

- Type slot: `Rc<Type>` (alias `NRc<NTy>` in ut/mod.rs). Formula slot: `Noun`.
- mint family returns `(Rc<Type>, Noun)` = (type, formula); play family returns
  `Rc<Type>`.
- Construction: use the `_n` constructors' native (`ty_*_n(...).1`,
  `cell_type_n(...)?.1`) for now (transient double-build of the discarded noun;
  add native-only `cons_*` constructors later as an optimization). Collapses are
  already mirrored in the `_n` ctors.
- Boundary bridges: `native_of(noun, &slab.noun_space())?` (noun→native),
  `native.to_noun(self.slab)` (native→noun).
- Type consumers (nest/fond/repo/type_*_parts/wrap_type decoders) still take
  nouns for now; a flipped producer feeding a consumer `to_noun`s at the call.
  Consumers convert to read `Rc<Type>` in a later pass (drops those `to_noun`s).

## Ordered checklist (leaf producers → spine → consumers → boundary)

1. [DONE] play_core -> Rc<Type>  (callers: play_inner BarCen/BarPat)
2. [DONE] play_inner / play -> Rc<Type>. Arms: delegating forward native; leaf
   arms cons_*/native_of; helper arms bridge via `pb`. The ~40 external self.play
   callers + play_* helpers' internal self.play renamed to `play_noun` (= play +
   to_noun) so they keep compiling unchanged. Gate PASS, tests green.
3. [ ] play_* helpers -> Rc<Type>  (drop `pb`/`play_noun` bridges as each flips)
4. [DONE] mint_core -> (Rc<Type>, Noun). nice/core_mint_cache stay noun (cache
   bridged via native_of on hit); native built bottom-up via ty_core_n.
5. [DONE] mine -> (Rc<Type>, Noun). wrap_type/nice bridged (to_noun->...->native_of).
   Its 2 callers (mint_inner BarCen/BarPat) to_noun the type slot.
6. [ ] mint_inner / mint -> (Rc<Type>, Noun)  (78 self.mint callers -> mint_noun)
7. [ ] mint_* helpers -> (Rc<Type>, Noun)
8. [ ] nice / wrap_type -> Rc<Type>
9. [ ] type consumers (nest/fond/repo/type_*_parts) read Rc<Type>; drop to_noun shims
10. [ ] boundary: emit nouns only at output + typed-Dynock; delete noun ty_* ctors
11. [ ] full kernel byte-parity; delete _n duplicates / dead noun paths

## LAST MILE (2026-06-20) — frame-arena-default LANDS the memory win; kernel blocked by a fond/peek corpus-gap bug

UPDATE 2026-06-28: RESOLVED — the fond/peek corpus-gap blocker described here is
fixed; full native hoon-138 mint completes ~40 s with bounded RSS and is
byte-identical to hoonc (2,286,744 B). See
`crates/honk/tests/native_parity_138.rs`. The 2026-06-20 blocker analysis below is
retained as the diagnosis record.

frame-arena-default committed (602334f1: force_frame_arena=true on the binary's
build-context Ut). Dumb kernel: COMPLETES in 153s at 10.1GB (vs >900s/61GB
without) == pre-flip memory; time ~3.3x pre-flip (46s) — down from ~20x. Fixtures
byte-parity PASS with frame-arena-default; lib/compiler_mint use the non-binary
compile path so unaffected.

BLOCKER (kernel correctness, NOT memory/frame-arena): with the frame arena now
reaching ztd/one.hoon (the no-frame path OOMed before getting there), the native
compile errors. Instrumented (HONK_FIND_TRACE) to the exact failure:
  arm poly: arm $: find failed for wing [Parent(0, None), Axis(12)], way=Rite,
  sut_tag=core, frame=true.
The subject is a VALID core (native payload is a heap Rc, survives frames), so
this is a native fond/peek LOGIC bug on this wing shape — NOT a frame-arena
reclaim. Axis(12) navigates tail->head->head = into the core's PAYLOAD (the
deepening subject); then Parent(0, None) (skip=0, name=None) resolves via
fond_name. fond returns Pony::Unmatched/Void -> find errors. The fixtures +
compiler_mint (69/0) do NOT exercise this shape: a CORPUS COVERAGE GAP. There may
be MORE such kernel-only fond/peek/skin bugs behind it (the corpus passing does
not imply the kernel compiles).
NEXT (correctness, gates the perf islands): debug native fond_name / peek for
[Parent(0,None), Axis(12)] on a core payload — compare to the pre-flip noun
fond/peek semantics (git show the pre-C6+C9 fond at 066151ea~1). Candidates:
fond_name's name=None (bare Parent) path, or peek into a core payload returning
void at a deep axis. Then re-run the dumb-frame-arena repro (153s) and iterate on
any further kernel-exposed bugs until full dumb byte-parity vs /tmp/dumb_preflip.jam.
THEN the CPU islands (repo_hold cache re-key / redo / fine) to close the 3.3x time
gap. The islands are MOOT until the kernel compiles correctly.

## RESOLUTION (2026-06-20) — memory regression SOLVED by the frame arena; CPU tail remains

UPDATE superseding the "severe regression" panic below. Root-caused via
instrumentation (the earlier hypotheses — coil jamming, boundary bridges as the
MEMORY hog — were WRONG; proven: leaf jam+cue is only ~1GB, intern_node <500k):
the 61GB was PER-ARM SCRATCH accumulating in the grow-only NounSlab. The H7 FRAME
ARENA (HONK_FRAME_ARENA, built but opt-in) reclaims it. Results on dumb:
  - no frame arena:   >900s, 61GB (regression).
  - frame arena + base-resident to-noun memos (committed 43fbc4de): peak RSS
    ~10GB == PRE-FLIP's 10.2GB. MEMORY REGRESSION SOLVED.
The frame-arena lifetime bug (TO_NOUN_MEMO/LEAF_MEMO caching frame-resident nouns
-> dangling "unknown tag" on pop) is fixed by copy_to_base'ing memo values to base
(no dangling, no per-pop re-lowering; no-op on the default path). Byte-parity PASS
with AND without the frame arena (shadow_gate); compiler_mint 69/0.

REMAINING = CPU TIME (not memory): dumb with the frame arena is ~4x+ pre-flip (46s
-> >200s) at bounded memory. Profile (frame-arena binary, sample 60s): the cost is
the boundary BRIDGES jam (10119) + cue (8603) + native_of/intern_type_noun (6878)
+ copy_into (5138); copy_to_base itself is negligible (123). native_of is only ~70
calls but each lifts a DEEPENING noun type, jamming every leaf — i.e. the LAST
noun islands (repo_hold/rest_inner, the redo/wet SCC, fine()'s noun typ) still
native_of/jam/cue deepening types. The frame arena made that memory-safe but not
free. This is now a CONVERGENT, finite tail (not the pervasive O(N^2) it looked
like before the frame arena bounded memory).
NEXT (CPU): nativize repo_hold/rest_inner + redo SCC + fine() so they no longer
native_of/jam/cue deepening types (eliminates the jam/cue/native_of hot frames);
then wire the frame arena ON by default for the kernel path; then full
dumb byte-parity vs /tmp/dumb_preflip.jam + the perf gate. The native-type
migration is byte-parity-correct and the memory goal is achieved; speed is the
last mile.

## DECISIVE BASELINE (2026-06-20) — earlier panic, now superseded by the RESOLUTION above

Measured the dumb-kernel compile (hoon/apps/dumbnet/outer.hoon, --prelude
hoon/common/hoon.hoon), same machine, golden byte-match confirmed both sides:
  - PRE-FLIP  (e6e653e2 = "native-types INC6", native SHADOW / noun-primary):
      46.17 s,  10.18 GB peak RSS,  COMPLETES (golden matches).
  - POST-FLIP (Phase 1 + Phase 2, native-PRIMARY, this branch):
      >900 s (timeout, did NOT complete),  61.07 GB peak RSS.
=> The flip regressed the real kernel compile ~20x in time and ~6x in memory, and
it no longer completes. This is worse than the original 32 GB OOM it set out to fix.

ROOT CAUSE: the boundary form makes native `Rc<Type>` the primary representation
but keeps a noun side reachable via live_to_noun/native_of bridges + jammed leaves.
Every place a DEEPENING type is lowered to a noun (the remaining noun bridges:
repo_hold's rest/Hold cache-key live_to_noun(&subject) [explicitly deferred] +
fork-result native_of, the redo/fire noun paths, noun-keyed caches) caches an
O(size) noun in the per-compile, never-freed TO_NOUN_MEMO. Summed over the
deepening chain that is O(N^2) bytes -> 61 GB. The grow-only NounSlab + unbounded
per-compile to-noun memo is an inherently O(N^2) memory model while ANY noun bridge
touches deepening types.

NON-CONVERGENCE: 7 profiling-driven fixes (cache re-keys, core threading, Phase-2
coil context, play_* native, bran native) each removed their profiled hotspot but
the regression persisted (time/memory stayed catastrophic) — the next bridge took
over each time. Incremental hotspot removal has NOT converged.

STRATEGIC DECISION POINT (do not blind-grind further): the byte-parity native-type
migration is CORRECT but the native-primary boundary-form strategy, as implemented,
is a severe perf/memory regression. Options: (A) eliminate EVERY remaining
deepening-type->noun lowering — next known lever is the repo_hold/rest/Hold cache
native re-key (the deferred subject-lowering) — but convergence is uncertain and
the grow-only-memo model is the deeper issue; (B) rethink the memory model (bound
or free the to-noun memo; don't bridge deepening types at all); (C) the e6e653e2
noun-primary shadow baseline (46 s/10 GB, completes) was materially BETTER — re-
evaluate whether native-primary is the right premise, or target the original 32 GB
OOM case more surgically. Needs a human call before more engineering.

## PERF DIAGNOSTIC LOG (2026-06-20) — Phase 1 + Phase 2 done; chasing the dumb O(N^2)

Phase 1 (native skeleton) + Phase 2 (native coil context) are committed +
byte-parity-exact (shadow_gate PASS, compiler_mint 69/0, lib 122/0/1 at every
step). The dumb kernel compile is still slow (>200s timeout). Profiling-driven
fixes applied (each removed its profiled hotspot but total time stayed ~198s — the
classic pervasive-O(N^2) / can't-see-under-the-timeout signature):
  - C-final.1b/4a: re-key mint/core_mint/mull/fuse/crop/fish caches native.
  - C-final.4b: thread native core through the arm-battery (no per-arm native_of).
  - Phase 2: Core carries context as shared Rc (no per-core context jam).
  - Phase 2 tail: play_* helpers return native (drop play_to_noun/pb round-trip);
    bran_canonical_semi native (drop repo_noun + blow_ktsg's live_to_noun(&sut)).
KEY DATA: compiling a tiny input with `--prelude hoon.hoon` takes ~1.3s (the
prelude loads from embedded cold-state, NOT recompiled) — so the O(N^2) is in
`outer.hoon` (the dumbnet kernel) + its imports (ztd/...), NOT the hoon.hoon
prelude. The time has been STABLE at ~198s across all fixes (each capped at the
200s timeout, so the true time was never observed). NEXT: a 900s completion run
(in flight) for the REAL time + peak RSS + byte-parity, and the pre-flip
(e6e653e2) baseline, to determine whether this is a flip regression or a
pre-existing slow/large kernel compile — and whether the MEMORY (the original OOM
goal) is bounded even if time is not yet competitive. Until those numbers land, do
not conclude. The architectural migration (native types end-to-end) IS done; the
open question is purely the kernel-compile perf profile.

## CURRENT STATUS (2026-06-20) — correctness COMPLETE; perf tail remaining

UPDATE 2026-06-28: SUPERSEDED — the perf/memory tail described in this section
(and the "do NOT ship Phase 1 alone" caution at its end) is RESOLVED. Native
hoon-138 mint now completes ~40 s with bounded RSS and is byte-identical to hoonc
(2,286,744 B); see `crates/honk/tests/native_parity_138.rs`. The 2026-06-20
snapshot below is kept as the point-in-time record.

The functional migration is DONE: mint/play + EVERY type consumer (repo/peek/
wrap_type/fuse/crop/miss/nest/mull/fish/find/take/fond/gain/lose/cool/chip) operate
on native `Rc<Type>`. Committed + byte-parity-verified at each step (shadow_gate 4
fixtures + compiler_mint 69/0 single-threaded semantic-parity incl metamorphic +
strict-source + ket-variance + representative; full lib 122/0/1). Commits this
push: C8 4327128, C7 cfce3f6b, C6+C9 066151ea (+ the live_reset keystone that
fixed the entire cross-compile aliasing class — compiler_mint 12-15 failing -> 0),
C-final.1a 6a29ffc7 (mint family native), 1b 346a8e39 (mint/core_mint/mull caches
native-keyed), .2 62ec3778 (play sut native), .4a 4ed24dd4 (fuse/crop/fish caches
native + repo/miss bridge drops).

PERF WIN BLOCKED ON PHASE 2 (pivotal finding, 2026-06-20, proven by profiling):
the dumb kernel compiles O(N^2) (>250s, still inside the prelude at timeout) and
this is a Phase-1 REPRESENTATIONAL limit, NOT a remaining noun-bridge. ROOT CAUSE:
`core_context_from_payload` returns the payload = THE DEEPENING SUBJECT, and the
native `Type::Core { payload: Rc, coil: Leaf }` carries the coil (which contains
that context) as a JAMMED Leaf. So every native Core construction
(ty_core_n/cons_core -> Leaf::from_noun -> Jammer::jam) jams a FRESH COPY of the
deepening subject per core -> O(N^2) jam time + O(N^2) bytes. Pre-flip the noun
coil POINTED to the shared subject noun (no copy, O(1)); Phase-1's coil-as-Leaf
UN-SHARES it. Profiling (sample 70s) confirms: self-time is dominated by jam/cue/
copy_into/IntMap (the jammer), via ty_core_n->Leaf::from_noun->Jammer::jam and
native_of->Type::from_noun->Leaf::from_noun; native_of itself is only ~5%. The
cache re-keys (1b/4a) and the per-arm native_of removal (4b) were real cleanups
but addressed the WRONG cost (~5%), so the dumb kernel is unchanged.
CONSEQUENCE: Phase 1 is byte-parity-correct + kills the noun-rep bug class + makes
the type SKELETON native (payload/cell/face/hint/hold-subject chains shared as Rc),
but it is a PERF REGRESSION vs pre-flip for core-heavy code (the coil context is
duplicated/jammed per core) and is NOT shippable as-is. THE WIN REQUIRES PHASE 2:
re-architect `Type::Core` to carry the coil CONTEXT as a shared native `Rc<Type>`
child (and garb/battery/tomes as leaves), so the deepening context is shared +
hash-consed, never jammed. This also lets the intern table dedup across cores. Same
treatment later for `Fork{set}` and `Hold{gene}` (Phase-2 leaf nativization, per
the original migration plan §3.3). Phase 2 is a substantial project (Type enum +
from_noun/to_noun + intern node_eq/hash + every coil consumer peek/nest_core/
core_dox/fond/fire/coil_parts-callers + the constructors) — comparable to one
consumer-flip phase. Until Phase 2, the branch is byte-correct but slower than
master on core-heavy kernels; do NOT ship Phase 1 alone.

(superseded) earlier perf plan — the noun-bridged paths below are NOT the
dominant cost; left for reference:
  - repo.rs repo_hold / rest_inner: native_of(the hold-expansion fork RESULT) +
    the repo Hold-arm still lowers subject/gene/typ via live_to_noun for the
    noun-keyed leg_id intern + rest_boundary cache key. (rest_inner now threads
    the native leg subject to play — 4a — but the result lift + cache keys remain.)
  - wet.rs redo SCC (redo_dext/redo_sint/redo_done + redo_wet_payload): fully
    noun-based, calls repo_noun/peek_noun/miss_noun/nest_noun -> native_of per call.
  - rest/Hold caches (rest_boundary, repo_raw/hold_type/hold_repo) still noun/mug-
    keyed -> lower the deepening subject for keys.
REMAINING PERF PLAN (each: build + compiler_mint 69/0 + shadow_gate, then re-profile
+ dumb): (i) nativize the redo (wet) SCC (drop repo_noun/peek_noun/miss_noun/
nest_noun -> native; thread native sut); (ii) nativize repo_hold/rest_inner fully
(build the expansion fork without native_of'ing the deepening result; re-key the
leg_id + rest_boundary on native identity); (iii) re-key rest/Hold caches native;
(iv) THEN dumb byte-parity vs /tmp/dumb_preflip.jam (19873112 B) + perf (must drop
well under the pre-flip baseline; NounSlab no longer grows monotonically).
CLEANUP (after the win): delete the now-dead bridges (repo_noun/peek_noun/
wrap_type_noun/fuse_noun/crop_noun/miss_noun/nest_noun/find_noun/fend_noun/
feel_noun/gain_noun/lose_noun/cnts_base_port_noun/mull_noun + nest_mug_*) + the
old noun boundary caches + the noun ty_* ctors/_n variants + SKELETON decoders
(keep leaf decoders + live_leaf_to_noun = Phase-1 residue).

## STATE (update every turn)

- Branch: feature branch (non-compiling intermediate accepted, but kept compiling
  so far via the boundary-bridge technique).
- Done: native IR boundaries (Type/Formula to_noun+from_noun), intern table,
  `_n` constructor + wrapper vocabulary, intern accessors
  (live_intern/native_of/assert_native_eq).
- Gate: `shadow_gate.sh` now compares fixture output vs FIXED `flip-baselines/*.jam`
  (the flag no longer changes output once producers return native). PASS.
- Steps 1+2 DONE: `play_core` + `play`/`play_inner` return `Rc<Type>`. Bridges:
  `cons_cell`/`cons_void`/`cons_noun` (native leaf ctors), `pb` (helper noun ->
  native), `play_noun` (native play -> noun for not-yet-flipped callers). Byte
  parity PASS, native tests green, lib + bin compile.
- Steps 4-5 DONE: `mint_core` + `mine` return `(Rc<Type>, Noun)`; nice/cache stay
  noun (bridged), callers (mint_inner BarCen/BarPat) to_noun the type slot.
- Compiles: YES.

KEY FINDING (2026-06-19, corrected): the migration is CONSUMER-DOMINATED, and the
producer-output flips have reached the boundary of what's cheap.
- "nice-first" was WRONG: `nice` has ~52 callers (nearly every mint_* helper +
  inline arm), all passing/consuming NOUN. Flipping nice alone cascades to 52
  bridge sites with no benefit (reverted). nice can only flip together with its
  callers.
- ROOT shape: the type is consumed pervasively by NOUN-based code — nice, nest,
  fond, fish, peek, gain, lose, fuse, crop, the whole type-algebra, and the
  type_*_parts decoders. ANY native type bridges back to a noun (to_noun) for
  these. So nouns are still BUILT (transiently, per consumer call) until the
  CONSUMERS read native. => no memory win, and every further producer flip just
  adds bridges, until the consumer subsystem is native.
- Therefore the remaining BULK = rewrite the type-consuming subsystem to operate
  on `Rc<Type>` (match the enum) instead of decoding nouns: the type_*_parts
  decoders first (the leaves of consumption), then peek/gain/lose/nest/fond/fish/
  fuse/crop/wrap_type, then nice. Once consumers are native, the producer natives
  (play/mint_core/mine, already done) flow straight in with no to_noun, the sut
  input can thread native (shared subject => the O(N^2)->O(N) win), and the noun
  ty_* ctors can be deleted. This is the large core of the migration (~500+ lines
  of type algebra), best done decoder-leaves-first, each validated by shadow_gate.

DONE producer spine: play_core, play/play_inner, mint_core, mine -> native
(committed, compiling, byte-parity). play_* and mint_* helpers + mint_inner/mint
dispatch remain noun (bridged) and are best flipped AFTER the consumer subsystem
so they don't double-bridge.

CONSUMER FLIP (mapped 2026-06-19 by consumer-flip-map workflow; leaves-first,
flip-in-place + bridge; decoders become enum matches retired bottom-up). Plan
steps and status:
  C1 [DONE] repo/repo_hold -> native (repo_noun bridge for 27 callers).
  C2 [DONE] peek -> native (Core lowers coil leaf to coil_parts/garb_vair; Fork lowers
         set to fork_set_options; Hold -> repo native). ~4 callers.
  C3 [DONE] wrap_type -> native (needs collapse-aware cons_core/cons_face/cons_hint).
  C4 [DONE] fuse/fuse_inner -> native (fitz stays noun; nest bridged; caches keep
         to_noun mug keys until C-final).
  C5 [DONE] crop/crop_inner/crop_sint -> native.
  C5b [DONE] miss family (miss/miss_dext/miss_dext_uncached/miss_sint, mod.rs ~9612)
         -> native (returns bool; type params sut/ref_ -> NRc; uses type_*_parts +
         repo + nest — same pattern as crop; nest bridged via lowering).
  C8 [DONE] NEST SCC (nest/nest_inner/nest_inner_impl/nest_sint/nest_core/
         deem_variance/nest_meet/nest_deep_tomes/nest_deep_arms + atom_nest) read
         native (NRc<NTy>) in ONE atomic pass. Done as designed:
         - Type id for seen/gil/memo = interned `Rc` ptr (`NRc::as_ptr as u64`);
           dropped the `NestTypeInterner` threading. Added id-based
           insert_id/remove_id/contains_id to NestSeenSet/NestPairSet (types.rs);
           the noun-interner methods stay for the interner unit tests.
         - Boundary cache RE-KEYED native: `nest_cache_lookup/store` in intern.rs
           keyed on (sut_ptr, ref_ptr, vet, fan), reset in `live_reset`. This
           AVOIDS lowering the deepening subject to a noun just to compute a mug
           key (which would be O(N^2) over the deepening chain) — the old
           `nest_mug_lookup/register` are now dead (retire at C-final).
         - Deepening children (cell/face/hint/core payloads) stay native (the
           win). Leaf parts lowered memoized: nest_core lowers coil leaves for
           coil_parts/garb_*/rest_tomes + native_of's the context; fork options via
           fork_options_native (lower set + re-lift); atom_nest lowers the small
           atom; core_dox_native builds the dox from the coil leaf only (dummy
           %noun payload — core_dox ignores payload) so the deep payload is never
           lowered. repo/peek native; play still takes a noun subject (lowered).
         - nest_noun bridge added; 19 mod.rs + 1 fire.rs + 9 test.rs callers
           renamed to nest_noun.
         - VALIDATION: shadow_gate.sh byte-parity PASS (all 4 fixtures); full honk
           lib test suite green single-threaded (121 passed, 2 ignored). Repaired
           PRE-EXISTING test.rs breakage from C4/C5 (the fuse/crop/miss algebra
           test still passed noun args to the now-native fns).
         - chunked_tisgar_chain test: my live_to_noun use surfaced a latent bug in
           the DEAD-CODE `mint_tisgar_chain_chunked` driver (mints each layer in a
           fresh slab without live_reset -> stale-slab nouns from the ptr-keyed
           memos). Fixed by adding per-layer `live_reset()` (mirrors honk.rs's
           per-compile reset). A/B-confirmed it passed pre-C8.
         - KNOWN PRE-EXISTING failures (NOT C8; both fail isolated on the pre-C8
           commit, only "pass" in parallel runs via thread-local pollution), now
           `#[ignore]`d with notes: frame_arena_core_mint_matches_monolithic +
           frame_arena_wet_gate_function_sample_matches_monolithic — native mint of
           a wet `|-` loop against a BARE %noun subject fails ("coil missing
           tail" / "tag head not atom"). The full binary path (prelude subject)
           compiles the same sources fine. TODO: fix the bare-subject wet-mint
           corner (separate from the flip).
         - compiler_mint.rs integration target: 13 failures, all PRE-EXISTING
           (mid-flip incompleteness: find/gain/lose/mull not yet native). A/B: the
           pre-C8 commit has 15 such failures — C8 reduces it to 13.
  C6+C9 [DONE] FUSED wing-nav core (find/fond/fond_name/fond_hold_inner/fend/fund/
         twin/resolve_wing_axis/fine + take/take_inner/take_inner_head_tail/take_axis
         + cnts_tack/cnts_toss/tack/toss/cnts_base_port + feel) AND the skin family
         (gain/lose/chip/cool + gain_skin[_inner]/gain_{atom,cell,leaf}_skin +
         lose_skin[_inner]/lose_{atom,cell,leaf}_skin) flipped to native (NRc<NTy>)
         in ONE atomic compile. Done as designed:
         - types.rs CARRIERS: Port/Pony::Synthetic.typ -> NRc<NTy> (formula stays
           Noun); Opal::Leg(NRc<NTy>); Opal::Arm.arms Vec<(NRc<NTy>, Noun)> (core
           native, foot stays Noun). Added `Debug` derive to ir::Type + ir::Leaf
           (the carriers derive Debug). DORMANT Port-embedding caches DELETED
           (find/find_raw/strict_term_port[_raw] + FindCacheEntry/FindRawCacheEntry/
           FindMemoValue/StrictTermPortCacheEntry + their typedefs) — grep-verified
           dead (only Default+clear). Surviving LookupMemoSet entries (cool/chip/
           wing_axis/look/loot/strict_term_core_parts_raw) kept noun-keyed (also
           dormant); re-key at C-final.
         - find.rs FOND SCC: reads &*sut; %hold cycle guard = Vec<u64> of NRc::as_ptr
           (interned ptr==structural); fond_hold_inner -> self.repo; peek native
           (dropped peek_noun); coil/face-tool/coil-tome leaves lowered via
           live_leaf_to_noun then look/loot (look/loot/dab/dom/cog stay NOUN); fork
           via fork_from_options(noun)+native_of; fund/fond->mint+play lower native
           sut via live_to_noun, ty_noun goal, native_of minted typ. is_term_face
           now reads the lowered face-tool leaf directly (a term face's tool is a
           bare atom). twin: Synthetic.formula keeps noun_eq; arm dedup uses
           NRc::ptr_eq on cores + noun_eq on feet.
         - fine() RETURN-SHAPE CHOICE: kept fine returning (Noun typ, Noun formula),
           lowering the native Port typ via live_to_noun, to bound blast radius
           (callers mint_fits/mint_wing/feel/fond_name-go consume noun typ today).
           fire stays NOUN-bridged (arm cores lowered in fine); wet.rs untouched.
         - TAKE SCC: duz bound Fn(&mut Self, NRc<NTy>)->Result<NRc<NTy>>; vil ->
           HashSet<u64> NRc-ptr; rebuilds via cons_cell/cons_core/cons_face/cons_hint
           + repo native + fork_from_options(noun)+native_of.
         - SKIN SCC: sut/ref_/return NRc; seen -> HashSet<u64> NRc-ptr; cons_* with
           tool/note/coil leaves preserved; native nest/fuse/crop/repo; play lowers
           sut; atom/leaf arms lower the small atom for type_atom_parts/fitz/atom_max
           via live_to_noun. gain/lose asymmetry preserved EXACTLY (gain Help/Name
           re-wrap cons_hint/cons_face, gain Flag=fork; lose Help/Name drop, lose
           Flag=chained lose). gain Help hint head = [sut note] (byte-parity with
           hint_type(sut,note,payload)). cool identity collapse -> NRc::ptr_eq(ty,sut).
         - mull C7-trio re-touched: mull_cnts calls native find; mull_cnts_with_ports/
           mull_endo read native Port/Palo/Opal (dropped native_of-on-typ + the find
           lowering); mull WutCol/WutHax/ZapPat arms call native gain/lose/fend/feel
           (dropped lowering); fire-bound arm cores still lowered (fire noun).
         - BRIDGES added (self-call protected, no global rename used): find_noun,
           fend_noun, feel_noun, gain_noun, lose_noun, resolve_wing_axis_noun,
           cnts_base_port_noun. mint-side noun callers route through them
           (mint_fits/mint_wing/mint_wthx/mint_cnts/play_cnts/play_wtcl/mint_wtcl/
           mint_zppt/play_inner ZapPat/skin_test_formula Over). fuse_noun/crop_noun/
           ty_face now used only by test.rs (lib-build dead-code warnings; harmless).
         - REGRESSION CAUGHT + KEYSTONE FIX (live_reset in Ut::new): the initial
           C6+C9 draft built green + passed shadow_gate, but the deterministic
           single-threaded compiler_mint A/B vs C7 (cfce3f6b) found 3 NEW failures
           (compile_opened_runes, compile_representative, metamorphic_branch_swap)
           with decode errors ("unknown type tag", "atom missing tail",
           atom->noun widening). ROOT CAUSE (parallel diagnosis, all 3 converged):
           C6+C9 made the persistent thread-local intern table + TO_NOUN_MEMO/
           LEAF_MEMO (keyed by Rc/noun pointer, bound to a compile's slab) the
           PRIMARY path for the wing-nav/skin families AND deleted the find caches
           that bounded re-decoding — so a test thread compiling many exprs without
           live_reset got a prior compile's freed-slab noun aliased back via
           live_to_noun -> decoded as garbage. The binary already resets per
           batch-entry (honk.rs:1108) and per chunk-layer, which is why shadow_gate
           (fresh process per fixture) passed but the multi-compile-per-thread test
           suite failed. FIX: call intern::live_reset() at the top of Ut::new
           (every compile boundary). Safe for the binary: its long-lived
           build-context Ut resets once at construction while the per-entry reset
           still dominates intra-compile sharing; cross-Ut types cross as nouns
           (pre-C-final) so no Ut depends on another's table.
         - VALIDATION (after the live_reset fix): cargo build --lib + --tests GREEN;
           shadow_gate byte-parity PASS (4 fixtures); compiler_mint single-threaded
           69 passed / 0 failed (was 12 failing on C7, 14 on C8 — the live_reset
           fixed the entire cross-compile aliasing class, which had masqueraded as
           "mid-flip incompleteness"); full lib suite 122 passed / 0 failed / 1
           ignored. frame_arena_wet_gate_function_sample_matches_monolithic was the
           same aliasing bug -> now FIXED + un-ignored. frame_arena_core_mint stays
           ignored: confirmed still failing with a fresh table ("coil missing tail")
           -> a GENUINE pre-existing bare-%noun wet-|- mint bug, separate from the
           flip. Zero regressions vs C7. Adversarial review of the byte-parity-risky
           spots pending.
  C6 [SUPERSEDED by C6+C9 above] gain/lose skin families + cool/chip -> native.
         ENTANGLED with find/take/Port/Palo (chip/cool drive `take`, whose duz
         closure passes type NOUNS to the skins) -> flip TOGETHER with the find/take + fond
         batch (C9), not standalone. gain_skin builds via cons_face/cons_hint/
         fork; uses fuse/crop (native), nest (native after C8), play (bridge).
  C7 [DONE] mull family + fish (type_test_formula_on_axis) SCC read/return native
         (NRc<NTy>) in one atomic pass. Done as designed:
         - FISH SCC {type_test_formula_on_axis, _inner, _fork}: TYPE input -> NRc;
           RETURN stays Noun (it is a NOCK FORMULA). seen_holds Vec<NRc> with
           NRc::ptr_eq dedup (dropped raw_equals/noun_eq). atom value lowered via
           live_to_noun + type_atom_parts; fork via fork_options_native; hold via
           native repo + ptr_eq cycle guard; core => Err(fish-core). Fish boundary
           cache kept noun/mug-keyed: lower typ once at entry, pass that noun.
           mint_fits now uses self.play (NRc ref_type) + native fish (no bridge).
         - MULL SCC {mull, mull_inner, mull_open_then_recurse, mull_nice, mull_beth,
           mull_grow, mile, mull_mile, mull_balk, mull_bake, mull_cnts, emul,
           mull_cnts_with_ports, mull_endo}: sut/gol/dox + both return slots -> NRc
           (mull returns TWO TYPES p/q, NOT type+formula). ~30 mull_inner arms use
           cons_*/ty_*_n; void guards via matches!(NTy::Void). play_noun->play
           (lower sut), wrap_type_noun->wrap_type, nest_noun->nest. Still-noun deps
           (C6 gain/lose; C9 find/fend/feel/tack/toss/fire/Port/Palo/Opal; mint;
           busk; hint_type) bridged: lower native args via live_to_noun, native_of
           the returned TYPE noun(s); mint's formula slot + cove axes stay noun.
           %fits keeps noun_eq (formula compare) + syx_p!=syx_q (u64 axes).
           mull_mile builds the native core via ty_core_n.1 (coil from leaf parts;
           payload native; core_context_from_payload fed a lowered sut/dox).
           mull cache kept noun/mug-keyed (lower sut/gol/dox once at entry,
           native_of cached p_ty/q_ty on hit). fork_from_options stays noun path
           (RT-07) + native_of lift.
         - mull_noun bridge added (#[allow(dead_code)]; the live wet.rs caller uses
           native mull directly with native_of'd args; only test.rs uses mull_noun).
           mull_check_wet (wet.rs) stays noun-signatured (fire/C9 boundary),
           native_of's wet_core/noun_goal/dox before native mull. fire_wet_rib stays
           noun-keyed.
         - test.rs direct callers (mull_cnts_with_ports, mull_endo, mull_bake,
           mull_balk, mull) fixed: native_of the type-noun args / route through
           mull_noun.
         - VALIDATION: cargo build -p honk --lib GREEN; full honk lib suite green
           single-threaded (121 passed, 2 ignored — same as C8). Release build +
           shadow_gate byte-parity left to the parent (per task discipline).
  C9 [DONE — see C6+C9 fused entry above] find/take/Port/Palo + fond family ->
         native (the wing-nav subsystem; gain/lose/cool/chip folded in).
  C-final [IN PROGRESS] BOUNDARY CLOSE = the memory win. Decomposed:
    1a [DONE] mint family native: mint/mint_inner + ALL mint_* helpers + nice +
       hint_type + mint_core + mine take NRc sut/gol, return (NRc<NTy>, Noun).
       Calls the native consumers directly (drops the *_noun bridge CALLS in mint
       code; play still takes noun sut so mint lowers it). Caches stay noun-keyed
       (lower sut/gol at the boundary, memoized) — so the dumb compile is still
       O(N^2)/slow until 1b; mint_core now builds Core{payload: shared native Rc}
       (the deepening subject is shared, not rebuilt). mint_noun bridge added
       (self-call protected); binary/compile_expr_with_options/play/fire/tests
       route through it. VALIDATION: shadow_gate byte-parity PASS; compiler_mint
       69/0 single-threaded (held); lib 122/0/1; zero regressions. (dumb
       byte-parity deferred to 1b — too slow until the cache re-key.)
    1b [ ] re-key mint/core_mint/mull caches on native Rc identity (NEST_CACHE
       template) -> removes the boundary sut lowering -> the O(N^2)->O(N) win.
       VALIDATE: full dumb-kernel byte-parity vs /tmp/dumb_preflip.jam + perf +
       NounSlab no longer grows monotonically.
    2 [DONE] play sut param Noun->NRc (C-final.2). play/play_inner/play_core + ALL
       play_* helpers (colsig/brtis/brcb/wtcl/wtts/ketvar/dbug/note/kthp/kttr/ktcl/
       dtls/wing/cnts/cnts_apply_leg_patches/limb/tune/opened) take NRc<NTy>; busk
       flipped native too (cons_face, shared payload). Internal callers (mint_inner
       Rock/Sand, mint_fits/ktsl/kthp/sigzap/zpcom/zpgl/tscm, nest_core deep-arms,
       chip/gain_skin/lose_skin spec+over, ALL mull arms) now thread native sut to
       play directly — every `live_to_noun(&sut)`-for-play lowering REMOVED (grep
       clean). play_noun KEPT as the noun-in/noun-out bridge (now native_of's sut
       at entry) for the 2 binary prelude callers; added play_to_noun (native-in/
       noun-out) for the noun-returning play_* helpers; pb kept (helper noun-lift).
       Play has NO own boundary cache -> nothing to re-key on the play side. One
       residual lowering REMAINS in repo.rs rest_inner (the %hold resolution path
       carries each leg's inner subject as a NOUN leaf -> native_of per hold; NOT
       on the main subject-deepening chain). VALIDATE: lib build GREEN; compiler_mint
       69/0; lib 122/0/1; shadow_gate byte-parity PASS (4 fixtures). Dumb-kernel
       perf/byte-parity = parent's next run (first measurable memory win).
    3 [ ] delete now-dead bridges (repo_noun/peek_noun/wrap_type_noun/fuse_noun/
       crop_noun/miss_noun/nest_noun/find_noun/fend_noun/feel_noun/gain_noun/
       lose_noun/cnts_base_port_noun/mull_noun + nest_mug_lookup/register).
    4 [ ] re-key remaining boundary caches native (crop/fuse/redo/rest/fish/Hold/
       Lookup) + delete dead nest_raw/nest noun caches + NestTypeInterner.
    5 [ ] delete noun ty_* ctors + _n variants + SKELETON decoders (keep leaf
       decoders coil_parts/garb_*/rest_tomes/fork_set_options/type_atom_parts +
       live_leaf_to_noun — Phase-1 leaf residue). Final shadow_gate + dumb parity.
KEY RISKS (from the map): collapse-parity (add cons_core/face/hint, never bare
live_intern for branch rebuilds); fork RT-07 order (keep noun fork path til late);
boundary-cache key drift (keep to_noun mug keys until C-final); NEST SCC atomicity.

## RESUMPTION NOTES (2026-06-19) — read before continuing the grind

BRANCH: fwd/bitemyapp/native-compiler-pma-native-compiler-types (NOT pma-hell-4;
I mixed them up once — flip commits are here). HEAD = the latest "FLIP consumer-N"
commit.

USER DIRECTIVE: complete the ENTIRE flip (blind big-bang); do NOT ask per-step.
The branch is intentionally perf-broken (real-kernel compiles >180s) until C-final
removes all bridges — this is expected, not a regression to chase. Validate each
family ONLY with the fast fixture gate; dumb byte-parity+perf is checked at
C-final against /tmp/dumb_preflip.jam (the verified pre-flip golden, 19873112 B;
regenerate from commit e6e653e2 if /tmp lost it).

PERF (settled): the flip's slowness is the pervasive bridge machinery
(native_of/to_noun/ty_core_n double-build/mug-keyed caches) on every algebra call;
it vanishes only at C-final. Infra in place: Leaf carries a cached content hash
(leaf.rs) so interning leaf-carrying types is O(1)/leaf; intern::live_to_noun
(Type) + live_leaf_to_noun (coil/set) memoize lowering by interned/Arc pointer.

WORKFLOW: a linter reformats mod.rs constantly -> the Edit tool fails "modified
since read". Apply edits via `python3` text-replace (read current content, assert
count==1, replace, write). Pattern per consumer family:
  1. extract current fn text via awk to /tmp; python-replace with native body.
  2. native body: match &*sut (and &*ref_); Rc::ptr_eq for equality; cons_cell/
     cons_void/cons_noun/cons_core/cons_face/cons_hint for rebuilds (collapse-aware);
     self.repo(x) native; for leaf-carried parts (coil/set/atom) lower via
     live_leaf_to_noun + the existing noun decoder (coil_parts/fork_set_options/
     type_atom_parts); native-pointer seen-sets (HashSet<(usize,usize)>).
  3. not-yet-flipped callees (nest until C8) bridged by lowering args via
     live_to_noun.
  4. add `fn <name>_noun` bridge; python-rename `self.<name>(`/`ut.<name>(` ->
     `<name>_noun(` in mod.rs/wet.rs/find.rs/test.rs; then fix the bridge fn's OWN
     self-call back to native (the rename hits it).
  5. cargo build -p honk --lib; then release build + shadow_gate.sh (timeout 120).
     commit. mark C# DONE here.

DONE so far: C1 repo, C2 peek, C3 wrap_type (+cons_core/face/hint), C4 fuse,
C5 crop. NEXT: C5b miss, C6 gain/lose+cool, C7 mull glue, C8 NEST SCC, C9 fond,
C-final.


## NEST SCC impl plan (do as ONE atomic python pass — next step)

nest family (mod.rs ~8124-8620): nest, nest_inner, nest_inner_impl, nest_sint,
nest_core, nest_meet, nest_deep_tomes, nest_deep_arms + atom_nest (Atom arm).
All mutually recursive -> flip together (non-compiling until all done).

Helper structs (mod.rs, search NestTypeInterner/NestSeenSet/NestPairSet/
NestMemoKey): they assign ids to type NOUNS (id_for) and key the memo/seen/gil on
those ids. NATIVE: the canonical Rc pointer IS the id -> drop NestTypeInterner;
key NestSeenSet (HashSet<u64>), NestPairSet (HashSet<(u64,u64)>), NestMemoKey
{sut:u64, ref_:u64, seg, reg, gil} on NRc::as_ptr(&t) as u64. snapshot()/insert/
remove become trivial ptr ops (no `self`/interner needed).

Per-fn: sut/ref_ (+ dom/dab/hem/dox/vim in nest_core/meet/deep) -> NRc<NTy>;
noun_eq(sut,ref_) -> Rc::ptr_eq; type_tag_kind+type_*_parts -> match &*; repo ->
native (done); nest_deep_* play_noun -> native play (result drives nest); nest_core
lowers the core coil leaf via live_leaf_to_noun for coil_parts/garb_poly/garb_vair/
rest_tomes (those stay noun); atom_nest reads NTy::Atom leaves (lower small for
type_atom_parts/fitz). nest_mug_lookup/register (3542): keep noun-keyed (lower
sut/ref_ via live_to_noun) until C-final.

Then: add `fn nest_noun(sut: Noun, ref_: Noun) -> bool` bridge; python-rename the
19 self.nest(/ut.nest( callers -> nest_noun (in mod.rs/wet.rs/find.rs/test.rs);
fix nest_noun's own self-call back to native; cargo build -p honk --lib; release
build + shadow_gate.sh; commit; mark C8 DONE.
