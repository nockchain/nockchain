use std::fmt;

use bincode::{Decode, Encode};
use thiserror::Error;

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum OffsetError {
    #[error("word offset {words} does not fit in u64")]
    TooLargeForU64 { words: usize },
    #[error("word offset {words} does not fit in usize")]
    TooLargeForUsize { words: u64 },
}

pub trait WordOffset: Copy + Eq + Ord + fmt::Debug + Sized {
    fn from_words(words: u64) -> Self;
    fn words(self) -> u64;

    fn zero() -> Self {
        Self::from_words(0)
    }

    fn checked_add(self, rhs: Self) -> Option<Self> {
        self.words().checked_add(rhs.words()).map(Self::from_words)
    }

    fn checked_add_words(self, rhs: u64) -> Option<Self> {
        self.words().checked_add(rhs).map(Self::from_words)
    }

    fn checked_sub(self, rhs: Self) -> Option<Self> {
        self.words().checked_sub(rhs.words()).map(Self::from_words)
    }

    fn checked_sub_words(self, rhs: u64) -> Option<Self> {
        self.words().checked_sub(rhs).map(Self::from_words)
    }

    fn checked_bytes(self) -> Option<u64> {
        self.words().checked_mul(8)
    }

    fn checked_bytes_usize(self) -> Option<usize> {
        self.checked_bytes()
            .and_then(|bytes| usize::try_from(bytes).ok())
    }

    fn try_from_usize(words: usize) -> Result<Self, OffsetError> {
        let words = u64::try_from(words).map_err(|_| OffsetError::TooLargeForU64 { words })?;
        Ok(Self::from_words(words))
    }

    fn try_into_usize(self) -> Result<usize, OffsetError> {
        usize::try_from(self.words()).map_err(|_| OffsetError::TooLargeForUsize {
            words: self.words(),
        })
    }
}

macro_rules! define_offset_words {
    ($name:ident) => {
        #[repr(transparent)]
        #[derive(
            Clone, Copy, Debug, Default, Encode, Decode, PartialEq, Eq, PartialOrd, Ord, Hash,
        )]
        pub struct $name(u64);

        impl $name {
            pub const fn from_words(words: u64) -> Self {
                Self(words)
            }

            pub const fn words(self) -> u64 {
                self.0
            }

            pub fn try_from_usize(words: usize) -> Result<Self, OffsetError> {
                <Self as WordOffset>::try_from_usize(words)
            }

            pub fn try_into_usize(self) -> Result<usize, OffsetError> {
                <Self as WordOffset>::try_into_usize(self)
            }

            pub fn checked_add(self, rhs: Self) -> Option<Self> {
                <Self as WordOffset>::checked_add(self, rhs)
            }

            pub fn checked_add_words(self, rhs: u64) -> Option<Self> {
                <Self as WordOffset>::checked_add_words(self, rhs)
            }

            pub fn checked_sub(self, rhs: Self) -> Option<Self> {
                <Self as WordOffset>::checked_sub(self, rhs)
            }

            pub fn checked_sub_words(self, rhs: u64) -> Option<Self> {
                <Self as WordOffset>::checked_sub_words(self, rhs)
            }

            pub fn checked_bytes(self) -> Option<u64> {
                <Self as WordOffset>::checked_bytes(self)
            }

            pub fn checked_bytes_usize(self) -> Option<usize> {
                <Self as WordOffset>::checked_bytes_usize(self)
            }
        }

        impl WordOffset for $name {
            fn from_words(words: u64) -> Self {
                Self(words)
            }

            fn words(self) -> u64 {
                self.0
            }
        }

        impl TryFrom<usize> for $name {
            type Error = OffsetError;

            fn try_from(words: usize) -> Result<Self, Self::Error> {
                Self::try_from_usize(words)
            }
        }

        impl From<$name> for u64 {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

define_offset_words!(StackOffsetWords);
define_offset_words!(PmaOffsetWords);
