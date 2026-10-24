//! Format in a power-of-two radix.

use core::fmt::{self, Formatter};

use crate::arch::word::Word;
use crate::fmt::digit_writer::DigitWriter;
use crate::fmt::{InRadixFull, PreparedForFormatting};
use crate::math;
use crate::primitive::{WORD_BITS, WORD_BITS_USIZE};
use crate::radix::{self, Digit};
use crate::ubig::Repr::*;

impl InRadixFull<'_> {
    /// Radix must be a power of 2.
    pub(crate) fn fmt_power_two(&self, f: &mut Formatter) -> fmt::Result {
        debug_assert!(radix::is_radix_valid(self.radix) && self.radix.is_power_of_two());
        match self.magnitude.repr() {
            Small(word) => {
                let mut prepared = PreparedWord::new(*word, self.radix);
                self.format_prepared(f, &mut prepared)
            }
            Large(buffer) => {
                let mut prepared = PreparedLarge::new(buffer, self.radix);
                self.format_prepared(f, &mut prepared)
            }
        }
    }
}

/// A `Word` prepared for formatting.
struct PreparedWord {
    word: Word,
    log_radix: u32,
    width: usize,
}

impl PreparedWord {
    /// Prepare a `Word` for formatting.
    fn new(word: Word, radix: Digit) -> PreparedWord {
        debug_assert!(radix::is_radix_valid(radix) && radix.is_power_of_two());
        let log_radix = radix.trailing_zeros();
        let width = math::ceil_div(math::bit_len(word), log_radix).max(1) as usize;

        PreparedWord {
            word,
            log_radix,
            width,
        }
    }
}

impl PreparedForFormatting for PreparedWord {
    fn width(&self) -> usize {
        self.width
    }

    fn write(&mut self, digit_writer: &mut DigitWriter) -> fmt::Result {
        let mask: Word = math::ones(self.log_radix);
        let mut digits = [0; WORD_BITS_USIZE];
        for idx in 0..self.width {
            let digit = ((self.word >> (idx as u32 * self.log_radix)) & mask) as u8;
            digits[self.width - 1 - idx] = digit;
        }
        digit_writer.write(&digits[..self.width])
    }
}

/// A large number prepared for formatting.
struct PreparedLarge<'a> {
    words: &'a [Word],
    log_radix: u32,
    width: usize,
}

impl PreparedLarge<'_> {
    /// Prepare a large number for formatting.
    fn new(words: &[Word], radix: Digit) -> PreparedLarge {
        debug_assert!(radix::is_radix_valid(radix) && radix.is_power_of_two());
        let log_radix = radix.trailing_zeros();

        // No overflow because words.len() * WORD_BITS <= usize::MAX for
        // words.len() <= Buffer::MAX_CAPACITY.
        let width = math::ceil_div(
            words.len() * WORD_BITS_USIZE
                - words
                    .last()
                    .unwrap_or_else(|| {
                        panic!(
                            "Panicked at {}:{} (git sha: {:?})",
                            file!(),
                            line!(),
                            option_env!("GIT_SHA")
                        )
                    })
                    .leading_zeros() as usize,
            log_radix as usize,
        )
        .max(1);

        PreparedLarge {
            words,
            log_radix,
            width,
        }
    }
}

impl PreparedForFormatting for PreparedLarge<'_> {
    fn width(&self) -> usize {
        self.width
    }

    fn write(&mut self, digit_writer: &mut DigitWriter) -> fmt::Result {
        let mask: Word = math::ones(self.log_radix);

        let mut it = self.words.iter().rev();
        let mut word = it.next().unwrap_or_else(|| {
            panic!(
                "Panicked at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        let mut bits = (self.width * self.log_radix as usize
            - (self.words.len() - 1) * WORD_BITS_USIZE) as u32;

        loop {
            let digit;
            if bits < self.log_radix {
                match it.next() {
                    Some(w) => {
                        let extra_bits = self.log_radix - bits;
                        bits = WORD_BITS - extra_bits;
                        digit = (((word << extra_bits) | (w >> bits)) & mask) as u8;
                        word = w;
                    }
                    None => break,
                }
            } else {
                bits -= self.log_radix;
                digit = ((word >> bits) & mask) as u8;
            }
            digit_writer.write(&[digit])?;
        }
        debug_assert!(bits == 0);
        Ok(())
    }
}
