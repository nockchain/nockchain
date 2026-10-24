use std::cmp::max;
use std::vec;

use nockvm::noun::NounSpace;
use noun_serde::NounDecode;
use num_traits::MulAdd;

use crate::belt::{Belt, FieldError};
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
    let n = fp.len();

    if n == 1 {
        return vec![fp[0]];
    }

    debug_assert!(n.is_power_of_two());

    let log_2_of_n = n.ilog2();
    let mut x: Vec<Felt> = fp.into();

    for k in 0..n {
        let rk = bitreverse(k as u32, log_2_of_n);
        if k < rk as usize {
            x.swap(rk as usize, k);
        }
    }

    let mut m = 1;
    for _ in 0..log_2_of_n {
        let mut w_m = Felt::zero();
        fpow(root, (n / (2 * m)) as u64, &mut w_m);

        let mut k = 0;
        while k < n {
            let mut w = Felt::one();

            for j in 0..m {
                let u = x[k + j];
                let v = x[k + j + m] * w;
                x[k + j] = u + v;
                x[k + j + m] = u - v;
                w = w * w_m;
            }

            k += 2 * m;
        }

        m *= 2;
    }
    x
}

#[inline(always)]
pub fn fp_fft(fp: &[Felt]) -> Result<Vec<Felt>, FieldError> {
    let order = fp.len() as u64;
    let root = &Felt::ordered_root(order)?;
    Ok(fp_ntt(fp, root))
}

#[inline(always)]
pub fn fp_ifft(fp: &[Felt]) -> Result<Vec<Felt>, FieldError> {
    let order = fp.len() as u64;
    let ordered_root = Belt(order).ordered_root()?;
    let root = &Felt::constant(ordered_root.inv().0);
    let scale_factor = &mut Felt::from([Belt(order).inv(), Belt(0), Belt(0)]);

    let ntt_result = fp_ntt(fp, root);
    let mut scal_res = vec![Felt::zero(); order as usize];

    fpscal(scale_factor, &ntt_result, &mut scal_res);
    Ok(scal_res)
}

#[inline(always)]
pub fn fp_coseword(fp: &[Felt], offset: &Felt, order: u32, root: &Felt) -> Vec<Felt> {
    // shift
    let len_res: u32 = order;
    let mut res = vec![Felt::zero(); len_res as usize];
    fp_shift(fp, offset, &mut res);

    fp_ntt(&res, root)
}

#[inline(always)]
pub fn fp_intercosate(offset: &Felt, order: u32, values: &[Felt]) -> Result<Vec<Felt>, FieldError> {
    let len = values.len();
    if order == 0 || !order.is_power_of_two() || len != order as usize {
        return Err(FieldError::OrderedRootError);
    }

    let mut res = vec![Felt::zero(); len];
    let ifft_res = fp_ifft(values)?;
    let mut finv_res = Felt::zero();

    finv(offset, &mut finv_res);

    fp_shift(&ifft_res, &finv_res, &mut res);

    Ok(res)
}

#[inline(always)]
pub fn fpmul_fast(a: &[Felt], b: &[Felt], res: &mut [Felt]) {
    if res.is_empty() {
        return;
    }

    let mut new_a = a.to_vec();
    let mut new_b = b.to_vec();

    let res_len = res.len();
    let padded_len = res_len.next_power_of_two();

    new_a.resize(padded_len, Felt::zero());
    new_b.resize(padded_len, Felt::zero());

    let res_fpoly_a = fp_fft(&new_a).expect("fp_fft failed");
    let res_fpoly_b = fp_fft(&new_b).expect("fp_fft failed");

    let mut res_mul = vec![Felt::zero(); padded_len];
    res_mul
        .iter_mut()
        .zip(res_fpoly_a.iter())
        .zip(res_fpoly_b.iter())
        .for_each(|((res_vec, a_vec), b_vec)| {
            fmul(a_vec, b_vec, res_vec);
        });

    let ifft_result = fp_ifft(res_mul.as_slice()).expect("fp_ifft failed");
    res.copy_from_slice(&ifft_result[0..res.len()]);
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
pub fn lift_to_fpoly(belts: HoonList<'_>, res: &mut [Felt], space: &NounSpace) {
    for (i, b) in belts.into_iter().enumerate() {
        let belt = Belt::from_noun(&b, space).unwrap_or_else(|err| {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn felt(i: u64) -> Felt {
        Felt::from([Belt(i), Belt(i + 1), Belt(i + 2)])
    }

    #[test]
    fn fp_intercosate_rejects_order_mismatch_without_panicking() {
        let values = [felt(1), felt(4)];
        assert!(matches!(
            fp_intercosate(&Felt::one(), 4, &values),
            Err(FieldError::OrderedRootError)
        ));
    }

    #[test]
    fn fpmul_fast_matches_naive_for_large_inputs() {
        let a = (0..40).map(felt).collect::<Vec<_>>();
        let b = (40..90).map(felt).collect::<Vec<_>>();
        let mut naive = vec![Felt::zero(); a.len() + b.len() - 1];
        let mut fast = vec![Felt::zero(); naive.len()];

        fpmul(&a, &b, &mut naive);
        fpmul_fast(&a, &b, &mut fast);

        assert_eq!(fast, naive);
    }
}
