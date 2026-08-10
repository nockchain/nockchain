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

/// Aserti3-2d difficulty adjustment parameters — one instance per
/// puzzle. Mirrors the Hoon `+$ zk-asert` / `+$ ai-asert` types in
/// `hoon/common/tx-engine-1.hoon`: same field shape, each carries its
/// own defaults at the type level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsertParams {
    pub phase: u64,
    pub anchor_height: u64,
    pub anchor_target_atom: UBig,
    pub ideal_block_time: u64,
    pub half_life: u64,
    /// Median-of-11 timestamp at the anchor block, pinned as a protocol
    /// constant at phase-2 cutover (014-aletheia). Replaces the
    /// previous walk through `min-timestamps` to the anchor. Must stay
    /// in sync with the corresponding Hoon `+$ zk-asert` / etc. types
    /// in `hoon/common/tx-engine-1.hoon`.
    pub anchor_min_timestamp: u64,
}

impl AsertParams {
    /// Defaults for the ZK puzzle ASERT pre-AI-activation regime,
    /// matching `+$ zk-asert`'s `$~` clause in
    /// `hoon/common/tx-engine-1.hoon`. 150s ideal — current mainnet.
    pub fn zk_default() -> Self {
        Self {
            phase: 65_500,
            anchor_height: 65_499,
            anchor_target_atom: UBig::from(1u64) << 291,
            ideal_block_time: 150,
            half_life: 12 * 60 * 60,
            // Mainnet phase-2 hardcoded anchor median-of-11 at height 65,499.
            anchor_min_timestamp: 9_223_372_093_639_027_842,
        }
    }

    /// Defaults for the ZK puzzle ASERT post-AI-activation regime,
    /// matching `+$ zk-asert-post-ai`'s `$~` clause. A 214s ideal gives
    /// ZK about 70% of blocks when paired with AI's 500s ideal; their rates
    /// retain the chain's 150s global cadence within one tenth of a second.
    pub fn zk_post_ai_default() -> Self {
        Self {
            phase: BlockchainConstants::DEFAULT_AI_POW_ACTIVATION_HEIGHT,
            anchor_height: BlockchainConstants::DEFAULT_AI_POW_ACTIVATION_HEIGHT - 1,
            // Preserve the calibrated ZK work rate at the 214s interval.
            anchor_target_atom: (UBig::from(375u64) * (UBig::from(1u64) << 291))
                / UBig::from(214u64),
            // 214s ideal => ZK wins about 70% of blocks (paired with AI's 500s).
            ideal_block_time: 214,
            half_life: 12 * 60 * 60,
            // Recovered from the activation predecessor's validated branch
            // through the shared puzzle-keyed ASERT anchor cache.
            anchor_min_timestamp: 0,
        }
    }

    /// Defaults for the AI puzzle ASERT, matching `+$ ai-asert`'s `$~`
    /// clause in tx-engine-1.hoon. Its anchor is the final pre-activation
    /// block, so the shared puzzle-keyed re-pin schedule can recover the
    /// branch timestamp before the first AI block arrives.
    ///
    /// The anchor sets the AI puzzle's LAUNCH BLOCK INTERVAL and its launch
    /// fork-choice weight: a post-activation block's heaviness is the expected
    /// work at its own target (Hoon `+block-work-at`), so `2^256 / anchor`
    /// below is also the anchor block's weight in MAC-equivalents.
    ///
    /// An `%ai-pow` target prices one MAC-equivalent of matmul, so
    /// `2^256 / anchor` is the expected MAC-equivalents per block. `2^192` is
    /// `2^64` of them — about 3.7e16 MAC/s at the 500s ideal, about a hundred
    /// consumer GPUs at the 200-400 TeraMAC/s a 4090/5090 does in Pearl pools.
    ///
    /// It must also stay at or below `2^232 - 1` (Hoon `+max-ai-target-atom`,
    /// `ai_pow::difficulty::AI_POW_MAX_CONSENSUS_TARGET`): above that, the
    /// shape-scaled jackpot threshold leaves the 256-bit domain and every AI
    /// block at the anchor is rejected.
    ///
    /// The Rust and Hoon defaults must match because fakenet constants cross
    /// the noun boundary.
    pub fn ai_default() -> Self {
        Self {
            phase: BlockchainConstants::DEFAULT_AI_POW_ACTIVATION_HEIGHT,
            anchor_height: BlockchainConstants::DEFAULT_AI_POW_ACTIVATION_HEIGHT - 1,
            anchor_target_atom: UBig::from(1u64) << 192,
            // 500s ideal => AI wins about 30% of blocks (paired with ZK's 214s).
            ideal_block_time: 500,
            half_life: 12 * 60 * 60,
            // Recovered from the activation predecessor's validated branch
            // through the shared puzzle-keyed ASERT anchor cache.
            anchor_min_timestamp: 0,
        }
    }
}

impl NounEncode for AsertParams {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let phase = Atom::new(allocator, self.phase).as_noun();
        let anchor_height = Atom::new(allocator, self.anchor_height).as_noun();
        let anchor_target_atom = Atom::from_ubig(allocator, &self.anchor_target_atom).as_noun();
        let ideal_block_time = Atom::new(allocator, self.ideal_block_time).as_noun();
        let half_life = Atom::new(allocator, self.half_life).as_noun();
        let anchor_min_timestamp = Atom::new(allocator, self.anchor_min_timestamp).as_noun();
        T(
            allocator,
            &[
                phase, anchor_height, anchor_target_atom, ideal_block_time, half_life,
                anchor_min_timestamp,
            ],
        )
    }
}

/// Decode a `UBig` from a noun atom (little-endian bytes).
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

/// Decode a `Seconds` value stored as `UBig(seconds) << 64` (the
/// `update-candidate-timestamp-interval` encoding; the low 64 bits must be zero).
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

impl NounDecode for AsertParams {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let mut c = noun
            .in_space(space)
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;
        let phase = u64::from_noun(&c.head().noun(), space)?;
        c = c
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;
        let anchor_height = u64::from_noun(&c.head().noun(), space)?;
        c = c
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;
        let anchor_target_atom = decode_ubig(&c.head().noun(), space, "asert-anchor-target-atom")?;
        c = c
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;
        let ideal_block_time = u64::from_noun(&c.head().noun(), space)?;
        c = c
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;
        let half_life = u64::from_noun(&c.head().noun(), space)?;
        let anchor_min_timestamp = u64::from_noun(&c.tail().noun(), space)?;
        Ok(Self {
            phase,
            anchor_height,
            anchor_target_atom,
            ideal_block_time,
            half_life,
            anchor_min_timestamp,
        })
    }
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
    // NOTE: the Hoon `check-pow-flag` constant has been removed from the
    // public API. PoW is unconditionally verified by the kernel (the
    // no-pow skip path was deleted). The noun slot is preserved (always
    // %.y) by to_noun to keep the blockchain-constants:v0 shape stable —
    // removing the slot would be a cascading constants-version migration.
    pub coinbase_timelock_min: u64,
    pub pow_len: u64,
    pub max_coinbase_split: u64,
    pub first_month_coinbase_min: u64,
    pub v1_phase: u64,
    pub bythos_phase: u64,
    pub note_data: NoteDataConstraints,
    pub base_fee: u64,
    pub input_fee_divisor: u64,
    pub zk_asert: AsertParams,
    /// ZK ASERT regime 2 — active at and after `ai_pow_activation_height`.
    /// Its 214s ideal gives ZK about 70% of blocks beside AI's 500s regime.
    pub zk_asert_post_ai: AsertParams,
    /// At and after this height, the kernel's mandatory recursive-certificate
    /// verify jet admits valid `%ai-pow` blocks. Pre-activation `%ai-pow` is
    /// rejected.
    pub ai_pow_activation_height: u64,
    pub ai_asert: AsertParams,
}

impl BlockchainConstants {
    pub const DEFAULT_MAX_BLOCK_SIZE: u64 = 16_000_000;
    pub const DEFAULT_BLOCKS_PER_EPOCH: u64 = 2_016;
    pub const DEFAULT_TARGET_EPOCH_DURATION: u64 = 1_209_600;
    pub const DEFAULT_UPDATE_CANDIDATE_TIMESTAMP_INTERVAL_SECS: u64 = 300;
    pub const DEFAULT_MAX_FUTURE_TIMESTAMP: u64 = 7_200;
    pub const DEFAULT_GENESIS_TARGET_ATOM: &str =
        "0x3ffffffec0000003bffffff88000000b3ffffff34000000b3ffffff880000003bffffffec0000";
    pub const DEFAULT_MAX_TIP5_ATOM: &str =
        "0xfffffffb0000000effffffe20000002cffffffcd0000002cffffffe20000000efffffffb00000000";
    pub const DEFAULT_MIN_PAST_BLOCKS: u64 = 11;
    // Fixed value emitted for the legacy `check-pow-flag` noun slot in
    // blockchain-constants:v0. PoW is always verified; this slot exists
    // only to preserve the noun shape. Not a tunable knob.
    pub const CHECK_POW_FLAG_NOUN_VALUE: bool = true;
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
    pub const DEFAULT_AI_POW_ACTIVATION_HEIGHT: u64 = 126_000;

    pub fn new() -> Self {
        let max_target_atom = UBig::from_str_with_radix_prefix(Self::DEFAULT_MAX_TIP5_ATOM)
            .expect("Failed to parse max tip5 atom");
        let genesis_target_atom =
            UBig::from_str_with_radix_prefix(Self::DEFAULT_GENESIS_TARGET_ATOM)
                .expect("Failed to parse genesis target atom");

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
            zk_asert: AsertParams::zk_default(),
            zk_asert_post_ai: AsertParams::zk_post_ai_default(),
            ai_pow_activation_height: Self::DEFAULT_AI_POW_ACTIVATION_HEIGHT,
            ai_asert: AsertParams::ai_default(),
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

    pub fn with_zk_asert(mut self, zk_asert: AsertParams) -> Self {
        self.zk_asert = zk_asert;
        self
    }

    pub fn with_zk_asert_post_ai(mut self, zk_asert_post_ai: AsertParams) -> Self {
        self.zk_asert_post_ai = zk_asert_post_ai;
        self
    }

    pub fn with_ai_asert(mut self, ai_asert: AsertParams) -> Self {
        self.ai_asert = ai_asert;
        self
    }

    pub fn with_ai_pow_activation_height(mut self, h: u64) -> Self {
        self.ai_pow_activation_height = h;
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
        // Legacy noun slot — always %.y. PoW is unconditionally verified;
        // the slot is retained only to keep the v0 noun shape stable.
        let check_pow_flag = Self::CHECK_POW_FLAG_NOUN_VALUE.to_noun(allocator);
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
        // blockchain-constants:v1 — current Hoon shape per
        // `hoon/common/tx-engine-1.hoon`. 10-slot right-fold:
        //
        //   slot 1 : v1-phase
        //   slot 2 : bythos-phase
        //   slot 3 : data sub-cell [max-size min-fee]
        //   slot 4 : base-fee
        //   slot 5 : input-fee-divisor
        //   slot 6 : blockchain-constants:v0 (13-atom sub-cell)
        //   slot 7 : zk-asert sub-cell (regime 1, pre-AI: 150s ideal)
        //   slot 8 : zk-asert-post-ai sub-cell (regime 2, post-AI: 214s ideal)
        //   slot 9 : ai-pow-activation-height
        //   slot 10: ai-asert sub-cell
        let v1_phase = Atom::new(allocator, self.v1_phase).as_noun();
        let bythos_phase = Atom::new(allocator, self.bythos_phase).as_noun();
        let note_data = self.note_data.to_noun(allocator);
        let base_fee = Atom::new(allocator, self.base_fee).as_noun();
        let input_fee_divisor = Atom::new(allocator, self.input_fee_divisor).as_noun();
        let v0_fields = self.to_blockchain_constants_v0_fields(allocator);
        let v0_constants = T(allocator, &v0_fields);
        let zk_asert = self.zk_asert.to_noun(allocator);
        let zk_asert_post_ai = self.zk_asert_post_ai.to_noun(allocator);
        let ai_pow_activation_height =
            Atom::new(allocator, self.ai_pow_activation_height).as_noun();
        let ai_asert = self.ai_asert.to_noun(allocator);

        T(
            allocator,
            &[
                v1_phase, bythos_phase, note_data, base_fee, input_fee_divisor, v0_constants,
                zk_asert, zk_asert_post_ai, ai_pow_activation_height, ai_asert,
            ],
        )
    }
}

impl NounDecode for BlockchainConstants {
    /// Inverse of [`to_noun`](Self::to_noun) — the `blockchain-constants:v1`
    /// 10-slot right-fold (see that method): v1-phase, bythos-phase, note-data,
    /// base-fee, input-fee-divisor, v0 (13-atom sub-cell), zk-asert,
    /// zk-asert-post-ai, ai-pow-activation-height, ai-asert.
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

        // slot 6: v0 (blockchain-constants:v0) sub-cell; slots 7-10 in the tail.
        let mut v0 = outer
            .head()
            .noun()
            .in_space(space)
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;
        outer = outer
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
        // legacy check-pow slot (always %.y); PoW is unconditionally verified.
        let _check_pow_flag = bool::from_noun(&v0.head().noun(), space)?;
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

        // slots 7-10: the three AsertParams regimes + the AI activation height.
        let zk_asert = AsertParams::from_noun(&outer.head().noun(), space)?;
        outer = outer
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;
        let zk_asert_post_ai = AsertParams::from_noun(&outer.head().noun(), space)?;
        outer = outer
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;
        let ai_pow_activation_height = u64::from_noun(&outer.head().noun(), space)?;
        let ai_asert = AsertParams::from_noun(&outer.tail().noun(), space)?;

        Ok(Self {
            max_block_size,
            blocks_per_epoch,
            target_epoch_duration,
            update_candidate_timestamp_interval,
            max_future_timestamp,
            min_past_blocks,
            genesis_target_atom,
            max_target_atom,
            coinbase_timelock_min,
            pow_len,
            max_coinbase_split,
            first_month_coinbase_min,
            v1_phase,
            bythos_phase,
            note_data,
            base_fee,
            input_fee_divisor,
            zk_asert,
            zk_asert_post_ai,
            ai_pow_activation_height,
            ai_asert,
        })
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

pub fn fakenet_blockchain_constants(pow_len: u64, target_bex: u64) -> BlockchainConstants {
    // Fakenet starts from the mainnet defaults and overrides only the fields
    // that are intentionally relaxed for local testing.
    //
    // `update_candidate_timestamp_interval = 5s` (not 5 minutes) so the
    // kernel's post-poke `update-candidate-block` quickly emits a `%mine`
    // effect after the miner pokes `enable-mining`. With the mainnet
    // default of 5 minutes, the first candidate emission lags ~5min,
    // which is impractical for a fakenet smoke test.
    BlockchainConstants::new()
        .with_update_candidate_timestamp_interval(Seconds(5))
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

    /// Walk an `AsertParams` sub-cell (6-atom right-fold) and check each
    /// field matches the expected `AsertParams` value. Noun reads are bound
    /// to the source `NounSpace`.
    fn assert_asert_subcell(noun: Noun, space: &nockvm::noun::NounSpace, expected: &AsertParams) {
        // slot 1 of 6: phase
        let c1 = noun
            .in_space(space)
            .as_cell()
            .expect("asert sub-cell is a cell");
        assert_eq!(
            c1.head().as_atom().unwrap().as_u64().unwrap(),
            expected.phase,
            "asert.phase"
        );
        // slot 2: anchor-height
        let c2 = c1.tail().as_cell().expect("c2");
        assert_eq!(
            c2.head().as_atom().unwrap().as_u64().unwrap(),
            expected.anchor_height,
            "asert.anchor_height"
        );
        // slot 3: anchor-target-atom (big atom; just confirm shape)
        let c3 = c2.tail().as_cell().expect("c3");
        assert!(c3.head().is_atom(), "asert.anchor_target_atom is atom");
        // slot 4: ideal-block-time
        let c4 = c3.tail().as_cell().expect("c4");
        assert_eq!(
            c4.head().as_atom().unwrap().as_u64().unwrap(),
            expected.ideal_block_time,
            "asert.ideal_block_time"
        );
        // slot 5: half-life
        let c5 = c4.tail().as_cell().expect("c5");
        assert_eq!(
            c5.head().as_atom().unwrap().as_u64().unwrap(),
            expected.half_life,
            "asert.half_life"
        );
        // slot 6: anchor-min-timestamp (terminal — sits in c5.tail as atom)
        assert_eq!(
            c5.tail().as_atom().unwrap().as_u64().unwrap(),
            expected.anchor_min_timestamp,
            "asert.anchor_min_timestamp"
        );
    }

    /// Walk the BlockchainConstants noun shape: 10-slot right-fold
    /// `[v1-phase bythos-phase data base-fee input-fee-divisor
    ///   v0-sub-cell zk-asert zk-asert-post-ai ai-pow-activation-height
    ///   ai-asert]`. The three ASERT sub-cells are 6-atom right-folds
    /// (see `AsertParams::to_noun`).
    fn assert_blockchain_constants_shape(constants: &BlockchainConstants) {
        use nockvm::noun::NounAllocator;
        let mut slab: NounSlab = NounSlab::new();
        let noun = constants.to_noun(&mut slab);
        slab.set_root(noun);
        let space = slab.noun_space();
        let root = unsafe { *slab.root() };

        // slot 1: v1_phase
        let c1 = root.in_space(&space).as_cell().expect("root is a cell");
        assert_eq!(
            c1.head().as_atom().unwrap().as_u64().unwrap(),
            constants.v1_phase,
            "slot 1 = v1_phase"
        );
        // slot 2: bythos_phase
        let c2 = c1.tail().as_cell().expect("c2");
        assert_eq!(
            c2.head().as_atom().unwrap().as_u64().unwrap(),
            constants.bythos_phase,
            "slot 2 = bythos_phase"
        );
        // slot 3: data sub-cell [max-size min-fee]
        let c3 = c2.tail().as_cell().expect("c3");
        let nd = c3.head().as_cell().expect("data sub-cell");
        assert_eq!(
            nd.head().as_atom().unwrap().as_u64().unwrap(),
            constants.note_data.max_size,
        );
        assert_eq!(
            nd.tail().as_atom().unwrap().as_u64().unwrap(),
            constants.note_data.min_fee,
        );
        // slot 4: base_fee
        let c4 = c3.tail().as_cell().expect("c4");
        assert_eq!(
            c4.head().as_atom().unwrap().as_u64().unwrap(),
            constants.base_fee,
            "slot 4 = base_fee"
        );
        // slot 5: input_fee_divisor
        let c5 = c4.tail().as_cell().expect("c5");
        assert_eq!(
            c5.head().as_atom().unwrap().as_u64().unwrap(),
            constants.input_fee_divisor,
            "slot 5 = input_fee_divisor"
        );
        // slot 6: v0 sub-cell (13-atom right-fold)
        let c6 = c5.tail().as_cell().expect("c6");
        let v0 = c6.head().noun();
        assert_eq!(tuple_len(v0, &space), 13, "slot 6 = v0 13-atom right-fold");
        // slot 7: zk-asert sub-cell (pre-AI regime)
        let c7 = c6.tail().as_cell().expect("c7");
        assert_asert_subcell(c7.head().noun(), &space, &constants.zk_asert);
        // slot 8: zk-asert-post-ai sub-cell (post-AI regime)
        let c8 = c7.tail().as_cell().expect("c8");
        assert_asert_subcell(c8.head().noun(), &space, &constants.zk_asert_post_ai);
        // slot 9: ai-pow-activation-height
        let c9 = c8.tail().as_cell().expect("c9");
        assert_eq!(
            c9.head().as_atom().unwrap().as_u64().unwrap(),
            constants.ai_pow_activation_height,
            "slot 9 = ai_pow_activation_height"
        );
        // slot 10: ai-asert sub-cell (terminal — sits in c9.tail as cell-noun)
        assert_asert_subcell(c9.tail().noun(), &space, &constants.ai_asert);
    }

    #[test]
    fn to_noun_matches_current_hoon_source_shape() {
        assert_blockchain_constants_shape(&default_fakenet_blockchain_constants());
    }

    #[test]
    fn blockchain_constants_roundtrip_from_noun_for_mainnet_and_fakenet() {
        // `from_noun(to_noun(c)) == c` guards the hand-written v1 10-slot encode/
        // decode (incl. the `AsertParams` trios) against any encode↔decode asymmetry.
        for constants in [BlockchainConstants::new(), default_fakenet_blockchain_constants()] {
            let mut slab: NounSlab = NounSlab::new();
            let noun = constants.to_noun(&mut slab);
            slab.set_root(noun);
            let space = slab.noun_space();
            let root = unsafe { *slab.root() };
            let decoded = BlockchainConstants::from_noun(&root, &space)
                .expect("from_noun(to_noun(c)) must decode");
            assert_eq!(
                decoded, constants,
                "blockchain-constants noun round-trip must be identity"
            );
        }
    }

    #[test]
    fn blockchain_constants_new_defaults_are_valid() {
        let constants = BlockchainConstants::new();

        assert_eq!(
            constants.max_block_size, 16_000_000,
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
        // zk-asert defaults: 150s pre-AI;
        // zk-asert-post-ai defaults: 214s post-AI activation.
        assert_eq!(
            constants.zk_asert,
            AsertParams::zk_default(),
            "zk-asert mismatch"
        );
        assert_eq!(
            constants.zk_asert_post_ai,
            AsertParams::zk_post_ai_default(),
            "zk-asert-post-ai mismatch"
        );
        assert_eq!(
            constants.ai_pow_activation_height, 126_000,
            "ai-pow-activation-height mismatch"
        );
        assert_eq!(
            constants.zk_asert_post_ai,
            AsertParams {
                phase: 126_000,
                anchor_height: 125_999,
                anchor_target_atom: (UBig::from(375u64) * (UBig::from(1u64) << 291))
                    / UBig::from(214u64),
                ideal_block_time: 214,
                half_life: 12 * 60 * 60,
                anchor_min_timestamp: 0,
            },
            "zk-asert post-AI schedule mismatch"
        );
        assert_eq!(
            constants.ai_asert,
            AsertParams {
                phase: 126_000,
                anchor_height: 125_999,
                anchor_target_atom: UBig::from(1u64) << 192,
                ideal_block_time: 500,
                half_life: 12 * 60 * 60,
                anchor_min_timestamp: 0,
            },
            "ai-asert schedule mismatch"
        );
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
    fn with_zk_asert_overrides_default() {
        let custom = AsertParams {
            phase: 10,
            anchor_height: 9,
            anchor_target_atom: UBig::from(1u64) << 2,
            ideal_block_time: 60,
            half_life: 3_600,
            anchor_min_timestamp: 1_234_567,
        };
        let constants = BlockchainConstants::new().with_zk_asert(custom.clone());
        assert_eq!(constants.zk_asert, custom);
    }

    #[test]
    fn with_ai_asert_overrides_default() {
        let custom = AsertParams {
            phase: 100,
            anchor_height: 100,
            anchor_target_atom: UBig::from(1u64) << 10,
            ideal_block_time: 600,
            half_life: 3_600,
            anchor_min_timestamp: 7_654_321,
        };
        let constants = BlockchainConstants::new().with_ai_asert(custom.clone());
        assert_eq!(constants.ai_asert, custom);
    }

    #[test]
    fn with_ai_pow_activation_height_overrides_default() {
        let constants = BlockchainConstants::new().with_ai_pow_activation_height(123);
        assert_eq!(constants.ai_pow_activation_height, 123);
    }

    /// Mainnet-side companion: `BlockchainConstants::new()` round-trips
    /// through the same shape the fakenet test exercises.
    #[test]
    fn mainnet_defaults_encode_in_current_hoon_source_shape() {
        assert_blockchain_constants_shape(&BlockchainConstants::new());
    }

    /// The AI anchor sets the puzzle's launch block interval: an `%ai-pow`
    /// target prices one MAC-equivalent, so `2^256 / anchor` is the expected
    /// MAC-equivalents per block and provides the launch fork-choice work.
    /// Mirrors Hoon `test-ai-anchor-sets-the-launch-block-interval`.
    #[test]
    fn ai_anchor_sets_the_launch_block_interval() {
        let anchor = AsertParams::ai_default().anchor_target_atom;
        assert_eq!(anchor, UBig::from(1u64) << 192);
        assert_eq!(256 - (anchor.bit_len() - 1), 64);
        assert_eq!(AsertParams::ai_default().ideal_block_time, 500);
    }

    /// The AI anchor must be minable: the accept path scales it by the tile
    /// shape work factor (up to 2^24) in 256 bits, fail-closed. An anchor above
    /// `2^232 - 1` rejects every AI block, and the AI ASERT — which only
    /// advances on an accepted AI block — never escapes it. Mirrors Hoon
    /// `+max-ai-target-atom` and `ai_pow::difficulty::AI_POW_MAX_CONSENSUS_TARGET`.
    #[test]
    fn ai_anchor_is_inside_the_minable_target_domain() {
        let max_consensus_target = (UBig::from(1u64) << 232) - UBig::from(1u64);
        assert!(AsertParams::ai_default().anchor_target_atom <= max_consensus_target);
    }
}
