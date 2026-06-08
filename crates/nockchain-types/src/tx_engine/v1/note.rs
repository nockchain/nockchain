use bytes::Bytes;
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::noun::NounEncodeJamExt;
use nockapp::utils::make_tas;
use nockapp::Noun;
use nockchain_math::belt::Belt;
use nockchain_math::noun_ext::NounMathExtHandle;
use nockchain_math::owned_based_noun::{owned_based_noun_decode_error, OwnedBasedNoun};
use nockchain_math::structs::collect_zmap_entries_strict;
use nockchain_math::zoon::common::DefaultTipHasher;
use nockchain_math::zoon::zmap::{self, ZMap};
use nockvm::noun::{NounAllocator, NounSpace, D};
use noun_serde::{NounDecode, NounDecodeError, NounEncode};

use crate::tx_engine::common::{BlockHeight, Hash, Name, Nicks, Version};
use crate::tx_engine::v0::NoteV0;
use crate::tx_engine::v1::tx::Lock;

#[derive(Debug, Clone, PartialEq, Eq, NounDecode, NounEncode)]
pub struct BalanceUpdate {
    pub height: BlockHeight,
    pub block_id: Hash,
    pub notes: Balance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Balance(pub Vec<(Name, Note)>);

impl NounEncode for Balance {
    fn to_noun<A: NounAllocator>(&self, stack: &mut A) -> Noun {
        ZMap::try_from_entries(self.0.clone())
            .expect("balance z-map should encode")
            .to_noun(stack)
    }
}

impl NounDecode for Balance {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        Ok(Balance(
            ZMap::<Name, Note>::from_noun(noun, space)?.into_entries(),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    V0(NoteV0),
    V1(NoteV1),
}
/// Version 1 note representation
#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct NoteV1 {
    pub version: Version,
    pub origin_page: BlockHeight,
    pub name: Name,
    pub note_data: NoteData,
    pub assets: Nicks,
}

impl NounEncode for Note {
    fn to_noun<A: NounAllocator>(&self, stack: &mut A) -> Noun {
        match self {
            Note::V0(note) => NoteV0::to_noun(note, stack),
            Note::V1(note) => NoteV1::to_noun(note, stack),
        }
    }
}

impl NounDecode for Note {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let cell = noun.in_space(space).as_cell()?;
        let hed = cell.head().noun();
        match hed.is_cell() {
            true => Ok(Note::V0(NoteV0::from_noun(noun, space)?)),
            false => Ok(Note::V1(NoteV1::from_noun(noun, space)?)),
        }
    }
}

impl NoteV1 {
    pub fn new(origin_page: BlockHeight, name: Name, note_data: NoteData, assets: Nicks) -> Self {
        Self {
            version: Version::V1,
            origin_page,
            name,
            note_data,
            assets,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteData(pub Vec<NoteDataEntry>);

impl NoteData {
    pub fn new(entries: Vec<NoteDataEntry>) -> Self {
        Self(entries)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &NoteDataEntry> {
        self.0.iter()
    }

    pub fn from_noun_entry_value(
        noun: &Noun,
        key: &str,
        space: &NounSpace,
    ) -> Result<NoteDataValue, NounDecodeError> {
        NoteDataValue::decode_for_key(key, *noun, space)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteDataValue {
    Lock { lock: Box<Lock> },
    BridgeDeposit(BridgeDepositNoteData),
    BridgeWithdrawal(BridgeWithdrawalNoteData),
    Noun(OwnedBasedNoun),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeDepositNoteData {
    pub evm_address_based: [u64; 3],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeWithdrawalNoteData {
    pub beid: Vec<Belt>,
    pub base_hash: Hash,
    pub lock_root: Hash,
    pub base_batch_end: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
enum LockPayloadNoun {
    #[noun(tag = 0)]
    V0(Lock),
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
enum BridgeDepositPayloadNoun {
    #[noun(tag = 0)]
    V0(String, [u64; 3]),
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
enum BridgeWithdrawalPayloadNoun {
    #[noun(tag = 0)]
    V0(Vec<Belt>, Hash, Hash, u64),
}

pub const NOTE_DATA_KEY_LOCK: &str = "lock";
pub const NOTE_DATA_KEY_BRIDGE_DEPOSIT: &str = "bridge";
pub const NOTE_DATA_KEY_BRIDGE_WITHDRAWAL: &str = "bridge-w";
const BRIDGE_DEPOSIT_NETWORK_BASE: &str = "base";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteDataEntry {
    pub key: String,
    pub value: NoteDataValue,
}

impl NoteDataEntry {
    pub fn new<V: Into<NoteDataValue>>(key: String, value: V) -> Self {
        Self {
            key,
            value: value.into(),
        }
    }

    pub fn lock(lock: Lock) -> Self {
        Self::new(
            NOTE_DATA_KEY_LOCK.to_string(),
            NoteDataValue::Lock {
                lock: Box::new(lock),
            },
        )
    }

    pub fn bridge_deposit(evm_address_based: [u64; 3]) -> Self {
        Self::new(
            NOTE_DATA_KEY_BRIDGE_DEPOSIT.to_string(),
            BridgeDepositNoteData { evm_address_based },
        )
    }

    pub fn bridge_withdrawal(
        beid: Vec<Belt>,
        base_hash: Hash,
        lock_root: Hash,
        base_batch_end: u64,
    ) -> Self {
        Self::new(
            NOTE_DATA_KEY_BRIDGE_WITHDRAWAL.to_string(),
            BridgeWithdrawalNoteData {
                beid,
                base_hash,
                lock_root,
                base_batch_end,
            },
        )
    }

    pub fn raw_blob(&self) -> Bytes {
        self.value.raw_blob()
    }

    pub fn from_raw_blob(key: String, blob: Bytes) -> Result<Self, NounDecodeError> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = slab.cue_into(blob).map_err(|err| {
            NounDecodeError::Custom(format!("failed to cue note-data blob: {err}"))
        })?;
        let space = slab.noun_space();
        let value = NoteDataValue::decode_for_key(&key, noun, &space).map_err(|err| {
            NounDecodeError::Custom(format!(
                "failed to decode note-data blob for key {key:?}: {err}"
            ))
        })?;
        Ok(Self { key, value })
    }

    fn value_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        self.value.to_noun(allocator)
    }
}

impl NoteDataValue {
    fn typed_payload_noun<A: NounAllocator>(&self, allocator: &mut A) -> Option<Noun> {
        match self {
            Self::Lock { lock } => {
                Some(LockPayloadNoun::V0(lock.as_ref().clone()).to_noun(allocator))
            }
            Self::BridgeDeposit(bridge) => Some(
                BridgeDepositPayloadNoun::V0(
                    BRIDGE_DEPOSIT_NETWORK_BASE.to_string(),
                    bridge.evm_address_based,
                )
                .to_noun(allocator),
            ),
            Self::BridgeWithdrawal(bridge) => Some(
                BridgeWithdrawalPayloadNoun::V0(
                    bridge.beid.clone(),
                    bridge.base_hash.clone(),
                    bridge.lock_root.clone(),
                    bridge.base_batch_end,
                )
                .to_noun(allocator),
            ),
            Self::Noun(_) => None,
        }
    }

    pub fn raw_blob(&self) -> Bytes {
        if let Self::Noun(noun) = self {
            return noun.jam_bytes();
        }

        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = self
            .typed_payload_noun(&mut slab)
            .expect("typed note-data values must encode");
        slab.set_root(noun);
        slab.jam()
    }

    pub(crate) fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        if let Some(noun) = self.typed_payload_noun(allocator) {
            return noun;
        }

        let Self::Noun(noun) = self else {
            unreachable!("typed payloads must be handled above");
        };
        noun.to_noun(allocator)
    }

    fn raw_noun_value(raw_value: Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        OwnedBasedNoun::from_noun(raw_value, space)
            .map(Self::Noun)
            .map_err(owned_based_noun_decode_error)
    }

    fn decode_for_key(
        key: &str,
        raw_value: Noun,
        space: &NounSpace,
    ) -> Result<Self, NounDecodeError> {
        Ok(match key {
            NOTE_DATA_KEY_LOCK => match LockPayloadNoun::from_noun(&raw_value, space) {
                Ok(LockPayloadNoun::V0(lock)) => Self::Lock {
                    lock: Box::new(lock),
                },
                Err(_) => Self::raw_noun_value(raw_value, space)?,
            },
            NOTE_DATA_KEY_BRIDGE_DEPOSIT => {
                match BridgeDepositPayloadNoun::from_noun(&raw_value, space) {
                    Ok(BridgeDepositPayloadNoun::V0(network, evm_address_based))
                        if network == BRIDGE_DEPOSIT_NETWORK_BASE =>
                    {
                        Self::BridgeDeposit(BridgeDepositNoteData { evm_address_based })
                    }
                    _ => Self::raw_noun_value(raw_value, space)?,
                }
            }
            NOTE_DATA_KEY_BRIDGE_WITHDRAWAL => {
                match BridgeWithdrawalPayloadNoun::from_noun(&raw_value, space) {
                    Ok(BridgeWithdrawalPayloadNoun::V0(
                        beid,
                        base_hash,
                        lock_root,
                        base_batch_end,
                    )) => Self::BridgeWithdrawal(BridgeWithdrawalNoteData {
                        beid,
                        base_hash,
                        lock_root,
                        base_batch_end,
                    }),
                    Err(_) => Self::raw_noun_value(raw_value, space)?,
                }
            }
            _ => Self::raw_noun_value(raw_value, space)?,
        })
    }
}

impl From<OwnedBasedNoun> for NoteDataValue {
    fn from(value: OwnedBasedNoun) -> Self {
        Self::Noun(value)
    }
}

impl From<Lock> for NoteDataValue {
    fn from(value: Lock) -> Self {
        Self::Lock {
            lock: Box::new(value),
        }
    }
}

impl From<BridgeDepositNoteData> for NoteDataValue {
    fn from(value: BridgeDepositNoteData) -> Self {
        Self::BridgeDeposit(value)
    }
}

impl From<BridgeWithdrawalNoteData> for NoteDataValue {
    fn from(value: BridgeWithdrawalNoteData) -> Self {
        Self::BridgeWithdrawal(value)
    }
}

impl NounEncode for NoteData {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        self.0.iter().fold(D(0), |map, entry| {
            let mut key = make_tas(allocator, &entry.key).as_noun();
            // TODO error if key is not a belt

            let mut value = entry.value_noun(allocator);
            zmap::z_map_put(allocator, &map, &mut key, &mut value, &DefaultTipHasher)
                .expect("failed to encode note-data entry")
        })
    }
}

impl NounDecode for NoteData {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let handle = noun.in_space(space);
        let entries = collect_zmap_entries_strict(&handle)
            .map_err(|_| NounDecodeError::Custom("malformed note-data z-map node".into()))?
            .into_iter()
            .map(|entry| {
                let [raw_key, raw_value] = entry.uncell().map_err(|_| {
                    NounDecodeError::Custom("note-data entry must be a cell".into())
                })?;

                let key_atom = raw_key
                    .as_atom()
                    .map_err(|_| NounDecodeError::Custom("note-data key must be an atom".into()))?;

                let key = key_atom.into_string().map_err(|err| {
                    NounDecodeError::Custom(format!(
                        "failed to convert note-data key to string: {err}"
                    ))
                })?;

                let raw_value = raw_value.noun();
                let value =
                    NoteDataValue::decode_for_key(&key, raw_value, space).map_err(|err| {
                        NounDecodeError::Custom(format!(
                            "failed to decode note-data value for key {key:?}: {err}"
                        ))
                    })?;

                Ok(NoteDataEntry {
                    key: key.clone(),
                    value,
                })
            })
            .collect::<Result<Vec<_>, NounDecodeError>>()?;

        Ok(Self(entries))
    }
}

#[cfg(test)]
mod tests {
    use nockapp::noun::NounEncodeJamExt;
    use nockchain_math::belt::PRIME;
    use nockchain_math::owned_based_noun::OwnedBasedNoun;
    use nockvm::mem::NockStack;
    use nockvm::noun::{Atom, NounAllocator};

    use super::*;
    use crate::tx_engine::v1::tx::{LockPrimitive, SpendCondition};

    #[test]
    fn typed_lock_note_data_roundtrips() {
        let note_data = NoteData::new(vec![NoteDataEntry::lock(Lock::SpendCondition(
            SpendCondition::new(vec![LockPrimitive::Burn]),
        ))]);

        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = note_data.to_noun(&mut slab);
        let space = slab.noun_space();
        let decoded =
            NoteData::from_noun(&noun, &space).expect("typed lock note-data should decode");

        assert!(matches!(
            decoded.0.as_slice(),
            [NoteDataEntry {
                key,
                value: NoteDataValue::Lock { .. }
            }] if key == NOTE_DATA_KEY_LOCK
        ));
    }

    #[test]
    fn malformed_known_note_data_payload_falls_back_to_owned_based_noun() {
        let entry = NoteDataEntry::from_raw_blob(NOTE_DATA_KEY_LOCK.to_string(), 0_u64.jam_bytes())
            .expect("based raw blob should decode");
        let note_data = NoteData::new(vec![entry.clone()]);

        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = note_data.to_noun(&mut slab);
        let space = slab.noun_space();
        let decoded =
            NoteData::from_noun(&noun, &space).expect("owned-based note-data should decode");

        assert_eq!(decoded, note_data);
        assert!(matches!(
            decoded.0.as_slice(),
            [NoteDataEntry {
                key,
                value: NoteDataValue::Noun(_)
            }] if key == NOTE_DATA_KEY_LOCK
        ));
    }

    #[test]
    fn owned_based_note_data_raw_blob_matches_jammed_noun() {
        let noun = OwnedBasedNoun::cell(
            OwnedBasedNoun::try_atom(7).expect("atom should be based"),
            OwnedBasedNoun::list(vec![
                OwnedBasedNoun::try_atom(8).expect("atom should be based"),
                OwnedBasedNoun::try_atom(9).expect("atom should be based"),
            ]),
        );
        let value = NoteDataValue::Noun(noun.clone());

        assert_eq!(value.raw_blob(), noun.jam_bytes());
    }

    #[test]
    fn unknown_note_data_value_must_be_based() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);
        let noun = Atom::new(&mut stack, PRIME).as_noun();
        let space = stack.noun_space();

        let err = NoteData::from_noun_entry_value(&noun, "mystery", &space)
            .expect_err("non-based unknown note-data should fail");

        assert!(
            err.to_string()
                .contains("owned based noun atom is not based"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn unknown_memo_note_data_value_must_be_based() {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let mut key = make_tas(&mut slab, "memo").as_noun();
        let mut value = make_tas(&mut slab, "complex v1 lock example").as_noun();
        let noun = zmap::z_map_put(&mut slab, &D(0), &mut key, &mut value, &DefaultTipHasher)
            .expect("memo note-data z-map should encode");
        let space = slab.noun_space();

        let err = NoteData::from_noun(&noun, &space)
            .expect_err("unbased unknown memo note-data should fail");

        assert!(
            err.to_string()
                .contains("failed to decode note-data value for key \"memo\""),
            "unexpected error: {err}"
        );
        assert!(
            err.to_string()
                .contains("owned based noun atom exceeded u64 range"),
            "unexpected error: {err}"
        );
    }
}
