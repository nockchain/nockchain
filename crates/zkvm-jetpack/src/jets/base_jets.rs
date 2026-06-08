use nockchain_math::belt::*;
use nockvm::interpreter::Context;
use nockvm::jets::bits::util::rip;
use nockvm::jets::util::{bite, slot, BAIL_FAIL};
use nockvm::jets::Result;
use nockvm::mem::NockStack;
use nockvm::noun::{Atom, Noun, NounSpace, D, NO, T, YES};
use tracing::debug;

// base field jets
//
// When possible, all these functions do is get the sample from the subject,
// convert them into the appropriate datatypes, allocate space for a result,
// hand off the actual business logic elsewhere, and then return the result.
//
// In some cases, like bpmul_jet, this can result in a little more work being
// done than strictly necessary. We could, e.g., check that a polynomial is
// zero and then shortcircuit calling bpmul by just returning zero. Instead,
// we allocate space for a polynomial of sufficient size without checking
// whether either is zero, and then bpmul does the zero check. While this is
// inefficient, it makes division of labor clear.

pub fn badd_jet(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let a = slot(sam, 2, &space)?;
    let b = slot(sam, 3, &space)?;

    let (Ok(a_atom), Ok(b_atom)) = (a.as_atom(), b.as_atom()) else {
        debug!("a or b was not an atom");
        return Err(BAIL_FAIL);
    };
    let (a_belt, b_belt): (Belt, Belt) = (
        a_atom.in_space(&space).as_u64()?.into(),
        b_atom.in_space(&space).as_u64()?.into(),
    );
    Ok(Atom::new(&mut context.stack, (a_belt + b_belt).into()).as_noun())
}

pub fn bsub_jet(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let a = slot(sam, 2, &space)?;
    let b = slot(sam, 3, &space)?;

    let (Ok(a_atom), Ok(b_atom)) = (a.as_atom(), b.as_atom()) else {
        debug!("a or b was not an atom");
        return Err(BAIL_FAIL);
    };
    let (a_belt, b_belt): (Belt, Belt) = (
        a_atom.in_space(&space).as_u64()?.into(),
        b_atom.in_space(&space).as_u64()?.into(),
    );

    Ok(Atom::new(&mut context.stack, (a_belt - b_belt).into()).as_noun())
}

pub fn bneg_jet(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let a = slot(subject, 6, &space)?;
    let Ok(a_atom) = a.as_atom() else {
        debug!("a was not an atom");
        return Err(BAIL_FAIL);
    };
    let a_belt: Belt = a_atom.in_space(&space).as_u64()?.into();

    Ok(Atom::new(&mut context.stack, (-a_belt).into()).as_noun())
}

pub fn bmul_jet(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let a = slot(sam, 2, &space)?;
    let b = slot(sam, 3, &space)?;

    let (Ok(a_atom), Ok(b_atom)) = (a.as_atom(), b.as_atom()) else {
        debug!("a or b was not an atom");
        return Err(BAIL_FAIL);
    };
    let (a_belt, b_belt): (Belt, Belt) = (
        a_atom.in_space(&space).as_u64()?.into(),
        b_atom.in_space(&space).as_u64()?.into(),
    );

    Ok(Atom::new(&mut context.stack, (a_belt * b_belt).into()).as_noun())
}

pub fn ordered_root_jet(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let n = slot(subject, 6, &space)?;

    let Ok(n_atom) = n.as_atom() else {
        debug!("n was not an atom");
        return Err(BAIL_FAIL);
    };
    let n_u64 = Belt(n_atom.in_space(&space).as_u64()?);
    // TODO: clean this up
    let res_atom = Atom::new(&mut context.stack, n_u64.ordered_root()?.into());
    Ok(res_atom.as_noun())
}

pub fn bpow_jet(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let x = slot(sam, 2, &space)?;
    let n = slot(sam, 3, &space)?;

    let (Ok(x_atom), Ok(n_atom)) = (x.as_atom(), n.as_atom()) else {
        debug!("x or n was not an atom");
        return Err(BAIL_FAIL);
    };
    let (x_belt, n_belt) = (
        x_atom.in_space(&space).as_u64()?,
        n_atom.in_space(&space).as_u64()?,
    );

    Ok(Atom::new(&mut context.stack, bpow(x_belt, n_belt)).as_noun())
}

pub fn rip_correct_jet(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let stack = &mut context.stack;
    let sam = slot(subject, 6, &space)?;
    let a_noun = slot(sam, 2, &space)?;
    let b_noun = slot(sam, 3, &space)?;

    let b = b_noun.in_space(&space).as_atom()?.atom();
    let (bloq, step) = bite(a_noun, &space)?;
    rip_correct(stack, bloq, step, b, &space)
}

pub fn rip_correct(
    stack: &mut NockStack,
    bloq: usize,
    step: usize,
    b: Atom,
    space: &NounSpace,
) -> Result {
    if b.is_direct() && b.in_space(space).as_u64()? == 0 {
        return Ok(T(stack, &[D(0), D(0)]));
    }
    rip(stack, bloq, step, b, space)
}

pub fn levy_based(a_noun: Noun, space: &NounSpace) -> bool {
    let mut list = a_noun;
    loop {
        if unsafe { list.raw_equals(&D(0)) } {
            return true;
        }
        let cell = list.in_space(space).as_cell().expect("cell not found");
        let based_res = based(cell.head().noun(), space);
        if !based_res {
            return false;
        }

        list = cell.tail().noun();
    }
}

pub fn based_jet(_context: &mut Context, subject: Noun) -> Result {
    let space = _context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    if based(sam, &space) {
        Ok(YES)
    } else {
        Ok(NO)
    }
}

fn based(a_noun: Noun, space: &NounSpace) -> bool {
    let Ok(a_atom) = a_noun.in_space(space).as_atom() else {
        return false; // no atom
    };
    let Ok(a_u64) = a_atom.as_u64() else {
        return false; // no u64
    };

    a_u64 < PRIME
}

pub fn based_noun_jet(_context: &mut Context, subject: Noun) -> Result {
    let space = _context.stack.noun_space();
    let n = slot(subject, 6, &space)?;
    if based_noun(n, &space) {
        Ok(YES)
    } else {
        Ok(NO)
    }
}

pub fn based_noun(n: Noun, space: &NounSpace) -> bool {
    if n.is_atom() {
        return based(n, space);
    }

    let n_cell = n
        .in_space(space)
        .as_cell()
        .expect("n should be a cell since it's not an atom");
    let res1 = based_noun(n_cell.head().noun(), space);
    if !res1 {
        return false;
    }

    based_noun(n_cell.tail().noun(), space)
}
