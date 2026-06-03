use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use libp2p::peer_store::Store;
use libp2p::request_response::ResponseChannel;
use libp2p::{PeerId, Swarm};
use nockapp::NockAppError;
use serde_bytes::ByteBuf;
use tokio::sync::{mpsc, Mutex};
use tokio::time::Duration;
use tracing::{error, trace, warn};

use crate::behaviour::NockchainBehaviour;
use crate::driver::gen2;
use crate::ip_block::{ExclusionOutcome, PeerExclusions};
use crate::messages::{NockchainRequest, NockchainResponse};
use crate::metrics::NockchainP2PMetrics;
use crate::p2p_state::{OutboundRequestContext, P2PState};
use crate::p2p_util::{log_fail2ban_ipv4, log_fail2ban_ipv6};
use crate::tracked_join_set::TrackedJoinSet;
use crate::traffic_cop;

#[derive(Debug)]
pub(crate) enum SwarmAction {
    SendResponse {
        channel: ResponseChannel<NockchainResponse>,
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
    swarm: &mut Swarm<NockchainBehaviour>,
    buffered_swarm_actions: &mut VecDeque<SwarmAction>,
    swarm_tx: &mpsc::Sender<SwarmAction>,
    join_set: &mut TrackedJoinSet<Result<(), NockAppError>>,
    driver_state: &Arc<Mutex<P2PState>>,
    metrics: &Arc<NockchainP2PMetrics>,
    peer_exclusions: &PeerExclusions,
    equix_builder: &mut equix::EquiXBuilder,
    peer_gen2_inbound: &mut BTreeMap<PeerId, bool>,
    pending_gen2_batches: &mut BTreeMap<PeerId, gen2::PendingGen2Batch>,
    req_res_gen2_send_enabled: bool,
    req_res_limits: gen2::ReqResRuntimeLimits,
    traffic_cop: &traffic_cop::TrafficCop,
) -> Result<(), NockAppError> {
    match swarm_action {
        SwarmAction::QueueKernelRequest {
            peer_id,
            request_message,
        } => {
            gen2::process_queue_kernel_request_action(
                peer_id, request_message, swarm, driver_state, metrics, equix_builder,
                peer_gen2_inbound, pending_gen2_batches, req_res_gen2_send_enabled, req_res_limits,
            )
            .await
        }
        SwarmAction::SendRequest {
            peer_id,
            request,
            request_context,
        } => {
            gen2::process_send_request_action(
                peer_id, request, request_context, swarm, driver_state, metrics, equix_builder,
                peer_gen2_inbound, pending_gen2_batches, req_res_gen2_send_enabled, req_res_limits,
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
        SwarmAction::SendResponse { channel, response } => {
            trace!("SAction: SendResponse");
            let _ = swarm
                .behaviour_mut()
                .request_response
                .send_response(channel, response);
            Ok(())
        }
        SwarmAction::BlockPeer { peer_id } => {
            warn!("SAction: Blocking peer {peer_id}");
            swarm.behaviour_mut().allow_block_list.block_peer(peer_id);
            {
                let peer_addresses = swarm
                    .behaviour_mut()
                    .peer_store
                    .store()
                    .addresses_of_peer(&peer_id);
                if let Some(peer_multi_addrs) = peer_addresses {
                    for multi_addr in peer_multi_addrs {
                        for protocol in multi_addr.iter() {
                            match protocol {
                                libp2p::core::multiaddr::Protocol::Ip4(ip) => {
                                    log_fail2ban_ipv4(&peer_id, &ip);
                                }
                                libp2p::core::multiaddr::Protocol::Ip6(ip) => {
                                    log_fail2ban_ipv6(&peer_id, &ip);
                                }
                                _ => {}
                            }
                        }
                    }
                } else {
                    error!("Failed to get peer IP address for peer id: {peer_id}");
                };
            }
            let _ = swarm.disconnect_peer_id(peer_id);
            Ok(())
        }
        SwarmAction::RecordExclusionOutcome {
            outcome,
            related_peers,
        } => {
            super::record_exclusion_outcome(
                swarm, driver_state, peer_exclusions, metrics, outcome, &related_peers,
            )
            .await;
            Ok(())
        }
    }
}
