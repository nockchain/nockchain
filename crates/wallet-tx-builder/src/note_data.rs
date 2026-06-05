use bytes::Bytes;
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::Noun;
use nockchain_types::tx_engine::common::Hash;
use nockchain_types::tx_engine::v1::note::{NoteData, NoteDataEntry};
use nockchain_types::tx_engine::v1::tx::{Lock, SpendCondition};
use noun_serde::{NounDecode, NounDecodeError, NounEncode};
use thiserror::Error;

use crate::types::RawNoteDataEntry;

/// Canonical note-data key for lock payloads.
pub const NOTE_DATA_KEY_LOCK: &str = "lock";
/// Canonical note-data key for bridge deposit payloads.
pub const NOTE_DATA_KEY_BRIDGE_DEPOSIT: &str = "bridge";
/// Canonical note-data key for bridge withdrawal payloads.
pub const NOTE_DATA_KEY_BRIDGE_WITHDRAWAL: &str = "bridge-w";

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
/// Internal noun parser for `%lock` payload shape `[%0 lock]`.
enum LockPayloadNoun {
    #[noun(tag = 0)]
    V0(Lock),
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
/// Internal noun parser for `%bridge` payload shape `[%0 %base [a b c]]`.
enum BridgeDepositPayloadNoun {
    #[noun(tag = 0)]
    V0(String, [u64; 3]),
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
/// Internal noun parser for `%bridge-w` payload shape
/// `[%0 base-event-id base-hash lock-root base-batch-end]`.
enum BridgeWithdrawalPayloadNoun {
    #[noun(tag = 0)]
    V0(Vec<u64>, Hash, Hash, u64),
}

/// Typed note-data entry constructors for canonical wallet/planner payloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypedNoteDataEntry {
    /// `%lock` => jam([%0 lock])
    Lock { lock: Box<Lock> },
    /// `%bridge` => jam([%0 %base [a b c]])
    BridgeDeposit { evm_address_based: [u64; 3] },
    /// `%bridge-w` => jam([%0 base-event-id base-hash lock-root base-batch-end])
    BridgeWithdrawal {
        base_event_id: Vec<u64>,
        base_hash: Hash,
        lock_root: Hash,
        base_batch_end: u64,
    },
}

impl TypedNoteDataEntry {
    /// Constructs a typed lock note-data entry.
    pub fn lock(lock: Lock) -> Self {
        Self::Lock {
            lock: Box::new(lock),
        }
    }

    /// Constructs a typed bridge deposit note-data entry.
    pub fn bridge_deposit(evm_address_based: [u64; 3]) -> Self {
        Self::BridgeDeposit { evm_address_based }
    }

    /// Constructs a typed bridge withdrawal note-data entry.
    pub fn bridge_withdrawal(
        base_event_id: Vec<u64>,
        base_hash: Hash,
        lock_root: Hash,
        base_batch_end: u64,
    ) -> Self {
        Self::BridgeWithdrawal {
            base_event_id,
            base_hash,
            lock_root,
            base_batch_end,
        }
    }

    /// Returns the canonical note-data key for this typed entry.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Lock { .. } => NOTE_DATA_KEY_LOCK,
            Self::BridgeDeposit { .. } => NOTE_DATA_KEY_BRIDGE_DEPOSIT,
            Self::BridgeWithdrawal { .. } => NOTE_DATA_KEY_BRIDGE_WITHDRAWAL,
        }
    }

    /// Encodes this typed entry into raw key/blob form for tx-engine note-data.
    pub fn to_raw_entry(&self) -> RawNoteDataEntry {
        let blob = match self {
            Self::Lock { lock } => jam_payload(&LockPayloadNoun::V0(lock.as_ref().clone())),
            Self::BridgeDeposit { evm_address_based } => jam_payload(
                &BridgeDepositPayloadNoun::V0("base".to_string(), *evm_address_based),
            ),
            Self::BridgeWithdrawal {
                base_event_id,
                base_hash,
                lock_root,
                base_batch_end,
            } => jam_payload(&BridgeWithdrawalPayloadNoun::V0(
                base_event_id.clone(),
                base_hash.clone(),
                lock_root.clone(),
                *base_batch_end,
            )),
        };
        RawNoteDataEntry {
            key: self.key().to_string(),
            blob,
        }
    }
}

impl RawNoteDataEntry {
    /// Encodes a typed `%lock` note-data entry.
    pub fn from_lock(lock: Lock) -> Self {
        TypedNoteDataEntry::lock(lock).to_raw_entry()
    }

    /// Encodes a typed `%bridge` note-data entry for Base deposits.
    pub fn from_bridge_deposit(evm_address_based: [u64; 3]) -> Self {
        TypedNoteDataEntry::bridge_deposit(evm_address_based).to_raw_entry()
    }

    /// Encodes a typed `%bridge-w` note-data entry for bridge withdrawals.
    pub fn from_bridge_withdrawal(
        base_event_id: Vec<u64>,
        base_hash: Hash,
        lock_root: Hash,
        base_batch_end: u64,
    ) -> Self {
        TypedNoteDataEntry::bridge_withdrawal(base_event_id, base_hash, lock_root, base_batch_end)
            .to_raw_entry()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Canonicalized view of known note-data keys.
pub enum NormalizedNoteDataKey {
    /// `%lock` payload.
    Lock,
    /// `%bridge` payload.
    Bridge,
    /// `%bridge-w` payload.
    BridgeWithdrawal,
    /// Any unrecognized key preserved verbatim.
    Other(String),
}

impl NormalizedNoteDataKey {
    /// Normalizes a raw key into known variants while preserving unknown keys.
    pub fn from_raw(raw: &str) -> Self {
        let normalized = raw.trim();
        match normalized {
            NOTE_DATA_KEY_LOCK => Self::Lock,
            NOTE_DATA_KEY_BRIDGE_DEPOSIT => Self::Bridge,
            NOTE_DATA_KEY_BRIDGE_WITHDRAWAL => Self::BridgeWithdrawal,
            other => Self::Other(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Decoded `%lock` note-data payload.
pub struct LockDataPayload {
    /// Payload schema version.
    pub version: u64,
    /// Flattened lock spend-condition leaves in deterministic left-to-right order.
    pub spend_conditions: Vec<SpendCondition>,
}

impl LockDataPayload {
    /// Parses a `%lock` payload noun with shape `[version lock]`.
    pub fn from_noun(noun: &Noun) -> Result<Self, NoteDataDecodeError> {
        let spend_conditions =
            match LockPayloadNoun::from_noun(noun).map_err(NoteDataDecodeError::from)? {
                LockPayloadNoun::V0(lock) => lock.flatten_spend_conditions(),
            };
        Ok(Self {
            version: 0,
            spend_conditions,
        })
    }

    /// Cues and parses a `%lock` payload blob.
    pub fn from_blob(blob: &Bytes) -> Result<Self, NoteDataDecodeError> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = slab
            .cue_into(blob.clone())
            .map_err(|error| NoteDataDecodeError::InvalidJam(error.to_string()))?;
        Self::from_noun(&noun)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Parsed lock form extracted from tracked lock nouns and note-data payloads.
pub struct ParsedLockForm {
    /// Flattened lock spend-condition leaves in deterministic left-to-right order.
    pub spend_conditions: Vec<SpendCondition>,
    /// Number of spend-condition leaves represented by this lock tree.
    pub spend_condition_count: u64,
}

impl ParsedLockForm {
    /// Parses a lock noun with tx-engine's canonical decoder and exposes flattened leaves.
    pub fn from_noun(noun: &Noun) -> Result<Self, NoteDataDecodeError> {
        let lock = Lock::from_noun(noun).map_err(NoteDataDecodeError::from)?;
        Ok(Self {
            spend_conditions: lock.flatten_spend_conditions(),
            spend_condition_count: lock.spend_condition_count(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Supported bridge network tags for typed bridge note-data payloads.
pub enum BridgeNetwork {
    /// Ethereum Base network.
    Base,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Decoded `%bridge` note-data payload.
pub struct BridgeDepositDataPayload {
    /// Payload schema version.
    pub version: u64,
    /// Bridge network identifier.
    pub network: BridgeNetwork,
    /// Encoded EVM address split into three words.
    pub evm_address_based: [u64; 3],
}

impl BridgeDepositDataPayload {
    /// Parses a `%bridge` payload noun with shape `[version network evm-address-based]`.
    pub fn from_noun(noun: &Noun) -> Result<Self, NoteDataDecodeError> {
        let (network, evm_address_based) =
            match BridgeDepositPayloadNoun::from_noun(noun).map_err(NoteDataDecodeError::from)? {
                BridgeDepositPayloadNoun::V0(network, evm_address_based) => {
                    (network, evm_address_based)
                }
            };
        if network != "base" {
            return Err(NoteDataDecodeError::UnsupportedBridgeNetwork(network));
        }
        Ok(Self {
            version: 0,
            network: BridgeNetwork::Base,
            evm_address_based,
        })
    }

    /// Cues and parses a `%bridge` payload blob.
    pub fn from_blob(blob: &Bytes) -> Result<Self, NoteDataDecodeError> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = slab
            .cue_into(blob.clone())
            .map_err(|error| NoteDataDecodeError::InvalidJam(error.to_string()))?;
        Self::from_noun(&noun)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Decoded `%bridge-w` note-data payload.
pub struct BridgeWithdrawalDataPayload {
    /// Payload schema version.
    pub version: u64,
    /// Source bridge event identifier segments.
    pub base_event_id: Vec<u64>,
    /// Source bridge batch hash.
    pub base_hash: Hash,
    /// Destination lock root used for withdrawal output.
    pub lock_root: Hash,
    /// Source bridge batch end height.
    pub base_batch_end: u64,
}

impl BridgeWithdrawalDataPayload {
    /// Parses a `%bridge-w` payload noun with shape
    /// `[version base-event-id base-hash lock-root base-batch-end]`.
    pub fn from_noun(noun: &Noun) -> Result<Self, NoteDataDecodeError> {
        let (base_event_id, base_hash, lock_root, base_batch_end) =
            match BridgeWithdrawalPayloadNoun::from_noun(noun).map_err(NoteDataDecodeError::from)? {
                BridgeWithdrawalPayloadNoun::V0(
                    base_event_id,
                    base_hash,
                    lock_root,
                    base_batch_end,
                ) => (base_event_id, base_hash, lock_root, base_batch_end),
            };
        Ok(Self {
            version: 0,
            base_event_id,
            base_hash,
            lock_root,
            base_batch_end,
        })
    }

    /// Cues and parses a `%bridge-w` payload blob.
    pub fn from_blob(blob: &Bytes) -> Result<Self, NoteDataDecodeError> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = slab
            .cue_into(blob.clone())
            .map_err(|error| NoteDataDecodeError::InvalidJam(error.to_string()))?;
        Self::from_noun(&noun)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Best-effort typed decode of a note-data blob.
pub enum DecodedNoteDataPayload {
    /// Successfully decoded `%lock`.
    Lock(LockDataPayload),
    /// Successfully decoded `%bridge`.
    BridgeDeposit(BridgeDepositDataPayload),
    /// Successfully decoded `%bridge-w`.
    BridgeWithdrawal(BridgeWithdrawalDataPayload),
    /// Raw or failed-to-decode payload.
    Raw,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Fully decoded note-data list with raw and typed per-entry views.
pub struct DecodedNoteData(pub Vec<DecodedNoteDataEntry>);

impl From<&NoteData> for DecodedNoteData {
    /// Decodes an entire note-data list into typed entries with error retention.
    fn from(note_data: &NoteData) -> Self {
        Self(
            note_data
                .iter()
                .map(DecodedNoteDataEntry::from_entry)
                .collect(),
        )
    }
}

impl From<NoteData> for DecodedNoteData {
    /// Decodes an owned note-data list into typed entries with error retention.
    fn from(note_data: NoteData) -> Self {
        Self(
            note_data
                .0
                .into_iter()
                .map(|entry| DecodedNoteDataEntry::from_entry(&entry))
                .collect(),
        )
    }
}

impl DecodedNoteData {
    /// Returns the first decoded lock payload entry in this decoded note-data set.
    pub fn first_decoded_lock(&self) -> Option<&LockDataPayload> {
        self.0.iter().find_map(|entry| match &entry.payload {
            DecodedNoteDataPayload::Lock(lock) => Some(lock),
            _ => None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// One decoded note-data entry with both raw and typed views.
pub struct DecodedNoteDataEntry {
    /// Original key as stored on chain.
    pub raw_key: String,
    /// Canonicalized key used for dispatch.
    pub normalized_key: NormalizedNoteDataKey,
    /// Original jammed payload blob.
    pub raw_blob: Bytes,
    /// Typed payload view when decode succeeds.
    pub payload: DecodedNoteDataPayload,
    /// Decode error message when known-key decode fails.
    pub decode_error: Option<String>,
}

impl DecodedNoteDataEntry {
    /// Decodes one raw note-data entry and preserves payload on failures.
    pub fn from_raw_entry(entry: &RawNoteDataEntry) -> Self {
        let normalized_key = NormalizedNoteDataKey::from_raw(&entry.key);
        let decode_result = match normalized_key {
            NormalizedNoteDataKey::Lock => {
                LockDataPayload::from_blob(&entry.blob).map(DecodedNoteDataPayload::Lock)
            }
            NormalizedNoteDataKey::Bridge => BridgeDepositDataPayload::from_blob(&entry.blob)
                .map(DecodedNoteDataPayload::BridgeDeposit),
            NormalizedNoteDataKey::BridgeWithdrawal => {
                BridgeWithdrawalDataPayload::from_blob(&entry.blob)
                    .map(DecodedNoteDataPayload::BridgeWithdrawal)
            }
            NormalizedNoteDataKey::Other(_) => Ok(DecodedNoteDataPayload::Raw),
        };

        match decode_result {
            Ok(payload) => Self {
                raw_key: entry.key.clone(),
                normalized_key,
                raw_blob: entry.blob.clone(),
                payload,
                decode_error: None,
            },
            Err(error) => Self {
                raw_key: entry.key.clone(),
                normalized_key,
                raw_blob: entry.blob.clone(),
                payload: DecodedNoteDataPayload::Raw,
                decode_error: Some(error.to_string()),
            },
        }
    }

    /// Decodes one tx-engine note-data entry by key and preserves payload on failures.
    pub fn from_entry(entry: &NoteDataEntry) -> Self {
        Self::from_raw_entry(&RawNoteDataEntry {
            key: entry.key.clone(),
            blob: entry.blob.clone(),
        })
    }
}

#[derive(Debug, Error)]
/// Errors surfaced by note-data payload decoding.
pub enum NoteDataDecodeError {
    #[error("invalid jam payload: {0}")]
    InvalidJam(String),
    #[error("noun decode failed: {0}")]
    NounDecode(String),
    #[error("unsupported bridge network tag: {0}")]
    UnsupportedBridgeNetwork(String),
}

impl From<NounDecodeError> for NoteDataDecodeError {
    /// Maps noun-serde failures into note-data decode errors.
    fn from(value: NounDecodeError) -> Self {
        Self::NounDecode(value.to_string())
    }
}

/// TODO(grpc): Extend balance note-data messages to include decoded/typed
/// `DecodedNoteData` alongside raw blobs so wallet/planner paths can consume
/// typed note-data without re-decoding jam payloads per consumer.
/// Jams a noun-encodable value into a byte payload suitable for note-data blobs.
fn jam_payload<T: NounEncode>(value: &T) -> Bytes {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let noun = value.to_noun(&mut slab);
    slab.set_root(noun);
    slab.jam()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use nockchain_types::tx_engine::v1::tx::{Lock, LockPrimitive, Pkh};
    use nockvm::ext::NounExt;
    use nockvm::mem::NockStack;
    use nockvm::noun::{Noun, D, T};
    use noun_serde::{NounDecode, NounEncode};

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq, NounDecode)]
    struct FixtureEntry {
        case: String,
        note_data: NoteData,
    }

    fn hash(v: u64) -> Hash {
        Hash::from_limbs(&[v, 0, 0, 0, 0])
    }

    fn hash_from_limbs(a: u64, b: u64, c: u64, d: u64, e: u64) -> Hash {
        Hash::from_limbs(&[a, b, c, d, e])
    }

    fn jam<T: NounEncode>(value: &T) -> Bytes {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = value.to_noun(&mut slab);
        slab.set_root(noun);
        slab.jam()
    }

    fn decode_fixtures() -> Vec<FixtureEntry> {
        let fixture_bytes = include_bytes!("../tests/fixtures/note_data_fixtures.jam");
        let mut stack = NockStack::new(nockapp::utils::NOCK_STACK_SIZE, 0);
        let noun = Noun::cue_bytes_slice(&mut stack, fixture_bytes).expect("fixture jam must cue");
        Vec::<FixtureEntry>::from_noun(&noun).expect("fixture noun must decode")
    }

    fn normalize_case_tag(tag: &str) -> &str {
        tag.strip_prefix('%').unwrap_or(tag)
    }

    fn fixture_note_data(case: &str) -> NoteData {
        decode_fixtures()
            .into_iter()
            .find(|fixture| normalize_case_tag(&fixture.case) == case)
            .unwrap_or_else(|| panic!("missing fixture case: {case}"))
            .note_data
    }

    #[test]
    fn normalize_note_data_keys_handles_known_bridge_keys() {
        assert_eq!(
            NormalizedNoteDataKey::from_raw(NOTE_DATA_KEY_LOCK),
            NormalizedNoteDataKey::Lock
        );
        assert_eq!(
            NormalizedNoteDataKey::from_raw(NOTE_DATA_KEY_BRIDGE_DEPOSIT),
            NormalizedNoteDataKey::Bridge
        );
        assert_eq!(
            NormalizedNoteDataKey::from_raw(NOTE_DATA_KEY_BRIDGE_WITHDRAWAL),
            NormalizedNoteDataKey::BridgeWithdrawal
        );
    }

    #[test]
    fn lock_payload_noun_decodes_tagged_zero_atom() {
        let spend = SpendCondition::new(vec![LockPrimitive::Burn]);
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = LockPayloadNoun::V0(Lock::SpendCondition(spend.clone())).to_noun(&mut slab);

        let version = noun
            .as_cell()
            .expect("tagged payload should be a cell")
            .head();

        assert_eq!(
            u64::from_noun(&version).expect("tag atom should be numeric"),
            0
        );

        assert_eq!(
            LockPayloadNoun::from_noun(&noun).expect("tagged zero atom should decode"),
            LockPayloadNoun::V0(Lock::SpendCondition(spend))
        );
    }

    #[test]
    fn typed_lock_note_data_entry_encodes_and_decodes() {
        let spend = SpendCondition::new(vec![LockPrimitive::Burn]);
        let entry = RawNoteDataEntry::from_lock(Lock::SpendCondition(spend.clone()));
        assert_eq!(entry.key, NOTE_DATA_KEY_LOCK);

        let decoded = DecodedNoteDataEntry::from_raw_entry(&entry);
        match decoded.payload {
            DecodedNoteDataPayload::Lock(lock_payload) => {
                assert_eq!(lock_payload.version, 0);
                assert_eq!(lock_payload.spend_conditions, vec![spend]);
            }
            other => panic!("expected typed lock payload, got {other:?}"),
        }
        assert!(decoded.decode_error.is_none());
    }

    #[test]
    fn typed_bridge_deposit_note_data_entry_encodes_and_decodes() {
        let entry = RawNoteDataEntry::from_bridge_deposit([11, 22, 33]);
        assert_eq!(entry.key, NOTE_DATA_KEY_BRIDGE_DEPOSIT);

        let decoded = DecodedNoteDataEntry::from_raw_entry(&entry);
        match decoded.payload {
            DecodedNoteDataPayload::BridgeDeposit(bridge_payload) => {
                assert_eq!(bridge_payload.version, 0);
                assert_eq!(bridge_payload.network, BridgeNetwork::Base);
                assert_eq!(bridge_payload.evm_address_based, [11, 22, 33]);
            }
            other => panic!("expected typed bridge deposit payload, got {other:?}"),
        }
        assert!(decoded.decode_error.is_none());
    }

    #[test]
    fn typed_bridge_withdrawal_note_data_entry_encodes_and_decodes() {
        let entry = RawNoteDataEntry::from_bridge_withdrawal(
            vec![1, 2, 3, 4],
            hash_from_limbs(1, 2, 3, 4, 5),
            hash_from_limbs(6, 7, 8, 9, 10),
            57_600,
        );
        assert_eq!(entry.key, NOTE_DATA_KEY_BRIDGE_WITHDRAWAL);

        let decoded = DecodedNoteDataEntry::from_raw_entry(&entry);
        match decoded.payload {
            DecodedNoteDataPayload::BridgeWithdrawal(bridge_payload) => {
                assert_eq!(bridge_payload.version, 0);
                assert_eq!(bridge_payload.base_event_id, vec![1, 2, 3, 4]);
                assert_eq!(bridge_payload.base_hash, hash_from_limbs(1, 2, 3, 4, 5));
                assert_eq!(bridge_payload.lock_root, hash_from_limbs(6, 7, 8, 9, 10));
                assert_eq!(bridge_payload.base_batch_end, 57_600);
            }
            other => panic!("expected typed bridge withdrawal payload, got {other:?}"),
        }
        assert!(decoded.decode_error.is_none());
    }

    #[test]
    fn fixture_jam_contains_expected_cases() {
        let cases = decode_fixtures()
            .into_iter()
            .map(|fixture| normalize_case_tag(&fixture.case).to_string())
            .collect::<BTreeSet<_>>();
        let expected = [
            "all-keys", "lock-single", "lock-v2", "lock-v4", "bridge-deposit", "lock-v8",
            "lock-v16", "lock-unsupported-version", "bridge-deposit-large",
            "bridge-unsupported-network", "bridge-unsupported-version", "bridge-withdrawal",
            "bridge-withdrawal-unsupported-version", "bridge-withdrawal-long-event", "wildcard",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
        assert_eq!(cases, expected);
    }

    #[test]
    fn decode_lock_single_note_data_payload() {
        let note_data = fixture_note_data("lock-single");
        let decoded = DecodedNoteData::from(&note_data).0;
        let entry = &decoded[0];
        match &entry.payload {
            DecodedNoteDataPayload::Lock(lock) => {
                let burn = SpendCondition::new(vec![LockPrimitive::Burn]);
                assert_eq!(lock.version, 0);
                assert_eq!(lock.spend_conditions, vec![burn]);
            }
            other => panic!(
                "expected lock payload, got {:?}, error={:?}",
                other, entry.decode_error
            ),
        }
    }

    #[test]
    fn decode_lock_v2_note_data_payload() {
        let note_data = fixture_note_data("lock-v2");
        let decoded = DecodedNoteData::from(&note_data).0;
        let entry = &decoded[0];
        match &entry.payload {
            DecodedNoteDataPayload::Lock(lock) => {
                let burn = SpendCondition::new(vec![LockPrimitive::Burn]);
                assert_eq!(lock.version, 0);
                assert_eq!(lock.spend_conditions, vec![burn.clone(), burn]);
            }
            other => panic!(
                "expected lock payload, got {:?}, error={:?}",
                other, entry.decode_error
            ),
        }
    }

    #[test]
    fn decode_lock_note_data_payload() {
        let note_data = fixture_note_data("lock-v4");
        let decoded = DecodedNoteData::from(&note_data).0;
        let entry = &decoded[0];
        match &entry.payload {
            DecodedNoteDataPayload::Lock(lock) => {
                let burn = SpendCondition::new(vec![LockPrimitive::Burn]);
                assert_eq!(lock.version, 0);
                assert_eq!(
                    lock.spend_conditions,
                    vec![burn.clone(), burn.clone(), burn.clone(), burn]
                );
            }
            other => panic!(
                "expected lock payload, got {:?}, error={:?}",
                other, entry.decode_error
            ),
        }
    }

    #[test]
    fn decode_lock_v8_note_data_payload() {
        let note_data = fixture_note_data("lock-v8");
        let decoded = DecodedNoteData::from(&note_data).0;
        let entry = &decoded[0];
        match &entry.payload {
            DecodedNoteDataPayload::Lock(lock) => {
                let burn = SpendCondition::new(vec![LockPrimitive::Burn]);
                assert_eq!(lock.version, 0);
                assert_eq!(lock.spend_conditions.len(), 8);
                assert!(lock.spend_conditions.iter().all(|sc| sc == &burn));
            }
            other => panic!(
                "expected lock payload, got {:?}, error={:?}",
                other, entry.decode_error
            ),
        }
    }

    #[test]
    fn decode_lock_v16_note_data_payload() {
        let note_data = fixture_note_data("lock-v16");
        let decoded = DecodedNoteData::from(&note_data).0;
        let entry = &decoded[0];
        match &entry.payload {
            DecodedNoteDataPayload::Lock(lock) => {
                let burn = SpendCondition::new(vec![LockPrimitive::Burn]);
                assert_eq!(lock.version, 0);
                assert_eq!(lock.spend_conditions.len(), 16);
                assert!(lock.spend_conditions.iter().all(|sc| sc == &burn));
            }
            other => panic!(
                "expected lock payload, got {:?}, error={:?}",
                other, entry.decode_error
            ),
        }
    }

    #[test]
    fn decode_lock_form_noun_reports_tree_leaf_count() {
        fn simple_sc(v: u64) -> SpendCondition {
            SpendCondition::new(vec![LockPrimitive::Pkh(Pkh::new(1, vec![hash(v)]))])
        }

        let sc1 = simple_sc(11);
        let sc2 = simple_sc(12);
        let sc3 = simple_sc(13);
        let sc4 = simple_sc(14);

        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let sc1_n = sc1.to_noun(&mut slab);
        let sc2_n = sc2.to_noun(&mut slab);
        let sc3_n = sc3.to_noun(&mut slab);
        let sc4_n = sc4.to_noun(&mut slab);

        let v2_left_pair = T(&mut slab, &[sc1_n, sc2_n]);
        let v2_right_pair = T(&mut slab, &[sc3_n, sc4_n]);
        let v4_pair = T(&mut slab, &[v2_left_pair, v2_right_pair]);
        let v4_lock = T(&mut slab, &[D(4), v4_pair]);

        let parsed = ParsedLockForm::from_noun(&v4_lock).expect("decode lock form");
        assert_eq!(parsed.spend_condition_count, 4);
        assert_eq!(parsed.spend_conditions, vec![sc1, sc2, sc3, sc4]);
    }

    #[test]
    fn recognized_keys_fallback_to_raw_on_decode_errors() {
        let malformed = jam(&42_u64);
        let note_data = NoteData::new(vec![NoteDataEntry::new("bridge".to_string(), malformed)]);

        let decoded = DecodedNoteData::from(&note_data).0;
        let entry = &decoded[0];
        assert!(matches!(entry.payload, DecodedNoteDataPayload::Raw));
        assert!(entry.decode_error.is_some());
    }

    #[test]
    fn decode_bridge_deposit_payload() {
        let note_data = fixture_note_data("bridge-deposit");
        let decoded = DecodedNoteData::from(&note_data).0;
        let entry = &decoded[0];
        match &entry.payload {
            DecodedNoteDataPayload::BridgeDeposit(bridge) => {
                assert_eq!(bridge.version, 0);
                assert_eq!(bridge.network, BridgeNetwork::Base);
                assert_eq!(bridge.evm_address_based, [11, 22, 33]);
            }
            other => panic!(
                "expected bridge payload, got {:?}, error={:?}",
                other, entry.decode_error
            ),
        }
    }

    #[test]
    fn decode_bridge_deposit_large_payload() {
        let note_data = fixture_note_data("bridge-deposit-large");
        let decoded = DecodedNoteData::from(&note_data).0;
        let entry = &decoded[0];
        match &entry.payload {
            DecodedNoteDataPayload::BridgeDeposit(bridge) => {
                assert_eq!(bridge.version, 0);
                assert_eq!(bridge.network, BridgeNetwork::Base);
                assert_eq!(
                    bridge.evm_address_based,
                    [4_200_001, 98_765_432, 1_234_567_890]
                );
            }
            other => panic!(
                "expected bridge payload, got {:?}, error={:?}",
                other, entry.decode_error
            ),
        }
    }

    #[test]
    fn decode_bridge_withdrawal_payload() {
        let note_data = fixture_note_data("bridge-withdrawal");
        let decoded = DecodedNoteData::from(&note_data).0;
        let entry = &decoded[0];
        match &entry.payload {
            DecodedNoteDataPayload::BridgeWithdrawal(bridge_w) => {
                assert_eq!(bridge_w.version, 0);
                assert_eq!(bridge_w.base_event_id, vec![1, 2, 3, 4]);
                assert_eq!(bridge_w.base_hash, hash_from_limbs(1, 2, 3, 4, 5));
                assert_eq!(bridge_w.lock_root, hash_from_limbs(6, 7, 8, 9, 10));
                assert_eq!(bridge_w.base_batch_end, 57_600);
            }
            other => panic!(
                "expected bridge withdrawal payload, got {:?}, error={:?}",
                other, entry.decode_error
            ),
        }
    }

    #[test]
    fn decode_bridge_withdrawal_long_event_payload() {
        let note_data = fixture_note_data("bridge-withdrawal-long-event");
        let decoded = DecodedNoteData::from(&note_data).0;
        let entry = &decoded[0];
        match &entry.payload {
            DecodedNoteDataPayload::BridgeWithdrawal(bridge_w) => {
                assert_eq!(bridge_w.version, 0);
                assert_eq!(bridge_w.base_event_id, vec![10, 20, 30, 40, 50, 60, 70]);
                assert_eq!(bridge_w.base_hash, hash_from_limbs(90, 80, 70, 60, 50));
                assert_eq!(bridge_w.lock_root, hash_from_limbs(15, 25, 35, 45, 55));
                assert_eq!(bridge_w.base_batch_end, 88_001);
            }
            other => panic!(
                "expected bridge withdrawal payload, got {:?}, error={:?}",
                other, entry.decode_error
            ),
        }
    }

    #[test]
    fn malformed_recognized_keys_fallback_to_raw_with_decode_error() {
        for case in [
            "lock-unsupported-version", "bridge-unsupported-network", "bridge-unsupported-version",
            "bridge-withdrawal-unsupported-version",
        ] {
            let note_data = fixture_note_data(case);
            let decoded = DecodedNoteData::from(&note_data).0;
            let entry = &decoded[0];
            assert!(
                matches!(entry.payload, DecodedNoteDataPayload::Raw),
                "case {case} should fallback to raw payload"
            );
            assert!(
                entry.decode_error.is_some(),
                "case {case} should include decode error"
            );
        }
    }

    #[test]
    fn decode_all_keys_fixture_includes_wildcard_as_raw() {
        let note_data = fixture_note_data("all-keys");
        let decoded = DecodedNoteData::from(&note_data).0;
        assert_eq!(decoded.len(), 4);
        assert!(decoded.iter().all(|entry| entry.decode_error.is_none()));

        let lock_entry = decoded
            .iter()
            .find(|entry| entry.raw_key == "lock")
            .expect("lock entry");
        match &lock_entry.payload {
            DecodedNoteDataPayload::Lock(lock) => assert_eq!(lock.spend_conditions.len(), 4),
            other => panic!("expected lock payload, got {other:?}"),
        }

        let bridge_entry = decoded
            .iter()
            .find(|entry| entry.raw_key == "bridge")
            .expect("bridge entry");
        assert!(matches!(
            bridge_entry.payload,
            DecodedNoteDataPayload::BridgeDeposit(_)
        ));

        let bridge_withdrawal_entry = decoded
            .iter()
            .find(|entry| entry.raw_key == "bridge-w")
            .expect("bridge withdrawal entry");
        assert!(matches!(
            bridge_withdrawal_entry.payload,
            DecodedNoteDataPayload::BridgeWithdrawal(_)
        ));

        let wildcard_entry = decoded
            .iter()
            .find(|entry| entry.raw_key == "memo")
            .expect("wildcard entry");
        assert!(matches!(
            wildcard_entry.payload,
            DecodedNoteDataPayload::Raw
        ));
    }
}
