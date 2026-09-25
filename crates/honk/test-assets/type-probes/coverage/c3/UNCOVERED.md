# c3 ledger: uncovered code in `native/ut/mod.rs` from line 9583

This package covers `crates/honk/src/native/ut/mod.rs` from line 9583 (the skin
`gain`/`lose` helpers) to the end of the file.

Uncovered by unit tests only (`U`): 0 lines and 1 branch outcome. By the parity
corpus only (`P`): 886 lines and 81 branch outcomes. By both (`UP`): 35 lines
and 23 branch outcomes. `L<n>` is a line and `B<n>T`/`B<n>F` a branch outcome;
`c<k>` names the k-th condition of a compound test. `P` entries are covered by
unit tests.

## Test-only code

These items are `#[cfg(test)]`, so the parity build never runs them.

- `fuse_noun` L10239-10244, `miss_noun` L10264-10268, `crop_noun` L10525-10530,
  `fork_from_options_n` L10783-10787, `peek_noun` L10980-10984, `mull_noun`
  L11052-11061 [P]: noun-bridged wrappers around the native operations.
- `with_stack_guard` L10992-10994 [P]: the test-build call counter.
- `type_face_name_if_atom` L12245-12254, `type_hold_type` L12306-12316,
  `find_face_axis_skip` L12476-12484, and `find_face_axis_skip_inner`
  L12487-12566 with all its branches (B12497 through B12554) [P].
- `mint_tisgar_chain_chunked` L12701-12745, B12709T, B12709F, B12727T,
  B12727F, B12741T, B12741F [P]: the chunked-mint prototype. The production
  chunked prelude mint is `mint_honc_prelude_chunked` in `bin/honk.rs`.
- `ty_face` L12769-12772 and the paired constructors `ty_noun_n`
  L12922-12924, `ty_void_n` L12927-12929, `ty_cell_n` L12943-12952,
  `ty_face_tool_n` L12955-12977, `ty_face_n` L12980-12988, `ty_hint_n`
  L12991-13023, `ty_hold_n` L13026-13042, `ty_core_n` L13045-13073, `ty_fork_n`
  L13076-13080 [P].
- `is_const_bool_formula` L14066-14072, B14067T, B14067F, B14070T, B14070F,
  B14071T, B14071F; `cell_type` L14075-14076, L14084-14090, B14076F, B14086T,
  B14086F, B14086c2T, B14086c2F; `cell_type_n` L14094-14108, B14101T, B14101F,
  B14104T, B14104F [P].
- `cell_type` L14077-14083, B14076T [UP]: the `HONK_NATIVE_TYPES` branch. The
  flag is read once per process, so a test that sets it would race the others.

## Unreachable from source

Skins:

- L9642-9643 (gain), L9989-9990 (lose) [P]: a `[%base %null]` skin. `flay`
  turns `~` into a `%rock %n` leaf, so no source skin has this form.
- L9645, L9992 [P]: a `[%base %void]` skin. `!!` is `%zpzp`, which `flay`
  rejects.
- L9667, L10007 [P]: `Skin::Dbug`. `flay` never builds `%dbug` skins (in
  hoon-138 or hatch).
- L9668-9671, L9674-9678, L10008 [P]: `Skin::Help`. It needs a `%note %help`
  hoon in `?#` skin position; hatch attaches docs only to `^=` names, which
  `flay` then rejects.
- B9648T, L9649 (`%noun` base gain), L9719 (`gain_atom_skin`), L9794
  (`gain_cell_skin`), L9882 (`gain_leaf_skin`), L10035 (`lose_atom_skin`),
  L10087 (`lose_cell_skin`), L10176 (`lose_leaf_skin`), L10326
  (`miss_dext_uncached`) [P]: a void ref. The face, hint, cell and fork
  constructors collapse void, and a void leg makes the subject void, which
  fails with mint-vain first.
- L9798 [P]: `gain_cell_skin` on a `%noun` ref whose head skin gains `%void`.
  From `%noun` only `[%base %void]` does that, and `flay` never builds it.
- B9897T, B9899T, B9900F, B9905T, L9906 [P]: a leaf skin with an empty or `@`
  aura. Leaf skins come only from `%rock`s, which always carry an aura.
- B9695T, L9696 (`gain spec`), B10018T, L10019 (`lose spec`) [P]: `ar:fish`
  already requires the ref to nest in the spec, so after a successful mint
  this check cannot fail.

Type operations:

- B10614T, L10615-10617 [P]: crop-loop. A `?=` computes the gain before the
  lose, and the gain fails with fuse-loop first.
- L10652 [P]: the `crop_sint` fallback. `crop_inner` handles `%void` and
  `%noun` refs before calling it, and its atom, cell and core callers handle
  atom and cell refs themselves.
- B10701T, L10702 (`end_bytes`), B10718T, L10719 (`rsh_bytes`) [UP]: dead.
  `$` is 0, so the `fitz` loop returns before `end_bytes` sees an empty atom,
  and `rsh_bytes` returns `$` before `rest` can be empty.
- B10726c3F [UP]: the `p_w != 0` test is reached only when `p_w != 0`.
- B10926c5T, B10926c6T, B10926c6F, B10937T, L10938 [UP]: the blocked-context
  arm of `peek`. `peel` returns `con = true` only together with `sam = true`,
  so the first clause always matches first.

`mull`:

- B11328T, B11343T, L11344 [UP]: an axis mismatch between the sut and dox
  wings. Wet samples keep the declared faces, so a wing resolves to the same
  axis on both sides.
- B11367T, B11367F, L11367-11371 [P]: `%lost` in `mull`. mint rejects a
  `%lost` on a non-void subject first (mint-lost), a void subject fails with
  mull-none before `mull_inner`, and the `vet`-off branch is unreachable (next
  entry).
- B11468T, L11469 (`mull_nice`), B11677F, L11679 (`mull_cnts_with_ports`) [P]:
  `vet` off inside `mull`. hoon-138 runs `mull` only under `vet` (`fire`
  checks `!vet` first).
- B11460F, L11462-11463 [P]: mull-open. Every parsed gene has a `mull` arm or
  opens.
- B11662T, L11663 [P]: edits on a synthetic port. `tack` rejects them in mint
  first.
- L11670 [P]: mixed synthetic and natural ports. sut and dox share the
  definition context, so a wing resolves synthetically on both sides or on
  neither.
- B11707T, L11708-11710, B11728T, L11729-11731, B11741T, L11742-11744,
  L11755-11757 [P]: `mull_endo` axis and opal mismatches. The sut and dox cores
  share arm layout and sample faces.

Atoms, axes and nouns:

- L11765 (`atom_is_zero`), L11772-11774, L11776-11777, B11774F, B11777T,
  B11777F (`atom_is_flag`) [P]: the `ParsedAtom::Big` arms. `play_sand` calls
  them only for `%n` and `%f` sands, whose source atoms (`~` and the loobean
  literals) the parser stores as `ParsedAtom::Small`.
- B11774T, L11775 [UP]: dead. `BigUint::to_bytes_le()` of zero is `[0]`, never
  empty.
- B12569T, L12570 (`axis_cap_mas`), B12601T, L12602 (`axis_big_cap_mas`),
  B12641T, L12642-12644 (`peg_axis_big_pair`) [P]: defensive `axis <= 1` and
  `b == 0` checks.
- B12575T, L12576-12578 (`axis_cap_mas`), B12605T, L12606-12608
  (`axis_big_cap_mas`) [UP]: dead. `axis > 1` implies a bit length of at least
  2.
- B12077T, B12078T, B12078F, L12078-12082 [P]: `map_put_mug` on an existing
  key. Every caller builds the map from a `HashMap`, so keys are unique.
- L12094-12095, L12119-12120 [UP]: `map_put_mug` missing-branches errors. The
  recursive call returns a node it just built.
- L12348-12349 [P]: a fork-set node without branches on the push side of
  `fork_set_options`. Defensive against malformed type nouns.
- L12363-12364 [UP]: the same error on the pop side, which re-reads a node the
  push side already checked.
- B12338T, L12339-12341 [P]: the million-node budget of `fork_set_options`.
  Defensive. A unit test reaches it with a shared-subtree DAG, but a fork
  built from source is a treap with one node per distinct member type, so it
  would need a million different types.
- B12192T, L12193 [P]: `type_tag_kind` on a void core payload. `ty_core` is its
  only production caller, and the native constructors collapse a void payload
  first.
- B12208F, L12211-12212 [P]: an unknown type tag. No type noun has one.
- L12777 (`ty_face_tool`), L12806 (`ty_core`) [P]: a void inner type or
  payload. Callers pass types the native constructors already collapsed.
- L12456-12458 [P]: an unknown foot tag in `foot_parts`. Defensive.
- L12882-12888 [P]: `NativeForkOptionIter::size_hint`. Allocation sizing only;
  no compile path calls it.
- L13442 [P]: a tune alias without a hoon in `tune_to_noun`. Parsed tunes
  always carry one (`=*`, busk).

`%hand` lowering:

- L11163-11166 (`mull_inner`), `type_to_noun` L13468-13514,
  `face_type_to_noun` L13516-13524, `coil_to_noun` L13583-13610,
  `garb_to_noun` L13612-13623, `poly_to_noun` L13625-13630, `vair_to_noun`
  L13632-13639, `semi_noun_expr_to_noun` L13641-13645, `stencil_to_noun`
  L13647-13668, `block_to_noun` L13670-13676, `gate_to_noun` L13695-13700,
  `spec_to_noun` L13702-13896, `basetype_to_noun` L13898-13910,
  `skin_to_noun` L13912-13960, `atom_to_noun` L13978-13980, `nock_to_noun`
  L13982-14052, `nock_hint_to_noun` L14054-14063 [P]: `Hoon::Hand` comes only
  from `noun_to_hoon`, never from parsing.

Note wings:

- L13541-13546, L13550 (`note_to_noun`), `wing_to_noun` L13557-13563,
  `limb_to_noun` L13565-13581, `slot_formula_axis_noun` L11817-11819,
  `slot_formula_axis_big` L11821-11824 [P]: a `%made` note with a wing list.
  It comes only from `example` or `relative` of a `%made` spec, which neither
  hatch's parser nor hoon-138 builds (hatch makes `Spec::Made` only in
  `noun_to_spec`).

## Rejected by both compilers

Each case is unit-tested, and hoonc fails on the same program.

- B9730c2T, L9731-9733 (`gain_atom_skin`), B9900c2T, L9901-9903
  (`gain_leaf_skin`) [P]: atom-mismatch (`?#(@ud t)`, `?#(%5 t)` on a `@t`).
- B10496T, L10497-10499 [P]: fuse-loop (`?=(@ v)` with `v=$@(@ r)`).
- L11033 [P]: mull-none (`=>(!! a)` in a wet arm).
- L11277 [P]: mull-bonk-b.
- L11295 [P]: mull-bonk-c.
- B11328c2T, L11329 [P]: mull-bonk-a (`?=(_a a)`).
- B11347T, L11348 [P]: mull-bonk-x (a non-nesting `?#` wing).
- B11406T, L11407 [P]: mull-bonk-f (`!@(p.a ...)`).
- L11423 [P]: `%eror` in `mull`, from a duplicate arm or chapter. hoon-138
  `++open` crashes on `%eror`.

One rejection has no hoonc verdict:

- L9703-9707 [P]: gain-wash, a wash skin as a `?:` condition. hoon-138
  `ar:gain` recurses forever on it, so hoonc never finishes and no
  hoonc-checked compile reaches this line. The native-only probe
  `../../reject/wash_gain_loop.hoon` pins honk's error.

## Caches and hash collisions

- B10273c2F, B10277F [P]: `set_miss_memo_persistence` enabling persistence
  that is already on. The `honk` binary enables it only on a `Ut` where it is
  off.
- B10362c4T [P]: the reversed-pair hold guard in `miss_dext_uncached`. It needs
  mutually recursive molds through `redo`.
- L10859, B10857F, B10857c2F (`seen_hold`), L10883-10884, B10880F, B10880c2F,
  B10886F (`unsee_hold`) [UP]: a `peek` seen-hold bucket holding a different
  (hold, axis) pair. That needs a collision of the combined hash.
- L10889, B10876F, B10879F [UP]: dead. `unsee_hold` removes only a pair that
  `seen_hold` inserted, so its bucket and entry are always present.
- L11024-11025 [UP]: the `mull` cache signature fallback. Every AST gets a
  signature.
- L11045 [UP]: the error return of `mull_cache_store`. The lookup has already
  computed the same fan key.
- B11852T, L11853, B11858F, L11859, L11875-11876, B11890F, B11891T, B11891F,
  L11891-11892, L11896 [P]: `dor`, the tie-break of `gor_mug` and `mor_mug`. It
  runs only when two treap keys have equal mugs, so which arms run depends on
  hash collisions in the compiled programs.
