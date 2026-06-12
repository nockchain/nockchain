# Phase 0: Parity-Matrix & Hoonc-Oracle-Fixture Design

**Status**: Design for implementation (plan §2.3, §5.2, RT-01)  
**Context**: TODOS #11, #15 deferral; honk native-compiler post-H3 correctness baseline

## Overview

This document specifies:
1. **Parity matrices** for the eight key type-resolution arms (find/fend/fund/fond/repo/tack/toss/cnts/mine) — the input shapes, wing forms, and edge cases that distinguish correct behavior from hoonc
2. **Hoonc-oracle-fixture mechanism** — how to compile an expression against hoonc to a golden jam, execute honk's equivalent, and enforce byte-identity via a test harness
3. **Seed corpus categories** — the structured expression families that comprehensively exercise each arm's decision boundaries

Together, these define Phase 0 (planning and design artifacts) for per-expression parity validation, separate from the whole-kernel byte-parity harness (H0 / `just honk-parity`).

---

## 1. Canonical Arms In Scope

The eight arms are the strict type-resolution / wing-path-resolution core of the native compiler's type-checking (`++ut` in hoon-138, implemented natively in honk's `crates/honk/src/native/ut/{find,repo}.rs`):

| Arm | File | Lines | Status | Purpose |
|-----|------|-------|--------|---------|
| `++find` | find.rs | 38-49 | partial | Resolve a wing path to a port (type + formula or arm spec) |
| `++fend` | find.rs | 55-66 | partial | Find and extract the axis (linear path) from a port |
| `++fund` | find.rs | 68-77 | partial | Resolve a wing or expression (implicit reek check) |
| `++fond` | find.rs | 79-140 | partial | Recursive wing-path walker (the core recursion) |
| `++repo` | repo.rs | 176-205 | partial | Unwrap a type's outermost structure (face/hint/core/hold/noun) |
| `++tack` | test.rs | 1155-1198 (canonical) | not checked | Allocate a new name into subject (edit/annotation) |
| `++toss` | test.rs | 1156-1169 (canonical) | not checked | Merge arm lists with axis validation |
| `++cnts` | not yet in scope | — | not checked | Count/compact duplicate arm references |
| `++mine` | not yet in scope | — | not checked | Mint expression variants (recursion, gates, loops) |

All eight currently carry `status=partial` or are untested in the strict harness. The parity-matrix design makes each executable by specifying: (a) what input families the matrix exercises, (b) for each family, the oracle (hoonc compiled result), and (c) how honk's result is compared.

---

## 2. Parity-Matrix Design (Per-Arm)

### 2.1 Structure of a Parity Matrix

Each arm gets a **parity matrix** — a table of test cases organized by input shape:

```
Matrix := [
  | Category | Input Shape | Wing Form | Edge Case | Oracle Behavior | Honk Path | Assertion |
  | —— | —— | —— | —— | —— | —— | —— |
  | struct | leaf/face/hold/fork | term/axis/parent | base | golden jam | [compare] | byte-eq |
]
```

**Fields**:
- **Category**: Semantic class (structure, recursion, abstraction, etc.)
- **Input Shape**: The subject type structure (atom, cell, face, core, fork, hold)
- **Wing Form**: How the path is expressed (term name, axis number, parent-skip, empty wing)
- **Edge Case**: Boundary condition (nil type, void, loops, ambiguity)
- **Oracle Behavior**: What hoonc's `++find` (etc.) produces when compiled with the input
- **Honk Path**: Which code path in honk.rs (e.g., `fond_name`, `fond` base case)
- **Assertion**: Comparison rule (byte-match jam, type-match, axis-match, error category)

### 2.2 Example: `++find` Parity Matrix

**++find(sut, way, wing) -> Port**

Input: subject type, arm-resolution way (Read/Write/Peek), wing path  
Output: Port (either Palo with vein/opal, or Synthetic with type/formula, or error)

#### Categories:

1. **Leaf Paths** (single-limb wings)
   | Input | Wing | Oracle | Honk Code | Check |
   |-------|------|--------|-----------|-------|
   | `@ud` | `[1]` (empty) | Palo vein=[] opal=Leg(@ud) | find, base case | vein=[]; opal type matches |
   | `[@ @]` | `[2]` (head axis) | Palo vein=[Some(2)] opal=Leg(@) | find->fond->Axis | vein has 2, opal type is @ |
   | face `foo=@` | `[foo]` (term) | Palo vein=[None,Some(1)] opal=Leg(@) | find->fond->fond_name | vein has face marker + axis |
   | face `foo=@` | `[4]` (skip 0) | Palo opal=Leg(@) | find->fond->Axis (through face) | axis reaches inner |

2. **Recursive Paths** (multi-limb wings)
   | Input | Wing | Oracle | Honk Code | Check |
   |-------|------|--------|-----------|-------|
   | `[@ @]` | `[2; 2]` | Palo vein=[Some(2), Some(2)] | fond recursion | both axes in vein |
   | `[foo=[@ @] @]` | `[foo; 2]` | Palo vein=[None, Some(1), Some(2)] | fond recurse through face | face marker + axes |
   | face `tree=(list @)` | `[tree; tail]` | Palo vein=... opal=Leg(list) | fond through face then term | list type materialized |

3. **Arm Paths** (core arms)
   | Input | Wing | Oracle | Honk Code | Check |
   |-------|------|--------|-----------|-------|
   | core with `++foo` arm | `[foo]` | Palo opal=Arm{axis, arms=[(core, foot)]} | fond->core branch | arm axis correct; foot matches |
   | nested core, find in parent | `[foo]` (skip>0) | Pony::Unmatched(skip') | fond->core, search fails | skip count decremented |
   | core with `=*` alias | `[alias]` (tune bridge) | Port::Synthetic{formula} | fond->face tool bridges | bridge formula emitted |

4. **Fork Resolution**
   | Input | Wing | Oracle | Honk Code | Check |
   |-------|------|--------|-----------|-------|
   | fork `[@ @ud]` | `[2]` | fork each branch; merge results | fond->fork via twin | both forks resolved; twin matches |
   | fork with void branch | `[2]` | void filtered out | fond->fork + void handling | void handled correctly |
   | fork with identical branches | `[term]` | single result deduplicated | fond->fork + twin dedup | no duplicate arms |

5. **Hold (Recursive Type) Paths**
   | Input | Wing | Oracle | Honk Code | Check |
   |-------|------|--------|-----------|-------|
   | hold `%(inner gen)` | `[term]` (in inner) | fond resolves through hold via repo | fond->hold->seen check | loop guard works; axis correct |
   | hold with same hold visited | recurrent wing | Void (loop detected) | fond->hold->seen loop guard | loop guard triggers |
   | hold with different inner | `[term]` | fresh repo on inner | fond->hold with different context | repo returns distinct type |

6. **Error / Void Paths**
   | Input | Wing | Oracle | Honk Code | Check |
   |-------|------|--------|-----------|-------|
   | void | any | Void | fond base void case | Void returned |
   | atom (no substructure) | `[2]` (non-1) | Void/error | fond atom no-match | axis out of bounds |
   | face on void | `[term]` | void propagated | fond->face->void | void propagates |

---

### 2.3 Template for Other Arms

**++fond (recursive wing walker)**:
- Base case: empty wing → return subject wrapped in Palo
- Axis case: split wing, find current axis in subject, prepend to vein, recurse on tail
- Term case: find term in subject (via name resolution), handle face/core/fork
- Hold case: check seen guard, recurse into inner via repo
- Fork case: recurse each branch, merge via twin

Matrix categories:
1. Base/empty wings
2. Pure axis chains
3. Term resolution (face, core arm names, parent skip)
4. Face tool (tune bridges, aliases)
5. Core context (polymorphism, vair, sam/con)
6. Hold recursion (loop guards, via repo)
7. Fork merging (via twin, void handling)
8. Error propagation (void, type mismatches)

**++repo (type unwrapping)**:
- Face: unwrap to inner
- Hint: unwrap to inner
- Core: extract payload, create [noun payload] cell type
- Hold: call rest on a singleton leg (repo_hold)
- Noun/Atom/Cell/etc.: construct canonical result
- Non-canonical inputs: repo-fltt error

Matrix categories:
1. Face unwrapping (term vs non-term faces)
2. Hint unwrapping (different payloads)
3. Core extraction (with/without explicit payload faces)
4. Hold via rest (singleton leg paths)
5. Base types (noun, atom, cell)
6. Void handling
7. Error cases (non-canonical input)
8. Cached ty_hold_cached behavior (structural equality)

**++fend (find + extract axis)**:
- Calls find; if Port::Palo with Leg, extract axis from vein via tend
- If Port::Palo with Arm or Synthetic, error "fend-fragment"

Matrix categories:
1. Simple leg paths (return axis)
2. Recursive paths (axis composition)
3. Arm paths (error case)
4. Synthetic paths (error case)
5. Error propagation

**++fund (resolve wing or expression)**:
- Try reek (extract implicit wing); if found, call find
- Else mint the expression against noun type

Matrix categories:
1. Pure wing expressions (reek succeeds → find)
2. Synthetic expressions (reek fails → mint)
3. Hybrid (term reference, but not a pure wing)
4. Error cases (invalid reek, mint failure)

**++tack (allocate name into subject)**:
- Add a face to the subject type at the given wing
- Return new type and edit axis

Matrix categories:
1. Empty wing (root face)
2. Existing face (replace)
3. Nested faces (compound path)
4. On core (payload vs context)
5. On hold (via repo)
6. Error/void cases

**++toss (merge arm lists)**:
- Check all arms have the same edit axis
- Return merged arm list or error "mate" if axes differ

Matrix categories:
1. Single arm
2. Duplicate arms (dedup)
3. Matching axes (merge)
4. Mismatched axes (error)
5. Empty list (error "need")

---

## 3. Hoonc-Oracle-Fixture Mechanism

### 3.1 Overview

A **hoonc-oracle fixture** is:
- A **seed expression** in Hoon source (e.g., a wing reference in a specific subject context)
- A **compiled binary** (jam) produced by hoonc (the oracle)
- A **honk compilation** of the same expression, producing a formula
- An **assertion** comparing them byte-for-byte

The fixture captures: "when hoonc compiles this expression, it produces this jam; honk must produce an identical formula."

### 3.2 Mechanism: Compile-and-Compare

```rust
// Pseudocode test structure:

#[test]
fn find_oracle_leaf_axis_path() {
    // (1) ORACLE SETUP: Create a source expression and compile with hoonc
    let src = r#"
      =| sut=[@ @]
      [2]   /@ the head of a cell
    "#;
    let oracle_jam = compile_with_hoonc(src); // -> Vec<u8>
    
    // (2) HONK COMPILATION: Parse and mint in honk
    let expr = parse_expr(src);
    let mut slab = NounSlab::new();
    let sut_type = honk::native::ut::cell_type(
        &mut slab, 
        honk::native::ut::ty_atom(&mut slab, "@", None),  // head
        honk::native::ut::ty_atom(&mut slab, "@", None)   // tail
    )?;
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    let (_ty, honk_formula) = ut.mint(sut_type, gol, &expr)?;
    
    // (3) ORACLE COMPARISON: Serialize and byte-compare
    let honk_jam = jam_formula(&slab, honk_formula)?;
    assert_eq!(honk_jam, oracle_jam, "find axis path must match hoonc formula");
}
```

### 3.3 Test Harness Structure

**File**: `crates/honk/tests/compiler_oracle.rs` (new)

**Sections**:
```rust
// === ORACLE FIXTURES ===
// Compile expressions via hoonc, cache jam binaries in test-assets/

mod oracle_harness {
    // Helper to invoke hoonc subprocess or use cached golden jams
    fn compile_expression_oracle(src: &str) -> Vec<u8> { ... }
    
    // Setup: Create a test subject type via Ut constructors
    fn test_subject_cell() -> Noun { ... }
    fn test_subject_face_tree() -> Noun { ... }
    fn test_subject_core_with_arms() -> Noun { ... }
    // ... per matrix category
    
    // Comparison: Byte-exact jam match
    fn assert_formula_matches_oracle(honk_jam: Vec<u8>, oracle_jam: Vec<u8>) {
        let honk_hash = blake3::hash(&honk_jam);
        let oracle_hash = blake3::hash(&oracle_jam);
        assert_eq!(honk_hash, oracle_hash, 
            "formula jam must match hoonc oracle byte-for-byte");
    }
}

// === FIND MATRIX TESTS ===
#[test] fn find_oracle_empty_wing() { ... }
#[test] fn find_oracle_axis_leaf() { ... }
#[test] fn find_oracle_term_leaf() { ... }
#[test] fn find_oracle_recursive_path() { ... }
#[test] fn find_oracle_arm_resolution() { ... }
#[test] fn find_oracle_hold_recursion() { ... }
#[test] fn find_oracle_fork_resolution() { ... }
#[test] fn find_oracle_face_tool_bridges() { ... }
#[test] fn find_oracle_void_propagation() { ... }
#[test] fn find_oracle_error_non_canonical() { ... }

// === FOND, FEND, FUND, REPO, TACK, TOSS TESTS ===
// Similar structure for each arm
#[test] fn fond_oracle_... { ... }
#[test] fn repo_oracle_... { ... }
// etc.
```

### 3.4 Oracle Compilation Strategy

**Option A: Subprocess per test** (simple, slow)
- Each test spawns `hoonc --output /tmp/test-NONCE.jam <input.hoon>`
- Reads the jam file
- Compares to honk's output

**Option B: Batch pre-compilation** (fast, requires setup)
- Before tests run, invoke a harness script:
  ```bash
  ./crates/honk/test-assets/generate-oracle-jams.sh
  ```
- Generates `test-assets/oracle-jams/{find,fond,repo,...}/*.jam` indexed by test name
- Tests load these golden jams from disk
- On mismatch, test failure includes a `--generate` flag to rebuild: `cargo test oracle -- --generate`

**Recommendation**: Start with Option A (subprocess) for simplicity; optimize to Option B (batch) if test suite grows large.

### 3.5 Honk-Side Implementation

```rust
// In crates/honk/tests/compiler_mint.rs or new compiler_oracle.rs

use honk::native::ut::{Ut, ty_noun, ty_atom, ty_cell, ty_face, ty_core, ...};
use nockapp::noun::slab::NounSlab;

// Helper: Mint an expression given a subject type, return both type and formula
fn mint_with_subject(
    subject_type: Noun,
    expr: &Hoon,
) -> Result<(Noun, Noun)> {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let goal = ty_noun(&mut slab);
    ut.mint(subject_type, goal, expr)
}

// Helper: Serialize a formula to jam
fn formula_to_jam(slab: &NounSlab, formula: Noun) -> Vec<u8> {
    let jam_vec = crate::artifact::jam_noun(&slab, formula)?;
    Ok(jam_vec)
}

// Helper: Assert oracle match (blake3 hash or byte-equal)
fn assert_formula_oracle(
    test_name: &str,
    oracle_jam: &[u8],
    honk_jam: &[u8],
) {
    let oracle_hash = blake3::hash(oracle_jam);
    let honk_hash = blake3::hash(honk_jam);
    assert_eq!(
        honk_hash, oracle_hash,
        "test: {} — formula must match hoonc oracle byte-for-byte",
        test_name
    );
}
```

---

## 4. Seed Corpus Categories

The fixture test cases are structured around **expression families** that exercise decision boundaries in each arm. Each category exercises a distinct code path or type constructor.

### 4.1 Expression Families

#### **A. Wet/Polymorphic Arms**
- Subject: core with `|*` wet gate or `|%` with typed arms
- Wing: reference to arm name (triggers `poly` dispatch)
- Oracle path: find->fond->core, vair branching on `poly` (wet vs dry)
- Honk path: foot_from_poly, semi-noun lazy vs full
- Edge cases: wet with no sample, wet with nested polymorphism

**Seed expressions**:
```hoon
=| outer=|*([a]([b] [a b]))  /@ wet gate with double nesting
[outer a]
```

#### **B. Recursion and Loops**
- Subject: recursive core (hold type)
- Wing: path through hold that self-references
- Oracle path: find->fond->hold->seen guard, repo
- Honk path: fond_hold_inner, hold seen tracking
- Edge cases: loop detected, loop skipped, multiple holds

**Seed expressions**:
```hoon
=| my-list=(list @)
?~(my-list 0 [i.my-list ($(my-list t.my-list))])
```

#### **C. Chains and Composition** (`=>` / TisGar)
- Subject: layered cores from `=> p q` composition
- Wing: resolve through multiple `=>` layers
- Oracle path: find recurses through composed types
- Honk path: fond recursion with composed vein
- Edge cases: empty wing on composed type, nested `=>`

**Seed expressions**:
```hoon
=< [a b]
=> [a=1 b=2]
[@ @]
```

#### **D. Structurally Equal Cores**
- Subject: Two distinct Noun allocations that are structurally equal (same type tree)
- Wing: Same path resolved on both
- Oracle path: find produces identical axis
- Honk path: fond_name must handle noun_eq fallback (not raw pointer match)
- Edge cases: deep structural equality, mug collision

**Seed expressions**:
```hoon
=| s1=|%(x 1)
=| s2=|%(x 1)  /@ same structure, distinct allocations
[s1 x.s1]
```

#### **E. Mug-Collision Forks**
- Subject: fork with branches that have the same mug but different structure
- Wing: resolve to each branch
- Oracle path: fork each branch, twin to merge
- Honk path: fond->fork->twin, with mug-collision handling
- Edge cases: three branches with partial collisions, void branches

**Seed expressions**:
```hoon
=| f=?(@ (list @))  /@ fork with potential mug collision
=+(x=f
  ?@(x x (map x)))
```

#### **F. Dbug On/Off**
- Subject: Same type tree, one with dbug info, one without
- Wing: Same path
- Oracle path: both should compile to identical formulas (dbug is transparent to find)
- Honk path: dbug stripped by type extractors (type_tag, type_cell_parts, etc.)
- Edge cases: nested dbug, dbug wrapping core

**Seed expressions**:
```hoon
=| x=@ud
!>  /@ dbug wrapping (creates hint with spot info)
x
```

#### **G. Typed vs Untyped Dynock**
- Subject: A core with explicit type vs. inferred mold (|*, |%, etc.)
- Wing: resolve arms in both
- Oracle path: find->fond->core, both paths should yield same axis/type
- Honk path: foot construction differs but result axis matches
- Edge cases: generic mold, constrained type

**Seed expressions**:
```hoon
=| g=|*([a] a)
=| c=|%(foo [@ @])
[g c]
```

---

### 4.2 Corpus Organization

**File**: `crates/honk/test-assets/parity-matrix-corpus.hoon` (source) or `crates/honk/tests/oracle_corpus/` (directory of .hoon files)

**Structure**:
```
oracle_corpus/
├── find/
│   ├── empty-wing.hoon
│   ├── axis-leaf.hoon
│   ├── term-leaf.hoon
│   ├── recursive-path.hoon
│   ├── arm-resolution.hoon
│   ├── hold-recursion.hoon
│   ├── fork-resolution.hoon
│   ├── face-tool-bridges.hoon
│   └── void-propagation.hoon
├── fond/
│   ├── base-case.hoon
│   ├── axis-chain.hoon
│   ├── term-resolution.hoon
│   ├── face-unwrap.hoon
│   ├── core-context.hoon
│   ├── hold-loop-guard.hoon
│   ├── fork-merge.hoon
│   └── error-propagation.hoon
├── repo/
│   ├── face-unwrap.hoon
│   ├── hint-unwrap.hoon
│   ├── core-extraction.hoon
│   ├── hold-via-rest.hoon
│   ├── base-types.hoon
│   ├── void-handling.hoon
│   └── error-cases.hoon
├── fend/
│   ├── simple-leg.hoon
│   ├── recursive-axis.hoon
│   ├── arm-path-error.hoon
│   ├── synthetic-error.hoon
│   └── error-propagation.hoon
├── fund/
│   ├── pure-wing.hoon
│   ├── synthetic-expr.hoon
│   ├── hybrid.hoon
│   └── error-cases.hoon
├── tack/
│   ├── root-face.hoon
│   ├── existing-face.hoon
│   ├── nested-faces.hoon
│   ├── on-core.hoon
│   ├── on-hold.hoon
│   └── error-cases.hoon
├── toss/
│   ├── single-arm.hoon
│   ├── duplicate-arms.hoon
│   ├── matching-axes.hoon
│   ├── mismatched-axes.hoon
│   └── error-cases.hoon
├── semantic-families/
│   ├── wet-polymorphism.hoon
│   ├── recursion-loops.hoon
│   ├── composition-chains.hoon
│   ├── structural-equality.hoon
│   ├── mug-collision-forks.hoon
│   ├── dbug-transparency.hoon
│   └── typed-vs-untyped.hoon
└── README.md  (corpus index and oracle-generation instructions)
```

Each `.hoon` file is a **self-contained test case**:
```hoon
!~
::  find/empty-wing.hoon
::  Oracle: find with an empty wing returns the subject type unchanged
::
=/  sut  @ud
[sut.]  /@ empty wing resolves to subject
```

---

## 5. Test Harness Implementation

### 5.1 Harness Architecture

```
test-layer:
  +-- oracle_harness::compile_expression_oracle(src: &str) -> Vec<u8>
  |   (invokes hoonc or loads cached jam)
  |
  +-- oracle_harness::mint_expression_honk(
  |       subject: Noun, 
  |       expr: &Hoon
  |   ) -> Result<Vec<u8>>
  |   (invokes honk's mint, serializes to jam)
  |
  +-- oracle_harness::assert_formula_matches_oracle(
  |       test_name: &str,
  |       honk_jam: Vec<u8>,
  |       oracle_jam: Vec<u8>
  |   )
  |   (blake3 hash compare, with detailed mismatch report)
  |
  +-- Per-arm test modules
      +-- tests::find_oracle::*
      +-- tests::fond_oracle::*
      +-- tests::repo_oracle::*
      +-- ... (fend, fund, tack, toss, ...)
```

### 5.2 Example Test Implementation

```rust
#[test]
fn find_oracle_empty_wing_leaf_type() {
    // Subject: a simple atom type @ud
    let oracle_src = r#"
        =| sut=@ud
        [sut.]  /@ empty wing, should return sut unchanged
    "#;
    
    let oracle_jam = compile_expression_oracle(oracle_src)
        .expect("oracle compilation failed");
    
    // Build the same in honk
    let mut slab = NounSlab::new();
    let sut = honk::native::ut::ty_atom(&mut slab, "ud", None);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    
    // Parse the wing reference (empty wing, just `[sut.]`)
    let expr = parse_expr("[sut.]");
    let (_ty, honk_formula) = ut.mint(sut, gol, &expr)
        .expect("honk mint failed");
    
    // Serialize honk formula
    let honk_jam = formula_to_jam(&slab, honk_formula)
        .expect("honk jam serialization failed");
    
    // Compare
    assert_formula_matches_oracle(
        "find/empty-wing (atom)",
        honk_jam,
        oracle_jam
    );
}

#[test]
fn fond_oracle_recursive_path_cell_axes() {
    let oracle_src = r#"
        =| sut=[@ @]
        [sut.[2] sut.[3]]  /@ head and tail via axes
    "#;
    let oracle_jam = compile_expression_oracle(oracle_src)
        .expect("oracle compilation");
    
    let mut slab = NounSlab::new();
    let head = honk::native::ut::ty_atom(&mut slab, "@", None);
    let tail = honk::native::ut::ty_atom(&mut slab, "@", None);
    let sut = honk::native::ut::ty_cell(&mut slab, head, tail);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    
    let expr = parse_expr("[sut.[2] sut.[3]]");
    let (_ty, honk_formula) = ut.mint(sut, gol, &expr)
        .expect("honk mint");
    
    let honk_jam = formula_to_jam(&slab, honk_formula)
        .expect("honk jam");
    
    assert_formula_matches_oracle(
        "fond/recursive-path (cell axes)",
        honk_jam,
        oracle_jam
    );
}

#[test]
fn repo_oracle_face_unwrap() {
    let oracle_src = r#"
        =| x=foo=@ud
        x  /@ unwrap face foo
    "#;
    let oracle_jam = compile_expression_oracle(oracle_src)
        .expect("oracle");
    
    let mut slab = NounSlab::new();
    let inner = honk::native::ut::ty_atom(&mut slab, "ud", None);
    let sut = honk::native::ut::ty_face(&mut slab, "foo", inner);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    
    let expr = parse_expr("x");  // resolves to the inner type via repo
    let (_ty, honk_formula) = ut.mint(sut, gol, &expr)
        .expect("honk mint");
    
    let honk_jam = formula_to_jam(&slab, honk_formula)
        .expect("honk jam");
    
    assert_formula_matches_oracle(
        "repo/face-unwrap",
        honk_jam,
        oracle_jam
    );
}
```

### 5.3 Oracle Generation Script

**File**: `crates/honk/scripts/generate-oracle-jams.sh`

```bash
#!/bin/bash
# Generate golden oracle jams for parity-matrix tests
# Invoked: ./scripts/generate-oracle-jams.sh [--regenerate]

set -e

CORPUS_DIR="test-assets/oracle-corpus"
ORACLE_DIR="test-assets/oracle-jams"
HOONC="${HOONC_BIN:-hoonc}"

# Ensure hoonc is available
if ! command -v "$HOONC" &> /dev/null; then
    echo "ERROR: hoonc not found at $HOONC"
    exit 1
fi

# Create output directory
mkdir -p "$ORACLE_DIR"

# For each corpus file, compile with hoonc and save the jam
for category_dir in "$CORPUS_DIR"/*; do
    if [ -d "$category_dir" ]; then
        category=$(basename "$category_dir")
        mkdir -p "$ORACLE_DIR/$category"
        
        for test_file in "$category_dir"/*.hoon; do
            if [ -f "$test_file" ]; then
                test_name=$(basename "$test_file" .hoon)
                output_jam="$ORACLE_DIR/$category/${test_name}.jam"
                
                echo "Compiling: $category/$test_name"
                "$HOONC" --arbitrary "$test_file" > "$output_jam" 2>/dev/null \
                    || (echo "ERROR: hoonc failed on $test_file"; rm -f "$output_jam"; exit 1)
                
                # Verify the jam is not empty
                if [ ! -s "$output_jam" ]; then
                    echo "ERROR: Empty jam for $test_file"
                    rm -f "$output_jam"
                    exit 1
                fi
            fi
        done
    fi
done

echo "Oracle jams generated successfully in $ORACLE_DIR"
```

---

## 6. Comparison and Assertion Strategy

### 6.1 Byte-Exact vs. Structural Equivalence

**Requirement**: Honk's compiled formula must be byte-identical to hoonc's when both are jammed.

**Why byte-exact?**: The parity matrix is a **correctness assertion** — if honk produces a different formula (even structurally equivalent), it indicates a behavioral divergence. Byte-identity is the gold standard because:
- It captures all expression variations (hints, memo keys, etc.)
- It gates the full-kernel parity (H0) via the same jam mechanism
- Structural equivalence would hide formulation differences (e.g., `[1 x]` vs `[1 [x]]`)

### 6.2 Mismatch Reporting

When a test fails, the harness must provide **diagnostic detail**:

```rust
fn assert_formula_matches_oracle(
    test_name: &str,
    honk_jam: Vec<u8>,
    oracle_jam: Vec<u8>,
) {
    let honk_hash = blake3::hash(&honk_jam);
    let oracle_hash = blake3::hash(&oracle_jam);
    
    if honk_hash != oracle_hash {
        // Deep diagnostic
        let honk_noun = cue(&honk_jam).expect("honk jam decode");
        let oracle_noun = cue(&oracle_jam).expect("oracle jam decode");
        
        eprintln!("ORACLE MISMATCH: {}", test_name);
        eprintln!("Expected (oracle) hash: {}", oracle_hash);
        eprintln!("Got      (honk)   hash: {}", honk_hash);
        eprintln!("\nJam sizes: oracle={} honk={}", oracle_jam.len(), honk_jam.len());
        eprintln!("\nFirst difference at byte index: {:?}",
            oracle_jam.iter().zip(&honk_jam).position(|(a, b)| a != b));
        eprintln!("\nOracleNoun: {}", noun_tree_debug(&oracle_noun));
        eprintln!("\nHonkNoun: {}", noun_tree_debug(&honk_noun));
        
        panic!("Oracle parity assertion failed");
    }
}
```

### 6.3 Named Waivers

Some mismatches may be **intentional** (e.g., a known divergence in honk from hoonc that is documented and accepted). A **waiver registry** allows tests to pass conditionally:

**File**: `crates/honk/tests/oracle-waivers.toml` (future)

```toml
[[waiver]]
test_name = "find_oracle_wet_polymorphism_lazy_battery"
reason = "honk's lazy battery defers semi-noun construction; formula equivalent but not byte-identical"
issue = "https://..."
expiration_date = "2026-12-31"  # Enforce re-evaluation periodically
hash_oracle = "abc123..."
hash_honk = "def456..."

[[waiver]]
test_name = "repo_oracle_hold_recursive_type"
reason = "honk and hoonc emit different recursive-type structures; semantically equivalent"
...
```

In tests:
```rust
#[test]
fn find_oracle_wet_polymorphism_lazy_battery() {
    let result = || {
        // ... test logic
        assert_formula_matches_oracle(...);
    }();
    
    if let Err(e) = result {
        if is_waived("find_oracle_wet_polymorphism_lazy_battery") {
            eprintln!("WAIVED: find_oracle_wet_polymorphism_lazy_battery");
            // Test passes
        } else {
            panic!("{}", e);
        }
    }
}
```

---

## 7. Integration and Execution

### 7.1 Test Invocation

```bash
# Run all oracle parity tests
cargo test -p honk --test compiler_oracle -- --nocapture

# Run a specific arm's tests
cargo test -p honk --test compiler_oracle find_oracle

# Run with oracle regeneration (if mismatch)
HONK_REGEN_ORACLES=1 cargo test -p honk --test compiler_oracle

# Run with verbose diff output
HONK_ORACLE_VERBOSE=1 cargo test -p honk --test compiler_oracle
```

### 7.2 CI Integration

Add to `.github/workflows/honk-tests.yml` (or justfile recipe):

```yaml
- name: Honk Oracle Parity
  run: |
    cargo test -p honk --test compiler_oracle \
      --release \
      -- --test-threads=1
```

Or as a justfile recipe:

```justfile
honk-oracle-parity:
    cargo test -p honk --test compiler_oracle -- --nocapture --test-threads=1
```

### 7.3 Relation to Whole-Kernel Parity (H0)

| Scope | Tool | Granularity | Gate |
|-------|------|-------------|------|
| **Per-expression** | oracle-fixtures (Phase 0) | find/fond/repo per input shape | Per-category coverage |
| **Whole-kernel** | jam-diff --kernel-parity (H0) | Six kernels end-to-end | Byte-exact or dir-hash-only |

The oracle fixtures are a **supplement** to H0: they validate that individual arms' decisions are correct **before** applying them to real kernel compiles. If a kernel parity test fails, the oracle suite helps isolate whether the issue is in find/fond/repo or elsewhere.

---

## 8. Implementation Roadmap

### Phase 0 (Design, this document)
- [x] Define parity-matrix structure per arm
- [x] Specify seed-corpus categories
- [x] Design hoonc-oracle-fixture mechanism
- [ ] **TODO**: Finalize corpus and golden-jam layout

### Phase 0.5 (Oracle Setup)
- [ ] Generate oracle jams for all corpus categories (via generate-oracle-jams.sh)
- [ ] Commit golden jams as test assets
- [ ] Build oracle-harness module (harness.rs)

### Phase 1 (Find/Fond Parity)
- [ ] Implement find_oracle test suite (10–15 tests per category)
- [ ] Implement fond_oracle test suite
- [ ] Verify byte-exactness; document any waivers
- [ ] Remove `status=partial` markers from find.rs / fond when tests all pass

### Phase 2 (Remaining Arms)
- [ ] repo_oracle suite
- [ ] fend_oracle suite
- [ ] fund_oracle suite
- [ ] tack_oracle suite (if in scope)
- [ ] toss_oracle suite (if in scope)

### Phase 3 (Coverage and Hardening)
- [ ] Add semantic-family tests (wet, recursion, composition, etc.)
- [ ] Verify dbug transparency (same formula for on/off)
- [ ] Verify structural-equality handling (noun_eq fallback)
- [ ] Add mug-collision tests
- [ ] Document any known divergences as waivers

### Phase 4 (Integration)
- [ ] Wire oracle suite into CI (`just honk-oracle-parity`)
- [ ] Update TODOS.md to mark #11/#15 resolved (with caveats if incomplete)
- [ ] Update README.md parity claims with oracle coverage

---

## 9. Success Criteria

Phase 0 completion is achieved when:

1. **Matrix design locked**: All eight arms have detailed parity matrices specifying input families, edge cases, oracle behavior, and honk paths.
2. **Corpus complete**: 60–80 test cases cover all matrix categories; corpus committed to test-assets/.
3. **Oracle jams generated**: All golden jams (hoonc outputs) cached and reproducible via generate-oracle-jams.sh.
4. **Harness operational**: oracle_harness module compiles and all test placeholders execute (even if some fail initially).
5. **Per-arm gates**: Each arm's test suite can be run independently (`cargo test find_oracle`, etc.).
6. **Mismatch diagnostics**: Failed tests report blake3 hash, byte diffs, noun trees, and waiver status.

Phase 1 completion (find/fond parity) adds:
- All find_oracle tests passing (byte-exact or waived)
- All fond_oracle tests passing
- `status=partial` markers removed from find.rs / fond (TODOS #15 resolved for those two arms)
- Docstring updated in find.rs lines 38-81 to reference oracle test suite

---

## 10. Open Questions & Future Work

1. **Lazy battery handling**: Honk's `semi_noun_lazy` vs hoonc's full-battery construction may produce different formulas that are semantically equivalent. Should this be a waiver, or should honk be forced to match hoonc's battery eagerness?

2. **Dbug transparency**: The spec requires dbug to be transparent (same formula on/off). Is honk currently achieving this? If not, the dbug-family tests will fail — investigate whether this is a bug or a design difference.

3. **Mug collision handling**: When structurally distinct types have the same mug, honk's type system must fall back to `noun_eq` structural comparison. The mug-collision test family will exercise this; if failures occur, it may indicate a bug in the fallback path.

4. **Hold recursion via repo**: The `repo_hold` function is the only path that calls `rest` (a separate arm not in this matrix). Does `rest`'s result always align with what `repo` would return? The hold-family tests will verify this.

5. **Cross-expression composition**: What happens when expressions reference types built by other expressions (e.g., through `=+`, `=|`, `=*`)? The composition-chain and semantic-family tests explore this; ensure honk's subject-embedding handles it correctly.

6. **Performance impact**: The oracle suite will invoke hoonc (or load large golden jams) for every test. Monitor CI time; if it becomes a bottleneck, switch to binary caching or selective-run strategies.

---

## Appendix A: Matrix Template (Blank)

Use this template for designing matrices for arms not yet detailed:

```
# ++ARMNAME Parity Matrix

Input: [describe inputs]
Output: [describe outputs]

## Categories

### 1. Category Name
| Input | Case | Oracle | Honk Path | Check |
|-------|------|--------|-----------|-------|
| | | | | |

## Edge Cases
- [list edge cases]

## Oracle Behavior Summary
[High-level description of hoonc's behavior]

## Honk Implementation Notes
[Code paths, potential divergences, implementation strategy]
```

---

## Appendix B: Useful References

- **TODOS.md**: Items #11 (hoonc-oracle fixtures), #15 (find/fend/fund/fond parity matrices)
- **OSS-NEXT-PLAN.md**: Phase 0 design artifacts, deferred follow-ups
- **find.rs**: Lines 38-81 (find/fond/fend/fund current implementation)
- **repo.rs**: Lines 174-205 (repo current implementation)
- **compiler_mint.rs**: Lines 2452–2459 (strict semantic tests, intentionally no hoonc)
- **H0 kernel-parity harness**: `just honk-parity`, `jam_diff --kernel-parity`

---

**Document Version**: 1.0  
**Status**: Ready for implementation (Phase 0.5 / Phase 1)  
**Last Updated**: 2026-06-16  
**Author**: Claude Code (Anthropic)
