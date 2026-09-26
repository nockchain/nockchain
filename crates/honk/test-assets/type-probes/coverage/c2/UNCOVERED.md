# c2 ledger: uncovered branches in `crates/honk/src/native/ut/mod.rs` 4954-9849

This range holds `fish` and skin tests, seminouns, lazy resolvers and musk,
`bran`, `%=`, the hoon/spec/open caches, core minting, literals, `nest`,
`wrap_type`, and `burp_type`.

The gap report lists 319 uncovered lines and 118 untaken branch outcomes: none
are missed only by unit tests (U), 186 lines and 59 branches are missed only
by the parity corpus (P), and 133 lines and 59 branches are missed by both (UP).
`L` rows are lines never run; `B` rows are branch outcomes never taken (`T` or
`F`, with `c2` for the second condition of the same `if`). The parity corpus
also compiles the rejection probes in `../../reject/`, so a rejection path is
covered there once a reject probe reaches it.

## Error edges

| Gaps | Tag | Reason |
|---|---|---|
| L5822 (`lazy_resolver_compile_arm`), L7859 (`mint_core`), L8172, L8219 (lazy-resolver arm-map walkers), L9581 (`burp_type`) | UP | Defensive: the callee fails only on a malformed noun (an axis of 0, a map or type noun that does not decode, a duplicate arm axis). |

## Skins (`?#`, `ar:fish`)

| Gaps | Tag | Reason |
|---|---|---|
| L5332-5334 | P | Test-only: `#[cfg(test)]` call counter. |
| L5370-5371 (`%base %null` skin), L5373 (`%base %void` skin) | P | Unreachable from source: `flay` turns `~` into a `%leaf` skin and rejects `!!` (`%zpzp`), so no source skin is `%base %null` or `%base %void`. |
| L5414-5415 (`%dbug` and `%help` skins) | P | Unreachable from source: `flay` never builds `%dbug`, and no source form puts a `%help` note in skin position. |
| L5426, B5425 T | P | Rejected by both compilers: a `%spec` skin whose example does not nest the tested type (hoonc `nest-fail`, honk `wthx spec`). |

## Seminouns, lazy resolvers, and musk

| Gaps | Tag | Reason |
|---|---|---|
| L5625, B5624 T (`semi_blocks_root_blocked` cache hit) | P | Performance cache only: the only caller, `semi_noun_blocked_encoded`, memoizes its own result. |
| L5661, B5660 T | P | Unreachable in practice: resolver-ID wraparound needs 2^64 IDs. |
| L5777, B5770 F (`lazy_resolver_resolve_axis`) | P | Defensive: every resolver ID honk makes is registered when its lazy root is built (`mint_core` and `mull_mile`), so only a decoded type noun could carry an unknown one. |
| L5788, B5787 F; L5791, B5790 T; L5805-5808, B5804 F, B5805 T (`lazy_resolver_compile_arm` guards) | P | Unreachable in production: the only caller, `lazy_resolver_resolve_axis`, has just checked that the ID is registered, the axis is not cached, and the axis is an arm. |
| L5794, B5793 T; L5825-5828, B5816 T, B5825 T; L7416-7421, B7413 T, B7414 F, B7418 T/F (`arm_goal_for_hoon_in_progress`) | P | A `^~` fold inside an arm that needs that same arm's formula. hoon-138's `++laze` recurses without bound on such code, so hoonc gives no verdict to compare. |
| L5802, B5786 F | UP | Dead: the block before the `else` always returns early or yields `Some`. |
| B5805 F, B5825 F, B5844 F, B5852 F | UP | Unreachable: resolvers are never unregistered, so the context cannot disappear during a compile. |
| L5833-5834 | P | Defensive: an arm gene that does not decode. |
| L5899-5912, L5914-5916 (`%half` mask import) | P | Unreachable in production: honk never encodes a `%half` mask (coil seminouns are `[%full ~]`, blocked, or the `%lazy` root). |
| L5922-5923, L5925-5926, L5928-5929, L5933-5934, L5939 | P | Defensive: malformed `%lazy` masks and unknown mask tags. |
| L6026, L6029, B6025 T, B6028 T (`semi_fragment_big`); L6108, L6111, B6107 T, B6110 T (`semi_mutate_big`) | UP | Dead: the wide-axis walkers hand every u64 axis to the small walker first, so their axis 0 and 1 checks never run. |
| L6084, B6083 F (`semi_mutate`); L6118, B6117 F (`semi_mutate_big`) | UP | Dead: when a target's head fragment exists, its tail fragment does too. |
| L6188, B6187 T (`musk-loop`) | P | Re-entering the same dynamic nock state. hoon-138's `++araw` recurses without bound here, so hoonc gives no verdict to compare. |
| L6224 | UP | Dead: a formula head that is not a cell is an atom. |
| L6397, B6396 F; L6410, B6409 F; L6451, B6450 F | UP | Dead: `axis_big` is `Some` whenever `axis_small` is `None`. |
| L6469 | UP | Dead: an op-11 hint that is not an atom is a cell. |
| L6503-6508, L6510 `semi_complete_value_id` | P | Test-only (`#[cfg(test)]`). |
| L6593-6595, L6608, B6606 F, L6643-6645 | P | Defensive: recovery after the eval stack is exhausted during mack or the core copy (resource exhaustion only). |
| L9599, L9603, L9606, B9598 F, B9602 F, B9605 F (`semi_is_full_complete`) | P | Defensive: malformed seminoun nouns. |

## `bran`

| Gaps | Tag | Reason |
|---|---|---|
| L6701, B6700 T (`bran_seen_holds_equal` length mismatch) | P | Unreachable in production: the cache key includes the seen-hold count, so entries in one bucket have equal lengths. |
| L6729, L6731, B6726 F | P | Performance cache only: needs a seen-hold signature collision. |
| B6725 F | UP | Dead: the key is the subject's arena ID, so a bucket entry always holds the same subject pointer. |
| L6740, B6739 T | P | Performance cache only: bucket overflow at 8 entries per key. |
| L6821-6822 | UP | Defensive: a core `rest` that is not a cell. |
| L6830 | UP | Dead: `repo` of a face or hint never fails. |
| L6840 | P | Defensive: a hold whose expansion fails to play; holds built from compiled arms play. |
| L6835, B6834 T | UP | Dead: repeats the seen-hold check that `bran_canonical_semi_inner` makes just before. |

## `%=` and `%hand`

| Gaps | Tag | Reason |
|---|---|---|
| L7097, B7096 T (`cnts_toss`) | P | Rejected by both compilers: `mate`, raised when an arm edit meets arms whose sample axes differ (hoon-138's `++toss` asserts the same). |
| L7158-7159, B7156 F (`play_cnts`) | P | Rejected by both compilers: `%=` edits through a synthetic alias port in play position (both fail with `hoon`). |
| L7192-7198 `mint_hand` | P | Unreachable from source: `%hand` comes only from noun decoding. |

## Caches (performance only)

| Gaps | Tag | Reason |
|---|---|---|
| B7567 F (`cache_hoon_ast_ptr`); B7588 F, B7605 F (`cache_hoon_ast_for_node`); B7639 F (`spec_example_cached`); B7684 F (`spec_factory_open_cached`); B7731 F (`open_cached`); L8649, B8645 F (`decode_hold_hoon_ast`) | UP | Unreachable: the order queue is non-empty whenever its length exceeds the limit. |
| L7625, B7624 F (`spec_example_cached`); L7654-7661, B7653 F (`spec_factory_open_cached`); L7699-7700, B7698 F (`open_cached`) | UP | Unreachable: the signature writers never return `None`. |
| L7631, L7643, B7629 F, B7636 F (`spec_example_cached`); L7668, L7688, B7666 F, B7681 F (`spec_factory_open_cached`); L8630, L8632, B8628 F (`hoon_ast_lookup_cached`) | UP | Performance cache only: needs a signature or mug collision between unequal specs or genes. |
| L7646, B7645 T (`spec_example_cached`); L7691, B7690 T (`spec_factory_open_cached`) | UP | Performance cache only: bucket overflow needs 8 colliding specs. |
| L7605-7607, B7604 T, B7605 T (`cache_hoon_ast_for_node`); L7684-7686, B7683 T, B7684 T (`spec_factory_open_cached`); L8645-8648, B8644 T, B8645 T (`decode_hold_hoon_ast`) | P | Performance cache only: key eviction past 16,409 entries. |
| L8651, B8642 F (`decode_hold_hoon_ast`) | UP | Dead: the raw-map hit returns before this check, so the key is never already present. |
| L7577, B7576 T (`cache_hoon_ast_for_node`); L8612, B8611 T (`hoon_ast_lookup_cached`); L8681, B8677 F (`hoon_noun_for_node`) | P | Parity runs cannot take these: they run only when `exact_hoon_ast_lookup_enabled` is off, and every honk entry point turns it on. |
| L8638, B8637 T (`decode_hold_hoon_ast` raw hit) | P | Parity runs cannot take it: `hoon_ast_lookup_cached` checks the same map first unless exact lookup is off. |
| L8679, B8678 T (`hoon_noun_for_node`) | P | Performance cache only: needs `hoon_noun_for_node` on a gene decoded from a hold noun; no parity probe does this. |
| B8691 F (`hoon_noun_for_node`) | UP | Performance cache only: the arena registers every Hoon node under its root, so this lookup does not miss. |
| B9589 F (`burp_type`) | UP | Performance cache only: the burp cache stops growing at 65,536 entries. |

## Cores

| Gaps | Tag | Reason |
|---|---|---|
| L7769, 7772-7775, 7777-7783, 7785-7797, 7799, B7768 F, B7769 T, B7772 T/F, B7785 T/F (`mint_core` cache hit and its `HONK_MEMO_VERIFY` check) | UP | Performance cache only: the per-gen mint cache has the same key components and answers first, so the core mint cache never hits. |
| L7819, B7818 T | UP | Dead: `check_goal_core_chapter_counts` already rejects a chapter-count mismatch. |
| L7910-7911, B7909 T (`mine`) | UP | Dead: `mine` is called only with `%gold`. |
| L8007, B8004 F (`core-nice`); L8016, B8015 T (`core-number-of-chapters`); L8090, B8089 F (`unexpcted-chapter`); L8112, B8111 F (`unexpected-arm`); L8255, B8254 T (`core-number-of-arms`) | P | Rejected by both compilers: hoon-138's `core-check`, `chapters-check`, `get-arms`, `get-arm-type`, and `arms-check` reject the same cases. |
| L8043 | UP | Dead: every type noun carries one of the nine type tags, all matched above. |
| L8070, B8069 T; L8077 (`goal_core_for_mine`) | P | Defensive: the guard against a self-referential hold goal and the 128-step walk bound. |
| L8115-8116 | P | Defensive: a goal arm gene that does not decode. |
| L8129, B8128 F; L8384, B8383 F | P | Unreachable from source: an empty arm map; a parsed chapter always has at least one arm. |
| L8155-8157, B8145 T | UP | Defensive: a duplicate arm axis, which a well-formed map cannot have. |
| L8309-8310 (`with_arm_context`) | P | Diagnostics only: rewraps an arm's error message with the arm name. |
| L8312-8316 (`with_arm_context`) | UP | Diagnostics only: the same rewrapping for the other error kinds. |
| L8352-8357, L8359-8365 | UP | `debug_assert_eq!` arguments, compiled out in the instrumented builds. |
| L8457-8460, B8455 F (`mint_opened`); L8508-8511, B8506 F (`play_opened`) | UP | Defensive: "unsupported" fallbacks; every Hoon variant is minted or played directly or changed by `open`. |

## Literals

| Gaps | Tag | Reason |
|---|---|---|
| L8550, B8549 T (`sand-null`); L8557, B8556 T (`sand-flag`) | P | Unreachable from source: the parser builds `%n` and `%f` sands only with 0 (tic-aura casts). |
| L8563 | P | Unreachable from source: the parser never builds a `%sand` with a cell value. |
| L8597-8608, B8604 T/F, B8604 c2 T/F `hint_type_n` | P | Test-only (`#[cfg(test)]`). |

## Nesting and wrapping

| Gaps | Tag | Reason |
|---|---|---|
| L8771-8773 | P | Defensive: fork options requested for a non-fork. |
| L8790, B8784 F | UP | Dead: an `unreachable!()` after re-matching the same enum. |
| L9119, L9128 (`nest_core` with a non-core) | P | Defensive: the only caller dispatches on core/core pairs. |
| L9410-9415, L9417-9422 (`nest_deep_arms`) | P | Defensive: an arm gene that does not decode. |
| L9451, B9450 c2 T (`wrap-core`) | P | Rejected by both compilers: re-wrapping a non-gold core with `^\|` or `^&` (hoonc fails in `++wrap`). |

## `HONK_MEMO_VERIFY`

| Gaps | Tag | Reason |
|---|---|---|
| `type_test_formula_on_axis` L5237-5241, B5236 T, B5240 T; `blow_ktsg` L5556-5562, 5567, B5555 T; `bran_canonical_semi_inner` L6767-6772, B6766 T, B6772 T; `open_cached` L7716-7718, B7715 T; `nest` L8710-8713, 8715-8718, 8720-8723, B8709 T, B8723 T | P | Diagnostics only: the parity corpus runs without `HONK_MEMO_VERIFY`; `c6_cli_memo_verify_rechecks_cache_hits_without_changing_the_artifact` covers it. |
| `type_test_formula_on_axis` L5242-5243, B5240 F; `blow_ktsg` L5564-5565, 5568-5571, 5573-5574, 5576, 5578-5579, B5573 T/F; `bran_canonical_semi_inner` L6773-6774, 6776-6777, 6779, B6772 F; `open_cached` L7719-7720; `nest` L8724, B8723 F | UP | Diagnostics only: a recompute that disagrees with its cached hit, or changes the cache context beyond the arm epoch, which no build produces. |
| `burp_type` L9507-9516, 9520, B9504 F, B9506 T, B9510 T/F, B9514 T/F | UP | Diagnostics only: the verify CLI test's program never hits the burp cache, and the parity corpus runs without `HONK_MEMO_VERIFY`. |
