use crate::arch::word::Word;
use crate::ibig::IBig;
use crate::math;
use crate::memory::{self, MemoryAllocation};
use crate::modular::modulo::{Modulo, ModuloLarge, ModuloRepr, ModuloSmall, ModuloSmallRaw};
use crate::modular::modulo_ring::ModuloRingSmall;
use crate::primitive::{
    double_word, split_double_word, PrimitiveUnsigned, WORD_BITS, WORD_BITS_USIZE,
};
use crate::sign::Sign::*;
use crate::ubig::Repr::*;
use crate::ubig::UBig;

impl<'a> Modulo<'a> {
    /// Exponentiation.
    ///
    /// # Examples
    ///
    /// ```
    /// # use ibig::{modular::ModuloRing, ubig};
    /// # #[cfg(miri)]fn main() {}
    /// # #[cfg(not(miri))]
    /// # fn main() {
    /// // A Mersenne prime.
    /// let p = ubig!(2).pow(607) - ubig!(1);
    /// let ring = ModuloRing::new(&p);
    /// // Fermat's little theorem: a^(p-1) = 1 (mod p)
    /// let a = ring.from(123);
    /// assert_eq!(a.pow(&(p - ubig!(1))), ring.from(1));
    /// /// # }
    /// ```
    #[inline]
    pub fn pow(&self, exp: &UBig) -> Modulo<'a> {
        match self.repr() {
            ModuloRepr::Small(self_small) => self_small.pow(exp).into(),
            ModuloRepr::Large(self_large) => self_large.pow(exp).into(),
        }
    }

    /// Exponentiation to a signed exponent.
    ///
    /// # Panic
    ///
    /// Panics if the exponent is negative and the base is not invertible.
    ///
    /// # Examples
    ///
    /// ```
    /// # use ibig::{modular::ModuloRing, ibig, ubig};
    /// let ring = ModuloRing::new(&ubig!(10));
    /// assert_eq!(ring.from(2).pow_signed(&ibig!(4)), ring.from(6));
    /// assert_eq!(ring.from(3).pow_signed(&ibig!(-3)), ring.from(3));
    /// ```
    #[inline]
    pub fn pow_signed(&self, exp: &IBig) -> Modulo<'a> {
        match exp.sign() {
            Positive => self.pow(exp.magnitude()),
            Negative => match self.inverse() {
                None => panic!("Non-invertible Modulo taken to a negative power"),
                Some(inv) => inv.pow(exp.magnitude()),
            },
        }
    }
}

impl ModuloSmallRaw {
    /// self^exp
    #[inline]
    pub(crate) const fn pow_word(self, exp: Word, ring: &ModuloRingSmall) -> ModuloSmallRaw {
        if exp == 0 {
            return ModuloSmallRaw::from_word(1, ring);
        }

        let bits = WORD_BITS - 1 - exp.leading_zeros();
        self.pow_helper(bits, self, exp, ring)
    }

    /// self^2^bits * base^exp[..bits]
    #[inline]
    const fn pow_helper(
        self,
        mut bits: u32,
        base: ModuloSmallRaw,
        exp: Word,
        ring: &ModuloRingSmall,
    ) -> ModuloSmallRaw {
        let mut res = self;
        while bits > 0 {
            res = res.mul(res, ring);
            bits -= 1;
            if exp & (1 << bits) != 0 {
                res = res.mul(base, ring);
            }
        }
        res
    }
}

impl<'a> ModuloSmall<'a> {
    /// Exponentiation.
    #[inline]
    fn pow(&self, exp: &UBig) -> ModuloSmall<'a> {
        match exp.repr() {
            // self^0 == 1
            Small(0) => ModuloSmall::from_ubig(&UBig::from_word(1), self.ring()),
            // self^1 == self
            Small(1) => self.clone(),
            // self^2 == self * self
            Small(2) => {
                let res = self.raw().mul(self.raw(), self.ring());
                ModuloSmall::new(res, self.ring())
            }
            _ => self.pow_nontrivial(exp),
        }
    }

    fn pow_nontrivial(&self, exp: &UBig) -> ModuloSmall<'a> {
        debug_assert!(*exp >= UBig::from_word(3));

        let exp_words = exp.as_words();
        let mut n = exp_words.len() - 1;
        let mut val = self.raw().pow_word(exp_words[n], self.ring());
        while n != 0 {
            n -= 1;
            val = val.pow_helper(WORD_BITS, self.raw(), exp_words[n], self.ring());
        }
        ModuloSmall::new(val, self.ring())
    }
}

impl<'a> ModuloLarge<'a> {
    fn pow(&self, exp: &UBig) -> ModuloLarge<'a> {
        match exp.repr() {
            // self^0 == 1
            Small(0) => ModuloLarge::from_ubig(UBig::from_word(1), self.ring()),
            // self^1 == self
            Small(1) => self.clone(),
            _ => self.pow_nontrivial(exp),
        }
    }

    fn pow_nontrivial(&self, exp: &UBig) -> ModuloLarge<'a> {
        debug_assert!(*exp >= UBig::from_word(2));

        let n = self.ring().normalized_modulus().len();
        let window_len = ModuloLarge::choose_pow_window_len(exp.bit_len());

        // Precomputed table of small odd powers up to 2^window_len, starting from self^3.
        #[allow(clippy::redundant_closure)]
        let table_words = ((1usize << (window_len - 1)) - 1)
            .checked_mul(n)
            .unwrap_or_else(|| memory::panic_out_of_memory());

        let memory_requirement = memory::add_layout(
            memory::array_layout::<Word>(table_words),
            self.ring().mul_memory_requirement(),
        );
        let mut allocation = MemoryAllocation::new(memory_requirement);
        let mut memory = allocation.memory();
        let (table, mut memory) = memory.allocate_slice_fill::<Word>(table_words, 0);

        // val = self^2
        let mut val = self.clone();
        val.mul_in_place(self, &mut memory);

        // self^(2*i+1) = self^(2*i-1) * val
        for i in 1..(1 << (window_len - 1)) {
            let (prev, cur) = if i == 1 {
                (self.normalized_value(), &mut table[0..n])
            } else {
                let (prev, cur) = table[(i - 2) * n..i * n].split_at_mut(n);
                (&*prev, cur)
            };
            cur.copy_from_slice(self.ring().mul_normalized(
                prev,
                val.normalized_value(),
                &mut memory,
            ));
        }

        let exp_words = exp.as_words();
        // We already have self^2 in val.
        // exp.bit_len() >= 2 because exp >= 2.
        let mut bit = exp.bit_len() - 2;

        loop {
            // val = self ^ exp[bit..] ignoring the lowest bit
            let word_idx = bit / WORD_BITS_USIZE;
            let bit_idx = (bit % WORD_BITS_USIZE) as u32;
            let cur_word = exp_words[word_idx];
            if cur_word & (1 << bit_idx) != 0 {
                let next_word = if word_idx == 0 {
                    0
                } else {
                    exp_words[word_idx - 1]
                };
                // Get a window of window_len bits, with top bit of 1.
                let (mut window, _) = split_double_word(
                    double_word(next_word, cur_word) >> (bit_idx + 1 + WORD_BITS - window_len),
                );
                window &= math::ones::<Word>(window_len);
                // Shift right to make the window odd.
                let num_bits = window_len - window.trailing_zeros();
                window >>= window_len - num_bits;
                // val := val^2^(num_bits-1)
                for _ in 0..num_bits - 1 {
                    val.square_in_place(&mut memory);
                }
                bit -= (num_bits as usize) - 1;
                // Now val = self ^ exp[bit..] ignoring the num_bits lowest bits.
                // val = val * self^window from precomputed table.
                debug_assert!(window & 1 == 1);
                let entry_idx = (window >> 1) as usize;
                let entry = if entry_idx == 0 {
                    self.normalized_value()
                } else {
                    &table[(entry_idx - 1) * n..entry_idx * n]
                };
                val.mul_normalized_in_place(entry, &mut memory);
            }
            // val = self ^ exp[bit..]
            if bit == 0 {
                break;
            }
            bit -= 1;
            val.square_in_place(&mut memory);
        }
        val
    }

    /// Choose the optimal window size for n-bit exponents.
    /// 1 <= window_size < min(WORD_BITS, usize::BIT_SIZE) inclusive.
    fn choose_pow_window_len(n: usize) -> u32 {
        // This won't overflow because cost(3) is already approximately usize::MAX / 4
        // and it can only grow by a factor of 2.
        let cost = |window_size| (1usize << (window_size - 1)) - 1 + n / (window_size as usize + 1);
        let mut window_size = 1;
        let mut c = cost(window_size);
        while window_size + 1 < WORD_BITS.min(usize::BIT_SIZE) {
            let c2 = cost(window_size + 1);
            if c <= c2 {
                break;
            }
            window_size += 1;
            c = c2;
        }
        window_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pow_word() {
        let ring = ModuloRingSmall::new(100);
        let a = ModuloSmallRaw::from_word(17, &ring);
        assert_eq!(a.pow_word(0, &ring).residue(&ring), 1);
        assert_eq!(a.pow_word(15, &ring).residue(&ring), 93);
    }
}
