//! Coverage-driven tests for token, doc, and spot handling.
//!
//! Added to close branch-coverage gaps; see the coverage report in the PR.

use std::sync::Arc;

use chumsky::prelude::*;
use num_bigint::BigUint;
use num_traits::{ToPrimitive, Zero};

use crate::ast::hoon::*;
#[allow(unused_imports)]
use crate::utils::*;

// ---------------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------------

/// `@s` encoding of a signed integer (even = nonnegative, odd = negative).
fn si(n: i128) -> u128 {
    if n >= 0 {
        (n as u128) * 2
    } else {
        ((-n) as u128) * 2 - 1
    }
}

fn big(n: u128) -> BigUint {
    BigUint::from(n)
}

fn atom_u128(a: &ParsedAtom) -> u128 {
    a.to_biguint().to_u128().expect("atom should fit in u128")
}

/// Text of a little-endian cord atom.
fn cord_text(a: &ParsedAtom) -> String {
    let bytes: Vec<u8> = if a.is_zero() {
        vec![]
    } else {
        a.to_biguint().to_bytes_le()
    };
    String::from_utf8(bytes).expect("cord should be UTF-8")
}

fn cord_atom(s: &str) -> BigUint {
    BigUint::from_bytes_le(s.as_bytes())
}

#[derive(Debug, PartialEq)]
enum F {
    Fin(bool, u128, BigUint),
    Inf(bool),
    Nan,
}

fn bf(x: &BinaryFloat) -> F {
    match x {
        BinaryFloat::Finite { sign, exp, mant } => F::Fin(*sign, *exp, mant.clone()),
        BinaryFloat::Infinity { sign } => F::Inf(*sign),
        BinaryFloat::NaN => F::Nan,
    }
}

fn df(x: &DecimalFloat) -> F {
    match x {
        DecimalFloat::Finite { sign, exp, mant } => F::Fin(*sign, *exp, mant.clone()),
        DecimalFloat::Infinity { sign } => F::Inf(*sign),
        DecimalFloat::NaN => F::Nan,
    }
}

fn fin(sign: bool, exp: i128, mant: u128) -> BinaryFloat {
    BinaryFloat::Finite {
        sign,
        exp: si(exp),
        mant: big(mant),
    }
}

fn ffin(sign: bool, exp: i128, mant: u128) -> F {
    F::Fin(sign, si(exp), big(mant))
}

/// Parse a whole Hoon source with docs off and unwrap the one-expression file.
fn parse_expr(src: &str) -> Hoon {
    let linemap = Arc::new(LineMap::new(src));
    let parsed = crate::native_parser(vec!["test".into(), "p2.hoon".into()], false, linemap)
        .parse(src)
        .into_result()
        .unwrap_or_else(|e| panic!("{src:?} should parse: {e:?}"));
    match parsed {
        Hoon::TisSig(mut items) if items.len() == 1 => items.remove(0),
        other => other,
    }
}

fn parse_fails(src: &str) -> bool {
    let linemap = Arc::new(LineMap::new(src));
    crate::native_parser(vec!["test".into(), "p2.hoon".into()], false, linemap)
        .parse(src)
        .into_result()
        .is_err()
}

fn sand(aura: &str, n: u128) -> Hoon {
    Hoon::Sand(aura.to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(n)))
}

fn sand_atom(h: &Hoon) -> (String, BigUint) {
    match h {
        Hoon::Sand(aura, NounExpr::ParsedAtom(a)) | Hoon::Rock(aura, NounExpr::ParsedAtom(a)) => {
            (aura.clone(), a.to_biguint())
        }
        other => panic!("expected an atom literal, got {other:?}"),
    }
}

/// `wing()` boxed so it satisfies `ParserExt` (which needs `Clone`).
fn wing_p<'src>() -> Boxed<'src, 'src, &'src str, Hoon, Err<'src>> {
    wing().boxed()
}

fn term(s: &str) -> Limb {
    Limb::Term(s.to_string())
}

fn ax(n: u64) -> Limb {
    Limb::Axis(Axis::from(n))
}

// ---------------------------------------------------------------------------
// @r literals: grd_fl / lug / bif, and knot rendering via sea / drg
// ---------------------------------------------------------------------------

/// Parse a float literal (with its leading `.`) to (aura, bits).
fn float_bits(lit: &str) -> (String, u128) {
    let body = lit
        .strip_prefix('.')
        .expect("float literal starts with '.'");
    let (aura, atom) = float()
        .parse(body)
        .into_result()
        .unwrap_or_else(|e| panic!("{lit} should parse as a float: {e:?}"));
    (aura, atom_u128(&atom))
}

// Round-to-nearest-even IEEE encodings (the same literals are parity-checked
// against hoonc by coverage/p2/p2_floats_{rs,wide}.hoon).
const FLOAT_LITERALS: &[(&str, &str, u128)] = &[
    (".1.5", "rs", 0x3fc00000),
    (".-1.5", "rs", 0xbfc00000),
    (".-1", "rs", 0xbf800000),
    (".-1e3", "rs", 0xc47a0000),
    (".1e-3", "rs", 0x3a83126f),
    (".-2e5", "rs", 0xc8435000),
    (".0", "rs", 0x0),
    (".-0", "rs", 0x80000000),
    (".0.0", "rs", 0x0),
    (".-0.0", "rs", 0x80000000),
    (".inf", "rs", 0x7f800000),
    (".-inf", "rs", 0xff800000),
    (".nan", "rs", 0x7fc00000),
    (".-3.4e38", "rs", 0xff7fc99e),
    (".-3.5e38", "rs", 0xff800000),
    (".-1e39", "rs", 0xff800000),
    (".3.4e-38", "rs", 0x1391d15),
    (".1e-38", "rs", 0x6ce3ee),
    (".1e-40", "rs", 0x116c2),
    (".1e-45", "rs", 0x1),
    (".7e-46", "rs", 0x0),
    (".1e-50", "rs", 0x0),
    (".-1e-50", "rs", 0x80000000),
    (".1.4e-45", "rs", 0x1),
    (".2.1e-45", "rs", 0x1),
    (".-16777217", "rs", 0xcb800000),
    (".-16777219", "rs", 0xcb800002),
    (".0.1", "rs", 0x3dcccccd),
    (".1.25e-1", "rs", 0x3e000000),
    (".2.5", "rs", 0x40200000),
    (".-7", "rs", 0xc0e00000),
    (".0.3", "rs", 0x3e99999a),
    (".9.999999e-1", "rs", 0x3f7ffffe),
    (".1.00000005", "rs", 0x3f800000),
    (".1.00000006", "rs", 0x3f800001),
    (".1.0000001", "rs", 0x3f800001),
    (".3.14159265358979", "rs", 0x40490fdb),
    (".~1.5", "rd", 0x3ff8000000000000),
    (".~-1.5", "rd", 0xbff8000000000000),
    (".~-1", "rd", 0xbff0000000000000),
    (".~-1e20", "rd", 0xc415af1d78b58c40),
    (".~1e-310", "rd", 0x12688b70e62b),
    (".~1e-330", "rd", 0x0),
    (".~inf", "rd", 0x7ff0000000000000),
    (".~-inf", "rd", 0xfff0000000000000),
    (".~nan", "rd", 0x7ff8000000000000),
    (".~0.1", "rd", 0x3fb999999999999a),
    (".~2.2250738585072014e-308", "rd", 0x10000000000000),
    (".~4.9e-324", "rd", 0x1),
    (".~2.4e-324", "rd", 0x0),
    (".~1.7976931348623157e-100", "rd", 0x2b392a32afcc661e),
    (".~~1.5", "rh", 0x3e00),
    (".~~-2", "rh", 0xc000),
    (".~~-65504", "rh", 0xfbff),
    (".~~-65520", "rh", 0xfc00),
    (".~~-1e5", "rh", 0xfc00),
    (".~~1e-5", "rh", 0xa8),
    (".~~1e-8", "rh", 0x0),
    (".~~-inf", "rh", 0xfc00),
    (".~~0.1", "rh", 0x2e66),
    (".~~6e-8", "rh", 0x1),
    (".~~5.9e-8", "rh", 0x1),
    (".~~3e-8", "rh", 0x1),
    (".~~nan", "rh", 0x7e00),
    (".~~-0", "rh", 0x8000),
    (".~~~1.5", "rq", 0x3fff8000000000000000000000000000),
    (".~~~-1e10", "rq", 0xc0202a05f20000000000000000000000),
    (".~~~-1e30", "rq", 0xc06293e5939a08ce9dbd480000000000),
    (".~~~1e-4940", "rq", 0xcc64f1cc4376f7da08f39),
    (".~~~nan", "rq", 0x7fff8000000000000000000000000000),
    (".~~~0", "rq", 0x0),
    (".~~~0.1", "rq", 0x3ffb999999999999999999999999999a),
    (".~~~-inf", "rq", 0xffff0000000000000000000000000000),
];

#[test]
fn float_literals_round_to_nearest_even_in_every_width() {
    for (lit, aura, bits) in FLOAT_LITERALS {
        let (got_aura, got_bits) = float_bits(lit);
        assert_eq!(got_aura, *aura, "{lit} aura");
        assert_eq!(
            got_bits, *bits,
            "{lit} should encode as {bits:#x}, got {got_bits:#x}"
        );
    }
}

#[test]
fn float_literal_through_whole_parser_is_a_sand() {
    assert_eq!(
        parse_expr(".-1e3"),
        sand("rs", 0xc47a0000),
        "a float literal jocks to a %sand of its @rs bits"
    );
    assert_eq!(parse_expr(".~~-inf"), sand("rh", 0xfc00));
}

#[test]
fn float_rly_decodes_each_ieee_class() {
    // 1.5 = 15 x 10^-1 in every width
    for df_val in [
        rlys(0x3fc00000),
        rlyd(0x3ff8000000000000),
        rlyh(0x3e00),
        rlyq(0x3fff8000000000000000000000000000),
    ] {
        assert_eq!(df(&df_val), ffin(true, -1, 15));
    }
    assert_eq!(df(&rlys(0)), ffin(true, 0, 0), "+0 renders as a zero dn");
    assert_eq!(
        df(&rlys(0x80000000)),
        ffin(false, 0, 0),
        "-0 keeps its sign"
    );
    assert_eq!(df(&rlys(0x7f800000)), F::Inf(true));
    assert_eq!(df(&rlys(0xff800000)), F::Inf(false));
    assert_eq!(df(&rlys(0x7fc00000)), F::Nan);
    assert_eq!(df(&rlyh(0x7e00)), F::Nan);
    // 0.1 as the shortest round-tripping digits
    assert_eq!(df(&rlys(0x3dcccccd)), ffin(true, -1, 1));
    assert_eq!(df(&rlyd(0x3fb999999999999a)), ffin(true, -1, 1));
    // a power-of-two mantissa takes the halfway (tightened lower bound) path
    assert_eq!(df(&rlys(0x3f000000)), ffin(true, -1, 5));
    assert_eq!(df(&rlys(0xc0000000)), ffin(false, 0, 2));
}

#[test]
fn float_rendering_round_trips_large_and_subnormal_values() {
    // Positive binary exponents and subnormals: the digits must parse back to
    // the same bits (negative values keep grd on its correct sign path).
    for bits in [0xcb800002u128, 0xf149f2ca, 0xff7fffff, 0x116c2, 0x1, 0x80000001] {
        let dn = rlys(bits);
        assert_eq!(atom_u128(&ryls(dn)), bits, "{bits:#x} should round-trip");
    }
    for bits in [0xc415af1d78b58c40u128, 0x12688b70e62b, 0x1] {
        let dn = rlyd(bits);
        assert_eq!(atom_u128(&ryld(dn)), bits, "{bits:#x} should round-trip");
    }
    for bits in [0xfbffu128, 0x1, 0xa8] {
        let dn = rlyh(bits);
        assert_eq!(atom_u128(&rylh(dn)), bits, "{bits:#x} should round-trip");
    }
    for bits in [0xc06293e5939a08ce9dbd480000000000u128, 0xcc64f1cc4376f7da08f39] {
        let dn = rlyq(bits);
        assert_eq!(atom_u128(&rylq(dn)), bits, "{bits:#x} should round-trip");
    }
}

#[test]
fn float_knots_render_as_ta_text() {
    let cases: &[(&str, u128, &str)] = &[
        ("rs", 0x3fc00000, ".1.5"),
        ("rs", 0xc0200000, ".-2.5"),
        ("rs", 0x3f000000, ".0.5"),
        ("rs", 0xc0000000, ".-2"),
        ("rs", 0x0, ".0"),
        ("rs", 0x80000000, ".-0"),
        ("rs", 0x7f800000, ".inf"),
        ("rs", 0xff800000, ".-inf"),
        ("rs", 0x7fc00000, ".nan"),
        ("rs", 0x3dcccccd, ".0.1"),
        ("rd", 0x3fb999999999999a, ".~0.1"),
        ("rh", 0x3e00, ".~~1.5"),
        ("rq", 0x3fff8000000000000000000000000000, ".~~~1.5"),
    ];
    for (aura, bits, text) in cases {
        let rendered = rent_co(&Coin::Dime(aura.to_string(), ParsedAtom::Small(*bits)));
        assert_eq!(cord_text(&rendered), *text, "{aura} {bits:#x}");
    }
}

#[test]
fn sea_splits_sign_exponent_and_fraction() {
    let s = |bits: u128| bf(&sea(8, 23, 254, &ParsedAtom::Small(bits)));
    assert_eq!(s(0), ffin(true, 0, 0), "zero");
    assert_eq!(s(0x80000000), ffin(false, 0, 0), "negative zero");
    assert_eq!(s(0x1), ffin(true, -149, 1), "smallest subnormal");
    assert_eq!(s(0x3fc00000), ffin(true, -23, 0xc00000), "1.5");
    assert_eq!(s(0xbfc00000), ffin(false, -23, 0xc00000), "-1.5");
    assert_eq!(s(0x7f800000), F::Inf(true));
    assert_eq!(s(0xff800000), F::Inf(false));
    assert_eq!(s(0x7fc00000), F::Nan);
    assert!(sig(23, 8, &ParsedAtom::Small(0x3fc00000)));
    assert!(!sig(23, 8, &ParsedAtom::Small(0xbfc00000)));
}

#[test]
fn drg_emits_shortest_digits_and_tightens_power_of_two_bounds() {
    // 8 = 1000b with p=4: a == 2^(p-1) takes the halfway branch under %i
    let (k, o) = drg(si(0), big(8), 4, si(0), 20, 'i');
    assert_eq!((k, o), (si(0), big(8)), "exact integer under %i");
    let (k, o) = drg(si(0), big(8), 4, si(0), 20, 'd');
    assert_eq!(
        (k, o),
        (si(0), big(8)),
        "e == v under %d skips the tightening"
    );
    // a small mantissa is widened by xpd first
    let (k, o) = drg(si(0), big(1), 4, si(-10), 20, 'd');
    assert_eq!((k, o), (si(0), big(1)));
    // 12 x 2^-4 = 0.75
    let (k, o) = drg(si(-4), big(12), 4, si(-10), 20, 'd');
    assert_eq!((k, o), (si(-2), big(75)));
    // an exponent below v leaves xpd nothing to widen
    let (k, o) = drg(si(-12), big(1), 4, si(-10), 20, 'd');
    assert_eq!((k, o), (si(-4), big(2)));
}

#[test]
fn drg_fl_maps_each_binary_class() {
    assert_eq!(df(&drg_fl(fin(false, 0, 0), 23, 8, 254)), ffin(false, 0, 0));
    assert_eq!(
        df(&drg_fl(BinaryFloat::Infinity { sign: false }, 23, 8, 254)),
        F::Inf(false)
    );
    assert_eq!(df(&drg_fl(BinaryFloat::NaN, 23, 8, 254)), F::Nan);
    assert_eq!(
        df(&drg_fl(fin(true, -23, 0xc00000), 23, 8, 254)),
        ffin(true, -1, 15)
    );
}

#[test]
fn float_sign_and_rounding_helpers() {
    assert_eq!(swr('d'), 'u');
    assert_eq!(swr('u'), 'd');
    assert_eq!(swr('n'), 'n');
    assert_eq!(swr('z'), 'z');
    assert_eq!(bf(&fli(fin(true, 3, 5))), ffin(false, 3, 5));
    assert_eq!(
        bf(&fli(BinaryFloat::Infinity { sign: true })),
        F::Inf(false)
    );
    assert_eq!(bf(&fli(BinaryFloat::NaN)), F::Nan);
    assert_eq!(bf(&zer()), ffin(false, 0, 0));
}

#[test]
fn signed_integer_helpers_follow_hoon_si() {
    assert!(syn_si(si(3)) && !syn_si(si(-3)) && syn_si(0));
    assert_eq!(abs_si(si(-7)), 7);
    assert_eq!(old_si(si(-7)), (false, 7));
    assert_eq!(new_si(true, 0), 0);
    assert_eq!(new_si(false, 0), 0);
    assert_eq!(new_si(false, 4), si(-4));
    assert_eq!(sum_si(si(-2), si(-3)), si(-5));
    assert_eq!(sum_si(si(-2), si(5)), si(3));
    assert_eq!(sum_si(si(-5), si(2)), si(-3));
    assert_eq!(sum_si(si(5), si(-2)), si(3));
    assert_eq!(sum_si(si(2), si(-5)), si(-3));
    assert_eq!(dif_si(si(2), si(5)), si(-3));
    assert_eq!(me(254, 23), si(-149), "@rs minimum exponent");
    // cmp: 0 equal, 1 (= -1) less, 2 (= --1) greater
    assert_eq!(cmp_si(si(4), si(4)), 0);
    assert_eq!(cmp_si(si(5), si(3)), 2);
    assert_eq!(cmp_si(si(3), si(5)), 1);
    assert_eq!(cmp_si(si(3), si(-5)), 2);
    assert_eq!(cmp_si(si(-3), si(5)), 1);
    assert_eq!(cmp_si(si(-5), si(-3)), 1);
    assert_eq!(cmp_si(si(-3), si(-5)), 2);
    assert_eq!(bex(0), 1);
    assert_eq!(bex(10), 1024);
    assert_eq!(pow(5, 0), big(1));
    assert_eq!(pow(5, 3), big(125));
}

// A tiny 4-bit-precision format for exercising the +lug rounding modes:
// p = 4, minimum exponent v = -10, exponent span w = 20 (emx = 10).
const P4: u128 = 4;
const W20: u128 = 20;

fn div4(a_e: i128, a: u128, b: u128, r: char, d: char) -> F {
    bf(&binaryfloat_div_internal(
        si(a_e),
        big(a),
        si(0),
        big(b),
        P4,
        si(-10),
        W20,
        r,
        d,
    ))
}

fn mul4(a_e: i128, a: u128, r: char, d: char) -> F {
    bf(&binaryfloat_mul_internal(
        si(a_e),
        big(a),
        si(0),
        big(1),
        P4,
        si(-10),
        W20,
        r,
        d,
    ))
}

#[test]
fn lug_rounds_inexact_quotients_by_mode() {
    // 1/3 = 0.0101..b: floor 10 x 2^-5, ceiling and nearest 11 x 2^-5
    assert_eq!(div4(0, 1, 3, 'z', 'd'), ffin(true, -5, 10));
    assert_eq!(div4(0, 1, 3, 'd', 'd'), ffin(true, -5, 10));
    assert_eq!(div4(0, 1, 3, 'u', 'd'), ffin(true, -5, 11));
    assert_eq!(div4(0, 1, 3, 'a', 'd'), ffin(true, -5, 11));
    assert_eq!(div4(0, 1, 3, 'n', 'd'), ffin(true, -5, 11));
    assert_eq!(
        div4(0, 1, 3, '?', 'd'),
        ffin(true, -5, 11),
        "unknown mode rounds nearest"
    );
}

#[test]
fn lug_rounds_exact_products_to_even() {
    // 17 = 10001b -> tie, even 8 stays: 8 x 2^1
    assert_eq!(mul4(0, 17, 'n', 'd'), ffin(true, 1, 8));
    // 19 = 10011b -> tie, odd 9 rounds up to 10
    assert_eq!(mul4(0, 19, 'n', 'd'), ffin(true, 1, 10));
    // 31 = 11111b -> tie, odd 15 carries into a fifth bit and renormalizes
    assert_eq!(mul4(0, 31, 'n', 'd'), ffin(true, 2, 8));
    // 33 = 100001b -> below half, truncate
    assert_eq!(mul4(0, 33, 'n', 'd'), ffin(true, 2, 8));
    // 35 = 100011b -> above half, round up
    assert_eq!(mul4(0, 35, 'n', 'd'), ffin(true, 2, 9));
    // floor keeps the truncated mantissa; ceiling bumps an inexact one
    assert_eq!(mul4(0, 31, 'z', 'd'), ffin(true, 1, 15));
    assert_eq!(mul4(0, 17, 'u', 'd'), ffin(true, 1, 9));
}

#[test]
fn lug_flushes_values_below_the_minimum_exponent() {
    // quotient 21 (5 bits) lands wholly below v: q = 16 > m
    assert_eq!(div4(-20, 1, 3, 'z', 'd'), ffin(true, 0, 0), "floor to zero");
    assert_eq!(
        div4(-20, 1, 3, 'u', 'd'),
        ffin(true, -10, 1),
        "ceiling to the least subnormal"
    );
    assert_eq!(
        div4(-20, 1, 3, 'n', 'd'),
        ffin(true, 0, 0),
        "nearest, far below half"
    );
    // q == m: the dropped bits are the whole mantissa (21 > 2^4, inexact)
    assert_eq!(
        div4(-9, 1, 3, 'n', 'd'),
        ffin(true, -10, 1),
        "nearest, above half"
    );
    // exact quotients: exactly half rounds to zero, above half rounds up
    assert_eq!(
        div4(-11, 1, 1, 'n', 'd'),
        ffin(true, 0, 0),
        "exact half ties to zero"
    );
    assert_eq!(
        div4(-12, 3, 1, 'n', 'd'),
        ffin(true, -10, 1),
        "exact above half"
    );
}

#[test]
fn lug_overflow_and_denormal_modes() {
    // e = 30 > emx = 10: infinity unless the exponent is unbounded (%i)
    assert_eq!(mul4(30, 8, 'n', 'd'), F::Inf(true));
    assert_eq!(mul4(30, 8, 'n', 'i'), ffin(true, 30, 8));
    // %i widens a short exact mantissa to full precision (xpd)
    assert_eq!(mul4(0, 3, 'n', 'i'), ffin(true, -2, 12));
    // %f leaves an infinity alone
    assert_eq!(mul4(30, 8, 'n', 'f'), F::Inf(true));
    // %d widens a short exact mantissa only down to the minimum exponent
    assert_eq!(mul4(-9, 1, 'n', 'd'), ffin(true, -10, 2));
}

#[test]
fn binaryfloat_mul_handles_nan_infinity_and_zero() {
    let (p, v, w) = (24u128, si(-149), 253u128);
    let one = || fin(true, 0, 1);
    let neg3 = || fin(false, 0, 3);
    let zero = || fin(true, 0, 0);
    let inf = |s| BinaryFloat::Infinity { sign: s };
    let m = |a, b| bf(&binaryfloat_mul(a, b, p, v, w, 'n', 'd'));
    assert_eq!(m(BinaryFloat::NaN, one()), F::Nan);
    assert_eq!(m(one(), BinaryFloat::NaN), F::Nan);
    assert_eq!(m(inf(true), inf(false)), F::Inf(false));
    assert_eq!(m(inf(true), inf(true)), F::Inf(true));
    assert_eq!(m(inf(true), zero()), F::Nan, "inf x 0");
    assert_eq!(m(inf(true), neg3()), F::Inf(false));
    assert_eq!(m(zero(), inf(false)), F::Nan, "0 x inf");
    assert_eq!(m(neg3(), inf(true)), F::Inf(false));
    assert_eq!(m(zero(), neg3()), ffin(false, 0, 0), "signed zero product");
    assert_eq!(m(one(), zero()), ffin(true, 0, 0), "zero on the right");
    // mixed signs: -3 x 5 = -15, widened to 24 bits
    assert_eq!(m(neg3(), fin(true, 0, 5)), ffin(false, -20, 15 << 20));
}

#[test]
fn binaryfloat_div_handles_nan_infinity_and_zero() {
    let (p, v, w) = (24u128, si(-149), 253u128);
    let one = || fin(true, 0, 1);
    let neg3 = || fin(false, 0, 3);
    let zero = |s| fin(s, 0, 0);
    let inf = |s| BinaryFloat::Infinity { sign: s };
    let d = |a, b| bf(&binaryfloat_div(a, b, p, v, w, 'n', 'd'));
    assert_eq!(d(BinaryFloat::NaN, one()), F::Nan);
    assert_eq!(d(one(), BinaryFloat::NaN), F::Nan);
    assert_eq!(d(inf(true), inf(true)), F::Nan, "inf / inf");
    assert_eq!(d(inf(true), neg3()), F::Inf(false));
    assert_eq!(d(inf(false), neg3()), F::Inf(true));
    assert_eq!(d(neg3(), inf(true)), ffin(false, 0, 0), "finite / inf");
    assert_eq!(d(zero(true), zero(false)), F::Nan, "0 / 0");
    assert_eq!(d(zero(true), neg3()), ffin(false, 0, 0));
    assert_eq!(d(neg3(), zero(true)), F::Inf(false), "x / 0");
    // same signs: 1/1 = 1; opposite signs: -3/1 = -3
    assert_eq!(d(one(), one()), ffin(true, -23, 1 << 23));
    assert_eq!(d(neg3(), one()), ffin(false, -22, 3 << 22));
}

#[test]
fn bif_packs_every_class() {
    let b = |x| atom_u128(&bif(x, 8, 23, 254, 'n'));
    assert_eq!(b(BinaryFloat::Infinity { sign: true }), 0x7f800000);
    assert_eq!(b(BinaryFloat::Infinity { sign: false }), 0xff800000);
    assert_eq!(b(BinaryFloat::NaN), 0x7fc00000);
    assert_eq!(b(fin(true, 0, 0)), 0);
    assert_eq!(b(fin(false, 0, 0)), 0x80000000);
    assert_eq!(b(fin(true, -149, 1)), 1, "subnormal");
    assert_eq!(b(fin(false, -149, 1)), 0x80000001, "negative subnormal");
    assert_eq!(b(fin(true, -23, 0xc00000)), 0x3fc00000);
    assert_eq!(b(fin(false, -23, 0xc00000)), 0xbfc00000);
}

#[test]
fn fil_repeats_a_block() {
    assert_eq!(fil(0, 0, 1), ParsedAtom::Small(0));
    assert_eq!(atom_u128(&fil(0, 8, 1)), 0xff);
    assert_eq!(atom_u128(&fil(3, 4, 0xab)), 0xabababab);
    assert_eq!(fil(3, 2, 0).to_biguint(), BigUint::zero(), "zero block");
    // 2^7-bit blocks: the mask is the whole u128
    assert_eq!(fil(7, 2, 1).to_biguint(), (big(1) << 128) + big(1));
    // wider than 128 bits goes through BigUint
    let want = (0..17).fold(BigUint::zero(), |acc, i| acc + (big(1) << (8 * i)));
    assert_eq!(fil(3, 17, 1).to_biguint(), want);
}

#[test]
fn grd_fl_passes_nan_and_infinity_through() {
    assert_eq!(bf(&grd_fl(DecimalFloat::NaN, 254, 23, 8, 'n')), F::Nan);
    assert_eq!(
        bf(&grd_fl(
            DecimalFloat::Infinity { sign: false },
            254,
            23,
            8,
            'n'
        )),
        F::Inf(false)
    );
    assert_eq!(atom_u128(&ryls(DecimalFloat::NaN)), 0x7fc00000);
    assert_eq!(
        atom_u128(&ryld(DecimalFloat::Infinity { sign: false })),
        0xfff0000000000000
    );
}

// ---------------------------------------------------------------------------
// token parsers
// ---------------------------------------------------------------------------

#[test]
fn winglist_parses_every_limb_form() {
    let w = |s: &str| {
        winglist()
            .parse(s)
            .into_result()
            .unwrap_or_else(|e| panic!("{s}: {e:?}"))
    };
    assert_eq!(w(","), vec![Limb::Parent(0, None)]);
    assert_eq!(w("^^a"), vec![Limb::Parent(2, Some("a".into()))]);
    assert_eq!(w("$"), vec![term("$")]);
    assert_eq!(w("+6"), vec![ax(6)]);
    // &n and |n address the nth list item and the nth tail (++rope)
    assert_eq!(w("&3"), vec![ax(14)]);
    assert_eq!(w("|3"), vec![ax(15)]);
    assert_eq!(w("."), vec![ax(1)]);
    assert_eq!(w("+"), vec![ax(3)]);
    assert_eq!(w("-"), vec![ax(2)]);
    assert_eq!(w("+<"), vec![ax(6)]);
    assert_eq!(w("->+"), vec![ax(11)]);
    assert_eq!(w("a.+<.b"), vec![term("a"), ax(6), term("b")]);
}

#[test]
fn wing_keeps_axis_zero_as_a_plain_wing() {
    assert_eq!(
        wing().parse("+0").into_result().unwrap(),
        Hoon::Wing(vec![ax(0)])
    );
    assert_eq!(
        wing().parse("+3").into_result().unwrap(),
        Hoon::CenTis(vec![ax(3)], vec![])
    );
    assert_eq!(
        wing().parse("^a").into_result().unwrap(),
        Hoon::Wing(vec![Limb::Parent(1, Some("a".into()))])
    );
}

#[test]
fn concatanate_pairs_two_wide_hoons() {
    // `a^b` builds a cell of its two sides (no caller in the parser today)
    let parsed = concatanate(wing_p()).parse("a^b").into_result().unwrap();
    assert_eq!(
        parsed,
        Hoon::Pair(
            Box::new(Hoon::Wing(vec![term("a")])),
            Box::new(Hoon::Wing(vec![term("b")]))
        )
    );
}

#[test]
fn name_lists_and_term_lists() {
    assert_eq!(
        list_names_tall()
            .parse("a  b\n  c\n==")
            .into_result()
            .unwrap(),
        vec!["a".to_string(), "b".into(), "c".into()]
    );
    assert_eq!(
        list_names_wide().parse("[a b]").into_result().unwrap(),
        vec!["a".to_string(), "b".into()]
    );
    let terms = list_term_hoon(wing_p())
        .parse("a  b\nc  d\n")
        .into_result()
        .unwrap();
    assert_eq!(terms.len(), 2);
    assert_eq!(terms[1].0, "c");
    assert!(gap().parse("  ::  comment\n  ").into_result().is_ok());
}

#[test]
fn variable_name_and_type_forms() {
    let star = just('*').to(Spec::Base(BaseType::NounExpr));
    let atom = just('@')
        .ignore_then(symbol())
        .map(|s| Spec::Base(BaseType::Atom(s)));
    let spec = choice((star, atom)).boxed();
    let v = |s: &'static str| variable_name_and_type(spec.clone()).parse(s).into_result();
    // =@ud autonames to ud
    let Ok(Skin::Name(name, _)) = v("=@ud") else {
        panic!("=@ud should autoname");
    };
    assert_eq!(name, "ud");
    // =* cannot be autonamed, and nothing else matches
    assert!(v("=*").is_err(), "a noun spec has no autoname");
    assert_eq!(v("a").unwrap(), Skin::Term("a".into()));
    assert!(matches!(v("a/@ud").unwrap(), Skin::Name(n, _) if n == "a"));
    assert!(matches!(v("a=*").unwrap(), Skin::Name(n, _) if n == "a"));
    assert!(matches!(v("*").unwrap(), Skin::Spec(_, _)));
}

#[test]
fn tiki_forms_name_wings_and_hoons() {
    let tw = |s: &str| tiki_wide(wing_p()).parse(s).into_result().unwrap();
    assert_eq!(tw("b=a"), Tiki::Wing((Some("b".into()), vec![term("a")])));
    assert_eq!(tw("a"), Tiki::Wing((None, vec![term("a")])));
    let hoon = just('%')
        .ignore_then(symbol())
        .map(|s| Hoon::Rock("tas".into(), NounExpr::ParsedAtom(string_to_atom(s))))
        .boxed();
    let tt = |s: &'static str| tiki_tall(hoon.clone(), wing_p()).parse(s).into_result();
    assert_eq!(
        tt("^=  b=a").unwrap(),
        Tiki::Wing((Some("b".into()), vec![term("a")]))
    );
    assert!(matches!(tt("^=  b=%foo").unwrap(), Tiki::Hoon((Some(n), _)) if n == "b"));
    assert!(matches!(tt("%foo").unwrap(), Tiki::Hoon((None, _))));
}

#[test]
fn tell_and_yell_wrap_wide_lists() {
    assert_eq!(
        tell(wing_p()).parse("<a b>").into_result().unwrap(),
        Hoon::Tell(vec![
            Hoon::Wing(vec![term("a")]),
            Hoon::Wing(vec![term("b")])
        ])
    );
    assert_eq!(
        yell_parser(wing_p()).parse(">a<").into_result().unwrap(),
        Hoon::Yell(vec![Hoon::Wing(vec![term("a")])])
    );
}

// ---------------------------------------------------------------------------
// tapes and cords
// ---------------------------------------------------------------------------

fn woof_bytes(w: &[Woof]) -> Vec<u128> {
    w.iter()
        .map(|w| match w {
            Woof::ParsedAtom(a) => atom_u128(a),
            Woof::Hoon(_) => u128::MAX,
        })
        .collect()
}

fn soil_parse(src: &str) -> Result<Vec<Woof>, String> {
    let linemap = Arc::new(LineMap::new(src));
    soil(wing_p(), linemap)
        .parse(src)
        .into_result()
        .map_err(|e| format!("{e:?}"))
}

#[test]
fn wide_tapes_decode_escapes_and_interpolation() {
    let w = soil_parse(r#""a\\b\"c\{d\41{e}""#).unwrap();
    let mut want: Vec<u128> = "a\\b\"c{dA".bytes().map(u128::from).collect();
    want.push(u128::MAX);
    assert_eq!(woof_bytes(&w), want);
    assert!(
        soil_parse("\"a\nb\"").is_err(),
        "raw newline in a wide tape"
    );
    assert!(soil_parse(r#""a\qb""#).is_err(), "unknown escape");
    assert!(soil_parse("\"a\x7fb\"").is_err(), "DEL in a wide tape");
    assert!(
        soil_parse(r#""a{ }b""#).is_err(),
        "unparseable interpolation"
    );
}

#[test]
fn tall_tapes_strip_the_block_indent() {
    let src = "  \"\"\"\n  one\n    two {x}\n\n  \\{\\\\\\41\n  \"\"\"";
    let w = soil_parse(src).unwrap();
    let text: String = woof_bytes(&w)
        .into_iter()
        .map(|b| {
            if b == u128::MAX {
                '#'
            } else {
                char::from_u32(b as u32).unwrap()
            }
        })
        .collect();
    assert_eq!(text, "one\n  two #\n\n{\\A");
    assert!(
        soil_parse("  \"\"\"\n  one\n \"\"\"").is_err(),
        "closing delimiter must match the opening indent"
    );
    assert!(
        soil_parse("  \"\"\"\n  one\n two\n  \"\"\"").is_err(),
        "a short content line is inconsistent"
    );
    assert!(
        soil_parse("\"\"\"\n\x01\n\"\"\"").is_err(),
        "control char in a tall tape"
    );
    assert!(
        soil_parse("\"\"\"\n\x7f\n\"\"\"").is_err(),
        "DEL in a tall tape"
    );
    assert!(
        soil_parse("\"\"\"\n\\q\n\"\"\"").is_err(),
        "unknown tall escape"
    );
    // a whitespace-only line shorter than the block indent is rejected (as
    // hoonc does)
    assert!(soil_parse("  \"\"\"\n  a\n \n  \"\"\"").is_err());
}

#[test]
fn tape_joins_adjacent_soils() {
    let src = "\"ab\".\"cd\"";
    let linemap = Arc::new(LineMap::new(src));
    let Hoon::Knit(w) = tape(wing_p(), linemap).parse(src).into_result().unwrap() else {
        panic!("tape should knit");
    };
    assert_eq!(
        woof_bytes(&w),
        "abcd".bytes().map(u128::from).collect::<Vec<_>>()
    );
}

fn cord_parse(src: &str) -> Result<BigUint, String> {
    let linemap = Arc::new(LineMap::new(src));
    cord(linemap)
        .parse(src)
        .into_result()
        .map(|a| a.to_biguint())
        .map_err(|e| format!("{e:?}"))
}

#[test]
fn cords_decode_escapes_and_continuations() {
    assert_eq!(cord_parse(r"'a\\b\'c\41'").unwrap(), cord_atom("a\\b'cA"));
    assert_eq!(cord_parse("'foo\\\n  /bar'").unwrap(), cord_atom("foobar"));
    assert!(
        cord_parse("'a\tb'").is_err(),
        "control characters are not cord text"
    );
    assert!(cord_parse("'a\x7fb'").is_err(), "DEL is not cord text");
}

#[test]
fn triple_quoted_cords_strip_the_block_indent() {
    let src = "  '''\n  one\n    two\n\n  '''";
    assert_eq!(cord_parse(src).unwrap(), cord_atom("one\n  two\n"));
    assert!(
        cord_parse("  '''\n  one\n '''").is_err(),
        "closing indent mismatch"
    );
    assert!(
        cord_parse("  '''\n  one\n two\n  '''").is_err(),
        "inconsistent indent"
    );
    // a blank content line alone yields the empty cord; so does a block with
    // no content line at all (hoonc agrees: coverage/p2/p2_text.hoon)
    assert_eq!(cord_parse("  '''\n\n  '''").unwrap(), BigUint::zero());
    // a whitespace-only line shorter than the block indent is rejected (as
    // hoonc does)
    assert!(cord_parse("  '''\n  a\n \n  '''").is_err());
    assert_eq!(cord_parse("  '''\n  \n  '''").unwrap(), BigUint::zero());
}

#[test]
fn constants_cover_cords_and_loobeans() {
    let c = |s: &str| {
        constant(Arc::new(LineMap::new(s)))
            .parse(s)
            .into_result()
            .unwrap()
    };
    assert_eq!(c("%$"), Coin::Dime("tas".into(), ParsedAtom::Small(0)));
    assert_eq!(c("%&"), Coin::Dime("f".into(), ParsedAtom::Small(0)));
    assert_eq!(c("%|"), Coin::Dime("f".into(), ParsedAtom::Small(1)));
    let Coin::Dime(aura, atom) = c("%'hi'") else {
        panic!("a cord constant is a dime")
    };
    assert_eq!((aura.as_str(), atom.to_biguint()), ("t", cord_atom("hi")));
}

// ---------------------------------------------------------------------------
// atom helpers: dates, signs, base58check, knots, UTF-8/UTF-32
// ---------------------------------------------------------------------------

fn days(n: u128) -> u128 {
    (n * 86_400) << 64
}

#[test]
fn year_counts_days_across_leap_rules() {
    let da = |y, m, d| year(true, y, m, d, 0, 0, 0, &[]);
    assert_eq!(da(1970, 1, 1), 0x8000000cce9e0d80u128 << 64, "unix epoch");
    assert_eq!(da(2024, 3, 1) - da(2024, 2, 28), days(2), "2024 is leap");
    assert_eq!(da(2023, 3, 1) - da(2023, 2, 28), days(1));
    assert_eq!(
        da(1900, 3, 1) - da(1900, 2, 28),
        days(1),
        "1900 is not leap"
    );
    assert_eq!(da(2000, 3, 1) - da(2000, 2, 28), days(2), "2000 is leap");
    assert_eq!(da(2100, 1, 1) - da(2000, 1, 1), days(36_525));
    assert_eq!(da(1904, 1, 1) - da(1900, 1, 1), days(1_460));
    assert_eq!(da(2101, 1, 1) - da(2100, 1, 1), days(365));
    // parsed literals agree
    let (aura, atom) = sand_atom(&parse_expr("~2024.3.1"));
    assert_eq!((aura.as_str(), atom), ("da", big(da(2024, 3, 1))));
}

#[test]
fn apply_sign_encodes_small_and_big_magnitudes() {
    assert_eq!(
        apply_sign(true, ParsedAtom::Small(5)),
        ParsedAtom::Small(10)
    );
    assert_eq!(
        apply_sign(false, ParsedAtom::Small(5)),
        ParsedAtom::Small(9)
    );
    assert_eq!(
        apply_sign(false, ParsedAtom::Small(0)),
        ParsedAtom::Small(0)
    );
    let n: BigUint = big(1) << 130;
    assert_eq!(
        apply_sign(true, ParsedAtom::Big(n.clone())),
        ParsedAtom::Big(&n << 1)
    );
    assert_eq!(
        apply_sign(false, ParsedAtom::Big(n.clone())),
        ParsedAtom::Big(((&n - 1u32) << 1) + 1u32)
    );
    assert_eq!(
        apply_sign(false, ParsedAtom::Big(BigUint::zero())),
        ParsedAtom::Big(BigUint::zero())
    );
    // through the parser
    assert_eq!(sand_atom(&parse_expr("-0")).1, big(0));
    assert_eq!(
        sand_atom(&parse_expr(
            "-0x1.0000.0000.0000.0000.0000.0000.0000.0000.0000"
        ))
        .1,
        (big(1) << 145) - 1u32
    );
}

const BTC: &str = "1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2";

#[test]
fn base58check_addresses_decode_and_reencode() {
    let payload = base58_to_atom(BTC.to_string()).expect("valid address");
    assert_eq!(pad_fa(&payload), 1, "a P2PKH payload is 20 bytes");
    let (aura, atom) = sand_atom(&parse_expr(&format!("0c{BTC}")));
    assert_eq!((aura.as_str(), atom), ("uc", payload.to_biguint()));
    // enc_fa re-attaches the checksum that den_fa strips
    let full = enc_fa(&payload);
    assert_eq!(
        den_fa(&full).map(|a| a.to_biguint()),
        Some(payload.to_biguint())
    );
    // a flipped checksum bit and a non-alphabet character are rejected
    assert!(den_fa(&ParsedAtom::from_biguint(full.to_biguint() ^ big(1))).is_none());
    assert!(
        base58_to_atom("1".to_string()).is_none(),
        "an all-zero payload fails its checksum"
    );
    assert!(
        base58_to_atom("1Bv0".to_string()).is_none(),
        "0 is not base58"
    );
    assert!(
        base58_to_atom("1Bv\u{e9}".to_string()).is_none(),
        "a Latin-1 character is not base58"
    );
    assert!(
        base58_to_atom("1Bv\u{20ac}".to_string()).is_none(),
        "a character past U+00FF is not base58"
    );
    assert!(
        parse_fails(&format!("0c{}3", &BTC[..BTC.len() - 1])),
        "bad checksum"
    );
    // shay pads short input with zero bytes and truncates long input
    assert_eq!(shay(2, &big(0x61)), shay(2, &big(0x0061)));
    assert_ne!(shay(0, &big(1)), shay(1, &big(1)));
    assert_eq!(shay(1, &big(0x6261)), shay(1, &big(0x61)));
}

#[test]
fn knot_escapes_and_many_coins() {
    let n = |s: &'static str| nusk().parse(s).into_result();
    assert_eq!(
        n("foo").unwrap(),
        Coin::Dime("tas".into(), string_to_atom("foo".into()))
    );
    assert_eq!(
        n("~~.a~-b").unwrap(),
        Coin::Dime("ta".into(), string_to_atom("a_b".into()))
    );
    assert_eq!(
        n("a-b").unwrap(),
        Coin::Dime("tas".into(), string_to_atom("a-b".into()))
    );
    assert!(n("~x").is_err(), "~x is not a knot escape");
    assert!(n("a~").is_err(), "dangling ~");
    assert!(n("a~-b").is_err(), "a_b is not a coin");
    let Coin::Many(coins) = nuck().parse("._1_~~.a__").into_result().unwrap() else {
        panic!("expected %many");
    };
    assert_eq!(coins.len(), 2);
}

#[test]
fn urx_builds_cords_from_escapes() {
    let u = |s: &str| urx().parse(s).into_result().unwrap().to_biguint();
    assert_eq!(u("a1-_"), cord_atom("a1-_"));
    assert_eq!(u("a.b"), cord_atom("a b"));
    assert_eq!(u("~~~."), cord_atom("~."));
    assert_eq!(u("~2605."), cord_atom("\u{2605}"));
    assert_eq!(u("~e9.~1f600."), cord_atom("\u{e9}\u{1f600}"));
}

#[test]
fn tuft_and_taft_convert_between_utf32_and_utf8() {
    for s in ["a", "\u{e9}", "\u{2605}", "\u{1f600}"] {
        let cp = s.chars().next().unwrap() as u128;
        assert_eq!(tuft(&ParsedAtom::Small(cp)).to_biguint(), cord_atom(s));
        assert_eq!(
            atom_u128(&taft(&ParsedAtom::from_biguint(cord_atom(s)))),
            cp
        );
    }
    assert_eq!(taft(&ParsedAtom::Small(0)), ParsedAtom::Small(0));
    // more than four code points needs a big atom
    let five = taft(&ParsedAtom::from_biguint(cord_atom("abcde")));
    let want = "abcde"
        .chars()
        .enumerate()
        .fold(BigUint::zero(), |acc, (i, c)| {
            acc + (big(c as u128) << (32 * i))
        });
    assert_eq!(five.to_biguint(), want);
    // malformed UTF-8 (unreachable from urx) decodes defensively to U+FFFD
    for bytes in [
        vec![0xc3u8, 0x41],
        vec![0xc0, 0x80],
        vec![0xe0, 0x41, 0x80],
        vec![0xe0, 0x80, 0x41],
        vec![0xf0, 0x90, 0x41, 0x80],
        vec![0xf0, 0x90, 0x80, 0x41],
        vec![0xe0, 0x80, 0x80],
        vec![0xf0, 0x41, 0x80, 0x80],
    ] {
        let got = taft(&ParsedAtom::from_biguint(BigUint::from_bytes_le(&bytes)));
        assert_eq!(atom_u128(&got) & 0xffff_ffff, 0xfffd, "{bytes:x?}");
    }
    // @c literals go through urx then taft
    let (aura, atom) = sand_atom(&parse_expr("~-~2605.a"));
    assert_eq!((aura.as_str(), atom), ("c", big(0x2605 | (0x61 << 32))));
    assert_eq!(sand_atom(&parse_expr("~-~0.")).1, big(0));
}

#[test]
fn number_group_parsers() {
    let n = |s: &'static str| number().parse(s).into_result();
    assert!(n("0x0abc").is_err(), "a leading zero group is not @ux");
    assert_eq!(n("0x0").unwrap(), ("ux".to_string(), ParsedAtom::Small(0)));
    assert_eq!(n("0v1.23456").unwrap().1, base32_to_atom("123456".into()));
    assert_eq!(n("0w1.aBc-~").unwrap().1, base64_to_atom("1aBc-~".into()));
    assert_eq!(n("0wz").unwrap().1, base64_to_atom("z".into()));
    assert_eq!(n("0v1").unwrap().1, ParsedAtom::Small(1));
    assert!(
        ipv4_address().parse("1.02.3.4").into_result().is_err(),
        "leading zero octet"
    );
    assert!(ipv4_address().parse("1.2.3.256").into_result().is_err());
    assert_eq!(
        ipv4_address().parse("0.1.2.3").into_result().unwrap(),
        "0.1.2.3"
    );
    assert_eq!(
        ipv4_address().parse("1.2.3.4").into_result().unwrap(),
        "1.2.3.4"
    );
    assert_eq!(
        ipv6_address()
            .parse("1.2.3.4.5.6.7.8")
            .into_result()
            .unwrap(),
        "1:2:3:4:5:6:7:8"
    );
    assert!(
        base32().parse("vz").into_result().is_err(),
        "z is not base32"
    );
    assert!(
        base32().parse("v.").into_result().is_err(),
        "punctuation is not base32"
    );
}

#[test]
fn atom_shift_helpers() {
    let one = ParsedAtom::Small(1);
    assert_eq!(lsh(0, 0, &one), one, "zero shift is the identity");
    assert_eq!(rsh(0, 0, &one), one);
    assert_eq!(lsh(0, 130, &one).to_biguint(), big(1) << 130);
    assert_eq!(
        rsh(0, 130, &ParsedAtom::Small(u128::MAX)),
        ParsedAtom::Small(0)
    );
    // yell walks the fractional 16-bit words down to an empty remainder
    let tarp = yell(&ParsedAtom::Small(1));
    assert_eq!((tarp.d, tarp.h, tarp.m, tarp.s), (0, 0, 0, 0));
    assert_eq!(tarp.f, vec![0, 0, 0, 1]);
}

#[test]
fn list_helpers() {
    assert_eq!(weld(vec![1, 2], vec![3]), vec![1, 2, 3]);
    assert_eq!(scag(2, vec![1, 2, 3]), vec![1, 2]);
    assert_eq!(slag(2, vec![1, 2, 3]), vec![3]);
    assert_eq!(flop(vec![1, 2, 3]), vec![3, 2, 1]);
}

// ---------------------------------------------------------------------------
// paths and coins
// ---------------------------------------------------------------------------

fn ta(s: &str) -> Hoon {
    Hoon::Sand("ta".into(), NounExpr::ParsedAtom(string_to_atom(s.into())))
}

#[test]
fn posh_interpolates_equals_and_percent_segments() {
    let wer = vec!["a".to_string(), "b".into(), "c".into()];
    // no pre tyke: the whole current path
    assert_eq!(
        posh(None, None, wer.clone()),
        Some(vec![ta("a"), ta("b"), ta("c")])
    );
    // =/x fills the first segment from wer
    assert_eq!(
        posh(Some(vec![None, Some(ta("x"))]), None, wer.clone()),
        Some(vec![ta("a"), ta("x")])
    );
    // /x%/y keeps the rest of wer after the pre tyke, then % drops back
    assert_eq!(
        posh(
            Some(vec![Some(ta("x"))]),
            Some((1, vec![Some(ta("y"))])),
            wer.clone()
        ),
        Some(vec![ta("x"), ta("b"), ta("y")])
    );
    // a % suffix whose = has nothing to borrow fails
    assert_eq!(posh(Some(vec![]), Some((0, vec![None])), vec![]), None);
    // a pre = with no wer fails
    assert_eq!(posh(Some(vec![None]), None, vec![]), None);
    // explicit segments need no wer
    assert_eq!(
        posh(Some(vec![Some(ta("x"))]), None, vec![]),
        Some(vec![ta("x")])
    );
}

#[test]
fn jock_builds_rocks_sands_and_tuples() {
    let cell = NounExpr::Cell(
        Box::new(NounExpr::ParsedAtom(ParsedAtom::Small(1))),
        Box::new(NounExpr::ParsedAtom(ParsedAtom::Small(2))),
    );
    assert_eq!(
        jock(true, &Coin::Blob(cell.clone())),
        Hoon::Rock("$".into(), cell.clone())
    );
    assert_eq!(
        jock(false, &Coin::Blob(cell)),
        Hoon::Pair(Box::new(sand("$", 1)), Box::new(sand("$", 2)))
    );
    assert_eq!(
        jock(
            false,
            &Coin::Blob(NounExpr::ParsedAtom(ParsedAtom::Small(3)))
        ),
        sand("$", 3)
    );
    assert_eq!(
        jock(
            true,
            &Coin::Many(vec![Coin::Dime("ud".into(), ParsedAtom::Small(1))])
        ),
        Hoon::ColTar(vec![Hoon::Rock(
            "ud".into(),
            NounExpr::ParsedAtom(ParsedAtom::Small(1))
        )])
    );
    // ~0 blobs through the parser
    assert_eq!(
        parse_expr("~04hh"),
        Hoon::Pair(Box::new(sand("$", 1)), Box::new(sand("$", 2)))
    );
}

// ---------------------------------------------------------------------------
// LineMap spots
// ---------------------------------------------------------------------------

#[test]
fn linemap_spots_shift_tall_tape_lines() {
    let src = "x \t\n    \"\"\"\n\t   abc\n  q\n    \"\"\"\ny";
    let lm = LineMap::new_with_docs(src, false);
    let a = src.find("abc").unwrap();
    let q = src.find('q').unwrap();
    let y = src.rfind('y').unwrap();
    let p = lm.pint(a..q);
    assert_eq!(p.p, (3, 1), "tall tape body lines lose the block indent");
    assert_eq!(p.q, (4, 1), "a column left of the block indent clamps to 1");
    assert_eq!(
        lm.pint(y..y + 1).p,
        (6, 1),
        "lines after the tape are unshifted"
    );
    assert_eq!(LineMap::new("a\nb").pint(2..3).p, (2, 1));
}

// ---------------------------------------------------------------------------
// doc comments (docs-on parses; entry files are parsed docs-off by honk, so
// these anchors are only exercised here and by the prelude)
// ---------------------------------------------------------------------------

/// Render a doc noun: cords as quoted text, other atoms as numbers, cells as
/// right-flattened `[a b c]`.
fn render_noun(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Object(m) if m.contains_key("ParsedAtom") => {
            let a = &m["ParsedAtom"];
            let n: BigUint = if let Some(x) = a.get("Small") {
                x.as_str().unwrap().parse::<BigUint>().unwrap()
            } else {
                a["Big"].as_str().unwrap().parse::<BigUint>().unwrap()
            };
            if n.is_zero() {
                return "0".into();
            }
            let bytes = n.to_bytes_le();
            match String::from_utf8(bytes) {
                Ok(t) if t.chars().all(|c| !c.is_control()) && n > BigUint::from(9u8) => {
                    format!("{t:?}")
                }
                _ => n.to_string(),
            }
        }
        Value::Object(m) if m.contains_key("Cell") => {
            let mut parts = vec![render_noun(&m["Cell"][0])];
            let mut tail = &m["Cell"][1];
            while let Some(c) = tail.get("Cell") {
                parts.push(render_noun(&c[0]));
                tail = &c[1];
            }
            let last = render_noun(tail);
            if last != "0" {
                parts.push(last);
            } else {
                parts.push("~".into());
            }
            format!("[{}]", parts.join(" "))
        }
        other => format!("{other}"),
    }
}

fn collect_helps(v: &serde_json::Value, out: &mut Vec<String>) {
    use serde_json::Value;
    match v {
        Value::Object(m) => {
            for (k, val) in m {
                match k.as_str() {
                    "Help" => out.push(render_noun(val)),
                    "Gist" => out.push(format!("gist {}", render_noun(&val[0]))),
                    "BarCen" => {
                        if let Some(tomes) = val[1].as_object() {
                            let mut names: Vec<_> = tomes.keys().collect();
                            names.sort();
                            for name in names {
                                let what = &tomes[name][0];
                                if !what.is_null() {
                                    out.push(format!("chapter {name} {}", render_noun(what)));
                                }
                            }
                        }
                    }
                    _ => {}
                }
                collect_helps(val, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_helps(x, out)),
        _ => {}
    }
}

/// Every help note in a docs-on parse of `src`, in serialization order.
fn helps(src: &str) -> Vec<String> {
    let linemap = Arc::new(LineMap::new_with_docs(src, true));
    let parsed = crate::native_parser(vec!["t".into(), "doc.hoon".into()], false, linemap)
        .parse(src)
        .into_result()
        .unwrap_or_else(|e| panic!("{src:?} should parse: {e:?}"));
    let text = serde_json::to_string(&parsed).expect("hoon serializes");
    let json = quote_small_atoms(&text);
    let mut out = Vec::new();
    collect_helps(&json, &mut out);
    out
}

/// Parse serialized JSON with `Small` atoms quoted, so u128 values survive
/// the trip through a `Value`.
fn quote_small_atoms(text: &str) -> serde_json::Value {
    let mut quoted = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("\"Small\":") {
        let (head, tail) = rest.split_at(i + 8);
        quoted.push_str(head);
        let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
        quoted.push('"');
        quoted.push_str(&tail[..digits]);
        quoted.push('"');
        rest = &tail[digits..];
    }
    quoted.push_str(rest);
    serde_json::from_str(&quoted).expect("json reparses")
}

#[test]
fn docs_anchor_to_the_file_head_and_its_first_line() {
    // A target on line 0 has nothing above it; a doc block reaching the file head
    // stops at line 0.
    assert_eq!(
        helps("|%\n++  a  1\n--\n"),
        vec![] as Vec<&str>,
        "|%\n++  a  1\n--\n"
    );
    assert_eq!(
        helps("::  |%: top doc\n|%\n++  a  1\n--\n"),
        vec!["[0 \"|%: top doc\" ~]"] as Vec<&str>,
        "::  |%: top doc\n|%\n++  a  1\n--\n"
    );
    assert_eq!(
        helps("::    larg top doc\n::\n::  body\n|%\n++  a  1\n--\n"),
        vec!["[0 \"larg top doc\" [[0 \"body\"] ~] ~]"] as Vec<&str>,
        "::    larg top doc\n::\n::  body\n|%\n++  a  1\n--\n"
    );
}

#[test]
fn larg_docs_keep_code_sections_and_plan_links() {
    // Larg (four-space) summaries: paragraphs, code lines, and a `$name:` link
    // whose details stop at the first code line.
    assert_eq!(
        helps("|%\n::    $foo: larg plan link on an arm\n::\n::  para\n::\n::    code\n::  after\n++  a  1\n--\n"),
        vec!["[[[\"plan\" \"foo\"] ~] \"larg plan link on an arm\" [[0 \"para\"] ~] ~]"] as Vec<&str>,
        "|%\n::    $foo: larg plan link on an arm\n::\n::  para\n::\n::    code\n::  after\n++  a  1\n--\n"
    );
    assert_eq!(
        helps("|%\n::    larg summary\n::\n::  para\n::\n::    code\n::  after\n++  a  1\n--\n"),
        vec!["[0 \"larg summary\" [[0 \"para\"] ~] [[1 \"code\"] [0 \"after\"] ~] ~]"] as Vec<&str>,
        "|%\n::    larg summary\n::\n::  para\n::\n::    code\n::  after\n++  a  1\n--\n"
    );
    assert_eq!(
        helps("|%\n::  prose summary\n::      six\n++  a  1\n--\n"),
        vec![] as Vec<&str>,
        "|%\n::  prose summary\n::      six\n++  a  1\n--\n"
    );
    assert_eq!(
        helps("|%\n::  prose summary\n::    four\n++  a  1\n--\n"),
        vec!["[0 \"four\" ~]"] as Vec<&str>,
        "|%\n::  prose summary\n::    four\n++  a  1\n--\n"
    );
}

#[test]
fn doc_summary_links_need_well_formed_names() {
    // `+name:` / `$name:` links: bare links only in smol docs, lowercase names,
    // and literal text when the link shape fails.
    assert_eq!(
        helps("|%\n::  +a:\n++  a  1\n--\n"),
        vec![] as Vec<&str>,
        "|%\n::  +a:\n++  a  1\n--\n"
    );
    assert_eq!(
        helps("|%\n::  +a:\n::\n::    details\n++  a  1\n--\n"),
        vec!["[[[\"funk\" \"a\"] ~] 0 [[0 \"details\"] ~] ~]"] as Vec<&str>,
        "|%\n::  +a:\n::\n::    details\n++  a  1\n--\n"
    );
    assert_eq!(
        helps("|%\n::  +: empty name\n++  a  1\n--\n"),
        vec!["[0 \"+: empty name\" ~]"] as Vec<&str>,
        "|%\n::  +: empty name\n++  a  1\n--\n"
    );
    assert_eq!(
        helps("|%\n::  +a9: digit name\n++  a9  1\n--\n"),
        vec!["[[[\"funk\" \"a9\"] ~] \"digit name\" ~]"] as Vec<&str>,
        "|%\n::  +a9: digit name\n++  a9  1\n--\n"
    );
    assert_eq!(
        helps("|%\n::  +Foo: upper\n++  a  1\n--\n"),
        vec!["[0 \"+Foo: upper\" ~]"] as Vec<&str>,
        "|%\n::  +Foo: upper\n++  a  1\n--\n"
    );
    assert_eq!(
        helps("|%\n::  +Foo:\n++  a  1\n--\n"),
        vec!["[0 \"+Foo:\" ~]"] as Vec<&str>,
        "|%\n::  +Foo:\n++  a  1\n--\n"
    );
    assert_eq!(
        helps("|%\n::  +foo bar\n++  a  1\n--\n"),
        vec!["[0 \"+foo bar\" ~]"] as Vec<&str>,
        "|%\n::  +foo bar\n++  a  1\n--\n"
    );
    assert_eq!(
        helps("|%\n::  $a\n++  a  1\n--\n"),
        vec![] as Vec<&str>,
        "|%\n::  $a\n++  a  1\n--\n"
    );
}

#[test]
fn named_arm_lists_pick_the_matching_entry() {
    // A block of two or more `+name: summary` lines documents each named arm; a
    // single named line above a door arm is left to the door.
    assert_eq!(
        helps("|%\n::  +a: one\n::  +b: two\n++  a  1\n++  b  2\n--\n"),
        vec!["[[[\"funk\" \"a\"] ~] \"one\" ~]", "[[[\"funk\" \"b\"] ~] \"two\" ~]"] as Vec<&str>,
        "|%\n::  +a: one\n::  +b: two\n++  a  1\n++  b  2\n--\n"
    );
    assert_eq!(
        helps("|%\n  ::  +a: one\n::  +b: two\n++  a  1\n--\n"),
        vec!["[[[\"funk\" \"a\"] ~] \"one\" ~]"] as Vec<&str>,
        "|%\n  ::  +a: one\n::  +b: two\n++  a  1\n--\n"
    );
    assert_eq!(
        helps("|%\n::  +a: one\n::  +b: two\n\n++  a  1\n--\n"),
        vec!["[[[\"funk\" \"a\"] ~] \"one\" ~]"] as Vec<&str>,
        "|%\n::  +a: one\n::  +b: two\n\n++  a  1\n--\n"
    );
    assert_eq!(
        helps("|%\n::  +a: only\n++  a  1\n--\n"),
        vec!["[[[\"funk\" \"a\"] ~] \"only\" ~]"] as Vec<&str>,
        "|%\n::  +a: only\n++  a  1\n--\n"
    );
    assert_eq!(
        helps("|_  a=@\n::  +b: only\n++  b  1\n--\n"),
        vec![] as Vec<&str>,
        "|_  a=@\n::  +b: only\n++  b  1\n--\n"
    );
}

#[test]
fn plan_and_arm_tail_docs() {
    // `+$` plan docs (named-arm summaries, larg details with code) and `++` arm
    // tails whose details follow an over-indented or pre-detail line.
    assert_eq!(
        helps("|%\n::  $t: plan\n+$  t  @\n::  +t: named\n::  +u: named2\n+$  u  @\n--\n"),
        vec!["[[[\"plan\" \"t\"] ~] \"plan\" ~]", "[[[\"funk\" \"u\"] ~] \"named2\" ~]"]
            as Vec<&str>,
        "|%\n::  $t: plan\n+$  t  @\n::  +t: named\n::  +u: named2\n+$  u  @\n--\n"
    );
    assert_eq!(
        helps("|%\n::    $t: larg plan\n::\n::  words\n::\n::    code\n::  tail\n+$  t  @\n--\n"),
        vec!["[[[\"plan\" \"t\"] ~] \"larg plan\" [[0 \"words\"] ~] ~]", "[0 \"tail\" ~]"]
            as Vec<&str>,
        "|%\n::    $t: larg plan\n::\n::  words\n::\n::    code\n::  tail\n+$  t  @\n--\n"
    );
    assert_eq!(
        helps("|%\n::  $t: plan\n::\n::    code\n::\n::  tail\n+$  t  @\n--\n"),
        vec!["[[[\"plan\" \"t\"] ~] \"plan\" [[0 \"code\"] ~] ~]"] as Vec<&str>,
        "|%\n::  $t: plan\n::\n::    code\n::\n::  tail\n+$  t  @\n--\n"
    );
    assert_eq!(
        helps("|%\n::  +a: arm tail\n::  pre\n::\n::  detail\n++  a  1\n--\n"),
        vec!["[[[\"funk\" \"a\"] ~] \"arm tail\" ~]"] as Vec<&str>,
        "|%\n::  +a: arm tail\n::  pre\n::\n::  detail\n++  a  1\n--\n"
    );
    assert_eq!(
        helps("|%\n::    +a: arm tail larg\n::\n::    over\n::  detail\n++  a  1\n--\n"),
        vec![
            "[[[\"funk\" \"a\"] ~] \"arm tail larg\" [[1 \"over\"] [0 \"detail\"] ~] ~]",
            "[0 \"detail\" ~]"
        ] as Vec<&str>,
        "|%\n::    +a: arm tail larg\n::\n::    over\n::  detail\n++  a  1\n--\n"
    );
    assert_eq!(
        helps(
            "|%\n::  +a: arm tail\n::\n::      - bullet\n::    detail\n::\n::    \n++  a  1\n--\n"
        ),
        vec!["[[[\"funk\" \"a\"] ~] \"arm tail\" ~]", "[0 \"detail\" ~]"] as Vec<&str>,
        "|%\n::  +a: arm tail\n::\n::      - bullet\n::    detail\n::\n::    \n++  a  1\n--\n"
    );
}

#[test]
fn postfix_and_item_docs() {
    // Trailing `::  ` docs on arms, choice items, casts, samples, gates and
    // lines; frag-link blocks above `:*` entries.
    assert_eq!(
        helps(
            "|%\n++  a  ::  +a: postfix\n  1\n++  b  :: odd\n  2\n++  c  1  ::  after body\n--\n"
        ),
        vec!["[[[\"funk\" \"a\"] ~] \"+a: postfix\" ~]", "[0 \"after body\" ~]"] as Vec<&str>,
        "|%\n++  a  ::  +a: postfix\n  1\n++  b  :: odd\n  2\n++  c  1  ::  after body\n--\n"
    );
    assert_eq!(
        helps("|%\n+$  t\n  $%  [%a p=@]  ::  a doc\n      ::  b doc\n      [%b q=@]\n      [%c r=@]  ::   three\n  ==\n--\n"),
        vec!["gist [0 \"a doc\" ~]", "gist [0 \"three\" ~]"] as Vec<&str>,
        "|%\n+$  t\n  $%  [%a p=@]  ::  a doc\n      ::  b doc\n      [%b q=@]\n      [%c r=@]  ::   three\n  ==\n--\n"
    );
    assert_eq!(
        helps("|%\n+$  t\n  $?  %a  ::  a doc\n      ::  b doc\n      %b\n  ==\n--\n"),
        vec!["gist [0 \"a doc\" ~]"] as Vec<&str>,
        "|%\n+$  t\n  $?  %a  ::  a doc\n      ::  b doc\n      %b\n  ==\n--\n"
    );
    assert_eq!(
        helps("|%\n++  a\n  :*  ::  .one: first\n      ::  .two: second\n      1\n      2\n  ==\n--\n"),
        vec!["[[[\"frag\" \"two\"] ~] \"second\" ~]", "[[[\"frag\" \"one\"] ~] \"first\" ~]"] as Vec<&str>,
        "|%\n++  a\n  :*  ::  .one: first\n      ::  .two: second\n      1\n      2\n  ==\n--\n"
    );
    assert_eq!(
        helps("|%\n++  a\n  :*  ::  .one: first\n      ::  .two: second\n      ::    four\n      ::  .three:\n      ::  x\n      1\n      2\n  ==\n--\n"),
        vec!["[[[\"frag\" \"two\"] ~] \"second\" ~]", "[[[\"frag\" \"one\"] ~] \"first\" ~]"] as Vec<&str>,
        "|%\n++  a\n  :*  ::  .one: first\n      ::  .two: second\n      ::    four\n      ::  .three:\n      ::  x\n      1\n      2\n  ==\n--\n"
    );
    assert_eq!(
        helps("|%\n++  a\n  :*  ::  .one: first\n      1\n  ==\n--\n"),
        vec![] as Vec<&str>,
        "|%\n++  a\n  :*  ::  .one: first\n      1\n  ==\n--\n"
    );
    assert_eq!(
        helps("|%\n++  a  :*  ::  .one: opener doc\n  ::  .two: second\n  1\n  2\n==\n--\n"),
        vec!["[[[\"frag\" \"two\"] ~] \"second\" ~]", "[[[\"frag\" \"one\"] ~] \"opener doc\" ~]"]
            as Vec<&str>,
        "|%\n++  a  :*  ::  .one: opener doc\n  ::  .two: second\n  1\n  2\n==\n--\n"
    );
    assert_eq!(
        helps("|%\n+$  t  $~  0  ::  default doc\n  @\n--\n"),
        vec!["[0 \"default doc\" ~]"] as Vec<&str>,
        "|%\n+$  t  $~  0  ::  default doc\n  @\n--\n"
    );
    assert_eq!(
        helps("|%\n++  a\n  ^-  @  ::  cast doc\n  1\n--\n"),
        vec!["gist [0 \"cast doc\" ~]"] as Vec<&str>,
        "|%\n++  a\n  ^-  @  ::  cast doc\n  1\n--\n"
    );
    assert_eq!(
        helps("|%\n++  a\n  =|  b=@  ::  sample doc\n  b\n--\n"),
        vec!["gist [0 \"sample doc\" ~]"] as Vec<&str>,
        "|%\n++  a\n  =|  b=@  ::  sample doc\n  b\n--\n"
    );
    assert_eq!(
        helps("|%\n++  a\n  ::  line doc\n  %+  add  1\n  2\n--\n"),
        vec![] as Vec<&str>,
        "|%\n++  a\n  ::  line doc\n  %+  add  1\n  2\n--\n"
    );
    assert_eq!(
        helps("|%\n++  a\n  (add 1 2)  ::  line expr doc\n--\n"),
        vec!["[0 \"line expr doc\" ~]"] as Vec<&str>,
        "|%\n++  a\n  (add 1 2)  ::  line expr doc\n--\n"
    );
    assert_eq!(
        helps("|%\n++  a\n  |=  b=@  ::  gate doc\n  b\n--\n"),
        vec!["gist [0 \"gate doc\" ~]"] as Vec<&str>,
        "|%\n++  a\n  |=  b=@  ::  gate doc\n  b\n--\n"
    );
    assert_eq!(
        helps("|%\n++  a  ::    scye larg\n  1\n--\n"),
        vec!["[0 \"scye larg\" ~]"] as Vec<&str>,
        "|%\n++  a  ::    scye larg\n  1\n--\n"
    );
    assert_eq!(
        helps("|%\n++  a\n    ::    scye larg\n  1\n--\n"),
        vec![] as Vec<&str>,
        "|%\n++  a\n    ::    scye larg\n  1\n--\n"
    );
}

fn render_expr(n: &NounExpr) -> String {
    let text = serde_json::to_string(n).expect("noun serializes");
    render_noun(&quote_small_atoms(&text))
}

fn render_opt(n: Option<NounExpr>) -> Option<String> {
    n.as_ref().map(render_expr)
}

#[test]
fn docs_on_arms_that_share_the_battery_line() {
    // `|%  ++  a` puts the arm on line 0 (nothing above it); a doc block above
    // that line reaches the file head.
    assert_eq!(helps("|%  ++  a  1\n--\n"), Vec::<String>::new());
    assert_eq!(
        helps("::  +a: x\n|%  ++  a  1\n--\n"),
        vec![r#"[[["funk" "a"] ~] "x" ~]"#, r#"[[["funk" "a"] ~] "x" ~]"#]
    );
    assert_eq!(helps("|%  +$  t  @\n--\n"), Vec::<String>::new());
    assert_eq!(
        helps("::  $t: x\n|%  +$  t  @\n--\n"),
        vec![r#"[[["plan" "t"] ~] "x" ~]"#, r#"[[["plan" "t"] ~] "x" ~]"#]
    );
}

#[test]
fn plan_docs_need_a_linked_summary_at_or_left_of_the_arm() {
    // a doc block left of an indented `+$` does not anchor
    assert_eq!(
        helps("|%\n::  $t: plan\n  +$  t  @\n--\n"),
        Vec::<String>::new()
    );
    // a bare `$t` is not a plan summary
    assert_eq!(helps("|%\n::  $t\n+$  t  @\n--\n"), Vec::<String>::new());
    // pre-detail lines are skipped; details after the code block are dropped
    assert_eq!(
        helps("|%\n::  $t: plan\n::  second\n::\n::    code\n::  tail\n+$  t  @\n--\n"),
        vec![r#"[[["plan" "t"] ~] "plan" ~]"#]
    );
    // an arm tail keeps the four-space detail after an over-indented line
    assert_eq!(
        helps("|%\n::  +a: tail\n::\n::      over\n::  low\n::    four\n++  a  1\n--\n"),
        vec![r#"[[["funk" "a"] ~] "tail" ~]"#, r#"[0 "four" ~]"#]
    );
}

#[test]
fn chapter_labels_skip_a_leading_section_marker() {
    for src in [
        "|%\n::  # Section\n::\n::    chapter summary\n+|  %chap\n++  a  1\n--\n",
        "|%\n::  # Section\n::    chapter summary\n+|  %chap\n++  a  1\n--\n",
        "|%\n::# Section\n::\n::    chapter summary\n+|  %chap\n++  a  1\n--\n",
    ] {
        assert_eq!(
            helps(src),
            vec![r#"chapter chap [0 "chapter summary" ~]"#],
            "{src:?}"
        );
    }
}

#[test]
fn arm_docs_are_not_duplicated_onto_a_body_that_has_them() {
    // the arm-tail detail equals the body's own postfix doc
    assert_eq!(
        helps("|%\n::  +a: tail\n::  pre\n::\n::    line expr doc\n++  a\n  (add 1 2)  ::  line expr doc\n--\n"),
        vec![r#"[[["funk" "a"] ~] "tail" ~]"#, r#"[0 "line expr doc" ~]"#]
    );
    assert_eq!(
        helps("|%\n::  +a: tail\n::  pre\n::\n::    in cenhep\n++  a\n  %-  add\n  (add 1 2)  ::  in cenhep\n--\n"),
        vec![r#"[[["funk" "a"] ~] "tail" ~]"#, r#"[0 "in cenhep" ~]"#]
    );
    // a one-line gate body owns its postfix doc even under an arm-tail note
    assert_eq!(
        helps("|%\n::  +a: tail\n::  pre\n::\n::    detail\n++  a  |=  b=@  b  ::  post\n--\n"),
        vec![r#"[[["funk" "a"] ~] "tail" ~]"#, r#"[0 "post" ~]"#, r#"[0 "detail" ~]"#]
    );
    assert_eq!(
        helps("|%\n::  +a: tail\n::  pre\n::\n::    detail\n++  a  1  ::  detail\n--\n"),
        vec![r#"[[["funk" "a"] ~] "tail" ~]"#, r#"[0 "detail" ~]"#]
    );
    // named-arm list entries combine with postfix docs
    assert_eq!(
        helps("|%\n::  +a: one\n::  +b: two\n++  a  ::  post\n  1\n--\n"),
        vec![r#"[[["funk" "a"] ~] "one" ~]"#, r#"[[["funk" "a"] ~] "post" ~]"#]
    );
    assert_eq!(
        helps("|%\n::  +a: one\n::  +b: two\n++  a  1  ::  post\n--\n"),
        vec![r#"[[["funk" "a"] ~] "one" ~]"#, r#"[0 "post" ~]"#]
    );
}

#[test]
fn postfix_doc_shapes() {
    // `++  name  ::    summary` (scye) with trailing spaces, a `::`-ended or
    // `:`-led comment, detail lines, and a body left of the comment
    assert_eq!(
        helps("|%\n++  a  ::    scye doc  \n  1\n++  b  ::    doc ::\n  1\n++  c  ::    doc :x\n  1\n++  d  ::    :x\n  1\n++  e  ::    doc\n  ::  detail\n  1\n++  f  ::    doc\n            1\n--\n"),
        vec![r#"[0 "scye doc" ~]"#, r#"[0 "doc :x" ~]"#, r#"[0 "doc" ~]"#]
    );
    // line-expression postfix docs: trailing spaces kept out, `::`-ended and
    // `::`-led comments rejected
    assert_eq!(
        helps("|%\n++  a\n  (add 1 2)  ::  doc  \n++  b\n  (add 1 2)  ::  doc ::\n++  c\n  (add 1 2)  ::  doc :x\n++  d\n  (add 1 2)  ::  :: x\n--\n"),
        vec![r#"[0 "doc" ~]"#, r#"[0 "doc :x" ~]"#]
    );
    // three spaces is neither smol nor larg
    assert_eq!(
        helps("|%\n+$  t  $~  0  ::   three\n  @\n--\n"),
        Vec::<String>::new()
    );
}

#[test]
fn frag_blocks_above_coltar_entries() {
    let block = |opener: &str| {
        format!(
            "|%\n++  a\n  :*  1{opener}\n      ::  .a: one\n      ::  .b: two\n      2\n  ==\n--\n"
        )
    };
    let frags = [r#"[[["frag" "b"] ~] "two" ~]"#, r#"[[["frag" "a"] ~] "one" ~]"#];
    // a first entry on line 0 has nothing above it
    assert_eq!(helps(":*  1\n    2\n==\n"), Vec::<String>::new());
    // opener-line comments: `:::`, a bare `::`, and `::`-ended text do not join
    // the block; plain and `:`-bearing text anchors on the first entry
    for opener in ["  :::  x", "  ::", "  ::  x ::"] {
        assert_eq!(helps(&block(opener)), frags.to_vec(), "{opener:?}");
    }
    for (opener, text) in [("  ::  x  ", "x"), ("  ::  a:x", "a:x")] {
        let mut want = vec![format!("[0 {text:?} ~]")];
        want.extend(frags.iter().map(|s| s.to_string()));
        assert_eq!(helps(&block(opener)), want, "{opener:?}");
    }
    // a code line with a trailing comment but no `:*` ends the block
    assert_eq!(
        helps("|%\n++  a\n  :*  1\n      2  ::  x\n      ::  .a: one\n      ::  .b: two\n      3\n  ==\n--\n"),
        vec![r#"[0 "x" ~]"#, frags[0], frags[1]]
    );
    // blank lines are skipped; docs left of the entry are ignored
    assert_eq!(
        helps("|%\n++  a\n  :*  1\n      ::  .a: one\n      ::  .b: two\n\n      2\n  ==\n--\n"),
        frags.to_vec()
    );
    assert_eq!(
        helps("|%\n++  a\n  :*  1\n  ::  .a: one\n  ::  .b: two\n      2\n  ==\n--\n"),
        Vec::<String>::new()
    );
}

#[test]
fn choice_item_and_line_expression_docs() {
    let src = "  [%a p=@]  ::  a doc  \n  %b::    x\n  [%c q=@]    ::    four\n  [%d r=@]\n::  c\n  [%e s=@]\n";
    let lm = LineMap::new_with_docs(src, true);
    let at = |s: &str| src.find(s).unwrap();
    // postfix: trailing spaces trimmed; a span ending on another line starts
    // at that line's first token; a span ending on the final empty line has
    // no doc
    assert_eq!(
        render_opt(lm.help_after_choice_spec_item(2, 10)),
        Some(r#"[0 "a doc" ~]"#.into())
    );
    assert_eq!(
        render_opt(lm.help_after_choice_spec_item(0, src.len())),
        None
    );
    // prefix: the previous line's larg postfix doc anchors the next item
    assert_eq!(
        render_opt(lm.help_before_choice_spec_item(at("[%a"))),
        None,
        "first line"
    );
    assert_eq!(
        render_opt(lm.help_before_choice_spec_item(at("[%c"))),
        Some(r#"[0 "x" ~]"#.into())
    );
    assert_eq!(
        render_opt(lm.help_before_choice_spec_item(at("[%d"))),
        Some(r#"[0 "four" ~]"#.into())
    );
    assert_eq!(
        render_opt(lm.help_before_choice_spec_item(at("[%e"))),
        None,
        "comment line above"
    );

    let src = "::  c\n  x  ::  doc\n\n";
    let lm = LineMap::new_with_docs(src, true);
    assert_eq!(render_opt(lm.help_after_line_expr_ending_at(0)), None);
    assert_eq!(
        render_opt(lm.help_after_line_expr_ending_at(3)),
        None,
        "comment line"
    );
    assert_eq!(
        render_opt(lm.help_after_line_expr_ending_at(src.len())),
        None,
        "blank line"
    );
    assert_eq!(
        render_opt(lm.help_after_line_expr_ending_at(9)),
        Some(r#"[0 "doc" ~]"#.into())
    );
}

#[test]
fn body_spec_doc_targets() {
    let src = "::    doc\n!!\n::    doc2\n   \n::\n::  smol\n  $foo\n";
    let lm = LineMap::new_with_docs(src, true);
    assert_eq!(
        render_opt(lm.help_before_body_spec(src.find("!!").unwrap())),
        Some(r#"[0 "doc" ~]"#.into())
    );
    assert_eq!(
        render_opt(lm.help_before_body_spec(src.find("   \n").unwrap() + 3)),
        None,
        "blank line"
    );
    assert_eq!(
        render_opt(lm.help_before_body_spec(src.find("$foo").unwrap())),
        None,
        "smol prose"
    );
    let src = "::  x\n$foo\n";
    let lm = LineMap::new_with_docs(src, true);
    assert_eq!(
        render_opt(lm.help_before_body_spec(src.find("$foo").unwrap())),
        None
    );
}

/// Start (line, column) of each `:*` entry's %dbug spot in `++main`.
fn coltar_spot_starts(src: &str) -> Vec<(u64, u64)> {
    let lm = Arc::new(LineMap::new_with_docs(src, false));
    let parsed = crate::native_parser(vec!["t".into(), "x.hoon".into()], true, lm)
        .parse(src)
        .into_result()
        .expect("spot source parses");
    let Hoon::TisSig(items) = parsed else {
        panic!("file")
    };
    let Hoon::Dbug(_, core) = &items[0] else {
        panic!("core spot")
    };
    let Hoon::BarCen(None, tomes) = core.as_ref() else {
        panic!("core")
    };
    let Hoon::Dbug(_, main) = &tomes["$"].1["main"] else {
        panic!("main spot")
    };
    let Hoon::ColTar(entries) = main.as_ref() else {
        panic!("coltar")
    };
    entries
        .iter()
        .map(|e| match e {
            Hoon::Dbug(spot, _) => spot.q.p,
            other => panic!("entry without spot: {other:?}"),
        })
        .collect()
}

#[test]
fn dbug_spots_anchor_on_doccord_shaped_comments() {
    let src = "|%\n++  main\n  :*  ::  +link-9: smol\n      1\n      ::  %12.ab: cone\n      2\n      :-  \"it's\"  ::    larg\n      3\n      ::  +foo:\n      4\n      ::  %x: no\n      5\n      ::  %3\n      6\n      ::  +foo: \n      7\n  ==\n--\n";
    // smol en-links (digits in names, `%` cones, a link-only line) and larg
    // comments anchor the next entry's span; a bare `+foo:`, a `+foo: ` with
    // nothing after it, and a non-numeric `%x` do not (as hoonc:
    // coverage/p2/p2_comment_spots.hoon)
    assert_eq!(
        coltar_spot_starts(src),
        vec![(3, 7), (5, 7), (7, 7), (10, 7), (12, 7), (13, 7), (16, 7)]
    );
}
