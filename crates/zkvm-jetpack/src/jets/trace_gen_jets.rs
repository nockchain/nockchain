use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::noun::{IndirectAtom, Noun, T};
use tracing::error;

use crate::form::felt::Felt;
use crate::form::gen_trace::{build_tree_data, TreeData};
use crate::form::handle::new_handle_mut_felt;
use crate::form::noun_ext::NounMathExt;

pub fn build_tree_data_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let sam = slot(subject, 6)?;
    let t = slot(sam, 2)?;
    let alf_noun = slot(sam, 3)?;
    let Ok(alf) = alf_noun.as_felt() else {
        error!("alf not a felt");
        return Err(BAIL_FAIL);
    };

    let tree_data: TreeData = build_tree_data(t, alf)?;

    let (leaf_atom, leaf_res): (IndirectAtom, &mut Felt) = new_handle_mut_felt(&mut context.stack);
    let (dyck_atom, dyck_res): (IndirectAtom, &mut Felt) = new_handle_mut_felt(&mut context.stack);
    let (size_atom, size_res): (IndirectAtom, &mut Felt) = new_handle_mut_felt(&mut context.stack);
    *leaf_res = tree_data.leaf;
    *dyck_res = tree_data.dyck;
    *size_res = tree_data.size;

    let res: Noun = T(
        &mut context.stack,
        &[size_atom.as_noun(), dyck_atom.as_noun(), leaf_atom.as_noun(), t],
    );
    Ok(res)
}
