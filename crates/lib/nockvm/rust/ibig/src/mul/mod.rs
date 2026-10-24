//! Multiplication.

use alloc::alloc::Layout;
use core::mem;

use static_assertions::const_assert;

use crate::add;
use crate::arch::word::{SignedWord, Word};
use crate::memory::Memory;
use crate::primitive::{double_word, extend_word, split_double_word};
use crate::sign::Sign;

/// If smaller length <= MAX_LEN_SIMPLE, simple multiplication can be used.
const MAX_LEN_SIMPLE: usize = 24;
const_assert!(MAX_LEN_SIMPLE <= simple::MAX_SMALLER_LEN);
const_assert!(MAX_LEN_SIMPLE + 1 >= karatsuba::MIN_LEN);

/// If smaller length <= this, Karatsuba multiplication can be used.
const MAX_LEN_KARATSUBA: usize = 192;
const_assert!(MAX_LEN_KARATSUBA + 1 >= toom_3::MIN_LEN);

mod helpers;
mod karatsuba;
pub(crate) mod ntt;
mod simple;
mod toom_3;

/// Multiply a word sequence by a `Word` in place.
///
/// Returns carry.
#[must_use]
pub(crate) fn mul_word_in_place(words: &mut [Word], rhs: Word) -> Word {
    mul_word_in_place_with_carry(words, rhs, 0)
}

/// Multiply a word sequence by a `Word` in place with carry in.
///
/// Returns carry.
#[must_use]
pub(crate) fn mul_word_in_place_with_carry(words: &mut [Word], rhs: Word, mut carry: Word) -> Word {
    for a in words {
        // a * b + carry <= MAX * MAX + MAX < DoubleWord::MAX
        let (v_lo, v_hi) =
            split_double_word(extend_word(*a) * extend_word(rhs) + extend_word(carry));
        *a = v_lo;
        carry = v_hi;
    }
    carry
}

/// words += mult * rhs
///
/// Returns carry.
#[must_use]
fn add_mul_word_same_len_in_place(words: &mut [Word], mult: Word, rhs: &[Word]) -> Word {
    assert!(words.len() == rhs.len());
    let mut carry: Word = 0;
    for (a, b) in words.iter_mut().zip(rhs.iter()) {
        // a + mult * b + carry <= MAX * MAX + 2 * MAX <= DoubleWord::MAX
        let (v_lo, v_hi) = split_double_word(
            extend_word(*a) + extend_word(carry) + extend_word(mult) * extend_word(*b),
        );
        *a = v_lo;
        carry = v_hi;
    }
    carry
}

/// words += mult * rhs
///
/// Returns carry.
#[must_use]
fn add_mul_word_in_place(words: &mut [Word], mult: Word, rhs: &[Word]) -> Word {
    assert!(words.len() >= rhs.len());
    let n = rhs.len();
    let mut carry = add_mul_word_same_len_in_place(&mut words[..n], mult, rhs);
    if words.len() > n {
        carry = Word::from(add::add_word_in_place(&mut words[n..], carry));
    }
    carry
}

/// words -= mult * rhs
///
/// Returns borrow.
#[must_use]
pub(crate) fn sub_mul_word_same_len_in_place(words: &mut [Word], mult: Word, rhs: &[Word]) -> Word {
    assert!(words.len() == rhs.len());
    // carry is in -Word::MAX..0
    // carry_plus_max = carry + Word::MAX
    let mut carry_plus_max = Word::MAX;
    for (a, b) in words.iter_mut().zip(rhs.iter()) {
        // Compute val = a - mult * b + carry_plus_max - MAX + (MAX << BITS)
        // val >= 0 - MAX * MAX - MAX + MAX*(MAX+1) = 0
        // val <= MAX - 0 + MAX - MAX + (MAX<<BITS) = DoubleWord::MAX
        // This fits exactly in DoubleWord!
        // We have to be careful to calculate in the correct order to avoid overflow.
        let v = extend_word(*a)
            + extend_word(carry_plus_max)
            + (double_word(0, Word::MAX) - extend_word(Word::MAX))
            - extend_word(mult) * extend_word(*b);
        let (v_lo, v_hi) = split_double_word(v);
        *a = v_lo;
        carry_plus_max = v_hi;
    }
    Word::MAX - carry_plus_max
}

/// Temporary scratch space required for multiplication.
pub(crate) fn memory_requirement_up_to(_total_len: usize, smaller_len: usize) -> Layout {
    if smaller_len <= MAX_LEN_SIMPLE {
        simple::memory_requirement_up_to(smaller_len)
    } else if smaller_len <= MAX_LEN_KARATSUBA {
        karatsuba::memory_requirement_up_to(smaller_len)
    } else {
        toom_3::memory_requirement_up_to(smaller_len)
    }
}

/// Temporary scratch space required for multiplication.
pub(crate) fn memory_requirement_exact(total_len: usize, smaller_len: usize) -> Layout {
    memory_requirement_up_to(total_len, smaller_len)
}

/// c += sign * a * b
///
/// Returns carry.
#[must_use]
pub(crate) fn add_signed_mul<'a>(
    c: &mut [Word],
    sign: Sign,
    mut a: &'a [Word],
    mut b: &'a [Word],
    memory: &mut Memory,
) -> SignedWord {
    debug_assert!(c.len() == a.len() + b.len());

    if a.len() < b.len() {
        mem::swap(&mut a, &mut b);
    }

    if b.len() <= MAX_LEN_SIMPLE {
        simple::add_signed_mul(c, sign, a, b, memory)
    } else if b.len() <= MAX_LEN_KARATSUBA {
        karatsuba::add_signed_mul(c, sign, a, b, memory)
    } else {
        toom_3::add_signed_mul(c, sign, a, b, memory)
    }
}

/// c += sign * a * b
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

    if n <= MAX_LEN_SIMPLE {
        simple::add_signed_mul_same_len(c, sign, a, b, memory)
    } else if n <= MAX_LEN_KARATSUBA {
        karatsuba::add_signed_mul_same_len(c, sign, a, b, memory)
    } else {
        toom_3::add_signed_mul_same_len(c, sign, a, b, memory)
    }
}
