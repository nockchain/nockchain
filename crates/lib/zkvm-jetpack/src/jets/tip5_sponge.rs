use bitvec::prelude::{BitSlice, Lsb0};
use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::mem::NockStack;
use nockvm::noun::{Cell, Noun, NounSpace, T};

use crate::form::tip5;
use crate::jets::tip5_jets::*;
use crate::utils::*;

// edit door values
//  Returns `Err(BAIL_FAIL)` rather than panicking on a zero edit axis or a tree
//  that is too shallow for the axis, so such inputs fall back to the Hoon arm.
fn door_edit(
    stack: &mut NockStack,
    edit_axis_path: u64,
    patch: Noun,
    mut tree: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    let edit_axis = BitSlice::<u64, Lsb0>::from_element(&edit_axis_path);

    let mut res = patch;
    let mut dest: *mut Noun = &mut res;
    let mut cursor = edit_axis.last_one().ok_or(BAIL_FAIL)?;
    loop {
        if cursor == 0 {
            unsafe {
                *dest = patch;
            }
            break;
        };
        if let Ok(tree_cell) = tree.in_space(space).as_cell() {
            cursor -= 1;
            if edit_axis[cursor] {
                unsafe {
                    let (cell, cellmem) = Cell::new_raw_mut(stack);
                    *dest = cell.as_noun();
                    (*cellmem).head = tree_cell.head().noun();
                    dest = &mut ((*cellmem).tail);
                }
                tree = tree_cell.tail().noun();
            } else {
                unsafe {
                    let (cell, cellmem) = Cell::new_raw_mut(stack);
                    *dest = cell.as_noun();
                    (*cellmem).tail = tree_cell.tail().noun();
                    dest = &mut ((*cellmem).head);
                }
                tree = tree_cell.head().noun();
            }
        } else {
            return Err(BAIL_FAIL);
        };
    }
    Ok(res)
}

fn require_default_sponge_num_rounds(sponge_door: Noun, space: &NounSpace) -> Result<(), JetErr> {
    let tip5_door = slot(sponge_door, 7, space).map_err(|_| JetErr::Punt)?;
    let num_rounds = slot(tip5_door, 6, space)
        .map_err(|_| JetErr::Punt)?
        .in_space(space)
        .as_atom()
        .map_err(|_| JetErr::Punt)?
        .as_u64()
        .map_err(|_| JetErr::Punt)?;
    if num_rounds != tip5::NUM_ROUNDS as u64 {
        return Err(JetErr::Punt);
    }
    Ok(())
}

pub fn sponge_absorb_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let stack = &mut context.stack;
    let input_noun = slot(subject, 6, &space)?;
    let sponge_door = slot(subject, 7, &space)?;
    require_default_sponge_num_rounds(sponge_door, &space)?;
    let sponge_noun = slot(sponge_door, 6, &space)?;

    let input_vec = hoon_list_to_vecbelt(input_noun, &space)?;
    let mut sponge = hoon_list_to_sponge(sponge_noun, &space)?;

    // require that input is made of base field elements; return a deterministic
    // jet error (falling back to Hoon) rather than panicking otherwise
    if !input_vec
        .iter()
        .all(|b| crate::form::belt::based_check(b.0))
    {
        return Err(BAIL_FAIL);
    }

    let input = input_vec.iter().map(|belt| belt.0).collect::<Vec<_>>();
    tip5::hash::absorb(&mut sponge, &input);

    // update sponge in door
    let new_sponge = vec_to_hoon_list(stack, &sponge);
    let edit = door_edit(stack, 6, new_sponge, sponge_door, &space)?;

    Ok(edit)
}

//   ++  permute
//     ~%  %permute  +  ~
//     |.  ^+  sponge
//     (permutation sponge)
//   ::
// pub fn sponge_permute_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
//     let door = slot(subject, 7)?;
//     let sponge_noun = slot(door, 6)?;
//     let mut sponge = hoon_list_to_sponge(sponge_noun)?;
//
//     permute(&mut sponge);
//
//     // update sponge in door
//     let new_sponge = vec_to_hoon_list(context, &sponge);
//     let edit = door_edit(&mut context.stack, 6, new_sponge, door);
//
//     Ok(edit)
// }

// squeeze out the full rate and bring out of montgomery space
pub fn sponge_squeeze_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let stack = &mut context.stack;
    let sponge_door = slot(subject, 3, &space)?;
    require_default_sponge_num_rounds(sponge_door, &space)?;
    let sponge_noun = slot(sponge_door, 6, &space)?;
    let mut sponge = hoon_list_to_sponge(sponge_noun, &space)?;

    let output = tip5::hash::squeeze(&mut sponge);

    // update sponge in door
    let new_sponge = vec_to_hoon_list(stack, &sponge);
    let edit = door_edit(stack, 6, new_sponge, sponge_door, &space)?;

    let output_noun = vec_to_hoon_list(stack, &output);
    let res = T(stack, &[output_noun, edit]);
    Ok(res)
}

#[cfg(test)]
mod tests {
    use nockvm::jets::util::test::init_context;
    use nockvm::noun::{D, T};

    use super::*;

    fn sponge_door(stack: &mut NockStack, num_rounds: u64) -> Noun {
        let sponge = vec_to_hoon_list(stack, &[0; tip5::STATE_SIZE]);
        let tip5_door = T(stack, &[D(0), D(num_rounds), D(0)]);
        T(stack, &[D(0), sponge, tip5_door])
    }

    #[test]
    fn sponge_absorb_punts_on_five_round_varlen_vector_path() {
        let c = &mut init_context();
        // The Hoon crypto KATs call `(~(hash-varlen tip5 5) ~)`. The top-level
        // hash jet punts on num-rounds=5, and this sponge arm must also punt so
        // the 5-round vector stays on the interpreted Hoon path.
        let input = D(0);
        let door = sponge_door(&mut c.stack, 5);
        let subject = T(&mut c.stack, &[D(0), input, door]);

        assert!(matches!(sponge_absorb_jet(c, subject), Err(JetErr::Punt)));
    }

    #[test]
    fn sponge_squeeze_punts_on_five_round_varlen_vector_path() {
        let c = &mut init_context();
        let door = sponge_door(&mut c.stack, 5);
        let subject = T(&mut c.stack, &[D(0), door]);

        assert!(matches!(sponge_squeeze_jet(c, subject), Err(JetErr::Punt)));
    }
}
