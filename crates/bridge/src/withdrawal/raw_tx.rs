use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::{Bytes, NounAllocator};
use nockchain_types::tx_engine::common::{Hash, Version};
use nockchain_types::v1::{hash_leaf_atom, hash_pair, HashHashable};
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
                let id = raw_tx_id_from_version_and_spends(&version, &spends)?;
                let tx_id_base58 = id.to_base58();
                Ok(Self {
                    tx_id_base58,
                    raw_tx: nockchain_types::v1::RawTx {
                        version,
                        id,
                        spends,
                    },
                })
            }
        }
    }
}

fn raw_tx_id_from_version_and_spends(
    version: &Version,
    spends: &nockchain_types::v1::Spends,
) -> Result<Hash, BridgeError> {
    if version != &Version::V1 {
        return Err(BridgeError::Runtime(format!(
            "withdrawal raw tx id hashing only supports v1 transactions, got {version:?}"
        )));
    }
    let version = hash_leaf_atom(u32::from(version.clone()) as u64)
        .map_err(|err| BridgeError::Runtime(format!("failed to hash raw tx version: {err}")))?;
    let spends = spends
        .hash_digest()
        .map_err(|err| BridgeError::Runtime(format!("failed to hash raw tx spends: {err}")))?;
    Ok(hash_pair(&version, &spends))
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
