//! Toom-Cook-3 multiplication algorithm.

use alloc::alloc::Layout;

use crate::arch::word::{SignedWord, Word};
use crate::memory::{self, Memory};
use crate::mul::{self, helpers};
use crate::sign::Sign::{self, *};
use crate::{add, div, math, shift};

// We must have:
// 2 * (n+2) <= n
// i * n3 + 2 <= (i+1) * n3
// 5 * n3 + 2 <= 2n
// where n3 = floor((n+2)/3)
//
// Verify:
// 2 * n3 <= 2/3 (n+2) = 1/3 (2n + 2) <= n if n >= 2
// i * n3 + 2 <= (i+1) * n3 if n3 >= 2
// 5 * n3 + 2 <= 5/3 (n+2) + 2 = 1/3 (5n + 16) <= 2n if n >= 16
// If n >= 16, then n3 >= (16+2)/3 = 6 >= 2
/// Minimum supported length of the factors.
pub(crate) const MIN_LEN: usize = 16;

/// Temporary memory required for multiplication.
///
/// n bounds the length of the smaller factor in words.
pub(crate) fn memory_requirement_up_to(n: usize) -> Layout {
    // In each level of recursion we use:
    // a_eval: n3 + 1
    // b_eval: n3 + 1
    // c_eval: 2 * (n3 + 1)
    // t1:     2 * (n3 + 1)
    // t2:     2 * (n3 + 1)
    // total: 8 * (n3 + 1)
    //
    // Prove by induction that f(n) <= 4n + 20(log_3 (n-2.5)).
    // Base case, f(3) >= 0, OK.
    // For n > 3:
    // f(n)  = 8*(ceil(n/3)+1) + f(ceil(n/3)+1)
    //      <= 8*(n+5)/3 + 4*(n+5)/3 + 20 log_3 ((n+5)/3-2.5)
    //       = 4n + 20 + 20 log_3 ((n+5)/3-2.5)
    //       = 4n + 20 log_3 (n-2.5)
    //
    // 20 log_3 (n-2.5) <= 20 log_3 n = 20 log_2 n / log_2 3 < 13 log_2 n
    // So we use 4n + 13 ceil log_2 n.
    //
    // Note: the recurence also works when we transition to Karatsuba, because
    // Karatsuba memory requirements are smaller.
    let num_words = 4 * n + 13 * (math::ceil_log_2(n) as usize);
    memory::array_layout::<Word>(num_words)
}

/// c += sign * a * b
/// Toom-Cook-3 method. O(a.len() * b.len()^0.47).
///
/// Returns carry.
#[must_use]
pub(crate) fn add_signed_mul(
    c: &mut [Word],
    sign: Sign,
    a: &[Word],
    b: &[Word],
    memory: &mut Memory,
) -> SignedWord {
    assert!(a.len() >= b.len() && b.len() >= MIN_LEN && c.len() == a.len() + b.len());

    helpers::add_signed_mul_split_into_chunks(
        c,
        sign,
        a,
        b,
        b.len(),
        memory,
        add_signed_mul_same_len,
    )
}

/// c += sign * a * b
/// Toom-Cook-3 method: O(n^1.47).
///
/// Returns carry.
#[must_use]
pub(crate) fn add_signed_mul_same_len(
    c: &mut [Word],
    sign: Sign,
    a: &[Word],
    b: &[Word],
    memory: &mut Memory,
) -> SignedWord {
    let n = a.len();
    debug_assert!(b.len() == n && c.len() == 2 * n);
    debug_assert!(n >= MIN_LEN);

    // Brent, Zimmermann, Modern Computer Arithmetic 0.5.9, Algorithm 1.4.
    //
    // We evaluate the polynomials A(x) = a0 + a1*x + a2*x^2, B(x) = b0 + b1*x + b2*x^2
    // at points 0, 1, -1, 2, infinity.
    // Multiplying, this gives us values of V(x) = A(x)*B(x) = c0 + c1*x + c2*x^2 + c3*x^3 + c4*x^4
    // at the same points (using 5 recursive multiplications).
    //
    // Then we interpolate the polynomial coefficients, which gives the following formulas:
    // c_0 = V(0)
    // c_1 = V(1) - t1
    // c_2 = t2 - V(0) - V(inf)
    // c_3 = t1 - t2
    // c_4 = V(inf)
    // where:
    // t1 = (3V(0) + 2V(-1) + V(2))/6 - 2V(inf)
    // t2 = (V(1) + V(-1))/2

    // Split into 3 parts. Note: a2, b2 may be shorter.
    let n3 = n.div_ceil(3);
    let n3_short = n - 2 * n3;

    let (a0, a12) = a.split_at(n3);
    let (a1, a2) = a12.split_at(n3);
    let (b0, b12) = b.split_at(n3);
    let (b1, b2) = b12.split_at(n3);

    let mut carry: SignedWord = 0;
    // Accumulate intermediate carries, we will add them at the end.
    let mut carry_c0: SignedWord = 0; // at 2*n3
    let mut carry_c1: SignedWord = 0; // at 3*n3+2
    let mut carry_c2: SignedWord = 0; // at 4*n3+2
    let mut carry_c3: SignedWord = 0; // at 5*n3+2

    // Evaluate at 0.
    // V(0) = a0 * b0
    // c_0 += V(0)
    // c_2 -= V(0)
    // t1 = 3*V(0)
    let (t1, mut memory) = memory.allocate_slice_fill(2 * n3 + 2, 0);
    {
        let t1_short = &mut t1[..2 * n3];
        let overflow = mul::add_signed_mul_same_len(t1_short, Positive, a0, b0, &mut memory);
        assert!(overflow == 0);
        carry_c0 += add::add_signed_same_len_in_place(&mut c[..2 * n3], sign, t1_short);
        carry_c2 += add::add_signed_in_place(&mut c[2 * n3..4 * n3 + 2], -sign, t1_short);
        t1[2 * n3] = mul::mul_word_in_place(t1_short, 3);
        t1[2 * n3 + 1] = 0;
    }

    // Evaluate at 2.
    // a_eval = a0 + 2a1 + 4a2
    // b_eval = b0 + 2b1 + 4b2
    // V(2) = a_eval * b_eval
    // t1 += V(2)
    let (a_eval, mut memory) = memory.allocate_slice_copy_fill(n3 + 1, a0, 0);
    let (b_eval, mut memory) = memory.allocate_slice_copy_fill(n3 + 1, b0, 0);
    {
        a_eval[n3] = mul::add_mul_word_same_len_in_place(&mut a_eval[..n3], 2, a1);
        a_eval[n3] += mul::add_mul_word_in_place(&mut a_eval[..n3], 4, a2);
        b_eval[n3] = mul::add_mul_word_same_len_in_place(&mut b_eval[..n3], 2, b1);
        b_eval[n3] += mul::add_mul_word_in_place(&mut b_eval[..n3], 4, b2);
        let overflow = mul::add_signed_mul_same_len(t1, Positive, a_eval, b_eval, &mut memory);
        assert!(overflow == 0);
    }

    // Evaluate at inf.
    // V(inf) = a4 * b4
    // c_2 -= V(inf)
    // c_4 += V(inf)
    // t1 -= 12V(inf)
    // Now t1 = 3V(0) + V(2) - 12V(inf)
    {
        let (c_eval, mut memory) = memory.allocate_slice_fill(2 * n3 + 2, 0);
        let c_eval_short = &mut c_eval[..2 * n3_short];
        let overflow = mul::add_signed_mul_same_len(c_eval_short, Positive, a2, b2, &mut memory);
        assert!(overflow == 0);
        carry_c2 += add::add_signed_in_place(&mut c[2 * n3..4 * n3 + 2], -sign, c_eval_short);
        carry += add::add_signed_same_len_in_place(&mut c[4 * n3..], sign, c_eval_short);
        c_eval[2 * n3_short] = mul::mul_word_in_place(c_eval_short, 12);
        let overflow = add::sub_in_place(t1, &c_eval[..2 * n3_short + 1]);
        // 3V(0) + V(2) - 12V(inf) is never negative
        assert!(!overflow);
    }

    // Sign of V(-1).
    let mut value_neg1_sign;
    let (t2, mut memory) = memory.allocate_slice_fill(2 * n3 + 2, 0);
    {
        // Evaluate at 1.
        // a_eval = a0 + a1 + a2
        // b_eval = b0 + b1 + b2
        // V(1) = a_eval * b_eval
        // c_1 += V(1)
        // t2 = V(1)
        // a02 = a0 + a2
        // b02 = b0 + b2
        // a02 and b02 take the same amount of space as c_eval.
        let (a02, mut memory) = memory.allocate_slice_copy_fill(n3 + 1, a0, 0);
        a02[n3] = Word::from(add::add_in_place(&mut a02[..n3], a2));
        a_eval.copy_from_slice(a02);
        a_eval[n3] += Word::from(add::add_same_len_in_place(&mut a_eval[..n3], a1));

        let (b02, mut memory) = memory.allocate_slice_copy_fill(n3 + 1, b0, 0);
        b02[n3] = Word::from(add::add_in_place(&mut b02[..n3], b2));
        b_eval.copy_from_slice(b02);
        b_eval[n3] += Word::from(add::add_same_len_in_place(&mut b_eval[..n3], b1));

        let overflow = mul::add_signed_mul_same_len(t2, Positive, a_eval, b_eval, &mut memory);
        assert!(overflow == 0);
        carry_c1 += add::add_signed_in_place(&mut c[n3..3 * n3 + 2], sign, t2);

        // Evaluate at -1.
        // a_eval = a02 - a1
        // b_eval = b02 - b1
        // V(-1) = a_eval * b_eval
        // t2 += V(-1)
        // t1 += 2*V(-1)
        // Now t1 = 3V(0) + 2V(-1) + V(2) - 12V(inf),
        //     t2 = V(1) + V(-1).
        a_eval.copy_from_slice(a02);
        value_neg1_sign = add::sub_in_place_with_sign(a_eval, a1);
        b_eval.copy_from_slice(b02);
        value_neg1_sign *= add::sub_in_place_with_sign(b_eval, b1);
        // We don't need a02, b02 any more, exit the block so that we can use c_eval again.
    }
    let (c_eval, mut memory) = memory.allocate_slice_fill(2 * (n3 + 1), 0);
    let overflow = mul::add_signed_mul_same_len(c_eval, Positive, a_eval, b_eval, &mut memory);
    assert!(overflow == 0);
    let overflow = add::add_signed_same_len_in_place(t2, value_neg1_sign, c_eval);
    assert!(overflow == 0);
    match value_neg1_sign {
        Positive => {
            let overflow = mul::add_mul_word_same_len_in_place(t1, 2, c_eval);
            assert!(overflow == 0);
        }
        Negative => {
            let overflow = mul::sub_mul_word_same_len_in_place(t1, 2, c_eval);
            assert!(overflow == 0);
        }
    }

    // t1 /= 6
    // t2 /= 2
    // Now t1 = (3V(0) + 2V(-1) + V(2))/6 - 2V(inf)
    //     t2 = (V(1) + V(-1))/2
    let t1_rem = div::div_by_word_in_place(t1, 6);
    let t2_rem = shift::shr_in_place(t2, 1);
    assert_eq!(t1_rem, 0);
    assert_eq!(t2_rem, 0);

    // c1 -= t1
    // c3 += t1
    // c2 += t2
    // c3 -= t2
    carry_c1 += add::add_signed_same_len_in_place(&mut c[n3..3 * n3 + 2], -sign, t1);
    carry_c3 += add::add_signed_same_len_in_place(&mut c[3 * n3..5 * n3 + 2], sign, t1);
    carry_c2 += add::add_signed_same_len_in_place(&mut c[2 * n3..4 * n3 + 2], sign, t2);
    carry_c3 += add::add_signed_same_len_in_place(&mut c[3 * n3..5 * n3 + 2], -sign, t2);

    // Apply carries.
    carry_c1 += add::add_signed_word_in_place(&mut c[2 * n3..3 * n3 + 2], carry_c0);
    carry_c2 += add::add_signed_word_in_place(&mut c[3 * n3 + 2..4 * n3 + 2], carry_c1);
    carry_c3 += add::add_signed_word_in_place(&mut c[4 * n3 + 2..5 * n3 + 2], carry_c2);
    carry += add::add_signed_word_in_place(&mut c[5 * n3 + 2..], carry_c3);

    assert!(carry.abs() <= 1);
    carry
}
