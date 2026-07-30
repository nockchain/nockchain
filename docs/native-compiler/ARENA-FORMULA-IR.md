# Arena-indexed Nock formula IR

## Outcome

Honk now carries generated Nock formulas end to end as compact `FormulaId(u32)` values in one compile-local `FormulaArena`. Formula construction, composition, equality, cache values, arm lookup, type tests, edits, hints, and the final mint result stay in the arena; they become noun trees only at explicit formula-as-data interpreter boundaries or when the public compiler result is emitted. This replaces repeated noun allocation and recursive structural comparison with dense identity and one-time materialization while preserving exact compiler output.

## Representation

`FormulaArena` stores canonical nodes for raw formulas, slots, quotes, evaluation, autocons, conditionals, kicks, edits, hints, and the remaining fixed-arity opcodes. Child edges are `FormulaId`s, each node is hash-consed with collision-safe structural equality, and each entry caches its materialized noun. Axes use a small `u64` representation when possible and `BigUint` otherwise, so the IR does not reintroduce Honk's former 64-bit axis limit. Quoted constants and hint clues are opaque leaves: importing `[1 constant]` does not recursively intern an arbitrarily large data noun.

The arena belongs to a single `Ut` compiler session. IDs never escape that lifetime and therefore need no generation counter. Structural cache keys that previously hashed or compared formula nouns now carry `FormulaId`; a cache hit cannot mistake two merely hash-colliding formulas because the arena assigns an ID only after exact node equality. Imported legacy formula nouns also have a raw-identity memo so a noun already crossing into the arena is decoded once.

## Semantic boundaries

The default direction is native construction and composition. `mint`, `find`, `fine`, `fire`, lazy arm resolution, synthetic ports, `fish`, type-test construction, `hike`, `%ktsg` fold keys, and core battery composition all exchange `FormulaId`s. The arena implements the historical `cons`, `comb`, `cond`, `flip`, `flan`, `flor`, `and`, and `cove` transformations directly, preserving their simplification and check order.

Materialization remains intentional where Nock semantics treat a formula as ordinary data. Musk fold evaluation needs an executable noun, hint clues remain noun payloads, a completed arm formula becomes data in a core battery, and `%zpts` quotes a formula. The public `mint_noun` boundary materializes the final formula. Materialization recursively emits each distinct arena node no more than once and retains the resulting noun, preserving DAG sharing in the slab.

`%hand` historically accepts extension or malformed formulas, so formula import does not make Honk stricter. If a noun is outside the canonical opcode shape, the arena stores it as an opaque `Raw` leaf and re-emits it exactly. Leaf atoms that fit `u64` but exceed the tagged direct-atom range go through `Atom::new`, allowing the allocator to choose the correct representation. These details are covered by focused regressions.

## Performance evidence

The primary workload is a clean release build of Dumbnet with native Honk, pinned to CPU 12 on `wx-workstation`. The accepted parent is commit `c1ee8695db485e6ba4ad8aafe0c765a0022575b8`, whose runtime source is identical to `194d90d2`. The control and candidate each used three warmups followed by ten measured runs. Control wall time was 87.652 seconds mean, 87.560 median, 86.660–88.700 range, 0.594 standard deviation, and ±0.425 seconds 95% confidence interval. Candidate wall time was 85.922 seconds mean, 85.880 median, 85.270–86.650 range, 0.419 standard deviation, and ±0.300 seconds 95% confidence interval. That is 1.974% less wall time and 2.013% more compile throughput. Mean peak RSS fell from 6,197,459 KB to 6,169,566 KB, a reduction of 27,893 KB or 0.450%. The candidate output SHA-256 was `2b5b0f77937f5162ed0e6c8f8ebd2651761576798f79dfa95a079d490423bb01`, exactly matching Dumbnet's accepted artifact.

A 497 Hz Samply profile of an exact-output candidate compile collected 39,503 compiler-worker samples. `FormulaArena::intern_with_materialized` accounted for 0.056% self and 0.157% inclusive samples, `comb` for 0.008% self and 0.030% inclusive, and materialization for one sample. The remaining profile is again dominated by noun equality and type/compiler tables. This is the intended result: formulas are no longer a meaningful hot path after the migration, and further micro-optimization of arena hashing, axes, or materialization has negligible expected value. The separate fixed-arity materialization commit removes a known heap allocation but is below profile and benchmark resolution; it is not included in the claimed measured yield.

## Correctness gates

Focused FormulaArena tests prove hash consing, exact smart-constructor noun parity, opaque quoted imports, preservation of noncanonical `%hand` formulas, and correct allocation of `u64::MAX` leaves. The broader gates are all Honk library tests, all compiler-mint integration tests, Hatch's library suite, NockVM's library suite, `cargo check -p honk --all-targets`, and byte-exact native builds of Dumbnet, Wallet, Miner, Peek, Bridge, and Roswell. The production-kernel hashes and final benchmark statistics belong in the validation record below rather than being inferred from projections.

## Validation record

Final source commit, production-kernel hashes, and terminal benchmark statistics are populated only from the exact pushed source after every gate completes.
