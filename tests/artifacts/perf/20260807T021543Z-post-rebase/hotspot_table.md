# Ranked hoon-138 CPU hotspots

The profile contains 62,805 weighted samples at roughly 1 kHz. Percentages
below are self time: each sample is assigned once, so rows and clusters may be
added without double-counting. Cluster totals are approximate because the
grouping is conceptual.

| Rank | Self-time cluster | Approx. share | Representative symbols | Interpretation |
| ---: | --- | ---: | --- | --- |
| 1 | Noun/value transport and copying | 28.5% | `Ut::value_import` 7.90%; `NounSlab::copy_into` 5.45%; `copy_noun_into_allocator` 5.18%; `ValueArena::intern_cell_with_noun` 3.54%; `TaggedPtr::resolve_const` 3.42%; `Ut::copy_into_eval_stack_shared` 2.96% | The strongest CPU target is data movement across noun, allocator, and evaluation boundaries. It is also the most plausible CPU-side contributor to memory pressure, though this CPU profile cannot prove retained allocation. |
| 2 | Map/cache growth and hashing | about 10–12% | nest memo reserve/rehash 2.11%; `IntMap::increase_cache` 1.91%; Hoon-id map insertion 1.71%; another fast-hash insertion path 1.36%; raw-table rehash and `IntMap` insertion each around 1.2% | Exact or bounded pre-sizing is worth testing before changing map implementations. |
| 3 | Type recursion and miss paths | at least 8.6% | `Ut::miss_dext` 4.07%; `Ut::nest_inner_impl` closure 2.74%; `noun_eq` 1.34%; `TypeTable::intern_node` 0.42% | Type work still matters, but the arena interner itself is not the dominant remaining cost. |
| 4 | Formula/source serialization | about 2.1% | `Sig64::write_hoon` 1.58%; `Sig64::write_spot` 0.49% | Serialization is now a minor target and should not lead the next optimization pass. |

Selected individual self-time functions:

| Rank | Function | Self samples | Self time |
| ---: | --- | ---: | ---: |
| 1 | `Ut::value_import` | 4,962 | 7.901% |
| 2 | `NounSlab::copy_into` | 3,424 | 5.452% |
| 3 | `copy_noun_into_allocator` | 3,256 | 5.184% |
| 4 | `Ut::miss_dext` | 2,557 | 4.071% |
| 5 | `ValueArena::intern_cell_with_noun` | 2,224 | 3.541% |
| 6 | `TaggedPtr::resolve_const` | 2,150 | 3.423% |
| 7 | `Ut::copy_into_eval_stack_shared` | 1,861 | 2.963% |
| 8 | `Ut::nest_inner_impl` closure | 1,723 | 2.743% |
| 9 | Nest memo reserve/rehash | 1,327 | 2.113% |
| 10 | `IntMap::increase_cache` | 1,197 | 1.906% |

Inclusive stacks put `Ut::mint*` at about 89.6%, native entry compilation at
about 59.8%, `Ut::musk_araw` at about 35.5%, `Ut::mull` at about 30.2%, and
`Ut::nest` at about 20.6%. Inclusive values overlap and must not be added.
