use ibig::{ibig, ubig};
use serde_test::{assert_de_tokens, assert_tokens, Token};

#[test]
fn test_ubig_serde() {
    assert_tokens(&ubig!(0), &[Token::Seq { len: Some(0) }, Token::SeqEnd]);
    assert_de_tokens(&ubig!(0), &[Token::Seq { len: None }, Token::SeqEnd]);
    assert_tokens(
        &ubig!(17),
        &[Token::Seq { len: Some(1) }, Token::U64(17), Token::SeqEnd],
    );
    assert_de_tokens(
        &ubig!(17),
        &[Token::Seq { len: None }, Token::U8(17), Token::SeqEnd],
    );
    assert_tokens(
        &ubig!(0x123451234567890abcdef),
        &[
            Token::Seq { len: Some(2) },
            Token::U64(0x1234567890abcdef),
            Token::U64(0x12345),
            Token::SeqEnd,
        ],
    );
    assert_de_tokens(
        &ubig!(0x123451234567890abcdef),
        &[
            Token::Seq { len: None },
            Token::U64(0x1234567890abcdef),
            Token::U64(0x12345),
            Token::SeqEnd,
        ],
    );
}

#[test]
fn test_ibig_serde() {
    assert_tokens(
        &ibig!(0),
        &[
            Token::Tuple { len: 2 },
            Token::UnitVariant {
                name: "Sign",
                variant: "Positive",
            },
            Token::Seq { len: Some(0) },
            Token::SeqEnd,
            Token::TupleEnd,
        ],
    );
    assert_de_tokens(
        &ibig!(0),
        &[
            Token::Seq { len: None },
            Token::UnitVariant {
                name: "Sign",
                variant: "Negative",
            },
            Token::Seq { len: None },
            Token::SeqEnd,
            Token::SeqEnd,
        ],
    );
    assert_tokens(
        &ibig!(17),
        &[
            Token::Tuple { len: 2 },
            Token::UnitVariant {
                name: "Sign",
                variant: "Positive",
            },
            Token::Seq { len: Some(1) },
            Token::U64(17),
            Token::SeqEnd,
            Token::TupleEnd,
        ],
    );
    assert_tokens(
        &ibig!(-17),
        &[
            Token::Tuple { len: 2 },
            Token::UnitVariant {
                name: "Sign",
                variant: "Negative",
            },
            Token::Seq { len: Some(1) },
            Token::U64(17),
            Token::SeqEnd,
            Token::TupleEnd,
        ],
    );
}
