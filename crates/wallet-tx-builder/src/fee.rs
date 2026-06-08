use nockchain_types::tx_engine::common::BlockHeight;

const DEFAULT_INPUT_FEE_DIVISOR: u64 = 1;
pub const NICKS_PER_NOCK: u64 = 65_536;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeeInputs {
    pub seed_words: u64,
    pub witness_words: u64,
    pub base_fee: u64,
    pub input_fee_divisor: u64,
    pub min_fee: u64,
    pub height: BlockHeight,
    pub bythos_phase: BlockHeight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeeBreakdown {
    pub seed_fee: u64,
    pub witness_fee: u64,
    pub word_fee: u64,
    pub minimum_fee: u64,
    pub witness_divisor: u64,
}

/// Computes the bridge fee using the same whole-NOCK rounding rule as the
/// existing bridge deposit path.
pub fn compute_bridge_fee(amount: u64, nicks_fee_per_nock: u64) -> u64 {
    if amount == 0 || nicks_fee_per_nock == 0 {
        return 0;
    }

    let rounded_nocks = 1u64.saturating_add(amount.saturating_sub(1) / NICKS_PER_NOCK);
    rounded_nocks.saturating_mul(nicks_fee_per_nock)
}

pub fn compute_minimum_fee(inputs: FeeInputs) -> FeeBreakdown {
    let witness_divisor =
        witness_divisor(inputs.height, inputs.bythos_phase, inputs.input_fee_divisor);
    let seed_fee = inputs.seed_words.saturating_mul(inputs.base_fee);
    let witness_fee = if witness_divisor == 0 {
        0
    } else {
        inputs
            .witness_words
            .saturating_mul(inputs.base_fee)
            .saturating_div(witness_divisor)
    };
    let word_fee = seed_fee.saturating_add(witness_fee);
    let minimum_fee = word_fee.max(inputs.min_fee);

    FeeBreakdown {
        seed_fee,
        witness_fee,
        word_fee,
        minimum_fee,
        witness_divisor,
    }
}

pub fn witness_divisor(
    height: BlockHeight,
    bythos_phase: BlockHeight,
    input_fee_divisor: u64,
) -> u64 {
    if (height.0).0 >= (bythos_phase.0).0 {
        input_fee_divisor.max(DEFAULT_INPUT_FEE_DIVISOR)
    } else {
        DEFAULT_INPUT_FEE_DIVISOR
    }
}

#[cfg(test)]
mod tests {
    use nockchain_math::belt::Belt;

    use super::*;

    fn fee_inputs() -> FeeInputs {
        FeeInputs {
            seed_words: 0,
            witness_words: 0,
            base_fee: 0,
            input_fee_divisor: DEFAULT_INPUT_FEE_DIVISOR,
            min_fee: 0,
            height: BlockHeight(Belt(0)),
            bythos_phase: BlockHeight(Belt(0)),
        }
    }

    #[test]
    fn pre_bythos_uses_divisor_one() {
        let bythos_divisor = 8;
        let divisor = witness_divisor(BlockHeight(Belt(10)), BlockHeight(Belt(20)), bythos_divisor);
        assert_eq!(divisor, DEFAULT_INPUT_FEE_DIVISOR);
    }

    #[test]
    fn post_bythos_uses_input_divisor() {
        let bythos_divisor = 8;
        let divisor = witness_divisor(BlockHeight(Belt(20)), BlockHeight(Belt(20)), bythos_divisor);
        assert_eq!(divisor, bythos_divisor);
    }

    #[test]
    fn compute_minimum_fee_uses_pre_bythos_divisor_one() {
        let mut inputs = fee_inputs();
        inputs.seed_words = 3;
        inputs.witness_words = 8;
        inputs.base_fee = 5;
        inputs.input_fee_divisor = 4;
        inputs.height = BlockHeight(Belt(9));
        inputs.bythos_phase = BlockHeight(Belt(10));

        let fee = compute_minimum_fee(inputs);
        assert_eq!(fee.seed_fee, 15);
        assert_eq!(fee.witness_divisor, 1);
        assert_eq!(fee.witness_fee, 40);
        assert_eq!(fee.word_fee, 55);
        assert_eq!(fee.minimum_fee, 55);
    }

    #[test]
    fn compute_minimum_fee_uses_post_bythos_divisor() {
        let mut inputs = fee_inputs();
        inputs.seed_words = 3;
        inputs.witness_words = 8;
        inputs.base_fee = 5;
        inputs.input_fee_divisor = 4;
        inputs.height = BlockHeight(Belt(10));
        inputs.bythos_phase = BlockHeight(Belt(10));

        let fee = compute_minimum_fee(inputs);
        assert_eq!(fee.seed_fee, 15);
        assert_eq!(fee.witness_divisor, 4);
        assert_eq!(fee.witness_fee, 10);
        assert_eq!(fee.word_fee, 25);
        assert_eq!(fee.minimum_fee, 25);
    }

    #[test]
    fn compute_minimum_fee_applies_min_fee_floor() {
        let mut inputs = fee_inputs();
        inputs.seed_words = 1;
        inputs.witness_words = 1;
        inputs.base_fee = 2;
        inputs.min_fee = 99;

        let fee = compute_minimum_fee(inputs);
        assert_eq!(fee.word_fee, 4);
        assert_eq!(fee.minimum_fee, 99);
    }

    #[test]
    fn compute_minimum_fee_uses_saturating_math() {
        let mut inputs = fee_inputs();
        inputs.seed_words = u64::MAX;
        inputs.witness_words = u64::MAX;
        inputs.base_fee = 2;
        inputs.input_fee_divisor = 1;

        let fee = compute_minimum_fee(inputs);
        assert_eq!(fee.seed_fee, u64::MAX);
        assert_eq!(fee.witness_fee, u64::MAX);
        assert_eq!(fee.word_fee, u64::MAX);
        assert_eq!(fee.minimum_fee, u64::MAX);
    }

    #[test]
    fn compute_bridge_fee_returns_zero_for_zero_amount() {
        assert_eq!(compute_bridge_fee(0, 195), 0);
    }

    #[test]
    fn compute_bridge_fee_charges_fixed_fee_per_whole_nock() {
        assert_eq!(compute_bridge_fee(NICKS_PER_NOCK, 195), 195);
        assert_eq!(compute_bridge_fee(NICKS_PER_NOCK * 2, 195), 390);
    }

    #[test]
    fn compute_bridge_fee_rounds_partial_nock_up() {
        assert_eq!(compute_bridge_fee(1, 195), 195);
        assert_eq!(compute_bridge_fee(NICKS_PER_NOCK + 1, 195), 390);
    }
}
