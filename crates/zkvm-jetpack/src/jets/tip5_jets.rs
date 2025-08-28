use ibig::UBig;
use nockvm::interpreter::Context;
use nockvm::jets::list::util::{lent, weld};
use nockvm::jets::util::slot;
use nockvm::jets::JetErr;
use nockvm::mem::NockStack;
use nockvm::noun::{Atom, Noun, D, T};
use nockvm_macros::tas;

use crate::based;
use crate::form::math::tip5::*;
use crate::form::{Belt, Poly};
use crate::hand::structs::HoonList;
use crate::jets::bp_jets::bpoly_to_list;
use crate::jets::mary_jets::{change_step, get_mary_fields};
use crate::jets::shape_jets::{do_leaf_sequence, dyck, leaf_sequence};
use crate::jets::utils::jet_err;
use crate::noun::noun_ext::NounExt;
use crate::utils::{
    belt_as_noun, bitslice_to_u128, fits_in_u128, hoon_list_to_vecbelt, hoon_list_to_vecnoun,
    vec_to_hoon_list, vecnoun_to_hoon_list,
};

pub fn hoon_list_to_sponge(list: Noun) -> Result<[u64; STATE_SIZE], JetErr> {
    if list.is_atom() {
        return jet_err();
    }

    let mut sponge = [0; STATE_SIZE];
    let mut current = list;
    let mut i = 0;

    while current.is_cell() {
        let cell = current.as_cell()?;
        sponge[i] = cell.head().as_atom()?.as_u64()?;
        current = cell.tail();
        i += 1;
    }

    if i != STATE_SIZE {
        return jet_err();
    }

    Ok(sponge)
}

pub fn permutation_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let sample = slot(subject, 6)?;
    let mut sponge = hoon_list_to_sponge(sample)?;
    permute(&mut sponge);

    let new_sponge = vec_to_hoon_list(stack, &sponge);

    Ok(new_sponge)
}

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
        input_vec[i] = montify(input_vec[i]);
    }
}

// calc digest
pub fn tip5_calc_digest(sponge: &[u64; 16]) -> [u64; 5] {
    let mut digest = [0u64; DIGEST_LENGTH];
    for i in 0..DIGEST_LENGTH {
        digest[i] = mont_reduction(sponge[i] as u128).0;
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

pub fn hash_varlen_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let input = slot(subject, 6)?;
    let mut input_vec = hoon_list_to_vecbelt(input)?;

    let digest = hash_varlen(&mut input_vec);

    Ok(vec_to_hoon_list(stack, &digest))
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

pub fn montify_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let sam = slot(subject, 6)?;
    let x = Belt(sam.as_atom()?.as_u64()?);

    let res = montify(x);

    Ok(belt_as_noun(stack, res))
}

fn montify(x: Belt) -> Belt {
    // transform to Montgomery space, i.e. compute x•r = xr mod p
    montiply(x, Belt(R2))
}

pub fn montiply_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let sam = slot(subject, 6)?;
    let a = Belt(sam.as_cell()?.head().as_atom()?.as_u64()?);
    let b = Belt(sam.as_cell()?.tail().as_atom()?.as_u64()?);
    Ok(belt_as_noun(&mut context.stack, montiply(a, b)))
}

fn montiply(a: Belt, b: Belt) -> Belt {
    // computes a*b = (abr^{-1} mod p)
    based!(a.0);
    based!(b.0);
    mont_reduction((a.0 as u128) * (b.0 as u128))
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

    Ok(belt_as_noun(&mut context.stack, mont_reduction(x_u128)))
}

pub fn mont_reduction(x_u128: u128) -> Belt {
    // mont-reduction: computes x•r^{-1} = (xr^{-1} mod p).
    assert!(x_u128 < RP);

    const R_MOD_P1: u128 = (R_MOD_P + 1) as u128; // 4.294.967.296
    const RX: u128 = R; // 18.446.744.073.709.551.616
    const PX: u128 = P as u128; // 0xffffffff00000001

    let x1_u128_div = x_u128 / R_MOD_P1;
    let x1_u128 = x1_u128_div % R_MOD_P1;
    let x2_u128 = x_u128 / RX;
    let x0_u128 = x_u128 % R_MOD_P1;
    let c_u128 = (x0_u128 + x1_u128) * R_MOD_P1;
    let f_u128 = c_u128 / RX;
    let d_u128 = c_u128 - (x1_u128 + (f_u128 * PX));

    let res = if x2_u128 >= d_u128 {
        x2_u128 - d_u128
    } else {
        (x2_u128 + PX) - d_u128
    };

    Belt(res as u64)
}

pub fn hash_belts_list_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let input = slot(subject, 6)?;
    hash_belts_list(stack, input)
}

pub fn hash_belts_list(stack: &mut NockStack, input: Noun) -> Result<Noun, JetErr> {
    let mut input_vec = hoon_list_to_vecbelt(input)?;
    let digest = hash_varlen(&mut input_vec);
    Ok(digest_to_noundigest(stack, digest))
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

    let digest = hash_10(&mut input_vec);

    Ok(vec_to_hoon_list(stack, &digest))
}

fn hash_10(input_vec: &mut Vec<Belt>) -> [u64; 5] {
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
            let digest = hash_10(&mut pair);
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
    do_leaf_sequence(ten_cell, &mut leaf)?;
    let mut leaf_belt = leaf.into_iter().map(Belt).collect();

    // list-to-tuple hash10
    let digest = hash_10(&mut leaf_belt);
    Ok(digest_to_noundigest(stack, digest))
}

pub fn hash_noun_varlen_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let n = slot(subject, 6)?;
    hash_noun_varlen(stack, n)
}

fn hash_noun_varlen(stack: &mut NockStack, n: Noun) -> Result<Noun, JetErr> {
    let leaf = leaf_sequence(stack, n)?;
    let dyck = dyck(stack, n)?;
    let size = lent(leaf).map(|x| D(x as u64))?;

    // [size (weld leaf dyck)]
    let weld = weld(stack, leaf, dyck)?;
    let arg = T(stack, &[size, weld]);

    hash_belts_list(stack, arg)
}

pub fn hash_hashable_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let h = slot(subject, 6)?;

    hash_hashable(stack, h)
}

pub fn hash_hashable(stack: &mut NockStack, h: Noun) -> Result<Noun, JetErr> {
    if !h.is_cell() {
        return jet_err();
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
    hash_noun_varlen(stack, p)
}
fn hash_hashable_list(stack: &mut NockStack, p: Noun) -> Result<Noun, JetErr> {
    let turn: Vec<Noun> = HoonList::try_from(p)?
        .into_iter()
        .map(|x| hash_hashable(stack, x).unwrap())
        .collect();
    let turn_list = vecnoun_to_hoon_list(stack, &turn);
    hash_noun_varlen(stack, turn_list)
}
fn hash_hashable_mary(stack: &mut NockStack, p: Noun) -> Result<Noun, JetErr> {
    let (ma_step, ma_array_len, _ma_array_dat) = get_mary_fields(p)?;

    let ma_changed = change_step(stack, p, D(1))?;
    let [_ma_changed_step, ma_changed_array] = ma_changed.uncell()?; // +$  mary  [step=@ =array]
    let bpoly_list = bpoly_to_list(stack, ma_changed_array)?;
    let hash_belts_list = hash_belts_list(stack, bpoly_list)?;

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

    let bp_big = b_big * UBig::from(P);
    let cp2_big = c_big * UBig::from(P).pow(2);
    let dp3_big = d_big * UBig::from(P).pow(3);
    let ep4_big = e_big * UBig::from(P).pow(4);

    let res: UBig = a_big + bp_big + cp2_big + dp3_big + ep4_big;

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
