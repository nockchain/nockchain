use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use nockapp::NockAppError;
use serde_bytes::ByteBuf;
use tokio::sync::{mpsc, Mutex};
use tokio::time::Duration;
use tracing::{error, trace, warn};

use crate::driver::gen2;
use crate::messages::{NockchainRequest, NockchainResponse};
use crate::metrics::NockchainP2PMetrics;
use crate::p2p_state::{OutboundRequestContext, P2PState};
use crate::p2p_util::{log_fail2ban_ipv4, log_fail2ban_ipv6};
use crate::peer_policy::ExclusionOutcome;
use crate::tracked_join_set::TrackedJoinSet;
use crate::traffic_cop;
use crate::transport::TransportCommand;
use crate::types::{InboundRequestId, NodeId as PeerId};

#[derive(Debug)]
pub(crate) enum SwarmAction {
    SendResponse {
        id: InboundRequestId,
        response: NockchainResponse,
    },
    FlushDeferredHeardBlocks,
    QueueKernelRequest {
        peer_id: PeerId,
        request_message: ByteBuf,
    },
    SendRequest {
        peer_id: PeerId,
        request: NockchainRequest,
        request_context: Option<OutboundRequestContext>,
    },
    SendGossip {
        peer_id: PeerId,
        message: ByteBuf,
    },
    RetryRequests {
        requests: Vec<OutboundRequestContext>,
        delay: Duration,
    },
    BlockPeer {
        peer_id: PeerId,
    },
    RecordExclusionOutcome {
        outcome: ExclusionOutcome,
        related_peers: Vec<PeerId>,
    },
}

pub(super) enum SwarmActionDispatcher<'a> {
    Buffered(&'a mut VecDeque<SwarmAction>),
    Channel(&'a mpsc::Sender<SwarmAction>),
}

impl SwarmActionDispatcher<'_> {
    pub(super) async fn dispatch(
        &mut self,
        action: SwarmAction,
    ) -> Result<(), mpsc::error::SendError<SwarmAction>> {
        match self {
            SwarmActionDispatcher::Buffered(buffered) => {
                buffered.push_back(action);
                Ok(())
            }
            SwarmActionDispatcher::Channel(sender) => sender.send(action).await,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn process_swarm_action(
    swarm_action: SwarmAction,
    local_peer_id: PeerId,
    transport_tx: &mpsc::Sender<TransportCommand>,
    next_request_id: &mut u64,
    buffered_swarm_actions: &mut VecDeque<SwarmAction>,
    swarm_tx: &mpsc::Sender<SwarmAction>,
    join_set: &mut TrackedJoinSet<Result<(), NockAppError>>,
    driver_state: &Arc<Mutex<P2PState>>,
    metrics: &Arc<NockchainP2PMetrics>,
    equix_builder: &mut equix::EquiXBuilder,
    pending_gen2_batches: &mut BTreeMap<PeerId, gen2::PendingGen2Batch>,
    req_res_limits: gen2::ReqResRuntimeLimits,
    traffic_cop: &traffic_cop::TrafficCop,
) -> Result<(), NockAppError> {
    match swarm_action {
        SwarmAction::QueueKernelRequest {
            peer_id,
            request_message,
        } => {
            gen2::process_queue_kernel_request_action(
                peer_id, request_message, local_peer_id, transport_tx, next_request_id,
                driver_state, metrics, equix_builder, pending_gen2_batches, req_res_limits,
            )
            .await
        }
        SwarmAction::SendRequest {
            peer_id,
            request,
            request_context,
        } => {
            gen2::process_send_request_action(
                peer_id, request, request_context, transport_tx, next_request_id, driver_state,
                metrics,
            )
            .await
        }
        SwarmAction::SendGossip { peer_id, message } => {
            gen2::process_send_gossip_action(
                peer_id, message, local_peer_id, transport_tx, next_request_id, driver_state,
                metrics, equix_builder,
            )
            .await
        }
        SwarmAction::RetryRequests { requests, delay } => {
            gen2::spawn_retry_requests(join_set, swarm_tx, requests, delay);
            Ok(())
        }
        SwarmAction::FlushDeferredHeardBlocks => {
            gen2::process_flush_deferred_heard_blocks_action(
                buffered_swarm_actions, traffic_cop, metrics, driver_state,
            )
            .await
        }
        SwarmAction::SendResponse { id, response } => {
            trace!("SAction: SendResponse");
            transport_tx
                .send(TransportCommand::CompleteInbound {
                    id,
                    response: Some(response),
                })
                .await
                .map_err(|_| {
                    NockAppError::OtherError(String::from("transport command channel closed"))
                })
        }
        SwarmAction::BlockPeer { peer_id } => {
            warn!("SAction: Blocking peer {peer_id}");
            let addresses = driver_state
                .lock()
                .await
                .peer_connections
                .get(&peer_id)
                .map(|connections| connections.values().copied().collect::<Vec<_>>())
                .unwrap_or_default();
            if addresses.is_empty() {
                error!("Failed to get peer IP address for peer id: {peer_id}");
            }
            for address in addresses {
                match address.socket.ip() {
                    std::net::IpAddr::V4(ip) => log_fail2ban_ipv4(&peer_id, &ip),
                    std::net::IpAddr::V6(ip) => log_fail2ban_ipv6(&peer_id, &ip),
                }
            }
            transport_tx
                .send(TransportCommand::DisconnectPeer { peer: peer_id })
                .await
                .map_err(|_| {
                    NockAppError::OtherError(String::from("transport command channel closed"))
                })
        }
        SwarmAction::RecordExclusionOutcome {
            outcome,
            related_peers,
        } => {
            super::record_exclusion_outcome(
                transport_tx, driver_state, metrics, outcome, &related_peers,
            )
            .await
        }
    }
}
