use nockvm::jets::util::BAIL_FAIL;
use nockvm::jets::JetErr;
use nockvm::noun::{Noun, NounAllocator};
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
