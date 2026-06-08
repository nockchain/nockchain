# Current status

> Historical low-level design note. For current public-release guidance, prefer [`DESIGN.md`](./DESIGN.md), [`DURABILITY-OPERATIONS.md`](./DURABILITY-OPERATIONS.md), and [`NOUN-PROVENANCE-AND-BRANDED-HANDLES.md`](./NOUN-PROVENANCE-AND-BRANDED-HANDLES.md).

The live nockvm still runs on the contiguous arena defined in `open/crates/nockvm/rust/nockvm/src/mem.rs`. That module owns the `Memory` abstraction (wrapping either `memmap2::MmapMut` or `malloc`) and the `NockStack`, a single slab that tracks `frame_offset`, `stack_offset`, and `alloc_offset` as word counts off of a base pointer. Every noun the VM manipulates lives inside that slab until it is explicitly copied into a PMA image.

- `Memory::allocate` chooses mmap vs. malloc and hands back a base pointer that stays immutable for the life of the VM; all in-VM pointers today are literal `base + offset` derivations performed via `derive_ptr()`, not tagged offsets.

- `NockStack` models both stack frames and bump-allocation via its `west`/`east` orientation flag, the `AllocationType` enum, and the `pc` (pre-copy) bit that gates when frame flipping or preservation can occur. The reserved slots at the bottom of each frame cache the previous frame/stack/alloc pointers so that copying collectors and frame pops can restore provenance without chasing raw pointers.

- `open/crates/nockvm/rust/nockvm/src/noun.rs` consumes the stack API through `NounAllocator`, layering
the tag scheme (direct vs. indirect atoms vs. cells) and the forwarding-pointer rules that keep
structural sharing intact while slabs are copied between frames or into the PMA. Helper modules such
as `jets.rs` and `flog.rs` lean on `Preserve`/`preserve_with` from `mem.rs` to ensure nouns stay pinned
during host callbacks.

- We have not yet switched the runtime over to offset-tagged references: any noun reloaded from a
persisted PMA still has to be patched up by rerunning `derive_ptr()` with the process-local base pointer.

## A Young System's Programmer's Primer

1. Read [https://doc.rust-lang.org/nomicon/](https://doc.rust-lang.org/nomicon/) and [https://blog.regehr.org/archives/213](https://blog.regehr.org/archives/213)
2. Meditate on the most vivid possible meaning of the "nasal demons" metaphor for undefined behavior and let it put the fear of God in you
3. Miri is enabled on every test except those it absolutely cannot be made to work for (hi tokio, ffi). If the test executes too slowly in Miri, your test is too slow. Make it faster or more "targeted" to the code coverage you need from Miri.

Relevant history:

## Epoch History

1. 2025-03-28: PR #1167, titled “Offsets, not aliasing” and authored by Chris Allen (`@bitemyapp`).
  - This branch (commit e4adb5a8c, 2025‑03‑28) is where the NockStack struct stopped storing live frame/stack/alloc raw pointers and instead began recording `frame_offset`, `stack_offset`, and `alloc_offset` word counts from the slab’s base pointer. The change also introduced `derive_ptr()`/`frame_pointer()` helpers so every access reconstructs a pointer from the base plus offset, and `MemoryState` now snapshots offsets instead of raw pointers (see history at commit e4adb5a8c affecting `open/crates/nockvm/rust/nockvm/src/mem.rs`)
2. 2025‑05‑19 · commit 00d288b1 · PR #1554 “Incremental hierarchy for hoonc”
  - Focused on reducing allocator overhead when running hoonc by (a) allowing builds to short-circuit
  OOM checks via a `no_check_oom` feature, (b) dropping the expensive assert_no_alloc::permit_alloc
  scaffolding around pointer validity checks, and (c) rewriting prev_alloc_offset() to use a single
  wrapping_sub instead of branching on the base pointer. Also simplified frame_pop’s null-pointer
  panic to avoid heap allocations. Net effect: the stack allocator became leaner and more predictable
  under hoonc's incremental compile workload.
3. 2025‑05‑26 · commit a61d3289 · PR #1664 “Least space metric”
  - Added the `least_space` field to `NockStack`, threaded it through initialization, resets, and frame
  flips, and updated both `west`/`east` allocation paths to maintain a running low-water mark. Exposed
  a `least_space()` accessor so the runtime could export a gauge of minimum free words/bytes, enabling
  Slam telemetry to flag stacks that are close to exhaustion.
4. 2025‑06‑27 · commit d01347fd · branch “test jets vs hoon” (squash merge)
  - Extended the `Preserve` trait with a trivial implementation for `()`, which let the jets test harness
  reuse preservation APIs without manufacturing dummy nouns. Small change, but it marked the first
  divergence where preservation logic needed to tolerate no-op placeholders.
5. 2025‑07‑01 · commit 0013e50e · branch “Fix rust formatting in open/”
  - Pure rustfmt/rust-analyzer cleanup of `mem.rs`: reordered use statements into the standard blocks
  (std → crates → local) so the file complied with workspace formatting rules. No behavioral changes,
  but it stabilized future diffs for readability.
6. 2025‑07‑24 · commit b6ebdc7a · branch “Tracing backends integration”
  - Touched `frame_pop` and the debugging walkers to move from format!-style placeholders to the new
  Rust inline formatting (`{ptr:p}`). This kept panic/log strings allocation-free and aligned with the
  tracing backend expectations while keeping the underlying mechanics unchanged.
7. 2025‑09‑24 · commit 68b40a80 · branch “gRPC public API / light wallet”
  - Updated the `NounAllocator` for `NockStack` impl so that callers using the allocator through the trait could invoke a new `equals()` hook. Under the hood it forwards to `crate::unifying_equality::unifying_equality`, ensuring components like the light-wallet gRPC service can compare nouns without downcasting to `NockStack`.
8. 2025‑10‑06 · commit c809688f · branch “hoonc benchmarking and prewarm best result”
  - Largest post-introduction refactor:
    * Marked `word_size_of` as #[inline] and pulled in `Vec` to support a heap-based worklist.
    * Promoted `frame_push` to pub fn and inlined pop/top helpers for tighter hot-path codegen.
    * Replaced the heavyweight NockStack::copy method (which reused the lightweight stack as a
    worklist) with a new noun_preserve free function that uses a Vec<(Noun, *mut Noun)>. The new
    routine bails early when the root is already direct, already forwarded, or outside the current
    frame, dramatically reducing the amount of stack flipping during hoonc prewarm. Preservation
    invariants (assert_acyclic, assert_no_forwarding_pointers, assert_no_junior_pointers) still bookend
    the operation, but the worklist logic now lives entirely off-stack, improving determinism when the
    allocator is hot.
    * There was recently a weird interaction between the `axis` vs. `axis.form` issue and hoonc's prewarm bootstrap, at time of writing I'm not totally clear on how the dust settled there but I don't remember Logan saying prewarm in-and-of-itself was implicated, just flagging a risk.

This foregoing history exemplifies the recent substantive architecture epochs for `mem.rs`:
- initial migration from sword
- partial offset-ification of NockStack indexes into the slab (cf. `e4adb5a8c`)
- perf hardening for hoonc
- observability of stack pressure
- and the more recent noun-preservation rewrite that decouples maintenance work from the lightweight stack. - Subsequent commits are mostly ergonomic or integration tweaks layered on that foundation.

# Tooling, debugging, profiling

Make sure your build and test/validation entrypoints you use to iterate on your work are batch-executable (meaning: not daemonized/persistent) Makefile entrypoints that "just work" out of the box with no additional steps required before-hand to make them complete successfully.

- Memory safety, segfaults, use-after-frees, etc.:

  * ASAN on Linux (ask `@bitemyapp` how but the repo Makefile has breadcrumbs of me doing this)
  * guard malloc on macOS ^^

- Memory leaks:

  * For Linux, I recommend `bytehound`. Same suggestions as the previous, there are breadcrumbs but ask me how. You'll probably need to flip the `Memory` type to using an ephemeral malloc if the problem you are diagnosing implicates the slab. You will think you have alternatives to `bytehound` for diagnosing leaks on Linux and I will be very surprised if that's true. You'll probably waste your time trying to find something better, I was not successful after many hours. If you find something nicer or better maintained please let me know.

  * macOS: Just use `cargo-instruments` and XCode. Frankly easier than Linux but might be slightly less informative/clear/precise than `bytehound` depending on your circumstances. Seems to work great for Mitchell Hashimoto across the board, idk man. I need to spend more time on it.

- Performance:
  * Cheap and cheerful, works for Linux and macOS, samples native runtime stacks: `samply record make run-my-benchmark-or-whatever`, you'll have to clear detritus threads from Cargo in the Firefox Profiler tab that spawns if the benchmark wasn't already built but the actual benchmark threads should be in there somewhere regardless.
  * _There is no legacy tracing JSON profiling for Nock in NockVM_. If there is, I simply forgot to merge the branch deleting it. If it exists, delete it. Please don't use it. Tracy subsumes any need for this and it was wasting bytes and developer time.
  * There, however, is _tracing for Nock in NockVM_: use tracy. [watch my youtube video](https://www.youtube.com/watch?v=Z1UA0SzZd6Q)
  * Linux only unified nock + native stack profiling: tracy profiler again. make sure you align the locally compiled version of the tracy profiler GUI and the library version in the Cargo project. Look at the repo makefile.
    - `macOS` works with Tracy fine, client and server-side but you're going to get the nock traces by default, native (20 khz!) stack samples in Tracy only work on Windows and Linux. No, I don't know why they refuse to support it on macOS. Because they definitely could. They just choose not to. This has a solution: _use Docker_ (do I need to say it again? Look at the Makefile targets and Dockerfile I wrote for this)
    - I would strongly encourage you to take advantage of `tracy`'s `ondemand` mode (look at the Cargo features specified for `tracing-tracy`) so that you aren't eating the profiling overhead when the nockapp first boots and loads the slab, but I won't blame you if that's more faffing about than you have patience for.

# Writing tests gooder

This is all speaking to Rust norms and structural conventions. I don't care what Uncle Bob thinks an integration test is. Don't tell me, I don't want to know either.

## Unit tests live in the library modules

## Integration tests live in separate binaries

You see all the Rust test cases in `tests/` sub-directories? That's what makes them an integration test. Importantly, _you can have multiple test cases in a single integration test binary_. Too many integration test binaries increase linker surface area, please don't exacerbate that.

Reasons you'd use an integration test:
# Milestone 1: Offset-addressing

You need to have a pointer representation that can be used from the Nock code which addresses other objects as offsets from a static (not constant) base pointer (base address).

Some of this work happened in Chris's earlier offset branch, but it isn't complete and we're still leaning heavily on `derive_ptr()` because Chris didn't want to add a new tag bit or churn the rest of the runtime. That time has passed and we need to rip the bandaid off and finish the other 80% of the work now to set the stage for position-independent addressing for a persistent mmap slab.

We're going to mmap the PMA, let the system decide where the map the PMA, let the system decide the base address.

The base address is universal and singular to the PMA slab used for the nockvm instance.

The NockVM runtime will still be using direct pointers constructed using base address + offset arithmetic to dereference Noun nodes in the PMA. However, the PMA itself will work purely in terms of position-independent addressing which is all offset based so that if you reload the mmap-based PMA slab from disk all the offsets are still valid and simply recalculated in terms of the new base address you got from the virtual memory subsystem of your platform.

We will need to use pointer tags to distinguish between pointers and offsets.

On a read you branch on the pointer tag bits and you variously:

- strip the tag bits, and dereference the resulting pointer
- strip the tag bits, add the offset to a base address, cast that into a pointer, and dereference that pointer

Discriminant is a single bit in the tag. Signifies whether the reference is in the PMA or in the nockstack noun slab. There's a separate discriminator bit already in the Noun representation for distinguishing Atoms, IndirectAtoms, and Cells. For our purposes, we care about values vs. references.

After you've established whether the tag bits signify whether a value is a direct pointer to a noun entrypoint or a PMA offset, you now need to to distinguish whether the

## Milestone 1 discriminant bits hypothetical diff

### Current status / before milestone 1

Before (current master) ― four discriminants only: direct atom, indirect atom, cell, forwarding pointer.

All allocated variants currently hold raw pointers produced by `NockStack::derive_ptr()` (`open/crates/nockvm/rust/nockvm/src/mem.rs`).

```rust
/// Mirrors the actual constants in noun.rs.
const DIRECT_MASK: u64 = !(u64::MAX >> 1); // 0x8000_0000_0000_0000
const DIRECT_TAG: u64 = 0;

const INDIRECT_MASK: u64 = !(u64::MAX >> 2); // 0xC000_0000_0000_0000
const INDIRECT_TAG: u64 = u64::MAX & DIRECT_MASK; // pattern 10xx...

const CELL_MASK: u64 = !(u64::MAX >> 3); // 0xE000_0000_0000_0000
const CELL_TAG: u64 = u64::MAX & INDIRECT_MASK; // pattern 110x...

const FORWARDING_MASK: u64 = CELL_MASK;
const FORWARDING_TAG: u64 = u64::MAX & CELL_MASK; // pattern 111x...

#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Noun {
    raw: u64,
}

impl Noun {
    #[inline]
    fn tag_bits(self) -> u64 {
        match self.raw & DIRECT_MASK {
            DIRECT_TAG => DIRECT_TAG,
            _ => self.raw & CELL_MASK, // covers indirect/cell/forwarding
        }
    }

    #[inline]
    fn payload_bits(self) -> u64 {
        match self.tag_bits() {
            DIRECT_TAG => self.raw,                         // value <= DIRECT_MAX
            INDIRECT_TAG => self.raw & !INDIRECT_MASK,      // pointer to IndirectAtom
            CELL_TAG => self.raw & !CELL_MASK,              // pointer to CellMemory
            FORWARDING_TAG => self.raw & !FORWARDING_MASK,  // pointer to Allocated
            _ => unreachable!(),
        }
    }
}
```

This is exactly what the current noun representation does: a single u64 word whose top three bits
distinguish values, indirect atoms, cells, or transient forwarding pointers; every allocated case stores
a literal pointer to stack memory.

#### Sidebar about discriminant bits

Can we please just use a safer library for doing this instead of doing it by hand? There's no performance or clarity downside unless they're doing something dumb. It's an unforced error to keep doing this raw when we're in-progress on changing the design anyway.

Short‑list after surveying the current ecosystem (no code, just tradeoffs):

#### Before (just type tags: direct vs indirect vs cell vs forwarding)

- `bitflags` (1.4+): still the cleanest zero‑cost way to name the masks and expose helper methods. Gives
  you a readable bitflags! { struct NounTag: u64 { … } } and keeps the rest of the code close to what we
  already have, just without hand‑rolled constants.
- `bitfield-struct`: generates getters/setters for named bit ranges in a `repr(transparent)` wrapper. Useful
  if you want a tidy `struct NounBits { #[bits = 1] kind: u8, … }` but don’t want a macro DSL as heavy as
  `modular-bitfield`.
- If you want to stick with enums, `strum` + `num_enum::TryFromPrimitive` can encode the three tag states
  into an enum without rolling your own match ladder; it’s still zero cost once optimized. (in...theory. in practice I use bit-masking in some places so an enum could give me heartburn later.)

#### After (type tags + “stack pointer vs PMA offset” location bit)

- `bitflags` still works here, but pairing it with `bytemuck::TransparentWrapper` lets you define a
  `TaggedPtr(u64)` newtype and safely reinterpret between masks/payloads and raw words, which makes the
  pointer/offset split less error‑prone.
- `bitfield-struct` or `modular-bitfield` both shine once you have two orthogonal fields (kind + location).
  They emit getters that return plain integers, so you branch on location() without remembering which bit
  it lives in, and the generated code is just a few shifts/masks.
- For the pointer/offset arm specifically, tagged-pointer (crate) can encode the “pointer with spare
  high bits” case; you would still keep your own offset handling, but it gives you a typed wrapper with
  compile‑time guarantees that the high bit is reserved for tagging.
- If you’d rather treat the tag word as a mini struct, packed_struct lets you declare
  `#[packed_struct(bit_numbering = "msb0")] struct NounBits { #[packed_field(bits="0:0")] location: bool,
  … }` and the derive does the rest. Slightly heavier macro, but great when you need to document the
  layout inline.

Bottom line: for the current “before” layout, `bitflags` (optionally with a thin newtype) keeps things
minimal. Once you add the location bit in Milestone 1, stepping up to a `bitfield` derive (`bitfield-struct`, `modular-bitfield`, or `packed_struct`) or a purpose-built tagged-pointer wrapper gives you clearer semantics without runtime cost, and you can choose whichever macro style fits your tolerance for abstraction.

#### Validating which bitfield/bitflag crate to use

Assuming they're not messing up the target representation (make note of any applications of `repr` in `noun.rs` in the git history) or outright buggy, it should come down to perf/unforced overhead.

Get some basic operations (like the case discrimination helpers, chewing through IndirectAtoms of cords, etc.) lifted into a `criterion` benchmark harness, implement all the variations of the same minimal target representation with these verbs attached, and horse-race them with the benchmarks.

If the benchmarks confuse you grab `@bitemyapp` as he will greatly enjoy being confused with you. I'm not expecting them to be different unless the underlying representations are different.

Oh yeah, and write tests that verify the exact bit representation of the noun values for each tag-bit discriminant/scenario/type.

### Discriminant bits / Noun repr after Milestone 1, hypothetical diff

After (Milestone 1) ― same value/allocated/forwarding taxonomy, but add a location bit so we can
distinguish direct stack pointers from PMA-relative offsets. Reads first branch on the location bit, then
interpret the payload as either a raw pointer (stack slab) or a word offset to be rebased through the PMA
base pointer supplied by `NockStack`.

The distinction is between the nursery (not persistent, will get thrown away on a stack flip if not permanently allocated) and the persistent non-nursery part of the area (it survived nursery generation on a stack flip/preserve because it was permanently allocated). This distinction exists in the previous system but there was no "persistence" entailed in surviving the nursery and reaching the slab permanently.

```rust
const LOCATION_BIT: u64 = 1 << 60; // next free bit above CELL_MASK
const VALUE_MASK: u64 = !(DIRECT_MASK | LOCATION_BIT);

#[derive(Clone, Copy)]
enum PtrKind {
    StackPtr(*mut u8),
    PmaOffset(u32), // word index inside mmap’d PMA slab
}

impl Noun {
    #[inline]
    fn pointer_descriptor(self) -> Option<(u64 /* tag */, PtrKind)> {
        let tag = self.tag_bits();
        if tag == DIRECT_TAG {
            return None;
        }

        let payload = self.raw & VALUE_MASK;
        let ptr = if self.raw & LOCATION_BIT == 0 {
            PtrKind::StackPtr(payload as *mut u8)
        } else {
            PtrKind::PmaOffset(payload as u32)
        };

        Some((tag, ptr))
    }

    #[inline]
    fn resolve_cell<'a>(&self, base: *const u8) -> Option<&'a CellMemory> {
        let (tag, descriptor) = self.pointer_descriptor()?;
        if tag != CELL_TAG {
            return None;
        }

        match descriptor {
            PtrKind::StackPtr(ptr) => Some(unsafe { &*(ptr as *const CellMemory) }),
            PtrKind::PmaOffset(words) => {
                let ptr = unsafe { base.add((words as usize) << 3) } as *const CellMemory;
                Some(unsafe { &*ptr })
            }
        }
    }
}
```

This “after” block is the hypothetical partial diff you can paste into `NOCK-PMA.md`: it keeps the
exact bit patterns the runtime already relies on, but demonstrates how Milestone 1 splits the allocated
payload into “stack pointer” vs “offset into PMA,” which is the key new discriminant the doc needs to
communicate.


# Milestone 2: Persistence

Using mmap to persist to disk. We will be assuming only a single reader/writer
for now (Milestone 5 is concurrent reads).

This consists of two phases:

## Phase 1

Phase 1 is to separate out the NockStack from the arena.

```

  ┌──────────────────────┐    ┌──────────────────────┐
  │      NockStack       │    │         PMA          │
  │(ephemeral, anon mmap)│    │  (persistent, file)  │
  │                      │    │                      │
  │ [frames][stk→ ←alloc]│    │ [bump-allocated      │
  │                      │    │  nouns in offset     │
  │ Cleared after each   │    │  form]               │
  │ event                │    │                      │
  │                      │    │ Loaded at boot,      │
  │ Stack-pointer form   │    │ persisted to disk    │
  │ only                 │    │                      │
  └──────────────────────┘    └──────────────────────┘
           │                            ▲
           │   evacuate_to_pma()        │
           └────────────────────────────┘
```

We need to push the persistent arena to a memory slab that is bump-allocated at
the page level. As things stand now, NockStack lives in an anonymous mmap.

Currently, at the end of every event, NockVM is left with a single stack frame,
the top frame, and a bunch of data to be preserved - the kernel, jet states, and
cache. `preserve()` gets called on all of these, which copies them to the other
side of the memory arena, where then any Nouns that are in stack-pointer form
are retagged into offset form.

This step is to be replaced with a new copying step, into a file-backed mmap
called the persistent memory arena (PMA).

Phase 1 will be complete when data is being copied into the PMA at the
conclusion of each event, and NockStack works with references into the PMA for a
single writer/reader.

### Phase 1 spec

Here is a more detailed spec for phase 1:

The central struct for the PMA. `alloc_offset` uses `usize` for now since there
is only one reader/writer, but we will move to `AtomicUsize` when multiplayer
gets enabled.
```rust
/// The Persistent Memory Arena
///
/// A bump-allocated memory region for storing nouns in offset form.
/// The PMA is backed by a file (in future milestones) and persists across
/// program restarts.
///
/// "Bump-allocated" means allocation simply increments the `alloc_offset`
/// pointer by the requested size—there is no free list, no compaction, and
/// no mechanism to reclaim memory once allocated. This makes allocation
/// extremely fast (just a pointer bump) but means the PMA grows monotonically
/// until explicitly reset.
///
/// When a Noun that lives in the PMA needs to be modified, the workflow is:
/// 1. The Noun is read from the PMA (already in offset form)
/// 2. Modifications happen in the NockStack (ephemeral working memory)
/// 3. The modified Noun is copied back to the PMA via `copy_to_pma()`
///
/// Step 3 only allocates space for the Allocated subtrees that changed. For
/// example, if `[2 3]` becomes `[4 3]`:
/// - The Cell is Allocated, so a NEW cell is allocated in the PMA with head=4,
///   tail=3 with new DirectAtoms for the 4 and 3 since they are not Allocated.
/// - The old `[2 3]` cell remains in the PMA, untouched but now unreachable
///
/// For Allocated structures, unchanged subtrees that are already in PMA (offset
/// form) are reused without copying. If `[[1 2] 3]` becomes `[[1 2] 4]`:
/// - A NEW outer cell is allocated with tail=4
/// - The head still points to the existing `[1 2]` in PMA (no copy needed)
/// - Only the old outer cell becomes garbage; `[1 2]` is shared
///
/// This copy allocates fresh space in the PMA for the new version—the old
/// version is not overwritten or freed, it simply becomes unreachable garbage.
/// Garbage collection (Milestone 4) will eventually reclaim this dead space.
///
/// Currently Pma is only suitable for a single reader/writer. In the future,
/// `alloc_offset` will be changed to `AtomicUsize` to allow multiple readers.
pub struct Pma {
    /// The underlying arena for memory management and pointer resolution
    arena: Arc<Arena>,
    /// Current allocation offset in words (bump pointer)
    alloc_offset: usize,
    /// Path to the backing file (for future file-backed persistence)
    path: PathBuf,
}
```

As the `Pma` is a place where `Noun`s get allocated, it ought to implement
`NounAllocator`:
```rust
impl NounAllocator for Pma { ... }
```

There is a `PmaError` enum for `Result` types coming out of the PMA.
```rust
#[derive(Debug, Error)]
pub enum PmaError {
    #[error("PMA is full, cannot allocate {requested} words (available: {available})")]
    OutOfMemory { requested: usize, available: usize },

    #[error("PMA not installed in thread-local storage")]
    NotInstalled,

    #[error("Failed to create arena: {0}")]
    ArenaError(#[from] NewStackError),
}
```

Everything that lives in a NockStack that we'd like to live in the PMA
implements the `PmaCopy` trait:
```rust
pub trait PmaCopy {
    /// Copy this value into the PMA.
    ///
    /// For nouns, this evacuates allocated data (indirect atoms, cells) to the PMA
    /// and converts pointers to offset form. Direct atoms are unchanged since they
    /// fit in a single word.
    ///
    /// # Safety
    /// The caller must ensure that the stack's arena is installed in thread-local storage.
    unsafe fn copy_to_pma(&mut self, stack: &NockStack, pma: &mut Pma);

    /// Assert that this value is fully contained within the PMA.
    ///
    /// For nouns, this verifies that all allocated data (indirect atoms, cells)
    /// resides in the PMA. Direct atoms always pass since they have no allocations.
    ///
    /// # Panics
    /// Panics if any part of this value is not in the PMA.
    fn assert_in_pma(&self, pma: &Pma);
}
```

`PmaCopy` is implemented for the following types:
```rust
// nouns
impl PmaCopy for Noun { ... } // Calls copy_noun_to_pma below
// The rest of the Noun types probably just call .as_noun().copy_to_pma()
impl PmaCopy for Atom { ... }
impl PmaCopy for IndirectAtom { ... }
impl PmaCopy for DirectAtom { ... }
impl PmaCopy for Allocated { ... }
impl PmaCopy for Cell { ... }
// cache
impl<T: Copy + PmaCopy> PmaCopy for Hamt<T> { ... }
// jet state
impl PmaCopy for Warm { ... }
impl PmaCopy for WarmEntry { ... }
impl PmaCopy for Hot { ... }
impl PmaCopy for Batteres { ... }
impl PmaCopy for BatteriesList { ... }
impl PmaCopy for NounList { ... }
impl PmaCopy for Cold { ... }
```
I'm not sure about this one, but `Retag` is implemented for it so I've done it.
```rust
impl PmaCopy for () { ... } // Ctrl-F d01347fd for why this is implemented for Preserve. It also implements
                            // Retag which makes me think it probably will be.
```

The main function to accomplish copying to the PMA for `Nouns`. Something like
this:
```rust
impl PmaCopy for Noun {
    /// Copy a noun and all its allocated substructure to the PMA.
    ///
    /// Uses a worklist algorithm to avoid stack overflow on deep structures.
    /// Structural sharing is preserved via forwarding pointers: if the same
    /// substructure is referenced multiple times, it's only copied once.
    ///
    /// # Algorithm
    /// 1. Push (noun, destination_ptr) onto worklist
    /// 2. Pop and process each item:
    ///    - Direct atoms: write directly to destination
    ///    - Already in PMA (offset form): write directly to destination
    ///    - Has forwarding pointer: write forwarded offset-form to destination
    ///    - Indirect atom: copy to PMA, set forwarding pointer, write offset-form
    ///    - Cell: copy metadata to PMA, set forwarding pointer, queue head/tail
    ///
    /// # Safety
    /// - The PMA arena should be installed for reading evacuated nouns afterward
    /// - Source nouns will have forwarding pointers set (corrupting the stack data)
...
}
```

#### Tests
Summary of tests implemented:

```rust
    // Verifies bump allocation returns sequential offsets and correctly tracks free space.
    fn test_pma_allocation() { ... }
    // Verifies offset-to-pointer and pointer-to-offset conversions are inverses of each other.
    fn test_pma_offset_round_trip() { ... }
    // Verifies reset() clears the allocation pointer and reset_to() sets it to a specific offset.
    fn test_pma_reset() { ... }
    // Verifies thread-local PMA installation, access via with_current(), and cleanup via clear.
    fn test_pma_thread_local() { ... }
    // Verifies direct atoms are unchanged by evacuation since they fit in a single word.
    fn test_evacuate_direct_atom() { ... }
    // Verifies indirect atoms (too large for direct representation) are copied to PMA and converted to offset form.
    fn test_evacuate_indirect_atom() { ... }
    // Verifies a simple cell with direct atom contents is evacuated and readable from PMA.
    fn test_evacuate_simple_cell() { ... }
    // Verifies nested cell structures are fully evacuated with all sub-cells in offset form.
    fn test_evacuate_nested_cells() { ... }
    // Verifies cells containing indirect atoms have both the cell and atoms correctly evacuated.
    fn test_evacuate_with_indirect_atoms() { ... }
    // Verifies structural sharing is preserved: [x x] evacuates x only once, with both refs pointing to same PMA location.
    fn test_evacuate_shared_structure() { ... }
    // Verifies sharing is preserved across separate evacuate calls via forwarding pointers left in stack memory.
    fn test_evacuate_multiple_nouns_preserves_sharing() { ... }
    // Verifies evacuating an already-evacuated noun is a no-op that allocates nothing.
    fn test_evacuate_already_evacuated() { ... }
    // Verifies deeply nested structures are fully evacuated and traversable after evacuation.
    fn test_evacuate_deep_tree() { ... }
    // Verifies contains_ptr correctly identifies pointers inside vs outside the PMA memory region.
    fn test_pma_contains_ptr() { ... }
    // Verifies allocation fails gracefully when PMA is full, rolling back the failed allocation.
    fn test_pma_out_of_memory() { ... }
    // checks that allocating in PMA bumps the alloc ptr
    fn test_persistent_arena_allocation_is_monotonic() { ... }
    // checks NockStack is empty after moving noun to PMA,
    fn test_pma_preserve_moves_noun_and_resets_stack() { ... }
    // does a HAMT preserve work?
    fn test_preserve_hamt_round_trip()  { ... }
    // HAMT evacuate with Cells as values and IndirectAtoms as keys
    fn test_evacuate_hamt_complex_nouns() { ... }
    // jet state round trip tests
    fn test_evacuate_warm_round_trip() { ... }
    fn test_evacuate_warm_entry_round_trip() { ... }
    fn test_evacuate_hot_round_trip() { ... }
    fn test_evacuate_batteries_round_trip() { ... }
    fn test_evacuate_batteries_list_round_trip() { ... }
    fn test_evacuate_noun_list_round_trip() { ... }
    fn test_evacuate_cold_round_trip() { ... }
```
Tests not yet implemented:

##### Memory alignment and layout:
- `test_evacuate_indirect_atom_alignment` - Verifies indirect atoms of various sizes (1, 2, 3, 7, 8, 9 words)
 are properly aligned in PMA and readable without alignment faults.
- `test_evacuate_cell_memory_layout` - Verifies CellMemory fields (metadata, head, tail) are at correct
offsets after evacuation by reading each field independently.

##### Forwarding pointer edge cases:
- `test_forwarding_pointer_diamond_sharing` - Verifies diamond-shaped DAGs (A→B, A→C, B→D, C→D) preserve all
sharing and D is only copied once.
- `test_forwarding_pointer_wide_sharing` - Verifies a single noun referenced by many (e.g., 100) different
cells is only copied once.
- `test_forwarding_pointer_not_leaked_to_pma` - Verifies no forwarding pointers remain in PMA memory after
evacuation completes (they should only exist transiently in stack memory).

##### Boundary and edge cases:
- `test_evacuate_maximum_depth_tree` - Verifies evacuation handles very deep trees (e.g., 1000 levels)
without stack overflow in the worklist loop.
- `test_evacuate_large_indirect_atom` - Verifies indirect atoms near the maximum representable size evacuate
correctly.
- `test_evacuate_single_word_indirect_atom` - Verifies the smallest possible indirect atom (just over
DIRECT_MAX) evacuates correctly.
- `test_evacuate_mixed_pma_stack_noun` - Verifies a cell where head is already in PMA and tail is on stack
evacuates correctly (only tail gets copied).

##### Use-after-evacuation (Miri should catch these):
- `test_stack_memory_not_accessed_after_evacuation` - After evacuation, verify that reading the evacuated
noun uses PMA memory, not stack memory (may need to poison/zero stack to detect).
- `test_evacuate_then_pop_frame_then_read` - Evacuate a noun, pop the stack frame that contained it, then
read from the PMA copy - should work without accessing freed memory.

##### Concurrent allocation:
For now we are assuming the PMA has only a single writer/reader, so we won't
implement these tests, but they are listed for future reference.
- `test_concurrent_pma_allocation` - Spawns multiple threads that allocate from PMA simultaneously, verifies
no overlapping allocations and total allocated equals sum of individual allocations.
- `test_concurrent_allocation_under_pressure` - Multiple threads racing to allocate when PMA is nearly full,
verifies OOM errors are returned correctly without corruption.

##### Idempotency and repeated operations:
- `test_evacuate_same_noun_twice_same_call` - Passes the same noun pointer twice in succession; second call
should be pure no-op.
- `test_evacuate_after_pma_reset` - Evacuate, reset PMA, evacuate same structure again - verifies clean
re-evacuation without confusion from old data.

##### Memory initialization:
- `test_evacuated_metadata_initialized` - Verifies cell metadata is properly copied (not uninitialized) by
checking mug cache bits after evacuation.
- `test_evacuated_indirect_atom_padding_zeroed` - For indirect atoms that don't fill their last word
completely, verify padding bytes are deterministic.

##### Invalid input detection (debug assertions):
- `test_evacuate_rejects_cyclic_structure` - Verifies the assert_acyclic! macro fires when given a cyclic
noun (if we can construct one).
- `test_evacuate_rejects_existing_forwarding_pointer` - Verifies `assert_no_forwarding_pointers!` fires when
given a noun with pre-existing forwarding pointers.

##### Arena switching correctness:
- `test_read_pma_noun_with_wrong_arena_installed` - Verifies reading an evacuated noun with the stack arena
(not PMA arena) installed produces incorrect/detectable results or panics.
- `test_arena_switch_mid_traversal` - Verifies that traversing a noun tree requires consistent arena
installation throughout.

## Phase 2

Once we have successfully separated out NockStack from the PMA, we need to
actually implement the ability to load the PMA from disk and make use of it in
ordinary operation of the NockVM.

# Milestone 3: Mutation and freeing

# Milestone 4: Garbage collection

# Milestone 5: Concurrent reads
