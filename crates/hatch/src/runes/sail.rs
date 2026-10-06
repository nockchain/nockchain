//  Sail, hoon-138 `++sail`: after a `;`, a tall-form (`++tall-top`) or
//  wide-form (`++wide-top`) XML template. A node becomes `[%xray manx]`, a
//  node list `[%mcts marl]`. Markdown (`++cram`: `;>`, and bare text lines
//  among an element's tall children) is not supported.

use std::sync::Arc;

use chumsky::extra::ParserExtra;
use chumsky::prelude::*;

use crate::ast::hoon::*;
use crate::utils::*;

#[path = "cram.rs"]
mod cram;

type Boxed<'src, O> = chumsky::Boxed<'src, 'src, &'src str, O, Err<'src>>;

/// What sail needs to reparse markdown text: the file path and whether
/// hoons carry dbug spots.
#[derive(Clone)]
pub struct SailCtx {
    wer: Path,
    trace: bool,
}

impl SailCtx {
    pub fn new(wer: Path, trace: bool) -> Self {
        Self { wer, trace }
    }
}

/// A node or a node list, hoon-138 `(each tuna marl)`.
#[derive(Clone)]
enum Top {
    One(Tuna),
    Many(Marl),
}

/// An element of `++quote-innards`: a text byte, an embedded node, or, in a
/// `"""` block, a newline with the count of spaces that follow it.
#[derive(Clone)]
enum Innard {
    Byte(u8),
    Tuna(Tuna),
    Newline(usize),
}

fn drop_top(top: Top) -> Marl {
    match top {
        Top::One(tuna) => vec![tuna],
        Top::Many(marl) => marl,
    }
}

fn join_tops(tops: Vec<Top>) -> Marl {
    tops.into_iter().flat_map(drop_top).collect()
}

//  hoon-138 keeps one beer char per text byte. A beer char cord stores a
//  byte as the char with that code point.
fn byte_beer(byte: u8) -> Beer {
    Beer::Char(char::from(byte).to_string())
}

fn utf8_bytes(c: char) -> Vec<u8> {
    let mut buf = [0u8; 4];
    c.encode_utf8(&mut buf).as_bytes().to_vec()
}

/// The bytes of a cord (`++trip`).
fn cord_bytes(atom: &ParsedAtom) -> Vec<u8> {
    let big = atom.to_biguint();
    if big == 0u32.into() {
        Vec::new()
    } else {
        big.to_bytes_le()
    }
}

//  A woof atom holds text bytes; split any multi-byte one rather than
//  reading it as a Unicode scalar.
fn woof_to_beers(woof: Woof) -> Vec<Beer> {
    match woof {
        Woof::ParsedAtom(atom) => atom
            .to_biguint()
            .to_bytes_le()
            .into_iter()
            .map(byte_beer)
            .collect(),
        Woof::Hoon(hoon) => vec![Beer::Hoon(hoon)],
    }
}

/// `++hopefully-quote`: the text of a tape, else the hoon itself.
fn hoon_to_beers(hoon: Hoon) -> Vec<Beer> {
    match hoon {
        Hoon::Knit(woofs) => woofs.into_iter().flat_map(woof_to_beers).collect(),
        other => vec![Beer::Hoon(other)],
    }
}

fn text_beers(text: &str) -> Vec<Beer> {
    text.bytes().map(byte_beer).collect()
}

/// An element head with no attributes, `[name ~]`.
fn tag(name: &str) -> Marx {
    Marx {
        n: Mane::Tag(name.to_string()),
        a: vec![],
    }
}

/// `;/(tape)`: a text node, `[[%$ [%$ tape] ~] ~]`.
fn text_node(bytes: &[u8]) -> Tuna {
    let text = Mane::Tag(String::new());
    Tuna::Manx(Manx {
        g: Marx {
            n: text.clone(),
            a: vec![(text, bytes.iter().copied().map(byte_beer).collect())],
        },
        c: vec![],
    })
}

/// `++collapse-chars`: runs of text bytes become text nodes. In tall form
/// the last run loses its trailing spaces and gains a newline.
fn collapse_chars(innards: Vec<Innard>, tall: bool) -> Marl {
    let mut out = Vec::new();
    let mut run: Vec<u8> = Vec::new();
    for innard in innards {
        match innard {
            Innard::Byte(byte) => run.push(byte),
            Innard::Newline(_) => run.push(b'\n'),
            Innard::Tuna(tuna) => {
                if !run.is_empty() {
                    out.push(text_node(&run));
                    run.clear();
                }
                out.push(tuna);
            }
        }
    }
    if tall {
        while run.last() == Some(&b' ') {
            run.pop();
        }
        run.push(b'\n');
    }
    if !run.is_empty() {
        out.push(text_node(&run));
    }
    out
}

/// `++apex`: a node is `%xray`, a node list `%mcts`.
fn apex(top: Top) -> Hoon {
    match top {
        Top::One(Tuna::Manx(manx)) => Hoon::Xray(manx),
        Top::One(tuna) => Hoon::MicTis(vec![tuna]),
        Top::Many(marl) => Hoon::MicTis(marl),
    }
}

fn mixed_case_symbol<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> + Clone {
    any()
        .filter(|c: &char| c.is_ascii_alphabetic())
        .then(
            any()
                .filter(|c: &char| c.is_ascii_alphanumeric() || *c == '-')
                .repeated()
                .collect::<Vec<char>>(),
        )
        .map(|(first, rest)| {
            let mut out = String::with_capacity(rest.len() + 1);
            out.push(first);
            out.extend(rest);
            out
        })
        .labelled("Sail Symbol")
}

/// `++a-mane`: `name` or `space_name`.
fn mane_parser<'src>() -> impl Parser<'src, &'src str, Mane, Err<'src>> + Clone {
    mixed_case_symbol()
        .then(just('_').ignore_then(mixed_case_symbol()).or_not())
        .map(|(base, suffix)| match suffix {
            Some(ns) => Mane::TagSpace(base, ns),
            None => Mane::Tag(base),
        })
}

/// `++tuna-mode`
fn tuna_mode<'src>() -> impl Parser<'src, &'src str, fn(Hoon) -> TunaTail, Err<'src>> + Clone {
    choice((
        just('-').to(TunaTail::Tape as fn(Hoon) -> TunaTail),
        just('+').to(TunaTail::Manx as fn(Hoon) -> TunaTail),
        just('*').to(TunaTail::Marl as fn(Hoon) -> TunaTail),
        just('%').to(TunaTail::Call as fn(Hoon) -> TunaTail),
    ))
}

/// `++bix:ab`: two lowercase hex digits.
fn hex_byte<'src, E: ParserExtra<'src, &'src str>>() -> impl Parser<'src, &'src str, u8, E> + Clone
{
    let six = any().filter(|c: &char| matches!(c, '0'..='9' | 'a'..='f'));
    six.then(six).map(|(hi, lo)| {
        (hi.to_digit(16).expect("hex digit") * 16 + lo.to_digit(16).expect("hex digit")) as u8
    })
}

/// `++prn`: a character of non-control bytes.
fn is_prn(c: char) -> bool {
    c >= ' ' && c != '\u{7f}'
}

/// `++sump`: `{hoon hoon ...}` as `%cltr`.
fn sump<'src>(hoon_wide: Boxed<'src, Hoon>) -> Boxed<'src, Hoon> {
    hoon_wide
        .separated_by(just(' '))
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(just('{'), just('}'))
        .map(Hoon::ColTar)
        .boxed()
}

/// An element of `++quote-innards` other than a newline: an escape, an
/// embedded node, or text (where in tall form `"` is text).
fn innard<'src, E, P>(embed: P, tall: bool) -> impl Parser<'src, &'src str, Vec<Innard>, E> + Clone
where
    E: ParserExtra<'src, &'src str>,
    P: Parser<'src, &'src str, Tuna, E> + Clone,
{
    let escape = just('\\')
        .ignore_then(choice((
            one_of("-+*%;{\\\"").map(|c: char| c as u8),
            hex_byte(),
        )))
        .map(|byte| vec![Innard::Byte(byte)]);
    let embed = embed.map(|tuna| vec![Innard::Tuna(tuna)]);
    let text = any()
        .filter(move |c: &char| is_prn(*c) && *c != '\\' && *c != '{' && (tall || *c != '"'))
        .map(|c| utf8_bytes(c).into_iter().map(Innard::Byte).collect());
    choice((escape, embed, text))
}

/// `++quote-innards` on one line.
fn quote_innards<'src>(inline_embed: Boxed<'src, Tuna>, tall: bool) -> Boxed<'src, Vec<Innard>> {
    innard(inline_embed, tall)
        .repeated()
        .collect::<Vec<Vec<Innard>>>()
        .map(|chunks| chunks.into_iter().flatten().collect())
        .boxed()
}

type BlockErr<'src> = extra::Full<Rich<'src, char>, (), usize>;

/// `++quote-innards` in a `"""` block whose `"""` is `lev` columns in (the
/// parser context). A newline is kept with the indentation of the next
/// line, unless that line is the closing `"""`: `lev` spaces and `"""`.
fn block_innards<'src>(
    inline_embed: Boxed<'src, Tuna>,
    tall: bool,
) -> impl Parser<'src, &'src str, Vec<Innard>, BlockErr<'src>> + Clone {
    let newline = just('\n')
        .ignore_then(just(' ').repeated().count())
        .then(just("\"\"\"").rewind().or_not())
        .map_with(|(spaces, close), extra| (spaces, close.is_some(), *extra.ctx()))
        .try_map(|(spaces, close, lev), span| {
            if close && spaces == lev {
                Err(Rich::custom(span, "end of sail block"))
            } else {
                Ok(vec![Innard::Newline(spaces)])
            }
        });
    choice((innard(inline_embed.with_ctx(()), tall), newline))
        .repeated()
        .collect::<Vec<Vec<Innard>>>()
        .map(|chunks| chunks.into_iter().flatten().collect())
}

/// Resolve a `"""` block's indentation as hoon-138 `++inde` does: every line
/// is indented at least as far as the opening `"""` (that much is dropped)
/// or is empty, and the closing `"""` is indented exactly that far.
fn dedent_block(
    lev: usize,
    first: usize,
    innards: Vec<Innard>,
    close: usize,
) -> Result<Vec<Innard>, &'static str> {
    let spaces = |count: usize| std::iter::repeat(Innard::Byte(b' ')).take(count - lev);
    //  a line may also be empty: its newline is followed by another
    let next_is_newline = |i: usize| matches!(innards.get(i), Some(Innard::Newline(_)) | None);
    let indented = |count: usize, i: usize| count >= lev || (count == 0 && next_is_newline(i));
    if close != lev {
        return Err("the closing \"\"\" of a sail block is not aligned with the opening");
    }
    if !indented(first, 0) {
        return Err("a line of a sail block is indented less than its \"\"\"");
    }
    let mut out = Vec::with_capacity(innards.len());
    if first >= lev {
        out.extend(spaces(first));
    }
    for (i, innard) in innards.iter().enumerate() {
        match innard {
            Innard::Newline(count) => {
                if !indented(*count, i + 1) {
                    return Err("a line of a sail block is indented less than its \"\"\"");
                }
                out.push(Innard::Byte(b'\n'));
                if *count >= lev {
                    out.extend(spaces(*count));
                }
            }
            other => out.push(other.clone()),
        }
    }
    Ok(out)
}

/// `++wide-quote`: `"text"`, or a `"""` block.
fn wide_quote<'src>(
    inline_embed: Boxed<'src, Tuna>,
    tall: bool,
    linemap: Arc<LineMap>,
) -> Boxed<'src, Marl> {
    let single = just("\"\"\"")
        .not()
        .ignore_then(quote_innards(inline_embed.clone(), tall).delimited_by(just('"'), just('"')))
        .map(move |innards| collapse_chars(innards, tall));
    let open = just("\"\"\"").map_with(move |_, extra| {
        let span: SimpleSpan = extra.span();
        linemap.raw_column(span.start)
    });
    let indent = just(' ').repeated().count();
    let body = indent
        .then(block_innards(inline_embed, tall))
        .then(just('\n').ignore_then(indent).then_ignore(just("\"\"\"")));
    let block = open.then_ignore(just('\n')).then_with_ctx(body).try_map(
        move |(lev, ((first, innards), close)), span| {
            dedent_block(lev, first, innards, close)
                .map(|innards| collapse_chars(innards, tall))
                .map_err(|msg| Rich::custom(span, msg))
        },
    );
    choice((single, block)).boxed()
}

/// The sail parsers `++tall-top` and `++wide-top` (each run after the
/// leading `;`), and `++inline-embed`.
struct Sail<'src> {
    tall_top: Boxed<'src, Top>,
    wide_top: Boxed<'src, Top>,
    inline_embed: Boxed<'src, Tuna>,
}

fn sail_parsers<'src>(
    hoon: Boxed<'src, Hoon>,
    hoon_wide: Boxed<'src, Hoon>,
    linemap: Arc<LineMap>,
    ctx: SailCtx,
) -> Sail<'src> {
    let mut wide_top = Recursive::declare();
    let mut tall_top = Recursive::declare();

    let sump = sump(hoon_wide.clone());

    //  ++wide-attrs: `(name value, name value)`
    let attribute = mane_parser()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .map(|(name, value)| (name, hoon_to_beers(value)));
    let wide_attrs = attribute
        .separated_by(just(", "))
        .collect::<Vec<_>>()
        .delimited_by(just('('), just(')'))
        .or_not()
        .map(Option::unwrap_or_default)
        .boxed();

    //  ++tag-head: name, #id, .classes, /"href" or @"src", then attributes
    let id = just('#')
        .ignore_then(symbol())
        .map(|id| (Mane::Tag("id".to_string()), text_beers(&id)));
    let classes = just('.')
        .ignore_then(symbol())
        .repeated()
        .collect::<Vec<_>>()
        .map(|classes| {
            (!classes.is_empty()).then(|| {
                (
                    Mane::Tag("class".to_string()),
                    text_beers(&classes.join(" ")),
                )
            })
        });
    let link = choice((just('/').to("href"), just('@').to("src")))
        .then(soil(hoon_wide.clone(), linemap.clone()))
        .map(|(name, woofs)| {
            let beers = woofs.into_iter().flat_map(woof_to_beers).collect();
            (Mane::Tag(name.to_string()), beers)
        });
    let tag_head = mane_parser()
        .then(id.or_not())
        .then(classes)
        .then(link.or_not())
        .then(wide_attrs.clone())
        .map(|((((name, id), class), link), attrs)| {
            let mut mart: Mart = [id, class, link].into_iter().flatten().collect();
            mart.extend(attrs);
            Marx { n: name, a: mart }
        })
        .boxed();

    //  ++wide-inner-top, ++wide-elems, ++wide-paren-elems
    let wide_inner_top = choice((
        wide_top.clone(),
        tuna_mode()
            .then(hoon_wide.clone())
            .map(|(mode, hoon)| Top::One(Tuna::TunaTail(mode(hoon)))),
    ))
    .boxed();
    let wide_elems = just(' ')
        .ignore_then(wide_inner_top.clone())
        .repeated()
        .collect::<Vec<_>>()
        .map(join_tops);
    let wide_paren_elems = wide_inner_top
        .separated_by(just(' '))
        .collect::<Vec<_>>()
        .delimited_by(just('('), just(')'))
        .map(join_tops)
        .boxed();

    //  ++bracketed-elem and ++inline-embed
    let bracketed_elem = tag_head
        .clone()
        .then(wide_elems)
        .delimited_by(just('{'), just('}'))
        .map(|(g, c)| Manx { g, c });
    let inline_embed = choice((
        just(';').ignore_then(bracketed_elem).map(Tuna::Manx),
        tuna_mode()
            .then(sump.clone())
            .map(|(mode, hoon)| Tuna::TunaTail(mode(hoon))),
        sump.map(|hoon| Tuna::TunaTail(TunaTail::Tape(hoon))),
    ))
    .boxed();

    //  ++wrapped-elems and ++wide-tail
    let cord_node = cord(linemap.clone()).map(|atom| vec![text_node(&cord_bytes(&atom))]);
    let wrapped_elems = choice((
        wide_paren_elems.clone(),
        cord_node,
        wide_top.clone().map(drop_top),
    ))
    .boxed();
    let wide_tail = choice((
        just(':').ignore_then(wrapped_elems.clone()),
        just(';').to(Vec::new()),
        empty().to(Vec::new()),
    ));

    //  ++wide-top
    wide_top.define(
        choice((
            wide_quote(inline_embed.clone(), false, linemap.clone()).map(Top::Many),
            wide_paren_elems.map(Top::Many),
            tag_head
                .clone()
                .then(wide_tail)
                .map(|(g, c)| Top::One(Tuna::Manx(Manx { g, c }))),
        ))
        .boxed(),
    );

    //  ++cram: markdown, as a list of children or in a `;>` block
    let expr = just(' ')
        .repeated()
        .ignore_then(just(';'))
        .ignore_then(tall_top.clone())
        .then_ignore(gap().rewind())
        .map(drop_top)
        .boxed();
    let cram = cram::cram(expr, linemap.clone(), ctx);

    //  ++tall-tail and ++tall-kids
    let top_level = just(';').ignore_then(tall_top.clone());
    let tall_kids = choice((top_level, cram.clone().map(Top::Many)))
        .separated_by(gap())
        .at_least(1)
        .collect::<Vec<_>>()
        .map(join_tops);
    let tall_tail = choice((
        just(';').to(Vec::new()),
        just(':').ignore_then(wrapped_elems),
        just(": ")
            .ignore_then(quote_innards(inline_embed.clone(), true))
            .map(|innards| collapse_chars(innards, false)),
        gap()
            .ignore_then(tall_kids)
            .then_ignore(gap())
            .then_ignore(just("==")),
    ))
    .boxed();

    //  ++tall-attrs and ++tall-elem
    let tall_attrs = gap()
        .then(just('='))
        .ignore_then(mane_parser())
        .then_ignore(gap())
        .then(hoon_wide.clone())
        .map(|(name, value)| (name, hoon_to_beers(value)))
        .repeated()
        .collect::<Vec<_>>();
    let tall_elem = tag_head
        .then(tall_attrs)
        .then(tall_tail.clone())
        .map(|((mut g, attrs), c)| {
            g.a.extend(attrs);
            Manx { g, c }
        });

    //  ++script-or-style and ++script-style-tail: `;` lines of raw text
    let script_or_style = choice((just("script"), just("style")))
        .then(wide_attrs)
        .map(|(name, a)| Marx {
            n: Mane::Tag(name.to_string()),
            a,
        });
    let raw_line = just(';').ignore_then(choice((
        just(' ').ignore_then(
            any()
                .filter(|c: &char| is_prn(*c))
                .repeated()
                .collect::<String>()
                .map(|line| text_node(line.as_bytes())),
        ),
        empty().to(text_node(b"\n")),
    )));
    let script_style_tail = gap()
        .ignore_then(raw_line.separated_by(gap()).at_least(1).collect::<Vec<_>>())
        .then_ignore(gap())
        .then_ignore(just("=="));

    //  ++tall-top
    tall_top.define(
        choice((
            just(' ')
                .repeated()
                .at_least(1)
                .ignore_then(quote_innards(inline_embed.clone(), true))
                .map(|innards| Top::Many(collapse_chars(innards, true))),
            script_or_style
                .then(script_style_tail)
                .map(|(g, c)| Top::One(Tuna::Manx(Manx { g, c }))),
            tall_elem.map(|manx| Top::One(Tuna::Manx(manx))),
            wide_quote(inline_embed.clone(), true, linemap).map(Top::Many),
            just('=').ignore_then(tall_tail).map(Top::Many),
            just('>')
                .ignore_then(gap())
                .ignore_then(cram)
                .map(|c| Top::One(Tuna::Manx(Manx { g: tag("div"), c }))),
            tuna_mode()
                .then_ignore(gap())
                .then(hoon)
                .map(|(mode, hoon)| Top::Many(vec![Tuna::TunaTail(mode(hoon))])),
            empty().to(Top::Many(vec![text_node(b"\n")])),
        ))
        .boxed(),
    );

    Sail {
        tall_top: tall_top.boxed(),
        wide_top: wide_top.boxed(),
        inline_embed,
    }
}

/// Tall-form sail (hoon-138 `apex:(sail &)`), after the leading `;`.
pub fn sail_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
    linemap: Arc<LineMap>,
    ctx: SailCtx,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    sail_parsers(hoon.boxed(), hoon_wide.boxed(), linemap, ctx)
        .tall_top
        .map(apex)
}

/// Wide-form sail (hoon-138 `apex:(sail |)`), after the leading `;`.
pub fn sail_wide<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
    linemap: Arc<LineMap>,
    ctx: SailCtx,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    sail_parsers(hoon.boxed(), hoon_wide.boxed(), linemap, ctx)
        .wide_top
        .map(apex)
}
