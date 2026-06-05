use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::noun::NounAllocatorExt;
use nockchain_math::belt::Belt;
use nockchain_math::noun_ext::NounMathExt;
use nockchain_math::structs::{HoonList, HoonMapIter};
use nockchain_math::zoon::common::DefaultTipHasher;
use nockchain_math::zoon::zmap::{self, ZMap};
use nockchain_math::zoon::zset::ZSet;
use nockvm::ext::{make_tas, AtomExt};
use nockvm::noun::{Noun, NounAllocator, D};
use noun_serde::{NounDecode, NounDecodeError, NounEncode};

use super::hashable::{
    hash_leaf_atom, hash_leaf_null, hash_pair, hash_unit_belt, Hashable, HashableEncodingError,
    HashableTreeHasher,
};
use super::note::NoteData;
use crate::tx_engine::common::{
    BlockHeight, BlockHeightDelta, FirstName, Hash, Name, Nicks, SchnorrPubkey, SchnorrSignature,
    Signature, Source, TxId, Version,
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
        ZMap::try_from_entries(self.0.clone())
            .expect("spends z-map should encode")
            .to_noun(allocator)
    }
}

impl NounDecode for Spends {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        Ok(Self(ZMap::<Name, Spend>::from_noun(noun)?.into_entries()))
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
        ZSet::try_from_items(self.0.clone())
            .expect("seed z-set should encode")
            .to_noun(allocator)
    }
}

impl NounDecode for Seeds {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        Ok(Self(ZSet::<Seed>::from_noun(noun)?.into_items()))
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
        let entries = self
            .0
            .iter()
            .cloned()
            .map(|entry| {
                (
                    entry.hash.clone(),
                    PkhSignatureValue {
                        pubkey: entry.pubkey,
                        signature: entry.signature,
                    },
                )
            })
            .collect::<Vec<_>>();
        ZMap::try_from_entries(entries)
            .expect("pkh-signature z-map should encode")
            .to_noun(allocator)
    }
}

impl NounDecode for PkhSignature {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let entries = ZMap::<Hash, PkhSignatureValue>::from_noun(noun)?.into_entries();
        Ok(Self(
            entries
                .into_iter()
                .map(|(hash, value)| PkhSignatureEntry {
                    hash,
                    pubkey: value.pubkey,
                    signature: value.signature,
                })
                .collect(),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
struct PkhSignatureValue {
    pubkey: SchnorrPubkey,
    signature: SchnorrSignature,
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

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct LockMerkleProofStub {
    pub spend_condition: SpendCondition,
    pub axis: u64,
    pub proof: MerkleProof,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct LockMerkleProofFull {
    pub version: u64,
    pub spend_condition: SpendCondition,
    pub axis: u64,
    pub proof: MerkleProof,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode)]
#[noun(untagged)]
pub enum LockMerkleProof {
    Full(LockMerkleProofFull),
    Stub(LockMerkleProofStub),
}

impl LockMerkleProof {
    pub fn new_full(spend_condition: SpendCondition, axis: u64, proof: MerkleProof) -> Self {
        use nockvm_macros::tas;
        Self::Full(LockMerkleProofFull {
            version: tas!(b"full"),
            spend_condition,
            axis,
            proof,
        })
    }

    pub fn new_stub(spend_condition: SpendCondition, axis: u64, proof: MerkleProof) -> Self {
        Self::Stub(LockMerkleProofStub {
            spend_condition,
            axis,
            proof,
        })
    }

    pub fn spend_condition(&self) -> &SpendCondition {
        match self {
            Self::Full(proof) => &proof.spend_condition,
            Self::Stub(proof) => &proof.spend_condition,
        }
    }

    pub fn axis(&self) -> u64 {
        match self {
            Self::Full(proof) => proof.axis,
            Self::Stub(proof) => proof.axis,
        }
    }

    pub fn proof(&self) -> &MerkleProof {
        match self {
            Self::Full(proof) => &proof.proof,
            Self::Stub(proof) => &proof.proof,
        }
    }

    pub fn into_parts(self) -> (SpendCondition, u64, MerkleProof) {
        match self {
            Self::Full(proof) => (proof.spend_condition, proof.axis, proof.proof),
            Self::Stub(proof) => (proof.spend_condition, proof.axis, proof.proof),
        }
    }
}

impl NounDecode for LockMerkleProof {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        if let Ok(full) = LockMerkleProofFull::from_noun(noun) {
            if full.version != nockvm_macros::tas!(b"full") {
                return Err(NounDecodeError::Custom(
                    "lock-merkle-proof version must be %full".into(),
                ));
            }
            return Ok(Self::Full(full));
        }
        Ok(Self::Stub(LockMerkleProofStub::from_noun(noun)?))
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NumericTag(u64);

impl NumericTag {
    fn into_inner(self) -> u64 {
        self.0
    }
}

impl NounDecode for NumericTag {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        if let Ok(value) = u64::from_noun(noun) {
            return Ok(Self(value));
        }

        let tag = String::from_noun(noun)?;
        let value = tag.parse::<u64>().map_err(|_| {
            NounDecodeError::Custom("lock tree tag must contain only digits".into())
        })?;
        Ok(Self(value))
    }
}

/// Binary lock tree with power-of-two fanout over spend conditions.
///
/// This mirrors `$lock` in `tx-engine-1.hoon`: a lock is either a single
/// spend-condition leaf, or a tagged `%2/%4/%8/%16` tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lock {
    SpendCondition(SpendCondition),
    V2(LockV2),
    V4(LockV4),
    V8(LockV8),
    V16(LockV16),
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct LockV2 {
    pub p: SpendCondition,
    pub q: SpendCondition,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct LockV4 {
    pub p: LockV2,
    pub q: LockV2,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct LockV8 {
    pub p: LockV4,
    pub q: LockV4,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct LockV16 {
    pub p: LockV8,
    pub q: LockV8,
}

impl LockV2 {
    fn flatten_spend_conditions(&self) -> Vec<SpendCondition> {
        vec![self.p.clone(), self.q.clone()]
    }
}

impl LockV4 {
    fn flatten_spend_conditions(&self) -> Vec<SpendCondition> {
        let mut out = self.p.flatten_spend_conditions();
        out.extend(self.q.flatten_spend_conditions());
        out
    }
}

impl LockV8 {
    fn flatten_spend_conditions(&self) -> Vec<SpendCondition> {
        let mut out = self.p.flatten_spend_conditions();
        out.extend(self.q.flatten_spend_conditions());
        out
    }
}

impl LockV16 {
    fn flatten_spend_conditions(&self) -> Vec<SpendCondition> {
        let mut out = self.p.flatten_spend_conditions();
        out.extend(self.q.flatten_spend_conditions());
        out
    }
}

impl Lock {
    /// Returns how many spend-condition leaves are present in this lock.
    pub fn spend_condition_count(&self) -> u64 {
        match self {
            Self::SpendCondition(_) => 1,
            Self::V2(_) => 2,
            Self::V4(_) => 4,
            Self::V8(_) => 8,
            Self::V16(_) => 16,
        }
    }

    /// Flattens the lock tree in left-to-right order.
    pub fn flatten_spend_conditions(&self) -> Vec<SpendCondition> {
        match self {
            Self::SpendCondition(spend_condition) => vec![spend_condition.clone()],
            Self::V2(v2) => v2.flatten_spend_conditions(),
            Self::V4(v4) => v4.flatten_spend_conditions(),
            Self::V8(v8) => v8.flatten_spend_conditions(),
            Self::V16(v16) => v16.flatten_spend_conditions(),
        }
    }

    /// Computes the consensus lock root by hashing this lock's handwritten hashable form.
    pub fn hash(&self) -> Result<Hash, LockHashError> {
        self.hash_digest()
    }
}

impl NounEncode for Lock {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        match self {
            Self::SpendCondition(spend_condition) => spend_condition.to_noun(allocator),
            Self::V2(v2) => {
                let value = v2.to_noun(allocator);
                nockvm::noun::T(allocator, &[D(2), value])
            }
            Self::V4(v4) => {
                let value = v4.to_noun(allocator);
                nockvm::noun::T(allocator, &[D(4), value])
            }
            Self::V8(v8) => {
                let value = v8.to_noun(allocator);
                nockvm::noun::T(allocator, &[D(8), value])
            }
            Self::V16(v16) => {
                let value = v16.to_noun(allocator);
                nockvm::noun::T(allocator, &[D(16), value])
            }
        }
    }
}

impl NounDecode for Lock {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        if let Ok(spend_condition) = SpendCondition::from_noun(noun) {
            return Ok(Self::SpendCondition(spend_condition));
        }

        let cell = noun.as_cell().map_err(|_| {
            NounDecodeError::Custom("lock must be spend-condition or lock tree".into())
        })?;
        let tag = NumericTag::from_noun(&cell.head())?.into_inner();
        match tag {
            2 => Ok(Self::V2(LockV2::from_noun(&cell.tail())?)),
            4 => Ok(Self::V4(LockV4::from_noun(&cell.tail())?)),
            8 => Ok(Self::V8(LockV8::from_noun(&cell.tail())?)),
            16 => Ok(Self::V16(LockV16::from_noun(&cell.tail())?)),
            _ => Err(NounDecodeError::Custom(format!(
                "unsupported lock tree tag: {tag}"
            ))),
        }
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
    pub hashes: ZSet<Hash>,
}

impl Pkh {
    pub fn new<I>(m: u64, hashes: I) -> Self
    where
        I: IntoIterator<Item = Hash>,
    {
        Self {
            m,
            hashes: ZSet::try_from_items(hashes).expect("pkh hash z-set should build"),
        }
    }
}

impl NounEncode for Pkh {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let m = self.m.to_noun(allocator);
        let hashes = self.hashes.to_noun(allocator);
        nockvm::noun::T(allocator, &[m, hashes])
    }
}

impl NounDecode for Pkh {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell()?;
        let m = u64::from_noun(&cell.head())?;
        let hashes = ZSet::<Hash>::from_noun(&cell.tail())?;
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
pub struct Hax(pub ZSet<Hash>);

impl Hax {
    pub fn new<I>(hashes: I) -> Self
    where
        I: IntoIterator<Item = Hash>,
    {
        Self(ZSet::try_from_items(hashes).expect("hax z-set should build"))
    }
}

impl NounEncode for Hax {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        self.0.to_noun(allocator)
    }
}

impl NounDecode for Hax {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        Ok(Self(ZSet::<Hash>::from_noun(noun)?))
    }
}

/// Errors raised while converting consensus values to hashable nouns.
#[derive(Debug, thiserror::Error)]
pub enum LockHashError {
    #[error(transparent)]
    HashableEncoding(#[from] HashableEncodingError),
}

#[derive(Debug, thiserror::Error)]
pub enum FirstNameFromLockRootError {
    #[error(transparent)]
    HashableEncoding(#[from] HashableEncodingError),
}

struct FirstNameDigestInput<'a> {
    lock_root: &'a Hash,
}

impl Hashable for FirstNameDigestInput<'_> {
    type Error = FirstNameFromLockRootError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let first_tag = hash_leaf_atom(0)?;
        Ok(hash_pair(&first_tag, self.lock_root))
    }
}

impl FirstName {
    /// Derives the v1 first-name digest from a lock-root hash.
    pub fn from_lock_root(lock_root: &Hash) -> Result<Self, FirstNameFromLockRootError> {
        Ok(Self(FirstNameDigestInput { lock_root }.hash_digest()?))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SpendConditionFirstNameError {
    #[error(transparent)]
    LockHash(#[from] LockHashError),
    #[error(transparent)]
    FirstNameFromLockRoot(#[from] FirstNameFromLockRootError),
}

impl SpendCondition {
    /// Builds a simple single-signer PKH spend-condition.
    pub fn simple_pkh(pkh: Hash) -> Self {
        Self::new(vec![LockPrimitive::Pkh(Pkh::new(1, vec![pkh]))])
    }

    /// Builds a coinbase-style single-signer PKH spend-condition with a relative timelock.
    pub fn coinbase_pkh(pkh: Hash, coinbase_relative_min: u64) -> Self {
        let lock_tim = LockTim {
            rel: TimelockRangeRelative::new(
                Some(BlockHeightDelta(Belt(coinbase_relative_min))),
                None,
            ),
            abs: TimelockRangeAbsolute::none(),
        };
        Self::new(vec![
            LockPrimitive::Pkh(Pkh::new(1, vec![pkh])),
            LockPrimitive::Tim(lock_tim),
        ])
    }

    /// Computes the consensus spend-condition hash.
    pub fn hash(&self) -> Result<Hash, LockHashError> {
        self.hash_digest()
    }

    /// Computes the v1 note first-name from this spend-condition.
    pub fn first_name(&self) -> Result<FirstName, SpendConditionFirstNameError> {
        let lock_root = Lock::SpendCondition(self.clone()).hash()?;
        Ok(FirstName::from_lock_root(&lock_root)?)
    }
}

impl Hashable for SpendCondition {
    type Error = LockHashError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let mut tail = hash_leaf_null();
        for primitive in self.0.iter().rev() {
            let head = primitive.hash_digest()?;
            tail = hash_pair(&head, &tail);
        }
        Ok(tail)
    }
}

impl Hashable for LockPrimitive {
    type Error = LockHashError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        match self {
            Self::Pkh(pkh) => {
                let tag = hash_leaf_atom(nockvm_macros::tas!(b"pkh"))?;
                let payload = pkh.hash_digest()?;
                Ok(hash_pair(&tag, &payload))
            }
            Self::Tim(tim) => {
                let tag = hash_leaf_atom(nockvm_macros::tas!(b"tim"))?;
                let payload = tim.hash_digest()?;
                Ok(hash_pair(&tag, &payload))
            }
            Self::Hax(hax) => {
                let tag = hash_leaf_atom(nockvm_macros::tas!(b"hax"))?;
                let payload = hax.hash_digest()?;
                Ok(hash_pair(&tag, &payload))
            }
            Self::Burn => {
                let burn_tag = hash_leaf_atom(nockvm_macros::tas!(b"brn"))?;
                let null_leaf = hash_leaf_null();
                Ok(hash_pair(&burn_tag, &null_leaf))
            }
        }
    }
}

impl Hashable for Pkh {
    type Error = LockHashError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let m_hashable = hash_leaf_atom(self.m)?;
        let hashes_hashable = self.hashes.hash_with(&HashableTreeHasher);
        Ok(hash_pair(&m_hashable, &hashes_hashable))
    }
}

impl Hashable for Hax {
    type Error = LockHashError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        Ok(self.0.hash_with(&HashableTreeHasher))
    }
}

impl Hashable for LockTim {
    type Error = LockHashError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let rel_min = hash_unit_belt(self.rel.min.as_ref().map(|height| height.0));
        let rel_max = hash_unit_belt(self.rel.max.as_ref().map(|height| height.0));
        let abs_min = hash_unit_belt(self.abs.min.as_ref().map(|height| height.0));
        let abs_max = hash_unit_belt(self.abs.max.as_ref().map(|height| height.0));
        let rel = hash_pair(&rel_min, &rel_max);
        let abs = hash_pair(&abs_min, &abs_max);
        Ok(hash_pair(&rel, &abs))
    }
}

impl Hashable for Lock {
    type Error = LockHashError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        match self {
            Self::SpendCondition(spend_condition) => spend_condition.hash_digest(),
            Self::V2(v2) => {
                let tag = hash_leaf_atom(2)?;
                let payload = v2.hash_digest()?;
                Ok(hash_pair(&tag, &payload))
            }
            Self::V4(v4) => {
                let tag = hash_leaf_atom(4)?;
                let payload = v4.hash_digest()?;
                Ok(hash_pair(&tag, &payload))
            }
            Self::V8(v8) => {
                let tag = hash_leaf_atom(8)?;
                let payload = v8.hash_digest()?;
                Ok(hash_pair(&tag, &payload))
            }
            Self::V16(v16) => {
                let tag = hash_leaf_atom(16)?;
                let payload = v16.hash_digest()?;
                Ok(hash_pair(&tag, &payload))
            }
        }
    }
}

impl Hashable for LockV2 {
    type Error = LockHashError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let p_hash = self.p.hash()?;
        let q_hash = self.q.hash()?;
        Ok(hash_pair(&p_hash, &q_hash))
    }
}

impl Hashable for LockV4 {
    type Error = LockHashError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let left = self.p.hash_digest()?;
        let right = self.q.hash_digest()?;
        Ok(hash_pair(&left, &right))
    }
}

impl Hashable for LockV8 {
    type Error = LockHashError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let left = self.p.hash_digest()?;
        let right = self.q.hash_digest()?;
        Ok(hash_pair(&left, &right))
    }
}

impl Hashable for LockV16 {
    type Error = LockHashError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let left = self.p.hash_digest()?;
        let right = self.q.hash_digest()?;
        Ok(hash_pair(&left, &right))
    }
}

#[cfg(test)]
mod tests {
    use nockapp::noun::slab::{NockJammer, NounSlab};
    use nockchain_math::belt::Belt;
    use noun_serde::{NounDecode, NounEncode};

    use super::{Hax, Lock, LockPrimitive, LockTim, LockV2, LockV4, Pkh, SpendCondition};
    use crate::tx_engine::common::{
        BlockHeight, BlockHeightDelta, Hash, TimelockRangeAbsolute, TimelockRangeRelative,
    };

    const ADDRESS_A_B58: &str = "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV";
    const ADDRESS_B_B58: &str = "9phXGACnW4238oqgvn2gpwaUjG3RAqcxq2Ash2vaKp8KjzSd3MQ56Jt";
    const EXPECTED_PKH_ROOT_B58: &str = "DKrgXqE8bXR1uBZ3t4vU13m2KquGCDbnn1PeoPL7dxSHTucGPFDPt53";
    const EXPECTED_MULTISIG_2_OF_2_ROOT_B58: &str =
        "4eMAT3BuhLPjYFronoYJ9RSLVSgveCL3nQB7RHSLZzjBTiYCxEzkzEH";
    const EXPECTED_TIM_ROOT_B58: &str = "66FLtgznHvE7v4Fi4wZ6aA9EzsPD6pfaL3qL85apJuiBF8unRKXVsor";
    const EXPECTED_HAX_ROOT_B58: &str = "4kwz3RMCacfRXY3ydNoQ1tsUKuzaBEzGSpX9GpSWf8T3Rj24Ucuj6v4";
    const EXPECTED_LOCK_V2_ROOT_B58: &str =
        "e3qeUqDf6ZTkayiiQDpKpax6RqXMBAMRLtrppvL41EdyJYFj743ZKB";
    const EXPECTED_LOCK_V4_ROOT_B58: &str =
        "6ezbUN1ozEvZi9TUGVN1pY2TcCJc5KWoCzjj519ihE6LGupvJpnysjo";
    const EXPECTED_MEGA_LOCK_V4_ROOT_B58: &str =
        "DaNZuUK5iHhkCiDt3UShiNbz79TLdoLyNs3dKjTX1yzEZ7tjwjzbe8U";
    const BRIDGE_ROOT_B58: &str = "AcsPkuhXQoGeEsF91yynpm1kcW17PQ2Z1MEozgx7YnDPkZwrtzLuuqd";

    fn pkh_condition(m: u64, hashes: Vec<Hash>) -> SpendCondition {
        SpendCondition::new(vec![pkh_primitive(m, hashes)])
    }

    fn pkh_primitive(m: u64, hashes: Vec<Hash>) -> LockPrimitive {
        LockPrimitive::Pkh(Pkh::new(m, hashes))
    }

    fn tim_condition() -> SpendCondition {
        SpendCondition::new(vec![tim_primitive(Some(3), Some(10), Some(20), None)])
    }

    fn tim_primitive(
        rel_min: Option<u64>,
        rel_max: Option<u64>,
        abs_min: Option<u64>,
        abs_max: Option<u64>,
    ) -> LockPrimitive {
        LockPrimitive::Tim(LockTim {
            rel: TimelockRangeRelative {
                min: rel_min.map(|value| BlockHeightDelta(Belt(value))),
                max: rel_max.map(|value| BlockHeightDelta(Belt(value))),
            },
            abs: TimelockRangeAbsolute {
                min: abs_min.map(|value| BlockHeight(Belt(value))),
                max: abs_max.map(|value| BlockHeight(Belt(value))),
            },
        })
    }

    fn hax_condition(hashes: Vec<Hash>) -> SpendCondition {
        SpendCondition::new(vec![hax_primitive(hashes)])
    }

    fn hax_primitive(hashes: Vec<Hash>) -> LockPrimitive {
        LockPrimitive::Hax(Hax::new(hashes))
    }

    #[test]
    fn lock_hash_matches_known_hoon_vectors() {
        let address_a = Hash::from_base58(ADDRESS_A_B58).expect("address a should parse");
        let address_b = Hash::from_base58(ADDRESS_B_B58).expect("address b should parse");
        let bridge_root = Hash::from_base58(BRIDGE_ROOT_B58).expect("bridge root should parse");

        let single_pkh_lock = Lock::SpendCondition(pkh_condition(1, vec![address_a.clone()]));
        let multisig_lock =
            Lock::SpendCondition(pkh_condition(2, vec![address_a.clone(), address_b.clone()]));
        let tim_lock = Lock::SpendCondition(tim_condition());
        let hax_lock =
            Lock::SpendCondition(hax_condition(vec![address_a.clone(), address_b.clone()]));
        let lock_v2 = Lock::V2(LockV2 {
            p: pkh_condition(1, vec![address_a.clone()]),
            q: tim_condition(),
        });
        let lock_v4 = Lock::V4(LockV4 {
            p: LockV2 {
                p: pkh_condition(1, vec![address_a.clone()]),
                q: tim_condition(),
            },
            q: LockV2 {
                p: hax_condition(vec![address_a.clone(), address_b.clone()]),
                q: SpendCondition::new(vec![LockPrimitive::Burn]),
            },
        });
        let mega_lock_v4 = Lock::V4(LockV4 {
            p: LockV2 {
                p: SpendCondition::new(vec![
                    pkh_primitive(2, vec![address_a.clone(), address_b.clone()]),
                    tim_primitive(Some(5), Some(15), Some(25), None),
                    hax_primitive(vec![address_a.clone(), bridge_root.clone()]),
                ]),
                q: SpendCondition::new(vec![
                    hax_primitive(vec![address_b.clone()]),
                    tim_primitive(None, Some(8), None, Some(40)),
                    pkh_primitive(1, vec![address_b.clone()]),
                ]),
            },
            q: LockV2 {
                p: SpendCondition::new(vec![
                    pkh_primitive(1, vec![address_a.clone()]),
                    tim_primitive(Some(2), None, Some(30), Some(60)),
                    hax_primitive(vec![address_a.clone(), address_b.clone()]),
                ]),
                q: SpendCondition::new(vec![
                    tim_primitive(Some(1), Some(4), Some(50), Some(90)),
                    hax_primitive(vec![address_a.clone()]),
                    pkh_primitive(1, vec![address_a.clone(), address_b.clone()]),
                ]),
            },
        });

        assert_eq!(
            single_pkh_lock
                .hash()
                .expect("single pkh lock hash should compute")
                .to_base58(),
            EXPECTED_PKH_ROOT_B58
        );
        assert_eq!(
            multisig_lock
                .hash()
                .expect("multisig lock hash should compute")
                .to_base58(),
            EXPECTED_MULTISIG_2_OF_2_ROOT_B58
        );
        assert_eq!(
            tim_lock
                .hash()
                .expect("tim lock hash should compute")
                .to_base58(),
            EXPECTED_TIM_ROOT_B58
        );
        assert_eq!(
            hax_lock
                .hash()
                .expect("hax lock hash should compute")
                .to_base58(),
            EXPECTED_HAX_ROOT_B58
        );
        assert_eq!(
            lock_v2
                .hash()
                .expect("v2 lock hash should compute")
                .to_base58(),
            EXPECTED_LOCK_V2_ROOT_B58
        );
        assert_eq!(
            lock_v4
                .hash()
                .expect("v4 lock hash should compute")
                .to_base58(),
            EXPECTED_LOCK_V4_ROOT_B58
        );
        assert_eq!(
            mega_lock_v4
                .hash()
                .expect("mega v4 lock hash should compute")
                .to_base58(),
            EXPECTED_MEGA_LOCK_V4_ROOT_B58
        );
    }

    #[test]
    fn lock_tree_roundtrip_preserves_leaf_count() {
        fn pkh_with_value(value: u64) -> SpendCondition {
            pkh_condition(1, vec![Hash::from_limbs(&[value, 0, 0, 0, 0])])
        }

        let lock = Lock::V4(LockV4 {
            p: LockV2 {
                p: pkh_with_value(11),
                q: pkh_with_value(12),
            },
            q: LockV2 {
                p: pkh_with_value(13),
                q: pkh_with_value(14),
            },
        });

        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = lock.to_noun(&mut slab);
        let decoded = Lock::from_noun(&noun).expect("lock should decode");
        assert_eq!(decoded.spend_condition_count(), 4);
        assert_eq!(decoded.flatten_spend_conditions().len(), 4);
    }
}
