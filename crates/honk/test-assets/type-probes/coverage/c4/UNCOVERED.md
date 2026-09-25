# c4 ledger: uncovered code in the other `ut` modules and the native helpers

This package covers `native/ut/{find,wet,repo,fire,types,keys}.rs`,
`native/noun.rs`, `native/formula.rs`, `native/hot.rs`, `native/identity.rs`
and `native/mod.rs`; `hot.rs`, `identity.rs` and `keys.rs` have no gaps.

Uncovered by unit tests only (`U`): 0 lines and 0 branch outcomes. By the parity
corpus only (`P`): 698 lines and 180 branch outcomes. By both (`UP`): 13 lines
and 14 branch outcomes. `L<n>` is a line and `B<n>T`/`B<n>F` a branch outcome;
`c<k>` names the k-th condition of a compound test. `P` entries are covered by
unit tests. Where an entry says both compilers reject, hoonc fails on the same
program.

## find.rs

- L19-30, L229-230 [P]: a face name that is not valid text
  (`atom_handle_to_string`, `is_term_face`). Parsed faces are always terms, so
  only malformed input reaches this.
- L57-60 (`find_noun`), L86-94 (`fend_noun`), L593-599, B594T, B594F
  (`resolve_wing_axis`), L603-606 (`resolve_wing_axis_noun`) [P]:
  `#[cfg(test)]` helpers.
- L78 [P]: `fend` on an arm port. `?#` fends `[[%& 1] q.gen]` like hoon-138,
  which turns an arm into a leg on its core (`../regressions/c4_wthx_arm_fend.hoon`),
  so only an `%over` skin naming an arm reaches it, and hoon-138's `++fish`
  crashes there too.
- L80 [P]: `fend` on a synthetic port (fend-fragment). Both compilers reject
  (`=*  w  [y y]` then `?#(^ w)`).
- B190T, L191 [P]: `here` with a skip and no name. That needs a limb
  `[%| n ~]` with n > 0, but the parser only produces `[%| 0 ~]` (from `,`).
- B235T, L236 [UP]: `face_tool_tune_parts` on an atom tool. `is_term_face`
  already handles atom tools.
- L239-240 [UP]: a noun that is neither an atom nor a cell.
- L358-361 [UP]: a face tool that is neither a term nor a tune. Atoms go to
  `is_term_face`, and cells always decode as `Some`.
- L247-252, L254, B246T [P]: the `[%tune aliases bridges]` tagged tool form.
  hoonc and hatch both build untagged tunes, so this is a defensive decode.
- B245F [P]: a tune whose head is a non-text atom. Alias-map heads are cells or
  `~`, so only malformed input reaches this.
- B261T, L262, L373-382 [P]: a tune alias whose value is `~`. No source
  construct makes one: `=*` always stores `[~ hoon]`, and `=,` uses bridges.
- L265-266, L268-269, L271-272, L274-276, B273T [P]: a malformed alias unit
  (`unit_hoon_value`). Defensive.
- L317, L331, L457 [P]: the error return of a nested head, tail or bridge
  search. Only rejected programs or malformed face tools raise an error there.
- L389-392, L440-443 [P]: a tune hoon missing from the AST cache. Tune hoons
  come from the compiler's own lowering, so this is defensive.
- L432-435 [P]: an improper bridge list. Defensive.
- B501F, L502 [P]: a `%fork` with no members. Canonical forks have at least
  two members.
- B616F, L618 [P]: two different holds with the same mug in the goal-core walk
  (`noun_seen_insert_structural`). This needs a 31-bit mug collision.
- B643T, L644 [P]: two synthetic ports with different formulas (find-fork).
  Both compilers reject. Any two source `=*` aliases carry different debug
  spots, so a fork of two branch aliases always gets here.
- B681T, L682 [P]: the same arm name at different battery axes (find-fork).
  Both compilers reject.
- B694F, L697 [P]: the same interned core with a different foot. An interned
  core has one foot per arm, so this is unreachable from source.
- L711 [P]: a leg in one fork branch and an arm in the other (find-fork). Both
  compilers reject.
- B632F, L714 [P]: different unmatched skip counts, or mixed port kinds
  (find-fork). Both compilers reject (`^^a.c` over `?(b [a=1 a=2] [a=1 z=2])`).

## fire.rs

- B36T, B36F, B113c2T, L114, L116 [P]: the hoon-138 shortcut for a single wet
  arm whose body is literally `[%$ 1]`. The parser never produces an axis node
  as an arm body (`.` parses to `[%wing ~[[%& 1]]]`), and wet arms keep their
  raw source hoon.
- B46F, L53 (`fire_arm_dry`), B67F, L74 (`fire_arm_wet`) [UP]: a non-core arm
  type. The core check in `fire_with_mode` guards both.
- B106T, L107 [P]: `fire` with no arms. `fond` always yields at least one arm.
- B112F [P]: a malformed foot. Defensive.
- B123T, L124 [P]: a non-core entry in an arm list. Arms come only from
  `%core` matches in `fond`.
- L156-163 [P]: `core_dox` is a `#[cfg(test)]` helper.
- L172-174 [P]: `garb_with_vair` with a vair other than gold. Every production
  caller passes `Vair::Gold`.

## repo.rs

- L5-14, B9T, B9F (`collect_rest_leg_ids`), L47-57, B52T, B52F
  (`with_rest_legs`), L90-109, B102T, B102F (`rest`), L114-122
  (`with_rest_leg`) [P]: `#[cfg(test)]` helpers.
- B21T, L26 [P]: rest-loop. It fires only when a hold is forced inside its own
  expansion, and hoonc crashes the same way. The hold guards in `nest` and
  `peek` cut source cycles first (`++  a  -.a` compiles in both), so no known
  source program reaches it.
- L32 [UP]: the argument line of `debug_assert!`. Neither coverage build
  records it as executed.
- L75-80 [P]: a hold gene that does not decode as hoon. Defensive.
- L156, L169, L180, L195, B155T, B166T, B168T, B172F, B173F, B194T [P]:
  hold-type memo raw hits, structural hits, key collisions and bucket
  eviction. Performance cache only.
- L231 [P]: repo-fltt. Every caller passes only face, hint, core, hold or noun
  types.

## wet.rs

- B55T [UP]: dead. A rib entry and its raw key are pushed and popped
  together, and a key already present is never pushed again, so an entry whose
  arm noun is raw-equal to the probe is always in the raw key set, which
  answers first.
- B56T, L58 [UP]: an entry whose arm is structurally equal to the probe's but
  under a different noun. Arm nouns come from the battery, and
  `canonicalize_nonsemantic_hoon_noun` strips only `%dbug`, so the canonical
  noun is a stable sub-noun unless it rebuilds a `%ktcl` over a `%gist`. hatch
  attaches that `%gist` from a postfix doc on a `^:` arm body, and the same doc
  wraps the body in `%note %help`, so the rebuild never sees `%ktcl` on top.
  Unreachable from source.
- L92-95 [P]: a wet arm hoon that does not decode. Defensive.
- B100F [UP]: `fire_wet_rib.pop()` returning `None`. The entry is pushed just
  above.
- B174T, L175 [P]: face stacks of different lengths. `wec` then has two
  members, and the redo ends in redo-match. Both compilers reject: hoonc
  prints %dear-many, then %redo-match, for `|*  a=?(b=@ @)  a` applied to `5`.
- B202F, L203 [UP]: `redo_subject_hold_in_fan` on a type that is not a hold.
  Its only caller is the `%hold` arm of `redo_dext`.
- B307T [UP]: a `gil` hit on the original reference without a hit on the
  reduced one. `redo_sint` with `hod` off is deterministic and idempotent on
  its output, so a repeated reference always reduces to a recorded reduced
  reference, which the first check finds.

## types.rs

- B49F, B89F, B123F [UP]: `pop_front` on a queue that has just grown past its
  limit. The queue is never empty there.
- L49-51, L53, B46F, B48T, B49T (`RawMemoMap::insert_with_limit`), L127, B120F
  (`BucketMemo::ensure_key`) [P]: existing-key updates and eviction.
  Performance cache only.
- L57-60, L131-134 [P]: `RawMemoMap::clear` and `BucketMemo::clear` have no
  production callers (the memos are reset by assigning `Default`).
- L69-74, L81-94, B86T, B86F, B88T, B88F, B89T [P]: `RawMemoSet` has no
  production users (dead code).
- L184-236 and its branches B191T through B232F (`StructNounSet`), L262-427
  and its branches B280T through B423F (`StructNounPairSet`), L438-469, B444T,
  B444F, B450T, B450F, B452F, B452c2T, B452c2F (`NestTypeInterner`) [P]: used
  only by tests.
- B452T [UP]: a raw-equal hit inside a `NestTypeInterner` mug bucket. Every
  bucketed noun's raw word is also in `raw_ids`, which answers first.
- L498-500, L518, L524-564, L590-592, L612, L618-666 [P]: `contains_id`, the
  miss arms of `remove_id`, and the `NestNounId` impls of `NestSeenSet` and
  `NestPairSet`. Production `nest` uses the `TypeId` sets through `insert_id`
  and `remove_id` hits only.
- L727-729, L747-769 [P]: the `FastHasher` `write_u16` and signed-width methods
  (`write_i8` through `write_isize`). No key hashed on the compile path uses
  them.

## noun.rs

- B32c2T [P]: `term_to_noun` on an empty string. A defensive guard (an empty
  term would build a zero-byte atom); no probe compile passes one.
- L54-57, L70-72, B52F, B69T [P]: malformed unit nouns in `opt_from_noun`.
  Defensive.
- B97T, B103T, L104 [UP]: dead code in `parsed_atom_to_noun`. A value above
  `DIRECT_MAX` always has a non-zero byte, and `BigUint::to_bytes_le()` of
  zero is `[0]`, never empty.
- L119-142, B124T, B124F, B128T, B128F [P]: `list_to_vec` has no callers
  outside tests.
- L152-163 [P]: an atom that is not valid UTF-8 in `atom_to_string`.
  Defensive.
- L186-200 [P]: `cell_head` and `cell_tail` have no callers outside tests.
- L275-276, B232c2F, B246F, B246c2F [P]: an atom and a cell with equal mugs in
  `noun_eq`. This needs a 31-bit mug collision.

## formula.rs

- L8-16, B12T, B12F (`cons`), L97-109, B99T, B99F, B102T, B102F, B105T, B105F
  (`cond`), L111-119, B112T, B112F, B115T, B115F (`const_formula_value`),
  L121-127, B122T, B122F, B125T, B125F (`is_const_bool`), L129-134, B130T,
  B130F, B133T, B133F (`is_axis_zero_formula`) [P]: `cons` and `cond` have no
  callers outside tests. Program formulas are built by the equivalent rules in
  `ir::formula_dag`.
- L24, L26-31, L33-38, L40-50, L52-53, L58, L60, L63, B23T, B24T, B24F, B26T,
  B26F, B27T, B27F, B33T, B33F, B34T, B34F, B35T, B35F, B36T, B36F, B40T, B40F,
  B40c2T, B40c2F, B56F, B57T, B62T (`comb`), L70-77 (`nock_peg`), L79-82
  (`slot_formula_axis`), L86, L90-92, L94, B85F, B88F, B91T, B91F
  (`axis_formula_value`), B140T (`is_axis_one_formula`) [P]: outside tests,
  `comb` runs only in the chunked prelude assembly (`mint_honc_prelude_chunked`
  in `bin/honk.rs`). Each prelude layer formula is a cell that is neither
  `[0 a]` nor `[x [0 1]]`, and no right side is `[0 1]`, so every fold falls
  through to `[7 mal buz]`.

## mod.rs

- L25-67, B53T, B53F [P]: `NativeCompiler` backs the library `honk::Compiler`.
  The `honk` binary that builds the probes drives `Ut` directly, so none of
  this runs under the parity corpus. B53T and L54 are the diagnostics-only
  `HONK_IR_ROUNDTRIP` self-check, covered by `tests/cov_c4_ir_roundtrip.rs`.
