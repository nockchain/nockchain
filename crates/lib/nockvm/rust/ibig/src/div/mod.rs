//! Division functions.

use alloc::alloc::Layout;

use crate::arch::word::Word;
use crate::fast_divide::FastDivideNormalized;
use crate::memory::{self, Memory};
use crate::primitive::{double_word, extend_word};
use crate::shift;

mod divide_conquer;
mod simple;

/// If divisor or quotient is at most this length, use the simple division algorithm.
const MAX_LEN_SIMPLE: usize = 32;

/// Normalize a large divisor.
///
/// Returns (shift, fast division for the top word).
pub(crate) fn normalize_large(words: &mut [Word]) -> (u32, FastDivideNormalized) {
    assert!(words.len() >= 2);
    let shift = words
        .last()
        .unwrap_or_else(|| {
            panic!(
                "Panicked at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        })
        .leading_zeros();
    let overflow = shift::shl_in_place(words, shift);
    assert!(overflow == 0);
    (
        shift,
        FastDivideNormalized::new(*words.last().unwrap_or_else(|| {
            panic!(
                "Panicked at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        })),
    )
}

/// words = words / rhs
///
/// rhs must be non-zero
///
/// Returns words % rhs.
#[must_use]
pub(crate) fn div_by_word_in_place(words: &mut [Word], rhs: Word) -> Word {
    debug_assert!(rhs != 0);
    if words.is_empty() {
        return 0;
    }
    if rhs.is_power_of_two() {
        let sh = rhs.trailing_zeros();
        let rem = shift::shr_in_place(words, sh);
        return rem;
    }

    let fast_div_rhs = FastDivideNormalized::new(rhs << rhs.leading_zeros());
    fast_div_by_word_in_place(words, rhs, fast_div_rhs)
}

/// words = words / rhs
///
/// Returns words % rhs.
#[must_use]
pub(crate) fn fast_div_by_word_in_place(
    words: &mut [Word],
    rhs: Word,
    fast_div_rhs: FastDivideNormalized,
) -> Word {
    let shift = rhs.leading_zeros();
    let mut rem = shift::shl_in_place(words, shift);

    for word in words.iter_mut().rev() {
        let a = double_word(*word, rem);
        let (q, r) = fast_div_rhs.div_rem(a);
        *word = q;
        rem = r;
    }
    rem >> shift
}

/// words % rhs
pub(crate) fn rem_by_word(words: &[Word], rhs: Word) -> Word {
    debug_assert!(rhs != 0);
    if words.is_empty() {
        return 0;
    }
    if rhs.is_power_of_two() {
        return words[0] & (rhs - 1);
    }

    let shift = rhs.leading_zeros();
    let fast_div_rhs = FastDivideNormalized::new(rhs << shift);
    let rem = fast_rem_by_normalized_word(words, fast_div_rhs);
    let a = extend_word(rem) << shift;
    let (_, rem) = fast_div_rhs.div_rem(a);
    rem >> shift
}

/// words % rhs
pub(crate) fn fast_rem_by_normalized_word(
    words: &[Word],
    fast_div_rhs: FastDivideNormalized,
) -> Word {
    let mut iter = words.iter().rev();

    let mut rem: Word = 0;
    match iter.next() {
        None => return rem,
        Some(word) => rem = fast_div_rhs.div_rem_word(*word).1,
    }

    for word in iter {
        let a = double_word(*word, rem);
        let (_, r) = fast_div_rhs.div_rem(a);
        rem = r;
    }

    rem
}

/// Memory requirement for division.
pub(crate) fn memory_requirement_exact(lhs_len: usize, rhs_len: usize) -> Layout {
    assert!(lhs_len >= rhs_len && rhs_len >= 2);
    if rhs_len <= MAX_LEN_SIMPLE || lhs_len - rhs_len <= MAX_LEN_SIMPLE {
        memory::zero_layout()
    } else {
        divide_conquer::memory_requirement_exact(lhs_len, rhs_len)
    }
}

/// Divide lhs by rhs, replacing the top words of lhs by the quotient and the
/// bottom words of lhs by the remainder.
///
/// rhs must have at least 2 words and the top bit must be 1.
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
    assert!(lhs.len() >= rhs.len() && rhs.len() >= 2);

    if rhs.len() <= MAX_LEN_SIMPLE || lhs.len() - rhs.len() <= MAX_LEN_SIMPLE {
        simple::div_rem_in_place(lhs, rhs, fast_div_rhs_top)
    } else {
        divide_conquer::div_rem_in_place(lhs, rhs, fast_div_rhs_top, memory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_div_by_word_in_place_empty() {
        let mut a = [];
        let rem = div_by_word_in_place(&mut a, 7);
        assert_eq!(rem, 0);
    }

    #[test]
    fn test_rem_by_word_empty() {
        let a = [];
        let rem = rem_by_word(&a, 7);
        assert_eq!(rem, 0);
    }
}
