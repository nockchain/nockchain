# c2 ledger: uncovered branches in `crates/honk/src/native/ut/mod.rs` 4818-9582

This range holds `fish` and skin tests, seminouns, lazy resolvers and musk,
`bran`, `%=`, the hoon/spec/open caches, core minting, literals, `nest`,
`wrap_type`, and `burp_type`.

The gap report lists 247 uncovered lines and 95 untaken branch outcomes: none
are missed only by unit tests (U), 152 lines and 52 branches are missed only
by the parity corpus (P), and 95 lines and 43 branches are missed by both (UP).
`L` rows are lines never run; `B` rows are branch outcomes never taken (`T` or
`F`, with `c2` for the second condition of the same `if`). The parity corpus
holds only programs both compilers accept, so every rejection path is at least
P.

## Error edges

| Gaps | Tag | Reason |
|---|---|---|
| L5636 (`lazy_resolver_compile_arm`), L7621 (`mint_core`), L7934, L7981 (lazy-resolver arm-map walkers), L9314 (`burp_type`) | UP | Defensive: the callee fails only on a malformed noun (an axis of 0, a map or type noun that does not decode, a duplicate arm axis). |
| L5142 (`type_test_formula_on_axis_inner`) | UP | Rejection path: a cell type whose head fails to fish (`fish-core`, `fish-loop`), which hoon-138's `++fish` rejects too. Uncovered: no test exercises it yet. |
| L8037, L8043, L8053, L8056 (`build_tomes_battery_from_maps`), L8193 (`build_arms_battery_from_map`) | UP | Rejection path: an arm that fails to compile in a non-root chapter, or in the left subtree of an arm-map node with two children. Uncovered: no test exercises it yet. |
| L8641, L8719 (`nest_inner_impl`), L8807 (`nest_sint`), L8904, L8915, L8930 (`nest_core`), L9015 (`nest_meet`), L9063, L9076 (`nest_deep_tomes`), L9138, L9151 (`nest_deep_arms`) | UP | Err edge of a recursive `nest`. Apart from malformed input, `nest` fails only when a `%hold` expansion or a deep-compared arm fails to play. Uncovered: no test exercises it yet. |

## Skins (`?#`, `ar:fish`)

| Gaps | Tag | Reason |
|---|---|---|
| L5187-5189 | P | Test-only: `#[cfg(test)]` call counter. |
| L5225-5226 (`%base %null` skin), L5228 (`%base %void` skin) | P | Unreachable from source: `flay` turns `~` into a `%leaf` skin and rejects `!!` (`%zpzp`), so no source skin is `%base %null` or `%base %void`. |
| L5269-5270 (`%dbug` and `%help` skins) | P | Unreachable from source: `flay` never builds `%dbug`, and no source form puts a `%help` note in skin position. |
| L5281, B5280 T | P | Rejected by both compilers: a `%spec` skin whose example does not nest the tested type (hoonc `nest-fail`, honk `wthx spec`). |

## Seminouns, lazy resolvers, and musk

| Gaps | Tag | Reason |
|---|---|---|
| L5454, B5453 T (`semi_blocks_root_blocked` cache hit) | P | Performance cache only: the only caller, `semi_noun_blocked_encoded`, memoizes its own result. |
| L5490, B5489 T | P | Unreachable in practice: resolver-ID wraparound needs 2^64 IDs. |
| L5591, B5584 F (`lazy_resolver_resolve_axis`) | P | A resolver ID with no registered context (only `mull_lazy_resolver_id` makes one, for cores built by `mull`). Uncovered: no parity probe exercises it yet. |
| L5602, B5601 F; L5605, B5604 T; L5619-5622, B5618 F, B5619 T (`lazy_resolver_compile_arm` guards) | P | Unreachable in production: the only caller, `lazy_resolver_resolve_axis`, has just checked that the ID is registered, the axis is not cached, and the axis is an arm. |
| L5608, B5607 T; L5639-5642, B5630 T, B5639 T; L7214-7219, B7211 T, B7212 F, B7216 T/F (`arm_goal_for_hoon_in_progress`) | P | A `^~` fold inside an arm that needs that same arm's formula. hoon-138's `++laze` recurses without bound on such code, so hoonc gives no verdict to compare. |
| L5616, B5600 F | UP | Dead: the block before the `else` always returns early or yields `Some`. |
| B5619 F, B5639 F, B5658 F, B5666 F | UP | Unreachable: resolvers are never unregistered, so the context cannot disappear during a compile. |
| L5647-5648 | P | Defensive: an arm gene that does not decode. |
| L5713-5726, L5728-5730 (`%half` mask import) | P | Unreachable in production: honk never encodes a `%half` mask (coil seminouns are `[%full ~]`, blocked, or the `%lazy` root). |
| L5736-5737, L5739-5740, L5742-5743, L5747-5748, L5753 | P | Defensive: malformed `%lazy` masks and unknown mask tags. |
| L5840, L5843, B5839 T, B5842 T (`semi_fragment_big`); L5922, L5925, B5921 T, B5924 T (`semi_mutate_big`) | UP | Dead: the wide-axis walkers hand every u64 axis to the small walker first, so their axis 0 and 1 checks never run. |
| L5898, B5897 F (`semi_mutate`); L5932, B5931 F (`semi_mutate_big`) | UP | Dead: when a target's head fragment exists, its tail fragment does too. |
| L6002, B6001 T (`musk-loop`) | P | Re-entering the same dynamic nock state. hoon-138's `++araw` recurses without bound here, so hoonc gives no verdict to compare. |
| L6038 | UP | Dead: a formula head that is not a cell is an atom. |
| L6211, B6210 F; L6224, B6223 F; L6265, B6264 F | UP | Dead: `axis_big` is `Some` whenever `axis_small` is `None`. |
| L6283 | UP | Dead: an op-11 hint that is not an atom is a cell. |
| L6317-6322, L6324 `semi_complete_value_id` | P | Test-only (`#[cfg(test)]`). |
| L6407-6409, L6422, B6420 F, L6457-6459 | P | Defensive: recovery after the eval stack is exhausted during mack or the core copy (resource exhaustion only). |
| L9332, L9336, L9339, B9331 F, B9335 F, B9338 F (`semi_is_full_complete`) | P | Defensive: malformed seminoun nouns. |

## `bran`

| Gaps | Tag | Reason |
|---|---|---|
| L6515, B6514 T (`bran_seen_holds_equal` length mismatch) | P | Unreachable in production: the cache key includes the seen-hold count, so entries in one bucket have equal lengths. |
| L6543, L6545, B6540 F | P | Performance cache only: needs a seen-hold signature collision. |
| B6539 F | UP | Dead: the key is the subject's arena ID, so a bucket entry always holds the same subject pointer. |
| L6554, B6553 T | P | Performance cache only: bucket overflow at 8 entries per key. |
| L6619-6620 | UP | Defensive: a core `rest` that is not a cell. |
| L6628 | UP | Dead: `repo` of a face or hint never fails. |
| L6638 | P | Defensive: a hold whose expansion fails to play; holds built from compiled arms play. |
| L6633, B6632 T | UP | Dead: repeats the seen-hold check that `bran_canonical_semi_inner` makes just before. |

## `%=` and `%hand`

| Gaps | Tag | Reason |
|---|---|---|
| L6895, B6894 T (`cnts_toss`) | P | Rejected by both compilers: `mate`, raised when an arm edit meets arms whose sample axes differ (hoon-138's `++toss` asserts the same). |
| L6956-6957, B6954 F (`play_cnts`) | P | Rejected by both compilers: `%=` edits through a synthetic alias port in play position (both fail with `hoon`). |
| L6990-6996 `mint_hand` | P | Unreachable from source: `%hand` comes only from noun decoding. |

## Caches (performance only)

| Gaps | Tag | Reason |
|---|---|---|
| B7365 F (`cache_hoon_ast_ptr`); B7386 F, B7403 F (`cache_hoon_ast_for_node`); B7437 F (`spec_example_cached`); B7482 F (`spec_factory_open_cached`); B7521 F (`open_cached`); L8414, B8410 F (`decode_hold_hoon_ast`) | UP | Unreachable: the order queue is non-empty whenever its length exceeds the limit. |
| L7423, B7422 F (`spec_example_cached`); L7452-7459, B7451 F (`spec_factory_open_cached`); L7497-7498, B7496 F (`open_cached`) | UP | Unreachable: the signature writers never return `None`. |
| L7429, L7441, B7427 F, B7434 F (`spec_example_cached`); L7466, L7486, B7464 F, B7479 F (`spec_factory_open_cached`); L8395, L8397, B8393 F (`hoon_ast_lookup_cached`) | UP | Performance cache only: needs a signature or mug collision between unequal specs or genes. |
| L7444, B7443 T (`spec_example_cached`); L7489, B7488 T (`spec_factory_open_cached`) | UP | Performance cache only: bucket overflow needs 8 colliding specs. |
| L7403-7405, B7402 T, B7403 T (`cache_hoon_ast_for_node`); L7482-7484, B7481 T, B7482 T (`spec_factory_open_cached`); L8410-8413, B8409 T, B8410 T (`decode_hold_hoon_ast`) | P | Performance cache only: key eviction past 16,384 entries. |
| L8416, B8407 F (`decode_hold_hoon_ast`) | UP | Dead: the raw-map hit returns before this check, so the key is never already present. |
| L7375, B7374 T (`cache_hoon_ast_for_node`); L8377, B8376 T (`hoon_ast_lookup_cached`); L8446, B8442 F (`hoon_noun_for_node`) | P | Parity runs cannot take these: they run only when `exact_hoon_ast_lookup_enabled` is off, and every honk entry point turns it on. |
| L8403, B8402 T (`decode_hold_hoon_ast` raw hit) | P | Parity runs cannot take it: `hoon_ast_lookup_cached` checks the same map first unless exact lookup is off. |
| L8444, B8443 T (`hoon_noun_for_node`) | P | Performance cache only: needs `hoon_noun_for_node` on a gene decoded from a hold noun; no parity probe does this. |
| B8456 F (`hoon_noun_for_node`) | UP | Performance cache only: the arena registers every Hoon node under its root, so this lookup does not miss. |
| B9322 F (`burp_type`) | UP | Performance cache only: the burp cache stops growing at 65,536 entries. |

## Cores

| Gaps | Tag | Reason |
|---|---|---|
| L7558, L7561, B7558 T (`mint_core` cache hit) | UP | Performance cache only: the per-gen mint cache has the same key components and answers first. |
| L7582, B7581 T | UP | Dead: `check_goal_core_chapter_counts` already rejects a chapter-count mismatch. |
| L7672-7673, B7671 T (`mine`) | UP | Dead: `mine` is called only with `%gold`. |
| L7769, B7766 F (`core-nice`); L7778, B7777 T (`core-number-of-chapters`); L7852, B7851 F (`unexpcted-chapter`); L7874, B7873 F (`unexpected-arm`); L8017, B8016 T (`core-number-of-arms`) | P | Rejected by both compilers: hoon-138's `core-check`, `chapters-check`, `get-arms`, `get-arm-type`, and `arms-check` reject the same cases. |
| L7805 | UP | Dead: every type noun carries one of the nine type tags, all matched above. |
| L7832, B7831 T; L7839 (`goal_core_for_mine`) | P | Defensive: the guard against a self-referential hold goal and the 128-step walk bound. |
| L7877-7878 | P | Defensive: a goal arm gene that does not decode. |
| L7891, B7890 F; L8146, B8145 F | P | Unreachable from source: an empty arm map; a parsed chapter always has at least one arm. |
| L7917-7919, B7907 T | UP | Defensive: a duplicate arm axis, which a well-formed map cannot have. |
| L8071-8072 (`with_arm_context`) | P | Diagnostics only: rewraps an arm's error message with the arm name. |
| L8074-8078 (`with_arm_context`) | UP | Diagnostics only: the same rewrapping for the other error kinds. |
| L8114-8119, L8121-8127 | UP | `debug_assert_eq!` arguments, compiled out in the instrumented builds. |
| L8219-8222, B8217 F (`mint_opened`); L8273-8276, B8271 F (`play_opened`) | UP | Defensive: "unsupported" fallbacks; every Hoon variant is minted or played directly or changed by `open`. |

## Literals

| Gaps | Tag | Reason |
|---|---|---|
| L8315, B8314 T (`sand-null`); L8322, B8321 T (`sand-flag`) | P | Unreachable from source: the parser builds `%n` and `%f` sands only with 0 (tic-aura casts). |
| L8328 | P | Unreachable from source: the parser never builds a `%sand` with a cell value. |
| L8362-8373, B8369 T/F, B8369 c2 T/F `hint_type_n` | P | Test-only (`#[cfg(test)]`). |

## Nesting and wrapping

| Gaps | Tag | Reason |
|---|---|---|
| L8518-8520 | P | Defensive: fork options requested for a non-fork. |
| L8537, B8531 F | UP | Dead: an `unreachable!()` after re-matching the same enum. |
| L8742, B8741 T (`nest_inner_impl`) | P | Degenerate `sut` (hoon-138's `seg` check in `++nest`): a `%hold` whose expansion reaches itself again without passing through a cell answers no. Uncovered: no parity probe exercises it yet. |
| L8866, L8875 (`nest_core` with a non-core) | P | Defensive: the only caller dispatches on core/core pairs. |
| L9157-9162, L9164-9169 (`nest_deep_arms`) | P | Defensive: an arm gene that does not decode. |
| L9198, B9197 c2 T (`wrap-core`) | P | Rejected by both compilers: re-wrapping a non-gold core with `^\|` or `^&` (hoonc fails in `++wrap`). |
