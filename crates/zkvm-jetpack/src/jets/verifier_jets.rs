use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::mem::NockStack;
use nockvm::noun::{Atom, Cell, IndirectAtom, Noun};
use nockvm_macros::tas;
use tracing::debug;

use crate::form::belt::{bpow, Belt};
use crate::form::felt::*;
use crate::form::handle::new_handle_mut_felt;
use crate::form::noun_ext::{AtomMathExt, NounMathExt};
use crate::form::poly::{BPolySlice, Element, FPolySlice, Poly, PolySlice};
use crate::form::structs::{HoonList, HoonMapIter};
use crate::jets::proof_gen_jets::{MPUltra, ProofMap};
pub struct IndexFeltMap(pub ProofMap<usize, Felt>);
pub struct IndexBeltMap(pub ProofMap<usize, Belt>);

impl TryFrom<Noun> for IndexFeltMap {
    type Error = JetErr;

    fn try_from(hoon_map: Noun) -> Result<Self, Self::Error> {
        let hoon_map = HoonMapIter::from(hoon_map);
        let mut map = ProofMap::<usize, Felt>::new();

        for term_noun in hoon_map.into_iter() {
            let (k, v): (usize, Felt) = {
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
                    *term_cell.tail().as_atom()?.as_felt()?,
                )
            };

            map.insert(k, v);
        }
        Ok(IndexFeltMap(map))
    }
}

impl TryFrom<Noun> for IndexBeltMap {
    type Error = JetErr;

    fn try_from(hoon_map: Noun) -> Result<Self, Self::Error> {
        let hoon_map = HoonMapIter::from(hoon_map);
        let mut map = ProofMap::<usize, Belt>::new();

        for term_noun in hoon_map.into_iter() {
            let (k, v): (usize, Belt) = {
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
                    term_cell.tail().as_atom()?.as_belt()?,
                )
            };

            map.insert(k, v);
        }
        Ok(IndexBeltMap(map))
    }
}

pub fn evaluate_deep_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let sam = slot(subject, 6)?;
    let mut sam_cur: Cell = sam.as_cell()?;

    // Extract all parameters from the subject
    let trace_evaluations = sam_cur.head();
    sam_cur = sam_cur.tail().as_cell()?;
    let comp_evaluations = sam_cur.head();
    sam_cur = sam_cur.tail().as_cell()?;
    let trace_elems = sam_cur.head();
    sam_cur = sam_cur.tail().as_cell()?;
    let comp_elems = sam_cur.head();
    sam_cur = sam_cur.tail().as_cell()?;
    let num_comp_pieces = sam_cur.head();
    sam_cur = sam_cur.tail().as_cell()?;
    let weights = sam_cur.head();
    sam_cur = sam_cur.tail().as_cell()?;
    let heights = sam_cur.head();
    sam_cur = sam_cur.tail().as_cell()?;
    let full_widths = sam_cur.head();
    sam_cur = sam_cur.tail().as_cell()?;
    let omega = sam_cur.head();
    sam_cur = sam_cur.tail().as_cell()?;
    let index = sam_cur.head();
    sam_cur = sam_cur.tail().as_cell()?;
    let deep_challenge = sam_cur.head();
    let new_comp_eval = sam_cur.tail();

    // Convert nouns to appropriate types
    let Ok(trace_evaluations) = FPolySlice::try_from(trace_evaluations) else {
        debug!("trace_evaluations is not a valid FPolySlice");
        return Err(BAIL_FAIL);
    };
    let Ok(comp_evaluations) = FPolySlice::try_from(comp_evaluations) else {
        debug!("comp_evaluations is not a valid FPolySlice");
        return Err(BAIL_FAIL);
    };
    let trace_elems: Vec<Belt> = HoonList::try_from(trace_elems)?
        .into_iter()
        .map(|x| x.as_atom().unwrap().as_u64().unwrap())
        .map(Belt)
        .collect();
    let comp_elems: Vec<Belt> = HoonList::try_from(comp_elems)?
        .into_iter()
        .map(|x| x.as_atom().unwrap().as_u64().unwrap())
        .map(Belt)
        .collect();
    let num_comp_pieces = num_comp_pieces.as_atom()?.as_u64()?;
    let Ok(weights) = FPolySlice::try_from(weights) else {
        debug!("weights is not a valid FPolySlice");
        return Err(BAIL_FAIL);
    };
    let heights: Vec<u64> = HoonList::try_from(heights)?
        .into_iter()
        .map(|x| x.as_atom().unwrap().as_u64().unwrap())
        .collect();
    let full_widths: Vec<u64> = HoonList::try_from(full_widths)?
        .into_iter()
        .map(|x| x.as_atom().unwrap().as_u64().unwrap())
        .collect();
    let omega = omega.as_felt()?;
    let index = index.as_atom()?.as_u64()?;
    let deep_challenge = deep_challenge.as_felt()?;
    let new_comp_eval = new_comp_eval.as_felt()?;

    //  TODO use g defined wherever it is
    let g = Felt::lift(Belt(7));
    let omega_pow = fmul_(&fpow_(omega, index as u64), &g);

    let mut acc = Felt::zero();
    let mut num = 0usize;
    let mut total_full_width = 0usize;

    for (i, &height) in heights.iter().enumerate() {
        let full_width = full_widths[i] as usize;
        let omicron = Felt::lift(Belt(height).ordered_root()?);

        let current_trace_elems = &trace_elems[total_full_width..(total_full_width + full_width)];

        // Process first row trace columns
        let denom = fsub_(&omega_pow, deep_challenge);
        (acc, num) = process_belt(
            current_trace_elems, trace_evaluations.0, weights.0, full_width, num, &denom, &acc,
        );

        // Process second row trace columns (shifted by omicron)
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

        // Process first row trace columns with new_comp_eval
        let denom = fsub_(&omega_pow, new_comp_eval);
        (acc, num) = process_belt(
            current_trace_elems, trace_evaluations.0, weights.0, full_width, num, &denom, &acc,
        );

        // Process second row trace columns with new_comp_eval (shifted by omicron)
        let denom = fsub_(&omega_pow, &fmul_(new_comp_eval, &omicron));
        (acc, num) = process_belt(
            current_trace_elems, trace_evaluations.0, weights.0, full_width, num, &denom, &acc,
        );

        total_full_width += full_width;
    }

    // Process composition elements
    let denom = fsub_(&omega_pow, &fpow_(deep_challenge, num_comp_pieces as u64));

    (acc, _) = process_belt(
        &comp_elems,
        comp_evaluations.0,
        &weights.0[num..],
        num_comp_pieces as usize,
        0,
        &denom,
        &acc,
    );

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
) -> (Felt, usize) {
    let mut acc = *acc_start;
    let mut num = start_num;

    for i in 0..width {
        let elem_val = Felt::lift(elems[i]);
        let eval_val = evals[num];
        let weight_val = weights[num];

        // (elem_val - eval_val) / denom * weight_val + acc
        let diff = fsub_(&elem_val, &eval_val);
        let term = fmul_(&fdiv_(&diff, denom), &weight_val);
        acc = fadd_(&acc, &term);

        num += 1;
    }

    (acc, num)
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
    fn from_noun(noun: Noun) -> Result<Self, JetErr>;
    // =/  pow-op   ?:(=(field %base) bpow fpow)
    fn pow(&self, exp: u64) -> Self;
    // =/  lift-op  ?:(=(field %base) |=(v=@ `@ux`v) lift)
    fn lift(v: Belt) -> Self;
}

impl Fops for Belt {
    fn to_noun(self, stack: &mut NockStack) -> Noun {
        Atom::new(stack, self.0).as_noun()
    }

    fn from_noun(noun: Noun) -> Result<Self, JetErr> {
        Ok(Belt(noun.as_atom()?.as_u64()?))
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

    fn from_noun(noun: Noun) -> Result<Self, JetErr> {
        if let Ok(r) = noun.as_felt() {
            Ok(*r)
        } else {
            return Err(BAIL_FAIL);
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
    let sam = slot(subject, 6)?;
    let stack = &mut context.stack;
    let [field, mp, args, chal_map, dyns, com_map] = sam.uncell()?;
    let chals = BPolySlice::try_from(chal_map)?.0;

    let Ok(dyns) = BPolySlice::try_from(dyns) else {
        return Err(BAIL_FAIL);
    };

    let ret = match field.as_direct()?.data() {
        tas!(b"ext") => {
            let Ok(com_map) = IndexFeltMap::try_from(com_map) else {
                return Err(BAIL_FAIL);
            };
            let args = FPolySlice::try_from(args)?.0;
            mpeval::<Felt>(mp, args, chals, dyns.0, Some(&com_map.0))?.to_noun(stack)
        }
        tas!(b"base") => {
            let Ok(com_map) = IndexBeltMap::try_from(com_map) else {
                return Err(BAIL_FAIL);
            };
            let args = BPolySlice::try_from(args)?.0;
            mpeval::<Belt>(mp, args, chals, dyns.0, Some(&com_map.0))?.to_noun(stack)
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
    //com_map: Noun,
) -> Result<F, JetErr>
where
    for<'a> PolySlice<'a, F>: TryFrom<Noun>,
{
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
    let mut mp = HoonMapIter::from(mp);

    mp.try_fold(F::zero(), |acc, n| {
        let [k, v] = n.uncell()?;

        let Ok(k) = BPolySlice::try_from(k) else {
            return Err(BAIL_FAIL);
        };
        let v = Belt::from_noun(v)?;
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
        let res = k
            // =/  ter  (~(snag bop k) i)
            .iter()
            .copied()
            // =/  [typ=mega-typ idx=@ exp=@ud]  (brek ter)
            .map(crate::form::brek)
            .map(|(typ, idx, exp)| {
                // ?-  typ
                match typ {
                    //     %var
                    crate::form::MegaTyp::Var => {
                        //   %+  pow-op
                        //     (~(snag aop-door args) idx)
                        //   exp
                        args[idx].pow(exp)
                    }
                    // ::
                    //     %rnd
                    crate::form::MegaTyp::Rnd => {
                        //   %+  pow-op
                        //     (lift-op (~(got by chal-map) idx))
                        //   exp
                        let v = chals[idx];
                        F::lift(v).pow(exp)
                    }
                    // ::
                    //     %dyn
                    crate::form::MegaTyp::Dyn => {
                        //   %+  pow-op
                        //     (lift-op (~(snag bop dyns) idx))
                        //   exp
                        F::lift(dyns[idx]).pow(exp)
                    }
                    // ::
                    //     %con
                    crate::form::MegaTyp::Con => {
                        //   init-one
                        F::one()
                    }
                    // ::
                    //     %com
                    crate::form::MegaTyp::Com => {
                        //   %+  pow-op
                        //     (~(got by com-map) idx)
                        //   exp
                        let v = com_map
                            .expect("Composition dependencies are empty")
                            .get(&{ idx })
                            .unwrap_or_else(|| {
                                panic!(
                                    "Panicked at {}:{} (git sha: {:?})",
                                    file!(),
                                    line!(),
                                    option_env!("GIT_SHA")
                                )
                            });
                        v.pow(exp)
                    }
                }
                // ==
            })
            // %+  mul-op  res
            .fold(F::one(), core::ops::Mul::mul);

        Ok(acc + (coeff * res))
    })
}

pub fn mpeval_ultra_felt(
    mp: &MPUltra,
    args: &[Felt],
    chals: &[Belt],
    dyns: &[Belt],
) -> Result<Vec<Felt>, JetErr> {
    match mp {
        MPUltra::Mega(mp_mega) => {
            let mut vec: Vec<Felt> = Vec::new();
            let eval = mpeval::<Felt>(*mp_mega, args, chals, dyns, None)?;
            vec.push(eval);
            Ok(vec)
        }
        MPUltra::Comp(mp_comp) => {
            let mut deps = ProofMap::new();
            for (i, dep) in mp_comp.dep.iter().enumerate() {
                let eval = mpeval::<Felt>(*dep, args, chals, dyns, None)?;
                deps.insert(i, eval);
            }

            mp_comp
                .com
                .iter()
                .map(|com| mpeval::<Felt>(*com, args, chals, dyns, Some(&deps)))
                .collect()
        }
    }
}
