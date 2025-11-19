use crate::form::belt::*;
use crate::form::bpoly::{bp_fft, bpoly_zero_extend};
use crate::form::felt::{fpow, Felt};
use crate::form::fpoly::*;
use crate::form::mary::{snag_as_bpoly, MarySlice};
use crate::form::poly::*;
use crate::form::structs::HoonList;

pub fn precompute_ntts(
    polys: MarySlice,
    height: usize,
    max_ntt_len: usize,
    res: &mut [Belt],
) -> Result<(), FieldError> {
    let new_len = height * max_ntt_len;

    for i in 0..polys.len as usize {
        let bp = snag_as_bpoly(polys, i);
        let mut extended = vec![Belt::zero(); new_len];
        bpoly_zero_extend(bp, &mut extended);
        let fft = bp_fft(&extended)?;
        res[i * new_len..(i + 1) * new_len].copy_from_slice(&fft);
    }
    Ok(())
}

pub fn compute_deep(
    trace_polys: HoonList,
    trace_openings: &[Felt],
    composition_pieces: HoonList,
    composition_piece_openings: &[Felt],
    weights: &[Felt],
    omicrons: &[Felt],
    deep_challenge: &Felt,
    comp_eval_point: &Felt,
) -> Vec<Felt> {
    let composition_pieces = composition_pieces
        .into_iter()
        .map(|x| {
            FPolySlice::try_from(x)
                .unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                })
                .0
        })
        .collect::<Vec<&[Felt]>>();

    let mut acc_trace = vec![Felt::zero()];
    let mut curr = 0;

    let mut fps: Vec<(Vec<Vec<Felt>>, &Felt)> = vec![];
    for (trace_poly, omicron) in trace_polys.into_iter().zip(omicrons.iter()) {
        let Ok(trace_poly) = MarySlice::try_from(trace_poly) else {
            panic!("trace_poly in trace_polys is not a valid FPolySlice");
        };

        let mut fp_list =
            vec![vec![Felt::zero(); trace_poly.step as usize]; trace_poly.len as usize];

        for (j, item) in fp_list.iter_mut().enumerate().take(trace_poly.len as usize) {
            bpoly_to_fpoly(snag_as_bpoly(trace_poly, j), item);
        }

        fps.push((fp_list, omicron));
    }

    for (fp_list, omicron) in &fps {
        let fp_list_slice = fp_list
            .iter()
            .map(|x| x.as_slice())
            .collect::<Vec<&[Felt]>>();
        let ptr_width = fp_list_slice.len();

        let deep_chal_poly = vec![*deep_challenge];
        let shifted_chal = vec![**omicron * *deep_challenge];

        let first_row = weighted_linear_combo(
            &fp_list_slice,
            &trace_openings[curr..curr + ptr_width],
            &deep_chal_poly,
            &weights[curr..curr + ptr_width],
        );

        curr += ptr_width;

        let second_row = weighted_linear_combo(
            &fp_list_slice,
            &trace_openings[curr..curr + ptr_width],
            &shifted_chal,
            &weights[curr..curr + ptr_width],
        );

        curr += ptr_width;

        acc_trace = fpadd_(
            second_row.as_slice(),
            &fpadd_(acc_trace.as_slice(), first_row.as_slice()),
        );
    }

    //  now do the same thing but for the second composition poly
    for (fp_list, omicron) in fps {
        let fp_list_slice = fp_list
            .iter()
            .map(|x| x.as_slice())
            .collect::<Vec<&[Felt]>>();
        let ptr_width = fp_list_slice.len();

        let comp_eval_poly = vec![*comp_eval_point];
        let shifted_chal = vec![*omicron * *comp_eval_point];

        let first_row = weighted_linear_combo(
            &fp_list_slice,
            &trace_openings[curr..curr + ptr_width],
            &comp_eval_poly,
            &weights[curr..curr + ptr_width],
        );

        curr += ptr_width;

        let second_row = weighted_linear_combo(
            &fp_list_slice,
            &trace_openings[curr..curr + ptr_width],
            &shifted_chal,
            &weights[curr..curr + ptr_width],
        );

        curr += ptr_width;

        acc_trace = fpadd_(
            second_row.as_slice(),
            &fpadd_(acc_trace.as_slice(), first_row.as_slice()),
        );
    }

    let mut piece_eval = Felt::zero();

    fpow(
        deep_challenge,
        composition_pieces.len() as u64,
        &mut piece_eval,
    );

    let pieces = weighted_linear_combo(
        &composition_pieces,
        composition_piece_openings,
        &[piece_eval],
        &weights[curr..],
    );

    fpadd_(&acc_trace, &pieces)
}

fn weighted_linear_combo(
    polys: &[&[Felt]],
    openings: &[Felt],
    x_poly: &[Felt],
    weights: &[Felt],
) -> Vec<Felt> {
    let mut acc = vec![Felt::zero()];

    let id_fpoly: Vec<Felt> = vec![Felt::zero(), Felt::one()];

    for ((poly, scale), opening) in polys.iter().zip(weights.iter()).zip(openings) {
        let opening = vec![*opening];

        // acc += alpha*(f(x) - f(Z)  / x - Z)
        let numerator = fpsub_(poly, opening.as_slice());
        let denom = fpsub_(&id_fpoly, x_poly);

        let quotient = fpdiv_(numerator.as_slice(), denom.as_slice());

        let weighted = fpscal_(scale, quotient.as_slice());

        acc = fpadd_(acc.as_slice(), weighted.as_slice());
    }
    acc
}
