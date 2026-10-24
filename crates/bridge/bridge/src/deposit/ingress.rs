use alloy::primitives::{Address, Signature as AlloySignature, B256};
use hex::encode;
use tonic::{Request, Response, Status};
use tracing::{info, warn};

use crate::deposit::cache::{
    ProposalStatus as CacheProposalStatus, SignatureAddResult, SignatureData,
};
use crate::deposit::types::DepositId;
use crate::observability::metrics;
use crate::observability::tui::types::{AlertSeverity, Proposal, ProposalStatus};
use crate::shared::ingress::proto::{
    DepositConfirmationBroadcast, DepositConfirmationBroadcastResponse,
    DepositProposalStatusRequest, DepositProposalStatusResponse, DepositSignatureBroadcast,
    DepositSignatureBroadcastResponse,
};
use crate::shared::ingress::IngressService;

/// Verifies an Ethereum signature and recovers the deposit signer address.
pub(crate) fn verify_deposit_signature(
    proposal_hash: &[u8; 32],
    signature: &[u8],
) -> Option<Address> {
    if signature.len() != 65 {
        warn!(
            target: "bridge.ingress",
            sig_len = signature.len(),
            "invalid signature length, expected 65 bytes"
        );
        return None;
    }

    let mut r = [0u8; 32];
    let mut s = [0u8; 32];
    r.copy_from_slice(&signature[0..32]);
    s.copy_from_slice(&signature[32..64]);
    let v = signature[64];

    if v != 27 && v != 28 {
        warn!(
            target: "bridge.ingress",
            v = v,
            "invalid signature v value, expected 27 or 28"
        );
        return None;
    }

    let y_parity = v == 28;
    let sig = AlloySignature::new(
        alloy::primitives::U256::from_be_bytes(r),
        alloy::primitives::U256::from_be_bytes(s),
        y_parity,
    );

    let hash = B256::from_slice(proposal_hash);
    match sig.recover_address_from_msg(hash.as_slice()) {
        Ok(addr) => Some(addr),
        Err(err) => {
            warn!(
                target: "bridge.ingress",
                error = %err,
                sig_r = %hex::encode(r),
                sig_s = %hex::encode(s),
                sig_v = v,
                hash = %hex::encode(proposal_hash),
                "failed to recover address from signature"
            );
            None
        }
    }
}

/// Handles inbound deposit-signature broadcasts from peer bridges.
pub(crate) async fn broadcast_deposit_signature(
    service: &IngressService,
    request: Request<DepositSignatureBroadcast>,
) -> Result<Response<DepositSignatureBroadcastResponse>, Status> {
    let metrics = metrics::init_metrics();
    metrics.ingress_broadcast_signature_requests.increment();
    let req = request.into_inner();

    if req.deposit_id.len() != 120 {
        metrics
            .ingress_broadcast_signature_invalid_deposit_id_len
            .increment();
        return Ok(Response::new(DepositSignatureBroadcastResponse {
            accepted: false,
            error: format!(
                "invalid deposit_id length: expected 120, got {}",
                req.deposit_id.len()
            ),
        }));
    }
    if req.proposal_hash.len() != 32 {
        metrics
            .ingress_broadcast_signature_invalid_proposal_hash_len
            .increment();
        return Ok(Response::new(DepositSignatureBroadcastResponse {
            accepted: false,
            error: format!(
                "invalid proposal_hash length: expected 32, got {}",
                req.proposal_hash.len()
            ),
        }));
    }
    if req.signature.len() != 65 {
        metrics
            .ingress_broadcast_signature_invalid_signature_len
            .increment();
        return Ok(Response::new(DepositSignatureBroadcastResponse {
            accepted: false,
            error: format!(
                "invalid signature length: expected 65, got {}",
                req.signature.len()
            ),
        }));
    }
    if req.signer_address.len() != 20 {
        metrics
            .ingress_broadcast_signature_invalid_signer_address_len
            .increment();
        return Ok(Response::new(DepositSignatureBroadcastResponse {
            accepted: false,
            error: format!(
                "invalid signer_address length: expected 20, got {}",
                req.signer_address.len()
            ),
        }));
    }

    let deposit_id = match DepositId::from_bytes(&req.deposit_id) {
        Ok(id) => id,
        Err(err) => {
            metrics
                .ingress_broadcast_signature_invalid_deposit_id_decode
                .increment();
            return Ok(Response::new(DepositSignatureBroadcastResponse {
                accepted: false,
                error: format!("failed to deserialize deposit_id: {}", err),
            }));
        }
    };

    let signer_address = Address::from_slice(&req.signer_address);
    let mut proposal_hash = [0u8; 32];
    proposal_hash.copy_from_slice(&req.proposal_hash);

    if signer_address == service.deposit_signer().address() {
        metrics.ingress_broadcast_signature_ignored_self.increment();
        tracing::debug!(
            target: "bridge.ingress",
            signer = %signer_address,
            "ignoring self signature broadcast"
        );
        return Ok(Response::new(DepositSignatureBroadcastResponse {
            accepted: true,
            error: String::new(),
        }));
    }

    let signer_is_known = service
        .deposit_signer_address_to_node_id()
        .contains_key(&signer_address);
    if signer_is_known {
        metrics.ingress_broadcast_signature_known_signer.increment();
    } else {
        metrics
            .ingress_broadcast_signature_unknown_signer
            .increment();
    }

    info!(
        target: "bridge.ingress",
        deposit_id = ?deposit_id,
        deposit_id_bytes = %encode(&req.deposit_id),
        signer = %signer_address,
        proposal_hash = %encode(proposal_hash),
        timestamp = req.timestamp,
        "received signature broadcast"
    );

    let existing_state = match service.deposit_proposal_cache().get_state(&deposit_id) {
        Ok(state) => state,
        Err(err) => {
            warn!(
                target: "bridge.ingress",
                error = %err,
                "failed to read proposal cache state"
            );
            None
        }
    };

    if let Some(state) = existing_state.as_ref() {
        metrics
            .ingress_broadcast_signature_known_proposal
            .increment();
        if !signer_is_known {
            metrics
                .ingress_broadcast_signature_unknown_signer_known_proposal
                .increment();
        }
        if state.proposal_hash != proposal_hash {
            metrics
                .ingress_broadcast_signature_hash_mismatch
                .increment();
            let expected_hex = encode(state.proposal_hash);
            let received_hex = encode(proposal_hash);
            warn!(
                target: "bridge.ingress",
                deposit_id = %encode(&req.deposit_id),
                signer = %signer_address,
                expected_hash = %expected_hex,
                received_hash = %received_hex,
                "peer signature proposal hash mismatch, possible nonce divergence"
            );
            service.bridge_status().push_alert(
                AlertSeverity::Error,
                "Nonce Divergence Suspected".to_string(),
                format!(
                    "Peer signature proposal hash mismatch for deposit {}. expected={}, received={}, signer={}",
                    encode(&req.deposit_id),
                    expected_hex,
                    received_hex,
                    signer_address
                ),
                "nonce-divergence".to_string(),
            );
            return Ok(Response::new(DepositSignatureBroadcastResponse {
                accepted: false,
                error: "proposal hash mismatch (possible nonce divergence)".to_string(),
            }));
        }
    } else {
        metrics
            .ingress_broadcast_signature_unknown_proposal
            .increment();
    }

    let result = service.deposit_proposal_cache().add_signature(
        &deposit_id,
        SignatureData {
            signer_address,
            signature: req.signature.clone(),
            proposal_hash,
            is_mine: false,
        },
        None,
        verify_deposit_signature,
    );

    let proposal_hash_hex = encode(proposal_hash);

    match result {
        Ok(SignatureAddResult::Added) => {
            metrics.ingress_broadcast_signature_result_added.increment();
            info!(
                target: "bridge.ingress",
                deposit_id = ?deposit_id,
                signer = %signer_address,
                "signature added to cache"
            );

            if let Ok(Some(state)) = service.deposit_proposal_cache().get_state(&deposit_id) {
                service.bridge_status().sync_proposal_signatures_from_cache(
                    &proposal_hash_hex,
                    &state,
                    service.deposit_signer_address_to_node_id(),
                    service.node_id(),
                );
            }

            Ok(Response::new(DepositSignatureBroadcastResponse {
                accepted: true,
                error: String::new(),
            }))
        }
        Ok(SignatureAddResult::ThresholdReached) => {
            metrics
                .ingress_broadcast_signature_result_threshold_reached
                .increment();
            info!(
                target: "bridge.ingress",
                deposit_id = ?deposit_id,
                signer = %signer_address,
                "signature added - threshold reached!"
            );

            if let Ok(Some(state)) = service.deposit_proposal_cache().get_state(&deposit_id) {
                service.bridge_status().sync_proposal_signatures_from_cache(
                    &proposal_hash_hex,
                    &state,
                    service.deposit_signer_address_to_node_id(),
                    service.node_id(),
                );
            }

            Ok(Response::new(DepositSignatureBroadcastResponse {
                accepted: true,
                error: String::new(),
            }))
        }
        Ok(SignatureAddResult::Duplicate) => {
            metrics
                .ingress_broadcast_signature_result_duplicate
                .increment();
            info!(
                target: "bridge.ingress",
                deposit_id = ?deposit_id,
                signer = %signer_address,
                "duplicate signature ignored"
            );
            Ok(Response::new(DepositSignatureBroadcastResponse {
                accepted: true,
                error: String::new(),
            }))
        }
        Ok(SignatureAddResult::Stale) => {
            metrics.ingress_broadcast_signature_result_stale.increment();
            info!(
                target: "bridge.ingress",
                deposit_id = ?deposit_id,
                signer = %signer_address,
                "deposit proposal stale (already confirmed), rejecting signature"
            );
            Ok(Response::new(DepositSignatureBroadcastResponse {
                accepted: false,
                error: "deposit proposal stale (already confirmed)".to_string(),
            }))
        }
        Ok(SignatureAddResult::Invalid(msg)) => {
            metrics
                .ingress_broadcast_signature_result_invalid
                .increment();
            warn!(
                target: "bridge.ingress",
                deposit_id = ?deposit_id,
                signer = %signer_address,
                error = %msg,
                "signature verification failed"
            );
            Ok(Response::new(DepositSignatureBroadcastResponse {
                accepted: false,
                error: msg,
            }))
        }
        Err(err) => {
            metrics.ingress_broadcast_signature_result_error.increment();
            warn!(
                target: "bridge.ingress",
                deposit_id = ?deposit_id,
                signer = %signer_address,
                error = %err,
                "cannot add signature to cache (peer may retry)"
            );
            Ok(Response::new(DepositSignatureBroadcastResponse {
                accepted: false,
                error: err,
            }))
        }
    }
}

/// Returns the local proposal status used by peer signature convergence.
pub(crate) async fn get_deposit_proposal_status(
    service: &IngressService,
    request: Request<DepositProposalStatusRequest>,
) -> Result<Response<DepositProposalStatusResponse>, Status> {
    let req = request.into_inner();

    if req.deposit_id.len() != 120 {
        return Err(Status::invalid_argument(format!(
            "invalid deposit_id length: expected 120, got {}",
            req.deposit_id.len()
        )));
    }

    let deposit_id = DepositId::from_bytes(&req.deposit_id).map_err(|err| {
        Status::invalid_argument(format!("failed to deserialize deposit_id: {}", err))
    })?;

    let state = service
        .deposit_proposal_cache()
        .get_state(&deposit_id)
        .map_err(|err| Status::internal(format!("failed to get proposal state: {}", err)))?;

    match state {
        Some(state) => {
            let status_str = match state.status {
                CacheProposalStatus::Collecting => "collecting",
                CacheProposalStatus::Ready => "ready",
                CacheProposalStatus::Posting => "posting",
                CacheProposalStatus::Confirmed => "confirmed",
                CacheProposalStatus::Failed => "failed",
            };

            let signature_count = (state.peer_signatures.len()
                + if state.my_signature.is_some() { 1 } else { 0 })
                as u32;

            let mut signers: Vec<Vec<u8>> = Vec::new();
            if state.my_signature.is_some() {
                signers.push(service.deposit_signer().address().to_vec());
            }
            for addr in state.peer_signatures.keys() {
                signers.push(addr.to_vec());
            }

            info!(
                target: "bridge.ingress",
                deposit_id = ?deposit_id,
                status = status_str,
                signature_count = signature_count,
                "retrieved proposal status"
            );

            Ok(Response::new(DepositProposalStatusResponse {
                status: status_str.to_string(),
                signature_count,
                signers,
                tx_hash: None,
            }))
        }
        None => {
            info!(
                target: "bridge.ingress",
                deposit_id = ?deposit_id,
                "proposal not found in cache"
            );
            Ok(Response::new(DepositProposalStatusResponse {
                status: "not_found".to_string(),
                signature_count: 0,
                signers: vec![],
                tx_hash: None,
            }))
        }
    }
}

/// Handles confirmation broadcasts from the node that posted to BASE.
pub(crate) async fn broadcast_deposit_confirmation(
    service: &IngressService,
    request: Request<DepositConfirmationBroadcast>,
) -> Result<Response<DepositConfirmationBroadcastResponse>, Status> {
    let req = request.into_inner();

    let deposit_id = match DepositId::from_bytes(&req.deposit_id) {
        Ok(id) => id,
        Err(err) => {
            warn!(
                target: "bridge.ingress",
                error = %err,
                "failed to parse deposit_id from confirmation broadcast"
            );
            return Ok(Response::new(DepositConfirmationBroadcastResponse {
                accepted: false,
            }));
        }
    };

    let proposal_hash = hex::encode(&req.proposal_hash);
    let tx_hash = hex::encode(&req.tx_hash);

    info!(
        target: "bridge.ingress",
        deposit_id = ?deposit_id,
        proposal_hash = %proposal_hash,
        tx_hash = %tx_hash,
        block_number = req.block_number,
        "received confirmation broadcast"
    );

    match service.deposit_proposal_cache().mark_confirmed(&deposit_id) {
        Ok(()) => {
            info!(
                target: "bridge.ingress",
                deposit_id = ?deposit_id,
                proposal_hash = %proposal_hash,
                "marked proposal as confirmed from broadcast"
            );

            if let Some(mut tui_proposal) = service.bridge_status().find_proposal(&proposal_hash) {
                tui_proposal.status = ProposalStatus::Executed;
                tui_proposal.tx_hash = Some(tx_hash.clone());
                tui_proposal.executed_at_block = Some(req.block_number);
                service.bridge_status().update_proposal(tui_proposal);
                info!(
                    target: "bridge.ingress",
                    proposal_hash = %proposal_hash,
                    "updated TUI proposal to Executed status"
                );
            } else {
                let placeholder = Proposal {
                    id: proposal_hash.clone(),
                    proposal_type: "deposit".to_string(),
                    description: "Confirmed via peer broadcast".to_string(),
                    signatures_collected: 3,
                    signatures_required: 3,
                    signers: vec![],
                    created_at: std::time::SystemTime::now(),
                    status: ProposalStatus::Executed,
                    data_hash: proposal_hash.clone(),
                    submitted_at_block: Some(req.block_number),
                    submitted_at: Some(std::time::SystemTime::now()),
                    tx_hash: Some(tx_hash.clone()),
                    time_to_submit_ms: None,
                    executed_at_block: Some(req.block_number),
                    source_block: None,
                    amount: None,
                    recipient: None,
                    nonce: None,
                    source_tx_id: None,
                    current_proposer: None,
                    is_my_turn: false,
                    time_until_takeover: None,
                };
                service.bridge_status().update_proposal(placeholder);
                info!(
                    target: "bridge.ingress",
                    proposal_hash = %proposal_hash,
                    "created placeholder TUI proposal for early confirmation"
                );
            }

            Ok(Response::new(DepositConfirmationBroadcastResponse {
                accepted: true,
            }))
        }
        Err(err) => {
            warn!(
                target: "bridge.ingress",
                error = %err,
                deposit_id = ?deposit_id,
                "failed to mark proposal as confirmed"
            );
            Ok(Response::new(DepositConfirmationBroadcastResponse {
                accepted: false,
            }))
        }
    }
}
