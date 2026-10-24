use std::mem::size_of;

use either::{Left, Right};
use ibig::UBig;
use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::mem::NockStack;
use nockvm::noun::{Atom, IndirectAtom, Noun, NounSpace, D, T};
use nockvm_macros::tas;

use crate::form::belt::{based_check, mont_reduction, montify, montiply, Belt};
use crate::form::felt::Felt;
use crate::form::handle::{finalize_mary, new_handle_mut_mary};
use crate::form::mary::{MarySlice, MarySliceMut};
use crate::form::math::tip5;
use crate::form::merk::{build_merk_heap, merk_heap_size};
use crate::form::noun_ext::{AtomMathExt, NounMathExt};
use crate::form::structs::HoonList;
use crate::jets::bp_jets::bpoly_to_list;
use crate::jets::mary_jets::{change_step, get_mary_fields};
use crate::utils::{
    belt_as_noun, bitslice_to_u128, fits_in_u128, hoon_list_to_vecbelt, hoon_list_to_vecnoun,
    vec_to_hoon_list, vecnoun_to_hoon_list,
};

/// The `tip5` door is parameterized by `num-rounds` (default 7). Production
/// always uses the default, but the door also supports a 5-round variant
/// (`round-constants` branches on `num-rounds ∈ {5, 7}`). These jets implement
/// only the 7-round permutation, so a non-default round count must be declined
/// (Punt) and left to the interpreted Hoon — otherwise the jet would silently
/// return a 7-round digest where the Hoon returns a 5-round one (a jet/Hoon
/// divergence).
///
/// For a gate that is a direct arm of the `tip5` door, the gate's subject is
/// `[sample [battery context]]` whose `context` (axis 7) is the door core; the
/// door's `num-rounds` sample is at axis 6 of that core, i.e. axis 30 of the
/// gate subject. If the value can't be read or isn't 7, Punt (safe: the Hoon
/// is authoritative either way).
const TIP5_DOOR_NUM_ROUNDS_AXIS: u64 = 30;

fn require_default_num_rounds(subject: Noun, space: &NounSpace) -> Result<(), JetErr> {
    let num_rounds = slot(subject, TIP5_DOOR_NUM_ROUNDS_AXIS, space)
        .map_err(|_| JetErr::Punt)?
        .in_space(space)
        .as_atom()
        .map_err(|_| JetErr::Punt)?
        .as_u64()
        .map_err(|_| JetErr::Punt)?;
    if num_rounds != tip5::NUM_ROUNDS as u64 {
        return Err(JetErr::Punt);
    }
    Ok(())
}

pub fn do_init_mary_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let step = slot(sam, 2, &space)?.as_atom()?.as_u32()?;
    let poly = slot(sam, 3, &space)?;

    let list: Vec<Noun> = HoonList::try_from(poly, &space)?.into_iter().collect();
    let list_len = list.len();
    let step_usize = step as usize;

    // The Hoon gate handles an empty list through zero-mary rather than the
    // packed representation below. Punt so the authoritative path preserves
    // that behavior.
    if list.is_empty() || u32::try_from(list_len).is_err() {
        return Err(JetErr::Punt);
    }

    // Mirror do-init-mary's levy guard. Each element is either one base-field
    // word at step 1, or `step` data words plus its encoding marker. Invalid
    // input must reach the Hoon guard instead of being truncated or indexed.
    for p in &list {
        let atom = p.in_space(&space).as_atom().map_err(|_| JetErr::Punt)?;
        let bytes = atom.as_ne_bytes();
        let word_count = bytes.len() / size_of::<u64>();
        let valid_width =
            (step_usize == 1 && word_count == 1) || step_usize.checked_add(1) == Some(word_count);
        let all_words_based = bytes.chunks_exact(size_of::<u64>()).all(|chunk| {
            let word = <[u8; 8]>::try_from(chunk).expect("word-sized atom chunk");
            based_check(u64::from_ne_bytes(word))
        });

        if step_usize == 0 || !valid_width || !all_words_based {
            return Err(JetErr::Punt);
        }
    }

    let (res, res_mary): (IndirectAtom, MarySliceMut) =
        new_handle_mut_mary(&mut context.stack, step_usize, list_len);
    for (j, p) in list.iter().enumerate() {
        let atom = p.in_space(&space).as_atom()?;
        match atom.as_either() {
            Left(direct) => {
                res_mary.dat[step_usize * j] = direct.data();
            }
            Right(_) => {
                for (dst, chunk) in res_mary.dat[(step_usize * j)..(step_usize * (j + 1))]
                    .iter_mut()
                    .zip(atom.as_ne_bytes().chunks_exact(8))
                {
                    let word = <[u8; 8]>::try_from(chunk).expect("word-sized atom chunk");
                    *dst = u64::from_ne_bytes(word);
                }
            }
        }
    }

    Ok(finalize_mary(
        &mut context.stack, step as usize, list_len, res,
    ))
}

pub fn hoon_list_to_sponge(
    list: Noun,
    space: &NounSpace,
) -> Result<[u64; tip5::STATE_SIZE], JetErr> {
    if list.is_atom() {
        return Err(BAIL_FAIL);
    }

    let mut sponge = [0; tip5::STATE_SIZE];
    let mut current = list;
    let mut i = 0;

    while current.is_cell() {
        let cell = current.in_space(space).as_cell()?;
        sponge[i] = cell.head().as_atom()?.as_u64()?;
        current = cell.tail().noun();
        i += 1;
    }

    if i != tip5::STATE_SIZE {
        return Err(BAIL_FAIL);
    }

    Ok(sponge)
}

pub fn permutation_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    require_default_num_rounds(subject, &space)?;
    let stack = &mut context.stack;
    let sample = slot(subject, 6, &space)?;
    let mut sponge = hoon_list_to_sponge(sample, &space)?;
    tip5::permute(&mut sponge);

    let new_sponge = vec_to_hoon_list(stack, &sponge);

    Ok(new_sponge)
}

pub fn hash_varlen_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    require_default_num_rounds(subject, &space)?;
    let stack = &mut context.stack;
    let input = slot(subject, 6, &space)?;
    let mut input_vec = hoon_list_to_vecbelt(input, &space)?;

    let digest = tip5::hash::hash_varlen(&mut input_vec);

    Ok(vec_to_hoon_list(stack, &digest))
}

pub fn montify_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let stack = &mut context.stack;
    let sam = slot(subject, 6, &space)?;
    let x = sam.in_space(&space).as_atom()?.as_u64()?;

    let res = montify(x);

    Ok(belt_as_noun(stack, Belt(res)))
}

pub fn montiply_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let sam_cell = sam.in_space(&space).as_cell()?;
    let a = sam_cell.head().as_atom()?.as_u64()?;
    let b = sam_cell.tail().as_atom()?.as_u64()?;
    Ok(belt_as_noun(&mut context.stack, Belt(montiply(a, b))))
}

pub fn mont_reduction_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let x_atom = sam.in_space(&space).as_atom()?;

    let x_u128: u128 = if x_atom.is_indirect() {
        if x_atom.size() > 2 {
            // mont_reduction asserts that x < RP, so u128 should be sufficient anyway??!!
            let x_bitslice = x_atom.as_bitslice();
            // Atoms wider than u128 are out of mont_reduction's domain (Hoon
            // asserts x < RP); return a jet error so the runtime falls back to
            // the Hoon arm instead of panicking.
            if !fits_in_u128(x_bitslice) {
                return Err(BAIL_FAIL);
            }
            bitslice_to_u128(x_bitslice)
        } else if x_atom.size() == 2 {
            let x = x_atom.as_u64_pair()?;
            ((x[1] as u128) << 64u128) + (x[0] as u128)
        } else {
            x_atom.as_u64()? as u128
        }
    } else {
        x_atom.as_u64()? as u128
    };

    Ok(belt_as_noun(
        &mut context.stack,
        Belt(mont_reduction(x_u128)),
    ))
}

pub fn hash_belts_list_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    require_default_num_rounds(subject, &space)?;
    let stack = &mut context.stack;
    let input = slot(subject, 6, &space)?;
    tip5::hash::hash_belts_list(stack, input, &space)
}

pub fn hash_belts_mary_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    require_default_num_rounds(subject, &space)?;
    let ma = slot(subject, 6, &space)?;
    let Ok(mary) = MarySlice::try_from(ma, &space) else {
        return Err(BAIL_FAIL);
    };
    if mary.step != 1 {
        return Err(BAIL_FAIL);
    }

    let mut input: Vec<Belt> = mary.dat.iter().copied().map(Belt).collect();
    let digest = tip5::hash::hash_varlen(&mut input);
    Ok(digest_to_noundigest(&mut context.stack, digest))
}

pub fn digest_to_noundigest(stack: &mut NockStack, digest: [u64; 5]) -> Noun {
    let n0 = belt_as_noun(stack, Belt(digest[0]));
    let n1 = belt_as_noun(stack, Belt(digest[1]));
    let n2 = belt_as_noun(stack, Belt(digest[2]));
    let n3 = belt_as_noun(stack, Belt(digest[3]));
    let n4 = belt_as_noun(stack, Belt(digest[4]));

    T(stack, &[n0, n1, n2, n3, n4])
}

//hash-10: hash list of 10 belts into a list of 5 belts
pub fn hash_10_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    require_default_num_rounds(subject, &space)?;
    let stack = &mut context.stack;
    let input = slot(subject, 6, &space)?;
    let mut input_vec = hoon_list_to_vecbelt(input, &space)?;

    let digest = tip5::hash::hash_10(&mut input_vec);

    Ok(vec_to_hoon_list(stack, &digest))
}

pub fn hash_felt_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    require_default_num_rounds(subject, &space)?;
    let sam = slot(subject, 6, &space)?;
    let Ok(felt) = sam.as_felt(&space) else {
        return Err(BAIL_FAIL);
    };

    let digest = hash_felt_digest(felt);
    Ok(digest_to_noundigest(&mut context.stack, digest))
}

pub fn hash_felts_list_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    require_default_num_rounds(subject, &space)?;
    let felts = slot(subject, 6, &space)?;
    let list = HoonList::try_from(felts, &space)?;
    let len = list.count();
    let mut mary_dat = vec![0; len * 3];

    for (i, e) in list.into_iter().enumerate() {
        let felt = e.as_felt(&space)?;
        let n = 3 * i;
        mary_dat[n] = felt[0].0;
        mary_dat[n + 1] = felt[1].0;
        mary_dat[n + 2] = felt[2].0;
    }

    let mary = MarySlice {
        step: 3,
        len: len as u32,
        dat: &mary_dat,
    };
    let digest = hash_felts_mary_digest(&mary)?;
    Ok(digest_to_noundigest(&mut context.stack, digest))
}

pub fn hash_felts_mary_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    require_default_num_rounds(subject, &space)?;
    let ma = slot(subject, 6, &space)?;
    let Ok(mary) = MarySlice::try_from(ma, &space) else {
        return Err(BAIL_FAIL);
    };
    if mary.step != 3 {
        return Err(BAIL_FAIL);
    }

    let digest = hash_felts_mary_digest(&mary)?;
    Ok(digest_to_noundigest(&mut context.stack, digest))
}

pub fn build_merk_heap_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let ma = slot(subject, 6, &space)?;
    let Ok(mary) = MarySlice::try_from(ma, &space) else {
        return Err(BAIL_FAIL);
    };
    let total_size = merk_heap_size(mary.len).map_err(|_| BAIL_FAIL)?;

    let (res, mut res_mary): (IndirectAtom, MarySliceMut) =
        new_handle_mut_mary(&mut context.stack, tip5::DIGEST_LENGTH, total_size as usize);
    build_merk_heap(&mary, &mut res_mary).map_err(|_| BAIL_FAIL)?;

    let root_digest: [u64; tip5::DIGEST_LENGTH] = res_mary.dat[0..tip5::DIGEST_LENGTH]
        .try_into()
        .map_err(|_| BAIL_FAIL)?;
    let heap_mary = finalize_mary(
        &mut context.stack,
        tip5::DIGEST_LENGTH,
        total_size as usize,
        res,
    );
    let root_hash = digest_to_noundigest(&mut context.stack, root_digest);
    let depth = mary.len.ilog2() + 1;

    Ok(T(
        &mut context.stack,
        &[D(depth as u64), root_hash, heap_mary],
    ))
}

fn hash_felt_digest(input: &Felt) -> [u64; 5] {
    let mut leaf = [0; tip5::RATE];
    leaf[0] = input[0].0;
    leaf[1] = input[1].0;
    leaf[2] = input[2].0;
    tip5::hash::hash_ten_cell(leaf)
}

fn hash_ten_belts(input: &[u64]) -> Result<[u64; 5], JetErr> {
    if input.len() != tip5::RATE {
        return Err(BAIL_FAIL);
    }
    let pair: [u64; tip5::RATE] = input.try_into().map_err(|_| BAIL_FAIL)?;
    Ok(tip5::hash::hash_ten_cell(pair))
}

fn hash_felts_mary_digest(mary: &MarySlice<'_>) -> Result<[u64; 5], JetErr> {
    if mary.step != 3 || mary.len == 0 {
        return Err(BAIL_FAIL);
    }

    let mut curr = vec![0; mary.len as usize * 5];
    for (felt_words, out) in mary.dat.chunks_exact(3).zip(curr.chunks_mut(5)) {
        let digest = hash_felt_digest(&Felt::from([
            Belt(felt_words[0]),
            Belt(felt_words[1]),
            Belt(felt_words[2]),
        ]));
        out.copy_from_slice(&digest);
    }

    let mut size = mary.len as usize;
    let mut next = vec![0; curr.len()];
    while size > 1 {
        let curr_layer = &curr[..size * 5];
        let next_size = size.div_ceil(2);
        next.fill(0);

        for (chunk, out) in curr_layer
            .chunks(10)
            .zip(next[..next_size * 5].chunks_mut(5))
        {
            if chunk.len() < 10 {
                out.copy_from_slice(chunk);
            } else {
                out.copy_from_slice(&hash_ten_belts(chunk)?);
            }
        }

        curr[..next_size * 5].copy_from_slice(&next[..next_size * 5]);
        size = next_size;
    }

    curr[0..5].try_into().map_err(|_| BAIL_FAIL)
}

pub fn hash_pairs_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    require_default_num_rounds(subject, &space)?;
    let stack = &mut context.stack;
    let lis_noun = slot(subject, 6, &space)?; // (list (list @))

    hash_pairs(stack, lis_noun, &space)
}

pub fn hash_pairs(
    stack: &mut NockStack,
    lis_noun: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    let lis = hoon_list_to_vecnoun(lis_noun, space)?;
    let lent_lis = lis.len();
    if lent_lis == 0 {
        return Err(BAIL_FAIL);
    }

    let mut res: Vec<Noun> = Vec::new();

    for i in 0..lent_lis / 2 {
        let b = i * 2;
        if (b + 1) == lent_lis {
            res.push(lis[b]);
        } else {
            let b0 = hoon_list_to_vecbelt(lis[b], space)?;
            let mut b1 = hoon_list_to_vecbelt(lis[b + 1], space)?;
            let mut pair = b0;
            pair.append(&mut b1);
            let digest = tip5::hash::hash_10(&mut pair);
            let digest_noun = vec_to_hoon_list(stack, &digest);
            res.push(digest_noun);
        }
    }

    Ok(vecnoun_to_hoon_list(stack, res.as_slice(), space))
}

pub fn hash_ten_cell_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    require_default_num_rounds(subject, &space)?;
    let stack = &mut context.stack;
    let ten_cell = slot(subject, 6, &space)?; // [noun-digest noun-digest]
    hash_ten_cell(stack, ten_cell, &space)
}

fn hash_ten_cell(stack: &mut NockStack, ten_cell: Noun, space: &NounSpace) -> Result<Noun, JetErr> {
    // leaf_sequence(ten-cell)
    let mut leaf: Vec<u64> = Vec::<u64>::new();
    crate::form::shape::do_leaf_sequence(ten_cell, &mut leaf, space)?;
    let leaf: [u64; tip5::RATE] = leaf.try_into().map_err(|_| BAIL_FAIL)?;

    // list-to-tuple hash10
    let digest = tip5::hash::hash_ten_cell(leaf);
    Ok(digest_to_noundigest(stack, digest))
}

pub fn hash_noun_varlen_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    require_default_num_rounds(subject, &space)?;
    let stack = &mut context.stack;
    let n = slot(subject, 6, &space)?;
    tip5::hash::hash_noun_varlen(stack, n, &space)
}

pub fn hash_hashable_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    require_default_num_rounds(subject, &space)?;
    let stack = &mut context.stack;
    let h = slot(subject, 6, &space)?;

    hash_hashable(stack, h, &space)
}

pub fn hash_hashable(stack: &mut NockStack, h: Noun, space: &NounSpace) -> Result<Noun, JetErr> {
    let mut remaining = 1usize << 20;
    let mut cache = std::collections::HashMap::new();
    hash_hashable_with_budget(stack, h, space, &mut remaining, &mut cache)
}

fn hash_hashable_with_budget(
    stack: &mut NockStack,
    h: Noun,
    space: &NounSpace,
    remaining: &mut usize,
    cache: &mut std::collections::HashMap<usize, Noun>,
) -> Result<Noun, JetErr> {
    if !h.is_cell() {
        return Err(BAIL_FAIL);
    }

    let h_cell = h.in_space(space).as_cell()?;
    let key = unsafe { h_cell.raw_pointer() } as usize;
    if let Some(&digest) = cache.get(&key) {
        return Ok(digest);
    }
    if *remaining == 0 {
        return Err(BAIL_FAIL);
    }
    *remaining -= 1;

    let h_head = h_cell.head().noun();
    let h_tail = h_cell.tail().noun();

    let digest = if h_head.is_direct() {
        let tag = h_head.as_direct()?;

        match tag.data() {
            tas!(b"hash") => hash_hashable_hash(stack, h_tail),
            tas!(b"leaf") => hash_hashable_leaf(stack, h_tail, space),
            tas!(b"list") => hash_hashable_list(stack, h_tail, space, remaining, cache),
            tas!(b"mary") => hash_hashable_mary(stack, h_tail, space, remaining, cache),
            _ => hash_hashable_other(stack, h_head, h_tail, space, remaining, cache),
        }
    } else {
        hash_hashable_other(stack, h_head, h_tail, space, remaining, cache)
    }?;
    cache.insert(key, digest);
    Ok(digest)
}

fn hash_hashable_hash(_stack: &mut NockStack, p: Noun) -> Result<Noun, JetErr> {
    Ok(p)
}
fn hash_hashable_leaf(stack: &mut NockStack, p: Noun, space: &NounSpace) -> Result<Noun, JetErr> {
    tip5::hash::hash_noun_varlen(stack, p, space)
}
fn hash_hashable_list(
    stack: &mut NockStack,
    p: Noun,
    space: &NounSpace,
    remaining: &mut usize,
    cache: &mut std::collections::HashMap<usize, Noun>,
) -> Result<Noun, JetErr> {
    // Propagate any per-element error as a jet error (falling back to Hoon)
    // instead of panicking.
    let turn: Vec<Noun> = HoonList::try_from(p, space)?
        .into_iter()
        .map(|x| hash_hashable_with_budget(stack, x, space, remaining, cache))
        .collect::<Result<Vec<Noun>, JetErr>>()?;
    let turn_list = vecnoun_to_hoon_list(stack, &turn, space);
    tip5::hash::hash_noun_varlen(stack, turn_list, space)
}
fn hash_hashable_mary(
    stack: &mut NockStack,
    p: Noun,
    space: &NounSpace,
    remaining: &mut usize,
    cache: &mut std::collections::HashMap<usize, Noun>,
) -> Result<Noun, JetErr> {
    let (ma_step, ma_array_len, _ma_array_dat) = get_mary_fields(p, space)?;

    let ma_changed = change_step(stack, p, D(1), space)?;
    let [_ma_changed_step, ma_changed_array] = ma_changed.uncell(space)?; // +$  mary  [step=@ =array]
    let bpoly_list = bpoly_to_list(stack, ma_changed_array, space)?;
    let hash_belts_list = tip5::hash::hash_belts_list(stack, bpoly_list, space)?;

    let leaf_step = T(stack, &[D(tas!(b"leaf")), ma_step.as_noun()]);
    let leaf_len = T(stack, &[D(tas!(b"leaf")), ma_array_len.as_noun()]);
    let hash = T(stack, &[D(tas!(b"hash")), hash_belts_list]);
    let arg = T(stack, &[leaf_step, leaf_len, hash]);

    hash_hashable_with_budget(stack, arg, space, remaining, cache)
}

fn hash_hashable_other(
    stack: &mut NockStack,
    p: Noun,
    q: Noun,
    space: &NounSpace,
    remaining: &mut usize,
    cache: &mut std::collections::HashMap<usize, Noun>,
) -> Result<Noun, JetErr> {
    let ph = hash_hashable_with_budget(stack, p, space, remaining, cache)?;
    let qh = hash_hashable_with_budget(stack, q, space, remaining, cache)?;

    let cell = T(stack, &[ph, qh]);

    hash_ten_cell(stack, cell, space)
}

pub fn digest_to_atom_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let stack = &mut context.stack;
    let cells = slot(subject, 6, &space)?;
    let [a, b, c, d, e] = cells.uncell(&space)?;

    let a_big = a.in_space(&space).as_atom()?.as_ubig(stack);
    let b_big = b.in_space(&space).as_atom()?.as_ubig(stack);
    let c_big = c.in_space(&space).as_atom()?.as_ubig(stack);
    let d_big = d.in_space(&space).as_atom()?.as_ubig(stack);
    let e_big = e.in_space(&space).as_atom()?.as_ubig(stack);

    // Use stack-aware operations for pow and multiplication
    let p_ubig = UBig::from(crate::form::belt::PRIME);
    let p2_ubig = p_ubig.pow_stack(stack, 2);
    let p3_ubig = p_ubig.pow_stack(stack, 3);
    let p4_ubig = p_ubig.pow_stack(stack, 4);

    let bp_big = UBig::mul_stack(stack, b_big, p_ubig);
    let cp2_big = UBig::mul_stack(stack, c_big, p2_ubig);
    let dp3_big = UBig::mul_stack(stack, d_big, p3_ubig);
    let ep4_big = UBig::mul_stack(stack, e_big, p4_ubig);

    // Use stack-aware addition
    let res1 = UBig::add_stack(stack, a_big, bp_big);
    let res2 = UBig::add_stack(stack, res1, cp2_big);
    let res3 = UBig::add_stack(stack, res2, dp3_big);
    let res = UBig::add_stack(stack, res3, ep4_big);

    Ok(Atom::from_ubig(stack, &res).as_noun())
}

#[cfg(test)]
mod tests {
    use ibig::ubig;
    use nockvm::jets::util::test::*;
    use nockvm::noun::{D, T};

    use super::*;
    use crate::utils::u128_as_noun;

    #[test]
    fn test_mont_reduction_jet() {
        let c = &mut init_context();

        // [%mont-reduction-x 18.446.744.065.119.617.025]
        // [%mont-reduction-res 4.294.967.295]
        let sam = belt_as_noun(&mut c.stack, Belt(18446744065119617025));
        let res = D(4294967295);
        assert_jet(c, mont_reduction_jet, sam, res);

        // [%mont-reduction-x 45.157.629.471.412.822.477.200]
        // [%mont-reduction-res 10.514.079.938.160]
        let sam = u128_as_noun(&mut c.stack, 45157629471412822477200u128);
        let res = D(10514079938160);
        assert_jet(c, mont_reduction_jet, sam, res);

        // [%mont-reduction-x 0]
        // [%mont-reduction-res 0]
        let sam = D(0);
        let res = D(0);
        assert_jet(c, mont_reduction_jet, sam, res);

        // [%mont-reduction-x 24.583.549.534.147.014.201.149.663.878.358.805.000]
        // [%mont-reduction-res 6.813.007.285.744.613.222]
        let sam = u128_as_noun(&mut c.stack, 24583549534147014201149663878358805000u128);
        let res = u128_as_noun(&mut c.stack, 6813007285744613222);
        assert_jet(c, mont_reduction_jet, sam, res);
    }

    #[test]
    fn test_montify_jet() {
        let c = &mut init_context();

        let sam = D(1);
        let res = D(4294967295);
        assert_jet(c, montify_jet, sam, res);

        let sam = D(122);
        let res = D(523986009990);
        assert_jet(c, montify_jet, sam, res);

        let sam = D(127128);
        let res = D(546010602278760);
        assert_jet(c, montify_jet, sam, res);

        let sam = D(127128129);
        let res = D(546011156329541055);
        assert_jet(c, montify_jet, sam, res);

        let sam = D(127128129130);
        let res = belt_as_noun(&mut c.stack, Belt(11055578874863858041));
        assert_jet(c, montify_jet, sam, res);

        let sam = D(127128129130131);
        let res = belt_as_noun(&mut c.stack, Belt(5979177847162748366));
        assert_jet(c, montify_jet, sam, res);
    }

    #[test]
    fn test_hash_varlen_jet() {
        let c = &mut init_context();
        // tip5-door payload carrying the default num-rounds=7 at axis 30 of the
        // gate subject, so the round-count guard sees the production config.
        let pay = T(&mut c.stack, &[D(0), D(7), D(0)]);

        // [%test-hash-varlen-tv ~]
        let b11048995573592393898 = belt_as_noun(&mut c.stack, Belt(11048995573592393898));
        let sam = D(0);
        let res = T(
            &mut c.stack,
            &[
                b11048995573592393898,
                D(6655187932135147625),
                D(8573492257662932655),
                D(4379820112787053727),
                D(3881663824627898703),
                D(0),
            ],
        );
        assert_jet_door(c, hash_varlen_jet, sam, pay, res);

        // [%test-hash-varlen-tv [i=2 t=~]]
        let b12061287490523852513 = belt_as_noun(&mut c.stack, Belt(12061287490523852513));
        let sam = T(&mut c.stack, &[D(2), D(0)]);
        let res = T(
            &mut c.stack,
            &[
                D(8342164316692288712),
                b12061287490523852513,
                D(4038969618836824144),
                D(5830796451787599265),
                D(468390350313364562),
                D(0),
            ],
        );
        assert_jet_door(c, hash_varlen_jet, sam, pay, res);

        // [%test-hash-varlen-tv [i=5 t=[i=26 t=~]]]
        let b13674194094340317530 = belt_as_noun(&mut c.stack, Belt(13674194094340317530));
        let b13743008867885290460 = belt_as_noun(&mut c.stack, Belt(13743008867885290460));
        let sam = T(&mut c.stack, &[D(5), D(26), D(0)]);
        let res = T(
            &mut c.stack,
            &[
                D(4045697570544439560),
                b13674194094340317530,
                b13743008867885290460,
                D(6020910684025273897),
                D(3362765570390427021),
                D(0),
            ],
        );
        assert_jet_door(c, hash_varlen_jet, sam, pay, res);

        let c = &mut init_context();
        let pay = T(&mut c.stack, &[D(0), D(7), D(0)]);
        // (hash-varlen:tip5.zeke ~[1 2.448 1 0 0 0 0 0 0 0])
        // [ i=12.811.986.333.282.368.874
        //   t=[i=13.601.598.673.786.067.780 t=~[3.807.788.325.936.413.287 5.511.165.615.113.400.862 11.490.077.061.305.916.457]]
        // ]
        let b12811986333282368874 = belt_as_noun(&mut c.stack, Belt(12811986333282368874));
        let b13601598673786067780 = belt_as_noun(&mut c.stack, Belt(13601598673786067780));
        let b11490077061305916457 = belt_as_noun(&mut c.stack, Belt(11490077061305916457));
        let sam = T(
            &mut c.stack,
            &[D(1), D(2448), D(1), D(0), D(0), D(0), D(0), D(0), D(0), D(0), D(0)],
        );
        let res = T(
            &mut c.stack,
            &[
                b12811986333282368874,
                b13601598673786067780,
                D(3807788325936413287),
                D(5511165615113400862),
                b11490077061305916457,
                D(0),
            ],
        );
        assert_jet_door(c, hash_varlen_jet, sam, pay, res);
    }

    // The round-count guard reads num-rounds at axis 30 of a tip5-door arm gate.
    // `[a [sample [b [num-rounds c]]]]` places the sample at axis 6 and
    // num-rounds at axis 30, matching the real gate layout.
    #[test]
    fn round_guard_accepts_default_7() {
        let c = &mut init_context();
        let subj = T(&mut c.stack, &[D(0), D(0), D(0), D(7), D(0)]);
        let space = c.stack.noun_space();
        assert!(require_default_num_rounds(subj, &space).is_ok());
    }

    #[test]
    fn round_guard_punts_on_non_default_5() {
        let c = &mut init_context();
        let subj = T(&mut c.stack, &[D(0), D(0), D(0), D(5), D(0)]);
        let space = c.stack.noun_space();
        assert!(matches!(
            require_default_num_rounds(subj, &space),
            Err(JetErr::Punt)
        ));
    }

    #[test]
    fn round_guard_punts_when_axis_absent() {
        let c = &mut init_context();
        // [0 sample 0]: axis 7 is an atom, so axis 30 can't be read -> Punt.
        let subj = T(&mut c.stack, &[D(0), D(0), D(0)]);
        let space = c.stack.noun_space();
        assert!(matches!(
            require_default_num_rounds(subj, &space),
            Err(JetErr::Punt)
        ));
    }

    #[test]
    fn door_hash_wrappers_punt_on_non_default_rounds() {
        let c = &mut init_context();
        let pay = T(&mut c.stack, &[D(0), D(5), D(0)]);
        let subj = T(&mut c.stack, &[D(0), D(0), pay]);
        assert!(matches!(hash_noun_varlen_jet(c, subj), Err(JetErr::Punt)));

        let c = &mut init_context();
        let pay = T(&mut c.stack, &[D(0), D(5), D(0)]);
        let subj = T(&mut c.stack, &[D(0), D(0), pay]);
        assert!(matches!(hash_hashable_jet(c, subj), Err(JetErr::Punt)));
    }

    #[test]
    fn hash_hashable_reuses_shared_subtrees() {
        let c = &mut init_context();
        let mut hashable = T(&mut c.stack, &[D(tas!(b"leaf")), D(0)]);
        for _ in 0..60 {
            hashable = T(&mut c.stack, &[hashable, hashable]);
        }
        let space = c.stack.noun_space();
        assert!(hash_hashable(&mut c.stack, hashable, &space).is_ok());
    }

    #[test]
    fn hash_felts_mary_digest_rejects_empty_input_without_panicking() {
        let mary = MarySlice {
            step: 3,
            len: 0,
            dat: &[],
        };

        assert!(hash_felts_mary_digest(&mary).is_err());
    }

    #[test]
    fn do_init_mary_punts_on_zero_step_with_nonempty_poly() {
        let c = &mut init_context();
        let poly = T(&mut c.stack, &[D(1), D(0)]);
        let sam = T(&mut c.stack, &[D(0), poly]);
        let subject = T(&mut c.stack, &[D(0), sam, D(0)]);

        assert!(matches!(do_init_mary_jet(c, subject), Err(JetErr::Punt)));
    }

    #[test]
    fn do_init_mary_punts_on_element_width_mismatch() {
        let c = &mut init_context();
        let poly = T(&mut c.stack, &[D(1), D(0)]);
        let sam = T(&mut c.stack, &[D(2), poly]);
        let subject = T(&mut c.stack, &[D(0), sam, D(0)]);

        assert!(matches!(do_init_mary_jet(c, subject), Err(JetErr::Punt)));
    }

    #[test]
    fn do_init_mary_punts_on_non_field_word() {
        let c = &mut init_context();
        let non_field_word = Atom::new(&mut c.stack, crate::form::belt::PRIME).as_noun();
        let poly = T(&mut c.stack, &[non_field_word, D(0)]);
        let sam = T(&mut c.stack, &[D(1), poly]);
        let subject = T(&mut c.stack, &[D(0), sam, D(0)]);

        assert!(matches!(do_init_mary_jet(c, subject), Err(JetErr::Punt)));
    }

    #[test]
    fn do_init_mary_packs_elements_like_rep() {
        // Success-path coverage: the packed result must equal the Hoon gate's
        // (add (rep [6 step] poly) high-bit).
        // (rep [6 1] ~[x] = x, plus the high-bit marker at bit 64).
        let c = &mut init_context();
        let x = D(0x1234_5678_9abc_def0);
        let poly = T(&mut c.stack, &[x, D(0)]);
        let sam = T(&mut c.stack, &[D(1), poly]);
        let packed = A(
            &mut c.stack,
            &(ubig!(0x1234_5678_9abc_def0) + (ubig!(1) << 64)),
        );
        let array = T(&mut c.stack, &[D(1), packed]);
        let expected = T(&mut c.stack, &[D(1), array]);
        assert_jet(c, do_init_mary_jet, sam, expected);

        // step=1, 2-word element (lent v = step+1 = 2): rep [6 1] keeps only
        // the low word, matching `end [6 1]` in the Hoon gate.
        let c = &mut init_context();
        let elt = A(
            &mut c.stack,
            &(ubig!(0xdead_beef_cafe_f00d) + (ubig!(0x1010) << 64)),
        );
        let poly = T(&mut c.stack, &[elt, D(0)]);
        let sam = T(&mut c.stack, &[D(1), poly]);
        let packed = A(
            &mut c.stack,
            &(ubig!(0xdead_beef_cafe_f00d) + (ubig!(1) << 64)),
        );
        let array = T(&mut c.stack, &[D(1), packed]);
        let expected = T(&mut c.stack, &[D(1), array]);
        assert_jet(c, do_init_mary_jet, sam, expected);

        // step=3, 4-word element (3 data words + marker): rep [6 3] keeps the
        // low 3 words; marker bit lands at bit 64*3.
        let c = &mut init_context();
        let elt = A(
            &mut c.stack,
            &(ubig!(0x1111_2222_3333_4444)
                + (ubig!(0x5555_6666_7777_8888) << 64)
                + (ubig!(0x9999_aaaa_bbbb_cccc) << 128)
                + (ubig!(0xeeee_ffff) << 192)),
        );
        let poly = T(&mut c.stack, &[elt, D(0)]);
        let sam = T(&mut c.stack, &[D(3), poly]);
        let packed = A(
            &mut c.stack,
            &(ubig!(0x1111_2222_3333_4444)
                + (ubig!(0x5555_6666_7777_8888) << 64)
                + (ubig!(0x9999_aaaa_bbbb_cccc) << 128)
                + (ubig!(1) << 192)),
        );
        let array = T(&mut c.stack, &[D(1), packed]);
        let expected = T(&mut c.stack, &[D(3), array]);
        assert_jet(c, do_init_mary_jet, sam, expected);
    }
}
