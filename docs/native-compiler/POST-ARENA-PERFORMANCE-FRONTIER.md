# Post-arena Honk performance frontier

## Scope

This campaign starts at commit `58646a60810130cea6705cca63d6664783a2a879`, after the arena-indexed Hoon and type IR work. It uses the release-LTO compiler on `wx-workstation`, pins every compile to CPU 12, rejects measurements contaminated by concurrent CI compilation, brackets every candidate with its exact control, and requires the resulting JAM to match the known SHA-256. Throughput gain is `control midpoint / candidate - 1`.

## Accepted changes

Live native-type leaves are now canonicalized by the compile-wide `TypeTable`. Direct atoms retain their zero-lookup path. For allocated nouns, source identity is memoized by raw address and a new identity is compared exactly only against canonical leaves in the same mug bucket. A mug collision cannot conflate unequal nouns, while structurally equal leaves subsequently share one raw noun and make type-table equality pointer-fast. The isolated final version measured 88.50s against a Dumbnet control bracket of 89.68/89.70s (+1.34%) and 131.89s against a Roswell bracket of 132.96/134.78s (+1.50%). Peak RSS rose by roughly 46 MB on Dumbnet and 116 MB on Roswell because the compile context retains the identity index.

`CellHandle::head_tail` resolves a cell pointer once and returns both children. `noun_eq` now uses it for each compared cell rather than resolving the same left and right cells separately for their heads and tails. Against the canonical-leaf compiler, the reverse Dumbnet bracket measured 86.43/87.23s around an 88.93s control (+2.42%), and Roswell measured 133.24s against a 136.21/136.09s bracket (+2.17%). A NockVM regression test proves that the paired accessor returns the same nouns as the individual accessors.

The final two-change compiler is not fully additive because optimized code placement changes when both patches are present. Direct final-versus-antecedent brackets measured the following results, and every candidate and control artifact had the exact known hash.

| Kernel | Antecedent bracket | Final compiler | Throughput gain | Antecedent/final maximum RSS | Exact output SHA-256 |
| --- | ---: | ---: | ---: | ---: | --- |
| Dumbnet | 89.24/89.05s | 86.72s | +2.80% | 6,168,348/6,200,108 KB | `2b5b0f77937f5162ed0e6c8f8ebd2651761576798f79dfa95a079d490423bb01` |
| Wallet | 92.62/92.48s | 91.26s | +1.41% | 6,854,818/6,900,768 KB | `7f6fc956b87f660f7371bd2caf955f4d2610103bd5a05c1c20e0fce66fbfa474` |
| Roswell | 131.39/133.57s | 130.88s | +1.22% | 9,432,588/9,537,440 KB | `612025fc58d84dcd226c4b7c0b49602a4c86a430683d6b22b66a3e109815a842` |

## Rejected variants

Pruning the discarded side of type-algebra results remained byte-exact and slightly reduced memory, but clean brackets regressed throughput by 0.64% and 1.62%. Reusing the `musk_araw` map measured +0.06%, indistinguishable from noise, while slightly increasing RSS. Both were reverted.

Representing common axes inline as `u64` and retaining `BigUint` only for wide axes preserved arbitrary-precision correctness and saved about 49 MB on Dumbnet, but regressed throughput by 1.86%. It was reverted; axis arithmetic is not currently expensive enough to repay the extra representation branches.

Caching the structural signature of every source `Spot` was independently positive: Dumbnet improved by 0.84% in control/candidate/control order and 1.54% in reverse order, while Roswell improved by 0.78% and used about 19 MB less peak memory. It did not compose with canonical leaves: the combined binary was -0.60% in the first Dumbnet bracket, -0.14% in reverse order, and -1.06% on Roswell. The implementation is preserved in the isolated `nockchain-honk-spot-signature` worktree, but it is excluded from the final branch until a future PGO or code-layout regime makes the composition positive.

Inlining and directly constructing `NounSpace::empty` targeted 635,618,425 instrumented calls but regressed both benchmark orders by about 0.7%; optimized release code already removes most of the apparent cost, while forced inlining expands the callers. Adding a raw-pointer fast path immediately outside `noun_eq` was -0.87% in one order and +0.56% in the reverse order, or -0.02% pooled across all six runs. Both were reverted.

Extending paired cell reads beyond `noun_eq` failed the measured gate. Applying it in `slab_mug` regressed Dumbnet by 1.04%, and applying it to the dominant `semi_parts` loop regressed by 2.06%. These results show that the accepted win is specific to eliminating repeated resolutions inside structural comparison; mechanically widening the accessor change perturbs hotter callers enough to lose throughput.

## Validation

The final source adds exact structural-collision protection for canonical leaf buckets, a regression proving pointer-distinct equal nouns canonicalize to one carried leaf, and a NockVM regression proving paired cell access matches individual access. The complete Honk library suite passes 138 tests, the complete Hatch library suite passes all 276 tests, the NockVM library suite passes 183 tests with four pre-existing ignored tests, and all 70 compiler-mint integration tests pass. `cargo check -p honk --all-targets`, `cargo fmt --check`, and `git diff --check` pass. The benchmark release binary SHA-256 is `5ca39efa0294870612b1d052a0ed0543f2d60c8bcd7a6d8d698bfe12e628f496`. An independent clean checkout of pushed source commit `194d90d2fbdfee50405a466f1f6138ccc5875fb2` rebuilt as SHA-256 `09a697aca9e3729c6d7421f16b1c62128301ee41bd71f85282f51ef44c8ba123`; release debug metadata embeds its different worktree path, but it independently emitted the exact Dumbnet oracle hash.

The six production artifact oracle remains Dumbnet `2b5b0f77937f5162ed0e6c8f8ebd2651761576798f79dfa95a079d490423bb01`, Wallet `7f6fc956b87f660f7371bd2caf955f4d2610103bd5a05c1c20e0fce66fbfa474`, Miner `1a7615c7af9df85066c9981a30e3bfe128dd11f7e1760df182014a7cbb3aa418`, Peek `9af1f26217fc3226f190bece929da7ac181b427fd721e42081faa731657fe094`, Bridge `6304ae91b546e0fe39d3be6e558d4710278904151c86406e37e4a903fcd8e7c3`, and Roswell `612025fc58d84dcd226c4b7c0b49602a4c86a430683d6b22b66a3e109815a842`.
