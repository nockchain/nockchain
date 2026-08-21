# honk: Native-Types Migration Plan (v2)

Migrate honk's *working* representation of the Hoon type system and Nock output
from **Nouns** to **native Rust data structures** (`Type`, `Formula` enums with
`Rc` sharing + hash-consing), emitting Nouns only at well-defined, provenanced
boundaries. Dedicated branch effort.

Status: HISTORICAL IMPLEMENTATION PLAN, **v2**. The arena migration is now
implemented; `ARENA-HOON-IR.md`, `ARENA-TYPE-IR.md`, `ARENA-FORMULA-IR.md`,
`ARENA-SEMINOUN-IR.md`, and `POST-ARENA-PERFORMANCE-FRONTIER.md` describe the
landed architecture and validation. The OOM and noun-based design below record
the starting point, not the current compiler.
Review provenance: self-reviewed once, then **red-teamed** against the honk
source — see `NATIVE-TYPES-MIGRATION-RT.md` (18 findings RT-01…RT-18, all
verified against code: 17 confirmed, RT-01 mostly-confirmed). This v2
incorporates every finding; §"Appendix A" is the finding→section traceability
matrix. Where the red team's framing is calibrated rather than adopted verbatim
it is called out inline (RT-01 severity, RT-03 "necessary-but-not-sufficient").

The governing bar (the red team's, adopted): **every "native representation
makes this go away" claim must become an ownership invariant, a byte-exact
fixture, a cache/lifetime matrix entry, or an explicitly named boundary.** This
plan is written to that bar.

---

## 0. Why

honk reimplements `++ut` in Rust but represents compiler **types** and
**formulas** as `Noun`s in a grow-only `NounSlab`, mirroring hoon-138's noun
encodings. That is the root of what this branch fixes:

- **Memory.** Native hoon-138 mint OOMs: ~32 GB peak, no convergence (linear
  ~3 GB/min). Cause = honk builds structurally-equal type nouns **with no
  construction-time sharing**, compounded by **subject-deepening** (`mint_core`
  embeds the whole subject per core, O(N²)) and **resolver-id churn** (fresh
  `lazy_resolver_next_id` per core defeats dedup), all in a never-freed slab. The
  H7 frame arena proved per-arm *scratch* reclamation cannot bound this; the mass
  is preserved, shared, un-interned structure.
- **A persistent bug class** (all artifacts of nouns-as-working-representation):
  pointer-keyed caches that dangle, mug collisions, `noun_eq` deep walks,
  `as_raw()` machinery, dbug-spot preservation, frame-arena `copy_to_base`
  duplication.
- **Maintainability:** ~12.7k-line `ut` dispatching on noun tags via string
  compares and hand-decoded shapes, with no compiler-enforced type structure.

**The bet:** native Rust enums give structural sharing (`Rc`), hash-consing (an
intern table with cached structural hash + `Rc::ptr_eq` short-circuit — real
hashing, not mugs), and reclamation (`Drop`). We pay a `to_noun` at well-defined
boundaries.

### 0.1 Calibration: native interning is necessary, not sufficient (RT-03)

Native interning removes the **dominant** memory sources — the leaked grow-only
*type* slab and the O(N²) subject-deepening (those genuinely disappear: shared
subtrees become one `Rc`, dropped when unreferenced). But it does **not** by
itself hit acceptance criterion A2, because other retained graphs survive:
- the long-lived **musk/nockvm eval contexts** (`SLAB-PMA-COMINGLING.md:46-80`
  documents one deliberately leaked `Ut` slab, two long-lived eval contexts,
  copy-in/out by convention, eval-stack caches on root frames);
- **wrapper/vase products**, **fold results**, and **cache entries** held across
  a compile session;
- **output-assembly** intermediates (see §3.9 / RT-16).

So A2 requires an explicit **ownership/lifetime design** (§4), not a hand-wave
that "`Drop` replaces the frame arena." `TODOS-PERF.md:45-54` independently ranks
the leaked slab + monolithic `ut.mint` as root cause #1 and type interning as #2,
and says memory must still be bounded *after* interning — consistent with this
calibration.

### 0.2 The enabling fact

honk's output is **formula-only in the normal modes**:
- **Standard / Arbitrary** (kernels + parity): output jam is **only the Nock
  formula** — a `[battery payload]` trap; the Hoon *type* never appears in the
  bytes (`honk.rs:2170-2271, 2047-2058`). Types are internal scaffolding.
- **Dynock** has two production variants (RT-17): library `[type (trap nock)]`
  and CLI `jam_dynock_output_native`; **untyped** Dynock deliberately emits
  `%noun`, **typed** (`--dynock-typed`) retains the inferred type
  (`lib.rs:88-118`, `honk.rs:2296-2302, 2732-2738`).

So in the dominant path types can be fully native with no byte constraint; only
**formulas** always need `to_noun`, plus **typed-Dynock types** (narrow, but
real, and must rebuild exact Hoon treaps — RT-07).

---

## 1. Goals, non-goals, success criteria

### Goals
1. Bound native hoon-138 mint memory so self-hosting is feasible (target:
   converges in available RAM, stretch: competitive with embedded path).
2. Faster honk (kill mug walks, noun decode, deep `noun_eq`; recover roswell <60s).
3. Preserve **byte-exact output parity** at every step (the contract, §2).
4. Eliminate the noun-representation bug class.
5. Maintainability: exhaustive `match` over a small, enforced, *normalizing* type
   algebra.

### Non-goals
- Changing `++ut` **semantics** (nest/mint/mull/find/fish/crop/fuse/fire/redo are
  **frozen as-is**, including any pre-existing behavior; this is a representation
  change). The roswell wet-polymorphism gap and find/fend matrices are
  **orthogonal** algorithm work (§5.3 / RT-01).
- Changing the output contract (the user's Nock is unchanged).
- Rewriting nockvm / `NounSlab` — they remain for I/O, output jamming, the
  in-compile interpreter, and provenanced noun leaves.

### Success criteria (acceptance)
- **A1 (strict).** Every currently-passing kernel (dumb, wal, miner, peek,
  bridge) compiled by native honk is **`cmp`-identical** to the hoonc reference,
  **not** merely dir-hash-tolerant (RT-02). roswell tracked separately (§5.3).
- **A2.** `--native-parity` hoon-138 mint **completes** with bounded RSS,
  supported by the §4 ownership/drop accounting (not just a peak number) — RT-03.
- **A3.** `cargo nextest run` green; strict parity harness green; debug build
  green (validation regime on).
- **A4.** roswell native compile under the 60s gate, against a recorded baseline,
  with **one current number** and no ambiguous "accepted" escape hatch absent a
  named waiver (RT-02).
- **A5.** noun `ut` retired; every non-final noun boundary (§3.10) has a native
  equivalent or a named `ToNoun` adapter; `NounSlab` use in honk is
  boundary-only.

---

## 2. The parity contract and gate (RT-01, RT-02)

### 2.1 What must be byte-exact

| Surface | Constraint | Producer |
|---|---|---|
| Nock **formula** (all modes) | byte-exact incl. quoted constants, `%fast`/`%spot`/`%note` hints, dbug spots | `Formula::to_noun` |
| Hoon **type** (Standard/Arbitrary) | none — internal only | n/a |
| Hoon **type** (typed Dynock) | byte-exact `[%tag …]`, incl. exact `%fork`/set/map **treap** shape | `Type::to_noun` |
| **untyped** Dynock | must stay `%noun` (not inferred type) — RT-17 | constant |
| cold-state / wrapper / vase / arm-map / data-import (§3.10) | per-boundary decision | native or named adapter |

### 2.2 The gate is strict `cmp`, not dir-hash-tolerant (RT-02)

`just honk-parity` today passes byte-equality **or** a dir-hash-only difference
(`justfile:87-97`, echoed `OSS-NEXT-PLAN.md:49`), while `README.md:20-26` calls
byte-for-byte the acceptance criterion and structural compare diagnostic-only.
For migration acceptance we **split the gates**:
- **Acceptance gate:** strict `cmp` byte-equality of the output jam.
- **Diagnostic:** `jam-diff --kernel-parity` (structural, dir-hash tolerant) for
  *localizing* a failure only.
- **Waiver:** any kernel that legitimately differs only by the sandbox dir-hash
  leaf is a *named exception with a written waiver*, not a silent tolerant pass.

Why this matters: a native phase could be "byte-exact" while leaning on a
comparison mode that tolerates a non-identical artifact, masking a real diff.

### 2.3 The oracle is incomplete — fix it in Phase 0 (RT-01)

The plan uses the noun `ut` as oracle (native grows until output matches it). But
the noun honk is **not** a complete hoonc oracle: per-expression hoonc byte
fixtures are deferred (`TODOS.md:28-29`), strict semantic tests intentionally do
not run hoonc (`compiler_mint.rs:2452-2459`), and find/fend/fund/fond/repo carry
`status=partial` parity markers (`find.rs:38-81`, `repo.rs:83-98,174-176`). So
native byte-matching the noun honk can **inherit the noun honk's own drift from
hoonc** on constructs no whole-kernel artifact exercises.

Calibration (RT-01 severity): the six kernels are broad real-world coverage, so
this is **High**, not Critical — but the fix is mandatory: Phase 0 stands up a
**hoonc-oracle fixture set** for every migrated surface plus **executable
parity matrices** for find/fend/fund/fond/repo/tack/toss/cnts/mine *before* those
surfaces are used as oracle-only evidence.

---

## 3. Target architecture

### 3.1 The native/noun split

- **Types fully native.** Children `Rc<Type>`; auras/face-names interned symbols;
  `%hold` genes as native AST `Rc<Hoon>` — no noun leaves in types.
- **Formulas native with *provenanced* leaves** (RT-04, §3.9): quoted constants,
  hint clues, dbug spot tuples are owned, provenance-tagged leaves, never bare
  `Noun`.
- **AST round-trip dropped (target).** Today `ut` round-trips the native `Hoon`
  AST through nouns (`hoon_to_noun` `ut/mod.rs:6761`, `hoon_ast_lookup_result`,
  `decode_hold_hoon_ast`) and that round-trip is **entangled** (RT-13): `busk`
  stores genes as noun face tools (`4337-4343`), arm maps store AST as noun map
  values (`7054-7067`), rest/repo resolves hold genes by noun→AST lookup before
  `play` (`repo.rs:67-80`), lazy arm compilation decodes `arm_entry.hoon_noun`
  (`5045-5058`), and fallback uses `noun_to_hoon` caches (`7826-7877`). Dropping
  it therefore requires **native replacements** for each (§3.6), not a free win.
- **Nouns remain** for: final output, provenanced formula leaves, the in-compile
  interpreter (§3.10), cold-state/wrapper assets, and any boundary that keeps a
  noun analogue.

### 3.2 Formula IR

```
enum Axis { Small(u64), Big(Rc<BigUint>) }    // RT-08: axes are arbitrary atoms, not u64

enum Formula {
    Slot(Axis),                                // [0 axis]
    Quote(Leaf),                               // [1 const]   — provenanced leaf (§3.9)
    Eval(Rc<Formula>, Rc<Formula>),            // [2 …]
    Cell(Rc<Formula>, Rc<Formula>),            // autocons
    Op(u8, …),                                 // 3/4/5/7/8/9/10/12
    Cond(Rc<Formula>, Rc<Formula>, Rc<Formula>), // [6 p q r]
    // Hint kinds are NOT interchangeable (RT-12): different to_noun AND different
    // compile-time semantics; do NOT collapse:
    JetHint  { clue: Leaf, body: Rc<Formula> }, // [11 [%fast …] body] — runtime jet-registry contract (§3.11)
    NoteHint { note: Leaf, body: Rc<Formula> }, // [11/12 …] type/typo notes (play path too)
    Dbug     { spot: Leaf, body: Rc<Formula> }, // [11 spot body] — stack-ordered source spot
}
```

Required properties:
- **Smart constructors, not passthrough (RT-09).** `cons`/`comb`/`cond` are
  semantic peephole optimizers applying hoon-138 rewrite order
  (`formula.rs:6-14,16-76,97-108` — verified: `cons` folds constants, `comb` does
  the peg/`[8 x buz]`/`mal` rewrites, `cond` folds true/false/axis-zero). The IR's
  builders **must reproduce these rewrites** or the emitted Nock differs.
- **Every emit site ported (RT-09).** Formula emission is NOT only `mint_inner`:
  `find.rs` composes formulas (`99-103,151-153`), `fire.rs` emits op-9
  (`9-18`), `hike_formula` emits op-10 edit trees (`6316-6337`), hints/notes emit
  op-11/12 (`6165-6173,6459-6478`), `play_note`/`mint_note` emit directly, core
  construction quotes batteries (`6980-6989`). Phase 1 ships an **emit-site
  inventory** and leaves **no production direct `T(… D(op) …)` formula
  construction** outside `Formula::to_noun`, wrapper fixtures, or named adapters.
- **Big axes (RT-08).** `Slot`/op-9/op-10 take `Axis`; gate on indirect-atom axis
  round-trip + interpreter fixtures (`slot_formula_axis_big` `11013-11016`,
  BigUint helpers `11852-11923`; interpreter stores axes as `Atom`,
  `interpreter.rs:158-160,1574-1603`; mack copies non-u64 axis `5907-5924`).
- **Dbug spots are stack-ordered AND phase-wide (RT-15).** Captured at mint time
  in `dbug_locations` stack order so `[11 spot …]` nesting is byte-exact; spots
  are artifact data and must not be normalized away (`source-spots.md:1-10`).
  Crucially, dbug is **not just a Formula concern** — it is a *phase-wide input*:
  the parser's dbug mode **changes the AST** (`pipeline.rs:52-64,145-156,186-191`),
  cache signatures **include spots** because mint returns `%spot`-hinted formulas
  (`ut/mod.rs:323-333,2370-2373`), and `mint_dbug` pushes **error-location
  context** before emitting the op-11 hint (`7664-7681`). The native migration
  must therefore thread dbug through parse, cache keys (§3.8), error metadata, and
  formula output as one consistent mode — `--dbug` and `--no-dbug` must not alias,
  nested source locations must survive, and chunked/segmented mint must stay
  byte-exact under dbug. Fixtures: nested-spot cases with dbug on **and** off.

### 3.3 Type representation, hash-consing, and smart-constructor normalization (RT-06)

```
enum Type {
    Void, Noun_,
    Atom { aura: Sym, constant: Option<Rc<BigUint>> },
    Cell(Rc<Type>, Rc<Type>),
    Face { name: Sym, inner: Rc<Type> },
    Hint { … },
    Fork(ForkSet),                 // see §3.4 — NOT a plain BTreeSet
    Core(Rc<Core>),
    Hold(Rc<Hold>),
}
```

- **Intern table** returns canonical `Rc<Type>`; per-node cached structural hash,
  bottom-up interning (children already canonical → O(1) amortized), `Eq`/`Hash`
  short-circuit on `Rc::ptr_eq`. The noun path dedups only at **jam time** (the
  `NounMap` mug+`noun_eq`); it does no construction-time sharing — that is the
  memory cost removed. (The earlier *top-down* mug-interning attempt regressed
  because it walked whole cores and fell to full compares on collisions;
  bottom-up + cached hash avoids this.)
- **Constructors normalize — they are NOT a 1:1 enum mapping (RT-06).** Today's
  `ty_*` erase wrappers: `ty_face_tool(void)→void` (`12027-12039`),
  `ty_hint(void)`/`ty_hint(noun)` collapse (`12041-12051`), `ty_core(void,coil)→void`
  (`12058-12068`), fork construction flattens/omits via set insertion
  (`12070-12076`, `fork_set_insert` flattens nested forks + drops void
  `11222-11229`). The native `TypeTable` must encode **every such simplification
  as a smart-constructor invariant**, with a fixture per simplification — a naive
  `Type::Face/Hint/Core/Fork` that preserves wrappers would change `nice`/`nest`,
  cache keys, arm maps, and typed-Dynock bytes even with the algorithm unchanged.
- **Canonicality is a hard invariant.** Add a debug assertion that no two live
  `Rc<Type>` are structurally-equal but pointer-distinct — a violation silently
  breaks every identity-keyed memo (§3.8) and the fan scope (§3.7).
- **Auras / names / genes must intern cleanly (RT-06 nuance):** auras and names →
  interned symbols; genes → `Rc<Hoon>` deriving `Hash`/`Eq` (or interned).

### 3.4 Forks, sets, maps: internal canonicalization vs output serialization (RT-07)

`%fork` is currently a **Hoon set treap** built by `set_put_mug`, ordered by noun
**mug** with `dor` tiebreak and `gor_mug`/`mor_mug` rotations
(`ut/mod.rs:11134-11212`). Therefore:
- **Internal** representation may use any canonical order (e.g. a sorted set keyed
  by `Rc` identity / structural hash) for dedup and fast ops.
- **Output** (typed Dynock) `Type::to_noun` must **rebuild the exact Hoon treap**
  with current `gor_mug`/`mor_mug`/`dor` semantics — a logically-equal fork in
  Rust order would serialize to a different treap shape. Standard output hides
  this; `--dynock-typed` does not.
- Fixtures: **mug-collision** fork/set/map cases proving treap shape parity.
- Same rule applies to any other type carrying a Hoon set/map treap.

### 3.5 Core, battery, coil; lazy cores as a lifetime/scope contract (RT-05)

```
struct Core { payload: Rc<Type>, garb: Garb, context: Coil, battery: Battery }
enum Battery { Full(Rc<…>), Lazy(Rc<LazyBattery>) }
struct LazyBattery {
    context: Rc<Type>, poly: Poly, fan_scope: FanScope,   // scope of DEFINITION
    arms: Rc<ArmMap>,                                      // axis -> Rc<Hoon> (native AST)
    cache: RefCell<HashMap<Axis, Rc<Formula>>>,           // per-arm compiled formula
}
```

Lazy cores are not merely resolver-id churn; they encode a **lifetime/scope
contract** (RT-05, all verified):
- Lazy resolvers are intentionally **never cleared** because cached types embed
  `[%lazy 1 id]` semis (`2027-2035`); registration copies `core_type` + arm AST to
  base because another core's arm can reference this one after its frame popped
  (`4947-4965`); compiled arm formulas are cached for the whole compile because
  **re-minting in the caller's `%hold`/fan scope is wrong** (`5062-5075`);
  `mint_core` leaves the resolver registered forever (`6962-6992`).
- Native consequences (Phase 2/3 must define, not assume):
  - **Ownership:** who holds each `Rc<LazyBattery>` until *all* output and eval
    boundaries finish. It must outlive every type/formula/fold that references it.
  - **Scope fidelity:** per-arm memoization must preserve the **defining** fan
    scope (store `fan_scope`), so resolution never recomputes under the caller's.
  - **Output:** a type carrying an unresolved lazy battery must never reach
    `to_noun` (typed Dynock) dangling — either force-resolve or faithfully
    serialize the `[%lazy …]` semi. Define which.
  - **No drop/evict while live:** `Rc` identity replaces the integer id, but the
    *retention* contract (kept for the whole compile) stays; do not evict.
- **Laziness cannot be deferred to Phase 3.** Eager cores don't terminate on
  self/sibling-referential cores (that is *why* the seminoun exists). Phase 2 must
  either bridge to the existing noun lazy-resolver or bring a minimal native
  `LazyBattery` forward.

### 3.6 `%hold`, and the AST-native data structures (RT-13)

- `Hold { subject: Rc<Type>, gene: Rc<Hoon> }` is a **finite** node; recursion is
  `repo`/`rest` expansion on demand, memoized on `Rc<Hold>` identity — **never a
  cyclic `Rc`** (cycles leak). Add a leak/cycle check.
- Dropping the AST round-trip requires explicit native replacements (RT-13):
  - **Arm/tome maps** keyed by term with `Rc<Hoon>` values (replacing noun map
    values, `7054-7067`).
  - **Face tools** (`busk`/`tune` payloads) that carry genes without noun
    encoding (`4337-4343`).
  - **fan/rest keys** over canonical `(Rc<Type>, Rc<Hoon>)` identities
    (`repo.rs:67-80`).
  - native lazy-arm AST access (replacing `arm_entry.hoon_noun` decode `5045-5058`).
  - Fixtures: parsed-AST holds **and** decoded-hold-noun cases (both code paths).

### 3.7 fan / `%rest` scope

Native: active legs keyed by `(Rc<Type> inner, Rc<Hoon> gene)` **pointer**
identity. Today this dual-keys (raw ptr → mug bucket → `noun_eq`,
`2100-2119`) precisely because nouns aren't canonical; hash-consing makes types
canonical so `Rc::ptr_eq` is exact and the triple collapses — **conditional on the
§3.3 canonicality invariant holding** (assert it).

### 3.8 Caches: a semantic-context migration matrix, not "shrink/disappear" (RT-14)

The plan must **not** claim caches just disappear. Boundary keys carry semantic
context — `vet`, fan context, arm epoch, placeholder context — to avoid
roswell-class stale hits (`types.rs:133-162`); the H2 audit found seven surfaces
with `%rest`/`%hold` context omissions and accepted a roswell slowdown to fix them
(`OSS-NEXT-PLAN.md:57-61`); frame invalidation clears many caches but
intentionally **not** lazy resolvers (`7479-7521`). Identity-keyed native caches
that drop this context reintroduce stale hits even with perfect `Rc` equality.

**Deliverable: a cache matrix** with, per cache: old key, native key, **semantic
context fields retained**, lifetime owner, invalidation trigger, and a proof that
each *removed* cache had no semantic dependency beyond interned identity. Raw/mug
caches are deleted only after their semantic context is shown to be redundant
under interning; fold/lookup caches are re-keyed (not deleted) to avoid hiding
perf regressions (RT-11, RT-16).

### 3.9 Provenanced leaves and the `to_noun` contract (RT-04, RT-16)

Bare `Noun` leaves in a `Formula`/`Type` are an **alien-pointer hazard** (RT-04,
verified): `Compiled` deliberately hides raw formula nouns due to private slab
lifetime (`lib.rs:57-60,73-78`); `NounSlab::set_root` panics on allocated roots
outside the slab; safe copy needs the source noun **and** its source `NounSpace`
(`extensions.rs:136-190`); release-mode `NounSpace` skips range checks on the
no-PMA identity fast path (`noun.rs:401-410`) — so a wrong-provenance leaf can
panic on root, or pass release-mode derefs until a later impossible-to-localize
wrong-arena access.

Contract:
- Every leaf is a **provenanced owned leaf** — e.g. `Leaf { root: Noun, owner:
  SourceSpace/Brand }` or **jam bytes** — never a bare `Noun`.
- `to_noun` **deep-copies** each leaf into the destination slab through a
  **checked** copy API (source noun + source space), never splicing a foreign
  pointer.
- **Output-assembly memory (RT-16):** `copy_into` only dedups within one
  traversal (`extensions.rs:140-190`), so `to_noun` could copy the same huge
  battery/constant repeatedly — passing byte parity (jam re-backrefs structurally,
  `slab.rs:928-970,1031-1098`) but **OOMing during assembly**. `to_noun` therefore
  needs a **destination-slab copy cache** keyed by leaf identity/provenance or
  structural hash, and Phase 3 measures **output-assembly peak RSS separately**
  from final jam size.

### 3.10 The full non-final noun-boundary matrix (RT-10, RT-11, RT-17)

There are more noun boundaries than "final output + mack/fold". `NativeCompiler`
still mints noun `(ty, formula)`, derives `ArmMap` from `TypeNoun`, returns a
noun-backed `CompiledNative` (`native/mod.rs:34-60`). Each boundary needs a
decision: **native analogue** vs **named `ToNoun` adapter**.

| Boundary | Evidence | Decision (Phase) |
|---|---|---|
| `ArmMap::from_type` (walks noun coils/tomes) | `arm_map.rs:41-53,166-221` | native arm map over `Rc<Type>` (Ph2) |
| CLI wrapper / vase / trap construction | `honk.rs:2007-2012,2033-2058,2740-2793` | adapter (Ph1/5) |
| standard output: interpret formula → copy product trap out of transient frame → jam | `honk.rs:2170-2271,3515-3520` | `to_noun` + checked copy (Ph1) |
| **mack/fold** in-compile interpreter | `musk_interpret_mack_in_context` `5848-5892` | `to_noun` before `interpret` (Ph1) |
| **`musk_araw` formula *consumer*** (decodes formulas by opcode incl op-11) | `5486-5768` | first-class Formula consumer boundary (Ph1) — RT-11 |
| mack cache (raw core addr + axis) | `5803-5834` | re-key on native identity (Ph1) |
| cold-state / musk contexts | `SLAB-PMA-COMINGLING.md` | stays noun; document (Ph0) |
| data-import vases | `honk.rs` import path | adapter (Ph5) |
| library + CLI Dynock (typed & untyped) | `lib.rs:88-118`, `honk.rs:2296-2302,2732-2738` | both, preserve `%noun` untyped (Ph5) — RT-17 |

mack/fold specifics (RT-11): mack catches all interpreter panics/exhaustion as
`None` (`5848-5892`) and the panic-vs-no-fold classification is an open TODO
(`TODOS.md:25-26`). Phase 1 must classify mack outcomes into cacheable
folded/no-fold vs uncached exhausted/internal-panic, treat `musk_araw` as a
Formula consumer (not just a `to_noun` site), and avoid repeatedly re-converting
formulas in strict `^~`/seminoun analysis.

### 3.11 `%fast`/jet is a runtime-state contract (RT-12)

`%fast` is **dynamically evaluated** after the hinted body, reads `chum` + a
parent formula from the clue, registers into cold state, and may rebuild the warm
table (`FAST-HINT-REGISTRATION.md:61-103,355-383`). honk requires **hint-blind**
matching at the warm lookup and in `Batteries::matches` or jets silently never
fire / registration cascades fail (`FIND-JET-NORMALIZATION.md`,
`BATTERIES-MATCHES-STRUCTURAL-EQUALITY.md`; this is the N3 JetDispatchMode work).
So a byte-exact op-11 noun under the **wrong `Context` dispatch mode** loses
compiler jets and then changes fold behavior via timeouts/budgets. Phase 1 must:
assert honk eval **and** musk contexts use the intended dispatch mode; include a
`%fast` registration-cascade test; include `Batteries::matches` hint-blind tests;
and add a **jets-fired / NoJet canary** for compiler arms.

---

## 4. The memory thesis and ownership design (RT-03)

A2 is *proven*, not asserted. Before Phase 2, write an explicit
ownership/lifetime design; Phase 3 supplies the evidence:

- **What native frees:** type subtrees (Rc/Drop + interning) — removes the slab
  leak and subject-deepening duplication.
- **What survives and needs accounting:** musk/nockvm eval contexts and their
  root-frame caches; wrapper/vase products; fold results; compile-session caches.
- **Design artifacts (Phase 0/2):** per-entry arena roots; cache budgets /
  eviction policy; eval-stack product ownership; a proof that **all non-output
  native graphs are dropped before the next compile entry**.
- **Evidence (Phase 3 gate):** RSS attribution by category (types / formulas /
  eval contexts / caches / output assembly); ownership/drop accounting;
  output-assembly RSS measured separately (§3.9); lazy-battery and eval-stack
  product lifetimes bounded; a bounded compile-session cache story — *before*
  claiming A2.

---

## 5. Migration strategy

### 5.1 Dual representation, noun honk as oracle — plus a real hoonc oracle
Build the native core as a new module set behind a flag (`HONK_NATIVE_IR`),
default OFF. The noun `ut` is the **interim** oracle (dual-run: compile each
corpus item both ways, **strict `cmp`** the output jams). Because the noun honk
is itself incomplete vs hoonc (§2.3), Phase 0 **also** stands up hoonc-oracle
fixtures so the migrated surfaces are checked against the true reference, not only
against noun-honk.

### 5.2 Corpus (concrete — RT-01)
(a) all 6 kernels; (b) every `compiler_mint.rs` fixture; (c) broad `hoon/` sweep
weighted to **wet/`|*` arms, `|-`/recursion, `=>`/core chains**; (d) targeted
**structurally-equal core** fixtures (prove canonical interning/dedup); (e)
**mug-collision** fork/set/map fixtures (RT-07); (f) **dbug on/off** + nested-spot
fixtures (RT-15); (g) **`%fast` cascade** + jets-fired canary fixtures (RT-12);
(h) **typed & untyped Dynock** fixtures (RT-17); (i) per-expression hoonc-oracle
fixtures + find/fend/fund/fond/repo/tack/toss/cnts/mine **parity matrices** (RT-01).

### 5.3 The algorithm tail is orthogonal — fix the oracle first (RT-01)
roswell wet polymorphism (redo/mull/nest) and find/fend matrices are **algorithm**
gaps, not representation. A buggy oracle won't catch native regressions, so
**close roswell on the noun honk first** (make the oracle 6/6) — that work
transfers to either representation. `fire_wet_rib` is semantic (cycle detection in
`mull_check_wet`) and affects both types and formulas; port it carefully.

---

## 6. Phases (the gate rewrite — RT "Recommended gate rewrite")

### Phase 0 — EVIDENCE PHASE (new; was scaffolding)
The red team's central restructure: Phase 0 produces *evidence and design*, not
code that mints types. Deliverables:
- **Strict byte-equality harness** + `just native-parity-dual` (dual-run,
  `cmp`); the diagnostic structural diff kept separate (§2.2).
- **hoonc-oracle per-expression fixtures** + find/fend/… **parity matrices** (§2.3).
- **Emitted-formula fixture corpus** (current noun formulas captured as golden).
- **Provenance leaf design** (§3.9) and **`to_noun` copy-cache** design.
- **Cache matrix** (§3.8) and **non-final boundary matrix** (§3.10), filled in.
- **Path/command reconciliation** + **chunked-mint decision** (§9 / RT-18).
- Module skeleton `native_ir/{formula.rs, ty.rs, intern.rs, core.rs, leaf.rs}`.
- **Gate:** all artifacts exist; noun path still green (no behavior change).

### Phase 1 — Formula IR shadowing
- `Formula` + `Axis` + provenanced `Leaf` + `to_noun`; **smart constructors**
  reproducing `cons`/`comb`/`cond` peephole rewrites (RT-09); three hint kinds
  (RT-12); stack-ordered dbug spots (RT-15); big axes (RT-08).
- **Complete emit-site inventory**; port `mint*` **and** `play*` **and** find.rs /
  fire.rs / hike_formula / core-battery sites; leave **no production direct
  `T(… D(op) …)`** outside `to_noun`/named adapters (RT-09).
- Wire `to_noun` at **both** boundaries: output **and** mack/fold; treat
  `musk_araw` as a Formula consumer; classify mack outcomes (RT-11).
- **Gate:** strict `to_noun` jam == noun-formula jam on 100+ expressions and all
  passing kernels; **mack/araw input parity**; **`%fast`/jet canary** green
  (jets fire); **dbug on/off** fixtures byte-exact; big-axis round-trip fixtures.

### Phase 2 — Type smart constructors + interning
- `Type` enum + intern table + **normalization invariants as smart constructors**
  (RT-06); fork/set/map internal canonicalization (RT-07); AST-native data
  structures — arm/tome maps, face tools, fan/rest keys (RT-13); a lazy-battery
  **ownership** design + Phase-2 lazy **bridge** (RT-05); cache **semantic
  context** preserved per the §3.8 matrix (RT-14).
- Port the ~476 type call sites + the AST-round-trip removal sites (module by
  module, dual-run after each).
- **Gate:** full-corpus strict parity; canonicality assertion holds (no
  equal-but-distinct live `Rc<Type>`); cache matrix shows no dropped semantic
  context.

### Phase 3 — Lazy/hold/fan native + **prove the memory thesis** (decides A2)
- Native `LazyBattery` (Rc + per-arm memo with defining fan scope), `%hold`
  finite lazy nodes, fan via Rc identity; resolver-id removed.
- **Memory evidence (§4):** RSS attribution, ownership/drop accounting,
  output-assembly RSS, lazy-battery + eval-stack product lifetimes, bounded
  compile-session cache story.
- **Gate:** full-corpus strict parity **AND** A2 (hoon-138 mint converges,
  bounded RSS, accounted).

### Phase 4 — Cache/perf hardening
- Re-keyed caches per the matrix; delete only proven-redundant caches; profile.
- **Gate:** corpus strict parity; **roswell <60s vs a recorded baseline, one
  current number, no ambiguous "accepted" without a named waiver** (RT-02/A4).

### Phase 5 — Dynock (typed + untyped, library + CLI)
- `Type::to_noun` incl. exact treap serialization (RT-07); cover **library and
  bin** paths; preserve **untyped `%noun`** vs **typed inferred-type**; exact
  `[type (trap nock)]` fixtures (RT-17).
- **Gate:** `--dynock` and `--dynock-typed` byte-exact, both paths.

### Phase 6 — Retire noun `ut`
- Delete noun `ut` **only after** every non-final boundary (§3.10) has a native
  equivalent or a named `ToNoun` adapter and no stale direct noun type/formula
  consumer remains (RT-10). Reduce `NounSlab` use to boundary-only.
- **Gate:** A1–A5 met; full suite green; docs reconciled.

---

## 7. Validation (cross-cutting)
- **Dual-run + strict `cmp`** on every change until Phase 6; hoonc-oracle fixtures
  as the external net.
- Debug-build validation on (catches provenance/representation bugs early); add
  the canonicality assertion (§3.3) and the Rc-cycle/leak check (§3.6).
- Memory/time tracked from Phase 3 with attribution (§4).

---

## 8. Risk register (amplified)
- **R1 Parity regression during port** → dual-run strict oracle, module-by-module
  under flag, retire noun path only at full corpus parity.
- **R2 Algorithm tail conflation** (RT-01) → fix roswell/find-fend on noun honk
  first; frozen semantics.
- **R3 Memory not solved by Rc alone** (RT-03) → §4 ownership design + Phase-3
  attribution; A2 is evidence-gated.
- **R4 Provenance/alien-pointer leaves** (RT-04) → provenanced leaves + checked
  `to_noun` copy.
- **R5 Lazy lifetime/scope contract** (RT-05) → ownership + defining-fan-scope memo
  + output resolution policy; no eviction.
- **R6 Constructor normalization lost** (RT-06) → smart constructors with
  per-simplification fixtures.
- **R7 Fork treap byte-exactness** (RT-07) → output serialization rebuilds exact
  treap; mug-collision fixtures.
- **R8 Axis width** (RT-08) → `Axis::Small|Big`; indirect-atom fixtures.
- **R9 Smart-constructor + emit-site coverage** (RT-09) → emit inventory; no
  stray direct Nock construction.
- **R10 Hidden noun boundaries** (RT-10) → boundary matrix; per-boundary decision.
- **R11 mack/fold + panic-cache entanglement** (RT-11) → `musk_araw` as Formula
  consumer; outcome classification; identity re-key.
- **R12 `%fast`/jet runtime-state** (RT-12) → dispatch-mode asserts; cascade +
  jets-fired canaries.
- **R13 AST round-trip entanglement** (RT-13) → native arm maps/face tools/fan
  keys; parsed + decoded-hold fixtures.
- **R14 Cache semantic context dropped** (RT-14) → cache matrix; re-key not delete
  for fold/lookup.
- **R15 dbug cross-cutting** (RT-15) → dbug as phase-wide input (parse,
  signatures, caches, errors, output); nested-spot dbug on/off fixtures.
- **R16 Output-assembly OOM before jam** (RT-16) → `to_noun` copy cache; separate
  assembly RSS.
- **R17 Dynock two impls** (RT-17) → both paths; typed vs untyped preserved.
- **R18 Stale branch hygiene / chunked is live** (RT-18) → §9; classify chunked.
- **R19 Oracle incompleteness** (RT-01) → hoonc fixtures + parity matrices before
  oracle-only reliance.
- **R20 Gate ambiguity** (RT-02) → strict `cmp` acceptance; dir-hash only as named
  waiver.
- **R21 Rc cycles in recursive types** → finite lazy `%hold`, leak check.
- **R22 Two representations coexisting** → minimize the flag window; retire
  promptly after Phase-3 parity.

## 9. Branch hygiene (RT-18 — corrected)
- **The chunked-prelude mint is ACTIVE, not obsolete.** v1 said "drop it"; that is
  wrong. Canonical prelude mint routes through chunked when the peeled root is `=<`
  (`honk.rs:2603-2617`), and chunked is **not byte-exact under `dbug=true`** yet
  (`honk.rs:2486-2500`). So it currently **gates native-parity memory
  experiments**. Decision required in Phase 0: **delete / quarantine / productize**
  — do **not** delete blindly, and do not use non-byte-exact chunked output as
  Phase-3 evidence.
- **Reconcile stale docs/commands.** Adjacent docs reference `open/…` paths and
  Bazel targets while this checkout uses `crates/{honk,hatch,hoonc}` and
  `hoon/common/hoon.hoon` (`README.md:3-14,28-34`, `artifact-parity.md:7-20`).
  Phase 0 replaces stale `open/` commands with in-repo equivalents or marks them
  as exported-tree docs.
- **Carry over:** strict + diagnostic parity harnesses, timing harness, H2 cache
  fixes, N1–N6 nockvm cleanup, the embedded-prelude shipping path.
- **Supersede:** the H7 frame arena + arena profiling (`Rc`/`Drop` + §4 replace
  them) — keep only in git history. The H7 work informed §4; it is not carried.

## 10. Sizing (rough; calibrate after Phase 1)
| Phase | Scope | Size |
|---|---|---|
| 0 | evidence: harnesses, hoonc fixtures, matrices, designs, chunked decision | ~1–2 wks |
| 1 | Formula IR + smart ctors + all emit sites + mack/araw + axis + hints + canaries | ~2–3 wks |
| 2 | Type smart ctors + interning + ~476 sites + AST-native + lazy bridge + cache context | ~3–5 wks |
| 3 | lazy/hold/fan native + memory thesis evidence (decides A2; long pole) | ~3–4 wks |
| 4 | cache/perf hardening; 60s gate | ~1–2 wks |
| 5 | Dynock typed+untyped, library+CLI | ~1 wk |
| 6 | retire noun ut + boundary cleanup | ~1 wk |

Order-of-magnitude **~12–18 weeks** (revised up: Phase 0 is now a real phase, and
RT-04/06/07/13/14 add design+fixture load). Phase 3 decides success — front-load
its risk.

## 11. First concrete steps
1. Cut the branch; land this plan + the RT doc.
2. Stand up the strict + diagnostic harnesses and the corpus (§5.2).
3. Build the hoonc-oracle fixtures + find/fend parity matrices; **close roswell on
   noun honk** so the oracle is 6/6 (R2/R19).
4. Write the provenance-leaf design, cache matrix, boundary matrix, and the
   chunked-mint decision (the Phase-0 designs).
5. Then Phase 1 Formula IR shadowing.

The whole-effort decision gate remains **Phase 3 / A2** with §4 evidence.
Everything before it is valuable regardless (parity preserved, bug class reduced,
faster); A2 is the prize.

---

## Appendix A — Red-team finding → plan traceability (all 18 incorporated)

| Finding | Severity (RT / mine) | Where addressed |
|---|---|---|
| RT-01 oracle incomplete | Critical / **High** | §2.3, §5.1, §5.2(i), Phase 0, R19 |
| RT-02 gate ambiguity | Critical | §2.2, A1, A4, Phase 4, R20 |
| RT-03 Rc≠memory win | Critical | §0.1, §4, A2, Phase 3, R3 |
| RT-04 alien-pointer leaves | Critical | §3.1, §3.9, Phase 0/1, R4 |
| RT-05 lazy = lifetime/scope | Critical | §3.5, Phase 2/3, R5 |
| RT-06 constructor normalization | High | §3.3, Phase 2, R6 |
| RT-07 fork treap byte-exact | High | §3.4, §2.1, Phase 5, R7 |
| RT-08 Slot too narrow | High | §3.2 (`Axis`), Phase 1, R8 |
| RT-09 smart ctors + emit sites | High | §3.2, Phase 1, R9 |
| RT-10 more noun boundaries | High | §3.10 matrix, Phase 6, R10 |
| RT-11 mack/fold + panic-cache | High | §3.10, §3.11, Phase 1, R11 |
| RT-12 %fast runtime contract | High | §3.11, Phase 1 canaries, R12 |
| RT-13 AST round-trip entangled | High | §3.1, §3.6, Phase 2, R13 |
| RT-14 cache semantic matrix | High | §3.8 matrix, Phase 2, R14 |
| RT-15 dbug cross-cutting | Medium | §3.2, §5.2(f), Phase 1, R15 |
| RT-16 output-assembly OOM | Medium | §3.9, §4, Phase 3, R16 |
| RT-17 Dynock two impls | Medium | §0.2, §2.1, §3.10, Phase 5, R17 |
| RT-18 chunked is live / stale docs | Medium | §9, Phase 0 decision, R18 |

Bar restated: every cell above is an ownership invariant, a byte-exact fixture, a
cache/lifetime matrix entry, or a named boundary — not a "native makes it go away."

## Phase 1 construction-port finding: no combinator-granularity seam (2026-06-17)

Attempted the first construction-port slice — route honk's formula combinators
(`cons`/`comb`/`cond`) through the native IR under a flag, bridging inputs via
`Formula::from_noun` and emitting via `to_noun`. RESULT: **infeasible — O(n²)**.
`from_noun` re-parses the entire (growing) sub-formula on every combinator call,
and combinators compose, so on real formulas it blows up quadratically (a
normally-~45s dumb compile did not finish in 6+ minutes). Reverted.

Lesson (refines the Phase-1 plan): there is **no cheap incremental seam at
combinator granularity** — per-call noun↔native bridging is quadratic. The
correct approach is **additive native-shadow threading**: each `mint`/`play`
formula site returns its native `Rc<Formula>` ALONGSIDE the noun formula, built
from its CHILDREN's already-native `Rc<Formula>` (no `from_noun` re-parse → O(n)),
with `to_noun(native) == noun_formula` asserted at the boundary as the live
oracle. Once native shadows everywhere and validates, flip the output to native
and drop the noun formula. This is the safe path, but it is a LARGE mechanical
change across every formula-producing `ut` function (return-type/threading), i.e.
a dedicated effort — not a one-turn slice.

Strategic note: the formula construction port is **structural** — formulas are
the output (~MBs), not the memory blowup. The blowup is TYPES (subject-deepening,
un-interned subtrees), so the memory win (A2) is the Phase-2/3 TYPE port. The
formula IR BOUNDARY is already fully proven (to_noun + from_noun round-trip the
entire compiled prelude + all 6 kernels byte-exact). So the open decision is
sequencing: (a) commit to the large formula-shadow construction port now
(structural; enables a clean type port), or (b) since the boundary is proven, go
straight at the Phase-2/3 native TYPE representation + hash-consing (the memory
win), threading native formulas as part of that same native-mint effort (cores
couple type+formula anyway). Recommendation: (b).

## Phase 2 finding: hash-cons table + live native-type harness (2026-06-18)

Built the memory-win keystone and the first WORKING piece of the native-mint
construction port.

KEYSTONE — `intern::TypeTable` (hash-cons): bottom-up interning returning the
canonical `Rc<Type>`; node hash/eq shallow (children by canonical pointer, leaves
by content) → O(1)/node. Proven by unit test to collapse a fully-duplicated
depth-12 cell tree (8191 structural nodes) to 13 distinct — O(2^n)→O(n), exactly
the subject-deepening fix. Plus `intern_type_noun`: a POINTER-MEMOIZED
decode-and-intern walk — the O(n) construction primitive (one shared memo across
the whole compile makes the noun DAG walked once; structurally-equal-but-pointer-
distinct subtrees collapsed by the table). This is the primitive that sidesteps
the combinator O(n²) trap (which was re-parsing with no shared memo).

LIVE HARNESS (`HONK_NATIVE_TYPES`, additive): `mint_core` now builds the interned
native type for each minted core into one persistent live table, ALONGSIDE the
noun path. Verified: (1) it works on real mint — `core_chain` interns its core
(1 core, 15 nodes); (2) it is byte-exactly ADDITIVE — output jam is identical
with and without the flag (the noun path is untouched, the live oracle preserved).

FINDING (the additive ceiling): the additive shadow proves the construction
MECHANISM works live and correctly, but it **cannot measure the at-scale memory
win**, for a fundamental reason: with the prelude EMBEDDED (normal compiles) the
app cores embed only the 14-node prelude type, so per-core dedup is tiny; and with
the prelude MINTED (`--native-parity`, the subject-deepening case) the NOUN side —
which still runs, since the shadow is additive — is exactly the 32 GB OOM we are
trying to fix, so it dies before the native table can report. An additive shadow
runs both representations, so it can never be cheaper than the noun path it
shadows. The at-scale win is only observable AFTER the FLIP: native interned types
REPLACE nouns as mint's working representation (noun construction dropped). The
keystone (table + O(n) primitive) and the live construction hook are now in place
and validated; the remaining big-bang is the flip — thread native `Rc<Type>` (and
`Rc<Formula>`) as the RETURN of every `mint`/`play` site (additive-native-shadow
threading, O(n) via children-already-native), validate `to_noun == noun`
throughout, then drop the noun path. That is the dedicated multi-week effort; all
its prerequisites (IR boundaries, intern table, O(n) primitive, live hook) are
proven.

## INC1 + INC2: native-shadow producer vocabulary (2026-06-18)

Began the return-threading port per a 5-agent mapping workflow (full ut/mod.rs
type-construction surface: 21 producers, the call graph + sut dataflow, a 202-site
constructor census, the consumer set, a 13-step dependency-ordered plan). Key
census fact: `ty_void`/`ty_noun` alone have 123 call sites, so direct return-type
changes would cascade catastrophically. Chosen representation: **additive `_n`
sibling producers** returning `(Noun, Rc<Type>)`, leaving the 202 noun-only call
sites untouched; native children passed explicitly (O(n) sharing), collapses
mirrored, all interned through one shared thread-local table.

- INC1: `ty_{noun,void,atom,cell,face,face_tool,hint,hold,core,fork,bool}_n` +
  intern accessors `live_intern`/`native_of`/`assert_native_eq`. Each builds the
  byte-identical noun via its `ty_*` sibling AND the interned native from
  already-native children; branch ctors mirror collapse by reading the result
  tag. Unit-tested byte-exact across all 9 tags + every collapse path.
- Adversarial verification (3 skeptic lenses) found one CRITICAL real bug — the
  thread-local memo (keyed by noun raw pointer) must reset per batch entry or a
  reused slab address aliases a stale `Rc` — fixed (`live_reset` per entry); plus
  an empty-aura coverage gap + a redundant-allocation cleanup. Verdict: byte-exact
  + O(n) sound.
- INC2: `cell_type_n`/`hint_type_n`/`fork_from_options_n` (the normalizing entry
  points mint/play call), each mirroring its collapse; Ut-based tests byte-exact.

The full native-returning producer vocabulary is now built + verified. What
remains is WIRING (INC3+): thread these through play_core/mint_core → mine →
play/mint → the mint_*/play_* helper fleet, carrying native `Rc<Type>` as the type
slot of each return and `sut` native as an input, with `to_noun==noun` asserted
live. This is the cascade; it pays off cumulatively (the memory win lands only once
`sut` native flows through the whole chain and the noun path is dropped at the
flip, INC13).

## INC4-6 + the flip inflection (2026-06-18)

Wired flag-gated build+assert native construction at the three meaningful node
producers, validated live by `shadow_gate.sh` (4 fixtures, flag on/off
byte-identical, no assert trip):

- INC4 `play_core`, INC5 `mint_core` (the subject-deepening site), INC6
  `cell_type` — each builds its native bottom-up via `ty_*_n`/`cell_type_n` from
  the payload/child natives and asserts byte-exact. Shipping path unchanged.

This COMPLETES the meaningful incremental validation: native *construction* at the
node constructors (core, cell) is proven on real types, and the leaf constructors
(atom/face/hint/hold/fork) are unit-tested + covered by the IR-boundary
round-trip. Going further with flag-gated build+assert is tautological (it reduces
to `native_of(result)` = the already-proven IR-boundary round-trip).

THE INFLECTION: the remaining work is the FLIP, and it cannot be done as a
shipping-safe incremental. Return-threading native through `mint`/`play` forces
native to be built on EVERY compile (a function can't conditionally return a
different type), so the not-yet-threaded `native_of` fallbacks run O(N²) on the
shipping path for the entire (multi-hundred-edit) duration of the migration. The
only ways out are (a) flag-gated DUPLICATION of the whole mint/play dispatch
(`_n` family alongside the noun family — ~600 lines of throwaway dispatch, deleted
at the flip), or (b) an atomic REPLACE big-bang (swap noun construction for native
throughout, drop nouns, non-compiling intermediate on this branch, validated
end-to-end by the kernel/fixture parity harness).

Recommendation: (b), the atomic replace, executed as a focused effort on this
branch. It reaches the clean end state directly (native-only internals, O(n), no
double-build, no dup to later delete) — the fastest route to "native types
completely". The build+assert work (INC4-6) has de-risked it: native construction
is proven correct at the constructors the replace will route through. Validation:
`shadow_gate.sh` (fast) during; full kernel byte-parity vs the current output at
completion. Scope of the replace: the type slot of mint/mint_inner/mint_core/mine,
play/play_inner/play_core, nice, wrap_type + the mint_*/play_* helper fleet
(returns), the type consumers (nest/fond/repo/type_*_parts → read `Rc<Type>`), and
emit nouns only at the output boundary (`to_noun`) + typed-Dynock.
