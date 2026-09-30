# c5 (native/ir) uncovered-branch ledger

Range: `crates/honk/src/native/ir/*.rs`, excluding `#[cfg(test)]` modules and
`cov_c5.rs`. Gaps: 652 lines (0 unit-only, 632 parity-only, 20 both) and 100
branch outcomes (0 unit-only, 89 parity-only, 11 both). Tags: `U` = not
covered by unit tests, `P` = not covered by the parity corpus plus the
`coverage/c5` probes, `UP` = both. Line numbers match the current source.

## formula.rs

- L35-407 and every branch outcome in that range (B215, B224-B260, B276,
  B303) [P]: the tree formula IR (`Axis`, `Formula::to_noun`/`from_noun`,
  `cons`/`comb`/`cond`, `peg`, `pair_leaf`). It backs only the
  `HONK_IR_ROUNDTRIP` check (`mod.rs::roundtrip_check`), and parity runs honk
  without that variable. Debug-only; unit-covered.

## formula_dag.rs

- L37-46, 246, 248, 251-254, 259-260, 270-272, 274, 277-278, 281-282, 285,
  287-288, 295-296, 298-300, 304, 306-307, 311-312, 314-316, 320, 322-323,
  326; B243F B245T B250F B257T [P]: `import` of anything except a `[1 …]`
  quote, and `Axis::from_noun`, which only `import` calls. Only `%hand`
  (`mint_hand`) imports general formulas, and the hoon-138 parser never builds
  `%hand` (only noun-to-hoon conversion does). `.^` imports only its
  `[1 [138 type]]` hint, which `c5_scry_import` covers along with the
  materialized backfill (L163-164). Unit-covered.
- L289-290 [UP]: unreachable. This is the heap-fallback copy of the
  `smallvec!` arguments, and two arguments never exceed the inline capacity of
  2.
- L72 [P]: `Axis::is_one` on `Axis::Big`. `comb` tests a big bare slot for
  `[0 1]` only after a non-slot `mal`; source slots are dbug-wrapped, and
  big-axis wings through arms are composed inside the vein. A
  `+36893488147419103232.arm` probe compiles but does not reach it.
  Unit-covered.
- L182-184 [P]: `distinct()` is a statistics accessor with no compiler caller.
  Unit-covered.
- L346 [UP]: unreachable. The arena builds leaves only with
  `Leaf::from_noun_raw` (`Direct`/`Noun`), never `Jammed`.
- L358 [UP]: unreachable. `Raw` nodes are created only by `import`, with
  `materialized: Some(noun)`, so `materialize` returns before this line.
- B333F [UP]: unreachable. Only cells reach L333, and a cell is never direct.
- L414 [P]: defensive `unreachable!` for an `op` of arity 0 or 3+. The
  compiler never builds one. The unit test uses `should_panic`.
- L464 [P]: `cove` through a `%11` hint. `cove` reads the mint of a bare
  `[%wing …]`, which is `[0 a]` or an arm kick, never a hint. Unit-covered.
- L497, 500-502, 504-510, 513; B493F B495F B499T B500T/F B504T/F B504c2T/F
  [P]: `comb` rule-1 fallthroughs and rule 1b need a bare `[0 0]` or
  `[2 [0 x] [0 y]]` operand. Every source expression carries a `%dbug` spot,
  so operands reach the peephole as `[11 spot f]` and never take these shapes.
  Unit-covered through the arena API and dbug-off source parsing in
  `mint_source`.
- L541, 544, 547; B540T B543T B546T [P]: `flip` is only called by fish on
  `[3 [0 axis]]` (`formula_flip` in `ut/mod.rs`), never on a constant or a
  crash. Unit-covered.
- B558T B564T B576T B582T [P]: `flan`/`flor` with a `[0 0]` crash operand.
  Fish and `?#` skin tests never emit one. Unit-covered.
- L590-593 [P]: `and` has no compiler caller (only `cov_c5` calls it).
  Unit-covered.

## intern.rs

- L44-47, 107-109; B42F B100F [P]: malformed type nouns with unknown tags.
  Defensive: compiler-built type nouns are always well-formed. Unit-covered.
- L219-221 [P]: `Context::default` has no compiler caller (`Ut` uses
  `Context::new`). Unit-covered.
- B249F [P]: `native_of_mug_insert` with a handle already in the bucket.
  `native_of_cached` returns early on any structurally equal candidate, so a
  freshly decoded handle is never already present. Performance cache only.
  Unit-covered.
- L571, 573, 575, 577-579, 582; B578F [P], and L574, B578T [UP]:
  `live_enabled` is `#[cfg(test)]`, so the parity binary does not contain it.
  Its `HONK_NATIVE_TYPES` branch is a process-wide toggle that only the
  environment sets. Test-only code.
- L722 [P]: `cons_hint(_, %noun)`. `hint_type` returns `%noun` payloads before
  calling it, and take/gain/lose/wrap never turn a non-noun hint payload into
  exactly `%noun`. Unit-covered.
- L750-754, 756-758; B752T/F [P]: `live_leaf_to_noun` of `Leaf::Jammed`. Live
  types carry only `Direct`/`Noun` leaves; `Jammed` exists only on the debug
  path. Unit-covered.
- L765-774, 776-777, 779 [P]: `assert_native_eq` is the `#[cfg(test)]` type
  oracle, absent from the parity binary. Test-only code.
- L819, B818F [P]: a live leaf for an indirect atom that still fits a u64
  (2^63..2^64). Source type leaves are ASCII terms or cells, never in that
  range. Unit-covered.
- L827, B826F [UP]: unreachable. Live mug buckets hold only `Leaf::Noun`.
- L850-859, 862-887, 889-894, 896-899, 901-902 [P]: `intern_boundary`, used
  only by the debug `type_intern_stats`. Unit-covered.
- L1034; B988F B1002F B1002c2F B1002c3F B1033F [UP]: `node_eq` returning false
  on child `TypeId`s, garb, or variant. These fields are hashed exactly, so
  reaching this needs a 64-bit `NodeHash` (SipHash) collision, which cannot be
  constructed. The leaf-carried cases are reachable: `c5_mug_collision` covers
  B987F and B1012F in parity with real 31-bit mug collisions, and unit tests
  that forge mugs cover all three.
- B1022F [P]: hint heads with colliding mugs over an equal payload. A hint
  head embeds its subject type, and that differs whenever the note differs
  (tried with `+$` `%made` notes). Unit-covered with a forged mug.

## leaf.rs

- L47-52, 54-60, 80-85, 89, 98-99, 108, 121-124; B48T/F B49T/F B99T/F
  B99c2T/F [P]: the jam form (`Leaf::from_noun`, `Jammed` eq/hash/to_noun)
  serves only the debug round trip. `Leaf::Noun::to_noun` (L89) is bypassed
  live: `live_leaf_to_noun` and `leaf_noun` use the raw noun. Unit-covered.

## mod.rs

- L28-33, 35-37, 39-40, 48, 54-59, 63-68, 70-72, 74-75, 83; B39T B74T [P]:
  `roundtrip_check`, `type_intern_stats`, `type_roundtrip_check`, all
  debug-only (`HONK_IR_ROUNDTRIP`). Unit-covered.
- L42-46, 77-81; B39F B74F [UP]: unreachable. The mismatch arms need decode
  followed by encode to change the jam, but every accepted formula or type
  node re-emits its source exactly (leaves are carried verbatim).

## ty.rs

- L73-75, 117-119, 123-125, 130-132, 139-141 [P]: `TypeRef::identity` and the
  `AsRef`/`Debug`/`PartialEq`/`Hash` impls are unused by the compiler, which
  calls `ptr_eq`/`arena_id`. Unit-covered.
- L173-175, 214-221, 223-224; B211F [P]: named core garbs (`nym = [~ term]`).
  The hoon-138 parser always builds `|%`/`|@` with a null name
  (`runo cen %brcn ~`), and every desugaring passes `~`. Unit-covered.
- L237-239, 254-256 [P]: malformed garb terms. Defensive. Unit-covered. (The
  `%zinc` decode, L252, is covered by `c5_zinc_nest`.)
- L331-338, 340-343, 346-349, 351, 353-358, 360-363, 365-368, 370-372,
  374-377, 380 [P]: `Type::to_noun` is used only by the `#[cfg(test)]` oracle.
  The live compiler uses `live_to_noun_node`, which emits the same shapes.
  Unit-covered.
- L420-550 and every branch outcome in that range (B472-B539) [P]:
  `BoundaryType`, debug-only. Unit-covered.
- L568-570, 577-578; B567T [P]: fork treaps that are malformed or over the
  1M-node budget. Defensive. Unit-covered (the 1M-node test uses a
  shared-subtree DAG).
- L592-593 [UP]: unreachable. After the pop, the node's branch pair is checked
  again, but it was already checked before the push.

## value_dag.rs

- L71, B70T [UP]: unreachable. `import` skips already-registered nouns before
  it calls `intern_atom`, which has no other caller.
- L112, B111T [P]: `intern_cell_with_noun` is called only from
  `semi_combine`, always with a freshly allocated cell. Unit-covered.
