//! Helper functions for multiplication algorithms.

use crate::arch::word::{SignedWord, Word};
use crate::memory::Memory;
use crate::sign::Sign;
use crate::{add, mul};

/// c += sign * a * b
///
/// Splits a into chunks of chunk_len, using regular multiplication for the remainder if any.
///
/// Returns carry.
pub(crate) fn add_signed_mul_split_into_chunks<F>(
    mut c: &mut [Word],
    sign: Sign,
    mut a: &[Word],
    b: &[Word],
    chunk_len: usize,
    memory: &mut Memory,
    f_add_signed_mul_chunk: F,
) -> SignedWord
where
    F: Fn(&mut [Word], Sign, &[Word], &[Word], &mut Memory) -> SignedWord,
{
    debug_assert!(a.len() >= b.len() && c.len() == a.len() + b.len());
    debug_assert!(b.len() <= chunk_len);

    let n = b.len();
    let mut carry_n = 0; // at c[n]
    while a.len() >= chunk_len {
        let (a_lo, a_hi) = a.split_at(chunk_len);
        // Propagate carry_n
        carry_n = add::add_signed_word_in_place(&mut c[n..chunk_len + n], carry_n);
        carry_n += f_add_signed_mul_chunk(&mut c[..chunk_len + n], sign, a_lo, b, memory);
        a = a_hi;
        c = &mut c[chunk_len..];
    }
    // Propagate carry_n
    let mut carry = add::add_signed_word_in_place(&mut c[n..], carry_n);
    if a.len() >= b.len() {
        carry += mul::add_signed_mul(c, sign, a, b, memory);
    } else if !a.is_empty() {
        carry += mul::add_signed_mul(c, sign, b, a, memory);
    }
    carry
}
