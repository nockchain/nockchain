use std::cmp::Ordering;

use crate::interpreter::Context;
use crate::jets::sort::util::{gor, mor};
use crate::jets::util::slot;
use crate::jets::{JetErr, Result};
use crate::mem::NockStack;
//use crate::mug::mug;
use crate::noun::{Noun, NounSpace, Slots, D, NO, T, YES};

type JetResult<T> = std::result::Result<T, JetErr>;

#[inline(always)]
fn is_yes(noun: Noun) -> bool {
    unsafe { noun.raw_equals(&YES) }
}

fn decompose(node: Noun, space: &NounSpace) -> JetResult<(Noun, Noun, Noun)> {
    let cell = node.in_space(space).as_cell()?;
    let tail = cell.tail().as_cell()?;
    Ok((cell.head().noun(), tail.head().noun(), tail.tail().noun()))
}

fn make_node(stack: &mut NockStack, value: Noun, left: Noun, right: Noun) -> Noun {
    let tail = T(stack, &[left, right]);
    T(stack, &[value, tail])
}

// TODO: fix this jet. identical elements are not being deduplicated
pub fn jet_put(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let elem = slot(subject, 6, &space)?;
    let parent = match slot(subject, 7, &space) {
        Ok(parent) => parent,
        Err(_) => return Err(JetErr::Punt),
    };
    let set = match slot(parent, 6, &space) {
        Ok(set) => set,
        Err(_) => return Err(JetErr::Punt),
    };

    put_iter(&mut context.stack, set, elem, &space)
}

fn put_iter(stack: &mut NockStack, root: Noun, elem: Noun, space: &NounSpace) -> JetResult<Noun> {
    if unsafe { root.raw_equals(&D(0)) } {
        return Ok(make_node(stack, elem, D(0), D(0)));
    }

    let mut path: Vec<(Noun, bool)> = Vec::new();
    let mut current = root;

    loop {
        if unsafe { current.raw_equals(&D(0)) } {
            break;
        }

        let (value, left, right) = decompose(current, space)?;

        if unsafe { elem.raw_equals(&value) } {
            return Ok(root);
        }

        let go_left = is_yes(gor(stack, elem, value, space));
        path.push((current, go_left));
        current = if go_left { left } else { right };
    }

    let mut new_subtree = make_node(stack, elem, D(0), D(0));

    while let Some((node, went_left)) = path.pop() {
        let (value, left, right) = decompose(node, space)?;
        let (c_val, c_left, c_right) = decompose(new_subtree, space)?;

        new_subtree = if went_left {
            if is_yes(mor(stack, value, c_val, space)) {
                make_node(stack, value, new_subtree, right)
            } else {
                let new_a = make_node(stack, value, c_right, right);
                make_node(stack, c_val, c_left, new_a)
            }
        } else if is_yes(mor(stack, value, c_val, space)) {
            make_node(stack, value, left, new_subtree)
        } else {
            let new_a = make_node(stack, value, left, c_left);
            make_node(stack, c_val, new_a, c_right)
        };
    }

    Ok(new_subtree)
}

#[inline(always)]
fn ord_cmp(stack: &mut NockStack, a: Noun, b: Noun, space: &NounSpace) -> Ordering {
    unsafe {
        if a.raw_equals(&b) {
            return Ordering::Equal;
        }
    }
    if is_yes(gor(stack, b, a, space)) {
        return Ordering::Less;
    } else {
        return Ordering::Greater;
    }
}

fn has_loop(
    stack: &mut NockStack,
    mut tree: Noun,
    elem: Noun,
    space: &NounSpace,
) -> JetResult<bool> {
    while unsafe { !tree.raw_equals(&D(0)) } {
        let (val, left, right) = decompose(tree, space)?;
        match ord_cmp(stack, elem, val, space) {
            Ordering::Equal => return Ok(true),
            Ordering::Less => tree = left,
            Ordering::Greater => tree = right,
        }
    }
    Ok(false)
}

// TODO: check this jet.
pub fn jet_has(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let elem = subject.slot(6, &space)?;
    let parent = match subject.slot(7, &space) {
        Ok(parent) => parent,
        Err(_) => return Err(JetErr::Punt),
    };
    let set = match parent.slot(6, &space) {
        Ok(set) => set,
        Err(_) => return Err(JetErr::Punt),
    };
    let present = has_loop(&mut context.stack, set, elem, &space)?;
    Ok(if present { YES } else { NO })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jets::util::test::{assert_jet_door, assert_noun_eq, init_context};
    use crate::noun::{Noun, T};

    fn context_with_set(stack: &mut NockStack, set: Noun) -> Noun {
        T(stack, &[D(0), set, D(0)])
    }

    fn node(stack: &mut NockStack, value: Noun, left: Noun, right: Noun) -> Noun {
        let tail = T(stack, &[left, right]);
        T(stack, &[value, tail])
    }
    //  TODO: fix this test
    //    #[test]
    //    fn has_detects_membership_in_manual_tree() {
    //        let context = &mut init_context();
    //        let left = node(&mut context.stack, D(11), D(0), D(0));
    //        let right = node(&mut context.stack, D(3), D(0), D(0));
    //        let tree = node(&mut context.stack, D(7), left, right);
    //        let pay = context_with_set(&mut context.stack, tree);
    //
    //        assert_jet_door(context, jet_has, D(7), pay, YES);
    //        assert_jet_door(context, jet_has, D(3), pay, YES);
    //        assert_jet_door(context, jet_has, D(11), pay, YES);
    //        assert_jet_door(context, jet_has, D(2), pay, NO);
    //    }

    fn contains(stack: &mut NockStack, mut tree: Noun, elem: Noun) -> bool {
        let space = stack.noun_space();
        loop {
            if unsafe { tree.raw_equals(&D(0)) } {
                return false;
            }
            let (value, left, right) =
                decompose(tree, &space).expect("tree should be valid set node");
            if unsafe { value.raw_equals(&elem) } {
                return true;
            }
            tree = if is_yes(gor(stack, elem, value, &space)) {
                left
            } else {
                right
            };
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn insert_into_empty_set() {
        let context = &mut init_context();
        let elem = D(1);
        let expected = node(&mut context.stack, elem, D(0), D(0));
        let pay = context_with_set(&mut context.stack, D(0));

        assert_jet_door(context, jet_put, elem, pay, expected);
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn insert_duplicate_retains_tree() {
        let context = &mut init_context();
        let set = node(&mut context.stack, D(5), D(0), D(0));
        let pay = context_with_set(&mut context.stack, set);

        assert_jet_door(context, jet_put, D(5), pay, set);
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn insert_distinct_elements() {
        let context = &mut init_context();
        let base = node(&mut context.stack, D(9), D(0), D(0));
        let pay_left = context_with_set(&mut context.stack, base);
        let subject_left = T(&mut context.stack, &[D(0), D(4), pay_left]);
        let res_left = jet_put(context, subject_left).expect("jet_put left insert");
        assert!(contains(&mut context.stack, res_left, D(4)));
        assert!(contains(&mut context.stack, res_left, D(9)));

        let pay_right = context_with_set(&mut context.stack, res_left);
        let subject_right = T(&mut context.stack, &[D(0), D(12), pay_right]);
        let res_right = jet_put(context, subject_right).expect("jet_put right insert");
        assert!(contains(&mut context.stack, res_right, D(4)));
        assert!(contains(&mut context.stack, res_right, D(9)));
        assert!(contains(&mut context.stack, res_right, D(12)));
    }

    fn put_recursive(stack: &mut NockStack, tree: Noun, elem: Noun) -> JetResult<Noun> {
        let space = stack.noun_space();
        if unsafe { tree.raw_equals(&D(0)) } {
            return Ok(make_node(stack, elem, D(0), D(0)));
        }

        let (value, left, right) = decompose(tree, &space)?;

        if unsafe { elem.raw_equals(&value) } {
            return Ok(tree);
        }

        if is_yes(gor(stack, elem, value, &space)) {
            let c = put_recursive(stack, left, elem)?;
            let (c_val, c_left, c_right) = decompose(c, &space)?;

            if is_yes(mor(stack, value, c_val, &space)) {
                Ok(make_node(stack, value, c, right))
            } else {
                let new_a = make_node(stack, value, c_right, right);
                Ok(make_node(stack, c_val, c_left, new_a))
            }
        } else {
            let c = put_recursive(stack, right, elem)?;
            let (c_val, c_left, c_right) = decompose(c, &space)?;

            if is_yes(mor(stack, value, c_val, &space)) {
                Ok(make_node(stack, value, left, c))
            } else {
                let new_a = make_node(stack, value, left, c_left);
                Ok(make_node(stack, c_val, new_a, c_right))
            }
        }
    }

    fn permute(values: &mut [u64], start: usize, out: &mut Vec<Vec<u64>>) {
        if start == values.len() {
            out.push(values.to_vec());
            return;
        }

        for i in start..values.len() {
            values.swap(start, i);
            permute(values, start + 1, out);
            values.swap(start, i);
        }
    }

    fn tree_height(tree: Noun, space: &NounSpace) -> usize {
        let mut max = 0usize;
        let mut stack_vec = vec![(tree, 0usize)];

        while let Some((node, depth)) = stack_vec.pop() {
            if unsafe { node.raw_equals(&D(0)) } {
                continue;
            }

            if depth > max {
                max = depth;
            }

            let (value, left, right) = decompose(node, space).unwrap_or((node, D(0), D(0)));
            let _ = value;
            stack_vec.push((left, depth + 1));
            stack_vec.push((right, depth + 1));
        }

        max
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn put_matches_recursive_small_inputs() {
        let mut base = [1u64, 2, 3, 4];
        let mut perms = Vec::new();
        permute(&mut base, 0, &mut perms);

        for perm in perms.into_iter().take(120) {
            let context = &mut init_context();
            let mut jet_tree = D(0);
            let mut rec_tree = D(0);
            let space = context.stack.noun_space();

            for val in perm {
                let noun_val = D(val);
                jet_tree = put_iter(&mut context.stack, jet_tree, noun_val, &space)
                    .expect("put_iter should succeed");
                rec_tree = put_recursive(&mut context.stack, rec_tree, noun_val)
                    .expect("put_recursive should succeed");
            }

            assert_eq!(tree_height(jet_tree, &space), tree_height(rec_tree, &space));
            assert_noun_eq(&mut context.stack, jet_tree, rec_tree);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn put_matches_recursive_random_inputs() {
        let mut seed = 0xDEADBEEFu64;
        for _ in 0..20 {
            let context = &mut init_context();
            let mut jet_tree = D(0);
            let mut rec_tree = D(0);
            let space = context.stack.noun_space();

            for _ in 0..80 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let value = (seed >> 32) & 0xFFFF;
                let noun_val = D(value);
                jet_tree = put_iter(&mut context.stack, jet_tree, noun_val, &space)
                    .expect("put_iter should succeed");
                rec_tree = put_recursive(&mut context.stack, rec_tree, noun_val)
                    .expect("put_recursive should succeed");
            }

            assert_eq!(tree_height(jet_tree, &space), tree_height(rec_tree, &space));
            assert_noun_eq(&mut context.stack, jet_tree, rec_tree);
        }
    }
}
