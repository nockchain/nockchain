Hoon's +dor decides "heads equal → compare tails, heads unequal → recurse on heads" using =(-.a -.b) — true structural equality. Master's jet approximated = with raw_equals (pointer/bit identity), which is only correct when equal nouns have already been unified into pointer equality. honk's nouns live in NounSlabs and the PMA where unification never happens (and, per this branch's readonly-range work, is explicitly forbidden), so the approximation breaks systematically there. The fix makes the jet agree with Hoon's soft +dor, with urbit's reference jet (vere's u3wc_dor uses full u3r_sing structural equality), and — most tellingly — with honk's own native dor, which already had exactly this structure.

The bug the new test pins down

Master's jet on (dor [[1 2] 4] [[1 2] 3]) with freshly allocated (unshared) [1 2] heads:

- Heads aren't raw_equals (separate allocations) → takes the "heads unequal" branch → recurses dor([1 2], [1 2]) → 1 and 1 are direct atoms, bit-equal → compare tails → 2 raw-equals 2 → YES.
- Hoon's +dor: heads are = → compare tails: (lth 4 3) → NO.

And by the same path master also returns YES for the swapped arguments — so a < b and b < a simultaneously: the comparator loses antisymmetry and stops being a total order. There's a second variant for indirect atoms: equal-but-unshared big atoms fail raw_equals → fall to lth(x, x) → NO where = says YES, inverting gor/mor tie-breaks. The new regression test in sort.rs (dor([[1 2] 4], [[1 2] 3]) must be NO) is precisely the first case.

Since dor is the tie-breaker inside gor and mor, and gor/mor ordering decisions are baked into the shape of every +map/+set treap, an inconsistent comparator means structurally corrupt trees: entries inserted down one branch that probes later won't find, +apt violations, unstable +sort output.

Why hoonc never noticed, and why honk can't avoid it

On the NockStack, the = jet is unifying_equality, which aggressively merges structurally equal nouns into pointer equality as a side effect of every comparison. Map keys get compared by = constantly (+get:by/+put:by check =(b p.n.a) before calling gor), so by the time master's dor looked at heads, equal heads were usually already pointer-equal and raw_equals happened to give the right answer. The bug only fired in the gap before unification—rare and transient in hoonc's world.

honk removes the mask completely, three ways:

1. Its nouns can't unify. honk's compiler state, cued assets (honc-cold-138.jam, formula/type jams), and zero-copy mack subjects live in slabs and the PMA. The readonly-extra-range machinery added in this same branch exists specifically so unifying_equality won't rewrite into that memory. Equal nouns from these sources are non-pointer-equal forever, so master's jet doesn't occasionally mis-order — it mis-orders every time.

2. honk builds treaps natively and the shapes must match bit-for-bit. ut/mod.rs:11177 (map_put_mug), set_put_mug, gor_mug/mor_mug (ut/mod.rs:11035,11044) are a native Rust implementation of +put:by/+put:in that honk uses to construct real map/set nouns (e.g., the tome/arm maps at ut/mod.rs:7027,7043). honk's entire value proposition rests on producing output noun-identical to hoonc's (that's why it hardcodes %spot line numbers for parity elsewhere). A map built by honk's native put and "the same" map built by jetted Hoon code must have identical treap shapes, or = fails on them and jams diverge. That forces honk's native comparator and the nockvm jet to implement the same ordering — the correct, structural one.

3. honk's native dor already does it. Look at ut/mod.rs:11023:

```
if a_head.raw_equals(&b_head)
    || (slab_mug(a_head, &space) == slab_mug(b_head, &space)
        && matches!(noun_eq(a_head, b_head, &space), Ok(true)))
{
    dor(slab, a_tail, b_tail)
} else {
    dor(slab, a_head, b_head)
}
```

3. The nockvm jet change is this exact structure transplanted: pointer fast path, then structural equality, then branch. The jet was brought into agreement with the native implementation, not the other way around.

There's likely a fourth, operational reason: nockvm's test_jets machinery runs a jet and its soft Hoon side by side and BAIL_JESTs on disagreement. Master's dor jet disagrees with soft +dor whenever pointer-sharing differs—so any jet-validation pass during honk bring-up (where unshared-equal nouns are the norm) would have tripped on it immediately.

Why noun_equality specifically, and the perf shape that fell out

unifying_equality was not an option here: it requires mutable slots it's allowed to rewrite, and the whole point is that these nouns may sit in PMA assets or read-only slabs. noun_equality's doc says it outright: "suitable for use with allocators that don't support unification (e.g., Pma, NounSlab)." It's also cycle-safe via its already_equal set and uses cached mugs for fast rejection.

The perf regression relative to honk's own version comes from two omissions, not from the concept:

- honk's native dor puts an explicit mug comparison in front of the deep compare (slab_mug(a) == slab_mug(b) && noun_eq(...)), computing and caching mugs in the slab — so unequal heads are rejected in O(1) almost always. The nockvm jet skips that pre-filter and relies on noun_equality's internal cached-mug check, which only helps if both sides already carry cached mugs; otherwise unequal heads can cost a structural walk to discover they're unequal.

- noun_equality heap-allocates its Vec worklist (and an IntMap on first insert) on every call, which honk's slab-side version tolerates because it runs at compile-time cadence, not at interpreter cadence inside gor/mor tie-breaks.

The invariant to preserve is "the dor jet implements = structurally, identically to soft +dor and honk's native dor" — that's load-bearing for honk's map parity and for jet/soft consistency generally (it's a real bug fix you'd want even without honk). The cost, like find_jet's, is an implementation choice: a mug pre-filter on the heads (mirroring honk's own version) and a non-allocating equality walk would keep the semantics while removing most of the new expense.
