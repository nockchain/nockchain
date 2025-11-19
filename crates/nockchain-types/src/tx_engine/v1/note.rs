use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::noun::NounAllocatorExt;
use nockapp::utils::make_tas;
use nockapp::{AtomExt, Noun};
use nockchain_math::noun_ext::NounMathExt;
use nockchain_math::structs::HoonMapIter;
use nockchain_math::zoon::common::DefaultTipHasher;
use nockchain_math::zoon::zmap;
use nockvm::noun::{NounAllocator, D};
use noun_serde::{NounDecode, NounDecodeError, NounEncode};

use crate::tx_engine::common::{BlockHeight, Hash, Name, Nicks, Version};
use crate::tx_engine::v0::NoteV0;

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
        let keys_noun_map = self.0.iter().fold(D(0), |map, (name, note)| {
            let mut key = name.to_noun(stack);
            let mut value = note.to_noun(stack);
            zmap::z_map_put(stack, &map, &mut key, &mut value, &DefaultTipHasher).unwrap()
        });
        keys_noun_map
    }
}

impl NounDecode for Balance {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let notes = HoonMapIter::from(*noun)
            .filter(|kv| kv.is_cell())
            .map(|kv| {
                let [k, v] = kv.uncell()?;
                let name = Name::from_noun(&k)?;
                let note = <Note as NounDecode>::from_noun(&v)?;

                Ok((name, note))
            })
            .collect::<Result<Vec<_>, NounDecodeError>>()?;
        Ok(Balance(notes))
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
            Note::V0(note) => NoteV0::to_noun(&note, stack),
            Note::V1(note) => NoteV1::to_noun(&note, stack),
        }
    }
}

impl NounDecode for Note {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let hed = noun.as_cell()?.head();
        match hed.is_cell() {
            true => Ok(Note::V0(NoteV0::from_noun(noun)?)),
            false => Ok(Note::V1(NoteV1::from_noun(noun)?)),
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteDataEntry {
    pub key: String,
    // Jammed Bytes
    pub blob: bytes::Bytes,
}

impl NoteDataEntry {
    pub fn new(key: String, value: bytes::Bytes) -> Self {
        Self { key, blob: value }
    }
}

impl NounEncode for NoteData {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        self.0.iter().fold(D(0), |map, entry| {
            let mut key = make_tas(allocator, &entry.key).as_noun();
            // TODO error if key is not a belt

            let mut slab: NounSlab<NockJammer> = NounSlab::new();
            // TODO: fix cue_into to take &Bytes so we don't have to clone
            slab.cue_into(entry.blob.clone())
                .expect("failed to cue blob");
            let mut value = unsafe {
                let &root = slab.root();
                allocator.copy_into(root)
            };
            zmap::z_map_put(allocator, &map, &mut key, &mut value, &DefaultTipHasher)
                .expect("failed to encode note-data entry")
        })
    }
}

impl NounDecode for NoteData {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let entries = HoonMapIter::from(*noun)
            .filter(|entry| entry.is_cell())
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

                let mut slab: NounSlab<NockJammer> = NounSlab::new();
                slab.copy_into(raw_value);
                let jam = slab.jam();
                Ok(NoteDataEntry { key, blob: jam })
            })
            .collect::<Result<Vec<_>, NounDecodeError>>()?;

        Ok(Self(entries))
    }
}
