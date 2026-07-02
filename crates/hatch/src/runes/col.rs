use std::collections::*;
use std::sync::Arc;

use chumsky::input::{Stream, ValueInput};
use chumsky::prelude::*;

use crate::ast::hoon::*;
use crate::utils::*;

pub fn col_runes_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just('^').ignore_then(colket(hoon.clone(), linemap.clone())),
        just('_').ignore_then(colcab(hoon.clone(), linemap.clone())),
        just("+").ignore_then(collus(hoon.clone(), linemap.clone())),
        just('-').ignore_then(colhep(hoon.clone(), linemap.clone())),
        just('*').ignore_then(coltar(hoon.clone(), linemap.clone())),
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

fn attach_rune_help(
    hoon: Hoon,
    start: usize,
    end: usize,
    linemap: &LineMap,
    allow_four_space: bool,
) -> (Hoon, Option<NounExpr>) {
    let Some((spaces, help)) = linemap.help_after_rune_with_spaces(start, end) else {
        return (hoon, None);
    };
    if spaces == 4 && !allow_four_space {
        return (hoon, Some(help));
    }
    if hoon_tail_has_help(&hoon, &help) {
        (hoon, None)
    } else {
        (Hoon::Note(Note::Help(help), Box::new(hoon)), None)
    }
}

pub fn collus<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(
            hoon.clone()
                .map_with(|p: Hoon, e| (p, e.span().start(), e.span().end())),
        )
        .then_ignore(gap())
        .then(
            hoon.clone()
                .map_with(|q: Hoon, e| (q, e.span().start(), e.span().end())),
        )
        .then_ignore(gap())
        .then(hoon.clone())
        .map(move |(((p, p_start, p_end), (q, q_start, q_end)), r)| {
            let (p, p_help) = attach_rune_help(p, p_start, p_end, &linemap, false);
            let (mut q, q_help) = attach_rune_help(q, q_start, q_end, &linemap, false);
            if let Some(help) = p_help {
                q = attach_help_to_hoon(q, help);
            }
            let r = if let Some(help) = q_help {
                attach_help_to_hoon(r, help)
            } else {
                r
            };
            Hoon::ColLus(Box::new(p), Box::new(q), Box::new(r))
        })
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
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(
            hoon.clone()
                .map_with(|p: Hoon, e| (p, e.span().start(), e.span().end())),
        )
        .then_ignore(gap())
        .then(hoon.clone())
        .map(move |((p, p_start, p_end), q)| {
            let (p, p_help) = attach_rune_help(p, p_start, p_end, &linemap, false);
            let q = if let Some(help) = p_help {
                attach_help_to_hoon(q, help)
            } else {
                q
            };
            Hoon::ColHep(Box::new(p), Box::new(q))
        })
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
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(
            hoon.clone()
                .map_with(|p: Hoon, e| (p, e.span().start(), e.span().end())),
        )
        .then_ignore(gap())
        .then(hoon.clone())
        .map(move |((p, p_start, p_end), q)| {
            let (p, p_help) = attach_rune_help(p, p_start, p_end, &linemap, false);
            let q = if let Some(help) = p_help {
                attach_help_to_hoon(q, help)
            } else {
                q
            };
            Hoon::ColCab(Box::new(p), Box::new(q))
        })
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
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(
            hoon.clone()
                .map_with(|p: Hoon, e| (p, e.span().start(), e.span().end())),
        )
        .then_ignore(gap())
        .then(
            hoon.clone()
                .map_with(|q: Hoon, e| (q, e.span().start(), e.span().end())),
        )
        .then_ignore(gap())
        .then(
            hoon.clone()
                .map_with(|s: Hoon, e| (s, e.span().start(), e.span().end())),
        )
        .then_ignore(gap())
        .then(hoon.clone())
        .map(
            move |((((p, p_start, p_end), (q, q_start, q_end)), (s, s_start, s_end)), r)| {
                let (p, p_help) = attach_rune_help(p, p_start, p_end, &linemap, false);
                let (mut q, q_help) = attach_rune_help(q, q_start, q_end, &linemap, false);
                if let Some(help) = p_help {
                    q = attach_help_to_hoon(q, help);
                }
                let (mut s, s_help) = attach_rune_help(s, s_start, s_end, &linemap, false);
                if let Some(help) = q_help {
                    s = attach_help_to_hoon(s, help);
                }
                let r = if let Some(help) = s_help {
                    attach_help_to_hoon(r, help)
                } else {
                    r
                };
                Hoon::ColKet(Box::new(p), Box::new(q), Box::new(s), Box::new(r))
            },
        )
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
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(
            hoon.clone()
                .map_with(|hoon: Hoon, e| (hoon, e.span().start(), e.span().end()))
                .separated_by(gap())
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .then_ignore(gap())
        .then_ignore(just("=="))
        .map(move |list| {
            // hoon-138 ++clad: a `.name:` frag-doc block is the prefix whit of
            // the entry FOLLOWING it — all its bat entries stack as nested
            // %notes on that one entry (in ~(tap by bat) order), they are NOT
            // distributed to the same-named entries.
            Hoon::ColTar(
                list.into_iter()
                    .map(|(hoon, start, _end)| {
                        let entries = linemap.frag_block_doc_entries(start);
                        stack_block_docs_clad(hoon, entries)
                    })
                    .collect(),
            )
        })
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
