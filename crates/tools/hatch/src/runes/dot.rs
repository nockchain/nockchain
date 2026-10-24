use std::collections::*;

use chumsky::input::{Stream, ValueInput};
use chumsky::prelude::*;

use crate::ast::hoon::*;
use crate::utils::*;

pub fn dot_runes_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just("+").ignore_then(dotlus(hoon.clone())),
        just('*').ignore_then(dottar(hoon.clone())),
        just('=').ignore_then(dottis(hoon.clone())),
        just('?').ignore_then(dotwut(hoon.clone())),
        just('^').ignore_then(dotket(hoon.clone(), spec.clone())),
    ))
}

pub fn dot_runes_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just("+").ignore_then(dotlus_wide(hoon_wide.clone())),
        just('*').ignore_then(dottar_wide(hoon_wide.clone())),
        just('=').ignore_then(dottis_wide(hoon_wide.clone())),
        just('?').ignore_then(dotwut_wide(hoon_wide.clone())),
        just('^').ignore_then(dotket_wide(hoon_wide.clone(), spec_wide.clone())),
        dotlus_irregular(hoon_wide.clone()),
    ))
}

pub fn dotlus<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .map(|p| Hoon::DotLus(Box::new(p)))
}

pub fn dotlus_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .delimited_by(just('('), just(')'))
        .map(|p| Hoon::DotLus(Box::new(p)))
}

pub fn dotlus_irregular<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .delimited_by(just('('), just(')'))
        .map(|p| Hoon::DotLus(Box::new(p)))
}

pub fn dotket_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    spec_wide
        .clone()
        .then_ignore(just(' '))
        .then(list_hoon_wide(hoon_wide.clone()))
        .delimited_by(just('('), just(')'))
        .map(|(s, list)| Hoon::DotKet(Box::new(s), Box::new(Hoon::ColTar(list))))
}

pub fn dottar<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(s, h)| Hoon::DotTar(Box::new(s), Box::new(h)))
}

pub fn dotket<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(spec.clone())
        .then_ignore(gap())
        .then(list_hoon_tall(hoon.clone()))
        .then_ignore(gap())
        .then_ignore(just("=="))
        .map(|(s, list)| Hoon::DotKet(Box::new(s), Box::new(Hoon::ColTar(list))))
}

pub fn dotwut<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .map(|p| Hoon::DotWut(Box::new(p)))
}

pub fn dottis<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(s, h)| Hoon::DotTis(Box::new(s), Box::new(h)))
}

pub fn dottis_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    two_hoons_wide(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(s, h)| Hoon::DotTis(Box::new(s), Box::new(h)))
}

pub fn dottar_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::DotTar(Box::new(p), Box::new(q)))
}

pub fn dotwut_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .delimited_by(just('('), just(')'))
        .map(|p| Hoon::DotWut(Box::new(p)))
}

pub fn dottis_irregular<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::DotTis(Box::new(p), Box::new(q)))
}
