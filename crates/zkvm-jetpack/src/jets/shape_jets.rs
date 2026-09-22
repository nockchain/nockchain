use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::noun::{Atom, Noun, D};

use crate::form::shape::{dyck, leaf_sequence};

pub fn leaf_sequence_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let t = slot(subject, 6, &space)?;
    leaf_sequence(&mut context.stack, t, &space)
}

pub fn dyck_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let stack = &mut context.stack;
    let t = slot(subject, 6, &space)?;
    dyck(stack, t, &space)
}

pub fn num_of_leaves_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let tuple = slot(subject, 6, &space)?;

    if tuple.is_atom() {
        return Ok(D(1));
    }

    let tuple = tuple.in_space(&space).as_cell()?;
    // The count is defined over the LOGICAL tree, so shared structure cannot
    // be skipped (a cue-decoded DAG of `k` physical nodes implies up to 2^k
    // logical leaves). Bound the walk instead: a noun whose logical leaf
    // count exceeds any legitimate object is rejected rather than expanded
    // into unbounded work.
    const MAX_LEAF_COUNT: u64 = 1 << 20;
    let mut num_leaves = 0u64;
    let mut next = vec![tuple];

    while let Some(curr) = next.pop() {
        let (head, tail) = (curr.head(), curr.tail());
        if head.is_atom() {
            num_leaves = num_leaves.checked_add(1).ok_or(BAIL_FAIL)?;
        } else {
            next.push(head.as_cell()?);
        }

        if tail.is_atom() {
            num_leaves = num_leaves.checked_add(1).ok_or(BAIL_FAIL)?;
        } else {
            next.push(tail.as_cell()?);
        }

        if num_leaves > MAX_LEAF_COUNT || next.len() > MAX_LEAF_COUNT as usize {
            return Err(BAIL_FAIL);
        }
    }

    Ok(Atom::new(&mut context.stack, num_leaves).as_noun())
}

#[cfg(test)]
mod tests {
    use nockvm::jets::util::test::*;
    use nockvm::noun::{D, T};

    use super::*;

    #[test]
    fn test_mont_reduction_jet() {
        let c = &mut init_context();

        // > (leaf-sequence:shape.zeke 1)
        // ~[1]
        let sam = D(1);
        let res = T(&mut c.stack, &[D(1), D(0)]);
        assert_jet(c, leaf_sequence_jet, sam, res);

        // > (leaf-sequence:shape.zeke ~)
        // ~[0]
        let sam = D(0);
        let res = T(&mut c.stack, &[D(0), D(0)]);
        assert_jet(c, leaf_sequence_jet, sam, res);

        // > (leaf-sequence:shape.zeke ~[1 2 3])
        // ~[1 2 3 0]
        let sam = T(&mut c.stack, &[D(1), D(2), D(3), D(0)]);
        let res = T(&mut c.stack, &[D(1), D(2), D(3), D(0), D(0)]);
        assert_jet(c, leaf_sequence_jet, sam, res);

        // > (leaf-sequence:shape.zeke [[1 2] 3])
        // ~[1 2 3]
        let t12 = T(&mut c.stack, &[D(1), D(2)]);
        let sam = T(&mut c.stack, &[t12, D(3), D(0)]);
        let res = T(&mut c.stack, &[D(1), D(2), D(3), D(0), D(0)]);
        assert_jet(c, leaf_sequence_jet, sam, res);

        // > (leaf-sequence:shape.zeke [[1 2] 3 [4 5] 6])
        // ~[1 2 3 4 5 6]
        let t12 = T(&mut c.stack, &[D(1), D(2)]);
        let t45 = T(&mut c.stack, &[D(4), D(5)]);
        let sam = T(&mut c.stack, &[t12, D(3), t45, D(6)]);
        let res = T(&mut c.stack, &[D(1), D(2), D(3), D(4), D(5), D(6), D(0)]);
        assert_jet(c, leaf_sequence_jet, sam, res);
    }

    #[test]
    fn num_of_leaves_counts_logical_leaves_on_shared_nouns() {
        let c = &mut init_context();
        // A shared subnoun `[1 2]` referenced twice: the list
        // [shared shared ~] has FIVE logical atom leaves (1, 2, 1, 2, and
        // the null terminator); the count must stay defined over the
        // logical tree, not the physical DAG.
        let shared = T(&mut c.stack, &[D(1), D(2)]);
        let sam = T(&mut c.stack, &[shared, shared, D(0)]);
        let res = D(5);
        assert_jet(c, num_of_leaves_jet, sam, res);
    }

    #[test]
    fn num_of_leaves_rejects_dag_bomb_expansion() {
        // GHSA-6vmg-7vh9-fc55: a shared DAG `a_k = [a_(k-1) a_(k-1)]` of a
        // few dozen physical nodes implies 2^k logical leaves. The count is
        // defined over the logical tree, so the walk is budget-bounded and
        // errors instead of expanding unbounded work.
        let c = &mut init_context();
        let mut noun = D(0);
        for _ in 0..40 {
            noun = T(&mut c.stack, &[noun, noun]);
        }
        let inner = T(&mut c.stack, &[noun, D(0)]);
        let subject = T(&mut c.stack, &[D(0), inner]);
        assert!(num_of_leaves_jet(c, subject).is_err());
    }
}
