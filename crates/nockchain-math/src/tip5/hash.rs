use nockvm::jets::list::util::{lent, weld};
use nockvm::jets::JetErr;
use nockvm::noun::{Noun, NounAllocator, D, T};
use noun_serde::{NounDecode, NounEncode};

use super::*;
use crate::based;
use crate::belt::{montify, Belt};
use crate::poly::Poly;
use crate::shape::*;

// assert that input is made of base field elements
pub fn assert_all_based(vecbelt: &Vec<Belt>) {
    vecbelt.iter().for_each(|b| based!(b.0));
}

// calc q and r for vecbelt, based on RATE
pub fn tip5_calc_q_r(input_vec: &Vec<Belt>) -> (usize, usize) {
    let lent_input = input_vec.len();
    let (q, r) = (lent_input / RATE, lent_input % RATE);
    (q, r)
}

// pad vecbelt with ~[1 0 ... 0] to be a multiple of rate
pub fn tip5_pad_vecbelt(input_vec: &mut Vec<Belt>, r: usize) {
    input_vec.push(Belt(1));
    for _i in 0..(RATE - r) - 1 {
        input_vec.push(Belt(0));
    }
}

// monitify vecbelt (bring into montgomery space)
pub fn tip5_montify_vecbelt(input_vec: &mut Vec<Belt>) {
    for i in 0..input_vec.len() {
        input_vec[i] = Belt(montify(input_vec[i].0));
    }
}

// calc digest
pub fn tip5_calc_digest(sponge: &[u64; 16]) -> [u64; 5] {
    let mut digest = [0u64; DIGEST_LENGTH];
    for i in 0..DIGEST_LENGTH {
        digest[i] = mont_reduction(sponge[i] as u128);
    }
    digest
}

// absorb complete input
pub fn tip5_absorb_input(input_vec: &mut Vec<Belt>, sponge: &mut [u64; 16], q: usize) {
    let mut cnt_q = q;
    let mut input_to_absorb = input_vec.as_slice();
    loop {
        let (scag_input, slag_input) = input_to_absorb.split_at(RATE);
        tip5_absorb_rate(sponge, scag_input);

        if cnt_q == 0 {
            break;
        }
        cnt_q -= 1;
        input_to_absorb = slag_input;
    }
}

// absorb one part of input (size RATE)
pub fn tip5_absorb_rate(sponge: &mut [u64; 16], input: &[Belt]) {
    assert_eq!(input.len(), RATE);

    for copy_pos in 0..RATE {
        sponge[copy_pos] = input[copy_pos].0;
    }

    permute(sponge);
}

pub fn hash_varlen(input_vec: &mut Vec<Belt>) -> [u64; 5] {
    let mut sponge = create_init_sponge_variable();

    // assert that input is made of base field elements
    assert_all_based(input_vec);

    // pad input with ~[1 0 ... 0] to be a multiple of rate
    let (q, r) = tip5_calc_q_r(input_vec);
    tip5_pad_vecbelt(input_vec, r);

    // bring input into montgomery space
    tip5_montify_vecbelt(input_vec);

    // process input in batches of size RATE
    tip5_absorb_input(input_vec, &mut sponge, q);

    // calc digest

    tip5_calc_digest(&sponge)
}

pub fn create_init_sponge_variable() -> [u64; STATE_SIZE] {
    [0u64; STATE_SIZE]
}
pub fn create_init_sponge_fixed() -> [u64; STATE_SIZE] {
    [
        0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 4294967295u64, 4294967295u64,
        4294967295u64, 4294967295u64, 4294967295u64, 4294967295u64,
    ]
}

pub fn hash_10(input_vec: &mut Vec<Belt>) -> [u64; 5] {
    // check input
    let (q, r) = tip5_calc_q_r(input_vec);
    assert_eq!(q, 1);
    assert_eq!(r, 0);
    assert_all_based(input_vec);

    // bring input into montgomery space
    tip5_montify_vecbelt(input_vec);

    // create init sponge (%fixed)
    let mut sponge = create_init_sponge_fixed();

    // process input (q=1, so one batch only)
    //tip5_absorb_input(&mut input_vec, &mut sponge, q);
    tip5_absorb_rate(&mut sponge, input_vec.as_slice());

    //  calc digest
    tip5_calc_digest(&sponge)
}

pub fn hash_noun_varlen<A: NounAllocator>(stack: &mut A, n: Noun) -> Result<Noun, JetErr> {
    let leaf = leaf_sequence(stack, n)?;
    let dyck = dyck(stack, n)?;
    let size = lent(leaf).map(|x| D(x as u64))?;

    // [size (weld leaf dyck)]
    let weld = weld(stack, leaf, dyck)?;
    let arg = T(stack, &[size, weld]);

    hash_belts_list(stack, arg)
}

pub fn hash_noun_varlen_digest<A: NounAllocator>(
    stack: &mut A,
    n: Noun,
) -> Result<[u64; 5], JetErr> {
    let noun_res = hash_noun_varlen(stack, n)?;
    let digest = <[u64; 5]>::from_noun(&noun_res)?;
    Ok(digest)
}

pub fn hash_belts_list<A: NounAllocator>(alloc: &mut A, input: Noun) -> Result<Noun, JetErr> {
    let mut input_vec = <Vec<Belt>>::from_noun(&input)?;
    let digest = hash_varlen(&mut input_vec);
    let res = digest.to_noun(alloc);
    Ok(res)
}
