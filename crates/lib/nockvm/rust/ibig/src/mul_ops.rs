//! Multiplication operators.

use core::mem;
use core::ops::{Mul, MulAssign};

use static_assertions::const_assert;

use crate::arch::word::Word;
use crate::buffer::Buffer;
use crate::ibig::IBig;
use crate::memory::{MemoryAllocation, Stack};
use crate::primitive::{extend_word, PrimitiveSigned, PrimitiveUnsigned};
use crate::sign::Sign::{self, *};
use crate::ubig::Repr::*;
use crate::ubig::UBig;
use crate::{helper_macros, mul};

impl Mul<UBig> for UBig {
    type Output = UBig;

    /// WARNING: This uses global allocator. Use mul_stack instead to prevent memory leaks.
    #[inline]
    fn mul(self, rhs: UBig) -> UBig {
        match (self.into_repr(), rhs.into_repr()) {
            (Small(word0), Small(word1)) => UBig::mul_word(word0, word1),
            (Small(word0), Large(buffer1)) => UBig::mul_large_word(buffer1, word0),
            (Large(buffer0), Small(word1)) => UBig::mul_large_word(buffer0, word1),
            (Large(buffer0), Large(buffer1)) => UBig::mul_large(&buffer0, &buffer1),
        }
    }
}

impl Mul<&UBig> for UBig {
    type Output = UBig;

    /// WARNING: This uses global allocator. Use mul_stack instead to prevent memory leaks.
    #[inline]
    fn mul(self, rhs: &UBig) -> UBig {
        match (self.into_repr(), rhs.repr()) {
            (Small(word0), Small(word1)) => UBig::mul_word(word0, *word1),
            (Small(word0), Large(buffer1)) => UBig::mul_large_word(buffer1.clone(), word0),
            (Large(buffer0), Small(word1)) => UBig::mul_large_word(buffer0, *word1),
            (Large(buffer0), Large(buffer1)) => UBig::mul_large(&buffer0, buffer1),
        }
    }
}

impl Mul<UBig> for &UBig {
    type Output = UBig;

    /// WARNING: This uses global allocator. Use mul_stack instead to prevent memory leaks.
    #[inline]
    fn mul(self, rhs: UBig) -> UBig {
        rhs.mul(self)
    }
}

impl Mul<&UBig> for &UBig {
    type Output = UBig;

    /// WARNING: This uses global allocator. Use mul_stack instead to prevent memory leaks.
    #[inline]
    fn mul(self, rhs: &UBig) -> UBig {
        match (self.repr(), rhs.repr()) {
            (Small(word0), Small(word1)) => UBig::mul_word(*word0, *word1),
            (Small(word0), Large(buffer1)) => UBig::mul_large_word(buffer1.clone(), *word0),
            (Large(buffer0), Small(word1)) => UBig::mul_large_word(buffer0.clone(), *word1),
            (Large(buffer0), Large(buffer1)) => UBig::mul_large(buffer0, buffer1),
        }
    }
}

impl MulAssign<UBig> for UBig {
    #[inline]
    fn mul_assign(&mut self, rhs: UBig) {
        *self = mem::take(self) * rhs;
    }
}

impl MulAssign<&UBig> for UBig {
    #[inline]
    fn mul_assign(&mut self, rhs: &UBig) {
        *self = mem::take(self) * rhs;
    }
}

impl Mul<IBig> for IBig {
    type Output = IBig;

    #[inline]
    fn mul(self, rhs: IBig) -> IBig {
        let (sign0, mag0) = self.into_sign_magnitude();
        let (sign1, mag1) = rhs.into_sign_magnitude();
        IBig::from_sign_magnitude(sign0 * sign1, mag0 * mag1)
    }
}

impl Mul<&IBig> for IBig {
    type Output = IBig;

    #[inline]
    fn mul(self, rhs: &IBig) -> IBig {
        let (sign0, mag0) = self.into_sign_magnitude();
        let (sign1, mag1) = (rhs.sign(), rhs.magnitude());
        IBig::from_sign_magnitude(sign0 * sign1, mag0 * mag1)
    }
}

impl Mul<IBig> for &IBig {
    type Output = IBig;

    #[inline]
    fn mul(self, rhs: IBig) -> IBig {
        rhs.mul(self)
    }
}

impl Mul<&IBig> for &IBig {
    type Output = IBig;

    #[inline]
    fn mul(self, rhs: &IBig) -> IBig {
        let (sign0, mag0) = (self.sign(), self.magnitude());
        let (sign1, mag1) = (rhs.sign(), rhs.magnitude());
        IBig::from_sign_magnitude(sign0 * sign1, mag0 * mag1)
    }
}

impl MulAssign<IBig> for IBig {
    #[inline]
    fn mul_assign(&mut self, rhs: IBig) {
        *self = mem::take(self) * rhs;
    }
}

impl MulAssign<&IBig> for IBig {
    #[inline]
    fn mul_assign(&mut self, rhs: &IBig) {
        *self = mem::take(self) * rhs;
    }
}

impl Mul<Sign> for Sign {
    type Output = Sign;

    #[inline]
    fn mul(self, rhs: Sign) -> Sign {
        match (self, rhs) {
            (Positive, Positive) => Positive,
            (Positive, Negative) => Negative,
            (Negative, Positive) => Negative,
            (Negative, Negative) => Positive,
        }
    }
}

impl MulAssign<Sign> for Sign {
    #[inline]
    fn mul_assign(&mut self, rhs: Sign) {
        *self = *self * rhs;
    }
}

impl Mul<IBig> for Sign {
    type Output = IBig;

    #[inline]
    fn mul(self, rhs: IBig) -> IBig {
        let (sign, mag) = rhs.into_sign_magnitude();
        IBig::from_sign_magnitude(self * sign, mag)
    }
}

macro_rules! impl_mul_ubig_unsigned {
    ($t:ty) => {
        impl Mul<$t> for UBig {
            type Output = UBig;

            #[inline]
            fn mul(self, rhs: $t) -> UBig {
                self.mul_unsigned(rhs)
            }
        }

        impl Mul<$t> for &UBig {
            type Output = UBig;

            #[inline]
            fn mul(self, rhs: $t) -> UBig {
                self.mul_ref_unsigned(rhs)
            }
        }

        helper_macros::forward_binop_second_arg_by_value!(impl Mul<$t> for UBig, mul);
        helper_macros::forward_binop_swap_args!(impl Mul<UBig> for $t, mul);

        impl MulAssign<$t> for UBig {
            #[inline]
            fn mul_assign(&mut self, rhs: $t) {
                self.mul_assign_unsigned(rhs)
            }
        }

        helper_macros::forward_binop_assign_arg_by_value!(impl MulAssign<$t> for UBig, mul_assign);
    };
}

impl_mul_ubig_unsigned!(u8);
impl_mul_ubig_unsigned!(u16);
impl_mul_ubig_unsigned!(u32);
impl_mul_ubig_unsigned!(u64);
impl_mul_ubig_unsigned!(u128);
impl_mul_ubig_unsigned!(usize);

macro_rules! impl_mul_ubig_signed {
    ($t:ty) => {
        impl Mul<$t> for UBig {
            type Output = UBig;

            #[inline]
            fn mul(self, rhs: $t) -> UBig {
                self.mul_signed(rhs)
            }
        }

        impl Mul<$t> for &UBig {
            type Output = UBig;

            #[inline]
            fn mul(self, rhs: $t) -> UBig {
                self.mul_ref_signed(rhs)
            }
        }

        helper_macros::forward_binop_second_arg_by_value!(impl Mul<$t> for UBig, mul);
        helper_macros::forward_binop_swap_args!(impl Mul<UBig> for $t, mul);

        impl MulAssign<$t> for UBig {
            #[inline]
            fn mul_assign(&mut self, rhs: $t) {
                self.mul_assign_signed(rhs)
            }
        }

        helper_macros::forward_binop_assign_arg_by_value!(impl MulAssign<$t> for UBig, mul_assign);
    };
}

impl_mul_ubig_signed!(i8);
impl_mul_ubig_signed!(i16);
impl_mul_ubig_signed!(i32);
impl_mul_ubig_signed!(i64);
impl_mul_ubig_signed!(i128);
impl_mul_ubig_signed!(isize);

macro_rules! impl_mul_ibig_primitive {
    ($t:ty) => {
        impl Mul<$t> for IBig {
            type Output = IBig;

            #[inline]
            fn mul(self, rhs: $t) -> IBig {
                self.mul_primitive(rhs)
            }
        }

        impl Mul<$t> for &IBig {
            type Output = IBig;

            #[inline]
            fn mul(self, rhs: $t) -> IBig {
                self.mul_ref_primitive(rhs)
            }
        }

        helper_macros::forward_binop_second_arg_by_value!(impl Mul<$t> for IBig, mul);
        helper_macros::forward_binop_swap_args!(impl Mul<IBig> for $t, mul);

        impl MulAssign<$t> for IBig {
            #[inline]
            fn mul_assign(&mut self, rhs: $t) {
                self.mul_assign_primitive(rhs)
            }
        }

        helper_macros::forward_binop_assign_arg_by_value!(impl MulAssign<$t> for IBig, mul_assign);
    };
}

impl_mul_ibig_primitive!(u8);
impl_mul_ibig_primitive!(u16);
impl_mul_ibig_primitive!(u32);
impl_mul_ibig_primitive!(u64);
impl_mul_ibig_primitive!(u128);
impl_mul_ibig_primitive!(usize);
impl_mul_ibig_primitive!(i8);
impl_mul_ibig_primitive!(i16);
impl_mul_ibig_primitive!(i32);
impl_mul_ibig_primitive!(i64);
impl_mul_ibig_primitive!(i128);
impl_mul_ibig_primitive!(isize);

impl UBig {
    #[inline]
    pub fn mul_stack<S: Stack>(stack: &mut S, lhs: UBig, rhs: UBig) -> UBig {
        match (lhs.into_repr(), rhs.into_repr()) {
            (Small(word0), Small(word1)) => UBig::mul_word_stack(stack, word0, word1),
            (Small(word0), Large(buffer1)) => UBig::mul_large_word_stack(stack, buffer1, word0),
            (Large(buffer0), Small(word1)) => UBig::mul_large_word_stack(stack, buffer0, word1),
            (Large(buffer0), Large(buffer1)) => UBig::mul_large_stack(stack, &buffer0, &buffer1),
        }
    }

    #[inline]
    fn mul_word_stack<S: Stack>(stack: &mut S, a: Word, b: Word) -> UBig {
        UBig::from_unsigned_stack(stack, extend_word(a) * extend_word(b))
    }

    fn mul_large_word_stack<S: Stack>(stack: &mut S, mut buffer: Buffer, a: Word) -> UBig {
        match a {
            0 => UBig::from_word(0),
            1 => buffer.into(),
            _ => {
                let carry = mul::mul_word_in_place(&mut buffer, a);
                if carry != 0 {
                    buffer.push_may_reallocate_stack(stack, carry);
                }
                buffer.into()
            }
        }
    }

    pub fn mul_large_stack<S: Stack>(stack: &mut S, lhs: &[Word], rhs: &[Word]) -> UBig {
        debug_assert!(lhs.len() >= 2 && rhs.len() >= 2);

        // This may be 1 too large.
        const_assert!(Buffer::MAX_CAPACITY - UBig::MAX_LEN >= 1);
        let res_len = lhs.len() + rhs.len();
        let mut buffer = Buffer::allocate_stack(stack, res_len);
        buffer.push_zeros(res_len);

        let mut allocation = MemoryAllocation::new_stack(
            stack,
            mul::memory_requirement_exact(res_len, lhs.len().min(rhs.len())),
        );
        let mut memory = allocation.memory();
        let overflow = mul::add_signed_mul(&mut buffer, Positive, lhs, rhs, &mut memory);
        assert!(overflow == 0);
        buffer.into()
    }

    /// Multiply two `Word`s.
    #[inline]
    fn mul_word(a: Word, b: Word) -> UBig {
        UBig::from(extend_word(a) * extend_word(b))
    }

    /// Multiply a large number by a `Word`.
    fn mul_large_word(mut buffer: Buffer, a: Word) -> UBig {
        match a {
            0 => UBig::from_word(0),
            1 => buffer.into(),
            _ => {
                let carry = mul::mul_word_in_place(&mut buffer, a);
                if carry != 0 {
                    buffer.push_may_reallocate(carry);
                }
                buffer.into()
            }
        }
    }

    /// Multiply two large numbers.
    fn mul_large(lhs: &[Word], rhs: &[Word]) -> UBig {
        debug_assert!(lhs.len() >= 2 && rhs.len() >= 2);

        // This may be 1 too large.
        const_assert!(Buffer::MAX_CAPACITY - UBig::MAX_LEN >= 1);
        let res_len = lhs.len() + rhs.len();
        let mut buffer = Buffer::allocate(res_len);
        buffer.push_zeros(res_len);

        let mut allocation = MemoryAllocation::new(mul::memory_requirement_exact(
            res_len,
            lhs.len().min(rhs.len()),
        ));
        let mut memory = allocation.memory();
        let overflow = mul::add_signed_mul(&mut buffer, Positive, lhs, rhs, &mut memory);
        assert!(overflow == 0);
        buffer.into()
    }

    #[inline]
    fn mul_unsigned<T: PrimitiveUnsigned>(self, rhs: T) -> UBig {
        self * UBig::from_unsigned(rhs)
    }

    #[inline]
    fn mul_ref_unsigned<T: PrimitiveUnsigned>(&self, rhs: T) -> UBig {
        self * UBig::from_unsigned(rhs)
    }

    #[inline]
    fn mul_assign_unsigned<T: PrimitiveUnsigned>(&mut self, rhs: T) {
        *self *= UBig::from_unsigned(rhs)
    }

    #[inline]
    fn mul_signed<T: PrimitiveSigned>(self, rhs: T) -> UBig {
        UBig::from_ibig_panic_on_overflow(IBig::from(self) * IBig::from_signed(rhs))
    }

    #[inline]
    fn mul_ref_signed<T: PrimitiveSigned>(&self, rhs: T) -> UBig {
        UBig::from_ibig_panic_on_overflow(IBig::from(self) * IBig::from_signed(rhs))
    }

    #[inline]
    fn mul_assign_signed<T: PrimitiveSigned>(&mut self, rhs: T) {
        *self = mem::take(self).mul_signed(rhs)
    }
}

impl IBig {
    #[inline]
    fn mul_primitive<T>(self, rhs: T) -> IBig
    where
        IBig: From<T>,
    {
        self * IBig::from(rhs)
    }

    #[inline]
    fn mul_ref_primitive<T>(&self, rhs: T) -> IBig
    where
        IBig: From<T>,
    {
        self * IBig::from(rhs)
    }

    #[inline]
    fn mul_assign_primitive<T>(&mut self, rhs: T)
    where
        IBig: From<T>,
    {
        *self *= IBig::from(rhs)
    }
}
