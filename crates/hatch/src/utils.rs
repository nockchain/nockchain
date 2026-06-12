use std::cell::Cell;
use std::cmp;
use std::collections::{HashMap, *};
use std::fs::File;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{BufWriter, Write};
use std::ops::BitAnd;
use std::path::PathBuf;
use std::rc::Rc;
use std::str::FromStr;
use std::sync::Arc;

use bitvec::order::Lsb0;
use bitvec::prelude::*;
use bitvec::slice::BitSlice;
use bitvec::vec::BitVec;
use bytes::Bytes;
use chumsky::input::{Input, MapExtra, StrInput, Stream, ValueInput};
use chumsky::prelude::*;
use chumsky::span::Span;
use either::Either::{Left, Right};
use ibig::UBig;
use nockapp::noun::slab::NounSlab;
use nockapp::AtomExt;
use nockvm::ext::noun_equality;
use nockvm::jets::math::util::lth_b;
use nockvm::mug::{calc_atom_mug_u32, calc_cell_mug_u32, get_mug, set_mug};
use nockvm::noun::{
    Atom, DirectAtom, Noun, NounAllocator, NounHandle, NounSpace, D, DIRECT_MAX, T,
};
use nockvm_macros::tas;
use num_bigint::BigUint;
use num_traits::identities::Zero;
use num_traits::{FromPrimitive, Num, One, ToPrimitive};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::ast::hoon::*;
pub type Err<'src> = extra::Full<Rich<'src, char>, (), ()>;

pub trait ParserExt<'src, O>: Parser<'src, &'src str, O, Err<'src>> + Clone + 'src {}

impl<'src, O, P> ParserExt<'src, O> for P where
    P: Parser<'src, &'src str, O, Err<'src>> + Clone + 'src
{
}

fn slab_mug(a: Noun, space: &NounSpace) -> u32 {
    let mut stack = vec![a];
    while let Some(noun) = stack.pop() {
        if let Ok(mut allocated) = noun.as_allocated() {
            if get_mug(noun, space).is_none() {
                match allocated.as_either() {
                    Left(indirect) => unsafe {
                        set_mug(
                            &mut allocated,
                            calc_atom_mug_u32(indirect.as_atom(), space),
                            space,
                        );
                    },
                    Right(cell) => {
                        let cell = cell.in_space(space);
                        match (
                            get_mug(cell.head().noun(), space),
                            get_mug(cell.tail().noun(), space),
                        ) {
                            (Some(head_mug), Some(tail_mug)) => unsafe {
                                set_mug(
                                    &mut allocated,
                                    calc_cell_mug_u32(head_mug, tail_mug, space),
                                    space,
                                );
                            },
                            _ => {
                                stack.push(noun);
                                stack.push(cell.tail().noun());
                                stack.push(cell.head().noun());
                            }
                        }
                    }
                }
            }
        }
    }
    get_mug(a, space).expect("Noun should have a mug once mugged.")
}

//
// String -> ParsedAtom conversion functions
//

pub fn string_to_atom(s: String) -> ParsedAtom {
    let vec_u128: Vec<u128> = s.chars().map(|c| c as u128).collect();

    rap(3, &vec_u128)
}

pub fn ta_to_atom(s: String) -> ParsedAtom {
    if s == "~.".to_string() {
        return ParsedAtom::Small(0);
    }
    let vec_u128: Vec<u128> = s.chars().map(|c| c as u128).collect();

    rap(3, &vec_u128)
}

pub fn term_to_atom(s: String) -> ParsedAtom {
    if s == "$".to_string() {
        return ParsedAtom::Small(0);
    }
    let vec_u128: Vec<u128> = s.chars().map(|c| c as u128).collect();

    rap(3, &vec_u128)
}

//  @ud to @
pub fn decimal_to_atom(s: String) -> ParsedAtom {
    match s.parse::<u128>() {
        Ok(n) => ParsedAtom::Small(n),
        Err(_) => {
            let big = BigUint::parse_bytes(s.as_bytes(), 10).expect("invalid decimal in big atom");
            ParsedAtom::from_biguint(big)
        }
    }
}

//  @ux to @
pub fn hex_to_atom(s: String) -> ParsedAtom {
    let clean = s.strip_prefix("0x").unwrap_or(&s);

    if clean.len() <= 32 {
        if let Ok(n) = u128::from_str_radix(clean, 16) {
            return ParsedAtom::Small(n);
        }
    }

    let big = BigUint::parse_bytes(clean.as_bytes(), 16).expect("invalid hex in big atom");

    ParsedAtom::Big(big)
}

//  @ub to @
pub fn binary_to_atom(s: String) -> ParsedAtom {
    match u128::from_str_radix(&s, 2) {
        Ok(n) => ParsedAtom::Small(n),
        Err(_) => {
            let big = BigUint::parse_bytes(s.as_bytes(), 2).expect("invalid binary in big atom");
            ParsedAtom::from_biguint(big)
        }
    }
}

//  @t to @
pub fn cord_chars_to_atom(chars: Vec<char>) -> ParsedAtom {
    let mut atom = BigUint::zero();
    let mut power = BigUint::from(1u32);
    let base = BigUint::from(256u32);

    for &c in &chars {
        let byte = BigUint::from(c as u32 & 0xFF);
        atom += &byte * &power;
        power *= &base;
    }

    ParsedAtom::Big(atom)
}

const ALPH64: &str = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-~";

//  @uw to @
pub fn base64_to_atom(s: String) -> ParsedAtom {
    let mut n: u128 = 0;

    for ch in s.chars() {
        let v = match ALPH64.find(ch) {
            Some(i) => i as u128,
            None => panic!("invalid digit '{ch}' in base64"),
        };

        n = n.checked_mul(64).expect("value exceeds u128 range (mul)");

        n = n.checked_add(v).expect("value exceeds u128 range (add)");
    }

    ParsedAtom::Small(n)
}

const ALPH32: &str = "0123456789abcdefghijklmnopqrstuv";

//  @uv to @
pub fn base32_to_atom(s: String) -> ParsedAtom {
    let mut n: u128 = 0;

    for ch in s.chars() {
        let v = match ALPH32.find(ch) {
            Some(i) => i as u128,
            None => panic!("invalid digit '{ch}' in base32"),
        };

        n = n.checked_mul(32).expect("value exceeds u128 range (mul)");

        n = n.checked_add(v).expect("value exceeds u128 range (add)");
    }

    ParsedAtom::Small(n)
}

// +fim
pub fn base58_to_atom(s: String) -> Option<ParsedAtom> {
    let yek = build_yek();

    let digits: Vec<u8> = s
        .chars()
        .map(|ch| cha_fa(&yek, ch))
        .collect::<Option<_>>()?;

    let a = ParsedAtom::Big(bass_58(&digits));
    den_fa(&a)
}

pub fn ipv4_to_atom(s: String) -> Option<ParsedAtom> {
    let addr = s.parse::<std::net::Ipv4Addr>().ok()?;

    let ip_num = u32::from_be_bytes(addr.octets());

    Some(ParsedAtom::Small(ip_num.into()))
}

pub fn ipv6_to_atom(s: String) -> Option<ParsedAtom> {
    let addr = s.parse::<std::net::Ipv6Addr>().ok()?;
    let num = u128::from_be_bytes(addr.octets());
    Some(ParsedAtom::Small(num))
}

pub fn basal(bas: BaseType) -> Hoon {
    match bas {
        BaseType::Atom(a) => {
            let literal = if a == "da" {
                ParsedAtom::Small(year(true, 2000, 1, 1, 0, 0, 0, &Vec::new()))
            } else {
                decimal_to_atom("0".to_string())
            };
            Hoon::Sand(a, NounExpr::ParsedAtom(literal))
        }
        BaseType::NounExpr => {
            let rock0 = Box::new(Hoon::Rock(
                "$".to_string(),
                NounExpr::ParsedAtom(ParsedAtom::Small(0)),
            ));
            let rock1 = Box::new(Hoon::Rock(
                "$".to_string(),
                NounExpr::ParsedAtom(ParsedAtom::Small(1)),
            ));
            let rock0_clone = rock0.clone();
            let rock0_clone2 = rock0.clone();
            Hoon::KetLus(
                Box::new(Hoon::DotTar(
                    rock0,
                    Box::new(Hoon::Pair(rock0_clone, rock1)),
                )),
                rock0_clone2,
            )
        }
        BaseType::Cell => {
            let noun = Box::new(basal(BaseType::NounExpr));
            let noun_clone = noun.clone();
            Hoon::Pair(noun, noun_clone)
        }
        BaseType::Flag => {
            let rock0 = Box::new(Hoon::Rock(
                "$".to_string(),
                NounExpr::ParsedAtom(ParsedAtom::Small(0)),
            ));
            let rock0_clone = rock0.clone();
            let rock1_clone = rock0.clone();
            Hoon::KetLus(Box::new(Hoon::DotTis(rock0, rock0_clone)), rock1_clone)
        }
        BaseType::Null => Hoon::Rock("n".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(0))),
        BaseType::Void => Hoon::ZapZap,
    }
}

pub fn function(
    fun: Spec,
    arg: Spec,
    mod_: &Spec,
    dom: u64,
    hay: &WingType,
    cox: &HashMap<String, Spec>,
    bug: &Vec<Spot>,
    nut: &Option<Note>,
    def: &Option<Hoon>,
) -> Hoon {
    Hoon::TisGar(
        Box::new(Hoon::Pair(
            Box::new(example(&fun.clone(), dom, hay, cox, &vec![], &None, &None)),
            Box::new(example(&arg.clone(), dom, hay, cox, &vec![], &None, &None)),
        )),
        Box::new(Hoon::KetBar(Box::new(Hoon::BarCol(
            Box::new(Hoon::Axis(2)),
            Box::new(Hoon::Axis(15)),
        )))),
    )
}

pub fn interface(
    variance: Vair,
    payload: Spec,
    arms: HashMap<String, Spec>,
    mod_: &Spec,
    dom: u64,
    hay: &WingType,
    cox: &HashMap<String, Spec>,
    bug: &Vec<Spot>,
    nut: &Option<Note>,
    def: &Option<Hoon>,
) -> Hoon {
    let map: HashMap<String, Hoon> = arms
        .into_iter()
        .map(|(term, spec)| (term, example(&spec, dom, hay, cox, &vec![], &None, &None)))
        .collect();
    let brcn = Hoon::BarCen(None, HashMap::from([("$".to_string(), (None, map))]));

    let example_res = example(&payload, dom, hay, cox, &vec![], &None, &None);
    let tsgr = Hoon::TisGar(Box::new(example_res), Box::new(brcn));
    match variance {
        Vair::Gold => tsgr,
        Vair::Lead => Hoon::KetWut(Box::new(tsgr)),
        Vair::Zinc => Hoon::KetPam(Box::new(tsgr)),
        Vair::Iron => Hoon::KetBar(Box::new(tsgr)),
    }
}

// TODO: accept args by ref?
pub fn spore(
    spec: Spec,
    dom: u64,
    hay: WingType,
    cox: HashMap<String, Spec>,
    bug: Vec<Spot>,
    nut: Option<Note>,
    def: Option<Hoon>,
) -> Hoon {
    // Canonical hoon-138 `++spore`:
    //   :+  %ktls  [%bust %noun]
    //   %-  decorate
    //   %-  home
    //   ?^  def  u.def
    //   |-  ...
    let subject = match def {
        Some(d) => d,
        None => spore_recursion(spec, dom, hay.clone(), cox, bug.clone(), nut.clone(), None),
    };
    let ketlus_tail = decorate(home(subject, hay, dom), bug, nut);
    Hoon::KetLus(
        Box::new(Hoon::Bust(BaseType::NounExpr)),
        Box::new(ketlus_tail),
    )
}

pub fn spore_recursion(
    spec: Spec,
    dom: u64,
    hay: WingType,
    cox: HashMap<String, Spec>,
    bug: Vec<Spot>,
    nut: Option<Note>,
    def: Option<Hoon>,
) -> Hoon {
    match spec {
        Spec::Base(b) => match b {
            BaseType::Void => {
                Hoon::Rock("n".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(0)))
            }
            _ => basal(b),
        },
        Spec::BucBuc(s, map) => {
            let mut new_cox = cox;
            new_cox.extend(map);
            new_cox.insert("$".to_string(), *s.clone());
            spore_recursion(*s, dom, hay, new_cox, bug, nut, def)
        }
        Spec::Dbug(spot, spec) => {
            let tail = spore_recursion(*spec, dom, hay, cox, bug, nut, def);
            Hoon::Dbug(spot, Box::new(tail))
        }
        Spec::Gist(_, spec) => spore_recursion(*spec, dom, hay, cox, bug, nut, def),
        Spec::Leaf(term, atom) => Hoon::Rock(term, NounExpr::ParsedAtom(atom)),
        Spec::Loop(term) => {
            let spec = cox.get(&term).expect("Spec-Loop: Name not found");
            spore_recursion(spec.clone(), dom, hay, cox, bug, nut, def)
        }
        Spec::Like(wing, wings) => {
            let p = unreel(wing, wings);
            spore_recursion(Spec::BucMic(p), dom, hay, cox, bug, nut, def)
        }
        Spec::Made(_, q) => spore_recursion(*q, dom, hay, cox, bug, nut, def),
        Spec::Make(hoon, specs) => {
            let p = unfold(hoon, specs);
            spore_recursion(Spec::BucMic(p), dom, hay, cox, bug, nut, def)
        }
        Spec::Name(term, spec) => spore_recursion(*spec, dom, hay, cox, bug, nut, def),
        Spec::Over(wing, spec) => spore_recursion(*spec, dom, wing, cox, bug, nut, def),
        Spec::BucBar(spec, hoon) => spore_recursion(*spec, dom, hay, cox, bug, nut, def),
        Spec::BucCab(_) => Hoon::Rock("n".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(0))),
        Spec::BucCol(spec, specs) => {
            spore_buccol_recursion(*spec, specs, dom, hay, cox, bug, nut, def)
        }
        Spec::BucCen(spec, specs) => {
            spore_buccen_recursion(*spec, specs, dom, hay, cox, bug, nut, def)
        }
        Spec::BucHep(spec, specs) => {
            Hoon::Rock("n".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(0)))
        }
        Spec::BucGal(p_spec, q_spec) => spore_recursion(*q_spec, dom, hay, cox, bug, nut, def),
        Spec::BucGar(p_spec, q_spec) => spore_recursion(*q_spec, dom, hay, cox, bug, nut, def),
        Spec::BucKet(p_spec, q_spec) => spore_recursion(*q_spec, dom, hay, cox, bug, nut, def),
        Spec::BucLus(stud, spec) => {
            let tail = spore_recursion(*spec, dom, hay, cox, bug, nut, def);
            Hoon::Note(Note::Know(stud), Box::new(tail))
        }
        Spec::BucMic(hoon) => Hoon::TisGal(Box::new(Hoon::Axis(6)), Box::new(hoon)),
        Spec::BucPam(spec, hoon) => spore_recursion(*spec, dom, hay, cox, bug, nut, def),
        Spec::BucSig(hoon, spec) => Hoon::KetHep(spec, Box::new(hoon)),
        Spec::BucTis(skin, spec) => {
            let tail = spore_recursion(*spec, dom, hay, cox, bug, nut, def);
            Hoon::KetTis(skin, Box::new(tail))
        }
        Spec::BucPat(p_spec, q_spec) => spore_recursion(*p_spec, dom, hay, cox, bug, nut, def),
        Spec::BucWut(spec, specs) => {
            spore_bucwut_recursion(*spec, specs, dom, hay, cox, bug, nut, def)
        }
        Spec::BucDot(..) | Spec::BucFas(..) | Spec::BucTic(..) | Spec::BucZap(..) => {
            Hoon::Rock("n".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(0)))
        }
    }
}

pub fn spore_buccol_recursion(
    spec: Spec,
    list_spec: Vec<Spec>,
    dom: u64,
    hay: WingType,
    cox: HashMap<String, Spec>,
    bug: Vec<Spot>,
    nut: Option<Note>,
    def: Option<Hoon>,
) -> Hoon {
    if list_spec.is_empty() {
        spore_recursion(spec, dom, hay, cox, bug, nut, def)
    } else {
        let head = spore_recursion(
            spec,
            dom.clone(),
            hay.clone(),
            cox.clone(),
            bug.clone(),
            nut.clone(),
            def.clone(),
        );
        let tail = spore_buccol_recursion(
            list_spec
                .first()
                .expect("non-empty spec list checked above")
                .clone(),
            list_spec[1..].to_vec(),
            dom,
            hay,
            cox,
            bug,
            nut,
            def,
        );
        Hoon::Pair(Box::new(head), Box::new(tail))
    }
}

pub fn spore_bucwut_recursion(
    spec: Spec,
    list_spec: Vec<Spec>,
    dom: u64,
    hay: WingType,
    cox: HashMap<String, Spec>,
    bug: Vec<Spot>,
    nut: Option<Note>,
    def: Option<Hoon>,
) -> Hoon {
    if list_spec.is_empty() {
        spore_recursion(spec, dom, hay, cox, bug, nut, def)
    } else {
        spore_bucwut_recursion(
            list_spec
                .first()
                .expect("non-empty spec list checked above")
                .clone(),
            list_spec[1..].to_vec(),
            dom,
            hay,
            cox,
            bug,
            nut,
            def,
        )
    }
}

pub fn spore_buccen_recursion(
    spec: Spec,
    list_spec: Vec<Spec>,
    dom: u64,
    hay: WingType,
    cox: HashMap<String, Spec>,
    bug: Vec<Spot>,
    nut: Option<Note>,
    def: Option<Hoon>,
) -> Hoon {
    if list_spec.is_empty() {
        spore_recursion(spec, dom, hay, cox, bug, nut, def)
    } else {
        spore_buccen_recursion(
            list_spec
                .first()
                .expect("non-empty spec list checked above")
                .clone(),
            list_spec[1..].to_vec(),
            dom,
            hay,
            cox,
            bug,
            nut,
            def,
        )
    }
}

pub fn example(
    mod_: &Spec,
    dom: u64,
    hay: &WingType,
    cox: &HashMap<String, Spec>,
    bug: &Vec<Spot>,
    nut: &Option<Note>,
    def: &Option<Hoon>,
) -> Hoon {
    match mod_ {
        Spec::Base(b) => decorate(basal(b.clone()), bug.clone(), nut.clone()),
        Spec::Dbug(spot, inner) => {
            let mut bug = bug.clone();
            bug.insert(0, spot.clone());
            example(&inner, dom, hay, cox, &bug, nut, def)
        }
        Spec::Gist(help, inner) => example(
            &inner,
            dom,
            hay,
            cox,
            bug,
            &Some(Note::Help(help.clone())),
            def,
        ),
        Spec::Leaf(term, atom) => decorate(
            Hoon::Rock(term.clone(), NounExpr::ParsedAtom(atom.clone())),
            bug.clone(),
            nut.clone(),
        ),
        Spec::Like(wing, list) => {
            // hoon-138 `++example` handles `%like` by delegating through `%bcmc`:
            //   [%like *]  example(mod bcmc/(unreel p.mod q.mod))
            // The %bcmc case wraps in `=<($ ...)` so we get the gate's default
            // value rather than the gate itself.  This is critical for `++musk`
            // constant-folding: `*bloq` folds to `[1 0]` only when the example
            // expression is `=<($ bloq)` (which evaluates to the default sample)
            // rather than bare `bloq` (which evaluates to the whole gate).
            example(
                &Spec::BucMic(unreel(wing.clone(), list.clone())),
                dom,
                hay,
                cox,
                bug,
                nut,
                def,
            )
        }

        Spec::Loop(term) => Hoon::Limb(term.clone()),
        Spec::Made((t, list), inner) => {
            let pieces = list
                .iter()
                .map(|s| vec![Limb::Term(s.to_string())])
                .collect();
            example(
                &inner,
                dom,
                hay,
                cox,
                bug,
                &Some(Note::Made(t.to_string(), Some(pieces))),
                def,
            )
        }
        Spec::Make(head, tail) => example(
            &Spec::BucMic(unfold(head.clone(), tail.clone())),
            dom,
            hay,
            cox,
            bug,
            nut,
            def,
        ),
        Spec::Name(term, inner) => {
            // Canonical hoon-138 `++example` `%name`:
            //   [%name *]  example(mod q.mod, nut `made/[p.mod ~])
            // `%name` records metadata only; it does not wrap the produced value in `%ktts`.
            let nut = Some(Note::Made(term.to_string(), None));
            example(&inner, dom, hay, cox, bug, &nut, def)
        }
        Spec::Over(wing, inner) => example(&inner, dom, wing, cox, bug, nut, def),
        Spec::BucCab(p) => decorate(
            home(p.clone(), hay.clone(), dom.clone()),
            bug.clone(),
            nut.clone(),
        ),
        Spec::BucCol(head, tail) => {
            // Canonical tuple/spec cell construction is right-associated:
            // `[a b c]` => `[a [b c]]`.
            // Build from the rightmost element so head stays on the left.
            let mut items: Vec<&Spec> = Vec::with_capacity(tail.len().saturating_add(1));
            items.push(head.as_ref());
            items.extend(tail.iter());
            let mut iter = items.into_iter().rev();
            let mut result = example(
                iter.next().expect("BucCol has at least one item"),
                dom,
                hay,
                cox,
                &vec![],
                &None,
                &None,
            );
            for item in iter {
                let left = example(item, dom, hay, cox, &vec![], &None, &None);
                result = Hoon::Pair(Box::new(left), Box::new(result));
            }

            decorate(result, bug.clone(), nut.clone())
        }
        Spec::BucHep(p, q) => {
            let function_res = function(
                *p.clone(),
                *q.clone(),
                mod_,
                dom,
                hay,
                cox,
                &vec![],
                &None,
                &None,
            );
            decorate(function_res, bug.clone(), nut.clone())
        }
        Spec::BucMic(inner) => {
            let tsgl = Hoon::TisGal(
                Box::new(Hoon::Limb("$".to_string())),
                Box::new(inner.clone()),
            );
            decorate(
                home(tsgl, hay.clone(), dom.clone()),
                bug.clone(),
                nut.clone(),
            )
        }
        Spec::BucSig(inner, list) => Hoon::KetLus(
            Box::new(example(&list, dom, hay, cox, bug, nut, def)),
            Box::new(home(inner.clone(), hay.clone(), dom.clone())),
        ),
        Spec::BucLus(stud, inner) => decorate(
            Hoon::Note(
                Note::Know(stud.clone()),
                Box::new(example(&inner.clone(), dom, hay, cox, bug, nut, def)),
            ),
            bug.clone(),
            nut.clone(),
        ),
        Spec::BucTis(skin, inner) => {
            // hoon-138 `++example` `%bcts`:
            //   (decorate [%ktts p.mod example:clear(mod q.mod)])
            // The inner example keeps the same subject context, but clears accumulated
            // debug/default/note metadata before expanding the wrapped spec.
            let clear_bug = Vec::new();
            let clear_nut = None;
            let clear_def = None;
            decorate(
                Hoon::KetTis(
                    skin.clone(),
                    Box::new(example(
                        inner, dom, hay, cox, &clear_bug, &clear_nut, &clear_def,
                    )),
                ),
                bug.clone(),
                nut.clone(),
            )
        }
        Spec::BucDot(inner, map) => vair_case(
            Vair::Gold,
            *inner.clone(),
            map.clone(),
            mod_,
            dom,
            hay,
            cox,
            bug,
            nut,
            def,
        ),
        Spec::BucFas(inner, map) => vair_case(
            Vair::Iron,
            *inner.clone(),
            map.clone(),
            mod_,
            dom,
            hay,
            cox,
            bug,
            nut,
            def,
        ),
        Spec::BucZap(inner, map) => vair_case(
            Vair::Lead,
            *inner.clone(),
            map.clone(),
            mod_,
            dom,
            hay,
            cox,
            bug,
            nut,
            def,
        ),
        Spec::BucTic(inner, map) => vair_case(
            Vair::Zinc,
            *inner.clone(),
            map.clone(),
            mod_,
            dom,
            hay,
            cox,
            bug,
            nut,
            def,
        ),
        _ => {
            let spore_result = spore(
                mod_.clone(),
                dom.clone(),
                hay.clone(),
                cox.clone(),
                bug.clone(),
                nut.clone(),
                def.clone(),
            );
            let dom = peg(dom, 3).expect("example +peg failed");
            let relative_result = relative(2, mod_, dom, hay, cox, bug, nut, def);
            Hoon::TisLus(Box::new(spore_result), Box::new(relative_result))
        }
    }
}

// used in +example
fn vair_case(
    vair: Vair,
    payload: Spec,
    arms: HashMap<String, Spec>,
    mod_: &Spec,
    dom: u64,
    hay: &WingType,
    cox: &HashMap<String, Spec>,
    bug: &Vec<Spot>,
    nut: &Option<Note>,
    def: &Option<Hoon>,
) -> Hoon {
    let hoon = interface(vair, payload, arms, mod_, dom, hay, cox, bug, nut, def);
    decorate(
        home(hoon, hay.clone(), dom.clone()),
        bug.clone(),
        nut.clone(),
    )
}

pub fn basic(
    bas: BaseType,
    axe: u64,
    mod_: &Spec,
    dom: u64,
    hay: &WingType,
    cox: &HashMap<String, Spec>,
    bug: &Vec<Spot>,
    nut: &Option<Note>,
    def: &Option<Hoon>,
) -> Hoon {
    match bas {
        BaseType::Atom(a) => {
            let cnls = Hoon::CenLus(
                Box::new(Hoon::Limb("ruth".to_string())),
                Box::new(Hoon::Sand(
                    "ta".to_string(),
                    NounExpr::ParsedAtom(term_to_atom(a)),
                )),
                Box::new(Hoon::Axis(axe)),
            );

            let example_res = Box::new(example(mod_, dom, hay, cox, bug, nut, def));

            let wtpt_limb = Limb::Axis(axe);
            let wtpt_wing: Vec<Limb> = vec![wtpt_limb];
            let wtpt = Hoon::WutPat(wtpt_wing, Box::new(Hoon::Axis(axe)), Box::new(Hoon::ZapZap));

            let zppt_limb = Limb::Parent(0, Some("ruth".to_string()));
            let zppt_wing: Vec<Limb> = vec![zppt_limb];
            let zppt_list_wing: Vec<Vec<Limb>> = vec![zppt_wing];
            let zppt = Hoon::ZapPat(zppt_list_wing, Box::new(cnls), Box::new(wtpt));

            Hoon::KetLus(example_res, Box::new(zppt))
        }
        BaseType::Cell => {
            let example_res = Box::new(example(mod_, dom, hay, cox, bug, nut, def));
            let wing = Limb::Axis(axe);
            let wing: Vec<Limb> = vec![wing];
            let mut p = wing.clone();
            p.insert(0, Limb::Axis(2));
            let mut q = wing.clone();
            q.insert(0, Limb::Axis(3));
            let pair = Hoon::Pair(Box::new(Hoon::Wing(p)), Box::new(Hoon::Wing(q)));

            Hoon::KetLus(example_res, Box::new(pair))
        }
        BaseType::Flag => {
            let rock = Box::new(Hoon::Rock(
                "f".to_string(),
                NounExpr::ParsedAtom(ParsedAtom::Small(0)),
            ));
            let dtts = Box::new(Hoon::DotTis(
                Box::new(Hoon::Rock(
                    "$".to_string(),
                    NounExpr::ParsedAtom(ParsedAtom::Small(0)),
                )),
                Box::new(Hoon::Axis(axe)),
            ));
            let wtgr = Box::new(Hoon::WutGar(
                Box::new(Hoon::DotTis(
                    Box::new(Hoon::Rock(
                        "$".to_string(),
                        NounExpr::ParsedAtom(ParsedAtom::Small(1)),
                    )),
                    Box::new(Hoon::Axis(axe)),
                )),
                Box::new(Hoon::Rock(
                    "f".to_string(),
                    NounExpr::ParsedAtom(ParsedAtom::Small(1)),
                )),
            ));
            Hoon::WutCol(dtts, rock, wtgr)
        }
        BaseType::Null => {
            let rock = Box::new(Hoon::Rock(
                "n".to_string(),
                NounExpr::ParsedAtom(ParsedAtom::Small(0)),
            ));
            let dtts = Box::new(Hoon::DotTis(
                Box::new(Hoon::Bust(BaseType::NounExpr)),
                Box::new(Hoon::Axis(axe)),
            ));
            Hoon::WutGar(dtts, rock)
        }
        BaseType::NounExpr => Hoon::Axis(axe),
        BaseType::Void => Hoon::ZapZap,
    }
}

pub fn switch(
    one: Spec,
    mut rep: Vec<Spec>,
    axe: u64,
    mod_: &Spec,
    dom: u64,
    hay: &WingType,
    cox: &HashMap<String, Spec>,
    bug: &Vec<Spot>,
    nut: &Option<Note>,
    def: &Option<Hoon>,
) -> Hoon {
    if rep.is_empty() {
        return relative(axe, &one, dom, hay, cox, &vec![], &None, &None);
    }

    let mut iter = rep.into_iter();
    let i_rep = iter.next().expect("non-empty switch rep checked above");
    let t_rep: Vec<Spec> = iter.collect();

    let fin = switch(
        i_rep.clone(),
        t_rep,
        axe,
        mod_,
        dom,
        hay,
        cox,
        bug,
        nut,
        def,
    );

    let example_res = example(&one.clone(), dom, hay, cox, &vec![], &None, &None);

    let fits = Hoon::Fits(
        Box::new(Hoon::TisGal(Box::new(Hoon::Axis(2)), Box::new(example_res))),
        vec![Limb::Axis(peg(axe, 2).expect("+switch, peg failed!"))],
    );

    let relative_result = relative(axe, &one, dom, hay, cox, &vec![], &None, &None);

    Hoon::WutCol(Box::new(fits), Box::new(relative_result), Box::new(fin))
}

pub fn choice_(
    one: Spec,
    mut rep: Vec<Spec>,
    axe: u64,
    mod_: &Spec,
    dom: u64,
    hay: &WingType,
    cox: &HashMap<String, Spec>,
    bug: &Vec<Spot>,
    nut: &Option<Note>,
    def: &Option<Hoon>,
) -> Hoon {
    if rep.is_empty() {
        return relative(axe, &one, dom, hay, cox, &vec![], &None, &None);
    }

    let mut iter = rep.into_iter();
    let i_rep = iter.next().expect("non-empty choice rep checked above");
    let t_rep: Vec<Spec> = iter.collect();

    let example_res = example(&one.clone(), dom, hay, cox, &vec![], &None, &None);

    let fits = Hoon::Fits(Box::new(example_res), vec![Limb::Axis(axe)]);

    let relative_result = relative(axe, &one.clone(), dom, hay, cox, &vec![], &None, &None);
    let tail = choice_(
        i_rep.clone(),
        t_rep,
        axe,
        mod_,
        dom,
        hay,
        cox,
        bug,
        nut,
        def,
    );

    Hoon::WutCol(Box::new(fits), Box::new(relative_result), Box::new(tail))
}

pub fn relative(
    axe: u64,
    mod_: &Spec,
    dom: u64,
    hay: &WingType,
    cox: &HashMap<String, Spec>,
    bug: &Vec<Spot>,
    nut: &Option<Note>,
    def: &Option<Hoon>,
) -> Hoon {
    match &mod_ {
        Spec::Base(p) => decorate(
            basic(p.clone(), axe, mod_, dom, hay, cox, &vec![], &None, &None),
            bug.clone(),
            nut.clone(),
        ),
        Spec::Dbug(p, q) => {
            let mut bug = bug.clone();
            bug.insert(0, p.clone());
            relative(axe, &*q, dom, hay, cox, &bug, nut, def)
        }
        Spec::Gist(p, q) => relative(
            axe,
            &*q,
            dom,
            hay,
            cox,
            bug,
            &Some(Note::Help(p.clone())),
            def,
        ),
        Spec::Leaf(p, q) => decorate(
            Hoon::WutGar(
                Box::new(Hoon::DotTis(
                    Box::new(Hoon::Axis(axe)),
                    Box::new(Hoon::Rock("$".to_string(), NounExpr::ParsedAtom(q.clone()))),
                )),
                Box::new(Hoon::Rock(p.clone(), NounExpr::ParsedAtom(q.clone()))),
            ),
            bug.clone(),
            nut.clone(),
        ),
        Spec::Make(p, q) => relative(
            axe,
            &Spec::BucMic(unfold(p.clone(), q.clone())),
            dom,
            hay,
            cox,
            bug,
            nut,
            def,
        ),
        Spec::Like(p, q) => relative(
            axe,
            &Spec::BucMic(unreel(p.clone(), q.clone())),
            dom,
            hay,
            cox,
            bug,
            nut,
            def,
        ),
        Spec::Loop(p) => decorate(
            Hoon::CenHep(Box::new(Hoon::Limb(p.clone())), Box::new(Hoon::Axis(axe))),
            bug.clone(),
            nut.clone(),
        ),
        Spec::Name(p, q) => {
            // Canonical hoon-138 `++relative` `%name`:
            //   [%name *]  relative(mod q.mod, nut `made/[p.mod ~])
            // This is metadata-only and must not add a `%ktts` face wrapper.
            let nut = Some(Note::Made(p.clone(), None));
            relative(axe, &*q, dom, hay, cox, bug, &nut, def)
        }
        Spec::Made((term, list), q) => {
            let pieces = list
                .iter()
                .map(|s| vec![Limb::Term(s.to_string())])
                .collect();
            let nut = Some(Note::Made(term.clone(), Some(pieces)));
            relative(axe, &*q, dom, hay, cox, bug, &nut, def)
        }
        Spec::Over(p, q) => relative(axe, &*q, dom, p, cox, bug, nut, def),
        Spec::BucBuc(p, q) => {
            let new_dom = peg(3, dom).expect("+relative-bucbuc-peg-failed");
            let map: HashMap<String, Hoon> = q
                .into_iter()
                .map(|(term, spec)| {
                    (
                        term.clone(),
                        relative(axe, spec, new_dom, hay, cox, bug, nut, def),
                    )
                })
                .collect();
            Hoon::BarKet(
                Box::new(relative(axe, &*p, new_dom, hay, cox, bug, nut, def)),
                HashMap::from([("$".to_string(), (None, map))]),
            )
        }
        Spec::BucPam(p, q) => Hoon::TisLus(
            Box::new(relative(axe, &*p, dom, hay, cox, bug, nut, def)),
            Box::new(Hoon::TisLus(
                Box::new(Hoon::TisGar(Box::new(Hoon::Axis(3)), Box::new(q.clone()))),
                Box::new(Hoon::TisLus(
                    Box::new(Hoon::CenHep(
                        Box::new(Hoon::Axis(2)),
                        Box::new(Hoon::Axis(6)),
                    )),
                    Box::new(Hoon::WutGar(
                        Box::new(Hoon::WutBar(vec![
                            Hoon::DotTis(Box::new(Hoon::Axis(14)), Box::new(Hoon::Axis(2))),
                            Hoon::DotTis(
                                Box::new(Hoon::Axis(2)),
                                Box::new(Hoon::CenHep(
                                    Box::new(Hoon::Axis(6)),
                                    Box::new(Hoon::Axis(2)),
                                )),
                            ),
                        ])),
                        Box::new(Hoon::Axis(2)),
                    )),
                )),
            )),
        ),
        Spec::BucBar(p, q) => Hoon::TisLus(
            Box::new(relative(axe, &*p, dom, hay, cox, bug, nut, def)),
            Box::new(Hoon::WutGar(
                Box::new(Hoon::CenHep(
                    Box::new(Hoon::TisGar(Box::new(Hoon::Axis(3)), Box::new(q.clone()))),
                    Box::new(Hoon::Axis(2)),
                )),
                Box::new(Hoon::Axis(2)),
            )),
        ),
        Spec::BucCab(p) => decorate(
            home(p.clone(), hay.clone(), dom.clone()),
            bug.clone(),
            nut.clone(),
        ),
        Spec::BucCen(p, t) => decorate(
            switch(
                *p.clone(),
                t.clone(),
                axe,
                mod_,
                dom,
                hay,
                cox,
                bug,
                nut,
                def,
            ),
            bug.clone(),
            nut.clone(),
        ),
        Spec::BucCol(p, q) => {
            // hoon-138's %bccl in ++relative:
            //   %-  decorate                     :: wraps with current bug
            //   |-  ^-  hoon
            //   ?~  t.p.mod
            //     relative:clear(mod i.p.mod)    :: single element: clear
            //   :-  relative:clear(mod i.p.mod, axe (peg axe 2))  :: head: clear
            //   %=  relative                     :: tail: re-enter relative, preserving bug
            //     i.p.mod  i.t.p.mod
            //     t.p.mod  t.t.p.mod
            //     axe      (peg axe 3)
            //   ==
            //
            // The tail uses `%=  relative` (NOT `relative:clear`), so `bug` is
            // preserved through recursion.  Each re-entry into the BucCol case
            // applies `%-  decorate` again, producing one spot-hint per
            // intermediate cell level (n spots for n elements).
            if q.is_empty() {
                // Single element: relative:clear, then decorate with current bug
                decorate(
                    relative(axe, &*p, dom, hay, cox, &vec![], &None, &None),
                    bug.clone(),
                    nut.clone(),
                )
            } else {
                // Head: relative:clear
                let head = relative(
                    peg(axe, 2).expect("+relative-buccol-peg-failed"),
                    &*p,
                    dom,
                    hay,
                    cox,
                    &vec![],
                    &None,
                    &None,
                );
                // Tail: re-enter relative with remaining elements as BucCol,
                // preserving bug/nut/def (matching hoon-138's `%=  relative`)
                let remaining = Spec::BucCol(Box::new(q[0].clone()), q[1..].to_vec());
                let tail = relative(
                    peg(axe, 3).expect("+relative-buccol-peg-failed"),
                    &remaining,
                    dom,
                    hay,
                    cox,
                    bug,
                    nut,
                    def,
                );
                decorate(
                    Hoon::Pair(Box::new(head), Box::new(tail)),
                    bug.clone(),
                    nut.clone(),
                )
            }
        }
        Spec::BucGal(p, q) => Hoon::TisLus(
            Box::new(relative(axe, &*q, dom, hay, cox, &vec![], &None, &None)),
            Box::new(Hoon::WutGal(
                Box::new(Hoon::WutTis(
                    Box::new(Spec::Over(vec![Limb::Axis(3)], p.clone())),
                    vec![Limb::Axis(4)],
                )),
                Box::new(Hoon::Axis(2)),
            )),
        ),
        Spec::BucGar(p, q) => Hoon::TisLus(
            Box::new(relative(axe, &*q, dom, hay, cox, &vec![], &None, &None)),
            Box::new(Hoon::WutGar(
                Box::new(Hoon::WutTis(
                    Box::new(Spec::Over(vec![Limb::Axis(3)], p.clone())),
                    vec![Limb::Axis(4)],
                )),
                Box::new(Hoon::Axis(2)),
            )),
        ),
        Spec::BucHep(p, q) => {
            let function_res = function(
                *p.clone(),
                *q.clone(),
                mod_,
                dom,
                hay,
                cox,
                &vec![],
                &None,
                &None,
            );
            decorate(
                match def {
                    Some(d) => Hoon::KetLus(Box::new(function_res), Box::new(d.clone())),
                    None => function_res,
                },
                bug.clone(),
                nut.clone(),
            )
        }
        Spec::BucKet(p, q) => decorate(
            Hoon::WutCol(
                Box::new(Hoon::DotWut(Box::new(Hoon::Axis(
                    peg(axe, 2).expect("bucket-peg-failed"),
                )))),
                Box::new(relative(axe, &*p, dom, hay, cox, &vec![], &None, &None)),
                Box::new(relative(axe, &*q, dom, hay, cox, &vec![], &None, &None)),
            ),
            bug.clone(),
            nut.clone(),
        ),
        Spec::BucMic(p) => decorate(
            Hoon::CenCol(
                Box::new(home(p.clone(), hay.clone(), dom.clone())),
                vec![Hoon::Axis(axe)],
            ),
            bug.clone(),
            nut.clone(),
        ),
        Spec::BucSig(p, q) => relative(
            axe,
            &*q,
            dom,
            hay,
            cox,
            bug,
            nut,
            &Some(Hoon::KetHep(q.clone(), Box::new(p.clone()))),
        ),
        Spec::BucWut(p, t) => decorate(
            choice_(
                *p.clone(),
                t.clone(),
                axe,
                mod_,
                dom,
                hay,
                cox,
                bug,
                nut,
                def,
            ),
            bug.clone(),
            nut.clone(),
        ),
        Spec::BucTis(p, q) => Hoon::KetTis(
            p.clone(),
            Box::new(relative(axe, &*q, dom, hay, cox, bug, nut, def)),
        ),
        Spec::BucPat(p, q) => decorate(
            Hoon::WutCol(
                Box::new(Hoon::DotWut(Box::new(Hoon::Axis(axe)))),
                Box::new(relative(axe, &*q, dom, hay, cox, &vec![], &None, &None)),
                Box::new(relative(axe, &*p, dom, hay, cox, &vec![], &None, &None)),
            ),
            bug.clone(),
            nut.clone(),
        ),
        Spec::BucLus(p, q) => Hoon::Note(
            Note::Know(p.clone()),
            Box::new(relative(axe, &*q, dom, hay, cox, bug, nut, def)),
        ),
        Spec::BucDot(p, q) => {
            let x = interface(
                Vair::Gold,
                *p.clone(),
                q.clone(),
                mod_,
                dom,
                hay,
                cox,
                bug,
                nut,
                def,
            );
            let y = home(x, hay.clone(), dom.clone());
            decorate(y, bug.clone(), nut.clone())
        }

        Spec::BucFas(p, q) => {
            let x = interface(
                Vair::Iron,
                *p.clone(),
                q.clone(),
                mod_,
                dom,
                hay,
                cox,
                bug,
                nut,
                def,
            );
            let y = home(x, hay.clone(), dom.clone());
            decorate(y, bug.clone(), nut.clone())
        }

        Spec::BucZap(p, q) => {
            let x = interface(
                Vair::Lead,
                *p.clone(),
                q.clone(),
                mod_,
                dom,
                hay,
                cox,
                bug,
                nut,
                def,
            );
            let y = home(x, hay.clone(), dom.clone());
            decorate(y, bug.clone(), nut.clone())
        }

        Spec::BucTic(p, q) => {
            let x = interface(
                Vair::Zinc,
                *p.clone(),
                q.clone(),
                mod_,
                dom,
                hay,
                cox,
                bug,
                nut,
                def,
            );
            let y = home(x, hay.clone(), dom.clone());
            decorate(y, bug.clone(), nut.clone())
        }
    }
}

pub fn home(gen: Hoon, mut hay: WingType, dom: u64) -> Hoon {
    let wing = if dom == 1 {
        hay
    } else {
        hay.push(Limb::Axis(dom));
        hay
    };

    if wing.is_empty() {
        gen
    } else {
        Hoon::TisGar(Box::new(Hoon::Wing(wing)), Box::new(gen))
    }
}

pub fn unreel(one: WingType, res: Vec<WingType>) -> Hoon {
    if res.is_empty() {
        Hoon::Wing(one)
    } else {
        match res.first() {
            Some(first) => {
                let wing_tail = unreel(first.clone(), res[1..].to_vec());
                Hoon::TisGal(Box::new(Hoon::Wing(one)), Box::new(wing_tail))
            }
            None => Hoon::Wing(one),
        }
    }
}

pub fn unfold(fun: Hoon, arg: Vec<Spec>) -> Hoon {
    let cencol_tail: Vec<Hoon> = arg
        .iter()
        .map(|spec| Hoon::KetCol(Box::new(spec.clone())))
        .collect();
    Hoon::CenCol(Box::new(fun), cencol_tail)
}

pub fn factory(
    mod_: Spec,
    dom: u64,
    hay: WingType,
    cox: HashMap<String, Spec>,
    bug: Vec<Spot>,
    nut: Option<Note>,
    def: Option<Hoon>,
) -> Hoon {
    match mod_ {
        Spec::Dbug(spot, spec) => {
            let mut bug = bug.clone();
            bug.insert(0, spot);
            factory(*spec, dom, hay, cox, bug, nut, def)
        }
        Spec::BucSig(hoon, spec) => {
            let spec_clone = spec.clone();
            let spec_clone2 = spec.clone();
            factory(
                *spec_clone,
                dom,
                hay,
                cox,
                bug,
                nut,
                Some(Hoon::KetHep(spec_clone2, Box::new(hoon))),
            )
        }
        _ => match (def.clone(), mod_.clone()) {
            // hoon-138 `++factory` short-circuits when `def` is null and the spec
            // is an indirection (`%bcmc`, `%like`, `%loop`, `%make`):
            //   ?:  &(=(~ def) ?=(?(%bcmc %like %loop %make) -.mod))
            //     (decorate (home ...))
            (None, Spec::BucMic(h)) => decorate(home(h, hay, dom), bug, nut),
            (None, Spec::Like(wing, vec_wing)) => {
                decorate(home(unreel(wing, vec_wing), hay, dom), bug, nut)
            }
            (None, Spec::Loop(term)) => decorate(home(Hoon::Limb(term), hay, dom), bug, nut),
            (None, Spec::Make(h, s)) => decorate(home(unfold(h, s), hay, dom), bug, nut),
            _ => {
                let spore_res = spore(
                    mod_.clone(),
                    dom.clone(),
                    hay.clone(),
                    cox.clone(),
                    bug.clone(),
                    nut.clone(),
                    def.clone(),
                );

                let ketsig = Box::new(Hoon::KetSig(Box::new(spore_res)));

                let descent_axis = peg(7, dom).expect("factory-peg-failed");
                let tislus = Hoon::TisLus(
                    Box::new(Hoon::DotTis(
                        Box::new(Hoon::Axis(14)),
                        Box::new(Hoon::Axis(2)),
                    )),
                    Box::new(Hoon::Axis(6)),
                );
                let relative_res = relative(6, &mod_, descent_axis, &hay, &cox, &bug, &nut, &def);
                let tail = Hoon::TisLus(Box::new(relative_res), Box::new(tislus));

                Hoon::BarCol(ketsig, Box::new(tail))
            }
        },
    }
}

pub fn open(gen: Hoon) -> Hoon {
    match gen {
        Hoon::Axis(a) => Hoon::CenTis(vec![Limb::Axis(a)], Vec::new()),
        Hoon::Base(b) => factory(
            Spec::Base(b),
            1,
            Vec::new(),
            HashMap::new(),
            Vec::new(),
            None,
            None,
        ),
        Hoon::Bust(b) => example(
            &Spec::Base(b),
            1,
            &WingType::default(),
            &HashMap::new(),
            &Vec::new(),
            &None,
            &None,
        ),
        Hoon::KetCol(spec) => factory(*spec, 1, Vec::new(), HashMap::new(), Vec::new(), None, None),
        Hoon::Dbug(_, q) => *q,
        Hoon::Eror(s) => panic!("{}", s),
        Hoon::Knit(woofs) => {
            let ktts = Hoon::KetTis(Skin::Term("v".to_string()), Box::new(Hoon::Axis(1)));

            fn knit_loop(woofs: Vec<Woof>) -> Hoon {
                if woofs.is_empty() {
                    Hoon::Bust(BaseType::Null)
                } else {
                    let head = &woofs[0];
                    let tail = knit_loop(woofs[1..].to_vec());
                    match head {
                        Woof::ParsedAtom(a) => {
                            let sand =
                                Hoon::Sand("tD".to_string(), NounExpr::ParsedAtom(a.clone()));
                            Hoon::Pair(Box::new(sand), Box::new(tail))
                        }
                        Woof::Hoon(p) => {
                            let bindings = Hoon::Pair(
                                Box::new(Hoon::KetTis(
                                    Skin::Term("a".to_string()),
                                    Box::new(Hoon::KetLus(
                                        Box::new(Hoon::Limb("$".to_string())),
                                        Box::new(Hoon::TisGar(
                                            Box::new(Hoon::Limb("v".to_string())),
                                            Box::new(p.clone()),
                                        )),
                                    )),
                                )),
                                Box::new(Hoon::KetTis(Skin::Term("b".to_string()), Box::new(tail))),
                            );
                            let b = Hoon::BarHep(Box::new(Hoon::WutPat(
                                vec![Limb::Term("a".to_string())],
                                Box::new(Hoon::Limb("b".to_string())),
                                Box::new(Hoon::Pair(
                                    Box::new(Hoon::TisGal(
                                        Box::new(Hoon::Axis(2)),
                                        Box::new(Hoon::Limb("a".to_string())),
                                    )),
                                    Box::new(Hoon::CenTis(
                                        vec![Limb::Term("$".to_string())],
                                        vec![(
                                            vec![Limb::Term("a".to_string())],
                                            Hoon::TisGal(
                                                Box::new(Hoon::Axis(3)),
                                                Box::new(Hoon::Limb("a".to_string())),
                                            ),
                                        )],
                                    )),
                                )),
                            )));

                            Hoon::TisLus(Box::new(bindings), Box::new(b))
                        }
                    }
                }
            }

            let ktls = Hoon::KetLus(
                Box::new(Hoon::BarHep(Box::new(Hoon::WutCol(
                    Box::new(Hoon::Bust(BaseType::Flag)),
                    Box::new(Hoon::Bust(BaseType::Null)),
                    Box::new(Hoon::Pair(
                        Box::new(Hoon::KetTis(
                            Skin::Term("i".to_string()),
                            Box::new(Hoon::Sand(
                                "tD".to_string(),
                                NounExpr::ParsedAtom(ParsedAtom::Small(0)),
                            )),
                        )),
                        Box::new(Hoon::KetTis(
                            Skin::Term("t".to_string()),
                            Box::new(Hoon::Limb("$".to_string())),
                        )),
                    )),
                )))),
                Box::new(knit_loop(woofs)),
            );

            let brhp = Hoon::BarHep(Box::new(ktls));

            Hoon::TisGar(Box::new(ktts), Box::new(brhp))
        }
        Hoon::Leaf(term, atom) => factory(
            Spec::Leaf(term, atom),
            1,
            Vec::new(),
            HashMap::new(),
            Vec::new(),
            None,
            None,
        ),
        Hoon::Limb(term) => Hoon::CenTis(vec![Limb::Term(term)], Vec::new()),
        Hoon::Wing(wing) => Hoon::CenTis(wing, Vec::new()),
        Hoon::Note(_, q) => *q,

        Hoon::Tell(hoons) => {
            let zpgr = Hoon::ZapGar(Box::new(Hoon::ColTar(hoons)));
            Hoon::CenCol(Box::new(Hoon::Limb("noah".to_string())), vec![zpgr])
        }

        Hoon::Yell(hoons) => {
            let zpgr = Hoon::ZapGar(Box::new(Hoon::ColTar(hoons)));
            Hoon::CenCol(Box::new(Hoon::Limb("cain".to_string())), vec![zpgr])
        }

        Hoon::BarBuc(sample, body) => {
            if sample.is_empty() {
                panic!("empty sample in BarBuc");
            }

            fn barbuc_body(body: Spec) -> Hoon {
                match body {
                    Spec::Gist(help, inner) => {
                        Hoon::Note(Note::Help(help), Box::new(barbuc_body(*inner)))
                    }
                    other => Hoon::KetCol(Box::new(other)),
                }
            }

            let tar = Spec::Base(BaseType::NounExpr);
            let bcsg = Spec::BucSig(
                Hoon::Base(BaseType::NounExpr),
                Box::new(Spec::BucHep(Box::new(tar.clone()), Box::new(tar))),
            );

            let transformed: Vec<Spec> = sample
                .iter()
                .map(|term| Spec::BucTis(Skin::Term(term.clone()), Box::new(bcsg.clone())))
                .collect();

            let (first, rest) = transformed
                .split_first()
                .expect("non-empty |$ sample checked above");

            Hoon::BarTar(
                Box::new(Spec::BucCol(Box::new(first.clone()), rest.to_vec())),
                Box::new(barbuc_body(*body)),
            )
        }

        Hoon::BarCab(spec, alas, arms) => {
            let transformed_arms = arms
                .into_iter()
                .map(|(term, tome)| {
                    let (what, tome_map) = tome;
                    let wrapped_pairs: Vec<(String, Hoon)> = tome_map
                        .into_iter()
                        .map(|(face, expr)| {
                            let wrapped_expr =
                                alas.iter()
                                    .rev()
                                    .fold(expr, |body, (alas_face, alas_init)| {
                                        Hoon::TisTar(
                                            (alas_face.clone(), None),
                                            Box::new(alas_init.clone()),
                                            Box::new(body),
                                        )
                                    });
                            (face, wrapped_expr)
                        })
                        .collect();

                    let tome_map: HashMap<_, _> = wrapped_pairs.into_iter().collect();

                    (term, (what, tome_map))
                })
                .collect();

            Hoon::TisLus(
                Box::new(Hoon::KetTar(spec)),
                Box::new(Hoon::BarCen(None, transformed_arms)),
            )
        }

        Hoon::BarCol(p, q) => Hoon::TisLus(p, Box::new(Hoon::BarDot(q))),

        Hoon::BarDot(p) => {
            let map_term_hoon = {
                let mut m = HashMap::new();
                m.insert("$".to_string(), *p);
                m
            };
            let map_term_tome = {
                let mut m = HashMap::new();
                m.insert("$".to_string(), (None, map_term_hoon));
                m
            };
            Hoon::BarCen(None, map_term_tome)
        }

        Hoon::BarKet(p, arms) => {
            let mut map = arms.clone();
            if let Some(zil) = arms.get(&"$".to_string()) {
                let updated = {
                    let (what, mut inner) = zil.clone();
                    inner.insert("$".to_string(), *p.clone());
                    (what, inner)
                };
                map.insert("$".to_string(), updated);
            } else {
                let mut inner = HashMap::new();
                inner.insert("$".to_string(), *p.clone());
                map.insert("$".to_string(), (None, inner));
            }
            Hoon::TisGal(
                Box::new(Hoon::Limb("$".to_string())),
                Box::new(Hoon::BarCen(None, map)),
            )
        }

        Hoon::BarHep(p) => Hoon::TisGal(
            Box::new(Hoon::Limb("$".to_string())),
            Box::new(Hoon::BarDot(Box::new(*p))),
        ),

        Hoon::BarSig(spec, q) => Hoon::KetBar(Box::new(Hoon::BarTis(spec.clone(), q.clone()))),

        Hoon::BarTar(spec, q) => {
            let map_term_hoon = {
                let mut m = HashMap::new();
                m.insert("$".to_string(), *q);
                m
            };
            let map_term_tome = {
                let mut m = HashMap::new();
                m.insert("$".to_string(), (None, map_term_hoon));
                m
            };
            Hoon::TisLus(
                Box::new(Hoon::KetTar(spec)),
                Box::new(Hoon::BarPat(None, map_term_tome)),
            )
        }

        Hoon::BarTis(spec, q) => {
            let map_term_hoon = {
                let mut m = HashMap::new();
                m.insert("$".to_string(), *q);
                m
            };
            let map_term_tome = {
                let mut m = HashMap::new();
                m.insert("$".to_string(), (None, map_term_hoon));
                m
            };
            Hoon::BarCab(spec, vec![], map_term_tome)
        }

        Hoon::BarWut(p) => Hoon::KetWut(Box::new(Hoon::BarDot(p))),

        Hoon::ColKet(p, q, r, s) => {
            Hoon::Pair(p, Box::new(Hoon::Pair(q, Box::new(Hoon::Pair(r, s)))))
        }

        Hoon::ColCab(p, q) => Hoon::Pair(q, p),

        Hoon::ColHep(p, q) => Hoon::Pair(p, q),

        Hoon::ColLus(p, q, r) => Hoon::Pair(p, Box::new(Hoon::Pair(q, r))),

        Hoon::ColSig(hoons) => match hoons.as_slice() {
            [] => Hoon::Rock("n".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(0))),
            [h, tail @ ..] => {
                let rest = open(Hoon::ColSig(tail.to_vec()));
                Hoon::Pair(Box::new(h.clone()), Box::new(rest))
            }
        },

        Hoon::ColTar(hoons) => match hoons.as_slice() {
            [] => Hoon::ZapZap,
            [h] => h.clone(),
            [h, tail @ ..] => {
                let rest = open(Hoon::ColTar(tail.to_vec()));
                Hoon::Pair(Box::new(h.clone()), Box::new(rest))
            }
        },
        Hoon::KetTar(spec) => Hoon::KetSig(Box::new(example(
            &spec,
            1,
            &Vec::new(),
            &HashMap::new(),
            &Vec::new(),
            &None,
            &None,
        ))),

        Hoon::CenCab(wing, pairs) => Hoon::KetLus(
            Box::new(Hoon::Wing(wing.clone())),
            Box::new(Hoon::CenTis(wing, pairs)),
        ),

        Hoon::CenDot(p, q) => Hoon::CenCol(q, vec![*p]),

        Hoon::CenKet(p, q, r, s) => Hoon::CenCol(p, vec![*q, *r, *s]),

        Hoon::CenLus(p, q, r) => Hoon::CenCol(p, vec![*q, *r]),

        Hoon::CenHep(p, q) => Hoon::CenCol(p, vec![*q]),

        Hoon::CenCol(p, hoons) => Hoon::CenSig(vec![Limb::Term("$".to_string())], p, hoons),

        Hoon::CenSig(wing, p, hoons) => {
            fn compile_r_gen_rec(r_gen: &[Hoon], axe: u64) -> Vec<(Vec<Limb>, Hoon)> {
                match r_gen.split_first() {
                    None => vec![],
                    Some((hoon, rest)) => {
                        let (wing_axe, next_axe) = if rest.is_empty() {
                            (axe, 0)
                        } else {
                            (
                                peg(axe, 2).expect("+open: peg failed"),
                                peg(axe, 3).expect("+open: peg failed"),
                            )
                        };

                        let wing = vec![Limb::Parent(0, None), Limb::Axis(wing_axe)];

                        let mut out = vec![(wing, hoon.clone())];
                        if !rest.is_empty() {
                            out.extend(compile_r_gen_rec(rest, next_axe));
                        }
                        out
                    }
                }
            }
            let list = compile_r_gen_rec(&hoons, 6);
            Hoon::CenTar(wing, p, list)
        }

        Hoon::CenTar(mut wing, p, pairs) => {
            if pairs.is_empty() {
                return Hoon::TisGar(p, Box::new(Hoon::Wing(wing)));
            }
            wing.extend(vec![Limb::Axis(2)]);
            let wrapped = pairs
                .into_iter()
                .map(|(p, q)| (p, Hoon::TisGar(Box::new(Hoon::Axis(3)), Box::new(q))))
                .collect();
            Hoon::TisLus(p, Box::new(Hoon::CenTis(wing, wrapped)))
        }

        Hoon::KetDot(p, q) => Hoon::KetLus(Box::new(Hoon::CenCol(p, vec![*q.clone()])), q),

        Hoon::KetHep(spec, q) => {
            let example_res = example(
                &spec,
                1,
                &Vec::new(),
                &HashMap::new(),
                &Vec::new(),
                &None,
                &None,
            );
            Hoon::KetLus(Box::new(example_res), q)
        }

        Hoon::KetTis(skin, p) => grip(skin, *p, vec![]),

        Hoon::SigBar(p, q) => {
            let fek = {
                let fek = feck(*p.clone());
                match fek {
                    Some(s) => Hoon::Rock("tas".to_string(), NounExpr::ParsedAtom(s)),
                    None => Hoon::BarDot(Box::new(Hoon::CenCol(
                        Box::new(Hoon::Limb("cain".to_string())),
                        vec![Hoon::ZapGar(Box::new(Hoon::TisGar(Box::new(Hoon::Axis(3)), p)))],
                    ))),
                }
            };
            let hint = TermOrPair::Pair("mean".to_string(), Box::new(fek));
            Hoon::SigGar(hint, q)
        }

        Hoon::SigCab(p, q) => Hoon::SigGar(
            TermOrPair::Pair("mean".to_string(), Box::new(Hoon::BarDot(p))),
            q,
        ),

        Hoon::SigCen(chum, p, tyre, q) => {
            let clsg_vec = {
                let mut nob = vec![];
                let mut r = tyre;
                while !r.is_empty() {
                    let (p_i, q_i) = r.remove(0);
                    nob.push(Hoon::Pair(
                        Box::new(Hoon::Rock(
                            "$".to_string(),
                            NounExpr::ParsedAtom(string_to_atom(p_i)),
                        )),
                        Box::new(Hoon::ZapTis(Box::new(q_i))),
                    ));
                }
                nob
            };
            let clls = Hoon::ColLus(
                Box::new(Hoon::Rock("$".to_string(), chum_to_nounexpr(chum))),
                Box::new(Hoon::ZapTis(q.clone())),
                Box::new(Hoon::ColSig(clsg_vec)),
            );
            Hoon::SigGal(TermOrPair::Pair("fast".to_string(), Box::new(clls)), q)
        }

        Hoon::SigFas(chum, q) => Hoon::SigCen(chum, Box::new(Hoon::Axis(7)), vec![], q),

        Hoon::SigGal(term_or_pair, q) => Hoon::TisGal(
            Box::new(Hoon::SigGar(term_or_pair, Box::new(Hoon::Axis(1)))),
            q,
        ),

        Hoon::SigBuc(term, q) => Hoon::SigGar(
            TermOrPair::Pair(
                "live".to_string(),
                Box::new(Hoon::Rock(
                    "$".to_string(),
                    NounExpr::ParsedAtom(string_to_atom(term)),
                )),
            ),
            q,
        ),

        Hoon::SigLus(a, q) => Hoon::SigGar(
            TermOrPair::Pair(
                "memo".to_string(),
                Box::new(Hoon::Rock(
                    "$".to_string(),
                    NounExpr::ParsedAtom(ParsedAtom::Small(a.into())),
                )),
            ),
            q,
        ),

        Hoon::SigPam(a, p, q) => Hoon::SigGar(
            TermOrPair::Pair(
                "slog".to_string(),
                Box::new(Hoon::Pair(
                    Box::new(Hoon::Sand(
                        "$".to_string(),
                        NounExpr::ParsedAtom(ParsedAtom::Small(a.into())),
                    )),
                    Box::new(Hoon::CenCol(
                        Box::new(Hoon::Limb("cain".to_string())),
                        vec![Hoon::ZapGar(p)],
                    )),
                )),
            ),
            q,
        ),

        Hoon::SigTis(p, q) => Hoon::SigGar(TermOrPair::Pair("germ".to_string(), p), q),

        Hoon::SigWut(a, p, q, r) => {
            let wtdt = Hoon::WutDot(
                p,
                Box::new(Hoon::Bust(BaseType::Null)),
                Box::new(Hoon::Pair(
                    Box::new(Hoon::Bust(BaseType::Null)),
                    Box::new(*q),
                )),
            );
            let sgpm = Hoon::SigPam(
                a,
                Box::new(Hoon::Axis(5)),
                Box::new(Hoon::TisGar(Box::new(Hoon::Axis(3)), r.clone())),
            );
            let wtsg = Hoon::WutSig(
                vec![Limb::Axis(2)],
                Box::new(Hoon::TisGar(Box::new(Hoon::Axis(3)), r)),
                Box::new(sgpm),
            );
            Hoon::TisLus(Box::new(wtdt), Box::new(wtsg))
        }

        Hoon::MicTis(marl) => {
            fn loop_marl(marl: Marl) -> Hoon {
                match marl.split_first() {
                    None => Hoon::Bust(BaseType::Null),
                    Some((head, tail)) => match head {
                        Tuna::Manx(m) => Hoon::Pair(
                            Box::new(Hoon::Xray(m.clone())),
                            Box::new(loop_marl(tail.to_vec())),
                        ),
                        Tuna::TunaTail(TunaTail::Manx(m)) => {
                            Hoon::Pair(Box::new(m.clone()), Box::new(loop_marl(tail.to_vec())))
                        }
                        Tuna::TunaTail(TunaTail::Tape(t)) => Hoon::Pair(
                            Box::new(Hoon::MicFas(Box::new(t.clone()))),
                            Box::new(loop_marl(tail.to_vec())),
                        ),
                        Tuna::TunaTail(TunaTail::Call(h)) => {
                            Hoon::CenCol(Box::new(h.clone()), vec![loop_marl(tail.to_vec())])
                        }
                        Tuna::TunaTail(TunaTail::Marl(sub)) => {
                            let tsbr = Box::new(Hoon::TisBar(
                                Box::new(Spec::Base(BaseType::Cell)),
                                Box::new(Hoon::BarPat(None, {
                                    let sug = vec![Limb::Axis(12)];
                                    let wtsg = Hoon::WutSig(
                                        sug.clone(),
                                        Box::new(Hoon::CenTis(
                                            sug.clone(),
                                            vec![(vec![Limb::Axis(1)], Hoon::Axis(13))],
                                        )),
                                        Box::new(Hoon::CenTis(
                                            sug.clone(),
                                            vec![(
                                                vec![Limb::Axis(3)],
                                                Hoon::CenTis(
                                                    vec![Limb::Term("$".to_string())],
                                                    vec![(sug, Hoon::Axis(25))],
                                                ),
                                            )],
                                        )),
                                    );
                                    let map_term_hoon = {
                                        let mut m = HashMap::new();
                                        m.insert("$".to_string(), wtsg);
                                        m
                                    };
                                    let map_term_tome = {
                                        let mut m = HashMap::new();
                                        m.insert("$".to_string(), (None, map_term_hoon));
                                        m
                                    };
                                    map_term_tome
                                })),
                            ));
                            Hoon::CenDot(
                                Box::new(Hoon::Pair(
                                    Box::new(sub.clone()),
                                    Box::new(loop_marl(tail.to_vec())),
                                )),
                                tsbr,
                            )
                        }
                    },
                }
            }
            loop_marl(marl)
        }

        Hoon::MicCol(p, hoons) => match hoons.as_slice() {
            [] => Hoon::ZapZap,
            [h] => h.clone(),
            [h, tail @ ..] => {
                let yex = hoons;
                fn loop_yex(yex: &[Hoon]) -> Hoon {
                    match yex {
                        [] => panic!("empty yex"),
                        // hoon-138 `%mccl` open lowering terminal case:
                        //   [* ~]  [%tsgr [%$ 3] i.yex]
                        [h] => Hoon::TisGar(Box::new(Hoon::Axis(3)), Box::new(h.clone())),
                        [h, t @ ..] => Hoon::CenCol(
                            Box::new(Hoon::Axis(2)),
                            vec![
                                Hoon::TisGar(Box::new(Hoon::Axis(3)), Box::new(h.clone())),
                                loop_yex(t),
                            ],
                        ),
                        _ => panic!("miccol error"),
                    }
                }
                Hoon::TisLus(p, Box::new(loop_yex(&yex)))
            }
        },

        Hoon::MicFas(p) => {
            let zoy = Hoon::Rock("ta".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(0)));
            Hoon::ColSig(vec![Hoon::Pair(
                Box::new(zoy.clone()),
                Box::new(Hoon::ColSig(vec![Hoon::Pair(
                    Box::new(zoy.clone()),
                    p.clone(),
                )])),
            )])
        }

        Hoon::MicGal(spec, q, r, s) => {
            let ktcl_p = Hoon::KetCol(spec.clone());
            let cnhp = Hoon::CenHep(q, Box::new(ktcl_p));
            let brts = Hoon::BarTis(spec, Box::new(Hoon::TisGar(Box::new(Hoon::Axis(3)), s)));
            Hoon::CenLus(Box::new(cnhp), r, Box::new(brts))
        }

        Hoon::MicSig(p, q) => {
            fn loop_tail(p: Box<Hoon>, q: Vec<Hoon>) -> Hoon {
                match q.as_slice() {
                    [] => {
                        panic!("open-mcsg")
                    }
                    [first, rest @ ..] => {
                        if rest.is_empty() {
                            return Hoon::TisGar(
                                Box::new(Hoon::Limb("v".to_string())),
                                Box::new(first.clone()),
                            );
                        }
                        let a_bind = Hoon::KetTis(
                            Skin::Term("a".to_string()),
                            Box::new(loop_tail(p.clone(), rest.to_vec())),
                        );

                        let b_expr = Hoon::TisGar(
                            Box::new(Hoon::Limb("v".to_string())),
                            Box::new(first.clone()),
                        );
                        let b_bind = Hoon::KetTis(
                            Skin::Term("b".to_string()),
                            Box::new(Hoon::TisGar(
                                Box::new(Hoon::Limb("v".to_string())),
                                Box::new(first.clone()),
                            )),
                        );

                        let wing_c = vec![Limb::Parent(0, None), Limb::Axis(6)];
                        let c_expr = Hoon::TisGal(
                            Box::new(Hoon::Wing(wing_c)),
                            Box::new(Hoon::Limb("b".to_string())),
                        );
                        let c_bind = Hoon::KetTis(
                            Skin::Term("c".to_string()),
                            Box::new(Hoon::TisGal(
                                Box::new(Hoon::Wing(vec![Limb::Parent(0, None), Limb::Axis(6)])),
                                Box::new(Hoon::Limb("b".to_string())),
                            )),
                        );

                        let tsgr_v_p =
                            Hoon::TisGar(Box::new(Hoon::Limb("v".to_string())), p.clone());
                        let cncl_b_c = Hoon::CenCol(
                            Box::new(Hoon::Limb("b".to_string())),
                            vec![Hoon::Limb("c".to_string())],
                        );
                        let cnts_wing = vec![Limb::Parent(0, None), Limb::Axis(6)];
                        let cnts = Hoon::CenTis(
                            vec![Limb::Term("a".to_string())],
                            vec![(cnts_wing, Hoon::Limb("c".to_string()))],
                        );
                        let cnls =
                            Hoon::CenLus(Box::new(tsgr_v_p), Box::new(cncl_b_c), Box::new(cnts));

                        Hoon::TisLus(
                            Box::new(a_bind),
                            Box::new(Hoon::TisLus(
                                Box::new(b_bind),
                                Box::new(Hoon::TisLus(
                                    Box::new(c_bind),
                                    Box::new(Hoon::BarDot(Box::new(cnls))),
                                )),
                            )),
                        )
                    }
                }
            };

            let tail = loop_tail(p, q);

            Hoon::TisGar(
                Box::new(Hoon::KetTis(
                    Skin::Term("v".to_string()),
                    Box::new(Hoon::Axis(1)),
                )),
                Box::new(tail),
            )
        }

        Hoon::MicMic(spec, q) => Hoon::CenHep(
            Box::new(factory(
                *spec,
                1,
                Vec::new(),
                HashMap::new(),
                Vec::new(),
                None,
                None,
            )),
            q,
        ),

        Hoon::TisBar(spec, q) => Hoon::TisLus(Box::new(Hoon::KetTar(spec)), q),

        Hoon::TisTar((term, spec_opt), p, q) => {
            let inner = match spec_opt {
                None => *p,
                Some(spec_box) => Hoon::KetHep(spec_box, p),
            };
            let mut m = HashMap::new();
            m.insert(term, Some(inner));
            Hoon::TisGal(q, Box::new(Hoon::Tune(TermOrTune::Tune((m, vec![])))))
        }

        Hoon::TisCol(pairs, q) => {
            // hoon-138 `%tscl` open lowering:
            //   [%tsgr [%cncb [[%& 1] ~] p.gen] q.gen]
            let wing = vec![Limb::Axis(1)];
            Hoon::TisGar(Box::new(Hoon::CenCab(wing, pairs)), q)
        }

        Hoon::TisFas(skin, p, q) => Hoon::TisLus(Box::new(Hoon::KetTis(skin, p)), q),

        Hoon::TisMic(skin, p, q) => Hoon::TisFas(skin, q, p),

        Hoon::TisDot(wing, p, q) => Hoon::TisGar(
            Box::new(Hoon::CenCab(vec![Limb::Axis(1)], vec![(wing, *p)])),
            q,
        ),

        Hoon::TisWut(wing, p, q, r) => {
            let wtcl = Hoon::WutCol(p, q, Box::new(Hoon::Wing(wing.clone())));
            Hoon::TisDot(wing, Box::new(wtcl), r)
        }

        Hoon::TisGal(p, q) => Hoon::TisGar(q, p),

        Hoon::TisHep(p, q) => Hoon::TisLus(q, p),

        Hoon::TisKet(skin, wing, p, q) => {
            let wuy = weld(wing.clone(), vec![Limb::Term("v".to_string())]);
            let v_bind = Hoon::KetTis(Skin::Term("v".to_string()), Box::new(Hoon::Axis(1)));
            let a_bind = Hoon::KetTis(
                Skin::Term("a".to_string()),
                Box::new(Hoon::TisGar(
                    Box::new(Hoon::Limb("v".to_string())),
                    p.clone(),
                )),
            );
            let tsdt = Box::new(Hoon::TisDot(
                wuy.clone(),
                Box::new(Hoon::TisGal(
                    Box::new(Hoon::Axis(3)),
                    Box::new(Hoon::Limb("a".to_string())),
                )),
                Box::new(Hoon::TisGar(
                    Box::new(Hoon::Pair(
                        Box::new(Hoon::KetTis(
                            Skin::Over(vec![Limb::Term("v".to_string())], Box::new(skin)),
                            Box::new(Hoon::TisGal(
                                Box::new(Hoon::Axis(2)),
                                Box::new(Hoon::Limb("a".to_string())),
                            )),
                        )),
                        Box::new(Hoon::Limb("v".to_string())),
                    )),
                    q,
                )),
            ));
            Hoon::TisGar(
                Box::new(v_bind),
                Box::new(Hoon::TisLus(Box::new(a_bind), tsdt)),
            )
        }

        Hoon::TisLus(p, q) => Hoon::TisGar(Box::new(Hoon::Pair(p, Box::new(Hoon::Axis(1)))), q),

        Hoon::TisSig(hoons) => match hoons.as_slice() {
            [] => Hoon::Axis(1),
            [h] => h.clone(),
            [h, tail @ ..] => {
                let rest = open(Hoon::TisSig(tail.to_vec()));
                Hoon::TisGar(Box::new(h.clone()), Box::new(rest))
            }
        },
        Hoon::WutBar(p) => match p.as_slice() {
            [] => Hoon::Rock("f".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(1))),
            [head, tail @ ..] => {
                let recurse = open(Hoon::WutBar(tail.to_vec()));
                Hoon::WutCol(
                    Box::new(head.clone()),
                    Box::new(Hoon::Rock(
                        "f".to_string(),
                        NounExpr::ParsedAtom(ParsedAtom::Small(0)),
                    )),
                    Box::new(recurse),
                )
            }
        },

        Hoon::WutDot(p, q, r) => Hoon::WutCol(Box::new(*p), r, q),

        Hoon::WutGal(p, q) => Hoon::WutCol(Box::new(*p), Box::new(Hoon::ZapZap), q),

        Hoon::WutGar(p, q) => Hoon::WutCol(Box::new(*p), q, Box::new(Hoon::ZapZap)),

        Hoon::WutKet(p, q, r) => {
            let wuttis = Hoon::WutTis(Box::new(Spec::Base(BaseType::Atom("$".to_string()))), p);
            Hoon::WutCol(Box::new(wuttis), r, q)
        }

        Hoon::WutHep(p, q) => match q.as_slice() {
            [] => Hoon::Lost(Box::new(Hoon::Wing(p))),
            [(spec, head), tail @ ..] => {
                let wtts = Hoon::WutTis(Box::new(spec.clone()), p.clone());
                let recurse = open(Hoon::WutHep(p.clone(), tail.to_vec()));
                Hoon::WutCol(Box::new(wtts), Box::new(head.clone()), Box::new(recurse))
            }
        },

        Hoon::WutLus(p, q, r) => {
            let mut new_r = r.clone();
            new_r.push((Spec::Base(BaseType::NounExpr), *q));
            Hoon::WutHep(p, new_r)
        }

        Hoon::WutPam(p) => match p.as_slice() {
            [] => Hoon::Rock("f".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(0))),
            [head, tail @ ..] => {
                let recurse = open(Hoon::WutPam(tail.to_vec()));
                Hoon::WutCol(
                    Box::new(head.clone()),
                    Box::new(recurse),
                    Box::new(Hoon::Rock(
                        "f".to_string(),
                        NounExpr::ParsedAtom(ParsedAtom::Small(1)),
                    )),
                )
            }
        },

        Hoon::Xray(manx) => {
            let open_mane = match &manx.g.n {
                Mane::Tag(s) => Hoon::Rock(
                    "tas".to_string(),
                    NounExpr::ParsedAtom(string_to_atom(s.clone())),
                ),
                Mane::TagSpace(a, b) => {
                    let left = Hoon::Rock(
                        "tas".to_string(),
                        NounExpr::ParsedAtom(string_to_atom(a.clone())),
                    );
                    let right = Hoon::Rock(
                        "tas".to_string(),
                        NounExpr::ParsedAtom(string_to_atom(b.clone())),
                    );
                    Hoon::Pair(Box::new(left), Box::new(right))
                }
            };

            let clsg_items: Vec<Hoon> = manx
                .g
                .a
                .iter()
                .map(|(mane, beers)| {
                    let n_hoon = match &mane {
                        Mane::Tag(s) => Hoon::Rock(
                            "tas".to_string(),
                            NounExpr::ParsedAtom(string_to_atom(s.clone())),
                        ),
                        Mane::TagSpace(a, b) => {
                            let left = Hoon::Rock(
                                "tas".to_string(),
                                NounExpr::ParsedAtom(string_to_atom(a.clone())),
                            );
                            let right = Hoon::Rock(
                                "tas".to_string(),
                                NounExpr::ParsedAtom(string_to_atom(b.clone())),
                            );
                            Hoon::Pair(Box::new(left), Box::new(right))
                        }
                    };
                    let woofs: Vec<Woof> = beers
                        .iter()
                        .map(|b| match b {
                            Beer::Char(cord) => Woof::ParsedAtom(string_to_atom(cord.clone())),
                            Beer::Hoon(hoon) => Woof::Hoon(hoon.clone()),
                        })
                        .collect();

                    Hoon::Pair(Box::new(n_hoon), Box::new(Hoon::Knit(woofs)))
                })
                .collect();

            let clsg = Hoon::ColSig(clsg_items);
            let head = Hoon::Pair(Box::new(open_mane), Box::new(clsg));
            let tail = Hoon::MicTis(manx.c);

            Hoon::Pair(Box::new(head), Box::new(tail))
        }

        Hoon::WutPat(p, q, r) => {
            let wtts = Hoon::WutTis(Box::new(Spec::Base(BaseType::Atom("$".to_string()))), p);
            Hoon::WutCol(Box::new(wtts), q, r)
        }

        Hoon::WutSig(p, q, r) => {
            let wtts = Hoon::WutTis(Box::new(Spec::Base(BaseType::Null)), p);
            Hoon::WutCol(Box::new(wtts), q, r)
        }

        Hoon::WutTis(spec, q) => {
            let example_res = example(
                &spec,
                1,
                &Vec::new(),
                &HashMap::new(),
                &Vec::new(),
                &None,
                &None,
            );
            Hoon::Fits(Box::new(example_res), q)
        }

        Hoon::WutZap(p) => Hoon::WutCol(
            p,
            Box::new(Hoon::Rock(
                "f".to_string(),
                NounExpr::ParsedAtom(ParsedAtom::Small(1)),
            )),
            Box::new(Hoon::Rock(
                "f".to_string(),
                NounExpr::ParsedAtom(ParsedAtom::Small(0)),
            )),
        ),

        Hoon::ZapGar(p) => {
            let limb_onan = Hoon::Limb("onan".to_string());
            let limb_abel = Hoon::Limb("abel".to_string());
            let bcmc = Spec::BucMic(limb_abel);
            let kttr = Hoon::KetTar(Box::new(bcmc));
            let zpmc = Hoon::ZapMic(Box::new(kttr), p);

            Hoon::CenCol(Box::new(limb_onan), vec![zpmc])
        }

        Hoon::ZapWut(arg, q) => {
            const HOON_VERSION: u64 = 138; // hardcoded...

            let version_ok = match &arg {
                ZpwtArg::ParsedAtom(s) => s.parse::<u64>().map_or(false, |v| HOON_VERSION <= v),
                ZpwtArg::Pair(min_s, max_s) => match (min_s.parse::<u64>(), max_s.parse::<u64>()) {
                    (Ok(min), Ok(max)) => min <= HOON_VERSION && HOON_VERSION <= max,
                    _ => false,
                },
            };

            if version_ok {
                *q
            } else {
                panic!("hoon-version")
            }
        }

        _ => gen,
    }
}

pub fn chum_to_nounexpr(chum: Chum) -> NounExpr {
    match chum {
        Chum::Lef(term) => NounExpr::ParsedAtom(string_to_atom(term)),
        Chum::StdKel(term, u) => NounExpr::Cell(
            Box::new(NounExpr::ParsedAtom(string_to_atom(term))),
            Box::new(NounExpr::ParsedAtom(u)),
        ),
        Chum::VenProKel(t1, t2, u) => NounExpr::Cell(
            Box::new(NounExpr::ParsedAtom(string_to_atom(t1))),
            Box::new(NounExpr::Cell(
                Box::new(NounExpr::ParsedAtom(string_to_atom(t2))),
                Box::new(NounExpr::ParsedAtom(u)),
            )),
        ),
        Chum::VenProVerKel(t1, t2, u1, u2) => NounExpr::Cell(
            Box::new(NounExpr::ParsedAtom(string_to_atom(t1))),
            Box::new(NounExpr::Cell(
                Box::new(NounExpr::ParsedAtom(string_to_atom(t2))),
                Box::new(NounExpr::Cell(
                    Box::new(NounExpr::ParsedAtom(u1)),
                    Box::new(NounExpr::ParsedAtom(u2)),
                )),
            )),
        ),
    }
}

pub fn flay(gen: Hoon) -> Option<Skin> {
    match gen {
        Hoon::Pair(p, q) => {
            let maybe_p = flay(*p);
            let maybe_q = flay(*q);
            match (maybe_p, maybe_q) {
                (Some(p), Some(q)) => Some(Skin::Cell(Box::new(p), Box::new(q))),
                _ => None,
            }
        }

        Hoon::Base(b) => Some(Skin::Base(b.clone())),

        Hoon::Rock(t, n) => match n {
            NounExpr::ParsedAtom(a) => Some(Skin::Leaf(t.to_string(), a)),
            NounExpr::Cell(_, _) => None,
        },

        Hoon::CenTis(w, l) => match (w, l) {
            (v, l) if l.is_empty() => match v.as_slice() {
                [Limb::Term(t)] => Some(Skin::Term((*t).to_string())),
                _ => None,
            },
            _ => None,
        },

        Hoon::TisGar(p, q) => {
            let maybe_wing = reek(*p);
            match maybe_wing {
                Some(w) => {
                    let skin = flay(*q);
                    match skin {
                        None => None,
                        Some(s) => Some(Skin::Over(w, Box::new(s))),
                    }
                }
                None => None,
            }
        }

        Hoon::Note(Note::Help(help), q) => flay(*q).map(|skin| Skin::Help(help, Box::new(skin))),

        Hoon::Limb(t) => Some(Skin::Term(t.to_string())),

        Hoon::Wing(w) => match w.as_slice() {
            [Limb::Term(t)] => Some(Skin::Term(t.clone())),
            _ => {
                fn recur(w: &[Limb]) -> Option<Skin> {
                    match w {
                        [] => Some(Skin::Wash(0)),
                        [Limb::Parent(0, None), rest @ ..] => recur(rest),
                        _ => None,
                    }
                }
                recur(w.as_slice())
            }
        },

        Hoon::KetTar(s) => Some(Skin::Spec(
            s.clone(),
            Box::new(Skin::Base(BaseType::NounExpr)),
        )),

        Hoon::KetTis(skin, h) => {
            let maybe_skin = flay(*h);
            match maybe_skin {
                Some(s) => match skin {
                    Skin::Term(ref t) => Some(Skin::Name(t.to_string(), Box::new(s))),
                    Skin::Name(ref t, ref b) if matches!(**b, Skin::Base(BaseType::NounExpr)) => {
                        Some(Skin::Name(t.clone(), Box::new(s)))
                    }
                    _ => None,
                },
                None => None,
            }
        }

        _ => {
            let desugared = open(gen.clone());
            if desugared == gen {
                None
            } else {
                flay(desugared)
            }
        }
    }
}

pub fn feck(gen: Hoon) -> Option<ParsedAtom> {
    match gen {
        Hoon::Sand(term, noun) => {
            if term == "tas" {
                match noun {
                    NounExpr::ParsedAtom(s) => Some(s),
                    NounExpr::Cell(_, _) => None,
                }
            } else {
                None
            }
        }

        Hoon::Dbug(_spot, expr) => feck(*expr),

        _ => None,
    }
}

pub fn grip(skin: Skin, gen: Hoon, rel: WingType) -> Hoon {
    match skin {
        Skin::Term(term) => {
            Hoon::TisGal(Box::new(Hoon::Tune(TermOrTune::Term(term))), Box::new(gen))
        }

        Skin::Base(base) => {
            if base == BaseType::NounExpr {
                gen
            } else {
                Hoon::KetHep(Box::new(Spec::Base(base)), Box::new(gen))
            }
        }

        Skin::Cell(car_skin, cdr_skin) => {
            let haf = half(gen.clone());
            match haf {
                None => {
                    let car_gen = Hoon::Axis(4);
                    let cdr_gen = Hoon::Axis(5);
                    let pair = Hoon::Pair(
                        Box::new(grip(*car_skin, car_gen, rel.clone())),
                        Box::new(grip(*cdr_skin, cdr_gen, rel.clone())),
                    );
                    Hoon::TisLus(Box::new(gen), Box::new(pair))
                }
                Some((p, q)) => Hoon::Pair(
                    Box::new(grip(*car_skin, p, rel.clone())),
                    Box::new(grip(*cdr_skin, q, rel.clone())),
                ),
            }
        }

        Skin::Dbug(spot, inner_skin) => Hoon::Dbug(spot, Box::new(grip(*inner_skin, gen, rel))),

        Skin::Help(help, inner_skin) => {
            Hoon::Note(Note::Help(help), Box::new(grip(*inner_skin, gen, rel)))
        }

        Skin::Leaf(aura, atom) => Hoon::KetHep(Box::new(Spec::Leaf(aura, atom)), Box::new(gen)),

        Skin::Name(term, inner_skin) => Hoon::TisGal(
            Box::new(Hoon::Tune(TermOrTune::Term(term))),
            Box::new(grip(*inner_skin, gen, rel)),
        ),

        Skin::Over(mut wing, inner_skin) => {
            wing.extend(rel);
            grip(*inner_skin, gen, wing)
        }

        Skin::Spec(spec, inner_skin) => {
            let check_skin = if rel.is_empty() {
                spec
            } else {
                Box::new(Spec::Over(rel.clone(), spec))
            };

            let inner = grip(*inner_skin, gen, rel);

            Hoon::KetHep(check_skin, Box::new(inner))
        }

        Skin::Wash(depth) => {
            let wing: WingType = (0..depth).map(|_| Limb::Parent(0, None)).collect();
            Hoon::TisGal(Box::new(Hoon::Wing(wing)), Box::new(gen))
        }
    }
}

pub fn half(gen: Hoon) -> Option<(Hoon, Hoon)> {
    match gen {
        Hoon::Pair(car, cdr) => Some((*car, *cdr)),

        Hoon::Dbug(_spot, expr) => half(*expr),

        Hoon::ColCab(car, cdr) => Some((*cdr, *car)),

        Hoon::ColHep(car, cdr) => Some((*car, *cdr)),

        Hoon::ColKet(a, b, c, d) => {
            let tail = Hoon::ColLus(b, c, d);
            Some((*a, tail))
        }

        Hoon::ColSig(mut items) => {
            if items.is_empty() {
                None
            } else {
                let head = items.remove(0);
                Some((head, Hoon::ColSig(items)))
            }
        }

        Hoon::ColTar(mut items) => {
            if items.is_empty() {
                None
            } else if items.len() == 1 {
                half(items.remove(0))
            } else {
                let head = items.remove(0);
                let tail = Hoon::ColTar(items);
                Some((head, tail))
            }
        }

        _ => None,
    }
}

pub fn reek(gen: Hoon) -> Option<WingType> {
    match gen {
        Hoon::Pair(p, _q) => match *p {
            Hoon::Axis(a) => Some(vec![Limb::Axis(a)]),
            _ => None,
        },
        Hoon::Limb(t) => Some(vec![Limb::Term(t.clone())]),
        Hoon::Wing(w) => Some(w.to_vec()),
        Hoon::CenTis(wing, ref pairs) if pairs.is_empty() => Some(wing),
        Hoon::CenCab(wing, ref pairs) if pairs.is_empty() => Some(wing),
        Hoon::Dbug(_s, h) => reek(*h),
        _ => None,
    }
}

pub fn name_ax(gen: Hoon) -> Option<String> {
    match gen {
        Hoon::Wing(p) => {
            if p.is_empty() {
                None
            } else if let Some(i) = p.first() {
                match i {
                    Limb::Axis(_) => None,
                    Limb::Term(q) => Some(q.to_string()),
                    Limb::Parent(_, q) => q.clone(),
                }
            } else {
                None
            }
        }
        Hoon::Limb(p) => Some(p),
        Hoon::Dbug(_, q) => name_ax(*q),
        Hoon::TisGal(p, q) => name_ax(Hoon::TisGar(q, p)),
        Hoon::TisGar(_, q) => name_ax(*q),
        _ => None,
    }
}

pub fn autoname(mod_spec: Spec) -> Option<String> {
    //  ++autoname:ax
    match mod_spec {
        Spec::Base(base) => match base {
            BaseType::Atom(aura) => {
                if aura == "$" {
                    //  how empty terms will be represented here in rust land?...
                    Some("atom".to_string())
                } else {
                    Some(aura)
                }
            }
            _ => None,
        },
        Spec::Dbug(_, q) => autoname(*q),
        Spec::Gist(_, q) => autoname(*q),
        Spec::Leaf(p, _) => Some(p),
        Spec::Loop(p) => Some(p),
        Spec::Like(wing, _list_wing) => {
            if wing.is_empty() {
                None
            } else if let Some(i) = wing.first() {
                match i {
                    Limb::Axis(_) => None,
                    Limb::Term(q) => Some(q.to_string()),
                    Limb::Parent(_, q) => q.clone(),
                }
            } else {
                None
            }
        }
        Spec::Make(p, _) => name_ax(p),
        Spec::Made(_, q) => autoname(*q),
        Spec::Name(_, q) => autoname(*q),
        Spec::Over(_, q) => autoname(*q),
        Spec::BucBuc(p, _) => autoname(*p),
        Spec::BucBar(p, _) => autoname(*p),
        Spec::BucCab(p) => name_ax(p),
        Spec::BucCol(i, _) => autoname(*i),
        Spec::BucCen(i, _) => autoname(*i),
        Spec::BucDot(_, _) => None,
        Spec::BucGal(_, q) => autoname(*q),
        Spec::BucGar(_, q) => autoname(*q),
        Spec::BucHep(p, _) => autoname(*p),
        Spec::BucKet(_, q) => autoname(*q),
        Spec::BucLus(_, q) => autoname(*q),
        Spec::BucFas(_, _) => None,
        Spec::BucMic(p) => name_ax(p),
        Spec::BucPam(p, _) => autoname(*p),
        Spec::BucSig(_, q) => autoname(*q),
        Spec::BucTic(_, _) => None,
        Spec::BucTis(_, q) => autoname(*q),
        Spec::BucPat(_, q) => autoname(*q),
        Spec::BucWut(i, _) => autoname(*i),
        Spec::BucZap(_, _) => None,
    }
}

pub fn decorate(gen: Hoon, bug: Vec<Spot>, nut: Option<Note>) -> Hoon {
    let mut out = gen;

    for spot in bug.into_iter().rev() {
        out = Hoon::Dbug(spot, Box::new(out));
    }

    match nut {
        None => out,
        Some(note) => Hoon::Note(note, Box::new(out)),
    }
}

pub fn blue(tik: Tiki, gen: Hoon) -> Hoon {
    match tik {
        Tiki::Hoon((None, h)) => Hoon::TisGar(Box::new(Hoon::Axis(3)), Box::new(gen)),
        _ => gen,
    }
}

pub fn teal(tik: Tiki, mod_: Spec) -> Spec {
    match tik {
        Tiki::Wing((_, _)) => mod_,
        Tiki::Hoon((_, _)) => Spec::Over(vec![Limb::Axis(3)], Box::new(mod_)),
    }
}

pub fn tele(tik: Tiki, syn: Skin) -> Skin {
    match tik {
        Tiki::Wing((_, _)) => syn,
        Tiki::Hoon((_, _)) => Skin::Over(vec![Limb::Axis(3)], Box::new(syn)),
    }
}

pub fn gray(tik: Tiki, gen: Hoon) -> Hoon {
    match tik {
        Tiki::Wing((p, q)) => match p {
            None => gen,
            Some(u) => Hoon::TisTar((u, None), Box::new(Hoon::Wing(q)), Box::new(gen)),
        },
        Tiki::Hoon((p, q)) => {
            let arg = match p {
                None => q,
                Some(u) => Box::new(Hoon::KetTis(Skin::Term(u), q)),
            };
            Hoon::TisLus(arg, Box::new(gen))
        }
    }
}

pub fn puce(tik: Tiki) -> WingType {
    match tik {
        Tiki::Wing((p, q)) => match p {
            None => q,
            Some(u) => vec![Limb::Term(u)],
        },
        Tiki::Hoon((_, _)) => vec![Limb::Axis(2)],
    }
}

pub fn wthp(tik: Tiki, opt: Vec<(Spec, Hoon)>) -> Hoon {
    let mapped = opt
        .into_iter()
        .map(|(a, b)| (a, blue(tik.clone(), b)))
        .collect::<Vec<(Spec, Hoon)>>();
    gray(tik.clone(), Hoon::WutHep(puce(tik.clone()), mapped))
}

pub fn wtkt(tik: Tiki, sic: Hoon, non: Hoon) -> Hoon {
    gray(
        tik.clone(),
        Hoon::WutKet(
            puce(tik.clone()),
            Box::new(blue(tik.clone(), sic)),
            Box::new(blue(tik.clone(), non)),
        ),
    )
}

pub fn wtls(tik: Tiki, gen: Hoon, opt: Vec<(Spec, Hoon)>) -> Hoon {
    let mapped = opt
        .into_iter()
        .map(|(a, b)| (a, blue(tik.clone(), b)))
        .collect::<Vec<(Spec, Hoon)>>();
    gray(
        tik.clone(),
        Hoon::WutLus(puce(tik.clone()), Box::new(blue(tik.clone(), gen)), mapped),
    )
}

pub fn wtpt(tik: Tiki, sic: Hoon, non: Hoon) -> Hoon {
    gray(
        tik.clone(),
        Hoon::WutPat(
            puce(tik.clone()),
            Box::new(blue(tik.clone(), sic)),
            Box::new(blue(tik.clone(), non)),
        ),
    )
}

pub fn wtsg(tik: Tiki, sic: Hoon, non: Hoon) -> Hoon {
    gray(
        tik.clone(),
        Hoon::WutSig(
            puce(tik.clone()),
            Box::new(blue(tik.clone(), sic)),
            Box::new(blue(tik.clone(), non)),
        ),
    )
}

pub fn wthx(tik: Tiki, syn: Skin) -> Hoon {
    gray(
        tik.clone(),
        Hoon::WutHax(tele(tik.clone(), syn), puce(tik.clone())),
    )
}

pub fn wtts(tik: Tiki, mod_: Spec) -> Hoon {
    gray(
        tik.clone(),
        Hoon::WutTis(Box::new(teal(tik.clone(), mod_)), puce(tik.clone())),
    )
}

pub fn right_child(n: u64) -> u64 {
    if n == 0 {
        1
    } else {
        (2 * right_child(n - 1)) + 1
    }
}

pub fn left_child(n: u64) -> u64 {
    if n == 0 {
        0
    } else {
        2 * (left_child(n - 1) + 1)
    }
}

pub fn peg(a: u64, b: u64) -> Result<u64, &'static str> {
    if a == 0 || b == 0 {
        return Err("peg: a and b must be non-zero");
    }

    let k = b.ilog2();
    let offset = b & ((1u64 << k) - 1);
    Ok((a << k) + offset)
}

// non-control ASCII (32-255, excluding 127/DEL)
fn non_control_char<'src>() -> impl Parser<'src, &'src str, char, Err<'src>> {
    any()
        .filter(|c: &char| {
            let code = *c as u32;
            (code >= 0x20 && code < 0x7F) || code >= 0x80
        })
        .labelled("Non-Control Character")
}

fn gah<'src>() -> impl Parser<'src, &'src str, (), Err<'src>> {
    choice((just(' ').ignored(), newline())).labelled("Space or NewLine")
}

pub fn vul<'src>() -> impl Parser<'src, &'src str, (), Err<'src>> {
    just("::")
        .ignore_then(non_control_char().repeated())
        .ignore_then(newline())
        .ignored()
        .labelled("Comments")
}

fn gaq<'src>() -> impl Parser<'src, &'src str, (), Err<'src>> {
    choice((
        newline().ignored(),
        gah().ignore_then(choice((gah().ignored(), vul()))),
        vul(),
    ))
    .ignored()
    .labelled("End of Line")
}

pub fn gap<'src>() -> impl Parser<'src, &'src str, (), Err<'src>> {
    gaq()
        .then_ignore(choice((vul(), gah().ignored())).repeated().or_not())
        .ignored()
        .labelled("Gap")
}

pub fn list_term_hoon<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Vec<(String, Hoon)>, Err<'src>> {
    symbol()
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .repeated()
        .at_least(1)
        .collect::<Vec<(String, Hoon)>>()
}

pub fn list_names_tall<'src>() -> impl Parser<'src, &'src str, Vec<String>, Err<'src>> {
    symbol()
        .separated_by(gap())
        .at_least(1)
        .collect::<Vec<_>>()
        .then_ignore(gap().ignore_then(just("==")))
}

pub fn list_names_wide<'src>() -> impl Parser<'src, &'src str, Vec<String>, Err<'src>> {
    symbol()
        .separated_by(just(' '))
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(just("["), just("]"))
}

pub fn winglist<'src>() -> impl Parser<'src, &'src str, WingType, Err<'src>> {
    let name =      //  Name or $
        just('$')
            .to("$".to_string())
            .or(symbol());

    let com =   //  ,
        just(',')
        .to(Limb::Parent(0, None));

    let ket_name =   //  ^^name or name
        just('^')
            .repeated()
            .count()
            .then(name)
            .map(|(cnt, name)| {
                if cnt == 0 {
                    return Limb::Term(name);
                } else {
                    return Limb::Parent(cnt as u64, Some(name));
                }
            });

    let lus_number =   //  +10
            just('+')
                .ignore_then(digits())
                .map(|s| {
                    let num = s.parse::<u64>().expect("axis digits should parse as u64");
                    Limb::Axis(num)
                });

    let pam_number =   //  &10
            just('&')
                .ignore_then(digits())
                .map(|s| {
                    let num = s.parse::<u64>().expect("axis digits should parse as u64");
                    Limb::Axis(left_child(num))
                });

    let bar_number =  //  |10
           just('|').ignore_then(digits())
                .map(|s| {
                    let num = s.parse::<u64>().expect("axis digits should parse as u64");
                    Limb::Axis(right_child(num))
                });

    let dot =  //  .
            just('.').to(Limb::Axis(1));

    let lus =  //  +
        just('+').to(Limb::Axis(3));

    let hep =  //  -
        just('-').to(Limb::Axis(2));

    let sign = any().filter(|c: &char| *c == '+' || *c == '-');
    let angle = any().filter(|c: &char| *c == '<' || *c == '>');

    let lark =   //    +>-<  notation
            sign
                .then(angle)
                .repeated()
                .at_least(1)
                .collect::<Vec<_>>()
            .then(sign.or_not())
            .map(|(pairs, tail)| {
                let mut out = String::new();
                for (s, a) in pairs {
                    out.push(s);
                    out.push(a);
                }
                if let Some(t) = tail {
                    out.push(t);
                }
                out
            })
            .map(|s: String| {
                let mut axis = 1;
                for c in s.chars() {
                    match c {
                        '+' | '>' => axis = peg(axis, 3).expect("lark axis calculation failed"),
                        '-' | '<' => axis = peg(axis, 2).expect("lark axis calculation failed"),
                        _ => axis = 1,
                    }
                }
                Limb::Axis(axis)
            }).labelled("Lark Expression");

    choice((
        com, ket_name, lus_number, pam_number, bar_number, lark, dot, lus, hep,
    ))
    .separated_by(just('.'))
    .at_least(1)
    .collect::<Vec<_>>()
    .labelled("Wing")
}

pub fn variable_name_and_type<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Skin, Err<'src>> {
    let not_named = just('=') // =/  =foo
        .ignore_then(spec_wide.clone())
        .try_map(|spec, span| {
            let auto = autoname(spec.clone());
            match auto {
                None => Err(Rich::custom(span, "cannot autoname")),
                Some(term) => Ok(Skin::Name(
                    term,
                    Box::new(Skin::Spec(
                        Box::new(spec),
                        Box::new(Skin::Base(BaseType::NounExpr)),
                    )),
                )),
            }
        });

    let name_or_namedspec = symbol() //  =/  a=foo  ,  =/  a
        .then(
            just('/')
                .or(just('='))
                .ignore_then(spec_wide.clone())
                .or_not(),
        )
        .map(|(term, maybe_spec)| match maybe_spec {
            None => Skin::Term(term),
            Some(spec) => Skin::Name(
                term,
                Box::new(Skin::Spec(
                    Box::new(spec),
                    Box::new(Skin::Base(BaseType::NounExpr)),
                )),
            ),
        });

    let just_type = spec_wide
        .clone() // =/  type
        .map(|s| Skin::Spec(Box::new(s), Box::new(Skin::Base(BaseType::NounExpr))));

    choice((not_named, name_or_namedspec, just_type))
}

// ++  si                                                  ::  signed integer
pub fn syn_si(a: u128) -> bool {
    end_u128(0, 1, a) == 0
}

pub fn abs_si(a: u128) -> u128 {
    let rsh_res = rsh_u128(0, 1, a);
    let end_res = end_u128(0, 1, a.clone());
    end_res + rsh_res
}

pub fn old_si(a: u128) -> (bool, u128) {
    (syn_si(a), abs_si(a))
}
pub fn new_si(sign: bool, mag: u128) -> u128 {
    if mag == 0 {
        0
    } else if sign {
        mag << 1
    } else {
        (mag << 1) - 1
    }
}
fn sun_si(a: u128) -> u128 {
    a << 1
}

pub fn sum_si(a: u128, b: u128) -> u128 {
    let (c_sign, c_mag) = old_si(a);
    let (d_sign, d_mag) = old_si(b);
    match (c_sign, d_sign) {
        (false, false) => new_si(false, c_mag.wrapping_add(d_mag)),
        (false, true) => {
            if c_mag >= d_mag {
                new_si(false, c_mag - d_mag)
            } else {
                new_si(true, d_mag - c_mag)
            }
        }
        (true, false) => {
            if c_mag >= d_mag {
                new_si(true, c_mag - d_mag)
            } else {
                new_si(false, d_mag - c_mag)
            }
        }
        (true, true) => new_si(true, c_mag.wrapping_add(d_mag)),
    }
}

pub fn dif_si(a: u128, b: u128) -> u128 {
    let (b_sign, b_mag) = old_si(b);
    let neg_b = new_si(!b_sign, b_mag);
    sum_si(a, neg_b)
}

pub fn me(b: u128, p: u128) -> u128 {
    let t = dif_si(2, b);
    let p_si = sun_si(p);
    dif_si(t, p_si)
}

pub fn sig(p: usize, w: usize, a: &ParsedAtom) -> bool {
    let bit = cut(0, p + w, 1, a);
    match bit {
        ParsedAtom::Small(0) => true,
        ParsedAtom::Small(1) => false,
        _ => unreachable!(),
    }
}

pub fn sea(w: u128, p: u128, b: u128, a: &ParsedAtom) -> BinaryFloat {
    let f = cut(0, 0, p as usize, a);
    let e_atom = cut(0, p as usize, w as usize, a);
    let s = sig(p as usize, w as usize, a);

    let e = match e_atom {
        ParsedAtom::Small(x) => x,
        ParsedAtom::Big(_) => panic!("exponent field >128 bits"),
    };
    let f_u128 = match f {
        ParsedAtom::Small(x) => x,
        ParsedAtom::Big(_) => panic!("mantissa field >128 bits"),
    };

    let max_exp_field = sub_or_panic(bex(w), 1); // bex(w) >= 1

    if e == 0 {
        if f_u128 == 0 {
            BinaryFloat::Finite {
                sign: s,
                exp: 0,
                mant: BigUint::zero(),
            }
        } else {
            let me_val = me(b, p);
            BinaryFloat::Finite {
                sign: s,
                exp: me_val,
                mant: BigUint::from(f_u128),
            }
        }
    } else if e == max_exp_field {
        if f_u128 == 0 {
            BinaryFloat::Infinity { sign: s }
        } else {
            BinaryFloat::NaN
        }
    } else {
        let me_val = me(b, p);
        let q = sum_si(sum_si(sun_si(e), me_val), 1); // e + me + (-1)

        let r = f_u128.wrapping_add(bex(p));

        BinaryFloat::Finite {
            sign: s,
            exp: q,
            mant: BigUint::from(r),
        }
    }
}

//  inner function for drg_fl
pub fn drg(e: u128, a: BigUint, p: u128, v: u128, w: u128, d: char) -> (u128, BigUint) {
    assert!(!a.is_zero(), "drg: mantissa must be nonzero");
    eprintln!("drg caleed e {} a {} p {} v {} w {} d {}", e, a, p, v, w, d);
    // drg caleed e 43 a 13176795 p 24 v 299 w 253 d d
    //  it should return (13, 31.415.927)
    //  but it returns 0 and 13176795

    let (e, a) = xpd(e, a, d, p, v);
    eprintln!("xpd result: e:{} a:{}", e, a);
    assert!(!a.is_zero(), "xpd must not produce zero in drg");

    let (mut r, mut s, mut mn, mut mp) = {
        if syn_si(e) {
            let shift = abs_si(e) as usize;
            let r = lsh_big(0, shift, &a.clone());
            let s = BigUint::one();
            let mn = BigUint::one();
            let mp = BigUint::one();
            (r, s, mn, mp)
        } else {
            let shift = abs_si(e) as usize;
            let s = lsh_big(0, shift, &BigUint::one());
            let r = a.clone();
            let mn = BigUint::one();
            let mp = BigUint::one();
            (r, s, mn, mp)
        }
    };

    eprintln!("r: {} s: {} mn: {} mp: {}", r, s, mn, mp);

    let a_orig = BigUint::from(1u128) << sub_or_panic(prc(p), 1); // 2^(p-1)
    let halfway = a == a_orig;
    let cond2 = e != v || d == 'i';
    if halfway && cond2 {
        r = lsh_big(0, 1, &r);
        s = lsh_big(0, 1, &s);
        mp = lsh_big(0, 1, &mp);
    }

    let mut k = 0u128; // --0 = 0 (@s zero)
    let ten = BigUint::from(10u32);
    let nine = BigUint::from(9u32);
    let q = (&s + &nine) / &ten;
    loop {
        if r >= q {
            break;
        }
        k = dif_si(k, 2);
        r *= &ten;
        mn *= &ten;
        mp *= &ten;
    }
    loop {
        let two_r = &r * 2u32;
        let left = &two_r + &mp;
        let right = &s * 2u32;
        if left < right {
            break;
        }
        s *= &ten;
        k = sum_si(k, 2);
    }

    let mut o = BigUint::zero();
    let mut u = BigUint::zero();

    loop {
        let (u_big, rem) = dvr_big(&(&r * &ten), &s);

        k = dif_si(k, 2);

        u = (u_big.to_u64().expect("digit ≥10") as u32).into();

        r = rem;
        mn *= &ten;
        mp *= &ten;

        let l = &r * 2u32 < mn;

        let two_s = &s * 2u32;
        let h = two_s < mp || (&r * 2u32 > sub_or_panic_big(&two_s, &mp));

        if !l && !h {
            o = o * &ten + u;
            continue;
        }

        let q = h && (!l || &r * 2u32 > s);
        let digit = if q { u + BigUint::one() } else { u };
        o = o * &ten + digit;
        break;
    }
    eprintln!("drg returning {} {}", k, o);
    (k, o)
}

//  @rs to decimal float.
pub fn drg_fl(a: BinaryFloat, p: u128, w: u128, b: u128) -> DecimalFloat {
    match a {
        BinaryFloat::Finite { sign, exp, mant } => {
            if mant.is_zero() {
                DecimalFloat::Finite {
                    sign,
                    exp: 0,
                    mant: BigUint::zero(),
                }
            } else {
                let p = p + 1;
                let v = me(b, p);
                let w = bex(w) - 3;
                let d = 'd';
                let (k, digits) = drg(exp, mant, p, v, w, d);
                DecimalFloat::Finite {
                    sign,
                    exp: k,
                    mant: digits,
                }
            }
        }
        BinaryFloat::Infinity { sign } => DecimalFloat::Infinity { sign },
        BinaryFloat::NaN => DecimalFloat::NaN,
    }
}

// swr: swap rounding direction for negative numbers
pub fn swr(r: char) -> char {
    match r {
        'd' => 'u',
        'u' => 'd',
        _ => r,
    }
}

// fli: flip sign of BinaryFloat
pub fn fli(a: BinaryFloat) -> BinaryFloat {
    match a {
        BinaryFloat::Finite { sign, exp, mant } => BinaryFloat::Finite {
            sign: !sign,
            exp,
            mant,
        },
        BinaryFloat::Infinity { sign } => BinaryFloat::Infinity { sign: !sign },
        BinaryFloat::NaN => BinaryFloat::NaN,
    }
}

// zer: zero float node
pub fn zer() -> BinaryFloat {
    BinaryFloat::Finite {
        sign: false,
        exp: 0, // si-encoding of 0 is 0
        mant: BigUint::from(0u8),
    }
}

fn rau(e: u128, a: BigUint, t: bool, p: u128, v: u128, w: u128, r: char, d: char) -> BinaryFloat {
    let mode = match r {
        'z' | 'd' => LugMode::Floor,
        'a' | 'u' => LugMode::Ceiling,
        'n' => LugMode::Nearest,
        _ => LugMode::Nearest,
    };

    lug(mode, e, a, t, p, v, w, r, d)
}

pub fn cmp_si(a: u128, b: u128) -> u128 {
    if a == b {
        0
    } else if syn_si(a) {
        if syn_si(b) {
            if a > b {
                2
            } else {
                1
            }
        } else {
            2
        }
    } else if syn_si(b) {
        1
    } else {
        if a > b {
            1
        } else {
            2
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum LugMode {
    Floor,   // %fl
    Ceiling, // %ce
    Smaller, // %sm
    Larger,  // %lg
    Nearest, // %ne  (ties to even)
    NearestAway,
    NearestTowards,
}

fn sub_or_panic(mut a: u128, b: u128) -> u128 {
    a = a.checked_sub(b).expect("subtraction underflow");
    a
}

fn sub_or_panic_big(a: &BigUint, b: &BigUint) -> BigUint {
    if a < b {
        panic!("subtraction underflow");
    }
    a - b
}

fn prc(p: u128) -> u128 {
    assert!(p > 1, "precision should be >= 2");
    p
}

fn lug(
    mode: LugMode,
    mut e: u128,
    mut a: BigUint,
    s: bool,
    p: u128,
    v: u128,
    w: u128,
    r: char,
    d: char,
) -> BinaryFloat {
    use BinaryFloat::*;
    use LugMode::*;

    if a == BigUint::zero() {
        panic!("lug: mantissa zero");
    }

    let m = met(0, &ParsedAtom::Big(a.clone())) as u128;
    let prc_res = prc(p);
    assert!(
        s | (m > prc_res),
        "lug: stick bit is false or precision is invalid"
    );

    let max_p = if m > prc_res {
        sub_or_panic(m as u128, prc_res)
    } else {
        0
    };

    let max_q = {
        let abs_arg = if d == 'i' {
            0
        } else if cmp_si(e, v) == 1 {
            dif_si(v, e)
        } else {
            0
        };
        abs_si(abs_arg)
    };

    let q = max_p.max(max_q);

    let b = end_big(0, q as usize, &a)
        .to_u128()
        .expect("value too large for u128");

    a = rsh(0, q as usize, &ParsedAtom::Big(a)).to_biguint();

    e = sum_si(e, sun_si(q));

    if a == BigUint::zero() {
        assert!(d != 'i', "lug: d == %i");
        return match mode {
            Floor | Smaller => Finite {
                sign: true,
                exp: 0,
                mant: BigUint::zero(),
            },
            Ceiling | Larger => Finite {
                sign: true,
                exp: v,
                mant: BigUint::one(),
            },
            Nearest | NearestTowards => {
                let half = bex(q.saturating_sub(1));
                if s {
                    if b <= half {
                        return Finite {
                            sign: true,
                            exp: 0,
                            mant: BigUint::zero(),
                        };
                    }
                    return Finite {
                        sign: true,
                        exp: v,
                        mant: BigUint::one(),
                    };
                }
                if b < half {
                    return Finite {
                        sign: true,
                        exp: 0,
                        mant: BigUint::zero(),
                    };
                }
                return Finite {
                    sign: true,
                    exp: v,
                    mant: BigUint::one(),
                };
            }
            NearestAway => {
                let half = bex(q.saturating_sub(1));
                if b < half {
                    return Finite {
                        sign: true,
                        exp: 0,
                        mant: BigUint::zero(),
                    };
                }
                return Finite {
                    sign: true,
                    exp: v,
                    mant: BigUint::one(),
                };
            }
        };
    }

    (e, a) = xpd(e, a, d, p, v);

    match mode {
        Floor => { /* no change */ }
        Larger => a = a + BigUint::one(),
        Smaller => {
            if b == 0 && s {
                if e == v && d != 'i' {
                    a = sub_or_panic_big(&a, &BigUint::one());
                } else {
                    let y =
                        sub_or_panic_big(&(a.clone() * BigUint::from(2 as u128)), &BigUint::one());
                    if met_big(0, &y) as u128 <= prc_res {
                        a = y;
                        e = dif_si(e, 2);
                    } else {
                        a = sub_or_panic_big(&a, &BigUint::one());
                    }
                }
            }
        }
        Ceiling => {
            if !(b == 0 && !s) {
                a = a + BigUint::one();
            }
        }
        Nearest => {
            if b != 0 {
                let y = bex(sub_or_panic(q, 1));
                if b == y && s {
                    if dis_big(&a, &BigUint::one()) != BigUint::zero() {
                        a = a + BigUint::one();
                    }
                } else if b < y {
                } else {
                    a = a + BigUint::one();
                }
            }
        }
        NearestAway => {
            if b != 0 {
                let y = bex(sub_or_panic(q, 1));
                if !(b < y) {
                    a = a + BigUint::one();
                }
            }
        }
        NearestTowards => {
            if b != 0 {
                let y = bex(sub_or_panic(q, 1));
                if b == y {
                    if !s {
                        a = a + BigUint::one();
                    }
                }
                if !(b < y) {
                    a = a + BigUint::one();
                }
            }
        }
    };

    (e, a) = if (met_big(0, &a.clone()) as u128) != (prc_res + 1) {
        (e, a)
    } else {
        a = rsh(0, 1, &ParsedAtom::Big(a))
            .to_u128()
            .expect("lug: cast failled")
            .into();
        e = sum_si(e, 2);
        (e, a)
    };

    if a == BigUint::zero() {
        return Finite {
            sign: true,
            exp: 0,
            mant: BigUint::zero(),
        };
    }

    let res = if d == 'i' {
        Finite {
            sign: true,
            exp: e,
            mant: BigUint::from(a),
        }
    } else if cmp_si(emx(v, w), e) == 1 {
        Infinity { sign: true }
    } else {
        Finite {
            sign: true,
            exp: e,
            mant: BigUint::from(a),
        }
    };

    if !(d == 'f') {
        return res;
    }

    match res {
        Finite {
            sign,
            exp,
            ref mant,
        } => {
            if met_big(0, &mant.clone()) as u128 == prc(p) {
                return Finite {
                    sign: true,
                    exp: 0,
                    mant: BigUint::zero(),
                };
            }
            res
        }
        _ => res,
    }
}

fn emx(v: u128, w: u128) -> u128 {
    sum_si(v, sun_si(w))
}

fn rou(e: u128, a: BigUint, p: u128, v: u128, w: u128, r: char, d: char) -> BinaryFloat {
    rau(e, a, true, p, v, w, r, d)
}

pub fn binaryfloat_mul_internal(
    a_e: u128,
    a_a: BigUint,
    b_e: u128,
    b_a: BigUint,
    p: u128,
    v: u128,
    w: u128,
    r: char,
    d: char,
) -> BinaryFloat {
    let e = sum_si(a_e, b_e);
    let a = a_a * b_a;
    rou(e, a, p, v, w, r, d)
}

pub fn binaryfloat_div_internal(
    a_e: u128,
    a_a: BigUint,
    b_e: u128,
    b_a: BigUint,
    p: u128,
    v_min: u128,
    w: u128,
    r: char,
    d: char,
) -> BinaryFloat {
    let ma = met_big(0, &a_a) as u128;
    let mb = met_big(0, &b_a) as u128;

    let rhs = sun_si(mb + prc(p) + 1);
    let v = dif_si(sun_si(ma), rhs);

    let (a_e_shifted, a_a_shifted) = if syn_si(v) {
        (a_e, a_a)
    } else {
        let shift = abs_si(v) as usize;
        let new_e = sum_si(v, a_e);
        let new_a = lsh(0, shift, &ParsedAtom::Big(a_a.clone())).to_biguint();
        (new_e, new_a)
    };

    let j = dif_si(a_e_shifted, b_e);
    let (quot, rem) = dvr_big(&a_a_shifted, &b_a);

    rau(j, quot, rem.is_zero(), p, v_min, w, r, d)
}

fn dvr_big(a: &BigUint, b: &BigUint) -> (BigUint, BigUint) {
    let quot = a / b;
    let rem = a % b;
    (quot, rem)
}

pub fn bex(a: u128) -> u128 {
    if a == 0 {
        1
    } else {
        assert!(a < 128, "bex: exponent too large for u128");
        1u128 << a
    }
}

fn xpd(e: u128, a: BigUint, d: char, p: u128, v: u128) -> (u128, BigUint) {
    let ma = met_big(0, &a.clone()) as u128;

    if ma >= prc(p) {
        return (e, a);
    }

    let shift = if d == 'i' {
        sub_or_panic(prc(p), ma as u128)
    } else {
        let w = dif_si(e, v);
        let q = if syn_si(w) { abs_si(w) } else { 0 };
        let needed = sub_or_panic(prc(p), ma as u128);
        q.min(needed)
    };

    let e_new = dif_si(e, sun_si(shift));
    let a_new = lsh_big(0, shift as usize, &a);

    (e_new, a_new)
}

pub fn binaryfloat_mul(
    a: BinaryFloat,
    b: BinaryFloat,
    p: u128,
    v: u128,
    w: u128,
    mut r: char,
    d: char,
) -> BinaryFloat {
    use BinaryFloat::*;

    if matches!(a, NaN) || matches!(b, NaN) {
        return NaN;
    }

    if let Infinity { sign: sa } = a {
        if let Infinity { sign: sb } = b {
            return Infinity { sign: sa == sb };
        }

        let b_mant = if let Finite { ref mant, .. } = b {
            mant.clone()
        } else {
            BigUint::zero()
        };
        if b_mant == BigUint::zero() {
            return NaN;
        }
        return Infinity {
            sign: sa == b.sign(),
        };
    }

    if let Infinity { sign: sb } = b {
        let a_mant = if let Finite { ref mant, .. } = a {
            mant.clone()
        } else {
            BigUint::zero()
        };
        if a_mant == BigUint::zero() {
            return NaN;
        }
        return Infinity {
            sign: a.sign() == sb,
        };
    }

    let (sa, ea, ma) = if let Finite { sign, exp, mant } = a {
        (sign, exp, mant)
    } else {
        (false, 0, BigUint::zero())
    };
    let (sb, eb, mb) = if let Finite { sign, exp, mant } = b {
        (sign, exp, mant)
    } else {
        (false, 0, BigUint::zero())
    };

    if ma == BigUint::zero() || mb == BigUint::zero() {
        return Finite {
            sign: sa == sb, // =(s.a s.b)
            exp: 0,         // zer = [e=--0 a=0]
            mant: BigUint::zero(),
        };
    }

    if ma == BigUint::zero() || mb == BigUint::zero() {
        return binaryfloat_mul_internal(ea, ma, eb, mb, p, v, w, r, d);
    }
    r = swr(r);
    fli(binaryfloat_mul_internal(ea, ma, eb, mb, p, v, w, r, d))
}

pub fn binaryfloat_div(
    a: BinaryFloat,
    b: BinaryFloat,
    p: u128,
    v: u128,
    w: u128,
    mut r: char,
    d: char,
) -> BinaryFloat {
    use BinaryFloat::*;

    if matches!(a, NaN) || matches!(b, NaN) {
        return NaN;
    }

    if let Infinity { sign: sa } = a {
        if let Infinity { sign: sb } = b {
            return NaN;
        }
        return Infinity {
            sign: sa == b.sign(),
        };
    }

    if let Infinity { sign: sb } = b {
        return Finite {
            sign: a.sign() == sb,
            exp: 0, // zer = [e=--0 a=0]
            mant: BigUint::zero(),
        };
    }

    let (sa, ea, ma) = if let Finite { sign, exp, mant } = a {
        (sign, exp, mant)
    } else {
        (false, 0, BigUint::zero())
    };
    let (sb, eb, mb) = if let Finite { sign, exp, mant } = b {
        (sign, exp, mant)
    } else {
        (false, 0, BigUint::zero())
    };

    if ma == BigUint::zero() {
        if mb == BigUint::zero() {
            return NaN;
        }
        return Finite {
            sign: sa == sb,
            exp: 0,
            mant: BigUint::zero(),
        };
    }

    if mb == BigUint::zero() {
        return Infinity { sign: sa == sb };
    }

    if sa == sb {
        return binaryfloat_div_internal(ea, ma, eb, mb, p, v, w, r, d);
    }
    r = swr(r);
    fli(binaryfloat_div_internal(ea, ma, eb, mb, p, v, w, r, d))
}

pub fn pow(base: u128, exp: u128) -> BigUint {
    if exp == 0 {
        return BigUint::from(1u8);
    }

    let mut result = BigUint::from(1u8);
    let mut base = BigUint::from(base);
    let mut exp = exp;

    while exp > 0 {
        if exp & 1 == 1 {
            result *= &base;
        }
        base *= base.clone();
        exp >>= 1;
    }

    result
}

pub fn fil(a: u32, b: u32, c: u128) -> ParsedAtom {
    if b == 0 {
        return ParsedAtom::Small(0);
    }

    let bloq_bits = 1u32 << a; // 2^a bits per block
    let mask = if bloq_bits >= 128 {
        u128::MAX
    } else {
        (1u128 << bloq_bits) - 1
    };
    let c_masked = c & mask;

    if bloq_bits as u64 * b as u64 <= 128 && c_masked != 0 {
        let mut result = 0u128;
        for i in 0..b {
            let shift = (b - 1 - i) as u32 * bloq_bits;
            if shift >= 128 {
                break;
            }
            result |= c_masked << shift;
        }
        ParsedAtom::Small(result)
    } else {
        let c_big = BigUint::from(c_masked);
        let mut result = BigUint::from(0u8);
        for i in 0..b {
            let shift = (b - 1 - i) as usize * bloq_bits as usize;
            result |= &c_big << shift;
        }
        ParsedAtom::Big(result)
    }
}

pub fn bif(a: BinaryFloat, w: u128, p: u128, b: u128, r: char) -> ParsedAtom {
    match a {
        BinaryFloat::Infinity { sign } => {
            let fill_val = fil(0, w as u32, 1);
            let q = lsh(0, p as usize, &fill_val);
            if sign {
                q
            } else {
                let q_u128 = q.to_u128().expect("float bigger than 128 bits");
                ParsedAtom::Small(q_u128.wrapping_add(bex(w + p)))
            }
        }

        BinaryFloat::NaN => {
            let fill_val = fil(0, (w + 1) as u32, 1);
            let shift = sub_or_panic(p, 1) as usize;
            if shift >= 128 {
                panic!("bif: shift too large");
            }
            lsh(0, shift, &fill_val)
        }

        BinaryFloat::Finite {
            sign,
            exp: e,
            mant: a_a,
        } => {
            if a_a.is_zero() {
                return if sign {
                    ParsedAtom::Small(0)
                } else {
                    ParsedAtom::Small(bex(w + p))
                };
            }

            let ma = met_big(0, &a_a) as u128;

            if ma != p + 1 {
                assert!(
                    e == dif_si(dif_si(2, b), sun_si(p)),
                    "bif: subnormal exponent != me"
                );
                assert!(ma < p + 1, "bif: subnormal mantissa too large");

                let a_small = if a_a.bits() > 128 {
                    panic!("bif: mantissa too large for Small");
                } else {
                    a_a.to_u128().expect("mantissa bit-width was checked")
                };

                return if sign {
                    ParsedAtom::Small(a_small)
                } else {
                    ParsedAtom::Small(a_small.wrapping_add(bex(w + p)))
                };
            }

            let diff = dif_si(e, dif_si(dif_si(2, b), sun_si(p)));
            let q = sum_si(diff, 2);

            let abs_q = abs_si(q);
            let shifted = (abs_q as u128) << p;
            let a_small = if a_a.bits() > 128 {
                panic!("bif: mantissa too large");
            } else {
                a_a.to_u128().expect("mantissa bit-width was checked")
            };
            let low_p = a_small & ((1u128 << p) - 1);
            let r = shifted.wrapping_add(low_p);

            if sign {
                ParsedAtom::Small(r)
            } else {
                ParsedAtom::Small(r.wrapping_add(bex(w + p)))
            }
        }
    }
}

pub fn grd_fl(a: DecimalFloat, b: u128, p: u128, w: u128, mut r: char) -> BinaryFloat {
    //  +pa:ff arm will set these configs before calling +grd:fl
    let v = me(b, p);
    let p = p + 1;
    let w = bex(w) - 3;
    let d = 'd';

    match a {
        DecimalFloat::NaN => BinaryFloat::NaN,
        DecimalFloat::Infinity { sign } => BinaryFloat::Infinity { sign },
        DecimalFloat::Finite { sign, exp: e, mant } => {
            r = 'n';
            let q = abs_si(e);
            let pow5 = pow(5, q);

            let left = BinaryFloat::Finite {
                sign,
                exp: 0,
                mant: BigUint::from(mant),
            };
            if syn_si(e) {
                let right = BinaryFloat::Finite {
                    sign: true,
                    exp: e,
                    mant: pow5,
                };
                binaryfloat_mul(left, right, p, v, w, r, d)
            } else {
                let divisor = BinaryFloat::Finite {
                    sign: true,
                    exp: sun_si(q),
                    mant: pow5,
                };
                binaryfloat_div(left.clone(), divisor.clone(), p, v, w, r, d)
            }
        }
    }
}

//  finish parsing @rh
//  rylh -> grd:rh -> grd:ff -> grd:fl
pub fn rylh(a: DecimalFloat) -> ParsedAtom {
    let w = 5;
    let p = 10;
    let b = 30; // --15
    let r = 'z';
    let grd_res = grd_fl(a, b, p, w, r);
    bif(grd_res, w, p, b, r)
}

//  prep @rh for print
pub fn rlyh(a: u128) -> DecimalFloat {
    let w = 5;
    let p = 10;
    let b = 30; // --15
    let r = 'z';
    let sea_res = sea(w, p, b, &ParsedAtom::Small(a));
    drg_fl(sea_res, p, w, b)
}

//  finish parsing @rq
pub fn rylq(a: DecimalFloat) -> ParsedAtom {
    let w = 15;
    let p = 112;
    let b = 32766; // --16.383
    let r = 'z';
    let grd_res = grd_fl(a, b, p, w, r);
    bif(grd_res, w, p, b, r)
}

//  prep @rq for print
pub fn rlyq(a: u128) -> DecimalFloat {
    let w = 15;
    let p = 112;
    let b = 32766; // --16.383
    let r = 'z';
    let sea_res = sea(w, p, b, &ParsedAtom::Small(a));
    drg_fl(sea_res, p, w, b)
}

//  finish parsing @rd
pub fn ryld(a: DecimalFloat) -> ParsedAtom {
    let w = 11;
    let p = 52;
    let b = 2046; // --1.023
    let r = 'z';
    let grd_res = grd_fl(a, b, p, w, r);
    bif(grd_res, w, p, b, r)
}

//  prep @rd for print
pub fn rlyd(a: u128) -> DecimalFloat {
    let w = 11;
    let p = 52;
    let b = 2046; // --1.023
    let r = 'z';
    let sea_res = sea(w, p, b, &ParsedAtom::Small(a));
    drg_fl(sea_res, p, w, b)
}

//  finish parsing @rs
pub fn ryls(a: DecimalFloat) -> ParsedAtom {
    let w = 8;
    let p = 23;
    let b = 254; // --127
    let r = 'z';
    let grd_res = grd_fl(a, b, p, w, r);
    bif(grd_res, w, p, b, r)
}

// prep @rs for print
pub fn rlys(a: u128) -> DecimalFloat {
    let w = 8;
    let p = 23;
    let b = 254; // --127
    let r = 'z';
    let sea_res = sea(w, p, b, &ParsedAtom::Small(a));
    drg_fl(sea_res, p, w, b)
}

pub fn float<'src>() -> impl Parser<'src, &'src str, (String, ParsedAtom), Err<'src>> {
    let floats = just('-')
        .or_not()
        .then(decimal_without_leading_zero())
        .then(choice((
            just('.').ignore_then(digits()).map(|frac| {
                (
                    frac.len(),
                    frac.parse::<BigUint>().expect("float: invalid fraction"),
                )
            }),
            empty().to((0, BigUint::zero())),
        )))
        .then(choice((
            just('e')
                .ignore_then(just('-').or_not())
                .then(decimal_without_leading_zero())
                .map(|(maybe_hep, expo)| {
                    (
                        !maybe_hep.is_some(),
                        expo.parse::<u128>().expect("float: invalid exponent"),
                    )
                }),
            empty().to((true, 0)),
        )))
        .map(|(((maybe_hep, p), (len_mant, mant)), (sign_expo, expo))| {
            let term1 = new_si(sign_expo, expo);
            let term2 = sun_si(len_mant as u128);
            let h = dif_si(term1, term2);
            let po = BigUint::from(10u32).pow(
                len_mant
                    .try_into()
                    .expect("fraction length should fit BigUint exponent width"),
            );
            let integer_part = p.parse::<BigUint>().expect("float: invalid decimal");
            let a = integer_part * po + mant;
            DecimalFloat::Finite {
                sign: !maybe_hep.is_some(),
                exp: h,
                mant: a,
            }
        });

    let inf = just('-')
        .or_not() //  -inf or inf
        .then(just("inf"))
        .map(|(maybe_hep, inf)| DecimalFloat::Infinity {
            sign: !maybe_hep.is_some(),
        })
        .boxed();

    let nan = just("nan").to(DecimalFloat::NaN).boxed(); //  nan

    let royl_rn = choice((
        floats, ///  1.10 or 1e10
        inf, nan,
    ))
    .boxed();

    let rh = just("~~").ignore_then(royl_rn.clone());
    let rq = just("~~~").ignore_then(royl_rn.clone());
    let rd = just('~').ignore_then(royl_rn.clone());
    let rs = royl_rn;

    choice((
        rh.map(|dn| ("rh".to_string(), rylh(dn))),
        rq.map(|dn| ("rq".to_string(), rylq(dn))),
        rd.map(|dn| ("rd".to_string(), ryld(dn))),
        rs.map(|dn| ("rs".to_string(), ryls(dn))),
    ))
    .labelled("Float")
}

pub fn list_wing_hoon_wide<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Vec<(WingType, Hoon)>, Err<'src>> {
    let pair = winglist().then_ignore(just(' ')).then(hoon.clone());

    pair.separated_by(just(",").then(just(' ')))
        .at_least(1)
        .collect::<Vec<_>>()
}

pub fn list_hoon_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Vec<Hoon>, Err<'src>> {
    hoon_wide
        .clone()
        .separated_by(just(' '))
        .at_least(1)
        .collect::<Vec<Hoon>>()
}

pub fn list_spec_closed_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Vec<Spec>, Err<'src>> {
    spec_wide
        .clone()
        .separated_by(just(' '))
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(just('('), just(')'))
}

pub fn list_spec_closed_tall<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Vec<Spec>, Err<'src>> {
    gap()
        .ignore_then(
            spec.clone()
                .separated_by(gap())
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .then_ignore(gap())
        .then_ignore(just("=="))
}

pub fn list_wing_hoon_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Vec<(WingType, Hoon)>, Err<'src>> {
    let pair = winglist()
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap());

    pair.repeated()
        .at_least(1)
        .collect::<Vec<(WingType, Hoon)>>()
}

pub fn tiki_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Tiki, Err<'src>> {
    let with_name = symbol()
        .then_ignore(just('='))
        .then(
            winglist()
                .map(|w| {
                    Box::new(move |t: String| Tiki::Wing((Some(t), w)))
                        as Box<dyn FnOnce(String) -> Tiki>
                })
                .or(hoon_wide.clone().map(|h| {
                    Box::new(move |t: String| Tiki::Hoon((Some(t), Box::new(h))))
                        as Box<dyn FnOnce(String) -> Tiki>
                })),
        )
        .map(|(t, f)| f(t));

    let no_name = winglist()
        .map(|w| Tiki::Wing((None, w)))
        .or(hoon_wide.clone().map(|h| Tiki::Hoon((None, Box::new(h)))));

    with_name.or(no_name)
}

pub fn tiki_tall<'src>(
    hoon_tall: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Tiki, Err<'src>> {
    let with_name = symbol()
        .then_ignore(just('='))
        .then(
            winglist()
                .map(|w| {
                    Box::new(move |t: String| Tiki::Wing((Some(t), w)))
                        as Box<dyn FnOnce(String) -> Tiki>
                })
                .or(hoon_tall.clone().map(|h| {
                    Box::new(move |t: String| Tiki::Hoon((Some(t), Box::new(h))))
                        as Box<dyn FnOnce(String) -> Tiki>
                })),
        )
        .map(|(t, f)| f(t));

    tiki_wide(hoon_wide.clone()) //  the hoon parser has ^= case here but
        .or(just("^=").then(gap()).or_not().ignore_then(with_name))
        .or(hoon_tall.clone().map(|h| Tiki::Hoon((None, Box::new(h)))))
}

///  Parses arms of a Core (grouped by chapters).
///     chapters can be unamed or named with +$
///     arms can be named with ++ or +$
///
pub fn chapters<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, HashMap<String, Tome>, Err<'src>> {
    let luslus_linemap = linemap.clone();
    let luslus = just("++")
        .ignore_then(gap())
        .ignore_then(
            just('$')
                .to("$".to_string())
                .or(symbol())
                .map_with(|name: String, e| (name, e.span().start(), e.span().end())),
        )
        .then_ignore(gap())
        .then(hoon.clone())
        .map(move |((name, start, end), hoon)| {
            let hoon =
                if let Some(help) = luslus_linemap.arm_postfix_help("funk", &name, start, end) {
                    Hoon::Note(Note::Help(help), Box::new(hoon))
                } else {
                    hoon
                };
            (name, hoon)
        })
        .labelled("Arm ++");

    let lusbuc_linemap = linemap.clone();
    let lusbuc = just("+$")
        .ignore_then(gap())
        .ignore_then(symbol().map_with(|name: String, e| (name, e.span().start(), e.span().end())))
        .then_ignore(gap())
        .then(
            spec.clone()
                .map_with(|spec: Spec, e| (spec, e.span().start(), e.span().end())),
        )
        .map(move |((name, start, end), (spec, spec_start, _spec_end))| {
            let spec = if let Some(help) = lusbuc_linemap.help_before_spec(spec_start) {
                Spec::Gist(help, Box::new(spec))
            } else {
                spec
            };
            let hoon = Hoon::KetCol(Box::new(Spec::Name(name.clone(), Box::new(spec))));
            let hoon =
                if let Some(help) = lusbuc_linemap.arm_postfix_help("plan", &name, start, end) {
                    Hoon::Note(Note::Help(help), Box::new(hoon))
                } else {
                    hoon
                };
            (name, hoon)
        })
        .labelled("Arm +$");

    let label_linemap = linemap.clone();
    let optional_chapter_label = just("+|")
        .then_ignore(gap())
        .ignore_then(just("%").ignore_then(symbol()))
        .map_with(move |label: String, e| {
            let what = label_linemap.help_before_chapter_label(e.span().start());
            (label, what)
        })
        .then_ignore(gap())
        .or_not()
        .labelled("Chapter Label +|");

    let chapter = optional_chapter_label.then(
        luslus
            .or(lusbuc)
            .then_ignore(gap())
            .repeated()
            .at_least(1)
            .collect::<Vec<_>>(),
    );

    chapter
        .repeated()
        .at_least(1)
        .collect::<Vec<_>>()
        .then_ignore(just("--"))
        .map(
            |chapters_vec: Vec<(Option<(String, Option<NounExpr>)>, Vec<(String, Hoon)>)>| {
                let mut map_term_tome = HashMap::new();
                for (opt_label, arms_vec) in chapters_vec {
                    let (key, what) = opt_label.unwrap_or_else(|| ("$".to_string(), None));
                    // hoon.hoon repeats chapter labels (`+| %containers`, etc.) across layers.
                    // Treat these as append-to-existing rather than overwriting the previous chapter.
                    let tome = map_term_tome
                        .entry(key)
                        .or_insert_with(|| (what.clone(), HashMap::new()));
                    if tome.0.is_none() {
                        tome.0 = what;
                    }
                    for (name, hoon) in arms_vec {
                        // If an arm is redefined within a later chunk of the same chapter, keep the
                        // last definition (matches typical "last wins" parse behavior).
                        tome.1.insert(name, hoon);
                    }
                }
                map_term_tome
            },
        )
}

pub fn list_hoon_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Vec<Hoon>, Err<'src>> {
    hoon.clone()
        .separated_by(gap())
        .at_least(1)
        .collect::<Vec<_>>()
}

pub fn term<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    just("%").ignore_then(symbol())
}

pub fn jet_hooks<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Vec<(String, Hoon)>, Err<'src>> {
    just('~').to(Vec::new()).or(just("==")
        .ignore_then(gap())
        .ignore_then(
            just("%")
                .ignore_then(symbol())
                .then_ignore(gap())
                .then(hoon.clone())
                .separated_by(gap())
                .at_least(1)
                .collect::<Vec<(String, Hoon)>>(),
        )
        .then_ignore(gap())
        .then_ignore(just("==")))
}

pub fn jet_signature<'src>() -> impl Parser<'src, &'src str, Chum, Err<'src>> {
    let lef = symbol().map(Chum::Lef); //  %k

    let stdkel = symbol() //  %k.138
        .then_ignore(just('.'))
        .then(decimal_number())
        .map(|(s, n)| Chum::StdKel(s, decimal_to_atom(n)));

    let venprokel = symbol() //  %k:foo.138
        .then_ignore(just(':'))
        .then(symbol())
        .then_ignore(just('.'))
        .then(decimal_number())
        .map(|((s1, s2), n)| Chum::VenProKel(s1, s2, decimal_to_atom(n)));

    let venproverkel =  //  %k:foo:bar..138
                symbol()
                .then_ignore(just(':'))
                .then(symbol())
                .then_ignore(just(".."))
                .then(decimal_number())
                .map(|((s1, s2), n)| Chum::VenProKel(s1, s2, decimal_to_atom(n)));

    just("%")
        .ignore_then(choice((venproverkel, venprokel, stdkel, lef)))
        .labelled("Jet Signature")
}

//  +lute
//
pub fn noun_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon.separated_by(gap())
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(just('[').ignore_then(gap()), gap().ignore_then(just(']')))
        .map(|h| Hoon::ColTar(h))
}

pub fn newline<'src>() -> impl Parser<'src, &'src str, (), Err<'src>> {
    just('\n').labelled("Newline").ignored()
}

pub fn soil<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Vec<Woof>, Err<'src>> {
    let sump = hoon_wide
        .separated_by(just(' '))
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(just('{'), just('}'))
        .map(|h| Woof::Hoon(Hoon::ColTar(h)))
        .boxed();

    // non-control 32-256, excluding DEL, {,  ", \
    let wide_char = any().filter(|c: &char| {
        let x = *c as u32;
        (x >= 0x20 && x <= 0x7E && *c != '{' && *c != '"' && *c != '\\') || (x >= 0x80 && x <= 0xFF)
    });

    //
    //  "foo"
    //
    let wide_tape = choice((
        //
        //  escaped \, ", {, hex
        //
        just("\\")
            .ignore_then(choice((
                just("\\").to('\\'),
                just("\"").to('\"'),
                just("{").to('{'),
                // \HH hex escape
                any()
                    .filter(|c: &char| c.is_ascii_hexdigit())
                    .then(any().filter(|c: &char| c.is_ascii_hexdigit()))
                    .map(|(a, b)| {
                        let hx = format!("{}{}", a, b);
                        let byte =
                            u8::from_str_radix(&hx, 16).expect("hex tape escape was validated");
                        byte as char
                    }),
            )))
            .map(|c: char| Woof::ParsedAtom(ParsedAtom::Small(c as u128))),
        //
        //  {hoon}
        //
        sump.clone(),
        ///
        wide_char.map(|c| Woof::ParsedAtom(ParsedAtom::Small(c as u128))),
    ))
    .repeated()
    .collect::<Vec<Woof>>()
    .delimited_by(just("\""), just("\""))
    .labelled("Tape");

    // non-control 32-256, excluding DEL, {,  \
    let tall_char = any().filter(|c: &char| {
        let x = *c as u32;
        (x >= 0x20 && x <= 0x7E && *c != '{' && *c != '\\') || (x >= 0x80 && x <= 0xFF)
    });

    // let tall_tape_line_break =
    //             newline()
    //             .ignore_then(just("\"\"\"").not())
    //             .to(Woof::ParsedAtom(ParsedAtom::Small('\n' as u128)));

    let tall_tape_line_content = choice((
        //
        //  escaped \, {, hex
        //
        just("\\")
            .ignore_then(choice((
                just("\\").to('\\'),
                just("{").to('{'),
                // \HH hex escape
                any()
                    .filter(|c: &char| c.is_ascii_hexdigit())
                    .then(any().filter(|c: &char| c.is_ascii_hexdigit()))
                    .map(|(a, b)| {
                        let hx = format!("{}{}", a, b);
                        let byte =
                            u8::from_str_radix(&hx, 16).expect("hex tape escape was validated");
                        byte as char
                    }),
            )))
            .map(|c: char| Woof::ParsedAtom(ParsedAtom::Small(c as u128))),
        //
        tall_char.map(|c| Woof::ParsedAtom(ParsedAtom::Small(c as u128))),
        //
        //  {hoon}
        //
        sump,
    ))
    .repeated()
    .collect::<Vec<Woof>>();

    let prefix_spaces = just(' ').repeated();

    let tall_tape_open = just("\"\"\"").map_with(move |_, extra| {
        let span: SimpleSpan = extra.span(); // get identation
        let (_line, col) = linemap.line_col(span.start);
        if col != 0 {
            return (col - 1) as usize;
        }
        return 0 as usize;
    });

    let tall_tape_close = newline()
        .ignore_then(just(' ').repeated().count())
        .then_ignore(just("\"\"\""))
        .boxed();

    let tall_tape_line = tall_tape_close.clone().not().ignore_then(
        newline()
            .ignore_then(just(' ').repeated().count())
            .then(tall_tape_line_content),
    );

    //  """
    //  foo
    //  """
    let tall_tape = prefix_spaces
        .ignore_then(tall_tape_open)
        .then(tall_tape_line.repeated().collect::<Vec<_>>())
        .then(tall_tape_close)
        .validate(|((absolute_indent, lines), close_indent), extra, emit| {
            let span = extra.span();

            if close_indent != absolute_indent {
                emit.emit(Rich::custom(span, "closing delimiter indentation mismatch"));
                return Vec::new();
            }

            let mut out: Vec<Woof> = vec![];
            for (mut indent, mut line) in lines {
                if indent > absolute_indent {
                    let extra = indent - absolute_indent;
                    indent = absolute_indent;
                    //  extra whitespaces belongs longs to line not indentation
                    let space = Woof::ParsedAtom(ParsedAtom::Small(' ' as u128));
                    line.splice(0..0, std::iter::repeat(space).take(extra));
                }

                //  if line is just a linebreak allow it
                if indent != absolute_indent && !(line.is_empty() && (indent == 0 as usize)) {
                    emit.emit(Rich::custom(span, "inconsistent indentation in tall tape"));
                    return Vec::new();
                }
                out.push(Woof::ParsedAtom(ParsedAtom::Small('\n' as u128)));
                if !line.is_empty() {
                    out.extend(line);
                }
            }
            // first linebreak after """ should not be in the tape
            out.remove(0);
            out
        })
        .labelled("Tape");

    choice((tall_tape, wide_tape))
}

pub fn tape<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    soil(hoon_wide.clone(), linemap.clone())
        .separated_by(just('.').ignore_then(gap().or_not()))
        .at_least(1)
        .collect::<Vec<_>>()
        .map(|s: Vec<Vec<Woof>>| {
            let wof: Vec<Woof> = s.into_iter().flatten().collect();
            Hoon::Knit(wof)
        })
        .labelled("Tape")
}

pub fn aura_text<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    just('@')
        .ignore_then(
            any()
                .filter(|c: &char| c.is_ascii_lowercase())
                .repeated()
                .collect::<Vec<char>>()
                .then(
                    any()
                        .filter(|c: &char| c.is_ascii_uppercase())
                        .repeated()
                        .collect::<Vec<char>>(),
                )
                .map(|(lowers, uppers)| {
                    let mut s = String::new();
                    s.extend(lowers);
                    s.extend(uppers);
                    s
                }),
        )
        .labelled("Aura<@foo>")
}

pub fn aura_hoon<'src>() -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    aura_text()
        .map(|s| Hoon::Base(BaseType::Atom(s)))
        .labelled("Aura")
}

pub fn aura_spec<'src>() -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    aura_text()
        .map(|s| Spec::Base(BaseType::Atom(s)))
        .labelled("Aura")
}

pub fn loop_spec<'src>() -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    just('/')
        .ignore_then(choice((just('$').to("$".to_string()), symbol())))
        .map(|s| Spec::Loop(s))
}

pub fn concatanate<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just('^'))
        .then(hoon_wide.clone())
        .map(|(p, q)| Hoon::Pair(Box::new(p), Box::new(q)))
}

pub fn wing<'src>() -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    winglist()
        .map(|list: WingType| match list.first() {
            Some(Limb::Axis(0)) | Some(Limb::Term(_)) | Some(Limb::Parent(_, _)) => {
                Hoon::Wing(list)
            }
            _ => Hoon::CenTis(list, vec![]),
        })
        .labelled("Wing")
}

pub fn tell<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    just("<")
        .ignore_then(list_hoon_wide(hoon_wide.clone()))
        .then_ignore(just(">"))
        .map(|list| Hoon::Tell(list))
}

pub fn yell_parser<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    just(">")
        .ignore_then(list_hoon_wide(hoon_wide.clone()))
        .then_ignore(just("<"))
        .map(|list| Hoon::Yell(list))
}

pub fn constant<'src>(linemap: Arc<LineMap>) -> impl Parser<'src, &'src str, Coin, Err<'src>> {
    let buc =      // %$
        just('$')
        .to(Coin::Dime("tas".to_string(), ParsedAtom::Small(0)));

    let cord =      // %'foo'
        cord(linemap)
        .map(|s| Coin::Dime("t".to_string(), s));

    let coin =      // %123, %~m5, etc.
        nuck();

    let no = just('|').to(Coin::Dime("f".to_string(), ParsedAtom::Small(1)));

    let yes = just('&').to(Coin::Dime("f".to_string(), ParsedAtom::Small(0)));

    just('%')
        .ignore_then(choice((buc, yes, no, cord, coin)))
        .labelled("Constant<%foo>")
}

pub fn cord<'src>(linemap: Arc<LineMap>) -> impl Parser<'src, &'src str, ParsedAtom, Err<'src>> {
    let empty_triple_quoted = just("'''")
        .then_ignore(newline())
        .then_ignore(just("'''"))
        .to(cord_chars_to_atom(Vec::new()));

    //  \\, \' and \AA were A is a hex digit
    let escape = just('\\').ignore_then(choice((
        just('\\').to('\\'),
        just('\'').to('\''),
        // \HH hex escape
        any()
            .filter(|c: &char| c.is_ascii_hexdigit())
            .then(any().filter(|c: &char| c.is_ascii_hexdigit()))
            .map(|(a, b)| {
                let hx = format!("{}{}", a, b);
                let byte = u8::from_str_radix(&hx, 16).expect("hex cord escape was validated");
                byte as char
            }),
    )));

    //  non-control chars excluding DEL, single quote, and backslash
    let raw_char = any().filter(|c: &char| {
        let x = *c as u32;
        x >= 0x20
            && x != 0x7F // DEL
            && x != 0x27 // '
            && x != 0x5C // '\'
    });

    let gon = just("\\") // multiline separator
        .ignore_then(gap())
        .ignore_then(just("/"))
        .ignored()
        .labelled("Cord Multiline Separator");

    let char_in_singled_quoted = choice((escape, raw_char)).labelled("Cord Character");

    let single_quoted = char_in_singled_quoted
        .then_ignore(gon.or_not())
        .repeated()
        .collect::<Vec<char>>()
        .delimited_by(just("'"), just("'"))
        .map(cord_chars_to_atom);

    let prefix_spaces = just(' ').repeated();

    let triple_quoted_open = just("'''")
        .map_with(move |_, extra| {
            let span: SimpleSpan = extra.span(); // get identation
            let (_line, col) = linemap.line_col(span.start);
            if col != 0 {
                return (col - 1) as usize;
            }
            return 0 as usize;
        })
        .then_ignore(vul().or(newline()));

    let triple_quoted_close = newline()
        .ignore_then(just(' ').repeated().count())
        .then_ignore(just("'''"))
        .boxed();

    let triple_quoted_content = non_control_char().repeated().collect::<Vec<char>>().boxed();

    let triple_quoted_first_line = triple_quoted_close
        .clone()
        .not()
        .ignore_then(just(' ').repeated().count())
        .then(triple_quoted_content.clone());

    let triple_quoted_line = triple_quoted_close.clone().not().ignore_then(
        newline()
            .ignore_then(just(' ').repeated().count())
            .then(triple_quoted_content),
    );

    let triple_quoted = prefix_spaces
        .ignore_then(triple_quoted_open)
        .then(
            triple_quoted_first_line
                .then(triple_quoted_line.repeated().collect::<Vec<_>>())
                .map(|(first, mut rest)| {
                    rest.insert(0, first);
                    rest
                })
                .or_not()
                .map(|lines| lines.unwrap_or_default()),
        )
        .then(triple_quoted_close)
        .validate(|((absolute_indent, rest), close_indent), extra, emit| {
            let span = extra.span();

            if close_indent != absolute_indent {
                emit.emit(Rich::custom(span, "closing delimiter indentation mismatch"));
                return Vec::new();
            }

            if rest.is_empty() {
                return Vec::new();
            }

            let mut out: Vec<char> = vec![];
            for (mut indent, mut line) in rest {
                if indent > absolute_indent {
                    let extra = indent - absolute_indent;
                    indent = absolute_indent;
                    //  extra whitespaces belongs longs to line not indentation
                    line.splice(0..0, std::iter::repeat(' ').take(extra));
                }

                //  if line is just a linebreak allow it
                if indent != absolute_indent && !(line.is_empty() && (indent == 0 as usize)) {
                    emit.emit(Rich::custom(
                        span, "inconsistent indentation in multiline cord",
                    ));
                    return Vec::new();
                }
                out.push('\n');
                if !line.is_empty() {
                    out.extend(line);
                }
            }
            out.remove(0);
            out
        })
        .map(cord_chars_to_atom);

    choice((empty_triple_quoted, triple_quoted, single_quoted)).labelled("Cord")
}

pub fn increment<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    just('.')
        .or_not()
        .ignore_then(just("+"))
        .ignore_then(just('('))
        .ignore_then(hoon_wide.clone())
        .then_ignore(just(')'))
        .map(|h| Hoon::DotLus(Box::new(h)))
        .labelled("Increment: +(p)")
}

pub fn function_call<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    just('(')
        .ignore_then(hoon.clone())
        .then(
            just(' ')
                .ignore_then(hoon.clone())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .then_ignore(just(')'))
        .map(|(func, args)| Hoon::CenCol(Box::new(func), args))
        .labelled("Function Call")
}

const YEAR_OFFSET: u64 = 292_277_024_400;

fn yelp(yer: u64) -> bool {
    (yer % 4 == 0) && ((yer % 100 != 0) || (yer % 400 == 0))
}

// Constants from ++yo
const CETY: u64 = 36_524; // days in 100 years (non-leap century)
const DAY: u64 = 86_400; // seconds/day
const ERA: u64 = 146_097; // days in 400 years
const HOR: u64 = 3_600; // seconds/hour
const MIT: u64 = 60; // seconds/minute
const MOH: [u64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]; // normal
const MOY: [u64; 12] = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]; // leap

// ++yawn: days since "Jesus" (proleptic Gregorian)
fn yawn(mut yer: u64, mut mot: u64, mut day: u64) -> u64 {
    // => .(mot (dec mot), day (dec day))
    mot = mot.saturating_sub(1);
    day = day.saturating_sub(1);

    let cah = if yelp(yer) { &MOY } else { &MOH };
    for i in 0..mot as usize {
        day += cah[i];
    }

    loop {
        if yer % 4 != 0 {
            if yer == 0 {
                break;
            }
            yer -= 1;
            day += if yelp(yer) { 366 } else { 365 };
            continue;
        }
        if yer % 100 != 0 {
            if yer < 4 {
                break;
            }
            yer -= 4;
            day += if yelp(yer) { 1_461 } else { 1_460 };
            continue;
        }
        if yer % 400 != 0 {
            if yer < 100 {
                break;
            }
            yer -= 100;
            day += if yelp(yer) { 36_525 } else { 36_524 };
            continue;
        }
        // divisible by 400
        day += (yer / 400) * (1 + 4 * CETY); // 1 + 4*36524 = 146097 = ERA
        break;
    }
    day
}

pub fn apply_sign(a: bool, b: ParsedAtom) -> ParsedAtom {
    match b {
        ParsedAtom::Small(n) => {
            let out = if a {
                2 * n
            } else if n == 0 {
                0
            } else {
                2 * (n - 1) + 1
            };
            ParsedAtom::Small(out)
        }
        ParsedAtom::Big(n) => {
            let out = if a {
                &n << 1
            } else if n.is_zero() {
                num_bigint::BigUint::from(0u32)
            } else {
                ((&n - 1u32) << 1) + 1u32
            };
            ParsedAtom::Big(out)
        }
    }
}

///  Alphanumeric with hyphens
///      Start with a lowercase letter
///      Followed by zero or more: lowercase letter, digit, or hyphen
///
pub fn symbol<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    any()
        .filter(|c: &char| c.is_ascii_lowercase())
        .then(
            any()
                .filter(|c: &char| matches!(c, 'a'..='z' | '0'..='9' | '-'))
                .repeated()
                .collect::<Vec<char>>(),
        )
        .map(|(first, rest)| {
            let mut s = String::with_capacity(rest.len() + 1);
            s.push(first);
            s.extend(rest);
            s
        })
        .labelled("Term")
}

const BTC_BASE58: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

fn build_yek() -> [u8; 256] {
    let mut yek = [0xFFu8; 256];
    for (i, ch) in BTC_BASE58.chars().enumerate() {
        let idx = ch as u8 as usize;
        if idx < 256 {
            yek[idx] = i as u8;
        }
    }
    yek
}

fn cha_fa(yek: &[u8; 256], ch: char) -> Option<u8> {
    let idx = ch as u32;
    if idx > 255 {
        return None;
    }
    let val = yek[idx as usize];
    if val == 0xFF {
        None
    } else {
        Some(val)
    }
}

fn bass_58(digits: &[u8]) -> BigUint {
    digits
        .iter()
        .fold(BigUint::from(0u32), |acc, &d| &acc * 58u32 + d as u32)
}

fn tok(a: &ParsedAtom) -> ParsedAtom {
    let b = pad_fa(&a);

    let swapped = swp(3, a);

    let padded = lsh(3, b, &swapped);

    let len = b + met(3, a);

    let hashed = shay(len as u64, &padded.to_biguint());

    let double_hashed = &ParsedAtom::Big(shay(32, &hashed));
    let truncated = end(3, 4, double_hashed);

    let n = net(5, &truncated);
    n
}

pub fn shay(len: u64, ruz: &BigUint) -> BigUint {
    let len = len as usize;

    let ruz_bytes = ruz.to_bytes_le();
    let msg_len = ruz_bytes.len();

    let mut msg = vec![0u8; len];

    if len == 0 {
    } else if msg_len >= len {
        msg.copy_from_slice(&ruz_bytes[..len]);
    } else {
        msg[..msg_len].copy_from_slice(&ruz_bytes);
    }

    let mut hasher = Sha256::new();
    hasher.update(&msg);
    let digest = hasher.finalize();

    BigUint::from_bytes_le(&digest)
}

fn swp(bloq: usize, b: &ParsedAtom) -> ParsedAtom {
    let blocks = rip(bloq, b);
    let rev = flop(&blocks);
    rep(bloq, None, &rev)
}

fn rip(bloq: usize, b: &ParsedAtom) -> Vec<ParsedAtom> {
    if b.is_zero() {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut cur = b.clone();

    while !cur.is_zero() {
        out.push(end(bloq, 1, &cur));
        cur = rsh(bloq, 1, &cur);
    }

    out
}

pub fn den_fa(a: &ParsedAtom) -> Option<ParsedAtom> {
    let b = rsh(3, 4, a);

    if tok(&b) == end(3, 4, a) {
        Some(b)
    } else {
        None
    }
}

fn sit(a: usize, b: &ParsedAtom) -> ParsedAtom {
    end(a, 1, b)
}

//  flip byte endianness
fn net(a: usize, b: &ParsedAtom) -> ParsedAtom {
    let b = sit(a, b);

    if a <= 3 {
        return b;
    }

    let c: usize = a - 1;

    let hi_bit = cut(c, 0, 1, &b);
    let hi = net(c, &hi_bit);

    let lo_bit = cut(c, 1, 1, &b);
    let lo = net(c, &lo_bit);

    let res = con_atoms(lsh(c, 1, &hi), lo);
    res
}

fn met_big(bloq: u32, atom: &BigUint) -> u32 {
    let bits = 1u32 << bloq; // bloq_bits
    if atom.is_zero() {
        return 1;
    }
    let atom_bits = atom.bits() as u32;
    (atom_bits + bits - 1) / bits
}

/// pad(a): number of zero bytes needed to pad `a` to 21 bytes
fn pad_fa_big(a: &BigUint) -> usize {
    let b = met(3, &ParsedAtom::Big(a.clone()));
    if b >= 21 {
        0
    } else {
        21 - b as usize
    }
}

pub fn pad_fa(atom: &ParsedAtom) -> usize {
    21usize.saturating_sub(met(3, atom))
}

pub fn enc_fa(atom: &ParsedAtom) -> ParsedAtom {
    let a = atom;

    let shifted = lsh(3, 4, a).to_biguint();
    let checksum = tok(atom).to_biguint();

    ParsedAtom::from_biguint(shifted ^ checksum)
}

pub fn bitcoin_address<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    just("0c")
        .ignore_then(alphanumeric())
        .labelled("Bitcoin Address")
}

pub fn urs<'src>() -> impl Parser<'src, &'src str, ParsedAtom, Err<'src>> {
    any()
        .filter(|c: &char| matches!(c, '0'..='9' | 'a'..='z' | '.' | '_' | '~' | '-'))
        .repeated()
        .collect::<String>()
        .map(string_to_atom)
}

pub fn urt<'src>() -> impl Parser<'src, &'src str, &'src str, Err<'src>> {
    any()
        .filter(|c: &char| matches!(c, '0'..='9' | 'a'..='z' | '.' | '~' | '-'))
        .repeated()
        .at_least(1)
        .to_slice()
}

fn wick(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '~' {
            match chars.next() {
                Some('~') => out.push('~'),    // ~~ -> ~
                Some('-') => out.push('_'),    // ~- -> _
                Some(_) | None => return None, // invalid escape
            }
        } else {
            // Only allow valid @ta characters: [a-z0-9._-]
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-' {
                out.push(c);
            } else {
                return None; // invalid char in atom
            }
        }
    }

    Some(out)
}

pub fn urx<'src>() -> impl Parser<'src, &'src str, ParsedAtom, Err<'src>> {
    let hex_escape = any()
        .filter(|c: &char| c.is_ascii_hexdigit())
        .repeated()
        .at_least(1)
        .collect::<String>()
        .delimited_by(just('~'), just('.'))
        .map(|hex_str: String| {
            let big = BigUint::from_str_radix(&hex_str, 16).unwrap_or_default();
            let value_32 = big.iter_u32_digits().next().unwrap_or(0); // low 32 bits

            let tuft_result = tuft(&ParsedAtom::Small(value_32 as u128));

            match tuft_result {
                ParsedAtom::Small(n) => n,
                ParsedAtom::Big(_) => panic!("tuft overflow"),
            }
        });

    let special = choice((
        just("~~").to(b'~' as u128),
        just("~.").to(b'.' as u128),
        just('.').to(b' ' as u128),
    ));

    let ascii = any()
        .filter(|c: &char| c.is_ascii_digit() || c.is_ascii_lowercase() || *c == '-' || *c == '_')
        .map(|c| c as u128);

    let token = choice((hex_escape, special, ascii));

    token
        .repeated()
        .at_least(1)
        .collect::<Vec<u128>>()
        .map(|chars: Vec<u128>| rap(3, &chars))
}

fn atom_shl(a: &ParsedAtom, bits: usize) -> ParsedAtom {
    if bits == 0 {
        return a.clone();
    }
    match a {
        ParsedAtom::Small(n) => {
            if bits >= 128 {
                ParsedAtom::from_biguint(BigUint::from(*n) << bits)
            } else {
                ParsedAtom::Small(n << bits)
            }
        }
        ParsedAtom::Big(b) => ParsedAtom::from_biguint(b << bits),
    }
}

fn atom_shr(atom: &ParsedAtom, bits: usize) -> ParsedAtom {
    if bits == 0 {
        return atom.clone();
    }
    match atom {
        ParsedAtom::Small(n) => {
            if bits >= 128 {
                ParsedAtom::Small(0)
            } else {
                ParsedAtom::Small(n >> bits)
            }
        }
        ParsedAtom::Big(b) => ParsedAtom::from_biguint(b >> bits),
    }
}

fn atom_mask_low_bits(atom: &ParsedAtom, bits: usize) -> ParsedAtom {
    if bits == 0 {
        return ParsedAtom::Small(0);
    }
    match atom {
        ParsedAtom::Small(n) => {
            if bits >= 128 {
                ParsedAtom::Small(*n)
            } else {
                let mask = (1u128 << bits) - 1;
                ParsedAtom::Small(*n & mask)
            }
        }
        ParsedAtom::Big(b) => {
            if bits <= 128 {
                let mask: u128 = (1u128 << bits) - 1;
                let mut limbs = b.iter_u64_digits();
                let lo = limbs.next().unwrap_or(0);
                let hi = limbs.skip(1).next().unwrap_or(0);
                let low_u128 = ((hi as u128) << 64) | (lo as u128);
                ParsedAtom::Small(low_u128 & mask)
            } else {
                let mask = (BigUint::one() << bits) - BigUint::one();
                ParsedAtom::from_biguint(b & &mask)
            }
        }
    }
}

// tuft: ParsedAtom (codepoint) -> ParsedAtom (UTF-8 bytes, @t)
pub fn tuft(atom: &ParsedAtom) -> ParsedAtom {
    // This builds a little-endian byte list, then rap 3 packs it
    let mut bytes: Vec<u8> = Vec::new();
    let mut a = atom.clone();

    loop {
        // ?: =(`@`0 a)
        if a.is_zero() {
            break;
        }

        // b=(end 5 a)
        let b_atom = end(5, 1, &a);
        let b = b_atom.to_u128().expect("byte chunk should fit in u128");

        // c=$(a (rsh 5 a))
        a = rsh(5, 1, &a);

        if b <= 0x7f {
            bytes.push(b as u8);
            continue;
        }

        if b <= 0x7ff {
            bytes.push((0b1100_0000 | cut_u(b, 6, 5)) as u8);
            bytes.push((0b1000_0000 | (b & 0x3f)) as u8);
            continue;
        }

        if b <= 0xffff {
            bytes.push((0b1110_0000 | cut_u(b, 12, 4)) as u8);
            bytes.push((0b1000_0000 | cut_u(b, 6, 6)) as u8);
            bytes.push((0b1000_0000 | (b & 0x3f)) as u8);
            continue;
        }

        bytes.push((0b1111_0000 | cut_u(b, 18, 3)) as u8);
        bytes.push((0b1000_0000 | cut_u(b, 12, 6)) as u8);
        bytes.push((0b1000_0000 | cut_u(b, 6, 6)) as u8);
        bytes.push((0b1000_0000 | (b & 0x3f)) as u8);
    }

    // rap 3: pack bytes little-endian into @t
    let mut acc: u128 = 0;
    for (i, byte) in bytes.iter().enumerate() {
        acc |= (*byte as u128) << (i * 8);
    }

    ParsedAtom::Small(acc)
}
// --- Extract low byte as u8 ---
fn atom_to_u8(atom: &ParsedAtom) -> u8 {
    match end(3, 1, atom) {
        ParsedAtom::Small(n) => n as u8,
        ParsedAtom::Big(_) => 0,
    }
}

// --- UTF-8 continuation byte check ---
fn is_continuation(b: u8) -> bool {
    b & 0xC0 == 0x80
}

// --- teff: UTF-8 leading byte → length (1–4) ---
fn teff(atom: &ParsedAtom) -> usize {
    let b = atom_to_u8(atom);
    if b == 0 {
        return 0;
    }
    if b <= 0x7F {
        1
    } else if b <= 0xDF {
        2
    } else if b <= 0xEF {
        3
    } else if b <= 0xF4 {
        4
    } else {
        1
    } // invalid → skip 1 byte
}

// --- Decode one UTF-8 codepoint ---
fn decode_one_utf8(atom: &ParsedAtom, len: usize) -> u32 {
    match len {
        1 => atom_to_u8(atom) as u32,
        2 => {
            let b0 = atom_to_u8(atom);
            let b1 = atom_to_u8(&rsh(3, 1, atom));
            if !is_continuation(b1) {
                return 0xFFFD;
            }
            let cp = ((b0 & 0x1F) as u32) << 6 | (b1 & 0x3F) as u32;
            if cp < 0x80 {
                0xFFFD
            } else {
                cp
            }
        }
        3 => {
            let b0 = atom_to_u8(atom);
            let b1 = atom_to_u8(&rsh(3, 1, atom));
            let b2 = atom_to_u8(&rsh(3, 2, atom));
            if !is_continuation(b1) || !is_continuation(b2) {
                return 0xFFFD;
            }
            let cp = ((b0 & 0x0F) as u32) << 12 | ((b1 & 0x3F) as u32) << 6 | (b2 & 0x3F) as u32;
            if cp < 0x800 || (0xD800..=0xDFFF).contains(&cp) {
                0xFFFD
            } else {
                cp
            }
        }
        4 => {
            let b0 = atom_to_u8(atom);
            let b1 = atom_to_u8(&rsh(3, 1, atom));
            let b2 = atom_to_u8(&rsh(3, 2, atom));
            let b3 = atom_to_u8(&rsh(3, 3, atom));
            if !is_continuation(b1) || !is_continuation(b2) || !is_continuation(b3) {
                return 0xFFFD;
            }
            let cp = ((b0 & 0x07) as u32) << 18
                | ((b1 & 0x3F) as u32) << 12
                | ((b2 & 0x3F) as u32) << 6
                | (b3 & 0x3F) as u32;
            if !(0x1_0000..=0x10_FFFF).contains(&cp) {
                0xFFFD
            } else {
                cp
            }
        }
        _ => 0xFFFD,
    }
}

// @t (UTF-8 atom) -> @c (UTF-32 packed atom)
pub fn taft(atom: &ParsedAtom) -> ParsedAtom {
    let mut codepoints = Vec::new();
    let mut current = atom.clone();

    loop {
        let len = teff(&current);
        if len == 0 {
            break;
        }
        let cp = decode_one_utf8(&current, len);
        codepoints.push(cp);
        current = rsh(3, len, &current); // shift by `len` bytes
    }

    // Pack into @c: each u32 in 32-bit lane, LSB-first (rap 5)
    if codepoints.is_empty() {
        ParsedAtom::Small(0)
    } else if codepoints.len() <= 4 {
        let mut acc: u128 = 0;
        for (i, &cp) in codepoints.iter().enumerate() {
            acc |= (cp as u128) << (i * 32);
        }
        ParsedAtom::Small(acc)
    } else {
        let mut acc = BigUint::zero();
        for (i, &cp) in codepoints.iter().enumerate() {
            acc |= BigUint::from(cp) << (i * 32);
        }
        ParsedAtom::from_biguint(acc)
    }
}

pub fn binary_number<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    let bit = any().filter(|c: &char| *c == '0' || *c == '1');

    let first_group = just('0').to("0".to_string()).or(just('1')
        .then(bit.repeated().at_most(3).collect::<String>())
        .map(|(h, t)| h.to_string() + &t));

    let first = just("0b").ignore_then(first_group);

    let rest = just('.')
        .ignore_then(gap().or_not())
        .ignore_then(bit.repeated().exactly(4).collect::<String>());

    first
        .then(rest.repeated().collect::<Vec<String>>())
        .map(|(first, rest)| {
            if rest.is_empty() {
                first
            } else {
                let mut s = first;
                for r in rest {
                    s.push_str(&r);
                }
                s
            }
        })
        .labelled("Binary")
}

pub fn hexadecimal_number<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    let hex = any().filter(|c: &char| c.is_ascii_hexdigit());

    let first_group = hex
        .then(hex.repeated().at_most(3).collect::<String>())
        .map(|(head, tail)| {
            if head == '0' && !tail.is_empty() {
                String::new()
            } else {
                let mut s = String::new();
                s.push(head);
                s.push_str(&tail);
                s
            }
        })
        .filter(|s| !s.is_empty());

    let first = just("0x").ignore_then(first_group);

    let rest = just('.')
        .ignore_then(gap().or_not())
        .ignore_then(hex.repeated().exactly(4).collect::<String>())
        .repeated()
        .collect::<Vec<String>>();

    first
        .then(rest)
        .map(|(first, rest)| {
            if rest.is_empty() {
                first
            } else {
                let mut s = first;
                for r in rest {
                    s.push_str(&r);
                }
                s
            }
        })
        .labelled("Hexadecimal")
}

pub fn ipv4_address<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    let octet = any()
        .filter(|c: &char| c.is_ascii_digit())
        .repeated()
        .at_least(1)
        .at_most(3)
        .collect::<String>()
        .filter(|s: &String| {
            if s.is_empty() || s.starts_with('0') && s.len() > 1 {
                return false;
            }
            let n = s.parse::<u16>().unwrap_or(256);
            n <= 255
        });

    octet
        .separated_by(just('.').ignore_then(gap().or_not()))
        .exactly(4)
        .collect::<Vec<String>>()
        .map(|parts| parts.join("."))
        .labelled("IPv4-Address")
}

pub fn ipv6_address<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    let rest = just('.')
        .ignore_then(gap().or_not())
        .ignore_then(alphanumeric())
        .repeated()
        .exactly(7)
        .collect::<Vec<_>>();

    alphanumeric()
        .then(rest)
        .map(|(first, mut rest)| {
            if rest.is_empty() {
                first.to_string()
            } else {
                let mut parts = vec![first];
                parts.append(&mut rest);
                parts.join(":").to_string()
            }
        })
        .labelled("Ipv6-Address")
}

pub fn base32_number<'src>() -> impl Parser<'src, &'src str, ParsedAtom, Err<'src>> {
    let base32_digit = any().filter(|c: &char| c.is_ascii_digit() || ('a'..='v').contains(c));

    let first = just("0v").ignore_then(choice((
        just('0').to("0".to_string()),
        any()
            .filter(|c: &char| matches!(c, '1'..='9' | 'a'..='v'))
            .then(base32_digit.repeated().at_most(4).collect::<String>())
            .map(|(h, t)| h.to_string() + &t),
    )));

    let rest = just('.')
        .ignore_then(gap().or_not())
        .ignore_then(base32_digit.repeated().exactly(5).collect::<String>())
        .repeated()
        .collect::<Vec<String>>();

    first
        .then(rest)
        .map(|(first, mut rest)| {
            if rest.is_empty() {
                base32_to_atom(first.to_string())
            } else {
                let mut parts = vec![first];
                parts.append(&mut rest);
                base32_to_atom(parts.join(""))
            }
        })
        .labelled("Base32")
}

pub fn base64_number<'src>() -> impl Parser<'src, &'src str, ParsedAtom, Err<'src>> {
    let digit = any().filter(|c: &char| matches!(c, '0'..='9' | 'a'..='z' | 'A'..='Z' | '-' | '~'));

    let first = just("0w").ignore_then(
        just('0').to("0".to_string()).or(any()
            .filter(|c: &char| matches!(c, '1'..='9' | 'a'..='z' | 'A'..='Z' | '-' | '~'))
            .then(digit.repeated().at_most(4).collect::<String>())
            .map(|(h, t)| h.to_string() + &t)),
    );

    let group = just('.')
        .ignore_then(gap().or_not())
        .ignore_then(digit.repeated().exactly(5).collect::<String>());

    first
        .then(group.repeated().collect::<Vec<String>>())
        .map(|(first, rest)| {
            if rest.is_empty() {
                base64_to_atom(first)
            } else {
                let mut parts = vec![first];
                parts.extend(rest);
                base64_to_atom(parts.join(""))
            }
        })
        .labelled("Base64")
}

pub fn base32<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    any()
        .filter(|c: &char| c.is_ascii_alphanumeric() && *c <= 'v')
        .repeated()
        .at_least(1)
        .collect::<String>()
}

pub fn digits<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    any()
        .filter(|c: &char| c.is_ascii_digit())
        .repeated()
        .at_least(1)
        .collect::<String>()
}

pub fn alphanumeric<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    any()
        .filter(|c: &char| c.is_ascii_alphanumeric())
        .repeated()
        .at_least(1)
        .collect::<String>()
}

pub fn decimal_number<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    let digit = any().filter(|c: &char| c.is_ascii_digit());

    let non_zero_digit = any().filter(|c: &char| matches!(c, '1'..='9'));

    let first = just('0').to("0".to_string()).or(non_zero_digit
        .then(digit.repeated().at_most(2).collect::<Vec<char>>())
        .map(|(h, t)| {
            let mut s = String::with_capacity(3);
            s.push(h);
            s.extend(t);
            s
        }));

    let three_digits = digit.repeated().exactly(3).collect::<String>();

    let rest = just('.')
        .ignore_then(gap().or_not())
        .ignore_then(three_digits)
        .repeated()
        .collect::<Vec<String>>();

    first
        .then(rest)
        .map(|(first_digits, rest_digits)| {
            let mut out = first_digits;
            for chunk in rest_digits {
                out.push_str(&chunk);
            }
            out
        })
        .labelled("Decimal Number")
}

fn snag<T>(index: usize, list: &[T]) -> &T {
    list.get(index).expect("snag: index out of bounds")
}

pub fn weld<T: Clone>(a: impl AsRef<[T]>, b: impl AsRef<[T]>) -> Vec<T> {
    let a = a.as_ref();
    let b = b.as_ref();
    let mut v = Vec::with_capacity(a.len() + b.len());
    v.extend_from_slice(a);
    v.extend_from_slice(b);
    v
}

pub fn scag<T: Clone>(n: usize, list: impl AsRef<[T]>) -> Vec<T> {
    list.as_ref().iter().take(n).cloned().collect()
}

pub fn slag<T: Clone>(n: usize, list: impl AsRef<[T]>) -> Vec<T> {
    list.as_ref().iter().skip(n).cloned().collect()
}

pub fn flop<T: Clone>(list: impl AsRef<[T]>) -> Vec<T> {
    let mut v = list.as_ref().to_vec();
    v.reverse();
    v
}

fn poof(pax: Path) -> Vec<Hoon> {
    pax.iter()
        .map(|a| {
            Hoon::Sand(
                "ta".to_string(),
                NounExpr::ParsedAtom(string_to_atom(a.clone())),
            )
        })
        .collect()
}

// used to create dbug traces
#[derive(Clone)]
pub struct LineMap {
    starts: Vec<usize>,
    col_offsets: Vec<u64>,
    source: Arc<str>,
    docs_enabled: bool,
}

fn leading_spaces(bytes: &[u8]) -> usize {
    bytes.iter().take_while(|&&b| b == b' ').count()
}

fn strip_doc_spaces(line: &str, spaces: usize) -> String {
    let mut bytes = line.as_bytes();
    let mut stripped = 0;
    while stripped < spaces && bytes.first() == Some(&b' ') {
        bytes = &bytes[1..];
        stripped += 1;
    }
    String::from_utf8_lossy(bytes).trim_end().to_string()
}

impl LineMap {
    #[inline]
    pub fn new(src: &str) -> Self {
        Self::new_with_docs(src, false)
    }

    #[inline]
    pub fn new_with_docs(src: &str, docs_enabled: bool) -> Self {
        let mut starts = Vec::with_capacity(128);
        starts.push(0);

        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }

        let bytes = src.as_bytes();
        let mut col_offsets = vec![0u64; starts.len()];
        let mut in_tall_tape = false;
        let mut tall_indent: usize = 0;

        for line_idx in 0..starts.len() {
            let start = starts[line_idx];
            let mut end = starts.get(line_idx + 1).copied().unwrap_or(bytes.len());
            if end > start && bytes[end - 1] == b'\n' {
                end -= 1;
            }
            let line = &bytes[start..end];
            let mut cursor = 0;
            let mut indent = 0usize;
            while cursor < line.len() && (line[cursor] == b' ' || line[cursor] == b'\t') {
                cursor += 1;
                indent += 1;
            }
            let mut trimmed_end = line.len();
            while trimmed_end > cursor
                && (line[trimmed_end - 1] == b' ' || line[trimmed_end - 1] == b'\t')
            {
                trimmed_end -= 1;
            }
            let trimmed = &line[cursor..trimmed_end];

            if !in_tall_tape {
                if trimmed.starts_with(b"\"\"\"") {
                    in_tall_tape = true;
                    tall_indent = indent;
                }
            } else if indent == tall_indent && trimmed.starts_with(b"\"\"\"") {
                in_tall_tape = false;
            } else {
                col_offsets[line_idx] = tall_indent as u64;
            }
        }

        let source = Arc::<str>::from(src);
        Self {
            starts,
            col_offsets,
            source,
            docs_enabled,
        }
    }

    #[inline(always)]
    fn line_col(&self, byte: usize) -> (u64, u64) {
        let line = match self.starts.binary_search(&byte) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let mut col = (byte - self.starts[line] + 1) as u64;
        let offset = self.col_offsets.get(line).copied().unwrap_or(0);
        if offset > 0 {
            col = col.saturating_sub(offset);
            if col == 0 {
                col = 1;
            }
        }

        ((line + 1) as u64, col)
    }

    #[inline(always)]
    pub fn pint(&self, span: std::ops::Range<usize>) -> Pint {
        Pint {
            p: self.line_col(span.start),
            q: self.line_col(span.end),
        }
    }

    fn help_before_with_target(
        &self,
        byte: usize,
        is_target: impl Fn(&Self, usize, usize) -> bool,
    ) -> Option<NounExpr> {
        self.help_before_with_target_options(byte, is_target, false)
    }

    fn help_before_with_target_options(
        &self,
        byte: usize,
        is_target: impl Fn(&Self, usize, usize) -> bool,
        skip_section_marker: bool,
    ) -> Option<NounExpr> {
        if !self.docs_enabled {
            return None;
        }

        let line = self.line_index(byte.min(self.source.len()));
        if line == 0 {
            return None;
        }
        if !is_target(self, line, byte) {
            return None;
        }
        let target_indent = self.line_indent(line)?;

        let mut docs = Vec::new();
        let mut idx = line - 1;
        loop {
            let Some((indent, content)) = self.doc_comment(idx) else {
                break;
            };
            docs.push((indent, content));
            if idx == 0 {
                break;
            }
            idx -= 1;
        }

        docs.reverse();
        while docs.first().is_some_and(|(_, line)| line.trim().is_empty()) {
            docs.remove(0);
        }
        while docs.last().is_some_and(|(_, line)| line.trim().is_empty()) {
            docs.pop();
        }
        if skip_section_marker
            && docs.first().is_some_and(|(_, line)| {
                leading_spaces(line.as_bytes()) >= 2 && strip_doc_spaces(line, 2).starts_with('#')
            })
        {
            if let Some(idx) = docs.iter().rposition(|(_, line)| line.trim().is_empty()) {
                docs.drain(..=idx);
                while docs.first().is_some_and(|(_, line)| line.trim().is_empty()) {
                    docs.remove(0);
                }
                while docs.last().is_some_and(|(_, line)| line.trim().is_empty()) {
                    docs.pop();
                }
            }
        }
        if docs.is_empty() {
            return None;
        }
        let doc_indent = docs[0].0;
        if target_indent > doc_indent {
            return None;
        }

        let summary_raw = &docs[0].1;
        let summary_indent = leading_spaces(summary_raw.as_bytes());
        let (summary_strip, detail_strip) = if summary_indent >= 4 {
            (4, 2)
        } else if summary_indent >= 2 {
            let summary = strip_doc_spaces(summary_raw, 2);
            if !matches!(
                summary.as_bytes().first(),
                Some(b'|' | b'.' | b'+' | b'$' | b'%')
            ) {
                return None;
            }
            (2, 4)
        } else {
            return None;
        };
        let summary = strip_doc_spaces(summary_raw, summary_strip);
        if summary.is_empty() {
            return None;
        }

        let mut sections: Vec<NounExpr> = Vec::new();
        let mut section: Vec<NounExpr> = Vec::new();
        let mut in_details = false;
        for (_, raw) in docs.iter().skip(1) {
            if raw.trim().is_empty() {
                if !section.is_empty() {
                    sections.push(Self::doc_list(std::mem::take(&mut section)));
                }
                in_details = true;
                continue;
            }
            if !in_details {
                continue;
            }

            let indent = leading_spaces(raw.as_bytes());
            let is_code = indent >= detail_strip + 2;
            let text = strip_doc_spaces(
                raw,
                if is_code {
                    detail_strip + 2
                } else {
                    detail_strip
                },
            );
            if text.is_empty() {
                continue;
            }
            section.push(Self::doc_cell(
                Self::doc_atom(is_code as u128),
                Self::doc_cord(&text),
            ));
        }
        if !section.is_empty() {
            sections.push(Self::doc_list(section));
        }

        let crib = Self::doc_cell(Self::doc_cord(&summary), Self::doc_list(sections));
        Some(Self::doc_cell(Self::doc_atom(0), crib))
    }

    fn help_before_spec(&self, byte: usize) -> Option<NounExpr> {
        self.help_before_with_target(byte, Self::line_starts_like_spec_doc_target)
    }

    pub fn help_before_body_spec(&self, byte: usize) -> Option<NounExpr> {
        self.help_before_with_target(byte, Self::line_starts_like_body_spec_doc_target)
    }

    fn help_before_hoon(&self, byte: usize) -> Option<NounExpr> {
        self.help_before_with_target(byte, Self::line_starts_like_hoon_target)
    }

    fn help_before_chapter_label(&self, byte: usize) -> Option<NounExpr> {
        self.help_before_with_target_options(byte, Self::line_starts_like_hoon_target, true)
    }

    fn postfix_doc_summary(&self, start_byte: usize, end_byte: usize) -> Option<String> {
        if !self.docs_enabled {
            return None;
        }

        let len = self.source.len();
        let start_line = self.line_index(start_byte.min(len));
        let end_line = self.line_index(end_byte.min(len));
        if start_line != end_line {
            return None;
        }

        let (line_start, line_end) = self.line_bounds(end_line)?;
        let line = &self.source.as_bytes()[line_start..line_end];
        let start_offset = start_byte.saturating_sub(line_start).min(line.len());
        let mut prefix_start = 0usize;
        while prefix_start < start_offset
            && (line[prefix_start] == b' ' || line[prefix_start] == b'\t')
        {
            prefix_start += 1;
        }
        let mut prefix_end = start_offset;
        while prefix_end > prefix_start
            && (line[prefix_end - 1] == b' ' || line[prefix_end - 1] == b'\t')
        {
            prefix_end -= 1;
        }
        let prefix = &line[prefix_start..prefix_end];
        if prefix.is_empty()
            || !(prefix.len() == 2
                && prefix
                    .iter()
                    .copied()
                    .all(|byte| !byte.is_ascii_alphanumeric() && byte != b'_' && byte != b' '))
        {
            return None;
        }

        let mut cursor = end_byte.saturating_sub(line_start).min(line.len());
        while cursor < line.len() && (line[cursor] == b' ' || line[cursor] == b'\t') {
            cursor += 1;
        }
        if line.get(cursor) != Some(&b':') || line.get(cursor + 1) != Some(&b':') {
            return None;
        }

        let mut trimmed_end = line.len();
        while trimmed_end > cursor
            && (line[trimmed_end - 1] == b' ' || line[trimmed_end - 1] == b'\t')
        {
            trimmed_end -= 1;
        }
        if trimmed_end - cursor > 2
            && line.get(trimmed_end - 2) == Some(&b':')
            && line.get(trimmed_end - 1) == Some(&b':')
        {
            return None;
        }

        let mut content_start = cursor + 2;
        let mut spaces = 0usize;
        while content_start < trimmed_end && line[content_start] == b' ' {
            spaces += 1;
            content_start += 1;
        }
        if spaces != 2 || content_start >= trimmed_end {
            return None;
        }
        if matches!(line.get(content_start), Some(b' ' | b'\t' | b':')) {
            return None;
        }

        Some(String::from_utf8_lossy(&line[content_start..trimmed_end]).to_string())
    }

    fn help_after(&self, start_byte: usize, end_byte: usize) -> Option<NounExpr> {
        let summary = self.postfix_doc_summary(start_byte, end_byte)?;
        let crib = Self::doc_cell(Self::doc_cord(&summary), Self::doc_atom(0));
        Some(Self::doc_cell(Self::doc_atom(0), crib))
    }

    fn arm_postfix_help(
        &self,
        link_tag: &str,
        arm_name: &str,
        start_byte: usize,
        end_byte: usize,
    ) -> Option<NounExpr> {
        let summary = self.postfix_doc_summary(start_byte, end_byte)?;
        let link_name = if arm_name == "$" {
            Self::doc_atom(0)
        } else {
            Self::doc_cord(arm_name)
        };
        let link = Self::doc_cell(Self::doc_cord(link_tag), link_name);
        let cuff = Self::doc_list(vec![link]);
        let crib = Self::doc_cell(Self::doc_cord(&summary), Self::doc_atom(0));
        Some(Self::doc_cell(cuff, crib))
    }

    #[inline(always)]
    fn line_index(&self, byte: usize) -> usize {
        match self.starts.binary_search(&byte) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
    }

    fn line_bounds(&self, idx: usize) -> Option<(usize, usize)> {
        let start = *self.starts.get(idx)?;
        let mut end = self
            .starts
            .get(idx + 1)
            .copied()
            .unwrap_or(self.source.len());
        let bytes = self.source.as_bytes();
        if end > start && bytes[end - 1] == b'\n' {
            end -= 1;
        }
        Some((start, end))
    }

    fn line_indent(&self, idx: usize) -> Option<usize> {
        let (start, end) = self.line_bounds(idx)?;
        let line = &self.source.as_bytes()[start..end];
        let mut cursor = 0;
        while cursor < line.len() && (line[cursor] == b' ' || line[cursor] == b'\t') {
            cursor += 1;
        }
        Some(cursor)
    }

    fn doc_comment(&self, idx: usize) -> Option<(usize, String)> {
        let (start, end) = self.line_bounds(idx)?;
        let line = &self.source.as_bytes()[start..end];
        let mut cursor = 0;
        while cursor < line.len() && (line[cursor] == b' ' || line[cursor] == b'\t') {
            cursor += 1;
        }
        if line.get(cursor) != Some(&b':') || line.get(cursor + 1) != Some(&b':') {
            return None;
        }
        let mut trimmed_end = line.len();
        while trimmed_end > cursor
            && (line[trimmed_end - 1] == b' ' || line[trimmed_end - 1] == b'\t')
        {
            trimmed_end -= 1;
        }
        if trimmed_end - cursor > 2
            && line.get(trimmed_end - 2) == Some(&b':')
            && line.get(trimmed_end - 1) == Some(&b':')
        {
            return None;
        }
        let content = &line[cursor + 2..trimmed_end];
        let mut content_start = 0;
        while content_start < content.len() && content[content_start] == b' ' {
            content_start += 1;
        }
        if content.get(content_start) == Some(&b':') {
            return None;
        }
        Some((cursor, String::from_utf8_lossy(content).into_owned()))
    }

    fn line_starts_like_spec_doc_target(&self, idx: usize, byte: usize) -> bool {
        let Some((start, end)) = self.line_bounds(idx) else {
            return false;
        };
        let line = &self.source.as_bytes()[start..end];
        let mut cursor = 0;
        while cursor < line.len() && (line[cursor] == b' ' || line[cursor] == b'\t') {
            cursor += 1;
        }
        if start + cursor != byte.min(self.source.len()) {
            return false;
        }
        match line.get(cursor).copied() {
            Some(b'$' | b'@' | b'*' | b'(' | b'[' | b'%' | b'_') => true,
            Some(b'!') => line.get(cursor + 1) == Some(&b'!'),
            Some(b'?') => !matches!(
                line.get(cursor + 1),
                Some(b'-' | b':' | b'.' | b'~' | b'=' | b'?' | b'|' | b'&' | b'!')
            ),
            Some(b'^') => !matches!(line.get(cursor + 1), Some(b'-' | b'+' | b'=' | b'|')),
            Some(b'~') => !matches!(
                line.get(cursor + 1),
                Some(b'%' | b'/' | b'>' | b'+' | b'-' | b'&' | b'|' | b'_')
            ),
            Some(ch) => ch.is_ascii_alphabetic(),
            None => false,
        }
    }

    fn line_starts_like_body_spec_doc_target(&self, idx: usize, byte: usize) -> bool {
        let Some((start, end)) = self.line_bounds(idx) else {
            return false;
        };
        let line = &self.source.as_bytes()[start..end];
        let mut cursor = 0;
        while cursor < line.len() && (line[cursor] == b' ' || line[cursor] == b'\t') {
            cursor += 1;
        }
        if start + cursor != byte.min(self.source.len()) {
            return false;
        }
        if line.get(cursor) == Some(&b'[') {
            return true;
        }
        if line.get(cursor) != Some(&b'$') {
            return Self::line_starts_like_spec_doc_target(self, idx, byte);
        }

        self.doc_summary_before_line(idx)
            .is_some_and(|summary| summary != "tree mold generator")
    }

    fn doc_summary_before_line(&self, idx: usize) -> Option<String> {
        let mut docs = Vec::new();
        let mut scan_idx = idx;
        while scan_idx > 0 {
            scan_idx -= 1;
            let Some((_, content)) = self.doc_comment(scan_idx) else {
                break;
            };
            docs.push(content);
        }
        docs.reverse();
        while docs.first().is_some_and(|line| line.trim().is_empty()) {
            docs.remove(0);
        }
        docs.first().map(|line| {
            let indent = leading_spaces(line.as_bytes());
            let strip = if indent >= 4 { 4 } else { 2 };
            strip_doc_spaces(line, strip)
        })
    }

    fn line_starts_like_hoon_target(&self, idx: usize, byte: usize) -> bool {
        let Some((start, end)) = self.line_bounds(idx) else {
            return false;
        };
        let line = &self.source.as_bytes()[start..end];
        let mut cursor = 0;
        while cursor < line.len() && (line[cursor] == b' ' || line[cursor] == b'\t') {
            cursor += 1;
        }
        if start + cursor != byte.min(self.source.len()) {
            return false;
        }
        match line.get(cursor).copied() {
            Some(b':') => !matches!(line.get(cursor + 1), Some(b':')),
            Some(b'-') => !matches!(line.get(cursor + 1), Some(b'-')),
            Some(
                b'!' | b'"' | b'$' | b'%' | b'&' | b'\'' | b'(' | b'+' | b',' | b'.' | b'/' | b';'
                | b'<' | b'=' | b'>' | b'?' | b'@' | b'[' | b'^' | b'_' | b'`' | b'~' | b'|',
            ) => true,
            Some(_) => false,
            None => false,
        }
    }

    fn doc_atom(value: u128) -> NounExpr {
        NounExpr::ParsedAtom(ParsedAtom::Small(value))
    }

    fn doc_cord(value: &str) -> NounExpr {
        NounExpr::ParsedAtom(string_to_atom(value.to_string()))
    }

    fn doc_cell(head: NounExpr, tail: NounExpr) -> NounExpr {
        NounExpr::Cell(Box::new(head), Box::new(tail))
    }

    fn doc_list(items: Vec<NounExpr>) -> NounExpr {
        items
            .into_iter()
            .rev()
            .fold(Self::doc_atom(0), |tail, item| Self::doc_cell(item, tail))
    }

    #[inline]
    fn expand_gap_start(&self, start: usize) -> usize {
        let bytes = self.source.as_bytes();
        let mut start = start.min(bytes.len());
        while start < bytes.len() {
            match bytes[start] {
                b' ' | b'\t' | b'\n' | b'\r' => start += 1,
                _ => break,
            }
        }
        let mut line_start = start;
        while line_start > 0 && bytes[line_start - 1] != b'\n' {
            line_start -= 1;
        }
        #[cfg(test)]
        let debug_expand = std::env::var("DEBUG_EXPAND_GAP_START").is_ok();
        #[cfg(test)]
        if debug_expand {
            let (line_no, col_no) = self.line_col(start);
            eprintln!(
                "expand-gap start={} line_start={} line={} col={}",
                start, line_start, line_no, col_no
            );
        }
        let mut saw_non_space = None;
        for idx in line_start..start {
            match bytes[idx] {
                b' ' | b'\t' => {}
                _ => {
                    saw_non_space = Some(idx);
                    break;
                }
            }
        }
        if saw_non_space.is_some() {
            let mut cursor = line_start;
            while cursor < bytes.len() && (bytes[cursor] == b' ' || bytes[cursor] == b'\t') {
                cursor += 1;
            }
            if bytes.get(cursor) == Some(&b':') && bytes.get(cursor + 1) == Some(&b':') {
                start = cursor;
            } else {
                return start;
            }
        }

        let doccord_info = |line: &[u8], idx: usize, min_spaces: usize| -> Option<(bool, usize)> {
            if idx + 1 >= line.len() || line[idx] != b':' || line[idx + 1] != b':' {
                return None;
            }
            let after = idx + 2;
            let mut end = line.len();
            while end > idx && (line[end - 1] == b' ' || line[end - 1] == b'\t') {
                end -= 1;
            }
            if end - idx > 2 && line[end - 2] == b':' && line[end - 1] == b':' {
                return None;
            }
            let mut spaces = 0;
            let mut pos = after;
            while pos < line.len() && line[pos] == b' ' {
                spaces += 1;
                pos += 1;
            }
            let mut content_start = pos;
            while content_start < line.len() && line[content_start] == b' ' {
                content_start += 1;
            }
            let has_content = content_start < line.len();
            if has_content
                && line.get(content_start) == Some(&b'~')
                && line.get(content_start + 1) == Some(&b'_')
            {
                return None;
            }
            let mut compact_doc = false;
            if has_content && spaces < min_spaces {
                if spaces == 0 {
                    let ch = line[content_start];
                    if ch != b':' && !ch.is_ascii_alphanumeric() {
                        compact_doc = true;
                    } else {
                        return None;
                    }
                } else {
                    return None;
                }
            }
            if has_content
                && line[content_start] == b':'
                && line.get(content_start + 1) == Some(&b':')
            {
                return None;
            }
            Some((has_content && !compact_doc, idx))
        };
        let doc_comment_offset = |line: &[u8], idx: usize| -> Option<usize> {
            if idx + 1 >= line.len() || line[idx] != b':' || line[idx + 1] != b':' {
                return None;
            }
            let mut end = line.len();
            while end > idx && (line[end - 1] == b' ' || line[end - 1] == b'\t') {
                end -= 1;
            }
            if end - idx > 2 && line[end - 2] == b':' && line[end - 1] == b':' {
                return None;
            }
            Some(idx)
        };

        let inline_doc_offset = |line: &[u8]| -> Option<(bool, usize)> {
            let mut has_code = false;
            for (idx, ch) in line.iter().enumerate() {
                if *ch == b':' && idx + 1 < line.len() && line[idx + 1] == b':' {
                    if idx > 0 && (line[idx - 1] == b' ' || line[idx - 1] == b'\t') {
                        if has_code {
                            if let Some((has_content, doc_offset)) = doccord_info(line, idx, 1) {
                                return Some((has_content, doc_offset));
                            }
                        }
                    }
                }
                if *ch != b' ' && *ch != b'\t' {
                    has_code = true;
                }
            }
            None
        };
        let doc_line_is_label = |line: &[u8], doc_offset: usize| -> bool {
            let mut idx = doc_offset + 2;
            while idx < line.len() && line[idx] == b' ' {
                idx += 1;
            }
            let mut end = line.len();
            while end > idx && (line[end - 1] == b' ' || line[end - 1] == b'\t') {
                end -= 1;
            }
            if idx >= end {
                return false;
            }
            let content = &line[idx..end];
            let mut word_end = 0;
            while word_end < content.len() {
                let b = content[word_end];
                if b.is_ascii_alphanumeric() || b == b'-' {
                    word_end += 1;
                } else {
                    break;
                }
            }
            word_end > 0 && content.get(word_end) == Some(&b':')
        };
        let doc_line_is_simple_heading = |line: &[u8], doc_offset: usize| -> bool {
            let mut idx = doc_offset + 2;
            while idx < line.len() && line[idx] == b' ' {
                idx += 1;
            }
            let mut end = line.len();
            while end > idx && (line[end - 1] == b' ' || line[end - 1] == b'\t') {
                end -= 1;
            }
            if idx >= end {
                return false;
            }
            let mut saw_letter = false;
            for &b in &line[idx..end] {
                if b == b' ' {
                    continue;
                }
                if b.is_ascii_lowercase() {
                    saw_letter = true;
                    continue;
                }
                return false;
            }
            saw_letter
        };
        let doc_line_is_single_word_heading = |line: &[u8], doc_offset: usize| -> bool {
            let mut idx = doc_offset + 2;
            while idx < line.len() && line[idx] == b' ' {
                idx += 1;
            }
            let mut end = line.len();
            while end > idx && (line[end - 1] == b' ' || line[end - 1] == b'\t') {
                end -= 1;
            }
            if idx >= end {
                return false;
            }
            let mut saw_letter = false;
            for &b in &line[idx..end] {
                if b == b' ' || b == b'\t' {
                    return false;
                }
                if b.is_ascii_lowercase() || b == b'-' {
                    saw_letter = true;
                    continue;
                }
                return false;
            }
            saw_letter
        };
        let doc_line_anchorable = |line: &[u8], doc_offset: usize| -> bool {
            let mut idx = doc_offset + 2;
            while idx < line.len() && line[idx] == b' ' {
                idx += 1;
            }
            let mut end = line.len();
            while end > idx && (line[end - 1] == b' ' || line[end - 1] == b'\t') {
                end -= 1;
            }
            if idx >= end {
                return false;
            }
            let content = &line[idx..end];
            if content.starts_with(b"XX")
                && matches!(content.get(2), None | Some(b' ') | Some(b'\t') | Some(b':'))
            {
                return false;
            }
            if content[0] == b':' {
                return false;
            }
            let mut has_upper = false;
            let mut has_lower = false;
            let mut has_other = false;
            for &b in content {
                if b.is_ascii_lowercase() {
                    has_lower = true;
                    break;
                }
                if b.is_ascii_uppercase() {
                    has_upper = true;
                } else if b != b' ' && b != b'\t' {
                    has_other = true;
                }
            }
            if !has_lower && has_upper && !has_other {
                return false;
            }
            true
        };
        let doc_line_starts_with_rune_like = |line: &[u8], doc_offset: usize| -> bool {
            let mut idx = doc_offset + 2;
            while idx < line.len() && line[idx] == b' ' {
                idx += 1;
            }
            match line.get(idx) {
                Some(b'?') => true,
                Some(b'=') => true,
                Some(b'|') => matches!(
                    line.get(idx + 1),
                    Some(b'=' | b'*' | b'%' | b'$' | b'+' | b'-' | b'_' | b':')
                ),
                Some(b'~') => matches!(
                    line.get(idx + 1),
                    Some(b'>' | b'/' | b'%' | b'+' | b'-' | b'&' | b'|')
                ),
                _ => false,
            }
        };
        let doc_line_is_compact_plus_payload = |line: &[u8], doc_offset: usize| -> bool {
            let mut idx = doc_offset + 2;
            while idx < line.len() && line[idx] == b' ' {
                idx += 1;
            }
            let mut end = line.len();
            while end > idx && (line[end - 1] == b' ' || line[end - 1] == b'\t') {
                end -= 1;
            }
            idx < end
                && line[idx..end].iter().any(|b| *b == b'+')
                && line[idx..end].iter().all(|b| *b != b' ' && *b != b'\t')
        };
        let doc_line_mentions_name = |line: &[u8], doc_offset: usize, name: &[u8]| -> bool {
            let mut idx = doc_offset + 2;
            while idx < line.len() && line[idx] == b' ' {
                idx += 1;
            }
            if idx >= line.len() {
                return false;
            }
            let content = &line[idx..];
            let content = if matches!(content.get(0), Some(b'+' | b'-')) {
                &content[1..]
            } else {
                content
            };
            if !content.starts_with(name) {
                return false;
            }
            match content.get(name.len()) {
                None | Some(b' ') | Some(b'\t') | Some(b':') => true,
                _ => false,
            }
        };
        let doc_line_starts_with_triple_quote = |line: &[u8], doc_offset: usize| -> bool {
            let mut idx = doc_offset + 2;
            while idx < line.len() && line[idx] == b' ' {
                idx += 1;
            }
            line.get(idx..idx + 3) == Some(b"\"\"\"")
        };
        let doc_line_starts_with_tilde = |line: &[u8], doc_offset: usize| -> bool {
            let mut idx = doc_offset + 2;
            while idx < line.len() && line[idx] == b' ' {
                idx += 1;
            }
            line.get(idx) == Some(&b'~')
        };
        let is_tilde_header_line = |line: &[u8], cursor: usize| -> bool {
            line.get(cursor) == Some(&b'~')
                && matches!(line.get(cursor + 1), Some(b'%') | Some(b'/'))
        };
        let doc_line_is_trailing_comment = |line_start: usize, cursor: usize| -> bool {
            if line_start == 0 {
                return false;
            }
            let line_end = bytes[line_start..]
                .iter()
                .position(|&b| b == b'\n')
                .map_or(bytes.len(), |idx| line_start + idx);
            if line_start > 0 {
                let mut prev_end = line_start.saturating_sub(1);
                let mut prev_start = prev_end;
                while prev_start > 0 && bytes[prev_start - 1] != b'\n' {
                    prev_start -= 1;
                }
                let prev_line = &bytes[prev_start..prev_end];
                let mut prev_cursor = 0;
                while prev_cursor < prev_line.len()
                    && (prev_line[prev_cursor] == b' ' || prev_line[prev_cursor] == b'\t')
                {
                    prev_cursor += 1;
                }
                if prev_cursor < prev_line.len()
                    && prev_cursor == cursor
                    && (doccord_info(prev_line, prev_cursor, 2).is_some()
                        || doc_comment_offset(prev_line, prev_cursor).is_some())
                {
                    return false;
                }
            }
            let next_start = line_end + 1;
            if next_start < bytes.len() {
                let next_end = bytes[next_start..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(bytes.len(), |idx| next_start + idx);
                let next_line = &bytes[next_start..next_end];
                let mut next_cursor = 0;
                while next_cursor < next_line.len()
                    && (next_line[next_cursor] == b' ' || next_line[next_cursor] == b'\t')
                {
                    next_cursor += 1;
                }
                if next_cursor < next_line.len()
                    && next_cursor == cursor
                    && (doccord_info(next_line, next_cursor, 2).is_some()
                        || doc_comment_offset(next_line, next_cursor).is_some())
                {
                    return false;
                }
            }

            let mut prev_end = line_start.saturating_sub(1);
            let mut prev_cursor_opt = None;
            while prev_end > 0 {
                let mut prev_start = prev_end;
                while prev_start > 0 && bytes[prev_start - 1] != b'\n' {
                    prev_start -= 1;
                }
                let prev_line = &bytes[prev_start..prev_end];
                let mut prev_cursor = 0;
                while prev_cursor < prev_line.len()
                    && (prev_line[prev_cursor] == b' ' || prev_line[prev_cursor] == b'\t')
                {
                    prev_cursor += 1;
                }
                if prev_cursor == prev_line.len() {
                    return false;
                }
                if doccord_info(prev_line, prev_cursor, 2).is_some()
                    || doc_comment_offset(prev_line, prev_cursor).is_some()
                {
                    prev_end = prev_start.saturating_sub(1);
                    continue;
                }
                if matches!(inline_doc_offset(prev_line), Some((true, _))) {
                    return false;
                }
                if matches!(prev_line.get(prev_cursor), Some(b'^'))
                    && matches!(prev_line.get(prev_cursor + 1), Some(b'|'))
                {
                    return false;
                }
                if prev_line.get(prev_cursor) == Some(&b'=')
                    && prev_line.get(prev_cursor + 1) == Some(&b'>')
                {
                    let mut idx = prev_cursor + 2;
                    while idx < prev_line.len()
                        && (prev_line[idx] == b' ' || prev_line[idx] == b'\t')
                    {
                        idx += 1;
                    }
                    if idx == prev_line.len() {
                        return false;
                    }
                }
                prev_cursor_opt = Some(prev_cursor);
                break;
            }
            let Some(prev_cursor) = prev_cursor_opt else {
                return false;
            };

            let mut next_start = line_end + 1;
            let mut next_cursor_opt = None;
            while next_start < bytes.len() {
                let next_end = bytes[next_start..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(bytes.len(), |idx| next_start + idx);
                let next_line = &bytes[next_start..next_end];
                let mut next_cursor = 0;
                while next_cursor < next_line.len()
                    && (next_line[next_cursor] == b' ' || next_line[next_cursor] == b'\t')
                {
                    next_cursor += 1;
                }
                if next_cursor == next_line.len() {
                    return false;
                }
                if doccord_info(next_line, next_cursor, 2).is_some()
                    || doc_comment_offset(next_line, next_cursor).is_some()
                {
                    next_start = next_end + 1;
                    continue;
                }
                if matches!(next_line.get(next_cursor), Some(b'^'))
                    && matches!(next_line.get(next_cursor + 1), Some(b'|'))
                {
                    return false;
                }
                if next_line.get(next_cursor) == Some(&b'=')
                    && next_line.get(next_cursor + 1) == Some(&b'>')
                {
                    let mut idx = next_cursor + 2;
                    while idx < next_line.len()
                        && (next_line[idx] == b' ' || next_line[idx] == b'\t')
                    {
                        idx += 1;
                    }
                    if idx == next_line.len() {
                        return false;
                    }
                }
                next_cursor_opt = Some(next_cursor);
                break;
            }

            let Some(next_cursor) = next_cursor_opt else {
                return false;
            };

            prev_cursor == cursor && next_cursor == cursor
        };
        let next_non_doc_line_start = |line_start: usize| -> Option<usize> {
            let mut next_start = bytes[line_start..]
                .iter()
                .position(|&b| b == b'\n')
                .map_or(bytes.len(), |idx| line_start + idx + 1);
            while next_start < bytes.len() {
                let next_end = bytes[next_start..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(bytes.len(), |idx| next_start + idx);
                let next_line = &bytes[next_start..next_end];
                let mut next_cursor = 0;
                while next_cursor < next_line.len()
                    && (next_line[next_cursor] == b' ' || next_line[next_cursor] == b'\t')
                {
                    next_cursor += 1;
                }
                if next_cursor == next_line.len() {
                    next_start = next_end + 1;
                    continue;
                }
                if doccord_info(next_line, next_cursor, 2).is_some()
                    || doc_comment_offset(next_line, next_cursor).is_some()
                {
                    next_start = next_end + 1;
                    continue;
                }
                return Some(next_start + next_cursor);
            }
            None
        };
        let is_terminator_line = |line: &[u8], cursor: usize| -> bool {
            let mut idx = cursor;
            while idx < line.len() && (line[idx] == b' ' || line[idx] == b'\t') {
                idx += 1;
            }
            if idx + 1 >= line.len() {
                return false;
            }
            let first = line[idx];
            let second = line[idx + 1];
            if !(first == b'=' && second == b'=') && !(first == b'-' && second == b'-') {
                return false;
            }
            idx += 2;
            while idx < line.len() && (line[idx] == b' ' || line[idx] == b'\t') {
                idx += 1;
            }
            idx == line.len()
        };
        let prev_non_doc_line = |line_start: usize| -> Option<(usize, &[u8])> {
            if line_start == 0 {
                return None;
            }
            let mut prev_end = line_start.saturating_sub(1);
            while prev_end > 0 {
                let mut prev_start = prev_end;
                while prev_start > 0 && bytes[prev_start - 1] != b'\n' {
                    prev_start -= 1;
                }
                let prev_line = &bytes[prev_start..prev_end];
                let mut prev_cursor = 0;
                while prev_cursor < prev_line.len()
                    && (prev_line[prev_cursor] == b' ' || prev_line[prev_cursor] == b'\t')
                {
                    prev_cursor += 1;
                }
                if prev_cursor == prev_line.len() {
                    prev_end = prev_start.saturating_sub(1);
                    continue;
                }
                if doccord_info(prev_line, prev_cursor, 2).is_some()
                    || doc_comment_offset(prev_line, prev_cursor).is_some()
                {
                    prev_end = prev_start.saturating_sub(1);
                    continue;
                }
                return Some((prev_cursor, prev_line));
            }
            None
        };
        let doc_line_preceded_by_outdent_terminator = |line_start: usize, cursor: usize| -> bool {
            let Some((prev_cursor, prev_line)) = prev_non_doc_line(line_start) else {
                return false;
            };
            if prev_cursor <= cursor {
                return false;
            }
            is_terminator_line(prev_line, prev_cursor)
        };
        let doc_line_preceded_by_equals_slash = |line_start: usize, cursor: usize| -> bool {
            let Some((prev_cursor, prev_line)) = prev_non_doc_line(line_start) else {
                return false;
            };
            if prev_cursor != cursor {
                return false;
            }
            prev_line.get(prev_cursor) == Some(&b'=')
                && prev_line.get(prev_cursor + 1) == Some(&b'/')
        };
        let doc_line_preceded_by_question_greater = |line_start: usize, cursor: usize| -> bool {
            let Some((prev_cursor, prev_line)) = prev_non_doc_line(line_start) else {
                return false;
            };
            if prev_cursor != cursor {
                return false;
            }
            prev_line.get(prev_cursor) == Some(&b'?')
                && prev_line.get(prev_cursor + 1) == Some(&b'>')
        };
        let doc_line_preceded_by_bar_dollar = |line_start: usize, cursor: usize| -> bool {
            let Some((prev_cursor, prev_line)) = prev_non_doc_line(line_start) else {
                return false;
            };
            if prev_cursor != cursor {
                return false;
            }
            prev_line.get(prev_cursor) == Some(&b'|')
                && prev_line.get(prev_cursor + 1) == Some(&b'$')
        };
        let doc_content_indent = |line: &[u8], doc_offset: usize| -> usize {
            let mut idx = doc_offset + 2;
            while idx < line.len() && line[idx] == b' ' {
                idx += 1;
            }
            idx - (doc_offset + 2)
        };
        let is_bar_gate_sample_line = |line: &[u8], cursor: usize| -> bool {
            line.get(cursor) == Some(&b'|')
                && matches!(line.get(cursor + 1), Some(b':') | Some(b'='))
        };
        let inline_doc_starts_with_bar = |line: &[u8], offset: usize| -> bool {
            let mut idx = offset + 2;
            while idx < line.len() && line[idx] == b' ' {
                idx += 1;
            }
            matches!(line.get(idx), Some(b'|'))
        };
        let inline_doc_starts_with_branch_marker = |line: &[u8], offset: usize| -> bool {
            let mut idx = offset + 2;
            while idx < line.len() && line[idx] == b' ' {
                idx += 1;
            }
            let start = idx;
            let first = match line.get(start) {
                Some(&b) => b,
                None => return false,
            };
            if !matches!(first, b'.' | b'+' | b'$') {
                return false;
            }
            while idx < line.len() && line[idx] != b' ' && line[idx] != b'\t' {
                idx += 1;
            }
            while idx < line.len() && (line[idx] == b' ' || line[idx] == b'\t') {
                idx += 1;
            }
            if idx >= line.len() {
                return true;
            }
            line.get(idx) == Some(&b':') && line.get(idx + 1) == Some(&b':')
        };
        let is_face_line = |line: &[u8], cursor: usize| -> bool {
            let Some(&first) = line.get(cursor) else {
                return false;
            };
            if !first.is_ascii_lowercase() {
                return false;
            }
            let mut idx = cursor;
            while idx < line.len() {
                let b = line[idx];
                if b.is_ascii_alphanumeric() || b == b'-' {
                    idx += 1;
                } else {
                    break;
                }
            }
            idx < line.len() && line[idx] == b':' && line.get(idx + 1) != Some(&b':')
        };
        let is_face_bind_line = |line: &[u8], cursor: usize| -> bool {
            let Some(&first) = line.get(cursor) else {
                return false;
            };
            if !first.is_ascii_lowercase() {
                return false;
            }
            let mut idx = cursor;
            while idx < line.len() {
                let b = line[idx];
                if b.is_ascii_alphanumeric() || b == b'-' {
                    idx += 1;
                } else {
                    break;
                }
            }
            line.get(idx) == Some(&b'=')
        };
        let inline_doc_is_heading = |line: &[u8], offset: usize| -> bool {
            let mut idx = offset + 2;
            while idx < line.len() && line[idx] == b' ' {
                idx += 1;
            }
            match line.get(idx) {
                Some(b'.' | b'|' | b'=' | b'-' | b'+' | b'`') => true,
                Some(b) if b.is_ascii_alphanumeric() => false,
                Some(_) => false,
                None => false,
            }
        };
        let is_plus_header_line = |line: &[u8], cursor: usize| -> bool {
            line.get(cursor) == Some(&b'+')
                && matches!(line.get(cursor + 1), Some(b'+') | Some(b'$'))
        };
        let doc_line_under_plus_header = |line_start: usize, cursor: usize| -> bool {
            let mut scan_end = line_start.saturating_sub(1);
            while scan_end > 0 {
                let mut scan_start = scan_end;
                while scan_start > 0 && bytes[scan_start - 1] != b'\n' {
                    scan_start -= 1;
                }
                let scan_line = &bytes[scan_start..scan_end];
                let mut scan_cursor = 0;
                while scan_cursor < scan_line.len()
                    && (scan_line[scan_cursor] == b' ' || scan_line[scan_cursor] == b'\t')
                {
                    scan_cursor += 1;
                }
                if scan_cursor == scan_line.len() {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if doccord_info(scan_line, scan_cursor, 2).is_some() {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if scan_line.get(scan_cursor) == Some(&b'~')
                    && scan_line.get(scan_cursor + 1) == Some(&b'/')
                {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if scan_cursor > cursor {
                    return false;
                }
                if scan_cursor < cursor && cursor.saturating_sub(scan_cursor) != 2 {
                    return false;
                }
                return is_plus_header_line(scan_line, scan_cursor);
            }
            false
        };
        let is_type_header_line = |line: &[u8], cursor: usize| -> bool {
            line.get(cursor) == Some(&b'+') && line.get(cursor + 1) == Some(&b'$')
        };
        let is_tilde_percent_line = |line: &[u8], cursor: usize| -> bool {
            line.get(cursor) == Some(&b'~') && line.get(cursor + 1) == Some(&b'%')
        };
        let doc_line_preceded_by_tilde_percent = |line_start: usize, cursor: usize| -> bool {
            let Some((prev_cursor, prev_line)) = prev_non_doc_line(line_start) else {
                return false;
            };
            if prev_cursor != cursor {
                return false;
            }
            is_tilde_percent_line(prev_line, prev_cursor)
        };
        let doc_line_preceded_by_tilde_slash = |line_start: usize, cursor: usize| -> bool {
            let Some((prev_cursor, prev_line)) = prev_non_doc_line(line_start) else {
                return false;
            };
            if prev_cursor != cursor {
                return false;
            }
            prev_line.get(prev_cursor) == Some(&b'~')
                && prev_line.get(prev_cursor + 1) == Some(&b'/')
        };
        let doc_line_under_type_header = |line_start: usize, cursor: usize| -> bool {
            let mut scan_end = line_start.saturating_sub(1);
            while scan_end > 0 {
                let mut scan_start = scan_end;
                while scan_start > 0 && bytes[scan_start - 1] != b'\n' {
                    scan_start -= 1;
                }
                let scan_line = &bytes[scan_start..scan_end];
                let mut scan_cursor = 0;
                while scan_cursor < scan_line.len()
                    && (scan_line[scan_cursor] == b' ' || scan_line[scan_cursor] == b'\t')
                {
                    scan_cursor += 1;
                }
                if scan_cursor == scan_line.len() {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if doccord_info(scan_line, scan_cursor, 2).is_some() {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if scan_line.get(scan_cursor) == Some(&b'~')
                    && scan_line.get(scan_cursor + 1) == Some(&b'/')
                {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if scan_cursor > cursor {
                    return false;
                }
                if scan_cursor < cursor && cursor.saturating_sub(scan_cursor) != 2 {
                    return false;
                }
                return is_type_header_line(scan_line, scan_cursor);
            }
            false
        };
        let doc_line_under_dollar_colon_header = |line_start: usize, cursor: usize| -> bool {
            let mut scan_end = line_start.saturating_sub(1);
            while scan_end > 0 {
                let mut scan_start = scan_end;
                while scan_start > 0 && bytes[scan_start - 1] != b'\n' {
                    scan_start -= 1;
                }
                let scan_line = &bytes[scan_start..scan_end];
                let mut scan_cursor = 0;
                while scan_cursor < scan_line.len()
                    && (scan_line[scan_cursor] == b' ' || scan_line[scan_cursor] == b'\t')
                {
                    scan_cursor += 1;
                }
                if scan_cursor == scan_line.len() {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if doccord_info(scan_line, scan_cursor, 2).is_some() {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if scan_line.get(scan_cursor) == Some(&b'~')
                    && scan_line.get(scan_cursor + 1) == Some(&b'/')
                {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if scan_cursor >= cursor {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                return scan_line.get(scan_cursor) == Some(&b'$')
                    && scan_line.get(scan_cursor + 1) == Some(&b':');
            }
            false
        };
        let doc_line_under_tilde_percent_in_arm = |line_start: usize, cursor: usize| -> bool {
            let mut scan_end = line_start.saturating_sub(1);
            let mut tilde_start = None;
            while scan_end > 0 {
                let mut scan_start = scan_end;
                while scan_start > 0 && bytes[scan_start - 1] != b'\n' {
                    scan_start -= 1;
                }
                let scan_line = &bytes[scan_start..scan_end];
                let mut scan_cursor = 0;
                while scan_cursor < scan_line.len()
                    && (scan_line[scan_cursor] == b' ' || scan_line[scan_cursor] == b'\t')
                {
                    scan_cursor += 1;
                }
                if scan_cursor == scan_line.len() {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if doccord_info(scan_line, scan_cursor, 2).is_some() {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if scan_line.get(scan_cursor) == Some(&b'~')
                    && scan_line.get(scan_cursor + 1) == Some(&b'/')
                {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if scan_cursor > cursor {
                    return false;
                }
                if scan_cursor < cursor && cursor.saturating_sub(scan_cursor) != 2 {
                    return false;
                }
                if is_tilde_percent_line(scan_line, scan_cursor) {
                    tilde_start = Some(scan_start);
                }
                break;
            }
            let Some(tilde_start) = tilde_start else {
                return false;
            };
            let mut scan_end = tilde_start.saturating_sub(1);
            while scan_end > 0 {
                let mut scan_start = scan_end;
                while scan_start > 0 && bytes[scan_start - 1] != b'\n' {
                    scan_start -= 1;
                }
                let scan_line = &bytes[scan_start..scan_end];
                let mut scan_cursor = 0;
                while scan_cursor < scan_line.len()
                    && (scan_line[scan_cursor] == b' ' || scan_line[scan_cursor] == b'\t')
                {
                    scan_cursor += 1;
                }
                if scan_cursor == scan_line.len() {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if doccord_info(scan_line, scan_cursor, 2).is_some() {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if scan_line.get(scan_cursor) == Some(&b'~')
                    && scan_line.get(scan_cursor + 1) == Some(&b'/')
                {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if scan_cursor > cursor {
                    return false;
                }
                if scan_cursor < cursor && cursor.saturating_sub(scan_cursor) != 2 {
                    return false;
                }
                return is_plus_header_line(scan_line, scan_cursor);
            }
            false
        };
        let doc_line_plus_header_has_doc_content = |line_start: usize, cursor: usize| -> bool {
            let mut scan_end = line_start.saturating_sub(1);
            let mut header_start = None;
            while scan_end > 0 {
                let mut scan_start = scan_end;
                while scan_start > 0 && bytes[scan_start - 1] != b'\n' {
                    scan_start -= 1;
                }
                let scan_line = &bytes[scan_start..scan_end];
                let mut scan_cursor = 0;
                while scan_cursor < scan_line.len()
                    && (scan_line[scan_cursor] == b' ' || scan_line[scan_cursor] == b'\t')
                {
                    scan_cursor += 1;
                }
                if scan_cursor == scan_line.len() {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if doccord_info(scan_line, scan_cursor, 2).is_some() {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if scan_line.get(scan_cursor) == Some(&b'~')
                    && scan_line.get(scan_cursor + 1) == Some(&b'/')
                {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if scan_cursor > cursor {
                    return false;
                }
                if scan_cursor < cursor && cursor.saturating_sub(scan_cursor) != 2 {
                    return false;
                }
                if is_plus_header_line(scan_line, scan_cursor) {
                    header_start = Some(scan_start);
                }
                break;
            }
            let Some(header_start) = header_start else {
                return false;
            };
            let mut header_prev_end = header_start.saturating_sub(1);
            while header_prev_end > 0 {
                let mut header_prev_start = header_prev_end;
                while header_prev_start > 0 && bytes[header_prev_start - 1] != b'\n' {
                    header_prev_start -= 1;
                }
                let header_prev_line = &bytes[header_prev_start..header_prev_end];
                let mut header_prev_cursor = 0;
                while header_prev_cursor < header_prev_line.len()
                    && (header_prev_line[header_prev_cursor] == b' '
                        || header_prev_line[header_prev_cursor] == b'\t')
                {
                    header_prev_cursor += 1;
                }
                if header_prev_cursor == header_prev_line.len() {
                    header_prev_end = header_prev_start.saturating_sub(1);
                    continue;
                }
                if let Some((has_content, _)) =
                    doccord_info(header_prev_line, header_prev_cursor, 2)
                {
                    if has_content {
                        return true;
                    }
                    header_prev_end = header_prev_start.saturating_sub(1);
                    continue;
                }
                break;
            }
            false
        };
        let doc_line_plus_header_has_inline_doc = |line_start: usize, cursor: usize| -> bool {
            let mut scan_end = line_start.saturating_sub(1);
            while scan_end > 0 {
                let mut scan_start = scan_end;
                while scan_start > 0 && bytes[scan_start - 1] != b'\n' {
                    scan_start -= 1;
                }
                let scan_line = &bytes[scan_start..scan_end];
                let mut scan_cursor = 0;
                while scan_cursor < scan_line.len()
                    && (scan_line[scan_cursor] == b' ' || scan_line[scan_cursor] == b'\t')
                {
                    scan_cursor += 1;
                }
                if scan_cursor == scan_line.len() {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if doccord_info(scan_line, scan_cursor, 2).is_some() {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if scan_line.get(scan_cursor) == Some(&b'~')
                    && scan_line.get(scan_cursor + 1) == Some(&b'/')
                {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if scan_cursor > cursor {
                    return false;
                }
                if scan_cursor < cursor && cursor.saturating_sub(scan_cursor) != 2 {
                    return false;
                }
                if is_plus_header_line(scan_line, scan_cursor) {
                    return matches!(inline_doc_offset(scan_line), Some((true, _)));
                }
                return false;
            }
            false
        };
        let is_arm_header_line = |line: &[u8], cursor: usize| -> bool {
            line.get(cursor) == Some(&b'+') && line.get(cursor + 1) == Some(&b'+')
        };
        let is_question_header_line =
            |line: &[u8], cursor: usize| -> bool { line.get(cursor) == Some(&b'?') };
        let doc_line_under_question_header = |line_start: usize, cursor: usize| -> bool {
            let Some((prev_cursor, prev_line)) = prev_non_doc_line(line_start) else {
                return false;
            };
            if prev_cursor >= cursor {
                return false;
            }
            if !is_question_header_line(prev_line, prev_cursor) {
                return false;
            }
            matches!(prev_line.get(prev_cursor + 1), Some(b'&') | Some(b'|'))
        };
        let doc_block_between_question_branches = |line_start: usize, doc_indent: usize| -> bool {
            let mut scan_end = line_start.saturating_sub(1);
            while scan_end > 0 {
                let mut scan_start = scan_end;
                while scan_start > 0 && bytes[scan_start - 1] != b'\n' {
                    scan_start -= 1;
                }
                let scan_line = &bytes[scan_start..scan_end];
                let mut scan_cursor = 0;
                while scan_cursor < scan_line.len()
                    && (scan_line[scan_cursor] == b' ' || scan_line[scan_cursor] == b'\t')
                {
                    scan_cursor += 1;
                }
                if scan_cursor == scan_line.len() {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if doccord_info(scan_line, scan_cursor, 2).is_some()
                    || doc_comment_offset(scan_line, scan_cursor).is_some()
                {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if scan_cursor > doc_indent {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if scan_cursor < doc_indent {
                    return false;
                }
                return is_question_header_line(scan_line, scan_cursor)
                    && scan_line.get(scan_cursor + 1) == Some(&b':');
            }
            false
        };
        let is_colon_caret_line = |line: &[u8], cursor: usize| -> bool {
            line.get(cursor) == Some(&b':') && line.get(cursor + 1) == Some(&b'^')
        };
        let is_colon_tilde_line = |line: &[u8], cursor: usize| -> bool {
            line.get(cursor) == Some(&b':') && line.get(cursor + 1) == Some(&b'~')
        };
        let is_dollar_header_line =
            |line: &[u8], cursor: usize| -> bool { line.get(cursor) == Some(&b'$') };
        let is_dollar_question_line = |line: &[u8], cursor: usize| -> bool {
            line.get(cursor) == Some(&b'$') && line.get(cursor + 1) == Some(&b'?')
        };
        let is_dollar_percent_line = |line: &[u8], cursor: usize| -> bool {
            line.get(cursor) == Some(&b'$') && line.get(cursor + 1) == Some(&b'%')
        };
        let is_dollar_paren_line = |line: &[u8], cursor: usize| -> bool {
            line.get(cursor) == Some(&b'$') && line.get(cursor + 1) == Some(&b'(')
        };
        let is_percent_header_line =
            |line: &[u8], cursor: usize| -> bool { line.get(cursor) == Some(&b'%') };
        let is_equals_header_line =
            |line: &[u8], cursor: usize| -> bool { line.get(cursor) == Some(&b'=') };
        let is_equals_plus_line = |line: &[u8], cursor: usize| -> bool {
            line.get(cursor) == Some(&b'=') && line.get(cursor + 1) == Some(&b'+')
        };
        let equals_plus_has_colon_star = |line: &[u8], cursor: usize, doc_offset: usize| -> bool {
            let end = doc_offset.min(line.len());
            let mut idx = cursor + 2;
            while idx + 1 < end {
                if line[idx] == b':' && line[idx + 1] == b'*' {
                    return true;
                }
                idx += 1;
            }
            false
        };
        let is_caret_hep_line = |line: &[u8], cursor: usize| -> bool {
            line.get(cursor) == Some(&b'^') && line.get(cursor + 1) == Some(&b'-')
        };
        let is_caret_bar_line = |line: &[u8], cursor: usize| -> bool {
            line.get(cursor) == Some(&b'^') && line.get(cursor + 1) == Some(&b'|')
        };
        let is_tilde_hint_line = |line: &[u8], cursor: usize| -> bool {
            line.get(cursor) == Some(&b'~') && line.get(cursor + 1) == Some(&b'_')
        };
        let is_tilde_slash_line = |line: &[u8], cursor: usize| -> bool {
            line.get(cursor) == Some(&b'~') && line.get(cursor + 1) == Some(&b'/')
        };
        let is_fas_import_line = |line: &[u8], cursor: usize| -> bool {
            line.get(cursor) == Some(&b'/')
                && matches!(line.get(cursor + 1), Some(b'=') | Some(b'#'))
        };
        let is_branch_line = |line: &[u8], cursor: usize| -> bool {
            line.get(cursor) == Some(&b'[') && line.get(cursor + 1) == Some(&b'%')
        };
        let is_branch_tag_line = |line: &[u8], cursor: usize| -> bool {
            if line.get(cursor) != Some(&b'%') {
                return false;
            }
            let mut idx = cursor + 1;
            while idx < line.len() {
                let b = line[idx];
                if b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' {
                    idx += 1;
                } else {
                    break;
                }
            }
            if idx == cursor + 1 {
                return false;
            }
            while idx < line.len() && (line[idx] == b' ' || line[idx] == b'\t') {
                idx += 1;
            }
            if idx >= line.len() {
                return true;
            }
            line.get(idx) == Some(&b':') && line.get(idx + 1) == Some(&b':')
        };
        let branch_line_has_multiple_tags =
            |line: &[u8], cursor: usize, doc_offset: usize| -> bool {
                let mut idx = cursor;
                let mut count = 0;
                let end = doc_offset.min(line.len());
                while idx + 1 < end {
                    if line[idx] == b'[' && line[idx + 1] == b'%' {
                        count += 1;
                        if count > 1 {
                            return true;
                        }
                        idx += 2;
                        continue;
                    }
                    idx += 1;
                }
                false
            };
        let branch_line_parent_header_char =
            |line_start: usize, cursor: usize, start_cursor: usize| -> Option<u8> {
                let mut scan_end = line_start.saturating_sub(1);
                while scan_end > 0 {
                    let mut scan_start = scan_end;
                    while scan_start > 0 && bytes[scan_start - 1] != b'\n' {
                        scan_start -= 1;
                    }
                    let scan_line = &bytes[scan_start..scan_end];
                    let mut scan_cursor = 0;
                    while scan_cursor < scan_line.len()
                        && (scan_line[scan_cursor] == b' ' || scan_line[scan_cursor] == b'\t')
                    {
                        scan_cursor += 1;
                    }
                    if scan_cursor == scan_line.len() {
                        scan_end = scan_start.saturating_sub(1);
                        continue;
                    }
                    if doccord_info(scan_line, scan_cursor, 2).is_some() {
                        scan_end = scan_start.saturating_sub(1);
                        continue;
                    }
                    if scan_cursor >= cursor {
                        scan_end = scan_start.saturating_sub(1);
                        continue;
                    }
                    if scan_cursor != start_cursor {
                        return None;
                    }
                    match scan_line.get(scan_cursor) {
                        Some(b':') if scan_line.get(scan_cursor + 1) != Some(&b':') => {
                            return scan_line.get(scan_cursor + 1).copied();
                        }
                        Some(b'?') => return Some(b'?'),
                        _ => return None,
                    }
                }
                None
            };

        let mut lead = line_start;
        while lead < bytes.len()
            && bytes[lead] != b'\n'
            && (bytes[lead] == b' ' || bytes[lead] == b'\t')
        {
            lead += 1;
        }
        if lead >= bytes.len() || bytes[lead] == b'\n' {
            return start;
        }
        let start_line_end = bytes[line_start..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |idx| line_start + idx);
        let start_line = &bytes[line_start..start_line_end];
        let mut start_cursor = 0;
        while start_cursor < start_line.len()
            && (start_line[start_cursor] == b' ' || start_line[start_cursor] == b'\t')
        {
            start_cursor += 1;
        }
        if doccord_info(start_line, start_cursor, 2).is_none() {
            let mut scan_end = line_start.saturating_sub(1);
            let mut doc_lines: Vec<(usize, usize, usize)> = Vec::new();
            let mut doc_min_indent: Option<usize> = None;
            let mut doc_max_indent: Option<usize> = None;
            let mut saw_doc = false;
            let mut doc_block_has_text = false;
            let mut prev_non_doc_cursor = None;
            let mut prev_non_doc_start = None;
            while scan_end > 0 {
                let mut scan_start = scan_end;
                while scan_start > 0 && bytes[scan_start - 1] != b'\n' {
                    scan_start -= 1;
                }
                let line = &bytes[scan_start..scan_end];
                let mut cursor = 0;
                while cursor < line.len() && (line[cursor] == b' ' || line[cursor] == b'\t') {
                    cursor += 1;
                }
                if cursor == line.len() {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if cursor != start_cursor {
                    prev_non_doc_cursor = Some(cursor);
                    prev_non_doc_start = Some(scan_start);
                    break;
                }
                if let Some((has_content, doc_offset)) = doccord_info(line, cursor, 2) {
                    saw_doc = true;
                    if has_content && !doc_line_starts_with_rune_like(line, doc_offset) {
                        doc_block_has_text = true;
                        let content_indent = doc_content_indent(line, doc_offset);
                        doc_lines.push((scan_start, doc_offset, content_indent));
                        doc_min_indent =
                            Some(doc_min_indent.map_or(content_indent, |v| v.min(content_indent)));
                        doc_max_indent =
                            Some(doc_max_indent.map_or(content_indent, |v| v.max(content_indent)));
                    }
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                break;
            }
            if saw_doc && doc_block_has_text {
                if let (Some(min_indent), Some(max_indent)) = (doc_min_indent, doc_max_indent) {
                    if max_indent > min_indent {
                        if let (Some(prev_cursor), Some(prev_start)) =
                            (prev_non_doc_cursor, prev_non_doc_start)
                        {
                            if prev_cursor > start_cursor
                                && doc_block_between_question_branches(prev_start, start_cursor)
                            {
                                if let Some((line_start, doc_offset, _)) = doc_lines
                                    .iter()
                                    .rfind(|(_, _, indent)| *indent == max_indent)
                                {
                                    return line_start + doc_offset;
                                }
                            }
                        }
                    }
                }
            }
        }
        if doccord_info(start_line, start_cursor, 2).is_some()
            && doc_line_is_trailing_comment(line_start, start_cursor)
        {
            if let Some(next_start) = next_non_doc_line_start(line_start) {
                return next_start;
            }
        }
        let start_is_term_line = start_line
            .get(start_cursor)
            .map(|b| b.is_ascii_lowercase())
            .unwrap_or(false);
        let start_is_face_line = is_face_line(start_line, start_cursor);
        let start_is_face_binding = is_face_bind_line(start_line, start_cursor);
        let start_equals_has_inline_body = if start_line.get(start_cursor) == Some(&b'=') {
            let mut idx = start_cursor;
            let mut tokens = 0;
            while idx < start_line.len() {
                while idx < start_line.len()
                    && (start_line[idx] == b' ' || start_line[idx] == b'\t')
                {
                    idx += 1;
                }
                if idx >= start_line.len() {
                    break;
                }
                tokens += 1;
                while idx < start_line.len() && start_line[idx] != b' ' && start_line[idx] != b'\t'
                {
                    idx += 1;
                }
            }
            tokens > 2
        } else {
            false
        };
        let plus_header_has_inline_body = |line: &[u8], cursor: usize, doc_offset: usize| -> bool {
            let end = doc_offset.min(line.len());
            let mut idx = cursor;
            let mut tokens = 0;
            while idx < end {
                while idx < end && (line[idx] == b' ' || line[idx] == b'\t') {
                    idx += 1;
                }
                if idx >= end {
                    break;
                }
                tokens += 1;
                while idx < end && line[idx] != b' ' && line[idx] != b'\t' {
                    idx += 1;
                }
            }
            tokens > 2
        };
        let is_bare_dollar_colon_header = |line: &[u8], cursor: usize, doc_offset: usize| -> bool {
            if line.get(cursor) != Some(&b'$') || line.get(cursor + 1) != Some(&b':') {
                return false;
            }
            let end = doc_offset.min(line.len());
            let mut idx = cursor + 2;
            while idx < end {
                let b = line[idx];
                if b != b' ' && b != b'\t' {
                    return false;
                }
                idx += 1;
            }
            true
        };
        let dollar_colon_line_has_inline_field = |line: &[u8], cursor: usize| -> bool {
            if line.get(cursor) != Some(&b'$') || line.get(cursor + 1) != Some(&b':') {
                return false;
            }
            let mut idx = cursor + 2;
            while idx < line.len() && (line[idx] == b' ' || line[idx] == b'\t') {
                idx += 1;
            }
            if idx >= line.len() {
                return false;
            }
            !(line.get(idx) == Some(&b':') && line.get(idx + 1) == Some(&b':'))
        };
        let dollar_percent_line_has_inline_branch = |line: &[u8], cursor: usize| -> bool {
            if line.get(cursor) != Some(&b'$') || line.get(cursor + 1) != Some(&b'%') {
                return false;
            }
            let mut idx = cursor + 2;
            while idx < line.len() && (line[idx] == b' ' || line[idx] == b'\t') {
                idx += 1;
            }
            if idx >= line.len() {
                return false;
            }
            !(line.get(idx) == Some(&b':') && line.get(idx + 1) == Some(&b':'))
        };
        let dollar_colon_line_has_nested_header = |line: &[u8], cursor: usize| -> bool {
            if line.get(cursor) != Some(&b'$') || line.get(cursor + 1) != Some(&b':') {
                return false;
            }
            let mut idx = cursor + 2;
            while idx < line.len() && (line[idx] == b' ' || line[idx] == b'\t') {
                idx += 1;
            }
            line.get(idx) == Some(&b'$') && line.get(idx + 1) == Some(&b':')
        };
        let is_semicolon_tilde_line = |line: &[u8], cursor: usize| -> bool {
            line.get(cursor) == Some(&b';') && line.get(cursor + 1) == Some(&b'~')
        };
        let start_has_same_indent_continuation = {
            if start_line_end >= bytes.len() {
                false
            } else {
                let mut next_start = start_line_end + 1;
                let mut same_indent = false;
                while next_start < bytes.len() {
                    let next_end = bytes[next_start..]
                        .iter()
                        .position(|&b| b == b'\n')
                        .map_or(bytes.len(), |idx| next_start + idx);
                    let next_line = &bytes[next_start..next_end];
                    let mut next_cursor = 0;
                    while next_cursor < next_line.len()
                        && (next_line[next_cursor] == b' ' || next_line[next_cursor] == b'\t')
                    {
                        next_cursor += 1;
                    }
                    if next_cursor == next_line.len() {
                        next_start = next_end + 1;
                        continue;
                    }
                    if doccord_info(next_line, next_cursor, 2).is_some() {
                        next_start = next_end + 1;
                        continue;
                    }
                    same_indent = next_cursor == start_cursor;
                    break;
                }
                same_indent
            }
        };
        let start_is_doc_line = doccord_info(start_line, start_cursor, 2).is_some();
        let start_is_tilde = bytes.get(lead) == Some(&b'~');
        let start_is_dollar = bytes.get(lead) == Some(&b'$');
        let start_is_tilde_hint =
            matches!(bytes.get(lead), Some(b'~')) && matches!(bytes.get(lead + 1), Some(b'_'));
        let start_is_tilde_slog =
            matches!(bytes.get(lead), Some(b'~')) && matches!(bytes.get(lead + 1), Some(b'>'));
        let start_is_tilde_print =
            matches!(bytes.get(lead), Some(b'~')) && matches!(bytes.get(lead + 1), Some(b'&'));
        let start_is_tilde_plus =
            matches!(bytes.get(lead), Some(b'~')) && matches!(bytes.get(lead + 1), Some(b'+'));
        let start_is_tilde_percent =
            matches!(bytes.get(lead), Some(b'~')) && matches!(bytes.get(lead + 1), Some(b'%'));
        let start_is_tilde_slash =
            matches!(bytes.get(lead), Some(b'~')) && matches!(bytes.get(lead + 1), Some(b'/'));
        let start_is_caret = bytes.get(lead) == Some(&b'^');
        let start_is_caret_plus = start_is_caret && start_line.get(start_cursor + 1) == Some(&b'+');
        let start_is_question = bytes.get(lead) == Some(&b'?');
        let start_is_question_tilde = start_is_question && bytes.get(lead + 1) == Some(&b'~');
        let start_is_equals = bytes.get(lead) == Some(&b'=');
        let start_is_percent_caret =
            bytes.get(lead) == Some(&b'%') && bytes.get(lead + 1) == Some(&b'^');
        let start_is_equals_slash =
            start_is_equals && start_line.get(start_cursor + 1) == Some(&b'/');
        let start_is_equals_dot =
            start_is_equals && start_line.get(start_cursor + 1) == Some(&b'.');
        let start_is_equals_caret =
            start_is_equals && start_line.get(start_cursor + 1) == Some(&b'^');
        let start_is_bar_percent =
            bytes.get(lead) == Some(&b'|') && bytes.get(lead + 1) == Some(&b'%');
        let start_is_colon_hep =
            bytes.get(lead) == Some(&b':') && bytes.get(lead + 1) == Some(&b'-');
        let start_is_colon_rune =
            bytes.get(lead) == Some(&b':') && bytes.get(lead + 1) != Some(&b':');
        let start_is_dollar_percent =
            bytes.get(lead) == Some(&b'$') && bytes.get(lead + 1) == Some(&b'%');
        let start_is_dollar_plus =
            bytes.get(lead) == Some(&b'$') && bytes.get(lead + 1) == Some(&b'+');
        let start_is_dollar_paren =
            bytes.get(lead) == Some(&b'$') && bytes.get(lead + 1) == Some(&b'(');
        let start_is_dollar_colon =
            bytes.get(lead) == Some(&b'$') && bytes.get(lead + 1) == Some(&b':');
        let start_is_equals_bar = start_is_equals && bytes.get(lead + 1) == Some(&b'|');
        let start_inline_doc_offset = inline_doc_offset(start_line)
            .and_then(|(has_content, offset)| has_content.then_some(offset));
        let start_has_inline_doc = start_inline_doc_offset.is_some();
        let start_inline_doc_is_heading = start_inline_doc_offset
            .map(|offset| inline_doc_is_heading(start_line, offset))
            .unwrap_or(false);
        let start_is_branch_tag = is_branch_tag_line(start_line, start_cursor);
        let start_is_bar = start_line.get(start_cursor) == Some(&b'|');
        let start_is_bar_cab = start_is_bar && start_line.get(start_cursor + 1) == Some(&b'_');
        let start_is_plus_header = is_plus_header_line(start_line, start_cursor);
        let start_is_bar_gate_sample = is_bar_gate_sample_line(start_line, start_cursor);
        let start_is_bar_equals = start_is_bar && start_line.get(start_cursor + 1) == Some(&b'=');
        let start_is_bar_star = start_is_bar && start_line.get(start_cursor + 1) == Some(&b'*');
        let start_is_bar_hep = start_is_bar && start_line.get(start_cursor + 1) == Some(&b'-');
        let start_is_underscore = bytes.get(lead) == Some(&b'_');
        let start_is_backtick = bytes.get(lead) == Some(&b'`');
        let start_under_dollar_colon = doc_line_under_dollar_colon_header(line_start, start_cursor);
        let start_equals_slash_name = if start_is_equals_slash {
            let mut idx = start_cursor + 2;
            while idx < start_line.len() && (start_line[idx] == b' ' || start_line[idx] == b'\t') {
                idx += 1;
            }
            let start_idx = idx;
            while idx < start_line.len() {
                let b = start_line[idx];
                if b.is_ascii_alphanumeric() || b == b'-' {
                    idx += 1;
                } else {
                    break;
                }
            }
            if idx > start_idx {
                Some(&start_line[start_idx..idx])
            } else {
                None
            }
        } else {
            None
        };
        if start_is_caret && start_has_inline_doc && line_start > 0 {
            let prev_end = line_start.saturating_sub(1);
            let mut prev_start = prev_end;
            while prev_start > 0 && bytes[prev_start - 1] != b'\n' {
                prev_start -= 1;
            }
            let prev_line = &bytes[prev_start..prev_end];
            let mut prev_cursor = 0;
            while prev_cursor < prev_line.len()
                && (prev_line[prev_cursor] == b' ' || prev_line[prev_cursor] == b'\t')
            {
                prev_cursor += 1;
            }
            if prev_cursor == start_cursor
                && is_bar_gate_sample_line(prev_line, prev_cursor)
                && matches!(inline_doc_offset(prev_line), Some((true, _)))
            {
                if let Some((_has_content, doc_offset)) = inline_doc_offset(prev_line) {
                    return prev_start + doc_offset;
                }
            }
        }
        let start_question_header_context =
            if start_is_question && line_start > 0 && start_cursor > 0 {
                let mut scan_end = line_start.saturating_sub(1);
                let mut found = false;
                while scan_end > 0 {
                    let mut scan_start = scan_end;
                    while scan_start > 0 && bytes[scan_start - 1] != b'\n' {
                        scan_start -= 1;
                    }
                    let scan_line = &bytes[scan_start..scan_end];
                    let mut scan_cursor = 0;
                    while scan_cursor < scan_line.len()
                        && (scan_line[scan_cursor] == b' ' || scan_line[scan_cursor] == b'\t')
                    {
                        scan_cursor += 1;
                    }
                    if scan_cursor == scan_line.len() {
                        scan_end = scan_start.saturating_sub(1);
                        continue;
                    }
                    if doccord_info(scan_line, scan_cursor, 2).is_some() {
                        scan_end = scan_start.saturating_sub(1);
                        continue;
                    }
                    found = scan_cursor == start_cursor
                        && is_question_header_line(scan_line, scan_cursor);
                    break;
                }
                found
            } else {
                false
            };
        let bar_hep_has_header_context = if start_is_bar_hep && line_start > 0 && start_cursor > 0 {
            let mut scan_end = line_start.saturating_sub(1);
            let mut found = false;
            while scan_end > 0 {
                let mut scan_start = scan_end;
                while scan_start > 0 && bytes[scan_start - 1] != b'\n' {
                    scan_start -= 1;
                }
                let scan_line = &bytes[scan_start..scan_end];
                let mut scan_cursor = 0;
                while scan_cursor < scan_line.len()
                    && (scan_line[scan_cursor] == b' ' || scan_line[scan_cursor] == b'\t')
                {
                    scan_cursor += 1;
                }
                if scan_cursor == scan_line.len() {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if doccord_info(scan_line, scan_cursor, 2).is_some() {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                found = scan_cursor < start_cursor && is_plus_header_line(scan_line, scan_cursor);
                break;
            }
            found
        } else {
            false
        };
        let allow_doc = match bytes[lead] {
            b'|' => {
                let next = bytes.get(lead + 1).copied().unwrap_or(0);
                if next == b'-' {
                    bar_hep_has_header_context
                } else {
                    !matches!(next, b'(')
                }
            }
            b':' => true,
            b'=' => {
                matches!(start_line.get(start_cursor + 1), Some(b'/') | Some(b'.'))
                    || start_under_dollar_colon
            }
            b'$' => true,
            b'%' => !matches!(bytes.get(lead + 1), Some(b'=')),
            _ => true,
        };
        if !allow_doc {
            return start;
        }
        if start_is_face_line {
            return start;
        }
        if start_is_doc_line && line_start > 0 {
            let prev_end = line_start.saturating_sub(1);
            let mut prev_start = prev_end;
            while prev_start > 0 && bytes[prev_start - 1] != b'\n' {
                prev_start -= 1;
            }
            let prev_line = &bytes[prev_start..prev_end];
            if let Some((_has_content, doc_offset)) = inline_doc_offset(prev_line) {
                return prev_start + doc_offset;
            }
        }

        let mut idx = line_start;
        let start_line_start = line_start;
        let mut saw_doc = false;
        let mut saw_non_doc_comment_before_doc = false;
        let mut saw_doc_content = false;
        let mut doc_anchor_inline = false;
        let mut last_non_inline_blank = false;
        let mut doc_top_blank = false;
        let mut doc_start = None;
        let mut doc_start_by_indent: Vec<(usize, usize)> = Vec::new();
        let mut doc_label_start_by_indent: Vec<(usize, usize)> = Vec::new();
        let mut doc_indent = None;
        let mut doc_min_content_indent: Option<usize> = None;
        let mut doc_second_min_content_indent: Option<usize> = None;
        let mut doc_max_content_indent: Option<usize> = None;
        let mut doc_deep_indent = false;
        let mut suppress_doc_block = false;
        let mut doc_content_lines = 0usize;
        let mut doc_any_content = false;
        let mut doc_blank_after_content = false;
        let mut doc_blank_after_content_lines = 0usize;
        let mut doc_content_after_blank = false;
        let mut doc_simple_heading = false;
        let mut doc_single_word_heading = false;
        let mut doc_heading_block = false;
        let mut doc_block_has_rune_like = false;
        let mut doc_block_has_non_gate_rune_like = false;
        let mut doc_block_has_label = false;
        let mut doc_block_has_non_label = false;
        let mut doc_block_has_non_label_after_blank = false;
        let mut doc_under_plus_header_block = false;
        let mut doc_under_caret_bar_block = false;
        let mut saw_tilde_slash = false;
        let mut doc_has_triple_quote = false;
        let mut doc_mentions_start_binding = false;
        let mut doc_under_dollar_colon = false;
        let mut doc_under_equals_slash = false;
        let doc_block_has_text = {
            let mut scan_end = line_start.saturating_sub(1);
            let mut has_text = false;
            while scan_end > 0 {
                let mut scan_start = scan_end;
                while scan_start > 0 && bytes[scan_start - 1] != b'\n' {
                    scan_start -= 1;
                }
                let line = &bytes[scan_start..scan_end];
                let mut cursor = 0;
                while cursor < line.len() && (line[cursor] == b' ' || line[cursor] == b'\t') {
                    cursor += 1;
                }
                if cursor == line.len() {
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                if let Some((has_content, doc_offset)) = doccord_info(line, cursor, 2) {
                    if has_content && !doc_line_starts_with_rune_like(line, doc_offset) {
                        has_text = true;
                    }
                    scan_end = scan_start.saturating_sub(1);
                    continue;
                }
                break;
            }
            has_text
        };
        loop {
            if idx == 0 {
                if start_is_backtick || start_is_tilde_slog || start_is_tilde_print {
                    return start;
                }
                if start_is_tilde_plus
                    && doc_content_lines <= 1
                    && !doc_blank_after_content
                    && !doc_top_blank
                {
                    return start;
                }
                if saw_doc && !doc_anchor_inline {
                    let has_leading_blank_doc = doc_blank_after_content && !doc_content_after_blank;
                    let start_allows_blank_doc = (start_is_dollar && !start_is_dollar_colon)
                        || start_is_bar_percent
                        || start_is_underscore
                        || doc_under_dollar_colon;
                    let doc_has_mixed_indent = matches!(
                        (doc_min_content_indent, doc_max_content_indent),
                        (Some(min_indent), Some(max_indent)) if max_indent > min_indent
                    );
                    if (doc_top_blank || has_leading_blank_doc)
                        && !start_allows_blank_doc
                        && !doc_has_mixed_indent
                    {
                        return start;
                    }
                }
                if saw_doc_content {
                    if let Some(indent) = doc_indent {
                        if indent > 0 && indent == start_cursor {
                            if doc_top_blank && doc_content_lines <= 1 {
                                return start;
                            }
                            if start_is_question_tilde
                                || (start_is_question && start_question_header_context)
                            {
                                return start;
                            }
                            if start_is_equals && doc_top_blank && doc_content_lines <= 1 {
                                return start;
                            }
                            if start_is_equals_bar {
                                return start;
                            }
                            return doc_start.unwrap_or(start);
                        }
                    }
                    if doc_anchor_inline {
                        return doc_start.unwrap_or(start);
                    }
                }
                return start;
            }
            if bytes[idx - 1] != b'\n' {
                #[cfg(test)]
                if debug_expand {
                    eprintln!(
                        "expand-gap return saw_doc_content={} doc_start={:?} doc_content_lines={} blank_after_content={} content_after_blank={} labels={} non_labels={} non_labels_after_blank={}",
                        saw_doc_content,
                        doc_start,
                        doc_content_lines,
                        doc_blank_after_content,
                        doc_content_after_blank,
                        doc_block_has_label,
                        doc_block_has_non_label,
                        doc_block_has_non_label_after_blank
                    );
                }
                return if saw_doc_content {
                    doc_start.unwrap_or(idx)
                } else {
                    start
                };
            }

            let line_end = idx - 1;
            let mut prev_start = line_end;
            while prev_start > 0 && bytes[prev_start - 1] != b'\n' {
                prev_start -= 1;
            }
            let line = &bytes[prev_start..line_end];

            let mut cursor = 0;
            while cursor < line.len() && (line[cursor] == b' ' || line[cursor] == b'\t') {
                cursor += 1;
            }
            if cursor == line.len() {
                return if saw_doc_content {
                    doc_start.unwrap_or(idx)
                } else {
                    start
                };
            }
            if cursor == start_cursor && is_tilde_slash_line(line, cursor) {
                saw_tilde_slash = true;
            }

            let allow_inline = start_is_doc_line
                || (saw_doc && doc_indent == Some(start_cursor))
                || start_cursor > cursor
                || (start_cursor == cursor
                    && (is_branch_line(line, cursor)
                        || is_branch_tag_line(line, cursor)
                        || (start_is_branch_tag && is_dollar_question_line(line, cursor))
                        || (start_is_caret && is_bar_gate_sample_line(line, cursor))
                        || (start_is_question_tilde && is_caret_hep_line(line, cursor))))
                || (start_cursor < cursor && start_is_colon_rune && is_branch_line(line, cursor))
                || (start_cursor < cursor
                    && start_is_dollar_paren
                    && is_question_header_line(line, cursor));
            let parent_header_char =
                if start_cursor < cursor && start_is_colon_rune && is_branch_line(line, cursor) {
                    branch_line_parent_header_char(prev_start, cursor, start_cursor)
                } else {
                    None
                };
            let allow_compact_doc =
                doc_under_dollar_colon || doc_line_under_dollar_colon_header(prev_start, cursor);
            let doc_line_info = doccord_info(line, cursor, 2).or_else(|| {
                allow_compact_doc
                    .then(|| doccord_info(line, cursor, 0))
                    .flatten()
            });
            if doc_line_info.is_none() && doc_comment_offset(line, cursor).is_some() {
                if !saw_doc {
                    saw_non_doc_comment_before_doc = true;
                } else if doc_indent == Some(cursor) {
                    saw_doc = false;
                    saw_doc_content = false;
                    doc_any_content = false;
                    doc_content_lines = 0;
                    doc_blank_after_content = false;
                    doc_blank_after_content_lines = 0;
                    doc_content_after_blank = false;
                    doc_top_blank = false;
                    doc_indent = None;
                    doc_anchor_inline = false;
                    doc_start = None;
                    doc_deep_indent = false;
                    doc_simple_heading = false;
                    doc_single_word_heading = false;
                    doc_heading_block = false;
                    doc_block_has_rune_like = false;
                    doc_block_has_non_gate_rune_like = false;
                    doc_block_has_label = false;
                    doc_block_has_non_label = false;
                    doc_block_has_non_label_after_blank = false;
                    doc_under_plus_header_block = false;
                    doc_under_caret_bar_block = false;
                    doc_start_by_indent.clear();
                    doc_label_start_by_indent.clear();
                }
                idx = prev_start;
                continue;
            }
            let doc_kind = if let Some((has_content, doc_offset)) = doc_line_info {
                Some((false, has_content, doc_offset))
            } else if allow_inline {
                inline_doc_offset(line).and_then(|(has_content, offset)| {
                    if !has_content && !(start_is_doc_line || saw_doc) {
                        None
                    } else if start_cursor > cursor
                        && start_cursor - cursor > 4
                        && !is_plus_header_line(line, cursor)
                        && !(is_equals_plus_line(line, cursor)
                            && equals_plus_has_colon_star(line, cursor, offset))
                    {
                        None
                    } else if start_cursor > cursor
                        && is_equals_header_line(line, cursor)
                        && !(is_equals_plus_line(line, cursor)
                            && equals_plus_has_colon_star(line, cursor, offset))
                    {
                        None
                    } else if start_cursor > cursor && is_question_header_line(line, cursor) {
                        None
                    } else if start_is_colon_hep && start_cursor > cursor {
                        None
                    } else if start_cursor > cursor && is_colon_caret_line(line, cursor) {
                        None
                    } else if start_cursor > cursor && is_colon_tilde_line(line, cursor) {
                        None
                    } else if start_cursor > cursor
                        && line.get(cursor) == Some(&b':')
                        && line.get(cursor + 1) != Some(&b':')
                        && start_has_inline_doc
                        && !start_inline_doc_is_heading
                    {
                        None
                    } else if parent_header_char == Some(b'?') {
                        None
                    } else if parent_header_char.is_some() && parent_header_char != Some(b'^') {
                        None
                    } else if parent_header_char == Some(b'^')
                        && !branch_line_has_multiple_tags(line, cursor, offset)
                    {
                        None
                    } else if start_cursor > cursor && is_percent_header_line(line, cursor) {
                        None
                    } else if start_cursor > cursor && is_plus_header_line(line, cursor) && !saw_doc
                    {
                        None
                    } else if start_cursor > cursor
                        && is_dollar_percent_line(line, cursor)
                        && dollar_percent_line_has_inline_branch(line, cursor)
                        && !inline_doc_is_heading(line, offset)
                    {
                        None
                    } else if start_cursor > cursor
                        && is_dollar_header_line(line, cursor)
                        && dollar_colon_line_has_inline_field(line, cursor)
                        && !inline_doc_is_heading(line, offset)
                    {
                        None
                    } else if start_cursor > cursor
                        && is_dollar_header_line(line, cursor)
                        && !inline_doc_is_heading(line, offset)
                        && !is_bare_dollar_colon_header(line, cursor, offset)
                    {
                        None
                    } else if start_cursor > cursor && is_semicolon_tilde_line(line, cursor) {
                        None
                    } else if start_is_question
                        && start_cursor == cursor
                        && is_equals_header_line(line, cursor)
                    {
                        None
                    } else if start_has_inline_doc
                        && start_cursor == cursor
                        && !is_branch_line(line, cursor)
                        && !is_branch_tag_line(line, cursor)
                        && !(start_is_caret && is_bar_gate_sample_line(line, cursor))
                        && !(start_is_question_tilde && is_caret_hep_line(line, cursor))
                    {
                        None
                    } else if start_cursor == cursor
                        && is_branch_line(line, cursor)
                        && branch_line_has_multiple_tags(line, cursor, offset)
                        && !start_is_colon_rune
                    {
                        None
                    } else if start_cursor == cursor
                        && is_branch_line(line, cursor)
                        && !start_is_colon_rune
                        && !inline_doc_starts_with_branch_marker(line, offset)
                    {
                        None
                    } else if start_cursor > cursor
                        && is_plus_header_line(line, cursor)
                        && (plus_header_has_inline_body(line, cursor, offset)
                            || start_is_bar
                            || start_is_question
                            || start_is_equals
                            || !start_is_dollar_percent
                            || is_type_header_line(line, cursor))
                        && !start_is_doc_line
                        && !start_is_tilde
                    {
                        None
                    } else {
                        Some((true, has_content, offset))
                    }
                })
            } else {
                None
            };

            if let Some((is_inline, has_content, doc_offset)) = doc_kind {
                if start_is_question && is_inline && saw_doc && cursor > start_cursor {
                    return start;
                }
                if !is_inline {
                    if let Some((prev_cursor, prev_line)) = prev_non_doc_line(prev_start) {
                        if prev_cursor <= cursor
                            && prev_line.get(prev_cursor) == Some(&b'=')
                            && prev_line.get(prev_cursor + 1) == Some(&b'/')
                        {
                            doc_under_equals_slash = true;
                        }
                    }
                }
                if !is_inline && start_has_inline_doc && start_cursor == cursor {
                    if !doc_line_under_plus_header(prev_start, cursor)
                        && !doc_line_preceded_by_tilde_percent(prev_start, cursor)
                    {
                        return start;
                    }
                }
                let mut current_content_indent = None;
                let mut line_is_label = false;
                let mut line_simple_heading = false;
                let mut line_single_word_heading = false;
                if has_content {
                    if !is_inline {
                        line_is_label = doc_line_is_label(line, doc_offset);
                        if line_is_label && doc_content_indent(line, doc_offset) >= 4 {
                            line_is_label = false;
                        }
                    }
                    if let Some(name) = start_equals_slash_name {
                        if doc_line_mentions_name(line, doc_offset, name) {
                            doc_mentions_start_binding = true;
                        }
                    }
                }
                if !is_inline && has_content {
                    if line_is_label {
                        doc_block_has_label = true;
                    } else {
                        doc_block_has_non_label = true;
                    }
                    if doc_blank_after_content && !line_is_label {
                        doc_block_has_non_label_after_blank = true;
                    }
                }
                if !is_inline && has_content {
                    line_simple_heading = doc_line_is_simple_heading(line, doc_offset);
                    line_single_word_heading = doc_line_is_single_word_heading(line, doc_offset);
                    doc_content_lines += 1;
                    if doc_content_lines == 1 {
                        doc_simple_heading = line_simple_heading;
                        doc_single_word_heading = line_single_word_heading;
                    } else {
                        doc_simple_heading = false;
                        doc_single_word_heading = false;
                    }
                    doc_any_content = true;
                    if doc_blank_after_content {
                        doc_content_after_blank = true;
                    }
                    if (doc_under_dollar_colon || start_under_dollar_colon)
                        && line_simple_heading
                        && doc_blank_after_content
                    {
                        doc_heading_block = true;
                        saw_doc_content = false;
                        doc_anchor_inline = false;
                        doc_start = None;
                        doc_start_by_indent.clear();
                        doc_label_start_by_indent.clear();
                    }
                    let content_indent = doc_content_indent(line, doc_offset);
                    current_content_indent = Some(content_indent);
                    if let Some(min_indent) = doc_min_content_indent {
                        if content_indent < min_indent {
                            doc_second_min_content_indent = Some(min_indent);
                            doc_min_content_indent = Some(content_indent);
                        } else if content_indent > min_indent {
                            doc_second_min_content_indent = Some(
                                doc_second_min_content_indent
                                    .map_or(content_indent, |v| v.min(content_indent)),
                            );
                        }
                    } else {
                        doc_min_content_indent = Some(content_indent);
                    }
                    doc_max_content_indent = Some(
                        doc_max_content_indent.map_or(content_indent, |v| v.max(content_indent)),
                    );
                    if let (Some(min_indent), Some(max_indent)) =
                        (doc_min_content_indent, doc_max_content_indent)
                    {
                        if max_indent.saturating_sub(min_indent) >= 8 {
                            doc_deep_indent = true;
                        }
                    }
                    if doc_line_starts_with_triple_quote(line, doc_offset) {
                        doc_has_triple_quote = true;
                    }
                }
                if !has_content && !is_inline && doc_any_content {
                    doc_blank_after_content = true;
                    doc_blank_after_content_lines += 1;
                    if (doc_under_dollar_colon || start_under_dollar_colon)
                        && doc_single_word_heading
                        && doc_content_lines <= 1
                    {
                        saw_doc_content = false;
                        doc_anchor_inline = false;
                        doc_start = None;
                        doc_start_by_indent.clear();
                        doc_label_start_by_indent.clear();
                    }
                    if (doc_under_dollar_colon || start_under_dollar_colon)
                        && doc_blank_after_content_lines >= 1
                        && !doc_content_after_blank
                        && matches!(doc_max_content_indent, Some(indent) if indent < 4)
                    {
                        saw_doc_content = false;
                        doc_anchor_inline = false;
                        doc_start = None;
                        doc_start_by_indent.clear();
                        doc_label_start_by_indent.clear();
                    }
                    if (doc_under_dollar_colon || start_under_dollar_colon)
                        && doc_blank_after_content_lines >= 2
                        && !doc_content_after_blank
                    {
                        saw_doc_content = false;
                        doc_anchor_inline = false;
                        doc_start = None;
                        doc_start_by_indent.clear();
                        doc_label_start_by_indent.clear();
                    }
                }
                let doc_line_content_indent = if !is_inline && has_content {
                    Some(doc_content_indent(line, doc_offset))
                } else {
                    None
                };
                let mut anchorable = if is_inline {
                    has_content
                } else {
                    has_content && doc_line_anchorable(line, doc_offset)
                };
                if !is_inline
                    && (doc_under_dollar_colon || start_under_dollar_colon)
                    && matches!(doc_max_content_indent, Some(indent) if indent < 4)
                {
                    anchorable = false;
                }
                if doc_heading_block {
                    anchorable = false;
                }
                if is_inline
                    && anchorable
                    && is_bare_dollar_colon_header(line, cursor, doc_offset)
                    && !inline_doc_is_heading(line, doc_offset)
                    && !doc_indent.map_or(false, |indent| indent > cursor)
                {
                    anchorable = false;
                }
                if is_inline
                    && anchorable
                    && has_content
                    && start_is_branch_tag
                    && start_cursor == cursor
                    && is_branch_tag_line(line, cursor)
                    && !inline_doc_is_heading(line, doc_offset)
                    && prev_start != start_line_start
                {
                    anchorable = false;
                }
                let doc_line_is_question_greater = if !is_inline && has_content {
                    let content_indent = doc_content_indent(line, doc_offset);
                    let content_start = doc_offset + 2 + content_indent;
                    line.get(content_start) == Some(&b'?')
                        && line.get(content_start + 1) == Some(&b'>')
                } else {
                    false
                };
                let line_is_indented_heading = if !is_inline && has_content && !line_is_label {
                    match (current_content_indent, doc_min_content_indent) {
                        (Some(current_indent), Some(min_indent)) => current_indent > min_indent,
                        _ => false,
                    }
                } else {
                    false
                };
                if doc_line_is_question_greater {
                    anchorable = false;
                }
                let doc_line_is_rune_like =
                    !is_inline && has_content && doc_line_starts_with_rune_like(line, doc_offset);
                let doc_line_is_bar_gate_rune_like = if !is_inline && has_content {
                    let content_start =
                        doc_offset + 2 + doc_line_content_indent.unwrap_or_default();
                    line.get(content_start) == Some(&b'|')
                        && matches!(line.get(content_start + 1), Some(b':' | b'='))
                } else {
                    false
                };
                if doc_line_is_rune_like && !doc_block_has_text {
                    anchorable = false;
                }
                if doc_line_is_rune_like {
                    doc_block_has_rune_like = true;
                    if !doc_line_is_bar_gate_rune_like {
                        doc_block_has_non_gate_rune_like = true;
                    }
                }
                if !is_inline
                    && anchorable
                    && doc_block_has_rune_like
                    && !doc_line_is_rune_like
                    && doc_line_is_compact_plus_payload(line, doc_offset)
                {
                    anchorable = false;
                }
                #[cfg(test)]
                if debug_expand {
                    let (line_no, _) = self.line_col(prev_start);
                    let line_str = String::from_utf8_lossy(line);
                    eprintln!(
                        "expand-gap doc line {} {:?} inline={} content={} rune_like={} block_has_text={} anchorable={}",
                        line_no,
                        line_str,
                        is_inline,
                        has_content,
                        doc_line_is_rune_like,
                        doc_block_has_text,
                        anchorable
                    );
                }
                if !is_inline
                    && anchorable
                    && doc_line_preceded_by_question_greater(prev_start, cursor)
                    && matches!(doc_line_content_indent, Some(indent) if indent < 4)
                {
                    anchorable = false;
                }
                let doc_under_plus_header =
                    !is_inline && doc_line_under_plus_header(prev_start, cursor);
                let doc_under_type_header =
                    !is_inline && doc_line_under_type_header(prev_start, cursor);
                let doc_under_dollar_colon_header =
                    !is_inline && doc_line_under_dollar_colon_header(prev_start, cursor);
                let doc_under_tilde_percent_in_arm =
                    !is_inline && doc_line_under_tilde_percent_in_arm(prev_start, cursor);
                let doc_under_question_header =
                    !is_inline && doc_line_under_question_header(prev_start, cursor);
                let plus_header_has_doc_content =
                    !is_inline && doc_line_plus_header_has_doc_content(prev_start, cursor);
                let plus_header_has_inline_doc =
                    !is_inline && doc_line_plus_header_has_inline_doc(prev_start, cursor);
                if doc_under_plus_header {
                    doc_under_plus_header_block = true;
                }
                let doc_under_caret_bar = !is_inline
                    && prev_non_doc_line(prev_start).map_or(false, |(prev_cursor, prev_line)| {
                        prev_cursor == cursor && is_caret_bar_line(prev_line, prev_cursor)
                    });
                if doc_under_caret_bar {
                    doc_under_caret_bar_block = true;
                }
                if doc_under_plus_header
                    && start_is_bar_gate_sample
                    && doc_block_has_label
                    && !doc_block_has_non_label
                {
                    anchorable = false;
                }
                if doc_under_plus_header
                    && start_is_bar_gate_sample
                    && doc_block_has_label
                    && doc_content_after_blank
                    && !doc_block_has_non_label_after_blank
                    && !doc_block_has_non_label
                {
                    anchorable = false;
                }
                if doc_deep_indent
                    && start_is_bar_gate_sample
                    && !doc_under_plus_header
                    && !doc_under_type_header
                    && !doc_under_question_header
                {
                    suppress_doc_block = true;
                }
                if doc_under_dollar_colon_header {
                    doc_under_dollar_colon = true;
                }
                if (doc_under_dollar_colon_header || start_under_dollar_colon)
                    && doc_simple_heading
                    && doc_content_lines <= 1
                    && doc_top_blank
                {
                    anchorable = false;
                }
                if suppress_doc_block {
                    saw_doc = false;
                    saw_doc_content = false;
                    doc_any_content = false;
                    doc_content_lines = 0;
                    doc_blank_after_content = false;
                    doc_blank_after_content_lines = 0;
                    doc_content_after_blank = false;
                    doc_top_blank = false;
                    doc_indent = None;
                    doc_anchor_inline = false;
                    doc_start = None;
                    doc_deep_indent = false;
                    doc_simple_heading = false;
                    doc_single_word_heading = false;
                    doc_heading_block = false;
                    doc_block_has_rune_like = false;
                    doc_block_has_non_gate_rune_like = false;
                    doc_block_has_label = false;
                    doc_block_has_non_label = false;
                    doc_block_has_non_label_after_blank = false;
                    doc_under_plus_header_block = false;
                    doc_under_caret_bar_block = false;
                    suppress_doc_block = false;
                    doc_start_by_indent.clear();
                    doc_label_start_by_indent.clear();
                    idx = prev_start;
                    continue;
                }
                if !is_inline && saw_non_doc_comment_before_doc {
                    anchorable = false;
                }
                let mut allow_trailing_doc = false;
                if start_is_equals_slash && doc_line_preceded_by_equals_slash(prev_start, cursor) {
                    allow_trailing_doc = true;
                }
                if start_is_underscore && doc_line_preceded_by_bar_dollar(prev_start, cursor) {
                    allow_trailing_doc = true;
                }
                if doc_line_preceded_by_bar_dollar(prev_start, cursor) {
                    allow_trailing_doc = true;
                }
                if doc_block_between_question_branches(prev_start, cursor) {
                    allow_trailing_doc = true;
                }
                if doc_line_preceded_by_tilde_percent(prev_start, cursor) {
                    if !doc_line_under_tilde_percent_in_arm(prev_start, cursor)
                        || matches!(doc_line_content_indent, Some(indent) if indent >= 4)
                    {
                        allow_trailing_doc = true;
                    }
                }
                if doc_line_preceded_by_tilde_slash(prev_start, cursor) {
                    allow_trailing_doc = true;
                }
                if start_is_face_binding && doc_under_dollar_colon_header {
                    allow_trailing_doc = true;
                }
                if start_is_question {
                    let content_indent = doc_content_indent(line, doc_offset);
                    let content_start = doc_offset + 2 + content_indent;
                    if line.get(content_start) == Some(&b'[') {
                        allow_trailing_doc = true;
                    }
                }
                if start_is_bar_gate_sample {
                    if let Some((prev_cursor, _)) = prev_non_doc_line(prev_start) {
                        if prev_cursor < cursor {
                            allow_trailing_doc = true;
                        }
                    }
                }
                if let Some((prev_cursor, prev_line)) = prev_non_doc_line(prev_start) {
                    if prev_cursor == cursor
                        && prev_line.get(prev_cursor) == Some(&b'?')
                        && prev_line.get(prev_cursor + 1) == Some(&b'>')
                    {
                        allow_trailing_doc = true;
                    }
                }
                if start_is_term_line {
                    if let Some((prev_cursor, prev_line)) = prev_non_doc_line(prev_start) {
                        if prev_cursor == cursor && is_caret_hep_line(prev_line, prev_cursor) {
                            allow_trailing_doc = true;
                        }
                    }
                }
                if !is_inline
                    && anchorable
                    && doc_line_is_trailing_comment(prev_start, cursor)
                    && !allow_trailing_doc
                {
                    anchorable = false;
                }
                if doc_under_plus_header && start_is_bar_star {
                    anchorable = false;
                }
                if doc_under_plus_header && start_is_bar_equals {
                    if doc_content_after_blank {
                        let tilde_slash_header =
                            doc_line_preceded_by_tilde_slash(prev_start, cursor);
                        if doc_block_has_label && !tilde_slash_header && !plus_header_has_inline_doc
                        {
                            anchorable = false;
                        } else if doc_block_has_non_label_after_blank
                            && !tilde_slash_header
                            && !plus_header_has_inline_doc
                        {
                            anchorable = false;
                        }
                    } else if !doc_blank_after_content && !doc_content_after_blank && !doc_top_blank
                    {
                        let tilde_slash_header =
                            doc_line_preceded_by_tilde_slash(prev_start, cursor);
                        if doc_content_lines <= 1
                            && !plus_header_has_inline_doc
                            && !tilde_slash_header
                        {
                            anchorable = false;
                        }
                    }
                }
                if doc_under_plus_header
                    && start_is_tilde_slash
                    && matches!(doc_line_content_indent, Some(indent) if indent < 4)
                {
                    anchorable = false;
                }
                if doc_under_type_header && start_is_dollar_colon {
                    anchorable = false;
                }
                if doc_under_type_header
                    && doc_content_lines <= 1
                    && !doc_blank_after_content
                    && !doc_top_blank
                    && !(start_is_dollar && !start_is_dollar_colon)
                {
                    anchorable = false;
                }
                if !is_inline
                    && anchorable
                    && doc_line_preceded_by_question_greater(prev_start, cursor)
                    && start_is_question
                {
                    anchorable = false;
                }
                if doc_under_tilde_percent_in_arm && start_is_bar_percent {
                    if matches!(doc_line_content_indent, Some(indent) if indent < 4) {
                        anchorable = false;
                    }
                }
                if doc_under_question_header {
                    anchorable = false;
                }
                if start_is_dollar_plus {
                    anchorable = false;
                }
                if !is_inline
                    && anchorable
                    && line_is_label
                    && (start_is_equals || start_is_face_binding)
                {
                    if let Some(min_indent) = doc_min_content_indent {
                        if current_content_indent == Some(min_indent) {
                            anchorable = false;
                        }
                    } else {
                        anchorable = false;
                    }
                }
                if !is_inline && has_content && line_is_label {
                    if let Some(content_indent) = current_content_indent {
                        if let Some(pos) = doc_label_start_by_indent
                            .iter()
                            .position(|(indent, _)| *indent == content_indent)
                        {
                            doc_label_start_by_indent[pos].1 = prev_start + doc_offset;
                        } else {
                            doc_label_start_by_indent
                                .push((content_indent, prev_start + doc_offset));
                        }
                    }
                }
                if !is_inline
                    && anchorable
                    && doc_line_preceded_by_outdent_terminator(prev_start, cursor)
                {
                    if !(start_is_bar_percent || start_is_plus_header) {
                        anchorable = false;
                    }
                }
                if !is_inline
                    && anchorable
                    && doc_deep_indent
                    && start_is_bar_gate_sample
                    && !doc_under_plus_header
                    && !doc_under_type_header
                    && !doc_under_question_header
                {
                    anchorable = false;
                }
                if start_is_equals_slash
                    && !is_inline
                    && has_content
                    && start_equals_slash_name.is_none()
                {
                    anchorable = false;
                }
                if is_inline
                    && has_content
                    && start_has_inline_doc
                    && start_is_branch_tag
                    && start_cursor == cursor
                    && is_branch_tag_line(line, cursor)
                    && !inline_doc_is_heading(line, doc_offset)
                {
                    return start;
                }
                let was_saw_doc = saw_doc;
                if start_is_caret && is_inline && is_plus_header_line(line, cursor) {
                    return start;
                }
                if start_is_caret_plus && is_inline && is_bar_gate_sample_line(line, cursor) {
                    return start;
                }
                if start_is_tilde_hint {
                    return start;
                }
                if !has_content && !is_inline && !doc_any_content && !doc_anchor_inline {
                    if !saw_doc {
                        doc_top_blank = true;
                    }
                    saw_doc = true;
                    last_non_inline_blank = true;
                    if doc_indent.is_none() {
                        doc_indent = Some(cursor);
                    }
                    idx = prev_start;
                    continue;
                }
                if is_inline && !has_content && doc_anchor_inline {
                    idx = prev_start;
                    continue;
                }
                let allow_plus_header_inline_doc = start_cursor > cursor
                    && is_arm_header_line(line, cursor)
                    && !plus_header_has_inline_body(line, cursor, doc_offset)
                    && !start_is_bar
                    && !start_is_question
                    && !start_is_equals
                    && start_is_dollar_percent;
                let allow_equals_plus_tuple_inline_doc = start_cursor > cursor
                    && is_equals_plus_line(line, cursor)
                    && equals_plus_has_colon_star(line, cursor, doc_offset);
                if saw_doc && !doc_anchor_inline && is_inline {
                    if !has_content {
                        return start;
                    }
                    let inline_header_indented_block = (is_arm_header_line(line, cursor)
                        && doc_indent.map_or(false, |indent| indent > cursor))
                        || (is_bare_dollar_colon_header(line, cursor, doc_offset)
                            && doc_indent.map_or(false, |indent| indent > cursor));
                    if last_non_inline_blank
                        && doc_indent.map_or(false, |indent| indent <= start_cursor)
                        && !allow_plus_header_inline_doc
                        && !allow_equals_plus_tuple_inline_doc
                        && !inline_header_indented_block
                    {
                        return start;
                    }
                    if !last_non_inline_blank
                        && !allow_plus_header_inline_doc
                        && !allow_equals_plus_tuple_inline_doc
                        && !inline_header_indented_block
                    {
                        return if saw_doc_content {
                            doc_start.unwrap_or(idx)
                        } else {
                            start
                        };
                    }
                }
                if start_is_tilde && is_inline && has_content {
                    return start;
                }
                if !has_content && !is_inline && doc_anchor_inline {
                    return if saw_doc_content {
                        doc_start.unwrap_or(idx)
                    } else {
                        start
                    };
                }
                if !is_inline {
                    last_non_inline_blank = !has_content;
                    if !was_saw_doc {
                        doc_top_blank = !has_content;
                    }
                    if doc_indent.is_none() {
                        doc_indent = Some(cursor);
                    }
                }
                #[cfg(test)]
                if debug_expand {
                    let line_str = String::from_utf8_lossy(line);
                    eprintln!(
                        "expand-gap doc line final {:?} anchorable={} has_content={} line_is_label={} doc_content_after_blank={} doc_block_has_non_label_after_blank={}",
                        line_str,
                        anchorable,
                        has_content,
                        line_is_label,
                        doc_content_after_blank,
                        doc_block_has_non_label_after_blank
                    );
                }
                if anchorable {
                    saw_doc_content = true;
                    doc_anchor_inline = is_inline;
                }
                saw_doc = true;
                if anchorable {
                    if let Some(content_indent) = current_content_indent {
                        if let Some(pos) = doc_start_by_indent
                            .iter()
                            .position(|(indent, _)| *indent == content_indent)
                        {
                            doc_start_by_indent[pos].1 = prev_start + doc_offset;
                        } else {
                            doc_start_by_indent.push((content_indent, prev_start + doc_offset));
                        }
                        if let (Some(min_indent), Some(max_indent)) =
                            (doc_min_content_indent, doc_max_content_indent)
                        {
                            if max_indent > min_indent {
                                let preferred_indent =
                                    doc_second_min_content_indent.unwrap_or(max_indent);
                                if let Some((_, offset)) = doc_start_by_indent
                                    .iter()
                                    .find(|(indent, _)| *indent == preferred_indent)
                                {
                                    doc_start = Some(*offset);
                                } else if let Some((_, offset)) = doc_label_start_by_indent
                                    .iter()
                                    .find(|(indent, _)| *indent == preferred_indent)
                                {
                                    doc_start = Some(*offset);
                                }
                            } else {
                                doc_start = Some(prev_start + doc_offset);
                            }
                        } else {
                            doc_start = Some(prev_start + doc_offset);
                        }
                    } else {
                        doc_start = Some(prev_start + doc_offset);
                    }
                }
                if !doc_anchor_inline {
                    if let (Some(min_indent), Some(max_indent)) =
                        (doc_min_content_indent, doc_max_content_indent)
                    {
                        if max_indent > min_indent {
                            let preferred_indent =
                                doc_second_min_content_indent.unwrap_or(max_indent);
                            if let Some((_, offset)) = doc_start_by_indent
                                .iter()
                                .find(|(indent, _)| *indent == preferred_indent)
                            {
                                doc_start = Some(*offset);
                            } else if let Some((_, offset)) = doc_label_start_by_indent
                                .iter()
                                .find(|(indent, _)| *indent == preferred_indent)
                            {
                                doc_start = Some(*offset);
                            }
                        }
                    }
                }
                if is_inline && has_content && start_cursor < cursor && is_branch_line(line, cursor)
                {
                    return doc_start.unwrap_or(start);
                }
                if is_inline
                    && has_content
                    && start_cursor == cursor
                    && is_branch_line(line, cursor)
                    && inline_doc_starts_with_branch_marker(line, doc_offset)
                {
                    return doc_start.unwrap_or(start);
                }
                if is_inline
                    && has_content
                    && start_cursor == cursor
                    && start_is_branch_tag
                    && is_branch_tag_line(line, cursor)
                    && inline_doc_is_heading(line, doc_offset)
                {
                    return doc_start.unwrap_or(start);
                }
                if is_inline && !was_saw_doc && start_cursor > cursor {
                    return doc_start.unwrap_or(start);
                }
                idx = prev_start;
                continue;
            }

            if saw_doc {
                #[cfg(test)]
                if debug_expand {
                    let line_str = String::from_utf8_lossy(line);
                    eprintln!(
                        "expand-gap saw_doc line={} cursor={} start_cursor={} saw_doc_content={} doc_start={:?} saw_tilde_slash={}",
                        self.line_col(prev_start).0,
                        cursor + 1,
                        start_cursor + 1,
                        saw_doc_content,
                        doc_start,
                        saw_tilde_slash
                    );
                    eprintln!("expand-gap saw_doc line text={:?}", line_str);
                }
                if start_is_bar_cab
                    && !doc_anchor_inline
                    && !doc_under_plus_header_block
                    && !doc_under_caret_bar_block
                    && !matches!(doc_min_content_indent, Some(indent) if indent >= 4)
                {
                    return start;
                }
                if start_is_bar_percent && is_fas_import_line(line, cursor) {
                    return start;
                }
                if start_is_bar_gate_sample
                    && !doc_anchor_inline
                    && !saw_tilde_slash
                    && doc_top_blank
                {
                    if !(start_cursor > cursor && is_plus_header_line(line, cursor)) {
                        return start;
                    }
                }
                if start_is_equals
                    && start_cursor > cursor
                    && is_plus_header_line(line, cursor)
                    && !doc_top_blank
                    && !saw_tilde_slash
                    && !matches!(doc_min_content_indent, Some(indent) if indent >= 4)
                {
                    return start;
                }
                if start_is_caret
                    && !start_is_caret_plus
                    && start_cursor > cursor
                    && is_plus_header_line(line, cursor)
                    && !saw_tilde_slash
                {
                    let plus_header_has_inline_doc = inline_doc_offset(line)
                        .map(|(has_content, _)| has_content)
                        .unwrap_or(false);
                    if !plus_header_has_inline_doc
                        && start_cursor == cursor + 2
                        && doc_top_blank
                        && !doc_content_after_blank
                        && doc_min_content_indent.map_or(false, |indent| indent >= 4)
                    {
                        return if saw_doc_content {
                            doc_start.unwrap_or(start)
                        } else {
                            start
                        };
                    }
                    return start;
                }
                if !doc_anchor_inline {
                    let has_leading_blank_doc = doc_blank_after_content && !doc_content_after_blank;
                    let header_allows_blank_doc = is_plus_header_line(line, cursor);
                    let doc_has_mixed_indent = matches!(
                        (doc_min_content_indent, doc_max_content_indent),
                        (Some(min_indent), Some(max_indent)) if max_indent > min_indent
                    );
                    if start_is_equals_slash
                        && doc_under_equals_slash
                        && !doc_has_mixed_indent
                        && !doc_mentions_start_binding
                    {
                        return start;
                    }
                    let start_allows_blank_doc = (start_is_dollar && !start_is_dollar_colon)
                        || start_is_bar_percent
                        || start_is_underscore
                        || header_allows_blank_doc
                        || doc_under_dollar_colon
                        || (start_is_equals_slash && doc_has_mixed_indent);
                    let has_blank_after_content = doc_blank_after_content || doc_top_blank;
                    let has_internal_blank_doc = doc_blank_after_content && doc_content_after_blank;
                    if start_is_colon_rune {
                        return start;
                    }
                    if start_is_backtick {
                        return start;
                    }
                    if start_is_tilde_slog || start_is_tilde_print {
                        return start;
                    }
                    if start_is_tilde_plus
                        && doc_content_lines <= 1
                        && !doc_blank_after_content
                        && !doc_top_blank
                    {
                        return start;
                    }
                    if start_is_equals_dot && doc_under_plus_header_block {
                        return start;
                    }
                    if start_is_bar_star && has_internal_blank_doc {
                        return start;
                    }
                    if doc_under_dollar_colon
                        && !doc_top_blank
                        && !has_leading_blank_doc
                        && dollar_colon_line_has_inline_field(line, cursor)
                        && !dollar_colon_line_has_nested_header(line, cursor)
                    {
                        return start;
                    }
                    if start_cursor > cursor
                        && line.get(cursor) == Some(&b'?')
                        && line.get(cursor + 1) == Some(&b'.')
                    {
                        return start;
                    }
                    if start_cursor > cursor
                        && line.get(cursor) == Some(&b'?')
                        && line.get(cursor + 1) == Some(&b'~')
                        && !doc_has_triple_quote
                        && doc_min_content_indent == doc_max_content_indent
                    {
                        return start;
                    }
                    if start_cursor > cursor
                        && line.get(cursor) == Some(&b'?')
                        && line.get(cursor + 1) == Some(&b':')
                        && !doc_has_triple_quote
                        && doc_min_content_indent == doc_max_content_indent
                    {
                        return start;
                    }
                    if start_is_question
                        && doc_indent == Some(start_cursor)
                        && doc_min_content_indent.is_some()
                        && doc_min_content_indent == doc_max_content_indent
                    {
                        return start;
                    }
                    if start_cursor > cursor && is_plus_header_line(line, cursor) {
                        if let Some((has_content, doc_offset)) = inline_doc_offset(line) {
                            if has_content && plus_header_has_inline_body(line, cursor, doc_offset)
                            {
                                return start;
                            }
                        }
                        if start_is_equals
                            && doc_content_lines <= 1
                            && !doc_blank_after_content
                            && !doc_top_blank
                        {
                            return start;
                        }
                    }
                    let same_or_deeper_indent = cursor >= start_cursor;
                    let top_blank_disallowed = same_or_deeper_indent
                        && doc_top_blank
                        && !start_allows_blank_doc
                        && !doc_has_mixed_indent
                        && (doc_content_lines <= 1
                            || (start_is_equals
                                && !(start_is_equals_slash && doc_mentions_start_binding)));
                    let leading_blank_disallowed = same_or_deeper_indent
                        && has_leading_blank_doc
                        && !start_allows_blank_doc
                        && !doc_has_mixed_indent;
                    if top_blank_disallowed || leading_blank_disallowed {
                        let is_bar_dollar_line =
                            line.get(cursor) == Some(&b'|') && line.get(cursor + 1) == Some(&b'$');
                        let is_tilde_line = is_tilde_header_line(line, cursor);
                        if !is_bar_dollar_line && !is_tilde_line {
                            return start;
                        }
                    }
                    if let Some(indent) = doc_indent {
                        if indent != start_cursor {
                            return start;
                        }
                        if start_is_question_tilde
                            || (start_is_question && start_question_header_context)
                        {
                            return start;
                        }
                        if start_is_equals_bar {
                            return start;
                        }
                        if cursor == start_cursor {
                            if line.get(cursor) == Some(&b'~')
                                && line.get(cursor + 1) == Some(&b'+')
                            {
                                return start;
                            }
                            match line.get(cursor) {
                                Some(b'?') if line.get(cursor + 1) != Some(&b'>') => return start,
                                Some(b'=')
                                    if start_equals_has_inline_body
                                        && !(start_is_equals_slash && doc_has_mixed_indent) =>
                                {
                                    return start
                                }
                                _ => {}
                            }
                            let prev_is_question_gt = line.get(cursor) == Some(&b'?')
                                && line.get(cursor + 1) == Some(&b'>');
                            let prev_is_caret_hep = line.get(cursor) == Some(&b'^')
                                && line.get(cursor + 1) == Some(&b'-');
                            let prev_is_caret_bar = line.get(cursor) == Some(&b'^')
                                && line.get(cursor + 1) == Some(&b'|');
                            let prev_is_equals_slash = line.get(cursor) == Some(&b'=')
                                && line.get(cursor + 1) == Some(&b'/');
                            let prev_is_equals_dot = line.get(cursor) == Some(&b'=')
                                && line.get(cursor + 1) == Some(&b'.');
                            let prev_is_tilde_percent = line.get(cursor) == Some(&b'~')
                                && line.get(cursor + 1) == Some(&b'%');
                            let prev_is_tilde_slash = line.get(cursor) == Some(&b'~')
                                && line.get(cursor + 1) == Some(&b'/');
                            let prev_is_bar_dollar = line.get(cursor) == Some(&b'|')
                                && line.get(cursor + 1) == Some(&b'$');
                            let prev_is_question = line.get(cursor) == Some(&b'?');
                            let doc_has_mixed_indent = matches!(
                                (doc_min_content_indent, doc_max_content_indent),
                                (Some(min_indent), Some(max_indent)) if max_indent > min_indent
                            );
                            let doc_block_has_blank = doc_blank_after_content || doc_top_blank;
                            let prev_allows_equals_slash_doc = prev_is_equals_slash
                                || prev_is_question_gt
                                || (prev_is_equals_dot && doc_has_mixed_indent);
                            if start_is_equals_slash
                                && doc_block_has_blank
                                && (!doc_has_mixed_indent || start_equals_has_inline_body)
                                && !prev_allows_equals_slash_doc
                            {
                                return start;
                            }
                            let allow_blank_between_code_lines = doc_block_has_blank
                                && doc_content_lines > 1
                                && !start_is_question
                                && !start_is_percent_caret
                                && !(doc_block_has_rune_like && !doc_has_mixed_indent);
                            let allow_mixed_indent_between_code_lines =
                                doc_has_mixed_indent && (!start_is_question || !prev_is_question);
                            let allow_between_code_lines = prev_is_question_gt
                                || (prev_is_caret_hep
                                    && ((doc_content_lines <= 1 && !doc_blank_after_content)
                                        || doc_has_mixed_indent))
                                || prev_is_caret_bar
                                || (prev_is_equals_slash
                                    && start_is_equals
                                    && !start_equals_has_inline_body
                                    && !doc_block_has_blank
                                    && (!start_is_equals_slash || doc_mentions_start_binding));
                            let allow_tilde_percent_arm_heading = prev_is_tilde_percent
                                && doc_max_content_indent.map_or(false, |indent| indent >= 4);
                            let allow_between_code_lines = allow_between_code_lines
                                || (prev_is_tilde_percent
                                    && cursor == 0
                                    && !doc_line_under_tilde_percent_in_arm(prev_start, cursor))
                                || allow_tilde_percent_arm_heading
                                || start_is_tilde_percent
                                || allow_blank_between_code_lines
                                || allow_mixed_indent_between_code_lines;
                            let allow_tilde_slash_between = prev_is_tilde_slash
                                && !(doc_under_plus_header_block && doc_content_lines > 1);
                            let allow_between_code_lines = allow_between_code_lines
                                || allow_tilde_slash_between
                                || prev_is_bar_dollar;
                            let allow_dollar_colon_between = doc_under_dollar_colon
                                && doc_max_content_indent.map_or(false, |indent| indent >= 4);
                            let allow_between_code_lines =
                                allow_between_code_lines || allow_dollar_colon_between;
                            if !allow_between_code_lines {
                                return start;
                            }
                        }
                        if start_is_caret
                            && indent == cursor
                            && is_bar_gate_sample_line(line, cursor)
                        {
                            return start;
                        }
                        if start_is_caret && indent == cursor && is_tilde_hint_line(line, cursor) {
                            idx = prev_start;
                            continue;
                        }
                        if start_is_caret && indent == cursor && line.get(cursor) != Some(&b'+') {
                            return start;
                        }
                        if indent == cursor && line.get(cursor) == Some(&b'^') {
                            if start_is_equals {
                                return start;
                            }
                            if start_is_question_tilde {
                                return start;
                            }
                            if start_is_question && line.get(cursor + 1) == Some(&b'+') {
                                return start;
                            }
                        }
                        if indent == cursor && line.get(cursor) == Some(&b'[') {
                            return start;
                        }
                        if indent != cursor {
                            let lead = line.get(cursor).copied();
                            let mut end = line.len();
                            while end > cursor && (line[end - 1] == b' ' || line[end - 1] == b'\t')
                            {
                                end -= 1;
                            }
                            let trimmed = &line[cursor..end];
                            let is_tall_terminator = trimmed == b"==";
                            if start_is_caret && is_tall_terminator {
                                return start;
                            }
                            if indent > cursor
                                && doc_simple_heading
                                && doc_blank_after_content
                                && matches!(lead, Some(b'|'))
                            {
                                return start;
                            }
                            if indent < cursor && !is_tall_terminator {
                                if start_is_question && !doc_has_mixed_indent {
                                    return start;
                                }
                                let is_branch = is_branch_line(line, cursor)
                                    || is_branch_tag_line(line, cursor);
                                if is_branch {
                                    match (doc_min_content_indent, doc_max_content_indent) {
                                        (Some(min_indent), Some(max_indent))
                                            if max_indent > min_indent => {}
                                        _ => return start,
                                    }
                                } else if !(start_is_equals_slash && doc_has_mixed_indent)
                                    && !(doc_block_between_question_branches(prev_start, indent)
                                        && doc_has_mixed_indent)
                                    && !(doc_has_mixed_indent && indent == start_cursor)
                                {
                                    return start;
                                }
                            }
                            if indent > cursor {
                                let allow_dollar_colon = doc_under_dollar_colon
                                    && lead == Some(b'$')
                                    && line.get(cursor + 1) == Some(&b':');
                                let allow_equals_slash = start_is_equals_slash
                                    && lead == Some(b'=')
                                    && line.get(cursor + 1) == Some(&b'/');
                                let allow = matches!(lead, Some(b'|') | Some(b'+') | Some(b'?'))
                                    || allow_dollar_colon
                                    || allow_equals_slash;
                                if !allow {
                                    return start;
                                }
                                if doc_under_plus_header_block
                                    && doc_block_has_rune_like
                                    && doc_block_has_non_gate_rune_like
                                    && !doc_has_mixed_indent
                                {
                                    return start;
                                }
                                if start_is_question && lead == Some(b'?') {
                                    return start;
                                }
                                if start_is_caret
                                    && lead == Some(b'+')
                                    && is_plus_header_line(line, cursor)
                                {
                                    if let Some((has_content, _)) = inline_doc_offset(line) {
                                        if has_content {
                                            return start;
                                        }
                                    }
                                }
                                if lead == Some(b'+')
                                    && line.get(cursor + 1) == Some(&b'$')
                                    && start_has_same_indent_continuation
                                    && !doc_under_dollar_colon
                                {
                                    return start;
                                }
                            }
                        }
                        if indent == cursor && is_tilde_hint_line(line, cursor) {
                            return start;
                        }
                    }
                }
                return if saw_doc_content {
                    doc_start.unwrap_or(idx)
                } else {
                    start
                };
            }
            return start;
        }
    }
}

fn poon(pag: &[Hoon], goo: &[Option<Hoon>]) -> Option<Vec<Hoon>> {
    if goo.is_empty() {
        return Some(vec![]);
    }

    let (goo_hd, goo_tl) = goo.split_first().expect("non-empty goo checked above");

    let head = match goo_hd {
        Some(x) => x.clone(),
        None => {
            let (pag_hd, _) = pag.split_first()?;
            pag_hd.clone()
        }
    };

    let pag_tl = if pag.is_empty() { &[] } else { &pag[1..] };

    let mut rest = poon(pag_tl, goo_tl)?;

    let mut out = Vec::with_capacity(rest.len() + 1);
    out.push(head);
    out.append(&mut rest);

    Some(out)
}

pub fn posh(
    pre: Option<Vec<Option<Hoon>>>,          // (unit tyke)
    pof: Option<(usize, Vec<Option<Hoon>>)>, // (unit [p=@ud q=tyke])
    wer: Path,
) -> Option<Vec<Hoon>> {
    let wom: Vec<Hoon> = poof(wer);

    let yez = if let Some(pre_val) = pre.as_ref() {
        let moz = poon(&wom, pre_val)?;

        if pof.is_some() {
            let n = pre_val.len();
            let sl = slag(n, &wom.clone());
            Some(weld(&moz, &sl))
        } else {
            Some(moz)
        }
    } else {
        Some(wom.clone())
    }?;

    let Some((p, q)) = pof else {
        return Some(yez);
    };

    let zey = flop(&yez.clone());

    let moz = scag(p, &zey);
    let gul = slag(p, &zey);

    let zom = poon(&flop(&moz.clone()), &q);

    match zom {
        None => None,
        Some(z) => Some(weld(&flop(&gul), z)),
    }
}

pub fn nusk<'src>() -> impl Parser<'src, &'src str, Coin, Err<'src>> {
    urt()
        .try_map(|s, span| {
            wick(s).ok_or_else(|| Rich::custom(span, format!("invalid knot escape in '{}'", s)))
        })
        .try_map(|unescaped: String, span| {
            let parsed = nuck().parse(&unescaped);
            match parsed.into_result() {
                Ok(output) => Ok(output),
                Err(_errors) => Err(Rich::custom(span, "nuck parse failed")),
            }
        })
}

pub fn jock(rad: bool, lot: &Coin) -> Hoon {
    match lot {
        Coin::Dime(tag, atom) => {
            if rad {
                Hoon::Rock(tag.clone(), NounExpr::ParsedAtom(atom.clone()))
            } else {
                Hoon::Sand(tag.clone(), NounExpr::ParsedAtom(atom.clone()))
            }
        }

        Coin::Blob(noun) => {
            if rad {
                Hoon::Rock("$".to_string(), noun.clone())
            } else {
                match noun {
                    NounExpr::ParsedAtom(atom) => {
                        Hoon::Sand("$".to_string(), NounExpr::ParsedAtom(atom.clone()))
                    }
                    NounExpr::Cell(head, tail) => Hoon::Pair(
                        Box::new(jock(rad, &Coin::Blob(*head.clone()))),
                        Box::new(jock(rad, &Coin::Blob(*tail.clone()))),
                    ),
                }
            }
        }

        Coin::Many(coins) => Hoon::ColTar(coins.iter().map(|c| jock(rad, c)).collect()),
    }
}

pub fn nuck<'src>() -> impl Parser<'src, &'src str, Coin, Err<'src>> {
    choice((
        symbol().map(|s| Coin::Dime("tas".to_string(), string_to_atom(s))),
        number().map(|(p, q)| Coin::Dime(p, q)),
        just('.').ignore_then(perd()),
        just('~').ignore_then(choice((
            twid(),
            empty().to(Coin::Dime("n".to_string(), ParsedAtom::Small(0))),
        ))),
    ))
    .boxed()
}

pub fn perd<'src>() -> impl Parser<'src, &'src str, Coin, Err<'src>> {
    choice((
        zust(),
        nusk()
            .separated_by(just('_'))
            .at_least(1)
            .collect::<Vec<_>>()
            .delimited_by(just('_'), just("__"))
            .map(|t| Coin::Many(t)),
    ))
}

pub fn zust<'src>() -> impl Parser<'src, &'src str, Coin, Err<'src>> {
    choice((
        ipv6_address().try_map(|s, span| {
            let maybe_ipv6 = ipv6_to_atom(s.clone());
            match maybe_ipv6 {
                None => Err(Rich::custom(span, "invalid ipv6")),
                Some(atom) => Ok(Coin::Dime("is".to_string(), atom)),
            }
        }),
        ipv4_address().try_map(|s, span| {
            let maybe_ipv4 = ipv4_to_atom(s);
            match maybe_ipv4 {
                None => Err(Rich::custom(span, "invalid ipv4")),
                Some(atom) => Ok(Coin::Dime("if".to_string(), atom)),
            }
        }),
        float().map(|(p, q)| Coin::Dime(p, q)),
        just("y").to(Coin::Dime("f".to_string(), ParsedAtom::Small(0))),
        just("n").to(Coin::Dime("f".to_string(), ParsedAtom::Small(1))),
        just('~')
            .ignore_then(phonemic_name_unscrambled())
            .map(|s| Coin::Dime("q".to_string(), s)),
    ))
}

pub fn trip(mut atom: ParsedAtom) -> Tape {
    let mut out = Vec::new();

    while atom != ParsedAtom::Small(0) {
        let byte_atom = end(3, 1, &atom);

        let byte = match byte_atom {
            ParsedAtom::Small(x) => x as u8,
            ParsedAtom::Big(b) => b.try_into().unwrap_or(0),
        };

        out.push((byte as char).to_string());
        atom = rsh(3, 1, &atom);
    }

    out
}

pub fn wack(a: &str) -> String {
    a.chars()
        .flat_map(|c| match c {
            '~' => vec!['~', '~'],
            '_' => vec!['~', '-'],
            _ => vec![c],
        })
        .collect()
}

pub fn reap<T: Clone>(a: usize, b: T) -> Vec<T> {
    vec![b; a]
}

pub fn path<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    wer: Path,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    let wer1 = wer.clone();
    let wer2 = wer.clone();
    let wer3 = wer.clone();
    let wer4 = wer.clone();

    let hasp = choice((
        hoon_wide.clone().delimited_by(just('['), just(']')),
        hoon_wide
            .clone()
            .separated_by(just(' '))
            .at_least(1)
            .collect::<Vec<_>>()
            .delimited_by(just('('), just(')'))
            .map(|list| {
                let (first, rest) = list.split_first().expect("non-empty path list parsed");
                Hoon::CenCol(Box::new(first.clone()), rest.to_vec())
            }),
        just('$').to(Hoon::Sand(
            "tas".to_string(),
            NounExpr::ParsedAtom(ParsedAtom::Small(0)),
        )),
        cord(linemap).map(|s| Hoon::Sand("t".to_string(), NounExpr::ParsedAtom(s))),
        nuck().map(|coin| {
            let aura = match &coin {
                Coin::Dime(a, _) if a == "tas" => "tas",
                _ => "ta",
            };
            Hoon::Sand(aura.to_string(), NounExpr::ParsedAtom(rent_co(&coin)))
        }),
    ));

    let gasp = choice((
        just('=')
            .to(None)
            .repeated()
            .collect::<Vec<Option<Hoon>>>()
            .then(hasp.map(|h| vec![Some(h)]))
            .then(just('=').to(None).repeated().collect::<Vec<Option<Hoon>>>())
            .map(|((mut a, b), c)| {
                a.extend(b);
                a.extend(c);
                a
            }),
        just('=')
            .to(None)
            .repeated()
            .at_least(1)
            .collect::<Vec<Option<Hoon>>>(),
    ));

    let limp = just("/").repeated().count().then(gasp).map(|(a, mut b)| {
        for _ in 0..a {
            b.insert(
                0,
                Some(Hoon::Sand(
                    "tas".to_string(),
                    NounExpr::ParsedAtom(ParsedAtom::Small(0)),
                )),
            );
        }
        b
    });

    let gash = limp
        .separated_by(just("/"))
        .collect::<Vec<Vec<Option<Hoon>>>>()
        .map(|a| a.into_iter().flatten().collect::<Vec<_>>())
        .boxed();

    let porc = just("%")
        .repeated()
        .count() //  usize
        .then(just("/").ignore_then(gash.clone())); // Vec<Option<Hoon>>

    let poor = gash
        .clone()
        .map(|pre| Some(pre))
        .then(just("%").ignore_then(porc.clone()).or_not());

    let rood = {
        just("/")
            .ignore_then(
                poor.try_map(move |(pre, pof), span| match posh(pre, pof, wer1.clone()) {
                    Some(list) => Ok(Hoon::ColSig(list)),
                    None => Err(Rich::custom(span, "error parsing path")),
                }),
            )
            .labelled("Path")
    };

    let cen_fas = {
        porc.try_map(
            move |(a, b), span| match posh(Some(vec![None]), Some((a, b)), wer2.clone()) {
                Some(list) => Ok(Hoon::ColSig(list)),
                None => Err(Rich::custom(span, "error parsing path")),
            },
        )
    };

    let multi_cen = {
        just("%").repeated().count().try_map(move |n, span| {
            match posh(Some(vec![None]), Some((n, vec![])), wer3.clone()) {
                Some(list) => Ok(Hoon::ColSig(list)),
                None => Err(Rich::custom(span, "error parsing path")),
            }
        })
    };

    let cen_path = just("%")
        .ignore_then(choice((cen_fas, multi_cen)))
        .labelled("Path");

    choice((
        rood.boxed(),     //  /foo/%/foo
        cen_path.boxed(), //  %/foo  and  %%
    ))
    .labelled("Path")
}

pub fn rent_co(lot: &Coin) -> ParsedAtom {
    let rend_res = rend_co(lot);
    let bytes: Vec<u128> = rend_res
        .into_iter()
        .flat_map(|s: String| s.chars().map(|c| c as u128).collect::<Vec<_>>())
        .collect();
    let rap_res = rap(3 as usize, &bytes);
    rap_res
}

pub fn rend_co(lot: &Coin) -> Tape {
    rend_with_rep(lot, vec![])
}

fn rend_many(coins: &[Coin], rep: Tape) -> Tape {
    if coins.is_empty() {
        return vec!["_".to_string(), "_".to_string()]
            .into_iter()
            .chain(rep)
            .collect();
    }
    let first = &coins[0];
    let rest = &coins[1..];

    let mut res = vec!["_".to_string()];
    let rendered_first = rend_co(first);
    let escaped_knot = wack(&rendered_first.concat());
    let taped_escaped = trip(string_to_atom(escaped_knot));
    res.extend(taped_escaped);
    res.extend(rend_many(rest, rep));
    res
}

fn rend_with_rep(lot: &Coin, mut rep: Tape) -> Tape {
    match lot {
        Coin::Blob(noun) => {
            let jammed = jam_simple(noun.clone());
            let mut res = vec!["~".to_string(), "0".to_string()];
            res.extend(v_co(1, &jammed));
            res
        }

        Coin::Many(coins) => {
            let mut res = vec![".".to_string()];
            res.extend(rend_many(coins, rep));
            res
        }

        Coin::Dime(prefix, q) => {
            let yed = end(3, 1, &string_to_atom(prefix.to_string())); // first char of prefix
            let hay = cut(3, 1, 1, &string_to_atom(prefix.to_string())); // second char

            let yed_char = match &yed {
                ParsedAtom::Small(x) => *x as u8 as char,
                ParsedAtom::Big(_) => unreachable!(), // prefix is short
            };

            let hay_char = match &hay {
                ParsedAtom::Small(x) => *x as u8 as char,
                ParsedAtom::Big(_) => unreachable!(),
            };

            match yed_char {
                'c' => {
                    let mut res = vec!['~'.to_string(), '-'.to_string()];
                    let wood_res = wood(&tuft(q));
                    let rip_res = rip(3, &wood_res);
                    let qtape: Vec<_> = rip_res.into_iter().flat_map(|a| trip(a)).collect();
                    res.extend(qtape);
                    res.extend(rep);
                    res
                }

                'd' => match hay_char {
                    'a' => {
                        let yod = yore(q);
                        let mut rep = rep;
                        if !yod.t.f.is_empty() {
                            let frac_tape = s_co(&yod.t.f);
                            let mut new_rep = vec![".".to_string()];
                            new_rep.extend(frac_tape);
                            new_rep.extend(rep);
                            rep = new_rep;
                        }

                        let t = &yod.t;
                        if !(yod.t.f.is_empty() && t.h == 0 && t.m == 0 && t.s == 0) {
                            let s_atom = ParsedAtom::Small(t.s as u128);
                            let mut new_rep = vec![".".to_string()];
                            new_rep.extend(y_co(&s_atom));
                            let m_atom = ParsedAtom::Small(t.m as u128);
                            let mut newer_rep = vec![".".to_string()];
                            newer_rep.extend(y_co(&m_atom));
                            newer_rep.extend(new_rep);
                            let h_atom = ParsedAtom::Small(t.h as u128);
                            let mut newest_rep = vec![".".to_string(), ".".to_string()];
                            newest_rep.extend(y_co(&h_atom));
                            newest_rep.extend(newer_rep);
                            newest_rep.extend(rep);
                            rep = newest_rep
                        }

                        let d_atom = ParsedAtom::Small(t.d as u128);
                        let mut new_rep = vec![".".to_string()];
                        new_rep.extend(a_co(&d_atom));
                        new_rep.extend(rep);
                        rep = new_rep;

                        let m_atom = ParsedAtom::Small(yod.m as u128);
                        let mut newer_rep = vec![".".to_string()];
                        newer_rep.extend(a_co(&m_atom));
                        newer_rep.extend(rep);
                        rep = newer_rep;

                        if !yod.era {
                            let mut newest_rep = vec!["-".to_string()];
                            newest_rep.extend(rep);
                            rep = newest_rep;
                        }

                        let y_atom = ParsedAtom::Small(yod.y as u128);
                        let mut res = vec!["~".to_string()];
                        res.extend(a_co(&y_atom));
                        res.extend(rep);
                        res
                    }

                    'r' => {
                        let yug = yell(q);

                        let mut rep = rep;

                        if !yug.f.is_empty() {
                            let frac_tape = s_co(&yug.f);
                            let mut new_rep = vec![".".to_string()];
                            new_rep.extend(frac_tape);
                            new_rep.extend(rep);
                            rep = new_rep;
                        }

                        let mut res = vec!["~".to_string()];

                        if yug.d == 0 && yug.m == 0 && yug.h == 0 && yug.s == 0 {
                            res.extend(vec!["s".to_string(), "0".to_string()]);
                            res.extend(rep);
                            return res;
                        }

                        if yug.s != 0 {
                            let s_atom = ParsedAtom::Small(yug.s as u128);
                            let mut new_rep = vec![".".to_string(), "s".to_string()];
                            new_rep.extend(a_co(&s_atom));
                            new_rep.extend(rep);
                            rep = new_rep;
                        }

                        if yug.m != 0 {
                            let m_atom = ParsedAtom::Small(yug.m as u128);
                            let mut new_rep = vec![".".to_string(), "m".to_string()];
                            new_rep.extend(a_co(&m_atom));
                            new_rep.extend(rep);
                            rep = new_rep;
                        }

                        if yug.h != 0 {
                            let h_atom = ParsedAtom::Small(yug.h as u128);
                            let mut new_rep = vec![".".to_string(), "h".to_string()];
                            new_rep.extend(a_co(&h_atom));
                            new_rep.extend(rep);
                            rep = new_rep;
                        }

                        if yug.d != 0 {
                            let d_atom = ParsedAtom::Small(yug.d as u128);
                            let mut new_rep = vec![".".to_string(), "d".to_string()];
                            new_rep.extend(a_co(&d_atom));
                            new_rep.extend(rep);
                            rep = new_rep;
                        }

                        res.extend(rep.iter().skip(1).cloned());
                        res
                    }

                    _ => z_co(q),
                },

                'f' => match q {
                    ParsedAtom::Small(0) => vec!['.'.to_string(), 'y'.to_string()],
                    ParsedAtom::Small(1) => vec!['.'.to_string(), 'n'.to_string()],
                    _ => z_co(q),
                }
                .into_iter()
                .chain(rep.into_iter())
                .collect(),

                'n' => {
                    let mut res = vec!['~'.to_string()];
                    res.extend(rep);
                    res
                }

                'i' => match hay_char {
                    'f' => ro_co([3, 10, 4], &|x| d_ne(x), q),
                    's' => ro_co([4, 16, 8], &|x| x_ne(x), q),
                    _ => z_co(q),
                },

                'p' => {
                    let sxz = fein(q.clone());
                    let dyx = met(3, &sxz);

                    let mut out: Tape = vec!['~'.to_string()];

                    if dyx <= 1 {
                        let byte = sxz.to_u8_lossy();
                        let syl = tod_po(byte);
                        out.extend(trip(syl));
                        out.extend(rep);
                        return out;
                    }

                    let dyy = met(4, &sxz);
                    let mut chunks = Vec::with_capacity(dyy);

                    for imp in 0..dyy {
                        let log = cut(4, imp, 1, &sxz);

                        let hi_atom = rsh(3, 1, &log);
                        let hi = hi_atom.to_u8_lossy();

                        let lo_atom = end(3, 1, &log);
                        let lo = lo_atom.to_u8_lossy();

                        let prefix = trip(tos_po(hi));
                        let suffix = trip(tod_po(lo));

                        let mut chunk = weld(&prefix, &suffix);

                        let sep = if imp % 4 == 0 {
                            if imp == 0 {
                                vec![]
                            } else {
                                vec!['-'.to_string(), '-'.to_string()]
                            }
                        } else {
                            vec!['-'.to_string()]
                        };
                        chunk.extend(sep);

                        chunks.push(chunk);
                    }

                    chunks.reverse();
                    for chunk in chunks {
                        out.extend(chunk);
                    }
                    out.extend(rep);
                    out
                }

                'q' => {
                    let head = vec![".".to_string(), "~".to_string()];

                    let lot: Vec<ParsedAtom> = if q.is_zero() {
                        vec![ParsedAtom::Small(0)]
                    } else {
                        rip(3, q)
                    };

                    let mut r: Tape = Vec::new();
                    let mut s = true;

                    for atom in lot.into_iter() {
                        let q_atom = atom.to_u8().expect("byte");

                        let mut rendered = if s {
                            trip(tod_po(q_atom))
                        } else {
                            trip(tos_po(q_atom))
                        };

                        let tail = if s && !r.is_empty() {
                            let mut t = vec!["-".to_string()];
                            t.extend(r);
                            t
                        } else {
                            r
                        };

                        s = !s;
                        r = weld(rendered, tail);
                    }

                    let mut res = head;
                    res = weld(res, r);
                    res = weld(res, rep);
                    res
                }

                'r' => match hay_char {
                    'd' => {
                        let val = q.to_u128().expect("decimal cord atom should fit u128");
                        let df = rlyd(val);
                        let rc = r_co(&df, rep.clone());
                        let mut res = vec![".".to_string(), "~".to_string()];
                        res.extend(rc);
                        res.extend(rep);
                        res
                    }
                    'h' => {
                        let val = q.to_u128().expect("hex cord atom should fit u128");
                        let df = rlyh(val);
                        let rc = r_co(&df, rep.clone());
                        let mut res = vec![".".to_string(), "~".to_string(), "~".to_string()];
                        res.extend(rc);
                        res.extend(rep);
                        res
                    }
                    'q' => {
                        let val = q.to_u128().expect("quad cord atom should fit u128");
                        let df = rlyq(val);
                        let rc = r_co(&df, rep.clone());
                        let mut res = vec![
                            ".".to_string(),
                            "~".to_string(),
                            "~".to_string(),
                            "~".to_string(),
                        ];
                        res.extend(rc);
                        res.extend(rep);
                        res
                    }
                    's' => {
                        let val = q.to_u128().expect("signed cord atom should fit u128");
                        let df = rlys(val);
                        let rc = r_co(&df, rep.clone());
                        let mut res = vec![".".to_string()];
                        res.extend(rc);
                        res.extend(rep);
                        res
                    }
                    _ => {
                        let mut res = z_co(q);
                        res.extend(rep);
                        res
                    }
                },

                'u' => {
                    match hay_char {
                        'c' => {
                            // base58check with padding
                            let encoded = enc_fa(q);
                            let padded_ones = reap(pad_fa(&q), '1'.to_string());
                            let mut res = vec!['0'.to_string(), 'c'.to_string()];
                            res.extend(padded_ones);
                            if q.is_zero() {
                                res.push("0".to_string());
                            } else {
                                res.extend(c_co(&encoded));
                            }
                            res.extend(rep);
                            res
                        }
                        'b' => with_prefix("0b", &ox_co([2, 4], &|x| d_ne(x), q), q, rep),
                        'i' => with_prefix("0i", &d_co(1, q), q, rep),
                        'x' => with_prefix("0x", &ox_co([16, 4], &|x| x_ne(x), q), q, rep),
                        'v' => with_prefix("0v", &ox_co([32, 5], &|x| x_ne(x), q), q, rep),
                        'w' => with_prefix("0w", &ox_co([64, 5], &|x| w_ne(x), q), q, rep),
                        _ => with_prefix("", &ox_co([10, 3], &|x| d_ne(x), q), q, rep),
                    }
                }

                's' => {
                    let q = q.to_u128().expect("signed number is bigger than 128 bits");
                    let sign_prefix_chars = if syn_si(q) {
                        vec!['-'.to_string(), '-'.to_string()]
                    } else {
                        vec!['-'.to_string()]
                    };
                    let abs_val = abs_si(q);
                    let mut res: Tape = sign_prefix_chars.into_iter().collect();
                    res.extend(rend_with_rep(
                        &Coin::Dime("u".into(), ParsedAtom::Small(abs_val)),
                        rep,
                    ));
                    res
                }

                't' => {
                    if hay_char == 'a' {
                        let third = cut(3, 2, 1, &string_to_atom(prefix.to_string()));
                        let third_char = match &third {
                            ParsedAtom::Small(x) => *x as u8 as char,
                            ParsedAtom::Big(_) => '\0',
                        };
                        if third_char == 's' {
                            let mut res: Vec<_> =
                                rip(3, q).into_iter().flat_map(|a| trip(a)).collect();
                            res.extend(rep);
                            res
                        } else {
                            let mut res = vec!['~'.to_string(), '.'.to_string()];
                            res.extend(rip(3, q).into_iter().flat_map(|a| trip(a)));
                            res.extend(rep);
                            res
                        }
                    } else {
                        let mut res = vec!['~'.to_string(), '~'.to_string()];
                        let wooded = wood(q);
                        res.extend(
                            rip(3, &ParsedAtom::from(wooded))
                                .into_iter()
                                .flat_map(|a| trip(a)),
                        );
                        res.extend(rep);
                        res
                    }
                }

                _ => z_co(q),
            }
        }
    }
}

fn r_co(df: &DecimalFloat, mut rep: Tape) -> Tape {
    match df {
        DecimalFloat::Infinity { sign } => {
            let prefix = if *sign { "inf" } else { "-inf" };
            prefix
                .chars()
                .map(|c| c.to_string())
                .chain(rep.into_iter())
                .collect()
        }
        DecimalFloat::NaN => "nan"
            .chars()
            .map(|c| c.to_string())
            .chain(rep.into_iter())
            .collect(),
        DecimalFloat::Finite { sign, exp, mant } => {
            let f: Tape = d_co(1, &ParsedAtom::Big(mant.clone()));

            let (e, exp): (u128, u128) = {
                let e = sun_si(f.len() as u128);

                let sci = sum_si(*exp, sum_si(e, 1));

                if syn_si(dif_si(*exp, 6)) {
                    (2, sci)
                } else if !syn_si(dif_si(sci, 3)) {
                    (2, sci)
                } else {
                    (sum_si(sci, 2), 0)
                }
            };

            if exp != 0u128 {
                let exp_mark = if syn_si(exp) { "e" } else { "e-" };
                rep = weld(
                    vec![exp_mark.to_string()],
                    d_co(1, &ParsedAtom::Small(abs_si(exp))),
                );
            }

            let mut out = weld(ed_co(&e, &f), rep);

            if !sign {
                out = weld(vec!["-".to_string()], out);
            }

            out
        }
    }
}

fn ed_co(exp: &u128, int: &Tape) -> Tape {
    let cmp = cmp_si(*exp, 0);
    let pos = cmp == 2;
    let dig = abs_si(*exp) as usize;

    if !pos {
        let mut out = reap(dig + 1, "0".to_string());
        out.extend(int.clone());
        return into(out, 1, ".");
    }

    let len = int.len();

    if dig < len {
        return into(int.clone(), dig, ".");
    }

    let mut out = int.clone();
    out.extend(reap(dig - len, "0".to_string()));
    out
}

fn wood_go(a: &ParsedAtom) -> Vec<u128> {
    if a.is_zero() {
        return Vec::new();
    }

    let b = teff(a);
    let c_atom = taft(&end(3, b, a));
    let c = c_atom.to_u32().expect("cord byte should fit in u32");
    let mut d = wood_go(&rsh(3, b, a));

    // alnum or '-'
    if (c >= b'a' as u32 && c <= b'z' as u32)
        || (c >= b'0' as u32 && c <= b'9' as u32)
        || c == b'-' as u32
    {
        d.insert(0, c as u128);
        return d;
    }

    match c as u8 {
        b' ' => {
            d.insert(0, b'.' as u128);
        }
        b'.' => {
            d.insert(0, b'.' as u128);
            d.insert(0, b'~' as u128);
        }
        b'~' => {
            d.insert(0, b'~' as u128);
            d.insert(0, b'~' as u128);
        }
        _ => {
            d = wood_hex(c, d);
        }
    }

    d
}

fn wood_hex(c: u32, mut d: Vec<u128>) -> Vec<u128> {
    let e = met(2, &ParsedAtom::Small(c as u128));

    d.insert(0, b'.' as u128);

    for i in 0..e {
        let shift = i * 4;
        let f = (c >> shift) & 0xF;
        let ch = if f <= 9 { 48 + f } else { 87 + f };
        d.insert(0, ch as u128);
    }

    d.insert(0, b'~' as u128);
    d
}

pub fn wood(a: &ParsedAtom) -> ParsedAtom {
    let bytes = wood_go(a);
    rap(3, &bytes)
}

fn into(mut tape: Tape, idx: usize, ch: &str) -> Tape {
    tape.insert(idx, ch.to_string());
    tape
}

fn atom_to_char(atom: &ParsedAtom) -> char {
    let code = match atom {
        ParsedAtom::Small(x) => *x as u32,
        ParsedAtom::Big(b) => {
            if *b > BigUint::from(u32::MAX) {
                0xFFFD //  replacement
            } else {
                b.clone().try_into().unwrap_or(0xFFFD)
            }
        }
    };
    std::char::from_u32(code).unwrap_or('\u{FFFD}')
}

fn d_ne(tig: u128) -> char {
    (tig as u8 + b'0') as char
}

fn x_ne(tig: u128) -> char {
    if tig < 10 {
        (b'0' + tig as u8) as char
    } else {
        (b'a' + (tig - 10) as u8) as char
    }
}

fn v_ne(tig: u128) -> char {
    if tig >= 10 {
        (tig + 87) as u8 as char
    } else {
        (tig + 48) as u8 as char
    }
}

fn w_ne(tig: u128) -> char {
    // base64 with - and ~ for 62/63
    if tig == 62 {
        '-'
    } else if tig == 63 {
        '~'
    } else if tig < 26 {
        (b'A' + tig as u8) as char
    } else if tig < 52 {
        (b'a' + (tig - 26) as u8) as char
    } else if tig < 62 {
        (b'0' + (tig - 52) as u8) as char
    } else {
        unreachable!()
    }
}

fn c_ne(tig: u128) -> char {
    // base58: skips 0, O, I, l
    const CHARS: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    CHARS[tig as usize] as char
}

fn with_prefix(prefix: &str, body: &Tape, dat: &ParsedAtom, rep: Tape) -> Tape {
    let mut res: Tape = prefix.chars().map(|c| c.to_string()).collect();
    if dat.is_zero() {
        res.push("0".to_string());
    } else {
        res.extend(body.iter().cloned());
    }
    res.extend(rep);
    res
}

fn s_co(frac: &[u64]) -> Tape {
    if frac.is_empty() {
        return vec![];
    }
    let mut res = vec![".".to_string()];
    let first = ParsedAtom::Small(frac[0] as u128);
    res.extend(x_co(4, &first));
    res.extend(s_co(&frac[1..]));
    res
}

fn em_co<F>(bas: u128, min: usize, mut par: F, hol: &ParsedAtom, rep: Tape) -> Tape
where
    F: FnMut(bool, u128, Tape) -> Tape,
{
    if hol.is_zero() && min == 0 {
        return rep;
    }
    let (dar, rad) = dvr(hol, &ParsedAtom::Small(bas));
    let next_min = min.saturating_sub(1);
    let rad_u128 = rad.to_u128().unwrap_or(0);
    let next_rep = par(dar.is_zero(), rad_u128, rep);
    em_co(bas, next_min, par, &dar, next_rep)
}

// Helper: dvr for ParsedAtom
fn dvr(a: &ParsedAtom, b: &ParsedAtom) -> (ParsedAtom, ParsedAtom) {
    match (a, b) {
        (ParsedAtom::Small(x), ParsedAtom::Small(y)) => {
            let (q, r) = (x / y, x % y);
            (ParsedAtom::Small(q), ParsedAtom::Small(r))
        }
        _ => {
            let a_big = a.to_biguint();
            let b_big = b.to_biguint();
            let (q, r) = dvr_big(&a_big, &b_big);
            (ParsedAtom::Big(q), ParsedAtom::Big(r))
        }
    }
}

fn dvr_u64(a: u64, b: u64) -> (u64, u64) {
    (a / b, a % b)
}

fn d_co(min: usize, dat: &ParsedAtom) -> Tape {
    em_co(
        10,
        min,
        |_, b, c: Tape| {
            let ch = d_ne(b);
            std::iter::once(ch.to_string()).chain(c).collect()
        },
        dat,
        vec![],
    )
}

fn x_co(min: usize, dat: &ParsedAtom) -> Tape {
    em_co(
        16,
        min,
        |_, b, c| {
            let ch = x_ne(b).to_string();
            std::iter::once(ch).chain(c).collect::<Vec<String>>()
        },
        dat,
        vec![],
    )
}

fn v_co(min: usize, dat: &ParsedAtom) -> Tape {
    em_co(
        32,
        min,
        |_, b, c| {
            let ch = v_ne(b).to_string();
            std::iter::once(ch).chain(c).collect::<Vec<String>>()
        },
        dat,
        vec![],
    )
}

fn w_co(min: usize, dat: &ParsedAtom) -> Tape {
    em_co(
        64,
        min,
        |_, b, c| {
            let ch = w_ne(b).to_string();
            std::iter::once(ch).chain(c).collect::<Vec<String>>()
        },
        dat,
        vec![],
    )
}

fn c_co(dat: &ParsedAtom) -> Tape {
    em_co(
        58,
        1,
        |_, b, c| {
            let ch = c_ne(b).to_string();
            std::iter::once(ch).chain(c).collect::<Vec<String>>()
        },
        dat,
        vec![],
    )
}

fn a_co(dat: &ParsedAtom) -> Tape {
    d_co(1, dat)
}

fn y_co(dat: &ParsedAtom) -> Tape {
    d_co(2, dat)
}

fn z_co(dat: &ParsedAtom) -> Tape {
    let mut res = vec!["0".to_string(), "x".to_string()];
    res.extend(x_co(1, dat));
    res
}

fn ox_co<F>([bas, gop]: [u128; 2], dug: &F, hol: &ParsedAtom) -> Tape
where
    F: Fn(u128) -> char,
{
    let pow_bas_gop = pow(bas, gop).to_u128().expect("base does not fit in u128");
    em_co(
        pow_bas_gop,
        0,
        |top, seg, res| {
            let prefix: Tape = if top { vec![] } else { vec!['.'.to_string()] };
            let inner = em_co(
                bas,
                if top { 0 } else { gop as usize },
                |_, b, c| {
                    std::iter::once(dug(b).to_string())
                        .chain(c)
                        .collect::<Vec<String>>()
                },
                &ParsedAtom::Small(seg),
                res,
            );
            prefix.into_iter().chain(inner).collect()
        },
        hol,
        vec![],
    )
}

fn ro_co<F>([buz, bas, mut dop]: [usize; 3], dug: &F, hol: &ParsedAtom) -> Tape
where
    F: Fn(u128) -> char,
{
    if dop == 0 {
        return vec![];
    }
    let pod = dop - 1;
    let seg = cut(buz, pod, 1, hol); // bloq = buz, start = pod, run = 1
    let mut res = vec!['.'.to_string()];
    res.extend(em_co(
        bas as u128,
        1,
        |_, b, c| {
            std::iter::once(dug(b).to_string())
                .chain(c)
                .collect::<Vec<String>>()
        },
        &seg,
        ro_co([buz, bas, pod], dug, hol),
    ));
    res
}

pub fn number<'src>() -> impl Parser<'src, &'src str, (String, ParsedAtom), Err<'src>> {
    let ud_number = decimal_number().map(|s| ("ud".to_string(), decimal_to_atom(s)));

    let ux_number = hexadecimal_number().map(|s| ("ux".to_string(), hex_to_atom(s)));

    let uc_number = bitcoin_address().try_map(|s, span| {
        let maybe_base58 = base58_to_atom(s);
        match maybe_base58 {
            None => Err(Rich::custom(span, "Invalid BTC address.")),
            Some(atom) => Ok(("uc".to_string(), atom)),
        }
    });

    let ub_number = binary_number().map(|s| ("ub".to_string(), binary_to_atom(s)));

    let uv_number = base32_number().map(|a| ("uv".to_string(), a));

    let uw_number = base64_number().map(|a| ("uw".to_string(), a));

    let ui_number = just("0i")
        .ignore_then(digits())
        .map(|s| ("ui".to_string(), decimal_to_atom(s)));

    let negative = choice((
        hexadecimal_number().map(|s| ("sx".to_string(), hex_to_atom(s))),
        binary_number().map(|s| ("sb".to_string(), binary_to_atom(s))),
        bitcoin_address().try_map(|s, span| {
            let maybe_base58 = base58_to_atom(s);
            match maybe_base58 {
                None => Err(Rich::custom(span, "Invalid BTC address.")),
                Some(atom) => Ok(("uc".to_string(), atom)),
            }
        }),
        base32_number().map(|a| ("sv".to_string(), a)),
        base64_number().map(|a| ("sw".to_string(), a)),
        just("0i")
            .ignore_then(digits())
            .map(|s| ("si".to_string(), decimal_to_atom(s))),
        decimal_number().map(|s| ("sd".to_string(), decimal_to_atom(s))),
    ))
    .boxed();

    let signed_number = // signed: -num and --num
        just('-')
        .ignore_then(
            just('-')
            .ignore_then(negative.clone().map(|(p, q)| (p, apply_sign(true, q))))
            .or(negative.map(|(p, q)| (p, apply_sign(false, q)))));

    choice((
        signed_number, ub_number, uc_number, ui_number, ux_number, uv_number, uw_number, ud_number,
    ))
    .labelled("Number")
}

// decimal without leading 0 and without dots.
//
pub fn decimal_without_leading_zero<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    just('0').to("0".to_string()).or(any()
        .filter(|c: &char| matches!(c, '1'..='9'))
        .then(
            any()
                .filter(|c: &char| c.is_ascii_digit())
                .repeated()
                .collect::<String>(),
        )
        .map(|(h, t)| format!("{h}{t}")))
}

pub fn absolute_date<'src>() -> impl Parser<'src, &'src str, ParsedAtom, Err<'src>> {
    let era_year = decimal_without_leading_zero()
        .then(just('-').to(false).or_not().map(|opt| opt.unwrap_or(true)))
        .try_map(|(year_str, era), span| {
            let year: u64 = year_str
                .parse()
                .map_err(|_| Rich::custom(span, "invalid year number"))?;

            if year == 0 {
                return Err(Rich::custom(span, "year must be ≥ 1"));
            }

            Ok((era, year))
        });
    let month = just('.').ignore_then(digits()).try_map(|s: String, span| {
        let m: u64 = s.parse().map_err(|_| Rich::custom(span, "invalid month"))?;
        if (1..=12).contains(&m) {
            Ok(m)
        } else {
            Err(Rich::custom(span, "month out of range (1–12)"))
        }
    });
    let day = just('.').ignore_then(digits()).try_map(|s, span| {
        let d: u64 = s.parse().map_err(|_| Rich::custom(span, "invalid day"))?;
        if (1..=31).contains(&d) {
            Ok(d)
        } else {
            Err(Rich::custom(span, "day out of range (1–31)"))
        }
    });
    let hour_min_secs_fractions = just("..")
        .ignore_then(
            digits()
                .try_map(|s, span| {
                    let h: u64 = s
                        .parse::<u64>()
                        .map_err(|_| Rich::custom(span, "invalid hour"))?;
                    if h < 24 {
                        Ok(h)
                    } else {
                        Err(Rich::custom(span, "hour out of range (0–23)"))
                    }
                })
                .then_ignore(just("."))
                .then(digits().try_map(|s, span| {
                    let m: u64 = s
                        .parse::<u64>()
                        .map_err(|_| Rich::custom(span, "invalid minute"))?;
                    if m < 60 {
                        Ok(m)
                    } else {
                        Err(Rich::custom(span, "minute out of range (0–59)"))
                    }
                }))
                .then_ignore(just("."))
                .then(digits().try_map(|s, span| {
                    let s: u64 = s
                        .parse::<u64>()
                        .map_err(|_| Rich::custom(span, "invalid second"))?;
                    if s < 60 {
                        Ok(s)
                    } else {
                        Err(Rich::custom(span, "second out of range (0–59)"))
                    }
                })),
        )
        .then(
            just("..")
                .ignore_then(
                    alphanumeric()
                        .separated_by(just("."))
                        .at_least(1)
                        .collect::<Vec<String>>(),
                )
                .or_not()
                .map(|opt| opt.unwrap_or_default()),
        )
        .try_map(|(((h, m), s), frags), span| {
            let mut fractions = Vec::new();

            for f in frags {
                let val = u16::from_str_radix(&f, 16)
                    .map_err(|_| Rich::custom(span, "invalid fraction digits"))?;
                fractions.push(val);
            }

            Ok((h, m, s, fractions))
        })
        .or_not()
        .map(|opt| opt.unwrap_or((0, 0, 0, Vec::new())));

    era_year
        .then(month)
        .then(day)
        .then(hour_min_secs_fractions)
        .map(|((((era, y), m), d), (hour, min, sec, f))| {
            ParsedAtom::Small(year(era, y, m, d, hour, min, sec, &f))
        })
}

fn unit_value_pair<'src>() -> impl Parser<'src, &'src str, (char, u64), Err<'src>> {
    one_of("dhms").then(decimal_without_leading_zero().try_map(|s, span| {
        s.parse::<u64>()
            .map_err(|_| Rich::custom(span, "Invalid Number"))
    }))
}

pub fn relative_date<'src>() -> impl Parser<'src, &'src str, ParsedAtom, Err<'src>> {
    let time_part = unit_value_pair()
        .separated_by(just('.'))
        .at_least(1)
        .collect::<Vec<(char, u64)>>();

    let hex_part = just("..")
        .ignore_then(
            any()
                .filter(|c: &char| c.is_ascii_hexdigit())
                .repeated()
                .exactly(4)
                .collect::<String>()
                .map(|s| u16::from_str_radix(&s, 16).unwrap_or(0))
                .separated_by(just('.'))
                .at_least(1)
                .collect::<Vec<u16>>(),
        )
        .or_not()
        .map(|v| v.unwrap_or_default());

    time_part
        .then(hex_part)
        .map(|(pairs, hex_vec): (Vec<(char, u64)>, Vec<u16>)| {
            let mut days = 0u64;
            let mut hours = 0u64;
            let mut minutes = 0u64;
            let mut seconds = 0u64;

            for (unit, value) in pairs {
                match unit {
                    'd' => days += value,
                    'h' => hours += value,
                    'm' => minutes += value,
                    's' => seconds += value,
                    _ => {}
                }
            }

            ParsedAtom::Small(yule(days, hours, minutes, seconds, &hex_vec))
        })
}

// ++year: date -> @da
pub fn year(a: bool, y: u64, m: u64, d: u64, h: u64, min: u64, s: u64, f: &[u16]) -> u128 {
    let yer = if a {
        YEAR_OFFSET + y
    } else {
        // (sub 292.277.024.400 (dec y))
        YEAR_OFFSET - (y - 1)
    };

    let day_count = yawn(yer, m, d);

    yule(day_count, h, min, s, f)
}

pub fn yell(now: &ParsedAtom) -> Tarp {
    let sec_atom = rsh(6, 1, now);

    let raw = end(6, 1, now);

    let mut fan = Vec::new();
    let mut muc = 4;
    let mut current_raw = raw.clone();

    while muc > 0 && !current_raw.is_zero() {
        muc -= 1;
        let digit_atom = cut(4, muc, 1, &current_raw);
        let digit: u64 = match &digit_atom {
            ParsedAtom::Small(x) => *x as u64,
            ParsedAtom::Big(b) => b.clone().try_into().unwrap_or(0),
        };
        fan.push(digit);

        current_raw = end(4, muc, &current_raw);
    }

    let sec_u64: u64 = match &sec_atom {
        ParsedAtom::Small(x) => *x as u64,
        ParsedAtom::Big(b) => b.clone().try_into().expect("yell: sec too large"),
    };

    let day = (sec_u64 / DAY) as u64;
    let sec = (sec_u64 % DAY) as u64;
    let hor = (sec / HOR) as u64;
    let sec = (sec % HOR) as u64;
    let mit = (sec / MIT) as u64;
    let sec = (sec % MIT) as u64;

    Tarp {
        d: day,
        h: hor,
        m: mit,
        s: sec,
        f: fan,
    }
}

pub fn yore(now: &ParsedAtom) -> Date {
    let rip: Tarp = yell(now);
    let (y_ger, m_ger, d_ger) = yall(rip.d);

    const PIVOT: u64 = 292_277_024_400;

    let (era, y_out) = if y_ger > PIVOT {
        (true, y_ger - PIVOT)
    } else {
        (false, PIVOT - y_ger)
    };

    Date {
        era,
        y: y_out,
        m: m_ger,
        t: Tarp {
            d: d_ger,
            h: rip.h,
            m: rip.m,
            s: rip.s,
            f: rip.f,
        },
    }
}

pub fn yall(day: u64) -> (u64, u64, u64) {
    let mut day = day;
    let mut era = 0;
    let mut cet = 0;
    let mut lep = false;

    // => .(era (div day era:yo), day (mod day era:yo))
    era = day / ERA;
    day %= ERA;

    // ?: (lth day +(cet:yo)) ...
    if day < CETY + 1 {
        lep = true;
        cet = 0;
    } else {
        lep = false;
        day = day - (CETY + 1);
        cet = 1 + (day / CETY);
        day %= CETY;
    }

    let mut yer = 400 * era + 100 * cet;

    // |- loop: subtract years
    loop {
        let dis = if lep { 366 } else { 365 };
        if day < dis {
            break;
        }
        let ner = yer + 1;
        day = day - dis;
        // lep =(0 (end [0 2] ner)) → is ner divisible by 4? (end [0 2] = lowest 2 bits)
        // end(0, 2, ner) = lowest 2 bits; =0 means divisible by 4
        lep = (ner & 3) == 0; // faster than atom ops
        yer = ner;
    }

    // month loop
    let cah = if lep { &MOY } else { &MOH };
    let mut mot = 0;
    loop {
        let zis = cah[mot as usize];
        if day < zis {
            return (yer, mot + 1, day + 1); // 1-based month/day
        }
        day -= zis;
        mot += 1;
    }
}

fn is_leap(year: u64) -> bool {
    (year % 4 == 0) && (year % 100 != 0 || year % 400 == 0)
}

pub fn is_leap_year(year: i32) -> bool {
    // Gregorian calendar proleptic
    (year % 4 == 0) && (year % 100 != 0 || year % 400 == 0)
}

pub fn yule(d: u64, h: u64, m: u64, s: u64, f: &[u16]) -> u128 {
    let sec = d * DAY + h * HOR + m * MIT + s;

    let mut fac: u64 = 0;
    let mut muc = 4i32; // starts at 4
    for &val in f.iter().take(4) {
        muc -= 1; // decrement *before* shift
        fac += (val as u64) << (muc as u32 * 16);
    }

    ((sec as u128) << 64) | (fac as u128)
}

fn bloq_bits(bloq: u32) -> u32 {
    if bloq >= 7 {
        panic!("bloq must be < 7 (max 64-bit chunks for u128)");
    }
    1 << bloq
}

pub fn met(bloq: usize, atom: &ParsedAtom) -> usize {
    let bits_per_block: usize = 1usize << bloq;

    match atom {
        ParsedAtom::Small(n) => {
            if *n == 0 {
                1
            } else {
                let atom_bits: usize = 128 - n.leading_zeros() as usize;
                (atom_bits + bits_per_block - 1) / bits_per_block
            }
        }
        ParsedAtom::Big(b) => {
            if b.is_zero() {
                1
            } else {
                let atom_bits: usize = b.bits() as usize;
                (atom_bits + bits_per_block - 1) / bits_per_block
            }
        }
    }
}

/// rep: assemble list of ParsedAtoms into one ParsedAtom using bite spec
///
/// - `bloq`: block size exponent (e.g. 3 → 8-bit blocks)
/// - `step_opt`: number of bloqs to take from each atom; if `None`, defaults to 1 (per Hoon ?^(a a [a *step]))
/// - `list`: slice of ParsedAtoms (representing Hoon `(list @)`)
///
/// Semantics:
///   result = Σ_i ( (atom_i & mask) << (i * chunk_bits) )
///   where mask = (1 << chunk_bits) - 1
pub fn rep(bloq: usize, step_opt: Option<usize>, list: &[ParsedAtom]) -> ParsedAtom {
    let step = step_opt.unwrap_or(1); // default step = 1

    let bloq_size = 1usize << bloq; // 2^bloq
    let chunk_bits = step * bloq_size; // bits per item

    if list.is_empty() || chunk_bits == 0 {
        return ParsedAtom::Small(0);
    }

    let mut result = BigUint::from(0u32);

    for (i, atom) in list.iter().enumerate() {
        let atom_bu = atom.to_biguint();

        let truncated = if chunk_bits < 128 {
            let mask = (1u128 << chunk_bits) - 1;
            let mask_bu = BigUint::from(mask);
            atom_bu & mask_bu
        } else {
            if atom_bu.bits() as usize <= chunk_bits {
                atom_bu
            } else {
                let mask = (BigUint::from(1u32) << chunk_bits) - 1u8;
                &atom_bu & mask
            }
        };

        let shifted = if i == 0 {
            truncated
        } else {
            truncated << (i * chunk_bits)
        };

        result += shifted;
    }

    ParsedAtom::Big(result)
}

pub fn rap(bloq: usize, chunks: &[u128]) -> ParsedAtom {
    if chunks.is_empty() {
        return ParsedAtom::Small(0);
    }

    let bits_per_bloq = bloq_bits(bloq as u32) as u64;
    let mut result = BigUint::zero();
    let mut shift = 0u64;

    for &chunk in chunks {
        let width_bloqs = met(bloq, &ParsedAtom::Small(chunk)) as u64;
        let width_bits = width_bloqs * bits_per_bloq;

        let mask = if width_bits >= 128 {
            u128::MAX
        } else {
            (1u128 << width_bits) - 1
        };
        if chunk & !mask != 0 {
            panic!("atom {:#x} too large for bloq {}", chunk, bloq);
        }

        let chunk_big = BigUint::from(chunk);
        result |= chunk_big << shift;

        shift += width_bits;

        if shift > 128 {}
    }

    // Now decide which variant to return
    if shift <= 128 {
        let value = result
            .to_u128()
            .expect("logic error: shift <=128 but not u128");
        ParsedAtom::Small(value)
    } else {
        ParsedAtom::Big(result)
    }
}

fn cut_u(v: u128, shift: usize, bits: usize) -> u8 {
    ((v >> shift) & ((1 << bits) - 1)) as u8
}

/// Extract `run` bloqs starting at bloq `start`, where each bloq is `2^bloq` bits.
pub fn cut(bloq: usize, start: usize, run: usize, atom: &ParsedAtom) -> ParsedAtom {
    if run == 0 {
        return ParsedAtom::Small(0);
    }

    let bloq_bits = match 1usize.checked_shl(bloq as u32) {
        Some(b) => b,
        None => return ParsedAtom::Small(0),
    };

    let bit_start = match start.checked_mul(bloq_bits) {
        Some(s) => s,
        None => return ParsedAtom::Small(0),
    };

    let bit_len = match run.checked_mul(bloq_bits) {
        Some(l) => l,
        None => return ParsedAtom::Small(0),
    };

    let src_bits = match atom {
        ParsedAtom::Small(0) => 0,
        ParsedAtom::Small(n) => (128 - n.leading_zeros()) as usize,
        ParsedAtom::Big(b) => b.bits() as usize,
    };

    if bit_start >= src_bits {
        return ParsedAtom::Small(0);
    }

    let bit_len = cmp::min(bit_len, src_bits - bit_start);
    if bit_len == 0 {
        return ParsedAtom::Small(0);
    }

    let shifted = match atom {
        ParsedAtom::Small(n) => {
            if bit_start >= 128 {
                ParsedAtom::Small(0)
            } else {
                ParsedAtom::Small(n >> bit_start)
            }
        }
        ParsedAtom::Big(b) => {
            if bit_start == 0 {
                atom.clone()
            } else {
                ParsedAtom::from_biguint(b >> bit_start)
            }
        }
    };

    match &shifted {
        ParsedAtom::Small(n) => {
            if bit_len >= 128 {
                shifted
            } else {
                let mask = (1u128 << bit_len) - 1;
                ParsedAtom::Small(*n & mask)
            }
        }
        ParsedAtom::Big(b) => {
            // b: &BigUint
            if bit_len <= 128 {
                // Extract low 128 bits manually (portable)
                let low_u128 = {
                    // Convert to u128, but preserve low bits even if truncated
                    // u128::try_from returns Err for >u128::MAX, but we want modulo 2^128
                    // So: take first 2 u64 limbs
                    let mut limbs = b.iter_u64_digits();
                    let lo = limbs.next().unwrap_or(0);
                    let hi = limbs.next().unwrap_or(0);
                    ((hi as u128) << 64) | (lo as u128)
                };
                let mask = if bit_len == 128 {
                    u128::MAX
                } else {
                    (1u128 << bit_len) - 1
                };
                ParsedAtom::Small(low_u128 & mask)
            } else {
                // Big mask: (1 << bit_len) - 1
                let mask = (BigUint::one() << bit_len) - BigUint::one();
                // Use & with references to avoid move
                let masked = b & &mask; // &BigUint & &BigUint → BigUint
                ParsedAtom::from_biguint(masked)
            }
        }
    }
}

pub fn lsh(bloq: usize, step: usize, atom: &ParsedAtom) -> ParsedAtom {
    let bits = match step.checked_mul(1usize << bloq) {
        Some(b) => b,
        None => return ParsedAtom::Small(0),
    };
    atom_shl(atom, bits)
}

pub fn rsh(bloq: usize, step: usize, atom: &ParsedAtom) -> ParsedAtom {
    let bits = match step.checked_mul(1usize << bloq) {
        Some(b) => b,
        None => return ParsedAtom::Small(0),
    };
    atom_shr(atom, bits)
}

fn lsh_u128(bloq: usize, step: usize, atom: u128) -> u128 {
    let bits = step.checked_mul(1 << bloq).unwrap_or(128);
    if bits >= 128 {
        0
    } else {
        atom << bits
    }
}

fn rsh_u128(bloq: usize, step: usize, atom: u128) -> u128 {
    let bits = step.checked_mul(1 << bloq).unwrap_or(128);
    if bits >= 128 {
        0
    } else {
        atom >> bits
    }
}

fn lsh_big(bloq: usize, step: usize, atom: &BigUint) -> BigUint {
    let bits = step.checked_mul(1 << bloq).unwrap_or(usize::MAX);
    if bits == 0 {
        atom.clone()
    } else {
        atom << bits
    }
}

fn rsh_big(bloq: usize, step: usize, atom: &BigUint) -> BigUint {
    let bits = step.checked_mul(1 << bloq).unwrap_or(usize::MAX);
    if bits == 0 {
        atom.clone()
    } else {
        atom >> bits
    }
}

fn end(bloq: usize, step: usize, atom: &ParsedAtom) -> ParsedAtom {
    let total_bits = match step.checked_mul(1usize << bloq) {
        Some(b) => b,
        None => return ParsedAtom::Small(0),
    };
    atom_mask_low_bits(atom, total_bits)
}

fn end_big(bloq: usize, step: usize, atom: &BigUint) -> BigUint {
    let total_bits = match step.checked_mul(1usize << bloq) {
        Some(b) => b as u128,
        None => return BigUint::zero(),
    };
    if total_bits == 0 {
        return BigUint::zero();
    }
    let mask = (BigUint::one() << total_bits) - BigUint::one();
    atom & &mask
}

fn end_u128(bloq: usize, step: usize, atom: u128) -> u128 {
    let total_bits = match step.checked_mul(1usize << bloq) {
        Some(b) => b as u128,
        None => return 0,
    };
    if total_bits >= 128 {
        atom
    } else {
        let mask = (1u128 << total_bits) - 1;
        atom & mask
    }
}

pub const SIS: [[u8; 3]; 256] = [
    *b"doz", *b"mar", *b"bin", *b"wan", *b"sam", *b"lit", *b"sig", *b"hid", *b"fid", *b"lis",
    *b"sog", *b"dir", *b"wac", *b"sab", *b"wis", *b"sib", *b"rig", *b"sol", *b"dop", *b"mod",
    *b"fog", *b"lid", *b"hop", *b"dar", *b"dor", *b"lor", *b"hod", *b"fol", *b"rin", *b"tog",
    *b"sil", *b"mir", *b"hol", *b"pas", *b"lac", *b"rov", *b"liv", *b"dal", *b"sat", *b"lib",
    *b"tab", *b"han", *b"tic", *b"pid", *b"tor", *b"bol", *b"fos", *b"dot", *b"los", *b"dil",
    *b"for", *b"pil", *b"ram", *b"tir", *b"win", *b"tad", *b"bic", *b"dif", *b"roc", *b"wid",
    *b"bis", *b"das", *b"mid", *b"lop", *b"ril", *b"nar", *b"dap", *b"mol", *b"san", *b"loc",
    *b"nov", *b"sit", *b"nid", *b"tip", *b"sic", *b"rop", *b"wit", *b"nat", *b"pan", *b"min",
    *b"rit", *b"pod", *b"mot", *b"tam", *b"tol", *b"sav", *b"pos", *b"nap", *b"nop", *b"som",
    *b"fin", *b"fon", *b"ban", *b"mor", *b"wor", *b"sip", *b"ron", *b"nor", *b"bot", *b"wic",
    *b"soc", *b"wat", *b"dol", *b"mag", *b"pic", *b"dav", *b"bid", *b"bal", *b"tim", *b"tas",
    *b"mal", *b"lig", *b"siv", *b"tag", *b"pad", *b"sal", *b"div", *b"dac", *b"tan", *b"sid",
    *b"fab", *b"tar", *b"mon", *b"ran", *b"nis", *b"wol", *b"mis", *b"pal", *b"las", *b"dis",
    *b"map", *b"rab", *b"tob", *b"rol", *b"lat", *b"lon", *b"nod", *b"nav", *b"fig", *b"nom",
    *b"nib", *b"pag", *b"sop", *b"ral", *b"bil", *b"had", *b"doc", *b"rid", *b"moc", *b"pac",
    *b"rav", *b"rip", *b"fal", *b"tod", *b"til", *b"tin", *b"hap", *b"mic", *b"fan", *b"pat",
    *b"tac", *b"lab", *b"mog", *b"sim", *b"son", *b"pin", *b"lom", *b"ric", *b"tap", *b"fir",
    *b"has", *b"bos", *b"bat", *b"poc", *b"hac", *b"tid", *b"hav", *b"sap", *b"lin", *b"dib",
    *b"hos", *b"dab", *b"bit", *b"bar", *b"rac", *b"par", *b"lod", *b"dos", *b"bor", *b"toc",
    *b"hil", *b"mac", *b"tom", *b"dig", *b"fil", *b"fas", *b"mit", *b"hob", *b"har", *b"mig",
    *b"hin", *b"rad", *b"mas", *b"hal", *b"rag", *b"lag", *b"fad", *b"top", *b"mop", *b"hab",
    *b"nil", *b"nos", *b"mil", *b"fop", *b"fam", *b"dat", *b"nol", *b"din", *b"hat", *b"nac",
    *b"ris", *b"fot", *b"rib", *b"hoc", *b"nim", *b"lar", *b"fit", *b"wal", *b"rap", *b"sar",
    *b"nal", *b"mos", *b"lan", *b"don", *b"dan", *b"lad", *b"dov", *b"riv", *b"bac", *b"pol",
    *b"lap", *b"tal", *b"pit", *b"nam", *b"bon", *b"ros", *b"ton", *b"fod", *b"pon", *b"sov",
    *b"noc", *b"sor", *b"lav", *b"mat", *b"mip", *b"fip",
];

pub const DEX: [[u8; 3]; 256] = [
    *b"zod", *b"nec", *b"bud", *b"wes", *b"sev", *b"per", *b"sut", *b"let", *b"ful", *b"pen",
    *b"syt", *b"dur", *b"wep", *b"ser", *b"wyl", *b"sun", *b"ryp", *b"syx", *b"dyr", *b"nup",
    *b"heb", *b"peg", *b"lup", *b"dep", *b"dys", *b"put", *b"lug", *b"hec", *b"ryt", *b"tyv",
    *b"syd", *b"nex", *b"lun", *b"mep", *b"lut", *b"sep", *b"pes", *b"del", *b"sul", *b"ped",
    *b"tem", *b"led", *b"tul", *b"met", *b"wen", *b"byn", *b"hex", *b"feb", *b"pyl", *b"dul",
    *b"het", *b"mev", *b"rut", *b"tyl", *b"wyd", *b"tep", *b"bes", *b"dex", *b"sef", *b"wyc",
    *b"bur", *b"der", *b"nep", *b"pur", *b"rys", *b"reb", *b"den", *b"nut", *b"sub", *b"pet",
    *b"rul", *b"syn", *b"reg", *b"tyd", *b"sup", *b"sem", *b"wyn", *b"rec", *b"meg", *b"net",
    *b"sec", *b"mul", *b"nym", *b"tev", *b"web", *b"sum", *b"mut", *b"nyx", *b"rex", *b"teb",
    *b"fus", *b"hep", *b"ben", *b"mus", *b"wyx", *b"sym", *b"sel", *b"ruc", *b"dec", *b"wex",
    *b"syr", *b"wet", *b"dyl", *b"myn", *b"mes", *b"det", *b"bet", *b"bel", *b"tux", *b"tug",
    *b"myr", *b"pel", *b"syp", *b"ter", *b"meb", *b"set", *b"dut", *b"deg", *b"tex", *b"sur",
    *b"fel", *b"tud", *b"nux", *b"rux", *b"ren", *b"wyt", *b"nub", *b"med", *b"lyt", *b"dus",
    *b"neb", *b"rum", *b"tyn", *b"seg", *b"lyx", *b"pun", *b"res", *b"red", *b"fun", *b"rev",
    *b"ref", *b"mec", *b"ted", *b"rus", *b"bex", *b"leb", *b"dux", *b"ryn", *b"num", *b"pyx",
    *b"ryg", *b"ryx", *b"fep", *b"tyr", *b"tus", *b"tyc", *b"leg", *b"nem", *b"fer", *b"mer",
    *b"ten", *b"lus", *b"nus", *b"syl", *b"tec", *b"mex", *b"pub", *b"rym", *b"tuc", *b"fyl",
    *b"lep", *b"deb", *b"ber", *b"mug", *b"hut", *b"tun", *b"byl", *b"sud", *b"pem", *b"dev",
    *b"lur", *b"def", *b"bus", *b"bep", *b"run", *b"mel", *b"pex", *b"dyt", *b"byt", *b"typ",
    *b"lev", *b"myl", *b"wed", *b"duc", *b"fur", *b"fex", *b"nul", *b"luc", *b"len", *b"ner",
    *b"lex", *b"rup", *b"ned", *b"lec", *b"ryd", *b"lyd", *b"fen", *b"wel", *b"nyd", *b"hus",
    *b"rel", *b"rud", *b"nes", *b"hes", *b"fet", *b"des", *b"ret", *b"dun", *b"ler", *b"nyr",
    *b"seb", *b"hul", *b"ryl", *b"lud", *b"rem", *b"lys", *b"fyn", *b"wer", *b"ryc", *b"sug",
    *b"nys", *b"nyl", *b"lyn", *b"dyn", *b"dem", *b"lux", *b"fed", *b"sed", *b"bec", *b"mun",
    *b"lyr", *b"tes", *b"mud", *b"nyt", *b"byr", *b"sen", *b"weg", *b"fyr", *b"mur", *b"tel",
    *b"rep", *b"teg", *b"pec", *b"nel", *b"nev", *b"fes",
];

/// Fetch prefix syllable (Hoon ++tos)
pub fn tos_po(i: u8) -> ParsedAtom {
    let b = SIS[i as usize];
    ParsedAtom::Small((b[0] as u128) | ((b[1] as u128) << 8) | ((b[2] as u128) << 16))
}

/// Fetch suffix syllable (Hoon ++tod)
pub fn tod_po(i: u8) -> ParsedAtom {
    let b = DEX[i as usize];
    ParsedAtom::Small((b[0] as u128) | ((b[1] as u128) << 8) | ((b[2] as u128) << 16))
}

/// Linear prefix search (Hoon ++ins)
pub fn ins(a: &[u8]) -> Option<u8> {
    if a.len() != 3 {
        return None;
    }

    let key = [a[0], a[1], a[2]];

    for (i, entry) in SIS.iter().enumerate() {
        if *entry == key {
            return Some(i as u8);
        }
    }

    None
}

/// Linear suffix search (Hoon ++ind)
pub fn ind(a: &[u8]) -> Option<u8> {
    if a.len() != 3 {
        return None;
    }

    let key = [a[0], a[1], a[2]];

    for (i, entry) in DEX.iter().enumerate() {
        if *entry == key {
            return Some(i as u8);
        }
    }

    None
}

// +tip:ab
pub fn tip<'src>() -> impl Parser<'src, &'src str, u8, Err<'src>> {
    any()
        .filter(|c: &char| c.is_ascii_lowercase())
        .repeated()
        .exactly(3)
        .collect::<String>()
        .try_map(|s, span| match ins(s.as_bytes()) {
            Some(i) => Ok(i),
            None => Err(Rich::custom(span, format!("invalid prefix syllable '{s}'"))),
        })
        .labelled("Phonetic Prefix")
}

// +tiq:ab
pub fn tiq<'src>() -> impl Parser<'src, &'src str, u8, Err<'src>> {
    any()
        .filter(|c: &char| c.is_ascii_lowercase())
        .repeated()
        .exactly(3)
        .collect::<String>()
        .try_map(|s, span| match ind(s.as_bytes()) {
            Some(i) => Ok(i),
            None => Err(Rich::custom(span, format!("invalid suffix syllable '{s}'"))),
        })
        .labelled("Phonetic Suffix")
}

// +hif:ab
pub fn hif<'src>() -> impl Parser<'src, &'src str, u16, Err<'src>> {
    tip()
        .then(tiq())
        .try_map(|(p, q), span| Ok((p as u16) * 256 + (q as u16)))
}

pub fn phonemic_name<'src>() -> impl Parser<'src, &'src str, ParsedAtom, Err<'src>> {
    let tep = any()
        .filter(|c: &char| c.is_ascii_lowercase())
        .repeated()
        .exactly(3)
        .to_slice()
        .try_map(|s: &str, span| {
            if s == "doz" {
                return Err(Rich::custom(span, "prefix 'doz' is forbidden"));
            }
            match ins(s.as_bytes()) {
                Some(i) => Ok(i),
                None => Err(Rich::custom(span, format!("invalid prefix syllable '{s}'"))),
            }
        })
        .labelled("Phonetic Prefix");
    let hef = tip()
        .then(tiq())
        .try_map(|(p, q), span| {
            let val = (p as u16) * 256 + (q as u16);
            if val == 0 {
                Err(Rich::custom(span, format!("phonetic is zero")))
            } else {
                Ok(val)
            }
        })
        .boxed();
    let huf = hef
        .clone() // u16
        .then(
            just('-')
                .ignore_then(hif()) // u16
                .repeated()
                .at_most(3)
                .collect::<Vec<_>>(),
        )
        .map(|(first, rest)| std::iter::once(first).chain(rest).collect::<Vec<_>>())
        .map(|hefs: Vec<u16>| {
            let mut acc = BigUint::from(0u32);
            for &digit in &hefs {
                acc = (acc << 16) + BigUint::from(digit);
            }
            acc
        });
    let hyf = hif()
        .separated_by(just('-'))
        .exactly(4)
        .collect::<Vec<_>>()
        .map(|hefs: Vec<u16>| {
            let mut acc = BigUint::from(0u32);
            for &digit in &hefs {
                acc = (acc << 16) + BigUint::from(digit);
            }
            acc
        });
    let other = huf
        .then(
            just("--")
                .ignore_then(gap().or_not())
                .ignore_then(hyf)
                .repeated()
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .map(|(first, rest)| std::iter::once(first).chain(rest).collect::<Vec<_>>())
        .map(|hefs: Vec<BigUint>| {
            let acc = hefs
                .iter()
                .fold(BigUint::from(0u32), |acc, d| (acc << 64) + d);
            ParsedAtom::Big(fynd_big(&acc))
        });
    let planet_moon = hef
        .then(
            just('-')
                .ignore_then(hif())
                .repeated()
                .at_least(1)
                .at_most(3)
                .collect::<Vec<_>>(),
        )
        .map(|(first, rest)| std::iter::once(first).chain(rest).collect::<Vec<_>>())
        .map(|hefs: Vec<u16>| {
            let mut acc = BigUint::zero();
            for &digit in &hefs {
                acc = (acc << 16) + BigUint::from(digit as u32);
            }
            ParsedAtom::Big(fynd_big(&acc))
        });
    let star = tep.then(tiq()).try_map(|(p, q), span| {
        let x = (p as u16) * 256 + (q as u16);
        Ok(ParsedAtom::Small(x as u128))
    });
    let galaxy = tiq().map(|p| ParsedAtom::Small(p.into()));

    choice((
        other.labelled("Long Phonemic"),
        planet_moon.labelled("Planet or Moon"),
        star.labelled("Star"),
        galaxy.labelled("Galaxy"),
    ))
}

pub fn phonemic_name_unscrambled<'src>() -> impl Parser<'src, &'src str, ParsedAtom, Err<'src>> {
    hif()
        .or(tiq().map(|i| i as u16))
        .then(
            just('-')
                .ignore_then(gap().or_not())
                .ignore_then(hif())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .map(|(first, rest)| {
            std::iter::once(first)
                .chain(rest)
                .map(ParsedAtom::from)
                .collect::<Vec<ParsedAtom>>()
        })
        .map(|mut hifs| {
            hifs.reverse();
            rep(4, None, &hifs)
        })
}

fn dis_big(x: &BigUint, mask: &BigUint) -> BigUint {
    x & mask
}

// fn dis(x: u64, mask: u64) -> u64 {
fn dis<T: Copy + BitAnd<Output = T>>(x: T, mask: T) -> T {
    x & mask
}

fn con(hi: u64, lo: u64) -> u64 {
    hi | lo
}

fn con_atoms(hi: ParsedAtom, lo: ParsedAtom) -> ParsedAtom {
    match (hi, lo) {
        (ParsedAtom::Small(a), ParsedAtom::Small(b)) => ParsedAtom::Small(a | b),
        (a, b) => {
            let x = a.to_biguint();
            let y = b.to_biguint();
            ParsedAtom::from_biguint(x | y)
        }
    }
}

fn mix(x: u64, y: u64) -> u64 {
    x ^ y
}

fn mix_big(x: &BigUint, y: &BigUint) -> BigUint {
    x ^ y
}

fn mix_atoms(a: ParsedAtom, b: ParsedAtom) -> ParsedAtom {
    match (a, b) {
        (ParsedAtom::Small(x), ParsedAtom::Small(y)) => ParsedAtom::Small(x ^ y),
        (a, b) => {
            let x = a.to_biguint();
            let y = b.to_biguint();
            ParsedAtom::from_biguint(&x ^ &y)
        }
    }
}

const RAKU: [u32; 4] = [0xb76d_5eed, 0xee28_1300, 0x85bc_ae01, 0x4b38_7af7];
#[inline]
fn rol32(x: u32, r: u32) -> u32 {
    x.rotate_left(r)
}

#[inline]
fn fmix32(mut h: u32) -> u32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^= h >> 16;
    h
}

fn muk(seed: u32, len: u32, key: u64) -> u32 {
    let c1: u32 = 0xcc9e_2d51;
    let c2: u32 = 0x1b87_3593;

    let mut data = vec![0u8; len as usize];
    let mut k = key;
    for i in 0..len as usize {
        data[i] = (k & 0xff) as u8;
        k >>= 8;
    }

    let nblocks = (len / 4) as usize; // intentionally off-by-one
    let mut h1 = seed;

    let mut blocks = Vec::new();
    for i in 0..nblocks {
        let mut v = 0u32;
        for j in 0..4 {
            let idx = i * 4 + j;
            if idx < data.len() {
                v |= (data[idx] as u32) << (8 * j);
            }
        }
        blocks.push(v);
    }

    let mut i = nblocks;
    while i > 0 {
        let mut k1 = blocks[nblocks - i];
        k1 = k1.wrapping_mul(c1);
        k1 = k1.rotate_left(15);
        k1 = k1.wrapping_mul(c2);

        h1 ^= k1;
        h1 = h1.rotate_left(13);
        h1 = h1.wrapping_mul(5).wrapping_add(0xe654_6b64);
        i -= 1;
    }

    let tail = &data[(nblocks * 4)..];
    let mut k1 = 0u32;

    match len & 3 {
        3 => {
            k1 ^= (tail[2] as u32) << 16;
            k1 ^= (tail[1] as u32) << 8;
            k1 ^= tail[0] as u32;
            k1 = k1.wrapping_mul(c1);
            k1 = k1.rotate_left(15);
            k1 = k1.wrapping_mul(c2);
            h1 ^= k1;
        }
        2 => {
            k1 ^= (tail[1] as u32) << 8;
            k1 ^= tail[0] as u32;
            k1 = k1.wrapping_mul(c1);
            k1 = k1.rotate_left(15);
            k1 = k1.wrapping_mul(c2);
            h1 ^= k1;
        }
        1 => {
            k1 ^= tail[0] as u32;
            k1 = k1.wrapping_mul(c1);
            k1 = k1.rotate_left(15);
            k1 = k1.wrapping_mul(c2);
            h1 ^= k1;
        }
        _ => {}
    }

    h1 ^= len;
    fmix32(h1)
}

fn eff(j: u64, r: u64) -> u64 {
    let seed = RAKU[(j as usize) & 3];
    muk(seed, 2, r) as u64
}

fn fen(r: u64, a: u64, b: u64, m: u64) -> u64 {
    let mut j = r;

    let (ahh, ale) = if r % 2 == 0 {
        (m % a, m / a)
    } else {
        (m / a, m % a)
    };

    let (mut ell, mut arr) = if ale == a { (ahh, ale) } else { (ale, ahh) };

    while j >= 1 {
        let f = eff(j - 1, ell);

        let tmp = if j % 2 != 0 {
            (arr + a - (f % a)) % a
        } else {
            (arr + b - (f % b)) % b
        };

        j -= 1;
        arr = ell;
        ell = tmp;
    }

    &arr * a + ell
}

fn fe<F>(r: u64, a: &ParsedAtom, b: &ParsedAtom, prf: &F, m: &ParsedAtom) -> ParsedAtom
where
    F: Fn(u64, &ParsedAtom) -> ParsedAtom,
{
    let mut j: u64 = 1;
    let mut ell = end(0, met(0, a), m); // m mod a = lowest (bitlen a) bits of m
    let mut arr = rsh(0, met(0, a), m); // m div a = m >> (bitlen a)

    loop {
        if j > r {
            if r % 2 == 1 {
                let shifted = match &arr {
                    ParsedAtom::Small(n) => {
                        let shifted_n = n.checked_shl(16).unwrap_or(0);
                        ParsedAtom::Small(shifted_n) | ell.clone()
                    }
                    ParsedAtom::Big(big) => {
                        let shifted = (big.clone() << 16) + ell.to_biguint();
                        ParsedAtom::Big(shifted)
                    }
                };
                return shifted;
            } else {
                // even rounds
                if arr.eq(a) {
                    let a_bits = met(0, a);
                    let shifted_a = rsh(0, 0, a); // identity
                    let shifted = match a {
                        ParsedAtom::Small(n) => {
                            let shifted_n = n.checked_shl(a_bits as u32).unwrap_or(0);
                            ParsedAtom::Small(shifted_n) | ell.clone()
                        }
                        ParsedAtom::Big(big) => {
                            let shifted = (big.clone() << a_bits) + ell.to_biguint();
                            ParsedAtom::Big(shifted)
                        }
                    };
                    return shifted;
                } else {
                    let a_bits = met(0, a);
                    let shifted = match &ell {
                        ParsedAtom::Small(n) => {
                            let shifted_n = n.checked_shl(a_bits as u32).unwrap_or(0);
                            ParsedAtom::Small(shifted_n) | arr.clone()
                        }
                        ParsedAtom::Big(big) => {
                            let shifted = (big.clone() << a_bits) + arr.to_biguint();
                            ParsedAtom::Big(shifted)
                        }
                    };
                    return shifted;
                }
            }
        }

        let f = prf(j - 1, &arr);

        let modulus = if j % 2 == 1 { a } else { b };
        let sum = match (&f, &ell) {
            (ParsedAtom::Small(x), ParsedAtom::Small(y)) => ParsedAtom::Small(x.wrapping_add(*y)),
            _ => {
                let bx = f.to_biguint();
                let by = ell.to_biguint();
                ParsedAtom::Big(&bx + &by)
            }
        };
        let tmp = end(0, met(0, modulus), &sum); // sum mod modulus

        ell = arr;
        arr = tmp;
        j += 1;
    }
}

pub fn feis(m: ParsedAtom) -> ParsedAtom {
    debug_assert!(m.lt(&ParsedAtom::Small(0xffff_0000))); // domain guarantee
    let m_u64 = m.to_u64_lossy();
    let a = 0xffffu64;
    let b = 0x1_0000u64;
    let k = a * b; // 0xffff_0000

    let mut c = fe_u64(4, a, b, |j, r| eff(j, r), m_u64);
    while c >= k {
        c = fe_u64(4, a, b, |j, r| eff(j, r), c);
    }
    ParsedAtom::Small(c as u128)
}

fn fe_u64(r: u64, a: u64, b: u64, prf: impl Fn(u64, u64) -> u64, m: u64) -> u64 {
    let mut j = 1u64;
    let mut ell = m % a;
    let mut arr = m / a;

    loop {
        if j > r {
            return if r % 2 == 1 {
                arr * a + ell
            } else if arr == a {
                arr * a + ell
            } else {
                ell * a + arr
            };
        }

        let f = prf(j - 1, arr);
        let tmp = if j % 2 == 1 {
            (f + ell) % a
        } else {
            (f + ell) % b
        };

        ell = arr;
        arr = tmp;
        j += 1;
    }
}

fn feen(r: u64, a: u64, b: u64, k: u64, m: u64) -> u64 {
    let c = fen(r, a, b, m);
    if c < k.into() {
        c
    } else {
        fen(r, a, b, c)
    }
}

pub fn fein(pyn: ParsedAtom) -> ParsedAtom {
    let lower_16 = ParsedAtom::Small(0x1_0000);
    let upper_16 = ParsedAtom::Small(0xffff_ffff);
    let lower_32 = ParsedAtom::Small(0x1_0000_0000);
    let upper_32 = ParsedAtom::Small(0xffff_ffff_ffff_ffff);

    if pyn.ge(&lower_16) && pyn.le(&upper_16) {
        let offset = match (&pyn, &lower_16) {
            (ParsedAtom::Small(x), ParsedAtom::Small(y)) => ParsedAtom::Small(x - y),
            _ => ParsedAtom::Big(&pyn.to_biguint() - &lower_16.to_biguint()),
        };
        let feised = feis(offset);
        match (&feised, &lower_16) {
            (ParsedAtom::Small(x), ParsedAtom::Small(y)) => ParsedAtom::Small(x + y),
            _ => ParsedAtom::Big(&feised.to_biguint() + &lower_16.to_biguint()),
        }
    } else if pyn.ge(&lower_32) && pyn.le(&upper_32) {
        let mask_lo = ParsedAtom::Small(0xffff_ffff);
        let lo = match (&pyn, &mask_lo) {
            (ParsedAtom::Small(x), ParsedAtom::Small(m)) => ParsedAtom::Small(dis(*x, *m)),
            _ => ParsedAtom::Big(dis_big(&pyn.to_biguint(), &mask_lo.to_biguint())),
        };

        let mask_hi = ParsedAtom::Small(0xffff_ffff_0000_0000);
        let hi = match (&pyn, &mask_hi) {
            (ParsedAtom::Small(x), ParsedAtom::Small(m)) => ParsedAtom::Small(dis(*x, *m)),
            _ => ParsedAtom::Big(dis_big(&pyn.to_biguint(), &mask_hi.to_biguint())),
        };

        let feined_lo = fein(lo);
        con_atoms(hi, feined_lo)
    } else {
        pyn
    }
}

fn tail(m: u64) -> u64 {
    feen(4, 0xffff, 0x1_0000, 0xffff * 0x1_0000, m)
}

fn fynd_big(cry: &BigUint) -> BigUint {
    let one_16 = BigUint::from(0x1_0000u32);
    let max_32 = BigUint::from(0xffff_ffffu32);
    let one_32 = BigUint::from(0x1_0000_0000u64);
    let max_64 = BigUint::from(u64::MAX);

    if cry >= &one_16 && cry <= &max_32 {
        let x = cry.to_u64().expect("32-bit value should fit in u64");
        return BigUint::from(fynd_u64(x));
    }

    if cry >= &one_32 && cry <= &max_64 {
        let lo = cry & &max_32;
        let hi = cry - &lo;
        let lo_f = BigUint::from(fynd_u64(
            lo.to_u64().expect("32-bit low word should fit u64"),
        ));
        return hi + lo_f;
    }

    cry.clone()
}

pub fn fynd_u64(cry: u64) -> u64 {
    if cry >= 0x1_0000 && cry <= 0xffff_ffff {
        return 0x1_0000 + tail(cry - 0x1_0000);
    }

    if cry >= 0x1_0000_0000 {
        let lo = dis(cry, 0xffff_ffff);
        let hi = dis(cry, 0xffff_ffff_0000_0000);
        return con(hi, fynd_u64(lo));
    }

    cry
}

pub fn twid<'src>() -> impl Parser<'src, &'src str, Coin, Err<'src>> {
    choice((
        just('0').ignore_then(base32()).try_map(|s, span| {
            let atom = base32_to_atom(s);
            cue_simple(atom)
                .map(Coin::Blob)
                .map_err(|e| Rich::custom(span, format!("Failed to +cue: {}", e)))
        }),
        crub(),
    ))
}

pub fn cue_simple(buffer: ParsedAtom) -> Result<NounExpr, Box<dyn std::error::Error>> {
    let bits = atom_to_bits(&buffer);
    let mut backrefs = HashMap::new();
    let (noun, _) = cue_inner(&bits, 0, &mut backrefs)?;
    Ok(noun)
}

fn noun_hash(noun: &NounExpr) -> u64 {
    let mut hasher = DefaultHasher::new();
    noun.hash(&mut hasher);
    hasher.finish()
}

pub fn jam_simple(noun: NounExpr) -> ParsedAtom {
    let mut bits = Vec::new();
    let mut backrefs = HashMap::new();
    let mut stack = vec![noun];

    while let Some(current) = stack.pop() {
        if let Some(&offset) = backrefs.get(&current) {
            let use_backref = match &current {
                NounExpr::ParsedAtom(atom) => {
                    let atom_bits = mat_bits(atom).len();
                    let offset_bits = mat_bits(&offset_to_atom(offset)).len();
                    offset_bits < atom_bits
                }
                NounExpr::Cell(_, _) => true,
            };

            if use_backref {
                bits.push(true);
                bits.push(true);
                bits.extend(mat_bits(&offset_to_atom(offset)));
                continue;
            }
        }

        let offset = bits.len();
        backrefs.insert(current.clone(), offset);

        match current {
            NounExpr::ParsedAtom(atom) => {
                bits.push(false);
                bits.extend(mat_bits(&atom));
            }
            NounExpr::Cell(head, tail) => {
                bits.push(true);
                bits.push(false);
                stack.push(*tail);
                stack.push(*head);
            }
        }
    }

    bits_to_atom(&bits)
}

fn offset_to_atom(offset: usize) -> ParsedAtom {
    if offset <= u128::MAX as usize {
        ParsedAtom::Small(offset as u128)
    } else {
        ParsedAtom::Big(BigUint::from(offset))
    }
}

fn mat_bits(atom: &ParsedAtom) -> Vec<bool> {
    let n = atom_bit_len(atom); // = met0(atom): number of bits needed to represent the atom

    let mut bits = Vec::new();

    if n == 0 {
        bits.push(true);
        return bits;
    }

    let k = usize_bit_len(n); // met0(n)

    bits.extend(std::iter::repeat(false).take(k));

    bits.push(true);

    if k > 1 {
        let offset = n - (1usize << (k - 1)); // same as n & ((1 << (k-1)) - 1)
        for i in 0..(k - 1) {
            bits.push((offset >> i) & 1 == 1);
        }
    }

    for i in 0..n {
        bits.push(atom_get_bit(atom, i as u64));
    }

    bits
}

fn usize_bit_len(x: usize) -> usize {
    if x == 0 {
        1
    } else {
        (usize::BITS - x.leading_zeros()) as usize
    }
}

fn atom_bit_len(atom: &ParsedAtom) -> usize {
    match atom {
        ParsedAtom::Small(0) => 0,
        ParsedAtom::Small(x) => (128 - x.leading_zeros() as usize),
        ParsedAtom::Big(x) => x.bits() as usize,
    }
}

fn atom_get_bit(atom: &ParsedAtom, i: u64) -> bool {
    match atom {
        ParsedAtom::Small(x) => i < 128 && ((x >> i) & 1 == 1),
        ParsedAtom::Big(x) => {
            let byte_index = (i / 8) as usize;
            let bit_index = (i % 8) as u8;
            let bytes = x.to_bytes_le();
            if byte_index < bytes.len() {
                let byte = bytes[byte_index];
                (byte >> bit_index) & 1 == 1
            } else {
                false
            }
        }
    }
}

fn bits_to_atom(bits: &[bool]) -> ParsedAtom {
    if bits.is_empty() {
        return ParsedAtom::Small(0);
    }

    let len = bits.len();

    if len <= 128 {
        let mut val: u128 = 0;
        for (i, &bit) in bits.iter().enumerate() {
            if bit {
                val |= 1u128 << i;
            }
        }
        ParsedAtom::Small(val)
    } else {
        let mut big = BigUint::from(0u32);
        for (i, &bit) in bits.iter().enumerate() {
            if bit {
                big += BigUint::from(1u32) << i;
            }
        }
        ParsedAtom::Big(big)
    }
}

#[derive(Debug)]
enum ParseAction {
    Start(u64),                       // start parsing noun at cursor
    CellHeadDone(u64, Box<NounExpr>), // head done, now parse tail at given cursor
    FinishCell(Box<NounExpr>, Box<NounExpr>),
    StoreBackref(u64),
}
fn rub_backref(bits: &[bool], cursor: &mut usize) -> Result<u64, Box<dyn std::error::Error>> {
    let size = get_size(bits, cursor)?;
    if size == 0 {
        return Ok(0);
    }
    if size > 64 {
        return Err("backref offset too large (>64 bits)".into());
    }
    if *cursor + size as usize > bits.len() {
        return Err("not enough bits for backref".into());
    }

    let mut val: u64 = 0;
    for i in 0..size {
        if bits[*cursor + i as usize] {
            val |= 1u64 << i;
        }
    }
    *cursor += size as usize;
    Ok(val)
}

fn rub_atom(bits: &[bool], cursor: &mut usize) -> Result<ParsedAtom, Box<dyn std::error::Error>> {
    let size = get_size(bits, cursor)?;

    if size == 0 {
        return Ok(ParsedAtom::Small(0));
    }

    if *cursor + size as usize > bits.len() {
        return Err("not enough bits for rub atom payload".into());
    }

    // Read `size` bits, LSB-first → value = sum bit_i * 2^i
    if size <= 128 {
        let mut val: u128 = 0;
        for i in 0..size {
            if bits[*cursor + i as usize] {
                val |= 1u128 << i;
            }
        }
        *cursor += size as usize;
        Ok(ParsedAtom::Small(val))
    } else {
        // Use BigUint
        let mut big = BigUint::from(0u32);
        for i in 0..size {
            if bits[*cursor + i as usize] {
                big += BigUint::from(1u32) << i;
            }
        }
        *cursor += size as usize;
        Ok(ParsedAtom::Big(big))
    }
}

fn get_size(bits: &[bool], cursor: &mut usize) -> Result<u64, &'static str> {
    let start = *cursor;
    // Count leading zeros
    while *cursor < bits.len() && !bits[*cursor] {
        *cursor += 1;
    }

    if *cursor >= bits.len() {
        return Err("unexpected EOF in rub size prefix");
    }

    let c = (*cursor - start) as u32; // number of leading zeros
    *cursor += 1; // consume the '1'

    if c == 0 {
        Ok(0)
    } else {
        // Read c-1 bits
        if *cursor + (c - 1) as usize > bits.len() {
            return Err("not enough bits for rub size field");
        }

        let mut x = 0u64;
        for i in 0..(c - 1) {
            if bits[*cursor + i as usize] {
                x |= 1u64 << i; // LSB-first: first bit = 2^0
            }
        }
        *cursor += (c - 1) as usize;

        let size = (1u64 << (c - 1)) + x;
        Ok(size)
    }
}

fn atom_to_bits(atom: &ParsedAtom) -> Vec<bool> {
    match atom {
        ParsedAtom::Small(x) => {
            let mut bits = Vec::with_capacity(128);
            for i in 0..128 {
                bits.push((x >> i) & 1 == 1);
            }
            // Trim trailing zeros beyond highest set bit? Not needed — cue stops when done.
            bits
        }
        ParsedAtom::Big(x) => {
            // Convert to little-endian bytes, then bits
            let bytes = x.to_bytes_le();
            let mut bits = Vec::new();
            for &byte in &bytes {
                for i in 0..8 {
                    bits.push((byte >> i) & 1 == 1);
                }
            }
            // Pad to next multiple of 8? Not necessary.
            bits
        }
    }
}

fn cue_inner(
    // rename
    bits: &[bool],
    cursor: usize,
    backrefs: &mut HashMap<u64, NounExpr>,
) -> Result<(NounExpr, usize), Box<dyn std::error::Error>> {
    if cursor >= bits.len() {
        return Err("unexpected EOF".into());
    }

    let tag0 = bits[cursor];
    if !tag0 {
        let mut cur = cursor + 1;
        let atom = rub_atom(bits, &mut cur)?;
        let noun = NounExpr::ParsedAtom(atom);
        backrefs.insert(cursor as u64, noun.clone());
        Ok((noun, cur))
    } else {
        if cursor + 1 >= bits.len() {
            return Err("unexpected EOF after tag 1".into());
        }
        let tag1 = bits[cursor + 1];
        if !tag1 {
            let mut cur = cursor + 2;
            let (head, next) = cue_inner(bits, cur, backrefs)?;
            cur = next;
            let (tail, next2) = cue_inner(bits, cur, backrefs)?;
            cur = next2;
            let noun = NounExpr::Cell(Box::new(head), Box::new(tail));
            backrefs.insert(cursor as u64, noun.clone());
            Ok((noun, cur))
        } else {
            let mut cur = cursor + 2;
            let offset = rub_backref(bits, &mut cur)?;

            let noun = backrefs
                .get(&(offset))
                .cloned()
                .ok_or_else(|| format!("backref to {} not found", offset))?;
            Ok((noun, cur))
        }
    }
}

pub fn crub<'src>() -> impl Parser<'src, &'src str, Coin, Err<'src>> {
    choice((
        absolute_date().map(|d| Coin::Dime("da".to_string(), d)),
        relative_date().map(|d| Coin::Dime("dr".to_string(), d)),
        phonemic_name().map(|p| Coin::Dime("p".to_string(), p)),
        just('.')
            .ignore_then(urs())
            .map(|atom| Coin::Dime("ta".to_string(), atom)),
        just('~')
            .ignore_then(urx())
            .map(|atom| Coin::Dime("t".to_string(), atom)),
        just('-')
            .ignore_then(urx())
            .map(|atom| Coin::Dime("c".to_string(), taft(&atom))),
    ))
}

//  +rump: name/hoon or name+hoon
//
pub fn constant_separator_hoon<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just('$').to(Hoon::Rock(
            "tas".to_string(),
            NounExpr::ParsedAtom(ParsedAtom::Small(0)),
        )),
        symbol().map(|s| Hoon::Rock("tas".to_string(), NounExpr::ParsedAtom(string_to_atom(s)))),
        number().map(|(p, q)| Hoon::Rock(p, NounExpr::ParsedAtom(q))),
        just('&').to(Hoon::Rock(
            "f".to_string(),
            NounExpr::ParsedAtom(ParsedAtom::Small(0)),
        )),
        just('|').to(Hoon::Rock(
            "f".to_string(),
            NounExpr::ParsedAtom(ParsedAtom::Small(1)),
        )),
        just('~').to(Hoon::Bust(BaseType::Null)),
    ))
    .then(just('+').or(just('/')).ignore_then(hoon.clone()))
    .map(|(p, hoon)| Hoon::Pair(Box::new(p), Box::new(hoon)))
}

//  `@p`q
//
pub fn tic_aura<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    aura_text()
        .then_ignore(just("`"))
        .then(hoon_wide.clone())
        .map(|(a, b)| {
            Hoon::KetLus(
                Box::new(Hoon::Sand(a, NounExpr::ParsedAtom(ParsedAtom::Small(0)))),
                Box::new(Hoon::KetLus(
                    Box::new(Hoon::Sand(
                        "$".to_string(),
                        NounExpr::ParsedAtom(ParsedAtom::Small(0)),
                    )),
                    Box::new(b),
                )),
            )
        })
}

pub fn tic_cell_construction<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide.clone().map(|h| {
        Hoon::Pair(
            Box::new(Hoon::Rock(
                "n".to_string(),
                NounExpr::ParsedAtom(ParsedAtom::Small(0)),
            )),
            Box::new(h),
        )
    })
}

pub fn parenthesis_spec<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    hoon_wide
        .clone()
        .then(
            just(' ')
                .ignore_then(spec_wide.clone())
                .repeated()
                .collect::<Vec<_>>()
                .or_not()
                .map(|specs| specs.unwrap_or_default()),
        )
        .delimited_by(just('('), just(')'))
        .map(|(name, specs)| Spec::Make(name, specs))
}

pub fn reference_spec<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    let lower = any().filter(|c: &char| matches!(c, 'a'..='z'));

    let ident_tail = any().filter(|c: &char| c.is_ascii_alphanumeric());

    let ident = lower
        .then(ident_tail.repeated().collect::<Vec<char>>())
        .to(());

    let special = any().filter(|c: &char| matches!(c, '$' | '^' | ',')).to(());

    let guard = ident.or(special).rewind();

    // prevents this parser from matching
    //  inputs that starts with: "([a-z][a-zA-Z0-9]*)|[\$\^\,]"
    guard.rewind().ignore_then(
        winglist()
            .separated_by(just(':'))
            .at_least(1)
            .collect::<Vec<_>>()
            .map(|wings: Vec<WingType>| {
                let (first, rest) = wings.split_first().expect("non-empty wing list parsed");
                Spec::Like(first.clone(), rest.to_vec())
            }),
    )
}

pub fn two_hoons_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, (Hoon, Hoon), Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
}

pub fn two_hoons_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, (Hoon, Hoon), Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
}

pub fn three_hoons_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, ((Hoon, Hoon), Hoon), Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
}

pub fn three_hoons_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, ((Hoon, Hoon), Hoon), Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
}

pub fn two_specs_tall<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, (Spec, Spec), Err<'src>> {
    gap()
        .ignore_then(spec.clone())
        .then_ignore(gap())
        .then(spec.clone())
}

pub fn two_specs_closed_tall<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, (Spec, Spec), Err<'src>> {
    two_specs_tall(spec.clone())
        .then_ignore(gap())
        .then_ignore(just("=="))
}

pub fn two_specs_closed_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, (Spec, Spec), Err<'src>> {
    spec_wide
        .clone()
        .then_ignore(just(' '))
        .then(spec_wide.clone())
        .delimited_by(just('('), just(')'))
}

pub fn hoon_spec_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, (Hoon, Spec), Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(spec_wide.clone())
        .delimited_by(just('('), just(')'))
}

pub fn hoon_spec_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, (Hoon, Spec), Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(spec.clone())
}

pub fn spec_hoon_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, (Spec, Hoon), Err<'src>> {
    gap()
        .ignore_then(spec.clone())
        .then_ignore(gap())
        .then(hoon.clone())
}

pub fn spec_hoon_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, (Spec, Hoon), Err<'src>> {
    spec_wide
        .clone()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
}

pub fn name_spec_tall<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, (String, Spec), Err<'src>> {
    gap()
        .ignore_then(symbol())
        .then_ignore(gap())
        .then(spec.clone())
}

pub fn name_spec_closed_tall<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, (String, Spec), Err<'src>> {
    gap()
        .ignore_then(symbol())
        .then_ignore(gap())
        .then(spec.clone())
        .then_ignore(just("=="))
}

pub fn name_spec_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec> + Clone,
) -> impl Parser<'src, &'src str, (String, Spec), Err<'src>> {
    symbol()
        .then_ignore(just(' '))
        .then(spec_wide.clone())
        .delimited_by(just('('), just(')'))
}

pub fn one_hoon_closed_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide.clone().delimited_by(just('('), just(')'))
}

pub fn one_hoon_closed_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .delimited_by(just('='), just('='))
}

pub fn one_spec_closed_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    spec_wide.clone().delimited_by(just('('), just(')'))
}

pub fn one_spec_closed_tall<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    gap()
        .ignore_then(spec.clone())
        .then_ignore(gap())
        .delimited_by(just('='), just('='))
}

fn apply_hoon_trace(node: Hoon, spot: Spot) -> Hoon {
    match node {
        Hoon::Dbug(existing_spot, inner) if existing_spot == spot => {
            Hoon::Dbug(existing_spot, inner)
        }
        Hoon::Dbug(existing_spot, inner) => {
            Hoon::Dbug(spot, Box::new(Hoon::Dbug(existing_spot, inner)))
        }
        other => Hoon::Dbug(spot, Box::new(other)),
    }
}

fn apply_hoon_docs(mut node: Hoon, span: (usize, usize), linemap: &LineMap) -> Hoon {
    if let Some(help) = linemap.help_after(span.0, span.1) {
        node = attach_help_to_hoon(node, help);
    }
    if let Some(help) = linemap.help_before_hoon(span.0) {
        node = attach_help_to_hoon(node, help);
    }
    node
}

fn apply_spec_docs(mut node: Spec, span: (usize, usize), linemap: &LineMap) -> Spec {
    if let Some(help) = linemap.help_after(span.0, span.1) {
        node = attach_help_to_spec(node, help);
    }
    if let Some(help) = linemap.help_before_spec(span.0) {
        node = attach_help_to_spec(node, help);
    }
    node
}

fn apply_spec_postfix_docs(mut node: Spec, span: (usize, usize), linemap: &LineMap) -> Spec {
    if let Some(help) = linemap.help_after(span.0, span.1) {
        node = attach_help_to_spec(node, help);
    }
    node
}

fn attach_help_to_hoon(node: Hoon, help: NounExpr) -> Hoon {
    if matches!(&node, Hoon::Note(Note::Help(existing), _) if existing == &help) {
        return node;
    }
    Hoon::Note(Note::Help(help), Box::new(node))
}

fn attach_help_to_spec(node: Spec, help: NounExpr) -> Spec {
    if matches!(&node, Spec::Gist(existing, _) if existing == &help) {
        return node;
    }
    Spec::Gist(help, Box::new(node))
}

pub fn hoon_with_span(
    node: Hoon,
    span: (usize, usize),
    wer: &Path,
    linemap: &Arc<LineMap>,
) -> Hoon {
    let spot = chumsky_spot_to_hoon_spot(span, wer, linemap);
    apply_hoon_trace(node, spot)
}

pub fn wrap_hoon_with_trace(
    wer: Path,
    linemap: Arc<LineMap>,
) -> impl for<'src> Fn(Hoon, &mut MapExtra<'src, '_, &'src str, Err<'src>>) -> Hoon + Clone {
    move |node, e| {
        let span = (e.span().start(), e.span().end());
        let spot = chumsky_spot_to_hoon_spot(span, &wer, &linemap);
        let node = apply_hoon_docs(node, span, &linemap);
        if let Hoon::Dbug(existing_spot, inner) = node {
            if existing_spot == spot {
                return Hoon::Dbug(existing_spot, inner);
            }

            let line_idx = spot.q.p.0;
            let should_skip_outer = if spot.p == existing_spot.p {
                let idx = line_idx.saturating_sub(1) as usize;
                if idx < linemap.starts.len() {
                    let start = linemap.starts[idx];
                    let mut end = linemap
                        .starts
                        .get(idx + 1)
                        .copied()
                        .unwrap_or(linemap.source.len());
                    let bytes = linemap.source.as_bytes();
                    if end > start && bytes[end - 1] == b'\n' {
                        end -= 1;
                    }
                    let line = &bytes[start..end];
                    let mut cursor = 0;
                    while cursor < line.len() && (line[cursor] == b' ' || line[cursor] == b'\t') {
                        cursor += 1;
                    }
                    matches!(
                        line.get(cursor),
                        Some(b'/')
                            if matches!(line.get(cursor + 1), Some(b'=') | Some(b'*') | Some(b'#'))
                    )
                } else {
                    false
                }
            } else {
                false
            };

            if should_skip_outer {
                return Hoon::Dbug(existing_spot, inner);
            }

            return Hoon::Dbug(spot, Box::new(Hoon::Dbug(existing_spot, inner)));
        }

        Hoon::Dbug(spot, Box::new(node))
    }
}

pub fn wrap_hoon_with_docs(
    linemap: Arc<LineMap>,
) -> impl for<'src> Fn(Hoon, &mut MapExtra<'src, '_, &'src str, Err<'src>>) -> Hoon + Clone {
    move |node, e| apply_hoon_docs(node, (e.span().start(), e.span().end()), &linemap)
}

pub fn wrap_spec_with_docs(
    linemap: Arc<LineMap>,
) -> impl for<'src> Fn(Spec, &mut MapExtra<'src, '_, &'src str, Err<'src>>) -> Spec + Clone {
    move |node, e| apply_spec_postfix_docs(node, (e.span().start(), e.span().end()), &linemap)
}

pub fn wrap_spec_with_trace(
    wer: Path,
    linemap: Arc<LineMap>,
) -> impl for<'src> Fn(Spec, &mut MapExtra<'src, '_, &'src str, Err<'src>>) -> Spec + Clone {
    move |node, e| {
        let span = (e.span().start(), e.span().end());
        let spot = chumsky_spot_to_hoon_spot(span, &wer, &linemap);
        let node = apply_spec_docs(node, span, &linemap);

        match node {
            Spec::Dbug(existing_spot, inner) => {
                if existing_spot == spot {
                    Spec::Dbug(existing_spot, inner)
                } else {
                    Spec::Dbug(spot, Box::new(Spec::Dbug(existing_spot, inner)))
                }
            }
            other => Spec::Dbug(spot, Box::new(other)),
        }
    }
}

fn arm_body_start_from_header(raw_start: usize, end: usize, linemap: &LineMap) -> Option<usize> {
    let bytes = linemap.source.as_bytes();
    if raw_start >= bytes.len() || raw_start >= end {
        return None;
    }

    let mut line_start = raw_start;
    while line_start > 0 && bytes[line_start - 1] != b'\n' {
        line_start -= 1;
    }
    let mut cursor = line_start;
    while cursor < bytes.len() && (bytes[cursor] == b' ' || bytes[cursor] == b'\t') {
        cursor += 1;
    }
    if raw_start > cursor {
        return None;
    }
    if !matches!(bytes.get(cursor), Some(b'+'))
        || !matches!(bytes.get(cursor + 1), Some(b'+') | Some(b'$'))
    {
        return None;
    }

    cursor += 2;
    while cursor < bytes.len() && (bytes[cursor] == b' ' || bytes[cursor] == b'\t') {
        cursor += 1;
    }
    if bytes.get(cursor) == Some(&b'$') {
        cursor += 1;
    } else {
        let name_start = cursor;
        while cursor < bytes.len() {
            let b = bytes[cursor];
            if b.is_ascii_alphanumeric() || b == b'-' {
                cursor += 1;
            } else {
                break;
            }
        }
        if cursor == name_start {
            return None;
        }
    }

    let mut first_doc_content = None;
    while cursor < end {
        match bytes.get(cursor).copied() {
            Some(b' ' | b'\t' | b'\r' | b'\n') => cursor += 1,
            Some(b':') if bytes.get(cursor + 1) == Some(&b':') => {
                let comment_start = cursor;
                cursor += 2;
                let doc_marker =
                    matches!(bytes.get(cursor), None | Some(b' ' | b'\t' | b'\r' | b'\n'));
                let mut content = cursor;
                while content < end && matches!(bytes.get(content), Some(b' ' | b'\t')) {
                    content += 1;
                }
                let has_content =
                    content < end && !matches!(bytes.get(content), None | Some(b'\r' | b'\n'));
                if doc_marker && has_content && first_doc_content.is_none() {
                    first_doc_content = Some(comment_start);
                }
                while cursor < end && bytes.get(cursor) != Some(&b'\n') {
                    cursor += 1;
                }
            }
            Some(_) => {
                if first_doc_content.is_some() && bytes.get(cursor) == Some(&b'=') {
                    return Some(cursor);
                }
                return Some(linemap.expand_gap_start(cursor));
            }
            None => return None,
        }
    }
    first_doc_content
}

fn non_doc_start_after_leading_doc_span(
    raw_start: usize,
    end: usize,
    linemap: &LineMap,
) -> Option<usize> {
    let bytes = linemap.source.as_bytes();
    let mut start = raw_start.min(bytes.len());
    while start < bytes.len() && matches!(bytes[start], b' ' | b'\t' | b'\r' | b'\n') {
        start += 1;
    }

    let mut line_start = start;
    while line_start > 0 && bytes[line_start - 1] != b'\n' {
        line_start -= 1;
    }
    let mut cursor = line_start;
    while cursor < bytes.len() && matches!(bytes[cursor], b' ' | b'\t') {
        cursor += 1;
    }
    if cursor != start {
        return None;
    }
    if bytes.get(cursor) != Some(&b':') || bytes.get(cursor + 1) != Some(&b':') {
        return None;
    }

    let mut scan_start = line_start;
    while scan_start < bytes.len() {
        let line_end = bytes[scan_start..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |idx| scan_start + idx);
        let mut scan_cursor = scan_start;
        while scan_cursor < line_end && matches!(bytes[scan_cursor], b' ' | b'\t') {
            scan_cursor += 1;
        }
        if scan_cursor == line_end
            || (bytes.get(scan_cursor) == Some(&b':') && bytes.get(scan_cursor + 1) == Some(&b':'))
        {
            if bytes.get(scan_cursor) == Some(&b':') && bytes.get(scan_cursor + 1) == Some(&b':') {
                let mut content = scan_cursor + 2;
                let mut spaces = 0usize;
                while content < line_end && bytes[content] == b' ' {
                    spaces += 1;
                    content += 1;
                }
                if content < line_end && spaces >= 4 {
                    let mut preferred = scan_cursor;
                    let mut prev_end = scan_start.saturating_sub(1);
                    while prev_end > 0 {
                        let mut prev_line_start = prev_end;
                        while prev_line_start > 0 && bytes[prev_line_start - 1] != b'\n' {
                            prev_line_start -= 1;
                        }
                        let mut prev_cursor = prev_line_start;
                        while prev_cursor < prev_end && matches!(bytes[prev_cursor], b' ' | b'\t') {
                            prev_cursor += 1;
                        }
                        if prev_cursor != scan_cursor - scan_start + prev_line_start
                            || bytes.get(prev_cursor) != Some(&b':')
                            || bytes.get(prev_cursor + 1) != Some(&b':')
                        {
                            break;
                        }
                        let mut prev_content = prev_cursor + 2;
                        let mut prev_spaces = 0usize;
                        while prev_content < prev_end && bytes[prev_content] == b' ' {
                            prev_spaces += 1;
                            prev_content += 1;
                        }
                        if prev_content >= prev_end || prev_spaces != spaces {
                            break;
                        }
                        preferred = prev_cursor;
                        prev_end = prev_line_start.saturating_sub(1);
                    }
                    return Some(linemap.expand_gap_start(preferred));
                }
            }
            scan_start = line_end.saturating_add(1);
            continue;
        }
        if scan_cursor >= end {
            return None;
        }

        let expanded = linemap.expand_gap_start(scan_cursor);
        return Some(if expanded > start {
            expanded
        } else {
            scan_cursor
        });
    }
    None
}

fn skip_plain_doc_before_equals_slash_start(
    start: usize,
    raw_start: usize,
    linemap: &LineMap,
) -> usize {
    if start >= raw_start {
        return start;
    }

    let bytes = linemap.source.as_bytes();
    let raw = raw_start.min(bytes.len());
    let mut raw_line_start = raw;
    while raw_line_start > 0 && bytes[raw_line_start - 1] != b'\n' {
        raw_line_start -= 1;
    }
    let mut raw_cursor = raw_line_start;
    while raw_cursor < bytes.len() && matches!(bytes[raw_cursor], b' ' | b'\t') {
        raw_cursor += 1;
    }
    if bytes.get(raw_cursor) != Some(&b'=') || bytes.get(raw_cursor + 1) != Some(&b'/') {
        return start;
    }
    let mut name_cursor = raw_cursor + 2;
    while name_cursor < bytes.len() && matches!(bytes[name_cursor], b' ' | b'\t') {
        name_cursor += 1;
    }
    let name_start = name_cursor;
    while name_cursor < bytes.len()
        && (bytes[name_cursor].is_ascii_alphanumeric() || bytes[name_cursor] == b'-')
    {
        name_cursor += 1;
    }
    let raw_name = (name_cursor > name_start).then_some(&bytes[name_start..name_cursor]);

    let mut doc_line_start = start.min(bytes.len());
    while doc_line_start > 0 && bytes[doc_line_start - 1] != b'\n' {
        doc_line_start -= 1;
    }

    if doc_line_start == 0 {
        return start;
    }
    let prev_end = doc_line_start.saturating_sub(1);
    let mut prev_line_start = prev_end;
    while prev_line_start > 0 && bytes[prev_line_start - 1] != b'\n' {
        prev_line_start -= 1;
    }
    let mut prev_cursor = prev_line_start;
    while prev_cursor < prev_end && matches!(bytes[prev_cursor], b' ' | b'\t') {
        prev_cursor += 1;
    }
    if prev_cursor - prev_line_start != raw_cursor - raw_line_start
        || bytes.get(prev_cursor) != Some(&b'=')
        || bytes.get(prev_cursor + 1) != Some(&b'/')
    {
        return start;
    }

    let mut saw_plain_doc = false;
    let mut scan_start = doc_line_start;
    while scan_start < raw_line_start {
        let line_end = bytes[scan_start..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |idx| scan_start + idx);
        let mut cursor = scan_start;
        while cursor < line_end && matches!(bytes[cursor], b' ' | b'\t') {
            cursor += 1;
        }
        if cursor == line_end {
            scan_start = line_end.saturating_add(1);
            continue;
        }
        if bytes.get(cursor) != Some(&b':') || bytes.get(cursor + 1) != Some(&b':') {
            return start;
        }

        let mut content = cursor + 2;
        let mut spaces = 0usize;
        while content < line_end && bytes[content] == b' ' {
            spaces += 1;
            content += 1;
        }
        if content < line_end {
            if let Some(name) = raw_name {
                let after_plus = content + 1;
                if bytes.get(content) == Some(&b'+')
                    && bytes.get(after_plus..after_plus + name.len()) == Some(name)
                {
                    return start;
                }
            }
            if spaces >= 4 {
                return start;
            }
            saw_plain_doc = true;
        }
        scan_start = line_end.saturating_add(1);
    }

    if saw_plain_doc {
        raw_cursor
    } else {
        start
    }
}

fn chumsky_spot_to_hoon_spot(span: (usize, usize), wer: &Path, linemap: &Arc<LineMap>) -> Spot {
    let (raw_start, end) = span;
    let arm_body_start = arm_body_start_from_header(raw_start, end, linemap);
    let start = match arm_body_start {
        Some(start) => start,
        None => non_doc_start_after_leading_doc_span(raw_start, end, linemap)
            .unwrap_or_else(|| linemap.expand_gap_start(raw_start)),
    };
    let start = skip_plain_doc_before_equals_slash_start(start, raw_start, linemap);
    let (sl, sc) = linemap.line_col(start);
    let (el, ec) = linemap.line_col(end);

    Spot {
        p: wer.clone(),
        q: Pint {
            p: (sl as u64, sc as u64),
            q: (el as u64, ec as u64),
        },
    }
}

pub fn print_noun(noun: NounHandle<'_>, max_depth: usize, current_depth: usize) -> String {
    if current_depth >= max_depth {
        return "...".to_string();
    }

    match noun.as_either_atom_cell() {
        Left(atom) => format!("{:?}", atom.atom()),

        Right(cell) => {
            let head = cell.head();
            let tail = cell.tail();

            let head_is_atom = head.as_either_atom_cell().is_left();
            let tail_is_atom = tail.as_either_atom_cell().is_left();

            if head_is_atom && tail_is_atom {
                format!(
                    "[{} {}]",
                    print_noun(head, max_depth, current_depth + 1),
                    print_noun(tail, max_depth, current_depth + 1),
                )
            } else {
                let indent = "  ".repeat(current_depth);
                let inner_indent = "  ".repeat(current_depth + 1);

                format!(
                    "[\n{}{}\n{}{}\n{}]",
                    inner_indent,
                    print_noun(head, max_depth, current_depth + 1),
                    inner_indent,
                    print_noun(tail, max_depth, current_depth + 1),
                    indent,
                )
            }
        }
    }
}

// pub fn print_noun(
//     noun: &Noun,
//     max_depth: usize,
//     current_depth: usize,
// ) -> String {
//     if current_depth >= max_depth {
//         return "...".to_string();
//     }

//     let indent = "  ".repeat(current_depth);

//     match noun.as_either_atom_cell() {
//         Left(atom) => format!("{:?}", atom),
//         Right(cell) => format!(
//             "[\n{}  {}\n{}  {}\n{}]",
//             indent,
//             print_noun(&cell.head(), max_depth, current_depth + 1),
//             indent,
//             print_noun(&cell.tail(), max_depth, current_depth + 1),
//             indent,
//         ),
//     }
// }

fn skip_dbug(mut n: NounHandle<'_>) -> NounHandle<'_> {
    loop {
        let cell = match n.cell() {
            Some(c) => c,
            None => return n,
        };

        let head = match cell.head().as_atom() {
            Ok(a) => a,
            Err(_) => return n,
        };

        if !head.as_noun().eq_bytes(b"dbug") {
            return n;
        }

        let tail_cell = match cell.tail().as_cell() {
            Ok(c) => c,
            Err(_) => return n,
        };

        n = tail_cell.tail();
    }
}
pub fn diff_noun(a: NounHandle<'_>, b: NounHandle<'_>, printed: &mut bool) -> Result<(), ()> {
    let a = skip_dbug(a);
    let b = skip_dbug(b);

    if noun_equality(a, b) {
        return Ok(());
    }

    match (a.as_either_atom_cell(), b.as_either_atom_cell()) {
        (Right(ac), Right(bc)) => {
            if diff_noun(ac.head(), bc.head(), printed).is_err() {
                if !*printed {
                    print_context(a, b);
                    *printed = true;
                }
                return Err(());
            }

            if diff_noun(ac.tail(), bc.tail(), printed).is_err() {
                if !*printed {
                    print_context(a, b);
                    *printed = true;
                }
                return Err(());
            }

            Ok(())
        }

        _ => Err(()),
    }
}

fn print_context(a: NounHandle<'_>, b: NounHandle<'_>) {
    eprintln!("Mismatch in subtree:");
    eprintln!("expected: {}", print_noun(a, 10, 0));
    eprintln!("actual:   {}", print_noun(b, 10, 0));
}

pub fn diff_and_report(a: NounHandle<'_>, b: NounHandle<'_>) {
    let mut printed = false;
    if diff_noun(a, b, &mut printed).is_ok() {
        eprintln!("Test passed!");
    }
}

fn atom_to_tas_string(atom: &DirectAtom) -> String {
    let val: u128 = atom.data() as u128;
    if val == 0 {
        return String::new();
    }

    let bytes = val.to_le_bytes();
    let mut null_seen = false;
    let mut valid = true;
    let mut len = 0;

    for &b in &bytes {
        if b == 0 {
            null_seen = true;
        } else if null_seen {
            valid = false;
            break;
        } else if !b.is_ascii_lowercase() && b != b'-' {
            valid = false;
            break;
        } else {
            len += 1;
        }

        // Cap at 126 bytes (Urbit tas limit)
        if len > 126 {
            valid = false;
            break;
        }
    }

    if valid && len > 0 {
        format!("%{}", unsafe {
            std::str::from_utf8_unchecked(&bytes[..len])
        })
    } else {
        String::new()
    }
}

pub fn hoon_to_noun(slab: &mut NounSlab, hoon: &Hoon) -> Noun {
    use Hoon::*;

    match hoon {
        Pair(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[p, q])
        }
        ZapZap => T(slab, &[D(tas!(b"zpzp")), D(0)]),
        Axis(a) => T(slab, &[D(0), D(*a)]),
        Base(bt) => {
            let bt_noun = basetype_to_noun(slab, bt);
            T(slab, &[D(tas!(b"base")), bt_noun])
        }
        Bust(bt) => {
            let bt_noun = basetype_to_noun(slab, bt);
            T(slab, &[D(tas!(b"bust")), bt_noun])
        }
        Dbug(spot, h) => {
            let spot_noun = spot_to_noun(slab, spot);
            let h_noun = hoon_to_noun(slab, h);
            T(slab, &[D(tas!(b"dbug")), spot_noun, h_noun])
        }
        Eror(msg) => {
            let msg_noun = cord_to_noun(slab, msg);
            T(slab, &[D(tas!(b"eror")), msg_noun])
        }
        Hand(typ, nock) => {
            let typ_noun = type_to_noun(slab, typ);
            let nock_noun = nock_to_noun(slab, nock);
            T(slab, &[D(tas!(b"hand")), typ_noun, nock_noun])
        }
        Note(note, h) => {
            let note_noun = note_to_noun(slab, note);
            let h_noun = hoon_to_noun(slab, h);
            T(slab, &[D(tas!(b"note")), note_noun, h_noun])
        }
        Fits(h, wing) => {
            let h_noun = hoon_to_noun(slab, h);
            let wing_noun = wing_to_noun(slab, wing);
            T(slab, &[D(tas!(b"fits")), h_noun, wing_noun])
        }
        Knit(woofs) => {
            let woofs_noun: Vec<_> = woofs.iter().map(|w| woof_to_noun(slab, w)).collect();
            let list = list_to_noun(slab, woofs_noun);
            T(slab, &[D(tas!(b"knit")), list])
        }
        Leaf(tag, atom) => {
            let tag_noun = term_to_noun(slab, tag);
            let atom_noun = atom_to_noun(slab, atom);
            T(slab, &[D(tas!(b"leaf")), tag_noun, atom_noun])
        }
        Limb(name) => {
            let name_noun = term_to_noun(slab, name);
            T(slab, &[D(tas!(b"limb")), name_noun])
        }
        Lost(h) => {
            let h_noun = hoon_to_noun(slab, h);
            T(slab, &[D(tas!(b"lost")), h_noun])
        }
        Rock(au, expr) => {
            let au_noun = term_to_noun(slab, au);
            let expr_noun = noun_expr_to_noun(slab, expr);
            T(slab, &[D(tas!(b"rock")), au_noun, expr_noun])
        }
        Sand(au, expr) => {
            let au_noun = term_to_noun(slab, au);
            let expr_noun = noun_expr_to_noun(slab, expr);
            T(slab, &[D(tas!(b"sand")), au_noun, expr_noun])
        }
        Tell(hoons) => {
            let hoons_noun: Vec<_> = hoons.iter().map(|h| hoon_to_noun(slab, h)).collect();
            let list = list_to_noun(slab, hoons_noun);
            T(slab, &[D(tas!(b"tell")), list])
        }
        Tune(tune) => {
            let tune_noun = term_or_tune_to_noun(slab, tune);
            T(slab, &[D(tas!(b"tune")), tune_noun])
        }
        Wing(wing) => {
            let wing_noun = wing_to_noun(slab, wing);
            T(slab, &[D(tas!(b"wing")), wing_noun])
        }
        Yell(hoons) => {
            let hoons_noun: Vec<_> = hoons.iter().map(|h| hoon_to_noun(slab, h)).collect();
            let list = list_to_noun(slab, hoons_noun);
            T(slab, &[D(tas!(b"yell")), list])
        }
        Xray(manx) => {
            let manx_noun = manx_to_noun(slab, manx);
            T(slab, &[D(tas!(b"xray")), manx_noun])
        }
        BarBuc(tagnames, spec) => {
            let tags_noun: Vec<_> = tagnames.iter().map(|s| term_to_noun(slab, s)).collect();
            let list = list_to_noun(slab, tags_noun);
            let spec_noun = spec_to_noun(slab, spec);
            T(slab, &[D(tas!(b"brbc")), list, spec_noun])
        }
        BarCab(spec, alas, tomes) => {
            let spec_noun = spec_to_noun(slab, spec);
            let alas_noun = alas_to_noun(slab, alas);

            let mut tomes_pairs = Vec::new();
            for (k, tome) in tomes {
                let k_noun = term_to_noun(slab, k);
                let tome_noun = tome_to_noun(slab, tome);
                tomes_pairs.push((k_noun, tome_noun));
            }
            let tomes_noun = map_to_noun(slab, tomes_pairs);
            T(slab, &[D(tas!(b"brcb")), spec_noun, alas_noun, tomes_noun])
        }
        BarCol(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"brcl")), p, q])
        }
        BarCen(prefix, tomes) => {
            let prefix_noun = match prefix.as_ref() {
                None => D(0u64),
                Some(s) => {
                    let term_noun = term_to_noun(slab, s);
                    T(slab, &[D(0), term_noun])
                }
            };
            let mut tomes_pairs = Vec::new();
            for (k, tome) in tomes {
                let k_noun = term_to_noun(slab, k);
                let tome_noun = tome_to_noun(slab, tome);
                tomes_pairs.push((k_noun, tome_noun));
            }
            let tomes_noun = map_to_noun(slab, tomes_pairs);
            T(slab, &[D(tas!(b"brcn")), prefix_noun, tomes_noun])
        }
        BarDot(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"brdt")), p])
        }
        BarKet(p, tomes) => {
            let p_noun = hoon_to_noun(slab, p);
            let mut tomes_pairs = Vec::new();
            for (k, tome) in tomes {
                let k_noun = term_to_noun(slab, k);
                let tome_noun = tome_to_noun(slab, tome);
                tomes_pairs.push((k_noun, tome_noun));
            }
            let tomes_noun = map_to_noun(slab, tomes_pairs);
            T(slab, &[D(tas!(b"brkt")), p_noun, tomes_noun])
        }
        BarHep(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"brhp")), p])
        }
        BarSig(spec, p) => {
            let spec_noun = spec_to_noun(slab, spec);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"brsg")), spec_noun, p_noun])
        }
        BarTar(spec, p) => {
            let spec_noun = spec_to_noun(slab, spec);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"brtr")), spec_noun, p_noun])
        }
        BarTis(spec, p) => {
            let spec_noun = spec_to_noun(slab, spec);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"brts")), spec_noun, p_noun])
        }
        BarPat(prefix, tomes) => {
            let prefix_noun = match prefix.as_ref() {
                None => D(0u64),
                Some(s) => {
                    let term_noun = term_to_noun(slab, s);
                    T(slab, &[D(0), term_noun])
                }
            };
            let mut tomes_pairs = Vec::new();
            for (k, tome) in tomes {
                let k_noun = term_to_noun(slab, k);
                let tome_noun = tome_to_noun(slab, tome);
                tomes_pairs.push((k_noun, tome_noun));
            }
            let tomes_noun = map_to_noun(slab, tomes_pairs);
            T(slab, &[D(tas!(b"brpt")), prefix_noun, tomes_noun])
        }
        BarWut(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"brwt")), p])
        }
        ColCab(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"clcb")), p, q])
        }
        ColKet(a, b, c, d) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            let c = hoon_to_noun(slab, c);
            let d = hoon_to_noun(slab, d);
            T(slab, &[D(tas!(b"clkt")), a, b, c, d])
        }
        ColHep(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"clhp")), p, q])
        }
        ColLus(a, b, c) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            let c = hoon_to_noun(slab, c);
            T(slab, &[D(tas!(b"clls")), a, b, c])
        }
        ColSig(hoons) => {
            let hoons_noun: Vec<_> = hoons.iter().map(|h| hoon_to_noun(slab, h)).collect();
            let list = list_to_noun(slab, hoons_noun);
            T(slab, &[D(tas!(b"clsg")), list])
        }
        ColTar(hoons) => {
            let hoons_noun: Vec<_> = hoons.iter().map(|h| hoon_to_noun(slab, h)).collect();
            let list = list_to_noun(slab, hoons_noun);
            T(slab, &[D(tas!(b"cltr")), list])
        }
        CenCab(wing, pairs) => {
            let wing_noun = wing_to_noun(slab, wing);
            let pairs_noun: Vec<_> = pairs
                .iter()
                .map(|(w, h)| {
                    let w_noun = wing_to_noun(slab, w);
                    let h_noun = hoon_to_noun(slab, h);
                    T(slab, &[w_noun, h_noun])
                })
                .collect();
            let list = list_to_noun(slab, pairs_noun);
            T(slab, &[D(tas!(b"cncb")), wing_noun, list])
        }
        CenDot(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"cndt")), p, q])
        }
        CenHep(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"cnhp")), p, q])
        }
        CenCol(p, hoons) => {
            let p = hoon_to_noun(slab, p);
            let hoons_noun: Vec<_> = hoons.iter().map(|h| hoon_to_noun(slab, h)).collect();
            let list = list_to_noun(slab, hoons_noun);
            T(slab, &[D(tas!(b"cncl")), p, list])
        }
        CenTar(wing, p, pairs) => {
            let wing_noun = wing_to_noun(slab, wing);
            let p_noun = hoon_to_noun(slab, p);
            let pairs_noun: Vec<_> = pairs
                .iter()
                .map(|(w, h)| {
                    let w_noun = wing_to_noun(slab, w);
                    let h_noun = hoon_to_noun(slab, h);
                    T(slab, &[w_noun, h_noun])
                })
                .collect();
            let list = list_to_noun(slab, pairs_noun);
            T(slab, &[D(tas!(b"cntr")), wing_noun, p_noun, list])
        }
        CenKet(a, b, c, d) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            let c = hoon_to_noun(slab, c);
            let d = hoon_to_noun(slab, d);
            T(slab, &[D(tas!(b"cnkt")), a, b, c, d])
        }
        CenLus(a, b, c) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            let c = hoon_to_noun(slab, c);
            T(slab, &[D(tas!(b"cnls")), a, b, c])
        }
        CenSig(wing, p, hoons) => {
            let wing_noun = wing_to_noun(slab, wing);
            let p_noun = hoon_to_noun(slab, p);
            let hoons_noun: Vec<_> = hoons.iter().map(|h| hoon_to_noun(slab, h)).collect();
            let list = list_to_noun(slab, hoons_noun);
            T(slab, &[D(tas!(b"cnsg")), wing_noun, p_noun, list])
        }
        CenTis(wing, pairs) => {
            let wing_noun = wing_to_noun(slab, wing);
            let pairs_noun: Vec<_> = pairs
                .iter()
                .map(|(w, h)| {
                    let w_noun = wing_to_noun(slab, w);
                    let h_noun = hoon_to_noun(slab, h);
                    T(slab, &[w_noun, h_noun])
                })
                .collect();
            let list = list_to_noun(slab, pairs_noun);
            T(slab, &[D(tas!(b"cnts")), wing_noun, list])
        }
        DotKet(spec, p) => {
            let spec_noun = spec_to_noun(slab, spec);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"dtkt")), spec_noun, p_noun])
        }
        DotLus(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"dtls")), p])
        }
        DotTar(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"dttr")), p, q])
        }
        DotTis(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"dtts")), p, q])
        }
        DotWut(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"dtwt")), p])
        }
        KetBar(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"ktbr")), p])
        }
        KetDot(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"ktdt")), p, q])
        }
        KetLus(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"ktls")), p, q])
        }
        KetHep(spec, p) => {
            let spec_noun = spec_to_noun(slab, spec);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"kthp")), spec_noun, p_noun])
        }
        KetPam(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"ktpm")), p])
        }
        KetSig(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"ktsg")), p])
        }
        KetTis(skin, p) => {
            let skin_noun = skin_to_noun(slab, skin);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"ktts")), skin_noun, p_noun])
        }
        KetWut(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"ktwt")), p])
        }
        KetTar(spec) => {
            let spec_noun = spec_to_noun(slab, spec);
            T(slab, &[D(tas!(b"kttr")), spec_noun])
        }
        KetCol(spec) => {
            let spec_noun = spec_to_noun(slab, spec);
            T(slab, &[D(tas!(b"ktcl")), spec_noun])
        }
        SigBar(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"sgbr")), p, q])
        }
        SigCab(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"sgcb")), p, q])
        }
        SigCen(chum, p, tyre, q) => {
            let chum_noun = chum_to_noun(slab, chum);
            let p_noun = hoon_to_noun(slab, p);
            let tyre_noun = tyre_to_noun(slab, tyre);
            let q_noun = hoon_to_noun(slab, q);
            T(
                slab,
                &[D(tas!(b"sgcn")), chum_noun, p_noun, tyre_noun, q_noun],
            )
        }
        SigFas(chum, p) => {
            let chum_noun = chum_to_noun(slab, chum);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"sgfs")), chum_noun, p_noun])
        }
        SigGal(term_or_pair, p) => {
            let term_noun = term_or_pair_to_noun(slab, term_or_pair);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"sggl")), term_noun, p_noun])
        }
        SigGar(term_or_pair, p) => {
            let term_noun = term_or_pair_to_noun(slab, term_or_pair);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"sggr")), term_noun, p_noun])
        }
        SigBuc(tag, p) => {
            let tag_noun = term_to_noun(slab, tag);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"sgbc")), tag_noun, p_noun])
        }
        SigLus(n, p) => {
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"sgls")), D(*n), p_noun])
        }
        SigPam(n, p, q) => {
            let p_noun = hoon_to_noun(slab, p);
            let q_noun = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"sgpm")), D(*n), p_noun, q_noun])
        }
        SigTis(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"sgts")), p, q])
        }
        SigWut(n, a, b, c) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            let c = hoon_to_noun(slab, c);
            T(slab, &[D(tas!(b"sgwt")), D(*n), a, b, c])
        }
        SigZap(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"sgzp")), p, q])
        }
        MicTis(marl) => {
            let marl_noun = marl_to_noun(slab, marl);
            T(slab, &[D(tas!(b"mcts")), marl_noun])
        }
        MicCol(p, hoons) => {
            let p = hoon_to_noun(slab, p);
            let hoons_noun: Vec<_> = hoons.iter().map(|h| hoon_to_noun(slab, h)).collect();
            let list = list_to_noun(slab, hoons_noun);
            T(slab, &[D(tas!(b"mccl")), p, list])
        }
        MicFas(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"mcfs")), p])
        }
        MicGal(spec, a, b, c) => {
            let spec_noun = spec_to_noun(slab, spec);
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            let c = hoon_to_noun(slab, c);
            T(slab, &[D(tas!(b"mcgl")), spec_noun, a, b, c])
        }
        MicSig(p, hoons) => {
            let p = hoon_to_noun(slab, p);
            let hoons_noun: Vec<_> = hoons.iter().map(|h| hoon_to_noun(slab, h)).collect();
            let list = list_to_noun(slab, hoons_noun);
            T(slab, &[D(tas!(b"mcsg")), p, list])
        }
        MicMic(spec, p) => {
            let spec_noun = spec_to_noun(slab, spec);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"mcmc")), spec_noun, p_noun])
        }
        TisBar(spec, p) => {
            let spec_noun = spec_to_noun(slab, spec);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"tsbr")), spec_noun, p_noun])
        }
        TisCol(pairs, p) => {
            let pairs_noun: Vec<_> = pairs
                .iter()
                .map(|(w, h)| {
                    let w_noun = wing_to_noun(slab, w);
                    let h_noun = hoon_to_noun(slab, h);
                    T(slab, &[w_noun, h_noun])
                })
                .collect();
            let list = list_to_noun(slab, pairs_noun);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"tscl")), list, p_noun])
        }
        TisFas(skin, a, b) => {
            let skin_noun = skin_to_noun(slab, skin);
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"tsfs")), skin_noun, a, b])
        }
        TisMic(skin, a, b) => {
            let skin_noun = skin_to_noun(slab, skin);
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"tsmc")), skin_noun, a, b])
        }
        TisDot(wing, a, b) => {
            let wing_noun = wing_to_noun(slab, wing);
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"tsdt")), wing_noun, a, b])
        }
        TisWut(wing, a, b, c) => {
            let wing_noun = wing_to_noun(slab, wing);
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            let c = hoon_to_noun(slab, c);
            T(slab, &[D(tas!(b"tswt")), wing_noun, a, b, c])
        }
        TisGal(a, b) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"tsgl")), a, b])
        }
        TisHep(a, b) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"tshp")), a, b])
        }
        TisGar(a, b) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"tsgr")), a, b])
        }
        TisKet(skin, wing, a, b) => {
            let skin_noun = skin_to_noun(slab, skin);
            let wing_noun = wing_to_noun(slab, wing);
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"tskt")), skin_noun, wing_noun, a, b])
        }
        TisLus(a, b) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"tsls")), a, b])
        }
        TisSig(hoons) => {
            let hoons_noun: Vec<_> = hoons.iter().map(|h| hoon_to_noun(slab, h)).collect();
            let list = list_to_noun(slab, hoons_noun);
            T(slab, &[D(tas!(b"tssg")), list])
        }
        TisTar((name, spec_opt), a, b) => {
            let name_noun = term_to_noun(slab, name);
            let spec_unit = match spec_opt.as_ref() {
                None => D(0u64),
                Some(spec) => {
                    let spec_noun = spec_to_noun(slab, spec);
                    T(slab, &[D(0), spec_noun])
                }
            };
            let name_spec = T(slab, &[name_noun, spec_unit]);
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"tstr")), name_spec, a, b])
        }
        TisCom(a, b) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"tscm")), a, b])
        }
        WutBar(hoons) => {
            let hoons_noun: Vec<_> = hoons.iter().map(|h| hoon_to_noun(slab, h)).collect();
            let list = list_to_noun(slab, hoons_noun);
            T(slab, &[D(tas!(b"wtbr")), list])
        }
        WutHep(wing, pairs) => {
            let wing_noun = wing_to_noun(slab, wing);
            let pairs_noun: Vec<_> = pairs
                .iter()
                .map(|(spec, h)| {
                    let spec_noun = spec_to_noun(slab, spec);
                    let h_noun = hoon_to_noun(slab, h);
                    T(slab, &[spec_noun, h_noun])
                })
                .collect();
            let list = list_to_noun(slab, pairs_noun);
            T(slab, &[D(tas!(b"wthp")), wing_noun, list])
        }
        WutCol(a, b, c) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            let c = hoon_to_noun(slab, c);
            T(slab, &[D(tas!(b"wtcl")), a, b, c])
        }
        WutDot(a, b, c) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            let c = hoon_to_noun(slab, c);
            T(slab, &[D(tas!(b"wtdt")), a, b, c])
        }
        WutKet(wing, a, b) => {
            let wing_noun = wing_to_noun(slab, wing);
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"wtkt")), wing_noun, a, b])
        }
        WutGal(a, b) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"wtgl")), a, b])
        }
        WutGar(a, b) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"wtgr")), a, b])
        }
        WutLus(wing, a, pairs) => {
            let wing_noun = wing_to_noun(slab, wing);
            let a = hoon_to_noun(slab, a);
            let pairs_noun: Vec<_> = pairs
                .iter()
                .map(|(spec, h)| {
                    let spec_noun = spec_to_noun(slab, spec);
                    let h_noun = hoon_to_noun(slab, h);
                    T(slab, &[spec_noun, h_noun])
                })
                .collect();
            let list = list_to_noun(slab, pairs_noun);
            T(slab, &[D(tas!(b"wtls")), wing_noun, a, list])
        }
        WutPam(hoons) => {
            let hoons_noun: Vec<_> = hoons.iter().map(|h| hoon_to_noun(slab, h)).collect();
            let list = list_to_noun(slab, hoons_noun);
            T(slab, &[D(tas!(b"wtpm")), list])
        }
        WutPat(wing, a, b) => {
            let wing_noun = wing_to_noun(slab, wing);
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"wtpt")), wing_noun, a, b])
        }
        WutSig(wing, a, b) => {
            let wing_noun = wing_to_noun(slab, wing);
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"wtsg")), wing_noun, a, b])
        }
        WutHax(skin, wing) => {
            let skin_noun = skin_to_noun(slab, skin);
            let wing_noun = wing_to_noun(slab, wing);
            T(slab, &[D(tas!(b"wthx")), skin_noun, wing_noun])
        }
        WutTis(spec, wing) => {
            let spec_noun = spec_to_noun(slab, spec);
            let wing_noun = wing_to_noun(slab, wing);
            T(slab, &[D(tas!(b"wtts")), spec_noun, wing_noun])
        }
        WutZap(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"wtzp")), p])
        }
        ZapCom(a, b) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"zpcm")), a, b])
        }
        ZapGar(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"zpgr")), p])
        }
        ZapGal(spec, p) => {
            let spec_noun = spec_to_noun(slab, spec);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"zpgl")), spec_noun, p_noun])
        }
        ZapMic(a, b) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"zpmc")), a, b])
        }
        ZapTis(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"zpts")), p])
        }
        ZapPat(wings, a, b) => {
            let wing_nouns: Vec<_> = wings.iter().map(|w| wing_to_noun(slab, w)).collect();
            let wings_noun = list_to_noun(slab, wing_nouns);
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"zppt")), wings_noun, a, b])
        }
        ZapWut(arg, p) => {
            let arg_noun = zpwt_arg_to_noun(slab, arg);
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"zpwt")), arg_noun, p])
        }
    }
}

fn list_to_noun(slab: &mut NounSlab, nouns: Vec<Noun>) -> Noun {
    nouns
        .into_iter()
        .rev()
        .fold(D(0u64), |tail, head| T(slab, &[head, tail]))
}

fn noun_is_zero(noun: Noun) -> bool {
    unsafe { noun.raw_equals(&D(0)) }
}

fn dor(slab: &mut NounSlab, a: Noun, b: Noun) -> bool {
    let space = slab.noun_space();
    dor_in(slab, a, b, &space)
}

fn dor_in(slab: &mut NounSlab, a: Noun, b: Noun, space: &NounSpace) -> bool {
    if unsafe { a.raw_equals(&b) } {
        return true;
    }

    match (a.as_either_atom_cell(), b.as_either_atom_cell()) {
        (Left(atom_a), Left(atom_b)) => lth_b(slab, atom_a, atom_b, space),
        (Left(_), Right(_)) => true,
        (Right(_), Left(_)) => false,
        (Right(cell_a), Right(cell_b)) => {
            let cell_a = cell_a.in_space(space);
            let cell_b = cell_b.in_space(space);
            let a_head = cell_a.head().noun();
            let b_head = cell_b.head().noun();
            let a_tail = cell_a.tail().noun();
            let b_tail = cell_b.tail().noun();

            if unsafe { a_head.raw_equals(&b_head) } {
                dor_in(slab, a_tail, b_tail, space)
            } else {
                dor_in(slab, a_head, b_head, space)
            }
        }
    }
}

fn gor_mug(slab: &mut NounSlab, a: Noun, b: Noun) -> bool {
    let space = slab.noun_space();
    match slab_mug(a, &space).cmp(&slab_mug(b, &space)) {
        cmp::Ordering::Less => true,
        cmp::Ordering::Greater => false,
        cmp::Ordering::Equal => dor(slab, a, b),
    }
}

fn mor_mug(slab: &mut NounSlab, a: Noun, b: Noun) -> bool {
    let space = slab.noun_space();
    let mug_a = slab_mug(a, &space);
    let mug_b = slab_mug(b, &space);
    let mug_mug_a = slab_mug(D(mug_a as u64), &space);
    let mug_mug_b = slab_mug(D(mug_b as u64), &space);

    match mug_mug_a.cmp(&mug_mug_b) {
        cmp::Ordering::Less => true,
        cmp::Ordering::Greater => false,
        cmp::Ordering::Equal => dor(slab, a, b),
    }
}

fn map_put_mug(slab: &mut NounSlab, tree: Noun, key: Noun, value: Noun) -> Option<Noun> {
    if noun_is_zero(tree) {
        let node = T(slab, &[key, value]);
        return Some(T(slab, &[node, D(0), D(0)]));
    }

    let space = slab.noun_space();
    let tree_cell = tree.in_space(&space).as_cell().ok()?;
    let node = tree_cell.head().noun();
    let rest = tree_cell.tail().as_cell().ok()?;
    let left = rest.head().noun();
    let right = rest.tail().noun();

    let node_cell = node.in_space(&space).as_cell().ok()?;
    let node_key = node_cell.head().noun();
    let node_val = node_cell.tail().noun();

    if noun_equality(key.in_space(&space), node_key.in_space(&space)) {
        if noun_equality(value.in_space(&space), node_val.in_space(&space)) {
            return Some(tree);
        }
        let new_node = T(slab, &[key, value]);
        return Some(T(slab, &[new_node, left, right]));
    }

    if gor_mug(slab, key, node_key) {
        let d = map_put_mug(slab, left, key, value)?;
        let space = slab.noun_space();
        let d_cell = d.in_space(&space).as_cell().ok()?;
        let d_node = d_cell.head().noun();
        let d_rest = d_cell.tail().noun();
        let d_rest_cell = d_rest.in_space(&space).as_cell().ok()?;
        let d_left = d_rest_cell.head().noun();
        let d_right = d_rest_cell.tail().noun();
        let d_node_cell = d_node.in_space(&space).as_cell().ok()?;
        let d_key = d_node_cell.head().noun();

        if mor_mug(slab, node_key, d_key) {
            Some(T(slab, &[node, d, right]))
        } else {
            let new_a = T(slab, &[node, d_right, right]);
            Some(T(slab, &[d_node, d_left, new_a]))
        }
    } else {
        let d = map_put_mug(slab, right, key, value)?;
        let space = slab.noun_space();
        let d_cell = d.in_space(&space).as_cell().ok()?;
        let d_node = d_cell.head().noun();
        let d_rest = d_cell.tail().noun();
        let d_rest_cell = d_rest.in_space(&space).as_cell().ok()?;
        let d_left = d_rest_cell.head().noun();
        let d_right = d_rest_cell.tail().noun();
        let d_node_cell = d_node.in_space(&space).as_cell().ok()?;
        let d_key = d_node_cell.head().noun();

        if mor_mug(slab, node_key, d_key) {
            Some(T(slab, &[node, left, d]))
        } else {
            let new_a = T(slab, &[node, left, d_left]);
            Some(T(slab, &[d_node, new_a, d_right]))
        }
    }
}

fn map_to_noun(slab: &mut NounSlab, pairs: Vec<(Noun, Noun)>) -> Noun {
    let mut map = D(0);

    for (key, val) in pairs {
        if let Some(updated) = map_put_mug(slab, map, key, val) {
            map = updated;
        }
    }

    map
}

fn term_to_noun(slab: &mut NounSlab, s: &str) -> Noun {
    let atom = term_to_atom(s.to_string());
    atom_to_noun(slab, &atom)
}

fn cord_to_noun(slab: &mut NounSlab, s: &str) -> Noun {
    let atom = string_to_atom(s.to_string());
    atom_to_noun(slab, &atom)
}

fn atom_to_noun(slab: &mut NounSlab, atom: &ParsedAtom) -> Noun {
    match atom {
        ParsedAtom::Small(n) => {
            if *n <= DIRECT_MAX as u128 {
                D(*n as u64)
            } else {
                let bytes = n.to_le_bytes();
                let trimmed_len = bytes.iter().rev().take_while(|&&b| b == 0).count();
                let trimmed = &bytes[..bytes.len() - trimmed_len];
                let bytes_slice = if trimmed.is_empty() { &[0u8] } else { trimmed };
                let bytes = Bytes::copy_from_slice(bytes_slice);
                Atom::from_bytes(slab, &bytes).as_noun()
            }
        }
        ParsedAtom::Big(b) => {
            let ubig: UBig = UBig::from_le_bytes(b.to_bytes_le().as_slice());
            Atom::from_ubig(slab, &ubig).as_noun()
        }
    }
}

fn biguint_to_ubig(b: &BigUint) -> UBig {
    UBig::from_le_bytes(&b.to_bytes_le())
}

fn opt_to_noun<T, F>(slab: &mut NounSlab, opt: &Option<T>, f: F) -> Noun
where
    F: FnOnce(&T) -> Noun,
{
    match opt {
        None => D(0u64),
        Some(x) => {
            let x_noun = f(x);
            T(slab, &[D(0u64), x_noun])
        }
    }
}

fn basetype_to_noun(slab: &mut NounSlab, bt: &BaseType) -> Noun {
    match bt {
        BaseType::NounExpr => D(tas!(b"noun")),
        BaseType::Cell => D(tas!(b"cell")),
        BaseType::Flag => D(tas!(b"flag")),
        BaseType::Null => D(tas!(b"null")),
        BaseType::Void => D(tas!(b"void")),
        BaseType::Atom(au) => {
            let at = term_to_noun(slab, au);
            T(slab, &[D(tas!(b"atom")), at])
        }
    }
}

fn noun_expr_to_noun(slab: &mut NounSlab, expr: &NounExpr) -> Noun {
    match expr {
        NounExpr::ParsedAtom(a) => atom_to_noun(slab, a),
        NounExpr::Cell(l, r) => {
            let l_noun = noun_expr_to_noun(slab, l);
            let r_noun = noun_expr_to_noun(slab, r);
            T(slab, &[l_noun, r_noun])
        }
    }
}

fn type_to_noun(slab: &mut NounSlab, typ: &Type) -> Noun {
    use Type::*;
    match typ {
        NounExpr => D(tas!(b"noun")),
        Void => D(tas!(b"void")),
        ParsedAtom(au, bits) => {
            let au_noun = term_to_noun(slab, au);
            let bits_noun = opt_to_noun(slab, bits, |n| D(*n));
            T(slab, &[D(tas!(b"atom")), au_noun, bits_noun])
        }
        Cell(l, r) => {
            let l = type_to_noun(slab, l);
            let r = type_to_noun(slab, r);
            T(slab, &[D(tas!(b"cell")), l, r])
        }
        Core(face, coil) => {
            let face_noun = type_to_noun(slab, face);
            let coil_noun = coil_to_noun(slab, coil);
            T(slab, &[D(tas!(b"core")), face_noun, coil_noun])
        }
        Face(face_type, inner) => {
            let face_noun = face_type_to_noun(slab, face_type);
            let inner_noun = type_to_noun(slab, inner);
            T(slab, &[D(tas!(b"face")), face_noun, inner_noun])
        }
        Fork(types) => {
            let types_vec: Vec<_> = types.iter().map(|t| type_to_noun(slab, t)).collect();
            let types_noun = list_to_noun(slab, types_vec);
            T(slab, &[D(tas!(b"fork")), types_noun])
        }
        Hint((inner, note), payload) => {
            let inner_noun = type_to_noun(slab, inner);
            let note_noun = note_to_noun(slab, note);
            let payload_noun = type_to_noun(slab, payload);
            let hint_inner = T(slab, &[inner_noun, note_noun]);
            T(slab, &[D(tas!(b"hint")), hint_inner, payload_noun])
        }
        Hold(typ, hoon) => {
            let typ_noun = type_to_noun(slab, typ);
            let hoon_noun = hoon_to_noun(slab, hoon);
            T(slab, &[D(tas!(b"hold")), typ_noun, hoon_noun])
        }
    }
}

fn face_type_to_noun(slab: &mut NounSlab, ft: &FaceType) -> Noun {
    match ft {
        FaceType::Term(s) => term_to_noun(slab, s),
        FaceType::Tune(tune) => {
            let tune_noun = tune_to_noun(slab, tune);
            T(slab, &[D(tas!(b"tune")), tune_noun])
        }
    }
}

fn coil_to_noun(slab: &mut NounSlab, coil: &Coil) -> Noun {
    let garb_noun = garb_to_noun(slab, &coil.p);
    let type_noun = type_to_noun(slab, &coil.q);
    let semi_noun = semi_noun_expr_to_noun(slab, &coil.r.0);

    let tomes_entries: Vec<_> = coil
        .r
        .1
        .iter()
        .map(|(k, v)| {
            let (what, v) = v;
            let k_noun = term_to_noun(slab, k);
            let what_noun = what
                .as_ref()
                .map(|what| noun_expr_to_noun(slab, what))
                .unwrap_or_else(|| D(0));
            let inner_entries: Vec<_> = v
                .iter()
                .map(|(kk, vv)| (term_to_noun(slab, kk), hoon_to_noun(slab, vv)))
                .collect();
            let v_noun = map_to_noun(slab, inner_entries);
            (k_noun, T(slab, &[what_noun, v_noun]))
        })
        .collect();

    let tomes_noun = map_to_noun(slab, tomes_entries);
    T(slab, &[garb_noun, type_noun, semi_noun, tomes_noun])
}

fn garb_to_noun(slab: &mut NounSlab, garb: &Garb) -> Noun {
    let name_noun = match garb.name.as_ref() {
        None => D(0),
        Some(s) => {
            let term_noun = term_to_noun(slab, s);
            T(slab, &[D(0), term_noun])
        }
    };
    let poly_noun = poly_to_noun(slab, &garb.poly);
    let vair_noun = vair_to_noun(slab, &garb.vair);
    T(slab, &[name_noun, poly_noun, vair_noun])
}

fn poly_to_noun(_slab: &mut NounSlab, poly: &Poly) -> Noun {
    match poly {
        Poly::Wet => D(tas!(b"wet")),
        Poly::Dry => D(tas!(b"dry")),
    }
}

fn vair_to_noun(_slab: &mut NounSlab, vair: &Vair) -> Noun {
    match vair {
        Vair::Gold => D(tas!(b"gold")),
        Vair::Iron => D(tas!(b"iron")),
        Vair::Lead => D(tas!(b"lead")),
        Vair::Zinc => D(tas!(b"zinc")),
    }
}

fn semi_noun_expr_to_noun(slab: &mut NounSlab, (stencil, expr): &SemiNounExpr) -> Noun {
    let stencil_noun = stencil_to_noun(slab, stencil);
    let expr_noun = noun_expr_to_noun(slab, expr);
    T(slab, &[stencil_noun, expr_noun])
}

fn stencil_to_noun(slab: &mut NounSlab, st: &Stencil) -> Noun {
    match st {
        Stencil::Half { left, rite } => {
            let l = stencil_to_noun(slab, left);
            let r = stencil_to_noun(slab, rite);
            T(slab, &[D(tas!(b"half")), l, r])
        }
        Stencil::Full { blocks } => {
            let blocks_vec: Vec<_> = blocks.iter().map(|b| block_to_noun(slab, b)).collect();
            let blocks_noun = list_to_noun(slab, blocks_vec);
            T(slab, &[D(tas!(b"full")), blocks_noun])
        }
        Stencil::Lazy { fragment, resolve } => {
            let gate_noun = gate_to_noun(slab, resolve);
            T(slab, &[D(tas!(b"lazy")), D(*fragment), gate_noun])
        }
    }
}

fn block_to_noun(slab: &mut NounSlab, block: &Block) -> Noun {
    let paths: Vec<_> = block.iter().map(|path| path_to_noun(slab, path)).collect();
    list_to_noun(slab, paths)
}

fn path_to_noun(slab: &mut NounSlab, path: &Path) -> Noun {
    let knots: Vec<_> = path.iter().map(|k| cord_to_noun(slab, k)).collect();
    list_to_noun(slab, knots)
}

fn gate_to_noun(slab: &mut NounSlab, (spec, body): &Gate) -> Noun {
    let spec_noun = spec_to_noun(slab, spec);
    let body_noun = spec_to_noun(slab, body);
    T(slab, &[spec_noun, body_noun])
}

fn spec_to_noun(slab: &mut NounSlab, spec: &Spec) -> Noun {
    use Spec::*;
    match spec {
        Base(bt) => {
            let bt_noun = basetype_to_noun(slab, bt);
            T(slab, &[D(tas!(b"base")), bt_noun])
        }
        Dbug(spot, s) => {
            let spot_noun = spot_to_noun(slab, spot);
            let s_noun = spec_to_noun(slab, s);
            T(slab, &[D(tas!(b"dbug")), spot_noun, s_noun])
        }
        Gist(help, s) => {
            let help_noun = noun_expr_to_noun(slab, help);
            let help = T(slab, &[D(tas!(b"help")), help_noun]);
            let s_noun = spec_to_noun(slab, s);
            T(slab, &[D(tas!(b"gist")), help, s_noun])
        }
        Leaf(tag, atom) => {
            let tag_noun = term_to_noun(slab, tag);
            let atom_noun = atom_to_noun(slab, atom);
            T(slab, &[D(tas!(b"leaf")), tag_noun, atom_noun])
        }
        Like(wing, wings) => {
            let wing_noun = wing_to_noun(slab, wing);
            let wings_vec: Vec<_> = wings.iter().map(|w| wing_to_noun(slab, w)).collect();
            let wings_noun = list_to_noun(slab, wings_vec);
            T(slab, &[D(tas!(b"like")), wing_noun, wings_noun])
        }
        Loop(name) => {
            let name_noun = term_to_noun(slab, name);
            T(slab, &[D(tas!(b"loop")), name_noun])
        }
        Made((name, args), s) => {
            let name_noun = term_to_noun(slab, name);
            let args_vec: Vec<_> = args.iter().map(|a| term_to_noun(slab, a)).collect();
            let args_noun = list_to_noun(slab, args_vec);
            let s_noun = spec_to_noun(slab, s);
            let inner = T(slab, &[name_noun, args_noun]);
            T(slab, &[D(tas!(b"made")), inner, s_noun])
        }
        Make(hoon, specs) => {
            let hoon_noun = hoon_to_noun(slab, hoon);
            let specs_vec: Vec<_> = specs.iter().map(|s| spec_to_noun(slab, s)).collect();
            let specs_noun = list_to_noun(slab, specs_vec);
            T(slab, &[D(tas!(b"make")), hoon_noun, specs_noun])
        }
        Name(name, s) => {
            let name_noun = term_to_noun(slab, name);
            let s_noun = spec_to_noun(slab, s);
            T(slab, &[D(tas!(b"name")), name_noun, s_noun])
        }
        Over(wing, s) => {
            let wing_noun = wing_to_noun(slab, wing);
            let s_noun = spec_to_noun(slab, s);
            T(slab, &[D(tas!(b"over")), wing_noun, s_noun])
        }
        BucGar(a, b) => {
            let a_noun = spec_to_noun(slab, a);
            let b_noun = spec_to_noun(slab, b);
            T(slab, &[D(tas!(b"bcgr")), a_noun, b_noun])
        }
        BucBuc(a, map) => {
            let a_noun = spec_to_noun(slab, a);
            let entries: Vec<_> = map
                .iter()
                .map(|(k, v)| (term_to_noun(slab, k), spec_to_noun(slab, v)))
                .collect();
            let map_noun = map_to_noun(slab, entries);
            T(slab, &[D(tas!(b"bcbc")), a_noun, map_noun])
        }
        BucBar(a, h) => {
            let a_noun = spec_to_noun(slab, a);
            let h_noun = hoon_to_noun(slab, h);
            T(slab, &[D(tas!(b"bcbr")), a_noun, h_noun])
        }
        BucCab(h) => {
            let h_noun = hoon_to_noun(slab, h);
            T(slab, &[D(tas!(b"bccb")), h_noun])
        }
        BucCol(a, specs) => {
            let a_noun = spec_to_noun(slab, a);
            let specs_vec: Vec<_> = specs.iter().map(|s| spec_to_noun(slab, s)).collect();
            let specs_noun = list_to_noun(slab, specs_vec);
            T(slab, &[D(tas!(b"bccl")), a_noun, specs_noun])
        }
        BucCen(a, specs) => {
            let a_noun = spec_to_noun(slab, a);
            let specs_vec: Vec<_> = specs.iter().map(|s| spec_to_noun(slab, s)).collect();
            let specs_noun = list_to_noun(slab, specs_vec);
            T(slab, &[D(tas!(b"bccn")), a_noun, specs_noun])
        }
        BucDot(a, map) => {
            let a_noun = spec_to_noun(slab, a);
            let entries: Vec<_> = map
                .iter()
                .map(|(k, v)| (term_to_noun(slab, k), spec_to_noun(slab, v)))
                .collect();
            let map_noun = map_to_noun(slab, entries);
            T(slab, &[D(tas!(b"bcdt")), a_noun, map_noun])
        }
        BucGal(a, b) => {
            let a_noun = spec_to_noun(slab, a);
            let b_noun = spec_to_noun(slab, b);
            T(slab, &[D(tas!(b"bcgl")), a_noun, b_noun])
        }
        BucHep(a, b) => {
            let a_noun = spec_to_noun(slab, a);
            let b_noun = spec_to_noun(slab, b);
            T(slab, &[D(tas!(b"bchp")), a_noun, b_noun])
        }
        BucKet(a, b) => {
            let a_noun = spec_to_noun(slab, a);
            let b_noun = spec_to_noun(slab, b);
            T(slab, &[D(tas!(b"bckt")), a_noun, b_noun])
        }
        BucLus(tag, s) => {
            let tag_noun = term_to_noun(slab, tag);
            let s_noun = spec_to_noun(slab, s);
            T(slab, &[D(tas!(b"bcls")), tag_noun, s_noun])
        }
        BucFas(a, map) => {
            let a_noun = spec_to_noun(slab, a);
            let entries: Vec<_> = map
                .iter()
                .map(|(k, v)| (term_to_noun(slab, k), spec_to_noun(slab, v)))
                .collect();
            let map_noun = map_to_noun(slab, entries);
            T(slab, &[D(tas!(b"bcfs")), a_noun, map_noun])
        }
        BucMic(h) => {
            let inner = hoon_to_noun(slab, h);
            T(slab, &[D(tas!(b"bcmc")), inner])
        }
        BucPam(a, h) => {
            let a_noun = spec_to_noun(slab, a);
            let h_noun = hoon_to_noun(slab, h);
            T(slab, &[D(tas!(b"bcpm")), a_noun, h_noun])
        }
        BucSig(h, a) => {
            let h_noun = hoon_to_noun(slab, h);
            let a_noun = spec_to_noun(slab, a);
            T(slab, &[D(tas!(b"bcsg")), h_noun, a_noun])
        }
        BucTic(a, map) => {
            let a_noun = spec_to_noun(slab, a);
            let entries: Vec<_> = map
                .iter()
                .map(|(k, v)| (term_to_noun(slab, k), spec_to_noun(slab, v)))
                .collect();
            let map_noun = map_to_noun(slab, entries);
            T(slab, &[D(tas!(b"bctc")), a_noun, map_noun])
        }
        BucTis(skin, a) => {
            let skin_noun = skin_to_noun(slab, skin);
            let a_noun = spec_to_noun(slab, a);
            T(slab, &[D(tas!(b"bcts")), skin_noun, a_noun])
        }
        BucPat(a, b) => {
            let a_noun = spec_to_noun(slab, a);
            let b_noun = spec_to_noun(slab, b);
            T(slab, &[D(tas!(b"bcpt")), a_noun, b_noun])
        }
        BucWut(a, specs) => {
            let a_noun = spec_to_noun(slab, a);
            let specs_vec: Vec<_> = specs.iter().map(|s| spec_to_noun(slab, s)).collect();
            let specs_noun = list_to_noun(slab, specs_vec);
            T(slab, &[D(tas!(b"bcwt")), a_noun, specs_noun])
        }
        BucZap(a, map) => {
            let a_noun = spec_to_noun(slab, a);
            let entries: Vec<_> = map
                .iter()
                .map(|(k, v)| (term_to_noun(slab, k), spec_to_noun(slab, v)))
                .collect();
            let map_noun = map_to_noun(slab, entries);
            T(slab, &[D(tas!(b"bczp")), a_noun, map_noun])
        }
    }
}

fn skin_to_noun(slab: &mut NounSlab, skin: &Skin) -> Noun {
    use Skin::*;
    match skin {
        Term(s) => term_to_noun(slab, s),
        Base(bt) => {
            let inner = basetype_to_noun(slab, bt);
            T(slab, &[D(tas!(b"base")), inner])
        }
        Cell(l, r) => {
            let l = skin_to_noun(slab, l);
            let r = skin_to_noun(slab, r);
            T(slab, &[D(tas!(b"cell")), l, r])
        }
        Dbug(spot, s) => {
            let spot_noun = spot_to_noun(slab, spot);
            let s_noun = skin_to_noun(slab, s);
            T(slab, &[D(tas!(b"dbug")), spot_noun, s_noun])
        }
        Help(help, s) => {
            let help_noun = noun_expr_to_noun(slab, help);
            let s_noun = skin_to_noun(slab, s);
            T(slab, &[D(tas!(b"help")), help_noun, s_noun])
        }
        Leaf(tag, atom) => {
            let tag_noun = term_to_noun(slab, tag);
            let atom_noun = atom_to_noun(slab, atom);
            T(slab, &[D(tas!(b"leaf")), tag_noun, atom_noun])
        }
        Name(name, s) => {
            let name_noun = term_to_noun(slab, name);
            let s_noun = skin_to_noun(slab, s);
            T(slab, &[D(tas!(b"name")), name_noun, s_noun])
        }
        Over(wing, s) => {
            let wing_noun = wing_to_noun(slab, wing);
            let s_noun = skin_to_noun(slab, s);
            T(slab, &[D(tas!(b"over")), wing_noun, s_noun])
        }
        Spec(spec, s) => {
            let spec_noun = spec_to_noun(slab, spec);
            let s_noun = skin_to_noun(slab, s);
            T(slab, &[D(tas!(b"spec")), spec_noun, s_noun])
        }
        Wash(n) => T(slab, &[D(tas!(b"wash")), D(*n)]),
    }
}

fn wing_to_noun(slab: &mut NounSlab, wing: &WingType) -> Noun {
    let limbs: Vec<Noun> = wing.iter().map(|l| limb_to_noun(slab, l)).collect();

    list_to_noun(slab, limbs)
}

fn limb_to_noun(slab: &mut NounSlab, limb: &Limb) -> Noun {
    match limb {
        Limb::Term(s) => term_to_noun(slab, s),

        Limb::Axis(n) => T(slab, &[D(0), D(*n)]),

        Limb::Parent(n, opt) => {
            let opt_noun = match opt {
                Some(s) => {
                    let s_noun = term_to_noun(slab, s);
                    T(slab, &[D(0), s_noun])
                }
                None => D(0),
            };

            T(slab, &[D(1), D(*n), opt_noun])
        }
    }
}

fn spot_to_noun(slab: &mut NounSlab, spot: &Spot) -> Noun {
    let path_noun = path_to_noun(slab, &spot.p);
    let pint_noun = pint_to_noun(slab, &spot.q);
    T(slab, &[path_noun, pint_noun])
}

fn pint_to_noun(slab: &mut NounSlab, pint: &Pint) -> Noun {
    let p = T(slab, &[D(pint.p.0), D(pint.p.1)]);
    let q = T(slab, &[D(pint.q.0), D(pint.q.1)]);
    T(slab, &[p, q])
}

fn note_to_noun(slab: &mut NounSlab, note: &Note) -> Noun {
    match note {
        Note::Help(help) => {
            let help_noun = noun_expr_to_noun(slab, help);
            T(slab, &[D(tas!(b"help")), help_noun])
        }

        Note::Know(s) => {
            let s_noun = term_to_noun(slab, s);
            T(slab, &[D(tas!(b"know")), s_noun])
        }

        Note::Made(s, opt_wings) => {
            let s_noun = term_to_noun(slab, s);

            let wings_noun = opt_wings.as_ref().map(|wings| {
                let wing_nouns: Vec<Noun> = wings.iter().map(|w| wing_to_noun(slab, w)).collect();

                list_to_noun(slab, wing_nouns)
            });

            let wings_noun = match wings_noun {
                None => D(0),
                Some(p) => T(slab, &[D(0), p]),
            };

            T(slab, &[D(tas!(b"made")), s_noun, wings_noun])
        }
    }
}

fn woof_to_noun(slab: &mut NounSlab, woof: &Woof) -> Noun {
    match woof {
        Woof::ParsedAtom(a) => {
            let val = atom_to_noun(slab, a);
            val
        }
        Woof::Hoon(h) => {
            let val = hoon_to_noun(slab, h);
            T(slab, &[D(0), val])
        }
    }
}

fn tome_to_noun(slab: &mut NounSlab, tome: &Tome) -> Noun {
    let what = tome
        .0
        .as_ref()
        .map(|what| noun_expr_to_noun(slab, what))
        .unwrap_or_else(|| D(0));
    let pairs: Vec<_> = tome
        .1
        .iter()
        .map(|(k, v)| (term_to_noun(slab, k), hoon_to_noun(slab, v)))
        .collect();
    let map = map_to_noun(slab, pairs);
    T(slab, &[what, map])
}

fn alas_to_noun(slab: &mut NounSlab, alas: &Alas) -> Noun {
    let pairs: Vec<Noun> = alas
        .iter()
        .map(|(k, v)| {
            let k_noun = term_to_noun(slab, k);
            let v_noun = hoon_to_noun(slab, v);
            T(slab, &[k_noun, v_noun])
        })
        .collect();
    list_to_noun(slab, pairs)
}

fn tyre_to_noun(slab: &mut NounSlab, tyre: &Tyre) -> Noun {
    let pairs: Vec<Noun> = tyre
        .iter()
        .map(|(k, v)| {
            let k_noun = term_to_noun(slab, k);
            let v_noun = hoon_to_noun(slab, v);
            T(slab, &[k_noun, v_noun])
        })
        .collect();
    list_to_noun(slab, pairs)
}

fn chum_to_noun(slab: &mut NounSlab, chum: &Chum) -> Noun {
    match chum {
        Chum::Lef(s) => term_to_noun(slab, s),
        Chum::StdKel(s, a) => {
            let s_noun = term_to_noun(slab, s);
            let a_noun = atom_to_noun(slab, a);
            T(slab, &[s_noun, a_noun])
        }
        Chum::VenProKel(v, p, a) => {
            let v_noun = term_to_noun(slab, v);
            let p_noun = term_to_noun(slab, p);
            let a_noun = atom_to_noun(slab, a);
            T(slab, &[v_noun, p_noun, a_noun])
        }
        Chum::VenProVerKel(v, p, a1, a2) => {
            let v_noun = term_to_noun(slab, v);
            let p_noun = term_to_noun(slab, p);
            let a1_noun = atom_to_noun(slab, a1);
            let a2_noun = atom_to_noun(slab, a2);
            T(slab, &[v_noun, p_noun, a1_noun, a2_noun])
        }
    }
}

fn nock_to_noun(slab: &mut NounSlab, nock: &Nock) -> Noun {
    use Nock::*;
    match nock {
        Pair(a, b) => {
            let a_noun = nock_to_noun(slab, a);
            let b_noun = nock_to_noun(slab, b);
            T(slab, &[D(2u64), a_noun, b_noun])
        }
        Const(expr) => {
            let expr_noun = noun_expr_to_noun(slab, expr);
            T(slab, &[D(1u64), expr_noun])
        }
        Compose(f, g) => {
            let f_noun = nock_to_noun(slab, f);
            let g_noun = nock_to_noun(slab, g);
            T(slab, &[D(7u64), f_noun, g_noun])
        }
        CellTest(n) => {
            let n_noun = nock_to_noun(slab, n);
            T(slab, &[D(3u64), n_noun])
        }
        Increment(n) => {
            let n_noun = nock_to_noun(slab, n);
            T(slab, &[D(4u64), n_noun])
        }
        Equality(a, b) => {
            let a_noun = nock_to_noun(slab, a);
            let b_noun = nock_to_noun(slab, b);
            T(slab, &[D(5u64), a_noun, b_noun])
        }
        IfThenElse(cond, yes, no) => {
            let cond_noun = nock_to_noun(slab, cond);
            let yes_noun = nock_to_noun(slab, yes);
            let no_noun = nock_to_noun(slab, no);
            T(slab, &[D(6u64), cond_noun, yes_noun, no_noun])
        }
        Edit((axis, new), core) => {
            let new_noun = nock_to_noun(slab, new);
            let core_noun = nock_to_noun(slab, core);
            let axis_cell = T(slab, &[D(*axis), new_noun]);
            T(slab, &[D(11u64), axis_cell, core_noun])
        }
        Hint(hint, n) => {
            let hint_noun = nock_hint_to_noun(slab, hint);
            let n_noun = nock_to_noun(slab, n);
            T(slab, &[D(12u64), hint_noun, n_noun])
        }
        SerialCompose(f, g) => {
            let f = nock_to_noun(slab, f);
            let g = nock_to_noun(slab, g);
            T(slab, &[D(8u64), f, g])
        }
        PushSubject(n, subj) => {
            let n = nock_to_noun(slab, n);
            let subj = nock_to_noun(slab, subj);
            T(slab, &[D(9u64), n, subj])
        }
        SelectArm(axis, core) => {
            let core = nock_to_noun(slab, core);
            T(slab, &[D(10u64), D(*axis), core])
        }
        GrabData(core, path) => {
            let core = nock_to_noun(slab, core);
            let path = nock_to_noun(slab, path);
            T(slab, &[D(13u64), core, path])
        }
        AxisSelect(axis) => D(*axis),
    }
}

fn nock_hint_to_noun(slab: &mut NounSlab, hint: &NockHint) -> Noun {
    match hint {
        NockHint::ParsedAtom(a) => D(*a),
        NockHint::Pair(tag, n) => {
            let n_noun = nock_to_noun(slab, n);
            T(slab, &[D(*tag), n_noun])
        }
    }
}

fn term_or_tune_to_noun(slab: &mut NounSlab, tot: &TermOrTune) -> Noun {
    match tot {
        TermOrTune::Term(s) => term_to_noun(slab, s),
        TermOrTune::Tune(tune) => tune_to_noun(slab, tune),
    }
}

fn tune_to_noun(slab: &mut NounSlab, (map, vec): &Tune) -> Noun {
    let map_pairs: Vec<_> = map
        .iter()
        .map(|(k, opt_v)| {
            let k_noun = term_to_noun(slab, k);
            let v_noun = match opt_v {
                None => D(0),
                Some(v) => {
                    let hoon_noun = hoon_to_noun(slab, v);
                    T(slab, &[D(0), hoon_noun])
                }
            };
            (k_noun, v_noun)
        })
        .collect();

    let map_noun = map_to_noun(slab, map_pairs);

    let vec_nouns: Vec<_> = vec.iter().map(|v| hoon_to_noun(slab, v)).collect();

    let vec_noun = list_to_noun(slab, vec_nouns);

    T(slab, &[map_noun, vec_noun])
}

fn term_or_pair_to_noun(slab: &mut NounSlab, top: &TermOrPair) -> Noun {
    match top {
        TermOrPair::Term(s) => term_to_noun(slab, s),
        TermOrPair::Pair(s, h) => {
            let s_noun = term_to_noun(slab, s);
            let h_noun = hoon_to_noun(slab, h);
            T(slab, &[s_noun, h_noun])
        }
    }
}

fn zpwt_arg_to_noun(slab: &mut NounSlab, arg: &ZpwtArg) -> Noun {
    match arg {
        ZpwtArg::ParsedAtom(s) => {
            let tag = D(tas!(b"atom"));
            let s_noun = cord_to_noun(slab, s);
            T(slab, &[tag, s_noun])
        }
        ZpwtArg::Pair(s1, s2) => {
            let tag = D(tas!(b"pair"));
            let s1_noun = cord_to_noun(slab, s1);
            let s2_noun = cord_to_noun(slab, s2);
            T(slab, &[tag, s1_noun, s2_noun])
        }
    }
}

fn mane_to_noun(slab: &mut NounSlab, mane: &Mane) -> Noun {
    match mane {
        Mane::Tag(s) => term_to_noun(slab, s),
        Mane::TagSpace(s1, s2) => {
            let s1_noun = term_to_noun(slab, s1);
            let s2_noun = term_to_noun(slab, s2);
            T(slab, &[s1_noun, s2_noun])
        }
    }
}

fn marx_to_noun(slab: &mut NounSlab, marx: &Marx) -> Noun {
    let n = mane_to_noun(slab, &marx.n);
    let a = mart_to_noun(slab, &marx.a);
    T(slab, &[n, a])
}

fn manx_to_noun(slab: &mut NounSlab, manx: &Manx) -> Noun {
    let g = marx_to_noun(slab, &manx.g);
    let c = marl_to_noun(slab, &manx.c);
    T(slab, &[g, c])
}

fn mart_to_noun(slab: &mut NounSlab, mart: &Mart) -> Noun {
    let cells: Vec<Noun> = mart
        .iter()
        .map(|(mane, beers)| {
            let mane_noun = mane_to_noun(slab, mane);

            let beer_nouns: Vec<Noun> = beers.iter().map(|b| beer_to_noun(slab, b)).collect();

            let beers_noun = list_to_noun(slab, beer_nouns);

            T(slab, &[mane_noun, beers_noun])
        })
        .collect();

    list_to_noun(slab, cells)
}

fn beer_to_noun(slab: &mut NounSlab, beer: &Beer) -> Noun {
    match beer {
        Beer::Char(cord) => cord_to_noun(slab, cord),
        Beer::Hoon(h) => {
            let hoon_noun = hoon_to_noun(slab, h);
            T(slab, &[D(0), hoon_noun])
        }
    }
}

fn marl_to_noun(slab: &mut NounSlab, marl: &Marl) -> Noun {
    let items: Vec<Noun> = marl.iter().map(|t| tuna_to_noun(slab, t)).collect();

    list_to_noun(slab, items)
}

fn tuna_to_noun(slab: &mut NounSlab, tuna: &Tuna) -> Noun {
    match tuna {
        Tuna::Manx(m) => manx_to_noun(slab, m),
        Tuna::TunaTail(tail) => tuna_tail_to_noun(slab, tail),
    }
}

fn tuna_tail_to_noun(slab: &mut NounSlab, tail: &TunaTail) -> Noun {
    match tail {
        TunaTail::Tape(h) => {
            let h_noun = hoon_to_noun(slab, h);
            T(slab, &[D(tas!(b"tape")), h_noun])
        }
        TunaTail::Manx(h) => {
            let h_noun = hoon_to_noun(slab, h);
            T(slab, &[D(tas!(b"manx")), h_noun])
        }
        TunaTail::Marl(h) => {
            let h_noun = hoon_to_noun(slab, h);
            T(slab, &[D(tas!(b"marl")), h_noun])
        }
        TunaTail::Call(h) => {
            let h_noun = hoon_to_noun(slab, h);
            T(slab, &[D(tas!(b"call")), h_noun])
        }
    }
}

// pub fn lth_b(slab: &mut NounSlab, a: Atom, b: Atom) -> bool {
//     if let (Ok(a), Ok(b)) = (a.as_direct(), b.as_direct()) {
//         a.data() < b.data()
//     } else if a.bit_size() > b.bit_size() {
//         false
//     } else if a.bit_size() < b.bit_size() {
//         true
//     } else {
//         a.as_ubig(stack) < b.as_ubig(stack)
//     }
// }

// pub fn lth(slab: &mut NounSlab, a: Atom, b: Atom) -> Noun {
//     if lth_b(stack, a, b) {
//         YES
//     } else {
//         NO
//     }
// }

// ---- noun_to_hoon: reverse conversion from gene nouns to Hoon ASTs ----
//
// This is the inverse of `hoon_to_noun`. It decodes a noun produced by the
// Hoon compiler (hoonc) or by `hoon_to_noun` back into a Rust `Hoon` AST.
// Used by the native compiler to decode gene nouns from oracle type trees
// (e.g. holds in hoonc's sut type) so they can be processed by `play`.

fn noun_to_direct(noun: NounHandle<'_>) -> Result<u64, String> {
    noun.as_atom()
        .map_err(|_| "expected atom".into())
        .and_then(|a| {
            a.as_direct()
                .map(|d| d.data())
                .map_err(|_| "expected direct atom".into())
        })
}

fn noun_atom_to_text(noun: NounHandle<'_>, zero_is_dollar: bool) -> Result<String, String> {
    let atom = noun.as_atom().map_err(|_| "expected atom for term")?;
    let bytes = atom.as_ne_bytes();
    let len = bytes
        .iter()
        .rposition(|&b| b != 0)
        .map(|i| i + 1)
        .unwrap_or(0);
    if len == 0 {
        return Ok(if zero_is_dollar {
            "$".to_string()
        } else {
            String::new()
        });
    }
    String::from_utf8(bytes[..len].to_vec()).map_err(|e| format!("invalid UTF-8 in term: {e}"))
}

fn noun_to_term(noun: NounHandle<'_>) -> Result<String, String> {
    noun_atom_to_text(noun, true)
}

fn noun_to_cord(noun: NounHandle<'_>) -> Result<String, String> {
    noun_atom_to_text(noun, false) // same byte encoding; different zero semantics
}

fn noun_to_parsed_atom(noun: NounHandle<'_>) -> Result<ParsedAtom, String> {
    let atom = noun
        .as_atom()
        .map_err(|_| "expected atom for parsed_atom")?;
    if let Ok(d) = atom.as_direct() {
        Ok(ParsedAtom::Small(d.data() as u128))
    } else {
        let bytes = atom.as_ne_bytes();
        let big = BigUint::from_bytes_le(bytes);
        Ok(ParsedAtom::Big(big))
    }
}

fn noun_to_noun_expr(noun: NounHandle<'_>) -> Result<NounExpr, String> {
    match noun.as_cell() {
        Ok(cell) => {
            let l = noun_to_noun_expr(cell.head())?;
            let r = noun_to_noun_expr(cell.tail())?;
            Ok(NounExpr::Cell(Box::new(l), Box::new(r)))
        }
        Err(_) => {
            let atom = noun_to_parsed_atom(noun)?;
            Ok(NounExpr::ParsedAtom(atom))
        }
    }
}

fn noun_to_list<'a, T, F>(noun: NounHandle<'a>, f: F) -> Result<Vec<T>, String>
where
    F: Fn(NounHandle<'a>) -> Result<T, String>,
{
    let mut result = Vec::new();
    let mut cur = noun;
    loop {
        if let Ok(a) = cur.as_atom() {
            if let Ok(d) = a.as_direct() {
                if d.data() == 0 {
                    break;
                }
            }
            return Err("list: unexpected non-zero atom tail".into());
        }
        let cell = cur.as_cell().map_err(|_| "list: expected cell")?;
        result.push(f(cell.head())?);
        cur = cell.tail();
    }
    Ok(result)
}

fn noun_is_zero_handle(noun: NounHandle<'_>) -> bool {
    noun.as_atom()
        .ok()
        .and_then(|atom| atom.as_direct().ok())
        .is_some_and(|direct| direct.data() == 0)
}

fn noun_to_fork_set_options<'a>(noun: NounHandle<'a>) -> Result<Vec<Type>, String> {
    let mut out = Vec::new();
    let mut stack: Vec<NounHandle<'a>> = Vec::new();
    let mut current = noun;
    let mut visited_nodes = 0usize;

    loop {
        while !noun_is_zero_handle(current) {
            visited_nodes = visited_nodes.saturating_add(1);
            if visited_nodes > 1_000_000 {
                return Err("fork set decode exceeded node budget".into());
            }
            let cell = current
                .as_cell()
                .map_err(|_| "fork set: expected node cell")?;
            let branches = cell
                .tail()
                .as_cell()
                .map_err(|_| "fork set: expected branch cell")?;
            stack.push(current);
            current = branches.tail();
        }

        let Some(node) = stack.pop() else {
            break;
        };
        let cell = node.as_cell().map_err(|_| "fork set: expected node cell")?;
        out.push(noun_to_type(cell.head())?);
        let branches = cell
            .tail()
            .as_cell()
            .map_err(|_| "fork set: expected branch cell")?;
        current = branches.head();
    }

    Ok(out)
}

fn noun_to_opt<'a, T, F>(noun: NounHandle<'a>, f: F) -> Result<Option<T>, String>
where
    F: FnOnce(NounHandle<'a>) -> Result<T, String>,
{
    if let Ok(a) = noun.as_atom() {
        if let Ok(d) = a.as_direct() {
            if d.data() == 0 {
                return Ok(None);
            }
        }
    }
    // unit: [~ value]
    let cell = noun.as_cell().map_err(|_| "unit: expected cell")?;
    Ok(Some(f(cell.tail())?))
}

fn noun_to_basetype(noun: NounHandle<'_>) -> Result<BaseType, String> {
    if let Ok(cell) = noun.as_cell() {
        let tag = noun_to_direct(cell.head())?;
        if tag == tas!(b"atom") {
            let au = noun_to_term(cell.tail())?;
            return Ok(BaseType::Atom(au));
        }
        return Err(format!("basetype: unknown cell tag {tag}"));
    }
    let tag = noun_to_direct(noun)?;
    if tag == tas!(b"noun") {
        return Ok(BaseType::NounExpr);
    }
    if tag == tas!(b"cell") {
        return Ok(BaseType::Cell);
    }
    if tag == tas!(b"flag") {
        return Ok(BaseType::Flag);
    }
    if tag == tas!(b"null") {
        return Ok(BaseType::Null);
    }
    if tag == tas!(b"void") {
        return Ok(BaseType::Void);
    }
    Err(format!("basetype: unknown tag {tag}"))
}

fn noun_to_spot(noun: NounHandle<'_>) -> Result<Spot, String> {
    let cell = noun.as_cell().map_err(|_| "spot: expected cell")?;
    let path_noun = cell.head();
    let pint_noun = cell.tail();
    let p = noun_to_list(path_noun, noun_to_cord)?;
    let q = noun_to_pint(pint_noun)?;
    Ok(Spot { p, q })
}

fn noun_to_pint(noun: NounHandle<'_>) -> Result<Pint, String> {
    let cell = noun.as_cell().map_err(|_| "pint: expected cell")?;
    let p_cell = cell.head().as_cell().map_err(|_| "pint.p: expected cell")?;
    let q_cell = cell.tail().as_cell().map_err(|_| "pint.q: expected cell")?;
    let p = (
        noun_to_direct(p_cell.head())?,
        noun_to_direct(p_cell.tail())?,
    );
    let q = (
        noun_to_direct(q_cell.head())?,
        noun_to_direct(q_cell.tail())?,
    );
    Ok(Pint { p, q })
}

fn noun_to_note(noun: NounHandle<'_>) -> Result<Note, String> {
    let cell = noun.as_cell().map_err(|_| "note: expected cell")?;
    let tag = noun_to_direct(cell.head())?;
    if tag == tas!(b"know") {
        let s = noun_to_term(cell.tail())?;
        Ok(Note::Know(s))
    } else if tag == tas!(b"made") {
        let rest = cell
            .tail()
            .as_cell()
            .map_err(|_| "note made: expected cell")?;
        let s = noun_to_term(rest.head())?;
        let opt_wings = noun_to_opt(rest.tail(), |n| noun_to_list(n, noun_to_wing))?;
        Ok(Note::Made(s, opt_wings))
    } else if tag == tas!(b"germ") {
        // hoon-138 has %germ which we map to Know
        let s = noun_to_term(cell.tail())?;
        Ok(Note::Know(s))
    } else if tag == tas!(b"help") {
        Ok(Note::Help(noun_to_noun_expr(cell.tail())?))
    } else {
        Err(format!("note: unknown tag {tag}"))
    }
}

fn noun_to_wing(noun: NounHandle<'_>) -> Result<Vec<Limb>, String> {
    noun_to_list(noun, noun_to_limb)
}

fn noun_to_limb(noun: NounHandle<'_>) -> Result<Limb, String> {
    if let Ok(atom) = noun.as_atom() {
        // Bare atom → term limb (unless it's 0, which shouldn't happen)
        let s = noun_to_term(noun)?;
        return Ok(Limb::Term(s));
    }
    let cell = noun.as_cell().map_err(|_| "limb: expected cell")?;
    let head_val = noun_to_direct(cell.head())?;
    if head_val == 0 {
        // [0 axis] → Axis limb
        let axis = noun_to_direct(cell.tail())?;
        Ok(Limb::Axis(axis))
    } else if head_val == 1 {
        // [1 n opt] → Parent limb
        let rest = cell
            .tail()
            .as_cell()
            .map_err(|_| "parent limb: expected cell")?;
        let n = noun_to_direct(rest.head())?;
        let opt = noun_to_opt(rest.tail(), noun_to_term)?;
        Ok(Limb::Parent(n, opt))
    } else {
        Err(format!("limb: unexpected head {head_val}"))
    }
}

fn noun_to_skin(noun: NounHandle<'_>) -> Result<Skin, String> {
    // Skin::Term is a bare atom
    if let Ok(_) = noun.as_atom() {
        let s = noun_to_term(noun)?;
        return Ok(Skin::Term(s));
    }
    let cell = noun.as_cell().map_err(|_| "skin: expected cell")?;
    let head = cell.head();
    // Check for tagged variant
    if let Ok(tag) = noun_to_direct(head) {
        if tag == tas!(b"base") {
            return Ok(Skin::Base(noun_to_basetype(cell.tail())?));
        }
        if tag == tas!(b"cell") {
            let rest = cell.tail().as_cell().map_err(|_| "skin cell")?;
            return Ok(Skin::Cell(
                Box::new(noun_to_skin(rest.head())?),
                Box::new(noun_to_skin(rest.tail())?),
            ));
        }
        if tag == tas!(b"dbug") {
            let rest = cell.tail().as_cell().map_err(|_| "skin dbug")?;
            return Ok(Skin::Dbug(
                noun_to_spot(rest.head())?,
                Box::new(noun_to_skin(rest.tail())?),
            ));
        }
        if tag == tas!(b"help") {
            let rest = cell.tail().as_cell().map_err(|_| "skin help")?;
            return Ok(Skin::Help(
                noun_to_noun_expr(rest.head())?,
                Box::new(noun_to_skin(rest.tail())?),
            ));
        }
        if tag == tas!(b"leaf") {
            let rest = cell.tail().as_cell().map_err(|_| "skin leaf")?;
            return Ok(Skin::Leaf(
                noun_to_term(rest.head())?,
                noun_to_parsed_atom(rest.tail())?,
            ));
        }
        if tag == tas!(b"name") {
            let rest = cell.tail().as_cell().map_err(|_| "skin name")?;
            return Ok(Skin::Name(
                noun_to_term(rest.head())?,
                Box::new(noun_to_skin(rest.tail())?),
            ));
        }
        if tag == tas!(b"over") {
            let rest = cell.tail().as_cell().map_err(|_| "skin over")?;
            return Ok(Skin::Over(
                noun_to_wing(rest.head())?,
                Box::new(noun_to_skin(rest.tail())?),
            ));
        }
        if tag == tas!(b"spec") {
            let rest = cell.tail().as_cell().map_err(|_| "skin spec")?;
            return Ok(Skin::Spec(
                Box::new(noun_to_spec(rest.head())?),
                Box::new(noun_to_skin(rest.tail())?),
            ));
        }
        if tag == tas!(b"wash") {
            return Ok(Skin::Wash(noun_to_direct(cell.tail())?));
        }
    }
    Err(format!("skin: unrecognized"))
}

fn noun_to_spec(noun: NounHandle<'_>) -> Result<Spec, String> {
    let cell = noun.as_cell().map_err(|_| "spec: expected cell")?;
    let tag = noun_to_direct(cell.head())?;
    let rest = cell.tail();
    if tag == tas!(b"base") {
        return Ok(Spec::Base(noun_to_basetype(rest)?));
    }
    if tag == tas!(b"dbug") {
        let r = rest.as_cell().map_err(|_| "spec dbug")?;
        return Ok(Spec::Dbug(
            noun_to_spot(r.head())?,
            Box::new(noun_to_spec(r.tail())?),
        ));
    }
    if tag == tas!(b"leaf") {
        let r = rest.as_cell().map_err(|_| "spec leaf")?;
        return Ok(Spec::Leaf(
            noun_to_term(r.head())?,
            noun_to_parsed_atom(r.tail())?,
        ));
    }
    if tag == tas!(b"like") {
        let r = rest.as_cell().map_err(|_| "spec like")?;
        return Ok(Spec::Like(
            noun_to_wing(r.head())?,
            noun_to_list(r.tail(), noun_to_wing)?,
        ));
    }
    if tag == tas!(b"loop") {
        return Ok(Spec::Loop(noun_to_term(rest)?));
    }
    if tag == tas!(b"made") {
        let r = rest.as_cell().map_err(|_| "spec made")?;
        let inner = r.head().as_cell().map_err(|_| "spec made inner")?;
        let name = noun_to_term(inner.head())?;
        let args = noun_to_list(inner.tail(), noun_to_term)?;
        return Ok(Spec::Made((name, args), Box::new(noun_to_spec(r.tail())?)));
    }
    if tag == tas!(b"make") {
        let r = rest.as_cell().map_err(|_| "spec make")?;
        return Ok(Spec::Make(
            noun_to_hoon(r.head())?,
            noun_to_list(r.tail(), noun_to_spec)?,
        ));
    }
    if tag == tas!(b"name") {
        let r = rest.as_cell().map_err(|_| "spec name")?;
        return Ok(Spec::Name(
            noun_to_term(r.head())?,
            Box::new(noun_to_spec(r.tail())?),
        ));
    }
    if tag == tas!(b"over") {
        let r = rest.as_cell().map_err(|_| "spec over")?;
        return Ok(Spec::Over(
            noun_to_wing(r.head())?,
            Box::new(noun_to_spec(r.tail())?),
        ));
    }
    if tag == tas!(b"gist") {
        let r = rest.as_cell().map_err(|_| "spec gist")?;
        let note = r.head().as_cell().map_err(|_| "spec gist help")?;
        if noun_to_direct(note.head())? != tas!(b"help") {
            return Err("spec gist: expected help".to_string());
        }
        return Ok(Spec::Gist(
            noun_to_noun_expr(note.tail())?,
            Box::new(noun_to_spec(r.tail())?),
        ));
    }
    if tag == tas!(b"bcgr") {
        let r = rest.as_cell().map_err(|_| "spec bcgr")?;
        return Ok(Spec::BucGar(
            Box::new(noun_to_spec(r.head())?),
            Box::new(noun_to_spec(r.tail())?),
        ));
    }
    if tag == tas!(b"bcbc") {
        let r = rest.as_cell().map_err(|_| "spec bcbc")?;
        let map: HashMap<String, Spec> =
            noun_to_treap(r.tail(), |k, v| Ok((noun_to_term(k)?, noun_to_spec(v)?)))?
                .into_iter()
                .collect();
        return Ok(Spec::BucBuc(Box::new(noun_to_spec(r.head())?), map));
    }
    if tag == tas!(b"bcbr") {
        let r = rest.as_cell().map_err(|_| "spec bcbr")?;
        return Ok(Spec::BucBar(
            Box::new(noun_to_spec(r.head())?),
            noun_to_hoon(r.tail())?,
        ));
    }
    if tag == tas!(b"bccb") {
        return Ok(Spec::BucCab(noun_to_hoon(rest)?));
    }
    if tag == tas!(b"bccl") {
        let r = rest.as_cell().map_err(|_| "spec bccl")?;
        return Ok(Spec::BucCol(
            Box::new(noun_to_spec(r.head())?),
            noun_to_list(r.tail(), noun_to_spec)?,
        ));
    }
    if tag == tas!(b"bccn") {
        let r = rest.as_cell().map_err(|_| "spec bccn")?;
        return Ok(Spec::BucCen(
            Box::new(noun_to_spec(r.head())?),
            noun_to_list(r.tail(), noun_to_spec)?,
        ));
    }
    if tag == tas!(b"bcdt") {
        let r = rest.as_cell().map_err(|_| "spec bcdt")?;
        let map: HashMap<String, Spec> =
            noun_to_treap(r.tail(), |k, v| Ok((noun_to_term(k)?, noun_to_spec(v)?)))?
                .into_iter()
                .collect();
        return Ok(Spec::BucDot(Box::new(noun_to_spec(r.head())?), map));
    }
    if tag == tas!(b"bcgl") {
        let r = rest.as_cell().map_err(|_| "spec bcgl")?;
        return Ok(Spec::BucGal(
            Box::new(noun_to_spec(r.head())?),
            Box::new(noun_to_spec(r.tail())?),
        ));
    }
    if tag == tas!(b"bchp") {
        let r = rest.as_cell().map_err(|_| "spec bchp")?;
        return Ok(Spec::BucHep(
            Box::new(noun_to_spec(r.head())?),
            Box::new(noun_to_spec(r.tail())?),
        ));
    }
    if tag == tas!(b"bckt") {
        let r = rest.as_cell().map_err(|_| "spec bckt")?;
        return Ok(Spec::BucKet(
            Box::new(noun_to_spec(r.head())?),
            Box::new(noun_to_spec(r.tail())?),
        ));
    }
    if tag == tas!(b"bcls") {
        let r = rest.as_cell().map_err(|_| "spec bcls")?;
        return Ok(Spec::BucLus(
            noun_to_term(r.head())?,
            Box::new(noun_to_spec(r.tail())?),
        ));
    }
    if tag == tas!(b"bcfs") {
        let r = rest.as_cell().map_err(|_| "spec bcfs")?;
        let map: HashMap<String, Spec> =
            noun_to_treap(r.tail(), |k, v| Ok((noun_to_term(k)?, noun_to_spec(v)?)))?
                .into_iter()
                .collect();
        return Ok(Spec::BucFas(Box::new(noun_to_spec(r.head())?), map));
    }
    if tag == tas!(b"bcmc") {
        return Ok(Spec::BucMic(noun_to_hoon(rest)?));
    }
    if tag == tas!(b"bcpm") {
        let r = rest.as_cell().map_err(|_| "spec bcpm")?;
        return Ok(Spec::BucPam(
            Box::new(noun_to_spec(r.head())?),
            noun_to_hoon(r.tail())?,
        ));
    }
    if tag == tas!(b"bcsg") {
        let r = rest.as_cell().map_err(|_| "spec bcsg")?;
        return Ok(Spec::BucSig(
            noun_to_hoon(r.head())?,
            Box::new(noun_to_spec(r.tail())?),
        ));
    }
    if tag == tas!(b"bctc") {
        let r = rest.as_cell().map_err(|_| "spec bctc")?;
        let map: HashMap<String, Spec> =
            noun_to_treap(r.tail(), |k, v| Ok((noun_to_term(k)?, noun_to_spec(v)?)))?
                .into_iter()
                .collect();
        return Ok(Spec::BucTic(Box::new(noun_to_spec(r.head())?), map));
    }
    if tag == tas!(b"bcts") {
        let r = rest.as_cell().map_err(|_| "spec bcts")?;
        return Ok(Spec::BucTis(
            noun_to_skin(r.head())?,
            Box::new(noun_to_spec(r.tail())?),
        ));
    }
    if tag == tas!(b"bcpt") {
        let r = rest.as_cell().map_err(|_| "spec bcpt")?;
        return Ok(Spec::BucPat(
            Box::new(noun_to_spec(r.head())?),
            Box::new(noun_to_spec(r.tail())?),
        ));
    }
    if tag == tas!(b"bcwt") {
        let r = rest.as_cell().map_err(|_| "spec bcwt")?;
        return Ok(Spec::BucWut(
            Box::new(noun_to_spec(r.head())?),
            noun_to_list(r.tail(), noun_to_spec)?,
        ));
    }
    if tag == tas!(b"bczp") {
        let r = rest.as_cell().map_err(|_| "spec bczp")?;
        let map: HashMap<String, Spec> =
            noun_to_treap(r.tail(), |k, v| Ok((noun_to_term(k)?, noun_to_spec(v)?)))?
                .into_iter()
                .collect();
        return Ok(Spec::BucZap(Box::new(noun_to_spec(r.head())?), map));
    }
    Err(format!("spec: unknown tag {tag}"))
}

/// Walk a Hoon-style treap (mug-ordered binary tree) and collect key-value pairs.
fn noun_to_treap<'a, T, F>(noun: NounHandle<'a>, f: F) -> Result<Vec<T>, String>
where
    F: Fn(NounHandle<'a>, NounHandle<'a>) -> Result<T, String> + Copy,
{
    let mut out = Vec::new();
    treap_walk(noun, &f, &mut out)?;
    Ok(out)
}

fn treap_walk<'a, T, F>(noun: NounHandle<'a>, f: &F, out: &mut Vec<T>) -> Result<(), String>
where
    F: Fn(NounHandle<'a>, NounHandle<'a>) -> Result<T, String>,
{
    if let Ok(a) = noun.as_atom() {
        if let Ok(d) = a.as_direct() {
            if d.data() == 0 {
                return Ok(());
            }
        }
    }
    // [node left right] where node=[key value]
    let cell = noun.as_cell().map_err(|_| "treap: expected cell")?;
    let node = cell.head();
    let rest = cell
        .tail()
        .as_cell()
        .map_err(|_| "treap: expected [left right]")?;
    let left = rest.head();
    let right = rest.tail();
    // In-order traversal: left, node, right
    treap_walk(left, f, out)?;
    let kv = node.as_cell().map_err(|_| "treap node: expected cell")?;
    out.push(f(kv.head(), kv.tail())?);
    treap_walk(right, f, out)?;
    Ok(())
}

fn noun_to_chum(noun: NounHandle<'_>) -> Result<Chum, String> {
    if let Ok(_) = noun.as_atom() {
        return Ok(Chum::Lef(noun_to_term(noun)?));
    }
    let cell = noun.as_cell().map_err(|_| "chum")?;
    let a = noun_to_term(cell.head())?;
    let rest = cell.tail();
    if let Ok(_) = rest.as_atom() {
        return Ok(Chum::StdKel(a, noun_to_parsed_atom(rest)?));
    }
    let r2 = rest.as_cell().map_err(|_| "chum")?;
    let b = noun_to_term(r2.head())?;
    let rest2 = r2.tail();
    if let Ok(_) = rest2.as_atom() {
        return Ok(Chum::VenProKel(a, b, noun_to_parsed_atom(rest2)?));
    }
    let r3 = rest2.as_cell().map_err(|_| "chum")?;
    Ok(Chum::VenProVerKel(
        a,
        b,
        noun_to_parsed_atom(r3.head())?,
        noun_to_parsed_atom(r3.tail())?,
    ))
}

fn noun_to_term_or_pair(noun: NounHandle<'_>) -> Result<TermOrPair, String> {
    if let Ok(_) = noun.as_atom() {
        return Ok(TermOrPair::Term(noun_to_term(noun)?));
    }
    let cell = noun.as_cell().map_err(|_| "term_or_pair")?;
    Ok(TermOrPair::Pair(
        noun_to_term(cell.head())?,
        Box::new(noun_to_hoon(cell.tail())?),
    ))
}

fn noun_to_woof(noun: NounHandle<'_>) -> Result<Woof, String> {
    if let Ok(_) = noun.as_atom() {
        return Ok(Woof::ParsedAtom(noun_to_parsed_atom(noun)?));
    }
    // [0 hoon] → Woof::Hoon
    let cell = noun.as_cell().map_err(|_| "woof")?;
    Ok(Woof::Hoon(noun_to_hoon(cell.tail())?))
}

fn noun_to_tome(noun: NounHandle<'_>) -> Result<Tome, String> {
    // Tome is (What, HashMap<Term, Hoon>). The noun encoding from tome_to_noun
    // stores [what map] where map is a treap of (term, hoon) entries.
    let inner = noun.as_cell().map_err(|_| "tome: expected cell")?;
    let what = if let Ok(atom) = inner.head().as_atom() {
        if atom.as_direct().is_ok_and(|direct| direct.data() == 0) {
            None
        } else {
            Some(noun_to_noun_expr(inner.head())?)
        }
    } else {
        Some(noun_to_noun_expr(inner.head())?)
    };
    let map: HashMap<String, Hoon> = noun_to_treap(inner.tail(), |k, v| {
        Ok((noun_to_term(k)?, noun_to_hoon(v)?))
    })?
    .into_iter()
    .collect();
    Ok((what, map))
}

fn noun_to_alas(noun: NounHandle<'_>) -> Result<Alas, String> {
    noun_to_list(noun, |n| {
        let c = n.as_cell().map_err(|_| "alas entry")?;
        Ok((noun_to_term(c.head())?, noun_to_hoon(c.tail())?))
    })
}

fn noun_to_tyre(noun: NounHandle<'_>) -> Result<Vec<(String, Hoon)>, String> {
    noun_to_list(noun, |n| {
        let c = n.as_cell().map_err(|_| "tyre entry")?;
        Ok((noun_to_term(c.head())?, noun_to_hoon(c.tail())?))
    })
}

fn noun_to_zpwt_arg(noun: NounHandle<'_>) -> Result<ZpwtArg, String> {
    let cell = noun.as_cell().map_err(|_| "zpwt_arg")?;
    let tag = noun_to_direct(cell.head())?;
    if tag == tas!(b"atom") {
        return Ok(ZpwtArg::ParsedAtom(noun_to_cord(cell.tail())?));
    }
    if tag == tas!(b"pair") {
        let r = cell.tail().as_cell().map_err(|_| "zpwt pair")?;
        return Ok(ZpwtArg::Pair(
            noun_to_cord(r.head())?,
            noun_to_cord(r.tail())?,
        ));
    }
    Err(format!("zpwt_arg: unknown tag {tag}"))
}

fn noun_to_mane(noun: NounHandle<'_>) -> Result<Mane, String> {
    if let Ok(_) = noun.as_atom() {
        return Ok(Mane::Tag(noun_to_term(noun)?));
    }
    let cell = noun.as_cell().map_err(|_| "mane")?;
    Ok(Mane::TagSpace(
        noun_to_term(cell.head())?,
        noun_to_term(cell.tail())?,
    ))
}

fn noun_to_beer(noun: NounHandle<'_>) -> Result<Beer, String> {
    if let Ok(_) = noun.as_atom() {
        return Ok(Beer::Char(noun_to_cord(noun)?));
    }
    let cell = noun.as_cell().map_err(|_| "beer")?;
    // [0 hoon]
    Ok(Beer::Hoon(noun_to_hoon(cell.tail())?))
}

fn noun_to_mart(noun: NounHandle<'_>) -> Result<Mart, String> {
    noun_to_list(noun, |n| {
        let c = n.as_cell().map_err(|_| "mart entry")?;
        let mane = noun_to_mane(c.head())?;
        let beers = noun_to_list(c.tail(), noun_to_beer)?;
        Ok((mane, beers))
    })
}

fn noun_to_marx(noun: NounHandle<'_>) -> Result<Marx, String> {
    let cell = noun.as_cell().map_err(|_| "marx")?;
    Ok(Marx {
        n: noun_to_mane(cell.head())?,
        a: noun_to_mart(cell.tail())?,
    })
}

fn noun_to_manx(noun: NounHandle<'_>) -> Result<Manx, String> {
    let cell = noun.as_cell().map_err(|_| "manx")?;
    Ok(Manx {
        g: noun_to_marx(cell.head())?,
        c: noun_to_marl(cell.tail())?,
    })
}

fn noun_to_tuna_tail(noun: NounHandle<'_>) -> Result<TunaTail, String> {
    let cell = noun.as_cell().map_err(|_| "tuna_tail")?;
    let tag = noun_to_direct(cell.head())?;
    if tag == tas!(b"tape") {
        return Ok(TunaTail::Tape(noun_to_hoon(cell.tail())?));
    }
    if tag == tas!(b"manx") {
        return Ok(TunaTail::Manx(noun_to_hoon(cell.tail())?));
    }
    if tag == tas!(b"marl") {
        return Ok(TunaTail::Marl(noun_to_hoon(cell.tail())?));
    }
    if tag == tas!(b"call") {
        return Ok(TunaTail::Call(noun_to_hoon(cell.tail())?));
    }
    Err(format!("tuna_tail: unknown tag {tag}"))
}

fn noun_to_tuna(noun: NounHandle<'_>) -> Result<Tuna, String> {
    // Try as a tuna_tail first (tagged), else try as manx
    if let Ok(cell) = noun.as_cell() {
        if let Ok(tag) = noun_to_direct(cell.head()) {
            if tag == tas!(b"tape")
                || tag == tas!(b"manx")
                || tag == tas!(b"marl")
                || tag == tas!(b"call")
            {
                return Ok(Tuna::TunaTail(noun_to_tuna_tail(noun)?));
            }
        }
    }
    Ok(Tuna::Manx(noun_to_manx(noun)?))
}

fn noun_to_marl(noun: NounHandle<'_>) -> Result<Marl, String> {
    noun_to_list(noun, noun_to_tuna)
}

fn noun_to_nock(noun: NounHandle<'_>) -> Result<Nock, String> {
    if let Ok(atom) = noun.as_atom() {
        let val = if let Ok(d) = atom.as_direct() {
            d.data()
        } else {
            return Err("nock: large atom".into());
        };
        return Ok(Nock::AxisSelect(val));
    }
    let cell = noun.as_cell().map_err(|_| "nock")?;
    let head_val = noun_to_direct(cell.head())?;
    let rest = cell.tail();
    match head_val {
        1 => Ok(Nock::Const(noun_to_noun_expr(rest)?)),
        2 => {
            let r = rest.as_cell().map_err(|_| "nock 2")?;
            Ok(Nock::Pair(
                Box::new(noun_to_nock(r.head())?),
                Box::new(noun_to_nock(r.tail())?),
            ))
        }
        3 => Ok(Nock::CellTest(Box::new(noun_to_nock(rest)?))),
        4 => Ok(Nock::Increment(Box::new(noun_to_nock(rest)?))),
        5 => {
            let r = rest.as_cell().map_err(|_| "nock 5")?;
            Ok(Nock::Equality(
                Box::new(noun_to_nock(r.head())?),
                Box::new(noun_to_nock(r.tail())?),
            ))
        }
        6 => {
            let r = rest.as_cell().map_err(|_| "nock 6")?;
            let r2 = r.tail().as_cell().map_err(|_| "nock 6")?;
            Ok(Nock::IfThenElse(
                Box::new(noun_to_nock(r.head())?),
                Box::new(noun_to_nock(r2.head())?),
                Box::new(noun_to_nock(r2.tail())?),
            ))
        }
        7 => {
            let r = rest.as_cell().map_err(|_| "nock 7")?;
            Ok(Nock::Compose(
                Box::new(noun_to_nock(r.head())?),
                Box::new(noun_to_nock(r.tail())?),
            ))
        }
        8 => {
            let r = rest.as_cell().map_err(|_| "nock 8")?;
            Ok(Nock::SerialCompose(
                Box::new(noun_to_nock(r.head())?),
                Box::new(noun_to_nock(r.tail())?),
            ))
        }
        9 => {
            let r = rest.as_cell().map_err(|_| "nock 9")?;
            Ok(Nock::PushSubject(
                Box::new(noun_to_nock(r.head())?),
                Box::new(noun_to_nock(r.tail())?),
            ))
        }
        10 => {
            let r = rest.as_cell().map_err(|_| "nock 10")?;
            Ok(Nock::SelectArm(
                noun_to_direct(r.head())?,
                Box::new(noun_to_nock(r.tail())?),
            ))
        }
        11 => {
            let r = rest.as_cell().map_err(|_| "nock 11")?;
            let hint = r.head().as_cell().map_err(|_| "nock 11 hint")?;
            Ok(Nock::Edit(
                (
                    noun_to_direct(hint.head())?,
                    Box::new(noun_to_nock(hint.tail())?),
                ),
                Box::new(noun_to_nock(r.tail())?),
            ))
        }
        12 => {
            let r = rest.as_cell().map_err(|_| "nock 12")?;
            let hint = noun_to_nock_hint(r.head())?;
            Ok(Nock::Hint(hint, Box::new(noun_to_nock(r.tail())?)))
        }
        13 => {
            let r = rest.as_cell().map_err(|_| "nock 13")?;
            Ok(Nock::GrabData(
                Box::new(noun_to_nock(r.head())?),
                Box::new(noun_to_nock(r.tail())?),
            ))
        }
        _ => Err(format!("nock: unknown opcode {head_val}")),
    }
}

fn noun_to_nock_hint(noun: NounHandle<'_>) -> Result<NockHint, String> {
    if let Ok(atom) = noun.as_atom() {
        return Ok(NockHint::ParsedAtom(noun_to_direct(noun)?));
    }
    let cell = noun.as_cell().map_err(|_| "nock_hint")?;
    Ok(NockHint::Pair(
        noun_to_direct(cell.head())?,
        Box::new(noun_to_nock(cell.tail())?),
    ))
}

fn noun_to_type(noun: NounHandle<'_>) -> Result<Type, String> {
    if let Ok(tag) = noun_to_direct(noun) {
        if tag == tas!(b"noun") {
            return Ok(Type::NounExpr);
        }
        if tag == tas!(b"void") {
            return Ok(Type::Void);
        }
    }
    let cell = noun.as_cell().map_err(|_| "type: expected cell")?;
    let tag = noun_to_direct(cell.head())?;
    let rest = cell.tail();
    if tag == tas!(b"atom") {
        let r = rest.as_cell().map_err(|_| "type atom")?;
        let au = noun_to_term(r.head())?;
        let bits = noun_to_opt(r.tail(), noun_to_direct)?;
        return Ok(Type::ParsedAtom(au, bits));
    }
    if tag == tas!(b"cell") {
        let r = rest.as_cell().map_err(|_| "type cell")?;
        return Ok(Type::Cell(
            Box::new(noun_to_type(r.head())?),
            Box::new(noun_to_type(r.tail())?),
        ));
    }
    if tag == tas!(b"core") {
        let r = rest.as_cell().map_err(|_| "type core")?;
        // Core decoding is complex; return an opaque representation
        return Err("type: core decoding not supported in noun_to_type".into());
    }
    if tag == tas!(b"face") {
        let r = rest.as_cell().map_err(|_| "type face")?;
        return Err("type: face decoding not fully supported".into());
    }
    if tag == tas!(b"fork") {
        let types = noun_to_fork_set_options(rest).or_else(|_| noun_to_list(rest, noun_to_type))?;
        return Ok(Type::Fork(types));
    }
    if tag == tas!(b"hint") {
        let r = rest.as_cell().map_err(|_| "type hint")?;
        let inner_cell = r.head().as_cell().map_err(|_| "type hint inner")?;
        let inner = noun_to_type(inner_cell.head())?;
        let note = noun_to_note(inner_cell.tail())?;
        let payload = noun_to_type(r.tail())?;
        return Ok(Type::Hint((Box::new(inner), note), Box::new(payload)));
    }
    if tag == tas!(b"hold") {
        let r = rest.as_cell().map_err(|_| "type hold")?;
        let typ = noun_to_type(r.head())?;
        let hoon = noun_to_hoon(r.tail())?;
        return Ok(Type::Hold(Box::new(typ), hoon));
    }
    Err(format!("type: unknown tag {tag}"))
}

/// Convert a gene noun (as produced by hoonc or `hoon_to_noun`) back to a `Hoon` AST.
pub fn noun_to_hoon(noun: NounHandle<'_>) -> Result<Hoon, String> {
    let cell = noun
        .as_cell()
        .map_err(|_| format!("noun_to_hoon: expected cell, got atom"))?;
    let head = cell.head();
    let tail = cell.tail();

    // If head is a cell, this is a Pair (no tag).
    if head.as_cell().is_ok() {
        let p = noun_to_hoon(head)?;
        let q = noun_to_hoon(tail)?;
        return Ok(Hoon::Pair(Box::new(p), Box::new(q)));
    }

    let tag = noun_to_direct(head)?;

    // Axis: [0 n]
    if tag == 0 {
        let n = noun_to_direct(tail)?;
        return Ok(Hoon::Axis(n));
    }

    if tag == tas!(b"zpzp") {
        return Ok(Hoon::ZapZap);
    }

    if tag == tas!(b"base") {
        return Ok(Hoon::Base(noun_to_basetype(tail)?));
    }
    if tag == tas!(b"bust") {
        return Ok(Hoon::Bust(noun_to_basetype(tail)?));
    }

    if tag == tas!(b"dbug") {
        let r = tail.as_cell().map_err(|_| "dbug")?;
        return Ok(Hoon::Dbug(
            noun_to_spot(r.head())?,
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"eror") {
        return Ok(Hoon::Eror(noun_to_cord(tail)?));
    }
    if tag == tas!(b"hand") {
        let r = tail.as_cell().map_err(|_| "hand")?;
        return Ok(Hoon::Hand(
            Box::new(noun_to_type(r.head())?),
            noun_to_nock(r.tail())?,
        ));
    }
    if tag == tas!(b"note") {
        let r = tail.as_cell().map_err(|_| "note")?;
        return Ok(Hoon::Note(
            noun_to_note(r.head())?,
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"fits") {
        let r = tail.as_cell().map_err(|_| "fits")?;
        return Ok(Hoon::Fits(
            Box::new(noun_to_hoon(r.head())?),
            noun_to_wing(r.tail())?,
        ));
    }
    if tag == tas!(b"knit") {
        return Ok(Hoon::Knit(noun_to_list(tail, noun_to_woof)?));
    }
    if tag == tas!(b"leaf") {
        let r = tail.as_cell().map_err(|_| "leaf")?;
        return Ok(Hoon::Leaf(
            noun_to_term(r.head())?,
            noun_to_parsed_atom(r.tail())?,
        ));
    }
    if tag == tas!(b"limb") {
        return Ok(Hoon::Limb(noun_to_term(tail)?));
    }
    if tag == tas!(b"lost") {
        return Ok(Hoon::Lost(Box::new(noun_to_hoon(tail)?)));
    }
    if tag == tas!(b"rock") {
        let r = tail.as_cell().map_err(|_| "rock")?;
        return Ok(Hoon::Rock(
            noun_to_term(r.head())?,
            noun_to_noun_expr(r.tail())?,
        ));
    }
    if tag == tas!(b"sand") {
        let r = tail.as_cell().map_err(|_| "sand")?;
        return Ok(Hoon::Sand(
            noun_to_term(r.head())?,
            noun_to_noun_expr(r.tail())?,
        ));
    }
    if tag == tas!(b"tell") {
        return Ok(Hoon::Tell(noun_to_list(tail, noun_to_hoon)?));
    }
    if tag == tas!(b"tune") {
        return Ok(Hoon::Tune(noun_to_term_or_tune(tail)?));
    }
    if tag == tas!(b"wing") {
        return Ok(Hoon::Wing(noun_to_wing(tail)?));
    }
    if tag == tas!(b"yell") {
        return Ok(Hoon::Yell(noun_to_list(tail, noun_to_hoon)?));
    }
    if tag == tas!(b"xray") {
        return Ok(Hoon::Xray(noun_to_manx(tail)?));
    }

    // Bar runes
    if tag == tas!(b"brbc") {
        let r = tail.as_cell().map_err(|_| "brbc")?;
        return Ok(Hoon::BarBuc(
            noun_to_list(r.head(), noun_to_term)?,
            Box::new(noun_to_spec(r.tail())?),
        ));
    }
    if tag == tas!(b"brcb") {
        let r = tail.as_cell().map_err(|_| "brcb")?;
        let r2 = r.tail().as_cell().map_err(|_| "brcb")?;
        let tomes: HashMap<String, Tome> =
            noun_to_treap(r2.tail(), |k, v| Ok((noun_to_term(k)?, noun_to_tome(v)?)))?
                .into_iter()
                .collect();
        return Ok(Hoon::BarCab(
            Box::new(noun_to_spec(r.head())?),
            noun_to_alas(r2.head())?,
            tomes,
        ));
    }
    if tag == tas!(b"brcl") {
        let r = tail.as_cell().map_err(|_| "brcl")?;
        return Ok(Hoon::BarCol(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"brcn") {
        let r = tail.as_cell().map_err(|_| "brcn")?;
        let prefix = noun_to_opt(r.head(), noun_to_term)?;
        let tomes: HashMap<String, Tome> =
            noun_to_treap(r.tail(), |k, v| Ok((noun_to_term(k)?, noun_to_tome(v)?)))?
                .into_iter()
                .collect();
        return Ok(Hoon::BarCen(prefix, tomes));
    }
    if tag == tas!(b"brdt") {
        return Ok(Hoon::BarDot(Box::new(noun_to_hoon(tail)?)));
    }
    if tag == tas!(b"brkt") {
        let r = tail.as_cell().map_err(|_| "brkt")?;
        let tomes: HashMap<String, Tome> =
            noun_to_treap(r.tail(), |k, v| Ok((noun_to_term(k)?, noun_to_tome(v)?)))?
                .into_iter()
                .collect();
        return Ok(Hoon::BarKet(Box::new(noun_to_hoon(r.head())?), tomes));
    }
    if tag == tas!(b"brhp") {
        return Ok(Hoon::BarHep(Box::new(noun_to_hoon(tail)?)));
    }
    if tag == tas!(b"brsg") {
        let r = tail.as_cell().map_err(|_| "brsg")?;
        return Ok(Hoon::BarSig(
            Box::new(noun_to_spec(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"brtr") {
        let r = tail.as_cell().map_err(|_| "brtr")?;
        return Ok(Hoon::BarTar(
            Box::new(noun_to_spec(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"brts") {
        let r = tail.as_cell().map_err(|_| "brts")?;
        return Ok(Hoon::BarTis(
            Box::new(noun_to_spec(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"brpt") {
        let r = tail.as_cell().map_err(|_| "brpt")?;
        let prefix = noun_to_opt(r.head(), noun_to_term)?;
        let tomes: HashMap<String, Tome> =
            noun_to_treap(r.tail(), |k, v| Ok((noun_to_term(k)?, noun_to_tome(v)?)))?
                .into_iter()
                .collect();
        return Ok(Hoon::BarPat(prefix, tomes));
    }
    if tag == tas!(b"brwt") {
        return Ok(Hoon::BarWut(Box::new(noun_to_hoon(tail)?)));
    }

    // Col runes
    if tag == tas!(b"clcb") {
        let r = tail.as_cell().map_err(|_| "clcb")?;
        return Ok(Hoon::ColCab(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"clkt") {
        let r = tail.as_cell().map_err(|_| "clkt")?;
        let r2 = r.tail().as_cell().map_err(|_| "clkt")?;
        let r3 = r2.tail().as_cell().map_err(|_| "clkt")?;
        return Ok(Hoon::ColKet(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r2.head())?),
            Box::new(noun_to_hoon(r3.head())?),
            Box::new(noun_to_hoon(r3.tail())?),
        ));
    }
    if tag == tas!(b"clhp") {
        let r = tail.as_cell().map_err(|_| "clhp")?;
        return Ok(Hoon::ColHep(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"clls") {
        let r = tail.as_cell().map_err(|_| "clls")?;
        let r2 = r.tail().as_cell().map_err(|_| "clls")?;
        return Ok(Hoon::ColLus(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r2.head())?),
            Box::new(noun_to_hoon(r2.tail())?),
        ));
    }
    if tag == tas!(b"clsg") {
        return Ok(Hoon::ColSig(noun_to_list(tail, noun_to_hoon)?));
    }
    if tag == tas!(b"cltr") {
        return Ok(Hoon::ColTar(noun_to_list(tail, noun_to_hoon)?));
    }

    // Cen runes
    if tag == tas!(b"cncb") {
        let r = tail.as_cell().map_err(|_| "cncb")?;
        let pairs = noun_to_list(r.tail(), |n| {
            let c = n.as_cell().map_err(|_| "cncb pair")?;
            Ok((noun_to_wing(c.head())?, noun_to_hoon(c.tail())?))
        })?;
        return Ok(Hoon::CenCab(noun_to_wing(r.head())?, pairs));
    }
    if tag == tas!(b"cndt") {
        let r = tail.as_cell().map_err(|_| "cndt")?;
        return Ok(Hoon::CenDot(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"cnhp") {
        let r = tail.as_cell().map_err(|_| "cnhp")?;
        return Ok(Hoon::CenHep(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"cncl") {
        let r = tail.as_cell().map_err(|_| "cncl")?;
        return Ok(Hoon::CenCol(
            Box::new(noun_to_hoon(r.head())?),
            noun_to_list(r.tail(), noun_to_hoon)?,
        ));
    }
    if tag == tas!(b"cntr") {
        let r = tail.as_cell().map_err(|_| "cntr")?;
        let r2 = r.tail().as_cell().map_err(|_| "cntr")?;
        let pairs = noun_to_list(r2.tail(), |n| {
            let c = n.as_cell().map_err(|_| "cntr pair")?;
            Ok((noun_to_wing(c.head())?, noun_to_hoon(c.tail())?))
        })?;
        return Ok(Hoon::CenTar(
            noun_to_wing(r.head())?,
            Box::new(noun_to_hoon(r2.head())?),
            pairs,
        ));
    }
    if tag == tas!(b"cnkt") {
        let r = tail.as_cell().map_err(|_| "cnkt")?;
        let r2 = r.tail().as_cell().map_err(|_| "cnkt")?;
        let r3 = r2.tail().as_cell().map_err(|_| "cnkt")?;
        return Ok(Hoon::CenKet(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r2.head())?),
            Box::new(noun_to_hoon(r3.head())?),
            Box::new(noun_to_hoon(r3.tail())?),
        ));
    }
    if tag == tas!(b"cnls") {
        let r = tail.as_cell().map_err(|_| "cnls")?;
        let r2 = r.tail().as_cell().map_err(|_| "cnls")?;
        return Ok(Hoon::CenLus(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r2.head())?),
            Box::new(noun_to_hoon(r2.tail())?),
        ));
    }
    if tag == tas!(b"cnsg") {
        let r = tail.as_cell().map_err(|_| "cnsg")?;
        let r2 = r.tail().as_cell().map_err(|_| "cnsg")?;
        return Ok(Hoon::CenSig(
            noun_to_wing(r.head())?,
            Box::new(noun_to_hoon(r2.head())?),
            noun_to_list(r2.tail(), noun_to_hoon)?,
        ));
    }
    if tag == tas!(b"cnts") {
        let r = tail.as_cell().map_err(|_| "cnts")?;
        let pairs = noun_to_list(r.tail(), |n| {
            let c = n.as_cell().map_err(|_| "cnts pair")?;
            Ok((noun_to_wing(c.head())?, noun_to_hoon(c.tail())?))
        })?;
        return Ok(Hoon::CenTis(noun_to_wing(r.head())?, pairs));
    }

    // Dot runes
    if tag == tas!(b"dtkt") {
        let r = tail.as_cell().map_err(|_| "dtkt")?;
        return Ok(Hoon::DotKet(
            Box::new(noun_to_spec(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"dtls") {
        return Ok(Hoon::DotLus(Box::new(noun_to_hoon(tail)?)));
    }
    if tag == tas!(b"dttr") {
        let r = tail.as_cell().map_err(|_| "dttr")?;
        return Ok(Hoon::DotTar(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"dtts") {
        let r = tail.as_cell().map_err(|_| "dtts")?;
        return Ok(Hoon::DotTis(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"dtwt") {
        return Ok(Hoon::DotWut(Box::new(noun_to_hoon(tail)?)));
    }

    // Ket runes
    if tag == tas!(b"ktbr") {
        return Ok(Hoon::KetBar(Box::new(noun_to_hoon(tail)?)));
    }
    if tag == tas!(b"ktdt") {
        let r = tail.as_cell().map_err(|_| "ktdt")?;
        return Ok(Hoon::KetDot(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"ktls") {
        let r = tail.as_cell().map_err(|_| "ktls")?;
        return Ok(Hoon::KetLus(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"kthp") {
        let r = tail.as_cell().map_err(|_| "kthp")?;
        return Ok(Hoon::KetHep(
            Box::new(noun_to_spec(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"ktpm") {
        return Ok(Hoon::KetPam(Box::new(noun_to_hoon(tail)?)));
    }
    if tag == tas!(b"ktsg") {
        return Ok(Hoon::KetSig(Box::new(noun_to_hoon(tail)?)));
    }
    if tag == tas!(b"ktts") {
        let r = tail.as_cell().map_err(|_| "ktts")?;
        return Ok(Hoon::KetTis(
            noun_to_skin(r.head())?,
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"ktwt") {
        return Ok(Hoon::KetWut(Box::new(noun_to_hoon(tail)?)));
    }
    if tag == tas!(b"kttr") {
        return Ok(Hoon::KetTar(Box::new(noun_to_spec(tail)?)));
    }
    if tag == tas!(b"ktcl") {
        return Ok(Hoon::KetCol(Box::new(noun_to_spec(tail)?)));
    }

    // Sig runes
    if tag == tas!(b"sgbr") {
        let r = tail.as_cell().map_err(|_| "sgbr")?;
        return Ok(Hoon::SigBar(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"sgcb") {
        let r = tail.as_cell().map_err(|_| "sgcb")?;
        return Ok(Hoon::SigCab(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"sgcn") {
        let r = tail.as_cell().map_err(|_| "sgcn")?;
        let r2 = r.tail().as_cell().map_err(|_| "sgcn")?;
        let r3 = r2.tail().as_cell().map_err(|_| "sgcn")?;
        return Ok(Hoon::SigCen(
            noun_to_chum(r.head())?,
            Box::new(noun_to_hoon(r2.head())?),
            noun_to_tyre(r3.head())?,
            Box::new(noun_to_hoon(r3.tail())?),
        ));
    }
    if tag == tas!(b"sgfs") {
        let r = tail.as_cell().map_err(|_| "sgfs")?;
        return Ok(Hoon::SigFas(
            noun_to_chum(r.head())?,
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"sggl") {
        let r = tail.as_cell().map_err(|_| "sggl")?;
        return Ok(Hoon::SigGal(
            noun_to_term_or_pair(r.head())?,
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"sggr") {
        let r = tail.as_cell().map_err(|_| "sggr")?;
        return Ok(Hoon::SigGar(
            noun_to_term_or_pair(r.head())?,
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"sgbc") {
        let r = tail.as_cell().map_err(|_| "sgbc")?;
        return Ok(Hoon::SigBuc(
            noun_to_term(r.head())?,
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"sgls") {
        let r = tail.as_cell().map_err(|_| "sgls")?;
        return Ok(Hoon::SigLus(
            noun_to_direct(r.head())?,
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"sgpm") {
        let r = tail.as_cell().map_err(|_| "sgpm")?;
        let r2 = r.tail().as_cell().map_err(|_| "sgpm")?;
        return Ok(Hoon::SigPam(
            noun_to_direct(r.head())?,
            Box::new(noun_to_hoon(r2.head())?),
            Box::new(noun_to_hoon(r2.tail())?),
        ));
    }
    if tag == tas!(b"sgts") {
        let r = tail.as_cell().map_err(|_| "sgts")?;
        return Ok(Hoon::SigTis(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"sgwt") {
        let r = tail.as_cell().map_err(|_| "sgwt")?;
        let r2 = r.tail().as_cell().map_err(|_| "sgwt")?;
        let r3 = r2.tail().as_cell().map_err(|_| "sgwt")?;
        return Ok(Hoon::SigWut(
            noun_to_direct(r.head())?,
            Box::new(noun_to_hoon(r2.head())?),
            Box::new(noun_to_hoon(r3.head())?),
            Box::new(noun_to_hoon(r3.tail())?),
        ));
    }
    if tag == tas!(b"sgzp") {
        let r = tail.as_cell().map_err(|_| "sgzp")?;
        return Ok(Hoon::SigZap(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }

    // Mic runes
    if tag == tas!(b"mcts") {
        return Ok(Hoon::MicTis(noun_to_marl(tail)?));
    }
    if tag == tas!(b"mccl") {
        let r = tail.as_cell().map_err(|_| "mccl")?;
        return Ok(Hoon::MicCol(
            Box::new(noun_to_hoon(r.head())?),
            noun_to_list(r.tail(), noun_to_hoon)?,
        ));
    }
    if tag == tas!(b"mcfs") {
        return Ok(Hoon::MicFas(Box::new(noun_to_hoon(tail)?)));
    }
    if tag == tas!(b"mcgl") {
        let r = tail.as_cell().map_err(|_| "mcgl")?;
        let r2 = r.tail().as_cell().map_err(|_| "mcgl")?;
        let r3 = r2.tail().as_cell().map_err(|_| "mcgl")?;
        return Ok(Hoon::MicGal(
            Box::new(noun_to_spec(r.head())?),
            Box::new(noun_to_hoon(r2.head())?),
            Box::new(noun_to_hoon(r3.head())?),
            Box::new(noun_to_hoon(r3.tail())?),
        ));
    }
    if tag == tas!(b"mcsg") {
        let r = tail.as_cell().map_err(|_| "mcsg")?;
        return Ok(Hoon::MicSig(
            Box::new(noun_to_hoon(r.head())?),
            noun_to_list(r.tail(), noun_to_hoon)?,
        ));
    }
    if tag == tas!(b"mcmc") {
        let r = tail.as_cell().map_err(|_| "mcmc")?;
        return Ok(Hoon::MicMic(
            Box::new(noun_to_spec(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }

    // Tis runes
    if tag == tas!(b"tsbr") {
        let r = tail.as_cell().map_err(|_| "tsbr")?;
        return Ok(Hoon::TisBar(
            Box::new(noun_to_spec(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"tscl") {
        let r = tail.as_cell().map_err(|_| "tscl")?;
        let pairs = noun_to_list(r.head(), |n| {
            let c = n.as_cell().map_err(|_| "tscl pair")?;
            Ok((noun_to_wing(c.head())?, noun_to_hoon(c.tail())?))
        })?;
        return Ok(Hoon::TisCol(pairs, Box::new(noun_to_hoon(r.tail())?)));
    }
    if tag == tas!(b"tsfs") {
        let r = tail.as_cell().map_err(|_| "tsfs")?;
        let r2 = r.tail().as_cell().map_err(|_| "tsfs")?;
        return Ok(Hoon::TisFas(
            noun_to_skin(r.head())?,
            Box::new(noun_to_hoon(r2.head())?),
            Box::new(noun_to_hoon(r2.tail())?),
        ));
    }
    if tag == tas!(b"tsmc") {
        let r = tail.as_cell().map_err(|_| "tsmc")?;
        let r2 = r.tail().as_cell().map_err(|_| "tsmc")?;
        return Ok(Hoon::TisMic(
            noun_to_skin(r.head())?,
            Box::new(noun_to_hoon(r2.head())?),
            Box::new(noun_to_hoon(r2.tail())?),
        ));
    }
    if tag == tas!(b"tsdt") {
        let r = tail.as_cell().map_err(|_| "tsdt")?;
        let r2 = r.tail().as_cell().map_err(|_| "tsdt")?;
        return Ok(Hoon::TisDot(
            noun_to_wing(r.head())?,
            Box::new(noun_to_hoon(r2.head())?),
            Box::new(noun_to_hoon(r2.tail())?),
        ));
    }
    if tag == tas!(b"tswt") {
        let r = tail.as_cell().map_err(|_| "tswt")?;
        let r2 = r.tail().as_cell().map_err(|_| "tswt")?;
        let r3 = r2.tail().as_cell().map_err(|_| "tswt")?;
        return Ok(Hoon::TisWut(
            noun_to_wing(r.head())?,
            Box::new(noun_to_hoon(r2.head())?),
            Box::new(noun_to_hoon(r3.head())?),
            Box::new(noun_to_hoon(r3.tail())?),
        ));
    }
    if tag == tas!(b"tsgl") {
        let r = tail.as_cell().map_err(|_| "tsgl")?;
        return Ok(Hoon::TisGal(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"tshp") {
        let r = tail.as_cell().map_err(|_| "tshp")?;
        return Ok(Hoon::TisHep(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"tsgr") {
        let r = tail.as_cell().map_err(|_| "tsgr")?;
        return Ok(Hoon::TisGar(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"tskt") {
        let r = tail.as_cell().map_err(|_| "tskt")?;
        let r2 = r.tail().as_cell().map_err(|_| "tskt")?;
        let r3 = r2.tail().as_cell().map_err(|_| "tskt")?;
        return Ok(Hoon::TisKet(
            noun_to_skin(r.head())?,
            noun_to_wing(r2.head())?,
            Box::new(noun_to_hoon(r3.head())?),
            Box::new(noun_to_hoon(r3.tail())?),
        ));
    }
    if tag == tas!(b"tsls") {
        let r = tail.as_cell().map_err(|_| "tsls")?;
        return Ok(Hoon::TisLus(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"tssg") || tag == tas!(b"tsig") {
        return Ok(Hoon::TisSig(noun_to_list(tail, noun_to_hoon)?));
    }
    if tag == tas!(b"tstr") {
        let r = tail.as_cell().map_err(|_| "tstr")?;
        let name_spec = r.head().as_cell().map_err(|_| "tstr name_spec")?;
        let name = noun_to_term(name_spec.head())?;
        let spec_opt = noun_to_opt(name_spec.tail(), noun_to_spec)?;
        let r2 = r.tail().as_cell().map_err(|_| "tstr")?;
        return Ok(Hoon::TisTar(
            (name, spec_opt.map(Box::new)),
            Box::new(noun_to_hoon(r2.head())?),
            Box::new(noun_to_hoon(r2.tail())?),
        ));
    }
    if tag == tas!(b"tscm") {
        let r = tail.as_cell().map_err(|_| "tscm")?;
        return Ok(Hoon::TisCom(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }

    // Wut runes
    if tag == tas!(b"wtbr") {
        return Ok(Hoon::WutBar(noun_to_list(tail, noun_to_hoon)?));
    }
    if tag == tas!(b"wthp") {
        let r = tail.as_cell().map_err(|_| "wthp")?;
        let pairs = noun_to_list(r.tail(), |n| {
            let c = n.as_cell().map_err(|_| "wthp pair")?;
            Ok((noun_to_spec(c.head())?, noun_to_hoon(c.tail())?))
        })?;
        return Ok(Hoon::WutHep(noun_to_wing(r.head())?, pairs));
    }
    if tag == tas!(b"wtcl") {
        let r = tail.as_cell().map_err(|_| "wtcl")?;
        let r2 = r.tail().as_cell().map_err(|_| "wtcl")?;
        return Ok(Hoon::WutCol(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r2.head())?),
            Box::new(noun_to_hoon(r2.tail())?),
        ));
    }
    if tag == tas!(b"wtdt") {
        let r = tail.as_cell().map_err(|_| "wtdt")?;
        let r2 = r.tail().as_cell().map_err(|_| "wtdt")?;
        return Ok(Hoon::WutDot(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r2.head())?),
            Box::new(noun_to_hoon(r2.tail())?),
        ));
    }
    if tag == tas!(b"wtkt") {
        let r = tail.as_cell().map_err(|_| "wtkt")?;
        let r2 = r.tail().as_cell().map_err(|_| "wtkt")?;
        return Ok(Hoon::WutKet(
            noun_to_wing(r.head())?,
            Box::new(noun_to_hoon(r2.head())?),
            Box::new(noun_to_hoon(r2.tail())?),
        ));
    }
    if tag == tas!(b"wtgl") {
        let r = tail.as_cell().map_err(|_| "wtgl")?;
        return Ok(Hoon::WutGal(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"wtgr") {
        let r = tail.as_cell().map_err(|_| "wtgr")?;
        return Ok(Hoon::WutGar(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"wtls") {
        let r = tail.as_cell().map_err(|_| "wtls")?;
        let r2 = r.tail().as_cell().map_err(|_| "wtls")?;
        let pairs = noun_to_list(r2.tail(), |n| {
            let c = n.as_cell().map_err(|_| "wtls pair")?;
            Ok((noun_to_spec(c.head())?, noun_to_hoon(c.tail())?))
        })?;
        return Ok(Hoon::WutLus(
            noun_to_wing(r.head())?,
            Box::new(noun_to_hoon(r2.head())?),
            pairs,
        ));
    }
    if tag == tas!(b"wtpm") {
        return Ok(Hoon::WutPam(noun_to_list(tail, noun_to_hoon)?));
    }
    if tag == tas!(b"wtpt") {
        let r = tail.as_cell().map_err(|_| "wtpt")?;
        let r2 = r.tail().as_cell().map_err(|_| "wtpt")?;
        return Ok(Hoon::WutPat(
            noun_to_wing(r.head())?,
            Box::new(noun_to_hoon(r2.head())?),
            Box::new(noun_to_hoon(r2.tail())?),
        ));
    }
    if tag == tas!(b"wtsg") {
        let r = tail.as_cell().map_err(|_| "wtsg")?;
        let r2 = r.tail().as_cell().map_err(|_| "wtsg")?;
        return Ok(Hoon::WutSig(
            noun_to_wing(r.head())?,
            Box::new(noun_to_hoon(r2.head())?),
            Box::new(noun_to_hoon(r2.tail())?),
        ));
    }
    if tag == tas!(b"wthx") {
        let r = tail.as_cell().map_err(|_| "wthx")?;
        return Ok(Hoon::WutHax(
            noun_to_skin(r.head())?,
            noun_to_wing(r.tail())?,
        ));
    }
    if tag == tas!(b"wtts") {
        let r = tail.as_cell().map_err(|_| "wtts")?;
        return Ok(Hoon::WutTis(
            Box::new(noun_to_spec(r.head())?),
            noun_to_wing(r.tail())?,
        ));
    }
    if tag == tas!(b"wtzp") {
        return Ok(Hoon::WutZap(Box::new(noun_to_hoon(tail)?)));
    }

    // Zap runes
    if tag == tas!(b"zpcm") {
        let r = tail.as_cell().map_err(|_| "zpcm")?;
        return Ok(Hoon::ZapCom(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"zpgr") {
        return Ok(Hoon::ZapGar(Box::new(noun_to_hoon(tail)?)));
    }
    if tag == tas!(b"zpgl") {
        let r = tail.as_cell().map_err(|_| "zpgl")?;
        return Ok(Hoon::ZapGal(
            Box::new(noun_to_spec(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"zpmc") {
        let r = tail.as_cell().map_err(|_| "zpmc")?;
        return Ok(Hoon::ZapMic(
            Box::new(noun_to_hoon(r.head())?),
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }
    if tag == tas!(b"zpts") {
        return Ok(Hoon::ZapTis(Box::new(noun_to_hoon(tail)?)));
    }
    if tag == tas!(b"zppt") {
        let r = tail.as_cell().map_err(|_| "zppt")?;
        let r2 = r.tail().as_cell().map_err(|_| "zppt")?;
        return Ok(Hoon::ZapPat(
            noun_to_list(r.head(), noun_to_wing)?,
            Box::new(noun_to_hoon(r2.head())?),
            Box::new(noun_to_hoon(r2.tail())?),
        ));
    }
    if tag == tas!(b"zpwt") {
        let r = tail.as_cell().map_err(|_| "zpwt")?;
        return Ok(Hoon::ZapWut(
            noun_to_zpwt_arg(r.head())?,
            Box::new(noun_to_hoon(r.tail())?),
        ));
    }

    // Convert tag to readable form for error message
    let tag_bytes = tag.to_le_bytes();
    let tag_len = tag_bytes
        .iter()
        .rposition(|&b| b != 0)
        .map(|i| i + 1)
        .unwrap_or(0);
    let tag_str = std::str::from_utf8(&tag_bytes[..tag_len]).unwrap_or("???");
    Err(format!("noun_to_hoon: unknown tag {tag} ({tag_str})"))
}

fn noun_to_term_or_tune(noun: NounHandle<'_>) -> Result<TermOrTune, String> {
    if let Ok(_) = noun.as_atom() {
        return Ok(TermOrTune::Term(noun_to_term(noun)?));
    }
    // It's a tune: (map, vec)
    let cell = noun.as_cell().map_err(|_| "term_or_tune")?;
    let map = noun_to_treap(cell.head(), |k, v| {
        let term = noun_to_term(k)?;
        let opt = noun_to_opt(v, noun_to_hoon)?;
        Ok((term, opt))
    })?;
    let vec = noun_to_list(cell.tail(), noun_to_hoon)?;
    Ok(TermOrTune::Tune((map.into_iter().collect(), vec)))
}

pub fn collect_inputs(path: &PathBuf) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_inputs_inner(path, &mut files);
    files.sort();
    files
}

fn collect_inputs_inner(path: &PathBuf, out: &mut Vec<PathBuf>) {
    if path.is_file() {
        if path.extension().and_then(|e| e.to_str()) == Some("hoon") {
            out.push(path.to_path_buf());
        }
    } else if path.is_dir() {
        let entries = std::fs::read_dir(path).unwrap_or_else(|e| {
            eprintln!("Failed to read directory '{}': {}", path.display(), e);
            std::process::exit(1);
        });

        for entry in entries {
            let entry = entry.unwrap_or_else(|e| {
                eprintln!(
                    "Failed to read directory entry in '{}': {}",
                    path.display(),
                    e
                );
                std::process::exit(1);
            });

            collect_inputs_inner(&entry.path(), out);
        }
    } else {
        eprintln!("Invalid input path: {}", path.display());
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chumsky::Parser;
    use nockapp::noun::slab::NounSlab;
    use nockchain_math::noun_ext::NounMathExt;
    use nockchain_math::zoon::common::{gor_tip, DefaultTipHasher};
    use nockvm::ext::noun_equality;
    use nockvm::noun::{Noun, NounAllocator, D, T};

    use super::{
        chumsky_spot_to_hoon_spot, flay, gor_mug, limb_to_noun, map_to_noun, mor_mug, open,
        rent_co, slab_mug, string_to_atom, term_to_noun, Limb, LineMap,
    };
    use crate::ast::hoon::{BaseType, Coin, Hoon, ParsedAtom, Skin, Spec};

    fn noun_is_zero(noun: Noun) -> bool {
        unsafe { noun.raw_equals(&D(0)) }
    }
    fn slab_noun_equality<J>(slab: &NounSlab<J>, left: &Noun, right: &Noun) -> bool {
        let space = slab.noun_space();
        noun_equality(left.in_space(&space), right.in_space(&space))
    }

    #[test]
    fn rent_co_renders_unsigned_decimal_zero_as_zero_text() {
        let rendered = rent_co(&Coin::Dime("ud".to_string(), ParsedAtom::Small(0)));

        assert_eq!(
            rendered,
            string_to_atom("0".to_string()),
            "path segment /0 should re-render as @ta '0', not the empty atom"
        );
    }

    #[test]
    fn postfix_docs_on_named_specs_wrap_whole_spec() {
        let src = "  =|  a=(tree (pair))  ::  (map)\n  *\n";
        let start = src.find("a=").expect("missing named spec");
        let end = src.find("  ::").expect("missing postfix doc");
        let linemap = LineMap::new_with_docs(src, true);
        let spec = Spec::BucTis(
            Skin::Term("a".to_string()),
            Box::new(Spec::Base(BaseType::NounExpr)),
        );

        let spec = super::apply_spec_postfix_docs(spec, (start, end), &linemap);
        assert!(
            matches!(spec, Spec::Gist(_, _)),
            "postfix doc should decorate the whole named spec"
        );
    }

    #[test]
    fn parser_attaches_postfix_docs_to_tisbar_named_sample_spec() {
        let src = "  =|  a=(tree (pair))  ::  (map)\n  *\n";
        let linemap = Arc::new(LineMap::new_with_docs(src, true));
        let parsed = crate::native_parser(
            vec!["test".into(), "map-sample.hoon".into()],
            false,
            linemap,
        )
        .parse(src)
        .into_result()
        .expect("sample should parse");

        let Hoon::TisSig(items) = parsed else {
            panic!("expected top-level TisSig");
        };
        let [Hoon::TisBar(spec, _)] = items.as_slice() else {
            panic!("expected one TisBar expression");
        };
        assert!(
            matches!(spec.as_ref(), Spec::Gist(_, _)),
            "postfix doc should decorate the named sample spec"
        );
    }

    #[test]
    fn parser_spots_arm_body_at_body_rune_after_multiparagraph_arm_docs() {
        let src = concat!(
            "|%\n", "++  gen\n", "  ::  arm-level docs\n", "  ::\n", "  ::  second paragraph\n",
            "  ^-  @\n", "  0\n", "--\n",
        );
        let linemap = Arc::new(LineMap::new_with_docs(src, true));
        let parsed = crate::native_parser(vec!["test".into(), "core.hoon".into()], true, linemap)
            .parse(src)
            .into_result()
            .expect("core should parse");

        let Hoon::TisSig(items) = parsed else {
            panic!("expected top-level TisSig");
        };
        let [Hoon::Dbug(_, core)] = items.as_slice() else {
            panic!("expected one traced core expression");
        };
        let Hoon::BarCen(_, tomes) = core.as_ref() else {
            panic!("expected a |% core");
        };
        let arm = tomes
            .get("$")
            .and_then(|(_, arms)| arms.get("gen"))
            .expect("expected ++gen arm");
        let Hoon::Dbug(spot, _) = arm else {
            panic!("expected traced arm body");
        };

        assert_eq!(spot.q.p, (6, 3));
    }

    fn map_is_apt_mug(slab: &mut NounSlab, tree: Noun) -> bool {
        fn inner(tree: Noun, min: Option<Noun>, max: Option<Noun>, slab: &mut NounSlab) -> bool {
            if noun_is_zero(tree) {
                return true;
            }

            let Ok([node, left, right]) = tree.uncell(&slab.noun_space()) else {
                return false;
            };
            let Ok([key, _val]) = node.uncell(&slab.noun_space()) else {
                return false;
            };

            if let Some(min_key) = min {
                if gor_mug(slab, key, min_key) {
                    return false;
                }
            }
            if let Some(max_key) = max {
                if gor_mug(slab, max_key, key) {
                    return false;
                }
            }

            if !noun_is_zero(left) {
                let Ok([left_node, _, _]) = left.uncell(&slab.noun_space()) else {
                    return false;
                };
                let Ok([left_key, _]) = left_node.uncell(&slab.noun_space()) else {
                    return false;
                };
                if !mor_mug(slab, key, left_key) {
                    return false;
                }
            }

            if !noun_is_zero(right) {
                let Ok([right_node, _, _]) = right.uncell(&slab.noun_space()) else {
                    return false;
                };
                let Ok([right_key, _]) = right_node.uncell(&slab.noun_space()) else {
                    return false;
                };
                if !mor_mug(slab, key, right_key) {
                    return false;
                }
            }

            inner(left, min, Some(key), slab) && inner(right, Some(key), max, slab)
        }

        inner(tree, None, None, slab)
    }

    fn find_tip_mug_mismatch(limit: u64) -> Option<(u64, u64)> {
        let hasher = DefaultTipHasher;
        let mut slab: NounSlab = NounSlab::new();

        for a in 1..=limit {
            for b in 1..=limit {
                if a == b {
                    continue;
                }
                let mut a_tip = D(a);
                let mut b_tip = D(b);
                let tip_less = match gor_tip(&mut slab, &mut a_tip, &mut b_tip, &hasher) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                let mug_a = slab_mug(D(a), &slab.noun_space());
                let mug_b = slab_mug(D(b), &slab.noun_space());
                if mug_a == mug_b {
                    continue;
                }
                let mug_less = mug_a < mug_b;
                if tip_less != mug_less {
                    return Some((a, b));
                }
            }
        }

        None
    }

    #[test]
    fn flay_kettis_uses_inner_skin() {
        let expr = Hoon::KetTis(
            Skin::Term("bnd".to_string()),
            Box::new(Hoon::ColTar(vec![
                Hoon::Limb("c".to_string()),
                Hoon::Limb("d".to_string()),
            ])),
        );

        let skin = flay(expr).expect("flay should return a skin for bnd=[c d]");
        let expected = Skin::Name(
            "bnd".to_string(),
            Box::new(Skin::Cell(
                Box::new(Skin::Term("c".to_string())),
                Box::new(Skin::Term("d".to_string())),
            )),
        );

        assert_eq!(skin, expected);
    }

    #[test]
    fn open_tiscol_lowers_to_axis_one_cncb() {
        let pairs = vec![(vec![Limb::Term("foo".to_string())], Hoon::Axis(2))];
        let gen = Hoon::TisCol(pairs.clone(), Box::new(Hoon::Axis(3)));
        let got = open(gen);
        let expected = Hoon::TisGar(
            Box::new(Hoon::CenCab(vec![Limb::Axis(1)], pairs)),
            Box::new(Hoon::Axis(3)),
        );
        assert_eq!(got, expected);
    }

    #[test]
    fn open_miccol_terminal_lowers_to_tisgar() {
        let p = Hoon::Axis(10);
        let a = Hoon::Axis(20);
        let b = Hoon::Axis(30);
        let got = open(Hoon::MicCol(
            Box::new(p.clone()),
            vec![a.clone(), b.clone()],
        ));
        let expected = Hoon::TisLus(
            Box::new(p),
            Box::new(Hoon::CenCol(
                Box::new(Hoon::Axis(2)),
                vec![
                    Hoon::TisGar(Box::new(Hoon::Axis(3)), Box::new(a)),
                    Hoon::TisGar(Box::new(Hoon::Axis(3)), Box::new(b)),
                ],
            )),
        );
        assert_eq!(got, expected);
    }

    #[test]
    fn map_to_noun_respects_mug_order() {
        let (a, b) = find_tip_mug_mismatch(512).expect("no tip/mug mismatch found, raise limit");

        let mut slab: NounSlab = NounSlab::new();
        let map = map_to_noun(&mut slab, vec![(D(a), D(0)), (D(b), D(1))]);
        assert!(
            map_is_apt_mug(&mut slab, map),
            "map_to_noun produced a non-apt mug map for keys {a} and {b}"
        );
    }

    #[test]
    fn limb_to_noun_encodes_axis_and_parent_tags() {
        let mut slab = NounSlab::new();

        let axis = limb_to_noun(&mut slab, &Limb::Axis(1));
        let expected_axis = T(&mut slab, &[D(0), D(1)]);
        assert!(
            slab_noun_equality(&slab, &axis, &expected_axis),
            "axis limb did not encode as [0 axis]"
        );

        let parent = limb_to_noun(&mut slab, &Limb::Parent(2, None));
        let expected_parent = T(&mut slab, &[D(1), D(2), D(0)]);
        assert!(
            slab_noun_equality(&slab, &parent, &expected_parent),
            "parent limb did not encode as [1 axis 0]"
        );

        let parent_with_term = limb_to_noun(&mut slab, &Limb::Parent(3, Some("foo".to_string())));
        let term = term_to_noun(&mut slab, "foo");
        let expected_term = T(&mut slab, &[D(0), term]);
        let expected_parent_with_term = T(&mut slab, &[D(1), D(3), expected_term]);
        assert!(
            slab_noun_equality(&slab, &parent_with_term, &expected_parent_with_term),
            "parent limb with term did not encode as [1 axis [0 term]]"
        );
    }

    #[test]
    fn line_map_expands_gap_start_over_blank_and_comment_lines() {
        let src = "a\n\n::  comment\n|%\n";
        let start = src.find("|%").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (3, 1),
            "expected start to include doc comment line"
        );
        assert_eq!(spot.q.q, (4, 3), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_over_detail_docs_before_equals_slash_body() {
        // hoonc's seam consumes a leading detail-format (four-space) doc
        // block inside the %dbug span of an arm body, even when the body
        // starts with =/ (h-zoon roswell parity case).
        let src = concat!(
            "++  test-foo\n", "  ::    the legacy z-set uses raw gor\n",
            "  ::    so things stay consistent\n", "  =/  one  17\n", "  (add one one)\n",
        );
        let start = src.find("=/").expect("missing rune");
        let end = src.len() - 1;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (2, 3),
            "expected start to include the detail doc block"
        );
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_bare_doc_markers() {
        let src = "::\n~%  %two  +  ~\n";
        let start = src.find("~%").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 1), "expected start to stay on the rune line");
        assert_eq!(spot.q.q, (2, 3), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_over_inline_doc_comment_lines() {
        let src = concat!("|%\n", "++  fn  ::  summary\n", "  ::  details\n", "  $%  [%f]\n",);
        let start = src.find("::  details").expect("missing doc");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (2, 9),
            "expected start to include inline doc comment"
        );
        assert_eq!(spot.q.q, (3, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_over_inline_doc_branch_tags_without_heading() {
        let src = concat!("  %stet  ::  end\n", "  %dent  ::  out\n");
        let start = src.find("%dent").expect("missing branch tag");
        let end = start + "%dent".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (line, col) = linemap.line_col(start);

        assert_eq!(
            spot.q.p,
            (line, col),
            "expected start to stay on the branch tag line"
        );
    }

    #[test]
    fn line_map_does_not_expand_gap_start_over_inline_doc_branch_tags_without_heading_for_plain_tag(
    ) {
        let src =
            concat!("  %op-l  ::  0 means atom\n", "  %op-r  ::  0 means atom\n", "  %count\n",);
        let start = src.find("%count").expect("missing branch tag");
        let end = start + "%count".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (line, col) = linemap.line_col(start);

        assert_eq!(
            spot.q.p,
            (line, col),
            "expected start to stay on the %count line"
        );
    }

    #[test]
    fn line_map_expands_gap_start_over_inline_doc_branch_tags_with_heading() {
        let src = concat!("  %stet  ::  == end\n", "  %dent  ::  out\n");
        let start = src.find("%dent").expect("missing branch tag");
        let end = start + "%dent".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src.find("::  == end").expect("missing inline doc");
        let expected_col = (doc_offset + 1) as u64;
        let line2_start = src.find('\n').expect("missing newline") + 1;
        let expected_end_col = ((start - line2_start) + 1 + "%dent".len()) as u64;

        assert_eq!(
            spot.q.p,
            (1, expected_col),
            "expected start to anchor to inline doc comment"
        );
        assert_eq!(spot.q.q, (2, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_over_inline_doc_after_dollar_question_line() {
        let src = concat!("  $?  %rule  ::  --- horz rule\n", "      %fens  ::  ``` code fence\n",);
        let start = src.find("%fens").expect("missing branch tag");
        let end = start + "%fens".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src.find("::  --- horz rule").expect("missing inline doc");
        let expected_col = (doc_offset + 1) as u64;

        assert_eq!(
            spot.q.p,
            (1, expected_col),
            "expected start to anchor to inline doc on $?-line"
        );
    }

    #[test]
    fn line_map_expands_gap_start_over_inline_doc_after_dollar_question_line_with_list_marker() {
        let src = concat!("  $?  %lite  ::  + line item\n", "      %lint  ::  - line item\n",);
        let start = src.find("%lint").expect("missing branch tag");
        let end = start + "%lint".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src.find("::  + line item").expect("missing inline doc");
        let expected_col = (doc_offset + 1) as u64;

        assert_eq!(
            spot.q.p,
            (1, expected_col),
            "expected start to anchor to inline doc on $?-line"
        );
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_dollar_question_inline_doc_non_heading() {
        let src = concat!("  $?  %down  ::  outer embed\n", "      %lunt  ::  unordered list\n",);
        let start = src.find("%lunt").expect("missing branch tag");
        let end = start + "%lunt".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (start - line_start + 1) as u64;
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (2, expected_col),
            "expected start to stay on the branch line"
        );
        assert_eq!(spot.q.q, (2, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_prefers_nearest_branch_tag_heading() {
        let src = concat!(
            "  $?  %rule  ::  --- horz rule\n", "      %fens  ::  ``` code fence\n",
            "      %expr  ::  ;sail expression\n",
        );
        let start = src.find("%expr").expect("missing branch tag");
        let end = start + "%expr".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src.find("::  ``` code fence").expect("missing inline doc");
        let (doc_line, doc_col) = linemap.line_col(doc_offset);

        assert_eq!(
            spot.q.p,
            (doc_line, doc_col),
            "expected start to anchor to nearest branch inline doc heading"
        );
    }

    #[test]
    fn line_map_expands_gap_start_over_inline_doc_with_outer_blank_line() {
        let src = concat!(
            "::\n", "++  fn  ::  summary\n", "        ::\n", "        ::  details\n",
            "        $%  [%f]\n",
        );
        let start = src.find("::  details").expect("missing doc");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (2, 9),
            "expected start to include inline doc comment"
        );
        assert_eq!(spot.q.q, (4, 11), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_from_body_to_inline_doc_header() {
        let src = concat!(
            "::\n", "++  fn  ::  summary\n", "        ::\n", "        ::  details\n",
            "        $%  [%f]\n",
        );
        let start = src.find("$%").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (2, 9),
            "expected start to include inline doc comment"
        );
        assert_eq!(spot.q.q, (5, 11), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_tilde_slash_runes_with_inline_header() {
        let src =
            concat!("++  spun  ::  internal spin\n", "  ::\n", "  ::  a: list\n", "  ~/  %spun\n",);
        let start = src.find("~/").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (4, 3), "expected start to stay on the rune line");
        assert_eq!(spot.q.q, (4, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_tilde_slash_runes_without_inline_header() {
        let src = concat!("++  rev\n", "  ::  reverses block order\n", "  ~/  %rev\n",);
        let start = src.find("~/").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (3, 3), "expected start to stay on the rune line");
        assert_eq!(spot.q.q, (3, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_underscore_runes() {
        let src = concat!("++  step\n", "  ::  atom size or offset, in bloqs\n", "  _`@u`1\n",);
        let start = src.find("_`").expect("missing underscore rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 3), "expected start to include doc comment");
        assert_eq!(spot.q.q, (3, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_face_lines() {
        let src = concat!("  ::  transformed list\n", "  p:(spin a b)\n",);
        let start = src.find("p:").expect("missing face");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 3), "expected start to stay on the face line");
        assert_eq!(spot.q.q, (2, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_equals_arrow_lines() {
        let src = concat!("  ::  doc about the binding above\n", "  =>  foo\n",);
        let start = src.find("=>").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (2, 3),
            "expected start to stay on the equals-arrow line"
        );
        assert_eq!(spot.q.q, (2, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_prefers_doc_block_over_inline_doc_lines() {
        let src = concat!(
            "  ++  lth  ::  less-than\n", "  ::  comparisons return ~ in the event of a NaN\n",
            "  |=  [a=?]\n",
        );
        let start = src.find("|=").expect("missing gate rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (2, 3),
            "expected start to prefer the doc block line"
        );
        assert_eq!(spot.q.q, (3, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_prefers_inline_doc_when_blank_doc_line_follows() {
        let src = concat!(
            "::\n", "++  fn  ::  float, infinity, or NaN\n", "  ::\n", "  ::  details follow\n",
            "  $%  foo\n",
        );
        let start = src.find("$%").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 9), "expected start to use inline doc line");
        assert_eq!(spot.q.q, (5, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_equals_runes() {
        let src = concat!("  ::  doc about binding\n", "  =/  foo  42\n",);
        let start = src.find("=/").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (1, 3), "expected start to include the doc line");
        assert_eq!(spot.q.q, (2, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_equals_tall_runes() {
        let src = concat!("  ::  doc about binding\n", "  =/  foo\n", "    42\n",);
        let start = src.find("=/").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (1, 3), "expected start to include the doc line");
        assert_eq!(spot.q.q, (2, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_equals_with_label_doc_block() {
        let src = concat!("  ::  foo: header\n", "  ::  bar: details\n", "  =/  foo  42\n",);
        let start = src.find("=/").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (3, 3),
            "expected start to stay on the equals line"
        );
        assert_eq!(spot.q.q, (3, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_face_binding_with_label_doc_block() {
        let src = concat!(
            "  ::  fex: primary parser\n", "  ::  sab: secondary parser\n", "  ::\n",
            "  fex=rule\n",
        );
        let start = src.find("fex=rule").expect("missing binding");
        let end = start + "fex=rule".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let expected_end = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (4, 3),
            "expected start to stay on the binding line"
        );
        assert_eq!(spot.q.q, expected_end, "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_equals_after_label_and_diagram() {
        let src = concat!(
            "  ::  foo: header\n", "  ::\n", "  ::      AB\n", "  ::\n", "  ::    +foo is fine\n",
            "  =/  foo\n",
        );
        let start = src.find("=/").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (5, 3),
            "expected start to anchor to the text doc line"
        );
        assert_eq!(spot.q.q, (6, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_prefers_indented_doc_lines_in_block() {
        let src = concat!(
            "  ::  header line\n", "  ::  ?~  foo\n", "  ::    =+  bar\n", "  ::    ?~(bar ~)\n",
            "  [~ %a]\n",
        );
        let start = src.find("[~").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (3, 3),
            "expected start to anchor to indented doc line"
        );
        assert_eq!(spot.q.q, (5, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_after_branch_label_dedent() {
        let src =
            concat!("    [%core *]\n", "  ::  core fallback\n", "  ::    =+  foo\n", "  [~ %a]\n",);
        let start = src.find("[~").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (3, 3),
            "expected start to include indented doc line after branch label"
        );
        assert_eq!(spot.q.q, (4, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_branch_label_without_indented_docs() {
        let src = concat!("    [%core *]\n", "  ::  short branch comment\n", "  [~ %a]\n",);
        let start = src.find("[~").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (3, 3), "expected start to stay on the code line");
        assert_eq!(spot.q.q, (3, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_percent_equals() {
        let src = concat!("  ::  apply changes\n", "  %=  foo\n",);
        let start = src.find("%=").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (2, 3),
            "expected start to stay on the percent-equals line"
        );
        assert_eq!(spot.q.q, (2, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_colon_runes() {
        let src = concat!("  ==\n", "  ::  descend into cell\n", "  ::\n", "  :+  %cell\n",);
        let start = src.find(":+").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (4, 3),
            "expected start to stay on the colon rune line"
        );
        assert_eq!(spot.q.q, (4, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_equals_after_blank_doc_block() {
        let src = concat!(
            "  =/  foo  1\n", "  ::\n", "  ::  note about the next binding\n", "  =/  bar  2\n",
        );
        let start = src.rfind("=/").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (4, 3),
            "expected start to stay on the equals line"
        );
        assert_eq!(spot.q.q, (4, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_bar_gate_after_blank_doc_block() {
        let src = concat!(
            "  =-  (cook - werk)\n", "  ::\n", "  ::  collect raw tarp into xml tags\n",
            "  |=  gaf=(list graf)\n",
        );
        let start = src.find("|=").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (start - line_start + 1) as u64;
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (4, expected_col),
            "expected start to stay on the |= line"
        );
        assert_eq!(spot.q.q, (4, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_bar_cab_after_section_doc_block() {
        let src = concat!(
            "--\n", "::\n", "::  structs\n", "::\n", "::  helper functions\n", "|_  a=*\n",
            "++  foo  a\n", "--\n",
        );
        let start = src.find("|_").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(spot.q.p, (expected_line, expected_col));
        assert_eq!(spot.q.q, (end_line, end_col));
    }

    #[test]
    fn line_map_expands_gap_start_for_bar_cab_after_indented_doc_line() {
        let src = concat!(
            "~%  %door  ..part  ~\n", "::    door docs\n", "|_  a=*\n", "++  foo  a\n", "--\n",
        );
        let start = src.find("|_").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_start = src.find("::").expect("missing doc");
        let (expected_line, expected_col) = linemap.line_col(doc_start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(spot.q.p, (expected_line, expected_col));
        assert_eq!(spot.q.q, (end_line, end_col));
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_bar_gate_after_internal_blank_doc_line() {
        let src = concat!(
            "++  load\n", "  ::  use the below for validation of new state upgrades\n",
            "  ::  |=  untyped-arg=*\n", "  ::\n", "  ::  use this for production\n",
            "  |=  arg=@\n",
        );
        let start = src.rfind("|=").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to stay on the |= line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_gate_doc_comment() {
        let src = concat!(
            "++  map-to-poly\n",
            "  ::  keys need to be 0, 1, 2, ... which is enforced by \"got\" below\n",
            "  |=  mp=(map @ felt)\n",
        );
        let start = src.find("|=").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to stay on the |= line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_gate_doc_after_tilde_slash() {
        let src =
            concat!("++  max\n", "  ~/  %max\n", "  ::  unsigned maximum\n", "  |=  [a=@ b=@]\n",);
        let start = src.find("|=").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_start = src.find("::  unsigned maximum").expect("missing doc line");
        let doc_line_start = src[..doc_start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (doc_start - doc_line_start + 1) as u64;
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (3, expected_col),
            "expected start to anchor to the doc line"
        );
        assert_eq!(spot.q.q, (4, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_gate_doc_with_label_section_after_blank() {
        let src = concat!(
            "++  add\n", "  ~/  %add\n", "  ::  unsigned addition\n", "  ::\n",
            "  ::  a: augend\n", "  ::  b: addend\n", "  |=  [a=@ b=@]\n",
        );
        let start = src.find("|=").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_start = src.find("::  unsigned addition").expect("missing doc line");
        let doc_line_start = src[..doc_start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (doc_start - doc_line_start + 1) as u64;
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (3, expected_col),
            "expected start to anchor to the doc line"
        );
        assert_eq!(spot.q.q, (7, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_gate_doc_with_label_section_without_tilde_slash() {
        let src = concat!(
            "++  sponge\n", "  ::  sponge construction\n", "  ::\n",
            "  ::  preperm: permutation function\n", "  ::  padding: padding function\n",
            "  |=  [a=@ b=@]\n",
        );
        let start = src.find("|=").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to stay on the |= line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_underscore_after_bar_dollar_doc_block() {
        let src = concat!(
            "++  trap\n", "  |$  [product]\n", "  ::  a core with one arm `$`\n", "  ::\n",
            "  _|?($:product)\n",
        );
        let start = src.find("_|?").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_start = src
            .find("::  a core with one arm `$`")
            .expect("missing doc line");
        let doc_line_start = src[..doc_start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (doc_start - doc_line_start + 1) as u64;
        let expected_end_col = (end - src[..start].rfind('\n').map_or(0, |idx| idx + 1) + 1) as u64;

        assert_eq!(
            spot.q.p,
            (3, expected_col),
            "expected start to move to the doc line"
        );
        assert_eq!(spot.q.q, (5, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_outer_doc_block_before_inline_doc() {
        let src = concat!("  ::  outer doc\n", "    foo  ::  inline doc\n", "  ::\n", "    bar\n",);
        let start = src.find("bar").expect("missing bar");
        let end = start + 3;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (start - line_start + 1) as u64;
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (4, expected_col),
            "expected start to stay on the bar line"
        );
        assert_eq!(spot.q.q, (4, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_blank_doc_block_for_percent_rune() {
        let src = concat!(
            "  %+  cook  foo\n", "  ::\n", "  ::  note about the next rune\n", "  %+  ifix  bar\n",
        );
        let start = src.rfind("%+").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (start - line_start + 1) as u64;
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (4, expected_col),
            "expected start to stay on the percent rune line"
        );
        assert_eq!(spot.q.q, (4, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_doc_block_at_arm_body_start() {
        let src = concat!("++  main\n", "  ::\n", "  ::  intro\n", "  =/  foo  1\n",);
        let start = src.find("=/").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(start);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected arm body start to stay on the =/ line"
        );
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_equals_after_multiparagraph_arm_docs() {
        let src = concat!(
            "++  main\n", "  ::  high-level arm docs\n", "  ::\n", "  ::  implementation note\n",
            "  =/  foo  1\n",
        );
        let start = src.find("=/").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(spot.q.p, (expected_line, expected_col));
        assert_eq!(spot.q.q, (end_line, end_col));
    }

    #[test]
    fn line_map_uses_body_start_for_arm_header_span_before_equals_dot() {
        let src = concat!(
            "++  test-v1-fee-consolidation-cheaper-than-splitting\n",
            "  ::  Test that consolidating notes is cheaper than splitting them\n",
            "  ::  With input-fee-divisor=4:\n",
            "  ::  - Consolidation: high witness words, low seed words\n",
            "  ::  - Splitting: low witness words, high seed words\n",
            "  ::  Consolidation should be cheaper since inputs are discounted\n", "  ::\n",
            "  ::  Fee formula: seed_words * base_fee + witness_words * base_fee / divisor\n",
            "  ::\n", "  ::  With same total words (500), compare:\n",
            "  ::  - Consolidation: 400 witness, 100 seed\n",
            "  ::  - Splitting: 100 witness, 400 seed\n", "  ::\n",
            "  =.  constants  bc-with-fees:helpers\n", "  =/  base-fee=@  256\n",
            "  ?>  =(51.200 consolidation-fee)\n",
        );
        let start = src.find("++").expect("missing arm header");
        let body_start = src.find("=.").expect("missing body rune");
        let end = src
            .find("?>  =(51.200 consolidation-fee)")
            .expect("missing final assertion")
            + "?>  =(51.200 consolidation-fee)".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(body_start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected arm header span to anchor to the body rune"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_bar_percent_after_doc_block_with_prior_line() {
        let src = concat!(
            "~%  %one  +  ~\n", "::    layer-1\n", "::\n", "::  basic mathematical operations\n",
            "|%\n",
        );
        let start = src.rfind("|%").expect("missing bar-percent");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (2, 1),
            "expected start to include the doc block before |%"
        );
        assert_eq!(spot.q.q, (5, 3), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_bar_percent_after_fas_import_doc_block() {
        let src = concat!("/=  foo  /bar\n", "::  doc block for file, not the core\n", "|%\n",);
        let start = src.find("|%").expect("missing |%");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (start - line_start + 1) as u64;
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (3, expected_col),
            "expected start to stay on the |% line"
        );
        assert_eq!(spot.q.q, (3, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_doc_block_between_equals_lines() {
        let src = concat!("  =/  foo  1\n", "  ::  comment\n", "  =/  bar  2\n",);
        let start = src.rfind("=/").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (start - line_start + 1) as u64;
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (3, expected_col),
            "expected start to stay on the equals line"
        );
        assert_eq!(spot.q.q, (3, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_equals_slash_after_inline_doc_comment() {
        let src =
            concat!("  =|  in=foo\n", "  ::  comment about the next binding\n", "  =/  bar  2\n",);
        let start = src.find("=/").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (start - line_start + 1) as u64;
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (3, expected_col),
            "expected start to stay on the =/ line"
        );
        assert_eq!(spot.q.q, (3, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_multi_line_arm_doc_block_before_equals_slash() {
        let src = concat!(
            "  ++  main\n", "    ::  state of the parsing loop.\n", "    ::  more detail\n",
            "    =/  verbose  &\n",
        );
        let start = src.rfind("=/").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(start);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected arm body start to stay on the =/ line"
        );
    }

    #[test]
    fn line_map_expands_gap_start_for_tall_equals_after_doc_block() {
        let src = concat!(
            "  =/  len  1\n", "  ::  +lip overlap length\n", "  =/  lip\n", "    =+  foo  1\n",
        );
        let start = src.rfind("=/  lip").expect("missing tall binding");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src.find("::  +lip").expect("missing doc");
        let doc_line_start = src[..doc_offset].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (doc_offset - doc_line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (2, expected_col),
            "expected start to include the doc block for the tall binding"
        );
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_tall_equals_after_doc_block_without_name() {
        let src = concat!(
            "  =/  axis  1\n", "  ::  compute merkle opening\n", "  =/  leaf\n", "    42\n",
        );
        let start = src.rfind("=/  leaf").expect("missing tall binding");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (start - line_start + 1) as u64;
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (3, expected_col),
            "expected start to stay on the tall equals line"
        );
        assert_eq!(spot.q.q, (3, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_tall_equals_after_blank_doc_block() {
        let src = concat!(
            "  =/  f  1\n", "  ::\n", "  ::  Note about tmp\n", "  =/  tmp\n", "    =+  foo  1\n",
        );
        let start = src.rfind("=/  tmp").expect("missing tall binding");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (start - line_start + 1) as u64;
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (4, expected_col),
            "expected start to stay on the tall equals line"
        );
        assert_eq!(spot.q.q, (4, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_equals_bar_doc_block() {
        let src = concat!("  ::  output stack\n", "  =|  lug=wall\n",);
        let start = src.find("=|").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (2, 3),
            "expected start to stay on the equals-bar line"
        );
        assert_eq!(spot.q.q, (2, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_over_indented_comment_lines() {
        let src = "  :: comment\n  foo\n";
        let start = src.find("foo").expect("missing foo");
        let end = start + 3;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (2, 3),
            "expected start to stay on the indented code line"
        );
        assert_eq!(spot.q.q, (2, 6), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_over_indented_doc_block_for_gate_lines() {
        let src = concat!(
            "++  mul\n", "  ~/  %mul\n", "  ::    unsigned multiplication\n", "  ::\n",
            "  ::  a: multiplicand\n", "  ::  b: multiplier\n", "  |:  [a=`@`1 b=`@`1]\n",
        );
        let start = src.find("|:").expect("missing gate rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (3, 3),
            "expected start to include indented doc block"
        );
        assert_eq!(spot.q.q, (7, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_over_doc_block_after_tall_terminator() {
        let src = concat!(
            "~%  %tri  +\n", "  ==\n", "    %year  year\n", "  ==\n", "::    layer-3\n", "::\n",
            "|%\n",
        );
        let start = src.find("|%").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (5, 1),
            "expected start to include doc block after == terminator"
        );
        assert_eq!(spot.q.q, (7, 3), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_over_doc_block_after_dedent_body() {
        let src = concat!(
            "  =+  foo\n", "    bar\n", "  ::\n", "  ::  fold into accumulator\n", "  ::\n",
            "  %+  roll  foo\n",
        );
        let start = src.find("%+").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (6, 3),
            "expected start to stay on the rune line after dedent"
        );
        assert_eq!(spot.q.q, (6, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_less_indented_doc_headers() {
        let src = concat!("  ::  section header\n", "    foo\n");
        let start = src.find("foo").expect("missing foo");
        let end = start + 3;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (2, 5),
            "expected start to stay on the deeper-indented line"
        );
        assert_eq!(spot.q.q, (2, 8), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_colon_banner_doc_lines() {
        let src = concat!("  ::    :::::: cores\n", "  foo\n");
        let start = src.find("foo").expect("missing foo");
        let end = start + 3;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 3), "expected start to stay on the code line");
        assert_eq!(spot.q.q, (2, 6), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_xx_doc_line() {
        let src = concat!("  ::  XX not a docstring\n", "  foo\n");
        let start = src.find("foo").expect("missing foo");
        let end = start + 3;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 3), "expected start to stay on the code line");
        assert_eq!(spot.q.q, (2, 6), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_blank_inline_doc_marker() {
        let src = concat!("  $%  ::\n", "      ::  entry doc\n", "      ::\n", "      foo\n",);
        let start = src.find("foo").expect("missing foo");
        let end = start + 3;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (4, 7), "expected start to stay on the entry line");
        assert_eq!(spot.q.q, (4, 10), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_list_item() {
        let src = concat!(
            "    [first]\n", "    ::\n", "    ::  second doc\n", "    ::\n", "    [second]\n",
        );
        let start = src.find("[second]").expect("missing second");
        let end = start + "[second]".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (5, 5),
            "expected start to stay on the list item line"
        );
        assert_eq!(spot.q.q, (5, 13), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_list_header_inline_doc() {
        let src = concat!("    :~  0  ::  pad\n", "        o0\n");
        let start = src.find("o0").expect("missing item");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (2, 9),
            "expected start to stay on the list item line"
        );
        assert_eq!(spot.q.q, (2, 11), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_plus_header_tall_body() {
        let src =
            concat!("+$  seminoun\n", "  ::  partial noun\n", "  ::\n", "  $~  foo\n", "  bar\n",);
        let start = src.find("$~").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (4, 3), "expected start to stay on the rune line");
        assert_eq!(spot.q.q, (4, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_arm_inline_doc() {
        let src = concat!("++  put  ::  insert new tail\n", "  |*  b=*\n");
        let start = src.find("|*").expect("missing gate");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 3), "expected start to stay on the gate line");
        assert_eq!(spot.q.q, (2, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_plus_header_doc_block_for_bar_star() {
        let src = concat!("++  hmac\n", "  ~/  %hmac\n", "  ::  main logic\n", "  |*  [a=@]\n",);
        let start = src.find("|*").expect("missing gate");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (line, col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (line, col),
            "expected start to stay on the gate line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_arm_inline_doc_with_doc_block() {
        let src = concat!(
            "++  line  ^+  .  ::  body line loop\n", "  ::\n", "  ::  abort after first error\n",
            "  ?:  !=(~ err)  .\n",
        );
        let start = src.find("?:").expect("missing if");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (4, 3), "expected start to stay on the ?: line");
        assert_eq!(spot.q.q, (4, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_to_arm_inline_doc_before_doc_block() {
        let src = concat!(
            "++  fn  ::  float, infinity, or NaN\n", "        ::\n",
            "        ::  s=sign, e=exponent, a=arithmetic form\n",
            "        $%  [%f s=? e=@s a=@u]\n",
        );
        let start = src.find("$%").expect("missing union");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (1, 9),
            "expected start to expand to the arm inline doc"
        );
        assert_eq!(spot.q.q, (4, 11), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_type_inline_doc() {
        let src = concat!("+$  stud  ::  standard name\n", "  $@  mark=@tas\n");
        let start = src.find("$@").expect("missing atom");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 3), "expected start to stay on the $@ line");
        assert_eq!(spot.q.q, (2, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_arm_inline_doc_for_equals() {
        let src = concat!("++  fl  ::  arb. precision fp\n", "  =/  foo  bar\n");
        let start = src.find("=/").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 3), "expected start to stay on the =/ line");
        assert_eq!(spot.q.q, (2, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_arm_inline_doc_for_plus_rune() {
        let src = concat!("++  sym  ::  symbol\n", "  +%  cook\n");
        let start = src.find("+%").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 3), "expected start to stay on the +% line");
        assert_eq!(spot.q.q, (2, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_arm_inline_doc_for_dollar_atom() {
        let src = concat!("++  pony  ::  raw match\n", "  $@  ~\n");
        let start = src.find("$@").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 3), "expected start to stay on the $@ line");
        assert_eq!(spot.q.q, (2, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_switch_inline_doc() {
        let src = concat!("  ?+  t  r  ::  switch doc\n", "    %a  foo\n");
        let start = src.find("%a").expect("missing branch");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (2, 5),
            "expected start to stay on the branch line"
        );
        assert_eq!(spot.q.q, (2, 7), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_if_inline_doc() {
        let src = concat!("  ?:  c  ::  cond doc\n", "    ?~  a  b\n");
        let start = src.find("?~").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 5), "expected start to stay on the rune line");
        assert_eq!(spot.q.q, (2, 7), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_dollar_inline_doc() {
        let src = concat!("  $@  $?  %noun  ::  any noun\n", "      %cell\n");
        let start = src.find("%cell").expect("missing branch");
        let end = start + 5;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (2, 7),
            "expected start to stay on the branch line"
        );
        assert_eq!(spot.q.q, (2, 12), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_dollar_percent_inline_doc() {
        let src = concat!("  $%  [%a p]  ::  ~(p q r...)\n", "      [%b q]\n",);
        let start = src.find("[%b").expect("missing branch");
        let end = start + 3;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (line, col) = linemap.line_col(start);

        assert_eq!(
            spot.q.p,
            (line, col),
            "expected start to stay on the branch line"
        );
    }

    #[test]
    fn line_map_expands_gap_start_over_branch_inline_doc_before_colon_rune() {
        let src = concat!(
            "        :^    %cnls  ::  %+\n",
            "            [%tsgr [%limb %v] p.gen]  ::  =>(v {p.gen})\n",
            "          [%cncl [%limb %b] [%limb %c] ~]               ::    (b c)\n",
            "        :+  %cnts  [%a ~]                               ::  a(,.+6 c)\n",
        );
        let start = src.find(":+  %cnts").expect("missing :+ rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src.find("::    (b c)").expect("missing inline doc");
        let (line, col) = linemap.line_col(doc_offset);

        assert_eq!(
            spot.q.p,
            (line, col),
            "expected start to anchor to inline doc on branch line"
        );
    }

    #[test]
    fn line_map_expands_gap_start_over_doc_block_under_equals_plus_tuple() {
        let src = concat!(
            "++  ax\n", "  =+  :*  ::  .dom: axis to home\n", "          ::  .hay: wing to home\n",
            "          ::\n", "          dom=`axis`1\n", "          hay=*wing\n", "      ==\n",
        );
        let start = src.find("dom=`axis`1").expect("missing dom line");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src
            .find("::  .dom: axis to home")
            .expect("missing inline doc");
        let (doc_line, doc_col) = linemap.line_col(doc_offset);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (doc_line, doc_col),
            "expected start to anchor to inline doc on the tuple header line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_prefers_branch_inline_doc_over_dollar_percent_heading() {
        let src = concat!(
            "  $%  [%bold p=(list graf)]  ::  *bold*\n",
            "      [%talc p=(list graf)]  ::  _italics_\n",
        );
        let start = src.find("[%talc").expect("missing branch");
        let end = start + "[%talc".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (line, col) = linemap.line_col(start);

        assert_eq!(
            spot.q.p,
            (line, col),
            "expected start to stay on the branch line"
        );
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_dollar_percent_inline_branch_doc() {
        let src = concat!("  $%  [%m-root p=@]  ::  root\n", "      [%puzzle p=@]\n",);
        let start = src.find("[%puzzle").expect("missing branch");
        let end = start + "[%puzzle".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (line, col) = linemap.line_col(start);

        assert_eq!(
            spot.q.p,
            (line, col),
            "expected start to stay on the branch line"
        );
        assert_eq!(spot.q.q, linemap.line_col(end), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_semicolon_tilde_inline_doc() {
        let src = concat!(
            "  ;~  pfix  (plus whitespace)  ::  separated by some whitespace\n",
            "    %+  cook  crip  ;~  pose  ::  enclosed in quotes\n", "      foo\n",
        );
        let start = src.find("%+").expect("missing %+ line");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (line, col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (line, col),
            "expected start to stay on the %+ line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_tuple_inline_doc() {
        let src = concat!("  $:  auth=@tas  ::  standards authority\n", "      type=path\n");
        let start = src.find("type=path").expect("missing field");
        let end = start + 4;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 7), "expected start to stay on the field line");
        assert_eq!(spot.q.q, (2, 11), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_dollar_colon_heading_doc_block() {
        let src = concat!(
            "  $:  version=%0\n", "      ::\n", "      ::  hashchains\n",
            "      last-nock-block=@\n",
        );
        let start = src.find("last-nock-block").expect("missing field");
        let end = start + "last".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (line, col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (line, col),
            "expected start to stay on the field line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_bare_dollar_colon_inline_doc() {
        let src = concat!(
            "  $:  ::  if non-null, enforces output source\n",
            "      output-source=(unit source)\n",
        );
        let start = src.find("output-source").expect("missing field");
        let end = start + 6;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (line, col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (line, col),
            "expected start to stay on the field line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_after_equals_slash_doc_block_with_nested_bullets() {
        let src = concat!(
            "  =/  signature-leaves=@\n", "    ::  - 13 leaves for the key\n",
            "    ::    - 6 for x, 6 for y, 1 for inf flag\n",
            "    ::  - 16 leaves for the signature\n", "    =/  num-sigs-required=@\n",
        );
        let start = src
            .find("=/  num-sigs-required")
            .expect("missing assignment");
        let end = start + 2;
        let doc_start = src
            .find("::    - 6 for x, 6 for y, 1 for inf flag")
            .expect("missing nested doc line");
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (line, col) = linemap.line_col(doc_start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (line, col),
            "expected start to expand to the nested doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_doc_block_with_nested_bullets_before_code() {
        let src = concat!(
            "  ::  this goes without saying\n", "  ::    - do not call it\n",
            "  ::    - do not sign it\n", "  [~ ~ %.n]\n",
        );
        let start = src.find("[~ ~ %.n]").expect("missing branch result");
        let end = start + 2;
        let doc_start = src
            .find("::    - do not call it")
            .expect("missing nested doc line");
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (line, col) = linemap.line_col(doc_start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (line, col),
            "expected start to expand to the nested doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_dollar_colon_section_doc_block() {
        let src = concat!(
            "  $:  version=%0\n", "      foo=@\n", "      ::  track unsettled asset allocations\n",
            "      ::\n", "      ::  For each hashchain we need two sets:\n", "      bar=@\n",
        );
        let start = src.find("bar=@").expect("missing field");
        let end = start + 3;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (line, col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (line, col),
            "expected start to stay on the field line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_after_bare_dollar_colon_inline_doc_with_doc_block() {
        let src = concat!(
            "  $:  ::  header\n", "      ::  details\n", "      output-source=(unit source)\n",
        );
        let start = src.find("output-source").expect("missing field");
        let end = start + 6;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src.find("::").expect("missing inline doc");
        let (line, col) = linemap.line_col(doc_offset);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (line, col),
            "expected start to anchor to the inline doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_doc_line_between_dollar_colon_fields() {
        let src = concat!(
            "  $:  ::  header\n", "      output-source=(unit source)\n",
            "      ::    the .sig of the output note\n", "      recipient=sig\n",
        );
        let start = src.find("recipient=sig").expect("missing field");
        let end = start + "recipient".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src
            .find("::    the .sig of the output note")
            .expect("missing doc line");
        let (doc_line, doc_col) = linemap.line_col(doc_offset);

        assert_eq!(
            spot.q.p,
            (doc_line, doc_col),
            "expected start to anchor to the tuple field doc line"
        );
    }

    #[test]
    fn line_map_expands_gap_start_for_doc_block_before_dollar_colon_equals_field() {
        let src = concat!(
            "  $:  ::  header\n", "      foo=bar\n", "      ::    line one\n",
            "      ::    line two\n", "      =baz\n",
        );
        let start = src.find("=baz").expect("missing =baz");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src.find("::    line one").expect("missing doc line");
        let (doc_line, doc_col) = linemap.line_col(doc_offset);

        assert_eq!(
            spot.q.p,
            (doc_line, doc_col),
            "expected start to anchor to the tuple doc block"
        );
    }

    #[test]
    fn line_map_expands_gap_start_after_blank_doc_line_under_dollar_colon() {
        let src = concat!(
            "  $:  absolute=timelock-range  ::  a range of absolute pages\n", "      ::\n",
            "      ::    a range of relative diffs\n", "      relative=timelock-range\n",
        );
        let start = src.find("relative=timelock-range").expect("missing field");
        let end = start + "relative".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src
            .find("::    a range of relative diffs")
            .expect("missing doc line");
        let (doc_line, doc_col) = linemap.line_col(doc_offset);

        assert_eq!(
            spot.q.p,
            (doc_line, doc_col),
            "expected start to anchor to the doc line after the blank marker"
        );
    }

    #[test]
    fn line_map_expands_gap_start_for_nested_dollar_colon_doc_block() {
        let src = concat!(
            "  +$  form\n", "    $:  $:  version=%0  ::  utxo version number\n",
            "          ::    the page number in which the note was added\n",
            "          origin-page=page-number\n",
            "          ::    a note with a null timelock has no restrictions\n",
            "          =timelock\n",
        );
        let start = src
            .find("origin-page=page-number")
            .expect("missing origin-page field");
        let end = start + "origin-page".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src
            .find("::    the page number in which the note was added")
            .expect("missing doc line");
        let (doc_line, doc_col) = linemap.line_col(doc_offset);

        assert_eq!(
            spot.q.p,
            (doc_line, doc_col),
            "expected start to anchor to the nested $: doc line"
        );
    }

    #[test]
    fn line_map_expands_gap_start_with_compact_doc_lines_under_dollar_colon() {
        let src = concat!(
            "  +$  form\n", "    $:  $:  version=%0  ::  utxo version number\n",
            "          ::    the page number in which the note was added\n",
            "          ::NOTE while for dumbnet this could be block-id instead\n",
            "          ::would simplify some code, for airwalk this would lead to a hashloop\n",
            "          origin-page=page-number\n", "          =timelock\n",
        );
        let start = src
            .find("origin-page=page-number")
            .expect("missing origin-page field");
        let end = start + "origin-page".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src
            .find("::    the page number in which the note was added")
            .expect("missing doc line");
        let (doc_line, doc_col) = linemap.line_col(doc_offset);

        assert_eq!(
            spot.q.p,
            (doc_line, doc_col),
            "expected start to anchor to the doc line before compact comments"
        );
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_inline_field_doc_under_dollar_colon() {
        let src = concat!(
            "  +$  form\n", "    $+  page\n", "    $:  digest=block-id\n",
            "        :: everything below this is what is hashed for the digest: +.page\n",
            "        pow=$+(pow (unit proof))\n",
        );
        let start = src.find("pow=$+").expect("missing pow field");
        let end = start + "pow".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (5, 9), "expected start to stay on the field line");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_equals_inline_doc() {
        let src = concat!("  =+  h  ::  upper bound\n", "    ?|  foo\n");
        let start = src.find("?|").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 5), "expected start to stay on the rune line");
        assert_eq!(spot.q.q, (2, 7), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_dollar_runes() {
        let src =
            concat!("+$  bite\n", "  ::  atom slice specifier\n", "  $@(bloq [=bloq =step])\n",);
        let start = src.find("$@").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 3), "expected start to include doc comment");
        assert_eq!(spot.q.q, (3, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_dollar_paren_doc_block() {
        let src = concat!("  ::  recursion step\n", "  $(foo 1)\n",);
        let start = src.find("$(").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_start = src.find("::  recursion step").expect("missing doc line");
        let (expected_line, expected_col) = linemap.line_col(doc_start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to anchor to the doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_inline_doc_heading_in_tall_body() {
        let src = concat!(
            "|%\n", "+$  link  ::  header\n", "  $%  [%chat p=term]  ::  |chapter\n",
            "      [%cone p=aura q=atom]\n",
        );
        let start = src.find("[%cone").expect("missing branch");
        let end = start + "[%cone".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src.find("::  |chapter").expect("missing inline doc");
        let (doc_line, doc_col) = linemap.line_col(doc_offset);

        assert_eq!(
            spot.q.p,
            (doc_line, doc_col),
            "expected start to include inline doc heading"
        );
    }

    #[test]
    fn line_map_expands_gap_start_for_inline_doc_branch_in_tall_body() {
        let src = concat!(
            "|%\n", "+$  link  ::  header\n", "  $%  [%chat p=term]  ::  |chapter\n",
            "      [%frag p=term]  ::  .face\n", "      [%funk p=term]\n",
        );
        let start = src.find("[%funk").expect("missing branch");
        let end = start + "[%funk".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src.find("::  .face").expect("missing inline doc");
        let (doc_line, doc_col) = linemap.line_col(doc_offset);

        assert_eq!(
            spot.q.p,
            (doc_line, doc_col),
            "expected start to include inline doc branch"
        );
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_inline_doc_branch_in_tall_body() {
        let src = concat!(
            "|%\n", "+$  link  ::  header\n", "  $%  [%chat p=term]  ::  branch\n",
            "      [%cone p=aura q=atom]\n",
        );
        let start = src.find("[%cone").expect("missing branch");
        let end = start + "[%cone".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (line, col) = linemap.line_col(start);

        assert_eq!(
            spot.q.p,
            (line, col),
            "expected start to stay on the branch line"
        );
    }

    #[test]
    fn line_map_expands_gap_start_for_bracket_lines() {
        let src = concat!(
            "++  qual\n", "  ::  quadruple tuple\n", "  [p=first q=second r=third s=fourth]\n",
        );
        let start = src.find('[').expect("missing list");
        let end = start + 1;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 3), "expected start to include doc comment");
        assert_eq!(spot.q.q, (3, 4), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_bar_paren_lines() {
        let src = "  ::  comment\n  |(foo bar)\n";
        let start = src.find("|(").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (2, 3),
            "expected start to stay on the bar-paren line"
        );
        assert_eq!(spot.q.q, (2, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_bar_hep_lines() {
        let src = "  ::  comment\n  |-  foo\n";
        let start = src.find("|-").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (2, 3),
            "expected start to stay on the bar-hep line"
        );
        assert_eq!(spot.q.q, (2, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_bar_hep_lines_under_plus_header() {
        let src = concat!(
            "++  autoname\n", "  ::  derive name from spec\n", "  ::\n", "  |-  ^-  (unit term)\n",
        );
        let start = src.find("|-").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (2, 3),
            "expected start to include doc comment under the arm header"
        );
        assert_eq!(spot.q.q, (4, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_caret_hep_lines() {
        let src = "  ::  comment\n  ^-  @\n";
        let start = src.find("^-").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (1, 3), "expected start to include doc comment");
        assert_eq!(spot.q.q, (2, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_caret_after_gate_sample_doc() {
        let src = concat!("++  mul\n", "  |:  [a=@ b=@]\n", "  ::  product\n", "  ^-  @\n",);
        let start = src.find("^-").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (4, 3),
            "expected start to stay on the caret-hep line"
        );
        assert_eq!(spot.q.q, (4, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_caret_after_gate_sample_inline_doc() {
        let src = concat!(
            "++  poon\n", "  |=  [pag=(list hoon) goo=tyke]  ::  default to pag\n",
            "  ^-  (unit (list hoon))          ::  for null goo's\n",
        );
        let start = src.find("^-").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src.find("::  default to pag").expect("missing inline doc");
        let expected = linemap.line_col(doc_offset);

        assert_eq!(
            spot.q.p, expected,
            "expected start to anchor to inline doc on the gate sample line"
        );
        assert_eq!(spot.q.q, (3, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_caret_after_tilde_hint_doc() {
        let src = concat!(
            "++  sub\n", "  |=  [a=@ b=@]\n", "  ~_  leaf+\"subtract-underflow\"\n",
            "  ::  difference\n", "  ^-  @\n",
        );
        let start = src.find("^-").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (5, 3),
            "expected start to stay on the caret-hep line"
        );
        assert_eq!(spot.q.q, (5, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_tilde_hint_with_doc_block() {
        let src =
            concat!("++  grow\n", "  |=  a=@\n", "  ::  make al\n", "  ~_  leaf+\"mull-grow\"\n",);
        let start = src.find("~_").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (4, 3),
            "expected start to stay on the tilde-hint line"
        );
        assert_eq!(spot.q.q, (4, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_tilde_hint_doc_line() {
        let src = concat!("  ?:  cond\n", "  ::  ~_  (dunk %note)\n", "  =.  foo  bar\n",);
        let start = src.find("=.").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (3, 3), "expected start to stay on the rune line");
        assert_eq!(spot.q.q, (3, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_tilde_slog_doc_line() {
        let src = concat!("  ::  emit message\n", "  ~>  %slog.[1 'note']\n",);
        let start = src.find("~>").expect("missing ~>");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (start - line_start + 1) as u64;
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (2, expected_col),
            "expected start to stay on the ~> line"
        );
        assert_eq!(spot.q.q, (2, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_tilde_print_doc_line() {
        let src = concat!("  ::  emit message\n", "  ~&  >>  note\n",);
        let start = src.find("~&").expect("missing ~&");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (start - line_start + 1) as u64;
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (2, expected_col),
            "expected start to stay on the ~& line"
        );
        assert_eq!(spot.q.q, (2, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_over_doc_block_after_plus_header_inline_doc() {
        let src =
            concat!("++  dear  ::  header\n", "  ::  unified tool stack\n", "  ::\n", "  ^-  @\n",);
        let start = src.find("^-").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (4, 3),
            "expected start to stay on the caret-hep line"
        );
        assert_eq!(spot.q.q, (4, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_caret_after_plus_header_doc_block() {
        let src = concat!(
            "++  burp\n", "  ::    expel undigested seminouns\n", "  ::\n", "  ^-  type\n",
        );
        let start = src.find("^-").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 3), "expected start to include doc block");
        assert_eq!(spot.q.q, (4, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_caret_after_plain_plus_header_doc_block() {
        let src = concat!("++  test\n", "  ::  plain arm comment\n", "  ::\n", "  ^-  tang\n",);
        let start = src.find("^-").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (4, 3), "expected start to stay on caret line");
        assert_eq!(spot.q.q, (4, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_caret_plus_after_plus_header_doc_line() {
        let src = concat!("++  burp\n", "  ::  expel undigested seminouns\n", "  ^+  .\n",);
        let start = src.find("^+").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 3), "expected start to include doc comment");
        assert_eq!(spot.q.q, (3, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_caret_after_plus_header_doc_line() {
        let src = concat!("++  burp\n", "  ::  expel undigested seminouns\n", "  ^-  type\n",);
        let start = src.find("^-").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (start - line_start + 1) as u64;
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (3, expected_col),
            "expected start to stay on the caret line"
        );
        assert_eq!(spot.q.q, (3, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_tilde_slash_after_plus_header_doc_line() {
        let src = concat!("++  burp\n", "  ::  expel undigested seminouns\n", "  ~/  %burp\n",);
        let start = src.find("~/").expect("missing ~/");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (start - line_start + 1) as u64;
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (3, expected_col),
            "expected start to stay on the ~/"
        );
        assert_eq!(spot.q.q, (3, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_equals_after_plus_header_doc_line() {
        let src = concat!(
            "++  test-block-id-b58-conversion\n",
            "  ::  test block-id conversion to/from base58 along with to-list and list-to-tuple conversion\n",
            "  =/  bid-string=@t  'DfPNrKYEzZgnxBAgiSeqz3D8dKYKTtyQ8z98TRKH94Bhv49qhHdHpKy'\n",
        );
        let start = src.find("=/").expect("missing =/");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (start - line_start + 1) as u64;
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (3, expected_col),
            "expected start to stay on the =/ line"
        );
        assert_eq!(spot.q.q, (3, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_internal_step_doc_before_equals() {
        let src = concat!(
            "++  test-v1-lock-pkh-m-of-n-valid\n",
            "  ::  use v1-phase=2 to quickly reach v1 activation\n",
            "  =.  constants  bc-v1-phase:helpers\n",
            "  =/  con=consensus-state  initial-consensus-state:h\n",
            "  ::  advance to just before v1 coinbase activation\n",
            "  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)\n",
            "  ::\n", "  ::  step 1: create simple v1 coinbase for key1\n", "  ::\n",
            "  ::    we cannot directly create a v1 coinbase with an m-of-n lock because\n",
            "  ::    accept-page splits multi-owner coinbases into separate notes, each\n",
            "  ::    with a 1-of-1 lock for a single owner. instead, we create a simple\n",
            "  ::    coinbase locked to key1 that we'll spend in the next step.\n", "  ::\n",
            "  =/  page0=page:t  (make-empty-page:h par)\n",
        );
        let start = src.find("=/  page0").expect("missing =/");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (start - line_start + 1) as u64;
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (15, expected_col),
            "expected start to stay on the =/ line"
        );
        assert_eq!(spot.q.q, (15, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_internal_step_doc_after_equals_slash() {
        let src = concat!(
            "++  test-v1-invalid-1-of-2-multisig-input-wrong-sig\n",
            "  =/  con=consensus-state  initial-consensus-state:h\n", "  ::\n",
            "  ::  step 1: create simple v1 note for key1\n", "  ::\n",
            "  ::    we use the simple note helper which creates a v1 note\n",
            "  ::    locked to key1 with a simple 1-of-1 lock.\n", "  ::\n",
            "  =/  page0=page:t  default-genesis-page:h\n",
        );
        let start = src.find("=/  page0").expect("missing =/");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_start = src
            .find("::    we use the simple note helper")
            .expect("missing detail doc line");
        let (expected_line, expected_col) = linemap.line_col(doc_start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to include the detail doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_internal_step_doc_after_question_greater() {
        let src = concat!(
            "  ?>  ?=(@ -.note0)\n", "  ::\n",
            "  ::  step 2: spend coinbase into intermediate note with 1-of-2 lock\n", "  ::\n",
            "  ::    we create a note that can be unlocked by EITHER key1 OR key2.\n", "  ::\n",
            "  =/  nam=nname:t  ~(name get:nnote:t note0)\n",
        );
        let start = src.find("=/").expect("missing =/");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_start = src
            .find("::    we create a note")
            .expect("missing detail doc line");
        let (expected_line, expected_col) = linemap.line_col(doc_start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to include the detail doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_internal_step_doc_after_equals_dot() {
        let src = concat!(
            "  =.  con  (~(update-heaviest dcon con constants) page0)\n", "  ::\n",
            "  ::  step 2: spend coinbase into intermediate note with 2-of-3 lock\n", "  ::\n",
            "  ::    we construct a transaction that:\n",
            "  ::    - spends the coinbase (locked to key1)\n",
            "  ::    - creates an output with a 2-of-3 lock requiring 2 sigs\n",
            "  ::    - uses key1's signature to unlock the coinbase\n", "  ::\n",
            "  ::    the witness for this spend proves we can unlock the coinbase.\n",
            "  ::    the seeds for this spend create the new note with the m-of-n lock.\n",
            "  ::\n", "  =/  nam=nname:t  ~(name get:nnote:t coin)\n",
        );
        let start = src.find("=/").expect("missing =/");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_start = src
            .find("::    we construct a transaction")
            .expect("missing detail doc line");
        let (expected_line, expected_col) = linemap.line_col(doc_start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to include the detail doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_tilde_slash_after_plus_header_doc_line_with_tilde() {
        let src = concat!("++  mevy\n", "  ::    ~ if no failures\n", "  ~/  %mevy\n",);
        let start = src.find("~/").expect("missing ~/");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 3), "expected start to include doc comment");
        assert_eq!(spot.q.q, (3, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_type_header_doc_line() {
        let src = concat!("+$  bignum\n", "  ::  LSB order\n", "  [%bn p=@]\n",);
        let start = src.find("[%").expect("missing type");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (start - line_start + 1) as u64;
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (3, expected_col),
            "expected start to stay on the type line"
        );
        assert_eq!(spot.q.q, (3, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_type_header_doc_block() {
        let src =
            concat!("+$  bloq\n", "  ::  blocksize\n", "  ::\n", "  ::  more detail\n", "  @\n",);
        let start = src.find("@").expect("missing type");
        let end = start + 1;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src.find("::  blocksize").expect("missing doc line");
        let (doc_line, doc_col) = linemap.line_col(doc_offset);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (doc_line, doc_col),
            "expected start to include the doc block"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_caret_doc_block() {
        let src = concat!("  ^-  @\n", "  ::  return type\n", "  ?~  a  ~\n",);
        let start = src.find("?~").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (3, 3), "expected start to stay on the rune line");
        assert_eq!(spot.q.q, (3, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_caret_doc_block_for_equals() {
        let src = concat!(
            "  ^-  @\n", "  ::  convert columns\n", "  ::  into marys\n", "  =/  foo=@  0\n",
        );
        let start = src.find("=/").expect("missing =/");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (4, 3), "expected start to stay on the =/ line");
        assert_eq!(spot.q.q, (4, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_caret_doc_block_for_body_single_line() {
        let src = concat!("  ^-  @\n", "  ::  example body\n", "  (add 1 2)\n",);
        let start = src.find("(add").expect("missing body");
        let end = start + 4;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (start_line, start_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (start_line, start_col),
            "expected start to stay on the body line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_caret_doc_block_with_question_greater() {
        let src = concat!(
            "  ^-  noun-digest\n", "  ::  ?>  (based leaf)  commented out\n",
            "  (hash-belts-list ~[leaf])\n",
        );
        let start = src.find("(hash-belts-list").expect("missing body");
        let end = start + "(hash-belts-list".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (start_line, start_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (start_line, start_col),
            "expected start to stay on the body line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_caret_doc_block_for_body_multi_line() {
        let src = concat!("  ^-  @\n", "  ::  line one\n", "  ::  line two\n", "  (add 1 2)\n",);
        let start = src.find("(add").expect("missing body");
        let end = start + 4;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (start_line, start_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (start_line, start_col),
            "expected start to stay on the body line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_question_doc_block() {
        let src =
            concat!("  ?~  a  ~\n", "  ::  any reference faces must be clear\n", "  ?.  b\n",);
        let start = src.find("?.").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (3, 3), "expected start to stay on the rune line");
        assert_eq!(spot.q.q, (3, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_after_question_greater_doc_line() {
        let src = concat!("  ?>  =(a b)\n", "  ::    doc line\n", "  =/  foo  bar\n",);
        let start = src.find("=/").expect("missing =/");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_start = src.find("::    doc line").expect("missing doc line");
        let (expected_line, expected_col) = linemap.line_col(doc_start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to anchor to the doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_question_greater_doc_line_shallow_indent() {
        let src = concat!("  ?>  =(a b)\n", "  ::  doc line\n", "  =/  foo  bar\n",);
        let start = src.find("=/").expect("missing =/");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (start_line, start_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (start_line, start_col),
            "expected start to stay on the =/ line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_question_tilde_after_doc_line() {
        let src = concat!("  ::  if line is blank\n", "  ?~  saw\n");
        let start = src.find("?~").expect("missing ?~");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 3), "expected start to stay on the ?~ line");
        assert_eq!(spot.q.q, (2, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_question_after_tilde_hint_doc_line() {
        let src = concat!("  ~>  %slog.[0 'note']\n", "  ::  check condition\n", "  ?:  =(1 1)\n",);
        let start = src.find("?:").expect("missing ?:");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (start - line_start + 1) as u64;
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (3, expected_col),
            "expected start to stay on the ?: line"
        );
        assert_eq!(spot.q.q, (3, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_use_leading_plain_comment_as_question_span_start() {
        let src = concat!(
            "  |=  block-num=@\n", "  ^-  @  :: emission is number of atoms\n",
            "  ?:  =(0 block-num)  0\n", "  ?:  (gth block-num tail-end)  0\n",
            "  ::  Pre-activation eons preserve the actual on-chain emission\n",
            "  ::  (powers of two, not the rounded doc values).\n",
            "  ?:  (lte block-num eon-0-end)\n",
            "    ^~((mul (bex 16) atoms-per-nock))         :: 65,536 NOCK\n",
            "  ?:  (lte block-num eon-1-end)\n",
            "    ^~((mul (bex 15) atoms-per-nock))         :: 32,768 NOCK\n",
            "  ?:  (lte block-num activation)\n",
            "    ^~((mul (bex 14) atoms-per-nock))         :: 16,384 NOCK\n",
            "  ::  Eon 3: 6-month activation era at 2,048 NOCK.\n",
            "  ?:  (lte block-num eon-3-end)\n", "    ^~((mul 2.048 atoms-per-nock))\n",
            "  ::  Decay phase (eons 4..=12): 9 one-year eras of 210k blocks each.\n",
            "  ?:  (lte block-num decay-end)\n",
            "    =/  era-idx=@  (div (sub block-num +(eon-3-end)) era-blocks)\n",
            "    (mul (snag era-idx decay-rewards) atoms-per-nock)\n",
            "  ::  Tail: 64 NOCK/block until the cap is hit at tail-end.\n",
            "  ^~((mul 64 atoms-per-nock))\n",
        );
        let raw_start = src.find("::  Eon 3").expect("missing leading comment");
        let code_start = src
            .find("?:  (lte block-num eon-3-end)")
            .expect("missing ?: line");
        let end = src.rfind("^~").expect("missing tail") + "^~((mul 64 atoms-per-nock))".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let (start_line, start_col) = linemap.line_col(code_start);
        let (end_line, end_col) = linemap.line_col(end);

        for start in [raw_start, code_start] {
            let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
            assert_eq!(
                spot.q.p,
                (start_line, start_col),
                "expected start to stay on the ?: line"
            );
            assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
        }
    }

    #[test]
    fn line_map_keeps_bullet_doc_start_before_question_span() {
        let src = concat!(
            "  ?:  !=(~ hold)\n",
            "    $(settlements t.settlements)\n",
            "  =/  counterpart=deposit\n",
            "    =+  block-with-deposit=(~(got z-by nock-hashchain.hash-state.state) as-of)\n",
            "    (~(got z-by deposits.block-with-deposit) name)\n",
            "  ::\n",
            "  ::  find the corresponding unsettled deposit in the hash-state.\n",
            "  ::  we do not require the bridge node to have seen the proposal prior to observing\n",
            "  ::  the deposit settlement.\n",
            "  ::    - if bridge node has seen proposal, the deposit will be in the unsettled deposit set.\n",
            "  ::    - if the unsettled deposit is not the unsettled deposit set, this is a STOP condition.\n",
            "  ?.  (has-unsettled-deposit as-of name)\n",
            "    [%| [%stop 'failed to process deposit settlement: cannot find unsettled deposit in state']]\n",
            "  ?.  (check-deposit-settlement counterpart settlement)\n",
            "    [%| [%stop 'failed to process deposit settlement: counterpart does not match settlement']]\n",
            "  =.  unsettled-deposits.hash-state.state\n",
            "    (~(del z-bi unsettled-deposits.hash-state.state) [as-of name])\n",
            "  $(settlements t.settlements)\n",
        );
        let first_bullet = src
            .find("::    - if bridge node")
            .expect("missing first bullet doc line");
        let second_bullet = src
            .find("::    - if the unsettled deposit")
            .expect("missing second bullet doc line");
        let code_start = src
            .find("?.  (has-unsettled-deposit as-of name)")
            .expect("missing ?. line");
        let end = src
            .find("$(settlements t.settlements)")
            .expect("missing recursion")
            + "$(settlements t.settlements)".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let (expected_line, expected_col) = linemap.line_col(first_bullet);
        let (end_line, end_col) = linemap.line_col(end);

        for start in [first_bullet, second_bullet, code_start] {
            let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
            assert_eq!(
                spot.q.p,
                (expected_line, expected_col),
                "expected start to anchor to the first bullet doc line"
            );
            assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
        }
    }

    #[test]
    fn line_map_uses_first_indented_doc_continuation_for_doc_started_span() {
        let src = concat!(
            "  =/  parent=page:t  (to-page:local-page:t parent-local)\n",
            "  ::  determine the target the candidate (child of .parent) must have.\n",
            "  ::    post-activation: compute aserti3-2d fresh from the anchor and the\n",
            "  ::    parent's stored median-of-11. pre-activation: read the next target\n",
            "  ::    stored at parent.digest by the epoch rule.\n",
            "  =/  candidate-height=@  +(~(height get:page:t parent))\n",
            "  =/  candidate-target=bignum:bignum:t\n",
            "    ?:  (post-asert-activation:t candidate-height)\n",
            "      (~(got z-by targets.c) u.heaviest-block.c)\n", "  (add-txs-to-candidate c)\n",
        );
        let first_doc = src
            .find("::    post-activation")
            .expect("missing first indented doc line");
        let second_doc = src
            .find("::    parent's stored")
            .expect("missing second indented doc line");
        let code_start = src
            .find("=/  candidate-height")
            .expect("missing candidate-height line");
        let end = src
            .find("(add-txs-to-candidate c)")
            .expect("missing candidate tail")
            + "(add-txs-to-candidate c)".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let (expected_line, expected_col) = linemap.line_col(first_doc);
        let (end_line, end_col) = linemap.line_col(end);

        for start in [first_doc, second_doc, code_start] {
            let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
            assert_eq!(
                spot.q.p,
                (expected_line, expected_col),
                "expected start to anchor to the first indented doc line"
            );
            assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
        }
    }

    #[test]
    fn line_map_skips_plain_comment_when_span_starts_on_interstitial_doc() {
        let src = concat!(
            "  =/  lock=lock:t  sc  ::  single spend-condition lock\n",
            "  ::  build both versions\n", "  =/  proof-stub=lock-merkle-proof-stub:v1:t\n",
            "    (build-lock-merkle-proof-stub:lock:t lock 1)\n",
            "  =/  proof-full=lock-merkle-proof-full:v1:t\n",
            "    (build-lock-merkle-proof-full:lock:t lock 1)\n", "  ::  hash both\n",
            "  =/  hash-stub=hash:t  (hash:lock-merkle-proof-stub:v1:t proof-stub)\n",
            "  =/  hash-full=hash:t  (hash:lock-merkle-proof-full:v1:t proof-full)\n",
            "  %+  expect-eq\n", "    !>  [%.y %.y %.y]\n",
            "  !>  [hashes-differ stub-deterministic full-deterministic]\n",
        );
        let doc_start = src
            .find("::  build both versions")
            .expect("missing interstitial comment");
        let code_start = src.find("=/  proof-stub").expect("missing proof-stub line");
        let end = src.find("!>  [hashes-differ").expect("missing final vase")
            + "!>  [hashes-differ stub-deterministic full-deterministic]".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let (expected_line, expected_col) = linemap.line_col(code_start);
        let (end_line, end_col) = linemap.line_col(end);

        for start in [doc_start, code_start] {
            let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
            assert_eq!(
                spot.q.p,
                (expected_line, expected_col),
                "expected start to stay on the first =/ line"
            );
            assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
        }
    }

    #[test]
    fn line_map_skips_single_plain_comment_before_equals_slash() {
        let src = concat!(
            "  =/  con  (initial-consensus-state-custom:h bc)\n",
            "  ::  genesis is the only block in state; it is the known parent.\n",
            "  =/  genesis=page:t\n",
            "    (to-page:local-page:t (~(got z-by blocks.con) (need heaviest-block.con)))\n",
            "  ::  honest child of genesis (height 1, epoch-counter 1, timestamp 600)\n",
            "  =/  honest=page:t  (make-empty-page:h genesis)\n",
            "  %+  expect-eq  !>(%.n)  !>(-.result)\n", "  ==\n",
        );
        let doc_start = src
            .find("::  genesis is the only block")
            .expect("missing interstitial comment");
        let code_start = src.find("=/  genesis").expect("missing genesis binding");
        let end = src.rfind("==").expect("missing terminator") + "==".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let (expected_line, expected_col) = linemap.line_col(code_start);
        let (end_line, end_col) = linemap.line_col(end);

        for start in [doc_start, code_start] {
            let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
            assert_eq!(
                spot.q.p,
                (expected_line, expected_col),
                "expected start to stay on the =/ line"
            );
            assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
        }
    }

    #[test]
    fn line_map_expands_gap_start_for_question_colon_after_doc_block_examples() {
        let src = concat!(
            "  ^-  nock\n", "  ::  this optimization can remove crashes\n", "  ::\n",
            "  ::  ?:  ?=([[%0 *] [%0 *]] +<)\n", "  ::    [%0 (div p.vur 2)]\n",
            "  ?:  ?=([[%1 *] [%1 *]] +<)\n",
        );
        let start = src.rfind("?:").expect("missing ?:");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src
            .find("::    [%0 (div p.vur 2)]")
            .expect("missing example doc");
        let (doc_line, doc_col) = linemap.line_col(doc_offset);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (doc_line, doc_col),
            "expected start to anchor to the indented example doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_question_plus_after_caret_plus_doc_block() {
        let src = concat!(
            "  ^+  .\n", "  ::  =-  ~>  %slog.[0 (dunk 'sint: sut')]\n",
            "  ::      ~>  %slog.[0 (dunk(sut ref) 'sint: ref')]\n", "  ?+  ref  .\n",
        );
        let start = src.find("?+").expect("missing ?+");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (start_line, start_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (start_line, start_col),
            "expected start to stay on the ?+ line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_deep_inline_doc() {
        let src = concat!(
            "++  miss  ::  nonintersection\n", "  |=  $:  ::  ref: symmetric type\n",
            "          ::\n", "          ref=type\n",
        );
        let start = src.find("ref=type").expect("missing ref=type");
        let end = start + "ref=type".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (4, 11),
            "expected start to stay on the ref=type line"
        );
        assert_eq!(spot.q.q, (4, 19), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_plus_header_inline_doc() {
        let src = concat!(
            "++  prev\n", "::\n", "++  fn  ::  float, infinity, or NaN\n", "        ::\n",
            "        ::  s=sign, e=exponent\n", "        $%  [%f s=?]\n",
        );
        let start = src.find("$%").expect("missing $%");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (3, 9),
            "expected start to anchor to inline doc on ++ line"
        );
        assert_eq!(spot.q.q, (6, 11), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_plus_header_inline_doc_without_doc_block() {
        let src = concat!(
            "++  trig-style  ::  type of parsed line\n", "  $%  $:  %end  ::  terminator\n",
        );
        let start = src.find("$%").expect("missing $%");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 3), "expected start to stay on the $% line");
        assert_eq!(spot.q.q, (2, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_when_start_has_inline_doc() {
        let src = concat!(
            "  =.  ind  ?~(out.ind [col.saw col.saw] ind)  ::  init indents\n", "  ::\n",
            "  ?:  ?|  ?=(~ par)  :: if after a paragraph or\n",
        );
        let start = src.find("?:").expect("missing ?:");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (3, 3), "expected start to stay on the ?: line");
        assert_eq!(spot.q.q, (3, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_arm_doc_block_with_inline_doc_line() {
        let src = concat!(
            "  ++  analyze\n", "    ::  normalize a fragment of the subject\n", "    ::\n",
            "    |_  $:  ::  axe: axis to fragment\n", "          ::\n", "          axe=axis\n",
            "      ==\n",
        );
        let start = src.find("|_").expect("missing |_");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src
            .find("::  normalize a fragment of the subject")
            .expect("missing doc line");
        let expected = linemap.line_col(doc_offset);
        let expected_end = linemap.line_col(end);

        assert_eq!(
            spot.q.p, expected,
            "expected start to anchor to the arm doc block"
        );
        assert_eq!(spot.q.q, expected_end, "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_note_comment_under_arm() {
        let src = concat!(
            "  ++  permute\n", "    ::NOTE  takes and returns eight values\n",
            "    ::  lists keep the code tidy\n", "    |=  s=(list @)\n",
        );
        let start = src.find("|=").expect("missing |=");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to stay on the |= line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_bar_equals_under_plus_header_doc_block() {
        let src = concat!(
            "  ++  sponge\n", "    ::  sponge construction\n", "    ::\n",
            "    |=  $:  preperm=$-(@ud $-(@ @))\n", "            padding=$-([octs @ud] octs)\n",
            "        ==\n",
        );
        let start = src.find("|=").expect("missing |=");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_start = src
            .find("::  sponge construction")
            .expect("missing doc line");
        let expected = linemap.line_col(doc_start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p, expected,
            "expected start to expand to the doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_plus_header_doc_block_with_rune_lines() {
        let src = concat!(
            "  ++  load\n", "    ::  use the below for validation of new state upgrades\n",
            "    ::  |=  untyped-arg=*\n", "    ::  ~>  %slog.[0 leaf+\"typing kernel state\"]\n",
            "    ::\n", "    ::  use this for production\n", "    |=  arg=load-kernel-state:dk\n",
        );
        let start = src.find("|=  arg").expect("missing |=");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to stay on the |= line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_plus_header_doc_block_after_tilde_slash() {
        let src = concat!(
            "  ++  table-to-verifier-funcs\n", "    ~/  %table-to-verifier-funcs\n",
            "    ::  this arm is theoretically\n", "    ::  ideally this disappears\n",
            "    |=  fs=table-funcs\n",
        );
        let start = src.find("|=  fs").expect("missing |=");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to stay on the |= line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_plus_header_label_doc_block() {
        let src = concat!(
            "  ++  argon2\n", "    ::  out:  128-bit hash\n", "    ::  typ:  hash type\n",
            "    |=  arg=*\n",
        );
        let start = src.find("|=  arg").expect("missing |=");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to stay on the |= line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_bar_header_section_heading() {
        let src = concat!(
            "  |=  [input=octs output=@ud]\n", "  |^  ^-  @\n", "    ::\n", "    ::  padding\n",
            "    =.  input  (padding input bitrate)\n",
        );
        let start = src.find("=.").expect("missing =.");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to stay on the =. line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_hoon_138_analyze_doc_block() {
        let src = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../hoonc/hoon/hoon-138.hoon"
        ));
        let start = src
            .find("|_  $:  ::  axe: axis to fragment")
            .expect("missing analyze gate line");
        let end = start + 2;
        let doc_line_start = src
            .find("    ::    normalize a fragment of the subject")
            .expect("missing analyze doc line");
        let doc_offset = doc_line_start + 4;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let expected = linemap.line_col(doc_offset);

        assert_eq!(
            spot.q.p, expected,
            "expected start to anchor to hoon-138 analyze doc block"
        );
    }

    #[test]
    fn line_map_expands_gap_start_for_hoon_138_lip_doc_block() {
        let src = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../hoonc/hoon/hoon-138.hoon"
        ));
        let doc_line_start = src
            .find("      ::    +lip is (lent B), where +hay is forward AB")
            .expect("missing lip doc line");
        let doc_offset = doc_line_start + 6;
        let start = src[doc_line_start..]
            .find("      =/  lip")
            .map(|idx| idx + doc_line_start)
            .expect("missing lip binding");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let expected = linemap.line_col(doc_offset);

        assert_eq!(
            spot.q.p, expected,
            "expected start to anchor to hoon-138 lip doc block"
        );
    }

    #[test]
    fn line_map_expands_gap_start_for_hoon_138_max_doc_line() {
        let src = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../hoonc/hoon/hoon-138.hoon"
        ));
        let doc_line_start = src
            .find("  ::    unsigned maximum")
            .expect("missing max doc line");
        let doc_offset = doc_line_start + 2;
        let start = src[doc_line_start..]
            .find("|=  [a=@ b=@]")
            .map(|idx| idx + doc_line_start)
            .expect("missing max sample");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let expected = linemap.line_col(doc_offset);

        assert_eq!(
            spot.q.p, expected,
            "expected start to anchor to hoon-138 max doc line"
        );
    }

    #[test]
    fn line_map_expands_gap_start_for_tilde_percent_doc_block_with_inline_doc() {
        let src = concat!(
            "~%  %ext-field  ..belt  ~\n",
            "::    math-ext: arithmetic for elements and polynomials over the extension field.\n",
            "|_  deg=_`@`3  ::  field extension degree\n",
        );
        let start = src.find("|_").expect("missing |_");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src
            .find("::    math-ext: arithmetic for elements")
            .expect("missing doc line");
        let expected = linemap.line_col(doc_offset);
        let expected_end = linemap.line_col(end);

        assert_eq!(
            spot.q.p, expected,
            "expected start to anchor to the ~% doc block"
        );
        assert_eq!(spot.q.q, expected_end, "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_tilde_percent_doc_block_before_bar_percent() {
        let src = concat!(
            "++  fl\n", "  =>\n", "    ~%  %cofl  +>  ~\n", "    ::    cofl\n", "    ::\n",
            "    ::  internal functions; mostly operating on [e=@s a=@u]\n",
            "    ::  positive numbers.\n", "    |%\n", "    ++  rou\n",
        );
        let start = src.find("|%").expect("missing |%");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src.find("::    cofl").expect("missing doc line");
        let expected = linemap.line_col(doc_offset);
        let expected_end = linemap.line_col(end);

        assert_eq!(
            spot.q.p, expected,
            "expected start to anchor to the ~% doc block before |%"
        );
        assert_eq!(spot.q.q, expected_end, "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_question_after_equals_inline_doc() {
        let src = concat!(
            "        ::\n", "        ::  line is not blank\n", "        =>  .(saw u.saw)\n",
            "        ::\n", "        ::  if end of input, complete\n",
            "        ?:  ?=(%end -.sty.saw)\n", "          ..$(q.loc col.saw)\n", "        ::\n",
            "        =.  ind  ?~(out.ind [col.saw col.saw] ind)      ::  init indents\n",
            "        ::\n",
            "        ?:  ?|  ?=(~ par)                          :: if after a paragraph or\n",
            "                ?&  ?=(?(%down %lime %bloc) p.cur)  :: unspaced new container\n",
            "                    |(!=(%old -.sty.saw) (gth col.saw inr.ind))\n",
            "            ==  ==\n",
        );
        let start = src.rfind("?:  ?|").expect("missing ?: ?|");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (11, 9), "expected start to stay on the ?: line");
        assert_eq!(spot.q.q, (11, 11), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_doc_line_between_code_lines() {
        let src = concat!(
            "  %+  cook  join-tops\n", "  ::  look for sail first, or markdown if not\n",
            "  (most gap ;~(pose top-level (stag %| cram)))\n",
        );
        let start = src.find("(most").expect("missing (most");
        let end = start + "(most".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (3, 3), "expected start to stay on the (most line");
        assert_eq!(spot.q.q, (3, 8), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_doc_block_with_rune_content() {
        let src = concat!(
            "  =+  si\n", "  ::  ?>  ?&  =(a b)\n", "  ::          =(c d)\n", "  ::      ==\n",
            "  =+  q\n",
        );
        let start = src.rfind("=+  q").expect("missing =+ line");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (start_line, start_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (start_line, start_col),
            "expected start to stay on the =+ line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_flat_doc_after_equals_dot() {
        let src = concat!(
            "    =.  rng  ~(verifier-fiat-shamir proof-stream proof)\n",
            "    ::\n",
            "    ::  We now use the randomness to compute the expected fingerprints of the compute stack and product stack based on the given [s f] and product, respectively.\n",
            "    ::  We then dynamically generate constraints that force the cs and ps to be equivalent to the expected fingerprints.\n",
            "    ::  As long as the prover replicates this exact protocol, the opened indicies should match up.\n",
            "    ::  The boundary constraint then ensures that the computation in cleartext is linked to the computation in the trace.\n",
            "    ::\n",
            "    ::  generate scalars for the random linear combination of the composition polynomial\n",
            "    =/  total-constraints=@\n",
        );
        let start = src.find("=/").expect("missing =/");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (start - line_start + 1) as u64;
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (9, expected_col),
            "expected start to stay on the =/ line"
        );
        assert_eq!(spot.q.q, (9, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_to_commented_slog_terminator_before_question() {
        let src = concat!(
            "    =/  has-excluded  (has-excluded tx-id)\n",
            "    =/  has-bnb-raw-tx  (has-bnb-raw-tx tx-id)\n",
            "    =/  has-raw-tx  (has-raw-tx tx-id)\n",
            "    ::~&  :*  has-excluded+has-excluded\n",
            "    ::        has-bnb-raw-tx+has-bnb-raw-tx\n",
            "    ::        has-raw-tx+has-raw-tx\n", "    ::    ==\n", "    ?&  has-excluded\n",
            "        !has-bnb-raw-tx\n", "        has-raw-tx\n", "    ==\n",
        );
        let start = src.find("?&").expect("missing ?& line");
        let end = start + 2;
        let expected_start = src.find("::    ==").expect("missing commented terminator");
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let expected = linemap.line_col(expected_start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p, expected,
            "expected start to match the commented == terminator"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_rune_doc_block_after_question_branch() {
        let src = concat!(
            "  ?:  =(a b)\n", "    [~ ~]\n", "  ::  ?>  ?&  =(c d)\n", "  ::          =(e f)\n",
            "  ::      ==\n", "  =+  q=(fra d c)\n",
        );
        let start = src.rfind("=+  q").expect("missing =+ line");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (start_line, start_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (start_line, start_col),
            "expected start to stay on the =+ line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_doc_block_in_question_list() {
        let src = concat!(
            "  ?&  (check)\n", "      ::\n", "      ::  extra detail\n", "      (verify)\n",
        );
        let start = src.find("(verify").expect("missing (verify");
        let end = start + "(verify".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (start_line, start_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (start_line, start_col),
            "expected start to stay on the (verify line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_doc_block_between_question_branches() {
        let src = concat!(
            "  ?:  cond\n", "    [~ ~ %.y]\n", "  ::\n", "  ::  this goes without saying\n",
            "  ::    - do not call\n", "  [~ ~ %.n]\n",
        );
        let start = src.rfind("[~ ~ %.n]").expect("missing false branch");
        let end = start + 1;
        let expected = src.find("::    -").expect("missing bullet doc line");
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(expected);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to expand to the bullet doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_multi_bullet_doc_block_between_question_branches() {
        let src = concat!(
            "  ?:  cond\n", "    [~ ~ %.y]\n", "  ::\n", "  ::  this goes without saying\n",
            "  ::    - do not call\n", "  ::    - do not sign\n", "  ::    - return stop\n",
            "  [~ ~ %.n]\n",
        );
        let start = src.rfind("[~ ~ %.n]").expect("missing false branch");
        let end = start + 1;
        let expected = src
            .find("::    - do not call")
            .expect("missing first bullet doc line");
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(expected);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to anchor to the first bullet doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_question_doc_block_with_internal_blank_and_bullets() {
        let src = concat!(
            "      ?:  ?&(dest-matches amount-matches tx-id-matches)\n",
            "        [~ ~ %.y]\n",
            "      ::\n",
            "      ::  if this condition is hit, it should result in a stop condition because\n",
            "      ::  it means the deposit entry under (block-hash, name) exists, but the\n",
            "      ::  the destination and/or amount does not match. This means that the proposer\n",
            "      ::  submitted an invalid proposal.\n",
            "      ::\n",
            "      ::  this goes without saying, if [~ ~ %.n] is returned:\n",
            "      ::    - do not call %evaluate-base-call\n",
            "      ::    - do not sign the proposal\n",
            "      ::    - return a STOP condition\n",
            "      [~ ~ %.n]\n",
        );
        let start = src.rfind("[~ ~ %.n]").expect("missing false branch");
        let end = start + 1;
        let expected = src
            .find("::    - do not call %evaluate-base-call")
            .expect("missing first bullet doc line");
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(expected);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to anchor to the first bullet doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_from_bullet_doc_line_with_internal_blank_to_first_bullet() {
        let src = concat!(
            "      ?:  ?&(dest-matches amount-matches tx-id-matches)\n",
            "        [~ ~ %.y]\n",
            "      ::\n",
            "      ::  if this condition is hit, it should result in a stop condition because\n",
            "      ::  it means the deposit entry under (block-hash, name) exists, but the\n",
            "      ::  the destination and/or amount does not match. This means that the proposer\n",
            "      ::  submitted an invalid proposal.\n",
            "      ::\n",
            "      ::  this goes without saying, if [~ ~ %.n] is returned:\n",
            "      ::    - do not call %evaluate-base-call\n",
            "      ::    - do not sign the proposal\n",
            "      ::    - return a STOP condition\n",
            "      [~ ~ %.n]\n",
        );
        let start = src
            .find("::    - return a STOP condition")
            .expect("missing last bullet doc line");
        let end = start + 2;
        let expected = src
            .find("::    - do not call %evaluate-base-call")
            .expect("missing first bullet doc line");
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(expected);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to anchor to the first bullet doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_from_bullet_doc_line_to_first_bullet() {
        let src = concat!(
            "  ?:  cond\n", "    [~ ~ %.y]\n", "  ::\n", "  ::  this goes without saying\n",
            "  ::    - do not call\n", "  ::    - do not sign\n", "  ::    - return stop\n",
            "  [~ ~ %.n]\n",
        );
        let start = src
            .find("::    - return stop")
            .expect("missing last bullet doc line");
        let end = start + 2;
        let expected = src
            .find("::    - do not call")
            .expect("missing first bullet doc line");
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(expected);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to anchor to the first bullet doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_plain_doc_between_question_branches() {
        let src = concat!("  ?:  cond\n", "    foo\n", "  ::  otherwise\n", "  bar\n",);
        let start = src.rfind("bar").expect("missing bar");
        let end = start + 1;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (start_line, start_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (start_line, start_col),
            "expected start to stay on the bar line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_mixed_indent_doc_block_with_leading_blank_before_question() {
        let src = concat!(
            "  =/  foo  1\n", "  ::\n", "  ::  note about foo\n", "  ::    - detail\n",
            "  ?.  bar\n",
        );
        let start = src.find("?.").expect("missing ?. line");
        let end = start + 2;
        let expected = src.find("::    - detail").expect("missing bullet doc line");
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(expected);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to anchor to the bullet doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_prefers_shallow_indent_in_mixed_indent_doc_block() {
        let src = concat!(
            "  =/  foo  0\n", "  ::  TODO: revisit\n", "  ::    produce settlement\n",
            "  ::      deeper detail\n", "  =/  bar  1\n",
        );
        let start = src.rfind("=/  bar").expect("missing =/ bar");
        let end = start + 2;
        let expected = src
            .find("::    produce settlement")
            .expect("missing shallow indented doc line");
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(expected);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to anchor to the shallow indented doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_doc_block_under_question_caret() {
        let src = concat!("  ?^  foo\n", "    ::  note\n", "    ::    nested\n", "    ?:  bar\n",);
        let start = src.find("?:").expect("missing ?:");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (start_line, start_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (start_line, start_col),
            "expected start to stay on the ?: line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_skips_trailing_doc_line_when_span_starts_on_doc() {
        let src = concat!(
            "  ^-  noun-digest\n", "  ::  ?>  (based leaf)  commented out\n",
            "  (hash-belts-list ~[leaf])\n",
        );
        let doc_start = src.find("::").expect("missing doc line");
        let doc_end = doc_start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((doc_start, doc_end), &wer, &linemap);
        let (end_line, end_col) = linemap.line_col(doc_end);

        assert_eq!(
            spot.q.p,
            (3, 3),
            "expected start to move to the (hash-belts-list line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_when_non_doc_comment_breaks_doc_block() {
        let src = concat!(
            "  ?.  flag\n", "    ::  this case only happens during testing\n",
            "    ::%  skipping pow hash check\n", "    %.y\n",
        );
        let start = src.find("%.y").expect("missing %.y");
        let end = start + 3;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (4, 5), "expected start to stay on the %.y line");
        assert_eq!(spot.q.q, (4, 8), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_triple_quote_after_compact_tilde_doc() {
        let src = concat!(
            "  ?:  cond\n", "    ::~&  >\n", "    ::    \"\"\"\n", "    ::    detailed note\n",
            "    ::    \"\"\"\n", "    =/  log-message\n",
        );
        let start = src.find("=/").expect("missing =/");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src.find("::    \"\"\"").expect("missing triple quote");
        let (doc_line, doc_col) = linemap.line_col(doc_offset);

        assert_eq!(
            spot.q.p,
            (doc_line, doc_col),
            "expected start to anchor to the triple-quote doc line"
        );
        let (end_line, end_col) = linemap.line_col(end);
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_doc_block_after_compact_question_doc() {
        let src = concat!(
            "  ::  header\n", "  ::?:  %+  levy  tables\n", "  ::    |=  t=table-dat\n",
            "  ::    !=(step.p.p.t base-width.p.t)\n", "  =/  num-tables  (lent tables)\n",
        );
        let start = src.find("=/").expect("missing =/");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src.find("::    |=").expect("missing doc line");
        let (doc_line, doc_col) = linemap.line_col(doc_offset);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (doc_line, doc_col),
            "expected start to anchor to the doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_doc_block_under_question_dot() {
        let src = concat!(
            "  ?.  flag\n", "    ::  pending blocks are waiting on tx\n",
            "    =/  tx-pending-blocks  foo\n",
        );
        let start = src.find("=/").expect("missing =/");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (3, 5), "expected start to stay on the =/ line");
        assert_eq!(spot.q.q, (3, 7), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_doc_line_after_outdent_terminator() {
        let src = concat!("    ==\n", "  ::  comment after close\n", "  =^  foo  bar\n",);
        let start = src.find("=^").expect("missing =^");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (line, col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (line, col),
            "expected start to stay on the =^ line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_doc_line_after_outdent_terminator_before_list() {
        let src = concat!("    ==\n", "  ::  comment after close\n", "  [foo bar]\n",);
        let start = src.find("[foo").expect("missing list");
        let end = start + 1;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (line, col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (line, col),
            "expected start to stay on the list line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_simple_doc_under_question_colon() {
        let src = concat!(
            "  ?:  cond\n", "    ::  ask for next-heaviest block\n", "    =/  log-message\n",
        );
        let start = src.find("=/").expect("missing =/");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (3, 5), "expected start to stay on the =/ line");
        assert_eq!(spot.q.q, (3, 7), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_triple_quote_doc_under_question_colon() {
        let src = concat!(
            "  ?:  cond\n", "    ::  \"\"\"\n", "    ::  detailed note\n", "    ::  \"\"\"\n",
            "    =/  log-message\n",
        );
        let start = src.find("=/").expect("missing =/");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src.find("::  \"\"\"").expect("missing triple quote");
        let (doc_line, doc_col) = linemap.line_col(doc_offset);

        assert_eq!(
            spot.q.p,
            (doc_line, doc_col),
            "expected start to anchor to the triple-quote doc line"
        );
        let (end_line, end_col) = linemap.line_col(end);
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_backtick_literal_after_doc_line() {
        let src = concat!("  ::  return early\n", "  `k\n");
        let start = src.find("`k").expect("missing `k");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (start - line_start + 1) as u64;
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (2, expected_col),
            "expected start to stay on the `k line"
        );
        assert_eq!(spot.q.q, (2, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_doc_block_between_code_lines() {
        let src = concat!(
            "  =/  db  (dec (lent b))\n",
            "  ::  db = 0, rem = ~ => condition below this one is false\n",
            "  ::  Problem is (degree ~) = 0\n", "  ?:  =(db 0)\n",
        );
        let start = src.find("?:").expect("missing ?:");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (start - line_start + 1) as u64;
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (4, expected_col),
            "expected start to stay on the ?: line"
        );
        assert_eq!(spot.q.q, (4, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_mixed_indent_doc_block_between_code_lines() {
        let src = concat!(
            "  %+  roll  (flop xs)\n", "  ::  let sx = (flop xs)\n",
            "  ::    [a b c] => [sx2 sx1 sx0 a b c]\n", "  ::  = [a b c] => [xs0 sx1 sx2 a b c]\n",
            "  |=  [x=pelt ps-new=_ps]\n", "  (~(push-bottom pstack ps-new) x)\n",
        );
        let start = src.find("|=").expect("missing |=");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_start = src
            .find("::    [a b c]")
            .expect("missing indented doc line");
        let (expected_line, expected_col) = linemap.line_col(doc_start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to anchor to indented doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_withdrawal_todo_doc_block() {
        let src = concat!(
            "  ?:  (is-bridge-withdrawal-tx tx)\n",
            "    ::  crash here. there should be no withdrawals from the bridge address until we implement them.\n",
            "    ~>  %slog.[0 'fatal: withdrawal tx detected, but withdrawals are disabled.']\n",
            "    !!\n",
            "  ::  TODO: revisit when its time to implement withdrawals\n",
            "  ::    produce a withdrawal settlement\n",
            "  ::  =/  withdraw-info=(unit [recipient=nock-addr name=nname:t amount=@ as-of=base-hash counterpart-base-event-id=base-event-id])\n",
            "  ::    (extract-withdrawal-info tx)\n",
            "  ::  ?~  withdraw-info\n",
            "  ::    ::  just skip it\n",
            "  ::    $(tx-list t.tx-list)\n",
            "  ::  =/  w-settle=withdrawal-settlement\n",
            "  ::    :*  tx-id\n",
            "  ::        name.u.withdraw-info\n",
            "  ::        ::  TODO: nock-tx-fee\n",
            "  ::        *@\n",
            "  ::    ==\n",
            "  ::  $(tx-list t.tx-list)\n",
            "  $(tx-list t.tx-list)\n",
        );
        let start = src
            .rfind("$(tx-list t.tx-list)")
            .expect("missing $(tx-list)");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_start = src
            .find("::    produce a withdrawal settlement")
            .expect("missing doc line");
        let (expected_line, expected_col) = linemap.line_col(doc_start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to anchor to the indented doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_deep_indent_doc_block_between_code_lines() {
        let src = concat!(
            "  %+  turn  foo\n", "  ::  computes b^7 in 4 base field multiplications\n", "  ::\n",
            "  ::  Note that we are able to replace montiplys with\n",
            "  ::  bmuls due to the fact that R^3 = 1 mod p. Thus:\n",
            "  ::         m^7 = R^7*b^7\n", "  ::            = (R^3)^2*R*b^7\n",
            "  ::            = R*b^7 mod p\n", "  |=  m=@\n",
        );
        let start = src.find("|=").expect("missing |=");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to stay on the |= line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_doc_block_before_question_line() {
        let src = concat!(
            "  ?^  cond\n", "  ::\n", "  ::  heading\n", "  ::    detail\n", "  ::  tail\n",
            "  ?:  yes\n", "  no\n",
        );
        let start = src.find("?:").expect("missing ?:");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to stay on the ?: line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_doc_block_with_blank_between_code_lines() {
        let src = concat!(
            "  =/  term  0\n", "  ::  heading\n", "  ::\n", "  ::  detail line\n",
            "  (do-stuff term)\n",
        );
        let start = src.find("(do-stuff").expect("missing (do-stuff");
        let end = start + "(do-stuff".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_start = src.find("::  heading").expect("missing doc header");
        let (expected_line, expected_col) = linemap.line_col(doc_start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to anchor to the doc header line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_percent_caret_after_blank_doc_block() {
        let src = concat!(
            "  =/  foo  0\n", "  ::\n", "  ::  Indexing & Selector Constraints\n", "  ::\n",
            "  %^  tag-mp-pelt  %ln-inc\n", "    (mpsub-pelt foo bar)\n",
        );
        let start = src.find("%^").expect("missing %^");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (expected_line, expected_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to stay on the %^ line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_to_label_doc_line_with_heading() {
        let src = concat!(
            "  $~  :*\n", "        v1-phase=39.000\n", "        ::  note data field constraints\n",
            "        ::    max-size: maximum number of leaves\n",
            "        ::    min-fee:  minimum fee\n", "        data=[max-size=2.048 min-fee=256]\n",
            "        ::  base fee per word\n", "        base-fee=(bex 15)\n", "    ==\n",
        );
        let start = src.find("data=").expect("missing data=");
        let end = start + "data".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_start = src
            .find("::    max-size")
            .expect("missing max-size doc line");
        let (expected_line, expected_col) = linemap.line_col(doc_start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to anchor to the label doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_tilde_percent_after_equals_arrow_doc() {
        let src = concat!("=>\n", "::  header\n", "~%  %foo  +  ~\n", "|%\n");
        let start = src.find("~%").expect("missing ~%");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_start = src.find("::  header").expect("missing doc header");
        let (expected_line, expected_col) = linemap.line_col(doc_start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (expected_line, expected_col),
            "expected start to anchor to the doc header line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_prefers_first_doc_line_in_doc_block() {
        let src = concat!(
            "~%  %one  +  ~\n", "::    layer-1\n", "::\n", "::  basic mathematical operations\n",
            "|%\n",
        );
        let start = src.find("|%").expect("missing |%");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (2, 1),
            "expected start to anchor to the first doc line"
        );
        assert_eq!(spot.q.q, (5, 3), "unexpected end spot");
    }

    #[test]
    fn line_map_prefers_doc_line_after_tilde_header_without_blank() {
        let src = concat!("~%  %stark-core  ..tlib  ~\n", "::    stark-core\n", "|%\n");
        let start = src.find("|%").expect("missing |%");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 1), "expected start to anchor to the doc line");
        assert_eq!(spot.q.q, (3, 3), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_ztd_one_header_doc_line() {
        let src = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../hoon/common/ztd/one.hoon"
        ));
        let doc_line_start = src
            .find("::    math-base: base field definitions and arithmetic")
            .expect("missing ztd one header doc line");
        let doc_offset = doc_line_start;
        let start = src.find("|%").expect("missing |%");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let expected = linemap.line_col(doc_offset);

        assert_eq!(
            spot.q.p, expected,
            "expected start to anchor to ztd one header doc line"
        );
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_tilde_header_doc_in_arm() {
        let src = concat!(
            "++  cheetah\n", "  ~%  %cheetah  ..cheetah  ~\n", "  ::  degree-six extension\n",
            "  |%\n",
        );
        let start = src.find("|%").expect("missing |%");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_col = (start - line_start + 1) as u64;
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (4, expected_col),
            "expected start to stay on the |% line"
        );
        assert_eq!(spot.q.q, (4, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_tilde_header_doc_in_arm_heading() {
        let src = concat!(
            "++  lib-u32\n", "  ~%  %lib-u32  +  ~\n", "  ::    Unsigned 32-bit Arithmetic\n",
            "  |%\n",
        );
        let start = src.find("|%").expect("missing |%");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_start = src.find("::").expect("missing doc line");
        let doc_line_start = src[..doc_start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_doc_col = (doc_start - doc_line_start + 1) as u64;
        let line_start = src[..start].rfind('\n').map_or(0, |idx| idx + 1);
        let expected_end_col = (end - line_start + 1) as u64;

        assert_eq!(
            spot.q.p,
            (3, expected_doc_col),
            "expected start to anchor to the heading doc line"
        );
        assert_eq!(spot.q.q, (4, expected_end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_prefers_doc_line_after_tilde_header() {
        let src = concat!(
            "++  max\n", "  ~/  %max\n", "  ::    unsigned maximum\n", "  |=  [a=@ b=@]\n",
        );
        let start = src.find("|=").expect("missing gate rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (3, 3), "expected start to anchor to the doc line");
        assert_eq!(spot.q.q, (4, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_prefers_doc_line_after_caret_bar() {
        let src =
            concat!("++  rq\n", "  ^|\n", "  ::  round to nearest\n", "  |_  r=$?(%n %u %d %z)\n",);
        let start = src.find("|_").expect("missing |_");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (3, 3), "expected start to anchor to the doc line");
        assert_eq!(spot.q.q, (4, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_prefers_inline_doc_for_gate_sample_before_caret() {
        let src = concat!("++  poon\n", "  |=  [a=@]  ::  sample doc\n", "  ^-  @  ::  detail\n",);
        let start = src.find("^-").expect("missing ^-");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (2, 14), "expected start to anchor to inline doc");
        assert_eq!(spot.q.q, (3, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_prefers_inline_doc_for_caret_hep_before_question() {
        let src = concat!(
            "++  poon\n", "  |=  [a=@]\n", "  ^-  @  ::  result type\n",
            "  ?~  a  `~  ::  keep empty\n",
        );
        let start = src.find("?~").expect("missing ?~");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (3, 10), "expected start to anchor to inline doc");
        assert_eq!(spot.q.q, (4, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_caret_hep_before_question_colon() {
        let src = concat!("++  poon\n", "  ^-  @  ::  result type\n", "  ?:  =(0 a)  0\n",);
        let start = src.find("?:").expect("missing ?:");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(spot.q.p, (3, 3), "expected start to stay on the rune line");
        assert_eq!(spot.q.q, (3, 5), "unexpected end spot");
    }

    #[test]
    fn line_map_prefers_inline_doc_for_dollar_paren_after_question() {
        let src = concat!(
            "    %+  both  ::  otherwise head comes\n", "      ?^  foo  ::  from goo or pag\n",
            "    $(bar)  ::  recurse on tails\n",
        );
        let start = src.find("$(").expect("missing $(");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src.find("::  from goo or pag").expect("missing doc");
        let (doc_line, doc_col) = linemap.line_col(doc_offset);

        assert_eq!(
            spot.q.p,
            (doc_line, doc_col),
            "expected start to anchor to inline doc"
        );
        assert_eq!(spot.q.q, (3, 7), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_nested_after_colon_header() {
        let src = concat!("  :^  %wtcl  ::  ?:\n", "    [%bust %flag]  ::  ?\n",);
        let start = src.find("[%bust").expect("missing branch");
        let end = start + "[%bust".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (start_line, start_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (start_line, start_col),
            "expected start to stay on the nested line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_prefers_colon_header_doc_when_nested_doc_is_heading() {
        let src = concat!("  :+  %tsls  ::  header\n", "    [%ktts %b]  ::  =+  b\n",);
        let start = src.find("[%ktts").expect("missing body");
        let end = start + "[%ktts".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src.find("::  header").expect("missing header doc");
        let (doc_line, doc_col) = linemap.line_col(doc_offset);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (doc_line, doc_col),
            "expected start to anchor to the colon header doc"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_colon_header_when_nested_doc_not_heading() {
        let src = concat!("  :+  %ktls  ::  ^+\n", "    [%limb %$]  ::  $\n",);
        let start = src.find("[%limb").expect("missing limb");
        let end = start + "[%limb".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (start_line, start_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (start_line, start_col),
            "expected start to stay on the nested line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_colon_hep_under_colon_header() {
        let src = concat!("  :+  %ktls  ::  ^+\n", "    :-  %brhp  ::  |-\n",);
        let start = src.find(":-").expect("missing :-");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (start_line, start_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (start_line, start_col),
            "expected start to stay on the :- line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_branch_doc_under_colon_header_sibling() {
        let src = concat!(
            "  :^  %wtcl  ::  ?:\n", "      [%bust %flag]  ::  ?\n", "    [%bust %null]  ::  ~\n",
            "  :-  [%ktts %i]  ::  :-  i=~~\n",
        );
        let start = src.find(":-  [%ktts").expect("missing :-");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (start_line, start_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (start_line, start_col),
            "expected start to stay on the :- line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_branch_doc_under_question_header_sibling() {
        let src = concat!(
            "  ?:  ?=(~ a)  ::\n", "    [%tsgr %v]  ::  v\n",
            "  :+  %tsls  [%ktts %a]  ::  =+  a\n",
        );
        let start = src.find(":+  %tsls").expect("missing :+");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (start_line, start_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (start_line, start_col),
            "expected start to stay on the :+ line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_trailing_blank_doc_block() {
        let src = concat!("  ::  descend into cell\n", "  ::\n", "  :+  %cell\n",);
        let start = src.find(":+").expect("missing :+");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (start_line, start_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (start_line, start_col),
            "expected start to stay on the :+ line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_after_mixed_indent_doc_block_with_trailing_blank() {
        let src = concat!(
            "  =/  tables  0\n", "  ::  check that the tables have correct base width\n",
            "  ::?:  %+  levy  tables\n", "  ::    |=  t=table-dat\n",
            "  ::    !=(step.p.p.t base-width.p.t)\n", "  ::\n", "  =/  num-tables  1\n",
        );
        let start = src.rfind("=/  num-tables").expect("missing binding");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src.find("::    |=").expect("missing doc line");
        let (doc_line, doc_col) = linemap.line_col(doc_offset);

        assert_eq!(
            spot.q.p,
            (doc_line, doc_col),
            "expected start to anchor to mixed-indent doc line"
        );
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_tilde_plus_doc_block() {
        let src = concat!(
            "  ~+\n", "  ::\n", "  ::  Equivalent to:\n", "  ::    %-  bar\n",
            "  =/  num-succ  1\n",
        );
        let start = src.rfind("=/  num-succ").expect("missing binding");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (start_line, start_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (start_line, start_col),
            "expected start to stay on the =/ line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_equals_after_blank_in_doc_block() {
        let src = concat!(
            "  =/  tables  0\n", "  ::  compute the Composition Polynomial\n",
            "  ::  This polynomial composes the trace polynomials with the constraints\n",
            "  ::\n",
            "  ::  compute weights used in linear combination of composition polynomial\n",
            "  =/  num-constraints  1\n",
        );
        let start = src.rfind("=/  num-constraints").expect("missing binding");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (start_line, start_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (start_line, start_col),
            "expected start to stay on the =/ line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_leading_blank_doc_block_between_code_lines() {
        let src = concat!(
            "  =/  f  0\n", "  ::\n", "  ::  Note about tmp\n", "  ::\n", "  =/  tmp  1\n",
        );
        let start = src.rfind("=/  tmp").expect("missing rune");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (start_line, start_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (start_line, start_col),
            "expected start to stay on the rune line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_blank_doc_before_dollar_colon() {
        let src = concat!(
            "  ::\n", "  ::  indexes and not-fully-validated state\n", "  $:\n", "    $:  foo=@\n",
        );
        let start = src.find("$:").expect("missing $:");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (start_line, start_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (start_line, start_col),
            "expected start to stay on the $: line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_doc_block_before_dollar_at() {
        let src = concat!(
            "+$  bite\n", "  ::    atom slice specifier\n", "  ::\n", "  $@(bloq [=bloq =step])\n",
        );
        let start = src.find("$@(").expect("missing $@");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src.find("::    atom slice specifier").expect("missing doc");
        let (doc_line, doc_col) = linemap.line_col(doc_offset);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (doc_line, doc_col),
            "expected start to expand to the doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_doc_block_before_step_body() {
        let src = concat!(
            "++  step\n", "  ::    atom size or offset, in bloqs\n", "  ::\n", "  _`@u`1\n",
        );
        let start = src.find("_`@u`1").expect("missing body");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src
            .find("::    atom size or offset, in bloqs")
            .expect("missing doc");
        let (doc_line, doc_col) = linemap.line_col(doc_offset);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (doc_line, doc_col),
            "expected start to expand to the doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_for_doc_block_before_bar_percent() {
        let src = concat!("  ==\n", "::    layer-3\n", "::\n", "|%\n",);
        let start = src.find("|%").expect("missing |%");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src.find("::    layer-3").expect("missing doc");
        let (doc_line, doc_col) = linemap.line_col(doc_offset);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (doc_line, doc_col),
            "expected start to expand to the doc line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_for_branch_tag_under_colon_header_sibling() {
        let src = concat!(
            "  :-  :+  %ktts  ::  ^=\n", "        %a  ::  a\n", "      :+  %ktls  ::  ^+\n",
        );
        let start = src.find(":+  %ktls").expect("missing :+");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (start_line, start_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (start_line, start_col),
            "expected start to stay on the :+ line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_prefers_branch_doc_above_colon_rune() {
        let src =
            concat!("    [%cncl %b %c]  ::  (b c)\n", "  :+  %cnts  [%a ~]  ::  a(,.+6 c)\n",);
        let start = src.find(":+").expect("missing :+");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let doc_offset = src.find("::  (b c)").expect("missing branch doc");
        let (doc_line, doc_col) = linemap.line_col(doc_offset);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (doc_line, doc_col),
            "expected start to anchor to branch doc"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_does_not_expand_gap_start_after_blank_doc_block_for_nested_inline_docs() {
        let src = concat!("  [%$ ~]  ::  $\n", "  ::\n", "    [%leaf *]\n",);
        let start = src.find("[%leaf").expect("missing leaf");
        let end = start + "[%leaf".len();
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);
        let (start_line, start_col) = linemap.line_col(start);
        let (end_line, end_col) = linemap.line_col(end);

        assert_eq!(
            spot.q.p,
            (start_line, start_col),
            "expected start to stay on the nested line"
        );
        assert_eq!(spot.q.q, (end_line, end_col), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_stops_at_file_start_comments() {
        let src = ":: header\n:: more\nfoo\n";
        let start = src.find("foo").expect("missing foo");
        let end = start + 3;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (3, 1),
            "expected start to remain on first code line"
        );
        assert_eq!(spot.q.q, (3, 4), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_keeps_indented_columns_without_doc_comments() {
        let src = "  foo\n";
        let start = src.find("foo").expect("missing foo");
        let end = start + 3;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (1, 3),
            "expected start to keep the indented column"
        );
        assert_eq!(spot.q.q, (1, 6), "unexpected end spot");
    }

    #[test]
    fn line_map_expands_gap_start_keeps_inline_columns() {
        let src = "aa  bb\n";
        let start = src.find("bb").expect("missing bb");
        let end = start + 2;
        let linemap = Arc::new(LineMap::new(src));
        let wer: crate::ast::hoon::Path = vec!["test".to_string()];
        let spot = chumsky_spot_to_hoon_spot((start, end), &wer, &linemap);

        assert_eq!(
            spot.q.p,
            (1, 5),
            "expected start to stay on the inline token column"
        );
        assert_eq!(spot.q.q, (1, 7), "unexpected end spot");
    }
}
