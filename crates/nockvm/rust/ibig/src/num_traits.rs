//! Implement num-traits traits.

use crate::error::ParseError;
use crate::ibig::IBig;
use crate::ops::Abs;
use crate::ubig::UBig;

impl num_traits::Zero for UBig {
    #[inline]
    fn zero() -> Self {
        Self::from(0u8)
    }

    #[inline]
    fn is_zero(&self) -> bool {
        *self == Self::from(0u8)
    }
}

impl num_traits::Zero for IBig {
    #[inline]
    fn zero() -> Self {
        Self::from(0u8)
    }

    #[inline]
    fn is_zero(&self) -> bool {
        *self == Self::from(0u8)
    }
}

impl num_traits::One for UBig {
    #[inline]
    fn one() -> Self {
        Self::from(1u8)
    }
}

impl num_traits::One for IBig {
    #[inline]
    fn one() -> Self {
        Self::from(1u8)
    }
}

impl num_traits::Pow<usize> for &UBig {
    type Output = UBig;

    #[inline]
    // FIXME: This use of ibig's pow will leak memory.
    fn pow(self, rhs: usize) -> UBig {
        self.pow(rhs)
    }
}

impl num_traits::Pow<usize> for &IBig {
    type Output = IBig;

    #[inline]
    // FIXME: This use of ibig's pow will leak memory.
    fn pow(self, rhs: usize) -> IBig {
        self.pow(rhs)
    }
}

impl num_traits::Unsigned for UBig {}

impl num_traits::Signed for IBig {
    #[inline]
    fn abs(&self) -> Self {
        Abs::abs(self)
    }

    #[inline]
    fn abs_sub(&self, other: &Self) -> Self {
        Abs::abs(self - other)
    }

    #[inline]
    fn signum(&self) -> Self {
        self.signum()
    }

    #[inline]
    fn is_positive(&self) -> bool {
        *self > IBig::from(0u8)
    }

    #[inline]
    fn is_negative(&self) -> bool {
        *self < IBig::from(0u8)
    }
}

impl num_traits::Num for UBig {
    type FromStrRadixErr = ParseError;

    fn from_str_radix(s: &str, radix: u32) -> Result<Self, ParseError> {
        Self::from_str_radix(s, radix)
    }
}

impl num_traits::Num for IBig {
    type FromStrRadixErr = ParseError;

    fn from_str_radix(s: &str, radix: u32) -> Result<Self, ParseError> {
        Self::from_str_radix(s, radix)
    }
}
