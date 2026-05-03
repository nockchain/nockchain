use std::collections::BTreeSet;

use nockchain_types::tx_engine::common::{BlockHeight, SchnorrPubkey, Version};
use nockchain_types::tx_engine::v0::TimelockIntent as V0TimelockIntent;
use nockchain_types::tx_engine::v1::tx::{LockPrimitive, LockTim, SpendCondition};
use thiserror::Error;

use crate::determinism::sort_candidates;
use crate::fee::{compute_minimum_fee, FeeInputs};
use crate::lock_resolver::{LockMatcher, LockResolutionSource, ResolveLockRequest};
use crate::types::{
    CandidateNote, CandidateV0Note, CandidateVersionPolicy, PlanRequest, PlanResult, PlannedOutput,
    PlanningMode, SelectionMode, WordCountBreakdown,
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
}

#[derive(Debug, Clone)]
/// One selected candidate note tracked in planner state.
struct SelectedInput {
    /// Candidate note accepted into the current plan.
    candidate: CandidateNote,
}

#[derive(Debug, Clone)]
/// Recomputed fee/output state for standard planner output selection.
struct StandardRecomputeState {
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

#[derive(Debug)]
/// Mutable planner session carrying running selection and fee state.
struct PlanSession<'a> {
    /// Immutable request/configuration for this planning run.
    request: &'a PlanRequest,
    /// Word-count estimator bound to request chain context.
    word_count_estimator: WordCountEstimator<'a>,
    /// Sum of recipient gift amounts (target transfer value).
    gift_total: u64,
    /// Seed-word baseline for recipient outputs only (no refund output).
    /// This is used as the first lower-bound fee check before considering refund shape.
    seed_words_without_refund: u64,
    /// Running total of witness words for all currently selected inputs.
    witness_words_total: u64,
    /// Selected inputs in deterministic order.
    selected: Vec<SelectedInput>,
    /// Running sum of selected input assets.
    selected_total: u64,
    /// Human-readable decision trace emitted in plan result.
    debug_trace: Vec<String>,
}

impl<'a> PlanSession<'a> {
    /// Initializes a planning session with immutable request context and
    /// precomputed recipient-side seed word baseline.
    fn new(request: &'a PlanRequest) -> Self {
        let word_count_estimator = WordCountEstimator::new(&request.chain_context);
        let (gift_total, seed_words_without_refund) = match &request.planning_mode {
            PlanningMode::Standard => {
                let gift_total = request
                    .recipient_outputs
                    .iter()
                    .fold(0u64, |acc, output| acc.saturating_add(output.amount));
                let seed_words_without_refund =
                    word_count_estimator.estimate_seed_words(&request.recipient_outputs);
                (gift_total, seed_words_without_refund)
            }
            PlanningMode::V0MigrationSweep { destination_output } => (
                0,
                word_count_estimator.estimate_seed_words(std::slice::from_ref(destination_output)),
            ),
        };
        Self {
            request,
            word_count_estimator,
            gift_total,
            seed_words_without_refund,
            witness_words_total: 0,
            selected: Vec::new(),
            selected_total: 0,
            debug_trace: Vec::new(),
        }
    }

    /// Returns the total amount needed to satisfy gifts plus the provided fee.
    fn required_total(&self, fee: u64) -> u64 {
        self.gift_total.saturating_add(fee)
    }

    /// Applies the request's candidate-version policy and emits the usual
    /// manual/auto-mode version mismatch diagnostics.
    fn candidate_version_allowed(&mut self, candidate: &CandidateNote) -> Result<bool, PlanError> {
        let candidate_version = candidate.version();
        let allowed_version = match self.request.candidate_version_policy {
            CandidateVersionPolicy::V1Only => Version::V1,
            CandidateVersionPolicy::V0Only => Version::V0,
        };
        if candidate_version != allowed_version {
            let (first, last) = candidate.note_name_display();
            if matches!(&self.request.selection_mode, SelectionMode::Manual { .. }) {
                return Err(PlanError::CandidateVersionDisabled {
                    version: candidate_version,
                    policy: self.request.candidate_version_policy,
                    first,
                    last,
                });
            }
            self.debug_trace.push(format!(
                "skipped note {first}/{last}: version {candidate_version:?} disabled by selector policy {policy:?}",
                policy = self.request.candidate_version_policy
            ));
            return Ok(false);
        }

        Ok(true)
    }

    /// Attempts to add one standard candidate note, resolving spendability and
    /// updating running witness/asset totals when accepted.
    fn try_select_standard_candidate<M: LockMatcher>(
        &mut self,
        candidate: CandidateNote,
        matcher: &M,
    ) -> Result<Option<StandardRecomputeState>, PlanError> {
        if !self.candidate_version_allowed(&candidate)? {
            return Ok(None);
        }

        if let CandidateNote::V0(note) = candidate {
            return self.try_select_standard_v0_candidate(note);
        }

        let resolution = matcher.resolve_lock(ResolveLockRequest {
            note_first_name: &candidate.identity().name.first,
            decoded_note_data: candidate.decoded_note_data(),
            signer_pkh: self.request.signer_pkh.as_ref(),
            coinbase_relative_min: self.request.coinbase_relative_min,
        });
        let Some(spend_condition) = resolution.spend_condition else {
            let (first, last) = candidate.note_name_display();
            if matches!(&self.request.selection_mode, SelectionMode::Manual { .. }) {
                return Err(PlanError::UnknownLock {
                    first,
                    last,
                    resolution_source: resolution.source,
                });
            }
            self.debug_trace.push(format!(
                "skipped note {first}/{last}: unresolved lock source={:?}",
                resolution.source
            ));
            return Ok(None);
        };
        if !timelock_satisfied(
            &spend_condition,
            &candidate.identity().origin_page,
            &self.request.chain_context.height,
        ) {
            let (first, last) = candidate.note_name_display();
            self.debug_trace.push(format!(
                "skipped note {first}/{last}: timelock not satisfied at height={}",
                height_value(&self.request.chain_context.height)
            ));
            return Ok(None);
        }

        let candidate_assets = candidate.assets().0 as u64;
        let (first, last) = candidate.note_name_display();
        let witness_words_for_input =
            self.word_count_estimator
                .estimate_witness_words_for_input(&WitnessWordInput {
                    spend_condition: spend_condition.clone(),
                    input_origin_page: candidate.identity().origin_page.clone(),
                    spend_condition_count: resolution.spend_condition_count,
                });
        self.selected_total = self.selected_total.saturating_add(candidate_assets);
        self.witness_words_total = self
            .witness_words_total
            .saturating_add(witness_words_for_input);
        self.selected.push(SelectedInput { candidate });

        let recompute = self.recompute_standard_fee();
        let required = self.required_total(recompute.final_fee);
        self.debug_trace.push(format!(
            "selected note {first}/{last} assets={} selected_total={} seed_words={} witness_words={} min_fee={} final_fee={} required={}",
            candidate_assets,
            self.selected_total,
            recompute.seed_words,
            recompute.witness_words,
            recompute.minimum_fee,
            recompute.final_fee,
            required
        ));

        Ok(Some(recompute))
    }

    /// Attempts to admit one legacy v0 note under the standard create-tx flow.
    fn try_select_standard_v0_candidate(
        &mut self,
        note: CandidateV0Note,
    ) -> Result<Option<StandardRecomputeState>, PlanError> {
        let (first, last) = (
            note.identity.name.first.to_base58(),
            note.identity.name.last.to_base58(),
        );
        if !v0_note_spendable_by_signers(&note, &self.request.v0_migration_signer_pubkeys) {
            self.debug_trace.push(format!(
                "skipped note {first}/{last}: legacy lock is not spendable by planner signer pubkeys",
            ));
            return Ok(None);
        }
        if !v0_timelock_satisfied(
            note.timelock.as_ref(),
            &note.identity.origin_page,
            &self.request.chain_context.height,
        ) {
            self.debug_trace.push(format!(
                "skipped note {first}/{last}: v0 timelock not satisfied at height={}",
                height_value(&self.request.chain_context.height)
            ));
            return Ok(None);
        }

        let candidate_assets = note.assets.0 as u64;
        let witness_words_for_input = self
            .word_count_estimator
            .estimate_v0_witness_words(note.lock.keys_required);
        self.selected_total = self.selected_total.saturating_add(candidate_assets);
        self.witness_words_total = self
            .witness_words_total
            .saturating_add(witness_words_for_input);
        self.selected.push(SelectedInput {
            candidate: CandidateNote::V0(note),
        });

        let recompute = self.recompute_standard_fee();
        let required = self.required_total(recompute.final_fee);
        self.debug_trace.push(format!(
            "selected note {first}/{last} assets={} selected_total={} seed_words={} witness_words={} min_fee={} final_fee={} required={}",
            candidate_assets,
            self.selected_total,
            recompute.seed_words,
            recompute.witness_words,
            recompute.minimum_fee,
            recompute.final_fee,
            required
        ));

        Ok(Some(recompute))
    }

    /// Attempts to admit one legacy v0 note under the v0 migration sweep rules.
    fn try_select_v0_migration_candidate(
        &mut self,
        candidate: CandidateNote,
    ) -> Result<Option<V0MigrationRecomputeState>, PlanError> {
        if !self.candidate_version_allowed(&candidate)? {
            return Ok(None);
        }

        let CandidateNote::V0(note) = candidate else {
            return Ok(None);
        };
        let (first, last) = (
            note.identity.name.first.to_base58(),
            note.identity.name.last.to_base58(),
        );
        if !v0_note_spendable_by_signers(&note, &self.request.v0_migration_signer_pubkeys) {
            self.debug_trace.push(format!(
                "skipped note {first}/{last}: legacy lock is not spendable by migration signer pubkeys",
            ));
            return Ok(None);
        }
        if !v0_timelock_satisfied(
            note.timelock.as_ref(),
            &note.identity.origin_page,
            &self.request.chain_context.height,
        ) {
            self.debug_trace.push(format!(
                "skipped note {first}/{last}: v0 timelock not satisfied at height={}",
                height_value(&self.request.chain_context.height)
            ));
            return Ok(None);
        }

        let candidate_assets = note.assets.0 as u64;
        let witness_words_for_input = self
            .word_count_estimator
            .estimate_v0_witness_words(note.lock.keys_required);
        self.selected_total = self.selected_total.saturating_add(candidate_assets);
        self.witness_words_total = self
            .witness_words_total
            .saturating_add(witness_words_for_input);
        self.selected.push(SelectedInput {
            candidate: CandidateNote::V0(note),
        });

        let recompute = self.recompute_v0_migration();
        self.debug_trace.push(format!(
            "selected migration note {first}/{last} assets={} selected_total={} seed_words={} witness_words={} min_fee={} final_fee={}",
            candidate_assets,
            self.selected_total,
            recompute.seed_words(),
            recompute.witness_words(),
            recompute.minimum_fee(),
            recompute.final_fee(),
        ));

        Ok(Some(recompute))
    }

    /// Recomputes fee and output shape from the current standard selected-input state.
    fn recompute_standard_fee(&self) -> StandardRecomputeState {
        let witness_words = self.witness_words_total;
        let fee_capacity = self.selected_total.saturating_sub(self.gift_total);
        let minimum_without_refund =
            self.minimum_fee(self.seed_words_without_refund, witness_words);
        let mut final_fee = minimum_without_refund;
        if fee_capacity > minimum_without_refund {
            let refund_if_min_without = fee_capacity.saturating_sub(minimum_without_refund);
            let outputs_with_refund = outputs_with_refund(self.request, refund_if_min_without);
            let seed_words_with_refund = self
                .word_count_estimator
                .estimate_seed_words(&outputs_with_refund);
            let minimum_with_refund = self.minimum_fee(seed_words_with_refund, witness_words);
            if fee_capacity > minimum_with_refund {
                final_fee = minimum_with_refund;
            } else {
                // If we cannot fit min-fee-with-refund, consume the remainder as fee and
                // emit no refund output to preserve conservation without iterative toggling.
                // This never increases gifts: recipient outputs are fixed by
                // `request.recipient_outputs`; only the leftover split between refund and
                // fee changes in this branch.
                final_fee = fee_capacity;
            }
        }

        let refund = self.refund_amount(final_fee);
        let outputs = outputs_with_refund(self.request, refund);
        let seed_words = self.word_count_estimator.estimate_seed_words(&outputs);
        let minimum_fee = self.minimum_fee(seed_words, witness_words);
        StandardRecomputeState {
            final_fee,
            minimum_fee,
            seed_words,
            witness_words,
            outputs,
        }
    }

    /// Recomputes fee and destination output shape for the current v0 migration selection.
    fn recompute_v0_migration(&self) -> V0MigrationRecomputeState {
        let PlanningMode::V0MigrationSweep { destination_output } = &self.request.planning_mode
        else {
            unreachable!("v0 migration recompute should only run in V0MigrationSweep mode");
        };
        let witness_words = self.witness_words_total;
        let seed_words = self
            .word_count_estimator
            .estimate_seed_words(std::slice::from_ref(destination_output));
        let minimum_fee = self.minimum_fee(seed_words, witness_words);
        let final_fee = minimum_fee;
        let Some(sweep_amount) = self.selected_total.checked_sub(final_fee) else {
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

    /// Computes minimum fee from seed/witness word counts under current chain rules.
    fn minimum_fee(&self, seed_words: u64, witness_words: u64) -> u64 {
        compute_minimum_fee(FeeInputs {
            seed_words,
            witness_words,
            base_fee: self.request.chain_context.base_fee,
            input_fee_divisor: self.request.chain_context.input_fee_divisor,
            min_fee: self.request.chain_context.min_fee,
            height: self.request.chain_context.height.clone(),
            bythos_phase: self.request.chain_context.bythos_phase.clone(),
        })
        .minimum_fee
    }

    /// Computes refundable remainder for a candidate final fee.
    fn refund_amount(&self, fee: u64) -> u64 {
        let required = self.gift_total.saturating_add(fee);
        self.selected_total.saturating_sub(required)
    }
}

/// Plans input selection, fee, and outputs for create-tx using deterministic
/// ordering and lock/timelock spendability checks.
pub fn plan_create_tx<M: LockMatcher>(
    request: &PlanRequest,
    matcher: &M,
) -> Result<PlanResult, PlanError> {
    if matches!(&request.planning_mode, PlanningMode::Standard)
        && request.recipient_outputs.is_empty()
    {
        return Err(PlanError::NoRecipients);
    }

    let candidates = request.ordered_candidates()?;
    let mut session = PlanSession::new(request);

    match &request.planning_mode {
        PlanningMode::Standard => {
            for candidate in candidates {
                let Some(recompute) = session.try_select_standard_candidate(candidate, matcher)?
                else {
                    continue;
                };
                if matches!(&request.selection_mode, SelectionMode::Auto)
                    && session.selected_total >= session.required_total(recompute.final_fee)
                {
                    break;
                }
            }

            let recompute = session.recompute_standard_fee();
            let required = session.required_total(recompute.final_fee);
            if session.selected_total < required {
                return Err(PlanError::InsufficientFunds {
                    selected_total: session.selected_total,
                    required,
                });
            }

            let allocation = allocate_inputs(
                session.selected_total, session.gift_total, recompute.final_fee,
            )
            .expect("required <= selected_total should always allocate");
            let conservation = ConservationCheck {
                input_total: session.selected_total,
                gift_total: allocation.gift_total,
                refund_total: allocation.refund,
                fee: allocation.fee,
            };
            if !conservation.is_balanced() {
                return Err(PlanError::ConservationFailed);
            }

            Ok(PlanResult {
                selected: session
                    .selected
                    .iter()
                    .map(|input| input.candidate.identity().clone())
                    .collect(),
                selected_total: session.selected_total,
                outputs: recompute.outputs,
                final_fee: recompute.final_fee,
                word_counts: WordCountBreakdown {
                    seed_words: recompute.seed_words,
                    witness_words: recompute.witness_words,
                },
                debug_trace: session.debug_trace,
            })
        }
        PlanningMode::V0MigrationSweep { .. } => {
            for candidate in candidates {
                let _ = session.try_select_v0_migration_candidate(candidate)?;
            }

            let recompute = session.recompute_v0_migration();
            let ready = match recompute {
                V0MigrationRecomputeState::NeedsMoreInputs { final_fee, .. } => {
                    return Err(PlanError::V0MigrationProducesZeroValue {
                        selected_total: session.selected_total,
                        fee: final_fee,
                    });
                }
                V0MigrationRecomputeState::Ready(ready) => ready,
            };

            if session.selected_total
                != ready
                    .destination_output
                    .amount
                    .saturating_add(ready.final_fee)
            {
                return Err(PlanError::ConservationFailed);
            }

            Ok(PlanResult {
                selected: session
                    .selected
                    .iter()
                    .map(|input| input.candidate.identity().clone())
                    .collect(),
                selected_total: session.selected_total,
                outputs: vec![ready.destination_output],
                final_fee: ready.final_fee,
                word_counts: WordCountBreakdown {
                    seed_words: ready.seed_words,
                    witness_words: ready.witness_words,
                },
                debug_trace: session.debug_trace,
            })
        }
    }
}

impl PlanRequest {
    /// Produces candidate ordering for the selected mode:
    /// deterministic sort by `SelectionOrder` for both auto and manual candidate sets.
    fn ordered_candidates(&self) -> Result<Vec<CandidateNote>, PlanError> {
        match &self.selection_mode {
            SelectionMode::Auto => {
                let mut out = self.candidates.clone();
                sort_candidates(&mut out, self.order_direction);
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
                    let Some(candidate) = self
                        .candidates
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
                sort_candidates(&mut out, self.order_direction);
                Ok(out)
            }
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
            amount: refund,
            ..request.refund_output.clone()
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
    use nockapp::noun::slab::{NockJammer, NounSlab};
    use nockchain_math::belt::Belt;
    use nockchain_math::crypto::cheetah::{ch_scal, A_GEN};
    use nockchain_types::tx_engine::common::{
        BlockHeight, BlockHeightDelta, Hash, Name, Nicks, SchnorrPubkey, TimelockRangeAbsolute,
        TimelockRangeRelative, Version,
    };
    use nockchain_types::tx_engine::v0::{Lock as V0Lock, TimelockIntent as V0TimelockIntent};
    use nockchain_types::tx_engine::v1::tx::{Lock, LockPrimitive, LockTim, Pkh, SpendCondition};
    use noun_serde::NounEncode;

    use super::*;
    use crate::lock_resolver::LockMatcher;
    use crate::note_data::{
        DecodedNoteData, DecodedNoteDataEntry, DecodedNoteDataPayload, LockDataPayload,
        NormalizedNoteDataKey,
    };
    use crate::types::{
        CandidateIdentity, CandidateNote, CandidateV0Note, CandidateV1Note, CandidateVersionPolicy,
        ChainContext, PlanningMode, RawNoteDataEntry, SelectionOrder,
    };

    /// Constructs a deterministic hash value from a single test limb.
    fn hash(v: u64) -> Hash {
        Hash::from_limbs(&[v, 0, 0, 0, 0])
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
                blob: jam(&0_u64),
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

    fn p2pkh_output(lock_root: u64, amount: u64) -> PlannedOutput {
        PlannedOutput {
            lock_root: hash(lock_root),
            amount,
            note_data: vec![RawNoteDataEntry::from_lock(Lock::SpendCondition(simple_pkh_lock(
                hash(lock_root),
            )))],
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

    /// Jam-encodes an arbitrary noun-encodable test value into bytes.
    fn jam<T: NounEncode>(value: &T) -> Bytes {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = value.to_noun(&mut slab);
        slab.set_root(noun);
        slab.jam()
    }

    /// Creates a baseline plan request used by planner unit tests.
    fn base_request() -> PlanRequest {
        PlanRequest {
            planning_mode: PlanningMode::Standard,
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
        request.planning_mode = PlanningMode::V0MigrationSweep {
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
        request.planning_mode = PlanningMode::V0MigrationSweep {
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
        request.planning_mode = PlanningMode::V0MigrationSweep {
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
        request.planning_mode = PlanningMode::V0MigrationSweep {
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
    /// Verifies v0 migration uses the expected legacy witness words and fee under fakenet-style constants.
    fn v0_migration_sweep_matches_expected_word_and_fee_counts() {
        let mut request = base_request();
        request.planning_mode = PlanningMode::V0MigrationSweep {
            destination_output: p2pkh_output(42, 0),
        };
        request.chain_context = ChainContext {
            height: BlockHeight(Belt(1)),
            bythos_phase: BlockHeight(Belt(1)),
            base_fee: 128,
            input_fee_divisor: 4,
            min_fee: 256,
        };
        request.candidate_version_policy = CandidateVersionPolicy::V0Only;
        request.candidates = vec![candidate_v0(1, 25_000)];
        request.recipient_outputs = Vec::new();
        request.v0_migration_signer_pubkeys = vec![signer_pubkey(1)];

        let result = plan_create_tx(&request, &AlwaysMatches).expect("migration plan");
        assert_eq!(result.selected_total, 25_000);
        assert_eq!(result.word_counts.seed_words, 14);
        assert_eq!(result.word_counts.witness_words, 31);
        assert_eq!(result.final_fee, 2_784);
        assert_eq!(result.outputs.len(), 1);
        assert_eq!(result.outputs[0].amount, 22_216);
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
