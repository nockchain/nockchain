use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::mem::NockStack;
use nockvm::noun::{Atom, IndirectAtom, Noun, NounSpace};
use nockvm_macros::tas;
use tracing::debug;

use crate::form::belt::{bpow, Belt};
use crate::form::felt::*;
use crate::form::handle::new_handle_mut_felt;
use crate::form::noun_ext::{AtomMathExt, NounMathExt, NounMathExtHandle};
use crate::form::poly::{BPolySlice, Element, FPolySlice, Poly};
use crate::form::proof::ProofMap;
use crate::form::structs::{HoonList, HoonMapIter};
pub struct IndexFeltMap(pub ProofMap<usize, Felt>);
pub struct IndexBeltMap(pub ProofMap<usize, Belt>);

impl IndexFeltMap {
    pub fn try_from(hoon_map: Noun, space: &NounSpace) -> Result<Self, JetErr> {
        let hoon_map_handle = hoon_map.in_space(space);
        let hoon_map = HoonMapIter::new(&hoon_map_handle);
        let mut map = ProofMap::<usize, Felt>::new();

        for term_noun in hoon_map.into_iter() {
            let (k, v): (usize, Felt) = {
                let term_cell = term_noun.as_cell()?;
                (
                    term_cell.head().as_atom()?.as_u64()? as usize,
                    *term_cell.tail().as_atom()?.atom().as_felt(space)?,
                )
            };

            map.insert(k, v);
        }
        Ok(IndexFeltMap(map))
    }
}

impl IndexBeltMap {
    pub fn try_from(hoon_map: Noun, space: &NounSpace) -> Result<Self, JetErr> {
        let hoon_map_handle = hoon_map.in_space(space);
        let hoon_map = HoonMapIter::new(&hoon_map_handle);
        let mut map = ProofMap::<usize, Belt>::new();

        for term_noun in hoon_map.into_iter() {
            let (k, v): (usize, Belt) = {
                let term_cell = term_noun.as_cell()?;
                (
                    term_cell.head().as_atom()?.as_u64()? as usize,
                    term_cell.tail().as_atom()?.atom().as_belt(space)?,
                )
            };

            map.insert(k, v);
        }
        Ok(IndexBeltMap(map))
    }
}

/// Decode a Hoon list of u64 atoms. Proof nouns are attacker-supplied, so a
/// list element with the wrong shape must be an error, never a panic: the
/// serf thread resumes panics, which kills the node.
fn hoon_u64_list(noun: Noun, space: &NounSpace) -> Result<Vec<u64>, JetErr> {
    HoonList::try_from(noun, space)?
        .into_iter()
        .map(|x| Ok(x.in_space(space).as_atom()?.as_u64()?))
        .collect()
}

/// Decode a Hoon list of Belt atoms, with the same no-panic guarantee.
fn hoon_belt_list(noun: Noun, space: &NounSpace) -> Result<Vec<Belt>, JetErr> {
    Ok(hoon_u64_list(noun, space)?.into_iter().map(Belt).collect())
}

pub fn evaluate_deep_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let mut sam_cur = sam.in_space(&space).as_cell()?;

    // Extract all parameters from the subject
    let trace_evaluations = sam_cur.head().noun();
    sam_cur = sam_cur.tail().as_cell()?;
    let comp_evaluations = sam_cur.head().noun();
    sam_cur = sam_cur.tail().as_cell()?;
    let trace_elems = sam_cur.head().noun();
    sam_cur = sam_cur.tail().as_cell()?;
    let comp_elems = sam_cur.head().noun();
    sam_cur = sam_cur.tail().as_cell()?;
    let num_comp_pieces = sam_cur.head().noun();
    sam_cur = sam_cur.tail().as_cell()?;
    let weights = sam_cur.head().noun();
    sam_cur = sam_cur.tail().as_cell()?;
    let heights = sam_cur.head().noun();
    sam_cur = sam_cur.tail().as_cell()?;
    let full_widths = sam_cur.head().noun();
    sam_cur = sam_cur.tail().as_cell()?;
    let omega = sam_cur.head().noun();
    sam_cur = sam_cur.tail().as_cell()?;
    let index = sam_cur.head().noun();
    sam_cur = sam_cur.tail().as_cell()?;
    let deep_challenge = sam_cur.head().noun();
    let new_comp_eval = sam_cur.tail().noun();

    // Convert nouns to appropriate types
    let Ok(trace_evaluations) = FPolySlice::try_from(trace_evaluations, &space) else {
        debug!("trace_evaluations is not a valid FPolySlice");
        return Err(BAIL_FAIL);
    };
    let Ok(comp_evaluations) = FPolySlice::try_from(comp_evaluations, &space) else {
        debug!("comp_evaluations is not a valid FPolySlice");
        return Err(BAIL_FAIL);
    };
    let trace_elems = hoon_belt_list(trace_elems, &space)?;
    let comp_elems = hoon_belt_list(comp_elems, &space)?;
    let num_comp_pieces = num_comp_pieces.in_space(&space).as_atom()?.as_u64()?;
    let Ok(weights) = FPolySlice::try_from(weights, &space) else {
        debug!("weights is not a valid FPolySlice");
        return Err(BAIL_FAIL);
    };
    let heights = hoon_u64_list(heights, &space)?;
    let full_widths = hoon_u64_list(full_widths, &space)?;
    if heights.len() != full_widths.len() {
        debug!("heights and full_widths must name the same tables");
        return Err(BAIL_FAIL);
    }
    let omega = omega.as_felt(&space)?;
    let index = index.in_space(&space).as_atom()?.as_u64()?;
    let deep_challenge = deep_challenge.as_felt(&space)?;
    let new_comp_eval = new_comp_eval.as_felt(&space)?;

    //  TODO use g defined wherever it is
    let g = Felt::lift(Belt(7));
    let omega_pow = fmul_(&fpow_(omega, index), &g);

    let mut acc = Felt::zero();
    let mut num = 0usize;
    let mut total_full_width = 0usize;

    for (&height, &declared_width) in heights.iter().zip(&full_widths) {
        let full_width = usize::try_from(declared_width).map_err(|_| BAIL_FAIL)?;
        let omicron = Felt::lift(Belt(height).ordered_root()?);

        let end = total_full_width
            .checked_add(full_width)
            .filter(|end| *end <= trace_elems.len())
            .ok_or(BAIL_FAIL)?;
        let current_trace_elems = &trace_elems[total_full_width..end];

        // Process first row trace columns
        let denom = fsub_(&omega_pow, deep_challenge);
        (acc, num) = process_belt(
            current_trace_elems, trace_evaluations.0, weights.0, full_width, num, &denom, &acc,
        )?;

        // Process second row trace columns (shifted by omicron)
        let denom = fsub_(&omega_pow, &fmul_(deep_challenge, &omicron));
        (acc, num) = process_belt(
            current_trace_elems, trace_evaluations.0, weights.0, full_width, num, &denom, &acc,
        )?;

        total_full_width = end;
    }

    total_full_width = 0;
    for (&height, &declared_width) in heights.iter().zip(&full_widths) {
        let full_width = usize::try_from(declared_width).map_err(|_| BAIL_FAIL)?;
        let omicron = Felt::lift(Belt(height).ordered_root()?);

        let end = total_full_width
            .checked_add(full_width)
            .filter(|end| *end <= trace_elems.len())
            .ok_or(BAIL_FAIL)?;
        let current_trace_elems = &trace_elems[total_full_width..end];

        // Process first row trace columns with new_comp_eval
        let denom = fsub_(&omega_pow, new_comp_eval);
        (acc, num) = process_belt(
            current_trace_elems, trace_evaluations.0, weights.0, full_width, num, &denom, &acc,
        )?;

        // Process second row trace columns with new_comp_eval (shifted by omicron)
        let denom = fsub_(&omega_pow, &fmul_(new_comp_eval, &omicron));
        (acc, num) = process_belt(
            current_trace_elems, trace_evaluations.0, weights.0, full_width, num, &denom, &acc,
        )?;

        total_full_width = end;
    }

    // Process composition elements
    let denom = fsub_(&omega_pow, &fpow_(deep_challenge, num_comp_pieces));

    let comp_width = usize::try_from(num_comp_pieces).map_err(|_| BAIL_FAIL)?;
    let comp_weights = weights.0.get(num..).ok_or(BAIL_FAIL)?;
    (acc, _) = process_belt(
        &comp_elems, comp_evaluations.0, comp_weights, comp_width, 0, &denom, &acc,
    )?;
    // Return the result as a Noun
    let (res_atom, res_felt): (IndirectAtom, &mut Felt) = new_handle_mut_felt(&mut context.stack);
    *res_felt = acc;

    Ok(res_atom.as_noun())
}

// Helper function for processing belts
fn process_belt(
    elems: &[Belt],
    evals: &[Felt],
    weights: &[Felt],
    width: usize,
    start_num: usize,
    denom: &Felt,
    acc_start: &Felt,
) -> Result<(Felt, usize), JetErr> {
    let mut acc = *acc_start;
    let mut num = start_num;

    for elem in elems.iter().take(width) {
        let elem_val = Felt::lift(*elem);
        let eval_val = *evals.get(num).ok_or(BAIL_FAIL)?;
        let weight_val = *weights.get(num).ok_or(BAIL_FAIL)?;

        // (elem_val - eval_val) / denom * weight_val + acc
        let diff = fsub_(&elem_val, &eval_val);
        let term = fmul_(&fdiv_(&diff, denom), &weight_val);
        acc = fadd_(&acc, &term);

        num += 1;
    }

    Ok((acc, num))
}

// =/  add-op   ?:(=(field %base) badd fadd)
// =/  mul-op   ?:(=(field %base) bmul fmul)
// =/  aop-door   ?:(=(field %base) bop fop)
// =/  init-zero=@ux  (lift-op 0)
// =/  init-one=@ux  (lift-op 1)
trait Fops:
    Element + Copy + core::ops::Add<Output = Self> + core::ops::Mul<Output = Self> + PartialEq + Eq
{
    fn to_noun(self, stack: &mut NockStack) -> Noun;
    fn from_noun(noun: Noun, space: &NounSpace) -> Result<Self, JetErr>;
    // =/  pow-op   ?:(=(field %base) bpow fpow)
    fn pow(&self, exp: u64) -> Self;
    // =/  lift-op  ?:(=(field %base) |=(v=@ `@ux`v) lift)
    fn lift(v: Belt) -> Self;
}

impl Fops for Belt {
    fn to_noun(self, stack: &mut NockStack) -> Noun {
        Atom::new(stack, self.0).as_noun()
    }

    fn from_noun(noun: Noun, space: &NounSpace) -> Result<Self, JetErr> {
        Ok(Belt(noun.in_space(space).as_atom()?.as_u64()?))
    }

    fn pow(&self, exp: u64) -> Self {
        Self(bpow(self.0, exp))
    }

    fn lift(v: Belt) -> Self {
        v
    }
}

impl Fops for Felt {
    fn to_noun(self, stack: &mut NockStack) -> Noun {
        let (a, b) = new_handle_mut_felt(stack);
        *b = self;
        a.as_noun()
    }

    fn from_noun(noun: Noun, space: &NounSpace) -> Result<Self, JetErr> {
        if let Ok(r) = noun.as_felt(space) {
            Ok(*r)
        } else {
            Err(BAIL_FAIL)
        }
    }

    fn pow(&self, exp: u64) -> Self {
        fpow_(self, exp)
    }

    fn lift(v: Belt) -> Self {
        Felt::lift(v)
    }
}

pub fn mpeval_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    // |=  $:  field=?(%ext %base)
    //         mp=mp-mega
    //         args=bpoly  :: can be bpoly or fpoly
    //         chals=bpoly
    //         dyns=bpoly
    //         com-map=(map @ elt)
    //     ==
    // ^-  elt
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let stack = &mut context.stack;
    let [field, mp, args, chal_map, dyns, com_map] = sam.uncell(&space)?;
    let chals = BPolySlice::try_from(chal_map, &space)?.0;

    let Ok(dyns) = BPolySlice::try_from(dyns, &space) else {
        return Err(BAIL_FAIL);
    };

    let ret = match field.as_direct()?.data() {
        tas!(b"ext") => {
            let Ok(com_map) = IndexFeltMap::try_from(com_map, &space) else {
                return Err(BAIL_FAIL);
            };
            let args = FPolySlice::try_from(args, &space)?.0;
            mpeval::<Felt>(mp, args, chals, dyns.0, Some(&com_map.0), &space)?.to_noun(stack)
        }
        tas!(b"base") => {
            let Ok(com_map) = IndexBeltMap::try_from(com_map, &space) else {
                return Err(BAIL_FAIL);
            };
            let args = BPolySlice::try_from(args, &space)?.0;
            mpeval::<Belt>(mp, args, chals, dyns.0, Some(&com_map.0), &space)?.to_noun(stack)
        }
        _ => return Err(BAIL_FAIL),
    };

    Ok(ret)
}

fn mpeval<F: Fops>(
    mp: Noun,
    args: &[F],
    chals: &[Belt],
    dyns: &[Belt],
    com_map: Option<&ProofMap<usize, F>>,
    space: &NounSpace,
    //com_map: Noun,
) -> Result<F, JetErr> {
    /*
    let Ok(args) = PolySlice::try_from(args) else {
        return jet_err();
    };

    let Ok(chals) = BPolySlice::try_from(chals) else {
        return jet_err();
    };
    */

    //let com_map = HoonMap::try_from(com_map).ok();

    // ?:  =(~ mp)
    if mp.is_atom() {
        if mp.is_direct() && mp.as_direct()?.data() == 0 {
            return Ok(F::zero());
        } else {
            return Err(BAIL_FAIL);
        }
    }

    // %+  roll  ~(tap by mp)
    // |=  [[k=bpoly v=belt] acc=_init-zero]
    let mp_handle = mp.in_space(space);
    let mut mp = HoonMapIter::new(&mp_handle);

    mp.try_fold(F::zero(), |acc, n| {
        let [k, v] = n.uncell()?;

        let Ok(k) = BPolySlice::try_from(k.noun(), space) else {
            return Err(BAIL_FAIL);
        };
        let v = Belt::from_noun(v.noun(), space)?;
        // =/  coeff=@ux  (lift-op v)
        let coeff = F::lift(v);
        // ?:  =(init-zero coeff)
        if coeff == F::zero() {
            // acc
            return Ok(acc);
        }

        // %+  add-op  acc
        // %+  mul-op  coeff
        // %+  roll  (range len.k)
        // |=  [i=@ res=_init-one]
        // ?:  =(init-zero res)
        //   init-zero
        let res = k.iter().copied().try_fold(F::one(), |res, ter| {
            let (typ, idx, exp) = crate::form::brek(ter)?;
            let factor = match typ {
                crate::form::MegaTyp::Var => args.get(idx).ok_or(BAIL_FAIL)?.pow(exp),
                crate::form::MegaTyp::Rnd => F::lift(*chals.get(idx).ok_or(BAIL_FAIL)?).pow(exp),
                crate::form::MegaTyp::Dyn => F::lift(*dyns.get(idx).ok_or(BAIL_FAIL)?).pow(exp),
                crate::form::MegaTyp::Con => F::one(),
                crate::form::MegaTyp::Com => com_map
                    .and_then(|map| map.get(&idx))
                    .ok_or(BAIL_FAIL)?
                    .pow(exp),
            };
            Ok::<F, JetErr>(res * factor)
        })?;

        Ok(acc + (coeff * res))
    })
}

#[cfg(test)]
mod tests {
    use nockvm::mem::NockStack;
    use nockvm::noun::{D, T};

    use super::{mpeval, IndexBeltMap, IndexFeltMap};
    use crate::form::belt::Belt;
    use crate::form::handle::{finalize_poly, new_handle_mut_slice};
    use crate::form::proof::ProofMap;

    #[test]
    fn malformed_map_slots_return_errors() {
        let mut stack = NockStack::new(1 << 20, 0);
        let malformed_map = T(&mut stack, &[D(0), D(0), D(0)]);
        let space = stack.noun_space();

        assert!(IndexFeltMap::try_from(malformed_map, &space).is_err());
        assert!(IndexBeltMap::try_from(malformed_map, &space).is_err());
    }

    #[test]
    fn mpeval_rejects_missing_composition_dependency() {
        let mut stack = NockStack::new(1 << 20, 0);
        let encoded_com_term = 4 | (99 << 3) | (1 << 13);
        let (key_atom, key_data) = new_handle_mut_slice(&mut stack, Some(1));
        key_data[0] = Belt(encoded_com_term);
        let key = finalize_poly(&mut stack, Some(1), key_atom);
        let entry = T(&mut stack, &[key, D(1)]);
        let polynomial = T(&mut stack, &[entry, D(0), D(0)]);
        let space = stack.noun_space();
        let composition_map = ProofMap::new();

        assert!(
            mpeval::<Belt>(polynomial, &[], &[], &[], Some(&composition_map), &space,).is_err()
        );
    }

    #[test]
    fn mpeval_rejects_invalid_mega_term_type() {
        let mut stack = NockStack::new(1 << 20, 0);
        // %invalid term: type=5, idx=1, exp=1
        let encoded_bad_term = 5 | (1 << 3) | (1 << 13);
        let (key_atom, key_data) = new_handle_mut_slice(&mut stack, Some(1));
        key_data[0] = Belt(encoded_bad_term);
        let key = finalize_poly(&mut stack, Some(1), key_atom);
        let entry = T(&mut stack, &[key, D(1)]);
        let polynomial = T(&mut stack, &[entry, D(0), D(0)]);
        let space = stack.noun_space();
        let composition_map = ProofMap::new();

        assert!(
            mpeval::<Belt>(polynomial, &[], &[], &[], Some(&composition_map), &space,).is_err()
        );
    }

    #[test]
    fn evaluate_deep_jet_rejects_malformed_proof_inputs() {
        use std::panic::{catch_unwind, AssertUnwindSafe};

        use nockvm::jets::util::test::init_context;
        use nockvm::noun::IndirectAtom;

        use super::evaluate_deep_jet;

        // A valid zero-length fpoly: [len=0 | indirect backing atom].
        let fpoly = |stack: &mut NockStack| {
            let words = [0_u64];
            let dat =
                unsafe { IndirectAtom::new_raw(stack, words.len(), words.as_ptr()) }.as_noun();
            T(stack, &[D(0), dat])
        };

        // (field count and order must match evaluate_deep_jet's sample.)
        let subject_for = |stack: &mut NockStack,
                           trace_elems: nockvm::noun::Noun,
                           heights: nockvm::noun::Noun,
                           full_widths: nockvm::noun::Noun| {
            let f = fpoly(stack);
            let sam = T(
                stack,
                &[
                    f,
                    f, // trace_evaluations, comp_evaluations
                    trace_elems,
                    D(0), // comp_elems: ~
                    D(0), // num_comp_pieces
                    f,    // weights
                    heights,
                    full_widths,
                    D(1), // omega
                    D(0), // index
                    D(1), // deep_challenge
                    D(1), // new_comp_eval
                ],
            );
            let inner = T(stack, &[sam, D(0)]);
            T(stack, &[D(0), inner])
        };

        // A list element that is a cell instead of an atom: must be an
        // error (the serf thread resumes panics, killing the node).
        let context = &mut init_context();
        let stack = &mut context.stack;
        let cell_elem = T(stack, &[D(1), D(1)]);
        let bad_elems = T(stack, &[cell_elem, D(0)]);
        let subject = subject_for(stack, bad_elems, D(0), D(0));
        let result = catch_unwind(AssertUnwindSafe(|| evaluate_deep_jet(context, subject)));
        assert!(result.is_ok(), "cell element must not panic the jet");
        assert!(result.expect("no panic").is_err());

        // A trace window wider than the (empty) trace_elems list.
        let context = &mut init_context();
        let stack = &mut context.stack;
        let heights = T(stack, &[D(4), D(0)]);
        let full_widths = T(stack, &[D(48), D(0)]);
        let subject = subject_for(stack, D(0), heights, full_widths);
        let result = catch_unwind(AssertUnwindSafe(|| evaluate_deep_jet(context, subject)));
        assert!(result.is_ok(), "short trace_elems must not panic the jet");
        assert!(result.expect("no panic").is_err());

        // heights and full_widths naming different table counts.
        let context = &mut init_context();
        let stack = &mut context.stack;
        let heights = T(stack, &[D(4), D(0)]);
        let subject = subject_for(stack, D(0), heights, D(0));
        let result = catch_unwind(AssertUnwindSafe(|| evaluate_deep_jet(context, subject)));
        assert!(result.is_ok(), "mismatched table lists must not panic");
        assert!(result.expect("no panic").is_err());
    }
}
