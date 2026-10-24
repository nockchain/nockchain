//! Exponentiation.

use crate::ibig::IBig;
use crate::memory::Stack;
use crate::primitive::PrimitiveUnsigned;
use crate::sign::Sign::*;
use crate::ubig::Repr::*;
use crate::ubig::UBig;

impl UBig {
    /// Raises self to the power of `exp`.
    ///
    /// # Example
    ///
    /// ```
    /// # use ibig::ubig;
    /// assert_eq!(ubig!(3).pow(3), ubig!(27));
    /// ```
    #[inline]
    #[deprecated(
        note = "This uses global allocator. Use pow_stack instead to prevent memory leaks"
    )]
    pub fn pow(&self, exp: usize) -> UBig {
        match exp {
            0 => return UBig::from_word(1),
            1 => return self.clone(),
            2 => return self * self,
            _ => {}
        }
        match self.repr() {
            Small(0) => return UBig::from_word(0),
            Small(1) => return UBig::from_word(1),
            Small(2) => {
                let mut x = UBig::from_word(0);
                x.set_bit(exp);
                return x;
            }
            _ => {}
        }
        let mut p = usize::BIT_SIZE - 2 - exp.leading_zeros();
        let mut res = self * self;
        loop {
            if exp & (1 << p) != 0 {
                res *= self;
            }
            if p == 0 {
                break;
            }
            p -= 1;
            res = &res * &res;
        }
        res
    }

    /// Raises self to the power of `exp`, allocating via the provided `stack`.
    #[inline]
    pub fn pow_stack<S: Stack>(&self, stack: &mut S, exp: usize) -> UBig {
        match exp {
            0 => return UBig::from_word(1),
            1 => return self.clone_stack(stack),
            2 => {
                let a = self.clone_stack(stack);
                let b = self.clone_stack(stack);
                return UBig::mul_stack(stack, a, b);
            }
            _ => {}
        }
        match self.repr() {
            Small(0) => return UBig::from_word(0),
            Small(1) => return UBig::from_word(1),
            Small(2) => {
                let mut x = UBig::from_word(0);
                x.set_bit(exp);
                return x;
            }
            _ => {}
        }

        // Exponentiation by squaring using stack-aware multiplication
        let mut p = usize::BIT_SIZE - 2 - exp.leading_zeros();
        let a = self.clone_stack(stack);
        let b = self.clone_stack(stack);
        let mut res = UBig::mul_stack(stack, a, b);
        loop {
            if exp & (1 << p) != 0 {
                let c = self.clone_stack(stack);
                res = UBig::mul_stack(stack, res, c);
            }
            if p == 0 {
                break;
            }
            p -= 1;
            // Need to clone res for both arguments
            let tmp1 = res.clone_stack(stack);
            let tmp2 = res;
            res = UBig::mul_stack(stack, tmp1, tmp2);
        }
        res
    }
}

impl IBig {
    /// Raises self to the power of `exp`.
    ///
    /// # Example
    ///
    /// ```
    /// # use ibig::ibig;
    /// assert_eq!(ibig!(-3).pow(3), ibig!(-27));
    /// ```
    #[inline]
    #[deprecated(
        note = "This uses global allocator. Use pow_stack instead to prevent memory leaks"
    )]
    pub fn pow(&self, exp: usize) -> IBig {
        let sign = if self.sign() == Negative && exp % 2 == 1 {
            Negative
        } else {
            Positive
        };
        IBig::from_sign_magnitude(sign, self.magnitude().pow(exp))
    }
}
