use alloc::vec::Vec;
use core::fmt::{self, Formatter};

use serde::de::{Deserialize, Deserializer, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeSeq, Serializer};
use static_assertions::const_assert;

use crate::arch::word::Word;
use crate::buffer::Buffer;
use crate::ibig::IBig;
use crate::primitive::WORD_BITS_USIZE;
use crate::ubig::UBig;

const_assert!(64 % WORD_BITS_USIZE == 0);
const WORDS_PER_U64: usize = 64 / WORD_BITS_USIZE;

impl Serialize for UBig {
    #[allow(clippy::useless_conversion)]
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let chunks = self.as_words().chunks(WORDS_PER_U64);
        let mut seq = serializer.serialize_seq(Some(chunks.len()))?;
        for chunk in chunks {
            let mut word_u64: u64 = 0;
            for (i, word) in chunk.iter().enumerate() {
                word_u64 |= u64::from(*word) << (i * WORD_BITS_USIZE);
            }
            seq.serialize_element(&word_u64)?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for UBig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_seq(UBigVisitor)
    }
}

struct UBigVisitor;

impl<'de> Visitor<'de> for UBigVisitor {
    type Value = UBig;

    fn expecting(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "a sequence of 64-bit words")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<UBig, A::Error> {
        match seq.size_hint() {
            Some(0) => {
                assert!(seq.next_element::<u64>()?.is_none());
                Ok(UBig::from_word(0))
            }
            Some(1) => {
                let word_64: u64 = seq.next_element()?.expect(&format!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    env!("GIT_SHA")
                ));
                assert!(seq.next_element::<u64>()?.is_none());
                Ok(UBig::from(word_64))
            }
            Some(num_words_64) => {
                let mut buffer = Buffer::allocate(len_64_to_max_len(num_words_64));
                for _ in 0..num_words_64 {
                    let word_64: u64 = seq.next_element()?.expect(&format!(
                        "Called `expect()` at {}:{} (git sha: {})",
                        file!(),
                        line!(),
                        env!("GIT_SHA")
                    ));
                    push_word_64(&mut buffer, word_64);
                }
                assert!(seq.next_element::<u64>()?.is_none());
                Ok(buffer.into())
            }
            None => {
                let mut words_64 = Vec::new();
                while let Some(word_64) = seq.next_element()? {
                    words_64.push(word_64);
                }
                let mut buffer = Buffer::allocate(len_64_to_max_len(words_64.len()));
                for word_64 in words_64 {
                    push_word_64(&mut buffer, word_64);
                }
                Ok(buffer.into())
            }
        }
    }
}

fn push_word_64(buffer: &mut Buffer, word_64: u64) {
    for i in 0..WORDS_PER_U64 {
        buffer.push((word_64 >> (i * WORD_BITS_USIZE)) as Word);
    }
}

#[allow(clippy::absurd_extreme_comparisons)]
fn len_64_to_max_len(len_64: usize) -> usize {
    // Make sure we always have enough space for leading zero Words.
    const_assert!(Buffer::MAX_CAPACITY - UBig::MAX_LEN >= WORDS_PER_U64 - 1);
    #[allow(clippy::redundant_closure)]
    len_64
        .checked_mul(WORDS_PER_U64)
        .unwrap_or_else(|| UBig::panic_number_too_large())
}

impl Serialize for IBig {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (self.sign(), self.magnitude()).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for IBig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (sign, magnitude) = Deserialize::deserialize(deserializer)?;
        Ok(IBig::from_sign_magnitude(sign, magnitude))
    }
}
