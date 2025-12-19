use std::error::Error;
use std::time::Duration;

use ibig::UBig;
use nockapp::noun::slab::Jammer;
use nockapp::noun::IntoSlab;
use nockapp::utils::make_tas;
use nockapp::wire::Wire;
use nockapp::{AtomExt, Bytes, NockApp, NockAppError, ToBytes};
use nockvm::noun::{Atom, Noun, NounAllocator, D, T};
use nockvm_macros::tas;
use noun_serde::NounEncode;
use tracing::info;

use crate::NounSlab;

#[cfg(feature = "bazel_build")]
pub static FAKENET_GENESIS_BLOCK: &[u8] = include_bytes!(env!("FAKENET_GENESIS_PATH"));

#[cfg(not(feature = "bazel_build"))]
pub static FAKENET_GENESIS_BLOCK: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/jams/fakenet-genesis-pow-2-bex-1.jam"
));

// TODO: Necessary for now, but we will delete this parameter from genesis seal
pub const DEFAULT_GENESIS_BLOCK_HEIGHT: u64 = 0;
pub const FAKENET_GENESIS_MESSAGE: &str = "3WNP3WtcQJYtP5PCvFHDQVEeiZEznsULEY5Lc4vUKV64Ge8feBxAkYp";
pub const REALNET_GENESIS_MESSAGE: &str = "2c8Ltbg44dPkEGcNPupcVAtDgD87753M9pG2fg8yC2mTEqg5qAFvvbT";

pub enum SetupCommand {
    PokeFakenetConstants(BlockchainConstants),
    PokeSetGenesisSeal(String),
    PokeSetBtcData,
}

pub fn fakenet_blockchain_constants(pow_len: u64, target_bex: u64) -> BlockchainConstants {
    BlockchainConstants::new()
        .with_update_candidate_timestamp_interval(Seconds(5 * 60))
        .with_pow_len(pow_len)
        .with_genesis_target_atom_bex(target_bex as u128)
        .with_first_month_coinbase_min(0)
        .with_coinbase_timelock_min(1)
}

pub async fn poke<J: Jammer + Send + 'static>(
    nockapp: &mut NockApp<J>,
    command: SetupCommand,
) -> Result<(), Box<dyn Error>> {
    let poke: NounSlab = match command {
        SetupCommand::PokeFakenetConstants(constants) => {
            let mut poke_slab = NounSlab::new();
            let tag = make_tas(&mut poke_slab, "set-constants").as_noun();
            let constants_noun = constants.to_noun(&mut poke_slab);
            let poke_noun = T(&mut poke_slab, &[D(tas!(b"command")), tag, constants_noun]);
            poke_slab.set_root(poke_noun);
            poke_slab
        }
        SetupCommand::PokeSetGenesisSeal(seal) => {
            let mut poke_slab = NounSlab::new();
            let block_height_noun =
                Atom::new(&mut poke_slab, DEFAULT_GENESIS_BLOCK_HEIGHT).as_noun();
            let seal_byts = Bytes::from(
                seal.to_bytes()
                    .expect("Failed to convert seal message to bytes"),
            );
            let seal_noun = Atom::from_bytes(&mut poke_slab, &seal_byts).as_noun();
            let tag = Bytes::from(b"set-genesis-seal".to_vec());
            let set_genesis_seal = Atom::from_bytes(&mut poke_slab, &tag).as_noun();
            let poke_noun = T(
                &mut poke_slab,
                &[D(tas!(b"command")), set_genesis_seal, block_height_noun, seal_noun],
            );
            poke_slab.set_root(poke_noun);
            poke_slab
        }
        SetupCommand::PokeSetBtcData => {
            let mut poke_slab = NounSlab::new();
            let poke_noun = T(
                &mut poke_slab,
                &[D(tas!(b"command")), D(tas!(b"btc-data")), D(0)],
            );
            poke_slab.set_root(poke_noun);
            poke_slab
        }
    };

    nockapp
        .poke(nockapp::wire::SystemWire.to_wire(), poke)
        .await?;
    Ok(())
}

pub fn heard_fake_genesis_block(
    fake_genesis_data: Option<Vec<u8>>,
) -> Result<NounSlab, NockAppError> {
    let mut poke_slab = NounSlab::new();
    let tag = make_tas(&mut poke_slab, "heard-block").as_noun();
    // load the block bytes
    let block_bytes = if let Some(data) = fake_genesis_data {
        Bytes::from(data)
    } else {
        Bytes::from(FAKENET_GENESIS_BLOCK)
    };
    let block = poke_slab.cue_into(block_bytes)?;
    let poke_noun = T(&mut poke_slab, &[D(tas!(b"fact")), D(0), tag, block]);
    poke_slab.set_root(poke_noun);
    Ok(poke_slab)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, NounEncode)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, NounEncode)]
pub struct NoteDataConstraints {
    pub max_size: u64,
    pub min_fee: u64,
}

pub struct BlockchainConstants {
    // In bits
    pub max_block_size: u64,
    pub blocks_per_epoch: u64,
    // In seconds
    pub target_epoch_duration: Seconds,
    // in seconds
    pub update_candidate_timestamp_interval: Seconds,
    // in seconds
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
    pub note_data: NoteDataConstraints,
    pub base_fee: u64,
}

impl BlockchainConstants {
    pub const DEFAULT_MAX_BLOCK_SIZE: u64 = 8000000;
    pub const DEFAULT_BLOCKS_PER_EPOCH: u64 = 2016;
    pub const DEFAULT_TARGET_EPOCH_DURATION: u64 = 1209600;
    pub const DEFAULT_UPDATE_CANDIDATE_TIMESTAMP_INTERVAL_SECS: u64 = 300;
    pub const DEFAULT_MAX_FUTURE_TIMESTAMP: u64 = 7200;
    // DEFAULT_GENESIS_TARGET_ATOM = MAX_TIP5_ATOM / (2 << 14)
    pub const DEFAULT_GENESIS_TARGET_ATOM: &str =
        "0x3ffffffec0000003bffffff88000000b3ffffff34000000b3ffffff880000003bffffffec0000";
    // Largest tip5 hash in base-p form
    pub const DEFAULT_MAX_TIP5_ATOM: &str =
        "0xfffffffb0000000effffffe20000002cffffffcd0000002cffffffe20000000efffffffb00000000";
    pub const DEFAULT_MIN_PAST_BLOCKS: u64 = 11;
    pub const DEFAULT_CHECK_POW_FLAG: bool = true;
    pub const DEFAULT_COINBASE_TIMELOCK_MIN: u64 = 100;
    pub const DEFAULT_POW_LEN: u64 = 64;
    pub const DEFAULT_MAX_COINBASE_SPLIT: u64 = 2;
    pub const DEFAULT_FIRST_MONTH_COINBASE_MIN: u64 = 4383;
    // TODO: update these for FINAL RELEASE
    pub const DEFAULT_V1_PHASE: u64 = 40_000;
    pub const DEFAULT_NOTE_DATA_MAX_SIZE: u64 = 2_048;
    pub const DEFAULT_NOTE_DATA_MIN_FEE: u64 = 256;
    pub const DEFAULT_BASE_FEE: u64 = 256;

    pub fn new() -> Self {
        let max_tip5_atom = UBig::from_str_with_radix_prefix(Self::DEFAULT_MAX_TIP5_ATOM)
            .expect("Failed to parse max tip5 atom");
        let genesis_target_atom =
            UBig::from_str_with_radix_prefix(Self::DEFAULT_GENESIS_TARGET_ATOM)
                .expect("Failed to parse genesis target atom");
        BlockchainConstants {
            max_block_size: Self::DEFAULT_MAX_BLOCK_SIZE,
            blocks_per_epoch: Self::DEFAULT_BLOCKS_PER_EPOCH,
            target_epoch_duration: Self::DEFAULT_TARGET_EPOCH_DURATION.into(),
            update_candidate_timestamp_interval:
                Self::DEFAULT_UPDATE_CANDIDATE_TIMESTAMP_INTERVAL_SECS.into(),
            max_future_timestamp: Self::DEFAULT_MAX_FUTURE_TIMESTAMP.into(),
            min_past_blocks: Self::DEFAULT_MIN_PAST_BLOCKS,
            genesis_target_atom: genesis_target_atom,
            max_target_atom: max_tip5_atom,
            check_pow_flag: Self::DEFAULT_CHECK_POW_FLAG,
            coinbase_timelock_min: Self::DEFAULT_COINBASE_TIMELOCK_MIN,
            pow_len: Self::DEFAULT_POW_LEN,
            max_coinbase_split: Self::DEFAULT_MAX_COINBASE_SPLIT,
            first_month_coinbase_min: Self::DEFAULT_FIRST_MONTH_COINBASE_MIN,
            v1_phase: Self::DEFAULT_V1_PHASE,
            note_data: NoteDataConstraints {
                max_size: Self::DEFAULT_NOTE_DATA_MAX_SIZE,
                min_fee: Self::DEFAULT_NOTE_DATA_MIN_FEE,
            },
            base_fee: Self::DEFAULT_BASE_FEE,
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

    pub fn with_first_month_coinbase_min(mut self, coinbase_min: u64) -> Self {
        self.first_month_coinbase_min = coinbase_min;
        self
    }

    pub fn with_coinbase_timelock_min(mut self, coinbase_max: u64) -> Self {
        self.coinbase_timelock_min = coinbase_max;
        self
    }

    fn to_blockchain_constants_v0_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let max_block_size = Atom::new(allocator, self.max_block_size).as_noun();
        let blocks_per_epoch = Atom::new(allocator, self.blocks_per_epoch).as_noun();
        let target_epoch_duration = self.target_epoch_duration.to_noun(allocator);
        let update_candidate_timestamp_interval_atoms =
            UBig::from(self.update_candidate_timestamp_interval.0) << 64; // convert seconds to atoms
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

        T(
            allocator,
            &[
                max_block_size, blocks_per_epoch, target_epoch_duration,
                update_candidate_timestamp_interval, max_future_timestamp, min_past_blocks,
                genesis_target_atom, max_target_atom, check_pow_flag, coinbase_timelock_min,
                pow_len, max_coinbase_split, first_month_coinbase_min,
            ],
        )
    }
}

impl NounEncode for BlockchainConstants {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let v1_phase = Atom::new(allocator, self.v1_phase).as_noun();
        let note_data = self.note_data.to_noun(allocator);
        let base_fee = Atom::new(allocator, self.base_fee).as_noun();
        let blockchain_constants_v0 = self.to_blockchain_constants_v0_noun(allocator);

        T(
            allocator,
            &[v1_phase, note_data, base_fee, blockchain_constants_v0],
        )
    }
}

impl IntoSlab for BlockchainConstants {
    fn into_slab(self) -> NounSlab {
        let mut slab = NounSlab::new();
        let noun = self.to_noun(&mut slab);
        slab.set_root(noun);
        slab
    }
}

#[cfg(test)]
mod tests {
    use ibig::UBig;

    use super::*;

    #[test]
    fn default_blockchain_constants_match_hoon_definition() {
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
        assert_eq!(constants.v1_phase, 40_000, "v1-phase mismatch");
        assert_eq!(
            constants.note_data,
            NoteDataConstraints {
                max_size: 2_048,
                min_fee: 256,
            },
            "note-data mismatch",
        );
        assert_eq!(constants.base_fee, 256, "base-fee mismatch");
    }

    #[test]
    fn with_v1_phase_overrides_default() {
        let constants = BlockchainConstants::new().with_v1_phase(54_321);

        assert_eq!(constants.v1_phase, 54_321);
    }

    #[test]
    fn blockchain_constants_encode_in_new_v1_wrapper() {
        let slab = BlockchainConstants::new().into_slab();
        let root = unsafe { *slab.root() };

        let outer = root.as_cell().expect("outer tuple");
        let v1_phase_atom = outer.head().as_atom().expect("v1-phase should be atom");
        assert_eq!(
            v1_phase_atom.as_u64().expect("v1-phase as u64"),
            BlockchainConstants::DEFAULT_V1_PHASE
        );

        let rest = outer.tail().as_cell().expect("rest tuple");
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

        let base_fee_and_v0 = rest.tail().as_cell().expect("base-fee and v0 tuple");
        let base_fee_atom = base_fee_and_v0.head().as_atom().expect("base-fee atom");
        assert_eq!(
            base_fee_atom.as_u64().expect("base-fee as u64"),
            BlockchainConstants::DEFAULT_BASE_FEE
        );

        let v0_tuple = base_fee_and_v0.tail().as_cell().expect("v0 tuple");
        let max_block_size_atom = v0_tuple.head().as_atom().expect("max-block-size atom");
        assert_eq!(
            max_block_size_atom.as_u64().expect("max-block-size as u64"),
            BlockchainConstants::DEFAULT_MAX_BLOCK_SIZE
        );
    }
}
