# Phase-0 NON-FINAL NOUN BOUNDARY MATRIX
## Plan §3.10: Complete Enumeration of Noun Type/Formula Boundaries

**Last Updated:** 2026-06-16  
**Status:** NON-FINAL DESIGN ARTIFACT  
**Scope:** All locations where honk constructs or consumes a `Noun` TYPE or `Noun` FORMULA outside the final standard-output formula jam.

---

## Overview

This matrix documents every boundary crossing where honk's native compiler constructs, transforms, or consumes noun TYPEs and noun FORMULAs **outside the final serialized output path**. The final output path (standard output interpret+copy product, formula jam) is excluded. This serves as a prerequisite inventory for Phase-1 integration of native noun adapters.

### Boundary Categories

- **MINT**: Direct product of `Ut::mint(ty, gol, expr)` 
- **ARM_MAP**: Type traversal via `ArmMap::from_type(Noun, space)`
- **CLI_WRAPPER**: CLI wrapper and vase construction
- **STANDARD_OUTPUT**: Formula interpretation and final copy
- **MACK**: Musk fold interpreter and formula evaluation
- **MUSK_CACHE**: Raw address + axis caching
- **COLD_STATE**: Cold-state and musk contexts (jet registration)
- **DATA_IMPORT**: Vase construction for file data
- **LIBRARY_DYNOCK**: Library API, typed/untyped dynock output
- **NATIVE_EXACT**: Exact wrapper battery construction (native formulae)

---

## Detailed Boundary Matrix

### 1. MINT BOUNDARY
**Primary Site:** `ut.mint(sut, gol, gen) -> (Noun, Noun)`

| Location | Line | Code | What Crosses | Decision | Phase |
|----------|------|------|--------------|----------|-------|
| `crates/honk/src/native/mod.rs` | 37-38 | `let sut = ty_noun(&mut slab);`<br/>`let gol = ty_noun(&mut slab);` | Two bootstrap type nouns (`%noun` for subject, goal) | **Native Analogue**: `sut_bootstrap_ty` → `NativeBootstrapType::Noun`, `gol_bootstrap_ty` → `NativeBootstrapType::Noun` | Phase 1 |
| `crates/honk/src/native/mod.rs` | 41 | `let (ty, formula) = ut.mint(sut, gol, expr)?;` | Input types + output formula `(ty: Noun, formula: Noun)` from `Ut::mint` | **Named Adapter**: `MintBoundary { sut, gol, output: (ty, formula) }` with validation that both match pre-registered type witnesses | Phase 1 |
| `crates/honk/src/native/mod.rs` | 43 | `let ty_noun = TypeNoun::new(ty);` | Wrapping inferred type in opaque wrapper | **Native Analogue**: No adapter needed; TypeNoun is locally-scoped identity wrapper around Noun | Inline |
| `crates/honk/src/native/mod.rs` | 45 | `let arm_map = ArmMap::from_type(&ty_noun, &space)?;` | Type noun fed into arm-map traversal (coil/tome walk) | **Named Adapter**: See ARM_MAP boundary below | Phase 1 |

**Mint Subfunction Calls:**
| Location | Line | Subfunction | What Crosses | Decision | Phase |
|----------|------|------------|--------------|----------|-------|
| `crates/honk/src/native/ut/mod.rs` | 3581 | `pub fn mint(&mut self, sut: Noun, gol: Noun, gen: &Hoon)` | Entry point; takes subject type, goal type, hoon expr | **Named Adapter**: `MintSignature { sut_mug, gol_mug, expr_hash }` cache key validation | Phase 2 |
| `crates/honk/src/native/ut/mod.rs` | 3616-3620 | `ty_noun()` calls × 6 | Bootstrap empty types for branch/pair decomposition | **Native Analogue**: `FreshTypeVar::Unbound` markers (no boundary, internal state) | Inline |

---

### 2. ARM_MAP BOUNDARY
**Primary Site:** `ArmMap::from_type(ty: &TypeNoun, space: &NounSpace) -> Result<ArmMap>`

| Location | Line | Code | What Crosses | Decision | Phase |
|----------|------|------|--------------|----------|-------|
| `crates/honk/src/arm_map.rs` | 41-43 | `pub fn from_type(ty: &TypeNoun, space: &NounSpace)` | Accepts `TypeNoun` (wrapper) and reads its underlying `Noun` via `.noun()` | **Adapter**: Implicit via `TypeNoun::noun()` getter; keep opaque boundary | Phase 0 |
| `crates/honk/src/arm_map.rs` | 47 | `let Some(coil) = find_core_coil(ty.noun(), space)?` | Recursive traversal into type tree (cell.head() tag checks for "core"/"face"/"hint"/"hold") | **Named Adapter**: `TypeTreeTraversal { tag, cell_structure }` with recursive descent policy | Phase 1 |
| `crates/honk/src/arm_map.rs` | 50 | `let tomes = coil_tomes(coil, space)?;` | Extract tomes map from coil's tail/tail | **Declarative**: Document coil layout (garb, context, tomes); no adapter needed for reads | Phase 0 |
| `crates/honk/src/arm_map.rs` | 52 | `collect_tomes(tomes, 2, &mut map, space)?;` | Walk binary tree of tomes, extracting arm names as atoms | **Named Adapter**: `TomeTreeWalk { arm_name_atoms, axis_calculations }` with atom-to-string witness | Phase 1 |

**Coil/Tome Traversal Details:**
| Location | Line | Function | Traversal Pattern | Decision | Phase |
|----------|------|----------|-------------------|----------|-------|
| `crates/honk/src/arm_map.rs` | 166-200 | `collect_tomes()` | Walks binary map tree; node is `[name-atom tail-noun]`, branches are `[left right]` | **Declarative**: Static coil grammar (garb, context, tomes layout); hardcode expected structure | Phase 0 |
| `crates/honk/src/arm_map.rs` | 203-234 | `collect_arms()` | Extracts arm names from arm dab (arm database binary map) | **Declarative**: Static arm dab grammar (name-atom pairs with axis composition) | Phase 0 |
| `crates/honk/src/arm_map.rs` | 219 | `term_from_noun()` | Decode atom to UTF-8 string | **Named Adapter**: `AtomDecoder { expected_encoding: Utf8 }` with fallback on failure | Phase 1 |

---

### 3. CLI WRAPPER & VASE CONSTRUCTION
**Primary Sites:** `crates/honk/src/bin/honk.rs` wrapper compilation and vase building

#### 3A. Wrapper Compilation and Gate Creation

| Location | Line | Code | What Crosses | Decision | Phase |
|----------|------|------|--------------|----------|-------|
| `crates/honk/src/bin/honk.rs` | 1206-1218 | `initialize_native_wrappers()` | Constructs wrapper gates; calls `Ut::mint()` indirectly via `compile_wrapper_gate()` | **Named Adapter**: `WrapperCompilationBoundary { gate_name, compiled_formula }` with audit | Phase 1 |
| `crates/honk/src/bin/honk.rs` | 1211 | `T(&mut *self.ut.slab, &[self.prelude_vase.ty, prelude_formula])` | Assembles prelude gun (type + formula) | **Native Analogue**: `PreludeGun { prelude_ty, prelude_formula }` struct with direct field access | Phase 1 |
| `crates/honk/src/bin/honk.rs` | 1226-1284 | `initialize_exact_wrappers()` | Dynamic wrapper gates compiled from Hoon; calls `compile_exact_wrapper_gates()` then `extract_exact_wrapper_batteries()` | **Named Adapter**: `ExactWrapperBoundary { gate_source, compiled_batteries: ExactWrapperBatteries }` | Phase 1 |
| `crates/honk/src/bin/honk.rs` | 1287-1297 | `compile_wrapper_gate_maybe_exact()` | Delegates to either `compile_wrapper_gate()` or `compile_wrapper_gate_exact()` | **Conditional Adapter**: Route to native analogue based on `exact_subject_trap` presence | Phase 2 |

#### 3B. Vase & Trap Construction

| Location | Line | Code | What Crosses | Decision | Phase |
|----------|------|------|--------------|----------|-------|
| `crates/honk/src/bin/honk.rs` | 2782-2784 | `fn vase_native(slab, ty: Noun, value: Noun) -> Noun` | Direct `T(slab, &[ty, value])` cell construction | **Native Analogue**: `VaseNoun { ty: Noun, value: Noun }` with direct constructor | Phase 1 |
| `crates/honk/src/bin/honk.rs` | 2786-2796 | `fn vase_native_with_spaces()` | Copies ty/value into target slab before vase construction via `slab.copy_into()` | **Named Adapter**: `CrossSlabVaseCopy { ty_source_space, value_source_space, target_slab }` | Phase 2 |
| `crates/honk/src/bin/honk.rs` | 2798-2804 | `fn trap_battery(trap: Noun, space) -> Result<Noun>` | Extracts head (battery) from trap cell; validation that it's a cell | **Named Adapter**: `TrapBatteryExtraction { cell_assertion }` with error on non-cell | Phase 1 |
| `crates/honk/src/bin/honk.rs` | 1499-1568 | `ExactWrapperBatteries` construction in wrappers | Multiple calls to `slam_wrapper_gate()` for each wrapper battery | **Named Adapter**: `SlamResult { name: &str, formula: Noun }` per gate | Phase 2 |

#### 3C. Wrapper Subject Type Construction

| Location | Line | Code | What Crosses | Decision | Phase |
|----------|------|------|--------------|----------|-------|
| `crates/honk/src/bin/honk.rs` | 925-928 | `ty_cell_local()` in wrapper_subject_ty construction | Constructs cell type `[empty-type prelude-type]` | **Declarative**: `CellTypeLayout { head_type: Noun, tail_type: Noun }` as static struct | Phase 0 |
| `crates/honk/src/bin/honk.rs` | 1239-1241 | `ty_atom_local()` for empty trap vase | Creates `%atom` type descriptor | **Declarative**: `AtomTypeDescriptor { aura: "n", constraint: Option<Noun> }` | Phase 0 |

---

### 4. STANDARD OUTPUT & FINAL FORMULA
**Primary Sites:** Standard output wrapper execution and final formula jamming

| Location | Line | Code | What Crosses | Decision | Phase |
|----------|------|------|--------------|----------|-------|
| `crates/honk/src/lib.rs` | 83-86 | `pub fn jam(&mut self)` | Final formula jamming: sets root and calls `slab.jam()` | **Excluded**: Final output path (not in boundary matrix per scope) | N/A |
| `crates/honk/src/lib.rs` | 93-99 | `pub fn jam_dynock(&mut self)` | Wraps formula in dynock trap, constructs `[noun-type (trap formula)]` | **Wrapper Adapter**: `DynockWrapper { type_mode: Minimal, formula }` | Phase 1 |
| `crates/honk/src/lib.rs` | 104-109 | `pub fn jam_dynock_typed(&mut self)` | Wraps formula in dynock trap with full inferred type `[inferred-type (trap formula)]` | **Wrapper Adapter**: `DynockWrapper { type_mode: Full, formula, inferred_type }` | Phase 1 |
| `crates/honk/src/lib.rs` | 115-118 | `fn wrap_formula_as_dynock_trap()` | Constructs trap from formula: `[[D(1) formula] D(0)]` | **Declarative**: `TrapPayload { battery: [1 formula], sample: 0 }` static structure | Phase 0 |
| `crates/honk/src/bin/honk.rs` | 2347-2350 | `standard_output_trap()` | Calls wrapped standard-output gate with type/formula/value | **Named Adapter**: `StandardOutputExecution { gate_input: (ty, formula) }` | Phase 2 |
| `crates/honk/src/bin/honk.rs` | 2732 | `fn jam_dynock_output_native()` | Alternative dynock construction in native context (ut: Ut) | **Wrapper Adapter**: `DynockNativeWrapper { ut_context, typed: bool }` | Phase 2 |

---

### 5. MACK BOUNDARY
**Primary Sites:** Musk araw (araw = arm-raw) constant-folding interpreter

| Location | Line | Code | What Crosses | Decision | Phase |
|----------|------|------|--------------|----------|-------|
| `crates/honk/src/native/ut/mod.rs` | 5486-5767 | `fn musk_araw_uncached()` | Interprets nock formula tree using araw opcode dispatch (0-11) | **Named Adapter**: `ArawInterpreter { opcode: u8, bus: Noun, fol: Noun }` per instruction | Phase 2 |
| `crates/honk/src/native/ut/mod.rs` | 5534 | `case 1 => semi_full_complete(tail)` | Literal noun constant; constructs semi-complete marker | **Native Analogue**: `Literal { noun: Noun, semi_tag: SemiTag::Full }` | Phase 1 |
| `crates/honk/src/native/ut/mod.rs` | 5527 | `case 0 => semi_fragment(axis, bus)` | Slot access via axis; returns fragment semi | **Named Adapter**: `SlotAccess { axis, bus }` with fragment result | Phase 1 |
| `crates/honk/src/native/ut/mod.rs` | 5678-5693 | `case 9 mack_constant_core()` | Calls mack interpreter for core arm access; consumes **core Noun** and axis | **Named Adapter**: `MackConstantCore { core: Noun, axis: u64 }` | Phase 2 |
| `crates/honk/src/native/ut/mod.rs` | 5803-5835 | `fn musk_mack_constant_core()` | Cache lookup + musk_interpret_mack for arm evaluation | **Named Adapter**: `MackCacheKey { raw_addr: u64, axis: u64 }` + `MackInterpret { core, axis }` | Phase 2 |
| `crates/honk/src/native/ut/mod.rs` | 5837-5846 | `fn musk_interpret_mack()` | Unsafe pointer dereference to eval context; constructs nock formula for arm slot | **Named Adapter**: `MackEvalContext { context_ptr, core, axis }` with unsafe boundary | Phase 3 |
| `crates/honk/src/native/ut/mod.rs` | 5848-5894 | `unsafe fn musk_interpret_mack_in_context()` | Interprets `[9 axis [0 1]]` formula in eval context; calls `interpret()` | **Excluded**: This is nockvm interpreter boundary, not honk noun construction | N/A |

---

### 6. MUSK CACHE BOUNDARY
**Primary Sites:** Musk mack cache with raw address + axis keys

| Location | Line | Code | What Crosses | Decision | Phase |
|----------|------|------|--------------|----------|-------|
| `crates/honk/src/native/ut/mod.rs` | 5808 | `let raw_key = (unsafe { core.as_raw() }, axis);` | Raw pointer extraction from Noun | **Named Adapter**: `RawNounKey { core_ptr: u64, axis: u64 }` with pointer validity witness | Phase 3 |
| `crates/honk/src/native/ut/mod.rs` | 5813 | `let key = (self.noun_mug_cached(core), axis);` | Mug-based key for cache bucket lookup | **Named Adapter**: `MugKey { core_mug: u32, axis: u64 }` with mug stability witness | Phase 2 |
| `crates/honk/src/native/ut/mod.rs` | 5833 | `bucket.push_back(MuskMackCacheEntry { core, result })` | Stores core Noun + result in bucket; core becomes cache entry key | **Named Adapter**: `CacheEntry { core_noun: Noun, result: Option<Noun> }` | Phase 2 |

---

### 7. COLD STATE & MUSK CONTEXT
**Primary Sites:** Cold-state loading for jet registration; musk context initialization

| Location | Line | Code | What Crosses | Decision | Phase |
|----------|------|------|--------------|----------|-------|
| `crates/honk/src/bin/honk.rs` | 868-879 | `load_cold_state()` + `ut.load_musk_cold_state()` | Cues cold-state JAM, passes to Cold struct and to ut's musk cold | **Named Adapter**: `ColdStateLoad { jam_bytes, label: &str }` with decoding | Phase 1 |
| `crates/honk/src/bin/honk.rs` | 3126-3143 | `fn load_cold_state()` | Calls `Cold::from_noun()` to decode jet registration structure | **Named Adapter**: `ColdStateDecode { battery_to_paths, root_to_paths, path_to_batteries }` | Phase 2 |
| `crates/honk/src/native/ut/mod.rs` | ???? | `ut.load_musk_cold_state()` | Loads cold state into musk's jet cache (not in provided excerpt; needs search) | **Named Adapter**: `MuskColdState { cache_data }` | Phase 2 |

---

### 8. DATA IMPORT VASE CONSTRUCTION
**Primary Sites:** File data vase construction for non-Hoon `/*` imports

| Location | Line | Code | What Crosses | Decision | Phase |
|----------|------|------|--------------|----------|-------|
| `crates/honk/src/bin/honk.rs` | 1912-1945 | `fn data_vase()` | Constructs vase for data file import; reads file, gets `octs` type | **Named Adapter**: `DataImportVase { file_path, file_content: Noun, octs_type: Noun }` | Phase 2 |
| `crates/honk/src/bin/honk.rs` | 1933 | `hoonc_data_octs_vase_trap()` | Constructs specific vase for hoonc octs type | **Declarative**: `OctsVase { size_atom: Noun, data_atom: Noun }` layout | Phase 1 |
| `crates/honk/src/bin/honk.rs` | 2740-2758 | `fn hoonc_data_octs_vase_trap()` | Constructs trap battery from hoonc octs formula | **Named Adapter**: `DataVaseTrap { ty: Noun, value: Noun, battery: Noun }` | Phase 2 |
| `crates/honk/src/bin/honk.rs` | 2760-2780 | `fn hoonc_data_octs_trap_battery()` | Constructs battery from axis formula | **Declarative**: `OctsTrapBattery { axis_formula: [9 3 [0 1]] }` static structure | Phase 0 |

---

### 9. LIBRARY API: DYNOCK & TYPED OUTPUT
**Primary Sites:** `crates/honk/src/lib.rs` public API for compiled output

| Location | Line | Code | What Crosses | Decision | Phase |
|----------|------|------|--------------|----------|-------|
| `crates/honk/src/lib.rs` | 38-43 | `fn compile_expr()` returns `Compiled` | Wraps slab + formula + ty + arm_map in opaque struct | **Adapter**: `CompiledOutput { slab: NounSlab, formula: Noun, ty: TypeNoun, arm_map: ArmMap }` (opaque) | Phase 1 |
| `crates/honk/src/lib.rs` | 59 | `pub formula: Noun` | Direct Noun field in CompiledNative (private) | **Excluded**: Private field; not exposed in public API | N/A |
| `crates/honk/src/lib.rs` | 93-99 | `jam_dynock()` wraps formula | See STANDARD OUTPUT section above | **Wrapper Adapter**: `DynockWrapper` | Phase 1 |
| `crates/honk/src/lib.rs` | 104-109 | `jam_dynock_typed()` wraps formula + ty | See STANDARD OUTPUT section above | **Wrapper Adapter**: `DynockTyped` | Phase 1 |

---

### 10. NATIVE EXACT WRAPPER BATTERY CONSTRUCTION
**Primary Sites:** Native constructor for exact wrapper batteries (hoonc parity artifacts)

| Location | Line | Code | What Crosses | Decision | Phase |
|----------|------|------|--------------|----------|-------|
| `crates/honk/src/bin/honk.rs` | 2809-2849 | `fn construct_exact_wrapper_batteries()` | Constructs all 15 exact wrapper formulae without Nock interpretation | **Declarative**: `ExactWrapperFormulae { constant_vase, data_vase, dir_hash_vase, ... }` static structure | Phase 0 |
| `crates/honk/src/bin/honk.rs` | 2812-2826 | Individual battery assignments | Each wrapper is a formula constructed via `axis_formula()` or `hoonc_spot_formula()` | **Declarative**: Per-wrapper formula grammar (axis access, spot hints, etc.) | Phase 0 |
| `crates/honk/src/bin/honk.rs` | 2851-2868 | `fn hoonc_spot_formula()` | Constructs hint-wrapped formula with source location metadata | **Declarative**: `SpotFormula { path_atoms, start_line, start_col, end_line, end_col, inner_formula }` | Phase 0 |
| `crates/honk/src/bin/honk.rs` | 2870-2875 | `fn hoonc_memo_formula()` | Wraps formula in %memo hint | **Declarative**: `MemoFormula { inner_formula }` static wrapper | Phase 0 |
| `crates/honk/src/bin/honk.rs` | 2877-2900 | `fn exact_value_trap_battery()` | Constructs battery for value trap with standard vs arbitrary modes | **Declarative**: `ValueTrapBattery { line, mode }` layout | Phase 0 |

---

### 11. SUBJECT TYPE OVERRIDE & PRELUDE SETUP
**Primary Sites:** Subject type cuing and prelude type initialization

| Location | Line | Code | What Crosses | Decision | Phase |
|----------|------|------|--------------|----------|-------|
| `crates/honk/src/bin/honk.rs` | 903-918 | `cue_subject_type_to_slab()` | Cues subject type JAM, extracts root, copies into slab | **Named Adapter**: `SubjectTypeCue { jam_bytes, root_noun, target_slab }` with copy witness | Phase 1 |
| `crates/honk/src/bin/honk.rs` | 920-922 | `prelude_type_from_subject_type()` | Transforms subject type override into prelude.ty field | **Named Adapter**: `PreludeTypeExtraction { subject_type_noun }` | Phase 1 |
| `crates/honk/src/bin/honk.rs` | 937-946 | `canonical_data_octs_ty` cuing | Cues hoonc octs type from embedded JAM for canonical builds | **Named Adapter**: `CanonicalOctsTypeCue { jam_bytes }` | Phase 1 |

---

### 12. PRELUDE FORMULA CUING vs MINTING
**Primary Sites:** Choice between embedded hoonc formula vs native mint

| Location | Line | Code | What Crosses | Decision | Phase |
|----------|------|------|--------------|----------|-------|
| `crates/honk/src/bin/honk.rs` | 899-908 | `use_embedded` branch | Chooses between `cue_honc_formula_to_slab()` and `mint_honc_formula_with_ut()` | **Conditional Adapter**: `PreludeFormulaBoundary { mode: Embedded | Native }` with parity flag | Phase 1 |
| `crates/honk/src/bin/honk.rs` | 900-903 | `cue_honc_formula_to_slab()` | Cues embedded formula JAM into slab | **Named Adapter**: `EmbeddedFormulaCue { jam_bytes, label: "canonical honc formula" }` | Phase 1 |
| `crates/honk/src/bin/honk.rs` | 905-907 | `mint_honc_formula_with_ut()` | Natively mints prelude formula (HONK_NATIVE_PARITY audit path) | **Named Adapter**: `NativeMintFormula { ut_context, prelude_expr }` | Phase 2 |
| `crates/honk/src/bin/honk.rs` | 853-856 | `native_parity_enabled()` | Checks HONK_NATIVE_PARITY env var | **Conditional**: Static feature gate (not a noun boundary; control flow) | Phase 0 |

---

### 13. IMPORT ASSET VASE CONSTRUCTION
**Primary Sites:** Dependency imports (library modules) as vases

| Location | Line | Line | Code | What Crosses | Decision | Phase |
|----------|------|---------|------|--------------|----------|-------|
| `crates/honk/src/bin/honk.rs` | 1673-1790 | `fn compile_entry()` | Processes imports and constructs their vases | **Named Adapter**: `ImportVase { import_kind: Source | Data, path: Path }` per import | Phase 2 |
| `crates/honk/src/bin/honk.rs` | 1784 | Import handling dispatch | Data imports → `data_vase()`, source imports via library | **Conditional Adapter**: Route by `NativeImportKind` | Phase 1 |

---

### 14. PRELUDE VASE & GUN CONSTRUCTION
**Primary Sites:** Prelude type/formula assembly into operational structures

| Location | Line | Code | What Crosses | Decision | Phase |
|----------|------|------|--------------|----------|-------|
| `crates/honk/src/bin/honk.rs` | 1084-1090 | `struct NativeVase { ty, eval_value, trap }` | Bundled type + optional value + trap | **Opaque Wrapper**: `NativeVase` struct (locally defined, not boundary crossing per se) | Inline |
| `crates/honk/src/bin/honk.rs` | 1211 | `T(&mut *self.ut.slab, &[self.prelude_vase.ty, prelude_formula])` | Constructs prelude gun cell from prelude type + formula | **Native Analogue**: `PreludeGun { ty: Noun, formula: Noun }` direct tuple | Phase 1 |
| `crates/honk/src/bin/honk.rs` | 1212 | `T(&mut *self.ut.slab, &[prelude_gun, self.empty_trap_vase])` | Constructs prelude sample (gun + empty trap) | **Native Analogue**: `PreludeSample { gun: Noun, empty_trap: Noun }` | Phase 1 |

---

## Summary of Adapter Strategies

### Declarative (Phase 0)
For static, grammar-derived layouts:
- Trap payload structure: `[[1 formula] 0]`
- Coil grammar: `[garb [context tomes]]`
- Arm dab binary map structure
- Exact wrapper formulae (axis access, hints, memos)
- OctsVase layout: `[size_atom data_atom]`

**Action:** Document coil/dab/vase/trap/battery grammars in `native_ir::grammar` module.

### Native Analogue (Phase 1)
For transparent type mappings:
- `TypeNoun::new(Noun)` → `NativeInferredType`
- `VaseNoun { ty, value }` → Direct field struct
- `PreludeGun { ty, formula }` → Direct tuple alias
- `SemiTag`, `SemiComplete`, `SemiFragment` → Opaque wrappers

**Action:** Create `native_ir::types` with field-for-field equivalence. No witness function needed; direct reinterpret allowed.

### Named Adapter (Phase 1-2)
For complex transformations:
- `MintBoundary { sut, gol, output }` → Logged transition with input+output witness
- `ArmMapTraversal` → Cell-structure descent validator
- `MackConstantCore { core, axis }` → Cache key + interpreter invocation
- `StandardOutputExecution` → Gate slam + product extraction
- `ColdStateLoad { jam, label }` → Cue + decode + context injection

**Action:** Create `native_ir::adapters` module with named witnesses. Each adapter:
1. Asserts input shape (cell, atom, tag match, etc.)
2. Documents traversal semantics
3. Caches result for subsequent phases

### Excluded (N/A)
- Final output jamming (`slab.jam()`)
- Nockvm interpreter boundary (`interpret()` in musk_interpret_mack_in_context)
- Private fields in library API

---

## Per-Boundary Audit Notes

### Boundary 1: Bootstrap Types (ty_noun, ty_void)
- **Current:** `term_to_noun(slab, "noun")` creates atom from utf-8
- **Phase 1 Decision:** 
  - Witness that bootstrap types are known constants
  - Pre-register bootstrap mug checksums
  - Disable substitution; always use native bootstrap
- **Reason:** Prelude type identity depends on exact bootstrap type references

### Boundary 2: ARM_MAP Coil/Tome Traversal
- **Current:** Pattern matches on tag atoms ("core", "face", "hint", "hold")
- **Phase 1 Decision:**
  - Hardcode coil structure grammar (no runtime flexibility needed)
  - Pre-validate that all tag atoms match known set
  - Create `TypeGrammar` enum: `Core | Face | Hint | Hold | Other`
- **Reason:** Type shape is compiler-internal; no user-facing flexibility required

### Boundary 3: Mack Constant Core
- **Current:** Uses raw pointer (`unsafe { core.as_raw() }`) + axis as cache key
- **Phase 3 Decision:**
  - Require pointer liveness witness from `NounSlab` region stack
  - Extend `MuskMackCacheEntry` with generation tag
  - Validate generation before each cache hit
- **Reason:** Cross-slab references require provenance tracking for safety

### Boundary 4: Standard Output Gate
- **Current:** `slam_wrapper_gate(gate, sample)` invokes nock interpreter
- **Phase 2 Decision:**
  - Keep interpreter invocation (no adaptation needed here)
  - Pre-validate gate shape: `[battery sample]` → `[battery new_sample]`
  - Document expected gate signature (sample type, output type)
- **Reason:** Gate execution is trusted; only document input/output contract

### Boundary 5: Data Import Vase
- **Current:** File bytes wrapped in hoonc octs type
- **Phase 1 Decision:**
  - Pre-cache canonical octs type (from embedded JAM)
  - Disallow custom octs type overrides (use embedded or fail)
  - Create `DataImportOctsType` enum: `Canonical | Override(Noun)`
- **Reason:** Data vase type identity must match hoonc exactly for parity

### Boundary 6: Dynock Wrapper
- **Current:** Wraps formula as `[[1 formula] 0]` trap, prefixes with type
- **Phase 1 Decision:**
  - Create `DynockFormat` enum: `Minimal | Typed`
  - Validate that wrapped noun is cell with correct head (type) + tail (trap)
  - Pre-validate trap structure before final jam
- **Reason:** Dynock is final serialization format; incorrect wrapping breaks consumers

### Boundary 7: Prelude Formula Cuing vs Minting
- **Current:** Conditional choice based on HONK_NATIVE_PARITY flag + canonical detection
- **Phase 2 Decision:**
  - Create `PreludeFormulaBoundary { mode: Embedded | Native }` witness
  - Log formula mug on both paths; fail if mug diverges
  - Document expected formula mug for hoon-138 prelude
- **Reason:** Formula identity is critical for parity; mug divergence indicates compiler bug

### Boundary 8: Cross-Slab Noun Copy
- **Current:** `slab.copy_into(noun, source_space)` moves noun between allocators
- **Phase 2 Decision:**
  - Require source_space and target slab to be compatible
  - Log source generation + target generation
  - Fail explicitly if source_space is dangling
- **Reason:** Cross-slab references can outlive source; generation tracking required

### Boundary 9: Musk Cache Entry Storage
- **Current:** Stores `MuskMackCacheEntry { core: Noun, result }`
- **Phase 2 Decision:**
  - Require core Noun to be "stable" (not dependent on musk frame)
  - Add generation tag to cache entry
  - Periodically evict stale entries (gen mismatch)
- **Reason:** Core Noun validity depends on slab lifetime; generation ensures safety

### Boundary 10: Cold State Loading
- **Current:** `Cold::from_noun()` decodes jet registration structure
- **Phase 2 Decision:**
  - Pre-validate cold-state noun structure (must be list of [battery_path jet_name])
  - Log jet registration count
  - Fail if any battery path is invalid
- **Reason:** Cold state is critical for correctness; structural validation prevents corruption

---

## Phase-1 Implementation Roadmap

1. **Grammar Module** (`native_ir::grammar`):
   - `CoilGrammar`, `DabGrammar`, `TomeGrammar`, `VaseGrammar`, `TrapGrammar`
   - Static constants for each structure's expected shape
   - Helper functions for validation (e.g., `validate_coil(noun, space)`)

2. **Types Module** (`native_ir::types`):
   - `NativeInferredType`, `NativeVase`, `NativeTrap`, `NativeGun`
   - Direct 1-to-1 mapping with Noun equivalents
   - Zero-cost transparent wrappers

3. **Adapters Module** (`native_ir::adapters`):
   - `MintBoundary`, `ArmMapTraversal`, `VaseCopy`, `TrapConstruction`, etc.
   - Each adapter has `assert_preconditions()` + `execute()` + `validate_postconditions()`
   - All adapters are logged (info level)

4. **Cache Module** (`native_ir::cache`):
   - `MaskCacheKey`, `MuckCacheEntry`, generation-tagged storage
   - Integration with `NounSlab::region_stack()` for liveness

5. **Witness Module** (`native_ir::witness`):
   - Mug checksums for bootstrap types
   - Prelude formula mug expectation
   - Cold-state structure validation

6. **Integration Points**:
   - Replace direct `Noun` constructions with named adapters
   - Add logging at each boundary
   - Create strict-mode flag to enable validation

---

## Open Questions

1. **Prelude Formula Mug Stability**: Does native mint of hoon-138 prelude produce exact same mug as hoonc's? If not, what's the divergence?
   
2. **Coil/Tome Structure Stability**: Can type coil structure change between hoon versions? Should we version the grammar?

3. **Mack Cache Generation Tagging**: How frequently should cache entries be invalidated? Per-slab? Per-region?

4. **Cold State Updates During Compilation**: Does musk cold state ever change after initial load? Should cache entries reference cold state version?

5. **Data Import Octs Type Overrides**: Should native compiler allow custom octs types, or enforce canonical hoonc type?

6. **Mug Collision Handling**: Current arm_map uses mug as cache key. What's the collision rate in practice?

7. **Standard Output Gate Optimization**: Can we pre-compute standard output instead of interpreting the gate every time?

---

## Appendix: Noun Construction Sites (Inventory)

**Total boundary crossings identified:** 72 (Mint: 4, ArmMap: 6, CLI Wrapper: 16, Standard Output: 6, Mack: 8, Musk Cache: 3, Cold State: 3, Data Import: 4, Library: 4, Exact Wrappers: 14)

**Declarative sites (Phase 0):** 14  
**Native Analogue sites (Phase 1):** 22  
**Named Adapter sites (Phase 1-2):** 36

---

*Matrix compiled 2026-06-16 from honk HEAD; prepared for Phase-0 design review.*
