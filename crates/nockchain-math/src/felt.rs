use core::ops::{Add, Div, Mul, Neg, Sub};

use nockvm::noun::IndirectAtom;
use noun_serde::{NounDecode, NounEncode};
use num_traits::{MulAdd, Pow};

use crate::belt::*;
use crate::bpoly::*;
use crate::handle::new_handle_mut_felt;
use crate::noun_ext::NounMathExt;
use crate::poly::*;

#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct Felt(pub [Belt; 3]);

// Custom NounEncode implementation for Felt
impl NounEncode for Felt {
    fn to_noun<A: nockvm::noun::NounAllocator>(&self, allocator: &mut A) -> nockvm::noun::Noun {
        let (res_atom, res_felt): (IndirectAtom, &mut Felt) = new_handle_mut_felt(allocator);
        res_felt.0.copy_from_slice(&self.0);
        res_atom.as_noun()
    }
}

// Custom NounDecode implementation for Felt
impl NounDecode for Felt {
    fn from_noun(noun: &nockvm::noun::Noun) -> Result<Self, noun_serde::NounDecodeError> {
        let felt_slice = noun.as_felt()?;
        let res: [Belt; 3] = [felt_slice[0], felt_slice[1], felt_slice[2]];
        Ok(Felt(res))
    }
}

impl Felt {
    #[inline(always)]
    pub fn zero() -> Self {
        Felt(Default::default())
    }

    #[inline(always)]
    pub fn one() -> Self {
        Self::from([1, 0, 0])
    }

    #[inline(always)]
    pub fn constant(a: u64) -> Self {
        Felt([Belt(a), Belt::zero(), Belt::zero()])
    }

    #[inline(always)]
    pub fn lift(a: Belt) -> Self {
        Felt([a, Belt::zero(), Belt::zero()])
    }

    #[inline(always)]
    pub fn ordered_root(order: u64) -> Result<Self, FieldError> {
        Ok(Self::constant(Belt(order).ordered_root()?.into()))
    }

    #[inline(always)]
    pub fn is_zero(&self) -> bool {
        self.0.iter().all(|&e| e.is_zero())
    }

    #[inline(always)]
    pub fn degree(&self) -> u32 {
        match self.0.iter().rposition(|&x| x.is_zero()) {
            Some(i) => i as u32,
            // TODO: change return to an enum so this can be negative infty?
            None => 0,
        }
    }
    #[inline(always)]
    pub fn unpack(self) -> [u64; 3] {
        [self.0[0].0, self.0[1].0, self.0[2].0]
    }

    #[inline(always)]
    pub fn copy_from_slice(&mut self, src: &Felt) {
        self.0.copy_from_slice(&src.0)
    }
}

impl core::ops::Index<usize> for Felt {
    type Output = Belt;

    #[inline(always)]
    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}

impl core::ops::IndexMut<usize> for Felt {
    #[inline(always)]
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.0[index]
    }
}

impl From<[Belt; 3]> for Felt {
    #[inline(always)]
    fn from(f: [Belt; 3]) -> Self {
        Felt(f)
    }
}

impl From<[u64; 3]> for Felt {
    #[inline(always)]
    fn from(f: [u64; 3]) -> Self {
        Felt(f.map(Belt::from))
    }
}

impl TryFrom<&[u64]> for Felt {
    type Error = ();

    #[inline(always)]
    fn try_from(f: &[u64]) -> Result<Self, Self::Error> {
        if f.len() != 3 {
            return Err(());
        }
        Ok(Felt([Belt::from(f[0]), Belt::from(f[1]), Belt::from(f[2])]))
    }
}

impl TryFrom<&[Belt]> for Felt {
    type Error = ();

    #[inline(always)]
    fn try_from(f: &[Belt]) -> Result<Self, Self::Error> {
        if f.len() != 3 {
            return Err(());
        }
        Ok(Felt([f[0], f[1], f[2]]))
    }
}

impl From<Felt> for [u64; 3] {
    #[inline(always)]
    fn from(f: Felt) -> Self {
        f.0.map(u64::from)
    }
}

impl From<Felt> for [Belt; 3] {
    #[inline(always)]
    fn from(f: Felt) -> Self {
        f.0
    }
}

impl<'a> From<&'a Felt> for &'a [Belt] {
    #[inline(always)]
    fn from(f: &'a Felt) -> &'a [Belt] {
        &f.0
    }
}

#[cfg(test)]
impl quickcheck::Arbitrary for Felt {
    fn arbitrary(g: &mut quickcheck::Gen) -> Self {
        Felt([Belt::arbitrary(g), Belt::arbitrary(g), Belt::arbitrary(g)])
    }
}

impl Add for Felt {
    type Output = Self;

    #[inline(always)]
    fn add(self, rhs: Self) -> Self::Output {
        let mut res: Felt = Felt::zero();
        fadd(&self, &rhs, &mut res);
        res
    }
}

impl Sub for Felt {
    type Output = Self;

    #[inline(always)]
    fn sub(self, rhs: Self) -> Self::Output {
        let mut res: Felt = Felt::zero();
        fsub(&self, &rhs, &mut res);
        res
    }
}

impl Mul for Felt {
    type Output = Self;

    #[inline(always)]
    fn mul(self, rhs: Self) -> Self::Output {
        let mut res: Felt = Felt::zero();
        fmul(&self, &rhs, &mut res);
        res
    }
}

impl Pow<usize> for Felt {
    type Output = Self;

    #[inline(always)]
    fn pow(self, rhs: usize) -> Self::Output {
        let mut res: Felt = Felt::zero();
        fpow(&self, rhs as u64, &mut res);
        res
    }
}

impl Neg for Felt {
    type Output = Self;

    #[inline(always)]
    fn neg(self) -> Self::Output {
        let mut res: Felt = Felt::zero();
        fneg(&self, &mut res);
        res
    }
}

impl Div for Felt {
    type Output = Self;

    #[inline(always)]
    fn div(self, rhs: Self) -> Self::Output {
        let mut res: Felt = Felt::zero();
        fdiv(&self, &rhs, &mut res);
        res
    }
}

impl MulAdd for Felt {
    type Output = Self;
    #[inline(always)]
    fn mul_add(self, a: Self, b: Self) -> Self {
        let mut res: Felt = Felt::zero();
        fmul(&self, &a, &mut res);
        fadd_self(&b, &mut res);
        res
    }
}

//==============================================================================
// field extension methods
//==============================================================================

pub fn fadd(a: &Felt, b: &Felt, res: &mut Felt) {
    for i in 0..3 {
        res[i] = a[i] + b[i];
    }
}

pub fn fadd_(a: &Felt, b: &Felt) -> Felt {
    let mut res: Felt = Felt::zero();
    fadd(a, b, &mut res);
    res
}

pub fn fadd_self(a: &Felt, res: &mut Felt) {
    for i in 0..3 {
        res[i] = a[i] + res[i];
    }
}

#[inline(always)]
pub fn fsub(a: &Felt, b: &Felt, res: &mut Felt) {
    for i in 0..3 {
        res[i] = a[i] - b[i];
    }
}

pub fn fsub_(a: &Felt, b: &Felt) -> Felt {
    let mut res: Felt = Felt::zero();
    fsub(a, b, &mut res);
    res
}

#[inline(always)]
pub fn fneg(a: &Felt, res: &mut Felt) {
    for i in 0..3 {
        res[i] = -a[i];
    }
}

pub fn fneg_(a: &Felt) -> Felt {
    let mut res: Felt = Felt::zero();
    fneg(a, &mut res);
    res
}

#[inline(always)]
pub fn felt_to_bpoly(dat: &Felt) -> BPolyVec {
    PolyVec(vec![dat.0[0], dat.0[1], dat.0[2]])
}

pub const IRD: [Belt; 4] = [Belt(1), Belt(18446744069414584320), Belt(0), Belt(1)];

// See hoon comments for full explanation
#[inline(always)]
pub fn fmul(a: &Felt, b: &Felt, res: &mut Felt) {
    let poly_a: &[Belt] = &a.0;
    let poly_b: &[Belt] = &b.0;

    let a0: Belt = poly_a[0];
    let a1: Belt = poly_a[1];
    let a2: Belt = poly_a[2];
    let b0: Belt = poly_b[0];
    let b1: Belt = poly_b[1];
    let b2: Belt = poly_b[2];

    let a0b0: Belt = a0 * b0;
    let a1b1: Belt = a1 * b1;
    let a2b2: Belt = a2 * b2;
    let a0b1_a1b0: Belt = ((a0 + a1) * (b0 + b1) - a0b0) - a1b1;
    let a1b2_a2b1: Belt = ((a1 + a2) * (b1 + b2) - a1b1) - a2b2;
    let a0b2_a2b0: Belt = ((a0 + a2) * (b0 + b2) - a0b0) - a2b2;

    res.0[0] = a0b0 - a1b2_a2b1;
    res.0[1] = (a0b1_a1b0 + a1b2_a2b1) - a2b2;
    res.0[2] = a0b2_a2b0 + a1b1 + a2b2;
}

pub fn fmul_(a: &Felt, b: &Felt) -> Felt {
    let mut res = Felt::zero();
    fmul(a, b, &mut res);
    res
}

pub fn fscal_(a: &Belt, b: &Felt) -> Felt {
    Felt([b.0[0] * *a, b.0[1] * *a, b.0[2] * *a])
}

pub fn fscal_self(a: &Belt, b: &mut Felt) {
    b.0[0] = b.0[0] * *a;
    b.0[1] = b.0[1] * *a;
    b.0[2] = b.0[2] * *a;
}

const POLY: [Belt; 4] = [Belt(1), Belt(18446744069414584320), Belt(0), Belt(1)];

pub const DUV_LEN: usize = 4;

#[inline(always)]
pub fn finv(a: &Felt, res: &mut Felt) {
    let poly_a: &[Belt] = a.into();

    let mut poly_d = [Belt(0); DUV_LEN];
    let mut poly_u = [Belt(0); DUV_LEN];
    let mut poly_v = [Belt(0); DUV_LEN];
    let poly_bpoly = POLY.as_slice();

    bpegcd(
        poly_bpoly,
        poly_a,
        poly_d.as_mut_slice(),
        poly_u.as_mut_slice(),
        poly_v.as_mut_slice(),
    );

    bpscal(poly_d[0].inv(), poly_v.as_slice(), &mut res.0);
}

pub fn finv_(a: &Felt) -> Felt {
    let mut res: Felt = Felt::zero();
    finv(a, &mut res);
    res
}

#[inline(always)]
pub fn fpow_(term: &Felt, exponent: u64) -> Felt {
    let mut res: Felt = Felt::zero();
    fpow(term, exponent, &mut res);
    res
}

#[inline(always)]
pub fn fpow(term: &Felt, exponent: u64, c: &mut Felt) {
    let a: &mut Felt = &mut term.clone();
    let mut b: u64 = exponent;
    *c = Felt::from([Belt(1), Belt(0), Belt(0)]);
    if b == 0 {
        return;
    }

    while b > 1 {
        let a_clone = *a;

        if b & 1 == 0 {
            fmul(&a_clone, &a_clone, a);
            b /= 2;
        } else {
            fmul(&c.clone(), a, c);
            fmul(&a_clone, &a_clone, a);
            b = (b - 1) / 2;
        }
    }
    fmul(&c.clone(), a, c);
}

#[inline(always)]
pub fn fdiv(a: &Felt, b: &Felt, res: &mut Felt) {
    let binv = &mut Felt::zero();
    finv(b, binv);
    fmul(a, binv, res);
}

#[inline(always)]
pub fn fdiv_(a: &Felt, b: &Felt) -> Felt {
    let mut res: Felt = Felt::zero();
    fdiv(a, b, &mut res);
    res
}
