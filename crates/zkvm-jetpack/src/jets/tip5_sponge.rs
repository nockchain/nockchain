use bitvec::prelude::{BitSlice, Lsb0};
use nockvm::interpreter::Context;
use nockvm::jets::util::slot;
use nockvm::jets::JetErr;
use nockvm::mem::NockStack;
use nockvm::noun::{Cell, Noun, T};

use crate::form::belt::mont_reduction;
use crate::form::tip5;
use crate::jets::tip5_jets::*;
use crate::utils::*;

// edit door values
fn door_edit(stack: &mut NockStack, edit_axis_path: u64, patch: Noun, mut tree: Noun) -> Noun {
    let edit_axis = BitSlice::<u64, Lsb0>::from_element(&edit_axis_path);

    let mut res = patch;
    let mut dest: *mut Noun = &mut res;
    let mut cursor = edit_axis
        .last_one()
        .expect("0 is not allowed as an edit axis");
    loop {
        if cursor == 0 {
            unsafe {
                *dest = patch;
            }
            break;
        };
        if let Ok(tree_cell) = tree.as_cell() {
            cursor -= 1;
            if edit_axis[cursor] {
                unsafe {
                    let (cell, cellmem) = Cell::new_raw_mut(stack);
                    *dest = cell.as_noun();
                    (*cellmem).head = tree_cell.head();
                    dest = &mut ((*cellmem).tail);
                }
                tree = tree_cell.tail();
            } else {
                unsafe {
                    let (cell, cellmem) = Cell::new_raw_mut(stack);
                    *dest = cell.as_noun();
                    (*cellmem).tail = tree_cell.tail();
                    dest = &mut ((*cellmem).head);
                }
                tree = tree_cell.head();
            }
        } else {
            panic!("Invalid axis for edit");
        };
    }
    res
}

pub fn sponge_absorb_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let stack = &mut context.stack;
    let input_noun = slot(subject, 6)?;
    let door = slot(subject, 7)?;
    let sponge_noun = slot(door, 6)?;

    let mut input_vec = hoon_list_to_vecbelt(input_noun)?;
    let mut sponge = hoon_list_to_sponge(sponge_noun)?;

    // assert that input is made of base field elements
    tip5::hash::assert_all_based(&input_vec);

    // pad input with ~[1 0 ... 0] to be a multiple of rate
    let (q, r) = tip5::hash::tip5_calc_q_r(&input_vec);
    tip5::hash::tip5_pad_vecbelt(&mut input_vec, r);

    // bring input into montgomery space
    tip5::hash::tip5_montify_vecbelt(&mut input_vec);

    // process input in batches of size RATE
    tip5::hash::tip5_absorb_input(&mut input_vec, &mut sponge, q);

    // update sponge in door
    let new_sponge = vec_to_hoon_list(stack, &sponge);
    let edit = door_edit(stack, 6, new_sponge, door);

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
    let stack = &mut context.stack;
    let door = slot(subject, 3)?;
    let sponge_noun = slot(door, 6)?;
    let mut sponge = hoon_list_to_sponge(sponge_noun)?;

    let mut output = [0u64; tip5::RATE];
    for i in 0..tip5::RATE {
        output[i] = mont_reduction(sponge[i] as u128);
    }

    tip5::permute(&mut sponge);

    // update sponge in door
    let new_sponge = vec_to_hoon_list(stack, &sponge);
    let edit = door_edit(stack, 6, new_sponge, door);

    let output_noun = vec_to_hoon_list(stack, &output);
    let res = T(stack, &[output_noun, edit]);
    Ok(res)
}
