use crate::shared::errors::BridgeError;
use crate::shared::proposer::withdrawal_turn_proposer;
use crate::withdrawal::state::{LiveWithdrawalView, WithdrawalState};
use crate::withdrawal::types::WithdrawalId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawalSubmissionCandidateKind {
    AuthorizePeerCanonical,
    SubmitAuthorized,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithdrawalSubmissionCandidate {
    pub id: WithdrawalId,
    pub current_epoch: u64,
    pub kind: WithdrawalSubmissionCandidateKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawalAuthorizationStatusDecision {
    Evaluate,
    SkipAlreadyAdvanced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawalSubmissionStatusDecision {
    RequireAuthorizedMatch,
    SkipAlreadyAdvanced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WithdrawalSubmissionRowDecision {
    AwaitingOtherProposer,
    BlockedFrontierState,
    SkipReleasedNonce,
    Candidate(WithdrawalSubmissionCandidateKind),
}

pub fn select_frontier_authorize_or_submit_candidate(
    row: &LiveWithdrawalView,
    handoff_index: u64,
    local_node_id: u64,
    node_pkhs: &[nockchain_types::tx_engine::common::Hash],
) -> Result<Option<WithdrawalSubmissionCandidate>, BridgeError> {
    match submission_row_decision(row, handoff_index, local_node_id, node_pkhs)? {
        WithdrawalSubmissionRowDecision::SkipReleasedNonce
        | WithdrawalSubmissionRowDecision::AwaitingOtherProposer
        | WithdrawalSubmissionRowDecision::BlockedFrontierState => Ok(None),
        WithdrawalSubmissionRowDecision::Candidate(kind) => {
            Ok(Some(WithdrawalSubmissionCandidate {
                id: row.id.clone(),
                current_epoch: row.current_epoch,
                kind,
            }))
        }
    }
}

fn submission_row_decision(
    row: &LiveWithdrawalView,
    handoff_index: u64,
    local_node_id: u64,
    node_pkhs: &[nockchain_types::tx_engine::common::Hash],
) -> Result<WithdrawalSubmissionRowDecision, BridgeError> {
    match row.state {
        WithdrawalState::MempoolAccepted | WithdrawalState::Confirmed => {
            Ok(WithdrawalSubmissionRowDecision::SkipReleasedNonce)
        }
        WithdrawalState::PeerCanonical => {
            if withdrawal_turn_proposer(&row.id, row.current_epoch, handoff_index, node_pkhs)
                != local_node_id as usize
            {
                return Ok(WithdrawalSubmissionRowDecision::AwaitingOtherProposer);
            }
            Ok(WithdrawalSubmissionRowDecision::Candidate(
                WithdrawalSubmissionCandidateKind::AuthorizePeerCanonical,
            ))
        }
        WithdrawalState::Authorized => {
            if withdrawal_turn_proposer(&row.id, row.current_epoch, handoff_index, node_pkhs)
                != local_node_id as usize
            {
                return Ok(WithdrawalSubmissionRowDecision::AwaitingOtherProposer);
            }
            Ok(WithdrawalSubmissionRowDecision::Candidate(
                WithdrawalSubmissionCandidateKind::SubmitAuthorized,
            ))
        }
        WithdrawalState::Assembling | WithdrawalState::Prepared => {
            Ok(WithdrawalSubmissionRowDecision::BlockedFrontierState)
        }
        WithdrawalState::Pending => Ok(WithdrawalSubmissionRowDecision::SkipReleasedNonce),
    }
}

pub fn plan_authorization_status(
    found: bool,
    state: &str,
) -> WithdrawalAuthorizationStatusDecision {
    if found && matches!(state, "authorized" | "mempool_accepted" | "confirmed") {
        WithdrawalAuthorizationStatusDecision::SkipAlreadyAdvanced
    } else {
        WithdrawalAuthorizationStatusDecision::Evaluate
    }
}

pub fn plan_submission_status(found: bool, state: &str) -> WithdrawalSubmissionStatusDecision {
    if found && matches!(state, "mempool_accepted" | "confirmed") {
        WithdrawalSubmissionStatusDecision::SkipAlreadyAdvanced
    } else {
        WithdrawalSubmissionStatusDecision::RequireAuthorizedMatch
    }
}

#[cfg(test)]
mod tests {
    use nockchain_math::belt::Belt;

    use super::*;
    use crate::shared::proposer::withdrawal_active_proposer;
    use crate::shared::types::{zero_tip5_hash, Tip5Hash};

    fn sample_id(seed: u64) -> WithdrawalId {
        WithdrawalId {
            as_of: zero_tip5_hash(),
            base_event_id: crate::shared::types::BaseEventId(vec![seed as u8; 32]),
        }
    }

    fn sample_row(seed: u64, nonce: u64, epoch: u64, state: WithdrawalState) -> LiveWithdrawalView {
        LiveWithdrawalView {
            id: sample_id(seed),
            recipient: Some(zero_tip5_hash()),
            gross_burned_amount: Some(1_000),
            base_batch_end: Some(100),
            withdrawal_nonce: Some(nonce),
            current_epoch: epoch,
            proposal_hash: None,
            peer_commit_certificate: None,

            authorized_transaction_name: None,
            handoff_index: 0,
            turn_started_base_height: None,
            submit_attempt_count: 0,
            last_submit_attempt_base_height: None,
            last_submit_error: None,
            state,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn sample_node_pkhs() -> Vec<nockchain_types::tx_engine::common::Hash> {
        vec![
            Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            Tip5Hash([Belt(6), Belt(7), Belt(8), Belt(9), Belt(10)]),
        ]
    }

    #[test]
    fn prepared_frontier_has_no_candidate() {
        let candidate = select_frontier_authorize_or_submit_candidate(
            &sample_row(1, 1, 0, WithdrawalState::Prepared),
            0,
            0,
            &sample_node_pkhs(),
        )
        .expect("select candidate");
        assert!(candidate.is_none());
    }

    #[test]
    fn wrong_proposer_frontier_awaits_other_proposer() {
        let node_pkhs = sample_node_pkhs();
        let row = sample_row(2, 2, 0, WithdrawalState::PeerCanonical);
        let active_proposer = withdrawal_active_proposer(&row.id, row.current_epoch, &node_pkhs);
        let local_node_id = if active_proposer == 0 { 1 } else { 0 };
        assert_eq!(
            submission_row_decision(&row, 0, local_node_id as u64, &node_pkhs)
                .expect("plan frontier row"),
            WithdrawalSubmissionRowDecision::AwaitingOtherProposer
        );
    }

    #[test]
    fn handoff_index_rotates_frontier_proposer() {
        let node_pkhs = sample_node_pkhs();
        let row = sample_row(2, 2, 0, WithdrawalState::PeerCanonical);
        let turn0 = withdrawal_turn_proposer(&row.id, row.current_epoch, 0, &node_pkhs);
        let turn1 = withdrawal_turn_proposer(&row.id, row.current_epoch, 1, &node_pkhs);
        assert_ne!(turn0, turn1);
        assert_eq!(
            submission_row_decision(&row, 1, turn1 as u64, &node_pkhs).expect("plan frontier row"),
            WithdrawalSubmissionRowDecision::Candidate(
                WithdrawalSubmissionCandidateKind::AuthorizePeerCanonical,
            )
        );
    }

    #[test]
    fn prepared_frontier_is_blocked_frontier_state() {
        assert_eq!(
            submission_row_decision(
                &sample_row(3, 3, 0, WithdrawalState::Prepared),
                0,
                0,
                &sample_node_pkhs(),
            )
            .expect("plan frontier row"),
            WithdrawalSubmissionRowDecision::BlockedFrontierState
        );
    }

    #[test]
    fn authorized_frontier_yields_submit_candidate() {
        let node_pkhs = sample_node_pkhs();
        let row = sample_row(2, 2, 0, WithdrawalState::Authorized);
        let local_node_id =
            withdrawal_active_proposer(&row.id, row.current_epoch, &node_pkhs) as u64;
        let candidate =
            select_frontier_authorize_or_submit_candidate(&row, 0, local_node_id, &node_pkhs)
                .expect("select candidate")
                .expect("authorized frontier should yield submit candidate");
        assert_eq!(candidate.id, row.id);
        assert_eq!(
            candidate.kind,
            WithdrawalSubmissionCandidateKind::SubmitAuthorized
        );
    }

    #[test]
    fn authorization_status_skips_advanced_states() {
        assert_eq!(
            plan_authorization_status(true, "mempool_accepted"),
            WithdrawalAuthorizationStatusDecision::SkipAlreadyAdvanced
        );
    }

    #[test]
    fn submission_status_requires_match_for_authorized() {
        assert_eq!(
            plan_submission_status(true, "authorized"),
            WithdrawalSubmissionStatusDecision::RequireAuthorizedMatch
        );
    }
}
