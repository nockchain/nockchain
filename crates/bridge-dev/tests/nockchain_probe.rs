use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use async_trait::async_trait;
use bridge::shared::types::{OutputV1, OutputsV1, Tx, TxV1};
use bridge_dev::nockchain_probe::{
    decode_nockchain_transaction, wait_for_nockchain_transaction, NockchainInclusionFacts,
    NockchainInputSnapshotFacts, NockchainObservedBlock, NockchainProbeError,
    NockchainProbeObservation, NockchainProbeRequest, NockchainProbeSource, NoteNameFacts,
    SelectedInputNoteFacts,
};
use nockchain_math::belt::Belt;
use nockchain_types::tx_engine::common::{
    BlockHeight, FirstName, Hash, Name, Nicks, Signature, Version,
};
use nockchain_types::v1::{
    LockMerkleProof, LockPrimitive, MerkleProof, Note, NoteData, NoteV1, Pkh, PkhSignature, RawTx,
    Seed, Seeds, Spend, Spend0, Spend1, SpendCondition, Spends, Witness,
};

#[test]
fn decodes_exact_mixed_spends_outputs_fees_and_recipient_candidates() {
    let fixture = fixture();
    let facts = decode_nockchain_transaction(
        &fixture.request,
        &fixture.observation,
        fixture.block,
        vec![fixture.observation.inclusion.clone().expect("inclusion")],
    )
    .expect("known transaction must decode");

    assert_eq!(facts.transaction_id, fixture.request.transaction_id);
    assert_eq!(facts.inclusion.height, 100);
    assert_eq!(facts.tip_height, 105);
    assert_eq!(facts.confirmation_depth, 5);
    assert_eq!(facts.raw_transaction.version, 1);
    assert_eq!(
        facts.raw_transaction.embedded_transaction_id,
        facts.transaction_id
    );
    assert_eq!(
        facts.raw_transaction.computed_transaction_id,
        facts.transaction_id
    );
    assert_eq!(
        facts
            .raw_transaction
            .spends
            .iter()
            .map(|spend| spend.spend_kind.as_str())
            .collect::<Vec<_>>(),
        vec!["legacy", "witness"]
    );
    assert_eq!(facts.transaction_fee_nicks, 10);
    assert_eq!(facts.total_input_nicks, 1_200);
    assert_eq!(facts.total_output_nicks, 1_190);
    assert_eq!(facts.unaccounted_nicks, 0);
    assert_eq!(facts.outputs.len(), 3);
    assert_eq!(facts.matching_recipient_output_indices.len(), 2);
    assert!(facts
        .matching_recipient_output_indices
        .iter()
        .all(|index| { facts.outputs[*index].lock_root == fixture.request.recipient_lock_root }));
    assert!(facts.outputs.iter().all(|output| {
        output.assets_nicks > 0
            && output.origin_height == 100
            && output.origin_transaction_id == facts.transaction_id
            && output.note_version == 1
    }));

    let serialized = serde_json::to_value(&facts).expect("evidence must serialize");
    assert_eq!(serialized["transaction_fee_nicks"], 10);
    assert!(serialized["selected_inputs"][0]["assets_nicks"]
        .as_u64()
        .is_some());
}

#[tokio::test]
async fn waits_through_lagging_public_tip_and_private_block_index() {
    let fixture = fixture();
    let inclusion = fixture.observation.inclusion.clone();
    let mut source = ScriptedSource::new(vec![
        NockchainProbeObservation {
            mempool_accepted: Some(true),
            tip_height: 99,
            inclusion: inclusion.clone(),
        },
        NockchainProbeObservation {
            mempool_accepted: Some(true),
            tip_height: 105,
            inclusion,
        },
    ]);
    source.push_block_response(100, None);
    source.push_block_response(100, Some(fixture.block));

    let facts = wait_for_nockchain_transaction(
        &mut source,
        &fixture.request,
        Duration::from_secs(1),
        Duration::ZERO,
    )
    .await
    .expect("probe must wait for both tip and private block index");
    assert_eq!(facts.confirmation_depth, 5);
    assert_eq!(source.block_requests, vec![100, 100]);
}

#[tokio::test]
async fn follows_reinclusion_to_the_new_block_before_confirming() {
    let fixture = fixture();
    let old_inclusion = NockchainInclusionFacts {
        height: 100,
        block_id: hash(900).to_base58(),
    };
    let new_inclusion = NockchainInclusionFacts {
        height: 101,
        block_id: hash(901).to_base58(),
    };
    let mut new_block = fixture.block.clone();
    new_block.height = 101;
    new_block.block_id = new_inclusion.block_id.clone();
    for (_, transaction) in &mut new_block.transactions {
        let Tx::V1(transaction) = transaction;
        for output in &mut transaction.outputs.0 {
            let Note::V1(note) = &mut output.note else {
                continue;
            };
            note.origin_page = BlockHeight(Belt(101));
        }
    }
    let mut source = ScriptedSource::new(vec![
        NockchainProbeObservation {
            mempool_accepted: Some(true),
            tip_height: 100,
            inclusion: Some(old_inclusion.clone()),
        },
        NockchainProbeObservation {
            mempool_accepted: Some(false),
            tip_height: 103,
            inclusion: Some(new_inclusion.clone()),
        },
    ]);
    source.push_block_response(100, Some(fixture.block));
    source.push_block_response(101, Some(new_block));
    let mut request = fixture.request;
    request.confirmation_depth = 2;

    let facts = wait_for_nockchain_transaction(
        &mut source,
        &request,
        Duration::from_secs(1),
        Duration::ZERO,
    )
    .await
    .expect("re-included transaction must confirm at its new location");
    assert_eq!(facts.inclusion, new_inclusion);
    assert_eq!(
        facts.inclusion_history,
        vec![old_inclusion, facts.inclusion.clone()]
    );
}

#[tokio::test]
async fn accepted_but_absent_transaction_times_out_with_last_chain_observation(
) -> Result<(), String> {
    let fixture = fixture();
    let pending = NockchainProbeObservation {
        mempool_accepted: Some(true),
        tip_height: 99,
        inclusion: None,
    };
    let mut source = ScriptedSource::new(vec![pending.clone()]);
    let error = wait_for_nockchain_transaction(
        &mut source,
        &fixture.request,
        Duration::from_millis(2),
        Duration::ZERO,
    )
    .await
    .expect_err("accepted transaction without inclusion must time out");
    match error {
        NockchainProbeError::Timeout {
            last_observation,
            inclusion_history,
        } => {
            assert_eq!(last_observation, Some(pending));
            assert!(inclusion_history.is_empty());
        }
        other => return Err(format!("unexpected error: {other}")),
    }
    Ok(())
}

#[test]
fn rejects_missing_transaction_wrong_block_and_input_snapshot_drift() {
    let fixture = fixture();
    let mut missing = fixture.block.clone();
    missing.transactions.clear();
    assert!(matches!(
        decode_nockchain_transaction(
            &fixture.request,
            &fixture.observation,
            missing,
            Vec::new()
        ),
        Err(NockchainProbeError::Malformed(message))
            if message.contains("does not contain")
    ));

    let mut wrong_block = fixture.block.clone();
    wrong_block.block_id = hash(999).to_base58();
    assert!(matches!(
        decode_nockchain_transaction(
            &fixture.request,
            &fixture.observation,
            wrong_block,
            Vec::new()
        ),
        Err(NockchainProbeError::Malformed(message))
            if message.contains("block identity")
    ));

    let mut drifted = fixture.request.clone();
    drifted.selected_inputs[0].name = NoteNameFacts {
        first: hash(700).to_base58(),
        last: hash(701).to_base58(),
    };
    assert!(matches!(
        decode_nockchain_transaction(&drifted, &fixture.observation, fixture.block, Vec::new()),
        Err(NockchainProbeError::InputSnapshotMismatch { .. })
    ));
}

#[test]
fn rejects_malformed_embedded_raw_id_and_arithmetic_overflow() {
    let fixture = fixture();
    let mut malformed_block = fixture.block.clone();
    let (_, Tx::V1(transaction)) = &mut malformed_block.transactions[0];
    transaction.raw_tx.id = hash(777);
    assert!(matches!(
        decode_nockchain_transaction(
            &fixture.request,
            &fixture.observation,
            malformed_block,
            Vec::new()
        ),
        Err(NockchainProbeError::Malformed(message))
            if message.contains("raw transaction id")
    ));

    let mut overflow = fixture.request.clone();
    overflow.selected_inputs[0].assets_nicks = u64::MAX;
    overflow.selected_inputs[1].assets_nicks = 1;
    assert!(matches!(
        decode_nockchain_transaction(&overflow, &fixture.observation, fixture.block, Vec::new()),
        Err(NockchainProbeError::ArithmeticOverflow)
    ));
}

#[derive(Clone)]
struct Fixture {
    request: NockchainProbeRequest,
    observation: NockchainProbeObservation,
    block: NockchainObservedBlock,
}

fn fixture() -> Fixture {
    let input_names = [Name::new(hash(10), hash(11)), Name::new(hash(20), hash(21))];
    let recipient_root = hash(30);
    let change_root = hash(40);
    let spend_condition =
        SpendCondition::new(vec![LockPrimitive::Pkh(Pkh::new(1, vec![hash(50)]))]);
    let legacy = Spend::Legacy(Spend0 {
        signature: Signature(Vec::new()),
        seeds: Seeds(Vec::new()),
        fee: Nicks(3),
    });
    let witness = Spend::Witness(Spend1 {
        witness: Witness::new(
            LockMerkleProof::new_stub(
                spend_condition,
                1,
                MerkleProof {
                    root: hash(60),
                    path: Vec::new(),
                },
            ),
            PkhSignature(Vec::new()),
            Vec::new(),
        ),
        seeds: Seeds(Vec::new()),
        fee: Nicks(7),
    });
    let mut raw_tx = RawTx {
        version: Version::V1,
        id: hash(0),
        spends: Spends(vec![
            (input_names[0].clone(), legacy),
            (input_names[1].clone(), witness),
        ]),
    };
    raw_tx.id = raw_tx.compute_id().expect("fixture raw tx must hash");
    let transaction_id = raw_tx.id.to_base58();
    let outputs = vec![
        output(recipient_root.clone(), hash(31), 600, 100),
        output(recipient_root.clone(), hash(32), 300, 100),
        output(change_root, hash(41), 290, 100),
    ];
    let transaction = Tx::V1(TxV1 {
        version: 1,
        raw_tx,
        total_size: 512,
        outputs: OutputsV1(outputs),
    });
    let block_id = hash(900).to_base58();
    let inclusion = NockchainInclusionFacts {
        height: 100,
        block_id: block_id.clone(),
    };
    Fixture {
        request: NockchainProbeRequest {
            transaction_id: transaction_id.clone(),
            confirmation_depth: 3,
            recipient_lock_root: recipient_root.to_base58(),
            input_snapshot: NockchainInputSnapshotFacts {
                height: 99,
                block_id: hash(899).to_base58(),
            },
            selected_inputs: vec![
                selected_input(&input_names[0], 700, 90, 70, 0),
                selected_input(&input_names[1], 500, 91, 71, 1),
            ],
        },
        observation: NockchainProbeObservation {
            mempool_accepted: Some(false),
            tip_height: 105,
            inclusion: Some(inclusion),
        },
        block: NockchainObservedBlock {
            height: 100,
            block_id,
            transactions: vec![(transaction_id, transaction)],
        },
    }
}

fn selected_input(
    name: &Name,
    assets_nicks: u64,
    origin_height: u64,
    origin_hash: u64,
    note_version: u64,
) -> SelectedInputNoteFacts {
    SelectedInputNoteFacts {
        name: NoteNameFacts {
            first: name.first.to_base58(),
            last: name.last.to_base58(),
        },
        note_version,
        assets_nicks,
        origin_height,
        origin_transaction_id: (note_version == 0).then(|| hash(origin_hash).to_base58()),
        origin_is_coinbase: (note_version == 0).then_some(false),
    }
}

fn output(lock_root: Hash, name_last: Hash, assets_nicks: usize, origin_height: u64) -> OutputV1 {
    let first_name = FirstName::from_lock_root(&lock_root)
        .expect("fixture lock root must derive a first name")
        .into_hash();
    let seed = Seed {
        output_source: None,
        lock_root,
        note_data: NoteData::new(Vec::new()),
        gift: Nicks(assets_nicks),
        parent_hash: name_last.clone(),
    };
    OutputV1 {
        note: Note::V1(NoteV1::new(
            BlockHeight(Belt(origin_height)),
            Name::new(first_name, name_last),
            NoteData::new(Vec::new()),
            Nicks(assets_nicks),
        )),
        seeds: Seeds(vec![seed]),
    }
}

fn hash(seed: u64) -> Hash {
    Hash::from_limbs(&[seed, seed + 1, seed + 2, seed + 3, seed + 4])
}

struct ScriptedSource {
    observations: VecDeque<NockchainProbeObservation>,
    repeat: NockchainProbeObservation,
    blocks: HashMap<u64, VecDeque<Option<NockchainObservedBlock>>>,
    block_requests: Vec<u64>,
}

impl ScriptedSource {
    fn new(observations: Vec<NockchainProbeObservation>) -> Self {
        let repeat = observations
            .last()
            .cloned()
            .expect("scripted source needs an observation");
        Self {
            observations: observations.into(),
            repeat,
            blocks: HashMap::new(),
            block_requests: Vec::new(),
        }
    }

    fn push_block_response(&mut self, height: u64, block: Option<NockchainObservedBlock>) {
        self.blocks.entry(height).or_default().push_back(block);
    }
}

#[async_trait]
impl NockchainProbeSource for ScriptedSource {
    async fn observe_transaction(
        &mut self,
        _transaction_id: &str,
    ) -> Result<NockchainProbeObservation, String> {
        Ok(self
            .observations
            .pop_front()
            .unwrap_or_else(|| self.repeat.clone()))
    }

    async fn block_at_height(
        &mut self,
        height: u64,
    ) -> Result<Option<NockchainObservedBlock>, String> {
        self.block_requests.push(height);
        Ok(self
            .blocks
            .get_mut(&height)
            .and_then(VecDeque::pop_front)
            .flatten())
    }
}
