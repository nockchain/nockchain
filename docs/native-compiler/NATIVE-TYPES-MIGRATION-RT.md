# Native Types Migration Red-Team Review

> UPDATE 2026-06-28: RESOLVED — native hoon-138 mint now COMPLETES (~40 s release)
> with BOUNDED memory and is byte-identical to hoonc (both 2,286,744 B, `cmp`
> clean); the OOM/memory-wall that motivated this review is gone. Enforced by
> `crates/honk/tests/native_parity_138.rs`. This red-team record (RT-01…RT-18) is
> retained in full as the design/verification rationale — the findings remain
> valuable for future representation work; only this banner is added.

## Verdict

The native Type/Formula direction is probably right, but the current plan is not yet a safe executable migration plan: it overstates oracle readiness, understates non-final noun boundaries, treats several lifetime/provenance contracts as representation cleanup, and makes byte-exactness depend on underspecified behavior in formulas, lazy cores, source spots, jet registration, and typed Dynock output.

## Evidence reviewed

Reviewed primary plan `docs/native-compiler/NATIVE-TYPES-MIGRATION.md`, native-compiler docs (`README.md`, `artifact-parity.md`, `performance.md`, `source-spots.md`, `TODOS.md`, `TODOS-PERF.md`, `FIND-JET-NORMALIZATION.md`, `BATTERIES-MATCHES-STRUCTURAL-EQUALITY.md`, `SLAB-PMA-COMINGLING.md`, `DOR-DEEP-EQUALITY.md`), PMA fast-hint docs (`docs/pma/FAST-HINT-REGISTRATION.md`), native honk compiler code (`crates/honk/src/native/**`, `crates/honk/src/bin/honk.rs`, `crates/honk/src/lib.rs`, `crates/honk/src/arm_map.rs`, `crates/honk/src/pipeline.rs`, `crates/honk/tests/compiler_mint.rs`), and noun/jam/interpreter boundaries (`crates/nockapp/src/noun/{slab.rs,extensions.rs}`, `crates/nockvm/rust/nockvm/src/{noun.rs,interpreter.rs,serialization.rs}`).

## Severity scale

Critical means the plan can produce a wrong, unsafe, or unprovable migration even if the implementation follows it; High means a phase gate or cutover can be falsely green; Medium means a missing invariant or stale assumption will cost major rework; Low means clarity/hygiene.

## Findings

### RT-01 — Critical — The noun honk oracle is not complete enough to be the migration oracle
- Evidence: The plan says the noun `ut` is the oracle and native grows until output jams match it (`docs/native-compiler/NATIVE-TYPES-MIGRATION.md:320-333`), but per-expression hoonc byte fixtures are explicitly deferred (`docs/native-compiler/TODOS.md:28-29`), strict semantic tests intentionally do not run hoonc (`crates/honk/tests/compiler_mint.rs:2452-2459`), and find/fend/fund/fond/repo still carry `status=partial` parity markers (`crates/honk/src/native/ut/find.rs:38-81`, `crates/honk/src/native/ut/repo.rs:83-98`, `crates/honk/src/native/ut/repo.rs:174-176`).
- Failure mode: Native IR can be byte-identical to noun honk while preserving existing honk-vs-hoonc drift in constructs that no whole-kernel artifact exercises.
- Required plan change: Make Phase 0 include a hoonc-oracle fixture set for every migrated surface, plus executable find/fend/fund/fond/repo/tack/toss/cnts/mine parity matrices before those surfaces are used as oracle-only evidence.

### RT-02 — Critical — The parity gate is ambiguous: byte-identical means different things in different docs and recipes
- Evidence: A1 requires byte-identical kernels vs noun honk/hoonc (`docs/native-compiler/NATIVE-TYPES-MIGRATION.md:87-95`), native-compiler policy says byte-for-byte equality is the acceptance criterion and structural JAM comparison is diagnostic only (`docs/native-compiler/README.md:20-26`), but `just honk-parity` passes byte equality or a dir-hash-only difference (`justfile:87-97`) and `docs/OSS-NEXT-PLAN.md:49` repeats that tolerant gate.
- Failure mode: A native phase can be called byte-exact while relying on a comparison mode that explicitly tolerates a non-identical artifact.
- Required plan change: Define separate gates: strict `cmp` byte equality for migration acceptance, dir-hash-tolerant `jam-diff --kernel-parity` only as a diagnostic or a named kernel exception with a waiver.

### RT-03 — Critical — `Rc`/hash-consing/`Drop` does not by itself solve the memory wall
- Evidence: The plan says `Rc`, hash-consing, and `Drop` directly dissolve the memory wall (`docs/native-compiler/NATIVE-TYPES-MIGRATION.md:41-46`) and that the frame arena is removed because `Rc`/`Drop` replaces it (`docs/native-compiler/NATIVE-TYPES-MIGRATION.md:281-282`), but the current perf doc ranks the leaked never-freed bump slab plus monolithic `ut.mint` as root cause #1, type interning as #2, and says memory still must be bounded via a frame arena or chunked generation after interning (`docs/native-compiler/TODOS-PERF.md:45-54`).
- Evidence: The PMA/slab audit documents one deliberately leaked `Ut` slab, two long-lived eval contexts, copy-in/copy-out by convention, and eval-stack caches living on root frames (`docs/native-compiler/SLAB-PMA-COMINGLING.md:46-80`).
- Failure mode: Native types reduce duplicate type subtrees but still retain compile-session graphs, formula leaves, wrapper products, eval-stack roots, and cache entries long enough to OOM or miss A2.
- Required plan change: Add an explicit ownership/lifetime design before Phase 2: per-entry arena roots, cache budgets/eviction, output-assembly RSS accounting, eval-stack product ownership, and a proof that all non-output native graphs are dropped before the next compile entry.

### RT-04 — Critical — Bare `Noun` leaves in `Formula` are an alien-pointer hazard
- Evidence: The plan stores quoted constants, hint clues, and dbug spots as bare `Noun` leaves in native formulas (`docs/native-compiler/NATIVE-TYPES-MIGRATION.md:125-137`), while current `Compiled` deliberately avoids exposing raw formula nouns because they depend on a private slab lifetime (`crates/honk/src/lib.rs:57-60`, `crates/honk/src/lib.rs:73-78`).
- Evidence: `NounSlab::set_root` panics on allocated roots outside the slab (`crates/nockapp/src/noun/slab.rs:823-852`, `crates/nockapp/src/noun/slab.rs:1990-2003`), and safe copying requires both the source noun and its source `NounSpace` (`crates/nockapp/src/noun/extensions.rs:136-190`); release-mode `NounSpace` skips range checks on the no-PMA identity fast path (`crates/nockvm/rust/nockvm/src/noun.rs:401-410`).
- Failure mode: `Formula::to_noun(&mut fresh_slab)` can embed an eval-stack/slab/parser noun with the wrong provenance, panic when rooted, or pass release-mode dereferences until a later impossible-to-localize wrong-arena access.
- Required plan change: Replace every bare noun leaf in the architecture with a provenanced owned leaf type, such as `(root, source_space/brand/owner)` or jam bytes, and require `to_noun` to deep-copy leaves into the destination slab through a checked copy API.

### RT-05 — Critical — Lazy cores are a semantic lifetime/scope contract, not only resolver-id churn
- Evidence: The plan deletes `lazy_resolver_next_id` in favor of `Rc<LazyBattery>` identity (`docs/native-compiler/NATIVE-TYPES-MIGRATION.md:220-242`), but current code intentionally never clears lazy resolvers because cached types embed `[%lazy 1 id]` semis and clearing/reusing ids would dangle or cross-wire references (`crates/honk/src/native/ut/mod.rs:2027-2035`).
- Evidence: Resolver registration copies `core_type` and arm AST nouns to base because another core's arm can reference this one long after its frame popped (`crates/honk/src/native/ut/mod.rs:4947-4965`), compiled arm formulas are cached for the whole compile because re-minting in the caller's `%hold`/fan scope is wrong (`crates/honk/src/native/ut/mod.rs:5062-5075`), and `mint_core` leaves the resolver registered forever after creating the lazy and final core types (`crates/honk/src/native/ut/mod.rs:6962-6992`).
- Failure mode: Native `Rc<LazyBattery>` can be dropped, evicted, or recomputed under the wrong fan scope while a type/formula/fold still needs it, producing blocked batteries, wrong constant folds, or impossible `Type::to_noun` for Dynock.
- Required plan change: Make Phase 2 define who owns lazy batteries until all output/eval boundaries finish, whether `Type::to_noun` forces or faithfully serializes unresolved lazy batteries, and how per-arm formula memoization preserves the defining fan scope.

### RT-06 — High — Type builders encode semantics; a simple enum replacement will not preserve current behavior
- Evidence: Current constructors erase wrappers and normalize types: face(void) collapses to void (`crates/honk/src/native/ut/mod.rs:12027-12039`), hint(void) and hint(noun) collapse (`crates/honk/src/native/ut/mod.rs:12041-12051`), core(void, coil) collapses (`crates/honk/src/native/ut/mod.rs:12058-12068`), and fork construction flattens/omits through set insertion (`crates/honk/src/native/ut/mod.rs:12070-12076`, `crates/honk/src/native/ut/mod.rs:11222-11229`).
- Failure mode: Native `Type::Face`, `Type::Hint`, `Type::Core`, and `Type::Fork` can preserve wrappers that noun constructors currently erase, changing `nice`, `nest`, cache keys, arm maps, and typed Dynock bytes even when the high-level algorithm is unchanged.
- Required plan change: State constructor normalization as `TypeTable` smart-constructor invariants and add fixtures for every simplification before porting any callsite.

### RT-07 — High — `Type::Fork(BTreeSet<Rc<Type>>)` is not a byte-exact representation for typed Dynock
- Evidence: The plan proposes `Fork(BTreeSet<Rc<Type>>)` with canonical ordering (`docs/native-compiler/NATIVE-TYPES-MIGRATION.md:187-198`, `docs/native-compiler/NATIVE-TYPES-MIGRATION.md:471-474`), but `%fork` is currently emitted as a Hoon set treap built by `set_put_mug`, ordered by noun mug with `dor` fallback and rotations through `gor_mug`/`mor_mug` (`crates/honk/src/native/ut/mod.rs:11134-11212`).
- Evidence: Typed Dynock emits the full inferred type noun in both library and bin paths (`crates/honk/src/lib.rs:101-108`, `crates/honk/src/bin/honk.rs:2732-2738`).
- Failure mode: A logically equivalent native fork sorted by Rust structural order, structural hash, or pointer identity can serialize to a different Hoon treap shape; Standard output may hide it, but `--dynock-typed` will not.
- Required plan change: Separate internal fork canonicalization from output serialization and require `Type::to_noun` to rebuild exact Hoon set/map treaps with current `gor_mug`/`mor_mug`/`dor` semantics, including mug-collision fixtures.

### RT-08 — High — `Formula::Slot(u64)` is too narrow for the stated semantics
- Evidence: The target `Formula` sketch uses `Slot(u64)` (`docs/native-compiler/NATIVE-TYPES-MIGRATION.md:141-155`), but native-compiler policy says Nock axis handling must support arbitrary-size atoms (`docs/native-compiler/README.md:41-46`), the interpreter stores Nock 0/9/10 axes as `Atom` rather than `u64` (`crates/nockvm/rust/nockvm/src/interpreter.rs:158-160`, `crates/nockvm/rust/nockvm/src/interpreter.rs:1472-1477`, `crates/nockvm/rust/nockvm/src/interpreter.rs:1572-1585`, `crates/nockvm/rust/nockvm/src/interpreter.rs:1592-1605`), and honk has `slot_formula_axis_big` plus BigUint axis helpers (`crates/honk/src/native/ut/mod.rs:11013-11016`, `crates/honk/src/native/ut/mod.rs:11852-11923`).
- Evidence: Mack already has an axis-noun variant that copies a non-`u64` axis into the eval stack before building `[9 axis [0 1]]` (`crates/honk/src/native/ut/mod.rs:5907-5924`).
- Failure mode: Large axes are rejected, truncated, or shoved through opaque `Op` escape hatches, making byte-exact formula output and interpreter semantics depend on untyped side paths.
- Required plan change: Define an explicit native axis type, e.g. `Axis::Small(u64) | Axis::Big(Rc<BigUint>)` or an owned atom axis, and gate Phase 1 on indirect-atom axis round-trip and interpreter fixtures.

### RT-09 — High — Formula IR must preserve peephole constructor rules and port every emit site, not just obvious mint formulas
- Evidence: Current `cons`, `comb`, and `cond` are semantic smart constructors that inspect formula nouns and apply hoon-138 rewrite order (`crates/honk/src/native/formula.rs:6-14`, `crates/honk/src/native/formula.rs:16-76`, `crates/honk/src/native/formula.rs:97-108`).
- Evidence: Formula emission happens outside the obvious `mint_inner` path: `find.rs` composes synthetic formulas (`crates/honk/src/native/ut/find.rs:99-103`, `crates/honk/src/native/ut/find.rs:151-153`), `fire.rs` emits op-9 formulas (`crates/honk/src/native/ut/fire.rs:9-18`), `hike_formula` emits op-10 edit trees (`crates/honk/src/native/ut/mod.rs:6316-6337`), hints and notes emit op-11/op-12 (`crates/honk/src/native/ut/mod.rs:6165-6173`, `crates/honk/src/native/ut/mod.rs:6459-6478`), and core construction quotes batteries into formulas (`crates/honk/src/native/ut/mod.rs:6980-6989`).
- Failure mode: Porting only `formula.rs` and common `mint` cases leaves mixed noun/native formulas, repeated conversion seams, or logically equivalent but byte-different Nock trees.
- Required plan change: Add an emit-site checklist generated from direct Nock opcode construction and require Phase 1 to leave no production direct `T(... D(op) ...)` formula construction outside `Formula::to_noun`, wrapper fixtures, or explicitly named noun-boundary adapters.

### RT-10 — High — There are more noun boundaries than final output plus mack/fold
- Evidence: The plan says nouns remain for final output, quoted constants, hints, dbug spots, cold-state/wrapper assets (`docs/native-compiler/NATIVE-TYPES-MIGRATION.md:135-137`) and adds mack/fold as a second boundary (`docs/native-compiler/NATIVE-TYPES-MIGRATION.md:301-317`), but current `NativeCompiler` still mints noun `(ty, formula)`, derives `ArmMap` from `TypeNoun`, and returns a noun-backed `CompiledNative` (`crates/honk/src/native/mod.rs:34-60`).
- Evidence: `ArmMap::from_type` traverses noun type coils and tomes (`crates/honk/src/arm_map.rs:41-53`, `crates/honk/src/arm_map.rs:166-221`), CLI wrapper/vase paths construct nouns mid-pipeline (`crates/honk/src/bin/honk.rs:2007-2012`, `crates/honk/src/bin/honk.rs:2033-2058`, `crates/honk/src/bin/honk.rs:2740-2793`), standard output interprets a formula, copies a product trap out of a transient frame, and jams through the NockStack jammer (`crates/honk/src/bin/honk.rs:2170-2271`, `crates/honk/src/bin/honk.rs:3515-3520`).
- Failure mode: A clean `Compiled { Rc<Formula>, Rc<Type> }` API can pass library direct-jam tests while CLI native builds, wrapper asset dumps, data imports, standard kernel products, and arm-map consumers still require noun types/formulas.
- Required plan change: Add a non-final noun-boundary matrix covering ArmMap, vase/trap wrappers, wrapper assets, data-import vases, standard product evaluation, cold-state/musk contexts, and both library and bin Dynock paths; decide native analogue vs `ToNoun` at each boundary.

### RT-11 — High — Mack/fold integration is under-specified and still entangled with provenance and panic-cache work
- Evidence: The plan correctly names mack/fold as a second `to_noun` boundary and says fold caches must be re-keyed (`docs/native-compiler/NATIVE-TYPES-MIGRATION.md:301-317`), but current mack catches all interpreter panics/exhaustion as `None` (`crates/honk/src/native/ut/mod.rs:5848-5892`) and TODOs still flag panic-vs-no-fold classification as open (`docs/native-compiler/TODOS.md:25-26`).
- Evidence: `musk_araw_uncached` is itself a formula consumer that recursively decodes arbitrary formula nouns by opcode, including op-11 hint handling (`crates/honk/src/native/ut/mod.rs:5486-5768`), and mack caches still key by raw core address plus axis (`crates/honk/src/native/ut/mod.rs:5803-5834`).
- Failure mode: Adding `formula.to_noun` only immediately before `interpret` leaves strict `^~`/seminoun analysis either bypassing native identity, repeatedly converting formulas, or caching failures/panics as ordinary no-fold results.
- Required plan change: Treat `musk_araw` as a first-class Formula consumer boundary, classify mack outcomes into cacheable folded/no-fold vs uncached exhausted/internal-panic, and merge this with the eval-boundary/provenance cleanup before Phase 1 is considered green.

### RT-12 — High — `%fast`/jet behavior is a runtime state contract, not merely a Formula variant
- Evidence: The plan splits `JetHint`, `NoteHint`, and `Dbug` and notes `%fast` registry side effects (`docs/native-compiler/NATIVE-TYPES-MIGRATION.md:149-168`), but `%fast` is dynamically evaluated after the hinted body, reads `chum` and a parent formula from the clue, registers into cold state, and may rebuild the warm table (`docs/pma/FAST-HINT-REGISTRATION.md:61-77`, `docs/pma/FAST-HINT-REGISTRATION.md:79-103`, `docs/pma/FAST-HINT-REGISTRATION.md:355-383`).
- Evidence: Honk requires transparent-hint-blind matching at both the warm formula lookup and `Batteries::matches`; otherwise jets silently never fire or registration cascades fail (`docs/native-compiler/FIND-JET-NORMALIZATION.md:1-15`, `docs/native-compiler/FIND-JET-NORMALIZATION.md:23-27`, `docs/native-compiler/BATTERIES-MATCHES-STRUCTURAL-EQUALITY.md:1-29`).
- Failure mode: Native Formula can emit byte-exact op-11 nouns but run under the wrong `Context` dispatch mode, fail hint-blind ancestry checks, lose compiler jets, and then change fold behavior through timeouts or budgets.
- Required plan change: Phase 1 must assert honk eval and musk contexts use the intended jet dispatch mode, include `%fast` registration cascade tests, include `Batteries::matches` hint-blind tests, and add a jets-fired/NoJet canary for compiler arms.

### RT-13 — High — AST noun round-trip removal is entangled with hold/fan/lazy semantics
- Evidence: The plan says the target drops `hoon_to_noun`/decode round-trips and ports `hoon_ast_lookup_result`/repo/play sites to native AST (`docs/native-compiler/NATIVE-TYPES-MIGRATION.md:128-134`), but current `busk` stores Hoon genes as noun face tools (`crates/honk/src/native/ut/mod.rs:4337-4343`), arm maps store Hoon AST as noun map values (`crates/honk/src/native/ut/mod.rs:7054-7067`), rest/repo resolves hold genes by noun-to-AST lookup before `play` (`crates/honk/src/native/ut/repo.rs:67-80`), lazy arm compilation decodes `arm_entry.hoon_noun` (`crates/honk/src/native/ut/mod.rs:5045-5058`), and fallback decoding uses `noun_to_hoon` caches (`crates/honk/src/native/ut/mod.rs:7826-7877`).
- Failure mode: Converting `Hold { subject, gene: Rc<Hoon> }` without native replacements for face tools, arm/tome maps, exact AST lookup, and fan/rest keys breaks recursive hold detection or changes which AST identity a cached type refers to.
- Required plan change: Split AST migration into explicit native data structures: arm maps keyed by terms with `Rc<Hoon>`, face tools that can encode busk/tune payloads without noun genes, fan/rest keys over canonical `(Type,Hoon)` identities, and tests for parsed-AST holds plus decoded hold nouns.

### RT-14 — High — Caches need a semantic migration matrix, not a delete/shrink promise
- Evidence: The plan says most caches shrink or disappear and raw/mug/AST caches are deleted (`docs/native-compiler/NATIVE-TYPES-MIGRATION.md:271-283`), but current cache-key docs say boundary keys must carry vet, fan context, arm epoch, and placeholder context to avoid roswell-class stale hits (`crates/honk/src/native/ut/types.rs:133-162`).
- Evidence: The H2 audit found seven cache surfaces with latent `%rest`/`%hold` context omissions, accepted a roswell slowdown from extra misses, and still lists follow-ups (`docs/OSS-NEXT-PLAN.md:57-61`); frame invalidation clears many caches but intentionally not lazy resolvers (`crates/honk/src/native/ut/mod.rs:7479-7521`).
- Failure mode: Identity-keyed native caches that omit semantic context reintroduce stale hits even if `Rc<Type>` equality is perfect; deleting fold or lookup caches can also hide catastrophic performance regressions until late phases.
- Required plan change: Add a cache matrix with old key, native key, semantic context fields, lifetime owner, invalidation trigger, and proof that each removed cache had no semantic dependency beyond interned identity.

### RT-15 — Medium — Dbug/source spots are cross-cutting state, not only `Formula::Dbug`
- Evidence: The plan correctly requires stack-ordered dbug spots in formulas (`docs/native-compiler/NATIVE-TYPES-MIGRATION.md:169-173`), but source-spot policy says spots and dbug values are artifact data and must not be normalized away (`docs/native-compiler/README.md:20-26`, `docs/native-compiler/source-spots.md:1-10`).
- Evidence: Parser dbug mode changes the AST (`crates/honk/src/pipeline.rs:52-64`, `crates/honk/src/pipeline.rs:145-156`, `crates/honk/src/pipeline.rs:186-191`), cache signatures intentionally include spots because mint returns formula nouns with `%spot` hints (`crates/honk/src/native/ut/mod.rs:323-333`, `crates/honk/src/native/ut/mod.rs:2370-2373`), and `mint_dbug` also pushes error-location context before emitting op-11 spot hints (`crates/honk/src/native/ut/mod.rs:7664-7681`).
- Failure mode: Formula output can preserve spots while native AST/cache/error paths alias `--dbug` and `--no-dbug`, lose nested source locations, or make chunked mint non-byte-exact.
- Required plan change: Treat dbug as a phase-wide input for parse, signatures, caches, error metadata, formula output, and wrapper parity; add nested-spot fixtures with dbug on/off.

### RT-16 — Medium — Output assembly can recreate the memory problem before jam even if final bytes dedup
- Evidence: The plan's byte-safety argument relies on jam structural dedup (`docs/native-compiler/NATIVE-TYPES-MIGRATION.md:291-299`), and `NockJammer` indeed backrefs by mug plus structural equality (`crates/nockapp/src/noun/slab.rs:928-970`, `crates/nockapp/src/noun/slab.rs:1031-1098`).
- Evidence: `copy_into` preserves sharing only within one copy traversal through a local pointer map (`crates/nockapp/src/noun/extensions.rs:140-190`, `crates/nockapp/src/noun/slab.rs:664-730`).
- Failure mode: `Formula::to_noun` can copy the same huge quoted constant or battery leaf repeatedly into the destination slab, pass byte parity because jam backrefs it later, and still OOM during output assembly.
- Required plan change: Give `Formula::to_noun`/`Type::to_noun` a destination-slab copy cache keyed by leaf identity/provenance or structural hash, and measure peak RSS for output assembly separately from final jam size.

### RT-17 — Medium — Dynock is underspecified and has two production implementations
- Evidence: The plan says Dynock output is `[type formula]` and needs narrow `Type::to_noun` (`docs/native-compiler/NATIVE-TYPES-MIGRATION.md:55-60`, `docs/native-compiler/NATIVE-TYPES-MIGRATION.md:405-408`), but current library output is `[type (trap nock)]`, untyped Dynock deliberately uses `%noun`, and typed Dynock retains the inferred type (`crates/honk/src/lib.rs:88-108`, `crates/honk/src/lib.rs:112-118`).
- Evidence: The CLI has a separate `jam_dynock_output_native` and separate `--dynock`/`--dynock-typed`/batch mode dispatch (`crates/honk/src/bin/honk.rs:76-112`, `crates/honk/src/bin/honk.rs:2296-2302`, `crates/honk/src/bin/honk.rs:2732-2738`).
- Failure mode: Implementing `Type::to_noun` only for the library path or only for typed output leaves CLI/batch Dynock still expecting noun types, or changes untyped Dynock from `%noun` to inferred-type bytes.
- Required plan change: Phase 5 must cover library and bin paths together, explicitly preserve untyped `%noun` vs typed inferred-type behavior, and add separate `--dynock` and `--dynock-typed` fixtures.

### RT-18 — Medium — Current branch hygiene assumptions are stale enough to mislead implementers
- Evidence: The plan says the Step-2 chunked-prelude WIP is obsolete and should be dropped (`docs/native-compiler/NATIVE-TYPES-MIGRATION.md:487-491`), but current code actively routes canonical prelude mint through chunked mint when the peeled root is `=<` (`crates/honk/src/bin/honk.rs:2603-2617`) and the chunked implementation says it is not byte-exact under `dbug=true` yet (`crates/honk/src/bin/honk.rs:2486-2500`).
- Evidence: Adjacent docs still refer to `open/...` paths and Bazel targets (`docs/native-compiler/README.md:3-14`, `docs/native-compiler/README.md:28-34`, `docs/native-compiler/artifact-parity.md:7-20`), while this checkout uses `crates/honk`, `crates/hatch`, `crates/hoonc`, and `hoon/common/hoon.hoon`.
- Failure mode: An implementer can run stale commands, use non-byte-exact chunked output as Phase 3 evidence, or delete active code that currently gates native-parity memory experiments.
- Required plan change: Before implementation, reconcile docs with the current checkout, classify chunked mint as delete/quarantine/production, and replace all stale `open/` commands with in-repo equivalents or mark them explicitly as exported-tree docs.

## Recommended gate rewrite
- Phase 0 must become the evidence phase: strict byte-equality harness, hoonc-oracle per-expression fixtures, emitted-formula fixture corpus, current path/command reconciliation, provenance leaf design, cache matrix, non-final boundary matrix, and a decision on chunked mint.
- Phase 1 must be Formula IR shadowing with smart constructors, a complete emit-site inventory, provenanced noun leaves, big-axis support, direct formula jam parity, mack/araw input parity, `%fast`/jet canaries, dbug on/off fixtures, and no production direct Nock opcode construction outside approved boundaries.
- Phase 2 must be Type smart constructors plus interning, not only enum replacement; it must include constructor normalization, fork/map/set serialization rules, AST-native data structures, lazy-battery ownership, and cache-key semantic context.
- Phase 3 must prove the memory thesis with RSS attribution, ownership/drop accounting, output-assembly RSS, lazy battery lifetimes, eval-stack product lifetimes, and a bounded compile-session cache story before claiming A2.
- Phase 4 must be cache/perf hardening against a recorded baseline, with roswell gate status resolved to one current number and no ambiguous “accepted” escape hatch unless a named waiver exists.
- Phase 5 must cover typed and untyped Dynock across both library and CLI/batch paths, with exact `[type (trap nock)]` fixtures and `%noun` untyped behavior preserved.
- Phase 6 should delete noun `ut` only after every non-final boundary either has a native equivalent or an explicit `ToNoun` adapter, and after no stale direct noun type/formula consumer remains.

## Bottom line
The plan is a strong architecture sketch but not yet a migration contract; the red-team bar is to turn every “native representation should make this go away” claim into an ownership invariant, a byte-exact fixture, a cache/lifetime matrix entry, or an explicit deleted boundary.
