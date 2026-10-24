//! Element of modular arithmetic.

use alloc::vec::Vec;

use crate::arch::word::Word;
use crate::math;
use crate::modular::modulo_ring::{ModuloRingLarge, ModuloRingSmall};

/// Modular arithmetic.
///
/// # Examples
///
/// ```
/// # use ibig::{modular::ModuloRing, ubig};
/// let ring = ModuloRing::new(&ubig!(10000));
/// let x = ring.from(12345);
/// let y = ring.from(55443);
/// assert_eq!((x - y).residue(), ubig!(6902));
/// ```
pub struct Modulo<'a>(ModuloRepr<'a>);

pub(crate) enum ModuloRepr<'a> {
    Small(ModuloSmall<'a>),
    Large(ModuloLarge<'a>),
}

/// Modular value in some unknown ring. The ring must be provided to operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ModuloSmallRaw {
    /// must be in range 0..modulus and divisible by the shift for ModuloSmallRing
    normalized_value: Word,
}

#[derive(Clone)]
pub(crate) struct ModuloSmall<'a> {
    ring: &'a ModuloRingSmall,
    raw: ModuloSmallRaw,
}

pub(crate) struct ModuloLarge<'a> {
    ring: &'a ModuloRingLarge,
    /// normalized_value.len() == ring.normalized_modulus.len()
    normalized_value: Vec<Word>,
}

impl<'a> Modulo<'a> {
    /// Get representation.
    #[inline]
    pub(crate) fn repr(&self) -> &ModuloRepr<'a> {
        &self.0
    }

    /// Get mutable representation.
    #[inline]
    pub(crate) fn repr_mut(&mut self) -> &mut ModuloRepr<'a> {
        &mut self.0
    }

    /// Panics when trying to do operations on [Modulo] values from different rings.
    pub(crate) fn panic_different_rings() -> ! {
        panic!("Modulo values from different rings")
    }
}

impl<'a> From<ModuloSmall<'a>> for Modulo<'a> {
    #[inline]
    fn from(a: ModuloSmall<'a>) -> Self {
        Modulo(ModuloRepr::Small(a))
    }
}

impl<'a> From<ModuloLarge<'a>> for Modulo<'a> {
    fn from(a: ModuloLarge<'a>) -> Self {
        Modulo(ModuloRepr::Large(a))
    }
}

impl ModuloSmallRaw {
    #[inline]
    pub(crate) const fn normalized(self) -> Word {
        self.normalized_value
    }

    #[inline]
    pub(crate) const fn from_normalized(normalized_value: Word) -> Self {
        ModuloSmallRaw { normalized_value }
    }

    #[inline]
    pub(crate) const fn is_valid(&self, ring: &ModuloRingSmall) -> bool {
        self.normalized_value < ring.normalized_modulus()
            && self.normalized_value & math::ones_word(ring.shift()) == 0
    }
}

impl<'a> ModuloSmall<'a> {
    #[inline]
    pub(crate) fn new(raw: ModuloSmallRaw, ring: &'a ModuloRingSmall) -> Self {
        debug_assert!(raw.is_valid(ring));
        ModuloSmall { ring, raw }
    }

    /// Get the ring.
    #[inline]
    pub(crate) fn ring(&self) -> &'a ModuloRingSmall {
        self.ring
    }

    #[inline]
    pub(crate) fn raw(&self) -> ModuloSmallRaw {
        self.raw
    }

    #[inline]
    pub(crate) fn set_raw(&mut self, raw: ModuloSmallRaw) {
        debug_assert!(raw.is_valid(self.ring));
        self.raw = raw;
    }

    /// Checks that two values are from the same ring.
    #[inline]
    pub(crate) fn check_same_ring(&self, other: &ModuloSmall) {
        if self.ring() != other.ring() {
            Modulo::panic_different_rings();
        }
    }
}

impl<'a> ModuloLarge<'a> {
    /// Create new ModuloLarge.
    ///
    /// normalized_value must have the same length as the modulus, be in range 0..modulus,
    /// and be divisible by the shift.
    pub(crate) fn new(normalized_value: Vec<Word>, ring: &'a ModuloRingLarge) -> Self {
        debug_assert!(ring.is_valid(&normalized_value));
        ModuloLarge {
            ring,
            normalized_value,
        }
    }

    /// Get the ring.
    pub(crate) fn ring(&self) -> &'a ModuloRingLarge {
        self.ring
    }

    /// Get normalized value.
    pub(crate) fn normalized_value(&self) -> &[Word] {
        &self.normalized_value
    }

    /// Modify normalized value.
    pub(crate) fn modify_normalized_value<F>(&mut self, f: F)
    where
        F: FnOnce(&mut [Word], &ModuloRingLarge),
    {
        f(&mut self.normalized_value, self.ring);
        debug_assert!(self.ring.is_valid(&self.normalized_value));
    }

    /// Checks that two values are from the same ring.
    pub(crate) fn check_same_ring(&self, other: &ModuloLarge) {
        if self.ring() != other.ring() {
            Modulo::panic_different_rings();
        }
    }
}

impl Clone for Modulo<'_> {
    #[inline]
    fn clone(&self) -> Self {
        Modulo(self.0.clone())
    }

    #[inline]
    fn clone_from(&mut self, source: &Self) {
        self.0.clone_from(&source.0);
    }
}

impl Clone for ModuloRepr<'_> {
    #[inline]
    fn clone(&self) -> Self {
        match self {
            ModuloRepr::Small(modulo_small) => ModuloRepr::Small(modulo_small.clone()),
            ModuloRepr::Large(modulo_large) => ModuloRepr::Large(modulo_large.clone()),
        }
    }

    #[inline]
    fn clone_from(&mut self, source: &Self) {
        if let (ModuloRepr::Large(modulo_large), ModuloRepr::Large(source_large)) =
            (&mut *self, source)
        {
            modulo_large.clone_from(source_large);
        } else {
            *self = source.clone();
        }
    }
}

impl Clone for ModuloLarge<'_> {
    fn clone(&self) -> Self {
        ModuloLarge {
            ring: self.ring,
            normalized_value: self.normalized_value.clone(),
        }
    }

    fn clone_from(&mut self, source: &Self) {
        self.ring = source.ring;
        if self.normalized_value.len() == source.normalized_value.len() {
            self.normalized_value
                .copy_from_slice(&source.normalized_value)
        } else {
            // We don't want to have spare capacity, so do not clone_from.
            self.normalized_value = source.normalized_value.clone();
        }
    }
}
