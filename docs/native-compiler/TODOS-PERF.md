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

### RESOLVED — native prelude self-mint completes with strict parity
honk's native hoon-138 self-mint (`HONK_NATIVE_PARITY=1`, or
`just honk-138-parity`) is now bounded and completes byte-identically to
hoonc. The scope-precise fan key makes the build converge, and the bounded
`musk` core-copy cache prevents stale fold entries from dominating RSS.

Fresh cold measurements on 2026-07-18:

- honk native self-mint: 47.12 s, 9.56 GiB peak RSS
- hoonc arbitrary reference: 181.10 s, 0.58 GiB peak RSS
- output: exact `cmp`, SHA-256
  `b156034a5d5d158c133fad6591a71b96def4339e49faba4773b9f9b68e1c1741`

The RSS guard in `honk_138_native_parity.sh` remains as a regression safety
rail. The embedded precompiled prelude remains the normal application-build
path, but it is no longer a feasibility requirement.

### OPEN — 60 s roswell gate
Roswell native compile measured 140.00 s with 11.93 GiB peak RSS on the
2026-07-18 cold parity run (> 60 s). The H4/H9 items below remain relevant.

### OPEN — incremental honk perf (H4 / H9; not yet executed)
- **Standard batch recomputes the directory mug per entry** (`exact_directory_mug` / `directory_mug_with_files`): O(entries × dir size) I/O. Fix: cache per-batch directory manifests/content hashes; avoid `WalkDir` when a manifest file list is supplied.
- **Hoon sources read/scanned/parsed multiple times** (content key, import resolution, leaf parse): avoidable I/O + parse. Fix: a `SourceFile { text, hash, imports, ast }` cache.
- **Batch cache/slab lifetime unbounded** (leaked build-context slab; caches never evicted): retains type/trap/eval graphs for the process lifetime. Fix: split reusable prelude state from per-entry arenas; budgets/eviction. (Same leaked-slab root as native-mint #1 and PMA Phase 3 "un-leak the slab".)
- **`with_stack_guard` is a no-op** (`ut/mod.rs`): deep nested types can overflow the Rust stack. Fix: `stacker::maybe_grow` (already a dep) or explicit-stack the worst recursion.
- **AST→noun repeatedly materialized for cache keys** (`hoon_noun_for_node` / `hoon_to_noun`): repeated recursive allocation on hot paths. Fix: stable AST signatures / pointer-guarded keys; materialize once per parsed node.
- **Wing/fond recursion allocates heavily** (`find.rs`: BigUint axes, cloned vein vectors): Fix: keep axes `u64` until overflow, push/pop vein stacks, borrowed wing extraction.
