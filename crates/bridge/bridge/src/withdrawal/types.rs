use nockapp::noun::slab::{NockJammer, NounSlab};
use nockchain_types::tx_engine::common::Hash as Tip5Hash;
use nockchain_types::v1::Name;
use noun_serde::{NounDecode, NounEncode};

use crate::shared::errors::BridgeError;
use crate::shared::types::{AtomBytes, BaseEventId};

/// Full kernel/proposal withdrawal reference.
///
/// Sequencer registration and ordering are keyed by `base_event_id` only. The
/// `as_of` hash remains part of canonical proposal and Hoon settlement
/// semantics, so the full id is still validated after proposal hydration.
#[derive(Debug, Clone, PartialEq, Eq, Hash, NounEncode, NounDecode)]
pub struct WithdrawalId {
    pub as_of: Tip5Hash,
    pub base_event_id: BaseEventId,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct WithdrawalSnapshot {
    pub height: u64,
    pub block_id: Tip5Hash,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub struct WithdrawalProposalData {
    pub id: WithdrawalId,
    pub recipient: Tip5Hash,
    pub amount: u64,
    pub burned_amount: u64,
    pub base_batch_end: u64,
    pub epoch: u64,
    pub snapshot: WithdrawalSnapshot,
    pub selected_inputs: Vec<Name>,
    pub transaction: nockchain_types::v1::Transaction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WithdrawalSequencerProposalArtifacts {
    pub id: WithdrawalId,
    pub epoch: u64,
    pub proposal_hash: String,
    pub amount: u64,
    pub base_batch_end: u64,
    pub snapshot: WithdrawalSnapshot,
    pub selected_inputs: Vec<Name>,
    pub transaction: nockchain_types::v1::Transaction,
    pub commit_certificate: Option<Vec<u8>>,
    pub authorized_transaction_name: Option<String>,
    pub authorized_transaction_jam: Option<Vec<u8>>,
    pub authorized_raw_tx: Option<Vec<u8>>,
}

pub(crate) fn normalized_note_names(inputs: &[Name]) -> Vec<Name> {
    fn note_name_sort_key(name: &Name) -> ([u8; 40], [u8; 40]) {
        (name.first.to_be_limb_bytes(), name.last.to_be_limb_bytes())
    }

    let mut normalized = inputs.to_vec();
    normalized.sort_by_key(note_name_sort_key);
    normalized.dedup_by(|left, right| left == right);
    normalized
}

impl WithdrawalProposalData {
    pub fn proposal_hash(&self) -> Result<String, BridgeError> {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"withdrawal-proposal-v1");
        hasher.update(&self.id.as_of.to_be_limb_bytes());
        hash_len_prefixed(&mut hasher, &self.id.base_event_id.0);
        hasher.update(&self.recipient.to_be_limb_bytes());
        hasher.update(&self.amount.to_be_bytes());
        hasher.update(&self.burned_amount.to_be_bytes());
        hasher.update(&self.base_batch_end.to_be_bytes());
        hasher.update(&self.epoch.to_be_bytes());
        hasher.update(&self.snapshot.height.to_be_bytes());
        hasher.update(&self.snapshot.block_id.to_be_limb_bytes());
        let selected_inputs = normalized_note_names(&self.selected_inputs);
        hasher.update(
            &u64::try_from(selected_inputs.len())
                .map_err(|err| {
                    BridgeError::ValueConversion(format!(
                        "selected input count too large for proposal hash: {err}"
                    ))
                })?
                .to_be_bytes(),
        );
        for input in &selected_inputs {
            hasher.update(&input.first.to_be_limb_bytes());
            hasher.update(&input.last.to_be_limb_bytes());
        }
        let nockchain_types::v1::Transaction::V1(tx) = &self.transaction;
        hash_len_prefixed(&mut hasher, tx.name.as_bytes());
        hash_noun_encoded(&mut hasher, &tx.spends);
        hash_noun_encoded(&mut hasher, &tx.metadata);
        Ok(hasher.finalize().to_hex().to_string())
    }
}

fn hash_len_prefixed(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn hash_noun_encoded<T: NounEncode>(hasher: &mut blake3::Hasher, value: &T) {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let noun = value.to_noun(&mut slab);
    slab.set_root(noun);
    hash_len_prefixed(hasher, &slab.jam());
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct SelectedWithdrawalNoteData {
    pub name: Name,
    pub note: nockchain_types::v1::Note,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct CreateWithdrawalTxData {
    pub id: WithdrawalId,
    pub recipient: Tip5Hash,
    pub amount: u64,
    pub burned_amount: u64,
    pub base_batch_end: u64,
    pub epoch: u64,
    pub snapshot: WithdrawalSnapshot,
    pub fee: u64,
    pub selected_notes: Vec<SelectedWithdrawalNoteData>,
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct BaseWithdrawalEntry {
    pub base_tx_id: AtomBytes,
    pub withdrawal: Withdrawal,
}

#[derive(Debug, Clone, NounEncode, NounDecode)]
pub struct Withdrawal {
    pub base_tx_id: AtomBytes,
    pub dest: Option<Tip5Hash>,
    pub raw_amount: u64,
}

/// Nock withdrawal request as emitted by the kernel.
///
/// Matches the Hoon type:
/// `[=base-event-id recipient=nock-lock-root amount=@ base-batch-end=@ as-of=base-hash]`
#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct NockWithdrawalRequestKernelData {
    pub base_event_id: BaseEventId,
    pub recipient: Tip5Hash,
    pub amount: u64,
    pub base_batch_end: u64,
    pub as_of: Tip5Hash,
}

impl NockWithdrawalRequestKernelData {
    pub fn withdrawal_id(&self) -> WithdrawalId {
        WithdrawalId {
            as_of: self.as_of.clone(),
            base_event_id: self.base_event_id.clone(),
        }
    }
}
