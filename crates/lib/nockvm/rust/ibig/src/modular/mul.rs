use alloc::alloc::Layout;
use core::ops::{Mul, MulAssign};

use crate::arch::word::Word;
use crate::assert::debug_assert_in_const_fn;
use crate::memory::{self, Memory, MemoryAllocation};
use crate::modular::modulo::{Modulo, ModuloLarge, ModuloRepr, ModuloSmall, ModuloSmallRaw};
use crate::modular::modulo_ring::{ModuloRingLarge, ModuloRingSmall};
use crate::primitive::extend_word;
use crate::sign::Sign::Positive;
use crate::{div, mul, shift};

impl<'a> Mul<Modulo<'a>> for Modulo<'a> {
    type Output = Modulo<'a>;

    #[inline]
    fn mul(self, rhs: Modulo<'a>) -> Modulo<'a> {
        self.mul(&rhs)
    }
}

impl<'a> Mul<&Modulo<'a>> for Modulo<'a> {
    type Output = Modulo<'a>;

    #[inline]
    fn mul(mut self, rhs: &Modulo<'a>) -> Modulo<'a> {
        self.mul_assign(rhs);
        self
    }
}

impl<'a> Mul<Modulo<'a>> for &Modulo<'a> {
    type Output = Modulo<'a>;

    #[inline]
    fn mul(self, rhs: Modulo<'a>) -> Modulo<'a> {
        rhs.mul(self)
    }
}

impl<'a> Mul<&Modulo<'a>> for &Modulo<'a> {
    type Output = Modulo<'a>;

    #[inline]
    fn mul(self, rhs: &Modulo<'a>) -> Modulo<'a> {
        self.clone().mul(rhs)
    }
}

impl<'a> MulAssign<Modulo<'a>> for Modulo<'a> {
    #[inline]
    fn mul_assign(&mut self, rhs: Modulo<'a>) {
        self.mul_assign(&rhs)
    }
}

impl<'a> MulAssign<&Modulo<'a>> for Modulo<'a> {
    #[inline]
    fn mul_assign(&mut self, rhs: &Modulo<'a>) {
        match (self.repr_mut(), rhs.repr()) {
            (ModuloRepr::Small(self_small), ModuloRepr::Small(rhs_small)) => {
                self_small.check_same_ring(rhs_small);
                self_small.mul_in_place(rhs_small);
            }
            (ModuloRepr::Large(self_large), ModuloRepr::Large(rhs_large)) => {
                self_large.check_same_ring(rhs_large);
                let memory_requirement = self_large.ring().mul_memory_requirement();
                let mut allocation = MemoryAllocation::new(memory_requirement);
                let mut memory = allocation.memory();
                self_large.mul_in_place(rhs_large, &mut memory);
            }
            _ => Modulo::panic_different_rings(),
        }
    }
}

impl ModuloSmallRaw {
    #[inline]
    pub(crate) const fn mul(self, other: ModuloSmallRaw, ring: &ModuloRingSmall) -> ModuloSmallRaw {
        debug_assert_in_const_fn!(self.is_valid(ring) && other.is_valid(ring));
        let a = self.normalized();
        let b = other.normalized();
        let product = extend_word(a >> ring.shift()) * extend_word(b);
        let (_, rem) = ring.fast_div().div_rem(product);
        ModuloSmallRaw::from_normalized(rem)
    }
}

impl<'a> ModuloSmall<'a> {
    /// self *= rhs
    #[inline]
    pub(crate) fn mul_in_place(&mut self, rhs: &ModuloSmall<'a>) {
        self.check_same_ring(rhs);
        self.set_raw(self.raw().mul(rhs.raw(), self.ring()));
    }
}

impl ModuloRingLarge {
    pub(crate) fn mul_memory_requirement(&self) -> Layout {
        let n = self.normalized_modulus().len();
        memory::add_layout(
            memory::array_layout::<Word>(2 * n),
            memory::max_layout(
                mul::memory_requirement_exact(2 * n, n),
                div::memory_requirement_exact(2 * n, n),
            ),
        )
    }

    /// Returns a * b allocated in memory.
    pub(crate) fn mul_normalized<'a>(
        &self,
        a: &[Word],
        b: &[Word],
        memory: &'a mut Memory,
    ) -> &'a [Word] {
        let modulus = self.normalized_modulus();
        let n = modulus.len();
        debug_assert!(a.len() == n && b.len() == n);

        let (product, mut memory) = memory.allocate_slice_fill::<Word>(2 * n, 0);
        let overflow = mul::add_signed_mul_same_len(product, Positive, a, b, &mut memory);
        assert_eq!(overflow, 0);
        shift::shr_in_place(product, self.shift());

        let _overflow = div::div_rem_in_place(product, modulus, self.fast_div_top(), &mut memory);
        &product[..n]
    }
}

impl<'a> ModuloLarge<'a> {
    /// self *= rhs
    pub(crate) fn mul_in_place(&mut self, rhs: &ModuloLarge<'a>, memory: &mut Memory) {
        self.mul_normalized_in_place(rhs.normalized_value(), memory);
    }

    /// self *= self
    pub(crate) fn square_in_place(&mut self, memory: &mut Memory) {
        self.modify_normalized_value(|words, ring| {
            words.copy_from_slice(ring.mul_normalized(words, words, memory));
        });
    }

    /// self *= rhs
    pub(crate) fn mul_normalized_in_place(
        &mut self,
        normalized_value: &[Word],
        memory: &mut Memory,
    ) {
        self.modify_normalized_value(|words, ring| {
            words.copy_from_slice(ring.mul_normalized(words, normalized_value, memory));
        });
    }
}
