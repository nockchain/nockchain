//! Noun leaves carried by the native IR.
//!
//! Types and formulas carry noun leaves: quoted constants (`[1 const]`), hint
//! clues, dbug spots, and the non-recursive parts of types. A leaf is a small
//! atom inline, owned jam bytes (the slab-independent form used by the
//! round-trip checks), or a raw noun in the compile slab (the live compile
//! path). `to_noun` copies the leaf into a destination slab. See
//! `docs/native-compiler/PHASE0-PROVENANCE-DESIGN.md`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use bytes::Bytes;
use nockapp::noun::slab::NounSlab;
use nockvm::noun::{Atom, Noun, NounAllocator, NounSpace};

use crate::native::identity::*;

/// A noun leaf carried by a native type or formula.
///
/// `Jammed` and `Noun` cache a hash (content hash or mug) computed once at
/// creation, so hash-consing the types and formulas that carry them is O(1) per
/// leaf instead of rehashing large leaves on every intern. `Jammed` equality
/// short-circuits on `Arc` pointer identity and compares bytes only on a hash
/// match across distinct allocations.
#[derive(Clone, Debug)]
pub enum Leaf {
    /// An atom that fits in a `u64`, stored inline.
    Direct(u64),
    /// Anything larger (big atoms, cells) as owned jam bytes plus a cached
    /// content hash; `to_noun` cues it into the destination slab.
    Jammed(Arc<[u8]>, JamHash),
    /// A raw noun plus its cached mug, carried without a jam/cue round trip. The
    /// compile slab never recycles the noun's address, so it stays valid for the
    /// whole compile. Built only by [`Leaf::from_noun_raw`] on the live compile
    /// path; the round-trip path ([`Leaf::from_noun`]) uses `Jammed`. The two
    /// never share an intern table, so the cross-variant `PartialEq => false`
    /// case never arises.
    Noun(Noun, NounMug),
}

impl Leaf {
    /// Capture `noun` (resolved in `space`) as an owned leaf: atoms `<= u64`
    /// become `Direct`, everything larger is `Jammed`. Slab-independent; used by
    /// the round-trip checks. The live compile path uses [`Leaf::from_noun_raw`].
    pub fn from_noun(noun: Noun, space: &NounSpace) -> Self {
        if let Ok(atom) = noun.in_space(space).as_atom() {
            if let Ok(v) = atom.as_u64() {
                return Leaf::Direct(v);
            }
        }
        // Larger atoms and cells: jam through a scratch slab to own the bytes.
        let mut scratch: NounSlab = NounSlab::new();
        scratch.copy_into(noun, space);
        let bytes: Arc<[u8]> = Arc::from(&scratch.jam()[..]);
        let mut hasher = DefaultHasher::new();
        bytes[..].hash(&mut hasher);
        Leaf::Jammed(bytes, JamHash(hasher.finish()))
    }

    /// Capture `noun` for the live compile path: atoms `<= u64` become `Direct`,
    /// everything larger is carried as a raw `Leaf::Noun` without a jam/cue round
    /// trip. The compile slab never recycles the noun's address, so the raw noun
    /// stays valid for the whole compile.
    pub fn from_noun_raw(noun: Noun, space: &NounSpace) -> Self {
        if let Ok(atom) = noun.in_space(space).as_atom() {
            if let Ok(v) = atom.as_u64() {
                return Leaf::Direct(v);
            }
        }
        // Carry the live noun as-is; cache its mug as the hash bucket.
        Leaf::Noun(noun, NounMug(crate::native::noun::slab_mug(noun, space)))
    }

    /// Materialize the leaf into `dst` via a checked copy (no foreign pointer).
    pub fn to_noun(&self, dst: &mut NounSlab) -> Noun {
        match self {
            Leaf::Direct(v) => Atom::new(dst, *v).as_noun(),
            Leaf::Jammed(bytes, _) => {
                let mut scratch: NounSlab = NounSlab::new();
                let cued = scratch
                    .cue_into(Bytes::copy_from_slice(bytes))
                    .expect("leaf jam bytes must cue");
                dst.copy_into(cued, &scratch.noun_space())
            }
            // Copy into a possibly different slab. `live_leaf_to_noun` skips the
            // copy because the noun already lives in the compile slab.
            Leaf::Noun(n, _) => dst.copy_into(*n, &NounSpace::empty()),
        }
    }
}

impl PartialEq for Leaf {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Leaf::Direct(a), Leaf::Direct(b)) => a == b,
            (Leaf::Jammed(a, ha), Leaf::Jammed(b, hb)) => {
                ha == hb && (Arc::ptr_eq(a, b) || a[..] == b[..])
            }
            // Mug pre-filter, then an exact structural compare: the same
            // equivalence jam equality gives for `Jammed`. Cross-variant pairs
            // never occur within one intern table.
            (Leaf::Noun(n1, m1), Leaf::Noun(n2, m2)) => {
                m1 == m2
                    && crate::native::noun::noun_eq(*n1, *n2, &NounSpace::empty()).unwrap_or(false)
            }
            _ => false,
        }
    }
}
impl Eq for Leaf {}

impl Hash for Leaf {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Leaf::Direct(v) => {
                0u8.hash(state);
                v.hash(state);
            }
            Leaf::Jammed(_, h) => {
                1u8.hash(state);
                h.hash(state);
            }
            Leaf::Noun(_, mug) => {
                2u8.hash(state);
                mug.hash(state);
            }
        }
    }
}
