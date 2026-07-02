use std::collections::*;
use std::sync::Arc;

use chumsky::input::{Stream, ValueInput};
use chumsky::prelude::*;

use crate::ast::hoon::*;
use crate::utils::*;

pub fn ket_runes_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just("|").ignore_then(ketbar(hoon.clone())),
        just('.').ignore_then(ketdot(hoon.clone())),
        just(':').ignore_then(ketcol(spec.clone())),
        just('-').ignore_then(kethep(hoon.clone(), spec.clone(), linemap.clone())),
        just("+").ignore_then(ketlus(hoon.clone(), linemap.clone())),
        just("&").ignore_then(ketpam(hoon.clone())),
        just('~').ignore_then(ketsig(hoon.clone())),
        just('=').ignore_then(kettis(hoon.clone(), linemap.clone())),
        just('?').ignore_then(ketwut(hoon.clone())),
        just('*').ignore_then(kettar(spec.clone())),
    ))
}

pub fn ket_runes_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just('~').ignore_then(ketsig_wide(hoon_wide.clone())),
        just("+").ignore_then(ketlus_wide(hoon_wide.clone())),
        just('.').ignore_then(ketdot_wide(hoon_wide.clone())),
        just(':').ignore_then(ketcol_wide(spec_wide.clone())),
        just('-').ignore_then(kethep_wide(hoon_wide.clone(), spec_wide.clone())),
        just("|").ignore_then(ketbar_wide(hoon_wide.clone())),
        just("&").ignore_then(ketpam_wide(hoon_wide.clone())),
        just('=').ignore_then(kettis_wide(hoon_wide.clone())),
        just('?').ignore_then(ketwut_wide(hoon_wide.clone())),
        just('*').ignore_then(kettar_wide(spec_wide.clone())),
    ))
}

pub fn ketdot<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(p, q)| Hoon::KetDot(Box::new(p), Box::new(q)))
}

pub fn ketdot_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::KetDot(Box::new(p), Box::new(q)))
}

pub fn ketbar<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .map(|p| Hoon::KetBar(Box::new(p)))
}

pub fn ketbar_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .delimited_by(just('('), just(')'))
        .map(|p| Hoon::KetBar(Box::new(p)))
}

pub fn ketpam<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .map(|p| Hoon::KetPam(Box::new(p)))
}

pub fn ketpam_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .delimited_by(just('('), just(')'))
        .map(|p| Hoon::KetPam(Box::new(p)))
}

pub fn ketsig<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .map(|p| Hoon::KetSig(Box::new(p)))
}

pub fn ketwut<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .map(|p| Hoon::KetWut(Box::new(p)))
}

pub fn ketwut_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .delimited_by(just('('), just(')'))
        .map(|p| Hoon::KetWut(Box::new(p)))
}

pub fn kettis<'src>(
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
        .validate(move |((p, p_start, p_end), q), e, emit| {
            let doc_end = match &p {
                Hoon::Limb(term) => p_start + term.len(),
                Hoon::Wing(w) => match w.as_slice() {
                    [Limb::Term(term)] => p_start + term.len(),
                    _ => p_end,
                },
                _ => p_end,
            };
            let p = if let Some(help) = linemap.help_after_rune(p_start, doc_end) {
                if matches!(&p, Hoon::Note(Note::Help(existing), _) if existing == &help) {
                    p
                } else {
                    Hoon::Note(Note::Help(help), Box::new(p))
                }
            } else {
                p
            };
            let maybe_skin = flay(p);
            match maybe_skin {
                Some(s) => Hoon::KetTis(s, Box::new(q)),
                None => {
                    emit.emit(Rich::custom(e.span(), "Invalid variable declaration."));
                    Hoon::KetTis(Skin::Term("error".to_string()), Box::new(Hoon::ZapZap))
                }
            }
        })
}

pub fn kettis_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .try_map(|(p, q), span| {
            let maybe_skin = flay(p);
            match maybe_skin {
                Some(s) => Ok(Hoon::KetTis(s, Box::new(q))),
                None => Err(Rich::custom(span, "Invalid variable declaration.")),
            }
        })
}

pub fn kettis_irregular<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    spec_wide.clone().try_map(|spec, span| {
        let auto = autoname(spec.clone());
        match auto {
            None => Err(Rich::custom(span, "cannot name spec")),
            Some(auto_term) => Ok(Hoon::KetTis(
                Skin::Term(auto_term.to_string()),
                Box::new(Hoon::KetTar(Box::new(spec.clone()))),
            )),
        }
    })
}

pub fn kethep<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
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
                    return Hoon::KetHep(Box::new(s), Box::new(attach_help_to_hoon(h, help)));
                }
                return Hoon::KetHep(Box::new(attach_help_to_spec(s, help)), Box::new(h));
            }
            let s = if let Some(help) = linemap.help_after_line_expr_ending_at(end) {
                attach_help_to_spec(s, help)
            } else {
                s
            };
            Hoon::KetHep(Box::new(s), Box::new(h))
        })
}

pub fn kethep_wide<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    spec.clone()
        .then_ignore(just(' '))
        .then(hoon.clone())
        .delimited_by(just('('), just(')'))
        .map(|(s, h)| Hoon::KetHep(Box::new(s), Box::new(h)))
}

pub fn ketsig_wide<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon.clone()
        .delimited_by(just('('), just(')'))
        .map(|h| Hoon::KetSig(Box::new(h)))
}

pub fn ketlus<'src>(
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
            let p = if let Some(help) = linemap.help_after_rune(p_start, p_end) {
                if hoon_tail_has_help(&p, &help) {
                    p
                } else {
                    Hoon::Note(Note::Help(help), Box::new(p))
                }
            } else {
                p
            };
            Hoon::KetLus(Box::new(p), Box::new(q))
        })
}

pub fn ketlus_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::KetLus(Box::new(p), Box::new(q)))
}

pub fn kettar_irregular<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    just('*')
        .ignore_then(spec_wide.clone())
        .map(|s| Hoon::KetTar(Box::new(s)))
}

pub fn kethep_irregular<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    spec_wide
        .clone()
        .then_ignore(just("`"))
        .then(hoon_wide.clone())
        .map(|(s, w)| Hoon::KetHep(Box::new(s), Box::new(w)))
}

pub fn kethep_noun_irregular<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    just('*')
        .ignore_then(just('`'))
        .ignore_then(hoon_wide.clone())
        .map(|w| Hoon::KetHep(Box::new(Spec::Base(BaseType::NounExpr)), Box::new(w)))
}

pub fn ketlus_irregular<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    just('+')
        .ignore_then(hoon_wide.clone())
        .then_ignore(just("`"))
        .then(hoon_wide.clone())
        .map(|(p, q)| Hoon::KetLus(Box::new(p), Box::new(q)))
}

pub fn ketcol_irregular<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    just(",")
        .ignore_then(spec_wide.clone())
        .map(|p| Hoon::KetCol(Box::new(p)))
}

pub fn kettar<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(spec.clone())
        .map(|s| Hoon::KetTar(Box::new(s)))
}

pub fn kettar_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    one_spec_closed_wide(spec_wide.clone()).map(|s| Hoon::KetTar(Box::new(s)))
}

pub fn ketcol<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(spec.clone())
        .map(|s| Hoon::KetCol(Box::new(s)))
}

pub fn ketcol_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    one_spec_closed_wide(spec_wide.clone()).map(|s| Hoon::KetCol(Box::new(s)))
}
