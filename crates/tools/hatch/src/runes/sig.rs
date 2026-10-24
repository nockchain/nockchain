use std::collections::*;

use chumsky::input::{Stream, ValueInput};
use chumsky::prelude::*;

use crate::ast::hoon::*;
use crate::utils::*;

pub fn sig_runes_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just('%').ignore_then(sigcen(hoon.clone())),
        just('/').ignore_then(sigfas(hoon.clone())),
        just('_').ignore_then(sigcab(hoon.clone())),
        just('+').ignore_then(siglus(hoon.clone())),
        just('!').ignore_then(sigzap(hoon.clone())),
        just('|').ignore_then(sigbar(hoon.clone())),
        just('>').ignore_then(siggar(hoon.clone())),
        just('<').ignore_then(siggal(hoon.clone())),
        just('&').ignore_then(sigpam(hoon.clone())),
        just('?').ignore_then(sigwut(hoon.clone())),
        just('=').ignore_then(sigtis(hoon.clone())),
        just('!').ignore_then(sigzap(hoon.clone())),
    ))
}

pub fn sig_runes_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just('%').ignore_then(sigcen_wide(hoon_wide.clone())),
        just('/').ignore_then(sigfas_wide(hoon_wide.clone())),
        just('!').ignore_then(sigzap_wide(hoon_wide.clone())),
        just('<').ignore_then(siggal_wide(hoon_wide.clone())),
        just('?').ignore_then(sigwut_wide(hoon_wide.clone())),
        just('=').ignore_then(sigtis_wide(hoon_wide.clone())),
        just('>').ignore_then(siggar_wide(hoon_wide.clone())),
        just('+').ignore_then(siglus_wide(hoon_wide.clone())),
        just('_').ignore_then(sigcab_wide(hoon_wide.clone())),
        just('|').ignore_then(sigbar_wide(hoon_wide.clone())),
        just('&').ignore_then(sigpam_wide(hoon_wide.clone())),
    ))
}

pub fn sigpam<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(
            just(">")
                .repeated()
                .at_most(3)
                .count()
                .then_ignore(gap())
                .or_not(),
        )
        .then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|((maybe_p, q), r)| {
            let p = maybe_p.unwrap_or(0);
            Hoon::SigPam(p as u64, Box::new(q), Box::new(r))
        })
}

pub fn sigwut<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(
            just(">")
                .repeated()
                .at_most(3)
                .count()
                .then_ignore(gap())
                .or_not(),
        )
        .then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(((maybe_p, q), r), s)| {
            let p = maybe_p.unwrap_or(0);
            Hoon::SigWut(p as u64, Box::new(q), Box::new(r), Box::new(s))
        })
}

pub fn sigwut_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    just(">")
        .repeated()
        .at_most(3)
        .count()
        .then_ignore(just(' '))
        .or_not()
        .then(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(((maybe_p, q), r), s)| {
            let p = maybe_p.unwrap_or(0);
            Hoon::SigWut(p as u64, Box::new(q), Box::new(r), Box::new(s))
        })
}

pub fn sigpam_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    just(">")
        .repeated()
        .at_most(3)
        .count()
        .then_ignore(just(' '))
        .or_not()
        .then(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|((maybe_p, q), r)| {
            let p = maybe_p.unwrap_or(0);
            Hoon::SigPam(p as u64, Box::new(q), Box::new(r))
        })
}

pub fn sigzap<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(p, q)| Hoon::SigZap(Box::new(p), Box::new(q)))
}

pub fn sigzap_wide<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    two_hoons_tall(hoon.clone()).map(|(p, q)| Hoon::SigZap(Box::new(p), Box::new(q)))
}

pub fn sigbar<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(p, q)| Hoon::SigBar(Box::new(p), Box::new(q)))
}

pub fn sigbar_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::SigBar(Box::new(p), Box::new(q)))
}

pub fn sigtis<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    two_hoons_tall(hoon.clone()).map(|(p, q)| Hoon::SigTis(Box::new(p), Box::new(q)))
}

pub fn sigtis_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    two_hoons_wide(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::SigTis(Box::new(p), Box::new(q)))
}

pub fn siglus<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    //  the hoon parser accepts an optional first arg here
    gap() //  but its never used anywhere, and idk what is...
        .ignore_then(hoon.clone())
        .map(|p| Hoon::SigLus(0, Box::new(p)))
}

pub fn siglus_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    //  the hoon parser accepts an optional first arg here
    hoon_wide
        .clone() //  but its never used anywhere, and idk what is...
        .delimited_by(just('('), just(')'))
        .map(|p| Hoon::SigLus(0, Box::new(p)))
}

pub fn sigcab<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(p, q)| Hoon::SigCab(Box::new(p), Box::new(q)))
}

pub fn sigcab_wide<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon.clone()
        .then_ignore(just(' '))
        .then(hoon.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::SigCab(Box::new(p), Box::new(q)))
}

pub fn sigcen<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(jet_signature())
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .then(jet_hooks(hoon.clone()))
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(((p, q), r), s)| Hoon::SigCen(p, Box::new(q), r, Box::new(s)))
}

pub fn sigcen_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    jet_signature()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(jet_hooks(hoon_wide.clone()))
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(((p, q), r), s)| Hoon::SigCen(p, Box::new(q), r, Box::new(s)))
}

pub fn sigfas<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(jet_signature())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(p, q)| Hoon::SigFas(p, Box::new(q)))
}

pub fn sigfas_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    jet_signature()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::SigFas(p, Box::new(q)))
}

pub fn siggar_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    term()
        .then(just('.').ignore_then(hoon_wide.clone()).or_not())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|((term, maybe_hoon), q)| match maybe_hoon {
            None => Hoon::SigGar(TermOrPair::Term(term), Box::new(q)),
            Some(h) => Hoon::SigGar(TermOrPair::Pair(term, Box::new(h)), Box::new(q)),
        })
}

pub fn siggar<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(term())
        .then(just('.').ignore_then(hoon_wide.clone()).or_not())
        .then_ignore(gap())
        .then(hoon_wide.clone())
        .map(|((term, maybe_hoon), q)| match maybe_hoon {
            None => Hoon::SigGar(TermOrPair::Term(term), Box::new(q)),
            Some(h) => Hoon::SigGar(TermOrPair::Pair(term, Box::new(h)), Box::new(q)),
        })
}

pub fn siggal<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(term())
        .then(just('.').ignore_then(hoon_wide.clone()).or_not())
        .then_ignore(gap())
        .then(hoon_wide.clone())
        .map(|((term, maybe_hoon), q)| match maybe_hoon {
            None => Hoon::SigGal(TermOrPair::Term(term), Box::new(q)),
            Some(h) => Hoon::SigGal(TermOrPair::Pair(term, Box::new(h)), Box::new(q)),
        })
}

pub fn siggal_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    term()
        .then(just('.').ignore_then(hoon_wide.clone()).or_not())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|((term, maybe_hoon), q)| match maybe_hoon {
            None => Hoon::SigGal(TermOrPair::Term(term), Box::new(q)),
            Some(h) => Hoon::SigGal(TermOrPair::Pair(term, Box::new(h)), Box::new(q)),
        })
}
