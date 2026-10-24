use async_trait::async_trait;
use tonic::Request;

use crate::shared::errors::BridgeError;
use crate::shared::ingress::proto::withdrawal_sequencer_client::WithdrawalSequencerClient;
use crate::shared::ingress::proto::{
    CurrentLiveWithdrawalNonceRequest, CurrentLiveWithdrawalNonceResponse,
    NextPendingWithdrawalOrderingRequest, NextPendingWithdrawalOrderingResponse,
    SequencedWithdrawalStatusRequest, SequencedWithdrawalStatusResponse,
    SequencerAdvancePrecanonicalHandoffRequest, SequencerAuthorizeProposalRequest,
    SequencerCanonicalProposalArtifactsRequest, SequencerCanonicalProposalArtifactsResponse,
    SequencerFrontierAllowsWithdrawalRequest, SequencerFrontierAllowsWithdrawalResponse,
    SequencerRecordCanonicalRequest, SequencerRecordSignedProposalRequest,
    SequencerRegisterWithdrawalRequest, SequencerReservedWithdrawalInputsRequest,
    SequencerSubmitProposalRequest, WithdrawalCommitCertificate,
};
use crate::withdrawal::proposals::TrackedWithdrawalRequest;
use crate::withdrawal::sequencer::store::cue_transaction;
use crate::withdrawal::submission::{
    is_withdrawal_submit_deferred_error, NextPendingWithdrawalOrdering,
    WithdrawalSequencerCanonicalizationError, WithdrawalSequencerPort,
    WithdrawalSequencerSubmitOutcome,
};
use crate::withdrawal::transport::{
    note_name_from_proto, proposal_to_proto, snapshot_from_proto, withdrawal_id_from_proto,
    withdrawal_id_to_proto,
};
use crate::withdrawal::types::{
    WithdrawalId, WithdrawalProposalData, WithdrawalSequencerProposalArtifacts,
};

#[derive(Clone, Debug)]
pub struct GrpcWithdrawalSequencerClient {
    endpoint: String,
}

impl GrpcWithdrawalSequencerClient {
    /// Builds a gRPC sequencer client pointed at the API-node-hosted sequencer
    /// service.
    pub fn new(endpoint: String) -> Self {
        Self { endpoint }
    }

    /// Opens a fresh gRPC connection to the withdrawal sequencer.
    async fn connect(
        &self,
    ) -> Result<WithdrawalSequencerClient<tonic::transport::Channel>, BridgeError> {
        WithdrawalSequencerClient::connect(self.endpoint.clone())
            .await
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to connect withdrawal sequencer client at {}: {err}",
                    self.endpoint
                ))
            })
    }

    /// Wraps a typed proposal together with its derived withdrawal nonce for
    /// sequencer RPC submission.
    fn proposal_envelope(
        proposal: &WithdrawalProposalData,
        withdrawal_nonce: u64,
    ) -> Result<crate::shared::ingress::proto::WithdrawalProposalEnvelope, BridgeError> {
        let mut envelope = proposal_to_proto(proposal)?;
        envelope.withdrawal_nonce = withdrawal_nonce;
        Ok(envelope)
    }
}

#[async_trait]
impl WithdrawalSequencerPort for GrpcWithdrawalSequencerClient {
    async fn register_withdrawal(
        &self,
        tracked: &TrackedWithdrawalRequest,
    ) -> Result<(), BridgeError> {
        let mut client = self.connect().await?;
        let response = client
            .register_withdrawal(Request::new(SequencerRegisterWithdrawalRequest {
                withdrawal_nonce: tracked.withdrawal_nonce,
                recipient: tracked.recipient.to_be_limb_bytes().to_vec(),
                burned_amount: tracked.amount,
                base_batch_end: tracked.base_batch_end,
                base_event_id: tracked.id.base_event_id.0.clone(),
            }))
            .await
            .map(|response| response.into_inner())
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to register withdrawal {:?} nonce {} with sequencer: {err}",
                    tracked.id, tracked.withdrawal_nonce
                ))
            })?;
        if response.request_accepted {
            Ok(())
        } else {
            Err(BridgeError::Runtime(format!(
                "sequencer rejected withdrawal {:?} nonce {} registration: {}",
                tracked.id, tracked.withdrawal_nonce, response.error
            )))
        }
    }

    async fn advance_precanonical_handoff(
        &self,
        id: &WithdrawalId,
        epoch: u64,
        next_handoff_index: u64,
        turn_started_base_height: u64,
    ) -> Result<(), BridgeError> {
        let mut client = self.connect().await?;
        let response = client
            .advance_precanonical_handoff(Request::new(
                SequencerAdvancePrecanonicalHandoffRequest {
                    withdrawal_id: Some(withdrawal_id_to_proto(id)),
                    epoch,
                    next_handoff_index,
                    turn_started_base_height,
                },
            ))
            .await
            .map(|response| response.into_inner())
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to advance pre-canonical handoff for withdrawal {:?} epoch {} at sequencer: {err}",
                    id, epoch
                ))
            })?;
        if response.request_accepted {
            Ok(())
        } else {
            Err(BridgeError::Runtime(format!(
                "sequencer rejected pre-canonical handoff for withdrawal {:?} epoch {}: {}",
                id, epoch, response.error
            )))
        }
    }

    async fn record_peer_canonical_proposal(
        &self,
        proposal: &WithdrawalProposalData,
        withdrawal_nonce: u64,
        commit_certificate: &WithdrawalCommitCertificate,
        caller_node_id: u64,
    ) -> Result<(), WithdrawalSequencerCanonicalizationError> {
        let mut client = self.connect().await?;
        let response = client
            .record_canonical_proposal(Request::new(SequencerRecordCanonicalRequest {
                proposal: Some(Self::proposal_envelope(proposal, withdrawal_nonce)?),
                commit_certificate: Some(commit_certificate.clone()),
                caller_node_id,
            }))
            .await
            .map(|response| response.into_inner())
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to record canonical withdrawal {:?} epoch {} at sequencer: {err}",
                    proposal.id, proposal.epoch
                ))
            })?;
        if response.request_accepted {
            Ok(())
        } else {
            Err(WithdrawalSequencerCanonicalizationError::Rejected(
                response.error,
            ))
        }
    }

    async fn record_signed_proposal(
        &self,
        proposal: &WithdrawalProposalData,
        withdrawal_nonce: u64,
        signer_node_id: u64,
    ) -> Result<(), BridgeError> {
        let mut client = self.connect().await?;
        let response = client
            .record_signed_proposal(Request::new(SequencerRecordSignedProposalRequest {
                proposal: Some(Self::proposal_envelope(proposal, withdrawal_nonce)?),
                signer_node_id,
            }))
            .await
            .map(|response| response.into_inner())
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to record signed withdrawal {:?} epoch {} at sequencer: {err}",
                    proposal.id, proposal.epoch
                ))
            })?;
        if response.request_accepted {
            Ok(())
        } else {
            Err(BridgeError::Runtime(format!(
                "sequencer rejected signed withdrawal {:?} epoch {}: {}",
                proposal.id, proposal.epoch, response.error
            )))
        }
    }

    async fn get_reserved_withdrawal_inputs(
        &self,
    ) -> Result<Vec<nockchain_types::v1::Name>, BridgeError> {
        let mut client = self.connect().await?;
        let response = client
            .get_reserved_withdrawal_inputs(Request::new(
                SequencerReservedWithdrawalInputsRequest {},
            ))
            .await
            .map(|response| response.into_inner())
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to load reserved withdrawal inputs from sequencer: {err}"
                ))
            })?;
        response
            .reserved_inputs
            .iter()
            .map(note_name_from_proto)
            .collect()
    }

    async fn authorize_proposal(
        &self,
        proposal: &WithdrawalProposalData,
        withdrawal_nonce: u64,
        commit_certificate: &WithdrawalCommitCertificate,
        caller_node_id: u64,
    ) -> Result<(), BridgeError> {
        let mut client = self.connect().await?;
        let response = client
            .authorize_proposal(Request::new(SequencerAuthorizeProposalRequest {
                proposal: Some(Self::proposal_envelope(proposal, withdrawal_nonce)?),
                commit_certificate: Some(commit_certificate.clone()),
                caller_node_id,
            }))
            .await
            .map(|response| response.into_inner())
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to authorize withdrawal {:?} epoch {} at sequencer: {err}",
                    proposal.id, proposal.epoch
                ))
            })?;
        if response.request_accepted {
            Ok(())
        } else {
            Err(BridgeError::Runtime(format!(
                "sequencer rejected withdrawal {:?} epoch {} authorization: {}",
                proposal.id, proposal.epoch, response.error
            )))
        }
    }

    async fn submit_proposal(
        &self,
        proposal: &WithdrawalProposalData,
        withdrawal_nonce: u64,
        caller_node_id: u64,
    ) -> Result<Option<WithdrawalSequencerSubmitOutcome>, BridgeError> {
        let mut client = self.connect().await?;
        let response = client
            .submit_proposal(Request::new(SequencerSubmitProposalRequest {
                proposal: Some(Self::proposal_envelope(proposal, withdrawal_nonce)?),
                caller_node_id,
            }))
            .await
            .map(|response| response.into_inner())
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to submit withdrawal {:?} epoch {} through sequencer: {err}",
                    proposal.id, proposal.epoch
                ))
            })?;
        if !response.request_accepted {
            if is_withdrawal_submit_deferred_error(&response.error) {
                return Ok(None);
            }
            return Err(BridgeError::Runtime(format!(
                "sequencer rejected withdrawal {:?} epoch {} submission: {}",
                proposal.id, proposal.epoch, response.error
            )));
        }
        let status = self.get_sequenced_withdrawal_status(&proposal.id).await?;
        if !status.found || status.current_epoch != proposal.epoch {
            return Err(BridgeError::Runtime(format!(
                "sequencer returned no final submission status for withdrawal {:?} epoch {}",
                proposal.id, proposal.epoch
            )));
        }
        match status.state.as_str() {
            "mempool_accepted" | "confirmed" => {
                Ok(Some(WithdrawalSequencerSubmitOutcome::MempoolAccepted))
            }
            other => Err(BridgeError::Runtime(format!(
                "unexpected sequencer submission state for withdrawal {:?} epoch {}: {other}",
                proposal.id, proposal.epoch
            ))),
        }
    }

    async fn get_next_pending_withdrawal_ordering(
        &self,
    ) -> Result<Option<NextPendingWithdrawalOrdering>, BridgeError> {
        let mut client = self.connect().await?;
        let response: NextPendingWithdrawalOrderingResponse = client
            .get_next_pending_withdrawal_ordering(Request::new(
                NextPendingWithdrawalOrderingRequest {},
            ))
            .await
            .map(|response| response.into_inner())
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to query next pending withdrawal ordering from sequencer: {err}"
                ))
            })?;
        if !response.found {
            return Ok(None);
        }
        let id = response
            .withdrawal_id
            .as_ref()
            .ok_or_else(|| {
                BridgeError::Runtime(
                    "sequencer next pending withdrawal ordering response was missing withdrawal id"
                        .into(),
                )
            })
            .and_then(crate::withdrawal::transport::withdrawal_id_from_proto)?;
        Ok(Some(NextPendingWithdrawalOrdering {
            id,
            withdrawal_nonce: response.withdrawal_nonce,
        }))
    }

    async fn current_live_withdrawal_nonce(&self) -> Result<Option<u64>, BridgeError> {
        let mut client = self.connect().await?;
        let response: CurrentLiveWithdrawalNonceResponse = client
            .get_current_live_withdrawal_nonce(Request::new(CurrentLiveWithdrawalNonceRequest {}))
            .await
            .map(|response| response.into_inner())
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to query current live withdrawal nonce from sequencer: {err}"
                ))
            })?;
        if response.found {
            Ok(Some(response.withdrawal_nonce))
        } else {
            Ok(None)
        }
    }

    async fn frontier_allows_withdrawal(&self, id: &WithdrawalId) -> Result<bool, BridgeError> {
        let mut client = self.connect().await?;
        let response: SequencerFrontierAllowsWithdrawalResponse = client
            .frontier_allows_withdrawal(Request::new(SequencerFrontierAllowsWithdrawalRequest {
                withdrawal_id: Some(withdrawal_id_to_proto(id)),
            }))
            .await
            .map(|response| response.into_inner())
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to query withdrawal frontier permission for {:?}: {err}",
                    id
                ))
            })?;
        Ok(response.allowed)
    }

    async fn get_sequenced_withdrawal_status(
        &self,
        id: &WithdrawalId,
    ) -> Result<SequencedWithdrawalStatusResponse, BridgeError> {
        let mut client = self.connect().await?;
        client
            .get_sequenced_withdrawal_status(Request::new(SequencedWithdrawalStatusRequest {
                withdrawal_id: Some(withdrawal_id_to_proto(id)),
            }))
            .await
            .map(|response| response.into_inner())
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to query sequenced withdrawal status for {:?}: {err}",
                    id
                ))
            })
    }

    async fn load_canonical_proposal_artifacts(
        &self,
        id: &WithdrawalId,
    ) -> Result<Option<WithdrawalSequencerProposalArtifacts>, BridgeError> {
        let mut client = self.connect().await?;
        let response: SequencerCanonicalProposalArtifactsResponse = client
            .get_canonical_proposal_artifacts(Request::new(
                SequencerCanonicalProposalArtifactsRequest {
                    withdrawal_id: Some(withdrawal_id_to_proto(id)),
                },
            ))
            .await
            .map(|response| response.into_inner())
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to load canonical withdrawal artifacts for {:?}: {err}",
                    id
                ))
            })?;
        if !response.found {
            return Ok(None);
        }
        let response_id = response
            .withdrawal_id
            .as_ref()
            .ok_or_else(|| {
                BridgeError::Runtime(
                    "sequencer canonical artifacts response missing withdrawal id".into(),
                )
            })
            .and_then(withdrawal_id_from_proto)?;
        let snapshot = response
            .snapshot
            .as_ref()
            .ok_or_else(|| {
                BridgeError::Runtime(
                    "sequencer canonical artifacts response missing snapshot".into(),
                )
            })
            .and_then(snapshot_from_proto)?;
        let selected_inputs = response
            .selected_inputs
            .iter()
            .map(note_name_from_proto)
            .collect::<Result<Vec<_>, _>>()?;
        let transaction = cue_transaction(response.transaction_jam)?;
        Ok(Some(WithdrawalSequencerProposalArtifacts {
            id: response_id,
            epoch: response.epoch,
            proposal_hash: response.proposal_hash,
            amount: response.amount,
            base_batch_end: response.base_batch_end,
            snapshot,
            selected_inputs,
            transaction,
            commit_certificate: (!response.commit_certificate.is_empty())
                .then_some(response.commit_certificate),
            authorized_transaction_name: (!response.authorized_transaction_name.is_empty())
                .then_some(response.authorized_transaction_name),
            authorized_transaction_jam: (!response.authorized_transaction_jam.is_empty())
                .then_some(response.authorized_transaction_jam),
            authorized_raw_tx: (!response.authorized_raw_tx.is_empty())
                .then_some(response.authorized_raw_tx),
        }))
    }
}
