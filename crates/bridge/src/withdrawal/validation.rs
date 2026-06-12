use nockchain_types::v1::{
    FirstName, InputMetadata, Lock, LockMetadata, Name, NoteData, NoteDataValue, OutputLockMap,
    Spend, Transaction, TransactionV1, VersionedLockMetadata, WitnessData,
};

use crate::shared::errors::BridgeError;
use crate::shared::types::Tip5Hash;
use crate::withdrawal::types::{normalized_note_names, WithdrawalProposalData};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithdrawalTransactionBodyValidator {
    bridge_lock_root: Tip5Hash,
}

impl WithdrawalTransactionBodyValidator {
    pub fn new(bridge_lock_root: Tip5Hash) -> Self {
        Self { bridge_lock_root }
    }

    pub fn bridge_lock_root(&self) -> &Tip5Hash {
        &self.bridge_lock_root
    }

    pub fn validate(&self, proposal: &WithdrawalProposalData) -> Result<(), BridgeError> {
        validate_withdrawal_transaction_body(proposal, &self.bridge_lock_root)
    }
}

pub fn validate_withdrawal_transaction_body(
    proposal: &WithdrawalProposalData,
    bridge_lock_root: &Tip5Hash,
) -> Result<(), BridgeError> {
    let Transaction::V1(tx) = &proposal.transaction;
    validate_transaction_inputs(proposal, tx, bridge_lock_root)?;
    validate_transaction_outputs(proposal, tx, bridge_lock_root)?;
    Ok(())
}

fn validate_transaction_inputs(
    proposal: &WithdrawalProposalData,
    tx: &TransactionV1,
    bridge_lock_root: &Tip5Hash,
) -> Result<(), BridgeError> {
    let selected_inputs = normalized_note_names(&proposal.selected_inputs);
    if selected_inputs.len() != proposal.selected_inputs.len() {
        return Err(invalid_body(
            proposal, "selected input list contains duplicates",
        ));
    }
    let spend_inputs = normalized_note_names(
        &tx.spends
            .0
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>(),
    );
    if spend_inputs.len() != tx.spends.0.len() {
        return Err(invalid_body(
            proposal, "transaction spends contain duplicate inputs",
        ));
    }
    if selected_inputs != spend_inputs {
        return Err(invalid_body(
            proposal, "selected inputs do not match transaction spend inputs",
        ));
    }

    let spend_conditions = match &tx.metadata.inputs {
        InputMetadata::SpendConditions(spend_conditions) => spend_conditions,
        InputMetadata::LegacySignatures(_) => {
            return Err(invalid_body(
                proposal, "withdrawal transaction uses legacy signature input metadata",
            ));
        }
    };
    validate_input_name_map(
        proposal,
        selected_inputs.as_slice(),
        &spend_conditions.0,
        "spend-condition metadata",
    )?;

    let witness_map = match &tx.witness_data {
        WitnessData::Witnesses(witness_map) => witness_map,
        WitnessData::Signatures(_) => {
            return Err(invalid_body(
                proposal, "withdrawal transaction uses legacy signature witness data",
            ));
        }
    };
    validate_input_name_map(
        proposal,
        selected_inputs.as_slice(),
        &witness_map.0,
        "witness data",
    )?;

    for (input_name, spend) in &tx.spends.0 {
        let spend_condition = find_named_entry(&spend_conditions.0, input_name)
            .ok_or_else(|| invalid_body(proposal, "missing spend-condition metadata"))?;
        let witness = find_named_entry(&witness_map.0, input_name)
            .ok_or_else(|| invalid_body(proposal, "missing witness data"))?;
        let Spend::Witness(spend) = spend else {
            return Err(invalid_body(
                proposal, "withdrawal transaction contains a legacy spend",
            ));
        };

        if spend.witness.lock_merkle_proof.spend_condition() != spend_condition {
            return Err(invalid_body(
                proposal, "spend witness proof spend condition does not match input metadata",
            ));
        }
        if witness.lock_merkle_proof.spend_condition() != spend_condition {
            return Err(invalid_body(
                proposal, "witness data proof spend condition does not match input metadata",
            ));
        }
        if &spend.witness.lock_merkle_proof.proof().root != bridge_lock_root {
            return Err(invalid_body(
                proposal, "spend witness proof root is not the bridge lock root",
            ));
        }
        if &witness.lock_merkle_proof.proof().root != bridge_lock_root {
            return Err(invalid_body(
                proposal, "witness data proof root is not the bridge lock root",
            ));
        }
    }

    Ok(())
}

fn validate_input_name_map<T>(
    proposal: &WithdrawalProposalData,
    selected_inputs: &[Name],
    entries: &[(Name, T)],
    label: &'static str,
) -> Result<(), BridgeError> {
    let entry_names = normalized_note_names(
        &entries
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>(),
    );
    if entry_names.len() != entries.len() {
        return Err(invalid_body(
            proposal,
            format!("{label} contains duplicate inputs"),
        ));
    }
    if entry_names != selected_inputs {
        return Err(invalid_body(
            proposal,
            format!("{label} inputs do not match transaction spends"),
        ));
    }
    Ok(())
}

fn validate_transaction_outputs(
    proposal: &WithdrawalProposalData,
    tx: &TransactionV1,
    bridge_lock_root: &Tip5Hash,
) -> Result<(), BridgeError> {
    validate_output_metadata(proposal, &tx.metadata.outputs, bridge_lock_root)?;

    let mut withdrawal_seed_count = 0_u64;
    let mut withdrawal_seed_amount = 0_u64;
    let mut total_fee = 0_u64;
    for (_, spend) in &tx.spends.0 {
        let Spend::Witness(spend) = spend else {
            return Err(invalid_body(
                proposal, "withdrawal transaction contains a legacy spend",
            ));
        };
        total_fee = total_fee
            .checked_add(u64::try_from(spend.fee.0).map_err(|err| {
                BridgeError::ValueConversion(format!("withdrawal spend fee overflow: {err}"))
            })?)
            .ok_or_else(|| invalid_body(proposal, "withdrawal spend fee sum overflowed"))?;

        for seed in &spend.seeds.0 {
            if seed.output_source.is_some() {
                return Err(invalid_body(
                    proposal, "withdrawal transaction seed uses an output source",
                ));
            }

            if seed.lock_root == proposal.recipient {
                validate_withdrawal_seed_note_data(proposal, &seed.note_data)?;
                validate_seed_output_metadata(
                    proposal,
                    &tx.metadata.outputs,
                    &seed.lock_root,
                    ExpectedOutputKind::Withdrawal,
                    bridge_lock_root,
                )?;
                withdrawal_seed_count = withdrawal_seed_count.checked_add(1).ok_or_else(|| {
                    invalid_body(proposal, "withdrawal recipient seed count overflowed")
                })?;
                withdrawal_seed_amount = withdrawal_seed_amount
                    .checked_add(u64::try_from(seed.gift.0).map_err(|err| {
                        BridgeError::ValueConversion(format!(
                            "withdrawal recipient seed amount overflow: {err}"
                        ))
                    })?)
                    .ok_or_else(|| {
                        invalid_body(proposal, "withdrawal recipient seed amount overflowed")
                    })?;
            } else if &seed.lock_root == bridge_lock_root {
                validate_refund_seed_note_data(proposal, &seed.note_data, bridge_lock_root)?;
                validate_seed_output_metadata(
                    proposal,
                    &tx.metadata.outputs,
                    &seed.lock_root,
                    ExpectedOutputKind::Refund,
                    bridge_lock_root,
                )?;
            } else {
                return Err(invalid_body(
                    proposal,
                    "withdrawal transaction creates an output not addressed to the recipient or bridge refund lock",
                ));
            }
        }
    }

    if withdrawal_seed_count != 1 {
        return Err(invalid_body(
            proposal,
            format!(
                "withdrawal transaction must create exactly one recipient output, found {withdrawal_seed_count}"
            ),
        ));
    }
    if withdrawal_seed_amount != proposal.amount {
        return Err(invalid_body(
            proposal,
            format!(
                "withdrawal recipient output amount {withdrawal_seed_amount} does not match proposal amount {}",
                proposal.amount
            ),
        ));
    }
    let spent_to_recipient_and_miner = proposal
        .amount
        .checked_add(total_fee)
        .ok_or_else(|| invalid_body(proposal, "withdrawal amount plus fee overflowed"))?;
    if spent_to_recipient_and_miner > proposal.burned_amount {
        return Err(invalid_body(
            proposal, "withdrawal recipient amount plus transaction fee exceeds burned amount",
        ));
    }

    Ok(())
}

fn validate_output_metadata(
    proposal: &WithdrawalProposalData,
    outputs: &OutputLockMap,
    bridge_lock_root: &Tip5Hash,
) -> Result<(), BridgeError> {
    let mut withdrawal_metadata_count = 0_u64;
    for (output_name, metadata) in &outputs.0 {
        match metadata {
            LockMetadata::Versioned(VersionedLockMetadata::BridgeWithdrawal {
                root,
                beid,
                base_hash,
                base_batch_end,
            }) => {
                if root != &proposal.recipient
                    || beid != &proposal.id.base_event_id.to_belt_digits()
                    || base_hash != &proposal.id.as_of
                    || base_batch_end != &proposal.base_batch_end
                {
                    return Err(invalid_body(
                        proposal, "bridge-withdrawal output metadata does not match proposal facts",
                    ));
                }
                validate_output_first_name(proposal, output_name, root)?;
                withdrawal_metadata_count =
                    withdrawal_metadata_count.checked_add(1).ok_or_else(|| {
                        invalid_body(proposal, "withdrawal output metadata count overflowed")
                    })?;
            }
            LockMetadata::Legacy(metadata) => validate_refund_output_lock(
                proposal, output_name, &metadata.lock, bridge_lock_root,
            )?,
            LockMetadata::Versioned(VersionedLockMetadata::Lock { lock, .. }) => {
                validate_refund_output_lock(proposal, output_name, lock, bridge_lock_root)?
            }
            LockMetadata::Versioned(VersionedLockMetadata::LockRoot(root)) => {
                if root != bridge_lock_root {
                    return Err(invalid_body(
                        proposal, "lock-root output metadata is not the bridge refund lock",
                    ));
                }
                validate_output_first_name(proposal, output_name, root)?;
            }
            LockMetadata::Versioned(VersionedLockMetadata::BridgeDeposit { .. }) => {
                return Err(invalid_body(
                    proposal, "withdrawal transaction contains bridge-deposit output metadata",
                ));
            }
        }
    }

    if withdrawal_metadata_count != 1 {
        return Err(invalid_body(
            proposal,
            format!(
                "withdrawal transaction must contain exactly one bridge-withdrawal output metadata entry, found {withdrawal_metadata_count}"
            ),
        ));
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectedOutputKind {
    Withdrawal,
    Refund,
}

fn validate_seed_output_metadata(
    proposal: &WithdrawalProposalData,
    outputs: &OutputLockMap,
    lock_root: &Tip5Hash,
    expected_kind: ExpectedOutputKind,
    bridge_lock_root: &Tip5Hash,
) -> Result<(), BridgeError> {
    let first_name = first_name_hash(proposal, lock_root)?;
    let Some(metadata) = outputs
        .0
        .iter()
        .find(|(candidate, _)| candidate == &first_name)
        .map(|(_, metadata)| metadata)
    else {
        return Err(invalid_body(
            proposal, "seed output has no matching output metadata",
        ));
    };

    match (expected_kind, metadata) {
        (
            ExpectedOutputKind::Withdrawal,
            LockMetadata::Versioned(VersionedLockMetadata::BridgeWithdrawal { root, .. }),
        ) if root == lock_root => Ok(()),
        (ExpectedOutputKind::Refund, LockMetadata::Legacy(metadata)) => {
            validate_refund_lock_root(proposal, &metadata.lock, bridge_lock_root)
        }
        (
            ExpectedOutputKind::Refund,
            LockMetadata::Versioned(VersionedLockMetadata::Lock { lock, .. }),
        ) => validate_refund_lock_root(proposal, lock, bridge_lock_root),
        (
            ExpectedOutputKind::Refund,
            LockMetadata::Versioned(VersionedLockMetadata::LockRoot(root)),
        ) if root == bridge_lock_root => Ok(()),
        _ => Err(invalid_body(
            proposal, "seed output metadata has the wrong withdrawal output kind",
        )),
    }
}

fn validate_withdrawal_seed_note_data(
    proposal: &WithdrawalProposalData,
    note_data: &NoteData,
) -> Result<(), BridgeError> {
    let mut matching_entries = 0_u64;
    for entry in note_data.iter() {
        if let NoteDataValue::BridgeWithdrawal(bridge) = &entry.value {
            if bridge.beid != proposal.id.base_event_id.to_belt_digits()
                || bridge.base_hash != proposal.id.as_of
                || bridge.lock_root != proposal.recipient
                || bridge.base_batch_end != proposal.base_batch_end
            {
                return Err(invalid_body(
                    proposal, "recipient bridge-withdrawal note data does not match proposal facts",
                ));
            }
            matching_entries = matching_entries.checked_add(1).ok_or_else(|| {
                invalid_body(proposal, "withdrawal note-data entry count overflowed")
            })?;
        }
    }

    if matching_entries != 1 {
        return Err(invalid_body(
            proposal,
            format!(
                "recipient output must contain exactly one bridge-withdrawal note-data entry, found {matching_entries}"
            ),
        ));
    }
    Ok(())
}

fn validate_refund_seed_note_data(
    proposal: &WithdrawalProposalData,
    note_data: &NoteData,
    bridge_lock_root: &Tip5Hash,
) -> Result<(), BridgeError> {
    let mut has_bridge_lock = false;
    for entry in note_data.iter() {
        match &entry.value {
            NoteDataValue::Lock { lock } => {
                if lock.as_ref().hash().map_err(|err| {
                    BridgeError::Runtime(format!(
                        "failed to hash withdrawal refund output lock: {err}"
                    ))
                })? == *bridge_lock_root
                {
                    has_bridge_lock = true;
                }
            }
            NoteDataValue::BridgeWithdrawal(_) => {
                return Err(invalid_body(
                    proposal, "refund output unexpectedly contains bridge-withdrawal note data",
                ));
            }
            NoteDataValue::BridgeDeposit(_) | NoteDataValue::Noun(_) => {}
        }
    }
    if !has_bridge_lock {
        return Err(invalid_body(
            proposal, "refund output note data does not contain the bridge refund lock",
        ));
    }
    Ok(())
}

fn validate_refund_output_lock(
    proposal: &WithdrawalProposalData,
    output_name: &Tip5Hash,
    lock: &Lock,
    bridge_lock_root: &Tip5Hash,
) -> Result<(), BridgeError> {
    validate_refund_lock_root(proposal, lock, bridge_lock_root)?;
    validate_output_first_name(proposal, output_name, bridge_lock_root)
}

fn validate_refund_lock_root(
    proposal: &WithdrawalProposalData,
    lock: &Lock,
    bridge_lock_root: &Tip5Hash,
) -> Result<(), BridgeError> {
    let root = lock.hash().map_err(|err| {
        BridgeError::Runtime(format!(
            "failed to hash withdrawal refund output lock: {err}"
        ))
    })?;
    if &root != bridge_lock_root {
        return Err(invalid_body(
            proposal, "refund output lock does not hash to the bridge lock root",
        ));
    }
    Ok(())
}

fn validate_output_first_name(
    proposal: &WithdrawalProposalData,
    output_name: &Tip5Hash,
    lock_root: &Tip5Hash,
) -> Result<(), BridgeError> {
    let expected = first_name_hash(proposal, lock_root)?;
    if output_name != &expected {
        return Err(invalid_body(
            proposal, "output metadata first-name does not match its lock root",
        ));
    }
    Ok(())
}

fn first_name_hash(
    proposal: &WithdrawalProposalData,
    lock_root: &Tip5Hash,
) -> Result<Tip5Hash, BridgeError> {
    FirstName::from_lock_root(lock_root)
        .map(|first_name| first_name.into_hash())
        .map_err(|err| {
            invalid_body(
                proposal,
                format!("failed to derive output first-name from lock root: {err}"),
            )
        })
}

fn find_named_entry<'a, T>(entries: &'a [(Name, T)], name: &Name) -> Option<&'a T> {
    entries
        .iter()
        .find(|(candidate, _)| candidate == name)
        .map(|(_, value)| value)
}

fn invalid_body(proposal: &WithdrawalProposalData, reason: impl Into<String>) -> BridgeError {
    BridgeError::Runtime(format!(
        "invalid withdrawal transaction body for {:?} epoch {}: {}",
        proposal.id,
        proposal.epoch,
        reason.into()
    ))
}

#[cfg(test)]
mod tests {
    use nockchain_math::belt::Belt;
    use nockchain_types::v1::{
        LockMerkleProof, LockPrimitive, MerkleProof, Nicks, NoteDataEntry, Pkh, PkhSignature, Seed,
        Seeds, Spend1, SpendCondition, SpendConditionMap, TransactionMetadata, Witness, WitnessMap,
    };

    use super::*;
    use crate::shared::types::AtomBytes;
    use crate::withdrawal::types::{WithdrawalId, WithdrawalSnapshot};

    fn hash(seed: u64) -> Tip5Hash {
        Tip5Hash([Belt(seed), Belt(seed + 1), Belt(seed + 2), Belt(seed + 3), Belt(seed + 4)])
    }

    fn base_event_id() -> AtomBytes {
        AtomBytes((1_u8..=32).collect())
    }

    fn bridge_spend_condition() -> SpendCondition {
        SpendCondition::new(vec![LockPrimitive::Pkh(Pkh::new(1, vec![hash(100)]))])
    }

    fn sample_proposal() -> (WithdrawalProposalData, Tip5Hash) {
        let bridge_condition = bridge_spend_condition();
        let bridge_lock = Lock::SpendCondition(bridge_condition.clone());
        let bridge_lock_root = bridge_lock.hash().expect("bridge lock root");
        let recipient = hash(200);
        let id = WithdrawalId {
            as_of: hash(300),
            base_event_id: base_event_id(),
        };
        let base_batch_end = 77;
        let amount = 1_234;
        let input_name = Name::new(hash(400), hash(500));
        let witness = Witness::new(
            LockMerkleProof::new_stub(
                bridge_condition.clone(),
                2,
                MerkleProof {
                    root: bridge_lock_root.clone(),
                    path: Vec::new(),
                },
            ),
            PkhSignature::new(Vec::new()),
            Vec::new(),
        );
        let withdrawal_seed = Seed {
            output_source: None,
            lock_root: recipient.clone(),
            note_data: NoteData::new(vec![NoteDataEntry::bridge_withdrawal(
                id.base_event_id.to_belt_digits(),
                id.as_of.clone(),
                recipient.clone(),
                base_batch_end,
            )]),
            gift: Nicks(amount as usize),
            parent_hash: hash(600),
        };
        let refund_seed = Seed {
            output_source: None,
            lock_root: bridge_lock_root.clone(),
            note_data: NoteData::new(vec![NoteDataEntry::lock(bridge_lock.clone())]),
            gift: Nicks(5),
            parent_hash: hash(601),
        };
        let spend = Spend::Witness(Spend1 {
            witness: witness.clone(),
            seeds: Seeds(vec![withdrawal_seed, refund_seed]),
            fee: Nicks(3),
        });
        let recipient_first_name = FirstName::from_lock_root(&recipient)
            .expect("recipient first name")
            .into_hash();
        let bridge_first_name = FirstName::from_lock_root(&bridge_lock_root)
            .expect("bridge first name")
            .into_hash();
        let transaction = Transaction::V1(TransactionV1 {
            name: "sample-withdrawal".to_string(),
            spends: nockchain_types::v1::Spends(vec![(input_name.clone(), spend)]),
            metadata: TransactionMetadata {
                inputs: InputMetadata::SpendConditions(SpendConditionMap(vec![(
                    input_name.clone(),
                    bridge_condition,
                )])),
                outputs: OutputLockMap(vec![
                    (
                        recipient_first_name,
                        LockMetadata::Versioned(VersionedLockMetadata::BridgeWithdrawal {
                            root: recipient.clone(),
                            beid: id.base_event_id.to_belt_digits(),
                            base_hash: id.as_of.clone(),
                            base_batch_end,
                        }),
                    ),
                    (
                        bridge_first_name,
                        LockMetadata::Versioned(VersionedLockMetadata::Lock {
                            lock: bridge_lock,
                            include_data: false,
                        }),
                    ),
                ]),
            },
            witness_data: WitnessData::Witnesses(WitnessMap(vec![(input_name.clone(), witness)])),
        });

        (
            WithdrawalProposalData {
                id,
                recipient,
                amount,
                burned_amount: amount + 100,
                base_batch_end,
                epoch: 0,
                snapshot: WithdrawalSnapshot {
                    height: 10,
                    block_id: hash(700),
                },
                selected_inputs: vec![input_name],
                transaction,
            },
            bridge_lock_root,
        )
    }

    #[test]
    fn validate_withdrawal_transaction_body_accepts_matching_body() {
        let (proposal, bridge_lock_root) = sample_proposal();

        validate_withdrawal_transaction_body(&proposal, &bridge_lock_root)
            .expect("matching body should validate");
    }

    #[test]
    fn validate_withdrawal_transaction_body_rejects_wrong_recipient_output() {
        let (mut proposal, bridge_lock_root) = sample_proposal();
        let Transaction::V1(tx) = &mut proposal.transaction;
        let (_, Spend::Witness(spend)) = tx.spends.0.first_mut().expect("spend") else {
            panic!("expected witness spend");
        };
        spend.seeds.0[0].lock_root = hash(999);

        let err = validate_withdrawal_transaction_body(&proposal, &bridge_lock_root)
            .expect_err("wrong recipient output should fail");

        assert!(
            err.to_string().contains("not addressed to the recipient"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn proposal_hash_commits_to_stable_transaction_body() {
        let (mut proposal, _) = sample_proposal();
        let original_hash = proposal.proposal_hash().expect("original hash");

        let Transaction::V1(tx) = &mut proposal.transaction;
        let (_, Spend::Witness(spend)) = tx.spends.0.first_mut().expect("spend") else {
            panic!("expected witness spend");
        };
        spend.seeds.0[0].gift = Nicks(2_000);

        assert_ne!(
            original_hash,
            proposal.proposal_hash().expect("mutated hash")
        );
    }

    #[test]
    fn proposal_hash_ignores_witness_data_contributions() {
        let (mut proposal, _) = sample_proposal();
        let original_hash = proposal.proposal_hash().expect("original hash");

        let Transaction::V1(tx) = &mut proposal.transaction;
        tx.witness_data = WitnessData::Witnesses(WitnessMap(Vec::new()));

        assert_eq!(
            original_hash,
            proposal.proposal_hash().expect("witness-only hash")
        );
    }
}
