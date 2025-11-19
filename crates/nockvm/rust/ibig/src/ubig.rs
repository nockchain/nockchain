//! Unsigned big integer.

use core::slice;

use self::Repr::*;
use crate::arch::ntt;
use crate::arch::word::Word;
use crate::buffer::Buffer;
use crate::math;
use crate::memory::Stack;
use crate::primitive::WORD_BITS_USIZE;

/// Internal representation of UBig.
#[derive(Debug, Eq, Hash, PartialEq)]
pub(crate) enum Repr {
    /// A number that fits in a single Word.
    Small(Word),
    /// A number that does not fit in a single Word.
    ///
    /// The buffer has:
    /// * length at least 2
    /// * no leading zero
    /// * compact capacity
    Large(Buffer),
}

/// Unsigned big integer.
///
/// Arbitrarily large unsigned integer.
///
/// # Examples
///
/// ```
/// # use ibig::{error::ParseError, ubig, UBig};
/// let a = ubig!(a2a123bbb127779cccc123123ccc base 32);
/// let b = ubig!(0x1231abcd4134);
/// let c = UBig::from_str_radix("a2a123bbb127779cccc123123ccc", 32)?;
/// let d = UBig::from_str_radix("1231abcd4134", 16)?;
/// assert_eq!(a, c);
/// assert_eq!(b, d);
/// # Ok::<(), ParseError>(())
/// ```
#[derive(Eq, Hash, PartialEq)]
pub struct UBig(Repr);

impl UBig {
    /// Construct from one word.
    #[inline]
    pub(crate) fn from_word(word: Word) -> UBig {
        UBig(Small(word))
    }

    /// Get the representation of UBig.
    #[inline]
    pub(crate) fn repr(&self) -> &Repr {
        &self.0
    }

    /// Convert into representation.
    #[inline]
    pub(crate) fn into_repr(self) -> Repr {
        self.0
    }

    /// Length in Words.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        match self.repr() {
            Small(_) => 1,
            Large(buffer) => buffer.len(),
        }
    }

    /// Representation in Words.
    #[inline]
    pub(crate) fn as_words(&self) -> &[Word] {
        match self.repr() {
            Small(0) => &[],
            Small(word) => slice::from_ref(word),
            Large(buffer) => buffer,
        }
    }

    /// Maximum length in `Word`s.
    ///
    /// Ensures that the number of bits fits in `usize`, which is useful for bit count
    /// operations, and for radix conversions (even base 2 can be represented).
    ///
    /// This also guarantees that up to 16 * length will not overflow.
    ///
    /// We also make sure that any multiplication whose result fits in `MAX_LEN` can fit
    /// within the largest possible number-theoretic transform.
    ///
    /// Also make sure this is even, useful for checking whether a square will overflow.
    pub(crate) const MAX_LEN: usize = math::min_usize(
        usize::MAX / WORD_BITS_USIZE,
        match 1usize.checked_shl(ntt::MAX_ORDER) {
            Some(ntt_len) => ntt_len,
            None => usize::MAX,
        },
    ) & !1usize;

    /// Maximum length in bits.
    ///
    /// [UBig]s up to this length are supported. Creating a longer number
    /// will panic.
    ///
    /// This does not guarantee that there is sufficient memory to store numbers
    /// up to this length. Memory allocation may fail even for smaller numbers.
    ///
    /// The fact that this limit fits in `usize` guarantees that all bit
    /// addressing operations can be performed using `usize`.
    ///
    /// It is typically close to `usize::MAX`, but the exact value is platform-dependent.
    pub const MAX_BIT_LEN: usize = UBig::MAX_LEN * WORD_BITS_USIZE;

    pub(crate) fn panic_number_too_large() -> ! {
        panic!("number too large, maximum is {} bits", UBig::MAX_BIT_LEN)
    }
}

impl Clone for UBig {
    /// WARNING: This uses global allocator. Use clone_stack instead to prevent memory leaks.
    #[inline]
    fn clone(&self) -> UBig {
        match self.repr() {
            Small(x) => UBig(Small(*x)),
            Large(buffer) => UBig(Large(buffer.clone())),
        }
    }

    #[inline]
    fn clone_from(&mut self, source: &UBig) {
        if let Large(buffer) = &mut self.0 {
            if let Large(source_buffer) = source.repr() {
                buffer.resizing_clone_from(source_buffer);
                return;
            }
        }
        *self = source.clone();
    }
}

impl UBig {
    /// Clone a UBig using a stack allocator
    #[inline]
    pub fn clone_stack<S: Stack>(&self, stack: &mut S) -> UBig {
        match self.repr() {
            Small(x) => UBig(Small(*x)),
            Large(buffer) => {
                let mut new_buffer = Buffer::allocate_stack(stack, buffer.len());
                new_buffer.clone_from(buffer);
                UBig(Large(new_buffer))
            }
        }
    }
}

impl From<Buffer> for UBig {
    /// If the Buffer was allocated with `Buffer::allocate(n)`
    /// and the normalized length is between `n - 2` and `n + 2`
    /// (or even approximately between `0.9 * n` and `1.125 * n`),
    /// there will be no reallocation here.
    fn from(mut buffer: Buffer) -> UBig {
        buffer.pop_leading_zeros();

        let len = buffer.len();
        match len {
            0 => UBig::from_word(0),
            1 => UBig::from_word(buffer[0]),
            _ if len > UBig::MAX_LEN => UBig::panic_number_too_large(),
            _ => {
                buffer.shrink();
                UBig(Large(buffer))
            }
        }
    }
}

// FIXME: ibig makes invalid assumptions about how Vec works, we need to audit this.
// #[cfg(test)]
// mod tests {
//     use super::*;

//     /// Current capacity in Words.
//     fn capacity(x: &UBig) -> usize {
//         match x.repr() {
//             Small(_) => 1,
//             Large(large) => large.capacity(),
//         }
//     }

//     #[test]
//     fn test_buffer_to_ubig() {
//         let buf = Buffer::allocate(5);
//         let num: UBig = buf.into();
//         assert_eq!(num, UBig::from_word(0));

//         let mut buf = Buffer::allocate(5);
//         buf.push(7);
//         let num: UBig = buf.into();
//         assert_eq!(num, UBig::from_word(7));

//         let mut buf = Buffer::allocate(100);
//         buf.push(7);
//         buf.push(0);
//         buf.push(0);
//         let num: UBig = buf.into();
//         assert_eq!(num, UBig::from_word(7));

//         let mut buf = Buffer::allocate(5);
//         buf.push(1);
//         buf.push(2);
//         buf.push(3);
//         buf.push(4);
//         let num: UBig = buf.into();
//         assert_eq!(capacity(&num), 7);

//         let mut buf = Buffer::allocate(100);
//         buf.push(1);
//         buf.push(2);
//         buf.push(3);
//         buf.push(4);
//         let num: UBig = buf.into();
//         assert_eq!(capacity(&num), 6);
//     }

//     #[test]
//     fn test_clone() {
//         let a = UBig::from_word(5);
//         assert_eq!(a.clone(), a);

//         let a = gen_ubig(10);
//         let b = a.clone();
//         assert_eq!(a, b);
//         assert_eq!(capacity(&a), capacity(&b));
//     }

//     #[test]
//     fn test_clone_from() {
//         let num: UBig = gen_ubig(10);

//         let mut a = UBig::from_word(3);
//         a.clone_from(&num);
//         assert_eq!(a, num);
//         let b = UBig::from_word(7);
//         a.clone_from(&b);
//         assert_eq!(a, b);
//         a.clone_from(&b);
//         assert_eq!(a, b);

//         let mut a = gen_ubig(9);
//         let prev_cap = capacity(&a);
//         a.clone_from(&num);
//         // The buffer should be reused, 9 is close enough to 10.
//         assert_eq!(capacity(&a), prev_cap);
//         assert_ne!(capacity(&a), capacity(&num));

//         let mut a = gen_ubig(2);
//         let prev_cap = capacity(&a);
//         a.clone_from(&num);
//         // The buffer should now be reallocated, it's too small.
//         assert_ne!(capacity(&a), prev_cap);
//         assert_eq!(capacity(&a), capacity(&num));

//         let mut a = gen_ubig(100);
//         let prev_cap = capacity(&a);
//         a.clone_from(&num);
//         // The buffer should now be reallocated, it's too large.
//         assert_ne!(capacity(&a), prev_cap);
//         assert_eq!(capacity(&a), capacity(&num));
//     }

//     fn gen_ubig(num_words: u16) -> UBig {
//         let mut buf = Buffer::allocate(num_words.into());
//         for i in 0..num_words {
//             buf.push(i.into());
//         }
//         buf.into()
//     }
// }
