use nockchain_math::belt::Belt;
use nockchain_types::tx_engine::common::Name;
use nockchain_types::tx_engine::v1::{
    BalanceUpdate, Hax, HaxPreimage as V1HaxPreimage, LockMerkleProof, LockPrimitive, LockTim,
    MerkleProof, Note, NoteData, NoteDataEntry, NoteV1, Pkh, PkhSignature, PkhSignatureEntry,
    RawTx as V1RawTx, Seed as V1Seed, Spend as V1Spend, Spend0, Spend1, SpendCondition,
    Witness as V1Witness,
};
use nockchain_types::{v0, v1};
use nockvm_macros::tas;

use crate::common::{ConversionError, Required};
use crate::pb::common::v1::{
    BlockHeight as PbBlockHeight, Hash as PbHash, Name as PbName, Nicks as PbNicks,
    NoteVersion as PbNoteVersion, PageResponse as PbPageResponse, SchnorrPubkey as PbSchnorrPubkey,
    SchnorrSignature as PbSchnorrSignature, Signature as PbSignature, Source as PbSource,
    TimeLockRangeAbsolute as PbTimeLockRangeAbsolute,
    TimeLockRangeRelative as PbTimeLockRangeRelative,
};
use crate::pb::common::v2::{
    lock_primitive, note, spend, Balance as PbBalance, BalanceEntry as PbBalanceEntry,
    BurnLock as PbBurnLock, HaxLock as PbHaxLock, HaxPreimage as PbHaxPreimage,
    LegacySpend as PbLegacySpend, LockMerkleProof as PbLockMerkleProof,
    LockPrimitive as PbLockPrimitive, LockTim as PbLockTim, MerkleProof as PbMerkleProof,
    Note as PbNote, NoteData as PbNoteData, NoteDataEntry as PbNoteDataEntry, NoteV1 as PbNoteV1,
    PkhLock as PbPkhLock, PkhSignature as PbPkhSignature, PkhSignatureEntry as PbPkhSignatureEntry,
    RawTransaction as PbRawTransaction, Seed as PbSeed, Spend as PbSpend,
    SpendCondition as PbSpendCondition, SpendEntry as PbSpendEntry, Witness as PbWitness,
    WitnessSpend as PbWitnessSpend,
};
use crate::pb::public::v2::{
    wallet_get_balance_response, WalletGetBalanceResponse as PbWalletGetBalanceResponse,
};

impl From<NoteDataEntry> for PbNoteDataEntry {
    fn from(entry: NoteDataEntry) -> Self {
        PbNoteDataEntry {
            key: entry.key,
            blob: entry.blob.to_vec(),
        }
    }
}

impl TryFrom<PbNoteDataEntry> for NoteDataEntry {
    type Error = ConversionError;
    fn try_from(entry: PbNoteDataEntry) -> Result<Self, Self::Error> {
        Ok(NoteDataEntry {
            key: entry.key,
            blob: entry.blob.into(),
        })
    }
}

impl From<NoteData> for PbNoteData {
    fn from(data: NoteData) -> Self {
        PbNoteData {
            entries: data.0.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<PbNoteData> for NoteData {
    type Error = ConversionError;
    fn try_from(data: PbNoteData) -> Result<Self, Self::Error> {
        let entries = data
            .entries
            .into_iter()
            .map(NoteDataEntry::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(NoteData(entries))
    }
}

impl From<NoteV1> for PbNoteV1 {
    fn from(note: NoteV1) -> Self {
        let version_value: u32 = note.version.into();
        PbNoteV1 {
            version: Some(PbNoteVersion {
                value: version_value,
            }),
            origin_page: Some(PbBlockHeight::from(note.origin_page)),
            name: Some(PbName::from(note.name)),
            note_data: Some(PbNoteData::from(note.note_data)),
            assets: Some(PbNicks::from(note.assets)),
        }
    }
}

impl From<Note> for PbNote {
    fn from(note: Note) -> Self {
        let note_version = match note {
            Note::V0(legacy) => note::NoteVersion::Legacy(legacy.into()),
            Note::V1(v1) => note::NoteVersion::V1(v1.into()),
        };
        PbNote {
            note_version: Some(note_version),
        }
    }
}

impl TryFrom<PbNote> for Note {
    type Error = ConversionError;
    fn try_from(note: PbNote) -> Result<Self, Self::Error> {
        match note.note_version.required("Note", "note_version")? {
            note::NoteVersion::Legacy(legacy) => Ok(Note::V0(legacy.try_into()?)),
            note::NoteVersion::V1(v1) => Ok(Note::V1(NoteV1 {
                version: v1::Version::V1,
                origin_page: v1::BlockHeight(Belt(
                    v1.origin_page.required("NoteV1", "origin_page")?.value,
                )),
                name: v0::Name::try_from(v1.name.required("NoteV1", "name")?)?,
                note_data: NoteData::try_from(v1.note_data.required("NoteV1", "note_data")?)?,
                assets: v1.assets.required("NoteV1", "assets")?.into(),
            })),
        }
    }
}

impl TryFrom<PbBalance> for BalanceUpdate {
    type Error = ConversionError;
    fn try_from(update: PbBalance) -> Result<Self, Self::Error> {
        let notes: Vec<(v1::Name, v1::Note)> = update
            .notes
            .into_iter()
            .map(|be| -> Result<(v1::Name, v1::Note), ConversionError> {
                Ok((
                    v1::Name::try_from(be.name.required("BalanceEntry", "name")?)?,
                    v1::Note::try_from(be.note.required("BalanceEntry", "note")?)?,
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(BalanceUpdate {
            height: v1::BlockHeight(Belt(update.height.required("Balance", "height")?.value)),
            block_id: v1::Hash::try_from(update.block_id.required("Balance", "block_id")?)?,
            notes: v1::Balance(notes),
        })
    }
}

impl From<BalanceUpdate> for PbBalance {
    fn from(update: BalanceUpdate) -> Self {
        let notes = update
            .notes
            .0
            .into_iter()
            .map(|(name, note)| PbBalanceEntry {
                name: Some(PbName::from(name)),
                note: Some(PbNote::from(note)),
            })
            .collect();

        PbBalance {
            notes,
            height: Some(PbBlockHeight::from(update.height)),
            block_id: Some(PbHash::from(update.block_id)),
            page: Some(PbPageResponse {
                next_page_token: String::new(),
            }),
        }
    }
}

impl From<BalanceUpdate> for PbWalletGetBalanceResponse {
    fn from(update: BalanceUpdate) -> Self {
        PbWalletGetBalanceResponse {
            result: Some(wallet_get_balance_response::Result::Balance(
                PbBalance::from(update),
            )),
        }
    }
}

impl From<Spend0> for PbLegacySpend {
    fn from(spend: Spend0) -> Self {
        let seeds = spend.seeds.0.into_iter().map(PbSeed::from).collect();

        PbLegacySpend {
            signature: Some(PbSignature::from(spend.signature)),
            seeds,
            fee: Some(PbNicks::from(spend.fee)),
        }
    }
}

impl From<V1Seed> for PbSeed {
    fn from(seed: V1Seed) -> Self {
        PbSeed {
            output_source: seed.output_source.map(PbSource::from),
            lock_root: Some(PbHash::from(seed.lock_root)),
            note_data: Some(PbNoteData::from(seed.note_data)),
            gift: Some(PbNicks::from(seed.gift)),
            parent_hash: Some(PbHash::from(seed.parent_hash)),
        }
    }
}

impl From<Spend1> for PbWitnessSpend {
    fn from(spend: Spend1) -> Self {
        let seeds = spend.seeds.0.into_iter().map(PbSeed::from).collect();

        PbWitnessSpend {
            witness: Some(PbWitness::from(spend.witness)),
            seeds,
            fee: Some(PbNicks::from(spend.fee)),
        }
    }
}

impl From<V1Spend> for PbSpend {
    fn from(spend: V1Spend) -> Self {
        let spend_kind = match spend {
            V1Spend::Legacy(legacy) => spend::SpendKind::Legacy(legacy.into()),
            V1Spend::Witness(witness) => spend::SpendKind::Witness(witness.into()),
        };
        PbSpend {
            spend_kind: Some(spend_kind),
        }
    }
}

impl From<(Name, V1Spend)> for PbSpendEntry {
    fn from(entry: (Name, V1Spend)) -> Self {
        PbSpendEntry {
            name: Some(PbName::from(entry.0)),
            spend: Some(PbSpend::from(entry.1)),
        }
    }
}

impl From<SpendCondition> for PbSpendCondition {
    fn from(condition: SpendCondition) -> Self {
        PbSpendCondition {
            primitives: condition.0.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<Pkh> for PbPkhLock {
    fn from(pkh: Pkh) -> Self {
        let hashes = pkh.hashes.into_iter().map(PbHash::from).collect::<Vec<_>>();
        PbPkhLock { m: pkh.m, hashes }
    }
}

impl From<LockTim> for PbLockTim {
    fn from(tim: LockTim) -> Self {
        PbLockTim {
            rel: Some(PbTimeLockRangeRelative::from(tim.rel)),
            abs: Some(PbTimeLockRangeAbsolute::from(tim.abs)),
        }
    }
}

impl From<Hax> for PbHaxLock {
    fn from(hax: Hax) -> Self {
        let hashes = hax.0.into_iter().map(PbHash::from).collect::<Vec<_>>();
        PbHaxLock { hashes }
    }
}

impl From<LockPrimitive> for PbLockPrimitive {
    fn from(primitive: LockPrimitive) -> Self {
        let primitive = match primitive {
            LockPrimitive::Pkh(pkh) => lock_primitive::Primitive::Pkh(pkh.into()),
            LockPrimitive::Tim(tim) => lock_primitive::Primitive::Tim(tim.into()),
            LockPrimitive::Hax(hax) => lock_primitive::Primitive::Hax(hax.into()),
            LockPrimitive::Burn => lock_primitive::Primitive::Burn(PbBurnLock {}),
        };
        PbLockPrimitive {
            primitive: Some(primitive),
        }
    }
}

impl From<PkhSignatureEntry> for PbPkhSignatureEntry {
    fn from(entry: PkhSignatureEntry) -> Self {
        PbPkhSignatureEntry {
            hash: Some(PbHash::from(entry.hash)),
            pubkey: Some(PbSchnorrPubkey::from(entry.pubkey)),
            signature: Some(PbSchnorrSignature::from(entry.signature)),
        }
    }
}

impl From<PkhSignature> for PbPkhSignature {
    fn from(signature: PkhSignature) -> Self {
        PbPkhSignature {
            entries: signature.0.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<V1HaxPreimage> for PbHaxPreimage {
    fn from(preimage: V1HaxPreimage) -> Self {
        PbHaxPreimage {
            hash: Some(PbHash::from(preimage.hash)),
            value: preimage.value.to_vec(),
        }
    }
}

impl From<MerkleProof> for PbMerkleProof {
    fn from(proof: MerkleProof) -> Self {
        PbMerkleProof {
            root: Some(PbHash::from(proof.root)),
            path: proof.path.into_iter().map(PbHash::from).collect(),
        }
    }
}

impl From<LockMerkleProof> for PbLockMerkleProof {
    fn from(proof: LockMerkleProof) -> Self {
        match proof {
            LockMerkleProof::Full(full) => PbLockMerkleProof {
                spend_condition: Some(PbSpendCondition::from(full.spend_condition)),
                axis: full.axis,
                proof: Some(PbMerkleProof::from(full.proof)),
                lmp_version: Some(full.version),
            },
            LockMerkleProof::Stub(stub) => PbLockMerkleProof {
                spend_condition: Some(PbSpendCondition::from(stub.spend_condition)),
                axis: stub.axis,
                proof: Some(PbMerkleProof::from(stub.proof)),
                lmp_version: None,
            },
        }
    }
}

impl From<V1Witness> for PbWitness {
    fn from(witness: V1Witness) -> Self {
        PbWitness {
            lock_merkle_proof: Some(PbLockMerkleProof::from(witness.lock_merkle_proof)),
            pkh_signature: Some(PbPkhSignature::from(witness.pkh_signature)),
            hax: witness.hax.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<V1RawTx> for PbRawTransaction {
    fn from(tx: V1RawTx) -> Self {
        PbRawTransaction {
            version: Some(PbNoteVersion {
                value: tx.version.into(),
            }),
            id: Some(PbHash::from(tx.id)),
            spends: tx.spends.0.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<PbSeed> for V1Seed {
    type Error = ConversionError;
    fn try_from(seed: PbSeed) -> Result<Self, Self::Error> {
        Ok(V1Seed {
            output_source: seed.output_source.map(|s| s.try_into()).transpose()?,
            lock_root: v1::Hash::try_from(seed.lock_root.required("Seed", "lock_root")?)?,
            note_data: NoteData::try_from(seed.note_data.required("Seed", "note_data")?)?,
            gift: seed.gift.required("Seed", "gift")?.into(),
            parent_hash: v1::Hash::try_from(seed.parent_hash.required("Seed", "parent_hash")?)?,
        })
    }
}

impl TryFrom<PbPkhSignatureEntry> for PkhSignatureEntry {
    type Error = ConversionError;
    fn try_from(entry: PbPkhSignatureEntry) -> Result<Self, Self::Error> {
        Ok(PkhSignatureEntry {
            hash: v1::Hash::try_from(entry.hash.required("PkhSignatureEntry", "hash")?)?,
            pubkey: entry
                .pubkey
                .required("PkhSignatureEntry", "pubkey")?
                .try_into()?,
            signature: entry
                .signature
                .required("PkhSignatureEntry", "signature")?
                .try_into()?,
        })
    }
}

impl TryFrom<PbPkhSignature> for PkhSignature {
    type Error = ConversionError;
    fn try_from(signature: PbPkhSignature) -> Result<Self, Self::Error> {
        let entries = signature
            .entries
            .into_iter()
            .map(PkhSignatureEntry::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PkhSignature(entries))
    }
}

impl TryFrom<PbHaxPreimage> for V1HaxPreimage {
    type Error = ConversionError;
    fn try_from(preimage: PbHaxPreimage) -> Result<Self, Self::Error> {
        Ok(V1HaxPreimage {
            hash: v1::Hash::try_from(preimage.hash.required("HaxPreimage", "hash")?)?,
            value: preimage.value.into(),
        })
    }
}

impl TryFrom<PbMerkleProof> for MerkleProof {
    type Error = ConversionError;
    fn try_from(proof: PbMerkleProof) -> Result<Self, Self::Error> {
        Ok(MerkleProof {
            root: v1::Hash::try_from(proof.root.required("MerkleProof", "root")?)?,
            path: proof
                .path
                .into_iter()
                .map(v1::Hash::try_from)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl TryFrom<PbLockPrimitive> for LockPrimitive {
    type Error = ConversionError;
    fn try_from(primitive: PbLockPrimitive) -> Result<Self, Self::Error> {
        match primitive.primitive.required("LockPrimitive", "primitive")? {
            lock_primitive::Primitive::Pkh(pkh) => {
                let hashes = pkh
                    .hashes
                    .into_iter()
                    .map(v1::Hash::try_from)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(LockPrimitive::Pkh(Pkh::new(pkh.m, hashes)))
            }
            lock_primitive::Primitive::Tim(tim) => Ok(LockPrimitive::Tim(LockTim {
                rel: tim.rel.required("LockTim", "rel")?.into(),
                abs: tim.abs.required("LockTim", "abs")?.into(),
            })),
            lock_primitive::Primitive::Hax(hax) => {
                let hashes = hax
                    .hashes
                    .into_iter()
                    .map(v1::Hash::try_from)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(LockPrimitive::Hax(Hax::new(hashes)))
            }
            lock_primitive::Primitive::Burn(_) => Ok(LockPrimitive::Burn),
        }
    }
}

impl TryFrom<PbSpendCondition> for SpendCondition {
    type Error = ConversionError;
    fn try_from(condition: PbSpendCondition) -> Result<Self, Self::Error> {
        let primitives = condition
            .primitives
            .into_iter()
            .map(LockPrimitive::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(SpendCondition(primitives))
    }
}

impl TryFrom<PbLockMerkleProof> for LockMerkleProof {
    type Error = ConversionError;
    fn try_from(proof: PbLockMerkleProof) -> Result<Self, Self::Error> {
        let pb_version = proof.lmp_version;
        let spend_condition = SpendCondition::try_from(
            proof
                .spend_condition
                .required("LockMerkleProof", "spend_condition")?,
        )?;
        let axis = proof.axis;
        let merkle_proof =
            MerkleProof::try_from(proof.proof.required("LockMerkleProof", "proof")?)?;
        if let Some(version) = pb_version {
            if version != tas!(b"full") {
                return Err(ConversionError::Invalid("invalid lmp version"));
            }
            return Ok(LockMerkleProof::new_full(
                spend_condition, axis, merkle_proof,
            ));
        }

        Ok(LockMerkleProof::new_stub(
            spend_condition, axis, merkle_proof,
        ))
    }
}

impl TryFrom<PbWitness> for V1Witness {
    type Error = ConversionError;
    fn try_from(witness: PbWitness) -> Result<Self, Self::Error> {
        Ok(V1Witness {
            lock_merkle_proof: LockMerkleProof::try_from(
                witness
                    .lock_merkle_proof
                    .required("Witness", "lock_merkle_proof")?,
            )?,
            pkh_signature: PkhSignature::try_from(
                witness.pkh_signature.required("Witness", "pkh_signature")?,
            )?,
            hax: witness
                .hax
                .into_iter()
                .map(V1HaxPreimage::try_from)
                .collect::<Result<Vec<_>, _>>()?,
            tim: 0,
        })
    }
}

impl TryFrom<PbLegacySpend> for Spend0 {
    type Error = ConversionError;
    fn try_from(spend: PbLegacySpend) -> Result<Self, Self::Error> {
        let seeds = spend
            .seeds
            .into_iter()
            .map(V1Seed::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Spend0 {
            signature: spend
                .signature
                .required("LegacySpend", "signature")?
                .try_into()?,
            seeds: nockchain_types::tx_engine::v1::Seeds(seeds),
            fee: spend.fee.required("LegacySpend", "fee")?.into(),
        })
    }
}

impl TryFrom<PbWitnessSpend> for Spend1 {
    type Error = ConversionError;
    fn try_from(spend: PbWitnessSpend) -> Result<Self, Self::Error> {
        let seeds = spend
            .seeds
            .into_iter()
            .map(V1Seed::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Spend1 {
            witness: V1Witness::try_from(spend.witness.required("WitnessSpend", "witness")?)?,
            seeds: nockchain_types::tx_engine::v1::Seeds(seeds),
            fee: spend.fee.required("WitnessSpend", "fee")?.into(),
        })
    }
}

impl TryFrom<PbSpend> for V1Spend {
    type Error = ConversionError;
    fn try_from(spend: PbSpend) -> Result<Self, Self::Error> {
        match spend.spend_kind.required("Spend", "spend_kind")? {
            spend::SpendKind::Legacy(legacy) => Ok(V1Spend::Legacy(Spend0::try_from(legacy)?)),
            spend::SpendKind::Witness(witness) => Ok(V1Spend::Witness(Spend1::try_from(witness)?)),
        }
    }
}

impl TryFrom<PbSpendEntry> for (Name, V1Spend) {
    type Error = ConversionError;
    fn try_from(entry: PbSpendEntry) -> Result<Self, Self::Error> {
        Ok((
            v0::Name::try_from(entry.name.required("SpendEntry", "name")?)?,
            V1Spend::try_from(entry.spend.required("SpendEntry", "spend")?)?,
        ))
    }
}

impl TryFrom<PbRawTransaction> for V1RawTx {
    type Error = ConversionError;
    fn try_from(tx: PbRawTransaction) -> Result<Self, Self::Error> {
        let version_value = tx.version.required("RawTransaction", "version")?.value;

        let version = match version_value {
            1 => v1::Version::V1,
            _ => return Err(ConversionError::Invalid("invalid version")),
        };

        let spends = tx
            .spends
            .into_iter()
            .map(<(Name, V1Spend)>::try_from)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(V1RawTx {
            version,
            id: v0::Hash::try_from(tx.id.required("RawTransaction", "id")?)?,
            spends: nockchain_types::tx_engine::v1::Spends(spends),
        })
    }
}

#[cfg(test)]
mod tests {
    use nockchain_types::tx_engine::common::{Hash, Name, Nicks, Version};

    use super::*;

    fn sample_hash(seed: u64) -> Hash {
        Hash::from_limbs(&[seed, seed + 1, seed + 2, seed + 3, seed + 4])
    }

    fn sample_spend_condition() -> SpendCondition {
        SpendCondition::new(vec![LockPrimitive::Burn])
    }

    fn sample_merkle_proof() -> MerkleProof {
        MerkleProof {
            root: Hash::from_limbs(&[1, 2, 3, 4, 5]),
            path: vec![Hash::from_limbs(&[6, 7, 8, 9, 10])],
        }
    }

    #[test]
    fn test_lock_merkle_proof_stub_roundtrip() {
        let spend_condition = sample_spend_condition();
        let merkle_proof = sample_merkle_proof();
        let stub = LockMerkleProof::new_stub(spend_condition, 42, merkle_proof);

        let pb = PbLockMerkleProof::from(stub.clone());
        assert!(pb.lmp_version.is_none());

        let decoded = LockMerkleProof::try_from(pb).expect("decode stub");
        assert_eq!(decoded, stub);
    }

    #[test]
    fn test_lock_merkle_proof_full_roundtrip() {
        let spend_condition = sample_spend_condition();
        let merkle_proof = sample_merkle_proof();
        let full = LockMerkleProof::new_full(spend_condition, 84, merkle_proof);
        let expected_version = match &full {
            LockMerkleProof::Full(proof) => proof.version,
            LockMerkleProof::Stub(_) => panic!("expected full proof"),
        };

        let pb = PbLockMerkleProof::from(full.clone());
        assert_eq!(pb.lmp_version, Some(expected_version));

        let decoded = LockMerkleProof::try_from(pb).expect("decode full");
        assert_eq!(decoded, full);
    }

    #[test]
    fn test_lock_merkle_proof_invalid_version_rejected() {
        let spend_condition = sample_spend_condition();
        let merkle_proof = sample_merkle_proof();
        let stub = LockMerkleProof::new_stub(spend_condition.clone(), 7, merkle_proof.clone());
        let expected_version = match LockMerkleProof::new_full(spend_condition, 7, merkle_proof) {
            LockMerkleProof::Full(proof) => proof.version,
            LockMerkleProof::Stub(_) => panic!("expected full proof"),
        };

        let mut pb = PbLockMerkleProof::from(stub);
        pb.lmp_version = Some(expected_version + 1);

        let err = LockMerkleProof::try_from(pb).expect_err("invalid version should fail");
        match err {
            ConversionError::Invalid(_) => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn test_pkh_lock_decode_canonicalizes_hash_set() {
        let hash_a = sample_hash(11);
        let hash_b = sample_hash(21);
        let pb = PbLockPrimitive {
            primitive: Some(lock_primitive::Primitive::Pkh(PbPkhLock {
                m: 2,
                hashes: vec![
                    PbHash::from(hash_b.clone()),
                    PbHash::from(hash_a.clone()),
                    PbHash::from(hash_b.clone()),
                ],
            })),
        };

        let decoded = LockPrimitive::try_from(pb).expect("decode pkh lock");
        let expected = LockPrimitive::Pkh(Pkh::new(2, vec![hash_a, hash_b]));
        assert_eq!(decoded, expected);

        let roundtrip = PbLockPrimitive::from(decoded.clone());
        let decoded_roundtrip = LockPrimitive::try_from(roundtrip).expect("re-decode pkh lock");
        assert_eq!(decoded_roundtrip, expected);
    }

    #[test]
    fn test_hax_lock_decode_canonicalizes_hash_set() {
        let hash_a = sample_hash(31);
        let hash_b = sample_hash(41);
        let pb = PbLockPrimitive {
            primitive: Some(lock_primitive::Primitive::Hax(PbHaxLock {
                hashes: vec![
                    PbHash::from(hash_b.clone()),
                    PbHash::from(hash_a.clone()),
                    PbHash::from(hash_b.clone()),
                ],
            })),
        };

        let decoded = LockPrimitive::try_from(pb).expect("decode hax lock");
        let expected = LockPrimitive::Hax(Hax::new(vec![hash_a, hash_b]));
        assert_eq!(decoded, expected);

        let roundtrip = PbLockPrimitive::from(decoded.clone());
        let decoded_roundtrip = LockPrimitive::try_from(roundtrip).expect("re-decode hax lock");
        assert_eq!(decoded_roundtrip, expected);
    }

    #[test]
    fn test_spend_condition_decode_canonicalizes_nested_set_locks() {
        let pkh_a = sample_hash(51);
        let pkh_b = sample_hash(61);
        let hax_a = sample_hash(71);
        let hax_b = sample_hash(81);
        let pb = PbSpendCondition {
            primitives: vec![
                PbLockPrimitive {
                    primitive: Some(lock_primitive::Primitive::Pkh(PbPkhLock {
                        m: 2,
                        hashes: vec![
                            PbHash::from(pkh_b.clone()),
                            PbHash::from(pkh_a.clone()),
                            PbHash::from(pkh_b.clone()),
                        ],
                    })),
                },
                PbLockPrimitive {
                    primitive: Some(lock_primitive::Primitive::Hax(PbHaxLock {
                        hashes: vec![
                            PbHash::from(hax_b.clone()),
                            PbHash::from(hax_a.clone()),
                            PbHash::from(hax_b.clone()),
                        ],
                    })),
                },
                PbLockPrimitive {
                    primitive: Some(lock_primitive::Primitive::Burn(PbBurnLock {})),
                },
            ],
        };

        let decoded = SpendCondition::try_from(pb).expect("decode spend condition");
        let expected = SpendCondition::new(vec![
            LockPrimitive::Pkh(Pkh::new(2, vec![pkh_a, pkh_b])),
            LockPrimitive::Hax(Hax::new(vec![hax_a, hax_b])),
            LockPrimitive::Burn,
        ]);
        assert_eq!(decoded, expected);

        let roundtrip = PbSpendCondition::from(decoded.clone());
        let decoded_roundtrip =
            SpendCondition::try_from(roundtrip).expect("re-decode spend condition");
        assert_eq!(decoded_roundtrip, expected);
    }

    #[test]
    fn test_raw_transaction_roundtrip_canonicalizes_nested_set_locks() {
        let pkh_a = sample_hash(91);
        let pkh_b = sample_hash(101);
        let hax_a = sample_hash(111);
        let hax_b = sample_hash(121);
        let expected = V1RawTx {
            version: Version::V1,
            id: sample_hash(131),
            spends: nockchain_types::tx_engine::v1::Spends(vec![(
                Name::new(sample_hash(141), sample_hash(151)),
                V1Spend::Witness(Spend1 {
                    witness: V1Witness::new(
                        LockMerkleProof::new_stub(
                            SpendCondition::new(vec![
                                LockPrimitive::Pkh(Pkh::new(2, vec![pkh_a.clone(), pkh_b.clone()])),
                                LockPrimitive::Hax(Hax::new(vec![hax_a.clone(), hax_b.clone()])),
                            ]),
                            42,
                            sample_merkle_proof(),
                        ),
                        PkhSignature(Vec::new()),
                        Vec::new(),
                    ),
                    seeds: nockchain_types::tx_engine::v1::Seeds(Vec::new()),
                    fee: Nicks(7),
                }),
            )]),
        };

        let mut pb = PbRawTransaction::from(expected.clone());
        let spend = pb.spends.first_mut().expect("raw tx spend entry");
        let witness = match spend
            .spend
            .as_mut()
            .and_then(|spend| spend.spend_kind.as_mut())
            .expect("witness spend kind")
        {
            spend::SpendKind::Witness(witness) => witness,
            other => panic!("expected witness spend, got {other:?}"),
        };
        let lock_merkle_proof = witness
            .witness
            .as_mut()
            .and_then(|witness| witness.lock_merkle_proof.as_mut())
            .expect("lock merkle proof");
        let primitives = &mut lock_merkle_proof
            .spend_condition
            .as_mut()
            .expect("spend condition")
            .primitives;

        match primitives
            .get_mut(0)
            .and_then(|primitive| primitive.primitive.as_mut())
            .expect("pkh primitive")
        {
            lock_primitive::Primitive::Pkh(pkh) => {
                pkh.hashes = vec![
                    PbHash::from(pkh_b.clone()),
                    PbHash::from(pkh_a.clone()),
                    PbHash::from(pkh_b.clone()),
                ];
            }
            other => panic!("expected pkh primitive, got {other:?}"),
        }

        match primitives
            .get_mut(1)
            .and_then(|primitive| primitive.primitive.as_mut())
            .expect("hax primitive")
        {
            lock_primitive::Primitive::Hax(hax) => {
                hax.hashes = vec![
                    PbHash::from(hax_b.clone()),
                    PbHash::from(hax_a.clone()),
                    PbHash::from(hax_b.clone()),
                ];
            }
            other => panic!("expected hax primitive, got {other:?}"),
        }

        let decoded = V1RawTx::try_from(pb).expect("decode raw transaction");
        assert_eq!(decoded, expected);
    }
}
