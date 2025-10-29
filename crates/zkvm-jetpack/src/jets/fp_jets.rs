use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::Result;
use nockvm::noun::{Cell, IndirectAtom, Noun, D};
use tracing::debug;

use crate::form::felt::Felt;
use crate::form::fpoly::{fp_coseword, fpeval, lift_to_fpoly};
use crate::form::handle::{finalize_poly, new_handle_mut_felt, new_handle_mut_slice};
use crate::form::noun_ext::{AtomMathExt, NounMathExt};
use crate::form::poly::FPolySlice;
use crate::form::structs::HoonList;

pub fn fp_coseword_jet(context: &mut Context, subject: Noun) -> Result {
    let sam = slot(subject, 6)?;
    let p = slot(sam, 2)?;
    let offset = slot(sam, 6)?;
    let order = slot(sam, 7)?;

    let (Ok(p_poly), Ok(offset_felt), Ok(order_atom)) =
        (FPolySlice::try_from(p), offset.as_felt(), order.as_atom())
    else {
        debug!("p not an fpoly, offset not a felt, or order not an atom");
        return Err(BAIL_FAIL);
    };
    let order_32: u32 = order_atom.as_u32()?;
    let root = Felt::ordered_root(order_32 as u64)?;
    let returned_fpoly = fp_coseword(p_poly.0, offset_felt, order_32, &root);

    let (res, res_poly): (IndirectAtom, &mut [Felt]) =
        new_handle_mut_slice(&mut context.stack, Some(returned_fpoly.len() as usize));
    res_poly.copy_from_slice(&returned_fpoly[..]);
    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);

    Ok(res_cell)
}

pub fn init_fpoly_jet(context: &mut Context, subject: Noun) -> Result {
    let poly = slot(subject, 6)?;

    let list_felt = HoonList::try_from(poly)?.into_iter();
    let count = list_felt.count();

    let (res, res_poly): (IndirectAtom, &mut [Felt]) =
        new_handle_mut_slice(&mut context.stack, Some(count as usize));
    for (i, felt_noun) in list_felt.enumerate() {
        let Ok(felt) = felt_noun.as_felt() else {
            debug!("list element not a felt");
            return Err(BAIL_FAIL);
        };
        res_poly[i] = *felt;
    }

    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);

    Ok(res_cell)
}
pub fn fpeval_jet(context: &mut Context, subject: Noun) -> Result {
    let sam = slot(subject, 6)?;
    let fp = slot(sam, 2)?;
    let felt = slot(sam, 3)?;
    let (Ok(fp_poly), Ok(felt)) = (FPolySlice::try_from(fp), felt.as_felt()) else {
        debug!("fp or fq not an fpoly");
        return Err(BAIL_FAIL);
    };
    let (res, res_poly): (IndirectAtom, &mut Felt) = new_handle_mut_felt(&mut context.stack);
    let result = fpeval(fp_poly.0, *felt);
    res_poly.copy_from_slice(&result);

    Ok(res.as_noun())
}

pub fn lift_to_fpoly_jet(context: &mut Context, subject: Noun) -> Result {
    let belt = slot(subject, 6)?;

    let Ok(belts) = HoonList::try_from(belt) else {
        debug!("belts not a list");
        return Err(BAIL_FAIL);
    };
    let belts_iter = belts.into_iter();
    let count = belts_iter.count();

    let (res, res_poly): (IndirectAtom, &mut [Felt]) =
        new_handle_mut_slice(&mut context.stack, Some(count));

    lift_to_fpoly(belts, res_poly);

    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);

    Ok(res_cell)
}

pub fn range_jet(context: &mut Context, subject: Noun) -> Result {
    let sample = slot(subject, 6)?;

    let mut res = D(0);
    let mut dest: *mut Noun = &mut res;

    let start: u64;
    let end: u64;

    if let Ok(atom) = sample.as_atom() {
        start = 0;
        end = atom.as_direct()?.data();
    } else {
        let cell = sample.as_cell()?;
        start = cell.head().as_atom()?.as_direct()?.data();
        end = cell.tail().as_atom()?.as_direct()?.data();
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
