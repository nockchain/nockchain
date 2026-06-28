/** Sorting jets
 */
use crate::interpreter::Context;
use crate::jets;
use crate::jets::util::slot;
use crate::noun::Noun;

crate::gdb!();

pub fn jet_dor(context: &mut Context, subject: Noun) -> jets::Result {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let a = slot(sam, 2, &space)?;
    let b = slot(sam, 3, &space)?;

    Ok(util::dor(&mut context.stack, a, b, &space))
}

pub fn jet_gor(context: &mut Context, subject: Noun) -> jets::Result {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let a = slot(sam, 2, &space)?;
    let b = slot(sam, 3, &space)?;

    Ok(util::gor(&mut context.stack, a, b, &space))
}

pub fn jet_mor(context: &mut Context, subject: Noun) -> jets::Result {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let a = slot(sam, 2, &space)?;
    let b = slot(sam, 3, &space)?;

    Ok(util::mor(&mut context.stack, a, b, &space))
}

pub mod util {
    use std::cmp::Ordering;

    use either::{Left, Right};
    use smallvec::SmallVec;

    use crate::ext::noun_equality;
    use crate::jets::math::util::lth;
    use crate::jets::util::slot;
    use crate::mem::NockStack;
    use crate::mug::mug;
    use crate::noun::{Noun, NounSpace, NO, YES};

    pub fn dor(stack: &mut NockStack, a: Noun, b: Noun, space: &NounSpace) -> Noun {
        if unsafe { a.raw_equals(&b) } {
            YES
        } else {
            match (a.as_either_atom_cell(), b.as_either_atom_cell()) {
                (Left(atom_a), Left(atom_b)) => lth(stack, atom_a, atom_b, space),
                (Left(_), Right(_)) => YES,
                (Right(_), Left(_)) => NO,
                (Right(cell_a), Right(cell_b)) => {
                    let a_head = match slot(cell_a.as_noun(), 2, space) {
                        Ok(n) => n,
                        Err(_) => return NO,
                    };
                    let b_head = slot(cell_b.as_noun(), 2, space).unwrap_or_else(|err| {
                        panic!(
                            "Panicked with {err:?} at {}:{} (git sha: {:?})",
                            file!(),
                            line!(),
                            option_env!("GIT_SHA")
                        )
                    });
                    let a_tail = slot(cell_a.as_noun(), 3, space).unwrap_or_else(|err| {
                        panic!(
                            "Panicked with {err:?} at {}:{} (git sha: {:?})",
                            file!(),
                            line!(),
                            option_env!("GIT_SHA")
                        )
                    });
                    let b_tail = slot(cell_b.as_noun(), 3, space).unwrap_or_else(|err| {
                        panic!(
                            "Panicked with {err:?} at {}:{} (git sha: {:?})",
                            file!(),
                            line!(),
                            option_env!("GIT_SHA")
                        )
                    });
                    if heads_equal(stack, a_head, b_head, space) {
                        dor(stack, a_tail, b_tail, space)
                    } else {
                        dor(stack, a_head, b_head, space)
                    }
                }
            }
        }
    }

    /// Structural equality for dor's head comparison (Hoon's `=(-.a -.b)`):
    /// pointer fast path, cached-mug pre-filter (`mug` caches recursively on
    /// both trees, so unequal heads reject in O(1) amortized), then a
    /// heap-free worklist walk with per-node mug rejection. Non-unifying:
    /// dor runs over nouns the caller may not own (PMA-resident state,
    /// slab-built cores), and must agree with soft `+dor` and honk's native
    /// dor for map/set treap parity.
    fn heads_equal(stack: &mut NockStack, a: Noun, b: Noun, space: &NounSpace) -> bool {
        if unsafe { a.raw_equals(&b) } {
            return true;
        }
        if mug(stack, a).data() != mug(stack, b).data() {
            return false;
        }

        // Mug-equal, unshared: walk structurally. Heavily shared subtrees
        // resolve through the raw-equality fast path; a bounded budget
        // spills pathological (mug-colliding, sharing-heavy) comparisons to
        // ext::noun_equality, which carries a seen-pair table.
        const WALK_BUDGET: usize = 4096;
        let mut budget = WALK_BUDGET;
        let mut work: SmallVec<[(Noun, Noun); 32]> = SmallVec::new();
        work.push((a, b));
        while let Some((x, y)) = work.pop() {
            if unsafe { x.raw_equals(&y) } {
                continue;
            }
            if budget == 0 {
                return noun_equality(x.in_space(space), y.in_space(space))
                    && work.into_iter().all(|(x, y)| {
                        (unsafe { x.raw_equals(&y) })
                            || noun_equality(x.in_space(space), y.in_space(space))
                    });
            }
            budget -= 1;
            if mug(stack, x).data() != mug(stack, y).data() {
                return false;
            }
            match (x.as_either_atom_cell(), y.as_either_atom_cell()) {
                (Left(xa), Left(ya)) => {
                    let xa = xa.in_space(space);
                    let ya = ya.in_space(space);
                    if xa.as_ne_bytes() != ya.as_ne_bytes() {
                        return false;
                    }
                }
                (Right(xc), Right(yc)) => {
                    let xc = xc.in_space(space);
                    let yc = yc.in_space(space);
                    work.push((xc.tail().noun(), yc.tail().noun()));
                    work.push((xc.head().noun(), yc.head().noun()));
                }
                _ => return false,
            }
        }
        true
    }

    pub fn gor(stack: &mut NockStack, a: Noun, b: Noun, space: &NounSpace) -> Noun {
        let c = mug(stack, a);
        let d = mug(stack, b);

        match c.data().cmp(&d.data()) {
            Ordering::Greater => NO,
            Ordering::Less => YES,
            Ordering::Equal => dor(stack, a, b, space),
        }
    }

    pub fn mor(stack: &mut NockStack, a: Noun, b: Noun, space: &NounSpace) -> Noun {
        let c = mug(stack, a);
        let d = mug(stack, b);

        let e = mug(stack, c.as_noun());
        let f = mug(stack, d.as_noun());

        match e.data().cmp(&f.data()) {
            Ordering::Greater => NO,
            Ordering::Less => YES,
            Ordering::Equal => dor(stack, a, b, space),
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
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
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

        let head_a = T(&mut c.stack, &[D(1), D(2)]);
        let head_b = T(&mut c.stack, &[D(1), D(2)]);
        let a = T(&mut c.stack, &[head_a, D(4)]);
        let b = T(&mut c.stack, &[head_b, D(3)]);
        let sam = T(&mut c.stack, &[a, b]);
        assert_jet(c, jet_dor, sam, NO);

        // Equal-but-unshared deep heads: structural equality must hold
        // through the worklist walk, and the tails decide the order.
        let inner_a = T(&mut c.stack, &[D(7), D(8)]);
        let inner_b = T(&mut c.stack, &[D(7), D(8)]);
        let head_a = T(&mut c.stack, &[inner_a, D(2)]);
        let head_b = T(&mut c.stack, &[inner_b, D(2)]);
        let a = T(&mut c.stack, &[head_a, D(3)]);
        let b = T(&mut c.stack, &[head_b, D(4)]);
        let sam = T(&mut c.stack, &[a, b]);
        assert_jet(c, jet_dor, sam, YES);

        // Equal-but-unshared indirect-atom heads: byte comparison after the
        // mug pre-filter, tails decide.
        let big_a = A(&mut c.stack, &ubig!(_0x3fffffffffffffff));
        let big_b = A(&mut c.stack, &ubig!(_0x3fffffffffffffff));
        let a = T(&mut c.stack, &[big_a, D(4)]);
        let b = T(&mut c.stack, &[big_b, D(3)]);
        let sam = T(&mut c.stack, &[a, b]);
        assert_jet(c, jet_dor, sam, NO);

        // Same-shape unequal heads: the mug pre-filter rejects, dor recurses
        // into the heads.
        let head_a = T(&mut c.stack, &[D(1), D(2)]);
        let head_b = T(&mut c.stack, &[D(1), D(3)]);
        let a = T(&mut c.stack, &[head_a, D(9)]);
        let b = T(&mut c.stack, &[head_b, D(0)]);
        let sam = T(&mut c.stack, &[a, b]);
        assert_jet(c, jet_dor, sam, YES);
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_gor() {
        let c = &mut init_context();

        let sam = T(&mut c.stack, &[D(1), D(1)]);
        assert_jet(c, jet_gor, sam, YES);

        let a = A(&mut c.stack, &ubig!(_0x3fffffffffffffff));
        let sam = T(&mut c.stack, &[a, a]);
        assert_jet(c, jet_gor, sam, YES);
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_mor() {
        let c = &mut init_context();

        let sam = T(&mut c.stack, &[D(1), D(1)]);
        assert_jet(c, jet_mor, sam, YES);

        let a = A(&mut c.stack, &ubig!(_0x3fffffffffffffff));
        let sam = T(&mut c.stack, &[a, a]);
        assert_jet(c, jet_mor, sam, YES);
    }
}
