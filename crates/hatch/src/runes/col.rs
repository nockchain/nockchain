use std::collections::*;

use chumsky::input::{Stream, ValueInput};
use chumsky::prelude::*;

use crate::ast::hoon::*;
use crate::utils::*;

pub fn col_runes_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just('^').ignore_then(colket(hoon.clone())),
        just('_').ignore_then(colcab(hoon.clone())),
        just("+").ignore_then(collus(hoon.clone())),
        just('-').ignore_then(colhep(hoon.clone())),
        just('*').ignore_then(coltar(hoon.clone())),
        just('~').ignore_then(colsig(hoon.clone())),
    ))
}

pub fn col_runes_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just('^').ignore_then(colket_wide(hoon_wide.clone())),
        just('_').ignore_then(colcab_wide(hoon_wide.clone())),
        just("+").ignore_then(collus_wide(hoon_wide.clone())),
        just('-').ignore_then(colhep_wide(hoon_wide.clone())),
        just('*').ignore_then(coltar_wide(hoon_wide.clone())),
        just('~').ignore_then(colsig_wide(hoon_wide.clone())),
    ))
}

pub fn collus<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    three_hoons_tall(hoon.clone())
        .map(|((p, q), r)| Hoon::ColLus(Box::new(p), Box::new(q), Box::new(r)))
}

pub fn collus_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    three_hoons_wide(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|((p, q), r)| Hoon::ColLus(Box::new(p), Box::new(q), Box::new(r)))
}

pub fn colhep<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    two_hoons_tall(hoon.clone()).map(|(p, q)| Hoon::ColHep(Box::new(p), Box::new(q)))
}

pub fn colhep_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    two_hoons_wide(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::ColHep(Box::new(p), Box::new(q)))
}

pub fn colcab<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(p, q)| Hoon::ColCab(Box::new(p), Box::new(q)))
}

pub fn colcab_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    two_hoons_wide(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::ColCab(Box::new(p), Box::new(q)))
}

pub fn colket<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(((p, q), s), r)| Hoon::ColKet(Box::new(p), Box::new(q), Box::new(s), Box::new(r)))
}

pub fn colket_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(((p, q), s), r)| Hoon::ColKet(Box::new(p), Box::new(q), Box::new(s), Box::new(r)))
}

pub fn coltar<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(list_hoon_tall(hoon.clone()))
        .then_ignore(gap())
        .then_ignore(just("=="))
        .map(|list| Hoon::ColTar(list))
}

pub fn coltar_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    list_hoon_wide(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|list| Hoon::ColTar(list))
}

pub fn colsig<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(list_hoon_tall(hoon.clone()))
        .then_ignore(gap())
        .then_ignore(just("=="))
        .map(|list| Hoon::ColSig(list))
}

pub fn colsig_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    list_hoon_wide(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|list| Hoon::ColSig(list))
}

pub fn list_syntax<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    just("~[")
        .to(true)
        .or(just("[").to(false)) //  ~[  or  [
        .then(choice((
            hoon.clone()
                .separated_by(gap())
                .at_least(1)
                .collect::<Vec<_>>()
                .delimited_by(just(' '), gap()),
            hoon_wide
                .clone()
                .separated_by(just(' '))
                .at_least(1)
                .collect::<Vec<_>>(),
        )))
        .then(just("]~").to(true).or(just("]").to(false))) //  ]~ or ]
        .map(|((start, list), end)| {
            if start {
                if end {
                    return Hoon::ColSig(vec![Hoon::ColSig(list)]);
                }
                {
                    return Hoon::ColSig(list);
                }
            } else {
                if end {
                    return Hoon::ColSig(vec![Hoon::ColTar(list)]);
                }
                {
                    return Hoon::ColTar(list);
                }
            }
        })
}
