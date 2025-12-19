use nockvm::jets::util::BAIL_FAIL;
use nockvm::jets::JetErr;
use nockvm::noun::{Noun, NounAllocator, D};
use noun_serde::NounDecode;

use crate::belt::Belt;

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
    use nockvm::jets::math::util::lth;
    if unsafe { stack.equals(a, b) } {
        Ok(true)
    } else if !a.is_atom() {
        if b.is_atom() {
            Ok(false)
        } else if unsafe { stack.equals(&mut a.as_cell()?.head(), &mut b.as_cell()?.head()) } {
            dor_tip(stack, &mut a.as_cell()?.tail(), &mut b.as_cell()?.tail())
        } else {
            dor_tip(stack, &mut a.as_cell()?.head(), &mut b.as_cell()?.head())
        }
    } else if !b.is_atom() {
        Ok(false)
    } else {
        let cmp = lth(stack, a.as_atom()?, b.as_atom()?);
        Ok(unsafe { cmp.raw_equals(&D(1)) })
    }
}
