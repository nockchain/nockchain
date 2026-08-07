# Phase 0 Decision: Chunked Prelude Mint (RT-18)

**Status:** RESOLVED — productized for native-parity mode
**Author:** Code Analysis  
**Date:** 2026-06-16  
**Resolved:** 2026-08-06
**Scope:** honk native compiler, canonical hoon-138 prelude compilation  
**Decision:** PRODUCTIZE for `HONK_NATIVE_PARITY=1`; retain monolithic escape hatch

---

## Executive Summary

The chunked prelude mint is now the bounded implementation used when
`HONK_NATIVE_PARITY=1` asks Honk to rebuild hoon-138 instead of substituting the
embedded prelude. That route intentionally parses the prelude with `dbug=false`
and passes the strict whole-artifact comparison in `just honk-138-parity`.
`NATIVE_HOON_NO_CHUNK=1` retains the monolithic diagnostic path. Chunking must
not be reused with prelude debugging spots enabled until the peeled outer
`Dbug`/`Note` wrappers are preserved.

The remainder of this document is the historical Phase-0 decision analysis.
Its quarantine proposal explains the risk that informed the final routing, but
it is not a current implementation instruction.

---

## Current State: What Chunked Does

### 1. Purpose and Design

The chunked mint was designed to **bound peak working memory** during canonical prelude compilation by minting each top-level layer in its own fresh `Ut`/slab, rather than accumulating all layers in a single monolithic slab.

**The canonical hoon-138 prelude structure:**
```
=< ride => %138 => |% … => |% …  (six layer cores)
```

**What chunked does:**
1. Peels transparent wrapper nodes (`TisSig`, `Dbug`, `Note`) to reach the underlying compose chain
2. Walks the `=>` (TisGar) chain and collects each layer
3. For each layer (and finally the ride):
   - Creates a fresh `Ut`/working slab
   - Loads cold state from embedded hoon-138 compile base
   - Mints the layer against the carried subject (type)
   - Copies only the resulting type and formula into the output slab
   - Drops the working slab (reclaiming memory)
4. Composes layer formulas via `comb` exactly as the monolithic mint does

**Peak memory bound:** one layer + the carried subject, instead of cumulative six-layer accumulation.

### 2. Routing Path (ACTIVE)

**File:** `/Users/callen/work/nockchain/crates/honk/src/bin/honk.rs`

**Decision point:** `mint_honc_formula_with_ut` (line 2603)

```rust
fn mint_honc_formula_with_ut(
    ut: &mut Ut<'_>,
    _context: &mut Context,
    prelude: &Hoon,
) -> Result<Noun> {
    // The canonical prelude is `=< …`; mint it chunked to bound memory (Step 2).
    if matches!(peel_transparent(prelude), Hoon::TisGal(_, _)) {
        return mint_honc_prelude_chunked(&mut *ut.slab, prelude);
    }
    // ... fallback to monolithic mint
}
```

**When chunked is invoked:**
- Canonical hoon-138 prelude (e.g., in roswell kernel builds)
- The peeled root node is `Hoon::TisGal` (`=<` operator)
- **This path is taken unconditionally** — no feature flag or environment variable gates it for normal operation

**Related code paths:**
- `native_parity_enabled()` (line 854): When `HONK_NATIVE_PARITY` is set, native compilation happens instead of embedded formula substitution, which means the chunked path becomes visible for validation
- In embedded mode (default), the chunked path's output is frozen into the binary and not recomputed

### 3. Byte-Exactness Validation

**Monolithic equivalence test:** `chunked_tisgar_chain_matches_monolithic_mint` (in `crates/honk/src/native/ut/test.rs`)

```rust
#[test]
fn chunked_tisgar_chain_matches_monolithic_mint() {
    // Synthetic: =>  |%  ++  a  1  --  =>  |%  ++  b  a  --  b
    // Validates that chunked mint produces byte-identical formula to monolithic,
    // proving cross-layer lazy-resolver clearing is safe
}
```

**Key invariant proven:** Chunked formulas are byte-identical to monolithic when each layer's cores have full batteries (lazy resolvers, dropped per-layer, are not needed for cross-layer name resolution). This test confirms the correctness crux.

### 4. The dbug=true Non-Exactness Issue

**Location:** `honk.rs:2495-2500`

```rust
/// Peel transparent wrappers the parser adds around the prelude (a single-
/// element `=~`/TisSig, and `Dbug`/`Note` spot/hint wrappers) to reach the
/// underlying compose node. NOTE: this is for NAVIGATION only — minting the
/// peeled layers loses the outer `Dbug` location stack, so chunked output is
/// NOT byte-exact under dbug=true yet (spot preservation is follow-on work);
/// it is correct for measuring the memory trajectory and for dbug=false.
```

**The Problem:**

The `peel_transparent` function (line 2501) strips `Dbug`/`Note` nodes to find the underlying `TisGal` compose root. When minting each peeled layer, the location metadata in these stripped nodes is **lost**. The monolithic path preserves this metadata in the formula output (via `Dbug` hinting), but the chunked path does not.

**Implications:**
- **dbug=false:** Chunked output is byte-exact ✓
- **dbug=true:** Chunked output loses location information, **differs from monolithic** ✗
- Error reporting in chunked mode under `dbug=true` will have incomplete stack traces
- **Phase-3 parity evidence cannot use chunked output** when dbug differs

### 5. Memory Experiment Gating

The chunked path is the mechanism for **validating memory-bounded native compilation** — a critical thesis for Phase 3:

> "The native prelude mint, properly chunked, stays within a bounded working-memory envelope, proving the native compiler can self-host the kernel."

**Current status:**
- Chunked mint is validated to be **byte-exact for dbug=false** (via the test)
- Chunked mint is **not validated for dbug=true** (known non-exactness)
- The path exists, is in production, and is used for memory measurements
- **But it cannot be used as Phase-3 evidence** without dbug byte-exactness

---

## Analysis: Why This Matters

### A. The Chunked Path Is Not Obsolete

Earlier plans suggested dropping it; that is **incorrect**. Evidence:

1. **It is routed unconditionally** in production (line 2615-2616)
2. **The test passes** — byte-exactness for dbug=false is proven
3. **It gates the memory thesis** — without it, we have no measurement of bounded compilation
4. **Removing it would break native-parity audits** that rely on comparing native prelude output

### B. The dbug=true Gap Is Real but Tractable

The loss of location metadata is a known, **scoped limitation**:
- Affects only peeled outer wrappers (not inner expressions)
- Solvable via "spot preservation" — threading dbug context through the chunked driver
- Not a fundamental flaw in the chunked approach

### C. It Gates Phase-3 Memory Evidence

Phase 3 must answer: **"Can honk compile the kernel within bounded memory?"**

The chunked path is the only current implementation that bounds memory. Its memory trajectory can be measured, but only the **dbug=false variant** is suitable for validation against monolithic output.

---

## Recommendation: QUARANTINE (Conditional Gating)

### Decision

**Gate the chunked path behind an explicit feature flag** rather than unconditionally routing it. This approach:

1. ✓ Preserves the correctness validation (test keeps running)
2. ✓ Allows memory experiments to proceed (flag can be enabled for measurement)
3. ✓ Prevents accidental use of non-byte-exact output as Phase-3 evidence
4. ✓ Leaves the door open for eventual productization after dbug=true fix
5. ✓ Keeps stale code visible and testable (not deleted, not silently active)

### Concrete Steps

#### Step 1: Add an Environment Variable Gate

**File:** `honk.rs` at the routing point (line 2615)

Replace:
```rust
if matches!(peel_transparent(prelude), Hoon::TisGal(_, _)) {
    return mint_honc_prelude_chunked(&mut *ut.slab, prelude);
}
```

With:
```rust
if std::env::var_os("HONK_CHUNKED_PRELUDE").is_some() &&
   matches!(peel_transparent(prelude), Hoon::TisGal(_, _)) {
    return mint_honc_prelude_chunked(&mut *ut.slab, prelude);
}
```

This requires explicit opt-in: `HONK_CHUNKED_PRELUDE=1 honk …`

#### Step 2: Document the Flag

**Location:** Code comment above routing point

```rust
/// QUARANTINED: Chunked prelude mint is currently NON-BYTE-EXACT under dbug=true
/// due to loss of Dbug/Note location metadata during transparent-node peeling
/// (see honk.rs:2495-2500). The path is correct for dbug=false and suitable for
/// memory-trajectory measurement, but MUST NOT be used as Phase-3 parity evidence
/// until spot preservation is implemented. Enable only for memory experiments:
/// `HONK_CHUNKED_PRELUDE=1`.
```

#### Step 3: Update the Test Comment

**File:** `crates/honk/src/native/ut/test.rs`

Add to `chunked_tisgar_chain_matches_monolithic_mint`:

```rust
/// QUARANTINE SCOPE: This test validates chunked mint under dbug=false.
/// The chunked path is gated behind HONK_CHUNKED_PRELUDE to prevent use of
/// non-byte-exact output (dbug=true case) as Phase-3 evidence.
/// This test must pass before PRODUCTIZING chunked.
```

#### Step 4: Add Warnings to Verbose Output

When chunked is enabled, emit a warning:

```rust
if env::var_os("HONK_CHUNKED_PRELUDE").is_some() {
    eprintln!(
        "[honk] WARNING: chunked prelude mint is QUARANTINED (non-byte-exact under dbug=true)"
    );
    eprintln!(
        "[honk] Use only for memory experiments. Phase-3 parity evidence requires dbug-true byte-exactness."
    );
}
```

#### Step 5: Plan Productization

Create a follow-on task (H7-E or Phase-1 work):

> **Chunked prelude — spot preservation (dbug=true byte-exactness)**
>
> Thread dbug context (`%spot` hints) through `mint_honc_prelude_chunked` so that peeled-layer minting preserves location metadata. Validate with `chunked_tisgar_chain_matches_monolithic_mint` under both `dbug=false` and `dbug=true`. Once validated, remove the `HONK_CHUNKED_PRELUDE` gate and productize.

---

## Rationale for QUARANTINE over DELETE

### Why Not DELETE?

1. **Loss of evidence:** Deleting the path means losing the byte-exactness test and the only current memory-bounding implementation
2. **Phase-3 risk:** Memory thesis cannot be validated without it
3. **Code clarity:** Deletion obscures why the decision was made; quarantine with comments is more informative

### Why Not PRODUCTIZE Immediately?

1. **dbug=true is broken:** Non-byte-exact output cannot be used as Phase-3 evidence
2. **Spot preservation work is deferred:** The fix (threading dbug context) is tractable but requires focused work
3. **Current embedded path is sufficient:** Default mode uses the embedded prelude, so non-production testing is unaffected
4. **Risk mitigation:** Explicit gate prevents accidental use in validation

### Why QUARANTINE?

1. **Explicit gating prevents accidents**
2. **Test keeps running, keeping the path warm**
3. **Memory experiments can proceed** (researchers enable the flag)
4. **Clear path to productization** (fix spot preservation, remove gate)
5. **Better than silent breakage** (deletion, then rediscovery)

---

## Warnings and Non-Evidence Constraints

### ⚠️ Critical: Do NOT Use Chunked Output as Phase-3 Evidence

If `HONK_CHUNKED_PRELUDE` is enabled and `--dbug` is active, the resulting formulas are **NOT byte-identical to hoonc** and **MUST NOT be used for parity validation**.

**Valid uses of chunked output:**
- Memory-trajectory measurement (dbug=false only)
- Relative performance comparison (dbug=false only)
- Correctness testing on small synthetic prelude examples

**Invalid uses:**
- Parity evidence for Phase 3 (unless dbug=true byte-exactness is proven)
- Validation against oracle/hoonc output when dbug differs
- Kernel shipping (not affected; embedded mode is default)

### How to Enforce This Locally

Add a check in the CI/validation harness:

```rust
if env::var_os("HONK_CHUNKED_PRELUDE").is_some() &&
   env::var_os("HONK_DBUG_TRUE").is_some() {
    eprintln!("[honk] ERROR: cannot use chunked prelude with dbug=true for parity evidence");
    std::process::exit(1);
}
```

---

## Implementation Timeline and Rollout

### Phase 0 (Immediate)

1. Apply the gating (Step 1 above)
2. Update comments and warnings (Steps 2–4)
3. Add no-use assertion to validation harness
4. Mark task H7-E for follow-on

**Effort:** ~30 minutes  
**Risk:** Very low (gate is conservative; test still passes)

### Phase 1 (Within 2–3 weeks, not a blocker)

1. Plan spot-preservation work
2. Prototype dbug context threading
3. Validate byte-exactness under both flags

### Productization (After Phase 1)

1. Remove `HONK_CHUNKED_PRELUDE` gate
2. Delete quarantine warnings
3. Confirm byte-exactness test passes
4. Update TODOS-PERF with results

---

## Files Affected

| File | Change | Reason |
|------|--------|--------|
| `crates/honk/src/bin/honk.rs:2615` | Add `HONK_CHUNKED_PRELUDE` gate | Gating logic |
| `crates/honk/src/bin/honk.rs:2486-2510` | Update docstring | Clarify quarantine status |
| `crates/honk/src/native/ut/test.rs` | Add quarantine note to test | Document scope |
| `docs/native-compiler/PHASE0-CHUNKED-DECISION.md` | Create this document | Rationale & evidence |
| `docs/native-compiler/TODOS-PERF.md` | Update Step 2 section | Reference quarantine decision |
| (Optional) Harness code | Add parity assertion | Prevent misuse |

---

## Success Criteria

- [ ] Gate is applied and chunked path requires explicit `HONK_CHUNKED_PRELUDE=1`
- [ ] Test `chunked_tisgar_chain_matches_monolithic_mint` still passes
- [ ] Default builds (without flag) use monolithic mint as before
- [ ] Memory experiments can enable flag and measure bounded trajectory
- [ ] Documentation warns against using chunked output as Phase-3 evidence under dbug=true
- [ ] Follow-on task (H7-E) is created for spot-preservation work

---

## Related Documents and Tasks

- **OSS-NEXT-PLAN.md § Step 2:** Memory bounding via chunked prelude (background)
- **TODOS-PERF.md:** Step 2 progress notes; will reference this decision
- **NATIVE-TYPES-MIGRATION.md § §9:** Branch hygiene (RT-18); this decision fulfills it
- **Task H7-E** (to be created): Spot preservation for chunked dbug=true byte-exactness

---

## Appendix: Code Locations

### Chunked Mint Entry Point
**File:** `/Users/callen/work/nockchain/crates/honk/src/bin/honk.rs`
- Lines 2534–2601: `mint_honc_prelude_chunked` function
- Lines 2501–2510: `peel_transparent` helper
- Lines 2603–2617: Routing in `mint_honc_formula_with_ut`
- Lines 2512–2532: `prelude_variant_name` helper

### Chunked Layer Mint (Ut Layer)
**File:** `/Users/callen/work/nockchain/crates/honk/src/native/ut/mod.rs`
- Lines 11964–12004: `mint_tisgar_chain_chunked` function (core layer logic)
- Lines 11958–11963: Docstring with invariant description

### Byte-Exactness Test
**File:** `/Users/callen/work/nockchain/crates/honk/src/native/ut/test.rs`
- Test: `chunked_tisgar_chain_matches_monolithic_mint`
- Validates chunked output equals monolithic under normal conditions

### Related Context
- **Embedded prelude formula:** `EMBEDDED_HONC_COLD_138_JAM` (honk.rs:871–877)
- **Native parity flag:** `HONK_NATIVE_PARITY` (honk.rs:848–856)
- **Dbug documentation:** honk.rs:2495–2500, with spot/hint preservation notes

---

## Conclusion

The chunked prelude mint is **a live, correctness-validated mechanism** that is **not a stale branch to delete**. However, it has a **known non-exactness under dbug=true** that makes it unsuitable for Phase-3 parity evidence **in its current form**.

**Quarantining** it (gating behind `HONK_CHUNKED_PRELUDE=1`) preserves its value for memory experiments while preventing accidental misuse. This is a **low-risk, high-clarity decision** that acknowledges both its current utility and its current limitations, charts a clear path to full productization, and ensures Phase-3 evidence is not compromised.

**Action: Implement QUARANTINE gating as specified in Steps 1–5 above.**
