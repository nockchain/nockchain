//! Addition and subtraction functions.

use core::cmp::Ordering::*;

use crate::arch::word::{SignedWord, Word};
use crate::arch::{self};
use crate::primitive::PrimitiveSigned;
use crate::sign::Sign::{self, *};

/// Add one to a word sequence.
///
/// Returns overflow.
#[must_use]
pub(crate) fn add_one_in_place(words: &mut [Word]) -> bool {
    for word in words {
        let (a, overflow) = word.overflowing_add(1);
        *word = a;
        if !overflow {
            return false;
        }
    }
    true
}

/// Subtract one from a word sequence.
///
/// Returns borrow.
#[must_use]
pub(crate) fn sub_one_in_place(words: &mut [Word]) -> bool {
    for word in words {
        let (a, borrow) = word.overflowing_sub(1);
        *word = a;
        if !borrow {
            return false;
        }
    }
    true
}

/// Add a word to a non-empty word sequence.
///
/// Returns overflow.
#[must_use]
pub(crate) fn add_word_in_place(words: &mut [Word], rhs: Word) -> bool {
    let (word_0, words_hi) = words.split_first_mut().unwrap_or_else(|| {
        panic!(
            "Panicked at {}:{} (git sha: {})",
            file!(),
            line!(),
            option_env!("GIT_SHA").unwrap_or("unknown")
        )
    });
    let (a, overflow) = word_0.overflowing_add(rhs);
    *word_0 = a;
    overflow && add_one_in_place(words_hi)
}

/// Subtract a word from a non-empty word sequence.
///
/// Returns borrow.
#[must_use]
pub(crate) fn sub_word_in_place(words: &mut [Word], rhs: Word) -> bool {
    let (word_0, words_hi) = words.split_first_mut().unwrap_or_else(|| {
        panic!(
            "Panicked at {}:{} (git sha: {})",
            file!(),
            line!(),
            option_env!("GIT_SHA").unwrap_or("unknown")
        )
    });
    let (a, borrow) = word_0.overflowing_sub(rhs);
    *word_0 = a;
    borrow && sub_one_in_place(words_hi)
}

/// Add a word sequence of same length in place.
///
/// Returns overflow.
#[must_use]
pub(crate) fn add_same_len_in_place(words: &mut [Word], rhs: &[Word]) -> bool {
    debug_assert!(words.len() == rhs.len());

    let mut carry = false;
    for (a, b) in words.iter_mut().zip(rhs.iter()) {
        let (sum, c) = arch::add::add_with_carry(*a, *b, carry);
        *a = sum;
        carry = c;
    }
    carry
}

/// lhs -= rhs
///
/// Returns borrow.
#[must_use]
pub(crate) fn sub_same_len_in_place(lhs: &mut [Word], rhs: &[Word]) -> bool {
    debug_assert!(lhs.len() == rhs.len());
    let mut borrow = false;
    for (a, b) in lhs.iter_mut().zip(rhs.iter()) {
        let (diff, borrow1) = arch::add::sub_with_borrow(*a, *b, borrow);
        *a = diff;
        borrow = borrow1;
    }
    borrow
}

/// lhs += rhs
///
/// Returns overflow.
#[must_use]
pub(crate) fn add_in_place(lhs: &mut [Word], rhs: &[Word]) -> bool {
    let (lhs_lo, lhs_hi) = lhs.split_at_mut(rhs.len());
    let carry = add_same_len_in_place(lhs_lo, rhs);
    carry && add_one_in_place(lhs_hi)
}

/// lhs -= rhs
///
/// Returns borrow.
#[must_use]
pub(crate) fn sub_in_place(lhs: &mut [Word], rhs: &[Word]) -> bool {
    let (lhs_lo, lhs_hi) = lhs.split_at_mut(rhs.len());
    let borrow = sub_same_len_in_place(lhs_lo, rhs);
    borrow && sub_one_in_place(lhs_hi)
}

/// rhs = lhs - rhs
///
/// Returns borrow.
#[must_use]
pub(crate) fn sub_same_len_in_place_swap(lhs: &[Word], rhs: &mut [Word]) -> bool {
    debug_assert!(lhs.len() == rhs.len());
    let mut borrow = false;
    for (a, b) in lhs.iter().zip(rhs.iter_mut()) {
        let (diff, borrow1) = arch::add::sub_with_borrow(*a, *b, borrow);
        *b = diff;
        borrow = borrow1;
    }
    borrow
}

/// (sign, lhs) = lhs - rhs
#[must_use]
pub(crate) fn sub_in_place_with_sign(lhs: &mut [Word], rhs: &[Word]) -> Sign {
    assert!(lhs.len() >= rhs.len());
    let mut lhs_len = lhs.len();
    while lhs_len != 0 && lhs[lhs_len - 1] == 0 {
        lhs_len -= 1;
    }
    let mut rhs_len = rhs.len();
    while rhs_len != 0 && rhs[rhs_len - 1] == 0 {
        rhs_len -= 1;
    }
    match lhs_len.cmp(&rhs_len) {
        Greater => {
            let overflow = sub_in_place(&mut lhs[..lhs_len], &rhs[..rhs_len]);
            assert!(!overflow);
            Positive
        }
        Less => {
            let borrow = sub_same_len_in_place_swap(&rhs[..lhs_len], &mut lhs[..lhs_len]);
            lhs[lhs_len..rhs_len].copy_from_slice(&rhs[lhs_len..rhs_len]);
            if borrow {
                let overflow = sub_one_in_place(&mut lhs[lhs_len..rhs_len]);
                assert!(!overflow);
            }
            Negative
        }
        Equal => {
            let mut n = lhs_len;
            while n != 0 {
                match lhs[n - 1].cmp(&rhs[n - 1]) {
                    Greater => {
                        let overflow = sub_same_len_in_place(&mut lhs[..n], &rhs[..n]);
                        assert!(!overflow);
                        return Positive;
                    }
                    Less => {
                        let overflow = sub_same_len_in_place_swap(&rhs[..n], &mut lhs[..n]);
                        assert!(!overflow);
                        return Negative;
                    }
                    Equal => {
                        n -= 1;
                        lhs[n] = 0;
                    }
                }
            }
            // Zero.
            Positive
        }
    }
}

/// Add a signed word to a non-empty word sequence.
///
/// Returns overflow.
#[must_use]
pub(crate) fn add_signed_word_in_place(words: &mut [Word], rhs: SignedWord) -> SignedWord {
    if rhs == 0 || words.is_empty() {
        return rhs;
    }
    match rhs.to_sign_magnitude() {
        (Positive, u) => SignedWord::from(add_word_in_place(words, u)),
        (Negative, u) => -SignedWord::from(sub_word_in_place(words, u)),
    }
}

/// words += sign * rhs
///
/// Returns overflow.
#[must_use]
pub(crate) fn add_signed_same_len_in_place(
    words: &mut [Word],
    sign: Sign,
    rhs: &[Word],
) -> SignedWord {
    debug_assert!(words.len() == rhs.len());
    match sign {
        Positive => SignedWord::from(add_same_len_in_place(words, rhs)),
        Negative => -SignedWord::from(sub_same_len_in_place(words, rhs)),
    }
}

/// words += sign * rhs
///
/// Returns overflow.
#[must_use]
pub(crate) fn add_signed_in_place(words: &mut [Word], sign: Sign, rhs: &[Word]) -> SignedWord {
    debug_assert!(words.len() >= rhs.len());
    match sign {
        Positive => SignedWord::from(add_in_place(words, rhs)),
        Negative => -SignedWord::from(sub_in_place(words, rhs)),
    }
}

// FIXME: ibig makes invalid assumptions about how Vec works, we need to audit this.
// #[cfg(test)]
// mod tests {
//     use super::*;

//     #[test]
//     fn test_add_one_in_place() {
//         let mut a = [1, 2, 3];
//         let overflow = add_one_in_place(&mut a);
//         assert!(!overflow);
//         assert_eq!(a, [2, 2, 3]);

//         let mut a = [Word::MAX, Word::MAX, 3];
//         let overflow = add_one_in_place(&mut a);
//         assert!(!overflow);
//         assert_eq!(a, [0, 0, 4]);

//         let mut a = [Word::MAX, Word::MAX, Word::MAX];
//         let overflow = add_one_in_place(&mut a);
//         assert!(overflow);
//         assert_eq!(a, [0, 0, 0]);

//         let mut a = [];
//         let overflow = add_one_in_place(&mut a);
//         assert!(overflow);
//     }

//     #[test]
//     fn test_add_word_in_place() {
//         let mut a = [1, 2, 3];
//         let overflow = add_word_in_place(&mut a, 7);
//         assert!(!overflow);
//         assert_eq!(a, [8, 2, 3]);

//         let mut a = [Word::MAX / 2, Word::MAX, 3];
//         let overflow = add_word_in_place(&mut a, Word::MAX / 2 + 3);
//         assert!(!overflow);
//         assert_eq!(a, [1, 0, 4]);

//         let mut a = [Word::MAX / 2, Word::MAX, Word::MAX];
//         let overflow = add_word_in_place(&mut a, Word::MAX / 2 + 3);
//         assert!(overflow);
//         assert_eq!(a, [1, 0, 0]);
//     }

//     #[test]
//     fn test_add_signed_word_in_place() {
//         let mut a = [];
//         let overflow = add_signed_word_in_place(&mut a, -5);
//         assert_eq!(overflow, -5);

//         let mut a = [1, 2, 3];
//         let overflow = add_signed_word_in_place(&mut a, 4);
//         assert_eq!(overflow, 0);
//         assert_eq!(a, [5, 2, 3]);

//         let mut a = [3, 0];
//         let overflow = add_signed_word_in_place(&mut a, -4);
//         assert_eq!(overflow, -1);
//         assert_eq!(a, [Word::MAX, Word::MAX]);
//     }

//     #[test]
//     fn test_add_in_place() {
//         let mut a = [1, 2, 3];
//         let overflow = add_in_place(&mut a, &[3, 7]);
//         assert!(!overflow);
//         assert_eq!(a, [4, 9, 3]);

//         let mut a = [Word::MAX / 2, 1, Word::MAX];
//         let overflow = add_in_place(&mut a, &[Word::MAX / 2 + 3, Word::MAX]);
//         assert!(overflow);
//         assert_eq!(a, [1, 1, 0]);
//     }

//     #[test]
//     fn test_sub_one_in_place() {
//         let mut a = [2, 2, 3];
//         let overflow = sub_one_in_place(&mut a);
//         assert!(!overflow);
//         assert_eq!(a, [1, 2, 3]);

//         let mut a = [0, 0, 4];
//         let overflow = sub_one_in_place(&mut a);
//         assert!(!overflow);
//         assert_eq!(a, [Word::MAX, Word::MAX, 3]);

//         let mut a = [0, 0, 0];
//         let overflow = sub_one_in_place(&mut a);
//         assert!(overflow);
//         assert_eq!(a, [Word::MAX, Word::MAX, Word::MAX]);

//         let mut a = [];
//         let overflow = sub_one_in_place(&mut a);
//         assert!(overflow);
//     }

//     #[test]
//     fn test_sub_word_in_place() {
//         let mut a = [8, 2, 3];
//         let overflow = sub_word_in_place(&mut a, 7);
//         assert!(!overflow);
//         assert_eq!(a, [1, 2, 3]);

//         let mut a = [1, 0, 4];
//         let overflow = sub_word_in_place(&mut a, Word::MAX / 2 + 3);
//         assert!(!overflow);
//         assert_eq!(a, [Word::MAX / 2, Word::MAX, 3]);

//         let mut a = [1, 0, 0];
//         let overflow = sub_word_in_place(&mut a, Word::MAX / 2 + 3);
//         assert!(overflow);
//         assert_eq!(a, [Word::MAX / 2, Word::MAX, Word::MAX]);
//     }

//     #[test]
//     fn test_sub_in_place() {
//         let mut a = [4, 9, 3];
//         let overflow = sub_in_place(&mut a, &[3, 7]);
//         assert!(!overflow);
//         assert_eq!(a, [1, 2, 3]);

//         let mut a = [1, 1, 0];
//         let overflow = sub_in_place(&mut a, &[Word::MAX / 2 + 3, Word::MAX]);
//         assert!(overflow);
//         assert_eq!(a, [Word::MAX / 2, 1, Word::MAX]);
//     }

//     #[test]
//     fn test_sub_in_place_with_sign() {
//         let mut a = [4, 9, 3];
//         let sign = sub_in_place_with_sign(&mut a, &[3, 7]);
//         assert_eq!(sign, Positive);
//         assert_eq!(a, [1, 2, 3]);

//         let mut a = [4, 0, 0, 0];
//         let sign = sub_in_place_with_sign(&mut a, &[1, 2, 0]);
//         assert_eq!(sign, Negative);
//         assert_eq!(a, [Word::MAX - 2, 1, 0, 0]);

//         let mut a = [4, 9, 3];
//         let sign = sub_in_place_with_sign(&mut a, &[3, 9, 3]);
//         assert_eq!(sign, Positive);
//         assert_eq!(a, [1, 0, 0]);

//         let mut a = [4, 9, 3];
//         let sign = sub_in_place_with_sign(&mut a, &[5, 9, 3]);
//         assert_eq!(sign, Negative);
//         assert_eq!(a, [1, 0, 0]);
//     }
// }
