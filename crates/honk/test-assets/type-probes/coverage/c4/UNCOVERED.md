# c4 ledger: branches left uncovered

Scope: `native/ut/{find,wet,repo,fire,types,keys}.rs`, `native/noun.rs`,
`native/formula.rs`, `native/hot.rs`, `native/identity.rs`, `native/mod.rs`.
`U` = still missed by unit tests, `P` = still missed by the parity probes.
`L<n>` is a line and `B<n>T/F` is a branch outcome, with line numbers as of the
coverage run. `hot.rs`, `identity.rs` and `keys.rs` have no gaps.

Unit tests: `crates/honk/src/native/ut/cov/c4_ut_sub.rs` and
`crates/honk/tests/cov_c4_ir_roundtrip.rs`. Probes: `c4_find_walk`,
`c4_find_tune`, `c4_goal_hold`. Fixed divergence: `../regressions/c4_wthx_arm_fend.hoon`.

Where this ledger says "both reject", the case was checked with
`probe-diag.sh`: hoonc fails with the same crash as honk.

## find.rs

- L19-30, L226-227 [P]: a face name that is not valid text. Parsed faces are
  always terms, so only malformed input reaches this. Unit-tested.
- L78 [P]: `fend` on an arm port. `?#` now fends `[[%& 1] q.gen]` like
  hoon-138, which turns the arm into a leg on its core
  (`../regressions/c4_wthx_arm_fend.hoon`), so only an `%over` skin naming an
  arm reaches it, and hoon-138's `++fish` crashes there too.
- L80 [P]: `fend` on a synthetic port (fend-fragment). Both reject
  (`=*  w  [y y]` then `?#(^ w)`). Unit-tested.
- L124, L125 [P]: `fond` whose tail search is void or unmatched. `find` then
  fails, so this is rejection-only. Unit-tested.
- L188, B187T [P]: `here` with a skip and no name. That needs a limb
  `[%| n ~]` with n > 0, but the parser only produces `[%| 0 ~]` (from `,`).
  Not reachable from source. Unit-tested.
- L233, B232T [UP]: `face_tool_tune_parts` on an atom tool. `is_term_face`
  already handles atom tools. Unreachable.
- L236-237 [UP]: a noun that is neither an atom nor a cell. Unreachable.
- L355-358 [UP]: a face tool that is neither a term nor a tune. Atoms go to
  `is_term_face`, and cells always decode as `Some`. Unreachable.
- L244-249, L251, B243T [P]: the `[%tune aliases bridges]` tagged tool form.
  hoonc and hatch both build untagged tunes, so this is a defensive decode.
  Unit-tested.
- B242F [P]: a tune whose head is a non-text atom. Alias-map heads are cells or
  `~`, so only malformed input reaches this. Unit-tested.
- L259, L370-379, B258T [P]: a tune alias whose value is `~`. No source
  construct makes one: `=*` always stores `[~ hoon]`, and `=,` uses bridges.
  Unit-tested.
- L262-273, B270T [P]: a malformed alias unit. Defensive. Unit-tested.
- L314, L328, L454 [P]: an error raised inside a head, tail or bridge search.
  This happens only with malformed face tools. Unit-tested.
- L386-389, L437-440 [P]: a tune hoon that does not decode. Tune hoons come
  from the compiler's own lowering, so this is defensive. Unit-tested.
- L429-432 [P]: an improper bridge list. Defensive. Unit-tested.
- L499, B498F [P]: a `%fork` with no members. Canonical forks have at least two
  members. Unit-tested.
- L591, B590T [P]: `resolve_wing_axis` with an empty wing. Its only caller is
  the `%over` skin, whose wing comes from `reek` of a non-empty rope. Not
  reachable from source. Unit-tested.
- L613, B611F [P]: two different holds with the same mug in the goal-core walk.
  This needs a 31-bit mug collision. Unit-tested with colliding atoms.
- L639, B638T [P]: two synthetic ports with different formulas (find-fork).
  Both reject. Any two source `=*` aliases carry different debug spots, so a
  fork of two branch aliases always gets here. Unit-tested.
- L677, B676T [P]: the same arm name at different battery axes (find-fork).
  Both reject. Unit-tested.
- L692, B689F [P]: the same interned core with a different foot. An interned
  core has one foot per arm, so this is unreachable from source. Unit-tested
  with a direct `twin` call.
- L706 [P]: a leg in one fork branch and an arm in the other (find-fork). Both
  reject. Unit-tested.
- L709, B627F [P]: different unmatched skip counts, or mixed port kinds
  (find-fork). Both reject (`^^a.c` over `?(b [a=1 a=2] [a=1 z=2])`).
  Unit-tested.

## fire.rs

- L49, L70, B42F, B63F [UP]: a non-core type in `fire_arm_dry` or
  `fire_arm_wet`. The core check at L118 guards both. Unreachable.
- L102, B101T [P]: `fire` with no arms. `fond` always yields at least one arm.
  Unreachable from source. Unit-tested.
- L109, L111, B32T, B32F, B108c2T [P]: the hoon-138 shortcut for a single wet
  arm whose body is literally `[%$ 1]`. The parser never produces an axis node
  as an arm body (`.` parses to `[%wing ~[[%& 1]]]`), and wet arms keep their
  raw source hoon. Unreachable from source. Unit-tested with hand-built feet.
- B107F [P]: a malformed foot. Defensive. Unit-tested.
- L119, B118T [P]: a non-core entry in an arm list. Arms come only from
  `%core` matches in `fond`. Unreachable from source. Unit-covered by existing
  tests.
- L167-169 [P]: `garb_with_vair` with a vair other than gold. Every production
  caller passes `Vair::Gold`. Unit-tested.

## repo.rs

- L26, B21T [P]: rest-loop. It fires only when a hold is forced inside its own
  expansion, and hoonc crashes the same way. No source was found that gets
  past the hold guards in `nest` and `peek` first; for example,
  `++  a  -.a` compiles in both. Unit-covered by existing tests.
- L32 [UP]: the argument line of `debug_assert!`. Neither coverage build
  records it as executed.
- L75-80 [P]: a hold gene that does not decode as hoon. Defensive.
  Unit-tested.
- L156, L169, L180, L195, B155T, B166T, B168T, B172F, B173F, B194T [P]:
  hold-type memo raw hits, structural hits, key collisions and bucket
  eviction. Performance cache only. Unit-tested.
- L231 [P]: repo-fltt. Every caller passes only face, hint, core, hold or noun
  types. Unit-covered by existing tests.

## wet.rs

- L54-58, L62-63, L401-405, L407, B56T, B56F, B399F [U; P for L54, L62, B56F]:
  `redo_fork_fallback_candidate` and the empty-`wec` fallback. Blocked by the
  known `redo_dear` empty-`wec` / `redo_fork_fallback_candidate` divergence.
- L92-94, L96, B91T, B92T, B92F, B93T, B93F, B94T, B94F [UP]: the structural
  scan of `fire_wet_rib`. Blocked by the known wet-arm rib-key divergence in
  `mull_check_wet`. That key uses the wet core, where hoon-138 uses `sut`.
- L124-127 [P]: a wet arm hoon that does not decode. Defensive. Unit-tested.
- B132F [UP]: `fire_wet_rib.pop()` returning `None`. The entry is pushed just
  above. Unreachable.
- L201, B200T [P]: face stacks of different lengths. `wec` then has two
  members, and the redo ends in redo-match. Both reject: hoonc prints
  %dear-many, then %redo-match, for `|*  a=?(b=@ @)  a` applied to `5`.
  Unit-tested.
- L229, B228F [UP]: `redo_subject_hold_in_fan` on a type that is not a hold.
  Its only caller is the `%hold` arm of `redo_dext`. Unreachable.
- B333T [UP]: a gil hit on the original reference without a hit on the
  reduced one. `redo_sint` is idempotent: a reduced reference has no top-level
  face, hint or fork. So an original-reference hit always comes with a
  reduced-reference hit first. Unreachable.

## types.rs

- B49F, B89F, B123F [UP]: `pop_front` on a queue that has just grown past its
  limit. The queue is never empty there. Unreachable.
- B452T [UP]: a raw-equal hit inside a `NestTypeInterner` mug bucket. Every
  bucketed noun's raw word is also in `raw_ids`, which returns first.
  Unreachable.
- L49-53, L127, B46F, B48T, B49T, B120F, B123F [P]: existing-key updates and
  eviction in `RawMemoMap` and `BucketMemo`. The limits are 65,536 keys.
  Performance cache only. Unit-tested.
- L57-60, L131-134 [P]: `RawMemoMap::clear` and `BucketMemo::clear` have no
  production callers (dead code). Unit-tested.
- L69-74, L81-94 [P]: `RawMemoSet` has no production users (dead code).
  Unit-tested.
- L184-236, L262-427, L438-469 and their branches (B191-B232, B280-B423, B444)
  [P]: `StructNounSet`, `StructNounPairSet` and `NestTypeInterner` are used
  only by tests. Unit-tested.
- L498-500, L518, L524-564, L590-592, L612, L618-666 [P]:
  `contains_id` / `remove_id` misses and the `NestNounId`
  `NestSeenSet` / `NestPairSet` impls. Production `nest` uses the `TypeId` sets
  through `insert_id` / `remove_id` hits only. Unit-tested.
- L727-729, L747-754 [P]: the `FastHasher` `write_u16`, `write_i8` and
  `write_i16` widths. No hashed key type uses them. Unit-tested.

## noun.rs

- L102, B101T [UP]: `BigUint::to_bytes_le()` of zero is `[0]`, never empty.
  Unreachable.
- B95T [UP]: a value above `DIRECT_MAX` always has a non-zero byte.
  Unreachable.
- L52-55, L68-70, B50F, B67T [P]: malformed unit nouns. Defensive.
  Unit-tested.
- L117-140, B122T, B122F, B126T, B126F [P]: `list_to_vec` is not called on the
  probe compile path; it is a library helper. Unit-tested.
- L150-161 [P]: an atom that is not valid UTF-8 in `atom_to_string`.
  Defensive. Unit-tested.
- L184-198 [P]: `cell_head` and `cell_tail` have no callers on the probe
  compile path. Unit-tested.
- L240, L273-274, B230c2F, B239T, B244F, B244c2F [P]: distinct nouns with equal
  mugs. This needs a 31-bit mug collision. Unit-tested with colliding
  atoms and cells found by search.

## formula.rs

- All of `cons`, `comb`, `cond` and their helpers (L8-134, 39 branch outcomes)
  [P]. These noun-level `++cons` / `++comb` / `++cond` rules are used only by
  the chunked prelude assembly in `bin/honk.rs` (`mint_honc_prelude_chunked`)
  and by tests. The prelude fixes their input, so no probe can change it. The
  program formulas that probes do control are built by the equivalent rules in
  `ir::formula_dag`. Every rule and fallthrough is unit-tested.

## native/mod.rs

- L25-67, B53T, B53F [P]: `NativeCompiler` backs the library `honk::Compiler`.
  The `honk` binary that builds the probes drives `Ut` directly, so none of
  this runs under probes. Unit-covered. L54 / B53T is the diagnostics-only
  `HONK_IR_ROUNDTRIP` self-check, covered by `tests/cov_c4_ir_roundtrip.rs`.
