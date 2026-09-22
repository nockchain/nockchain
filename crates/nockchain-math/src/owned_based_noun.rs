use nockvm::noun::{Noun, NounAllocator, NounSpace};
use noun_serde::{NounDecodeError, NounEncode};

use crate::belt::{based_check, Belt};
use crate::tip5;

/// Upper bound on owned nodes materialized by [`OwnedBasedNoun::from_noun`].
/// A jam DAG with structural sharing decodes in `O(input)` but implies an
/// exponentially larger logical tree; without this budget a ~60-byte crafted
/// noun would allocate billions of nodes.
pub const MAX_OWNED_BASED_NOUN_NODES: usize = 1 << 20;

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
    /// The source noun expanded past the owned-node budget. A jam DAG with
    /// structural sharing can imply an exponentially larger logical tree, so
    /// conversion is bounded instead of materializing every node.
    #[error("owned based noun exceeds the node budget of {0}")]
    TooLarge(usize),
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

impl Drop for OwnedBasedNoun {
    fn drop(&mut self) {
        // Box children drop recursively, so a deep spine (attacker-chosen
        // via wallet RPC nouns, or a long byte-tape note) would overflow
        // the dropping thread's stack. Take children out of their boxes
        // onto a worklist instead; every popped value is then either an
        // atom (whose drop is a no-op) or a cell already flattened to
        // trivial children, so no drop re-enters with work to do.
        let mut worklist: Vec<Box<Self>> = Vec::new();
        if let Self::Cell(head, tail) = self {
            worklist.push(std::mem::replace(head, Box::new(Self::Atom(Belt(0)))));
            worklist.push(std::mem::replace(tail, Box::new(Self::Atom(Belt(0)))));
        }
        while let Some(mut boxed) = worklist.pop() {
            if let Self::Cell(head, tail) = &mut *boxed {
                worklist.push(std::mem::replace(head, Box::new(Self::Atom(Belt(0)))));
                worklist.push(std::mem::replace(tail, Box::new(Self::Atom(Belt(0)))));
            }
        }
    }
}

impl OwnedBasedNoun {
    pub fn from_noun(noun: Noun, space: &NounSpace) -> Result<Self, OwnedBasedNounError> {
        Self::from_noun_with_budget(noun, space, MAX_OWNED_BASED_NOUN_NODES)
    }

    /// Budget-bounded variant of [`OwnedBasedNoun::from_noun`].
    ///
    /// The walk is iterative: a deep source noun (attacker-chosen via wallet
    /// RPC hax preimages and note blobs) must not recurse on the worker
    /// thread's stack, and a structurally shared DAG must not expand into
    /// its exponentially larger logical tree.
    pub fn from_noun_with_budget(
        noun: Noun,
        space: &NounSpace,
        max_nodes: usize,
    ) -> Result<Self, OwnedBasedNounError> {
        enum Task {
            Convert(Noun),
            FinishCell,
        }

        let mut stack = vec![Task::Convert(noun)];
        let mut results: Vec<Self> = Vec::new();
        let mut count = 0usize;
        while let Some(task) = stack.pop() {
            match task {
                Task::Convert(noun) => {
                    if noun.is_atom() {
                        let atom = noun
                            .in_space(space)
                            .as_atom()
                            .map_err(|_| OwnedBasedNounError::Malformed("expected atom"))?;
                        let atom = atom
                            .as_u64()
                            .map_err(|_| OwnedBasedNounError::AtomTooLarge)?;
                        if !based_check(atom) {
                            return Err(OwnedBasedNounError::AtomNotBased(atom));
                        }
                        count += 1;
                        if count > max_nodes {
                            return Err(OwnedBasedNounError::TooLarge(max_nodes));
                        }
                        results.push(Self::Atom(Belt(atom)));
                    } else {
                        let cell = noun
                            .in_space(space)
                            .as_cell()
                            .map_err(|_| OwnedBasedNounError::Malformed("expected cell"))?;
                        stack.push(Task::FinishCell);
                        stack.push(Task::Convert(cell.tail().noun()));
                        stack.push(Task::Convert(cell.head().noun()));
                    }
                }
                Task::FinishCell => {
                    let tail = results.pop().expect("cell tail result is pending");
                    let head = results.pop().expect("cell head result is pending");
                    count += 1;
                    if count > max_nodes {
                        return Err(OwnedBasedNounError::TooLarge(max_nodes));
                    }
                    results.push(Self::cell(head, tail));
                }
            }
        }
        Ok(results.pop().expect("from_noun produces exactly one root"))
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
    /// Tip5 `hash-noun-varlen` starts the flattened leaf stream with this count,
    /// so we cache it by traversal when producing the input belt list.
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

    /// Computes the digest of this noun after applying tx-engine
    /// `hashable-noun` semantics.
    ///
    /// This is different from [`hash_owned_based_noun_varlen`], which hashes
    /// the noun itself. Here atoms are interpreted as `%leaf` payloads and
    /// cells are interpreted as hashable pairs, matching Hoon:
    ///
    /// ```text
    /// ?^  n  [$(n -.n) $(n +.n)]
    /// leaf+n
    /// ```
    ///
    /// TODO(types/maths): Revisit crate layering so primitive digest/hash
    /// types can live below both `nockchain-math` and `nockchain-types`.
    /// This currently returns raw limbs to avoid making `nockchain-math`
    /// depend on `nockchain-types`.
    pub fn hashable_noun_digest(&self) -> [u64; 5] {
        match self {
            Self::Atom(atom) => hash_leaf_belt(*atom),
            Self::Cell(left, right) => {
                hash_hashable_pair(left.hashable_noun_digest(), right.hashable_noun_digest())
            }
        }
    }
}

impl NounEncode for OwnedBasedNoun {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        match self {
            Self::Atom(atom) => atom.to_noun(allocator),
            Self::Cell(left, right) => {
                let left = left.to_noun(allocator);
                let right = right.to_noun(allocator);
                nockvm::noun::T(allocator, &[left, right])
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

fn hash_leaf_belt(belt: Belt) -> [u64; 5] {
    tip5::hash::hash_belts_slice(&[1, belt.0])
}

fn hash_hashable_pair(left: [u64; 5], right: [u64; 5]) -> [u64; 5] {
    let mut input = [0; 10];
    input[..5].copy_from_slice(&left);
    input[5..].copy_from_slice(&right);
    tip5::hash::hash_ten_cell(input)
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
        let mut stack = NockStack::new(nockvm::mem::NOCK_STACK_SIZE_TINY, 0);
        let noun = Atom::new(&mut stack, 7).as_noun();
        let space = stack.noun_space();

        assert_eq!(
            OwnedBasedNoun::from_noun(noun, &space),
            Ok(OwnedBasedNoun::Atom(Belt(7)))
        );
    }

    #[test]
    fn from_noun_accepts_indirect_based_atoms_that_fit_u64() {
        let mut stack = NockStack::new(nockvm::mem::NOCK_STACK_SIZE_TINY, 0);
        let noun = Atom::new(&mut stack, PRIME - 1).as_noun();
        let space = stack.noun_space();
        let atom = noun.as_atom().expect("noun should be atom");
        assert!(atom.is_indirect());

        assert_eq!(
            OwnedBasedNoun::from_noun(noun, &space),
            Ok(OwnedBasedNoun::Atom(Belt(PRIME - 1)))
        );
    }

    #[test]
    fn from_noun_rejects_non_based_atoms() {
        let mut stack = NockStack::new(nockvm::mem::NOCK_STACK_SIZE_TINY, 0);
        let noun = Atom::new(&mut stack, PRIME).as_noun();
        let space = stack.noun_space();

        assert_eq!(
            OwnedBasedNoun::from_noun(noun, &space),
            Err(OwnedBasedNounError::AtomNotBased(PRIME))
        );
    }

    #[test]
    fn from_noun_rejects_atoms_larger_than_u64() {
        let mut stack = NockStack::new(nockvm::mem::NOCK_STACK_SIZE_TINY, 0);
        let big = ubig!(1) << 80;
        let noun = Atom::from_ubig(&mut stack, &big).as_noun();
        let space = stack.noun_space();

        assert_eq!(
            OwnedBasedNoun::from_noun(noun, &space),
            Err(OwnedBasedNounError::AtomTooLarge)
        );
    }

    #[test]
    fn from_noun_survives_deep_chain_on_worker_stack() {
        // GHSA-qrw6-mmw4-vjfq: an 80,000-deep right-leaning spine in an
        // ~61 KB hax preimage must convert without stack overflow on a
        // tokio-worker-sized (2 MB) thread.
        let depth = 80_000;
        let result = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || {
                let mut stack = NockStack::new(nockvm::mem::NOCK_STACK_SIZE_SMALL, 0);
                let mut noun = nockvm::noun::D(0);
                for _ in 0..depth {
                    noun = nockvm::noun::T(&mut stack, &[nockvm::noun::D(1), noun]);
                }
                let space = stack.noun_space();
                let converted = OwnedBasedNoun::from_noun(noun, &space)
                    .expect("deep chain converts iteratively");
                // Walk the spine to confirm the full depth materialized.
                let mut cur = &converted;
                let mut levels = 0;
                while let OwnedBasedNoun::Cell(_, tail) = cur {
                    levels += 1;
                    cur = tail;
                }
                levels
            })
            .expect("spawn worker thread")
            .join();
        assert_eq!(result.expect("thread must not overflow"), depth);
    }

    #[test]
    fn from_noun_rejects_exponential_dag_expansion() {
        // GHSA-3f53-rmcr-5jmf: a doubling tower `cN = [c(N-1) c(N-1)]`
        // shares structure in the slab but implies a 2^(N+1)-1 node
        // logical tree. Conversion must stop at the budget and return an
        // error instead of allocating until the process aborts.
        let levels = 30;
        let mut stack = NockStack::new(nockvm::mem::NOCK_STACK_SIZE_SMALL, 0);
        let mut noun = nockvm::noun::D(0);
        for _ in 0..levels {
            let cell = nockvm::noun::Cell::new(&mut stack, noun, noun);
            noun = cell.as_noun();
        }
        let space = stack.noun_space();

        assert_eq!(
            OwnedBasedNoun::from_noun(noun, &space),
            Err(OwnedBasedNounError::TooLarge(
                super::MAX_OWNED_BASED_NOUN_NODES
            ))
        );
    }
}
