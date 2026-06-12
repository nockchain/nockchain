use std::collections::*;

use chumsky::input::{Stream, ValueInput};
use chumsky::prelude::*;

use crate::ast::hoon::*;
use crate::utils::*;

pub fn buc_runes_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just('@').ignore_then(bucpat(spec.clone())),
        just('_').ignore_then(buccab(hoon.clone())),
        just(':').ignore_then(buccol(spec.clone())),
        just("%").ignore_then(buccen(spec.clone())),
        just("<").ignore_then(bucgal(spec.clone())),
        just(">").ignore_then(bucgar(spec.clone())),
        just("|").ignore_then(bucbar(hoon.clone(), spec.clone())),
        just("&").ignore_then(bucpam(hoon.clone(), spec.clone())),
        just('^').ignore_then(bucket(spec.clone())),
        just('~').ignore_then(bucsig(hoon.clone(), spec.clone())),
        just('-').ignore_then(buchep(spec.clone())),
        just('=').ignore_then(buctis(spec.clone())),
        just('?').ignore_then(bucwut(spec.clone())),
        just("+").ignore_then(buclus(spec.clone())),
        just('.').ignore_then(bucdot(spec.clone())),
        just(",").ignore_then(buccom(spec.clone())),
    ))
}
fn split_nonempty_spec_list(specs: &[Spec]) -> (&Spec, &[Spec]) {
    specs
        .split_first()
        .expect("list_spec_closed parser returned an empty spec list")
}

pub fn buc_runes_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just('@').ignore_then(bucpat_wide(spec_wide.clone())),
        just('_').ignore_then(buccab_wide(hoon_wide.clone())),
        just(':').ignore_then(buccol_wide(spec_wide.clone())),
        just("%").ignore_then(buccen_wide(spec_wide.clone())),
        just("<").ignore_then(bucgal_wide(spec_wide.clone())),
        just(">").ignore_then(bucgar_wide(spec_wide.clone())),
        just("|").ignore_then(bucbar_wide(hoon_wide.clone(), spec_wide.clone())),
        just("&").ignore_then(bucpam_wide(hoon_wide.clone(), spec_wide.clone())),
        just('^').ignore_then(bucket_wide(spec_wide.clone())),
        just('~').ignore_then(bucsig_wide(hoon_wide.clone(), spec_wide.clone())),
        just('-').ignore_then(buchep_wide(spec_wide.clone())),
        just('=').ignore_then(buctis_wide(spec_wide.clone())),
        just('?').ignore_then(bucwut_wide(spec_wide.clone())),
        just("+").ignore_then(buclus_wide(spec_wide.clone())),
        just('.').ignore_then(bucdot_wide(spec_wide.clone())),
        just(",").ignore_then(buccom_wide(spec_wide.clone())),
    ))
}

pub fn buc_spec_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> + Clone {
    choice((
        just(':').ignore_then(buccol_spec(spec.clone())),
        just("%").ignore_then(buccen_spec(spec.clone())),
        just("<").ignore_then(bucgal_spec(spec.clone())),
        just(">").ignore_then(bucgar_spec(spec.clone())),
        just('^').ignore_then(bucket_spec(spec.clone())),
        just('~').ignore_then(bucsig_spec(hoon.clone(), spec.clone())),
        just("|").ignore_then(bucbar_spec(hoon.clone(), spec.clone())),
        just("&").ignore_then(bucpam_spec(hoon.clone(), spec.clone())),
        just('@').ignore_then(bucpat_spec(spec.clone())),
        just('_').ignore_then(buccab_spec(hoon.clone())),
        just('-').ignore_then(buchep_spec(spec.clone())),
        just('=').ignore_then(buctis_spec(spec.clone())),
        just('?').ignore_then(bucwut_spec(spec.clone())),
        just(";").ignore_then(bucmic_spec(hoon.clone())),
        just("+").ignore_then(buclus_spec(spec.clone())),
    ))
    .boxed()
}

pub fn buc_spec_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> + Clone {
    choice((
        just(':').ignore_then(buccol_spec_wide(spec_wide.clone())),
        just("%").ignore_then(buccen_spec_wide(spec_wide.clone())),
        just("<").ignore_then(bucgal_spec_wide(spec_wide.clone())),
        just(">").ignore_then(bucgar_spec_wide(spec_wide.clone())),
        just('^').ignore_then(bucket_spec_wide(spec_wide.clone())),
        just('~').ignore_then(bucsig_spec_wide(hoon_wide.clone(), spec_wide.clone())),
        just("|").ignore_then(bucbar_spec_wide(hoon_wide.clone(), spec_wide.clone())),
        just("&").ignore_then(bucpam_spec_wide(hoon_wide.clone(), spec_wide.clone())),
        just('@').ignore_then(bucpat_spec_wide(spec_wide.clone())),
        just('_').ignore_then(buccab_spec_wide(hoon_wide.clone())),
        just('-').ignore_then(buchep_spec_wide(spec_wide.clone())),
        just('=').ignore_then(buctis_spec_wide(spec_wide.clone())),
        just('?').ignore_then(bucwut_spec_wide(spec_wide.clone())),
        just(";").ignore_then(bucmic_spec_wide(hoon_wide.clone())),
        just("+").ignore_then(buclus_spec_wide(spec_wide.clone())),
    ))
    .boxed()
}

pub fn buccab_irregular<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    just('_')
        .ignore_then(hoon_wide.clone())
        .map(|h| Hoon::KetCol(Box::new(Spec::BucCab(h))))
}

pub fn bucdot<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    one_spec_closed_tall(spec.clone()).map(|s| Hoon::KetTar(Box::new(s)))
}

pub fn bucdot_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    one_spec_closed_wide(spec_wide.clone()).map(|s| Hoon::KetTar(Box::new(s)))
}

pub fn buccom<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    one_spec_closed_tall(spec.clone()).map(|s| Hoon::KetCol(Box::new(s)))
}

pub fn buccom_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    one_spec_closed_wide(spec_wide.clone()).map(|s| Hoon::KetCol(Box::new(s)))
}

pub fn bucket<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    two_specs_tall(spec.clone())
        .map(|(p, q)| Hoon::KetCol(Box::new(Spec::BucKet(Box::new(p), Box::new(q)))))
}

pub fn bucket_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    bucket_spec_wide(spec_wide.clone()).map(|p| Hoon::KetCol(Box::new(p)))
}

pub fn bucket_spec_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    two_specs_closed_wide(spec_wide.clone()).map(|(p, q)| Spec::BucKet(Box::new(p), Box::new(q)))
}

pub fn bucpam<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    bucpam_spec(hoon.clone(), spec.clone()).map(|p| Hoon::KetCol(Box::new(p)))
}

pub fn bucpam_spec<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    spec_hoon_tall(hoon.clone(), spec.clone()).map(|(p, q)| Spec::BucPam(Box::new(p), q))
}

pub fn buclus<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    name_spec_tall(spec.clone()).map(|(p, q)| Hoon::KetCol(Box::new(Spec::BucLus(p, Box::new(q)))))
}

pub fn buclus_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    name_spec_wide(spec_wide.clone())
        .map(|(p, q)| Hoon::KetCol(Box::new(Spec::BucLus(p, Box::new(q)))))
}

pub fn bucwut<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(
            spec_wide
                .clone()
                .separated_by(gap())
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .then_ignore(gap())
        .then_ignore(just("=="))
        .map(|specs| {
            let (first, rest) = split_nonempty_spec_list(&specs);
            Hoon::KetCol(Box::new(Spec::BucWut(
                Box::new(first.clone()),
                rest.to_vec(),
            )))
        })
}

pub fn bucwut_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    list_spec_closed_wide(spec_wide.clone()).map(|specs| {
        let (first, rest) = split_nonempty_spec_list(&specs);
        Hoon::KetCol(Box::new(Spec::BucWut(
            Box::new(first.clone()),
            rest.to_vec(),
        )))
    })
}

pub fn bucsig_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_spec_wide(hoon_wide.clone(), spec_wide.clone())
        .map(|(h, s)| Hoon::KetCol(Box::new(Spec::BucSig(h, Box::new(s)))))
}

pub fn bucpat<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    bucpat_spec(spec.clone()).map(|s| Hoon::KetCol(Box::new(s)))
}

pub fn buccab<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    buccab_spec(hoon.clone()).map(|p| Hoon::KetCol(Box::new(p)))
}

pub fn buccab_spec<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    gap().ignore_then(hoon.clone()).map(|p| Spec::BucCab(p))
}

pub fn buccab_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    buccab_spec_wide(hoon_wide.clone()).map(|s| Hoon::KetCol(Box::new(s)))
}

pub fn buccab_spec_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    one_hoon_closed_wide(hoon_wide.clone()).map(|p| Spec::BucCab(p))
}

pub fn bucgar<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    bucgar_spec(spec.clone()).map(|p| Hoon::KetCol(Box::new(p)))
}

pub fn bucgar_spec<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    two_specs_tall(spec.clone()).map(|(p, q)| Spec::BucGar(Box::new(p), Box::new(q)))
}

pub fn bucgar_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    bucgar_spec_wide(spec_wide.clone()).map(|p| Hoon::KetCol(Box::new(p)))
}

pub fn bucgar_spec_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    two_specs_closed_wide(spec_wide.clone()).map(|(p, q)| Spec::BucGar(Box::new(p), Box::new(q)))
}

pub fn bucgal<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    bucgal_spec(spec.clone()).map(|p| Hoon::KetCol(Box::new(p)))
}

pub fn bucgal_spec<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    two_specs_tall(spec.clone()).map(|(p, q)| Spec::BucGal(Box::new(p), Box::new(q)))
}

pub fn bucgal_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    bucgal_spec_wide(spec_wide.clone()).map(|p| Hoon::KetCol(Box::new(p)))
}

pub fn bucgal_spec_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    two_specs_closed_wide(spec_wide.clone()).map(|(p, q)| Spec::BucGal(Box::new(p), Box::new(q)))
}

pub fn buchep<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    two_specs_tall(spec.clone())
        .map(|(p, q)| Hoon::KetCol(Box::new(Spec::BucHep(Box::new(p), Box::new(q)))))
}

pub fn buchep_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    two_specs_closed_wide(spec_wide.clone())
        .map(|(p, q)| Hoon::KetCol(Box::new(Spec::BucHep(Box::new(p), Box::new(q)))))
}

pub fn bucpam_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    spec_hoon_wide(hoon_wide.clone(), spec_wide.clone())
        .map(|(p, q)| Hoon::KetCol(Box::new(Spec::BucPam(Box::new(p), q))))
}

pub fn buccol<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    list_spec_closed_tall(spec.clone()).map(|specs| {
        let (first, rest) = split_nonempty_spec_list(&specs);
        Hoon::KetCol(Box::new(Spec::BucCol(
            Box::new(first.clone()),
            rest.to_vec(),
        )))
    })
}

pub fn buccol_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    buccol_spec_wide(spec_wide.clone()).map(|s| Hoon::KetCol(Box::new(s)))
}

pub fn buccol_spec_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    list_spec_closed_wide(spec_wide.clone()).map(|specs| {
        let (first, rest) = split_nonempty_spec_list(&specs);
        Spec::BucCol(Box::new(first.clone()), rest.to_vec())
    })
}

pub fn bucsig<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_spec_tall(hoon.clone(), spec.clone())
        .map(|(p, q)| Hoon::KetCol(Box::new(Spec::BucSig(p, Box::new(q)))))
}

pub fn buccen<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    buccen_spec(spec.clone()).map(|s| Hoon::KetCol(Box::new(s)))
}

pub fn buccen_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    spec_wide
        .clone()
        .separated_by(just(' '))
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(just('('), just(')'))
        .map(|specs| {
            let (first, rest) = split_nonempty_spec_list(&specs);
            Hoon::KetCol(Box::new(Spec::BucCen(
                Box::new(first.clone()),
                rest.to_vec(),
            )))
        })
}

pub fn bucpat_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    spec_wide
        .clone()
        .then_ignore(just(' '))
        .then(spec_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::KetCol(Box::new(Spec::BucPat(Box::new(p), Box::new(q)))))
}

pub fn buccab_spec_irregular<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    just('_')
        .ignore_then(hoon_wide.clone())
        .map(|h| Spec::BucCab(h))
}

pub fn bucbar<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    bucbar_spec(hoon.clone(), spec.clone()).map(|s| Hoon::KetCol(Box::new(s)))
}

pub fn bucbar_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    spec_hoon_wide(hoon_wide.clone(), spec_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Hoon::KetCol(Box::new(Spec::BucBar(Box::new(p), q))))
}

pub fn bucbar_spec<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    spec_hoon_tall(hoon.clone(), spec.clone()).map(|(p, q)| Spec::BucBar(Box::new(p), q))
}

pub fn bucmic<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon.clone()
        .map(|h| Hoon::KetCol(Box::new(Spec::BucMic(h))))
}

pub fn bucmic_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    bucmic(hoon_wide.clone())
}

pub fn bucmic_spec<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    gap().ignore_then(hoon.clone()).map(|h| Spec::BucMic(h))
}

pub fn bucmic_spec_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    one_hoon_closed_wide(hoon_wide.clone()).map(|h| Spec::BucMic(h))
}

pub fn bucmic_spec_irregular<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    just(",")
        .ignore_then(hoon_wide.clone())
        .map(|h| Spec::BucMic(h))
}

pub fn bucket_spec<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    two_specs_tall(spec.clone()).map(|(p, q)| Spec::BucKet(Box::new(p), Box::new(q)))
}

pub fn buclus_spec<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    name_spec_tall(spec.clone()).map(|(p, q)| Spec::BucLus(p, Box::new(q)))
}

pub fn bucwut_irregular<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    just(' ')
        .repeated()
        .ignored()
        .ignore_then(
            spec_wide
                .clone()
                .separated_by(just(' '))
                .at_least(1)
                .collect::<Vec<_>>()
                .delimited_by(just('('), just(')')),
        )
        .map(|specs| {
            let (first, rest) = split_nonempty_spec_list(&specs);
            Hoon::KetCol(Box::new(Spec::BucWut(
                Box::new(first.clone()),
                rest.to_vec(),
            )))
        })
}

pub fn bucwut_irregular_spec<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    just("?(")
        .ignore_then(
            spec_wide
                .clone()
                .separated_by(just(' '))
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .then_ignore(just(')'))
        .map(|specs| {
            let (first, rest) = split_nonempty_spec_list(&specs);
            Spec::BucWut(Box::new(first.clone()), rest.to_vec())
        })
}

pub fn buctis<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    name_spec_tall(spec.clone())
        .map(|(name, s)| Hoon::KetCol(Box::new(Spec::BucTis(Skin::Term(name), Box::new(s)))))
}

pub fn buctis_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    buctis_spec_wide(spec_wide.clone()).map(|s| Hoon::KetCol(Box::new(s)))
}

pub fn buctis_spec_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    name_spec_wide(spec_wide.clone()).map(|(name, s)| Spec::BucTis(Skin::Term(name), Box::new(s)))
}

pub fn buctis_spec<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    name_spec_tall(spec.clone()).map(|(name, s)| Spec::BucTis(Skin::Term(name), Box::new(s)))
}

pub fn bucwut_spec<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    gap()
        .ignore_then(
            spec_wide
                .clone()
                .separated_by(gap())
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .then_ignore(gap())
        .then_ignore(just("=="))
        .map(|specs| {
            let (first, rest) = split_nonempty_spec_list(&specs);
            Spec::BucWut(Box::new(first.clone()), rest.to_vec())
        })
}

pub fn bucwut_spec_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    spec_wide
        .clone()
        .separated_by(just(' '))
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(just('('), just(')'))
        .map(|specs| {
            let (first, rest) = split_nonempty_spec_list(&specs);
            Spec::BucWut(Box::new(first.clone()), rest.to_vec())
        })
}

pub fn buctis_irregular<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    let symbol_tis_spec = symbol()
        .then_ignore(just('='))
        .then(spec_wide.clone())
        .map(|(n, s)| Spec::BucTis(Skin::Term(n.to_string()), Box::new(s)));

    let tis_symbol_tis_spec = symbol()
        .then_ignore(just('='))
        .then(spec_wide.clone())
        .map(|(name, spec)| (Some(name), spec));

    let tis_spec = spec_wide.clone().map(|spec| (None, spec));

    choice((
        symbol_tis_spec, // foo=bar
        just('=')
            .ignore_then(choice((
                tis_symbol_tis_spec, // =foo=bar
                tis_spec,            // =bar
            )))
            .try_map(|(name, spec), span| {
                match name {
                    Some(n) => {
                        // no autoname needed
                        let term = n;
                        Ok(Spec::BucTis(Skin::Term(term), Box::new(spec)))
                    }
                    None => {
                        // need autoname
                        match autoname(spec.clone()) {
                            None => Err(Rich::custom(span, "cannot name spec")),
                            Some(auto_term) => Ok(Spec::BucTis(
                                Skin::Term(auto_term.to_string()),
                                Box::new(spec),
                            )),
                        }
                    }
                }
            }),
    ))
}

pub fn buccol_irregular<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    spec_wide
        .clone()
        .separated_by(just(' '))
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(just("["), just("]"))
        .map(|specs| {
            let (first, rest) = split_nonempty_spec_list(&specs);
            Spec::BucCol(Box::new(first.clone()), rest.to_vec())
        })
}

pub fn bucsig_spec_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    hoon_spec_wide(hoon_wide.clone(), spec_wide.clone()).map(|(h, s)| Spec::BucSig(h, Box::new(s)))
}

pub fn buclus_spec_wide<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    symbol()
        .then_ignore(just(' '))
        .then(spec.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Spec::BucLus(p, Box::new(q)))
}

pub fn buchep_spec<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    two_specs_tall(spec.clone()).map(|(p, q)| Spec::BucHep(Box::new(p), Box::new(q)))
}

pub fn buchep_spec_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    two_specs_closed_wide(spec_wide.clone()).map(|(p, q)| Spec::BucHep(Box::new(p), Box::new(q)))
}

pub fn bucpat_spec<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    two_specs_tall(spec.clone()).map(|(p, q)| Spec::BucPat(Box::new(p), Box::new(q)))
}

pub fn buccol_spec<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    gap()
        .ignore_then(
            spec.clone()
                .separated_by(gap())
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .then_ignore(gap())
        .then_ignore(just("=="))
        .map(|specs| {
            let (first, rest) = split_nonempty_spec_list(&specs);
            Spec::BucCol(Box::new(first.clone()), rest.to_vec())
        })
}

pub fn bucsig_spec<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    hoon_spec_tall(hoon.clone(), spec.clone()).map(|(p, q)| Spec::BucSig(p, Box::new(q)))
}

pub fn buccen_spec_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    spec_wide
        .clone()
        .separated_by(just(' '))
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(just('('), just(')'))
        .map(|specs| {
            let (first, rest) = split_nonempty_spec_list(&specs);
            Spec::BucCen(Box::new(first.clone()), rest.to_vec())
        })
}

pub fn buccen_spec<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    list_spec_closed_tall(spec.clone()).map(|specs| {
        let (first, rest) = split_nonempty_spec_list(&specs);
        Spec::BucCen(Box::new(first.clone()), rest.to_vec())
    })
}

pub fn bucpat_spec_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    two_specs_closed_wide(spec_wide.clone()).map(|(p, q)| Spec::BucPat(Box::new(p), Box::new(q)))
}

pub fn bucbar_spec_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    spec_hoon_wide(hoon_wide.clone(), spec_wide.clone())
        .delimited_by(just('('), just(')'))
        .map(|(p, q)| Spec::BucBar(Box::new(p), q))
}

pub fn bucpam_spec_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    spec_hoon_wide(hoon_wide.clone(), spec_wide.clone()).map(|(p, q)| Spec::BucPam(Box::new(p), q))
}
