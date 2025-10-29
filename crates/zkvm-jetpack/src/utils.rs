use bitvec::prelude::{BitSlice, Lsb0};
use ibig::UBig;
use nockvm::interpreter::Context;
use nockvm::jets::JetErr;
use nockvm::mem::NockStack;
use nockvm::noun::{Atom, IndirectAtom, Noun, D, DIRECT_MAX, NONE, T};
pub use tracing::{debug, trace};

use crate::form::belt::*;

// tests whether a felt atom has the leading 1. we cannot actually test
// Felt, because it doesn't include the leading 1.
pub fn felt_atom_is_valid(felt_atom: IndirectAtom) -> bool {
    let dat_ptr = felt_atom.data_pointer();
    unsafe { *(dat_ptr.add(3)) == 0x1 }
}

pub fn vecnoun_to_hoon_list(stack: &mut NockStack, vec: &[Noun]) -> Noun {
    let mut list = D(0);
    for n in vec.iter().rev() {
        list = T(stack, &[*n, list]);
    }
    list
}

pub fn vec_to_hoon_list(stack: &mut NockStack, vec: &[u64]) -> Noun {
    let mut list = D(0);
    for e in vec.iter().rev() {
        let n = Atom::new(stack, *e).as_noun();
        list = T(stack, &[n, list]);
    }
    list
}

pub fn vec_to_hoon_tuple(stack: &mut NockStack, vec: &[u64]) -> Noun {
    assert!(vec.len() >= 2);
    let mut list = NONE;
    for e in vec.iter().rev() {
        let n = Atom::new(stack, *e).as_noun();
        list = if list.is_none() {
            n
        } else {
            T(stack, &[n, list])
        }
    }
    list
}

pub fn vecnoun_to_hoon_tuple(stack: &mut NockStack, vec: &[Noun]) -> Noun {
    assert!(vec.len() >= 2);
    let mut list = NONE;
    for n in vec.iter().rev() {
        list = if list.is_none() {
            *n
        } else {
            T(stack, &[*n, list])
        }
    }
    list
}

// convert bitslice to u128 (check with fits_in_u128 before, if you don't know size)
pub fn bitslice_to_u128(bits: &BitSlice<u64, Lsb0>) -> u128 {
    bits.iter().by_vals().enumerate().fold(
        0u128,
        |acc, (i, bit)| {
            if bit {
                acc | (1u128 << i)
            } else {
                acc
            }
        },
    )
}

// check if bitslice fits into u128
pub fn fits_in_u128(bits: &BitSlice<u64, Lsb0>) -> bool {
    bits.iter()
        .by_vals()
        .enumerate()
        .rfind(|&(_, bit)| bit)
        .is_none_or(|(i, _)| i <= 127)
}

// convert a belt to noun
#[inline(always)]
pub fn belt_as_noun(stack: &mut NockStack, res: Belt) -> Noun {
    u128_as_noun(stack, res.0 as u128)
}

// convert a u128 to noun
#[inline(always)]
pub fn u128_as_noun(stack: &mut NockStack, res: u128) -> Noun {
    if res < DIRECT_MAX as u128 {
        D(res as u64)
    } else {
        let res_big = UBig::from(res);
        Atom::from_ubig(stack, &res_big).as_noun()
    }
}

pub fn hoon_list_to_vecbelt(list: Noun) -> Result<Vec<Belt>, JetErr> {
    let mut input_iterate = list;
    let mut input_vec: Vec<Belt> = Vec::new();
    while !is_hoon_list_end(&input_iterate) {
        let input_cell = input_iterate.as_cell()?;
        let head_belt = Belt(input_cell.head().as_atom()?.as_u64()?);
        input_vec.push(head_belt);
        input_iterate = input_cell.tail();
    }

    Ok(input_vec)
}

pub fn hoon_list_to_vecnoun(list: Noun) -> Result<Vec<Noun>, JetErr> {
    let mut input_iterate = list;
    let mut input_vec: Vec<Noun> = Vec::new();
    while !is_hoon_list_end(&input_iterate) {
        let input_cell = input_iterate.as_cell()?;
        let head_belt = input_cell.head();
        input_vec.push(head_belt);
        input_iterate = input_cell.tail();
    }

    Ok(input_vec)
}

#[inline(always)]
pub fn is_hoon_list_end(noun: &Noun) -> bool {
    unsafe { noun.raw_equals(&D(0)) }
}

pub fn make_cell_hash(context: &mut Context, hash: &[u64]) -> Noun {
    assert!(hash.len() == 5);
    let mut res_cell = Atom::new(&mut context.stack, hash[4]).as_noun();
    for i in (0..=3).rev() {
        let b = Atom::new(&mut context.stack, hash[i]).as_noun();
        res_cell = T(&mut context.stack, &[b, res_cell]);
    }
    res_cell
}
