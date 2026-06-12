use chumsky::prelude::*;
use num_bigint::BigUint;

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

fn parsed_atom_to_cord(atom: ParsedAtom) -> String {
    let code = match atom {
        ParsedAtom::Small(x) => x as u32,
        ParsedAtom::Big(b) => {
            if b > BigUint::from(u32::MAX) {
                0xFFFD
            } else {
                b.try_into().unwrap_or(0xFFFD)
            }
        }
    };
    std::char::from_u32(code).unwrap_or('\u{FFFD}').to_string()
}

fn woof_to_beer(woof: Woof) -> Beer {
    match woof {
        Woof::ParsedAtom(atom) => Beer::Char(parsed_atom_to_cord(atom)),
        Woof::Hoon(hoon) => Beer::Hoon(hoon),
    }
}

fn hoon_to_beers(hoon: Hoon) -> Vec<Beer> {
    match hoon {
        Hoon::Knit(woofs) => woofs.into_iter().map(woof_to_beer).collect(),
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
    symbol()
        .then_ignore(inline_space1())
        .then(hoon_wide)
        .map(|(name, value)| (Mane::Tag(name), hoon_to_beers(value)))
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
    mane_parser()
        .then(class_attr().or_not())
        .then(id_attr().or_not())
        .then(paren_attrs(hoon_wide).or_not())
        .map(|(((name, class_attr), id_attr), extra_attrs)| {
            let mut attrs = Vec::new();
            if let Some(attr) = class_attr {
                attrs.push(attr);
            }
            if let Some(attr) = id_attr {
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
}

fn tuna_tail<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Tuna, Err<'src>> {
    let mode = choice((
        just('-').to(TunaMode::Tape),
        just('+').to(TunaMode::Manx),
        just('*').to(TunaMode::Marl),
    ));

    mode.then_ignore(gap()).then(hoon).map(|(mode, hoon)| {
        let tail = match mode {
            TunaMode::Tape => TunaTail::Tape(hoon),
            TunaMode::Manx => TunaTail::Manx(hoon),
            TunaMode::Marl => TunaTail::Marl(hoon),
        };
        Tuna::TunaTail(tail)
    })
}

fn tag<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
    manx: impl ParserExt<'src, Manx>,
) -> impl Parser<'src, &'src str, Manx, Err<'src>> {
    let tail_item = just(';').ignore_then(choice((
        tuna_tail(hoon.clone()),
        manx.clone().map(Tuna::Manx),
    )));

    let tall_children = gap()
        .ignore_then(tail_item.then_ignore(gap()).repeated().collect::<Vec<_>>())
        .then_ignore(just("=="));

    let inline_children = choice((
        just(';').to(Vec::new()),
        just(':')
            .ignore_then(inline_space())
            .ignore_then(wrapped_elems(hoon_wide.clone())),
    ));

    tag_head(hoon_wide)
        .then(choice((inline_children, tall_children)))
        .map(|(head, children)| Manx {
            g: head,
            c: children,
        })
}

fn sail_parser<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    let mut manx = Recursive::declare();
    let tag_parser = tag(hoon.clone(), hoon_wide.clone(), manx.clone()).boxed();
    manx.define(tag_parser);

    let marl_tail = tuna_tail(hoon).map(|tuna| Hoon::MicTis(vec![tuna]));
    choice((manx.map(Hoon::Xray), marl_tail))
}

pub fn sail_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    sail_parser(hoon, hoon_wide)
}

pub fn sail_wide<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    sail_parser(hoon, hoon_wide)
}
