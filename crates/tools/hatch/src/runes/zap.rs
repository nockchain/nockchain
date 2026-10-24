use std::collections::*;

use chumsky::input::{Stream, ValueInput};
use chumsky::prelude::*;

use crate::ast::hoon::*;
use crate::utils::*;

pub fn zap_runes_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
    hoon_with_trace: impl ParserExt<'src, Hoon>,
    hoon_no_trace: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just(':').ignore_then(zapcol(hoon_with_trace.clone())),
        just('.').ignore_then(zapdot(hoon_no_trace.clone())),
        just(",").ignore_then(zapcom(hoon.clone())),
        just(";").ignore_then(zapmic(hoon.clone())),
        just(">").ignore_then(zapgar(hoon.clone())),
        just("<").ignore_then(zapgal(hoon.clone(), spec.clone())),
        just('@').ignore_then(zappat(hoon.clone())),
        just('=').ignore_then(zaptis(hoon.clone())),
        just('?').ignore_then(zapwut(hoon.clone())),
        just("!").to(Hoon::ZapZap),
    ))
    .boxed()
}

pub fn zap_runes_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
    hoon_with_trace: impl ParserExt<'src, Hoon>,
    hoon_no_trace: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just(':').ignore_then(zapcol_wide(hoon_with_trace.clone())),
        just('.').ignore_then(zapdot_wide(hoon_no_trace.clone())),
        just(",").ignore_then(zapcom_wide(hoon_wide.clone())),
        just(";").ignore_then(zapmic_wide(hoon_wide.clone())),
        just(">").ignore_then(zapgar_wide(hoon_wide.clone())),
        just("<").ignore_then(zapgal_wide(hoon_wide.clone(), spec_wide.clone())),
        just('@').ignore_then(zappat_wide(hoon_wide.clone())),
        just('=').ignore_then(zaptis_wide(hoon_wide.clone())),
        just('?').ignore_then(zapwut_wide(hoon_wide.clone())),
        just("!").to(Hoon::ZapZap),
    ))
}

pub fn zapcom<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    two_hoons_tall(hoon.clone()).map(|(p, q)| Hoon::ZapCom(Box::new(p), Box::new(q)))
}

pub fn zapcom_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    two_hoons_wide(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::ZapCom(Box::new(p), Box::new(q)))
}

pub fn zappat<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    winglist()
        .separated_by(just(","))
        .at_least(1)
        .collect::<Vec<_>>()
        .then(two_hoons_tall(hoon.clone()))
        .map(|(list, (p, q))| Hoon::ZapPat(list, Box::new(p), Box::new(q)))
}

pub fn zappat_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    winglist()
        .separated_by(just(","))
        .at_least(1)
        .collect::<Vec<_>>()
        .then_ignore(just(' '))
        .then(two_hoons_wide(hoon_wide.clone()))
        .delimited_by(just('('), just(')'))
        .map(|(list, (p, q))| Hoon::ZapPat(list, Box::new(p), Box::new(q)))
}

pub fn zapmic<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    two_hoons_tall(hoon.clone()).map(|(p, q)| Hoon::ZapMic(Box::new(p), Box::new(q)))
}

pub fn zapmic_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    two_hoons_wide(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::ZapMic(Box::new(p), Box::new(q)))
}

pub fn zapdot<'src>(
    hoon_no_trace: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap().ignore_then(hoon_no_trace.clone()).boxed()
}

pub fn zapdot_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide.clone().delimited_by(just('('), just(')'))
}

pub fn zaptis<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .map(|h| Hoon::ZapTis(Box::new(h)))
}

pub fn zaptis_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .delimited_by(just('('), just(')'))
        .map(|h| Hoon::ZapTis(Box::new(h)))
}

pub fn zapgar<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .map(|h| Hoon::ZapGar(Box::new(h)))
}

pub fn zapgar_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .delimited_by(just('('), just(')'))
        .map(|h| Hoon::ZapGar(Box::new(h)))
}

pub fn zapgal<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(spec.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(p, q)| Hoon::ZapGal(Box::new(p), Box::new(q)))
}

pub fn zapgal_wide<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    spec.clone()
        .then_ignore(just(' '))
        .then(hoon.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::ZapGal(Box::new(p), Box::new(q)))
}

pub fn zapcol<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap().ignore_then(hoon.clone())
}

pub fn zapcol_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide.clone().delimited_by(just('('), just(')'))
}

pub fn zapwut<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(
            decimal_number()
                .map(|n| ZpwtArg::ParsedAtom(n))
                .or(decimal_number()
                    .then_ignore(gap())
                    .then(decimal_number())
                    .delimited_by(just("["), just("]"))
                    .map(|(s1, s2)| ZpwtArg::Pair(s1, s2)))
                .map(|p| p),
        )
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(p, q)| Hoon::ZapWut(p, Box::new(q)))
}

pub fn zapwut_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    decimal_number()
        .map(|n| ZpwtArg::ParsedAtom(n))
        .or(decimal_number()
            .then_ignore(just(' '))
            .then(decimal_number())
            .delimited_by(just("["), just("]"))
            .map(|(s1, s2)| ZpwtArg::Pair(s1, s2)))
        .map(|p| p)
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::ZapWut(p, Box::new(q)))
}
