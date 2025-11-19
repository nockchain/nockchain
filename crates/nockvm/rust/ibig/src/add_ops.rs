//! Addition and subtraction operators.

use core::mem;
use core::ops::{Add, AddAssign, Sub, SubAssign};

use crate::arch::word::Word;
use crate::buffer::Buffer;
use crate::ibig::IBig;
use crate::memory::Stack;
use crate::primitive::{PrimitiveSigned, PrimitiveUnsigned};
use crate::sign::Sign::*;
use crate::ubig::Repr::*;
use crate::ubig::UBig;
use crate::{add, helper_macros};

impl Add<UBig> for UBig {
    type Output = UBig;

    /// WARNING: This uses global allocator. Use add_stack instead to prevent memory leaks.
    #[inline]
    fn add(self, rhs: UBig) -> UBig {
        match (self.into_repr(), rhs.into_repr()) {
            (Small(word0), Small(word1)) => UBig::add_word(word0, word1),
            (Small(word0), Large(buffer1)) => UBig::add_large_word(buffer1, word0),
            (Large(buffer0), Small(word1)) => UBig::add_large_word(buffer0, word1),
            (Large(buffer0), Large(buffer1)) => {
                if buffer0.len() >= buffer1.len() {
                    UBig::add_large(buffer0, &buffer1)
                } else {
                    UBig::add_large(buffer1, &buffer0)
                }
            }
        }
    }
}

impl Add<&UBig> for UBig {
    type Output = UBig;

    /// WARNING: This uses global allocator. Use add_stack instead to prevent memory leaks.
    #[inline]
    fn add(self, rhs: &UBig) -> UBig {
        match (self.into_repr(), rhs.repr()) {
            (Small(word0), Small(word1)) => UBig::add_word(word0, *word1),
            (Small(word0), Large(buffer1)) => UBig::add_large_word(buffer1.clone(), word0),
            (Large(buffer0), Small(word1)) => UBig::add_large_word(buffer0, *word1),
            (Large(buffer0), Large(buffer1)) => UBig::add_large(buffer0, buffer1),
        }
    }
}

impl Add<UBig> for &UBig {
    type Output = UBig;

    /// WARNING: This uses global allocator. Use add_stack instead to prevent memory leaks.
    #[inline]
    fn add(self, rhs: UBig) -> UBig {
        rhs.add(self)
    }
}

impl Add<&UBig> for &UBig {
    type Output = UBig;

    /// WARNING: This uses global allocator. Use add_stack instead to prevent memory leaks.
    #[inline]
    fn add(self, rhs: &UBig) -> UBig {
        match (self.repr(), rhs.repr()) {
            (Small(word0), Small(word1)) => UBig::add_word(*word0, *word1),
            (Small(word0), Large(buffer1)) => UBig::add_large_word(buffer1.clone(), *word0),
            (Large(buffer0), Small(word1)) => UBig::add_large_word(buffer0.clone(), *word1),
            (Large(buffer0), Large(buffer1)) => {
                if buffer0.len() >= buffer1.len() {
                    UBig::add_large(buffer0.clone(), buffer1)
                } else {
                    UBig::add_large(buffer1.clone(), buffer0)
                }
            }
        }
    }
}

impl AddAssign<UBig> for UBig {
    #[inline]
    fn add_assign(&mut self, rhs: UBig) {
        *self = mem::take(self) + rhs;
    }
}

impl AddAssign<&UBig> for UBig {
    #[inline]
    fn add_assign(&mut self, rhs: &UBig) {
        *self = mem::take(self) + rhs;
    }
}

impl Sub<UBig> for UBig {
    type Output = UBig;

    #[inline]
    fn sub(self, rhs: UBig) -> UBig {
        match (self.into_repr(), rhs.into_repr()) {
            (Small(word0), Small(word1)) => UBig::sub_word(word0, word1),
            (Small(_), Large(_)) => UBig::panic_negative(),
            (Large(buffer0), Small(word1)) => UBig::sub_large_word(buffer0, word1),
            (Large(buffer0), Large(buffer1)) => UBig::sub_large(buffer0, &buffer1),
        }
    }
}

impl Sub<&UBig> for UBig {
    type Output = UBig;

    #[inline]
    fn sub(self, rhs: &UBig) -> UBig {
        match (self.into_repr(), rhs.repr()) {
            (Small(word0), Small(word1)) => UBig::sub_word(word0, *word1),
            (Small(_), Large(_)) => UBig::panic_negative(),
            (Large(buffer0), Small(word1)) => UBig::sub_large_word(buffer0, *word1),
            (Large(buffer0), Large(buffer1)) => UBig::sub_large(buffer0, buffer1),
        }
    }
}

impl Sub<UBig> for &UBig {
    type Output = UBig;

    #[inline]
    fn sub(self, rhs: UBig) -> UBig {
        match (self.repr(), rhs.into_repr()) {
            (Small(word0), Small(word1)) => UBig::sub_word(*word0, word1),
            (Small(_), Large(_)) => UBig::panic_negative(),
            (Large(buffer0), Small(word1)) => UBig::sub_large_word(buffer0.clone(), word1),
            (Large(buffer0), Large(buffer1)) => UBig::sub_large_ref_val(buffer0, buffer1),
        }
    }
}

impl Sub<&UBig> for &UBig {
    type Output = UBig;

    #[inline]
    fn sub(self, rhs: &UBig) -> UBig {
        match (self.repr(), rhs.repr()) {
            (Small(word0), Small(word1)) => UBig::sub_word(*word0, *word1),
            (Small(_), Large(_)) => UBig::panic_negative(),
            (Large(buffer0), Small(word1)) => UBig::sub_large_word(buffer0.clone(), *word1),
            (Large(buffer0), Large(buffer1)) => UBig::sub_large(buffer0.clone(), buffer1),
        }
    }
}

impl SubAssign<UBig> for UBig {
    #[inline]
    fn sub_assign(&mut self, rhs: UBig) {
        *self = mem::take(self) - rhs;
    }
}

impl SubAssign<&UBig> for UBig {
    #[inline]
    fn sub_assign(&mut self, rhs: &UBig) {
        *self = mem::take(self) - rhs;
    }
}

impl Add<IBig> for IBig {
    type Output = IBig;

    #[inline]
    fn add(self, rhs: IBig) -> IBig {
        let (sign0, mag0) = self.into_sign_magnitude();
        let (sign1, mag1) = rhs.into_sign_magnitude();
        match (sign0, sign1) {
            (Positive, Positive) => IBig::from(mag0 + mag1),
            (Positive, Negative) => IBig::sub_ubig_val_val(mag0, mag1),
            (Negative, Positive) => IBig::sub_ubig_val_val(mag1, mag0),
            (Negative, Negative) => -IBig::from(mag0 + mag1),
        }
    }
}

impl Add<&IBig> for IBig {
    type Output = IBig;

    #[inline]
    fn add(self, rhs: &IBig) -> IBig {
        let (sign0, mag0) = self.into_sign_magnitude();
        let (sign1, mag1) = (rhs.sign(), rhs.magnitude());
        match (sign0, sign1) {
            (Positive, Positive) => IBig::from(mag0 + mag1),
            (Positive, Negative) => IBig::sub_ubig_val_ref(mag0, mag1),
            (Negative, Positive) => -IBig::sub_ubig_val_ref(mag0, mag1),
            (Negative, Negative) => -IBig::from(mag0 + mag1),
        }
    }
}

impl Add<IBig> for &IBig {
    type Output = IBig;

    #[inline]
    fn add(self, rhs: IBig) -> IBig {
        rhs.add(self)
    }
}

impl Add<&IBig> for &IBig {
    type Output = IBig;

    #[inline]
    fn add(self, rhs: &IBig) -> IBig {
        let (sign0, mag0) = (self.sign(), self.magnitude());
        let (sign1, mag1) = (rhs.sign(), rhs.magnitude());
        match (sign0, sign1) {
            (Positive, Positive) => IBig::from(mag0 + mag1),
            (Positive, Negative) => IBig::sub_ubig_ref_ref(mag0, mag1),
            (Negative, Positive) => IBig::sub_ubig_ref_ref(mag1, mag0),
            (Negative, Negative) => -IBig::from(mag0 + mag1),
        }
    }
}

impl AddAssign<IBig> for IBig {
    #[inline]
    fn add_assign(&mut self, rhs: IBig) {
        *self = mem::take(self) + rhs;
    }
}

impl AddAssign<&IBig> for IBig {
    #[inline]
    fn add_assign(&mut self, rhs: &IBig) {
        *self = mem::take(self) + rhs;
    }
}

impl Sub<IBig> for IBig {
    type Output = IBig;

    #[inline]
    fn sub(self, rhs: IBig) -> IBig {
        self + -rhs
    }
}

impl Sub<&IBig> for IBig {
    type Output = IBig;

    #[inline]
    fn sub(self, rhs: &IBig) -> IBig {
        -(-self + rhs)
    }
}

impl Sub<IBig> for &IBig {
    type Output = IBig;

    #[inline]
    fn sub(self, rhs: IBig) -> IBig {
        self + -rhs
    }
}

impl Sub<&IBig> for &IBig {
    type Output = IBig;

    #[inline]
    fn sub(self, rhs: &IBig) -> IBig {
        let (sign0, mag0) = (self.sign(), self.magnitude());
        let (sign1, mag1) = (rhs.sign(), rhs.magnitude());
        match (sign0, sign1) {
            (Positive, Positive) => IBig::sub_ubig_ref_ref(mag0, mag1),
            (Positive, Negative) => IBig::from(mag0 + mag1),
            (Negative, Positive) => -IBig::from(mag0 + mag1),
            (Negative, Negative) => IBig::sub_ubig_ref_ref(mag1, mag0),
        }
    }
}

impl SubAssign<IBig> for IBig {
    #[inline]
    fn sub_assign(&mut self, rhs: IBig) {
        *self = mem::take(self) - rhs;
    }
}

impl SubAssign<&IBig> for IBig {
    #[inline]
    fn sub_assign(&mut self, rhs: &IBig) {
        *self = mem::take(self) - rhs;
    }
}

macro_rules! impl_add_ubig_unsigned {
    ($t:ty) => {
        impl Add<$t> for UBig {
            type Output = UBig;

            #[inline]
            fn add(self, rhs: $t) -> UBig {
                self.add_unsigned(rhs)
            }
        }

        impl Add<$t> for &UBig {
            type Output = UBig;

            #[inline]
            fn add(self, rhs: $t) -> UBig {
                self.add_ref_unsigned(rhs)
            }
        }

        helper_macros::forward_binop_second_arg_by_value!(impl Add<$t> for UBig, add);
        helper_macros::forward_binop_swap_args!(impl Add<UBig> for $t, add);

        impl AddAssign<$t> for UBig {
            #[inline]
            fn add_assign(&mut self, rhs: $t) {
                self.add_assign_unsigned(rhs)
            }
        }

        helper_macros::forward_binop_assign_arg_by_value!(impl AddAssign<$t> for UBig, add_assign);

        impl Sub<$t> for UBig {
            type Output = UBig;

            #[inline]
            fn sub(self, rhs: $t) -> UBig {
                self.sub_unsigned(rhs)
            }
        }

        impl Sub<$t> for &UBig {
            type Output = UBig;

            #[inline]
            fn sub(self, rhs: $t) -> UBig {
                self.sub_ref_unsigned(rhs)
            }
        }

        helper_macros::forward_binop_second_arg_by_value!(impl Sub<$t> for UBig, sub);

        impl SubAssign<$t> for UBig {
            #[inline]
            fn sub_assign(&mut self, rhs: $t) {
                self.sub_assign_unsigned(rhs)
            }
        }

        helper_macros::forward_binop_assign_arg_by_value!(impl SubAssign<$t> for UBig, sub_assign);
    };
}

impl_add_ubig_unsigned!(u8);
impl_add_ubig_unsigned!(u16);
impl_add_ubig_unsigned!(u32);
impl_add_ubig_unsigned!(u64);
impl_add_ubig_unsigned!(u128);
impl_add_ubig_unsigned!(usize);

macro_rules! impl_add_ubig_signed {
    ($t:ty) => {
        impl Add<$t> for UBig {
            type Output = UBig;

            #[inline]
            fn add(self, rhs: $t) -> UBig {
                self.add_signed(rhs)
            }
        }

        impl Add<$t> for &UBig {
            type Output = UBig;

            #[inline]
            fn add(self, rhs: $t) -> UBig {
                self.add_ref_signed(rhs)
            }
        }

        helper_macros::forward_binop_second_arg_by_value!(impl Add<$t> for UBig, add);
        helper_macros::forward_binop_swap_args!(impl Add<UBig> for $t, add);

        impl AddAssign<$t> for UBig {
            #[inline]
            fn add_assign(&mut self, rhs: $t) {
                self.add_assign_signed(rhs)
            }
        }

        helper_macros::forward_binop_assign_arg_by_value!(impl AddAssign<$t> for UBig, add_assign);

        impl Sub<$t> for UBig {
            type Output = UBig;

            #[inline]
            fn sub(self, rhs: $t) -> UBig {
                self.sub_signed(rhs)
            }
        }

        impl Sub<$t> for &UBig {
            type Output = UBig;

            #[inline]
            fn sub(self, rhs: $t) -> UBig {
                self.sub_ref_signed(rhs)
            }
        }

        helper_macros::forward_binop_second_arg_by_value!(impl Sub<$t> for UBig, sub);

        impl SubAssign<$t> for UBig {
            #[inline]
            fn sub_assign(&mut self, rhs: $t) {
                self.sub_assign_signed(rhs)
            }
        }

        helper_macros::forward_binop_assign_arg_by_value!(impl SubAssign<$t> for UBig, sub_assign);
    };
}

impl_add_ubig_signed!(i8);
impl_add_ubig_signed!(i16);
impl_add_ubig_signed!(i32);
impl_add_ubig_signed!(i64);
impl_add_ubig_signed!(i128);
impl_add_ubig_signed!(isize);

macro_rules! impl_add_ibig_primitive {
    ($t:ty) => {
        impl Add<$t> for IBig {
            type Output = IBig;

            #[inline]
            fn add(self, rhs: $t) -> IBig {
                self.add_primitive(rhs)
            }
        }

        impl Add<$t> for &IBig {
            type Output = IBig;

            #[inline]
            fn add(self, rhs: $t) -> IBig {
                self.add_ref_primitive(rhs)
            }
        }

        helper_macros::forward_binop_second_arg_by_value!(impl Add<$t> for IBig, add);
        helper_macros::forward_binop_swap_args!(impl Add<IBig> for $t, add);

        impl AddAssign<$t> for IBig {
            #[inline]
            fn add_assign(&mut self, rhs: $t) {
                self.add_assign_primitive(rhs)
            }
        }

        helper_macros::forward_binop_assign_arg_by_value!(impl AddAssign<$t> for IBig, add_assign);

        impl Sub<$t> for IBig {
            type Output = IBig;

            #[inline]
            fn sub(self, rhs: $t) -> IBig {
                self.sub_primitive(rhs)
            }
        }

        impl Sub<$t> for &IBig {
            type Output = IBig;

            #[inline]
            fn sub(self, rhs: $t) -> IBig {
                self.sub_ref_primitive(rhs)
            }
        }

        impl Sub<IBig> for $t {
            type Output = IBig;

            #[inline]
            fn sub(self, rhs: IBig) -> IBig {
                rhs.sub_from_primitive(self)
            }
        }

        impl Sub<&IBig> for $t {
            type Output = IBig;

            #[inline]
            fn sub(self, rhs: &IBig) -> IBig {
                rhs.sub_ref_from_primitive(self)
            }
        }

        helper_macros::forward_binop_second_arg_by_value!(impl Sub<$t> for IBig, sub);
        helper_macros::forward_binop_first_arg_by_value!(impl Sub<IBig> for $t, sub);

        impl SubAssign<$t> for IBig {
            #[inline]
            fn sub_assign(&mut self, rhs: $t) {
                self.sub_assign_primitive(rhs)
            }
        }

        helper_macros::forward_binop_assign_arg_by_value!(impl SubAssign<$t> for IBig, sub_assign);
    };
}

impl_add_ibig_primitive!(u8);
impl_add_ibig_primitive!(u16);
impl_add_ibig_primitive!(u32);
impl_add_ibig_primitive!(u64);
impl_add_ibig_primitive!(u128);
impl_add_ibig_primitive!(usize);
impl_add_ibig_primitive!(i8);
impl_add_ibig_primitive!(i16);
impl_add_ibig_primitive!(i32);
impl_add_ibig_primitive!(i64);
impl_add_ibig_primitive!(i128);
impl_add_ibig_primitive!(isize);

impl UBig {
    // Practially, addition will always be in-place if used with brand-new UBigs because they're
    // given at least 2 words of extra capacity.  However, this supports UBigs which have already
    // been expanded through other operations.
    #[inline]
    pub fn add_stack<S: Stack>(stack: &mut S, lhs: UBig, rhs: UBig) -> UBig {
        match (lhs.into_repr(), rhs.into_repr()) {
            (Small(word0), Small(word1)) => UBig::add_word_stack(stack, word0, word1),
            (Small(word0), Large(buffer1)) => UBig::add_large_word_stack(stack, buffer1, word0),
            (Large(buffer0), Small(word1)) => UBig::add_large_word_stack(stack, buffer0, word1),
            (Large(buffer0), Large(buffer1)) => {
                if buffer0.len() >= buffer1.len() {
                    UBig::add_large_stack(stack, buffer0, &buffer1)
                } else {
                    UBig::add_large_stack(stack, buffer1, &buffer0)
                }
            }
        }
    }

    /// Add two `Word`s.
    #[inline]
    fn add_word_stack<S: Stack>(stack: &mut S, a: Word, b: Word) -> UBig {
        let (res, overflow) = a.overflowing_add(b);

        if overflow {
            let mut buffer = Buffer::allocate_stack(stack, 2);
            buffer.push(res);
            buffer.push(1);
            buffer.into()
        } else {
            UBig::from_word(res)
        }
    }

    /// Add a large number to a `Word`.
    fn add_large_word_stack<S: Stack>(stack: &mut S, mut buffer: Buffer, rhs: Word) -> UBig {
        debug_assert!(buffer.len() >= 2);
        if add::add_word_in_place(&mut buffer, rhs) {
            buffer.push_may_reallocate_stack(stack, 1);
        }
        buffer.into()
    }

    /// Add two large numbers.
    fn add_large_stack<S: Stack>(stack: &mut S, mut buffer: Buffer, rhs: &[Word]) -> UBig {
        let n = buffer.len().min(rhs.len());
        let overflow = add::add_same_len_in_place(&mut buffer[..n], &rhs[..n]);
        if rhs.len() > n {
            buffer.ensure_capacity_stack(stack, rhs.len());
            buffer.extend(&rhs[n..]);
        }
        if overflow && add::add_one_in_place(&mut buffer[n..]) {
            buffer.push_may_reallocate_stack(stack, 1);
        }
        buffer.into()
    }

    /// Add two `Word`s.
    #[inline]
    fn add_word(a: Word, b: Word) -> UBig {
        let (res, overflow) = a.overflowing_add(b);
        if overflow {
            let mut buffer = Buffer::allocate(2);
            buffer.push(res);
            buffer.push(1);
            buffer.into()
        } else {
            UBig::from_word(res)
        }
    }

    /// Add a large number to a `Word`.
    fn add_large_word(mut buffer: Buffer, rhs: Word) -> UBig {
        debug_assert!(buffer.len() >= 2);
        if add::add_word_in_place(&mut buffer, rhs) {
            buffer.push_may_reallocate(1);
        }
        buffer.into()
    }

    /// Add two large numbers.
    fn add_large(mut buffer: Buffer, rhs: &[Word]) -> UBig {
        let n = buffer.len().min(rhs.len());
        let overflow = add::add_same_len_in_place(&mut buffer[..n], &rhs[..n]);
        if rhs.len() > n {
            buffer.ensure_capacity(rhs.len());
            buffer.extend(&rhs[n..]);
        }
        if overflow && add::add_one_in_place(&mut buffer[n..]) {
            buffer.push_may_reallocate(1);
        }
        buffer.into()
    }

    // Subtraction is always in-place
    #[inline]
    pub fn sub_stack<S: Stack>(_stack: &mut S, lhs: UBig, rhs: UBig) -> UBig {
        lhs - rhs
    }

    /// Subtract two `Word`s.
    #[inline]
    fn sub_word(a: Word, b: Word) -> UBig {
        match a.checked_sub(b) {
            Some(res) => UBig::from_word(res),
            None => UBig::panic_negative(),
        }
    }

    fn sub_large_word(mut lhs: Buffer, rhs: Word) -> UBig {
        let overflow = add::sub_word_in_place(&mut lhs, rhs);
        assert!(!overflow);
        lhs.into()
    }

    fn sub_large(mut lhs: Buffer, rhs: &[Word]) -> UBig {
        if lhs.len() < rhs.len() || add::sub_in_place(&mut lhs, rhs) {
            UBig::panic_negative();
        }
        lhs.into()
    }

    fn sub_large_ref_val(lhs: &[Word], mut rhs: Buffer) -> UBig {
        let n = rhs.len();
        if lhs.len() < n {
            UBig::panic_negative();
        }
        let borrow = add::sub_same_len_in_place_swap(&lhs[..n], &mut rhs);
        rhs.ensure_capacity(lhs.len());
        rhs.extend(&lhs[n..]);
        if borrow && add::sub_one_in_place(&mut rhs[n..]) {
            UBig::panic_negative();
        }
        rhs.into()
    }

    #[inline]
    fn add_unsigned<T: PrimitiveUnsigned>(self, rhs: T) -> UBig {
        self + UBig::from_unsigned(rhs)
    }

    #[inline]
    fn add_ref_unsigned<T: PrimitiveUnsigned>(&self, rhs: T) -> UBig {
        self + UBig::from_unsigned(rhs)
    }

    #[inline]
    fn add_assign_unsigned<T: PrimitiveUnsigned>(&mut self, rhs: T) {
        *self += UBig::from_unsigned(rhs)
    }

    #[inline]
    fn sub_unsigned<T: PrimitiveUnsigned>(self, rhs: T) -> UBig {
        self - UBig::from_unsigned(rhs)
    }

    #[inline]
    fn sub_ref_unsigned<T: PrimitiveUnsigned>(&self, rhs: T) -> UBig {
        self - UBig::from_unsigned(rhs)
    }

    #[inline]
    fn sub_assign_unsigned<T: PrimitiveUnsigned>(&mut self, rhs: T) {
        *self -= UBig::from_unsigned(rhs)
    }

    #[inline]
    fn add_signed<T: PrimitiveSigned>(self, rhs: T) -> UBig {
        UBig::from_ibig_panic_on_overflow(IBig::from(self) + IBig::from_signed(rhs))
    }

    #[inline]
    fn add_ref_signed<T: PrimitiveSigned>(&self, rhs: T) -> UBig {
        UBig::from_ibig_panic_on_overflow(IBig::from(self) + IBig::from_signed(rhs))
    }

    #[inline]
    fn add_assign_signed<T: PrimitiveSigned>(&mut self, rhs: T) {
        *self = mem::take(self).add_signed(rhs)
    }

    #[inline]
    fn sub_signed<T: PrimitiveSigned>(self, rhs: T) -> UBig {
        UBig::from_ibig_panic_on_overflow(IBig::from(self) - IBig::from_signed(rhs))
    }

    #[inline]
    fn sub_ref_signed<T: PrimitiveSigned>(&self, rhs: T) -> UBig {
        UBig::from_ibig_panic_on_overflow(IBig::from(self) - IBig::from_signed(rhs))
    }

    #[inline]
    fn sub_assign_signed<T: PrimitiveSigned>(&mut self, rhs: T) {
        *self = mem::take(self).sub_signed(rhs)
    }
}

impl IBig {
    #[inline]
    fn sub_ubig_val_val(lhs: UBig, rhs: UBig) -> IBig {
        match (lhs.into_repr(), rhs.into_repr()) {
            (Small(word0), Small(word1)) => IBig::sub_word_word(word0, word1),
            (Small(word0), Large(buffer1)) => -IBig::sub_large_word(buffer1, word0),
            (Large(buffer0), Small(word1)) => IBig::sub_large_word(buffer0, word1),
            (Large(buffer0), Large(buffer1)) => {
                if buffer0.len() >= buffer1.len() {
                    IBig::sub_large(buffer0, &buffer1)
                } else {
                    -IBig::sub_large(buffer1, &buffer0)
                }
            }
        }
    }

    #[inline]
    fn sub_ubig_val_ref(lhs: UBig, rhs: &UBig) -> IBig {
        match (lhs.into_repr(), rhs.repr()) {
            (Small(word0), Small(word1)) => IBig::sub_word_word(word0, *word1),
            (Small(word0), Large(buffer1)) => -IBig::sub_large_word(buffer1.clone(), word0),
            (Large(buffer0), Small(word1)) => IBig::sub_large_word(buffer0, *word1),
            (Large(buffer0), Large(buffer1)) => IBig::sub_large(buffer0, buffer1),
        }
    }

    #[inline]
    fn sub_ubig_ref_ref(lhs: &UBig, rhs: &UBig) -> IBig {
        match (lhs.repr(), rhs.repr()) {
            (Small(word0), Small(word1)) => IBig::sub_word_word(*word0, *word1),
            (Small(word0), Large(buffer1)) => -IBig::sub_large_word(buffer1.clone(), *word0),
            (Large(buffer0), Small(word1)) => IBig::sub_large_word(buffer0.clone(), *word1),
            (Large(buffer0), Large(buffer1)) => {
                if buffer0.len() >= buffer1.len() {
                    IBig::sub_large(buffer0.clone(), buffer1)
                } else {
                    -IBig::sub_large(buffer1.clone(), buffer0)
                }
            }
        }
    }

    #[inline]
    fn sub_word_word(lhs: Word, rhs: Word) -> IBig {
        let (val, overflow) = lhs.overflowing_sub(rhs);
        if !overflow {
            IBig::from(val)
        } else {
            -IBig::from(val.wrapping_neg())
        }
    }

    fn sub_large_word(lhs: Buffer, rhs: Word) -> IBig {
        UBig::sub_large_word(lhs, rhs).into()
    }

    fn sub_large(mut lhs: Buffer, rhs: &[Word]) -> IBig {
        if lhs.len() >= rhs.len() {
            let sign = add::sub_in_place_with_sign(&mut lhs, rhs);
            IBig::from_sign_magnitude(sign, lhs.into())
        } else {
            let res = UBig::sub_large_ref_val(rhs, lhs);
            IBig::from_sign_magnitude(Negative, res)
        }
    }

    #[inline]
    fn add_primitive<T>(self, rhs: T) -> IBig
    where
        IBig: From<T>,
    {
        self + IBig::from(rhs)
    }

    #[inline]
    fn add_ref_primitive<T>(&self, rhs: T) -> IBig
    where
        IBig: From<T>,
    {
        self + IBig::from(rhs)
    }

    #[inline]
    fn add_assign_primitive<T>(&mut self, rhs: T)
    where
        IBig: From<T>,
    {
        *self += IBig::from(rhs)
    }

    #[inline]
    fn sub_primitive<T>(self, rhs: T) -> IBig
    where
        IBig: From<T>,
    {
        self - IBig::from(rhs)
    }

    #[inline]
    fn sub_ref_primitive<T>(&self, rhs: T) -> IBig
    where
        IBig: From<T>,
    {
        self - IBig::from(rhs)
    }

    #[inline]
    fn sub_assign_primitive<T>(&mut self, rhs: T)
    where
        IBig: From<T>,
    {
        *self -= IBig::from(rhs)
    }

    #[inline]
    fn sub_from_primitive<T>(self, rhs: T) -> IBig
    where
        IBig: From<T>,
    {
        IBig::from(rhs) - self
    }

    #[inline]
    fn sub_ref_from_primitive<T>(&self, rhs: T) -> IBig
    where
        IBig: From<T>,
    {
        IBig::from(rhs) - self
    }
}
