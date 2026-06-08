use nockvm::jets::list::util::flop;
use nockvm::jets::JetErr;
use nockvm::noun::{Noun, NounAllocator, NounSpace, D, T};
use noun_serde::NounEncode;

pub fn dyck<A: NounAllocator>(stack: &mut A, t: Noun, space: &NounSpace) -> Result<Noun, JetErr> {
    let vec = dyck_recursive(stack, t, D(0), space)?;
    let stack_space = stack.noun_space();
    flop(stack, vec, &stack_space)
}

fn dyck_recursive<A: NounAllocator>(
    stack: &mut A,
    t: Noun,
    vec: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    if t.is_atom() {
        Ok(vec)
    } else {
        let t_cell = t.in_space(space).as_cell()?;
        let vec_inner = T(stack, &[D(0), vec]);
        let head = t_cell.head().noun();
        let dyck_inner = dyck_recursive(stack, head, vec_inner, space)?;
        let vec_outer = T(stack, &[D(1), dyck_inner]);
        let tail = t_cell.tail().noun();
        dyck_recursive(stack, tail, vec_outer, space)
    }
}

pub fn leaf_sequence<A: NounAllocator>(
    stack: &mut A,
    t: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    let mut leaf: Vec<u64> = Vec::<u64>::new();
    do_leaf_sequence(t, &mut leaf, space)?;
    let res = leaf.to_noun(stack);
    Ok(res)
}

pub fn do_leaf_sequence(noun: Noun, vec: &mut Vec<u64>, space: &NounSpace) -> Result<(), JetErr> {
    if noun.is_atom() {
        vec.push(noun.in_space(space).as_atom()?.as_u64()?);
        Ok(())
    } else {
        let cell = noun.in_space(space).as_cell()?;
        let head = cell.head().noun();
        let tail = cell.tail().noun();
        do_leaf_sequence(head, vec, space)?;
        do_leaf_sequence(tail, vec, space)?;
        Ok(())
    }
}
