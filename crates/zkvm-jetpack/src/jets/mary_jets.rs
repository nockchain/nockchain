use nockvm::interpreter::Context;
use nockvm::jets::bits::rep;
use nockvm::jets::bits::util::{lsh, rip};
use nockvm::jets::list::util::{lent, reap, snip, weld, zing};
use nockvm::jets::math::util::add;
use nockvm::jets::util::{bite_to_word, chop, slot, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::mem::NockStack;
use nockvm::noun::{Atom, IndirectAtom, Noun, D, NO, T, YES};
use nockvm_macros::tas;
use tracing::{debug, error};

use crate::form::belt::*;
use crate::form::handle::{
    finalize_mary, finalize_poly, new_handle_mut_mary, new_handle_mut_slice,
};
use crate::form::mary::*;
use crate::form::noun_ext::{AtomMathExt, NounMathExt};
use crate::form::shape::leaf_sequence;
use crate::form::structs::HoonList;
use crate::form::tip5::DIGEST_LENGTH;
use crate::jets::base_jets::{levy_based, rip_correct};
use crate::jets::bp_jets::init_bpoly;
use crate::jets::tip5_jets::{digest_to_noundigest, hash_hashable, hash_pairs};
use crate::utils::vecnoun_to_hoon_list;

pub fn mary_swag_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let door = slot(subject, 7)?;
    let ma = slot(door, 6)?;
    let sam = slot(subject, 6)?;
    let i = sam.as_cell()?.head().as_direct()?.data() as usize;
    let j = sam.as_cell()?.tail().as_direct()?.data() as usize;

    let Ok(mary) = MarySlice::try_from(ma) else {
        debug!("cannot convert mary arg to mary");
        return Err(BAIL_FAIL);
    };

    let (res, res_poly): (IndirectAtom, MarySliceMut) =
        new_handle_mut_mary(&mut context.stack, mary.step as usize, j);
    let step = mary.step as usize;

    res_poly
        .dat
        .copy_from_slice(&mary.dat[(i * step)..(i + j) * step]);

    let res_cell = finalize_mary(&mut context.stack, step, j, res);
    Ok(res_cell)
}

pub fn mary_weld_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let door = slot(subject, 7)?;
    let ma = slot(door, 6)?;
    let ma2 = slot(subject, 6)?;

    let step = ma.as_cell()?.head().as_direct()?.data() as u32;
    let step2 = ma2.as_cell()?.head().as_direct()?.data() as u32;
    if step != step2 {
        debug!("can only weld marys of same step");
        return Err(BAIL_FAIL);
    }

    let (Ok(mary1), Ok(mary2)) = (MarySlice::try_from(ma), MarySlice::try_from(ma2)) else {
        debug!("mary1 or mary2 is not an fpoly");
        return Err(BAIL_FAIL);
    };
    let res_len = mary1.len + mary2.len;
    let (res, res_poly): (IndirectAtom, MarySliceMut) =
        new_handle_mut_mary(&mut context.stack, step as usize, res_len as usize);

    mary_weld(mary1, mary2, res_poly);
    let res_cell = finalize_mary(&mut context.stack, step as usize, res_len as usize, res);
    Ok(res_cell)
}

pub fn mary_transpose_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let door = slot(subject, 7)?;
    let ma = slot(door, 6)?;
    let offset = slot(subject, 6)?;

    let (Ok(mary), Ok(offset)) = (MarySlice::try_from(ma), offset.as_atom()?.as_u64()) else {
        debug!("fp is not an fpoly or n is not an atom");
        return Err(BAIL_FAIL);
    };

    let offset = offset as usize;

    let (res, mut res_poly): (IndirectAtom, MarySliceMut) = new_handle_mut_mary(
        &mut context.stack,
        mary.len as usize * offset,
        mary.step as usize / offset,
    );

    mary_transpose(mary, offset, &mut res_poly);

    let res_cell = finalize_mary(
        &mut context.stack, res_poly.step as usize, res_poly.len as usize, res,
    );

    Ok(res_cell)
}

pub fn lift_elt_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let door = slot(subject, 7)?;
    let step = slot(door, 6)?.as_atom()?.as_u64()?;
    let a = slot(subject, 6)?;

    if step == 1u64 {
        Ok(a)
    } else {
        let reap_res = reap(stack, step - 1, D(0))?;
        let init_bpoly_arg = T(stack, &[a, reap_res]);
        let init_bpoly_arg_list = HoonList::try_from(init_bpoly_arg)?;

        let count = init_bpoly_arg_list.count();
        let (res, res_poly): (IndirectAtom, &mut [Belt]) = new_handle_mut_slice(stack, Some(count));
        init_bpoly(init_bpoly_arg_list, res_poly);

        let res_cell = finalize_poly(stack, Some(res_poly.len()), res);
        Ok(res_cell.as_cell()?.tail())
    }
}

pub fn fet_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let door = slot(subject, 7)?;
    let step = slot(door, 6)?.as_atom()?.as_u64()?;
    let a = slot(subject, 6)?.as_atom()?;

    let v = rip_correct(stack, 6, 1, a)?;

    let lent_v = lent(v)? as u64;

    if ((lent_v == 1) && (step == 1)) || (lent_v == (step + 1)) && levy_based(v) {
        Ok(YES)
    } else {
        Ok(NO)
    }
}

pub fn transpose_bpolys_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let sam = slot(subject, 6)?;
    let bpolys = MarySlice::try_from(sam).expect("cannot convert bpolys arg");
    transpose_bpolys(context, bpolys)
}

fn transpose_bpolys(context: &mut Context, bpolys: MarySlice) -> Result<Noun, JetErr> {
    let offset = 1;

    let (res, mut res_poly): (IndirectAtom, MarySliceMut) = new_handle_mut_mary(
        &mut context.stack,
        bpolys.len as usize * offset,
        bpolys.step as usize / offset,
    );

    mary_transpose(bpolys, offset, &mut res_poly);

    let res_cell = finalize_mary(
        &mut context.stack, res_poly.step as usize, res_poly.len as usize, res,
    );

    Ok(res_cell)
}

pub fn snag_one_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let door = slot(subject, 7)?;
    let mary_noun = slot(door, 6)?;
    let i = slot(subject, 6)?.as_direct()?.data() as usize;

    snag_one(stack, mary_noun, i)
}

// snag from ave
pub fn snag_one(stack: &mut NockStack, mary_noun: Noun, i: usize) -> Result<Noun, JetErr> {
    let mary_cell = mary_noun.as_cell()?;
    let ma_step = mary_cell.head().as_atom()?.as_u32()?;
    let ma_len = mary_cell.tail().as_cell()?.head().as_atom()?.as_u32()?;
    let ma_dat: Atom = mary_cell.tail().as_cell()?.tail().as_atom()?;

    assert!(i < ma_len as usize);
    snag_one_fields(stack, i, ma_step, ma_dat)
}

// snag from ave with separate fields
pub fn snag_one_fields(
    stack: &mut NockStack,
    i: usize,
    ma_step: u32,
    ma_dat: Atom,
) -> Result<Noun, JetErr> {
    let res = cut(stack, 6, i * ma_step as usize, ma_step as usize, ma_dat)?;
    if ma_step == 1 {
        return Ok(res);
    }
    let high_bit = lsh(stack, 0, bex(6) * ma_step as usize, D(1).as_atom()?)?;

    Ok(add(stack, high_bit.as_atom()?, res.as_atom()?).as_noun())
}

// cut from hoon-138
fn cut(
    stack: &mut NockStack,
    bloq: usize,
    start: usize,
    run: usize,
    atom: Atom,
) -> Result<Noun, JetErr> {
    if run == 0 {
        return Ok(D(0));
    }

    let new_indirect = unsafe {
        let (mut new_indirect, new_slice) =
            IndirectAtom::new_raw_mut_bitslice(stack, bite_to_word(bloq, run)?);
        chop(bloq, start, run, 0, new_slice, atom.as_bitslice())?;
        new_indirect.normalize_as_atom()
    };
    Ok(new_indirect.as_noun())
}
fn bex(arg: usize) -> usize {
    if arg >= 63 {
        error!("simple bex implementation only valid for arg <63 !!");
    }
    1 << arg
}

pub fn snag_as_bpoly_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let door = slot(subject, 7)?;
    let mary_noun = slot(door, 6)?;
    let i = slot(subject, 6)?.as_direct()?.data() as usize;

    snag_as_bpoly(stack, mary_noun, i)
}

pub fn snag_as_bpoly(stack: &mut NockStack, mary_noun: Noun, i: usize) -> Result<Noun, JetErr> {
    let mary_cell = mary_noun.as_cell()?;
    let ma_step = mary_cell.head().as_atom()?.as_u32()?;

    let dat = snag_one(stack, mary_noun, i)?;

    if ma_step == 1 {
        let step = bex(6) * ma_step as usize;
        let high_bit = lsh(stack, 0, step, D(1).as_atom()?)?;
        let res_add = add(stack, high_bit.as_atom()?, dat.as_atom()?).as_noun();
        return Ok(T(stack, &[D(ma_step as u64), res_add]));
    }

    Ok(T(stack, &[D(ma_step as u64), dat]))
}

pub fn change_step_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let door = slot(subject, 7)?;
    let ma_noun = slot(door, 6)?;
    let new_step_noun = slot(subject, 6)?;

    change_step(stack, ma_noun, new_step_noun)
}

pub fn change_step(
    stack: &mut NockStack,
    ma_noun: Noun,
    new_step_noun: Noun,
) -> Result<Noun, JetErr> {
    let new_step = new_step_noun.as_atom()?.as_u64()?; //   |=  [new-step=@]  ??

    let [ma_step_noun, ma_array] = ma_noun.uncell()?; // +$  mary  [step=@ =array]
    let [array_len_noun, array_dat] = ma_array.uncell()?; // +$  array  [len=@ dat=@ux]

    let ma_step = ma_step_noun.as_atom()?.as_u64()?;
    let array_len = array_len_noun.as_atom()?.as_u64()?;

    if ma_step == new_step {
        return Ok(ma_noun);
    }
    assert_eq!(0, (ma_step * array_len) % new_step);

    let res1 = D((ma_step * array_len) / new_step);
    let res = T(stack, &[new_step_noun, res1, array_dat]);
    Ok(res)
}

pub fn bp_build_merk_heap_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let mary_noun = slot(subject, 6)?;

    let (_ma_step, ma_array_len, _ma_array_dat) = get_mary_fields(mary_noun)?;
    let heap_mary = heapify_mary(stack, mary_noun)?;
    let xeb_m = simple_xeb(ma_array_len.as_u64()? as usize);

    let snag_digest = snag_as_digest(stack, heap_mary, 0)?;

    let res1 = T(stack, &[snag_digest, heap_mary]);
    let res = T(stack, &[D(xeb_m as u64), res1]);
    Ok(res)
}

fn simple_xeb(n: usize) -> usize {
    if n == 0 {
        0
    } else {
        (64 - n.leading_zeros()) as usize
    }
}

pub fn get_mary_fields(p: Noun) -> Result<(Atom, Atom, Noun), JetErr> {
    let [ma_step, ma_array] = p.uncell()?; // +$  mary  [step=@ =array]
    let [ma_array_len, ma_array_dat] = ma_array.uncell()?; // +$  array  [len=@ dat=@ux]
    Ok((ma_step.as_atom()?, ma_array_len.as_atom()?, ma_array_dat))
}

fn heapify_mary(stack: &mut NockStack, m_noun: Noun) -> Result<Noun, JetErr> {
    let (_ma_step, ma_array_len, _ma_array_dat) = get_mary_fields(m_noun)?;
    let size = bex(simple_xeb(ma_array_len.as_u64()? as usize)) - 1;

    // calc high-bit
    let high_bit = lsh(stack, 6, size * 5, D(1).as_atom()?)?.as_atom()?;

    // make leaves
    let mut res_vec: Vec<Noun> = Vec::new();
    for i in 0..ma_array_len.as_u64()? {
        let t = snag_as_bpoly(stack, m_noun, i as usize)?;
        let hashable_bpoly = T(stack, &[D(tas!(b"mary")), D(1), t]);
        let hash = hash_hashable(stack, hashable_bpoly)?;
        let leafs = leaf_sequence(stack, hash)?;
        res_vec.push(leafs);
    }
    let mut res = vecnoun_to_hoon_list(stack, res_vec.as_slice());

    let mut curr = res;
    loop {
        let lent_curr = lent(curr)?;
        if lent_curr == 1 {
            break;
        } else {
            let pairs = hash_pairs(stack, curr)?;
            res = weld(stack, pairs, res)?;
            curr = pairs;
        }
    }

    let a = zing(stack, res)?;
    let b = rep(stack, D(6), a)?;
    let c = add(stack, high_bit, b.as_atom()?);
    let res = T(stack, &[D(5), D(size as u64), c.as_noun()]);

    Ok(res)
}

pub fn snag_as_digest_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let sam = slot(subject, 6)?;
    let m_noun = slot(sam, 2)?;
    let i_noun = slot(sam, 3)?;

    let i = i_noun.as_atom()?.as_u64()? as usize;
    snag_as_digest(stack, m_noun, i)
}

fn snag_as_digest(stack: &mut NockStack, m_noun: Noun, i: usize) -> Result<Noun, JetErr> {
    let buf = snag_one(stack, m_noun, i)?.as_atom()?;

    let mut digest = [0u64; DIGEST_LENGTH];
    digest[0] = cut(stack, 6, 0, 1, buf)?.as_atom()?.as_u64()?;
    digest[1] = cut(stack, 6, 1, 1, buf)?.as_atom()?.as_u64()?;
    digest[2] = cut(stack, 6, 2, 1, buf)?.as_atom()?.as_u64()?;
    digest[3] = cut(stack, 6, 3, 1, buf)?.as_atom()?.as_u64()?;
    digest[4] = cut(stack, 6, 4, 1, buf)?.as_atom()?.as_u64()?;

    Ok(digest_to_noundigest(stack, digest))
}

pub fn mary_to_list_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let ma_noun = slot(subject, 6)?;
    mary_to_list(stack, ma_noun)
}

pub fn mary_to_list(stack: &mut NockStack, ma_noun: Noun) -> Result<Noun, JetErr> {
    let (ma_step, ma_array_len, ma_array_dat) = get_mary_fields(ma_noun)?;
    let ma_step = ma_step.as_u64()? as usize;

    mary_to_list_fields(stack, ma_array_len, ma_array_dat, ma_step)
}

pub fn mary_to_list_fields(
    stack: &mut NockStack,
    ma_array_len: Atom,
    ma_array_dat: Noun,
    ma_step: usize,
) -> Result<Noun, JetErr> {
    if ma_array_len.as_u64()? == 0 {
        return Ok(D(0));
    }

    let res_rip = rip(stack, 6, ma_step, ma_array_dat.as_atom()?)?;
    let res_snip = snip(stack, res_rip)?;

    let mut res_turn: Vec<Noun> = Vec::new();
    for elem in HoonList::try_from(res_snip)?.into_iter() {
        //%+  add  elem
        //let x = elem +
        let res_wutcol = if ma_step == 1 {
            D(0)
        } else {
            lsh(stack, 6, ma_step, D(1).as_atom()?)?
        };

        let res_add = add(stack, elem.as_atom()?, res_wutcol.as_atom()?);
        res_turn.push(res_add.as_noun());
    }

    Ok(vecnoun_to_hoon_list(stack, res_turn.as_slice()))
}
