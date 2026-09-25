//! Coverage-driven tests for the desugarer (`open`, `+ax`, `+ap`, `+ah` ports).
//!
//! Added to close branch-coverage gaps; see the coverage report in the PR.
//! Each expected tree is transcribed from the matching hoon-138 arm
//! (`crates/hoonc/hoon/hoon-138.hoon`: `++ax` ~7505, `++ap` ~8177,
//! `++open` ~8333, `++ah` ~7440), so a failure here means the port no longer
//! produces what hoonc's desugarer produces.

use std::collections::HashMap;

use num_bigint::BigUint;

use crate::ast::hoon::*;
#[allow(unused_imports)]
use crate::utils::*;

// ---------------------------------------------------------------------------
// Small AST builders
// ---------------------------------------------------------------------------

fn b<T>(x: T) -> Box<T> {
    Box::new(x)
}

fn ax(n: u64) -> Hoon {
    Hoon::Axis(n.into())
}

fn limb(s: &str) -> Hoon {
    Hoon::Limb(s.to_string())
}

fn terms(ts: &[&str]) -> WingType {
    ts.iter().map(|t| Limb::Term(t.to_string())).collect()
}

fn axis_limb(n: u64) -> Limb {
    Limb::Axis(n.into())
}

fn small(n: u128) -> NounExpr {
    NounExpr::ParsedAtom(ParsedAtom::Small(n))
}

fn rock(aura: &str, n: u128) -> Hoon {
    Hoon::Rock(aura.to_string(), small(n))
}

fn sand(aura: &str, n: u128) -> Hoon {
    Hoon::Sand(aura.to_string(), small(n))
}

fn cord(s: &str) -> ParsedAtom {
    string_to_atom(s.to_string())
}

fn atom_spec(aura: &str) -> Spec {
    Spec::Base(BaseType::Atom(aura.to_string()))
}

fn noun_spec() -> Spec {
    Spec::Base(BaseType::NounExpr)
}

fn leaf_spec(n: u128) -> Spec {
    Spec::Leaf("tas".to_string(), ParsedAtom::Small(n))
}

fn term_skin(s: &str) -> Skin {
    Skin::Term(s.to_string())
}

fn spot() -> Spot {
    Spot {
        p: vec!["p1".to_string()],
        q: Pint {
            p: (1, 1),
            q: (1, 9),
        },
    }
}

fn help() -> NounExpr {
    small(7)
}

fn ex(spec: &Spec) -> Hoon {
    example(
        spec,
        1u64.into(),
        &vec![],
        &HashMap::new(),
        &vec![],
        &None,
        &None,
    )
}

fn fac(spec: Spec) -> Hoon {
    factory(
        spec,
        1u64.into(),
        vec![],
        HashMap::new(),
        vec![],
        None,
        None,
    )
}

fn rel(axe: u64, spec: &Spec) -> Hoon {
    relative(
        axe.into(),
        spec,
        1u64.into(),
        &vec![],
        &HashMap::new(),
        &vec![],
        &None,
        &None,
    )
}

fn sp(spec: Spec) -> Hoon {
    spore(
        spec,
        1u64.into(),
        vec![],
        HashMap::new(),
        vec![],
        None,
        None,
    )
}

/// `++spore` always casts its product with `[%ktls [%bust %noun] ...]`.
fn bust_noun(h: Hoon) -> Hoon {
    Hoon::KetLus(b(Hoon::Bust(BaseType::NounExpr)), b(h))
}

fn one_arm_core(arms: HashMap<String, Hoon>) -> Hoon {
    Hoon::BarCen(None, HashMap::from([("$".to_string(), (None, arms))]))
}

fn mean(h: Hoon) -> TermOrPair {
    TermOrPair::Pair("mean".to_string(), b(h))
}

fn tas_rock(s: &str) -> Hoon {
    Hoon::Rock("tas".to_string(), NounExpr::ParsedAtom(cord(s)))
}

fn buc_rock(s: &str) -> Hoon {
    Hoon::Rock("$".to_string(), NounExpr::ParsedAtom(cord(s)))
}

// ---------------------------------------------------------------------------
// Atom conversions
// ---------------------------------------------------------------------------

#[test]
fn ta_to_atom_maps_the_empty_knot_to_zero() {
    assert_eq!(ta_to_atom("~.".to_string()), ParsedAtom::Small(0));
    assert_eq!(ta_to_atom("ab".to_string()), cord("ab"));
}

#[test]
fn decimal_to_atom_goes_big_past_u128() {
    let two_128 = "340282366920938463463374607431768211456";
    assert_eq!(
        decimal_to_atom(two_128.to_string()),
        ParsedAtom::Big(BigUint::from(1u8) << 128usize)
    );
    assert_eq!(decimal_to_atom("42".to_string()), ParsedAtom::Small(42));
}

#[test]
#[should_panic(expected = "invalid hex in big atom")]
fn hex_to_atom_rejects_an_empty_digit_string() {
    // A short string that u128 rejects falls through to the BigUint path.
    hex_to_atom("0x".to_string());
}

#[test]
fn binary_to_atom_goes_big_past_u128() {
    let s = format!("1{}", "0".repeat(128));
    assert_eq!(
        binary_to_atom(s),
        ParsedAtom::from_biguint(BigUint::from(1u8) << 128usize)
    );
    assert_eq!(binary_to_atom("101".to_string()), ParsedAtom::Small(5));
}

#[test]
fn base64_and_base32_digits() {
    // 0w1a = 1*64 + 10 ; 0w~ = 63 ; 0v1v = 1*32 + 31
    assert_eq!(base64_to_atom("1a".to_string()), ParsedAtom::Small(74));
    assert_eq!(base64_to_atom("~".to_string()), ParsedAtom::Small(63));
    assert_eq!(base32_to_atom("1v".to_string()), ParsedAtom::Small(63));
}

#[test]
#[should_panic(expected = "invalid digit")]
fn base64_to_atom_panics_on_a_foreign_digit() {
    base64_to_atom("!".to_string());
}

#[test]
#[should_panic(expected = "invalid digit")]
fn base32_to_atom_panics_on_a_foreign_digit() {
    base32_to_atom("w".to_string());
}

#[test]
fn base58_ipv4_and_ipv6() {
    assert!(base58_to_atom("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa".to_string()).is_some());
    // '0' is not in the base58 alphabet.
    assert_eq!(base58_to_atom("10".to_string()), None);
    assert_eq!(
        ipv4_to_atom("127.0.0.1".to_string()),
        Some(ParsedAtom::Small(0x7f00_0001))
    );
    assert_eq!(ipv4_to_atom("1.2.3.999".to_string()), None);
    assert_eq!(
        ipv6_to_atom("0:0:0:0:0:0:0:1".to_string()),
        Some(ParsedAtom::Small(1))
    );
    assert_eq!(ipv6_to_atom("nope".to_string()), None);
}

// ---------------------------------------------------------------------------
// ++ax: basal, interface, spore, example, basic, relative, factory
// ---------------------------------------------------------------------------

#[test]
fn basal_void_and_null() {
    // [%void] -> [%zpzp ~] ; [%null] -> [%rock %n 0]
    assert_eq!(basal(BaseType::Void), Hoon::ZapZap);
    assert_eq!(basal(BaseType::Null), rock("n", 0));
    assert_eq!(ex(&Spec::Base(BaseType::Void)), Hoon::ZapZap);
}

fn core_arms() -> HashMap<String, Spec> {
    HashMap::from([("foo".to_string(), atom_spec("ud"))])
}

/// `++interface` for payload `*` and arms `{foo: @ud}`, before variance.
fn gold_interface() -> Hoon {
    Hoon::TisGar(
        b(ex(&noun_spec())),
        b(one_arm_core(HashMap::from([(
            "foo".to_string(),
            ex(&atom_spec("ud")),
        )]))),
    )
}

#[test]
fn interface_applies_variance() {
    let run = |v: Vair| {
        interface(
            v,
            noun_spec(),
            core_arms(),
            &noun_spec(),
            1u64.into(),
            &vec![],
            &HashMap::new(),
            &vec![],
            &None,
            &None,
        )
    };
    assert_eq!(run(Vair::Gold), gold_interface());
    assert_eq!(run(Vair::Iron), Hoon::KetBar(b(gold_interface())));
    assert_eq!(run(Vair::Lead), Hoon::KetWut(b(gold_interface())));
    assert_eq!(run(Vair::Zinc), Hoon::KetPam(b(gold_interface())));
}

#[test]
fn example_of_core_specs_builds_interfaces() {
    // [%bcdt *] (decorate (home (interface %gold p.mod q.mod))), and so on.
    let payload = || b(noun_spec());
    assert_eq!(ex(&Spec::BucDot(payload(), core_arms())), gold_interface());
    assert_eq!(
        ex(&Spec::BucFas(payload(), core_arms())),
        Hoon::KetBar(b(gold_interface()))
    );
    assert_eq!(
        ex(&Spec::BucZap(payload(), core_arms())),
        Hoon::KetWut(b(gold_interface()))
    );
    assert_eq!(
        ex(&Spec::BucTic(payload(), core_arms())),
        Hoon::KetPam(b(gold_interface()))
    );

    // home and decorate wrap the interface when dom/hay/bug/nut are set.
    let homed = example(
        &Spec::BucDot(payload(), core_arms()),
        2u64.into(),
        &terms(&["h"]),
        &HashMap::new(),
        &vec![spot()],
        &Some(Note::Know("k".to_string())),
        &None,
    );
    assert_eq!(
        homed,
        Hoon::Note(
            Note::Know("k".to_string()),
            b(Hoon::Dbug(
                spot(),
                b(Hoon::TisGar(
                    b(Hoon::Wing(vec![Limb::Term("h".to_string()), axis_limb(2)])),
                    b(gold_interface()),
                )),
            )),
        )
    );
}

#[test]
fn relative_of_core_specs_builds_interfaces() {
    let payload = || b(noun_spec());
    assert_eq!(
        rel(6, &Spec::BucDot(payload(), core_arms())),
        gold_interface()
    );
    assert_eq!(
        rel(6, &Spec::BucFas(payload(), core_arms())),
        Hoon::KetBar(b(gold_interface()))
    );
    assert_eq!(
        rel(6, &Spec::BucZap(payload(), core_arms())),
        Hoon::KetWut(b(gold_interface()))
    );
    assert_eq!(
        rel(6, &Spec::BucTic(payload(), core_arms())),
        Hoon::KetPam(b(gold_interface()))
    );
}

#[test]
fn spore_uses_the_default_when_set() {
    // ?^  def  u.def   (still homed and cast to noun)
    let got = spore(
        atom_spec("ud"),
        1u64.into(),
        terms(&["h"]),
        HashMap::new(),
        vec![],
        None,
        Some(rock("ud", 5)),
    );
    assert_eq!(
        got,
        bust_noun(Hoon::TisGar(b(Hoon::Wing(terms(&["h"]))), b(rock("ud", 5))))
    );
}

#[test]
fn spore_recursion_cases() {
    // [%base *]  ?:(=(%void p.mod) [%rock %n 0] (basal p.mod))
    assert_eq!(sp(Spec::Base(BaseType::Void)), bust_noun(rock("n", 0)));

    // [%bcbc *] merges q.mod into cox; [%loop *] looks the name up.
    let bcbc = Spec::BucBuc(
        b(Spec::BucCol(
            b(leaf_spec(1)),
            vec![Spec::Loop("x".to_string())],
        )),
        HashMap::from([("x".to_string(), atom_spec("ud"))]),
    );
    assert_eq!(
        sp(bcbc),
        bust_noun(Hoon::Pair(b(rock("tas", 1)), b(sand("ud", 0))))
    );

    // [%made *] [%over *] [%bcpm *] -> recurse
    let made = Spec::Made(("m".to_string(), vec!["a".to_string()]), b(atom_spec("ud")));
    assert_eq!(sp(made), bust_noun(sand("ud", 0)));
    let over = Spec::Over(terms(&["w"]), b(Spec::BucMic(limb("g"))));
    assert_eq!(sp(over), bust_noun(Hoon::TisGal(b(ax(6)), b(limb("g")))));
    assert_eq!(
        sp(Spec::BucPam(b(atom_spec("ud")), limb("fix"))),
        bust_noun(sand("ud", 0))
    );

    // [%bcgl *] [%bcgr *] [%bckt *]  $(mod q.mod)
    for spec in [
        Spec::BucGal(b(leaf_spec(1)), b(atom_spec("ux"))),
        Spec::BucGar(b(leaf_spec(1)), b(atom_spec("ux"))),
        Spec::BucKet(b(leaf_spec(1)), b(atom_spec("ux"))),
    ] {
        assert_eq!(sp(spec), bust_noun(sand("ux", 0)));
    }

    // [%bcdt *] [%bcfs *] [%bctc *] [%bczp *]  [%rock %n 0]
    for spec in [
        Spec::BucDot(b(noun_spec()), core_arms()),
        Spec::BucFas(b(noun_spec()), core_arms()),
        Spec::BucTic(b(noun_spec()), core_arms()),
        Spec::BucZap(b(noun_spec()), core_arms()),
    ] {
        assert_eq!(sp(spec), bust_noun(rock("n", 0)));
    }
}

#[test]
fn example_annotation_cases() {
    // [%loop *]  [%limb p.mod]
    assert_eq!(ex(&Spec::Loop("x".to_string())), limb("x"));
    // [%made *]  example(mod q.mod, nut `made/[p.p.mod `(pieces q.p.mod)])
    assert_eq!(
        ex(&Spec::Made(
            ("m".to_string(), vec!["a".to_string(), "b".to_string()]),
            b(atom_spec("ud")),
        )),
        Hoon::Note(
            Note::Made("m".to_string(), Some(vec![terms(&["a"]), terms(&["b"])])),
            b(sand("ud", 0)),
        )
    );
    // [%name *]  example(mod q.mod, nut `made/[p.mod ~])
    assert_eq!(
        ex(&Spec::Name("n".to_string(), b(atom_spec("ud")))),
        Hoon::Note(Note::Made("n".to_string(), None), b(sand("ud", 0)))
    );
    // [%over *]  example(hay p.mod, mod q.mod): the new hay reaches +home.
    assert_eq!(
        ex(&Spec::Over(terms(&["w"]), b(Spec::BucMic(limb("g"))))),
        Hoon::TisGar(
            b(Hoon::Wing(terms(&["w"]))),
            b(Hoon::TisGal(b(limb("$")), b(limb("g")))),
        )
    );
    // [%bcls *]  (decorate [%note [%know p.mod] example(mod q.mod)])
    assert_eq!(
        ex(&Spec::BucLus("k".to_string(), b(atom_spec("ud")))),
        Hoon::Note(Note::Know("k".to_string()), b(sand("ud", 0)))
    );
}

#[test]
fn basic_cell_and_void() {
    // %cell: [%ktls example [%wing [[%& 2] fetch-wing]] [%wing [[%& 3] fetch-wing]]]
    assert_eq!(
        rel(6, &Spec::Base(BaseType::Cell)),
        Hoon::KetLus(
            b(ex(&Spec::Base(BaseType::Cell))),
            b(Hoon::Pair(
                b(Hoon::Wing(vec![axis_limb(2), axis_limb(6)])),
                b(Hoon::Wing(vec![axis_limb(3), axis_limb(6)])),
            )),
        )
    );
    // %void: [%zpzp ~]
    assert_eq!(rel(6, &Spec::Base(BaseType::Void)), Hoon::ZapZap);
}

#[test]
fn relative_annotation_cases() {
    // [%loop *]  (decorate [%cnhp [%limb p.mod] fetch])
    assert_eq!(
        rel(6, &Spec::Loop("x".to_string())),
        Hoon::CenHep(b(limb("x")), b(ax(6)))
    );
    // [%made *]  relative(mod q.mod, nut `made/[...])
    assert_eq!(
        rel(
            6,
            &Spec::Made(("m".to_string(), vec!["a".to_string()]), b(noun_spec()))
        ),
        Hoon::Note(
            Note::Made("m".to_string(), Some(vec![terms(&["a"])])),
            b(ax(6))
        )
    );
    // [%over *]  relative(hay p.mod, mod q.mod)
    assert_eq!(
        rel(6, &Spec::Over(terms(&["w"]), b(Spec::BucMic(limb("g"))))),
        Hoon::CenCol(
            b(Hoon::TisGar(b(Hoon::Wing(terms(&["w"]))), b(limb("g")))),
            vec![ax(6)],
        )
    );
}

#[test]
fn relative_bucbuc_descends_into_axis_three() {
    // [%bcbc *] [%brkt relative(mod p.mod, dom (peg 3 dom)) [[%$ ~ arms] ~ ~]]
    let spec = Spec::BucBuc(
        b(Spec::BucMic(limb("g"))),
        HashMap::from([("x".to_string(), Spec::BucMic(limb("h")))]),
    );
    let at3 = |h: &str| {
        Hoon::CenCol(
            b(Hoon::TisGar(b(Hoon::Wing(vec![axis_limb(3)])), b(limb(h)))),
            vec![ax(6)],
        )
    };
    assert_eq!(
        rel(6, &spec),
        Hoon::BarKet(
            b(at3("g")),
            HashMap::from([(
                "$".to_string(),
                (None, HashMap::from([("x".to_string(), at3("h"))]))
            )]),
        )
    );
}

#[test]
fn relative_bucpam_repairs_and_checks() {
    // [%bcpm *] =+ raw =+ =>(+3 q) =+ (+2 +6) ?>(|(=(+14 +2) =(+2 (+6 +2))) +2)
    let expected = Hoon::TisLus(
        b(ax(6)),
        b(Hoon::TisLus(
            b(Hoon::TisGar(b(ax(3)), b(limb("fix")))),
            b(Hoon::TisLus(
                b(Hoon::CenHep(b(ax(2)), b(ax(6)))),
                b(Hoon::WutGar(
                    b(Hoon::WutBar(vec![
                        Hoon::DotTis(b(ax(14)), b(ax(2))),
                        Hoon::DotTis(b(ax(2)), b(Hoon::CenHep(b(ax(6)), b(ax(2))))),
                    ])),
                    b(ax(2)),
                )),
            )),
        )),
    );
    assert_eq!(rel(6, &Spec::BucPam(b(noun_spec()), limb("fix"))), expected);
}

#[test]
fn relative_bucgal_and_bucgar_filter() {
    // [%bcgl *] [%tsls relative:clear(mod q.mod)
    //             [%wtgl [%wtts [%over ~[&/3] p.mod] ~[&/4]] $/2]]
    let test = Hoon::WutTis(
        b(Spec::Over(vec![axis_limb(3)], b(leaf_spec(1)))),
        vec![axis_limb(4)],
    );
    assert_eq!(
        rel(6, &Spec::BucGal(b(leaf_spec(1)), b(noun_spec()))),
        Hoon::TisLus(b(ax(6)), b(Hoon::WutGal(b(test.clone()), b(ax(2)))))
    );
    assert_eq!(
        rel(6, &Spec::BucGar(b(leaf_spec(1)), b(noun_spec()))),
        Hoon::TisLus(b(ax(6)), b(Hoon::WutGar(b(test), b(ax(2)))))
    );
}

#[test]
fn relative_buchep_casts_to_the_default() {
    // [%bchp *] =/ fun (function:clear p.mod q.mod) ?^ def [%ktls fun u.def] fun
    let bchp = Spec::BucHep(b(noun_spec()), b(noun_spec()));
    let fun = Hoon::TisGar(
        b(Hoon::Pair(b(ex(&noun_spec())), b(ex(&noun_spec())))),
        b(Hoon::KetBar(b(Hoon::BarCol(b(ax(2)), b(ax(15)))))),
    );
    assert_eq!(rel(6, &bchp), fun);
    // [%bcsg *] relative(mod q.mod, def `[%kthp q.mod p.mod])
    let dflt = rock("ud", 5);
    assert_eq!(
        rel(6, &Spec::BucSig(dflt.clone(), b(bchp.clone()))),
        Hoon::KetLus(b(fun), b(Hoon::KetHep(b(bchp), b(dflt))))
    );
}

#[test]
fn factory_bucsig_sets_the_default() {
    // ?:  ?=(%bcsg -.mod)  factory(mod q.mod, def `[%kthp q.mod p.mod])
    let dflt = rock("ud", 5);
    let got = fac(Spec::BucSig(dflt.clone(), b(atom_spec("ud"))));
    let Hoon::BarCol(sample, _) = got else {
        panic!("factory should build a gate, got {got:?}");
    };
    assert_eq!(
        *sample,
        Hoon::KetSig(b(bust_noun(Hoon::KetHep(b(atom_spec("ud")), b(dflt)))))
    );
}

#[test]
fn factory_short_circuits_indirections_without_default() {
    // ?:  &(=(~ def) ?=(?(%bcmc %like %loop %make) -.mod))  (decorate (home ...))
    assert_eq!(fac(Spec::BucMic(limb("g"))), limb("g"));
    assert_eq!(fac(Spec::Loop("x".to_string())), limb("x"));
    // With a default, %bcmc still builds a normalizing gate.
    let gated = factory(
        Spec::BucMic(limb("g")),
        1u64.into(),
        vec![],
        HashMap::new(),
        vec![],
        None,
        Some(rock("ud", 5)),
    );
    assert!(matches!(gated, Hoon::BarCol(_, _)), "got {gated:?}");
}

#[test]
fn unreel_and_unfold() {
    assert_eq!(unreel(terms(&["a"]), vec![]), Hoon::Wing(terms(&["a"])));
    assert_eq!(
        unreel(terms(&["a"]), vec![terms(&["b"])]),
        Hoon::TisGal(b(Hoon::Wing(terms(&["a"]))), b(Hoon::Wing(terms(&["b"]))))
    );
    assert_eq!(
        unfold(limb("f"), vec![noun_spec()]),
        Hoon::CenCol(b(limb("f")), vec![Hoon::KetCol(b(noun_spec()))])
    );
}

// ---------------------------------------------------------------------------
// ++open: one rewrite step per rune
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "boom")]
fn open_eror_crashes_with_its_tape() {
    open(Hoon::Eror("boom".to_string()));
}

fn knit_head() -> Hoon {
    // [%brhp %wtcl [%bust %flag] [%bust %null] [i=~~ t=$]]
    Hoon::BarHep(b(Hoon::WutCol(
        b(Hoon::Bust(BaseType::Flag)),
        b(Hoon::Bust(BaseType::Null)),
        b(Hoon::Pair(
            b(Hoon::KetTis(term_skin("i"), b(sand("tD", 0)))),
            b(Hoon::KetTis(term_skin("t"), b(limb("$")))),
        )),
    )))
}

fn knit_of(body: Hoon) -> Hoon {
    Hoon::TisGar(
        b(Hoon::KetTis(term_skin("v"), b(ax(1)))),
        b(Hoon::BarHep(b(Hoon::KetLus(b(knit_head()), b(body))))),
    )
}

#[test]
fn open_knit_interpolates_hoon_pieces() {
    let piece = |p: Hoon, res: Hoon| {
        Hoon::TisLus(
            b(Hoon::Pair(
                b(Hoon::KetTis(
                    term_skin("a"),
                    b(Hoon::KetLus(
                        b(limb("$")),
                        b(Hoon::TisGar(b(limb("v")), b(p))),
                    )),
                )),
                b(Hoon::KetTis(term_skin("b"), b(res))),
            )),
            b(Hoon::BarHep(b(Hoon::WutPat(
                terms(&["a"]),
                b(limb("b")),
                b(Hoon::Pair(
                    b(Hoon::TisGal(b(ax(2)), b(limb("a")))),
                    b(Hoon::CenTis(
                        terms(&["$"]),
                        vec![(terms(&["a"]), Hoon::TisGal(b(ax(3)), b(limb("a"))))],
                    )),
                )),
            )))),
        )
    };
    let got = open(Hoon::Knit(vec![
        Woof::ParsedAtom(ParsedAtom::Small(0x61)),
        Woof::Hoon(limb("x")),
    ]));
    let expected = knit_of(Hoon::Pair(
        b(sand("tD", 0x61)),
        b(piece(limb("x"), Hoon::Bust(BaseType::Null))),
    ));
    assert_eq!(got, expected);
}

#[test]
fn open_simple_forwarders() {
    // [%leaf *] ~(factory ax gen) ; [%note *] q.gen
    assert_eq!(
        open(Hoon::Leaf("tas".to_string(), ParsedAtom::Small(5))),
        fac(leaf_spec(5))
    );
    assert_eq!(
        open(Hoon::Note(Note::Know("k".to_string()), b(ax(1)))),
        ax(1)
    );
    // [%tell *] [%cncl [%limb %noah] [%zpgr [%cltr p.gen]] ~] ; %yell with %cain
    assert_eq!(
        open(Hoon::Tell(vec![limb("a")])),
        Hoon::CenCol(
            b(limb("noah")),
            vec![Hoon::ZapGar(b(Hoon::ColTar(vec![limb("a")])))]
        )
    );
    assert_eq!(
        open(Hoon::Yell(vec![limb("a")])),
        Hoon::CenCol(
            b(limb("cain")),
            vec![Hoon::ZapGar(b(Hoon::ColTar(vec![limb("a")])))]
        )
    );
}

#[test]
#[should_panic(expected = "empty sample in BarBuc")]
fn open_barbuc_rejects_an_empty_sample() {
    open(Hoon::BarBuc(vec![], b(noun_spec())));
}

#[test]
fn open_barbuc_keeps_gist_notes_outside_the_ktcl() {
    // [%brbc *] [%brtr [%bccl samples]
    //             ?.(?=([%gist *] body) [%ktcl body] [%note p.body $(body q.body)])]
    let tar = noun_spec();
    let bcsg = Spec::BucSig(
        Hoon::Base(BaseType::NounExpr),
        b(Spec::BucHep(b(tar.clone()), b(tar))),
    );
    let got = open(Hoon::BarBuc(
        vec!["a".to_string()],
        b(Spec::Gist(help(), b(noun_spec()))),
    ));
    assert_eq!(
        got,
        Hoon::BarTar(
            b(Spec::BucCol(
                b(Spec::BucTis(term_skin("a"), b(bcsg))),
                vec![]
            )),
            b(Hoon::Note(
                Note::Help(help()),
                b(Hoon::KetCol(b(noun_spec())))
            )),
        )
    );
}

#[test]
fn open_barcab_wraps_arms_in_alas() {
    // [%brcb *] [%tsls [%kttr p] [%brcn ~ (arms under [%tstr [a ~] v ...] per alias)]]
    let arms = HashMap::from([(
        "$".to_string(),
        (None, HashMap::from([("foo".to_string(), limb("body"))])),
    )]);
    let got = open(Hoon::BarCab(
        b(noun_spec()),
        vec![("x".to_string(), ax(2)), ("y".to_string(), ax(3))],
        arms,
    ));
    let wrapped = Hoon::TisTar(
        ("x".to_string(), None),
        b(ax(2)),
        b(Hoon::TisTar(
            ("y".to_string(), None),
            b(ax(3)),
            b(limb("body")),
        )),
    );
    assert_eq!(
        got,
        Hoon::TisLus(
            b(Hoon::KetTar(b(noun_spec()))),
            b(one_arm_core(HashMap::from([("foo".to_string(), wrapped)]))),
        )
    );
}

#[test]
fn open_barket_with_and_without_a_buc_chapter() {
    // [%brkt *] [%tsgl [%limb %$] [%brcn ~ (put q.gen %$ ...)]]
    let with_buc = open(Hoon::BarKet(
        b(limb("p")),
        HashMap::from([(
            "$".to_string(),
            (None, HashMap::from([("foo".to_string(), limb("x"))])),
        )]),
    ));
    assert_eq!(
        with_buc,
        Hoon::TisGal(
            b(limb("$")),
            b(one_arm_core(HashMap::from([
                ("foo".to_string(), limb("x")),
                ("$".to_string(), limb("p")),
            ]))),
        )
    );
    let chapter_only = open(Hoon::BarKet(
        b(limb("p")),
        HashMap::from([(
            "chap".to_string(),
            (None, HashMap::from([("foo".to_string(), limb("x"))])),
        )]),
    ));
    assert_eq!(
        chapter_only,
        Hoon::TisGal(
            b(limb("$")),
            b(Hoon::BarCen(
                None,
                HashMap::from([
                    (
                        "chap".to_string(),
                        (None, HashMap::from([("foo".to_string(), limb("x"))])),
                    ),
                    (
                        "$".to_string(),
                        (None, HashMap::from([("$".to_string(), limb("p"))])),
                    ),
                ]),
            )),
        )
    );
}

#[test]
fn open_bar_and_col_runes() {
    // [%brsg *] [%ktbr [%brts p q]] ; [%brwt *] [%ktwt %brdt p]
    assert_eq!(
        open(Hoon::BarSig(b(noun_spec()), b(ax(1)))),
        Hoon::KetBar(b(Hoon::BarTis(b(noun_spec()), b(ax(1)))))
    );
    assert_eq!(
        open(Hoon::BarWut(b(ax(1)))),
        Hoon::KetWut(b(Hoon::BarDot(b(ax(1)))))
    );
    let (p, q, r, s) = (ax(2), ax(3), ax(4), ax(5));
    // [%clkt *] [p q r s] ; [%clcb *] [q p] ; [%clhp *] [p q] ; [%clls *] [p q r]
    assert_eq!(
        open(Hoon::ColKet(
            b(p.clone()),
            b(q.clone()),
            b(r.clone()),
            b(s.clone())
        )),
        Hoon::Pair(
            b(p.clone()),
            b(Hoon::Pair(b(q.clone()), b(Hoon::Pair(b(r.clone()), b(s)))))
        )
    );
    assert_eq!(
        open(Hoon::ColCab(b(p.clone()), b(q.clone()))),
        Hoon::Pair(b(q.clone()), b(p.clone()))
    );
    assert_eq!(
        open(Hoon::ColHep(b(p.clone()), b(q.clone()))),
        Hoon::Pair(b(p.clone()), b(q.clone()))
    );
    assert_eq!(
        open(Hoon::ColLus(b(p.clone()), b(q.clone()), b(r.clone()))),
        Hoon::Pair(b(p.clone()), b(Hoon::Pair(b(q.clone()), b(r))))
    );
    // [%clsg *] ?~(p.gen [%rock %n ~] [i.p.gen $(p.gen t.p.gen)])
    assert_eq!(open(Hoon::ColSig(vec![])), rock("n", 0));
    assert_eq!(
        open(Hoon::ColSig(vec![p.clone(), q.clone()])),
        Hoon::Pair(b(p), b(Hoon::Pair(b(q), b(rock("n", 0)))))
    );
}

#[test]
fn open_cen_runes() {
    let w = terms(&["w"]);
    let pairs = vec![(terms(&["a"]), ax(1))];
    // [%cncb *] [%ktls [%wing p.gen] %cnts p.gen q.gen]
    assert_eq!(
        open(Hoon::CenCab(w.clone(), pairs.clone())),
        Hoon::KetLus(b(Hoon::Wing(w.clone())), b(Hoon::CenTis(w, pairs)))
    );
    // [%cndt *] [%cncl q.gen [p.gen ~]] ; %cnkt ; %cnls
    assert_eq!(
        open(Hoon::CenDot(b(ax(2)), b(limb("f")))),
        Hoon::CenCol(b(limb("f")), vec![ax(2)])
    );
    assert_eq!(
        open(Hoon::CenKet(b(limb("f")), b(ax(2)), b(ax(3)), b(ax(4)))),
        Hoon::CenCol(b(limb("f")), vec![ax(2), ax(3), ax(4)])
    );
    assert_eq!(
        open(Hoon::CenLus(b(limb("f")), b(ax(2)), b(ax(3)))),
        Hoon::CenCol(b(limb("f")), vec![ax(2), ax(3)])
    );
}

#[test]
fn open_ktdt_casts_to_the_call() {
    // [%ktdt *] [%ktls [%cncl p.gen q.gen ~] q.gen]
    assert_eq!(
        open(Hoon::KetDot(b(limb("f")), b(ax(2)))),
        Hoon::KetLus(b(Hoon::CenCol(b(limb("f")), vec![ax(2)])), b(ax(2)))
    );
}

#[test]
fn open_sgbr_uses_feck_for_tas_constants() {
    // [%sgbr *] [%sggr [%mean ?^(fek [%rock %tas u.fek] [%brdt ...])] q.gen]
    let tas = Hoon::Dbug(
        spot(),
        b(Hoon::Sand(
            "tas".to_string(),
            NounExpr::ParsedAtom(cord("foo")),
        )),
    );
    assert_eq!(
        open(Hoon::SigBar(b(tas), b(ax(1)))),
        Hoon::SigGar(mean(tas_rock("foo")), b(ax(1)))
    );
    assert_eq!(
        open(Hoon::SigBar(b(limb("x")), b(ax(1)))),
        Hoon::SigGar(
            mean(Hoon::BarDot(b(Hoon::CenCol(
                b(limb("cain")),
                vec![Hoon::ZapGar(b(Hoon::TisGar(b(ax(3)), b(limb("x")))))],
            )))),
            b(ax(1)),
        )
    );
}

#[test]
fn open_sig_hint_runes() {
    // [%sgcb *] [%sggr [%mean [%brdt p.gen]] q.gen]
    assert_eq!(
        open(Hoon::SigCab(b(limb("x")), b(ax(1)))),
        Hoon::SigGar(mean(Hoon::BarDot(b(limb("x")))), b(ax(1)))
    );
    // [%sgbc *] [%sggr [%live [%rock %$ p.gen]] q.gen]
    assert_eq!(
        open(Hoon::SigBuc("foo".to_string(), b(ax(1)))),
        Hoon::SigGar(
            TermOrPair::Pair("live".to_string(), b(buc_rock("foo"))),
            b(ax(1)),
        )
    );
    // [%sgls *] [%sggr [%memo %rock %$ p.gen] q.gen]
    assert_eq!(
        open(Hoon::SigLus(3, b(ax(1)))),
        Hoon::SigGar(
            TermOrPair::Pair("memo".to_string(), b(rock("$", 3))),
            b(ax(1))
        )
    );
    // [%sgts *] [%sggr [%germ p.gen] q.gen]
    assert_eq!(
        open(Hoon::SigTis(b(limb("x")), b(ax(1)))),
        Hoon::SigGar(TermOrPair::Pair("germ".to_string(), b(limb("x"))), b(ax(1)))
    );
}

fn sgcn_with_hooks() -> Hoon {
    Hoon::SigCen(
        Chum::Lef("foo".to_string()),
        b(limb("parent")),
        vec![("bar".to_string(), limb("hook"))],
        b(limb("body")),
    )
}

#[test]
fn open_sgcn_lists_its_hooks() {
    // [%sgcn *] [%sggl [%fast %clls [%rock %$ p] [%zpts q] %clsg hooks] s.gen]
    let Hoon::SigGal(TermOrPair::Pair(name, clls), body) = open(sgcn_with_hooks()) else {
        panic!("%sgcn should open to %sggl");
    };
    assert_eq!(name, "fast");
    assert_eq!(*body, limb("body"));
    let Hoon::ColLus(chum, _parent, hooks) = *clls else {
        panic!("fast hint should be a :+ triple");
    };
    assert_eq!(*chum, buc_rock("foo"));
    assert_eq!(
        *hooks,
        Hoon::ColSig(vec![Hoon::Pair(
            b(buc_rock("bar")),
            b(Hoon::ZapTis(b(limb("hook")))),
        )])
    );
}

#[test]
#[ignore = "hatch open(%sgcn) puts the body, not the parent, under %zpts (utils.rs ~2138); \
            latent: honk lowers %sgcn itself in mint, and play/mull ignore hints"]
fn open_sgcn_puts_the_parent_under_zpts() {
    let Hoon::SigGal(TermOrPair::Pair(_, clls), _) = open(sgcn_with_hooks()) else {
        panic!("%sgcn should open to %sggl");
    };
    let Hoon::ColLus(_, parent, _) = *clls else {
        panic!("fast hint should be a :+ triple");
    };
    assert_eq!(*parent, Hoon::ZapTis(b(limb("parent"))));
}

#[test]
#[ignore = "hatch open(%tsbr) builds [%tsls [%kttr p] q], but hoon-138 is \
            [%tsls ~(example ax p) q] (no %ktsg fold; utils.rs ~2437); latent: honk \
            lowers %tsbr itself in mint and play, and mull only needs the type"]
fn open_tsbr_pushes_the_bare_example() {
    assert_eq!(
        open(Hoon::TisBar(b(atom_spec("ud")), b(ax(1)))),
        Hoon::TisLus(b(ex(&atom_spec("ud"))), b(ax(1)))
    );
}

#[test]
fn chum_to_nounexpr_shapes() {
    let a = |s: &str| NounExpr::ParsedAtom(cord(s));
    assert_eq!(
        chum_to_nounexpr(Chum::StdKel("k".to_string(), ParsedAtom::Small(138))),
        NounExpr::Cell(b(a("k")), b(small(138)))
    );
    assert_eq!(
        chum_to_nounexpr(Chum::VenProKel(
            "v".to_string(),
            "p".to_string(),
            ParsedAtom::Small(1)
        )),
        NounExpr::Cell(b(a("v")), b(NounExpr::Cell(b(a("p")), b(small(1)))))
    );
    assert_eq!(
        chum_to_nounexpr(Chum::VenProVerKel(
            "v".to_string(),
            "p".to_string(),
            ParsedAtom::Small(2),
            ParsedAtom::Small(1)
        )),
        NounExpr::Cell(
            b(a("v")),
            b(NounExpr::Cell(
                b(a("p")),
                b(NounExpr::Cell(b(small(2)), b(small(1))))
            ))
        )
    );
}

fn marl_gate() -> Hoon {
    let sug = vec![axis_limb(12)];
    let wtsg = Hoon::WutSig(
        sug.clone(),
        b(Hoon::CenTis(
            sug.clone(),
            vec![(vec![axis_limb(1)], ax(13))],
        )),
        b(Hoon::CenTis(
            sug.clone(),
            vec![(
                vec![axis_limb(3)],
                Hoon::CenTis(terms(&["$"]), vec![(sug, ax(25))]),
            )],
        )),
    );
    Hoon::TisBar(
        b(Spec::Base(BaseType::Cell)),
        b(Hoon::BarPat(
            None,
            HashMap::from([(
                "$".to_string(),
                (None, HashMap::from([("$".to_string(), wtsg)])),
            )]),
        )),
    )
}

#[test]
fn open_mcts_tuna_modes() {
    let null = Hoon::Bust(BaseType::Null);
    let manx = Manx {
        g: Marx {
            n: Mane::Tag("p".to_string()),
            a: vec![],
        },
        c: vec![],
    };
    // ^  [[%xray i] $]
    assert_eq!(
        open(Hoon::MicTis(vec![Tuna::Manx(manx.clone())])),
        Hoon::Pair(b(Hoon::Xray(manx)), b(null.clone()))
    );
    // %manx [p.i $]
    assert_eq!(
        open(Hoon::MicTis(vec![Tuna::TunaTail(TunaTail::Manx(limb(
            "m"
        )))])),
        Hoon::Pair(b(limb("m")), b(null.clone()))
    );
    // %tape [[%mcfs p.i] $]
    assert_eq!(
        open(Hoon::MicTis(vec![Tuna::TunaTail(TunaTail::Tape(limb(
            "t"
        )))])),
        Hoon::Pair(b(Hoon::MicFas(b(limb("t")))), b(null.clone()))
    );
    // %call [%cncl p.i [$]~]
    assert_eq!(
        open(Hoon::MicTis(vec![Tuna::TunaTail(TunaTail::Call(limb(
            "g"
        )))])),
        Hoon::CenCol(b(limb("g")), vec![null.clone()])
    );
    // %marl [%cndt [p.i $] [%tsbr [%base %cell] [%brpt ...]]]
    assert_eq!(
        open(Hoon::MicTis(vec![Tuna::TunaTail(TunaTail::Marl(limb(
            "s"
        )))])),
        Hoon::CenDot(b(Hoon::Pair(b(limb("s")), b(null))), b(marl_gate()))
    );
}

#[test]
fn open_mic_runes() {
    // [%mccl *] ~ -> [%zpzp ~] ; [* ~] -> i.q.gen
    assert_eq!(open(Hoon::MicCol(b(limb("f")), vec![])), Hoon::ZapZap);
    assert_eq!(open(Hoon::MicCol(b(limb("f")), vec![ax(2)])), ax(2));
    // [%mcfs *] =+(zoy=[%rock %ta %$] [%clsg [zoy [%clsg [zoy p.gen] ~]] ~])
    let zoy = rock("ta", 0);
    assert_eq!(
        open(Hoon::MicFas(b(limb("t")))),
        Hoon::ColSig(vec![Hoon::Pair(
            b(zoy.clone()),
            b(Hoon::ColSig(vec![Hoon::Pair(b(zoy), b(limb("t")))])),
        )])
    );
    // [%mcgl *] [%cnls [%cnhp q ktcl+p] r [%brts p [%tsgr $+3 s]]]
    let spec = atom_spec("ud");
    assert_eq!(
        open(Hoon::MicGal(
            b(spec.clone()),
            b(limb("bind")),
            b(ax(2)),
            b(limb("s"))
        )),
        Hoon::CenLus(
            b(Hoon::CenHep(
                b(limb("bind")),
                b(Hoon::KetCol(b(spec.clone())))
            )),
            b(ax(2)),
            b(Hoon::BarTis(
                b(spec),
                b(Hoon::TisGar(b(ax(3)), b(limb("s"))))
            )),
        )
    );
    // [%mcmc *] [%cnhp ~(factory ax p.gen) q.gen]
    assert_eq!(
        open(Hoon::MicMic(b(atom_spec("ud")), b(ax(2)))),
        Hoon::CenHep(b(fac(atom_spec("ud"))), b(ax(2)))
    );
}

fn v_bind() -> Hoon {
    Hoon::KetTis(term_skin("v"), b(ax(1)))
}

#[test]
fn open_mcsg_single_and_chained() {
    // [* ~]: [%tsgr [%ktts %v %$ 1] [%tsgr [%limb %v] i.q.gen]]
    assert_eq!(
        open(Hoon::MicSig(b(limb("p")), vec![limb("x")])),
        Hoon::TisGar(b(v_bind()), b(Hoon::TisGar(b(limb("v")), b(limb("x")))))
    );
    let c_wing = vec![Limb::Parent(0, None), axis_limb(6)];
    let chained = Hoon::TisLus(
        b(Hoon::KetTis(
            term_skin("a"),
            b(Hoon::TisGar(b(limb("v")), b(limb("y")))),
        )),
        b(Hoon::TisLus(
            b(Hoon::KetTis(
                term_skin("b"),
                b(Hoon::TisGar(b(limb("v")), b(limb("x")))),
            )),
            b(Hoon::TisLus(
                b(Hoon::KetTis(
                    term_skin("c"),
                    b(Hoon::TisGal(b(Hoon::Wing(c_wing.clone())), b(limb("b")))),
                )),
                b(Hoon::BarDot(b(Hoon::CenLus(
                    b(Hoon::TisGar(b(limb("v")), b(limb("p")))),
                    b(Hoon::CenCol(b(limb("b")), vec![limb("c")])),
                    b(Hoon::CenTis(terms(&["a"]), vec![(c_wing, limb("c"))])),
                )))),
            )),
        )),
    );
    assert_eq!(
        open(Hoon::MicSig(b(limb("p")), vec![limb("x"), limb("y")])),
        Hoon::TisGar(b(v_bind()), b(chained))
    );
}

#[test]
#[should_panic(expected = "open-mcsg")]
fn open_mcsg_rejects_an_empty_list() {
    open(Hoon::MicSig(b(limb("p")), vec![]));
}

#[test]
fn open_tis_runes() {
    let skin = term_skin("a");
    // [%tstr *] [%tsgl r.gen [%tune [[p.p.gen ~ [%kthp u.q.p.gen q.gen]] ~ ~] ~]]
    let spec = atom_spec("ud");
    assert_eq!(
        open(Hoon::TisTar(
            ("x".to_string(), Some(b(spec.clone()))),
            b(ax(2)),
            b(limb("body"))
        )),
        Hoon::TisGal(
            b(limb("body")),
            b(Hoon::Tune(TermOrTune::Tune((
                HashMap::from([("x".to_string(), Some(Hoon::KetHep(b(spec), b(ax(2)))))]),
                vec![],
            )))),
        )
    );
    // [%tsfs *] [%tsls [%ktts p.gen q.gen] r.gen] ; [%tsmc *] [%tsfs p.gen r.gen q.gen]
    assert_eq!(
        open(Hoon::TisFas(skin.clone(), b(ax(2)), b(ax(3)))),
        Hoon::TisLus(b(Hoon::KetTis(skin.clone(), b(ax(2)))), b(ax(3)))
    );
    assert_eq!(
        open(Hoon::TisMic(skin.clone(), b(ax(2)), b(ax(3)))),
        Hoon::TisFas(skin, b(ax(3)), b(ax(2)))
    );
    // [%tsdt *] [%tsgr [%cncb [[%& 1] ~] [[p.gen q.gen] ~]] r.gen]
    let w = terms(&["w"]);
    assert_eq!(
        open(Hoon::TisDot(w.clone(), b(ax(2)), b(ax(3)))),
        Hoon::TisGar(
            b(Hoon::CenCab(vec![axis_limb(1)], vec![(w.clone(), ax(2))])),
            b(ax(3))
        )
    );
    // [%tswt *] [%tsdt p.gen [%wtcl q.gen r.gen [%wing p.gen]] s.gen]
    assert_eq!(
        open(Hoon::TisWut(w.clone(), b(ax(2)), b(ax(3)), b(ax(4)))),
        Hoon::TisDot(
            w.clone(),
            b(Hoon::WutCol(b(ax(2)), b(ax(3)), b(Hoon::Wing(w)))),
            b(ax(4))
        )
    );
    // [%tshp *] [%tsls q.gen p.gen]
    assert_eq!(
        open(Hoon::TisHep(b(ax(2)), b(ax(3)))),
        Hoon::TisLus(b(ax(3)), b(ax(2)))
    );
    // [%tssg *] ~ -> [%$ 1] ; [* ~] -> i ; else [%tsgr i $]
    assert_eq!(open(Hoon::TisSig(vec![])), ax(1));
    assert_eq!(open(Hoon::TisSig(vec![ax(2)])), ax(2));
    assert_eq!(
        open(Hoon::TisSig(vec![ax(2), ax(3)])),
        Hoon::TisGar(b(ax(2)), b(ax(3)))
    );
}

#[test]
fn open_tskt_threads_the_state_through_v() {
    // [%tskt *] =+ wuy=(weld q.gen `wing`[%v ~])
    //   [%tsgr [%ktts %v %$ 1] [%tsls [%ktts %a %tsgr [%limb %v] r]
    //     [%tsdt wuy [%tsgl [%$ 3] [%limb %a]]
    //       [%tsgr [[%ktts [%over [%v ~] p] [%tsgl [%$ 2] [%limb %a]]] [%limb %v]] s]]]]
    let got = open(Hoon::TisKet(
        term_skin("x"),
        terms(&["w"]),
        b(limb("r")),
        b(limb("s")),
    ));
    let expected = Hoon::TisGar(
        b(v_bind()),
        b(Hoon::TisLus(
            b(Hoon::KetTis(
                term_skin("a"),
                b(Hoon::TisGar(b(limb("v")), b(limb("r")))),
            )),
            b(Hoon::TisDot(
                terms(&["w", "v"]),
                b(Hoon::TisGal(b(ax(3)), b(limb("a")))),
                b(Hoon::TisGar(
                    b(Hoon::Pair(
                        b(Hoon::KetTis(
                            Skin::Over(terms(&["v"]), b(term_skin("x"))),
                            b(Hoon::TisGal(b(ax(2)), b(limb("a")))),
                        )),
                        b(limb("v")),
                    )),
                    b(limb("s")),
                )),
            )),
        )),
    );
    assert_eq!(got, expected);
}

#[test]
fn open_wut_runes() {
    let (f0, f1) = (rock("f", 0), rock("f", 1));
    // [%wtbr *] ?~(p.gen [%rock %f 1] [%wtcl i.p.gen [%rock %f 0] $(p.gen t.p.gen)])
    assert_eq!(open(Hoon::WutBar(vec![])), f1.clone());
    assert_eq!(
        open(Hoon::WutBar(vec![ax(2)])),
        Hoon::WutCol(b(ax(2)), b(f0), b(f1))
    );
    // [%wtdt *] [%wtcl p r q] ; [%wtgl *] [%wtcl p [%zpzp ~] q]
    assert_eq!(
        open(Hoon::WutDot(b(ax(2)), b(ax(3)), b(ax(4)))),
        Hoon::WutCol(b(ax(2)), b(ax(4)), b(ax(3)))
    );
    assert_eq!(
        open(Hoon::WutGal(b(ax(2)), b(ax(3)))),
        Hoon::WutCol(b(ax(2)), b(Hoon::ZapZap), b(ax(3)))
    );
    // [%wtkt *] [%wtcl [%wtts [%base %atom %$] p.gen] r.gen q.gen]
    let w = terms(&["w"]);
    assert_eq!(
        open(Hoon::WutKet(w.clone(), b(ax(2)), b(ax(3)))),
        Hoon::WutCol(
            b(Hoon::WutTis(b(atom_spec("$")), w.clone())),
            b(ax(3)),
            b(ax(2))
        )
    );
    // [%wthp *] ?~(q.gen [%lost [%wing p.gen]] [%wtcl [%wtts p.i p.gen] q.i $])
    assert_eq!(
        open(Hoon::WutHep(w.clone(), vec![])),
        Hoon::Lost(b(Hoon::Wing(w.clone())))
    );
    assert_eq!(
        open(Hoon::WutHep(w.clone(), vec![(leaf_spec(1), ax(2))])),
        Hoon::WutCol(
            b(Hoon::WutTis(b(leaf_spec(1)), w.clone())),
            b(ax(2)),
            b(Hoon::Lost(b(Hoon::Wing(w.clone()))))
        )
    );
    // [%wtls *] [%wthp p.gen (weld r.gen [[%base %noun] q.gen] ~)]
    assert_eq!(
        open(Hoon::WutLus(
            w.clone(),
            b(ax(9)),
            vec![(leaf_spec(1), ax(2))]
        )),
        Hoon::WutHep(w, vec![(leaf_spec(1), ax(2)), (noun_spec(), ax(9))])
    );
}

#[test]
fn open_xray_builds_tag_attributes_and_children() {
    // [%xray *] [[(open-mane n) %clsg (turn a open-mart)] [%mcts c]]
    let manx = Manx {
        g: Marx {
            n: Mane::TagSpace("a".to_string(), "b".to_string()),
            a: vec![
                (
                    Mane::Tag("c".to_string()),
                    vec![Beer::Char("x".to_string()), Beer::Hoon(limb("y"))],
                ),
                (Mane::TagSpace("d".to_string(), "e".to_string()), vec![]),
            ],
        },
        c: vec![],
    };
    let expected = Hoon::Pair(
        b(Hoon::Pair(
            b(Hoon::Pair(b(tas_rock("a")), b(tas_rock("b")))),
            b(Hoon::ColSig(vec![
                Hoon::Pair(
                    b(tas_rock("c")),
                    b(Hoon::Knit(vec![
                        Woof::ParsedAtom(cord("x")),
                        Woof::Hoon(limb("y")),
                    ])),
                ),
                Hoon::Pair(
                    b(Hoon::Pair(b(tas_rock("d")), b(tas_rock("e")))),
                    b(Hoon::Knit(vec![])),
                ),
            ])),
        )),
        b(Hoon::MicTis(vec![])),
    );
    assert_eq!(open(Hoon::Xray(manx)), expected);

    // A plain tag name opens to a single %tas rock.
    let plain = Manx {
        g: Marx {
            n: Mane::Tag("p".to_string()),
            a: vec![],
        },
        c: vec![Tuna::TunaTail(TunaTail::Manx(limb("kid")))],
    };
    assert_eq!(
        open(Hoon::Xray(plain)),
        Hoon::Pair(
            b(Hoon::Pair(b(tas_rock("p")), b(Hoon::ColSig(vec![])))),
            b(Hoon::MicTis(vec![Tuna::TunaTail(TunaTail::Manx(limb(
                "kid"
            )))])),
        )
    );
}

#[test]
fn open_zpwt_accepts_matching_versions() {
    // [%zpwt *] ?@ p.gen (lte hoon-version p.gen)
    let body = || b(ax(1));
    assert_eq!(
        open(Hoon::ZapWut(ZpwtArg::ParsedAtom("138".to_string()), body())),
        ax(1)
    );
    assert_eq!(
        open(Hoon::ZapWut(ZpwtArg::ParsedAtom("200".to_string()), body())),
        ax(1)
    );
    // [138 138] is accepted under either reading of the pair.
    assert_eq!(
        open(Hoon::ZapWut(
            ZpwtArg::Pair("138".to_string(), "138".to_string()),
            body()
        )),
        ax(1)
    );
}

#[test]
#[should_panic(expected = "hoon-version")]
fn open_zpwt_rejects_an_old_version() {
    open(Hoon::ZapWut(
        ZpwtArg::ParsedAtom("137".to_string()),
        b(ax(1)),
    ));
}

#[test]
#[should_panic(expected = "hoon-version")]
fn open_zpwt_rejects_an_unparsable_pair() {
    open(Hoon::ZapWut(
        ZpwtArg::Pair("x".to_string(), "140".to_string()),
        b(ax(1)),
    ));
}

#[test]
#[should_panic(expected = "hoon-version")]
fn open_zpwt_rejects_a_pair_excluding_the_version() {
    open(Hoon::ZapWut(
        ZpwtArg::Pair("139".to_string(), "140".to_string()),
        b(ax(1)),
    ));
}

#[test]
#[ignore = "hoon-138 accepts !?([p q] x) when q <= 138 <= p; hatch reads the pair as \
            [min max] (utils.rs ~2695); see coverage/p1/divergent/p1_zpwt_pair_order.hoon"]
fn open_zpwt_pair_is_upper_bound_first() {
    assert_eq!(
        open(Hoon::ZapWut(
            ZpwtArg::Pair("140".to_string(), "130".to_string()),
            b(ax(1))
        )),
        ax(1)
    );
}

// ---------------------------------------------------------------------------
// ++ap: flay, feck, grip, half, reek, name; ++ax autoname
// ---------------------------------------------------------------------------

#[test]
fn flay_accepts_skin_shaped_hoons() {
    // [%cnts [@ ~] ~] `i.p.gen
    assert_eq!(
        flay(Hoon::CenTis(terms(&["a"]), vec![])),
        Some(term_skin("a"))
    );
    // [%limb @] `p.gen ; [%rock *] with an atom -> [%leaf p.gen q.gen]
    assert_eq!(flay(limb("a")), Some(term_skin("a")));
    assert_eq!(
        flay(rock("tas", 1)),
        Some(Skin::Leaf("tas".to_string(), ParsedAtom::Small(1)))
    );
    // [%wing *] of only [%| 0 ~] limbs -> [%wash depth] (depth stays 0 in hoon-138)
    assert_eq!(
        flay(Hoon::Wing(vec![
            Limb::Parent(0, None),
            Limb::Parent(0, None)
        ])),
        Some(Skin::Wash(0))
    );
    // [%tsgr *] (biff reek(p) |=(wing (bind flay(q) [%over wing skin])))
    assert_eq!(
        flay(Hoon::TisGar(b(Hoon::Wing(terms(&["w"]))), b(limb("a")))),
        Some(Skin::Over(terms(&["w"]), b(term_skin("a"))))
    );
    // [%kttr *] `[%spec p.gen %base %noun]
    assert_eq!(
        flay(Hoon::KetTar(b(atom_spec("ud")))),
        Some(Skin::Spec(
            b(atom_spec("ud")),
            b(Skin::Base(BaseType::NounExpr))
        ))
    );
    // [%ktts *] with p.gen = [%name @ [%base %noun]] -> [%name term skin]
    assert_eq!(
        flay(Hoon::KetTis(
            Skin::Name("d".to_string(), b(Skin::Base(BaseType::NounExpr))),
            b(limb("e")),
        )),
        Some(Skin::Name("d".to_string(), b(term_skin("e"))))
    );
}

#[test]
fn flay_rejects_non_skins() {
    // a cell with one bad side
    assert_eq!(flay(Hoon::Pair(b(limb("a")), b(Hoon::ZapZap))), None);
    // [%rock *] ?@(q.gen `[%leaf ...] ~)
    let cell_rock = Hoon::Rock("$".to_string(), NounExpr::Cell(b(small(1)), b(small(2))));
    assert_eq!(flay(cell_rock), None);
    // %cnts with a longer wing, or with changes
    assert_eq!(flay(Hoon::CenTis(terms(&["a", "b"]), vec![])), None);
    assert_eq!(
        flay(Hoon::CenTis(terms(&["a"]), vec![(terms(&["b"]), ax(1))])),
        None
    );
    // %tsgr whose subject is not a wing, or whose product does not flay
    assert_eq!(flay(Hoon::TisGar(b(Hoon::ZapZap), b(limb("a")))), None);
    assert_eq!(
        flay(Hoon::TisGar(b(Hoon::Wing(terms(&["w"]))), b(Hoon::ZapZap))),
        None
    );
    // a wing with a limb other than [%| 0 ~]
    assert_eq!(flay(Hoon::Wing(vec![axis_limb(2)])), None);
    // %ktts whose name skin is not over %noun, or whose product does not flay
    assert_eq!(
        flay(Hoon::KetTis(
            Skin::Name("d".to_string(), b(Skin::Base(BaseType::Cell))),
            b(limb("e")),
        )),
        None
    );
    assert_eq!(flay(Hoon::KetTis(term_skin("d"), b(Hoon::ZapZap))), None);
    // default: open does not change gen
    assert_eq!(flay(Hoon::ZapZap), None);
}

#[test]
fn feck_finds_tas_sands_only() {
    // [%sand %tas @] [~ q.gen] ; [%dbug *] $(gen q.gen) ; * ~
    let tas = Hoon::Sand("tas".to_string(), NounExpr::ParsedAtom(cord("foo")));
    assert_eq!(feck(tas.clone()), Some(cord("foo")));
    assert_eq!(feck(Hoon::Dbug(spot(), b(tas))), Some(cord("foo")));
    assert_eq!(feck(sand("ud", 1)), None);
    assert_eq!(
        feck(Hoon::Sand(
            "tas".to_string(),
            NounExpr::Cell(b(small(1)), b(small(2)))
        )),
        None
    );
    assert_eq!(feck(rock("tas", 1)), None);
}

#[test]
fn grip_skin_cases() {
    let gen = limb("g");
    let noun_skin = || b(Skin::Base(BaseType::NounExpr));
    // [%base *] ?:(?=(%noun base.skin) gen [%kthp skin gen])
    assert_eq!(
        grip(Skin::Base(BaseType::Cell), gen.clone(), vec![]),
        Hoon::KetHep(b(Spec::Base(BaseType::Cell)), b(gen.clone()))
    );
    // [%cell *] with a halvable gen
    assert_eq!(
        grip(
            Skin::Cell(b(term_skin("a")), b(term_skin("b"))),
            Hoon::Pair(b(ax(2)), b(ax(3))),
            vec![]
        ),
        Hoon::Pair(
            b(grip(term_skin("a"), ax(2), vec![])),
            b(grip(term_skin("b"), ax(3), vec![]))
        )
    );
    // [%dbug *] [%dbug spot.skin $(skin skin.skin)]
    assert_eq!(
        grip(Skin::Dbug(spot(), noun_skin()), gen.clone(), vec![]),
        Hoon::Dbug(spot(), b(gen.clone()))
    );
    // [%help *] [%note [%help help.skin] $(skin skin.skin)]
    assert_eq!(
        grip(Skin::Help(help(), noun_skin()), gen.clone(), vec![]),
        Hoon::Note(Note::Help(help()), b(gen.clone()))
    );
    // [%leaf *] [%kthp skin gen]
    assert_eq!(
        grip(
            Skin::Leaf("tas".to_string(), ParsedAtom::Small(1)),
            gen.clone(),
            vec![]
        ),
        Hoon::KetHep(b(leaf_spec(1)), b(gen.clone()))
    );
    // [%name *] [%tsgl [%tune term.skin] $(skin skin.skin)]
    assert_eq!(
        grip(
            Skin::Name("n".to_string(), b(term_skin("m"))),
            gen.clone(),
            vec![]
        ),
        Hoon::TisGal(
            b(Hoon::Tune(TermOrTune::Term("n".to_string()))),
            b(Hoon::TisGal(
                b(Hoon::Tune(TermOrTune::Term("m".to_string()))),
                b(gen.clone())
            )),
        )
    );
    // [%over *] $(rel (weld wing.skin rel)), then [%spec *] uses [%over rel spec]
    assert_eq!(
        grip(
            Skin::Over(
                terms(&["w"]),
                b(Skin::Spec(b(atom_spec("ud")), noun_skin()))
            ),
            gen.clone(),
            terms(&["r"]),
        ),
        Hoon::KetHep(
            b(Spec::Over(terms(&["w", "r"]), b(atom_spec("ud")))),
            b(gen.clone())
        )
    );
    // [%wash *] [%tsgl [%wing (reap depth [%| 0 ~])] gen]
    assert_eq!(
        grip(Skin::Wash(2), gen.clone(), vec![]),
        Hoon::TisGal(
            b(Hoon::Wing(vec![
                Limb::Parent(0, None),
                Limb::Parent(0, None)
            ])),
            b(gen)
        )
    );
}

#[test]
fn half_splits_cell_constructors() {
    let (p, q, r, s) = (ax(2), ax(3), ax(4), ax(5));
    assert_eq!(
        half(Hoon::Pair(b(p.clone()), b(q.clone()))),
        Some((p.clone(), q.clone()))
    );
    // [%clcb *] `[q.gen p.gen]
    assert_eq!(
        half(Hoon::ColCab(b(p.clone()), b(q.clone()))),
        Some((q.clone(), p.clone()))
    );
    assert_eq!(
        half(Hoon::ColHep(b(p.clone()), b(q.clone()))),
        Some((p.clone(), q.clone()))
    );
    // [%clkt *] `[p.gen %clls q.gen r.gen s.gen]
    assert_eq!(
        half(Hoon::ColKet(
            b(p.clone()),
            b(q.clone()),
            b(r.clone()),
            b(s.clone())
        )),
        Some((p.clone(), Hoon::ColLus(b(q.clone()), b(r.clone()), b(s))))
    );
    // [%clsg *] ?~(p.gen ~ `[i.p.gen %clsg t.p.gen])
    assert_eq!(half(Hoon::ColSig(vec![])), None);
    assert_eq!(
        half(Hoon::ColSig(vec![p.clone(), q.clone()])),
        Some((p.clone(), Hoon::ColSig(vec![q.clone()])))
    );
    // [%cltr *] ?~ p.gen ~ ?~(t.p.gen $(gen i.p.gen) `[i.p.gen %cltr t.p.gen])
    assert_eq!(half(Hoon::ColTar(vec![])), None);
    assert_eq!(
        half(Hoon::ColTar(vec![Hoon::Pair(b(p.clone()), b(q.clone()))])),
        Some((p.clone(), q.clone()))
    );
    assert_eq!(
        half(Hoon::ColTar(vec![p.clone(), q.clone(), r.clone()])),
        Some((p, Hoon::ColTar(vec![q, r])))
    );
    assert_eq!(half(Hoon::ZapZap), None);
}

#[test]
fn reek_finds_wings() {
    // [%limb *] `[p.gen ~] ; [%cnts * ~] `p.gen ; [%dbug *] reek(q.gen) ; * ~
    assert_eq!(reek(limb("a")), Some(terms(&["a"])));
    assert_eq!(
        reek(Hoon::CenTis(terms(&["a", "b"]), vec![])),
        Some(terms(&["a", "b"]))
    );
    assert_eq!(
        reek(Hoon::CenTis(terms(&["a"]), vec![(terms(&["b"]), ax(1))])),
        None
    );
    assert_eq!(
        reek(Hoon::Dbug(spot(), b(Hoon::Wing(terms(&["a"]))))),
        Some(terms(&["a"]))
    );
    assert_eq!(reek(Hoon::Pair(b(limb("a")), b(ax(1)))), None);
    assert_eq!(reek(Hoon::ZapZap), None);
    // hatch-only extension (hoon-138 +reek has no %cncb case): %cncb with no
    // changes reads as its wing; with changes it is not a wing.
    assert_eq!(
        reek(Hoon::CenCab(terms(&["a"]), vec![])),
        Some(terms(&["a"]))
    );
    assert_eq!(
        reek(Hoon::CenCab(terms(&["a"]), vec![(terms(&["b"]), ax(1))])),
        None
    );
}

#[test]
#[ignore = "hoon-138 +reek maps [%$ p] (pattern [~ *]) to ~[&/p]; hatch instead matches \
            a pair headed by an axis (utils.rs ~2958); unreachable from source"]
fn reek_reads_a_bare_axis_as_a_wing() {
    assert_eq!(reek(ax(4)), Some(vec![axis_limb(4)]));
    assert_eq!(reek(Hoon::Pair(b(ax(4)), b(ax(1)))), None);
}

#[test]
fn name_ax_follows_hoon_138_name() {
    // [%wing *] ?~ p.gen ~ ?^ i.p.gen ?:(?=(%& -.i.p.gen) ~ q.i.p.gen) `i.p.gen
    assert_eq!(name_ax(Hoon::Wing(vec![])), None);
    assert_eq!(name_ax(Hoon::Wing(vec![axis_limb(2)])), None);
    assert_eq!(
        name_ax(Hoon::Wing(vec![Limb::Parent(1, Some("x".to_string()))])),
        Some("x".to_string())
    );
    assert_eq!(name_ax(Hoon::Wing(vec![Limb::Parent(0, None)])), None);
    assert_eq!(
        name_ax(Hoon::Wing(terms(&["x", "y"]))),
        Some("x".to_string())
    );
    // [%limb *] `p.gen ; [%dbug *] ; [%tsgl *] via open ; [%tsgr *] $(gen q.gen)
    assert_eq!(name_ax(limb("x")), Some("x".to_string()));
    assert_eq!(
        name_ax(Hoon::Dbug(spot(), b(limb("x")))),
        Some("x".to_string())
    );
    assert_eq!(
        name_ax(Hoon::TisGal(b(limb("x")), b(limb("y")))),
        Some("x".to_string())
    );
    assert_eq!(
        name_ax(Hoon::TisGar(b(limb("y")), b(limb("x")))),
        Some("x".to_string())
    );
    assert_eq!(name_ax(Hoon::ZapZap), None);
}

#[test]
fn autoname_follows_hoon_138_autoname() {
    let name = |s: &str| Some(s.to_string());
    let a = || b(atom_spec("ud"));
    let arms = HashMap::new;
    // %base
    assert_eq!(autoname(atom_spec("$")), name("atom"));
    assert_eq!(autoname(atom_spec("ud")), name("ud"));
    assert_eq!(autoname(Spec::Base(BaseType::Cell)), None);
    // %dbug %gist %leaf %loop
    assert_eq!(autoname(Spec::Dbug(spot(), a())), name("ud"));
    assert_eq!(autoname(Spec::Gist(help(), a())), name("ud"));
    assert_eq!(autoname(leaf_spec(1)), name("tas"));
    assert_eq!(autoname(Spec::Loop("x".to_string())), name("x"));
    // %like ?~(p.mod ~ ?^(i.p.mod ?:(?=(%& -.i.p.mod) ~ q.i.p.mod) `i.p.mod))
    assert_eq!(autoname(Spec::Like(vec![], vec![])), None);
    assert_eq!(autoname(Spec::Like(vec![axis_limb(2)], vec![])), None);
    assert_eq!(
        autoname(Spec::Like(
            vec![Limb::Parent(1, Some("x".to_string()))],
            vec![]
        )),
        name("x")
    );
    assert_eq!(autoname(Spec::Like(terms(&["x"]), vec![])), name("x"));
    // %make ~(name ap p.mod) ; %made %name %over -> q.mod
    assert_eq!(
        autoname(Spec::Make(limb("list"), vec![noun_spec()])),
        name("list")
    );
    assert_eq!(
        autoname(Spec::Made(("m".to_string(), vec![]), a())),
        name("ud")
    );
    assert_eq!(autoname(Spec::Name("n".to_string(), a())), name("ud"));
    assert_eq!(autoname(Spec::Over(terms(&["w"]), a())), name("ud"));
    // $$ $| $_ $: $% $. $< $> $- $^ $+ $/ $; $& $~ $` $= $@ $? $!
    assert_eq!(autoname(Spec::BucBuc(a(), arms())), name("ud"));
    assert_eq!(autoname(Spec::BucBar(a(), limb("f"))), name("ud"));
    assert_eq!(autoname(Spec::BucCab(limb("x"))), name("x"));
    assert_eq!(autoname(Spec::BucCol(a(), vec![noun_spec()])), name("ud"));
    assert_eq!(autoname(Spec::BucCen(a(), vec![noun_spec()])), name("ud"));
    assert_eq!(autoname(Spec::BucDot(a(), arms())), None);
    assert_eq!(autoname(Spec::BucGal(b(noun_spec()), a())), name("ud"));
    assert_eq!(autoname(Spec::BucGar(b(noun_spec()), a())), name("ud"));
    assert_eq!(autoname(Spec::BucHep(a(), b(noun_spec()))), name("ud"));
    assert_eq!(autoname(Spec::BucKet(b(noun_spec()), a())), name("ud"));
    assert_eq!(autoname(Spec::BucLus("k".to_string(), a())), name("ud"));
    assert_eq!(autoname(Spec::BucFas(a(), arms())), None);
    assert_eq!(autoname(Spec::BucMic(limb("g"))), name("g"));
    assert_eq!(autoname(Spec::BucPam(a(), limb("f"))), name("ud"));
    assert_eq!(autoname(Spec::BucSig(ax(1), a())), name("ud"));
    assert_eq!(autoname(Spec::BucTic(a(), arms())), None);
    assert_eq!(autoname(Spec::BucTis(term_skin("t"), a())), name("ud"));
    assert_eq!(autoname(Spec::BucPat(b(noun_spec()), a())), name("ud"));
    assert_eq!(autoname(Spec::BucWut(a(), vec![noun_spec()])), name("ud"));
    assert_eq!(autoname(Spec::BucZap(a(), arms())), None);
}

#[test]
#[ignore = "the parser spells @ as aura \"\", but autoname only maps \"$\" to %atom \
            (utils.rs ~2999); see coverage/p1/divergent/p1_autoname_bare_atom.hoon"]
fn autoname_names_a_bare_atom_atom() {
    assert_eq!(autoname(atom_spec("")), Some("atom".to_string()));
}

// ---------------------------------------------------------------------------
// ++ah: tiki helpers; peg
// ---------------------------------------------------------------------------

#[test]
fn tiki_helpers_for_named_wings_and_bare_hoons() {
    let named_wing = Tiki::Wing((Some("b".to_string()), terms(&["a"])));
    // gray %&: [%tstr [u.p.tik ~] [%wing q.tik] gen] ; puce %&: [u.p.tik ~]
    assert_eq!(
        wtts(named_wing, atom_spec("ud")),
        Hoon::TisTar(
            ("b".to_string(), None),
            b(Hoon::Wing(terms(&["a"]))),
            b(Hoon::WutTis(b(atom_spec("ud")), terms(&["b"]))),
        )
    );
    // tele %|: [%over [%& 3]~ syn] ; puce %|: [[%& 2] ~] ; gray %| with no name
    let bare_hoon = Tiki::Hoon((None, b(limb("h"))));
    assert_eq!(
        tele(bare_hoon.clone(), term_skin("x")),
        Skin::Over(vec![axis_limb(3)], b(term_skin("x")))
    );
    assert_eq!(
        wthx(bare_hoon, term_skin("x")),
        Hoon::TisLus(
            b(limb("h")),
            b(Hoon::WutHax(
                Skin::Over(vec![axis_limb(3)], b(term_skin("x"))),
                vec![axis_limb(2)],
            )),
        )
    );
}

#[test]
fn peg_rejects_zero_axes() {
    assert!(peg(0u64, 2u64).is_err());
    assert!(peg(2u64, 0u64).is_err());
    assert_eq!(peg(3u64, 2u64).map(|a| a.to_string()), Ok("6".to_string()));
}
