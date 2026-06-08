use tonic::{Request, Response, Status};

use crate::shared::ingress::proto::{
    CanonicalWithdrawalProposalBroadcast, CanonicalWithdrawalProposalBroadcastResponse,
    SignedWithdrawalProposalBroadcast, SignedWithdrawalProposalBroadcastResponse,
    WithdrawalProposalBroadcast, WithdrawalProposalBroadcastResponse,
    WithdrawalProposalStatusRequest, WithdrawalProposalStatusResponse,
};
use crate::shared::ingress::IngressService;
use crate::withdrawal::transport::{proposal_from_proto, withdrawal_id_from_proto};

/// Handles inbound prepared-proposal broadcasts from peer bridges.
pub(crate) async fn broadcast_withdrawal_proposal(
    service: &IngressService,
    request: Request<WithdrawalProposalBroadcast>,
) -> Result<Response<WithdrawalProposalBroadcastResponse>, Status> {
    let Some(transport) = service.withdrawal_transport() else {
        return Err(Status::failed_precondition(
            "withdrawal proposal transport is not configured",
        ));
    };

    let req = request.into_inner();
    let proposal = req
        .proposal
        .ok_or_else(|| Status::invalid_argument("missing withdrawal proposal envelope"))?;
    let proposal = proposal_from_proto(&proposal)
        .map_err(|err| Status::invalid_argument(format!("invalid withdrawal proposal: {err}")))?;
    let proposal_hash = proposal
        .proposal_hash()
        .map_err(|err| Status::internal(format!("failed to hash withdrawal proposal: {err}")))?;
    if proposal_hash != req.proposal_hash {
        return Ok(Response::new(WithdrawalProposalBroadcastResponse {
            accepted: false,
            status: "hash_mismatch".to_string(),
            proposal_hash,
            responder_node_id: service.node_id(),
            error: "proposal hash mismatch".to_string(),
            commit_signature: None,
        }));
    }

    let response = transport
        .ingest_peer_proposal(req.sender_node_id, &proposal)
        .await
        .map_err(|err| Status::internal(format!("withdrawal proposal ingestion failed: {err}")))?;
    Ok(Response::new(response))
}

/// Handles inbound signed-proposal broadcasts from peer bridges.
pub(crate) async fn broadcast_signed_withdrawal_proposal(
    service: &IngressService,
    request: Request<SignedWithdrawalProposalBroadcast>,
) -> Result<Response<SignedWithdrawalProposalBroadcastResponse>, Status> {
    let Some(transport) = service.withdrawal_transport() else {
        return Err(Status::failed_precondition(
            "withdrawal proposal transport is not configured",
        ));
    };

    let req = request.into_inner();
    let proposal = req
        .proposal
        .ok_or_else(|| Status::invalid_argument("missing signed withdrawal proposal envelope"))?;
    let proposal = proposal_from_proto(&proposal).map_err(|err| {
        Status::invalid_argument(format!("invalid signed withdrawal proposal: {err}"))
    })?;
    let proposal_hash = proposal.proposal_hash().map_err(|err| {
        Status::internal(format!("failed to hash signed withdrawal proposal: {err}"))
    })?;
    if proposal_hash != req.proposal_hash {
        return Ok(Response::new(SignedWithdrawalProposalBroadcastResponse {
            accepted: false,
            status: "hash_mismatch".to_string(),
            proposal_hash,
            responder_node_id: service.node_id(),
            error: "proposal hash mismatch".to_string(),
        }));
    }

    let response = transport
        .ingest_peer_signed_proposal(req.sender_node_id, &proposal)
        .await
        .map_err(|err| {
            Status::internal(format!(
                "signed withdrawal proposal ingestion failed: {err}"
            ))
        })?;
    Ok(Response::new(response))
}

/// Handles inbound peer-canonical proposal broadcasts from peer bridges.
pub(crate) async fn broadcast_canonical_withdrawal_proposal(
    service: &IngressService,
    request: Request<CanonicalWithdrawalProposalBroadcast>,
) -> Result<Response<CanonicalWithdrawalProposalBroadcastResponse>, Status> {
    let Some(transport) = service.withdrawal_transport() else {
        return Err(Status::failed_precondition(
            "withdrawal proposal transport is not configured",
        ));
    };

    let req = request.into_inner();
    let proposal = req
        .proposal
        .ok_or_else(|| Status::invalid_argument("missing withdrawal proposal envelope"))?;
    let proposal = proposal_from_proto(&proposal)
        .map_err(|err| Status::invalid_argument(format!("invalid withdrawal proposal: {err}")))?;
    let proposal_hash = proposal
        .proposal_hash()
        .map_err(|err| Status::internal(format!("failed to hash withdrawal proposal: {err}")))?;
    if proposal_hash != req.proposal_hash {
        return Ok(Response::new(
            CanonicalWithdrawalProposalBroadcastResponse {
                accepted: false,
                status: "hash_mismatch".to_string(),
                proposal_hash,
                error: "proposal hash mismatch".to_string(),
            },
        ));
    }

    let response = transport
        .ingest_canonicalized_proposal(
            req.sender_node_id,
            &proposal,
            req.commit_certificate
                .as_ref()
                .ok_or_else(|| Status::invalid_argument("missing withdrawal commit certificate"))?,
        )
        .await
        .map_err(|err| {
            Status::internal(format!(
                "canonical withdrawal proposal ingestion failed: {err}"
            ))
        })?;
    Ok(Response::new(response))
}

/// Returns the locally persisted status for one withdrawal proposal epoch.
pub(crate) async fn get_withdrawal_proposal_status(
    service: &IngressService,
    request: Request<WithdrawalProposalStatusRequest>,
) -> Result<Response<WithdrawalProposalStatusResponse>, Status> {
    let Some(transport) = service.withdrawal_transport() else {
        return Err(Status::failed_precondition(
            "withdrawal proposal transport is not configured",
        ));
    };

    let req = request.into_inner();
    let id = req
        .withdrawal_id
        .ok_or_else(|| Status::invalid_argument("missing withdrawal id"))?;
    let id = withdrawal_id_from_proto(&id)
        .map_err(|err| Status::invalid_argument(format!("invalid withdrawal id: {err}")))?;
    let response = transport
        .proposal_status(id, req.epoch)
        .await
        .map_err(|err| Status::internal(format!("withdrawal proposal status failed: {err}")))?;
    Ok(Response::new(response))
}
