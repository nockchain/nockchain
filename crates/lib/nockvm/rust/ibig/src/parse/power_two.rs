//! Parse in a power-of-two radix.

use crate::arch::word::Word;
use crate::buffer::Buffer;
use crate::error::ParseError;
use crate::primitive::{WORD_BITS, WORD_BITS_USIZE};
use crate::radix::{self, Digit};
use crate::ubig::UBig;

/// Parse an unsigned string to [UBig].
pub(crate) fn parse(src: &str, radix: Digit) -> Result<UBig, ParseError> {
    debug_assert!(radix::is_radix_valid(radix) && radix.is_power_of_two());
    let radix_info = radix::radix_info(radix);

    if src.len() <= radix_info.digits_per_word {
        let word = parse_word(src, radix)?;
        Ok(UBig::from_word(word))
    } else {
        parse_large(src, radix)
    }
}

/// Parse an unsigned string to `Word`.
///
/// The length of the string must be at most digits_per_word(radix).
fn parse_word(src: &str, radix: Digit) -> Result<Word, ParseError> {
    debug_assert!(radix::is_radix_valid(radix) && radix.is_power_of_two());
    debug_assert!(src.len() <= radix::radix_info(radix).digits_per_word);

    let log_radix = radix.trailing_zeros();
    let mut word = 0;
    let mut bits = 0;
    for byte in src.as_bytes().iter().rev() {
        let digit = radix::digit_from_utf8_byte(*byte, radix).ok_or(ParseError::InvalidDigit)?;
        word |= (digit as Word) << bits;
        bits += log_radix;
    }
    Ok(word)
}

/// Parse an unsigned string to [UBig].
///
/// The result will usually not fit in a single word.
fn parse_large(src: &str, radix: Digit) -> Result<UBig, ParseError> {
    debug_assert!(radix::is_radix_valid(radix) && radix.is_power_of_two());

    let log_radix = radix.trailing_zeros();
    #[allow(clippy::redundant_closure)]
    let num_bits = src
        .len()
        .checked_mul(log_radix as usize)
        .unwrap_or_else(|| UBig::panic_number_too_large());
    let mut buffer = Buffer::allocate((num_bits - 1) / WORD_BITS_USIZE + 1);
    let mut bits = 0;
    let mut word = 0;
    for byte in src.as_bytes().iter().rev() {
        let digit = radix::digit_from_utf8_byte(*byte, radix).ok_or(ParseError::InvalidDigit)?;
        word |= (digit as Word) << bits;
        let new_bits = bits + log_radix;
        if new_bits >= WORD_BITS {
            buffer.push(word);
            word = (digit as Word) >> (WORD_BITS - bits);
            bits = new_bits - WORD_BITS;
        } else {
            bits = new_bits;
        }
    }
    if bits > 0 {
        buffer.push(word);
    }
    Ok(buffer.into())
}
