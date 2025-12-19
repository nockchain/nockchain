use nockvm::interpreter::Context;
use nockvm::jets::list::util::flop;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::{JetErr, Result};
use nockvm::mem::NockStack;
use nockvm::noun::{Atom, Cell, IndirectAtom, Noun, D, NO, T, YES};
use tracing::debug;

use crate::form::belt::*;
use crate::form::bpoly::*;
use crate::form::felt::{fadd, fmul, Felt};
use crate::form::handle::*;
use crate::form::noun_ext::{AtomMathExt, NounMathExt};
use crate::form::poly::*;
use crate::form::structs::HoonList;
use crate::jets::fpntt_jets::{felt_as_noun, felt_from_u64s};
use crate::jets::mary_jets::{mary_to_list_fields, snag_one_fields};
use crate::utils::is_hoon_list_end;

pub fn bpoly_to_list_jet(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let sam = slot(subject, 6)?;
    bpoly_to_list(stack, sam)
}

pub fn bpoly_to_list(stack: &mut NockStack, sam: Noun) -> Result {
    let Ok(sam_bpoly) = BPolySlice::try_from(sam) else {
        return Err(BAIL_FAIL);
    };

    //  empty list is a null atom
    let mut res_list = D(0);

    let len = sam_bpoly.len();

    if len == 0 {
        return Ok(res_list);
    }

    for i in (0..len).rev() {
        let res_atom = Atom::new(stack, sam_bpoly.0[i].into());
        res_list = T(stack, &[res_atom.as_noun(), res_list]);
    }

    Ok(res_list)
}

pub fn bpadd_jet(context: &mut Context, subject: Noun) -> Result {
    let sam = slot(subject, 6)?;
    let bp = slot(sam, 2)?;
    let bq = slot(sam, 3)?;

    let (Ok(bp_poly), Ok(bq_poly)) = (BPolySlice::try_from(bp), BPolySlice::try_from(bq)) else {
        return Err(BAIL_FAIL);
    };

    let res_len = std::cmp::max(bp_poly.len(), bq_poly.len());
    let (res, res_poly): (IndirectAtom, &mut [Belt]) =
        new_handle_mut_slice(&mut context.stack, Some(res_len));
    bpadd(bp_poly.0, bq_poly.0, res_poly);

    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);

    Ok(res_cell)
}

pub fn bpneg_jet(context: &mut Context, subject: Noun) -> Result {
    let bp = slot(subject, 6)?;

    let Ok(bp_poly) = BPolySlice::try_from(bp) else {
        return Err(BAIL_FAIL);
    };

    let (res, res_poly): (IndirectAtom, &mut [Belt]) =
        new_handle_mut_slice(&mut context.stack, Some(bp_poly.len()));
    bpneg(bp_poly.0, res_poly);

    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);

    Ok(res_cell)
}

pub fn bpsub_jet(context: &mut Context, subject: Noun) -> Result {
    let sam = slot(subject, 6)?;
    let p = slot(sam, 2)?;
    let q = slot(sam, 3)?;

    let (Ok(p_poly), Ok(q_poly)) = (BPolySlice::try_from(p), BPolySlice::try_from(q)) else {
        return Err(BAIL_FAIL);
    };

    let res_len = std::cmp::max(p_poly.len(), q_poly.len());
    let (res, res_poly): (IndirectAtom, &mut [Belt]) =
        new_handle_mut_slice(&mut context.stack, Some(res_len));
    bpsub(p_poly.0, q_poly.0, res_poly);

    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);

    Ok(res_cell)
}

pub fn bpscal_jet(context: &mut Context, subject: Noun) -> Result {
    let sam = slot(subject, 6)?;
    let c = slot(sam, 2)?;
    let bp = slot(sam, 3)?;
    let (Ok(c_atom), Ok(bp_poly)) = (c.as_atom(), BPolySlice::try_from(bp)) else {
        return Err(BAIL_FAIL);
    };
    let c_64 = c_atom.as_u64()?;

    let (res, res_poly): (IndirectAtom, &mut [Belt]) =
        new_handle_mut_slice(&mut context.stack, Some(bp_poly.len()));
    bpscal(Belt(c_64), bp_poly.0, res_poly);

    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);

    Ok(res_cell)
}

pub fn bpmul_jet(context: &mut Context, subject: Noun) -> Result {
    let sam = slot(subject, 6)?;
    let bp = slot(sam, 2)?;
    let bq = slot(sam, 3)?;

    let (Ok(bp_poly), Ok(bq_poly)) = (BPolySlice::try_from(bp), BPolySlice::try_from(bq)) else {
        return Err(BAIL_FAIL);
    };

    let res_len = if bp_poly.is_zero() | bq_poly.is_zero() {
        1
    } else {
        bp_poly.len() + bq_poly.len() - 1
    };

    let (res_atom, res_poly): (IndirectAtom, &mut [Belt]) =
        new_handle_mut_slice(&mut context.stack, Some(res_len));

    bpmul(bp_poly.0, bq_poly.0, res_poly);
    let res_cell = finalize_poly(&mut context.stack, Some(res_len), res_atom);

    Ok(res_cell)
}

pub fn bp_hadamard_jet(context: &mut Context, subject: Noun) -> Result {
    let sam = slot(subject, 6)?;
    let bp = slot(sam, 2)?;
    let bq = slot(sam, 3)?;

    let (Ok(bp_poly), Ok(bq_poly)) = (BPolySlice::try_from(bp), BPolySlice::try_from(bq)) else {
        return Err(BAIL_FAIL);
    };
    assert_eq!(bp_poly.len(), bq_poly.len());
    let res_len = bp_poly.len();
    let (res, res_poly): (IndirectAtom, &mut [Belt]) =
        new_handle_mut_slice(&mut context.stack, Some(res_len));
    bp_hadamard(bp_poly.0, bq_poly.0, res_poly);

    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);

    Ok(res_cell)
}

pub fn bp_ntt_jet(context: &mut Context, subject: Noun) -> Result {
    let sam = slot(subject, 6)?;
    let bp = slot(sam, 2)?;
    let root = slot(sam, 3)?;

    let (Ok(bp_poly), Ok(root_atom)) = (BPolySlice::try_from(bp), root.as_atom()) else {
        return Err(BAIL_FAIL);
    };
    let root_64 = root_atom.as_u64()?;
    let returned_bpoly = bp_ntt(bp_poly.0, &Belt(root_64));
    // TODO: preallocate and pass res buffer into bp_ntt?
    let (res_atom, res_poly): (IndirectAtom, &mut [Belt]) =
        new_handle_mut_slice(&mut context.stack, Some(returned_bpoly.len() as usize));
    res_poly.copy_from_slice(&returned_bpoly[..]);

    let res_cell: Noun = finalize_poly(&mut context.stack, Some(res_poly.len()), res_atom);

    Ok(res_cell)
}

pub fn bp_fft_jet(context: &mut Context, subject: Noun) -> Result {
    let p = slot(subject, 6)?;

    let Ok(p_poly) = BPolySlice::try_from(p) else {
        return Err(BAIL_FAIL);
    };
    let returned_bpoly = bp_fft(p_poly.0)?;
    let (res_atom, res_poly): (IndirectAtom, &mut [Belt]) =
        new_handle_mut_slice(&mut context.stack, Some(returned_bpoly.len() as usize));

    res_poly.copy_from_slice(&returned_bpoly);

    let res_cell: Noun = finalize_poly(&mut context.stack, Some(res_poly.len()), res_atom);

    Ok(res_cell)
}

pub fn bp_shift_jet(context: &mut Context, subject: Noun) -> Result {
    let sam = slot(subject, 6)?;
    let bp = slot(sam, 2)?;
    let c = slot(sam, 3)?;

    let (Ok(bp_poly), Ok(c_belt)) = (BPolySlice::try_from(bp), c.as_belt()) else {
        return Err(BAIL_FAIL);
    };
    let (res_atom, res_poly): (IndirectAtom, &mut [Belt]) =
        new_handle_mut_slice(&mut context.stack, Some(bp_poly.len()));
    bp_shift(bp_poly.0, &c_belt, res_poly);

    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res_atom);

    Ok(res_cell)
}

pub fn bp_coseword_jet(context: &mut Context, subject: Noun) -> Result {
    let sam = slot(subject, 6)?;
    let p = slot(sam, 2)?;
    let offset = slot(sam, 6)?;
    let order = slot(sam, 7)?;

    let (Ok(p_poly), Ok(offset_belt), Ok(order_atom)) =
        (BPolySlice::try_from(p), offset.as_belt(), order.as_atom())
    else {
        return Err(BAIL_FAIL);
    };
    let order_32: u32 = order_atom.as_u32()?;
    let root = Belt(order_32 as u64).ordered_root()?;
    let returned_bpoly = bp_coseword(p_poly.0, &offset_belt, order_32, &root);
    let (res, res_poly): (IndirectAtom, &mut [Belt]) =
        new_handle_mut_slice(&mut context.stack, Some(returned_bpoly.len() as usize));
    res_poly.copy_from_slice(&returned_bpoly);
    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);

    Ok(res_cell)
}

pub fn init_bpoly_jet(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let poly = slot(subject, 6)?;

    let list_belt = HoonList::try_from(poly)?.into_iter();
    let count = list_belt.count();
    let (res, res_poly): (IndirectAtom, &mut [Belt]) = new_handle_mut_slice(stack, Some(count));
    init_bpoly(list_belt, res_poly);

    let res_cell = finalize_poly(stack, Some(res_poly.len()), res);
    Ok(res_cell)
}

pub fn init_bpoly(list_belt: HoonList, res_poly: &mut [Belt]) {
    for (i, belt_noun) in list_belt.enumerate() {
        let belt = belt_noun.as_belt().expect("error at as_belt");
        res_poly[i] = belt;
    }
}

//-------------------------------------------------------------------------
//

pub fn bp_is_zero_jet(_context: &mut Context, subject: Noun) -> Result {
    let p = slot(subject, 6)?;

    if bp_is_zero(p) {
        Ok(YES)
    } else {
        Ok(NO)
    }
}

pub fn bp_is_zero(p: Noun) -> bool {
    let p_slice = BPolySlice::try_from(p).expect("invalid p");
    p_slice.is_zero()
}

// lift: the unique lift of a base field element into an extension field (Belt -> Felt)
fn lift(belt: Belt) -> Felt {
    felt_from_u64s(belt.0, 0, 0)
}

pub fn get_bpoly_fields(bpoly: Noun) -> std::result::Result<(Atom, Atom), JetErr> {
    let [bpoly_len, bpoly_dat] = bpoly.uncell()?; // +$  bpoly  [len=@ dat=@ux]
    Ok((bpoly_len.as_atom()?, bpoly_dat.as_atom()?))
}

// bpeval-lift: evaluate a bpoly at a felt
pub fn bpeval_lift_jet(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let sam = slot(subject, 6)?;
    let [bp, x_noun] = sam.uncell()?; // TODO defaults? [bp=`bpoly`one-bpoly x=`felt`(lift 1)]
    let x = x_noun.as_felt()?;

    let lift0 = lift(Belt(0));

    if bp_is_zero(bp) {
        return felt_as_noun(context, lift0);
    }

    let (bp_len, bp_dat) = get_bpoly_fields(bp)?;

    if bp_len.as_u64()? == 1 {
        return snag_one_fields(stack, 0, 1, bp_dat);
    }

    let p = mary_to_list_fields(stack, bp_len, bp_dat.as_noun(), 1)?;
    let mut p = flop(stack, p)?;
    let mut res = lift0;
    loop {
        if is_hoon_list_end(&p) {
            return Err(BAIL_FAIL);
        }

        let p_cell = p.as_cell()?;
        let res_lift = lift(p_cell.head().as_belt()?);
        let mut res_fmul = Felt::zero();
        fmul(&res, x, &mut res_fmul);
        let mut res_add = Felt::zero();
        fadd(&res_fmul, &res_lift, &mut res_add);

        if is_hoon_list_end(&p_cell.tail()) {
            return felt_as_noun(context, res_add);
        }

        res = res_add;
        p = p_cell.tail();
    }
}

pub fn bpdvr_jet(context: &mut Context, subject: Noun) -> Result {
    let sam = slot(subject, 6)?;
    let ba = slot(sam, 2)?;
    let bb = slot(sam, 3)?;

    let (Ok(ba_poly), Ok(bb_poly)) = (BPolySlice::try_from(ba), BPolySlice::try_from(bb)) else {
        debug!("ba or bb was not a bpoly");
        return Err(BAIL_FAIL);
    };

    if bb_poly.is_zero() {
        debug!("divide by zero");
        return Err(BAIL_FAIL);
    }

    let ba_deg = ba_poly.degree();
    let bb_deg = bb_poly.degree();
    let q_deg = ba_deg.saturating_sub(bb_deg);

    let (q_len, r_len) = if ba_poly.is_zero() {
        (1, 1)
    } else {
        (q_deg + 1, bb_deg + 1)
    };

    let (q_atom, q_poly): (IndirectAtom, &mut [Belt]) =
        new_handle_mut_slice(&mut context.stack, Some(q_len as usize));

    let (r_cell, r_poly): (IndirectAtom, &mut [Belt]) =
        new_handle_mut_slice(&mut context.stack, Some(r_len as usize));

    bpdvr(ba_poly.0, bb_poly.0, q_poly, r_poly);

    let res_cell_q = finalize_poly(&mut context.stack, Some(q_len as usize), q_atom);

    let r_final_len = r_poly.degree() + 1;
    let res_cell_r = finalize_poly(&mut context.stack, Some(r_final_len as usize), r_cell);

    Ok(Cell::new(&mut context.stack, res_cell_q, res_cell_r).as_noun())
}
