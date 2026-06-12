use std::collections::*;

use chumsky::input::Stream;
use chumsky::prelude::*;

use crate::ast::hoon::*;
use crate::utils::*;

pub fn tis_runes_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just("|").ignore_then(tisbar(hoon.clone(), spec.clone())),
        just('.').ignore_then(tisdot(hoon.clone())),
        just('?').ignore_then(tiswut(hoon.clone())),
        just('^').ignore_then(tisket(hoon.clone(), spec_wide.clone())),
        just(':').ignore_then(tiscol(hoon.clone())),
        just("/").ignore_then(tisfas(hoon.clone(), spec_wide.clone())),
        just(";").ignore_then(tismic(hoon.clone(), spec_wide.clone())),
        just("<").ignore_then(tisgal(hoon.clone())),
        just(">").ignore_then(tisgar(hoon.clone())),
        just('-').ignore_then(tishep(hoon.clone())),
        just('*').ignore_then(tistar(hoon.clone(), spec_wide.clone())),
        just(",").ignore_then(tiscom(hoon.clone())),
        just("+").ignore_then(tislus(hoon.clone())),
        just('~').ignore_then(tissig(hoon.clone())),
    ))
}

pub fn tis_runes_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just("|").ignore_then(tisbar_wide(hoon_wide.clone(), spec_wide.clone())),
        just('.').ignore_then(tisdot_wide(hoon_wide.clone())),
        just('?').ignore_then(tiswut_wide(hoon_wide.clone())),
        just('^').ignore_then(tisket_wide(hoon_wide.clone(), spec_wide.clone())),
        just(':').ignore_then(tiscol_wide(hoon_wide.clone())),
        just("/").ignore_then(tisfas_wide(hoon_wide.clone(), spec_wide.clone())),
        just(";").ignore_then(tismic_wide(hoon_wide.clone(), spec_wide.clone())),
        just("<").ignore_then(tisgal_wide(hoon_wide.clone())),
        just(">").ignore_then(tisgar_wide(hoon_wide.clone())),
        just('-').ignore_then(tishep_wide(hoon_wide.clone())),
        just('*').ignore_then(tistar_wide(hoon_wide.clone(), spec_wide.clone())),
        just(",").ignore_then(tiscom_wide(hoon_wide.clone())),
        just("+").ignore_then(tislus_wide(hoon_wide.clone())),
        just('~').ignore_then(tissig_wide(hoon_wide.clone())),
    ))
}

pub fn tiswut_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    winglist()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(((p, q), r), s)| Hoon::TisWut(p, Box::new(q), Box::new(r), Box::new(s)))
}

pub fn tiswut<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(winglist())
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(((p, q), r), s)| Hoon::TisWut(p, Box::new(q), Box::new(r), Box::new(s)))
}

pub fn tisgar_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::TisGar(Box::new(p), Box::new(q)))
}

pub fn tisgal_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::TisGal(Box::new(p), Box::new(q)))
}

pub fn tishep_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::TisHep(Box::new(p), Box::new(q)))
}

pub fn tiscom_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    two_hoons_wide(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::TisCom(Box::new(p), Box::new(q)))
}

pub fn tislus_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    two_hoons_wide(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::TisLus(Box::new(p), Box::new(q)))
}

pub fn tisket<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(variable_name_and_type(spec_wide.clone()))
        .then_ignore(gap())
        .then(winglist())
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(((p, q), r), s)| Hoon::TisKet(p, q, Box::new(r), Box::new(s)))
}

pub fn tisket_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    variable_name_and_type(spec_wide.clone())
        .then_ignore(just(' '))
        .then(winglist())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(((p, q), r), s)| Hoon::TisKet(p, q, Box::new(r), Box::new(s)))
}

pub fn tisfas<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(variable_name_and_type(spec_wide.clone()))
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|((p, q), r)| Hoon::TisFas(p, Box::new(q), Box::new(r)))
}

pub fn tisfas_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    variable_name_and_type(spec_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|((p, q), r)| Hoon::TisFas(p, Box::new(r), Box::new(q)))
}

pub fn tismic_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    variable_name_and_type(spec_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|((p, q), r)| Hoon::TisMic(p, Box::new(q), Box::new(r)))
}

pub fn tismic<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(variable_name_and_type(spec_wide.clone()))
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|((p, q), r)| Hoon::TisMic(p, Box::new(q), Box::new(r)))
}

pub fn tiscol<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(list_wing_hoon_tall(hoon.clone()))
        .then_ignore(just("=="))
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(p, q)| Hoon::TisCol(p, Box::new(q)))
}

pub fn tiscol_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    list_wing_hoon_wide(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::TisCol(p, Box::new(q)))
}

pub fn tisbar<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(spec.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(p, q)| Hoon::TisBar(Box::new(p), Box::new(q)))
}

pub fn tisbar_wide<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    spec.clone()
        .then_ignore(just(' '))
        .then(hoon.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::TisBar(Box::new(p), Box::new(q)))
}

pub fn tisgar<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(p, q)| Hoon::TisGar(Box::new(p), Box::new(q)))
}

pub fn tistar<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(symbol())
        .then(
            just('=')
                .ignore_then(spec_wide.clone())
                .map(|s| Box::new(s))
                .or_not(),
        )
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(((term, maybe_spec), q), r)| {
            Hoon::TisTar((term, maybe_spec), Box::new(q), Box::new(r))
        })
}

pub fn tistar_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    symbol()
        .then(
            just('=')
                .ignore_then(spec_wide.clone())
                .map(|s| Box::new(s))
                .or_not(),
        )
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(((term, maybe_spec), q), r)| {
            Hoon::TisTar((term, maybe_spec), Box::new(q), Box::new(r))
        })
}

pub fn tisdot<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(winglist())
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|((p, q), r)| Hoon::TisDot(p, Box::new(q), Box::new(r)))
}

pub fn tisdot_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    winglist()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|((p, q), r)| Hoon::TisDot(p, Box::new(q), Box::new(r)))
}

pub fn tiscom<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    two_hoons_tall(hoon.clone()).map(|(p, q)| Hoon::TisCom(Box::new(p), Box::new(q)))
}

pub fn tislus<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    two_hoons_tall(hoon.clone())
        .map(|(p, q)| Hoon::TisLus(Box::new(p), Box::new(q)))
        .labelled("TisLus")
}

pub fn tisgal<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(p, q)| Hoon::TisGal(Box::new(p), Box::new(q)))
}

pub fn tishep<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(p, q)| Hoon::TisHep(Box::new(p), Box::new(q)))
}

pub fn tissig<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(
            hoon.clone()
                .separated_by(gap())
                .at_least(2)
                .collect::<Vec<_>>(),
        )
        .then_ignore(gap())
        .then_ignore(just("=="))
        .map(|list| Hoon::TisSig(list))
}

pub fn tissig_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .separated_by(just(' '))
        .at_least(2)
        .collect::<Vec<_>>()
        .delimited_by(just('('), just(')'))
        .map(|list| Hoon::TisSig(list))
}
