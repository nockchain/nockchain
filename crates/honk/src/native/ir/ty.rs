//! Native Hoon type IR — Phase 1 BOUNDARY form (plan §3.3, the Type-IR boundary
//! slice).
//!
//! `Type::to_noun`/`from_noun` represent and re-emit honk's type nouns
//! byte-for-byte (proven by round-trip on the real compiled prelude + kernel
//! types), the type analogue of the Formula-IR boundary.
//!
//! BOUNDARY vs TARGET: this form makes the type SKELETON native — recursive type
//! children (`Cell` head/tail, `Core` payload, `Face` inner, `Hold` subject,
//! `Hint` payload) are `Rc<Type>` — while carrying the complex / leaf
//! sub-structures faithfully as provenanced [`Leaf`]s: the `%atom` aura+bits, the
//! `%face` tool, the `%hint` `[inner note]` head, the `%hold` gene (AST), the
//! `%core` coil (garb+context+battery seminoun), and the `%fork` mug-ordered
//! treap. Phase 2 nativizes those carried parts (aura→symbol, coil→native Coil +
//! lazy battery, treap→canonical set, gene→native AST) so they hash-cons — that
//! is where the memory win lands. The carried leaves are exactly the Phase-2
//! nativization targets (see `core.rs`/`intern.rs` for the target shapes).
//!
//! Normalization (RT-06: `ty_face(void)→void`, `ty_core(void)→void`,
//! `ty_hint(void|noun)→…`) happens at CONSTRUCTION, so real type nouns are
//! already normalized; `from_noun` only decodes what exists and `to_noun`
//! re-emits it. Normalization is a Phase-2 construction concern, not a boundary
//! one.
#![allow(dead_code)]

use std::rc::Rc;

use nockapp::noun::slab::NounSlab;
use nockvm::noun::{Noun, NounSpace, D, T};

use super::leaf::Leaf;
use crate::errors::{CompilerError, Result};
use crate::native::noun::{atom_to_string, noun_pair, term_to_noun};
use crate::native::ut::types::{Poly, Vair};

/// A native `%core` garb — the head of a coil, `[nym poly vair]`.
///
/// nym = `0` (anonymous, `None`) or `[0 term]` (named, `Some(term)`); poly =
/// `%wet`|`%dry`; vair = `%gold`|`%iron`|`%lead`|`%zinc`. Replaces the jammed
/// `Leaf` the coil head used to carry: built directly from `mint`'s parts and
/// re-emitted byte-identically (mirrors `garb_from_parts`), so the core type
/// noun round-trips unchanged while never jamming/cueing the tiny garb.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct Garb {
    pub nym: Option<String>,
    pub poly: Poly,
    pub vair: Vair,
}

/// Build a native [`Garb`] from `mint`'s parts (replaces `garb_from_parts`'
/// noun building where it feeds `cons_core`).
pub fn garb_native(name: Option<&str>, poly: Poly, vair: Vair) -> Garb {
    Garb {
        nym: name.map(str::to_string),
        poly,
        vair,
    }
}

impl Garb {
    /// Re-emit the garb noun `[nym poly vair]` BYTE-IDENTICALLY to
    /// `garb_from_parts` (nym = `0` | `[0 term]`; poly/vair via `term_to_noun`).
    pub fn to_noun(&self, dst: &mut NounSlab) -> Noun {
        let name_noun = match &self.nym {
            None => D(0),
            Some(term) => {
                let term_noun = term_to_noun(dst, term);
                T(dst, &[D(0), term_noun])
            }
        };
        let poly_noun = term_to_noun(
            dst,
            match self.poly {
                Poly::Wet => "wet",
                Poly::Dry => "dry",
            },
        );
        let vair_noun = term_to_noun(
            dst,
            match self.vair {
                Vair::Gold => "gold",
                Vair::Iron => "iron",
                Vair::Lead => "lead",
                Vair::Zinc => "zinc",
            },
        );
        T(dst, &[name_noun, poly_noun, vair_noun])
    }

    /// Decode a garb noun `[nym poly vair]` into a native [`Garb`] (mirrors
    /// `garb_parts`/`garb_poly`/`garb_vair`; errors as those do).
    pub fn from_noun(garb: Noun, space: &NounSpace) -> Result<Garb> {
        let cell = garb
            .in_space(space)
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("garb not cell: {err}")))?;
        let tail = cell
            .tail()
            .as_cell()
            .map_err(|err| CompilerError::Decode(format!("garb missing tail: {err}")))?;
        let nym_noun = cell.head().noun();
        let poly_noun = tail.head().noun();
        let vair_noun = tail.tail().noun();
        // nym = 0 (None) | [0 term] (Some).
        let nym = if nym_noun.in_space(space).as_atom().is_ok() {
            None
        } else {
            let nym_cell = nym_noun
                .in_space(space)
                .as_cell()
                .map_err(|err| CompilerError::Decode(format!("garb nym not cell: {err}")))?;
            let term_atom = nym_cell
                .tail()
                .as_atom()
                .map_err(|err| CompilerError::Decode(format!("garb nym term not atom: {err}")))?;
            Some(
                atom_to_string(term_atom)
                    .map_err(|err| CompilerError::Decode(format!("garb nym: {err}")))?,
            )
        };
        let poly_atom = poly_noun
            .in_space(space)
            .as_atom()
            .map_err(|err| CompilerError::Decode(format!("garb poly not atom: {err}")))?;
        let poly_name = atom_to_string(poly_atom)
            .map_err(|err| CompilerError::Decode(format!("garb poly: {err}")))?;
        let poly = match poly_name.as_str() {
            "wet" => Poly::Wet,
            "dry" => Poly::Dry,
            _ => {
                return Err(CompilerError::UnsupportedExpr(format!(
                    "native mint: garb poly {poly_name}"
                )))
            }
        };
        let vair_atom = vair_noun
            .in_space(space)
            .as_atom()
            .map_err(|err| CompilerError::Decode(format!("garb vair not atom: {err}")))?;
        let vair_name = atom_to_string(vair_atom)
            .map_err(|err| CompilerError::Decode(format!("garb vair: {err}")))?;
        let vair = match vair_name.as_str() {
            "gold" => Vair::Gold,
            "iron" => Vair::Iron,
            "lead" => Vair::Lead,
            "zinc" => Vair::Zinc,
            _ => {
                return Err(CompilerError::UnsupportedExpr(format!(
                    "native mint: garb vair {vair_name}"
                )))
            }
        };
        Ok(Garb { nym, poly, vair })
    }

    /// Return a copy with `vair` replaced (mirrors `garb_with_vair`).
    pub fn with_vair(&self, vair: Vair) -> Garb {
        Garb {
            nym: self.nym.clone(),
            poly: self.poly,
            vair,
        }
    }
}

/// A Hoon compiler type (Phase-1 boundary form; see module docs).
#[derive(Debug)]
pub enum Type {
    Void,
    Noun,
    /// `[%atom aura bits]` — aura + bits carried as leaves.
    Atom {
        aura: Leaf,
        bits: Leaf,
    },
    /// `[%cell head tail]` — both native.
    Cell(Rc<Type>, Rc<Type>),
    /// `[%core payload coil]` — payload native; coil = `[garb [context rest]]`.
    /// Phase 2: the coil's `context` (the deepening subject) is a SHARED native
    /// `Rc<Type>` child so it is never jammed/copied on the hot path; only the
    /// tiny `garb` and the bounded `rest` (battery seminoun + tomes) stay carried
    /// as leaves.
    Core {
        payload: Rc<Type>,
        garb: Garb,
        context: Rc<Type>,
        rest: Leaf,
    },
    /// `[%face tool inner]` — inner native; tool carried.
    Face {
        tool: Leaf,
        inner: Rc<Type>,
    },
    /// `[%hint [inner note] payload]` — payload native; `[inner note]` carried.
    Hint {
        head: Leaf,
        payload: Rc<Type>,
    },
    /// `[%fork set]` — the mug-ordered treap carried (Phase-2 nativizes to a
    /// canonical set + treap re-emission, RT-07).
    Fork {
        set: Leaf,
    },
    /// `[%hold sut gen]` — sut native; gene (AST) carried.
    Hold {
        subject: Rc<Type>,
        gene: Leaf,
    },
}

/// `@tas` cord = little-endian byte packing into an atom. honk builds type tags
/// via `term_to_noun` (same encoding); all nine tags are ≤ 4 bytes.
pub(crate) fn tas(s: &str) -> u64 {
    let mut v = 0u64;
    for (i, b) in s.bytes().enumerate() {
        v |= (b as u64) << (i * 8);
    }
    v
}

impl Type {
    pub fn to_noun(&self, dst: &mut NounSlab) -> Noun {
        match self {
            Type::Void => D(tas("void")),
            Type::Noun => D(tas("noun")),
            Type::Atom { aura, bits } => {
                let a = aura.to_noun(dst);
                let b = bits.to_noun(dst);
                T(dst, &[D(tas("atom")), a, b])
            }
            Type::Cell(h, t) => {
                let hn = h.to_noun(dst);
                let tn = t.to_noun(dst);
                T(dst, &[D(tas("cell")), hn, tn])
            }
            Type::Core {
                payload,
                garb,
                context,
                rest,
            } => {
                let p = payload.to_noun(dst);
                // Rebuild the coil noun = [garb [context rest]] (mirrors
                // coil_from_parts: [garb [context rest]]). The garb noun is built
                // BYTE-IDENTICALLY to garb_from_parts.
                let g = garb.to_noun(dst);
                let ctx = context.to_noun(dst);
                let r = rest.to_noun(dst);
                let tail = T(dst, &[ctx, r]);
                let coil = T(dst, &[g, tail]);
                T(dst, &[D(tas("core")), p, coil])
            }
            Type::Face { tool, inner } => {
                let tl = tool.to_noun(dst);
                let inn = inner.to_noun(dst);
                T(dst, &[D(tas("face")), tl, inn])
            }
            Type::Hint { head, payload } => {
                let h = head.to_noun(dst);
                let p = payload.to_noun(dst);
                T(dst, &[D(tas("hint")), h, p])
            }
            Type::Fork { set } => {
                let s = set.to_noun(dst);
                T(dst, &[D(tas("fork")), s])
            }
            Type::Hold { subject, gene } => {
                let s = subject.to_noun(dst);
                let g = gene.to_noun(dst);
                T(dst, &[D(tas("hold")), s, g])
            }
        }
    }

    pub fn from_noun(noun: Noun, space: &NounSpace) -> Result<Type> {
        // %void / %noun are bare atom cords.
        if let Ok(atom) = noun.in_space(space).as_atom() {
            if atom.eq_bytes(b"void") {
                return Ok(Type::Void);
            }
            if atom.eq_bytes(b"noun") {
                return Ok(Type::Noun);
            }
            return Err(CompilerError::Decode(
                "native type IR: unknown atom type tag".into(),
            ));
        }
        let (tag, tail) = noun_pair(noun, space)
            .map_err(|_| CompilerError::Decode("native type IR: type not a cell".into()))?;
        let tag = tag
            .in_space(space)
            .as_atom()
            .map_err(|_| CompilerError::Decode("native type IR: type tag not atom".into()))?;
        // tail-pair helper for the [a b] tails
        let pair = |n: Noun| {
            noun_pair(n, space)
                .map_err(|_| CompilerError::Decode("native type IR: bad tail".into()))
        };
        Ok(if tag.eq_bytes(b"atom") {
            let (aura, bits) = pair(tail)?;
            Type::Atom {
                aura: Leaf::from_noun(aura, space),
                bits: Leaf::from_noun(bits, space),
            }
        } else if tag.eq_bytes(b"cell") {
            let (h, t) = pair(tail)?;
            Type::Cell(
                rc(Type::from_noun(h, space)?),
                rc(Type::from_noun(t, space)?),
            )
        } else if tag.eq_bytes(b"core") {
            let (payload, coil) = pair(tail)?;
            // coil = [garb [context rest]]
            let (garb, coil_tail) = pair(coil)?;
            let (context, rest) = pair(coil_tail)?;
            Type::Core {
                payload: rc(Type::from_noun(payload, space)?),
                garb: Garb::from_noun(garb, space)?,
                context: rc(Type::from_noun(context, space)?),
                rest: Leaf::from_noun(rest, space),
            }
        } else if tag.eq_bytes(b"face") {
            let (tool, inner) = pair(tail)?;
            Type::Face {
                tool: Leaf::from_noun(tool, space),
                inner: rc(Type::from_noun(inner, space)?),
            }
        } else if tag.eq_bytes(b"hint") {
            let (head, payload) = pair(tail)?;
            Type::Hint {
                head: Leaf::from_noun(head, space),
                payload: rc(Type::from_noun(payload, space)?),
            }
        } else if tag.eq_bytes(b"fork") {
            Type::Fork {
                set: Leaf::from_noun(tail, space),
            }
        } else if tag.eq_bytes(b"hold") {
            let (subject, gene) = pair(tail)?;
            Type::Hold {
                subject: rc(Type::from_noun(subject, space)?),
                gene: Leaf::from_noun(gene, space),
            }
        } else {
            return Err(CompilerError::Decode(
                "native type IR: unknown type tag".into(),
            ));
        })
    }
}

fn rc(t: Type) -> Rc<Type> {
    Rc::new(t)
}

#[cfg(test)]
mod tests {
    use nockvm::noun::NounAllocator;

    use super::*;

    fn jam(mut slab: NounSlab, n: Noun) -> Vec<u8> {
        slab.set_root(n);
        slab.jam().to_vec()
    }

    // from_noun(t).to_noun() == t for hand-built type nouns covering all 9 tags.
    #[test]
    fn type_from_noun_roundtrips() {
        // Build representative type nouns (already-normalized shapes).
        let builders: Vec<fn(&mut NounSlab) -> Noun> = vec![
            |_s| D(tas("void")),
            |_s| D(tas("noun")),
            |s| T(s, &[D(tas("atom")), D(tas("ud")), D(0)]), // [%atom %ud ~]
            |s| {
                let val = T(s, &[D(0), D(42)]); // [~ 42] unit
                T(s, &[D(tas("atom")), D(tas("f")), val])
            },
            |s| {
                let a = T(s, &[D(tas("atom")), D(tas("ud")), D(0)]);
                let b = D(tas("noun"));
                T(s, &[D(tas("cell")), a, b])
            },
            |s| {
                // [%face %x [%atom %ud ~]]
                let inner = T(s, &[D(tas("atom")), D(tas("ud")), D(0)]);
                T(s, &[D(tas("face")), D(tas("x")), inner])
            },
            |s| {
                // [%hint [inner note] payload]
                let inner = D(0);
                let note = D(tas("fast"));
                let head = T(s, &[inner, note]);
                let payload = D(tas("noun"));
                T(s, &[D(tas("hint")), head, payload])
            },
            |s| {
                // [%hold sut gen]
                let sut = D(tas("noun"));
                let gen = T(s, &[D(1), D(2)]);
                T(s, &[D(tas("hold")), sut, gen])
            },
            |s| {
                // [%core payload coil] — coil = [garb [context rest]],
                // garb = [nym poly vair] = [0 %dry %gold] (anonymous dry gold core).
                let payload = T(s, &[D(tas("atom")), D(tas("ud")), D(0)]);
                let ctx = D(tas("noun"));
                let tomes = D(0);
                let garb = T(s, &[D(0), D(tas("dry")), D(tas("gold"))]);
                let tail = T(s, &[ctx, tomes]);
                let coil = T(s, &[garb, tail]);
                T(s, &[D(tas("core")), payload, coil])
            },
            |s| {
                // [%core payload coil] with a NAMED garb = [[0 %foo] %wet %iron].
                let payload = T(s, &[D(tas("atom")), D(tas("ud")), D(0)]);
                let ctx = D(tas("noun"));
                let tomes = D(0);
                let nym = T(s, &[D(0), D(tas("foo"))]);
                let garb = T(s, &[nym, D(tas("wet")), D(tas("iron"))]);
                let tail = T(s, &[ctx, tomes]);
                let coil = T(s, &[garb, tail]);
                T(s, &[D(tas("core")), payload, coil])
            },
            |s| {
                // [%fork set] — opaque treap (a 1-node-ish set noun)
                let opt = T(s, &[D(tas("atom")), D(tas("ud")), D(0)]);
                let lr = T(s, &[D(0), D(0)]);
                let set = T(s, &[opt, lr]);
                T(s, &[D(tas("fork")), set])
            },
        ];
        for (i, b) in builders.iter().enumerate() {
            let mut s: NounSlab = NounSlab::new();
            let orig = b(&mut s);
            let space = s.noun_space();
            let t = Type::from_noun(orig, &space)
                .unwrap_or_else(|e| panic!("type from_noun[{i}]: {e:?}"));
            let mut a: NounSlab = NounSlab::new();
            a.copy_into(orig, &space);
            let ja = a.jam().to_vec();
            let mut d: NounSlab = NounSlab::new();
            let r = t.to_noun(&mut d);
            let jr = jam(d, r);
            assert_eq!(ja, jr, "type round-trip case {i}");
        }
    }
}
