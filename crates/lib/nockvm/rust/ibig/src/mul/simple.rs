//! Simple multiplication algorithm.

use alloc::alloc::Layout;

use crate::arch::word::{SignedWord, Word};
use crate::arch::{self};
use crate::memory::{self, Memory};
use crate::mul::{self, helpers};
use crate::sign::Sign::{self, *};

/// Split larger length into chunks of CHUNK_LEN..2 * CHUNK_LEN for memory locality.
const CHUNK_LEN: usize = 1024;

/// Max supported smaller factor length.
pub(crate) const MAX_SMALLER_LEN: usize = CHUNK_LEN;

/// Temporary memory required for multiplication.
///
/// n bounds the length of the smaller factor in words.
pub(crate) fn memory_requirement_up_to(_n: usize) -> Layout {
    memory::zero_layout()
}

/// c += sign * a * b
/// Simple method: O(a.len() * b.len()).
///
/// Returns carry.
pub(crate) fn add_signed_mul(
    c: &mut [Word],
    sign: Sign,
    a: &[Word],
    b: &[Word],
    memory: &mut Memory,
) -> SignedWord {
    debug_assert!(a.len() >= b.len() && c.len() == a.len() + b.len());
    debug_assert!(b.len() <= MAX_SMALLER_LEN);
    if a.len() <= CHUNK_LEN {
        add_signed_mul_chunk(c, sign, a, b, memory)
    } else {
        helpers::add_signed_mul_split_into_chunks(
            c, sign, a, b, CHUNK_LEN, memory, add_signed_mul_chunk,
        )
    }
}

/// c += sign * a * b
/// Simple method: O(a.len() * b.len()).
///
/// Returns carry.
pub(crate) fn add_signed_mul_same_len(
    c: &mut [Word],
    sign: Sign,
    a: &[Word],
    b: &[Word],
    memory: &mut Memory,
) -> SignedWord {
    debug_assert!(a.len() == b.len() && c.len() == a.len() + b.len());
    debug_assert!(b.len() <= MAX_SMALLER_LEN);
    add_signed_mul_chunk(c, sign, a, b, memory)
}

/// c += sign * a * b
/// Simple method: O(a.len() * b.len()).
///
/// Returns carry.
fn add_signed_mul_chunk(
    c: &mut [Word],
    sign: Sign,
    a: &[Word],
    b: &[Word],
    _memory: &mut Memory,
) -> SignedWord {
    debug_assert!(a.len() >= b.len() && c.len() == a.len() + b.len());
    debug_assert!(a.len() <= CHUNK_LEN);

    match sign {
        Positive => SignedWord::from(add_mul_chunk(c, a, b)),
        Negative => -SignedWord::from(sub_mul_chunk(c, a, b)),
    }
}

/// c += a * b
/// Simple method: O(a.len() * b.len()).
///
/// Returns carry.
fn add_mul_chunk(c: &mut [Word], a: &[Word], b: &[Word]) -> bool {
    debug_assert!(a.len() >= b.len() && c.len() == a.len() + b.len());
    debug_assert!(a.len() < 2 * CHUNK_LEN);
    let mut carry = false;
    for (i, m) in b.iter().enumerate() {
        let carry_word = mul::add_mul_word_same_len_in_place(&mut c[i..i + a.len()], *m, a);
        let (carry_word, carry_next) = arch::add::add_with_carry(c[i + a.len()], carry_word, carry);
        c[i + a.len()] = carry_word;
        carry = carry_next;
    }
    carry
}

/// c -= a * b
/// Simple method: O(a.len() * b.len()).
///
/// Returns borrow.
fn sub_mul_chunk(c: &mut [Word], a: &[Word], b: &[Word]) -> bool {
    debug_assert!(a.len() >= b.len() && c.len() == a.len() + b.len());
    debug_assert!(a.len() < 2 * CHUNK_LEN);
    let mut borrow = false;
    for (i, m) in b.iter().enumerate() {
        let borrow_word = mul::sub_mul_word_same_len_in_place(&mut c[i..i + a.len()], *m, a);
        let (borrow_word, borrow_next) =
            arch::add::sub_with_borrow(c[i + a.len()], borrow_word, borrow);
        c[i + a.len()] = borrow_word;
        borrow = borrow_next;
    }
    borrow
}
