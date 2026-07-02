//! Native Nock formula IR (plan §3.2) — Phase 1 shadow.
//!
//! `Formula::to_noun` emits byte-exact Nock. The smart constructors
//! (`cons`/`comb`/`cond`) reproduce honk's hoon-138 peephole rewrites
//! (`crate::native::formula`) on the native enum. Hint kinds are split (RT-12);
//! axes are arbitrary atoms (RT-08); leaves are provenanced (RT-04).
//!
//! Byte-exactness note: the noun peephole checks are *structural* on the noun
//! (e.g. `noun_pair` splits any cell), so degenerate noun-only forms such as a
//! `Quote` of the literal constant `[0 1]` used as `comb`'s `mal` would match a
//! check that the native producer expresses as `Slot(1)`. The native mint is the
//! only producer of these Formulas and emits the canonical native shape, so the
//! native-structure checks below are byte-exact for the native pipeline. (The
//! noun path had to tolerate arbitrary nouns; the native path constructs them.)
#![allow(dead_code)]

use std::rc::Rc;

use nockapp::noun::slab::NounSlab;
use nockvm::ext::AtomExt;
use nockvm::noun::{Atom, Noun, NounAllocator, NounSpace, D, T};
use num_bigint::BigUint;

use super::leaf::Leaf;
use super::ToNoun;
use crate::errors::{CompilerError, Result};
use crate::native::noun::noun_pair;

/// A Nock axis. Arbitrary-size (Nock 0/9/10 axes are atoms, not `u64`) — RT-08.
#[derive(Clone, Debug)]
pub enum Axis {
    Small(u64),
    Big(Rc<BigUint>),
}

impl Axis {
    fn to_noun(&self, dst: &mut NounSlab) -> Noun {
        match self {
            Axis::Small(v) => Atom::new(dst, *v).as_noun(),
            Axis::Big(b) => Atom::from_bytes(dst, &b.to_bytes_le()).as_noun(),
        }
    }

    /// The small-axis value, mirroring the noun path's `as_u64()` (big axes
    /// return `None`, so they skip the `peg` optimization — byte-exact).
    fn small(&self) -> Option<u64> {
        match self {
            Axis::Small(v) => Some(*v),
            Axis::Big(_) => None,
        }
    }
}

/// A Nock formula.
pub enum Formula {
    Slot(Axis),                                  // [0 axis]
    Quote(Leaf),                                 // [1 const]
    Eval(Rc<Formula>, Rc<Formula>),              // [2 subj form]
    Cell(Rc<Formula>, Rc<Formula>),              // autocons [f g]
    Cond(Rc<Formula>, Rc<Formula>, Rc<Formula>), // [6 p q r]
    Kick {
        axis: Axis,
        core: Rc<Formula>,
    }, // [9 axis core]
    Edit {
        axis: Axis,
        value: Rc<Formula>,
        target: Rc<Formula>,
    }, // [10 [axis value] target]
    JetHint {
        clue: Leaf,
        body: Rc<Formula>,
    }, // [11 clue body]
    NoteHint {
        note: Leaf,
        body: Rc<Formula>,
    }, // [11 note body] (op-12 variant TBD in port)
    Dbug {
        spot: Leaf,
        body: Rc<Formula>,
    }, // [11 spot body]
    Op {
        code: u8,
        args: Vec<Rc<Formula>>,
    }, // [code args…] for 3/4/5/7/8/12 pending typed variants
}

impl Formula {
    pub fn to_noun(&self, dst: &mut NounSlab) -> Noun {
        match self {
            Formula::Slot(axis) => {
                let a = axis.to_noun(dst);
                T(dst, &[D(0), a])
            }
            Formula::Quote(leaf) => {
                let c = leaf.to_noun(dst);
                T(dst, &[D(1), c])
            }
            Formula::Eval(s, f) => {
                let sn = s.to_noun(dst);
                let fn_ = f.to_noun(dst);
                T(dst, &[D(2), sn, fn_])
            }
            Formula::Cell(h, t) => {
                let hn = h.to_noun(dst);
                let tn = t.to_noun(dst);
                T(dst, &[hn, tn])
            }
            Formula::Cond(p, y, w) => {
                let pn = p.to_noun(dst);
                let yn = y.to_noun(dst);
                let wn = w.to_noun(dst);
                T(dst, &[D(6), pn, yn, wn])
            }
            Formula::Kick { axis, core } => {
                let an = axis.to_noun(dst);
                let cn = core.to_noun(dst);
                T(dst, &[D(9), an, cn])
            }
            Formula::Edit {
                axis,
                value,
                target,
            } => {
                let an = axis.to_noun(dst);
                let vn = value.to_noun(dst);
                let edit = T(dst, &[an, vn]);
                let tn = target.to_noun(dst);
                T(dst, &[D(10), edit, tn])
            }
            Formula::JetHint { clue, body } => {
                let cn = clue.to_noun(dst);
                let bn = body.to_noun(dst);
                T(dst, &[D(11), cn, bn])
            }
            Formula::NoteHint { note, body } => {
                let nn = note.to_noun(dst);
                let bn = body.to_noun(dst);
                T(dst, &[D(11), nn, bn])
            }
            Formula::Dbug { spot, body } => {
                let sn = spot.to_noun(dst);
                let bn = body.to_noun(dst);
                T(dst, &[D(11), sn, bn])
            }
            Formula::Op { code, args } => {
                let mut parts = Vec::with_capacity(args.len() + 1);
                parts.push(D(*code as u64));
                for a in args {
                    parts.push(a.to_noun(dst));
                }
                T(dst, &parts)
            }
        }
    }
}

impl ToNoun for Formula {
    fn to_noun(&self, dst: &mut NounSlab) -> Noun {
        Formula::to_noun(self, dst)
    }
}

// ---- shape probes (native equivalents of the noun structural checks) --------

fn rc(f: Formula) -> Rc<Formula> {
    Rc::new(f)
}

/// `[0 a]` with the small-axis value, mirroring `axis_formula_value`.
fn slot_small(f: &Formula) -> Option<u64> {
    match f {
        Formula::Slot(a) => a.small(),
        _ => None,
    }
}

fn is_slot_one(f: &Formula) -> bool {
    matches!(slot_small(f), Some(1))
}

fn is_slot_zero(f: &Formula) -> bool {
    matches!(slot_small(f), Some(0))
}

/// The constant of `[1 c]`, as a small value where possible (for the bool checks).
fn quote_direct(f: &Formula) -> Option<u64> {
    match f {
        Formula::Quote(Leaf::Direct(v)) => Some(*v),
        _ => None,
    }
}

// ---- smart constructors (mirror crate::native::formula exactly) -------------

/// `++cons`: collapse two constants `[1 h]`/`[1 t]` to `[1 h t]`; else autocons.
pub fn cons(head: Formula, tail: Formula) -> Formula {
    if let (Formula::Quote(h), Formula::Quote(t)) = (&head, &tail) {
        return Formula::Quote(pair_leaf(h, t));
    }
    Formula::Cell(rc(head), rc(tail))
}

/// `++comb` composition, matching the noun check order exactly.
pub fn comb(mal: Formula, buz: Formula) -> Formula {
    // Check 1: mal = [0 a], a >= 1
    if let Some(a) = slot_small(&mal) {
        if a >= 1 {
            // 1a: buz = [0 b], b >= 1 → [0 peg(a,b)]
            if let Some(b) = slot_small(&buz) {
                if b >= 1 {
                    return Formula::Slot(Axis::Small(peg(a, b)));
                }
            }
            // 1b: buz = [2 [0 x] [0 y]] → [2 [0 peg(a,x)] [0 peg(a,y)]]
            if let Formula::Eval(p, q) = &buz {
                if let (Some(x), Some(y)) = (slot_small(p), slot_small(q)) {
                    if x >= 1 && y >= 1 {
                        return Formula::Eval(
                            rc(Formula::Slot(Axis::Small(peg(a, x)))),
                            rc(Formula::Slot(Axis::Small(peg(a, y)))),
                        );
                    }
                }
            }
            // 1 fallthrough → [7 mal buz]
            return Formula::Op {
                code: 7,
                args: vec![rc(mal), rc(buz)],
            };
        }
    }
    // Check 2: mal = [x [0 1]] → [8 x buz]
    if let Formula::Cell(h, t) = &mal {
        if is_slot_one(t) {
            return Formula::Op {
                code: 8,
                args: vec![Rc::clone(h), rc(buz)],
            };
        }
    }
    // Check 3: buz = [0 1] → mal
    if is_slot_one(&buz) {
        return mal;
    }
    Formula::Op {
        code: 7,
        args: vec![rc(mal), rc(buz)],
    }
}

/// `++cond`: const-true → yom, const-false → woq, `[0 0]` → pex, else `[6 …]`.
pub fn cond(pex: Formula, yom: Formula, woq: Formula) -> Formula {
    match quote_direct(&pex) {
        Some(0) => return yom,
        Some(1) => return woq,
        _ => {}
    }
    if is_slot_zero(&pex) {
        return pex;
    }
    Formula::Cond(rc(pex), rc(yom), rc(woq))
}

/// `++peg`: axis composition. Copied verbatim from `crate::native::formula`.
fn peg(a: u64, b: u64) -> u64 {
    if a == 1 {
        return b;
    }
    let b_path_width = 63 - b.leading_zeros() as u64;
    (a << b_path_width) + (b - (1u64 << b_path_width))
}

// ---- from_noun: parse a Nock formula noun into the native IR ----------------
//
// Used to prove IR completeness (round-trip `from_noun(f).to_noun() == f`) on
// real honk-emitted formulas before the construction port, and as a bridge that
// lets the native path consume not-yet-ported noun sub-formulas. Follows Nock's
// head-is-cell ⇒ autocons / head-is-atom ⇒ opcode rule. Hint kinds (`%fast`/
// `%note`/`%spot`) and op-12 all decode to a representation that re-emits the
// same `[11 …]`/`[12 …]` bytes — the semantic distinction is only needed when
// BUILDING from mint, not for representation/round-trip.
impl Formula {
    pub fn from_noun(noun: Noun, space: &NounSpace) -> Result<Formula> {
        let (head, tail) = noun_pair(noun, space)
            .map_err(|_| CompilerError::Noun("native IR: formula is not a cell".into()))?;
        // head is a cell ⇒ autocons [f g]
        if head.in_space(space).as_cell().is_ok() {
            return Ok(Formula::Cell(
                rc(Formula::from_noun(head, space)?),
                rc(Formula::from_noun(tail, space)?),
            ));
        }
        let op = head
            .in_space(space)
            .as_atom()
            .ok()
            .and_then(|a| a.as_u64().ok())
            .ok_or_else(|| CompilerError::Noun("native IR: opcode not a small atom".into()))?;
        let pair = |n: Noun| {
            noun_pair(n, space)
                .map_err(|_| CompilerError::Noun("native IR: bad opcode args".into()))
        };
        Ok(match op {
            0 => Formula::Slot(axis_from_noun(tail, space)?),
            1 => Formula::Quote(Leaf::from_noun(tail, space)),
            2 => {
                let (s, f) = pair(tail)?;
                Formula::Eval(
                    rc(Formula::from_noun(s, space)?),
                    rc(Formula::from_noun(f, space)?),
                )
            }
            3 | 4 => Formula::Op {
                code: op as u8,
                args: vec![rc(Formula::from_noun(tail, space)?)],
            },
            5 | 7 | 8 | 12 => {
                let (a, b) = pair(tail)?;
                Formula::Op {
                    code: op as u8,
                    args: vec![
                        rc(Formula::from_noun(a, space)?),
                        rc(Formula::from_noun(b, space)?),
                    ],
                }
            }
            6 => {
                let (p, qr) = pair(tail)?;
                let (q, r) = pair(qr)?;
                Formula::Cond(
                    rc(Formula::from_noun(p, space)?),
                    rc(Formula::from_noun(q, space)?),
                    rc(Formula::from_noun(r, space)?),
                )
            }
            9 => {
                let (axis, core) = pair(tail)?;
                Formula::Kick {
                    axis: axis_from_noun(axis, space)?,
                    core: rc(Formula::from_noun(core, space)?),
                }
            }
            10 => {
                let (edit, target) = pair(tail)?;
                let (axis, value) = pair(edit)?;
                Formula::Edit {
                    axis: axis_from_noun(axis, space)?,
                    value: rc(Formula::from_noun(value, space)?),
                    target: rc(Formula::from_noun(target, space)?),
                }
            }
            11 => {
                let (hint, body) = pair(tail)?;
                // Representation-only: all [11 …] decode to Dbug (re-emits the
                // same bytes). The jet/note/spot distinction is a build concern.
                Formula::Dbug {
                    spot: Leaf::from_noun(hint, space),
                    body: rc(Formula::from_noun(body, space)?),
                }
            }
            other => {
                return Err(CompilerError::Noun(format!(
                    "native IR: unsupported Nock opcode {other}"
                )))
            }
        })
    }
}

/// Decode a Nock axis atom into [`Axis`] (big atoms preserved as `Big`).
fn axis_from_noun(noun: Noun, space: &NounSpace) -> Result<Axis> {
    let atom = noun
        .in_space(space)
        .as_atom()
        .map_err(|_| CompilerError::Noun("native IR: axis not an atom".into()))?;
    match atom.as_u64() {
        Ok(v) => Ok(Axis::Small(v)),
        Err(_) => Ok(Axis::Big(Rc::new(BigUint::from_bytes_le(
            atom.as_ne_bytes(),
        )))),
    }
}

/// Build the constant pair leaf `[h t]` for the cons-collapse case.
fn pair_leaf(h: &Leaf, t: &Leaf) -> Leaf {
    let mut scratch: NounSlab = NounSlab::new();
    let hn = h.to_noun(&mut scratch);
    let tn = t.to_noun(&mut scratch);
    let pair = T(&mut scratch, &[hn, tn]);
    let space = scratch.noun_space();
    Leaf::from_noun(pair, &space)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::formula as nf;

    // Jam a noun by rooting it in its own slab.
    fn jam(mut slab: NounSlab, n: Noun) -> Vec<u8> {
        slab.set_root(n);
        slab.jam().to_vec()
    }

    fn native_jam(f: &Formula) -> Vec<u8> {
        let mut s: NounSlab = NounSlab::new();
        let n = f.to_noun(&mut s);
        jam(s, n)
    }

    // ---- to_noun encoding: each native node → exact Nock noun ----
    #[test]
    fn to_noun_encodes_primitive_opcodes() {
        // [0 5]
        let mut s: NounSlab = NounSlab::new();
        let expect = T(&mut s, &[D(0), D(5)]);
        assert_eq!(native_jam(&Formula::Slot(Axis::Small(5))), jam(s, expect));
        // [1 42]
        let mut s: NounSlab = NounSlab::new();
        let expect = T(&mut s, &[D(1), D(42)]);
        assert_eq!(
            native_jam(&Formula::Quote(Leaf::Direct(42))),
            jam(s, expect)
        );
        // [2 [0 1] [0 2]]
        let mut s: NounSlab = NounSlab::new();
        let l = T(&mut s, &[D(0), D(1)]);
        let r = T(&mut s, &[D(0), D(2)]);
        let expect = T(&mut s, &[D(2), l, r]);
        let f = Formula::Eval(
            rc(Formula::Slot(Axis::Small(1))),
            rc(Formula::Slot(Axis::Small(2))),
        );
        assert_eq!(native_jam(&f), jam(s, expect));
        // [6 [0 2] [1 0] [1 1]]
        let mut s: NounSlab = NounSlab::new();
        let p = T(&mut s, &[D(0), D(2)]);
        let y = T(&mut s, &[D(1), D(0)]);
        let w = T(&mut s, &[D(1), D(1)]);
        let expect = T(&mut s, &[D(6), p, y, w]);
        let f = Formula::Cond(
            rc(Formula::Slot(Axis::Small(2))),
            rc(Formula::Quote(Leaf::Direct(0))),
            rc(Formula::Quote(Leaf::Direct(1))),
        );
        assert_eq!(native_jam(&f), jam(s, expect));
        // [10 [3 [1 99]] [0 1]]
        let mut s: NounSlab = NounSlab::new();
        let val = T(&mut s, &[D(1), D(99)]);
        let edit = T(&mut s, &[D(3), val]);
        let tgt = T(&mut s, &[D(0), D(1)]);
        let expect = T(&mut s, &[D(10), edit, tgt]);
        let f = Formula::Edit {
            axis: Axis::Small(3),
            value: rc(Formula::Quote(Leaf::Direct(99))),
            target: rc(Formula::Slot(Axis::Small(1))),
        };
        assert_eq!(native_jam(&f), jam(s, expect));
    }

    #[test]
    fn to_noun_big_axis() {
        // An axis above u64 range round-trips as an indirect atom in [0 axis].
        let big = BigUint::from(1u8) << 80u32; // 2^80
        let mut s: NounSlab = NounSlab::new();
        let axn = Atom::from_bytes(&mut s, &big.to_bytes_le()).as_noun();
        let expect = T(&mut s, &[D(0), axn]);
        let f = Formula::Slot(Axis::Big(Rc::new(big)));
        assert_eq!(native_jam(&f), jam(s, expect));
    }

    // ---- smart-constructor parity vs the noun formula constructors ----
    // For each case: build native operands, take their to_noun as the noun
    // operands, then assert native-ctor(...).to_noun == noun-ctor(noun ops).
    fn check_comb(mal: Formula, buz: Formula) {
        let mut sn: NounSlab = NounSlab::new();
        let maln = mal_to_noun(&mal, &mut sn);
        let buzn = buz_to_noun(&buz, &mut sn);
        let noun_res = nf::comb(&mut sn, maln, buzn).expect("noun comb");
        let j_noun = jam(sn, noun_res);
        let j_native = native_jam(&comb(mal, buz));
        assert_eq!(j_native, j_noun, "comb parity");
    }
    fn mal_to_noun(f: &Formula, s: &mut NounSlab) -> Noun {
        f.to_noun(s)
    }
    fn buz_to_noun(f: &Formula, s: &mut NounSlab) -> Noun {
        f.to_noun(s)
    }

    #[test]
    fn comb_parity_all_branches() {
        // 1a: [0 a] o [0 b] → peg
        check_comb(Formula::Slot(Axis::Small(2)), Formula::Slot(Axis::Small(3)));
        // 1b: [0 a] o [2 [0 x] [0 y]]
        check_comb(
            Formula::Slot(Axis::Small(2)),
            Formula::Eval(
                rc(Formula::Slot(Axis::Small(2))),
                rc(Formula::Slot(Axis::Small(3))),
            ),
        );
        // 1 fallthrough: [0 a] o [1 c]
        check_comb(
            Formula::Slot(Axis::Small(2)),
            Formula::Quote(Leaf::Direct(7)),
        );
        // 2: [x [0 1]] o buz → [8 x buz]
        check_comb(
            Formula::Cell(
                rc(Formula::Quote(Leaf::Direct(5))),
                rc(Formula::Slot(Axis::Small(1))),
            ),
            Formula::Slot(Axis::Small(3)),
        );
        // 3: mal o [0 1] → mal
        check_comb(
            Formula::Quote(Leaf::Direct(9)),
            Formula::Slot(Axis::Small(1)),
        );
        // default: [1 c] o [1 d] → [7 …]
        check_comb(
            Formula::Quote(Leaf::Direct(1)),
            Formula::Quote(Leaf::Direct(2)),
        );
        // big axis skips peg (a is big → check 1 not taken)
        check_comb(
            Formula::Slot(Axis::Big(Rc::new(BigUint::from(1u8) << 70u32))),
            Formula::Slot(Axis::Small(3)),
        );
    }

    #[test]
    fn cons_parity() {
        let mut sn: NounSlab = NounSlab::new();
        // both const → [1 h t]
        let a = Formula::Quote(Leaf::Direct(3));
        let b = Formula::Quote(Leaf::Direct(4));
        let an = a.to_noun(&mut sn);
        let bn = b.to_noun(&mut sn);
        let noun_res = nf::cons(&mut sn, an, bn).expect("noun cons");
        let j_noun = jam(sn, noun_res);
        let j_native = native_jam(&cons(a, b));
        assert_eq!(j_native, j_noun, "cons collapse parity");

        // not both const → autocons
        let mut sn: NounSlab = NounSlab::new();
        let a = Formula::Slot(Axis::Small(2));
        let b = Formula::Quote(Leaf::Direct(4));
        let an = a.to_noun(&mut sn);
        let bn = b.to_noun(&mut sn);
        let noun_res = nf::cons(&mut sn, an, bn).expect("noun cons");
        let j_noun = jam(sn, noun_res);
        let j_native = native_jam(&cons(a, b));
        assert_eq!(j_native, j_noun, "cons autocons parity");
    }

    #[test]
    fn cond_parity() {
        let cases = [
            (Formula::Quote(Leaf::Direct(0)), true),  // const true → yom
            (Formula::Quote(Leaf::Direct(1)), false), // const false → woq
            (Formula::Slot(Axis::Small(0)), false),   // [0 0] → pex
            (Formula::Slot(Axis::Small(4)), false),   // default → [6 …]
        ];
        for (pex, _) in cases {
            let mut sn: NounSlab = NounSlab::new();
            let yom = Formula::Quote(Leaf::Direct(10));
            let woq = Formula::Quote(Leaf::Direct(20));
            let pexn = pex_clone(&pex).to_noun(&mut sn);
            let yomn = yom.to_noun(&mut sn);
            let woqn = woq.to_noun(&mut sn);
            let noun_res = nf::cond(&mut sn, pexn, yomn, woqn).expect("noun cond");
            let j_noun = jam(sn, noun_res);
            let yom = Formula::Quote(Leaf::Direct(10));
            let woq = Formula::Quote(Leaf::Direct(20));
            let j_native = native_jam(&cond(pex, yom, woq));
            assert_eq!(j_native, j_noun, "cond parity");
        }
    }

    fn pex_clone(f: &Formula) -> Formula {
        match f {
            Formula::Quote(Leaf::Direct(v)) => Formula::Quote(Leaf::Direct(*v)),
            Formula::Slot(Axis::Small(v)) => Formula::Slot(Axis::Small(*v)),
            _ => unreachable!("test pex shapes only"),
        }
    }

    // from_noun(f).to_noun() == f for raw Nock shapes (incl. ones the native
    // builders don't produce: unary ops, op-12, cell-payload hints, big axis).
    #[test]
    fn from_noun_roundtrips_raw_nock() {
        let builders: Vec<fn(&mut NounSlab) -> Noun> = vec![
            |s| T(s, &[D(0), D(7)]),  // [0 7] slot
            |s| T(s, &[D(1), D(42)]), // [1 42] quote atom
            |s| {
                let p = T(s, &[D(1), D(9)]);
                T(s, &[D(1), p])
            }, // [1 [1 9]] quote-of-cell
            |s| {
                let a = T(s, &[D(0), D(1)]);
                T(s, &[D(3), a])
            }, // [3 [0 1]] unary
            |s| {
                let a = T(s, &[D(0), D(1)]);
                T(s, &[D(4), a])
            }, // [4 [0 1]] unary
            |s| {
                let a = T(s, &[D(0), D(2)]);
                let b = T(s, &[D(0), D(3)]);
                T(s, &[D(5), a, b])
            }, // [5 a b]
            |s| {
                let p = T(s, &[D(0), D(2)]);
                let q = T(s, &[D(1), D(0)]);
                let r = T(s, &[D(1), D(1)]);
                T(s, &[D(6), p, q, r])
            }, // [6 p q r]
            |s| {
                let core = T(s, &[D(0), D(1)]);
                T(s, &[D(9), D(2), core])
            }, // [9 2 core]
            |s| {
                let val = T(s, &[D(1), D(5)]);
                let edit = T(s, &[D(3), val]);
                let tgt = T(s, &[D(0), D(1)]);
                T(s, &[D(10), edit, tgt])
            }, // [10 [3 [1 5]] [0 1]]
            |s| {
                let clue = T(s, &[D(1), D(2)]); // cell payload (dynamic hint)
                let body = T(s, &[D(0), D(1)]);
                T(s, &[D(11), clue, body])
            }, // [11 [1 2] [0 1]]
            |s| {
                let body = T(s, &[D(0), D(1)]);
                T(s, &[D(11), D(7), body])
            }, // [11 7 [0 1]] static hint
            |s| {
                let a = T(s, &[D(0), D(2)]);
                let b = T(s, &[D(0), D(3)]);
                T(s, &[D(12), a, b])
            }, // [12 a b]
            |s| {
                let l = T(s, &[D(0), D(2)]);
                let r = T(s, &[D(0), D(3)]);
                T(s, &[l, r])
            }, // [[0 2] [0 3]] autocons
        ];
        for (i, b) in builders.iter().enumerate() {
            let mut s: NounSlab = NounSlab::new();
            let orig = b(&mut s);
            let space = s.noun_space();
            let f = Formula::from_noun(orig, &space)
                .unwrap_or_else(|e| panic!("from_noun[{i}]: {e:?}"));
            let mut a: NounSlab = NounSlab::new();
            a.copy_into(orig, &space);
            let ja = a.jam().to_vec();
            let mut d: NounSlab = NounSlab::new();
            let r = f.to_noun(&mut d);
            d.set_root(r);
            let jr = d.jam().to_vec();
            assert_eq!(ja, jr, "round-trip case {i}");
        }
    }
}
