use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::noun::{IndirectAtom, Noun, D};
use noun_serde::NounDecode;
use tracing::debug;

use crate::form::belt::Belt;
use crate::form::felt::*;
use crate::form::handle::{finalize_poly, new_handle_mut_felt, new_handle_mut_slice};
use crate::form::math::prover::{compute_deep, degree_processing};
use crate::form::noun_ext::NounMathExt;
use crate::form::poly::{BPolySlice, FPolySlice};
use crate::form::proof::{ConstraintsSlice, CountMap, IndexBPolyMap};
use crate::form::structs::HoonList;
use crate::form::verifier_math::eval_composition_poly_with_degrees;

pub fn eval_composition_poly_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let [trace_evaluations, heights, constraint_map, counts_map, dyn_list, weights_map, challenges, deep_challange, table_full_widths, is_extra] =
        sam.uncell(&space)?;

    let Ok(trace_evaluations) = FPolySlice::try_from(trace_evaluations, &space) else {
        debug!("trace_evaluations is not a valid FPolySlice");
        return Err(BAIL_FAIL);
    };
    let Ok(heights) = Vec::<u64>::from_noun(&heights, &space) else {
        debug!("heights decode failed");
        return Err(BAIL_FAIL);
    };
    let constraint_map = ConstraintsSlice::try_from(constraint_map, &space)?;
    let counts_map = CountMap::try_from(counts_map, &space)?;

    let dyn_list: Vec<BPolySlice<'_>> = HoonList::try_from(dyn_list, &space)?
        .into_iter()
        .map(|x| BPolySlice::try_from(x, &space))
        .collect::<Result<Vec<BPolySlice<'_>>, _>>()?;
    let weights_map = IndexBPolyMap::try_from(weights_map, &space)?;
    let challenges = BPolySlice::try_from(challenges, &space)?;
    let deep_challenge = deep_challange.as_felt(&space)?;
    let table_full_widths: Vec<u64> = HoonList::try_from(table_full_widths, &space)?
        .into_iter()
        .map(|x| Ok(x.in_space(&space).as_atom()?.as_u64()?))
        .collect::<Result<Vec<u64>, JetErr>>()?;
    let is_extra = unsafe { is_extra.raw_equals(&D(0)) };

    let processed_degrees = degree_processing(&heights, is_extra, &constraint_map);
    let weights = weights_by_table(&weights_map, heights.len())?;
    let res = eval_composition_poly_with_degrees(
        &trace_evaluations, &heights, &processed_degrees, &counts_map, &dyn_list, &weights,
        &challenges, deep_challenge, &table_full_widths, is_extra,
    )?;

    let (res_atom, res_felt): (IndirectAtom, &mut Felt) = new_handle_mut_felt(&mut context.stack);
    res_felt.copy_from_slice(&res);
    Ok(res_atom.as_noun())
}

fn weights_by_table<'a>(
    weights_map: &'a IndexBPolyMap<'a>,
    table_count: usize,
) -> Result<Vec<&'a [Belt]>, JetErr> {
    (0..table_count)
        .map(|i| weights_map.0.get(&i).copied().ok_or(BAIL_FAIL))
        .collect()
}

pub fn compute_deep_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let trace_polys = slot(sam, 2, &space)?;
    let trace_openings = slot(sam, 6, &space)?;
    let composition_pieces = slot(sam, 14, &space)?;
    let composition_piece_openings = slot(sam, 30, &space)?;
    let weights = slot(sam, 62, &space)?;
    let omicrons = slot(sam, 126, &space)?;
    let deep_challenge = slot(sam, 254, &space)?;
    let comp_eval_point = slot(sam, 255, &space)?;

    //  TODO: implement conversion from NounError to JetErr
    let (Ok(trace_openings), Ok(composition_piece_openings), Ok(weights), Ok(omicrons)) = (
        FPolySlice::try_from(trace_openings, &space),
        FPolySlice::try_from(composition_piece_openings, &space),
        FPolySlice::try_from(weights, &space),
        FPolySlice::try_from(omicrons, &space),
    ) else {
        debug!("one of trace_openings, composition_piece_openings, weights, or omicrons is not a valid FPolySlice");
        return Err(BAIL_FAIL);
    };

    let trace_polys = HoonList::try_from(trace_polys, &space)?;
    let composition_pieces = HoonList::try_from(composition_pieces, &space)?;
    let deep_challenge = deep_challenge.as_felt(&space)?;
    let comp_eval_point = comp_eval_point.as_felt(&space)?;

    let compute_deep_res = match compute_deep(
        trace_polys, trace_openings.0, composition_pieces, composition_piece_openings.0, weights.0,
        omicrons.0, deep_challenge, comp_eval_point, &space,
    ) {
        Ok(res) => res,
        Err(e) => {
            debug!("compute_deep_jet: falling back to Hoon: {e}");
            return Err(BAIL_FAIL);
        }
    };

    let (res, res_poly): (IndirectAtom, &mut [Felt]) =
        new_handle_mut_slice(&mut context.stack, Some(compute_deep_res.len()));

    res_poly.copy_from_slice(compute_deep_res.as_slice());

    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);
    Ok(res_cell)
}
