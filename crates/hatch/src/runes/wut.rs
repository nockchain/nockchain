use std::collections::*;

use chumsky::input::{Stream, ValueInput};
use chumsky::prelude::*;

use crate::ast::hoon::*;
use crate::utils::*;

pub fn wut_runes_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just('~').ignore_then(wutsig(hoon.clone(), hoon_wide.clone())),
        just('.').ignore_then(wutdot(hoon.clone())),
        just(':').ignore_then(wutcol(hoon.clone())),
        just("|").ignore_then(wutbar(hoon.clone())),
        just(">").ignore_then(wutgar(hoon.clone())),
        just("<").ignore_then(wutgal(hoon.clone())),
        just('^').ignore_then(wutket(hoon.clone(), hoon_wide.clone())),
        just("&").ignore_then(wutpam(hoon.clone())),
        just('@').ignore_then(wutpat(hoon.clone(), hoon_wide.clone())),
        just('=').ignore_then(wuttis(hoon.clone(), hoon_wide.clone(), spec.clone())),
        just("+").ignore_then(wutlus(hoon.clone(), hoon_wide.clone(), spec.clone())),
        just('-').ignore_then(wuthep(hoon.clone(), hoon_wide.clone(), spec.clone())),
        just("!").ignore_then(wutzap(hoon.clone())),
        just('#').ignore_then(wuthax(hoon.clone(), hoon_wide.clone())),
    ))
}

pub fn wut_runes_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just('~').ignore_then(wutsig_wide(hoon_wide.clone())),
        just('.').ignore_then(wutdot_wide(hoon_wide.clone())),
        just(':').ignore_then(wutcol_wide(hoon_wide.clone())),
        just("|").ignore_then(wutbar_wide(hoon_wide.clone())),
        just(">").ignore_then(wutgar_wide(hoon_wide.clone())),
        just("<").ignore_then(wutgal_wide(hoon_wide.clone())),
        just('^').ignore_then(wutket_wide(hoon_wide.clone())),
        just("&").ignore_then(wutpam_wide(hoon_wide.clone())),
        just('@').ignore_then(wutpat_wide(hoon_wide.clone())),
        just('=').ignore_then(wuttis_wide(hoon_wide.clone(), spec_wide.clone())),
        just("+").ignore_then(wutlus_wide(hoon_wide.clone(), spec_wide.clone())),
        just('-').ignore_then(wuthep_wide(hoon_wide.clone(), spec_wide.clone())),
        just("!").ignore_then(wutzap_wide(hoon_wide.clone())),
        just('#').ignore_then(wuthax_wide(hoon_wide.clone())),
    ))
}

pub fn wutket<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(tiki_tall(hoon.clone(), hoon_wide.clone()))
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|((p, q), r)| wtkt(p, q, r))
}

pub fn wutket_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    tiki_wide(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|((p, q), r)| wtkt(p, q, r))
}

pub fn wutpat<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(tiki_tall(hoon.clone(), hoon_wide.clone()))
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|((p, q), r)| wtpt(p, q, r))
}

pub fn wutpat_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    tiki_wide(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|((p, q), r)| wtpt(p, q, r))
}

pub fn wutzap<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .map(|p| Hoon::WutZap(Box::new(p)))
}

pub fn wutzap_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .delimited_by(just('('), just(')'))
        .map(|p| Hoon::WutZap(Box::new(p)))
}

pub fn wutcol<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|((p, q), r)| Hoon::WutCol(Box::new(p), Box::new(q), Box::new(r)))
}

pub fn wutcol_wide<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon.clone()
        .then_ignore(just(' '))
        .then(hoon.clone())
        .then_ignore(just(' '))
        .then(hoon.clone())
        .delimited_by(just('('), just(')'))
        .map(|((p, q), r)| Hoon::WutCol(Box::new(p), Box::new(q), Box::new(r)))
}

pub fn wutgal<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    two_hoons_tall(hoon.clone()).map(|(p, q)| Hoon::WutGal(Box::new(p), Box::new(q)))
}

pub fn wutgal_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    two_hoons_wide(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::WutGal(Box::new(p), Box::new(q)))
}

pub fn wutdot_wide<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon.clone()
        .then_ignore(just(' '))
        .then(hoon.clone())
        .then_ignore(just(' '))
        .then(hoon.clone())
        .delimited_by(just('('), just(')'))
        .map(|((p, q), r)| Hoon::WutDot(Box::new(p), Box::new(q), Box::new(r)))
}

pub fn wutgar_wide<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon.clone()
        .then_ignore(just(' '))
        .then(hoon.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::WutGar(Box::new(p), Box::new(q)))
}

pub fn wuthax<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(tiki_tall(hoon.clone(), hoon_wide.clone()))
        .try_map(|(p, tik), span| match flay(p) {
            Some(syn) => Ok(wthx(tik, syn)),
            None => Err(Rich::custom(span, "invalid p in ?#p")),
        })
}

pub fn wuthax_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(tiki_wide(hoon_wide.clone()))
        .delimited_by(just('('), just(')'))
        .try_map(|(p, tik), span| match flay(p) {
            Some(syn) => Ok(wthx(tik, syn)),
            None => Err(Rich::custom(span, "invalid p in ?#(p q)")),
        })
}

pub fn wuttis<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(spec.clone())
        .then_ignore(gap())
        .then(tiki_tall(hoon.clone(), hoon_wide.clone()))
        .map(|(p, q)| wtts(q, p))
}

pub fn wuttis_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    spec_wide
        .clone()
        .then_ignore(just(' '))
        .then(tiki_wide(hoon_wide.clone()))
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| wtts(q, p))
}

pub fn wutdot<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|((p, q), r)| Hoon::WutDot(Box::new(p), Box::new(q), Box::new(r)))
}

pub fn wutgar<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(p, q)| Hoon::WutGar(Box::new(p), Box::new(q)))
}

pub fn wuthep<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(tiki_tall(hoon.clone(), hoon_wide.clone()))
        .then_ignore(gap())
        .then(
            spec.clone()
                .then_ignore(gap())
                .then(hoon.clone())
                .then_ignore(gap())
                .repeated()
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .then_ignore(just("=="))
        .map(|(t, list)| wthp(t, list))
}

pub fn wuthep_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    tiki_wide(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(
            spec_wide
                .clone()
                .then_ignore(just(' '))
                .then(hoon_wide.clone())
                .separated_by(just(",").then(just(' ')))
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| wthp(p, q))
}

pub fn wutlus<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(tiki_tall(hoon.clone(), hoon_wide.clone()))
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .then(
            spec.clone()
                .then_ignore(gap())
                .then(hoon.clone())
                .then_ignore(gap())
                .repeated()
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .then_ignore(just("=="))
        .map(|((t, h), list)| wtls(t, h, list))
}

pub fn wutlus_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    tiki_wide(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(
            spec_wide
                .clone()
                .then_ignore(just(' '))
                .then(hoon_wide.clone())
                .separated_by(just(",").then(just(' ')))
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .delimited_by(just('('), just(')'))
        .map(|((t, h), list)| wtls(t, h, list))
}

pub fn wutbar_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .separated_by(just(' '))
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(just('('), just(')'))
        .map(|hoons| Hoon::WutBar(hoons))
}

pub fn wutbar<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon.clone()
        .separated_by(gap())
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(gap(), gap())
        .then_ignore(just("=="))
        .map(|hoons| Hoon::WutBar(hoons))
}

pub fn wutsig<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(tiki_tall(hoon.clone(), hoon_wide.clone()))
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|((p, q), r)| wtsg(p, q, r))
}

pub fn wutsig_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    tiki_wide(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|((p, q), r)| wtsg(p, q, r))
}

pub fn wutpam<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon.clone()
        .separated_by(gap())
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(gap(), gap())
        .then_ignore(just("=="))
        .map(|hoons| Hoon::WutPam(hoons))
}

pub fn wutpam_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .separated_by(just(' '))
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(just('('), just(')'))
        .map(|hoons| Hoon::WutPam(hoons))
}

pub fn wutpam_irregular<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    just("&")
        .ignore_then(
            hoon_wide
                .clone()
                .separated_by(just(' '))
                .at_least(1)
                .collect::<Vec<_>>()
                .delimited_by(just('('), just(')')),
        )
        .map(|hoons| Hoon::WutPam(hoons))
}

pub fn wutbar_irregular<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    just("|")
        .ignore_then(
            hoon_wide
                .clone()
                .separated_by(just(' '))
                .at_least(1)
                .collect::<Vec<_>>()
                .delimited_by(just('('), just(')')),
        )
        .map(|hoons| Hoon::WutBar(hoons))
}

pub fn wutzap_irregular<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    just("!")
        .ignore_then(hoon_wide.clone())
        .map(|h| Hoon::WutZap(Box::new(h)))
}
