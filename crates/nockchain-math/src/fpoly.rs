use std::cmp::max;
use std::vec;

use noun_serde::NounDecode;
use num_traits::MulAdd;

use crate::belt::Belt;
use crate::bpoly::bitreverse;
use crate::felt::*;
use crate::poly::*;
use crate::structs::HoonList;

#[inline(always)]
pub fn fpadd(a: &[Felt], b: &[Felt], res: &mut [Felt]) {
    let min: &[Felt];
    let max: &[Felt];
    if a.len() <= b.len() {
        min = a;
        max = b;
    } else {
        min = b;
        max = a;
    }

    for ((res_vec, max_vec), min_vec) in res
        .iter_mut()
        .zip(max.iter())
        .zip(min.iter().map(Some).chain(std::iter::repeat(None)))
    {
        if let Some(min_vec) = min_vec {
            fadd(min_vec, max_vec, res_vec);
        } else {
            res_vec.copy_from_slice(max_vec);
        }
    }
}

#[inline(always)]
pub fn fpadd_(left: &[Felt], right: &[Felt]) -> Vec<Felt> {
    let len = max(left.len(), right.len());
    let mut res = vec![Felt::zero(); len];
    fpadd(left, right, res.as_mut_slice());
    res
}

#[inline(always)]
pub fn fpsub(a: &[Felt], b: &[Felt], res: &mut [Felt]) {
    debug_assert!(a.len() >= b.len());
    let min: &[Felt] = b;
    let max: &[Felt] = a;

    for ((res_vec, max_vec), min_vec) in res
        .iter_mut()
        .zip(max.iter())
        .zip(min.iter().map(Some).chain(std::iter::repeat(None)))
    {
        if let Some(min_vec) = min_vec {
            fsub(max_vec, min_vec, res_vec);
        } else {
            res_vec.copy_from_slice(max_vec);
        }
    }
}

#[inline(always)]
pub fn fpsub_in_place(a: &mut [Felt], b: &[Felt]) {
    debug_assert!(a.len() >= b.len());
    for (max_vec, min_vec) in a
        .iter_mut()
        .zip(b.iter().map(Some).chain(std::iter::repeat(None)))
    {
        if let Some(min_vec) = min_vec {
            *max_vec = *max_vec - *min_vec;
        } else {
            break;
        }
    }
}

#[inline(always)]
pub fn fpsub_(left: &[Felt], right: &[Felt]) -> Vec<Felt> {
    let len = max(left.len(), right.len());
    let mut res = vec![Felt::zero(); len];
    fpsub(left, right, res.as_mut_slice());

    //  TODO: hoon impl does not normalize here, but maybe it should?
    //normalize_poly(&mut res);
    res
}

#[inline(always)]
pub fn fpmul(a: &[Felt], b: &[Felt], res: &mut [Felt]) {
    let a_len = a.len();
    let b_len = b.len();
    for i in 0..a_len {
        if a[i].is_zero() {
            continue;
        }

        for j in 0..b_len {
            let mut result_felt: Felt = Felt::zero();
            let mut fmul_result: Felt = Felt::zero();

            fmul(&a[i], &b[j], &mut fmul_result);

            fadd(&res[i + j], &fmul_result, &mut result_felt);

            res[i + j] = result_felt;
        }
    }
}

#[allow(dead_code)]
#[inline(always)]
fn fpmul_(left: &[Felt], right: &[Felt]) -> Vec<Felt> {
    let len = left.len() + right.len() - 1;
    let mut res = vec![Felt::zero(); len];
    fpmul(left, right, res.as_mut_slice());
    res
}

pub fn fpdiv(a: &[Felt], b: &[Felt], res: &mut [Felt]) {
    let a_head_felt: &Felt = a.leading_coeff();
    let b_head_felt: &Felt = b.leading_coeff();

    // Calculate factor to be used rescale quotient.
    let lead = *a_head_felt / *b_head_felt;

    let mut a_inv: Felt = Felt::zero();
    let mut b_inv: Felt = Felt::zero();

    // Calculate inverses
    finv(a_head_felt, &mut a_inv);
    finv(b_head_felt, &mut b_inv);

    // Make poly monic
    let mut a_monic = fpscal_(&a_inv, a);
    let mut b_monic = fpscal_(&b_inv, b);

    // Get leading coefficient of divisor and take its inverse
    let mut divisor_leading_inv = Felt::zero();
    finv(b_monic.leading_coeff(), &mut divisor_leading_inv);

    // Obtain rev(a) and rev(b)
    a_monic.reverse();
    b_monic.reverse();

    let mut remainder = a_monic.clone();

    if a.degree() < b.degree() {
        res.fill(Felt::zero());
        return;
    }

    for i in 0..res.len() {
        let x = remainder[i] * divisor_leading_inv;
        res[i] = x;
        let scal_res = fpscal_(&x, &b_monic);
        fpsub_in_place(&mut remainder[i..], &scal_res);
    }
    res.reverse();

    let res_cpy = res.to_vec();
    fpscal(&lead, &res_cpy, res);
}

pub fn fpdiv_(left: &[Felt], right: &[Felt]) -> Vec<Felt> {
    let len = if left.len() < right.len() {
        1
    } else {
        left.len() - right.len() + 1
    };

    let mut res = vec![Felt::zero(); len];
    fpdiv(left, right, res.as_mut_slice());
    res
}

#[inline(always)]
pub fn fpscal(c: &Felt, fp: &[Felt], res: &mut [Felt]) {
    if fp.is_zero() {
        res.fill(Felt::zero());
        return;
    }

    for (res_vec, fp_vec) in res.iter_mut().zip(fp.iter()) {
        fmul(c, fp_vec, res_vec);
    }
}

#[allow(dead_code)]
#[inline(always)]
pub fn fpscal_(left: &Felt, right: &[Felt]) -> Vec<Felt> {
    let len = right.len();
    let mut res = vec![Felt::zero(); len];
    fpscal(left, right, res.as_mut_slice());
    res
}

#[inline(always)]
pub fn bpoly_to_fpoly(bpoly: &[Belt], res: &mut [Felt]) {
    for (i, b) in bpoly.iter().enumerate() {
        res[i] = Felt::lift(*b);
    }
}

#[inline(always)]
pub fn fp_shift(poly_a: &[Felt], felt_b: &Felt, poly_res: &mut [Felt]) {
    let mut felt_power: Felt = Felt::from([1, 0, 0]);

    for i in 0..poly_a.len() {
        let res_felt: &mut Felt = &mut Felt::from([0, 0, 0]);
        fmul(&poly_a[i], &felt_power, res_felt);
        poly_res[i] = *res_felt;

        fmul(&felt_power.clone(), felt_b, &mut felt_power);
    }
}

pub fn fp_ntt(fp: &[Felt], root: &Felt) -> Vec<Felt> {
    let n = fp.len() as u32;

    if n == 1 {
        return vec![fp[0]];
    }

    debug_assert!(n.is_power_of_two());

    let log_2_of_n = n.ilog2();

    const FELT0: Felt = Felt([Belt(0), Belt(0), Belt(0)]);
    const FELT1: Felt = Felt([Belt(1), Belt(0), Belt(0)]);

    let mut x: Vec<Felt> = vec![FELT0; n as usize];
    x.copy_from_slice(fp);

    for k in 0..n {
        let rk = bitreverse(k, log_2_of_n);
        if k < rk {
            x.swap(rk as usize, k as usize);
        }
    }

    let mut m = 1;
    for _ in 0..log_2_of_n {
        let mut w_m: Felt = Default::default();
        fpow(root, (n / (2 * m)) as u64, &mut w_m);

        let mut k = 0;
        while k < n {
            let mut w = FELT1;

            for j in 0..m {
                let u: Felt = x[(k + j) as usize];
                let v: Felt = x[(k + j + m) as usize] * w;
                x[(k + j) as usize] = u + v;
                x[(k + j + m) as usize] = u - v;
                w = w * w_m;
            }

            k += 2 * m;
        }

        m *= 2;
    }
    x
}

#[inline(always)]
pub fn fp_coseword(fp: &[Felt], offset: &Felt, order: u32, root: &Felt) -> Vec<Felt> {
    // shift
    let len_res: u32 = order;
    let mut res = vec![Felt::zero(); len_res as usize];
    fp_shift(fp, offset, &mut res);

    fp_ntt(&res, root)
}

// MIT License
// Copyright (c) 2023 Andrew J. Radcliffe <andrewjradcliffe@gmail.com>
pub fn horner_loop<T>(x: T, coefficients: &[T]) -> T
where
    T: Copy + MulAdd + MulAdd<Output = T>,
{
    let n = coefficients.len();
    if n > 0 {
        let a_n = coefficients[n - 1];
        coefficients[0..n - 1]
            .iter()
            .rfold(a_n, |result, &a| result.mul_add(x, a))
    } else {
        panic!(
            "coefficients.len() must be greater than or equal to 1, got {}",
            n
        );
    }
}

// fpoly and felt ranks are lowest to highest
pub fn fpeval(a: &[Felt], x: Felt) -> Felt {
    horner_loop(x, a)
}

#[inline(always)]
pub fn lift_to_fpoly(belts: HoonList, res: &mut [Felt]) {
    for (i, b) in belts.into_iter().enumerate() {
        let belt = Belt::from_noun(&b).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        res[i] = Felt::lift(belt);
    }
}
