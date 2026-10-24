use crate::arch::word::Word;

/// Add a + b + carry.
///
/// Returns (result, overflow).
#[inline]
pub(crate) fn add_with_carry(a: Word, b: Word, carry: bool) -> (Word, bool) {
    let (sum, c0) = a.overflowing_add(b);
    let (sum, c1) = sum.overflowing_add(Word::from(carry));
    (sum, c0 | c1)
}

/// Subtract a - b - borrow.
///
/// Returns (result, overflow).
#[inline]
pub(crate) fn sub_with_borrow(a: Word, b: Word, borrow: bool) -> (Word, bool) {
    let (diff, b0) = a.overflowing_sub(b);
    let (diff, b1) = diff.overflowing_sub(Word::from(borrow));
    (diff, b0 | b1)
}
