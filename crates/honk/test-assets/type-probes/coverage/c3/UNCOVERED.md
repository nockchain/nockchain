# c3 ledger: uncovered code in `native/ut/mod.rs` from line 9850

This package covers `crates/honk/src/native/ut/mod.rs` from line 9850 (the skin
`gain`/`lose` helpers) to the end of the file.

Uncovered by unit tests only (`U`): 0 lines and 0 branch outcomes. By the parity
corpus only (`P`): 901 lines and 84 branch outcomes. By both (`UP`): 59 lines
and 30 branch outcomes. `L<n>` is a line and `B<n>T`/`B<n>F` a branch outcome;
`c<k>` names the k-th condition of a compound test. `P` entries are covered by
unit tests.

## Test-only code

These items are `#[cfg(test)]`, so the parity build never runs them.

- `fuse_noun` L10515-10520, `miss_noun` L10540-10544, `crop_noun` L10836-10841,
  `fork_from_options_n` L11094-11098, `peek_noun` L11291-11295, `mull_noun`
  L11380-11389 [P]: noun-bridged wrappers around the native operations.
- `with_stack_guard` L11303-11305 [P]: the test-build call counter.
- `type_face_name_if_atom` L12587-12596, `type_hold_type` L12648-12658,
  `find_face_axis_skip` L12818-12826, and `find_face_axis_skip_inner`
  L12829-12908 with all its branches (B12839 through B12896) [P].
- `mint_tisgar_chain_chunked` L13043-13087, B13051T, B13051F, B13069T,
  B13069F, B13083T, B13083F [P]: the chunked-mint prototype. The production
  chunked prelude mint is `mint_honc_prelude_chunked` in `bin/honk.rs`.
- `ty_face` L13111-13114 and the paired constructors `ty_noun_n`
  L13264-13266, `ty_void_n` L13269-13271, `ty_cell_n` L13285-13294,
  `ty_face_tool_n` L13297-13319, `ty_face_n` L13322-13330, `ty_hint_n`
  L13333-13365, `ty_hold_n` L13368-13384, `ty_core_n` L13387-13415, `ty_fork_n`
  L13418-13422 [P].
- `is_const_bool_formula` L14423-14429, B14424T, B14424F, B14427T, B14427F,
  B14428T, B14428F; `cell_type` L14432-14433, L14441-14447, B14433F, B14443T,
  B14443F, B14443c2T, B14443c2F; `cell_type_n` L14451-14465, B14458T, B14458F,
  B14461T, B14461F [P].
- `cell_type` L14434-14440, B14433T [UP]: the `HONK_NATIVE_TYPES` branch. The
  flag is read once per process, so a test that sets it would race the others.

## Unreachable from source

Skins:

- L11906 [UP]: the `?` on `collect_lazy_resolver_arms_from_tomes_map` in
  `mull_mile`, which fails only on a malformed tomes map; `mull_mile` passes
  the map `mull` just built.
- L9909-9910 (gain), L10256-10257 (lose) [P]: a `[%base %null]` skin. `flay`
  turns `~` into a `%rock %n` leaf, so no source skin has this form.
- L9912, L10259 [P]: a `[%base %void]` skin. `!!` is `%zpzp`, which `flay`
  rejects.
- L9934, L10274 [P]: `Skin::Dbug`. `flay` never builds `%dbug` skins (in
  hoon-138 or hatch).
- L9935-9938, L9941-9945, L10275 [P]: `Skin::Help`. It needs a `%note %help`
  hoon in `?#` skin position; hatch attaches docs only to `^=` names, which
  `flay` then rejects.
- B9915T, L9916 (`%noun` base gain), L9986 (`gain_atom_skin`), L10061
  (`gain_cell_skin`), L10149 (`gain_leaf_skin`), L10302 (`lose_atom_skin`),
  L10354 (`lose_cell_skin`), L10443 (`lose_leaf_skin`), L10625
  (`miss_dext_uncached`) [P]: a void ref. The face, hint, cell and fork
  constructors collapse void, and a void leg makes the subject void, which
  fails with mint-vain first.
- L10065 [P]: `gain_cell_skin` on a `%noun` ref whose head skin gains `%void`.
  From `%noun` only `[%base %void]` does that, and `flay` never builds it.
- B10164T, B10166T, B10167F, B10172T, L10173 [P]: a leaf skin with an empty or `@`
  aura. Leaf skins come only from `%rock`s, which always carry an aura.
- B9962T, L9963 (`gain spec`), B10285T, L10286 (`lose spec`) [P]: `ar:fish`
  already requires the ref to nest in the spec, so after a successful mint
  this check cannot fail.

Type operations:

- B10925T, L10926-10928 [P]: crop-loop. A `?=` computes the gain before the
  lose, and the gain fails with fuse-loop first.
- L10963 [P]: the `crop_sint` fallback. `crop_inner` handles `%void` and
  `%noun` refs before calling it, and its atom, cell and core callers handle
  atom and cell refs themselves.
- B11012T, L11013 (`end_bytes`), B11029T, L11030 (`rsh_bytes`) [UP]: dead.
  `$` is 0, so the `fitz` loop returns before `end_bytes` sees an empty atom,
  and `rsh_bytes` returns `$` before `rest` can be empty.
- B11037c3F [UP]: the `p_w != 0` test is reached only when `p_w != 0`.
- B11237c5T, B11237c6T, B11237c6F, B11248T, L11249 [UP]: the blocked-context
  arm of `peek`. `peel` returns `con = true` only together with `sam = true`,
  so the first clause always matches first.

`mull`:

- B11656T, B11671T, L11672 [UP]: an axis mismatch between the sut and dox
  wings. Wet samples keep the declared faces, so a wing resolves to the same
  axis on both sides.
- B11695T, B11695F, L11695-11699 [P]: `%lost` in `mull`. mint rejects a
  `%lost` on a non-void subject first (mint-lost), a void subject fails with
  mull-none before `mull_inner`, and the `vet`-off branch is unreachable (next
  entry).
- B11796T, L11797 (`mull_nice`), B12019F, L12021 (`mull_cnts_with_ports`) [P]:
  `vet` off inside `mull`. hoon-138 runs `mull` only under `vet` (`fire`
  checks `!vet` first).
- B11788F, L11790-11791 [P]: mull-open. Every parsed gene has a `mull` arm or
  opens.
- B12004T, L12005 [P]: edits on a synthetic port. `tack` rejects them in mint
  first.
- L12012 [P]: mixed synthetic and natural ports. sut and dox share the
  definition context, so a wing resolves synthetically on both sides or on
  neither.
- B12049T, L12050-12052, B12070T, L12071-12073, B12083T, L12084-12086,
  L12097-12099 [P]: `mull_endo` axis and opal mismatches. The sut and dox cores
  share arm layout and sample faces.

Atoms, axes and nouns:

- L12107 (`atom_is_zero`), L12114-12116, L12118-12119, B12116F, B12119T,
  B12119F (`atom_is_flag`) [P]: the `ParsedAtom::Big` arms. `play_sand` calls
  them only for `%n` and `%f` sands, whose source atoms (`~` and the loobean
  literals) the parser stores as `ParsedAtom::Small`.
- B12116T, L12117 [UP]: dead. `BigUint::to_bytes_le()` of zero is `[0]`, never
  empty.
- B12911T, L12912 (`axis_cap_mas`), B12943T, L12944 (`axis_big_cap_mas`),
  B12983T, L12984-12986 (`peg_axis_big_pair`) [P]: defensive `axis <= 1` and
  `b == 0` checks.
- B12917T, L12918-12920 (`axis_cap_mas`), B12947T, L12948-12950
  (`axis_big_cap_mas`) [UP]: dead. `axis > 1` implies a bit length of at least
  2.
- B12419T, B12420T, B12420F, L12420-12424 [P]: `map_put_mug` on an existing
  key. Every caller builds the map from a `HashMap`, so keys are unique.
- L12436-12437, L12461-12462 [UP]: `map_put_mug` missing-branches errors. The
  recursive call returns a node it just built.
- L12690-12691 [P]: a fork-set node without branches on the push side of
  `fork_set_options`. Defensive against malformed type nouns.
- L12705-12706 [UP]: the same error on the pop side, which re-reads a node the
  push side already checked.
- B12680T, L12681-12683 [P]: the million-node budget of `fork_set_options`.
  Defensive. A unit test reaches it with a shared-subtree DAG, but a fork
  built from source is a treap with one node per distinct member type, so it
  would need a million different types.
- B12534T, L12535 [P]: `type_tag_kind` on a void core payload. `ty_core` is its
  only production caller, and the native constructors collapse a void payload
  first.
- B12550F, L12553-12554 [P]: an unknown type tag. No type noun has one.
- L13119 (`ty_face_tool`), L13148 (`ty_core`) [P]: a void inner type or
  payload. Callers pass types the native constructors already collapsed.
- L12798-12800 [P]: an unknown foot tag in `foot_parts`. Defensive.
- L13224-13230 [P]: `NativeForkOptionIter::size_hint`. Allocation sizing only;
  no compile path calls it.
- L13784 [P]: a tune alias without a hoon in `tune_to_noun`. Parsed tunes
  always carry one (`=*`, busk).

`%hand` lowering:

- L11491-11494 (`mull_inner`), `type_to_noun` L13810-13856,
  `face_type_to_noun` L13858-13866, `coil_to_noun` L13925-13952,
  `garb_to_noun` L13954-13965, `poly_to_noun` L13967-13972, `vair_to_noun`
  L13974-13981, `semi_noun_expr_to_noun` L13983-13987, `stencil_to_noun`
  L13989-14010, `block_to_noun` L14012-14018, `gate_to_noun` L14037-14042,
  `spec_to_noun` L14044-14238, `basetype_to_noun` L14240-14252,
  `skin_to_noun` L14254-14302, `atom_to_noun` L14335-14337, `nock_to_noun`
  L14339-14409, `nock_hint_to_noun` L14411-14420 [P]: `Hoon::Hand` comes only
  from `noun_to_hoon`, never from parsing.

Note wings:

- L13883-13888, L13892 (`note_to_noun`), `wing_to_noun` L13899-13905,
  `limb_to_noun` L13907-13923, `slot_formula_axis_noun` L12159-12161,
  `slot_formula_axis_big` L12163-12166 [P]: a `%made` note with a wing list.
  It comes only from `example` or `relative` of a `%made` spec, which neither
  hatch's parser nor hoon-138 builds (hatch makes `Spec::Made` only in
  `noun_to_spec`).

## No hoonc reference (P; unit-tested)

- L14314-14317: `spot_hint_formula`, which the chunked prelude mint calls to
  put back the spots it peels from a prelude parsed with dbug on. Only a
  non-canonical prelude outside native parity is minted that way, and hoonc
  has no build that takes a prelude.

## Rejected by both compilers

Each case is unit-tested, and hoonc fails on the same program.

- B9997c2T, L9998-10000 (`gain_atom_skin`), B10167c2T, L10168-10170
  (`gain_leaf_skin`) [P]: atom-mismatch (`?#(@ud t)`, `?#(%5 t)` on a `@t`).
- B10798T, L10799-10801 [P]: fuse-loop (`?=(@ v)` with `v=$@(@ r)`).
- L11361 [P]: mull-none (`=>(!! a)` in a wet arm).
- L11605 [P]: mull-bonk-b.
- L11623 [P]: mull-bonk-c.
- B11656c2T, L11657 [P]: mull-bonk-a (`?=(_a a)`).
- B11675T, L11676 [P]: mull-bonk-x (a non-nesting `?#` wing).
- B11734T, L11735 [P]: mull-bonk-f (`!@(p.a ...)`).
- L11751 [P]: `%eror` in `mull`, from a duplicate arm or chapter. hoon-138
  `++open` crashes on `%eror`.

One rejection has no hoonc verdict:

- L9970-9974 [P]: gain-wash, a wash skin as a `?:` condition. hoon-138
  `ar:gain` recurses forever on it, so hoonc never finishes and no
  hoonc-checked compile reaches this line. The native-only probe
  `../../reject/wash_gain_loop.hoon` pins honk's error.

## `HONK_MEMO_VERIFY` checks

- `fuse` L10497-10503, B10496T B10500T/F; `crop` L10818-10824, B10817T
  B10821T/F; `miss_dext` L10593-10600, 10602, 10604, B10590T B10598T/F [UP]:
  diagnostics only. The verify CLI test's program never hits these caches,
  and the parity corpus runs without `HONK_MEMO_VERIFY`.
- `mull` L11341-11349, 11353, B11340T B11348T [P]: diagnostics only; the
  verify CLI test covers them.
- `mull` L11351, 11354-11355, B11348F [UP]: a recompute that disagrees with
  its cached hit, which no build produces.

## Caches and hash collisions

- B10549c2F, B10553F [P]: `set_miss_memo_persistence` enabling persistence
  that is already on. The `honk` binary enables it only on a `Ut` where it is
  off.
- B10661c4T [P]: the reversed-pair hold guard in `miss_dext_uncached`. It needs
  mutually recursive molds through `redo`.
- L11170, B11168F, B11168c2F (`seen_hold`), L11194-11195, B11191F, B11191c2F,
  B11197F (`unsee_hold`) [UP]: a `peek` seen-hold bucket holding a different
  (hold, axis) pair. That needs a collision of the combined hash.
- L11200, B11187F, B11190F [UP]: dead. `unsee_hold` removes only a pair that
  `seen_hold` inserted, so its bucket and entry are always present.
- L11335-11336 [UP]: the `mull` cache signature fallback. Every AST gets a
  signature.
- L11373 [UP]: the error return of `mull_cache_store`. The lookup has already
  computed the same fan key.
- B12194T, L12195, B12200F, L12201, L12217-12218, B12232F, B12233T, B12233F,
  L12233-12234, L12238 [P]: `dor`, the tie-break of `gor_mug` and `mor_mug`. It
  runs only when two treap keys have equal mugs, so which arms run depends on
  hash collisions in the compiled programs.
