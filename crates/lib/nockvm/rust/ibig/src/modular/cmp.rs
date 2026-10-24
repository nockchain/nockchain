//! Comparisons.

use core::ptr;

use crate::modular::modulo::{Modulo, ModuloLarge, ModuloRepr, ModuloSmall};
use crate::modular::modulo_ring::{ModuloRing, ModuloRingLarge, ModuloRingSmall};

/// Equality is identity: two rings are not equal even if they have the same modulus.
impl PartialEq for ModuloRing {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self, other)
    }
}

impl Eq for ModuloRing {}

/// Equality is identity: two rings are not equal even if they have the same modulus.
impl PartialEq for ModuloRingSmall {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self, other)
    }
}

impl Eq for ModuloRingSmall {}

/// Equality is identity: two rings are not equal even if they have the same modulus.
impl PartialEq for ModuloRingLarge {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self, other)
    }
}

impl Eq for ModuloRingLarge {}

/// Equality within a ring.
///
/// # Panics
///
/// Panics if the two values are from different rings.
impl PartialEq for Modulo<'_> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        match (self.repr(), other.repr()) {
            (ModuloRepr::Small(self_small), ModuloRepr::Small(other_small)) => {
                self_small.eq(other_small)
            }
            (ModuloRepr::Large(self_large), ModuloRepr::Large(other_large)) => {
                self_large.eq(other_large)
            }
            _ => Modulo::panic_different_rings(),
        }
    }
}

impl Eq for Modulo<'_> {}

impl PartialEq for ModuloSmall<'_> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.check_same_ring(other);
        self.raw() == other.raw()
    }
}

impl Eq for ModuloLarge<'_> {}

impl PartialEq for ModuloLarge<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.check_same_ring(other);
        self.normalized_value() == other.normalized_value()
    }
}
