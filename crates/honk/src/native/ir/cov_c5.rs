//! Coverage-driven tests for the native IR arenas and interner.
//!
//! Added to close branch-coverage gaps; see the coverage report in the PR.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::rc::Rc as StdRc;
use std::sync::Arc;

#[allow(unused_imports)]
use nockvm::ext::AtomExt;
use nockvm::noun::{Atom, NounAllocator, D, T};
use num_bigint::BigUint;

use super::formula::{self as tree, Axis as TreeAxis, Formula};
use super::formula_dag::{FormulaArena, FormulaId};
use super::intern::{
    assert_native_eq, cons_hint, cons_noun, cons_void, live_intern, live_leaf_from_noun,
    live_leaf_to_noun, native_of, native_of_mug_candidates, native_of_mug_insert, Context,
    TypeTable,
};
use super::leaf::Leaf;
use super::ty::{
    garb_native, tas, visit_fork_set_members, BoundaryType, Garb, Type, TypeId, TypeRef,
};
use super::value_dag::ValueArena;
#[allow(unused_imports)]
use super::*;
use crate::native::identity::{JamHash, NockOpcode, NounMug};
use crate::native::noun::{noun_pair, slab_mug};
use crate::native::ut::types::{Poly, Vair};

// ---- helpers ---------------------------------------------------------------

/// Render a noun as `[a b c]` (tails flattened), atoms in decimal, atoms wider
/// than 64 bits in hex.
fn show(noun: Noun, space: &NounSpace) -> String {
    if let Ok(atom) = noun.in_space(space).as_atom() {
        return match atom.as_u64() {
            Ok(value) => value.to_string(),
            Err(_) => format!(
                "0x{}",
                BigUint::from_bytes_le(atom.as_ne_bytes()).to_str_radix(16)
            ),
        };
    }
    let mut parts = Vec::new();
    let mut current = noun;
    while let Ok((head, tail)) = noun_pair(current, space) {
        parts.push(show(head, space));
        current = tail;
    }
    parts.push(show(current, space));
    format!("[{}]", parts.join(" "))
}

fn jam_of(noun: Noun, space: &NounSpace) -> Vec<u8> {
    let mut slab: NounSlab = NounSlab::new();
    let copied = slab.copy_into(noun, space);
    slab.set_root(copied);
    slab.jam().to_vec()
}

fn tree_show(formula: &Formula) -> String {
    let mut slab: NounSlab = NounSlab::new();
    let noun = formula.to_noun(&mut slab);
    show(noun, &slab.noun_space())
}

fn big(bits: u32) -> BigUint {
    BigUint::from(1u8) << bits
}

fn big_atom(slab: &mut NounSlab, value: &BigUint) -> Noun {
    Atom::from_bytes(slab, &value.to_bytes_le()).as_noun()
}

/// Two distinct atoms above 2^64 (so both are indirect and neither fits a
/// `Leaf::Direct`) whose 31-bit mugs collide. Found by a birthday search.
fn colliding_big_atoms(slab: &mut NounSlab) -> (Noun, Noun) {
    let mut seen: HashMap<u32, Noun> = HashMap::new();
    for i in 0u64..4_000_000 {
        let mut bytes = i.to_le_bytes().to_vec();
        bytes.push(1);
        let atom = Atom::from_bytes(slab, &bytes).as_noun();
        let mug = slab_mug(atom, &slab.noun_space());
        if let Some(prior) = seen.insert(mug, atom) {
            return (prior, atom);
        }
    }
    panic!("no 31-bit mug collision among 4M atoms");
}

/// Parse `src` with dbug spots off (so formulas carry no `%11` spot hints) and
/// mint it against a `%noun` subject. Returns the rendered type and formula.
fn mint_source(src: &str) -> crate::errors::Result<(String, String)> {
    let hoon = crate::pipeline::parse_native_hoon_source(
        Path::new("cov-c5.hoon"),
        src,
        vec!["cov-c5".to_string()],
        false,
    )?;
    let mut slab: NounSlab = NounSlab::new();
    let noun = crate::native::ut::ty_noun(&mut slab);
    let (ty, formula) = {
        let mut ut = crate::native::ut::Ut::new(&mut slab);
        ut.mint_noun(noun, noun, &hoon)?
    };
    let space = slab.noun_space();
    Ok((show(ty, &space), show(formula, &space)))
}

// ---- tree formula IR (formula.rs) -------------------------------------------

#[test]
fn tree_hint_variants_encode_as_nock_eleven() {
    let jet = Formula::JetHint {
        clue: Leaf::Direct(tas("fast")),
        body: StdRc::new(Formula::Slot(TreeAxis::Small(1))),
    };
    assert_eq!(tree_show(&jet), format!("[11 {} 0 1]", tas("fast")));

    let note = Formula::NoteHint {
        note: Leaf::Direct(7),
        body: StdRc::new(Formula::Quote(Leaf::Direct(3))),
    };
    assert_eq!(tree_show(&note), "[11 7 1 3]");

    // The `ToNoun` trait impl emits the same noun as the inherent method.
    let mut slab: NounSlab = NounSlab::new();
    let via_trait = ToNoun::to_noun(&note, &mut slab);
    assert_eq!(show(via_trait, &slab.noun_space()), "[11 7 1 3]");
}

/// Native tree `comb` agrees with the noun `comb` and with an expected shape.
fn check_tree_comb(mal: Formula, buz: Formula, expect: &str) {
    let mut slab: NounSlab = NounSlab::new();
    let mal_noun = mal.to_noun(&mut slab);
    let buz_noun = buz.to_noun(&mut slab);
    let noun_result = crate::native::formula::comb(&mut slab, mal_noun, buz_noun).expect("comb");
    assert_eq!(show(noun_result, &slab.noun_space()), expect, "noun comb");
    assert_eq!(tree_show(&tree::comb(mal, buz)), expect, "native tree comb");
}

fn slot(axis: u64) -> Formula {
    Formula::Slot(TreeAxis::Small(axis))
}

fn quote(value: u64) -> Formula {
    Formula::Quote(Leaf::Direct(value))
}

fn eval(subject: Formula, formula: Formula) -> Formula {
    Formula::Eval(StdRc::new(subject), StdRc::new(formula))
}

#[test]
fn tree_comb_fallthroughs_match_noun_comb() {
    // mal = [0 0]: rule 1 needs a nonzero axis, so compose with %7.
    check_tree_comb(slot(0), slot(3), "[7 [0 0] 0 3]");
    // buz = [0 0]: rule 1a needs a nonzero axis; buz is no %2, so %7.
    check_tree_comb(slot(2), slot(0), "[7 [0 2] 0 0]");
    // buz = [2 [1 5] [0 3]]: rule 1b needs two slot operands.
    check_tree_comb(slot(2), eval(quote(5), slot(3)), "[7 [0 2] 2 [1 5] 0 3]");
    // buz = [2 [0 0] [0 3]] and [2 [0 3] [0 0]]: rule 1b needs nonzero axes.
    check_tree_comb(slot(2), eval(slot(0), slot(3)), "[7 [0 2] 2 [0 0] 0 3]");
    check_tree_comb(slot(2), eval(slot(3), slot(0)), "[7 [0 2] 2 [0 3] 0 0]");
    // rule 1b proper, for contrast.
    check_tree_comb(slot(6), eval(slot(2), slot(3)), "[2 [0 12] 0 13]");
    // mal = [[1 5] [0 2]]: a cell whose tail is not [0 1] skips rule 2.
    check_tree_comb(
        Formula::Cell(StdRc::new(quote(5)), StdRc::new(slot(2))),
        slot(3),
        "[7 [[1 5] 0 2] 0 3]",
    );
    // A non-canonical big-integer axis of 1 is still [0 1] for rules 2 and 3.
    let big_one = || Formula::Slot(TreeAxis::Big(StdRc::new(BigUint::from(1u8))));
    check_tree_comb(quote(3), big_one(), "[1 3]");
    check_tree_comb(
        Formula::Cell(StdRc::new(quote(4)), StdRc::new(big_one())),
        slot(7),
        "[8 [1 4] 0 7]",
    );
    // A genuinely big axis is not [0 1] (is_one on Axis::Big is false).
    let mut slab: NounSlab = NounSlab::new();
    let expected_axis = big_atom(&mut slab, &big(70));
    let expected = format!("[7 [1 3] 0 {}]", show(expected_axis, &slab.noun_space()));
    check_tree_comb(
        quote(3),
        Formula::Slot(TreeAxis::Big(StdRc::new(big(70)))),
        &expected,
    );
}

#[test]
fn tree_from_noun_decodes_eval_big_axes_and_rejects_unknown_opcodes() {
    let mut slab: NounSlab = NounSlab::new();
    let left = T(&mut slab, &[D(0), D(2)]);
    let right = T(&mut slab, &[D(0), D(3)]);
    let two = T(&mut slab, &[D(2), left, right]);
    let wide = big_atom(&mut slab, &big(80));
    let big_slot = T(&mut slab, &[D(0), wide]);
    let core = T(&mut slab, &[D(0), D(1)]);
    let big_kick = T(&mut slab, &[D(9), wide, core]);
    let thirteen = T(&mut slab, &[D(13), left, right]);
    let space = slab.noun_space();

    let parsed = Formula::from_noun(two, &space).expect("[2 …] decodes");
    assert!(matches!(parsed, Formula::Eval(..)));
    assert_eq!(tree_show(&parsed), "[2 [0 2] 0 3]");

    let parsed = Formula::from_noun(big_slot, &space).expect("big slot decodes");
    let Formula::Slot(TreeAxis::Big(axis)) = &parsed else {
        panic!("an axis above u64 decodes to Axis::Big");
    };
    assert_eq!(**axis, big(80));

    for noun in [two, big_slot, big_kick] {
        roundtrip_check(noun, &space).expect("formula IR round trip");
    }

    let err = match Formula::from_noun(thirteen, &space) {
        Ok(_) => panic!("opcode 13 is outside the tree IR"),
        Err(err) => err,
    };
    assert!(format!("{err:?}").contains("unsupported Nock opcode 13"));
    assert!(roundtrip_check(thirteen, &space).is_err());
}

// ---- type IR round trip and intern stats (mod.rs, ty.rs, intern.rs) --------

/// A type noun with every tag and a structurally duplicated (pointer-distinct)
/// subtree: `[%cell <core> [%cell <hint> [%cell <fork> [%cell <hold>
/// [%cell <face> [%cell <face'> [%cell %void %noun]]]]]]]`.
fn every_form_type_noun(slab: &mut NounSlab) -> Noun {
    let atom_ud = T(slab, &[D(tas("atom")), D(tas("ud")), D(0)]);
    let garb = T(slab, &[D(0), D(tas("dry")), D(tas("zinc"))]);
    let ctx_rest = T(slab, &[D(tas("noun")), D(0)]);
    let coil = T(slab, &[garb, ctx_rest]);
    let core = T(slab, &[D(tas("core")), atom_ud, coil]);
    let note = T(slab, &[D(0), D(tas("fast"))]);
    let hint = T(slab, &[D(tas("hint")), note, atom_ud]);
    let set_branches = T(slab, &[D(0), D(0)]);
    let set = T(slab, &[atom_ud, set_branches]);
    let fork = T(slab, &[D(tas("fork")), set]);
    let gene = T(slab, &[D(1), D(2)]);
    let hold = T(slab, &[D(tas("hold")), D(tas("noun")), gene]);
    let face_a = T(slab, &[D(tas("face")), D(tas("a")), atom_ud]);
    let atom_ud2 = T(slab, &[D(tas("atom")), D(tas("ud")), D(0)]);
    let face_b = T(slab, &[D(tas("face")), D(tas("a")), atom_ud2]);
    let tail = T(slab, &[D(tas("cell")), D(tas("void")), D(tas("noun"))]);
    let tail = T(slab, &[D(tas("cell")), face_b, tail]);
    let tail = T(slab, &[D(tas("cell")), face_a, tail]);
    let tail = T(slab, &[D(tas("cell")), hold, tail]);
    let tail = T(slab, &[D(tas("cell")), fork, tail]);
    let tail = T(slab, &[D(tas("cell")), hint, tail]);
    T(slab, &[D(tas("cell")), core, tail])
}

#[test]
fn type_boundary_round_trips_and_interns_every_form() {
    let mut slab: NounSlab = NounSlab::new();
    let noun = every_form_type_noun(&mut slab);
    let space = slab.noun_space();

    type_roundtrip_check(noun, &space).expect("type IR round trip");
    let (calls, distinct) = type_intern_stats(noun, &space).expect("intern stats");
    // Every node is visited, and the structurally equal `%atom`s and faces
    // collapse to one canonical node each.
    assert!(
        distinct < calls,
        "hash-consing collapsed duplicates: {distinct} < {calls}"
    );

    let parsed = BoundaryType::from_noun(noun, &space).expect("boundary decode");
    let mut table = TypeTable::new();
    let first = table.intern_boundary(&parsed);
    let second = table.intern_boundary(&parsed);
    assert!(
        TypeRef::ptr_eq(&first, &second),
        "re-interning is idempotent"
    );
    let Type::Cell(core, _) = &*first else {
        panic!("root is a cell");
    };
    let Type::Core { garb, .. } = &**core else {
        panic!("head is a core");
    };
    assert_eq!(garb.vair, Vair::Zinc);
}

#[test]
fn type_boundary_rejects_unknown_tags() {
    let mut slab: NounSlab = NounSlab::new();
    let bad_atom = D(tas("bogus"));
    let bad_cell = T(&mut slab, &[D(tas("bogus")), D(0)]);
    let space = slab.noun_space();
    for noun in [bad_atom, bad_cell] {
        let err = BoundaryType::from_noun(noun, &space).expect_err("unknown tag");
        assert!(format!("{err:?}").contains("unknown"));
        assert!(type_roundtrip_check(noun, &space).is_err());
        assert!(type_intern_stats(noun, &space).is_err());
    }
}

#[test]
fn garb_decoding_covers_names_every_vair_and_rejects_bad_terms() {
    let mut slab: NounSlab = NounSlab::new();
    for (vair_name, vair) in [
        ("gold", Vair::Gold),
        ("iron", Vair::Iron),
        ("lead", Vair::Lead),
        ("zinc", Vair::Zinc),
    ] {
        let nym = T(&mut slab, &[D(0), D(tas("foo"))]);
        let garb = T(&mut slab, &[nym, D(tas("wet")), D(tas(vair_name))]);
        let decoded = Garb::from_noun(garb, &slab.noun_space()).expect("garb");
        assert_eq!(decoded, garb_native(Some("foo"), Poly::Wet, vair));
        let mut out: NounSlab = NounSlab::new();
        let emitted = decoded.to_noun(&mut out);
        assert_eq!(
            jam_of(emitted, &out.noun_space()),
            jam_of(garb, &slab.noun_space())
        );
    }
    let bad_poly = T(&mut slab, &[D(0), D(tas("damp")), D(tas("gold"))]);
    let bad_vair = T(&mut slab, &[D(0), D(tas("dry")), D(tas("tin"))]);
    let space = slab.noun_space();
    let err = Garb::from_noun(bad_poly, &space).expect_err("bad poly");
    assert!(format!("{err:?}").contains("garb poly damp"));
    let err = Garb::from_noun(bad_vair, &space).expect_err("bad vair");
    assert!(format!("{err:?}").contains("garb vair tin"));
}

#[test]
fn fork_set_walk_rejects_missing_branches_and_enforces_node_budget() {
    let mut slab: NounSlab = NounSlab::new();
    // A treap node whose tail is an atom has no branch pair.
    let broken = T(&mut slab, &[D(tas("noun")), D(5)]);
    // A shared-subtree DAG whose tree expansion has 2^21 - 1 nodes, past the
    // 1_000_000-node budget, built from only 42 cells.
    let mut node = D(0);
    for level in 0..21u64 {
        let branches = T(&mut slab, &[node, node]);
        node = T(&mut slab, &[D(level), branches]);
    }
    let space = slab.noun_space();

    let err = visit_fork_set_members(broken, &space, |_| Ok(())).expect_err("missing branches");
    assert!(format!("{err:?}").contains("missing branches"));

    let mut visited = 0usize;
    let err = visit_fork_set_members(node, &space, |_| {
        visited += 1;
        Ok(())
    })
    .expect_err("budget");
    assert!(format!("{err:?}").contains("node budget"));
    assert!(
        visited < 1_000_000,
        "the walk stops before visiting every member"
    );
}

#[test]
fn type_ref_handles_hash_compare_and_debug_by_arena_identity() {
    let mut table = TypeTable::new();
    let noun = table.intern_shallow(Type::Noun);
    let void = table.intern_shallow(Type::Void);
    let noun_again = table.intern_shallow(Type::Noun);

    assert_eq!(noun.identity(), TypeId(0));
    assert_eq!(void.identity(), TypeId(1));
    assert_eq!(noun, noun_again);
    assert_ne!(noun, void);
    assert!(matches!(AsRef::<Type>::as_ref(&noun), Type::Noun));
    assert_eq!(format!("{noun:?}"), "Noun");

    let mut by_handle = DefaultHasher::new();
    noun.hash(&mut by_handle);
    let mut by_id = DefaultHasher::new();
    TypeId(0).hash(&mut by_id);
    assert_eq!(
        by_handle.finish(),
        by_id.finish(),
        "a handle hashes as its arena id"
    );
}

// ---- interner (intern.rs) -----------------------------------------------------

#[test]
fn intern_rejects_unknown_type_tags() {
    let mut slab: NounSlab = NounSlab::new();
    let bad_cell = T(&mut slab, &[D(tas("bogus")), D(0)]);
    let space = slab.noun_space();
    let mut cx = Context::default();
    let err = native_of(&mut cx, D(tas("bogus")), &space).expect_err("bad atom tag");
    assert!(format!("{err:?}").contains("unknown atom type tag"));
    let err = native_of(&mut cx, bad_cell, &space).expect_err("bad cell tag");
    assert!(format!("{err:?}").contains("unknown type tag"));
}

#[test]
fn cons_hint_normalizes_noun_and_void_payloads() {
    let mut cx = Context::new();
    let noun = cons_noun(&mut cx);
    let void = cons_void(&mut cx);
    let hinted_noun = cons_hint(&mut cx, Leaf::Direct(1), noun);
    let hinted_void = cons_hint(&mut cx, Leaf::Direct(1), void);
    assert!(
        TypeRef::ptr_eq(&hinted_noun, &noun),
        "hint(_, %noun) is %noun"
    );
    assert!(
        TypeRef::ptr_eq(&hinted_void, &void),
        "hint(_, %void) is %void"
    );
    let atom = live_intern(
        &mut cx,
        Type::Atom {
            aura: Leaf::Direct(tas("ud")),
            bits: Leaf::Direct(0),
        },
    );
    let hinted_atom = cons_hint(&mut cx, Leaf::Direct(1), atom);
    assert!(matches!(&*hinted_atom, Type::Hint { .. }));
}

#[test]
fn live_leaves_memoize_jammed_leaves_and_keep_wide_u64_atoms_direct() {
    let mut slab: NounSlab = NounSlab::new();
    let pair = T(&mut slab, &[D(1), D(2)]);
    let jammed = Leaf::from_noun(pair, &slab.noun_space());
    assert!(matches!(jammed, Leaf::Jammed(..)));

    let mut cx = Context::new();
    let first = live_leaf_to_noun(&mut cx, &jammed, &mut slab);
    let second = live_leaf_to_noun(&mut cx, &jammed, &mut slab);
    assert!(
        unsafe { first.raw_equals(&second) },
        "second lowering is memoized"
    );
    assert_eq!(show(first, &slab.noun_space()), "[1 2]");

    // u64::MAX is an indirect atom (direct atoms stop at 2^63 - 1) that still
    // fits a u64, so the live leaf is `Direct` rather than a raw noun.
    let wide = Atom::new(&mut slab, u64::MAX).as_noun();
    assert!(!wide.is_direct());
    let leaf = live_leaf_from_noun(&mut cx, wide, &slab.noun_space());
    assert!(matches!(leaf, Leaf::Direct(u64::MAX)));
}

#[test]
fn type_table_separates_nodes_whose_leaves_share_a_mug() {
    let mut slab: NounSlab = NounSlab::new();
    let first = T(&mut slab, &[D(1), D(2)]);
    let other = T(&mut slab, &[D(1), D(3)]);
    let first_again = T(&mut slab, &[D(1), D(2)]);
    // Forge one mug for all three leaves, so every node lands in one hash bucket
    // and only the exact leaf comparison can tell them apart.
    let mug = NounMug(0x5eed);
    let mut table = TypeTable::new();
    let inner = table.intern_shallow(Type::Noun);
    let makers: [fn(Leaf, TypeRef<Type>) -> Type; 3] = [
        |leaf, _| Type::Atom {
            aura: leaf,
            bits: Leaf::Direct(0),
        },
        |leaf, inner| Type::Face { tool: leaf, inner },
        |leaf, inner| Type::Hint {
            head: leaf,
            payload: inner,
        },
    ];
    for make in makers {
        let calls = table.interned_calls;
        let distinct = table.distinct;
        let a = table.intern_shallow(make(Leaf::Noun(first, mug), inner));
        let b = table.intern_shallow(make(Leaf::Noun(other, mug), inner));
        let c = table.intern_shallow(make(Leaf::Noun(first_again, mug), inner));
        assert!(
            !TypeRef::ptr_eq(&a, &b),
            "equal mugs, unequal leaves stay distinct"
        );
        assert!(TypeRef::ptr_eq(&a, &c), "structurally equal leaves dedup");
        assert_eq!(table.interned_calls, calls + 3);
        assert_eq!(table.distinct, distinct + 2);
    }
}

#[test]
#[should_panic(expected = "native shadow mismatch")]
fn assert_native_eq_panics_on_a_mismatched_shadow() {
    let mut cx = Context::new();
    let noun = cons_noun(&mut cx);
    assert_native_eq(D(tas("void")), &noun, &NounSpace::empty());
}

#[test]
fn native_of_mug_insert_is_idempotent_per_handle() {
    let mut cx = Context::new();
    let noun = cons_noun(&mut cx);
    let mug = NounMug(42);
    native_of_mug_insert(&mut cx, mug, noun);
    native_of_mug_insert(&mut cx, mug, noun);
    assert_eq!(native_of_mug_candidates(&cx, mug).len(), 1);
}

// ---- leaves (leaf.rs) ---------------------------------------------------------

#[test]
fn leaves_jam_wide_atoms_and_compare_jammed_bytes_exactly() {
    let mut slab: NounSlab = NounSlab::new();
    let wide = big_atom(&mut slab, &big(70));
    let leaf = Leaf::from_noun(wide, &slab.noun_space());
    let Leaf::Jammed(bytes, hash) = &leaf else {
        panic!("an atom above u64 is jammed");
    };
    let mut out: NounSlab = NounSlab::new();
    let back = leaf.to_noun(&mut out);
    assert_eq!(
        jam_of(back, &out.noun_space()),
        jam_of(wide, &slab.noun_space())
    );

    // Same allocation, same bytes in a new allocation, a different hash, and a
    // forged equal hash over different bytes.
    let same_alloc = Leaf::Jammed(Arc::clone(bytes), *hash);
    let copied: Arc<[u8]> = Arc::from(&bytes[..]);
    let same_bytes = Leaf::Jammed(copied, *hash);
    let other_hash = Leaf::Jammed(Arc::clone(bytes), JamHash(hash.0 ^ 1));
    let mut altered = bytes.to_vec();
    altered[0] ^= 0xff;
    let forged = Leaf::Jammed(Arc::from(altered.as_slice()), *hash);
    assert_eq!(leaf, same_alloc);
    assert_eq!(leaf, same_bytes);
    assert_ne!(leaf, other_hash);
    assert_ne!(leaf, forged);

    // Cross-variant leaves are never equal.
    let raw = Leaf::from_noun_raw(wide, &slab.noun_space());
    assert!(matches!(raw, Leaf::Noun(..)));
    assert_ne!(leaf, raw);
    assert_ne!(Leaf::Direct(1), raw);

    // Equal jammed leaves hash equally.
    let hash_of = |leaf: &Leaf| {
        let mut hasher = DefaultHasher::new();
        leaf.hash(&mut hasher);
        hasher.finish()
    };
    assert_eq!(hash_of(&leaf), hash_of(&same_bytes));
    assert_ne!(hash_of(&leaf), hash_of(&other_hash));
}

// ---- value arena (value_dag.rs) -----------------------------------------------

#[test]
fn value_arena_separates_colliding_atoms_and_reuses_registered_cells() {
    let mut slab: NounSlab = NounSlab::new();
    let (first, second) = colliding_big_atoms(&mut slab);
    let pair = T(&mut slab, &[D(1), D(2)]);
    let space = slab.noun_space();
    assert_eq!(slab_mug(first, &space), slab_mug(second, &space));

    let mut arena = ValueArena::new();
    let first_id = arena.import(first, &space).expect("import");
    let second_id = arena.import(second, &space).expect("import");
    assert_ne!(
        first_id, second_id,
        "a mug collision is resolved by comparing bytes"
    );
    assert!(arena.children(first_id).is_none(), "atoms have no children");

    let head = arena.import(D(1), &space).expect("head");
    let tail = arena.import(D(2), &space).expect("tail");
    let cell = arena.intern_cell_with_noun(pair, head, tail);
    let again = arena.intern_cell_with_noun(pair, head, tail);
    assert_eq!(cell, again, "a registered cell noun returns its id");
    assert_eq!(arena.children(cell), Some((head, tail)));
}

// ---- formula DAG (formula_dag.rs) ---------------------------------------------

/// Materialize into the arena's one output slab: an arena memoizes each node's
/// noun, so every materialization of one arena must target the same slab.
fn dag_show(arena: &mut FormulaArena, id: FormulaId, slab: &mut NounSlab) -> String {
    let noun = arena.materialize(id, slab);
    show(noun, &slab.noun_space())
}

#[test]
fn dag_import_decodes_every_opcode_to_canonical_nodes() {
    let mut slab: NounSlab = NounSlab::new();
    let s1 = T(&mut slab, &[D(0), D(1)]);
    let s2 = T(&mut slab, &[D(0), D(2)]);
    let s3 = T(&mut slab, &[D(0), D(3)]);
    let q0 = T(&mut slab, &[D(1), D(0)]);
    let q1 = T(&mut slab, &[D(1), D(1)]);
    let wide = big_atom(&mut slab, &big(80));
    let edit_arg = T(&mut slab, &[D(6), q0]);
    let hint_clue = D(tas("foo"));
    let nouns: Vec<(&str, Noun)> = vec![
        ("slot", T(&mut slab, &[D(0), D(5)])),
        ("big slot", T(&mut slab, &[D(0), wide])),
        ("quote", T(&mut slab, &[D(1), D(42)])),
        ("eval", T(&mut slab, &[D(2), s2, s3])),
        ("cell op", T(&mut slab, &[D(3), s1])),
        ("increment", T(&mut slab, &[D(4), s1])),
        ("equal", T(&mut slab, &[D(5), s2, s3])),
        ("cond", T(&mut slab, &[D(6), s2, q0, q1])),
        ("compose", T(&mut slab, &[D(7), s2, s3])),
        ("push", T(&mut slab, &[D(8), q0, s2])),
        ("kick", T(&mut slab, &[D(9), D(2), s1])),
        ("edit", T(&mut slab, &[D(10), edit_arg, s1])),
        ("hint", T(&mut slab, &[D(11), hint_clue, s1])),
        ("scry", T(&mut slab, &[D(12), s2, s3])),
        ("op 13", T(&mut slab, &[D(13), s2, s3])),
        ("autocons", T(&mut slab, &[s2, s3])),
    ];
    let space = slab.noun_space();
    let mut arena = FormulaArena::new();
    // Build the expected canonical nodes first, unmaterialized, so importing
    // their noun forms both hits them and backfills their materialization.
    let e1 = arena.slot_u64(1);
    let e2 = arena.slot_u64(2);
    let e3 = arena.slot_u64(3);
    let e_q0 = arena.quote(D(0), &space);
    let e_q1 = arena.quote(D(1), &space);
    let e_wide = arena.slot(big(80));
    let expected: Vec<FormulaId> = vec![
        arena.slot_u64(5),
        e_wide,
        arena.quote(D(42), &space),
        arena.eval(e2, e3),
        arena.op(NockOpcode::CELL, &[e1]),
        arena.op(NockOpcode::INCREMENT, &[e1]),
        arena.op(NockOpcode::EQUAL, &[e2, e3]),
        arena.conditional(e2, e_q0, e_q1),
        arena.op(NockOpcode::COMPOSE, &[e2, e3]),
        arena.op(NockOpcode::PUSH, &[e_q0, e2]),
        arena.kick(BigUint::from(2u8), e1),
        arena.edit(BigUint::from(6u8), e_q0, e1),
        arena.hint(hint_clue, e1, &space),
        arena.op(NockOpcode::SCRY, &[e2, e3]),
        arena.op(NockOpcode(13), &[e2, e3]),
        arena.cell(e2, e3),
    ];
    let before = arena.distinct();
    for ((label, noun), want) in nouns.iter().zip(expected) {
        let got = arena.import(*noun, &space).expect("import");
        assert_eq!(got, want, "{label} imports to the canonical node");
        // The imported noun now backs the node's materialization.
        let emitted = arena.materialize(got, &mut slab);
        assert!(
            unsafe { emitted.raw_equals(noun) },
            "{label} reuses its source noun"
        );
    }
    assert_eq!(arena.distinct(), before, "import created no new nodes");
    // `is_slot` widens a big axis to compare it with a small one.
    assert!(!arena.is_crash(e_wide));
    assert!(!arena.is_slot(e_wide, 5));

    // Importing the same noun again is a raw-identity memo hit.
    let imports = arena.imports;
    let again = arena.import(nouns[3].1, &space).expect("import");
    assert_eq!(arena.imports, imports);
    let eval = arena.eval(e2, e3);
    assert_eq!(again, eval);
}

#[test]
fn dag_import_keeps_noncanonical_formulas_opaque() {
    let mut slab: NounSlab = NounSlab::new();
    let s1 = T(&mut slab, &[D(0), D(1)]);
    let wide = big_atom(&mut slab, &big(70));
    let pair = T(&mut slab, &[D(1), D(2)]);
    let raw: Vec<Noun> = vec![
        D(7),                         // an atom is not a formula
        T(&mut slab, &[D(99), D(5)]), // unknown opcode
        T(&mut slab, &[D(0), pair]),  // axis is a cell
        T(&mut slab, &[D(2), D(5)]),  // malformed %2 arguments
        T(&mut slab, &[wide, s1]),    // opcode is not a small atom
    ];
    let space = slab.noun_space();
    let mut arena = FormulaArena::new();
    for noun in raw {
        let id = arena.import(noun, &space).expect("hand formulas import");
        let emitted = arena.materialize(id, &mut slab);
        assert!(
            unsafe { emitted.raw_equals(&noun) },
            "opaque formula re-emits exactly"
        );
        assert!(!arena.is_crash(id));
    }
}

#[test]
fn dag_materializes_constructed_nodes_in_nock_shape() {
    let mut slab: NounSlab = NounSlab::new();
    let wide = big_atom(&mut slab, &big(70));
    let space = slab.noun_space();
    let mut arena = FormulaArena::new();
    let s2 = arena.slot_u64(2);
    let s3 = arena.slot_u64(3);
    let q7 = arena.quote(D(7), &space);
    let eval = arena.eval(s2, s3);
    assert_eq!(dag_show(&mut arena, eval, &mut slab), "[2 [0 2] 0 3]");
    let kick = arena.kick(big(70), s3);
    assert_eq!(
        dag_show(&mut arena, kick, &mut slab),
        format!("[9 {} 0 3]", show(wide, &space))
    );
    let edit = arena.edit(BigUint::from(6u8), q7, eval);
    assert_eq!(
        dag_show(&mut arena, edit, &mut slab),
        "[10 [6 1 7] 2 [0 2] 0 3]"
    );
    let hint = arena.hint(D(9), edit, &space);
    assert_eq!(
        dag_show(&mut arena, hint, &mut slab),
        "[11 9 10 [6 1 7] 2 [0 2] 0 3]"
    );
    let inc = arena.op(NockOpcode::INCREMENT, &[hint]);
    assert_eq!(
        dag_show(&mut arena, inc, &mut slab),
        "[4 11 9 10 [6 1 7] 2 [0 2] 0 3]"
    );
    assert!(arena.materializations > 0);
}

#[test]
#[should_panic(expected = "arity one or two")]
fn dag_materialize_rejects_a_nullary_opcode_node() {
    let mut arena = FormulaArena::new();
    let bogus = arena.op(NockOpcode::CELL, &[]);
    let mut slab: NounSlab = NounSlab::new();
    arena.materialize(bogus, &mut slab);
}

/// The DAG `comb` agrees with the noun `comb` and with an expected shape.
fn check_dag_comb(
    build: impl FnOnce(&mut FormulaArena, &NounSpace) -> (FormulaId, FormulaId),
    expect: &str,
) {
    let mut slab: NounSlab = NounSlab::new();
    let space = slab.noun_space();
    let mut arena = FormulaArena::new();
    let (mal, buz) = build(&mut arena, &space);
    let mal_noun = arena.materialize(mal, &mut slab);
    let buz_noun = arena.materialize(buz, &mut slab);
    let noun_result = crate::native::formula::comb(&mut slab, mal_noun, buz_noun).expect("comb");
    assert_eq!(show(noun_result, &slab.noun_space()), expect, "noun comb");
    let combined = arena.comb(mal, buz);
    assert_eq!(
        dag_show(&mut arena, combined, &mut slab),
        expect,
        "dag comb"
    );
}

#[test]
fn dag_comb_covers_every_peephole_and_fallthrough() {
    // rule 1b: [0 a] o [2 [0 x] [0 y]] -> [2 [0 a.x] [0 a.y]]
    check_dag_comb(
        |a, _| {
            let mal = a.slot_u64(6);
            let (x, y) = (a.slot_u64(2), a.slot_u64(3));
            (mal, a.eval(x, y))
        },
        "[2 [0 12] 0 13]",
    );
    // rule 1b needs slot operands, and both must be nonzero.
    check_dag_comb(
        |a, s| {
            let mal = a.slot_u64(6);
            let (x, y) = (a.quote(D(5), s), a.slot_u64(3));
            (mal, a.eval(x, y))
        },
        "[7 [0 6] 2 [1 5] 0 3]",
    );
    check_dag_comb(
        |a, _| {
            let mal = a.slot_u64(6);
            let (x, y) = (a.slot_u64(0), a.slot_u64(3));
            (mal, a.eval(x, y))
        },
        "[7 [0 6] 2 [0 0] 0 3]",
    );
    check_dag_comb(
        |a, _| {
            let mal = a.slot_u64(6);
            let (x, y) = (a.slot_u64(2), a.slot_u64(0));
            (mal, a.eval(x, y))
        },
        "[7 [0 6] 2 [0 2] 0 0]",
    );
    // A crash on either side blocks rule 1.
    check_dag_comb(|a, _| (a.slot_u64(0), a.slot_u64(3)), "[7 [0 0] 0 3]");
    check_dag_comb(|a, _| (a.slot_u64(6), a.slot_u64(0)), "[7 [0 6] 0 0]");
    // Big axes: is_zero/peg on Axis::Big, and a big buz is never [0 1].
    let mut slab: NounSlab = NounSlab::new();
    let pegged = big_atom(&mut slab, &(big(71) + BigUint::from(1u8)));
    let wide = big_atom(&mut slab, &big(70));
    let space = slab.noun_space();
    check_dag_comb(
        |a, _| (a.slot(big(70)), a.slot_u64(3)),
        &format!("[0 {}]", show(pegged, &space)),
    );
    check_dag_comb(
        |a, s| (a.quote(D(1), s), a.slot(big(70))),
        &format!("[7 [1 1] 0 {}]", show(wide, &space)),
    );
}

#[test]
fn dag_boolean_constructors_fold_like_hoon_138() {
    let mut slab: NounSlab = NounSlab::new();
    let space = slab.noun_space();
    let mut a = FormulaArena::new();
    let yes = a.quote(D(0), &space);
    let no = a.quote(D(1), &space);
    let crash = a.slot_u64(0);
    let s2 = a.slot_u64(2);
    let x = a.op(NockOpcode::CELL, &[s2]);
    let s3 = a.slot_u64(3);
    let y = a.op(NockOpcode::CELL, &[s3]);

    // ++flip
    assert_eq!(a.flip(yes), no);
    assert_eq!(a.flip(no), yes);
    assert_eq!(a.flip(crash), crash);
    let flipped = a.flip(x);
    assert_eq!(
        dag_show(&mut a, flipped, &mut slab),
        "[6 [3 0 2] [1 1] 1 0]"
    );

    // ++flan: bos when bos=nif, bos=|, nif=&, bos=[0 0]; nif when bos=&,
    // nif=|, nif=[0 0]; else [6 bos nif [1 |]].
    assert_eq!(a.flan(x, x), x);
    assert_eq!(a.flan(no, x), no);
    assert_eq!(a.flan(x, yes), x);
    assert_eq!(a.flan(crash, x), crash);
    assert_eq!(a.flan(yes, x), x);
    assert_eq!(a.flan(x, no), no);
    assert_eq!(a.flan(x, crash), crash);
    let both = a.flan(x, y);
    assert_eq!(dag_show(&mut a, both, &mut slab), "[6 [3 0 2] [3 0 3] 1 1]");

    // ++flor: bos when bos=nif, bos=&, nif=|, bos=[0 0]; nif when bos=|,
    // nif=&, nif=[0 0]; else [6 bos [1 &] nif].
    assert_eq!(a.flor(x, x), x);
    assert_eq!(a.flor(yes, x), yes);
    assert_eq!(a.flor(x, no), x);
    assert_eq!(a.flor(crash, x), crash);
    assert_eq!(a.flor(no, x), x);
    assert_eq!(a.flor(x, yes), yes);
    assert_eq!(a.flor(x, crash), crash);
    let either = a.flor(x, y);
    assert_eq!(
        dag_show(&mut a, either, &mut slab),
        "[6 [3 0 2] [1 0] 3 0 3]"
    );

    // ++cond folds constant and crash tests; `and` is cond(l, r, |).
    assert_eq!(a.cond(yes, x, y), x);
    assert_eq!(a.cond(no, x, y), y);
    assert_eq!(a.cond(crash, x, y), crash);
    let anded = a.and(x, y);
    assert_eq!(
        dag_show(&mut a, anded, &mut slab),
        "[6 [3 0 2] [3 0 3] 1 1]"
    );
    assert!(a.equal(anded, both));
}

#[test]
fn dag_cove_reads_through_hints_and_rejects_non_slots() {
    let slab: NounSlab = NounSlab::new();
    let space = slab.noun_space();
    let mut arena = FormulaArena::new();
    let wide = arena.slot(big(70));
    let hinted = arena.hint(D(tas("spot")), wide, &space);
    let twice = arena.hint(D(1), hinted, &space);
    assert_eq!(arena.cove(twice).expect("hinted slot"), big(70));
    let quoted = arena.quote(D(3), &space);
    let err = arena.cove(quoted).expect_err("not a slot");
    assert!(format!("{err:?}").contains("cove"));
}

#[test]
fn dag_separates_quotes_of_colliding_atoms() {
    let mut slab: NounSlab = NounSlab::new();
    let (first, second) = colliding_big_atoms(&mut slab);
    let space = slab.noun_space();
    let mut arena = FormulaArena::new();
    let a = arena.quote(first, &space);
    let hits = arena.hits;
    let b = arena.quote(second, &space);
    assert_ne!(a, b, "equal mugs share a bucket but not a node");
    assert_eq!(arena.hits, hits);
    assert_eq!(arena.quote(first, &space), a);
}

// ---- source-level behavior (dbug off, so peepholes see bare formulas) ------

#[test]
fn source_tisgar_over_nock_two_composes_axes() {
    let (_ty, formula) = mint_source("=/  c  `[p=* q=*]`[0 0]\n=>(c .*(p q))").expect("mint");
    assert!(
        formula.ends_with("2 [0 4] 0 5]"),
        "rule 1b pegs both axes: {formula}"
    );
}

#[test]
fn source_fish_folds_void_holds_and_fork_members() {
    let src = "=>  |%  ++  vd  !!  --\n=/  c  `*`0\n=/  b  `?`&\n";
    // [_vd *]: the hold head fishes to [1 1], so the whole test folds to |.
    let (_ty, formula) = mint_source(&format!("{src}?=([_vd *] c)")).expect("mint");
    assert!(
        formula.ends_with("1 1]"),
        "void head folds the test: {formula}"
    );
    let (_ty, formula) = mint_source(&format!("{src}?=([* _vd] c)")).expect("mint");
    assert!(
        formula.ends_with("1 1]"),
        "void tail folds the test: {formula}"
    );
    // A fork holding %noun always matches.
    let (_ty, formula) = mint_source(&format!("{src}?=(_?:(b ^-(* 0) ^-(@ 0)) c)")).expect("mint");
    assert!(
        formula.ends_with("1 0]"),
        "a %noun member folds the test: {formula}"
    );
}

#[test]
fn source_constants_with_colliding_mugs_stay_distinct() {
    // 0x1.0000.0000.0000.3717 and 0x1.0000.0000.0000.d5d0 share a 31-bit mug,
    // as do the terms %mugclashwlza and %mugclasheydf (probe c5_mug_collision).
    let pair = "[1 0x10000000000003717 0x1000000000000d5d0]";
    let (_ty, formula) =
        mint_source("[0x1.0000.0000.0000.3717 0x1.0000.0000.0000.d5d0]").expect("mint");
    assert_eq!(formula, pair, "both quoted constants survive hash-consing");
    let (_ty, formula) =
        mint_source("^~([0x1.0000.0000.0000.3717 0x1.0000.0000.0000.d5d0])").expect("mint");
    assert_eq!(formula, pair, "the folded value keeps both atoms");
    let (ty, _formula) = mint_source("[%mugclashwlza %mugclasheydf]").expect("mint");
    assert!(
        ty.contains("0x617a6c776873616c6367756d") && ty.contains("0x666479656873616c6367756d"),
        "both constant atom types survive interning: {ty}"
    );
    let (ty, _formula) = mint_source("[mugclashwlza=1 mugclasheydf=1]").expect("mint");
    assert!(
        ty.contains("0x617a6c776873616c6367756d") && ty.contains("0x666479656873616c6367756d"),
        "both face tools survive interning: {ty}"
    );
}

#[test]
fn source_fits_on_an_arm_in_a_mulled_wet_gate_is_rejected_by_cove() {
    // hoonc rejects this too (`cove`): mull's %fits needs the wing's formula to
    // be a slot, and an arm reference compiles to a %9 kick.
    let src = "=>  |%  ++  foo  5  ++  wet  |*  a=*  ?=(@ foo)  --\n(wet 1)";
    let err = mint_source(src).expect_err("cove rejects the arm wing");
    assert!(format!("{err:?}").contains("cove"), "{err:?}");
}
