use std::collections::BTreeSet;

use nockchain_types::tx_engine::common::{BlockHeight, Hash, SchnorrPubkey, Version};
use nockchain_types::tx_engine::v0::TimelockIntent as V0TimelockIntent;
use nockchain_types::tx_engine::v1::tx::{LockPrimitive, LockTim, SpendCondition};
use thiserror::Error;

use crate::determinism::sort_candidates;
use crate::fee::{compute_bridge_fee, compute_minimum_fee, FeeInputs};
use crate::lock_resolver::{LockMatcher, LockResolutionSource, ResolveLockRequest};
use crate::types::{
    CandidateNote, CandidateV0Note, CandidateVersionPolicy, ChainContext, CreateTxPlanningMode,
    PlanRequest, PlanResult, PlannedOutput, RawNoteDataEntry, SelectionMode, SelectionOrder,
    WithdrawalPlanRequest, WithdrawalPlanResult, WordCountBreakdown,
};
use crate::word_count::{WitnessWordInput, WordCountEstimator};

/// Planner failures for candidate admission, lock resolution, and fee conservation.
#[derive(Debug, Error)]
pub enum PlanError {
    #[error("plan request must include at least one recipient output")]
    NoRecipients,
    #[error("manual mode requires at least one note name")]
    ManualNamesMissing,
    #[error("manual mode references unknown note {first}/{last}")]
    ManualNoteMissing { first: String, last: String },
    #[error("manual mode contains duplicate note name {first}/{last}")]
    DuplicateManualName { first: String, last: String },
    #[error(
        "unable to resolve effective lock for note {first}/{last}; source={resolution_source:?}"
    )]
    UnknownLock {
        first: String,
        last: String,
        resolution_source: LockResolutionSource,
    },
    #[error(
        "matcher selected note {first}/{last} but did not provide spend-condition metadata needed for planning; source={resolution_source:?}"
    )]
    MissingPlanningSpendCondition {
        first: String,
        last: String,
        resolution_source: LockResolutionSource,
    },
    #[error("insufficient funds: selected_total={selected_total} required={required}")]
    InsufficientFunds { selected_total: u64, required: u64 },
    #[error("conservation failed for selected transaction")]
    ConservationFailed,
    #[error(
        "v0 migration sweep leaves no spendable output after fees: selected_total={selected_total} fee={fee}"
    )]
    V0MigrationProducesZeroValue { selected_total: u64, fee: u64 },
    #[error(
        "candidate note {first}/{last} has version {version:?}, but selector policy is {policy:?}"
    )]
    CandidateVersionDisabled {
        version: Version,
        policy: CandidateVersionPolicy,
        first: String,
        last: String,
    },
    #[error(
        "withdrawal burned amount is too small to cover withdrawal + tx fees: burned_amount={burned_amount} withdrawal_fee={withdrawal_fee} tx_fee={tx_fee}"
    )]
    WithdrawalBurnedAmountTooSmall {
        burned_amount: u64,
        withdrawal_fee: u64,
        tx_fee: u64,
    },
    #[error(
        "withdrawal fee estimate changed after applying solved recipient amount: initial_fee={initial_fee} recomputed_fee={recomputed_fee}"
    )]
    WithdrawalFeeShapeChanged {
        initial_fee: u64,
        recomputed_fee: u64,
    },
}

#[derive(Debug, Clone)]
/// One selected candidate note tracked in planner state.
struct SelectedInput {
    /// Candidate note accepted into the current plan.
    candidate: CandidateNote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Result of attempting to admit one candidate into the current plan.
enum CandidateSelection {
    Skipped,
    Selected,
}

impl CandidateSelection {
    fn was_selected(self) -> bool {
        matches!(self, Self::Selected)
    }
}

#[derive(Debug, Clone)]
/// Recomputed fee/output state shared by standard and withdrawal planning.
struct PlanComputation {
    /// Fee chosen for the current selected-input set.
    final_fee: u64,
    /// Minimum fee implied by current seed/witness words.
    minimum_fee: u64,
    /// Seed words recomputed from current output set.
    seed_words: u64,
    /// Witness words recomputed from selected input locks.
    witness_words: u64,
    /// Output set corresponding to `final_fee` (refund included when present).
    outputs: Vec<PlannedOutput>,
}

#[derive(Debug, Clone)]
/// One v1 candidate whose admission checks have already succeeded.
struct AdmittedV1Candidate {
    candidate: CandidateNote,
    candidate_assets: u64,
    witness_words_for_input: u64,
    first: String,
    last: String,
}

struct V1CandidateAdmissionContext<'ctx, 'chain, M> {
    word_count_estimator: &'ctx WordCountEstimator<'chain>,
    debug_trace: &'ctx mut Vec<String>,
    matcher: &'ctx M,
    signer_pkh: Option<&'ctx Hash>,
    coinbase_relative_min: Option<u64>,
    current_height: &'ctx BlockHeight,
    unknown_lock_is_error: bool,
}

#[derive(Debug, Clone)]
/// One v0 candidate whose spendability/timelock checks have already succeeded.
struct AdmittedV0Candidate {
    note: CandidateV0Note,
    candidate_assets: u64,
    witness_words_for_input: u64,
    first: String,
    last: String,
}

#[derive(Debug)]
/// Shared planner bookkeeping reused across create-tx and withdrawal scenarios.
struct PlanningState<'a> {
    /// Word-count estimator bound to request chain context.
    word_count_estimator: WordCountEstimator<'a>,
    /// Running total of witness words for all currently selected inputs.
    witness_words_total: u64,
    /// Selected inputs in deterministic order.
    selected: Vec<SelectedInput>,
    /// Running sum of selected input assets.
    selected_total: u64,
    /// Human-readable decision trace emitted in plan result.
    debug_trace: Vec<String>,
}

impl<'a> PlanningState<'a> {
    fn new(chain_context: &'a ChainContext) -> Self {
        Self {
            word_count_estimator: WordCountEstimator::new(chain_context),
            witness_words_total: 0,
            selected: Vec::new(),
            selected_total: 0,
            debug_trace: Vec::new(),
        }
    }

    fn record_selection(
        &mut self,
        candidate: CandidateNote,
        candidate_assets: u64,
        witness_words_for_input: u64,
    ) {
        self.selected_total = self.selected_total.saturating_add(candidate_assets);
        self.witness_words_total = self
            .witness_words_total
            .saturating_add(witness_words_for_input);
        self.selected.push(SelectedInput { candidate });
    }

    fn covers_total(&self, required: u64) -> bool {
        self.selected_total >= required
    }

    fn into_plan_result(self, recompute: PlanComputation) -> PlanResult {
        PlanResult {
            selected: self
                .selected
                .into_iter()
                .map(|input| input.candidate.identity().clone())
                .collect(),
            selected_total: self.selected_total,
            outputs: recompute.outputs,
            final_fee: recompute.final_fee,
            word_counts: WordCountBreakdown {
                seed_words: recompute.seed_words,
                witness_words: recompute.witness_words,
            },
            debug_trace: self.debug_trace,
        }
    }
}

#[derive(Debug, Clone)]
/// Recomputed state for a finalizable v0 migration sweep.
struct V0MigrationReadyState {
    /// Fee chosen for the current selected-input set.
    final_fee: u64,
    /// Minimum fee implied by current seed/witness words.
    minimum_fee: u64,
    /// Seed words recomputed from the destination output.
    seed_words: u64,
    /// Witness words recomputed from selected input locks.
    witness_words: u64,
    /// Single migration destination output.
    destination_output: PlannedOutput,
}

#[derive(Debug, Clone)]
/// Explicit migration recompute state used while accumulating legacy inputs.
enum V0MigrationRecomputeState {
    /// Current selection still cannot fund a positive migration output after fees.
    NeedsMoreInputs {
        final_fee: u64,
        minimum_fee: u64,
        seed_words: u64,
        witness_words: u64,
    },
    /// Current selection can fund a positive migration destination output.
    Ready(V0MigrationReadyState),
}

impl V0MigrationRecomputeState {
    fn final_fee(&self) -> u64 {
        match self {
            Self::NeedsMoreInputs { final_fee, .. } => *final_fee,
            Self::Ready(state) => state.final_fee,
        }
    }

    fn minimum_fee(&self) -> u64 {
        match self {
            Self::NeedsMoreInputs { minimum_fee, .. } => *minimum_fee,
            Self::Ready(state) => state.minimum_fee,
        }
    }

    fn seed_words(&self) -> u64 {
        match self {
            Self::NeedsMoreInputs { seed_words, .. } => *seed_words,
            Self::Ready(state) => state.seed_words,
        }
    }

    fn witness_words(&self) -> u64 {
        match self {
            Self::NeedsMoreInputs { witness_words, .. } => *witness_words,
            Self::Ready(state) => state.witness_words,
        }
    }
}

impl From<V0MigrationReadyState> for PlanComputation {
    fn from(ready: V0MigrationReadyState) -> Self {
        Self {
            final_fee: ready.final_fee,
            minimum_fee: ready.minimum_fee,
            seed_words: ready.seed_words,
            witness_words: ready.witness_words,
            outputs: vec![ready.destination_output],
        }
    }
}

fn create_tx_candidate_version_allowed(
    request: &PlanRequest,
    debug_trace: &mut Vec<String>,
    candidate: &CandidateNote,
) -> Result<bool, PlanError> {
    let candidate_version = candidate.version();
    let allowed_version = match request.candidate_version_policy {
        CandidateVersionPolicy::V1Only => Version::V1,
        CandidateVersionPolicy::V0Only => Version::V0,
    };
    if candidate_version != allowed_version {
        let (first, last) = candidate.note_name_display();
        if matches!(&request.selection_mode, SelectionMode::Manual { .. }) {
            return Err(PlanError::CandidateVersionDisabled {
                version: candidate_version,
                policy: request.candidate_version_policy,
                first,
                last,
            });
        }
        debug_trace.push(format!(
            "skipped note {first}/{last}: version {candidate_version:?} disabled by selector policy {policy:?}",
            policy = request.candidate_version_policy
        ));
        return Ok(false);
    }

    Ok(true)
}

fn compute_chain_minimum_fee(
    chain_context: &ChainContext,
    seed_words: u64,
    witness_words: u64,
) -> u64 {
    compute_minimum_fee(FeeInputs {
        seed_words,
        witness_words,
        base_fee: chain_context.base_fee,
        input_fee_divisor: chain_context.input_fee_divisor,
        min_fee: chain_context.min_fee,
        height: chain_context.height.clone(),
        bythos_phase: chain_context.bythos_phase.clone(),
    })
    .minimum_fee
}

fn admit_selectable_v1_candidate<M: LockMatcher>(
    candidate: CandidateNote,
    context: V1CandidateAdmissionContext<'_, '_, M>,
) -> Result<Option<AdmittedV1Candidate>, PlanError> {
    let CandidateNote::V1(_) = &candidate else {
        return Ok(None);
    };

    let resolution = context.matcher.select_v1_candidate(ResolveLockRequest {
        note_first_name: &candidate.identity().name.first,
        decoded_note_data: candidate.decoded_note_data(),
        signer_pkh: context.signer_pkh,
        coinbase_relative_min: context.coinbase_relative_min,
    });
    let (first, last) = candidate.note_name_display();
    if !resolution.is_selected() {
        if context.unknown_lock_is_error {
            return Err(PlanError::UnknownLock {
                first,
                last,
                resolution_source: resolution.source,
            });
        }
        context.debug_trace.push(format!(
            "skipped note {first}/{last}: unresolved lock source={:?}",
            resolution.source
        ));
        return Ok(None);
    }
    let Some(spend_condition) = resolution.spend_condition else {
        return Err(PlanError::MissingPlanningSpendCondition {
            first,
            last,
            resolution_source: resolution.source,
        });
    };
    if !timelock_satisfied(
        &spend_condition,
        &candidate.identity().origin_page,
        context.current_height,
    ) {
        context.debug_trace.push(format!(
            "skipped note {first}/{last}: timelock not satisfied at height={}",
            height_value(context.current_height)
        ));
        return Ok(None);
    }

    let candidate_assets = candidate.assets().0 as u64;
    let witness_words_for_input = context
        .word_count_estimator
        .estimate_witness_words_for_input(&WitnessWordInput {
            spend_condition: spend_condition.clone(),
            input_origin_page: candidate.identity().origin_page.clone(),
            spend_condition_count: resolution.spend_condition_count,
        });
    Ok(Some(AdmittedV1Candidate {
        candidate,
        candidate_assets,
        witness_words_for_input,
        first,
        last,
    }))
}

fn admit_selectable_v0_candidate(
    word_count_estimator: &WordCountEstimator<'_>,
    debug_trace: &mut Vec<String>,
    note: CandidateV0Note,
    signer_pubkeys: &[SchnorrPubkey],
    current_height: &BlockHeight,
    signer_scope_label: &str,
) -> Option<AdmittedV0Candidate> {
    let (first, last) = (
        note.identity.name.first.to_base58(),
        note.identity.name.last.to_base58(),
    );
    if !v0_note_spendable_by_signers(&note, signer_pubkeys) {
        debug_trace.push(format!(
            "skipped note {first}/{last}: legacy lock is not spendable by {signer_scope_label} signer pubkeys",
        ));
        return None;
    }
    if !v0_timelock_satisfied(
        note.timelock.as_ref(),
        &note.identity.origin_page,
        current_height,
    ) {
        debug_trace.push(format!(
            "skipped note {first}/{last}: v0 timelock not satisfied at height={}",
            height_value(current_height)
        ));
        return None;
    }

    Some(AdmittedV0Candidate {
        candidate_assets: note.assets.0 as u64,
        witness_words_for_input: word_count_estimator
            .estimate_v0_witness_words(note.lock.keys_required),
        note,
        first,
        last,
    })
}

trait PlanningScenario {
    type Output;

    fn ordered_candidates(&self) -> Result<Vec<CandidateNote>, PlanError>;
    fn try_select_candidate(
        &mut self,
        candidate: CandidateNote,
    ) -> Result<CandidateSelection, PlanError>;
    fn should_stop(&self) -> bool;
    fn finalize(self) -> Result<Self::Output, PlanError>;

    fn execute(mut self) -> Result<Self::Output, PlanError>
    where
        Self: Sized,
    {
        for candidate in self.ordered_candidates()? {
            if !self.try_select_candidate(candidate)?.was_selected() {
                continue;
            }
            if self.should_stop() {
                break;
            }
        }

        self.finalize()
    }
}

struct StandardCreateTxScenario<'a, M> {
    request: &'a PlanRequest,
    matcher: &'a M,
    gift_total: u64,
    seed_words_without_refund: u64,
    state: PlanningState<'a>,
}

impl<'a, M: LockMatcher> StandardCreateTxScenario<'a, M> {
    fn new(request: &'a PlanRequest, matcher: &'a M) -> Self {
        let state = PlanningState::new(&request.chain_context);
        let gift_total = request
            .recipient_outputs
            .iter()
            .fold(0u64, |acc, output| acc.saturating_add(output.amount));
        let seed_words_without_refund = state
            .word_count_estimator
            .estimate_seed_words(&request.recipient_outputs);
        Self {
            request,
            matcher,
            gift_total,
            seed_words_without_refund,
            state,
        }
    }

    fn required_total(&self, fee: u64) -> u64 {
        self.gift_total.saturating_add(fee)
    }

    fn record_selected_candidate_trace(&mut self, first: &str, last: &str, candidate_assets: u64) {
        let recompute = self.recompute_fee();
        let required = self.required_total(recompute.final_fee);
        self.state.debug_trace.push(format!(
            "selected note {first}/{last} assets={} selected_total={} seed_words={} witness_words={} min_fee={} final_fee={} required={}",
            candidate_assets,
            self.state.selected_total,
            recompute.seed_words,
            recompute.witness_words,
            recompute.minimum_fee,
            recompute.final_fee,
            required,
        ));
    }

    fn try_select_v0_candidate(
        &mut self,
        candidate: CandidateNote,
    ) -> Result<CandidateSelection, PlanError> {
        if !create_tx_candidate_version_allowed(
            self.request, &mut self.state.debug_trace, &candidate,
        )? {
            return Ok(CandidateSelection::Skipped);
        }

        let CandidateNote::V0(note) = candidate else {
            unreachable!("v0-only create-tx planning should only admit legacy candidates");
        };
        let Some(admitted) = admit_selectable_v0_candidate(
            &self.state.word_count_estimator, &mut self.state.debug_trace, note,
            &self.request.v0_migration_signer_pubkeys, &self.request.chain_context.height,
            "planner",
        ) else {
            return Ok(CandidateSelection::Skipped);
        };

        self.state.record_selection(
            CandidateNote::V0(admitted.note),
            admitted.candidate_assets,
            admitted.witness_words_for_input,
        );
        self.record_selected_candidate_trace(
            &admitted.first, &admitted.last, admitted.candidate_assets,
        );

        Ok(CandidateSelection::Selected)
    }

    fn try_select_v1_candidate(
        &mut self,
        candidate: CandidateNote,
    ) -> Result<CandidateSelection, PlanError> {
        if !create_tx_candidate_version_allowed(
            self.request, &mut self.state.debug_trace, &candidate,
        )? {
            return Ok(CandidateSelection::Skipped);
        }

        let Some(admitted) = admit_selectable_v1_candidate(
            candidate,
            V1CandidateAdmissionContext {
                word_count_estimator: &self.state.word_count_estimator,
                debug_trace: &mut self.state.debug_trace,
                matcher: self.matcher,
                signer_pkh: self.request.signer_pkh.as_ref(),
                coinbase_relative_min: self.request.coinbase_relative_min,
                current_height: &self.request.chain_context.height,
                unknown_lock_is_error: matches!(
                    &self.request.selection_mode,
                    SelectionMode::Manual { .. }
                ),
            },
        )?
        else {
            return Ok(CandidateSelection::Skipped);
        };

        self.state.record_selection(
            admitted.candidate, admitted.candidate_assets, admitted.witness_words_for_input,
        );
        self.record_selected_candidate_trace(
            &admitted.first, &admitted.last, admitted.candidate_assets,
        );

        Ok(CandidateSelection::Selected)
    }

    fn recompute_fee(&self) -> PlanComputation {
        let witness_words = self.state.witness_words_total;
        let fee_capacity = self.state.selected_total.saturating_sub(self.gift_total);
        let minimum_without_refund = compute_chain_minimum_fee(
            &self.request.chain_context, self.seed_words_without_refund, witness_words,
        );
        let mut final_fee = minimum_without_refund;
        if fee_capacity > minimum_without_refund {
            let refund_if_min_without = fee_capacity.saturating_sub(minimum_without_refund);
            let outputs_with_refund = outputs_with_refund(self.request, refund_if_min_without);
            let seed_words_with_refund = self
                .state
                .word_count_estimator
                .estimate_seed_words(&outputs_with_refund);
            let minimum_with_refund = compute_chain_minimum_fee(
                &self.request.chain_context, seed_words_with_refund, witness_words,
            );
            if fee_capacity > minimum_with_refund {
                final_fee = minimum_with_refund;
            } else {
                final_fee = fee_capacity;
            }
        }

        let refund = self.refund_amount(final_fee);
        let outputs = outputs_with_refund(self.request, refund);
        let seed_words = self
            .state
            .word_count_estimator
            .estimate_seed_words(&outputs);
        let minimum_fee =
            compute_chain_minimum_fee(&self.request.chain_context, seed_words, witness_words);
        PlanComputation {
            final_fee,
            minimum_fee,
            seed_words,
            witness_words,
            outputs,
        }
    }

    fn refund_amount(&self, fee: u64) -> u64 {
        self.state
            .selected_total
            .saturating_sub(self.required_total(fee))
    }
}

impl<M: LockMatcher> PlanningScenario for StandardCreateTxScenario<'_, M> {
    type Output = PlanResult;

    fn ordered_candidates(&self) -> Result<Vec<CandidateNote>, PlanError> {
        ordered_candidates(
            &self.request.selection_mode, &self.request.candidates, self.request.order_direction,
        )
    }

    fn try_select_candidate(
        &mut self,
        candidate: CandidateNote,
    ) -> Result<CandidateSelection, PlanError> {
        match self.request.candidate_version_policy {
            CandidateVersionPolicy::V1Only => self.try_select_v1_candidate(candidate),
            CandidateVersionPolicy::V0Only => self.try_select_v0_candidate(candidate),
        }
    }

    fn should_stop(&self) -> bool {
        matches!(&self.request.selection_mode, SelectionMode::Auto)
            && self
                .state
                .covers_total(self.required_total(self.recompute_fee().final_fee))
    }

    fn finalize(self) -> Result<Self::Output, PlanError> {
        let recompute = self.recompute_fee();
        let required = self.required_total(recompute.final_fee);
        if !self.state.covers_total(required) {
            return Err(PlanError::InsufficientFunds {
                selected_total: self.state.selected_total,
                required,
            });
        }

        let allocation = allocate_inputs(
            self.state.selected_total, self.gift_total, recompute.final_fee,
        )
        .expect("required <= selected_total should always allocate");
        let conservation = ConservationCheck {
            input_total: self.state.selected_total,
            gift_total: allocation.gift_total,
            refund_total: allocation.refund,
            fee: allocation.fee,
        };
        if !conservation.is_balanced() {
            return Err(PlanError::ConservationFailed);
        }

        Ok(self.state.into_plan_result(recompute))
    }
}

struct V0MigrationScenario<'a> {
    request: &'a PlanRequest,
    state: PlanningState<'a>,
}

impl<'a> V0MigrationScenario<'a> {
    fn new(request: &'a PlanRequest) -> Self {
        Self {
            request,
            state: PlanningState::new(&request.chain_context),
        }
    }

    fn recompute_fee(&self) -> V0MigrationRecomputeState {
        let CreateTxPlanningMode::V0MigrationSweep { destination_output } =
            &self.request.planning_mode
        else {
            unreachable!("v0 migration recompute should only run in V0MigrationSweep mode");
        };
        let witness_words = self.state.witness_words_total;
        let seed_words = self
            .state
            .word_count_estimator
            .estimate_seed_words(std::slice::from_ref(destination_output));
        let minimum_fee =
            compute_chain_minimum_fee(&self.request.chain_context, seed_words, witness_words);
        let final_fee = minimum_fee;
        let Some(sweep_amount) = self.state.selected_total.checked_sub(final_fee) else {
            return V0MigrationRecomputeState::NeedsMoreInputs {
                final_fee,
                minimum_fee,
                seed_words,
                witness_words,
            };
        };
        if sweep_amount == 0 {
            return V0MigrationRecomputeState::NeedsMoreInputs {
                final_fee,
                minimum_fee,
                seed_words,
                witness_words,
            };
        }

        V0MigrationRecomputeState::Ready(V0MigrationReadyState {
            final_fee,
            minimum_fee,
            seed_words,
            witness_words,
            destination_output: PlannedOutput {
                lock_root: destination_output.lock_root.clone(),
                amount: sweep_amount,
                note_data: destination_output.note_data.clone(),
            },
        })
    }
}

impl PlanningScenario for V0MigrationScenario<'_> {
    type Output = PlanResult;

    fn ordered_candidates(&self) -> Result<Vec<CandidateNote>, PlanError> {
        ordered_candidates(
            &self.request.selection_mode, &self.request.candidates, self.request.order_direction,
        )
    }

    fn try_select_candidate(
        &mut self,
        candidate: CandidateNote,
    ) -> Result<CandidateSelection, PlanError> {
        if !create_tx_candidate_version_allowed(
            self.request, &mut self.state.debug_trace, &candidate,
        )? {
            return Ok(CandidateSelection::Skipped);
        }

        let CandidateNote::V0(note) = candidate else {
            return Ok(CandidateSelection::Skipped);
        };
        let Some(admitted) = admit_selectable_v0_candidate(
            &self.state.word_count_estimator, &mut self.state.debug_trace, note,
            &self.request.v0_migration_signer_pubkeys, &self.request.chain_context.height,
            "migration",
        ) else {
            return Ok(CandidateSelection::Skipped);
        };

        self.state.record_selection(
            CandidateNote::V0(admitted.note),
            admitted.candidate_assets,
            admitted.witness_words_for_input,
        );

        let recompute = self.recompute_fee();
        self.state.debug_trace.push(format!(
            "selected migration note {first}/{last} assets={} selected_total={} seed_words={} witness_words={} min_fee={} final_fee={}",
            admitted.candidate_assets,
            self.state.selected_total,
            recompute.seed_words(),
            recompute.witness_words(),
            recompute.minimum_fee(),
            recompute.final_fee(),
            first = admitted.first,
            last = admitted.last,
        ));

        Ok(CandidateSelection::Selected)
    }

    fn should_stop(&self) -> bool {
        false
    }

    fn finalize(self) -> Result<Self::Output, PlanError> {
        let recompute = self.recompute_fee();
        let ready = match recompute {
            V0MigrationRecomputeState::NeedsMoreInputs { final_fee, .. } => {
                return Err(PlanError::V0MigrationProducesZeroValue {
                    selected_total: self.state.selected_total,
                    fee: final_fee,
                });
            }
            V0MigrationRecomputeState::Ready(ready) => ready,
        };

        if self.state.selected_total
            != ready
                .destination_output
                .amount
                .saturating_add(ready.final_fee)
        {
            return Err(PlanError::ConservationFailed);
        }

        Ok(self.state.into_plan_result(ready.into()))
    }
}

struct WithdrawalScenario<'a, M> {
    request: &'a WithdrawalPlanRequest,
    matcher: &'a M,
    withdrawal_fee: u64,
    spendable_amount: u64,
    state: PlanningState<'a>,
}

impl<'a, M> WithdrawalScenario<'a, M> {
    fn new(
        request: &'a WithdrawalPlanRequest,
        matcher: &'a M,
        withdrawal_fee: u64,
        spendable_amount: u64,
    ) -> Self {
        let mut state = PlanningState::new(&request.chain_context);
        state.debug_trace.push(format!(
            "withdrawal setup burned_amount={} withdrawal_fee={} spendable_amount={} nicks_fee_per_nock={}",
            request.burned_amount,
            withdrawal_fee,
            spendable_amount,
            request.nicks_fee_per_nock
        ));
        Self {
            request,
            matcher,
            withdrawal_fee,
            spendable_amount,
            state,
        }
    }

    fn required_total(&self) -> u64 {
        self.spendable_amount
    }

    fn recompute_fee(&self) -> Result<PlanComputation, PlanError> {
        let witness_words = self.state.witness_words_total;
        let refund = self
            .state
            .selected_total
            .saturating_sub(self.spendable_amount);
        let seed_probe_outputs = withdrawal_outputs(self.request, self.spendable_amount, refund);
        let seed_probe_words = self
            .state
            .word_count_estimator
            .estimate_seed_words(&seed_probe_outputs);
        let final_fee =
            compute_chain_minimum_fee(&self.request.chain_context, seed_probe_words, witness_words);

        let recipient_amount = self.spendable_amount.checked_sub(final_fee).ok_or(
            PlanError::WithdrawalBurnedAmountTooSmall {
                burned_amount: self.request.burned_amount,
                withdrawal_fee: self.withdrawal_fee,
                tx_fee: final_fee,
            },
        )?;
        if recipient_amount == 0 {
            return Err(PlanError::WithdrawalBurnedAmountTooSmall {
                burned_amount: self.request.burned_amount,
                withdrawal_fee: self.withdrawal_fee,
                tx_fee: final_fee,
            });
        }

        let outputs = withdrawal_outputs(self.request, recipient_amount, refund);
        let seed_words = self
            .state
            .word_count_estimator
            .estimate_seed_words(&outputs);
        let minimum_fee =
            compute_chain_minimum_fee(&self.request.chain_context, seed_words, witness_words);
        if minimum_fee != final_fee {
            return Err(PlanError::WithdrawalFeeShapeChanged {
                initial_fee: final_fee,
                recomputed_fee: minimum_fee,
            });
        }

        Ok(PlanComputation {
            final_fee,
            minimum_fee,
            seed_words,
            witness_words,
            outputs,
        })
    }
}

impl<M: LockMatcher> PlanningScenario for WithdrawalScenario<'_, M> {
    type Output = WithdrawalPlanResult;

    fn ordered_candidates(&self) -> Result<Vec<CandidateNote>, PlanError> {
        ordered_candidates(
            &SelectionMode::Auto,
            &self.request.candidates,
            SelectionOrder::Ascending,
        )
    }

    fn try_select_candidate(
        &mut self,
        candidate: CandidateNote,
    ) -> Result<CandidateSelection, PlanError> {
        let candidate_version = candidate.version();
        if candidate_version != Version::V1 {
            let (first, last) = candidate.note_name_display();
            self.state.debug_trace.push(format!(
                "skipped note {first}/{last}: version {candidate_version:?} disabled by selector policy {policy:?}",
                policy = CandidateVersionPolicy::V1Only
            ));
            return Ok(CandidateSelection::Skipped);
        }

        let Some(admitted) = admit_selectable_v1_candidate(
            candidate,
            V1CandidateAdmissionContext {
                word_count_estimator: &self.state.word_count_estimator,
                debug_trace: &mut self.state.debug_trace,
                matcher: self.matcher,
                signer_pkh: None,
                coinbase_relative_min: None,
                current_height: &self.request.chain_context.height,
                unknown_lock_is_error: false,
            },
        )?
        else {
            return Ok(CandidateSelection::Skipped);
        };

        self.state.record_selection(
            admitted.candidate, admitted.candidate_assets, admitted.witness_words_for_input,
        );

        self.state.debug_trace.push(format!(
            "selected note {first}/{last} assets={} selected_total={} witness_words={} required={}",
            admitted.candidate_assets,
            self.state.selected_total,
            self.state.witness_words_total,
            self.required_total(),
            first = admitted.first,
            last = admitted.last,
        ));

        Ok(CandidateSelection::Selected)
    }

    fn should_stop(&self) -> bool {
        self.state.covers_total(self.required_total())
    }

    fn finalize(self) -> Result<Self::Output, PlanError> {
        let recompute = self.recompute_fee()?;
        let required = self.required_total();
        if !self.state.covers_total(required) {
            return Err(PlanError::InsufficientFunds {
                selected_total: self.state.selected_total,
                required,
            });
        }

        let net_recipient_amount = self
            .spendable_amount
            .checked_sub(recompute.final_fee)
            .ok_or(PlanError::WithdrawalBurnedAmountTooSmall {
                burned_amount: self.request.burned_amount,
                withdrawal_fee: self.withdrawal_fee,
                tx_fee: recompute.final_fee,
            })?;
        if net_recipient_amount == 0 {
            return Err(PlanError::WithdrawalBurnedAmountTooSmall {
                burned_amount: self.request.burned_amount,
                withdrawal_fee: self.withdrawal_fee,
                tx_fee: recompute.final_fee,
            });
        }

        let refund = self
            .state
            .selected_total
            .saturating_sub(self.spendable_amount);
        let conservation = ConservationCheck {
            input_total: self.state.selected_total,
            gift_total: net_recipient_amount,
            refund_total: refund,
            fee: recompute.final_fee,
        };
        if !conservation.is_balanced() {
            return Err(PlanError::ConservationFailed);
        }

        Ok(WithdrawalPlanResult {
            burned_amount: self.request.burned_amount,
            withdrawal_fee: self.withdrawal_fee,
            net_recipient_amount,
            plan: self.state.into_plan_result(recompute),
        })
    }
}

/// Plans input selection, fee, and outputs for create-tx using deterministic
/// ordering and lock/timelock spendability checks.
pub fn plan_create_tx<M: LockMatcher>(
    request: &PlanRequest,
    matcher: &M,
) -> Result<PlanResult, PlanError> {
    if matches!(&request.planning_mode, CreateTxPlanningMode::Standard)
        && request.recipient_outputs.is_empty()
    {
        return Err(PlanError::NoRecipients);
    }

    match &request.planning_mode {
        CreateTxPlanningMode::Standard => StandardCreateTxScenario::new(request, matcher).execute(),
        CreateTxPlanningMode::V0MigrationSweep { .. } => {
            V0MigrationScenario::new(request).execute()
        }
    }
}

/// Plans input selection, fee, and outputs for withdrawals where the burned
/// amount covers the bridge withdrawal fee, recipient disbursement, and final
/// tx fee.
pub fn plan_withdrawal_tx<M: LockMatcher>(
    request: &WithdrawalPlanRequest,
    matcher: &M,
) -> Result<WithdrawalPlanResult, PlanError> {
    let withdrawal_fee = compute_bridge_fee(request.burned_amount, request.nicks_fee_per_nock);
    let Some(spendable_amount) = request
        .burned_amount
        .checked_sub(withdrawal_fee)
        .filter(|amount| *amount > 0)
    else {
        return Err(PlanError::WithdrawalBurnedAmountTooSmall {
            burned_amount: request.burned_amount,
            withdrawal_fee,
            tx_fee: 0,
        });
    };

    WithdrawalScenario::new(request, matcher, withdrawal_fee, spendable_amount).execute()
}

/// Produces candidate ordering for the selected mode:
/// deterministic sort by `SelectionOrder` for both auto and manual candidate sets.
fn ordered_candidates(
    selection_mode: &SelectionMode,
    candidates: &[CandidateNote],
    order_direction: SelectionOrder,
) -> Result<Vec<CandidateNote>, PlanError> {
    match selection_mode {
        SelectionMode::Auto => {
            let mut out = candidates.to_vec();
            sort_candidates(&mut out, order_direction);
            Ok(out)
        }
        SelectionMode::Manual { note_names } => {
            if note_names.is_empty() {
                return Err(PlanError::ManualNamesMissing);
            }
            let mut seen = BTreeSet::<([u64; 5], [u64; 5])>::new();
            let mut out = Vec::<CandidateNote>::new();
            for name in note_names {
                let key = (name.first.to_array(), name.last.to_array());
                if !seen.insert(key) {
                    return Err(PlanError::DuplicateManualName {
                        first: name.first.to_base58(),
                        last: name.last.to_base58(),
                    });
                }
                let Some(candidate) = candidates
                    .iter()
                    .find(|candidate| candidate.identity().name == *name)
                    .cloned()
                else {
                    return Err(PlanError::ManualNoteMissing {
                        first: name.first.to_base58(),
                        last: name.last.to_base58(),
                    });
                };
                out.push(candidate);
            }
            sort_candidates(&mut out, order_direction);
            Ok(out)
        }
    }
}

/// Builds the output set for fee accounting and final result emission.
/// Recipient outputs are copied as-is; refund is optional and appended only when
/// `refund > 0`. Omitting refund does not alter gift amounts.
fn outputs_with_refund(request: &PlanRequest, refund: u64) -> Vec<PlannedOutput> {
    let mut outputs = request.recipient_outputs.clone();
    if refund > 0 {
        outputs.push(PlannedOutput {
            lock_root: request.refund_output.lock_root.clone(),
            amount: refund,
            note_data: request.refund_output.note_data.clone(),
        });
    }
    outputs
}

/// Builds the withdrawal output set for fee accounting and final result emission.
/// The recipient output amount is the planner-solved net disbursement; refund is
/// independent and is appended only when positive.
fn withdrawal_outputs(
    request: &WithdrawalPlanRequest,
    recipient_amount: u64,
    refund: u64,
) -> Vec<PlannedOutput> {
    let mut outputs = vec![PlannedOutput {
        lock_root: request.recipient_lock_root.clone(),
        amount: recipient_amount,
        note_data: vec![RawNoteDataEntry::from_bridge_withdrawal(
            request.beid.clone(),
            request.base_hash.clone(),
            request.recipient_lock_root.clone(),
            request.base_batch_end,
        )],
    }];
    if refund > 0 {
        outputs.push(PlannedOutput {
            lock_root: request.refund_output.lock_root.clone(),
            amount: refund,
            note_data: request.refund_output.note_data.clone(),
        });
    }
    outputs
}

fn v0_note_spendable_by_signers(note: &CandidateV0Note, signer_pubkeys: &[SchnorrPubkey]) -> bool {
    signer_pubkeys
        .iter()
        .any(|signer| v0_note_spendable_by_signer(note, signer))
}

fn v0_note_spendable_by_signer(note: &CandidateV0Note, signer_pubkey: &SchnorrPubkey) -> bool {
    (note.lock.pubkeys.len() == 1 && note.lock.pubkeys.first() == Some(signer_pubkey))
        || (note.lock.keys_required == 1 && note.lock.pubkeys.iter().any(|pk| pk == signer_pubkey))
}

fn v0_timelock_satisfied(
    timelock: Option<&V0TimelockIntent>,
    note_origin_page: &BlockHeight,
    current_height: &BlockHeight,
) -> bool {
    let Some(timelock) = timelock else {
        return true;
    };

    let now = height_value(current_height);
    let since = height_value(note_origin_page);
    let rel_min_ok = timelock.relative.min.as_ref().is_none_or(|min| {
        since
            .checked_add((min.0).0)
            .is_some_and(|required| now >= required)
    });
    let rel_max_ok = timelock.relative.max.as_ref().is_none_or(|max| {
        since
            .checked_add((max.0).0)
            .is_some_and(|required| now <= required)
    });
    let abs_min_ok = timelock
        .absolute
        .min
        .as_ref()
        .is_none_or(|min| now >= height_value(min));
    let abs_max_ok = timelock
        .absolute
        .max
        .as_ref()
        .is_none_or(|max| now <= height_value(max));

    rel_min_ok && rel_max_ok && abs_min_ok && abs_max_ok
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AllocationResult {
    gift_total: u64,
    fee: u64,
    refund: u64,
}

/// Splits selected inputs into gifts, fee, and refund while preserving conservation.
/// `gift_total` is caller-provided and never increased here; any leftover after
/// `gift_total + fee` is assigned to `refund`.
fn allocate_inputs(total_inputs: u64, gift_total: u64, fee: u64) -> Option<AllocationResult> {
    let required = gift_total.checked_add(fee)?;
    if total_inputs < required {
        return None;
    }
    Some(AllocationResult {
        gift_total,
        fee,
        refund: total_inputs - required,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConservationCheck {
    input_total: u64,
    gift_total: u64,
    refund_total: u64,
    fee: u64,
}

impl ConservationCheck {
    fn is_balanced(&self) -> bool {
        self.input_total
            == self
                .gift_total
                .saturating_add(self.refund_total)
                .saturating_add(self.fee)
    }
}

/// Extracts raw numeric block height from the tx-engine wrapper type.
fn height_value(height: &BlockHeight) -> u64 {
    (height.0).0
}

/// Returns true when every timelock primitive in the spend condition is
/// currently satisfiable.
fn timelock_satisfied(
    spend_condition: &SpendCondition,
    note_origin_page: &BlockHeight,
    current_height: &BlockHeight,
) -> bool {
    spend_condition.iter().all(|primitive| match primitive {
        LockPrimitive::Tim(tim) => {
            timelock_primitive_satisfied(tim, note_origin_page, current_height)
        }
        _ => true,
    })
}

/// Evaluates a single timelock primitive against note origin height and
/// current chain height.
fn timelock_primitive_satisfied(
    tim: &LockTim,
    note_origin_page: &BlockHeight,
    current_height: &BlockHeight,
) -> bool {
    let now = height_value(current_height);
    let since = height_value(note_origin_page);
    let rel_min_ok = tim.rel.min.as_ref().is_none_or(|min| {
        since
            .checked_add((min.0).0)
            .is_some_and(|required| now >= required)
    });
    let rel_max_ok = tim.rel.max.as_ref().is_none_or(|max| {
        since
            .checked_add((max.0).0)
            .is_some_and(|required| now <= required)
    });
    let abs_min_ok = tim
        .abs
        .min
        .as_ref()
        .is_none_or(|min| now >= height_value(min));
    let abs_max_ok = tim
        .abs
        .max
        .as_ref()
        .is_none_or(|max| now <= height_value(max));
    rel_min_ok && rel_max_ok && abs_min_ok && abs_max_ok
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use nockapp::noun::NounEncodeJamExt;
    use nockchain_math::belt::Belt;
    use nockchain_math::crypto::cheetah::{ch_scal, A_GEN};
    use nockchain_types::tx_engine::common::{
        BlockHeight, BlockHeightDelta, Hash, Name, Nicks, SchnorrPubkey, TimelockRangeAbsolute,
        TimelockRangeRelative, Version,
    };
    use nockchain_types::tx_engine::v0::{Lock as V0Lock, TimelockIntent as V0TimelockIntent};
    use nockchain_types::tx_engine::v1::tx::{LockPrimitive, LockTim, Pkh, SpendCondition};

    use super::*;
    use crate::lock_resolver::LockMatcher;
    use crate::note_data::{
        DecodedNoteData, DecodedNoteDataEntry, DecodedNoteDataPayload, LockDataPayload,
        NormalizedNoteDataKey,
    };
    use crate::types::{
        CandidateIdentity, CandidateNote, CandidateV0Note, CandidateV1Note, CandidateVersionPolicy,
        ChainContext, CreateTxPlanningMode, RawNoteDataEntry, RefundOutputTemplate, SelectionOrder,
        WithdrawalPlanRequest,
    };

    /// Constructs a deterministic hash value from a single test limb.
    fn hash(v: u64) -> Hash {
        Hash::from_limbs(&[v, 0, 0, 0, 0])
    }

    fn beid(start: u64) -> Vec<Belt> {
        (start..start + 32).map(Belt).collect()
    }

    /// Builds a deterministic note name pair for tests.
    fn name(v: u64) -> Name {
        Name::new(hash(v), hash(v + 100))
    }

    /// Builds a minimal candidate note with the provided asset amount.
    fn candidate(v: u64, assets: u64) -> CandidateNote {
        CandidateNote::V1(CandidateV1Note {
            identity: CandidateIdentity {
                name: name(v),
                origin_page: BlockHeight(Belt(10)),
            },
            assets: Nicks(assets as usize),
            raw_note_data: Vec::<RawNoteDataEntry>::new(),
            decoded_note_data: DecodedNoteData(Vec::new()),
        })
    }

    /// Builds a minimal candidate note with one decoded `%lock` entry.
    fn candidate_with_lock(
        v: u64,
        assets: u64,
        spend_conditions: Vec<SpendCondition>,
    ) -> CandidateNote {
        CandidateNote::V1(CandidateV1Note {
            identity: CandidateIdentity {
                name: name(v),
                origin_page: BlockHeight(Belt(10)),
            },
            assets: Nicks(assets as usize),
            raw_note_data: Vec::<RawNoteDataEntry>::new(),
            decoded_note_data: DecodedNoteData(vec![DecodedNoteDataEntry {
                raw_key: "lock".to_string(),
                normalized_key: NormalizedNoteDataKey::Lock,
                raw_blob: Bytes::new(),
                payload: DecodedNoteDataPayload::Lock(LockDataPayload {
                    version: 0,
                    spend_conditions,
                }),
                decode_error: None,
            }]),
        })
    }

    /// Builds a minimal v0 candidate note with the provided asset amount.
    fn candidate_v0(v: u64, assets: u64) -> CandidateNote {
        CandidateNote::V0(CandidateV0Note {
            identity: CandidateIdentity {
                name: name(v),
                origin_page: BlockHeight(Belt(10)),
            },
            assets: Nicks(assets as usize),
            lock: V0Lock {
                keys_required: 1,
                pubkeys: vec![signer_pubkey(1)],
            },
            timelock: None,
        })
    }

    fn candidate_v0_with_lock(
        v: u64,
        assets: u64,
        lock: V0Lock,
        timelock: Option<V0TimelockIntent>,
    ) -> CandidateNote {
        CandidateNote::V0(CandidateV0Note {
            identity: CandidateIdentity {
                name: name(v),
                origin_page: BlockHeight(Belt(10)),
            },
            assets: Nicks(assets as usize),
            lock,
            timelock,
        })
    }

    /// Builds an output with note-data so seed-word accounting exercises metadata paths.
    fn output(lock_root: u64, amount: u64) -> PlannedOutput {
        PlannedOutput {
            lock_root: hash(lock_root),
            amount,
            note_data: vec![RawNoteDataEntry {
                key: "meta".to_string(),
                blob: 0_u64.jam_bytes(),
            }],
        }
    }

    /// Builds an output with no note-data for tests that isolate refund behavior.
    fn output_without_note_data(lock_root: u64, amount: u64) -> PlannedOutput {
        PlannedOutput {
            lock_root: hash(lock_root),
            amount,
            note_data: Vec::new(),
        }
    }

    fn signer_pubkey(multiplier: u64) -> SchnorrPubkey {
        if multiplier == 1 {
            SchnorrPubkey(A_GEN)
        } else {
            SchnorrPubkey(ch_scal(multiplier, &A_GEN).expect("scaled test pubkey"))
        }
    }

    fn v0_timelock_relative_min(min: u64) -> Option<V0TimelockIntent> {
        Some(V0TimelockIntent {
            absolute: TimelockRangeAbsolute::none(),
            relative: TimelockRangeRelative::new(Some(BlockHeightDelta(Belt(min))), None),
        })
    }

    /// Creates a simple single-signer PKH spend condition.
    fn simple_pkh_lock(pkh: Hash) -> SpendCondition {
        SpendCondition::new(vec![LockPrimitive::Pkh(Pkh::new(1, vec![pkh]))])
    }

    /// Creates a coinbase-style lock containing PKH + relative timelock.
    fn coinbase_like_lock(pkh: Hash, relative_min: u64) -> SpendCondition {
        SpendCondition::new(vec![
            LockPrimitive::Pkh(Pkh::new(1, vec![pkh])),
            LockPrimitive::Tim(LockTim {
                rel: TimelockRangeRelative::new(Some(BlockHeightDelta(Belt(relative_min))), None),
                abs: TimelockRangeAbsolute::none(),
            }),
        ])
    }

    /// Creates a baseline plan request used by planner unit tests.
    fn base_request() -> PlanRequest {
        PlanRequest {
            planning_mode: CreateTxPlanningMode::Standard,
            selection_mode: SelectionMode::Auto,
            order_direction: SelectionOrder::Ascending,
            include_data: true,
            chain_context: ChainContext {
                height: BlockHeight(Belt(10)),
                bythos_phase: BlockHeight(Belt(10)),
                base_fee: 0,
                input_fee_divisor: 4,
                min_fee: 0,
            },
            signer_pkh: Some(hash(999)),
            candidate_version_policy: CandidateVersionPolicy::V1Only,
            candidates: vec![candidate(1, 8), candidate(2, 3), candidate(3, 20)],
            recipient_outputs: vec![output(42, 10)],
            refund_output: output(43, 0),
            coinbase_relative_min: Some(5),
            v0_migration_signer_pubkeys: Vec::new(),
        }
    }

    fn base_withdrawal_request() -> WithdrawalPlanRequest {
        WithdrawalPlanRequest {
            chain_context: ChainContext {
                height: BlockHeight(Belt(10)),
                bythos_phase: BlockHeight(Belt(10)),
                base_fee: 0,
                input_fee_divisor: 4,
                min_fee: 1,
            },
            candidates: vec![
                candidate_with_lock(1, 70_000, vec![simple_pkh_lock(hash(101))]),
                candidate_with_lock(2, 61_000, vec![simple_pkh_lock(hash(102))]),
                candidate_with_lock(3, 300_000, vec![simple_pkh_lock(hash(103))]),
            ],
            burned_amount: crate::fee::NICKS_PER_NOCK * 2,
            nicks_fee_per_nock: 195,
            recipient_lock_root: hash(42),
            beid: beid(1),
            base_hash: hash(77),
            base_batch_end: 12,
            refund_output: RefundOutputTemplate {
                lock_root: hash(43),
                note_data: Vec::new(),
            },
        }
    }

    struct AlwaysMatches;
    impl LockMatcher for AlwaysMatches {
        /// Test matcher that accepts every first-name/lock combination.
        fn matches(&self, _note_first_name: &Hash, _spend_condition: &SpendCondition) -> bool {
            true
        }
    }

    struct MatchSingleSignerPkh {
        signer_pkh: Hash,
    }

    impl LockMatcher for MatchSingleSignerPkh {
        /// Test matcher that only accepts locks whose PKH primitive can be
        /// satisfied by `signer_pkh` with m=1.
        fn matches(&self, _note_first_name: &Hash, spend_condition: &SpendCondition) -> bool {
            let mut saw_pkh = false;
            for primitive in spend_condition.iter() {
                match primitive {
                    LockPrimitive::Pkh(pkh) => {
                        saw_pkh = true;
                        if pkh.m != 1 {
                            return false;
                        }
                        if !pkh.hashes.iter().any(|hash| hash == &self.signer_pkh) {
                            return false;
                        }
                    }
                    LockPrimitive::Tim(_) => {}
                    _ => return false,
                }
            }
            saw_pkh
        }
    }

    #[test]
    /// Verifies planner rejects empty recipient output lists.
    fn no_recipients_returns_error() {
        let mut request = base_request();
        request.recipient_outputs = Vec::new();
        let error = plan_create_tx(&request, &AlwaysMatches).expect_err("expected no recipients");
        assert!(matches!(error, PlanError::NoRecipients));
    }

    #[test]
    /// Verifies v0 migration sweep consumes all admissible legacy notes and ignores v1 notes.
    fn v0_migration_sweep_selects_all_admissible_v0_candidates() {
        let mut request = base_request();
        request.planning_mode = CreateTxPlanningMode::V0MigrationSweep {
            destination_output: output_without_note_data(42, 0),
        };
        request.candidate_version_policy = CandidateVersionPolicy::V0Only;
        request.candidates = vec![candidate_v0(1, 8), candidate_v0(2, 3), candidate(3, 99)];
        request.recipient_outputs = Vec::new();
        request.v0_migration_signer_pubkeys = vec![signer_pubkey(1)];

        let result = plan_create_tx(&request, &AlwaysMatches).expect("migration plan");
        assert_eq!(result.selected.len(), 2);
        assert_eq!(result.selected_total, 11);
        assert_eq!(result.outputs.len(), 1);
        assert_eq!(result.outputs[0].amount, 11);
        assert_eq!(result.final_fee, 0);
    }

    #[test]
    /// Verifies v0 migration sweep skips legacy notes whose lock is not spendable by the migration signer.
    fn v0_migration_sweep_skips_unspendable_v0_candidates() {
        let mut request = base_request();
        request.planning_mode = CreateTxPlanningMode::V0MigrationSweep {
            destination_output: output_without_note_data(42, 0),
        };
        request.candidate_version_policy = CandidateVersionPolicy::V0Only;
        request.candidates = vec![
            candidate_v0(1, 8),
            candidate_v0_with_lock(
                2,
                13,
                V0Lock {
                    keys_required: 1,
                    pubkeys: vec![signer_pubkey(2)],
                },
                None,
            ),
        ];
        request.recipient_outputs = Vec::new();
        request.v0_migration_signer_pubkeys = vec![signer_pubkey(1)];

        let result = plan_create_tx(&request, &AlwaysMatches).expect("migration plan");
        assert_eq!(result.selected.len(), 1);
        assert_eq!(result.selected[0].name, name(1));
        assert_eq!(result.outputs[0].amount, 8);
    }

    #[test]
    /// Verifies v0 migration sweep skips legacy notes whose v0 timelock is not yet spendable.
    fn v0_migration_sweep_skips_unmatured_v0_timelocked_candidates() {
        let mut request = base_request();
        request.planning_mode = CreateTxPlanningMode::V0MigrationSweep {
            destination_output: output_without_note_data(42, 0),
        };
        request.candidate_version_policy = CandidateVersionPolicy::V0Only;
        request.candidates = vec![
            candidate_v0(1, 8),
            candidate_v0_with_lock(
                2,
                13,
                V0Lock {
                    keys_required: 1,
                    pubkeys: vec![signer_pubkey(1)],
                },
                v0_timelock_relative_min(5),
            ),
        ];
        request.recipient_outputs = Vec::new();
        request.v0_migration_signer_pubkeys = vec![signer_pubkey(1)];

        let result = plan_create_tx(&request, &AlwaysMatches).expect("migration plan");
        assert_eq!(result.selected.len(), 1);
        assert_eq!(result.selected[0].name, name(1));
        assert_eq!(result.outputs[0].amount, 8);
    }

    #[test]
    /// Verifies v0 migration sweep errors when fees consume the full selected value.
    fn v0_migration_sweep_errors_when_fee_consumes_all_selected_value() {
        let mut request = base_request();
        request.planning_mode = CreateTxPlanningMode::V0MigrationSweep {
            destination_output: output_without_note_data(42, 0),
        };
        request.chain_context.min_fee = 10;
        request.candidate_version_policy = CandidateVersionPolicy::V0Only;
        request.candidates = vec![candidate_v0(1, 10)];
        request.recipient_outputs = Vec::new();
        request.v0_migration_signer_pubkeys = vec![signer_pubkey(1)];

        let error =
            plan_create_tx(&request, &AlwaysMatches).expect_err("expected zero-value sweep error");
        assert!(matches!(
            error,
            PlanError::V0MigrationProducesZeroValue {
                selected_total: 10,
                fee: 10,
            }
        ));
    }

    #[test]
    /// Verifies manual mode requires at least one provided note name.
    fn manual_mode_requires_at_least_one_name() {
        let mut request = base_request();
        request.selection_mode = SelectionMode::Manual {
            note_names: Vec::new(),
        };
        let error =
            plan_create_tx(&request, &AlwaysMatches).expect_err("expected missing manual names");
        assert!(matches!(error, PlanError::ManualNamesMissing));
    }

    #[test]
    /// Verifies manual mode returns a structured error for unknown note names.
    fn manual_mode_unknown_note_name_returns_error() {
        let mut request = base_request();
        request.selection_mode = SelectionMode::Manual {
            note_names: vec![name(999)],
        };
        let error = plan_create_tx(&request, &AlwaysMatches).expect_err("expected missing note");
        assert!(matches!(error, PlanError::ManualNoteMissing { .. }));
    }

    #[test]
    /// Verifies manual mode rejects duplicate note names.
    fn manual_mode_duplicate_note_name_returns_error() {
        let mut request = base_request();
        request.selection_mode = SelectionMode::Manual {
            note_names: vec![name(1), name(1)],
        };
        let error =
            plan_create_tx(&request, &AlwaysMatches).expect_err("expected duplicate manual note");
        assert!(matches!(error, PlanError::DuplicateManualName { .. }));
    }

    #[test]
    /// Verifies auto mode consumes candidates in deterministic order until coverage.
    fn auto_mode_selects_ordered_notes_until_cover() {
        let request = base_request();
        let result = plan_create_tx(&request, &AlwaysMatches).expect("plan");

        assert_eq!(result.selected.len(), 2);
        assert_eq!(result.selected_total, 11);
        assert_eq!(result.final_fee, 0);
        assert_eq!(result.selected[0].name, name(2));
        assert_eq!(result.selected[1].name, name(1));
    }

    #[test]
    /// Verifies manual mode applies `SelectionOrder` after filtering to manual note names.
    fn manual_mode_orders_selected_candidates_by_selection_order() {
        let mut request = base_request();
        request.selection_mode = SelectionMode::Manual {
            note_names: vec![name(1), name(2)],
        };
        request.recipient_outputs = vec![output(42, 0)];

        let result = plan_create_tx(&request, &AlwaysMatches).expect("plan");
        assert_eq!(result.selected.len(), 2);
        assert_eq!(result.selected[0].name, name(2));
        assert_eq!(result.selected[1].name, name(1));
    }

    #[test]
    /// Verifies manual mode descending order reverses assets ordering for selected candidates.
    fn manual_mode_descending_orders_selected_candidates_by_selection_order() {
        let mut request = base_request();
        request.selection_mode = SelectionMode::Manual {
            note_names: vec![name(1), name(2)],
        };
        request.order_direction = SelectionOrder::Descending;
        request.recipient_outputs = vec![output(42, 0)];

        let result = plan_create_tx(&request, &AlwaysMatches).expect("plan");
        assert_eq!(result.selected.len(), 2);
        assert_eq!(result.selected[0].name, name(1));
        assert_eq!(result.selected[1].name, name(2));
    }

    #[test]
    /// Verifies v0-only selector policy rejects manual v1 candidates.
    fn manual_mode_v0_only_policy_rejects_v1_candidates() {
        let mut request = base_request();
        request.selection_mode = SelectionMode::Manual {
            note_names: vec![name(1)],
        };
        request.candidate_version_policy = CandidateVersionPolicy::V0Only;
        request.candidates = vec![candidate(1, 8)];
        request.recipient_outputs = vec![output(42, 10)];

        let error = plan_create_tx(&request, &AlwaysMatches)
            .expect_err("expected v0-only policy rejection for v1 manual selection");
        assert!(matches!(
            error,
            PlanError::CandidateVersionDisabled {
                version: Version::V1,
                policy: CandidateVersionPolicy::V0Only,
                ..
            }
        ));
    }

    #[test]
    /// Verifies planner consumes fee capacity when adding a refund output would
    /// increase the minimum fee beyond available capacity.
    fn auto_mode_consumes_capacity_as_fee_when_refund_output_is_not_fee_viable() {
        let signer = hash(999);
        let mut request = base_request();
        request.chain_context.base_fee = 1;
        request.chain_context.input_fee_divisor = 1_000_000_000;
        request.candidates = vec![candidate(1, 8), candidate(2, 4)];
        request.recipient_outputs = vec![output_without_note_data(42, 10)];
        request.refund_output = output_without_note_data(43, 0);
        request.signer_pkh = Some(signer);
        request.coinbase_relative_min = None;

        let result = plan_create_tx(&request, &AlwaysMatches).expect("plan");
        assert_eq!(result.selected_total, 12);
        assert_eq!(result.final_fee, 2);
        assert_eq!(
            result.outputs.len(),
            1,
            "no refund output should be emitted"
        );
    }

    #[test]
    /// Verifies output assembly appends refund output when refund amount is positive.
    fn outputs_with_refund_appends_refund_output_when_positive() {
        let request = base_request();
        let outputs = outputs_with_refund(&request, 1);
        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[1].amount, 1);
    }

    #[test]
    /// Verifies withdrawal fee word counting does not depend on the solved
    /// net recipient amount under the current estimator, as long as the
    /// refund-output shape is unchanged. If refund drops to zero, the refund
    /// output is omitted and the seed-word count may change. The planner's
    /// probe and final outputs share the same refund amount; only the solved
    /// recipient amount differs between them.
    fn withdrawal_seed_words_do_not_depend_on_recipient_amount() {
        let request = base_withdrawal_request();
        let estimator = WordCountEstimator::new(&request.chain_context);
        // Keep refund positive in both outputs. The planner uses the same
        // refund for its seed-word probe and final output assembly; crossing
        // to zero would remove the refund output entirely, which is a
        // different fee-shape case.
        let refund = 1;
        let spendable_amount = request.burned_amount
            - compute_bridge_fee(request.burned_amount, request.nicks_fee_per_nock);

        let full_amount = withdrawal_outputs(&request, spendable_amount, refund);
        let reduced_amount = withdrawal_outputs(&request, spendable_amount - 1, refund);

        assert_eq!(
            estimator.estimate_seed_words(&full_amount),
            estimator.estimate_seed_words(&reduced_amount)
        );
    }

    #[test]
    /// Verifies withdrawal planning solves a net payout whose fee is covered by
    /// the gross burned amount.
    fn withdrawal_planner_solves_net_amount_from_burned_amount() {
        let request = base_withdrawal_request();
        let result = plan_withdrawal_tx(&request, &AlwaysMatches).expect("plan withdrawal");

        assert_eq!(
            result.withdrawal_fee,
            compute_bridge_fee(request.burned_amount, request.nicks_fee_per_nock)
        );
        assert!(result.plan.final_fee > 0);
        assert_eq!(
            result.burned_amount,
            result
                .withdrawal_fee
                .saturating_add(result.net_recipient_amount)
                .saturating_add(result.plan.final_fee)
        );
        assert_eq!(
            result.burned_amount,
            result
                .withdrawal_fee
                .saturating_add(result.plan.outputs[0].amount)
                .saturating_add(result.plan.final_fee)
        );
        assert_eq!(result.plan.outputs[0].amount, result.net_recipient_amount);
        assert_eq!(result.plan.selected_total, 131_000);
        assert!(result.plan.selected_total < result.burned_amount);
    }

    #[test]
    fn withdrawal_planner_selects_lock_root_owned_note_without_lock_note_data() {
        let spend_condition = simple_pkh_lock(hash(777));
        let lock_root = spend_condition.hash().expect("lock root");
        let note_first_name = spend_condition
            .first_name()
            .expect("first-name")
            .into_hash();
        let mut request = base_withdrawal_request();
        request.candidates = vec![candidate(1, 140_000)];
        request.candidates[0].identity_mut().name.first = note_first_name;

        let matcher = crate::lock_resolver::LockRootLockMatcher::from_lock_root(&lock_root)
            .expect("matcher")
            .with_spend_condition(spend_condition);
        let result = plan_withdrawal_tx(&request, &matcher).expect("plan withdrawal");

        assert_eq!(result.plan.selected.len(), 1);
        assert_eq!(
            result.plan.selected[0].name,
            request.candidates[0].identity().name
        );
        assert_eq!(result.plan.selected_total, 140_000);
        assert!(result.plan.word_counts.witness_words > 0);
    }

    #[test]
    fn withdrawal_planner_errors_when_lock_root_matcher_omits_planning_spend_condition() {
        let spend_condition = simple_pkh_lock(hash(777));
        let lock_root = spend_condition.hash().expect("lock root");
        let note_first_name = spend_condition
            .first_name()
            .expect("first-name")
            .into_hash();
        let mut request = base_withdrawal_request();
        request.candidates = vec![candidate(1, 140_000)];
        request.candidates[0].identity_mut().name.first = note_first_name;

        let matcher =
            crate::lock_resolver::LockRootLockMatcher::from_lock_root(&lock_root).expect("matcher");
        let error =
            plan_withdrawal_tx(&request, &matcher).expect_err("expected planning metadata error");

        assert!(matches!(
            error,
            PlanError::MissingPlanningSpendCondition {
                resolution_source: LockResolutionSource::LockRootFirstName,
                ..
            }
        ));
    }

    #[test]
    /// Verifies withdrawals fail when the burned amount cannot cover the final fee.
    fn withdrawal_planner_rejects_burned_amount_smaller_than_fee() {
        let mut request = base_withdrawal_request();
        request.burned_amount = 1;
        request.candidates = vec![candidate(1, 5)];

        let error =
            plan_withdrawal_tx(&request, &AlwaysMatches).expect_err("expected burned amount error");
        assert!(matches!(
            error,
            PlanError::WithdrawalBurnedAmountTooSmall {
                burned_amount: 1,
                withdrawal_fee: 195,
                ..
            }
        ));
    }

    #[test]
    /// Verifies notes rejected by a signer-aware matcher are skipped and do not
    /// block selection of later spendable notes.
    fn auto_mode_skips_notes_unmatched_by_signer_matcher() {
        let signer = hash(999);
        let unmatched_lock = simple_pkh_lock(hash(111));
        let matched_lock = simple_pkh_lock(signer.clone());
        let mut request = base_request();
        request.candidates = vec![
            candidate_with_lock(1, 5, vec![unmatched_lock.clone()]),
            candidate_with_lock(2, 8, vec![matched_lock.clone()]),
        ];
        request.candidates[0].identity_mut().name.first =
            unmatched_lock.first_name().expect("first-name").into_hash();
        request.candidates[1].identity_mut().name.first =
            matched_lock.first_name().expect("first-name").into_hash();
        let expected_selected_name = request.candidates[1].identity().name.clone();
        request.recipient_outputs = vec![output(42, 8)];
        request.signer_pkh = None;
        request.coinbase_relative_min = None;

        let matcher = MatchSingleSignerPkh { signer_pkh: signer };
        let result = plan_create_tx(&request, &matcher).expect("plan");
        assert_eq!(result.selected.len(), 1);
        assert_eq!(result.selected[0].name, expected_selected_name);
    }

    #[test]
    /// Verifies unresolved locks are skipped and eventually surface as insufficient funds.
    fn unresolved_locks_are_skipped_until_insufficient_funds() {
        struct NeverMatches;
        impl LockMatcher for NeverMatches {
            /// Test matcher that rejects every first-name/lock combination.
            fn matches(&self, _note_first_name: &Hash, _spend_condition: &SpendCondition) -> bool {
                false
            }
        }

        let mut request = base_request();
        request.signer_pkh = None;
        request.coinbase_relative_min = None;
        let error = plan_create_tx(&request, &NeverMatches).expect_err("expected lock error");
        assert!(matches!(error, PlanError::InsufficientFunds { .. }));
    }

    #[test]
    /// Verifies v0-only selector policy skips v1 notes in auto mode.
    fn auto_mode_v0_only_policy_skips_v1_candidates() {
        let mut request = base_request();
        request.candidate_version_policy = CandidateVersionPolicy::V0Only;
        request.candidates = vec![candidate(1, 8), candidate_v0(2, 8)];
        request.recipient_outputs = vec![output(42, 12)];

        let error = plan_create_tx(&request, &AlwaysMatches)
            .expect_err("expected insufficient funds after v1 skip in v0-only mode");
        assert!(matches!(error, PlanError::InsufficientFunds { .. }));
    }

    #[test]
    /// Verifies v0-only selector policy can select v0 candidates.
    fn auto_mode_v0_only_policy_selects_v0_candidates() {
        struct NeverMatches;
        impl LockMatcher for NeverMatches {
            fn matches(&self, _note_first_name: &Hash, _spend_condition: &SpendCondition) -> bool {
                false
            }
        }

        let mut request = base_request();
        request.candidate_version_policy = CandidateVersionPolicy::V0Only;
        request.candidates = vec![candidate(1, 8), candidate_v0(2, 12)];
        request.recipient_outputs = vec![output(42, 10)];
        request.v0_migration_signer_pubkeys = vec![signer_pubkey(1)];

        let result = plan_create_tx(&request, &NeverMatches).expect("plan");
        assert_eq!(result.selected.len(), 1);
        assert_eq!(result.selected[0].name, name(2));
    }

    #[test]
    /// Verifies v1-only selector policy skips v0 notes in auto mode.
    fn auto_mode_v1_only_policy_skips_v0_candidates() {
        let mut request = base_request();
        request.candidates = vec![candidate_v0(1, 100)];
        request.recipient_outputs = vec![output(42, 10)];

        let error = plan_create_tx(&request, &AlwaysMatches)
            .expect_err("expected insufficient funds with v0 filtered out");
        assert!(matches!(error, PlanError::InsufficientFunds { .. }));
    }

    #[test]
    /// Verifies v1-only selector policy rejects manual v0 candidates.
    fn manual_mode_v1_only_policy_rejects_v0_candidates() {
        let mut request = base_request();
        request.selection_mode = SelectionMode::Manual {
            note_names: vec![name(1)],
        };
        request.candidates = vec![candidate_v0(1, 100)];
        request.recipient_outputs = vec![output(42, 10)];

        let error = plan_create_tx(&request, &AlwaysMatches)
            .expect_err("expected v1-only policy rejection for v0 manual selection");
        assert!(matches!(
            error,
            PlanError::CandidateVersionDisabled {
                version: Version::V0,
                policy: CandidateVersionPolicy::V1Only,
                ..
            }
        ));
    }

    #[test]
    /// Verifies auto mode skips notes gated by unsatisfied timelocks.
    fn auto_mode_skips_timelocked_notes_that_are_not_spendable_yet() {
        let signer = hash(999);
        let timelocked = coinbase_like_lock(signer.clone(), 5);
        let spendable = simple_pkh_lock(signer);

        let mut request = base_request();
        request.candidates = vec![
            candidate_with_lock(1, 8, vec![timelocked.clone()]),
            candidate_with_lock(2, 8, vec![spendable.clone()]),
        ];
        request.candidates[0].identity_mut().name.first =
            timelocked.first_name().expect("first-name").into_hash();
        request.candidates[1].identity_mut().name.first =
            spendable.first_name().expect("first-name").into_hash();
        let expected_selected_name = request.candidates[1].identity().name.clone();
        request.candidates[0].identity_mut().origin_page = BlockHeight(Belt(8));
        request.candidates[1].identity_mut().origin_page = BlockHeight(Belt(2));
        request.recipient_outputs = vec![output(42, 8)];
        request.signer_pkh = None;
        request.coinbase_relative_min = None;

        let result = plan_create_tx(&request, &AlwaysMatches).expect("plan");
        assert_eq!(result.selected.len(), 1);
        assert_eq!(result.selected[0].name, expected_selected_name);
        assert!(
            result
                .debug_trace
                .iter()
                .any(|entry| entry.contains("timelock not satisfied")),
            "expected debug trace to mention timelock filtering"
        );
    }

    #[test]
    /// Verifies manual mode also skips notes gated by unsatisfied timelocks.
    fn manual_mode_skips_timelocked_notes_that_are_not_spendable_yet() {
        let signer = hash(999);
        let timelocked = coinbase_like_lock(signer.clone(), 5);
        let spendable = simple_pkh_lock(signer);

        let mut request = base_request();
        request.candidates = vec![
            candidate_with_lock(1, 8, vec![timelocked.clone()]),
            candidate_with_lock(2, 8, vec![spendable.clone()]),
        ];
        request.candidates[0].identity_mut().name.first =
            timelocked.first_name().expect("first-name").into_hash();
        request.candidates[1].identity_mut().name.first =
            spendable.first_name().expect("first-name").into_hash();
        let selected_name = request.candidates[1].identity().name.clone();
        request.selection_mode = SelectionMode::Manual {
            note_names: request
                .candidates
                .iter()
                .map(|candidate| candidate.identity().name.clone())
                .collect(),
        };
        request.candidates[0].identity_mut().origin_page = BlockHeight(Belt(8));
        request.candidates[1].identity_mut().origin_page = BlockHeight(Belt(2));
        request.recipient_outputs = vec![output(42, 8)];
        request.signer_pkh = None;
        request.coinbase_relative_min = None;

        let result = plan_create_tx(&request, &AlwaysMatches).expect("plan");
        assert_eq!(result.selected.len(), 1);
        assert_eq!(result.selected[0].name, selected_name);
        assert!(
            result
                .debug_trace
                .iter()
                .any(|entry| entry.contains("timelock not satisfied")),
            "expected debug trace to mention timelock filtering"
        );
    }

    #[test]
    /// Verifies relative timelock min/max are inclusive at boundaries and
    /// fail outside those bounds.
    fn timelock_relative_bounds_apply_at_edges() {
        let tim = LockTim {
            rel: TimelockRangeRelative::new(
                Some(BlockHeightDelta(Belt(5))),
                Some(BlockHeightDelta(Belt(7))),
            ),
            abs: TimelockRangeAbsolute::none(),
        };
        let origin = BlockHeight(Belt(100));

        assert!(!timelock_primitive_satisfied(
            &tim,
            &origin,
            &BlockHeight(Belt(104))
        ));
        assert!(timelock_primitive_satisfied(
            &tim,
            &origin,
            &BlockHeight(Belt(105))
        ));
        assert!(timelock_primitive_satisfied(
            &tim,
            &origin,
            &BlockHeight(Belt(107))
        ));
        assert!(!timelock_primitive_satisfied(
            &tim,
            &origin,
            &BlockHeight(Belt(108))
        ));
    }

    #[test]
    /// Verifies absolute timelock min/max are inclusive at boundaries and
    /// fail outside those bounds.
    fn timelock_absolute_bounds_apply_at_edges() {
        let tim = LockTim {
            rel: TimelockRangeRelative::none(),
            abs: TimelockRangeAbsolute::new(
                Some(BlockHeight(Belt(200))),
                Some(BlockHeight(Belt(202))),
            ),
        };
        let origin = BlockHeight(Belt(0));

        assert!(!timelock_primitive_satisfied(
            &tim,
            &origin,
            &BlockHeight(Belt(199))
        ));
        assert!(timelock_primitive_satisfied(
            &tim,
            &origin,
            &BlockHeight(Belt(200))
        ));
        assert!(timelock_primitive_satisfied(
            &tim,
            &origin,
            &BlockHeight(Belt(202))
        ));
        assert!(!timelock_primitive_satisfied(
            &tim,
            &origin,
            &BlockHeight(Belt(203))
        ));
    }

    #[test]
    /// Verifies overflow in relative timelock arithmetic is treated as
    /// unsatisfied rather than wrapping.
    fn timelock_relative_overflow_is_unsatisfied() {
        let tim = LockTim {
            rel: TimelockRangeRelative::new(Some(BlockHeightDelta(Belt(10))), None),
            abs: TimelockRangeAbsolute::none(),
        };
        let origin = BlockHeight(Belt(u64::MAX - 1));
        assert!(!timelock_primitive_satisfied(
            &tim,
            &origin,
            &BlockHeight(Belt(u64::MAX))
        ));
    }
}
