# c5 (native/ir) uncovered-branch ledger

Scope: `crates/honk/src/native/ir/*.rs`. Tags: `U` = not covered by unit
tests (`native::ir::cov_c5` plus the existing tests), `P` = not covered by the
parity corpus plus the `coverage/c5` probes. Line numbers match this branch.

Two facts explain most `P` entries:

- **Debug-only code.** `mod.rs::{roundtrip_check, type_roundtrip_check,
  type_intern_stats}`, the tree `formula.rs` IR, `ty.rs::BoundaryType`,
  `intern.rs::intern_boundary`, and the jam form of `Leaf` run only when
  `HONK_IR_ROUNDTRIP` is set (`bin/honk.rs`, `native/mod.rs`). Parity runs honk
  without it.
- **Every source expression carries a `%dbug` spot.** Formula operands therefore
  reach the peephole constructors as `[11 spot f]`, never as a bare `[0 0]` or
  `[2 [0 x] [0 y]]`. The unit tests reach those shapes through the arena API,
  and through dbug-off source parsing in `mint_source`.

## formula.rs

- L35-373, all branches [P]: tree formula IR, used only by the `HONK_IR_ROUNDTRIP` check (debug-only). Unit-covered.
- L559, L617, B558F [UP]: panic and `unreachable!` arms in the file's existing tests (test code).

## formula_dag.rs

- L37-46, 246-248, 251-254, 259-260, 270-272, 274, 277-278, 281-282, 285, 287-288, 295-300, 304-307, 311-316, 320-323, 326; B243F B245T B250F B257T [P]: `import` of anything except a `[1 …]` quote. Only `%hand` (`mint_hand`) imports general formulas. The hoon-138 parser never builds `%hand`; only noun-to-hoon conversion does. `.^` imports only its `[1 [138 type]]` hint, which `c5_scry_import` covers along with the backfill (L164). Unit-covered.
- L289-290 [UP]: unreachable. This is the heap-fallback copy of the `smallvec!` arguments, and two arguments never exceed the inline capacity of 2.
- L72 [P]: `Axis::is_one` on `Axis::Big`. `comb` tests a big bare slot for `[0 1]` only after a non-slot `mal`. Source slots are dbug-wrapped, and big-axis wings through arms are composed inside the vein. A `+36893488147419103232.arm` probe compiled but did not reach it. Unit-covered.
- L182-184 [P]: `distinct()` is a statistics accessor with no compiler caller. Unit-covered.
- L346 [UP]: unreachable. The arena builds leaves only with `Leaf::from_noun_raw` (`Direct`/`Noun`), never `Jammed`.
- L358 [UP]: unreachable. `Raw` nodes are created only by `import`, with `materialized: Some(noun)`, so `materialize` returns before this line.
- L414 [P]: defensive `unreachable!` for an `op` of arity 0 or 3+. The compiler never builds one. Unit test uses `should_panic`.
- B333F [UP]: unreachable. Only cells get to L333, and a cell is never direct.
- L464 [P]: `cove` through a `%11` hint. `cove` reads the mint of a bare `[%wing …]`, which is `[0 a]` or an arm kick, never a hint. Unit-covered.
- L466 [P]: `cove` error. It is reachable only as a rejection: `?=` on an arm wing inside a wet gate that is called and so mulled. hoonc rejects the same program with `cove` (checked with probe-diag). Unit test: `source_fits_on_an_arm_in_a_mulled_wet_gate_is_rejected_by_cove`.
- L497, 500-513; B493F B495F B499T B500T/F B504T/F B504c2T/F [P]: comb rule-1 fallthroughs and rule 1b need a bare `[0 0]` or `[2 [0 x] [0 y]]` operand. The dbug-wrapped source never provides one (see the top of this file). Unit-covered, including by dbug-off source parsing.
- L541, 544, 547; B540T B543T B546T [P]: `flip` is only ever called by fish on `[3 [0 axis]]`, never on a constant or a crash. Unit-covered.
- B558T B564T B576T B582T [P]: `flan`/`flor` with a `[0 0]` operand. Fish and `?#` skin tests never emit `[0 0]`. Unit-covered.

## intern.rs

- L44-47, 107-109; B42F B100F [P]: malformed type nouns with unknown tags. Defensive: compiler-built type nouns are always well-formed. Unit-covered.
- L217-219 [P]: `Context::default` has no compiler caller (`Ut` uses `Context::new`). Unit-covered.
- B247F [P]: `native_of_mug_insert` with a handle already in the bucket. `native_of_cached` returns early on any structurally equal candidate, so a freshly decoded handle is never already present. Performance-cache only. Unit-covered.
- L563, B567T [UP]: `cfg(test)` `HONK_NATIVE_TYPES` toggle, a process-wide static that only the environment sets (test infrastructure).
- L711 [P]: `cons_hint(_, %noun)`. `hint_type` returns `%noun` payloads before calling it, and take/gain/lose/wrap never turn a non-noun hint payload into exactly `%noun`. Unit-covered.
- L739-747, B741T/F [P]: `live_leaf_to_noun` of `Leaf::Jammed`. Live types carry only `Direct`/`Noun` leaves; `Jammed` exists only on the debug path. Unit-covered.
- L765-766 [P]: failure message of the `cfg(test)` oracle `assert_native_eq`. Unit-covered (`should_panic`).
- L808, B807F [P]: a live leaf for an indirect atom that still fits a u64 (2^63..2^64). Source type leaves are ASCII terms or cells, never in that range. Unit-covered.
- L816, B815F [UP]: unreachable. Live mug buckets hold only `Leaf::Noun`.
- L839-891 [P]: `intern_boundary`, used only by the debug `type_intern_stats`. Unit-covered.
- B977F B991F B991c2F B991c3F B1022F L1023 [UP]: `node_eq` returning false on child `TypeId`s, garb, or variant. These fields are hashed exactly, so reaching this needs a 64-bit `NodeHash` (SipHash) collision, which cannot be constructed. (The leaf-carried cases are reachable. B976F and B1001F are covered in parity by `c5_mug_collision`, which uses real 31-bit mug collisions, and all three leaf cases are covered by unit tests that forge mugs.)
- B1011F [P]: hint heads with colliding mugs over an equal payload. A hint head embeds its subject type, and that differs whenever the note differs (tried with `+$` `%made` notes). Unit-covered with a forged mug.
- L1070, L1137, B1068F B1136F [UP]: panic arms in the file's existing tests (test code).

## leaf.rs

- L47-60, 80-85, 89, 98-99, 108, 121-124; B48 B49 B99 [P]: the jam form (`Leaf::from_noun`, `Jammed` eq/hash/to_noun) serves only the debug round trip. `Leaf::Noun::to_noun` (L89) is bypassed live: `live_leaf_to_noun` and `leaf_noun` use the raw noun. Unit-covered.

## mod.rs

- L28-83 [P]: `roundtrip_check`, `type_intern_stats`, `type_roundtrip_check`, all debug-only. Unit-covered.
- L42-46, 77-81, B39F B74F [UP]: unreachable. The mismatch arms need decode followed by encode to change the jam, but every accepted formula or type node re-emits its source exactly (leaves are carried verbatim).

## ty.rs

- L73-75, 117-119, 123-125, 130-132, 139-141 [P]: `TypeRef::identity` and the `AsRef`/`Debug`/`PartialEq`/`Hash` impls are unused by the compiler, which calls `ptr_eq`/`arena_id`. Unit-covered.
- L173-175, 214-224, B211F [P]: named core garbs (`nym = [~ term]`). The hoon-138 parser always builds `|%`/`|@` with a null name (`runo cen %brcn ~`), and every desugaring passes `~`. Unit-covered.
- L237-239, 254-256 [P]: malformed garb terms. Defensive. Unit-covered. (The `%zinc` decode, L252, is covered by `c5_zinc_nest`.)
- L331-380 [P]: `Type::to_noun` is used only by the `cfg(test)` oracle. The live compiler uses `live_to_noun_node`, which emits the same shapes. Unit-covered.
- L420-550, B472-B539 [P]: `BoundaryType`, debug-only. Unit-covered.
- L568-570, B567T, L577-578 [P]: fork treaps that are malformed or over the 1M-node budget. Defensive. Unit-covered (the 1M-node test uses a shared-subtree DAG).
- L592-593 [UP]: unreachable. After the pop, the node's branch pair is checked again, but it was already checked before the push.
- L684, B683F [UP]: panic arm in the file's existing tests (test code).

## value_dag.rs

- L71, B70T [UP]: unreachable. `import` skips already-registered nouns before it calls `intern_atom`, which has no other caller.
- L112, B111T [P]: `intern_cell_with_noun` is called only from `semi_combine`, always with a freshly allocated cell. Unit-covered.
