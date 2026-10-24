//! Operators on the sign of [IBig].

use core::ops::Neg;

use crate::ibig::IBig;
use crate::ops::{Abs, UnsignedAbs};
use crate::ubig::UBig;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub(crate) enum Sign {
    Positive,
    Negative,
}

use Sign::*;

impl Neg for Sign {
    type Output = Sign;

    #[inline]
    fn neg(self) -> Sign {
        match self {
            Positive => Negative,
            Negative => Positive,
        }
    }
}

impl IBig {
    /// A number representing the sign of `self`.
    ///
    /// * -1 if the number is negative
    /// * 0 if the number is zero
    /// * 1 if the number is positive
    ///
    /// # Examples
    /// ```
    /// # use ibig::ibig;
    /// assert_eq!(ibig!(-500).signum(), ibig!(-1));
    /// ```
    #[inline]
    pub fn signum(&self) -> IBig {
        match self.sign() {
            Positive => {
                if *self.magnitude() == UBig::from_word(0) {
                    IBig::from(0u8)
                } else {
                    IBig::from(1u8)
                }
            }
            Negative => IBig::from(-1i8),
        }
    }
}

impl Neg for IBig {
    type Output = IBig;

    #[inline]
    fn neg(self) -> IBig {
        let (sign, mag) = self.into_sign_magnitude();
        IBig::from_sign_magnitude(-sign, mag)
    }
}

impl Neg for &IBig {
    type Output = IBig;

    #[inline]
    fn neg(self) -> IBig {
        self.clone().neg()
    }
}

impl Abs for IBig {
    type Output = IBig;

    #[inline]
    fn abs(self) -> IBig {
        IBig::from(self.unsigned_abs())
    }
}

impl Abs for &IBig {
    type Output = IBig;

    #[inline]
    fn abs(self) -> IBig {
        IBig::from(self.unsigned_abs())
    }
}

impl UnsignedAbs for IBig {
    type Output = UBig;

    #[inline]
    fn unsigned_abs(self) -> UBig {
        let (_, mag) = self.into_sign_magnitude();
        mag
    }
}

impl UnsignedAbs for &IBig {
    type Output = UBig;

    #[inline]
    fn unsigned_abs(self) -> UBig {
        self.magnitude().clone()
    }
}
