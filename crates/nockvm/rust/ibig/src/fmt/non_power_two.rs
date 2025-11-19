//! Format in a non-power-of-two radix.

use alloc::vec::Vec;
use core::fmt::{self, Formatter};
use core::mem;

use static_assertions::const_assert;

use crate::arch::word::Word;
use crate::div;
use crate::fmt::digit_writer::DigitWriter;
use crate::fmt::{InRadixFull, PreparedForFormatting};
use crate::ops::DivRem;
use crate::radix::{self, Digit};
use crate::ubig::Repr::*;
use crate::ubig::UBig;

/// Format in chunks of CHUNK_LEN * digits_per_word.
const CHUNK_LEN: usize = 16;

impl InRadixFull<'_> {
    pub(crate) fn fmt_non_power_two(&self, f: &mut Formatter) -> fmt::Result {
        debug_assert!(radix::is_radix_valid(self.radix) && !self.radix.is_power_of_two());
        match self.magnitude.repr() {
            Small(word) => {
                let mut prepared = PreparedWord::new(*word, self.radix, 1);
                self.format_prepared(f, &mut prepared)
            }
            Large(buffer) => {
                let radix_info = radix::radix_info(self.radix);
                let max_digits = buffer.len() * (radix_info.digits_per_word + 1);
                if max_digits <= CHUNK_LEN * radix_info.digits_per_word {
                    let mut prepared = PreparedMedium::new(self.magnitude, self.radix);
                    self.format_prepared(f, &mut prepared)
                } else {
                    let mut prepared = PreparedLarge::new(self.magnitude, self.radix);
                    self.format_prepared(f, &mut prepared)
                }
            }
        }
    }
}

/// A `Word` prepared for formatting.
struct PreparedWord {
    // digits[start_index..] actually used.
    digits: [u8; radix::MAX_WORD_DIGITS_NON_POW_2],
    start_index: usize,
}

impl PreparedWord {
    /// Prepare a `Word` for formatting.
    fn new(mut word: Word, radix: Digit, min_digits: usize) -> PreparedWord {
        debug_assert!(radix::is_radix_valid(radix) && !radix.is_power_of_two());
        let radix_info = radix::radix_info(radix);

        let mut prepared = PreparedWord {
            digits: [0; radix::MAX_WORD_DIGITS_NON_POW_2],
            start_index: radix::MAX_WORD_DIGITS_NON_POW_2,
        };

        let max_start = radix::MAX_WORD_DIGITS_NON_POW_2 - min_digits;
        while prepared.start_index > max_start || word != 0 {
            let (new_word, d) = radix_info.fast_div_radix.div_rem(word);
            word = new_word;
            prepared.start_index -= 1;
            prepared.digits[prepared.start_index] = d as u8;
        }

        prepared
    }
}

impl PreparedForFormatting for PreparedWord {
    fn width(&self) -> usize {
        radix::MAX_WORD_DIGITS_NON_POW_2 - self.start_index
    }

    fn write(&mut self, digit_writer: &mut DigitWriter) -> fmt::Result {
        digit_writer.write(&self.digits[self.start_index..])
    }
}

/// A medium number prepared for formatting.
/// Must have no more than CHUNK_LEN * digits_per_word digits.
struct PreparedMedium {
    top_group: PreparedWord,
    // Little endian in groups of digits_per_word.
    low_groups: [Word; CHUNK_LEN],
    num_low_groups: usize,
    radix: Digit,
}

impl PreparedMedium {
    /// Prepare a medium number for formatting.
    fn new(number: &UBig, radix: Digit) -> PreparedMedium {
        debug_assert!(radix::is_radix_valid(radix) && !radix.is_power_of_two());
        let radix_info = radix::radix_info(radix);

        let (mut buffer, mut buffer_len) = ubig_to_chunk_buffer(number);

        let mut low_groups = [0; CHUNK_LEN];
        let mut num_low_groups = 0;

        while buffer_len > 1 {
            let rem = div::fast_div_by_word_in_place(
                &mut buffer[..buffer_len],
                radix_info.range_per_word,
                radix_info.fast_div_range_per_word,
            );
            low_groups[num_low_groups] = rem;
            num_low_groups += 1;

            while buffer[buffer_len - 1] == 0 {
                buffer_len -= 1;
            }
        }
        assert!(buffer_len == 1);
        PreparedMedium {
            top_group: PreparedWord::new(buffer[0], radix, 1),
            low_groups,
            num_low_groups,
            radix,
        }
    }
}

impl PreparedForFormatting for PreparedMedium {
    fn width(&self) -> usize {
        let radix_info = radix::radix_info(self.radix);
        self.top_group.width() + self.num_low_groups * radix_info.digits_per_word
    }

    fn write(&mut self, digit_writer: &mut DigitWriter) -> fmt::Result {
        let radix_info = radix::radix_info(self.radix);

        self.top_group.write(digit_writer)?;

        for group_word in self.low_groups[..self.num_low_groups].iter().rev() {
            let mut prepared =
                PreparedWord::new(*group_word, self.radix, radix_info.digits_per_word);
            prepared.write(digit_writer)?;
        }
        Ok(())
    }
}

/// A large number prepared for formatting.
struct PreparedLarge {
    top_chunk: PreparedMedium,
    // radix^((digits_per_word * CHUNK_LEN) << i)
    radix_powers: Vec<UBig>,
    // little endian chunks: (i, (digits_per_word * CHUNK_LEN)<<i digit number)
    // decreasing in size, so there is a logarithmic number of them
    big_chunks: Vec<(usize, UBig)>,
    radix: Digit,
}

impl PreparedLarge {
    /// Prepare a medium number for formatting in a non-power-of-2 radix.
    fn new(number: &UBig, radix: Digit) -> PreparedLarge {
        debug_assert!(radix::is_radix_valid(radix) && !radix.is_power_of_two());
        let radix_info = radix::radix_info(radix);

        let mut radix_powers = Vec::new();
        let mut big_chunks = Vec::new();
        // FIXME: This use of ibig's pow will leak memory.
        let chunk_power = UBig::from_word(radix_info.range_per_word).pow(CHUNK_LEN);
        if chunk_power > *number {
            return PreparedLarge {
                top_chunk: PreparedMedium::new(number, radix),
                radix_powers,
                big_chunks,
                radix,
            };
        }

        radix_powers.push(chunk_power);
        loop {
            let prev = radix_powers.last().unwrap_or_else(|| {
                panic!(
                    "Panicked at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            // Avoid multiplication if we know prev * prev > number just by looking at lengths.
            if 2 * prev.len() - 1 > number.len() {
                break;
            }
            // 2 * prev.len() is at most 1 larger than number.len().
            // It won't overflow because UBig::MAX_LEN is even.
            const_assert!(UBig::MAX_LEN % 2 == 0);
            let new = prev * prev;
            if new > *number {
                break;
            }
            radix_powers.push(new);
        }

        let mut power_iter = radix_powers.iter().enumerate().rev();
        let mut x = {
            let (i, p) = power_iter.next().unwrap_or_else(|| {
                panic!(
                    "Panicked at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            let (q, r) = number.div_rem(p);
            big_chunks.push((i, r));
            q
        };
        for (i, p) in power_iter {
            if x >= *p {
                let (q, r) = x.div_rem(p);
                big_chunks.push((i, r));
                x = q;
            }
        }

        PreparedLarge {
            top_chunk: PreparedMedium::new(&x, radix),
            radix_powers,
            big_chunks,
            radix,
        }
    }

    /// Write (digits_per_word * CHUNK_LEN) << i digits.
    fn write_big_chunk(&self, digit_writer: &mut DigitWriter, i: usize, x: UBig) -> fmt::Result {
        if i == 0 {
            self.write_chunk(digit_writer, x)
        } else {
            let (q, r) = x.div_rem(&self.radix_powers[i - 1]);
            self.write_big_chunk(digit_writer, i - 1, q)?;
            self.write_big_chunk(digit_writer, i - 1, r)
        }
    }

    /// Write digits_per_word * CHUNK_LEN digits.
    fn write_chunk(&self, digit_writer: &mut DigitWriter, x: UBig) -> fmt::Result {
        let radix_info = radix::radix_info(self.radix);
        let (mut buffer, mut buffer_len) = ubig_to_chunk_buffer(&x);

        let mut groups = [0; CHUNK_LEN];

        for group in groups.iter_mut() {
            *group = div::fast_div_by_word_in_place(
                &mut buffer[..buffer_len],
                radix_info.range_per_word,
                radix_info.fast_div_range_per_word,
            );
            while buffer_len != 0 && buffer[buffer_len - 1] == 0 {
                buffer_len -= 1;
            }
        }
        assert_eq!(buffer_len, 0);

        for group in groups.iter().rev() {
            let mut prepared = PreparedWord::new(*group, self.radix, radix_info.digits_per_word);
            prepared.write(digit_writer)?;
        }

        Ok(())
    }
}

impl PreparedForFormatting for PreparedLarge {
    fn width(&self) -> usize {
        let mut num_digits = self.top_chunk.width();
        let radix_info = radix::radix_info(self.radix);
        for (i, _) in &self.big_chunks {
            num_digits += (radix_info.digits_per_word * CHUNK_LEN) << i;
        }
        num_digits
    }

    fn write(&mut self, digit_writer: &mut DigitWriter) -> fmt::Result {
        self.top_chunk.write(digit_writer)?;

        let mut big_chunks = mem::take(&mut self.big_chunks);
        for (i, val) in big_chunks.drain(..).rev() {
            self.write_big_chunk(digit_writer, i, val)?;
        }
        Ok(())
    }
}

fn ubig_to_chunk_buffer(x: &UBig) -> ([Word; CHUNK_LEN], usize) {
    let mut buffer = [0; CHUNK_LEN];
    let words = x.as_words();
    let buffer_len = words.len();
    buffer[..buffer_len].copy_from_slice(words);
    (buffer, buffer_len)
}
