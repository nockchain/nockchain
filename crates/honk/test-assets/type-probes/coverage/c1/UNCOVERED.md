# c1 ledger: uncovered branches in `crates/honk/src/native/ut/mod.rs` 1-4799

Scope: the Hoon arena and `Sig64` signatures, `Ut` construction and memo
plumbing, fan-context keys, boundary caches, the `lower_*` helpers, musk
setup, and `mint_inner`/`play_inner` dispatch.

Tags: `U` = not covered by `cargo test` (after `cov/c1_ut_a.rs`), `P` = not
covered by the parity corpus plus the `coverage/c1/*.hoon` probes. Line
numbers match this worktree.

Unit tests cover everything in this range except the `U` rows below. Most
`P` rows are covered by the unit tests in `cov/c1_ut_a.rs`, named per row.

## Reason key

- **unreachable**: no input or caller can take the branch (reason given).
- **no-source**: reachable only from ASTs the Hoon parser never produces
  (hoonc's grammar and hatch agree), so no probe exists.
- **perf**: cache, memo, or signature plumbing. The branch cannot change a
  type, formula, or verdict; covered by unit tests.
- **diag**: error text or source locations only.
- **defensive**: malformed-input or out-of-memory guard.
- **divergence**: blocked by a divergence recorded under `divergent/`.
- **test-only**: `#[cfg(test)]` code, not compiled into honk, so parity
  cannot reach it.

## `HoonArena` and `Sig64` (lines 184-2050)

| Lines / branches | Tag | Reason |
|---|---|---|
| 184-191 `register_unsigned_root` | P | unreachable in production: `Sig64` never fails, so `enter_hoon_ast_scope` always registers a signed arena. Unit: `hoon_arena_unsigned_root_registers_a_single_unsigned_entry`. |
| 225-227 `child_count` | P | test-only. |
| 640T/F, 686F, 1383F `include_dbug_spot` false | P | unreachable in production: every `Sig64` is built spot-sensitive (`new_with_dbug_spots(true)`, `hoon_signatures_spot_sensitive_pooled`). Unit: `sig64_spot_insensitive_mode_ignores_dbug_spots`. |
| 1351T, 1352-1354 memo hit in `write_hoon` | P | unreachable in production: one tree walk never reaches the same `&Hoon` twice. Unit: `sig64_reuses_a_digest_for_a_node_written_twice`. |
| 582 `BaseType::Void`; 610-615 `Note::Made` with wings; 638-643 `Skin::Dbug`; 709-720 `Spec::Loop`; 745-753 `BucBuc`; 781-789 `BucDot`; 812-820 `BucFas`; 837-845 `BucTic`; 866-874 `BucZap`; 932, 941 `Tune` with a `None` entry or a list; 1022-1026 `Mane::TagSpace`; 1062-1064 `TunaTail::Call`; 1817-1822 `MicGal` | P | perf: `HoonSignature`/`SpecSignature` digests are exact cache keys and address-reuse guards and never reach output. These AST shapes are absent from the corpus and cost a new probe each (sail, core specs, `;<`). Unit: `sig64_distinguishes_every_rare_hoon_spec_skin_type_and_nock_shape`. |
| 898-904 `Chum::VenProVerKel` | P | no-source: hoon-138's `bonk` and hatch's `jet_signature` both read `%k:foo..138` as `[ven pro kel]`, so neither builds a four-field chum. Unit: zoo test. |
| 1098-1333 `write_poly/vair/garb/stencil/semi_noun_expr/coil/face_type/type/nock_hint/nock` (all arms) | P | no-source: reached only through `Hoon::Hand`, which exists only as a decoded `%hand` noun. Unit: zoo test (every `Type` x every `Nock`). |
| 1388-1395 `Hoon::Eror` / `Hoon::Hand`; 1414-1418 `Hoon::Leaf` | P | no-source: `Eror` comes only from `lower_micsig([])`, and `Hand`/`Leaf` only from noun decoding. Unit: zoo test. |
| 1496-1499, 1547-1550 `BarCen`/`BarPat` with `Some(prefix)` | P | no-source: the parser always emits `%brcn`/`%brpt` with `p=~`. Unit: zoo test. |
| 1768-1771 `Hoon::SigBuc` (`~$`) | P | divergence: hatch has no `~$` rune (`divergent/c1_sigbuc_parse.hoon`). Unit: zoo test. |
| 1335-1347 `write_zpwt_arg`, 2047-2050 `Hoon::ZapWut` (`!?`) | P | divergence: hatch encodes `!?`'s version wrongly (`divergent/c1_zpwt_noun.hoon`) and panics on the pair form (`divergent/c1_zpwt_range.hoon`). Unit: zoo test. |

## Musk eval-stack cache (2230-2331)

| Lines / branches | Tag | Reason |
|---|---|---|
| 2271, 2275-2278, 2277T | P | defensive: rollback when the eval stack runs out of memory during a core copy. Existing unit tests cover it. |
| 2277F, 2280 | U P | defensive: re-raises a non-`AllocationError` panic from the copy. Nothing the copy runs panics any other way. |
| 2308F, 2325F | P | perf: a copy without a slab-side mug to carry over. Production mugs the core first (`noun_mug_cached`). Unit: `musk_eval_stack_copy_shares_repeated_indirect_atoms`. |
| 2309F, 2313-2314, 2326F, 2330-2331 | U P | unreachable: a copied indirect atom or a freshly allocated cell is always allocated. |

## Build memos, fan legs, and fan context keys (2356-2907)

| Lines / branches | Tag | Reason |
|---|---|---|
| 2356-2370 `clear_build_memos` | P | perf: called only on the native prelude-mint path (`bin/honk.rs` ~3198/3224), never for `--arbitrary` entries. Unit: `clear_build_memos_resets_every_build_scoped_cache`. |
| 2444T, 2445, 2451T, 2454-2467, 2455T/F, 2457T/F, 2460T/F, 2462T/F, 2471T, 2472 (`hold_repo_fan_leg_lookup_id`/`intern_id`) | P | perf: leg-ID interning falls back to structural matching only when a structurally equal `[inner gene]` pair arrives at a new address. Hold nouns come from hash-consed `live_to_noun`, so the raw-hold map always answers first. Unit: `fan_leg_lookup_matches_structurally_and_skips_colliding_entries`. |
| 2481T, 2482; 2779T, 2780; 2874T, 2875 (ID counters wrapping to 0) | P | unreachable in practice: needs 2^64 interned IDs. Unit: `fan_leg_and_context_ids_wrap_past_zero`. |
| 2502F, 2508T, 2511T, 2511-2515 (raw-hold store: re-store and eviction past 65,536 keys) | P | perf: bounded memo. Unit: `fan_leg_hold_raw_store_dedupes_and_evicts_oldest`. |
| 2511F, 2556F (`pop_front` returning `None` when over the limit) | U P | unreachable: the queue is non-empty whenever its length exceeds the limit. |
| 2527T, 2529T, 2530-2536, 2534T, 2595T, 2596 (mug-hold lookup hits) | P | perf: as above, the raw-hold map hits first in production. Unit: `fan_leg_hold_mug_store_and_lookup_compare_structurally`. |
| 2553T, 2556T, 2556-2558, 2566T, 2568T, 2569, 2572T, 2573 (mug-hold store dedupe, bucket and key eviction) | P | perf: only mug collisions or 65,536+ distinct holds reach these. Unit: the mug-store test and `fan_leg_hold_mug_store_evicts_the_oldest_key`. |
| 2610T, 2611 (`hold_repo_fan_leg_id_by_ptr` hit) | P | perf: `reachable_legs` memoizes per type ID, so a second lookup never reaches this map in production. Existing unit tests cover it. |
| 2613F, 2614-2616 (leg ID of a non-hold) | P | defensive: `reachable_legs_node` calls it only on `NTy::Hold`. Unit: `fan_leg_id_for_native_hold_rejects_non_holds`. |
| 2730T (`intersect_sorted_legs` on an empty side) | P | perf: needs an empty reachable-leg set while the fan is non-empty (callers return 0 first when the fan is empty). The corpus never does this. Unit: `fan_subset_and_context_ids_ignore_signature_collisions`. |
| 2754T, 2755 (empty subset) | P | unreachable from callers: both scoped-key functions return 0 before interning an empty intersection. Covered by unit tests. |
| 2771F, 2773 (subset signature collision) | P | perf: needs a (sum, xor, len) collision between distinct leg sets. Unit: the collision test above. |
| 2803T/2804, 2821T/2822, 2841T/2842 (`!scoped_fan_enabled()`) | U P | unreachable: `scoped_fan_enabled()` is a constant `true`. |
| 2831F, 2833-2834 (pair-scoped key with a non-empty intersection) | P | perf: needs a mint, mull, or core-mint cache query while a `%rest` leg reachable from its subject or goal is active. Hold expansion only plays, and the corpus never does this. Existing unit tests (`native_mint_cache_partitions_on_goal_reachable_rest_fan`) cover it. |
| 2886 (activating an already-active leg) | P | unreachable in production: `with_active_rest_leg_ids` returns `rest-loop` before activating a leg that is already active. Unit: the collision test. |
| 2900F, 2907 (deactivating an inactive leg) | P | unreachable: legs are deactivated only after the same scope activated them. The branch is a `debug_assert`. Unit: `deactivating_an_inactive_fan_leg_is_a_debug_assertion`. |

## AST scopes and noun canonicalization (2958-3108)

| Lines / branches | Tag | Reason |
|---|---|---|
| 2966F, 2969-2970 | U P | unreachable: `hoon_signatures_spot_sensitive_pooled` always returns a root signature. |
| 3033F-3046 (`strip_dbug_wrapper_noun` breaks), 3055F-3062, 3066-3068, 3067F (`strip_spec_gist_wrapper_noun` breaks), 3078F-3080F, 3097, 3100 (`canonicalize_nonsemantic_hoon_noun` on a malformed noun) | P | defensive: hoon nouns reaching the wet-rib key are well-formed, so the malformed-noun breaks never fire. Unit: `nonsemantic_hoon_noun_canonicalization_strips_dbug_and_gist_wrappers`. |
| 3064F, 3067T, 3070 (stripping a real `%gist`), 3094 (a gene that is not `^:`) | P | perf (the `fire` wet-rib recursion-guard key). In the corpus, `mull_check_wet` only sees `^:` genes from `\|$` mold builders. A throwaway probe calling `\|*` gates under vet reached no new line here, so it was dropped. Unit: same test. |
| 3085F, 3088 (a `^:` whose spec has a `%gist` wrapper) | P | perf: only the `fire` wet-rib recursion-guard key uses this canonicalization, and it merges keys that differ only in `%dbug` or `%gist` wrappers, whose `mull` verdict is identical. No corpus wet arm has this shape. Unit: the canonicalization test. |
| 3104-3108 `hoon_noun_tag` | P | diag: used only to format "hold ast missing" errors (`repo.rs:75`, `mod.rs` ~9216). Unit: the canonicalization test. |

## `lower_*` desugarings (3110-3512)

| Lines / branches | Tag | Reason |
|---|---|---|
| 3112 (`:*` with no items) | P | no-source: hoon-138 `:*` takes `exps` (one or more), and hatch agrees. Unit: `lower_coltar_handles_empty_single_and_many`. |
| 3116F, 3117 | U P | unreachable: `split_first` already proved that there are two or more items. |
| 3319T, 3320-3321, 3329 (`feck` finds `[%sand %tas @]`) | P | no-source: hoon-138 emits `[%sand %tas @]` only inside path literals (`++poor`, 11522-11531), which are `%clsg` lists and never a bare `~|` operand. Unit: `lower_sigbar_uses_feck_for_tas_sand_and_a_cain_trap_otherwise`, `mint_sigbar_with_tas_sand_is_a_mean_hint`. |
| 3449F, 3450 (`;~` with no rules) | P | no-source: `;~` takes `expi` (a hoon plus one or more rules). Unit: `lower_micsig_rejects_empty_and_chains_each_rule`. |
| 3506-3512 `prefix_signature(Some(_))` | P | no-source: `%brcn`/`%brpt` prefixes are always `~` from the parser. Unit: `prefix_signature_distinguishes_named_prefixes`. |

## Memo context keys and boundary caches (3574-4049)

| Lines / branches | Tag | Reason |
|---|---|---|
| 3575F, 3577-3592, 3601F (placeholder set) | P | unreachable in production: nothing inserts into `arm_placeholder_play_in_progress`. Unit: `memo_context_keys_follow_placeholder_and_goal_recursion_state`. |
| 3600F (a goal in progress with `arm_in_progress` empty) | P | unreachable in production: `build_arm_formula_direct` (~8150) pushes and pops both together. Unit: same test. |
| 3624-3751 `mint_cache_key`, `mint_boundary_lookup_exact`, `mint_boundary_store_exact`; 4011-4049 `nest_mug_lookup`, `nest_mug_register` | P | test-only. Unit: `mint_boundary_exact_compares_structurally_and_evicts_full_buckets`, `nest_mug_memo_compares_structurally_and_evicts_full_buckets`. |
| 3860F-3868, 3864F/c2F (`redo_boundary_lookup` structural fallback and full-bucket miss); 3892-3898, 3892T/F-3896c2T/F, 3900T, 3901 (`redo_boundary_store` dedupe and eviction) | P | perf: a lookup precedes every store, and nouns are hash-consed, so the corpus stores only into empty buckets and hits only by raw identity. Unit: `redo_boundary_compares_structurally_and_evicts_full_buckets`. |
| 3930F-3938, 3932T, 3934F/c2F; 3962-3968, 3962T/F-3966c2T/F, 3970T, 3971 (`rest_boundary_*`, same shape) | P | perf: same reason. Unit: `rest_boundary_compares_structurally_and_evicts_full_buckets`. |

## `mint`, `mint_inner`, and `play_inner` dispatch (4051-4799)

| Lines / branches | Tag | Reason |
|---|---|---|
| 4116F, 4120, 4340F, 4342 (`cache_sig` is `None`) | U P | unreachable: every registered arena node has a signature (see 2966F). |
| 4235 `mint` of `%hand`; 4614-4616 `play` of `%hand` | P | no-source (`%hand` only comes from noun decoding). Unit: `mint_and_play_hand_use_the_carried_type`. |
| 4309-4311 `mint` of an empty `=~` | P | no-source: `=~` takes `expi` (two or more hoons). Unit: `mint_tissig_handles_empty_single_and_chains`. |
| 4707 `play` of a one-item `=~` | P | no-source: same grammar. hoonc and honk both reject `=~  a  ==` (checked with probe-diag). Unit: same test. |
| 4317-4318, 4711-4712 (`ok_or_else` in the multi-item `=~` arm) | U P | unreachable: the arm matches only lists of two or more. |
| 4349T, 4350 (`slot_axis` with axis 0) | P | unreachable in production: the only caller (`musk_mack_constant_core`) passes arm axes of 2 or more. Unit: `slot_axis_rejects_zero_and_walks_cells`. |
| 4418F (cwd with no components) | U P | diag: only when honk runs from `/`. |
| 4420T, 4422 (error path shortened below the cwd) | P | diag: error-location display. Unit: `dbug_path_strips_the_cwd_prefix_only_for_paths_below_it`. |
| 4535 `play` of `%fits` | P | no-source: `%fits` comes only from `?=` opening and mold `factory`/`choice_`, always as a `?:` condition, which `play %wtcl` never plays. Unit: `play_constructed_only_nodes_follow_hoon_138`. |

## Divergences found (see `divergent/`)

- `c1_sigbuc_parse.hoon`: HOONC-ONLY. hatch has no `~$` rune (hoon-138.hoon:13308).
- `c1_zpwt_noun.hoon`: MISMATCH. hatch `zpwt_arg_to_noun` (hatch/src/utils.rs:13716) encodes `!?(138 a)` as `[%zpwt [%atom '138'] ...]`; hoon-138 expects `[%zpwt 138 ...]`.
- `c1_zpwt_range.hoon`: HOONC-ONLY (honk panics). hatch `open` (utils.rs:2690) reads `!?([p q] ...)` as `[min max]`; hoon-138 (8677) requires `p >= 138 >= q`.
- `c1_bucpam_wide.hoon`: HOONC-ONLY. hatch rejects wide `$&(spec hoon)` in a `+$` arm, though the tall form parses.
