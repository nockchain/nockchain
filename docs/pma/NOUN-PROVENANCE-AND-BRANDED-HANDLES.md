# Noun Provenance and Branded Handles

This note consolidates the useful parts of the old `ALIEN-NOUNSPACE-AUDIT.md` and `ALIEN-NOUNSPACE-AUDIT-FOLLOW-UP.md` into a current-state explanation of the problem branded noun handles are meant to solve.

## Summary

A raw `Noun` is only a tagged word. For allocated nouns, the payload is meaningful only relative to the arena that owns it:

- a NockStack arena,
- a PMA arena,
- a NounSlab allocation range, or
- another explicit `NounSpace` that can resolve the pointer or offset.

PMA makes this invariant unavoidable. Stack pointers, PMA offsets, and slab pointers can all appear in ordinary runtime flows, but they are not interchangeable. A raw noun copied out of its owner and later decoded with the wrong `NounSpace` can become:

- a dangling pointer,
- an offset resolved against the wrong PMA,
- a mixed tree whose root is local but whose children point elsewhere,
- a value that works in today's call path only because the original owner happens to stay alive nearby.

The audit found that the main remaining risk was not one isolated bug. It was an API pattern: functions accepted or returned raw `Noun` values after having just used a `NounSpace` to decode them. That erases provenance at exactly the boundary where Rust types should preserve it.

`NounHandle<'a>` is the first mitigation: it pairs a raw noun with `&'a NounSpace`, so common traversal and decoding helpers can keep the owner nearby.

Branded handles go a step further. `NounSpace::with_brand(...)` creates a generative brand, and `BrandedNounHandle<'space, 'id>` carries that brand. Code inside the branded scope can only combine handles from the same branded space, and branded handles cannot escape the scope. This is intended to make accidental cross-arena mixing fail at compile time rather than at PMA dereference time.

## Bug Family

The audit grouped issues under the term "alien noun" or "alien NounSpace". The concrete patterns were:

1. Returning a raw `Noun` whose owner has already been dropped.
2. Storing raw `Noun` fields in owned decoded structs without storing the owner.
3. Iterating lists/maps as `Noun` after accepting a `NounSpace` or `NounHandle`.
4. Rebuilding new cells/lists in one allocator from raw children owned by another allocator.
5. Reconstructing a `NounSpace` after the fact from raw pointer ranges.
6. Validating only the root of a tree, while reachable descendants may still live in other arenas.

These are all the same logical problem: arena provenance is not represented in the type that crosses the API boundary.

## Current State of the Original Findings

### Fixed or materially improved

- `IntoNoun for &str` was a real dangling-pointer constructor: it allocated into a temporary `NounSlab`, returned the raw noun, and dropped the slab. This is now banned with a negative impl (`impl !IntoNoun for &str {}`). Use allocator-taking APIs or slab-producing APIs instead.

- `noun-serde` used to identity-encode and identity-decode raw `Noun`. That allowed typed decode to erase provenance and typed encode to smuggle foreign nouns into new structures. Raw `Noun` now has negative impls for `NounEncode` and `NounDecode`.

- Several slab modification paths now re-home returned nouns before installing them under a slab root. This makes `NounSlab::modify(...)`, `modify_noun(...)`, and `modify_with_imports3(...)` much safer for stack-pointer-form nouns.

- The block explorer has moved much of its parsing toward handle-based helpers. Many helpers now accept `&NounHandle`, and `HoonMapIter` yields `NounHandle` rather than raw nouns.

- The math-side situation is improved by the presence of handle-shaped traversal APIs such as `NounMathExtHandle::uncell(...)` and handle-yielding map iteration.

- Cold-state restore is less dangerous than the original audit suggested because the live checkpoint restore path copies checkpoint cold state into the active stack before decoding it.

### Still worth treating as hazardous API surface

- `NounSlab::set_root(...)` validates the top-level allocated root, not the full reachable graph. A root cell allocated in the slab can still theoretically contain descendants from another arena if a caller bypasses the re-homing helpers and constructs the cell manually.

- `NounSlab::rehome_noun(...)` can re-home stack-pointer-form nouns, but it cannot safely import PMA offset-form nouns without a source `NounSpace`. Callers that may receive PMA nouns need an explicit copy/import API that carries the source space.

- `NounSlab`'s public `NounMap<V>` stores raw noun keys and asks callers to provide a `NounSpace` later. Current uses appear internal to jamming/backrefs where the owner stays live, but the public shape still erases provenance.

- `nockchain-math::NounMathExt::uncell(...)` still returns raw `[Noun; N]` after taking a `&NounSpace`. The handle variant should be preferred for new code.

- Some `zkvm-jetpack` helpers still convert Hoon lists into `Vec<Noun>` and rebuild lists/tuples from raw nouns. Current jet call sites appear intra-stack, but the helpers are structurally unsafe for cross-space use.

- `jets::cold` has historically treated raw `Noun` as a decoded payload. Some re-homing mitigations have landed, but long-term encode/decode APIs should prefer `NounHandle` or branded forms rather than raw nouns.

- `bridge::signing::sign_proposal(...)` reconstructs `NounSpace` manually and rejects PMA offset-form nouns. It appears test-only today, but it is the exact pattern branded handles are meant to avoid.

## Why `NounHandle` Is Not Always Enough

`NounHandle<'a>` prevents many lifetime mistakes because it carries `&'a NounSpace`. It is the right default for traversal and local parsing.

However, unbranded handles from different spaces can still have the same Rust type. A function that accepts two `NounHandle<'a>` values can accidentally compare or combine nouns from different spaces if both spaces have compatible lifetimes. The runtime may catch some of this through range checks, but the type system does not know the two handles must share the same owner.

Branded handles add a generative identity:

```rust
space.with_brand(|space| {
    let noun = space.handle(raw_noun);
    // `noun` is tied to this exact branded space.
});
```

Inside the closure, all branded handles carry the same hidden brand. Handles from another `with_brand` call have a different brand and cannot be passed to functions expecting this one. A branded handle also cannot escape the closure because the brand lifetime is generative.

This directly addresses the audit's main pattern: APIs should not be able to accept "a noun and some space that probably matches" when they actually require "a noun proven to come from this exact space."

## API Guidance

For new or touched PMA-sensitive code:

1. Prefer `NounHandle` inputs over `(Noun, &NounSpace)` pairs.
2. Prefer handle-yielding iterators over `Iterator<Item = Noun>`.
3. Avoid owned structs with raw `Noun` fields unless the owning slab/space is stored with them and outlives every use.
4. If a noun crosses into a new allocator, copy/import it using an API that takes the source `NounSpace`.
5. If a helper must prove multiple nouns share the same arena, consider a branded API.
6. Do not reconstruct a `NounSpace` from raw pointers after the fact.
7. Do not use raw `Vec<Noun>` as an intermediate for values that may be re-encoded into another allocator.
8. If a raw-noun escape hatch remains for performance or ergonomics, document the lifetime/provenance precondition at the call site.

## Practical Migration Priority

The highest-value long-term cleanups are:

1. Convert remaining list/map/tuple traversal APIs that return raw nouns to handle-yielding forms.
2. Replace raw-noun intermediate structs in parser paths with borrowed view/parser helpers.
3. Add full-graph or source-space-aware slab import helpers where code may splice PMA nouns into slabs.
4. Remove or fence public APIs that store raw nouns for later comparison.
5. Use branded handles in APIs that require all nouns to come from one exact `NounSpace`.

The goal is not to eliminate raw `Noun` from the VM internals. The goal is to stop raw nouns from crossing ownership boundaries without a compile-time or explicit runtime proof of provenance.
