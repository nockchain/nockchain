/** Sorting jets
 */
use crate::interpreter::Context;
use crate::jets;
use crate::jets::util::slot;
use crate::noun::Noun;

crate::gdb!();

pub fn jet_dor(context: &mut Context, subject: Noun) -> jets::Result {
    let sam = slot(subject, 6)?;
    let a = slot(sam, 2)?;
    let b = slot(sam, 3)?;

    Ok(util::dor(&mut context.stack, a, b))
}

pub fn jet_gor(context: &mut Context, subject: Noun) -> jets::Result {
    let sam = slot(subject, 6)?;
    let a = slot(sam, 2)?;
    let b = slot(sam, 3)?;

    Ok(util::gor(&mut context.stack, a, b))
}

pub fn jet_mor(context: &mut Context, subject: Noun) -> jets::Result {
    let sam = slot(subject, 6)?;
    let a = slot(sam, 2)?;
    let b = slot(sam, 3)?;

    Ok(util::mor(&mut context.stack, a, b))
}

pub mod util {
    use std::cmp::Ordering;

    use either::{Left, Right};

    use crate::jets::math::util::lth;
    use crate::jets::util::slot;
    use crate::mem::NockStack;
    use crate::mug::mug;
    use crate::noun::{Noun, NO, YES};

    pub fn dor(stack: &mut NockStack, a: Noun, b: Noun) -> Noun {
        if unsafe { a.raw_equals(&b) } {
            YES
        } else {
            match (a.as_either_atom_cell(), b.as_either_atom_cell()) {
                (Left(atom_a), Left(atom_b)) => lth(stack, atom_a, atom_b),
                (Left(_), Right(_)) => YES,
                (Right(_), Left(_)) => NO,
                (Right(cell_a), Right(cell_b)) => {
                    let a_head = match slot(cell_a.as_noun(), 2) {
                        Ok(n) => n,
                        Err(_) => return NO,
                    };
                    let b_head = slot(cell_b.as_noun(), 2).unwrap_or_else(|err| {
                        panic!(
                            "Panicked with {err:?} at {}:{} (git sha: {:?})",
                            file!(),
                            line!(),
                            option_env!("GIT_SHA")
                        )
                    });
                    let a_tail = slot(cell_a.as_noun(), 3).unwrap_or_else(|err| {
                        panic!(
                            "Panicked with {err:?} at {}:{} (git sha: {:?})",
                            file!(),
                            line!(),
                            option_env!("GIT_SHA")
                        )
                    });
                    let b_tail = slot(cell_b.as_noun(), 3).unwrap_or_else(|err| {
                        panic!(
                            "Panicked with {err:?} at {}:{} (git sha: {:?})",
                            file!(),
                            line!(),
                            option_env!("GIT_SHA")
                        )
                    });
                    if unsafe { a_head.raw_equals(&b_head) } {
                        dor(stack, a_tail, b_tail)
                    } else {
                        dor(stack, a_head, b_head)
                    }
                }
            }
        }
    }

    pub fn gor(stack: &mut NockStack, a: Noun, b: Noun) -> Noun {
        let c = mug(stack, a);
        let d = mug(stack, b);

        match c.data().cmp(&d.data()) {
            Ordering::Greater => NO,
            Ordering::Less => YES,
            Ordering::Equal => dor(stack, a, b),
        }
    }

    pub fn mor(stack: &mut NockStack, a: Noun, b: Noun) -> Noun {
        let c = mug(stack, a);
        let d = mug(stack, b);

        let e = mug(stack, c.as_noun());
        let f = mug(stack, d.as_noun());

        match e.data().cmp(&f.data()) {
            Ordering::Greater => NO,
            Ordering::Less => YES,
            Ordering::Equal => dor(stack, a, b),
        }
    }
}

#[cfg(test)]
mod tests {
    use ibig::ubig;

    use super::*;
    use crate::jets::util::test::{assert_jet, init_context, A};
    use crate::noun::{D, NO, T, YES};

    #[test]
    fn test_dor() {
        let c = &mut init_context();

        let sam = T(&mut c.stack, &[D(1), D(1)]);
        assert_jet(c, jet_dor, sam, YES);

        let a = A(&mut c.stack, &ubig!(_0x3fffffffffffffff));
        let sam = T(&mut c.stack, &[a, D(1)]);
        assert_jet(c, jet_dor, sam, NO);

        let a = A(&mut c.stack, &ubig!(_0x3fffffffffffffff));
        let sam = T(&mut c.stack, &[a, a]);
        assert_jet(c, jet_dor, sam, YES);
    }

    #[test]
    fn test_gor() {
        let c = &mut init_context();

        let sam = T(&mut c.stack, &[D(1), D(1)]);
        assert_jet(c, jet_gor, sam, YES);

        let a = A(&mut c.stack, &ubig!(_0x3fffffffffffffff));
        let sam = T(&mut c.stack, &[a, a]);
        assert_jet(c, jet_gor, sam, YES);
    }

    #[test]
    fn test_mor() {
        let c = &mut init_context();

        let sam = T(&mut c.stack, &[D(1), D(1)]);
        assert_jet(c, jet_mor, sam, YES);

        let a = A(&mut c.stack, &ubig!(_0x3fffffffffffffff));
        let sam = T(&mut c.stack, &[a, a]);
        assert_jet(c, jet_mor, sam, YES);
    }
}
