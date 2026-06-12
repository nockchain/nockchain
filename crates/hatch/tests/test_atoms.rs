use std::sync::Arc;

use chumsky::Parser;
use hatch::native_parser;
use hatch::utils::{hoon_to_noun, LineMap};
use ibig::UBig;
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockvm::ext::noun_equality;
use nockvm::noun::{Atom, Noun, NounAllocator, D, T};
use nockvm_macros::tas;

fn slab_noun_equality<J>(slab: &NounSlab<J>, left: &Noun, right: &Noun) -> bool {
    let space = slab.noun_space();
    noun_equality(left.in_space(&space), right.in_space(&space))
}

// @ud
#[test]
fn test_ud_zero() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("0")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);
    let atom = T(&mut slab, &[D(tas!(b"sand")), D(tas!(b"ud")), D(0)]);
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @ud
#[test]
fn test_ud_leading_zero() {
    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("00")
        .into_result();
    assert!(res.is_err(), "Expected Err, got Ok");
}

// @ud
#[test]
fn test_failure_ud_without_dots() {
    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("1000")
        .into_result();
    assert!(res.is_err(), "Expected Err, got Ok");
}

// @ud
#[test]
fn test_decimal_with_dots() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("31.415.926.535.897")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);
    let atom = T(
        &mut slab,
        &[D(tas!(b"sand")), D(tas!(b"ud")), D(31415926535897)],
    );
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @r
#[test]
fn test_long_float() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse(".~~~9999999999999999999999999.999999999999999999999999999999999999999999")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let num_str = "85496536484895936903754622060608880640";
    let ubig = UBig::from_str_radix(num_str, 10).expect("test decimal literal should parse");
    let big_atom = Atom::from_ubig(&mut slab, &ubig).as_noun();

    let atom = T(&mut slab, &[D(tas!(b"sand")), D(tas!(b"rq")), big_atom]);
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @rh
#[test]
fn test_rh_float() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse(".~~3.14")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);
    let atom = T(&mut slab, &[D(tas!(b"sand")), D(tas!(b"rh")), D(16968)]);
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @rs
#[test]
fn test_rs_float() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse(".3.141592653589793")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);
    let atom = T(
        &mut slab,
        &[D(tas!(b"sand")), D(tas!(b"rs")), D(1078530011)],
    );
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @rd
#[test]
fn test_rd_float() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse(".~3.141592653589793")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);
    let atom = T(
        &mut slab,
        &[D(tas!(b"sand")), D(tas!(b"rd")), D(4614256656552045848)],
    );
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @rq
#[test]
fn test_rq_float() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse(".~~~3.141592653589793")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let num_str = "85073555474209096225796870687635750373";
    let ubig = UBig::from_str_radix(num_str, 10).expect("test decimal literal should parse");
    let big_atom = Atom::from_ubig(&mut slab, &ubig).as_noun();

    let atom = T(&mut slab, &[D(tas!(b"sand")), D(tas!(b"rq")), big_atom]);
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @c
#[test]
fn test_utf32() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("~-~45fed.")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let atom = T(&mut slab, &[D(tas!(b"sand")), D(tas!(b"c")), D(286701)]);
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @da
#[test]
fn test_absolute_date() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("~2018.5.14..22.31.46..1435")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let num_str = "170141184503308117923547626898177654784";
    let ubig = UBig::from_str_radix(num_str, 10).expect("test decimal literal should parse");
    let big_atom = Atom::from_ubig(&mut slab, &ubig).as_noun();

    let atom = T(&mut slab, &[D(tas!(b"sand")), D(tas!(b"da")), big_atom]);
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @dr
#[test]
fn test_relative_date() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("~h5.m30.s12")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let num_str = "365466893588333636616192";
    let ubig = UBig::from_str_radix(num_str, 10).expect("test decimal literal should parse");
    let big_atom = Atom::from_ubig(&mut slab, &ubig).as_noun();

    let atom = T(&mut slab, &[D(tas!(b"sand")), D(tas!(b"dr")), big_atom]);
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @ub
#[test]
fn test_binary_number() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("0b11.1000")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let atom = T(&mut slab, &[D(tas!(b"sand")), D(tas!(b"ub")), D(56)]);
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @ub
#[test]
fn test_failure_binary_without_dots() {
    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("0b0111000")
        .into_result();
    assert!(res.is_err(), "Expected Err, got Ok");
}

// @ux
#[test]
fn test_hexadecimal_number() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("0x84.5fed")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let atom = T(&mut slab, &[D(tas!(b"sand")), D(tas!(b"ux")), D(8675309)]);
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @ux
#[test]
fn test_hexadecimal_with_gap_number() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("0x84.  5fed")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let atom = T(&mut slab, &[D(tas!(b"sand")), D(tas!(b"ux")), D(8675309)]);
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @uw
#[test]
fn test_base64_number() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("0wx5~J.abcde")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let atom = T(
        &mut slab,
        &[D(tas!(b"sand")), D(tas!(b"uw")), D(9315042280129358)],
    );
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @uv
#[test]
fn test_base32_number() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("0v88nvd")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let atom = T(&mut slab, &[D(tas!(b"sand")), D(tas!(b"uv")), D(8675309)]);
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @ui
#[test]
fn test_ui_number() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("0i123456789")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let atom = T(&mut slab, &[D(tas!(b"sand")), D(tas!(b"ui")), D(123456789)]);
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @uc
#[test]
fn test_btc_address() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("0c1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let num_str = "564677839311255888116953130470068734144382799640";
    let ubig = UBig::from_str_radix(num_str, 10).expect("test decimal literal should parse");
    let big_atom = Atom::from_ubig(&mut slab, &ubig).as_noun();

    let atom = T(&mut slab, &[D(tas!(b"sand")), D(tas!(b"uc")), big_atom]);
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @is
#[test]
fn test_ipv6_address() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse(".0.0.0.0.0.1c.c3c6.8f5a")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let atom = T(
        &mut slab,
        &[D(tas!(b"sand")), D(tas!(b"is")), D(123543654234)],
    );
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @if
#[test]
fn test_ipv4_address() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse(".192.168.1.1")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let atom = T(
        &mut slab,
        &[D(tas!(b"sand")), D(tas!(b"if")), D(3232235777)],
    );
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @tas
#[test]
fn test_term() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("%nock")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let atom = T(
        &mut slab,
        &[D(tas!(b"rock")), D(tas!(b"tas")), D(1801678702)],
    );
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @tas
#[test]
fn test_buc_term() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("%$")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let atom = T(&mut slab, &[D(tas!(b"rock")), D(tas!(b"tas")), D(0)]);
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @tas
#[test]
fn test_failure_underscore_in_term() {
    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("%invalid_term")
        .into_result();
    assert!(res.is_err(), "Expected Err, got Ok");
}

// @ta
#[test]
fn test_knot() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("~.nock")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let atom = T(
        &mut slab,
        &[D(tas!(b"sand")), D(tas!(b"ta")), D(1801678702)],
    );
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @ta
#[test]
fn test_empty_knot() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("~.")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let atom = T(&mut slab, &[D(tas!(b"sand")), D(tas!(b"ta")), D(0)]);
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @t
#[test]
fn test_cord() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("'nock'")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let atom = T(&mut slab, &[D(tas!(b"sand")), D(tas!(b"t")), D(1801678702)]);
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @t
#[test]
fn test_empty_cord() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("''")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let atom = T(&mut slab, &[D(tas!(b"sand")), D(tas!(b"t")), D(0)]);
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @t
#[test]
fn test_multiline_cord() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let multiline_cord = "'''\n\
          line one\n\
          line two\n\
          \"quotes\" are fine\n\
          '''";

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse(multiline_cord)
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let num_str = "769716495981374736384378692615420590\
                    711998177018090080796281768113304337845844863340";
    let ubig = UBig::from_str_radix(num_str, 10).expect("test decimal literal should parse");
    let big_atom = Atom::from_ubig(&mut slab, &ubig).as_noun();

    let atom = T(&mut slab, &[D(tas!(b"sand")), D(tas!(b"t")), big_atom]);
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @sb
#[test]
fn test_double_signed_decimal_number() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("--123.100")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let atom = T(&mut slab, &[D(tas!(b"sand")), D(tas!(b"sd")), D(246200)]);
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}

// @sb
#[test]
fn test_signed_binary_number() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let res = native_parser(vec![], false, Arc::new(LineMap::new("")))
        .parse("-0b11.1000")
        .into_result()
        .expect("test parser fixture should parse");

    let actual_noun = hoon_to_noun(&mut slab, &res);

    let atom = T(&mut slab, &[D(tas!(b"sand")), D(tas!(b"sb")), D(111)]);
    let expected_noun = T(&mut slab, &[D(tas!(b"tssg")), atom, D(0)]);
    assert!(slab_noun_equality(&slab, &actual_noun, &expected_noun));
}
