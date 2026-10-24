use nockvm::jets::JetErr;
use nockvm::noun::{Noun, NounSpace};

use crate::form::belt::Belt;
use crate::form::felt::{fadd_, finv_, fmul_};
use crate::form::math::gen_trace::build_tree_data;
pub use crate::jets::table_utils::{NUM_EXT_CHALS, NUM_MEGA_EXT_CHALS};

pub fn augment_challenges(
    chals: &mut Vec<Belt>,
    subject: Noun,
    space: &NounSpace,
) -> Result<(), JetErr> {
    let ext_chals =
        crate::jets::table_utils::init_ext_chals_from_belts(&chals[0..NUM_EXT_CHALS as usize])?;
    let inv_alf = finv_(&ext_chals.alf);
    let subject_tree = build_tree_data(subject, &ext_chals.alf, space)?;
    let input_ifp = fadd_(
        &fmul_(&ext_chals.a, &subject_tree.size),
        &fadd_(
            &fmul_(&ext_chals.b, &subject_tree.dyck),
            &fmul_(&ext_chals.c, &subject_tree.leaf),
        ),
    );

    chals.push(inv_alf[0]);
    chals.push(inv_alf[1]);
    chals.push(inv_alf[2]);
    chals.push(input_ifp[0]);
    chals.push(input_ifp[1]);
    chals.push(input_ifp[2]);
    Ok(())
}
