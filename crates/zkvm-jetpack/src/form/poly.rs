#![allow(clippy::len_without_is_empty)]

use std::slice::Iter;

use noun_serde::{NounDecode, NounEncode};

use crate::form::based_check;

#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct Belt(pub u64);

// Manual implementation for Belt to encode/decode as atom
impl NounEncode for Belt {
    fn to_noun<A: nockvm::noun::NounAllocator>(&self, allocator: &mut A) -> nockvm::noun::Noun {
        nockvm::noun::Atom::new(allocator, self.0).as_noun()
    }
}

impl NounDecode for Belt {
    fn from_noun(noun: &nockvm::noun::Noun) -> Result<Self, noun_serde::NounDecodeError> {
        let atom = noun
            .as_atom()
            .map_err(|_| noun_serde::NounDecodeError::ExpectedAtom)?;
        let value = atom
            .as_u64()
            .map_err(|_| noun_serde::NounDecodeError::Custom("Belt value too large".to_string()))?;
        if !based_check(value) {
            return Err(noun_serde::NounDecodeError::Custom(
                "Belt value not based".to_string(),
            ));
        }
        Ok(Belt(value))
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct Felt(pub [Belt; 3]);

pub trait Element: Clone {
    fn is_zero(&self) -> bool;
    fn zero() -> Self;
    fn len() -> usize;
    fn one() -> Self;
}

impl Element for Felt {
    #[inline(always)]
    fn is_zero(&self) -> bool {
        self.is_zero()
    }
    #[inline(always)]
    fn zero() -> Self {
        Felt::zero()
    }
    #[inline(always)]
    fn len() -> usize {
        3
    }
    #[inline(always)]
    fn one() -> Self {
        Felt::one()
    }
}

impl Element for Belt {
    #[inline(always)]
    fn is_zero(&self) -> bool {
        self.is_zero()
    }
    #[inline(always)]
    fn zero() -> Self {
        Belt::zero()
    }
    #[inline(always)]
    fn len() -> usize {
        1
    }
    #[inline(always)]
    fn one() -> Self {
        Belt::one()
    }
}

impl Element for u64 {
    #[inline(always)]
    fn is_zero(&self) -> bool {
        *self == 0
    }
    #[inline(always)]
    fn zero() -> Self {
        0
    }
    #[inline(always)]
    fn len() -> usize {
        1
    }
    #[inline(always)]
    fn one() -> Self {
        1
    }
}

pub trait Poly {
    type Element: Element;

    fn data(&self) -> &[Self::Element];

    #[inline(always)]
    fn degree(&self) -> u32 {
        self.data()
            .iter()
            .rposition(|x| !Element::is_zero(x))
            .map_or(0, |i| i as u32)
    }
    #[inline(always)]
    fn leading_coeff(&self) -> &Self::Element {
        &self.data()[self.degree() as usize]
    }
    #[inline(always)]
    fn is_zero(&self) -> bool {
        let len = self.len();
        let data = self.data();
        if len == 0 || (len == 1 && data[0].is_zero()) {
            return true;
        }
        data.iter().all(|x| x.is_zero())
    }
    #[inline(always)]
    fn len(&self) -> usize {
        self.data().len()
    }
    #[inline(always)]
    fn iter(&self) -> Iter<'_, Self::Element> {
        self.data().iter()
    }
}

impl<T> Poly for &[T]
where
    T: Element,
{
    type Element = T;
    #[inline(always)]
    fn data(&self) -> &[T] {
        self
    }
}

impl<T> Poly for Vec<T>
where
    T: Element,
{
    type Element = T;
    #[inline(always)]
    fn data(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T> Poly for &mut [T]
where
    T: Element,
{
    type Element = T;
    #[inline(always)]
    fn data(&self) -> &[T] {
        self
    }
}

// Wrapper types for Polys to convert from Cell. Only called from top level jet wrapper or in tests.
// Note that form/math functions will always use slice primitives like &[Felt] and &mut [Felt]
#[derive(Clone, Debug)]
#[repr(transparent)]
pub struct PolyVec<T>(pub Vec<T>);

impl<T> PolyVec<T> {
    #[inline(always)]
    pub fn as_slice(&self) -> &[T] {
        self.0.as_slice()
    }
    #[inline(always)]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        self.0.as_mut_slice()
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(transparent)]
pub struct PolySlice<'a, T>(pub &'a [T]);

#[repr(transparent)]
pub struct PolySliceMut<'a, T>(pub &'a mut [T]);

pub type BPolyVec = PolyVec<Belt>;
pub type BPolySlice<'a> = PolySlice<'a, Belt>;
pub type BPolySliceMut<'a> = PolySliceMut<'a, Belt>;

pub type FPolyVec = PolyVec<Felt>;
pub type FPolySlice<'a> = PolySlice<'a, Felt>;
pub type FPolySliceMut<'a> = PolySliceMut<'a, Felt>;

impl<T> Poly for PolyVec<T>
where
    T: Element,
{
    type Element = T;
    #[inline(always)]
    fn data(&self) -> &[Self::Element] {
        &self.0
    }
}

impl<T: Element> Poly for PolySlice<'_, T> {
    type Element = T;

    #[inline(always)]
    fn data(&self) -> &[T] {
        self.0
    }
}

impl<T: Element> Poly for PolySliceMut<'_, T> {
    type Element = T;

    #[inline(always)]
    fn data(&self) -> &[T] {
        self.0
    }
}

// For testing BPolyVec funcs
impl From<Vec<u64>> for BPolyVec {
    fn from(b: Vec<u64>) -> Self {
        let belts: Vec<Belt> = b.into_iter().map(|item| item.into()).collect();
        PolyVec(belts)
    }
}

impl From<BPolyVec> for Vec<u64> {
    fn from(b: BPolyVec) -> Self {
        let belts: Vec<u64> = b.0.into_iter().map(|item| item.into()).collect();
        belts
    }
}

impl From<Felt> for BPolyVec {
    fn from(b: Felt) -> Self {
        PolyVec(vec![b.0[0], b.0[1], b.0[2]])
    }
}

impl<'a> From<&'a Felt> for BPolySlice<'a> {
    fn from(f: &'a Felt) -> Self {
        PolySlice(&f.0)
    }
}

impl<'a> From<&'a BPolySliceMut<'_>> for BPolySlice<'a> {
    fn from(p: &'a BPolySliceMut) -> Self {
        Self(p.0)
    }
}

#[cfg(test)]
impl quickcheck::Arbitrary for BPolyVec {
    fn arbitrary(g: &mut quickcheck::Gen) -> Self {
        PolyVec(Vec::<Belt>::arbitrary(g))
    }
}
