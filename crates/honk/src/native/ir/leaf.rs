//! Provenanced owned leaves for the native IR (plan §3.9, RT-04).
//!
//! Formulas (and, for typed Dynock, types) carry noun leaves: quoted constants
//! (`[1 const]`), hint clues, dbug spot tuples. A **bare `Noun`** leaf is an
//! alien-pointer hazard, so the IR carries an owned, provenance-free
//! representation: a small direct atom inline, or owned jam bytes for anything
//! larger. `to_noun` materializes the leaf into a destination slab through a
//! checked copy (never splicing a foreign pointer). The production copy-cache
//! lives in `docs/native-compiler/PHASE0-PROVENANCE-DESIGN.md`; this is the
//! Phase-1 shadow form.
#![allow(dead_code)]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use bytes::Bytes;
use nockapp::noun::slab::NounSlab;
use nockvm::noun::{Atom, Noun, NounAllocator, NounSpace};

/// An owned, provenance-safe noun leaf.
///
/// `Jammed` caches a content hash computed once at creation, so hash-consing the
/// types/formulas that CARRY these leaves (core coil, fork set, etc.) is O(1) per
/// leaf instead of re-hashing the (potentially large) bytes on every intern —
/// without this, interning deepened types with big coils is O(N^2). `Eq`
/// short-circuits on `Arc` pointer identity (the common case, since `native_of`
/// memoizes leaves by source-noun identity), falling back to a content compare
/// only on a hash match across distinct allocations.
#[derive(Clone, Debug)]
pub enum Leaf {
    /// A direct (≤ 63-bit) atom, stored inline.
    Direct(u64),
    /// Anything larger (big atoms, cells) as owned jam bytes + a cached content
    /// hash — provenance-free, cued into the destination slab by `to_noun`.
    Jammed(Arc<[u8]>, u64),
    /// A RAW noun + cached mug. Carried WITHOUT jam/cue round-trips — the
    /// jam-elimination win. Safe because the compile slab never recycles the
    /// address the noun points at (the frame arena was retired), so the noun
    /// stays live for the whole compile. Built only via [`Leaf::from_noun_raw`]
    /// on the LIVE compile path; the oracle path ([`Leaf::from_noun`]) uses
    /// `Jammed`. The two never share an interned table within one compile, so the
    /// cross-variant `PartialEq => false` is never exercised.
    Noun(Noun, u32),
}

impl Leaf {
    /// Capture `noun` (resolved in `space`) as an owned, provenance-free leaf:
    /// atoms `<= u64` become `Direct`, everything larger is `Jammed` (owned jam
    /// bytes). The ORACLE path — slab-agnostic, used by `Type::from_noun` and the
    /// round-trip oracle. The LIVE compile path uses [`Leaf::from_noun_raw`].
    pub fn from_noun(noun: Noun, space: &NounSpace) -> Self {
        if let Ok(atom) = noun.in_space(space).as_atom() {
            if let Ok(v) = atom.as_u64() {
                return Leaf::Direct(v);
            }
        }
        // Larger / cell: jam through a scratch slab so we own the bytes.
        let mut scratch: NounSlab = NounSlab::new();
        scratch.copy_into(noun, space);
        let bytes: Arc<[u8]> = Arc::from(&scratch.jam()[..]);
        let mut hasher = DefaultHasher::new();
        bytes[..].hash(&mut hasher);
        Leaf::Jammed(bytes, hasher.finish())
    }

    /// Capture `noun` as a LIVE-path leaf: atoms `<= u64` become `Direct`,
    /// everything larger is carried as a raw `Leaf::Noun` (no jam/cue round-trip
    /// — the jam-elimination win). Safe because the compile slab never recycles
    /// the noun's address (the frame arena was retired), so the raw noun stays
    /// live for the whole compile.
    pub fn from_noun_raw(noun: Noun, space: &NounSpace) -> Self {
        if let Ok(atom) = noun.in_space(space).as_atom() {
            if let Ok(v) = atom.as_u64() {
                return Leaf::Direct(v);
            }
        }
        // Carry the live noun as-is; cache its mug as the hash bucket.
        Leaf::Noun(noun, crate::native::noun::slab_mug(noun, space))
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
            // Generic/oracle path: copy into a possibly-different slab. The LIVE
            // path (`live_leaf_to_noun`) returns the noun as-is (no copy) since
            // it is already `dst`-resident — that is the cue elimination.
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
            // Mug pre-filter then the EXACT structural compare — the same
            // equivalence jam-equality gives for `Jammed`, so interning collapses
            // `Noun` leaves to the identical canonical set. Cross-variant pairs
            // never occur within one compile (raw is compile-wide-constant).
            (Leaf::Noun(n1, m1), Leaf::Noun(n2, m2)) => {
                m1 == m2
                    && crate::native::noun::noun_eq(*n1, *n2, &NounSpace::empty())
                        .unwrap_or(false)
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
