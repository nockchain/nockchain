use crate::arch::word::Word;

/// Add a + b + carry.
///
/// Returns (result, overflow).
#[inline]
pub(crate) fn add_with_carry(a: Word, b: Word, carry: bool) -> (Word, bool) {
    let mut sum = 0;
    let carry = unsafe { core::arch::x86_64::_addcarry_u64(carry.into(), a, b, &mut sum) };
    (sum, carry != 0)
}

/// Subtract a - b - borrow.
///
/// Returns (result, overflow).
#[inline]
pub(crate) fn sub_with_borrow(a: Word, b: Word, borrow: bool) -> (Word, bool) {
    let mut diff = 0;
    let borrow = unsafe { core::arch::x86_64::_subborrow_u64(borrow.into(), a, b, &mut diff) };
    (diff, borrow != 0)
}
