# c3 ledger: uncovered code in `native/ut/mod.rs` from line 9580

This package covers `crates/honk/src/native/ut/mod.rs` from line 9580 (the skin
`gain`/`lose` helpers) to the end of the file.

Uncovered by unit tests only (`U`): 0 lines and 0 branch outcomes. By the parity
corpus only (`P`): 891 lines and 82 branch outcomes. By both (`UP`): 32 lines
and 20 branch outcomes. `L<n>` is a line and `B<n>T`/`B<n>F` a branch outcome;
`c<k>` names the k-th condition of a compound test. `P` entries are covered by
unit tests.

## Test-only code

These items are `#[cfg(test)]`, so the parity build never runs them.

- `fuse_noun` L10236-10241, `miss_noun` L10261-10265, `crop_noun` L10522-10527,
  `fork_from_options_n` L10780-10784, `peek_noun` L10977-10981, `mull_noun`
  L11049-11058 [P]: noun-bridged wrappers around the native operations.
- `with_stack_guard` L10989-10991 [P]: the test-build call counter.
- `type_face_name_if_atom` L12256-12265, `type_hold_type` L12317-12327,
  `find_face_axis_skip` L12487-12495, and `find_face_axis_skip_inner`
  L12498-12577 with all its branches (B12508 through B12565) [P].
- `mint_tisgar_chain_chunked` L12712-12756, B12720T, B12720F, B12738T,
  B12738F, B12752T, B12752F [P]: the chunked-mint prototype. The production
  chunked prelude mint is `mint_honc_prelude_chunked` in `bin/honk.rs`.
- `ty_face` L12780-12783 and the paired constructors `ty_noun_n`
  L12933-12935, `ty_void_n` L12938-12940, `ty_cell_n` L12954-12963,
  `ty_face_tool_n` L12966-12988, `ty_face_n` L12991-12999, `ty_hint_n`
  L13002-13034, `ty_hold_n` L13037-13053, `ty_core_n` L13056-13084, `ty_fork_n`
  L13087-13091 [P].
- `is_const_bool_formula` L14092-14098, B14093T, B14093F, B14096T, B14096F,
  B14097T, B14097F; `cell_type` L14101-14102, L14110-14116, B14102F, B14112T,
  B14112F, B14112c2T, B14112c2F; `cell_type_n` L14120-14134, B14127T, B14127F,
  B14130T, B14130F [P].
- `cell_type` L14103-14109, B14102T [UP]: the `HONK_NATIVE_TYPES` branch. The
  flag is read once per process, so a test that sets it would race the others.

## Unreachable from source

Skins:

- L11575 [UP]: the `?` on `collect_lazy_resolver_arms_from_tomes_map` in
  `mull_mile`, which fails only on a malformed tomes map; `mull_mile` passes
  the map `mull` just built.
- L9639-9640 (gain), L9986-9987 (lose) [P]: a `[%base %null]` skin. `flay`
  turns `~` into a `%rock %n` leaf, so no source skin has this form.
- L9642, L9989 [P]: a `[%base %void]` skin. `!!` is `%zpzp`, which `flay`
  rejects.
- L9664, L10004 [P]: `Skin::Dbug`. `flay` never builds `%dbug` skins (in
  hoon-138 or hatch).
- L9665-9668, L9671-9675, L10005 [P]: `Skin::Help`. It needs a `%note %help`
  hoon in `?#` skin position; hatch attaches docs only to `^=` names, which
  `flay` then rejects.
- B9645T, L9646 (`%noun` base gain), L9716 (`gain_atom_skin`), L9791
  (`gain_cell_skin`), L9879 (`gain_leaf_skin`), L10032 (`lose_atom_skin`),
  L10084 (`lose_cell_skin`), L10173 (`lose_leaf_skin`), L10323
  (`miss_dext_uncached`) [P]: a void ref. The face, hint, cell and fork
  constructors collapse void, and a void leg makes the subject void, which
  fails with mint-vain first.
- L9795 [P]: `gain_cell_skin` on a `%noun` ref whose head skin gains `%void`.
  From `%noun` only `[%base %void]` does that, and `flay` never builds it.
- B9894T, B9896T, B9897F, B9902T, L9903 [P]: a leaf skin with an empty or `@`
  aura. Leaf skins come only from `%rock`s, which always carry an aura.
- B9692T, L9693 (`gain spec`), B10015T, L10016 (`lose spec`) [P]: `ar:fish`
  already requires the ref to nest in the spec, so after a successful mint
  this check cannot fail.

Type operations:

- B10611T, L10612-10614 [P]: crop-loop. A `?=` computes the gain before the
  lose, and the gain fails with fuse-loop first.
- L10649 [P]: the `crop_sint` fallback. `crop_inner` handles `%void` and
  `%noun` refs before calling it, and its atom, cell and core callers handle
  atom and cell refs themselves.
- B10698T, L10699 (`end_bytes`), B10715T, L10716 (`rsh_bytes`) [UP]: dead.
  `$` is 0, so the `fitz` loop returns before `end_bytes` sees an empty atom,
  and `rsh_bytes` returns `$` before `rest` can be empty.
- B10723c3F [UP]: the `p_w != 0` test is reached only when `p_w != 0`.
- B10923c5T, B10923c6T, B10923c6F, B10934T, L10935 [UP]: the blocked-context
  arm of `peek`. `peel` returns `con = true` only together with `sam = true`,
  so the first clause always matches first.

`mull`:

- B11325T, B11340T, L11341 [UP]: an axis mismatch between the sut and dox
  wings. Wet samples keep the declared faces, so a wing resolves to the same
  axis on both sides.
- B11364T, B11364F, L11364-11368 [P]: `%lost` in `mull`. mint rejects a
  `%lost` on a non-void subject first (mint-lost), a void subject fails with
  mull-none before `mull_inner`, and the `vet`-off branch is unreachable (next
  entry).
- B11465T, L11466 (`mull_nice`), B11688F, L11690 (`mull_cnts_with_ports`) [P]:
  `vet` off inside `mull`. hoon-138 runs `mull` only under `vet` (`fire`
  checks `!vet` first).
- B11457F, L11459-11460 [P]: mull-open. Every parsed gene has a `mull` arm or
  opens.
- B11673T, L11674 [P]: edits on a synthetic port. `tack` rejects them in mint
  first.
- L11681 [P]: mixed synthetic and natural ports. sut and dox share the
  definition context, so a wing resolves synthetically on both sides or on
  neither.
- B11718T, L11719-11721, B11739T, L11740-11742, B11752T, L11753-11755,
  L11766-11768 [P]: `mull_endo` axis and opal mismatches. The sut and dox cores
  share arm layout and sample faces.

Atoms, axes and nouns:

- L11776 (`atom_is_zero`), L11783-11785, L11787-11788, B11785F, B11788T,
  B11788F (`atom_is_flag`) [P]: the `ParsedAtom::Big` arms. `play_sand` calls
  them only for `%n` and `%f` sands, whose source atoms (`~` and the loobean
  literals) the parser stores as `ParsedAtom::Small`.
- B11785T, L11786 [UP]: dead. `BigUint::to_bytes_le()` of zero is `[0]`, never
  empty.
- B12580T, L12581 (`axis_cap_mas`), B12612T, L12613 (`axis_big_cap_mas`),
  B12652T, L12653-12655 (`peg_axis_big_pair`) [P]: defensive `axis <= 1` and
  `b == 0` checks.
- B12586T, L12587-12589 (`axis_cap_mas`), B12616T, L12617-12619
  (`axis_big_cap_mas`) [UP]: dead. `axis > 1` implies a bit length of at least
  2.
- B12088T, B12089T, B12089F, L12089-12093 [P]: `map_put_mug` on an existing
  key. Every caller builds the map from a `HashMap`, so keys are unique.
- L12105-12106, L12130-12131 [UP]: `map_put_mug` missing-branches errors. The
  recursive call returns a node it just built.
- L12359-12360 [P]: a fork-set node without branches on the push side of
  `fork_set_options`. Defensive against malformed type nouns.
- L12374-12375 [UP]: the same error on the pop side, which re-reads a node the
  push side already checked.
- B12349T, L12350-12352 [P]: the million-node budget of `fork_set_options`.
  Defensive. A unit test reaches it with a shared-subtree DAG, but a fork
  built from source is a treap with one node per distinct member type, so it
  would need a million different types.
- B12203T, L12204 [P]: `type_tag_kind` on a void core payload. `ty_core` is its
  only production caller, and the native constructors collapse a void payload
  first.
- B12219F, L12222-12223 [P]: an unknown type tag. No type noun has one.
- L12788 (`ty_face_tool`), L12817 (`ty_core`) [P]: a void inner type or
  payload. Callers pass types the native constructors already collapsed.
- L12467-12469 [P]: an unknown foot tag in `foot_parts`. Defensive.
- L12893-12899 [P]: `NativeForkOptionIter::size_hint`. Allocation sizing only;
  no compile path calls it.
- L13453 [P]: a tune alias without a hoon in `tune_to_noun`. Parsed tunes
  always carry one (`=*`, busk).

`%hand` lowering:

- L11160-11163 (`mull_inner`), `type_to_noun` L13479-13525,
  `face_type_to_noun` L13527-13535, `coil_to_noun` L13594-13621,
  `garb_to_noun` L13623-13634, `poly_to_noun` L13636-13641, `vair_to_noun`
  L13643-13650, `semi_noun_expr_to_noun` L13652-13656, `stencil_to_noun`
  L13658-13679, `block_to_noun` L13681-13687, `gate_to_noun` L13706-13711,
  `spec_to_noun` L13713-13907, `basetype_to_noun` L13909-13921,
  `skin_to_noun` L13923-13971, `atom_to_noun` L14004-14006, `nock_to_noun`
  L14008-14078, `nock_hint_to_noun` L14080-14089 [P]: `Hoon::Hand` comes only
  from `noun_to_hoon`, never from parsing.

Note wings:

- L13552-13557, L13561 (`note_to_noun`), `wing_to_noun` L13568-13574,
  `limb_to_noun` L13576-13592, `slot_formula_axis_noun` L11828-11830,
  `slot_formula_axis_big` L11832-11835 [P]: a `%made` note with a wing list.
  It comes only from `example` or `relative` of a `%made` spec, which neither
  hatch's parser nor hoon-138 builds (hatch makes `Spec::Made` only in
  `noun_to_spec`).

## No hoonc reference (P; unit-tested)

- L13983-13986: `spot_hint_formula`, which the chunked prelude mint calls to
  put back the spots it peels from a prelude parsed with dbug on. Only a
  non-canonical prelude outside native parity is minted that way, and hoonc
  has no build that takes a prelude.

## Rejected by both compilers

Each case is unit-tested, and hoonc fails on the same program.

- B9727c2T, L9728-9730 (`gain_atom_skin`), B9897c2T, L9898-9900
  (`gain_leaf_skin`) [P]: atom-mismatch (`?#(@ud t)`, `?#(%5 t)` on a `@t`).
- B10493T, L10494-10496 [P]: fuse-loop (`?=(@ v)` with `v=$@(@ r)`).
- L11030 [P]: mull-none (`=>(!! a)` in a wet arm).
- L11274 [P]: mull-bonk-b.
- L11292 [P]: mull-bonk-c.
- B11325c2T, L11326 [P]: mull-bonk-a (`?=(_a a)`).
- B11344T, L11345 [P]: mull-bonk-x (a non-nesting `?#` wing).
- B11403T, L11404 [P]: mull-bonk-f (`!@(p.a ...)`).
- L11420 [P]: `%eror` in `mull`, from a duplicate arm or chapter. hoon-138
  `++open` crashes on `%eror`.

One rejection has no hoonc verdict:

- L9700-9704 [P]: gain-wash, a wash skin as a `?:` condition. hoon-138
  `ar:gain` recurses forever on it, so hoonc never finishes and no
  hoonc-checked compile reaches this line. The native-only probe
  `../../reject/wash_gain_loop.hoon` pins honk's error.

## Caches and hash collisions

- B10270c2F, B10274F [P]: `set_miss_memo_persistence` enabling persistence
  that is already on. The `honk` binary enables it only on a `Ut` where it is
  off.
- B10359c4T [P]: the reversed-pair hold guard in `miss_dext_uncached`. It needs
  mutually recursive molds through `redo`.
- L10856, B10854F, B10854c2F (`seen_hold`), L10880-10881, B10877F, B10877c2F,
  B10883F (`unsee_hold`) [UP]: a `peek` seen-hold bucket holding a different
  (hold, axis) pair. That needs a collision of the combined hash.
- L10886, B10873F, B10876F [UP]: dead. `unsee_hold` removes only a pair that
  `seen_hold` inserted, so its bucket and entry are always present.
- L11021-11022 [UP]: the `mull` cache signature fallback. Every AST gets a
  signature.
- L11042 [UP]: the error return of `mull_cache_store`. The lookup has already
  computed the same fan key.
- B11863T, L11864, B11869F, L11870, L11886-11887, B11901F, B11902T, B11902F,
  L11902-11903, L11907 [P]: `dor`, the tie-break of `gor_mug` and `mor_mug`. It
  runs only when two treap keys have equal mugs, so which arms run depends on
  hash collisions in the compiled programs.
