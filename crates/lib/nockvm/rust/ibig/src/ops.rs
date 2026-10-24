//! Traits for number operations.

/// Bitwise AND NOT operation.
///
/// `x.and_not(y)` is equivalent to `x & !y`. For [UBig](crate::UBig) the latter is not a
/// valid expression because the `!` operator is not defined.
///
/// # Examples
///
/// ```
/// # use ibig::{ops::AndNot, ubig};
/// assert_eq!(ubig!(0xff).and_not(ubig!(0x1111)), ubig!(0xee));
/// ```
pub trait AndNot<Rhs = Self> {
    type Output;

    fn and_not(self, rhs: Rhs) -> Self::Output;
}

/// Next power of two.
///
/// # Examples
/// ```
/// # use ibig::{ops::NextPowerOfTwo, ubig};
/// assert_eq!(ubig!(5).next_power_of_two(), ubig!(8));
/// ```
pub trait NextPowerOfTwo {
    type Output;

    fn next_power_of_two(self) -> Self::Output;
}

/// Absolute value.
///
/// # Examples
/// ```
/// # use ibig::{ibig, ops::Abs};
/// assert_eq!(ibig!(-5).abs(), ibig!(5));
/// ```
pub trait Abs {
    type Output;

    fn abs(self) -> Self::Output;
}

/// Unsigned absolute value.
///
/// # Examples
/// ```
/// # use ibig::{ibig, ops::UnsignedAbs, ubig};
/// assert_eq!(ibig!(-5).unsigned_abs(), ubig!(5));
/// ```
pub trait UnsignedAbs {
    type Output;

    fn unsigned_abs(self) -> Self::Output;
}

/// Compute quotient and remainder at the same time.
///
/// # Example
/// ```
/// # use ibig::{ops::DivRem, ubig};
/// assert_eq!(ubig!(23).div_rem(ubig!(10)), (ubig!(2), ubig!(3)));
/// ```
pub trait DivRem<Rhs = Self> {
    type OutputDiv;
    type OutputRem;

    fn div_rem(self, rhs: Rhs) -> (Self::OutputDiv, Self::OutputRem);
}

/// Compute Euclidean quotient.
///
/// # Example
/// ```
/// # use ibig::{ibig, ops::DivEuclid};
/// assert_eq!(ibig!(-23).div_euclid(ibig!(10)), ibig!(-3));
/// ```
pub trait DivEuclid<Rhs = Self> {
    type Output;

    fn div_euclid(self, rhs: Rhs) -> Self::Output;
}

/// Compute Euclidean remainder.
///
/// # Example
/// ```
/// # use ibig::{ibig, ops::RemEuclid};
/// assert_eq!(ibig!(-23).rem_euclid(ibig!(10)), ibig!(7));
/// ```
pub trait RemEuclid<Rhs = Self> {
    type Output;

    fn rem_euclid(self, rhs: Rhs) -> Self::Output;
}

/// Compute Euclidean quotient and remainder at the same time.
///
/// # Example
/// ```
/// # use ibig::{ibig, ops::DivRemEuclid};
/// assert_eq!(ibig!(-23).div_rem_euclid(ibig!(10)), (ibig!(-3), ibig!(7)));
/// ```
pub trait DivRemEuclid<Rhs = Self> {
    type OutputDiv;
    type OutputRem;

    fn div_rem_euclid(self, rhs: Rhs) -> (Self::OutputDiv, Self::OutputRem);
}
