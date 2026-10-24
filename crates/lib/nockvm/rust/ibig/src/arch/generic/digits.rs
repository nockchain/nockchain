use crate::arch::word::Word;
use crate::primitive::WORD_BYTES;
use crate::radix::DigitCase;

/// Chunk length for digit conversion.
pub(crate) const DIGIT_CHUNK_LEN: usize = WORD_BYTES;

/// Convert raw digits to ASCII.
///
/// digits must be valid
#[inline]
pub(crate) fn digit_chunk_raw_to_ascii(digits: &mut [u8; DIGIT_CHUNK_LEN], digit_case: DigitCase) {
    let mut word = Word::from_ne_bytes(*digits);

    // ALL_ONES = 0x01010101...
    const ALL_ONES: Word = Word::MAX / 0xff;

    // For digits >= 10, add 'a'-'0' or 'A'-'0' as appropriate.
    if digit_case != DigitCase::NoLetters {
        // Find digits >= 10, in parallel.
        // 0x76 + digit will have the top bit set if digit >= 10.
        // letters: 0x01 if digit >= 10.
        let letters = ((0x76 * ALL_ONES + word) >> 7) & ALL_ONES;

        word += letters * (digit_case as Word);
    }

    // Convert digits to ASCII in parallel.
    word += ALL_ONES * (b'0' as Word);

    digits.copy_from_slice(&word.to_ne_bytes());
}
