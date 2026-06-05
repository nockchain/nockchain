use nockvm::noun::Noun;
use noun_serde::NounDecodeError;

use crate::belt::{based_check, Belt};
use crate::tip5;

/// Errors raised while converting allocator-backed nouns into owned based-noun trees.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OwnedBasedNounError {
    /// A source atom did not fit into the owned `u64` representation used here.
    #[error("owned based noun atom exceeded u64 range")]
    AtomTooLarge,
    /// A source atom fit into `u64` but was outside the base field.
    #[error("owned based noun atom is not based: {0}")]
    AtomNotBased(u64),
    /// The source noun did not match the expected atom/cell structure.
    #[error("{0}")]
    Malformed(&'static str),
}

/// Maps owned based-noun conversion failures into the generic noun-serde decode error.
pub fn owned_based_noun_decode_error(err: OwnedBasedNounError) -> NounDecodeError {
    NounDecodeError::Custom(err.to_string())
}

/// Allocator-free owned representation of a noun tree whose atom leaves are all
/// base-field elements.
///
/// We need this type when we want noun structure semantics without holding on
/// to an allocator-backed `nockvm::Noun`, while also making the current
/// protocol invariant explicit: every atom leaf participating in these paths
/// must already be a valid [`Belt`].
///
/// That comes up in two places:
/// 1. canonical z-set/z-map ordering, which caches noun-derived ordering keys
/// 2. direct hashable helpers, which need leaf-sequence and dyck-shape hashing
///
/// By owning the tree in plain Rust boxes, callers can compute noun-structural
/// properties without threading a `NounAllocator` through every operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedBasedNoun {
    /// Atom leaf storing a validated base-field element.
    Atom(Belt),
    /// Cell node storing its owned head and tail children.
    Cell(Box<OwnedBasedNoun>, Box<OwnedBasedNoun>),
}

impl OwnedBasedNoun {
    /// Copies an allocator-backed noun into an owned tree that can outlive the
    /// source allocator.
    ///
    /// This rejects any atom that is either larger than `u64` or not a valid
    /// base-field element.
    pub fn from_noun(noun: Noun) -> Result<Self, OwnedBasedNounError> {
        if noun.is_atom() {
            let atom = noun
                .as_atom()
                .map_err(|_| OwnedBasedNounError::Malformed("expected atom"))?;
            let atom = atom
                .as_u64()
                .map_err(|_| OwnedBasedNounError::AtomTooLarge)?;
            if !based_check(atom) {
                return Err(OwnedBasedNounError::AtomNotBased(atom));
            }
            let atom = Belt(atom);
            Ok(Self::Atom(atom))
        } else {
            let cell = noun
                .as_cell()
                .map_err(|_| OwnedBasedNounError::Malformed("expected cell"))?;
            Ok(Self::cell(
                Self::from_noun(cell.head())?,
                Self::from_noun(cell.tail())?,
            ))
        }
    }

    /// Builds an owned atom noun directly from a validated base-field element.
    pub fn atom(atom: Belt) -> Self {
        Self::Atom(atom)
    }

    /// Builds an owned atom noun from a raw `u64`, rejecting values outside the
    /// base field.
    pub fn try_atom(atom: u64) -> Result<Self, OwnedBasedNounError> {
        if !based_check(atom) {
            return Err(OwnedBasedNounError::AtomNotBased(atom));
        }
        Ok(Self::atom(Belt(atom)))
    }

    /// Builds an owned cell noun from owned head and tail children.
    pub fn cell(head: Self, tail: Self) -> Self {
        Self::Cell(Box::new(head), Box::new(tail))
    }

    /// Builds the right-associated tuple noun for a slice of raw atoms.
    ///
    /// Every atom must already be based. For example, `[1, 2, 3]` becomes
    /// `[1 [2 3]]`. The empty tuple is represented as the null atom `0`,
    /// matching the existing hashable helper behavior.
    pub fn tuple_atoms(atoms: &[u64]) -> Result<Self, OwnedBasedNounError> {
        let mut iter = atoms.iter().rev();
        let Some(&last) = iter.next() else {
            return Self::try_atom(0);
        };
        let mut noun = Self::try_atom(last)?;
        for &atom in iter {
            noun = Self::cell(Self::try_atom(atom)?, noun);
        }
        Ok(noun)
    }

    /// Builds a proper Hoon list terminated by the based null atom `0`.
    pub fn list(items: Vec<Self>) -> Self {
        items
            .into_iter()
            .rev()
            .fold(Self::atom(Belt(0)), |tail, head| Self::cell(head, tail))
    }

    /// Counts the number of atom leaves in the noun tree.
    ///
    /// Tip5 `hash-noun-varlen` prefixes the flattened leaf stream with this
    /// count, so we cache it by traversal when producing the input belt list.
    pub fn leaf_count(&self) -> usize {
        match self {
            Self::Atom(_) => 1,
            Self::Cell(left, right) => left.leaf_count() + right.leaf_count(),
        }
    }

    /// Appends the noun's leaf-sequence encoding to `out`.
    ///
    /// This is the flattened left-to-right list of atom leaves used by
    /// `hash-noun-varlen`.
    pub fn push_leaf_sequence(&self, out: &mut Vec<Belt>) {
        match self {
            Self::Atom(atom) => out.push(*atom),
            Self::Cell(left, right) => {
                left.push_leaf_sequence(out);
                right.push_leaf_sequence(out);
            }
        }
    }

    /// Appends the noun's dyck-shape encoding to `out`.
    ///
    /// Cells contribute `0` before the left subtree and `1` before the right
    /// subtree, while atoms contribute nothing.
    pub fn push_dyck(&self, out: &mut Vec<Belt>) {
        match self {
            Self::Atom(_) => {}
            Self::Cell(left, right) => {
                out.push(Belt(0));
                left.push_dyck(out);
                out.push(Belt(1));
                right.push_dyck(out);
            }
        }
    }
}

/// Computes the tip5 `hash-noun-varlen` digest for an owned noun tree.
///
/// This mirrors the jet path's noun hashing, but operates on an allocator-free
/// owned tree so higher-level code can hash noun structure directly.
pub fn hash_owned_based_noun_varlen(noun: &OwnedBasedNoun) -> [u64; 5] {
    let mut input = Vec::with_capacity(1 + noun.leaf_count() * 2);
    input.push(Belt(noun.leaf_count() as u64));
    noun.push_leaf_sequence(&mut input);
    noun.push_dyck(&mut input);
    tip5::hash::hash_varlen(&mut input)
}

#[cfg(test)]
mod tests {
    use ibig::ubig;
    use nockvm::mem::NockStack;
    use nockvm::noun::Atom;

    use super::{OwnedBasedNoun, OwnedBasedNounError};
    use crate::belt::{Belt, PRIME};

    #[test]
    fn from_noun_accepts_direct_based_atoms() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);
        let noun = Atom::new(&mut stack, 7).as_noun();

        assert_eq!(
            OwnedBasedNoun::from_noun(noun),
            Ok(OwnedBasedNoun::Atom(Belt(7)))
        );
    }

    #[test]
    fn from_noun_accepts_indirect_based_atoms_that_fit_u64() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);
        let noun = Atom::new(&mut stack, PRIME - 1).as_noun();
        let atom = noun.as_atom().expect("noun should be atom");
        assert!(atom.is_indirect());

        assert_eq!(
            OwnedBasedNoun::from_noun(noun),
            Ok(OwnedBasedNoun::Atom(Belt(PRIME - 1)))
        );
    }

    #[test]
    fn from_noun_rejects_non_based_atoms() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);
        let noun = Atom::new(&mut stack, PRIME).as_noun();

        assert_eq!(
            OwnedBasedNoun::from_noun(noun),
            Err(OwnedBasedNounError::AtomNotBased(PRIME))
        );
    }

    #[test]
    fn from_noun_rejects_atoms_larger_than_u64() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);
        let big = ubig!(1) << 80;
        let noun = Atom::from_ubig(&mut stack, &big).as_noun();

        assert_eq!(
            OwnedBasedNoun::from_noun(noun),
            Err(OwnedBasedNounError::AtomTooLarge)
        );
    }
}
