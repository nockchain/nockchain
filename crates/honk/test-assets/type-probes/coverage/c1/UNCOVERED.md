# c1 ledger: uncovered branches in `crates/honk/src/native/ut/mod.rs` 1-4817

This range holds the `Sig64` signature writers, the Hoon arena, `Ut` memo
plumbing, fan-context keys, boundary caches, the `lower_*` helpers, musk setup,
and the `mint_inner`/`play_inner` dispatch.

The gap report lists 561 uncovered lines and 134 untaken branch outcomes: none
are missed only by unit tests (U), 546 lines and 121 branches are missed only
by the parity corpus (P), and 15 lines and 13 branches are missed by both (UP).
`L` rows are lines never run; `B` rows are branch outcomes never taken (`T` or
`F`, with `c2`/`c3` for later conditions of the same `if`).

## Signature digests (`Sig64`)

| Gaps | Tag | Reason |
|---|---|---|
| L184-191 `register_unsigned_root` | P | Unreachable in production: the `Sig64` writers never return `None`, so `enter_hoon_ast_scope` always registers a signed arena. |
| L225-227 `child_count` | P | Test-only (`#[cfg(test)]`). |
| B692 F, B1389 F (`include_dbug_spot` false) | P | Unreachable in production: every `Sig64` is built spot-sensitive. |
| B1357 T, L1358-1360 (memo hit in `write_hoon`) | P | Unreachable in production: one tree walk never reaches the same `&Hoon` node twice. |
| L616-621 `Note::Made` with wings, L719-726 `Spec::Made` | P | Unreachable from source: both come only from a `%made` spec, which neither parser builds (hatch makes one only when decoding a noun). |
| L644-649, B646 T/F `Skin::Dbug` | P | Unreachable from source: `flay` never builds a `%dbug` skin, and hatch makes `Skin::Dbug` only when decoding a noun. |
| L751-759 `BucBuc`, L787-795 `BucDot`, L818-826 `BucFas`, L843-851 `BucTic`, L872-880 `BucZap` | P | Unreachable from source: hoon-138's parser never builds `%bcbc`, `%bcdt`, `%bcfs`, `%bctc`, or `%bczp`, and hatch makes them only when decoding nouns. |
| L904-910 `Chum::VenProVerKel` | P | Unreachable from source: hoon-138's `++bonk` and hatch both read `%k:foo..138` as `[ven pro kel]`, so no four-field chum exists. |
| L938 (tune entry without a value), L947 (tune list item) | P | Unreachable from source: the only source tune, from `=*`, has one entry with a value and an empty list. |
| L1104-1339 (every listed line in `write_poly`, `write_vair`, `write_garb`, `write_stencil`, `write_semi_noun_expr`, `write_coil`, `write_face_type`, `write_type`, `write_nock_hint`, `write_nock`) | P | Unreachable from source: reached only through `Hoon::Hand`. |
| L1398-1401 `Hoon::Hand` | P | Unreachable from source: it comes only from noun decoding. |
| L1502-1505 `BarCen`, L1553-1556 `BarPat` with a prefix | P | Unreachable from source: both parsers emit `%brcn` and `%brpt` with `p=~`. |

## Musk eval-stack copy (`musk_mack_cached_core_in_context`, `copy_into_eval_stack_shared`)

| Gaps | Tag | Reason |
|---|---|---|
| L2278, L2282-2285, B2284 T | P | Defensive: rolls back when the eval stack runs out of memory during a core copy (resource exhaustion only). |
| L2287, B2284 F | UP | Defensive: re-raises a panic other than `AllocationError`; nothing the copy runs panics any other way. |
| L2321, L2338, B2315 F, B2332 F | P | Performance only: a copied noun with no slab-side mug to carry over; production mugs the whole core first (`noun_mug_cached`). |
| L2320, L2337, B2316 F, B2333 F | UP | Unreachable: a copied indirect atom or a freshly allocated cell is always allocated. |

## Build memos, fan legs, and fan context keys

| Gaps | Tag | Reason |
|---|---|---|
| L2363-2377 `clear_build_memos` | P | Parity runs do not reach it: only the prelude mint paths in `bin/honk.rs` (3394, 3420) call it, and it only resets caches. |
| L2452, B2451 T, L2466-2472, B2462 T, B2464 F, B2467 T/F, B2469 T/F (`hold_repo_fan_leg_lookup_id`); L2479, B2478 T (`hold_repo_fan_leg_intern_id`) | P | Performance cache only: interning runs only after both hold maps miss, and hold nouns are hash-consed, so the raw-ID hit and the structural matches do not fire. |
| L2489, B2488 T; L2787, B2786 T; L2882, B2881 T (ID counters wrapping to 0) | P | Unreachable in practice: needs 2^64 interned IDs. |
| B2509 F, B2515 T, L2518-2520, B2518 T, L2522 (`hold_repo_fan_leg_id_by_hold_raw_store`) | P | Performance cache only: callers store only after a raw-map miss, and eviction needs more than 65,536 keys. |
| B2518 F, B2563 F | UP | Unreachable: the order queue is non-empty whenever its length exceeds the limit. |
| L2537-2538, L2542-2543, B2534 T, B2536 T, B2541 T (`hold_repo_fan_leg_id_by_hold_mug_lookup` hit); L2603, B2602 T | P | Performance cache only: the raw-hold map answers first for hash-consed holds. |
| L2563-2565, B2560 T, B2563 T (key eviction); L2576, B2573 T, B2575 T (dedupe); L2580, B2579 T (bucket overflow) in `hold_repo_fan_leg_id_by_hold_mug_store` | P | Performance cache only: eviction needs more than 65,536 holds, the store follows a lookup miss so no equal entry exists, and overflow needs 8 colliding mugs. |
| L2618, B2617 T (`hold_repo_fan_leg_id_by_ptr` hit) | P | Performance cache only: `reachable_legs` memoizes per type ID, so a hold's second lookup does not reach this map. |
| L2621-2623, B2620 F (leg ID of a non-hold) | P | Defensive: `reachable_legs_node` calls it only on `NTy::Hold`. |
| B2737 T (`intersect_sorted_legs` with an empty side) | P | Performance cache only: needs a scope with no reachable legs while the fan is active; no parity probe does this. |
| L2762, B2761 T (empty subset) | P | Unreachable from callers: both scoped-key functions return 0 before interning an empty intersection. |
| L2780, B2778 F (subset signature collision) | P | Performance cache only: needs a (sum, xor, len) collision between distinct leg sets. |
| L2811, B2810 T; L2829, B2828 T; L2849, B2848 T (`!scoped_fan_enabled()`) | UP | Unreachable: `scoped_fan_enabled()` is the constant `true`. |
| L2840-2841, B2838 F (pair-scoped key with a non-empty intersection) | P | Performance cache only: needs a mint, core-mint, mull, or nest cache query while a `%rest` leg reachable from one of its two types is active; no parity probe does this. |
| L2893 (activating an already-active leg) | P | Unreachable in production: `with_active_rest_leg_ids` returns `rest-loop` before activating a leg that is already active. |
| L2914, B2907 F (deactivating an inactive leg) | P | Unreachable: legs are deactivated only after the same scope activated them; the branch is a `debug_assert`. |

## AST scopes and noun canonicalization

| Gaps | Tag | Reason |
|---|---|---|
| L2976-2977, B2973 F | UP | Unreachable: `hoon_signatures_spot_sensitive_pooled` always returns a root signature. |
| L3041, L3044, L3047, L3053, B3040 F, B3043 F, B3046 F, B3052 F (`strip_dbug_wrapper_noun`); L3063, L3066, L3069, B3062 F, B3065 F, B3068 F, B3074 F (`strip_spec_gist_wrapper_noun`); L3101, L3104, L3107, B3085 F, B3086 F, B3087 F (`canonicalize_nonsemantic_hoon_noun`) | P | Defensive: the hoon nouns that reach the wet-rib key are well-formed, so the malformed-noun exits never run. |
| B3071 F, L3073-3075, B3074 T, L3077 (stripping a real `%gist`); L3095, B3092 F (rebuilding a `^:` whose spec had one) | P | Performance cache only: this canonicalization feeds only the `fire` wet-rib recursion-guard key, and no parity wet arm is a `^:` whose spec carries `%gist`. |
| L3111-3115 `hoon_noun_tag` | P | Diagnostics only: formats "ast missing" errors (`repo.rs:75`, `mod.rs` 9157 and 9164). |

## `lower_*` desugarings

| Gaps | Tag | Reason |
|---|---|---|
| L3119 (`:*` with no items) | P | Unreachable from source: `:*` takes one or more hoons in both parsers. |
| L3124, B3123 F | UP | Unreachable: `split_first` already proved there are two or more items. |
| L3327-3328, L3336, B3326 T (`feck` finds `[%sand %tas @]`) | P | Unreachable from source: hoon-138 builds `[%sand %tas @]` only as a path element (`++hasp`, `++limp`) inside a `%clsg` list, never as a bare `~\|` operand. |
| L3457, B3456 F (`;~` with no rules) | P | Unreachable from source: `;~` takes a hoon plus one or more rules. |
| L3513-3519 `prefix_signature` with a prefix | P | Unreachable from source: `%brcn` and `%brpt` prefixes are always `~` from the parser. |

## Memo context keys and boundary caches

| Gaps | Tag | Reason |
|---|---|---|
| L3584-3599, B3582 F, B3608 F (placeholder set non-empty) | P | Unreachable in production: nothing inserts into `arm_placeholder_play_in_progress`. |
| B3607 F (a goal in progress with `arm_in_progress` empty) | P | Unreachable in production: `build_arm_formula_direct` pushes both sets together (8106-8107). |
| L3631, L3636-3646 `mint_cache_key`; L3686-3695, L3697-3698, L3700-3711, L3713-3714, B3697 T/F, B3703 T/F, B3705 T/F, B3707 T/F, B3709 T/F, B3709 c2 T/F, B3709 c3 T/F `mint_boundary_lookup_exact`; L3717-3728, L3730-3745, L3747-3758, B3737 T/F, B3739 T/F, B3741 T/F, B3743 T/F, B3743 c2 T/F, B3743 c3 T/F, B3747 T/F `mint_boundary_store_exact`; L4018-4027, L4029-4031, L4033-4034, L4036-4037, B4026 T/F, B4030 T/F, B4031 T/F `nest_mug_lookup`; L4040-4056, B4052 T/F `nest_mug_register` | P | Test-only (`#[cfg(test)]`). |
| L3868, L3870, L3873, L3875, B3867 F, B3869 F, B3871 F, B3871 c2 F (`redo_boundary_lookup`); L3899-3905, L3908, B3899 T/F, B3901 T/F, B3903 T/F, B3903 c2 T/F, B3907 T (`redo_boundary_store`) | P | Performance cache only: a lookup precedes every store and nouns are hash-consed, so the corpus stores only into empty buckets and every lookup that finds a bucket matches by address. |
| L3938, L3943, L3945, B3937 F, B3939 T, B3941 F, B3941 c2 F (`rest_boundary_lookup`); L3969-3975, L3978, B3969 T/F, B3971 T/F, B3973 T/F, B3973 c2 T/F, B3977 T (`rest_boundary_store`) | P | Performance cache only: same shape as the redo cache; the legs list is rebuilt per call, so it matches structurally rather than by address. |

## `mint`, `mint_inner`, and `play_inner` dispatch

| Gaps | Tag | Reason |
|---|---|---|
| L4127, B4123 F; L4347, B4345 F (`cache_sig` is `None`) | UP | Unreachable: every registered arena node has a signature (see B2973 F). |
| L4240 `mint` of `%hand`; L4619-4621 `play` of `%hand` | P | Unreachable from source: `%hand` comes only from noun decoding. |
| L4314-4316 (`mint` of an empty `=~`) | P | Unreachable from source: `=~` takes two or more hoons, and hatch's whole-file wrapper holds at least one. |
| L4712 (`play` of a one-item `=~`) | P | Unreachable from source: `=~` takes two or more hoons; the only one-item `=~` is hatch's whole-file wrapper, which parity runs mint and never play. |
| L4322-4323, L4716-4717 (`ok_or_else` in the multi-item `=~` arm) | UP | Unreachable: the arm matches only lists of two or more. |
| L4355, B4354 T (`slot_axis` with axis 0) | P | Unreachable in production: the only caller, `musk_mack_constant_core`, passes op-9 arm axes, which compiled formulas never set to 0. |
| B4423 F (cwd with no components) | UP | Diagnostics only: taken only when honk runs from `/`. |
| L4427, B4425 T (error path below the cwd) | P | Diagnostics only: shortens error locations. |
| L4462-4466 `play_noun` | P | Parity runs do not reach it: only the prelude seeding and isolated prelude evaluation in `bin/honk.rs` (3233, 3416) call it. |
| L4540 `play` of `%fits` | P | Unreachable from source: `%fits` appears only as a `?:` condition (from `?=` opening and mold `factory`), and `play` of `%wtcl` never plays its condition. |
