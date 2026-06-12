use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::rc::Rc as NRc;

use hatch::ast::hoon::{BaseType, Hoon, NounExpr, ParsedAtom, Pint, Skin, Spec, Spot, Tome};
use nockapp::noun::slab::NounSlab;
use nockvm::ext::AtomExt;
use nockvm::noun::{Atom, Noun, NounAllocator, D, T};

use super::{
    cell_type, coil_from_parts, coil_parts, find_face_axis_skip, hoon_to_noun,
    is_const_bool_formula, map_to_noun, noun_eq, term_to_noun, ty_atom, ty_cell, ty_core, ty_face,
    ty_face_tool, ty_fork, ty_hint, ty_hold, ty_noun, ty_void, type_core_parts,
    type_face_name_if_atom, type_face_tool, type_tag, CompilerError, Limb, NestPairSet,
    NestSeenSet, NestTypeInterner, Opal, Palo, Poly, Port, StructNounPairSet, StructNounSet, Ut,
    Way,
};
use super::{native_of, NTy};
use crate::native::ut::wet::RedoState;

#[test]
fn location_from_spot_extracts_line_and_col() {
    let spot = Spot {
        p: vec!["foo".to_string(), "bar.hoon".to_string()],
        q: Pint {
            p: (12, 34),
            q: (12, 56),
        },
    };
    let location = Ut::location_from_spot(&spot);
    assert_eq!(location.file.as_deref(), Some("foo/bar.hoon"));
    assert_eq!(location.start_line, Some(12));
    assert_eq!(location.start_col, Some(34));
    assert_eq!(location.end_line, Some(12));
    assert_eq!(location.end_col, Some(56));
}

#[test]
fn struct_noun_set_insert_contains_remove_is_structural() {
    let mut slab = NounSlab::new();
    let noun_a = {
        let head = ty_atom(&mut slab, "ud", None);
        let tail = ty_noun(&mut slab);
        ty_cell(&mut slab, head, tail)
    };
    let noun_b = {
        let head = ty_atom(&mut slab, "ud", None);
        let tail = ty_noun(&mut slab);
        ty_cell(&mut slab, head, tail)
    };
    assert!(noun_eq(noun_a, noun_b, &slab.noun_space()).expect("noun_eq"));
    assert!(!unsafe { noun_a.raw_equals(&noun_b) });

    let mut ut = Ut::new(&mut slab);
    let mut set = StructNounSet::new();
    assert!(set.insert(&mut ut, noun_a).expect("insert"));
    assert!(set.contains(&mut ut, noun_b).expect("contains"));
    assert!(!set.insert(&mut ut, noun_b).expect("dedupe"));
    assert!(set.remove(&mut ut, noun_b).expect("remove"));
    assert!(!set
        .contains(&mut ut, noun_a)
        .expect("contains after remove"));
    assert!(
        set.buckets.is_empty(),
        "bucket should be dropped when empty"
    );
}

#[test]
fn struct_noun_set_remove_missing_returns_false() {
    let mut slab = NounSlab::new();
    let noun = ty_atom(&mut slab, "ud", None);
    let missing = ty_atom(&mut slab, "ux", None);
    let mut ut = Ut::new(&mut slab);
    let mut set = StructNounSet::new();
    assert!(set.insert(&mut ut, noun).expect("insert"));
    assert!(!set.remove(&mut ut, missing).expect("missing remove"));
}

#[test]
fn struct_noun_pair_set_is_structural() {
    let mut slab = NounSlab::new();
    let sut_a = {
        let head = ty_atom(&mut slab, "ud", None);
        let tail = ty_noun(&mut slab);
        ty_cell(&mut slab, head, tail)
    };
    let sut_b = {
        let head = ty_atom(&mut slab, "ud", None);
        let tail = ty_noun(&mut slab);
        ty_cell(&mut slab, head, tail)
    };
    let ref_a = {
        let head = ty_atom(&mut slab, "ux", None);
        let tail = ty_noun(&mut slab);
        ty_cell(&mut slab, head, tail)
    };
    let ref_b = {
        let head = ty_atom(&mut slab, "ux", None);
        let tail = ty_noun(&mut slab);
        ty_cell(&mut slab, head, tail)
    };
    let mut ut = Ut::new(&mut slab);
    let mut set = StructNounPairSet::new();
    assert!(set.insert(&mut ut, sut_a, ref_a).expect("insert"));
    assert!(!set.insert(&mut ut, sut_b, ref_b).expect("dedupe"));
    assert!(set.remove(&mut ut, sut_b, ref_b).expect("remove"));
    assert!(
        set.buckets.is_empty(),
        "pair bucket should be dropped when empty"
    );
    assert!(set.is_empty(), "pair set should report empty after removal");
}

#[test]
fn struct_noun_pair_set_signature_is_order_independent() {
    let mut slab = NounSlab::new();

    let mk_pair = |slab: &mut NounSlab, left: u64, right: u64| {
        let sut_head = ty_atom(slab, "@", Some(D(left)));
        let sut_tail = ty_noun(slab);
        let sut = ty_cell(slab, sut_head, sut_tail);
        let ref_head = ty_atom(slab, "@", Some(D(right)));
        let ref_tail = ty_noun(slab);
        let ref_ = ty_cell(slab, ref_head, ref_tail);
        (sut, ref_)
    };

    let (sut_a, ref_a) = mk_pair(&mut slab, 1, 2);
    let (sut_b, ref_b) = mk_pair(&mut slab, 3, 4);
    let (sut_a_dup, ref_a_dup) = mk_pair(&mut slab, 1, 2);
    let (sut_b_dup, ref_b_dup) = mk_pair(&mut slab, 3, 4);
    let mut ut = Ut::new(&mut slab);

    let mut left = StructNounPairSet::new();
    let mut right = StructNounPairSet::new();
    assert!(left.insert(&mut ut, sut_a, ref_a).expect("left insert a"));
    assert!(left.insert(&mut ut, sut_b, ref_b).expect("left insert b"));
    assert!(right
        .insert(&mut ut, sut_b_dup, ref_b_dup)
        .expect("right insert b"));
    assert!(right
        .insert(&mut ut, sut_a_dup, ref_a_dup)
        .expect("right insert a"));

    assert_eq!(left.pair_count, 2);
    assert_eq!(right.pair_count, 2);
    assert_eq!(left.cache_signature(), right.cache_signature());
    let left_snapshot = left.snapshot();
    let space = ut.slab.noun_space();
    assert!(right
        .matches_snapshot(&left_snapshot, &space)
        .expect("snapshot match"));
}

#[test]
fn type_tag_kind_matches_core_type_tags_without_string_dispatch() {
    let mut slab = NounSlab::new();
    let hoon_noun = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let cell_head = ty_noun(&mut slab);
    let cell_tail = ty_noun(&mut slab);
    let cell = ty_cell(&mut slab, cell_head, cell_tail);
    let face_inner = ty_noun(&mut slab);
    let face = ty_face(&mut slab, "foo", face_inner);
    let fork_left = ty_noun(&mut slab);
    let fork_right = ty_void(&mut slab);
    let fork = ty_fork(&mut slab, vec![fork_left, fork_right]);
    let hint_inner = ty_noun(&mut slab);
    let hint_payload = ty_atom(&mut slab, "ud", None);
    let hint = ty_hint(&mut slab, hint_inner, D(0), hint_payload);
    let hold_inner = ty_noun(&mut slab);
    let hold = ty_hold(&mut slab, hold_inner, hoon_noun);
    let void = ty_void(&mut slab);
    let noun = ty_noun(&mut slab);
    let atom = ty_atom(&mut slab, "ud", None);
    let space = slab.noun_space();

    assert_eq!(
        super::type_tag_kind(void, &space).expect("void"),
        super::TypeTagKind::Void
    );
    assert_eq!(
        super::type_tag_kind(noun, &space).expect("noun"),
        super::TypeTagKind::Noun
    );
    assert_eq!(
        super::type_tag_kind(atom, &space).expect("atom"),
        super::TypeTagKind::Atom
    );
    assert_eq!(
        super::type_tag_kind(cell, &space).expect("cell"),
        super::TypeTagKind::Cell
    );
    assert_eq!(
        super::type_tag_kind(face, &space).expect("face"),
        super::TypeTagKind::Face
    );
    assert_eq!(
        super::type_tag_kind(fork, &space).expect("fork"),
        super::TypeTagKind::Fork
    );
    assert_eq!(
        super::type_tag_kind(hint, &space).expect("hint"),
        super::TypeTagKind::Hint
    );
    assert_eq!(
        super::type_tag_kind(hold, &space).expect("hold"),
        super::TypeTagKind::Hold
    );
}

#[test]
fn nest_seen_set_is_structural_via_interner() {
    let mut slab = NounSlab::new();
    let noun_a = {
        let head = ty_atom(&mut slab, "ud", None);
        let tail = ty_noun(&mut slab);
        ty_cell(&mut slab, head, tail)
    };
    let noun_b = {
        let head = ty_atom(&mut slab, "ud", None);
        let tail = ty_noun(&mut slab);
        ty_cell(&mut slab, head, tail)
    };
    assert!(noun_eq(noun_a, noun_b, &slab.noun_space()).expect("noun_eq"));
    assert!(!unsafe { noun_a.raw_equals(&noun_b) });

    let mut ut = Ut::new(&mut slab);
    let mut interner = NestTypeInterner::new();
    let mut set = NestSeenSet::new();
    assert!(set.insert(&mut ut, &mut interner, noun_a).expect("insert"));
    assert!(set
        .contains(&mut ut, &mut interner, noun_b)
        .expect("contains"));
    assert!(!set.insert(&mut ut, &mut interner, noun_b).expect("dedupe"));
    assert!(set.remove(&mut ut, &mut interner, noun_b).expect("remove"));
}

#[test]
fn nest_pair_set_is_structural_via_interner() {
    let mut slab = NounSlab::new();
    let sut_a = {
        let head = ty_atom(&mut slab, "ud", None);
        let tail = ty_noun(&mut slab);
        ty_cell(&mut slab, head, tail)
    };
    let sut_b = {
        let head = ty_atom(&mut slab, "ud", None);
        let tail = ty_noun(&mut slab);
        ty_cell(&mut slab, head, tail)
    };
    let ref_a = {
        let head = ty_atom(&mut slab, "ux", None);
        let tail = ty_noun(&mut slab);
        ty_cell(&mut slab, head, tail)
    };
    let ref_b = {
        let head = ty_atom(&mut slab, "ux", None);
        let tail = ty_noun(&mut slab);
        ty_cell(&mut slab, head, tail)
    };
    let mut ut = Ut::new(&mut slab);
    let mut interner = NestTypeInterner::new();
    let mut set = NestPairSet::new();
    assert!(set
        .insert(&mut ut, &mut interner, sut_a, ref_a)
        .expect("insert"));
    assert!(!set
        .insert(&mut ut, &mut interner, sut_b, ref_b)
        .expect("dedupe"));
    assert!(set
        .remove(&mut ut, &mut interner, sut_b, ref_b)
        .expect("remove"));
}

#[test]
fn nest_cell_branch_resets_hold_seen_guards() {
    let mut slab = NounSlab::new();
    let hold_inner = ty_atom(&mut slab, "ud", None);
    let hoon_noun = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let hold = ty_hold(&mut slab, hold_inner, hoon_noun);

    // List-like tail shape: [@ hold(@)] against [@ @].
    let sut_head = ty_atom(&mut slab, "ud", None);
    let sut = ty_cell(&mut slab, sut_head, hold);
    let ref_head = ty_atom(&mut slab, "ud", None);
    let ref_tail = ty_atom(&mut slab, "ud", None);
    let ref_ = ty_cell(&mut slab, ref_head, ref_tail);

    let mut ut = Ut::new(&mut slab);
    let space = ut.slab.noun_space();
    let sut_n = native_of(&mut ut.cx, sut, &space).expect("native sut");
    let ref_n = native_of(&mut ut.cx, ref_, &space).expect("native ref");
    let hold_n = native_of(&mut ut.cx, hold, &space).expect("native hold");
    let mut seen_sut_holds = NestSeenSet::new();
    let mut seen_ref_holds = NestSeenSet::new();
    let mut gil = NestPairSet::new();
    let mut memo = Default::default();

    assert!(
        seen_sut_holds.insert_id(Rc::<NTy>::as_ptr(&hold_n) as u64),
        "seed insert should succeed"
    );

    let ok = ut
        .nest_inner(
            sut_n,
            ref_n,
            0,
            &mut seen_sut_holds,
            &mut seen_ref_holds,
            &mut gil,
            &mut memo,
        )
        .expect("nest cell");
    assert!(
        ok,
        "cell branch should use fresh seg/reg so parent hold guard does not leak into child checks"
    );
}

#[test]
fn nest_hold_seen_guard_still_applies_outside_cell() {
    let mut slab = NounSlab::new();
    let hold_inner = ty_atom(&mut slab, "ud", None);
    let hoon_noun = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let hold = ty_hold(&mut slab, hold_inner, hoon_noun);
    let ref_ = ty_atom(&mut slab, "ud", None);

    let mut ut = Ut::new(&mut slab);
    let space = ut.slab.noun_space();
    let hold_n = native_of(&mut ut.cx, hold, &space).expect("native hold");
    let ref_n = native_of(&mut ut.cx, ref_, &space).expect("native ref");
    let mut seen_sut_holds = NestSeenSet::new();
    let mut seen_ref_holds = NestSeenSet::new();
    let mut gil = NestPairSet::new();
    let mut memo = Default::default();
    assert!(
        seen_sut_holds.insert_id(Rc::<NTy>::as_ptr(&hold_n) as u64),
        "seed insert should succeed"
    );

    let ok = ut
        .nest_inner(
            hold_n.clone(),
            ref_n,
            0,
            &mut seen_sut_holds,
            &mut seen_ref_holds,
            &mut gil,
            &mut memo,
        )
        .expect("nest hold");
    assert!(
        !ok,
        "non-cell %hold checks should still honor the seen_sut_holds guard"
    );
}

#[test]
fn face_axis_search_ignores_non_atom_face_tool() {
    let mut slab = NounSlab::new();
    let face_tag = term_to_noun(&mut slab, "face");
    let non_atom_tool = T(&mut slab, &[D(1), D(2)]);
    let inner = ty_noun(&mut slab);
    let tail = T(&mut slab, &[non_atom_tool, inner]);
    let face_type = T(&mut slab, &[face_tag, tail]);
    let space = slab.noun_space();

    assert_eq!(
        type_face_name_if_atom(face_type, &space).expect("face-name decode should not fail"),
        None
    );
    assert_eq!(
        find_face_axis_skip(face_type, "gall", 0, &space)
            .expect("face-axis search should not fail"),
        None
    );
}

fn core_with_payload_face_and_same_named_arm(slab: &mut NounSlab, name: &str) -> Noun {
    let payload_inner = ty_noun(slab);
    let payload = ty_face(slab, name, payload_inner);
    let context = ty_noun(slab);
    let garb_name = term_to_noun(slab, "test");
    let garb_poly = term_to_noun(slab, "dry");
    let garb_vair = term_to_noun(slab, "gold");
    let garb = T(slab, &[garb_name, garb_poly, garb_vair]);
    let arm_hoon = hoon_to_noun(slab, &Hoon::Axis(1));
    let arm_key = term_to_noun(slab, name);
    let arms = map_to_noun(slab, vec![(arm_key, arm_hoon)]).expect("arms map");
    let tome = T(slab, &[D(0), arms]);
    let tome_key = term_to_noun(slab, "core");
    let tomes = map_to_noun(slab, vec![(tome_key, tome)]).expect("tomes");
    let rest = T(slab, &[D(0), tomes]);
    let coil = coil_from_parts(slab, garb, context, rest);
    ty_core(slab, payload, coil)
}

fn core_with_context_face_only(slab: &mut NounSlab, name: &str) -> Noun {
    let payload = ty_noun(slab);
    let context_inner = ty_noun(slab);
    let context = ty_face(slab, name, context_inner);
    let garb_name = term_to_noun(slab, "test");
    let garb_poly = term_to_noun(slab, "dry");
    let garb_vair = term_to_noun(slab, "gold");
    let garb = T(slab, &[garb_name, garb_poly, garb_vair]);
    let rest = T(slab, &[D(0), D(0)]);
    let coil = coil_from_parts(slab, garb, context, rest);
    ty_core(slab, payload, coil)
}

fn core_with_named_arm_and_vair(slab: &mut NounSlab, name: &str, vair: &str) -> Noun {
    let payload = ty_noun(slab);
    let context = ty_noun(slab);
    let garb_name = term_to_noun(slab, "test");
    let garb_poly = term_to_noun(slab, "dry");
    let garb_vair = term_to_noun(slab, vair);
    let garb = T(slab, &[garb_name, garb_poly, garb_vair]);
    let arm_hoon = hoon_to_noun(slab, &Hoon::Axis(1));
    let arm_key = term_to_noun(slab, name);
    let arms = map_to_noun(slab, vec![(arm_key, arm_hoon)]).expect("arms map");
    let tome = T(slab, &[D(0), arms]);
    let tome_key = term_to_noun(slab, "core");
    let tomes = map_to_noun(slab, vec![(tome_key, tome)]).expect("tomes");
    let rest = T(slab, &[D(0), tomes]);
    let coil = coil_from_parts(slab, garb, context, rest);
    ty_core(slab, payload, coil)
}

fn core_with_constant_payload_and_coil_semi(
    slab: &mut NounSlab,
    payload_value: u64,
    coil_value: u64,
) -> Noun {
    let payload = ty_atom(slab, "@", Some(D(payload_value)));
    let context = ty_noun(slab);
    let garb_name = term_to_noun(slab, "test");
    let garb_poly = term_to_noun(slab, "dry");
    let garb_vair = term_to_noun(slab, "gold");
    let garb = T(slab, &[garb_name, garb_poly, garb_vair]);
    let full = term_to_noun(slab, "full");
    let stencil = T(slab, &[full, D(0)]);
    let semi = T(slab, &[stencil, D(coil_value)]);
    let rest = T(slab, &[semi, D(0)]);
    let coil = coil_from_parts(slab, garb, context, rest);
    ty_core(slab, payload, coil)
}

fn algebra_type_corpus(slab: &mut NounSlab) -> Vec<(&'static str, Noun)> {
    let noun = ty_noun(slab);
    let void = ty_void(slab);
    let atom = ty_atom(slab, "@", None);
    let atom_zero = ty_atom(slab, "@", Some(D(0)));
    let atom_one = ty_atom(slab, "@", Some(D(1)));
    let cell_noun_noun = ty_cell(slab, noun, noun);
    let cell_atom_noun = ty_cell(slab, atom, noun);
    let cell_zero_one = ty_cell(slab, atom_zero, atom_one);
    let face_atom = ty_face(slab, "face", atom);
    let hint_cell = ty_hint(slab, noun, D(0), cell_atom_noun);
    let fork_atom_cell = ty_fork(slab, vec![atom, cell_zero_one]);
    let axis_one_hoon = hoon_to_noun(slab, &Hoon::Axis(1));
    let hold_atom = ty_hold(slab, atom, axis_one_hoon);

    vec![
        ("noun", noun),
        ("void", void),
        ("atom", atom),
        ("atom_zero", atom_zero),
        ("atom_one", atom_one),
        ("cell_noun_noun", cell_noun_noun),
        ("cell_atom_noun", cell_atom_noun),
        ("cell_zero_one", cell_zero_one),
        ("face_atom", face_atom),
        ("hint_cell", hint_cell),
        ("fork_atom_cell", fork_atom_cell),
        ("hold_atom", hold_atom),
    ]
}

fn algebra_binary_type_corpus(slab: &mut NounSlab) -> Vec<(&'static str, Noun)> {
    let noun = ty_noun(slab);
    let void = ty_void(slab);
    let atom = ty_atom(slab, "@", None);
    let atom_zero = ty_atom(slab, "@", Some(D(0)));
    let atom_one = ty_atom(slab, "@", Some(D(1)));
    let cell_noun_noun = ty_cell(slab, noun, noun);
    let cell_atom_noun = ty_cell(slab, atom, noun);
    let cell_zero_one = ty_cell(slab, atom_zero, atom_one);
    let face_atom = ty_face(slab, "face", atom);
    let hint_cell = ty_hint(slab, noun, D(0), cell_atom_noun);
    let fork_atom_cell = ty_fork(slab, vec![atom, cell_zero_one]);

    vec![
        ("noun", noun),
        ("void", void),
        ("atom", atom),
        ("atom_zero", atom_zero),
        ("atom_one", atom_one),
        ("cell_noun_noun", cell_noun_noun),
        ("cell_atom_noun", cell_atom_noun),
        ("cell_zero_one", cell_zero_one),
        ("face_atom", face_atom),
        ("hint_cell", hint_cell),
        ("fork_atom_cell", fork_atom_cell),
    ]
}

#[test]
fn type_algebra_nest_reflexive_top_and_void_laws() {
    let mut slab = NounSlab::new();
    let types = algebra_type_corpus(&mut slab);
    let noun = types
        .iter()
        .find_map(|(name, typ)| (*name == "noun").then_some(*typ))
        .expect("noun in corpus");
    let void = types
        .iter()
        .find_map(|(name, typ)| (*name == "void").then_some(*typ))
        .expect("void in corpus");
    let mut ut = Ut::new(&mut slab);

    for (name, typ) in &types {
        assert!(ut.nest_noun(*typ, *typ).expect("nest reflexive"), "{name}");
        assert!(
            ut.nest_noun(noun, *typ).expect("noun top nest"),
            "any type should fit %noun: {name}"
        );
        assert!(
            ut.nest_noun(*typ, void).expect("void bottom nest"),
            "%void should fit any goal type: {name}"
        );
    }
}

#[test]
fn type_algebra_fuse_and_crop_preserve_subtyping_and_exact_crop_disjointness() {
    let mut slab = NounSlab::new();
    let types = algebra_binary_type_corpus(&mut slab);
    let mut ut = Ut::new(&mut slab);

    for (left_name, left) in &types {
        for (right_name, right) in &types {
            let space = ut.slab.noun_space();
            let left_n = native_of(&mut ut.cx, *left, &space).expect("native left");
            let right_n = native_of(&mut ut.cx, *right, &space).expect("native right");
            let fused = ut
                .fuse(left_n.clone(), right_n.clone())
                .unwrap_or_else(|err| panic!("fuse({left_name}, {right_name}) errored: {err:?}"));
            assert!(
                ut.nest(left_n.clone(), fused.clone())
                    .expect("fused fits left"),
                "fuse({left_name}, {right_name}) should fit left"
            );
            assert!(
                ut.nest(right_n.clone(), fused).expect("fused fits right"),
                "fuse({left_name}, {right_name}) should fit right"
            );

            let cropped = ut
                .crop(left_n.clone(), right_n.clone())
                .unwrap_or_else(|err| panic!("crop({left_name}, {right_name}) errored: {err:?}"));
            assert!(
                ut.nest(left_n, cropped.clone())
                    .expect("cropped fits left"),
                "crop({left_name}, {right_name}) should still fit left"
            );
            // `crop` is conservative for shapes the type system cannot represent exactly
            // (for example, `%noun` minus the literal atom `0`).  Assert disjointness only
            // for exact broad partitions whose complement is representable.
            if matches!(*right_name, "noun" | "void" | "atom" | "cell_noun_noun") {
                assert!(
                    ut.miss(cropped, right_n).expect("crop disjoint"),
                    "crop({left_name}, {right_name}) should not overlap right"
                );
            }
        }
    }
}

#[test]
fn type_algebra_gain_and_lose_partition_base_skins() {
    let mut slab = NounSlab::new();
    let types = algebra_binary_type_corpus(&mut slab);
    let skins = [
        ("noun", Skin::Base(BaseType::NounExpr)),
        ("void", Skin::Base(BaseType::Void)),
        ("atom", Skin::Base(BaseType::Atom("@".to_string()))),
        ("cell", Skin::Base(BaseType::Cell)),
    ];
    let mut ut = Ut::new(&mut slab);
    let sut_noun = ty_noun(ut.slab);
    let sut = native_of(&mut ut.cx, sut_noun, &ut.slab.noun_space()).expect("native sut");

    for (type_name, typ) in &types {
        let typ_n = native_of(&mut ut.cx, *typ, &ut.slab.noun_space()).expect("native typ");
        for (skin_name, skin) in &skins {
            let gained = ut
                .gain_skin(sut.clone(), typ_n.clone(), skin)
                .unwrap_or_else(|err| panic!("gain({type_name}, {skin_name}) errored: {err:?}"));
            let lost = ut
                .lose_skin(sut.clone(), typ_n.clone(), skin)
                .unwrap_or_else(|err| panic!("lose({type_name}, {skin_name}) errored: {err:?}"));
            assert!(
                ut.nest(typ_n.clone(), gained.clone()).expect("gain subtype"),
                "gain({type_name}, {skin_name}) should fit original type"
            );
            assert!(
                ut.nest(typ_n.clone(), lost.clone()).expect("lose subtype"),
                "lose({type_name}, {skin_name}) should fit original type"
            );
            // Base noun/void/atom/cell skins form exact representable partitions.
            if matches!(*skin_name, "noun" | "void" | "atom" | "cell") {
                assert!(
                    ut.miss(gained, lost).expect("gain/lose disjoint"),
                    "gain({type_name}, {skin_name}) and lose({type_name}, {skin_name}) should be disjoint"
                );
            }
        }
    }
}

#[test]
fn type_algebra_peek_distributes_over_forks() {
    let mut slab = NounSlab::new();
    let left_head = ty_atom(&mut slab, "ud", None);
    let left_tail = ty_atom(&mut slab, "ux", None);
    let right_head = ty_atom(&mut slab, "@", Some(D(7)));
    let right_tail = ty_noun(&mut slab);
    let left = ty_cell(&mut slab, left_head, left_tail);
    let right = ty_cell(&mut slab, right_head, right_tail);
    let fork = ty_fork(&mut slab, vec![left, right]);
    let mut ut = Ut::new(&mut slab);

    let head = ut.peek_noun(fork, Way::Free, 2).expect("peek fork head");
    let expected_head = ut
        .fork_from_options(vec![left_head, right_head])
        .expect("expected head fork");
    assert!(noun_eq(head, expected_head, &ut.slab.noun_space()).expect("head noun_eq"));

    let tail = ut.peek_noun(fork, Way::Free, 3).expect("peek fork tail");
    let expected_tail = ut
        .fork_from_options(vec![left_tail, right_tail])
        .expect("expected tail fork");
    assert!(noun_eq(tail, expected_tail, &ut.slab.noun_space()).expect("tail noun_eq"));
}

#[test]
fn active_rest_fan_context_partitions_context_sensitive_native_ut_caches() {
    let mut slab = NounSlab::new();
    let sut = core_with_context_face_only(&mut slab, "tree");
    let gol = ty_noun(&mut slab);
    let ref_type = ty_atom(&mut slab, "@", Some(D(1)));
    let ty = ty_atom(&mut slab, "@", Some(D(2)));
    let formula = T(&mut slab, &[D(1), D(42)]);
    let inner_ty = ty_atom(&mut slab, "@", Some(D(9)));
    let _inner_formula = T(&mut slab, &[D(1), D(99)]);
    let redo_ref_inner = ty_noun(&mut slab);
    let redo_ref = ty_face(&mut slab, "a", redo_ref_inner);
    let redo_out_inner = ty_noun(&mut slab);
    let redo_out = ty_face(&mut slab, "b", redo_out_inner);
    let inner_redo_out_inner = ty_noun(&mut slab);
    let _inner_redo_out = ty_face(&mut slab, "c", inner_redo_out_inner);
    let mint_gen = hoon_to_noun(&mut slab, &Hoon::Axis(11));
    let mull_gen = hoon_to_noun(&mut slab, &Hoon::Axis(12));
    let rest_inner = ty_atom(&mut slab, "@", Some(D(7)));
    let rest_hoon = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let rest_cache_inner = ty_atom(&mut slab, "@", Some(D(8)));
    let rest_cache_hoon = hoon_to_noun(&mut slab, &Hoon::Axis(2));
    let rest_legs = [(rest_cache_inner, rest_cache_hoon)];
    let rest_sut = ty_hold(&mut slab, rest_cache_inner, rest_cache_hoon);

    let mut ut = Ut::new(&mut slab);
    ut.set_vet(false);
    // The mull cache is native-keyed (C8/C-final); native_of the noun sut/gol/ref
    // for the mull_cache_store/lookup calls (the other caches stay noun-keyed).
    let space = ut.slab.noun_space();
    let sut_n = crate::native::ir::intern::native_of(&mut ut.cx, sut, &space).expect("native sut");
    let gol_n = crate::native::ir::intern::native_of(&mut ut.cx, gol, &space).expect("native gol");
    let ref_n = crate::native::ir::intern::native_of(&mut ut.cx, ref_type, &space).expect("native ref");
    let ty_n = crate::native::ir::intern::native_of(&mut ut.cx, ty, &space).expect("native ty");
    let inner_ty_n =
        crate::native::ir::intern::native_of(&mut ut.cx, inner_ty, &space).expect("native inner");
    let rest_legs_noun = ut.rest_legs_noun(&rest_legs);
    ut.mint_boundary_store_exact(sut, gol, mint_gen, ty, formula)
        .expect("mint boundary store");
    ut.mull_cache_store(&sut_n, &gol_n, &ref_n, mull_gen, ty_n, inner_ty_n)
        .expect("mull boundary store");
    ut.redo_boundary_store(sut, redo_ref, redo_out)
        .expect("redo boundary store");
    ut.rest_boundary_store(rest_sut, rest_legs_noun, ty)
        .expect("rest boundary store");
    ut.nest_mug_register(sut, ref_type, true);

    assert!(ut
        .mint_boundary_lookup_exact(sut, gol, mint_gen)
        .expect("mint boundary lookup")
        .is_some());
    assert!(ut
        .mull_cache_lookup(&sut_n, &gol_n, &ref_n, mull_gen)
        .expect("mull boundary lookup")
        .is_some());
    assert!(ut
        .redo_boundary_lookup(sut, redo_ref)
        .expect("redo boundary lookup")
        .is_some());
    assert!(ut
        .rest_boundary_lookup(rest_sut, rest_legs_noun)
        .expect("rest boundary lookup")
        .is_some());
    assert_eq!(
        ut.nest_mug_lookup(sut, ref_type).expect("nest mug lookup"),
        Some(true)
    );
    ut.set_vet(true);
    assert_eq!(
        ut.nest_mug_lookup(sut, ref_type)
            .expect("nest mug lookup with vet enabled"),
        None
    );
    ut.set_vet(false);

    let first_inner_context_key = ut
        .with_rest_leg(rest_inner, rest_hoon, |ut| {
            let inner_context_key = ut.hold_repo_fan_context_key();
            assert_ne!(inner_context_key, 0);
            // Entries were stored with no active fan (fan_context_key == 0).
            // Under an active leg the cache keys carry a different
            // fan_context_key, so every context-sensitive cache MISSES rather
            // than serving the fan-empty result — the partition this test name
            // asserts (and the bug class that miscompiled roswell).
            assert!(ut
                .mint_boundary_lookup_exact(sut, gol, mint_gen)
                .expect("inner mint boundary lookup")
                .is_none());
            assert!(ut
                .mull_cache_lookup(&sut_n, &gol_n, &ref_n, mull_gen)
                .expect("inner mull boundary lookup")
                .is_none());
            assert!(ut
                .redo_boundary_lookup(sut, redo_ref)
                .expect("inner redo boundary lookup")
                .is_none());
            assert!(ut
                .rest_boundary_lookup(rest_sut, rest_legs_noun)
                .expect("inner rest boundary lookup")
                .is_none());
            assert_eq!(
                ut.nest_mug_lookup(sut, ref_type)
                    .expect("inner nest mug lookup"),
                None
            );
            ut.set_vet(true);
            assert_eq!(
                ut.nest_mug_lookup(sut, ref_type)
                    .expect("inner nest mug lookup with vet enabled"),
                None
            );
            ut.set_vet(false);
            Ok(inner_context_key)
        })
        .expect("rest leg should succeed");

    assert_eq!(ut.hold_repo_fan_context_key(), 0);
    assert!(ut
        .mint_boundary_lookup_exact(sut, gol, mint_gen)
        .expect("post mint boundary lookup")
        .is_some());
    assert!(ut
        .mull_cache_lookup(&sut_n, &gol_n, &ref_n, mull_gen)
        .expect("post mull boundary lookup")
        .is_some());
    assert!(ut
        .redo_boundary_lookup(sut, redo_ref)
        .expect("post redo boundary lookup")
        .is_some());
    assert!(ut
        .rest_boundary_lookup(rest_sut, rest_legs_noun)
        .expect("post rest boundary lookup")
        .is_some());
    assert_eq!(
        ut.nest_mug_lookup(sut, ref_type)
            .expect("post nest mug lookup"),
        Some(true)
    );

    ut.with_rest_leg(rest_inner, rest_hoon, |ut| {
        let inner_context_key = ut.hold_repo_fan_context_key();
        assert_eq!(inner_context_key, first_inner_context_key);
        // Same active leg, still fan_context_key != 0; the entries live only
        // under the fan-empty key, so these remain misses.
        assert!(ut
            .mint_boundary_lookup_exact(sut, gol, mint_gen)
            .expect("repeat inner mint boundary lookup")
            .is_none());
        assert!(ut
            .mull_cache_lookup(&sut_n, &gol_n, &ref_n, mull_gen)
            .expect("repeat inner mull boundary lookup")
            .is_none());
        assert!(ut
            .redo_boundary_lookup(sut, redo_ref)
            .expect("repeat inner redo boundary lookup")
            .is_none());
        assert!(ut
            .rest_boundary_lookup(rest_sut, rest_legs_noun)
            .expect("repeat inner rest boundary lookup")
            .is_none());
        assert_eq!(
            ut.nest_mug_lookup(sut, ref_type)
                .expect("repeat inner nest mug lookup"),
            None
        );
        Ok(())
    })
    .expect("rest leg should succeed");
}

#[test]
fn active_rest_fan_context_partitions_rest_boundary() {
    let mut slab = NounSlab::new();
    let inner = core_with_context_face_only(&mut slab, "tree");
    let hoon = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let cached = ty_atom(&mut slab, "@", Some(D(42)));
    let outer_inner = ty_atom(&mut slab, "@", Some(D(7)));
    let outer_hoon = hoon_to_noun(&mut slab, &Hoon::Axis(2));
    let rest_sut = ty_hold(&mut slab, inner, hoon);

    let mut ut = Ut::new(&mut slab);
    let legs = [(inner, hoon)];
    let legs_noun = ut.rest_legs_noun(&legs);
    let first_context = ut
        .with_rest_leg(inner, hoon, |ut| {
            let context_key = ut.hold_repo_fan_context_key();
            assert_ne!(context_key, 0);
            assert!(ut
                .rest_boundary_lookup(rest_sut, legs_noun)
                .expect("rest boundary lookup before store")
                .is_none());

            ut.rest_boundary_store(rest_sut, legs_noun, cached)
                .expect("rest boundary store");

            let rest_cached = ut
                .rest_boundary_lookup(rest_sut, legs_noun)
                .expect("rest boundary lookup after store")
                .expect("rest boundary should be populated");
            assert!(
                noun_eq(rest_cached, cached, &ut.slab.noun_space()).expect("rest boundary noun_eq")
            );
            Ok(context_key)
        })
        .expect("rest leg should succeed");

    // The entry was stored under an active leg (fan_context_key != 0). Outside
    // any fan scope the key carries fan_context_key == 0, so the lookup MISSES
    // — the rest boundary partitions by fan context (it does NOT serve a
    // fan-scoped result back at top level, which would replace a real type
    // with a possibly loop-truncated one).
    assert_eq!(ut.hold_repo_fan_context_key(), 0);
    assert!(ut
        .rest_boundary_lookup(rest_sut, legs_noun)
        .expect("rest boundary lookup outside fan")
        .is_none());

    // Re-entering the SAME leg restores the same fan_context_key, so the entry
    // stored under it hits.
    ut.with_rest_leg(inner, hoon, |ut| {
        let context_key = ut.hold_repo_fan_context_key();
        assert_eq!(context_key, first_context);
        let rest_cached = ut
            .rest_boundary_lookup(rest_sut, legs_noun)
            .expect("rest boundary lookup on re-entry")
            .expect("rest boundary should hit on re-entry under the same leg");
        assert!(
            noun_eq(rest_cached, cached, &ut.slab.noun_space()).expect("rest re-entry noun_eq")
        );
        Ok(())
    })
    .expect("same rest leg should reuse context");

    // A DIFFERENT (nested) leg set yields a different fan_context_key, so the
    // entry stored under the original leg does not leak across — it misses.
    ut.with_rest_leg(outer_inner, outer_hoon, |ut| {
        ut.with_rest_leg(inner, hoon, |ut| {
            let context_key = ut.hold_repo_fan_context_key();
            assert_ne!(context_key, first_context);
            assert!(ut
                .rest_boundary_lookup(rest_sut, legs_noun)
                .expect("nested rest boundary lookup")
                .is_none());
            Ok(())
        })
    })
    .expect("nested rest legs should succeed");
}

#[test]
fn active_rest_fan_context_is_order_insensitive_for_same_leg_set() {
    let mut slab = NounSlab::new();
    let inner_a = core_with_context_face_only(&mut slab, "tree");
    let hoon_a = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let inner_b = core_with_context_face_only(&mut slab, "node");
    let hoon_b = hoon_to_noun(&mut slab, &Hoon::Axis(2));

    let mut ut = Ut::new(&mut slab);
    let key_ab = ut
        .with_rest_leg(inner_a, hoon_a, |ut| {
            ut.with_rest_leg(inner_b, hoon_b, |ut| Ok(ut.hold_repo_fan_context_key()))
        })
        .expect("a->b rest legs should succeed");
    let key_ba = ut
        .with_rest_leg(inner_b, hoon_b, |ut| {
            ut.with_rest_leg(inner_a, hoon_a, |ut| Ok(ut.hold_repo_fan_context_key()))
        })
        .expect("b->a rest legs should succeed");

    assert_eq!(
        key_ab, key_ba,
        "same active rest-leg set should reuse the same fan context",
    );
}

#[test]
fn active_rest_fan_context_is_structural_for_equivalent_leg_pairs() {
    let mut slab = NounSlab::new();
    let inner_a = core_with_context_face_only(&mut slab, "tree");
    let hoon_a = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let inner_b = core_with_context_face_only(&mut slab, "tree");
    let hoon_b = hoon_to_noun(&mut slab, &Hoon::Axis(1));

    assert!(noun_eq(inner_a, inner_b, &slab.noun_space()).expect("inner noun_eq"));
    assert!(noun_eq(hoon_a, hoon_b, &slab.noun_space()).expect("hoon noun_eq"));
    assert!(!unsafe { inner_a.raw_equals(&inner_b) });
    assert!(!unsafe { hoon_a.raw_equals(&hoon_b) });

    let mut ut = Ut::new(&mut slab);
    let key_a = ut
        .with_rest_leg(inner_a, hoon_a, |ut| Ok(ut.hold_repo_fan_context_key()))
        .expect("first structural leg should succeed");
    let key_b = ut
        .with_rest_leg(inner_b, hoon_b, |ut| Ok(ut.hold_repo_fan_context_key()))
        .expect("equivalent structural leg should reuse context");

    assert_eq!(
        key_a, key_b,
        "structurally equivalent rest legs should reuse context ids"
    );

    ut.with_rest_leg(inner_a, hoon_a, |ut| {
        let err = ut.with_rest_leg(inner_b, hoon_b, |_ut| Ok(()));
        assert!(
            matches!(err, Err(CompilerError::Noun(ref message)) if message == "rest-loop"),
            "equivalent structural leg should trigger rest-loop while active: {err:?}"
        );
        Ok(())
    })
    .expect("outer structural leg should succeed");
}

#[test]
fn repo_noncanonical_cell_errors_with_repo_fltt() {
    let mut slab = NounSlab::new();
    let head = ty_noun(&mut slab);
    let tail = ty_noun(&mut slab);
    let sut = ty_cell(&mut slab, head, tail);
    let mut ut = Ut::new(&mut slab);

    let err = ut
        .repo_noun(sut)
        .expect_err("cell input should not pass canonical repo");
    assert!(
        matches!(err, CompilerError::Noun(ref message) if message == "repo-fltt"),
        "repo should reject non-canonical passthrough inputs with repo-fltt: {err:?}"
    );
}

#[test]
fn repo_hold_matches_rest_singleton_leg() {
    let mut slab = NounSlab::new();
    let inner = core_with_context_face_only(&mut slab, "tree");
    let hoon_noun = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let hold = ty_hold(&mut slab, inner, hoon_noun);
    let legs = [(inner, hoon_noun)];

    let expected = {
        let mut ut = Ut::new(&mut slab);
        ut.rest(hold, &legs).expect("rest singleton should succeed")
    };

    let actual = {
        let mut ut = Ut::new(&mut slab);
        ut.repo_noun(hold).expect("repo hold should succeed")
    };

    assert!(
        noun_eq(expected, actual, &slab.noun_space()).expect("noun_eq"),
        "repo(%hold) should agree with canonical rest singleton semantics"
    );
}

#[test]
fn rest_allows_duplicate_legs_in_one_call() {
    let mut slab = NounSlab::new();
    let inner = core_with_context_face_only(&mut slab, "tree");
    let hoon_noun = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let single = [(inner, hoon_noun)];
    let duplicate = [(inner, hoon_noun), (inner, hoon_noun)];

    let expected = {
        let mut ut = Ut::new(&mut slab);
        let hold = ty_hold(ut.slab, inner, hoon_noun);
        ut.rest(hold, &single)
            .expect("singleton rest should succeed")
    };

    let actual = {
        let mut ut = Ut::new(&mut slab);
        let hold = ty_hold(ut.slab, inner, hoon_noun);
        ut.rest(hold, &duplicate)
            .expect("duplicate legs in one rest call should not trigger rest-loop")
    };

    assert!(
        noun_eq(expected, actual, &slab.noun_space()).expect("noun_eq"),
        "duplicate local rest legs should collapse to the same fork result"
    );
}

#[test]
fn hold_repo_fan_leg_id_shortcuts_exact_hold_raw() {
    let mut slab = NounSlab::new();
    let inner = core_with_context_face_only(&mut slab, "tree");
    let hoon_noun = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let hold = ty_hold(&mut slab, inner, hoon_noun);
    let hold_raw = unsafe { hold.as_raw() };
    let mut ut = Ut::new(&mut slab);

    let first = ut
        .hold_repo_fan_leg_id_for_hold_type(hold, inner, hoon_noun)
        .expect("first hold leg id");
    let second = ut
        .hold_repo_fan_leg_id_for_hold_type(hold, inner, hoon_noun)
        .expect("second hold leg id");

    assert_eq!(first, second, "same exact hold raw should reuse leg id");
    assert_eq!(
        ut.hold_repo_fan_leg_id_by_hold_raw.get(&hold_raw).copied(),
        Some(first)
    );
}

#[test]
fn hold_repo_fan_leg_id_shortcuts_structurally_equal_hold() {
    let mut slab = NounSlab::new();
    let inner_a = core_with_context_face_only(&mut slab, "tree");
    let hoon_a = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let hold_a = ty_hold(&mut slab, inner_a, hoon_a);
    let inner_b = core_with_context_face_only(&mut slab, "tree");
    let hoon_b = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let hold_b = ty_hold(&mut slab, inner_b, hoon_b);
    assert!(noun_eq(hold_a, hold_b, &slab.noun_space()).expect("hold noun_eq"));
    assert!(!unsafe { hold_a.raw_equals(&hold_b) });

    let mut ut = Ut::new(&mut slab);
    let first = ut
        .hold_repo_fan_leg_id_for_hold_type(hold_a, inner_a, hoon_a)
        .expect("first hold leg id");
    let second = ut
        .hold_repo_fan_leg_id_for_hold_type(hold_b, inner_b, hoon_b)
        .expect("structurally equal hold leg id");

    assert_eq!(
        first, second,
        "structurally equal holds should reuse leg id"
    );
}

#[test]
fn gain_atom_skin_hold_guard_is_structural() {
    let mut slab = NounSlab::new();
    let inner_a = ty_atom(&mut slab, "@", None);
    let hoon_a = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let hold_a = ty_hold(&mut slab, inner_a, hoon_a);
    let inner_b = ty_atom(&mut slab, "@", None);
    let hoon_b = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let hold_b = ty_hold(&mut slab, inner_b, hoon_b);
    let sut_noun = ty_noun(&mut slab);
    assert!(noun_eq(hold_a, hold_b, &slab.noun_space()).expect("hold noun_eq"));
    assert!(!unsafe { hold_a.raw_equals(&hold_b) });

    let mut ut = Ut::new(&mut slab);
    // hold_a/hold_b are structurally equal: interning collapses them to ONE Rc,
    // so seeding the native ptr-id guard with hold_a guards hold_b too.
    let sut = native_of(&mut ut.cx, sut_noun, &ut.slab.noun_space()).expect("native sut");
    let hold_a_n = native_of(&mut ut.cx, hold_a, &ut.slab.noun_space()).expect("native hold_a");
    let hold_b_n = native_of(&mut ut.cx, hold_b, &ut.slab.noun_space()).expect("native hold_b");
    let mut seen: HashSet<u64> = HashSet::new();
    assert!(seen.insert(NRc::as_ptr(&hold_a_n) as u64), "seed guard");
    let out = ut
        .gain_atom_skin(sut, hold_b_n, "@", &mut seen)
        .expect("gain atom skin");

    assert!(
        matches!(&*out, NTy::Void),
        "canonical ar:gain uses a structural set type guard for holds"
    );
}

#[test]
fn lose_leaf_skin_hold_guard_is_structural() {
    let mut slab = NounSlab::new();
    let inner_a = ty_atom(&mut slab, "@", None);
    let hoon_a = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let hold_a = ty_hold(&mut slab, inner_a, hoon_a);
    let inner_b = ty_atom(&mut slab, "@", None);
    let hoon_b = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let hold_b = ty_hold(&mut slab, inner_b, hoon_b);
    let sut_noun = ty_noun(&mut slab);
    assert!(noun_eq(hold_a, hold_b, &slab.noun_space()).expect("hold noun_eq"));
    assert!(!unsafe { hold_a.raw_equals(&hold_b) });

    let mut ut = Ut::new(&mut slab);
    let sut = native_of(&mut ut.cx, sut_noun, &ut.slab.noun_space()).expect("native sut");
    let hold_a_n = native_of(&mut ut.cx, hold_a, &ut.slab.noun_space()).expect("native hold_a");
    let hold_b_n = native_of(&mut ut.cx, hold_b, &ut.slab.noun_space()).expect("native hold_b");
    let mut seen: HashSet<u64> = HashSet::new();
    assert!(seen.insert(NRc::as_ptr(&hold_a_n) as u64), "seed guard");
    let out = ut
        .lose_leaf_skin(sut, hold_b_n, "@", &ParsedAtom::Small(7), &mut seen)
        .expect("lose leaf skin");

    assert!(
        matches!(&*out, NTy::Void),
        "canonical ar:lose uses a structural set type guard for holds"
    );
}

#[test]
fn ty_hold_cached_constructs_structurally_equal_hold_nouns() {
    let mut slab = NounSlab::new();
    let inner_a = core_with_context_face_only(&mut slab, "tree");
    let hoon_a = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let inner_b = core_with_context_face_only(&mut slab, "tree");
    let hoon_b = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let mut ut = Ut::new(&mut slab);

    let hold_a = ut
        .ty_hold_cached(inner_a, hoon_a)
        .expect("first cached hold");
    let hold_b = ut
        .ty_hold_cached(inner_b, hoon_b)
        .expect("second cached hold");

    assert!(
        noun_eq(hold_a, hold_b, &ut.slab.noun_space()).expect("noun_eq"),
        "structurally equal hold constructors should remain structurally equal without memo reuse"
    );
}

#[test]
fn take_none_fork_branches_do_not_share_hold_seen_guard() {
    let mut slab = NounSlab::new();
    let inner = ty_atom(&mut slab, "ud", None);
    let hoon_noun = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let hold = ty_hold(&mut slab, inner, hoon_noun);
    let left = ty_face(&mut slab, "left", hold);
    let right = ty_face(&mut slab, "right", hold);
    let sut_noun = ty_fork(&mut slab, vec![left, right]);
    let mut ut = Ut::new(&mut slab);
    let sut = native_of(&mut ut.cx, sut_noun, &ut.slab.noun_space()).expect("native sut");
    let calls = Cell::new(0usize);

    let out = ut
        .take_inner_head_tail(sut, None, &[], &|_ut, a| {
            calls.set(calls.get().saturating_add(1));
            Ok(a)
        })
        .expect("take through forked holds should succeed");

    assert_eq!(
        calls.get(),
        2,
        "canonical ++take should re-enter each fork branch with a fresh hold guard after ?~ i.vit"
    );
    assert!(
        matches!(&*out, NTy::Fork { .. }),
        "take should preserve the fork shape"
    );
}

#[test]
fn toss_empty_errors_need() {
    let mut slab = NounSlab::new();
    let wing = vec![Limb::Term("foo".to_string())];
    let mur_noun = ty_noun(&mut slab);
    let mut ut = Ut::new(&mut slab);
    let mur = native_of(&mut ut.cx, mur_noun, &ut.slab.noun_space()).expect("native mur");

    let err = ut
        .toss(&wing, mur, &[])
        .expect_err("empty toss should fail canonically");
    assert!(
        matches!(err, CompilerError::Noun(ref message) if message == "need"),
        "canonical ++toss should fail with need on an empty arm list: {err:?}"
    );
}

#[test]
fn toss_mismatched_axes_errors_mate() {
    let mut slab = NounSlab::new();
    let face_inner = ty_noun(&mut slab);
    let left = ty_face(&mut slab, "foo", face_inner);
    let right_head_inner = ty_noun(&mut slab);
    let right_head = ty_face(&mut slab, "foo", right_head_inner);
    let right_tail = ty_noun(&mut slab);
    let right = ty_cell(&mut slab, right_head, right_tail);
    let wing = vec![Limb::Term("foo".to_string())];
    let mur_noun = ty_atom(&mut slab, "ud", None);
    let foot = D(0);
    let mut ut = Ut::new(&mut slab);
    let mur = native_of(&mut ut.cx, mur_noun, &ut.slab.noun_space()).expect("native mur");
    let left_n = native_of(&mut ut.cx, left, &ut.slab.noun_space()).expect("native left");
    let right_n = native_of(&mut ut.cx, right, &ut.slab.noun_space()).expect("native right");

    let (left_axis, _left_new) = ut.tack(left_n.clone(), &wing, mur.clone()).expect("left tack");
    let (right_axis, _right_new) =
        ut.tack(right_n.clone(), &wing, mur.clone()).expect("right tack");
    assert_ne!(
        left_axis, right_axis,
        "test setup should produce distinct edit axes for canonical mate failure"
    );

    let err = ut
        .toss(&wing, mur, &[(left_n, foot), (right_n, foot)])
        .expect_err("toss should reject mismatched edit axes");
    assert!(
        matches!(err, CompilerError::Noun(ref message) if message == "mate"),
        "canonical ++toss should fail with mate when per-arm edit axes differ: {err:?}"
    );
}

#[test]
fn strict_ktsg_blow_collapses_done_to_constant_formula() {
    let mut slab = NounSlab::new();
    let sut = ty_atom(&mut slab, "@", Some(D(41)));
    let gol = ty_noun(&mut slab);
    let mut ut = Ut::new(&mut slab);

    let (_ty, formula) = ut
        .mint_noun(sut, gol, &Hoon::KetSig(Box::new(Hoon::Axis(1))))
        .expect("^+ mint should succeed");
    let expected = T(&mut slab, &[D(1), D(41)]);
    assert!(
        noun_eq(formula, expected, &slab.noun_space()).expect("noun_eq should not fail"),
        "strict %ktsg should collapse to constant when apex:musk is done",
    );
}

#[test]
fn kttr_buccab_example_uses_referenced_value() {
    let mut slab = NounSlab::new();
    let high_bit = Atom::from_bytes(&mut slab, &[0, 0, 0, 0, 0, 0, 0, 0, 1]).as_noun();
    let zero_bpoly = T(&mut slab, &[D(1), high_bit]);
    let zero_head_ty = ty_atom(&mut slab, "@", Some(D(1)));
    let zero_tail_ty = ty_atom(&mut slab, "@ux", Some(high_bit));
    let zero_ty = cell_type(&mut slab, zero_head_ty, zero_tail_ty).expect("zero-bpoly type");
    let sut = ty_face(&mut slab, "zero-bpoly", zero_ty);
    let gol = ty_noun(&mut slab);
    let spec = Spec::BucTis(
        Skin::Term("acc".to_string()),
        Box::new(Spec::BucCab(Hoon::Wing(vec![Limb::Term(
            "zero-bpoly".to_string(),
        )]))),
    );
    let mut ut = Ut::new(&mut slab);

    let (_ty, formula) = ut
        .mint_noun(sut, gol, &Hoon::KetTar(Box::new(spec)))
        .expect("*_zero-bpoly mint");
    let expected = T(&mut slab, &[D(1), zero_bpoly]);
    assert!(
        noun_eq(formula, expected, &slab.noun_space()).expect("noun_eq should not fail"),
        "$_ examples must use the referenced value, not the referenced mold bunt",
    );
}

#[test]
fn kttr_buccab_example_folds_core_arm_value() {
    let mut slab = NounSlab::new();
    let one = Hoon::Rock("n".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(1)));
    let forty_two = Hoon::Rock("n".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(42)));
    let arm_value = Hoon::SigLus(0, Box::new(Hoon::Pair(Box::new(one), Box::new(forty_two))));
    let mut arms = HashMap::new();
    arms.insert("foo".to_string(), arm_value);
    let mut tomes = HashMap::new();
    tomes.insert("core".to_string(), (None, arms));
    let noun_ty = ty_noun(&mut slab);
    let mut ut = Ut::new(&mut slab);
    let (core_ty, _core_formula) = ut
        .mint_noun(noun_ty, noun_ty, &Hoon::BarCen(None, tomes))
        .expect("|% core mint");
    let spec = Spec::BucTis(
        Skin::Term("acc".to_string()),
        Box::new(Spec::BucCab(Hoon::Wing(vec![Limb::Term(
            "foo".to_string(),
        )]))),
    );

    let (_ty, formula) = ut
        .mint_noun(core_ty, noun_ty, &Hoon::KetTar(Box::new(spec)))
        .expect("*_foo mint");
    let expected_value = T(&mut slab, &[D(1), D(42)]);
    let expected = T(&mut slab, &[D(1), expected_value]);
    assert!(
        noun_eq(formula, expected, &slab.noun_space()).expect("noun_eq should not fail"),
        "$_ examples must fold referenced arm values, including memo-hinted arms",
    );
}

#[test]
fn kttr_buccab_example_folds_memoized_gate_arm_call() {
    let mut slab = NounSlab::new();
    let one = Hoon::Rock("n".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(1)));
    let sample_spec = Spec::BucTis(
        Skin::Term("x".to_string()),
        Box::new(Spec::Base(BaseType::Atom("$".to_string()))),
    );
    let make_arm = Hoon::BarTis(
        Box::new(sample_spec),
        Box::new(Hoon::Pair(
            Box::new(one),
            Box::new(Hoon::Limb("x".to_string())),
        )),
    );
    let forty_two = Hoon::Rock("n".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(42)));
    let foo_arm = Hoon::SigLus(
        0,
        Box::new(Hoon::CenCol(
            Box::new(Hoon::Limb("make".to_string())),
            vec![forty_two],
        )),
    );
    let mut arms = HashMap::new();
    arms.insert("make".to_string(), make_arm);
    arms.insert("foo".to_string(), foo_arm);
    let mut tomes = HashMap::new();
    tomes.insert("core".to_string(), (None, arms));
    let noun_ty = ty_noun(&mut slab);
    let mut ut = Ut::new(&mut slab);
    let (core_ty, _core_formula) = ut
        .mint_noun(noun_ty, noun_ty, &Hoon::BarCen(None, tomes))
        .expect("|% gate core mint");
    let spec = Spec::BucTis(
        Skin::Term("acc".to_string()),
        Box::new(Spec::BucCab(Hoon::Wing(vec![Limb::Term(
            "foo".to_string(),
        )]))),
    );

    let (_ty, formula) = ut
        .mint_noun(core_ty, noun_ty, &Hoon::KetTar(Box::new(spec)))
        .expect("*_foo mint");
    let expected_value = T(&mut slab, &[D(1), D(42)]);
    let expected = T(&mut slab, &[D(1), expected_value]);
    assert!(
        noun_eq(formula, expected, &slab.noun_space()).expect("noun_eq should not fail"),
        "$_ examples must fold memoized arm calls through sibling gates",
    );
}

#[test]
fn mint_wthx_leaf_exact_match_collapses_to_constant_formula() {
    let mut slab = NounSlab::new();
    let sut = ty_atom(&mut slab, "$", Some(D(7)));
    let gol = ty_noun(&mut slab);
    let gen = Hoon::WutHax(
        Skin::Leaf("@".to_string(), ParsedAtom::Small(7)),
        vec![Limb::Axis(1)],
    );
    let mut ut = Ut::new(&mut slab);

    let (_ty, formula) = ut.mint_noun(sut, gol, &gen).expect("?# mint");

    assert!(
        is_const_bool_formula(formula, true, &slab.noun_space()),
        "canonical ar:fish constant-folds exact leaf matches to [%1 &]"
    );
}

#[test]
fn mint_wthx_null_exact_nonzero_remains_dynamic_formula() {
    let mut slab = NounSlab::new();
    let sut = ty_atom(&mut slab, "$", Some(D(7)));
    let gol = ty_noun(&mut slab);
    let gen = Hoon::WutHax(Skin::Base(BaseType::Null), vec![Limb::Axis(1)]);
    let mut ut = Ut::new(&mut slab);

    let (_ty, formula) = ut.mint_noun(sut, gol, &gen).expect("?# mint");

    let slot = T(&mut slab, &[D(0), D(1)]);
    let zero = T(&mut slab, &[D(1), D(0)]);
    let expected = T(&mut slab, &[D(5), zero, slot]);
    assert!(
        noun_eq(formula, expected, &slab.noun_space()).expect("noun_eq should not fail"),
        "canonical ar:fish routes %null through %leaf 0 instead of folding nonzero exact atoms false"
    );
}

#[test]
fn mint_wthx_flag_exact_nonflag_remains_dynamic_formula() {
    let mut slab = NounSlab::new();
    let sut = ty_atom(&mut slab, "$", Some(D(2)));
    let gol = ty_noun(&mut slab);
    let gen = Hoon::WutHax(Skin::Base(BaseType::Flag), vec![Limb::Axis(1)]);
    let mut ut = Ut::new(&mut slab);

    let (_ty, formula) = ut.mint_noun(sut, gol, &gen).expect("?# mint");

    let slot = T(&mut slab, &[D(0), D(1)]);
    let zero = T(&mut slab, &[D(1), D(0)]);
    let one = T(&mut slab, &[D(1), D(1)]);
    let eq_zero = T(&mut slab, &[D(5), slot, zero]);
    let eq_one = T(&mut slab, &[D(5), slot, one]);
    let true_formula = T(&mut slab, &[D(1), D(0)]);
    let expected = T(&mut slab, &[D(6), eq_zero, true_formula, eq_one]);
    assert!(
        noun_eq(formula, expected, &slab.noun_space()).expect("noun_eq should not fail"),
        "canonical ar:fish keeps exact non-flag atoms as a dynamic 0-or-1 test"
    );
}

#[test]
fn mint_wthx_flag_on_noun_includes_atom_guard_formula() {
    let mut slab = NounSlab::new();
    let sut = ty_noun(&mut slab);
    let gol = ty_noun(&mut slab);
    let gen = Hoon::WutHax(Skin::Base(BaseType::Flag), vec![Limb::Axis(1)]);
    let mut ut = Ut::new(&mut slab);

    let (_ty, formula) = ut.mint_noun(sut, gol, &gen).expect("?# mint");

    let slot = T(&mut slab, &[D(0), D(1)]);
    let is_cell = T(&mut slab, &[D(3), slot]);
    let false_formula = T(&mut slab, &[D(1), D(1)]);
    let true_formula = T(&mut slab, &[D(1), D(0)]);
    let atom_guard = T(&mut slab, &[D(6), is_cell, false_formula, true_formula]);
    let zero = T(&mut slab, &[D(1), D(0)]);
    let one = T(&mut slab, &[D(1), D(1)]);
    let eq_zero = T(&mut slab, &[D(5), slot, zero]);
    let eq_one = T(&mut slab, &[D(5), slot, one]);
    let true_formula = T(&mut slab, &[D(1), D(0)]);
    let eq_flag = T(&mut slab, &[D(6), eq_zero, true_formula, eq_one]);
    let false_formula = T(&mut slab, &[D(1), D(1)]);
    let expected = T(&mut slab, &[D(6), atom_guard, eq_flag, false_formula]);
    assert!(
        noun_eq(formula, expected, &slab.noun_space()).expect("noun_eq should not fail"),
        "canonical ar:fish emits %flag as atom-test AND (slot==0 OR slot==1)"
    );
}

#[test]
fn mint_wthx_base_cell_on_core_collapses_to_constant_formula() {
    let mut slab = NounSlab::new();
    let sut = core_with_context_face_only(&mut slab, "ctx");
    let gol = ty_noun(&mut slab);
    let gen = Hoon::WutHax(Skin::Base(BaseType::Cell), vec![Limb::Axis(1)]);
    let mut ut = Ut::new(&mut slab);

    let (_ty, formula) = ut.mint_noun(sut, gol, &gen).expect("?# mint");

    assert!(
        is_const_bool_formula(formula, true, &slab.noun_space()),
        "canonical ar:fish recognizes cores as cells for %base %cell"
    );
}

#[test]
fn mint_wthx_base_atom_on_core_collapses_to_constant_formula() {
    let mut slab = NounSlab::new();
    let sut = core_with_context_face_only(&mut slab, "ctx");
    let gol = ty_noun(&mut slab);
    let gen = Hoon::WutHax(
        Skin::Base(BaseType::Atom("$".to_string())),
        vec![Limb::Axis(1)],
    );
    let mut ut = Ut::new(&mut slab);

    let (_ty, formula) = ut.mint_noun(sut, gol, &gen).expect("?# mint");

    assert!(
        is_const_bool_formula(formula, false, &slab.noun_space()),
        "canonical ar:fish recognizes cores as non-atoms"
    );
}

#[test]
fn mint_wthx_cell_skin_on_core_collapses_to_constant_formula() {
    let mut slab = NounSlab::new();
    let sut = core_with_context_face_only(&mut slab, "ctx");
    let gol = ty_noun(&mut slab);
    let gen = Hoon::WutHax(
        Skin::Cell(
            Box::new(Skin::Base(BaseType::NounExpr)),
            Box::new(Skin::Base(BaseType::NounExpr)),
        ),
        vec![Limb::Axis(1)],
    );
    let mut ut = Ut::new(&mut slab);

    let (_ty, formula) = ut.mint_noun(sut, gol, &gen).expect("?# mint");

    assert!(
        is_const_bool_formula(formula, true, &slab.noun_space()),
        "canonical ar:fish recognizes cores as cells for direct %cell skins"
    );
}

#[test]
fn miss_noun_with_hold_that_expands_to_void_uses_nest() {
    let mut slab = NounSlab::new();
    let noun = ty_noun(&mut slab);
    let void = ty_void(&mut slab);
    let hoon_noun = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let hold = ty_hold(&mut slab, void, hoon_noun);
    let mut ut = Ut::new(&mut slab);

    assert!(
        ut.nest_noun(void, hold).expect("nest void hold"),
        "test setup should produce a hold that nests in %void"
    );
    assert!(
        ut.miss_noun(noun, hold).expect("miss noun hold"),
        "canonical ++miss treats %noun as disjoint from any ref that nests in %void"
    );
}

#[test]
fn strict_ktsg_blow_preserves_formula_when_apex_waits() {
    let mut slab = NounSlab::new();
    let sut = ty_noun(&mut slab);
    let gol = ty_noun(&mut slab);
    let mut ut = Ut::new(&mut slab);

    let (_ty, formula) = ut
        .mint_noun(sut, gol, &Hoon::KetSig(Box::new(Hoon::Axis(1))))
        .expect("^+ mint should succeed");
    let expected = T(&mut slab, &[D(0), D(1)]);
    assert!(
        noun_eq(formula, expected, &slab.noun_space()).expect("noun_eq should not fail"),
        "strict %ktsg should keep original formula when apex:musk returns %wait",
    );
}

#[test]
fn strict_ktsg_blow_core_branch_uses_coil_semi() {
    let mut slab = NounSlab::new();
    let sut = core_with_constant_payload_and_coil_semi(&mut slab, 7, 42);
    let gol = ty_noun(&mut slab);
    let mut ut = Ut::new(&mut slab);

    let (_ty, formula) = ut
        .mint_noun(sut, gol, &Hoon::KetSig(Box::new(Hoon::Axis(2))))
        .expect("^+ mint on core should succeed");
    let expected = T(&mut slab, &[D(1), D(42)]);
    assert!(
        noun_eq(formula, expected, &slab.noun_space()).expect("noun_eq should not fail"),
        "strict %ktsg should use coil seminoun data in bran(%core)",
    );
}

#[test]
fn strict_ktsg_blow_fork_inputs_remain_blocked() {
    let mut slab = NounSlab::new();
    let gol = ty_noun(&mut slab);
    let left = ty_atom(&mut slab, "@", Some(D(1)));
    let right = ty_atom(&mut slab, "@", Some(D(2)));
    let fork = ty_fork(&mut slab, vec![left, right]);
    let slot_one = T(&mut slab, &[D(0), D(1)]);
    let mut ut = Ut::new(&mut slab);

    let (_fork_ty, fork_formula) = ut
        .mint_noun(fork, gol, &Hoon::KetSig(Box::new(Hoon::Axis(1))))
        .expect("^+ mint on fork should succeed");
    assert!(
        noun_eq(fork_formula, slot_one, &slab.noun_space()).expect("noun_eq should not fail"),
        "strict %ktsg should keep formula for blocked fork subjects",
    );
}

#[test]
fn strict_find_core_prefers_loot_arm_before_payload_face() {
    let mut slab = NounSlab::new();
    let sut = core_with_payload_face_and_same_named_arm(&mut slab, "foo");
    let wing = vec![Limb::Term("foo".to_string())];
    let mut ut = Ut::new(&mut slab);

    let port = ut
        .find_noun(sut, Way::Read, &wing)
        .expect("strict find should resolve foo");
    match port {
        Port::Palo(palo) => match palo.opal {
            Opal::Arm { .. } => {}
            Opal::Leg(_) => panic!("strict core lookup returned leg; expected arm via loot"),
        },
        Port::Synthetic { .. } => panic!("strict find returned synthetic port"),
    }
}

#[test]
fn strict_find_core_con_true_does_not_probe_context_outside_payload_path() {
    let mut slab = NounSlab::new();
    let sut = core_with_context_face_only(&mut slab, "foo");
    let wing = vec![Limb::Term("foo".to_string())];
    let mut ut = Ut::new(&mut slab);

    assert!(
        ut.find_noun(sut, Way::Read, &wing).is_err(),
        "strict core lookup should not find context-only face when payload path does not expose it",
    );
}

#[test]
fn strict_find_core_loot_shortcut_matches_canonical_arm_precedence() {
    let mut slab = NounSlab::new();
    let sut = core_with_named_arm_and_vair(&mut slab, "foo", "lead");
    let wing = vec![Limb::Term("foo".to_string())];
    let mut ut = Ut::new(&mut slab);

    assert!(
        ut.find_noun(sut, Way::Read, &wing).is_ok(),
        "strict find(Read) should still return core arms before peel (canonical ++fond)",
    );
    assert!(
        ut.find_noun(sut, Way::Free, &wing).is_ok(),
        "strict find(Free) should still resolve the arm on the same core",
    );
}

#[test]
fn strict_fend_rejects_arm_ports() {
    let mut slab = NounSlab::new();
    let sut = core_with_payload_face_and_same_named_arm(&mut slab, "foo");
    let wing = vec![Limb::Term("foo".to_string())];
    let mut ut = Ut::new(&mut slab);

    assert!(
        ut.fend_noun(sut, Way::Read, &wing).is_err(),
        "strict fend should reject arm ports and only accept leg ports",
    );
}

#[test]
fn strict_find_does_not_treat_dollar_as_subject_alias() {
    let mut slab = NounSlab::new();
    let sut = ty_noun(&mut slab);
    let wing = vec![Limb::Term("$".to_string())];
    let mut ut = Ut::new(&mut slab);

    assert!(
        ut.find_noun(sut, Way::Read, &wing).is_err(),
        "strict find should not resolve `$` as current subject alias",
    );
}

#[test]
fn strict_play_limb_dollar_is_not_subject_alias() {
    let mut slab = NounSlab::new();
    let sut = ty_noun(&mut slab);
    let mut ut = Ut::new(&mut slab);
    // play family takes a native subject now (C-final.2).
    let sut = crate::native::ir::intern::native_of(&mut ut.cx, sut, &ut.slab.noun_space())
        .expect("native sut");

    assert!(
        ut.play_limb(sut, "$").is_err(),
        "strict play_limb should not resolve `$` as current subject alias",
    );
}

#[test]
fn strict_mint_limb_dollar_is_not_subject_alias() {
    let mut slab = NounSlab::new();
    let sut = ty_noun(&mut slab);
    let gol = ty_noun(&mut slab);
    let mut ut = Ut::new(&mut slab);

    let space = ut.slab.noun_space();
    let sut_n = crate::native::ir::intern::native_of(&mut ut.cx, sut, &space).expect("native sut");
    let gol_n = crate::native::ir::intern::native_of(&mut ut.cx, gol, &space).expect("native gol");
    assert!(
        ut.mint_limb(sut_n, gol_n, "$").is_err(),
        "strict mint_limb should not resolve `$` as current subject alias",
    );
}

#[test]
fn strict_find_does_not_treat_dollar_prefixed_axis_as_subject_alias() {
    let mut slab = NounSlab::new();
    let head = ty_noun(&mut slab);
    let tail = ty_noun(&mut slab);
    let sut = cell_type(&mut slab, head, tail).expect("cell type");
    let wing = vec![Limb::Term("$".to_string()), Limb::Axis(2)];
    let mut ut = Ut::new(&mut slab);

    assert!(
        ut.find_noun(sut, Way::Read, &wing).is_err(),
        "strict find should not resolve `$.<axis>` as subject-alias projection",
    );
}

#[test]
fn strict_play_wing_dollar_on_non_core_is_not_subject_alias() {
    let mut slab = NounSlab::new();
    let head_inner = ty_noun(&mut slab);
    let head = ty_face(&mut slab, "head", head_inner);
    let tail = ty_noun(&mut slab);
    let sut = cell_type(&mut slab, head, tail).expect("cell type");
    let wing = vec![Limb::Term("$".to_string())];
    let mut ut = Ut::new(&mut slab);
    let sut = crate::native::ir::intern::native_of(&mut ut.cx, sut, &ut.slab.noun_space())
        .expect("native sut");

    assert!(
        ut.play_wing(sut, &wing).is_err(),
        "strict play_wing should not resolve `$` as subject alias on non-core subjects",
    );
}

#[test]
fn strict_mint_wing_dollar_on_non_core_is_not_subject_alias() {
    let mut slab = NounSlab::new();
    let head_inner = ty_noun(&mut slab);
    let head = ty_face(&mut slab, "head", head_inner);
    let tail = ty_noun(&mut slab);
    let sut = cell_type(&mut slab, head, tail).expect("cell type");
    let wing = vec![Limb::Term("$".to_string())];
    let gol = ty_noun(&mut slab);
    let mut ut = Ut::new(&mut slab);

    let space = ut.slab.noun_space();
    let sut_n = crate::native::ir::intern::native_of(&mut ut.cx, sut, &space).expect("native sut");
    let gol_n = crate::native::ir::intern::native_of(&mut ut.cx, gol, &space).expect("native gol");
    assert!(
        ut.mint_wing(sut_n, gol_n, &wing).is_err(),
        "strict mint_wing should not resolve `$` as subject alias on non-core subjects",
    );
}

#[test]
fn strict_play_wing_dollar_prefixed_axis_is_not_subject_alias_projection() {
    let mut slab = NounSlab::new();
    let head_inner = ty_noun(&mut slab);
    let head = ty_face(&mut slab, "head", head_inner);
    let tail = ty_noun(&mut slab);
    let sut = cell_type(&mut slab, head, tail).expect("cell type");
    let wing = vec![Limb::Term("$".to_string()), Limb::Axis(2)];
    let mut ut = Ut::new(&mut slab);
    let sut = crate::native::ir::intern::native_of(&mut ut.cx, sut, &ut.slab.noun_space())
        .expect("native sut");

    assert!(
        ut.play_wing(sut, &wing).is_err(),
        "strict play_wing should not resolve `$.<axis>` via subject alias",
    );
}

#[test]
fn strict_mint_wing_dollar_prefixed_axis_is_not_subject_alias_projection() {
    let mut slab = NounSlab::new();
    let head_inner = ty_noun(&mut slab);
    let head = ty_face(&mut slab, "head", head_inner);
    let tail = ty_noun(&mut slab);
    let sut = cell_type(&mut slab, head, tail).expect("cell type");
    let wing = vec![Limb::Term("$".to_string()), Limb::Axis(2)];
    let gol = ty_noun(&mut slab);
    let mut ut = Ut::new(&mut slab);

    let space = ut.slab.noun_space();
    let sut_n = crate::native::ir::intern::native_of(&mut ut.cx, sut, &space).expect("native sut");
    let gol_n = crate::native::ir::intern::native_of(&mut ut.cx, gol, &space).expect("native gol");
    assert!(
        ut.mint_wing(sut_n, gol_n, &wing).is_err(),
        "strict mint_wing should not resolve `$.<axis>` via subject alias",
    );
}

#[test]
fn strict_resolve_wing_axis_reuses_fend_contract() {
    let mut slab = NounSlab::new();
    let sut = core_with_payload_face_and_same_named_arm(&mut slab, "foo");
    let wing = vec![Limb::Term("foo".to_string())];
    let mut ut = Ut::new(&mut slab);

    assert!(
        ut.resolve_wing_axis_noun(sut, &wing).is_err(),
        "strict resolve_wing_axis should follow strict fend semantics for arm-only hits",
    );
}

#[test]
fn strict_find_non_term_face_tool_is_not_transparent() {
    let mut slab = NounSlab::new();
    let noun = ty_noun(&mut slab);
    let inner = ty_face(&mut slab, "foo", noun);
    let non_term_tool = T(&mut slab, &[D(1), D(2)]);
    let sut = ty_face_tool(&mut slab, non_term_tool, inner);
    let wing = vec![Limb::Term("foo".to_string())];
    let mut ut = Ut::new(&mut slab);

    assert!(
        ut.find_noun(sut, Way::Read, &wing).is_err(),
        "strict find should not transparently descend through non-term face tools",
    );
}

#[test]
fn strict_find_atom_face_name_mismatch_does_not_descend() {
    let mut slab = NounSlab::new();
    let noun = ty_noun(&mut slab);
    let inner = ty_face(&mut slab, "bar", noun);
    let sut = ty_face(&mut slab, "foo", inner);
    let wing = vec![Limb::Term("bar".to_string())];
    let mut ut = Ut::new(&mut slab);

    assert!(
        ut.find_noun(sut, Way::Read, &wing).is_err(),
        "strict find should lose on atom-face name mismatch instead of descending",
    );
}

#[test]
fn strict_find_term_face_mismatch_loses() {
    let mut slab = NounSlab::new();
    let inner_head = {
        let tail = ty_noun(&mut slab);
        ty_face(&mut slab, "n", tail)
    };
    let inner = {
        let tail = ty_noun(&mut slab);
        ty_cell(&mut slab, inner_head, tail)
    };
    let sut = ty_face(&mut slab, "a", inner);
    let wing = vec![Limb::Term("n".to_string())];
    let mut ut = Ut::new(&mut slab);

    assert!(
        ut.find_noun(sut, Way::Read, &wing).is_err(),
        "canonical strict find should lose on atom-term face mismatch",
    );
}

#[test]
fn strict_find_structural_face_recursive_tree_ra_resolves() {
    fn mk_node(slab: &mut NounSlab, depth: usize) -> Noun {
        let leaf = ty_atom(slab, "n", Some(D(0)));
        if depth == 0 {
            return leaf;
        }
        let inner = mk_node(slab, depth - 1);
        let leaf2 = ty_atom(slab, "n", Some(D(0)));
        let left = {
            let fork = ty_fork(slab, vec![inner, leaf2]);
            ty_face(slab, "l", fork)
        };
        let right = {
            let fork = ty_fork(slab, vec![inner, leaf]);
            ty_face(slab, "r", fork)
        };
        let head = {
            let head_tail = ty_noun(slab);
            ty_face(slab, "n", head_tail)
        };
        let body = ty_cell(slab, left, right);
        ty_cell(slab, head, body)
    }

    let mut slab = NounSlab::new();
    let pair = mk_node(&mut slab, 12);
    let sut = ty_face(&mut slab, "a", pair);
    let wing = vec![Limb::Term("r".to_string()), Limb::Term("a".to_string())];
    let mut ut = Ut::new(&mut slab);

    assert!(
        ut.find_noun(sut, Way::Read, &wing).is_ok(),
        "canonical strict find should resolve [r a] when both faces are explicitly present",
    );
}

#[test]
fn strict_find_structural_fork_unmatched_is_rejected() {
    let mut slab = NounSlab::new();
    let mismatch = {
        let inner = ty_noun(&mut slab);
        ty_face(&mut slab, "a", inner)
    };
    let match_inner = {
        let inner = ty_noun(&mut slab);
        ty_face(&mut slab, "a", inner)
    };
    let match_r = ty_face(&mut slab, "r", match_inner);
    let sut = ty_fork(&mut slab, vec![mismatch, match_r]);
    let wing = vec![Limb::Term("r".to_string()), Limb::Term("a".to_string())];
    let mut ut = Ut::new(&mut slab);

    assert!(
        ut.find_noun(sut, Way::Read, &wing).is_err(),
        "canonical strict twin should reject unmatched+hit fork merges",
    );
}

#[test]
fn strict_find_structural_p_n_a_chain_loses() {
    let mut slab = NounSlab::new();
    let leaf = ty_noun(&mut slab);
    let level3_tail = ty_noun(&mut slab);
    let level3 = ty_cell(&mut slab, leaf, level3_tail);
    let level2_tail = ty_noun(&mut slab);
    let level2 = ty_cell(&mut slab, level3, level2_tail);
    let sut_tail = ty_noun(&mut slab);
    let sut = ty_cell(&mut slab, level2, sut_tail);
    let wing = vec![
        Limb::Term("p".to_string()),
        Limb::Term("n".to_string()),
        Limb::Term("a".to_string()),
    ];
    let mut ut = Ut::new(&mut slab);

    assert!(
        ut.find_noun(sut, Way::Read, &wing).is_err(),
        "canonical strict find should not use structural term aliases for [p n a]",
    );
}

#[test]
fn strict_play_wing_does_not_use_compat_fallback_ladders() {
    let mut slab = NounSlab::new();
    let sut = ty_noun(&mut slab);
    let wing = vec![Limb::Term("definitely_missing".to_string())];
    let mut ut = Ut::new(&mut slab);
    let sut = crate::native::ir::intern::native_of(&mut ut.cx, sut, &ut.slab.noun_space())
        .expect("native sut");

    assert!(ut.play_wing(sut, &wing).is_err());
}

#[test]
fn strict_mint_wing_does_not_use_compat_fallback_ladders() {
    let mut slab = NounSlab::new();
    let sut = ty_noun(&mut slab);
    let gol = ty_noun(&mut slab);
    let wing = vec![Limb::Term("definitely_missing".to_string())];
    let mut ut = Ut::new(&mut slab);

    let space = ut.slab.noun_space();
    let sut_n = crate::native::ir::intern::native_of(&mut ut.cx, sut, &space).expect("native sut");
    let gol_n = crate::native::ir::intern::native_of(&mut ut.cx, gol, &space).expect("native gol");
    assert!(ut.mint_wing(sut_n, gol_n, &wing).is_err());
}

#[test]
fn strict_fend_does_not_use_compat_fallback_ladders() {
    let mut slab = NounSlab::new();
    let sut = ty_noun(&mut slab);
    let wing = vec![Limb::Term("definitely_missing".to_string())];
    let mut ut = Ut::new(&mut slab);

    assert!(ut.fend_noun(sut, Way::Read, &wing).is_err());
}

#[test]
fn core_dox_uses_context_payload() {
    let mut slab = NounSlab::new();
    let core = core_with_context_face_only(&mut slab, "ctx");
    let mut ut = Ut::new(&mut slab);
    let dox = ut.core_dox(core).expect("core_dox");
    let space = slab.noun_space();
    let (_payload, core_coil) = type_core_parts(core, &space).expect("core parts");
    let (_garb, context, _rest) = coil_parts(core_coil, &space).expect("coil parts");
    let (dox_payload, _dox_coil) = type_core_parts(dox, &space).expect("dox parts");
    assert!(noun_eq(dox_payload, context, &space).expect("noun_eq"));
}

#[test]
fn mull_cnts_mixed_ports_errors() {
    let mut slab = NounSlab::new();
    let sut = ty_noun(&mut slab);
    let dox = ty_noun(&mut slab);
    let gol = ty_noun(&mut slab);
    let synth_typ = ty_atom(&mut slab, "ud", None);
    let leg_typ = ty_noun(&mut slab);
    let mut ut = Ut::new(&mut slab);
    let space = ut.slab.noun_space();
    let sut_n = native_of(&mut ut.cx, sut, &space).expect("native sut");
    let gol_n = native_of(&mut ut.cx, gol, &space).expect("native gol");
    let dox_n = native_of(&mut ut.cx, dox, &space).expect("native dox");
    let lug_p = Port::Synthetic {
        typ: native_of(&mut ut.cx, synth_typ, &space).expect("native synth typ"),
        formula: D(0),
    };
    let lug_q = Port::Palo(Palo {
        vein: Vec::new(),
        opal: Opal::Leg(native_of(&mut ut.cx, leg_typ, &space).expect("native leg typ")),
    });
    let result = ut.mull_cnts_with_ports(sut_n, gol_n, dox_n, &lug_p, &lug_q, &[]);
    assert!(result.is_err(), "mixed synthetic/natural should error");
}

#[test]
fn mull_endo_mismatch_returns_noun_error() {
    let mut slab = NounSlab::new();
    let leg_typ = ty_noun(&mut slab);
    let sut = ty_noun(&mut slab);
    let gol = ty_noun(&mut slab);
    let dox = ty_noun(&mut slab);
    let mut ut = Ut::new(&mut slab);
    let space = ut.slab.noun_space();
    let palo_leg = Palo {
        vein: Vec::new(),
        opal: Opal::Leg(native_of(&mut ut.cx, leg_typ, &space).expect("native leg typ")),
    };
    let palo_arm = Palo {
        vein: Vec::new(),
        opal: Opal::Arm {
            axis: 1,
            arms: Vec::new(),
        },
    };
    let sut_n = native_of(&mut ut.cx, sut, &space).expect("native sut");
    let gol_n = native_of(&mut ut.cx, gol, &space).expect("native gol");
    let dox_n = native_of(&mut ut.cx, dox, &space).expect("native dox");
    let result = ut.mull_endo(sut_n, gol_n, dox_n, &palo_leg, &palo_arm, &[]);
    match result {
        Err(CompilerError::Noun(_)) => {}
        Err(other) => panic!("expected Noun error, got {other:?}"),
        Ok(_) => panic!("expected error for mismatched opals"),
    }
}

#[test]
fn redo_fork_ambiguity_errors() {
    let mut slab = NounSlab::new();
    let payload = ty_atom(&mut slab, "ud", None);
    let face_a = ty_face(&mut slab, "a", payload);
    let face_b = ty_face(&mut slab, "b", payload);
    let reference = ty_fork(&mut slab, vec![face_a, face_b]);
    let mut ut = Ut::new(&mut slab);
    let result = ut.redo_wet_payload(payload, reference);
    assert!(result.is_err(), "ambiguous fork faces should error");
}

#[test]
fn redo_fork_overlap_without_nest_kept() {
    let mut slab = NounSlab::new();
    let payload = ty_atom(&mut slab, "$", None);
    let narrow = ty_atom(&mut slab, "ud", None);
    let face_a = ty_face(&mut slab, "a", narrow);
    let cell_head = ty_noun(&mut slab);
    let cell_tail = ty_noun(&mut slab);
    let cell = ty_cell(&mut slab, cell_head, cell_tail);
    let face_b = ty_face(&mut slab, "b", cell);
    let reference = ty_fork(&mut slab, vec![face_a, face_b]);
    let expected = term_to_noun(&mut slab, "a");
    let mut ut = Ut::new(&mut slab);
    let result = ut.redo_wet_payload(payload, reference).expect("redo");
    assert_eq!(
        type_tag(result, &ut.slab.noun_space())
            .expect("tag")
            .as_str(),
        "face"
    );
    let tool = type_face_tool(result, &ut.slab.noun_space()).expect("tool");
    assert!(noun_eq(tool, expected, &ut.slab.noun_space()).expect("tool eq"));
}

#[test]
fn redo_fork_no_match_errors() {
    let mut slab = NounSlab::new();
    let payload = ty_atom(&mut slab, "ud", None);
    let cell_a_head = ty_noun(&mut slab);
    let cell_a_tail = ty_noun(&mut slab);
    let cell_a = ty_cell(&mut slab, cell_a_head, cell_a_tail);
    let cell_b_head = ty_void(&mut slab);
    let cell_b_tail = ty_noun(&mut slab);
    let cell_b = ty_cell(&mut slab, cell_b_head, cell_b_tail);
    let reference = ty_fork(&mut slab, vec![cell_a, cell_b]);
    let mut ut = Ut::new(&mut slab);
    let result = ut.redo_wet_payload(payload, reference);
    assert!(result.is_err(), "no-match fork should error");
}

#[test]
fn redo_cell_projects_free_over_fork_reference() {
    let mut slab = NounSlab::new();
    let payload_leaf = ty_noun(&mut slab);
    let payload_face = ty_face(&mut slab, "x", payload_leaf);
    let payload_head_tail = ty_noun(&mut slab);
    let payload_head = ty_cell(&mut slab, payload_face, payload_head_tail);
    let payload_tail = ty_noun(&mut slab);
    let payload = ty_cell(&mut slab, payload_head, payload_tail);

    let shared_ref_face_inner = ty_noun(&mut slab);
    let shared_ref_face = ty_face(&mut slab, "a", shared_ref_face_inner);
    let shared_ref_tail = ty_noun(&mut slab);
    let shared_ref_head = ty_cell(&mut slab, shared_ref_face, shared_ref_tail);
    let ref_tail_a = ty_noun(&mut slab);
    let ref_tail_b = ty_atom(&mut slab, "ud", None);
    let ref_option_a = ty_cell(&mut slab, shared_ref_head, ref_tail_a);
    let ref_option_b = ty_cell(&mut slab, shared_ref_head, ref_tail_b);
    let reference = ty_fork(&mut slab, vec![ref_option_a, ref_option_b]);

    let expected_head = {
        let mut ut = Ut::new(&mut slab);
        ut.redo_wet_payload(payload_head, shared_ref_head)
            .expect("head redo should succeed")
    };
    let expected = ty_cell(&mut slab, expected_head, payload_tail);

    let mut ut = Ut::new(&mut slab);
    let actual = ut
        .redo_wet_payload(payload, reference)
        .expect("cell redo over fork reference should succeed");

    assert!(
        noun_eq(actual, expected, &ut.slab.noun_space()).expect("noun_eq"),
        "redo(cell, fork(cell ..., cell ...)) should project the reference with peek(%free 2/3)"
    );
}

#[test]
fn miss_strips_matching_faces_before_hold_comparison() {
    let mut slab = NounSlab::new();
    let hoon_noun = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let left_head = ty_noun(&mut slab);
    let left_tail = ty_noun(&mut slab);
    let left_inner = ty_cell(&mut slab, left_head, left_tail);
    let right_head = ty_noun(&mut slab);
    let right_tail = ty_noun(&mut slab);
    let right_inner = ty_cell(&mut slab, right_head, right_tail);
    let left_hold = ty_hold(&mut slab, left_inner, hoon_noun);
    let right_hold = ty_hold(&mut slab, right_inner, hoon_noun);
    let left = ty_face(&mut slab, "n", left_hold);
    let right = ty_face(&mut slab, "n", right_hold);

    let mut ut = Ut::new(&mut slab);
    assert!(
        !ut.miss_noun(left, right).expect("miss"),
        "matching faces around equal hold generators with overlapping inners should not be pruned"
    );
}

#[test]
fn redo_fork_prunes_faced_atom_and_keeps_matching_cell_face() {
    let mut slab = NounSlab::new();
    let payload_head = ty_noun(&mut slab);
    let payload_tail = ty_noun(&mut slab);
    let payload = ty_cell(&mut slab, payload_head, payload_tail);
    let atom_inner = ty_atom(&mut slab, "$", None);
    let atom_option = ty_face(&mut slab, "i", atom_inner);
    let cell_head = ty_noun(&mut slab);
    let cell_tail = ty_noun(&mut slab);
    let cell_inner = ty_cell(&mut slab, cell_head, cell_tail);
    let cell_option = ty_face(&mut slab, "i", cell_inner);
    let reference = ty_fork(&mut slab, vec![atom_option, cell_option]);
    let expected = ty_face(&mut slab, "i", payload);

    let mut ut = Ut::new(&mut slab);
    let actual = ut.redo_wet_payload(payload, reference).expect("redo");
    assert!(noun_eq(actual, expected, &ut.slab.noun_space()).expect("noun_eq"));
}

#[test]
fn redo_fork_prunes_held_atom_and_keeps_matching_held_cell_face() {
    let mut slab = NounSlab::new();
    let hoon_noun = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let payload_head = ty_noun(&mut slab);
    let payload_tail = ty_noun(&mut slab);
    let payload = ty_cell(&mut slab, payload_head, payload_tail);
    let atom_inner = ty_atom(&mut slab, "$", None);
    let atom_face = ty_face(&mut slab, "i", atom_inner);
    let atom_option = ty_hold(&mut slab, atom_face, hoon_noun);
    let cell_head = ty_noun(&mut slab);
    let cell_tail = ty_noun(&mut slab);
    let cell_inner = ty_cell(&mut slab, cell_head, cell_tail);
    let cell_face = ty_face(&mut slab, "i", cell_inner);
    let cell_option = ty_hold(&mut slab, cell_face, hoon_noun);
    let reference = ty_fork(&mut slab, vec![atom_option, cell_option]);
    let expected = ty_face(&mut slab, "i", payload);

    let mut ut = Ut::new(&mut slab);
    assert!(ut.miss_noun(payload, atom_option).expect("atom miss"));
    assert!(!ut.miss_noun(payload, cell_option).expect("cell miss"));
    let actual = ut.redo_wet_payload(payload, reference).expect("redo");
    assert!(noun_eq(actual, expected, &ut.slab.noun_space()).expect("noun_eq"));
}

#[test]
fn redo_sint_reference_hold_respects_hod_flag() {
    let mut slab = NounSlab::new();
    let payload = ty_atom(&mut slab, "ud", None);
    let held_payload = ty_atom(&mut slab, "ud", None);
    let held = ty_face(&mut slab, "a", held_payload);
    let hoon_noun = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let reference = ty_hold(&mut slab, held, hoon_noun);
    let mut ut = Ut::new(&mut slab);

    // redo_sint is native (the redo SCC flip): lift the noun args to native and
    // assert on the native enum variant the tag used to denote.
    let space = ut.slab.noun_space();
    let payload_n = native_of(&mut ut.cx, payload, &space).expect("native payload");
    let reference_n = native_of(&mut ut.cx, reference, &space).expect("native reference");

    let (opaque_ref, _opaque_state) = ut
        .redo_sint(payload_n.clone(), reference_n.clone(), false, RedoState::default())
        .expect("redo_sint(false)");
    assert!(
        matches!(&*opaque_ref, NTy::Hold { .. }),
        "sint(|) should leave reference holds opaque"
    );

    let (expanded_ref, _expanded_state) = ut
        .redo_sint(payload_n, reference_n, true, RedoState::default())
        .expect("redo_sint(true)");
    assert!(
        matches!(&*expanded_ref, NTy::Atom { .. }),
        "sint(&) should expand reference holds via repo and keep reducing"
    );
}

#[test]
fn redo_subject_hold_restores_unchanged_hold() {
    let mut slab = NounSlab::new();
    let inner = ty_atom(&mut slab, "ud", None);
    let hoon_noun = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let payload = ty_hold(&mut slab, inner, hoon_noun);

    let hinted_subject = ty_noun(&mut slab);
    let hinted_payload = ty_atom(&mut slab, "ud", None);
    let hinted_inner = ty_hint(&mut slab, hinted_subject, D(0), hinted_payload);
    let reference = ty_hold(&mut slab, hinted_inner, hoon_noun);

    let mut ut = Ut::new(&mut slab);
    let actual = ut.redo_wet_payload(payload, reference).expect("redo");
    assert!(
        noun_eq(actual, payload, &ut.slab.noun_space()).expect("noun_eq"),
        "redo should restore the original hold when the redone repo matches the repo expansion"
    );
}

#[test]
fn redo_subject_hold_stops_at_active_fan_loop() {
    let mut slab = NounSlab::new();
    let inner = ty_atom(&mut slab, "ud", None);
    let hoon_noun = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let payload = ty_hold(&mut slab, inner, hoon_noun);
    let reference_inner = ty_atom(&mut slab, "ud", None);
    let reference = ty_face(&mut slab, "a", reference_inner);
    let expected = ty_face(&mut slab, "a", payload);

    let mut ut = Ut::new(&mut slab);
    let actual = ut
        .with_rest_leg(inner, hoon_noun, |ut| {
            ut.redo_wet_payload(payload, reference)
        })
        .expect("redo with active fan");
    assert!(
        noun_eq(actual, expected, &ut.slab.noun_space()).expect("noun_eq"),
        "redo should stop at an active fan loop and keep the subject hold instead of expanding it"
    );
}

#[test]
fn fuse_hold_seen_is_scoped_to_branch() {
    let mut slab = NounSlab::new();
    let hoon_noun = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let inner = ty_noun(&mut slab);
    let hold = ty_hold(&mut slab, inner, hoon_noun);
    let sut = ty_fork(&mut slab, vec![hold, hold]);
    let ref_ = ty_atom(&mut slab, "ud", None);
    let mut ut = Ut::new(&mut slab);
    let result = ut.fuse_noun(sut, ref_);
    assert!(
        result.is_ok(),
        "fuse should not loop across sibling branches"
    );
}

#[test]
fn crop_hold_seen_is_scoped_to_branch() {
    let mut slab = NounSlab::new();
    let hoon_noun = hoon_to_noun(&mut slab, &Hoon::Axis(1));
    let inner = ty_noun(&mut slab);
    let hold = ty_hold(&mut slab, inner, hoon_noun);
    let sut = ty_fork(&mut slab, vec![hold, hold]);
    let ref_ = ty_atom(&mut slab, "ud", None);
    let mut ut = Ut::new(&mut slab);
    let result = ut.crop_noun(sut, ref_);
    assert!(
        result.is_ok(),
        "crop should not loop across sibling branches"
    );
}

#[test]
fn mull_wthx_does_not_call_skin_match_static() {
    let mut slab = NounSlab::new();
    let atom = ty_atom(&mut slab, "$", None);
    let sut_tail = ty_noun(&mut slab);
    let sut = ty_cell(&mut slab, atom, sut_tail);
    let dox = sut;
    let gol = ty_noun(&mut slab);
    let gen = Hoon::WutHax(
        Skin::Base(BaseType::Atom("ud".to_string())),
        vec![Limb::Axis(2)],
    );
    let mut ut = Ut::new(&mut slab);
    ut.skin_match_static_calls = 0;
    ut.mull_noun(sut, gol, dox, &gen).expect("mull");
    assert_eq!(ut.skin_match_static_calls, 0);
}

#[test]
fn stack_guard_wrapped_paths() {
    let mut slab = NounSlab::new();
    let payload = ty_atom(&mut slab, "$", None);
    let noun_a = ty_noun(&mut slab);
    let noun_b = ty_noun(&mut slab);
    let noun_c = ty_noun(&mut slab);
    let leg_typ = ty_noun(&mut slab);
    let mut ut = Ut::new(&mut slab);
    let space = ut.slab.noun_space();
    let palo_leg = Palo {
        vein: Vec::new(),
        opal: Opal::Leg(native_of(&mut ut.cx, leg_typ, &space).expect("native leg typ")),
    };
    let noun_a_n = native_of(&mut ut.cx, noun_a, &space).expect("native a");
    let noun_b_n = native_of(&mut ut.cx, noun_b, &space).expect("native b");
    let noun_c_n = native_of(&mut ut.cx, noun_c, &space).expect("native c");

    ut.stack_guard_calls = 0;
    ut.redo_wet_payload(payload, payload).expect("redo");
    assert!(ut.stack_guard_calls > 0, "redo should use stack guard");

    ut.stack_guard_calls = 0;
    let arms: HashMap<String, Hoon> = HashMap::new();
    ut.mull_bake(noun_a_n.clone(), noun_b_n.clone(), Poly::Dry, &arms)
        .expect("bake");
    assert!(ut.stack_guard_calls > 0, "bake should use stack guard");

    ut.stack_guard_calls = 0;
    let tomes: HashMap<String, Tome> = HashMap::new();
    ut.mull_balk(noun_b_n.clone(), noun_c_n.clone(), Poly::Dry, &tomes)
        .expect("balk");
    assert!(ut.stack_guard_calls > 0, "balk should use stack guard");

    ut.stack_guard_calls = 0;
    ut.mull_endo(noun_a_n, noun_b_n, noun_c_n, &palo_leg, &palo_leg, &[])
        .expect("endo");
    assert!(ut.stack_guard_calls > 0, "endo should use stack guard");
}

fn mack_cache_test_core(slab: &mut NounSlab, a: u64, b: u64, c: u64, d: u64) -> Noun {
    let left = T(slab, &[D(a), D(b)]);
    let right = T(slab, &[D(c), D(d)]);
    T(slab, &[left, right])
}

#[test]
fn musk_mack_core_cache_reuses_runtime_copy() {
    let mut slab = NounSlab::new();
    let core = mack_cache_test_core(&mut slab, 1, 2, 3, 4);
    let core_space = slab.noun_space();
    let mut ut = Ut::new(&mut slab);
    let context = &mut ut.musk.context as *mut _;

    unsafe {
        let first = ut.musk_mack_cached_core_in_context(&mut *context, core, &core_space);
        let second = ut.musk_mack_cached_core_in_context(&mut *context, core, &core_space);

        // The copy shares structure: every cell of the core gets its own
        // cache entry ([[1 2] [3 4]] = root + two leaves).
        assert_eq!(ut.musk.mack_core_cache_raw.len(), 3);
        assert!(!first.raw_equals(&core));
        assert!(first.raw_equals(&second));
    }
}

#[test]
fn musk_mack_core_cache_persists_across_runtime_frames() {
    let mut slab = NounSlab::new();
    let core = mack_cache_test_core(&mut slab, 1, 2, 3, 4);
    let core_space = slab.noun_space();
    let mut ut = Ut::new(&mut slab);
    let context = &mut ut.musk.context as *mut _;

    unsafe {
        let first = ut.musk_mack_cached_core_in_context(&mut *context, core, &core_space);
        (*context).with_stack_frame(0, |_context| D(0));
        let second = ut.musk_mack_cached_core_in_context(&mut *context, core, &core_space);
        assert!(first.raw_equals(&second));
        assert_eq!(ut.musk.mack_core_cache_raw.len(), 3);
    }
}

#[test]
fn musk_mack_core_cache_clears_when_runtime_is_reset() {
    let mut slab = NounSlab::new();
    let core = mack_cache_test_core(&mut slab, 1, 2, 3, 4);
    let core_space = slab.noun_space();
    let mut ut = Ut::new(&mut slab);
    let context = &mut ut.musk.context as *mut _;

    unsafe {
        ut.musk_mack_cached_core_in_context(&mut *context, core, &core_space);
    }
    assert_eq!(ut.musk.mack_core_cache_raw.len(), 3);

    ut.clear_musk_context_dependent_caches();
    assert!(ut.musk.mack_core_cache_raw.is_empty());
}

// Validates the Step-2 chunked prelude mint mechanism on a small `=>` chain:
// `=> |%(a 1) => |%(b a) b` exercises cross-layer name resolution (arm `b`
// resolves `a` from the prior layer's subject; the body resolves `b`). The
// chunked driver mints each layer in a fresh Ut (dropping per-layer resolvers),
// so a byte-identical result proves cross-layer resolution survives without the
// per-layer lazy resolvers — the correctness crux for bounding the native mint.
#[test]
fn chunked_tisgar_chain_matches_monolithic_mint() {
    use std::path::Path;

    let src = "=>  |%  ++  a  1  --  =>  |%  ++  b  a  --  b";
    let chain = crate::pipeline::parse_native_hoon_source_without_docs(
        Path::new("synthetic.hoon"),
        src,
        Vec::new(),
        false,
    )
    .expect("parse synthetic => chain");

    let mono_jam = {
        let mut slab: NounSlab = NounSlab::new();
        let mut ut = Ut::new(&mut slab);
        let sut = super::ty_noun(&mut *ut.slab);
        let gol = super::ty_noun(&mut *ut.slab);
        let (_ty, formula) = ut.mint_noun(sut, gol, &chain).expect("monolithic mint");
        drop(ut);
        slab.set_root(formula);
        slab.jam().to_vec()
    };

    let chunk_jam = {
        let mut out_slab: NounSlab = NounSlab::new();
        let sut = super::ty_noun(&mut out_slab);
        let gol = super::ty_noun(&mut out_slab);
        let (_ty, formula) = super::mint_tisgar_chain_chunked(&mut out_slab, sut, gol, &chain)
            .expect("chunked mint");
        out_slab.set_root(formula);
        out_slab.jam().to_vec()
    };

    assert_eq!(
        mono_jam, chunk_jam,
        "chunked tisgar-chain mint must be byte-identical to monolithic mint"
    );
}

// Mints a multi-arm core that exercises the hard cases: cross-arm references
// (arm `b` resolves sibling `a`, arm `c` resolves `a` and `b` through the lazy
// resolver), and a recursive trap (`|-`/`$`) inside `c` that drives `%hold`/
// fan-leg interning. Must mint deterministically run-to-run (a fresh `Context`
// per compile gives each an isolated cache universe).
//
// PRE-EXISTING failure (NOT introduced by the nest/C8 flip): with a FRESH native
// intern context, minting this bare-%noun-subject wet-`|-` core fails with
// `arm c: arm $: coil missing tail: not a cell` — confirmed by A/B against the
// pre-C8 commit, where it also fails in isolation. Same bare-%noun wet-mint
// corner as `wet_gate_function_sample_mint_deterministic`. Tracked in
// docs/native-compiler/ATOMIC-FLIP-TRACKER.md.
#[ignore = "pre-existing bare-%noun wet-|- mint failure; see ATOMIC-FLIP-TRACKER.md"]
#[test]
fn core_mint_deterministic() {
    use std::path::Path;

    let src = "\
|%
++  a  1
++  b  a
++  c  =/  x  b  |-  ?:(=(x a) x $(x x))
--";
    let gen = crate::pipeline::parse_native_hoon_source_without_docs(
        Path::new("synthetic-core.hoon"),
        src,
        Vec::new(),
        false,
    )
    .expect("parse synthetic core");

    let mint_jam = || {
        let mut slab: NounSlab = NounSlab::new();
        let mut ut = Ut::new(&mut slab);
        let sut = super::ty_noun(&mut *ut.slab);
        let gol = super::ty_noun(&mut *ut.slab);
        let (_ty, formula) = ut.mint_noun(sut, gol, &gen).expect("mint synthetic core");
        drop(ut);
        slab.set_root(formula);
        slab.jam().to_vec()
    };

    assert_eq!(
        mint_jam(),
        mint_jam(),
        "core mint must be deterministic run-to-run (fresh Context per compile)"
    );
}

// FAST repro of the dumb-kernel `poly:$` failure:
//   "native mint: find failed for wing [Parent(0, None), Axis(12)]" (way=Rite).
//
// The wing `[Parent(0,None), Axis(12)]` is synthesised ONLY by `lower_censig`
// (mod.rs:2548) for the FIRST (non-last) argument of a multi-arg cen-sig
// (`~(arm core a b ...)`): `wing_axe = peg(6,2) = 12`. `fond` (find.rs) resolves
// `Axis(12)` first via `peek`, then applies the nameless `Parent(0,None)` via
// `fond_name`. When `peek(sut, Rite, 12)` returns `NTy::Void`, the nameless-parent
// walk returns `Pony::Void` and `find` errors.
//
// This source mints in <1ms (no prelude / no 153s dumb compile) and currently
// FAILS with exactly that error. A correct peek/fond fix should make it mint OK;
// flip the assertion to `is_ok()` to use it as a fix gate.
#[test]
fn repro_censig_two_arg_wing_find_failure() {
    use std::path::Path;

    // Door (`|_ s=@`) with a 2-arg arm `f`, called via `~(f d 1 2)`. The arm body
    // uses only its own sample, so NO stdlib is needed.
    let src = "=/  d  |_  s=@\n++  f  |=  [x=@ y=@]  [x y s]\n--\n~(f d 1 2)";

    let gen = crate::pipeline::parse_native_hoon_source_without_docs(
        Path::new("repro-censig.hoon"),
        src,
        Vec::new(),
        false,
    )
    .expect("parse repro censig");

    let mut slab: NounSlab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let sut = super::ty_noun(&mut *ut.slab);
    let gol = super::ty_noun(&mut *ut.slab);
    let result = ut.mint_noun(sut, gol, &gen);

    let err = format!("{:?}", result.err());
    assert!(
        err.contains("find failed for wing [Parent(0, None), Axis(12)]"),
        "expected the censig axis-12 wing find failure, got: {err}"
    );
}

// Reproduces the shape of stdlib `turn`/`add-all`: a wet `|-` loop whose subject
// carries a binding referenced via `$(a a, b b)` — stresses cross-arm type reuse.
// Mints against a BARE %noun subject (no prelude) and must mint deterministically
// run-to-run. This caught the cross-compile intern-table aliasing that the
// per-compile `Context` (fresh per `Ut`) fixed.
#[test]
fn wet_gate_function_sample_mint_deterministic() {
    use std::path::Path;

    let src = "\
=/  a  1
=/  b  2
|-
?:(=(a b) b $(a a, b b))";
    let gen = crate::pipeline::parse_native_hoon_source_without_docs(
        Path::new("synthetic-wet.hoon"),
        src,
        Vec::new(),
        false,
    )
    .expect("parse synthetic wet gate");

    let mint_jam = || {
        let mut slab: NounSlab = NounSlab::new();
        let mut ut = Ut::new(&mut slab);
        let sut = super::ty_noun(&mut *ut.slab);
        let gol = super::ty_noun(&mut *ut.slab);
        let (_ty, formula) = ut.mint_noun(sut, gol, &gen).expect("mint synthetic wet gate");
        drop(ut);
        slab.set_root(formula);
        slab.jam().to_vec()
    };

    assert_eq!(
        mint_jam(),
        mint_jam(),
        "wet-gate mint must be deterministic run-to-run (fresh Context per compile)"
    );
}
