use nockvm::jets::util::BAIL_FAIL;
use nockvm::jets::JetErr;
use nockvm::mem::NockStack;
use nockvm::noun::{Noun, NounAllocator, D, T};
use noun_serde::{NounDecode, NounDecodeError, NounEncode};

use crate::belt::Belt;
use crate::owned_based_noun::{
    hash_owned_based_noun_varlen, owned_based_noun_decode_error, OwnedBasedNoun,
    OwnedBasedNounError,
};
use crate::tip5;

pub trait TipHasher {
    fn hash_noun_varlen<A: NounAllocator>(
        &self,
        stack: &mut A,
        a: Noun,
    ) -> Result<[u64; 5], JetErr>;
    fn hash_ten_cell(&self, ten: [u64; 10]) -> Result<[u64; 5], JetErr>;
}

pub struct DefaultTipHasher;
impl TipHasher for DefaultTipHasher {
    fn hash_noun_varlen<A: NounAllocator>(
        &self,
        stack: &mut A,
        noun: Noun,
    ) -> Result<[u64; 5], JetErr> {
        let noun_res = crate::tip5::hash::hash_noun_varlen(stack, noun)?;
        let digest = <[u64; 5]>::from_noun(&noun_res)?;
        Ok(digest)
    }
    fn hash_ten_cell(&self, ten: [u64; 10]) -> Result<[u64; 5], JetErr> {
        let mut input: Vec<Belt> = ten.iter().map(|x| Belt(*x)).collect();
        if input.len() != 10 {
            return Err(BAIL_FAIL);
        }
        Ok(crate::tip5::hash::hash_10(&mut input))
    }
}

pub fn tip<H: TipHasher, A: NounAllocator>(
    stack: &mut A,
    a: Noun,
    hasher: &H,
) -> Result<[u64; 5], JetErr> {
    hasher.hash_noun_varlen(stack, a)
}

pub fn double_tip<H: TipHasher, A: NounAllocator>(
    stack: &mut A,
    a: Noun,
    hasher: &H,
) -> Result<[u64; 5], JetErr> {
    let hash = hasher.hash_noun_varlen(stack, a)?;
    let mut ten_cell = [0; 10];
    ten_cell[0..5].copy_from_slice(&hash);
    ten_cell[5..].copy_from_slice(&hash);
    hasher.hash_ten_cell(ten_cell)
}

pub fn lth_tip(a: &[u64; 5], b: &[u64; 5]) -> bool {
    for i in (0..=4).rev() {
        if a[i] < b[i] {
            return true;
        } else if a[i] > b[i] {
            return false;
        }
    }
    false
}

pub fn gor_tip<A: NounAllocator, H: TipHasher>(
    stack: &mut A,
    a: &mut Noun,
    b: &mut Noun,
    hasher: &H,
) -> Result<bool, JetErr> {
    let a_tip = tip(stack, *a, hasher)?;
    let b_tip = tip(stack, *b, hasher)?;

    if a_tip == b_tip {
        dor_tip(stack, a, b)
    } else {
        Ok(lth_tip(&a_tip, &b_tip))
    }
}

pub fn mor_tip<A: NounAllocator, H: TipHasher>(
    stack: &mut A,
    a: &mut Noun,
    b: &mut Noun,
    hasher: &H,
) -> Result<bool, JetErr> {
    let a_tip = double_tip(stack, *a, hasher)?;
    let b_tip = double_tip(stack, *b, hasher)?;

    if a_tip == b_tip {
        dor_tip(stack, a, b)
    } else {
        Ok(lth_tip(&a_tip, &b_tip))
    }
}

pub fn dor_tip<A: NounAllocator>(
    stack: &mut A,
    a: &mut Noun,
    b: &mut Noun,
) -> Result<bool, JetErr> {
    use nockvm::jets::math::util::lth_b;
    if unsafe { stack.equals(a, b) } {
        Ok(true)
    } else if !a.is_atom() {
        if b.is_atom() {
            Ok(false)
        } else {
            let a_cell = a.as_cell()?;
            let b_cell = b.as_cell()?;

            let mut a_head = a_cell.head();
            let mut b_head = b_cell.head();
            if unsafe { stack.equals(&mut a_head, &mut b_head) } {
                let mut a_tail = a_cell.tail();
                let mut b_tail = b_cell.tail();
                dor_tip(stack, &mut a_tail, &mut b_tail)
            } else {
                dor_tip(stack, &mut a_head, &mut b_head)
            }
        }
    } else if !b.is_atom() {
        Ok(true)
    } else {
        Ok(lth_b(stack, a.as_atom()?, b.as_atom()?))
    }
}

const SCRATCH_STACK_SIZE: usize = 8 << 10 << 10;

pub type OwnedZoonError = OwnedBasedNounError;

pub(crate) fn owned_zoon_decode_error(err: OwnedZoonError) -> NounDecodeError {
    owned_based_noun_decode_error(err)
}

fn hash_ten_limbs(ten: [u64; 10]) -> [u64; 5] {
    let mut input: Vec<Belt> = ten.into_iter().map(Belt).collect();
    tip5::hash::hash_10(&mut input)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OrderedNoun {
    pub(crate) noun: OwnedBasedNoun,
    pub(crate) tip: [u64; 5],
    pub(crate) double_tip: [u64; 5],
}

impl OrderedNoun {
    pub(crate) fn from_noun(noun: Noun) -> Result<Self, OwnedZoonError> {
        let noun = OwnedBasedNoun::from_noun(noun)?;
        Ok(Self::from_owned(noun))
    }

    pub(crate) fn encode<T: NounEncode>(value: &T) -> Result<Self, OwnedZoonError> {
        let mut stack = NockStack::new(SCRATCH_STACK_SIZE, 0);
        let noun = value.to_noun(&mut stack);
        Self::from_noun(noun)
    }

    fn from_owned(noun: OwnedBasedNoun) -> Self {
        let tip = hash_owned_based_noun_varlen(&noun);
        let mut ten = [0u64; 10];
        ten[0..5].copy_from_slice(&tip);
        ten[5..10].copy_from_slice(&tip);
        let double_tip = hash_ten_limbs(ten);
        Self {
            noun,
            tip,
            double_tip,
        }
    }
}

pub(crate) fn dor_owned(left: &OwnedBasedNoun, right: &OwnedBasedNoun) -> bool {
    if left == right {
        return true;
    }

    match (left, right) {
        (OwnedBasedNoun::Atom(left), OwnedBasedNoun::Atom(right)) => left < right,
        (OwnedBasedNoun::Atom(_), OwnedBasedNoun::Cell(_, _)) => true,
        (OwnedBasedNoun::Cell(_, _), OwnedBasedNoun::Atom(_)) => false,
        (
            OwnedBasedNoun::Cell(left_head, left_tail),
            OwnedBasedNoun::Cell(right_head, right_tail),
        ) => {
            if left_head == right_head {
                dor_owned(left_tail, right_tail)
            } else {
                dor_owned(left_head, right_head)
            }
        }
    }
}

pub(crate) fn gor_owned(left: &OrderedNoun, right: &OrderedNoun) -> bool {
    if left.tip == right.tip {
        dor_owned(&left.noun, &right.noun)
    } else {
        lth_tip(&left.tip, &right.tip)
    }
}

pub(crate) fn mor_owned(left: &OrderedNoun, right: &OrderedNoun) -> bool {
    if left.double_tip == right.double_tip {
        dor_owned(&left.noun, &right.noun)
    } else {
        lth_tip(&left.double_tip, &right.double_tip)
    }
}

pub(crate) trait ZTreeValue: Sized {
    type Output;

    fn ordered(&self) -> &OrderedNoun;
    fn same_slot(&self, other: &Self) -> bool;
    fn merge_duplicate(existing: Self, incoming: Self) -> Self;
    fn into_output(self) -> Self::Output;
}

pub(crate) trait ZTreeEncode {
    fn encode_payload<A: NounAllocator>(&self, allocator: &mut A) -> Noun;
}

pub(crate) trait ZTreeDecode: Sized {
    const KIND: &'static str;

    fn decode_payload(noun: &Noun) -> Result<Self, NounDecodeError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum ZTree<P> {
    #[default]
    Empty,
    Node {
        payload: P,
        left: Box<ZTree<P>>,
        right: Box<ZTree<P>>,
    },
}

impl<P> ZTree<P> {
    fn split_node(self) -> (P, Box<Self>, Box<Self>) {
        match self {
            Self::Node {
                payload,
                left,
                right,
            } => (payload, left, right),
            Self::Empty => unreachable!("expected z-tree node"),
        }
    }

    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::Node { left, right, .. } => 1 + left.len() + right.len(),
        }
    }

    pub(crate) fn iter(&self) -> ZTreeIter<'_, P> {
        ZTreeIter { stack: vec![self] }
    }

    pub(crate) fn fold<R, F>(&self, empty: &R, node: &F) -> R
    where
        R: Clone,
        F: Fn(&P, R, R) -> R,
    {
        match self {
            Self::Empty => empty.clone(),
            Self::Node {
                payload,
                left,
                right,
            } => {
                let left = left.fold(empty, node);
                let right = right.fold(empty, node);
                node(payload, left, right)
            }
        }
    }
}

pub(crate) struct ZTreeIter<'a, P> {
    stack: Vec<&'a ZTree<P>>,
}

impl<'a, P> Iterator for ZTreeIter<'a, P> {
    type Item = &'a P;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(tree) = self.stack.pop() {
            match tree {
                ZTree::Empty => continue,
                ZTree::Node {
                    payload,
                    left,
                    right,
                } => {
                    self.stack.push(right);
                    self.stack.push(left);
                    return Some(payload);
                }
            }
        }
        None
    }
}

impl<P: ZTreeValue> ZTree<P> {
    pub(crate) fn insert(self, candidate: P) -> (Self, bool) {
        match self {
            Self::Empty => (
                Self::Node {
                    payload: candidate,
                    left: Box::new(Self::Empty),
                    right: Box::new(Self::Empty),
                },
                true,
            ),
            Self::Node {
                payload,
                left,
                right,
            } => {
                if candidate.same_slot(&payload) {
                    return (
                        Self::Node {
                            payload: P::merge_duplicate(payload, candidate),
                            left,
                            right,
                        },
                        false,
                    );
                }

                if gor_owned(candidate.ordered(), payload.ordered()) {
                    let (inserted, added) = (*left).insert(candidate);
                    let (inserted_payload, inserted_left, inserted_right) = inserted.split_node();

                    if mor_owned(payload.ordered(), inserted_payload.ordered()) {
                        (
                            Self::Node {
                                payload,
                                left: Box::new(Self::Node {
                                    payload: inserted_payload,
                                    left: inserted_left,
                                    right: inserted_right,
                                }),
                                right,
                            },
                            added,
                        )
                    } else {
                        let new_root = Self::Node {
                            payload,
                            left: inserted_right,
                            right,
                        };
                        (
                            Self::Node {
                                payload: inserted_payload,
                                left: inserted_left,
                                right: Box::new(new_root),
                            },
                            added,
                        )
                    }
                } else {
                    let (inserted, added) = (*right).insert(candidate);
                    let (inserted_payload, inserted_left, inserted_right) = inserted.split_node();

                    if mor_owned(payload.ordered(), inserted_payload.ordered()) {
                        (
                            Self::Node {
                                payload,
                                left,
                                right: Box::new(Self::Node {
                                    payload: inserted_payload,
                                    left: inserted_left,
                                    right: inserted_right,
                                }),
                            },
                            added,
                        )
                    } else {
                        let new_root = Self::Node {
                            payload,
                            left,
                            right: inserted_left,
                        };
                        (
                            Self::Node {
                                payload: inserted_payload,
                                left: Box::new(new_root),
                                right: inserted_right,
                            },
                            added,
                        )
                    }
                }
            }
        }
    }

    pub(crate) fn into_outputs(self, out: &mut Vec<P::Output>) {
        match self {
            Self::Empty => {}
            Self::Node {
                payload,
                left,
                right,
            } => {
                out.push(payload.into_output());
                left.into_outputs(out);
                right.into_outputs(out);
            }
        }
    }
}

impl<P: ZTreeEncode> ZTree<P> {
    pub(crate) fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        match self {
            Self::Empty => D(0),
            Self::Node {
                payload,
                left,
                right,
            } => {
                let payload = payload.encode_payload(allocator);
                let left = left.to_noun(allocator);
                let right = right.to_noun(allocator);
                T(allocator, &[payload, left, right])
            }
        }
    }
}

impl<P: ZTreeDecode> ZTree<P> {
    pub(crate) fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        if let Ok(atom) = noun.as_atom() {
            if atom.as_u64()? == 0 {
                return Ok(Self::Empty);
            }
            return Err(NounDecodeError::Custom(format!(
                "{} encountered unexpected non-zero atom",
                P::KIND
            )));
        }

        let cell = noun.as_cell()?;
        let payload = P::decode_payload(&cell.head())?;
        let branches = cell
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::Custom(format!("{} branches must be a cell", P::KIND)))?;

        Ok(Self::Node {
            payload,
            left: Box::new(Self::from_noun(&branches.head())?),
            right: Box::new(Self::from_noun(&branches.tail())?),
        })
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use noun_serde::{NounDecode, NounDecodeError, NounEncode};
    use quickcheck::{empty_shrinker, Arbitrary, Gen};

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
    pub(crate) enum BoundedTreeValue {
        Atom(u16),
        Cell(Box<BoundedTreeValue>, Box<BoundedTreeValue>),
    }

    impl BoundedTreeValue {
        fn arbitrary_at_depth(g: &mut Gen, depth: usize) -> Self {
            let max_depth = 3;
            if depth >= max_depth || bool::arbitrary(g) {
                Self::Atom(u16::arbitrary(g))
            } else {
                Self::Cell(
                    Box::new(Self::arbitrary_at_depth(g, depth + 1)),
                    Box::new(Self::arbitrary_at_depth(g, depth + 1)),
                )
            }
        }
    }

    impl Arbitrary for BoundedTreeValue {
        fn arbitrary(g: &mut Gen) -> Self {
            Self::arbitrary_at_depth(g, 0)
        }

        fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
            empty_shrinker()
        }
    }

    impl NounEncode for BoundedTreeValue {
        fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
            match self {
                Self::Atom(atom) => u64::from(*atom).to_noun(allocator),
                Self::Cell(left, right) => {
                    let left = left.to_noun(allocator);
                    let right = right.to_noun(allocator);
                    T(allocator, &[left, right])
                }
            }
        }
    }

    impl NounDecode for BoundedTreeValue {
        fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
            if let Ok(atom) = noun.as_atom() {
                let atom = atom.as_u64().map_err(|_| {
                    NounDecodeError::Custom("atom too large for bounded value".into())
                })?;
                let atom = u16::try_from(atom).map_err(|_| {
                    NounDecodeError::Custom("atom too large for bounded value".into())
                })?;
                Ok(Self::Atom(atom))
            } else {
                let cell = noun.as_cell()?;
                Ok(Self::Cell(
                    Box::new(Self::from_noun(&cell.head())?),
                    Box::new(Self::from_noun(&cell.tail())?),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use nockvm::mem::NockStack;
    use nockvm::noun::{D, T};

    use super::dor_tip;

    #[test]
    fn dor_tip_matches_hoon_for_mixed_atom_cell_inputs() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);
        let cell = T(&mut stack, &[D(1), D(2)]);

        let mut atom_vs_cell_left = D(7);
        let mut atom_vs_cell_right = cell;
        assert!(
            dor_tip(&mut stack, &mut atom_vs_cell_left, &mut atom_vs_cell_right)
                .expect("dor-tip should succeed")
        );

        let mut cell_vs_atom_left = cell;
        let mut cell_vs_atom_right = D(7);
        assert!(
            !dor_tip(&mut stack, &mut cell_vs_atom_left, &mut cell_vs_atom_right)
                .expect("dor-tip should succeed")
        );

        // Regresses Roswell parity failure where matching deep heads should recurse to tails.
        let pair = T(&mut stack, &[D(1), D(2)]);
        let head = T(&mut stack, &[pair, D(3)]);
        let mut deep_left = T(&mut stack, &[head, D(4)]);
        let mut deep_right = T(&mut stack, &[head, D(5)]);
        assert!(
            dor_tip(&mut stack, &mut deep_left, &mut deep_right).expect("dor-tip should succeed"),
            "expected dor-tip to compare tails when deep heads are equal"
        );
    }
}
