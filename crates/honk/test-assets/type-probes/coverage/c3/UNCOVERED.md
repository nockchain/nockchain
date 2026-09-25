# c3 ledger: uncovered code in `native/ut/mod.rs` from line 9583

This package covers `crates/honk/src/native/ut/mod.rs` from line 9583 (the skin
`gain`/`lose` helpers) to the end of the file.

Uncovered by unit tests only (`U`): 0 lines and 0 branch outcomes. By the parity
corpus only (`P`): 887 lines and 82 branch outcomes. By both (`UP`): 32 lines
and 20 branch outcomes. `L<n>` is a line and `B<n>T`/`B<n>F` a branch outcome;
`c<k>` names the k-th condition of a compound test. `P` entries are covered by
unit tests.

## Test-only code

These items are `#[cfg(test)]`, so the parity build never runs them.

- `fuse_noun` L10239-10244, `miss_noun` L10264-10268, `crop_noun` L10525-10530,
  `fork_from_options_n` L10783-10787, `peek_noun` L10980-10984, `mull_noun`
  L11052-11061 [P]: noun-bridged wrappers around the native operations.
- `with_stack_guard` L10992-10994 [P]: the test-build call counter.
- `type_face_name_if_atom` L12259-12268, `type_hold_type` L12320-12330,
  `find_face_axis_skip` L12490-12498, and `find_face_axis_skip_inner`
  L12501-12580 with all its branches (B12511 through B12568) [P].
- `mint_tisgar_chain_chunked` L12715-12759, B12723T, B12723F, B12741T,
  B12741F, B12755T, B12755F [P]: the chunked-mint prototype. The production
  chunked prelude mint is `mint_honc_prelude_chunked` in `bin/honk.rs`.
- `ty_face` L12783-12786 and the paired constructors `ty_noun_n`
  L12936-12938, `ty_void_n` L12941-12943, `ty_cell_n` L12957-12966,
  `ty_face_tool_n` L12969-12991, `ty_face_n` L12994-13002, `ty_hint_n`
  L13005-13037, `ty_hold_n` L13040-13056, `ty_core_n` L13059-13087, `ty_fork_n`
  L13090-13094 [P].
- `is_const_bool_formula` L14080-14086, B14081T, B14081F, B14084T, B14084F,
  B14085T, B14085F; `cell_type` L14089-14090, L14098-14104, B14090F, B14100T,
  B14100F, B14100c2T, B14100c2F; `cell_type_n` L14108-14122, B14115T, B14115F,
  B14118T, B14118F [P].
- `cell_type` L14091-14097, B14090T [UP]: the `HONK_NATIVE_TYPES` branch. The
  flag is read once per process, so a test that sets it would race the others.

## Unreachable from source

Skins:

- L11578 [UP]: the `?` on `collect_lazy_resolver_arms_from_tomes_map` in
  `mull_mile`, which fails only on a malformed tomes map; `mull_mile` passes
  the map `mull` just built.
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
- B11468T, L11469 (`mull_nice`), B11691F, L11693 (`mull_cnts_with_ports`) [P]:
  `vet` off inside `mull`. hoon-138 runs `mull` only under `vet` (`fire`
  checks `!vet` first).
- B11460F, L11462-11463 [P]: mull-open. Every parsed gene has a `mull` arm or
  opens.
- B11676T, L11677 [P]: edits on a synthetic port. `tack` rejects them in mint
  first.
- L11684 [P]: mixed synthetic and natural ports. sut and dox share the
  definition context, so a wing resolves synthetically on both sides or on
  neither.
- B11721T, L11722-11724, B11742T, L11743-11745, B11755T, L11756-11758,
  L11769-11771 [P]: `mull_endo` axis and opal mismatches. The sut and dox cores
  share arm layout and sample faces.

Atoms, axes and nouns:

- L11779 (`atom_is_zero`), L11786-11788, L11790-11791, B11788F, B11791T,
  B11791F (`atom_is_flag`) [P]: the `ParsedAtom::Big` arms. `play_sand` calls
  them only for `%n` and `%f` sands, whose source atoms (`~` and the loobean
  literals) the parser stores as `ParsedAtom::Small`.
- B11788T, L11789 [UP]: dead. `BigUint::to_bytes_le()` of zero is `[0]`, never
  empty.
- B12583T, L12584 (`axis_cap_mas`), B12615T, L12616 (`axis_big_cap_mas`),
  B12655T, L12656-12658 (`peg_axis_big_pair`) [P]: defensive `axis <= 1` and
  `b == 0` checks.
- B12589T, L12590-12592 (`axis_cap_mas`), B12619T, L12620-12622
  (`axis_big_cap_mas`) [UP]: dead. `axis > 1` implies a bit length of at least
  2.
- B12091T, B12092T, B12092F, L12092-12096 [P]: `map_put_mug` on an existing
  key. Every caller builds the map from a `HashMap`, so keys are unique.
- L12108-12109, L12133-12134 [UP]: `map_put_mug` missing-branches errors. The
  recursive call returns a node it just built.
- L12362-12363 [P]: a fork-set node without branches on the push side of
  `fork_set_options`. Defensive against malformed type nouns.
- L12377-12378 [UP]: the same error on the pop side, which re-reads a node the
  push side already checked.
- B12352T, L12353-12355 [P]: the million-node budget of `fork_set_options`.
  Defensive. A unit test reaches it with a shared-subtree DAG, but a fork
  built from source is a treap with one node per distinct member type, so it
  would need a million different types.
- B12206T, L12207 [P]: `type_tag_kind` on a void core payload. `ty_core` is its
  only production caller, and the native constructors collapse a void payload
  first.
- B12222F, L12225-12226 [P]: an unknown type tag. No type noun has one.
- L12791 (`ty_face_tool`), L12820 (`ty_core`) [P]: a void inner type or
  payload. Callers pass types the native constructors already collapsed.
- L12470-12472 [P]: an unknown foot tag in `foot_parts`. Defensive.
- L12896-12902 [P]: `NativeForkOptionIter::size_hint`. Allocation sizing only;
  no compile path calls it.
- L13456 [P]: a tune alias without a hoon in `tune_to_noun`. Parsed tunes
  always carry one (`=*`, busk).

`%hand` lowering:

- L11163-11166 (`mull_inner`), `type_to_noun` L13482-13528,
  `face_type_to_noun` L13530-13538, `coil_to_noun` L13597-13624,
  `garb_to_noun` L13626-13637, `poly_to_noun` L13639-13644, `vair_to_noun`
  L13646-13653, `semi_noun_expr_to_noun` L13655-13659, `stencil_to_noun`
  L13661-13682, `block_to_noun` L13684-13690, `gate_to_noun` L13709-13714,
  `spec_to_noun` L13716-13910, `basetype_to_noun` L13912-13924,
  `skin_to_noun` L13926-13974, `atom_to_noun` L13992-13994, `nock_to_noun`
  L13996-14066, `nock_hint_to_noun` L14068-14077 [P]: `Hoon::Hand` comes only
  from `noun_to_hoon`, never from parsing.

Note wings:

- L13555-13560, L13564 (`note_to_noun`), `wing_to_noun` L13571-13577,
  `limb_to_noun` L13579-13595, `slot_formula_axis_noun` L11831-11833,
  `slot_formula_axis_big` L11835-11838 [P]: a `%made` note with a wing list.
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
- B11866T, L11867, B11872F, L11873, L11889-11890, B11904F, B11905T, B11905F,
  L11905-11906, L11910 [P]: `dor`, the tie-break of `gor_mug` and `mor_mug`. It
  runs only when two treap keys have equal mugs, so which arms run depends on
  hash collisions in the compiled programs.
