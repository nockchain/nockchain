use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use alloy::primitives::Address;
use tonic::{async_trait, Request, Response, Status};
use tonic_reflection::server::Builder as ReflectionBuilder;
use tracing::{debug, info, warn};

use crate::deposit::cache::ProposalCache as DepositProposalCache;
use crate::observability::status::{BridgeStatus, BridgeStatusState, StatusService};
use crate::observability::tui_api::proto::bridge_tui_server::BridgeTuiServer;
use crate::observability::tui_api::{BridgeTuiService, WithdrawalTuiSource};
use crate::shared::errors::BridgeError;
use crate::shared::runtime::BridgeRuntimeHandle;
use crate::shared::signing::BridgeSigner;
use crate::shared::stop::StopHandle;
use crate::withdrawal::transport::WithdrawalProposalTransport;

pub mod proto {
    #[cfg(feature = "bazel_build")]
    include!(env!("BRIDGE_INGRESS_PROTO_RS"));

    #[cfg(not(feature = "bazel_build"))]
    tonic::include_proto!("bridge.ingress.v1");
}

use proto::bridge_ingress_server::{BridgeIngress, BridgeIngressServer};
use proto::{
    CanonicalWithdrawalProposalBroadcast, CanonicalWithdrawalProposalBroadcastResponse,
    ConfirmationBroadcast, ConfirmationBroadcastResponse, DepositConfirmationBroadcast,
    DepositConfirmationBroadcastResponse, DepositProposalStatusRequest,
    DepositProposalStatusResponse, DepositSignatureBroadcast, DepositSignatureBroadcastResponse,
    HealthCheckRequest, HealthCheckResponse, ProposalStatusRequest, ProposalStatusResponse,
    SignatureBroadcast, SignatureBroadcastResponse, SignedWithdrawalProposalBroadcast,
    SignedWithdrawalProposalBroadcastResponse, StopBroadcast, StopBroadcastResponse,
    WithdrawalProposalBroadcast, WithdrawalProposalBroadcastResponse,
    WithdrawalProposalStatusRequest, WithdrawalProposalStatusResponse,
};

use crate::observability::status::proto::bridge_status_server::BridgeStatusServer;

/// Broadcasts a stop message to all configured peer bridges in a fire-and-
/// forget fashion.
pub fn spawn_broadcast_stop_to_peers(
    peers: &[crate::observability::health::PeerEndpoint],
    msg: StopBroadcast,
    component: &'static str,
) {
    use tracing::{info, warn};

    use crate::shared::ingress::proto::bridge_ingress_client::BridgeIngressClient;

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

struct IngressRuntimeDeps {
    runtime: Arc<BridgeRuntimeHandle>,
    /// Signer for creating Ethereum signatures on deposit proposals.
    deposit_signer: Arc<BridgeSigner>,
    /// Cache for aggregating deposit signatures from multiple bridge nodes.
    deposit_proposal_cache: Arc<DepositProposalCache>,
    /// Optional withdrawal proposal transport/canonicalization surface.
    withdrawal_transport: Option<Arc<WithdrawalProposalTransport>>,
}

struct IngressNodeState {
    node_id: u64,
    start_time: Instant,
}

struct IngressControl {
    /// Shared TUI state for updating proposal display on peer broadcasts
    bridge_status: BridgeStatus,
    stop_controller: crate::shared::stop::StopController,
}

struct IngressPeerState {
    /// Mapping from deposit signer address to node ID for TUI signature display.
    deposit_signer_address_to_node_id: Arc<std::collections::HashMap<Address, u64>>,
    peers: Arc<Vec<crate::observability::health::PeerEndpoint>>,
}

pub struct IngressService {
    deps: IngressRuntimeDeps,
    node: IngressNodeState,
    control: IngressControl,
    peers: IngressPeerState,
}

impl IngressService {
    #[allow(clippy::too_many_arguments)]
    /// Builds the ingress service with the runtime, signer, status, and peer
    /// dependencies required by the gRPC handlers.
    pub(crate) fn new(
        node_id: u64,
        runtime: Arc<BridgeRuntimeHandle>,
        deposit_signer: Arc<BridgeSigner>,
        deposit_proposal_cache: Arc<DepositProposalCache>,
        bridge_status: BridgeStatus,
        deposit_signer_address_to_node_id: std::collections::HashMap<Address, u64>,
        stop_controller: crate::shared::stop::StopController,
        peers: Vec<crate::observability::health::PeerEndpoint>,
    ) -> Self {
        Self {
            deps: IngressRuntimeDeps {
                runtime,
                deposit_signer,
                deposit_proposal_cache,
                withdrawal_transport: None,
            },
            node: IngressNodeState {
                node_id,
                start_time: Instant::now(),
            },
            control: IngressControl {
                bridge_status,
                stop_controller,
            },
            peers: IngressPeerState {
                deposit_signer_address_to_node_id: Arc::new(deposit_signer_address_to_node_id),
                peers: Arc::new(peers),
            },
        }
    }

    #[allow(dead_code)]
    /// Attaches the withdrawal transport so ingress can route withdrawal
    /// proposal broadcasts directly into the withdrawal runtime.
    pub(crate) fn with_withdrawal_transport(
        mut self,
        withdrawal_transport: Arc<WithdrawalProposalTransport>,
    ) -> Self {
        self.deps.withdrawal_transport = Some(withdrawal_transport);
        self
    }

    pub(crate) fn node_id(&self) -> u64 {
        self.node.node_id
    }

    pub(crate) fn deposit_signer(&self) -> &Arc<BridgeSigner> {
        &self.deps.deposit_signer
    }

    pub(crate) fn deposit_proposal_cache(&self) -> &Arc<DepositProposalCache> {
        &self.deps.deposit_proposal_cache
    }

    pub(crate) fn bridge_status(&self) -> &BridgeStatus {
        &self.control.bridge_status
    }

    pub(crate) fn deposit_signer_address_to_node_id(
        &self,
    ) -> &std::collections::HashMap<Address, u64> {
        self.peers.deposit_signer_address_to_node_id.as_ref()
    }

    pub(crate) fn withdrawal_transport(&self) -> Option<&Arc<WithdrawalProposalTransport>> {
        self.deps.withdrawal_transport.as_ref()
    }

    /// Returns the ingress service uptime in milliseconds for health probes.
    fn uptime_millis(&self) -> u64 {
        self.node.start_time.elapsed().as_millis() as u64
    }

    /// Triggers a local stop and optionally broadcasts that stop to peers.
    async fn trigger_stop(
        &self,
        reason: String,
        last: Option<crate::shared::types::StopLastBlocks>,
        source: crate::shared::stop::StopSource,
        broadcast: bool,
    ) {
        use std::time::{SystemTime, UNIX_EPOCH};

        use tracing::warn;

        use crate::observability::tui::types::AlertSeverity;
        use crate::shared::stop::{StopInfo, StopSource};

        let resolved_last = match last {
            Some(last) => Some(last),
            None => match self.deps.runtime.peek_stop_info().await {
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

        if !self.control.stop_controller.trigger(info) {
            return;
        }

        self.control.bridge_status.push_alert(
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
            if let Err(err) = self.deps.runtime.send_stop(last).await {
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
            sender_node_id: self.node.node_id,
            reason: reason.clone(),
            last_base_hash,
            last_base_height,
            last_nock_hash,
            last_nock_height,
            timestamp,
        };

        spawn_broadcast_stop_to_peers(self.peers.peers.as_ref(), msg, "bridge.ingress");
    }
}

#[async_trait]
impl BridgeIngress for IngressService {
    /// Returns the local bridge health snapshot for peer liveness checks.
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
            responder_node_id: self.node.node_id,
            uptime_millis: self.uptime_millis(),
            status: "healthy".into(),
            timestamp_millis,
        };
        Ok(Response::new(response))
    }

    /// Handles inbound deposit-signature broadcasts from peer bridges.
    async fn broadcast_deposit_signature(
        &self,
        request: Request<DepositSignatureBroadcast>,
    ) -> Result<Response<DepositSignatureBroadcastResponse>, Status> {
        crate::deposit::ingress::broadcast_deposit_signature(self, request).await
    }

    /// Returns the local deposit proposal status used by peer signature convergence.
    async fn get_deposit_proposal_status(
        &self,
        request: Request<DepositProposalStatusRequest>,
    ) -> Result<Response<DepositProposalStatusResponse>, Status> {
        crate::deposit::ingress::get_deposit_proposal_status(self, request).await
    }

    /// Handles inbound prepared-proposal broadcasts from peer bridges.
    async fn broadcast_withdrawal_proposal(
        &self,
        request: Request<WithdrawalProposalBroadcast>,
    ) -> Result<Response<WithdrawalProposalBroadcastResponse>, Status> {
        crate::withdrawal::ingress::broadcast_withdrawal_proposal(self, request).await
    }

    /// Handles inbound signed-proposal broadcasts from peer bridges.
    async fn broadcast_signed_withdrawal_proposal(
        &self,
        request: Request<SignedWithdrawalProposalBroadcast>,
    ) -> Result<Response<SignedWithdrawalProposalBroadcastResponse>, Status> {
        crate::withdrawal::ingress::broadcast_signed_withdrawal_proposal(self, request).await
    }

    /// Handles inbound peer-canonical proposal broadcasts from peer bridges.
    async fn broadcast_canonical_withdrawal_proposal(
        &self,
        request: Request<CanonicalWithdrawalProposalBroadcast>,
    ) -> Result<Response<CanonicalWithdrawalProposalBroadcastResponse>, Status> {
        crate::withdrawal::ingress::broadcast_canonical_withdrawal_proposal(self, request).await
    }

    /// Returns the locally persisted status for one withdrawal proposal epoch.
    async fn get_withdrawal_proposal_status(
        &self,
        request: Request<WithdrawalProposalStatusRequest>,
    ) -> Result<Response<WithdrawalProposalStatusResponse>, Status> {
        crate::withdrawal::ingress::get_withdrawal_proposal_status(self, request).await
    }

    /// Handles a deposit confirmation broadcast from the node that posted to BASE.
    /// This allows non-proposer nodes to mark proposals as confirmed and stop
    /// waiting for their failover turn.
    async fn broadcast_deposit_confirmation(
        &self,
        request: Request<DepositConfirmationBroadcast>,
    ) -> Result<Response<DepositConfirmationBroadcastResponse>, Status> {
        crate::deposit::ingress::broadcast_deposit_confirmation(self, request).await
    }

    // Legacy deposit RPC names kept only for rolling deploy compatibility with
    // bridge binaries that predate the deposit-prefixed endpoint names. Each
    // handler delegates to the current deposit validation path.
    async fn broadcast_signature(
        &self,
        request: Request<SignatureBroadcast>,
    ) -> Result<Response<SignatureBroadcastResponse>, Status> {
        let req = request.into_inner();
        let response = crate::deposit::ingress::broadcast_deposit_signature(
            self,
            Request::new(DepositSignatureBroadcast {
                deposit_id: req.deposit_id,
                proposal_hash: req.proposal_hash,
                signature: req.signature,
                signer_address: req.signer_address,
                timestamp: req.timestamp,
            }),
        )
        .await?
        .into_inner();

        Ok(Response::new(SignatureBroadcastResponse {
            accepted: response.accepted,
            error: response.error,
        }))
    }

    async fn get_proposal_status(
        &self,
        request: Request<ProposalStatusRequest>,
    ) -> Result<Response<ProposalStatusResponse>, Status> {
        let req = request.into_inner();
        let response = crate::deposit::ingress::get_deposit_proposal_status(
            self,
            Request::new(DepositProposalStatusRequest {
                deposit_id: req.deposit_id,
            }),
        )
        .await?
        .into_inner();

        Ok(Response::new(ProposalStatusResponse {
            status: response.status,
            signature_count: response.signature_count,
            signers: response.signers,
            tx_hash: response.tx_hash,
        }))
    }

    /// Compatibility handler for the pre-rename deposit confirmation RPC.
    async fn broadcast_confirmation(
        &self,
        request: Request<ConfirmationBroadcast>,
    ) -> Result<Response<ConfirmationBroadcastResponse>, Status> {
        let req = request.into_inner();
        let response = crate::deposit::ingress::broadcast_deposit_confirmation(
            self,
            Request::new(DepositConfirmationBroadcast {
                deposit_id: req.deposit_id,
                proposal_hash: req.proposal_hash,
                tx_hash: req.tx_hash,
                block_number: req.block_number,
                timestamp: req.timestamp,
            }),
        )
        .await?
        .into_inner();

        Ok(Response::new(ConfirmationBroadcastResponse {
            accepted: response.accepted,
        }))
    }

    /// Handles an inbound peer stop request.
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
                    crate::shared::types::Tip5Hash::from_be_limb_bytes(&last_base_hash_bytes).ok();

                let mut last_nock_hash_bytes = [0u8; 40];
                let nock_copy_len =
                    std::cmp::min(last_nock_hash_src.len(), last_nock_hash_bytes.len());
                last_nock_hash_bytes[..nock_copy_len]
                    .copy_from_slice(&last_nock_hash_src[..nock_copy_len]);
                let nock_hash =
                    crate::shared::types::Tip5Hash::from_be_limb_bytes(&last_nock_hash_bytes).ok();

                match (base_hash, nock_hash) {
                    (Some(base_hash), Some(nock_hash)) => {
                        Some(crate::shared::types::StopLastBlocks {
                            base: crate::shared::types::StopTipBase {
                                base_hash,
                                height: base_height,
                            },
                            nock: crate::shared::types::StopTipNock {
                                nock_hash,
                                height: nock_height,
                            },
                        })
                    }
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
            crate::shared::stop::StopSource::PeerBroadcast,
            false,
        )
        .await;

        Ok(Response::new(StopBroadcastResponse { accepted: true }))
    }
}

#[allow(clippy::too_many_arguments)]
/// Serves the bridge ingress gRPC surface until the process exits.
pub async fn serve_ingress(
    addr: SocketAddr,
    node_id: u64,
    runtime: Arc<BridgeRuntimeHandle>,
    status_state: BridgeStatusState,
    deposit_log: Arc<crate::deposit::log::DepositLog>,
    nonce_epoch: crate::shared::config::NonceEpochConfig,
    deposit_signer: Arc<BridgeSigner>,
    deposit_proposal_cache: Arc<DepositProposalCache>,
    bridge_status: BridgeStatus,
    deposit_signer_address_to_node_id: std::collections::HashMap<Address, u64>,
    stop_controller: crate::shared::stop::StopController,
    stop_handle: StopHandle,
    peers: Vec<crate::observability::health::PeerEndpoint>,
    withdrawal_transport: Option<Arc<WithdrawalProposalTransport>>,
    withdrawal_tui_source: Option<WithdrawalTuiSource>,
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
        stop_handle,
    );
    let tui_service = BridgeTuiService::new(
        bridge_status.clone(),
        status_state,
        deposit_log,
        nonce_epoch,
        withdrawal_tui_source,
    )
    .await?;
    let reflection_service = ReflectionBuilder::configure()
        .register_encoded_file_descriptor_set(crate::shared::grpc::FILE_DESCRIPTOR_SET)
        .build_v1()
        .map_err(|err| BridgeError::Runtime(format!("reflection build error: {}", err)))?;
    let service = IngressService::new(
        node_id, runtime, deposit_signer, deposit_proposal_cache, bridge_status,
        deposit_signer_address_to_node_id, stop_controller, peers,
    );
    let service = match withdrawal_transport.clone() {
        Some(transport) => service.with_withdrawal_transport(transport),
        None => service,
    };
    let builder = tonic::transport::Server::builder()
        .add_service(reflection_service)
        .add_service(BridgeIngressServer::new(service))
        .add_service(BridgeStatusServer::new(status_service))
        .add_service(BridgeTuiServer::new(tui_service));
    builder
        .serve(addr)
        .await
        .map_err(|err| BridgeError::Runtime(format!("ingress server error: {}", err)))
}

#[allow(clippy::too_many_arguments)]
/// Serves the bridge ingress gRPC surface with an explicit shutdown signal.
pub async fn serve_ingress_with_shutdown(
    addr: SocketAddr,
    node_id: u64,
    runtime: Arc<BridgeRuntimeHandle>,
    status_state: BridgeStatusState,
    deposit_log: Arc<crate::deposit::log::DepositLog>,
    nonce_epoch: crate::shared::config::NonceEpochConfig,
    deposit_signer: Arc<BridgeSigner>,
    deposit_proposal_cache: Arc<DepositProposalCache>,
    bridge_status: BridgeStatus,
    deposit_signer_address_to_node_id: std::collections::HashMap<Address, u64>,
    stop_controller: crate::shared::stop::StopController,
    stop_handle: StopHandle,
    peers: Vec<crate::observability::health::PeerEndpoint>,
    withdrawal_transport: Option<Arc<WithdrawalProposalTransport>>,
    withdrawal_tui_source: Option<WithdrawalTuiSource>,
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
        stop_handle,
    );
    let tui_service = BridgeTuiService::new(
        bridge_status.clone(),
        status_state,
        deposit_log,
        nonce_epoch,
        withdrawal_tui_source,
    )
    .await?;
    let reflection_service = ReflectionBuilder::configure()
        .register_encoded_file_descriptor_set(crate::shared::grpc::FILE_DESCRIPTOR_SET)
        .build_v1()
        .map_err(|err| BridgeError::Runtime(format!("reflection build error: {}", err)))?;
    let service = IngressService::new(
        node_id, runtime, deposit_signer, deposit_proposal_cache, bridge_status,
        deposit_signer_address_to_node_id, stop_controller, peers,
    );
    let service = match withdrawal_transport.clone() {
        Some(transport) => service.with_withdrawal_transport(transport),
        None => service,
    };
    let builder = tonic::transport::Server::builder()
        .add_service(reflection_service)
        .add_service(BridgeIngressServer::new(service))
        .add_service(BridgeStatusServer::new(status_service))
        .add_service(BridgeTuiServer::new(tui_service));
    builder
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

    use hex::encode;
    use nockchain_types::v1::Name;
    use tempfile::TempDir;
    use tokio::time::{sleep, Instant};
    use tonic::Request;

    use super::*;
    use crate::deposit::cache::{
        PendingSignatureReport, ProposalCache as DepositProposalCache,
        ProposalStatus as DepositProposalStatus, SignatureAddResult as DepositSignatureAddResult,
        SignatureData as DepositSignatureData,
    };
    use crate::deposit::log::{DepositLog, DepositLogEntry};
    use crate::deposit::types::{DepositId, NockDepositRequestData};
    use crate::observability::health::SharedHealthState;
    use crate::observability::tui::types::Proposal;
    use crate::shared::config::NonceEpochConfig;
    use crate::shared::runtime::{
        BridgeEvent, BridgeRuntime, BridgeRuntimeHandle, CauseBuildOutcome, CauseBuilder,
        EventEnvelope,
    };
    use crate::shared::types::{EthAddress, Tip5Hash};

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
        deposit_signer: Arc<BridgeSigner>,
        deposit_proposal_cache: Arc<DepositProposalCache>,
        bridge_status: BridgeStatus,
        deposit_log: Arc<DepositLog>,
        _data_dir: TempDir,
        _runtime_task: RuntimeTaskHandle,
        service: IngressService,
    }

    impl TestNode {
        async fn new(
            node_id: u64,
            deposit_signer: Arc<BridgeSigner>,
            address_map: HashMap<Address, u64>,
        ) -> Result<Self, BridgeError> {
            let (_runtime_task, runtime_handle) = make_runtime();
            let deposit_proposal_cache = Arc::new(DepositProposalCache::new());
            let bridge_status = test_bridge_status();
            let (stop_controller, _stop_handle) = crate::shared::stop::StopController::new();
            let data_dir = tempfile::tempdir()
                .map_err(|e| BridgeError::Runtime(format!("failed to create temp dir: {}", e)))?;
            let deposit_log_path = data_dir.path().join("deposit-log.sqlite");
            let deposit_log = Arc::new(DepositLog::open(deposit_log_path).await?);
            let service = IngressService::new(
                node_id,
                runtime_handle,
                deposit_signer.clone(),
                deposit_proposal_cache.clone(),
                bridge_status.clone(),
                address_map,
                stop_controller,
                Vec::new(),
            );
            Ok(Self {
                deposit_signer,
                deposit_proposal_cache,
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

        async fn build_deposit_request(
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

        async fn sign_deposit_request(&self, req: &NockDepositRequestData) -> Vec<u8> {
            let hash = req.compute_proposal_hash();
            self.deposit_signer
                .sign_hash(&hash)
                .await
                .expect("signing failed")
                .as_bytes()
                .to_vec()
        }

        fn add_deposit_signature(
            &self,
            req: &NockDepositRequestData,
            signature: Vec<u8>,
            is_mine: bool,
        ) -> DepositSignatureAddResult {
            let deposit_id = DepositId::from_effect_payload(req);
            let proposal_hash = req.compute_proposal_hash();
            self.deposit_proposal_cache
                .add_signature(
                    &deposit_id,
                    DepositSignatureData {
                        signer_address: self.deposit_signer.address(),
                        signature,
                        proposal_hash,
                        is_mine,
                    },
                    Some(req.clone()),
                    crate::deposit::ingress::verify_deposit_signature,
                )
                .expect("failed to add signature")
        }

        fn apply_pending_deposit_signatures(
            &self,
            deposit_id: &DepositId,
        ) -> PendingSignatureReport {
            self.deposit_proposal_cache
                .apply_pending_signatures(
                    deposit_id,
                    crate::deposit::ingress::verify_deposit_signature,
                )
                .expect("failed to apply pending signatures")
        }

        async fn broadcast_deposit_signature(&self, msg: DepositSignatureBroadcast) -> bool {
            let response = self
                .service
                .broadcast_deposit_signature(Request::new(msg))
                .await
                .expect("broadcast failed");
            response.into_inner().accepted
        }

        async fn broadcast_legacy_signature(&self, msg: SignatureBroadcast) -> bool {
            let response = self
                .service
                .broadcast_signature(Request::new(msg))
                .await
                .expect("legacy broadcast failed");
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

    fn make_deposit_signature_broadcast(
        deposit_id: &DepositId,
        proposal_hash: [u8; 32],
        signature: Vec<u8>,
        signer_address: Address,
    ) -> DepositSignatureBroadcast {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        DepositSignatureBroadcast {
            deposit_id: deposit_id.to_bytes(),
            proposal_hash: proposal_hash.to_vec(),
            signature,
            signer_address: signer_address.as_slice().to_vec(),
            timestamp,
        }
    }

    fn make_legacy_signature_broadcast(msg: DepositSignatureBroadcast) -> SignatureBroadcast {
        SignatureBroadcast {
            deposit_id: msg.deposit_id,
            proposal_hash: msg.proposal_hash,
            signature: msg.signature,
            signer_address: msg.signer_address,
            timestamp: msg.timestamp,
        }
    }

    async fn wait_for_ready(node: &TestNode, deposit_id: &DepositId) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(Some(state)) = node.deposit_proposal_cache.get_state(deposit_id) {
                if state.status == DepositProposalStatus::Ready && state.has_threshold() {
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
        let cache = Arc::new(DepositProposalCache::new());
        let (stop_controller, _stop_handle) = crate::shared::stop::StopController::new();
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
        let req0 = nodes[0].build_deposit_request(&epoch, next_nonce).await;
        let req1 = nodes[1].build_deposit_request(&epoch, next_nonce).await;
        let req2 = nodes[2].build_deposit_request(&epoch, next_nonce).await;

        assert_eq!(req0.tx_id, entry_a.tx_id);
        assert_eq!(req0.compute_proposal_hash(), req1.compute_proposal_hash());
        assert_eq!(req0.compute_proposal_hash(), req2.compute_proposal_hash());

        let deposit_id = DepositId::from_effect_payload(&req0);
        let sig0 = nodes[0].sign_deposit_request(&req0).await;
        nodes[0].add_deposit_signature(&req0, sig0.clone(), true);

        let msg0 = make_deposit_signature_broadcast(
            &deposit_id,
            req0.compute_proposal_hash(),
            sig0,
            nodes[0].deposit_signer.address(),
        );
        assert!(nodes[1].broadcast_deposit_signature(msg0.clone()).await);
        assert!(nodes[2].broadcast_deposit_signature(msg0).await);

        let sig1 = nodes[1].sign_deposit_request(&req1).await;
        nodes[1].add_deposit_signature(&req1, sig1.clone(), true);
        let report1 = nodes[1].apply_pending_deposit_signatures(&deposit_id);
        assert_eq!(report1.applied, 1);
        assert!(report1.mismatched.is_empty());

        let sig2 = nodes[2].sign_deposit_request(&req2).await;
        nodes[2].add_deposit_signature(&req2, sig2.clone(), true);
        let report2 = nodes[2].apply_pending_deposit_signatures(&deposit_id);
        assert_eq!(report2.applied, 1);
        assert!(report2.mismatched.is_empty());

        let msg1 = make_deposit_signature_broadcast(
            &deposit_id,
            req1.compute_proposal_hash(),
            sig1,
            nodes[1].deposit_signer.address(),
        );
        assert!(nodes[0].broadcast_deposit_signature(msg1.clone()).await);
        assert!(nodes[2].broadcast_deposit_signature(msg1).await);

        let msg2 = make_deposit_signature_broadcast(
            &deposit_id,
            req2.compute_proposal_hash(),
            sig2,
            nodes[2].deposit_signer.address(),
        );
        assert!(nodes[0].broadcast_deposit_signature(msg2.clone()).await);
        assert!(nodes[1].broadcast_deposit_signature(msg2).await);

        for node in nodes.iter() {
            wait_for_ready(node, &deposit_id).await;
        }

        Ok(())
    }

    #[tokio::test]
    async fn legacy_signature_broadcast_delegates_to_deposit_signature_path(
    ) -> Result<(), BridgeError> {
        let sender = test_bridge_signer_for(TEST_PRIVATE_KEYS[0]);
        let receiver = test_bridge_signer_for(TEST_PRIVATE_KEYS[1]);
        let address_map = HashMap::from([(sender.address(), 0), (receiver.address(), 1)]);
        let node = TestNode::new(1, receiver.clone(), address_map).await?;

        let entry = make_entry(10, 1, 0x11, 1000);
        let req = NockDepositRequestData {
            tx_id: entry.tx_id,
            name: entry.name,
            recipient: entry.recipient,
            amount: entry.amount_to_mint,
            block_height: entry.block_height,
            as_of: entry.as_of,
            nonce: 0,
        };
        let deposit_id = DepositId::from_effect_payload(&req);

        let receiver_sig = node.sign_deposit_request(&req).await;
        node.add_deposit_signature(&req, receiver_sig, true);

        let sender_sig = sender
            .sign_hash(&req.compute_proposal_hash())
            .await
            .expect("sender signing failed")
            .as_bytes()
            .to_vec();
        let msg = make_deposit_signature_broadcast(
            &deposit_id,
            req.compute_proposal_hash(),
            sender_sig,
            sender.address(),
        );

        assert!(
            node.broadcast_legacy_signature(make_legacy_signature_broadcast(msg))
                .await
        );

        let state = node
            .deposit_proposal_cache
            .get_state(&deposit_id)
            .expect("cache read failed")
            .expect("proposal missing");
        assert!(state.peer_signatures.contains_key(&sender.address()));

        let status = node
            .service
            .get_proposal_status(Request::new(ProposalStatusRequest {
                deposit_id: deposit_id.to_bytes(),
            }))
            .await
            .expect("legacy status failed")
            .into_inner();
        assert_eq!(status.signature_count, 2);

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
        let req = node.build_deposit_request(&epoch, next_nonce).await;
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
            .deposit_proposal_cache
            .add_signature(
                &deposit_id,
                DepositSignatureData {
                    signer_address: peer_signer.address(),
                    signature: peer_sig,
                    proposal_hash,
                    is_mine: false,
                },
                None,
                crate::deposit::ingress::verify_deposit_signature,
            )
            .map_err(BridgeError::Runtime)?;
        assert_eq!(queued, DepositSignatureAddResult::Added);

        // Seed the TUI with a proposal so we can verify the signers list later.
        node.bridge_status.update_proposal(Proposal {
            id: proposal_hash_hex.clone(),
            proposal_type: "deposit".to_string(),
            description: "pending signature refresh test".to_string(),
            signatures_collected: 0,
            signatures_required: crate::deposit::cache::SIGNATURE_THRESHOLD as u8,
            signers: Vec::new(),
            created_at: SystemTime::now(),
            status: crate::observability::tui::types::ProposalStatus::Pending,
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
        let my_sig = node.sign_deposit_request(&req).await;
        let add_result = node.add_deposit_signature(&req, my_sig, true);
        assert!(matches!(
            add_result,
            DepositSignatureAddResult::Added | DepositSignatureAddResult::ThresholdReached
        ));

        let report = node.apply_pending_deposit_signatures(&deposit_id);
        assert_eq!(report.applied, 1);

        let cache_state = node
            .deposit_proposal_cache
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
        let req0 = nodes[0].build_deposit_request(&epoch, next_nonce).await;
        let mut req1 = nodes[1].build_deposit_request(&epoch, next_nonce).await;
        req1.nonce += 1;

        let sig1 = nodes[1].sign_deposit_request(&req1).await;
        nodes[1].add_deposit_signature(&req1, sig1, true);

        let deposit_id = DepositId::from_effect_payload(&req0);
        let sig0 = nodes[0].sign_deposit_request(&req0).await;
        let msg0 = make_deposit_signature_broadcast(
            &deposit_id,
            req0.compute_proposal_hash(),
            sig0,
            nodes[0].deposit_signer.address(),
        );

        let accepted = nodes[1].broadcast_deposit_signature(msg0).await;
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
