use either::Either::*;
use murmur3::murmur3_32_of_slice;

use crate::mem::*;
use crate::noun::{Allocated, Atom, DirectAtom, Noun};
use crate::{assert_acyclic, assert_no_forwarding_pointers, assert_no_junior_pointers};

crate::gdb!();

// Murmur3 hash an atom with a given padded length
fn muk_u32(syd: u32, len: usize, key: Atom) -> u32 {
    match key.as_either() {
        Left(direct) => murmur3_32_of_slice(&direct.data().to_le_bytes()[0..len], syd),
        Right(indirect) => murmur3_32_of_slice(&indirect.as_ne_bytes()[..len], syd),
    }
}

/** Byte size of an atom.
 *
 * Assumes atom is normalized
 */
pub fn met3_usize(atom: Atom) -> usize {
    match atom.as_either() {
        Left(direct) => (64 - (direct.data().leading_zeros() as usize) + 7) >> 3,
        Right(indirect) => {
            let last_word = unsafe { *(indirect.data_pointer().add(indirect.size() - 1)) };
            let last_word_bytes = (64 - (last_word.leading_zeros() as usize) + 7) >> 3;
            ((indirect.size() - 1) << 3) + last_word_bytes
        }
    }
}

fn mum_u32(syd: u32, fal: u32, key: Atom) -> u32 {
    let wyd = met3_usize(key);
    let mut i = 0;
    loop {
        if i == 8 {
            break fal;
        } else {
            let haz = muk_u32(syd, wyd, key);
            let ham = (haz >> 31) ^ (haz & !(1 << 31));
            if ham == 0 {
                i += 1;
                continue;
            } else {
                break ham;
            }
        }
    }
}

pub fn calc_atom_mug_u32(atom: Atom) -> u32 {
    mum_u32(0xCAFEBABE, 0x7FFF, atom)
}

/** Unsafe because this passes a direct atom to mum_u32 made by concatenating the two mugs,
 * so we must ensure that the tail_mug conforms to the mug invariant and is only 31 bits
 *
 * # Safety
 * head_mug and tail_mug both have msb 0.
 */
pub unsafe fn calc_cell_mug_u32(head_mug: u32, tail_mug: u32) -> u32 {
    let cat_mugs = (head_mug as u64) | ((tail_mug as u64) << 32);
    mum_u32(
        0xDEADBEEF,
        0xFFFE,
        DirectAtom::new_unchecked(cat_mugs).as_atom(),
    ) // this is safe on mugs since mugs are 31 bits
}

pub fn get_mug(noun: Noun) -> Option<u32> {
    match noun.as_either_direct_allocated() {
        Left(direct) => Some(calc_atom_mug_u32(direct.as_atom())),
        Right(allocated) => allocated.get_cached_mug(),
    }
}

const MASK_OUT_MUG: u64 = !(u32::MAX as u64);

/**
 * Set the cached mug on an allocated noun
 *
 * # Safety
 *
 * Ensure the calculated mug is correct or this will result in incorrect mugs being returned.
 * This could cause jet mismatches.
 */
pub unsafe fn set_mug(allocated: &mut Allocated, mug: u32) {
    let metadata = allocated.get_metadata();
    allocated.set_metadata((metadata & MASK_OUT_MUG) | (mug as u64));
}

/** Calculate and cache the mug for a noun, but do *not* recursively calculate cache mugs for
 * children of cells.
 *
 * If called on a cell with no mug cached for the head or tail, this function will return `None`.
 */
pub fn allocated_mug_u32_one(allocated: &mut Allocated) -> Option<u32> {
    match allocated.get_cached_mug() {
        Some(mug) => Some(mug),
        None => match allocated.as_either() {
            Left(indirect) => {
                let mug = calc_atom_mug_u32(indirect.as_atom());
                unsafe {
                    set_mug(allocated, mug);
                }
                Some(mug)
            }
            Right(cell) => match (get_mug(cell.head()), get_mug(cell.tail())) {
                (Some(head_mug), Some(tail_mug)) => {
                    let mug = unsafe { calc_cell_mug_u32(head_mug, tail_mug) };
                    unsafe {
                        set_mug(allocated, mug);
                    }
                    Some(mug)
                }
                _ => None,
            },
        },
    }
}

pub fn mug_u32_one(mut noun: Noun) -> Option<u32> {
    match noun.as_ref_mut_either_direct_allocated() {
        Left(direct) => Some(calc_atom_mug_u32(direct.as_atom())),
        Right(allocated) => allocated_mug_u32_one(allocated),
    }
}

pub fn mug_u32(stack: &mut NockStack, noun: Noun) -> u32 {
    if let Some(mug) = get_mug(noun) {
        return mug;
    }

    assert_acyclic!(noun);
    assert_no_forwarding_pointers!(noun);
    assert_no_junior_pointers!(stack, noun);

    stack.frame_push(0);
    unsafe {
        *(stack.push()) = noun;
    }
    loop {
        if stack.stack_is_empty() {
            break;
        } else {
            let noun: Noun = unsafe { *(stack.top()) };
            match noun.as_either_direct_allocated() {
                Left(_direct) => {
                    unsafe {
                        stack.pop::<Noun>();
                    }
                    continue;
                } // no point in calculating a direct mug here as we wont cache it
                Right(mut allocated) => match allocated.get_cached_mug() {
                    Some(_mug) => {
                        unsafe {
                            stack.pop::<Noun>();
                        }
                        continue;
                    }
                    None => match allocated.as_either() {
                        Left(indirect) => unsafe {
                            set_mug(&mut allocated, calc_atom_mug_u32(indirect.as_atom()));
                            stack.pop::<Noun>();
                            continue;
                        },
                        Right(cell) => unsafe {
                            match (get_mug(cell.head()), get_mug(cell.tail())) {
                                (Some(head_mug), Some(tail_mug)) => {
                                    set_mug(&mut allocated, calc_cell_mug_u32(head_mug, tail_mug));
                                    stack.pop::<Noun>();
                                    continue;
                                }
                                _ => {
                                    *(stack.push()) = cell.tail();
                                    *(stack.push()) = cell.head();
                                    continue;
                                }
                            }
                        },
                    },
                },
            }
        }
    }
    unsafe {
        stack.frame_pop();
    }

    assert_acyclic!(noun);
    assert_no_forwarding_pointers!(noun);
    assert_no_junior_pointers!(stack, noun);

    get_mug(noun).expect("Noun should have a mug once it is mugged.")
}

pub fn mug(stack: &mut NockStack, noun: Noun) -> DirectAtom {
    unsafe { DirectAtom::new_unchecked(mug_u32(stack, noun) as u64) }
}
