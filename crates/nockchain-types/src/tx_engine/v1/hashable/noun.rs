use nockchain_math::noun_ext::NounMathExtHandle;
use nockchain_math::poly::BPolySlice;
use nockchain_math::structs::HoonList;
use nockchain_math::tip5;
use nockvm::jets::util::BAIL_FAIL;
use nockvm::jets::JetErr;
use nockvm::noun::{Atom, Noun, NounAllocator, NounSpace, D};
use nockvm_macros::tas;
use noun_serde::{NounDecode, NounEncode};

use super::{hash_list, hash_mary, hash_pair};
use crate::tx_engine::common::Hash;

/// Compatibility wrapper for noun-facing `hash-hashable` callers. The noun
/// frontend lowers into the same direct digest helpers used by handwritten Rust
/// `Hashable` implementations, then re-encodes the final digest as a noun.
pub fn hash_hashable<A: NounAllocator>(stack: &mut A, h: Noun) -> Result<Noun, JetErr> {
    Ok(hash_hashable_digest(stack, h)?.to_noun(stack))
}

/// Converts an arbitrary noun into the canonical recursive `hashable` noun
/// shape used by `note-data.hashable-noun` in tx-engine Hoon.
pub fn noun_hashable<A: NounAllocator>(stack: &mut A, noun: Noun) -> Noun {
    if !noun.is_cell() {
        return hashable_leaf_noun(stack, noun);
    }

    let space = stack.noun_space();
    let cell = noun
        .in_space(&space)
        .as_cell()
        .expect("cell-checked noun must decode as a cell");
    let left = noun_hashable(stack, cell.head().noun());
    let right = noun_hashable(stack, cell.tail().noun());
    nockvm::noun::T(stack, &[left, right])
}

fn hash_hashable_digest<A: NounAllocator>(stack: &mut A, h: Noun) -> Result<Hash, JetErr> {
    let space = stack.noun_space();
    let h_cell = h.in_space(&space).as_cell().map_err(|_| BAIL_FAIL)?;
    let h_head = h_cell.head();
    let h_tail = h_cell.tail();

    if h_head.is_direct() {
        let tag = h_head.as_atom()?.as_direct()?;

        match tag.data() {
            tas!(b"hash") => decode_hash_digest_noun(h_tail.noun(), &space),
            tas!(b"leaf") => hash_leaf_digest(stack, h_tail.noun()),
            tas!(b"list") => hash_hashable_list_digest(stack, h_tail.noun()),
            tas!(b"mary") => hash_hashable_mary_digest(stack, h_tail.noun()),
            _ => hash_hashable_other_digest(stack, h_head.noun(), h_tail.noun()),
        }
    } else {
        hash_hashable_other_digest(stack, h_head.noun(), h_tail.noun())
    }
}

fn decode_hash_digest_noun(noun: Noun, space: &NounSpace) -> Result<Hash, JetErr> {
    Hash::from_noun(&noun, space).map_err(|_| BAIL_FAIL)
}

pub(crate) fn hash_leaf_digest<A: NounAllocator>(
    stack: &mut A,
    noun: Noun,
) -> Result<Hash, JetErr> {
    let space = stack.noun_space();
    let digest = tip5::hash::hash_noun_varlen_digest(stack, noun, &space)?;
    Ok(Hash::from_limbs(&digest))
}

pub(crate) fn hashable_leaf_noun<A: NounAllocator>(stack: &mut A, noun: Noun) -> Noun {
    tagged_hashable_noun(stack, tas!(b"leaf"), noun)
}

pub(crate) fn hashable_hash_noun<A: NounAllocator>(stack: &mut A, hash: &Hash) -> Noun {
    let noun = hash.to_noun(stack);
    tagged_hashable_noun(stack, tas!(b"hash"), noun)
}

fn hash_hashable_list_digest<A: NounAllocator>(stack: &mut A, p: Noun) -> Result<Hash, JetErr> {
    let space = stack.noun_space();
    let mut hashed_items = Vec::new();
    for item in HoonList::try_from(p, &space)? {
        hashed_items.push(hash_hashable_digest(stack, item)?);
    }

    Ok(hash_list(&hashed_items))
}

fn hash_hashable_mary_digest<A: NounAllocator>(stack: &mut A, p: Noun) -> Result<Hash, JetErr> {
    let space = stack.noun_space();
    let [ma_step_noun, ma_array] = p.in_space(&space).uncell()?;
    let [ma_array_len_noun, _ma_array_dat] = ma_array.uncell()?;
    let ma_step = ma_step_noun.as_atom()?.as_u64()?;
    let ma_array_len = ma_array_len_noun.as_atom()?.as_u64()?;

    let ma_changed = change_mary_step(stack, p, 1)?;
    let [_ma_changed_step, ma_changed_array] = ma_changed.in_space(&space).uncell()?;
    let normalized_bpoly = BPolySlice::try_from(ma_changed_array.noun(), &space)?;

    hash_mary(ma_step, ma_array_len, normalized_bpoly.0).map_err(|_| BAIL_FAIL)
}

fn hash_hashable_other_digest<A: NounAllocator>(
    stack: &mut A,
    p: Noun,
    q: Noun,
) -> Result<Hash, JetErr> {
    let ph = hash_hashable_digest(stack, p)?;
    let qh = hash_hashable_digest(stack, q)?;
    Ok(hash_pair(&ph, &qh))
}

fn tagged_hashable_noun<A: NounAllocator>(stack: &mut A, tag: u64, payload: Noun) -> Noun {
    nockvm::noun::T(stack, &[D(tag), payload])
}

pub fn bpoly_to_list<A: NounAllocator>(stack: &mut A, sam: Noun) -> Result<Noun, JetErr> {
    let space = stack.noun_space();
    let sam_bpoly = BPolySlice::try_from(sam, &space)?;
    let mut res_list = D(0);
    for belt in sam_bpoly.0.iter().rev() {
        // Belt values are field elements and may exceed DIRECT_MAX, so they must
        // be encoded with Atom::new to allow indirect atoms.
        let belt_noun = belt.to_noun(stack);
        res_list = nockvm::noun::T(stack, &[belt_noun, res_list]);
    }
    Ok(res_list)
}

fn change_mary_step<A: NounAllocator>(
    stack: &mut A,
    ma_noun: Noun,
    new_step: u64,
) -> Result<Noun, JetErr> {
    let space = stack.noun_space();
    let [ma_step_noun, ma_array] = ma_noun.in_space(&space).uncell()?;
    let [array_len_noun, array_dat] = ma_array.uncell()?;

    let ma_step = ma_step_noun.as_atom()?.as_u64()?;
    let array_len = array_len_noun.as_atom()?.as_u64()?;

    if ma_step == new_step {
        return Ok(ma_noun);
    }
    if (ma_step * array_len) % new_step != 0 {
        return Err(BAIL_FAIL);
    }

    let new_array_len = ma_step.checked_mul(array_len).ok_or(BAIL_FAIL)? / new_step;
    let new_step_noun = Atom::new(stack, new_step).as_noun();
    let new_array_len_noun = Atom::new(stack, new_array_len).as_noun();

    Ok(nockvm::noun::T(
        stack,
        &[new_step_noun, new_array_len_noun, array_dat.noun()],
    ))
}
