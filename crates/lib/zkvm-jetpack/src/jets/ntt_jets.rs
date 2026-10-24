use nockvm::interpreter::Context;
use nockvm::jets::util::slot;
use nockvm::jets::JetErr;
use nockvm::noun::{IndirectAtom, Noun};

use crate::form::belt::Belt;
use crate::form::handle::{finalize_poly, new_handle_mut_slice};
use crate::form::mary::MarySlice;
use crate::form::math::ntt::precompute_ntts;

pub fn precompute_ntts_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let polys = slot(sam, 2, &space)?;
    let height = slot(sam, 6, &space)?.in_space(&space).as_atom()?.as_u64()? as usize;
    let max_ntt_len = slot(sam, 7, &space)?.in_space(&space).as_atom()?.as_u64()? as usize;

    let polys = MarySlice::try_from(polys, &space).unwrap_or_else(|err| {
        panic!(
            "Panicked with {err:?} at {}:{} (git sha: {:?})",
            file!(),
            line!(),
            option_env!("GIT_SHA")
        )
    });

    let (res, res_poly): (IndirectAtom, &mut [Belt]) = new_handle_mut_slice(
        &mut context.stack,
        Some(height * max_ntt_len * polys.len as usize),
    );
    precompute_ntts(polys, height, max_ntt_len, res_poly)?;

    Ok(finalize_poly(&mut context.stack, Some(res_poly.len()), res))
}
