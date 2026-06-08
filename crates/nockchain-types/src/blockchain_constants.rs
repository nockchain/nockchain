use std::time::Duration;

use ibig::UBig;
use nockapp::noun::slab::NounSlab;
use nockapp::noun::IntoSlab;
use nockvm::noun::{Atom, Noun, NounAllocator, NounSpace, T};
use noun_serde::{NounDecode, NounDecodeError, NounEncode};
use tracing::info;

pub const DEFAULT_FAKENET_POW_LEN: u64 = 2;
pub const DEFAULT_FAKENET_LOG_DIFFICULTY: u64 = 1;
pub const FAKENET_V1_PHASE: u64 = 1;
pub const FAKENET_BYTHOS_PHASE: u64 = 1;
pub const FAKENET_BASE_FEE: u64 = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, NounEncode, NounDecode)]
pub struct Seconds(pub u64);

impl Seconds {
    pub fn new(seconds: u64) -> Self {
        Self(seconds)
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }

    pub fn to_duration(&self) -> Duration {
        Duration::from_secs(self.0)
    }
}

impl From<u64> for Seconds {
    fn from(seconds: u64) -> Self {
        Self(seconds)
    }
}

impl TryFrom<Duration> for Seconds {
    type Error = &'static str;

    fn try_from(duration: Duration) -> Result<Self, Self::Error> {
        if duration.subsec_nanos() != 0 {
            return Err("Duration must be whole seconds only");
        }
        Ok(Self(duration.as_secs()))
    }
}

impl From<Seconds> for Duration {
    fn from(seconds: Seconds) -> Self {
        Duration::from_secs(seconds.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, NounEncode, NounDecode)]
pub struct NoteDataConstraints {
    pub max_size: u64,
    pub min_fee: u64,
}

/// Shared blockchain constants model used for explicit kernel constants pokes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockchainConstants {
    pub max_block_size: u64,
    pub blocks_per_epoch: u64,
    pub target_epoch_duration: Seconds,
    pub update_candidate_timestamp_interval: Seconds,
    pub max_future_timestamp: Seconds,
    pub min_past_blocks: u64,
    pub genesis_target_atom: UBig,
    pub max_target_atom: UBig,
    pub check_pow_flag: bool,
    pub coinbase_timelock_min: u64,
    pub pow_len: u64,
    pub max_coinbase_split: u64,
    pub first_month_coinbase_min: u64,
    pub v1_phase: u64,
    pub bythos_phase: u64,
    pub note_data: NoteDataConstraints,
    pub base_fee: u64,
    pub input_fee_divisor: u64,
    pub asert_phase: u64,
    pub asert_anchor_height: u64,
    pub asert_anchor_target_atom: UBig,
    pub asert_ideal_block_time: u64,
    pub asert_half_life: u64,
    pub asert_anchor_min_timestamp: u64,
}

impl BlockchainConstants {
    pub const DEFAULT_MAX_BLOCK_SIZE: u64 = 8_000_000;
    pub const DEFAULT_BLOCKS_PER_EPOCH: u64 = 2_016;
    pub const DEFAULT_TARGET_EPOCH_DURATION: u64 = 1_209_600;
    pub const DEFAULT_UPDATE_CANDIDATE_TIMESTAMP_INTERVAL_SECS: u64 = 300;
    pub const DEFAULT_MAX_FUTURE_TIMESTAMP: u64 = 7_200;
    pub const DEFAULT_GENESIS_TARGET_ATOM: &str =
        "0x3ffffffec0000003bffffff88000000b3ffffff34000000b3ffffff880000003bffffffec0000";
    pub const DEFAULT_MAX_TIP5_ATOM: &str =
        "0xfffffffb0000000effffffe20000002cffffffcd0000002cffffffe20000000efffffffb00000000";
    pub const DEFAULT_MIN_PAST_BLOCKS: u64 = 11;
    pub const DEFAULT_CHECK_POW_FLAG: bool = true;
    pub const DEFAULT_COINBASE_TIMELOCK_MIN: u64 = 100;
    pub const DEFAULT_POW_LEN: u64 = 64;
    pub const DEFAULT_MAX_COINBASE_SPLIT: u64 = 2;
    pub const DEFAULT_FIRST_MONTH_COINBASE_MIN: u64 = 4_383;
    pub const DEFAULT_V1_PHASE: u64 = 39_000;
    pub const DEFAULT_BYTHOS_PHASE: u64 = 54_000;
    pub const DEFAULT_NOTE_DATA_MAX_SIZE: u64 = 2_048;
    pub const DEFAULT_NOTE_DATA_MIN_FEE: u64 = 256;
    pub const DEFAULT_BASE_FEE: u64 = 16_384;
    pub const DEFAULT_INPUT_FEE_DIVISOR: u64 = 4;
    pub const DEFAULT_ASERT_PHASE: u64 = 65_500;
    pub const DEFAULT_ASERT_ANCHOR_HEIGHT: u64 = 65_499;
    pub const DEFAULT_ASERT_ANCHOR_TARGET_BEX: u64 = 291;
    pub const DEFAULT_ASERT_IDEAL_BLOCK_TIME: u64 = 150;
    pub const DEFAULT_ASERT_HALF_LIFE: u64 = 43_200;
    /// Median-of-11 timestamp at the canonical ASERT anchor block (mainnet
    /// height 65,499), pinned at phase-2 cutover of 014-aletheia. See
    /// `open/hoon/common/tx-engine-1.hoon`'s `blockchain-constants:v1`
    /// bunt — must stay in sync.
    pub const DEFAULT_ASERT_ANCHOR_MIN_TIMESTAMP: u64 = 9_223_372_093_639_027_842;

    pub fn new() -> Self {
        let max_target_atom = UBig::from_str_with_radix_prefix(Self::DEFAULT_MAX_TIP5_ATOM)
            .expect("Failed to parse max tip5 atom");
        let genesis_target_atom =
            UBig::from_str_with_radix_prefix(Self::DEFAULT_GENESIS_TARGET_ATOM)
                .expect("Failed to parse genesis target atom");
        let asert_anchor_target_atom =
            UBig::from(1u64) << (Self::DEFAULT_ASERT_ANCHOR_TARGET_BEX as usize);

        Self {
            max_block_size: Self::DEFAULT_MAX_BLOCK_SIZE,
            blocks_per_epoch: Self::DEFAULT_BLOCKS_PER_EPOCH,
            target_epoch_duration: Self::DEFAULT_TARGET_EPOCH_DURATION.into(),
            update_candidate_timestamp_interval:
                Self::DEFAULT_UPDATE_CANDIDATE_TIMESTAMP_INTERVAL_SECS.into(),
            max_future_timestamp: Self::DEFAULT_MAX_FUTURE_TIMESTAMP.into(),
            min_past_blocks: Self::DEFAULT_MIN_PAST_BLOCKS,
            genesis_target_atom,
            max_target_atom,
            check_pow_flag: Self::DEFAULT_CHECK_POW_FLAG,
            coinbase_timelock_min: Self::DEFAULT_COINBASE_TIMELOCK_MIN,
            pow_len: Self::DEFAULT_POW_LEN,
            max_coinbase_split: Self::DEFAULT_MAX_COINBASE_SPLIT,
            first_month_coinbase_min: Self::DEFAULT_FIRST_MONTH_COINBASE_MIN,
            v1_phase: Self::DEFAULT_V1_PHASE,
            bythos_phase: Self::DEFAULT_BYTHOS_PHASE,
            note_data: NoteDataConstraints {
                max_size: Self::DEFAULT_NOTE_DATA_MAX_SIZE,
                min_fee: Self::DEFAULT_NOTE_DATA_MIN_FEE,
            },
            base_fee: Self::DEFAULT_BASE_FEE,
            input_fee_divisor: Self::DEFAULT_INPUT_FEE_DIVISOR,
            asert_phase: Self::DEFAULT_ASERT_PHASE,
            asert_anchor_height: Self::DEFAULT_ASERT_ANCHOR_HEIGHT,
            asert_anchor_target_atom,
            asert_ideal_block_time: Self::DEFAULT_ASERT_IDEAL_BLOCK_TIME,
            asert_half_life: Self::DEFAULT_ASERT_HALF_LIFE,
            asert_anchor_min_timestamp: Self::DEFAULT_ASERT_ANCHOR_MIN_TIMESTAMP,
        }
    }

    pub fn with_genesis_target_atom_bex(mut self, bex: u128) -> Self {
        let difficulty = UBig::from((1 << bex) as u128);
        self.genesis_target_atom = self.max_target_atom.clone() / difficulty;
        info!("Genesis target atom set to {}", self.genesis_target_atom);
        self
    }

    pub fn with_update_candidate_timestamp_interval(mut self, interval_secs: Seconds) -> Self {
        self.update_candidate_timestamp_interval = interval_secs;
        self
    }

    pub fn with_pow_len(mut self, pow_len: u64) -> Self {
        self.pow_len = pow_len;
        self
    }

    pub fn with_v1_phase(mut self, v1_phase: u64) -> Self {
        self.v1_phase = v1_phase;
        self
    }

    pub fn with_bythos_phase(mut self, bythos_phase: u64) -> Self {
        self.bythos_phase = bythos_phase;
        self
    }

    pub fn with_first_month_coinbase_min(mut self, coinbase_min: u64) -> Self {
        self.first_month_coinbase_min = coinbase_min;
        self
    }

    pub fn with_coinbase_timelock_min(mut self, coinbase_min: u64) -> Self {
        self.coinbase_timelock_min = coinbase_min;
        self
    }

    pub fn with_base_fee(mut self, base_fee: u64) -> Self {
        self.base_fee = base_fee;
        self
    }

    pub fn with_asert_phase(mut self, asert_phase: u64) -> Self {
        self.asert_phase = asert_phase;
        self
    }

    pub fn with_asert_anchor_height(mut self, asert_anchor_height: u64) -> Self {
        self.asert_anchor_height = asert_anchor_height;
        self
    }

    pub fn with_asert_anchor_target_atom(mut self, asert_anchor_target_atom: UBig) -> Self {
        self.asert_anchor_target_atom = asert_anchor_target_atom;
        self
    }

    pub fn with_asert_anchor_target_bex(mut self, bex: u64) -> Self {
        self.asert_anchor_target_atom = UBig::from(1u64) << (bex as usize);
        self
    }

    fn to_blockchain_constants_v0_fields<A: NounAllocator>(&self, allocator: &mut A) -> Vec<Noun> {
        let max_block_size = Atom::new(allocator, self.max_block_size).as_noun();
        let blocks_per_epoch = Atom::new(allocator, self.blocks_per_epoch).as_noun();
        let target_epoch_duration = self.target_epoch_duration.to_noun(allocator);
        let update_candidate_timestamp_interval_atoms =
            UBig::from(self.update_candidate_timestamp_interval.0) << 64;
        let update_candidate_timestamp_interval =
            Atom::from_ubig(allocator, &update_candidate_timestamp_interval_atoms).as_noun();
        let max_future_timestamp = self.max_future_timestamp.to_noun(allocator);
        let min_past_blocks = Atom::new(allocator, self.min_past_blocks).as_noun();
        let genesis_target_atom = Atom::from_ubig(allocator, &self.genesis_target_atom).as_noun();
        let max_target_atom = Atom::from_ubig(allocator, &self.max_target_atom).as_noun();
        let check_pow_flag = self.check_pow_flag.to_noun(allocator);
        let coinbase_timelock_min = Atom::new(allocator, self.coinbase_timelock_min).as_noun();
        let pow_len = Atom::new(allocator, self.pow_len).as_noun();
        let max_coinbase_split = Atom::new(allocator, self.max_coinbase_split).as_noun();
        let first_month_coinbase_min =
            Atom::new(allocator, self.first_month_coinbase_min).as_noun();

        vec![
            max_block_size, blocks_per_epoch, target_epoch_duration,
            update_candidate_timestamp_interval, max_future_timestamp, min_past_blocks,
            genesis_target_atom, max_target_atom, check_pow_flag, coinbase_timelock_min, pow_len,
            max_coinbase_split, first_month_coinbase_min,
        ]
    }
}

impl Default for BlockchainConstants {
    fn default() -> Self {
        Self::new()
    }
}

impl NounEncode for BlockchainConstants {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let v1_phase = Atom::new(allocator, self.v1_phase).as_noun();
        let bythos_phase = Atom::new(allocator, self.bythos_phase).as_noun();
        let note_data = self.note_data.to_noun(allocator);
        let base_fee = Atom::new(allocator, self.base_fee).as_noun();
        let input_fee_divisor = Atom::new(allocator, self.input_fee_divisor).as_noun();
        let v0_fields = self.to_blockchain_constants_v0_fields(allocator);
        let v0_constants = T(allocator, &v0_fields);
        let asert_phase = Atom::new(allocator, self.asert_phase).as_noun();
        let asert_anchor_height = Atom::new(allocator, self.asert_anchor_height).as_noun();
        let asert_anchor_target_atom =
            Atom::from_ubig(allocator, &self.asert_anchor_target_atom).as_noun();
        let asert_ideal_block_time = Atom::new(allocator, self.asert_ideal_block_time).as_noun();
        let asert_half_life = Atom::new(allocator, self.asert_half_life).as_noun();
        let asert_anchor_min_timestamp =
            Atom::new(allocator, self.asert_anchor_min_timestamp).as_noun();

        T(
            allocator,
            &[
                v1_phase, bythos_phase, note_data, base_fee, input_fee_divisor, v0_constants,
                asert_phase, asert_anchor_height, asert_anchor_target_atom, asert_ideal_block_time,
                asert_half_life, asert_anchor_min_timestamp,
            ],
        )
    }
}

// TODO(withdrawals): Replace this manual decode with derived noun-serde once
// the blockchain-constants wire shape is represented by derive-friendly Rust
// wrapper types instead of the current mixed v1 wrapper + nested v0 payload.
impl NounDecode for BlockchainConstants {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let mut outer = noun
            .in_space(space)
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        let v1_phase = u64::from_noun(&outer.head().noun(), space)?;
        outer = outer
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        let bythos_phase = u64::from_noun(&outer.head().noun(), space)?;
        outer = outer
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        let note_data = NoteDataConstraints::from_noun(&outer.head().noun(), space)?;
        outer = outer
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        let base_fee = u64::from_noun(&outer.head().noun(), space)?;
        outer = outer
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        let input_fee_divisor = u64::from_noun(&outer.head().noun(), space)?;
        outer = outer
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        let mut v0 = outer
            .head()
            .noun()
            .in_space(space)
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;
        let mut asert = outer
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        let max_block_size = u64::from_noun(&v0.head().noun(), space)?;
        v0 = v0
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        let blocks_per_epoch = u64::from_noun(&v0.head().noun(), space)?;
        v0 = v0
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        let target_epoch_duration = Seconds::from_noun(&v0.head().noun(), space)?;
        v0 = v0
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        let update_candidate_timestamp_interval = decode_shifted_seconds(
            &v0.head().noun(),
            space,
            "update-candidate-timestamp-interval",
        )?;
        v0 = v0
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        let max_future_timestamp = Seconds::from_noun(&v0.head().noun(), space)?;
        v0 = v0
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        let min_past_blocks = u64::from_noun(&v0.head().noun(), space)?;
        v0 = v0
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        let genesis_target_atom = decode_ubig(&v0.head().noun(), space, "genesis-target-atom")?;
        v0 = v0
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        let max_target_atom = decode_ubig(&v0.head().noun(), space, "max-target-atom")?;
        v0 = v0
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        let check_pow_flag = bool::from_noun(&v0.head().noun(), space)?;
        v0 = v0
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        let coinbase_timelock_min = u64::from_noun(&v0.head().noun(), space)?;
        v0 = v0
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        let pow_len = u64::from_noun(&v0.head().noun(), space)?;
        v0 = v0
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        let max_coinbase_split = u64::from_noun(&v0.head().noun(), space)?;
        let first_month_coinbase_min = u64::from_noun(&v0.tail().noun(), space)?;

        let asert_phase = u64::from_noun(&asert.head().noun(), space)?;
        asert = asert
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        let asert_anchor_height = u64::from_noun(&asert.head().noun(), space)?;
        asert = asert
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        let asert_anchor_target_atom =
            decode_ubig(&asert.head().noun(), space, "asert-anchor-target-atom")?;
        asert = asert
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        let asert_ideal_block_time = u64::from_noun(&asert.head().noun(), space)?;
        asert = asert
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        let asert_half_life = u64::from_noun(&asert.head().noun(), space)?;
        let asert_anchor_min_timestamp = u64::from_noun(&asert.tail().noun(), space)?;

        Ok(Self {
            max_block_size,
            blocks_per_epoch,
            target_epoch_duration,
            update_candidate_timestamp_interval,
            max_future_timestamp,
            min_past_blocks,
            genesis_target_atom,
            max_target_atom,
            check_pow_flag,
            coinbase_timelock_min,
            pow_len,
            max_coinbase_split,
            first_month_coinbase_min,
            v1_phase,
            bythos_phase,
            note_data,
            base_fee,
            input_fee_divisor,
            asert_phase,
            asert_anchor_height,
            asert_anchor_target_atom,
            asert_ideal_block_time,
            asert_half_life,
            asert_anchor_min_timestamp,
        })
    }
}

fn decode_ubig(
    noun: &Noun,
    space: &NounSpace,
    field: &'static str,
) -> Result<UBig, NounDecodeError> {
    let atom = noun
        .in_space(space)
        .as_atom()
        .map_err(|_| NounDecodeError::Custom(format!("{field} should be atom")))?;
    Ok(UBig::from_le_bytes(&atom.to_le_bytes()))
}

fn decode_shifted_seconds(
    noun: &Noun,
    space: &NounSpace,
    field: &'static str,
) -> Result<Seconds, NounDecodeError> {
    let encoded = decode_ubig(noun, space, field)?;
    let lower_mask = UBig::from((1u128 << 64) - 1);
    if (&encoded & &lower_mask) != UBig::from(0u8) {
        return Err(NounDecodeError::Custom(format!(
            "{field} lower 64 bits must be zero"
        )));
    }
    let shifted = encoded >> 64usize;
    let seconds = u64::try_from(shifted)
        .map_err(|_| NounDecodeError::Custom(format!("{field} too large for u64 seconds")))?;
    Ok(Seconds(seconds))
}

impl IntoSlab for BlockchainConstants {
    fn into_slab(self) -> NounSlab {
        let mut slab = NounSlab::new();
        let noun = self.to_noun(&mut slab);
        slab.set_root(noun);
        slab
    }
}

pub fn fakenet_blockchain_constants(pow_len: u64, target_bex: u64) -> BlockchainConstants {
    // Fakenet starts from the mainnet defaults and overrides only the fields
    // that are intentionally relaxed for local testing.
    BlockchainConstants::new()
        .with_update_candidate_timestamp_interval(Seconds(5 * 60))
        .with_pow_len(pow_len)
        .with_genesis_target_atom_bex(target_bex as u128)
        .with_first_month_coinbase_min(0)
        .with_coinbase_timelock_min(1)
        .with_base_fee(FAKENET_BASE_FEE)
        .with_v1_phase(FAKENET_V1_PHASE)
        .with_bythos_phase(FAKENET_BYTHOS_PHASE)
}

pub fn default_fakenet_blockchain_constants() -> BlockchainConstants {
    fakenet_blockchain_constants(DEFAULT_FAKENET_POW_LEN, DEFAULT_FAKENET_LOG_DIFFICULTY)
}

#[cfg(test)]
mod tests {
    use ibig::UBig;
    use nockapp::noun::slab::NockJammer;

    use super::*;

    fn tuple_len(noun: Noun, space: &nockvm::noun::NounSpace) -> usize {
        let mut len = 0;
        let mut cur = noun;
        loop {
            if let Ok(cell) = cur.in_space(space).as_cell() {
                len += 1;
                cur = cell.tail().noun();
            } else {
                len += 1;
                break;
            }
        }
        len
    }

    #[test]
    fn blockchain_constants_new_defaults_are_valid() {
        let constants = BlockchainConstants::new();

        assert_eq!(
            constants.max_block_size, 8_000_000,
            "max-block-size mismatch"
        );
        assert_eq!(
            constants.blocks_per_epoch, 2_016,
            "blocks-per-epoch mismatch"
        );
        assert_eq!(
            constants.target_epoch_duration,
            Seconds::new(14 * 24 * 60 * 60),
            "target-epoch-duration mismatch",
        );
        assert_eq!(
            constants.update_candidate_timestamp_interval,
            Seconds::new(5 * 60),
            "update-candidate-interval mismatch",
        );
        assert_eq!(
            constants.max_future_timestamp,
            Seconds::new(60 * 120),
            "max-future-timestamp mismatch",
        );
        assert_eq!(constants.min_past_blocks, 11, "min-past-blocks mismatch");

        let max_tip5_atom =
            UBig::from_str_with_radix_prefix(BlockchainConstants::DEFAULT_MAX_TIP5_ATOM)
                .expect("parse max tip5 atom");
        assert_eq!(
            constants.max_target_atom, max_tip5_atom,
            "max-target-atom mismatch",
        );

        let expected_genesis_target = &max_tip5_atom / (UBig::from(1u64) << 14);
        assert_eq!(
            constants.genesis_target_atom, expected_genesis_target,
            "genesis-target-atom mismatch",
        );

        assert!(constants.check_pow_flag, "check-pow-flag mismatch");
        assert_eq!(
            constants.coinbase_timelock_min, 100,
            "coinbase-timelock-min mismatch"
        );
        assert_eq!(constants.pow_len, 64, "pow-len mismatch");
        assert_eq!(
            constants.max_coinbase_split, 2,
            "max-coinbase-split mismatch"
        );
        assert_eq!(
            constants.first_month_coinbase_min, 4_383,
            "first-month-coinbase-min mismatch",
        );
        assert_eq!(constants.v1_phase, 39_000, "v1-phase mismatch");
        assert_eq!(constants.bythos_phase, 54_000, "bythos-phase mismatch");
        assert_eq!(
            constants.note_data,
            NoteDataConstraints {
                max_size: 2_048,
                min_fee: 256,
            },
            "note-data mismatch",
        );
        assert_eq!(constants.base_fee, 16_384, "base-fee mismatch");
        assert_eq!(constants.input_fee_divisor, 4, "input-fee-divisor mismatch");
        assert_eq!(constants.asert_phase, 65_500, "asert-phase mismatch");
        assert_eq!(
            constants.asert_anchor_height, 65_499,
            "asert-anchor-height mismatch"
        );
        assert_eq!(
            constants.asert_anchor_target_atom,
            UBig::from(1u64) << 291,
            "asert-anchor-target-atom mismatch"
        );
        assert_eq!(
            constants.asert_ideal_block_time, 150,
            "asert-ideal-block-time mismatch"
        );
        assert_eq!(
            constants.asert_half_life, 43_200,
            "asert-half-life mismatch"
        );
    }

    #[test]
    fn blockchain_constants_roundtrip_from_noun_for_mainnet_and_fakenet() {
        for constants in [BlockchainConstants::default(), default_fakenet_blockchain_constants()] {
            let mut slab: NounSlab<NockJammer> = NounSlab::new();
            let noun = constants.to_noun(&mut slab);
            let space = slab.noun_space();
            let decoded =
                BlockchainConstants::from_noun(&noun, &space).expect("decode blockchain constants");
            assert_eq!(decoded, constants);
        }
    }

    #[test]
    fn fakenet_blockchain_constants_activate_early_phases() {
        let constants = default_fakenet_blockchain_constants();

        assert_eq!(
            constants.pow_len, DEFAULT_FAKENET_POW_LEN,
            "pow-len mismatch"
        );
        assert_eq!(
            constants.coinbase_timelock_min, 1,
            "coinbase-timelock-min mismatch"
        );
        assert_eq!(
            constants.first_month_coinbase_min, 0,
            "first-month-coinbase-min mismatch",
        );
        assert_eq!(constants.base_fee, FAKENET_BASE_FEE, "base-fee mismatch");
        assert_eq!(constants.v1_phase, FAKENET_V1_PHASE, "v1-phase mismatch");
        assert_eq!(
            constants.bythos_phase, FAKENET_BYTHOS_PHASE,
            "bythos-phase mismatch"
        );
    }

    #[test]
    fn with_v1_phase_overrides_default() {
        let constants = BlockchainConstants::new().with_v1_phase(54_321);

        assert_eq!(constants.v1_phase, 54_321);
    }

    #[test]
    fn with_asert_overrides_default() {
        let constants = BlockchainConstants::new()
            .with_asert_phase(10)
            .with_asert_anchor_height(9)
            .with_asert_anchor_target_bex(2);

        assert_eq!(constants.asert_phase, 10);
        assert_eq!(constants.asert_anchor_height, 9);
        assert_eq!(constants.asert_anchor_target_atom, UBig::from(1u64) << 2);
    }

    #[test]
    fn blockchain_constants_encode_in_new_v1_wrapper() {
        let slab = BlockchainConstants::new().into_slab();
        let root = unsafe { *slab.root() };
        let space = slab.noun_space();

        let outer = root.in_space(&space).as_cell().expect("outer tuple");
        let v1_phase_atom = outer.head().as_atom().expect("v1-phase should be atom");
        assert_eq!(
            v1_phase_atom.as_u64().expect("v1-phase as u64"),
            BlockchainConstants::DEFAULT_V1_PHASE
        );

        let rest = outer.tail().as_cell().expect("rest tuple");
        let bythos_phase_atom = rest.head().as_atom().expect("bythos-phase should be atom");
        assert_eq!(
            bythos_phase_atom.as_u64().expect("bythos-phase as u64"),
            BlockchainConstants::DEFAULT_BYTHOS_PHASE
        );

        let rest = rest.tail().as_cell().expect("note-data and rest tuple");
        let note_data = rest.head().as_cell().expect("note-data tuple");
        let note_data_max_size = note_data
            .head()
            .as_atom()
            .expect("note-data max-size atom")
            .as_u64()
            .expect("note-data max-size as u64");
        let note_data_min_fee = note_data
            .tail()
            .as_atom()
            .expect("note-data min-fee atom")
            .as_u64()
            .expect("note-data min-fee as u64");
        assert_eq!(
            note_data_max_size,
            BlockchainConstants::DEFAULT_NOTE_DATA_MAX_SIZE
        );
        assert_eq!(
            note_data_min_fee,
            BlockchainConstants::DEFAULT_NOTE_DATA_MIN_FEE
        );

        let base_fee_and_rest = rest.tail().as_cell().expect("base-fee and rest tuple");
        let base_fee_atom = base_fee_and_rest.head().as_atom().expect("base-fee atom");
        assert_eq!(
            base_fee_atom.as_u64().expect("base-fee as u64"),
            BlockchainConstants::DEFAULT_BASE_FEE
        );

        let input_fee_divisor_and_rest = base_fee_and_rest
            .tail()
            .as_cell()
            .expect("input-fee-divisor and rest tuple");
        let input_fee_divisor_atom = input_fee_divisor_and_rest
            .head()
            .as_atom()
            .expect("input-fee-divisor atom");
        assert_eq!(
            input_fee_divisor_atom
                .as_u64()
                .expect("input-fee-divisor as u64"),
            BlockchainConstants::DEFAULT_INPUT_FEE_DIVISOR
        );

        let v0_and_rest = input_fee_divisor_and_rest
            .tail()
            .as_cell()
            .expect("v0 constants and asert tail tuple");
        let v0_constants = v0_and_rest.head().noun();
        assert_eq!(
            tuple_len(v0_constants, &space),
            13,
            "v0 constants should be a 13-tuple"
        );
        let v0_cell = v0_constants
            .in_space(&space)
            .as_cell()
            .expect("v0 constants tuple");
        let max_block_size_atom = v0_cell.head().as_atom().expect("max-block-size atom");
        assert_eq!(
            max_block_size_atom.as_u64().expect("max-block-size as u64"),
            BlockchainConstants::DEFAULT_MAX_BLOCK_SIZE
        );

        let asert_phase_and_rest = v0_and_rest
            .tail()
            .as_cell()
            .expect("asert-phase and rest tuple");
        let asert_phase_atom = asert_phase_and_rest
            .head()
            .as_atom()
            .expect("asert-phase atom");
        assert_eq!(
            asert_phase_atom.as_u64().expect("asert-phase as u64"),
            BlockchainConstants::DEFAULT_ASERT_PHASE
        );

        let asert_anchor_height_and_rest = asert_phase_and_rest
            .tail()
            .as_cell()
            .expect("asert-anchor-height and rest tuple");
        let asert_anchor_height_atom = asert_anchor_height_and_rest
            .head()
            .as_atom()
            .expect("asert-anchor-height atom");
        assert_eq!(
            asert_anchor_height_atom
                .as_u64()
                .expect("asert-anchor-height as u64"),
            BlockchainConstants::DEFAULT_ASERT_ANCHOR_HEIGHT
        );

        let asert_anchor_target_and_rest = asert_anchor_height_and_rest
            .tail()
            .as_cell()
            .expect("asert-anchor-target and rest tuple");
        let _asert_anchor_target_atom = asert_anchor_target_and_rest
            .head()
            .as_atom()
            .expect("asert-anchor-target atom");

        let asert_ideal_and_rest = asert_anchor_target_and_rest
            .tail()
            .as_cell()
            .expect("asert-ideal-block-time and rest tuple");
        let asert_ideal_atom = asert_ideal_and_rest
            .head()
            .as_atom()
            .expect("asert-ideal-block-time atom");
        assert_eq!(
            asert_ideal_atom
                .as_u64()
                .expect("asert-ideal-block-time as u64"),
            BlockchainConstants::DEFAULT_ASERT_IDEAL_BLOCK_TIME
        );

        let asert_half_life_and_rest = asert_ideal_and_rest
            .tail()
            .as_cell()
            .expect("asert-half-life and rest tuple");
        let asert_half_life_atom = asert_half_life_and_rest
            .head()
            .as_atom()
            .expect("asert-half-life atom");
        assert_eq!(
            asert_half_life_atom
                .as_u64()
                .expect("asert-half-life as u64"),
            BlockchainConstants::DEFAULT_ASERT_HALF_LIFE
        );

        let asert_anchor_min_timestamp_atom = asert_half_life_and_rest
            .tail()
            .as_atom()
            .expect("asert-anchor-min-timestamp atom");
        assert_eq!(
            asert_anchor_min_timestamp_atom
                .as_u64()
                .expect("asert-anchor-min-timestamp as u64"),
            BlockchainConstants::DEFAULT_ASERT_ANCHOR_MIN_TIMESTAMP
        );
    }
}
