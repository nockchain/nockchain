# c1 ledger: uncovered branches in `crates/honk/src/native/ut/mod.rs` 1-4953

This range holds the `Sig64` signature writers, the Hoon arena, `Ut` memo
plumbing, fan-context keys, boundary caches, the `lower_*` helpers, musk setup,
and the `mint_inner`/`play_inner` dispatch.

The gap report lists 640 uncovered lines and 155 untaken branch outcomes: none
are missed only by unit tests (U), 607 lines and 132 branches are missed only
by the parity corpus (P), and 33 lines and 23 branches are missed by both (UP).
`L` rows are lines never run; `B` rows are branch outcomes never taken (`T` or
`F`, with `c2`/`c3` for later conditions of the same `if`).

## Signature digests (`Sig64`)

| Gaps | Tag | Reason |
|---|---|---|
| L207-214 `register_unsigned_root` | P | Unreachable in production: the `Sig64` writers never return `None`, so `enter_hoon_ast_scope` always registers a signed arena. |
| L248-250 `child_count` | P | Test-only (`#[cfg(test)]`). |
| B717 F, B1414 F (`include_dbug_spot` false) | P | Unreachable in production: every `Sig64` is built spot-sensitive. |
| B1382 T, L1383-1385 (memo hit in `write_hoon`) | P | Unreachable in production: one tree walk never reaches the same `&Hoon` node twice. |
| L641-646 `Note::Made` with wings, L744-751 `Spec::Made` | P | Unreachable from source: both come only from a `%made` spec, which neither parser builds (hatch makes one only when decoding a noun). |
| L669-674, B671 T/F `Skin::Dbug` | P | Unreachable from source: `flay` never builds a `%dbug` skin, and hatch makes `Skin::Dbug` only when decoding a noun. |
| L776-784 `BucBuc`, L812-820 `BucDot`, L843-851 `BucFas`, L868-876 `BucTic`, L897-905 `BucZap` | P | Unreachable from source: hoon-138's parser never builds `%bcbc`, `%bcdt`, `%bcfs`, `%bctc`, or `%bczp`, and hatch makes them only when decoding nouns. |
| L929-935 `Chum::VenProVerKel` | P | Unreachable from source: hoon-138's `++bonk` and hatch both read `%k:foo..138` as `[ven pro kel]`, so no four-field chum exists. |
| L963 (tune entry without a value), L972 (tune list item) | P | Unreachable from source: the only source tune, from `=*`, has one entry with a value and an empty list. |
| L1129-1364 (every listed line in `write_poly`, `write_vair`, `write_garb`, `write_stencil`, `write_semi_noun_expr`, `write_coil`, `write_face_type`, `write_type`, `write_nock_hint`, `write_nock`) | P | Unreachable from source: reached only through `Hoon::Hand`. |
| L1423-1426 `Hoon::Hand` | P | Unreachable from source: it comes only from noun decoding. |
| L1527-1530 `BarCen`, L1578-1581 `BarPat` with a prefix | P | Unreachable from source: both parsers emit `%brcn` and `%brpt` with `p=~`. |

## Musk eval-stack copy (`musk_mack_cached_core_in_context`, `copy_into_eval_stack_shared`)

| Gaps | Tag | Reason |
|---|---|---|
| L2304, L2308-2311, B2310 T | P | Defensive: rolls back when the eval stack runs out of memory during a core copy (resource exhaustion only). |
| L2313, B2310 F | UP | Defensive: re-raises a panic other than `AllocationError`; nothing the copy runs panics any other way. |
| L2347, L2364, B2341 F, B2358 F | P | Performance only: a copied noun with no slab-side mug to carry over; production mugs the whole core first (`noun_mug_cached`). |
| L2346, L2363, B2342 F, B2359 F | UP | Unreachable: a copied indirect atom or a freshly allocated cell is always allocated. |

## Build memos, fan legs, and fan context keys

| Gaps | Tag | Reason |
|---|---|---|
| L2389-2403 `clear_build_memos` | P | Parity runs do not reach it: only the prelude mint paths in `bin/honk.rs` (3451, 3477) call it, and it only resets caches. |
| L2478, B2477 T, L2492-2498, B2488 T, B2490 F, B2493 T/F, B2495 T/F (`hold_repo_fan_leg_lookup_id`); L2505, B2504 T (`hold_repo_fan_leg_intern_id`) | P | Performance cache only: interning runs only after both hold maps miss, and hold nouns are hash-consed, so the raw-ID hit and the structural matches do not fire. |
| L2515, B2514 T; L2813, B2812 T; L2908, B2907 T (ID counters wrapping to 0) | P | Unreachable in practice: needs 2^64 interned IDs. |
| B2535 F, B2541 T, L2544-2546, B2544 T, L2548 (`hold_repo_fan_leg_id_by_hold_raw_store`) | P | Performance cache only: callers store only after a raw-map miss, and eviction needs more than 65,536 keys. |
| B2544 F, B2589 F | UP | Unreachable: the order queue is non-empty whenever its length exceeds the limit. |
| L2563-2564, L2568-2569, B2560 T, B2562 T, B2567 T (`hold_repo_fan_leg_id_by_hold_mug_lookup` hit); L2629, B2628 T | P | Performance cache only: the raw-hold map answers first for hash-consed holds. |
| L2589-2591, B2586 T, B2589 T (key eviction); L2602, B2599 T, B2601 T (dedupe); L2606, B2605 T (bucket overflow) in `hold_repo_fan_leg_id_by_hold_mug_store` | P | Performance cache only: eviction needs more than 65,536 holds, the store follows a lookup miss so no equal entry exists, and overflow needs 8 colliding mugs. |
| L2644, B2643 T (`hold_repo_fan_leg_id_by_ptr` hit) | P | Performance cache only: `reachable_legs` memoizes per type ID, so a hold's second lookup does not reach this map. |
| L2647-2649, B2646 F (leg ID of a non-hold) | P | Defensive: `reachable_legs_node` calls it only on `NTy::Hold`. |
| B2763 T (`intersect_sorted_legs` with an empty side) | P | Performance cache only: needs a scope with no reachable legs while the fan is active; no parity probe does this. |
| L2788, B2787 T (empty subset) | P | Unreachable from callers: both scoped-key functions return 0 before interning an empty intersection. |
| L2806, B2804 F (subset signature collision) | P | Performance cache only: needs a (sum, xor, len) collision between distinct leg sets. |
| L2837, B2836 T; L2855, B2854 T; L2875, B2874 T (`!scoped_fan_enabled()`) | UP | Unreachable: `scoped_fan_enabled()` is the constant `true`. |
| L2866-2867, B2864 F (pair-scoped key with a non-empty intersection) | P | Performance cache only: needs a mint, core-mint, mull, or nest cache query while a `%rest` leg reachable from one of its two types is active; no parity probe does this. |
| L2919 (activating an already-active leg) | P | Unreachable in production: `with_active_rest_leg_ids` returns `rest-loop` before activating a leg that is already active. |
| L2940, B2933 F (deactivating an inactive leg) | P | Unreachable: legs are deactivated only after the same scope activated them; the branch is a `debug_assert`. |

## AST scopes and noun canonicalization

| Gaps | Tag | Reason |
|---|---|---|
| L3002-3003, B2999 F | UP | Unreachable: `hoon_signatures_spot_sensitive_pooled` always returns a root signature. |
| L3067, L3070, L3073, L3079, B3066 F, B3069 F, B3072 F, B3078 F (`strip_dbug_wrapper_noun`); L3089, L3092, L3095, B3088 F, B3091 F, B3094 F, B3100 F (`strip_spec_gist_wrapper_noun`); L3127, L3130, L3133, B3111 F, B3112 F, B3113 F (`canonicalize_nonsemantic_hoon_noun`) | P | Defensive: the hoon nouns that reach the wet-rib key are well-formed, so the malformed-noun exits never run. |
| B3097 F, L3099-3101, B3100 T, L3103 (stripping a real `%gist`); L3121, B3118 F (rebuilding a `^:` whose spec had one) | P | Performance cache only: this canonicalization feeds only the `fire` wet-rib recursion-guard key, and no parity wet arm is a `^:` whose spec carries `%gist`. |
| L3137-3141 `hoon_noun_tag` | P | Diagnostics only: formats "ast missing" errors (`repo.rs:78`, `mod.rs` 9413 and 9420). |

## `lower_*` desugarings

| Gaps | Tag | Reason |
|---|---|---|
| L3145 (`:*` with no items) | P | Unreachable from source: `:*` takes one or more hoons in both parsers. |
| L3150, B3149 F | UP | Unreachable: `split_first` already proved there are two or more items. |
| L3353-3354, L3362, B3352 T (`feck` finds `[%sand %tas @]`) | P | Unreachable from source: hoon-138 builds `[%sand %tas @]` only as a path element (`++hasp`, `++limp`) inside a `%clsg` list, never as a bare `~\|` operand. |
| L3483, B3482 F (`;~` with no rules) | P | Unreachable from source: `;~` takes a hoon plus one or more rules. |
| L3539-3545 `prefix_signature` with a prefix | P | Unreachable from source: `%brcn` and `%brpt` prefixes are always `~` from the parser. |

## Memo context keys and boundary caches

| Gaps | Tag | Reason |
|---|---|---|
| L3624-3639, B3622 F, B3648 F (placeholder set non-empty) | P | Unreachable in production: nothing inserts into `arm_placeholder_play_in_progress`. |
| B3647 F (a goal in progress with `arm_in_progress` empty) | P | Unreachable in production: `build_arm_formula_direct` pushes both sets together (8344-8345). |
| L3753, L3758-3768 `mint_cache_key`; L3808-3817, L3819-3820, L3822-3833, L3835-3836, B3819 T/F, B3825 T/F, B3827 T/F, B3829 T/F, B3831 T/F, B3831 c2 T/F, B3831 c3 T/F `mint_boundary_lookup_exact`; L3839-3850, L3852-3867, L3869-3880, B3859 T/F, B3861 T/F, B3863 T/F, B3865 T/F, B3865 c2 T/F, B3865 c3 T/F, B3869 T/F `mint_boundary_store_exact`; L4140-4149, L4151-4153, L4155-4156, L4158-4159, B4148 T/F, B4152 T/F, B4153 T/F `nest_mug_lookup`; L4162-4178, B4174 T/F `nest_mug_register` | P | Test-only (`#[cfg(test)]`). |
| L3990, L3992, L3995, L3997, B3989 F, B3991 F, B3993 F, B3993 c2 F (`redo_boundary_lookup`); L4021-4027, L4030, B4021 T/F, B4023 T/F, B4025 T/F, B4025 c2 T/F, B4029 T (`redo_boundary_store`) | P | Performance cache only: a lookup precedes every store and nouns are hash-consed, so the corpus stores only into empty buckets and every lookup that finds a bucket matches by address. |
| L4060, L4065, L4067, B4059 F, B4061 T, B4063 F, B4063 c2 F (`rest_boundary_lookup`); L4091-4097, L4100, B4091 T/F, B4093 T/F, B4095 T/F, B4095 c2 T/F, B4099 T (`rest_boundary_store`) | P | Performance cache only: same shape as the redo cache; the legs list is rebuilt per call, so it matches structurally rather than by address. |

## `mint`, `mint_inner`, and `play_inner` dispatch

| Gaps | Tag | Reason |
|---|---|---|
| L4263, B4245 F; L4483, B4481 F (`cache_sig` is `None`) | UP | Unreachable: every registered arena node has a signature (see B2999 F). |
| L4376 `mint` of `%hand`; L4755-4757 `play` of `%hand` | P | Unreachable from source: `%hand` comes only from noun decoding. |
| L4450-4452 (`mint` of an empty `=~`) | P | Unreachable from source: `=~` takes two or more hoons, and hatch's whole-file wrapper holds at least one. |
| L4848 (`play` of a one-item `=~`) | P | Unreachable from source: `=~` takes two or more hoons; the only one-item `=~` is hatch's whole-file wrapper, which parity runs mint and never play. |
| L4458-4459, L4852-4853 (`ok_or_else` in the multi-item `=~` arm) | UP | Unreachable: the arm matches only lists of two or more. |
| L4491, B4490 T (`slot_axis` with axis 0) | P | Unreachable in production: the only caller, `musk_mack_constant_core`, passes op-9 arm axes, which compiled formulas never set to 0. |
| B4559 F (cwd with no components) | UP | Diagnostics only: taken only when honk runs from `/`. |
| L4563, B4561 T (error path below the cwd) | P | Diagnostics only: shortens error locations. |
| L4598-4602 `play_noun` | P | Parity runs do not reach it: only the prelude seeding and isolated prelude evaluation in `bin/honk.rs` (3236, 3473) call it. |
| L4676 `play` of `%fits` | P | Unreachable from source: `%fits` appears only as a `?:` condition (from `?=` opening and mold `factory`), and `play` of `%wtcl` never plays its condition. |

## `TomesSignature` checks and `HONK_MEMO_VERIFY`

| Gaps | Tag | Reason |
|---|---|---|
| L93, B91 F, B91 c2 F (`lazy_resolver_id_in` skipping an entry) | UP | Hash collision: a bucket entry made for another tomes map or prefix under the same `TomesSignature`. |
| L3584, 3587-3588, B3577 T, B3584 F, B3584 c2 F (`core_mint_cache_lookup` hit) | P | Performance cache only: the per-gen mint cache has the same key components and answers first; unit tests call the lookup directly. |
| L3586, B3584 T, B3584 c2 T (`core_mint_cache_lookup` rejecting a hit) | UP | Hash collision: a stored entry for other arms or another prefix under the same `TomesSignature`. |
| `memo_verify_recompute` L3666-3675, 3679-3687, 3689-3691, B3687 F; `memo_verify_type_eq` L3693-3695, 3700, B3694 T; `memo_verify_formula_eq` L3702-3704, 3709, B3703 T; `memo_verify_noun_eq` L3711-3713; `memo_verify_typed_formula` L3717-3728, 3730, 3733, 3738-3739, 3742-3743, B3728 F, B3730 F; `mint_inner` L4249-4251, 4254, 4256, 4258, 4262, B4246 F, B4248 T, B4256 F | P | Diagnostics only: the parity corpus runs without `HONK_MEMO_VERIFY`; `c6_cli_memo_verify_rechecks_cache_hits_without_changing_the_artifact` covers it. |
| `memo_verify_recompute` L3688, B3687 T; `memo_verify_type_eq` L3696-3699, B3694 F; `memo_verify_formula_eq` L3705-3708, B3703 F; `memo_verify_typed_formula` L3729, 3731, 3736, 3740-3741, B3728 T, B3730 T; `mint_inner` L4253, 4257, B4256 T | UP | Diagnostics only: a recompute that disagrees with its cached hit, or changes the cache context beyond the arm epoch, which no build produces. |
