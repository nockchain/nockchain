//! Coverage-driven tests for atom, path, and spec parsers.
//!
//! Added to close branch-coverage gaps; see the coverage report in the PR.
//!
//! Most expectations are round trips (parse a literal, render it again with
//! `rend_co`, get the same text back) or constants that follow from the
//! hoon-138 definitions (`++year`, `++yule`, `++fein`/`++fynd`, `++jam`).
//! Where hatch is known to disagree with hoonc, the test says so and names the
//! probe under `crates/honk/test-assets/type-probes/coverage/p3/divergent/`;
//! those tests pin only what both compilers agree on, or hatch's current
//! verdict when the divergence is an accept/reject difference.

use std::sync::Arc;

use chumsky::prelude::*;
use nockapp::noun::slab::NounSlab;
use nockvm::noun::{Noun, NounAllocator, D, T};
use num_bigint::BigUint;
use num_traits::{One, ToPrimitive, Zero};

use crate::ast::hoon::{BaseType, BinaryFloat, Coin, Hoon, NounExpr, ParsedAtom, Spec, Spot};
#[allow(unused_imports)]
use crate::utils::*;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn parse_with_docs(src: &str, wer: Vec<String>, bug: bool, docs: bool) -> Result<Hoon, String> {
    let linemap = Arc::new(LineMap::new_with_docs(src, docs));
    crate::native_parser(wer, bug, linemap)
        .parse(src)
        .into_result()
        .map_err(|errs| format!("{errs:?}"))
}

fn parse_with(src: &str, wer: Vec<String>, bug: bool) -> Result<Hoon, String> {
    parse_with_docs(src, wer, bug, false)
}

fn parse_src(src: &str) -> Result<Hoon, String> {
    parse_with(src, vec!["p3".into(), "cov".into(), "hoon".into()], false)
}

fn parse_one(src: &str) -> Hoon {
    match parse_src(src).unwrap_or_else(|e| panic!("{src:?} should parse: {e}")) {
        Hoon::TisSig(mut items) if items.len() == 1 => items.remove(0),
        other => other,
    }
}

fn big(n: u128) -> BigUint {
    BigUint::from(n)
}

/// `(aura, value)` of a literal that parses to a `%sand`.
fn sand(src: &str) -> (String, BigUint) {
    match parse_one(src) {
        Hoon::Sand(aura, NounExpr::ParsedAtom(q)) => (aura, q.to_biguint()),
        other => panic!("{src:?}: expected %sand, got {other:?}"),
    }
}

fn nuck_dime(text: &str) -> (String, ParsedAtom) {
    match nuck().parse(text).into_result() {
        Ok(Coin::Dime(aura, q)) => (aura, q),
        other => panic!("{text:?}: expected a dime, got {other:?}"),
    }
}

fn render(coin: &Coin) -> String {
    rend_co(coin).concat()
}

fn render_dime(aura: &str, q: ParsedAtom) -> String {
    render(&Coin::Dime(aura.to_string(), q))
}

fn text_atom(s: &str) -> BigUint {
    BigUint::from_bytes_le(s.as_bytes())
}

/// Parse `text` as a knot and render it back; canonical knots survive.
fn assert_knot_round_trip(text: &str, aura: &str) {
    let (got_aura, q) = nuck_dime(text);
    assert_eq!(got_aura, aura, "aura of {text:?}");
    let coin = Coin::Dime(aura.to_string(), q);
    assert_eq!(render(&coin), text, "rend of {text:?}");
    assert_eq!(
        rent_co(&coin).to_biguint(),
        text_atom(text),
        "rent of {text:?}"
    );
}

fn path_items(src: &str) -> Vec<Hoon> {
    match parse_one(src) {
        Hoon::ColSig(items) => items,
        other => panic!("{src:?}: expected %clsg, got {other:?}"),
    }
}

fn knot_of(h: &Hoon) -> (String, String) {
    match h {
        Hoon::Sand(aura, NounExpr::ParsedAtom(q)) => {
            let text = if q.is_zero() {
                String::new()
            } else {
                String::from_utf8(q.to_biguint().to_bytes_le()).expect("knot is utf-8")
            };
            (aura.clone(), text)
        }
        other => panic!("expected a %sand path element, got {other:?}"),
    }
}

fn atom_from_bits(bits: &[bool]) -> ParsedAtom {
    let mut n = BigUint::zero();
    for (i, b) in bits.iter().enumerate() {
        if *b {
            n |= BigUint::one() << i;
        }
    }
    ParsedAtom::from_biguint(n)
}

/// hoon `++mat` as a bit vector (LSB first).
fn mat_bits_of(a: &BigUint) -> Vec<bool> {
    if a.is_zero() {
        return vec![true];
    }
    let b = a.bits() as usize;
    let c = usize::BITS as usize - b.leading_zeros() as usize;
    let mut out = vec![false; c];
    out.push(true);
    for i in 0..c - 1 {
        out.push((b >> i) & 1 == 1);
    }
    for i in 0..b {
        out.push(a.bit(i as u64));
    }
    out
}

fn atom(n: u128) -> NounExpr {
    NounExpr::ParsedAtom(ParsedAtom::Small(n))
}

fn cell(a: NounExpr, b: NounExpr) -> NounExpr {
    NounExpr::Cell(Box::new(a), Box::new(b))
}

/// Decode an `@s` as an i64.
fn si(e: u128) -> i64 {
    if e % 2 == 0 {
        (e / 2) as i64
    } else {
        -(((e + 1) / 2) as i64)
    }
}

// ---------------------------------------------------------------------------
// unsigned numbers, signed numbers, loobeans, null
// ---------------------------------------------------------------------------

#[test]
fn unsigned_number_literals_pin_values() {
    assert_eq!(sand("0"), ("ud".into(), big(0)));
    assert_eq!(sand("1.000.000"), ("ud".into(), big(1_000_000)));
    assert_eq!(sand("0xdead.beef"), ("ux".into(), big(0xdead_beef)));
    assert_eq!(sand("0b1.0101"), ("ub".into(), big(0b1_0101)));
    assert_eq!(sand("0v1f"), ("uv".into(), big(47)));
    assert_eq!(sand("0v1.00000"), ("uv".into(), big(1 << 25)));
    assert_eq!(sand("0i123"), ("ui".into(), big(123)));
    // 0w digits are 0-9 a-z A-Z - ~ ('-' = 62, '~' = 63, 'A' = 36)
    let a5 = 36 * (64u128.pow(4) + 64u128.pow(3) + 64u128.pow(2) + 64 + 1);
    assert_eq!(
        sand("0w-~.AAAAA"),
        ("uw".into(), big((62 * 64 + 63) * 64u128.pow(5) + a5))
    );
}

#[test]
fn bitcoin_address_literals_check_base58check() {
    // hash160 of the genesis coinbase key
    let genesis = BigUint::parse_bytes(b"62e907b15cbf27d5425399ebf6f0fb50ebb88f18", 16).unwrap();
    assert_eq!(
        sand("0c1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa"),
        ("uc".into(), genesis.clone())
    );
    assert_eq!(sand("0c1111111111111111111114oLvT2"), ("uc".into(), big(0)));
    // a corrupted checksum is rejected by both the unsigned and signed arms
    assert!(number()
        .parse("0c1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNb")
        .into_result()
        .is_err());
    assert!(number()
        .parse("-0c1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNb")
        .into_result()
        .is_err());
}

#[test]
fn signed_bitcoin_literal_value_is_si_encoded() {
    // hoon ++tash makes -0c... a %sc (regressions/p3_sc_signed_uc.hoon)
    let (p, q) = number()
        .parse("--0c1111111111111111111114oLvT2")
        .into_result()
        .expect("signed @uc parses");
    assert_eq!(p, "sc");
    assert_eq!(q.to_biguint(), big(0));
    let (p, q) = number()
        .parse("-0c1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa")
        .into_result()
        .expect("signed @uc parses");
    assert_eq!(p, "sc");
    let h = BigUint::parse_bytes(b"62e907b15cbf27d5425399ebf6f0fb50ebb88f18", 16).unwrap();
    assert_eq!(q.to_biguint(), h * 2u32 - 1u32);
}

#[test]
fn signed_number_literals_use_si_encoding() {
    assert_eq!(sand("-1"), ("sd".into(), big(1)));
    assert_eq!(sand("--1"), ("sd".into(), big(2)));
    assert_eq!(sand("-0x10"), ("sx".into(), big(31)));
    assert_eq!(sand("--0x10"), ("sx".into(), big(32)));
    assert_eq!(sand("-0b101"), ("sb".into(), big(9)));
    assert_eq!(sand("--0vv"), ("sv".into(), big(62)));
    assert_eq!(sand("-0w1"), ("sw".into(), big(1)));
    assert_eq!(sand("-0i7"), ("si".into(), big(13)));
}

#[test]
fn loobean_and_null_knots() {
    assert_eq!(sand(".y"), ("f".into(), big(0)));
    assert_eq!(sand(".n"), ("f".into(), big(1)));
    assert_eq!(nuck_dime("~"), ("n".to_string(), ParsedAtom::Small(0)));
    assert_eq!(render_dime("f", ParsedAtom::Small(0)), ".y");
    assert_eq!(render_dime("f", ParsedAtom::Small(1)), ".n");
    assert_eq!(render_dime("n", ParsedAtom::Small(0)), "~");
}

#[test]
fn unsigned_knots_round_trip() {
    for text in ["0", "7", "1.000", "1.000.000.007"] {
        assert_knot_round_trip(text, "ud");
    }
    for text in ["0x0", "0x1", "0xdead.beef", "0x1.0000"] {
        assert_knot_round_trip(text, "ux");
    }
    for text in ["0b0", "0b1", "0b1.0101"] {
        assert_knot_round_trip(text, "ub");
    }
    for text in ["0v0", "0v1f", "0v1.00000"] {
        assert_knot_round_trip(text, "uv");
    }
    for text in ["0i0", "0i42"] {
        assert_knot_round_trip(text, "ui");
    }
    assert_knot_round_trip("0c1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa", "uc");
    assert_knot_round_trip("0c1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2", "uc");
}

#[test]
fn signed_decimal_knots_round_trip() {
    for text in ["-1", "--1", "--0", "--1.000", "-1.000.000"] {
        assert_knot_round_trip(text, "sd");
    }
}

#[test]
fn signed_radix_knots_keep_their_sign_marker() {
    // ++rend:co recurses with yed 'u' but keeps hay, so the radix survives
    // (regressions/p3_path_signed_radix.hoon)
    let (_, q) = nuck_dime("-0x10");
    assert_eq!(render_dime("sx", q), "-0x10");
    let (_, q) = nuck_dime("--0x10");
    assert_eq!(render_dime("sx", q), "--0x10");
    let (_, q) = nuck_dime("-0b1010");
    assert_eq!(render_dime("sb", q), "-0b1010");
    let (_, q) = nuck_dime("-0c1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa");
    assert_eq!(
        render_dime("sc", q),
        "-0c1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa"
    );
    let (_, q) = nuck_dime("-1.000");
    assert_eq!(render_dime("sd", q), "-1.000");
    // wider than 128 bits
    let wide = "-0x1.0000.0000.0000.0000.0000.0000.0000.0000.0000";
    let (_, q) = nuck_dime(wide);
    assert_eq!(render_dime("sx", q), wide);
}

#[test]
fn uw_and_zero_uc_renders() {
    // ++w:ne: 0-9 a-z A-Z - ~ (regressions/p3_path_uw.hoon).
    // 62, 63, <10, <36, <62 each take their own arm.
    let n = 62 * 64u128.pow(4) + 63 * 64u128.pow(3) + 5 * 64u128.pow(2) + 30 * 64 + 55;
    assert_eq!(render_dime("uw", ParsedAtom::Small(n)), "0w-~5uT");
    assert_eq!(render_dime("uw", ParsedAtom::Small(1)), "0w1");
    assert_eq!(render_dime("uw", ParsedAtom::Small(0)), "0w0");
    // regressions/p3_path_uc_zero.hoon: (pad:fa 0) is 21 ones, then the
    // checksum of zero
    assert_eq!(
        render_dime("uc", ParsedAtom::Small(0)),
        "0c1111111111111111111114oLvT2"
    );
    assert_knot_round_trip("0c1111111111111111111114oLvT2", "uc");
}

#[test]
fn unknown_auras_render_as_hex() {
    // Only reachable through rend_co directly: nuck never produces these.
    assert_eq!(render_dime("dz", ParsedAtom::Small(0x1f)), "0x1f");
    assert_eq!(render_dime("f", ParsedAtom::Small(2)), "0x2");
    assert_eq!(render_dime("ix", ParsedAtom::Small(0xab)), "0xab");
    assert_eq!(render_dime("rz", ParsedAtom::Small(0x10)), "0x10");
    assert_eq!(render_dime("zz", ParsedAtom::Small(3)), "0x3");
    assert_eq!(render_dime("", ParsedAtom::Small(0)), "0x0");
}

// ---------------------------------------------------------------------------
// @da / @dr
// ---------------------------------------------------------------------------

/// `~1970.1.1` and `~2000.1.1`, from ++year.
const DA_1970: u128 = 0x8000_000c_ce9e_0d80_0000_0000_0000_0000;
const DA_2000: u128 = 0x8000_000d_070b_5100_0000_0000_0000_0000;

#[test]
fn absolute_dates_pin_values() {
    assert_eq!(sand("~1970.1.1"), ("da".into(), big(DA_1970)));
    assert_eq!(sand("~2000.1.1"), ("da".into(), big(DA_2000)));
    assert_eq!(
        DA_2000 - DA_1970,
        946_684_800u128 << 64,
        "unix time of 2000"
    );
    let (_, t) = sand("~2000.1.1..1.2.3");
    assert_eq!(t, big(DA_2000 + ((3600 + 2 * 60 + 3) << 64)));
    let (_, t) = sand("~2000.1.1..0.0.0..abcd.ef01");
    assert_eq!(t, big(DA_2000 + (0xabcd_ef01 << 32)));
    let (_, t) = sand("~2000.1.1..0.0.0..0001.0002.0003.0004");
    assert_eq!(t, big(DA_2000 + 0x0001_0002_0003_0004));
    let (_, t) = sand("~2000.1.2");
    assert_eq!(t, big(DA_2000 + (86_400 << 64)));
    // 2000 is a leap year: 60 days to March 1st
    let (_, t) = sand("~2000.3.1");
    assert_eq!(t, big(DA_2000 + ((60 * 86_400) << 64)));
    assert_eq!(year(true, 2000, 1, 1, 0, 0, 0, &[]), DA_2000);
}

#[test]
fn absolute_dates_round_trip() {
    for text in [
        "~2020.1.1", "~2020.2.29..12.30.45", "~2020.12.31..23.59.59..abcd", "~2020.1.1..00.01.00",
        "~2020.1.1..00.00.01", "~2000.1.1..00.00.00..0001.ffff", "~1999.3.1..01.02.03",
        "~1970.1.1", "~2400.2.29", "~2100.3.1", "~50000.6.15",
    ] {
        assert_knot_round_trip(text, "da");
    }
}

#[test]
fn bc_dates_parse_and_render_the_era_marker() {
    let (aura, q) = nuck_dime("~1-.1.1");
    assert_eq!(aura, "da");
    // ++year: (sub 292.277.024.400 (dec 1)) years after year 0
    assert_eq!(q.to_biguint(), big(year(false, 1, 1, 1, 0, 0, 0, &[])));
    let d = yore(&q);
    assert!(!d.era);
    assert_eq!((d.y.clone(), d.m, d.t.d.clone()), (big(1), 1, big(1)));
    // ++yore: [a=| y=+((sub 292.277.024.400 y.ger))]
    // (regressions/p3_path_bc_date.hoon)
    assert_knot_round_trip("~1-.1.1", "da");
    assert_knot_round_trip("~2-.12.31", "da");
    // AD year 0 is 1 BC
    let (_, q) = nuck_dime("~0.1.1");
    assert_eq!(render_dime("da", q), "~1-.1.1");
    // BC year 0, and BC years before the pivot, crash ++year
    for text in ["0-.1.1", "292277024402-.1.1"] {
        assert!(
            absolute_date().parse(text).into_result().is_err(),
            "{text:?} should be rejected"
        );
    }
    assert_knot_round_trip("~292277024401-.1.1", "da");
}

#[test]
fn absolute_date_range_errors() {
    // both compilers reject these
    for text in ["2020.13.1", "2020.0.1", "2020.1.0"] {
        assert!(
            absolute_date().parse(text).into_result().is_err(),
            "{text:?} should be rejected"
        );
    }
}

#[test]
fn absolute_date_fields_are_not_range_checked() {
    // hoon ++when reads these with dim:ag/dip:ag/dum:ag and no range checks
    // (regressions/p3_da_*.hoon); the overflow carries into the next field.
    let da = |text: &str| absolute_date().parse(text).into_result().unwrap();
    assert_eq!(da("2020.1.32"), da("2020.2.1"));
    assert_eq!(da("2020.1.1..24.0.0"), da("2020.1.2"));
    assert_eq!(da("2020.1.1..1.60.0"), da("2020.1.1..2.0.0"));
    assert_eq!(da("2020.1.1..1.1.60"), da("2020.1.1..1.2.0"));
    assert_eq!(da("2020.1.1..001.02.3"), da("2020.1.1..1.2.3"));
    assert_eq!(da("0.1.1"), da("1-.1.1"));
    // bignum years and days
    let huge = da("600000000000.1.1").to_biguint();
    assert!(huge.bits() > 128, "{huge}");
    assert_eq!(
        da("2020.1.100000000000000000000").to_biguint() - da("2020.1.1").to_biguint(),
        (BigUint::from(99_999_999_999_999_999_999u128) * 86_400u32) << 64
    );
}

#[test]
fn absolute_date_lexical_rules() {
    // mot:ag and dip:ag have no leading zero, qix:ab is exactly four
    // lowercase hex digits, and ++yule crashes on a fifth fraction word
    // (reject/p3_da_*.hoon)
    for text in ["2020.01.1", "2020.1.01", "2020.1.1..1.1.1..0001.0002.0003.0004.0005"] {
        assert!(
            absolute_date().parse(text).into_result().is_err(),
            "{text:?} should be rejected"
        );
    }
    // a short or uppercase fraction group is not part of the literal
    for text in ["2020.1.1..1.1.1..abc", "2020.1.1..1.1.1..ABCD"] {
        let (date, rest) = text.split_at(text.rfind("..").unwrap());
        assert!(
            absolute_date().parse(text).into_result().is_err(),
            "{text:?}"
        );
        assert!(
            absolute_date()
                .then_ignore(just(rest))
                .parse(text)
                .into_result()
                .is_ok(),
            "{text:?}"
        );
        assert!(absolute_date().parse(date).into_result().is_ok());
    }
    assert!(absolute_date()
        .parse("2020.1.1..1.1.1..0001.0002.0003.0004")
        .into_result()
        .is_ok());
    assert!(absolute_date().parse("2020.10.1").into_result().is_ok());
    assert!(absolute_date().parse("2020.12.1").into_result().is_ok());
}

#[test]
fn relative_dates_pin_values() {
    assert_eq!(sand("~s0"), ("dr".into(), big(0)));
    assert_eq!(sand("~s1"), ("dr".into(), big(1 << 64)));
    assert_eq!(sand("~m1"), ("dr".into(), big(60 << 64)));
    assert_eq!(sand("~h1"), ("dr".into(), big(3600 << 64)));
    assert_eq!(sand("~d1"), ("dr".into(), big(86_400 << 64)));
    assert_eq!(
        sand("~d1.h2.m3.s4"),
        ("dr".into(), big((86_400 + 7_200 + 180 + 4) << 64))
    );
    assert_eq!(sand("~s1.s2.s3"), ("dr".into(), big(6 << 64)));
    assert_eq!(
        sand("~s1..8000"),
        ("dr".into(), big((1 << 64) | (0x8000 << 48)))
    );
    assert_eq!(
        sand("~s0..0001.0002"),
        ("dr".into(), big((1 << 48) | (2 << 32)))
    );
}

#[test]
fn relative_dates_round_trip() {
    for text in [
        "~s0", "~s1", "~m1", "~h1", "~d1", "~d1.h2.m3.s4", "~s1..8000", "~d400",
        "~h23.m59.s59..ffff.ffff.ffff.ffff",
    ] {
        assert_knot_round_trip(text, "dr");
    }
    // non-canonical spellings normalize
    let (_, q) = nuck_dime("~s4.m3.h2.d1");
    assert_eq!(render_dime("dr", q), "~d1.h2.m3.s4");
    let (_, q) = nuck_dime("~s60");
    assert_eq!(render_dime("dr", q), "~m1");
    let (_, q) = nuck_dime("~s0..0001");
    assert_eq!(render_dime("dr", q), "~s0..0001");
}

#[test]
fn relative_date_fraction_rules() {
    // hoon ++yule crashes on a fifth fraction and six:ab is lowercase only
    // (reject/p3_dr_frac_*.hoon).
    assert!(relative_date()
        .parse("s1..0001.0002.0003.0004.0005")
        .into_result()
        .is_err());
    assert!(relative_date().parse("s1..ABCD").into_result().is_err());
    assert!(relative_date().parse("s1..abcd").into_result().is_ok());
}

#[test]
fn relative_dates_are_bignums() {
    // dim:ag fields and ++yule are bignums (regressions/p3_dr_*.hoon)
    let dr = |text: &str| {
        relative_date()
            .parse(text)
            .into_result()
            .unwrap()
            .to_biguint()
    };
    assert_eq!(
        dr("d300000000000000"),
        (BigUint::from(300_000_000_000_000u64) * 86_400u32) << 64
    );
    assert_eq!(
        dr("s99999999999999999999"),
        BigUint::from(99_999_999_999_999_999_999u128) << 64
    );
    let (_, q) = nuck_dime("~d300000000000000");
    assert_eq!(render_dime("dr", q), "~d300000000000000");
    let (_, q) = nuck_dime("~s99999999999999999999");
    assert_eq!(render_dime("dr", q), "~d1157407407407407.h9.m46.s39");
}

#[test]
fn date_helpers_agree_with_hoon() {
    assert_eq!(yall(0), (0, 1, 1));
    assert_eq!(yall(59), (0, 2, 29), "year 0 is a leap year");
    assert_eq!(yall(366), (1, 1, 1));
    assert_eq!(yall(146_097), (400, 1, 1));
    assert_eq!(yall(146_097 + 36_525), (500, 1, 1));
    assert_eq!(yall(146_097 + 36_525 + 59), (500, 3, 1), "500 is not leap");
    assert_eq!(yule(1, 2, 3, 4, &[]), (86_400 + 7_200 + 180 + 4) << 64);
    let zero = BigUint::zero();
    assert_eq!(
        yule_big(&zero, &zero, &zero, &zero, &[1, 2, 3, 4]),
        Some(BigUint::from(0x0001_0002_0003_0004u64))
    );
    assert_eq!(
        yule_big(&zero, &zero, &zero, &zero, &[1, 2, 3, 4, 5]),
        None,
        "a fifth fraction word crashes ++yule"
    );
    let t = yell(&ParsedAtom::Small(
        (90_061u128 << 64) | 0x0001_0002_0003_0004,
    ));
    assert_eq!((t.d.clone(), t.h, t.m, t.s), (big(1), 1, 1, 1));
    assert_eq!(t.f, vec![1, 2, 3, 4]);
    let t = yell(&ParsedAtom::Small((5u128 << 64) | (0xabcd << 48)));
    assert_eq!(t.f, vec![0xabcd]);
    let d = yore(&ParsedAtom::Small(DA_2000));
    assert!(d.era);
    assert_eq!((d.y.clone(), d.m, d.t.d.clone()), (big(2000), 1, big(1)));
    for (y, leap) in [(2000, true), (1900, false), (2024, true), (2023, false)] {
        assert_eq!(is_leap_year(y), leap, "{y}");
    }
}

// ---------------------------------------------------------------------------
// @p / @q
// ---------------------------------------------------------------------------

#[test]
fn phonemic_literals_pin_values() {
    assert_eq!(sand("~zod"), ("p".into(), big(0)));
    assert_eq!(sand("~nec"), ("p".into(), big(1)));
    assert_eq!(sand("~fes"), ("p".into(), big(255)));
    assert_eq!(sand("~marzod"), ("p".into(), big(256)));
    assert_eq!(sand("~fipfes"), ("p".into(), big(0xffff)));
    // planets whose ++fe hits the legacy "arr == a" permutation
    assert_eq!(sand("~fipfes-loppeg"), ("p".into(), big(0x1_21d2)));
    assert_eq!(sand("~fipfes-hollyx"), ("p".into(), big(0x1_62d9)));
    assert_eq!(
        render_dime("p", ParsedAtom::Small(0x1_b8a7)),
        "~fipfes-rapryd"
    );
    // oversized names are not scrambled
    assert_eq!(
        sand("~marzod--dozzod-dozzod-dozzod-dozzod"),
        ("p".into(), big(0x0100u128 << 64))
    );
    assert_eq!(
        sand("~marnec--fipfes-fipfes-fipfes-fipfes--dozzod-dozzod-dozzod-marzod"),
        (
            "p".into(),
            (big(0x0101) << 128u32) + (big(u64::MAX as u128) << 64u32) + big(0x100)
        )
    );
}

#[test]
fn phonemic_knots_round_trip() {
    for text in [
        "~zod", "~nec", "~marzod", "~fipfes", "~doznec-dozzod", "~sampel-palnet", "~fipfes-fipfes",
        "~doznec-dozzod-dozzod-dozzod", "~sampel-palnet-sampel-palnet",
        "~marzod--dozzod-dozzod-dozzod-dozzod", "~sampel-palnet--sampel-palnet-sampel-palnet",
        "~doznec--dozzod-dozzod-dozzod-dozzod--dozzod-dozzod-dozzod-dozzod",
    ] {
        assert_knot_round_trip(text, "p");
    }
}

#[test]
fn q_knots_pin_values_and_round_trip() {
    assert_eq!(sand(".~zod"), ("q".into(), big(0)));
    assert_eq!(sand(".~nec"), ("q".into(), big(1)));
    assert_eq!(sand(".~marzod"), ("q".into(), big(0x100)));
    assert_eq!(sand(".~doznec-marzod"), ("q".into(), big(0x0001_0100)));
    for text in [
        ".~zod", ".~nec", ".~marzod", ".~nec-marzod", ".~nec-marzod-fipfes",
        ".~sampel-palnet-sampel-palnet",
    ] {
        assert_knot_round_trip(text, "q");
    }
    let (_, q) = nuck_dime(".~doznec-marzod");
    assert_eq!(render_dime("q", q), ".~nec-marzod");
}

#[test]
fn phonemic_rejections() {
    // ++tep forbids a 'doz' star prefix; ++hef forbids a zero leading word
    for text in ["dozzod", "dozzod-marzod", "zzz", "nec-", "bin"] {
        assert!(
            phonemic_name().parse(text).into_result().is_err(),
            "{text:?} should be rejected"
        );
    }
    assert!(tip().parse("zzz").into_result().is_err());
    assert!(tiq().parse("doz").into_result().is_err());
    assert_eq!(tip().parse("mar").into_result().ok(), Some(1));
    assert_eq!(tiq().parse("nec").into_result().ok(), Some(1));
    assert_eq!(hif().parse("marnec").into_result().ok(), Some(0x0101));
    assert_eq!(ins(b"fip"), Some(255));
    assert_eq!(ind(b"fes"), Some(255));
    assert_eq!(ins(b"zod"), None);
    assert_eq!(ind(b"doz"), None);
    assert_eq!(ins(b"do"), None);
    assert_eq!(ind(b"zodd"), None);
    assert_eq!(tos_po(1).to_biguint(), text_atom("mar"));
    assert_eq!(tod_po(0).to_biguint(), text_atom("zod"));
}

#[test]
fn fein_and_fynd_are_inverse() {
    // planets (32-bit) go through feis/tail; moons (64-bit) scramble the low
    // word only; everything else is left alone.
    for x in [0x1_0000u64, 0x1_0001, 0x1234_5678, 0xffff_ffff] {
        let y = fein(ParsedAtom::Small(x as u128)).to_u128().unwrap() as u64;
        assert!((0x1_0000..=0xffff_ffff).contains(&y));
        assert_eq!(fynd_u64(y), x, "fynd(fein({x:#x}))");
    }
    for x in [0x1_0000_0000u64, 0xdead_beef_1234_5678, u64::MAX] {
        let y = fein(ParsedAtom::Small(x as u128)).to_u128().unwrap() as u64;
        assert_eq!(y >> 32, x >> 32, "high word is untouched");
        assert_eq!(fynd_u64(y), x, "fynd(fein({x:#x}))");
    }
    assert_eq!(fein(ParsedAtom::Small(5)).to_u128(), Some(5));
    assert_eq!(fynd_u64(5), 5);
    let huge = ParsedAtom::Big(BigUint::one() << 70u32);
    assert_eq!(fein(huge.clone()).to_biguint(), huge.to_biguint());
    // Big-represented planets and moons take the BigUint arms
    let planet = ParsedAtom::Big(big(0x1234_5678));
    let moon = ParsedAtom::Big(big(0xdead_beef_1234_5678));
    assert_eq!(
        fein(planet).to_biguint(),
        fein(ParsedAtom::Small(0x1234_5678)).to_biguint()
    );
    assert_eq!(
        fein(moon).to_biguint(),
        fein(ParsedAtom::Small(0xdead_beef_1234_5678)).to_biguint()
    );
}

#[test]
fn feis_is_a_permutation_on_a_sample() {
    // Sweeps enough of the domain to hit the rare "arr == a" legacy arm of
    // ++fe (about one input in 65536), and checks tail undoes feis.
    for m in 0u64..200_000 {
        let c = feis(ParsedAtom::Small(m as u128)).to_u128().unwrap() as u64;
        assert!(c < 0xffff_0000);
        assert_eq!(fynd_u64(0x1_0000 + c), 0x1_0000 + m, "tail(feis({m}))");
    }
}

// ---------------------------------------------------------------------------
// @if / @is / @r* / @t / @ta / @c / blobs / many
// ---------------------------------------------------------------------------

#[test]
fn ip_literals_pin_values() {
    assert_eq!(sand(".1.2.3.4"), ("if".into(), big(0x0102_0304)));
    assert_eq!(sand(".255.255.255.255"), ("if".into(), big(0xffff_ffff)));
    assert_eq!(sand(".0.0.0.0.0.0.0.1"), ("is".into(), big(1)));
    assert_eq!(
        sand(".fe80.0.0.0.0.0.0.abcd"),
        ("is".into(), big((0xfe80u128 << 112) | 0xabcd))
    );
    for text in [".1.2.3.4", ".0.0.0.0", ".0.0.0.0.0.0.0.1", ".fe80.0.0.0.0.0.0.abcd"] {
        let (aura, q) = nuck_dime(text);
        assert_eq!(render_dime(&aura, q), text);
    }
    // an eight-group address that is not valid hex is not an @is
    assert!(zust().parse("1.2.3.4.5.6.7.g").into_result().is_err());
}

#[test]
fn float_knots_render() {
    assert_eq!(sand(".1.5"), ("rs".into(), big(0x3fc0_0000)));
    assert_eq!(sand(".~1.5"), ("rd".into(), big(0x3ff8_0000_0000_0000)));
    assert_eq!(sand(".~~1.5"), ("rh".into(), big(0x3e00)));
    assert_eq!(sand(".~~~1.5"), ("rq".into(), big(0x3fff_8000u128 << 96)));
    assert_eq!(sand(".inf"), ("rs".into(), big(0x7f80_0000)));
    assert_eq!(sand(".-inf"), ("rs".into(), big(0xff80_0000)));
    for (text, aura) in [
        (".1.5", "rs"),
        (".-1.5", "rs"),
        (".1e-10", "rs"),
        (".0.01", "rs"),
        (".0.1", "rs"),
        (".123.456", "rs"),
        (".~1.5", "rd"),
        (".~0.5", "rd"),
        (".~-2.25e5", "rd"),
        (".~~1.5", "rh"),
        (".~~~1.5", "rq"),
        (".inf", "rs"),
        (".-inf", "rs"),
        (".nan", "rs"),
    ] {
        let (got, q) = nuck_dime(text);
        assert_eq!(got, aura, "{text}");
        assert_eq!(render_dime(aura, q), text);
    }
    // below 1e-2 the exponent form wins
    let (_, q) = nuck_dime(".0.001");
    assert_eq!(render_dime("rs", q), ".1e-3");
}

#[test]
fn cord_like_knots_round_trip() {
    assert_knot_round_trip("~.foo-bar", "ta");
    assert_knot_round_trip("~.", "ta");
    assert_knot_round_trip("~~foo", "t");
    assert_knot_round_trip("~~foo.bar~~baz~.q", "t");
    assert_knot_round_trip("~~~41.", "t");
    assert_knot_round_trip("~~a~5f.b", "t");
    assert_knot_round_trip("~~a1-b0", "t");
    assert_knot_round_trip("~-foo", "c");
    assert_knot_round_trip("~-~2603.", "c");
    let (_, q) = nuck_dime("~~foo.bar");
    assert_eq!(q.to_biguint(), text_atom("foo bar"));
    assert_eq!(
        wood(&string_to_atom("a~b.c d_E".into())).to_biguint(),
        text_atom("a~~b~.c.d~5f.~45.")
    );
    assert_eq!(wood(&ParsedAtom::Small(0)).to_biguint(), big(0));
}

#[test]
fn blob_literals_cue_and_render() {
    // jam values computed with hoon ++jam
    let cases = [
        ("~02", atom(0)),
        ("~0c", atom(1)),
        ("~019", cell(atom(0), atom(0))),
        (
            "~014t5",
            cell(cell(atom(0), atom(0)), cell(atom(0), atom(0))),
        ),
        ("~04jo3jk1", cell(atom(12_345), atom(12_345))),
        ("~038i3h", cell(atom(1), cell(atom(2), atom(3)))),
        ("~012o861", cell(atom(16), atom(17))),
    ];
    for (text, noun) in cases {
        match nuck().parse(text).into_result() {
            Ok(Coin::Blob(got)) => assert_eq!(got, noun, "{text}"),
            other => panic!("{text}: {other:?}"),
        }
        assert_eq!(render(&Coin::Blob(noun)), text);
    }
    // a non-canonical jam (no backrefs) re-renders as the canonical one,
    // which here needs more than 128 bits
    let mut list = atom(0);
    for _ in 0..15 {
        list = cell(cell(atom(0), atom(0)), list);
    }
    match nuck().parse("~02kmiqb9d5kmiqb9d5kmiqb9d5").into_result() {
        Ok(Coin::Blob(got)) => assert_eq!(got, list),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        render(&Coin::Blob(list)),
        "~0kjcjcjcjcjcjcjcjcjcjcjcjcjcjd5"
    );
    // jock: a rock blob is one %rock, a sand blob splits into %sand leaves
    match parse_one("%~019") {
        Hoon::Rock(aura, NounExpr::Cell(_, _)) => assert_eq!(aura, "$"),
        other => panic!("{other:?}"),
    }
    match parse_one("~019") {
        Hoon::Pair(p, q) => {
            assert!(matches!(*p, Hoon::Sand(ref a, _) if a == "$"));
            assert!(matches!(*q, Hoon::Sand(ref a, _) if a == "$"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn many_knots_round_trip() {
    match nuck().parse("._1_2__").into_result() {
        Ok(Coin::Many(items)) => assert_eq!(
            items,
            vec![
                Coin::Dime("ud".into(), ParsedAtom::Small(1)),
                Coin::Dime("ud".into(), ParsedAtom::Small(2)),
            ]
        ),
        other => panic!("{other:?}"),
    }
    for text in ["._1_2__", "._foo_~~.bar__", "._~~.a~-b__", "._1_.~-2~-3~-~-__"] {
        let coin = nuck().parse(text).into_result().expect(text);
        assert_eq!(render(&coin), text);
    }
    assert_eq!(render(&Coin::Many(vec![])), ".__");
    assert_eq!(wack("a~b_c"), "a~~b~-c");
    assert_eq!(reap(3, 'x'), vec!['x', 'x', 'x']);
    assert!(trip(ParsedAtom::Small(0)).is_empty());
    assert_eq!(trip(string_to_atom("abc".into())), vec!["a", "b", "c"]);
    let wide = ParsedAtom::Big(BigUint::from_bytes_le(&[b'x'; 20]));
    assert_eq!(trip(wide).concat(), "x".repeat(20));
}

#[test]
fn knot_constants_through_cen() {
    match parse_one("%~zod") {
        Hoon::Rock(aura, NounExpr::ParsedAtom(q)) => {
            assert_eq!(aura, "p");
            assert!(q.is_zero());
        }
        other => panic!("{other:?}"),
    }
    match parse_one("%._1_2__") {
        Hoon::ColTar(items) => assert_eq!(items.len(), 2),
        other => panic!("{other:?}"),
    }
    match parse_one("._1_2__") {
        Hoon::ColTar(items) => assert_eq!(items.len(), 2),
        other => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------------------------
// paths
// ---------------------------------------------------------------------------

#[test]
fn path_segments_re_render_as_knots() {
    let items = path_items("/foo/1/0x10/~2020.1.1/.1.2.3.4/~zod/~s1/.y/~/0b101");
    let got: Vec<_> = items.iter().map(knot_of).collect();
    let want = [
        ("tas", "foo"),
        ("ta", "1"),
        ("ta", "0x10"),
        ("ta", "~2020.1.1"),
        ("ta", ".1.2.3.4"),
        ("ta", "~zod"),
        ("ta", "~s1"),
        ("ta", ".y"),
        ("ta", "~"),
        ("ta", "0b101"),
    ];
    assert_eq!(got.len(), want.len());
    for ((ga, gt), (wa, wt)) in got.iter().zip(want) {
        assert_eq!((ga.as_str(), gt.as_str()), (wa, wt));
    }
}

#[test]
fn path_elements_other_than_knots() {
    let items = path_items("/foo/[a]/(add 1 2)/$/'cord'");
    assert_eq!(items.len(), 5);
    assert!(matches!(items[2], Hoon::CenCol(_, ref args) if args.len() == 2));
    assert_eq!(knot_of(&items[3]), ("tas".into(), String::new()));
    assert_eq!(knot_of(&items[4]), ("t".into(), "cord".into()));
    // each extra fas inserts a %$ segment
    let got: Vec<_> = path_items("/foo//bar").iter().map(knot_of).collect();
    assert_eq!(
        got,
        vec![
            ("tas".into(), "foo".into()),
            ("tas".into(), String::new()),
            ("tas".into(), "bar".into()),
        ]
    );
    assert_eq!(path_items("/foo///bar").len(), 4);
}

#[test]
fn paths_relative_to_wer() {
    let wer = vec!["base".to_string(), "desk".to_string(), "file".to_string()];
    let knots = |src: &str| -> Vec<String> {
        match parse_with(src, wer.clone(), false).expect(src) {
            Hoon::TisSig(mut items) => match items.remove(0) {
                Hoon::ColSig(items) => items.iter().map(|h| knot_of(h).1).collect(),
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(knots("%/foo"), ["base", "desk", "file", "foo"]);
    assert_eq!(knots("%%"), ["base", "desk"]);
    assert_eq!(knots("%%%"), ["base"]);
    assert_eq!(knots("%%/bar"), ["base", "desk", "bar"]);
    assert_eq!(knots("/foo%/bar"), ["foo", "desk", "file", "bar"]);
    assert_eq!(knots("/=/foo"), ["base", "foo"]);
    assert_eq!(knots("/==/foo"), ["base", "desk", "foo"]);
    assert_eq!(knots("/=foo/bar"), ["base", "foo", "bar"]);
    // with no file path there is nothing to interpolate
    for src in ["/=/foo", "%/foo", "%%"] {
        assert!(parse_with(src, vec![], false).is_err(), "{src}");
    }
}

// ---------------------------------------------------------------------------
// bit helpers
// ---------------------------------------------------------------------------

#[test]
fn met_counts_bloqs() {
    assert_eq!(met(0, &ParsedAtom::Small(0)), 1);
    assert_eq!(met(3, &ParsedAtom::Big(BigUint::zero())), 1);
    assert_eq!(met(0, &ParsedAtom::Small(0x80)), 8);
    assert_eq!(met(3, &ParsedAtom::Small(0x1_0000)), 3);
    assert_eq!(met(0, &ParsedAtom::Big(BigUint::one() << 200u32)), 201);
    assert_eq!(met(4, &ParsedAtom::Big(BigUint::one() << 200u32)), 13);
}

#[test]
fn rep_and_rap_assemble() {
    assert_eq!(rep(3, None, &[]).to_biguint(), big(0));
    assert_eq!(
        rep(3, Some(0), &[ParsedAtom::Small(1)]).to_biguint(),
        big(0)
    );
    assert_eq!(
        rep(3, None, &[ParsedAtom::Small(0x1ff), ParsedAtom::Small(2)]).to_biguint(),
        big(0x02ff)
    );
    assert_eq!(
        rep(
            4,
            Some(2),
            &[ParsedAtom::Small(0x1_2345_6789), ParsedAtom::Small(1)]
        )
        .to_biguint(),
        big(0x1_2345_6789)
    );
    // chunks of 128 and 256 bits
    let wide = ParsedAtom::Big((BigUint::one() << 130u32) + 5u32);
    assert_eq!(
        rep(7, None, &[wide.clone(), ParsedAtom::Small(1)]).to_biguint(),
        (BigUint::one() << 128u32) + 5u32
    );
    assert_eq!(
        rep(8, None, &[wide.clone(), ParsedAtom::Small(1)]).to_biguint(),
        (BigUint::one() << 256u32) + wide.to_biguint()
    );
    assert_eq!(rap(3, &[]).to_biguint(), big(0));
    assert_eq!(
        rap(3, &[0x61, 0x62, 0x6263]).to_biguint(),
        text_atom("abcb")
    );
    assert_eq!(
        rap(6, &[u128::MAX, 1]).to_biguint(),
        (BigUint::one() << 129u32) - 1u32
    );
}

#[test]
#[should_panic(expected = "bloq must be < 7")]
fn rap_rejects_oversized_bloq() {
    rap(7, &[1]);
}

#[test]
fn cut_edges() {
    let x = ParsedAtom::Small(0xdead_beef);
    assert_eq!(cut(3, 0, 0, &x).to_biguint(), big(0));
    assert_eq!(cut(64, 0, 1, &x).to_biguint(), big(0));
    assert_eq!(cut(3, usize::MAX, 1, &x).to_biguint(), big(0));
    assert_eq!(cut(3, 0, usize::MAX, &x).to_biguint(), big(0));
    assert_eq!(cut(3, 0, 1, &ParsedAtom::Small(0)).to_biguint(), big(0));
    assert_eq!(cut(3, 4, 1, &x).to_biguint(), big(0));
    assert_eq!(cut(3, 1, 2, &x).to_biguint(), big(0xadbe));
    assert_eq!(
        cut(7, 0, 1, &ParsedAtom::Small(u128::MAX)).to_biguint(),
        big(u128::MAX)
    );
    let b = (BigUint::one() << 200u32) + (BigUint::from(0xabcdu32) << 64u32) + 0x1234u32;
    let bb = ParsedAtom::Big(b.clone());
    assert_eq!(cut(3, 0, 2, &bb).to_biguint(), big(0x1234));
    assert_eq!(
        cut(3, 0, 16, &bb).to_biguint(),
        big((0xabcd << 64) | 0x1234)
    );
    assert_eq!(cut(3, 0, 26, &bb).to_biguint(), b);
    assert_eq!(cut(3, 8, 2, &bb).to_biguint(), big(0xabcd));
    assert_eq!(cut(0, 200, 1, &bb).to_biguint(), big(1));
    assert_eq!(cut(0, 201, 1, &bb).to_biguint(), big(0));
    assert_eq!(lsh(3, usize::MAX, &x).to_biguint(), big(0));
    assert_eq!(rsh(3, usize::MAX, &x).to_biguint(), big(0));
    assert_eq!(lsh(3, 1, &x).to_biguint(), big(0x00de_adbe_ef00));
    assert_eq!(rsh(3, 1, &x).to_biguint(), big(0x00de_adbe));
}

#[test]
fn float_helpers_reach_the_bignum_shifts() {
    // @rs parameters as drg_fl passes them: p=24, v=@s -150, w=@s -127
    let (p, v, w) = (24u128, 299u128, 253u128);
    let magnitude = |f: &BinaryFloat| -> f64 {
        match f {
            BinaryFloat::Finite { exp, mant, .. } => {
                mant.to_f64().unwrap() * 2f64.powi(si(*exp) as i32)
            }
            other => panic!("{other:?}"),
        }
    };
    // an exact product needs no rounding bits (end_big over zero bits)
    let nine = binaryfloat_mul_internal(0, big(3), 0, big(3), p, v, w, 'n', 'd');
    assert_eq!(magnitude(&nine), 9.0);
    let odd = big((1 << 30) + 1);
    let wide = binaryfloat_mul_internal(0, odd.clone(), 0, odd, p, v, w, 'n', 'd');
    let exact = ((1u64 << 30) + 1) as f64;
    assert!((magnitude(&wide) / (exact * exact) - 1.0).abs() < 1e-6);
    // drg: shortest decimal digits for 3*2^-1, 3*2^1 and 2^23 (e = 0)
    let dec = |(e, a): (u128, BigUint)| -> f64 { a.to_f64().unwrap() * 10f64.powi(si(e) as i32) };
    assert_eq!(dec(drg(1, big(3), p, v, w, 'd')), 1.5);
    assert_eq!(dec(drg(2, big(3), p, v, w, 'd')), 6.0);
    assert_eq!(dec(drg(0, big(1 << 23), p, v, w, 'd')), (1u64 << 23) as f64);
}

// ---------------------------------------------------------------------------
// jam / cue
// ---------------------------------------------------------------------------

#[test]
fn jam_matches_hoon_and_cue_inverts() {
    assert_eq!(jam_simple(atom(0)).to_biguint(), big(2));
    assert_eq!(jam_simple(atom(1)).to_biguint(), big(12));
    assert_eq!(jam_simple(cell(atom(0), atom(0))).to_biguint(), big(41));
    assert_eq!(
        jam_simple(cell(atom(12_345), atom(12_345))).to_biguint(),
        big(4_957_785_729)
    );
    let big_atom = NounExpr::ParsedAtom(ParsedAtom::Big((BigUint::one() << 300u32) + 7u32));
    let nouns = vec![
        atom(0),
        atom(u128::MAX),
        cell(cell(atom(1), atom(2)), cell(atom(1), atom(2))),
        cell(big_atom.clone(), cell(big_atom.clone(), atom(3))),
        cell(
            atom(0xdead_beef),
            cell(atom(0xdead_beef), atom(0xdead_beef)),
        ),
        big_atom.clone(),
    ];
    for noun in nouns {
        let jammed = jam_simple(noun.clone());
        assert_eq!(cue_simple(jammed.clone()).expect("cue of jam"), noun);
        // cue accepts the same value in either representation
        let as_big = ParsedAtom::Big(jammed.to_biguint());
        assert_eq!(cue_simple(as_big).expect("cue of Big"), noun);
    }
}

#[test]
fn cue_rejects_malformed_input() {
    let bad = |bits: &[bool]| cue_simple(atom_from_bits(bits));
    // all-zero: no size terminator
    assert!(cue_simple(ParsedAtom::Small(0)).is_err());
    // backref (tag 11) to offset 0, which is still being decoded
    assert!(cue_simple(ParsedAtom::Small(0b111)).is_err());
    // backref whose size prefix claims more than 64 bits
    let mut wide_ref = vec![true, true];
    wide_ref.extend([false; 8]);
    wide_ref.push(true);
    assert!(bad(&wide_ref).is_err());
    // atom payload running past the 128-bit window (hoonc's cue rejects ~0g0
    // too)
    assert!(cue_simple(ParsedAtom::Small(512)).is_err());
    assert!(twid().parse("0g0").into_result().is_err());
    // size field running past the end
    assert!(cue_simple(ParsedAtom::Small(1u128 << 127)).is_err());
    // a cell whose head ends exactly at bit 128 has no tail
    let mut head = vec![true, false, false];
    head.extend(mat_bits_of(&(BigUint::one() << 110u32)));
    assert_eq!(head.len(), 128);
    assert!(bad(&head).is_err());
    // a tail tag at the last bit with nothing after it
    let mut tag = vec![true, false, false];
    tag.extend(mat_bits_of(&(BigUint::one() << 109u32)));
    assert_eq!(tag.len(), 127);
    tag.push(true);
    assert!(bad(&tag).is_err());
    // a backref whose 16-bit offset runs past the window
    let mut short = vec![true, false, false];
    short.extend(mat_bits_of(&(BigUint::one() << 94u32)));
    assert_eq!(short.len(), 112);
    short.extend([true, true, false, false, false, false, false, true]);
    short.extend([false; 4]);
    assert_eq!(short.len(), 124);
    assert!(bad(&short).is_err());
}

// ---------------------------------------------------------------------------
// tall-form builders and traces
// ---------------------------------------------------------------------------

#[test]
fn closed_tall_builders() {
    let spec = just('*').to(Spec::Base(BaseType::NounExpr));
    let hoon = just('x').to(Hoon::ZapZap);
    let two = two_specs_closed_tall(spec.clone())
        .parse("  *  *\n  ==")
        .into_result()
        .expect("two specs");
    assert_eq!(
        two,
        (
            Spec::Base(BaseType::NounExpr),
            Spec::Base(BaseType::NounExpr)
        )
    );
    let named = name_spec_closed_tall(spec.clone())
        .parse("  foo  *==")
        .into_result()
        .expect("name spec");
    assert_eq!(named, ("foo".to_string(), Spec::Base(BaseType::NounExpr)));
    let one = one_hoon_closed_tall(hoon)
        .parse("=  x  =")
        .into_result()
        .expect("one hoon");
    assert_eq!(one, Hoon::ZapZap);
    assert!(two_specs_closed_tall(spec)
        .parse("  *  *")
        .into_result()
        .is_err());
}

#[test]
fn hoon_with_span_wraps_once() {
    let src = "!!\n!!\n";
    let linemap = Arc::new(LineMap::new(src));
    let wer = vec!["p3".to_string()];
    let first = hoon_with_span(Hoon::ZapZap, (0, 2), &wer, &linemap);
    let spot = match &first {
        Hoon::Dbug(spot, inner) => {
            assert_eq!(**inner, Hoon::ZapZap);
            spot.clone()
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(spot.p, wer);
    assert_eq!(spot.q.p, (1, 1));
    // the same span is idempotent
    assert_eq!(hoon_with_span(first.clone(), (0, 2), &wer, &linemap), first);
    // a different span nests
    match hoon_with_span(first.clone(), (3, 5), &wer, &linemap) {
        Hoon::Dbug(outer, inner) => {
            assert_ne!(outer, spot);
            assert_eq!(*inner, first);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn attach_help_is_idempotent() {
    let help = NounExpr::ParsedAtom(string_to_atom("doc".into()));
    let once = attach_help_to_hoon(Hoon::ZapZap, help.clone());
    let twice = attach_help_to_hoon(once.clone(), help.clone());
    assert_eq!(once, twice);
    let other = attach_help_to_hoon(once.clone(), atom(1));
    assert_ne!(other, once);
    let spec = attach_help_to_spec(Spec::Base(BaseType::Null), help.clone());
    assert_eq!(attach_help_to_spec(spec.clone(), help), spec);
}

fn dbug_spots(h: &Hoon, out: &mut Vec<Spot>) {
    match h {
        Hoon::Dbug(spot, inner) => {
            out.push(spot.clone());
            dbug_spots(inner, out);
        }
        Hoon::Note(_, inner) => dbug_spots(inner, out),
        _ => {}
    }
}

#[test]
fn traced_parses_of_doc_heavy_shapes() {
    // Trace-on parses of the shapes in p3_spot_*.hoon. The exact spots are
    // pinned by those parity probes; here every source must parse with a
    // traced top node and well-ordered spots.
    let sources = [
        concat!(
            "|%\n", "++  gen\n", "  ::  arm-level docs\n", "  ::\n", "  ::  second paragraph\n",
            "  ^-  @\n", "  0\n", "++  bar  ::  inline arm doc\n", "  1\n", "+$  mol\n",
            "  ::  a mold\n", "  @ud\n", "++  $\n", "  ::  buc arm\n", "  4\n", "++  qux\n",
            "  ::  doc then equals\n", "  =+  x=3\n", "  x\n", "--\n",
        ),
        concat!(
            "::  file doc\n", "=/  b  1\n", "::  plain doc\n", "=/  c  2\n", "::    larg one\n",
            "::    larg two\n", "=/  d  3\n", "::\n", "::    larg after blank\n", "=/  e  4\n",
            "::  +f: names the binder\n", "=/  f  5\n", "\n", "::  after blank line\n",
            "=/  g  6\n", "[b c d e f g]\n",
        ),
        concat!(
            "|=  [a=* d=*]\n", "=*  b\n", "  ::  doc on =* value\n", "  a\n", ":*  ?~  d\n",
            "      ::  yes\n", "      0\n", "    ::  no\n", "    1\n", "  ::\n", "    ?-  d\n",
            "      ::  first clause\n", "      @  6\n", "      ^  7\n", "    ==\n", "  ::\n",
            "    ?+  d\n", "      ::  default\n", "      8\n", "      ::  clause\n",
            "      @  9\n", "    ==\n", "  ::\n", "    ~>  %foo.16\n", "    17\n", "  ::\n",
            "    ~>  %foo\n", "    18\n", "  ::\n", "    .^(@ %a /foo)\n", "  ::\n", "    b\n",
            "==\n",
        ),
        concat!("|_  s=@\n", "+*  t\n", "  ::  alias doc\n", "  s\n", "++  get  t\n", "--\n",),
        "!:((add 1 2))",
        concat!(
            "|=  d=*\n", "?~  d\n", "  ::    larg doc on a colhep child\n", "  :-  1  2\n",
            "::    larg doc on the no branch\n", "[3 4]\n",
        ),
        "/=  foo  /lib/foo\n!:(1)",
        "|=  a=*\n?~  a  0  1",
        "!:\n/=  foo  /lib/foo\n1",
    ];
    for src in sources {
        for docs in [false, true] {
            let parsed = parse_with_docs(src, vec!["p3".into(), "trace".into()], true, docs)
                .unwrap_or_else(|e| panic!("{src}\n{e}"));
            let Hoon::TisSig(items) = parsed else {
                panic!("expected TisSig");
            };
            let mut spots = Vec::new();
            dbug_spots(&items[0], &mut spots);
            assert!(!spots.is_empty(), "{src}: top node should be traced");
            for spot in spots {
                assert!(spot.q.p <= spot.q.q, "{src}: spot {spot:?}");
            }
        }
    }
}

fn strip_wrappers(h: &Hoon) -> &Hoon {
    match h {
        Hoon::Note(_, inner) | Hoon::Dbug(_, inner) => strip_wrappers(inner),
        other => other,
    }
}

#[test]
fn postfix_docs_attach_to_tall_micsig_args() {
    // apply_hoon_docs: a postfix doc on a tall ;~ line goes to the gate when
    // it is the only token, otherwise to the last argument on the line; a
    // doc after the closing == names no argument.
    let src = concat!(
        "|%\n", "++  a  ;~  plug  (just 'a')  (just 'b')  ::  two rules\n", "       ==\n",
        "++  b  ;~  plug  (just 'a')  ==  ::  closed on one line\n",
        "++  c  ;~  plug  ::  just the gate\n", "         (just 'a')\n", "       ==\n", "--\n",
    );
    let parsed = parse_with_docs(src, vec!["p3".into(), "micsig".into()], false, true)
        .unwrap_or_else(|e| panic!("{e}"));
    let Hoon::TisSig(items) = parsed else {
        panic!("expected TisSig");
    };
    let Hoon::BarCen(_, tomes) = strip_wrappers(&items[0]) else {
        panic!("expected a core");
    };
    let arms = &tomes.get("$").expect("default chapter").1;
    let arm_json = |name: &str| serde_json::to_string(arms.get(name).expect(name)).unwrap();
    assert!(arm_json("a").contains("Help"), "{}", arm_json("a"));
    assert!(arm_json("c").contains("Help"), "{}", arm_json("c"));
    let _ = arm_json("b");
}

// ---------------------------------------------------------------------------
// noun printing and diffing (test diagnostics)
// ---------------------------------------------------------------------------

#[test]
fn print_and_diff_nouns() {
    let mut slab: NounSlab = NounSlab::new();
    let pair = T(&mut slab, &[D(1), D(2)]);
    let nested = T(&mut slab, &[pair, D(3)]);
    let deep = T(&mut slab, &[D(4), nested]);
    let other = T(&mut slab, &[D(1), D(5)]);
    let other_nested = T(&mut slab, &[other, D(3)]);
    let other_tail = T(&mut slab, &[pair, D(9)]);
    let head_atom_diff = T(&mut slab, &[D(5), D(2)]);
    let tail_pair = T(&mut slab, &[D(3), pair]);
    let tail_other = T(&mut slab, &[D(3), other]);
    let dbug_tag = D(u64::from_le_bytes(*b"dbug\0\0\0\0"));
    let wrapped = T(&mut slab, &[dbug_tag, D(0), nested]);
    let bad_dbug = T(&mut slab, &[dbug_tag, D(7)]);
    let cell_head = T(&mut slab, &[pair, pair]);
    let space = slab.noun_space();
    let h = |n: Noun| n.in_space(&space);

    let leaf = print_noun(h(D(7)), 4, 0);
    assert!(!leaf.contains('['), "{leaf}");
    let flat = print_noun(h(pair), 4, 0);
    assert!(flat.starts_with('[') && !flat.contains('\n'), "{flat}");
    let tall = print_noun(h(deep), 4, 0);
    assert!(tall.contains('\n'), "{tall}");
    assert!(print_noun(h(deep), 1, 0).contains("..."));

    let mut printed = false;
    assert!(diff_noun(h(nested), h(nested), &mut printed).is_ok());
    assert!(!printed);
    // %dbug wrappers are skipped; malformed ones are compared as-is
    assert!(diff_noun(h(wrapped), h(nested), &mut printed).is_ok());
    assert!(diff_noun(h(bad_dbug), h(bad_dbug), &mut printed).is_ok());
    assert!(diff_noun(h(cell_head), h(cell_head), &mut printed).is_ok());
    // head mismatch found one level down (printed there)
    let mut printed = false;
    assert!(diff_noun(h(nested), h(other_nested), &mut printed).is_err());
    assert!(printed);
    // head mismatch at this level
    let mut printed = false;
    assert!(diff_noun(h(pair), h(head_atom_diff), &mut printed).is_err());
    assert!(printed);
    // tail mismatch at this level
    let mut printed = false;
    assert!(diff_noun(h(nested), h(other_tail), &mut printed).is_err());
    assert!(printed);
    // tail mismatch found one level down
    let mut printed = false;
    assert!(diff_noun(h(tail_pair), h(tail_other), &mut printed).is_err());
    assert!(printed);
    // atom vs cell
    let mut printed = false;
    assert!(diff_noun(h(D(1)), h(pair), &mut printed).is_err());
    diff_and_report(h(nested), h(nested));
    diff_and_report(h(nested), h(other_nested));
}

// ---------------------------------------------------------------------------
// %dbug span heuristics (chumsky_spot_to_hoon_spot via hoon_with_span)
// ---------------------------------------------------------------------------

/// Start of the %dbug spot hatch assigns to a node spanning
/// `[raw, end)` of `src` (docs on, as honk parses).
fn spot_start(src: &str, raw: usize, end: usize) -> (u64, u64) {
    let linemap = Arc::new(LineMap::new_with_docs(src, true));
    let wer = vec!["p3".to_string()];
    match hoon_with_span(Hoon::ZapZap, (raw, end), &wer, &linemap) {
        Hoon::Dbug(spot, _) => spot.q.p,
        other => panic!("{other:?}"),
    }
}

fn at(src: &str, needle: &str) -> usize {
    src.find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {src:?}"))
}

#[test]
fn spans_starting_at_arm_headers() {
    // arm_body_start_from_header: a span that starts at `++ name` moves to
    // the body, stopping at an `=` rune right after a doc block.
    let src = "++  foo\n  ::  doc\n  =/  a  1\n  a\n";
    assert_eq!(spot_start(src, 0, src.len()), (3, 3));
    let src = "  ++  foo\n    ::  doc\n    =/  a  1\n    a\n";
    assert_eq!(spot_start(src, 0, src.len()), (3, 5));
    // body right after the name, no docs
    let src = "++  foo\n  7\n";
    let (line, _) = spot_start(src, 0, src.len());
    assert_eq!(line, 2);
    // `++  $` names the empty arm
    let src = "++  $\n  ::  buc doc\n  4\n";
    let (line, _) = spot_start(src, 0, src.len());
    assert!(line >= 2);
    // `+$` headers and a doc line without a space after `::`
    let src = "+$  mol\n  ::x\n  @ud\n";
    assert!(spot_start(src, 0, src.len()).0 >= 2);
    // an empty `::` line, then a doc, then code
    let src = "++  bar\n  ::\n  ::  after blank\n  2\n";
    assert!(spot_start(src, 0, src.len()).0 >= 2);
    // a `:` that is not a comment ends the header scan
    let src = "++  foo  :-(1 2)\n";
    assert_eq!(spot_start(src, 0, src.len()), (1, 10));
    // spans ending inside the header or the doc block
    let src = "++  foo  1";
    assert_eq!(spot_start(src, 0, 7), (1, 1));
    let src = "++  foo\n  ::  doc   ";
    assert_eq!(spot_start(src, 0, src.len()).0, 2);
    let src = "++  foo\n  ::   ";
    assert_eq!(spot_start(src, 0, src.len()), (1, 1));
    // headers that are not arms
    for src in ["++", "++\tfoo\n  1\n", "\t++  foo\n  1\n", "+-  foo\n  1\n", "++  (\n"] {
        let (line, col) = spot_start(src, 0, src.len());
        assert!(line >= 1 && col >= 1, "{src:?}");
    }
    // a span that starts after its own end, or at the end of the source
    let src = "1\n2\n";
    assert_eq!(spot_start(src, 2, 1), (2, 1));
    assert_eq!(spot_start(src, 4, 4).0, 3);
    assert_eq!(spot_start("   ", 0, 3), (1, 4));
}

#[test]
fn spans_starting_at_doc_blocks() {
    // non_doc_start_after_leading_doc_span
    // plain docs are skipped to the code line
    let src = "=/  a  1\n::  plain\n2\n";
    assert_eq!(spot_start(src, at(src, "::"), src.len()).0, 3);
    // a larg (4-space) doc line anchors the span, walking back over the
    // equally indented larg lines above it
    let src = "x\n::    one\n::    two\n2\n";
    assert_eq!(spot_start(src, at(src, "::    two"), src.len()), (2, 1));
    let src = "::    first\n::    second\n2\n";
    assert_eq!(spot_start(src, at(src, "::    second"), src.len()), (1, 1));
    let src = "::    only\n2\n";
    assert_eq!(spot_start(src, 0, src.len()), (1, 1));
    // the walk stops at a blank line, a shallower doc, a different indent, a
    // non-doc line and a doc with a different indent (the gap walk-back in
    // expand_gap_start may still pick an earlier larg line)
    for (src, marker, want) in [
        ("x\n\n::    larg\n2\n", "::    larg", Some((3, 1))),
        ("x\n::\n::    larg\n2\n", "::    larg", Some((3, 1))),
        ("x\n  ::    a\n::    b\n2\n", "::    b", Some((2, 3))),
        (":-  1  2\n::    b\n2\n", "::    b", Some((2, 1))),
        ("x\n::  two\n::    four\n2\n", "::    four", None),
        ("x\n::      six\n::    four\n2\n", "::    four", None),
    ] {
        let raw = at(src, marker);
        let got = spot_start(src, raw, src.len());
        let raw_line = src[..raw].matches('\n').count() as u64 + 1;
        assert!(got.0 >= 1 && got.0 <= raw_line, "{src:?}: {got:?}");
        if let Some(want) = want {
            assert_eq!(got, want, "{src:?}");
        }
    }
    // blank lines and a non-comment `:` line inside the block
    let src = "::  a\n   \n:-  1  2\n";
    assert_eq!(spot_start(src, 0, src.len()).0, 3);
    // docs that run to the end of the source, or only have spaces
    for src in ["::  a\n", "::  a", "::    \n1\n", "::\n1\n"] {
        let (line, col) = spot_start(src, 0, src.len());
        assert!(line >= 1 && col >= 1, "{src:?}");
    }
    // leading whitespace before the doc, and whitespace up to the end
    let src = "  \n  ::  a\n  1\n";
    assert!(spot_start(src, 0, src.len()).0 >= 2);
    let src = "1\n  \n  ";
    assert!(spot_start(src, 2, src.len()).0 >= 2);
    // the code line is past the span end
    let src = "::  a\n1\n";
    assert_eq!(spot_start(src, 0, 5).0, 1);
    // a smol link doc that the gap walk-back anchors on keeps the code start
    let src = "x\n::  +foo: link\n1\n";
    assert_eq!(spot_start(src, at(src, "::"), src.len()), (3, 1));
    // a span starting inside a comment line snaps to the comment
    let src = "::  x =/  b  2";
    assert_eq!(spot_start(src, at(src, "=/"), src.len()), (1, 1));
}

#[test]
fn spans_after_plain_docs_between_tisfas() {
    // skip_plain_doc_before_equals_slash_start: a plain doc between two =/
    // binders is not the second binder's anchor; a +link naming the binder,
    // a larg doc, or a non-doc line keeps the walked-back start.
    let cases = [
        "=/  a  1\n::  plain\n=/  b  2\nb\n", "=/  a  1\n::  +b: names it\n=/  b  2\nb\n",
        "=/  a  1\n::    larg\n=/  b  2\nb\n", "=/  a  1\n::  one\n\n::  two\n=/  b  2\nb\n",
        "=/  a  1\n::\n::  after empty\n=/  b  2\nb\n", "=/  a  1\n::  \n=/  b  2\nb\n",
        "=/  a  1\n::  plain\n=/  [b c]  [2 3]\nb\n", "=/  a  1\n::  +c: another\n=/  b  2\nb\n",
        "=+  a  1\n::  plain\n=/  b  2\nb\n", "  =/  a  1\n::  plain\n  =/  b  2\n  b\n",
        "=/  a  1\n  \n::  plain\n=/  b  2\nb\n", "::  plain\n=/  b  2\nb\n",
        "=/  a  1\n::  plain\n=/  b", "=/  a  1\n::  plain\n=/", "=/  a  1\n::  plain\n  ",
        "=/  a  1\n::  +c: x\n   ", "=/  a  1\n::  +c: x\n=/", "=/  a  1\n::  +c: x\n=/  b",
        "=/  a  1  ::    larg trailing\n=/  b  2\nb\n", "x\n\n::    larg\n=/  b  2\nb\n",
        "=+  a  1\n::    larg\n=/  b  2\nb\n", "=/  a  1\n::  +c: x\n\n=/  b  2\nb\n",
        "=/  a  1\nfoo  ::  +c: x\n=/  b  2\nb\n", "=/  a  1\n:-  1  ::  +c: x\n=/  b  2\nb\n",
        "=/  a  1\n::  +c: x\n::\n=/  b  2\nb\n", "=/  a  1\n::  +c: x\n::  \n=/  b  2\nb\n",
        "=/  a  1\n::  +c: x\n=/  [b c]  [2 3]\nb\n", "  =/  a  1\n  ::  +c: x\n  =/  b  2\n  b\n",
    ];
    for src in cases {
        let raw = if src.ends_with("   ") {
            src.rfind('\n').unwrap() + 1
        } else {
            src.rfind("=/").unwrap()
        };
        let (line, col) = spot_start(src, raw, src.len());
        let raw_line = src[..raw].matches('\n').count() as u64 + 1;
        assert!(
            line >= 1 && line <= raw_line && col >= 1,
            "{src:?}: {line}:{col}"
        );
    }
    // the plain-doc case lands back on the binder itself
    let src = "=/  a  1\n::  plain\n=/  b  2\nb\n";
    assert_eq!(spot_start(src, at(src, "=/  b"), src.len()), (3, 1));
    // a smol link naming the binder anchors at the doc
    let src = "=/  a  1\n::  +b: names it\n=/  b  2\nb\n";
    assert_eq!(spot_start(src, at(src, "=/  b"), src.len()), (2, 1));
    // so does a larg doc
    let src = "=/  a  1\n::    larg\n=/  b  2\nb\n";
    assert_eq!(spot_start(src, at(src, "=/  b"), src.len()), (2, 1));
    // a smol link naming something else is skipped like a plain doc (hoonc
    // anchors at the doc: divergent/p3_spot_doc_link_before_tisfas.hoon)
    let src = "=/  a  1\n::  +c: another\n=/  b  2\nb\n";
    assert_eq!(spot_start(src, at(src, "=/  b"), src.len()), (3, 1));
}
