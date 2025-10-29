use ibig::UBig;
use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::mem::NockStack;
use nockvm::noun::{Atom, Noun, D, T};
use nockvm_macros::tas;

use crate::form::belt::{mont_reduction, montify, montiply, Belt};
use crate::form::math::tip5;
use crate::form::noun_ext::NounMathExt;
use crate::form::structs::HoonList;
use crate::jets::bp_jets::bpoly_to_list;
use crate::jets::mary_jets::{change_step, get_mary_fields};
use crate::utils::{
    belt_as_noun, bitslice_to_u128, fits_in_u128, hoon_list_to_vecbelt, hoon_list_to_vecnoun,
    vec_to_hoon_list, vecnoun_to_hoon_list,
};

pub fn hoon_list_to_sponge(list: Noun) -> Result<[u64; tip5::STATE_SIZE], JetErr> {
    if list.is_atom() {
        return Err(BAIL_FAIL);
    }

    let mut sponge = [0; tip5::STATE_SIZE];
    let mut current = list;
    let mut i = 0;

    while current.is_cell() {
        let cell = current.as_cell()?;
        sponge[i] = cell.head().as_atom()?.as_u64()?;
        current = cell.tail();
        i += 1;
    }

    if i != tip5::STATE_SIZE {
        return Err(BAIL_FAIL);
    }

    Ok(sponge)
}

pub fn permutation_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let sample = slot(subject, 6)?;
    let mut sponge = hoon_list_to_sponge(sample)?;
    tip5::permute(&mut sponge);

    let new_sponge = vec_to_hoon_list(stack, &sponge);

    Ok(new_sponge)
}

pub fn hash_varlen_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let input = slot(subject, 6)?;
    let mut input_vec = hoon_list_to_vecbelt(input)?;

    let digest = tip5::hash::hash_varlen(&mut input_vec);

    Ok(vec_to_hoon_list(stack, &digest))
}

pub fn montify_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let sam = slot(subject, 6)?;
    let x = sam.as_atom()?.as_u64()?;

    let res = montify(x);

    Ok(belt_as_noun(stack, Belt(res)))
}

pub fn montiply_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let sam = slot(subject, 6)?;
    let a = sam.as_cell()?.head().as_atom()?.as_u64()?;
    let b = sam.as_cell()?.tail().as_atom()?.as_u64()?;
    Ok(belt_as_noun(&mut context.stack, Belt(montiply(a, b))))
}

pub fn mont_reduction_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let sam = slot(subject, 6)?;
    let x_atom = sam.as_atom()?;

    let x_u128: u128 = if x_atom.is_indirect() {
        if x_atom.as_indirect()?.size() > 2 {
            // mont_reduction asserts that x < RP, so u128 should be sufficient anyway??!!
            let x_bitslice = x_atom.as_bitslice();
            assert!(fits_in_u128(x_bitslice));
            bitslice_to_u128(x_bitslice)
        } else if x_atom.as_indirect()?.size() == 2 {
            let x = unsafe { x_atom.as_u64_pair()? };
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
    let stack = &mut context.stack;
    let input = slot(subject, 6)?;
    tip5::hash::hash_belts_list(stack, input)
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
    let stack = &mut context.stack;
    let input = slot(subject, 6)?;
    let mut input_vec = hoon_list_to_vecbelt(input)?;

    let digest = tip5::hash::hash_10(&mut input_vec);

    Ok(vec_to_hoon_list(stack, &digest))
}

pub fn hash_pairs_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let lis_noun = slot(subject, 6)?; // (list (list @))

    hash_pairs(stack, lis_noun)
}

pub fn hash_pairs(stack: &mut NockStack, lis_noun: Noun) -> Result<Noun, JetErr> {
    let lis = hoon_list_to_vecnoun(lis_noun)?;
    let lent_lis = lis.len();
    assert!(lent_lis > 0);

    let mut res: Vec<Noun> = Vec::new();

    for i in 0..lent_lis / 2 {
        let b = i * 2;
        if (b + 1) == lent_lis {
            res.push(lis[b]);
        } else {
            let b0 = hoon_list_to_vecbelt(lis[b])?;
            let mut b1 = hoon_list_to_vecbelt(lis[b + 1])?;
            let mut pair = b0;
            pair.append(&mut b1);
            let digest = tip5::hash::hash_10(&mut pair);
            let digest_noun = vec_to_hoon_list(stack, &digest);
            res.push(digest_noun);
        }
    }

    Ok(vecnoun_to_hoon_list(stack, res.as_slice()))
}

pub fn hash_ten_cell_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let ten_cell = slot(subject, 6)?; // [noun-digest noun-digest]
    hash_ten_cell(stack, ten_cell)
}

fn hash_ten_cell(stack: &mut NockStack, ten_cell: Noun) -> Result<Noun, JetErr> {
    // leaf_sequence(ten-cell)
    let mut leaf: Vec<u64> = Vec::<u64>::new();
    crate::form::shape::do_leaf_sequence(ten_cell, &mut leaf)?;
    let mut leaf_belt = leaf.into_iter().map(Belt).collect();

    // list-to-tuple hash10
    let digest = tip5::hash::hash_10(&mut leaf_belt);
    Ok(digest_to_noundigest(stack, digest))
}

pub fn hash_noun_varlen_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let n = slot(subject, 6)?;
    tip5::hash::hash_noun_varlen(stack, n)
}

pub fn hash_hashable_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let h = slot(subject, 6)?;

    hash_hashable(stack, h)
}

pub fn hash_hashable(stack: &mut NockStack, h: Noun) -> Result<Noun, JetErr> {
    if !h.is_cell() {
        return Err(BAIL_FAIL);
    }

    let h_head = h.as_cell()?.head();
    let h_tail = h.as_cell()?.tail();

    if h_head.is_direct() {
        let tag = h_head.as_direct()?;

        match tag.data() {
            tas!(b"hash") => hash_hashable_hash(stack, h_tail),
            tas!(b"leaf") => hash_hashable_leaf(stack, h_tail),
            tas!(b"list") => hash_hashable_list(stack, h_tail),
            tas!(b"mary") => hash_hashable_mary(stack, h_tail),
            _ => hash_hashable_other(stack, h_head, h_tail),
        }
    } else {
        hash_hashable_other(stack, h_head, h_tail)
    }
}

fn hash_hashable_hash(_stack: &mut NockStack, p: Noun) -> Result<Noun, JetErr> {
    Ok(p)
}
fn hash_hashable_leaf(stack: &mut NockStack, p: Noun) -> Result<Noun, JetErr> {
    tip5::hash::hash_noun_varlen(stack, p)
}
fn hash_hashable_list(stack: &mut NockStack, p: Noun) -> Result<Noun, JetErr> {
    let turn: Vec<Noun> = HoonList::try_from(p)?
        .into_iter()
        .map(|x| hash_hashable(stack, x).unwrap())
        .collect();
    let turn_list = vecnoun_to_hoon_list(stack, &turn);
    tip5::hash::hash_noun_varlen(stack, turn_list)
}
fn hash_hashable_mary(stack: &mut NockStack, p: Noun) -> Result<Noun, JetErr> {
    let (ma_step, ma_array_len, _ma_array_dat) = get_mary_fields(p)?;

    let ma_changed = change_step(stack, p, D(1))?;
    let [_ma_changed_step, ma_changed_array] = ma_changed.uncell()?; // +$  mary  [step=@ =array]
    let bpoly_list = bpoly_to_list(stack, ma_changed_array)?;
    let hash_belts_list = tip5::hash::hash_belts_list(stack, bpoly_list)?;

    let leaf_step = T(stack, &[D(tas!(b"leaf")), ma_step.as_noun()]);
    let leaf_len = T(stack, &[D(tas!(b"leaf")), ma_array_len.as_noun()]);
    let hash = T(stack, &[D(tas!(b"hash")), hash_belts_list]);
    let arg = T(stack, &[leaf_step, leaf_len, hash]);

    hash_hashable(stack, arg)
}

fn hash_hashable_other(stack: &mut NockStack, p: Noun, q: Noun) -> Result<Noun, JetErr> {
    let ph = hash_hashable(stack, p)?;
    let qh = hash_hashable(stack, q)?;

    let cell = T(stack, &[ph, qh]);

    hash_ten_cell(stack, cell)
}

pub fn digest_to_atom_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let cells = slot(subject, 6)?;
    let [a, b, c, d, e] = cells.uncell()?;

    let a_big = a.as_atom()?.as_ubig(stack);
    let b_big = b.as_atom()?.as_ubig(stack);
    let c_big = c.as_atom()?.as_ubig(stack);
    let d_big = d.as_atom()?.as_ubig(stack);
    let e_big = e.as_atom()?.as_ubig(stack);

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
        assert_jet(c, hash_varlen_jet, sam, res);

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
        assert_jet(c, hash_varlen_jet, sam, res);

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
        assert_jet(c, hash_varlen_jet, sam, res);

        let c = &mut init_context();
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
        assert_jet(c, hash_varlen_jet, sam, res);
    }
}
