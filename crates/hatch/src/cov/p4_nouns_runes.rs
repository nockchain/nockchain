//! Coverage-driven tests for AST/noun conversion and rune parsers.
//!
//! Added to close branch-coverage gaps; see the coverage report in the PR.

use std::collections::HashMap;
use std::sync::Arc;

use chumsky::Parser;
use ibig::UBig;
use nockapp::noun::slab::NounSlab;
use nockvm::mug::calc_atom_mug_u32;
use nockvm::noun::{Atom, Noun, NounAllocator, D, T};
use nockvm_macros::tas;
use num_bigint::BigUint;

use crate::ast::hoon::*;
#[allow(unused_imports)]
use crate::utils::*;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn b<X>(x: X) -> Box<X> {
    Box::new(x)
}

fn s(x: &str) -> String {
    x.to_string()
}

fn ax(n: u64) -> Hoon {
    Hoon::Axis(n.into())
}

fn limb(x: &str) -> Hoon {
    Hoon::Limb(s(x))
}

fn wing(x: &str) -> WingType {
    vec![Limb::Term(s(x))]
}

fn atom(n: u128) -> NounExpr {
    NounExpr::ParsedAtom(ParsedAtom::Small(n))
}

fn rock(n: u128) -> Hoon {
    Hoon::Rock(s("ud"), atom(n))
}

fn base_ud() -> Spec {
    Spec::Base(BaseType::Atom(s("ud")))
}

fn spot() -> Spot {
    Spot {
        p: vec![s("p4"), s("hoon")],
        q: Pint {
            p: (1, 2),
            q: (3, 4),
        },
    }
}

fn help_expr() -> NounExpr {
    NounExpr::Cell(b(atom(0)), b(NounExpr::Cell(b(atom(0x6f64)), b(atom(0)))))
}

fn tome(arms: &[(&str, Hoon)]) -> Tome {
    (
        None,
        arms.iter()
            .map(|(k, v)| (s(k), v.clone()))
            .collect::<HashMap<_, _>>(),
    )
}

fn encode(hoon: &Hoon) -> (NounSlab, Noun) {
    let mut slab = NounSlab::new();
    let noun = hoon_to_noun(&mut slab, hoon);
    (slab, noun)
}

fn decode(slab: &NounSlab, noun: Noun) -> Result<Hoon, String> {
    let space = slab.noun_space();
    noun_to_hoon(noun.in_space(&space))
}

/// Encode, decode, and assert the AST survives the trip unchanged.
fn roundtrip(hoon: Hoon) {
    let (slab, noun) = encode(&hoon);
    let back = decode(&slab, noun);
    assert_eq!(back.as_ref(), Ok(&hoon), "round trip changed {hoon:?}");
}

fn noun_eq(slab: &NounSlab, a: Noun, b: Noun) -> bool {
    let space = slab.noun_space();
    nockvm::ext::noun_equality(a.in_space(&space), b.in_space(&space))
}

fn big_atom(slab: &mut NounSlab) -> Noun {
    Atom::from_ubig(slab, &(UBig::from(1u8) << 100)).as_noun()
}

/// Decode `[tag tail]` where `f` builds the tail.
fn decode_tagged(tag: u64, f: impl FnOnce(&mut NounSlab) -> Noun) -> Result<Hoon, String> {
    let mut slab = NounSlab::new();
    let tail = f(&mut slab);
    let noun = T(&mut slab, &[D(tag), tail]);
    decode(&slab, noun)
}

fn parse_src(src: &str) -> Result<Hoon, String> {
    let linemap = Arc::new(LineMap::new_with_docs(src, true));
    crate::native_parser(vec![s("test"), s("p4.hoon")], false, linemap)
        .parse(src)
        .into_result()
        .map_err(|errs| format!("{errs:?}"))
}

/// Parse a file consisting of one expression and return that expression.
fn parse_one(src: &str) -> Hoon {
    match parse_src(src).unwrap_or_else(|e| panic!("{src:?} should parse: {e}")) {
        Hoon::TisSig(mut items) if items.len() == 1 => items.remove(0),
        other => panic!("expected a single-expression file, got {other:?}"),
    }
}

fn peel(h: &Hoon) -> &Hoon {
    match h {
        Hoon::Dbug(_, inner) => peel(inner),
        other => other,
    }
}

fn contains_help(h: &Hoon) -> bool {
    format!("{h:?}").contains("Help(")
}

fn contains_gist(h: &Hoon) -> bool {
    format!("{h:?}").contains("Gist(")
}

// ---------------------------------------------------------------------------
// hoon_to_noun / noun_to_hoon round trips, one per Hoon variant
// ---------------------------------------------------------------------------

#[test]
fn roundtrip_leaf_and_internal_hoon_forms() {
    roundtrip(Hoon::Pair(b(ax(1)), b(Hoon::ZapZap)));
    roundtrip(Hoon::Base(BaseType::Void));
    roundtrip(Hoon::Base(BaseType::Null));
    roundtrip(Hoon::Base(BaseType::Flag));
    roundtrip(Hoon::Base(BaseType::Cell));
    roundtrip(Hoon::Bust(BaseType::NounExpr));
    roundtrip(Hoon::Bust(BaseType::Atom(s("ud"))));
    roundtrip(Hoon::Dbug(spot(), b(ax(2))));
    roundtrip(Hoon::Eror(s("duplicate arm: +a")));
    roundtrip(Hoon::Eror(String::new()));
    roundtrip(Hoon::Note(Note::Help(help_expr()), b(ax(1))));
    roundtrip(Hoon::Note(Note::Know(s("foo")), b(ax(1))));
    roundtrip(Hoon::Note(Note::Made(s("foo"), None), b(ax(1))));
    roundtrip(Hoon::Note(
        Note::Made(
            s("foo"),
            Some(vec![wing("a"), vec![Limb::Axis(3u64.into())]]),
        ),
        b(ax(1)),
    ));
    roundtrip(Hoon::Fits(b(rock(1)), wing("a")));
    roundtrip(Hoon::Knit(vec![
        Woof::ParsedAtom(ParsedAtom::Small(97)),
        Woof::Hoon(limb("a")),
    ]));
    roundtrip(Hoon::Leaf(s("tas"), ParsedAtom::Small(0x6f66)));
    roundtrip(Hoon::Lost(b(Hoon::Wing(wing("a")))));
    roundtrip(Hoon::Sand(s("ud"), atom(5)));
    roundtrip(Hoon::Rock(
        s("ud"),
        NounExpr::Cell(
            b(atom(1)),
            b(NounExpr::ParsedAtom(ParsedAtom::Big(
                BigUint::from(1u8) << 200,
            ))),
        ),
    ));
    roundtrip(Hoon::Tell(vec![limb("a"), rock(1)]));
    roundtrip(Hoon::Yell(vec![limb("a"), rock(1)]));
    roundtrip(Hoon::Tune(TermOrTune::Term(s("foo"))));
    let mut tune_map = HashMap::new();
    tune_map.insert(s("a"), Some(rock(1)));
    tune_map.insert(s("b"), None);
    roundtrip(Hoon::Tune(TermOrTune::Tune((tune_map, vec![limb("c")]))));
    roundtrip(Hoon::Wing(vec![
        Limb::Term(s("a")),
        Limb::Axis(6u64.into()),
        Limb::Parent(2, Some(s("b"))),
        Limb::Parent(0, None),
    ]));
}

#[test]
fn roundtrip_sail_forms() {
    let child = Manx {
        g: Marx {
            n: Mane::Tag(s("p")),
            a: vec![],
        },
        c: vec![],
    };
    let manx = Manx {
        g: Marx {
            n: Mane::Tag(s("div")),
            a: vec![
                (
                    Mane::TagSpace(s("xml"), s("lang")),
                    vec![Beer::Char(s("x")), Beer::Hoon(limb("a"))],
                ),
                (Mane::Tag(s("id")), vec![Beer::Char(String::new())]),
            ],
        },
        c: vec![
            Tuna::Manx(child),
            Tuna::TunaTail(TunaTail::Tape(limb("t"))),
            Tuna::TunaTail(TunaTail::Manx(limb("m"))),
            Tuna::TunaTail(TunaTail::Marl(limb("l"))),
            Tuna::TunaTail(TunaTail::Call(limb("c"))),
        ],
    };
    roundtrip(Hoon::Xray(manx.clone()));
    roundtrip(Hoon::MicTis(vec![
        Tuna::Manx(manx),
        Tuna::TunaTail(TunaTail::Call(limb("c"))),
    ]));
}

#[test]
fn roundtrip_bar_col_cen_dot_ket_forms() {
    let t = tome(&[("a", rock(1)), ("$", limb("b"))]);
    let mut tomes = HashMap::new();
    tomes.insert(s("$"), t.clone());
    tomes.insert(s("chap"), (Some(help_expr()), t.1.clone()));

    roundtrip(Hoon::BarBuc(vec![s("a"), s("b")], b(base_ud())));
    roundtrip(Hoon::BarCab(
        b(base_ud()),
        vec![(s("al"), rock(2))],
        tomes.clone(),
    ));
    roundtrip(Hoon::BarCol(b(rock(1)), b(rock(2))));
    roundtrip(Hoon::BarCen(None, tomes.clone()));
    roundtrip(Hoon::BarCen(Some(s("pre")), tomes.clone()));
    roundtrip(Hoon::BarDot(b(rock(1))));
    roundtrip(Hoon::BarKet(b(limb("a")), tomes.clone()));
    roundtrip(Hoon::BarHep(b(rock(1))));
    roundtrip(Hoon::BarSig(b(base_ud()), b(rock(1))));
    roundtrip(Hoon::BarTar(b(base_ud()), b(rock(1))));
    roundtrip(Hoon::BarTis(b(base_ud()), b(rock(1))));
    roundtrip(Hoon::BarPat(None, tomes.clone()));
    roundtrip(Hoon::BarPat(Some(s("pre")), tomes));
    roundtrip(Hoon::BarWut(b(rock(1))));

    roundtrip(Hoon::ColCab(b(rock(1)), b(rock(2))));
    roundtrip(Hoon::ColKet(b(rock(1)), b(rock(2)), b(rock(3)), b(rock(4))));
    roundtrip(Hoon::ColHep(b(rock(1)), b(rock(2))));
    roundtrip(Hoon::ColLus(b(rock(1)), b(rock(2)), b(rock(3))));
    roundtrip(Hoon::ColSig(vec![rock(1), rock(2)]));
    roundtrip(Hoon::ColTar(vec![rock(1), rock(2)]));

    roundtrip(Hoon::CenCab(wing("a"), vec![(wing("b"), rock(1))]));
    roundtrip(Hoon::CenDot(b(rock(1)), b(limb("a"))));
    roundtrip(Hoon::CenHep(b(limb("a")), b(rock(1))));
    roundtrip(Hoon::CenCol(b(limb("a")), vec![rock(1), rock(2)]));
    roundtrip(Hoon::CenTar(
        wing("a"),
        b(rock(1)),
        vec![(wing("b"), rock(2))],
    ));
    roundtrip(Hoon::CenKet(
        b(limb("a")),
        b(rock(1)),
        b(rock(2)),
        b(rock(3)),
    ));
    roundtrip(Hoon::CenLus(b(limb("a")), b(rock(1)), b(rock(2))));
    roundtrip(Hoon::CenSig(wing("a"), b(limb("b")), vec![rock(1)]));
    roundtrip(Hoon::CenTis(wing("a"), vec![(wing("b"), rock(1))]));

    roundtrip(Hoon::DotKet(b(base_ud()), b(Hoon::ColTar(vec![rock(1)]))));
    roundtrip(Hoon::DotLus(b(rock(1))));
    roundtrip(Hoon::DotTar(b(rock(1)), b(rock(2))));
    roundtrip(Hoon::DotTis(b(rock(1)), b(rock(2))));
    roundtrip(Hoon::DotWut(b(rock(1))));

    roundtrip(Hoon::KetBar(b(rock(1))));
    roundtrip(Hoon::KetDot(b(limb("a")), b(rock(1))));
    roundtrip(Hoon::KetLus(b(rock(1)), b(rock(2))));
    roundtrip(Hoon::KetHep(b(base_ud()), b(rock(1))));
    roundtrip(Hoon::KetPam(b(rock(1))));
    roundtrip(Hoon::KetSig(b(rock(1))));
    roundtrip(Hoon::KetTis(Skin::Term(s("a")), b(rock(1))));
    roundtrip(Hoon::KetWut(b(rock(1))));
    roundtrip(Hoon::KetTar(b(base_ud())));
    roundtrip(Hoon::KetCol(b(base_ud())));
}

#[test]
fn roundtrip_sig_mic_tis_wut_zap_forms() {
    for chum in [
        Chum::Lef(s("k")),
        Chum::StdKel(s("k"), ParsedAtom::Small(138)),
        Chum::VenProKel(s("v"), s("p"), ParsedAtom::Small(1)),
        Chum::VenProVerKel(s("v"), s("p"), ParsedAtom::Small(1), ParsedAtom::Small(2)),
    ] {
        roundtrip(Hoon::SigCen(
            chum.clone(),
            b(ax(7)),
            vec![(s("t"), rock(1))],
            b(rock(2)),
        ));
        roundtrip(Hoon::SigFas(chum, b(rock(2))));
    }
    roundtrip(Hoon::SigBar(b(rock(1)), b(rock(2))));
    roundtrip(Hoon::SigCab(b(rock(1)), b(rock(2))));
    roundtrip(Hoon::SigGal(TermOrPair::Term(s("foo")), b(rock(1))));
    roundtrip(Hoon::SigGal(
        TermOrPair::Pair(s("foo"), b(rock(2))),
        b(rock(1)),
    ));
    roundtrip(Hoon::SigGar(TermOrPair::Term(s("foo")), b(rock(1))));
    roundtrip(Hoon::SigGar(
        TermOrPair::Pair(s("foo"), b(rock(2))),
        b(rock(1)),
    ));
    roundtrip(Hoon::SigBuc(s("foo"), b(rock(1))));
    roundtrip(Hoon::SigLus(3, b(rock(1))));
    roundtrip(Hoon::SigPam(2, b(rock(1)), b(rock(2))));
    roundtrip(Hoon::SigTis(b(rock(1)), b(rock(2))));
    roundtrip(Hoon::SigWut(1, b(rock(1)), b(rock(2)), b(rock(3))));
    roundtrip(Hoon::SigZap(b(rock(1)), b(rock(2))));

    roundtrip(Hoon::MicCol(b(limb("a")), vec![rock(1), rock(2)]));
    roundtrip(Hoon::MicFas(b(rock(1))));
    roundtrip(Hoon::MicGal(
        b(base_ud()),
        b(limb("a")),
        b(rock(1)),
        b(rock(2)),
    ));
    roundtrip(Hoon::MicSig(b(limb("a")), vec![rock(1)]));
    roundtrip(Hoon::MicMic(b(base_ud()), b(rock(1))));

    roundtrip(Hoon::TisBar(b(base_ud()), b(rock(1))));
    roundtrip(Hoon::TisCol(vec![(wing("a"), rock(1))], b(limb("a"))));
    roundtrip(Hoon::TisFas(Skin::Term(s("a")), b(rock(1)), b(limb("a"))));
    roundtrip(Hoon::TisMic(Skin::Term(s("a")), b(limb("a")), b(rock(1))));
    roundtrip(Hoon::TisDot(wing("a"), b(rock(1)), b(limb("a"))));
    roundtrip(Hoon::TisWut(
        wing("a"),
        b(rock(1)),
        b(rock(2)),
        b(limb("a")),
    ));
    roundtrip(Hoon::TisGal(b(rock(1)), b(rock(2))));
    roundtrip(Hoon::TisHep(b(rock(1)), b(rock(2))));
    roundtrip(Hoon::TisGar(b(rock(1)), b(rock(2))));
    roundtrip(Hoon::TisKet(
        Skin::Term(s("a")),
        wing("b"),
        b(rock(1)),
        b(rock(2)),
    ));
    roundtrip(Hoon::TisLus(b(rock(1)), b(rock(2))));
    roundtrip(Hoon::TisSig(vec![rock(1), rock(2)]));
    roundtrip(Hoon::TisTar((s("a"), None), b(rock(1)), b(limb("a"))));
    roundtrip(Hoon::TisTar(
        (s("a"), Some(b(base_ud()))),
        b(rock(1)),
        b(limb("a")),
    ));
    roundtrip(Hoon::TisCom(b(limb("a")), b(limb("b"))));

    roundtrip(Hoon::WutBar(vec![rock(0), rock(1)]));
    roundtrip(Hoon::WutHep(wing("a"), vec![(base_ud(), rock(1))]));
    roundtrip(Hoon::WutCol(b(rock(0)), b(rock(1)), b(rock(2))));
    roundtrip(Hoon::WutDot(b(rock(0)), b(rock(1)), b(rock(2))));
    roundtrip(Hoon::WutKet(wing("a"), b(rock(1)), b(rock(2))));
    roundtrip(Hoon::WutGal(b(rock(0)), b(rock(1))));
    roundtrip(Hoon::WutGar(b(rock(0)), b(rock(1))));
    roundtrip(Hoon::WutLus(
        wing("a"),
        b(rock(1)),
        vec![(base_ud(), rock(2))],
    ));
    roundtrip(Hoon::WutPam(vec![rock(0), rock(1)]));
    roundtrip(Hoon::WutPat(wing("a"), b(rock(1)), b(rock(2))));
    roundtrip(Hoon::WutSig(wing("a"), b(rock(1)), b(rock(2))));
    roundtrip(Hoon::WutHax(Skin::Term(s("a")), wing("b")));
    roundtrip(Hoon::WutTis(b(base_ud()), wing("b")));
    roundtrip(Hoon::WutZap(b(rock(0))));

    roundtrip(Hoon::ZapCom(b(rock(1)), b(rock(2))));
    roundtrip(Hoon::ZapGar(b(rock(1))));
    roundtrip(Hoon::ZapGal(b(base_ud()), b(rock(1))));
    roundtrip(Hoon::ZapMic(b(rock(1)), b(rock(2))));
    roundtrip(Hoon::ZapTis(b(rock(1))));
    roundtrip(Hoon::ZapPat(
        vec![wing("a"), wing("b")],
        b(rock(1)),
        b(rock(2)),
    ));
    roundtrip(Hoon::ZapWut(ZpwtArg::ParsedAtom(s("138")), b(rock(1))));
    roundtrip(Hoon::ZapWut(ZpwtArg::Pair(s("140"), s("130")), b(rock(1))));
    roundtrip(Hoon::ZapZap);
}

#[test]
fn zapwut_encoding_uses_tagged_digit_cords() {
    // Pins the current encoding recorded as a divergence in
    // coverage/p4/divergent/p4_zpwt_encoding.hoon: hoonc stores the bare
    // atom 138, hatch stores [%atom '138'].
    let (mut slab, noun) = encode(&Hoon::ZapWut(ZpwtArg::ParsedAtom(s("138")), b(ax(1))));
    let cord = Atom::from_ubig(&mut slab, &UBig::from(0x38_33_31u32)).as_noun();
    let arg = T(&mut slab, &[D(tas!(b"atom")), cord]);
    let axis = T(&mut slab, &[D(0), D(1)]);
    let expected = T(&mut slab, &[D(tas!(b"zpwt")), arg, axis]);
    assert!(noun_eq(&slab, noun, expected));
}

#[test]
fn roundtrip_every_spec_form() {
    let mut map = HashMap::new();
    map.insert(s("a"), base_ud());
    map.insert(s("b"), Spec::Base(BaseType::Null));
    let specs = vec![
        Spec::Base(BaseType::Void),
        Spec::Dbug(spot(), b(base_ud())),
        Spec::Gist(help_expr(), b(base_ud())),
        Spec::Leaf(s("tas"), ParsedAtom::Small(0x6f66)),
        Spec::Like(wing("a"), vec![wing("b"), wing("c")]),
        Spec::Loop(s("foo")),
        Spec::Made((s("foo"), vec![s("a"), s("b")]), b(base_ud())),
        Spec::Make(limb("list"), vec![base_ud()]),
        Spec::Name(s("foo"), b(base_ud())),
        Spec::Over(wing("a"), b(base_ud())),
        Spec::BucGar(b(base_ud()), b(Spec::Base(BaseType::Cell))),
        Spec::BucBuc(b(base_ud()), map.clone()),
        Spec::BucBar(b(base_ud()), limb("f")),
        Spec::BucCab(rock(1)),
        Spec::BucCol(b(base_ud()), vec![base_ud()]),
        Spec::BucCen(b(base_ud()), vec![base_ud()]),
        Spec::BucDot(b(base_ud()), map.clone()),
        Spec::BucGal(b(base_ud()), b(Spec::Base(BaseType::Cell))),
        Spec::BucHep(b(base_ud()), b(base_ud())),
        Spec::BucKet(b(base_ud()), b(base_ud())),
        Spec::BucLus(s("foo"), b(base_ud())),
        Spec::BucFas(b(base_ud()), map.clone()),
        Spec::BucMic(limb("f")),
        Spec::BucPam(b(base_ud()), limb("f")),
        Spec::BucSig(rock(1), b(base_ud())),
        Spec::BucTic(b(base_ud()), map.clone()),
        Spec::BucTis(Skin::Term(s("a")), b(base_ud())),
        Spec::BucPat(b(base_ud()), b(base_ud())),
        Spec::BucWut(b(base_ud()), vec![Spec::Base(BaseType::Null)]),
        Spec::BucZap(b(base_ud()), map),
    ];
    for spec in specs {
        roundtrip(Hoon::KetCol(b(spec)));
    }
}

#[test]
fn roundtrip_every_skin_form() {
    let skins = vec![
        Skin::Term(s("a")),
        Skin::Base(BaseType::Atom(s("ud"))),
        Skin::Cell(b(Skin::Term(s("a"))), b(Skin::Term(s("b")))),
        Skin::Dbug(spot(), b(Skin::Term(s("a")))),
        Skin::Help(help_expr(), b(Skin::Term(s("a")))),
        Skin::Leaf(s("tas"), ParsedAtom::Small(0x6f66)),
        Skin::Name(s("a"), b(Skin::Base(BaseType::NounExpr))),
        Skin::Over(wing("a"), b(Skin::Term(s("b")))),
        Skin::Spec(b(base_ud()), b(Skin::Term(s("a")))),
        Skin::Wash(2),
    ];
    for skin in skins {
        roundtrip(Hoon::KetTis(skin, b(rock(1))));
    }
}

// ---------------------------------------------------------------------------
// %hand: type_to_noun / nock_to_noun and their decoders
// ---------------------------------------------------------------------------

fn every_nock() -> Nock {
    use Nock::*;
    let c = |n: u128| b(Const(atom(n)));
    let parts = vec![
        Compose(c(1), c(2)),
        CellTest(c(3)),
        Increment(c(4)),
        Equality(c(5), c(6)),
        IfThenElse(c(7), c(8), c(9)),
        Edit((6u64.into(), c(10)), c(11)),
        Hint(NockHint::ParsedAtom(tas!(b"fast")), c(12)),
        Hint(NockHint::Pair(tas!(b"memo"), c(13)), c(14)),
        SerialCompose(c(15), c(16)),
        PushSubject(c(17), c(18)),
        SelectArm(2u64.into(), c(19)),
        GrabData(c(20), c(21)),
    ];
    parts
        .into_iter()
        .rev()
        .fold(AxisSelect(7u64.into()), |acc, n| Pair(b(n), b(acc)))
}

#[test]
fn roundtrip_hand_with_decodable_types_and_every_nock() {
    let atom_ty = Type::ParsedAtom(s("ud"), None);
    let types = vec![
        Type::NounExpr,
        Type::Void,
        atom_ty.clone(),
        Type::ParsedAtom(s("tas"), Some(0x6f66)),
        Type::Cell(b(Type::NounExpr), b(atom_ty.clone())),
        Type::Fork(vec![Type::NounExpr, Type::Void]),
        Type::Hint(
            (b(atom_ty.clone()), Note::Know(s("foo"))),
            b(Type::NounExpr),
        ),
        Type::Hold(b(Type::NounExpr), limb("a")),
    ];
    for ty in types {
        roundtrip(Hoon::Hand(b(ty), every_nock()));
    }
}

fn sample_coil() -> Coil {
    let gate: Gate = (b(base_ud()), b(Spec::Base(BaseType::NounExpr)));
    let stencil = Stencil::Half {
        left: b(Stencil::Full {
            blocks: vec![vec![vec![s("a"), s("b")]], vec![]],
        }),
        rite: b(Stencil::Lazy {
            fragment: 5u64.into(),
            resolve: gate,
        }),
    };
    let mut tomes = HashMap::new();
    tomes.insert(s("$"), (None, HashMap::from([(s("a"), rock(1))])));
    tomes.insert(
        s("chap"),
        (Some(help_expr()), HashMap::from([(s("b"), rock(2))])),
    );
    Coil {
        p: Garb {
            name: Some(s("door")),
            poly: Poly::Wet,
            vair: Vair::Iron,
        },
        q: Type::NounExpr,
        r: ((stencil, atom(0)), tomes),
    }
}

#[test]
fn hand_core_and_face_types_encode_but_do_not_decode() {
    let mut coil = sample_coil();
    for (name, poly, vair) in [
        (Some(s("door")), Poly::Dry, Vair::Gold),
        (None, Poly::Wet, Vair::Lead),
        (None, Poly::Dry, Vair::Zinc),
        (None, Poly::Wet, Vair::Iron),
    ] {
        coil.p.name = name;
        coil.p.poly = poly;
        coil.p.vair = vair;
        let hoon = Hoon::Hand(
            b(Type::Core(b(Type::NounExpr), b(coil.clone()))),
            Nock::Const(atom(0)),
        );
        let (slab, noun) = encode(&hoon);
        let err = decode(&slab, noun).expect_err("core types are not decodable");
        assert!(err.contains("core decoding not supported"), "{err}");
    }

    let mut tune_map = HashMap::new();
    tune_map.insert(s("a"), Some(rock(1)));
    for face in [FaceType::Term(s("a")), FaceType::Tune((tune_map, vec![limb("b")]))] {
        let hoon = Hoon::Hand(b(Type::Face(face, b(Type::NounExpr))), Nock::Const(atom(0)));
        let (slab, noun) = encode(&hoon);
        let err = decode(&slab, noun).expect_err("face types are not decodable");
        assert!(err.contains("face decoding"), "{err}");
    }
}

#[test]
fn fork_types_decode_from_a_set_treap() {
    // [%hand [%fork {%noun %void}] [1 0]] with the fork stored as a treap:
    // node %noun with left child %void.
    let mut slab = NounSlab::new();
    let leaf = T(&mut slab, &[D(tas!(b"void")), D(0), D(0)]);
    let set = T(&mut slab, &[D(tas!(b"noun")), leaf, D(0)]);
    let ty = T(&mut slab, &[D(tas!(b"fork")), set]);
    let nock = T(&mut slab, &[D(1), D(0)]);
    let noun = T(&mut slab, &[D(tas!(b"hand")), ty, nock]);
    let Ok(Hoon::Hand(ty, _)) = decode(&slab, noun) else {
        panic!("fork set should decode");
    };
    assert_eq!(*ty, Type::Fork(vec![Type::NounExpr, Type::Void]));
}

#[test]
fn fork_set_decode_stops_at_node_budget() {
    let mut slab = NounSlab::new();
    let mut set = D(0);
    for _ in 0..1_000_002u32 {
        let branches = T(&mut slab, &[D(0), set]);
        set = T(&mut slab, &[D(tas!(b"noun")), branches]);
    }
    let ty = T(&mut slab, &[D(tas!(b"fork")), set]);
    let nock = T(&mut slab, &[D(1), D(0)]);
    let noun = T(&mut slab, &[D(tas!(b"hand")), ty, nock]);
    assert!(decode(&slab, noun).is_err());
}

fn decode_hand(f: impl FnOnce(&mut NounSlab) -> (Noun, Noun)) -> Result<Hoon, String> {
    let mut slab = NounSlab::new();
    let (ty, nock) = f(&mut slab);
    let noun = T(&mut slab, &[D(tas!(b"hand")), ty, nock]);
    decode(&slab, noun)
}

#[test]
fn type_and_nock_decoders_reject_malformed_nouns() {
    // an unknown atom tag, an unknown cell tag, and an indirect atom type
    assert!(decode_hand(|sl| (D(tas!(b"zzzz")), T(sl, &[D(1), D(0)]))).is_err());
    let err = decode_hand(|sl| {
        let t = T(sl, &[D(tas!(b"zzzz")), D(0)]);
        (t, T(sl, &[D(1), D(0)]))
    })
    .unwrap_err();
    assert!(err.contains("type: unknown tag"), "{err}");
    assert!(decode_hand(|sl| {
        let big = big_atom(sl);
        (big, T(sl, &[D(1), D(0)]))
    })
    .is_err());
    // an unknown nock opcode
    let err = decode_hand(|sl| (D(tas!(b"noun")), T(sl, &[D(99), D(0)]))).unwrap_err();
    assert!(err.contains("unknown opcode"), "{err}");
}

// ---------------------------------------------------------------------------
// decoder edge cases for helper nouns
// ---------------------------------------------------------------------------

#[test]
fn list_decoding_rejects_non_null_tails() {
    // [%clsg [[0 1] 5]] and [%clsg [[0 1] <indirect atom>]]
    let err = decode_tagged(tas!(b"clsg"), |sl| {
        let a = T(sl, &[D(0), D(1)]);
        T(sl, &[a, D(5)])
    })
    .unwrap_err();
    assert!(err.contains("non-zero atom tail"), "{err}");
    let err = decode_tagged(tas!(b"clsg"), |sl| {
        let a = T(sl, &[D(0), D(1)]);
        let big = big_atom(sl);
        T(sl, &[a, big])
    })
    .unwrap_err();
    assert!(err.contains("non-zero atom tail"), "{err}");
    // %tsig is an accepted alias of %tssg
    assert_eq!(
        decode_tagged(tas!(b"tsig"), |_| D(0)),
        Ok(Hoon::TisSig(vec![]))
    );
}

#[test]
fn unit_and_treap_decoding_reject_non_null_atoms() {
    // a |% prefix unit that is a non-zero or indirect atom
    for use_big in [false, true] {
        let err = decode_tagged(tas!(b"brcn"), |sl| {
            let pre = if use_big { big_atom(sl) } else { D(5) };
            T(sl, &[pre, D(0)])
        })
        .unwrap_err();
        assert!(err.contains("unit"), "{err}");
    }
    // a tome treap that is a non-zero or indirect atom
    for use_big in [false, true] {
        let err = decode_tagged(tas!(b"brcn"), |sl| {
            let map = if use_big { big_atom(sl) } else { D(5) };
            T(sl, &[D(0), map])
        })
        .unwrap_err();
        assert!(err.contains("treap"), "{err}");
    }
}

#[test]
fn tome_decoding_keeps_non_null_what() {
    // tome = [what map], with `what` a nonzero atom and then a cell
    for cell_what in [false, true] {
        let res = decode_tagged(tas!(b"brcn"), |sl| {
            let what = if cell_what {
                T(sl, &[D(1), D(2)])
            } else {
                D(7)
            };
            let tome = T(sl, &[what, D(0)]);
            let node = T(sl, &[D(tas!(b"chap")), tome]);
            let map = T(sl, &[node, D(0), D(0)]);
            T(sl, &[D(0), map])
        })
        .expect("tome should decode");
        let Hoon::BarCen(None, tomes) = res else {
            panic!("expected brcn");
        };
        let (what, arms) = &tomes["chap"];
        assert!(what.is_some());
        assert!(arms.is_empty());
    }
}

#[test]
fn basetype_note_limb_decoders_reject_unknown_tags() {
    let err = decode_tagged(tas!(b"base"), |sl| T(sl, &[D(tas!(b"zzzz")), D(0)])).unwrap_err();
    assert!(err.contains("basetype: unknown cell tag"), "{err}");
    let err = decode_tagged(tas!(b"base"), |_| D(tas!(b"zzzz"))).unwrap_err();
    assert!(err.contains("basetype: unknown tag"), "{err}");

    // %germ notes decode as Know; unknown note tags error
    let res = decode_tagged(tas!(b"note"), |sl| {
        let note = T(sl, &[D(tas!(b"germ")), D(tas!(b"foo"))]);
        let h = T(sl, &[D(0), D(1)]);
        T(sl, &[note, h])
    });
    assert_eq!(res, Ok(Hoon::Note(Note::Know(s("foo")), b(ax(1)))));
    let err = decode_tagged(tas!(b"note"), |sl| {
        let note = T(sl, &[D(tas!(b"zzzz")), D(0)]);
        let h = T(sl, &[D(0), D(1)]);
        T(sl, &[note, h])
    })
    .unwrap_err();
    assert!(err.contains("note: unknown tag"), "{err}");

    // a limb whose head is neither 0 nor 1
    let err = decode_tagged(tas!(b"wing"), |sl| {
        let limb = T(sl, &[D(2), D(0)]);
        T(sl, &[limb, D(0)])
    })
    .unwrap_err();
    assert!(err.contains("limb: unexpected head"), "{err}");
}

#[test]
fn skin_and_spec_decoders_reject_unknown_shapes() {
    // skin whose head is a cell, and skin with an unknown tag
    let err = decode_tagged(tas!(b"ktts"), |sl| {
        let h = T(sl, &[D(1), D(2)]);
        let skin = T(sl, &[h, D(3)]);
        T(sl, &[skin, D(0), D(1)])
    })
    .unwrap_err();
    assert!(err.contains("skin: unrecognized"), "{err}");
    let err = decode_tagged(tas!(b"ktts"), |sl| {
        let skin = T(sl, &[D(tas!(b"zzzz")), D(3)]);
        T(sl, &[skin, D(0), D(1)])
    })
    .unwrap_err();
    assert!(err.contains("skin: unrecognized"), "{err}");

    // a %gist whose note is not %help, and an unknown spec tag
    let err = decode_tagged(tas!(b"ktcl"), |sl| {
        let note = T(sl, &[D(tas!(b"know")), D(0)]);
        let base = T(sl, &[D(tas!(b"base")), D(tas!(b"noun"))]);
        T(sl, &[D(tas!(b"gist")), note, base])
    })
    .unwrap_err();
    assert!(err.contains("expected help"), "{err}");
    let err = decode_tagged(tas!(b"ktcl"), |sl| T(sl, &[D(tas!(b"zzzz")), D(0)])).unwrap_err();
    assert!(err.contains("spec: unknown tag"), "{err}");
}

#[test]
fn woof_and_term_or_pair_decoders_cover_both_shapes() {
    // a knit woof given as [0 hoon]
    let res = decode_tagged(tas!(b"knit"), |sl| {
        let h = T(sl, &[D(0), D(1)]);
        let woof = T(sl, &[D(0), h]);
        T(sl, &[woof, D(0)])
    });
    assert_eq!(res, Ok(Hoon::Knit(vec![Woof::Hoon(ax(1))])));
    // ~> with a bare term hint
    let res = decode_tagged(tas!(b"sggr"), |sl| {
        let h = T(sl, &[D(0), D(1)]);
        T(sl, &[D(tas!(b"foo")), h])
    });
    assert_eq!(res, Ok(Hoon::SigGar(TermOrPair::Term(s("foo")), b(ax(1)))));
}

#[test]
fn tuna_decoding_falls_back_to_manx_for_untagged_items() {
    // a marl item that is an atom cannot be a manx
    let err = decode_tagged(tas!(b"mcts"), |sl| T(sl, &[D(5), D(0)])).unwrap_err();
    assert!(err.contains("manx"), "{err}");
    // a marl item headed by an unknown atom is treated as a manx and fails
    let err = decode_tagged(tas!(b"mcts"), |sl| {
        let item = T(sl, &[D(tas!(b"zzzz")), D(0)]);
        T(sl, &[item, D(0)])
    })
    .unwrap_err();
    assert!(err.contains("marx"), "{err}");
}

#[test]
fn zpwt_arg_and_hoon_decoders_reject_unknown_tags() {
    let err = decode_tagged(tas!(b"zpwt"), |sl| {
        let arg = T(sl, &[D(tas!(b"zzzz")), D(0)]);
        let h = T(sl, &[D(0), D(1)]);
        T(sl, &[arg, h])
    })
    .unwrap_err();
    assert!(err.contains("zpwt_arg: unknown tag"), "{err}");
    // hoonc's bare-atom version form is not accepted by the hatch decoder
    assert!(decode_tagged(tas!(b"zpwt"), |sl| {
        let h = T(sl, &[D(0), D(1)]);
        T(sl, &[D(138), h])
    })
    .is_err());
    let err = decode_tagged(tas!(b"zzzz"), |_| D(0)).unwrap_err();
    assert!(err.contains("unknown tag") && err.contains("zzzz"), "{err}");
    let slab: NounSlab = NounSlab::new();
    assert!(decode(&slab, D(5)).unwrap_err().contains("expected cell"));
}

// ---------------------------------------------------------------------------
// map encoding: duplicate keys and mug collisions
// ---------------------------------------------------------------------------

#[test]
fn map_encoding_merges_keys_with_the_same_atom() {
    // "$" and "" both encode to the atom 0, so map_put_mug sees a repeat key.
    let same = tome(&[("a", rock(1))]);
    let mut dup_same = HashMap::new();
    dup_same.insert(s("$"), same.clone());
    dup_same.insert(String::new(), same.clone());
    let (slab, noun) = encode(&Hoon::BarCen(None, dup_same));
    let Ok(Hoon::BarCen(None, tomes)) = decode(&slab, noun) else {
        panic!("brcn should decode");
    };
    assert_eq!(tomes.len(), 1);
    assert_eq!(tomes["$"], same);

    let mut dup_diff = HashMap::new();
    dup_diff.insert(s("$"), same);
    dup_diff.insert(String::new(), tome(&[("b", rock(2))]));
    let (slab, noun) = encode(&Hoon::BarCen(None, dup_diff));
    let Ok(Hoon::BarCen(None, tomes)) = decode(&slab, noun) else {
        panic!("brcn should decode");
    };
    assert_eq!(tomes.len(), 1, "one entry replaces the other");
}

/// Two distinct four-letter terms whose atoms have the same mug.
fn mug_colliding_terms() -> (String, String) {
    let mut seen: HashMap<u32, String> = HashMap::new();
    let slab: NounSlab = NounSlab::new();
    let space = slab.noun_space();
    let letters = b"abcdefghijklmnopqrstuvwxyz";
    for a in letters {
        for bb in letters {
            for c in letters {
                for d in letters {
                    let word = [*a, *bb, *c, *d];
                    let n = u32::from_le_bytes(word) as u64;
                    let atom = D(n).as_atom().expect("direct atom");
                    let mug = calc_atom_mug_u32(atom, &space);
                    let text = String::from_utf8(word.to_vec()).expect("ascii");
                    if let Some(prev) = seen.insert(mug, text.clone()) {
                        return (prev, text);
                    }
                }
            }
        }
    }
    panic!("no mug collision among four-letter terms");
}

#[test]
fn map_encoding_orders_mug_collisions_with_dor() {
    let (k1, k2) = mug_colliding_terms();
    assert_ne!(k1, k2);
    let mut tomes = HashMap::new();
    tomes.insert(k1, tome(&[("a", rock(1))]));
    tomes.insert(k2, tome(&[("b", rock(2))]));
    let (slab, noun) = encode(&Hoon::BarCen(None, tomes.clone()));
    assert_eq!(decode(&slab, noun), Ok(Hoon::BarCen(None, tomes)));
}

// ---------------------------------------------------------------------------
// materialization cache
// ---------------------------------------------------------------------------

#[test]
fn cached_materialization_matches_direct_encoding_for_nested_forms() {
    let hoon = Hoon::Hand(
        b(Type::Hold(b(Type::NounExpr), Hoon::Lost(b(limb("a"))))),
        Nock::AxisSelect(1u64.into()),
    );
    let mut cached = NounSlab::new();
    let mut count = 0usize;
    let noun = hoon_to_noun_with_cache(&mut cached, &hoon, |_, _| count += 1);
    assert!(
        count >= 3,
        "outer hoon, hold gene, and its child are recorded"
    );
    let (mut direct_slab, direct) = encode(&hoon);
    cached.set_root(noun);
    direct_slab.set_root(direct);
    assert_eq!(cached.jam(), direct_slab.jam());
}

// ---------------------------------------------------------------------------
// rune parsers: postfix `::` docs (the parse only runs with docs enabled,
// which honk does for the prelude but not for entry files)
// ---------------------------------------------------------------------------

fn parse_src_dbug(src: &str, dbug: bool) -> Result<Hoon, String> {
    let linemap = Arc::new(LineMap::new_with_docs(src, true));
    crate::native_parser(vec![s("test"), s("p4.hoon")], dbug, linemap)
        .parse(src)
        .into_result()
        .map_err(|errs| format!("{errs:?}"))
}

#[test]
fn bartis_and_kethep_multiline_spec_take_trailing_line_doc() {
    let src = "|=  $:  a=@\n        b=@\n    ==  ::  sample doc\na\n";
    let Hoon::BarTis(spec, _) = parse_one(src) else {
        panic!("expected |=");
    };
    assert!(matches!(*spec, Spec::Gist(..)), "{spec:?}");

    let src = "^-  $:  a=@\n        b=@\n    ==  ::  cast doc\n[1 2]\n";
    let Hoon::KetHep(spec, _) = parse_one(src) else {
        panic!("expected ^-");
    };
    assert!(matches!(*spec, Spec::Gist(..)), "{spec:?}");
}

#[test]
fn barbuc_wide_and_barhep_docs() {
    let Hoon::BarBuc(sample, body) = parse_one("|$(a (list a))\n") else {
        panic!("expected |$");
    };
    assert_eq!(sample, vec![s("a")]);
    assert!(matches!(*body, Spec::Make(..)));
    // a doc line before a wide |$ does not become a body gist
    let h = parse_one("::  doc\n|$(a (list a))\n");
    assert!(!contains_gist(&h), "{h:?}");

    // |-  5  ::  doc  attaches to the body
    let Hoon::BarHep(body) = parse_one("|-  5  ::  loop doc\n") else {
        panic!("expected |-");
    };
    assert!(matches!(*body, Hoon::Note(Note::Help(_), _)));
    // nested: the inner |- already carries the doc, the outer keeps it once
    let Hoon::BarHep(outer) = parse_one("|-  |-  5  ::  loop doc\n") else {
        panic!("expected |-");
    };
    let Hoon::BarHep(inner) = *outer else {
        panic!("outer |- should not add a second note: {outer:?}");
    };
    assert!(matches!(*inner, Hoon::Note(Note::Help(_), _)));
}

#[test]
fn buclus_tall_attaches_postfix_doc_to_spec() {
    let h = parse_one("$+  foo  @  ::  standard doc\n");
    let debug = format!("{h:?}");
    assert!(debug.contains("BucLus(\"foo\", Gist("), "{debug}");

    let h = parse_one("$+  foo  @\n");
    let debug = format!("{h:?}");
    assert!(debug.contains("BucLus(\"foo\", Base("), "{debug}");
}

#[test]
fn cenhep_four_space_docs_attach_only_to_the_gate() {
    let Hoon::CenHep(p, q) = parse_one("%-  add  ::    four-space doc\n[1 2]\n") else {
        panic!("expected %-");
    };
    assert!(matches!(*p, Hoon::Note(Note::Help(_), _)), "{p:?}");
    assert!(!contains_help(&q));

    let Hoon::CenHep(p, q) = parse_one("%-  add\n[1 2]  ::    four-space doc\n") else {
        panic!("expected %-");
    };
    assert!(!contains_help(&p));
    assert!(
        !contains_help(&q),
        "a four-space doc on the sample is dropped"
    );
}

#[test]
fn col_runes_carry_four_space_docs_to_the_next_child() {
    let Hoon::ColHep(p, q) = parse_one(":-  1  ::    carried doc\n2\n") else {
        panic!("expected :-");
    };
    assert!(!contains_help(&p));
    assert!(matches!(*q, Hoon::Note(Note::Help(_), _)));

    let Hoon::ColCab(p, q) = parse_one(":_  1  ::    carried doc\n2\n") else {
        panic!("expected :_");
    };
    assert!(!contains_help(&p));
    assert!(matches!(*q, Hoon::Note(Note::Help(_), _)));

    let Hoon::ColLus(_, q, r) = parse_one(":+  1\n  2  ::    carried doc\n3\n") else {
        panic!("expected :+");
    };
    assert!(!contains_help(&q));
    assert!(matches!(*r, Hoon::Note(Note::Help(_), _)));

    let src = ":^  1  ::    carried p doc\n  2  ::    carried q doc\n  3\n4\n";
    let Hoon::ColKet(p, q, s_, r) = parse_one(src) else {
        panic!("expected :^");
    };
    assert!(!contains_help(&p));
    assert!(matches!(*q, Hoon::Note(Note::Help(_), _)));
    assert!(matches!(*s_, Hoon::Note(Note::Help(_), _)));
    assert!(!contains_help(&r));

    let Hoon::ColSig(outer) = parse_one("~[1 2]~\n") else {
        panic!("expected ~[..]~");
    };
    assert!(matches!(outer.as_slice(), [Hoon::ColSig(inner)] if inner.len() == 2));
}

#[test]
fn kettis_tall_wide_and_irregular_forms() {
    // doc after the face wraps p before it is flayed into a skin
    let Hoon::KetTis(skin, _) = parse_one("^=  a  ::  face doc\n5\n") else {
        panic!("expected ^=");
    };
    assert!(matches!(skin, Skin::Help(..)), "{skin:?}");
    // a traced, non-limb p still flays (dbug parse)
    let h = parse_src_dbug("^=  [a b]  [1 2]\n", true).expect("dbug parse");
    assert!(format!("{h:?}").contains("KetTis(Cell("), "{h:?}");
    // p that is not a valid skin
    assert!(parse_src("^=  +(1)  5\n").is_err());
    // hoonc also reads `^=(+(1) 5)` as `^` = `(+(1) 5)`, so this parses
    assert!(matches!(
        parse_one("^=(+(1) 5)\n"),
        Hoon::KetTis(Skin::Base(BaseType::Cell), _)
    ));
    let Hoon::KetTis(Skin::Term(name), _) = parse_one("^=(a 5)\n") else {
        panic!("expected ^=(a 5)");
    };
    assert_eq!(name, "a");
    // =spec names itself: =@ud is ^=(ud ^*(@ud))
    let Hoon::KetTis(Skin::Term(name), body) = parse_one("=@ud\n") else {
        panic!("expected irregular ^=");
    };
    assert_eq!(name, "ud");
    assert!(matches!(*body, Hoon::KetTar(_)));
    // an axis or a multi-limb wing is not a face
    assert!(parse_src("^=  +3  5\n").is_err());
    assert!(parse_src("^=  a.b  5\n").is_err());
    // p=q with a non-skin p
    assert!(parse_src("+(1)=5\n").is_err());
    assert!(parse_src("[+(1)=5 6]\n").is_err());
}

#[test]
fn nested_postfix_docs_are_not_duplicated() {
    // ^+, =/, =. keep the doc their |- child already carries
    for src in [
        "^+  |-  5  ::  cast doc\n6\n", "=/  a  |-  5  ::  bind doc\na\n",
        "=.  a  |-  6  ::  edit doc\na\n",
    ] {
        let h = parse_one(src);
        let debug = format!("{h:?}");
        assert_eq!(debug.matches("Help(").count(), 1, "{src}: {debug}");
    }
    let src = "?.  &\n  |-  1  ::  yes doc\n|-  2  ::  no doc\n";
    let Hoon::WutDot(_, q, r) = parse_one(src) else {
        panic!("expected ?.");
    };
    assert!(matches!(*q, Hoon::BarHep(_)));
    assert!(matches!(*r, Hoon::BarHep(_)));
    let Hoon::WutDot(_, q, r) = parse_one("?.  &\n  1  ::  yes doc\n2  ::  no doc\n") else {
        panic!("expected ?.");
    };
    assert!(matches!(*q, Hoon::Note(Note::Help(_), _)));
    assert!(matches!(*r, Hoon::Note(Note::Help(_), _)));
}

#[test]
fn wuthep_and_wutlus_case_docs() {
    let src = "?-  a\n  %a  |-  1  ::  case doc\n  %b  2\n==\n";
    let h = parse_one(src);
    assert_eq!(format!("{h:?}").matches("Help(").count(), 1, "{h:?}");

    let src = "?+  a  |-  0  ::  default doc\n  %1  ::  spec doc\n    |-  1  ::  case doc\n  %2  2  ::  plain case doc\n==\n";
    let h = parse_one(src);
    assert!(contains_gist(&h), "spec doc should become a gist: {h:?}");
    assert_eq!(format!("{h:?}").matches("Help(").count(), 3, "{h:?}");
}

#[test]
fn wuthax_forms() {
    // `?#  %a  b` flays %a into a leaf skin tested against wing b
    let h = parse_one("?#  %a  b\n");
    let debug = format!("{h:?}");
    assert!(debug.contains("WutHax(Leaf(\"tas\""), "{debug}");
    assert!(parse_one("?#(%a b)\n") == h);
    assert!(parse_src("?#(+(1) b)\n").is_err());
    assert!(parse_src("?#  +(1)  b\n").is_err());
}

#[test]
fn sig_wide_and_tall_hint_forms() {
    assert!(matches!(parse_one("~?(& %hi 5)\n"), Hoon::SigWut(0, ..)));
    assert!(matches!(parse_one("~?(> & %hi 5)\n"), Hoon::SigWut(1, ..)));
    for (src, want) in [
        ("~>  %foo  5\n", "gar"),
        ("~>(%foo 5)\n", "gar"),
        ("~<  %foo  5\n", "gal"),
        ("~<(%foo 5)\n", "gal"),
    ] {
        match (parse_one(src), want) {
            (Hoon::SigGar(TermOrPair::Term(t), _), "gar") => assert_eq!(t, "foo"),
            (Hoon::SigGal(TermOrPair::Term(t), _), "gal") => assert_eq!(t, "foo"),
            (other, _) => panic!("{src}: {other:?}"),
        }
    }
    assert!(matches!(
        parse_one("~<(%foo.5 6)\n"),
        Hoon::SigGal(TermOrPair::Pair(..), _)
    ));
    assert!(matches!(
        parse_one("=*(a 5 a)\n"),
        Hoon::TisTar((_, None), ..)
    ));
}

#[test]
fn buc_wide_and_irregular_spec_forms() {
    assert!(matches!(
        parse_one("$?(%a %b)\n"),
        Hoon::KetCol(spec) if matches!(*spec, Spec::BucWut(_, ref rest) if rest.len() == 1)
    ));
    assert!(matches!(
        parse_one("$%([%a p=@] [%b q=@])\n"),
        Hoon::KetCol(spec) if matches!(*spec, Spec::BucCen(..))
    ));
    assert!(matches!(
        parse_one("?(%a %b)\n"),
        Hoon::KetCol(spec) if matches!(*spec, Spec::BucWut(..))
    ));
    let h = parse_one("^-($:(a=@ b=@) [1 2])\n");
    assert!(format!("{h:?}").contains("BucCol("), "{h:?}");
    // `=a=@` currently names the face `a`; hoonc names it `a-atom`
    // (coverage/p4/divergent/p4_buctis_prefixed_autoname.hoon).
    let h = parse_one("^-([=a=@ b=@] [1 2])\n");
    assert!(format!("{h:?}").contains("BucTis(Term(\"a\")"), "{h:?}");
    // `=*`: a spec with no autoname cannot be named
    assert!(parse_src("^-([=* b=@] [1 2])\n").is_err());
    // a `%` spec constant that is not a dime
    assert!(parse_src("^-(%._1_2__ 5)\n").is_err());
}

#[test]
fn sail_attribute_values_become_beers() {
    let Hoon::Xray(manx) = parse_one(";div(title \"hi\");\n") else {
        panic!("expected sail");
    };
    let (_, beers) = &manx.g.a[0];
    assert_eq!(beers, &vec![Beer::Char(s("h")), Beer::Char(s("i"))]);
    let Hoon::Xray(manx) = parse_one(";div(title \"a{b}\");\n") else {
        panic!("expected sail");
    };
    let (_, beers) = &manx.g.a[0];
    assert!(
        matches!(beers.as_slice(), [Beer::Char(_), Beer::Hoon(_)]),
        "{beers:?}"
    );
    let Hoon::Xray(manx) = parse_one(";div(title (add 1 2));\n") else {
        panic!("expected sail");
    };
    assert!(matches!(manx.g.a[0].1.as_slice(), [Beer::Hoon(_)]));
    let Hoon::Xray(manx) = parse_one(";foo_bar#x;\n") else {
        panic!("expected sail");
    };
    assert_eq!(manx.g.n, Mane::TagSpace(s("foo"), s("bar")));
    assert_eq!(manx.g.a[0].0, Mane::Tag(s("id")));
    assert!(matches!(
        parse_one(";-  \"x\"\n"),
        Hoon::MicTis(items) if matches!(items.as_slice(), [Tuna::TunaTail(TunaTail::Tape(_))])
    ));
}

// ---------------------------------------------------------------------------
// parser builders that the main grammar never wires in
// ---------------------------------------------------------------------------

fn word<'src>() -> impl ParserExt<'src, Hoon> {
    crate::utils::symbol().map(Hoon::Limb).boxed()
}

#[test]
fn unwired_fas_and_bucmic_builders_still_parse() {
    use crate::runes::*;
    assert_eq!(
        fastis(word(), word()).parse("  a  b  c").into_result(),
        Ok(limb("c"))
    );
    assert_eq!(
        fastar(word(), word()).parse("  a  b  c  d").into_result(),
        Ok(limb("d"))
    );
    assert_eq!(
        fashax(word(), word()).parse("  a  b").into_result(),
        Ok(limb("b"))
    );
    assert_eq!(
        bucmic(word()).parse("a").into_result(),
        Ok(Hoon::KetCol(b(Spec::BucMic(limb("a")))))
    );
    assert_eq!(
        bucmic_wide(word()).parse("a").into_result(),
        Ok(Hoon::KetCol(b(Spec::BucMic(limb("a")))))
    );
}

// ---------------------------------------------------------------------------
// ast/hoon.rs helpers
// ---------------------------------------------------------------------------

#[test]
fn parsed_atom_conversions() {
    assert_eq!(ParsedAtom::from(7u16), ParsedAtom::Small(7));
    assert_eq!(ParsedAtom::from(7u32), ParsedAtom::Small(7));
    assert_eq!(ParsedAtom::from(7u64), ParsedAtom::Small(7));
    let huge = ParsedAtom::Big((BigUint::from(1u8) << 130) | BigUint::from(0x1234u32));
    let big_small = ParsedAtom::Big(BigUint::from(200u8));

    assert_eq!(ParsedAtom::Small(200).to_u8(), Some(200));
    assert_eq!(ParsedAtom::Small(300).to_u8(), None);
    assert_eq!(big_small.to_u8(), Some(200));
    assert_eq!(huge.to_u8(), None);
    assert_eq!(ParsedAtom::Small(5).to_u32(), Some(5));
    assert_eq!(big_small.to_u32(), Some(200));
    assert_eq!(huge.to_u32(), None);
    assert_eq!(big_small.to_u128(), Some(200));
    assert_eq!(huge.to_u128(), None);
    assert_eq!(
        huge.to_biguint(),
        (BigUint::from(1u8) << 130) | BigUint::from(0x1234u32)
    );
    assert_eq!(ParsedAtom::zero(), ParsedAtom::Small(0));
    assert!(ParsedAtom::zero().is_zero());

    assert_eq!(ParsedAtom::Small(5).to_u64_lossy(), 5);
    assert_eq!(huge.to_u64_lossy(), 0x1234);
    assert_eq!(ParsedAtom::Small(0x1ff).to_u8_lossy(), 0xff);
    assert_eq!(ParsedAtom::Small(0x1_ffff).to_u16_lossy(), 0xffff);

    assert_eq!(ParsedAtom::from("ab"), ParsedAtom::Small(0x6261));
    let long = "abcdefghijklmnopqrstuvwxyz";
    assert_eq!(
        ParsedAtom::from(long),
        ParsedAtom::Big(BigUint::from_bytes_le(long.as_bytes()))
    );
}

#[test]
fn parsed_atom_ordering_and_bitor() {
    use std::cmp::Ordering::*;
    let small = ParsedAtom::Small(3);
    let huge = ParsedAtom::Big(BigUint::from(1u8) << 140);
    let huger = ParsedAtom::Big(BigUint::from(1u8) << 150);
    assert_eq!(small.cmp(&ParsedAtom::Small(4)), Less);
    assert_eq!(small.cmp(&huge), Less);
    assert_eq!(huge.cmp(&small), Greater);
    assert_eq!(huge.cmp(&huger), Less);
    assert_eq!(huge.cmp(&huge.clone()), Equal);
    assert!(small.lt(&huge));
    assert!(small.le(&small.clone()));
    assert!(huge.gt(&small));
    assert!(huge.ge(&huge.clone()));
    assert!(ParsedAtom::eq(&small, &ParsedAtom::Small(3)));

    assert_eq!(
        ParsedAtom::Small(1) | ParsedAtom::Small(2),
        ParsedAtom::Small(3)
    );
    let bit140: BigUint = BigUint::from(1u8) << 140;
    assert_eq!(
        ParsedAtom::Small(1) | huge.clone(),
        ParsedAtom::Big(bit140.clone() | BigUint::from(1u8))
    );
    assert_eq!(
        huge.clone() | ParsedAtom::Small(2),
        ParsedAtom::Big(bit140.clone() | BigUint::from(2u8))
    );
    assert_eq!(
        huge | huger,
        ParsedAtom::Big(bit140 | (BigUint::from(1u8) << 150))
    );
}

#[test]
fn binary_float_sign_and_axis_rendering() {
    let finite = BinaryFloat::Finite {
        sign: true,
        exp: 0,
        mant: BigUint::from(1u8),
    };
    assert!(finite.sign());
    assert!(!BinaryFloat::Infinity { sign: false }.sign());
    assert!(BinaryFloat::Infinity { sign: true }.sign());
    assert!(!BinaryFloat::NaN.sign());

    let small = Axis::from(5u64);
    let huge = Axis::from(BigUint::from(1u8) << 70);
    assert_eq!(small.to_u64(), Some(5));
    assert_eq!(huge.to_u64(), None);
    assert_eq!(format!("{small}"), "5");
    assert_eq!(format!("{small:?}"), "5");
    assert_eq!(serde_json::to_string(&small).unwrap(), "5");
    assert_eq!(
        serde_json::to_string(&huge).unwrap(),
        format!("\"{}\"", BigUint::from(1u8) << 70)
    );
    assert_eq!(
        serde_json::to_string(&ParsedAtom::Big(BigUint::from(1u8) << 130)).unwrap(),
        format!("{{\"Big\":\"{}\"}}", BigUint::from(1u8) << 130)
    );
}

// ---------------------------------------------------------------------------
// collect_inputs and diff_and_report
// ---------------------------------------------------------------------------

#[test]
fn collect_inputs_walks_directories_for_hoon_files() {
    let root = std::env::temp_dir().join(format!("hatch-p4-inputs-{}", std::process::id()));
    let sub = root.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(root.join("b.hoon"), "~").unwrap();
    std::fs::write(root.join("notes.txt"), "x").unwrap();
    std::fs::write(sub.join("a.hoon"), "~").unwrap();

    let found = collect_inputs(&root);
    assert_eq!(found, vec![root.join("b.hoon"), sub.join("a.hoon")]);
    assert_eq!(
        collect_inputs(&root.join("b.hoon")),
        vec![root.join("b.hoon")]
    );
    assert!(collect_inputs(&root.join("notes.txt")).is_empty());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn diff_and_report_handles_equal_and_different_nouns() {
    let mut slab: NounSlab = NounSlab::new();
    let a = T(&mut slab, &[D(1), D(2)]);
    let c = T(&mut slab, &[D(1), D(3)]);
    let space = slab.noun_space();
    diff_and_report(a.in_space(&space), a.in_space(&space));
    diff_and_report(a.in_space(&space), c.in_space(&space));
}

#[test]
fn kettis_doc_on_a_cell_face_and_sail_utf8_attribute() {
    // a postfix doc after a non-limb face wraps the pattern before flay
    let h = parse_one("^=  [a b]  ::  pair doc\n[1 2]\n");
    assert!(matches!(h, Hoon::KetTis(..)), "{h:?}");

    // Pins the behavior recorded in coverage/p4/divergent/p4_sail_attr_utf8:
    // the tape lexer yields the two UTF-8 bytes of "é" as two woofs, but
    // runes/sail.rs re-encodes each byte as a code point (hoonc keeps the
    // bytes).
    let Hoon::Xray(manx) = parse_one(";div(title \"h\u{e9}\");\n") else {
        panic!("expected sail");
    };
    assert_eq!(manx.g.a[0].1.len(), 3, "{:?}", manx.g.a[0].1);
}
