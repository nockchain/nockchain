use core::convert::TryInto;
use core::ops::{Div, DivAssign};

use crate::arch::word::Word;
use crate::ibig::IBig;
use crate::modular::modulo::{Modulo, ModuloLarge, ModuloRepr, ModuloSmall, ModuloSmallRaw};
use crate::ops::RemEuclid;
use crate::ubig::UBig;

impl<'a> Modulo<'a> {
    /// Inverse.
    ///
    /// Returns `None` if there is no unique inverse.
    ///
    /// # Examples
    ///
    /// ```
    /// # use ibig::{modular::ModuloRing, ubig};
    /// let ring = ModuloRing::new(&ubig!(10));
    /// assert_eq!(ring.from(7).inverse(), Some(ring.from(3)));
    /// assert_eq!(ring.from(2).inverse(), None);
    /// ```
    pub fn inverse(&self) -> Option<Modulo<'a>> {
        match self.repr() {
            ModuloRepr::Small(self_small) => self_small.inverse().map(Into::into),
            ModuloRepr::Large(self_large) => self_large.inverse().map(Into::into),
        }
    }
}

impl<'a> Div<Modulo<'a>> for Modulo<'a> {
    type Output = Modulo<'a>;

    #[inline]
    fn div(self, rhs: Modulo<'a>) -> Modulo<'a> {
        (&self).div(&rhs)
    }
}

impl<'a> Div<&Modulo<'a>> for Modulo<'a> {
    type Output = Modulo<'a>;

    #[inline]
    fn div(self, rhs: &Modulo<'a>) -> Modulo<'a> {
        (&self).div(rhs)
    }
}

impl<'a> Div<Modulo<'a>> for &Modulo<'a> {
    type Output = Modulo<'a>;

    #[inline]
    fn div(self, rhs: Modulo<'a>) -> Modulo<'a> {
        self.div(&rhs)
    }
}

impl<'a> Div<&Modulo<'a>> for &Modulo<'a> {
    type Output = Modulo<'a>;

    #[inline]
    fn div(self, rhs: &Modulo<'a>) -> Modulo<'a> {
        // Clippy doesn't like that division is implemented using multiplication.
        #[allow(clippy::suspicious_arithmetic_impl)]
        match rhs.inverse() {
            None => panic!("Division by a non-invertible Modulo"),
            Some(inv_rhs) => self * inv_rhs,
        }
    }
}

impl<'a> DivAssign<Modulo<'a>> for Modulo<'a> {
    #[inline]
    fn div_assign(&mut self, rhs: Modulo<'a>) {
        self.div_assign(&rhs)
    }
}

impl<'a> DivAssign<&Modulo<'a>> for Modulo<'a> {
    #[inline]
    fn div_assign(&mut self, rhs: &Modulo<'a>) {
        *self = (&*self).div(rhs)
    }
}

impl<'a> ModuloSmall<'a> {
    /// Inverse.
    fn inverse(&self) -> Option<ModuloSmall<'a>> {
        let a = self.residue();
        let b = self.ring().modulus();
        // TODO: Optimized `extended_gcd` for `Word`s.
        // x * a + _ * b == gcd
        let (gcd, x, _) = UBig::from(a).extended_gcd(&UBig::from(b));
        if gcd == UBig::from_word(1) {
            let res: Word = x
                .rem_euclid(IBig::from(b))
                .try_into()
                .unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
            Some(ModuloSmall::new(
                ModuloSmallRaw::from_word(res, self.ring()),
                self.ring(),
            ))
        } else {
            None
        }
    }
}

impl<'a> ModuloLarge<'a> {
    /// Inverse.
    fn inverse(&self) -> Option<ModuloLarge<'a>> {
        let a = self.residue();
        let b = self.ring().modulus();
        let (gcd, x, _) = a.extended_gcd(&b);
        if gcd == UBig::from_word(1) {
            // TODO: Get rid of redundant remainder computations.
            let res: UBig = x
                .rem_euclid(IBig::from(b))
                .try_into()
                .unwrap_or_else(|err| {
                    panic!(
                        "Panicked with err: {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
            Some(ModuloLarge::from_ubig(res, self.ring()))
        } else {
            None
        }
    }
}
