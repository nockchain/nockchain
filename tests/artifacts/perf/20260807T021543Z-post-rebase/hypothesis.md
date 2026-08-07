# Optimization hypotheses

## H1 — Eliminate redundant noun/value copies at native mint boundaries

**Evidence:** disjoint self samples attributed about 28.5% of CPU time to value
import, slab/allocator/evaluation-stack copying, cell interning, and tagged
pointer resolution.

**Experiment:** instrument bytes and nouns copied at `value_import`,
`NounSlab::copy_into`, `copy_noun_into_allocator`, and
`copy_into_eval_stack_shared`. Then prototype the narrowest ownership or
borrowing change that removes a confirmed duplicate copy without extending
arena lifetimes.

**Success criterion:** at least 10% lower hoon-138 wall time or peak RSS, no
regression greater than the defined variance envelope on Dumbnet, exact parity
for all production kernels, and no new unsafe lifetime escape.

**Risk:** high. Copy removal crosses allocator and lifetime boundaries; CPU
samples alone do not prove that any individual copy is redundant.

## H2 — Pre-size the measured memo and identifier maps

**Evidence:** growth, rehashing, and insertion account for roughly 10–12% of
self samples. Several tables grow repeatedly during the same compile.

**Experiment:** record final and peak sizes plus growth counts, then use those
measurements to set per-workload-independent initial capacities or carry safe
capacity hints from known input sizes.

**Success criterion:** fewer rehash/growth events and at least 3% lower wall
time without more than 5% additional peak RSS.

**Risk:** low to medium. Over-sizing can trade CPU for memory, which is already
the limiting resource for hoon-138.

## H3 — Allocation retention, not the type interner, explains the 10.29 GiB peak

**Evidence:** `TypeTable::intern_node` is only 0.416% self time, while noun/value
copy paths are hot. This is consistent with, but does not prove, excessive
retained value data.

**Experiment:** add allocation counters or use an allocation/heap profiler that
can attribute live bytes at the hoon-138 high-water mark. Rank retained bytes by
type and call site before changing ownership.

**Success criterion:** explain at least 80% of peak live bytes and identify a
bounded lifetime or duplication change that reduces peak RSS by at least 10%.

**Risk:** medium. RSS includes allocator behavior and pages that a CPU sampler
cannot attribute.

## Rejected leads

- Parsing and filesystem I/O: the hoon-138 leaf parse was 0.098 s and the timed
  processes performed no block I/O.
- Another arena-interner rewrite: `TypeTable::intern_node` was 0.416% self time.
- Serialization first: `Sig64::write_hoon` plus `write_spot` was about 2.1% self
  time, materially below the copy and cache clusters.
