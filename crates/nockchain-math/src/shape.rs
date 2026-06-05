use nockvm::jets::list::util::flop;
use nockvm::jets::JetErr;
use nockvm::noun::{Noun, NounAllocator, D, T};
use noun_serde::NounEncode;

pub fn dyck<A: NounAllocator>(stack: &mut A, t: Noun) -> Result<Noun, JetErr> {
    let vec = dyck_recursive(stack, t, D(0))?;
    flop(stack, vec)
}

fn dyck_recursive<A: NounAllocator>(stack: &mut A, t: Noun, vec: Noun) -> Result<Noun, JetErr> {
    if t.is_atom() {
        Ok(vec)
    } else {
        let t_cell = t.as_cell()?;
        let vec_inner = T(stack, &[D(0), vec]);
        let dyck_inner = dyck_recursive(stack, t_cell.head(), vec_inner)?;
        let vec_outer = T(stack, &[D(1), dyck_inner]);
        dyck_recursive(stack, t_cell.tail(), vec_outer)
    }
}

pub fn leaf_sequence<A: NounAllocator>(stack: &mut A, t: Noun) -> Result<Noun, JetErr> {
    let mut leaf: Vec<u64> = Vec::<u64>::new();
    do_leaf_sequence(t, &mut leaf)?;
    let res = leaf.to_noun(stack);
    Ok(res)
}

pub fn do_leaf_sequence(noun: Noun, vec: &mut Vec<u64>) -> Result<(), JetErr> {
    if noun.is_atom() {
        vec.push(noun.as_atom()?.as_u64()?);
        Ok(())
    } else {
        let cell = noun.as_cell()?;
        do_leaf_sequence(cell.head(), vec)?;
        do_leaf_sequence(cell.tail(), vec)?;
        Ok(())
    }
}
