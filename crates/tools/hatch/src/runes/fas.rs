use std::sync::Arc;

use chumsky::prelude::*;

use crate::ast::hoon::*;
use crate::utils::*;

pub fn fas_runes_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
    wer: Path,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    let line_tail = any().and_is(text::newline().not()).repeated();
    let line_end = text::newline().or_not();

    // /+ and related import runes can spill onto indented continuation lines.
    let import_line_tail = one_of("-+=*#?%")
        .ignore_then(line_tail.clone())
        .then_ignore(line_end.clone())
        .then(
            one_of(" \t")
                .repeated()
                .at_least(1)
                .ignore_then(line_tail.clone())
                .then_ignore(line_end.clone())
                .repeated(),
        )
        .ignored();

    let skip_imports = import_line_tail
        .then(
            just('/')
                .ignore_then(import_line_tail.clone())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .ignored()
        .then_ignore(gap().repeated())
        .ignore_then(hoon.clone());

    let _ = (hoon_wide, wer, linemap);
    skip_imports.boxed()
}

pub fn fastis<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .ignore_then(gap())
        .ignore_then(hoon_wide.clone())
        .ignore_then(gap())
        .ignore_then(hoon.clone())
}

pub fn fastar<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon_wide.clone())
        .ignore_then(gap())
        .ignore_then(hoon_wide.clone())
        .ignore_then(gap())
        .ignore_then(hoon_wide.clone())
        .ignore_then(gap())
        .ignore_then(hoon.clone())
}

pub fn fashax<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon_wide.clone())
        .ignore_then(gap())
        .ignore_then(hoon.clone())
}
