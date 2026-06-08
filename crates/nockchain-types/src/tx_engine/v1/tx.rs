use nockapp::noun::slab::{NockJammer, NounSlab};
use nockchain_math::belt::Belt;
use nockchain_math::noun_ext::NounMathExtHandle;
use nockchain_math::owned_based_noun::{owned_based_noun_decode_error, OwnedBasedNoun};
use nockchain_math::structs::{collect_zmap_entries_strict, HoonList};
use nockchain_math::zoon::common::DefaultTipHasher;
use nockchain_math::zoon::zmap::{self, ZMap};
use nockchain_math::zoon::zset::ZSet;
use nockvm::ext::make_tas;
use nockvm::noun::{Noun, NounAllocator, NounSpace, D};
use noun_serde::{NounDecode, NounDecodeError, NounEncode};

use super::hashable::{
    hash_hashable, hash_leaf_atom, hash_leaf_digest, hash_leaf_null, hash_pair, hash_unit_belt,
    noun_hashable, HashHashable, HashableEncodingError, HashableTreeHasher,
};
use super::note::{NoteData, NoteDataValue};
use crate::tx_engine::common::{
    BlockHeight, BlockHeightDelta, FirstName, Hash, Name, Nicks, SchnorrPubkey, SchnorrSignature,
    Signature, Source, TxId, Version,
};
use crate::v0::{TimelockRangeAbsolute, TimelockRangeRelative};
use crate::EthAddress;

#[derive(Debug, Clone, PartialEq)]
pub struct RawTx {
    pub version: Version,
    pub id: TxId,
    pub spends: Spends,
}

#[derive(Debug, thiserror::Error)]
pub enum RawTxIdError {
    #[error("raw tx id hashing only supports v1 transactions, got {0:?}")]
    UnsupportedVersion(Version),
    #[error("failed to encode hashable structure: {0}")]
    Encode(String),
    #[error("failed to hash raw tx id: {0}")]
    Hash(String),
    #[error("failed to decode raw tx id digest: {0}")]
    Decode(String),
    #[error("failed to hash spend-condition: {0}")]
    LockHash(#[from] LockHashError),
    #[error("failed to hash witness pubkey: {0}")]
    PubkeyHash(String),
}

// Fixed placeholder hash used by the legacy `LockMerkleProof::Stub` hashable form.
// Stub proofs do not commit to `axis`, so this sentinel preserves the old
// deterministic digest shape while keeping it distinct from the `%full` path.
const LOCK_MERKLE_PROOF_STUB_SENTINEL_B58: &str =
    "6mhCSwJQDvbkbiPAUNjetJtVoo1VLtEhmEYoU4hmdGd6ep1F6ayaV4A";

impl NounEncode for RawTx {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let version = self.version.to_noun(allocator);
        let id = self.id.to_noun(allocator);
        let spends = self.spends.to_noun(allocator);
        nockvm::noun::T(allocator, &[version, id, spends])
    }
}

impl NounDecode for RawTx {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let cell = noun.in_space(space).as_cell()?;
        let version = Version::from_noun(&cell.head().noun(), space)?;

        let tail = cell.tail();
        let cell = tail
            .as_cell()
            .map_err(|_| NounDecodeError::Custom("raw-tx tail not a cell".into()))?;
        let id = TxId::from_noun(&cell.head().noun(), space)?;

        let spends = Spends::from_noun(&cell.tail().noun(), space)?;

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

impl RawTx {
    pub fn compute_id(&self) -> Result<TxId, RawTxIdError> {
        self.hash_digest()
    }

    pub fn compute_id_base58(&self) -> Result<String, RawTxIdError> {
        Ok(self.compute_id()?.to_base58())
    }
}

impl HashHashable for RawTx {
    type Error = RawTxIdError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        // TODO(raw-tx-fixture): the Hoon-backed raw-tx parity fixture currently covers a
        // witness spend with opaque note-data, but not legacy spends, mixed spend maps, or
        // typed note-data (`%lock`, `%bridge`, `%bridge-w`). Keep this direct hashing path in
        // sync with Hoon and expand fixture coverage before treating it as exhaustive.
        if self.version != Version::V1 {
            return Err(RawTxIdError::UnsupportedVersion(self.version.clone()));
        }

        let version = hashable_leaf_value_digest(&1_u64)?;
        let spends = self.spends.hash_digest()?;
        Ok(hash_pair(&version, &spends))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureMap(pub Vec<(Name, Signature)>);

impl NounEncode for SignatureMap {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        zmap::encode_entries(allocator, &self.0, &DefaultTipHasher)
            .unwrap_or_else(|_| panic!("failed to encode signature map"))
    }
}

impl NounDecode for SignatureMap {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        zmap::decode_entries(noun, space, "signature").map(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendConditionMap(pub Vec<(Name, SpendCondition)>);

impl NounEncode for SpendConditionMap {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        zmap::encode_entries(allocator, &self.0, &DefaultTipHasher)
            .unwrap_or_else(|_| panic!("failed to encode spend-condition map"))
    }
}

impl NounDecode for SpendConditionMap {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        zmap::decode_entries(noun, space, "spend-condition").map(Self)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WitnessMap(pub Vec<(Name, Witness)>);

impl NounEncode for WitnessMap {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        zmap::encode_entries(allocator, &self.0, &DefaultTipHasher)
            .unwrap_or_else(|_| panic!("failed to encode witness map"))
    }
}

impl NounDecode for WitnessMap {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        zmap::decode_entries(noun, space, "witness").map(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub enum InputMetadata {
    #[noun(tag = 0)]
    LegacySignatures(SignatureMap),
    #[noun(tag = 1)]
    SpendConditions(SpendConditionMap),
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct LegacyLockMetadata {
    pub lock: Lock,
    pub include_data: bool,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub enum VersionedLockMetadata {
    #[noun(tag = "lock")]
    Lock { lock: Lock, include_data: bool },
    #[noun(tag = "lock-root")]
    LockRoot(Hash),
    #[noun(tag = "bridge-deposit")]
    BridgeDeposit { root: Hash, addr: EthAddress },
    #[noun(tag = "bridge-withdrawal")]
    BridgeWithdrawal {
        root: Hash,
        beid: Vec<Belt>,
        base_hash: Hash,
        base_batch_end: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub enum LockMetadata {
    #[noun(untagged)]
    Legacy(LegacyLockMetadata),
    #[noun(tag = 1)]
    Versioned(VersionedLockMetadata),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputLockMap(pub Vec<(Hash, LockMetadata)>);

impl NounEncode for OutputLockMap {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        zmap::encode_entries(allocator, &self.0, &DefaultTipHasher)
            .unwrap_or_else(|_| panic!("failed to encode output-lock map"))
    }
}

impl NounDecode for OutputLockMap {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        zmap::decode_entries(noun, space, "output-lock").map(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct TransactionMetadata {
    pub inputs: InputMetadata,
    pub outputs: OutputLockMap,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub enum WitnessData {
    #[noun(tag = 0)]
    Signatures(SignatureMap),
    #[noun(tag = 1)]
    Witnesses(WitnessMap),
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub struct TransactionV1 {
    pub name: String,
    pub spends: Spends,
    pub metadata: TransactionMetadata,
    pub witness_data: WitnessData,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub enum Transaction {
    #[noun(tag = 1)]
    V1(TransactionV1),
}

fn normalized_name_sort_key(name: &Name) -> ([u8; 40], [u8; 40]) {
    (name.first.to_be_limb_bytes(), name.last.to_be_limb_bytes())
}

fn normalized_note_names(inputs: &[Name]) -> Vec<Name> {
    let mut normalized = inputs.to_vec();
    normalized.sort_by_key(normalized_name_sort_key);
    normalized.dedup_by(|left, right| left == right);
    normalized
}

impl Transaction {
    pub fn normalized_input_names(&self) -> Vec<Name> {
        match self {
            Transaction::V1(tx) => normalized_note_names(
                &tx.spends
                    .0
                    .iter()
                    .map(|(name, _)| name.clone())
                    .collect::<Vec<_>>(),
            ),
        }
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
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        Ok(Self(
            ZMap::<Name, Spend>::from_noun(noun, space)?.into_entries(),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub enum Spend {
    #[noun(tag = 0)]
    Legacy(Spend0),
    #[noun(tag = 1)]
    Witness(Spend1),
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
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        Ok(Self(ZSet::<Seed>::from_noun(noun, space)?.into_items()))
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
            let mut value_noun = entry.value.to_noun(allocator);
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
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let cell = noun.in_space(space).as_cell()?;
        let lock_merkle_proof = LockMerkleProof::from_noun(&cell.head().noun(), space)?;

        let tail = cell.tail();
        let cell = tail
            .as_cell()
            .map_err(|_| NounDecodeError::Custom("witness tail not a cell".into()))?;
        let pkh_signature = PkhSignature::from_noun(&cell.head().noun(), space)?;

        let tail = cell.tail();
        let cell = tail
            .as_cell()
            .map_err(|_| NounDecodeError::Custom("witness hax tail not a cell".into()))?;

        let hax_map = cell.head();
        let hax_entries = collect_zmap_entries_strict(&hax_map)
            .map_err(|_| NounDecodeError::Custom("malformed witness hax z-map node".into()))?
            .into_iter()
            .map(|entry| {
                let [hash_raw, value_noun] = entry.uncell().map_err(|_| {
                    NounDecodeError::Custom("witness hax entry must be a pair".into())
                })?;
                let hash = Hash::from_noun(&hash_raw.noun(), space)?;
                let value = OwnedBasedNoun::from_noun(value_noun.noun(), space)
                    .map_err(owned_based_noun_decode_error)?;
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
    pub value: OwnedBasedNoun,
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
                    entry.pkh.clone(),
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
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let entries = ZMap::<Hash, PkhSignatureValue>::from_noun(noun, space)?.into_entries();
        Ok(Self(
            entries
                .into_iter()
                .map(|(hash, value)| PkhSignatureEntry {
                    pkh: hash,
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
    pub pkh: Hash,
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
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        if let Ok(full) = LockMerkleProofFull::from_noun(noun, space) {
            if full.version != nockvm_macros::tas!(b"full") {
                return Err(NounDecodeError::Custom(
                    "lock-merkle-proof version must be %full".into(),
                ));
            }
            return Ok(Self::Full(full));
        }
        Ok(Self::Stub(LockMerkleProofStub::from_noun(noun, space)?))
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
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let cell = noun.in_space(space).as_cell()?;
        let root = Hash::from_noun(&cell.head().noun(), space)?;
        let path_iter = HoonList::try_from(cell.tail().noun(), space)
            .map_err(|_| NounDecodeError::Custom("merkle proof path must be a list".into()))?;

        let mut path = Vec::new();
        for entry in path_iter {
            path.push(Hash::from_noun(&entry, space)?);
        }

        Ok(Self { root, path })
    }
}

/// Binary lock tree with power-of-two fanout over spend conditions.
///
/// This mirrors `$lock` in `tx-engine-1.hoon`: a lock is either a single
/// spend-condition leaf, or a tagged `%2/%4/%8/%16` tree.
#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub enum Lock {
    #[noun(untagged)]
    SpendCondition(SpendCondition),
    #[noun(tag = 2)]
    V2(LockV2),
    #[noun(tag = 4)]
    V4(LockV4),
    #[noun(tag = 8)]
    V8(LockV8),
    #[noun(tag = 16)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendCondition(pub Vec<LockPrimitive>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredPkhPolicy {
    pub threshold: usize,
    pub hashes: Vec<Hash>,
}

impl RequiredPkhPolicy {
    pub fn contains(&self, hash: &Hash) -> bool {
        self.hashes.iter().any(|candidate| candidate == hash)
    }
}

impl SpendCondition {
    pub fn new(primitives: Vec<LockPrimitive>) -> Self {
        Self(primitives)
    }

    pub fn iter(&self) -> impl Iterator<Item = &LockPrimitive> {
        self.0.iter()
    }

    /// Extracts the PKH threshold rule from a spend condition, if the input is
    /// controlled by a PKH multisig lock.
    pub fn required_pkh_policy(&self) -> Option<RequiredPkhPolicy> {
        for primitive in self.iter() {
            if let LockPrimitive::Pkh(pkh) = primitive {
                let threshold = usize::try_from(pkh.m).ok()?;
                let hashes = pkh.hashes.clone().into_items();
                return Some(RequiredPkhPolicy { threshold, hashes });
            }
        }
        None
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
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let iter = HoonList::try_from(*noun, space)
            .map_err(|_| NounDecodeError::Custom("spend-condition must be a list".into()))?;

        let mut primitives = Vec::new();
        for entry in iter {
            primitives.push(LockPrimitive::from_noun(&entry, space)?);
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
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let cell = noun.in_space(space).as_cell()?;
        let tag_atom = cell
            .head()
            .as_atom()
            .map_err(|_| NounDecodeError::Custom("lock-primitive tag must be an atom".into()))?;
        let tag = tag_atom
            .into_string()
            .map_err(|err| NounDecodeError::Custom(format!("invalid lock-primitive tag: {err}")))?;

        match tag.as_str() {
            "pkh" => Ok(LockPrimitive::Pkh(Pkh::from_noun(
                &cell.tail().noun(),
                space,
            )?)),
            "tim" => Ok(LockPrimitive::Tim(LockTim::from_noun(
                &cell.tail().noun(),
                space,
            )?)),
            "hax" => Ok(LockPrimitive::Hax(Hax::from_noun(
                &cell.tail().noun(),
                space,
            )?)),
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
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let cell = noun.in_space(space).as_cell()?;
        let m = u64::from_noun(&cell.head().noun(), space)?;
        let hashes = ZSet::<Hash>::from_noun(&cell.tail().noun(), space)?;
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
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        Ok(Self(ZSet::<Hash>::from_noun(noun, space)?))
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

impl HashHashable for FirstNameDigestInput<'_> {
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

impl HashHashable for SpendCondition {
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

impl HashHashable for LockPrimitive {
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

impl HashHashable for Pkh {
    type Error = LockHashError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let m_hashable = hash_leaf_atom(self.m)?;
        let hashes_hashable = self.hashes.hash_with(&HashableTreeHasher);
        Ok(hash_pair(&m_hashable, &hashes_hashable))
    }
}

impl HashHashable for Hax {
    type Error = LockHashError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        Ok(self.0.hash_with(&HashableTreeHasher))
    }
}

impl HashHashable for LockTim {
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

impl HashHashable for Lock {
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

impl HashHashable for LockV2 {
    type Error = LockHashError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let p_hash = self.p.hash()?;
        let q_hash = self.q.hash()?;
        Ok(hash_pair(&p_hash, &q_hash))
    }
}

impl HashHashable for LockV4 {
    type Error = LockHashError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let left = self.p.hash_digest()?;
        let right = self.q.hash_digest()?;
        Ok(hash_pair(&left, &right))
    }
}

impl HashHashable for LockV8 {
    type Error = LockHashError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let left = self.p.hash_digest()?;
        let right = self.q.hash_digest()?;
        Ok(hash_pair(&left, &right))
    }
}

impl HashHashable for LockV16 {
    type Error = LockHashError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let left = self.p.hash_digest()?;
        let right = self.q.hash_digest()?;
        Ok(hash_pair(&left, &right))
    }
}

fn hash_hashable_digest<A: NounAllocator>(
    allocator: &mut A,
    hashable: Noun,
) -> Result<Hash, RawTxIdError> {
    let digest =
        hash_hashable(allocator, hashable).map_err(|err| RawTxIdError::Hash(format!("{err:?}")))?;
    let space = allocator.noun_space();
    Hash::from_noun(&digest, &space).map_err(|err| RawTxIdError::Decode(err.to_string()))
}

fn hashable_leaf_value_digest<T: NounEncode>(value: &T) -> Result<Hash, RawTxIdError> {
    let mut slab = NounSlab::<NockJammer>::new();
    let noun = value.to_noun(&mut slab);
    hash_leaf_digest(&mut slab, noun).map_err(|err| RawTxIdError::Hash(format!("{err:?}")))
}

fn hash_hashable_tuple(items: &[Hash]) -> Hash {
    let mut iter = items.iter().rev();
    let Some(last) = iter.next() else {
        return hash_leaf_null();
    };
    let mut digest = last.clone();
    for item in iter {
        digest = hash_pair(item, &digest);
    }
    digest
}

impl HashHashable for Name {
    type Error = RawTxIdError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        Ok(hash_hashable_tuple(&[
            self.first.clone(),
            self.last.clone(),
            hash_leaf_null(),
        ]))
    }
}

impl HashHashable for Signature {
    type Error = RawTxIdError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let signature = ZMap::try_from_entries(self.0.clone())
            .map_err(|err| RawTxIdError::Encode(format!("signature z-map: {err}")))?;
        signature.try_fold_tree(
            || Ok(hash_leaf_null()),
            |pubkey, signature, left, right| {
                let pubkey_hash = pubkey
                    .pkh_hash()
                    .map_err(|err| RawTxIdError::PubkeyHash(err.to_string()))?;
                let signature = hashable_leaf_value_digest(signature)?;
                let entry = hash_pair(&pubkey_hash, &signature);
                Ok(hash_hashable_tuple(&[entry, left, right]))
            },
        )
    }
}

impl HashHashable for NoteData {
    type Error = RawTxIdError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let note_data = ZMap::try_from_entries(
            self.0
                .iter()
                .map(|entry| (NoteDataKey(entry.key.clone()), entry.value.clone())),
        )
        .map_err(|err| RawTxIdError::Encode(format!("note-data z-map: {err}")))?;
        note_data.try_fold_tree(
            || Ok(hash_leaf_null()),
            |key, value, left, right| {
                let key = hashable_leaf_value_digest(key)?;
                let value = value.hash_digest()?;
                let entry = hash_pair(&key, &value);
                Ok(hash_hashable_tuple(&[entry, left, right]))
            },
        )
    }
}

impl HashHashable for NoteDataValue {
    type Error = RawTxIdError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        Ok(match self {
            Self::Noun(noun) => Hash::from_limbs(&noun.hashable_noun_digest()),
            _ => {
                let mut slab = NounSlab::<NockJammer>::new();
                let noun = self.to_noun(&mut slab);
                let hashable = noun_hashable(&mut slab, noun);
                hash_hashable_digest(&mut slab, hashable)?
            }
        })
    }
}

impl HashHashable for Seed {
    type Error = RawTxIdError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let note_data = self.note_data.hash_digest()?;
        let gift = hashable_leaf_value_digest(&self.gift)?;
        Ok(hash_hashable_tuple(&[
            self.lock_root.clone(),
            note_data,
            gift,
            self.parent_hash.clone(),
        ]))
    }
}

impl HashHashable for Seeds {
    type Error = RawTxIdError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let seeds = ZSet::try_from_items(self.0.clone())
            .map_err(|err| RawTxIdError::Encode(format!("seeds z-set: {err}")))?;
        seeds.try_fold_tree(
            || Ok(hash_leaf_null()),
            |seed, left, right| {
                let seed = seed.hash_digest()?;
                Ok(hash_hashable_tuple(&[seed, left, right]))
            },
        )
    }
}

impl HashHashable for MerkleProof {
    type Error = RawTxIdError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let mut tail = hash_leaf_null();
        for hash in self.path.iter().rev() {
            tail = hash_pair(hash, &tail);
        }
        Ok(hash_pair(&self.root, &tail))
    }
}

impl HashHashable for LockMerkleProof {
    type Error = RawTxIdError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let spend_condition_hash = self.spend_condition().hash()?;
        let merkle = self.proof().hash_digest()?;
        let stub_sentinel = Hash::from_base58(LOCK_MERKLE_PROOF_STUB_SENTINEL_B58)
            .expect("lock-merkle-proof stub sentinel must parse");
        Ok(match self {
            LockMerkleProof::Full(full) => {
                let version = hashable_leaf_value_digest(&full.version)?;
                let axis = hashable_leaf_value_digest(&full.axis)?;
                hash_hashable_tuple(&[version, spend_condition_hash, axis, merkle])
            }
            LockMerkleProof::Stub(_) => {
                let head = hash_pair(&spend_condition_hash, &stub_sentinel);
                hash_pair(&head, &merkle)
            }
        })
    }
}

impl HashHashable for PkhSignature {
    type Error = RawTxIdError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let entries = ZMap::try_from_entries(self.0.iter().cloned().map(|entry| {
            (
                entry.pkh,
                PkhSignatureValue {
                    pubkey: entry.pubkey,
                    signature: entry.signature,
                },
            )
        }))
        .map_err(|err| RawTxIdError::Encode(format!("pkh-signature z-map: {err}")))?;
        entries.try_fold_tree(
            || Ok(hash_leaf_null()),
            |pkh, value, left, right| {
                let pubkey = value
                    .pubkey
                    .pkh_hash()
                    .map_err(|err| RawTxIdError::PubkeyHash(err.to_string()))?;
                let signature = hashable_leaf_value_digest(&value.signature)?;
                let value = hash_pair(&pubkey, &signature);
                let entry = hash_pair(pkh, &value);
                Ok(hash_hashable_tuple(&[entry, left, right]))
            },
        )
    }
}

impl HashHashable for [HaxPreimage] {
    type Error = RawTxIdError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let hax =
            ZMap::try_from_entries(self.iter().cloned().map(|entry| (entry.hash, entry.value)))
                .map_err(|err| RawTxIdError::Encode(format!("witness hax z-map: {err}")))?;
        hax.try_fold_tree(
            || Ok(hash_leaf_null()),
            |hash, value, left, right| {
                let value = Hash::from_limbs(&value.hashable_noun_digest());
                let entry = hash_pair(hash, &value);
                Ok(hash_hashable_tuple(&[entry, left, right]))
            },
        )
    }
}

impl HashHashable for Witness {
    type Error = RawTxIdError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let lmp = self.lock_merkle_proof.hash_digest()?;
        let pkh = self.pkh_signature.hash_digest()?;
        let hax = self.hax.as_slice().hash_digest()?;
        let tim = hashable_leaf_value_digest(&self.tim)?;
        Ok(hash_hashable_tuple(&[lmp, pkh, hax, tim]))
    }
}

impl HashHashable for Spend0 {
    type Error = RawTxIdError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let signature = self.signature.hash_digest()?;
        let seeds = self.seeds.hash_digest()?;
        let fee = hashable_leaf_value_digest(&self.fee)?;
        Ok(hash_hashable_tuple(&[signature, seeds, fee]))
    }
}

impl HashHashable for Spend1 {
    type Error = RawTxIdError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let witness = self.witness.hash_digest()?;
        let seeds = self.seeds.hash_digest()?;
        let fee = hashable_leaf_value_digest(&self.fee)?;
        Ok(hash_hashable_tuple(&[witness, seeds, fee]))
    }
}

impl HashHashable for Spend {
    type Error = RawTxIdError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        match self {
            Spend::Legacy(spend) => {
                let tag = hashable_leaf_value_digest(&0_u64)?;
                let payload = spend.hash_digest()?;
                Ok(hash_pair(&tag, &payload))
            }
            Spend::Witness(spend) => {
                let tag = hashable_leaf_value_digest(&1_u64)?;
                let payload = spend.hash_digest()?;
                Ok(hash_pair(&tag, &payload))
            }
        }
    }
}

impl HashHashable for Spends {
    type Error = RawTxIdError;

    fn hash_digest(&self) -> Result<Hash, Self::Error> {
        let spends = ZMap::try_from_entries(self.0.clone())
            .map_err(|err| RawTxIdError::Encode(format!("spends z-map: {err}")))?;
        spends.try_fold_tree(
            || Ok(hash_leaf_null()),
            |name, spend, left, right| {
                let name = name.hash_digest()?;
                let spend = spend.hash_digest()?;
                let entry = hash_pair(&name, &spend);
                Ok(hash_hashable_tuple(&[entry, left, right]))
            },
        )
    }
}

#[derive(Debug, Clone)]
struct NoteDataKey(String);

impl NounEncode for NoteDataKey {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        make_tas(allocator, &self.0).as_noun()
    }
}

#[cfg(test)]
mod tests {
    use nockapp::noun::slab::{NockJammer, NounSlab};
    use nockchain_math::belt::{Belt, PRIME};
    use nockchain_math::zoon::common::DefaultTipHasher;
    use nockchain_math::zoon::zmap;
    use nockvm::noun::{NounAllocator, D};
    use noun_serde::{NounDecode, NounEncode};

    use super::{
        HashHashable, Hax, InputMetadata, LegacyLockMetadata, Lock, LockMerkleProof, LockMetadata,
        LockPrimitive, LockTim, LockV2, LockV4, MerkleProof, Pkh, RawTx, Spend, Spend0,
        SpendCondition, SpendConditionMap, Transaction, Version, VersionedLockMetadata, Witness,
        WitnessData, WitnessMap,
    };
    use crate::tx_engine::common::{
        BlockHeight, BlockHeightDelta, Hash, Name, Nicks, Signature, TimelockRangeAbsolute,
        TimelockRangeRelative,
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

    fn atom_tag_value(noun: &nockvm::noun::Noun, space: &nockvm::noun::NounSpace) -> u64 {
        noun.in_space(space)
            .as_atom()
            .expect("tag must be an atom")
            .as_u64()
            .expect("tag should fit in u64")
    }

    fn sample_transaction_fixture() -> Transaction {
        const TRANSACTION_JAM: &[u8] = include_bytes!(
            "../../../../bridge/test-fixtures/transactions/9MpGym52AumtwyBxYPyVsWHvcamUYwZkc1Nq7w3cFGF28u8ceVDwt3e.tx"
        );

        let mut slab = NounSlab::<NockJammer>::new();
        let noun = slab
            .cue_into(TRANSACTION_JAM.to_vec().into())
            .expect("fixture transaction should cue");
        let space = slab.noun_space();

        Transaction::from_noun(&noun, &space).expect("fixture transaction should decode")
    }

    fn raw_tx_from_transaction_fixture(transaction: &Transaction) -> RawTx {
        let Transaction::V1(tx) = transaction;
        let spends = match &tx.witness_data {
            WitnessData::Signatures(signature_map) => tx
                .spends
                .0
                .iter()
                .map(|(name, spend)| {
                    let (_, signature) = signature_map
                        .0
                        .iter()
                        .find(|(signature_name, _)| signature_name == name)
                        .expect("fixture must have matching legacy signature");
                    let Spend::Legacy(legacy_spend) = spend else {
                        panic!("legacy witness data requires legacy spends");
                    };
                    (
                        name.clone(),
                        Spend::Legacy(Spend0 {
                            signature: signature.clone(),
                            seeds: legacy_spend.seeds.clone(),
                            fee: legacy_spend.fee.clone(),
                        }),
                    )
                })
                .collect(),
            WitnessData::Witnesses(witness_map) => tx
                .spends
                .0
                .iter()
                .map(|(name, spend)| {
                    let (_, witness) = witness_map
                        .0
                        .iter()
                        .find(|(witness_name, _)| witness_name == name)
                        .expect("fixture must have matching witness");
                    let Spend::Witness(witness_spend) = spend else {
                        panic!("witness data requires witness spends");
                    };
                    (
                        name.clone(),
                        Spend::Witness(super::Spend1 {
                            witness: witness.clone(),
                            seeds: witness_spend.seeds.clone(),
                            fee: witness_spend.fee.clone(),
                        }),
                    )
                })
                .collect(),
        };
        let mut raw_tx = RawTx {
            version: Version::V1,
            id: Hash::from_limbs(&[0, 0, 0, 0, 0]),
            spends: super::Spends(spends),
        };
        raw_tx.id = raw_tx
            .compute_id()
            .expect("fixture raw tx id should compute");
        raw_tx
    }

    #[test]
    fn derived_integer_tagged_unions_roundtrip() {
        let address_a = Hash::from_base58(ADDRESS_A_B58).expect("address a should parse");
        let sample_name = Name::new(address_a.clone(), address_a.clone());
        let sample_lock = Lock::SpendCondition(pkh_condition(1, vec![address_a.clone()]));

        let input_metadata = InputMetadata::SpendConditions(SpendConditionMap(vec![(
            sample_name.clone(),
            pkh_condition(1, vec![address_a.clone()]),
        )]));
        let witness_data = WitnessData::Witnesses(WitnessMap(Vec::new()));
        let spend = Spend::Legacy(Spend0 {
            signature: Signature(Vec::new()),
            seeds: super::Seeds(Vec::new()),
            fee: Nicks(0),
        });
        let legacy_lock_metadata = LockMetadata::Legacy(LegacyLockMetadata {
            lock: sample_lock.clone(),
            include_data: true,
        });
        let versioned_lock_metadata =
            LockMetadata::Versioned(VersionedLockMetadata::LockRoot(address_a.clone()));
        let bridge_withdrawal_lock_metadata =
            LockMetadata::Versioned(VersionedLockMetadata::BridgeWithdrawal {
                root: address_a.clone(),
                beid: (1..=32).map(Belt).collect(),
                base_hash: Hash::from_limbs(&[0x11, 0x22, 0x33, 0x44, 0x55]),
                base_batch_end: 57_600,
            });

        let mut slab = NounSlab::<NockJammer>::new();

        let input_noun = input_metadata.to_noun(&mut slab);
        let space = slab.noun_space();
        let input_cell = input_noun
            .in_space(&space)
            .as_cell()
            .expect("input metadata must be tagged");
        assert_eq!(atom_tag_value(&input_cell.head().noun(), &space), 1);
        assert_eq!(
            InputMetadata::from_noun(&input_noun, &space).expect("roundtrip input metadata"),
            input_metadata
        );

        let witness_noun = witness_data.to_noun(&mut slab);
        let space = slab.noun_space();
        let witness_cell = witness_noun
            .in_space(&space)
            .as_cell()
            .expect("witness data must be tagged");
        assert_eq!(atom_tag_value(&witness_cell.head().noun(), &space), 1);
        assert_eq!(
            WitnessData::from_noun(&witness_noun, &space).expect("roundtrip witness data"),
            witness_data
        );

        let spend_noun = spend.to_noun(&mut slab);
        let space = slab.noun_space();
        let spend_cell = spend_noun
            .in_space(&space)
            .as_cell()
            .expect("spend must be tagged");
        assert_eq!(atom_tag_value(&spend_cell.head().noun(), &space), 0);
        assert_eq!(
            Spend::from_noun(&spend_noun, &space).expect("roundtrip spend"),
            spend
        );

        let legacy_noun = legacy_lock_metadata.to_noun(&mut slab);
        let space = slab.noun_space();
        assert_eq!(
            LockMetadata::from_noun(&legacy_noun, &space).expect("roundtrip legacy lock metadata"),
            legacy_lock_metadata
        );

        let versioned_noun = versioned_lock_metadata.to_noun(&mut slab);
        let space = slab.noun_space();
        let versioned_cell = versioned_noun
            .in_space(&space)
            .as_cell()
            .expect("versioned lock metadata must be tagged");
        assert_eq!(atom_tag_value(&versioned_cell.head().noun(), &space), 1);
        assert_eq!(
            LockMetadata::from_noun(&versioned_noun, &space)
                .expect("roundtrip versioned lock metadata"),
            versioned_lock_metadata
        );

        let bridge_withdrawal_noun = bridge_withdrawal_lock_metadata.to_noun(&mut slab);
        let space = slab.noun_space();
        let bridge_withdrawal_cell = bridge_withdrawal_noun
            .in_space(&space)
            .as_cell()
            .expect("bridge-withdrawal lock metadata must be tagged");
        assert_eq!(
            atom_tag_value(&bridge_withdrawal_cell.head().noun(), &space),
            1
        );
        assert_eq!(
            LockMetadata::from_noun(&bridge_withdrawal_noun, &space)
                .expect("roundtrip bridge-withdrawal lock metadata"),
            bridge_withdrawal_lock_metadata
        );
    }

    #[test]
    fn derived_integer_tags_decode_manual_tagged_nouns() {
        let address_a = Hash::from_base58(ADDRESS_A_B58).expect("address a should parse");
        let sample_name = Name::new(address_a.clone(), address_a.clone());
        let spend_conditions = SpendConditionMap(vec![(
            sample_name,
            pkh_condition(1, vec![address_a.clone()]),
        )]);
        let versioned_lock = VersionedLockMetadata::BridgeDeposit {
            root: address_a.clone(),
            addr: crate::EthAddress([0x11; 20]),
        };

        let mut slab = NounSlab::<NockJammer>::new();
        let spend_conditions_noun = spend_conditions.to_noun(&mut slab);
        let manual_input = nockvm::noun::T(&mut slab, &[nockvm::noun::D(1), spend_conditions_noun]);
        let space = slab.noun_space();
        assert_eq!(
            InputMetadata::from_noun(&manual_input, &space).expect("decode manual input tag"),
            InputMetadata::SpendConditions(spend_conditions)
        );

        let versioned_lock_noun = versioned_lock.to_noun(&mut slab);
        let manual_lock = nockvm::noun::T(&mut slab, &[nockvm::noun::D(1), versioned_lock_noun]);
        let space = slab.noun_space();
        assert_eq!(
            LockMetadata::from_noun(&manual_lock, &space).expect("decode manual versioned lock"),
            LockMetadata::Versioned(versioned_lock)
        );
    }

    #[test]
    fn transaction_roundtrip_uses_integer_version_tag() {
        let mut slab = NounSlab::<NockJammer>::new();
        let transaction = sample_transaction_fixture();

        let noun = transaction.to_noun(&mut slab);
        let space = slab.noun_space();
        let cell = noun
            .in_space(&space)
            .as_cell()
            .expect("transaction should be tagged");
        let version = cell
            .head()
            .as_atom()
            .expect("transaction version must be atom")
            .as_u64()
            .expect("transaction version should fit in u64");
        assert_eq!(version, 1);
        assert_eq!(
            Transaction::from_noun(&noun, &space).expect("roundtrip transaction"),
            transaction
        );

        let Transaction::V1(inner) = sample_transaction_fixture();
        let inner_noun = inner.to_noun(&mut slab);
        let manual = nockvm::noun::T(&mut slab, &[nockvm::noun::D(1), inner_noun]);
        let space = slab.noun_space();
        assert_eq!(
            Transaction::from_noun(&manual, &space)
                .expect("decode manual numeric-tagged transaction"),
            Transaction::V1(inner)
        );
    }

    #[test]
    fn raw_tx_compute_id_changes_with_witness_updates_while_name_stays_stable() {
        let transaction = sample_transaction_fixture();
        let stable_name = match &transaction {
            Transaction::V1(tx) => tx.name.clone(),
        };
        let raw_tx = raw_tx_from_transaction_fixture(&transaction);
        let original_id = raw_tx.compute_id_base58().expect("original raw tx id");

        let mut mutated = raw_tx.clone();
        let witness_signature = mutated
            .spends
            .0
            .iter_mut()
            .find_map(|(_, spend)| match spend {
                Spend::Witness(spend) => spend.witness.pkh_signature.0.first_mut(),
                Spend::Legacy(_) => None,
            })
            .expect("fixture raw tx should contain a witness signature");
        witness_signature.signature.chal[0].0 += 1;

        let mutated_id = mutated.compute_id_base58().expect("mutated raw tx id");
        assert_ne!(original_id, mutated_id);
        assert_eq!(
            stable_name,
            match &transaction {
                Transaction::V1(tx) => tx.name.clone(),
            }
        );
    }

    #[test]
    fn witness_decode_rejects_non_based_hax_preimages() {
        let lock_merkle_proof = LockMerkleProof::new_stub(
            SpendCondition::new(vec![LockPrimitive::Burn]),
            0,
            MerkleProof {
                root: Hash::from_limbs(&[1, 0, 0, 0, 0]),
                path: vec![],
            },
        );
        let pkh_signature = super::PkhSignature::new(vec![]);

        let mut slab = NounSlab::<NockJammer>::new();
        let lmp = lock_merkle_proof.to_noun(&mut slab);
        let pkh = pkh_signature.to_noun(&mut slab);

        let mut hax = D(0);
        let mut key = Hash::from_limbs(&[2, 0, 0, 0, 0]).to_noun(&mut slab);
        let mut value = PRIME.to_noun(&mut slab);
        hax = zmap::z_map_put(&mut slab, &hax, &mut key, &mut value, &DefaultTipHasher)
            .expect("witness hax z-map should encode");

        let tim = 0_usize.to_noun(&mut slab);
        let witness = nockvm::noun::T(&mut slab, &[lmp, pkh, hax, tim]);
        let space = slab.noun_space();

        let err =
            Witness::from_noun(&witness, &space).expect_err("non-based hax preimage should fail");
        assert!(
            err.to_string()
                .contains("owned based noun atom is not based"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn lock_merkle_proof_stub_raw_tx_hash_ignores_axis_but_full_includes_it() {
        let address_a = Hash::from_base58(ADDRESS_A_B58).expect("address a should parse");
        let address_b = Hash::from_base58(ADDRESS_B_B58).expect("address b should parse");
        let spend_condition = pkh_condition(2, vec![address_a.clone(), address_b.clone()]);
        let merkle_proof = MerkleProof {
            root: address_a.clone(),
            path: vec![address_b.clone()],
        };

        let stub_axis_1 =
            LockMerkleProof::new_stub(spend_condition.clone(), 1, merkle_proof.clone());
        let stub_axis_7 =
            LockMerkleProof::new_stub(spend_condition.clone(), 7, merkle_proof.clone());
        let full_axis_1 =
            LockMerkleProof::new_full(spend_condition.clone(), 1, merkle_proof.clone());
        let full_axis_7 = LockMerkleProof::new_full(spend_condition, 7, merkle_proof);

        assert_eq!(
            stub_axis_1
                .hash_digest()
                .expect("stub raw-tx hash should compute"),
            stub_axis_7
                .hash_digest()
                .expect("stub raw-tx hash should compute")
        );
        assert_ne!(
            full_axis_1
                .hash_digest()
                .expect("full raw-tx hash should compute"),
            full_axis_7
                .hash_digest()
                .expect("full raw-tx hash should compute")
        );
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
        let space = slab.noun_space();
        let decoded = Lock::from_noun(&noun, &space).expect("lock should decode");
        assert_eq!(decoded.spend_condition_count(), 4);
        assert_eq!(decoded.flatten_spend_conditions().len(), 4);
    }
}
