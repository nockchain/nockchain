use std::collections::*;
use std::sync::Arc;

use chumsky::input::{StrInput, Stream};
use chumsky::prelude::*;

use crate::ast::hoon::*;
use crate::utils::*;

pub fn bar_runes_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> + 'src {
    choice((
        just('%').ignore_then(barcen(hoon.clone(), spec.clone(), linemap.clone())),
        just('.').ignore_then(bardot(hoon.clone())),
        just('*').ignore_then(bartar(hoon.clone(), spec.clone())),
        just('_').ignore_then(barcab(hoon.clone(), spec.clone(), linemap.clone())),
        just('@').ignore_then(barpat(hoon.clone(), spec.clone(), linemap.clone())),
        just('=').ignore_then(bartis(hoon.clone(), spec.clone(), linemap.clone())),
        just('~').ignore_then(barsig(hoon.clone(), spec.clone())),
        just('-').ignore_then(barhep(hoon.clone(), linemap.clone())),
        just('^').ignore_then(barket(hoon.clone(), spec.clone(), linemap.clone())),
        just(':').ignore_then(barcol(hoon.clone())),
        just('$').ignore_then(barbuc(spec.clone(), linemap.clone())),
        just('?').ignore_then(barwut(hoon.clone())),
    ))
}

pub fn bar_runes_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just('.').ignore_then(bardot_wide(hoon_wide.clone())),
        just('*').ignore_then(bartar_wide(hoon_wide.clone(), spec_wide.clone())),
        just('=').ignore_then(bartis_wide(hoon_wide.clone(), spec_wide.clone())),
        just('~').ignore_then(barsig_wide(hoon_wide.clone(), spec_wide.clone())),
        just('-').ignore_then(barhep_wide(hoon_wide.clone())),
        just('^').ignore_then(barket_wide(
            hoon_wide.clone(),
            spec_wide.clone(),
            linemap.clone(),
        )),
        just(':').ignore_then(barcol_wide(hoon_wide.clone())),
        just('$').ignore_then(barbuc_wide(spec_wide.clone(), linemap.clone())),
        just('?').ignore_then(barwut_wide(hoon_wide.clone())),
    ))
}

fn barcen<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(chapters(hoon.clone(), spec.clone(), linemap, true))
        .map(|map_term_tome| Hoon::BarCen(None, map_term_tome))
}

fn bardot<'src>(hoon: impl ParserExt<'src, Hoon>) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .map(|p| Hoon::BarDot(Box::new(p)))
}

fn bardot_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .delimited_by(just('('), just(')'))
        .map(|p| Hoon::BarDot(Box::new(p)))
}

pub fn bartar<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec> + Clone,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(spec.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(s, h)| Hoon::BarTar(Box::new(s), Box::new(h)))
}

pub fn bartar_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec> + Clone,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    spec_wide
        .clone()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(s, h)| Hoon::BarTar(Box::new(s), Box::new(h)))
}

pub fn barsig<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec> + Clone,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(spec.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(s, h)| Hoon::BarSig(Box::new(s), Box::new(h)))
}

pub fn barsig_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    spec_wide
        .clone()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(s, h)| Hoon::BarSig(Box::new(s), Box::new(h)))
}

pub fn bartis<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec> + Clone,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(
            spec.clone()
                .map_with(|s: Spec, e| (s, e.span().start(), e.span().end())),
        )
        .then_ignore(gap())
        .then(hoon.clone())
        .map(move |((s, start, end), h)| {
            if let Some((spaces, help)) = linemap.help_after_rune_with_spaces(start, end) {
                if spaces == 4 {
                    return Hoon::BarTis(Box::new(s), Box::new(attach_help_to_hoon(h, help)));
                }
                return Hoon::BarTis(Box::new(attach_help_to_spec(s, help)), Box::new(h));
            }
            let s = if let Some(help) = linemap.help_after_line_expr_ending_at(end) {
                attach_help_to_spec(s, help)
            } else {
                s
            };
            Hoon::BarTis(Box::new(s), Box::new(h))
        })
}

fn bartis_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    spec_wide
        .clone()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(s, h)| Hoon::BarTis(Box::new(s), Box::new(h)))
}

pub fn barbuc<'src>(
    spec: impl ParserExt<'src, Spec>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(barbuc_sample_tall())
        .then_ignore(gap())
        .then(
            spec.clone()
                .map_with(|body: Spec, e| (body, e.span().start(), e.span().end())),
        )
        .map(move |(list, (body, body_start, body_end))| {
            let body = if let Some(help) = linemap.help_before_body_spec(body_start) {
                Spec::Gist(help, Box::new(body))
            } else if let Some(help) = linemap.help_after_rune(body_start, body_end) {
                // Postfix `::` on a one-line `|$ sample body  :: doc` anchors to
                // the body spec (hoonc emits the `%gist` there, e.g. `++ jar`).
                Spec::Gist(help, Box::new(body))
            } else {
                body
            };
            Hoon::BarBuc(list, Box::new(body))
        })
}

pub fn barbuc_sample_tall<'src>() -> impl Parser<'src, &'src str, Vec<String>, Err<'src>> {
    choice((
        list_names_tall(),
        list_names_wide(),
        symbol().map(|s| vec![s]),
    ))
}

pub fn barbuc_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    barbuc_sample_wide()
        .then_ignore(just(' '))
        .then(
            spec_wide
                .clone()
                .map_with(|body: Spec, e| (body, e.span().start())),
        )
        .delimited_by(just('('), just(')'))
        .map(move |(list, (body, body_start))| {
            let body = if let Some(help) = linemap.help_before_body_spec(body_start) {
                Spec::Gist(help, Box::new(body))
            } else {
                body
            };
            Hoon::BarBuc(list, Box::new(body))
        })
}

pub fn barbuc_sample_wide<'src>() -> impl Parser<'src, &'src str, Vec<String>, Err<'src>> {
    choice((list_names_wide(), symbol().map(|s| vec![s])))
}

pub fn barcol<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(s, h)| Hoon::BarCol(Box::new(s), Box::new(h)))
}

pub fn barhep<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(
            hoon.clone()
                .map_with(|h: Hoon, e| (h, e.span().start(), e.span().end())),
        )
        .map(move |(h, h_start, h_end)| {
            let h = if let Some(help) = linemap.help_after_rune(h_start, h_end) {
                if hoon_tail_has_help(&h, &help) {
                    h
                } else {
                    Hoon::Note(Note::Help(help), Box::new(h))
                }
            } else {
                h
            };
            Hoon::BarHep(Box::new(h))
        })
}

pub fn barhep_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .delimited_by(just('('), just(')'))
        .map(|h| Hoon::BarHep(Box::new(h)))
}

pub fn barwut_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .delimited_by(just('('), just(')'))
        .map(|h| Hoon::BarWut(Box::new(h)))
}

pub fn barket<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(chapters(hoon.clone(), spec.clone(), linemap, true))
        .map(|(h, map_term_tome)| Hoon::BarKet(Box::new(h), map_term_tome))
}

pub fn barket_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(chapters(
            hoon_wide.clone(),
            spec_wide.clone(),
            linemap,
            true,
        ))
        .delimited_by(just('('), just(')'))
        .map(|(h, map_term_tome)| Hoon::BarKet(Box::new(h), map_term_tome))
}

pub fn barpat<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(chapters(hoon.clone(), spec.clone(), linemap, true))
        .map(|map_term_tome| Hoon::BarPat(None, map_term_tome))
}

pub fn barcab<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    let aliases =     //   +*  foo  1
                just("+*")
                    .ignore_then(gap())
                    .ignore_then(list_term_hoon(hoon.clone()));

    gap()
        .ignore_then(spec.clone())
        .then_ignore(gap())
        .then(aliases.or_not().map(|x| x.unwrap_or(vec![])))
        .then(chapters(hoon.clone(), spec.clone(), linemap, true))
        .map(|((spec, alas), map_term_tome)| Hoon::BarCab(Box::new(spec), alas, map_term_tome))
}

pub fn barcol_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::BarCol(Box::new(p), Box::new(q)))
}

pub fn barwut<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .map(|p| Hoon::BarWut(Box::new(p)))
}
