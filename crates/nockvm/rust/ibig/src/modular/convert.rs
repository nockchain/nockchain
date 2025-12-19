//! Conversion between Modulo, UBig and IBig.

use alloc::vec::Vec;
use core::iter;

use crate::arch::word::Word;
use crate::buffer::Buffer;
use crate::ibig::IBig;
use crate::memory::MemoryAllocation;
use crate::modular::modulo::{Modulo, ModuloLarge, ModuloRepr, ModuloSmall, ModuloSmallRaw};
use crate::modular::modulo_ring::{ModuloRing, ModuloRingLarge, ModuloRingRepr, ModuloRingSmall};
use crate::primitive::extend_word;
use crate::sign::Sign::*;
use crate::ubig::{Repr, UBig};
use crate::{div, shift};

impl ModuloRing {
    /// The ring modulus.
    ///
    /// # Example
    ///
    /// ```
    /// # use ibig::{modular::ModuloRing, ubig};
    /// let ring = ModuloRing::new(&ubig!(100));
    /// assert_eq!(ring.modulus(), ubig!(100));
    /// ```
    #[inline]
    pub fn modulus(&self) -> UBig {
        match self.repr() {
            ModuloRingRepr::Small(self_small) => UBig::from_word(self_small.modulus()),
            ModuloRingRepr::Large(self_large) => self_large.modulus(),
        }
    }

    /// Create an element of the ring from another type.
    ///
    /// # Examples
    ///
    /// ```
    /// # use ibig::{modular::ModuloRing, ubig};
    /// let ring = ModuloRing::new(&ubig!(100));
    /// let x = ring.from(-1234);
    /// let y = ring.from(ubig!(3366));
    /// assert!(x == y);
    /// ```
    #[inline]
    pub fn from<T: IntoModulo>(&self, x: T) -> Modulo {
        x.into_modulo(self)
    }
}

impl ModuloRingSmall {
    #[inline]
    pub(crate) fn modulus(&self) -> Word {
        self.normalized_modulus() >> self.shift()
    }
}

impl ModuloRingLarge {
    pub(crate) fn modulus(&self) -> UBig {
        let normalized_modulus = self.normalized_modulus();
        let mut buffer = Buffer::allocate(normalized_modulus.len());
        buffer.extend(normalized_modulus);
        let low_bits = shift::shr_in_place(&mut buffer, self.shift());
        assert!(low_bits == 0);
        buffer.into()
    }
}

impl Modulo<'_> {
    /// Get the residue in range `0..n` in an n-element ring.
    ///
    /// # Examples
    ///
    /// ```
    /// # use ibig::{modular::ModuloRing, ubig};
    /// let ring = ModuloRing::new(&ubig!(100));
    /// let x = ring.from(-1234);
    /// assert_eq!(x.residue(), ubig!(66));
    /// ```
    #[inline]
    pub fn residue(&self) -> UBig {
        match self.repr() {
            ModuloRepr::Small(self_small) => UBig::from_word(self_small.residue()),
            ModuloRepr::Large(self_large) => self_large.residue(),
        }
    }
}

impl ModuloSmallRaw {
    #[inline]
    pub(crate) fn residue(self, ring: &ModuloRingSmall) -> Word {
        debug_assert!(self.is_valid(ring));
        self.normalized() >> ring.shift()
    }

    #[inline]
    pub(crate) const fn from_word(word: Word, ring: &ModuloRingSmall) -> ModuloSmallRaw {
        let rem = if ring.shift() == 0 {
            ring.fast_div().div_rem_word(word).1
        } else {
            ring.fast_div().div_rem(extend_word(word) << ring.shift()).1
        };
        ModuloSmallRaw::from_normalized(rem)
    }

    fn from_large(words: &[Word], ring: &ModuloRingSmall) -> ModuloSmallRaw {
        let mut rem = div::fast_rem_by_normalized_word(words, ring.fast_div());
        if ring.shift() != 0 {
            rem = ring.fast_div().div_rem(extend_word(rem) << ring.shift()).1
        }
        ModuloSmallRaw::from_normalized(rem)
    }
}

impl ModuloSmall<'_> {
    #[inline]
    pub(crate) fn residue(&self) -> Word {
        self.raw().residue(self.ring())
    }
}

impl ModuloLarge<'_> {
    pub(crate) fn residue(&self) -> UBig {
        let words = self.normalized_value();
        let mut buffer = Buffer::allocate(words.len());
        buffer.extend(words);
        let low_bits = shift::shr_in_place(&mut buffer, self.ring().shift());
        assert!(low_bits == 0);
        buffer.into()
    }
}

/// Trait for types that can be converted into [Modulo] in a [ModuloRing].
pub trait IntoModulo {
    fn into_modulo(self, ring: &ModuloRing) -> Modulo;
}

impl IntoModulo for UBig {
    #[inline]
    fn into_modulo(self, ring: &ModuloRing) -> Modulo {
        match ring.repr() {
            ModuloRingRepr::Small(ring_small) => ModuloSmall::from_ubig(&self, ring_small).into(),
            ModuloRingRepr::Large(ring_large) => ModuloLarge::from_ubig(self, ring_large).into(),
        }
    }
}

impl IntoModulo for &UBig {
    #[inline]
    fn into_modulo(self, ring: &ModuloRing) -> Modulo {
        match ring.repr() {
            ModuloRingRepr::Small(ring_small) => ModuloSmall::from_ubig(self, ring_small).into(),
            ModuloRingRepr::Large(ring_large) => {
                ModuloLarge::from_ubig(self.clone(), ring_large).into()
            }
        }
    }
}

impl IntoModulo for IBig {
    #[inline]
    fn into_modulo(self, ring: &ModuloRing) -> Modulo {
        let (sign, mag) = self.into_sign_magnitude();
        let modulo = mag.into_modulo(ring);
        match sign {
            Positive => modulo,
            Negative => -modulo,
        }
    }
}

impl IntoModulo for &IBig {
    #[inline]
    fn into_modulo(self, ring: &ModuloRing) -> Modulo {
        let modulo = self.magnitude().into_modulo(ring);
        match self.sign() {
            Positive => modulo,
            Negative => -modulo,
        }
    }
}

impl<'a> ModuloSmall<'a> {
    #[inline]
    pub(crate) fn from_ubig(x: &UBig, ring: &'a ModuloRingSmall) -> ModuloSmall<'a> {
        let raw = match x.repr() {
            Repr::Small(word) => ModuloSmallRaw::from_word(*word, ring),
            Repr::Large(words) => ModuloSmallRaw::from_large(words, ring),
        };
        ModuloSmall::new(raw, ring)
    }
}

impl<'a> ModuloLarge<'a> {
    pub(crate) fn from_ubig(mut x: UBig, ring: &'a ModuloRingLarge) -> ModuloLarge<'a> {
        x <<= ring.shift() as usize;
        let modulus = ring.normalized_modulus();
        let mut vec = Vec::with_capacity(modulus.len());
        match x.into_repr() {
            Repr::Small(word) => vec.push(word),
            Repr::Large(mut words) => {
                if words.len() < modulus.len() {
                    vec.extend(&*words);
                } else {
                    let mut allocation = MemoryAllocation::new(div::memory_requirement_exact(
                        words.len(),
                        modulus.len(),
                    ));
                    let mut memory = allocation.memory();
                    let _overflow = div::div_rem_in_place(
                        &mut words,
                        modulus,
                        ring.fast_div_top(),
                        &mut memory,
                    );
                    vec.extend(&words[..modulus.len()]);
                }
            }
        }
        vec.extend(iter::repeat_n(0, modulus.len() - vec.len()));
        ModuloLarge::new(vec, ring)
    }
}

/// Implement `IntoModulo` for unsigned primitives.
macro_rules! impl_into_modulo_for_unsigned {
    ($t:ty) => {
        impl IntoModulo for $t {
            #[inline]
            fn into_modulo(self, ring: &ModuloRing) -> Modulo<'_> {
                UBig::from(self).into_modulo(ring)
            }
        }
    };
}

/// Implement `IntoModulo` for signed primitives.
macro_rules! impl_into_modulo_for_signed {
    ($t:ty) => {
        impl IntoModulo for $t {
            #[inline]
            fn into_modulo(self, ring: &ModuloRing) -> Modulo<'_> {
                IBig::from(self).into_modulo(ring)
            }
        }
    };
}

impl_into_modulo_for_unsigned!(bool);
impl_into_modulo_for_unsigned!(u8);
impl_into_modulo_for_unsigned!(u16);
impl_into_modulo_for_unsigned!(u32);
impl_into_modulo_for_unsigned!(u64);
impl_into_modulo_for_unsigned!(u128);
impl_into_modulo_for_unsigned!(usize);
impl_into_modulo_for_signed!(i8);
impl_into_modulo_for_signed!(i16);
impl_into_modulo_for_signed!(i32);
impl_into_modulo_for_signed!(i64);
impl_into_modulo_for_signed!(i128);
impl_into_modulo_for_signed!(isize);
