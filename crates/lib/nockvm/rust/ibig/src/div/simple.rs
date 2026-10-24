//! Simple division algorithm.

use core::cmp::Ordering;

use crate::arch::word::Word;
use crate::fast_divide::FastDivideNormalized;
use crate::primitive::{double_word, extend_word};
use crate::{add, cmp, mul};

/// Division in place using the simple algorithm.
///
/// Divide lhs by rhs, replacing the top words of lhs by the quotient and the
/// bottom words of lhs by the remainder.
///
/// lhs = [lhs / rhs, lhs % rhs]
///
/// Returns carry in the quotient. It is at most 1 because rhs is normalized.
#[must_use]
pub(crate) fn div_rem_in_place(
    lhs: &mut [Word],
    rhs: &[Word],
    fast_div_rhs_top: FastDivideNormalized,
) -> bool {
    // The Art of Computer Programming, algorithm 4.3.1D.

    let n = rhs.len();
    assert!(n >= 2);
    let rhs0 = rhs[n - 1];
    let rhs1 = rhs[n - 2];

    let mut lhs_len = lhs.len();
    assert!(lhs_len >= n);

    let quotient_carry = cmp::cmp_same_len(&lhs[lhs_len - n..], rhs) >= Ordering::Equal;
    if quotient_carry {
        let overflow = add::sub_same_len_in_place(&mut lhs[lhs_len - n..], rhs);
        assert!(!overflow);
    }

    while lhs_len > n {
        let lhs0 = lhs[lhs_len - 1];
        let lhs1 = lhs[lhs_len - 2];
        let lhs2 = lhs[lhs_len - 3];
        let lhs01 = double_word(lhs1, lhs0);

        // Approximate the next word of quotient by
        // q = floor([lhs0, lhs1] / rhs0)
        // r = remainder
        // q may be too large, but never too small
        //
        // Then improve the approximation by adding an extra word.
        // q' = floor([lhs0, lhs1, lhs2] / [rhs0, rhs1])
        // Most of the time q' will be exact, but may be 1 too large.
        //
        // q must be decreased if subtracting q * rhs1 from [r, lhs2] overflows,
        // i.e. if q * rhs1 > [r, lhs2].
        //
        // This can happen at most twice, because r will certainly overflow after
        // adding rhs0 twice.
        let mut q = if lhs0 < rhs0 {
            let (mut q, mut r) = fast_div_rhs_top.div_rem(lhs01);
            while extend_word(q) * extend_word(rhs1) > double_word(lhs2, r) {
                q -= 1;
                match r.checked_add(rhs0) {
                    None => break,
                    Some(r2) => r = r2,
                }
            }
            q
        } else {
            // In this case MAX is accurate (r is already overflown).
            Word::MAX
        };

        // Subtract a multiple of rhs.
        let mut borrow =
            mul::sub_mul_word_same_len_in_place(&mut lhs[lhs_len - 1 - n..lhs_len - 1], q, rhs);

        if borrow > lhs0 {
            // Rare case: q is too large (by 1).
            // Add a correction.
            q -= 1;
            let carry = add::add_same_len_in_place(&mut lhs[lhs_len - 1 - n..lhs_len - 1], rhs);
            debug_assert!(carry);
            borrow -= 1;
        }
        debug_assert!(borrow == lhs0);
        // lhs0 is now logically zeroed out
        lhs_len -= 1;
        // Store next digit of quotient.
        lhs[lhs_len] = q;
    }
    // Quotient is now in lhs[n..] and remainder in lhs[..n].
    quotient_carry
}
