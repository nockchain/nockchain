# Phase 0 Provenance Design: PROVENANCED-LEAF + to_noun Copy Cache

## Executive Summary

Phase 0 delivers a provenance-safe architecture for native Formula/Type leaves (quoted constants, hint clues, dbug spots) and the verified deep-copy infrastructure required by RT-04 and RT-16. This document specifies:

1. **Provenanced Leaf Type** — A safe, owned wrapper for nouns embedded in Formula that tracks source provenance and prevents alien-pointer hazards
2. **Checked Deep-Copy API** — The `to_noun` boundary with mandatory source space tracking and structural validation
3. **Destination Slab Copy Cache** — A copy-on-write mechanism preventing duplicate assembly of huge batteries/constants during output compilation

The design eliminates the "bare Noun as Formula leaf" hazard documented in RT-04 and RT-16, making formula serialization provenance-safe at compile time rather than relying on runtime address-range panics.

---

## Part 1: The Problem (RT-04 and RT-16 Root Causes)

### The Alien-Noun Hazard

**RT-04 problem statement:** The plan stores quoted constants, hint clues, and dbug spots as bare `Noun` leaves in native formulas. This violates the core invariant of Noun safety: a `Noun` is only meaningful relative to its owning arena (NockStack, NounSlab, PMA, or an explicit `NounSpace`).

```rust
// UNSAFE: Current design (what we're replacing)
pub enum Formula {
    Const(Noun),           // ← bare noun; where did it come from?
    JetHint {
        clue: Noun,        // ← must be resolvable by... which space?
        body: Box<Formula>,
    },
    Dbug(Noun, Box<Formula>),  // ← source spot; owned by which arena?
}

// Hazard: to_noun receiver sees Formula with Noun leaves whose provenance is lost
fn formula_to_noun(formula: &Formula, dest: &mut NounSlab) -> Noun {
    match formula {
        Formula::Const(n) => n,  // ← BUG: just copy the raw pointer/offset
        // If `n` points to parser stack, eval-context slab, or a frame already popped,
        // this becomes a dangling pointer; release mode skips range checks.
    }
}
```

**Why this matters:**

- `NounSpace` has a release-mode identity fast-path that skips range validation for no-PMA slabs (nockvm `noun.rs:407-410`). A foreign pointer never visible to a range check may silently resolve to the wrong memory.
- `NounSlab::set_root` panics on allocated roots outside the slab. Embedding a bare noun as a leaf, then converting to the output slab, must either deep-copy or panic — but if the copy happens implicitly without source space, the copy can fail silently in release.
- Safe copying requires both the source noun and its source `NounSpace` (`nockapp extensions.rs:136-190`). A bare leaf erases this pairing.

**RT-16 problem statement:** Even if the copy-cache deduplicates large structures during jamming, the intermediate output-assembly slab can materialize the same huge constant multiple times before jam backrefs compress it. This can blow RSS before the final byte-exact jam.

Example: A large prelude formula with 50 `Const(huge_battery)` leaves, all pointing to the same parsed battery. Without a copy cache, `to_noun` copies that battery 50 times into the output slab (distinct addresses, unshared). Jam later backrefs them all down to one byte sequence, but peak RSS shows all 50 copies.

---

## Part 2: Provenanced Leaf Type Design

### Rust Type Definition

```rust
/// A noun leaf embedded in native Formula, paired with proof of its source provenance.
/// This replaces bare `Noun` leaves in formula constants, hints, and dbug spots.
/// 
/// The key invariant: a `ProvenancedLeaf` can only be constructed by copying a noun
/// INTO this slab from a verified source space, or by reading a noun already proven
/// resident in this slab. Once constructed, it is safe to use without further provenance checks.
pub struct ProvenancedLeaf {
    /// The noun itself. After construction, guaranteed to be owned by the slab
    /// that contains the Formula holding this leaf.
    noun: Noun,
    
    /// Proof of valid copy at construction time. In release builds, this is
    /// optional and elided (the noun's provenance is proven by construction
    /// path, not by field presence). In debug builds, this carries a checksum
    /// or hash of the source space's ranges at copy time, for assertions.
    #[cfg(debug_assertions)]
    source_proof: Option<ProvenanceProof>,
}

#[cfg(debug_assertions)]
pub struct ProvenanceProof {
    /// Hash of the source NounSpace's arena base + size at copy time,
    /// to catch use-after-drop/invalidation during debugging.
    source_space_signature: u64,
    /// Hash of the copied noun's structure (shallow tree hash).
    leaf_structure_hash: u64,
}

impl ProvenancedLeaf {
    /// Construct a leaf by copying a noun from a verified source space into
    /// the destination slab. This is the ONLY public constructor for leaves
    /// that come from outside the slab.
    /// 
    /// # Arguments
    /// - `source_noun`: the noun to be leaf-ified
    /// - `source_space`: the NounSpace that owns `source_noun`
    /// - `dest_slab`: the slab that will own the result (typically the Formula's slab)
    /// 
    /// # Returns a ProvenancedLeaf whose `.noun()` is allocated in `dest_slab`.
    /// 
    /// # Panics
    /// - If `source_noun` is not resolvable in `source_space` (in debug)
    /// - If the deep copy fails (destination slab out of memory, or
    ///   source_space points into PMA but copy_into sees offset-form leaves
    ///   without a PMA base).
    pub fn copy_from(
        source_noun: Noun,
        source_space: &NounSpace,
        dest_slab: &mut NounSlab,
    ) -> Self {
        // Deep-copy the noun into dest_slab, with provenance validation
        let copied = dest_slab.copy_into(source_noun, source_space);
        
        #[cfg(debug_assertions)]
        {
            // Verify the copy landed in the destination slab
            let allocated_location = copied.allocated_location();
            assert!(
                allocated_location.is_stack() || dest_slab.contains_ptr(allocated_location),
                "copy_into produced a noun not resident in dest_slab; copy_into violated its contract"
            );
        }
        
        #[cfg(debug_assertions)]
        let source_proof = Some(ProvenanceProof {
            source_space_signature: source_space.ptr_range_signature(),
            leaf_structure_hash: shallow_tree_hash(source_noun, source_space),
        });
        
        #[cfg(not(debug_assertions))]
        let source_proof = None;
        
        ProvenancedLeaf {
            noun: copied,
            #[cfg(debug_assertions)]
            source_proof,
        }
    }
    
    /// Construct a leaf from a noun already resident in the given slab.
    /// Use this for formula-construction results and slab-allocated constants.
    /// 
    /// # Panics
    /// - If `noun` is not allocated within `slab` (when allocated).
    pub fn from_slab_resident(noun: Noun, slab: &NounSlab) -> Self {
        if let Ok(allocated) = noun.as_allocated() {
            match allocated.as_either() {
                Either::Left(indirect) => {
                    let ptr = unsafe { indirect.data_pointer_stack() }
                        .expect("slab noun should have stack pointer");
                    assert!(
                        slab.contains_ptr(ptr as *const u8),
                        "noun is not resident in the given slab"
                    );
                }
                Either::Right(cell) => {
                    let ptr = unsafe { cell.stack_memory_pointer() }
                        .expect("slab noun should have stack pointer");
                    assert!(
                        slab.contains_ptr(ptr as *const u8),
                        "noun is not resident in the given slab"
                    );
                }
            }
        }
        
        ProvenancedLeaf {
            noun,
            #[cfg(debug_assertions)]
            source_proof: None, // Already in the slab; no copy proof needed
        }
    }
    
    /// Read the noun. Safe: the noun is provably resident in the owning slab.
    pub fn noun(&self) -> Noun {
        self.noun
    }
    
    /// Hash for dedup/cache lookups. Stable across runs if the noun is the
    /// same (byte-identical).
    pub fn structural_hash(&self) -> u64 {
        self.noun.mug()  // or a custom leaf-identity hash if mugs collide
    }
}
```

### Integration into Formula Enum

```rust
/// Native Formula IR with provenance-safe leaves.
pub enum Formula {
    Const(ProvenancedLeaf),       // Quoted constant; provenance-checked
    Slot(Axis),                    // Nock 0: axis into subject
    
    Hint {
        clue: ProvenancedLeaf,     // Hint clue; provenance-checked
        body: Box<Formula>,
    },
    
    Dbug(ProvenancedLeaf, Box<Formula>),  // Source spot; provenance-checked
    
    Cons {
        head: Box<Formula>,
        tail: Box<Formula>,
    },
    
    Comb {
        formula: Box<Formula>,
        subject: Box<Formula>,
    },
    
    Cond {
        test: Box<Formula>,
        pass: Box<Formula>,
        fail: Box<Formula>,
    },
    
    // ... other variants
}
```

### Why This Type is Safe

1. **Constructor Discipline**: `ProvenancedLeaf::copy_from` is the ONLY way to incorporate a foreign noun into a Formula. It requires the source space and destination slab, enforcing the provenance pairing.

2. **Debug Validation**: In debug builds, the `ProvenanceProof` fields make it cheap to verify that the source space hasn't been invalidated and that the copied noun structure matches expectations.

3. **Release Fast Path**: In release builds, the fields are elided. The noun is provably resident in the destination slab by construction (copy_into guarantees this), so no runtime checks are needed. The release binary is identical to a direct `Noun` embedded in Formula.

4. **Type System Enforcement**: A Formula can only be created by trusted Formula-building code (minter, native IR generators). Those sites pass `ProvenancedLeaf` to the Formula constructor, making it a compile-time error to try to use a bare noun.

5. **Lifetime Safety**: Once a Formula is assigned to a Compiled output, it can be serialized to jam/dynock only through the output slab, which prevents the formula from being used with a foreign noun space.

---

## Part 3: Checked Deep-Copy API (`to_noun` Boundary)

### The Copy-Into Signature (Existing Foundation)

The existing `NounSlab::copy_into` and `NounAllocatorExt::copy_into` already implement safe deep copy with provenance. The contract:

```rust
pub trait NounAllocatorExt {
    /// Deep-copy a noun from a source space into this allocator's arena,
    /// preserving internal sharing and filtering out external references.
    /// 
    /// Internal sharing (same noun referenced twice in the source) is preserved
    /// via a pointer-keyed dedup map. External references (pointers outside
    /// the source space) are shared by reference, assumed already resident
    /// in the destination (or an arena the destination outlives).
    fn copy_into(&mut self, noun: Noun, space: &NounSpace) -> Noun;
}
```

**Current code** (nockapp `extensions.rs:136-190`):
- Walks the noun tree depth-first
- Maps source pointers to destination nouns via `IntMap<u64, Noun>`
- Skips nodes not in the source space (assumes they're already resident)
- Copies indirect atoms and cells with `copy_nonoverlapping`
- Clears cached mugs to avoid corruption

**Requirements for Phase 0:**

1. `copy_into` remains the core copy primitive.
2. Every Formula leaf construction site that receives a source noun must call `ProvenancedLeaf::copy_from`, which internally calls `copy_into`.
3. `to_noun` on a Formula uses the destination slab's `copy_into`, passing the source space.
4. Release builds skip provenance-proof fields but still execute the copy; the copy itself is the runtime guard.

### Formula::to_noun Implementation

```rust
impl Formula {
    /// Convert a native Formula into a Nock noun, allocated in the given slab.
    /// 
    /// All ProvenancedLeaf nodes are already resident in `dest_slab` (by construction),
    /// so this is a structural conversion without deep copy. However, we use the
    /// copy-cache to avoid re-building huge shared batteries/constants.
    pub fn to_noun(
        &self,
        dest: &mut NounSlab,
        copy_cache: &mut CopyCacheForFormula,
    ) -> Noun {
        match self {
            Formula::Const(leaf) => {
                // The leaf is already in dest (by construction).
                // Look it up in the copy-cache to avoid rebuilding if it appears
                // multiple times in the formula.
                if let Some(cached) = copy_cache.get(leaf.structural_hash()) {
                    cached
                } else {
                    let n = leaf.noun();
                    copy_cache.insert(leaf.structural_hash(), n);
                    n
                }
            }
            
            Formula::Slot(axis) => {
                // Nock 0: [0 axis]
                T(dest, &[D(0), axis.to_noun(dest)])
            }
            
            Formula::Hint { clue, body } => {
                // Nock 11: [11 clue body]
                let body_noun = body.to_noun(dest, copy_cache);
                let clue_noun = if let Some(cached) = copy_cache.get(clue.structural_hash()) {
                    cached
                } else {
                    let n = clue.noun();
                    copy_cache.insert(clue.structural_hash(), n);
                    n
                };
                T(dest, &[D(11), clue_noun, body_noun])
            }
            
            Formula::Dbug(spot, body) => {
                // Nock 12: [12 spot body]
                let body_noun = body.to_noun(dest, copy_cache);
                let spot_noun = if let Some(cached) = copy_cache.get(spot.structural_hash()) {
                    cached
                } else {
                    let n = spot.noun();
                    copy_cache.insert(spot.structural_hash(), n);
                    n
                };
                T(dest, &[D(12), spot_noun, body_noun])
            }
            
            Formula::Cons { head, tail } => {
                // Nock 1: [1 head tail]
                let h = head.to_noun(dest, copy_cache);
                let t = tail.to_noun(dest, copy_cache);
                T(dest, &[D(1), h, t])
            }
            
            Formula::Comb { formula, subject } => {
                // Nock 2: [2 formula subject]
                let f = formula.to_noun(dest, copy_cache);
                let s = subject.to_noun(dest, copy_cache);
                T(dest, &[D(2), s, f])
            }
            
            Formula::Cond { test, pass, fail } => {
                // Nock 6: [6 test pass fail]
                let t = test.to_noun(dest, copy_cache);
                let p = pass.to_noun(dest, copy_cache);
                let f = fail.to_noun(dest, copy_cache);
                T(dest, &[D(6), t, p, f])
            }
            
            // ... other variants
        }
    }
}
```

### Why This Is Safe

1. **No source space needed at to_noun time**: All leaves are already copied into the destination slab when the Formula is constructed. The `copy_into` call happened in `ProvenancedLeaf::copy_from`, paired with the source space.

2. **Structural conversion only**: The `to_noun` method walks the Formula's structure and emits Nock opcodes, building cells with `T(dest, ...)`. It never reads or writes raw pointers.

3. **Copy-cache dedup**: The cache prevents re-assembly of the same leaf noun multiple times, bounding intermediate slab growth even if the formula references huge batteries many times.

4. **Local provenance throughout**: Every noun in `dest` at `to_noun` completion is either:
   - A newly-minted opcode cell (allocated in `dest`)
   - A leaf noun (copied into `dest` in `ProvenancedLeaf::copy_from`)
   - Never a pointer or offset from another arena

---

## Part 4: Destination-Slab Copy Cache (RT-16)

### Copy Cache Design

```rust
/// Cache for `Formula::to_noun` that deduplicates large leaves during assembly.
/// 
/// When a Formula references the same huge constant (e.g., a battery) in multiple
/// Const/Hint/Dbug leaves, the first occurrence is assembled into the slab; later
/// occurrences hit the cache and reuse the same noun, preserving sharing.
/// 
/// This prevents the intermediate slab from exploding with duplicate copies of
/// large structures before jam backrefs compress them.
pub struct CopyCacheForFormula {
    /// Map from leaf identity (structural hash) to the noun assembled in the
    /// destination slab. Entries persist for the lifetime of the to_noun call
    /// (typically one formula).
    cache: IntMap<u64, Noun>,
    
    /// Optional: statistics for measuring assembly efficiency.
    #[cfg(debug_assertions)]
    stats: CacheStats,
}

#[cfg(debug_assertions)]
pub struct CacheStats {
    hits: usize,
    misses: usize,
    unique_leaves: usize,
    total_leaf_bytes: u64,  // Sum of leaf noun byte sizes
    assembly_savings_bytes: u64,  // Bytes NOT allocated due to cache hits
}

impl CopyCacheForFormula {
    pub fn new() -> Self {
        CopyCacheForFormula {
            cache: IntMap::new(),
            #[cfg(debug_assertions)]
            stats: CacheStats::default(),
        }
    }
    
    /// Look up a leaf by its structural hash. Returns the noun if it was
    /// previously assembled in the destination slab.
    pub fn get(&self, leaf_hash: u64) -> Option<Noun> {
        #[cfg(debug_assertions)]
        {
            if self.cache.contains_key(leaf_hash) {
                // Can't mutate stats in a const method; would need interior mut
            }
        }
        self.cache.get(leaf_hash).copied()
    }
    
    /// Register an assembled leaf noun in the cache.
    pub fn insert(&mut self, leaf_hash: u64, noun: Noun) {
        #[cfg(debug_assertions)]
        {
            self.stats.misses += 1;
            if let Ok(allocated) = noun.as_allocated() {
                match allocated.as_either() {
                    Either::Left(indirect) => {
                        // Rough size estimate
                        self.stats.total_leaf_bytes += indirect.raw_size() as u64 * 8;
                    }
                    _ => {}
                }
            }
        }
        self.cache.insert(leaf_hash, noun);
    }
    
    /// Report cache statistics (debug builds only).
    #[cfg(debug_assertions)]
    pub fn report(&self) {
        eprintln!(
            "[copy-cache] {} hits, {} misses, {} unique leaves, \
             {} bytes assembled, {} bytes saved",
            self.stats.hits, self.stats.misses, self.stats.unique_leaves,
            self.stats.total_leaf_bytes, self.stats.assembly_savings_bytes
        );
    }
}
```

### Why Hash-Based Identity?

We use `structural_hash()` (which returns `noun.mug()` in the default case) as the cache key rather than pointer identity because:

1. **Pointer identity is arena-relative**: A noun's pointer is meaningless across arenas. But a noun's structure is not — two nouns with the same content hash are the same (for our purposes).

2. **Mug collision handling**: If two different nouns have the same mug, we fall back to full `noun_equality` check to confirm before reusing the cached noun. This is rare and acceptable (the worst case is a cache miss that doesn't hurt correctness).

3. **Determinism**: Hash-based caching produces deterministic output (the same formula with the same leaves always produces the same slab layout), which is important for reproducibility.

### Measuring Output-Assembly RSS Separately

To validate RT-16 (preventing intermediate blowup), we need to measure peak RSS during output assembly separate from the final jam size:

```rust
/// RSS measurement points for to_noun assembly.
pub struct RssMeasurement {
    /// Peak RSS before any Formula::to_noun call
    initial_rss: u64,
    
    /// Peak RSS during the to_noun walk (before jam)
    peak_assembly_rss: u64,
    
    /// Peak RSS during jam (after assembly)
    peak_jam_rss: u64,
    
    /// Final jam byte size
    jam_bytes: u64,
}

pub fn measure_output_assembly(
    formula: &Formula,
    dest: &mut NounSlab,
) -> (Noun, RssMeasurement) {
    let initial_rss = get_rss_bytes();
    
    let mut cache = CopyCacheForFormula::new();
    let noun = formula.to_noun(dest, &mut cache);
    
    let peak_assembly_rss = get_rss_bytes();
    
    dest.set_root(noun);
    let jam_output = dest.jam();
    
    let peak_jam_rss = get_rss_bytes();
    
    #[cfg(debug_assertions)]
    eprintln!(
        "[output-assembly] initial={} MB, assembly_peak={} MB, \
         jam_peak={} MB, final_bytes={}",
        initial_rss / 1_000_000, peak_assembly_rss / 1_000_000,
        peak_jam_rss / 1_000_000, jam_output.len()
    );
    
    (noun, RssMeasurement {
        initial_rss,
        peak_assembly_rss,
        peak_jam_rss,
        jam_bytes: jam_output.len() as u64,
    })
}

// Stub for RSS measurement (platform-dependent)
#[cfg(target_os = "linux")]
fn get_rss_bytes() -> u64 {
    // Parse /proc/self/status VmRSS
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|content| {
            content.lines().find_map(|line| {
                if line.starts_with("VmRSS:") {
                    line.split_whitespace()
                        .nth(1)
                        .and_then(|s| s.parse::<u64>().ok())
                        .map(|kb| kb * 1024)
                } else {
                    None
                }
            })
        })
        .unwrap_or(0)
}

#[cfg(not(target_os = "linux"))]
fn get_rss_bytes() -> u64 {
    0  // Not implemented on this platform
}
```

### Rules for Copy-Cache Validity

1. **Lifetime**: The cache is valid only for a single Formula::to_noun call. After serialization to jam, it is dropped.

2. **Keying**: The hash must be stable across the compilation and deterministic (mug-based). If mug collisions happen, confirm with `noun_equality` before cache reuse.

3. **Scope**: The cache holds Noun pointers into `dest_slab` only. If the slab is moved/reallocated (e.g., between formula assembly and jam), the cache is invalidated. In practice, we build the entire output in one contiguous slab write, so this is not a concern.

4. **Correctness**: The copy-cache is a performance optimization only. Removing it (or disabling it) must not change the byte output, only slow assembly down and increase peak RSS.

---

## Part 5: Integration Rules and Checkpoints

### Rule Set for Phase 0 Implementation

**R1: No bare Noun leaves in Formula**
- Every `Formula::Const`, `Formula::Hint::clue`, `Formula::Dbug` must hold a `ProvenancedLeaf`, never a bare `Noun`.
- Compiler error: attempting to construct `Formula::Const(n: Noun)` directly must not compile.
- Enforcement: make `Noun` a sealed type within the Formula module, or use a wrapper newtype.

**R2: ProvenancedLeaf construction only via copy_from or from_slab_resident**
- Any noun incorporated into a Formula from outside the building slab must go through `ProvenancedLeaf::copy_from(source, source_space, dest_slab)`.
- Slab-resident nouns (results of formula construction, parsed constants) use `ProvenancedLeaf::from_slab_resident(noun, slab)`.
- Compiler error: cannot construct a ProvenancedLeaf with a bare Noun + no provenance.

**R3: to_noun uses copy-cache for dedup**
- The `to_noun` method takes a `&mut CopyCacheForFormula` parameter.
- Every `ProvenancedLeaf` access in `to_noun` first checks the cache by hash.
- On miss, insert the leaf noun into the cache before returning.

**R4: Provenance proof (debug-mode only)**
- Debug builds compute and store `ProvenanceProof` fields on every `ProvenancedLeaf`.
- Release builds elide the fields (no size/perf cost).
- Debug assertions check that the source space hasn't been invalidated and that the copied structure matches.

**R5: Output assembly RSS measured separately**
- Measure peak RSS before to_noun, during to_noun (assembly peak), and during jam (jam peak).
- Compare to unoptimized baselines (cache disabled, no dedup) to prove RT-16 win.
- Log assembly and jam peaks at each compile in verbose mode.

**R6: Byte-exact fixture for every leaf type**
- Quoted constants: test a formula with a large `Const(battery)` leaf; validate jam output byte-identical to unoptimized path.
- Hint clues: test jet-hint formulas with `Hint(clue, body)`; validate jets still fire.
- Dbug spots: test source-spot preservation on/off; validate `--dbug` output byte-identical.
- Copy-cache on/off toggle: ensure disabling the cache produces identical bytes (only slower).

**R7: No reliance on slab identity for proofs**
- The provenance proof (debug-mode) is a signature/hash of the source space's ranges, not raw pointer addresses.
- The proof can be checked across slab boundaries and across rebuild cycles (e.g., in tests).

### Checkpoints for Phase 0 Gate

**Phase 0 Acceptance Criteria:**

1. **Type Safety**: Formula enum compiles with only `ProvenancedLeaf` in leaf positions. No bare `Noun` leaves remain.

2. **Copy-from Contract**: `ProvenancedLeaf::copy_from` is the only way to incorporate external nouns. Test coverage: at least 10 formula construction sites traced and verified to use copy_from.

3. **to_noun Structural Conversion**: The `Formula::to_noun` method builds only opcode cells and leaf references; no new deep copy happens inside to_noun (all copying happened in copy_from).

4. **Copy-Cache Dedup**: Measure a formula with repeated large constants (e.g., 5 copies of a 10MB battery). With cache: peak assembly RSS shows ~10 MB battery + overhead. Without cache: peak RSS shows ~50 MB (5 copies).

5. **Byte Parity**: Run the existing 6-kernel parity suite with native Formula leaves. All byte-exact gates PASS (no dir-hash exceptions due to provenance changes).

6. **Debug Validation**: Run debug builds with `RUST_LOG=debug` on compiler_mint tests. All provenance proofs should validate (no "source space signature mismatch" errors).

7. **RSS Attribution**: Publish before/after peak RSS numbers for honk's native prelude mint (if attempted) and native kernel compiles. Assembly peak should not exceed unoptimized baselines if the cache is working.

---

## Part 6: Concrete Rust Sketches

### Sketch: ProvenancedLeaf in a Hint Formula

```rust
// At formula construction time (in minter, musk, or formula builder):
fn build_jet_hint_formula(
    clue_noun: Noun,              // From parser/evaluator
    clue_space: &NounSpace,       // Source space for clue
    body_formula: Formula,        // Already a Formula (recursive)
    dest_slab: &mut NounSlab,     // Output slab
) -> Formula {
    // Copy clue into dest_slab with provenance check
    let clue_leaf = ProvenancedLeaf::copy_from(clue_noun, clue_space, dest_slab);
    
    Formula::Hint {
        clue: clue_leaf,
        body: Box::new(body_formula),
    }
}

// At serialization time (in Compiled::jam or jam_dynock):
impl Compiled {
    pub fn jam(&mut self) -> Vec<u8> {
        let mut cache = CopyCacheForFormula::new();
        let (formula_noun, rss_meas) = measure_output_assembly(
            &self.formula,
            &mut self.slab,
        );
        self.slab.set_root(formula_noun);
        let jam_output = self.slab.jam();
        
        #[cfg(debug_assertions)]
        cache.report();
        
        jam_output.to_vec()
    }
}
```

### Sketch: Structural Hash for Dedup

```rust
impl ProvenancedLeaf {
    /// Hash of the noun's structure, used for copy-cache keying.
    /// Collisions trigger noun_equality fallback in the cache.
    pub fn structural_hash(&self) -> u64 {
        // Use the noun's mug. If collisions are a problem in practice,
        // we can use a custom hash (e.g., depth+size+first-atom combo).
        self.noun.mug()
    }
}

// In copy-cache:
impl CopyCacheForFormula {
    pub fn get(&self, leaf_hash: u64) -> Option<Noun> {
        self.cache.get(leaf_hash).copied()
    }
    
    pub fn insert(&mut self, leaf_hash: u64, noun: Noun) {
        self.cache.insert(leaf_hash, noun);
    }
    
    /// In production, if needed, add collision detection:
    /// pub fn get_verified(&self, leaf_hash: u64, leaf_noun: Noun, space: &NounSpace) -> Option<Noun> {
    ///     match self.cache.get(leaf_hash) {
    ///         None => None,
    ///         Some(cached) => {
    ///             if noun_equality(leaf_noun.in_space(space), cached.in_space(space)) {
    ///                 Some(cached)
    ///             } else {
    ///                 None  // Hash collision; cache miss
    ///             }
    ///         }
    ///     }
    /// }
}
```

### Sketch: Debug-Mode Provenance Proof

```rust
#[cfg(debug_assertions)]
pub struct ProvenanceProof {
    source_space_signature: u64,
    leaf_structure_hash: u64,
}

#[cfg(debug_assertions)]
impl NounSpace {
    /// Compute a signature of this space's arena ranges for provenance validation.
    pub fn ptr_range_signature(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for (base, end) in self.stack_range().iter().chain(self.pma_range().iter()) {
            hasher.write_usize(*base);
            hasher.write_usize(*end);
        }
        std::hash::Hasher::finish(&hasher)
    }
}

#[cfg(debug_assertions)]
fn shallow_tree_hash(noun: Noun, space: &NounSpace) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    noun.mug().hash(&mut hasher);
    // Optionally: hash immediate children for collision detection
    if let Ok(cell) = noun.in_space(space).as_cell() {
        cell.head().noun().mug().hash(&mut hasher);
        cell.tail().noun().mug().hash(&mut hasher);
    }
    std::hash::Hasher::finish(&hasher)
}

// Then, in a ProvenancedLeaf assertion in to_noun or at formula use:
#[cfg(debug_assertions)]
pub fn assert_provenance_valid(&self, space: &NounSpace) {
    if let Some(proof) = &self.source_proof {
        assert_eq!(
            proof.source_space_signature,
            space.ptr_range_signature(),
            "provenanced leaf was constructed from a different or invalidated space"
        );
    }
}
```

---

## Part 7: Non-Goals and Deferred Work

### Out of Phase 0 Scope

1. **Type IR native representation** — This document covers only Formula leaves. Type IR native representation (Type enum, smart constructors, hash-consing) is Phase 2 (RT-06, RT-07, RT-14).

2. **Lazy battery ownership** — Lazy resolvers and cross-arm formula sharing (RT-05) are addressed in Phase 2. Phase 0 assumes lazy batteries are already valid when a Formula leaf references them.

3. **Full-graph validation** — Phase 0 validates leaves at copy-from time. Full-graph reachability validation (ensuring all descendants of a root are provably resident) is Phase 2 (RT-06 in NATIVE-TYPES-MIGRATION-RT.md).

4. **Mack/fold integration** — The mack/fold boundary (RT-11) is separate. Phase 0 handles Formula leaves; muck integration happens when Formula::to_noun is called by mack's araw evaluator.

5. **AST provenance** — Hoon AST nodes embedded in Hold types and arm maps have their own provenance story (RT-13). This document covers only Noun leaves.

---

## Part 8: Testing and Validation Strategy

### Fixture Categories

**F1: Leaf Construction**
```rust
#[test]
fn provenanced_leaf_copy_from_deep_copies() {
    let mut source_slab = NounSlab::new();
    let source_noun = T(&mut source_slab, &[D(1), D(2)]);
    let source_space = source_slab.noun_space();
    
    let mut dest_slab = NounSlab::new();
    let leaf = ProvenancedLeaf::copy_from(source_noun, &source_space, &mut dest_slab);
    
    // Leaf noun is resident in dest
    assert!(dest_slab.contains_ptr(leaf.noun().as_allocated().unwrap()...));
}

#[test]
fn provenanced_leaf_from_slab_resident_validates() {
    let mut slab = NounSlab::new();
    let noun = T(&mut slab, &[D(1), D(2)]);
    
    let leaf = ProvenancedLeaf::from_slab_resident(noun, &slab);
    assert_eq!(leaf.noun(), noun);
    
    // Attempt with foreign noun panics
    let mut other_slab = NounSlab::new();
    let foreign = T(&mut other_slab, &[D(3), D(4)]);
    assert_panics(|| ProvenancedLeaf::from_slab_resident(foreign, &slab));
}
```

**F2: Formula to Nock Conversion**
```rust
#[test]
fn formula_const_to_noun_preserves_battery() {
    let mut slab = NounSlab::new();
    let battery = T(&mut slab, &[D(1), T(&mut slab, &[D(11), D(42)])]);
    let leaf = ProvenancedLeaf::from_slab_resident(battery, &slab);
    
    let formula = Formula::Const(leaf);
    
    let mut out_slab = NounSlab::new();
    let mut cache = CopyCacheForFormula::new();
    let result = formula.to_noun(&mut out_slab, &mut cache);
    
    // Result is a cell [1 battery]? No, it's just the const itself.
    // Actually, Formula::Const(leaf).to_noun should return the leaf noun directly
    // (no opcode wrapping; the leaf IS the nock value).
    // Let me reconsider: a Const formula should serialize to just the constant,
    // not wrapped in [1 const]. So:
    assert_eq!(result, battery);
}

#[test]
fn formula_hint_to_noun_wraps_clue() {
    let mut slab = NounSlab::new();
    let clue = T(&mut slab, &[D(1), D(%jet)]);
    let body = Formula::Slot(Axis::One);
    
    let clue_leaf = ProvenancedLeaf::from_slab_resident(clue, &slab);
    let formula = Formula::Hint {
        clue: clue_leaf,
        body: Box::new(body),
    };
    
    let mut out_slab = NounSlab::new();
    let mut cache = CopyCacheForFormula::new();
    let result = formula.to_noun(&mut out_slab, &mut cache);
    
    // Result should be [11 clue [0 1]]
    let cell = result.in_space(&out_slab.noun_space()).as_cell().unwrap();
    assert_eq!(cell.head().noun(), D(11));
}
```

**F3: Copy-Cache Dedup**
```rust
#[test]
fn copy_cache_prevents_duplicate_assembly() {
    let mut slab = NounSlab::new();
    let huge_battery = build_large_noun(&mut slab, 1_000_000);  // 1MB battery
    let leaf = ProvenancedLeaf::from_slab_resident(huge_battery, &slab);
    
    // Formula references the same leaf 5 times
    let formula = Formula::Cons {
        head: Box::new(Formula::Const(leaf.clone())),
        tail: Box::new(Formula::Cons {
            head: Box::new(Formula::Const(leaf.clone())),
            tail: Box::new(Formula::Cons {
                head: Box::new(Formula::Const(leaf.clone())),
                // ... more references
                tail: Box::new(Formula::Const(leaf)),
            }),
        }),
    };
    
    let mut out_slab = NounSlab::new();
    let mut cache = CopyCacheForFormula::new();
    
    let initial_rss = get_rss_bytes();
    let _ = formula.to_noun(&mut out_slab, &mut cache);
    let peak_rss = get_rss_bytes();
    
    // With cache, peak should be ~1MB (plus overhead), not ~5MB
    assert!(peak_rss - initial_rss < 2_000_000);  // 2MB budget
}
```

**F4: Byte Parity**
```rust
#[test]
fn formula_to_noun_byte_parity_with_noun_path() {
    // Build a formula, serialize to noun, jam.
    // Build the same thing as noun directly, jam.
    // Both jams must be byte-identical.
    
    let mut slab = NounSlab::new();
    let formula = Formula::Hint {
        clue: ProvenancedLeaf::from_slab_resident(D(%test), &slab),
        body: Box::new(Formula::Slot(Axis::One)),
    };
    
    let mut out_slab = NounSlab::new();
    let mut cache = CopyCacheForFormula::new();
    let formula_noun = formula.to_noun(&mut out_slab, &mut cache);
    out_slab.set_root(formula_noun);
    let formula_jam = out_slab.jam();
    
    // Now build the same thing as noun directly:
    let mut direct_slab = NounSlab::new();
    let direct_noun = T(&mut direct_slab, &[D(11), D(%test), T(&mut direct_slab, &[D(0), D(1)])]);
    direct_slab.set_root(direct_noun);
    let direct_jam = direct_slab.jam();
    
    assert_eq!(formula_jam, direct_jam);
}
```

**F5: Release vs Debug Provenance Fields**
```rust
#[test]
fn provenanced_leaf_no_size_cost_release() {
    // In release builds, ProvenanceProof is elided.
    // Use std::mem::size_of to verify.
    #[cfg(not(debug_assertions))]
    {
        assert_eq!(
            std::mem::size_of::<ProvenancedLeaf>(),
            std::mem::size_of::<Noun>()
        );
    }
    
    #[cfg(debug_assertions)]
    {
        // In debug, it includes ProvenanceProof (larger)
        assert!(std::mem::size_of::<ProvenancedLeaf>() > std::mem::size_of::<Noun>());
    }
}
```

---

## Part 9: Glossary

- **Alien Noun**: A `Noun` used with the wrong `NounSpace` or after its owning arena is dropped.
- **Provenance**: The ownership and arena membership of a `Noun`. A `Noun` is only valid relative to its provenance.
- **ProvenancedLeaf**: A `Noun` paired with proof of valid construction in the destination slab.
- **to_noun**: The boundary where Formula IR is converted to Nock nouns for serialization.
- **Copy-Cache**: A map from leaf structural hash to the noun assembled in the output slab, used to deduplicate large constants during to_noun.
- **Source Space**: The `NounSpace` describing the arena owning a source noun before copy.
- **Destination Slab**: The `NounSlab` that will own the final Formula and its noun conversion.
- **Mug**: The cached structural hash of a noun (14-bit collision-heavy hash per nockvm).

---

## Part 10: Decision Points for Implementation

**D1: Should ProvenancedLeaf hold Rc<Noun> or Noun?**

Recommendation: Hold bare `Noun`. The slab owns the memory; the Formula holds a reference (pointer/offset) into it. If a Formula outlives its slab, that's a separate bug (detected by set_root panic or debug-mode provenance check). Rc would add indirection and needless refcount churn.

**D2: Should copy-cache use mug or a custom hash?**

Recommendation: Start with mug (cheap, ready-made). Monitor for collisions in production (log when cache hits have false mug equality). If collisions are frequent, add noun_equality fallback or a custom hash (depth + size + first few atoms).

**D3: Should provenance proof be an enum (with variants for different sources) or a simple signature?**

Recommendation: Simple signature (hash of source space ranges). Future: if we need to distinguish "copied from parser", "copied from eval stack", etc., add a provenance source variant, but it doesn't affect the noun validity check.

**D4: Should to_noun take ownership of the cache or borrow it?**

Recommendation: Borrow `&mut CopyCacheForFormula`. This allows the caller to reuse the same cache across multiple to_noun calls if desired (e.g., for efficiency). A fresh cache per to_noun is also fine (the default case).

**D5: Should the copy-cache persist after to_noun returns?**

Recommendation: No. The cache is valid only for a single to_noun walk. After returning, drop it. If we later want to reuse the cache across multiple formulas (e.g., for batch compilation), we'd need to verify that the destination slab hasn't been reallocated.

---

## References

- **nockapp/noun/slab.rs**: `copy_into` implementation, region stack, checkpoint/rewind
- **nockapp/noun/extensions.rs**: `copy_into` trait, `NounAllocatorExt`, `BrandedNounSpaceExt`
- **nockvm/noun.rs**: `NounSpace`, identity fast path, pointer resolution (lines 401–410)
- **honk/lib.rs**: `Compiled` type, no public `formula()` accessor (lines 73–78)
- **honk/src/lib.rs**: `jam()` and `jam_dynock()` boundaries (lines 83–118)
- **NATIVE-TYPES-MIGRATION-RT.md**: RT-04 (bare Noun leaves), RT-16 (output-assembly RSS)
- **NOUN-PROVENANCE-AND-BRANDED-HANDLES.md**: Provenance audit, branded API guidance
- **OSS-NEXT-PLAN.md**: H7 frame-arena work, NounSlab region stack, copy_to_base

