//! Coverage-driven tests for `ut/mod.rs` (last third).
//!
//! Added to close branch-coverage gaps; see the coverage report in the PR.
//! Source-level cases mirror the parity probes in
//! `test-assets/type-probes/coverage/c3/`, whose results match hoonc, so the
//! exact types pinned here are hoonc's too. Rejection cases were checked
//! against hoonc with the same sources (both compilers reject them).

use chumsky::Parser as _;
use hatch::utils::LineMap;

#[allow(unused_imports)]
use super::super::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parses a wide or tall Hoon expression with the native parser.
fn parse(src: &str) -> Hoon {
    let linemap = Arc::new(LineMap::new(src));
    hatch::native_parser(vec!["test".to_string()], false, linemap)
        .parse(src)
        .into_result()
        .unwrap_or_else(|errs| panic!("parse {src:?}: {errs:?}"))
}

/// Renders a noun as `[a b]` with lowercase-text atoms as `%term`.
fn show(slab: &NounSlab, n: Noun) -> String {
    fn go(space: &NounSpace, n: Noun, out: &mut String) {
        match n.in_space(space).as_cell() {
            Ok(c) => {
                out.push('[');
                go(space, c.head().noun(), out);
                out.push(' ');
                go(space, c.tail().noun(), out);
                out.push(']');
            }
            Err(_) => match n.in_space(space).as_atom() {
                Ok(a) => match atom_to_string(a) {
                    Ok(s) if !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase()) => {
                        out.push('%');
                        out.push_str(&s);
                    }
                    _ => match a.as_u64() {
                        Ok(v) => out.push_str(&v.to_string()),
                        Err(_) => out.push_str("BIG"),
                    },
                },
                Err(_) => out.push('?'),
            },
        }
    }
    let space = slab.noun_space();
    let mut out = String::new();
    go(&space, n, &mut out);
    out
}

/// Named molds in a core, then typed legs pushed with `^*` (the mold's type,
/// not a constant). Mirrors the sample types of the `c3_skin_*` probes.
const SKIN_PRELUDE: &str = "=>  |%
    +$  num  @ud
    +$  duo  [@ @]
    +$  r  $@(@ r)
    +$  fac  x=@
    +$  duf  x=[p=@ q=@]
    --
=+  ^*([a=* b=@ud c=$@(@ [p=@ q=@]) e=[@ @] f=$@(x=@ [@ @]) g=$@(@ x=[p=@ q=@]) n=num u=duo h=fac k=duf w=$@(@ [* *]) v=r t=@t])
";

/// Mints `src` against a `%noun` subject; returns the lowered type and formula.
fn mint_src(src: &str) -> std::result::Result<(String, String), String> {
    let mut slab = NounSlab::new();
    let noun = ty_noun(&mut slab);
    let gen = parse(src);
    let mut ut = Ut::new(&mut slab);
    let out = ut.mint_noun(noun, noun, &gen);
    match out {
        Ok((ty, formula)) => Ok((show(&slab, ty), show(&slab, formula))),
        Err(err) => Err(format!("{err:?}")),
    }
}

/// Mints `expr` under [`SKIN_PRELUDE`] and returns the product type.
fn skin_ty(expr: &str) -> std::result::Result<String, String> {
    mint_src(&format!("{SKIN_PRELUDE}{expr}")).map(|(ty, _)| ty)
}

fn skin_ok(expr: &str) -> String {
    skin_ty(expr).unwrap_or_else(|err| panic!("{expr}: {err}"))
}

fn skin_err(expr: &str) -> String {
    match skin_ty(expr) {
        Ok(ty) => panic!("{expr}: expected rejection, got {ty}"),
        Err(err) => err,
    }
}

/// Wraps `body` as the wet arm `w` (sample `a=<decl>`) and calls it on `arg`.
fn wet_src(decl: &str, body: &str, arg: &str) -> String {
    format!("=>  |%\n    ++  w\n      |*  a={decl}\n      {body}\n    --\n(w {arg})\n")
}

fn wet_ty(decl: &str, body: &str, arg: &str) -> std::result::Result<String, String> {
    mint_src(&wet_src(decl, body, arg)).map(|(ty, _)| ty)
}

/// Plays `src` against a `%noun` subject.
fn play_ty(ut: &mut Ut<'_>, src: &str) -> NRc<NTy> {
    let noun = cons_noun(&mut ut.cx);
    let gen = parse(src);
    ut.play(noun, &gen)
        .unwrap_or_else(|err| panic!("play {src}: {err:?}"))
}

/// The type of `^*(<spec>)`: the mold's type.
fn spec_ty(ut: &mut Ut<'_>, spec: &str) -> NRc<NTy> {
    play_ty(ut, &format!("^*({spec})"))
}

fn lower(ut: &mut Ut<'_>, ty: &NRc<NTy>) -> String {
    let noun = live_to_noun(&mut ut.cx, ty, ut.slab);
    show(ut.slab, noun)
}

fn with_ut<R>(f: impl FnOnce(&mut Ut<'_>) -> R) -> R {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    f(&mut ut)
}

fn atom_ty(ut: &mut Ut<'_>, aura: &str, value: Option<u64>) -> NRc<NTy> {
    ty_atom_n(&mut ut.cx, ut.slab, aura, value.map(D)).1
}

fn term(ut: &mut Ut<'_>, name: &str) -> Noun {
    term_to_noun(ut.slab, name)
}

fn noun_err<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result {
        Ok(value) => panic!("expected an error, got {value:?}"),
        Err(err) => format!("{err:?}"),
    }
}

const ATOM: &str = "[%atom [0 0]]";
const CELL_NOUN: &str = "[%cell [%noun %noun]]";

// ---------------------------------------------------------------------------
// ?# gain/lose through source (mod.rs ~9642-10273)
// ---------------------------------------------------------------------------

#[test]
fn c3_skin_atom_and_leaf_refinement() {
    // Atom skin on a noun ref: gain @, lose [* *].
    assert_eq!(skin_ok("?:(?#(@ a) a !!)"), ATOM);
    assert_eq!(skin_ok("?:(?#(@ a) !! a)"), CELL_NOUN);
    // Leaf skins: a noun ref gains the constant; an @ud ref keeps its aura.
    assert_eq!(skin_ok("?:(?#(%5 a) a !!)"), "[%atom [%ud [0 5]]]");
    assert_eq!(skin_ok("?:(?#(%5 b) b !!)"), "[%atom [%ud [0 5]]]");
    assert_eq!(skin_ok("?:(?#(%5 b) !! b)"), "[%atom [%ud 0]]");
    // Constant refs: a mismatched constant gains %void; a match keeps it.
    assert_eq!(skin_ok("=/(c %5 ?:(?#(%6 c) !! c))"), "[%atom [%ud [0 5]]]");
    assert_eq!(skin_ok("=/(c %5 ?:(?#(%5 c) c !!))"), "[%atom [%ud [0 5]]]");
    // A cell ref gains %void and keeps itself on the lose side.
    assert_eq!(
        skin_ok("?:(?#(%5 e) !! e)"),
        "[%cell [[%atom [0 0]] [%atom [0 0]]]]"
    );
    // Fork refs with faced atom options (gain strips the face, lose keeps it).
    assert_eq!(
        skin_ok("?:(?#(%5 f) f f)"),
        "[%fork [[%atom [%ud [0 5]]] [[[%cell [[%atom [0 0]] [%atom [0 0]]]] [0 0]] [[%face [%x [%atom [0 0]]]] [0 0]]]]]"
    );
    assert_eq!(
        skin_ok("?:(?#(@ f) f f)"),
        "[%fork [[%atom [0 0]] [[[%cell [[%atom [0 0]] [%atom [0 0]]]] [0 0]] 0]]]"
    );
    assert!(skin_ok("?:(?#(%x f) f f)").starts_with("[%fork [[%face [%x [%atom [0 0]]]]"));
    // Hint refs (named molds) keep the hint on both sides.
    assert!(skin_ok("?:(?#(%5 n) n n)").starts_with("[%fork [[%hint "));
    assert!(skin_ok("?:(?#(@ h) h !!)").starts_with("[%hint "));
    assert!(skin_ok("?:(?#(%5 h) h h)").starts_with("[%fork [[%face [%x [%hint "));
    // The self-referential hold `r` terminates through the per-skin hold guard.
    assert!(skin_ok("?:(?#(@ v) v !!)").starts_with("[%hint "));
    assert!(skin_ok("?:(?#(%5 v) v v)").starts_with("[%fork [[%hint "));
}

#[test]
fn c3_skin_cell_refinement() {
    assert_eq!(
        skin_ok("?:(?#(^ c) c c)"),
        "[%fork [[%cell [[%face [%p [%atom [0 0]]]] [%face [%q [%atom [0 0]]]]]] [[[%atom [0 0]] [0 0]] 0]]]"
    );
    assert_eq!(
        skin_ok("?:(?#(^ g) g g)"),
        "[%fork [[%cell [[%face [%p [%atom [0 0]]]] [%face [%q [%atom [0 0]]]]]] [[[%atom [0 0]] [0 0]] 0]]]"
    );
    assert_eq!(
        skin_ok("?:(?#([@ @] w) w w)"),
        "[%fork [[%cell [%noun [%cell [%noun %noun]]]] [[[%cell [[%cell [%noun %noun]] %noun]] [[[%cell [[%atom [0 0]] [%atom [0 0]]]] [0 0]] [[%cell [[%cell [%noun %noun]] [%cell [%noun %noun]]]] [[[%atom [0 0]] [0 0]] 0]]]] 0]]]"
    );
    assert_eq!(
        skin_ok("?:(?#([^ @] w) w w)"),
        "[%fork [[%cell [%noun [%cell [%noun %noun]]]] [[[%atom [0 0]] [[[%cell [[%atom [0 0]] %noun]] [[[%cell [[%atom [0 0]] [%cell [%noun %noun]]]] [0 0]] 0]] 0]] [[%cell [[%cell [%noun %noun]] [%atom [0 0]]]] [0 0]]]]]"
    );
    let known = "[%cell [[%atom [0 0]] [%atom [0 0]]]]";
    assert_eq!(skin_ok("?:(?#([@ @] e) e !!)"), known);
    // Head skin ^ on an atom head: the gain is %void, the lose keeps the cell.
    assert_eq!(skin_ok("?:(?#([^ @] e) !! e)"), known);
    assert!(skin_ok("?:(?#(^ u) u !!)").starts_with("[%hint "));
    assert!(skin_ok("?:(?#(^ k) k !!)").starts_with("[%hint "));
    assert!(skin_ok("?:(?#(^ v) !! v)").starts_with("[%hint "));
    // `ar:lose` `%cell` strips a faced sub-ref: losing `^` from `q=@` leaves
    // a bare `@`.
    let lost = skin_ok("?:(?#([@ ^] c) !! c)");
    assert!(
        lost.contains("[%cell [[%face [%p [%atom [0 0]]]] [%atom [0 0]]]]"),
        "{lost}"
    );
    assert!(!lost.contains("[%face [%q "), "{lost}");
}

#[test]
fn c3_skin_core_refs() {
    // Atom and leaf skins on a core gain %void and leave the core on the lose side.
    assert!(skin_ok("=/(q |.(a) ?:(?#(@ q) !! q))").starts_with("[%core "));
    assert!(skin_ok("=/(q |.(a) ?:(?#(%5 q) !! q))").starts_with("[%core "));
    // Cell skins whose tail is neither the %noun term nor [%base %noun] refine
    // the core as a plain cell; a void head gains %void.
    assert!(skin_ok("=/(q |.(a) ?:(?#([* @] q) q !!))").starts_with("[%cell [[%cell "));
    assert!(skin_ok("=/(q |.(a) ?:(?#([@ @] q) !! q))").starts_with("[%cell [[%cell "));
    // Only the term tail `noun` keeps the core; `^` (a `[%base %noun]` tail)
    // gains a plain cell, and loses the tail from %noun, which is %void.
    assert!(skin_ok("=/(q |.(a) =/(noun * ?:(?#([* noun] q) q !!)))").starts_with("[%core "));
    assert!(skin_ok("=/(q |.(a) ?:(?#(^ q) q !!))").starts_with("[%cell [[%cell "));
    assert!(skin_err("=/(q |.(a) ?:(?#([@ *] q) !! %foo))").contains("mint-vain"));
    // ?= with fork and faced refs crop a core through crop_sint.
    assert!(skin_ok("=/(q |.(a) ?:(?=(?(%a %b) q) !! q))").starts_with("[%core "));
    assert!(skin_ok("=/(q |.(a) ?:(?=(x=@ q) !! q))").starts_with("[%core "));
}

#[test]
fn c3_skin_name_term_and_over() {
    assert!(skin_ok("?:(?#(num b) b !!)").starts_with("[%hint "));
    assert_eq!(
        skin_ok("?:(?#(x=@ a) a a)"),
        "[%fork [[%cell [%noun %noun]] [0 [[%face [%x [%atom [0 0]]]] [0 0]]]]]"
    );
    assert_eq!(
        skin_ok("?:(?#(=>(+3 @) a) a a)"),
        "[%fork [[%cell [%noun %noun]] [0 [[%atom [0 0]] [0 0]]]]]"
    );
}

#[test]
fn c3_skin_flag_noun_cell_and_synthetic_ports() {
    // %flag on a noun ref: gain the two loobeans; lose both leaves (still %noun).
    let flag = skin_ok("?:(?#(? a) a a)");
    assert!(
        flag.contains("[%atom [%f [0 0]]]") && flag.contains("[%atom [%f [0 1]]]"),
        "{flag}"
    );
    assert!(flag.contains("%noun"), "{flag}");
    // A non-generic cell skin loses nothing from a noun ref.
    let cell = skin_ok("?:(?#([@ @] a) !! a)");
    assert_eq!(cell, "%noun");
    // `?=` on a =* alias of a computed hoon finds a synthetic port: no refinement.
    let alias = skin_ok("=*  x  [a b]  ?:(?=([* @] x) x x)");
    assert_eq!(alias, "[%cell [%noun [%atom [%ud 0]]]]");
    // ?# fends its wing first, so an alias is a fend-fragment (hoon-138 too).
    assert!(skin_err("=*  x  [a b]  ?:(?#([* @] x) x x)").contains("fend-fragment"));
}

#[test]
fn c3_skin_rejections() {
    // hoon-138 `ar:gain` crashes with atom-mismatch when the aura does not fit.
    assert!(skin_err("?:(?#(@ud t) t !!)").contains("atom-mismatch"));
    assert!(skin_err("?:(?#(%5 t) t !!)").contains("atom-mismatch"));
    // ?= on the self-referential hold re-enters (r, @) in fuse: hoon-138 fuse-loop.
    assert!(skin_err("?:(?=(@ v) v v)").contains("fuse-loop"));
}

// ---------------------------------------------------------------------------
// gain_skin / lose_skin on constructed skins and refs
// ---------------------------------------------------------------------------

fn spot() -> Spot {
    Spot {
        p: vec!["c3".to_string()],
        q: Pint {
            p: (1, 1),
            q: (1, 2),
        },
    }
}

fn atom_skin(aura: &str) -> Skin {
    Skin::Base(BaseType::Atom(aura.to_string()))
}

fn noun_skin() -> Skin {
    Skin::Base(BaseType::NounExpr)
}

#[test]
fn c3_gain_lose_base_skins_not_built_by_flay() {
    with_ut(|ut| {
        let sut = cons_noun(&mut ut.cx);
        let noun = cons_noun(&mut ut.cx);
        let void = cons_void(&mut ut.cx);

        // %null gains the ~ constant; losing it from a noun ref keeps %noun.
        let t = ut
            .gain_skin(sut.clone(), noun.clone(), &Skin::Base(BaseType::Null))
            .unwrap();
        assert_eq!(lower(ut, &t), "[%atom [%n [0 0]]]");
        let t = ut
            .lose_skin(sut.clone(), noun.clone(), &Skin::Base(BaseType::Null))
            .unwrap();
        assert_eq!(lower(ut, &t), "%noun");

        // %void gains nothing and loses nothing.
        let t = ut
            .gain_skin(sut.clone(), noun.clone(), &Skin::Base(BaseType::Void))
            .unwrap();
        assert_eq!(lower(ut, &t), "%void");
        let t = ut
            .lose_skin(sut.clone(), noun.clone(), &Skin::Base(BaseType::Void))
            .unwrap();
        assert_eq!(lower(ut, &t), "%noun");

        // %dbug and %help wrappers: gain adds a %help hint, lose passes through.
        let dbug = Skin::Dbug(spot(), Box::new(atom_skin("")));
        let t = ut.gain_skin(sut.clone(), noun.clone(), &dbug).unwrap();
        assert_eq!(lower(ut, &t), ATOM);
        let t = ut.lose_skin(sut.clone(), noun.clone(), &dbug).unwrap();
        assert_eq!(lower(ut, &t), CELL_NOUN);
        let help = Skin::Help(
            NounExpr::ParsedAtom(ParsedAtom::Small(7)),
            Box::new(atom_skin("")),
        );
        let t = ut.gain_skin(sut.clone(), noun.clone(), &help).unwrap();
        assert_eq!(lower(ut, &t), "[%hint [[%noun [%help 7]] [%atom [0 0]]]]");
        let t = ut.lose_skin(sut.clone(), noun.clone(), &help).unwrap();
        assert_eq!(lower(ut, &t), CELL_NOUN);

        // %wash: hoon-138 `ar:gain` recurses forever (hoonc hangs), so honk
        // rejects it; `ar:lose` leaves the ref alone.
        let err = noun_err(ut.gain_skin(sut.clone(), noun.clone(), &Skin::Wash(0)));
        assert!(err.contains("gain-wash"), "{err}");
        let t = ut
            .lose_skin(sut.clone(), noun.clone(), &Skin::Wash(0))
            .unwrap();
        assert_eq!(lower(ut, &t), "%noun");

        // A void head under a cell skin collapses the gain on a noun ref.
        let cell = Skin::Cell(Box::new(Skin::Base(BaseType::Void)), Box::new(noun_skin()));
        let t = ut.gain_skin(sut.clone(), noun.clone(), &cell).unwrap();
        assert_eq!(lower(ut, &t), "%void");

        // Void refs stay void under every skin family.
        let leaf = Skin::Leaf("ud".to_string(), ParsedAtom::Small(5));
        let pair = Skin::Cell(Box::new(atom_skin("")), Box::new(atom_skin("")));
        for skin in [noun_skin(), atom_skin("ud"), leaf.clone(), pair.clone()] {
            let t = ut.gain_skin(sut.clone(), void.clone(), &skin).unwrap();
            assert_eq!(lower(ut, &t), "%void", "gain {skin:?}");
        }
        for skin in [atom_skin("ud"), leaf.clone(), pair] {
            let t = ut.lose_skin(sut.clone(), void.clone(), &skin).unwrap();
            assert_eq!(lower(ut, &t), "%void", "lose {skin:?}");
        }
        // Losing a leaf from a noun ref keeps %noun.
        let t = ut.lose_skin(sut, noun, &leaf).unwrap();
        assert_eq!(lower(ut, &t), "%noun");
    });
}

#[test]
fn c3_gain_skin_any_aura_leaves_keep_ref_aura() {
    with_ut(|ut| {
        let sut = cons_noun(&mut ut.cx);
        let ud = atom_ty(ut, "ud", None);
        // Leaf and atom skins with an empty or `@` aura take the ref's aura.
        for aura in ["", "@"] {
            let leaf = Skin::Leaf(aura.to_string(), ParsedAtom::Small(5));
            let t = ut.gain_skin(sut.clone(), ud.clone(), &leaf).unwrap();
            assert_eq!(lower(ut, &t), "[%atom [%ud [0 5]]]", "leaf aura {aura:?}");
        }
        let t = ut
            .gain_skin(sut.clone(), ud.clone(), &atom_skin("@"))
            .unwrap();
        assert_eq!(lower(ut, &t), "[%atom [%ud 0]]");
        // A narrower atom skin on a wider ref takes the larger aura.
        let u = atom_ty(ut, "u", None);
        let t = ut.gain_skin(sut, u, &atom_skin("ud")).unwrap();
        assert_eq!(lower(ut, &t), "[%atom [%ud 0]]");
    });
}

#[test]
fn c3_gain_lose_spec_skin_rejects_non_nesting_inner() {
    with_ut(|ut| {
        let sut = cons_noun(&mut ut.cx);
        let noun = cons_noun(&mut ut.cx);
        let spec = Skin::Spec(
            Box::new(Spec::Base(BaseType::Atom("ud".to_string()))),
            Box::new(noun_skin()),
        );
        assert!(noun_err(ut.gain_skin(sut.clone(), noun.clone(), &spec)).contains("gain spec"));
        // Losing %noun leaves %void, which nests anywhere; lose an atom instead.
        let spec = Skin::Spec(
            Box::new(Spec::Base(BaseType::Atom("ud".to_string()))),
            Box::new(atom_skin("")),
        );
        assert!(noun_err(ut.lose_skin(sut, noun, &spec)).contains("lose spec"));
    });
}

#[test]
fn c3_gain_lose_over_skin_plays_wing_in_subject() {
    with_ut(|ut| {
        let sut = spec_ty(ut, "[a=@ b=?]");
        let noun = cons_noun(&mut ut.cx);
        let over = Skin::Over(vec![Limb::Term("b".to_string())], Box::new(atom_skin("")));
        let t = ut.gain_skin(sut.clone(), noun.clone(), &over).unwrap();
        assert_eq!(lower(ut, &t), ATOM);
        let t = ut.lose_skin(sut, noun, &over).unwrap();
        assert_eq!(lower(ut, &t), CELL_NOUN);
    });
}

// ---------------------------------------------------------------------------
// fuse / crop / miss (mod.rs ~10278-10705)
// ---------------------------------------------------------------------------

#[test]
fn c3_fuse_and_crop_atoms_with_equal_constants() {
    with_ut(|ut| {
        let five_ud = atom_ty(ut, "ud", Some(5));
        let five_ux = atom_ty(ut, "ux", Some(5));
        let t = ut.fuse(five_ud.clone(), five_ux.clone()).unwrap();
        assert_eq!(lower(ut, &t), "[%atom [%ux [0 5]]]");
        let t = ut.crop(five_ud, five_ux).unwrap();
        assert_eq!(lower(ut, &t), "%void");
    });
}

#[test]
fn c3_fuse_and_crop_core_and_void_subjects() {
    with_ut(|ut| {
        let core = play_ty(ut, "|.(5)");
        let atom = atom_ty(ut, "", None);
        // fuse repos a core to [%cell %noun payload]; an atom ref misses it.
        let t = ut.fuse(core.clone(), atom.clone()).unwrap();
        assert_eq!(lower(ut, &t), "%void");
        // crop keeps a core against atom and fork refs.
        let t = ut.crop(core.clone(), atom.clone()).unwrap();
        assert!(lower(ut, &t).starts_with("[%core "));
        let fork = spec_ty(ut, "?(%a %b)");
        let t = ut.crop(core.clone(), fork).unwrap();
        assert!(lower(ut, &t).starts_with("[%core "));
        // crop_sint keeps the subject against a core ref and a leftover ref.
        let t = ut.crop(atom.clone(), core.clone()).unwrap();
        assert_eq!(lower(ut, &t), ATOM);
        let noun = cons_noun(&mut ut.cx);
        let mut seen = HashSet::new();
        let t = ut.crop_sint(atom.clone(), noun, &mut seen).unwrap();
        assert_eq!(lower(ut, &t), ATOM);
        // A void subject fuses to void.
        let void = cons_void(&mut ut.cx);
        let t = ut.fuse(void, atom).unwrap();
        assert_eq!(lower(ut, &t), "%void");
    });
}

/// The type of leg `v` (the self-referential `+$ r $@(@ r)`) and friends.
fn cyclic_types(ut: &mut Ut<'_>) -> (NRc<NTy>, NRc<NTy>, NRc<NTy>) {
    let r = play_ty(ut, "=>  |%  +$  r  $@(@ r)  --  ^*(r)");
    let p = play_ty(ut, "=>  |%  +$  p  $@(%a [p @])  --  ^*(p)");
    let q = play_ty(ut, "=>  |%  +$  q  $@(%b [q @])  --  ^*(q)");
    (r, p, q)
}

#[test]
fn c3_fuse_and_crop_detect_hold_loops() {
    with_ut(|ut| {
        let (r, _, _) = cyclic_types(ut);
        let atom = atom_ty(ut, "", None);
        assert!(noun_err(ut.fuse(r.clone(), atom.clone())).contains("fuse-loop"));
        assert!(noun_err(ut.crop(r, atom)).contains("crop-loop"));
    });
}

#[test]
fn c3_miss_covers_core_fork_constant_and_hold_cases() {
    with_ut(|ut| {
        let core = play_ty(ut, "|.(5)");
        let atom = atom_ty(ut, "", None);
        let a = atom_ty(ut, "tas", Some(0x61));
        let b = atom_ty(ut, "tas", Some(0x62));
        let c = atom_ty(ut, "tas", Some(0x63));
        // A core is a cell, so it misses every atom.
        assert!(ut.miss(core, atom.clone()).unwrap());
        // Distinct constants miss; a fork misses when every option does.
        assert!(ut.miss(a.clone(), b.clone()).unwrap());
        let ab = ut.cons_fork(vec![a.clone(), b.clone()]).unwrap();
        assert!(ut.miss(ab.clone(), c.clone()).unwrap());
        assert!(!ut.miss(ab, a.clone()).unwrap());
        // Cells miss when either side misses.
        let a_atom = cons_cell(&mut ut.cx, a.clone(), atom.clone());
        let b_atom = cons_cell(&mut ut.cx, b, atom.clone());
        assert!(ut.miss(a_atom, b_atom).unwrap());
        // Holds: the seen guard answers a repeated (sut, ref) pair, in either order.
        let (r, p, q) = cyclic_types(ut);
        let pair = cons_cell(&mut ut.cx, atom.clone(), atom.clone());
        // `r` = $@(@ r) holds only atoms, so it misses every cell.
        assert!(ut.miss(r.clone(), pair).unwrap());
        assert!(!ut.miss(r.clone(), atom.clone()).unwrap());
        assert!(ut.miss(p.clone(), q.clone()).unwrap());
        assert!(!ut.miss(p.clone(), p).unwrap());
        let _ = r;
    });
}

#[test]
fn c3_miss_memo_persistence_clears_on_context_change() {
    with_ut(|ut| {
        let a = atom_ty(ut, "tas", Some(0x61));
        let b = atom_ty(ut, "tas", Some(0x62));
        assert!(!ut.set_miss_memo_persistence(true));
        // Enabling twice is a no-op that reports the prior state.
        assert!(ut.set_miss_memo_persistence(true));
        assert!(ut.miss(a.clone(), b.clone()).unwrap());
        // Same context: the stored memo is reused.
        assert!(ut.miss(a.clone(), b.clone()).unwrap());
        // A vet flip changes the cache context key, so the memo is cleared.
        ut.vet = !ut.vet;
        assert!(ut.miss(a.clone(), b.clone()).unwrap());
        ut.vet = !ut.vet;
        assert!(ut.set_miss_memo_persistence(false));
        assert!(!ut.set_miss_memo_persistence(false));
        assert!(ut.miss(a, b).unwrap());
    });
}

// ---------------------------------------------------------------------------
// fitz / atom_max / cons_fork (mod.rs ~10707-10870)
// ---------------------------------------------------------------------------

#[test]
fn c3_fitz_size_suffixes() {
    with_ut(|ut| {
        let cases: [(&str, &str, bool); 9] = [
            ("uxE", "uxD", true),
            ("uxD", "uxE", false),
            ("uxD", "ux", true),
            ("ux", "uxD", true),
            ("D", "uD", true),
            ("uD", "D", true),
            ("D", "E", false),
            ("ud", "ux", false),
            ("t", "ta", true),
        ];
        for (yaz, wix, expect) in cases {
            let y = term(ut, yaz);
            let w = term(ut, wix);
            assert_eq!(ut.fitz(y, w).unwrap(), expect, "fitz({yaz}, {wix})");
        }
    });
}

#[test]
fn c3_atom_max_orders_auras_as_atoms() {
    with_ut(|ut| {
        let u = term(ut, "u");
        let ud = term(ut, "ud");
        let got = ut.atom_max(u, ud).unwrap();
        assert_eq!(show(ut.slab, got), "%ud");
        let got = ut.atom_max(ud, u).unwrap();
        assert_eq!(show(ut.slab, got), "%ud");
    });
}

#[test]
fn c3_cons_fork_collapses_single_void() {
    with_ut(|ut| {
        let void = cons_void(&mut ut.cx);
        let t = ut.cons_fork(vec![void]).unwrap();
        assert_eq!(lower(ut, &t), "%void");
        let t = ut.cons_fork(Vec::new()).unwrap();
        assert_eq!(lower(ut, &t), "%void");
    });
}

// ---------------------------------------------------------------------------
// peek / peel (mod.rs ~10894-11030, ~12674)
// ---------------------------------------------------------------------------

#[test]
fn c3_peek_void_and_cyclic_hold() {
    with_ut(|ut| {
        let void = cons_void(&mut ut.cx);
        let t = ut.peek(void, Way::Free, 2u64).unwrap();
        assert_eq!(lower(ut, &t), "%void");
        // `r` = $@(@ r): peeking its head re-enters the same hold at the same
        // axis, which the seen-hold guard cuts to %void.
        let (r, _, _) = cyclic_types(ut);
        let t = ut.peek(r, Way::Free, 2u64).unwrap();
        assert_eq!(lower(ut, &t), "%void");
    });
}

#[test]
fn c3_peek_through_variant_cores() {
    with_ut(|ut| {
        let zinc = play_ty(ut, "^&(|=(b=@ b))");
        let lead = play_ty(ut, "^?(|=(b=@ b))");
        let iron = play_ty(ut, "^|(|=(b=@ b))");
        // Zinc reads its sample but not its context.
        let sample = "[%face [%b [%atom [0 0]]]]";
        let t = ut.peek(zinc.clone(), Way::Read, 6u64).unwrap();
        assert_eq!(lower(ut, &t), sample);
        let t = ut.peek(zinc.clone(), Way::Read, 7u64).unwrap();
        assert_eq!(lower(ut, &t), "%noun");
        let t = ut.peek(zinc.clone(), Way::Read, 3u64).unwrap();
        assert_eq!(lower(ut, &t), format!("[%cell [{sample} %noun]]"));
        // Free sees everything; lead and iron read nothing.
        let t = ut.peek(zinc.clone(), Way::Free, 6u64).unwrap();
        assert_eq!(lower(ut, &t), sample);
        let t = ut.peek(lead.clone(), Way::Read, 6u64).unwrap();
        assert_eq!(lower(ut, &t), "%noun");
        let t = ut.peek(iron.clone(), Way::Rite, 6u64).unwrap();
        assert_eq!(lower(ut, &t), sample);
        // Blocked non-read access is a payload-block crash (hoon-138 `!!`).
        assert!(noun_err(ut.peek(zinc, Way::Both, 6u64)).contains("payload-block"));
        assert!(noun_err(ut.peek(lead, Way::Rite, 6u64)).contains("payload-block"));
        // The battery side of a core peeks as %noun.
        let t = ut.peek(iron, Way::Read, 2u64).unwrap();
        assert_eq!(lower(ut, &t), "%noun");
    });
    assert_eq!(peel(Way::Both, Vair::Zinc), (false, false));
    assert_eq!(peel(Way::Free, Vair::Lead), (true, true));
    assert_eq!(peel(Way::Rite, Vair::Iron), (true, false));
    assert_eq!(peel(Way::Read, Vair::Gold), (true, true));
}

// ---------------------------------------------------------------------------
// mull (mod.rs ~11062-11806)
// ---------------------------------------------------------------------------

fn wet_ok(decl: &str, body: &str, arg: &str) {
    let ty = wet_ty(decl, body, arg).unwrap_or_else(|err| panic!("a={decl} {body} ({arg}): {err}"));
    // A wet call's product is a lazily-expanded %hold over the edited core.
    assert!(ty.starts_with("[%hold "), "a={decl} {body} ({arg}): {ty}");
}

fn wet_err(decl: &str, body: &str, arg: &str) -> String {
    match wet_ty(decl, body, arg) {
        Ok(ty) => panic!("a={decl} {body} ({arg}): expected rejection, got {ty}"),
        Err(err) => err,
    }
}

#[test]
fn c3_mull_wet_bodies() {
    // Each body is mulled at the call-site sample (@ud or a pair) against the
    // declared sample.
    let cases: [(&str, &str, &str); 18] = [
        ("*", ".*(a [0 1])", "5"),
        ("@", "+(a)", "5"),
        ("*", "^|(|.(a))", "5"),
        ("*", "^&(|.(a))", "5"),
        ("*", "^?(|.(a))", "5"),
        ("*", "^.(|=(b=* b) a)", "5"),
        ("[b=* c=*]", "=,(a b)", "[1 2]"),
        ("*", "=-(- a)", "5"),
        ("*", "!=(a)", "5"),
        ("*", "[!@(a 1 2) !@(nope 3 4)]", "5"),
        ("*", "?:(=(a 0) !! a)", "5"),
        ("*", "?#(@ a)", "5"),
        // sut gain void, dox gain non-void: the dox branch is only played.
        ("*", "?:(?=(@ a) !! 2)", "[1 2]"),
        // sut lose void, dox lose non-void.
        ("*", "?:(?=(@ a) 1 !!)", "5"),
        // both gains void.
        ("@", "?:(?=(^ a) !! 2)", "5"),
        ("*", ".^(@ a)", "5"),
        ("*", "~!(a a)", "5"),
        // Both sides lose to %void: the else branch is never played.
        ("@", "?:(?=(@ a) 1 !!)", "5"),
    ];
    for (decl, body, arg) in cases {
        if body.starts_with("~!") {
            // Wide `~!(p q)` does not parse in hatch; use the tall form.
            wet_ok(decl, "~!  a\n      a", arg);
        } else {
            wet_ok(decl, body, arg);
        }
    }
}

#[test]
fn c3_mull_folds_arms_of_mulled_cores() {
    // The `?=` wing is an alias to `^~(+4)`, an arm formula in the battery of
    // a core built in the wet body. hoon-138's `++mile` battery is `++laze`,
    // which mints the arm, so the fold succeeds and `++cove` rejects the
    // constant; `+3` holds the unknown sample and stays a slot.
    let body = |axis: &str| format!("=>  |%  ++  x  1  ++  y  2  --  =*  z  ^~(+{axis})  ?=(@ z)");
    assert!(wet_err("*", &body("4"), "5").contains("cove"));
    wet_ok("*", &body("3"), "5");
}

#[test]
fn c3_mull_wet_rejections() {
    // Refinement differs between the call-site and the declared sample.
    assert!(wet_err("@", "?:(?=(^ a) 1 2)", "[1 2]").contains("mull-bonk-b"));
    assert!(wet_err("@", "?:(?=(@ a) 1 2)", "^*(*)").contains("mull-bonk-c"));
    // ?= and ?# on a wing whose axis moved between the two samples.
    // ?= whose pattern type depends on the sample fishes differently per side.
    assert!(wet_err("*", "?=(_a a)", "5").contains("mull-bonk-a"));
    // ?# on a wing whose call-site type does not nest in the declared one.
    assert!(wet_err("@", "?#(@ a)", "[1 2]").contains("mull-bonk-x"));
    // !@ sees p.a at the call site but not in the declared sample.
    assert!(wet_err("*", "!@(p.a 1 2)", "[p=1 q=2]").contains("mull-bonk-f"));
    // A void subject reaches mull: mull-none.
    assert!(wet_err("*", "=>(!! a)", "5").contains("mull-none"));
}

fn mull_types(
    ut: &mut Ut<'_>,
    sut: NRc<NTy>,
    dox: NRc<NTy>,
    gen: &Hoon,
) -> Result<(String, String)> {
    let gol = cons_noun(&mut ut.cx);
    let (p, q) = ut.mull(sut, gol, dox, gen)?;
    Ok((lower(ut, &p), lower(ut, &q)))
}

#[test]
fn c3_mull_direct_forms() {
    with_ut(|ut| {
        let noun = cons_noun(&mut ut.cx);
        let sand = |n: u128| {
            Box::new(Hoon::Sand(
                "ud".to_string(),
                NounExpr::ParsedAtom(ParsedAtom::Small(n)),
            ))
        };
        // ~| with a %tas sand message lowers to a %mean hint around q.
        let sgbr = Hoon::SigBar(
            Box::new(Hoon::Sand(
                "tas".to_string(),
                NounExpr::ParsedAtom(ParsedAtom::Small(0x6f)),
            )),
            sand(5),
        );
        let (p, q) = mull_types(ut, noun.clone(), noun.clone(), &sgbr).unwrap();
        assert_eq!(
            (p.as_str(), q.as_str()),
            ("[%atom [%ud 0]]", "[%atom [%ud 0]]")
        );
        // ~! plays p for the trace and mulls q.
        let sgzp = Hoon::SigZap(sand(1), sand(2));
        let (p, _) = mull_types(ut, noun.clone(), noun.clone(), &sgzp).unwrap();
        assert_eq!(p, "[%atom [%ud 0]]");
        // =- lowers to =+ with its arguments swapped.
        let tshp = Hoon::TisHep(Box::new(parse("-")), sand(3));
        let (p, _) = mull_types(ut, noun.clone(), noun.clone(), &tshp).unwrap();
        assert_eq!(p, "[%atom [%ud 0]]");
        // !; builds [played-p mulled-q].
        let zpmc = Hoon::ZapMic(sand(1), sand(2));
        let (p, q) = mull_types(ut, noun.clone(), noun.clone(), &zpmc).unwrap();
        assert_eq!(p, "[%cell [[%atom [%ud 0]] [%atom [%ud 0]]]]");
        assert_eq!(p, q);
        // !, quotes: both sides play p.
        let zpcm = Hoon::ZapCom(sand(1), sand(2));
        let (p, _) = mull_types(ut, noun.clone(), noun.clone(), &zpcm).unwrap();
        assert_eq!(p, "[%atom [%ud 0]]");
        // ^| ^. ^? wrap or cast the mulled product.
        for (src, want) in [
            ("^|(|.(5))", "[%core "),
            ("^?(|.(5))", "[%core "),
            ("^&(|.(5))", "[%core "),
            ("^.(|=(b=@ b) 5)", "[%hold "),
            (".+(5)", "[%atom "),
            // !< only plays its mold; the vase operand is not mulled.
            ("!<(@ 5)", "[%atom "),
        ] {
            let gen = parse(src);
            let (p, _) = mull_types(ut, noun.clone(), noun.clone(), &gen)
                .unwrap_or_else(|err| panic!("{src}: {err:?}"));
            assert!(p.starts_with(want), "{src}: {p}");
        }
    });
}

#[test]
fn c3_mull_lost_and_vet_off() {
    with_ut(|ut| {
        let noun = cons_noun(&mut ut.cx);
        let lost = Hoon::Lost(Box::new(Hoon::Sand(
            "ud".to_string(),
            NounExpr::ParsedAtom(ParsedAtom::Small(1)),
        )));
        assert!(noun_err(mull_types(ut, noun.clone(), noun.clone(), &lost)).contains("mull-skip"));
        // With vet off, %lost is %void and nice/cnts skip their nest checks.
        ut.vet = false;
        let (p, q) = mull_types(ut, noun.clone(), noun.clone(), &lost).unwrap();
        assert_eq!((p.as_str(), q.as_str()), ("%void", "%void"));
        let sut = spec_ty(ut, "[a=@ b=@]");
        let (p, _) = mull_types(ut, sut.clone(), sut, &parse("a")).unwrap();
        assert_eq!(p, ATOM);
        ut.vet = true;
    });
}

#[test]
fn c3_eror_crashes_mint_play_and_mull_with_its_tape() {
    // hoon-138 `open` crashes on %eror with its tape as the trace; nothing
    // mints it as %void
    with_ut(|ut| {
        let noun = cons_noun(&mut ut.cx);
        let eror = Hoon::Eror("duplicate arm: +x".to_string());
        let minted = ut.mint(noun.clone(), noun.clone(), &eror).map(|_| ());
        let played = ut.play(noun.clone(), &eror).map(|_| ());
        let mulled = mull_types(ut, noun.clone(), noun.clone(), &eror);
        for err in [noun_err(minted), noun_err(played), noun_err(mulled)] {
            assert!(err.contains("duplicate arm: +x"), "{err}");
        }
    });
}

#[test]
fn c3_mull_open_rejects_unopenable_gene() {
    with_ut(|ut| {
        let noun = cons_noun(&mut ut.cx);
        let gol = cons_noun(&mut ut.cx);
        let gen = Hoon::Sand("ud".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(1)));
        let err = noun_err(ut.mull_open_then_recurse(noun.clone(), gol, noun, &gen));
        assert!(err.contains("mull-open"), "{err}");
    });
}

#[test]
fn c3_mull_cnts_synthetic_ports() {
    with_ut(|ut| {
        let noun = cons_noun(&mut ut.cx);
        let atom = atom_ty(ut, "ud", None);
        let formula = ut.formula_slot_u64(1);
        let port = |typ: &NRc<NTy>| Port::Synthetic {
            typ: typ.clone(),
            formula,
        };
        let (p, q) = ut
            .mull_cnts_with_ports(
                noun.clone(),
                noun.clone(),
                noun.clone(),
                &port(&atom),
                &port(&atom),
                &[],
            )
            .unwrap();
        assert_eq!(lower(ut, &p), "[%atom [%ud 0]]");
        assert_eq!(lower(ut, &q), "[%atom [%ud 0]]");
        let edit = vec![(
            vec![Limb::Term("x".to_string())],
            Hoon::Sand("ud".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(1))),
        )];
        let err = noun_err(ut.mull_cnts_with_ports(
            noun.clone(),
            noun.clone(),
            noun,
            &port(&atom),
            &port(&atom),
            &edit,
        ));
        assert!(err.contains("mull-bonk-cnts"), "{err}");
    });
}

#[test]
fn c3_mull_endo_axis_mismatches() {
    with_ut(|ut| {
        let noun = cons_noun(&mut ut.cx);
        let ab = spec_ty(ut, "[a=@ b=@]");
        let ba = spec_ty(ut, "[b=@ a=@]");
        let edit = vec![(
            vec![Limb::Term("b".to_string())],
            Hoon::Sand("ud".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(1))),
        )];
        let leg = |t: &NRc<NTy>| Palo {
            vein: Vec::new(),
            opal: Opal::Leg(t.clone()),
        };
        let err = noun_err(ut.mull_endo(
            noun.clone(),
            noun.clone(),
            noun.clone(),
            &leg(&ab),
            &leg(&ba),
            &edit,
        ));
        assert!(err.contains("endo leg axis mismatch"), "{err}");

        let arm = |axis: u32, arms: Vec<(NRc<NTy>, Noun)>| Palo {
            vein: Vec::new(),
            opal: Opal::Arm {
                axis: BigUint::from(axis),
                arms,
            },
        };
        let err = noun_err(ut.mull_endo(
            noun.clone(),
            noun.clone(),
            noun.clone(),
            &arm(2, Vec::new()),
            &arm(3, Vec::new()),
            &[],
        ));
        assert!(err.contains("endo arm axis mismatch"), "{err}");

        let gate_ab = play_ty(ut, "|=([a=@ b=@] a)");
        let gate_ba = play_ty(ut, "|=([b=@ a=@] a)");
        let foot = D(0);
        let err = noun_err(ut.mull_endo(
            noun.clone(),
            noun.clone(),
            noun,
            &arm(2, vec![(gate_ab, foot)]),
            &arm(2, vec![(gate_ba, foot)]),
            &edit,
        ));
        assert!(err.contains("endo arm toss axis mismatch"), "{err}");
    });
}

// ---------------------------------------------------------------------------
// Noun helpers at the end of the file (mod.rs ~11808-14140)
// ---------------------------------------------------------------------------

#[test]
fn c3_parsed_atom_zero_and_flag() {
    let big = ParsedAtom::Big(BigUint::from(1u32) << 80);
    assert!(atom_is_zero(&ParsedAtom::Small(0)));
    assert!(!atom_is_zero(&big));
    assert!(atom_is_flag(&ParsedAtom::Big(BigUint::from(1u32))));
    assert!(!atom_is_flag(&ParsedAtom::Big(BigUint::from(2u32))));
    assert!(!atom_is_flag(&big));
}

#[test]
fn c3_slot_formula_axis_builders() {
    let mut slab = NounSlab::new();
    let f = slot_formula_axis_big(&mut slab, BigUint::from(1u32) << 70);
    assert_eq!(show(&slab, f), "[0 BIG]");
    let seven = D(7);
    let f = slot_formula_axis_noun(&mut slab, seven);
    assert_eq!(show(&slab, f), "[0 7]");
}

#[test]
fn c3_dor_gor_mor_order_nouns() {
    let mut slab = NounSlab::new();
    let a = T(&mut slab, &[D(1), D(2)]);
    let a2 = T(&mut slab, &[D(1), D(2)]);
    let b = T(&mut slab, &[D(1), D(3)]);
    let c = T(&mut slab, &[D(2), D(0)]);
    // Identical and structurally equal nouns are ordered either way.
    assert!(dor(&mut slab, a, a));
    assert!(dor(&mut slab, a, a2));
    // Atoms sort before cells and by value.
    assert!(dor(&mut slab, D(3), D(5)));
    assert!(!dor(&mut slab, D(5), D(3)));
    assert!(dor(&mut slab, D(5), a));
    assert!(!dor(&mut slab, a, D(5)));
    // Cells compare heads, then tails when the heads are equal.
    assert!(dor(&mut slab, a, b));
    assert!(!dor(&mut slab, b, a));
    assert!(dor(&mut slab, a, c));
    assert!(!dor(&mut slab, c, a));
    // Structurally equal (but separately allocated) cell heads compare tails.
    let h1 = T(&mut slab, &[D(7), D(8)]);
    let h2 = T(&mut slab, &[D(7), D(8)]);
    let p = T(&mut slab, &[h1, D(1)]);
    let q = T(&mut slab, &[h2, D(2)]);
    assert!(dor(&mut slab, p, q));
    assert!(!dor(&mut slab, q, p));
    // Equal mugs fall back to dor.
    assert!(gor_mug(&mut slab, a, a2));
    assert!(mor_mug(&mut slab, a, a2));
    let lt = gor_mug(&mut slab, D(1), D(2));
    assert_ne!(lt, gor_mug(&mut slab, D(2), D(1)));
}

#[test]
fn c3_set_uni_and_map_put_treaps() {
    let mut slab = NounSlab::new();
    let mut a = D(0);
    let mut b = D(0);
    for n in 1..=6u64 {
        a = set_put_mug(&mut slab, a, D(n)).unwrap();
    }
    for n in 4..=9u64 {
        b = set_put_mug(&mut slab, b, D(n)).unwrap();
    }
    let u = set_uni_mug(&mut slab, a, b).unwrap();
    let space = slab.noun_space();
    let mut members: Vec<u64> = fork_set_options(u, &space)
        .unwrap()
        .into_iter()
        .map(|n| n.in_space(&space).as_atom().unwrap().as_u64().unwrap())
        .collect();
    members.sort_unstable();
    assert_eq!(members, (1..=9).collect::<Vec<_>>());
    // Union with itself or an equal set is the identity.
    let again = set_uni_mug(&mut slab, u, u).unwrap();
    assert!(noun_eq(again, u, &slab.noun_space()).unwrap());
    let u2 = set_uni_mug(&mut slab, b, a).unwrap();
    let space = slab.noun_space();
    let mut members2: Vec<u64> = fork_set_options(u2, &space)
        .unwrap()
        .into_iter()
        .map(|n| n.in_space(&space).as_atom().unwrap().as_u64().unwrap())
        .collect();
    members2.sort_unstable();
    assert_eq!(members, members2);

    // map_put_mug: re-putting a key replaces its value or keeps the tree.
    let mut m = D(0);
    for n in 1..=5u64 {
        m = map_put_mug(&mut slab, m, D(n), D(n * 10)).unwrap();
    }
    let same = map_put_mug(&mut slab, m, D(3), D(30)).unwrap();
    assert!(noun_eq(same, m, &slab.noun_space()).unwrap());
    let changed = map_put_mug(&mut slab, m, D(3), D(31)).unwrap();
    assert!(!noun_eq(changed, m, &slab.noun_space()).unwrap());
    // Re-putting the root key with its value returns the tree itself.
    let root_key = {
        let space = slab.noun_space();
        let root = m
            .in_space(&space)
            .as_cell()
            .unwrap()
            .head()
            .as_cell()
            .unwrap();
        root.head().noun()
    };
    let root_val = {
        let space = slab.noun_space();
        let root = m
            .in_space(&space)
            .as_cell()
            .unwrap()
            .head()
            .as_cell()
            .unwrap();
        root.tail().noun()
    };
    let same = map_put_mug(&mut slab, m, root_key, root_val).unwrap();
    assert!(unsafe { same.raw_equals(&m) });
    let changed = map_put_mug(&mut slab, m, root_key, D(99)).unwrap();
    assert!(!noun_eq(changed, m, &slab.noun_space()).unwrap());
    // A malformed tree is a decode error.
    let bad = T(&mut slab, &[D(1), D(2)]);
    assert!(map_put_mug(&mut slab, bad, D(9), D(9)).is_err());
}

#[test]
fn c3_type_tag_helpers() {
    let mut slab = NounSlab::new();
    let space_kinds = [("void", TypeTagKind::Void), ("noun", TypeTagKind::Noun)];
    for (tag, kind) in space_kinds {
        let n = term_to_noun(&mut slab, tag);
        assert_eq!(type_tag_kind(n, &slab.noun_space()).unwrap(), kind);
    }
    let atom = ty_atom(&mut slab, "ud", None);
    let fork = ty_fork(&mut slab, vec![atom, D(0)]);
    assert_eq!(
        type_tag_kind(fork, &slab.noun_space()).unwrap(),
        TypeTagKind::Fork
    );
    let hold = ty_hold(&mut slab, atom, D(0));
    assert_eq!(
        type_tag_kind(hold, &slab.noun_space()).unwrap(),
        TypeTagKind::Hold
    );
    let zzz = term_to_noun(&mut slab, "zzz");
    let bogus = T(&mut slab, &[zzz, D(0)]);
    assert!(type_tag_kind(bogus, &slab.noun_space()).is_err());
    // Hint/hold accessors.
    let note = D(0);
    let hint = ty_hint(&mut slab, atom, note, atom);
    let inner = type_hint_inner(hint, &slab.noun_space()).unwrap();
    assert!(noun_eq(inner, atom, &slab.noun_space()).unwrap());
    let held = type_hold_type(hold, &slab.noun_space()).unwrap();
    assert!(noun_eq(held, atom, &slab.noun_space()).unwrap());
    // Face names: atom tools give the name, cell tools give none.
    let face = ty_face(&mut slab, "x", atom);
    assert_eq!(
        type_face_name_if_atom(face, &slab.noun_space())
            .unwrap()
            .as_deref(),
        Some("x")
    );
    let tune = T(&mut slab, &[D(0), D(0)]);
    let tuned = ty_face_tool(&mut slab, tune, atom);
    assert_eq!(
        type_face_name_if_atom(tuned, &slab.noun_space()).unwrap(),
        None
    );
    // Fork sets: an empty set has no options; a malformed node is rejected.
    assert!(fork_set_options(D(0), &slab.noun_space())
        .unwrap()
        .is_empty());
    let bad_set = T(&mut slab, &[D(1), D(2)]);
    assert!(fork_set_options(bad_set, &slab.noun_space()).is_err());
    assert!(type_fork_options(D(3), &slab.noun_space()).is_err());
    // Collapses in the noun constructors.
    let void = ty_void(&mut slab);
    let t = ty_face_tool(&mut slab, D(0x78), void);
    assert_eq!(show(&slab, t), "%void");
    let t = ty_core(&mut slab, void, D(0));
    assert_eq!(show(&slab, t), "%void");
    let noun = ty_noun(&mut slab);
    let t = ty_hint(&mut slab, atom, note, noun);
    assert_eq!(show(&slab, t), "%noun");
}

#[test]
fn c3_find_face_axis_skip_walks_every_type_kind() {
    let mut slab = NounSlab::new();
    let atom = ty_atom(&mut slab, "ud", None);
    let x = ty_face(&mut slab, "x", atom);
    let y = ty_face(&mut slab, "y", atom);
    let xx = ty_face(&mut slab, "x", x);
    let xy = ty_cell(&mut slab, x, y);
    let yx = ty_cell(&mut slab, y, x);
    let xa = ty_cell(&mut slab, x, atom);
    let hint = ty_hint(&mut slab, atom, D(0), xy);
    let hold = ty_hold(&mut slab, xy, D(0));
    let fork_same = ty_fork(&mut slab, vec![xy, xa]);
    let fork_diff = ty_fork(&mut slab, vec![xy, yx]);
    let dry = term_to_noun(&mut slab, "dry");
    let gold = term_to_noun(&mut slab, "gold");
    let garb = T(&mut slab, &[D(0), dry, gold]);
    let coil = coil_from_parts(&mut slab, garb, y, D(0));
    let core = ty_core(&mut slab, x, coil);
    let space = slab.noun_space();
    let find = |noun: Noun, name: &str, skip: u64| {
        find_face_axis_skip(noun, name, skip, &space)
            .unwrap()
            .map(|axis| axis.to_string())
    };
    // Direct faces, skipped faces, and a shadowed face found after a skip.
    assert_eq!(find(x, "x", 0).as_deref(), Some("1"));
    assert_eq!(find(x, "x", 1), None);
    assert_eq!(find(xx, "x", 1).as_deref(), Some("1"));
    // Cells search head, then tail.
    assert_eq!(find(xy, "x", 0).as_deref(), Some("2"));
    assert_eq!(find(yx, "x", 0).as_deref(), Some("3"));
    assert_eq!(find(xy, "z", 0), None);
    // Hints and holds are transparent.
    assert_eq!(find(hint, "y", 0).as_deref(), Some("3"));
    assert_eq!(find(hold, "y", 0).as_deref(), Some("3"));
    // Fork options must agree on the axis.
    assert_eq!(find(fork_same, "x", 0).as_deref(), Some("2"));
    assert_eq!(find(fork_diff, "x", 0), None);
    assert_eq!(find(fork_diff, "z", 0), None);
    // Cores search the payload (axis 3), then the context (axis 7).
    assert_eq!(find(core, "x", 0).as_deref(), Some("3"));
    assert_eq!(find(core, "y", 0).as_deref(), Some("7"));
    assert_eq!(find(core, "z", 0), None);
    assert_eq!(find(atom, "x", 0), None);
}

#[test]
fn c3_axis_helpers_reject_degenerate_axes() {
    assert!(axis_cap_mas(1).is_err());
    assert!(axis_cap_mas(0).is_err());
    assert_eq!(axis_cap_mas(6).unwrap(), (3, 2));
    assert!(axis_big_cap_mas(&BigUint::from(1u32)).is_err());
    assert_eq!(
        axis_big_cap_mas(&BigUint::from(5u32)).unwrap(),
        (2, BigUint::from(3u32))
    );
    assert!(peg_axis_big_pair(BigUint::from(2u32), &BigUint::from(0u32)).is_err());
    assert_eq!(
        peg_axis_big_pair(BigUint::from(2u32), &BigUint::from(3u32)).unwrap(),
        BigUint::from(5u32)
    );
}

#[test]
fn c3_native_fork_option_iter_is_exact_size() {
    with_ut(|ut| {
        // A fresh fork's first option request is transient; later ones are cached.
        let a = ty_atom(ut.slab, "tas", Some(D(0x61)));
        let b = ty_atom(ut.slab, "tas", Some(D(0x62)));
        let c = ty_atom(ut.slab, "tas", Some(D(0x63)));
        let fork = ty_fork(ut.slab, vec![a, b, c]);
        let fork = native_of(&mut ut.cx, fork, &ut.slab.noun_space()).unwrap();
        for _ in 0..2 {
            let options = ut.fork_options_native(&fork).unwrap();
            let iter = options.into_iter();
            assert_eq!(iter.size_hint(), (3, Some(3)));
            assert_eq!(iter.len(), 3);
        }
    });
}

#[test]
fn c3_foot_parts_rejects_unknown_poly() {
    let mut slab: NounSlab = NounSlab::new();
    let wet = term_to_noun(&mut slab, "wet");
    let foot = T(&mut slab, &[wet, D(0)]);
    assert!(matches!(
        foot_parts(foot, &slab.noun_space()).unwrap().0,
        Poly::Wet
    ));
    let odd = term_to_noun(&mut slab, "odd");
    let foot = T(&mut slab, &[odd, D(0)]);
    assert!(foot_parts(foot, &slab.noun_space()).is_err());
}

#[test]
fn c3_chunked_tisgar_chain_composes_layers() {
    // The top-level parser wraps a file in a one-element `=~`; take its body.
    let chain = match parse("=>  [a=1 b=2]  =>  [c=a d=b]  [d c]") {
        Hoon::TisSig(mut items) if items.len() == 1 => items.remove(0),
        other => other,
    };
    assert!(matches!(chain, Hoon::TisGar(..)), "{chain:?}");
    let mono = mint_src("=>  [a=1 b=2]  =>  [c=a d=b]  [d c]").unwrap();
    let mut out = NounSlab::new();
    let sut = ty_noun(&mut out);
    let gol = ty_noun(&mut out);
    let (ty, formula) = mint_tisgar_chain_chunked(&mut out, sut, gol, &chain).unwrap();
    assert_eq!(show(&out, ty), mono.0);
    // The chunked formula composes the per-layer formulas with `comb`.
    assert!(show(&out, formula).starts_with('['));
}

fn sample_spec_zoo() -> Spec {
    let atom = || Box::new(Spec::Base(BaseType::Atom("ud".to_string())));
    let wing = || {
        vec![
            Limb::Term("a".to_string()),
            Limb::Axis((2u64).into()),
            Limb::Parent(1, Some("b".to_string())),
            Limb::Parent(0, None),
        ]
    };
    let hoon = || Hoon::Sand("ud".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(1)));
    let mut map = HashMap::new();
    map.insert("k".to_string(), Spec::Base(BaseType::Cell));
    let list = vec![
        Spec::Base(BaseType::Flag),
        Spec::Base(BaseType::Null),
        Spec::Base(BaseType::Void),
        Spec::Base(BaseType::NounExpr),
        Spec::Dbug(spot(), atom()),
        Spec::Gist(NounExpr::ParsedAtom(ParsedAtom::Small(1)), atom()),
        Spec::Leaf("tas".to_string(), ParsedAtom::Small(0x61)),
        Spec::Like(wing(), vec![wing()]),
        Spec::Loop("l".to_string()),
        Spec::Made(("m".to_string(), vec!["a".to_string()]), atom()),
        Spec::Make(hoon(), vec![Spec::Base(BaseType::NounExpr)]),
        Spec::Name("n".to_string(), atom()),
        Spec::Over(wing(), atom()),
        Spec::BucGar(atom(), atom()),
        Spec::BucBuc(atom(), map.clone()),
        Spec::BucBar(atom(), hoon()),
        Spec::BucCab(hoon()),
        Spec::BucCen(atom(), vec![Spec::Base(BaseType::NounExpr)]),
        Spec::BucDot(atom(), map.clone()),
        Spec::BucGal(atom(), atom()),
        Spec::BucHep(atom(), atom()),
        Spec::BucKet(atom(), atom()),
        Spec::BucLus("t".to_string(), atom()),
        Spec::BucFas(atom(), map.clone()),
        Spec::BucMic(hoon()),
        Spec::BucPam(atom(), hoon()),
        Spec::BucSig(hoon(), atom()),
        Spec::BucTic(atom(), map.clone()),
        Spec::BucPat(atom(), atom()),
        Spec::BucWut(atom(), vec![Spec::Base(BaseType::NounExpr)]),
        Spec::BucZap(atom(), map),
    ];
    let skins = vec![
        Skin::Term("s".to_string()),
        Skin::Base(BaseType::NounExpr),
        Skin::Cell(
            Box::new(Skin::Wash(1)),
            Box::new(Skin::Base(BaseType::Cell)),
        ),
        Skin::Dbug(spot(), Box::new(Skin::Wash(0))),
        Skin::Help(
            NounExpr::ParsedAtom(ParsedAtom::Small(2)),
            Box::new(Skin::Wash(0)),
        ),
        Skin::Leaf("ud".to_string(), ParsedAtom::Small(3)),
        Skin::Name("x".to_string(), Box::new(Skin::Wash(0))),
        Skin::Over(wing(), Box::new(Skin::Wash(0))),
        Skin::Spec(atom(), Box::new(Skin::Wash(0))),
    ];
    let mut all = list;
    for skin in skins {
        all.push(Spec::BucTis(skin, atom()));
    }
    Spec::BucCol(atom(), all)
}

fn type_zoo() -> Type {
    let atom = || Box::new(Type::ParsedAtom("ud".to_string(), Some(5)));
    let hoon = Hoon::Sand("ud".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(1)));
    let mut arms = HashMap::new();
    arms.insert("arm".to_string(), hoon.clone());
    let mut tomes = HashMap::new();
    tomes.insert(
        "a".to_string(),
        (Some(NounExpr::ParsedAtom(ParsedAtom::Small(9))), arms),
    );
    tomes.insert("ch".to_string(), (None, HashMap::new()));
    let stencil = Stencil::Half {
        left: Box::new(Stencil::Full {
            blocks: vec![vec![vec!["knot".to_string()]]],
        }),
        rite: Box::new(Stencil::Lazy {
            fragment: (3u64).into(),
            resolve: (
                Box::new(sample_spec_zoo()),
                Box::new(Spec::Base(BaseType::NounExpr)),
            ),
        }),
    };
    let coil = |vair: AstVair, poly: AstPoly, name: Option<&str>| Coil {
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
    let mut tune_map = HashMap::new();
    tune_map.insert("t".to_string(), Some(hoon.clone()));
    tune_map.insert("u".to_string(), None);
    Type::Fork(vec![
        Type::NounExpr,
        Type::Void,
        Type::ParsedAtom("".to_string(), None),
        Type::Cell(atom(), atom()),
        Type::Core(
            atom(),
            Box::new(coil(AstVair::Gold, AstPoly::Wet, Some("c"))),
        ),
        Type::Core(atom(), Box::new(coil(AstVair::Iron, AstPoly::Dry, None))),
        Type::Core(atom(), Box::new(coil(AstVair::Lead, AstPoly::Dry, None))),
        Type::Core(atom(), Box::new(coil(AstVair::Zinc, AstPoly::Dry, None))),
        Type::Face(FaceType::Term("f".to_string()), atom()),
        Type::Face(FaceType::Tune((tune_map, vec![hoon.clone()])), atom()),
        Type::Hint(
            (
                atom(),
                Note::Help(NounExpr::ParsedAtom(ParsedAtom::Small(1))),
            ),
            atom(),
        ),
        Type::Hint((atom(), Note::Know("k".to_string())), atom()),
        Type::Hint((atom(), Note::Made("m".to_string(), None)), atom()),
        Type::Hint(
            (
                atom(),
                Note::Made(
                    "m".to_string(),
                    Some(vec![vec![Limb::Term("a".to_string())]]),
                ),
            ),
            atom(),
        ),
        Type::Hold(atom(), hoon),
    ])
}

#[test]
fn c3_type_to_noun_lowers_every_ast_form() {
    let mut slab = NounSlab::new();
    let noun = type_to_noun(&mut slab, &type_zoo()).unwrap();
    let text = show(&slab, noun);
    for needle in [
        "%fork", "%noun", "%void", "%cell", "%core", "%face", "%tune", "%hint", "%hold", "%help",
        "%know", "%made", "%half", "%full", "%lazy", "%gold", "%iron", "%lead", "%zinc", "%wet",
        "%dry", "%bccl", "%bcts", "%bcwt", "%bczp", "%base", "%flag", "%null", "%dbug", "%gist",
        "%leaf", "%like", "%loop", "%make", "%name", "%over", "%bcgr", "%bcbc", "%bcbr", "%bccb",
        "%bccn", "%bcdt", "%bcgl", "%bchp", "%bckt", "%bcls", "%bcfs", "%bcmc", "%bcpm", "%bcsg",
        "%bctc", "%bcpt", "%wash", "%spec",
    ] {
        assert!(text.contains(needle), "missing {needle}");
    }
}

#[test]
fn c3_hand_genes_through_mint_play_and_mull() {
    with_ut(|ut| {
        let nock = Nock::Pair(
            Box::new(Nock::Const(NounExpr::ParsedAtom(ParsedAtom::Small(5)))),
            Box::new(Nock::AxisSelect((1u64).into())),
        );
        let hand = Hoon::Hand(Box::new(Type::ParsedAtom("ud".to_string(), None)), nock);
        let noun = cons_noun(&mut ut.cx);
        let (p, q) = mull_types(ut, noun.clone(), noun.clone(), &hand).unwrap();
        assert_eq!(
            (p.as_str(), q.as_str()),
            ("[%atom [%ud 0]]", "[%atom [%ud 0]]")
        );
        let played = ut.play(noun.clone(), &hand).unwrap();
        assert_eq!(lower(ut, &played), "[%atom [%ud 0]]");
        let gol = cons_noun(&mut ut.cx);
        let (_, formula) = ut.mint(noun, gol, &hand).unwrap();
        let formula = ut.formula_materialize(formula);
        // hatch's `Nock::Pair` is Nock 2 (evaluate), not autocons.
        assert_eq!(show(ut.slab, formula), "[2 [[1 5] 1]]");
    });
}

#[test]
fn c3_nock_to_noun_lowers_every_opcode() {
    let mut slab = NounSlab::new();
    let c = |n: u128| Box::new(Nock::Const(NounExpr::ParsedAtom(ParsedAtom::Small(n))));
    let ax = || Box::new(Nock::AxisSelect((2u64).into()));
    let cases: Vec<(Nock, &str)> = vec![
        (Nock::Pair(c(1), ax()), "[2 [[1 1] 2]]"),
        (Nock::Compose(ax(), c(1)), "[7 [2 [1 1]]]"),
        (Nock::CellTest(ax()), "[3 2]"),
        (Nock::Increment(ax()), "[4 2]"),
        (Nock::Equality(ax(), c(0)), "[5 [2 [1 0]]]"),
        (Nock::IfThenElse(ax(), c(0), c(1)), "[6 [2 [[1 0] [1 1]]]]"),
        (Nock::SerialCompose(ax(), c(1)), "[8 [2 [1 1]]]"),
        (Nock::PushSubject(ax(), c(1)), "[9 [2 [1 1]]]"),
        (Nock::SelectArm((2u64).into(), ax()), "[10 [2 2]]"),
        (
            Nock::Edit(((2u64).into(), c(1)), ax()),
            "[11 [[2 [1 1]] 2]]",
        ),
        (Nock::Hint(NockHint::ParsedAtom(7), ax()), "[12 [7 2]]"),
        (
            Nock::Hint(NockHint::Pair(7, c(1)), ax()),
            "[12 [[7 [1 1]] 2]]",
        ),
        (Nock::GrabData(ax(), c(1)), "[13 [2 [1 1]]]"),
    ];
    for (nock, want) in cases {
        let n = nock_to_noun(&mut slab, &nock);
        assert_eq!(show(&slab, n), want, "{nock:?}");
    }
}

#[test]
fn c3_note_to_noun_made_wings_and_tunes() {
    let mut slab = NounSlab::new();
    let note = Note::Made(
        "m".to_string(),
        Some(vec![
            vec![Limb::Term("a".to_string())],
            vec![Limb::Axis((3u64).into())],
        ]),
    );
    let n = note_to_noun(&mut slab, &note).unwrap();
    assert_eq!(show(&slab, n), "[%made [%m [0 [[%a 0] [[[0 3] 0] 0]]]]]");
    let mut map = HashMap::new();
    map.insert("x".to_string(), None);
    let tune: Tune = (map, Vec::new());
    let n = tune_to_noun(&mut slab, &tune).unwrap();
    assert_eq!(show(&slab, n), "[[[%x 0] [0 0]] 0]");
}

#[test]
fn c3_const_bool_formula_and_cell_type() {
    let mut slab = NounSlab::new();
    let yes = T(&mut slab, &[D(1), D(0)]);
    let no = T(&mut slab, &[D(1), D(1)]);
    let slot = T(&mut slab, &[D(0), D(1)]);
    let space = slab.noun_space();
    assert!(is_const_bool_formula(yes, true, &space));
    assert!(is_const_bool_formula(no, false, &space));
    assert!(!is_const_bool_formula(yes, false, &space));
    assert!(!is_const_bool_formula(slot, true, &space));
    assert!(!is_const_bool_formula(D(3), true, &space));

    let atom = ty_atom(&mut slab, "ud", None);
    let void = ty_void(&mut slab);
    let c = cell_type(&mut slab, atom, atom).unwrap();
    assert_eq!(show(&slab, c), "[%cell [[%atom [%ud 0]] [%atom [%ud 0]]]]");
    let c = cell_type(&mut slab, void, atom).unwrap();
    assert_eq!(show(&slab, c), "%void");
    let c = cell_type(&mut slab, atom, void).unwrap();
    assert_eq!(show(&slab, c), "%void");

    let mut cx = Context::new();
    let space = slab.noun_space();
    let an = native_of(&mut cx, atom, &space).unwrap();
    let vn = native_of(&mut cx, void, &space).unwrap();
    let (n, _) = cell_type_n(&mut cx, &mut slab, (atom, an.clone()), (atom, an.clone())).unwrap();
    assert_eq!(show(&slab, n), "[%cell [[%atom [%ud 0]] [%atom [%ud 0]]]]");
    let (n, _) = cell_type_n(&mut cx, &mut slab, (void, vn.clone()), (atom, an.clone())).unwrap();
    assert_eq!(show(&slab, n), "%void");
    let (n, _) = cell_type_n(&mut cx, &mut slab, (atom, an), (void, vn)).unwrap();
    assert_eq!(show(&slab, n), "%void");
}
