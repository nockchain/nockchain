//! Coverage-driven tests for `ut/mod.rs` (middle third).
//!
//! Added to close branch-coverage gaps; see the coverage report in the PR.
//! Source-level cases mirror the parity probes in
//! `test-assets/type-probes/coverage/c2/`; the rest drive internal helpers
//! (musk, seminouns, lazy resolvers, goal checks, caches) directly.

use std::path::Path as FsPath;

#[allow(unused_imports)]
use super::super::*;

type TestResult<T> = std::result::Result<T, String>;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn try_parse(src: &str) -> TestResult<Hoon> {
    let gen = crate::pipeline::parse_native_hoon_source_without_docs(
        FsPath::new("c2-cov.hoon"),
        src,
        Vec::new(),
        false,
    )
    .map_err(|err| format!("parse {src:?}: {err:?}"))?;
    Ok(match gen {
        Hoon::TisSig(mut items) if items.len() == 1 => items.pop().expect("one item"),
        other => other,
    })
}

fn parse(src: &str) -> Hoon {
    try_parse(src).unwrap_or_else(|err| panic!("{err}"))
}

/// Mint `src` against a `%noun` subject and goal with the given `vet`.
fn mint_with(ut: &mut Ut, src: &str, vet: bool) -> TestResult<(Noun, Noun)> {
    let gen = try_parse(src)?;
    let sut = ty_noun(&mut *ut.slab);
    let gol = ty_noun(&mut *ut.slab);
    ut.set_vet(vet);
    ut.mint_noun(sut, gol, &gen)
        .map_err(|err| format!("{err:?}"))
}

/// Mint `subject_src`, then mint `body` against the resulting type.
fn mint_in(ut: &mut Ut, subject_src: &str, body: &str) -> TestResult<(Noun, Noun)> {
    let (sut, _) = mint_with(ut, subject_src, true)?;
    let gen = try_parse(body)?;
    let gol = ty_noun(&mut *ut.slab);
    ut.set_vet(true);
    ut.mint_noun(sut, gol, &gen)
        .map_err(|err| format!("{err:?}"))
}

fn mint_err(src: &str) -> String {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    match mint_with(&mut ut, src, true) {
        Ok(_) => panic!("mint {src:?} should fail"),
        Err(err) => err,
    }
}

fn mint_ok(src: &str) -> (Vec<u8>, Vec<u8>) {
    let mut slab = NounSlab::new();
    let (ty, fol) = {
        let mut ut = Ut::new(&mut slab);
        mint_with(&mut ut, src, true).unwrap_or_else(|err| panic!("mint {src:?}: {err}"))
    };
    slab.set_root(ty);
    let ty_jam = slab.jam().to_vec();
    slab.set_root(fol);
    (ty_jam, slab.jam().to_vec())
}

fn play_src(ut: &mut Ut, src: &str) -> TestResult<Noun> {
    let gen = try_parse(src)?;
    let sut = ty_noun(&mut *ut.slab);
    ut.play_noun(sut, &gen).map_err(|err| format!("{err:?}"))
}

fn noun_is(slab: &NounSlab, left: Noun, right: Noun) -> bool {
    noun_eq(left, right, &slab.noun_space()).expect("noun_eq")
}

/// True when `needle` occurs anywhere inside `hay` (structural equality).
fn contains(slab: &NounSlab, hay: Noun, needle: Noun) -> bool {
    let space = slab.noun_space();
    let mut stack = vec![hay];
    while let Some(noun) = stack.pop() {
        if noun_eq(noun, needle, &space).expect("noun_eq") {
            return true;
        }
        if let Ok(cell) = noun.in_space(&space).as_cell() {
            stack.push(cell.head().noun());
            stack.push(cell.tail().noun());
        }
    }
    false
}

fn tag_of(ut: &Ut, ty: Noun) -> String {
    type_tag(ty, &ut.slab.noun_space()).expect("type tag")
}

fn to_native(ut: &mut Ut, noun: Noun) -> NRc<NTy> {
    let space = ut.slab.noun_space();
    native_of(&mut ut.cx, noun, &space).expect("native_of")
}

fn big(bits: u32) -> BigUint {
    BigUint::from(1u32) << bits
}

fn big_atom(slab: &mut NounSlab, value: &BigUint) -> Noun {
    let bytes = value.to_bytes_le();
    Atom::from_bytes(slab, &bytes).as_noun()
}

/// `depth` nested heads ending in `leaf`, so `leaf` sits at axis 2^depth.
fn nest_heads(slab: &mut NounSlab, depth: u32, leaf: Noun) -> Noun {
    let mut noun = leaf;
    for _ in 0..depth {
        noun = T(slab, &[noun, D(0)]);
    }
    noun
}

fn formula(ut: &mut Ut, id: FormulaId) -> Noun {
    ut.formula_materialize(id)
}

fn musk_out(ut: &mut Ut, bus: SemiId, fol: Noun) -> MuskOutput {
    ut.musk_apex_output(bus, fol).expect("musk")
}

fn spot() -> Spot {
    Spot {
        p: vec!["c2".to_string()],
        q: Pint {
            p: (1, 1),
            q: (1, 2),
        },
    }
}

fn term(slab: &mut NounSlab, name: &str) -> Noun {
    term_to_noun(slab, name)
}

// ---------------------------------------------------------------------------
// `?:` and its lowerings
// ---------------------------------------------------------------------------

#[test]
fn wtcl_with_void_gain_and_lose_uses_slot_zero_test() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    // `a` is a %hold of a crashing arm: the subject is not void, but refining
    // `a` collapses both branches (hoon-138 `[%void %void] => |+[%0 0]`).
    let (ty, fol) = mint_in(
        &mut ut, "=>  |%  ++  bad  !!  --  =+  a=bad  .", "?:  ?=(@ a)  !!  !!",
    )
    .expect("mint");
    assert_eq!(tag_of(&ut, ty), "void");
    let expected = T(&mut *ut.slab, &[D(0), D(0)]);
    assert!(noun_is(ut.slab, fol, expected), "double-void ?: is [%0 0]");
}

#[test]
fn play_wtcl_skips_void_branches() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    // lose is void: only the %.y branch is played.
    let ty = play_src(&mut ut, "=+  a=`@`5  ?:  ?=(@ a)  [1 2]  %foo").expect("play");
    assert_eq!(tag_of(&ut, ty), "cell");
    // gain is void: only the %.n branch is played.
    let ty = play_src(&mut ut, "=+  a=`@`5  ?:  ?=(^ a)  [1 2]  %foo").expect("play");
    assert_eq!(tag_of(&ut, ty), "atom");
    // both live: a fork.
    let ty = play_src(&mut ut, "=+  a=`*`5  ?:  ?=(^ a)  [1 2]  %foo").expect("play");
    assert_eq!(tag_of(&ut, ty), "fork");
    // play of ?= itself is a flag.
    let ty = play_src(&mut ut, "?=(@ .)").expect("play ?=");
    assert_eq!(tag_of(&ut, ty), "fork");
}

#[test]
fn wtbr_wtgl_wtgr_lower_through_wtcl() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let (_ty, fol) = mint_with(&mut ut, "?|(%.n %.y)", true).expect("?|");
    let yes = T(&mut *ut.slab, &[D(1), D(0)]);
    assert!(noun_is(ut.slab, fol, yes), "?| of constants folds to [1 &]");
    let (_ty, fol) = mint_with(&mut ut, "?<(%.n 5)", true).expect("?<");
    let five = T(&mut *ut.slab, &[D(1), D(5)]);
    assert!(
        noun_is(ut.slab, fol, five),
        "?< with a false test is the body"
    );
    let (_ty, fol) = mint_with(&mut ut, "?>(%.y 5)", true).expect("?>");
    assert!(
        noun_is(ut.slab, fol, five),
        "?> with a true test is the body"
    );
}

#[test]
fn wtpm_and_wtbr_refine_through_chip() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    // gain of ?& chains both refinements; lose of ?& opens.
    let (ty, _fol) =
        mint_with(&mut ut, "=+  a=`*`0  ?:  ?&(?=(@ a) ?=(%5 a))  a  2", true).expect("?&");
    assert_eq!(tag_of(&ut, ty), "fork");
    // lose of ?| chains both refinements; gain of ?| opens.
    let (ty, _fol) =
        mint_with(&mut ut, "=+  a=`@`0  ?:  ?|(?=(%5 a) ?=(%6 a))  1  a", true).expect("?|");
    assert_eq!(tag_of(&ut, ty), "fork");
}

#[test]
fn chip_on_wthx_through_a_synthetic_port_keeps_the_subject() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    // A `=*` alias to a non-wing hoon resolves through a synthetic port.
    let (sut, _) = mint_with(&mut ut, "=*  foo  [a=1 b=2]  .", true).expect("alias");
    let sut_n = to_native(&mut ut, sut);
    let gen = Hoon::WutHax(
        Skin::Base(BaseType::Atom("$".to_string())),
        vec![Limb::Term("a".to_string()), Limb::Term("foo".to_string())],
    );
    let out = ut.chip(true, sut_n.clone(), &gen).expect("chip");
    assert!(
        NRc::ptr_eq(&out, &sut_n),
        "a synthetic ?# target does not refine"
    );
}

#[test]
fn tscm_busks_the_subject_to_expose_a_wing() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let (ty, fol) = mint_with(&mut ut, "=+  q=[c=1 d=2]  =,  q  c", true).expect("=,");
    assert_eq!(tag_of(&ut, ty), "atom");
    // The pushed pair folds to one constant: [%8 [%1 1 2] [%0 4]].
    let consts = T(&mut *ut.slab, &[D(1), D(2)]);
    let pair = T(&mut *ut.slab, &[D(1), consts]);
    let slot = T(&mut *ut.slab, &[D(0), D(4)]);
    let expected = T(&mut *ut.slab, &[D(8), pair, slot]);
    assert!(noun_is(ut.slab, fol, expected), "=, resolves c through q");
    let ty = play_src(&mut ut, "=+  q=[c=1 d=2]  =,  q  d").expect("play =,");
    assert_eq!(tag_of(&ut, ty), "atom");
}

#[test]
fn tsmc_and_dttr_mint() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let (_ty, fol) = mint_with(&mut ut, "=;  a  +(a)  5", true).expect("=;");
    assert!(contains(ut.slab, fol, D(4)));
    let (ty, fol) = mint_with(&mut ut, ".*(0 [1 5])", true).expect(".*");
    assert_eq!(tag_of(&ut, ty), "noun");
    let s = &mut *ut.slab;
    // [%2 [%1 0] [%1 1 5]]: the constant formula folds to one quote.
    let zero = T(s, &[D(1), D(0)]);
    let consts = T(s, &[D(1), D(5)]);
    let body = T(s, &[D(1), consts]);
    let expected = T(s, &[D(2), zero, body]);
    assert!(noun_is(ut.slab, fol, expected));
}

#[test]
fn fits_through_an_arm_composes_the_arm_call() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let (_ty, fol) = mint_in(&mut ut, "=>  |%  ++  five  5  --  .", "?=(@ five)").expect("?= arm");
    // [%7 [%9 2 %0 1] fish(@, 1)] with fish(@) = flip([%3 %0 1]).
    let s = &mut *ut.slab;
    let slot1 = T(s, &[D(0), D(1)]);
    let call = T(s, &[D(9), D(2), slot1]);
    let is_cell = T(s, &[D(3), slot1]);
    let f = T(s, &[D(1), D(1)]);
    let t = T(s, &[D(1), D(0)]);
    let fish = T(s, &[D(6), is_cell, f, t]);
    let expected = T(s, &[D(7), call, fish]);
    assert!(
        noun_is(ut.slab, fol, expected),
        "%fits on an arm composes with %7"
    );
}

#[test]
fn fish_of_void_example_is_constant_false() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let void = cons_void(&mut ut.cx);
    let id = ut.type_test_formula_on_axis(void, 6u64).expect("fish void");
    let fol = formula(&mut ut, id);
    assert!(is_const_bool_formula(fol, false, &ut.slab.noun_space()));
    let (_ty, fol) = mint_in(&mut ut, "=+  a=`@`5  .", "?=(_!! a)").expect("?= void");
    assert!(is_const_bool_formula(fol, false, &ut.slab.noun_space()));
}

// ---------------------------------------------------------------------------
// Skins (`?#` test formulas, hoon-138 `++fish:ar`)
// ---------------------------------------------------------------------------

/// `++fish:ar` for `skin` on a noun of type `ref_` at axis 6, in a `%noun` subject.
fn fish_at6(ut: &mut Ut, ref_: Noun, skin: &Skin) -> Noun {
    let ref_ = to_native(ut, ref_);
    let sut = cons_noun(&mut ut.cx);
    let id = ut
        .skin_test_formula(ref_, sut, BigUint::from(6u32), skin)
        .expect("fish");
    formula(ut, id)
}

/// `Some(b)` when `fish_at6` folds to the constant `[%1 b]`.
fn fish_const(ut: &mut Ut, ref_: Noun, skin: &Skin) -> Option<bool> {
    let fol = fish_at6(ut, ref_, skin);
    let space = ut.slab.noun_space();
    if is_const_bool_formula(fol, true, &space) {
        Some(true)
    } else if is_const_bool_formula(fol, false, &space) {
        Some(false)
    } else {
        None
    }
}

#[test]
fn skin_fish_folds_static_tests_for_every_base_and_wrapper() {
    let mut slab = NounSlab::new();
    let noun = ty_noun(&mut slab);
    let atom = ty_atom(&mut slab, "ud", None);
    let zero = ty_atom(&mut slab, "$", Some(D(0)));
    let seven = ty_atom(&mut slab, "$", Some(D(7)));
    let mut ut = Ut::new(&mut slab);

    assert_eq!(
        fish_const(&mut ut, noun, &Skin::Base(BaseType::Void)),
        Some(false)
    );
    assert_eq!(
        fish_const(&mut ut, zero, &Skin::Base(BaseType::Null)),
        Some(true)
    );
    assert_eq!(fish_const(&mut ut, atom, &Skin::Base(BaseType::Null)), None);

    // A cell skin on an atom ref is [%1 |].
    let cell_skin = Skin::Cell(
        Box::new(Skin::Base(BaseType::NounExpr)),
        Box::new(Skin::Base(BaseType::NounExpr)),
    );
    assert_eq!(fish_const(&mut ut, atom, &cell_skin), Some(false));

    let at = || Box::new(Skin::Base(BaseType::Atom("$".to_string())));
    let help = NounExpr::ParsedAtom(ParsedAtom::Small(0));
    assert_eq!(
        fish_const(&mut ut, atom, &Skin::Dbug(spot(), at())),
        Some(true)
    );
    assert_eq!(
        fish_const(&mut ut, atom, &Skin::Help(help, at())),
        Some(true)
    );
    assert_eq!(
        fish_const(&mut ut, atom, &Skin::Name("x".to_string(), at())),
        Some(true)
    );
    let leaf = Skin::Leaf("$".to_string(), ParsedAtom::Small(7));
    assert_eq!(fish_const(&mut ut, seven, &leaf), Some(true));
    assert_eq!(fish_const(&mut ut, atom, &leaf), None);
    assert_eq!(fish_const(&mut ut, atom, &Skin::Wash(0)), Some(true));

    // %flag: statically true on a flag, false on a cell, unknown on an atom.
    let flag = ty_bool(&mut *ut.slab);
    let cell = ty_cell(&mut *ut.slab, noun, noun);
    let flag_skin = Skin::Base(BaseType::Flag);
    assert_eq!(fish_const(&mut ut, flag, &flag_skin), Some(true));
    assert_eq!(fish_const(&mut ut, cell, &flag_skin), Some(false));
    assert_eq!(fish_const(&mut ut, atom, &flag_skin), None);

    // Cell skins: a statically false half decides, known cell or not.
    let at_skin = Skin::Base(BaseType::Atom("$".to_string()));
    let pair_of_atoms = Skin::Cell(Box::new(at_skin.clone()), Box::new(at_skin.clone()));
    let cell_head = ty_cell(&mut *ut.slab, cell, noun);
    assert_eq!(fish_const(&mut ut, cell_head, &pair_of_atoms), Some(false));
    let cell_atom = ty_cell(&mut *ut.slab, cell, atom);
    assert_eq!(fish_const(&mut ut, cell_atom, &pair_of_atoms), Some(false));
    let maybe = ty_fork(&mut *ut.slab, vec![atom, cell_head]);
    assert_eq!(fish_const(&mut ut, maybe, &pair_of_atoms), Some(false));
    let atoms = ty_cell(&mut *ut.slab, atom, atom);
    assert_eq!(fish_const(&mut ut, atoms, &pair_of_atoms), Some(true));
}

#[test]
fn skin_fish_cell_tests_fold_with_flan() {
    let mut slab = NounSlab::new();
    let noun = ty_noun(&mut slab);
    let atom = ty_atom(&mut slab, "ud", None);
    let mut ut = Ut::new(&mut slab);
    let at_skin = || Box::new(Skin::Base(BaseType::Atom("$".to_string())));
    let noun_skin = || Box::new(Skin::Base(BaseType::NounExpr));

    // A ref known to be a cell drops the [%3 %0 axis] test: `[@ @]` on `[@ *]`
    // is just the tail's atom test.
    let known = ty_cell(&mut *ut.slab, atom, noun);
    let fol = fish_at6(&mut ut, known, &Skin::Cell(at_skin(), at_skin()));
    let s = &mut *ut.slab;
    let slot13 = T(s, &[D(0), D(13)]);
    let is_cell13 = T(s, &[D(3), slot13]);
    let no = T(s, &[D(1), D(1)]);
    let yes = T(s, &[D(1), D(0)]);
    let expected = T(s, &[D(6), is_cell13, no, yes]);
    assert!(noun_is(ut.slab, fol, expected), "known cell");

    // A statically true tail folds away (flan head [%1 &]) = head, with no %6
    // for the conjunction.
    let fol = fish_at6(&mut ut, noun, &Skin::Cell(at_skin(), noun_skin()));
    let s = &mut *ut.slab;
    let slot6 = T(s, &[D(0), D(6)]);
    let is_cell6 = T(s, &[D(3), slot6]);
    let slot12 = T(s, &[D(0), D(12)]);
    let is_cell12 = T(s, &[D(3), slot12]);
    let head = T(s, &[D(6), is_cell12, no, yes]);
    let expected = T(s, &[D(6), is_cell6, head, no]);
    assert!(noun_is(ut.slab, fol, expected), "static tail");
}

#[test]
fn skin_test_formula_builds_dynamic_tests() {
    let mut slab = NounSlab::new();
    // Subject [a=@ud b=[c=@ d=*]].
    let a_ty = ty_atom(&mut slab, "ud", None);
    let a = ty_face(&mut slab, "a", a_ty);
    let c = ty_atom(&mut slab, "$", None);
    let c = ty_face(&mut slab, "c", c);
    let d = ty_noun(&mut slab);
    let d = ty_face(&mut slab, "d", d);
    let b = ty_cell(&mut slab, c, d);
    let b = ty_face(&mut slab, "b", b);
    let sut = ty_cell(&mut slab, a, b);
    let seven_ty = ty_atom(&mut slab, "$", Some(D(7)));
    let mut ut = Ut::new(&mut slab);
    let sut = to_native(&mut ut, sut);
    let a = to_native(&mut ut, a);
    let a_ty = to_native(&mut ut, a_ty);
    let seven_ty = to_native(&mut ut, seven_ty);

    // %leaf on a non-exact atom: [%5 [%1 7] [%0 2]].
    let leaf = Skin::Leaf("$".to_string(), ParsedAtom::Small(7));
    let id = ut
        .skin_test_formula(a.clone(), sut.clone(), BigUint::from(2u32), &leaf)
        .expect("leaf");
    let fol = formula(&mut ut, id);
    let s = &mut *ut.slab;
    let seven = T(s, &[D(1), D(7)]);
    let slot2 = T(s, &[D(0), D(2)]);
    let expected = T(s, &[D(5), seven, slot2]);
    assert!(noun_is(ut.slab, fol, expected));

    // Wrappers are transparent.
    let wrapped = [
        Skin::Dbug(spot(), Box::new(leaf.clone())),
        Skin::Help(
            NounExpr::ParsedAtom(ParsedAtom::Small(0)),
            Box::new(leaf.clone()),
        ),
        Skin::Name("x".to_string(), Box::new(leaf.clone())),
    ];
    for skin in wrapped.iter() {
        let id = ut
            .skin_test_formula(a.clone(), sut.clone(), BigUint::from(2u32), skin)
            .expect("wrapper");
        let fol = formula(&mut ut, id);
        assert!(noun_is(ut.slab, fol, expected), "{skin:?} is transparent");
    }

    // %over resolves its wing in `sut`, pegs the axis, and keeps `ref`.
    let over = Skin::Over(vec![Limb::Axis(3u64.into())], Box::new(leaf.clone()));
    let id = ut
        .skin_test_formula(a_ty.clone(), sut.clone(), BigUint::from(1u32), &over)
        .expect("over");
    let fol = formula(&mut ut, id);
    let s = &mut *ut.slab;
    let seven = T(s, &[D(1), D(7)]);
    let slot3 = T(s, &[D(0), D(3)]);
    let expected3 = T(s, &[D(5), seven, slot3]);
    assert!(noun_is(ut.slab, fol, expected3), "%over retargets the test");
    let id = ut
        .skin_test_formula(seven_ty, sut.clone(), BigUint::from(1u32), &over)
        .expect("over exact");
    let fol = formula(&mut ut, id);
    assert!(
        is_const_bool_formula(fol, true, &ut.slab.noun_space()),
        "%over tests the kept ref, not the type at the pegged axis"
    );

    // Nested %over skins resolve the inner wing in the outer wing's type.
    let nested = Skin::Over(
        vec![Limb::Term("b".to_string())],
        Box::new(Skin::Over(
            vec![Limb::Term("c".to_string())],
            Box::new(leaf.clone()),
        )),
    );
    let id = ut
        .skin_test_formula(a_ty.clone(), sut.clone(), BigUint::from(1u32), &nested)
        .expect("nested over");
    let fol = formula(&mut ut, id);
    let s = &mut *ut.slab;
    let seven = T(s, &[D(1), D(7)]);
    let slot6 = T(s, &[D(0), D(6)]);
    let expected6 = T(s, &[D(5), seven, slot6]);
    assert!(noun_is(ut.slab, fol, expected6), "nested %over");

    // A %spec whose example does not nest the tested type is rejected.
    let spec = Skin::Spec(
        Box::new(Spec::Base(BaseType::Cell)),
        Box::new(Skin::Base(BaseType::NounExpr)),
    );
    let err = ut
        .skin_test_formula(a, sut, BigUint::from(2u32), &spec)
        .expect_err("cell spec on an atom");
    assert!(format!("{err:?}").contains("wthx spec"));
}

#[test]
fn skin_fish_noun_void_and_flag() {
    let mut slab = NounSlab::new();
    let noun = ty_noun(&mut slab);
    let mut ut = Ut::new(&mut slab);
    let fol = fish_at6(&mut ut, noun, &Skin::Base(BaseType::NounExpr));
    assert!(is_const_bool_formula(fol, true, &ut.slab.noun_space()));
    let fol = fish_at6(&mut ut, noun, &Skin::Base(BaseType::Void));
    assert!(is_const_bool_formula(fol, false, &ut.slab.noun_space()));
    let fol = fish_at6(&mut ut, noun, &Skin::Base(BaseType::Flag));
    // The flag test guards with an atom check, then compares against 0 and 1.
    let slot6 = T(&mut *ut.slab, &[D(0), D(6)]);
    let is_cell = T(&mut *ut.slab, &[D(3), slot6]);
    assert!(contains(ut.slab, fol, is_cell));
    let one = T(&mut *ut.slab, &[D(1), D(1)]);
    let eq_one = T(&mut *ut.slab, &[D(5), slot6, one]);
    assert!(contains(ut.slab, fol, eq_one));
}

#[test]
fn wthx_on_an_arm_tests_its_core() {
    // hoon-138 fends `[[%& 1] wing]`, so an arm resolves to its core as a leg.
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let (_ty, fol) = mint_in(&mut ut, "|%  ++  foo  5  --", "?#(@ foo)").expect("?# arm");
    assert!(is_const_bool_formula(fol, false, &ut.slab.noun_space()));
    let (_ty, fol) = mint_in(&mut ut, "|%  ++  foo  5  --", "?#(^ foo)").expect("?# arm");
    assert!(is_const_bool_formula(fol, true, &ut.slab.noun_space()));
}

// ---------------------------------------------------------------------------
// Musk (the `^~` constant folder) and seminouns
// ---------------------------------------------------------------------------

#[test]
fn musk_stops_on_malformed_and_unfoldable_formulas() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let zero = ut.semi_full_complete(D(0));
    let s = &mut *ut.slab;
    let op_too_big = big_atom(s, &big(64));
    let n = |s: &mut NounSlab, parts: &[Noun]| T(s, parts);
    let slot0 = n(s, &[D(0), D(0)]);
    let slot1 = n(s, &[D(0), D(1)]);
    let q1 = n(s, &[D(1), D(1)]);
    let q2 = n(s, &[D(1), D(2)]);
    let q3 = n(s, &[D(1), D(3)]);
    let q4 = n(s, &[D(1), D(4)]);
    let q5 = n(s, &[D(1), D(5)]);
    let q0 = n(s, &[D(1), D(0)]);
    let pair12 = n(s, &[D(1), D(2)]);
    let q_pair12 = n(s, &[D(1), pair12]);
    let q_q7 = {
        let q7 = n(s, &[D(1), D(7)]);
        n(s, &[D(1), q7])
    };
    let foo = term(s, "foo");
    let hint_stop = n(s, &[foo, slot0]);
    let q_q2q3 = n(s, &[q2, q3]);
    let stops: Vec<Noun> = vec![
        D(5),
        n(s, &[slot0, q1]),
        n(s, &[q1, slot0]),
        n(s, &[op_too_big, D(0)]),
        n(s, &[D(0), D(1), D(2)]),
        n(s, &[D(2), D(5)]),
        n(s, &[D(2), slot0, q_q7]),
        n(s, &[D(4), q_pair12]),
        n(s, &[D(5), D(1)]),
        n(s, &[D(6), D(1)]),
        n(s, &[D(6), q0, D(5)]),
        n(s, &[D(6), q2, q3, q4]),
        n(s, &[D(7), D(1)]),
        n(s, &[D(7), slot0, q5]),
        n(s, &[D(8), D(1)]),
        n(s, &[D(8), slot0, q5]),
        n(s, &[D(9), D(1)]),
        n(s, &[D(9), q2, slot1]),
        n(s, &[D(9), D(2), slot0]),
        n(s, &[D(10), D(1)]),
        n(s, &[D(10), D(5), slot1]),
        n(s, &[D(10), q_q2q3, slot1]),
        n(s, &[D(11), D(1)]),
        n(s, &[D(11), hint_stop, q5]),
        n(s, &[D(12), q5, q5]),
    ];
    let edit_cases = {
        let two_q3 = n(s, &[D(2), q3]);
        let two_stop = n(s, &[D(2), slot0]);
        vec![n(s, &[D(10), two_q3, slot0]), n(s, &[D(10), two_stop, slot1])]
    };
    for fol in stops.into_iter().chain(edit_cases) {
        assert!(
            matches!(musk_out(&mut ut, zero, fol), MuskOutput::Stop),
            "formula should stop"
        );
    }

    // A constant core without the requested arm axis, and one whose arm crashes
    // under mack, both fall back to the partial path and stop.
    let s = &mut *ut.slab;
    let pair = T(s, &[D(1), D(2)]);
    let q_pair = T(s, &[D(1), pair]);
    let missing = T(s, &[D(9), D(7), q_pair]);
    let crash_core = T(s, &[slot0, D(5)]);
    let q_crash_core = T(s, &[D(1), crash_core]);
    let crashing = T(s, &[D(9), D(2), q_crash_core]);
    assert!(matches!(musk_out(&mut ut, zero, missing), MuskOutput::Stop));
    assert!(matches!(
        musk_out(&mut ut, zero, crashing),
        MuskOutput::Stop
    ));
}

#[test]
fn musk_folds_hints_conditionals_and_increments() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let zero = ut.semi_full_complete(D(0));
    let s = &mut *ut.slab;
    let q1 = T(s, &[D(1), D(1)]);
    let q3 = T(s, &[D(1), D(3)]);
    let q4 = T(s, &[D(1), D(4)]);
    let q5 = T(s, &[D(1), D(5)]);
    let q6 = T(s, &[D(1), D(6)]);
    let foo = term(s, "foo");
    let dyn_hint = T(s, &[foo, q6]);
    let static_hint = T(s, &[D(11), foo, q5]);
    let dynamic_hint = T(s, &[D(11), dyn_hint, q5]);
    let cond = T(s, &[D(6), q1, q3, q4]);
    for (fol, want) in [(static_hint, 5u64), (dynamic_hint, 5), (cond, 4)] {
        match musk_out(&mut ut, zero, fol) {
            MuskOutput::Done(noun) => assert!(noun_is(ut.slab, noun, D(want))),
            other => panic!("expected a fold, got {other:?}"),
        }
    }

    let s = &mut *ut.slab;
    let q41 = T(s, &[D(1), D(41)]);
    let inc = T(s, &[D(4), q41]);
    match musk_out(&mut ut, zero, inc) {
        MuskOutput::Done(noun) => assert!(noun_is(ut.slab, noun, D(42))),
        other => panic!("expected 42, got {other:?}"),
    }

    // Op 9 at a wide axis of a partial core fragments instead of macking.
    let blocked = ut.semi_full_blocked();
    let one = ut.semi_full_complete(D(1));
    let partial = ut.semi_combine(blocked, one).unwrap();
    let s = &mut *ut.slab;
    let a64 = big_atom(s, &big(64));
    let slot1 = T(s, &[D(0), D(1)]);
    let wide_call = T(s, &[D(9), a64, slot1]);
    assert!(!matches!(
        musk_out(&mut ut, partial, wide_call),
        MuskOutput::Done(_)
    ));

    // Increments that overflow a u64, carry through bytes, and grow a byte.
    let cases = [
        (BigUint::from(u64::MAX), big(64)),
        (big(64), big(64) + BigUint::from(1u32)),
        (big(80) - BigUint::from(1u32), big(80)),
    ];
    for (input, output) in cases {
        let s = &mut *ut.slab;
        let atom = big_atom(s, &input);
        let quoted = T(s, &[D(1), atom]);
        let fol = T(s, &[D(4), quoted]);
        let want = big_atom(s, &output);
        match musk_out(&mut ut, zero, fol) {
            MuskOutput::Done(noun) => assert!(noun_is(ut.slab, noun, want), "+({input})"),
            other => panic!("expected +({input}) to fold, got {other:?}"),
        }
    }

    // Two structurally equal %2 formulas share one dynamic memo entry.
    let s = &mut *ut.slab;
    let slot1 = T(s, &[D(0), D(1)]);
    let inner1 = T(s, &[D(1), D(5)]);
    let quoted1 = T(s, &[D(1), inner1]);
    let first = T(s, &[D(2), slot1, quoted1]);
    let slot1b = T(s, &[D(0), D(1)]);
    let inner2 = T(s, &[D(1), D(5)]);
    let quoted2 = T(s, &[D(1), inner2]);
    let second = T(s, &[D(2), slot1b, quoted2]);
    let both = T(s, &[first, second]);
    match musk_out(&mut ut, zero, both) {
        MuskOutput::Done(noun) => {
            let want = T(&mut *ut.slab, &[D(5), D(5)]);
            assert!(noun_is(ut.slab, noun, want));
        }
        other => panic!("expected [5 5], got {other:?}"),
    }
}

#[test]
fn musk_handles_axes_wider_than_a_u64() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let zero = ut.semi_full_complete(D(0));
    let s = &mut *ut.slab;
    let a64 = big_atom(s, &big(64));
    let a65 = big_atom(s, &big(65));
    let deep64 = nest_heads(s, 64, D(7));
    let q5 = T(s, &[D(1), D(5)]);
    let deep65 = nest_heads(s, 65, q5);
    let slot1 = T(s, &[D(0), D(1)]);
    let q_deep64 = T(s, &[D(1), deep64]);
    let q_deep65 = T(s, &[D(1), deep65]);
    // Op 0 at 2^64 of a deep constant.
    let frag = T(s, &[D(0), a64]);
    let frag_fol = T(s, &[D(7), q_deep64, frag]);
    // Op 9 at 2^65 of a constant core: mack at a wide axis.
    let call = T(s, &[D(9), a65, q_deep65]);
    // Op 9 at 2^64 of [1 2]: mack fails, the partial path stops.
    let pair = T(s, &[D(1), D(2)]);
    let q_pair = T(s, &[D(1), pair]);
    let bad_call = T(s, &[D(9), a64, q_pair]);
    // Op 10 at 2^64: edit the deep constant.
    let q3 = T(s, &[D(1), D(3)]);
    let edit_spec = T(s, &[a64, q3]);
    let edit = T(s, &[D(10), edit_spec, q_deep64]);
    let edited = nest_heads(s, 64, D(3));
    let bad_edit = T(s, &[D(10), edit_spec, q_pair]);
    let _ = slot1;

    match musk_out(&mut ut, zero, frag_fol) {
        MuskOutput::Done(noun) => assert!(noun_is(ut.slab, noun, D(7))),
        other => panic!("wide fragment: {other:?}"),
    }
    match musk_out(&mut ut, zero, call) {
        MuskOutput::Done(noun) => assert!(noun_is(ut.slab, noun, D(5))),
        other => panic!("wide mack: {other:?}"),
    }
    assert!(matches!(
        musk_out(&mut ut, zero, bad_call),
        MuskOutput::Stop
    ));
    match musk_out(&mut ut, zero, edit) {
        MuskOutput::Done(noun) => assert!(noun_is(ut.slab, noun, edited)),
        other => panic!("wide edit: {other:?}"),
    }
    assert!(matches!(
        musk_out(&mut ut, zero, bad_edit),
        MuskOutput::Stop
    ));
}

#[test]
fn semi_fragment_big_walks_every_node_kind() {
    let mut slab = NounSlab::new();
    let deep = nest_heads(&mut slab, 64, D(7));
    let tail_deep = {
        let inner = nest_heads(&mut slab, 64, D(9));
        T(&mut slab, &[D(0), inner])
    };
    let pair = T(&mut slab, &[D(1), D(2)]);
    let mut ut = Ut::new(&mut slab);
    let two64 = big(64);
    let c_pair = ut.semi_full_complete(pair);
    assert!(ut.semi_fragment_big(&two64, c_pair).unwrap().is_none());
    let c_deep = ut.semi_full_complete(deep);
    let hit = ut
        .semi_fragment_big(&two64, c_deep)
        .unwrap()
        .expect("deep head");
    assert!(ut.semi_full_complete_data(hit).is_some());
    let c_tail = ut.semi_full_complete(tail_deep);
    let three64 = BigUint::from(3u32) << 64u32;
    let hit = ut
        .semi_fragment_big(&three64, c_tail)
        .unwrap()
        .expect("deep tail");
    let (noun, _) = ut.semi_full_complete_data(hit).expect("complete");
    assert!(noun_is(ut.slab, noun, D(9)));

    let c_atom = ut.semi_full_complete(D(0));
    assert!(ut.semi_fragment_big(&two64, c_atom).unwrap().is_none());
    let blocked = ut.semi_full_blocked();
    assert_eq!(
        ut.semi_fragment_big(&two64, blocked).unwrap(),
        Some(blocked)
    );
    let tail_half = ut.semi_combine(c_pair, blocked).unwrap();
    let got = ut
        .semi_fragment_big(&three64, tail_half)
        .unwrap()
        .expect("half tail");
    assert!(matches!(ut.semi_arena.node(got), SemiNode::Blocked));
    let half = ut.semi_combine(blocked, c_pair).unwrap();
    let got = ut
        .semi_fragment_big(&two64, half)
        .unwrap()
        .expect("half head");
    assert!(matches!(ut.semi_arena.node(got), SemiNode::Blocked));
    let lazy = ut.semi_arena.lazy(BigUint::from(1u32), LazyResolverId(99));
    let got = ut.semi_fragment_big(&two64, lazy).unwrap().expect("lazy");
    match ut.semi_arena.node(got) {
        SemiNode::Lazy { fragment, .. } => assert_eq!(*fragment, two64),
        other => panic!("expected lazy, got {other:?}"),
    }
    // Small axes route through the u64 walker.
    assert_eq!(
        ut.semi_fragment_big(&BigUint::from(1u32), c_pair).unwrap(),
        Some(c_pair)
    );
    assert!(ut
        .semi_fragment_big(&BigUint::from(0u32), c_pair)
        .unwrap()
        .is_none());
}

#[test]
fn semi_mutate_small_and_wide_axes() {
    let mut slab = NounSlab::new();
    let pair = T(&mut slab, &[D(5), D(6)]);
    let head_pair = {
        let inner = T(&mut slab, &[D(1), D(2)]);
        T(&mut slab, &[inner, D(6)])
    };
    let deep = nest_heads(&mut slab, 64, D(7));
    let tail_deep = {
        let inner = nest_heads(&mut slab, 63, D(9));
        T(&mut slab, &[D(0), inner])
    };
    let mut ut = Ut::new(&mut slab);
    let rep = ut.semi_full_complete(D(3));
    let atom = ut.semi_full_complete(D(5));
    let c_pair = ut.semi_full_complete(pair);
    let c_head_pair = ut.semi_full_complete(head_pair);

    assert!(ut.semi_mutate(0, rep, c_pair).unwrap().is_none());
    assert_eq!(ut.semi_mutate(1, rep, c_pair).unwrap(), Some(rep));
    assert!(ut.semi_mutate(2, rep, atom).unwrap().is_none());
    assert!(ut.semi_mutate(3, rep, atom).unwrap().is_none());
    assert!(ut.semi_mutate(4, rep, atom).unwrap().is_none());
    assert!(ut.semi_mutate(4, rep, c_pair).unwrap().is_none());
    assert!(ut.semi_mutate(7, rep, c_pair).unwrap().is_none());
    assert!(ut.semi_mutate(4, rep, c_head_pair).unwrap().is_some());

    // Wide axes: small ones delegate, head and tail descents rebuild, and
    // descents into atoms stop.
    assert!(ut.semi_mutate_big(&BigUint::from(1u32), rep, atom).unwrap() == Some(rep));
    assert!(ut.semi_mutate_big(&big(64), rep, atom).unwrap().is_none());
    assert!(ut.semi_mutate_big(&big(64), rep, c_pair).unwrap().is_none());
    let c_deep = ut.semi_full_complete(deep);
    let out = ut
        .semi_mutate_big(&big(64), rep, c_deep)
        .unwrap()
        .expect("deep edit");
    let (noun, _) = ut.semi_full_complete_data(out).expect("complete");
    let want = nest_heads(&mut *ut.slab, 64, D(3));
    assert!(noun_is(ut.slab, noun, want));
    let c_tail = ut.semi_full_complete(tail_deep);
    let axis = BigUint::from(3u32) << 63u32;
    let out = ut
        .semi_mutate_big(&axis, rep, c_tail)
        .unwrap()
        .expect("tail edit");
    assert!(ut.semi_full_complete_data(out).is_some());
    let wide_tail = BigUint::from(3u32) << 64u32;
    assert!(ut
        .semi_mutate_big(&wide_tail, rep, c_pair)
        .unwrap()
        .is_none());

    assert!(ut
        .semi_require(None, |_ut, _noun| Ok(None))
        .unwrap()
        .is_none());
}

#[test]
fn semi_import_decodes_masks_and_rejects_malformed_ones() {
    let mut slab = NounSlab::new();
    let full = D(SEMI_TAG_FULL);
    let half = D(SEMI_TAG_HALF);
    let lazy = D(SEMI_TAG_LAZY);
    let complete_mask = T(&mut slab, &[full, D(0)]);
    let blocks = T(&mut slab, &[D(0), D(0), D(0)]);
    let blocked_mask = T(&mut slab, &[full, blocks]);
    let half_masks = T(&mut slab, &[complete_mask, blocked_mask]);
    let half_mask = T(&mut slab, &[half, half_masks]);
    let half_data = T(&mut slab, &[D(5), D(0)]);
    let half_semi = T(&mut slab, &[half_mask, half_data]);
    let half_mask_atom = T(&mut slab, &[half, D(7)]);
    let bad_half_masks = T(&mut slab, &[half_mask_atom, half_data]);
    let bad_half_data = T(&mut slab, &[half_mask, D(9)]);

    let frag_res = T(&mut slab, &[D(6), D(3)]);
    let lazy_mask = T(&mut slab, &[lazy, frag_res]);
    let lazy_semi = T(&mut slab, &[lazy_mask, D(0)]);
    let lazy_atom_tail = T(&mut slab, &[lazy, D(7)]);
    let bad_lazy_tail = T(&mut slab, &[lazy_atom_tail, D(0)]);
    let cell_frag = {
        let frag = T(&mut slab, &[D(1), D(2)]);
        let parts = T(&mut slab, &[frag, D(3)]);
        let mask = T(&mut slab, &[lazy, parts]);
        T(&mut slab, &[mask, D(0)])
    };
    let cell_res = {
        let res = T(&mut slab, &[D(1), D(2)]);
        let parts = T(&mut slab, &[D(6), res]);
        let mask = T(&mut slab, &[lazy, parts]);
        T(&mut slab, &[mask, D(0)])
    };
    let wide_res = {
        let res = big_atom(&mut slab, &big(70));
        let parts = T(&mut slab, &[D(6), res]);
        let mask = T(&mut slab, &[lazy, parts]);
        T(&mut slab, &[mask, D(0)])
    };
    let unknown = {
        let tag = term_to_noun(&mut slab, "what");
        let mask = T(&mut slab, &[tag, D(0)]);
        T(&mut slab, &[mask, D(0)])
    };
    let mut ut = Ut::new(&mut slab);

    let id = ut.semi_import_noun(half_semi).expect("half");
    assert!(matches!(ut.semi_arena.node(id), SemiNode::Half { .. }));
    let id = ut.semi_import_noun(lazy_semi).expect("lazy");
    assert!(matches!(ut.semi_arena.node(id), SemiNode::Lazy { .. }));
    let id = ut.semi_import_noun(unknown).expect("unknown tag");
    assert!(matches!(ut.semi_arena.node(id), SemiNode::Blocked));
    for bad in [bad_half_masks, bad_half_data, bad_lazy_tail, cell_frag, cell_res, wide_res] {
        let err = ut.semi_import_noun(bad).expect_err("malformed seminoun");
        assert!(matches!(err, CompilerError::Decode(_)), "{err:?}");
    }
}

#[test]
fn semi_small_helpers() {
    let mut slab = NounSlab::new();
    let full = term_to_noun(&mut slab, "full");
    let half = term_to_noun(&mut slab, "half");
    let full_mask = T(&mut slab, &[full, D(0)]);
    let full_semi = T(&mut slab, &[full_mask, D(5)]);
    let half_mask = T(&mut slab, &[half, D(0)]);
    let half_semi = T(&mut slab, &[half_mask, D(5)]);
    let atom_mask = T(&mut slab, &[D(7), D(5)]);
    let cell_head = {
        let head = T(&mut slab, &[D(1), D(2)]);
        let mask = T(&mut slab, &[head, D(0)]);
        T(&mut slab, &[mask, D(5)])
    };
    let mut ut = Ut::new(&mut slab);
    assert!(ut.semi_is_full_complete(full_semi).unwrap());
    assert!(!ut.semi_is_full_complete(half_semi).unwrap());
    assert!(!ut.semi_is_full_complete(D(5)).unwrap());
    assert!(!ut.semi_is_full_complete(atom_mask).unwrap());
    assert!(!ut.semi_is_full_complete(cell_head).unwrap());

    let first = ut.semi_blocks_root_blocked();
    let second = ut.semi_blocks_root_blocked();
    assert!(noun_is(ut.slab, first, second));

    let blocked = ut.semi_full_blocked();
    assert!(ut.semi_complete_value_id(blocked).is_err());

    ut.lazy_resolver_next_id = LazyResolverId(u64::MAX);
    assert_eq!(ut.lazy_resolver_new_id(), LazyResolverId(u64::MAX));
    assert_eq!(ut.lazy_resolver_next_id, LazyResolverId(1));
}

#[test]
fn musk_mack_recovers_when_an_arm_exhausts_the_eval_stack() {
    let mut slab = NounSlab::new();
    // Arm [8 [1 0] [9 2 [0 3]]]: push a cell and re-enter forever.
    let slot3 = T(&mut slab, &[D(0), D(3)]);
    let q0 = T(&mut slab, &[D(1), D(0)]);
    let kick = T(&mut slab, &[D(9), D(2), slot3]);
    let arm = T(&mut slab, &[D(8), q0, kick]);
    let core = T(&mut slab, &[arm, D(0)]);
    // The same loop at the wide axis 2^65.
    let a65 = big_atom(&mut slab, &big(65));
    let wide_kick = T(&mut slab, &[D(9), a65, slot3]);
    let wide_arm = T(&mut slab, &[D(8), q0, wide_kick]);
    let wide_core = nest_heads(&mut slab, 65, wide_arm);
    let mut ut = Ut::new(&mut slab);

    ut.musk.context.stack.set_alloc_floor_budget(Some(1 << 16));
    assert!(ut.musk_interpret_mack(core, 2).is_none());
    ut.musk.context.stack.set_alloc_floor_budget(Some(1 << 16));
    assert!(ut
        .musk_interpret_mack_axis_noun(wide_core, a65)
        .expect("wide mack")
        .is_none());
    ut.musk.context.stack.set_alloc_floor_budget(None);
}

// ---------------------------------------------------------------------------
// bran (`^~` subject projection)
// ---------------------------------------------------------------------------

#[test]
fn bran_blocks_a_self_referential_hold_but_folds_its_constant_head() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let (_ty, fol) = mint_in(
        &mut ut, "=>  |%  ++  foo  [%5 foo]  --  =/  x  foo  .", "^~(-.x)",
    )
    .expect("fold");
    let want = T(&mut *ut.slab, &[D(1), D(5)]);
    assert!(noun_is(ut.slab, fol, want));
}

#[test]
fn bran_cache_and_hold_guards() {
    let mut slab = NounSlab::new();
    let noun = ty_noun(&mut slab);
    let nope = hoon_to_noun(&mut slab, &Hoon::Limb("nope".to_string()));
    let broken = ty_hold(&mut slab, noun, nope);
    let five = hoon_to_noun(&mut slab, &Hoon::Axis(1u64.into()));
    let other = ty_hold(&mut slab, noun, five);
    let mut ut = Ut::new(&mut slab);
    let broken_n = to_native(&mut ut, broken);
    let other_n = to_native(&mut ut, other);
    // A %hold whose gene cannot be played is blocked.
    let semi = ut.bran_canonical_semi(broken_n.clone()).expect("bran");
    assert!(matches!(ut.semi_arena.node(semi), SemiNode::Blocked));

    assert!(!Ut::bran_seen_holds_equal(
        std::slice::from_ref(&broken_n),
        &[]
    ));
    let sut = cons_noun(&mut ut.cx);
    let blocked = ut.semi_full_blocked();
    for _ in 0..=Ut::BRAN_SEMI_CACHE_BUCKET_LIMIT {
        ut.bran_semi_cache_store(&sut, &[broken_n.clone(), other_n.clone()], blocked);
    }
    assert_eq!(
        ut.bran_semi_cache_lookup(&sut, &[broken_n.clone(), other_n.clone()])
            .unwrap(),
        Some(blocked)
    );
    // Same signature (order-insensitive), different order: no entry matches.
    assert_eq!(
        ut.bran_semi_cache_lookup(&sut, &[other_n, broken_n])
            .unwrap(),
        None
    );
}

// ---------------------------------------------------------------------------
// Lazy resolvers
// ---------------------------------------------------------------------------

#[test]
fn lazy_resolver_guards_and_caches() {
    let mut slab = NounSlab::new();
    let arm_hoon = hoon_to_noun(
        &mut slab,
        &Hoon::Sand("ud".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(5))),
    );
    let other_hoon = hoon_to_noun(
        &mut slab,
        &Hoon::Sand("ud".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(6))),
    );
    let arm_hoon_copy = hoon_to_noun(
        &mut slab,
        &Hoon::Sand("ud".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(5))),
    );
    let mut ut = Ut::new(&mut slab);
    let (core, _) = mint_with(&mut ut, "|%  ++  x  5  --", true).expect("core");
    let core_n = to_native(&mut ut, core);
    let two = BigUint::from(2u32);
    let entry = || LazyResolverArmEntry {
        arm_name: Arc::from("x"),
        hoon_noun: arm_hoon,
    };

    // Unknown resolvers answer nothing.
    let unknown = LazyResolverId(4242);
    assert!(ut
        .lazy_resolver_resolve_axis(unknown, &two)
        .unwrap()
        .is_none());
    assert!(ut
        .lazy_resolver_compile_arm(unknown, two.clone())
        .unwrap()
        .is_none());

    let id = ut.lazy_resolver_new_id();
    let mut arms = HashMap::new();
    arms.insert(two.clone(), entry());
    ut.lazy_resolver_register_context(id, core_n.clone(), Poly::Dry, arms);
    // Not an arm axis.
    assert!(ut
        .lazy_resolver_resolve_axis(id, &BigUint::from(3u32))
        .unwrap()
        .is_none());
    assert!(ut
        .lazy_resolver_compile_arm(id, BigUint::from(3u32))
        .unwrap()
        .is_none());
    // Already in progress.
    ut.lazy_resolvers
        .get_mut(&id)
        .unwrap()
        .in_progress_axes
        .insert(two.clone());
    assert!(ut
        .lazy_resolver_compile_arm(id, two.clone())
        .unwrap()
        .is_none());
    ut.lazy_resolvers
        .get_mut(&id)
        .unwrap()
        .in_progress_axes
        .clear();
    // Compiles, then answers from the cache.
    let first = ut
        .lazy_resolver_compile_arm(id, two.clone())
        .unwrap()
        .expect("arm");
    let again = ut
        .lazy_resolver_compile_arm(id, two.clone())
        .unwrap()
        .expect("cached");
    assert_eq!(first, again);
    assert_eq!(
        ut.lazy_resolver_resolve_axis(id, &two).unwrap(),
        Some(first)
    );

    // A wet resolver compiles with vet off.
    let wet = ut.lazy_resolver_new_id();
    let mut arms = HashMap::new();
    arms.insert(two.clone(), entry());
    ut.lazy_resolver_register_context(wet, core_n.clone(), Poly::Wet, arms);
    assert!(ut
        .lazy_resolver_compile_arm(wet, two.clone())
        .unwrap()
        .is_some());

    // An arm already being minted against the same core declines.
    let busy = ut.lazy_resolver_new_id();
    let mut arms = HashMap::new();
    arms.insert(two.clone(), entry());
    ut.lazy_resolver_register_context(busy, core_n.clone(), Poly::Dry, arms);
    let noun_goal = cons_noun(&mut ut.cx);
    let void_goal = cons_void(&mut ut.cx);
    let vet = ut.vet;
    let push = |ut: &mut Ut, hoon: Noun, goal: NRc<NTy>, vet: bool| {
        ut.arm_goal_in_progress.push(ArmInProgressEntry {
            key: Arc::from("x"),
            core: core_n.clone(),
            hoon,
            goal,
            vet,
        })
    };
    // Entries that differ in vet, hoon or goal do not match.
    push(&mut ut, arm_hoon, noun_goal.clone(), !vet);
    push(&mut ut, other_hoon, noun_goal.clone(), vet);
    push(&mut ut, arm_hoon, void_goal.clone(), vet);
    assert!(ut
        .arm_goal_for_hoon_in_progress(core_n.clone(), arm_hoon, noun_goal.clone(), vet)
        .unwrap()
        .is_none());
    // A structurally equal gene at another address does match.
    push(&mut ut, arm_hoon_copy, noun_goal.clone(), vet);
    assert!(ut
        .arm_goal_for_hoon_in_progress(core_n.clone(), arm_hoon, noun_goal.clone(), vet)
        .unwrap()
        .is_some());
    assert!(ut
        .lazy_resolver_compile_arm(busy, two.clone())
        .unwrap()
        .is_none());
    assert!(ut.lazy_resolvers[&busy].in_progress_axes.is_empty());
    ut.arm_goal_in_progress.clear();

    // A gene that is not a hoon noun cannot be recovered.
    let broken = ut.lazy_resolver_new_id();
    let mut arms = HashMap::new();
    arms.insert(
        two.clone(),
        LazyResolverArmEntry {
            arm_name: Arc::from("x"),
            hoon_noun: D(12345),
        },
    );
    ut.lazy_resolver_register_context(broken, core_n, Poly::Dry, arms);
    let err = ut
        .lazy_resolver_compile_arm(broken, two)
        .expect_err("undecodable arm gene");
    assert!(format!("{err:?}").contains("lazy resolver arm ast missing"));
}

// ---------------------------------------------------------------------------
// Cores: goals, chapters, batteries
// ---------------------------------------------------------------------------

#[test]
fn core_goal_mismatches_reject() {
    for (src, needle) in [
        (
            "^+  |%  +|  %aa  ++  y  1  --  |%  +|  %bb  ++  y  1  --", "unexpcted-chapter",
        ),
        ("^+  |%  ++  y  1  --  |%  ++  z  1  --", "unexpected-arm"),
        (
            "^+  |%  ++  y  1  ++  z  2  --  |%  ++  y  1  --", "core-number-of-arms",
        ),
        (
            "^+  |%  +|  %a  ++  y  1  +|  %b  ++  z  1  --  |%  ++  y  1  --",
            "core-number-of-chapters",
        ),
        ("^-  [@ *]  |%  ++  y  1  --", "core-nice"),
        ("^-  @  |%  ++  y  1  --", "core-nice"),
    ] {
        let err = mint_err(src);
        assert!(err.contains(needle), "{src}: {err}");
    }
}

#[test]
fn core_goals_through_wrappers_accept() {
    mint_ok("^+  f=|=(a=@ a)  |=(a=@ a)");
    mint_ok("^-  [* *]  |%  ++  y  1  --");
    mint_ok("=+  b=`?`%.y  ^+  ?:(b |=(a=@ a) |=(a=@ 1))  |=(a=@ a)");
    mint_ok("^+  |=(a=@ a)  |=(a=@ +(a))");
    mint_ok("^+  |%  +|  %one  ++  x  1  +|  %two  ++  y  2  --  |%  +|  %one  ++  x  3  +|  %two  ++  y  4  --");
}

#[test]
fn goal_chapter_checks_walk_wrappers_and_holds() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    // A core whose only arm names itself: its %hold repos to itself.
    let (core, _) = mint_with(&mut ut, "|%  ++  x  x  --", true).expect("core");
    let x_hoon = hoon_to_noun(&mut *ut.slab, &parse("x"));
    let hold = ty_hold(&mut *ut.slab, core, x_hoon);
    let mut seen = HashMap::new();
    ut.check_goal_core_chapter_counts(hold, 1, &mut seen)
        .expect("self-repo hold goal");
    assert!(ut.goal_core_for_mine(hold, D(0)).unwrap().is_none());

    // 130 nested faces exhaust the 128-step goal walk.
    let mut deep = core;
    for _ in 0..130 {
        deep = ty_face(&mut *ut.slab, "f", deep);
    }
    let mut seen = HashMap::new();
    ut.check_goal_core_chapter_counts(deep, 1, &mut seen)
        .expect("faces around a core");
    let tomes = {
        let space = ut.slab.noun_space();
        let (_payload, coil) = type_core_parts(core, &space).unwrap();
        let (_garb, _ctx, rest) = coil_parts(coil, &space).unwrap();
        coil_tomes(rest, &space).unwrap()
    };
    assert!(ut.goal_core_for_mine(deep, tomes).unwrap().is_none());

    // Hint-wrapped and forked core goals.
    let noun = ty_noun(&mut *ut.slab);
    let note = T(&mut *ut.slab, &[D(0), D(0)]);
    let hinted = ty_hint(&mut *ut.slab, noun, note, core);
    let mut seen = HashMap::new();
    ut.check_goal_core_chapter_counts(hinted, 1, &mut seen)
        .expect("hinted core goal");
    let forked = ty_fork(&mut *ut.slab, vec![core, hinted]);
    let mut seen = HashMap::new();
    ut.check_goal_core_chapter_counts(forked, 1, &mut seen)
        .expect("forked core goal");
    let mut seen = HashMap::new();
    let err = ut
        .check_goal_core_chapter_counts(forked, 2, &mut seen)
        .expect_err("chapter count mismatch");
    assert!(format!("{err:?}").contains("core-number-of-chapters"));

    // An undecodable expected arm gene.
    let x_key = term_to_noun(&mut *ut.slab, "x");
    let arms = map_put_mug(&mut *ut.slab, D(0), x_key, D(12345)).unwrap();
    let err = ut
        .goal_arm_expected_type(core, Some(arms), x_key)
        .expect_err("undecodable goal arm");
    assert!(format!("{err:?}").contains("goal arm ast missing"));
}

#[test]
fn multi_chapter_cores_build_tree_batteries() {
    for src in [
        "|%  +|  %a  ++  x  1  +|  %b  ++  y  2  --",
        "|%  +|  %b  ++  x  1  +|  %a  ++  y  2  --",
        "|%  +|  %x  ++  x  1  +|  %y  ++  y  2  --",
        "|%  +|  %a  ++  x  1  +|  %b  ++  y  2  +|  %c  ++  z  3  --",
        "|%  +|  %p  ++  x  1  +|  %q  ++  y  2  +|  %r  ++  z  3  +|  %s  ++  w  4  --",
        "|%  +|  %m  ++  x  1  +|  %n  ++  y  2  +|  %o  ++  z  3  +|  %k  ++  w  4  +|  %l  ++  v  5  --",
    ] {
        let mut slab = NounSlab::new();
        let mut ut = Ut::new(&mut slab);
        let (ty, fol) = mint_with(&mut ut, src, true).unwrap_or_else(|e| panic!("{src}: {e}"));
        assert_eq!(tag_of(&ut, ty), "core");
        for v in 1..=2u64 {
            let quoted = T(&mut *ut.slab, &[D(1), D(v)]);
            assert!(contains(ut.slab, fol, quoted), "{src}: arm {v} compiled");
        }
    }
}

#[test]
fn empty_maps_and_arm_errors() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let mut out = HashMap::new();
    ut.collect_lazy_resolver_arms_from_arms_map(D(0), BigUint::from(1u32), &mut out)
        .expect("empty arms");
    assert!(out.is_empty());
    let (core, _) = mint_with(&mut ut, "|%  ++  x  5  --", true).expect("core");
    let core_n = to_native(&mut ut, core);
    let battery = ut
        .build_arms_battery_from_map(D(0), core_n, Poly::Dry, core, None)
        .expect("empty battery");
    assert!(noun_is(ut.slab, battery, D(0)));
    let keys = ["a", "b", "c", "d", "e"];
    let mut map = D(0);
    for key in keys {
        let k = term_to_noun(&mut *ut.slab, key);
        map = map_put_mug(&mut *ut.slab, map, k, D(1)).unwrap();
    }
    assert_eq!(ut.map_entry_count(map).unwrap(), keys.len());

    let err = mint_err("|%  ++  x  nope  --");
    assert!(err.contains("arm x"), "{err}");
}

// ---------------------------------------------------------------------------
// Wrapping and core nesting
// ---------------------------------------------------------------------------

#[test]
fn wrap_rewraps_cells_faces_forks_and_rejects_rewrapped_cores() {
    for src in ["^|(^|(|=(a=@ a)))", "^&(^|(|=(a=@ a)))"] {
        assert!(mint_err(src).contains("wrap-core"), "{src}");
    }
    mint_ok("^?(^|(|=(a=@ a)))");
    mint_ok("^|([|=(a=@ a) f=|=(a=@ a)])");
    mint_ok("=+  b=`?`%.y  ^|(?:(b |=(a=@ a) |=(a=* 1)))");
    mint_ok("^&([f=|=(a=@ a) 7])");
}

#[test]
fn core_nesting_by_poly_context_and_variance() {
    // Dry goal, wet value.
    assert!(mint_err("^+(|=(a=@ a) |*(a=@ a))").contains("mint-nice"));
    // Wet cores nest only with equal tomes.
    mint_ok("^+(|*(a=@ a) |*(a=@ a))");
    assert!(mint_err("^+(|*(a=@ a) |*(a=@ +(a)))").contains("mint-nice"));
    // The value's context rejects its edited payload.
    assert!(mint_err("=/  g  |=(a=@ +(a))  ^+(|=(a=@ a) g(a [1 2]))").contains("mint-nice"));
    // The goal's context rejects its edited payload.
    assert!(mint_err("=/  g  |=(a=@ +(a))  ^+(g(a [1 2]) |=(a=@ a))").contains("mint-nice"));
    // Variance: iron goal against a zinc value fails; lead accepts; zinc nests zinc.
    assert!(mint_err("^+(^|(|=(a=@ a)) ^&(|=(a=@ +(a))))").contains("mint-nice"));
    mint_ok("^+(^?(|=(a=@ a)) ^|(|=(a=@ +(a))))");
    mint_ok("^+(^&(|=(a=@ a)) ^&(|=(a=@ +(a))))");
    // A core whose arm returns the core: the deep check re-enters the same pair.
    mint_ok("^+  |%  ++  x  ..x  --  |%  ++  x  ..x  --");
}

#[test]
fn nest_helpers_reject_non_cores_and_undecodable_arms() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let (core, _) = mint_with(&mut ut, "|%  ++  x  5  --", true).expect("core");
    let core_n = to_native(&mut ut, core);
    let noun_n = cons_noun(&mut ut.cx);
    let mut seen_sut = NestSeenSet::new();
    let mut seen_ref = NestSeenSet::new();
    let mut gil = NestPairSet::new();
    let mut memo = Default::default();
    assert!(ut
        .nest_core(
            noun_n.clone(),
            core_n.clone(),
            0,
            &mut seen_sut,
            &mut seen_ref,
            &mut gil,
            &mut memo
        )
        .is_err());
    assert!(ut
        .nest_core(
            core_n.clone(),
            noun_n.clone(),
            0,
            &mut seen_sut,
            &mut seen_ref,
            &mut gil,
            &mut memo
        )
        .is_err());
    assert!(ut.fork_options_native(&noun_n).is_err());

    let x_key = term_to_noun(&mut *ut.slab, "x");
    let good = hoon_to_noun(&mut *ut.slab, &Hoon::Axis(1u64.into()));
    let bad_arms = map_put_mug(&mut *ut.slab, D(0), x_key, D(12345)).unwrap();
    let good_arms = map_put_mug(&mut *ut.slab, D(0), x_key, good).unwrap();
    for (dab, hem) in [(bad_arms, good_arms), (good_arms, bad_arms)] {
        let err = ut
            .nest_deep_arms(
                dab,
                hem,
                core_n.clone(),
                core_n.clone(),
                0,
                &mut seen_sut,
                &mut seen_ref,
                &mut gil,
                &mut memo,
            )
            .expect_err("undecodable deep arm");
        assert!(format!("{err:?}").contains("arm ast missing"));
    }
}

// ---------------------------------------------------------------------------
// `%=` edits, tack/toss, synthetic ports
// ---------------------------------------------------------------------------

#[test]
fn hike_merges_siblings_and_drops_covered_edits() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let a = ut.formula_quote(D(6));
    let b = ut.formula_quote(D(7));
    let c = ut.formula_quote(D(8));
    let one = BigUint::from(1u32);
    let ax = |n: u32| BigUint::from(n);
    let root = ut.formula_slot_u64(1);
    let only_c = ut.formula_arena.edit(ax(2), c, root);
    // A later ancestor drops the earlier descendant, and a descendant after
    // its ancestor is skipped.
    assert_eq!(
        ut.hike_formula(one.clone(), &[(ax(4), a), (ax(2), c)])
            .unwrap(),
        only_c
    );
    assert_eq!(
        ut.hike_formula(one.clone(), &[(ax(2), c), (ax(4), a)])
            .unwrap(),
        only_c
    );
    // Siblings merge into their parent in both orders.
    let ab = ut.formula_cons(a, b);
    let merged = ut.formula_arena.edit(ax(2), ab, root);
    assert_eq!(
        ut.hike_formula(one.clone(), &[(ax(4), a), (ax(5), b)])
            .unwrap(),
        merged
    );
    assert_eq!(
        ut.hike_formula(one, &[(ax(5), b), (ax(4), a)]).unwrap(),
        merged
    );
}

#[test]
fn toss_requires_matching_axes_across_arms() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let (g1, _) = mint_with(&mut ut, "|=(a=@ a)", true).expect("gate");
    let (g2, _) = mint_with(&mut ut, "|=([b=@ a=@] a)", true).expect("gate");
    let g1 = to_native(&mut ut, g1);
    let g2 = to_native(&mut ut, g2);
    let at = ty_atom(&mut *ut.slab, "ud", None);
    let mur = to_native(&mut ut, at);
    let wing = vec![Limb::Term("a".to_string())];
    let (axis, out) = ut
        .cnts_toss(
            &wing,
            mur.clone(),
            &[(g1.clone(), D(0)), (g1.clone(), D(0))],
        )
        .expect("same axis");
    assert_eq!(axis, BigUint::from(6u32));
    assert_eq!(out.len(), 2);
    let err = ut
        .cnts_toss(&wing, mur.clone(), &[(g1, D(0)), (g2, D(0))])
        .expect_err("different axes");
    assert!(format!("{err:?}").contains("mate"));
    let err = ut.cnts_toss(&wing, mur, &[]).expect_err("no arms");
    assert!(format!("{err:?}").contains("need"));
}

#[test]
fn cnts_through_an_alias_is_synthetic() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    // A `=*` alias to a non-wing hoon resolves through a synthetic port.
    let (sut, _) = mint_with(&mut ut, "=*  foo  [a=1 b=2]  .", true).expect("alias");
    let gol = ty_noun(&mut *ut.slab);
    let ty = ut.play_noun(sut, &parse("a.foo")).expect("play a.foo");
    assert_eq!(tag_of(&ut, ty), "atom");
    let err = ut
        .play_noun(sut, &parse("a.foo(b 3)"))
        .expect_err("edit synthetic");
    assert!(format!("{err:?}").contains("hoon"), "{err:?}");
    let (ty, _fol) = ut.mint_noun(sut, gol, &parse("a.foo")).expect("mint a.foo");
    assert_eq!(tag_of(&ut, ty), "atom");
    let bare = Hoon::CenTis(
        vec![Limb::Term("a".to_string()), Limb::Term("foo".to_string())],
        Vec::new(),
    );
    let (ty, _fol) = ut.mint_noun(sut, gol, &bare).expect("mint %= a.foo");
    assert_eq!(tag_of(&ut, ty), "atom");
    let err = ut
        .mint_noun(sut, gol, &parse("a.foo(b 3)"))
        .expect_err("edit synthetic");
    assert!(format!("{err:?}").contains("hoon"), "{err:?}");
    let err = ut
        .mint_noun(sut, gol, &parse(".(a.foo 3)"))
        .expect_err("tack synthetic");
    assert!(format!("{err:?}").contains("tack"), "{err:?}");
    // !@ feels through the synthetic port.
    let (_ty, fol) = ut.mint_noun(sut, gol, &parse("!@(a.foo 1 2)")).expect("!@");
    let one = T(&mut *ut.slab, &[D(1), D(1)]);
    assert!(noun_is(ut.slab, fol, one));
}

#[test]
fn play_of_leg_edits_patches_the_leg_type() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let ty = play_src(&mut ut, "=+  x=[a=1 b=2]  x(a [3 4])").expect("play edit");
    assert_eq!(tag_of(&ut, ty), "cell");
}

// ---------------------------------------------------------------------------
// take (refinement through types)
// ---------------------------------------------------------------------------

#[test]
fn take_through_holds_voids_and_core_batteries() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    // Refinement into a hold that repos to void, and into a core battery.
    let (ty, _) = mint_in(
        &mut ut, "=>  |%  ++  bad  !!  --  =+  a=bad  .", "?:(?=(@ -.a) !! !!)",
    )
    .expect("void hold");
    assert_eq!(tag_of(&ut, ty), "void");
    let (_ty, fol) = mint_with(&mut ut, "=/  g  |=(c=@ c)  ?:(?=(@ -.g) 1 2)", true)
        .expect("battery refinement");
    assert!(contains(ut.slab, fol, D(2)));

    // A face-level step through a hold repos it.
    let noun = ty_noun(&mut *ut.slab);
    let gene = hoon_to_noun(
        &mut *ut.slab,
        &Hoon::Sand("ud".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(5))),
    );
    let hold = ty_hold(&mut *ut.slab, noun, gene);
    let hold_n = to_native(&mut ut, hold);
    let duz = |_ut: &mut Ut, a: NRc<NTy>| Ok(a);
    let (_axis, got) = ut.take(hold_n, &[None], &duz).expect("take hold");
    assert!(matches!(&*got, NTy::Atom { .. }));

    // A hold that repos to itself is cut on the second visit.
    let (core, _) = mint_with(&mut ut, "|%  ++  x  x  --", true).expect("core");
    let x_hoon = hoon_to_noun(&mut *ut.slab, &parse("x"));
    let loop_hold = ty_hold(&mut *ut.slab, core, x_hoon);
    let loop_n = to_native(&mut ut, loop_hold);
    let (_axis, got) = ut
        .take(loop_n, &[Some(BigUint::from(2u32))], &duz)
        .expect("take loop");
    assert!(matches!(&*got, NTy::Void));
}

// ---------------------------------------------------------------------------
// Literals, hints, %lost
// ---------------------------------------------------------------------------

#[test]
fn play_rock_sand_hint_and_lost() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let noun = cons_noun(&mut ut.cx);
    let atom = |v: u128| Box::new(NounExpr::ParsedAtom(ParsedAtom::Small(v)));
    let rock = Hoon::Rock("ud".to_string(), NounExpr::Cell(atom(1), atom(2)));
    let ty = ut.play(noun.clone(), &rock).expect("rock cell");
    assert!(matches!(&*ty, NTy::Cell(..)));
    let sand_cell = Hoon::Sand("ud".to_string(), NounExpr::Cell(atom(1), atom(2)));
    let ty = ut.play(noun.clone(), &sand_cell).expect("sand cell");
    assert!(matches!(&*ty, NTy::Cell(..)));
    let null = Hoon::Sand("n".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(0)));
    let ty = ut.play(noun.clone(), &null).expect("sand null");
    assert!(matches!(&*ty, NTy::Atom { .. }));
    let bad_null = Hoon::Sand("n".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(1)));
    assert!(format!("{:?}", ut.play(noun.clone(), &bad_null).unwrap_err()).contains("sand-null"));
    let bad_flag = Hoon::Sand("f".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(2)));
    assert!(format!("{:?}", ut.play(noun.clone(), &bad_flag).unwrap_err()).contains("sand-flag"));

    // Hints over void and noun payloads collapse to the payload.
    let void = cons_void(&mut ut.cx);
    let hinted = ut
        .hint_type(noun.clone(), D(0), void.clone())
        .expect("hint void");
    assert!(NRc::ptr_eq(&hinted, &void));
    let hinted = ut
        .hint_type(noun.clone(), D(0), noun.clone())
        .expect("hint noun");
    assert!(NRc::ptr_eq(&hinted, &noun));

    // %lost mints to void with vet off and rejects with vet on.
    let lost = Hoon::Lost(Box::new(Hoon::Axis(1u64.into())));
    ut.set_vet(false);
    let (ty, _fol) = ut
        .mint(noun.clone(), noun.clone(), &lost)
        .expect("lost, vet off");
    assert!(matches!(&*ty, NTy::Void));
    ut.set_vet(true);
    assert!(format!("{:?}", ut.mint(noun.clone(), noun, &lost).unwrap_err()).contains("mint-lost"));
    let (_ty, fol) = mint_with(&mut ut, "=+  b=`?`%.y  !=(?-(b %.y 1))", true).expect("!= ?-");
    assert!(contains(ut.slab, fol, D(1)));
}

#[test]
fn play_brcb_wraps_arms_in_aliases() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let with_alias = play_src(&mut ut, "|_  a=@  +*  b  a  ++  c  b  --").expect("door");
    assert_eq!(tag_of(&ut, with_alias), "core");
    let without = play_src(&mut ut, "|_  a=@  ++  c  a  --").expect("door");
    assert!(!noun_is(ut.slab, with_alias, without));
    let (ty, _fol) =
        mint_with(&mut ut, "|_  a=@  +*  b  a  ++  c  b  --", true).expect("mint door");
    assert_eq!(tag_of(&ut, ty), "core");
}

// ---------------------------------------------------------------------------
// Exact hoon AST caches (enabled for prelude builds)
// ---------------------------------------------------------------------------

#[test]
fn exact_hoon_ast_caches_hit_and_evict() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    ut.exact_hoon_ast_lookup_enabled = true;
    let (ty, _fol) = mint_with(&mut ut, "|%  ++  x  5  ++  y  [1 x]  --", true).expect("core");
    assert_eq!(tag_of(&ut, ty), "core");

    let gen = parse("[1 2]");
    ut.cache_hoon_ast_for_node(&gen);
    ut.cache_hoon_ast_for_node(&gen);
    let noun = ut.hoon_noun_for_node(&gen);
    let cached = ut
        .hoon_cache_struct
        .values()
        .find_map(|bucket| bucket.back().map(|(noun, _)| *noun))
        .expect("a cached gene");
    assert!(ut.hoon_ast_lookup_cached(cached).is_some(), "raw hit");
    let copy = hoon_to_noun(&mut *ut.slab, &gen);
    assert!(ut.hoon_ast_lookup_cached(copy).is_some(), "structural hit");
    let unknown = hoon_to_noun(&mut *ut.slab, &parse("[3 4]"));
    assert!(ut.hoon_ast_lookup_cached(unknown).is_none(), "miss");
    assert!(ut.hoon_ast_lookup_result(noun).is_ok());

    // Decoded %hold genes are cached by raw noun and by AST pointer.
    let decoded = ut.decode_hold_hoon_ast(unknown).expect("decode");
    let again = ut.decode_hold_hoon_ast(unknown).expect("decode again");
    assert!(Arc::ptr_eq(&decoded, &again));
    assert!(ut.hoon_ast_lookup_cached(unknown).is_some(), "decoded hit");
    let back = ut.hoon_noun_for_node(decoded.as_ref());
    assert!(noun_is(ut.slab, back, unknown));

    // A decoded node maps back to its own noun, so caching it twice finds the
    // same raw noun in both the raw map and its structural bucket.
    ut.cache_hoon_ast_for_node(decoded.as_ref());
    ut.cache_hoon_ast_for_node(decoded.as_ref());

    // Nine structurally equal genes at distinct addresses overflow one bucket.
    for _ in 0..=Ut::HOON_CACHE_STRUCT_BUCKET_LIMIT {
        let twin = parse("[1 2]");
        ut.cache_hoon_ast_for_node(&twin);
    }

    // Twins fill the raw map eight times faster than the structural map, so a
    // lone gene can leave the raw map while its structural bucket survives.
    let lone = Hoon::Sand(
        "ud".to_string(),
        NounExpr::ParsedAtom(ParsedAtom::Small(424242)),
    );
    ut.cache_hoon_ast_for_node(&lone);
    let lone_fresh = hoon_to_noun(&mut *ut.slab, &lone);
    let space = ut.slab.noun_space();
    let lone_noun = ut
        .hoon_cache_struct
        .values()
        .flat_map(|bucket| bucket.iter())
        .map(|(noun, _)| *noun)
        .find(|noun| noun_eq(*noun, lone_fresh, &space).unwrap_or(false))
        .expect("lone gene cached");
    let groups = Ut::HOON_CACHE_RAW_KEY_LIMIT / Ut::HOON_CACHE_STRUCT_BUCKET_LIMIT + 1;
    let twins: Vec<Hoon> = (0..groups as u128)
        .flat_map(|g| {
            (0..Ut::HOON_CACHE_STRUCT_BUCKET_LIMIT).map(move |_| {
                Hoon::Rock("ux".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(g)))
            })
        })
        .collect();
    for twin in twins.iter() {
        ut.cache_hoon_ast_for_node(twin);
    }
    assert!(
        ut.hoon_ast_lookup_cached(lone_noun).is_some(),
        "bucket raw hit"
    );

    // Distinct genes past the key limits evict the oldest entries.
    let gens: Vec<Hoon> = (0..=(Ut::HOON_CACHE_RAW_KEY_LIMIT as u128 + 1))
        .map(|v| Hoon::Rock("ud".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(v))))
        .collect();
    for gen in gens.iter() {
        ut.cache_hoon_ast_for_node(gen);
    }
    for v in 0..=(Ut::HOON_CACHE_RAW_KEY_LIMIT as u128 + 1) {
        let gene = hoon_to_noun(
            &mut *ut.slab,
            &Hoon::Sand("ud".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(v))),
        );
        ut.decode_hold_hoon_ast(gene).expect("decode");
    }
    assert!(ut.hoon_cache_raw_order.len() <= Ut::HOON_CACHE_RAW_KEY_LIMIT);
    assert!(ut.decoded_hold_hoon_cache_order.len() <= Ut::HOON_CACHE_RAW_KEY_LIMIT);
}

#[test]
fn spec_and_open_caches_evict_past_their_limits() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let limit = Ut::SPEC_CACHE_KEY_LIMIT as u128 + 1;
    for v in 0..=limit {
        let spec = Spec::Leaf("ud".to_string(), ParsedAtom::Small(v));
        let example = ut.spec_example_cached(&spec);
        let again = ut.spec_example_cached(&spec);
        assert!(Arc::ptr_eq(&example, &again));
        let factory = ut.spec_factory_open_cached(&spec);
        let again = ut.spec_factory_open_cached(&spec);
        assert!(Arc::ptr_eq(&factory, &again));
    }
    assert!(ut.spec_example_cache_order.len() <= Ut::SPEC_CACHE_KEY_LIMIT);
    assert!(ut.spec_factory_open_cache_order.len() <= Ut::SPEC_CACHE_KEY_LIMIT);
    // The open cache is keyed by AST address, so keep every node alive.
    let gens: Vec<Hoon> = (0..=(Ut::HOON_CACHE_RAW_KEY_LIMIT as u64 + 1))
        .map(|v| Hoon::Axis(v.into()))
        .collect();
    for gen in gens.iter() {
        assert!(ut.open_cached(gen).is_some());
    }
    assert!(ut.open_cached(&gens[gens.len() - 1]).is_some());
    assert!(ut.open_cache_order.len() <= Ut::HOON_CACHE_RAW_KEY_LIMIT);
}

// ---------------------------------------------------------------------------
// Parity probes that need no prelude, minted directly
// ---------------------------------------------------------------------------

fn mint_probe_source(src: &str, dbug: bool) {
    let gen = crate::pipeline::parse_native_hoon_source_without_docs(
        FsPath::new("c2-probe.hoon"),
        src,
        Vec::new(),
        dbug,
    )
    .unwrap_or_else(|err| panic!("parse probe: {err:?}"));
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let sut = ty_noun(&mut *ut.slab);
    let gol = ty_noun(&mut *ut.slab);
    ut.set_vet(true);
    let (ty, _fol) = ut
        .mint_noun(sut, gol, &gen)
        .unwrap_or_else(|err| panic!("mint probe: {err:?}"));
    assert_eq!(tag_of(&ut, ty), "core");
}

#[test]
fn nest_probes_mint_with_and_without_spots() {
    // Fork goals whose losing options are tried first depend on member order,
    // which depends on the spots; minting both ways varies the order.
    for src in [
        include_str!("../../../../test-assets/type-probes/coverage/c2/c2_nest_cores.hoon"),
        include_str!("../../../../test-assets/type-probes/coverage/c2/c2_nest_lead.hoon"),
        include_str!("../../../../test-assets/type-probes/coverage/c2/c2_core_goals.hoon"),
    ] {
        mint_probe_source(src, false);
        mint_probe_source(src, true);
    }
}

// ---------------------------------------------------------------------------
// Rejections that the reject probes check against hoonc
// ---------------------------------------------------------------------------

/// Mints a reject probe (a `|%` file) against a `%noun` subject and returns
/// the error.
fn reject_probe_err(src: &str, dbug: bool) -> String {
    let gen = crate::pipeline::parse_native_hoon_source_without_docs(
        FsPath::new("c2-reject.hoon"),
        src,
        Vec::new(),
        dbug,
    )
    .unwrap_or_else(|err| panic!("parse probe: {err:?}"));
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let sut = ty_noun(&mut *ut.slab);
    let gol = ty_noun(&mut *ut.slab);
    ut.set_vet(true);
    match ut.mint_noun(sut, gol, &gen) {
        Ok(_) => panic!("probe should fail:\n{src}"),
        Err(err) => format!("{err:?}"),
    }
}

#[test]
fn fish_rejects_a_core_in_a_cell_head() {
    let err = reject_probe_err(
        include_str!("../../../../test-assets/type-probes/reject/c2_fish_core_head.hoon"),
        true,
    );
    assert!(err.contains("fish-core"), "{err}");
}

#[test]
fn arm_errors_leave_every_battery_subtree() {
    // The chapter and arm treaps put the failing arm under one-child and
    // two-child nodes on both sides (see the probe comments).
    for src in [
        include_str!("../../../../test-assets/type-probes/reject/c2_battery_left_chapter.hoon"),
        include_str!("../../../../test-assets/type-probes/reject/c2_battery_right_chapter.hoon"),
    ] {
        let err = reject_probe_err(src, true);
        assert!(err.contains("zzz"), "{err}");
    }
}

#[test]
fn nest_propagates_hold_and_arm_play_failures() {
    // Each mold holds a %hold or an arm that fails to play; nest reaches it
    // through a cell head and a reference fork, a fork option, a core's
    // context and payload on either side, gold variance, and the deep arm
    // comparison.
    for src in [
        include_str!("../../../../test-assets/type-probes/reject/c2_nest_cell_fork_hold.hoon"),
        include_str!("../../../../test-assets/type-probes/reject/c2_nest_fork_option_hold.hoon"),
        include_str!("../../../../test-assets/type-probes/reject/c2_nest_core_payload_hold.hoon"),
        include_str!(
            "../../../../test-assets/type-probes/reject/c2_nest_core_ref_payload_hold.hoon"
        ),
        include_str!("../../../../test-assets/type-probes/reject/c2_nest_core_context_hold.hoon"),
        include_str!("../../../../test-assets/type-probes/reject/c2_nest_deep_arm_hold.hoon"),
    ] {
        for dbug in [false, true] {
            let err = reject_probe_err(src, dbug);
            assert!(err.contains("zzz"), "{err}");
        }
    }
    // A hold that expands to itself answers no, so the cast fails.
    let err = reject_probe_err(
        include_str!("../../../../test-assets/type-probes/reject/c2_nest_hold_loop.hoon"),
        true,
    );
    assert!(err.contains("mint-nice"), "{err}");
}
