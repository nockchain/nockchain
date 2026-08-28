use std::collections::HashSet;
use std::time::Duration;

use async_trait::async_trait;
use bridge::core::ports::NockSourcePort;
use bridge::shared::nockchain::NockGrpcSource;
use bridge::shared::types::{OutputV1, Tx};
use nockapp_grpc::pb::common::v1::Base58Hash;
use nockapp_grpc::pb::public::v2::transaction_accepted_response;
use nockapp_grpc::services::public_nockchain::v2::client::PublicNockchainGrpcClient;
use nockchain_types::tx_engine::common::{FirstName, Hash, Name};
use nockchain_types::v1::{Note, NoteData, RawTx, Seeds, Spend};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::time::{sleep, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteNameFacts {
    pub first: String,
    pub last: String,
}

impl NoteNameFacts {
    fn from_name(name: &Name) -> Self {
        Self {
            first: name.first.to_base58(),
            last: name.last.to_base58(),
        }
    }

    fn validate(&self) -> Result<(), NockchainProbeError> {
        validate_hash("note name first", &self.first)?;
        validate_hash("note name last", &self.last)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedInputNoteFacts {
    pub name: NoteNameFacts,
    pub note_version: u64,
    pub assets_nicks: u64,
    pub origin_height: u64,
    pub origin_transaction_id: Option<String>,
    pub origin_is_coinbase: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NockchainInputSnapshotFacts {
    pub height: u64,
    pub block_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NockchainProbeRequest {
    pub transaction_id: String,
    pub confirmation_depth: u64,
    pub recipient_lock_root: String,
    pub input_snapshot: NockchainInputSnapshotFacts,
    pub selected_inputs: Vec<SelectedInputNoteFacts>,
}

impl NockchainProbeRequest {
    fn validate(&self) -> Result<(), NockchainProbeError> {
        validate_hash("transaction id", &self.transaction_id)?;
        validate_hash("recipient lock root", &self.recipient_lock_root)?;
        validate_hash(
            "selected input snapshot block id", &self.input_snapshot.block_id,
        )?;
        if self.input_snapshot.height == 0 {
            return Err(NockchainProbeError::InvalidRequest(
                "selected input snapshot height must be positive".to_owned(),
            ));
        }
        if self.selected_inputs.is_empty() {
            return Err(NockchainProbeError::InvalidRequest(
                "selected input snapshot must not be empty".to_owned(),
            ));
        }
        let mut names = HashSet::with_capacity(self.selected_inputs.len());
        for input in &self.selected_inputs {
            input.name.validate()?;
            if let Some(origin_transaction_id) = &input.origin_transaction_id {
                validate_hash("input origin transaction id", origin_transaction_id)?;
            }
            if input.assets_nicks == 0 || input.origin_height == 0 || input.note_version > 1 {
                return Err(NockchainProbeError::InvalidRequest(
                    "selected input assets/origin must be nonzero and note version must be 0 or 1"
                        .to_owned(),
                ));
            }
            if !names.insert((input.name.first.clone(), input.name.last.clone())) {
                return Err(NockchainProbeError::InvalidRequest(
                    "selected input snapshot contains a duplicate note name".to_owned(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NockchainInclusionFacts {
    pub height: u64,
    pub block_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NockchainProbeObservation {
    pub mempool_accepted: Option<bool>,
    pub tip_height: u64,
    pub inclusion: Option<NockchainInclusionFacts>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawSpendSeedFacts {
    pub output_source_transaction_id: Option<String>,
    pub output_source_is_coinbase: Option<bool>,
    pub lock_root: String,
    pub gift_nicks: u64,
    pub parent_hash: String,
    pub note_data_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawSpendFacts {
    pub input_name: NoteNameFacts,
    pub spend_kind: String,
    pub fee_nicks: u64,
    pub seeds: Vec<RawSpendSeedFacts>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalRawTransactionFacts {
    pub version: u64,
    pub embedded_transaction_id: String,
    pub computed_transaction_id: String,
    pub size_bytes: u64,
    pub spends: Vec<RawSpendFacts>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionOutputFacts {
    pub index: usize,
    pub name: NoteNameFacts,
    pub note_version: u64,
    pub assets_nicks: u64,
    pub lock_root: String,
    pub origin_height: u64,
    pub origin_transaction_id: String,
    pub origin_is_coinbase: bool,
    pub note_data_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NockchainTransactionFacts {
    pub transaction_id: String,
    pub inclusion: NockchainInclusionFacts,
    pub tip_height: u64,
    pub confirmation_depth: u64,
    pub inclusion_history: Vec<NockchainInclusionFacts>,
    pub raw_transaction: CanonicalRawTransactionFacts,
    pub input_snapshot: NockchainInputSnapshotFacts,
    pub selected_inputs: Vec<SelectedInputNoteFacts>,
    pub outputs: Vec<TransactionOutputFacts>,
    pub transaction_fee_nicks: u64,
    pub total_input_nicks: u64,
    pub total_output_nicks: u64,
    pub unaccounted_nicks: u64,
    pub matching_recipient_output_indices: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct NockchainObservedBlock {
    pub height: u64,
    pub block_id: String,
    pub transactions: Vec<(String, Tx)>,
}

#[async_trait]
pub trait NockchainProbeSource: Send {
    async fn observe_transaction(
        &mut self,
        transaction_id: &str,
    ) -> Result<NockchainProbeObservation, String>;

    async fn block_at_height(
        &mut self,
        height: u64,
    ) -> Result<Option<NockchainObservedBlock>, String>;
}

pub struct LiveNockchainProbeSource {
    public: PublicNockchainGrpcClient,
    private: NockGrpcSource,
}

impl LiveNockchainProbeSource {
    pub async fn connect(
        public_endpoint: &str,
        private_endpoint: &str,
    ) -> Result<Self, NockchainProbeError> {
        let public = PublicNockchainGrpcClient::connect(public_endpoint)
            .await
            .map_err(|error| NockchainProbeError::Source(error.to_string()))?;
        let private = NockGrpcSource::connect(private_endpoint.to_owned())
            .await
            .map_err(|error| NockchainProbeError::Source(error.to_string()))?;
        Ok(Self { public, private })
    }
}

#[async_trait]
impl NockchainProbeSource for LiveNockchainProbeSource {
    async fn observe_transaction(
        &mut self,
        transaction_id: &str,
    ) -> Result<NockchainProbeObservation, String> {
        let request = Base58Hash {
            hash: transaction_id.to_owned(),
        };
        let accepted = self
            .public
            .transaction_accepted(request.clone())
            .await
            .map_err(|error| error.to_string())?;
        let mempool_accepted = match accepted.result {
            Some(transaction_accepted_response::Result::Accepted(value)) => Some(value),
            Some(transaction_accepted_response::Result::Error(error)) => return Err(error.message),
            None => None,
        };
        let inclusion = self
            .public
            .get_transaction_block(request)
            .await
            .map_err(|error| error.to_string())?
            .map(|(height, block_id)| NockchainInclusionFacts {
                height,
                block_id: block_id.to_base58(),
            });
        let tip_height = self
            .public
            .explorer_heaviest_height()
            .await
            .map_err(|error| error.to_string())?;
        Ok(NockchainProbeObservation {
            mempool_accepted,
            tip_height,
            inclusion,
        })
    }

    async fn block_at_height(
        &mut self,
        height: u64,
    ) -> Result<Option<NockchainObservedBlock>, String> {
        let event = self
            .private
            .fetch_block_at_height(height)
            .await
            .map_err(|error| error.to_string())?;
        Ok(event.map(|event| NockchainObservedBlock {
            height: event.block.height,
            block_id: event.block.digest.to_base58(),
            transactions: event
                .txs
                .into_iter()
                .map(|(id, transaction)| (id.to_base58(), transaction))
                .collect(),
        }))
    }
}

pub async fn wait_for_nockchain_transaction<S: NockchainProbeSource>(
    source: &mut S,
    request: &NockchainProbeRequest,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<NockchainTransactionFacts, NockchainProbeError> {
    request.validate()?;
    if timeout.is_zero() {
        return Err(NockchainProbeError::InvalidRequest(
            "probe timeout must be positive".to_owned(),
        ));
    }
    let deadline = Instant::now() + timeout;
    let mut inclusion_history = Vec::new();
    loop {
        let observation = source
            .observe_transaction(&request.transaction_id)
            .await
            .map_err(NockchainProbeError::Source)?;
        if let Some(inclusion) = &observation.inclusion {
            validate_hash("included block id", &inclusion.block_id)?;
            if inclusion_history.last() != Some(inclusion) {
                inclusion_history.push(inclusion.clone());
            }
            if observation.tip_height >= inclusion.height {
                if let Some(block) = source
                    .block_at_height(inclusion.height)
                    .await
                    .map_err(NockchainProbeError::Source)?
                {
                    let confirmation_depth = observation.tip_height - inclusion.height;
                    if confirmation_depth >= request.confirmation_depth {
                        return decode_nockchain_transaction(
                            request, &observation, block, inclusion_history,
                        );
                    }
                }
            }
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(NockchainProbeError::Timeout {
                last_observation: Some(observation),
                inclusion_history,
            });
        }
        let pause = poll_interval.min(deadline.saturating_duration_since(now));
        if pause.is_zero() {
            tokio::task::yield_now().await;
        } else {
            sleep(pause).await;
        }
    }
}

pub fn decode_nockchain_transaction(
    request: &NockchainProbeRequest,
    observation: &NockchainProbeObservation,
    block: NockchainObservedBlock,
    inclusion_history: Vec<NockchainInclusionFacts>,
) -> Result<NockchainTransactionFacts, NockchainProbeError> {
    request.validate()?;
    let inclusion = observation.inclusion.clone().ok_or_else(|| {
        NockchainProbeError::Malformed("cannot decode a transaction without inclusion".to_owned())
    })?;
    if observation.tip_height < inclusion.height {
        return Err(NockchainProbeError::LaggingTip {
            tip_height: observation.tip_height,
            inclusion_height: inclusion.height,
        });
    }
    if request.input_snapshot.height >= inclusion.height {
        return Err(NockchainProbeError::Malformed(
            "selected input snapshot must precede transaction inclusion".to_owned(),
        ));
    }
    if block.height != inclusion.height || block.block_id != inclusion.block_id {
        return Err(NockchainProbeError::Malformed(
            "private block identity does not match public inclusion".to_owned(),
        ));
    }
    let (_, transaction) = block
        .transactions
        .into_iter()
        .find(|(transaction_id, _)| transaction_id == &request.transaction_id)
        .ok_or_else(|| {
            NockchainProbeError::Malformed(
                "included block does not contain the requested transaction".to_owned(),
            )
        })?;
    let Tx::V1(transaction) = transaction;
    if transaction.version != 1 {
        return Err(NockchainProbeError::Malformed(format!(
            "unsupported canonical transaction version {}",
            transaction.version
        )));
    }
    let raw = &transaction.raw_tx;
    let computed_transaction_id = raw
        .compute_id_base58()
        .map_err(|error| NockchainProbeError::Malformed(error.to_string()))?;
    let embedded_transaction_id = raw.id.to_base58();
    if embedded_transaction_id != request.transaction_id
        || computed_transaction_id != request.transaction_id
    {
        return Err(NockchainProbeError::Malformed(
            "canonical raw transaction id does not match requested inclusion".to_owned(),
        ));
    }

    let mut spends = decode_spends(raw)?;
    spends.sort_by(|left, right| {
        note_name_key(&left.input_name).cmp(&note_name_key(&right.input_name))
    });
    let actual_input_names = spends
        .iter()
        .map(|spend| spend.input_name.clone())
        .collect::<Vec<_>>();
    let mut selected_inputs = request.selected_inputs.clone();
    selected_inputs
        .sort_by(|left, right| note_name_key(&left.name).cmp(&note_name_key(&right.name)));
    let expected_input_names = selected_inputs
        .iter()
        .map(|input| input.name.clone())
        .collect::<Vec<_>>();
    if actual_input_names != expected_input_names {
        return Err(NockchainProbeError::InputSnapshotMismatch {
            expected: expected_input_names,
            observed: actual_input_names,
        });
    }

    let transaction_fee_nicks = spends.iter().try_fold(0_u64, |total, spend| {
        total
            .checked_add(spend.fee_nicks)
            .ok_or(NockchainProbeError::ArithmeticOverflow)
    })?;
    let total_input_nicks = selected_inputs.iter().try_fold(0_u64, |total, input| {
        total
            .checked_add(input.assets_nicks)
            .ok_or(NockchainProbeError::ArithmeticOverflow)
    })?;
    let mut outputs = transaction
        .outputs
        .0
        .into_iter()
        .map(|output| decode_output(output, &request.transaction_id))
        .collect::<Result<Vec<_>, _>>()?;
    if outputs
        .iter()
        .any(|output| output.origin_height != inclusion.height)
    {
        return Err(NockchainProbeError::Malformed(
            "canonical output origin height does not match inclusion".to_owned(),
        ));
    }
    outputs.sort_by(|left, right| note_name_key(&left.name).cmp(&note_name_key(&right.name)));
    for (index, output) in outputs.iter_mut().enumerate() {
        output.index = index;
    }
    let total_output_nicks = outputs.iter().try_fold(0_u64, |total, output| {
        total
            .checked_add(output.assets_nicks)
            .ok_or(NockchainProbeError::ArithmeticOverflow)
    })?;
    let accounted = total_output_nicks
        .checked_add(transaction_fee_nicks)
        .ok_or(NockchainProbeError::ArithmeticOverflow)?;
    let unaccounted_nicks = total_input_nicks.checked_sub(accounted).ok_or_else(|| {
        NockchainProbeError::Malformed(
            "transaction outputs and fee exceed selected input assets".to_owned(),
        )
    })?;
    let matching_recipient_output_indices = outputs
        .iter()
        .filter(|output| output.lock_root == request.recipient_lock_root)
        .map(|output| output.index)
        .collect();

    Ok(NockchainTransactionFacts {
        transaction_id: request.transaction_id.clone(),
        inclusion,
        tip_height: observation.tip_height,
        confirmation_depth: observation.tip_height - block.height,
        inclusion_history,
        raw_transaction: CanonicalRawTransactionFacts {
            version: transaction.version,
            embedded_transaction_id,
            computed_transaction_id,
            size_bytes: transaction.total_size,
            spends,
        },
        input_snapshot: request.input_snapshot.clone(),
        selected_inputs,
        outputs,
        transaction_fee_nicks,
        total_input_nicks,
        total_output_nicks,
        unaccounted_nicks,
        matching_recipient_output_indices,
    })
}

fn decode_spends(raw: &RawTx) -> Result<Vec<RawSpendFacts>, NockchainProbeError> {
    raw.spends
        .0
        .iter()
        .map(|(name, spend)| {
            let (spend_kind, fee, seeds) = match spend {
                Spend::Legacy(spend) => ("legacy", &spend.fee, &spend.seeds),
                Spend::Witness(spend) => ("witness", &spend.fee, &spend.seeds),
            };
            Ok(RawSpendFacts {
                input_name: NoteNameFacts::from_name(name),
                spend_kind: spend_kind.to_owned(),
                fee_nicks: u64::try_from(fee.0)
                    .map_err(|_| NockchainProbeError::ArithmeticOverflow)?,
                seeds: decode_seeds(seeds)?,
            })
        })
        .collect()
}

fn decode_seeds(seeds: &Seeds) -> Result<Vec<RawSpendSeedFacts>, NockchainProbeError> {
    let mut facts = seeds
        .0
        .iter()
        .map(|seed| {
            Ok(RawSpendSeedFacts {
                output_source_transaction_id: seed
                    .output_source
                    .as_ref()
                    .map(|source| source.hash.to_base58()),
                output_source_is_coinbase: seed
                    .output_source
                    .as_ref()
                    .map(|source| source.is_coinbase),
                lock_root: seed.lock_root.to_base58(),
                gift_nicks: u64::try_from(seed.gift.0)
                    .map_err(|_| NockchainProbeError::ArithmeticOverflow)?,
                parent_hash: seed.parent_hash.to_base58(),
                note_data_keys: note_data_keys(&seed.note_data),
            })
        })
        .collect::<Result<Vec<_>, NockchainProbeError>>()?;
    facts.sort_by(|left, right| {
        (left.lock_root.as_str(), left.parent_hash.as_str())
            .cmp(&(right.lock_root.as_str(), right.parent_hash.as_str()))
    });
    Ok(facts)
}

fn decode_output(
    output: OutputV1,
    included_transaction_id: &str,
) -> Result<TransactionOutputFacts, NockchainProbeError> {
    match output.note {
        Note::V0(note) => Ok(TransactionOutputFacts {
            index: 0,
            name: NoteNameFacts::from_name(&note.tail.name),
            note_version: 0,
            assets_nicks: u64::try_from(note.tail.assets.0)
                .map_err(|_| NockchainProbeError::ArithmeticOverflow)?,
            lock_root: note.tail.name.first.to_base58(),
            origin_height: note.head.origin_page.0 .0,
            origin_transaction_id: note.tail.source.hash.to_base58(),
            origin_is_coinbase: note.tail.source.is_coinbase,
            note_data_keys: vec!["legacy-lock".to_owned()],
        }),
        Note::V1(note) => {
            let seed = output.seeds.0.first().ok_or_else(|| {
                NockchainProbeError::Malformed("canonical V1 output has no seeds".to_owned())
            })?;
            if output
                .seeds
                .0
                .iter()
                .any(|candidate| candidate.lock_root != seed.lock_root)
            {
                return Err(NockchainProbeError::Malformed(
                    "canonical V1 output combines different lock roots".to_owned(),
                ));
            }
            let expected_first_name = FirstName::from_lock_root(&seed.lock_root)
                .map_err(|error| NockchainProbeError::Malformed(error.to_string()))?
                .into_hash();
            if note.name.first != expected_first_name {
                return Err(NockchainProbeError::Malformed(
                    "canonical V1 output name does not match its seed lock root".to_owned(),
                ));
            }
            Ok(TransactionOutputFacts {
                index: 0,
                name: NoteNameFacts::from_name(&note.name),
                note_version: 1,
                assets_nicks: u64::try_from(note.assets.0)
                    .map_err(|_| NockchainProbeError::ArithmeticOverflow)?,
                lock_root: seed.lock_root.to_base58(),
                origin_height: note.origin_page.0 .0,
                origin_transaction_id: included_transaction_id.to_owned(),
                origin_is_coinbase: false,
                note_data_keys: note_data_keys(&note.note_data),
            })
        }
    }
}

fn note_data_keys(note_data: &NoteData) -> Vec<String> {
    let mut keys = note_data
        .0
        .iter()
        .map(|entry| entry.key.clone())
        .collect::<Vec<_>>();
    keys.sort();
    keys
}

fn note_name_key(name: &NoteNameFacts) -> (&str, &str) {
    (&name.first, &name.last)
}

fn validate_hash(field: &'static str, value: &str) -> Result<Hash, NockchainProbeError> {
    Hash::from_base58(value)
        .map_err(|error| NockchainProbeError::InvalidRequest(format!("invalid {field}: {error}")))
}

#[derive(Debug, Error)]
pub enum NockchainProbeError {
    #[error("invalid Nockchain probe request: {0}")]
    InvalidRequest(String),
    #[error("Nockchain probe source failed: {0}")]
    Source(String),
    #[error("Nockchain probe timed out")]
    Timeout {
        last_observation: Option<NockchainProbeObservation>,
        inclusion_history: Vec<NockchainInclusionFacts>,
    },
    #[error("Nockchain public tip {tip_height} is behind inclusion {inclusion_height}")]
    LaggingTip {
        tip_height: u64,
        inclusion_height: u64,
    },
    #[error("malformed Nockchain transaction evidence: {0}")]
    Malformed(String),
    #[error("selected input snapshot does not match raw transaction spends")]
    InputSnapshotMismatch {
        expected: Vec<NoteNameFacts>,
        observed: Vec<NoteNameFacts>,
    },
    #[error("Nockchain transaction arithmetic overflow")]
    ArithmeticOverflow,
}
