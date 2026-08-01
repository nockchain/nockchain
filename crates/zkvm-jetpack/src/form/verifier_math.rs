use nockvm::jets::JetErr;
use num_traits::Pow;

use crate::form::belt::Belt;
use crate::form::felt::{fadd_, finv_, fmul_, fpow_, fsub_, Felt};
use crate::form::math::prover::ProcessedDegrees;
use crate::form::poly::{BPolySlice, FPolySlice};
use crate::form::proof::{CountMap, MPUltraSlice, ProofMap};

#[inline(always)]
pub fn bpeval_lift_(bpoly: &[Belt], x: &Felt) -> Felt {
    let mut res = Felt::zero();
    for coeff in bpoly.iter().rev() {
        res = fadd_(&Felt::lift(*coeff), &fmul_(&res, x));
    }
    res
}

#[inline(always)]
fn mpeval_mega_felt(
    mp_mega: &ProofMap<&[Belt], Belt>,
    args: &[Felt],
    chals: &[Belt],
    dyns: &[Belt],
    com_map: Option<&ProofMap<usize, Felt>>,
) -> Felt {
    use crate::form::proof::ConstraintMegaTyp::*;

    let mut acc = Felt::zero();
    for (megas, coefficient) in mp_mega {
        if coefficient.is_zero() {
            continue;
        }
        let mut acc_inner = Felt::one();
        for encoded in (*megas).iter() {
            let mega = crate::form::proof::Mega::try_from(encoded)
                .expect("valid verifier constraint term");
            let value = match mega.typ {
                VAR => args[mega.idx],
                RND => Felt::lift(chals[mega.idx]),
                DYN => Felt::lift(dyns[mega.idx]),
                CON => continue,
                COM => *com_map
                    .expect("composition dependencies are available")
                    .get(&mega.idx)
                    .expect("composition dependency exists"),
            };
            acc_inner = acc_inner * value.pow(mega.exp as usize);
        }
        acc = acc + (Felt::lift(*coefficient) * acc_inner);
    }
    acc
}

#[allow(clippy::too_many_arguments)]
pub fn eval_composition_poly_with_degrees(
    trace_evaluations: &FPolySlice<'_>,
    heights: &[u64],
    processed_degrees: &ProcessedDegrees<'_>,
    counts_map: &CountMap,
    dyn_list: &[BPolySlice<'_>],
    weights_map: &[&[Belt]],
    challenges: &BPolySlice<'_>,
    deep_challenge: &Felt,
    table_full_widths: &[u64],
    is_extra: bool,
) -> Result<Felt, JetErr> {
    let boundary_zerofier = finv_(&fsub_(deep_challenge, &Felt::one()));

    let mut acc = Felt::zero();
    let mut eval_offset = 0;

    for (i, &height) in heights.iter().enumerate() {
        let width = table_full_widths[i] as usize;
        let omicron = Felt::lift(Belt(height).ordered_root()?);
        let last_row = fsub_(deep_challenge, &finv_(&omicron));
        let terminal_zerofier = finv_(&last_row);

        let weights = weights_map
            .get(i)
            .expect("weights_map should have entry for table index");
        let constraints = processed_degrees
            .constraints
            .get(&i)
            .expect("constraints should have entry for table index");
        let counts = counts_map
            .0
            .get(&i)
            .expect("counts_map should have entry for table index");
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
            &processed_degrees.fri_degree_bound,
            deep_challenge,
        )?;
        acc = fadd_(&acc, &fmul_(&boundary_zerofier, &boundary_eval));

        let row_start = 2 * counts.boundary;
        let row_eval = evaluate_constraints(
            &constraints.row,
            dyns,
            current_evals,
            &weights[row_start..row_start + 2 * counts.row],
            challenges.0,
            &processed_degrees.fri_degree_bound,
            deep_challenge,
        )?;
        acc = fadd_(&acc, &fmul_(&row_zerofier, &row_eval));

        let trans_start = row_start + 2 * counts.row;
        let trans_eval = evaluate_constraints(
            &constraints.transition,
            dyns,
            current_evals,
            &weights[trans_start..trans_start + 2 * counts.transition],
            challenges.0,
            &processed_degrees.fri_degree_bound,
            deep_challenge,
        )?;
        acc = fadd_(&acc, &fmul_(&transition_zerofier, &trans_eval));

        let term_start = trans_start + 2 * counts.transition;
        let term_eval = evaluate_constraints(
            &constraints.terminal,
            dyns,
            current_evals,
            &weights[term_start..term_start + 2 * counts.terminal],
            challenges.0,
            &processed_degrees.fri_degree_bound,
            deep_challenge,
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
                &processed_degrees.fri_degree_bound,
                deep_challenge,
            )?;
            acc = fadd_(&acc, &fmul_(&row_zerofier, &extra_eval));
        }
    }

    Ok(acc)
}

fn evaluate_constraints(
    constraints: &[crate::form::math::prover::PolyWithDegreeFudges<'_>],
    dyns: &BPolySlice<'_>,
    evals: &[Felt],
    weights: &[Belt],
    challenges: &[Belt],
    fri_degree_bound: &u64,
    deep_challenge: &Felt,
) -> Result<Felt, JetErr> {
    let mut acc = Felt::zero();
    let mut idx = 0;

    for constraint in constraints {
        let evaled = mpeval_ultra_felt(constraint.poly, evals, challenges, dyns.0);
        for (deg, eval) in constraint.degrees.iter().zip(evaled.iter()) {
            let alpha = Felt::lift(weights[2 * idx]);
            let beta = Felt::lift(weights[2 * idx + 1]);

            let degree_factor = fpow_(deep_challenge, fri_degree_bound - deg);
            let weight_factor = fadd_(&beta, &fmul_(&alpha, &degree_factor));

            acc = fadd_(&acc, &fmul_(eval, &weight_factor));
            idx += 1;
        }
    }

    Ok(acc)
}

pub fn mpeval_ultra_felt(
    mp: &MPUltraSlice,
    args: &[Felt],
    chals: &[Belt],
    dyns: &[Belt],
) -> Vec<Felt> {
    match mp {
        MPUltraSlice::Mega(mp_mega) => vec![mpeval_mega_felt(&mp_mega.0, args, chals, dyns, None)],
        MPUltraSlice::Comp(mp_comp) => {
            let mut deps: ProofMap<usize, Felt> = ProofMap::new();
            for (i, dep) in mp_comp.dep.iter().enumerate() {
                let res = mpeval_mega_felt(&dep.0, args, chals, dyns, None);
                deps.insert(i, res);
            }

            mp_comp
                .com
                .iter()
                .map(|com| mpeval_mega_felt(&com.0, args, chals, dyns, Some(&deps)))
                .collect()
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn evaluate_deep(
    trace_evaluations: &FPolySlice<'_>,
    comp_evaluations: &FPolySlice<'_>,
    trace_elems: &[Belt],
    comp_elems: &[Belt],
    num_comp_pieces: u64,
    weights: &FPolySlice<'_>,
    heights: &[u64],
    full_widths: &[u64],
    omega: &Felt,
    index: u64,
    deep_challenge: &Felt,
    new_comp_eval: &Felt,
) -> Result<Felt, JetErr> {
    let g = Felt::lift(Belt(7));
    let omega_pow = fmul_(&fpow_(omega, index), &g);

    let mut acc = Felt::zero();
    let mut num = 0usize;
    let mut total_full_width = 0usize;

    for (i, &height) in heights.iter().enumerate() {
        let full_width = full_widths[i] as usize;
        let omicron = Felt::lift(Belt(height).ordered_root()?);

        let current_trace_elems = &trace_elems[total_full_width..(total_full_width + full_width)];

        let denom = fsub_(&omega_pow, deep_challenge);
        (acc, num) = process_belt(
            current_trace_elems, trace_evaluations.0, weights.0, full_width, num, &denom, &acc,
        );

        let denom = fsub_(&omega_pow, &fmul_(deep_challenge, &omicron));
        (acc, num) = process_belt(
            current_trace_elems, trace_evaluations.0, weights.0, full_width, num, &denom, &acc,
        );

        total_full_width += full_width;
    }

    total_full_width = 0;
    for (i, &height) in heights.iter().enumerate() {
        let full_width = full_widths[i] as usize;
        let omicron = Felt::lift(Belt(height).ordered_root()?);

        let current_trace_elems = &trace_elems[total_full_width..(total_full_width + full_width)];

        let denom = fsub_(&omega_pow, new_comp_eval);
        (acc, num) = process_belt(
            current_trace_elems, trace_evaluations.0, weights.0, full_width, num, &denom, &acc,
        );

        let denom = fsub_(&omega_pow, &fmul_(new_comp_eval, &omicron));
        (acc, num) = process_belt(
            current_trace_elems, trace_evaluations.0, weights.0, full_width, num, &denom, &acc,
        );

        total_full_width += full_width;
    }

    let denom = fsub_(&omega_pow, &fpow_(deep_challenge, num_comp_pieces));

    (acc, _) = process_belt(
        comp_elems,
        comp_evaluations.0,
        &weights.0[num..],
        num_comp_pieces as usize,
        0,
        &denom,
        &acc,
    );

    Ok(acc)
}

/// Evaluate the V3 random batch that enforces each trace column's declared
/// per-table degree. For a table of height `h` and global height `H`, an honest
/// column has `deg(T) < h`, hence `deg(X^(H-h) * T) < H`. A zerofier shift
/// `T + c(X^h - 1)` reaches degree `H` and fails the existing strict FRI bound,
/// except with the extension-field probability of a random leading-term
/// cancellation.
pub fn evaluate_trace_degree_normalization(
    trace_elems: &[Belt],
    weights: &[Felt],
    heights: &[u64],
    full_widths: &[u64],
    x: &Felt,
) -> Option<Felt> {
    if heights.len() != full_widths.len() {
        return None;
    }
    let max_height = heights.iter().copied().max()?;
    let expected_cols = full_widths.iter().try_fold(0usize, |acc, width| {
        acc.checked_add(usize::try_from(*width).ok()?)
    })?;
    if trace_elems.len() != expected_cols || weights.len() != expected_cols {
        return None;
    }

    let mut acc = Felt::zero();
    let mut offset = 0usize;
    for (&height, &width) in heights.iter().zip(full_widths) {
        let width = usize::try_from(width).ok()?;
        let end = offset.checked_add(width)?;
        let degree_factor = fpow_(x, max_height.checked_sub(height)?);
        for (elem, weight) in trace_elems[offset..end].iter().zip(&weights[offset..end]) {
            let term = fmul_(weight, &fmul_(&degree_factor, &Felt::lift(*elem)));
            acc = fadd_(&acc, &term);
        }
        offset = end;
    }
    Some(acc)
}

fn process_belt(
    elems: &[Belt],
    evals: &[Felt],
    weights: &[Felt],
    width: usize,
    start_num: usize,
    denom: &Felt,
    acc_start: &Felt,
) -> (Felt, usize) {
    let mut acc = *acc_start;
    let mut num = start_num;
    let denom_inv = finv_(denom);

    for elem in &elems[..width] {
        let elem_val = Felt::lift(*elem);
        let eval_val = evals[num];
        let weight_val = weights[num];

        let diff = fsub_(&elem_val, &eval_val);
        let term = fmul_(&fmul_(&diff, &denom_inv), &weight_val);
        acc = fadd_(&acc, &term);

        num += 1;
    }

    (acc, num)
}

#[cfg(test)]
mod tests {
    use nockchain_math::fpoly::{fpdiv_, fpeval};

    use super::*;
    use crate::form::poly::{Poly, PolyVec};

    #[test]
    fn normalized_trace_term_exposes_zerofier_degree() {
        const HEIGHT: usize = 4;
        const MAX_HEIGHT: usize = 8;

        let c = Felt::lift(Belt(7));
        let mut shifted_trace = vec![Felt::zero(); HEIGHT + 1];
        shifted_trace[0] = -c;
        shifted_trace[HEIGHT] = c;

        let r = Felt::lift(Belt(2));
        let opening = fpeval(&shifted_trace, r);
        let mut numerator = shifted_trace.clone();
        numerator[0] = fsub_(&numerator[0], &opening);
        let quotient = fpdiv_(&numerator, &[-r, Felt::one()]);
        assert_eq!(PolyVec(quotient).degree(), (HEIGHT - 1) as u32);

        let mut normalized = vec![Felt::zero(); MAX_HEIGHT - HEIGHT];
        normalized.extend_from_slice(&shifted_trace);
        assert_eq!(PolyVec(normalized).degree(), MAX_HEIGHT as u32);
    }
}
