use std::cmp;

use nockvm::noun::NounSpace;

use crate::form::felt::{fpow, Felt};
use crate::form::fpoly::*;
use crate::form::mary::{snag_as_bpoly, MarySlice};
use crate::form::poly::*;
use crate::form::proof::{ConstraintDataSlice, ConstraintsSlice, MPUltraSlice, ProofMap};
use crate::form::structs::HoonList;

#[allow(clippy::too_many_arguments)]
pub fn compute_deep(
    trace_polys: HoonList,
    trace_openings: &[Felt],
    composition_pieces: HoonList,
    composition_piece_openings: &[Felt],
    weights: &[Felt],
    omicrons: &[Felt],
    deep_challenge: &Felt,
    comp_eval_point: &Felt,
    space: &NounSpace,
) -> Result<Vec<Felt>, ComputeDeepError> {
    let mut composition_pieces_vec: Vec<&[Felt]> = Vec::new();
    for (idx, x) in composition_pieces.into_iter().enumerate() {
        let slice = FPolySlice::try_from(x, space)
            .map_err(|_| ComputeDeepError::InvalidCompositionPiece { index: idx })?;
        composition_pieces_vec.push(slice.0);
    }

    let mut acc_trace = vec![Felt::zero()];
    let mut curr = 0;

    let mut fps: Vec<(Vec<Vec<Felt>>, &Felt)> = vec![];
    for (idx, (trace_poly, omicron)) in trace_polys.into_iter().zip(omicrons.iter()).enumerate() {
        let trace_poly = MarySlice::try_from(trace_poly, space)
            .map_err(|_| ComputeDeepError::InvalidTracePoly { index: idx })?;

        let mut fp_list =
            vec![vec![Felt::zero(); trace_poly.step as usize]; trace_poly.len as usize];

        for (j, item) in fp_list.iter_mut().enumerate().take(trace_poly.len as usize) {
            bpoly_to_fpoly(snag_as_bpoly(trace_poly, j), item);
        }

        fps.push((fp_list, omicron));
    }

    let ptr_widths: Vec<usize> = fps.iter().map(|(fp, _)| fp.len()).collect();
    let total_ptr_widths: usize = ptr_widths.iter().sum();
    let needed_openings = 4 * total_ptr_widths;
    let needed_weights = needed_openings + composition_pieces_vec.len();

    if trace_openings.len() < needed_openings {
        return Err(ComputeDeepError::TraceOpeningsTooShort {
            have: trace_openings.len(),
            need: needed_openings,
            trace_poly_count: fps.len(),
            ptr_widths,
        });
    }
    if weights.len() < needed_weights {
        return Err(ComputeDeepError::WeightsTooShort {
            have: weights.len(),
            need: needed_weights,
        });
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
        composition_pieces_vec.len() as u64,
        &mut piece_eval,
    );

    let pieces = weighted_linear_combo(
        &composition_pieces_vec,
        composition_piece_openings,
        &[piece_eval],
        &weights[curr..],
    );

    Ok(fpadd_(&acc_trace, &pieces))
}

#[derive(Debug)]
pub enum ComputeDeepError {
    TraceOpeningsTooShort {
        have: usize,
        need: usize,
        trace_poly_count: usize,
        ptr_widths: Vec<usize>,
    },
    WeightsTooShort {
        have: usize,
        need: usize,
    },
    InvalidTracePoly {
        index: usize,
    },
    InvalidCompositionPiece {
        index: usize,
    },
}

impl std::fmt::Display for ComputeDeepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TraceOpeningsTooShort {
                have,
                need,
                trace_poly_count,
                ptr_widths,
            } => write!(
                f,
                "compute_deep: trace_openings too short: have {have}, need {need} \
                 (trace_poly_count={trace_poly_count}, ptr_widths={ptr_widths:?})"
            ),
            Self::WeightsTooShort { have, need } => {
                write!(
                    f,
                    "compute_deep: weights too short: have {have}, need {need}"
                )
            }
            Self::InvalidTracePoly { index } => {
                write!(
                    f,
                    "compute_deep: trace_poly at index {index} is not a valid MarySlice"
                )
            }
            Self::InvalidCompositionPiece { index } => write!(
                f,
                "compute_deep: composition_piece at index {index} is not a valid FPolySlice"
            ),
        }
    }
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

pub struct PolyWithDegreeFudges<'a> {
    pub degrees: Vec<u64>,
    pub poly: &'a MPUltraSlice<'a>,
}

pub struct ConstraintsWDegree<'a> {
    pub boundary: Vec<PolyWithDegreeFudges<'a>>,
    pub row: Vec<PolyWithDegreeFudges<'a>>,
    pub transition: Vec<PolyWithDegreeFudges<'a>>,
    pub terminal: Vec<PolyWithDegreeFudges<'a>>,
    pub extra: Vec<PolyWithDegreeFudges<'a>>,
}

pub struct ProcessedDegrees<'a> {
    pub fri_degree_bound: u64,
    pub constraints: ProofMap<usize, ConstraintsWDegree<'a>>,
}

struct DegreeData<'a> {
    max_degree: u64,
    polys: Vec<PolyWithDegreeFudges<'a>>,
}

pub(crate) fn degree_processing<'a>(
    heights: &[u64],
    is_extra: bool,
    constraint_map: &'a ConstraintsSlice,
) -> ProcessedDegrees<'a> {
    let mut max_degree = 0;
    let mut constraints_with_degrees = ProofMap::<usize, ConstraintsWDegree<'a>>::new();
    for (i, &height) in heights.iter().enumerate() {
        let constraints = constraint_map
            .0
            .get(&i)
            .expect("constraints should contain every table");

        let boundary =
            do_degree_processing(height, &constraints.boundary, ConstraintType::Boundary);
        let row = do_degree_processing(height, &constraints.row, ConstraintType::Row);
        let transition =
            do_degree_processing(height, &constraints.transition, ConstraintType::Transition);
        let terminal =
            do_degree_processing(height, &constraints.terminal, ConstraintType::Terminal);
        let extra = if is_extra {
            do_degree_processing(height, &constraints.extra, ConstraintType::Row)
        } else {
            DegreeData {
                max_degree: 0,
                polys: Vec::new(),
            }
        };

        max_degree = max_degree
            .max(boundary.max_degree)
            .max(row.max_degree)
            .max(transition.max_degree)
            .max(terminal.max_degree)
            .max(extra.max_degree);

        constraints_with_degrees.insert(
            i,
            ConstraintsWDegree {
                boundary: boundary.polys,
                row: row.polys,
                transition: transition.polys,
                terminal: terminal.polys,
                extra: extra.polys,
            },
        );
    }

    let fri_degree_bound = 2_u64.pow((max_degree - 1).ilog2() + 1) - 1;
    ProcessedDegrees {
        fri_degree_bound,
        constraints: constraints_with_degrees,
    }
}

fn do_degree_processing<'a>(
    height: u64,
    constraints: &'a [ConstraintDataSlice<'a>],
    typ: ConstraintType,
) -> DegreeData<'a> {
    let mut max_degree = 0;
    let mut polys = Vec::<PolyWithDegreeFudges>::new();
    for constraint in constraints {
        let degrees = constraint
            .degs
            .iter()
            .map(|deg| compute_degree(&typ, height, *deg))
            .collect::<Vec<_>>();
        max_degree = cmp::max(
            max_degree,
            *degrees
                .iter()
                .max()
                .expect("constraint should contain at least one degree"),
        );
        polys.push(PolyWithDegreeFudges {
            degrees,
            poly: &constraint.constraint,
        });
    }
    DegreeData { max_degree, polys }
}

enum ConstraintType {
    Boundary,
    Row,
    Transition,
    Terminal,
}

fn compute_degree(typ: &ConstraintType, height: u64, deg: u64) -> u64 {
    match typ {
        ConstraintType::Boundary => {
            if height == 1 {
                0
            } else {
                (deg * (height - 1)) - 1
            }
        }
        ConstraintType::Row => {
            if height == 1 || deg == 1 {
                0
            } else {
                (deg * (height - 1)) - height
            }
        }
        ConstraintType::Transition => (deg - 1) * (height - 1),
        ConstraintType::Terminal => {
            if height == 1 {
                0
            } else {
                (deg * (height - 1)) - 1
            }
        }
    }
}
