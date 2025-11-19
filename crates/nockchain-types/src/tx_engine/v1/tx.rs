use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::noun::NounAllocatorExt;
use nockchain_math::noun_ext::NounMathExt;
use nockchain_math::structs::{HoonList, HoonMapIter};
use nockchain_math::zoon::common::DefaultTipHasher;
use nockchain_math::zoon::{zmap, zset};
use nockvm::ext::{make_tas, AtomExt};
use nockvm::noun::{Noun, NounAllocator, D};
use noun_serde::{NounDecode, NounDecodeError, NounEncode};

use super::note::NoteData;
use crate::tx_engine::common::{
    BlockHeight, Hash, Name, Nicks, SchnorrPubkey, SchnorrSignature, Signature, Source, TxId,
    Version,
};
use crate::v0::{TimelockRangeAbsolute, TimelockRangeRelative};

#[derive(Debug, Clone, PartialEq)]
pub struct RawTx {
    pub version: Version,
    pub id: TxId,
    pub spends: Spends,
}

impl NounEncode for RawTx {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let version = self.version.to_noun(allocator);
        let id = self.id.to_noun(allocator);
        let spends = self.spends.to_noun(allocator);
        nockvm::noun::T(allocator, &[version, id, spends])
    }
}

impl NounDecode for RawTx {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell()?;
        let version = Version::from_noun(&cell.head())?;

        let tail = cell.tail();
        let cell = tail
            .as_cell()
            .map_err(|_| NounDecodeError::Custom("raw-tx tail not a cell".into()))?;
        let id = TxId::from_noun(&cell.head())?;

        let spends = Spends::from_noun(&cell.tail())?;

        if version != Version::V1 {
            return Err(NounDecodeError::Custom("expected raw-tx version 1".into()));
        }

        Ok(Self {
            version,
            id,
            spends,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Spends(pub Vec<(Name, Spend)>);

impl NounEncode for Spends {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        self.0.iter().fold(D(0), |acc, (name, spend)| {
            let mut key = name.to_noun(allocator);
            let mut value = spend.to_noun(allocator);
            zmap::z_map_put(allocator, &acc, &mut key, &mut value, &DefaultTipHasher)
                .expect("failed to encode spends map")
        })
    }
}

impl NounDecode for Spends {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let entries = HoonMapIter::from(*noun)
            .filter(|entry| entry.is_cell())
            .map(|entry| {
                let [name_raw, spend_raw] = entry
                    .uncell()
                    .map_err(|_| NounDecodeError::Custom("spend entry must be a pair".into()))?;
                let name = Name::from_noun(&name_raw)?;
                let spend = Spend::from_noun(&spend_raw)?;
                Ok((name, spend))
            })
            .collect::<Result<Vec<_>, NounDecodeError>>()?;
        Ok(Self(entries))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Spend {
    Legacy(Spend0),
    Witness(Spend1),
}

impl NounEncode for Spend {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        match self {
            Spend::Legacy(spend) => {
                let tag = D(0);
                let value = spend.to_noun(allocator);
                nockvm::noun::T(allocator, &[tag, value])
            }
            Spend::Witness(spend) => {
                let tag = D(1);
                let value = spend.to_noun(allocator);
                nockvm::noun::T(allocator, &[tag, value])
            }
        }
    }
}

impl NounDecode for Spend {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell()?;
        let tag = cell.head().as_atom()?.as_u64()?;
        match tag {
            0 => Ok(Spend::Legacy(Spend0::from_noun(&cell.tail())?)),
            1 => Ok(Spend::Witness(Spend1::from_noun(&cell.tail())?)),
            _ => Err(NounDecodeError::InvalidEnumVariant),
        }
    }
}

#[derive(Debug, Clone, NounEncode, NounDecode, PartialEq)]
pub struct Spend0 {
    pub signature: Signature,
    pub seeds: Seeds,
    pub fee: Nicks,
}

#[derive(Debug, Clone, NounEncode, NounDecode, PartialEq)]
pub struct Spend1 {
    pub witness: Witness,
    pub seeds: Seeds,
    pub fee: Nicks,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Seeds(pub Vec<Seed>);

impl NounEncode for Seeds {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        self.0.iter().fold(D(0), |acc, seed| {
            let mut value = seed.to_noun(allocator);
            zset::z_set_put(allocator, &acc, &mut value, &DefaultTipHasher)
                .expect("failed to encode seeds set")
        })
    }
}

impl NounDecode for Seeds {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        decode_zset(noun, Seed::from_noun).map(Self)
    }
}

#[derive(Debug, Clone, NounEncode, NounDecode, PartialEq)]
pub struct Seed {
    pub output_source: Option<Source>,
    pub lock_root: Hash,
    pub note_data: NoteData,
    pub gift: Nicks,
    pub parent_hash: Hash,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Witness {
    pub lock_merkle_proof: LockMerkleProof,
    pub pkh_signature: PkhSignature,
    pub hax: Vec<HaxPreimage>,
    // should always be null (0)
    pub tim: usize,
}

impl Witness {
    pub fn new(
        lock_merkle_proof: LockMerkleProof,
        pkh_signature: PkhSignature,
        hax: Vec<HaxPreimage>,
    ) -> Self {
        Self {
            lock_merkle_proof,
            pkh_signature,
            hax,
            tim: 0,
        }
    }
}

impl NounEncode for Witness {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let lmp = self.lock_merkle_proof.to_noun(allocator);
        let pkh = self.pkh_signature.to_noun(allocator);
        let hax = self.hax.iter().fold(D(0), |acc, entry| {
            let mut key = entry.hash.to_noun(allocator);
            let mut value_noun = unsafe {
                let mut slab: NounSlab<NockJammer> = NounSlab::new();
                slab.cue_into(entry.value.clone())
                    .expect("failed to cue value");
                let &root = slab.root();
                allocator.copy_into(root)
            };
            zmap::z_map_put(
                allocator, &acc, &mut key, &mut value_noun, &DefaultTipHasher,
            )
            .expect("failed to encode witness hax map")
        });
        let tim = self.tim.to_noun(allocator);
        nockvm::noun::T(allocator, &[lmp, pkh, hax, tim])
    }
}

impl NounDecode for Witness {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell()?;
        let lock_merkle_proof = LockMerkleProof::from_noun(&cell.head())?;

        let tail = cell.tail();
        let cell = tail
            .as_cell()
            .map_err(|_| NounDecodeError::Custom("witness tail not a cell".into()))?;
        let pkh_signature = PkhSignature::from_noun(&cell.head())?;

        let tail = cell.tail();
        let cell = tail
            .as_cell()
            .map_err(|_| NounDecodeError::Custom("witness hax tail not a cell".into()))?;

        let hax_entries = HoonMapIter::from(cell.head())
            .filter(|entry| entry.is_cell())
            .map(|entry| {
                let [hash_raw, value_noun] = entry.uncell().map_err(|_| {
                    NounDecodeError::Custom("witness hax entry must be a pair".into())
                })?;
                let hash = Hash::from_noun(&hash_raw)?;
                let mut slab: NounSlab<NockJammer> = NounSlab::new();
                slab.copy_into(value_noun);
                let value = slab.jam();
                Ok(HaxPreimage { hash, value })
            })
            .collect::<Result<Vec<_>, NounDecodeError>>()?;

        Ok(Self {
            lock_merkle_proof,
            pkh_signature,
            hax: hax_entries,
            tim: 0,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HaxPreimage {
    pub hash: Hash,
    // Jammed Bytes
    pub value: bytes::Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkhSignature(pub Vec<PkhSignatureEntry>);

impl PkhSignature {
    pub fn new(entries: Vec<PkhSignatureEntry>) -> Self {
        Self(entries)
    }
}

impl NounEncode for PkhSignature {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        self.0.iter().fold(D(0), |acc, entry| {
            let mut key = entry.hash.to_noun(allocator);
            let mut value = entry.to_noun(allocator);
            zmap::z_map_put(allocator, &acc, &mut key, &mut value, &DefaultTipHasher)
                .expect("failed to encode pkh-signature map")
        })
    }
}

impl NounDecode for PkhSignature {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let entries = HoonMapIter::from(*noun)
            .filter(|entry| entry.is_cell())
            .map(|entry| {
                let [hash_raw, value_raw] = entry.uncell().map_err(|_| {
                    NounDecodeError::Custom("pkh-signature entry must be a pair".into())
                })?;
                let hash = Hash::from_noun(&hash_raw)?;
                PkhSignatureEntry::decode(hash, &value_raw)
            })
            .collect::<Result<Vec<_>, NounDecodeError>>()?;
        Ok(Self(entries))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkhSignatureEntry {
    pub hash: Hash,
    pub pubkey: SchnorrPubkey,
    pub signature: SchnorrSignature,
}

impl NounEncode for PkhSignatureEntry {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let pubkey = self.pubkey.to_noun(allocator);
        let signature = self.signature.to_noun(allocator);
        nockvm::noun::T(allocator, &[pubkey, signature])
    }
}

impl PkhSignatureEntry {
    fn decode(hash: Hash, noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell()?;
        let pubkey = SchnorrPubkey::from_noun(&cell.head())?;
        let signature = SchnorrSignature::from_noun(&cell.tail())?;
        Ok(Self {
            hash,
            pubkey,
            signature,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct LockMerkleProof {
    pub spend_condition: SpendCondition,
    pub axis: u64,
    pub proof: MerkleProof,
}

//impl NounEncode for LockMerkleProof {
//    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
//        let condition = self.spend_condition.to_noun(allocator);
//        let axis = self.axis.to_noun(allocator);
//        let proof = self.proof.to_noun(allocator);
//        nockvm::noun::T(allocator, &[condition, axis, proof])
//    }
//}

//impl NounDecode for LockMerkleProof {
//    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
//        let cell = noun.as_cell()?;
//        let spend_condition = SpendCondition::from_noun(&cell.head())?;
//
//        let tail = cell.tail();
//        let cell = tail
//            .as_cell()
//            .map_err(|_| NounDecodeError::Custom("lock-merkle-proof tail not a cell".into()))?;
//        let axis = u64::from_noun(&cell.head())?;
//        let proof = MerkleProof::from_noun(&cell.tail())?;
//
//        Ok(Self {
//            spend_condition,
//            axis,
//            proof,
//        })
//    }
//}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MerkleProof {
    pub root: Hash,
    pub path: Vec<Hash>,
}

impl NounEncode for MerkleProof {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let root = self.root.to_noun(allocator);
        let mut path_list = D(0);
        for hash in self.path.iter().rev() {
            let head = hash.to_noun(allocator);
            path_list = nockvm::noun::T(allocator, &[head, path_list]);
        }
        nockvm::noun::T(allocator, &[root, path_list])
    }
}

impl NounDecode for MerkleProof {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell()?;
        let root = Hash::from_noun(&cell.head())?;
        let path_iter = HoonList::try_from(cell.tail())
            .map_err(|_| NounDecodeError::Custom("merkle proof path must be a list".into()))?;

        let mut path = Vec::new();
        for entry in path_iter {
            path.push(Hash::from_noun(&entry)?);
        }

        Ok(Self { root, path })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendCondition(pub Vec<LockPrimitive>);

impl SpendCondition {
    pub fn new(primitives: Vec<LockPrimitive>) -> Self {
        Self(primitives)
    }

    pub fn iter(&self) -> impl Iterator<Item = &LockPrimitive> {
        self.0.iter()
    }
}

impl NounEncode for SpendCondition {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        self.0.iter().rev().fold(D(0), |acc, primitive| {
            let head = primitive.to_noun(allocator);
            nockvm::noun::T(allocator, &[head, acc])
        })
    }
}

impl NounDecode for SpendCondition {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let iter = HoonList::try_from(*noun)
            .map_err(|_| NounDecodeError::Custom("spend-condition must be a list".into()))?;

        let mut primitives = Vec::new();
        for entry in iter {
            primitives.push(LockPrimitive::from_noun(&entry)?);
        }

        Ok(Self(primitives))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockPrimitive {
    Pkh(Pkh),
    Tim(LockTim),
    Hax(Hax),
    Burn,
}

impl NounEncode for LockPrimitive {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        match self {
            LockPrimitive::Pkh(pkh) => {
                let tag = make_tas(allocator, "pkh").as_noun();
                let value = pkh.to_noun(allocator);
                nockvm::noun::T(allocator, &[tag, value])
            }
            LockPrimitive::Tim(tim) => {
                let tag = make_tas(allocator, "tim").as_noun();
                let value = tim.to_noun(allocator);
                nockvm::noun::T(allocator, &[tag, value])
            }
            LockPrimitive::Hax(hax) => {
                let tag = make_tas(allocator, "hax").as_noun();
                let value = hax.to_noun(allocator);
                nockvm::noun::T(allocator, &[tag, value])
            }
            LockPrimitive::Burn => {
                let tag = make_tas(allocator, "brn").as_noun();
                let value = D(0);
                nockvm::noun::T(allocator, &[tag, value])
            }
        }
    }
}

impl NounDecode for LockPrimitive {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell()?;
        let tag_atom = cell
            .head()
            .as_atom()
            .map_err(|_| NounDecodeError::Custom("lock-primitive tag must be an atom".into()))?;
        let tag = tag_atom
            .into_string()
            .map_err(|err| NounDecodeError::Custom(format!("invalid lock-primitive tag: {err}")))?;

        match tag.as_str() {
            "pkh" => Ok(LockPrimitive::Pkh(Pkh::from_noun(&cell.tail())?)),
            "tim" => Ok(LockPrimitive::Tim(LockTim::from_noun(&cell.tail())?)),
            "hax" => Ok(LockPrimitive::Hax(Hax::from_noun(&cell.tail())?)),
            "brn" => Ok(LockPrimitive::Burn),
            _ => Err(NounDecodeError::InvalidEnumVariant),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pkh {
    pub m: u64,
    // z-set of hashes
    pub hashes: Vec<Hash>,
}

impl NounEncode for Pkh {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let m = self.m.to_noun(allocator);
        let hashes = self.hashes.iter().fold(D(0), |acc, hash| {
            let mut value = hash.to_noun(allocator);
            zset::z_set_put(allocator, &acc, &mut value, &DefaultTipHasher)
                .expect("failed to encode pkh hash set")
        });
        nockvm::noun::T(allocator, &[m, hashes])
    }
}

impl NounDecode for Pkh {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell()?;
        let m = u64::from_noun(&cell.head())?;
        let hashes = decode_zset(&cell.tail(), Hash::from_noun)?;
        Ok(Self { m, hashes })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct LockTim {
    pub rel: TimelockRangeRelative,
    pub abs: TimelockRangeAbsolute,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct LockTimeBounds {
    pub min: Option<BlockHeight>,
    pub max: Option<BlockHeight>,
}

// Encode into a set of hashes
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hax(pub Vec<Hash>);

impl NounEncode for Hax {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        self.0.iter().fold(D(0), |acc, hash| {
            let mut value = hash.to_noun(allocator);
            zset::z_set_put(allocator, &acc, &mut value, &DefaultTipHasher)
                .expect("failed to encode hax set")
        })
    }
}

impl NounDecode for Hax {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        decode_zset(noun, Hash::from_noun).map(Self)
    }
}

fn decode_zset<T, F>(noun: &Noun, mut f: F) -> Result<Vec<T>, NounDecodeError>
where
    F: FnMut(&Noun) -> Result<T, NounDecodeError>,
{
    fn traverse<T, F>(node: &Noun, acc: &mut Vec<T>, f: &mut F) -> Result<(), NounDecodeError>
    where
        F: FnMut(&Noun) -> Result<T, NounDecodeError>,
    {
        if let Ok(atom) = node.as_atom() {
            if atom.as_u64()? == 0 {
                return Ok(());
            }
            return Err(NounDecodeError::ExpectedCell);
        }

        let cell = node
            .as_cell()
            .map_err(|_| NounDecodeError::Custom("z-set node must be a cell".into()))?;
        acc.push(f(&cell.head())?);

        let branches = cell
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::Custom("z-set branches must be a cell".into()))?;
        traverse(&branches.head(), acc, f)?;
        traverse(&branches.tail(), acc, f)?;
        Ok(())
    }

    let mut acc = Vec::new();
    traverse(noun, &mut acc, &mut f)?;
    Ok(acc)
}
