//! Bitwise operators.

use core::mem;
use core::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, BitXor, BitXorAssign, Not};

use crate::arch::word::Word;
use crate::buffer::Buffer;
use crate::ibig::IBig;
use crate::memory::Stack;
use crate::ops::{AndNot, NextPowerOfTwo, UnsignedAbs};
use crate::primitive::{double_word, PrimitiveSigned, PrimitiveUnsigned, WORD_BITS_USIZE};
use crate::sign::Sign::*;
use crate::ubig::Repr::*;
use crate::ubig::UBig;
use crate::{helper_macros, math};

impl UBig {
    /// Returns true if the `n`-th bit is set.
    ///
    /// # Examples
    ///
    /// ```
    /// # use ibig::ubig;
    /// assert_eq!(ubig!(0b10010).bit(1), true);
    /// assert_eq!(ubig!(0b10010).bit(3), false);
    /// assert_eq!(ubig!(0b10010).bit(100), false);
    /// ```
    #[inline]
    pub fn bit(&self, n: usize) -> bool {
        match self.repr() {
            Small(word) => n < WORD_BITS_USIZE && word & (1 << n) != 0,
            Large(buffer) => {
                let idx = n / WORD_BITS_USIZE;
                idx < buffer.len() && buffer[idx] & (1 << (n % WORD_BITS_USIZE)) != 0
            }
        }
    }

    /// Set the `n`-th bit.
    ///
    /// # Examples
    ///
    /// ```
    /// # use ibig::ubig;
    /// let mut a = ubig!(0b100);
    /// a.set_bit(0);
    /// assert_eq!(a, ubig!(0b101));
    /// a.set_bit(10);
    /// assert_eq!(a, ubig!(0b10000000101));
    /// ```
    #[inline]
    pub fn set_bit(&mut self, n: usize) {
        match mem::take(self).into_repr() {
            Small(word) => {
                if n < WORD_BITS_USIZE {
                    *self = UBig::from_word(word | (1 << n))
                } else {
                    *self = UBig::with_bit_word_slow(word, n)
                }
            }
            Large(buffer) => *self = UBig::with_bit_large(buffer, n),
        }
    }

    fn with_bit_word_slow(word: Word, n: usize) -> UBig {
        debug_assert!(n >= WORD_BITS_USIZE);
        let idx = n / WORD_BITS_USIZE;
        let mut buffer = Buffer::allocate(idx + 1);
        buffer.push(word);
        buffer.extend((1..idx).map(|_| 0));
        buffer.push(1 << (n % WORD_BITS_USIZE));
        buffer.into()
    }

    fn with_bit_large(mut buffer: Buffer, n: usize) -> UBig {
        let idx = n / WORD_BITS_USIZE;
        if idx < buffer.len() {
            buffer[idx] |= 1 << (n % WORD_BITS_USIZE);
        } else {
            buffer.ensure_capacity(idx + 1);
            buffer.push_zeros(idx - buffer.len());
            buffer.push(1 << (n % WORD_BITS_USIZE));
        }
        buffer.into()
    }

    /// Clear the `n`-th bit.
    ///
    /// # Examples
    ///
    /// ```
    /// # use ibig::ubig;
    /// let mut a = ubig!(0b101);
    /// a.clear_bit(0);
    /// assert_eq!(a, ubig!(0b100));
    /// ```
    #[inline]
    pub fn clear_bit(&mut self, n: usize) {
        match mem::take(self).into_repr() {
            Small(word) => {
                if n < WORD_BITS_USIZE {
                    *self = UBig::from_word(word & !(1 << n))
                }
            }
            Large(buffer) => *self = UBig::without_bit_large(buffer, n),
        }
    }

    fn without_bit_large(mut buffer: Buffer, n: usize) -> UBig {
        let idx = n / WORD_BITS_USIZE;
        if idx < buffer.len() {
            buffer[idx] &= !(1 << (n % WORD_BITS_USIZE));
        }
        buffer.into()
    }

    /// Returns the number of trailing zeros in the binary representation.
    ///
    /// In other words, it is the largest `n` such that 2 to the power of `n` divides the number.
    ///
    /// For 0, it returns `None`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use ibig::ubig;
    /// assert_eq!(ubig!(17).trailing_zeros(), Some(0));
    /// assert_eq!(ubig!(48).trailing_zeros(), Some(4));
    /// assert_eq!(ubig!(0b101000000).trailing_zeros(), Some(6));
    /// assert_eq!(ubig!(0).trailing_zeros(), None);
    /// ```
    #[inline]
    pub fn trailing_zeros(&self) -> Option<usize> {
        match self.repr() {
            Small(0) => None,
            Small(word) => Some(word.trailing_zeros() as usize),
            Large(buffer) => Some(UBig::trailing_zeros_large(buffer)),
        }
    }

    fn trailing_zeros_large(words: &[Word]) -> usize {
        debug_assert!(
            *words.last().unwrap_or_else(|| {
                panic!(
                    "Panicked at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            }) != 0
        );

        for (idx, word) in words.iter().enumerate() {
            if *word != 0 {
                return idx * WORD_BITS_USIZE + word.trailing_zeros() as usize;
            }
        }
        panic!("trailing_zeros_large(0)")
    }

    /// Bit length.
    ///
    /// The length of the binary representation of the number.
    ///
    /// For 0, the length is 0.
    ///
    /// For non-zero numbers it is:
    /// * `in_radix(2).to_string().len()`
    /// * the index of the top 1 bit plus one
    /// * the floor of the logarithm base 2 of the number plus one.
    ///
    ///
    ///
    /// # Examples
    ///
    /// ```
    /// # use ibig::ubig;
    /// assert_eq!(ubig!(17).bit_len(), 5);
    /// assert_eq!(ubig!(0b101000000).bit_len(), 9);
    /// assert_eq!(ubig!(0).bit_len(), 0);
    /// let x = ubig!(_0x90ffff3450897234);
    /// assert_eq!(x.bit_len(), x.in_radix(2).to_string().len());
    /// ```
    #[inline]
    pub fn bit_len(&self) -> usize {
        match self.repr() {
            Small(word) => math::bit_len(*word) as usize,
            Large(buffer) => {
                buffer.len() * WORD_BITS_USIZE
                    - buffer
                        .last()
                        .unwrap_or_else(|| {
                            panic!(
                                "Panicked at {}:{} (git sha: {})",
                                file!(),
                                line!(),
                                option_env!("GIT_SHA").unwrap_or("unknown")
                            )
                        })
                        .leading_zeros() as usize
            }
        }
    }

    /// True if the number is a power of 2.
    ///
    /// # Examples
    ///
    /// ```
    /// # use ibig::ubig;
    /// assert_eq!(ubig!(0).is_power_of_two(), false);
    /// assert_eq!(ubig!(8).is_power_of_two(), true);
    /// assert_eq!(ubig!(9).is_power_of_two(), false);
    /// ```
    #[inline]
    pub fn is_power_of_two(&self) -> bool {
        match self.repr() {
            Small(word) => word.is_power_of_two(),
            Large(buffer) => UBig::is_power_of_two_large(buffer),
        }
    }

    fn is_power_of_two_large(words: &[Word]) -> bool {
        debug_assert!(
            *words.last().unwrap_or_else(|| {
                panic!(
                    "Panicked at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            }) != 0
        );

        words[..words.len() - 1].iter().all(|x| *x == 0)
            && words
                .last()
                .unwrap_or_else(|| {
                    panic!(
                        "Panicked at {}:{} (git sha: {})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA").unwrap_or("unknown")
                    )
                })
                .is_power_of_two()
    }
}

impl IBig {
    /// Returns the number of trailing zeros in the two's complement binary representation.
    ///
    /// In other words, it is the largest `n` such that 2 to the power of `n` divides the number.
    ///
    /// For 0, it returns `None`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use ibig::ibig;
    /// assert_eq!(ibig!(17).trailing_zeros(), Some(0));
    /// assert_eq!(ibig!(-48).trailing_zeros(), Some(4));
    /// assert_eq!(ibig!(-0b101000000).trailing_zeros(), Some(6));
    /// assert_eq!(ibig!(0).trailing_zeros(), None);
    /// ```
    #[inline]
    pub fn trailing_zeros(&self) -> Option<usize> {
        self.magnitude().trailing_zeros()
    }
}

impl NextPowerOfTwo for UBig {
    type Output = UBig;

    #[inline]
    fn next_power_of_two(self) -> UBig {
        match self.into_repr() {
            Small(word) => match word.checked_next_power_of_two() {
                Some(p) => UBig::from_word(p),
                None => UBig::from(double_word(0, 1)),
            },
            Large(buffer) => UBig::next_power_of_two_large(buffer),
        }
    }
}

impl NextPowerOfTwo for &UBig {
    type Output = UBig;

    #[inline]
    fn next_power_of_two(self) -> UBig {
        self.clone().next_power_of_two()
    }
}

impl UBig {
    fn next_power_of_two_large(mut buffer: Buffer) -> UBig {
        debug_assert!(
            *buffer.last().unwrap_or_else(|| {
                panic!(
                    "Panicked at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            }) != 0
        );

        let n = buffer.len();
        let mut iter = buffer[..n - 1].iter_mut().skip_while(|x| **x == 0);

        let carry = match iter.next() {
            None => 0,
            Some(x) => {
                *x = 0;
                for x in iter {
                    *x = 0;
                }
                1
            }
        };

        let last = buffer.last_mut().unwrap_or_else(|| {
            panic!(
                "Panicked at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        });
        match last
            .checked_add(carry)
            .and_then(|x| x.checked_next_power_of_two())
        {
            Some(p) => *last = p,
            None => {
                *last = 0;
                buffer.ensure_capacity(n + 1);
                buffer.push(1);
            }
        }

        buffer.into()
    }
}

impl BitAnd<UBig> for UBig {
    type Output = UBig;

    #[inline]
    fn bitand(self, rhs: UBig) -> UBig {
        match (self.into_repr(), rhs.into_repr()) {
            (Small(word0), Small(word1)) => UBig::from_word(word0 & word1),
            (Small(word0), Large(buffer1)) => UBig::from_word(
                word0
                    & buffer1.first().unwrap_or_else(|| {
                        panic!(
                            "Panicked at {}:{} (git sha: {})",
                            file!(),
                            line!(),
                            option_env!("GIT_SHA").unwrap_or("unknown")
                        )
                    }),
            ),
            (Large(buffer0), Small(word1)) => UBig::from_word(
                buffer0.first().unwrap_or_else(|| {
                    panic!(
                        "Panicked at {}:{} (git sha: {})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA").unwrap_or("unknown")
                    )
                }) & word1,
            ),
            (Large(buffer0), Large(buffer1)) => {
                if buffer0.len() <= buffer1.len() {
                    UBig::bitand_large(buffer0, &buffer1)
                } else {
                    UBig::bitand_large(buffer1, &buffer0)
                }
            }
        }
    }
}

impl BitAnd<&UBig> for UBig {
    type Output = UBig;

    #[inline]
    fn bitand(self, rhs: &UBig) -> UBig {
        match (self.into_repr(), rhs.repr()) {
            (Small(word0), Small(word1)) => UBig::from_word(word0 & word1),
            (Small(word0), Large(buffer1)) => UBig::from_word(
                word0
                    & buffer1.first().unwrap_or_else(|| {
                        panic!(
                            "Panicked at {}:{} (git sha: {})",
                            file!(),
                            line!(),
                            option_env!("GIT_SHA").unwrap_or("unknown")
                        )
                    }),
            ),
            (Large(buffer0), Small(word1)) => UBig::from_word(
                buffer0.first().unwrap_or_else(|| {
                    panic!(
                        "Panicked at {}:{} (git sha: {})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA").unwrap_or("unknown")
                    )
                }) & word1,
            ),
            (Large(buffer0), Large(buffer1)) => UBig::bitand_large(buffer0, buffer1),
        }
    }
}

impl BitAnd<UBig> for &UBig {
    type Output = UBig;

    #[inline]
    fn bitand(self, rhs: UBig) -> UBig {
        rhs.bitand(self)
    }
}

impl BitAnd<&UBig> for &UBig {
    type Output = UBig;

    #[inline]
    fn bitand(self, rhs: &UBig) -> UBig {
        match (self.repr(), rhs.repr()) {
            (Small(word0), Small(word1)) => UBig::from_word(word0 & word1),
            (Small(word0), Large(buffer1)) => UBig::from_word(
                word0
                    & buffer1.first().unwrap_or_else(|| {
                        panic!(
                            "Panicked at {}:{} (git sha: {})",
                            file!(),
                            line!(),
                            option_env!("GIT_SHA").unwrap_or("unknown")
                        )
                    }),
            ),
            (Large(buffer0), Small(word1)) => UBig::from_word(
                buffer0.first().unwrap_or_else(|| {
                    panic!(
                        "Panicked at {}:{} (git sha: {})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA").unwrap_or("unknown")
                    )
                }) & word1,
            ),
            (Large(buffer0), Large(buffer1)) => {
                if buffer0.len() <= buffer1.len() {
                    UBig::bitand_large(buffer0.clone(), buffer1)
                } else {
                    UBig::bitand_large(buffer1.clone(), buffer0)
                }
            }
        }
    }
}

impl BitAndAssign<UBig> for UBig {
    #[inline]
    fn bitand_assign(&mut self, rhs: UBig) {
        *self = mem::take(self) & rhs;
    }
}

impl BitAndAssign<&UBig> for UBig {
    #[inline]
    fn bitand_assign(&mut self, rhs: &UBig) {
        *self = mem::take(self) & rhs;
    }
}

impl UBig {
    fn bitand_large(mut buffer: Buffer, rhs: &[Word]) -> UBig {
        if buffer.len() > rhs.len() {
            buffer.truncate(rhs.len());
        }
        for (x, y) in buffer.iter_mut().zip(rhs.iter()) {
            *x &= *y;
        }
        buffer.into()
    }
}

impl BitOr<UBig> for UBig {
    type Output = UBig;

    /// WARNING: This uses global allocator. Use bitor_stack or bitor_ref_stack instead to prevent memory leaks.
    #[inline]
    fn bitor(self, rhs: UBig) -> UBig {
        match (self.into_repr(), rhs.into_repr()) {
            (Small(word0), Small(word1)) => UBig::from_word(word0 | word1),
            (Small(word0), Large(buffer1)) => UBig::bitor_large_word(buffer1, word0),
            (Large(buffer0), Small(word1)) => UBig::bitor_large_word(buffer0, word1),
            (Large(buffer0), Large(buffer1)) => {
                if buffer0.len() >= buffer1.len() {
                    UBig::bitor_large(buffer0, &buffer1)
                } else {
                    UBig::bitor_large(buffer1, &buffer0)
                }
            }
        }
    }
}

impl BitOr<&UBig> for UBig {
    type Output = UBig;

    /// WARNING: This uses global allocator. Use bitor_stack or bitor_ref_stack instead to prevent memory leaks.
    #[inline]
    fn bitor(self, rhs: &UBig) -> UBig {
        match (self.into_repr(), rhs.repr()) {
            (Small(word0), Small(word1)) => UBig::from_word(word0 | word1),
            (Small(word0), Large(buffer1)) => UBig::bitor_large_word(buffer1.clone(), word0),
            (Large(buffer0), Small(word1)) => UBig::bitor_large_word(buffer0, *word1),
            (Large(buffer0), Large(buffer1)) => UBig::bitor_large(buffer0, buffer1),
        }
    }
}

impl BitOr<UBig> for &UBig {
    type Output = UBig;

    /// WARNING: This uses global allocator. Use bitor_stack or bitor_ref_stack instead to prevent memory leaks.
    #[inline]
    fn bitor(self, rhs: UBig) -> UBig {
        rhs.bitor(self)
    }
}

impl BitOr<&UBig> for &UBig {
    type Output = UBig;

    /// WARNING: This uses global allocator. Use bitor_stack or bitor_ref_stack instead to prevent memory leaks.
    #[inline]
    fn bitor(self, rhs: &UBig) -> UBig {
        match (self.repr(), rhs.repr()) {
            (Small(word0), Small(word1)) => UBig::from_word(word0 | word1),
            (Small(word0), Large(buffer1)) => UBig::bitor_large_word(buffer1.clone(), *word0),
            (Large(buffer0), Small(word1)) => UBig::bitor_large_word(buffer0.clone(), *word1),
            (Large(buffer0), Large(buffer1)) => {
                if buffer0.len() >= buffer1.len() {
                    UBig::bitor_large(buffer0.clone(), buffer1)
                } else {
                    UBig::bitor_large(buffer1.clone(), buffer0)
                }
            }
        }
    }
}

impl BitOrAssign<UBig> for UBig {
    #[inline]
    fn bitor_assign(&mut self, rhs: UBig) {
        *self = mem::take(self) | rhs;
    }
}

impl BitOrAssign<&UBig> for UBig {
    #[inline]
    fn bitor_assign(&mut self, rhs: &UBig) {
        *self = mem::take(self) | rhs;
    }
}

impl UBig {
    /// Bitwise OR with stack allocator.
    pub fn bitor_stack<S: Stack>(stack: &mut S, lhs: UBig, rhs: UBig) -> UBig {
        match (lhs.into_repr(), rhs.into_repr()) {
            (Small(word0), Small(word1)) => UBig::from_word(word0 | word1),
            (Small(word0), Large(buffer1)) => UBig::bitor_large_word_stack(stack, buffer1, word0),
            (Large(buffer0), Small(word1)) => UBig::bitor_large_word_stack(stack, buffer0, word1),
            (Large(buffer0), Large(buffer1)) => {
                if buffer0.len() >= buffer1.len() {
                    UBig::bitor_large_stack(stack, buffer0, &buffer1)
                } else {
                    UBig::bitor_large_stack(stack, buffer1, &buffer0)
                }
            }
        }
    }

    /// Bitwise OR reference with stack allocator.
    pub fn bitor_ref_stack<S: Stack>(stack: &mut S, lhs: &UBig, rhs: &UBig) -> UBig {
        match (lhs.repr(), rhs.repr()) {
            (Small(word0), Small(word1)) => UBig::from_word(word0 | word1),
            (Small(word0), Large(buffer1)) => {
                let mut new_buffer = Buffer::allocate_stack(stack, buffer1.len());
                new_buffer.clone_from(buffer1);
                UBig::bitor_large_word_stack(stack, new_buffer, *word0)
            }
            (Large(buffer0), Small(word1)) => {
                let mut new_buffer = Buffer::allocate_stack(stack, buffer0.len());
                new_buffer.clone_from(buffer0);
                UBig::bitor_large_word_stack(stack, new_buffer, *word1)
            }
            (Large(buffer0), Large(buffer1)) => {
                if buffer0.len() >= buffer1.len() {
                    let mut new_buffer = Buffer::allocate_stack(stack, buffer0.len());
                    new_buffer.clone_from(buffer0);
                    UBig::bitor_large_stack(stack, new_buffer, buffer1)
                } else {
                    let mut new_buffer = Buffer::allocate_stack(stack, buffer1.len());
                    new_buffer.clone_from(buffer1);
                    UBig::bitor_large_stack(stack, new_buffer, buffer0)
                }
            }
        }
    }

    fn bitor_large_word_stack<S: Stack>(_stack: &mut S, mut buffer: Buffer, rhs: Word) -> UBig {
        debug_assert!(buffer.len() >= 2);

        *buffer.first_mut().unwrap_or_else(|| {
            panic!(
                "Panicked at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        }) |= rhs;
        buffer.into()
    }

    fn bitor_large_stack<S: Stack>(stack: &mut S, mut buffer: Buffer, rhs: &[Word]) -> UBig {
        for (x, y) in buffer.iter_mut().zip(rhs.iter()) {
            *x |= *y;
        }
        if rhs.len() > buffer.len() {
            buffer.ensure_capacity_stack(stack, rhs.len());
            buffer.extend(&rhs[buffer.len()..]);
        }
        buffer.into()
    }

    fn bitor_large_word(mut buffer: Buffer, rhs: Word) -> UBig {
        debug_assert!(buffer.len() >= 2);

        *buffer.first_mut().unwrap_or_else(|| {
            panic!(
                "Panicked at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        }) |= rhs;
        buffer.into()
    }

    fn bitor_large(mut buffer: Buffer, rhs: &[Word]) -> UBig {
        for (x, y) in buffer.iter_mut().zip(rhs.iter()) {
            *x |= *y;
        }
        if rhs.len() > buffer.len() {
            buffer.ensure_capacity(rhs.len());
            buffer.extend(&rhs[buffer.len()..]);
        }
        buffer.into()
    }
}

impl BitXor<UBig> for UBig {
    type Output = UBig;

    #[inline]
    fn bitxor(self, rhs: UBig) -> UBig {
        match (self.into_repr(), rhs.into_repr()) {
            (Small(word0), Small(word1)) => UBig::from_word(word0 ^ word1),
            (Small(word0), Large(buffer1)) => UBig::bitxor_large_word(buffer1, word0),
            (Large(buffer0), Small(word1)) => UBig::bitxor_large_word(buffer0, word1),
            (Large(buffer0), Large(buffer1)) => {
                if buffer0.len() >= buffer1.len() {
                    UBig::bitxor_large(buffer0, &buffer1)
                } else {
                    UBig::bitxor_large(buffer1, &buffer0)
                }
            }
        }
    }
}

impl BitXor<&UBig> for UBig {
    type Output = UBig;

    #[inline]
    fn bitxor(self, rhs: &UBig) -> UBig {
        match (self.into_repr(), rhs.repr()) {
            (Small(word0), Small(word1)) => UBig::from_word(word0 ^ word1),
            (Small(word0), Large(buffer1)) => UBig::bitxor_large_word(buffer1.clone(), word0),
            (Large(buffer0), Small(word1)) => UBig::bitxor_large_word(buffer0, *word1),
            (Large(buffer0), Large(buffer1)) => UBig::bitxor_large(buffer0, buffer1),
        }
    }
}

impl BitXor<UBig> for &UBig {
    type Output = UBig;

    #[inline]
    fn bitxor(self, rhs: UBig) -> UBig {
        rhs.bitxor(self)
    }
}

impl BitXor<&UBig> for &UBig {
    type Output = UBig;

    #[inline]
    fn bitxor(self, rhs: &UBig) -> UBig {
        match (self.repr(), rhs.repr()) {
            (Small(word0), Small(word1)) => UBig::from_word(word0 ^ word1),
            (Small(word0), Large(buffer1)) => UBig::bitxor_large_word(buffer1.clone(), *word0),
            (Large(buffer0), Small(word1)) => UBig::bitxor_large_word(buffer0.clone(), *word1),
            (Large(buffer0), Large(buffer1)) => {
                if buffer0.len() >= buffer1.len() {
                    UBig::bitxor_large(buffer0.clone(), buffer1)
                } else {
                    UBig::bitxor_large(buffer1.clone(), buffer0)
                }
            }
        }
    }
}

impl BitXorAssign<UBig> for UBig {
    #[inline]
    fn bitxor_assign(&mut self, rhs: UBig) {
        *self = mem::take(self) ^ rhs;
    }
}

impl BitXorAssign<&UBig> for UBig {
    #[inline]
    fn bitxor_assign(&mut self, rhs: &UBig) {
        *self = mem::take(self) ^ rhs;
    }
}

impl UBig {
    fn bitxor_large_word(mut buffer: Buffer, rhs: Word) -> UBig {
        debug_assert!(buffer.len() >= 2);

        *buffer.first_mut().unwrap_or_else(|| {
            panic!(
                "Panicked at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        }) ^= rhs;
        buffer.into()
    }

    fn bitxor_large(mut buffer: Buffer, rhs: &[Word]) -> UBig {
        for (x, y) in buffer.iter_mut().zip(rhs.iter()) {
            *x ^= *y;
        }
        if rhs.len() > buffer.len() {
            buffer.ensure_capacity(rhs.len());
            buffer.extend(&rhs[buffer.len()..]);
        }
        buffer.into()
    }
}

impl AndNot<UBig> for UBig {
    type Output = UBig;

    #[inline]
    fn and_not(self, rhs: UBig) -> UBig {
        match (self.into_repr(), rhs.into_repr()) {
            (Small(word0), Small(word1)) => UBig::from_word(word0 & !word1),
            (Small(word0), Large(buffer1)) => UBig::from_word(
                word0
                    & !buffer1.first().unwrap_or_else(|| {
                        panic!(
                            "Panicked at {}:{} (git sha: {})",
                            file!(),
                            line!(),
                            option_env!("GIT_SHA").unwrap_or("unknown")
                        )
                    }),
            ),
            (Large(buffer0), Small(word1)) => UBig::and_not_large_word(buffer0, word1),
            (Large(buffer0), Large(buffer1)) => UBig::and_not_large(buffer0, &buffer1),
        }
    }
}

impl AndNot<&UBig> for UBig {
    type Output = UBig;

    #[inline]
    fn and_not(self, rhs: &UBig) -> UBig {
        match (self.into_repr(), rhs.repr()) {
            (Small(word0), Small(word1)) => UBig::from_word(word0 & !word1),
            (Small(word0), Large(buffer1)) => UBig::from_word(
                word0
                    & !buffer1.first().unwrap_or_else(|| {
                        panic!(
                            "Panicked at {}:{} (git sha: {})",
                            file!(),
                            line!(),
                            option_env!("GIT_SHA").unwrap_or("unknown")
                        )
                    }),
            ),
            (Large(buffer0), Small(word1)) => UBig::and_not_large_word(buffer0, *word1),
            (Large(buffer0), Large(buffer1)) => UBig::and_not_large(buffer0, buffer1),
        }
    }
}

impl AndNot<UBig> for &UBig {
    type Output = UBig;

    #[inline]
    fn and_not(self, rhs: UBig) -> UBig {
        match (self.repr(), rhs.into_repr()) {
            (Small(word0), Small(word1)) => UBig::from_word(word0 & !word1),
            (Small(word0), Large(buffer1)) => UBig::from_word(
                word0
                    & !buffer1.first().unwrap_or_else(|| {
                        panic!(
                            "Panicked at {}:{} (git sha: {})",
                            file!(),
                            line!(),
                            option_env!("GIT_SHA").unwrap_or("unknown")
                        )
                    }),
            ),
            (Large(buffer0), Small(word1)) => UBig::and_not_large_word(buffer0.clone(), word1),
            // TODO: Could reuse buffer1 in some cases.
            (Large(buffer0), Large(buffer1)) => UBig::and_not_large(buffer0.clone(), &buffer1),
        }
    }
}

impl AndNot<&UBig> for &UBig {
    type Output = UBig;

    #[inline]
    fn and_not(self, rhs: &UBig) -> UBig {
        match (self.repr(), rhs.repr()) {
            (Small(word0), Small(word1)) => UBig::from_word(word0 & !word1),
            (Small(word0), Large(buffer1)) => UBig::from_word(
                word0
                    & !buffer1.first().unwrap_or_else(|| {
                        panic!(
                            "Panicked at {}:{} (git sha: {})",
                            file!(),
                            line!(),
                            option_env!("GIT_SHA").unwrap_or("unknown")
                        )
                    }),
            ),
            (Large(buffer0), Small(word1)) => UBig::and_not_large_word(buffer0.clone(), *word1),
            (Large(buffer0), Large(buffer1)) => UBig::and_not_large(buffer0.clone(), buffer1),
        }
    }
}

impl UBig {
    fn and_not_large_word(mut buffer: Buffer, rhs: Word) -> UBig {
        debug_assert!(buffer.len() >= 2);

        *buffer.first_mut().unwrap_or_else(|| {
            panic!(
                "Panicked at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        }) &= !rhs;
        buffer.into()
    }

    fn and_not_large(mut buffer: Buffer, rhs: &[Word]) -> UBig {
        for (x, y) in buffer.iter_mut().zip(rhs.iter()) {
            *x &= !*y;
        }
        buffer.into()
    }
}

impl Not for IBig {
    type Output = IBig;

    #[inline]
    fn not(self) -> IBig {
        -(self + IBig::from(1u8))
    }
}

impl Not for &IBig {
    type Output = IBig;

    #[inline]
    fn not(self) -> IBig {
        -(self + IBig::from(1u8))
    }
}

impl BitAnd<IBig> for IBig {
    type Output = IBig;

    #[inline]
    fn bitand(self, rhs: IBig) -> IBig {
        match (self.sign(), rhs.sign()) {
            (Positive, Positive) => IBig::from(self.unsigned_abs() & rhs.unsigned_abs()),
            (Positive, Negative) => IBig::from(self.unsigned_abs().and_not((!rhs).unsigned_abs())),
            (Negative, Positive) => IBig::from(rhs.unsigned_abs().and_not((!self).unsigned_abs())),
            (Negative, Negative) => !IBig::from((!self).unsigned_abs() | (!rhs).unsigned_abs()),
        }
    }
}

impl BitAnd<&IBig> for IBig {
    type Output = IBig;

    #[inline]
    fn bitand(self, rhs: &IBig) -> IBig {
        match (self.sign(), rhs.sign()) {
            (Positive, Positive) => IBig::from(self.unsigned_abs() & rhs.magnitude()),
            (Positive, Negative) => IBig::from(self.unsigned_abs().and_not((!rhs).unsigned_abs())),
            (Negative, Positive) => IBig::from(rhs.magnitude().and_not((!self).unsigned_abs())),
            (Negative, Negative) => !IBig::from((!self).unsigned_abs() | (!rhs).unsigned_abs()),
        }
    }
}

impl BitAnd<IBig> for &IBig {
    type Output = IBig;

    #[inline]
    fn bitand(self, rhs: IBig) -> IBig {
        rhs.bitand(self)
    }
}

impl BitAnd<&IBig> for &IBig {
    type Output = IBig;

    #[inline]
    fn bitand(self, rhs: &IBig) -> IBig {
        match (self.sign(), rhs.sign()) {
            (Positive, Positive) => IBig::from(self.magnitude() & rhs.magnitude()),
            (Positive, Negative) => IBig::from(self.magnitude().and_not((!rhs).unsigned_abs())),
            (Negative, Positive) => IBig::from(rhs.magnitude().and_not((!self).unsigned_abs())),
            (Negative, Negative) => !IBig::from((!self).unsigned_abs() | (!rhs).unsigned_abs()),
        }
    }
}

impl BitAndAssign<IBig> for IBig {
    #[inline]
    fn bitand_assign(&mut self, rhs: IBig) {
        *self = mem::take(self) & rhs;
    }
}

impl BitAndAssign<&IBig> for IBig {
    #[inline]
    fn bitand_assign(&mut self, rhs: &IBig) {
        *self = mem::take(self) & rhs;
    }
}

impl BitOr<IBig> for IBig {
    type Output = IBig;

    #[inline]
    fn bitor(self, rhs: IBig) -> IBig {
        match (self.sign(), rhs.sign()) {
            (Positive, Positive) => IBig::from(self.unsigned_abs() | rhs.unsigned_abs()),
            (Positive, Negative) => !IBig::from((!rhs).unsigned_abs().and_not(self.unsigned_abs())),
            (Negative, Positive) => !IBig::from((!self).unsigned_abs().and_not(rhs.unsigned_abs())),
            (Negative, Negative) => !IBig::from((!self).unsigned_abs() & (!rhs).unsigned_abs()),
        }
    }
}

impl BitOr<&IBig> for IBig {
    type Output = IBig;

    #[inline]
    fn bitor(self, rhs: &IBig) -> IBig {
        match (self.sign(), rhs.sign()) {
            (Positive, Positive) => IBig::from(self.unsigned_abs() | rhs.magnitude()),
            (Positive, Negative) => !IBig::from((!rhs).unsigned_abs().and_not(self.unsigned_abs())),
            (Negative, Positive) => !IBig::from((!self).unsigned_abs().and_not(rhs.magnitude())),
            (Negative, Negative) => !IBig::from((!self).unsigned_abs() & (!rhs).unsigned_abs()),
        }
    }
}

impl BitOr<IBig> for &IBig {
    type Output = IBig;

    #[inline]
    fn bitor(self, rhs: IBig) -> IBig {
        rhs.bitor(self)
    }
}

impl BitOr<&IBig> for &IBig {
    type Output = IBig;

    #[inline]
    fn bitor(self, rhs: &IBig) -> IBig {
        match (self.sign(), rhs.sign()) {
            (Positive, Positive) => IBig::from(self.magnitude() | rhs.magnitude()),
            (Positive, Negative) => !IBig::from((!rhs).unsigned_abs().and_not(self.magnitude())),
            (Negative, Positive) => !IBig::from((!self).unsigned_abs().and_not(rhs.magnitude())),
            (Negative, Negative) => !IBig::from((!self).unsigned_abs() & (!rhs).unsigned_abs()),
        }
    }
}

impl BitOrAssign<IBig> for IBig {
    #[inline]
    fn bitor_assign(&mut self, rhs: IBig) {
        *self = mem::take(self) | rhs;
    }
}

impl BitOrAssign<&IBig> for IBig {
    #[inline]
    fn bitor_assign(&mut self, rhs: &IBig) {
        *self = mem::take(self) | rhs;
    }
}

impl BitXor<IBig> for IBig {
    type Output = IBig;

    #[inline]
    fn bitxor(self, rhs: IBig) -> IBig {
        match (self.sign(), rhs.sign()) {
            (Positive, Positive) => IBig::from(self.unsigned_abs() ^ rhs.unsigned_abs()),
            (Positive, Negative) => !IBig::from(self.unsigned_abs() ^ (!rhs).unsigned_abs()),
            (Negative, Positive) => !IBig::from((!self).unsigned_abs() ^ rhs.unsigned_abs()),
            (Negative, Negative) => IBig::from((!self).unsigned_abs() ^ (!rhs).unsigned_abs()),
        }
    }
}

impl BitXor<&IBig> for IBig {
    type Output = IBig;

    #[inline]
    fn bitxor(self, rhs: &IBig) -> IBig {
        match (self.sign(), rhs.sign()) {
            (Positive, Positive) => IBig::from(self.unsigned_abs() ^ rhs.magnitude()),
            (Positive, Negative) => !IBig::from(self.unsigned_abs() ^ (!rhs).unsigned_abs()),
            (Negative, Positive) => !IBig::from((!self).unsigned_abs() ^ rhs.magnitude()),
            (Negative, Negative) => IBig::from((!self).unsigned_abs() ^ (!rhs).unsigned_abs()),
        }
    }
}

impl BitXor<IBig> for &IBig {
    type Output = IBig;

    #[inline]
    fn bitxor(self, rhs: IBig) -> IBig {
        rhs.bitxor(self)
    }
}

impl BitXor<&IBig> for &IBig {
    type Output = IBig;

    #[inline]
    fn bitxor(self, rhs: &IBig) -> IBig {
        match (self.sign(), rhs.sign()) {
            (Positive, Positive) => IBig::from(self.magnitude() ^ rhs.magnitude()),
            (Positive, Negative) => !IBig::from(self.magnitude() ^ (!rhs).unsigned_abs()),
            (Negative, Positive) => !IBig::from((!self).unsigned_abs() ^ rhs.magnitude()),
            (Negative, Negative) => IBig::from((!self).unsigned_abs() ^ (!rhs).unsigned_abs()),
        }
    }
}

impl BitXorAssign<IBig> for IBig {
    #[inline]
    fn bitxor_assign(&mut self, rhs: IBig) {
        *self = mem::take(self) ^ rhs;
    }
}

impl BitXorAssign<&IBig> for IBig {
    #[inline]
    fn bitxor_assign(&mut self, rhs: &IBig) {
        *self = mem::take(self) ^ rhs;
    }
}

impl AndNot<IBig> for IBig {
    type Output = IBig;

    #[inline]
    fn and_not(self, rhs: IBig) -> IBig {
        match (self.sign(), rhs.sign()) {
            (Positive, Positive) => IBig::from(self.unsigned_abs().and_not(rhs.unsigned_abs())),
            (Positive, Negative) => IBig::from(self.unsigned_abs() & (!rhs).unsigned_abs()),
            (Negative, Positive) => !IBig::from((!self).unsigned_abs() | rhs.unsigned_abs()),
            (Negative, Negative) => {
                IBig::from((!rhs).unsigned_abs().and_not((!self).unsigned_abs()))
            }
        }
    }
}

impl AndNot<&IBig> for IBig {
    type Output = IBig;

    #[inline]
    fn and_not(self, rhs: &IBig) -> IBig {
        match (self.sign(), rhs.sign()) {
            (Positive, Positive) => IBig::from(self.unsigned_abs().and_not(rhs.magnitude())),
            (Positive, Negative) => IBig::from(self.unsigned_abs() & (!rhs).unsigned_abs()),
            (Negative, Positive) => !IBig::from((!self).unsigned_abs() | rhs.magnitude()),
            (Negative, Negative) => {
                IBig::from((!rhs).unsigned_abs().and_not((!self).unsigned_abs()))
            }
        }
    }
}

impl AndNot<IBig> for &IBig {
    type Output = IBig;

    #[inline]
    fn and_not(self, rhs: IBig) -> IBig {
        match (self.sign(), rhs.sign()) {
            (Positive, Positive) => IBig::from(self.magnitude().and_not(rhs.unsigned_abs())),
            (Positive, Negative) => IBig::from(self.magnitude() & (!rhs).unsigned_abs()),
            (Negative, Positive) => !IBig::from((!self).unsigned_abs() | rhs.unsigned_abs()),
            (Negative, Negative) => {
                IBig::from((!rhs).unsigned_abs().and_not((!self).unsigned_abs()))
            }
        }
    }
}

impl AndNot<&IBig> for &IBig {
    type Output = IBig;

    #[inline]
    fn and_not(self, rhs: &IBig) -> IBig {
        match (self.sign(), rhs.sign()) {
            (Positive, Positive) => IBig::from(self.magnitude().and_not(rhs.magnitude())),
            (Positive, Negative) => IBig::from(self.magnitude() & (!rhs).unsigned_abs()),
            (Negative, Positive) => !IBig::from((!self).unsigned_abs() | rhs.magnitude()),
            (Negative, Negative) => {
                IBig::from((!rhs).unsigned_abs().and_not((!self).unsigned_abs()))
            }
        }
    }
}

impl UBig {
    /// low n bits or'd
    #[inline]
    pub(crate) fn are_low_bits_nonzero(&self, n: usize) -> bool {
        match self.repr() {
            Small(word) => {
                let n = n.min(WORD_BITS_USIZE) as u32;
                word & math::ones::<Word>(n) != 0
            }
            Large(buffer) => {
                let n_words = n / WORD_BITS_USIZE;
                if n_words >= buffer.len() {
                    true
                } else {
                    let n_top = (n % WORD_BITS_USIZE) as u32;
                    buffer[..n_words].iter().any(|x| *x != 0)
                        || buffer[n_words] & math::ones::<Word>(n_top) != 0
                }
            }
        }
    }
}

macro_rules! impl_bit_ops_ubig_unsigned {
    ($t:ty) => {
        impl BitAnd<$t> for UBig {
            type Output = $t;

            #[inline]
            fn bitand(self, rhs: $t) -> $t {
                self.bitand_unsigned(rhs)
            }
        }

        impl BitAnd<$t> for &UBig {
            type Output = $t;

            #[inline]
            fn bitand(self, rhs: $t) -> $t {
                self.bitand_ref_unsigned(rhs)
            }
        }

        helper_macros::forward_binop_second_arg_by_value!(impl BitAnd<$t> for UBig, bitand);
        helper_macros::forward_binop_swap_args!(impl BitAnd<UBig> for $t, bitand);

        impl BitAndAssign<$t> for UBig {
            #[inline]
            fn bitand_assign(&mut self, rhs: $t) {
                self.bitand_assign_unsigned(rhs)
            }
        }

        helper_macros::forward_binop_assign_arg_by_value!(impl BitAndAssign<$t> for UBig, bitand_assign);

        impl BitOr<$t> for UBig {
            type Output = UBig;

            #[inline]
            fn bitor(self, rhs: $t) -> UBig {
                self.bitor_unsigned(rhs)
            }
        }

        impl BitOr<$t> for &UBig {
            type Output = UBig;

            #[inline]
            fn bitor(self, rhs: $t) -> UBig {
                self.bitor_ref_unsigned(rhs)
            }
        }

        helper_macros::forward_binop_second_arg_by_value!(impl BitOr<$t> for UBig, bitor);
        helper_macros::forward_binop_swap_args!(impl BitOr<UBig> for $t, bitor);

        impl BitOrAssign<$t> for UBig {
            #[inline]
            fn bitor_assign(&mut self, rhs: $t) {
                self.bitor_assign_unsigned(rhs)
            }
        }

        helper_macros::forward_binop_assign_arg_by_value!(impl BitOrAssign<$t> for UBig, bitor_assign);

        impl BitXor<$t> for UBig {
            type Output = UBig;

            #[inline]
            fn bitxor(self, rhs: $t) -> UBig {
                self.bitxor_unsigned(rhs)
            }
        }

        impl BitXor<$t> for &UBig {
            type Output = UBig;

            #[inline]
            fn bitxor(self, rhs: $t) -> UBig {
                self.bitxor_ref_unsigned(rhs)
            }
        }

        helper_macros::forward_binop_second_arg_by_value!(impl BitXor<$t> for UBig, bitxor);
        helper_macros::forward_binop_swap_args!(impl BitXor<UBig> for $t, bitxor);

        impl BitXorAssign<$t> for UBig {
            #[inline]
            fn bitxor_assign(&mut self, rhs: $t) {
                self.bitxor_assign_unsigned(rhs)
            }
        }

        helper_macros::forward_binop_assign_arg_by_value!(impl BitXorAssign<$t> for UBig, bitxor_assign);

        impl AndNot<$t> for UBig {
            type Output = UBig;

            #[inline]
            fn and_not(self, rhs: $t) -> UBig {
                self.and_not_unsigned(rhs)
            }
        }

        impl AndNot<$t> for &UBig {
            type Output = UBig;

            #[inline]
            fn and_not(self, rhs: $t) -> UBig {
                self.and_not_ref_unsigned(rhs)
            }
        }

        helper_macros::forward_binop_second_arg_by_value!(impl AndNot<$t> for UBig, and_not);
    };
}

impl_bit_ops_ubig_unsigned!(u8);
impl_bit_ops_ubig_unsigned!(u16);
impl_bit_ops_ubig_unsigned!(u32);
impl_bit_ops_ubig_unsigned!(u64);
impl_bit_ops_ubig_unsigned!(u128);
impl_bit_ops_ubig_unsigned!(usize);

macro_rules! impl_bit_ops_ubig_signed {
    ($t:ty) => {
        impl BitAnd<$t> for UBig {
            type Output = UBig;

            #[inline]
            fn bitand(self, rhs: $t) -> UBig {
                self.bitand_signed(rhs)
            }
        }

        impl BitAnd<$t> for &UBig {
            type Output = UBig;

            #[inline]
            fn bitand(self, rhs: $t) -> UBig {
                self.bitand_ref_signed(rhs)
            }
        }

        helper_macros::forward_binop_second_arg_by_value!(impl BitAnd<$t> for UBig, bitand);
        helper_macros::forward_binop_swap_args!(impl BitAnd<UBig> for $t, bitand);

        impl BitAndAssign<$t> for UBig {
            #[inline]
            fn bitand_assign(&mut self, rhs: $t) {
                self.bitand_assign_signed(rhs)
            }
        }

        helper_macros::forward_binop_assign_arg_by_value!(impl BitAndAssign<$t> for UBig, bitand_assign);

        impl BitOr<$t> for UBig {
            type Output = UBig;

            #[inline]
            fn bitor(self, rhs: $t) -> UBig {
                self.bitor_signed(rhs)
            }
        }

        impl BitOr<$t> for &UBig {
            type Output = UBig;

            #[inline]
            fn bitor(self, rhs: $t) -> UBig {
                self.bitor_ref_signed(rhs)
            }
        }

        helper_macros::forward_binop_second_arg_by_value!(impl BitOr<$t> for UBig, bitor);
        helper_macros::forward_binop_swap_args!(impl BitOr<UBig> for $t, bitor);

        impl BitOrAssign<$t> for UBig {
            #[inline]
            fn bitor_assign(&mut self, rhs: $t) {
                self.bitor_assign_signed(rhs)
            }
        }

        helper_macros::forward_binop_assign_arg_by_value!(impl BitOrAssign<$t> for UBig, bitor_assign);

        impl BitXor<$t> for UBig {
            type Output = UBig;

            #[inline]
            fn bitxor(self, rhs: $t) -> UBig {
                self.bitxor_signed(rhs)
            }
        }

        impl BitXor<$t> for &UBig {
            type Output = UBig;

            #[inline]
            fn bitxor(self, rhs: $t) -> UBig {
                self.bitxor_ref_signed(rhs)
            }
        }

        helper_macros::forward_binop_second_arg_by_value!(impl BitXor<$t> for UBig, bitxor);
        helper_macros::forward_binop_swap_args!(impl BitXor<UBig> for $t, bitxor);

        impl BitXorAssign<$t> for UBig {
            #[inline]
            fn bitxor_assign(&mut self, rhs: $t) {
                self.bitxor_assign_signed(rhs)
            }
        }

        helper_macros::forward_binop_assign_arg_by_value!(impl BitXorAssign<$t> for UBig, bitxor_assign);

        impl AndNot<$t> for UBig {
            type Output = UBig;

            #[inline]
            fn and_not(self, rhs: $t) -> UBig {
                self.and_not_signed(rhs)
            }
        }

        impl AndNot<$t> for &UBig {
            type Output = UBig;

            #[inline]
            fn and_not(self, rhs: $t) -> UBig {
                self.and_not_ref_signed(rhs)
            }
        }

        helper_macros::forward_binop_second_arg_by_value!(impl AndNot<$t> for UBig, and_not);
    };
}

impl_bit_ops_ubig_signed!(i8);
impl_bit_ops_ubig_signed!(i16);
impl_bit_ops_ubig_signed!(i32);
impl_bit_ops_ubig_signed!(i64);
impl_bit_ops_ubig_signed!(i128);
impl_bit_ops_ubig_signed!(isize);

macro_rules! impl_bit_ops_ibig_unsigned {
    ($t:ty) => {
        impl BitAnd<$t> for IBig {
            type Output = $t;

            #[inline]
            fn bitand(self, rhs: $t) -> $t {
                self.bitand_unsigned(rhs)
            }
        }

        impl BitAnd<$t> for &IBig {
            type Output = $t;

            #[inline]
            fn bitand(self, rhs: $t) -> $t {
                self.bitand_ref_unsigned(rhs)
            }
        }
    };
}

impl_bit_ops_ibig_unsigned!(u8);
impl_bit_ops_ibig_unsigned!(u16);
impl_bit_ops_ibig_unsigned!(u32);
impl_bit_ops_ibig_unsigned!(u64);
impl_bit_ops_ibig_unsigned!(u128);
impl_bit_ops_ibig_unsigned!(usize);

macro_rules! impl_bit_ops_ibig_signed {
    ($t:ty) => {
        impl BitAnd<$t> for IBig {
            type Output = IBig;

            #[inline]
            fn bitand(self, rhs: $t) -> IBig {
                self.bitand_signed(rhs)
            }
        }

        impl BitAnd<$t> for &IBig {
            type Output = IBig;

            #[inline]
            fn bitand(self, rhs: $t) -> IBig {
                self.bitand_ref_signed(rhs)
            }
        }
    };
}

impl_bit_ops_ibig_signed!(i8);
impl_bit_ops_ibig_signed!(i16);
impl_bit_ops_ibig_signed!(i32);
impl_bit_ops_ibig_signed!(i64);
impl_bit_ops_ibig_signed!(i128);
impl_bit_ops_ibig_signed!(isize);

macro_rules! impl_bit_ops_ibig_primitive {
    ($t:ty) => {
        helper_macros::forward_binop_second_arg_by_value!(impl BitAnd<$t> for IBig, bitand);
        helper_macros::forward_binop_swap_args!(impl BitAnd<IBig> for $t, bitand);

        impl BitAndAssign<$t> for IBig {
            #[inline]
            fn bitand_assign(&mut self, rhs: $t) {
                self.bitand_assign_primitive(rhs)
            }
        }

        helper_macros::forward_binop_assign_arg_by_value!(impl BitAndAssign<$t> for IBig, bitand_assign);

        impl BitOr<$t> for IBig {
            type Output = IBig;

            #[inline]
            fn bitor(self, rhs: $t) -> IBig {
                self.bitor_primitive(rhs)
            }
        }

        impl BitOr<$t> for &IBig {
            type Output = IBig;

            #[inline]
            fn bitor(self, rhs: $t) -> IBig {
                self.bitor_ref_primitive(rhs)
            }
        }

        helper_macros::forward_binop_second_arg_by_value!(impl BitOr<$t> for IBig, bitor);
        helper_macros::forward_binop_swap_args!(impl BitOr<IBig> for $t, bitor);

        impl BitOrAssign<$t> for IBig {
            #[inline]
            fn bitor_assign(&mut self, rhs: $t) {
                self.bitor_assign_primitive(rhs)
            }
        }

        helper_macros::forward_binop_assign_arg_by_value!(impl BitOrAssign<$t> for IBig, bitor_assign);

        impl BitXor<$t> for IBig {
            type Output = IBig;

            #[inline]
            fn bitxor(self, rhs: $t) -> IBig {
                self.bitxor_primitive(rhs)
            }
        }

        impl BitXor<$t> for &IBig {
            type Output = IBig;

            #[inline]
            fn bitxor(self, rhs: $t) -> IBig {
                self.bitxor_ref_primitive(rhs)
            }
        }

        helper_macros::forward_binop_second_arg_by_value!(impl BitXor<$t> for IBig, bitxor);
        helper_macros::forward_binop_swap_args!(impl BitXor<IBig> for $t, bitxor);

        impl BitXorAssign<$t> for IBig {
            #[inline]
            fn bitxor_assign(&mut self, rhs: $t) {
                self.bitxor_assign_primitive(rhs)
            }
        }

        helper_macros::forward_binop_assign_arg_by_value!(impl BitXorAssign<$t> for IBig, bitxor_assign);

        impl AndNot<$t> for IBig {
            type Output = IBig;

            #[inline]
            fn and_not(self, rhs: $t) -> IBig {
                self.and_not_primitive(rhs)
            }
        }

        impl AndNot<$t> for &IBig {
            type Output = IBig;

            #[inline]
            fn and_not(self, rhs: $t) -> IBig {
                self.and_not_ref_primitive(rhs)
            }
        }

        helper_macros::forward_binop_second_arg_by_value!(impl AndNot<$t> for IBig, and_not);
    };
}

impl_bit_ops_ibig_primitive!(u8);
impl_bit_ops_ibig_primitive!(u16);
impl_bit_ops_ibig_primitive!(u32);
impl_bit_ops_ibig_primitive!(u64);
impl_bit_ops_ibig_primitive!(u128);
impl_bit_ops_ibig_primitive!(usize);
impl_bit_ops_ibig_primitive!(i8);
impl_bit_ops_ibig_primitive!(i16);
impl_bit_ops_ibig_primitive!(i32);
impl_bit_ops_ibig_primitive!(i64);
impl_bit_ops_ibig_primitive!(i128);
impl_bit_ops_ibig_primitive!(isize);

impl UBig {
    #[inline]
    fn bitand_unsigned<T: PrimitiveUnsigned>(self, rhs: T) -> T {
        self.bitand(UBig::from_unsigned(rhs))
            .try_to_unsigned()
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            })
    }

    #[inline]
    fn bitand_ref_unsigned<T: PrimitiveUnsigned>(&self, rhs: T) -> T {
        self.bitand(UBig::from_unsigned(rhs))
            .try_to_unsigned()
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            })
    }

    #[inline]
    fn bitand_assign_unsigned<T: PrimitiveUnsigned>(&mut self, rhs: T) {
        self.bitand_assign(UBig::from_unsigned(rhs))
    }

    #[inline]
    fn bitor_unsigned<T: PrimitiveUnsigned>(self, rhs: T) -> UBig {
        self.bitor(UBig::from_unsigned(rhs))
    }

    #[inline]
    fn bitor_ref_unsigned<T: PrimitiveUnsigned>(&self, rhs: T) -> UBig {
        self.bitor(UBig::from_unsigned(rhs))
    }

    #[inline]
    fn bitor_assign_unsigned<T: PrimitiveUnsigned>(&mut self, rhs: T) {
        self.bitor_assign(UBig::from_unsigned(rhs))
    }

    #[inline]
    fn bitxor_unsigned<T: PrimitiveUnsigned>(self, rhs: T) -> UBig {
        self.bitxor(UBig::from_unsigned(rhs))
    }

    #[inline]
    fn bitxor_ref_unsigned<T: PrimitiveUnsigned>(&self, rhs: T) -> UBig {
        self.bitxor(UBig::from_unsigned(rhs))
    }

    #[inline]
    fn bitxor_assign_unsigned<T: PrimitiveUnsigned>(&mut self, rhs: T) {
        self.bitxor_assign(UBig::from_unsigned(rhs))
    }

    #[inline]
    fn and_not_unsigned<T: PrimitiveUnsigned>(self, rhs: T) -> UBig {
        self.and_not(UBig::from_unsigned(rhs))
    }

    #[inline]
    fn and_not_ref_unsigned<T: PrimitiveUnsigned>(&self, rhs: T) -> UBig {
        self.and_not(UBig::from_unsigned(rhs))
    }

    #[inline]
    fn bitand_signed<T: PrimitiveSigned>(self, rhs: T) -> UBig {
        UBig::from_ibig_panic_on_overflow(IBig::from(self) & IBig::from_signed(rhs))
    }

    #[inline]
    fn bitand_ref_signed<T: PrimitiveSigned>(&self, rhs: T) -> UBig {
        // Avoid big copy if rhs positive.
        let rhs_signed = IBig::from_signed(rhs);
        match rhs_signed.sign() {
            Positive => self & rhs_signed.unsigned_abs(),
            Negative => UBig::from_ibig_panic_on_overflow(IBig::from(self) & rhs_signed),
        }
    }

    #[inline]
    fn bitand_assign_signed<T: PrimitiveSigned>(&mut self, rhs: T) {
        *self = mem::take(self).bitand_signed(rhs)
    }

    #[inline]
    fn bitor_signed<T: PrimitiveSigned>(self, rhs: T) -> UBig {
        UBig::from_ibig_panic_on_overflow(IBig::from(self) | IBig::from_signed(rhs))
    }

    #[inline]
    fn bitor_ref_signed<T: PrimitiveSigned>(&self, rhs: T) -> UBig {
        UBig::from_ibig_panic_on_overflow(IBig::from(self) | IBig::from_signed(rhs))
    }

    #[inline]
    fn bitor_assign_signed<T: PrimitiveSigned>(&mut self, rhs: T) {
        *self = mem::take(self).bitor_signed(rhs)
    }

    #[inline]
    fn bitxor_signed<T: PrimitiveSigned>(self, rhs: T) -> UBig {
        UBig::from_ibig_panic_on_overflow(IBig::from(self) ^ IBig::from_signed(rhs))
    }

    #[inline]
    fn bitxor_ref_signed<T: PrimitiveSigned>(&self, rhs: T) -> UBig {
        UBig::from_ibig_panic_on_overflow(IBig::from(self) ^ IBig::from_signed(rhs))
    }

    #[inline]
    fn bitxor_assign_signed<T: PrimitiveSigned>(&mut self, rhs: T) {
        *self = mem::take(self).bitxor_signed(rhs)
    }

    #[inline]
    fn and_not_signed<T: PrimitiveSigned>(self, rhs: T) -> UBig {
        UBig::from_ibig_panic_on_overflow(IBig::from(self).and_not(IBig::from_signed(rhs)))
    }

    #[inline]
    fn and_not_ref_signed<T: PrimitiveSigned>(&self, rhs: T) -> UBig {
        UBig::from_ibig_panic_on_overflow(IBig::from(self).and_not(IBig::from_signed(rhs)))
    }
}

impl IBig {
    #[inline]
    fn bitand_unsigned<T: PrimitiveUnsigned>(self, rhs: T) -> T {
        self.bitand(IBig::from_unsigned(rhs))
            .try_to_unsigned()
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            })
    }

    #[inline]
    fn bitand_ref_unsigned<T: PrimitiveUnsigned>(&self, rhs: T) -> T {
        self.bitand(IBig::from_unsigned(rhs))
            .try_to_unsigned()
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            })
    }

    #[inline]
    fn bitand_signed<T: PrimitiveSigned>(self, rhs: T) -> IBig {
        self.bitand(IBig::from_signed(rhs))
    }

    #[inline]
    fn bitand_ref_signed<T: PrimitiveSigned>(&self, rhs: T) -> IBig {
        self.bitand(IBig::from_signed(rhs))
    }

    #[inline]
    fn bitand_assign_primitive<T>(&mut self, rhs: T)
    where
        IBig: From<T>,
    {
        self.bitand_assign(IBig::from(rhs))
    }

    #[inline]
    fn bitor_primitive<T>(self, rhs: T) -> IBig
    where
        IBig: From<T>,
    {
        self.bitor(IBig::from(rhs))
    }

    #[inline]
    fn bitor_ref_primitive<T>(&self, rhs: T) -> IBig
    where
        IBig: From<T>,
    {
        self.bitor(IBig::from(rhs))
    }

    #[inline]
    fn bitor_assign_primitive<T>(&mut self, rhs: T)
    where
        IBig: From<T>,
    {
        self.bitor_assign(IBig::from(rhs))
    }

    #[inline]
    fn bitxor_primitive<T>(self, rhs: T) -> IBig
    where
        IBig: From<T>,
    {
        self.bitxor(IBig::from(rhs))
    }

    #[inline]
    fn bitxor_ref_primitive<T>(&self, rhs: T) -> IBig
    where
        IBig: From<T>,
    {
        self.bitxor(IBig::from(rhs))
    }

    #[inline]
    fn bitxor_assign_primitive<T>(&mut self, rhs: T)
    where
        IBig: From<T>,
    {
        self.bitxor_assign(IBig::from(rhs))
    }

    #[inline]
    fn and_not_primitive<T>(self, rhs: T) -> IBig
    where
        IBig: From<T>,
    {
        self.and_not(IBig::from(rhs))
    }

    #[inline]
    fn and_not_ref_primitive<T>(&self, rhs: T) -> IBig
    where
        IBig: From<T>,
    {
        self.and_not(IBig::from(rhs))
    }
}
