# c2 uncovered ledger (`crates/honk/src/native/ut/mod.rs` 4800-9640)

Branches in the c2 range that are still uncovered after the c2 unit tests
(`src/native/ut/cov/c2_ut_b.rs`) and the parity probes in this directory.
Tags: `U` = missed by unit tests, `P` = missed by parity probes. Line numbers
match the `cov/c2` worktree.

## Error propagation that cannot fire

The uncovered part of each `?` on these lines is the `Err` edge. The callee
fails only on malformed input, such as a peg of axis 0 or a decode of a
non-type noun, which the compiler never builds. All are defensive (UP):

- 5255 (`peg_axis_big`), 5680 (`arm_goal_for_hoon_in_progress`), 7665,
  7978, 8025, 8081, 8087, 8097, 8100, 8237 (map walkers on well-formed maps)
- 8547, 8700, 8778, 8826, 8866, 8963, 8974-8975, 8989, 9074, 9122-9123,
  9135-9136, 9197-9198, 9210-9211 (`nest_inner` recursion), 9373 (`burp_type`)

## Skins (`?#`, `ar:fish`)

- 5065, 5098-5102, B5099T/F (P): `%base %void` and `%base %null` skins do not
  come from source. `?#` skins are built by `flay`: `!!` is `%zpzp` (no flay),
  and `~` is `%bust %null`, which flays through `example` to `%leaf`. Unit-covered.
- 5181-5182, 5191-5192, 5195-5201 (P): `base_test_formula` for
  `%noun`/`%null`/`%flag` is unreachable from `mint_wthx`. `%noun` always
  matches statically, `%null` is not source-reachable (above), and
  `skin_test_formula` handles `%flag` before its `%base` arm. Unit-covered by
  direct calls.
- 5133 (P): blocked by the known divergence (cell-skin formulas in
  `skin_test_formula`). A statically known cell whose halves are both unknown
  reaches `skin_test_formula`, which keeps a `[%3 %0 axis]` guard that
  hoon-138's `flan([%1 &] ..)` drops.
- 5144-5146 (P): `#[cfg(test)]` call counter, not compiled into the parity binary.
- 5148-5149, 5323-5324 (P): `%dbug` and `%help` skins. `flay` never builds
  `%dbug`, and no `?#` source form found carries a help note in skin position.
  Unit-covered.
- 5336, B5335T (P): a `%spec` skin whose example does not nest the tested
  type. Both compilers reject (hoonc `nest-fail`, honk `wthx spec`, checked
  with `probe-diag`). Unit-covered.
- B5292F (UP, unit via a direct call): a statically false skin nested inside a
  statically unknown parent. Unreachable from `mint_wthx`:
  `cell_skin_match_static` returns `Some(false)` for the parent whenever a half
  is false, and the inner skin of a `%spec` skin is always `%base %noun`.

## Seminouns and musk (`^~`)

- 5522, B5521T (P): the cached branch of `semi_blocks_root_blocked` is dead in
  production because its only caller, `semi_noun_blocked_encoded`, memoizes
  its own result. Unit-covered.
- 5558, B5557T (P): resolver-id wraparound needs 2^64 ids. Unit-covered.
- 5634-5636, B5628F; 5646, B5645F; 5649, B5648T; 5652, B5651T; 5660, B5662F;
  5663-5666, B5663T/F (UP or P): lazy-resolver guards for an unknown id, a
  cached axis, an in-progress axis, or a non-arm axis. `semi_complete` asks
  only for ids registered by `mint_core`, and `resolve_axis` checks the cache
  and the arm map first, so `compile_arm` never sees these cases. Unit-covered
  where the code allows it.
- 5683-5686, B5674T, B5683T (P) and `arm_goal_for_hoon_in_progress`
  7258-7263, B7255T, B7256F, B7260T/F (P): a `^~` fold inside an arm that needs
  that same arm's formula. hoon-138's `++laze` recurses without bound on this
  code, so hoonc has no verdict to compare. Unit-covered.
- B5644F, B5683F, B5702F, B5710F (UP): the resolver disappears in the middle
  of a compile. Unreachable, because resolvers are never unregistered.
- 5691-5692 (P): an arm gene that does not decode. Defensive. Unit-covered.
- 5757-5774 (P): importing a `%half` coil seminoun. honk encodes coil semis
  only as `[%full ~]` batteries, `*seminoun` (blocked) or the `%lazy` root, so
  no type carries a `%half` mask. Unit-covered.
- 5760-5764, 5780-5792, 5797 (P): malformed seminoun masks. Defensive.
  Unit-covered.
- 5884, 5887, B5883T, B5886T; 5956-5973 (0/1 part), B5965T, B5968T (UP): the
  wide-axis walkers first delegate every u64 axis to the small walker, so their
  axis-0 and axis-1 checks are dead.
- 5942, B5941F, 5975-5976, B5975F (UP): when the head fragment of a target
  exists, its tail fragment does too. A complete cell has both, and an atom
  already fails at the head. Dead.
- 6082 (UP): a formula head is always a cell (handled above) or an atom. Dead.
- 6254-6255, B6254F; 6267-6268, B6267F; 6308-6309, B6308F (UP): `axis_big` is
  always `Some` when `axis_small` is `None`. Dead `else` arms.
- 6327 (UP): an op-11 hint is always an atom (handled) or a cell. Dead.
- 6046, B6045T (P): `musk-loop`, which re-enters the same dynamic nock state.
  hoon-138's `araw` recurses without bound, so there is no hoonc verdict.
  Unit-covered (`test.rs` `musk_rejects_a_self_recursive_partial_arm`).
- 6361-6368 (P): `#[cfg(test)]` helper.
- 6451-6453, 6501-6503, 6466, B6464F (P): recovery after the eval stack is
  exhausted during mack or the core copy. Resource exhaustion only.
  Unit-covered with a stack budget.
- 9391-9398, B9390F, B9394F, B9397F (P): malformed seminoun nouns passed to
  `burp_type`. Defensive. Unit-covered.

## bran

- 6559, B6558T; 6587, 6589, B6583F, B6584F; 6598, B6597T (UP or P): bran memo
  internals (bucket overflow, signature collisions). Performance cache only.
  B6583F is dead because the key is the subject's arena id.
- 6663-6664 (UP): a core `rest` that is not a cell. Defensive.
- 6672 (UP), 6682 (P): `repo` failure. A face or hint never fails to repo, and
  a hold built from a compiled arm always plays. 6682 is unit-covered with a
  broken hold.
- 6677, B6676T (UP): this repeats the seen-hold check that
  `bran_canonical_semi_inner` makes just before. Dead.

## `%=`, `%hand`, `!@`

- 6939, B6938T (P): `mate`, raised when an arm edit meets arms whose sample
  axes differ. hoon-138 `++toss` asserts the same condition, so this is a
  rejection path. Unit-covered.
- 7000-7001, B6998F (P): `%=` with edits through a synthetic alias port, in
  play position. Both compilers reject with `hoon` (checked with `probe-diag`).
  Unit-covered.
- 7034-7040 (P): `%hand` has no source syntax. Unreachable.

## Caches (performance only)

- `spec_example_cached` 7467, 7473, 7485, 7488, B7466F, B7471F, B7478F,
  B7481F, B7487T; `spec_factory_open_cached` 7496-7503, 7510, 7526-7533,
  B7495F, B7508F, B7523F, B7525T, B7526T/F, B7532T; `open_cached` 7541-7542,
  B7540F, B7565F; `cache_hoon_ast_ptr` B7409F; `cache_hoon_ast_for_node`
  7447-7449, B7430F, B7446T, B7447T/F; `hoon_ast_lookup_cached` 8454, 8456,
  B8452F; `decode_hold_hoon_ast` 8462, 8469-8475, B8461T, B8466F, B8468T,
  B8469T/F; `hoon_noun_for_node` 8503, B8502T, B8515F; `burp_type` B9381F.
  These are signature `None` (the writers never fail), bucket or hash
  collisions, empty-queue pops, and eviction after 8 to 65,536 entries. They
  can change timing but never output. Where cheap, the unit tests fill the
  caches past their limits.
- 7419, B7418T, 8436, B8435T, 8505, B8501F (P): these paths run only when
  `exact_hoon_ast_lookup_enabled` is off. The honk binary always turns it on,
  so parity runs cannot take them. Unit-covered.

## Cores

- 7602, 7605, B7602T (UP): a core-mint cache hit. The per-gen mint cache has
  the same key components and answers first. Performance cache only.
- 7626, B7625T (UP): a chapter-count mismatch here is already rejected by
  `check_goal_core_chapter_counts`. Dead.
- 7716-7717, B7715T (UP): `mine` is called only with `%gold`. Dead.
- 7813, B7810F (P): a cell goal whose head is not `%noun` (`core-nice`).
  7822, B7821T (P): a fork goal with a core option whose chapter count differs
  (`core-number-of-chapters`). 7896 (P, `unexpcted-chapter`), 7918 (P,
  `unexpected-arm`), 8061 and B8060T (P, `core-number-of-arms`): hoon-138's
  `core-check`, `get-arms`, `get-arm-type` and `arms-check` reject the same
  cases. Rejection paths, all unit-covered.
- 7844, B7843T, 7876, 7883, B7875T (P): recursion guards against a
  self-referential hold goal, and the 128-step walk bound. Defensive.
  Unit-covered.
- 7849 (UP): the fallback for a type tag outside the nine %type tags. Dead.
- 7921-7922 (P), 9216-9228 (P): arm genes that do not decode. Defensive.
  Unit-covered.
- 7935, B7934F, 8190, B8189F (P): an empty arm map. A parsed chapter always
  has at least one arm. Unit-covered.
- 7961-7963, B7951T (UP): a duplicate arm axis, impossible in a well-formed
  map. Defensive.
- 8115-8122 (UP or P): `UnsupportedExpr`/`Backend` error rewrapping.
  Diagnostics only.
- 8158-8171 (UP): `debug_assert_eq!` arguments, compiled out in the
  instrumented builds.
- 8263-8266, B8261F; 8317-8320, B8315F (UP): "unsupported" fallbacks. Every
  Hoon variant is either minted directly or changed by `open`. Defensive.

## Literals

- 8373-8374, 8381, B8373T, B8380T (P): `%sand %n` with a nonzero value and
  `%sand %f` with a value other than 0 or 1. The parser builds `%n` and `%f`
  sands only with 0 (tic-aura casts), so these rejections are not
  source-reachable. Unit-covered.
- 8387 (P): a `%sand` with a cell value. The parser never builds one.
  Unit-covered.
- 8421-8432 (P): `#[cfg(test)]` helper.

## Nesting and wrapping

- 8925, 8934 (P): `nest_core` with a non-core argument. Callers dispatch on
  core/core only. Defensive. Unit-covered.
- 8577-8579 (P): fork options requested for a non-fork. Defensive.
  Unit-covered. 8596, B8590F (UP): an `unreachable!()` after re-matching the
  same enum. Dead.
- 9257, B9256c2T (P): re-wrapping a non-gold core with `^|`/`^&` (`wrap-core`).
  Both compilers reject (hoonc `wrap`, checked with `probe-diag`).
  Unit-covered.
- 9031, B9030c3T: covered by `c2_nest_cores`. One of three full coverage runs
  missed them (flaky profile merge), so they may show as uncovered on a single
  run.
