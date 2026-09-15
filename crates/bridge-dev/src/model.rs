use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const WITHDRAWAL_MODEL_SCHEMA_VERSION: u64 = 1;
const KERNEL_NODE_COUNT: u64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelBurnState {
    Absent,
    Canonical,
    Orphaned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPublicState {
    Pending,
    Ready,
    Submitted,
    SequencerConfirmed,
    Terminal,
    ReorgHold,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelProposalState {
    Assembled,
    Prepared,
    Canonicalized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelHoldKind {
    DeepBaseReorg,
    DeepNockReorg,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelNoteName {
    pub first: String,
    pub last: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelBurnFacts {
    pub withdrawal_id: String,
    pub nonce: u64,
    pub state: ModelBurnState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelProposalFacts {
    pub epoch: u64,
    pub handoff: u64,
    pub canonical_hash: String,
    pub state: ModelProposalState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInclusionFacts {
    pub transaction_id: String,
    pub height: u64,
    pub block_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WithdrawalModelState {
    pub schema_version: u64,
    pub burn: Option<ModelBurnFacts>,
    pub proposal: Option<ModelProposalFacts>,
    pub authorized_transactions: BTreeMap<u64, String>,
    pub active_transaction_id: Option<String>,
    pub submitted: bool,
    pub inclusion: Option<ModelInclusionFacts>,
    pub confirmed: bool,
    pub kernel_settled_nodes: BTreeSet<u64>,
    pub selected_inputs: BTreeSet<ModelNoteName>,
    #[serde(with = "reservation_map_serde")]
    pub reservations: BTreeMap<ModelNoteName, String>,
    pub released_inputs: BTreeSet<ModelNoteName>,
    pub reservation_release_count: u64,
    pub journal_generation: u64,
    pub replay_required: bool,
    pub hold: Option<ModelHoldKind>,
    pub public_state: ModelPublicState,
    pub payout_count: u64,
    pub refund_count: u64,
    pub terminal: bool,
}

impl Default for WithdrawalModelState {
    fn default() -> Self {
        Self {
            schema_version: WITHDRAWAL_MODEL_SCHEMA_VERSION,
            burn: None,
            proposal: None,
            authorized_transactions: BTreeMap::new(),
            active_transaction_id: None,
            submitted: false,
            inclusion: None,
            confirmed: false,
            kernel_settled_nodes: BTreeSet::new(),
            selected_inputs: BTreeSet::new(),
            reservations: BTreeMap::new(),
            released_inputs: BTreeSet::new(),
            reservation_release_count: 0,
            journal_generation: 0,
            replay_required: false,
            hold: None,
            public_state: ModelPublicState::Pending,
            payout_count: 0,
            refund_count: 0,
            terminal: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum WithdrawalModelAction {
    ObserveBurn {
        withdrawal_id: String,
        nonce: u64,
    },
    InvalidateBurn,
    ReadmitBurn,
    RecoverHold {
        hold: ModelHoldKind,
    },
    Assemble {
        epoch: u64,
        handoff: u64,
        proposal_hash: String,
        selected_inputs: BTreeSet<ModelNoteName>,
    },
    Prepare,
    Canonicalize,
    AdvanceHandoff {
        handoff: u64,
    },
    Reserve {
        owner: String,
        inputs: BTreeSet<ModelNoteName>,
    },
    RestoreReservations,
    Authorize {
        epoch: u64,
        transaction_id: String,
    },
    Submit {
        transaction_id: String,
    },
    Include {
        transaction_id: String,
        height: u64,
        block_id: String,
    },
    Confirm {
        transaction_id: String,
        height: u64,
        block_id: String,
    },
    SettleKernel {
        node_id: u64,
    },
    RecordPayout,
    RecordRefund,
    ReleaseReservations {
        owner: String,
        inputs: BTreeSet<ModelNoteName>,
    },
    Publish {
        state: ModelPublicState,
    },
    Restart {
        component: String,
    },
    ReplayJournal {
        generation: u64,
    },
    BaseReorg {
        deep: bool,
    },
    NockReorg {
        deep: bool,
        reinclusion_height: Option<u64>,
        reinclusion_block_id: Option<String>,
    },
}

impl WithdrawalModelAction {
    pub fn name(&self) -> &'static str {
        match self {
            Self::ObserveBurn { .. } => "observe_burn",
            Self::InvalidateBurn => "invalidate_burn",
            Self::ReadmitBurn => "readmit_burn",
            Self::RecoverHold { .. } => "recover_hold",
            Self::Assemble { .. } => "assemble",
            Self::Prepare => "prepare",
            Self::Canonicalize => "canonicalize",
            Self::AdvanceHandoff { .. } => "advance_handoff",
            Self::Reserve { .. } => "reserve",
            Self::RestoreReservations => "restore_reservations",
            Self::Authorize { .. } => "authorize",
            Self::Submit { .. } => "submit",
            Self::Include { .. } => "include",
            Self::Confirm { .. } => "confirm",
            Self::SettleKernel { .. } => "settle_kernel",
            Self::RecordPayout => "record_payout",
            Self::RecordRefund => "record_refund",
            Self::ReleaseReservations { .. } => "release_reservations",
            Self::Publish { .. } => "publish",
            Self::Restart { .. } => "restart",
            Self::ReplayJournal { .. } => "replay_journal",
            Self::BaseReorg { .. } => "base_reorg",
            Self::NockReorg { .. } => "nock_reorg",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WithdrawalModelEvent {
    BurnObserved,
    BurnOrphaned,
    BurnReadmitted,
    ProposalAssembled,
    ProposalPrepared,
    ProposalCanonicalized,
    HandoffAdvanced,
    InputsReserved,
    ReservationsRestored,
    TransactionAuthorized,
    TransactionSubmitted,
    TransactionIncluded,
    TransactionConfirmed,
    KernelSettled,
    PayoutRecorded,
    RefundRecorded,
    ReservationsReleased,
    PublicStateAdvanced,
    ComponentRestarted,
    JournalReplayed,
    HoldRecovered,
    NockReorgApplied,
    ReorgHoldEntered,
    DuplicateIgnored,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WithdrawalTransitionOutcome {
    pub action: String,
    pub changed: bool,
    pub events: Vec<WithdrawalModelEvent>,
    pub state_sha256: String,
}

impl WithdrawalModelState {
    pub fn apply(
        &mut self,
        action: &WithdrawalModelAction,
    ) -> Result<WithdrawalTransitionOutcome, WithdrawalModelError> {
        self.validate()?;
        let before = self.clone();
        let result = self.apply_inner(action);
        let mut events = match result {
            Ok(events) => events,
            Err(error) => {
                *self = before;
                return Err(error);
            }
        };
        if let Err(error) = self.validate() {
            *self = before;
            return Err(error);
        }
        let changed = *self != before;
        if !changed && events.is_empty() {
            events.push(WithdrawalModelEvent::DuplicateIgnored);
        }
        Ok(WithdrawalTransitionOutcome {
            action: action.name().to_owned(),
            changed,
            events,
            state_sha256: self.state_sha256()?,
        })
    }

    pub fn validate(&self) -> Result<(), WithdrawalModelError> {
        if self.schema_version != WITHDRAWAL_MODEL_SCHEMA_VERSION {
            return Err(WithdrawalModelError::UnsupportedSchema(self.schema_version));
        }
        if self
            .burn
            .as_ref()
            .is_some_and(|burn| burn.withdrawal_id.trim().is_empty())
            || self
                .proposal
                .as_ref()
                .is_some_and(|proposal| proposal.canonical_hash.trim().is_empty())
            || self
                .selected_inputs
                .iter()
                .any(|input| input.first.trim().is_empty() || input.last.trim().is_empty())
        {
            return Err(WithdrawalModelError::Invariant(
                "model identities must be nonempty",
            ));
        }
        if self.reservation_release_count > 1 {
            return Err(WithdrawalModelError::Invariant(
                "reservations released more than once",
            ));
        }
        if let Some(burn) = &self.burn {
            if self
                .reservations
                .values()
                .any(|owner| owner != &burn.withdrawal_id)
            {
                return Err(WithdrawalModelError::Invariant(
                    "reservation owner differs from withdrawal id",
                ));
            }
        }
        if self.payout_count > 1
            || self.refund_count > 1
            || (self.payout_count > 0 && self.refund_count > 0)
        {
            return Err(WithdrawalModelError::Invariant(
                "payout/refund cardinality is unsafe",
            ));
        }
        if self.confirmed && self.inclusion.is_none() {
            return Err(WithdrawalModelError::Invariant(
                "confirmation requires inclusion",
            ));
        }
        if self.submitted && self.active_transaction_id.is_none() {
            return Err(WithdrawalModelError::Invariant(
                "submission requires an active transaction",
            ));
        }
        if self
            .kernel_settled_nodes
            .iter()
            .any(|node| *node >= KERNEL_NODE_COUNT)
        {
            return Err(WithdrawalModelError::Invariant("invalid kernel node id"));
        }
        if !self.kernel_settled_nodes.is_empty() && !self.confirmed {
            return Err(WithdrawalModelError::Invariant(
                "kernel settlement requires confirmed inclusion",
            ));
        }
        if self
            .reservations
            .keys()
            .any(|input| !self.selected_inputs.contains(input))
        {
            return Err(WithdrawalModelError::Invariant(
                "reservation exists outside selected inputs",
            ));
        }
        if self
            .released_inputs
            .iter()
            .any(|input| self.reservations.contains_key(input))
        {
            return Err(WithdrawalModelError::Invariant(
                "released input remains reserved",
            ));
        }
        if let (Some(proposal), Some(active)) = (&self.proposal, &self.active_transaction_id) {
            if self.authorized_transactions.get(&proposal.epoch) != Some(active) {
                return Err(WithdrawalModelError::Invariant(
                    "active transaction is not the epoch authorization",
                ));
            }
        }
        if self.hold.is_some() && self.public_state != ModelPublicState::ReorgHold {
            return Err(WithdrawalModelError::Invariant(
                "reorg hold must be publicly visible",
            ));
        }
        match self.public_state {
            ModelPublicState::Ready => {
                let canonical = self
                    .proposal
                    .as_ref()
                    .is_some_and(|proposal| proposal.state == ModelProposalState::Canonicalized);
                if !canonical || self.reservations.len() != self.selected_inputs.len() {
                    return Err(WithdrawalModelError::Invariant(
                        "public ready state lacks canonical reserved proposal",
                    ));
                }
            }
            ModelPublicState::Submitted if !self.submitted => {
                return Err(WithdrawalModelError::Invariant(
                    "public submitted state lacks submission",
                ));
            }
            ModelPublicState::SequencerConfirmed if !self.confirmed => {
                return Err(WithdrawalModelError::Invariant(
                    "public confirmed state lacks confirmed inclusion",
                ));
            }
            _ => {}
        }
        if self.terminal {
            let burn_canonical = self
                .burn
                .as_ref()
                .is_some_and(|burn| burn.state == ModelBurnState::Canonical);
            let all_kernels = self.kernel_settled_nodes.len() == KERNEL_NODE_COUNT as usize;
            let all_released = self.released_inputs == self.selected_inputs
                && self.reservations.is_empty()
                && self.reservation_release_count == 1;
            if !burn_canonical
                || !self.confirmed
                || !all_kernels
                || !all_released
                || self.payout_count != 1
                || self.refund_count != 0
                || self.replay_required
                || self.hold.is_some()
                || self.public_state != ModelPublicState::Terminal
            {
                return Err(WithdrawalModelError::Invariant(
                    "terminal state lacks canonical burn/inclusion/settlement/release",
                ));
            }
        } else if self.public_state == ModelPublicState::Terminal {
            return Err(WithdrawalModelError::Invariant(
                "public terminal state lacks terminal proof",
            ));
        }
        Ok(())
    }

    pub fn state_sha256(&self) -> Result<String, WithdrawalModelError> {
        Ok(hex::encode(Sha256::digest(serde_json::to_vec(self)?)))
    }

    pub fn from_json(input: &str) -> Result<Self, WithdrawalModelError> {
        let state: Self = serde_json::from_str(input)?;
        state.validate()?;
        Ok(state)
    }

    fn apply_inner(
        &mut self,
        action: &WithdrawalModelAction,
    ) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        match action {
            WithdrawalModelAction::ObserveBurn {
                withdrawal_id,
                nonce,
            } => self.observe_burn(withdrawal_id, *nonce),
            WithdrawalModelAction::InvalidateBurn => self.invalidate_burn(),
            WithdrawalModelAction::ReadmitBurn => self.readmit_burn(),
            WithdrawalModelAction::RecoverHold { hold } => self.recover_hold(*hold),
            WithdrawalModelAction::Assemble {
                epoch,
                handoff,
                proposal_hash,
                selected_inputs,
            } => self.assemble(*epoch, *handoff, proposal_hash, selected_inputs),
            WithdrawalModelAction::Prepare => self.advance_proposal(
                ModelProposalState::Assembled,
                ModelProposalState::Prepared,
                WithdrawalModelEvent::ProposalPrepared,
            ),
            WithdrawalModelAction::Canonicalize => self.advance_proposal(
                ModelProposalState::Prepared,
                ModelProposalState::Canonicalized,
                WithdrawalModelEvent::ProposalCanonicalized,
            ),
            WithdrawalModelAction::AdvanceHandoff { handoff } => self.advance_handoff(*handoff),
            WithdrawalModelAction::Reserve { owner, inputs } => self.reserve(owner, inputs),
            WithdrawalModelAction::RestoreReservations => self.restore_reservations(),
            WithdrawalModelAction::Authorize {
                epoch,
                transaction_id,
            } => self.authorize(*epoch, transaction_id),
            WithdrawalModelAction::Submit { transaction_id } => self.submit(transaction_id),
            WithdrawalModelAction::Include {
                transaction_id,
                height,
                block_id,
            } => self.include(transaction_id, *height, block_id),
            WithdrawalModelAction::Confirm {
                transaction_id,
                height,
                block_id,
            } => self.confirm(transaction_id, *height, block_id),
            WithdrawalModelAction::SettleKernel { node_id } => self.settle_kernel(*node_id),
            WithdrawalModelAction::RecordPayout => self.record_payout(),
            WithdrawalModelAction::RecordRefund => self.record_refund(),
            WithdrawalModelAction::ReleaseReservations { owner, inputs } => {
                self.release(owner, inputs)
            }
            WithdrawalModelAction::Publish { state } => self.publish(*state),
            WithdrawalModelAction::Restart { component } => self.restart(component),
            WithdrawalModelAction::ReplayJournal { generation } => self.replay(*generation),
            WithdrawalModelAction::BaseReorg { deep } => self.base_reorg(*deep),
            WithdrawalModelAction::NockReorg {
                deep,
                reinclusion_height,
                reinclusion_block_id,
            } => self.nock_reorg(*deep, *reinclusion_height, reinclusion_block_id.as_deref()),
        }
    }

    fn observe_burn(
        &mut self,
        withdrawal_id: &str,
        nonce: u64,
    ) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        require_nonempty("withdrawal id", withdrawal_id, "observe_burn")?;
        match &self.burn {
            Some(existing)
                if existing.withdrawal_id == withdrawal_id
                    && existing.nonce == nonce
                    && existing.state == ModelBurnState::Canonical =>
            {
                Ok(Vec::new())
            }
            Some(_) => precondition("observe_burn", "another burn identity already exists"),
            None => {
                self.burn = Some(ModelBurnFacts {
                    withdrawal_id: withdrawal_id.to_owned(),
                    nonce,
                    state: ModelBurnState::Canonical,
                });
                Ok(vec![WithdrawalModelEvent::BurnObserved])
            }
        }
    }

    fn invalidate_burn(&mut self) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        if self.payout_count > 0 || self.terminal {
            return self.enter_hold(ModelHoldKind::DeepBaseReorg);
        }
        let burn = self
            .burn
            .as_mut()
            .ok_or_else(|| precondition_error("invalidate_burn", "burn is absent"))?;
        if burn.state == ModelBurnState::Orphaned {
            return Ok(Vec::new());
        }
        burn.state = ModelBurnState::Orphaned;
        self.clear_post_burn_progress();
        Ok(vec![WithdrawalModelEvent::BurnOrphaned])
    }

    fn readmit_burn(&mut self) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        if self.hold.is_some() {
            return precondition("readmit_burn", "deep reorg hold requires explicit recovery");
        }
        let burn = self
            .burn
            .as_mut()
            .ok_or_else(|| precondition_error("readmit_burn", "burn is absent"))?;
        match burn.state {
            ModelBurnState::Canonical => Ok(Vec::new()),
            ModelBurnState::Orphaned => {
                burn.state = ModelBurnState::Canonical;
                self.public_state = ModelPublicState::Pending;
                Ok(vec![WithdrawalModelEvent::BurnReadmitted])
            }
            ModelBurnState::Absent => precondition("readmit_burn", "burn identity is absent"),
        }
    }

    fn recover_hold(
        &mut self,
        expected: ModelHoldKind,
    ) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        if self.hold != Some(expected) {
            return precondition("recover_hold", "hold kind does not match");
        }
        if self.replay_required {
            return precondition("recover_hold", "journal replay must finish before recovery");
        }
        self.hold = None;
        self.public_state = if self.confirmed {
            ModelPublicState::SequencerConfirmed
        } else if self.submitted {
            ModelPublicState::Submitted
        } else {
            ModelPublicState::Pending
        };
        Ok(vec![WithdrawalModelEvent::HoldRecovered])
    }

    fn assemble(
        &mut self,
        epoch: u64,
        handoff: u64,
        proposal_hash: &str,
        selected_inputs: &BTreeSet<ModelNoteName>,
    ) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        self.require_canonical_burn("assemble")?;
        require_nonempty("proposal hash", proposal_hash, "assemble")?;
        if selected_inputs.is_empty() {
            return precondition("assemble", "selected inputs are empty");
        }
        if selected_inputs
            .iter()
            .any(|input| input.first.trim().is_empty() || input.last.trim().is_empty())
        {
            return precondition("assemble", "selected input identity is empty");
        }
        if let Some(proposal) = &self.proposal {
            if proposal.epoch == epoch
                && proposal.handoff == handoff
                && proposal.canonical_hash == proposal_hash
                && self.selected_inputs == *selected_inputs
            {
                return Ok(Vec::new());
            }
            if epoch <= proposal.epoch {
                return precondition("assemble", "replacement epoch must increase");
            }
            if self.submitted || self.confirmed || self.payout_count > 0 {
                return precondition("assemble", "submitted proposal cannot be replaced");
            }
        }
        self.proposal = Some(ModelProposalFacts {
            epoch,
            handoff,
            canonical_hash: proposal_hash.to_owned(),
            state: ModelProposalState::Assembled,
        });
        self.selected_inputs = selected_inputs.clone();
        self.reservations.clear();
        self.released_inputs.clear();
        self.reservation_release_count = 0;
        self.active_transaction_id = None;
        self.submitted = false;
        self.inclusion = None;
        self.confirmed = false;
        self.kernel_settled_nodes.clear();
        self.public_state = ModelPublicState::Pending;
        Ok(vec![WithdrawalModelEvent::ProposalAssembled])
    }

    fn advance_proposal(
        &mut self,
        expected: ModelProposalState,
        next: ModelProposalState,
        event: WithdrawalModelEvent,
    ) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        let proposal = self
            .proposal
            .as_mut()
            .ok_or_else(|| precondition_error("advance_proposal", "proposal is absent"))?;
        if proposal.state == next {
            return Ok(Vec::new());
        }
        if proposal.state != expected {
            return precondition("advance_proposal", "proposal state is out of order");
        }
        proposal.state = next;
        Ok(vec![event])
    }

    fn advance_handoff(
        &mut self,
        handoff: u64,
    ) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        let proposal = self
            .proposal
            .as_mut()
            .ok_or_else(|| precondition_error("advance_handoff", "proposal is absent"))?;
        if handoff == proposal.handoff {
            return Ok(Vec::new());
        }
        if handoff <= proposal.handoff || self.submitted {
            return precondition("advance_handoff", "handoff must increase before submission");
        }
        proposal.handoff = handoff;
        Ok(vec![WithdrawalModelEvent::HandoffAdvanced])
    }

    fn reserve(
        &mut self,
        owner: &str,
        inputs: &BTreeSet<ModelNoteName>,
    ) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        require_nonempty("reservation owner", owner, "reserve")?;
        if self.withdrawal_id("reserve")? != owner {
            return precondition("reserve", "reservation owner is not the withdrawal id");
        }
        if inputs.is_empty() || !inputs.is_subset(&self.selected_inputs) {
            return precondition("reserve", "inputs are empty or not selected");
        }
        for input in inputs {
            if let Some(existing) = self.reservations.get(input) {
                if existing != owner {
                    return precondition("reserve", "input already has another active owner");
                }
            }
            if self.released_inputs.contains(input) {
                return precondition("reserve", "released input cannot be reserved again");
            }
        }
        let before = self.reservations.len();
        for input in inputs {
            self.reservations.insert(input.clone(), owner.to_owned());
        }
        Ok((self.reservations.len() != before)
            .then_some(WithdrawalModelEvent::InputsReserved)
            .into_iter()
            .collect())
    }

    fn restore_reservations(&mut self) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        if !self.replay_required {
            return precondition("restore_reservations", "journal replay is not required");
        }
        let owner = self.withdrawal_id("restore_reservations")?.to_owned();
        if !self.reservations.is_empty() {
            return Ok(Vec::new());
        }
        for input in &self.selected_inputs {
            if !self.released_inputs.contains(input) {
                self.reservations.insert(input.clone(), owner.clone());
            }
        }
        Ok(vec![WithdrawalModelEvent::ReservationsRestored])
    }

    fn authorize(
        &mut self,
        epoch: u64,
        transaction_id: &str,
    ) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        require_nonempty("transaction id", transaction_id, "authorize")?;
        let proposal = self
            .proposal
            .as_ref()
            .ok_or_else(|| precondition_error("authorize", "proposal is absent"))?;
        if proposal.epoch != epoch || proposal.state != ModelProposalState::Canonicalized {
            return precondition("authorize", "proposal is not canonicalized for epoch");
        }
        if self.reservations.len() != self.selected_inputs.len() {
            return precondition("authorize", "selected inputs are not fully reserved");
        }
        if let Some(existing) = self.authorized_transactions.get(&epoch) {
            if existing == transaction_id {
                return Ok(Vec::new());
            }
            return precondition(
                "authorize", "epoch already authorizes another raw transaction",
            );
        }
        self.authorized_transactions
            .insert(epoch, transaction_id.to_owned());
        self.active_transaction_id = Some(transaction_id.to_owned());
        Ok(vec![WithdrawalModelEvent::TransactionAuthorized])
    }

    fn submit(
        &mut self,
        transaction_id: &str,
    ) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        self.require_active_transaction("submit", transaction_id)?;
        if self.replay_required || self.hold.is_some() {
            return precondition("submit", "replay or hold blocks submission");
        }
        if self.submitted {
            return Ok(Vec::new());
        }
        self.submitted = true;
        Ok(vec![WithdrawalModelEvent::TransactionSubmitted])
    }

    fn include(
        &mut self,
        transaction_id: &str,
        height: u64,
        block_id: &str,
    ) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        self.require_active_transaction("include", transaction_id)?;
        require_nonempty("block id", block_id, "include")?;
        if !self.submitted || height == 0 {
            return precondition("include", "transaction is not submitted or height is zero");
        }
        let next = ModelInclusionFacts {
            transaction_id: transaction_id.to_owned(),
            height,
            block_id: block_id.to_owned(),
        };
        if self.inclusion.as_ref() == Some(&next) {
            return Ok(Vec::new());
        }
        if self.confirmed || !self.kernel_settled_nodes.is_empty() || self.payout_count > 0 {
            return precondition(
                "include", "confirmed inclusion cannot move without reorg action",
            );
        }
        self.inclusion = Some(next);
        Ok(vec![WithdrawalModelEvent::TransactionIncluded])
    }

    fn confirm(
        &mut self,
        transaction_id: &str,
        height: u64,
        block_id: &str,
    ) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        self.require_active_transaction("confirm", transaction_id)?;
        let expected = ModelInclusionFacts {
            transaction_id: transaction_id.to_owned(),
            height,
            block_id: block_id.to_owned(),
        };
        if self.inclusion.as_ref() != Some(&expected) {
            return precondition("confirm", "confirmation does not match inclusion");
        }
        if self.confirmed {
            return Ok(Vec::new());
        }
        self.confirmed = true;
        Ok(vec![WithdrawalModelEvent::TransactionConfirmed])
    }

    fn settle_kernel(
        &mut self,
        node_id: u64,
    ) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        if !self.confirmed || node_id >= KERNEL_NODE_COUNT || self.hold.is_some() {
            return precondition(
                "settle_kernel", "confirmation/node/hold precondition failed",
            );
        }
        Ok(self
            .kernel_settled_nodes
            .insert(node_id)
            .then_some(WithdrawalModelEvent::KernelSettled)
            .into_iter()
            .collect())
    }

    fn record_payout(&mut self) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        if self.kernel_settled_nodes.len() != KERNEL_NODE_COUNT as usize || self.refund_count > 0 {
            return precondition("record_payout", "all kernels must settle without refund");
        }
        if self.payout_count == 1 {
            return Ok(Vec::new());
        }
        self.payout_count = self
            .payout_count
            .checked_add(1)
            .ok_or_else(|| precondition_error("record_payout", "payout counter overflow"))?;
        Ok(vec![WithdrawalModelEvent::PayoutRecorded])
    }

    fn record_refund(&mut self) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        let orphaned = self
            .burn
            .as_ref()
            .is_some_and(|burn| burn.state == ModelBurnState::Orphaned);
        if !orphaned || self.payout_count > 0 {
            return precondition("record_refund", "refund requires orphaned unpaid burn");
        }
        if self.refund_count == 1 {
            return Ok(Vec::new());
        }
        self.refund_count = self
            .refund_count
            .checked_add(1)
            .ok_or_else(|| precondition_error("record_refund", "refund counter overflow"))?;
        Ok(vec![WithdrawalModelEvent::RefundRecorded])
    }

    fn release(
        &mut self,
        owner: &str,
        inputs: &BTreeSet<ModelNoteName>,
    ) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        if self.payout_count != 1 || inputs != &self.selected_inputs {
            return precondition(
                "release_reservations", "payout and exact selected inputs required",
            );
        }
        if self.released_inputs == *inputs && self.reservations.is_empty() {
            return Ok(Vec::new());
        }
        for input in inputs {
            if self.reservations.get(input).map(String::as_str) != Some(owner) {
                return precondition("release_reservations", "reservation owner mismatch");
            }
        }
        for input in inputs {
            self.reservations.remove(input);
            self.released_inputs.insert(input.clone());
        }
        self.reservation_release_count =
            self.reservation_release_count
                .checked_add(1)
                .ok_or_else(|| {
                    precondition_error("release_reservations", "release counter overflow")
                })?;
        Ok(vec![WithdrawalModelEvent::ReservationsReleased])
    }

    fn publish(
        &mut self,
        state: ModelPublicState,
    ) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        if state == self.public_state {
            return Ok(Vec::new());
        }
        if self.public_state == ModelPublicState::ReorgHold && self.hold.is_some() {
            return precondition("publish", "active reorg hold cannot regress");
        }
        if state == ModelPublicState::Terminal {
            let burn_canonical = self
                .burn
                .as_ref()
                .is_some_and(|burn| burn.state == ModelBurnState::Canonical);
            let ready = burn_canonical
                && self.confirmed
                && self.kernel_settled_nodes.len() == KERNEL_NODE_COUNT as usize
                && self.payout_count == 1
                && self.refund_count == 0
                && self.reservations.is_empty()
                && self.released_inputs == self.selected_inputs
                && self.reservation_release_count == 1
                && !self.replay_required
                && self.hold.is_none();
            if !ready {
                return precondition("publish", "terminal proof preconditions are incomplete");
            }
            self.public_state = state;
            self.terminal = true;
            return Ok(vec![WithdrawalModelEvent::PublicStateAdvanced]);
        }
        let stage_ready = match state {
            ModelPublicState::Pending => self.burn.is_some(),
            ModelPublicState::Ready => {
                self.proposal
                    .as_ref()
                    .is_some_and(|proposal| proposal.state == ModelProposalState::Canonicalized)
                    && self.reservations.len() == self.selected_inputs.len()
            }
            ModelPublicState::Submitted => self.submitted,
            ModelPublicState::SequencerConfirmed => self.confirmed,
            ModelPublicState::Failed => true,
            ModelPublicState::ReorgHold | ModelPublicState::Terminal => true,
        };
        if !stage_ready {
            return precondition("publish", "public state preconditions are incomplete");
        }
        if state == ModelPublicState::ReorgHold {
            if self.hold.is_none() {
                return precondition("publish", "reorg hold has no model hold fact");
            }
        } else if state < self.public_state {
            return precondition("publish", "public state regression is forbidden");
        }
        self.public_state = state;
        Ok(vec![WithdrawalModelEvent::PublicStateAdvanced])
    }

    fn restart(
        &mut self,
        component: &str,
    ) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        require_nonempty("component", component, "restart")?;
        if component == "sequencer" {
            self.journal_generation = self
                .journal_generation
                .checked_add(1)
                .ok_or_else(|| precondition_error("restart", "journal generation overflow"))?;
            if !self.terminal {
                self.replay_required = true;
                self.reservations.clear();
                self.public_state = ModelPublicState::Pending;
            }
        }
        Ok(vec![WithdrawalModelEvent::ComponentRestarted])
    }

    fn replay(
        &mut self,
        generation: u64,
    ) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        if !self.replay_required || generation != self.journal_generation {
            return precondition(
                "replay_journal", "journal generation is stale or replay is unnecessary",
            );
        }
        self.replay_required = false;
        Ok(vec![WithdrawalModelEvent::JournalReplayed])
    }

    fn base_reorg(
        &mut self,
        deep: bool,
    ) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        if deep || self.payout_count > 0 || self.terminal {
            self.enter_hold(ModelHoldKind::DeepBaseReorg)
        } else {
            self.invalidate_burn()
        }
    }

    fn nock_reorg(
        &mut self,
        deep: bool,
        reinclusion_height: Option<u64>,
        reinclusion_block_id: Option<&str>,
    ) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        if deep || self.payout_count > 0 || self.terminal {
            return self.enter_hold(ModelHoldKind::DeepNockReorg);
        }
        if !self.confirmed {
            return precondition("nock_reorg", "transaction is not confirmed");
        }
        let active = self
            .active_transaction_id
            .clone()
            .ok_or_else(|| precondition_error("nock_reorg", "active transaction is absent"))?;
        self.confirmed = false;
        self.kernel_settled_nodes.clear();
        self.public_state = ModelPublicState::Submitted;
        self.inclusion = match (reinclusion_height, reinclusion_block_id) {
            (Some(height), Some(block_id)) if height > 0 && !block_id.is_empty() => {
                Some(ModelInclusionFacts {
                    transaction_id: active,
                    height,
                    block_id: block_id.to_owned(),
                })
            }
            (None, None) => None,
            _ => return precondition("nock_reorg", "reinclusion height and block must pair"),
        };
        Ok(vec![WithdrawalModelEvent::NockReorgApplied])
    }

    fn enter_hold(
        &mut self,
        hold: ModelHoldKind,
    ) -> Result<Vec<WithdrawalModelEvent>, WithdrawalModelError> {
        if self.hold == Some(hold) {
            return Ok(Vec::new());
        }
        self.hold = Some(hold);
        self.terminal = false;
        self.public_state = ModelPublicState::ReorgHold;
        Ok(vec![WithdrawalModelEvent::ReorgHoldEntered])
    }

    fn clear_post_burn_progress(&mut self) {
        self.proposal = None;
        self.active_transaction_id = None;
        self.submitted = false;
        self.inclusion = None;
        self.confirmed = false;
        self.kernel_settled_nodes.clear();
        self.reservations.clear();
        self.selected_inputs.clear();
        self.released_inputs.clear();
        self.reservation_release_count = 0;
        self.public_state = ModelPublicState::Pending;
        self.terminal = false;
    }

    fn require_canonical_burn(&self, action: &'static str) -> Result<(), WithdrawalModelError> {
        if self
            .burn
            .as_ref()
            .is_some_and(|burn| burn.state == ModelBurnState::Canonical)
            && self.hold.is_none()
        {
            Ok(())
        } else {
            precondition(action, "canonical burn without hold is required")
        }
    }

    fn require_active_transaction(
        &self,
        action: &'static str,
        transaction_id: &str,
    ) -> Result<(), WithdrawalModelError> {
        if self.active_transaction_id.as_deref() == Some(transaction_id) {
            Ok(())
        } else {
            precondition(action, "transaction is not the authorized active raw tx")
        }
    }

    fn withdrawal_id(&self, action: &'static str) -> Result<&str, WithdrawalModelError> {
        self.burn
            .as_ref()
            .map(|burn| burn.withdrawal_id.as_str())
            .ok_or_else(|| precondition_error(action, "withdrawal identity is absent"))
    }
}

mod reservation_map_serde {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::ModelNoteName;

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ReservationEntry {
        input: ModelNoteName,
        owner: String,
    }

    pub fn serialize<S>(
        reservations: &BTreeMap<ModelNoteName, String>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        reservations
            .iter()
            .map(|(input, owner)| ReservationEntry {
                input: input.clone(),
                owner: owner.clone(),
            })
            .collect::<Vec<_>>()
            .serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<BTreeMap<ModelNoteName, String>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let entries = Vec::<ReservationEntry>::deserialize(deserializer)?;
        let mut reservations = BTreeMap::new();
        for entry in entries {
            if reservations.insert(entry.input, entry.owner).is_some() {
                return Err(serde::de::Error::custom(
                    "duplicate reservation input in model state",
                ));
            }
        }
        Ok(reservations)
    }
}
fn require_nonempty(
    field: &'static str,
    value: &str,
    action: &'static str,
) -> Result<(), WithdrawalModelError> {
    if value.trim().is_empty() {
        precondition(action, field)
    } else {
        Ok(())
    }
}

fn precondition<T>(action: &'static str, reason: &'static str) -> Result<T, WithdrawalModelError> {
    Err(precondition_error(action, reason))
}

fn precondition_error(action: &'static str, reason: &'static str) -> WithdrawalModelError {
    WithdrawalModelError::Precondition { action, reason }
}

#[derive(Debug, Error)]
pub enum WithdrawalModelError {
    #[error("withdrawal model action {action} failed precondition: {reason}")]
    Precondition {
        action: &'static str,
        reason: &'static str,
    },
    #[error("withdrawal model invariant failed: {0}")]
    Invariant(&'static str),
    #[error("unsupported withdrawal model schema version {0}")]
    UnsupportedSchema(u64),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
