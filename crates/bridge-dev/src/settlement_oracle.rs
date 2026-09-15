use std::collections::{BTreeMap, HashSet};
use std::str::FromStr;
use std::time::Duration;

use alloy::primitives::U256;
use async_trait::async_trait;
use bridge::shared::types::{
    WITHDRAWAL_POLICY_V1_BASE_UNITS_PER_NICK,
    WITHDRAWAL_POLICY_V1_BRIDGE_FEE_NICKS_PER_STARTED_NOCK,
    WITHDRAWAL_POLICY_V1_MINIMUM_GROSS_NOCKS, WITHDRAWAL_POLICY_V1_NICKS_PER_NOCK,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::time::{sleep, Instant};

use crate::iris_driver::BurnEventProof;
use crate::nockchain_probe::{NockchainTransactionFacts, NoteNameFacts, TransactionOutputFacts};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExactNicks(pub String);

impl From<u64> for ExactNicks {
    fn from(value: u64) -> Self {
        Self(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EquationTerm {
    pub name: String,
    pub nicks: ExactNicks,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArithmeticEquationProof {
    pub name: String,
    pub left: EquationTerm,
    pub right_terms: Vec<EquationTerm>,
    pub right_total: ExactNicks,
    pub verdict: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementOutputProof {
    pub index: usize,
    pub name: NoteNameFacts,
    pub lock_root: String,
    pub assets_nicks: ExactNicks,
    pub note_version: u64,
    pub origin_height: u64,
    pub origin_transaction_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementConservationProof {
    pub schema_version: u64,
    pub base_event_id: String,
    pub nock_transaction_id: String,
    pub gross_burn_nicks: ExactNicks,
    pub bridge_fee_nicks: ExactNicks,
    pub transaction_fee_nicks: ExactNicks,
    pub recipient_payout_nicks: ExactNicks,
    pub recipient_lock_root: String,
    pub recipient_output: SettlementOutputProof,
    pub change_outputs: Vec<SettlementOutputProof>,
    pub change_total_nicks: ExactNicks,
    pub total_input_nicks: ExactNicks,
    pub total_output_nicks: ExactNicks,
    pub input_note_version_counts: BTreeMap<u64, usize>,
    pub transaction_conservation: ArithmeticEquationProof,
    pub burn_to_payout: ArithmeticEquationProof,
}

pub struct SettlementOracle;

impl SettlementOracle {
    pub fn prove(
        burn: &BurnEventProof,
        transaction: &NockchainTransactionFacts,
    ) -> Result<SettlementConservationProof, SettlementOracleError> {
        let gross_burn_nicks = parse_u64("burn amount nicks", &burn.amount_nicks)?;
        let gross_base_units = U256::from_str(&burn.amount_base_units)
            .map_err(|_| SettlementOracleError::Malformed("burn base units are not uint256"))?;
        let base_units_per_nick = U256::from(WITHDRAWAL_POLICY_V1_BASE_UNITS_PER_NICK);
        if gross_base_units == U256::ZERO
            || gross_base_units % base_units_per_nick != U256::ZERO
            || gross_base_units / base_units_per_nick != U256::from(gross_burn_nicks)
        {
            return Err(SettlementOracleError::BurnAmountMismatch);
        }
        let minimum_nicks = WITHDRAWAL_POLICY_V1_MINIMUM_GROSS_NOCKS
            .checked_mul(WITHDRAWAL_POLICY_V1_NICKS_PER_NOCK)
            .ok_or(SettlementOracleError::ArithmeticOverflow)?;
        if gross_burn_nicks < minimum_nicks {
            return Err(SettlementOracleError::BelowPolicyMinimum {
                observed: gross_burn_nicks,
                minimum: minimum_nicks,
            });
        }
        if burn.lock_root.trim().is_empty() {
            return Err(SettlementOracleError::Malformed(
                "burn lock root is missing",
            ));
        }

        let started_nocks = gross_burn_nicks
            .checked_add(WITHDRAWAL_POLICY_V1_NICKS_PER_NOCK - 1)
            .ok_or(SettlementOracleError::ArithmeticOverflow)?
            / WITHDRAWAL_POLICY_V1_NICKS_PER_NOCK;
        let bridge_fee_nicks = started_nocks
            .checked_mul(WITHDRAWAL_POLICY_V1_BRIDGE_FEE_NICKS_PER_STARTED_NOCK)
            .ok_or(SettlementOracleError::ArithmeticOverflow)?;
        let transaction_fee_nicks = checked_sum(
            transaction
                .raw_transaction
                .spends
                .iter()
                .map(|spend| spend.fee_nicks),
        )?;
        if transaction_fee_nicks != transaction.transaction_fee_nicks {
            return Err(SettlementOracleError::EvidenceMismatch(
                "raw spend fee sum differs from transaction fee fact",
            ));
        }

        let total_input_nicks = checked_sum(
            transaction
                .selected_inputs
                .iter()
                .map(|input| input.assets_nicks),
        )?;
        let total_output_nicks =
            checked_sum(transaction.outputs.iter().map(|output| output.assets_nicks))?;
        if total_input_nicks != transaction.total_input_nicks
            || total_output_nicks != transaction.total_output_nicks
        {
            return Err(SettlementOracleError::EvidenceMismatch(
                "recomputed input/output sums differ from transaction facts",
            ));
        }
        let accounted_outputs = total_output_nicks
            .checked_add(transaction_fee_nicks)
            .ok_or(SettlementOracleError::ArithmeticOverflow)?;
        if total_input_nicks != accounted_outputs || transaction.unaccounted_nicks != 0 {
            return Err(SettlementOracleError::EquationFailed {
                equation: "transaction_conservation",
                left: total_input_nicks,
                right: accounted_outputs,
            });
        }

        validate_output_set(&transaction.outputs)?;
        let candidate_indices = transaction
            .outputs
            .iter()
            .filter(|output| output.lock_root == burn.lock_root)
            .map(|output| output.index)
            .collect::<Vec<_>>();
        if candidate_indices != transaction.matching_recipient_output_indices {
            return Err(SettlementOracleError::EvidenceMismatch(
                "recipient candidates differ from transaction probe facts",
            ));
        }
        let recipient_index = match candidate_indices.as_slice() {
            [] => {
                return Err(SettlementOracleError::MissingRecipient(
                    burn.lock_root.clone(),
                ))
            }
            [index] => *index,
            indices => {
                return Err(SettlementOracleError::DuplicateRecipient {
                    lock_root: burn.lock_root.clone(),
                    count: indices.len(),
                })
            }
        };
        let recipient = transaction
            .outputs
            .iter()
            .find(|output| output.index == recipient_index)
            .ok_or(SettlementOracleError::EvidenceMismatch(
                "recipient output index is missing",
            ))?;
        if recipient.assets_nicks == 0 {
            return Err(SettlementOracleError::ZeroRecipientPayout);
        }
        let expected_payout = gross_burn_nicks
            .checked_sub(bridge_fee_nicks)
            .and_then(|value| value.checked_sub(transaction_fee_nicks))
            .ok_or(SettlementOracleError::FeeExceedsBurn {
                gross: gross_burn_nicks,
                bridge_fee: bridge_fee_nicks,
                transaction_fee: transaction_fee_nicks,
            })?;
        if recipient.assets_nicks != expected_payout {
            return Err(SettlementOracleError::EquationFailed {
                equation: "burn_to_payout",
                left: gross_burn_nicks,
                right: bridge_fee_nicks
                    .checked_add(transaction_fee_nicks)
                    .and_then(|value| value.checked_add(recipient.assets_nicks))
                    .ok_or(SettlementOracleError::ArithmeticOverflow)?,
            });
        }

        let change_outputs = transaction
            .outputs
            .iter()
            .filter(|output| output.index != recipient_index)
            .map(SettlementOutputProof::from)
            .collect::<Vec<_>>();
        let change_total_nicks = checked_sum(
            transaction
                .outputs
                .iter()
                .filter(|output| output.index != recipient_index)
                .map(|output| output.assets_nicks),
        )?;
        let input_note_version_counts =
            transaction
                .selected_inputs
                .iter()
                .fold(BTreeMap::new(), |mut counts, input| {
                    *counts.entry(input.note_version).or_insert(0) += 1;
                    counts
                });

        Ok(SettlementConservationProof {
            schema_version: 1,
            base_event_id: burn.base_event_id.clone(),
            nock_transaction_id: transaction.transaction_id.clone(),
            gross_burn_nicks: gross_burn_nicks.into(),
            bridge_fee_nicks: bridge_fee_nicks.into(),
            transaction_fee_nicks: transaction_fee_nicks.into(),
            recipient_payout_nicks: recipient.assets_nicks.into(),
            recipient_lock_root: burn.lock_root.clone(),
            recipient_output: SettlementOutputProof::from(recipient),
            change_outputs,
            change_total_nicks: change_total_nicks.into(),
            total_input_nicks: total_input_nicks.into(),
            total_output_nicks: total_output_nicks.into(),
            input_note_version_counts,
            transaction_conservation: equation(
                "transaction_conservation",
                "total_input_nicks",
                total_input_nicks,
                [
                    ("total_output_nicks", total_output_nicks),
                    ("transaction_fee_nicks", transaction_fee_nicks),
                ],
            )?,
            burn_to_payout: equation(
                "burn_to_payout",
                "gross_burn_nicks",
                gross_burn_nicks,
                [
                    ("bridge_fee_nicks", bridge_fee_nicks),
                    ("transaction_fee_nicks", transaction_fee_nicks),
                    ("recipient_payout_nicks", recipient.assets_nicks),
                ],
            )?,
        })
    }
}

impl From<&TransactionOutputFacts> for SettlementOutputProof {
    fn from(output: &TransactionOutputFacts) -> Self {
        Self {
            index: output.index,
            name: output.name.clone(),
            lock_root: output.lock_root.clone(),
            assets_nicks: output.assets_nicks.into(),
            note_version: output.note_version,
            origin_height: output.origin_height,
            origin_transaction_id: output.origin_transaction_id.clone(),
        }
    }
}

fn validate_output_set(outputs: &[TransactionOutputFacts]) -> Result<(), SettlementOracleError> {
    if outputs.is_empty() {
        return Err(SettlementOracleError::MissingFacts("transaction outputs"));
    }
    let mut indices = HashSet::with_capacity(outputs.len());
    let mut names = HashSet::with_capacity(outputs.len());
    for output in outputs {
        if !indices.insert(output.index) {
            return Err(SettlementOracleError::EvidenceMismatch(
                "duplicate output index",
            ));
        }
        if !names.insert((output.name.first.clone(), output.name.last.clone())) {
            return Err(SettlementOracleError::EvidenceMismatch(
                "duplicate output note name",
            ));
        }
    }
    if !(0..outputs.len()).all(|index| indices.contains(&index)) {
        return Err(SettlementOracleError::EvidenceMismatch(
            "output indices are not contiguous",
        ));
    }
    Ok(())
}

fn equation<const N: usize>(
    name: &str,
    left_name: &str,
    left: u64,
    right: [(&str, u64); N],
) -> Result<ArithmeticEquationProof, SettlementOracleError> {
    let right_total = checked_sum(right.iter().map(|(_, value)| *value))?;
    if left != right_total {
        return Err(SettlementOracleError::EquationFailed {
            equation: if name == "burn_to_payout" {
                "burn_to_payout"
            } else {
                "transaction_conservation"
            },
            left,
            right: right_total,
        });
    }
    Ok(ArithmeticEquationProof {
        name: name.to_owned(),
        left: EquationTerm {
            name: left_name.to_owned(),
            nicks: left.into(),
        },
        right_terms: right
            .into_iter()
            .map(|(term, value)| EquationTerm {
                name: term.to_owned(),
                nicks: value.into(),
            })
            .collect(),
        right_total: right_total.into(),
        verdict: true,
    })
}

fn checked_sum(values: impl IntoIterator<Item = u64>) -> Result<u64, SettlementOracleError> {
    values.into_iter().try_fold(0_u64, |total, value| {
        total
            .checked_add(value)
            .ok_or(SettlementOracleError::ArithmeticOverflow)
    })
}

fn parse_u64(field: &'static str, value: &str) -> Result<u64, SettlementOracleError> {
    value
        .parse()
        .map_err(|_| SettlementOracleError::InvalidInteger(field))
}

#[derive(Debug, Error)]
pub enum SettlementOracleError {
    #[error("missing settlement fact: {0}")]
    MissingFacts(&'static str),
    #[error("invalid exact integer for {0}")]
    InvalidInteger(&'static str),
    #[error("malformed settlement fact: {0}")]
    Malformed(&'static str),
    #[error("Base burn amount/base-unit facts disagree")]
    BurnAmountMismatch,
    #[error("gross burn {observed} nicks is below policy minimum {minimum}")]
    BelowPolicyMinimum { observed: u64, minimum: u64 },
    #[error("settlement arithmetic overflow")]
    ArithmeticOverflow,
    #[error("settlement evidence mismatch: {0}")]
    EvidenceMismatch(&'static str),
    #[error("settlement equation {equation} failed: left={left}, right={right}")]
    EquationFailed {
        equation: &'static str,
        left: u64,
        right: u64,
    },
    #[error("recipient output for lock root {0} is missing")]
    MissingRecipient(String),
    #[error("recipient lock root {lock_root} has {count} outputs; expected exactly one")]
    DuplicateRecipient { lock_root: String, count: usize },
    #[error("recipient payout is zero")]
    ZeroRecipientPayout,
    #[error(
        "bridge and transaction fees exceed burn: gross={gross}, bridge={bridge_fee}, tx={transaction_fee}"
    )]
    FeeExceedsBurn {
        gross: u64,
        bridge_fee: u64,
        transaction_fee: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimedTerminalFact<T> {
    pub observed_unix_ms: u64,
    pub source_name: String,
    pub correlation_group: String,
    pub facts: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelFrontierFacts {
    pub height: u64,
    pub block_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeKernelTerminalFacts {
    pub node_id: u64,
    pub available: bool,
    pub running: bool,
    pub target_withdrawal_id: String,
    pub target_base_event_id: String,
    pub hold_reason: Option<String>,
    pub frontier: KernelFrontierFacts,
    pub matching_unsettled_withdrawal: bool,
    pub other_unsettled_withdrawals: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SequencerTerminalState {
    Pending,
    MempoolAccepted,
    Confirmed,
    ReorgHold,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequencerTerminalFacts {
    pub withdrawal_id: String,
    pub withdrawal_nonce: u64,
    pub transaction_id: Option<String>,
    pub state: SequencerTerminalState,
    pub confirmation_event_id: Option<String>,
    pub confirmed_height: Option<u64>,
    pub confirmed_block_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReservationTerminalFacts {
    pub withdrawal_id: String,
    pub tracked_inputs: Vec<NoteNameFacts>,
    pub release_event_ids: Vec<String>,
    pub currently_reserved_inputs: Vec<NoteNameFacts>,
    pub release_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicWithdrawalState {
    Pending,
    MempoolAccepted,
    Confirmed,
    ReorgHold,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicWithdrawalTerminalFacts {
    pub withdrawal_id: String,
    pub withdrawal_nonce: u64,
    pub state: PublicWithdrawalState,
    pub base_event_id: String,
    pub transaction_id: Option<String>,
    pub confirmed_height: Option<u64>,
    pub confirmed_block_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalWithdrawalTarget {
    pub withdrawal_id: String,
    pub withdrawal_nonce: u64,
    pub base_event_id: String,
    pub transaction_id: String,
    pub confirmation_depth: u64,
    pub reserved_inputs: Vec<NoteNameFacts>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalLastFacts {
    pub chain: Option<TimedTerminalFact<NockchainTransactionFacts>>,
    pub kernels: Option<TimedTerminalFact<Vec<BridgeKernelTerminalFacts>>>,
    pub sequencer: Option<TimedTerminalFact<SequencerTerminalFacts>>,
    pub reservations: Option<TimedTerminalFact<ReservationTerminalFacts>>,
    pub public: Option<TimedTerminalFact<PublicWithdrawalTerminalFacts>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalWithdrawalProof {
    pub schema_version: u64,
    pub target: TerminalWithdrawalTarget,
    pub settlement: SettlementConservationProof,
    pub chain: TimedTerminalFact<NockchainTransactionFacts>,
    pub kernels: TimedTerminalFact<Vec<BridgeKernelTerminalFacts>>,
    pub sequencer: TimedTerminalFact<SequencerTerminalFacts>,
    pub reservations: TimedTerminalFact<ReservationTerminalFacts>,
    pub public: TimedTerminalFact<PublicWithdrawalTerminalFacts>,
    pub stable_observations: u64,
    pub diagnostics: Vec<String>,
}

#[async_trait]
pub trait TerminalChainSource: Send {
    async fn observe_chain(
        &mut self,
    ) -> Result<TimedTerminalFact<NockchainTransactionFacts>, String>;
}

#[async_trait]
pub trait TerminalKernelSource: Send {
    async fn observe_kernels(
        &mut self,
    ) -> Result<TimedTerminalFact<Vec<BridgeKernelTerminalFacts>>, String>;
}

#[async_trait]
pub trait TerminalSequencerSource: Send {
    async fn observe_sequencer(
        &mut self,
    ) -> Result<TimedTerminalFact<SequencerTerminalFacts>, String>;
}

#[async_trait]
pub trait TerminalReservationSource: Send {
    async fn observe_reservations(
        &mut self,
    ) -> Result<TimedTerminalFact<ReservationTerminalFacts>, String>;
}

#[async_trait]
pub trait TerminalPublicSource: Send {
    async fn observe_public(
        &mut self,
    ) -> Result<TimedTerminalFact<PublicWithdrawalTerminalFacts>, String>;
}

pub struct TerminalOracleSources<'a, C, K, S, R, P> {
    pub chain: &'a mut C,
    pub kernels: &'a mut K,
    pub sequencer: &'a mut S,
    pub reservations: &'a mut R,
    pub public: &'a mut P,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TerminalObservationTimes {
    chain: u64,
    kernels: u64,
    sequencer: u64,
    reservations: u64,
    public: u64,
}

fn terminal_observation_times(last: &TerminalLastFacts) -> Option<TerminalObservationTimes> {
    Some(TerminalObservationTimes {
        chain: last.chain.as_ref()?.observed_unix_ms,
        kernels: last.kernels.as_ref()?.observed_unix_ms,
        sequencer: last.sequencer.as_ref()?.observed_unix_ms,
        reservations: last.reservations.as_ref()?.observed_unix_ms,
        public: last.public.as_ref()?.observed_unix_ms,
    })
}

fn terminal_observations_are_fresh(
    previous: TerminalObservationTimes,
    current: TerminalObservationTimes,
    diagnostics: &mut Vec<String>,
) -> bool {
    let sources = [
        ("chain", previous.chain, current.chain),
        ("kernels", previous.kernels, current.kernels),
        ("sequencer", previous.sequencer, current.sequencer),
        ("reservations", previous.reservations, current.reservations),
        ("public", previous.public, current.public),
    ];
    let mut fresh = true;
    for (source, prior, observed) in sources {
        if observed <= prior {
            push_diagnostic(
                diagnostics,
                source,
                &format!(
                    "observation timestamp did not advance: previous={prior}, current={observed}"
                ),
            );
            fresh = false;
        }
    }
    fresh
}

pub async fn wait_for_terminal_withdrawal<C, K, S, R, P>(
    target: &TerminalWithdrawalTarget,
    settlement: &SettlementConservationProof,
    sources: &mut TerminalOracleSources<'_, C, K, S, R, P>,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<TerminalWithdrawalProof, TerminalOracleError>
where
    C: TerminalChainSource,
    K: TerminalKernelSource,
    S: TerminalSequencerSource,
    R: TerminalReservationSource,
    P: TerminalPublicSource,
{
    validate_terminal_request(target, settlement, timeout)?;
    let deadline = Instant::now() + timeout;
    let mut last: TerminalLastFacts;
    let mut diagnostics = Vec::new();
    let mut previous_stable_key = None;
    let mut stable_observations = 0_u64;
    let mut previous_observation_times = None;
    loop {
        let mut round = TerminalLastFacts::default();
        match sources.chain.observe_chain().await {
            Ok(facts) => round.chain = Some(facts),
            Err(error) => push_diagnostic(&mut diagnostics, "chain", &error),
        }
        match sources.kernels.observe_kernels().await {
            Ok(facts) => round.kernels = Some(facts),
            Err(error) => push_diagnostic(&mut diagnostics, "kernels", &error),
        }
        match sources.sequencer.observe_sequencer().await {
            Ok(facts) => round.sequencer = Some(facts),
            Err(error) => push_diagnostic(&mut diagnostics, "sequencer", &error),
        }
        match sources.reservations.observe_reservations().await {
            Ok(facts) => round.reservations = Some(facts),
            Err(error) => push_diagnostic(&mut diagnostics, "reservations", &error),
        }
        match sources.public.observe_public().await {
            Ok(facts) => round.public = Some(facts),
            Err(error) => push_diagnostic(&mut diagnostics, "public", &error),
        }
        last = round;

        let current_observation_times = terminal_observation_times(&last);
        let fresh_round = match (previous_observation_times, current_observation_times) {
            (Some(previous), Some(current)) => {
                terminal_observations_are_fresh(previous, current, &mut diagnostics)
            }
            (None, Some(_)) => true,
            (_, None) => false,
        };
        previous_observation_times = current_observation_times;
        if fresh_round {
            match terminal_stable_key(target, &last, &mut diagnostics)? {
                Some(key) => {
                    if previous_stable_key.as_ref() == Some(&key) {
                        stable_observations = stable_observations.checked_add(1).ok_or(
                            TerminalOracleError::InvalidRequest(
                                "stable observation count overflow",
                            ),
                        )?;
                    } else {
                        previous_stable_key = Some(key);
                        stable_observations = 1;
                    }
                    if stable_observations >= 2 {
                        return terminal_proof(
                            target, settlement, last, stable_observations, diagnostics,
                        );
                    }
                }
                None => {
                    previous_stable_key = None;
                    stable_observations = 0;
                }
            }
        } else {
            previous_stable_key = None;
            stable_observations = 0;
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(TerminalOracleError::Timeout {
                last: Box::new(last),
                diagnostics,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalStableKey {
    chain_height: u64,
    chain_block_id: String,
    sequencer_height: u64,
    sequencer_block_id: String,
    reservation_release_count: u64,
    public_height: u64,
    public_block_id: String,
}

fn validate_terminal_attribution<T>(
    source_name: &'static str,
    observation: &TimedTerminalFact<T>,
    last: &TerminalLastFacts,
) -> Result<(), TerminalOracleError> {
    if observation.source_name.trim().is_empty() || observation.correlation_group.trim().is_empty()
    {
        return Err(TerminalOracleError::Mismatch {
            source_name,
            reason: "evidence source or correlation group label is empty".to_owned(),
            last: Box::new(last.clone()),
        });
    }
    Ok(())
}

fn terminal_stable_key(
    target: &TerminalWithdrawalTarget,
    last: &TerminalLastFacts,
    diagnostics: &mut Vec<String>,
) -> Result<Option<TerminalStableKey>, TerminalOracleError> {
    let Some(chain) = &last.chain else {
        push_diagnostic(diagnostics, "chain", "no successful observation");
        return Ok(None);
    };
    validate_terminal_attribution("chain", chain, last)?;
    if chain.facts.transaction_id != target.transaction_id {
        return Err(TerminalOracleError::Mismatch {
            source_name: "chain",
            reason: "included transaction id differs from target".to_owned(),
            last: Box::new(last.clone()),
        });
    }
    if chain.facts.confirmation_depth < target.confirmation_depth {
        push_diagnostic(diagnostics, "chain", "confirmation depth has not converged");
        return Ok(None);
    }
    let chain_height = chain.facts.inclusion.height;
    let chain_block_id = chain.facts.inclusion.block_id.clone();
    if normalized_note_names(
        &chain
            .facts
            .selected_inputs
            .iter()
            .map(|input| input.name.clone())
            .collect::<Vec<_>>(),
    ) != normalized_note_names(&target.reserved_inputs)
    {
        return Err(TerminalOracleError::Mismatch {
            source_name: "chain",
            reason: "included transaction inputs differ from reserved target inputs".to_owned(),
            last: Box::new(last.clone()),
        });
    }

    let Some(kernels) = &last.kernels else {
        push_diagnostic(diagnostics, "kernels", "no successful observation");
        return Ok(None);
    };
    validate_terminal_attribution("kernels", kernels, last)?;
    if kernels.facts.len() != 5 {
        return Err(TerminalOracleError::Mismatch {
            source_name: "kernels",
            reason: format!(
                "expected five bridge snapshots, observed {}",
                kernels.facts.len()
            ),
            last: Box::new(last.clone()),
        });
    }
    let mut node_ids = HashSet::with_capacity(5);
    for kernel in &kernels.facts {
        if !node_ids.insert(kernel.node_id) || kernel.node_id >= 5 {
            return Err(TerminalOracleError::Mismatch {
                source_name: "kernels",
                reason: "bridge node ids are missing or duplicated".to_owned(),
                last: Box::new(last.clone()),
            });
        }
        if kernel.target_withdrawal_id != target.withdrawal_id
            || kernel.target_base_event_id != target.base_event_id
        {
            return Err(TerminalOracleError::Mismatch {
                source_name: "kernels",
                reason: format!(
                    "bridge-{} observed another withdrawal identity",
                    kernel.node_id
                ),
                last: Box::new(last.clone()),
            });
        }
        if !kernel.available {
            push_diagnostic(
                diagnostics,
                "kernels",
                &format!("bridge-{} is unavailable", kernel.node_id),
            );
            return Ok(None);
        }
        if !kernel.running || kernel.hold_reason.is_some() {
            return Err(TerminalOracleError::KernelStoppedOrHeld {
                node_id: kernel.node_id,
                reason: kernel
                    .hold_reason
                    .clone()
                    .unwrap_or_else(|| "not running".to_owned()),
                last: Box::new(last.clone()),
            });
        }
        if kernel.matching_unsettled_withdrawal || kernel.frontier.height < chain_height {
            push_diagnostic(
                diagnostics,
                "kernels",
                &format!("bridge-{} has not reconciled target", kernel.node_id),
            );
            return Ok(None);
        }
    }

    let Some(sequencer) = &last.sequencer else {
        push_diagnostic(diagnostics, "sequencer", "no successful observation");
        return Ok(None);
    };
    validate_terminal_attribution("sequencer", sequencer, last)?;
    validate_sequencer_identity(target, &sequencer.facts, last)?;
    match sequencer.facts.state {
        SequencerTerminalState::ReorgHold => {
            return Err(TerminalOracleError::ReorgHold {
                source_name: "sequencer",
                last: Box::new(last.clone()),
            })
        }
        SequencerTerminalState::Failed => {
            return Err(TerminalOracleError::Mismatch {
                source_name: "sequencer",
                reason: "sequencer reports terminal failure".to_owned(),
                last: Box::new(last.clone()),
            })
        }
        SequencerTerminalState::Confirmed => {}
        _ => {
            push_diagnostic(diagnostics, "sequencer", "withdrawal is not confirmed");
            return Ok(None);
        }
    }
    let (Some(sequencer_transaction_id), Some(sequencer_height), Some(sequencer_block_id)) = (
        sequencer.facts.transaction_id.as_deref(),
        sequencer.facts.confirmed_height,
        sequencer.facts.confirmed_block_id.as_ref(),
    ) else {
        push_diagnostic(diagnostics, "sequencer", "confirmed reference is missing");
        return Ok(None);
    };
    if sequencer
        .facts
        .confirmation_event_id
        .as_deref()
        .is_none_or(str::is_empty)
    {
        return Err(TerminalOracleError::Mismatch {
            source_name: "sequencer",
            reason: "confirmed state lacks a direct durable confirmation event id".to_owned(),
            last: Box::new(last.clone()),
        });
    }
    if sequencer_transaction_id != target.transaction_id {
        return Err(TerminalOracleError::Mismatch {
            source_name: "sequencer",
            reason: "confirmed transaction differs from target".to_owned(),
            last: Box::new(last.clone()),
        });
    }
    if sequencer_height != chain_height || sequencer_block_id != &chain_block_id {
        push_diagnostic(
            diagnostics, "sequencer", "confirmed reference is behind current inclusion",
        );
        return Ok(None);
    }

    let Some(reservations) = &last.reservations else {
        push_diagnostic(diagnostics, "reservations", "no successful observation");
        return Ok(None);
    };
    validate_terminal_attribution("reservations", reservations, last)?;
    if reservations.facts.withdrawal_id != target.withdrawal_id {
        return Err(TerminalOracleError::Mismatch {
            source_name: "reservations",
            reason: "withdrawal id differs from target".to_owned(),
            last: Box::new(last.clone()),
        });
    }
    if normalized_note_names(&reservations.facts.tracked_inputs)
        != normalized_note_names(&target.reserved_inputs)
    {
        return Err(TerminalOracleError::Mismatch {
            source_name: "reservations",
            reason: "tracked reserved inputs differ from target".to_owned(),
            last: Box::new(last.clone()),
        });
    }
    if reservations.facts.release_count > 1 {
        return Err(TerminalOracleError::Mismatch {
            source_name: "reservations",
            reason: "reserved inputs were released more than once".to_owned(),
            last: Box::new(last.clone()),
        });
    }
    let release_event_ids = reservations
        .facts
        .release_event_ids
        .iter()
        .filter(|event_id| !event_id.trim().is_empty())
        .collect::<HashSet<_>>();
    if (reservations.facts.release_count == 0 && !reservations.facts.release_event_ids.is_empty())
        || (reservations.facts.release_count == 1
            && (release_event_ids.len() != 1 || reservations.facts.release_event_ids.len() != 1))
    {
        return Err(TerminalOracleError::Mismatch {
            source_name: "reservations",
            reason: "reservation release count disagrees with durable release events".to_owned(),
            last: Box::new(last.clone()),
        });
    }
    if reservations.facts.release_count != 1
        || !reservations.facts.currently_reserved_inputs.is_empty()
    {
        push_diagnostic(diagnostics, "reservations", "target inputs remain reserved");
        return Ok(None);
    }

    let Some(public) = &last.public else {
        push_diagnostic(diagnostics, "public", "no successful observation");
        return Ok(None);
    };
    validate_terminal_attribution("public", public, last)?;
    validate_public_identity(target, &public.facts, last)?;
    match public.facts.state {
        PublicWithdrawalState::ReorgHold => {
            return Err(TerminalOracleError::ReorgHold {
                source_name: "public",
                last: Box::new(last.clone()),
            })
        }
        PublicWithdrawalState::Failed => {
            return Err(TerminalOracleError::Mismatch {
                source_name: "public",
                reason: "public API reports terminal failure".to_owned(),
                last: Box::new(last.clone()),
            })
        }
        PublicWithdrawalState::Confirmed => {}
        _ => {
            push_diagnostic(diagnostics, "public", "public state is not confirmed");
            return Ok(None);
        }
    }
    let (Some(public_transaction_id), Some(public_height), Some(public_block_id)) = (
        public.facts.transaction_id.as_deref(),
        public.facts.confirmed_height,
        public.facts.confirmed_block_id.as_ref(),
    ) else {
        push_diagnostic(diagnostics, "public", "confirmed reference is missing");
        return Ok(None);
    };
    if public_transaction_id != target.transaction_id {
        return Err(TerminalOracleError::Mismatch {
            source_name: "public",
            reason: "confirmed transaction differs from target".to_owned(),
            last: Box::new(last.clone()),
        });
    }
    if public_height != chain_height || public_block_id != &chain_block_id {
        push_diagnostic(
            diagnostics, "public", "confirmed reference is behind current inclusion",
        );
        return Ok(None);
    }

    Ok(Some(TerminalStableKey {
        chain_height,
        chain_block_id,
        sequencer_height,
        sequencer_block_id: sequencer_block_id.clone(),
        reservation_release_count: reservations.facts.release_count,
        public_height,
        public_block_id: public_block_id.clone(),
    }))
}

fn validate_sequencer_identity(
    target: &TerminalWithdrawalTarget,
    sequencer: &SequencerTerminalFacts,
    last: &TerminalLastFacts,
) -> Result<(), TerminalOracleError> {
    if sequencer.withdrawal_id != target.withdrawal_id
        || sequencer.withdrawal_nonce != target.withdrawal_nonce
        || sequencer
            .transaction_id
            .as_deref()
            .is_some_and(|id| id != target.transaction_id)
    {
        return Err(TerminalOracleError::Mismatch {
            source_name: "sequencer",
            reason: "withdrawal id, nonce, or transaction differs from target".to_owned(),
            last: Box::new(last.clone()),
        });
    }
    Ok(())
}

fn validate_public_identity(
    target: &TerminalWithdrawalTarget,
    public: &PublicWithdrawalTerminalFacts,
    last: &TerminalLastFacts,
) -> Result<(), TerminalOracleError> {
    if public.withdrawal_id != target.withdrawal_id
        || public.withdrawal_nonce != target.withdrawal_nonce
        || public.base_event_id != target.base_event_id
        || public
            .transaction_id
            .as_deref()
            .is_some_and(|id| id != target.transaction_id)
    {
        return Err(TerminalOracleError::Mismatch {
            source_name: "public",
            reason: "withdrawal id, nonce, Base event, or transaction differs from target"
                .to_owned(),
            last: Box::new(last.clone()),
        });
    }
    Ok(())
}

fn normalized_note_names(names: &[NoteNameFacts]) -> Vec<(String, String)> {
    let mut names = names
        .iter()
        .map(|name| (name.first.clone(), name.last.clone()))
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn validate_terminal_request(
    target: &TerminalWithdrawalTarget,
    settlement: &SettlementConservationProof,
    timeout: Duration,
) -> Result<(), TerminalOracleError> {
    if timeout.is_zero()
        || target.withdrawal_id.trim().is_empty()
        || target.base_event_id.trim().is_empty()
        || target.transaction_id.trim().is_empty()
        || target.reserved_inputs.is_empty()
    {
        return Err(TerminalOracleError::InvalidRequest(
            "target identity, reserved inputs, and timeout are required",
        ));
    }
    let unique_reserved_inputs = target
        .reserved_inputs
        .iter()
        .map(|name| (&name.first, &name.last))
        .collect::<HashSet<_>>();
    if unique_reserved_inputs.len() != target.reserved_inputs.len()
        || settlement.base_event_id != target.base_event_id
        || settlement.nock_transaction_id != target.transaction_id
        || !equation_proof_is_valid(&settlement.transaction_conservation)
        || !equation_proof_is_valid(&settlement.burn_to_payout)
    {
        return Err(TerminalOracleError::InvalidSettlement);
    }
    Ok(())
}

fn equation_proof_is_valid(equation: &ArithmeticEquationProof) -> bool {
    let Ok(left) = equation.left.nicks.0.parse::<u64>() else {
        return false;
    };
    let Ok(declared_right) = equation.right_total.0.parse::<u64>() else {
        return false;
    };
    let right = equation.right_terms.iter().try_fold(0_u64, |total, term| {
        term.nicks
            .0
            .parse::<u64>()
            .ok()
            .and_then(|value| total.checked_add(value))
    });
    equation.verdict && right == Some(left) && declared_right == left
}

fn terminal_proof(
    target: &TerminalWithdrawalTarget,
    settlement: &SettlementConservationProof,
    last: TerminalLastFacts,
    stable_observations: u64,
    diagnostics: Vec<String>,
) -> Result<TerminalWithdrawalProof, TerminalOracleError> {
    Ok(TerminalWithdrawalProof {
        schema_version: 2,
        target: target.clone(),
        settlement: settlement.clone(),
        chain: last.chain.ok_or(TerminalOracleError::IncompleteFacts)?,
        kernels: last.kernels.ok_or(TerminalOracleError::IncompleteFacts)?,
        sequencer: last.sequencer.ok_or(TerminalOracleError::IncompleteFacts)?,
        reservations: last
            .reservations
            .ok_or(TerminalOracleError::IncompleteFacts)?,
        public: last.public.ok_or(TerminalOracleError::IncompleteFacts)?,
        stable_observations,
        diagnostics,
    })
}

fn push_diagnostic(diagnostics: &mut Vec<String>, source: &str, message: &str) {
    const MAX_DIAGNOSTICS: usize = 64;
    let diagnostic = format!("{source}: {message}");
    if diagnostics.last() != Some(&diagnostic) {
        if diagnostics.len() == MAX_DIAGNOSTICS {
            diagnostics.remove(0);
        }
        diagnostics.push(diagnostic);
    }
}

#[derive(Debug, Error)]
pub enum TerminalOracleError {
    #[error("invalid terminal oracle request: {0}")]
    InvalidRequest(&'static str),
    #[error("settlement conservation proof did not pass")]
    InvalidSettlement,
    #[error("terminal oracle observed a mismatch from {source_name}: {reason}")]
    Mismatch {
        source_name: &'static str,
        reason: String,
        last: Box<TerminalLastFacts>,
    },
    #[error("bridge-{node_id} stopped or entered hold: {reason}")]
    KernelStoppedOrHeld {
        node_id: u64,
        reason: String,
        last: Box<TerminalLastFacts>,
    },
    #[error("{source_name} entered Nockchain reorg hold")]
    ReorgHold {
        source_name: &'static str,
        last: Box<TerminalLastFacts>,
    },
    #[error("terminal oracle timed out; diagnostics={diagnostics:?}")]
    Timeout {
        last: Box<TerminalLastFacts>,
        diagnostics: Vec<String>,
    },
    #[error("terminal oracle facts became incomplete")]
    IncompleteFacts,
}
