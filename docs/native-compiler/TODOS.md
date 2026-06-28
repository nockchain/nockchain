# honk native-compiler TODOs

Status reconciled 2026-06-15 against branch `fwd/bitemyapp/native-compiler-pma-hell-4`. Most of the original correctness list is resolved; see `docs/OSS-NEXT-PLAN.md` for the full work log. Resolved items are summarized first, then the active list.

## Resolved

- **Missing canonical octs type silently downgraded `/*` parity** (H1a): `build.rs` hard-errors when `crates/honk/assets/hoonc-octs-type-138.jam` is missing/empty (env override aside); the runtime no longer falls back to a local `[p=@ud q=@]` for data imports.
- **Artifact parse keeps docs disabled** (H1d): retained intentionally — docs-disabled parse is the byte-parity-correct input (the six kernels are byte-exact this way; doc-enabled parsing perturbs emitted dbug/spot nouns).
- **Batch-mode state-dependent miscompile** (H2b): the miss-memo key carries full fan + memo context; batch persistence is gated behind the `honk-batch-parity` byte check rather than per-entry state resets.
- **Semantic cache keys omitted %rest/fan context** (H2a): centralized typed cache-key builders now carry the full `CacheContextKey` (vet + fan_context + arm_epoch + placeholder) across mint/mull/redo/rest/fish/nest.
- **Native parity masked by embedded artifacts** (H3): `HONK_NATIVE_PARITY` splits oracle/bootstrap from native mint; with it set honk mints the prelude natively (which originally surfaced the native-mint blowup — see `TODOS-PERF.md`). UPDATE 2026-06-28: RESOLVED — native mint now completes (~40 s) with bounded memory and is byte-identical to hoonc (both 2,286,744 B, `cmp` clean); enforced by `crates/honk/tests/native_parity_138.rs`. The embedded prelude is retained as a validated speed cache, not a correctness crutch.
- **`exact_swet_vase_trap` misleadingly not exact** (H1c): removed, along with `uses_exact_artifact_swet` and its cache-key threading.
- **Import resolution silently ignored malformed runes** (H1e): malformed `/=` and `/*` hard-error; `/?` is ignored loudly; `/%` is rejected; marks are enforced (`%jam` → Data, others → UnsupportedExpr).
- **`softed-constraints.hoon` filename bypass** (H1g): the shortcut is pinned to blake3 content hashes of the source + constraint jams; a stale shortcut hard-errors instead of silently bypassing source changes.
- **Wrapper parity hardcoded positions** (H1f): blake3-fixtured native wrapper batteries + `ExactWrapperBatteries::assert_required`, so `hoonc.hoon` line drift fails a test.
- **Public API exposed raw `Noun` without lifetime binding** (H5): the `Compiled::formula()` raw-Noun accessor is deleted; callers obtain the formula only through slab-scoped `jam*` methods that keep the owning slab alive.
- **PMA/NounSlab co-mingling — Phase 0** (N2): the dormant readonly-range machinery is deleted and `unifying_equality.rs` is back to the merge-base implementation.
- **Rust oracle tests were structural-only** (partial): the cited `NounSlabRejam` compare mode is gone; whole-artifact byte parity is now enforced by the H0 kernel-parity harness (`just honk-parity` / `just bazel honk-parity`, all six kernels byte-exact). Per-expression byte fixtures remain deferred (see Open #11).

## Open

### PMA conformance: make honk a post-PMA citizen (co-mingle Phases 1–4)
Phase 1 (mechanical boundary) is PARTIAL: the two raw-pointer `VetGuard`s are replaced by a safe `with_vet_off`; `Compiled::formula()` is removed; branded eval-boundary primitives (`BrandedNounSpace` `copy_in`/`interpret`, in nockapp) exist with one eval call site migrated. STILL OPEN: corral the musk raw-pointer juggling (`ut/mod.rs` `musk_interpret_mack*` — genuine self-field aliasing of context + slab; needs struct-level borrow-splitting, not branding) and migrate the remaining `interpret` call sites. Phase 2 (validate provenance inside `copy_into`), Phase 3 (slab generations + explicit context epochs + un-leak the slab + mack outcome split), and Phase 4 (shared cold-state PMA arena) are OPEN. A branded-handle audit found brands help only the eval boundary; the bulk of conformance needs borrow-splitting plus a bounded arena (see the native-mint section of `TODOS-PERF.md`).

### mack fold failures cache panics as ordinary "no fold" (Phase 3)
`musk_interpret_mack_in_context` catches all unwinds → `None`, conflating expected non-folds with stack exhaustion / interpreter bugs. Fix: distinguish expected `Err` / resource exhaustion / internal panic and cache only deterministic successes + expected failures. Deferred with Phase 3; the audit rated mack currently sound.

### Per-expression hoonc-oracle byte fixtures (#11)
Several strict-semantic `compiler_mint` tests state they "intentionally do not run hoonc" and encode honk's current behavior. The H0 harness covers whole-artifact byte parity at kernel granularity; per-expression byte fixtures for constructs no kernel exercises remain to be added. Deferred.

### Split the ~12.7k-line `ut/mod.rs` (#14)
`mint_inner` and `play` carry parallel rune dispatches that must stay synchronized. Deferred; also a reviewability win for landing. H2's cache-key builders and H5's eval-boundary work carved partial seams.

### find/fend/fund/fond parity markers (#15)
`find.rs`/`repo.rs` still carry `status=partial` markers and `fend` returns a generic fragment. Convert each into an executable hoon-138 parity matrix; remove the marker only when covered. Deferred.
