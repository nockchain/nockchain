//! Comparisons operators.

use core::cmp::Ordering;

use crate::arch::word::Word;
use crate::ibig::IBig;
use crate::sign::Sign::*;
use crate::ubig::Repr::*;
use crate::ubig::UBig;

impl Ord for UBig {
    #[inline]
    fn cmp(&self, other: &UBig) -> Ordering {
        match (self.repr(), other.repr()) {
            (Small(word), Small(other_word)) => word.cmp(other_word),
            (Small(_), Large(_)) => Ordering::Less,
            (Large(_), Small(_)) => Ordering::Greater,
            (Large(buffer), Large(other_buffer)) => buffer
                .len()
                .cmp(&other_buffer.len())
                .then_with(|| cmp_same_len(buffer, other_buffer)),
        }
    }
}

impl PartialOrd for UBig {
    #[inline]
    fn partial_cmp(&self, other: &UBig) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for IBig {
    #[inline]
    fn cmp(&self, other: &IBig) -> Ordering {
        match (self.sign(), other.sign()) {
            (Positive, Positive) => self.magnitude().cmp(other.magnitude()),
            (Positive, Negative) => Ordering::Greater,
            (Negative, Positive) => Ordering::Less,
            (Negative, Negative) => other.magnitude().cmp(self.magnitude()),
        }
    }
}

impl PartialOrd for IBig {
    #[inline]
    fn partial_cmp(&self, other: &IBig) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Compare lhs with rhs as numbers.
pub(crate) fn cmp_same_len(lhs: &[Word], rhs: &[Word]) -> Ordering {
    assert!(lhs.len() == rhs.len());
    lhs.iter().rev().cmp(rhs.iter().rev())
}
