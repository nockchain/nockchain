use std::collections::*;
use std::sync::Arc;

use chumsky::input::{Stream, ValueInput};
use chumsky::prelude::*;

use crate::ast::hoon::*;
use crate::utils::*;

pub fn wut_runes_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
    spec_wide: impl ParserExt<'src, Spec>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just('~').ignore_then(wutsig(hoon.clone(), hoon_wide.clone(), linemap.clone())),
        just('.').ignore_then(wutdot(hoon.clone(), linemap.clone())),
        just(':').ignore_then(wutcol(hoon.clone(), linemap.clone())),
        just("|").ignore_then(wutbar(hoon.clone())),
        just(">").ignore_then(wutgar(hoon.clone())),
        just("<").ignore_then(wutgal(hoon.clone())),
        just('^').ignore_then(wutket(hoon.clone(), hoon_wide.clone())),
        just("&").ignore_then(wutpam(hoon.clone(), linemap.clone())),
        just('@').ignore_then(wutpat(hoon.clone(), hoon_wide.clone())),
        just('=').ignore_then(wuttis(hoon.clone(), hoon_wide.clone(), spec.clone())),
        just("+").ignore_then(wutlus(
            hoon.clone(),
            hoon_wide.clone(),
            spec.clone(),
            linemap.clone(),
        )),
        just('-').ignore_then(wuthep(
            hoon.clone(),
            hoon_wide.clone(),
            spec.clone(),
            linemap.clone(),
        )),
        just("!").ignore_then(wutzap(hoon.clone())),
        just('#').ignore_then(wuthax(hoon.clone(), hoon_wide.clone())),
    ))
}

pub fn wut_runes_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just('~').ignore_then(wutsig_wide(hoon_wide.clone())),
        just('.').ignore_then(wutdot_wide(hoon_wide.clone())),
        just(':').ignore_then(wutcol_wide(hoon_wide.clone(), linemap.clone())),
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
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(
            hoon.clone()
                .map_with(|q: Hoon, e| (q, e.span().start(), e.span().end())),
        )
        .then_ignore(gap())
        .then(
            hoon.clone()
                .map_with(|r: Hoon, e| (r, e.span().start(), e.span().end())),
        )
        .map(move |((p, (q, q_start, q_end)), (r, r_start, r_end))| {
            let attach = |hoon: Hoon, start, end| {
                if let Some(help) = linemap.help_after_rune(start, end) {
                    if hoon_tail_has_help(&hoon, &help) {
                        hoon
                    } else {
                        Hoon::Note(Note::Help(help), Box::new(hoon))
                    }
                } else {
                    hoon
                }
            };
            let q = attach(q, q_start, q_end);
            let r = attach(r, r_start, r_end);
            Hoon::WutCol(Box::new(p), Box::new(q), Box::new(r))
        })
}

pub fn wutcol_wide<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon.clone()
        .then_ignore(just(' '))
        .then(hoon.clone())
        .then_ignore(just(' '))
        .then(
            hoon.clone()
                .map_with(|r: Hoon, e| (r, e.span().start(), e.span().end())),
        )
        .delimited_by(just('('), just(')'))
        .map(move |((p, q), (r, start, end))| {
            let r = if let Some(help) = linemap.help_after_rune(start, end) {
                if hoon_tail_has_help(&r, &help) {
                    r
                } else {
                    Hoon::Note(Note::Help(help), Box::new(r))
                }
            } else {
                r
            };
            Hoon::WutCol(Box::new(p), Box::new(q), Box::new(r))
        })
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
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(
            hoon.clone()
                .map_with(|q: Hoon, e| (q, e.span().start(), e.span().end())),
        )
        .then_ignore(gap())
        .then(
            hoon.clone()
                .map_with(|r: Hoon, e| (r, e.span().start(), e.span().end())),
        )
        .map(move |((p, (q, q_start, q_end)), (r, r_start, r_end))| {
            let q = if let Some(help) = linemap.help_after_rune(q_start, q_end) {
                if hoon_tail_has_help(&q, &help) {
                    q
                } else {
                    Hoon::Note(Note::Help(help), Box::new(q))
                }
            } else {
                q
            };
            let r = if let Some(help) = linemap.help_after_rune(r_start, r_end) {
                if hoon_tail_has_help(&r, &help) {
                    r
                } else {
                    Hoon::Note(Note::Help(help), Box::new(r))
                }
            } else {
                r
            };
            Hoon::WutDot(Box::new(p), Box::new(q), Box::new(r))
        })
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
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(tiki_tall(hoon.clone(), hoon_wide.clone()))
        .then_ignore(gap())
        .then(
            spec.clone()
                .map_with(|spec: Spec, e| (spec, e.span().start(), e.span().end()))
                .then_ignore(gap())
                .then(
                    hoon.clone()
                        .map_with(|h: Hoon, e| (h, e.span().start(), e.span().end())),
                )
                .then_ignore(gap())
                .repeated()
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .then_ignore(just("=="))
        .map(move |(t, list)| {
            let list = list
                .into_iter()
                .map(|((spec, spec_start, spec_end), (hoon, start, end))| {
                    let hoon = if let Some(help) = linemap.help_after_rune(start, end) {
                        if hoon_tail_has_help(&hoon, &help) {
                            hoon
                        } else {
                            Hoon::Note(Note::Help(help), Box::new(hoon))
                        }
                    } else {
                        hoon
                    };
                    let spec = if let Some(help) =
                        linemap.help_after_line_start_rune(spec_start, spec_end)
                    {
                        Spec::Gist(help, Box::new(spec))
                    } else {
                        spec
                    };
                    (spec, hoon)
                })
                .collect::<Vec<_>>();
            wthp(t, list)
        })
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
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(tiki_tall(hoon.clone(), hoon_wide.clone()))
        .then_ignore(gap())
        .then(
            hoon.clone()
                .map_with(|h: Hoon, e| (h, e.span().start(), e.span().end())),
        )
        .then_ignore(gap())
        .then(
            spec.clone()
                .map_with(|spec: Spec, e| (spec, e.span().start(), e.span().end()))
                .then_ignore(gap())
                .then(
                    hoon.clone()
                        .map_with(|h: Hoon, e| (h, e.span().start(), e.span().end())),
                )
                .then_ignore(gap())
                .repeated()
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .then_ignore(just("=="))
        .map(move |((t, (h, h_start, h_end)), list)| {
            let h = if let Some(help) = linemap.help_after_rune(h_start, h_end) {
                if hoon_tail_has_help(&h, &help) {
                    h
                } else {
                    Hoon::Note(Note::Help(help), Box::new(h))
                }
            } else {
                h
            };
            let list = list
                .into_iter()
                .map(|((spec, spec_start, spec_end), (hoon, start, end))| {
                    let hoon = if let Some(help) = linemap.help_after_rune(start, end) {
                        if hoon_tail_has_help(&hoon, &help) {
                            hoon
                        } else {
                            Hoon::Note(Note::Help(help), Box::new(hoon))
                        }
                    } else {
                        hoon
                    };
                    let spec = if let Some(help) =
                        linemap.help_after_line_start_rune(spec_start, spec_end)
                    {
                        Spec::Gist(help, Box::new(spec))
                    } else {
                        spec
                    };
                    (spec, hoon)
                })
                .collect::<Vec<_>>();
            wtls(t, h, list)
        })
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
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(tiki_tall(hoon.clone(), hoon_wide.clone()))
        .then_ignore(gap())
        .then(
            hoon.clone()
                .map_with(|q: Hoon, e| (q, e.span().start(), e.span().end())),
        )
        .then_ignore(gap())
        .then(hoon.clone())
        .map(move |((p, (q, q_start, q_end)), r)| {
            let q = if let Some(help) = linemap.help_after_rune(q_start, q_end) {
                if hoon_tail_has_help(&q, &help) {
                    q
                } else {
                    Hoon::Note(Note::Help(help), Box::new(q))
                }
            } else {
                q
            };
            wtsg(p, q, r)
        })
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
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon.clone()
        .separated_by(gap())
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(gap(), gap())
        .then_ignore(just("=="))
        .map_with(move |hoons: Vec<Hoon>, e| {
            let close_end = e.span().end();
            let close_start = close_end.saturating_sub(2);
            let node = Hoon::WutPam(hoons);
            if let Some(help) = linemap.help_after_rune(close_start, close_end) {
                Hoon::Note(Note::Help(help), Box::new(node))
            } else {
                node
            }
        })
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
