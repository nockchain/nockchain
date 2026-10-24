//! Greatest common divisor.

use core::mem;

use crate::ibig::IBig;
use crate::ops::DivRem;
use crate::ubig::UBig;

impl UBig {
    /// Greatest common divisor.
    ///
    /// # Example
    ///
    /// ```
    /// # use ibig::ubig;
    /// assert_eq!(ubig!(12).gcd(&ubig!(18)), ubig!(6));
    /// ```
    ///
    /// # Panics
    ///
    /// `ubig!(0).gcd(&ubig!(0))` panics.
    pub fn gcd(&self, rhs: &UBig) -> UBig {
        let (mut a, mut b) = (self.clone(), rhs.clone());

        let zeros = match (a.trailing_zeros(), b.trailing_zeros()) {
            (None, None) => panic!("gcd(0, 0)"),
            (None, Some(_)) => return b,
            (Some(_), None) => return a,
            (Some(a_zeros), Some(b_zeros)) => {
                a >>= a_zeros;
                b >>= b_zeros;
                a_zeros.min(b_zeros)
            }
        };

        // One round of Euclidean algorithm.
        if a < b {
            mem::swap(&mut a, &mut b);
        }
        a %= &b;

        // Binary algorithm.
        loop {
            // b is odd
            match a.trailing_zeros() {
                None => break,
                Some(a_zeros) => a >>= a_zeros,
            }
            // a is odd

            if a < b {
                mem::swap(&mut a, &mut b);
            }
            a -= &b;
        }

        b << zeros
    }

    /// Greatest common divisors and the Bézout coefficients.
    ///
    /// If `a.extended_gcd(&b) == (g, x, y)` then:
    /// * `x * a + y * b == g`
    /// * `abs(x) <= max(b, 1)`
    /// * `abs(y) <= max(a, 1)`
    ///
    /// # Example
    /// ```
    /// # use ibig::{ubig, IBig, ops::UnsignedAbs};
    /// let a = ubig!(12);
    /// let b = ubig!(18);
    /// let (g, x, y) = a.extended_gcd(&b);
    /// assert_eq!(&a % &g, ubig!(0));
    /// assert_eq!(&b % &g, ubig!(0));
    /// assert_eq!(&x * IBig::from(&a) + &y * IBig::from(&b), IBig::from(g));
    /// assert!(x.unsigned_abs() <= b);
    /// assert!(y.unsigned_abs() <= a);
    /// ```
    ///
    /// # Panics
    ///
    /// `ubig!(0).extended_gcd(&ubig!(0))` panics.
    pub fn extended_gcd(&self, rhs: &UBig) -> (UBig, IBig, IBig) {
        let zeros = match (self.trailing_zeros(), rhs.trailing_zeros()) {
            (None, None) => panic!("extended_gcd(0, 0)"),
            (None, Some(_)) => return (rhs.clone(), 0u8.into(), 1u8.into()),
            (Some(_), None) => return (self.clone(), 1u8.into(), 0u8.into()),
            (Some(a_zeros), Some(b_zeros)) => a_zeros.min(b_zeros),
        };

        let u = self >> zeros;
        let v = rhs >> zeros;
        let mut a;
        let mut b;
        let mut ax;
        let mut ay;
        let mut bx;
        let mut by;

        // Invariants:
        // gcd(a, b) == gcd(u, v)
        // a = ax * u - ay * v
        // b = bx * u - by * v
        // ax, bx <= v
        // ay, by <= u

        // One round of Euclidean algorithm.
        if u <= v {
            let (q, r) = (&v).div_rem(&u);
            // u = 1 * u - 0 * v
            // r = v - q * u = (v-q) * u - (u-1) * v
            a = u.clone();
            ax = UBig::from_word(1);
            ay = UBig::from_word(0);
            b = r;
            bx = &v - q;
            by = &u - UBig::from_word(1);
        } else {
            let (q, r) = (&u).div_rem(&v);
            // v = 0 * u + 1 * v = v * u - (u-1) * v
            // r = 1 * u - q * v
            a = v.clone();
            ax = v.clone();
            ay = &u - UBig::from_word(1);

            b = r;
            bx = UBig::from_word(1);
            by = q;
        }

        // At least one of a and b is odd (because gcd(u, v) is odd). Make b odd.
        if &b & 1u8 == 0u8 {
            mem::swap(&mut a, &mut b);
            mem::swap(&mut ax, &mut bx);
            mem::swap(&mut ay, &mut by);
        }

        // Binary algorithm.
        while a != UBig::from_word(0) {
            // b is odd
            while &a & 1u8 == 0u8 {
                // a is even
                if &ax & 1u8 != 0u8 || &ay & 1u8 != 0u8 {
                    ax += &v;
                    ay += &u;
                }
                // Now ax, ay are even.
                a >>= 1usize;
                ax >>= 1usize;
                ay >>= 1usize;
                // Again ax <= v, bx <= u.
            }
            // Both a and b are odd.
            if a < b {
                mem::swap(&mut a, &mut b);
                mem::swap(&mut ax, &mut bx);
                mem::swap(&mut ay, &mut by);
            }
            a -= &b;
            if ax < bx {
                ax += &v;
                ay += &u;
            }
            ax -= &bx;
            ay -= &by;
            // ax >= 0 in both cases
            // ax <= v in both cases
            // ax * u - ay * v = a
            // ay * v = ax * u - a <= v * u - 0
            // ay <= u
            // After one round Euclidean, and at least one subtraction, a < min(u,v).
            // ay * v = ax * u - a >= -a > -min(u,v) >= -v
            // ay >= 0
        }

        (b << zeros, IBig::from(bx), -IBig::from(by))
    }
}

impl IBig {
    /// Greatest common divisor.
    ///
    /// # Example
    ///
    /// ```
    /// # use ibig::ibig;
    /// assert_eq!(ibig!(-12).gcd(&ibig!(18)), ibig!(6));
    /// ```
    ///
    /// # Panics
    ///
    /// `ibig!(0).gcd(&ibig!(0))` panics.
    pub fn gcd(&self, rhs: &IBig) -> IBig {
        self.magnitude().gcd(rhs.magnitude()).into()
    }

    /// Greatest common divisors and the Bézout coefficients.
    ///
    /// If `a.extended_gcd(&b) == (g, x, y)` then:
    /// * `x * a + y * b == g`
    /// * `abs(x) <= max(abs(b), 1)`
    /// * `abs(y) <= max(abs(a), 1)`
    ///
    /// # Example
    /// ```
    /// # use ibig::{ibig, IBig, ops::Abs};
    /// let a = ibig!(-12);
    /// let b = ibig!(18);
    /// let (g, x, y) = a.extended_gcd(&b);
    /// assert_eq!(&a % &g, ibig!(0));
    /// assert_eq!(&b % &g, ibig!(0));
    /// assert_eq!(&x * &a + &y * &b, g);
    /// assert!(x.abs() <= b.abs());
    /// assert!(y.abs() <= a.abs());
    /// ```
    ///
    /// # Panics
    ///
    /// `ibig!(0).extended_gcd(&ibig!(0))` panics.
    pub fn extended_gcd(&self, rhs: &IBig) -> (IBig, IBig, IBig) {
        let (g, x, y) = self.magnitude().extended_gcd(rhs.magnitude());
        (IBig::from(g), self.sign() * x, rhs.sign() * y)
    }
}
