# Nouns in honk: slab/stack/PMA co-mingling — analysis and fix plan

Investigation of `TODOS.md` item "Nouns in `honk` co-mingle PMA and NounSlabs".
Companion to `DOR-DEEP-EQUALITY.md`, `TODOS-PERF.md`, and
`docs/pma/NOUN-PROVENANCE-AND-BRANDED-HANDLES.md`.

## Correction first: honk does not touch the PMA today

`grep -i pma` over `crates/honk/src` returns nothing. Both eval contexts are
bare NockStacks with no PMA installed:

- `create_eval_context` — `src/bin/honk.rs:3084`
- `create_musk_eval_context` — `src/native/ut/mod.rs:10902`

The sentence in `DOR-DEEP-EQUALITY.md` ("honk's nouns live in NounSlabs and
the PMA") over-claims for honk-the-process; it is accurate only for the wider
system (the same nockvm code paths honk exercises also run over PMA nouns in
serf deployments). The real co-mingling inside honk is **NounSlab ↔ NockStack**,
plus cross-slab. The PMA enters the picture in two indirect ways, covered
below: (1) the *absence* of a PMA is what disables nockvm's pointer validation
for honk, and (2) a PMA is the natural future home for the shared cold state,
which is exactly the step the current code cannot survive.

## The post-PMA constraint model

Post-PMA nockvm makes noun provenance a hard invariant (see
`NOUN-PROVENANCE-AND-BRANDED-HANDLES.md`): a pointer-form noun is meaningful
only relative to a `NounSpace` that registers its arena (stack range, PMA
range, or extra ptr ranges). The enforcement point is
`NounSpace::resolve_stack_ptr` / `classify_ptr` (`noun.rs`), which panic on
unregistered pointers — **except** for the release-mode identity fast path
added on this branch: when `pma_base.is_none()`, validation is skipped
entirely and every pointer classifies as `Stack`. The comment on that fast
path names the beneficiary: "slab-heavy code like the native Hoon compiler."

Mutation rules follow from arenas: unification may rewrite slots only in
writable memory with appropriate seniority; forwarding pointers may only be
written into stack frames (`is_in_frame` now explicitly rejects foreign
pointers — `mem.rs`); PMA trees must be closed (no out-pointers); slab nouns
may receive cached-mug writes only while private, writable, and
single-threaded (`set_mug` note in `mug.rs`,
`NounSpace::with_readonly_extra_ptr_ranges` in `noun.rs`).

## What honk actually does (arena inventory)

1. **Compiler state**: one big `NounSlab` for `Ut`, deliberately leaked
   (`Box::leak`, `src/bin/honk.rs:848,951`) so slab nouns are
   process-immortal. Additional per-task slabs come and go. A slab's
   `NounSpace` is `NounSpace::empty().with_extra_ptr_ranges(self.ptr_ranges())`
   (`nockapp/src/noun/slab.rs:307-316`).

2. **Evaluation**: two long-lived nockvm `Context`s (eval + musk), each a bare
   NockStack. The cold state jam is cued **twice**, once per context, onto each
   stack's root frame (`load_cold_state` `src/bin/honk.rs:2954`,
   `install_musk_cold_state` `ut/mod.rs:10968`).

3. **Boundary discipline is copy-in / copy-out, by convention**:
   - eval path: `copy_noun_to_allocator(&mut context.stack, formula,
     formula_space)` and subjects built on the stack
     (`eval_formula_noun_in_context`, `src/bin/honk.rs:3090-3115`);
   - musk mack path: slab cores are copied onto the eval stack with structural
     sharing (`copy_into_eval_stack_shared`, `ut/mod.rs:1941`), results copied
     back via `slab.copy_into(result, &stack_space)`
     (`musk_interpret_mack_in_context`, `ut/mod.rs:5809-5851`).

4. **The eval stack doubles as a heap**: cold state, warm state, and the mack
   core cache live on the root frame for the context's lifetime. Panic
   recovery uses `NockStack::checkpoint`/`restore_checkpoint` plus manual
   `ContextSnapshot` rollback of cold/warm/cache. Soundness rests on the
   "monotone unification" argument (unification only ever rewrites slots to
   point at *more senior* memory, so discarding a junior region never leaves
   senior state dangling) — true today, documented only in a comment at
   `ut/mod.rs:5824`, enforced nowhere.

5. **Address-keyed caches erase provenance**:
   `mack_core_cache_raw: FastHashMap<u64, Noun>` and
   `mack_cache_raw: FastHashMap<(u64,u64), Option<Noun>>` (`ut/mod.rs:98-110`)
   key by `core.as_raw()` — a slab *address* used as identity. Cache values are
   eval-stack nouns. Invalidation is by `stack.frame_identity()`
   (`ut/mod.rs:1902-1908`), a heuristic (identity ^ frame offset) that can
   collide across reset/replace cycles.

6. **Public API leaks raw nouns**: `Compiled` owns a private slab but returns
   `formula() -> Noun` and `noun_space() -> NounSpace` separately
   (`lib.rs:57-79`) — the exact "alien noun" pattern the provenance audit
   flagged.

## Why it works today — five load-bearing accidents

1. The leaked `Ut` slab means slab addresses never dangle and never get
   reused, which is the only thing making address-keyed caches sound.
2. honk installs no PMA, so the release identity fast path waves every
   pointer through unvalidated. The one runtime net exists only in debug
   builds.
3. The copy-in convention happens to be followed at every `interpret()`
   boundary inspected. Nothing checks it; one missed `copy_into` produces a
   silent cross-arena tree in release.
4. `is_in_frame`'s new explicit foreign-pointer rejection means `preserve`
   leaves any stray slab pointer in place rather than corrupting it — masking
   rather than catching mistakes.
5. Unification monotonicity makes the checkpoint/rollback dance sound — an
   invariant maintained by `rewrite_direction` in `unifying_equality.rs` that
   honk depends on from two crates away.

Meanwhile the nockvm half of a *principled* zero-copy regime was built on this
branch and never wired: `NockStack::replace_extra_noun_ptr_ranges`,
`NounSpace::with_readonly_extra_ptr_ranges`, the readonly-slot/readonly-noun
propagation in `EqualityWork`, and the foreign-cell tests in `mem.rs` /
`unifying_equality.rs` all exist for "slab-resident subjects under the
zero-copy mack path" — and have **zero production callers**. honk shipped the
copy-based core cache instead. The machinery is pure tax (see TODOS-PERF #5)
and a second, half-finished regime confusing the picture.

## Where it breaks

- **Install a PMA into either eval context and everything detonates.** With
  `pma_base` set, full range validation turns on in release; every latent
  un-copied slab pointer becomes a panic ("pointer-form noun is not within
  stack or PMA arenas"). This matters because installing a PMA is the obvious
  next move for cold-state sharing (below) — the TODO's constraint "this isn't
  allowed in post-PMA nockvm" is precisely this trap.
- **Un-leak any slab and the address-keyed caches turn into UAF/collision
  bugs.** Freeing or compacting the `Ut` slab invalidates `mack_core_cache_raw`
  keys silently.
- **`Compiled` consumers can outlive the slab** (or mix spaces) with no
  compiler or runtime resistance.
- **Cross-context confusion**: two NockStacks + N slabs all produce
  pointer-form nouns of identical Rust type; `frame_identity` collisions or a
  missed `clear_context_dependent_caches` hand stale eval-stack nouns to a new
  context.

## The cold-state question the TODO asks

"The burden of and reasons for copying/referencing PMA nouns (cold state?
something else?)" — answered:

- Today's burden: the 700KB `honc-cold-138.jam` is cued twice (eval + musk
  contexts), each followed by `Cold::from_noun` + `Warm::init`; mack cores are
  copied slab→stack (amortized by the sharing cache); results copied
  stack→slab; assets cross every other boundary by jam/cue (which launders
  provenance and is sound).
- The natural optimization — cue the cold state once into a shared arena
  referenced by both contexts — is exactly where a PMA (or PMA-like shared
  arena) would enter honk. `rewrite_direction` already implements the PMA
  unification rules (PMA side wins; PMA↔PMA by pointer order) and the
  `CACHED_MUG_METADATA_MASK` work makes PMA-resident mugs sound. But adopting
  it flips on the validating regime, so **boundary enforcement must land
  first**. That ordering constraint is the actionable content of the TODO.

## Fix plan

**Phase 0 — pick one regime per boundary (decision, no code).**
Copy-at-boundary (R1) is what honk does and it is sound; zero-copy via
registered readonly ranges (R2) is half-built in nockvm and unused. Default to
R1 everywhere. Either commit R2 to the one boundary where copying measurably
dominates (mack cores: wire `replace_extra_noun_ptr_ranges` with the slab's
ranges readonly around `musk_interpret_mack*`, drop the core-cache copies,
measure) **or delete the R2 machinery** from `NockStack`/`EqualityWork`
(recovering the unifying-equality constant factor; keep readonly ranges only
in `NounSpace` for slab self-description). Dormant unsafe machinery is the
worst of both.

**Phase 1 — make the copy boundary mechanical, not conventional.**
- Honk-internal eval wrappers take `NounHandle` (or branded handles per the
  provenance doc) instead of `(Noun, &NounSpace)`; the wrapper owns the
  copy-in/copy-out so call sites *cannot* pass a raw slab noun to
  `interpret`.
- Replace `Compiled::formula()`/`noun_space()` with a scoped
  `with_formula(|handle| ...)` API (subsumes TODOS "Public API exposes raw
  Noun without lifetime binding").
- Move the musk raw-pointer juggling (`ut/mod.rs:5797-5805`) behind one small
  documented-unsafe module (subsumes TODOS "Unsafe borrow workarounds").

**Phase 2 — restore a runtime net in release.**
The copy walk already visits every node, so validate there: `copy_into`
(slab→stack and stack→slab) asserts each visited pointer resolves in the
source space, in release too — near-zero marginal cost at the only place it
matters. Optionally add a `validate-spaces` feature for CI/parity runs that
disables the identity fast path.

**Phase 3 — fix identity and lifetime in the caches.**
- Key mack caches by (slab generation, raw) — give `NounSlab` a monotonic
  generation bumped on clear/free; assert append-only behavior while any
  generation is outstanding.
- Replace the `frame_identity()` heuristic with an explicit epoch honk bumps
  when it resets or replaces a context — honk owns those events; inferring
  them from stack geometry is fragile.
- Un-leak the `Ut` slab once Phases 1–2 hold (subsumes TODOS-PERF "Batch
  cache/slab lifetime is unbounded").
- While here: split mack's cached `None` into expected-failure vs
  panic/exhaustion (TODOS "mack fold failures cache panics") — same cache,
  same edit.

**Phase 4 — only then share the cold state.**
Cue `honc-cold-138.jam` once into a dedicated long-lived arena (private PMA or
equivalent) installed into both contexts via `install_pma_arena`. This halves
boot cue cost, deduplicates the cold-state tree, and makes honk a real
post-PMA citizen — validation on, co-mingling gone by construction. Document
the monotone-unification invariant in `unifying_equality.rs` as a stated
contract at the same time, since checkpoint/rollback (and this plan) lean on
it.

**Tests that pin the model:**
- Debug-build (validation-on) runs of the honk parity suite in CI.
- A boundary test that hands a slab noun to an eval wrapper and asserts it is
  copied (or rejected), not referenced.
- A cache-generation test: clear/rebuild a slab, assert mack caches miss
  rather than alias.
- After Phase 4: the existing PMA/foreign-cell tests in `mem.rs` and
  `unifying_equality.rs` become load-bearing; keep them.

## Overlap map

- `TODOS.md` "Public API exposes raw Noun without lifetime binding" → Phase 1.
- `TODOS.md` "Unsafe borrow workarounds in high-level compiler logic" → Phase 1.
- `TODOS.md` "mack fold failures cache panics as ordinary no-fold" → Phase 3.
- `TODOS-PERF.md` #5 (dormant readonly-range machinery taxing
  `unifying_equality`) → Phase 0.
- `TODOS-PERF.md` "Batch cache/slab lifetime is unbounded" → Phase 3.
- `TODOS-PERF.md` #2 (battery-match structural equality) → cost model changes
  again after Phase 4 (PMA-resident cold state unifies differently); revisit
  after both land.
- `docs/pma/NOUN-PROVENANCE-AND-BRANDED-HANDLES.md` → Phase 1 is that doc's
  guidance applied to honk; `DOR-DEEP-EQUALITY.md` → fix the "and the PMA"
  wording to "NounSlabs (and, in serf deployments, the PMA)".
