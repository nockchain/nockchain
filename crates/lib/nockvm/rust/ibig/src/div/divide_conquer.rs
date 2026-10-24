//! Divide and conquer division algorithm.

use alloc::alloc::Layout;

use static_assertions::const_assert;

use crate::arch::word::{SignedWord, Word};
use crate::fast_divide::FastDivideNormalized;
use crate::memory::Memory;
use crate::sign::Sign::*;
use crate::{add, div, mul};

/// Memory requirement for division.
pub(crate) fn memory_requirement_exact(lhs_len: usize, rhs_len: usize) -> Layout {
    assert!(lhs_len >= rhs_len);
    // We need space for multiplications summing up to rhs.len(),
    // and at most lhs_len - rhs_len long.
    // One of the factors will be at most floor(rhs.len()/2),
    // and one of the factors will be at most lhs_len - rhs_len long.
    let smaller_len = (rhs_len / 2).min(lhs_len - rhs_len);
    mul::memory_requirement_up_to(rhs_len, smaller_len)
}

/// Division in place using divide and conquer.
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
    memory: &mut Memory,
) -> bool {
    assert!(lhs.len() > rhs.len() + div::MAX_LEN_SIMPLE && rhs.len() > div::MAX_LEN_SIMPLE);

    let mut overflow = false;
    let n = rhs.len();
    let mut m = lhs.len();
    assert!(n > div::MAX_LEN_SIMPLE && m >= n);
    while m >= 2 * n {
        let o = div_rem_in_place_same_len(&mut lhs[m - 2 * n..m], rhs, fast_div_rhs_top, memory);
        if o {
            assert!(m == lhs.len());
            overflow = true;
        }
        m -= n;
    }
    if m > n {
        let o = div_rem_in_place_small_quotient(&mut lhs[..m], rhs, fast_div_rhs_top, memory);
        if o {
            assert!(m == lhs.len());
            overflow = true;
        }
    }
    overflow
}

/// Quotient length = divisor length.
#[must_use]
fn div_rem_in_place_same_len(
    lhs: &mut [Word],
    rhs: &[Word],
    fast_div_rhs_top: FastDivideNormalized,
    memory: &mut Memory,
) -> bool {
    let n = rhs.len();
    assert!(n > div::MAX_LEN_SIMPLE && lhs.len() == 2 * n);
    // To guarantee n_lo >= 2.
    const_assert!(div::MAX_LEN_SIMPLE >= 3);
    let n_lo = n / 2;

    // Divide lhs[n_lo..] by rhs, putting quotient in lhs[n+n_lo..] and remainder in lhs[n_lo..n+n_lo].
    let overflow = div_rem_in_place_small_quotient(&mut lhs[n_lo..], rhs, fast_div_rhs_top, memory);

    // Divide lhs[..n+n_lo] by rhs, putting the rest of the quotient in lhs[n..n+n_lo] and remainder
    // in lhs[..n].
    let overflow_lo =
        div_rem_in_place_small_quotient(&mut lhs[..n + n_lo], rhs, fast_div_rhs_top, memory);
    assert!(!overflow_lo);

    overflow
}

/// Division in place using divide and conquer.
/// Quotient length < divisor length.
///
/// Divide lhs by rhs, replacing the top words of lhs by the quotient and the
/// bottom words of lhs by the remainder.
///
/// lhs = [lhs / rhs, lhs % rhs]
///
/// Returns carry in the quotient. It is at most 1 because rhs is normalized.
#[must_use]
fn div_rem_in_place_small_quotient(
    lhs: &mut [Word],
    rhs: &[Word],
    fast_div_rhs_top: FastDivideNormalized,
    memory: &mut Memory,
) -> bool {
    let n = rhs.len();
    assert!(n >= 2 && lhs.len() >= n);
    let m = lhs.len() - n;
    assert!(m < n);
    if m <= div::MAX_LEN_SIMPLE {
        return div::simple::div_rem_in_place(lhs, rhs, fast_div_rhs_top);
    }
    // Use top m words of the divisor to get a quotient approximation. It may be too large by at most 2.
    // Quotient is in lhs[n..], remainder in lhs[..n].
    // This is 2m / m division.
    let mut q_overflow: SignedWord =
        div_rem_in_place_same_len(&mut lhs[n - m..], &rhs[n - m..], fast_div_rhs_top, memory)
            .into();
    let (rem, q) = lhs.split_at_mut(n);

    // Subtract q * (the rest of rhs) from rem.
    // The multiplication here is m words by * (n-m) words.
    let mut rem_overflow: SignedWord = mul::add_signed_mul(rem, Negative, q, &rhs[..n - m], memory);
    if q_overflow != 0 {
        rem_overflow -= SignedWord::from(add::sub_same_len_in_place(&mut rem[m..], &rhs[..n - m]));
    }

    // If the remainder overflowed, adjust q and rem.
    while rem_overflow < 0 {
        rem_overflow += SignedWord::from(add::add_same_len_in_place(rem, rhs));
        q_overflow -= SignedWord::from(add::sub_one_in_place(q));
    }

    assert!(rem_overflow == 0 && (0..=1).contains(&q_overflow));
    q_overflow != 0
}
