//! A ring of integers modulo a positive integer.

use alloc::vec::Vec;
use core::cmp::Ordering;

use crate::arch::word::Word;
use crate::assert::debug_assert_in_const_fn;
use crate::fast_divide::FastDivideNormalized;
use crate::ubig::{Repr, UBig};
use crate::{cmp, div, math};

/// A ring of integers modulo a positive integer.
///
/// # Examples
///
/// ```
/// # use ibig::{modular::ModuloRing, ubig};
/// let ring = ModuloRing::new(&ubig!(100));
/// assert_eq!(ring.modulus(), ubig!(100));
/// ```
pub struct ModuloRing(ModuloRingRepr);

pub(crate) enum ModuloRingRepr {
    Small(ModuloRingSmall),
    Large(ModuloRingLarge),
}

pub(crate) struct ModuloRingSmall {
    normalized_modulus: Word,
    shift: u32,
    fast_div: FastDivideNormalized,
}

pub(crate) struct ModuloRingLarge {
    normalized_modulus: Vec<Word>,
    shift: u32,
    fast_div_top: FastDivideNormalized,
}

impl ModuloRing {
    /// Create a new ring of integers modulo `n`.
    ///
    /// For two [Modulo](crate::modular::Modulo) numbers to be compatible,
    /// they must come from the same [ModuloRing].
    /// Two different [ModuloRing]s are not compatible even if
    /// they have the same modulus `n`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use ibig::{modular::ModuloRing, ubig};
    /// let ring = ModuloRing::new(&ubig!(100));
    /// assert_eq!(ring.modulus(), ubig!(100));
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if `n` is zero.
    #[inline]
    pub fn new(n: &UBig) -> ModuloRing {
        match n.repr() {
            Repr::Small(0) => panic!("ModuloRing::new(0)"),
            Repr::Small(word) => ModuloRing(ModuloRingRepr::Small(ModuloRingSmall::new(*word))),
            Repr::Large(words) => ModuloRing(ModuloRingRepr::Large(ModuloRingLarge::new(words))),
        }
    }

    #[inline]
    pub(crate) fn repr(&self) -> &ModuloRingRepr {
        &self.0
    }
}

impl ModuloRingSmall {
    /// Create a new small ring of integers modulo `n`.
    #[inline]
    pub(crate) const fn new(n: Word) -> ModuloRingSmall {
        debug_assert_in_const_fn!(n != 0);
        let shift = n.leading_zeros();
        let normalized_modulus = n << shift;
        let fast_div = FastDivideNormalized::new(normalized_modulus);
        ModuloRingSmall {
            normalized_modulus,
            shift,
            fast_div,
        }
    }

    #[inline]
    pub(crate) const fn normalized_modulus(&self) -> Word {
        self.normalized_modulus
    }

    #[inline]
    pub(crate) const fn shift(&self) -> u32 {
        self.shift
    }

    #[inline]
    pub(crate) const fn fast_div(&self) -> FastDivideNormalized {
        self.fast_div
    }
}

impl ModuloRingLarge {
    /// Create a new large ring of integers modulo `n`.
    fn new(n: &[Word]) -> ModuloRingLarge {
        let mut normalized_modulus = n.to_vec();
        let (shift, fast_div_top) = div::normalize_large(&mut normalized_modulus);
        ModuloRingLarge {
            normalized_modulus,
            shift,
            fast_div_top,
        }
    }

    pub(crate) fn normalized_modulus(&self) -> &[Word] {
        &self.normalized_modulus
    }

    pub(crate) fn shift(&self) -> u32 {
        self.shift
    }

    pub(crate) fn fast_div_top(&self) -> FastDivideNormalized {
        self.fast_div_top
    }

    pub(crate) fn is_valid(&self, val: &[Word]) -> bool {
        val.len() == self.normalized_modulus.len()
            && cmp::cmp_same_len(val, &self.normalized_modulus) == Ordering::Less
            && val[0] & math::ones::<Word>(self.shift) == 0
    }
}
