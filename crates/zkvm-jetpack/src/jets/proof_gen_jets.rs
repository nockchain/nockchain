use std::cmp;
use std::collections::BTreeMap;

use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::noun::{IndirectAtom, Noun, NounSpace, D};
use nockvm_macros::tas;
use noun_serde::NounDecode;
use tracing::debug;

use crate::form::belt::Belt;
use crate::form::felt::*;
use crate::form::handle::{finalize_poly, new_handle_mut_felt, new_handle_mut_slice};
use crate::form::mary::*;
use crate::form::math::prover::*;
use crate::form::noun_ext::NounMathExt;
use crate::form::poly::*;
use crate::form::structs::{HoonList, HoonMapIter};
use crate::jets::verifier_jets::mpeval_ultra_felt;

pub enum MPUltra {
    Mega(Noun),
    Comp(MPComp),
}

pub struct MPComp {
    pub dep: Vec<Noun>,
    pub com: Vec<Noun>,
}

impl MPComp {
    pub fn try_from(noun: Noun, space: &NounSpace) -> Result<Self, JetErr> {
        let dep_list = HoonList::try_from(slot(noun, 2, space)?, space)?;
        let com_list = HoonList::try_from(slot(noun, 3, space)?, space)?;

        let mut dep = Vec::with_capacity(dep_list.count());
        let mut com = Vec::with_capacity(com_list.count());

        for dep_noun in dep_list {
            dep.push(dep_noun);
        }

        for com_noun in com_list {
            com.push(com_noun);
        }

        Ok(MPComp { dep, com })
    }
}

impl MPUltra {
    pub fn try_from(mp_ultra: Noun, space: &NounSpace) -> Result<Self, JetErr> {
        let mp_ultra_cell = mp_ultra.in_space(space).as_cell().unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        match mp_ultra_cell
            .head()
            .as_atom()
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            })
            .as_u64()
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            }) {
            tas!(b"mega") => Ok(MPUltra::Mega(mp_ultra_cell.tail().noun())),
            tas!(b"comp") => Ok(MPUltra::Comp(MPComp::try_from(
                mp_ultra_cell.tail().noun(),
                space,
            )?)),
            _ => panic!("Invalid MPUltra type"),
        }
    }
}
pub struct CountMap(pub ProofMap<usize, Counts>);
pub struct IndexBPolyMap<'a>(pub ProofMap<usize, &'a [Belt]>);

pub struct Counts {
    pub boundary: usize,
    pub row: usize,
    pub transition: usize,
    pub terminal: usize,
    pub extra: usize,
}

pub type ProofMap<K, V> = BTreeMap<K, V>;
pub struct Constraints(pub ProofMap<usize, MPDenseConstraints>);

pub struct MPDenseConstraints {
    pub boundary: Vec<ConstraintData>,
    pub row: Vec<ConstraintData>,
    pub transition: Vec<ConstraintData>,
    pub terminal: Vec<ConstraintData>,
    pub extra: Vec<ConstraintData>,
}
pub struct ConstraintData {
    pub constraint: MPUltra,
    pub degs: Vec<u64>,
}

impl CountMap {
    pub fn try_from(noun: Noun, space: &NounSpace) -> Result<Self, JetErr> {
        let noun_handle = noun.in_space(space);
        let counts = HoonMapIter::new(&noun_handle);

        let mut outer = ProofMap::<usize, Counts>::new();

        for term_noun in counts.into_iter() {
            let (k, v): (usize, Counts) = {
                let term_cell = term_noun.as_cell().unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
                (term_cell.head().as_atom()?.as_u64()? as usize, {
                    let tail = term_cell.tail().noun();
                    Counts {
                        boundary: slot(tail, 2, space)?.in_space(space).as_atom()?.as_u64()?
                            as usize,
                        row: slot(tail, 6, space)?.in_space(space).as_atom()?.as_u64()? as usize,
                        transition: slot(tail, 14, space)?.in_space(space).as_atom()?.as_u64()?
                            as usize,
                        terminal: slot(tail, 30, space)?.in_space(space).as_atom()?.as_u64()?
                            as usize,
                        extra: slot(tail, 31, space)?.in_space(space).as_atom()?.as_u64()? as usize,
                    }
                })
            };
            outer.insert(k, v);
        }
        Ok(CountMap(outer))
    }
}

impl IndexBPolyMap<'_> {
    pub fn try_from(hoon_map: Noun, space: &NounSpace) -> Result<Self, JetErr> {
        let mut composition_chals = ProofMap::<usize, &[Belt]>::new();
        let hoon_map_handle = hoon_map.in_space(space);
        let hoon_map = HoonMapIter::new(&hoon_map_handle);

        for term_noun in hoon_map.into_iter() {
            let (k, v): (usize, &[Belt]) = {
                let term_cell = term_noun.as_cell().unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
                (
                    term_cell.head().as_atom()?.as_u64()? as usize,
                    BPolySlice::try_from(term_cell.tail().noun(), space)
                        .unwrap_or_else(|err| {
                            panic!(
                                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                                file!(),
                                line!(),
                                option_env!("GIT_SHA")
                            )
                        })
                        .0,
                )
            };
            composition_chals.insert(k, v);
        }
        Ok(IndexBPolyMap(composition_chals))
    }
}

impl Constraints {
    pub fn try_from(hoon_map: Noun, space: &NounSpace) -> Result<Self, JetErr> {
        let hoon_map_handle = hoon_map.in_space(space);
        let hoon_map = HoonMapIter::new(&hoon_map_handle);
        let mut constraints = ProofMap::new();

        for term_noun in hoon_map.into_iter() {
            let (k, v): (usize, MPDenseConstraints) = {
                let term_cell = term_noun.as_cell().unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
                (
                    term_cell.head().as_atom()?.as_u64()? as usize,
                    MPDenseConstraints::try_from(term_cell.tail().noun(), space)?,
                )
            };

            constraints.insert(k, v);
        }
        Ok(Constraints(constraints))
    }
}

impl MPDenseConstraints {
    pub fn try_from(noun: Noun, space: &NounSpace) -> Result<Self, JetErr> {
        let [boundary, row, transition, terminal, extra] = noun.uncell(space)?;
        let boundary: Vec<ConstraintData> = HoonList::try_from(boundary, space)?
            .map(|x| ConstraintData::try_from(x, space))
            .collect::<Result<Vec<ConstraintData>, _>>()?;
        let row: Vec<ConstraintData> = HoonList::try_from(row, space)?
            .map(|x| ConstraintData::try_from(x, space))
            .collect::<Result<Vec<ConstraintData>, _>>()?;
        let transition: Vec<ConstraintData> = HoonList::try_from(transition, space)?
            .map(|x| ConstraintData::try_from(x, space))
            .collect::<Result<Vec<ConstraintData>, _>>()?;
        let terminal: Vec<ConstraintData> = HoonList::try_from(terminal, space)?
            .map(|x| ConstraintData::try_from(x, space))
            .collect::<Result<Vec<ConstraintData>, _>>()?;
        let extra: Vec<ConstraintData> = HoonList::try_from(extra, space)?
            .map(|x| ConstraintData::try_from(x, space))
            .collect::<Result<Vec<ConstraintData>, _>>()?;

        Ok(MPDenseConstraints {
            boundary,
            row,
            transition,
            terminal,
            extra,
        })
    }
}

impl ConstraintData {
    pub fn try_from(noun: Noun, space: &NounSpace) -> Result<Self, JetErr> {
        let cell = noun.in_space(space).as_cell()?;
        let cs = MPUltra::try_from(cell.head().noun(), space)?;
        let degs: Vec<u64> = HoonList::try_from(cell.tail().noun(), space)?
            .map(|n| n.in_space(space).as_atom()?.as_u64())
            .collect::<Result<Vec<u64>, _>>()?;
        Ok(ConstraintData {
            constraint: cs,
            degs,
        })
    }
}

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

    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);
    Ok(res_cell)
}

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
    let constraint_map = Constraints::try_from(constraint_map, &space)?;
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
        .map(|x| {
            x.in_space(&space)
                .as_atom()
                .expect("table_full_widths element should be an atom")
                .as_u64()
                .expect("table_full_widths element should be a u64")
        })
        .collect();
    let is_extra = unsafe { is_extra.raw_equals(&D(0)) };

    let res = eval_composition_poly(
        &trace_evaluations, &heights, &constraint_map, &counts_map, &dyn_list, &weights_map,
        &challenges, deep_challenge, &table_full_widths, is_extra, &space,
    )?;

    let (res_atom, res_felt): (IndirectAtom, &mut Felt) = new_handle_mut_felt(&mut context.stack);
    res_felt.copy_from_slice(&res);
    Ok(res_atom.as_noun())
}

#[allow(clippy::too_many_arguments)]
fn eval_composition_poly(
    trace_evaluations: &FPolySlice<'_>,
    heights: &[u64],
    constraint_map: &Constraints,
    counts_map: &CountMap,
    dyn_list: &[BPolySlice<'_>],
    weights_map: &IndexBPolyMap<'_>,
    challenges: &BPolySlice<'_>,
    deep_challenge: &Felt,
    table_full_widths: &[u64],
    is_extra: bool,
    space: &NounSpace,
) -> Result<Felt, JetErr> {
    let dp = degree_processing(heights, is_extra, constraint_map);

    let boundary_zerofier = finv_(&fsub_(deep_challenge, &Felt::one()));

    let mut acc = Felt::zero();
    let mut eval_offset = 0;

    for (i, &height) in heights.iter().enumerate() {
        let width = table_full_widths[i] as usize;
        let omicron = Felt::lift(Belt(height).ordered_root()?);
        let last_row = fsub_(deep_challenge, &finv_(&omicron));
        let terminal_zerofier = finv_(&last_row);

        let weights = weights_map
            .0
            .get(&i)
            .expect("weights_map should have entry for index");
        let constraints = dp
            .constraints
            .get(&i)
            .expect("constraints should have entry for index");
        let counts = counts_map
            .0
            .get(&i)
            .expect("counts_map should have entry for index");
        let dyns = &dyn_list[i];

        let row_zerofier = finv_(&fsub_(&fpow_(deep_challenge, height), &Felt::one()));

        let transition_zerofier = fmul_(&last_row, &row_zerofier);

        let current_evals = &trace_evaluations.0[eval_offset..eval_offset + 2 * width];
        eval_offset += 2 * width;

        let boundary_eval = evaluate_constraints(
            &constraints.boundary,
            dyns,
            current_evals,
            &weights[0..2 * counts.boundary],
            challenges.0,
            &dp.fri_degree_bound,
            deep_challenge,
            space,
        )?;
        acc = fadd_(&acc, &fmul_(&boundary_zerofier, &boundary_eval));

        let row_start = 2 * counts.boundary;
        let row_eval = evaluate_constraints(
            &constraints.row,
            dyns,
            current_evals,
            &weights[row_start..row_start + 2 * counts.row],
            challenges.0,
            &dp.fri_degree_bound,
            deep_challenge,
            space,
        )?;
        acc = fadd_(&acc, &fmul_(&row_zerofier, &row_eval));

        let trans_start = row_start + 2 * counts.row;
        let trans_eval = evaluate_constraints(
            &constraints.transition,
            dyns,
            current_evals,
            &weights[trans_start..trans_start + 2 * counts.transition],
            challenges.0,
            &dp.fri_degree_bound,
            deep_challenge,
            space,
        )?;
        acc = fadd_(&acc, &fmul_(&transition_zerofier, &trans_eval));

        let term_start = trans_start + 2 * counts.transition;
        let term_eval = evaluate_constraints(
            &constraints.terminal,
            dyns,
            current_evals,
            &weights[term_start..term_start + 2 * counts.terminal],
            challenges.0,
            &dp.fri_degree_bound,
            deep_challenge,
            space,
        )?;
        acc = fadd_(&acc, &fmul_(&terminal_zerofier, &term_eval));

        if is_extra {
            let extra_start = term_start + 2 * counts.terminal;
            let extra_eval = evaluate_constraints(
                &constraints.extra,
                dyns,
                current_evals,
                &weights[extra_start..],
                challenges.0,
                &dp.fri_degree_bound,
                deep_challenge,
                space,
            )?;
            acc = fadd_(&acc, &fmul_(&row_zerofier, &extra_eval));
        }
    }

    Ok(acc)
}

#[allow(clippy::too_many_arguments)]
fn evaluate_constraints(
    constraints: &[PolyWithDegreeFudges<'_>],
    dyns: &BPolySlice<'_>,
    evals: &[Felt],
    weights: &[Belt],
    challenges: &[Belt],
    fri_degree_bound: &u64,
    deep_challenge: &Felt,
    space: &NounSpace,
) -> Result<Felt, JetErr> {
    let mut acc = Felt::zero();
    let mut idx = 0;

    for constraint in constraints {
        let evaled = mpeval_ultra_felt(constraint.poly, evals, challenges, dyns.0, space)?;
        for (deg, eval) in constraint.degrees.iter().zip(evaled.iter()) {
            let alpha = Felt::lift(weights[2 * idx]);
            let beta = Felt::lift(weights[2 * idx + 1]);

            // Degree adjustment: alpha * X^(fri_degree_bound - deg) + beta
            let degree_factor = fpow_(deep_challenge, fri_degree_bound - deg);
            let weight_factor = fadd_(&beta, &fmul_(&alpha, &degree_factor));

            acc = fadd_(&acc, &fmul_(eval, &weight_factor));
            idx += 1;
        }
    }

    Ok(acc)
}

// Degree fudge factor for polynomial
type DegreeFudge = u64;

// MPUltra with a corresponding list of degree fudge factors for each poly
pub struct PolyWithDegreeFudges<'a> {
    pub degrees: Vec<DegreeFudge>,
    pub poly: &'a MPUltra,
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

pub fn degree_processing<'a>(
    heights: &[u64],
    is_extra: bool,
    constraint_map: &'a Constraints,
) -> ProcessedDegrees<'a> {
    let mut max_degree: u64 = 0;
    let mut res = ProofMap::<usize, ConstraintsWDegree<'a>>::new();
    for (i, &height) in heights.iter().enumerate() {
        let constraints = constraint_map.0.get(&i).unwrap_or_else(|| {
            panic!(
                "Panicked at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });

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
                polys: vec![],
            }
        };

        let data = ConstraintsWDegree {
            boundary: boundary.polys,
            row: row.polys,
            transition: transition.polys,
            terminal: terminal.polys,
            extra: extra.polys,
        };
        max_degree = max_degree
            .max(boundary.max_degree)
            .max(row.max_degree)
            .max(transition.max_degree)
            .max(terminal.max_degree)
            .max(extra.max_degree);
        res.insert(i, data);
    }
    let fri_degree_bound = 2_u64.pow((max_degree - 1).ilog2() + 1) - 1;
    ProcessedDegrees {
        fri_degree_bound,
        constraints: res,
    }
}

fn do_degree_processing(height: u64, cs: &[ConstraintData], typ: ConstraintType) -> DegreeData<'_> {
    let mut max_degree: u64 = 0;
    let mut res = Vec::<PolyWithDegreeFudges>::new();
    cs.iter().for_each(|cd| {
        let new_degs: Vec<u64> = cd
            .degs
            .iter()
            .map(|deg| compute_degree(&typ, height, *deg))
            .collect();
        max_degree = cmp::max(
            max_degree,
            *(new_degs.iter().max().unwrap_or_else(|| {
                panic!(
                    "Panicked at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            })),
        );
        res.push(PolyWithDegreeFudges {
            degrees: new_degs,
            poly: &cd.constraint,
        });
    });
    DegreeData {
        max_degree,
        polys: res,
    }
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

    let compute_deep_res = compute_deep(
        trace_polys, trace_openings.0, composition_pieces, composition_piece_openings.0, weights.0,
        omicrons.0, deep_challenge, comp_eval_point, &space,
    );

    let (res, res_poly): (IndirectAtom, &mut [Felt]) =
        new_handle_mut_slice(&mut context.stack, Some(compute_deep_res.len()));

    res_poly.copy_from_slice(compute_deep_res.as_slice());

    let res_cell = finalize_poly(&mut context.stack, Some(res_poly.len()), res);
    Ok(res_cell)
}
