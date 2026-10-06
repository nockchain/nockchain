use std::sync::Arc;

use chumsky::prelude::*;

use crate::ast::hoon::*;
use crate::utils::*;

/// The import block at the top of a file: `/-` `/+` `/=` `/*` `/#` `/?` `/%`
/// lines, their indented continuation lines, and the whitespace between
/// them. hoonc parses the block apart from the body (`+pile-rule`), so it is
/// only skipped at the start of a file, never where a tall hoon may start
/// (there `/-0x10` or `/=/foo` is a path). A rune must be followed by
/// whitespace to open a clause.
pub fn import_header<'src>() -> impl Parser<'src, &'src str, (), Err<'src>> + Clone {
    let line_tail = any().and_is(text::newline().not()).repeated();
    let line_end = text::newline().or_not();

    let import_line = just('/')
        .ignore_then(one_of("-+=*#?%"))
        .then_ignore(one_of(" \t\r\n").ignored().or(end()).rewind())
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

    import_line
        .then_ignore(gap().repeated())
        .repeated()
        .at_least(1)
        .boxed()
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
