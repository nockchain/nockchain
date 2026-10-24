use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::Result;
use nockvm::noun::{Cell, IndirectAtom, Noun, D};
use tracing::debug;

use crate::form::felt::Felt;
use crate::form::fpoly::{
    bpoly_to_fpoly, fp_coseword, fp_fft, fp_ifft, fp_intercosate, fp_shift, fpadd, fpdiv, fpeval,
    fpmul, fpmul_fast, fpscal, lift_to_fpoly,
};
use crate::form::handle::{finalize_poly, new_handle_mut_felt, new_handle_mut_slice};
use crate::form::noun_ext::{AtomMathExt, NounMathExt};
use crate::form::poly::{BPolySlice, FPolySlice, Poly};
use crate::form::structs::HoonList;

const FPMUL_FAST_MIN_RESULT_LEN: usize = 64;

pub fn fpadd_jet(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let fp = slot(sam, 2, &space)?;
    let fq = slot(sam, 3, &space)?;
    let (Ok(fp_poly), Ok(fq_poly)) = (
        FPolySlice::try_from(fp, &space),
        FPolySlice::try_from(fq, &space),
    ) else {
        debug!("fp or fq not an fpoly");
        return Err(BAIL_FAIL);
    };

    let res_len = std::cmp::max(fp_poly.len(), fq_poly.len());
    let (res, res_poly): (IndirectAtom, &mut [Felt]) =
        new_handle_mut_slice(&mut context.stack, Some(res_len));
    fpadd(fp_poly.0, fq_poly.0, res_poly);
    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);

    Ok(res_cell)
}

pub fn fpscal_jet(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let c = slot(sam, 2, &space)?;
    let fp = slot(sam, 3, &space)?;

    let (Ok(c_felt), Ok(fp_poly)) = (c.as_felt(&space), FPolySlice::try_from(fp, &space)) else {
        debug!("c not a felt or fp not an fpoly");
        return Err(BAIL_FAIL);
    };

    let (res, res_poly): (IndirectAtom, &mut [Felt]) =
        new_handle_mut_slice(&mut context.stack, Some(fp_poly.len()));
    fpscal(c_felt, fp_poly.0, res_poly);
    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);

    Ok(res_cell)
}

pub fn fpmul_jet(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let fp = slot(sam, 2, &space)?;
    let fq = slot(sam, 3, &space)?;

    let (Ok(fp_poly), Ok(fq_poly)) = (
        FPolySlice::try_from(fp, &space),
        FPolySlice::try_from(fq, &space),
    ) else {
        debug!("fp or fq not an fpoly");
        return Err(BAIL_FAIL);
    };

    let res_len = fp_poly.len() + fq_poly.len() - 1;
    let (res, res_poly): (IndirectAtom, &mut [Felt]) =
        new_handle_mut_slice(&mut context.stack, Some(res_len));
    if res_len >= FPMUL_FAST_MIN_RESULT_LEN {
        fpmul_fast(fp_poly.0, fq_poly.0, res_poly);
    } else {
        fpmul(fp_poly.0, fq_poly.0, res_poly);
    }
    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);

    Ok(res_cell)
}

pub fn fpdiv_jet(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let fp = slot(sam, 2, &space)?;
    let fq = slot(sam, 3, &space)?;

    let (Ok(fp_poly), Ok(fq_poly)) = (
        FPolySlice::try_from(fp, &space),
        FPolySlice::try_from(fq, &space),
    ) else {
        debug!("fp or fq not an fpoly");
        return Err(BAIL_FAIL);
    };

    let res_len = if fp_poly.len() < fq_poly.len() {
        1
    } else {
        fp_poly.len() - fq_poly.len() + 1
    };

    let (res, res_poly): (IndirectAtom, &mut [Felt]) =
        new_handle_mut_slice(&mut context.stack, Some(res_len));
    fpdiv(fp_poly.0, fq_poly.0, res_poly);

    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);
    Ok(res_cell)
}

pub fn fp_coseword_jet(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let p = slot(sam, 2, &space)?;
    let offset = slot(sam, 6, &space)?;
    let order = slot(sam, 7, &space)?;

    let (Ok(p_poly), Ok(offset_felt), Ok(order_atom)) = (
        FPolySlice::try_from(p, &space),
        offset.as_felt(&space),
        order.as_atom(),
    ) else {
        debug!("p not an fpoly, offset not a felt, or order not an atom");
        return Err(BAIL_FAIL);
    };
    let order_32: u32 = order_atom.as_u32()?;
    let root = Felt::ordered_root(order_32 as u64)?;
    let returned_fpoly = fp_coseword(p_poly.0, offset_felt, order_32, &root);

    let (res, res_poly): (IndirectAtom, &mut [Felt]) =
        new_handle_mut_slice(&mut context.stack, Some(returned_fpoly.len()));
    res_poly.copy_from_slice(&returned_fpoly[..]);
    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);

    Ok(res_cell)
}

pub fn fp_fft_jet(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let p = slot(subject, 6, &space)?;

    let Ok(p_poly) = FPolySlice::try_from(p, &space) else {
        debug!("p is not an fpoly");
        return Err(BAIL_FAIL);
    };

    let returned_fpoly = fp_fft(p_poly.0)?;
    let (res, res_poly): (IndirectAtom, &mut [Felt]) =
        new_handle_mut_slice(&mut context.stack, Some(returned_fpoly.len()));
    res_poly.copy_from_slice(&returned_fpoly);
    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);

    Ok(res_cell)
}

pub fn fp_ifft_jet(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let p = slot(subject, 6, &space)?;

    let Ok(p_poly) = FPolySlice::try_from(p, &space) else {
        debug!("p is not an fpoly");
        return Err(BAIL_FAIL);
    };

    let returned_fpoly = fp_ifft(p_poly.0)?;
    let (res, res_poly): (IndirectAtom, &mut [Felt]) =
        new_handle_mut_slice(&mut context.stack, Some(returned_fpoly.len()));
    res_poly.copy_from_slice(&returned_fpoly);
    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);

    Ok(res_cell)
}

pub fn fp_shift_jet(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let fp = slot(sam, 2, &space)?;
    let c = slot(sam, 3, &space)?;

    let (Ok(fp_poly), Ok(c_felt)) = (FPolySlice::try_from(fp, &space), c.as_felt(&space)) else {
        debug!("fp not an fpoly or c not a felt");
        return Err(BAIL_FAIL);
    };

    let (res, res_poly): (IndirectAtom, &mut [Felt]) =
        new_handle_mut_slice(&mut context.stack, Some(fp_poly.len()));
    fp_shift(fp_poly.0, c_felt, res_poly);
    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);

    Ok(res_cell)
}

pub fn fp_intercosate_jet(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let offset = slot(sam, 2, &space)?;
    let order = slot(sam, 6, &space)?;
    let p = slot(sam, 7, &space)?;

    let (Ok(p_poly), Ok(offset_felt), Ok(order_atom)) = (
        FPolySlice::try_from(p, &space),
        offset.as_felt(&space),
        order.as_atom(),
    ) else {
        debug!("p not an fpoly, offset not a felt, or order not an atom");
        return Err(BAIL_FAIL);
    };

    let order_32 = order_atom.as_u32()?;
    let returned_fpoly = fp_intercosate(offset_felt, order_32, p_poly.0)?;

    let (res, res_poly): (IndirectAtom, &mut [Felt]) =
        new_handle_mut_slice(&mut context.stack, Some(returned_fpoly.len()));
    res_poly.copy_from_slice(&returned_fpoly);
    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);

    Ok(res_cell)
}

pub fn bpoly_to_fpoly_jet(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let bp_noun = slot(subject, 6, &space)?;

    let Ok(bpoly) = BPolySlice::try_from(bp_noun, &space) else {
        debug!("bpoly not a bpoly");
        return Err(BAIL_FAIL);
    };

    let (res, res_poly): (IndirectAtom, &mut [Felt]) =
        new_handle_mut_slice(&mut context.stack, Some(bpoly.len()));
    bpoly_to_fpoly(bpoly.0, res_poly);

    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);

    Ok(res_cell)
}

pub fn init_fpoly_jet(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let poly = slot(subject, 6, &space)?;

    let list_felt = HoonList::try_from(poly, &space)?.into_iter();
    let count = list_felt.count();

    let (res, res_poly): (IndirectAtom, &mut [Felt]) =
        new_handle_mut_slice(&mut context.stack, Some(count));
    for (i, felt_noun) in list_felt.enumerate() {
        let Ok(felt) = felt_noun.as_felt(&space) else {
            debug!("list element not a felt");
            return Err(BAIL_FAIL);
        };
        res_poly[i] = *felt;
    }

    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);

    Ok(res_cell)
}
pub fn fpeval_jet(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let fp = slot(sam, 2, &space)?;
    let felt = slot(sam, 3, &space)?;
    let (Ok(fp_poly), Ok(felt)) = (FPolySlice::try_from(fp, &space), felt.as_felt(&space)) else {
        debug!("fp or fq not an fpoly");
        return Err(BAIL_FAIL);
    };
    let (res, res_poly): (IndirectAtom, &mut Felt) = new_handle_mut_felt(&mut context.stack);
    let result = fpeval(fp_poly.0, *felt);
    res_poly.copy_from_slice(&result);

    Ok(res.as_noun())
}

pub fn lift_to_fpoly_jet(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let belt = slot(subject, 6, &space)?;

    let Ok(belts) = HoonList::try_from(belt, &space) else {
        debug!("belts not a list");
        return Err(BAIL_FAIL);
    };
    let belts_iter = belts.into_iter();
    let count = belts_iter.count();

    let (res, res_poly): (IndirectAtom, &mut [Felt]) =
        new_handle_mut_slice(&mut context.stack, Some(count));

    lift_to_fpoly(belts, res_poly, &space);

    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);

    Ok(res_cell)
}

pub fn range_jet(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let sample = slot(subject, 6, &space)?;

    let mut res = D(0);
    let mut dest: *mut Noun = &mut res;

    let start: u64;
    let end: u64;

    if let Ok(atom) = sample.in_space(&space).as_atom() {
        start = 0;
        end = atom.atom().as_direct()?.data();
    } else {
        let cell = sample.in_space(&space).as_cell()?;
        start = cell.head().as_atom()?.atom().as_direct()?.data();
        end = cell.tail().as_atom()?.atom().as_direct()?.data();
    }

    for idx in start..end {
        unsafe {
            let (new_cell, new_mem) = Cell::new_raw_mut(&mut context.stack);
            (*new_mem).head = D(idx);
            *dest = new_cell.as_noun();
            dest = &mut (*new_mem).tail;
        }
    }
    unsafe {
        *dest = D(0);
    }

    Ok(res)
}
