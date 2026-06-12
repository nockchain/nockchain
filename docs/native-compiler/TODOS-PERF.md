> STATUS (2026-06-12): Items 1-6 are RESOLVED on this branch; item 7 is accepted as-is. Hint-blind jet matching (items 1, 2, 4) is now opt-in via `JetDispatchMode` on `Context` — hoonc/nockchain dispatch Exact (master behavior, zero added cost) while honk opts into HintBlind with warm alias-insertion memoization. log_hint_event and the parquet stack are deleted (item 3). unifying_equality is restored to the master implementation and the dormant readonly-range machinery is gone (item 5) — note the branch's EqualityWork loop measured ~25% faster than master's on the unifying_equality_canopy bench; re-deriving that win without the readonly baggage is a possible follow-up. dor keeps structural semantics with a cached-mug pre-filter and allocation-free walk (item 6). See docs/OSS-NEXT-PLAN.md.

### find_jet runs a full formula traversal on every warm-table miss — warm.rs:664

This is almost certainly your main regression. On master, a warm miss was one HAMT lookup returning NoJet. On this branch, every miss falls through to normalize_transparent_hints (warm.rs:526), which recursively walks the entire arm formula stripping %hand/%hunk/%lose/%mean/%spot hints, then does a second HAMT lookup. Three compounding costs, paid on every Nock 9 dispatch of every unjetted arm (i.e., the vast majority of calls in hoonc):

- O(formula-size) tree walk per call, with no memoization — the same arm re-normalizes on every invocation.
- If the formula contains any transparent hint (hint-laden compiler code does, pervasively), the normalized copy is re-allocated on the NockStack every call via T(stack, …), churning the current interpreter frame.
- The second HAMT lookup mugs the freshly allocated normalized tree, so the mug cache never helps — another O(n) pass per call.

This turns the per-call dispatch overhead from ~O(1) into ~O(arm formula size), several times over. For big arms (+mint:ut etc.) that's thousands of nodes per call.

### Batteries::matches falls back to structural formula equality — cold.rs:278,464,488

battery_eq_ignoring_hints now runs nock_formula_eq_ignoring_transparent_hints whenever plain unification fails. That function calls unifying_equality at every recursion node, so a failed battery match is roughly O(n²) in battery size — and batteries are huge. Worse, the case it's designed for (hinted vs. hint-stripped battery variants) can never unify, so the full structural compare re-runs on every jet dispatch for such cores; it never gets cheaper. For hoonc this fires on warm-chain candidates that don't unify; for honk it's on the hot path by design. It also runs inside cold.register during %fast replay at boot.

### log_hint_event string allocation on every hint push/pop — interpreter.rs:1907,2061,2077,2149

Every %slog, and every %hand/%hunk/%lose/%mean/%spot push and pop now does UTF-8 validation plus two String allocations (atom_text + format!) before write_behavior_event_safe checks whether tracing is even enabled (trace_info is None in normal hoonc runs, but the strings are built unconditionally). Compiler code is dense with ~|/~_ mean hints, so this is a steady tax across the whole run.

### Warm::init normalizes and double-inserts every jet, per %fast registration — warm.rs:618

insert_with_transparent_hints runs normalize_transparent_hints on each registered arm formula and inserts the formula twice (original + normalized) when they differ. Warm::init already rebuilds the whole table on every successful cold.register, so boot replay is O(registrations × jets) — now with normalization and stack allocation added per entry, and warm chains are up to 2× longer, which feeds back into the per-call cost of #2. Mostly a boot/startup cost, which hoonc pays on every run.

### unifying_equality rework — unifying_equality.rs:229

The change you suspected, but it's a constant-factor cost, not the dominant one: EqualityWork is ~40 bytes vs master's 16-byte (*mut Noun, *mut Noun) (2.5× work-stack traffic), there's an extra FinishCell item pushed/popped per cell with unequal children, and two readonly-range checks per Compare (these short-circuit to nothing since replace_extra_noun_ptr_ranges has no production callers — the range machinery is dormant). I'd estimate 10–30% on equality-heavy paths in isolation — but note its call count went up substantially via #2, which calls it at every formula node.

### dor now does deep non-unifying equality — sort.rs:86, ext.rs:141

The head comparison in util::dor (reached from gor/mor mug ties and direct dor/sort use) falls back to noun_equality, which heap-allocates a Vec worklist and an IntMap per call and never unifies, so equal-but-unshared keys pay the full deep compare every time. This is a real correctness fix (master's raw_equals-only check could mis-order), so it likely needs to survive the rebase in some cheaper form (e.g., unifying equality, or no per-call allocs).

### Small per-op taxes

- op_budget check at the top of the interpreter loop, every work item (interpreter.rs:730) — well-predicted branch when None, ~1%.
- is_in_frame now does an explicit bounds check in release builds (mem.rs) where master had only debug asserts — small constant on every preserve/copy decision.
- Offsetting speedup: resolve_stack_ptr/classify_ptr got a release-mode identity fast path when no PMA is installed.

## honk compiler perf — status (reconciled 2026-06-15)

### RESOLVED
- **Cached standard JAM cloned before write** → `jam_product` now `take()`s the `Vec<u8>` (`standard_jam.take()`, H1b).
- **Dead cool/chip cache stubs** → `cool_cache_*`/`chip_cache_*` deleted (they were no-op plumbing).

### OPEN — native prelude mint blows up (memory AND runtime): the headline finding
honk's native mint of hoon-138 (`HONK_NATIVE_PARITY=1`, or `just honk-138-parity`) does NOT complete: resident memory grows ~linearly at ~4 GB/min with no plateau, and it OOMs before finishing (reproduced 2026-06-14 on a 128 GB machine; ~30–50 min to exhaustion). Application kernels are unaffected — they compile against the embedded precompiled prelude. Ranked causes (full analysis in `docs/OSS-NEXT-PLAN.md`):
1. **Leaked, never-freed bump slab + one monolithic `ut.mint`** — `NounSlab` has no free/reset/compaction (only `Drop`, which never runs on the `Box::leak`'d Ut slab) and the whole prelude is minted in one call, so memory = cumulative allocation. This is the linear curve.
2. **No type interning / hash-consing** — `ty_*` re-allocate structurally-equal types at fresh addresses, defeating `noun_eq`'s pointer short-circuit and collapsing every mug/raw-pointer cache → super-linear deep walks (the runtime half) and permanent duplicate bytes.
3. **Subject-deepening** — `mint_core`/`play_core` embed the whole current subject as each core's context, so name resolution walks an O(arms-so-far) spine per reference → O(N²) over ~530 arms. hoon-138's monolithic 6-layer cumulative-subject core is the worst case.
4. **Redundant unconditional full-prelude `ut.play`** in `seed_honc_type_with_ut` — output-neutral; inhibiting it is byte-identical and shaves ~5 s off kernel builds, but it is not the dominant cost.
5. **Recursive molds + wet gates** (`type`/`hoon` are `$`-recursive with `%fork`/`%hold`; ~193 `|*`) hit honk's heaviest paths, amplified by (2).
6. **Never-cleared `lazy_resolvers`/fan interners + H2 context-widened cache keys** rarely collapse on the recursive prelude → re-misses.

**KEYSTONE FIX: type interning at the `ty_*` constructors** (generalize the in-tree `ty_hold_cached` pattern). It improves BOTH memory (dedup) and runtime (restores cache hits + O(1) equality), it is the precondition for a bounded arena, and it also targets the 60 s roswell gate. Then bound memory for real via a NockStack-style frame arena or chunked per-core generation. See the self-hosting plan in `OSS-NEXT-PLAN.md`.

### OPEN — 60 s roswell gate
Roswell native compile is ~71–76 s (> 60 s). Not recovered by the redundant-play shave; needs the interning keystone above plus the H4/H9 items below.

### OPEN — incremental honk perf (H4 / H9; not yet executed)
- **Standard batch recomputes the directory mug per entry** (`exact_directory_mug` / `directory_mug_with_files`): O(entries × dir size) I/O. Fix: cache per-batch directory manifests/content hashes; avoid `WalkDir` when a manifest file list is supplied.
- **Hoon sources read/scanned/parsed multiple times** (content key, import resolution, leaf parse): avoidable I/O + parse. Fix: a `SourceFile { text, hash, imports, ast }` cache.
- **Batch cache/slab lifetime unbounded** (leaked build-context slab; caches never evicted): retains type/trap/eval graphs for the process lifetime. Fix: split reusable prelude state from per-entry arenas; budgets/eviction. (Same leaked-slab root as native-mint #1 and PMA Phase 3 "un-leak the slab".)
- **`with_stack_guard` is a no-op** (`ut/mod.rs`): deep nested types can overflow the Rust stack. Fix: `stacker::maybe_grow` (already a dep) or explicit-stack the worst recursion.
- **AST→noun repeatedly materialized for cache keys** (`hoon_noun_for_node` / `hoon_to_noun`): repeated recursive allocation on hot paths. Fix: stable AST signatures / pointer-guarded keys; materialize once per parsed node.
- **Wing/fond recursion allocates heavily** (`find.rs`: BigUint axes, cloned vein vectors): Fix: keep axes `u64` until overflow, push/pop vein stacks, borrowed wing extraction.
