use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, Signature as AlloySignature, B256};
use hex::encode;
use tonic::{async_trait, Request, Response, Status};
use tonic_reflection::server::Builder as ReflectionBuilder;
use tracing::{debug, info, warn};

use crate::bridge_status::BridgeStatus;
use crate::errors::BridgeError;
use crate::metrics;
use crate::proposal_cache::{ProposalCache, SignatureAddResult};
use crate::runtime::BridgeRuntimeHandle;
use crate::signing::BridgeSigner;
use crate::status::{BridgeStatusState, StatusService};
use crate::tui::types::{AlertSeverity, Proposal, ProposalStatus};
use crate::tui_api::proto::bridge_tui_server::BridgeTuiServer;
use crate::tui_api::BridgeTuiService;
use crate::types::DepositId;

pub mod proto {
    tonic::include_proto!("bridge.ingress.v1");
}

use proto::bridge_ingress_server::{BridgeIngress, BridgeIngressServer};
use proto::{
    ConfirmationBroadcast, ConfirmationBroadcastResponse, HealthCheckRequest, HealthCheckResponse,
    ProposalStatusRequest, ProposalStatusResponse, SignatureBroadcast, SignatureBroadcastResponse,
    StopBroadcast, StopBroadcastResponse,
};

use crate::status::proto::bridge_status_server::BridgeStatusServer;

pub fn spawn_broadcast_stop_to_peers(
    peers: &[crate::health::PeerEndpoint],
    msg: StopBroadcast,
    component: &'static str,
) {
    use tracing::{info, warn};

    use crate::ingress::proto::bridge_ingress_client::BridgeIngressClient;

    for peer in peers {
        let addr = peer.address.clone();
        let peer_id = peer.node_id;
        let msg = msg.clone();
        // Note: there is no retry logic here, we fire-and-forget.
        tokio::spawn(async move {
            match BridgeIngressClient::connect(addr.clone()).await {
                Ok(mut client) => match client.broadcast_stop(msg).await {
                    Ok(_) => {
                        info!(component, peer_node_id = peer_id, "broadcast stop to peer");
                    }
                    Err(e) => {
                        warn!(
                            component,
                            peer_node_id=peer_id,
                            error=%e,
                            "failed to broadcast stop to peer"
                        );
                    }
                },
                Err(e) => {
                    warn!(
                        component,
                        peer_node_id=peer_id,
                        peer_address=%addr,
                        error=%e,
                        "failed to connect to peer for stop broadcast"
                    );
                }
            }
        });
    }
}

pub struct IngressService {
    runtime: Arc<BridgeRuntimeHandle>,
    node_id: u64,
    start_time: Instant,
    /// Signer for creating Ethereum signatures on proposals
    signer: Arc<BridgeSigner>,
    /// Cache for aggregating signatures from multiple bridge nodes
    proposal_cache: Arc<ProposalCache>,
    /// Shared TUI state for updating proposal display on peer broadcasts
    bridge_status: BridgeStatus,
    /// Mapping from Ethereum address to node ID for TUI signature display
    address_to_node_id: std::collections::HashMap<Address, u64>,
    stop_controller: crate::stop::StopController,
    peers: Vec<crate::health::PeerEndpoint>,
}

impl IngressService {
    #[allow(clippy::too_many_arguments)]
    fn new(
        node_id: u64,
        runtime: Arc<BridgeRuntimeHandle>,
        signer: Arc<BridgeSigner>,
        proposal_cache: Arc<ProposalCache>,
        bridge_status: BridgeStatus,
        address_to_node_id: std::collections::HashMap<Address, u64>,
        stop_controller: crate::stop::StopController,
        peers: Vec<crate::health::PeerEndpoint>,
    ) -> Self {
        Self {
            runtime,
            node_id,
            start_time: Instant::now(),
            signer,
            proposal_cache,
            bridge_status,
            address_to_node_id,
            stop_controller,
            peers,
        }
    }

    fn uptime_millis(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }

    async fn trigger_stop(
        &self,
        reason: String,
        last: Option<crate::types::StopLastBlocks>,
        source: crate::stop::StopSource,
        broadcast: bool,
    ) {
        use std::time::{SystemTime, UNIX_EPOCH};

        use tracing::warn;

        use crate::stop::{StopInfo, StopSource};
        use crate::tui::types::AlertSeverity;

        let resolved_last = match last {
            Some(last) => Some(last),
            None => match self.runtime.peek_stop_info().await {
                Ok(last) => last,
                Err(err) => {
                    warn!(
                        target: "bridge.ingress",
                        error=%err,
                        "failed to peek stop-info while triggering stop"
                    );
                    None
                }
            },
        };

        let info = StopInfo {
            reason: reason.clone(),
            last: resolved_last.clone(),
            source,
            at: SystemTime::now(),
        };

        if !self.stop_controller.trigger(info) {
            return;
        }

        self.bridge_status.push_alert(
            AlertSeverity::Error,
            "Bridge Stopped".to_string(),
            reason.clone(),
            match source {
                StopSource::KernelEffect => "kernel-stop".to_string(),
                StopSource::PeerBroadcast => "peer-stop".to_string(),
                StopSource::Local => "local-stop".to_string(),
            },
        );

        if let Some(last) = resolved_last.clone() {
            if let Err(err) = self.runtime.send_stop(last).await {
                warn!(
                    target: "bridge.ingress",
                    error=%err,
                    "failed to poke kernel with stop cause"
                );
            }
        } else {
            warn!(
                target: "bridge.ingress",
                "stop triggered without stop-info; skipping kernel stop poke"
            );
        }

        if !broadcast {
            return;
        }

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let (last_base_hash, last_base_height, last_nock_hash, last_nock_height) =
            if let Some(ref last) = resolved_last {
                (
                    Some(last.base.base_hash.to_be_limb_bytes().to_vec()),
                    Some(last.base.height),
                    Some(last.nock.nock_hash.to_be_limb_bytes().to_vec()),
                    Some(last.nock.height),
                )
            } else {
                (None, None, None, None)
            };

        let msg = StopBroadcast {
            sender_node_id: self.node_id,
            reason: reason.clone(),
            last_base_hash,
            last_base_height,
            last_nock_hash,
            last_nock_height,
            timestamp,
        };

        spawn_broadcast_stop_to_peers(&self.peers, msg, "bridge.ingress");
    }

    /// Verify an Ethereum signature and recover the signer address.
    /// Returns Some(address) if signature is valid, None otherwise.
    fn verify_signature(proposal_hash: &[u8; 32], signature: &[u8]) -> Option<Address> {
        if signature.len() != 65 {
            warn!(
                target: "bridge.ingress",
                sig_len = signature.len(),
                "invalid signature length, expected 65 bytes"
            );
            return None;
        }

        // Extract r, s, v from 65-byte signature
        let mut r = [0u8; 32];
        let mut s = [0u8; 32];
        r.copy_from_slice(&signature[0..32]);
        s.copy_from_slice(&signature[32..64]);
        let v = signature[64];

        // v must be 27 or 28 for Ethereum signatures
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

        // EIP-191 recovery (matches sign_hash in signing.rs which uses sign_message)
        let hash = B256::from_slice(proposal_hash);
        match sig.recover_address_from_msg(hash.as_slice()) {
            Ok(addr) => Some(addr),
            Err(e) => {
                warn!(
                    target: "bridge.ingress",
                    error = %e,
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
}

#[async_trait]
impl BridgeIngress for IngressService {
    async fn health_check(
        &self,
        request: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        let req = request.into_inner();
        debug!(
            target: "bridge.ingress",
            requester_id=req.requester_node_id,
            requester_addr=req.requester_address,
            "received health check"
        );
        let timestamp_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default();
        let response = HealthCheckResponse {
            responder_node_id: self.node_id,
            uptime_millis: self.uptime_millis(),
            status: "healthy".into(),
            timestamp_millis,
        };
        Ok(Response::new(response))
    }

    async fn broadcast_signature(
        &self,
        request: Request<SignatureBroadcast>,
    ) -> Result<Response<SignatureBroadcastResponse>, Status> {
        let metrics = metrics::init_metrics();
        metrics.ingress_broadcast_signature_requests.increment();
        let req = request.into_inner();

        // Validate inputs
        if req.deposit_id.len() != 120 {
            metrics
                .ingress_broadcast_signature_invalid_deposit_id_len
                .increment();
            return Ok(Response::new(SignatureBroadcastResponse {
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
            return Ok(Response::new(SignatureBroadcastResponse {
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
            return Ok(Response::new(SignatureBroadcastResponse {
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
            return Ok(Response::new(SignatureBroadcastResponse {
                accepted: false,
                error: format!(
                    "invalid signer_address length: expected 20, got {}",
                    req.signer_address.len()
                ),
            }));
        }

        // Deserialize deposit_id
        let deposit_id = match DepositId::from_bytes(&req.deposit_id) {
            Ok(id) => id,
            Err(e) => {
                metrics
                    .ingress_broadcast_signature_invalid_deposit_id_decode
                    .increment();
                return Ok(Response::new(SignatureBroadcastResponse {
                    accepted: false,
                    error: format!("failed to deserialize deposit_id: {}", e),
                }));
            }
        };

        let signer_address = Address::from_slice(&req.signer_address);
        let mut proposal_hash = [0u8; 32];
        proposal_hash.copy_from_slice(&req.proposal_hash);

        if signer_address == self.signer.address() {
            metrics.ingress_broadcast_signature_ignored_self.increment();
            tracing::debug!(
                target: "bridge.ingress",
                signer=%signer_address,
                "ignoring self signature broadcast"
            );
            return Ok(Response::new(SignatureBroadcastResponse {
                accepted: true,
                error: String::new(),
            }));
        }

        let signer_is_known = self.address_to_node_id.contains_key(&signer_address);
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

        let existing_state = match self.proposal_cache.get_state(&deposit_id) {
            Ok(state) => state,
            Err(err) => {
                warn!(
                    target: "bridge.ingress",
                    error=%err,
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
                self.bridge_status.push_alert(
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
                return Ok(Response::new(SignatureBroadcastResponse {
                    accepted: false,
                    error: "proposal hash mismatch (possible nonce divergence)".to_string(),
                }));
            }
        } else {
            metrics
                .ingress_broadcast_signature_unknown_proposal
                .increment();
        }

        // TODO: Verify signer is authorized bridge node (check against node config)
        // For now, we'll accept any signature and let the verification fail if invalid

        // Add signature to cache (or queue if we haven't processed this deposit yet)
        // The verify_fn will check that signature recovers to claimed signer
        let result = self.proposal_cache.add_signature(
            &deposit_id,
            crate::proposal_cache::SignatureData {
                signer_address,
                signature: req.signature.clone(),
                proposal_hash,
                is_mine: false, // not our signature
            },
            None, // No proposal data - we generate our own from kernel
            Self::verify_signature,
        );

        // Convert proposal_hash to hex string for TUI lookup
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

                if let Ok(Some(state)) = self.proposal_cache.get_state(&deposit_id) {
                    self.bridge_status.sync_proposal_signatures_from_cache(
                        &proposal_hash_hex, &state, &self.address_to_node_id, self.node_id,
                    );
                }

                Ok(Response::new(SignatureBroadcastResponse {
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

                if let Ok(Some(state)) = self.proposal_cache.get_state(&deposit_id) {
                    self.bridge_status.sync_proposal_signatures_from_cache(
                        &proposal_hash_hex, &state, &self.address_to_node_id, self.node_id,
                    );
                }

                Ok(Response::new(SignatureBroadcastResponse {
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
                Ok(Response::new(SignatureBroadcastResponse {
                    accepted: true, // not an error, just already have it
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
                Ok(Response::new(SignatureBroadcastResponse {
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
                Ok(Response::new(SignatureBroadcastResponse {
                    accepted: false,
                    error: msg,
                }))
            }
            Err(e) => {
                metrics.ingress_broadcast_signature_result_error.increment();
                // This can happen if peer broadcasts signature before we've seen the deposit
                // (race condition) - not a real error, peer should retry
                warn!(
                    target: "bridge.ingress",
                    deposit_id = ?deposit_id,
                    signer = %signer_address,
                    error = %e,
                    "cannot add signature to cache (peer may retry)"
                );
                Ok(Response::new(SignatureBroadcastResponse {
                    accepted: false,
                    error: e,
                }))
            }
        }
    }

    async fn get_proposal_status(
        &self,
        request: Request<ProposalStatusRequest>,
    ) -> Result<Response<ProposalStatusResponse>, Status> {
        let req = request.into_inner();

        // Validate deposit_id
        if req.deposit_id.len() != 120 {
            return Err(Status::invalid_argument(format!(
                "invalid deposit_id length: expected 120, got {}",
                req.deposit_id.len()
            )));
        }

        // Deserialize deposit_id
        let deposit_id = DepositId::from_bytes(&req.deposit_id).map_err(|e| {
            Status::invalid_argument(format!("failed to deserialize deposit_id: {}", e))
        })?;

        // Get proposal state from cache
        let state = self
            .proposal_cache
            .get_state(&deposit_id)
            .map_err(|e| Status::internal(format!("failed to get proposal state: {}", e)))?;

        match state {
            Some(state) => {
                // Convert status enum to string
                let status_str = match state.status {
                    crate::proposal_cache::ProposalStatus::Collecting => "collecting",
                    crate::proposal_cache::ProposalStatus::Ready => "ready",
                    crate::proposal_cache::ProposalStatus::Posting => "posting",
                    crate::proposal_cache::ProposalStatus::Confirmed => "confirmed",
                    crate::proposal_cache::ProposalStatus::Failed => "failed",
                };

                let signature_count = (state.peer_signatures.len()
                    + if state.my_signature.is_some() { 1 } else { 0 })
                    as u32;

                // Collect signer addresses
                let mut signers = Vec::new();
                if state.my_signature.is_some() {
                    signers.push(self.signer.address().to_vec());
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

                Ok(Response::new(ProposalStatusResponse {
                    status: status_str.to_string(),
                    signature_count,
                    signers,
                    tx_hash: None, // TODO: add tx_hash tracking when we integrate with poster
                }))
            }
            None => {
                info!(
                    target: "bridge.ingress",
                    deposit_id = ?deposit_id,
                    "proposal not found in cache"
                );
                Ok(Response::new(ProposalStatusResponse {
                    status: "not_found".to_string(),
                    signature_count: 0,
                    signers: vec![],
                    tx_hash: None,
                }))
            }
        }
    }

    /// Handle confirmation broadcast from the node that posted to BASE.
    /// This allows non-proposer nodes to mark proposals as confirmed and stop
    /// waiting for their failover turn.
    async fn broadcast_confirmation(
        &self,
        request: Request<ConfirmationBroadcast>,
    ) -> Result<Response<ConfirmationBroadcastResponse>, Status> {
        let req = request.into_inner();

        // Parse deposit_id from bytes
        let deposit_id = match DepositId::from_bytes(&req.deposit_id) {
            Ok(id) => id,
            Err(e) => {
                warn!(
                    target: "bridge.ingress",
                    error = %e,
                    "failed to parse deposit_id from confirmation broadcast"
                );
                return Ok(Response::new(ConfirmationBroadcastResponse {
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

        // Mark the proposal as confirmed in our cache
        match self.proposal_cache.mark_confirmed(&deposit_id) {
            Ok(()) => {
                info!(
                    target: "bridge.ingress",
                    deposit_id = ?deposit_id,
                    proposal_hash = %proposal_hash,
                    "marked proposal as confirmed from broadcast"
                );

                // Update TUI to show Executed status
                // Try to find existing proposal and update it
                if let Some(mut tui_proposal) = self.bridge_status.find_proposal(&proposal_hash) {
                    tui_proposal.status = ProposalStatus::Executed;
                    tui_proposal.tx_hash = Some(tx_hash.clone());
                    tui_proposal.executed_at_block = Some(req.block_number);
                    self.bridge_status.update_proposal(tui_proposal);
                    info!(
                        target: "bridge.ingress",
                        proposal_hash = %proposal_hash,
                        "updated TUI proposal to Executed status"
                    );
                } else {
                    // Confirmation arrived before we processed the deposit locally
                    // Create a minimal placeholder proposal so TUI shows something
                    let placeholder = Proposal {
                        id: proposal_hash.clone(),
                        proposal_type: "deposit".to_string(),
                        description: "Confirmed via peer broadcast".to_string(),
                        signatures_collected: 3, // threshold reached
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
                    self.bridge_status.update_proposal(placeholder);
                    info!(
                        target: "bridge.ingress",
                        proposal_hash = %proposal_hash,
                        "created placeholder TUI proposal for early confirmation"
                    );
                }

                Ok(Response::new(ConfirmationBroadcastResponse {
                    accepted: true,
                }))
            }
            Err(e) => {
                warn!(
                    target: "bridge.ingress",
                    error = %e,
                    deposit_id = ?deposit_id,
                    "failed to mark proposal as confirmed"
                );
                Ok(Response::new(ConfirmationBroadcastResponse {
                    accepted: false,
                }))
            }
        }
    }

    async fn broadcast_stop(
        &self,
        request: Request<StopBroadcast>,
    ) -> Result<Response<StopBroadcastResponse>, Status> {
        let req = request.into_inner();

        let last_base_hash_src = match req.last_base_hash.as_ref() {
            Some(bytes) => bytes.as_slice(),
            None => &[],
        };
        if let Some(ref bytes) = req.last_base_hash {
            if bytes.len() != 40 {
                warn!(
                    target: "bridge.ingress",
                    len=bytes.len(),
                    "received stop broadcast with malformed last_base_hash; decoding lossy"
                );
            }
        }

        let last_nock_hash_src = match req.last_nock_hash.as_ref() {
            Some(bytes) => bytes.as_slice(),
            None => &[],
        };
        if let Some(ref bytes) = req.last_nock_hash {
            if bytes.len() != 40 {
                warn!(
                    target: "bridge.ingress",
                    len=bytes.len(),
                    "received stop broadcast with malformed last_nock_hash; decoding lossy"
                );
            }
        }

        let last = match (
            req.last_base_hash.as_ref(),
            req.last_base_height,
            req.last_nock_hash.as_ref(),
            req.last_nock_height,
        ) {
            (Some(_), Some(base_height), Some(_), Some(nock_height)) => {
                let mut last_base_hash_bytes = [0u8; 40];
                let base_copy_len =
                    std::cmp::min(last_base_hash_src.len(), last_base_hash_bytes.len());
                last_base_hash_bytes[..base_copy_len]
                    .copy_from_slice(&last_base_hash_src[..base_copy_len]);
                let base_hash =
                    crate::types::Tip5Hash::from_be_limb_bytes(&last_base_hash_bytes).ok();

                let mut last_nock_hash_bytes = [0u8; 40];
                let nock_copy_len =
                    std::cmp::min(last_nock_hash_src.len(), last_nock_hash_bytes.len());
                last_nock_hash_bytes[..nock_copy_len]
                    .copy_from_slice(&last_nock_hash_src[..nock_copy_len]);
                let nock_hash =
                    crate::types::Tip5Hash::from_be_limb_bytes(&last_nock_hash_bytes).ok();

                match (base_hash, nock_hash) {
                    (Some(base_hash), Some(nock_hash)) => Some(crate::types::StopLastBlocks {
                        base: crate::types::StopTipBase {
                            base_hash,
                            height: base_height,
                        },
                        nock: crate::types::StopTipNock {
                            nock_hash,
                            height: nock_height,
                        },
                    }),
                    _ => None,
                }
            }
            _ => None,
        };

        info!(
            target: "bridge.ingress",
            sender_node_id=req.sender_node_id,
            reason=%req.reason,
            // The timestamp here is purely informational and does not affect the stop process.
            timestamp=req.timestamp,
            "received stop broadcast"
        );

        self.trigger_stop(
            format!("peer {} requested stop: {}", req.sender_node_id, req.reason),
            last,
            crate::stop::StopSource::PeerBroadcast,
            false,
        )
        .await;

        Ok(Response::new(StopBroadcastResponse { accepted: true }))
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn serve_ingress(
    addr: SocketAddr,
    node_id: u64,
    runtime: Arc<BridgeRuntimeHandle>,
    status_state: BridgeStatusState,
    deposit_log: Arc<crate::deposit_log::DepositLog>,
    nonce_epoch: crate::config::NonceEpochConfig,
    signer: Arc<BridgeSigner>,
    proposal_cache: Arc<ProposalCache>,
    bridge_status: BridgeStatus,
    address_to_node_id: std::collections::HashMap<Address, u64>,
    stop_controller: crate::stop::StopController,
    peers: Vec<crate::health::PeerEndpoint>,
) -> Result<(), BridgeError> {
    info!(
        target: "bridge.ingress",
        %addr,
        "starting bridge ingress gRPC server"
    );
    let status_service = StatusService::new(
        status_state.clone(),
        deposit_log.clone(),
        nonce_epoch.clone(),
        bridge_status.clone(),
    );
    let tui_service = BridgeTuiService::new(
        bridge_status.clone(),
        status_state,
        deposit_log,
        nonce_epoch,
    )
    .await?;
    let reflection_service = ReflectionBuilder::configure()
        .register_encoded_file_descriptor_set(crate::grpc::FILE_DESCRIPTOR_SET)
        .build_v1()
        .map_err(|err| BridgeError::Runtime(format!("reflection build error: {}", err)))?;
    let service = IngressService::new(
        node_id, runtime, signer, proposal_cache, bridge_status, address_to_node_id,
        stop_controller, peers,
    );
    tonic::transport::Server::builder()
        .add_service(reflection_service)
        .add_service(BridgeIngressServer::new(service))
        .add_service(BridgeStatusServer::new(status_service))
        .add_service(BridgeTuiServer::new(tui_service))
        .serve(addr)
        .await
        .map_err(|err| BridgeError::Runtime(format!("ingress server error: {}", err)))
}

#[allow(clippy::too_many_arguments)]
pub async fn serve_ingress_with_shutdown(
    addr: SocketAddr,
    node_id: u64,
    runtime: Arc<BridgeRuntimeHandle>,
    status_state: BridgeStatusState,
    deposit_log: Arc<crate::deposit_log::DepositLog>,
    nonce_epoch: crate::config::NonceEpochConfig,
    signer: Arc<BridgeSigner>,
    proposal_cache: Arc<ProposalCache>,
    bridge_status: BridgeStatus,
    address_to_node_id: std::collections::HashMap<Address, u64>,
    stop_controller: crate::stop::StopController,
    peers: Vec<crate::health::PeerEndpoint>,
    shutdown: tokio::sync::oneshot::Receiver<()>,
) -> Result<(), BridgeError> {
    info!(
        target: "bridge.ingress",
        %addr,
        "starting bridge ingress gRPC server with shutdown"
    );
    let status_service = StatusService::new(
        status_state.clone(),
        deposit_log.clone(),
        nonce_epoch.clone(),
        bridge_status.clone(),
    );
    let tui_service = BridgeTuiService::new(
        bridge_status.clone(),
        status_state,
        deposit_log,
        nonce_epoch,
    )
    .await?;
    let reflection_service = ReflectionBuilder::configure()
        .register_encoded_file_descriptor_set(crate::grpc::FILE_DESCRIPTOR_SET)
        .build_v1()
        .map_err(|err| BridgeError::Runtime(format!("reflection build error: {}", err)))?;
    let service = IngressService::new(
        node_id, runtime, signer, proposal_cache, bridge_status, address_to_node_id,
        stop_controller, peers,
    );
    tonic::transport::Server::builder()
        .add_service(reflection_service)
        .add_service(BridgeIngressServer::new(service))
        .add_service(BridgeStatusServer::new(status_service))
        .add_service(BridgeTuiServer::new(tui_service))
        .serve_with_shutdown(addr, async move {
            let _ = shutdown.await;
        })
        .await
        .map_err(|err| BridgeError::Runtime(format!("ingress server error: {}", err)))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, RwLock};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use nockchain_types::v1::Name;
    use tempfile::TempDir;
    use tokio::time::{sleep, Instant};
    use tonic::Request;

    use super::*;
    use crate::config::NonceEpochConfig;
    use crate::deposit_log::{DepositLog, DepositLogEntry};
    use crate::health::SharedHealthState;
    use crate::proposal_cache::{
        PendingSignatureReport, ProposalCache, ProposalStatus, SignatureAddResult, SignatureData,
    };
    use crate::runtime::{
        BridgeEvent, BridgeRuntime, BridgeRuntimeHandle, CauseBuildOutcome, CauseBuilder,
        EventEnvelope,
    };
    use crate::types::{DepositId, EthAddress, NockDepositRequestData, Tip5Hash};

    // Test private key (same as in signing.rs tests)
    const TEST_PRIVATE_KEY: &str =
        "4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318";
    const TEST_PRIVATE_KEYS: [&str; 3] = [
        "4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318",
        "5c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362319",
        "6c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f36231a",
    ];

    fn test_bridge_signer() -> Arc<BridgeSigner> {
        Arc::new(
            BridgeSigner::new(format!("0x{}", TEST_PRIVATE_KEY))
                .expect("valid test key for BridgeSigner"),
        )
    }

    fn test_bridge_signer_for(key: &str) -> Arc<BridgeSigner> {
        Arc::new(BridgeSigner::new(format!("0x{}", key)).expect("valid test key for BridgeSigner"))
    }

    fn test_bridge_status() -> BridgeStatus {
        let health: SharedHealthState = Arc::new(RwLock::new(Vec::new()));
        BridgeStatus::new(health)
    }

    fn test_address_map() -> std::collections::HashMap<Address, u64> {
        std::collections::HashMap::new()
    }

    // Type synonym for test runtime setup
    type RuntimeTaskHandle = tokio::task::JoinHandle<Result<(), BridgeError>>;

    struct NoOpBuilder;

    impl CauseBuilder for NoOpBuilder {
        fn build_poke(
            &self,
            _event: &EventEnvelope<BridgeEvent>,
        ) -> Result<CauseBuildOutcome, BridgeError> {
            Ok(CauseBuildOutcome::Deferred("test".into()))
        }
    }

    fn make_runtime() -> (RuntimeTaskHandle, Arc<BridgeRuntimeHandle>) {
        let builder = Arc::new(NoOpBuilder);
        let (runtime, handle) = BridgeRuntime::new(builder);
        let runtime_handle = Arc::new(handle);
        let task = tokio::spawn(runtime.run());
        (task, runtime_handle)
    }

    struct TestNode {
        signer: Arc<BridgeSigner>,
        proposal_cache: Arc<ProposalCache>,
        bridge_status: BridgeStatus,
        deposit_log: Arc<DepositLog>,
        _data_dir: TempDir,
        _runtime_task: RuntimeTaskHandle,
        service: IngressService,
    }

    impl TestNode {
        async fn new(
            node_id: u64,
            signer: Arc<BridgeSigner>,
            address_map: HashMap<Address, u64>,
        ) -> Result<Self, BridgeError> {
            let (_runtime_task, runtime_handle) = make_runtime();
            let proposal_cache = Arc::new(ProposalCache::new());
            let bridge_status = test_bridge_status();
            let (stop_controller, _stop_handle) = crate::stop::StopController::new();
            let data_dir = tempfile::tempdir()
                .map_err(|e| BridgeError::Runtime(format!("failed to create temp dir: {}", e)))?;
            let deposit_log_path = data_dir.path().join("deposit-log.sqlite");
            let deposit_log = Arc::new(DepositLog::open(deposit_log_path).await?);
            let service = IngressService::new(
                node_id,
                runtime_handle,
                signer.clone(),
                proposal_cache.clone(),
                bridge_status.clone(),
                address_map,
                stop_controller,
                Vec::new(),
            );
            Ok(Self {
                signer,
                proposal_cache,
                bridge_status,
                deposit_log,
                _data_dir: data_dir,
                _runtime_task,
                service,
            })
        }

        async fn insert_entries(&self, entries: &[DepositLogEntry]) {
            for entry in entries {
                self.deposit_log.insert_entry(entry).await.unwrap();
            }
        }

        async fn build_request(
            &self,
            epoch: &NonceEpochConfig,
            next_nonce: u64,
        ) -> NockDepositRequestData {
            let mut records = self
                .deposit_log
                .records_from_nonce(next_nonce, 1, epoch)
                .await
                .expect("deposit log query failed");
            let (nonce, entry) = records.pop().expect("missing deposit log entry");
            NockDepositRequestData {
                tx_id: entry.tx_id,
                name: entry.name,
                recipient: entry.recipient,
                amount: entry.amount_to_mint,
                block_height: entry.block_height,
                as_of: entry.as_of,
                nonce,
            }
        }

        async fn sign_request(&self, req: &NockDepositRequestData) -> Vec<u8> {
            let hash = req.compute_proposal_hash();
            self.signer
                .sign_hash(&hash)
                .await
                .expect("signing failed")
                .as_bytes()
                .to_vec()
        }

        fn add_signature(
            &self,
            req: &NockDepositRequestData,
            signature: Vec<u8>,
            is_mine: bool,
        ) -> SignatureAddResult {
            let deposit_id = DepositId::from_effect_payload(req);
            let proposal_hash = req.compute_proposal_hash();
            self.proposal_cache
                .add_signature(
                    &deposit_id,
                    SignatureData {
                        signer_address: self.signer.address(),
                        signature,
                        proposal_hash,
                        is_mine,
                    },
                    Some(req.clone()),
                    IngressService::verify_signature,
                )
                .expect("failed to add signature")
        }

        fn apply_pending(&self, deposit_id: &DepositId) -> PendingSignatureReport {
            self.proposal_cache
                .apply_pending_signatures(deposit_id, IngressService::verify_signature)
                .expect("failed to apply pending signatures")
        }

        async fn broadcast_signature(&self, msg: SignatureBroadcast) -> bool {
            let response = self
                .service
                .broadcast_signature(Request::new(msg))
                .await
                .expect("broadcast failed");
            response.into_inner().accepted
        }
    }

    fn make_hash(seed: u64) -> Tip5Hash {
        Tip5Hash::from_limbs(&[seed, seed + 1, seed + 2, seed + 3, seed + 4])
    }

    fn make_entry(
        block_height: u64,
        seed: u64,
        recipient_byte: u8,
        amount: u64,
    ) -> DepositLogEntry {
        DepositLogEntry {
            block_height,
            tx_id: make_hash(seed),
            as_of: make_hash(seed + 1000),
            name: Name::new(make_hash(seed + 2000), make_hash(seed + 3000)),
            recipient: EthAddress([recipient_byte; 20]),
            amount_to_mint: amount,
        }
    }

    fn make_broadcast(
        deposit_id: &DepositId,
        proposal_hash: [u8; 32],
        signature: Vec<u8>,
        signer_address: Address,
    ) -> SignatureBroadcast {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        SignatureBroadcast {
            deposit_id: deposit_id.to_bytes(),
            proposal_hash: proposal_hash.to_vec(),
            signature,
            signer_address: signer_address.as_slice().to_vec(),
            timestamp,
        }
    }

    async fn wait_for_ready(node: &TestNode, deposit_id: &DepositId) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(Some(state)) = node.proposal_cache.get_state(deposit_id) {
                if state.status == ProposalStatus::Ready && state.has_threshold() {
                    return;
                }
            }
            if Instant::now() > deadline {
                panic!("proposal never reached Ready status");
            }
            sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn health_check_reports_node_details() -> Result<(), BridgeError> {
        let (_task, runtime_handle) = make_runtime();
        let cache = Arc::new(ProposalCache::new());
        let (stop_controller, _stop_handle) = crate::stop::StopController::new();
        let service = IngressService::new(
            3,
            runtime_handle.clone(),
            test_bridge_signer(),
            cache,
            test_bridge_status(),
            test_address_map(),
            stop_controller,
            vec![],
        );
        let response = service
            .health_check(Request::new(HealthCheckRequest {
                requester_node_id: 2,
                requester_address: "local-test".into(),
            }))
            .await
            .expect("health response");
        let body = response.get_ref();
        assert_eq!(body.responder_node_id, 3);
        assert_eq!(body.status, "healthy");
        assert!(body.uptime_millis < 5000);
        assert!(body.timestamp_millis > 0);
        Ok(())
    }

    #[test]
    fn stop_broadcast_fields_are_optional() {
        // Ensures proto regeneration worked and these fields are optional in Rust.
        let msg = StopBroadcast {
            sender_node_id: 1,
            reason: "x".into(),
            last_base_hash: None,
            last_base_height: None,
            last_nock_hash: None,
            last_nock_height: None,
            timestamp: 0,
        };
        assert!(msg.last_base_hash.is_none());
        assert!(msg.last_base_height.is_none());
        assert!(msg.last_nock_hash.is_none());
        assert!(msg.last_nock_height.is_none());
    }

    #[tokio::test]
    async fn multi_node_signature_convergence_out_of_order() -> Result<(), BridgeError> {
        let epoch = NonceEpochConfig {
            base: 0,
            start_height: 0,
            start_tx_id: None,
        };

        let signers: Vec<_> = TEST_PRIVATE_KEYS
            .iter()
            .map(|key| test_bridge_signer_for(key))
            .collect();
        let address_map: HashMap<Address, u64> = signers
            .iter()
            .enumerate()
            .map(|(idx, signer)| (signer.address(), idx as u64))
            .collect();

        let mut nodes = Vec::new();
        for (idx, signer) in signers.iter().enumerate() {
            nodes.push(TestNode::new(idx as u64, signer.clone(), address_map.clone()).await?);
        }

        let entry_a = make_entry(10, 1, 0x11, 1000);
        let entry_b = make_entry(11, 2, 0x22, 2000);

        nodes[0]
            .insert_entries(&[entry_b.clone(), entry_a.clone()])
            .await;
        nodes[1]
            .insert_entries(&[entry_a.clone(), entry_b.clone()])
            .await;
        nodes[2]
            .insert_entries(&[entry_a.clone(), entry_b.clone()])
            .await;

        let next_nonce = epoch.first_epoch_nonce();
        let req0 = nodes[0].build_request(&epoch, next_nonce).await;
        let req1 = nodes[1].build_request(&epoch, next_nonce).await;
        let req2 = nodes[2].build_request(&epoch, next_nonce).await;

        assert_eq!(req0.tx_id, entry_a.tx_id);
        assert_eq!(req0.compute_proposal_hash(), req1.compute_proposal_hash());
        assert_eq!(req0.compute_proposal_hash(), req2.compute_proposal_hash());

        let deposit_id = DepositId::from_effect_payload(&req0);
        let sig0 = nodes[0].sign_request(&req0).await;
        nodes[0].add_signature(&req0, sig0.clone(), true);

        let msg0 = make_broadcast(
            &deposit_id,
            req0.compute_proposal_hash(),
            sig0,
            nodes[0].signer.address(),
        );
        assert!(nodes[1].broadcast_signature(msg0.clone()).await);
        assert!(nodes[2].broadcast_signature(msg0).await);

        let sig1 = nodes[1].sign_request(&req1).await;
        nodes[1].add_signature(&req1, sig1.clone(), true);
        let report1 = nodes[1].apply_pending(&deposit_id);
        assert_eq!(report1.applied, 1);
        assert!(report1.mismatched.is_empty());

        let sig2 = nodes[2].sign_request(&req2).await;
        nodes[2].add_signature(&req2, sig2.clone(), true);
        let report2 = nodes[2].apply_pending(&deposit_id);
        assert_eq!(report2.applied, 1);
        assert!(report2.mismatched.is_empty());

        let msg1 = make_broadcast(
            &deposit_id,
            req1.compute_proposal_hash(),
            sig1,
            nodes[1].signer.address(),
        );
        assert!(nodes[0].broadcast_signature(msg1.clone()).await);
        assert!(nodes[2].broadcast_signature(msg1).await);

        let msg2 = make_broadcast(
            &deposit_id,
            req2.compute_proposal_hash(),
            sig2,
            nodes[2].signer.address(),
        );
        assert!(nodes[0].broadcast_signature(msg2.clone()).await);
        assert!(nodes[1].broadcast_signature(msg2).await);

        for node in nodes.iter() {
            wait_for_ready(node, &deposit_id).await;
        }

        Ok(())
    }

    #[tokio::test]
    async fn pending_signatures_refresh_tui_signers() -> Result<(), BridgeError> {
        // Simulate a peer signature arriving before we process the deposit,
        // then ensure TUI signers update once pending signatures are applied.
        let epoch = NonceEpochConfig {
            base: 0,
            start_height: 0,
            start_tx_id: None,
        };

        let self_signer = test_bridge_signer_for(TEST_PRIVATE_KEYS[0]);
        let peer_signer = test_bridge_signer_for(TEST_PRIVATE_KEYS[1]);
        let mut address_map = HashMap::new();
        address_map.insert(peer_signer.address(), 1);

        let node = TestNode::new(0, self_signer.clone(), address_map.clone()).await?;

        let entry = make_entry(10, 1, 0x11, 1000);
        node.insert_entries(std::slice::from_ref(&entry)).await;

        let next_nonce = epoch.first_epoch_nonce();
        let req = node.build_request(&epoch, next_nonce).await;
        let deposit_id = DepositId::from_effect_payload(&req);
        let proposal_hash = req.compute_proposal_hash();
        let proposal_hash_hex = encode(proposal_hash);

        // Queue a peer signature while the proposal is still unknown locally.
        let peer_sig = peer_signer
            .sign_hash(&proposal_hash)
            .await
            .expect("peer signing failed")
            .as_bytes()
            .to_vec();
        let queued = node
            .proposal_cache
            .add_signature(
                &deposit_id,
                SignatureData {
                    signer_address: peer_signer.address(),
                    signature: peer_sig,
                    proposal_hash,
                    is_mine: false,
                },
                None,
                IngressService::verify_signature,
            )
            .map_err(BridgeError::Runtime)?;
        assert_eq!(queued, SignatureAddResult::Added);

        // Seed the TUI with a proposal so we can verify the signers list later.
        node.bridge_status.update_proposal(Proposal {
            id: proposal_hash_hex.clone(),
            proposal_type: "deposit".to_string(),
            description: "pending signature refresh test".to_string(),
            signatures_collected: 0,
            signatures_required: crate::proposal_cache::SIGNATURE_THRESHOLD as u8,
            signers: Vec::new(),
            created_at: SystemTime::now(),
            status: crate::tui::types::ProposalStatus::Pending,
            data_hash: proposal_hash_hex.clone(),
            submitted_at_block: None,
            submitted_at: None,
            tx_hash: None,
            time_to_submit_ms: None,
            executed_at_block: None,
            source_block: Some(req.block_height),
            amount: Some(req.amount as u128),
            recipient: None,
            nonce: Some(req.nonce),
            source_tx_id: None,
            current_proposer: None,
            is_my_turn: false,
            time_until_takeover: None,
        });

        // Add our own signature, then apply the queued peer signature.
        let my_sig = node.sign_request(&req).await;
        let add_result = node.add_signature(&req, my_sig, true);
        assert!(matches!(
            add_result,
            SignatureAddResult::Added | SignatureAddResult::ThresholdReached
        ));

        let report = node.apply_pending(&deposit_id);
        assert_eq!(report.applied, 1);

        let cache_state = node
            .proposal_cache
            .get_state(&deposit_id)
            .expect("cache lookup failed")
            .expect("missing cache state");
        node.bridge_status
            .sync_proposal_signatures_from_cache(&proposal_hash_hex, &cache_state, &address_map, 0);

        // After syncing from cache, the TUI should show both signer ids.
        let proposal = node
            .bridge_status
            .find_proposal(&proposal_hash_hex)
            .expect("missing TUI proposal");
        assert_eq!(proposal.signatures_collected, 2);
        assert!(proposal.signers.contains(&0));
        assert!(proposal.signers.contains(&1));

        let bridge_status = node.bridge_status.proposals();
        let pending = bridge_status
            .pending_inbound
            .iter()
            .find(|p| p.id == proposal_hash_hex)
            .expect("proposal not in pending inbound");
        assert_eq!(pending.signatures_collected, 2);
        assert!(pending.signers.contains(&0));
        assert!(pending.signers.contains(&1));

        Ok(())
    }

    #[tokio::test]
    async fn nonce_divergence_alerts_on_mismatch() -> Result<(), BridgeError> {
        let epoch = NonceEpochConfig {
            base: 0,
            start_height: 0,
            start_tx_id: None,
        };

        let signers: Vec<_> = TEST_PRIVATE_KEYS
            .iter()
            .take(2)
            .map(|key| test_bridge_signer_for(key))
            .collect();
        let address_map: HashMap<Address, u64> = signers
            .iter()
            .enumerate()
            .map(|(idx, signer)| (signer.address(), idx as u64))
            .collect();

        let mut nodes = Vec::new();
        for (idx, signer) in signers.iter().enumerate() {
            nodes.push(TestNode::new(idx as u64, signer.clone(), address_map.clone()).await?);
        }

        let entry = make_entry(10, 42, 0x33, 3000);
        nodes[0].insert_entries(std::slice::from_ref(&entry)).await;
        nodes[1].insert_entries(std::slice::from_ref(&entry)).await;

        let next_nonce = epoch.first_epoch_nonce();
        let req0 = nodes[0].build_request(&epoch, next_nonce).await;
        let mut req1 = nodes[1].build_request(&epoch, next_nonce).await;
        req1.nonce += 1;

        let sig1 = nodes[1].sign_request(&req1).await;
        nodes[1].add_signature(&req1, sig1, true);

        let deposit_id = DepositId::from_effect_payload(&req0);
        let sig0 = nodes[0].sign_request(&req0).await;
        let msg0 = make_broadcast(
            &deposit_id,
            req0.compute_proposal_hash(),
            sig0,
            nodes[0].signer.address(),
        );

        let accepted = nodes[1].broadcast_signature(msg0).await;
        assert!(!accepted, "mismatched proposal hash should be rejected");

        let has_divergence = {
            let alerts = nodes[1]
                .bridge_status
                .alerts
                .read()
                .expect("alert lock poisoned");
            alerts
                .alerts
                .iter()
                .any(|alert| alert.source == "nonce-divergence")
        };
        assert!(has_divergence, "expected nonce divergence alert");

        Ok(())
    }
}
