//! Bit shift operators.

use core::mem;
use core::ops::{Shl, ShlAssign, Shr, ShrAssign};

use crate::arch::word::Word;
use crate::buffer::Buffer;
use crate::ibig::IBig;
use crate::memory::Stack;
use crate::primitive::{double_word, extend_word, split_double_word, WORD_BITS_USIZE};
use crate::shift;
use crate::sign::Sign::*;
use crate::ubig::Repr::*;
use crate::ubig::UBig;

macro_rules! impl_shifts {
    ($t:ty) => {
        impl Shl<&usize> for $t {
            type Output = $t;

            #[inline]
            fn shl(self, rhs: &usize) -> $t {
                self.shl(*rhs)
            }
        }

        impl Shl<&usize> for &$t {
            type Output = $t;

            #[inline]
            fn shl(self, rhs: &usize) -> $t {
                self.shl(*rhs)
            }
        }

        impl ShlAssign<usize> for $t {
            #[inline]
            fn shl_assign(&mut self, rhs: usize) {
                *self = mem::take(self) << rhs;
            }
        }

        impl ShlAssign<&usize> for $t {
            #[inline]
            fn shl_assign(&mut self, rhs: &usize) {
                *self = mem::take(self) << rhs;
            }
        }

        impl Shr<&usize> for $t {
            type Output = $t;

            #[inline]
            fn shr(self, rhs: &usize) -> $t {
                self.shr(*rhs)
            }
        }

        impl Shr<&usize> for &$t {
            type Output = $t;

            #[inline]
            fn shr(self, rhs: &usize) -> $t {
                self.shr(*rhs)
            }
        }

        impl ShrAssign<usize> for $t {
            #[inline]
            fn shr_assign(&mut self, rhs: usize) {
                *self = mem::take(self).shr(rhs);
            }
        }

        impl ShrAssign<&usize> for $t {
            #[inline]
            fn shr_assign(&mut self, rhs: &usize) {
                *self = mem::take(self).shr(rhs);
            }
        }
    };
}

impl_shifts!(UBig);
impl_shifts!(IBig);

impl Shl<usize> for UBig {
    type Output = UBig;

    /// WARNING: This uses global allocator. Use shl_stack or shl_ref_stack instead to prevent memory leaks.
    #[inline]
    fn shl(self, rhs: usize) -> UBig {
        match self.into_repr() {
            Small(0) => UBig::from_word(0),
            Small(word) => UBig::shl_word(word, rhs),
            Large(buffer) => UBig::shl_large(buffer, rhs),
        }
    }
}

impl Shl<usize> for &UBig {
    type Output = UBig;

    /// WARNING: This uses global allocator. Use shl_stack or shl_ref_stack instead to prevent memory leaks.
    #[inline]
    fn shl(self, rhs: usize) -> UBig {
        match self.repr() {
            Small(0) => UBig::from_word(0),
            Small(word) => UBig::shl_word(*word, rhs),
            Large(buffer) => UBig::shl_ref_large(buffer, rhs),
        }
    }
}

impl Shr<usize> for UBig {
    type Output = UBig;

    #[inline]
    fn shr(self, rhs: usize) -> UBig {
        match self.into_repr() {
            Small(word) => UBig::shr_word(word, rhs),
            Large(buffer) => UBig::shr_large(buffer, rhs),
        }
    }
}

impl Shr<usize> for &UBig {
    type Output = UBig;

    #[inline]
    fn shr(self, rhs: usize) -> UBig {
        match self.repr() {
            Small(word) => UBig::shr_word(*word, rhs),
            Large(buffer) => UBig::shr_large_ref(buffer, rhs),
        }
    }
}

impl Shl<usize> for IBig {
    type Output = IBig;

    #[inline]
    fn shl(self, rhs: usize) -> IBig {
        let (sign, mag) = self.into_sign_magnitude();
        IBig::from_sign_magnitude(sign, mag.shl(rhs))
    }
}

impl Shl<usize> for &IBig {
    type Output = IBig;

    #[inline]
    fn shl(self, rhs: usize) -> IBig {
        let (sign, mag) = (self.sign(), self.magnitude());
        IBig::from_sign_magnitude(sign, mag.shl(rhs))
    }
}

impl Shr<usize> for IBig {
    type Output = IBig;

    #[inline]
    fn shr(self, rhs: usize) -> IBig {
        let (sign, mag) = self.into_sign_magnitude();
        match sign {
            Positive => IBig::from(mag.shr(rhs)),
            Negative => {
                let b = mag.are_low_bits_nonzero(rhs);
                -IBig::from(mag.shr(rhs)) - IBig::from(b)
            }
        }
    }
}

impl Shr<usize> for &IBig {
    type Output = IBig;

    #[inline]
    fn shr(self, rhs: usize) -> IBig {
        let (sign, mag) = (self.sign(), self.magnitude());
        match sign {
            Positive => IBig::from(mag.shr(rhs)),
            Negative => {
                let b = mag.are_low_bits_nonzero(rhs);
                -IBig::from(mag.shr(rhs)) - IBig::from(b)
            }
        }
    }
}

impl UBig {
    /// Shift left with stack allocator
    pub fn shl_stack<S: Stack>(stack: &mut S, value: UBig, rhs: usize) -> UBig {
        match value.into_repr() {
            Small(0) => UBig::from_word(0),
            Small(word) => UBig::shl_word_stack(stack, word, rhs),
            Large(buffer) => UBig::shl_large_stack(stack, buffer, rhs),
        }
    }

    /// Shift left reference with stack allocator
    pub fn shl_ref_stack<S: Stack>(stack: &mut S, value: &UBig, rhs: usize) -> UBig {
        match value.repr() {
            Small(0) => UBig::from_word(0),
            Small(word) => UBig::shl_word_stack(stack, *word, rhs),
            Large(buffer) => UBig::shl_ref_large_stack(stack, buffer, rhs),
        }
    }

    /// Shift right with stack allocator
    pub fn shr_stack<S: Stack>(stack: &mut S, value: UBig, rhs: usize) -> UBig {
        match value.into_repr() {
            Small(word) => UBig::shr_word(word, rhs),
            Large(buffer) => UBig::shr_large_stack(stack, buffer, rhs),
        }
    }

    /// Shift right reference with stack allocator
    pub fn shr_ref_stack<S: Stack>(stack: &mut S, value: &UBig, rhs: usize) -> UBig {
        match value.repr() {
            Small(word) => UBig::shr_word(*word, rhs),
            Large(buffer) => UBig::shr_large_ref_stack(stack, buffer, rhs),
        }
    }

    /// Shift left one non-zero `Word` by `rhs` bits with stack allocator.
    #[inline]
    fn shl_word_stack<S: Stack>(stack: &mut S, word: Word, rhs: usize) -> UBig {
        debug_assert!(word != 0);

        if rhs <= WORD_BITS_USIZE {
            UBig::from_unsigned_stack(stack, extend_word(word) << rhs)
        } else {
            UBig::shl_word_slow_stack(stack, word, rhs)
        }
    }

    /// Shift left one non-zero `Word` by `rhs` bits with stack allocator.
    fn shl_word_slow_stack<S: Stack>(stack: &mut S, word: Word, rhs: usize) -> UBig {
        let shift_words = rhs / WORD_BITS_USIZE;
        let shift_bits = (rhs % WORD_BITS_USIZE) as u32;
        let (lo, hi) = split_double_word(extend_word(word) << shift_bits);
        let mut buffer = Buffer::allocate_stack(stack, shift_words + 2);
        buffer.push_zeros(shift_words);
        buffer.push(lo);
        buffer.push(hi);
        buffer.into()
    }

    /// Shift left `buffer` by `rhs` bits with stack allocator.
    fn shl_large_stack<S: Stack>(stack: &mut S, mut buffer: Buffer, rhs: usize) -> UBig {
        let shift_words = rhs / WORD_BITS_USIZE;

        if buffer.capacity() < buffer.len() + shift_words + 1 {
            return UBig::shl_ref_large_stack(stack, &buffer, rhs);
        }

        let shift_bits = (rhs % WORD_BITS_USIZE) as u32;
        let carry = shift::shl_in_place(&mut buffer, shift_bits);
        buffer.push(carry);
        buffer.push_zeros_front(shift_words);
        buffer.into()
    }

    /// Shift left large number of words by `rhs` bits with stack allocator.
    fn shl_ref_large_stack<S: Stack>(stack: &mut S, words: &[Word], rhs: usize) -> UBig {
        let shift_words = rhs / WORD_BITS_USIZE;
        let shift_bits = (rhs % WORD_BITS_USIZE) as u32;

        let mut buffer = Buffer::allocate_stack(stack, shift_words + words.len() + 1);
        buffer.push_zeros(shift_words);
        buffer.extend(words);
        let carry = shift::shl_in_place(&mut buffer[shift_words..], shift_bits);
        buffer.push(carry);
        buffer.into()
    }

    /// Shift right `buffer` by `rhs` bits with stack allocator.
    fn shr_large_stack<S: Stack>(_stack: &mut S, mut buffer: Buffer, rhs: usize) -> UBig {
        let shift_words = rhs / WORD_BITS_USIZE;
        if shift_words >= buffer.len() {
            return UBig::from_word(0);
        }
        let shift_bits = (rhs % WORD_BITS_USIZE) as u32;
        buffer.erase_front(shift_words);
        shift::shr_in_place(&mut buffer, shift_bits);
        buffer.into()
    }

    /// Shift right large number of words by `rhs` bits with stack allocator.
    fn shr_large_ref_stack<S: Stack>(stack: &mut S, words: &[Word], rhs: usize) -> UBig {
        let shift_words = rhs / WORD_BITS_USIZE;
        let shift_bits = (rhs % WORD_BITS_USIZE) as u32;

        let words = &words[shift_words.min(words.len())..];

        match words {
            [] => UBig::from_word(0),
            &[w] => UBig::from_word(w >> shift_bits),
            &[lo, hi] => UBig::from_unsigned_stack(stack, double_word(lo, hi) >> shift_bits),
            _ => {
                let mut buffer = Buffer::allocate_stack(stack, words.len());
                buffer.extend(words);
                shift::shr_in_place(&mut buffer, shift_bits);
                buffer.into()
            }
        }
    }

    /// Shift left one non-zero `Word` by `rhs` bits.
    #[inline]
    fn shl_word(word: Word, rhs: usize) -> UBig {
        debug_assert!(word != 0);

        if rhs <= WORD_BITS_USIZE {
            UBig::from(extend_word(word) << rhs)
        } else {
            UBig::shl_word_slow(word, rhs)
        }
    }

    /// Shift left one non-zero `Word` by `rhs` bits.
    fn shl_word_slow(word: Word, rhs: usize) -> UBig {
        let shift_words = rhs / WORD_BITS_USIZE;
        let shift_bits = (rhs % WORD_BITS_USIZE) as u32;
        let (lo, hi) = split_double_word(extend_word(word) << shift_bits);
        let mut buffer = Buffer::allocate(shift_words + 2);
        buffer.push_zeros(shift_words);
        buffer.push(lo);
        buffer.push(hi);
        buffer.into()
    }

    /// Shift left `buffer` by `rhs` bits.
    fn shl_large(mut buffer: Buffer, rhs: usize) -> UBig {
        let shift_words = rhs / WORD_BITS_USIZE;

        if buffer.capacity() < buffer.len() + shift_words + 1 {
            return UBig::shl_ref_large(&buffer, rhs);
        }

        let shift_bits = (rhs % WORD_BITS_USIZE) as u32;
        let carry = shift::shl_in_place(&mut buffer, shift_bits);
        buffer.push(carry);
        buffer.push_zeros_front(shift_words);
        buffer.into()
    }

    /// Shift left large number of words by `rhs` bits.
    fn shl_ref_large(words: &[Word], rhs: usize) -> UBig {
        let shift_words = rhs / WORD_BITS_USIZE;
        let shift_bits = (rhs % WORD_BITS_USIZE) as u32;

        let mut buffer = Buffer::allocate(shift_words + words.len() + 1);
        buffer.push_zeros(shift_words);
        buffer.extend(words);
        let carry = shift::shl_in_place(&mut buffer[shift_words..], shift_bits);
        buffer.push(carry);
        buffer.into()
    }

    /// Shift right one `Word` by `rhs` bits.
    #[inline]
    fn shr_word(word: Word, rhs: usize) -> UBig {
        let word = if rhs < WORD_BITS_USIZE {
            word >> rhs
        } else {
            0
        };
        UBig::from_word(word)
    }

    /// Shift right `buffer` by `rhs` bits.
    fn shr_large(mut buffer: Buffer, rhs: usize) -> UBig {
        let shift_words = rhs / WORD_BITS_USIZE;
        if shift_words >= buffer.len() {
            return UBig::from_word(0);
        }
        let shift_bits = (rhs % WORD_BITS_USIZE) as u32;
        buffer.erase_front(shift_words);
        shift::shr_in_place(&mut buffer, shift_bits);
        buffer.into()
    }

    /// Shift right large number of words by `rhs` bits.
    fn shr_large_ref(words: &[Word], rhs: usize) -> UBig {
        let shift_words = rhs / WORD_BITS_USIZE;
        let shift_bits = (rhs % WORD_BITS_USIZE) as u32;

        let words = &words[shift_words.min(words.len())..];

        match words {
            [] => UBig::from_word(0),
            &[w] => UBig::from_word(w >> shift_bits),
            &[lo, hi] => UBig::from(double_word(lo, hi) >> shift_bits),
            _ => {
                let mut buffer = Buffer::allocate(words.len());
                buffer.extend(words);
                shift::shr_in_place(&mut buffer, shift_bits);
                buffer.into()
            }
        }
    }
}
