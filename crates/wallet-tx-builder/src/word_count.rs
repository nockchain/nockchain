use std::collections::BTreeMap;

use bytes::Bytes;
use nockchain_types::tx_engine::common::BlockHeight;
use nockchain_types::tx_engine::v1::tx::{LockPrimitive, SpendCondition};

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
        // mirror wallet +estimate-fee structure:
        // witness_words = lmp_words + pkh_signature_words + tim_words + hax_words
        let lmp_words =
            Self::estimate_lmp_words(spend_condition, bythos_active, spend_condition_count);
        let pkh_signature_words = Self::estimate_pkh_signature_words(spend_condition);
        let tim_words = 1; // witness.tim is null in current create-tx path
        let hax_words = 1; // witness.hax is empty map in current create-tx path
        lmp_words
            .saturating_add(pkh_signature_words)
            .saturating_add(tim_words)
            .saturating_add(hax_words)
    }

    fn estimate_lmp_words(
        spend_condition: &SpendCondition,
        bythos_active: bool,
        spend_condition_count: Option<u64>,
    ) -> u64 {
        // lock-merkle-proof:
        // stub: [spend_condition axis proof]
        // full: [%full spend_condition axis proof]
        // proof: [root path]
        // path length ~= log2(number of spend conditions). Use 1 (empty path) when
        // lock-level cardinality is unknown.
        let version_words: u64 = if bythos_active { 1 } else { 0 };
        let spend_condition_words = Self::estimate_spend_condition_words(spend_condition);
        let axis_words = 1;
        let proof_words = Self::estimate_merkle_proof_words(spend_condition_count);
        version_words
            .saturating_add(spend_condition_words)
            .saturating_add(axis_words)
            .saturating_add(proof_words)
    }

    fn estimate_merkle_proof_words(spend_condition_count_hint: Option<u64>) -> u64 {
        // proof: [root path], where root is one noun-digest (5 atoms) and path is
        // a list of noun-digests. For a single spend-condition lock, path is empty.
        let count = Self::normalize_spend_condition_count(spend_condition_count_hint.unwrap_or(1));
        let path_len = count.ilog2() as u64;
        let path_words = Self::estimate_list_words_len(path_len, 5);
        5 + path_words
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

    fn estimate_pkh_signature_words(spend_condition: &SpendCondition) -> u64 {
        let num_sigs_required = spend_condition
            .iter()
            .map(|primitive| match primitive {
                LockPrimitive::Pkh(pkh) => pkh.m,
                _ => 0,
            })
            .sum::<u64>();
        // from wallet +estimate-fee:
        // map-words entries key=hash(5 leaves) value=(pubkey + signature) => 13 + 16
        Self::map_words(num_sigs_required, 5, 13 + 16)
    }

    fn estimate_note_data_words(entries: &[RawNoteDataEntry]) -> u64 {
        if entries.is_empty() {
            return 1;
        }
        // z-map tree has n+1 null branches. Sum key/value leaves per entry + null branches.
        let kv_words = entries
            .iter()
            .map(|entry| {
                let key_words = 1; // @tas key atom
                let value_words = Self::estimate_note_data_value_words(entry);
                key_words + value_words
            })
            .sum::<u64>();
        kv_words.saturating_add(entries.len() as u64 + 1)
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
                let beid_words =
                    Self::estimate_list_words_len(bridge_w.base_event_id.len() as u64, 1);
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
    use nockapp::noun::slab::{NockJammer, NounSlab};
    use nockapp::utils::NOCK_STACK_SIZE;
    use nockchain_math::belt::Belt;
    use nockchain_types::tx_engine::common::Hash;
    use nockchain_types::tx_engine::v1::note::NoteData;
    use nockchain_types::tx_engine::v1::tx::{LockPrimitive, Pkh};
    use nockvm::ext::NounExt;
    use nockvm::mem::NockStack;
    use nockvm::noun::Noun;
    use noun_serde::{NounDecode, NounEncode};

    use super::*;
    use crate::types::RawNoteDataEntry;

    #[derive(Debug, Clone, PartialEq, Eq, NounDecode)]
    struct FixtureEntry {
        case: String,
        note_data: NoteData,
    }

    fn hash(v: u64) -> Hash {
        Hash::from_limbs(&[v, 0, 0, 0, 0])
    }

    fn jam<T: NounEncode>(value: &T) -> Bytes {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = value.to_noun(&mut slab);
        slab.set_root(noun);
        slab.jam()
    }

    fn output(lock_root: u64, key: &str, value: u64) -> PlannedOutput {
        PlannedOutput {
            lock_root: hash(lock_root),
            amount: 1,
            note_data: vec![RawNoteDataEntry {
                key: key.to_string(),
                blob: jam(&value),
            }],
        }
    }

    fn decode_note_data_fixtures() -> Vec<FixtureEntry> {
        let fixture_bytes = include_bytes!("../tests/fixtures/note_data_fixtures.jam");
        let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
        let noun = Noun::cue_bytes_slice(&mut stack, fixture_bytes).expect("fixture jam must cue");
        Vec::<FixtureEntry>::from_noun(&noun).expect("fixture noun must decode")
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
                blob: entry.blob.clone(),
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
                blob: entry.blob.clone(),
            })
            .collect::<Vec<_>>();
        PlannedOutput {
            lock_root: hash(lock_root),
            amount: 1,
            note_data,
        }
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

        assert_eq!(lock_single, 7);
        assert_eq!(lock_v2, 12);
        assert_eq!(lock_v4, 22);
        assert_eq!(lock_v8, 42);
        assert_eq!(lock_v16, 82);
        assert_eq!(bridge_deposit, 8);
        assert_eq!(bridge_deposit_large, 8);
        assert_eq!(bridge_withdrawal, 20);
        assert_eq!(bridge_withdrawal_long, 23);
    }

    #[test]
    fn seed_estimate_counts_wildcard_note_data_with_raw_blob_proxy() {
        let note_data = fixture_note_data("wildcard");
        let raw_entry = note_data.iter().next().expect("wildcard fixture entry");
        let expected_raw_blob_words = (raw_entry.blob.len() as u64).saturating_add(7) / 8;
        let expected_value_words = expected_raw_blob_words.max(1);
        // one key/value entry: key (1) + value + z-map null branches (2)
        let expected_total_words = 1 + expected_value_words + 2;

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
            let expected_raw_blob_words = (raw_entry.blob.len() as u64).saturating_add(7) / 8;
            let expected_value_words = expected_raw_blob_words.max(1);
            let expected_total_words = 1 + expected_value_words + 2;

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
}
