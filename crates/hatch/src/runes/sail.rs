use chumsky::prelude::*;

use crate::ast::hoon::*;
use crate::utils::*;

fn inline_space<'src>() -> impl Parser<'src, &'src str, (), Err<'src>> {
    one_of(" \t").repeated().ignored()
}

fn inline_space1<'src>() -> impl Parser<'src, &'src str, (), Err<'src>> {
    one_of(" \t").repeated().at_least(1).ignored()
}

fn mixed_case_symbol<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
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

fn mane_parser<'src>() -> impl Parser<'src, &'src str, Mane, Err<'src>> {
    mixed_case_symbol()
        .then(just('_').ignore_then(mixed_case_symbol()).or_not())
        .map(|(base, suffix)| match suffix {
            Some(ns) => Mane::TagSpace(base, ns),
            None => Mane::Tag(base),
        })
}

//  hoon-138 keeps one beer char per text byte. A beer char cord stores a
//  byte as the char with that code point, so split any multi-byte woof atom
//  into its bytes rather than reading it as a Unicode scalar.
fn woof_to_beers(woof: Woof) -> Vec<Beer> {
    match woof {
        Woof::ParsedAtom(atom) => atom
            .to_biguint()
            .to_bytes_le()
            .into_iter()
            .map(|byte| Beer::Char(char::from(byte).to_string()))
            .collect(),
        Woof::Hoon(hoon) => vec![Beer::Hoon(hoon)],
    }
}

fn hoon_to_beers(hoon: Hoon) -> Vec<Beer> {
    match hoon {
        Hoon::Knit(woofs) => woofs.into_iter().flat_map(woof_to_beers).collect(),
        other => vec![Beer::Hoon(other)],
    }
}

fn string_to_beers(value: String) -> Vec<Beer> {
    value.chars().map(|ch| Beer::Char(ch.to_string())).collect()
}

fn class_attr<'src>() -> impl Parser<'src, &'src str, (Mane, Vec<Beer>), Err<'src>> {
    just('.')
        .ignore_then(symbol())
        .repeated()
        .at_least(1)
        .collect::<Vec<_>>()
        .map(|classes| {
            let value = classes.join(" ");
            (Mane::Tag("class".to_string()), string_to_beers(value))
        })
}

fn id_attr<'src>() -> impl Parser<'src, &'src str, (Mane, Vec<Beer>), Err<'src>> {
    just('#')
        .ignore_then(symbol())
        .map(|id| (Mane::Tag("id".to_string()), string_to_beers(id)))
}

fn attr_pair<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, (Mane, Vec<Beer>), Err<'src>> {
    mane_parser()
        .then_ignore(inline_space1())
        .then(hoon_wide)
        .map(|(name, value)| (name, hoon_to_beers(value)))
}

//  hoon-138 ++tall-attrs: `=name  value` lines after a tall tag head
fn tall_attrs<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Mart, Err<'src>> {
    gap()
        .then(just('='))
        .ignore_then(mane_parser())
        .then_ignore(gap())
        .then(hoon_wide)
        .map(|(name, value)| (name, hoon_to_beers(value)))
        .repeated()
        .collect::<Vec<_>>()
}

fn paren_attrs<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Mart, Err<'src>> {
    let separator = just(',').then(inline_space()).ignored();
    attr_pair(hoon_wide)
        .separated_by(separator)
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just('('), just(')'))
}

fn tag_head<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Marx, Err<'src>> {
    //  hoon-138 ++tag-head: the #id comes before the .classes
    mane_parser()
        .then(id_attr().or_not())
        .then(class_attr().or_not())
        .then(paren_attrs(hoon_wide).or_not())
        .map(|(((name, id_attr), class_attr), extra_attrs)| {
            let mut attrs = Vec::new();
            if let Some(attr) = id_attr {
                attrs.push(attr);
            }
            if let Some(attr) = class_attr {
                attrs.push(attr);
            }
            if let Some(mut rest) = extra_attrs {
                attrs.append(&mut rest);
            }
            Marx { n: name, a: attrs }
        })
}

fn braced_hoon<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    let items = hoon_wide
        .separated_by(inline_space1())
        .at_least(1)
        .collect::<Vec<_>>();

    inline_space()
        .ignore_then(items)
        .then_ignore(inline_space())
        .delimited_by(just('{'), just('}'))
        .map(Hoon::ColTar)
}

fn wrapped_elems<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Marl, Err<'src>> {
    braced_hoon(hoon_wide).map(|hoon| vec![Tuna::TunaTail(TunaTail::Tape(hoon))])
}

#[derive(Clone, Copy)]
enum TunaMode {
    Tape,
    Manx,
    Marl,
    Call,
}

fn tuna_tail<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Tuna, Err<'src>> {
    let mode = choice((
        just('-').to(TunaMode::Tape),
        just('+').to(TunaMode::Manx),
        just('*').to(TunaMode::Marl),
        just('%').to(TunaMode::Call),
    ));

    mode.then_ignore(gap()).then(hoon).map(|(mode, hoon)| {
        let tail = match mode {
            TunaMode::Tape => TunaTail::Tape(hoon),
            TunaMode::Manx => TunaTail::Manx(hoon),
            TunaMode::Marl => TunaTail::Marl(hoon),
            TunaMode::Call => TunaTail::Call(hoon),
        };
        Tuna::TunaTail(tail)
    })
}

//  The children following a tag head or `;=`.
fn tag_tail<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
    manx: impl ParserExt<'src, Manx>,
    tail: impl ParserExt<'src, Marl>,
) -> impl Parser<'src, &'src str, Marl, Err<'src>> {
    //  a nested `;=` list is spliced into its parent's children
    let tail_item = just(';').ignore_then(choice((
        tuna_tail(hoon.clone()).map(|tuna| vec![tuna]),
        manx.clone().map(|manx| vec![Tuna::Manx(manx)]),
        just('=').ignore_then(tail),
    )));

    let tall_children = gap()
        .ignore_then(tail_item.then_ignore(gap()).repeated().collect::<Vec<_>>())
        .then_ignore(just("=="))
        .map(|items| items.into_iter().flatten().collect());

    let inline_children = choice((
        just(';').to(Vec::new()),
        just(':')
            .ignore_then(inline_space())
            .ignore_then(wrapped_elems(hoon_wide.clone())),
    ));

    choice((inline_children, tall_children))
}

fn sail_parser<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
    tall: bool,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    let mut manx = Recursive::declare();
    let mut tail = Recursive::declare();
    manx.define(
        tag_head(hoon_wide.clone())
            .then(tall_attrs(hoon_wide.clone()))
            .then(tail.clone())
            .map(|((mut head, attrs), children)| {
                head.a.extend(attrs);
                Manx {
                    g: head,
                    c: children,
                }
            })
            .boxed(),
    );
    tail.define(tag_tail(hoon.clone(), hoon_wide, manx.clone(), tail.clone()).boxed());

    let marl_tail = tuna_tail(hoon).map(|tuna| Hoon::MicTis(vec![tuna]));
    let top = choice((manx.map(Hoon::Xray), marl_tail));
    if tall {
        //  `;=` (%mcts): a list of nodes, tall form only
        choice((top, just('=').ignore_then(tail).map(Hoon::MicTis))).boxed()
    } else {
        top.boxed()
    }
}

pub fn sail_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    sail_parser(hoon, hoon_wide, true)
}

pub fn sail_wide<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    sail_parser(hoon, hoon_wide, false)
}
