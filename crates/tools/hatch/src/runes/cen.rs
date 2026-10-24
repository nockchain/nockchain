use std::collections::*;
use std::sync::Arc;

use chumsky::input::{Stream, ValueInput};
use chumsky::prelude::*;

use crate::ast::hoon::*;
use crate::utils::*;

pub fn cen_runes_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just('_').ignore_then(cencab(hoon.clone(), linemap.clone())),
        just('.').ignore_then(cendot(hoon.clone())),
        just('^').ignore_then(cenket(hoon.clone())),
        just("+").ignore_then(cenlus(hoon.clone(), linemap.clone())),
        just('-').ignore_then(cenhep(hoon.clone(), linemap.clone())),
        just(':').ignore_then(cencol(hoon.clone())),
        just('~').ignore_then(censig(hoon.clone())),
        just('*').ignore_then(centar(hoon.clone(), linemap.clone())),
        just('=').ignore_then(centis(hoon.clone(), linemap.clone())),
    ))
}

pub fn cen_runes_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just('_').ignore_then(cencab_wide(hoon_wide.clone())),
        just('.').ignore_then(cendot_wide(hoon_wide.clone())),
        just('^').ignore_then(cenket_wide(hoon_wide.clone())),
        just("+").ignore_then(cenlus_wide(hoon_wide.clone())),
        just('-').ignore_then(cenhep_wide(hoon_wide.clone())),
        just(':').ignore_then(cencol_wide(hoon_wide.clone())),
        just('~').ignore_then(censig_wide(hoon_wide.clone())),
        just('*').ignore_then(centar_wide(hoon_wide.clone())),
        just('=').ignore_then(centis_wide(hoon_wide.clone())),
    ))
}

pub fn cen_spec_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    choice((
        just('-').ignore_then(cenhep_spec(hoon.clone(), spec.clone())),
        just("+").ignore_then(cenlus_spec(hoon.clone(), spec.clone(), linemap.clone())),
        just("^").ignore_then(cenket_spec(hoon.clone(), spec.clone())),
        just(".").ignore_then(cendot_spec(hoon.clone(), spec.clone())),
        just(":").ignore_then(cencol_spec(hoon.clone(), spec.clone())),
    ))
}

pub fn cen_spec_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    choice((
        just('-').ignore_then(cenhep_spec_wide(hoon_wide.clone(), spec_wide.clone())),
        just("+").ignore_then(cenlus_spec_wide(hoon_wide.clone(), spec_wide.clone())),
        just("^").ignore_then(cenket_spec_wide(hoon_wide.clone(), spec_wide.clone())),
        just(".").ignore_then(cendot_spec_wide(hoon_wide.clone(), spec_wide.clone())),
        just(":").ignore_then(cencol_spec_wide(hoon_wide.clone(), spec_wide.clone())),
    ))
}

pub fn cenket<'src>(
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
        .map(|(((p, q), s), r)| Hoon::CenKet(Box::new(p), Box::new(q), Box::new(s), Box::new(r)))
}

pub fn cenket_wide<'src>(
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
        .map(|(((p, q), s), r)| Hoon::CenKet(Box::new(p), Box::new(q), Box::new(s), Box::new(r)))
}

pub fn cencol<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(list_hoon_tall(hoon.clone()).then_ignore(gap()))
        .then_ignore(just("=="))
        .map(|(p, q)| Hoon::CenCol(Box::new(p), q))
}

pub fn cencol_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .then(
            just(' ')
                .ignore_then(list_hoon_wide(hoon_wide.clone()))
                .or_not(),
        )
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::CenCol(Box::new(p), q.unwrap_or_default()))
}

fn documented_rune_body<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    linemap: Arc<LineMap>,
    allow_four_space: bool,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon.map_with(|hoon: Hoon, e| (hoon, e.span().start(), e.span().end()))
        .map(move |(hoon, start, end)| {
            attach_rune_help(hoon, start, end, linemap.as_ref(), allow_four_space)
        })
}

fn documented_rune_body_with_carry<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    linemap: Arc<LineMap>,
    allow_four_space: bool,
) -> impl Parser<'src, &'src str, (Hoon, Option<NounExpr>), Err<'src>> {
    hoon.map_with(|hoon: Hoon, e| (hoon, e.span().start(), e.span().end()))
        .map(move |(hoon, start, end)| {
            let Some((spaces, help)) = linemap.help_after_rune_with_spaces(start, end) else {
                return (hoon, None);
            };
            if spaces == 4 && !allow_four_space {
                return (hoon, Some(help));
            }
            let hoon = if hoon_tail_has_help(&hoon, &help) {
                hoon
            } else {
                Hoon::Note(Note::Help(help), Box::new(hoon))
            };
            (hoon, None)
        })
}

fn attach_rune_help(
    hoon: Hoon,
    start: usize,
    end: usize,
    linemap: &LineMap,
    allow_four_space: bool,
) -> Hoon {
    let Some((spaces, help)) = linemap.help_after_rune_with_spaces(start, end) else {
        return hoon;
    };
    if spaces == 4 && !allow_four_space {
        return hoon;
    }
    if hoon_tail_has_help(&hoon, &help) {
        hoon
    } else {
        Hoon::Note(Note::Help(help), Box::new(hoon))
    }
}

pub fn cenhep<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    let q_linemap = linemap.clone();
    gap()
        .ignore_then(documented_rune_body(hoon.clone(), linemap, true))
        .then_ignore(gap())
        .then(
            hoon.clone()
                .map_with(|q: Hoon, e| (q, e.span().start(), e.span().end())),
        )
        .map(move |(p, (q, start, end))| {
            let q = attach_rune_help(q, start, end, &q_linemap, false);
            Hoon::CenHep(Box::new(p), Box::new(q))
        })
}

pub fn cenhep_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::CenHep(Box::new(p), Box::new(q)))
}

pub fn cendot<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(p, q)| Hoon::CenDot(Box::new(p), Box::new(q)))
}

pub fn cenlus<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(documented_rune_body(hoon.clone(), linemap.clone(), true))
        .then_ignore(gap())
        .then(documented_rune_body_with_carry(
            hoon.clone(),
            linemap.clone(),
            false,
        ))
        .then_ignore(gap())
        .then(documented_rune_body(hoon.clone(), linemap, false))
        .map(|((p, (q, q_help)), r)| {
            let r = if let Some(help) = q_help {
                attach_help_to_hoon(r, help)
            } else {
                r
            };
            Hoon::CenLus(Box::new(p), Box::new(q), Box::new(r))
        })
}

pub fn cendot_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    two_hoons_wide(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::CenDot(Box::new(p), Box::new(q)))
}

pub fn cenlus_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    three_hoons_wide(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|((p, q), r)| Hoon::CenLus(Box::new(p), Box::new(q), Box::new(r)))
}

pub fn cencab<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(winglist())
        .then_ignore(gap())
        .then(list_wing_hoon_tall_with_docs(hoon.clone(), linemap))
        .then_ignore(just("=="))
        .map(|(p, q)| Hoon::CenCab(p, q))
}

pub fn cencab_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    winglist()
        .then_ignore(just(' '))
        .then(list_wing_hoon_wide(hoon_wide.clone()))
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::CenCab(p, q))
}

pub fn centar<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(winglist())
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .then(list_wing_hoon_tall_with_docs(hoon.clone(), linemap))
        .then_ignore(just("=="))
        .map(|((p, q), list)| Hoon::CenTar(p, Box::new(q), list))
}

pub fn centar_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    winglist()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(list_wing_hoon_wide(hoon_wide.clone()))
        .delimited_by(just('('), just(')'))
        .map(|((p, q), list)| Hoon::CenTar(p, Box::new(q), list))
}

pub fn centis<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(winglist())
        .then_ignore(gap())
        .then(list_wing_hoon_tall_with_docs(hoon.clone(), linemap))
        .then_ignore(just("=="))
        .map(|(name, list)| Hoon::CenTis(name, list))
}

pub fn centis_wide<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    winglist()
        .then_ignore(just(' '))
        .then(list_wing_hoon_wide(hoon.clone()))
        .delimited_by(just('('), just(')'))
        .map(|(name, list)| Hoon::CenTis(name, list))
}

pub fn censig<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(winglist())
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|((p, q), r)| Hoon::CenSig(p, Box::new(q), vec![r]))
}

pub fn censig_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    winglist()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|((p, q), r)| Hoon::CenSig(p, Box::new(q), vec![r]))
}

pub fn censig_irregular<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    winglist()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(list_hoon_wide(hoon_wide.clone()))
        .delimited_by(just('('), just(')'))
        .map(|((w, h), list)| Hoon::CenSig(w, Box::new(h), list))
}

pub fn centis_irregular<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    winglist()
        .then(list_wing_hoon_wide(hoon_wide.clone()).delimited_by(just('('), just(')')))
        .map(|(name, list)| Hoon::CenTis(name, list))
}

fn line_start_spec_arg_with_doc<'src>(
    spec: impl ParserExt<'src, Spec>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    spec.map_with(move |spec: Spec, e| {
        let start = e.span().start();
        if let Some(help) = linemap.help_after_line_start_expr(start) {
            attach_help_to_spec(spec, help)
        } else {
            spec
        }
    })
}

pub fn cenlus_spec<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    let q = line_start_spec_arg_with_doc(spec.clone(), linemap.clone());
    let r = line_start_spec_arg_with_doc(spec.clone(), linemap);
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(q)
        .then_ignore(gap())
        .then(r)
        .map(|((p, q), r)| Spec::Make(p, vec![q, r]))
}

pub fn cenlus_spec_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(spec_wide.clone())
        .then_ignore(just(' '))
        .then(spec_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|((p, q), r)| Spec::Make(p, vec![q, r]))
}

pub fn cenket_spec<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(spec.clone())
        .then_ignore(gap())
        .then(spec.clone())
        .then_ignore(gap())
        .then(spec.clone())
        .map(|(((p, q), r), s)| Spec::Make(p, vec![q, r, s]))
}

pub fn cenket_spec_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(spec_wide.clone())
        .then_ignore(just(' '))
        .then(spec_wide.clone())
        .then_ignore(just(' '))
        .then(spec_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(((p, q), r), s)| Spec::Make(p, vec![q, r, s]))
}

pub fn cendot_spec<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    gap()
        .ignore_then(spec.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(p, q)| Spec::Make(q, vec![p]))
}

pub fn cendot_spec_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    spec_wide
        .clone()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Spec::Make(q, vec![p]))
}

pub fn cencol_spec<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(list_spec_closed_tall(spec.clone()))
        .map(|(p, q)| Spec::Make(p, q))
}

pub fn cencol_spec_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(
            spec_wide
                .clone()
                .separated_by(just(' '))
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Spec::Make(p, q))
}

pub fn cenhep_spec<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    hoon_spec_tall(hoon.clone(), spec.clone()).map(|(p, q)| Spec::Make(p, vec![q]))
}

pub fn cenhep_spec_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    hoon_spec_wide(hoon_wide.clone(), spec_wide.clone()).map(|(p, q)| Spec::Make(p, vec![q]))
}
