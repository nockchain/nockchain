use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_EXIT, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::noun::{IndirectAtom, Noun, D, T};
use nockvm::site::{site_slam, Site};
use tracing::debug;

use crate::form::felt::*;
use crate::form::handle::new_handle_mut_felt;
use crate::form::noun_ext::NounMathExt;
use crate::utils::*;

pub fn zip_roll_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sample = slot(subject, 6, &space)?;
    let mut list_a = slot(sample, 2, &space)?;
    let mut list_b = slot(sample, 6, &space)?;
    let mut gate = slot(sample, 7, &space)?;
    let mut prod = slot(gate, 13, &space)?;

    let site = Site::new(context, &mut gate);
    loop {
        if let Ok(list_a_cell) = list_a.in_space(&space).as_cell() {
            if let Ok(list_b_cell) = list_b.in_space(&space).as_cell() {
                list_a = list_a_cell.tail().noun();
                list_b = list_b_cell.tail().noun();
                let left_sam = T(
                    &mut context.stack,
                    &[list_a_cell.head().noun(), list_b_cell.head().noun()],
                );
                let sam = T(&mut context.stack, &[left_sam, prod]);
                prod = site_slam(context, &site, sam)?;
            } else {
                debug!("list_a and list_b sizes unequal");
                return Err(BAIL_EXIT);
            }
        } else {
            if unsafe { !list_a.raw_equals(&D(0)) } {
                return Err(BAIL_EXIT);
            }
            if unsafe { !list_b.raw_equals(&D(0)) } {
                return Err(BAIL_EXIT);
            }
            return Ok(prod);
        }
    }
}

pub fn fadd_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let a = slot(sam, 2, &space)?;
    let b = slot(sam, 3, &space)?;

    let (Ok(a_felt), Ok(b_felt)) = (a.as_felt(&space), b.as_felt(&space)) else {
        debug!("a or b not a felt");
        return Err(BAIL_FAIL);
    };
    let (res_atom, res_felt): (IndirectAtom, &mut Felt) = new_handle_mut_felt(&mut context.stack);
    fadd(a_felt, b_felt, res_felt);

    assert!(felt_atom_is_valid(res_atom, &space));
    Ok(res_atom.as_noun())
}

pub fn fsub_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let a = slot(sam, 2, &space)?;
    let b = slot(sam, 3, &space)?;

    let (Ok(a_felt), Ok(b_felt)) = (a.as_felt(&space), b.as_felt(&space)) else {
        debug!("a or b not a felt");
        return Err(BAIL_FAIL);
    };
    let (res_atom, res_felt): (IndirectAtom, &mut Felt) = new_handle_mut_felt(&mut context.stack);
    fsub(a_felt, b_felt, res_felt);

    assert!(felt_atom_is_valid(res_atom, &space));
    Ok(res_atom.as_noun())
}

pub fn fneg_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let a = slot(subject, 6, &space)?;

    let Ok(a_felt) = a.as_felt(&space) else {
        debug!("a not a felt");
        return Err(BAIL_FAIL);
    };
    let (res_atom, res_felt): (IndirectAtom, &mut Felt) = new_handle_mut_felt(&mut context.stack);
    fneg(a_felt, res_felt);

    assert!(felt_atom_is_valid(res_atom, &space));
    Ok(res_atom.as_noun())
}

pub fn fmul_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let a = slot(sam, 2, &space)?;
    let b = slot(sam, 3, &space)?;

    let (Ok(a_felt), Ok(b_felt)) = (a.as_felt(&space), b.as_felt(&space)) else {
        debug!("a or b not a felt");
        return Err(BAIL_FAIL);
    };
    let (res_atom, res_felt): (IndirectAtom, &mut Felt) = new_handle_mut_felt(&mut context.stack);
    fmul(a_felt, b_felt, res_felt);

    assert!(felt_atom_is_valid(res_atom, &space));
    Ok(res_atom.as_noun())
}

pub fn finv_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let a = slot(subject, 6, &space)?;

    let Ok(a_felt) = a.as_felt(&space) else {
        debug!("a is not a felt");
        return Err(BAIL_FAIL);
    };
    let (res_atom, res_felt): (IndirectAtom, &mut Felt) = new_handle_mut_felt(&mut context.stack);
    finv(a_felt, res_felt);

    assert!(felt_atom_is_valid(res_atom, &space));
    Ok(res_atom.as_noun())
}

pub fn fdiv_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let a = slot(sam, 2, &space)?;
    let b = slot(sam, 3, &space)?;

    let (Ok(a_felt), Ok(b_felt)) = (a.as_felt(&space), b.as_felt(&space)) else {
        debug!("a or b not felts");
        return Err(BAIL_FAIL);
    };
    let (res_atom, res_felt): (IndirectAtom, &mut Felt) = new_handle_mut_felt(&mut context.stack);
    fdiv(a_felt, b_felt, res_felt);

    assert!(felt_atom_is_valid(res_atom, &space));
    Ok(res_atom.as_noun())
}

pub fn fpow_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let x = slot(sam, 2, &space)?;
    let n = slot(sam, 3, &space)?;

    let (Ok(x_felt), Ok(n_atom)) = (x.as_felt(&space), n.in_space(&space).as_atom()) else {
        debug!("x not a felt or n not an atom");
        return Err(BAIL_FAIL);
    };
    let n_64 = n_atom.as_u64()?;
    let (res_atom, res_felt): (IndirectAtom, &mut Felt) = new_handle_mut_felt(&mut context.stack);
    fpow(x_felt, n_64, res_felt);

    assert!(felt_atom_is_valid(res_atom, &space));
    Ok(res_atom.as_noun())
}
