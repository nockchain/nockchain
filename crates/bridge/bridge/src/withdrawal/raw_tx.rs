use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::{Bytes, NounAllocator};
use nockapp_grpc::pb::common::v2::RawTransaction as PbRawTransaction;
use nockchain_types::tx_engine::common::{Hash, Version};
use noun_serde::{NounDecode, NounEncode};

use crate::shared::errors::BridgeError;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SubmittedRawTx {
    pub tx_id_base58: String,
    pub raw_tx: nockchain_types::v1::RawTx,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PersistedRawTx {
    pub tx_id_base58: String,
    pub raw_tx: nockchain_types::v1::RawTx,
    pub raw_tx_bytes: Vec<u8>,
}

/// Returns the base58 transaction id consensus derives from the fully merged
/// raw transaction contents, rather than the stable envelope transaction name.
pub(crate) fn submitted_raw_tx_id_base58(
    transaction: &nockchain_types::v1::Transaction,
) -> Result<String, BridgeError> {
    Ok(SubmittedRawTx::try_from(transaction)?.tx_id_base58)
}

/// Converts a fully merged transaction envelope into the raw transaction shape
/// expected by submission and computes the consensus raw transaction id from
/// the finalized spend set.
pub(crate) fn raw_tx_from_transaction(
    transaction: &nockchain_types::v1::Transaction,
) -> Result<nockchain_types::v1::RawTx, BridgeError> {
    Ok(SubmittedRawTx::try_from(transaction)?.raw_tx)
}

/// Returns the stored submitted raw transaction id for an already constructed raw tx.
pub(crate) fn raw_tx_id_base58(raw_tx: &nockchain_types::v1::RawTx) -> String {
    raw_tx.id.to_base58()
}

/// Encodes a raw transaction into durable bytes for retry/resubmission.
pub(crate) fn encode_raw_tx(raw_tx: &nockchain_types::v1::RawTx) -> Result<Vec<u8>, BridgeError> {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let noun = raw_tx.to_noun(&mut slab);
    slab.set_root(noun);
    Ok(slab.jam().to_vec())
}

/// Decodes durable raw-transaction bytes back into a typed raw transaction.
pub(crate) fn decode_raw_tx(bytes: Vec<u8>) -> Result<nockchain_types::v1::RawTx, BridgeError> {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let noun = slab
        .cue_into(Bytes::from(bytes))
        .map_err(|err| BridgeError::Runtime(format!("failed to cue raw tx bytes: {err}")))?;
    let space = slab.noun_space();
    nockchain_types::v1::RawTx::from_noun(&noun, &space)
        .map_err(|err| BridgeError::Runtime(format!("failed to decode raw tx noun: {err}")))
}

/// Builds the persisted raw-tx retry artifact from a finalized structured
/// transaction, computing the raw tx exactly once.
pub(crate) fn persisted_raw_tx_from_transaction(
    transaction: &nockchain_types::v1::Transaction,
) -> Result<PersistedRawTx, BridgeError> {
    let submitted = SubmittedRawTx::try_from(transaction)?;
    let raw_tx = submitted.raw_tx;
    let raw_tx_bytes = encode_raw_tx(&raw_tx)?;
    Ok(PersistedRawTx {
        tx_id_base58: submitted.tx_id_base58,
        raw_tx,
        raw_tx_bytes,
    })
}

impl TryFrom<&nockchain_types::v1::Transaction> for SubmittedRawTx {
    type Error = BridgeError;

    fn try_from(transaction: &nockchain_types::v1::Transaction) -> Result<Self, Self::Error> {
        match transaction {
            nockchain_types::v1::Transaction::V1(tx) => {
                let version = Version::V1;
                let spends = apply_witness_data_to_spends(&tx.spends, &tx.witness_data)?;
                let id = Hash::from_limbs(&[0, 0, 0, 0, 0]);
                let raw_tx = canonicalize_raw_tx_for_submission(nockchain_types::v1::RawTx {
                    version,
                    id,
                    spends,
                })?;
                Ok(Self {
                    tx_id_base58: raw_tx.id.to_base58(),
                    raw_tx,
                })
            }
        }
    }
}

fn canonicalize_raw_tx_for_submission(
    raw_tx: nockchain_types::v1::RawTx,
) -> Result<nockchain_types::v1::RawTx, BridgeError> {
    let pb_raw_tx = PbRawTransaction::from(raw_tx);
    let mut raw_tx = nockchain_types::v1::RawTx::try_from(pb_raw_tx).map_err(|err| {
        BridgeError::Runtime(format!(
            "failed to canonicalize withdrawal raw tx through protobuf conversion: {err}"
        ))
    })?;
    raw_tx.id = raw_tx
        .compute_id()
        .map_err(|err| BridgeError::Runtime(format!("failed to compute raw tx id: {err}")))?;
    Ok(raw_tx)
}

/// Applies the transaction's merged witness data back onto its spends so the
/// submitted raw transaction contains the exact witness set consensus will
/// validate.
fn apply_witness_data_to_spends(
    spends: &nockchain_types::v1::Spends,
    witness_data: &nockchain_types::v1::WitnessData,
) -> Result<nockchain_types::v1::Spends, BridgeError> {
    let spends = match witness_data {
        nockchain_types::v1::WitnessData::Signatures(signature_map) => spends
            .0
            .iter()
            .map(|(name, spend)| {
                let (_, signature) = signature_map
                    .0
                    .iter()
                    .find(|(signature_name, _)| signature_name == name)
                    .ok_or_else(|| {
                        BridgeError::Runtime(format!(
                            "missing signature entry for withdrawal spend {name:?}"
                        ))
                    })?;
                let nockchain_types::v1::Spend::Legacy(legacy_spend) = spend else {
                    return Err(BridgeError::Runtime(
                        "legacy signature witness data requires legacy spends".into(),
                    ));
                };
                Ok((
                    name.clone(),
                    nockchain_types::v1::Spend::Legacy(nockchain_types::v1::Spend0 {
                        signature: signature.clone(),
                        seeds: legacy_spend.seeds.clone(),
                        fee: legacy_spend.fee.clone(),
                    }),
                ))
            })
            .collect::<Result<Vec<_>, BridgeError>>()?,
        nockchain_types::v1::WitnessData::Witnesses(witness_map) => spends
            .0
            .iter()
            .map(|(name, spend)| {
                let (_, witness) = witness_map
                    .0
                    .iter()
                    .find(|(witness_name, _)| witness_name == name)
                    .ok_or_else(|| {
                        BridgeError::Runtime(format!(
                            "missing witness entry for withdrawal spend {name:?}"
                        ))
                    })?;
                let nockchain_types::v1::Spend::Witness(witness_spend) = spend else {
                    return Err(BridgeError::Runtime(
                        "witness-based witness data requires witness spends".into(),
                    ));
                };
                Ok((
                    name.clone(),
                    nockchain_types::v1::Spend::Witness(nockchain_types::v1::Spend1 {
                        witness: witness.clone(),
                        seeds: witness_spend.seeds.clone(),
                        fee: witness_spend.fee.clone(),
                    }),
                ))
            })
            .collect::<Result<Vec<_>, BridgeError>>()?,
    };
    Ok(nockchain_types::v1::Spends(spends))
}

#[cfg(test)]
mod tests {
    use nockchain_types::tx_engine::common::{Name, Nicks};
    use nockchain_types::v1::{
        LockMerkleProof, LockPrimitive, MerkleProof, Pkh, PkhSignature, Spend, Spend1,
        SpendCondition, Spends, Witness,
    };

    use super::*;

    fn sample_hash(seed: u64) -> Hash {
        Hash::from_limbs(&[seed, seed + 1, seed + 2, seed + 3, seed + 4])
    }

    #[test]
    fn canonicalize_raw_tx_for_submission_recomputes_embedded_id() {
        let spend_condition = SpendCondition::new(vec![LockPrimitive::Pkh(Pkh::new(
            1,
            vec![sample_hash(10), sample_hash(20)],
        ))]);
        let raw_tx = nockchain_types::v1::RawTx {
            version: Version::V1,
            id: sample_hash(999),
            spends: Spends(vec![(
                Name::new(sample_hash(30), sample_hash(40)),
                Spend::Witness(Spend1 {
                    witness: Witness::new(
                        LockMerkleProof::new_stub(
                            spend_condition,
                            1,
                            MerkleProof {
                                root: sample_hash(50),
                                path: Vec::new(),
                            },
                        ),
                        PkhSignature(Vec::new()),
                        Vec::new(),
                    ),
                    seeds: nockchain_types::v1::Seeds(Vec::new()),
                    fee: Nicks(7),
                }),
            )]),
        };
        assert_ne!(
            raw_tx.id,
            raw_tx.compute_id().expect("fixture raw tx should hash")
        );

        let canonical =
            canonicalize_raw_tx_for_submission(raw_tx).expect("canonicalize raw tx for submit");

        assert_eq!(
            canonical.id,
            canonical
                .compute_id()
                .expect("canonical raw tx should hash")
        );
    }
}
