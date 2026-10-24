use std::collections::BTreeMap;

use bytes::Bytes;
#[cfg(test)]
use nockapp::noun::slab::{NockJammer, NounSlab};
#[cfg(test)]
use nockchain_math::belt::Belt;
#[cfg(test)]
use nockchain_math::crypto::cheetah::A_GEN;
use nockchain_types::tx_engine::common::BlockHeight;
#[cfg(test)]
use nockchain_types::tx_engine::common::{Hash, SchnorrPubkey, SchnorrSignature};
use nockchain_types::tx_engine::v1::tx::{LockPrimitive, SpendCondition};
#[cfg(test)]
use nockchain_types::tx_engine::v1::tx::{PkhSignature, PkhSignatureEntry};
#[cfg(test)]
use nockvm::noun::{Noun, NounAllocator, NounSpace};
#[cfg(test)]
use noun_serde::NounEncode;

use crate::note_data::{DecodedNoteDataEntry, DecodedNoteDataPayload, LockDataPayload};
use crate::types::{ChainContext, PlannedOutput, RawNoteDataEntry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitnessWordInput {
    pub spend_condition: SpendCondition,
    pub input_origin_page: BlockHeight,
    // Optional lock-level hint used for lock-merkle-path estimation.
    // When absent we conservatively assume a single spend-condition lock.
    pub spend_condition_count: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
pub struct WordCountEstimator<'a> {
    chain_context: &'a ChainContext,
}

impl<'a> WordCountEstimator<'a> {
    pub fn new(chain_context: &'a ChainContext) -> Self {
        Self { chain_context }
    }

    pub fn estimate_seed_words(&self, outputs: &[PlannedOutput]) -> u64 {
        if (self.chain_context.height.0).0 >= (self.chain_context.bythos_phase.0).0 {
            Self::estimate_seed_words_merged(outputs)
        } else {
            Self::estimate_seed_words_legacy(outputs)
        }
    }

    pub fn estimate_seed_words_legacy(outputs: &[PlannedOutput]) -> u64 {
        outputs
            .iter()
            .map(|output| Self::estimate_note_data_words(&output.note_data))
            .sum()
    }

    pub fn estimate_seed_words_merged(outputs: &[PlannedOutput]) -> u64 {
        let mut merged_by_lock_root = BTreeMap::<[u64; 5], BTreeMap<String, Bytes>>::new();
        for output in outputs {
            let entry = merged_by_lock_root
                .entry(output.lock_root.to_array())
                .or_default();
            for note_data_entry in &output.note_data {
                entry.insert(note_data_entry.key.clone(), note_data_entry.blob.clone());
            }
        }

        merged_by_lock_root
            .into_values()
            .map(|merged| {
                let entries = merged
                    .into_iter()
                    .map(|(key, blob)| RawNoteDataEntry { key, blob })
                    .collect::<Vec<_>>();
                Self::estimate_note_data_words(&entries)
            })
            .sum()
    }

    pub fn estimate_witness_words(&self, inputs: &[WitnessWordInput]) -> u64 {
        inputs
            .iter()
            .map(|input| self.estimate_witness_words_for_input(input))
            .sum()
    }

    pub fn estimate_witness_words_for_input(&self, input: &WitnessWordInput) -> u64 {
        let bythos_active = (input.input_origin_page.0).0 >= (self.chain_context.bythos_phase.0).0;
        Self::witness_words_for_lock(
            &input.spend_condition, bythos_active, input.spend_condition_count,
        )
    }

    pub fn estimate_v0_witness_words(&self, signatures_required: u64) -> u64 {
        // Legacy spend-0 witnesses only carry the signature map.
        // 13 words for the schnorr pubkey,
        // 16 words for the schnorr signature consisting of 2 8-tuples
        Self::map_words(signatures_required, 13, 16)
    }

    fn witness_words_for_lock(
        spend_condition: &SpendCondition,
        bythos_active: bool,
        spend_condition_count: Option<u64>,
    ) -> u64 {
        let lmp_words =
            Self::estimate_lmp_words(spend_condition, bythos_active, spend_condition_count);
        let pkh_words = Self::estimate_pkh_signature_words(spend_condition);
        let tim_words = 1;
        let hax_words = 1;
        lmp_words
            .saturating_add(pkh_words)
            .saturating_add(tim_words)
            .saturating_add(hax_words)
    }

    fn normalize_spend_condition_count(raw_count: u64) -> u64 {
        if raw_count <= 1 {
            return 1;
        }
        if raw_count.is_power_of_two() {
            return raw_count;
        }
        raw_count
            .checked_next_power_of_two()
            .unwrap_or(1_u64 << (u64::BITS - 1))
    }

    fn estimate_lmp_words(
        spend_condition: &SpendCondition,
        bythos_active: bool,
        spend_condition_count: Option<u64>,
    ) -> u64 {
        let spend_condition_words = Self::estimate_spend_condition_words(spend_condition);
        let axis_words = 1;
        let merkle_proof_words = Self::estimate_merkle_proof_words(spend_condition_count);
        let version_words = if bythos_active { 1 } else { 0 };
        spend_condition_words
            .saturating_add(axis_words)
            .saturating_add(merkle_proof_words)
            .saturating_add(version_words)
    }

    fn estimate_merkle_proof_words(spend_condition_count: Option<u64>) -> u64 {
        let count = Self::normalize_spend_condition_count(spend_condition_count.unwrap_or(1));
        let path_len = count.ilog2() as u64;
        let root_words = 5_u64;
        let path_words = Self::estimate_list_words_len(path_len, 5);
        root_words.saturating_add(path_words)
    }

    fn estimate_pkh_signature_words(spend_condition: &SpendCondition) -> u64 {
        let num_sigs_required = spend_condition
            .iter()
            .map(|primitive| match primitive {
                LockPrimitive::Pkh(pkh) => pkh.m,
                _ => 0,
            })
            .sum::<u64>();
        let hash_words = 5_u64;
        let pubkey_words = 13_u64;
        let signature_words = 16_u64;
        Self::map_words(
            num_sigs_required,
            hash_words,
            pubkey_words.saturating_add(signature_words),
        )
    }

    #[cfg(test)]
    fn count_encoded_leaves<T: NounEncode>(value: &T) -> u64 {
        let mut slab = NounSlab::<NockJammer>::new();
        let noun = value.to_noun(&mut slab);
        let space = slab.noun_space();
        Self::count_noun_leaves(noun, &space)
    }

    #[cfg(test)]
    fn count_noun_leaves(noun: Noun, space: &NounSpace) -> u64 {
        if noun.is_atom() {
            1
        } else {
            let cell = noun
                .in_space(space)
                .as_cell()
                .expect("non-atom noun should always be a cell");
            Self::count_noun_leaves(cell.head().noun(), space)
                .saturating_add(Self::count_noun_leaves(cell.tail().noun(), space))
        }
    }

    #[cfg(test)]
    fn synthetic_pkh_signature(spend_condition: &SpendCondition) -> PkhSignature {
        let num_sigs_required = spend_condition
            .iter()
            .map(|primitive| match primitive {
                LockPrimitive::Pkh(pkh) => pkh.m,
                _ => 0,
            })
            .sum::<u64>();
        PkhSignature(
            (0..num_sigs_required)
                .map(|index| PkhSignatureEntry {
                    pkh: Self::dummy_hash(index),
                    pubkey: SchnorrPubkey(A_GEN),
                    signature: SchnorrSignature {
                        chal: [Belt(0); 8],
                        sig: [Belt(0); 8],
                    },
                })
                .collect(),
        )
    }

    #[cfg(test)]
    fn dummy_hash(seed: u64) -> Hash {
        Hash::from_limbs(&[seed, 0, 0, 0, 0])
    }

    fn estimate_note_data_words(entries: &[RawNoteDataEntry]) -> u64 {
        if entries.is_empty() {
            return 1;
        }
        // Hoon fee counting first reduces note-data z-maps with `rep z-by`, which turns the map
        // into a list of key/value pairs. The charged shape is therefore the flattened entry list,
        // not the raw tree with null branches.
        let kv_words = entries
            .iter()
            .map(|entry| {
                let key_words = 1; // @tas key atom
                let value_words = Self::estimate_note_data_value_words(entry);
                key_words + value_words
            })
            .sum::<u64>();
        kv_words.saturating_add(1)
    }

    fn estimate_note_data_value_words(entry: &RawNoteDataEntry) -> u64 {
        let decoded = DecodedNoteDataEntry::from_raw_entry(entry);
        match decoded.payload {
            DecodedNoteDataPayload::Lock(LockDataPayload {
                version,
                spend_conditions,
            }) => {
                // [%0 lock]
                let version_words: u64 = if version == 0 { 1 } else { 2 };
                version_words.saturating_add(Self::estimate_lock_words(&spend_conditions))
            }
            DecodedNoteDataPayload::BridgeDeposit(bridge) => {
                // [%0 %base [a b c]]
                let network_words = match bridge.network {
                    crate::note_data::BridgeNetwork::Base => 1,
                };
                1 + network_words + 3
            }
            DecodedNoteDataPayload::BridgeWithdrawal(bridge_w) => {
                // [%0 beid base-hash lock-root base-batch-end]
                let beid_words = Self::estimate_list_words_len(bridge_w.beid.len() as u64, 1);
                1 + beid_words + 5 + 5 + 1
            }
            DecodedNoteDataPayload::Raw => Self::estimate_raw_blob_words(&entry.blob),
        }
    }

    fn estimate_raw_blob_words(blob: &Bytes) -> u64 {
        // Conservative content-size proxy when payload is opaque.
        // 8 bytes per word baseline, always at least 1 leaf.
        let byte_len = blob.len() as u64;
        byte_len.saturating_add(7).saturating_div(8).max(1)
    }

    fn estimate_spend_condition_words(spend_condition: &SpendCondition) -> u64 {
        let primitive_words = spend_condition
            .iter()
            .map(Self::estimate_lock_primitive_words)
            .sum::<u64>();
        // list terminator
        primitive_words + 1
    }

    fn estimate_lock_words(spend_conditions: &[SpendCondition]) -> u64 {
        if spend_conditions.is_empty() {
            return 1;
        }

        let spend_condition_words = spend_conditions
            .iter()
            .map(Self::estimate_spend_condition_words)
            .sum::<u64>();
        if spend_conditions.len() == 1 {
            return spend_condition_words;
        }

        let normalized_len = Self::normalize_spend_condition_count(spend_conditions.len() as u64);
        let branch_nodes = normalized_len.saturating_sub(1);
        // Each branch contributes a lock tag and one branch-pair cell.
        spend_condition_words.saturating_add(branch_nodes.saturating_mul(2))
    }

    fn estimate_lock_primitive_words(primitive: &LockPrimitive) -> u64 {
        match primitive {
            LockPrimitive::Pkh(pkh) => {
                // [%pkh [m hashes]]
                let hash_set_words = Self::estimate_set_words_len(pkh.hashes.len() as u64, 5);
                1 + 1 + hash_set_words
            }
            LockPrimitive::Tim(tim) => {
                // [%tim [rel abs]], each range: [min max], each bound option: None=>1, Some=>2.
                let rel_words = Self::estimate_range_words(
                    tim.rel.min.as_ref().map(|_| 1_u64),
                    tim.rel.max.as_ref().map(|_| 1_u64),
                );
                let abs_words = Self::estimate_range_words(
                    tim.abs.min.as_ref().map(|_| 1_u64),
                    tim.abs.max.as_ref().map(|_| 1_u64),
                );
                1 + rel_words + abs_words
            }
            LockPrimitive::Hax(hax) => {
                // [%hax hashes]
                let hash_set_words = Self::estimate_set_words_len(hax.0.len() as u64, 5);
                1 + hash_set_words
            }
            LockPrimitive::Burn => {
                // [%brn ~]
                1 + 1
            }
        }
    }

    fn estimate_range_words(min: Option<u64>, max: Option<u64>) -> u64 {
        Self::estimate_option_words(min) + Self::estimate_option_words(max)
    }

    fn estimate_option_words(payload_words: Option<u64>) -> u64 {
        match payload_words {
            Some(words) => 1 + words, // [~ value]
            None => 1,                // ~
        }
    }

    fn estimate_set_words_len(entries: u64, key_words: u64) -> u64 {
        // set has key payload + binary-branch null count.
        entries
            .saturating_mul(key_words)
            .saturating_add(entries.saturating_add(1))
    }

    fn map_words(entries: u64, key_leaves: u64, val_leaves: u64) -> u64 {
        let per_node_count = key_leaves.saturating_add(val_leaves);
        entries
            .saturating_mul(per_node_count)
            .saturating_add(entries.saturating_add(1))
    }

    fn estimate_list_words_len(entries: u64, item_words: u64) -> u64 {
        entries.saturating_mul(item_words).saturating_add(1)
    }
}

pub fn estimate_seed_words(outputs: &[PlannedOutput], chain_context: &ChainContext) -> u64 {
    WordCountEstimator::new(chain_context).estimate_seed_words(outputs)
}

pub fn estimate_seed_words_legacy(outputs: &[PlannedOutput]) -> u64 {
    WordCountEstimator::estimate_seed_words_legacy(outputs)
}

pub fn estimate_seed_words_merged(outputs: &[PlannedOutput]) -> u64 {
    WordCountEstimator::estimate_seed_words_merged(outputs)
}

pub fn estimate_witness_words(inputs: &[WitnessWordInput], chain_context: &ChainContext) -> u64 {
    WordCountEstimator::new(chain_context).estimate_witness_words(inputs)
}

#[cfg(test)]
mod tests {
    use nockapp::noun::NounEncodeJamExt;
    use nockapp::utils::NOCK_STACK_SIZE;
    use nockchain_math::belt::Belt;
    use nockchain_types::tx_engine::common::Hash;
    use nockchain_types::tx_engine::v1::note::{
        NoteData, NOTE_DATA_KEY_BRIDGE_WITHDRAWAL, NOTE_DATA_KEY_LOCK,
    };
    use nockchain_types::tx_engine::v1::tx::{
        Lock, LockMerkleProof, LockPrimitive, MerkleProof, Pkh, Spend, Witness,
    };
    use nockchain_types::v1::Transaction;
    use nockvm::ext::NounExt;
    use nockvm::mem::NockStack;
    use nockvm::noun::Noun;
    use noun_serde::NounDecode;

    use super::*;
    use crate::fee::{compute_minimum_fee, FeeInputs};
    use crate::types::RawNoteDataEntry;

    #[derive(Debug, Clone, PartialEq, Eq, NounDecode)]
    struct FixtureEntry {
        case: String,
        note_data: NoteData,
    }

    #[derive(Debug, Clone, PartialEq, NounDecode)]
    struct WithdrawalTxFixtureEntry {
        case: String,
        transaction: Transaction,
        height: u64,
        min_fee: u64,
        seed_words: u64,
        witness_words: u64,
    }

    const BRIDGE_PRE_BYTHOS_STUB_WITH_CHANGE: &str =
        "bridge-multisig-withdrawal-pre-bythos-stub-with-change";
    const BRIDGE_PRE_BYTHOS_STUB_ORIGIN_PAGE: u64 = 7;

    fn hash(v: u64) -> Hash {
        Hash::from_limbs(&[v, 0, 0, 0, 0])
    }

    fn output(lock_root: u64, key: &str, value: u64) -> PlannedOutput {
        PlannedOutput {
            lock_root: hash(lock_root),
            amount: 1,
            note_data: vec![RawNoteDataEntry {
                key: key.to_string(),
                blob: value.jam_bytes(),
            }],
        }
    }

    fn bridge_multisig_spend_condition() -> SpendCondition {
        SpendCondition::new(vec![LockPrimitive::Pkh(Pkh::new(
            3,
            vec![hash(1), hash(2), hash(3), hash(4), hash(5)],
        ))])
    }

    fn empty_merkle_proof() -> MerkleProof {
        MerkleProof {
            root: hash(99),
            path: Vec::new(),
        }
    }

    fn count_witness_words(
        lock_merkle_proof: LockMerkleProof,
        spend_condition: &SpendCondition,
    ) -> u64 {
        WordCountEstimator::count_encoded_leaves(&Witness::new(
            lock_merkle_proof,
            WordCountEstimator::synthetic_pkh_signature(spend_condition),
            Vec::new(),
        ))
    }

    fn encoded_stub_and_full_witness_words() -> (u64, u64) {
        let spend_condition = bridge_multisig_spend_condition();
        let stub_words = count_witness_words(
            LockMerkleProof::new_stub(spend_condition.clone(), 1, empty_merkle_proof()),
            &spend_condition,
        );
        let full_words = count_witness_words(
            LockMerkleProof::new_full(spend_condition.clone(), 1, empty_merkle_proof()),
            &spend_condition,
        );
        (stub_words, full_words)
    }

    fn decode_note_data_fixtures() -> Vec<FixtureEntry> {
        let fixture_bytes = include_bytes!("../tests/fixtures/note_data_fixtures.jam");
        let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
        let noun = Noun::cue_bytes_slice(&mut stack, fixture_bytes).expect("fixture jam must cue");
        let space = stack.noun_space();
        Vec::<FixtureEntry>::from_noun(&noun, &space).expect("fixture noun must decode")
    }

    fn decode_withdrawal_tx_fixtures() -> Vec<WithdrawalTxFixtureEntry> {
        let fixture_bytes = include_bytes!("../tests/fixtures/withdrawal_tx_fixtures.jam");
        let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
        let noun = Noun::cue_bytes_slice(&mut stack, fixture_bytes).expect("fixture jam must cue");
        let space = stack.noun_space();
        Vec::<WithdrawalTxFixtureEntry>::from_noun(&noun, &space).expect("fixture noun must decode")
    }

    fn normalize_case_tag(tag: &str) -> &str {
        tag.strip_prefix('%').unwrap_or(tag)
    }

    fn fixture_note_data(case: &str) -> NoteData {
        decode_note_data_fixtures()
            .into_iter()
            .find(|fixture| normalize_case_tag(&fixture.case) == case)
            .unwrap_or_else(|| panic!("missing fixture case: {case}"))
            .note_data
    }

    fn output_from_fixture(case: &str) -> PlannedOutput {
        let note_data = fixture_note_data(case);
        let note_data = note_data
            .iter()
            .map(|entry| RawNoteDataEntry {
                key: entry.key.clone(),
                blob: entry.raw_blob(),
            })
            .collect::<Vec<_>>();
        PlannedOutput {
            lock_root: hash(99),
            amount: 1,
            note_data,
        }
    }

    fn output_from_fixture_with_lock_root(case: &str, lock_root: u64) -> PlannedOutput {
        let note_data = fixture_note_data(case);
        let note_data = note_data
            .iter()
            .map(|entry| RawNoteDataEntry {
                key: entry.key.clone(),
                blob: entry.raw_blob(),
            })
            .collect::<Vec<_>>();
        PlannedOutput {
            lock_root: hash(lock_root),
            amount: 1,
            note_data,
        }
    }

    fn transaction_fixture(case: &str) -> Transaction {
        decode_withdrawal_tx_fixtures()
            .into_iter()
            .find(|fixture| normalize_case_tag(&fixture.case) == case)
            .unwrap_or_else(|| panic!("missing fixture case: {case}"))
            .transaction
    }

    fn withdrawal_tx_fixture_entry(case: &str) -> WithdrawalTxFixtureEntry {
        decode_withdrawal_tx_fixtures()
            .into_iter()
            .find(|fixture| normalize_case_tag(&fixture.case) == case)
            .unwrap_or_else(|| panic!("missing fixture case: {case}"))
    }

    fn first_witness_spend_condition(
        transaction: Transaction,
    ) -> (LockMerkleProof, SpendCondition) {
        let Transaction::V1(tx) = transaction;
        let (name, spend) = tx.spends.0.into_iter().next().expect("fixture spend");
        let Spend::Witness(spend) = spend else {
            panic!("fixture must contain a v1 witness spend");
        };
        let nockchain_types::tx_engine::v1::tx::InputMetadata::SpendConditions(input_metadata) =
            tx.metadata.inputs
        else {
            panic!("fixture must carry spend-condition metadata");
        };
        let spend_condition = input_metadata
            .0
            .into_iter()
            .find(|(candidate, _)| candidate == &name)
            .map(|(_, spend_condition)| spend_condition)
            .expect("fixture metadata for witness spend");

        (spend.witness.lock_merkle_proof, spend_condition)
    }

    fn outputs_from_transaction_fixture(case: &str) -> Vec<PlannedOutput> {
        let Transaction::V1(tx) = transaction_fixture(case);
        tx.spends
            .0
            .into_iter()
            .flat_map(|(_, spend)| match spend {
                Spend::Legacy(spend) => spend.seeds.0,
                Spend::Witness(spend) => spend.seeds.0,
            })
            .map(|seed| PlannedOutput {
                lock_root: seed.lock_root,
                amount: seed.gift.0 as u64,
                note_data: seed
                    .note_data
                    .iter()
                    .map(|entry| RawNoteDataEntry {
                        key: entry.key.clone(),
                        blob: entry.raw_blob(),
                    })
                    .collect(),
            })
            .collect()
    }

    fn bridge_withdrawal_output_from_transaction_fixture(case: &str) -> PlannedOutput {
        let mut outputs = outputs_from_transaction_fixture(case)
            .into_iter()
            .filter(|output| {
                output
                    .note_data
                    .iter()
                    .any(|entry| entry.key == NOTE_DATA_KEY_BRIDGE_WITHDRAWAL)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            outputs.len(),
            1,
            "fixture should contain exactly one bridge-w output"
        );
        outputs.pop().expect("bridge-w output")
    }

    fn refund_outputs_from_transaction_fixture(case: &str) -> Vec<PlannedOutput> {
        outputs_from_transaction_fixture(case)
            .into_iter()
            .filter(|output| {
                output
                    .note_data
                    .iter()
                    .all(|entry| entry.key != NOTE_DATA_KEY_BRIDGE_WITHDRAWAL)
            })
            .collect()
    }

    #[test]
    fn seed_estimate_counts_all_supported_note_data_payload_cases() {
        let lock_single = estimate_seed_words_legacy(&[output_from_fixture("lock-single")]);
        let lock_v2 = estimate_seed_words_legacy(&[output_from_fixture("lock-v2")]);
        let lock_v4 = estimate_seed_words_legacy(&[output_from_fixture("lock-v4")]);
        let lock_v8 = estimate_seed_words_legacy(&[output_from_fixture("lock-v8")]);
        let lock_v16 = estimate_seed_words_legacy(&[output_from_fixture("lock-v16")]);
        let bridge_deposit = estimate_seed_words_legacy(&[output_from_fixture("bridge-deposit")]);
        let bridge_deposit_large =
            estimate_seed_words_legacy(&[output_from_fixture("bridge-deposit-large")]);
        let bridge_withdrawal =
            estimate_seed_words_legacy(&[output_from_fixture("bridge-withdrawal")]);
        let bridge_withdrawal_long =
            estimate_seed_words_legacy(&[output_from_fixture("bridge-withdrawal-long-event")]);

        assert_eq!(lock_single, 6);
        assert_eq!(lock_v2, 11);
        assert_eq!(lock_v4, 21);
        assert_eq!(lock_v8, 41);
        assert_eq!(lock_v16, 81);
        assert_eq!(bridge_deposit, 7);
        assert_eq!(bridge_deposit_large, 7);
        assert_eq!(bridge_withdrawal, 19);
        assert_eq!(bridge_withdrawal_long, 19);
    }

    #[test]
    fn seed_estimate_counts_wildcard_note_data_with_raw_blob_proxy() {
        let note_data = fixture_note_data("wildcard");
        let raw_entry = note_data.iter().next().expect("wildcard fixture entry");
        let expected_raw_blob_words = (raw_entry.raw_blob().len() as u64).saturating_add(7) / 8;
        let expected_value_words = expected_raw_blob_words.max(1);
        // one key/value entry reduced by `rep z-by` into a singleton list.
        let expected_total_words = 1 + expected_value_words + 1;

        let actual = estimate_seed_words_legacy(&[output_from_fixture("wildcard")]);
        assert_eq!(actual, expected_total_words);
    }

    #[test]
    fn malformed_recognized_keys_use_raw_blob_word_proxy() {
        for case in [
            "lock-unsupported-version", "bridge-unsupported-network", "bridge-unsupported-version",
            "bridge-withdrawal-unsupported-version",
        ] {
            let note_data = fixture_note_data(case);
            let raw_entry = note_data.iter().next().expect("fixture entry");
            let expected_raw_blob_words = (raw_entry.raw_blob().len() as u64).saturating_add(7) / 8;
            let expected_value_words = expected_raw_blob_words.max(1);
            let expected_total_words = 1 + expected_value_words + 1;

            let actual = estimate_seed_words_legacy(&[output_from_fixture(case)]);
            assert_eq!(actual, expected_total_words, "case {case}");
        }
    }

    #[test]
    fn merged_seed_estimate_overwrites_duplicate_keys_per_lock_root() {
        let first = output_from_fixture_with_lock_root("bridge-deposit", 111);
        let second = output_from_fixture_with_lock_root("bridge-deposit-large", 111);
        let merged = estimate_seed_words_merged(&[first.clone(), second.clone()]);
        let expected = estimate_seed_words_legacy(std::slice::from_ref(&second));
        let legacy_total = estimate_seed_words_legacy(&[first, second]);

        assert_eq!(merged, expected);
        assert!(merged < legacy_total);
    }

    #[test]
    fn seed_estimate_switches_to_merged_at_bythos() {
        let outputs = vec![output(1, "k", 1), output(1, "k", 2), output(2, "k", 3)];
        let legacy = estimate_seed_words_legacy(&outputs);
        let merged = estimate_seed_words_merged(&outputs);
        assert!(merged <= legacy);

        let pre = estimate_seed_words(
            &outputs,
            &ChainContext {
                height: BlockHeight(Belt(9)),
                bythos_phase: BlockHeight(Belt(10)),
                base_fee: 1,
                input_fee_divisor: 4,
                min_fee: 0,
            },
        );
        assert_eq!(pre, legacy);

        let post = estimate_seed_words(
            &outputs,
            &ChainContext {
                height: BlockHeight(Belt(10)),
                bythos_phase: BlockHeight(Belt(10)),
                base_fee: 1,
                input_fee_divisor: 4,
                min_fee: 0,
            },
        );
        assert_eq!(post, merged);
    }

    #[test]
    fn withdrawal_tx_fixture_bridge_w_word_count_matches_note_data_fixture() {
        let expected = estimate_seed_words_legacy(&[output_from_fixture("bridge-withdrawal")]);
        let actual =
            estimate_seed_words_legacy(&[bridge_withdrawal_output_from_transaction_fixture(
                "bridge-multisig-withdrawal-basic",
            )]);
        assert_eq!(actual, expected);
    }

    #[test]
    fn withdrawal_tx_fixture_with_change_preserves_bridge_w_word_count() {
        let expected =
            estimate_seed_words_legacy(&[output_from_fixture("bridge-withdrawal-long-event")]);
        let actual =
            estimate_seed_words_legacy(&[bridge_withdrawal_output_from_transaction_fixture(
                "bridge-multisig-withdrawal-with-change",
            )]);
        assert_eq!(actual, expected);
    }

    #[test]
    fn bridge_multisig_withdrawal_fixture_refunds_to_shared_lock() {
        let Transaction::V1(tx) = transaction_fixture("bridge-multisig-withdrawal-with-change");
        let nockchain_types::tx_engine::v1::tx::InputMetadata::SpendConditions(input_metadata) =
            tx.metadata.inputs
        else {
            panic!("fixture must carry spend-condition metadata");
        };
        let spend_condition = input_metadata
            .0
            .into_iter()
            .next()
            .map(|(_, spend_condition)| spend_condition)
            .expect("fixture metadata for witness spend");
        let expected_lock_root = Lock::SpendCondition(spend_condition)
            .hash()
            .expect("fixture lock root");

        let refund_outputs =
            refund_outputs_from_transaction_fixture("bridge-multisig-withdrawal-with-change");
        assert_eq!(
            refund_outputs.len(),
            1,
            "fixture should contain exactly one refund output"
        );
        let refund_output = &refund_outputs[0];
        assert_eq!(refund_output.lock_root, expected_lock_root);
        assert!(
            refund_output
                .note_data
                .iter()
                .any(|entry| entry.key == NOTE_DATA_KEY_LOCK),
            "refund output should retain lock note-data"
        );
    }

    #[test]
    fn seed_estimate_matches_bridge_multisig_withdrawal_fixture_fee_component() {
        let fixture = withdrawal_tx_fixture_entry("bridge-multisig-withdrawal-with-change");
        let chain_context = ChainContext {
            height: BlockHeight(Belt(fixture.height)),
            bythos_phase: BlockHeight(Belt(10)),
            base_fee: 256,
            input_fee_divisor: 4,
            min_fee: 0,
        };

        let estimated = estimate_seed_words(
            &outputs_from_transaction_fixture("bridge-multisig-withdrawal-with-change"),
            &chain_context,
        );

        assert_eq!(estimated, fixture.seed_words);
    }

    #[test]
    fn witness_estimate_matches_bridge_multisig_withdrawal_fixture_fee_component() {
        let fixture = withdrawal_tx_fixture_entry("bridge-multisig-withdrawal-with-change");
        let Transaction::V1(tx) = fixture.transaction.clone();
        let nockchain_types::tx_engine::v1::tx::InputMetadata::SpendConditions(input_metadata) =
            tx.metadata.inputs
        else {
            panic!("fixture must carry spend-condition metadata");
        };
        let spend_condition = input_metadata
            .0
            .into_iter()
            .next()
            .map(|(_, spend_condition)| spend_condition)
            .expect("fixture metadata for witness spend");
        let chain_context = ChainContext {
            height: BlockHeight(Belt(fixture.height)),
            bythos_phase: BlockHeight(Belt(10)),
            base_fee: 256,
            input_fee_divisor: 4,
            min_fee: 0,
        };

        let estimated = estimate_witness_words(
            &[WitnessWordInput {
                spend_condition,
                input_origin_page: BlockHeight(Belt(17)),
                spend_condition_count: Some(1),
            }],
            &chain_context,
        );

        assert_eq!(estimated, fixture.witness_words);
    }

    #[test]
    fn witness_estimate_depends_on_required_signatures() {
        let sc_one = SpendCondition::new(vec![LockPrimitive::Pkh(Pkh::new(1, vec![hash(7)]))]);
        let sc_three = SpendCondition::new(vec![LockPrimitive::Pkh(Pkh::new(
            3,
            vec![hash(7), hash(8), hash(9)],
        ))]);

        let context = ChainContext {
            height: BlockHeight(Belt(11)),
            bythos_phase: BlockHeight(Belt(10)),
            base_fee: 1,
            input_fee_divisor: 4,
            min_fee: 0,
        };
        let one = estimate_witness_words(
            &[WitnessWordInput {
                spend_condition: sc_one,
                input_origin_page: BlockHeight(Belt(11)),
                spend_condition_count: None,
            }],
            &context,
        );
        let three = estimate_witness_words(
            &[WitnessWordInput {
                spend_condition: sc_three,
                input_origin_page: BlockHeight(Belt(11)),
                spend_condition_count: None,
            }],
            &context,
        );

        assert!(three > one);
    }

    #[test]
    fn witness_estimate_grows_with_merkle_path_hint() {
        let spend_condition =
            SpendCondition::new(vec![LockPrimitive::Pkh(Pkh::new(1, vec![hash(7)]))]);
        let context = ChainContext {
            height: BlockHeight(Belt(11)),
            bythos_phase: BlockHeight(Belt(10)),
            base_fee: 1,
            input_fee_divisor: 4,
            min_fee: 0,
        };

        let one = estimate_witness_words(
            &[WitnessWordInput {
                spend_condition: spend_condition.clone(),
                input_origin_page: BlockHeight(Belt(11)),
                spend_condition_count: Some(1),
            }],
            &context,
        );
        let four = estimate_witness_words(
            &[WitnessWordInput {
                spend_condition: spend_condition.clone(),
                input_origin_page: BlockHeight(Belt(11)),
                spend_condition_count: Some(4),
            }],
            &context,
        );
        let eight = estimate_witness_words(
            &[WitnessWordInput {
                spend_condition,
                input_origin_page: BlockHeight(Belt(11)),
                spend_condition_count: Some(8),
            }],
            &context,
        );

        assert!(four > one);
        assert!(eight > four);
    }

    #[test]
    fn witness_estimate_normalizes_non_power_of_two_count_hint() {
        let spend_condition =
            SpendCondition::new(vec![LockPrimitive::Pkh(Pkh::new(1, vec![hash(1)]))]);
        let context = ChainContext {
            height: BlockHeight(Belt(11)),
            bythos_phase: BlockHeight(Belt(10)),
            base_fee: 1,
            input_fee_divisor: 4,
            min_fee: 0,
        };

        let three = estimate_witness_words(
            &[WitnessWordInput {
                spend_condition: spend_condition.clone(),
                input_origin_page: BlockHeight(Belt(11)),
                spend_condition_count: Some(3),
            }],
            &context,
        );
        let four = estimate_witness_words(
            &[WitnessWordInput {
                spend_condition,
                input_origin_page: BlockHeight(Belt(11)),
                spend_condition_count: Some(4),
            }],
            &context,
        );
        assert_eq!(three, four);
    }

    #[test]
    fn v0_witness_estimate_matches_legacy_signature_map_shape() {
        let context = ChainContext {
            height: BlockHeight(Belt(1)),
            bythos_phase: BlockHeight(Belt(1)),
            base_fee: 128,
            input_fee_divisor: 4,
            min_fee: 256,
        };
        let estimator = WordCountEstimator::new(&context);

        assert_eq!(estimator.estimate_v0_witness_words(1), 31);
        assert_eq!(estimator.estimate_v0_witness_words(2), 61);
        assert_eq!(estimator.estimate_v0_witness_words(3), 91);
    }

    #[test]
    fn pre_bythos_stub_lmp_tx_fee_is_discounted_by_one_witness_word() {
        let spend_condition = bridge_multisig_spend_condition();
        let bythos_phase = BlockHeight(Belt(80));
        let context = ChainContext {
            height: BlockHeight(Belt(93)),
            bythos_phase: bythos_phase.clone(),
            base_fee: 256,
            input_fee_divisor: 4,
            min_fee: 0,
        };
        let (actual_stub_words, actual_full_words) = encoded_stub_and_full_witness_words();
        assert_eq!(
            actual_full_words - actual_stub_words,
            1,
            "full LMP carries the %full version atom that legacy stub LMP omits"
        );

        // Regression for bridge withdrawals that spend a pre-Bythos bridge
        // multisig note after Bythos activation. The selected input's origin
        // height, not the current chain height, determines the LMP shape.
        let estimated_stub_words = estimate_witness_words(
            &[WitnessWordInput {
                spend_condition: spend_condition.clone(),
                input_origin_page: BlockHeight(Belt(7)),
                spend_condition_count: Some(1),
            }],
            &context,
        );
        let estimated_full_words = estimate_witness_words(
            &[WitnessWordInput {
                spend_condition,
                input_origin_page: bythos_phase,
                spend_condition_count: Some(1),
            }],
            &context,
        );
        assert_eq!(estimated_stub_words, actual_stub_words);
        assert_eq!(estimated_full_words, actual_full_words);

        let seed_words = 37;
        let stub_fee = compute_minimum_fee(FeeInputs {
            seed_words,
            witness_words: estimated_stub_words,
            base_fee: context.base_fee,
            input_fee_divisor: context.input_fee_divisor,
            min_fee: context.min_fee,
            height: context.height.clone(),
            bythos_phase: context.bythos_phase.clone(),
        })
        .minimum_fee;
        let full_fee = compute_minimum_fee(FeeInputs {
            seed_words,
            witness_words: estimated_full_words,
            base_fee: context.base_fee,
            input_fee_divisor: context.input_fee_divisor,
            min_fee: context.min_fee,
            height: context.height,
            bythos_phase: context.bythos_phase,
        })
        .minimum_fee;

        assert_eq!(
            full_fee - stub_fee,
            context.base_fee / context.input_fee_divisor,
            "one extra full-proof witness word should be charged after the Bythos witness discount"
        );
    }

    #[test]
    fn mixed_stub_and_full_lmp_inputs_charge_each_input_shape() {
        let spend_condition = bridge_multisig_spend_condition();
        let bythos_phase = BlockHeight(Belt(80));
        let context = ChainContext {
            height: BlockHeight(Belt(93)),
            bythos_phase: bythos_phase.clone(),
            base_fee: 256,
            input_fee_divisor: 4,
            min_fee: 0,
        };
        let (stub_words, full_words) = encoded_stub_and_full_witness_words();

        let mixed_words = estimate_witness_words(
            &[
                WitnessWordInput {
                    spend_condition: spend_condition.clone(),
                    input_origin_page: BlockHeight(Belt(7)),
                    spend_condition_count: Some(1),
                },
                WitnessWordInput {
                    spend_condition,
                    input_origin_page: bythos_phase,
                    spend_condition_count: Some(1),
                },
            ],
            &context,
        );

        assert_eq!(
            mixed_words,
            stub_words + full_words,
            "a mixed withdrawal must charge pre-Bythos and post-Bythos inputs independently"
        );

        let fee = compute_minimum_fee(FeeInputs {
            seed_words: 37,
            witness_words: mixed_words,
            base_fee: context.base_fee,
            input_fee_divisor: context.input_fee_divisor,
            min_fee: context.min_fee,
            height: context.height,
            bythos_phase: context.bythos_phase,
        });

        assert_eq!(fee.witness_fee, mixed_words * 64);
        assert_eq!(fee.minimum_fee, fee.seed_fee + fee.witness_fee);
    }

    #[test]
    fn stub_and_full_lmp_tx_fees_can_match_when_min_fee_floor_dominates() {
        let (stub_words, full_words) = encoded_stub_and_full_witness_words();
        assert_eq!(
            full_words - stub_words,
            1,
            "this case still uses distinct stub/full proof shapes"
        );

        let min_fee = 1_000_000;
        let stub_fee = compute_minimum_fee(FeeInputs {
            seed_words: 0,
            witness_words: stub_words,
            base_fee: 256,
            input_fee_divisor: 4,
            min_fee,
            height: BlockHeight(Belt(93)),
            bythos_phase: BlockHeight(Belt(80)),
        });
        let full_fee = compute_minimum_fee(FeeInputs {
            seed_words: 0,
            witness_words: full_words,
            base_fee: 256,
            input_fee_divisor: 4,
            min_fee,
            height: BlockHeight(Belt(93)),
            bythos_phase: BlockHeight(Belt(80)),
        });

        assert!(stub_fee.word_fee < min_fee);
        assert!(full_fee.word_fee < min_fee);
        assert_eq!(stub_fee.minimum_fee, min_fee);
        assert_eq!(
            full_fee.minimum_fee, stub_fee.minimum_fee,
            "when both word fees are below the floor, the final tx fee is identical"
        );
    }

    #[test]
    fn pre_bythos_bridge_multisig_stub_fixture_fee_matches_hoon_oracle() {
        let fixture = withdrawal_tx_fixture_entry(BRIDGE_PRE_BYTHOS_STUB_WITH_CHANGE);
        let (lock_merkle_proof, spend_condition) =
            first_witness_spend_condition(fixture.transaction.clone());
        assert!(
            matches!(lock_merkle_proof, LockMerkleProof::Stub(_)),
            "fixture should spend a pre-Bythos note using the legacy stub proof shape"
        );
        let context = ChainContext {
            height: BlockHeight(Belt(fixture.height)),
            bythos_phase: BlockHeight(Belt(10)),
            base_fee: 256,
            input_fee_divisor: 4,
            min_fee: 0,
        };

        let estimated_seed_words = estimate_seed_words(
            &outputs_from_transaction_fixture(BRIDGE_PRE_BYTHOS_STUB_WITH_CHANGE),
            &context,
        );
        let estimated_witness_words = estimate_witness_words(
            &[WitnessWordInput {
                spend_condition,
                input_origin_page: BlockHeight(Belt(BRIDGE_PRE_BYTHOS_STUB_ORIGIN_PAGE)),
                spend_condition_count: Some(1),
            }],
            &context,
        );
        let minimum_fee = compute_minimum_fee(FeeInputs {
            seed_words: estimated_seed_words,
            witness_words: estimated_witness_words,
            base_fee: context.base_fee,
            input_fee_divisor: context.input_fee_divisor,
            min_fee: context.min_fee,
            height: context.height,
            bythos_phase: context.bythos_phase,
        })
        .minimum_fee;

        assert_eq!(estimated_seed_words, fixture.seed_words);
        assert_eq!(estimated_witness_words, fixture.witness_words);
        assert_eq!(
            minimum_fee, fixture.min_fee,
            "fixture words/min-fee are generated by the Hoon wallet estimate-fee path"
        );
    }

    #[test]
    fn witness_estimate_matches_bridge_multisig_withdrawal_fixture_shape() {
        let Transaction::V1(tx) = transaction_fixture("bridge-multisig-withdrawal-basic");
        let (name, spend) = tx.spends.0.into_iter().next().expect("fixture spend");
        let Spend::Witness(spend) = spend else {
            panic!("fixture must contain a v1 witness spend");
        };
        let nockchain_types::tx_engine::v1::tx::InputMetadata::SpendConditions(input_metadata) =
            tx.metadata.inputs
        else {
            panic!("fixture must carry spend-condition metadata");
        };
        let spend_condition = input_metadata
            .0
            .into_iter()
            .find(|(candidate, _)| candidate == &name)
            .map(|(_, spend_condition)| spend_condition)
            .expect("fixture metadata for witness spend");
        let bythos_phase = match &spend.witness.lock_merkle_proof {
            nockchain_types::tx_engine::v1::tx::LockMerkleProof::Full(_) => 17,
            nockchain_types::tx_engine::v1::tx::LockMerkleProof::Stub(_) => 18,
        };
        let context = ChainContext {
            height: BlockHeight(Belt(10)),
            bythos_phase: BlockHeight(Belt(bythos_phase)),
            base_fee: 128,
            input_fee_divisor: 4,
            min_fee: 256,
        };
        let estimated = estimate_witness_words(
            &[WitnessWordInput {
                spend_condition: spend_condition.clone(),
                input_origin_page: BlockHeight(Belt(17)),
                spend_condition_count: Some(1),
            }],
            &context,
        );
        let actual = WordCountEstimator::count_encoded_leaves(
            &nockchain_types::tx_engine::v1::tx::Witness::new(
                spend.witness.lock_merkle_proof.clone(),
                WordCountEstimator::synthetic_pkh_signature(&spend_condition),
                Vec::new(),
            ),
        );

        assert_eq!(estimated, actual);
    }

    #[test]
    fn minimum_fee_matches_bridge_multisig_withdrawal_with_change_fixture_fee_components() {
        let fixture = withdrawal_tx_fixture_entry("bridge-multisig-withdrawal-with-change");
        let minimum_fee = compute_minimum_fee(FeeInputs {
            seed_words: fixture.seed_words,
            witness_words: fixture.witness_words,
            base_fee: 256,
            input_fee_divisor: 4,
            min_fee: 0,
            height: BlockHeight(Belt(fixture.height)),
            bythos_phase: BlockHeight(Belt(10)),
        })
        .minimum_fee;

        assert_eq!(
            minimum_fee, fixture.min_fee,
            "fixture_seed_words={} fixture_witness_words={}",
            fixture.seed_words, fixture.witness_words
        );
    }

    #[test]
    fn lmp_estimate_matches_bridge_multisig_withdrawal_fixture_shape() {
        let Transaction::V1(tx) = transaction_fixture("bridge-multisig-withdrawal-with-change");
        let (name, spend) = tx.spends.0.into_iter().next().expect("fixture spend");
        let Spend::Witness(spend) = spend else {
            panic!("fixture must contain a v1 witness spend");
        };
        let nockchain_types::tx_engine::v1::tx::InputMetadata::SpendConditions(input_metadata) =
            tx.metadata.inputs
        else {
            panic!("fixture must carry spend-condition metadata");
        };
        let spend_condition = input_metadata
            .0
            .into_iter()
            .find(|(candidate, _)| candidate == &name)
            .map(|(_, spend_condition)| spend_condition)
            .expect("fixture metadata for witness spend");
        let bythos_active = matches!(
            spend.witness.lock_merkle_proof,
            nockchain_types::tx_engine::v1::tx::LockMerkleProof::Full(_)
        );
        let estimated =
            WordCountEstimator::estimate_lmp_words(&spend_condition, bythos_active, Some(1));
        let actual = WordCountEstimator::count_encoded_leaves(&spend.witness.lock_merkle_proof);

        assert_eq!(estimated, actual);
    }
}
