//! Karatsuba multiplication algorithm.

use alloc::alloc::Layout;

use crate::arch::word::{SignedWord, Word};
use crate::memory::{self, Memory};
use crate::mul::{self, helpers};
use crate::sign::Sign::{self, *};
use crate::{add, math};

// We must have 3 * floor((n+1)/2) <= 2n.
//
// If n >= 3 then:
// 6 * floor((n+1)/2) <= 3(n+1) = 3n + 3 <= 4n
/// Minimum supported length of the factors.
pub(crate) const MIN_LEN: usize = 3;

/// Temporary memory required for multiplication.
///
/// n bounds the length of the smaller factor in words.
pub(crate) fn memory_requirement_up_to(n: usize) -> Layout {
    // We prove by induction that:
    // f(n) <= 2n + 2 log_2 (n-1)
    //
    // Base case: f(2) >= 0.
    // For n > 2:
    // f(n) = 2ceil(n/2) + f(ceil(n/2)) - Const
    //      <= n+1 + n+1 + 2log ((n+1)/2-1) - Const
    //       = 2n + 2log (n-1) - Const
    //
    // Use 2n + 2 ceil log_2 n.
    let num_words = 2 * n + 2 * (math::ceil_log_2(n) as usize);
    memory::array_layout::<Word>(num_words)
}

/// c += sign * a * b
/// Karatsuba method: O(a.len() * b.len()^0.59).
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
    debug_assert!(a.len() >= b.len() && b.len() >= MIN_LEN && c.len() == a.len() + b.len());

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
/// Karatsuba method: O(n^1.59).
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

    let mid = n.div_ceil(2);

    let (a_lo, a_hi) = a.split_at(mid);
    let (b_lo, b_hi) = b.split_at(mid);
    // Result = a_lo * b_lo + a_hi * b_hi * Word^(2mid)
    //        + (a_lo * b_lo + a_hi * b_hi - (a_lo-a_hi)*(b_lo-b_hi)) * Word^mid
    let mut carry: SignedWord = 0;
    let mut carry_c0: SignedWord = 0; // 2*mid
    let mut carry_c1: SignedWord = 0; // 3*mid

    {
        // c_0 += a_lo * b_lo
        // c_1 += a_lo * b_lo
        let (c_lo, mut memory) = memory.allocate_slice_fill::<Word>(2 * mid, 0);
        let overflow = mul::add_signed_mul_same_len(c_lo, Positive, a_lo, b_lo, &mut memory);
        assert!(overflow == 0);
        carry_c0 += add::add_signed_same_len_in_place(&mut c[..2 * mid], sign, c_lo);
        carry_c1 += add::add_signed_same_len_in_place(&mut c[mid..3 * mid], sign, c_lo);
    }
    {
        // c_2 += a_hi * b_hi
        // c_1 += a_hi * b_hi
        let (c_hi, mut memory) = memory.allocate_slice_fill::<Word>(2 * (n - mid), 0);
        let overflow = mul::add_signed_mul_same_len(c_hi, Positive, a_hi, b_hi, &mut memory);
        assert!(overflow == 0);
        carry += add::add_signed_same_len_in_place(&mut c[2 * mid..], sign, c_hi);
        carry_c1 += add::add_signed_in_place(&mut c[mid..3 * mid], sign, c_hi);
    }
    {
        // c1 -= (a_lo - a_hi) * (b_lo - b_hi)
        let (a_diff, mut memory) = memory.allocate_slice_copy(a_lo);
        let mut diff_sign = add::sub_in_place_with_sign(a_diff, a_hi);
        let (b_diff, mut memory) = memory.allocate_slice_copy(b_lo);
        diff_sign *= add::sub_in_place_with_sign(b_diff, b_hi);

        carry_c1 += mul::add_signed_mul_same_len(
            &mut c[mid..3 * mid],
            -sign * diff_sign,
            a_diff,
            b_diff,
            &mut memory,
        );
    }

    // Propagate carries.
    carry_c1 += add::add_signed_word_in_place(&mut c[2 * mid..3 * mid], carry_c0);
    carry += add::add_signed_word_in_place(&mut c[3 * mid..], carry_c1);

    assert!(carry.abs() <= 1);
    carry
}
