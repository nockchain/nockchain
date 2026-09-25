//! Coverage-driven tests for `ut/mod.rs` (first third).
//!
//! Added to close branch-coverage gaps; see the coverage report in the PR.

use std::collections::HashMap;
use std::path::Path as FsPath;

#[allow(unused_imports)]
use super::super::*;

type TestResult<T> = std::result::Result<T, String>;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn try_parse(src: &str) -> TestResult<Hoon> {
    let gen = crate::pipeline::parse_native_hoon_source_without_docs(
        FsPath::new("c1-cov.hoon"),
        src,
        Vec::new(),
        false,
    )
    .map_err(|err| format!("parse {src:?}: {err:?}"))?;
    // A source file parses as a one-item `=~`; peel it so the tests reach the
    // rune under test directly.
    Ok(match gen {
        Hoon::TisSig(mut items) if items.len() == 1 => items.pop().expect("one item"),
        other => other,
    })
}

fn parse(src: &str) -> Hoon {
    try_parse(src).unwrap_or_else(|err| panic!("{err}"))
}

/// Mint `gen` against a `%noun` subject and goal; the result is the jam of
/// `[type formula]`, or the error text.
fn mint_gen_jam(gen: &Hoon) -> TestResult<Vec<u8>> {
    let mut slab = NounSlab::new();
    let pair = {
        let mut ut = Ut::new(&mut slab);
        let sut = ty_noun(&mut *ut.slab);
        let gol = ty_noun(&mut *ut.slab);
        let (ty, formula) = ut
            .mint_noun(sut, gol, gen)
            .map_err(|err| format!("{err:?}"))?;
        T(&mut *ut.slab, &[ty, formula])
    };
    slab.set_root(pair);
    Ok(slab.jam().to_vec())
}

fn mint_jam(src: &str) -> TestResult<Vec<u8>> {
    mint_gen_jam(&try_parse(src)?)
}

/// Play `gen` against a `%noun` subject; the result is the jam of the type.
fn play_gen_jam(gen: &Hoon) -> TestResult<Vec<u8>> {
    let mut slab = NounSlab::new();
    let ty = {
        let mut ut = Ut::new(&mut slab);
        let sut = ty_noun(&mut *ut.slab);
        ut.play_noun(sut, gen).map_err(|err| format!("{err:?}"))?
    };
    slab.set_root(ty);
    Ok(slab.jam().to_vec())
}

fn play_jam(src: &str) -> TestResult<Vec<u8>> {
    play_gen_jam(&try_parse(src)?)
}

/// Play `gen` against a `%noun` subject and return the top-level type tag.
fn play_tag(gen: &Hoon) -> String {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let sut = ty_noun(&mut *ut.slab);
    let ty = ut.play_noun(sut, gen).expect("play");
    let space = ut.slab.noun_space();
    type_tag(ty, &space).expect("type tag")
}

fn assert_mint_same(src: &str, equivalent: &str) {
    let left = mint_jam(src).unwrap_or_else(|err| panic!("mint {src:?}: {err}"));
    let right = mint_jam(equivalent).unwrap_or_else(|err| panic!("mint {equivalent:?}: {err}"));
    assert_eq!(left, right, "mint {src:?} should equal mint {equivalent:?}");
}

fn play_same(src: &str, equivalent: &str) -> TestResult<()> {
    let left = play_jam(src).map_err(|err| format!("play {src:?}: {err}"))?;
    let right = play_jam(equivalent).map_err(|err| format!("play {equivalent:?}: {err}"))?;
    if left != right {
        return Err(format!("play {src:?} should equal play {equivalent:?}"));
    }
    Ok(())
}

fn rock(aura: &str, value: u128) -> Hoon {
    Hoon::Rock(
        aura.to_string(),
        NounExpr::ParsedAtom(ParsedAtom::Small(value)),
    )
}

fn sand(aura: &str, value: u128) -> Hoon {
    Hoon::Sand(
        aura.to_string(),
        NounExpr::ParsedAtom(ParsedAtom::Small(value)),
    )
}

fn axis(n: u64) -> Hoon {
    Hoon::Axis(n.into())
}

fn bx(hoon: Hoon) -> Box<Hoon> {
    Box::new(hoon)
}

fn spot(file: &str, line: u64) -> Spot {
    Spot {
        p: vec![file.to_string()],
        q: Pint {
            p: (line, 1),
            q: (line, 9),
        },
    }
}

fn term_atom(name: &str) -> ParsedAtom {
    string_to_atom(name.to_string())
}

fn sig(hoon: &Hoon) -> HoonSignature {
    Sig64::hoon_signature_spot_sensitive(hoon).expect("every Hoon has a signature")
}

// ---------------------------------------------------------------------------
// Sig64: AST signatures (cache guards)
// ---------------------------------------------------------------------------

fn sample_spec() -> Spec {
    Spec::Base(BaseType::Atom("ud".to_string()))
}

fn spec_map(specs: &[(&str, Spec)]) -> HashMap<String, Spec> {
    specs
        .iter()
        .map(|(name, spec)| (name.to_string(), spec.clone()))
        .collect()
}

/// Every spec shape the signature writer distinguishes, including the ones
/// the parity corpus never produces.
fn spec_zoo() -> Vec<Spec> {
    let a = sample_spec();
    let b = Spec::Base(BaseType::Cell);
    let map = spec_map(&[("x", a.clone()), ("y", b.clone())]);
    let wing = vec![Limb::Term("w".to_string())];
    vec![
        a.clone(),
        Spec::Base(BaseType::Void),
        Spec::Base(BaseType::Null),
        Spec::Base(BaseType::Flag),
        Spec::Base(BaseType::NounExpr),
        Spec::Dbug(spot("s.hoon", 1), Box::new(a.clone())),
        Spec::Gist(
            NounExpr::ParsedAtom(ParsedAtom::Small(1)),
            Box::new(a.clone()),
        ),
        Spec::Leaf("tas".to_string(), term_atom("foo")),
        Spec::Like(wing.clone(), vec![wing.clone()]),
        Spec::Loop("lp".to_string()),
        Spec::Made(
            ("m".to_string(), vec!["arg".to_string()]),
            Box::new(a.clone()),
        ),
        Spec::Make(Hoon::Limb("mk".to_string()), vec![a.clone()]),
        Spec::Name("nm".to_string(), Box::new(a.clone())),
        Spec::Over(wing.clone(), Box::new(a.clone())),
        Spec::BucGar(Box::new(a.clone()), Box::new(b.clone())),
        Spec::BucBuc(Box::new(a.clone()), map.clone()),
        Spec::BucBar(Box::new(a.clone()), rock("f", 0)),
        Spec::BucCab(rock("ud", 1)),
        Spec::BucCol(Box::new(a.clone()), vec![b.clone()]),
        Spec::BucCen(Box::new(a.clone()), vec![b.clone()]),
        Spec::BucDot(Box::new(a.clone()), map.clone()),
        Spec::BucGal(Box::new(a.clone()), Box::new(b.clone())),
        Spec::BucHep(Box::new(a.clone()), Box::new(b.clone())),
        Spec::BucKet(Box::new(a.clone()), Box::new(b.clone())),
        Spec::BucLus("lus".to_string(), Box::new(a.clone())),
        Spec::BucFas(Box::new(a.clone()), map.clone()),
        Spec::BucMic(rock("ud", 2)),
        Spec::BucPam(Box::new(a.clone()), rock("ud", 3)),
        Spec::BucSig(rock("ud", 4), Box::new(a.clone())),
        Spec::BucTic(Box::new(a.clone()), map.clone()),
        Spec::BucTis(Skin::Term("t".to_string()), Box::new(a.clone())),
        Spec::BucPat(Box::new(a.clone()), Box::new(b.clone())),
        Spec::BucWut(Box::new(a.clone()), vec![b.clone()]),
        Spec::BucZap(Box::new(a), map),
    ]
}

fn skin_zoo() -> Vec<Skin> {
    let inner = Skin::Term("t".to_string());
    vec![
        inner.clone(),
        Skin::Base(BaseType::Void),
        Skin::Cell(Box::new(inner.clone()), Box::new(inner.clone())),
        Skin::Dbug(spot("k.hoon", 2), Box::new(inner.clone())),
        Skin::Help(
            NounExpr::ParsedAtom(ParsedAtom::Small(7)),
            Box::new(inner.clone()),
        ),
        Skin::Leaf("tas".to_string(), term_atom("lef")),
        Skin::Name("nm".to_string(), Box::new(inner.clone())),
        Skin::Over(vec![Limb::Axis(2u64.into())], Box::new(inner.clone())),
        Skin::Spec(Box::new(sample_spec()), Box::new(inner)),
        Skin::Wash(3),
    ]
}

fn nock_zoo() -> Vec<Nock> {
    let c = |n: u128| Nock::Const(NounExpr::ParsedAtom(ParsedAtom::Small(n)));
    let b = |n: u128| Box::new(c(n));
    vec![
        Nock::Pair(b(1), b(2)),
        c(3),
        Nock::Compose(b(1), b(2)),
        Nock::CellTest(b(1)),
        Nock::Increment(b(1)),
        Nock::Equality(b(1), b(2)),
        Nock::IfThenElse(b(1), b(2), b(3)),
        Nock::SerialCompose(b(1), b(2)),
        Nock::PushSubject(b(1), b(2)),
        Nock::SelectArm(2u64.into(), b(1)),
        Nock::Edit((6u64.into(), b(1)), b(2)),
        Nock::Hint(NockHint::ParsedAtom(5), b(1)),
        Nock::Hint(NockHint::Pair(5, b(2)), b(1)),
        Nock::GrabData(b(1), b(2)),
        Nock::AxisSelect(7u64.into()),
    ]
}

fn type_zoo() -> Vec<Type> {
    let stencil = Stencil::Half {
        left: Box::new(Stencil::Full {
            blocks: vec![vec![vec!["a".to_string(), "b".to_string()]]],
        }),
        rite: Box::new(Stencil::Lazy {
            fragment: 2u64.into(),
            resolve: (Box::new(sample_spec()), Box::new(sample_spec())),
        }),
    };
    let mut tomes = HashMap::new();
    let mut arms = HashMap::new();
    arms.insert("arm".to_string(), rock("ud", 1));
    tomes.insert(
        "chap".to_string(),
        (Some(NounExpr::ParsedAtom(ParsedAtom::Small(9))), arms),
    );
    let coil = |name: Option<&str>, poly: AstPoly, vair: AstVair| Coil {
        p: Garb {
            name: name.map(str::to_string),
            poly,
            vair,
        },
        q: Type::NounExpr,
        r: (
            (stencil.clone(), NounExpr::ParsedAtom(ParsedAtom::Small(0))),
            tomes.clone(),
        ),
    };
    let tune: Tune = (HashMap::from([("f".to_string(), None)]), vec![axis(1)]);
    vec![
        Type::NounExpr,
        Type::Void,
        Type::ParsedAtom("ud".to_string(), None),
        Type::ParsedAtom("ud".to_string(), Some(4)),
        Type::Cell(Box::new(Type::NounExpr), Box::new(Type::Void)),
        Type::Core(
            Box::new(Type::NounExpr),
            Box::new(coil(Some("core"), AstPoly::Wet, AstVair::Gold)),
        ),
        Type::Core(
            Box::new(Type::NounExpr),
            Box::new(coil(None, AstPoly::Dry, AstVair::Iron)),
        ),
        Type::Core(
            Box::new(Type::NounExpr),
            Box::new(coil(None, AstPoly::Dry, AstVair::Lead)),
        ),
        Type::Core(
            Box::new(Type::NounExpr),
            Box::new(coil(None, AstPoly::Dry, AstVair::Zinc)),
        ),
        Type::Face(FaceType::Term("fa".to_string()), Box::new(Type::NounExpr)),
        Type::Face(FaceType::Tune(tune), Box::new(Type::NounExpr)),
        Type::Fork(vec![Type::NounExpr, Type::Void]),
        Type::Hint(
            (
                Box::new(Type::NounExpr),
                Note::Made("made".to_string(), None),
            ),
            Box::new(Type::Void),
        ),
        Type::Hold(Box::new(Type::NounExpr), axis(1)),
    ]
}

fn manx_sample() -> Manx {
    Manx {
        g: Marx {
            n: Mane::TagSpace("ns".to_string(), "tag".to_string()),
            a: vec![(
                Mane::Tag("href".to_string()),
                vec![Beer::Char("x".to_string()), Beer::Hoon(rock("t", 1))],
            )],
        },
        c: vec![
            Tuna::Manx(Manx {
                g: Marx {
                    n: Mane::Tag("p".to_string()),
                    a: vec![],
                },
                c: vec![],
            }),
            Tuna::TunaTail(TunaTail::Tape(rock("t", 2))),
            Tuna::TunaTail(TunaTail::Manx(rock("t", 3))),
            Tuna::TunaTail(TunaTail::Marl(rock("t", 4))),
            Tuna::TunaTail(TunaTail::Call(rock("t", 5))),
        ],
    }
}

/// One node per `write_hoon` variant (and helper shape) not reached by the
/// parity corpus. Each must get a signature, and all must be distinct.
fn hoon_zoo() -> Vec<Hoon> {
    let a = || bx(rock("ud", 1));
    let b = || bx(rock("ud", 2));
    let wing = || vec![Limb::Term("w".to_string())];
    let mut arms = HashMap::new();
    arms.insert("arm".to_string(), rock("ud", 5));
    let mut tomes = HashMap::new();
    tomes.insert(
        "chap".to_string(),
        (Some(NounExpr::ParsedAtom(ParsedAtom::Small(1))), arms),
    );
    let mut out = vec![
        Hoon::Base(BaseType::Void),
        Hoon::Bust(BaseType::Void),
        Hoon::Note(
            Note::Made("made".to_string(), Some(vec![wing(), wing()])),
            a(),
        ),
        Hoon::Note(Note::Made("made".to_string(), None), a()),
        Hoon::Note(Note::Help(NounExpr::ParsedAtom(ParsedAtom::Small(1))), a()),
        Hoon::Note(Note::Know("kn".to_string()), a()),
        Hoon::Eror("boom".to_string()),
        Hoon::Leaf("tas".to_string(), term_atom("leaf")),
        Hoon::Tell(vec![rock("ud", 1), rock("ud", 2)]),
        Hoon::Yell(vec![rock("ud", 1)]),
        Hoon::Xray(manx_sample()),
        Hoon::MicTis(manx_sample().c),
        Hoon::Knit(vec![
            Woof::ParsedAtom(ParsedAtom::Small(1)),
            Woof::Hoon(rock("t", 1)),
        ]),
        Hoon::Tune(TermOrTune::Term("tn".to_string())),
        Hoon::Tune(TermOrTune::Tune((
            HashMap::from([("none".to_string(), None), ("some".to_string(), Some(rock("ud", 1)))]),
            vec![rock("ud", 3)],
        ))),
        Hoon::BarBuc(
            vec!["a".to_string(), "b".to_string()],
            Box::new(sample_spec()),
        ),
        Hoon::BarCab(
            Box::new(sample_spec()),
            vec![("al".to_string(), rock("ud", 1))],
            tomes.clone(),
        ),
        Hoon::BarCen(Some("pre".to_string()), tomes.clone()),
        Hoon::BarCen(None, tomes.clone()),
        Hoon::BarKet(a(), tomes.clone()),
        Hoon::BarSig(Box::new(sample_spec()), a()),
        Hoon::BarPat(Some("pre".to_string()), tomes.clone()),
        Hoon::BarPat(None, tomes),
        Hoon::BarWut(a()),
        Hoon::ColCab(a(), b()),
        Hoon::ColKet(a(), b(), a(), b()),
        Hoon::CenCab(wing(), vec![(wing(), rock("ud", 1))]),
        Hoon::CenKet(a(), b(), a(), b()),
        Hoon::SigBar(a(), b()),
        Hoon::SigBuc("buc".to_string(), a()),
        Hoon::SigTis(a(), b()),
        Hoon::SigCen(
            Chum::StdKel("std".to_string(), ParsedAtom::Small(1)),
            a(),
            vec![("hook".to_string(), rock("ud", 1))],
            b(),
        ),
        Hoon::SigFas(
            Chum::VenProKel("ven".to_string(), "pro".to_string(), ParsedAtom::Small(2)),
            a(),
        ),
        Hoon::SigFas(
            Chum::VenProVerKel(
                "ven".to_string(),
                "pro".to_string(),
                ParsedAtom::Small(2),
                ParsedAtom::Small(3),
            ),
            a(),
        ),
        Hoon::SigFas(Chum::Lef("lef".to_string()), a()),
        Hoon::MicCol(a(), vec![rock("ud", 3)]),
        Hoon::MicFas(a()),
        Hoon::MicGal(Box::new(sample_spec()), a(), b(), a()),
        Hoon::MicSig(a(), vec![rock("ud", 3)]),
        Hoon::MicMic(Box::new(sample_spec()), a()),
        Hoon::TisCol(vec![(wing(), rock("ud", 1))], b()),
        Hoon::TisMic(Skin::Term("s".to_string()), a(), b()),
        Hoon::TisWut(wing(), a(), b(), a()),
        Hoon::TisHep(a(), b()),
        Hoon::TisKet(Skin::Term("s".to_string()), wing(), a(), b()),
        Hoon::TisTar(("tt".to_string(), Some(Box::new(sample_spec()))), a(), b()),
        Hoon::TisTar(("tt".to_string(), None), a(), b()),
        Hoon::TisCom(a(), b()),
        Hoon::ZapWut(ZpwtArg::ParsedAtom("138".to_string()), a()),
        Hoon::ZapWut(ZpwtArg::Pair("1".to_string(), "2".to_string()), a()),
        Hoon::Dbug(spot("d.hoon", 3), a()),
        Hoon::Lost(a()),
        Hoon::Fits(a(), wing()),
    ];
    for typ in type_zoo() {
        for nock in nock_zoo() {
            out.push(Hoon::Hand(Box::new(typ.clone()), nock));
        }
    }
    for spec in spec_zoo() {
        out.push(Hoon::KetCol(Box::new(spec)));
    }
    for skin in skin_zoo() {
        out.push(Hoon::KetTis(skin, a()));
    }
    out
}

#[test]
fn sig64_distinguishes_every_rare_hoon_spec_skin_type_and_nock_shape() {
    let zoo = hoon_zoo();
    let mut seen = HashMap::new();
    for (index, hoon) in zoo.iter().enumerate() {
        let signature = sig(hoon);
        if let Some(previous) = seen.insert(signature, index) {
            panic!(
                "signature collision between zoo[{previous}] {:?} and zoo[{index}] {:?}",
                zoo[previous], hoon
            );
        }
        // Signatures are structural: an equal clone at a new address agrees.
        assert_eq!(sig(&hoon.clone()), signature, "zoo[{index}] clone");
    }
    // Spec signatures cover the same spec writer through the spec entry point.
    let mut spec_sigs = HashMap::new();
    for spec in spec_zoo() {
        let signature = Sig64::spec_signature_spot_sensitive(&spec).expect("spec signature");
        assert!(
            spec_sigs.insert(signature, spec.clone()).is_none(),
            "spec signature collision for {spec:?}"
        );
    }
}

#[test]
fn sig64_spot_insensitive_mode_ignores_dbug_spots() {
    let make = |line: u64| {
        let spec = Spec::Dbug(spot("s.hoon", line), Box::new(sample_spec()));
        let skin = Skin::Dbug(spot("k.hoon", line), Box::new(Skin::Term("t".to_string())));
        Hoon::Dbug(
            spot("h.hoon", line),
            bx(Hoon::Pair(
                bx(Hoon::KetCol(Box::new(spec))),
                bx(Hoon::KetTis(skin, bx(rock("ud", 1)))),
            )),
        )
    };
    let early = make(1);
    let late = make(2);
    let insensitive = |hoon: &Hoon| {
        let mut writer = Sig64::new_with_dbug_spots(false);
        writer.write_hoon(hoon).expect("write");
        writer
            .hoon_signatures
            .get(&HoonIdentity::of(hoon))
            .copied()
            .expect("root signature")
    };
    assert_eq!(insensitive(&early), insensitive(&late));
    assert_ne!(sig(&early), sig(&late), "spot-sensitive signatures differ");
}

#[test]
fn sig64_reuses_a_digest_for_a_node_written_twice() {
    let node = Hoon::Pair(bx(rock("ud", 1)), bx(rock("ud", 2)));
    let mut writer = Sig64::new_with_dbug_spots(true);
    writer.write_hoon(&node).expect("first write");
    let nodes_after_first = writer.hoon_nodes.len();
    let state_after_first = writer.state;
    // The second write of the same node hits the per-writer memo: no new
    // build nodes, but the parent state still absorbs the digest.
    writer.write_hoon(&node).expect("second write");
    assert_eq!(writer.hoon_nodes.len(), nodes_after_first);
    assert_ne!(writer.state, state_after_first);
}

#[test]
fn hoon_arena_unsigned_root_registers_a_single_unsigned_entry() {
    let root = Hoon::Pair(bx(axis(2)), bx(axis(3)));
    let mut arena = HoonArena::default();
    arena.register_unsigned_root(HoonIdentity::of(&root));
    assert_eq!(arena.entries.len(), 1);
    let id = arena.id_for(&root).expect("root id");
    assert_eq!(id, HoonId(0));
    assert!(arena.entry(id).signature.is_none());
    assert_eq!(arena.child_count(id), 0);
    assert_eq!(arena.source_ptr(id), &root as *const Hoon);
    // Registering again replaces the previous graph.
    let other = axis(7);
    arena.register_unsigned_root(HoonIdentity::of(&other));
    assert!(arena.id_for(&root).is_none());
    assert_eq!(arena.id_for(&other), Some(HoonId(0)));
}

// ---------------------------------------------------------------------------
// `lower_*` desugarings (hoon-138 `++open`)
// ---------------------------------------------------------------------------

#[test]
fn lower_coltar_handles_empty_single_and_many() {
    assert_eq!(Ut::lower_coltar(&[]), Hoon::ZapZap);
    assert_eq!(Ut::lower_coltar(&[axis(2)]), axis(2));
    assert_eq!(
        Ut::lower_coltar(&[axis(2), axis(6), axis(7)]),
        Hoon::Pair(bx(axis(2)), bx(Hoon::Pair(bx(axis(6)), bx(axis(7)))))
    );
}

#[test]
fn lower_tisdot_cenhep_and_wtbr_match_open() {
    let wing = vec![Limb::Term("a".to_string())];
    assert_eq!(
        Ut::lower_tisdot(&wing, &axis(2), &axis(3)),
        Hoon::TisGar(
            bx(Hoon::CenCab(
                vec![Limb::Axis(1u64.into())],
                vec![(wing.clone(), axis(2))]
            )),
            bx(axis(3))
        )
    );
    assert_eq!(
        Ut::lower_cenhep(&axis(2), &axis(3)),
        Hoon::CenCol(bx(axis(2)), vec![axis(3)])
    );
    assert_eq!(Ut::lower_wtbr(&[]), rock("f", 1));
    assert_eq!(
        Ut::lower_wtbr(&[axis(2)]),
        Hoon::WutCol(bx(axis(2)), bx(rock("f", 0)), bx(rock("f", 1)))
    );
}

#[test]
fn lower_sigbar_uses_feck_for_tas_sand_and_a_cain_trap_otherwise() {
    let body = rock("ud", 5);
    let tas = Hoon::Sand("tas".to_string(), NounExpr::ParsedAtom(term_atom("foo")));
    let expected_rock = Hoon::SigGar(
        TermOrPair::Pair(
            "mean".to_string(),
            bx(Hoon::Rock(
                "tas".to_string(),
                NounExpr::ParsedAtom(term_atom("foo")),
            )),
        ),
        bx(body.clone()),
    );
    assert_eq!(Ut::lower_sigbar(&tas, &body), expected_rock);
    // `feck` looks through `%dbug` wrappers.
    let wrapped = Hoon::Dbug(spot("f.hoon", 1), bx(tas.clone()));
    assert_eq!(Ut::lower_sigbar(&wrapped, &body), expected_rock);

    // A cell-valued `%tas` sand, a non-`%tas` sand, and any other hoon all
    // fall back to the `|.((cain !>(=>(+3 p))))` trap.
    let cell_sand = Hoon::Sand(
        "tas".to_string(),
        NounExpr::Cell(
            Box::new(NounExpr::ParsedAtom(ParsedAtom::Small(1))),
            Box::new(NounExpr::ParsedAtom(ParsedAtom::Small(2))),
        ),
    );
    for p in [cell_sand, sand("ud", 1), axis(2)] {
        let lowered = Ut::lower_sigbar(&p, &body);
        let Hoon::SigGar(TermOrPair::Pair(name, mean), q) = lowered else {
            panic!("sigbar must lower to a %mean sggr");
        };
        assert_eq!(name, "mean");
        assert_eq!(*q, body);
        let expected = Hoon::BarDot(bx(Hoon::CenCol(
            bx(Hoon::Limb("cain".to_string())),
            vec![Hoon::ZapGar(bx(Hoon::TisGar(bx(axis(3)), bx(p.clone()))))],
        )));
        assert_eq!(*mean, expected);
    }
}

#[test]
fn lower_miccol_matches_open_for_empty_single_and_chains() {
    let p = Hoon::Limb("f".to_string());
    assert_eq!(Ut::lower_miccol(&p, &[]), Hoon::ZapZap);
    assert_eq!(Ut::lower_miccol(&p, &[axis(2)]), axis(2));
    let tsgr3 = |h: Hoon| Hoon::TisGar(bx(axis(3)), bx(h));
    assert_eq!(
        Ut::lower_miccol(&p, &[axis(2), axis(6), axis(7)]),
        Hoon::TisLus(
            bx(p.clone()),
            bx(Hoon::CenCol(
                bx(axis(2)),
                vec![
                    tsgr3(axis(2)),
                    Hoon::CenCol(bx(axis(2)), vec![tsgr3(axis(6)), tsgr3(axis(7))]),
                ],
            )),
        )
    );
}

#[test]
fn lower_sigcen_encodes_each_tyre_hook() {
    let chum = Chum::Lef("core".to_string());
    let tyre = vec![("one".to_string(), axis(2)), ("two".to_string(), axis(3))];
    let lowered = Ut::lower_sigcen(&chum, &axis(7), &tyre, &axis(1));
    let Hoon::SigGal(TermOrPair::Pair(name, clls), body) = lowered else {
        panic!("sigcen lowers to a %fast sggl");
    };
    assert_eq!(name, "fast");
    assert_eq!(*body, axis(1));
    let Hoon::ColLus(_, parent, hooks) = *clls else {
        panic!("sigcen hint is a :+ triple");
    };
    assert_eq!(*parent, Hoon::ZapTis(bx(axis(7))));
    let Hoon::ColSig(items) = *hooks else {
        panic!("hooks are a :~ list");
    };
    assert_eq!(items.len(), 2);
    assert_eq!(
        items[1],
        Hoon::Pair(
            bx(Hoon::Rock(
                "$".to_string(),
                NounExpr::ParsedAtom(term_atom("two"))
            )),
            bx(Hoon::ZapTis(bx(axis(3)))),
        )
    );
}

#[test]
fn lower_micsig_rejects_empty_and_chains_each_rule() {
    assert_eq!(
        Ut::lower_micsig(&axis(1), &[]),
        Hoon::Eror("open-mcsg".to_string())
    );
    let single = Ut::lower_micsig(&axis(1), &[axis(2)]);
    assert_eq!(
        single,
        Hoon::TisGar(
            bx(Hoon::KetTis(Skin::Term("v".to_string()), bx(axis(1)))),
            bx(Hoon::TisGar(bx(Hoon::Limb("v".to_string())), bx(axis(2)))),
        )
    );
    let chained = Ut::lower_micsig(&axis(1), &[axis(2), axis(6)]);
    let Hoon::TisGar(_, body) = chained else {
        panic!("micsig binds v=. first");
    };
    // =+  a=$(rest)  =+  b==>(v i)  =+  c=,.+6.b  |.  %+  ...
    let Hoon::TisLus(a_bind, rest) = *body else {
        panic!("a two-rule chain binds a/b/c");
    };
    assert!(matches!(*a_bind, Hoon::KetTis(Skin::Term(ref a), _) if a == "a"));
    let Hoon::TisLus(b_bind, rest) = *rest else {
        panic!("b binding");
    };
    assert!(matches!(*b_bind, Hoon::KetTis(Skin::Term(ref b), _) if b == "b"));
    let Hoon::TisLus(c_bind, trap) = *rest else {
        panic!("c binding");
    };
    assert!(matches!(*c_bind, Hoon::KetTis(Skin::Term(ref c), _) if c == "c"));
    assert!(matches!(*trap, Hoon::BarDot(ref inner) if matches!(**inner, Hoon::CenLus(..))));
}

#[test]
fn prefix_signature_distinguishes_named_prefixes() {
    let none = Ut::prefix_signature(None);
    let foo = Ut::prefix_signature(Some("foo"));
    let bar = Ut::prefix_signature(Some("bar"));
    assert_ne!(none, foo);
    assert_ne!(foo, bar);
    assert_eq!(foo, Ut::prefix_signature(Some("foo")));
}

// ---------------------------------------------------------------------------
// mint_inner / play_inner dispatch through source snippets
// ---------------------------------------------------------------------------

#[test]
fn mint_dispatches_rare_runes_like_their_open_lowerings() {
    // =,  (TisCom)
    assert_mint_same("=/  a  [b=1 c=2]  =,  a  b", "=/  a  [b=1 c=2]  b.a");
    // ?|  (WutBar)
    assert_mint_same("?|(& |)", "?:(& %.y ?:(| %.y %.n))");
    // ?<  (WutGal)
    assert_mint_same("?<(| 5)", "?:(| !! 5)");
    // =;  (TisMic)
    assert_mint_same("=;  a=@  a  5", "=/  a=@  5  a");
    // =.  (TisDot)
    assert_mint_same("=/  a  1  =.  a  2  a", "=/  a  1  =>  %_(. a 2)  a");
    // %-  (CenHep)
    assert_mint_same("%-  |=(a=@ a)  5", "(|=(a=@ a) 5)");
    // ;~ with one rule (MicSig)
    assert_mint_same(";~(5 6)", "=>  v=.  =>  v  6");
    // ;: over a two-argument gate (MicCol)
    assert_mint_same(
        "=/  f  |=([a=@ b=@] a)  ;:(f 1 2 3)",
        "=/  f  |=([a=@ b=@] a)  =+  f  (+2 =>(+3 1) (+2 =>(+3 2) =>(+3 3)))",
    );
}

#[test]
fn mint_dottar_is_nock_two_with_noun_product() {
    let mut slab = NounSlab::new();
    let (ty, formula) = {
        let mut ut = Ut::new(&mut slab);
        let sut = ty_noun(&mut *ut.slab);
        let gol = ty_noun(&mut *ut.slab);
        ut.mint_noun(sut, gol, &parse(".*(1 [1 2])"))
            .expect("mint .*")
    };
    let space = slab.noun_space();
    assert_eq!(type_tag(ty, &space).expect("tag"), "noun");
    let expected = {
        let subject = T(&mut slab, &[D(1), D(1)]);
        let inner = T(&mut slab, &[D(1), D(2)]);
        let quoted = T(&mut slab, &[D(1), inner]);
        T(&mut slab, &[D(2), subject, quoted])
    };
    assert!(noun_eq(formula, expected, &slab.noun_space()).expect("noun_eq"));
}

#[test]
fn mint_sigbar_with_tas_sand_is_a_mean_hint() {
    let tas = Hoon::Sand("tas".to_string(), NounExpr::ParsedAtom(term_atom("foo")));
    let sigbar = Hoon::SigBar(bx(tas), bx(rock("ud", 5)));
    let hinted = Hoon::SigGar(
        TermOrPair::Pair(
            "mean".to_string(),
            bx(Hoon::Rock(
                "tas".to_string(),
                NounExpr::ParsedAtom(term_atom("foo")),
            )),
        ),
        bx(rock("ud", 5)),
    );
    assert_eq!(mint_gen_jam(&sigbar), mint_gen_jam(&hinted));
    assert_eq!(play_gen_jam(&sigbar), play_gen_jam(&rock("ud", 5)));
}

#[test]
fn mint_tissig_handles_empty_single_and_chains() {
    // An empty `=~` cannot come from the parser; natively it is void/[0 0].
    let mut slab = NounSlab::new();
    let (ty, formula) = {
        let mut ut = Ut::new(&mut slab);
        let sut = ty_noun(&mut *ut.slab);
        let gol = ty_noun(&mut *ut.slab);
        ut.mint_noun(sut, gol, &Hoon::TisSig(vec![]))
            .expect("mint empty =~")
    };
    let space = slab.noun_space();
    assert_eq!(type_tag(ty, &space).expect("tag"), "void");
    let expected = T(&mut slab, &[D(0), D(0)]);
    assert!(noun_eq(formula, expected, &slab.noun_space()).expect("noun_eq"));

    let chain = Hoon::TisSig(vec![
        Hoon::Pair(bx(rock("ud", 1)), bx(rock("ud", 2))),
        axis(3),
    ]);
    let tsgr = Hoon::TisGar(
        bx(Hoon::Pair(bx(rock("ud", 1)), bx(rock("ud", 2)))),
        bx(axis(3)),
    );
    assert_eq!(mint_gen_jam(&chain), mint_gen_jam(&tsgr));
    assert_eq!(
        mint_gen_jam(&Hoon::TisSig(vec![rock("ud", 4)])),
        mint_gen_jam(&rock("ud", 4))
    );
    assert_eq!(play_gen_jam(&chain), play_gen_jam(&tsgr));
    assert_eq!(play_gen_jam(&Hoon::TisSig(vec![])), play_jam("!!"));
    assert_eq!(
        play_gen_jam(&Hoon::TisSig(vec![rock("ud", 4)])),
        play_gen_jam(&rock("ud", 4))
    );
}

#[test]
fn mint_and_play_hand_use_the_carried_type() {
    let hand = Hoon::Hand(
        Box::new(Type::ParsedAtom("ud".to_string(), Some(7))),
        Nock::Const(NounExpr::ParsedAtom(ParsedAtom::Small(7))),
    );
    assert_eq!(
        play_gen_jam(&hand),
        play_gen_jam(&rock("ud", 7)),
        "play %hand returns p.gen"
    );
    let jam = mint_gen_jam(&hand).expect("mint %hand");
    assert_eq!(jam, mint_gen_jam(&rock("ud", 7)).expect("mint rock"));
}

#[test]
fn mint_barcab_with_aliases_mints_a_door() {
    let src = "|_  a=@\n+*  b  a\n++  foo  b\n--";
    let gen = parse(src);
    assert!(
        matches!(&gen, Hoon::BarCab(_, alas, _) if !alas.is_empty()),
        "the door carries its +* aliases: {gen:?}"
    );
    let with_alias = mint_jam(src).expect("mint door with +*");
    let without_alias = mint_jam("|_  a=@\n++  foo  a\n--").expect("mint door");
    assert_ne!(with_alias, without_alias, "aliases change the door battery");
}

#[test]
fn play_dispatches_rare_runes_like_their_open_lowerings() {
    let pairs: &[(&str, &str)] = &[
        ("?.(& 1 2)", "?:(& 2 1)"),
        ("=/  a  [b=1 c=2]  =,  a  b", "=/  a  [b=1 c=2]  b.a"),
        ("=/  a=*  1  ?#(%foo a)", ".=(1 1)"),
        ("?&(& |)", "?:(& ?:(| %.y %.n) %.n)"),
        ("?|(& |)", "?:(& %.y ?:(| %.y %.n))"),
        ("=/  a=*  1  ?@(a 1 2)", "=/  a=*  1  ?:(?=(@ a) 1 2)"),
        ("=/  a=*  1  ?^(a 1 2)", "=/  a=*  1  ?:(?=(@ a) 2 1)"),
        ("?!(&)", ".=(1 1)"),
        ("=/  a=*  1  ?=(@ a)", ".=(1 1)"),
        ("?<(| 1)", "?:(| !! 1)"),
        ("?>(& 1)", "?:(& 1 !!)"),
        (
            "=/  a=?(%x %y)  %x  ?-(a %x 1, %y 2)",
            "=/  a=?(%x %y)  %x  ?:(?=(%x a) 1 ?:(?=(%y a) 2 !!))",
        ),
        (
            "=/  a=?(%x %y)  %x  ?+(a 3 %x 1)", "=/  a=?(%x %y)  %x  ?:(?=(%x a) 1 3)",
        ),
        ("^.(|=(a=@ a) 5)", "(|=(a=@ a) 5)"),
        (".?(1)", ".=(1 1)"),
        (".*(1 [1 2])", "!=(1)"),
        ("=/  a  1  a", "=+  a=1  a"),
        ("=;  a  a  1", "=/  a  1  a"),
        ("=/  a  1  =.  a  2  a", "=/  a  1  =>  .(a 2)  a"),
        ("%-(|=(a=@ a) 5)", "(|=(a=@ a) 5)"),
        ("%+(|=([a=@ b=@] a) 5 6)", "(|=([a=@ b=@] a) 5 6)"),
        ("~|(5 6)", "6"),
        (";~(5 6)", "6"),
        (
            "=/  f  |=([a=@ b=@] a)  ;:(f 1 2 3)", "=/  f  |=([a=@ b=@] a)  (f 1 (f 2 3))",
        ),
        (":-(1 2)", "[1 2]"),
        (":+(1 2 3)", "[1 2 3]"),
        (":~(1 2)", "[1 2 ~]"),
        ("~+(5)", "5"),
        ("~!  1  2", "2"),
        ("!;(1 2)", "[1 2]"),
        (".^(@ 1)", "*@"),
        ("!,(5 6)", "5"),
        ("!<(@ 5)", "*@"),
        ("!=(5)", "!=(1)"),
        ("!@(a 1 2)", "2"),
        ("=/  a  1  !@(a 1 2)", "=/  a  1  1"),
    ];
    let failures: Vec<String> = pairs
        .iter()
        .filter_map(|(src, equivalent)| play_same(src, equivalent).err())
        .collect();
    assert!(
        failures.is_empty(),
        "play mismatches:\n{}",
        failures.join("\n")
    );
    // `;~` with several rules plays to the |. core it builds.
    let chained = parse("=/  g  |=(a=@ a)  ;~(g g g)");
    assert_eq!(play_tag(&chained), "core");
    // ^& and ^? wrap the core's variance.
    let zinc = play_jam("^&(|.(1))").expect("play ^&");
    let lead = play_jam("^?(|.(1))").expect("play ^?");
    let gold = play_jam("|.(1)").expect("play |.");
    assert_ne!(zinc, gold);
    assert_ne!(lead, gold);
    assert_ne!(zinc, lead);
}

#[test]
fn wtzp_opens_to_wtcl_and_drops_the_dead_branch() {
    // hoon-138 `++open`: `?!(p)` => `?:(p %.n %.y)`, so gain/lose prune a void branch.
    assert_mint_same(
        "=/  a=@  1  !=(?!(?=(^ a)))", "=/  a=@  1  !=(?:(?=(^ a) %.n %.y))",
    );
    play_same("=/  a=@  1  ?!(?=(^ a))", "=/  a=@  1  %.y").unwrap();
    play_same("=/  a=@  1  ?!(?=(@ a))", "=/  a=@  1  %.n").unwrap();
    // Under vet, the dead `%.n` branch is minted against a void subject.
    let err = mint_jam("=/  a=@  1  ?!(?=(^ a))").expect_err("dead ?! branch is mint-vain");
    assert!(err.contains("mint-vain"), "{err}");
}

#[test]
fn bare_axis_hoons_are_found_with_read_permission() {
    // hoon-138 `++open`: `[%$ p]` => `[%cnts [[%& p] ~] ~]`, so a bare axis (from `^=`
    // cell skins, for one) resolves like the wing `+p` and `++peel` hides blocked payloads.
    for core in ["|=(x=@ x)", "^|(|=(x=@ x))", "^?(|=(x=@ x))", "^&(|=(x=@ x))"] {
        for ax in [2u64, 3, 6, 7] {
            let bare = Hoon::TisGar(bx(parse(core)), bx(axis(ax)));
            let wing = Hoon::TisGar(bx(parse(core)), bx(Hoon::Wing(vec![Limb::Axis(ax.into())])));
            assert_eq!(
                play_gen_jam(&bare),
                play_gen_jam(&wing),
                "play {core} +{ax}"
            );
            assert_eq!(
                mint_gen_jam(&bare),
                mint_gen_jam(&wing),
                "mint {core} +{ax}"
            );
        }
    }
    let iron_sample = Hoon::TisGar(bx(parse("^|(|=(x=@ x))")), bx(axis(6)));
    assert_eq!(play_tag(&iron_sample), "noun");
    let gold_sample = Hoon::TisGar(bx(parse("|=(x=@ x)")), bx(axis(6)));
    assert_eq!(play_tag(&gold_sample), "face");
}

#[test]
fn play_constructed_only_nodes_follow_hoon_138() {
    // %lost plays to void; %fits plays to a flag.
    assert_eq!(play_gen_jam(&Hoon::Lost(bx(axis(1)))), play_jam("!!"));
    assert_eq!(
        play_gen_jam(&Hoon::Fits(
            bx(rock("ud", 1)),
            vec![Limb::Axis(1u64.into())]
        )),
        play_jam(".=(1 1)")
    );
    // :~ over no items is the null atom.
    assert_eq!(play_gen_jam(&Hoon::ColSig(vec![])), play_jam("~"));
}

// ---------------------------------------------------------------------------
// Build-memo reset, context keys, and small pure helpers
// ---------------------------------------------------------------------------

/// A structurally fresh `[%atom %$ `n]` type at a new slab address.
fn atom_ty(slab: &mut NounSlab, n: u64) -> Noun {
    ty_atom(slab, "@", Some(D(n)))
}

#[test]
fn clear_build_memos_resets_every_build_scoped_cache() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let noun_sut = ty_noun(&mut *ut.slab);
    let noun_gol = ty_noun(&mut *ut.slab);
    ut.mint_noun(noun_sut, noun_gol, &parse("=/  a  1  |=(b=@ [a b])"))
        .expect("mint populates caches");
    let sut = atom_ty(&mut *ut.slab, 1);
    let ref_ = atom_ty(&mut *ut.slab, 2);
    ut.redo_boundary_store(sut, ref_, ref_).expect("redo store");
    ut.arm_epoch = ArmEpoch(9);
    ut.spec_example_cache
        .insert(SpecSignature(1), VecDeque::new());
    ut.spec_example_cache_order.push_back(SpecSignature(1));
    ut.spec_factory_open_cache
        .insert(SpecSignature(2), VecDeque::new());
    ut.spec_factory_open_cache_order.push_back(SpecSignature(2));
    ut.burp_type_cache.insert(NounIdentity(3), D(0));
    ut.open_cache
        .insert(HoonIdentity(4), (HoonSignature(4), None));
    ut.open_cache_order.push_back(HoonIdentity(4));
    ut.arm_key_term_cache
        .insert(NounIdentity(5), Arc::from("k"));
    ut.arm_key_term_cache_order.push_back(NounIdentity(5));
    ut.hold_repo_fan_activate_leg_id(FanLegId(6));
    assert_ne!(ut.hold_repo_fan_context_key(), FanContextId(0));

    ut.clear_build_memos();

    assert_eq!(ut.arm_epoch, ArmEpoch(0));
    assert!(ut
        .boundary_memo
        .redo
        .get(&TypeBinaryKey {
            subject: ut.noun_mug_cached(sut),
            reference: ut.noun_mug_cached(ref_),
            vet: VetMode(ut.vet),
            fan: FanContextId(0),
        })
        .is_none());
    assert!(ut
        .redo_boundary_lookup(sut, ref_)
        .expect("redo lookup")
        .is_none());
    assert!(ut.spec_example_cache.is_empty() && ut.spec_example_cache_order.is_empty());
    assert!(ut.spec_factory_open_cache.is_empty() && ut.spec_factory_open_cache_order.is_empty());
    assert!(ut.burp_type_cache.is_empty());
    assert!(ut.open_cache.is_empty() && ut.open_cache_order.is_empty());
    assert!(ut.arm_key_term_cache.is_empty() && ut.arm_key_term_cache_order.is_empty());
    assert!(ut.hold_repo_fan_active_leg_ids.is_empty());
    assert_eq!(ut.hold_repo_fan_context_key(), FanContextId(0));
}

#[test]
fn memo_context_keys_follow_placeholder_and_goal_recursion_state() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    ut.arm_epoch = ArmEpoch(41);
    assert_eq!(
        ut.arm_placeholder_context_signature(),
        PlaceholderSignature(0)
    );
    assert_eq!(
        ut.arm_cache_epoch_key(),
        ArmEpoch(0),
        "steady state collapses"
    );

    // A live placeholder play keeps the epoch and signs the placeholder set.
    ut.arm_placeholder_play_in_progress.insert(NounIdentity(11));
    ut.arm_placeholder_play_in_progress.insert(NounIdentity(22));
    let signature = ut.arm_placeholder_context_signature();
    assert_ne!(signature, PlaceholderSignature(0));
    assert_eq!(ut.arm_cache_epoch_key(), ArmEpoch(41));
    assert_eq!(ut.memo_context_key().placeholder_context_key, signature);

    // The signature is a function of the set, not of insertion order.
    let mut other_slab = NounSlab::new();
    let mut other = Ut::new(&mut other_slab);
    other
        .arm_placeholder_play_in_progress
        .insert(NounIdentity(22));
    other
        .arm_placeholder_play_in_progress
        .insert(NounIdentity(11));
    assert_eq!(other.arm_placeholder_context_signature(), signature);
    other
        .arm_placeholder_play_in_progress
        .insert(NounIdentity(33));
    assert_ne!(other.arm_placeholder_context_signature(), signature);

    // An in-progress goal alone also pins the epoch.
    ut.arm_placeholder_play_in_progress.clear();
    let core = cons_noun(&mut ut.cx);
    ut.arm_goal_in_progress.push(ArmInProgressEntry {
        key: Arc::from("arm"),
        core: core.clone(),
        hoon: D(0),
        goal: core,
        vet: true,
    });
    assert!(ut.arm_in_progress.is_empty());
    assert_eq!(ut.arm_cache_epoch_key(), ArmEpoch(41));
}

#[test]
fn dbug_path_strips_the_cwd_prefix_only_for_paths_below_it() {
    assert_eq!(Ut::dbug_path(&[]), "?");
    let cwd: Vec<String> = std::env::current_dir()
        .expect("cwd")
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(segment) => Some(segment.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let mut below = cwd.clone();
    below.push("probe.hoon".to_string());
    assert_eq!(Ut::dbug_path(&below), "probe.hoon");
    // Exactly the cwd (not longer) is not shortened.
    assert_eq!(Ut::dbug_path(&cwd), cwd.join("/"));
    // Longer but elsewhere is printed whole.
    let mut elsewhere = vec!["not-the-cwd".to_string()];
    elsewhere.extend(cwd.iter().cloned());
    elsewhere.push("x.hoon".to_string());
    assert_eq!(Ut::dbug_path(&elsewhere), elsewhere.join("/"));
}

#[test]
fn slot_axis_rejects_zero_and_walks_cells() {
    let mut slab: NounSlab = NounSlab::new();
    let noun = T(&mut slab, &[D(4), D(5), D(6)]);
    let space = slab.noun_space();
    assert!(Ut::slot_axis(noun, 0u64, &space).is_none());
    let head = Ut::slot_axis(noun, 2u64, &space).expect("+2");
    assert!(unsafe { head.raw_equals(&D(4)) });
    let last = Ut::slot_axis(noun, 7u64, &space).expect("+7");
    assert!(unsafe { last.raw_equals(&D(6)) });
    assert!(Ut::slot_axis(noun, 4u64, &space).is_none(), "+4 of an atom");
}

#[test]
fn nonsemantic_hoon_noun_canonicalization_strips_dbug_and_gist_wrappers() {
    let mut slab = NounSlab::new();
    let dbug = term_to_noun(&mut slab, "dbug");
    let gist = term_to_noun(&mut slab, "gist");
    let ktcl = term_to_noun(&mut slab, "ktcl");
    let base = term_to_noun(&mut slab, "base");
    let spot_noun = T(&mut slab, &[D(0), D(0)]);
    let inner = T(&mut slab, &[base, D(0)]);
    let wrapped = T(&mut slab, &[dbug, spot_noun, inner]);
    let twice = T(&mut slab, &[dbug, spot_noun, wrapped]);
    let spec = T(&mut slab, &[base, D(0)]);
    let gisted_spec = T(&mut slab, &[gist, D(9), spec]);
    let ktcl_gisted = T(&mut slab, &[ktcl, gisted_spec]);
    let ktcl_plain = T(&mut slab, &[ktcl, spec]);
    let dbug_ktcl = T(&mut slab, &[dbug, spot_noun, ktcl_gisted]);
    // Malformed wrappers stop the strip loops.
    let bad_dbug_tail = T(&mut slab, &[dbug, D(1)]);
    let cell_head = T(&mut slab, &[inner, D(1)]);
    let non_utf8_head = T(&mut slab, &[D(0xff), D(1)]);
    let bad_gist_tail = T(&mut slab, &[gist, D(1)]);
    let ktcl_bad_gist = T(&mut slab, &[ktcl, bad_gist_tail]);
    let ktcl_cell_head_spec = T(&mut slab, &[ktcl, cell_head]);
    let ktcl_non_utf8_spec = T(&mut slab, &[ktcl, non_utf8_head]);

    let space = slab.noun_space();
    let strip = |noun: Noun| Ut::strip_dbug_wrapper_noun(noun, &space);
    assert!(unsafe { strip(twice).raw_equals(&inner) });
    for noun in [D(3), cell_head, non_utf8_head, bad_dbug_tail, inner] {
        assert!(unsafe { strip(noun).raw_equals(&noun) }, "unchanged");
    }
    let strip_gist = |noun: Noun| Ut::strip_spec_gist_wrapper_noun(noun, &space);
    assert!(unsafe { strip_gist(gisted_spec).raw_equals(&spec) });
    for noun in [D(3), cell_head, non_utf8_head, bad_gist_tail, spec] {
        assert!(unsafe { strip_gist(noun).raw_equals(&noun) }, "unchanged");
    }

    let mut ut = Ut::new(&mut slab);
    // `^:` with a gist-wrapped spec is rebuilt without the gist.
    let canonical = ut.canonicalize_nonsemantic_hoon_noun(dbug_ktcl);
    assert!(noun_eq(canonical, ktcl_plain, &ut.slab.noun_space()).expect("noun_eq"));
    assert!(!unsafe { canonical.raw_equals(&ktcl_plain) });
    // A plain `^:` is returned as is.
    let plain = ut.canonicalize_nonsemantic_hoon_noun(ktcl_plain);
    assert!(unsafe { plain.raw_equals(&ktcl_plain) });
    // Non-`^:` tags, non-string tags, cell heads, and atoms are returned stripped.
    for noun in [
        wrapped,
        non_utf8_head,
        cell_head,
        D(7),
        ktcl_bad_gist,
        ktcl_cell_head_spec,
        ktcl_non_utf8_spec,
    ] {
        let space = ut.slab.noun_space();
        let expected = Ut::strip_dbug_wrapper_noun(noun, &space);
        let got = ut.canonicalize_nonsemantic_hoon_noun(noun);
        assert!(unsafe { got.raw_equals(&expected) });
    }

    let space = ut.slab.noun_space();
    assert_eq!(
        Ut::hoon_noun_tag(ktcl_plain, &space).as_deref(),
        Some("ktcl")
    );
    assert_eq!(Ut::hoon_noun_tag(D(1), &space), None);
    assert_eq!(Ut::hoon_noun_tag(cell_head, &space), None);
}

// ---------------------------------------------------------------------------
// `%hold` fan-leg interning and fan context keys
// ---------------------------------------------------------------------------

#[test]
fn fan_leg_lookup_matches_structurally_and_skips_colliding_entries() {
    let mut slab = NounSlab::new();
    let inner = atom_ty(&mut slab, 7);
    let hoon = hoon_to_noun(&mut slab, &axis(1));
    let hoon_copy = hoon_to_noun(&mut slab, &axis(1));
    let hoon_copy2 = hoon_to_noun(&mut slab, &axis(1));
    let inner_other = atom_ty(&mut slab, 8);
    let hoon_other = hoon_to_noun(&mut slab, &axis(2));
    let lone_inner = atom_ty(&mut slab, 9);
    let lone_hoon = hoon_to_noun(&mut slab, &axis(3));
    let mut ut = Ut::new(&mut slab);

    let id = ut.hold_repo_fan_leg_intern_id(inner, hoon).expect("intern");
    assert_eq!(
        ut.hold_repo_fan_leg_intern_id(inner, hoon).expect("raw"),
        id
    );
    // Same inner address, structurally equal gene at a new address.
    assert_eq!(
        ut.hold_repo_fan_leg_lookup_id(inner, hoon_copy)
            .expect("mug"),
        Some(id)
    );

    // Entries that share the mug key but differ in inner or gene are skipped.
    let key = HoldKey {
        subject: ut.noun_mug_cached(inner),
        gene: ut.noun_mug_cached(hoon),
    };
    let bucket = ut.hold_repo_fan_leg_ids.get_mut(&key).expect("bucket");
    bucket.push(HoldRepoFanLegIdEntry {
        id: FanLegId(900),
        inner: inner_other,
        hoon,
    });
    bucket.push(HoldRepoFanLegIdEntry {
        id: FanLegId(901),
        inner,
        hoon: hoon_other,
    });
    assert_eq!(
        ut.hold_repo_fan_leg_lookup_id(inner, hoon_copy2)
            .expect("skip"),
        Some(id)
    );

    // A bucket whose entries all mismatch yields no leg.
    let lone_key = HoldKey {
        subject: ut.noun_mug_cached(lone_inner),
        gene: ut.noun_mug_cached(lone_hoon),
    };
    ut.hold_repo_fan_leg_ids.insert(
        lone_key,
        vec![HoldRepoFanLegIdEntry {
            id: FanLegId(902),
            inner: inner_other,
            hoon: hoon_other,
        }],
    );
    assert_eq!(
        ut.hold_repo_fan_leg_lookup_id(lone_inner, lone_hoon)
            .expect("miss"),
        None
    );
}

#[test]
fn fan_leg_and_context_ids_wrap_past_zero() {
    let mut slab = NounSlab::new();
    let inner = atom_ty(&mut slab, 70);
    let hoon = hoon_to_noun(&mut slab, &axis(1));
    let mut ut = Ut::new(&mut slab);

    ut.hold_repo_fan_leg_next_id = FanLegId(u64::MAX);
    let leg = ut.hold_repo_fan_leg_intern_id(inner, hoon).expect("intern");
    assert_eq!(leg, FanLegId(u64::MAX));
    assert_eq!(ut.hold_repo_fan_leg_next_id, FanLegId(1));

    ut.hold_repo_fan_context_next_id = FanContextId(u64::MAX);
    let subset = ut.intern_fan_subset_id(&[FanLegId(3), FanLegId(4)]);
    assert_eq!(subset, FanContextId(u64::MAX));
    assert_eq!(ut.hold_repo_fan_context_next_id, FanContextId(1));

    ut.hold_repo_fan_context_next_id = FanContextId(u64::MAX);
    assert!(ut.hold_repo_fan_activate_leg_id(FanLegId(5)));
    assert_eq!(ut.hold_repo_fan_context_id, FanContextId(u64::MAX));
    assert_eq!(ut.hold_repo_fan_context_next_id, FanContextId(1));
}

#[test]
fn fan_subset_and_context_ids_ignore_signature_collisions() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let subset = [FanLegId(1), FanLegId(2)];
    let signature = |legs: &[FanLegId]| {
        let mut sum = 0u64;
        let mut xor = 0u64;
        for leg in legs {
            let component = Ut::hold_repo_fan_leg_signature_component(*leg);
            sum = sum.wrapping_add(component);
            xor ^= component;
        }
        SetSignature {
            sum,
            xor,
            len: legs.len(),
        }
    };
    // A foreign subset filed under the same signature is not a hit.
    ut.hold_repo_fan_subset_by_signature.insert(
        signature(&subset),
        vec![(vec![FanLegId(77), FanLegId(78)], FanContextId(500))],
    );
    let id = ut.intern_fan_subset_id(&subset);
    assert_ne!(id, FanContextId(500));
    assert_eq!(ut.intern_fan_subset_id(&subset), id);
    assert_eq!(ut.intern_fan_subset_id(&[]), FanContextId(0));

    // Same for whole-fan context ids.
    ut.hold_repo_fan_context_by_signature.insert(
        signature(&[FanLegId(5)]),
        vec![(vec![FanLegId(99)], FanContextId(600))],
    );
    assert!(ut.hold_repo_fan_activate_leg_id(FanLegId(5)));
    assert_ne!(ut.hold_repo_fan_context_id, FanContextId(600));
    // Activating an already-active leg is a no-op.
    assert!(!ut.hold_repo_fan_activate_leg_id(FanLegId(5)));
    assert_eq!(ut.hold_repo_fan_active_leg_ids, vec![FanLegId(5)]);

    assert!(Ut::intersect_sorted_legs(&[], &subset).is_empty());
    assert!(Ut::intersect_sorted_legs(&subset, &[]).is_empty());
    assert_eq!(
        Ut::intersect_sorted_legs(&[FanLegId(1), FanLegId(3)], &[FanLegId(2), FanLegId(3)]),
        vec![FanLegId(3)]
    );
}

#[test]
fn deactivating_an_inactive_fan_leg_is_a_debug_assertion() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    assert!(ut.hold_repo_fan_activate_leg_id(FanLegId(8)));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ut.hold_repo_fan_deactivate_leg_id(FanLegId(9));
    }));
    assert_eq!(outcome.is_err(), cfg!(debug_assertions));
    if outcome.is_ok() {
        assert_eq!(ut.hold_repo_fan_active_leg_ids, vec![FanLegId(8)]);
    }
}

#[test]
fn fan_leg_hold_raw_store_dedupes_and_evicts_oldest() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let limit = Ut::HOLD_REPO_FAN_LEG_HOLD_RAW_KEY_LIMIT as u64;
    ut.hold_repo_fan_leg_id_by_hold_raw_store(NounIdentity(0), FanLegId(1));
    ut.hold_repo_fan_leg_id_by_hold_raw_store(NounIdentity(0), FanLegId(2));
    assert_eq!(ut.hold_repo_fan_leg_id_by_hold_raw_order.len(), 1);
    assert_eq!(
        ut.hold_repo_fan_leg_id_by_hold_raw.get(&NounIdentity(0)),
        Some(&FanLegId(2))
    );
    for raw in 1..=limit {
        ut.hold_repo_fan_leg_id_by_hold_raw_store(NounIdentity(raw), FanLegId(3));
    }
    assert_eq!(
        ut.hold_repo_fan_leg_id_by_hold_raw_order.len(),
        Ut::HOLD_REPO_FAN_LEG_HOLD_RAW_KEY_LIMIT
    );
    assert!(!ut
        .hold_repo_fan_leg_id_by_hold_raw
        .contains_key(&NounIdentity(0)));
    assert!(ut
        .hold_repo_fan_leg_id_by_hold_raw
        .contains_key(&NounIdentity(limit)));
}

#[test]
fn fan_leg_hold_mug_store_and_lookup_compare_structurally() {
    let mut slab = NounSlab::new();
    let hold = {
        let inner = atom_ty(&mut slab, 11);
        let hoon = hoon_to_noun(&mut slab, &axis(1));
        ty_hold(&mut slab, inner, hoon)
    };
    let hold_copy = {
        let inner = atom_ty(&mut slab, 11);
        let hoon = hoon_to_noun(&mut slab, &axis(1));
        ty_hold(&mut slab, inner, hoon)
    };
    let others: Vec<Noun> = (0..9u64)
        .map(|n| {
            let inner = atom_ty(&mut slab, 100 + n);
            let hoon = hoon_to_noun(&mut slab, &axis(1));
            ty_hold(&mut slab, inner, hoon)
        })
        .collect();
    let lone = {
        let inner = atom_ty(&mut slab, 12);
        let hoon = hoon_to_noun(&mut slab, &axis(1));
        ty_hold(&mut slab, inner, hoon)
    };
    let mut ut = Ut::new(&mut slab);

    ut.hold_repo_fan_leg_id_by_hold_mug_store(hold, FanLegId(1))
        .expect("store");
    // Re-storing the same hold, or a structural copy, is a no-op.
    ut.hold_repo_fan_leg_id_by_hold_mug_store(hold, FanLegId(2))
        .expect("store same");
    ut.hold_repo_fan_leg_id_by_hold_mug_store(hold_copy, FanLegId(3))
        .expect("store copy");
    let mug = ut.noun_mug_cached(hold);
    assert_eq!(ut.hold_repo_fan_leg_id_by_hold_mug[&mug].len(), 1);

    // Lookups by the same address and by a structural copy both hit.
    assert_eq!(
        ut.hold_repo_fan_leg_id_by_hold_mug_lookup(hold)
            .expect("lookup"),
        Some(FanLegId(1))
    );
    // A colliding entry ahead of the match (lookups scan newest first) is skipped.
    ut.hold_repo_fan_leg_id_by_hold_mug
        .get_mut(&mug)
        .expect("bucket")
        .push_back(HoldRepoFanHoldIdEntry {
            hold: others[0],
            id: FanLegId(50),
        });
    assert_eq!(
        ut.hold_repo_fan_leg_id_by_hold_mug_lookup(hold_copy)
            .expect("lookup copy"),
        Some(FanLegId(1))
    );
    assert_eq!(
        ut.hold_repo_fan_leg_id_by_hold_raw
            .get(&NounIdentity::of(hold_copy)),
        Some(&FanLegId(1)),
        "a structural hit is remembered by raw identity"
    );
    // Stores scan oldest first; a colliding entry ahead of the match is skipped.
    ut.hold_repo_fan_leg_id_by_hold_mug
        .get_mut(&mug)
        .expect("bucket")
        .push_front(HoldRepoFanHoldIdEntry {
            hold: others[1],
            id: FanLegId(51),
        });
    ut.hold_repo_fan_leg_id_by_hold_mug_store(hold_copy, FanLegId(4))
        .expect("store past collision");

    // A bucket of only colliding entries misses, and a full bucket evicts.
    let lone_mug = ut.noun_mug_cached(lone);
    let colliding: VecDeque<_> = others[1..]
        .iter()
        .map(|&hold| HoldRepoFanHoldIdEntry {
            hold,
            id: FanLegId(60),
        })
        .collect();
    assert_eq!(colliding.len(), Ut::HOLD_REPO_FAN_LEG_HOLD_MUG_BUCKET_LIMIT);
    ut.hold_repo_fan_leg_id_by_hold_mug
        .insert(lone_mug, colliding);
    assert_eq!(
        ut.hold_repo_fan_leg_id_by_hold_mug_lookup(lone)
            .expect("miss"),
        None
    );
    ut.hold_repo_fan_leg_id_by_hold_mug_store(lone, FanLegId(7))
        .expect("store into full bucket");
    let bucket = &ut.hold_repo_fan_leg_id_by_hold_mug[&lone_mug];
    assert_eq!(bucket.len(), Ut::HOLD_REPO_FAN_LEG_HOLD_MUG_BUCKET_LIMIT);
    assert!(unsafe { bucket.back().expect("newest").hold.raw_equals(&lone) });
    assert!(!bucket
        .iter()
        .any(|entry| unsafe { entry.hold.raw_equals(&others[1]) }));
}

#[test]
fn fan_leg_hold_mug_store_evicts_the_oldest_key() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let limit = Ut::HOLD_REPO_FAN_LEG_HOLD_MUG_KEY_LIMIT;
    let mut value = 0u64;
    while ut.hold_repo_fan_leg_id_by_hold_mug_order.len() < limit {
        ut.hold_repo_fan_leg_id_by_hold_mug_store(D(value), FanLegId(1))
            .expect("store");
        value += 1;
    }
    let oldest = *ut
        .hold_repo_fan_leg_id_by_hold_mug_order
        .front()
        .expect("oldest");
    // Keep storing until a fresh mug key arrives and forces an eviction.
    while ut.hold_repo_fan_leg_id_by_hold_mug.contains_key(&oldest) {
        ut.hold_repo_fan_leg_id_by_hold_mug_store(D(value), FanLegId(1))
            .expect("store");
        value += 1;
    }
    assert_eq!(ut.hold_repo_fan_leg_id_by_hold_mug_order.len(), limit);
}

#[test]
fn fan_leg_id_for_native_hold_rejects_non_holds() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let noun = cons_noun(&mut ut.cx);
    let err = ut
        .hold_repo_fan_leg_id_for_hold_native(&noun)
        .expect_err("a %noun type has no fan leg");
    assert!(format!("{err:?}").contains("leg-id of non-hold"));
}

// ---------------------------------------------------------------------------
// Musk eval-stack copy cache
// ---------------------------------------------------------------------------

#[test]
fn musk_core_cache_is_cleared_past_its_cap_in_the_same_context() {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let context = create_musk_eval_context();
    ut.ensure_musk_mack_core_cache_context(&context);
    let cap = Ut::MUSK_CORE_CACHE_CAP;
    ut.musk.mack_core_cache_raw.insert(NounIdentity(1), D(1));
    ut.ensure_musk_mack_core_cache_context(&context);
    assert_eq!(
        ut.musk.mack_core_cache_raw.len(),
        1,
        "under the cap is kept"
    );
    ut.musk.mack_core_cache_raw.reserve(cap + 1);
    for raw in 0..=(cap as u64) {
        ut.musk.mack_core_cache_raw.insert(NounIdentity(raw), D(0));
    }
    ut.ensure_musk_mack_core_cache_context(&context);
    assert!(
        ut.musk.mack_core_cache_raw.is_empty(),
        "over the cap is dropped"
    );
}

#[test]
fn musk_eval_stack_copy_shares_repeated_indirect_atoms() {
    let mut slab = NounSlab::new();
    let big = Atom::from_bytes(&mut slab, &[7u8; 16]).as_noun();
    let noun = T(&mut slab, &[big, big, D(3)]);
    let mut ut = Ut::new(&mut slab);
    let mut context = create_musk_eval_context();
    ut.ensure_musk_mack_core_cache_context(&context);
    let space = ut.slab.noun_space();
    let copied = unsafe { ut.copy_into_eval_stack_shared(&mut context, noun, &space) };
    let stack_space = context.stack.noun_space();
    let cell = copied.in_space(&stack_space).as_cell().expect("cell");
    let tail = cell.tail().as_cell().expect("tail cell");
    let head = cell.head().noun();
    assert!(
        unsafe { head.raw_equals(&tail.head().noun()) },
        "the second occurrence reuses the first copy"
    );
    assert!(unsafe { tail.tail().noun().raw_equals(&D(3)) });
    let atom = cell.head().as_atom().expect("atom");
    assert_eq!(atom.as_ne_bytes()[..16], [7u8; 16]);
    // Copying the same source cell again returns the cached copy.
    let again = unsafe { ut.copy_into_eval_stack_shared(&mut context, noun, &space) };
    assert!(unsafe { again.raw_equals(&copied) });
}

// ---------------------------------------------------------------------------
// Noun-keyed boundary caches: structural fallbacks, collisions, eviction
// ---------------------------------------------------------------------------

/// Three structurally equal copies of a type at distinct addresses, plus a
/// supply of structurally different types for colliding bucket entries.
struct BoundaryNouns {
    a: [Noun; 3],
    b: [Noun; 3],
    c: [Noun; 3],
    others: Vec<Noun>,
}

fn boundary_nouns(slab: &mut NounSlab) -> BoundaryNouns {
    let mut copies = |n: u64| [atom_ty(slab, n), atom_ty(slab, n), atom_ty(slab, n)];
    let a = copies(1);
    let b = copies(2);
    let c = copies(3);
    let others = (0..40u64).map(|n| atom_ty(slab, 1000 + n)).collect();
    BoundaryNouns { a, b, c, others }
}

#[test]
fn redo_boundary_compares_structurally_and_evicts_full_buckets() {
    let mut slab = NounSlab::new();
    let n = boundary_nouns(&mut slab);
    let mut ut = Ut::new(&mut slab);
    let [sut, sut2, sut3] = n.a;
    let [ref_, ref2, ref3] = n.b;
    let result = n.c[0];
    let key = TypeBinaryKey {
        subject: ut.noun_mug_cached(sut),
        reference: ut.noun_mug_cached(ref_),
        vet: VetMode(ut.vet),
        fan: FanContextId(0),
    };

    ut.redo_boundary_store(sut, ref_, result).expect("store");
    // Exact and structural re-stores are deduplicated.
    ut.redo_boundary_store(sut, ref_, result)
        .expect("store same");
    ut.redo_boundary_store(sut2, ref2, result)
        .expect("store copy");
    assert_eq!(ut.boundary_memo.redo.get(&key).expect("bucket").len(), 1);
    // Colliding entries (wrong subject, wrong reference) ahead of the match.
    {
        let bucket = ut
            .boundary_memo
            .redo
            .ensure_key(key, Ut::REDO_CACHE_KEY_LIMIT);
        bucket.push_front(UnaryTypeBoundaryEntry {
            sut: n.others[0],
            ref_,
            result,
        });
        bucket.push_front(UnaryTypeBoundaryEntry {
            sut,
            ref_: n.others[1],
            result,
        });
        bucket.push_back(UnaryTypeBoundaryEntry {
            sut: n.others[2],
            ref_,
            result,
        });
        bucket.push_back(UnaryTypeBoundaryEntry {
            sut: sut3,
            ref_: n.others[3],
            result,
        });
    }
    ut.redo_boundary_store(sut3, ref3, result)
        .expect("store past collisions");
    let hit = ut
        .redo_boundary_lookup(sut3, ref3)
        .expect("lookup")
        .expect("structural hit past collisions");
    assert!(unsafe { hit.raw_equals(&result) });

    // A bucket of only colliding entries misses; a full bucket evicts.
    {
        let bucket = ut
            .boundary_memo
            .redo
            .ensure_key(key, Ut::REDO_CACHE_KEY_LIMIT);
        bucket.clear();
        for other in &n.others[..Ut::REDO_CACHE_BUCKET_LIMIT] {
            bucket.push_back(UnaryTypeBoundaryEntry {
                sut: *other,
                ref_,
                result,
            });
        }
    }
    assert!(ut.redo_boundary_lookup(sut, ref_).expect("miss").is_none());
    ut.redo_boundary_store(sut, ref_, result)
        .expect("evicting store");
    let bucket = ut.boundary_memo.redo.get(&key).expect("bucket");
    assert_eq!(bucket.len(), Ut::REDO_CACHE_BUCKET_LIMIT);
    assert!(unsafe { bucket.back().expect("newest").sut.raw_equals(&sut) });
    assert!(ut.redo_boundary_lookup(sut2, ref2).expect("hit").is_some());
}

#[test]
fn rest_boundary_compares_structurally_and_evicts_full_buckets() {
    let mut slab = NounSlab::new();
    let n = boundary_nouns(&mut slab);
    let mut ut = Ut::new(&mut slab);
    let [sut, sut2, sut3] = n.a;
    let [legs, legs2, legs3] = n.b;
    let result = n.c[0];
    let key = RestKey {
        subject: ut.noun_mug_cached(sut),
        legs: ut.noun_mug_cached(legs),
        vet: VetMode(ut.vet),
        fan: FanContextId(0),
    };

    ut.rest_boundary_store(sut, legs, result).expect("store");
    ut.rest_boundary_store(sut, legs, result)
        .expect("store same");
    ut.rest_boundary_store(sut2, legs2, result)
        .expect("store copy");
    assert_eq!(ut.boundary_memo.rest.get(&key).expect("bucket").len(), 1);
    {
        let bucket = ut
            .boundary_memo
            .rest
            .ensure_key(key, Ut::REST_CACHE_KEY_LIMIT);
        bucket.push_front(RestCacheEntry {
            sut: n.others[0],
            legs,
            result,
        });
        bucket.push_front(RestCacheEntry {
            sut,
            legs: n.others[1],
            result,
        });
        bucket.push_back(RestCacheEntry {
            sut: n.others[2],
            legs,
            result,
        });
        bucket.push_back(RestCacheEntry {
            sut: sut3,
            legs: n.others[3],
            result,
        });
    }
    ut.rest_boundary_store(sut3, legs3, result)
        .expect("store past collisions");
    assert!(ut
        .rest_boundary_lookup(sut3, legs3)
        .expect("lookup")
        .is_some());

    {
        let bucket = ut
            .boundary_memo
            .rest
            .ensure_key(key, Ut::REST_CACHE_KEY_LIMIT);
        bucket.clear();
        for other in &n.others[..Ut::REST_CACHE_BUCKET_LIMIT] {
            bucket.push_back(RestCacheEntry {
                sut: *other,
                legs,
                result,
            });
        }
    }
    assert!(ut.rest_boundary_lookup(sut, legs).expect("miss").is_none());
    ut.rest_boundary_store(sut, legs, result)
        .expect("evicting store");
    let bucket = ut.boundary_memo.rest.get(&key).expect("bucket");
    assert_eq!(bucket.len(), Ut::REST_CACHE_BUCKET_LIMIT);
    assert!(ut.rest_boundary_lookup(sut2, legs2).expect("hit").is_some());
}

#[test]
fn mint_boundary_exact_compares_structurally_and_evicts_full_buckets() {
    let mut slab = NounSlab::new();
    let n = boundary_nouns(&mut slab);
    let gen = hoon_to_noun(&mut slab, &axis(2));
    let gen2 = hoon_to_noun(&mut slab, &axis(2));
    let gen_other = hoon_to_noun(&mut slab, &axis(3));
    let mut ut = Ut::new(&mut slab);
    let [sut, sut2, _] = n.a;
    let [gol, gol2, _] = n.b;
    let [ty, formula, _] = n.c;
    let gen_sig = HoonSignature(u64::from(ut.noun_mug_cached(gen).0));
    let key = ut.mint_cache_key(sut, gol, gen_sig);

    ut.mint_boundary_store_exact(sut, gol, gen, ty, formula)
        .expect("store");
    ut.mint_boundary_store_exact(sut, gol, gen, ty, formula)
        .expect("store same");
    ut.mint_boundary_store_exact(sut2, gol2, gen2, ty, formula)
        .expect("store copy");
    assert_eq!(ut.boundary_memo.mint.get(&key).expect("bucket").len(), 1);
    let collide = |sut, gol, gen| MintCacheEntry {
        sut,
        gol,
        gen,
        ty,
        formula,
    };
    {
        let bucket = ut
            .boundary_memo
            .mint
            .ensure_key(key, Ut::MINT_CACHE_KEY_LIMIT);
        bucket.clear();
        bucket.push_back(collide(n.others[0], gol, gen));
        bucket.push_back(collide(sut, n.others[1], gen));
        bucket.push_back(collide(sut, gol, gen_other));
    }
    assert!(ut
        .mint_boundary_lookup_exact(sut2, gol2, gen2)
        .expect("miss")
        .is_none());
    ut.mint_boundary_store_exact(sut2, gol2, gen2, ty, formula)
        .expect("store past collisions");
    assert!(ut
        .mint_boundary_lookup_exact(sut, gol, gen)
        .expect("hit")
        .is_some());
    // The bucket is now full; one more distinct entry evicts the oldest.
    {
        let bucket = ut
            .boundary_memo
            .mint
            .ensure_key(key, Ut::MINT_CACHE_KEY_LIMIT);
        assert_eq!(bucket.len(), Ut::MINT_CACHE_BUCKET_LIMIT);
        bucket.pop_back();
        bucket.push_back(collide(n.others[2], gol, gen));
    }
    ut.mint_boundary_store_exact(sut, gol, gen, ty, formula)
        .expect("evicting store");
    let bucket = ut.boundary_memo.mint.get(&key).expect("bucket");
    assert_eq!(bucket.len(), Ut::MINT_CACHE_BUCKET_LIMIT);
    assert!(unsafe { bucket.front().expect("oldest").sut.raw_equals(&sut) });
}

#[test]
fn nest_mug_memo_compares_structurally_and_evicts_full_buckets() {
    let mut slab = NounSlab::new();
    let n = boundary_nouns(&mut slab);
    let mut ut = Ut::new(&mut slab);
    let [sut, sut2, _] = n.a;
    let [ref_, ref2, _] = n.b;
    assert_eq!(ut.nest_mug_lookup(sut, ref_).expect("empty"), None);
    ut.nest_mug_register(sut, ref_, true);
    assert_eq!(ut.nest_mug_lookup(sut2, ref2).expect("hit"), Some(true));
    let key = TypeBinaryKey {
        subject: ut.noun_mug_cached(sut),
        reference: ut.noun_mug_cached(ref_),
        vet: VetMode(ut.vet),
        fan: FanContextId(0),
    };
    {
        let bucket = ut
            .boundary_memo
            .nest
            .ensure_key(key, Ut::NEST_MUG_KEY_LIMIT);
        bucket.push_back(NestCacheEntry {
            sut: n.others[0],
            ref_,
            result: false,
        });
        bucket.push_back(NestCacheEntry {
            sut,
            ref_: n.others[1],
            result: false,
        });
    }
    assert_eq!(ut.nest_mug_lookup(sut2, ref2).expect("hit"), Some(true));
    {
        let bucket = ut
            .boundary_memo
            .nest
            .ensure_key(key, Ut::NEST_MUG_KEY_LIMIT);
        bucket.clear();
        for other in &n.others[..Ut::NEST_MUG_BUCKET_LIMIT] {
            bucket.push_back(NestCacheEntry {
                sut: *other,
                ref_,
                result: false,
            });
        }
    }
    assert_eq!(ut.nest_mug_lookup(sut, ref_).expect("miss"), None);
    ut.nest_mug_register(sut, ref_, false);
    let bucket = ut.boundary_memo.nest.get(&key).expect("bucket");
    assert_eq!(bucket.len(), Ut::NEST_MUG_BUCKET_LIMIT);
    assert_eq!(ut.nest_mug_lookup(sut2, ref2).expect("hit"), Some(false));
}
