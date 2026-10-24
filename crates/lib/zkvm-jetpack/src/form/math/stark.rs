use std::cmp::max;

use crate::form::belt::{Belt, FieldError};
use crate::form::proof::Constraints;

const LOG_EXPAND_FACTOR: usize = 6;
const SECURITY_LEVEL: usize = 50;
const GENERATOR: u64 = 7;
const FRI_FOLDING_DEG: usize = 8;

#[derive(Debug, Clone)]
pub struct FriParams {
    pub generator: Belt,
    pub omega: Belt,
    pub init_domain_len: usize,
    pub expand_factor: usize,
    pub num_spot_checks: usize,
    pub folding_deg: usize,
}

impl FriParams {
    pub fn num_rounds(&self) -> usize {
        let mut len = self.init_domain_len;
        let mut rounds = 0usize;
        while len > self.expand_factor
            && (self.num_spot_checks * 4) < len
            && len.is_multiple_of(self.folding_deg)
        {
            len /= self.folding_deg;
            rounds += 1;
        }
        max(1, rounds.saturating_sub(1))
    }

    pub fn last_codeword_len(&self) -> usize {
        let rounds = self.num_rounds();
        self.init_domain_len / self.folding_deg.pow(rounds as u32)
    }
}

#[derive(Debug, Clone)]
pub struct StarkCalc {
    pub fri: FriParams,
}

impl StarkCalc {
    pub fn new(heights: &[u64], _constraints: &Constraints) -> Result<Self, FieldError> {
        let expand_factor = 1usize << LOG_EXPAND_FACTOR;
        let num_spot_checks = SECURITY_LEVEL / LOG_EXPAND_FACTOR;

        let max_height = heights.iter().copied().max().unwrap_or(0);
        let max_padded_height = padded_height(max_height).ok_or(FieldError::OrderedRootError)?;
        let init_domain_len_u64 = max_padded_height
            .checked_mul(expand_factor as u64)
            .ok_or(FieldError::OrderedRootError)?;
        let init_domain_len =
            usize::try_from(init_domain_len_u64).map_err(|_| FieldError::OrderedRootError)?;

        let omega = Belt(init_domain_len_u64).ordered_root()?;
        let fri = FriParams {
            generator: Belt(GENERATOR),
            omega,
            init_domain_len,
            expand_factor,
            num_spot_checks,
            folding_deg: FRI_FOLDING_DEG,
        };

        Ok(Self { fri })
    }
}

fn padded_height(height: u64) -> Option<u64> {
    if height <= 1 {
        Some(height)
    } else {
        1u64.checked_shl((height - 1).ilog2() + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form::proof::{Constraints, ProofMap};

    #[test]
    fn stark_calc_defaults() {
        let heights = vec![8, 16];
        let constraints = Constraints(ProofMap::new());

        let calc = StarkCalc::new(&heights, &constraints).expect("calc failed");
        assert_eq!(calc.fri.expand_factor, 64);
        assert_eq!(calc.fri.num_spot_checks, 8);
        assert_eq!(calc.fri.init_domain_len, 16 * 64);
    }
}
