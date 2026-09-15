use nockvm::ext::AtomExt;
use nockvm::noun::{Atom, Noun, NounAllocator, NounSpace, D, T};
use num_bigint::BigUint;

use crate::errors::Result;
use crate::native::noun::{noun_eq_direct, noun_pair};

pub fn cons<A: NounAllocator>(allocator: &mut A, head: Noun, tail: Noun) -> Result<Noun> {
    let space = allocator.noun_space();
    let head_const = const_formula_value(head, &space)?;
    let tail_const = const_formula_value(tail, &space)?;
    if let (Some(h), Some(t)) = (head_const, tail_const) {
        return Ok(T(allocator, &[D(1), h, t]));
    }
    Ok(T(allocator, &[head, tail]))
}

pub fn comb<A: NounAllocator>(allocator: &mut A, mal: Noun, buz: Noun) -> Result<Noun> {
    let space = allocator.noun_space();
    // Match hoon-138 ++comb check order exactly.
    //
    // Check 1: mal = [0 a] where a != 0
    if let Some(a) = axis_formula_value(mal, &space)? {
        if a.bits() > 0 {
            // Check 1a: buz = [0 b] where b != 0 → [0 peg(a, b)]
            if let Some(b) = axis_formula_value(buz, &space)? {
                if b.bits() > 0 {
                    let composed = nock_peg(&a, &b);
                    return Ok(slot_formula_axis(allocator, &composed));
                }
            }
            // Check 1b: buz = [2 [0 x] [0 y]] → [2 [0 peg(a,x)] [0 peg(a,y)]]
            if let Ok((buz_op, buz_rest)) = noun_pair(buz, &space) {
                if noun_eq_direct(buz_op, 2, &space) {
                    if let Ok((buz_p, buz_q)) = noun_pair(buz_rest, &space) {
                        if let (Some(x), Some(y)) = (
                            axis_formula_value(buz_p, &space)?,
                            axis_formula_value(buz_q, &space)?,
                        ) {
                            if x.bits() > 0 && y.bits() > 0 {
                                let px = nock_peg(&a, &x);
                                let py = nock_peg(&a, &y);
                                let left = slot_formula_axis(allocator, &px);
                                let right = slot_formula_axis(allocator, &py);
                                return Ok(T(allocator, &[D(2), left, right]));
                            }
                        }
                    }
                }
            }
            // Check 1 fallthrough
            return Ok(T(allocator, &[D(7), mal, buz]));
        }
    }
    // Check 2: mal = [x [0 1]] → [8 x buz]
    if let Ok((mal_head, mal_tail)) = noun_pair(mal, &space) {
        if is_axis_one_formula(mal_tail, &space)? {
            return Ok(T(allocator, &[D(8), mal_head, buz]));
        }
    }
    // Check 3: buz = [0 1] → mal
    if is_axis_one_formula(buz, &space)? {
        return Ok(mal);
    }
    Ok(T(allocator, &[D(7), mal, buz]))
}

/// Compute the nock axis composition: navigate to axis `a`, then axis `b` within.
/// Equivalent to hoon-138 `++peg`.
fn nock_peg(a: &BigUint, b: &BigUint) -> BigUint {
    // a has some path bits; b has some path bits; concatenate them.
    // a = 1_ppp (binary), b = 1_qqq (binary)
    // result = 1_ppp_qqq = a * 2^(width of b's path) + (b - 2^(width of b's path))
    let b_path_width = b.bits() - 1;
    let b_prefix = BigUint::from(1u8) << b_path_width;
    (a << b_path_width) + (b - b_prefix)
}

fn slot_formula_axis<A: NounAllocator>(allocator: &mut A, axis: &BigUint) -> Noun {
    let axis = Atom::from_bytes(allocator, &axis.to_bytes_le()).as_noun();
    T(allocator, &[D(0), axis])
}

fn axis_formula_value(formula: Noun, space: &NounSpace) -> Result<Option<BigUint>> {
    let Ok((head, tail)) = noun_pair(formula, space) else {
        return Ok(None);
    };
    if !noun_eq_direct(head, 0, space) {
        return Ok(None);
    }
    let Ok(axis) = tail.in_space(space).as_atom() else {
        return Ok(None);
    };
    Ok(Some(BigUint::from_bytes_le(axis.as_ne_bytes())))
}

pub fn cond<A: NounAllocator>(allocator: &mut A, pex: Noun, yom: Noun, woq: Noun) -> Result<Noun> {
    let space = allocator.noun_space();
    if is_const_bool(pex, true, &space)? {
        return Ok(yom);
    }
    if is_const_bool(pex, false, &space)? {
        return Ok(woq);
    }
    if is_axis_zero_formula(pex, &space)? {
        return Ok(pex);
    }
    Ok(T(allocator, &[D(6), pex, yom, woq]))
}

fn const_formula_value(formula: Noun, space: &NounSpace) -> Result<Option<Noun>> {
    let Ok((head, tail)) = noun_pair(formula, space) else {
        return Ok(None);
    };
    if noun_eq_direct(head, 1, space) {
        return Ok(Some(tail));
    }
    Ok(None)
}

fn is_const_bool(formula: Noun, value: bool, space: &NounSpace) -> Result<bool> {
    let Some(constant) = const_formula_value(formula, space)? else {
        return Ok(false);
    };
    let expected = if value { 0 } else { 1 };
    Ok(noun_eq_direct(constant, expected, space))
}

fn is_axis_zero_formula(formula: Noun, space: &NounSpace) -> Result<bool> {
    let Ok((head, tail)) = noun_pair(formula, space) else {
        return Ok(false);
    };
    Ok(noun_eq_direct(head, 0, space) && noun_eq_direct(tail, 0, space))
}

fn is_axis_one_formula(formula: Noun, space: &NounSpace) -> Result<bool> {
    let Ok((head, tail)) = noun_pair(formula, space) else {
        return Ok(false);
    };
    Ok(noun_eq_direct(head, 0, space) && noun_eq_direct(tail, 1, space))
}
