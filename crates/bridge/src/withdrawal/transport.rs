use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::Address;
use async_trait::async_trait;
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::{Bytes, NounAllocator};
use noun_serde::{NounDecode, NounEncode};
use tokio::sync::Mutex;
use tonic::Response;
use tracing::{info, warn};

use crate::core::withdrawal::assembly::{
    scheduled_assembler_node_id, scheduled_assembler_turn_node_id,
};
use crate::observability::health::PeerEndpoint;
use crate::observability::status::BridgeStatus;
use crate::shared::errors::BridgeError;
use crate::shared::ingress::proto::bridge_ingress_client::BridgeIngressClient;
use crate::shared::ingress::proto::{
    CanonicalWithdrawalProposalBroadcast, CanonicalWithdrawalProposalBroadcastResponse,
    SignedWithdrawalProposalBroadcast, SignedWithdrawalProposalBroadcastResponse,
    WithdrawalCommitCertificate, WithdrawalCommitSignature, WithdrawalId as ProtoWithdrawalId,
    WithdrawalNoteName as ProtoWithdrawalNoteName, WithdrawalProposalBroadcast,
    WithdrawalProposalBroadcastResponse, WithdrawalProposalEnvelope,
    WithdrawalProposalStatusResponse, WithdrawalSnapshot as ProtoWithdrawalSnapshot,
};
use crate::shared::signing::{verify_bridge_signature, BridgeSigner};
use crate::shared::types::{keccak256, AtomBytes, Tip5Hash};
use crate::withdrawal::proposals::{
    reconstruct_withdrawal_proposal, TrackedWithdrawalRequest, WithdrawalProposalRegistry,
    WithdrawalProposalValidationError, WithdrawalProposalValidationOutcome,
};
use crate::withdrawal::state::{LiveWithdrawalView, WithdrawalFallbackPolicy, WithdrawalState};
use crate::withdrawal::submission::{
    register_withdrawal_or_alert, sequenced_withdrawal_released,
    WithdrawalSequencerCanonicalizationError, WithdrawalSequencerPort,
};
use crate::withdrawal::types::{WithdrawalId, WithdrawalProposalData, WithdrawalSnapshot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithdrawalProposalBroadcastOutcome {
    pub proposal_hash: String,
    pub accepted_node_ids: Vec<u64>,
    pub canonicalized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedWithdrawalProposalBroadcastOutcome {
    pub proposal_hash: String,
    pub accepted_node_ids: Vec<u64>,
}

#[derive(Clone)]
pub struct WithdrawalProposalTransport {
    local_node_id: u64,
    node_pkhs: Vec<nockchain_types::tx_engine::common::Hash>,
    node_eth_addresses: HashMap<u64, Address>,
    min_signers: usize,
    commit_signer: Arc<BridgeSigner>,
    registry: Arc<WithdrawalProposalRegistry>,
    sequencer: Option<Arc<dyn WithdrawalSequencerPort>>,
    bridge_status: Option<BridgeStatus>,
    _fallback_policy: WithdrawalFallbackPolicy,
    acceptances: Arc<Mutex<HashMap<ProposalCommitKey, HashSet<u64>>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProposalCommitKey {
    id: WithdrawalId,
    epoch: u64,
    proposal_hash: String,
}

#[async_trait]
trait WithdrawalPeerRpc: Send + Sync {
    /// Broadcasts a freshly prepared withdrawal proposal to a peer bridge node.
    async fn broadcast_proposal(
        &self,
        peer: &PeerEndpoint,
        request: WithdrawalProposalBroadcast,
    ) -> Result<WithdrawalProposalBroadcastResponse, BridgeError>;

    /// Broadcasts a peer-canonical withdrawal proposal to a peer bridge node.
    async fn broadcast_canonicalized(
        &self,
        peer: &PeerEndpoint,
        request: CanonicalWithdrawalProposalBroadcast,
    ) -> Result<CanonicalWithdrawalProposalBroadcastResponse, BridgeError>;

    /// Broadcasts a signed withdrawal proposal contribution to a peer bridge
    /// node.
    async fn broadcast_signed(
        &self,
        peer: &PeerEndpoint,
        request: SignedWithdrawalProposalBroadcast,
    ) -> Result<SignedWithdrawalProposalBroadcastResponse, BridgeError>;
}

struct GrpcWithdrawalPeerRpc;

#[async_trait]
impl WithdrawalPeerRpc for GrpcWithdrawalPeerRpc {
    async fn broadcast_proposal(
        &self,
        peer: &PeerEndpoint,
        request: WithdrawalProposalBroadcast,
    ) -> Result<WithdrawalProposalBroadcastResponse, BridgeError> {
        let mut client = BridgeIngressClient::connect(peer.address.clone())
            .await
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to connect to peer {} for withdrawal proposal broadcast: {err}",
                    peer.node_id
                ))
            })?;
        client
            .broadcast_withdrawal_proposal(request)
            .await
            .map(Response::into_inner)
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to send withdrawal proposal broadcast to peer {}: {err}",
                    peer.node_id
                ))
            })
    }

    async fn broadcast_canonicalized(
        &self,
        peer: &PeerEndpoint,
        request: CanonicalWithdrawalProposalBroadcast,
    ) -> Result<CanonicalWithdrawalProposalBroadcastResponse, BridgeError> {
        let mut client = BridgeIngressClient::connect(peer.address.clone())
            .await
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to connect to peer {} for canonical withdrawal proposal broadcast: {err}",
                    peer.node_id
                ))
            })?;
        client
            .broadcast_canonical_withdrawal_proposal(request)
            .await
            .map(Response::into_inner)
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to send canonical withdrawal proposal broadcast to peer {}: {err}",
                    peer.node_id
                ))
            })
    }

    async fn broadcast_signed(
        &self,
        peer: &PeerEndpoint,
        request: SignedWithdrawalProposalBroadcast,
    ) -> Result<SignedWithdrawalProposalBroadcastResponse, BridgeError> {
        let mut client = BridgeIngressClient::connect(peer.address.clone())
            .await
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to connect to peer {} for signed withdrawal proposal broadcast: {err}",
                    peer.node_id
                ))
            })?;
        client
            .broadcast_signed_withdrawal_proposal(request)
            .await
            .map(Response::into_inner)
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to send signed withdrawal proposal broadcast to peer {}: {err}",
                    peer.node_id
                ))
            })
    }
}

impl WithdrawalProposalTransport {
    /// Builds the transport component that validates, gossips, and tracks
    /// acceptance state for withdrawal proposals.
    pub fn new(
        local_node_id: u64,
        node_pkhs: Vec<nockchain_types::tx_engine::common::Hash>,
        node_eth_addresses: HashMap<u64, Address>,
        min_signers: usize,
        commit_signer: Arc<BridgeSigner>,
        registry: Arc<WithdrawalProposalRegistry>,
        fallback_policy: WithdrawalFallbackPolicy,
    ) -> Self {
        Self {
            local_node_id,
            node_pkhs,
            node_eth_addresses,
            min_signers: min_signers.max(1),
            commit_signer,
            registry,
            sequencer: None,
            bridge_status: None,
            _fallback_policy: fallback_policy,
            acceptances: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Attaches the sequencer client used to report peer-canonical proposals
    /// before authorization begins.
    pub fn with_sequencer(mut self, sequencer: Arc<dyn WithdrawalSequencerPort>) -> Self {
        self.sequencer = Some(sequencer);
        self
    }

    /// Attaches the operator status sink used to alert on sequencer
    /// registration failures during frontier hydration.
    pub fn with_bridge_status(mut self, bridge_status: BridgeStatus) -> Self {
        self.bridge_status = Some(bridge_status);
        self
    }

    /// Returns the shared registry used for proposal persistence and epoch
    /// legality checks.
    pub fn registry(&self) -> &Arc<WithdrawalProposalRegistry> {
        &self.registry
    }

    async fn record_peer_canonical_at_sequencer(
        &self,
        proposal: &WithdrawalProposalData,
        commit_certificate: &WithdrawalCommitCertificate,
    ) -> Result<(), WithdrawalSequencerCanonicalizationError> {
        let Some(sequencer) = self.sequencer.as_ref() else {
            return Ok(());
        };
        let Some(withdrawal_nonce) = self
            .registry
            .fetch_live_withdrawal(&proposal.id)
            .await?
            .and_then(|row| row.withdrawal_nonce)
        else {
            return Err(WithdrawalSequencerCanonicalizationError::Bridge(
                BridgeError::Runtime(format!(
                    "missing live withdrawal nonce while recording canonical proposal {:?} at sequencer",
                    proposal.id
                )),
            ));
        };
        sequencer
            .record_peer_canonical_proposal(
                proposal, withdrawal_nonce, commit_certificate, self.local_node_id,
            )
            .await
    }

    async fn ensure_local_canonical_inputs_unreserved(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<(), WithdrawalSequencerCanonicalizationError> {
        let Some(sequencer) = self.sequencer.as_ref() else {
            return Ok(());
        };
        let reserved_inputs = sequencer.get_reserved_withdrawal_inputs().await?;
        for selected in &proposal.selected_inputs {
            if reserved_inputs.iter().any(|reserved| reserved == selected) {
                return Err(WithdrawalSequencerCanonicalizationError::Rejected(format!(
                    "selected input {:?} is already reserved at the sequencer",
                    selected
                )));
            }
        }
        Ok(())
    }

    async fn expire_local_precanonical_attempt(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<(), BridgeError> {
        let Some(existing) = self.registry.fetch_live_withdrawal(&proposal.id).await? else {
            return Ok(());
        };
        if existing.current_epoch == proposal.epoch && existing.state == WithdrawalState::Prepared {
            self.registry.mark_proposal_expired(proposal).await?;
        }
        Ok(())
    }

    pub(crate) async fn record_signed_progress_at_sequencer(
        &self,
        proposal: &WithdrawalProposalData,
        signer_node_id: u64,
    ) -> Result<(), BridgeError> {
        let Some(sequencer) = self.sequencer.as_ref() else {
            return Ok(());
        };
        if !self
            .sequencer_frontier_allows_withdrawal(&proposal.id)
            .await?
        {
            return Ok(());
        }
        let Some(row) = self.registry.fetch_live_withdrawal(&proposal.id).await? else {
            return Ok(());
        };
        if !matches!(
            row.state,
            WithdrawalState::PeerCanonical | WithdrawalState::Authorized
        ) {
            return Ok(());
        }
        let Some(withdrawal_nonce) = row.withdrawal_nonce else {
            return Err(BridgeError::Runtime(format!(
                "missing live withdrawal nonce while recording signed proposal {:?} at sequencer",
                proposal.id
            )));
        };
        match sequencer
            .record_signed_proposal(proposal, withdrawal_nonce, signer_node_id)
            .await
        {
            Ok(()) => Ok(()),
            Err(err) if is_stale_withdrawal_frontier_error(&err) => {
                info!(
                    target: "bridge.withdrawal.transport",
                    withdrawal_id = ?proposal.id,
                    epoch = proposal.epoch,
                    signer_node_id,
                    error = %err,
                    "dropping signed withdrawal progress because sequencer frontier advanced",
                );
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    pub(crate) async fn sequencer_frontier_allows_withdrawal(
        &self,
        id: &WithdrawalId,
    ) -> Result<bool, BridgeError> {
        let Some(sequencer) = self.sequencer.as_ref() else {
            return Ok(true);
        };
        if sequencer.current_live_withdrawal_nonce().await?.is_none() {
            for tracked in self
                .registry
                .load_sorted_tracked_withdrawal_requests()
                .await?
            {
                let status = sequencer
                    .get_sequenced_withdrawal_status(&tracked.id)
                    .await?;
                if sequenced_withdrawal_released(&status) {
                    continue;
                }
                if let Some(bridge_status) = self.bridge_status.as_ref() {
                    register_withdrawal_or_alert(sequencer.as_ref(), bridge_status, &tracked)
                        .await?;
                } else {
                    sequencer.register_withdrawal(&tracked).await?;
                }
                if sequencer.current_live_withdrawal_nonce().await?.is_some() {
                    break;
                }
                let status = sequencer
                    .get_sequenced_withdrawal_status(&tracked.id)
                    .await?;
                if sequenced_withdrawal_released(&status) {
                    continue;
                }
                break;
            }
        }
        sequencer.frontier_allows_withdrawal(id).await
    }

    async fn load_or_hydrate_canonical_proposal_for_row(
        &self,
        row: &LiveWithdrawalView,
        epoch: u64,
    ) -> Result<Option<WithdrawalProposalData>, BridgeError> {
        if let Some(proposal) = self
            .registry
            .fetch_cached_proposal(row.id.clone(), epoch)
            .await?
        {
            return Ok(Some(proposal));
        }
        let Some(sequencer) = self.sequencer.as_ref() else {
            return Ok(None);
        };
        let Some(artifacts) = sequencer.load_canonical_proposal_artifacts(&row.id).await? else {
            return Ok(None);
        };
        let tracked = TrackedWithdrawalRequest::from_live_withdrawal(row)?;
        let proposal = reconstruct_withdrawal_proposal(&tracked, artifacts)?;
        if proposal.epoch != epoch {
            return Err(BridgeError::Runtime(format!(
                "hydrated canonical proposal epoch {} does not match requested epoch {} for {:?}",
                proposal.epoch, epoch, row.id
            )));
        }
        self.registry
            .cache_reconstructed_proposal(proposal.clone())
            .await?;
        Ok(Some(proposal))
    }

    /// Validates, records, and signs a proposal received from a peer bridge node.
    pub async fn ingest_peer_proposal(
        &self,
        sender_node_id: u64,
        proposal: &WithdrawalProposalData,
    ) -> Result<WithdrawalProposalBroadcastResponse, BridgeError> {
        let proposal_hash = proposal.proposal_hash()?;
        if !self
            .sequencer_frontier_allows_withdrawal(&proposal.id)
            .await?
        {
            return Ok(WithdrawalProposalBroadcastResponse {
                accepted: false,
                status: "not_frontier".to_string(),
                proposal_hash,
                responder_node_id: self.local_node_id,
                error: "withdrawal nonce is not the current sequencer frontier".to_string(),
                commit_signature: None,
            });
        }
        let (proposal_hash, outcome) = match self
            .validate_and_record_acceptances(
                proposal,
                sender_node_id,
                [self.local_node_id, sender_node_id],
            )
            .await
        {
            Ok(outcome) => outcome,
            Err(err) => {
                return Ok(WithdrawalProposalBroadcastResponse {
                    accepted: false,
                    status: validation_error_status(&err).to_string(),
                    proposal_hash,
                    responder_node_id: self.local_node_id,
                    error: err.to_string(),
                    commit_signature: None,
                });
            }
        };

        Ok(WithdrawalProposalBroadcastResponse {
            accepted: true,
            status: validation_outcome_status(outcome).to_string(),
            proposal_hash: proposal_hash.clone(),
            responder_node_id: self.local_node_id,
            error: String::new(),
            commit_signature: Some(self.sign_commit_signature(proposal, &proposal_hash).await?),
        })
    }

    /// Validates a canonicalized proposal announcement and updates local
    /// durable state to reflect the peer-canonical candidate.
    pub async fn ingest_canonicalized_proposal(
        &self,
        sender_node_id: u64,
        proposal: &WithdrawalProposalData,
        commit_certificate: &WithdrawalCommitCertificate,
    ) -> Result<CanonicalWithdrawalProposalBroadcastResponse, BridgeError> {
        let proposal_hash = proposal.proposal_hash()?;
        if !self
            .sequencer_frontier_allows_withdrawal(&proposal.id)
            .await?
        {
            return Ok(CanonicalWithdrawalProposalBroadcastResponse {
                accepted: false,
                status: "not_frontier".to_string(),
                proposal_hash,
                error: "withdrawal nonce is not the current sequencer frontier".to_string(),
            });
        }
        let (proposal_hash, _) = match self
            .validate_and_record_acceptances(
                proposal,
                sender_node_id,
                [self.local_node_id, sender_node_id],
            )
            .await
        {
            Ok(outcome) => outcome,
            Err(err) => {
                return Ok(CanonicalWithdrawalProposalBroadcastResponse {
                    accepted: false,
                    status: validation_error_status(&err).to_string(),
                    proposal_hash,
                    error: err.to_string(),
                });
            }
        };
        self.verify_commit_certificate(
            proposal, &proposal_hash, commit_certificate, self.min_signers,
        )?;
        self.record_peer_canonical_at_sequencer(proposal, commit_certificate)
            .await
            .map_err(|err| match err {
                WithdrawalSequencerCanonicalizationError::Rejected(message) => {
                    BridgeError::Runtime(format!(
                        "sequencer rejected canonical withdrawal {:?} epoch {}: {}",
                        proposal.id, proposal.epoch, message
                    ))
                }
                WithdrawalSequencerCanonicalizationError::Bridge(err) => err,
            })?;
        self.registry
            .mark_proposal_canonical_with_certificate(proposal, commit_certificate)
            .await?;

        Ok(CanonicalWithdrawalProposalBroadcastResponse {
            accepted: true,
            status: "canonicalized".to_string(),
            proposal_hash,
            error: String::new(),
        })
    }

    /// Validates and records a signed proposal contribution received from a
    /// peer.
    pub async fn ingest_peer_signed_proposal(
        &self,
        sender_node_id: u64,
        proposal: &WithdrawalProposalData,
    ) -> Result<SignedWithdrawalProposalBroadcastResponse, BridgeError> {
        let proposal_hash = proposal.proposal_hash()?;
        if !self
            .sequencer_frontier_allows_withdrawal(&proposal.id)
            .await?
        {
            return Ok(SignedWithdrawalProposalBroadcastResponse {
                accepted: false,
                status: "not_frontier".to_string(),
                proposal_hash,
                responder_node_id: self.local_node_id,
                error: "withdrawal nonce is not the current sequencer frontier".to_string(),
            });
        }
        let Some(sequenced) = self.registry.fetch_live_withdrawal(&proposal.id).await? else {
            return Ok(SignedWithdrawalProposalBroadcastResponse {
                accepted: false,
                status: "not_live".to_string(),
                proposal_hash,
                responder_node_id: self.local_node_id,
                error: "withdrawal is not live in the authoritative withdrawal store".to_string(),
            });
        };
        let Some(base_proposal) = self
            .load_or_hydrate_canonical_proposal_for_row(&sequenced, proposal.epoch)
            .await?
        else {
            return Ok(SignedWithdrawalProposalBroadcastResponse {
                accepted: false,
                status: "unknown_proposal".to_string(),
                proposal_hash,
                responder_node_id: self.local_node_id,
                error: "signed withdrawal proposal has no persisted base proposal".to_string(),
            });
        };
        if !signed_proposal_matches_base(&base_proposal, proposal) {
            return Ok(SignedWithdrawalProposalBroadcastResponse {
                accepted: false,
                status: "proposal_mismatch".to_string(),
                proposal_hash,
                responder_node_id: self.local_node_id,
                error: "signed withdrawal proposal diverged from persisted base proposal"
                    .to_string(),
            });
        }
        let live_hash_matches = sequenced.proposal_hash.as_deref() == Some(proposal_hash.as_str());
        if !live_hash_matches {
            return Ok(SignedWithdrawalProposalBroadcastResponse {
                accepted: false,
                status: "not_canonical".to_string(),
                proposal_hash,
                responder_node_id: self.local_node_id,
                error: "signed withdrawal proposal does not match the live canonical proposal"
                    .to_string(),
            });
        }
        if let Err(err) = validate_signed_proposal_contribution(
            &base_proposal.transaction, &proposal.transaction, sender_node_id, &self.node_pkhs,
        ) {
            return Ok(SignedWithdrawalProposalBroadcastResponse {
                accepted: false,
                status: err.status().to_string(),
                proposal_hash,
                responder_node_id: self.local_node_id,
                error: err.to_string(),
            });
        }
        if self
            .registry
            .has_signed_proposal_from_signer(
                &proposal.id, proposal.epoch, &proposal_hash, sender_node_id,
            )
            .await?
        {
            return Ok(SignedWithdrawalProposalBroadcastResponse {
                accepted: true,
                status: "replay".to_string(),
                proposal_hash,
                responder_node_id: self.local_node_id,
                error: String::new(),
            });
        }

        self.registry
            .record_proposal_signed(proposal, sender_node_id)
            .await?;
        self.record_signed_progress_at_sequencer(proposal, sender_node_id)
            .await?;

        Ok(SignedWithdrawalProposalBroadcastResponse {
            accepted: true,
            status: "inserted".to_string(),
            proposal_hash,
            responder_node_id: self.local_node_id,
            error: String::new(),
        })
    }

    /// Returns the local persisted status for one withdrawal proposal epoch.
    pub async fn proposal_status(
        &self,
        id: WithdrawalId,
        epoch: u64,
    ) -> Result<WithdrawalProposalStatusResponse, BridgeError> {
        let Some(sequenced) = self.registry.fetch_live_withdrawal(&id).await? else {
            return Ok(WithdrawalProposalStatusResponse {
                status: "not_found".to_string(),
                proposal_hash: String::new(),
                transaction_name: String::new(),
                canonicalized: false,
                sequenced_state: String::new(),
            });
        };
        let stored = self
            .load_or_hydrate_canonical_proposal_for_row(&sequenced, epoch)
            .await?;

        let Some(proposal) = stored else {
            return Ok(WithdrawalProposalStatusResponse {
                status: "not_found".to_string(),
                proposal_hash: String::new(),
                transaction_name: String::new(),
                canonicalized: false,
                sequenced_state: sequenced.state.as_str().to_string(),
            });
        };

        let proposal_hash = proposal.proposal_hash()?;
        let transaction_name = transaction_name(&proposal).to_string();
        let canonicalized = sequenced.proposal_hash.as_deref() == Some(proposal_hash.as_str());
        let sequenced_state = sequenced.state.as_str().to_string();

        Ok(WithdrawalProposalStatusResponse {
            status: if canonicalized {
                "canonicalized".to_string()
            } else {
                "persisted".to_string()
            },
            proposal_hash,
            transaction_name,
            canonicalized,
            sequenced_state,
        })
    }

    /// Broadcasts a prepared proposal to peers, updating local acceptance state
    /// and promoting it to peer-canonical when threshold agreement is reached.
    pub async fn broadcast_proposal_to_peers(
        &self,
        proposal: &WithdrawalProposalData,
        peers: &[PeerEndpoint],
    ) -> Result<WithdrawalProposalBroadcastOutcome, BridgeError> {
        self.broadcast_proposal_with_rpc(proposal, peers, &GrpcWithdrawalPeerRpc)
            .await
    }

    pub async fn current_expected_assembler_node_id(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<u64, BridgeError> {
        self.expected_assembler_node_id(proposal)
            .await
            .map_err(|err| BridgeError::Runtime(format!("assembler lookup failed: {err}")))
    }

    /// Broadcasts a signed proposal contribution to peers after local
    /// persistence succeeds. By "signed', we are referring to a committed
    /// proposal whose Nock `Transaction` has been signed.
    pub async fn broadcast_signed_proposal_to_peers(
        &self,
        proposal: &WithdrawalProposalData,
        peers: &[PeerEndpoint],
    ) -> Result<SignedWithdrawalProposalBroadcastOutcome, BridgeError> {
        self.broadcast_signed_proposal_with_rpc(proposal, peers, &GrpcWithdrawalPeerRpc)
            .await
    }

    /// Shared implementation for proposal broadcast fanout and local acceptance
    /// bookkeeping.
    async fn broadcast_proposal_with_rpc<R: WithdrawalPeerRpc>(
        &self,
        proposal: &WithdrawalProposalData,
        peers: &[PeerEndpoint],
        rpc: &R,
    ) -> Result<WithdrawalProposalBroadcastOutcome, BridgeError> {
        let proposal_hash = proposal.proposal_hash()?;
        if !self
            .sequencer_frontier_allows_withdrawal(&proposal.id)
            .await?
        {
            return Ok(WithdrawalProposalBroadcastOutcome {
                proposal_hash,
                accepted_node_ids: Vec::new(),
                canonicalized: false,
            });
        }
        let (proposal_hash, _) = match self
            .validate_and_record_acceptances(proposal, self.local_node_id, [self.local_node_id])
            .await
        {
            Ok(outcome) => outcome,
            Err(err) if is_stale_local_proposal_validation_error(&err) => {
                info!(
                    target: "bridge.withdrawal.transport",
                    withdrawal_id = ?proposal.id,
                    epoch = proposal.epoch,
                    error = %err,
                    "dropping local withdrawal proposal because assembler ownership advanced",
                );
                return Ok(passive_proposal_broadcast_outcome(proposal_hash));
            }
            Err(err) => {
                return Err(BridgeError::Runtime(format!(
                    "local withdrawal proposal validation failed: {err}"
                )));
            }
        };

        let proposal_proto = proposal_to_proto(proposal)?;
        let timestamp = unix_timestamp_secs();
        let local_commit_signature = self.sign_commit_signature(proposal, &proposal_hash).await?;
        let mut accepted_node_ids = vec![self.local_node_id];
        let mut commit_signatures = vec![local_commit_signature];

        for peer in peers {
            let request = WithdrawalProposalBroadcast {
                sender_node_id: self.local_node_id,
                proposal: Some(proposal_proto.clone()),
                proposal_hash: proposal_hash.clone(),
                timestamp,
            };
            match rpc.broadcast_proposal(peer, request).await {
                Ok(inner) => {
                    if inner.accepted {
                        let Some(commit_signature) = inner.commit_signature else {
                            warn!(
                                target: "bridge.withdrawal.transport",
                                peer_node_id = peer.node_id,
                                proposal_hash = %proposal_hash,
                                "peer accepted withdrawal proposal without commit signature"
                            );
                            continue;
                        };
                        if let Err(err) = verify_withdrawal_commit_signature(
                            proposal, &proposal_hash, &commit_signature, &self.node_eth_addresses,
                        ) {
                            warn!(
                                target: "bridge.withdrawal.transport",
                                peer_node_id = peer.node_id,
                                proposal_hash = %proposal_hash,
                                error = %err,
                                "peer returned invalid withdrawal commit signature"
                            );
                            continue;
                        }
                        self.record_acceptance(proposal, inner.responder_node_id, &proposal_hash)
                            .await;
                        accepted_node_ids.push(inner.responder_node_id);
                        commit_signatures.push(commit_signature);
                    } else {
                        warn!(
                            target: "bridge.withdrawal.transport",
                            peer_node_id = peer.node_id,
                            proposal_hash = %proposal_hash,
                            error = %inner.error,
                            "peer rejected withdrawal proposal broadcast"
                        );
                    }
                }
                Err(err) => {
                    warn!(
                        target: "bridge.withdrawal.transport",
                        peer_node_id = peer.node_id,
                        proposal_hash = %proposal_hash,
                        error = %err,
                        "failed to send withdrawal proposal broadcast"
                    );
                }
            }
        }

        accepted_node_ids.sort_unstable();
        accepted_node_ids.dedup();
        commit_signatures.sort_by_key(|signature| signature.signer_node_id);
        commit_signatures.dedup_by_key(|signature| signature.signer_node_id);

        let canonicalized = commit_signatures.len() >= self.min_signers;
        if canonicalized {
            let commit_certificate = WithdrawalCommitCertificate {
                withdrawal_id: Some(withdrawal_id_to_proto(&proposal.id)),
                epoch: proposal.epoch,
                proposal_hash: proposal_hash.clone(),
                signatures: commit_signatures.clone(),
            };
            self.verify_commit_certificate(
                proposal, &proposal_hash, &commit_certificate, self.min_signers,
            )?;
            if let Err(err) = self
                .ensure_local_canonical_inputs_unreserved(proposal)
                .await
            {
                let err = match err {
                    WithdrawalSequencerCanonicalizationError::Rejected(message) => {
                        self.expire_local_precanonical_attempt(proposal).await?;
                        BridgeError::Runtime(format!(
                            "operator rejected canonical withdrawal {:?} epoch {} before sequencer submission: {}",
                            proposal.id, proposal.epoch, message
                        ))
                    }
                    WithdrawalSequencerCanonicalizationError::Bridge(err) => err,
                };
                return Err(err);
            }
            if let Err(err) = self
                .record_peer_canonical_at_sequencer(proposal, &commit_certificate)
                .await
            {
                let err = match err {
                    WithdrawalSequencerCanonicalizationError::Rejected(message) => {
                        if is_stale_withdrawal_frontier_message(&message) {
                            info!(
                                target: "bridge.withdrawal.transport",
                                withdrawal_id = ?proposal.id,
                                epoch = proposal.epoch,
                                error = %message,
                                "dropping canonical withdrawal proposal because sequencer frontier advanced",
                            );
                            return Ok(WithdrawalProposalBroadcastOutcome {
                                proposal_hash,
                                accepted_node_ids,
                                canonicalized: false,
                            });
                        }
                        self.expire_local_precanonical_attempt(proposal).await?;
                        BridgeError::Runtime(format!(
                            "sequencer rejected canonical withdrawal {:?} epoch {}: {}",
                            proposal.id, proposal.epoch, message
                        ))
                    }
                    WithdrawalSequencerCanonicalizationError::Bridge(err) => err,
                };
                return Err(err);
            }
            self.registry
                .mark_proposal_canonical_with_certificate(proposal, &commit_certificate)
                .await?;

            let canonical_msg = CanonicalWithdrawalProposalBroadcast {
                sender_node_id: self.local_node_id,
                proposal: Some(proposal_proto),
                proposal_hash: proposal_hash.clone(),
                timestamp,
                commit_certificate: Some(commit_certificate),
            };
            for peer in peers {
                if let Err(err) = rpc
                    .broadcast_canonicalized(peer, canonical_msg.clone())
                    .await
                {
                    warn!(
                        target: "bridge.withdrawal.transport",
                        peer_node_id = peer.node_id,
                        proposal_hash = %proposal_hash,
                        error = %err,
                        "failed to notify peer about canonical withdrawal proposal"
                    );
                }
            }
        }

        Ok(WithdrawalProposalBroadcastOutcome {
            proposal_hash,
            accepted_node_ids,
            canonicalized,
        })
    }

    /// Shared implementation for signed-proposal broadcast fanout.
    async fn broadcast_signed_proposal_with_rpc<R: WithdrawalPeerRpc>(
        &self,
        proposal: &WithdrawalProposalData,
        peers: &[PeerEndpoint],
        rpc: &R,
    ) -> Result<SignedWithdrawalProposalBroadcastOutcome, BridgeError> {
        let proposal_hash = proposal.proposal_hash()?;
        if !self
            .sequencer_frontier_allows_withdrawal(&proposal.id)
            .await?
        {
            return Ok(SignedWithdrawalProposalBroadcastOutcome {
                proposal_hash,
                accepted_node_ids: Vec::new(),
            });
        }
        let proposal_proto = proposal_to_proto(proposal)?;
        let timestamp = unix_timestamp_secs();
        let mut accepted_node_ids = vec![self.local_node_id];

        for peer in peers {
            let request = SignedWithdrawalProposalBroadcast {
                sender_node_id: self.local_node_id,
                proposal: Some(proposal_proto.clone()),
                proposal_hash: proposal_hash.clone(),
                timestamp,
            };
            match rpc.broadcast_signed(peer, request).await {
                Ok(inner) => {
                    if inner.accepted {
                        accepted_node_ids.push(inner.responder_node_id);
                    } else {
                        warn!(
                            target: "bridge.withdrawal.transport",
                            peer_node_id = peer.node_id,
                            proposal_hash = %proposal_hash,
                            error = %inner.error,
                            "peer rejected signed withdrawal proposal broadcast"
                        );
                    }
                }
                Err(err) => {
                    warn!(
                        target: "bridge.withdrawal.transport",
                        peer_node_id = peer.node_id,
                        proposal_hash = %proposal_hash,
                        error = %err,
                        "failed to send signed withdrawal proposal broadcast"
                    );
                }
            }
        }

        accepted_node_ids.sort_unstable();
        accepted_node_ids.dedup();

        Ok(SignedWithdrawalProposalBroadcastOutcome {
            proposal_hash,
            accepted_node_ids,
        })
    }

    /// Records a node's acceptance vote for a proposal hash and returns the
    /// updated accepted-node set.
    async fn record_acceptance(
        &self,
        proposal: &WithdrawalProposalData,
        node_id: u64,
        proposal_hash: &str,
    ) -> Vec<u64> {
        let mut guard = self.acceptances.lock().await;
        let entry = guard
            .entry(ProposalCommitKey {
                id: proposal.id.clone(),
                epoch: proposal.epoch,
                proposal_hash: proposal_hash.to_string(),
            })
            .or_default();
        entry.insert(node_id);
        let mut accepted = entry.iter().copied().collect::<Vec<_>>();
        accepted.sort_unstable();
        accepted
    }

    /// Validates a proposal, persists it if legal, and records commitment acceptance votes
    /// for the supplied node ids.
    async fn validate_and_record_acceptances<I>(
        &self,
        proposal: &WithdrawalProposalData,
        sender_node_id: u64,
        node_ids: I,
    ) -> Result<(String, WithdrawalProposalValidationOutcome), WithdrawalProposalValidationError>
    where
        I: IntoIterator<Item = u64>,
    {
        let proposal_hash = proposal.proposal_hash()?;
        let expected_node_id = self.expected_assembler_node_id(proposal).await?;
        if sender_node_id != expected_node_id {
            return Err(WithdrawalProposalValidationError::WrongAssembler {
                id: proposal.id.clone(),
                epoch: proposal.epoch,
                expected_node_id,
                received_node_id: sender_node_id,
            });
        }
        if let Some(existing) = self.registry.fetch_live_withdrawal(&proposal.id).await? {
            if proposal.epoch > existing.current_epoch {
                return Err(WithdrawalProposalValidationError::LiveAttemptExists {
                    id: proposal.id.clone(),
                    current_epoch: existing.current_epoch,
                    live_state: existing.state,
                    received_epoch: proposal.epoch,
                });
            }
        }
        let outcome = self.registry.validate_and_cache_prepared(proposal).await?;
        for node_id in node_ids {
            let _ = self
                .record_acceptance(proposal, node_id, &proposal_hash)
                .await;
        }
        Ok((proposal_hash, outcome))
    }

    async fn expected_assembler_node_id(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<u64, WithdrawalProposalValidationError> {
        if let Some(sequencer) = self.sequencer.as_ref() {
            let status = sequencer
                .get_sequenced_withdrawal_status(&proposal.id)
                .await?;
            if status.found && status.current_epoch == proposal.epoch {
                return Ok(scheduled_assembler_turn_node_id(
                    &proposal.id, proposal.epoch, status.handoff_index, &self.node_pkhs,
                )?);
            }
        }
        Ok(scheduled_assembler_node_id(
            &proposal.id, proposal.epoch, &self.node_pkhs,
        )?)
    }

    /// Signs the peer-canonical commit tuple for one proposal using this
    /// bridge node's Ethereum operator key.
    async fn sign_commit_signature(
        &self,
        proposal: &WithdrawalProposalData,
        proposal_hash: &str,
    ) -> Result<WithdrawalCommitSignature, BridgeError> {
        let digest = compute_withdrawal_commit_digest(&proposal.id, proposal.epoch, proposal_hash)?;
        let signature = self.commit_signer.sign_hash(&digest).await?;
        Ok(WithdrawalCommitSignature {
            signer_node_id: self.local_node_id,
            withdrawal_id: Some(withdrawal_id_to_proto(&proposal.id)),
            epoch: proposal.epoch,
            proposal_hash: proposal_hash.to_string(),
            signature: signature.as_bytes().to_vec(),
        })
    }

    /// Verifies the peer-canonical commit certificate for one proposal tuple.
    fn verify_commit_certificate(
        &self,
        proposal: &WithdrawalProposalData,
        proposal_hash: &str,
        certificate: &WithdrawalCommitCertificate,
        required_signers: usize,
    ) -> Result<(), BridgeError> {
        verify_withdrawal_commit_certificate(
            proposal, proposal_hash, certificate, required_signers, &self.node_eth_addresses,
        )
    }
}

fn passive_proposal_broadcast_outcome(proposal_hash: String) -> WithdrawalProposalBroadcastOutcome {
    WithdrawalProposalBroadcastOutcome {
        proposal_hash,
        accepted_node_ids: Vec::new(),
        canonicalized: false,
    }
}

fn is_stale_local_proposal_validation_error(err: &WithdrawalProposalValidationError) -> bool {
    matches!(
        err,
        WithdrawalProposalValidationError::WrongAssembler { .. }
    )
}

fn is_stale_withdrawal_frontier_error(err: &BridgeError) -> bool {
    is_stale_withdrawal_frontier_message(&err.to_string())
}

fn is_stale_withdrawal_frontier_message(message: &str) -> bool {
    message.contains("while sequencer frontier")
        || message.contains("because the sequencer has no current frontier")
        || message.contains("withdrawal nonce is not the current sequencer frontier")
}

/// Derives the minimum number of distinct commit signatures required by the
/// withdrawal transaction's PKH threshold policy.
pub(crate) fn required_withdrawal_commit_signature_threshold(
    proposal: &WithdrawalProposalData,
) -> Result<usize, BridgeError> {
    let nockchain_types::v1::Transaction::V1(transaction) = &proposal.transaction;
    let nockchain_types::v1::InputMetadata::SpendConditions(input_metadata) =
        &transaction.metadata.inputs
    else {
        return Err(BridgeError::Runtime(
            "withdrawal proposal does not use spend-condition metadata".into(),
        ));
    };

    let mut required = 0usize;
    for (_, spend_condition) in &input_metadata.0 {
        let Some(required_pkh) = spend_condition.required_pkh_policy() else {
            return Err(BridgeError::Runtime(
                "withdrawal proposal input is missing PKH threshold policy".into(),
            ));
        };
        required = required.max(required_pkh.threshold);
    }

    if required == 0 {
        return Err(BridgeError::Runtime(
            "withdrawal proposal commit threshold is zero".into(),
        ));
    }
    Ok(required)
}

/// Verifies one commit signature against the configured Ethereum address for the
/// claimed signer node.
pub(crate) fn verify_withdrawal_commit_signature(
    proposal: &WithdrawalProposalData,
    proposal_hash: &str,
    signature: &WithdrawalCommitSignature,
    node_eth_addresses: &HashMap<u64, Address>,
) -> Result<(), BridgeError> {
    if signature.epoch != proposal.epoch {
        return Err(BridgeError::Runtime(format!(
            "commit signature epoch {} does not match proposal epoch {}",
            signature.epoch, proposal.epoch
        )));
    }
    if signature.proposal_hash != proposal_hash {
        return Err(BridgeError::Runtime(format!(
            "commit signature proposal hash {} does not match proposal hash {}",
            signature.proposal_hash, proposal_hash
        )));
    }
    let Some(withdrawal_id) = signature.withdrawal_id.as_ref() else {
        return Err(BridgeError::Runtime(
            "commit signature is missing withdrawal_id".into(),
        ));
    };
    let signature_id = withdrawal_id_from_proto(withdrawal_id)?;
    if signature_id != proposal.id {
        return Err(BridgeError::Runtime(format!(
            "commit signature withdrawal id {:?} does not match proposal id {:?}",
            signature_id, proposal.id
        )));
    }

    let expected_address = node_eth_addresses
        .get(&signature.signer_node_id)
        .copied()
        .ok_or_else(|| {
            BridgeError::Runtime(format!(
                "missing configured Ethereum address for signer node {}",
                signature.signer_node_id
            ))
        })?;
    let digest = compute_withdrawal_commit_digest(&proposal.id, proposal.epoch, proposal_hash)?;
    let valid_addresses = HashSet::from([expected_address]);
    verify_bridge_signature(&digest, &signature.signature, &valid_addresses).ok_or_else(|| {
        BridgeError::Runtime(format!(
            "commit signature from node {} failed Ethereum address verification",
            signature.signer_node_id
        ))
    })?;
    Ok(())
}

/// Verifies the peer-canonical commit certificate for one proposal tuple.
pub(crate) fn verify_withdrawal_commit_certificate(
    proposal: &WithdrawalProposalData,
    proposal_hash: &str,
    certificate: &WithdrawalCommitCertificate,
    required_signers: usize,
    node_eth_addresses: &HashMap<u64, Address>,
) -> Result<(), BridgeError> {
    let Some(withdrawal_id) = certificate.withdrawal_id.as_ref() else {
        return Err(BridgeError::Runtime(
            "withdrawal commit certificate is missing withdrawal_id".into(),
        ));
    };
    let certificate_id = withdrawal_id_from_proto(withdrawal_id)?;
    if certificate_id != proposal.id {
        return Err(BridgeError::Runtime(format!(
            "withdrawal commit certificate id {:?} does not match proposal id {:?}",
            certificate_id, proposal.id
        )));
    }
    if certificate.epoch != proposal.epoch {
        return Err(BridgeError::Runtime(format!(
            "withdrawal commit certificate epoch {} does not match proposal epoch {}",
            certificate.epoch, proposal.epoch
        )));
    }
    if certificate.proposal_hash != proposal_hash {
        return Err(BridgeError::Runtime(format!(
            "withdrawal commit certificate proposal hash {} does not match {}",
            certificate.proposal_hash, proposal_hash
        )));
    }

    let mut distinct_signers = HashSet::new();
    for signature in &certificate.signatures {
        if !distinct_signers.insert(signature.signer_node_id) {
            return Err(BridgeError::Runtime(format!(
                "withdrawal commit certificate contains duplicate signer {}",
                signature.signer_node_id
            )));
        }
        verify_withdrawal_commit_signature(proposal, proposal_hash, signature, node_eth_addresses)?;
    }

    if distinct_signers.len() < required_signers {
        return Err(BridgeError::Runtime(format!(
            "withdrawal commit certificate has {} signatures but requires {}",
            distinct_signers.len(),
            required_signers
        )));
    }
    Ok(())
}

/// Converts a typed withdrawal proposal into its gRPC envelope form.
pub fn proposal_to_proto(
    proposal: &WithdrawalProposalData,
) -> Result<WithdrawalProposalEnvelope, BridgeError> {
    Ok(WithdrawalProposalEnvelope {
        withdrawal_id: Some(withdrawal_id_to_proto(&proposal.id)),
        recipient: tip5_to_bytes(&proposal.recipient),
        amount: proposal.amount,
        burned_amount: proposal.burned_amount,
        base_batch_end: proposal.base_batch_end,
        epoch: proposal.epoch,
        snapshot: Some(snapshot_to_proto(&proposal.snapshot)),
        selected_inputs: proposal
            .selected_inputs
            .iter()
            .map(|name| ProtoWithdrawalNoteName {
                first: tip5_to_bytes(&name.first),
                last: tip5_to_bytes(&name.last),
            })
            .collect(),
        transaction_name: transaction_name(proposal).to_string(),
        proposal_jam: jam_proposal(proposal)?,
        withdrawal_nonce: 0,
    })
}

/// Decodes and cross-checks a gRPC proposal envelope against its jammed noun
/// payload.
pub fn proposal_from_proto(
    proposal: &WithdrawalProposalEnvelope,
) -> Result<WithdrawalProposalData, BridgeError> {
    let id = proposal
        .withdrawal_id
        .as_ref()
        .ok_or_else(|| BridgeError::Runtime("missing withdrawal id".into()))
        .and_then(withdrawal_id_from_proto)?;
    let snapshot = proposal
        .snapshot
        .as_ref()
        .ok_or_else(|| BridgeError::Runtime("missing withdrawal snapshot".into()))
        .and_then(snapshot_from_proto)?;
    let decoded = cue_proposal(&proposal.proposal_jam)?;
    if decoded.id != id {
        return Err(BridgeError::Runtime(
            "proposal envelope withdrawal id does not match jam".into(),
        ));
    }
    if decoded.epoch != proposal.epoch {
        return Err(BridgeError::Runtime(
            "proposal envelope epoch does not match jam".into(),
        ));
    }
    if decoded.snapshot != snapshot {
        return Err(BridgeError::Runtime(
            "proposal envelope snapshot does not match jam".into(),
        ));
    }
    if decoded.recipient != tip5_from_bytes(&proposal.recipient)? {
        return Err(BridgeError::Runtime(
            "proposal envelope recipient does not match jam".into(),
        ));
    }
    if decoded.amount != proposal.amount {
        return Err(BridgeError::Runtime(
            "proposal envelope amount does not match jam".into(),
        ));
    }
    if decoded.burned_amount != proposal.burned_amount {
        return Err(BridgeError::Runtime(
            "proposal envelope burned_amount does not match jam".into(),
        ));
    }
    if decoded.base_batch_end != proposal.base_batch_end {
        return Err(BridgeError::Runtime(
            "proposal envelope base_batch_end does not match jam".into(),
        ));
    }
    let envelope_inputs = proposal
        .selected_inputs
        .iter()
        .map(note_name_from_proto)
        .collect::<Result<Vec<_>, _>>()?;
    if decoded.selected_inputs != envelope_inputs {
        return Err(BridgeError::Runtime(
            "proposal envelope selected_inputs do not match jam".into(),
        ));
    }
    if transaction_name(&decoded) != proposal.transaction_name {
        return Err(BridgeError::Runtime(
            "proposal envelope transaction_name does not match jam".into(),
        ));
    }
    Ok(decoded)
}

/// Converts a withdrawal id into its protobuf representation.
pub fn withdrawal_id_to_proto(id: &WithdrawalId) -> ProtoWithdrawalId {
    ProtoWithdrawalId {
        as_of: tip5_to_bytes(&id.as_of),
        base_event_id: id.base_event_id.0.clone(),
    }
}

/// Converts a protobuf withdrawal id into the domain type.
pub fn withdrawal_id_from_proto(id: &ProtoWithdrawalId) -> Result<WithdrawalId, BridgeError> {
    Ok(WithdrawalId {
        as_of: tip5_from_bytes(&id.as_of)?,
        base_event_id: AtomBytes(id.base_event_id.clone()),
    })
}

/// Converts a withdrawal snapshot into its protobuf representation.
pub fn snapshot_to_proto(snapshot: &WithdrawalSnapshot) -> ProtoWithdrawalSnapshot {
    ProtoWithdrawalSnapshot {
        height: snapshot.height,
        block_id: tip5_to_bytes(&snapshot.block_id),
    }
}

/// Converts a protobuf withdrawal snapshot into the domain type.
pub fn snapshot_from_proto(
    snapshot: &ProtoWithdrawalSnapshot,
) -> Result<WithdrawalSnapshot, BridgeError> {
    Ok(WithdrawalSnapshot {
        height: snapshot.height,
        block_id: tip5_from_bytes(&snapshot.block_id)?,
    })
}

/// Converts a tx-engine note name into its protobuf representation.
pub fn note_name_to_proto(name: &nockchain_types::v1::Name) -> ProtoWithdrawalNoteName {
    ProtoWithdrawalNoteName {
        first: tip5_to_bytes(&name.first),
        last: tip5_to_bytes(&name.last),
    }
}

/// Converts a protobuf note name into the tx-engine note-name type.
pub fn note_name_from_proto(
    name: &ProtoWithdrawalNoteName,
) -> Result<nockchain_types::v1::Name, BridgeError> {
    Ok(nockchain_types::v1::Name::new(
        tip5_from_bytes(&name.first)?,
        tip5_from_bytes(&name.last)?,
    ))
}

/// Jams a withdrawal proposal for transport over gRPC.
fn jam_proposal(proposal: &WithdrawalProposalData) -> Result<Vec<u8>, BridgeError> {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let noun = proposal.to_noun(&mut slab);
    slab.set_root(noun);
    Ok(slab.jam().to_vec())
}

/// Cues a jammed withdrawal proposal received over gRPC back into the typed
/// Rust envelope.
fn cue_proposal(bytes: &[u8]) -> Result<WithdrawalProposalData, BridgeError> {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let noun = slab.cue_into(Bytes::from(bytes.to_vec())).map_err(|err| {
        BridgeError::Runtime(format!("failed to cue withdrawal proposal jam: {err}"))
    })?;
    let space = slab.noun_space();
    WithdrawalProposalData::from_noun(&noun, &space).map_err(|err| {
        BridgeError::Runtime(format!("failed to decode withdrawal proposal noun: {err}"))
    })
}

/// Extracts the stable transaction name from a withdrawal proposal.
fn transaction_name(proposal: &WithdrawalProposalData) -> &str {
    match &proposal.transaction {
        nockchain_types::v1::Transaction::V1(tx) => &tx.name,
    }
}

/// Computes the domain-separated commit digest bridge operators sign when they
/// commit to one withdrawal proposal hash for one epoch.
///
/// `proposal_hash` already commits to the withdrawal id and epoch. They are
/// included here anyway so the signed tuple mirrors the certificate fields and
/// remains explicit if the proposal hash definition changes later.
pub(crate) fn compute_withdrawal_commit_digest(
    id: &WithdrawalId,
    epoch: u64,
    proposal_hash: &str,
) -> Result<[u8; 32], BridgeError> {
    let mut payload = b"bridge.withdrawal.commit.v1".to_vec();
    payload.extend_from_slice(&tip5_to_bytes(&id.as_of));
    payload.extend_from_slice(
        &u64::try_from(id.base_event_id.0.len())
            .map_err(|err| {
                BridgeError::ValueConversion(format!(
                    "base_event_id length too large for commit digest: {err}"
                ))
            })?
            .to_be_bytes(),
    );
    payload.extend_from_slice(&id.base_event_id.0);
    payload.extend_from_slice(&epoch.to_be_bytes());
    payload.extend_from_slice(&proposal_hash_bytes(proposal_hash)?);
    Ok(keccak256(&payload))
}

/// Decodes the blake3 proposal hash hex string into its raw 32-byte form for
/// commit signing.
fn proposal_hash_bytes(proposal_hash: &str) -> Result<Vec<u8>, BridgeError> {
    let bytes = hex::decode(proposal_hash).map_err(|err| {
        BridgeError::Runtime(format!("invalid proposal hash hex {proposal_hash}: {err}"))
    })?;
    if bytes.len() != 32 {
        return Err(BridgeError::Runtime(format!(
            "proposal hash must decode to 32 bytes, got {}",
            bytes.len()
        )));
    }
    Ok(bytes)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SignedProposalContributionError {
    status: &'static str,
    message: String,
}

impl SignedProposalContributionError {
    fn invalid_signer_delta(message: impl Into<String>) -> Self {
        Self {
            status: "invalid_signer_delta",
            message: message.into(),
        }
    }

    fn invalid_witness_signature(message: impl Into<String>) -> Self {
        Self {
            status: "invalid_witness_signature",
            message: message.into(),
        }
    }

    fn status(&self) -> &'static str {
        self.status
    }
}

impl std::fmt::Display for SignedProposalContributionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SignedProposalContributionError {}

fn validate_signed_proposal_contribution(
    base: &nockchain_types::v1::Transaction,
    signed: &nockchain_types::v1::Transaction,
    sender_node_id: u64,
    node_pkhs: &[nockchain_types::tx_engine::common::Hash],
) -> Result<(), SignedProposalContributionError> {
    // Resolve the PKH that this transport sender is allowed to contribute.
    let expected_signer_pkh = node_pkhs
        .get(sender_node_id as usize)
        .cloned()
        .ok_or_else(|| {
            SignedProposalContributionError::invalid_signer_delta(format!(
                "sender node {sender_node_id} has no configured withdrawal signer PKH"
            ))
        })?;
    let (
        nockchain_types::v1::Transaction::V1(base_tx),
        nockchain_types::v1::Transaction::V1(signed_tx),
    ) = (base, signed);
    let nockchain_types::v1::InputMetadata::SpendConditions(input_metadata) =
        &base_tx.metadata.inputs
    else {
        return Err(SignedProposalContributionError::invalid_signer_delta(
            "signed withdrawal proposals require spend-condition metadata",
        ));
    };
    let nockchain_types::v1::WitnessData::Witnesses(base_witness_map) = &base_tx.witness_data
    else {
        return Err(SignedProposalContributionError::invalid_signer_delta(
            "signed withdrawal proposals require witness-based transaction data",
        ));
    };
    let nockchain_types::v1::WitnessData::Witnesses(signed_witness_map) = &signed_tx.witness_data
    else {
        return Err(SignedProposalContributionError::invalid_signer_delta(
            "signed withdrawal proposals require witness-based transaction data",
        ));
    };

    // The signed proposal must preserve the exact witness entry set from the
    // base proposal; peers may only add signatures inside existing witnesses.
    for (name, _) in &base_witness_map.0 {
        if witness_for_name(signed_witness_map, name).is_none() {
            return Err(SignedProposalContributionError::invalid_signer_delta(
                format!("signed withdrawal proposal removed witness entry {name:?}"),
            ));
        }
    }
    for (name, _) in &signed_witness_map.0 {
        if witness_for_name(base_witness_map, name).is_none() {
            return Err(SignedProposalContributionError::invalid_signer_delta(
                format!("signed withdrawal proposal added unexpected witness entry {name:?}"),
            ));
        }
    }

    let mut added_any = false;
    for (name, spend_condition) in &input_metadata.0 {
        // Every spend named in the base proposal must still have both the base
        // and signed witness entries present.
        let base_witness = witness_for_name(base_witness_map, name).ok_or_else(|| {
            SignedProposalContributionError::invalid_signer_delta(format!(
                "persisted base proposal missing witness entry {name:?}"
            ))
        })?;
        let signed_witness = witness_for_name(signed_witness_map, name).ok_or_else(|| {
            SignedProposalContributionError::invalid_signer_delta(format!(
                "signed withdrawal proposal missing witness entry {name:?}"
            ))
        })?;
        // The signer contribution must not rewrite proof material or any other
        // canonical witness payload outside the PKH signature list.
        if base_witness.lock_merkle_proof != signed_witness.lock_merkle_proof
            || base_witness.hax != signed_witness.hax
            || base_witness.tim != signed_witness.tim
        {
            return Err(SignedProposalContributionError::invalid_signer_delta(
                format!("signed withdrawal witness {name:?} changed canonical proof data"),
            ));
        }

        // Existing signer entries are immutable; the peer may add a new
        // signature, but it may not remove or mutate a previously recorded one.
        for base_entry in &base_witness.pkh_signature.0 {
            let Some(signed_entry) = signed_witness
                .pkh_signature
                .0
                .iter()
                .find(|candidate| candidate.pkh == base_entry.pkh)
            else {
                return Err(SignedProposalContributionError::invalid_signer_delta(
                    format!(
                        "signed withdrawal witness {name:?} removed signer {}",
                        base_entry.pkh.to_base58()
                    ),
                ));
            };
            if signed_entry != base_entry {
                return Err(SignedProposalContributionError::invalid_signer_delta(
                    format!(
                        "signed withdrawal witness {name:?} changed existing signer {}",
                        base_entry.pkh.to_base58()
                    ),
                ));
            }
        }

        let required = spend_condition.required_pkh_policy();
        let spend = spend1_for_name(&base_tx.spends, name).ok_or_else(|| {
            SignedProposalContributionError::invalid_signer_delta(format!(
                "persisted base proposal missing witness spend {name:?}"
            ))
        })?;
        if let Some(required) = &required {
            if signed_witness.pkh_signature.0.len() > required.threshold {
                return Err(SignedProposalContributionError::invalid_signer_delta(
                    format!(
                        "signed withdrawal witness {name:?} exceeded PKH threshold {} with {} signatures",
                        required.threshold,
                        signed_witness.pkh_signature.0.len()
                    ),
                ));
            }
        }
        for signed_entry in &signed_witness.pkh_signature.0 {
            if base_witness
                .pkh_signature
                .0
                .iter()
                .any(|candidate| candidate.pkh == signed_entry.pkh)
            {
                continue;
            }
            added_any = true;
            // New signer entries are only allowed on PKH-gated inputs.
            let Some(required) = &required else {
                return Err(SignedProposalContributionError::invalid_signer_delta(
                    format!(
                        "signed withdrawal witness {name:?} added signer {} to a non-PKH input",
                        signed_entry.pkh.to_base58()
                    ),
                ));
            };
            // The new contribution must belong to the claimed sender node.
            if signed_entry.pkh != expected_signer_pkh {
                return Err(SignedProposalContributionError::invalid_signer_delta(format!(
                    "signed withdrawal witness {name:?} added signer {}, expected {} for sender node {}",
                    signed_entry.pkh.to_base58(),
                    expected_signer_pkh.to_base58(),
                    sender_node_id
                )));
            }
            // The claimed sender must actually be one of the required signers
            // for this spend condition.
            if !required.contains(&expected_signer_pkh) {
                return Err(SignedProposalContributionError::invalid_signer_delta(format!(
                    "signed withdrawal witness {name:?} added sender {} to an input that does not require that signer",
                    expected_signer_pkh.to_base58()
                )));
            }
            // The newly added witness signature itself must verify against the
            // spend's signature hash before we trust the contribution.
            spend.verify_pkh_signature(signed_entry).map_err(|err| {
                SignedProposalContributionError::invalid_witness_signature(format!(
                    "invalid withdrawal witness signature on input {name:?}: {err}"
                ))
            })?;
        }
    }

    // A "signed proposal" must actually contribute at least one new witness
    // signature; exact replays are handled separately by signer replay logic.
    if !added_any {
        return Err(SignedProposalContributionError::invalid_signer_delta(
            "signed withdrawal proposal produced no new witness contribution",
        ));
    }

    Ok(())
}

fn witness_for_name<'a>(
    witness_map: &'a nockchain_types::v1::WitnessMap,
    name: &nockchain_types::v1::Name,
) -> Option<&'a nockchain_types::v1::Witness> {
    witness_map
        .0
        .iter()
        .find(|(candidate_name, _)| candidate_name == name)
        .map(|(_, witness)| witness)
}

fn spend1_for_name<'a>(
    spends: &'a nockchain_types::v1::Spends,
    name: &nockchain_types::v1::Name,
) -> Option<&'a nockchain_types::v1::Spend1> {
    spends
        .0
        .iter()
        .find(|(candidate_name, _)| candidate_name == name)
        .and_then(|(_, spend)| match spend {
            nockchain_types::v1::Spend::Witness(spend) => Some(spend),
            _ => None,
        })
}

/// Returns whether a signed proposal only differs from the base proposal by
/// additional witness data.
pub(crate) fn signed_proposal_matches_base(
    base: &WithdrawalProposalData,
    signed: &WithdrawalProposalData,
) -> bool {
    base.id == signed.id
        && base.recipient == signed.recipient
        && base.amount == signed.amount
        && base.burned_amount == signed.burned_amount
        && base.base_batch_end == signed.base_batch_end
        && base.epoch == signed.epoch
        && base.snapshot == signed.snapshot
        && base.selected_inputs == signed.selected_inputs
        && signed_transaction_matches_base(&base.transaction, &signed.transaction)
}

/// Returns whether a signed transaction preserves the base transaction body and
/// only changes witness data.
fn signed_transaction_matches_base(
    base: &nockchain_types::v1::Transaction,
    signed: &nockchain_types::v1::Transaction,
) -> bool {
    match (base, signed) {
        (
            nockchain_types::v1::Transaction::V1(base_tx),
            nockchain_types::v1::Transaction::V1(signed_tx),
        ) => {
            base_tx.name == signed_tx.name
                && base_tx.spends == signed_tx.spends
                && base_tx.metadata == signed_tx.metadata
        }
    }
}

/// Converts a Tip5 hash into the byte representation used by gRPC payloads.
fn tip5_to_bytes(hash: &Tip5Hash) -> Vec<u8> {
    hash.to_be_limb_bytes().to_vec()
}

/// Converts raw gRPC hash bytes into a Tip5 hash.
pub(crate) fn tip5_from_bytes(bytes: &[u8]) -> Result<Tip5Hash, BridgeError> {
    Tip5Hash::from_be_limb_bytes(bytes)
        .map_err(|err| BridgeError::Runtime(format!("invalid tip5 bytes: {err}")))
}

/// Returns the current wall-clock time as Unix seconds for transport response
/// timestamps.
fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Maps proposal validation errors onto stable transport status strings.
fn validation_error_status(err: &WithdrawalProposalValidationError) -> &'static str {
    match err {
        WithdrawalProposalValidationError::UnknownWithdrawal { .. } => "unknown_withdrawal",
        WithdrawalProposalValidationError::WithdrawalMismatch { .. } => "withdrawal_mismatch",
        WithdrawalProposalValidationError::SameEpochEquivocation { .. } => {
            "same_epoch_equivocation"
        }
        WithdrawalProposalValidationError::InvalidTransactionBody { .. } => {
            "invalid_transaction_body"
        }
        WithdrawalProposalValidationError::NonContiguousEpoch { .. } => "non_contiguous_epoch",
        WithdrawalProposalValidationError::LiveAttemptExists { .. } => "live_attempt_exists",
        WithdrawalProposalValidationError::WrongAssembler { .. } => "wrong_assembler",
        WithdrawalProposalValidationError::Store(_) => "store_error",
    }
}

/// Maps proposal validation outcomes onto stable transport status strings.
fn validation_outcome_status(outcome: WithdrawalProposalValidationOutcome) -> &'static str {
    match outcome {
        WithdrawalProposalValidationOutcome::Inserted => "inserted",
        WithdrawalProposalValidationOutcome::Replay => "replay",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use nockchain_math::belt::Belt;
    use nockchain_math::crypto::cheetah::{ch_scal_big, trunc_g_order, A_GEN, G_ORDER};
    use nockchain_math::tip5::hash::hash_varlen;
    use nockchain_math::zoon::zset::ZSet;
    use nockchain_types::tx_engine::common::Hash as Tip5Hash;
    use nockchain_types::v1::{
        LockPrimitive, Name, PkhSignatureEntry, SchnorrPubkey, SchnorrSignature,
    };
    use num_bigint::BigUint;
    use prost::Message;
    use tempfile::tempdir;
    use tonic::Request;

    use super::*;
    use crate::deposit::cache::ProposalCache;
    use crate::observability::health::SharedHealthState;
    use crate::observability::status::BridgeStatus;
    use crate::observability::tui::types::AlertSeverity;
    use crate::shared::ingress::proto::bridge_ingress_server::BridgeIngress;
    use crate::shared::ingress::IngressService;
    use crate::shared::runtime::{
        BridgeRuntime, BridgeRuntimeHandle, CauseBuildOutcome, CauseBuilder,
    };
    use crate::shared::signing::BridgeSigner;
    use crate::shared::stop::StopController;
    use crate::withdrawal::proposals::{TrackedWithdrawalRequest, WithdrawalProjectionStore};
    use crate::withdrawal::sequencer::store::WithdrawalSequencerStore;
    use crate::withdrawal::submission::{
        NextPendingWithdrawalOrdering, WithdrawalSequencerCanonicalizationError,
        WithdrawalSequencerPort, WithdrawalSequencerSubmitOutcome,
    };
    use crate::withdrawal::types::NockWithdrawalRequestKernelData;

    #[derive(Clone)]
    struct LocalCanonicalSequencerPort {
        store: Arc<WithdrawalSequencerStore>,
    }

    #[derive(Clone, Default)]
    struct RejectingCanonicalSequencerPort;

    #[derive(Clone, Default)]
    struct StaleFrontierSequencerPort;

    #[derive(Clone, Default)]
    struct RegistrationFailingSequencerPort;

    #[async_trait]
    impl WithdrawalSequencerPort for LocalCanonicalSequencerPort {
        async fn register_withdrawal(
            &self,
            tracked: &TrackedWithdrawalRequest,
        ) -> Result<(), BridgeError> {
            self.store
                .ensure_tracked_withdrawal_ordering(tracked)
                .await?;
            Ok(())
        }

        async fn record_peer_canonical_proposal(
            &self,
            proposal: &WithdrawalProposalData,
            withdrawal_nonce: u64,
            commit_certificate: &WithdrawalCommitCertificate,
            _caller_node_id: u64,
        ) -> Result<(), WithdrawalSequencerCanonicalizationError> {
            self.store
                .ensure_registered_proposal_ordering(proposal, withdrawal_nonce)
                .await?;
            self.store
                .record_peer_canonical_proposal(proposal, Some(commit_certificate), 1)
                .await
                .map_err(|err| WithdrawalSequencerCanonicalizationError::Rejected(err.to_string()))
        }

        async fn record_signed_proposal(
            &self,
            proposal: &WithdrawalProposalData,
            _withdrawal_nonce: u64,
            signer_node_id: u64,
        ) -> Result<(), BridgeError> {
            self.store
                .record_proposal_signed(proposal, signer_node_id, 1)
                .await
                .map_err(|err| BridgeError::Runtime(err.to_string()))
        }

        async fn authorize_proposal(
            &self,
            _proposal: &WithdrawalProposalData,
            _withdrawal_nonce: u64,
            _commit_certificate: &WithdrawalCommitCertificate,
            _caller_node_id: u64,
        ) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn submit_proposal(
            &self,
            _proposal: &WithdrawalProposalData,
            _withdrawal_nonce: u64,
            _caller_node_id: u64,
        ) -> Result<Option<WithdrawalSequencerSubmitOutcome>, BridgeError> {
            Ok(Some(WithdrawalSequencerSubmitOutcome::MempoolAccepted))
        }

        async fn get_next_pending_withdrawal_ordering(
            &self,
        ) -> Result<Option<NextPendingWithdrawalOrdering>, BridgeError> {
            Ok(self.store.next_pending_withdrawal_ordering().await?.map(
                |(id, withdrawal_nonce)| NextPendingWithdrawalOrdering {
                    id,
                    withdrawal_nonce,
                },
            ))
        }

        async fn frontier_allows_withdrawal(&self, id: &WithdrawalId) -> Result<bool, BridgeError> {
            Ok(self.store.frontier_allows_withdrawal(id).await?.allowed())
        }

        async fn get_reserved_withdrawal_inputs(
            &self,
        ) -> Result<Vec<nockchain_types::v1::Name>, BridgeError> {
            self.store.list_reserved_input_names().await
        }

        async fn get_sequenced_withdrawal_status(
            &self,
            id: &WithdrawalId,
        ) -> Result<crate::shared::ingress::proto::SequencedWithdrawalStatusResponse, BridgeError>
        {
            let Some(row) = self.store.fetch_sequenced_withdrawal(id).await? else {
                return Ok(
                    crate::shared::ingress::proto::SequencedWithdrawalStatusResponse {
                        found: false,
                        current_epoch: 0,
                        state: String::new(),
                        proposal_hash: String::new(),
                        authorized_transaction_name: String::new(),
                        withdrawal_nonce: 0,
                        handoff_index: 0,
                        turn_started_base_height: None,

                        current_confirmed_base_height: None,

                        handoff_window_blocks: 0,

                        blocks_until_handoff: None,
                    },
                );
            };
            Ok(
                crate::shared::ingress::proto::SequencedWithdrawalStatusResponse {
                    found: true,
                    current_epoch: row.current_epoch,
                    state: row.state.as_str().to_string(),
                    proposal_hash: row.proposal_hash.unwrap_or_default(),
                    authorized_transaction_name: row
                        .authorized_transaction_name
                        .unwrap_or_default(),
                    withdrawal_nonce: row.withdrawal_nonce.unwrap_or_default(),
                    handoff_index: row.handoff_index,
                    turn_started_base_height: row.turn_started_base_height,

                    current_confirmed_base_height: None,

                    handoff_window_blocks: 0,

                    blocks_until_handoff: None,
                },
            )
        }
    }

    #[async_trait]
    impl WithdrawalSequencerPort for RejectingCanonicalSequencerPort {
        async fn register_withdrawal(
            &self,
            _tracked: &TrackedWithdrawalRequest,
        ) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn record_peer_canonical_proposal(
            &self,
            _proposal: &WithdrawalProposalData,
            _withdrawal_nonce: u64,
            _commit_certificate: &WithdrawalCommitCertificate,
            _caller_node_id: u64,
        ) -> Result<(), WithdrawalSequencerCanonicalizationError> {
            Err(WithdrawalSequencerCanonicalizationError::Rejected(
                "forced canonical rejection".to_string(),
            ))
        }

        async fn get_reserved_withdrawal_inputs(
            &self,
        ) -> Result<Vec<nockchain_types::v1::Name>, BridgeError> {
            Ok(Vec::new())
        }

        async fn authorize_proposal(
            &self,
            _proposal: &WithdrawalProposalData,
            _withdrawal_nonce: u64,
            _commit_certificate: &WithdrawalCommitCertificate,
            _caller_node_id: u64,
        ) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn submit_proposal(
            &self,
            _proposal: &WithdrawalProposalData,
            _withdrawal_nonce: u64,
            _caller_node_id: u64,
        ) -> Result<Option<WithdrawalSequencerSubmitOutcome>, BridgeError> {
            Ok(Some(WithdrawalSequencerSubmitOutcome::MempoolAccepted))
        }

        async fn get_next_pending_withdrawal_ordering(
            &self,
        ) -> Result<Option<NextPendingWithdrawalOrdering>, BridgeError> {
            Ok(None)
        }

        async fn current_live_withdrawal_nonce(&self) -> Result<Option<u64>, BridgeError> {
            Ok(Some(1))
        }

        async fn frontier_allows_withdrawal(
            &self,
            _id: &WithdrawalId,
        ) -> Result<bool, BridgeError> {
            Ok(true)
        }

        async fn get_sequenced_withdrawal_status(
            &self,
            _id: &WithdrawalId,
        ) -> Result<crate::shared::ingress::proto::SequencedWithdrawalStatusResponse, BridgeError>
        {
            Ok(
                crate::shared::ingress::proto::SequencedWithdrawalStatusResponse {
                    found: false,
                    current_epoch: 0,
                    state: String::new(),
                    proposal_hash: String::new(),
                    authorized_transaction_name: String::new(),
                    withdrawal_nonce: 0,
                    handoff_index: 0,
                    turn_started_base_height: None,

                    current_confirmed_base_height: None,

                    handoff_window_blocks: 0,

                    blocks_until_handoff: None,
                },
            )
        }
    }

    #[async_trait]
    impl WithdrawalSequencerPort for StaleFrontierSequencerPort {
        async fn register_withdrawal(
            &self,
            _tracked: &TrackedWithdrawalRequest,
        ) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn record_peer_canonical_proposal(
            &self,
            proposal: &WithdrawalProposalData,
            withdrawal_nonce: u64,
            _commit_certificate: &WithdrawalCommitCertificate,
            _caller_node_id: u64,
        ) -> Result<(), WithdrawalSequencerCanonicalizationError> {
            Err(WithdrawalSequencerCanonicalizationError::Rejected(format!(
                "cannot record canonical proposal for withdrawal {:?} nonce {} while sequencer frontier is {:?} nonce {}",
                proposal.id,
                withdrawal_nonce,
                sample_request().withdrawal_id(),
                withdrawal_nonce + 1
            )))
        }

        async fn record_signed_proposal(
            &self,
            proposal: &WithdrawalProposalData,
            withdrawal_nonce: u64,
            _signer_node_id: u64,
        ) -> Result<(), BridgeError> {
            Err(BridgeError::Runtime(format!(
                "sequencer rejected signed withdrawal {:?} epoch {}: cannot record signed proposal for withdrawal {:?} nonce {} while sequencer frontier is {:?} nonce {}",
                proposal.id,
                proposal.epoch,
                proposal.id,
                withdrawal_nonce,
                sample_request().withdrawal_id(),
                withdrawal_nonce + 1
            )))
        }

        async fn get_reserved_withdrawal_inputs(
            &self,
        ) -> Result<Vec<nockchain_types::v1::Name>, BridgeError> {
            Ok(Vec::new())
        }

        async fn authorize_proposal(
            &self,
            _proposal: &WithdrawalProposalData,
            _withdrawal_nonce: u64,
            _commit_certificate: &WithdrawalCommitCertificate,
            _caller_node_id: u64,
        ) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn submit_proposal(
            &self,
            _proposal: &WithdrawalProposalData,
            _withdrawal_nonce: u64,
            _caller_node_id: u64,
        ) -> Result<Option<WithdrawalSequencerSubmitOutcome>, BridgeError> {
            Ok(Some(WithdrawalSequencerSubmitOutcome::MempoolAccepted))
        }

        async fn get_next_pending_withdrawal_ordering(
            &self,
        ) -> Result<Option<NextPendingWithdrawalOrdering>, BridgeError> {
            Ok(None)
        }

        async fn current_live_withdrawal_nonce(&self) -> Result<Option<u64>, BridgeError> {
            Ok(Some(1))
        }

        async fn frontier_allows_withdrawal(
            &self,
            _id: &WithdrawalId,
        ) -> Result<bool, BridgeError> {
            Ok(true)
        }

        async fn get_sequenced_withdrawal_status(
            &self,
            _id: &WithdrawalId,
        ) -> Result<crate::shared::ingress::proto::SequencedWithdrawalStatusResponse, BridgeError>
        {
            Ok(
                crate::shared::ingress::proto::SequencedWithdrawalStatusResponse {
                    found: false,
                    current_epoch: 0,
                    state: String::new(),
                    proposal_hash: String::new(),
                    authorized_transaction_name: String::new(),
                    withdrawal_nonce: 0,
                    handoff_index: 0,
                    turn_started_base_height: None,

                    current_confirmed_base_height: None,

                    handoff_window_blocks: 0,

                    blocks_until_handoff: None,
                },
            )
        }
    }

    #[async_trait]
    impl WithdrawalSequencerPort for RegistrationFailingSequencerPort {
        async fn register_withdrawal(
            &self,
            _tracked: &TrackedWithdrawalRequest,
        ) -> Result<(), BridgeError> {
            Err(BridgeError::Runtime("sequencer unavailable".to_string()))
        }

        async fn authorize_proposal(
            &self,
            _proposal: &WithdrawalProposalData,
            _withdrawal_nonce: u64,
            _commit_certificate: &WithdrawalCommitCertificate,
            _caller_node_id: u64,
        ) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn submit_proposal(
            &self,
            _proposal: &WithdrawalProposalData,
            _withdrawal_nonce: u64,
            _caller_node_id: u64,
        ) -> Result<Option<WithdrawalSequencerSubmitOutcome>, BridgeError> {
            Ok(Some(WithdrawalSequencerSubmitOutcome::MempoolAccepted))
        }

        async fn get_next_pending_withdrawal_ordering(
            &self,
        ) -> Result<Option<NextPendingWithdrawalOrdering>, BridgeError> {
            Ok(None)
        }

        async fn get_sequenced_withdrawal_status(
            &self,
            _id: &WithdrawalId,
        ) -> Result<crate::shared::ingress::proto::SequencedWithdrawalStatusResponse, BridgeError>
        {
            Ok(
                crate::shared::ingress::proto::SequencedWithdrawalStatusResponse {
                    found: false,
                    current_epoch: 0,
                    state: String::new(),
                    proposal_hash: String::new(),
                    authorized_transaction_name: String::new(),
                    withdrawal_nonce: 0,
                    handoff_index: 0,
                    turn_started_base_height: None,

                    current_confirmed_base_height: None,

                    handoff_window_blocks: 0,

                    blocks_until_handoff: None,
                },
            )
        }
    }

    struct NoOpBuilder;

    impl CauseBuilder for NoOpBuilder {
        fn build_poke(
            &self,
            _event: &crate::shared::runtime::EventEnvelope<crate::shared::runtime::BridgeEvent>,
        ) -> Result<CauseBuildOutcome, BridgeError> {
            Ok(CauseBuildOutcome::Deferred("test".into()))
        }
    }

    fn make_runtime() -> (
        tokio::task::JoinHandle<Result<(), BridgeError>>,
        Arc<BridgeRuntimeHandle>,
    ) {
        let builder = Arc::new(NoOpBuilder);
        let (runtime, handle) = BridgeRuntime::new(builder);
        let handle = Arc::new(handle);
        let task = tokio::spawn(runtime.run());
        (task, handle)
    }

    fn test_bridge_status() -> BridgeStatus {
        let health: SharedHealthState = Arc::new(std::sync::RwLock::new(Vec::new()));
        BridgeStatus::new(health)
    }

    fn test_signer() -> Arc<BridgeSigner> {
        Arc::new(
            BridgeSigner::new(
                "0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318".to_string(),
            )
            .expect("valid test signer"),
        )
    }

    fn sample_node_pkhs() -> Vec<Tip5Hash> {
        vec![
            Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            Tip5Hash([Belt(11), Belt(12), Belt(13), Belt(14), Belt(15)]),
            Tip5Hash([Belt(21), Belt(22), Belt(23), Belt(24), Belt(25)]),
        ]
    }

    const TEST_WITHDRAWAL_OPERATOR_KEYS: [&str; 5] = [
        "0x0000000000000000000000000000000000000000000000000000000000000001",
        "0x0000000000000000000000000000000000000000000000000000000000000002",
        "0x0000000000000000000000000000000000000000000000000000000000000003",
        "0x0000000000000000000000000000000000000000000000000000000000000004",
        "0x0000000000000000000000000000000000000000000000000000000000000005",
    ];

    fn test_withdrawal_signer(node_id: u64) -> Arc<BridgeSigner> {
        Arc::new(
            BridgeSigner::new(TEST_WITHDRAWAL_OPERATOR_KEYS[node_id as usize].to_string())
                .expect("valid withdrawal test signer"),
        )
    }

    fn test_node_eth_addresses(count: usize) -> HashMap<u64, Address> {
        (0..count as u64)
            .map(|node_id| (node_id, test_withdrawal_signer(node_id).address()))
            .collect()
    }

    async fn open_transport(
        node_id: u64,
        min_signers: usize,
        node_pkhs: Vec<Tip5Hash>,
        fallback_policy: WithdrawalFallbackPolicy,
    ) -> (
        Arc<WithdrawalProposalTransport>,
        Arc<WithdrawalSequencerStore>,
        tokio::task::JoinHandle<Result<(), BridgeError>>,
        Arc<BridgeRuntimeHandle>,
        tempfile::TempDir,
    ) {
        let dir = tempdir().expect("tempdir");
        let projection_store = Arc::new(
            WithdrawalProjectionStore::open(
                dir.path()
                    .join(format!("withdrawal-local-state-{node_id}.sqlite")),
            )
            .await
            .expect("withdrawal projection store"),
        );
        let registry = Arc::new(
            WithdrawalProposalRegistry::new_without_transaction_body_validator_for_tests(
                projection_store,
            ),
        );
        let withdrawal_state_store = Arc::new(
            WithdrawalSequencerStore::open(
                dir.path().join(format!("state-store-{node_id}.sqlite")),
            )
            .await
            .expect("withdrawal state store"),
        );
        let transport = Arc::new(
            WithdrawalProposalTransport::new(
                node_id,
                node_pkhs,
                test_node_eth_addresses(5),
                min_signers,
                test_withdrawal_signer(node_id),
                registry,
                fallback_policy,
            )
            .with_sequencer(Arc::new(LocalCanonicalSequencerPort {
                store: withdrawal_state_store.clone(),
            })),
        );
        let (runtime_task, runtime) = make_runtime();
        (
            transport, withdrawal_state_store, runtime_task, runtime, dir,
        )
    }

    async fn open_transport_with_custom_sequencer(
        node_id: u64,
        min_signers: usize,
        node_pkhs: Vec<Tip5Hash>,
        fallback_policy: WithdrawalFallbackPolicy,
        sequencer: Arc<dyn WithdrawalSequencerPort>,
    ) -> (
        Arc<WithdrawalProposalTransport>,
        tokio::task::JoinHandle<Result<(), BridgeError>>,
        Arc<BridgeRuntimeHandle>,
        tempfile::TempDir,
    ) {
        let dir = tempdir().expect("tempdir");
        let projection_store = Arc::new(
            WithdrawalProjectionStore::open(
                dir.path()
                    .join(format!("withdrawal-local-state-{node_id}.sqlite")),
            )
            .await
            .expect("withdrawal projection store"),
        );
        let registry = Arc::new(
            WithdrawalProposalRegistry::new_without_transaction_body_validator_for_tests(
                projection_store,
            ),
        );
        let transport = Arc::new(
            WithdrawalProposalTransport::new(
                node_id,
                node_pkhs,
                test_node_eth_addresses(5),
                min_signers,
                test_withdrawal_signer(node_id),
                registry,
                fallback_policy,
            )
            .with_sequencer(sequencer),
        );
        let (runtime_task, runtime) = make_runtime();
        (transport, runtime_task, runtime, dir)
    }

    #[derive(Clone)]
    struct FakeWithdrawalPeerRpc {
        peers: Arc<HashMap<u64, Arc<WithdrawalProposalTransport>>>,
    }

    #[async_trait]
    impl WithdrawalPeerRpc for FakeWithdrawalPeerRpc {
        async fn broadcast_proposal(
            &self,
            peer: &PeerEndpoint,
            request: WithdrawalProposalBroadcast,
        ) -> Result<WithdrawalProposalBroadcastResponse, BridgeError> {
            let proposal = request
                .proposal
                .as_ref()
                .ok_or_else(|| BridgeError::Runtime("missing proposal envelope".into()))
                .and_then(proposal_from_proto)?;
            let peer_transport = self
                .peers
                .get(&peer.node_id)
                .ok_or_else(|| BridgeError::Runtime("missing fake peer transport".into()))?;
            peer_transport
                .ingest_peer_proposal(request.sender_node_id, &proposal)
                .await
        }

        async fn broadcast_canonicalized(
            &self,
            peer: &PeerEndpoint,
            request: CanonicalWithdrawalProposalBroadcast,
        ) -> Result<CanonicalWithdrawalProposalBroadcastResponse, BridgeError> {
            let proposal = request
                .proposal
                .as_ref()
                .ok_or_else(|| BridgeError::Runtime("missing proposal envelope".into()))
                .and_then(proposal_from_proto)?;
            let peer_transport = self
                .peers
                .get(&peer.node_id)
                .ok_or_else(|| BridgeError::Runtime("missing fake peer transport".into()))?;
            peer_transport
                .ingest_canonicalized_proposal(
                    request.sender_node_id,
                    &proposal,
                    request.commit_certificate.as_ref().ok_or_else(|| {
                        BridgeError::Runtime(
                            "missing canonical withdrawal commit certificate".into(),
                        )
                    })?,
                )
                .await
        }

        async fn broadcast_signed(
            &self,
            peer: &PeerEndpoint,
            request: SignedWithdrawalProposalBroadcast,
        ) -> Result<SignedWithdrawalProposalBroadcastResponse, BridgeError> {
            let proposal = request
                .proposal
                .as_ref()
                .ok_or_else(|| BridgeError::Runtime("missing proposal envelope".into()))
                .and_then(proposal_from_proto)?;
            let peer_transport = self
                .peers
                .get(&peer.node_id)
                .ok_or_else(|| BridgeError::Runtime("missing fake peer transport".into()))?;
            peer_transport
                .ingest_peer_signed_proposal(request.sender_node_id, &proposal)
                .await
        }
    }

    fn sample_base_event_id(start: u8) -> AtomBytes {
        AtomBytes((0..32).map(|offset| start.wrapping_add(offset)).collect())
    }

    fn sample_request() -> NockWithdrawalRequestKernelData {
        NockWithdrawalRequestKernelData {
            base_event_id: sample_base_event_id(0xaa),
            recipient: Tip5Hash([Belt(101), Belt(102), Belt(103), Belt(104), Belt(105)]),
            amount: 123_456,
            base_batch_end: 777,
            as_of: Tip5Hash([Belt(11), Belt(22), Belt(33), Belt(44), Belt(55)]),
        }
    }

    #[tokio::test]
    async fn transport_frontier_hydration_alerts_when_withdrawal_registration_fails() {
        let dir = tempdir().expect("tempdir");
        let projection_store = Arc::new(
            WithdrawalProjectionStore::open(dir.path().join("withdrawal-local-state.sqlite"))
                .await
                .expect("withdrawal projection store"),
        );
        let registry = Arc::new(
            WithdrawalProposalRegistry::new_without_transaction_body_validator_for_tests(
                projection_store,
            ),
        );
        let request = sample_request();
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        let bridge_status = test_bridge_status();
        let transport = WithdrawalProposalTransport::new(
            0,
            sample_node_pkhs(),
            test_node_eth_addresses(5),
            1,
            test_withdrawal_signer(0),
            registry,
            WithdrawalFallbackPolicy::default(),
        )
        .with_sequencer(Arc::new(RegistrationFailingSequencerPort))
        .with_bridge_status(bridge_status.clone());

        let err = transport
            .sequencer_frontier_allows_withdrawal(&request.withdrawal_id())
            .await
            .expect_err("registration failure should fail frontier hydration");
        assert!(err.to_string().contains("sequencer unavailable"));
        let alerts = bridge_status.alerts();
        assert!(alerts.alerts.iter().any(|alert| {
            alert.severity == AlertSeverity::Error
                && alert.title == "Withdrawal Registration Failed"
                && alert.source == "withdrawal-sequencer"
                && alert.message.contains("nonce 1")
        }));
    }

    fn sample_transaction() -> nockchain_types::v1::Transaction {
        const TRANSACTION_JAM: &[u8] = include_bytes!(
            "../../test-fixtures/transactions/9MpGym52AumtwyBxYPyVsWHvcamUYwZkc1Nq7w3cFGF28u8ceVDwt3e.tx"
        );

        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = slab
            .cue_into(TRANSACTION_JAM.to_vec().into())
            .expect("failed to cue transaction fixture");

        let space = nockapp::NounAllocator::noun_space(&slab);
        nockchain_types::v1::Transaction::from_noun(&noun, &space)
            .expect("failed to decode transaction fixture")
    }

    fn sample_proposal(epoch: u64) -> WithdrawalProposalData {
        let request = sample_request();
        let transaction = sample_transaction();
        WithdrawalProposalData {
            id: request.withdrawal_id(),
            recipient: request.recipient.clone(),
            amount: request.amount.saturating_sub(111),
            burned_amount: request.amount,
            base_batch_end: request.base_batch_end,
            epoch,
            snapshot: WithdrawalSnapshot {
                height: 42 + epoch,
                block_id: Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            },
            selected_inputs: transaction.normalized_input_names(),
            transaction,
        }
    }

    fn sample_proposal_for_request(
        request: &NockWithdrawalRequestKernelData,
        epoch: u64,
    ) -> WithdrawalProposalData {
        let mut proposal = sample_proposal(epoch);
        proposal.id = request.withdrawal_id();
        proposal.recipient = request.recipient.clone();
        proposal.amount = request.amount.saturating_sub(111);
        proposal.burned_amount = request.amount;
        proposal.base_batch_end = request.base_batch_end;
        proposal
    }

    async fn register_request_ordering(
        withdrawal_state_store: &WithdrawalSequencerStore,
        request: &NockWithdrawalRequestKernelData,
        withdrawal_nonce: u64,
    ) {
        withdrawal_state_store
            .ensure_tracked_withdrawal_ordering(&TrackedWithdrawalRequest {
                id: request.withdrawal_id(),
                recipient: request.recipient.clone(),
                amount: request.amount,
                base_batch_end: request.base_batch_end,
                withdrawal_nonce,
            })
            .await
            .expect("register withdrawal ordering");
    }

    async fn register_proposal_ordering(
        withdrawal_state_store: &WithdrawalSequencerStore,
        proposal: &WithdrawalProposalData,
        withdrawal_nonce: u64,
    ) {
        withdrawal_state_store
            .ensure_tracked_withdrawal_ordering(&TrackedWithdrawalRequest {
                id: proposal.id.clone(),
                recipient: proposal.recipient.clone(),
                amount: proposal.burned_amount,
                base_batch_end: proposal.base_batch_end,
                withdrawal_nonce,
            })
            .await
            .expect("register withdrawal ordering");
    }

    fn partially_signed_transaction_gap() -> (nockchain_types::v1::Transaction, Name, Tip5Hash) {
        let mut transaction = sample_transaction();
        let gap = {
            let nockchain_types::v1::Transaction::V1(transaction_v1) = &mut transaction;
            let nockchain_types::v1::InputMetadata::SpendConditions(input_metadata) =
                &transaction_v1.metadata.inputs
            else {
                panic!("fixture transaction must use spend-condition metadata");
            };
            let nockchain_types::v1::WitnessData::Witnesses(witness_map) =
                &mut transaction_v1.witness_data
            else {
                panic!("fixture transaction must use witness data");
            };

            let mut gap = None;
            for (name, spend_condition) in &input_metadata.0 {
                let Some(required) = spend_condition.required_pkh_policy() else {
                    continue;
                };
                let Some((_, witness)) = witness_map
                    .0
                    .iter_mut()
                    .find(|(witness_name, _)| witness_name == name)
                else {
                    continue;
                };
                let Some(removed) = witness.pkh_signature.0.pop() else {
                    continue;
                };
                if required.contains(&removed.pkh)
                    && witness
                        .pkh_signature
                        .0
                        .iter()
                        .all(|entry| entry.pkh != removed.pkh)
                    && witness.pkh_signature.0.len() < required.threshold
                {
                    gap = Some((name.clone(), removed.pkh));
                    break;
                }
                witness.pkh_signature.0.push(removed);
            }
            gap
        };

        if let Some((name, removed_hash)) = gap {
            return (transaction, name, removed_hash);
        }

        panic!("fixture transaction does not contain a removable signature");
    }

    fn partially_signed_transaction() -> (nockchain_types::v1::Transaction, Tip5Hash) {
        let (transaction, _name, signer_hash) = partially_signed_transaction_gap();
        (transaction, signer_hash)
    }

    fn threshold_satisfied_input_name(transaction: &nockchain_types::v1::Transaction) -> Name {
        let nockchain_types::v1::Transaction::V1(transaction_v1) = transaction;
        let nockchain_types::v1::InputMetadata::SpendConditions(input_metadata) =
            &transaction_v1.metadata.inputs
        else {
            panic!("fixture transaction must use spend-condition metadata");
        };
        let nockchain_types::v1::WitnessData::Witnesses(witness_map) = &transaction_v1.witness_data
        else {
            panic!("fixture transaction must use witness data");
        };

        for (name, spend_condition) in &input_metadata.0 {
            let Some(required) = spend_condition.required_pkh_policy() else {
                continue;
            };
            let Some((_, witness)) = witness_map
                .0
                .iter()
                .find(|(witness_name, _)| witness_name == name)
            else {
                continue;
            };
            if witness.pkh_signature.0.len() == required.threshold {
                return name.clone();
            }
        }

        panic!("fixture transaction missing threshold-satisfied witness");
    }

    fn install_valid_sender_contribution(
        base: &mut nockchain_types::v1::Transaction,
        signed: &mut nockchain_types::v1::Transaction,
        target_name: &Name,
        removed_hash: &Tip5Hash,
    ) -> Tip5Hash {
        let sk = BigUint::from(7u8);
        let pubkey = SchnorrPubkey(
            ch_scal_big(&biguint_to_ubig(&sk), &A_GEN).expect("derive sender pubkey"),
        );
        let signer_hash = pubkey.pkh_hash().expect("hash signer pubkey");

        replace_required_signer_hash(base, target_name, removed_hash, &signer_hash);
        replace_required_signer_hash(signed, target_name, removed_hash, &signer_hash);

        let spend = spend1_for_name(
            match signed {
                nockchain_types::v1::Transaction::V1(tx) => &tx.spends,
            },
            target_name,
        )
        .expect("signed transaction spend");
        let signature = sign_spend1(spend, &sk, &pubkey);
        replace_new_witness_entry(
            base, signed, target_name, removed_hash, &signer_hash, &pubkey, signature,
        );

        signer_hash
    }

    fn replace_required_signer_hash(
        transaction: &mut nockchain_types::v1::Transaction,
        target_name: &Name,
        removed_hash: &Tip5Hash,
        replacement_hash: &Tip5Hash,
    ) {
        let nockchain_types::v1::Transaction::V1(transaction_v1) = transaction;
        let nockchain_types::v1::InputMetadata::SpendConditions(input_metadata) =
            &mut transaction_v1.metadata.inputs
        else {
            panic!("fixture transaction must use spend-condition metadata");
        };
        let (_, spend_condition) = input_metadata
            .0
            .iter_mut()
            .find(|(name, _)| name == target_name)
            .expect("target spend condition");
        for primitive in &mut spend_condition.0 {
            let LockPrimitive::Pkh(pkh) = primitive else {
                continue;
            };
            let mut updated = pkh.hashes.clone().into_items();
            let Some(position) = updated.iter().position(|hash| hash == removed_hash) else {
                continue;
            };
            updated[position] = replacement_hash.clone();
            pkh.hashes = ZSet::try_from_items(updated).expect("updated signer set");
            return;
        }

        panic!("target spend condition missing PKH primitive");
    }

    fn replace_new_witness_entry(
        base: &nockchain_types::v1::Transaction,
        signed: &mut nockchain_types::v1::Transaction,
        target_name: &Name,
        removed_hash: &Tip5Hash,
        signer_hash: &Tip5Hash,
        pubkey: &SchnorrPubkey,
        signature: SchnorrSignature,
    ) {
        let (
            nockchain_types::v1::Transaction::V1(base_tx),
            nockchain_types::v1::Transaction::V1(signed_tx),
        ) = (base, signed);
        let nockchain_types::v1::WitnessData::Witnesses(base_witness_map) = &base_tx.witness_data
        else {
            panic!("base transaction must use witness data");
        };
        let nockchain_types::v1::WitnessData::Witnesses(signed_witness_map) =
            &mut signed_tx.witness_data
        else {
            panic!("signed transaction must use witness data");
        };

        let base_witness = base_witness_map
            .0
            .iter()
            .find(|(name, _)| name == target_name)
            .map(|(_, witness)| witness)
            .expect("base witness");
        let signed_witness = signed_witness_map
            .0
            .iter_mut()
            .find(|(name, _)| name == target_name)
            .map(|(_, witness)| witness)
            .expect("signed witness");
        let entry = signed_witness
            .pkh_signature
            .0
            .iter_mut()
            .find(|entry| {
                entry.pkh == *removed_hash
                    && base_witness
                        .pkh_signature
                        .0
                        .iter()
                        .all(|base_entry| base_entry.pkh != entry.pkh)
            })
            .expect("new witness entry to replace");
        *entry = PkhSignatureEntry {
            pkh: signer_hash.clone(),
            pubkey: pubkey.clone(),
            signature,
        };
    }

    fn append_allowed_signer_hash(
        transaction: &mut nockchain_types::v1::Transaction,
        target_name: &Name,
        signer_hash: &Tip5Hash,
    ) {
        let nockchain_types::v1::Transaction::V1(transaction_v1) = transaction;
        let nockchain_types::v1::InputMetadata::SpendConditions(input_metadata) =
            &mut transaction_v1.metadata.inputs
        else {
            panic!("fixture transaction must use spend-condition metadata");
        };
        let (_, spend_condition) = input_metadata
            .0
            .iter_mut()
            .find(|(name, _)| name == target_name)
            .expect("target spend condition");
        for primitive in &mut spend_condition.0 {
            let LockPrimitive::Pkh(pkh) = primitive else {
                continue;
            };
            let mut updated = pkh.hashes.clone().into_items();
            if updated.iter().any(|hash| hash == signer_hash) {
                return;
            }
            updated.push(signer_hash.clone());
            pkh.hashes = ZSet::try_from_items(updated).expect("updated signer set");
            return;
        }

        panic!("target spend condition missing PKH primitive");
    }

    fn append_new_witness_entry(
        signed: &mut nockchain_types::v1::Transaction,
        target_name: &Name,
        signer_hash: &Tip5Hash,
        pubkey: &SchnorrPubkey,
        signature: SchnorrSignature,
    ) {
        let nockchain_types::v1::Transaction::V1(signed_tx) = signed;
        let nockchain_types::v1::WitnessData::Witnesses(signed_witness_map) =
            &mut signed_tx.witness_data
        else {
            panic!("signed transaction must use witness data");
        };

        let signed_witness = signed_witness_map
            .0
            .iter_mut()
            .find(|(name, _)| name == target_name)
            .map(|(_, witness)| witness)
            .expect("signed witness");
        signed_witness.pkh_signature.0.push(PkhSignatureEntry {
            pkh: signer_hash.clone(),
            pubkey: pubkey.clone(),
            signature,
        });
    }

    fn install_extra_valid_sender_contribution(
        base: &mut nockchain_types::v1::Transaction,
        signed: &mut nockchain_types::v1::Transaction,
        target_name: &Name,
    ) -> Tip5Hash {
        let sk = BigUint::from(9u8);
        let pubkey = SchnorrPubkey(
            ch_scal_big(&biguint_to_ubig(&sk), &A_GEN).expect("derive sender pubkey"),
        );
        let signer_hash = pubkey.pkh_hash().expect("hash signer pubkey");

        append_allowed_signer_hash(base, target_name, &signer_hash);
        append_allowed_signer_hash(signed, target_name, &signer_hash);
        let spend = spend1_for_name(
            match signed {
                nockchain_types::v1::Transaction::V1(tx) => &tx.spends,
            },
            target_name,
        )
        .expect("signed transaction spend");
        let signature = sign_spend1(spend, &sk, &pubkey);
        append_new_witness_entry(signed, target_name, &signer_hash, &pubkey, signature);

        signer_hash
    }

    fn sign_spend1(
        spend: &nockchain_types::v1::Spend1,
        secret_key: &BigUint,
        pubkey: &SchnorrPubkey,
    ) -> SchnorrSignature {
        let msg =
            nockchain_types::v1::SigHashable::sig_hash_digest(spend).expect("spend signature hash");
        let nonce = BigUint::from(123_456_789u64);
        let r_point = ch_scal_big(&biguint_to_ubig(&nonce), &A_GEN).expect("nonce point");
        let mut hashable = vec![Belt(0); 6 * 4 + 5];
        hashable[0..6].copy_from_slice(&r_point.x.0);
        hashable[6..12].copy_from_slice(&r_point.y.0);
        hashable[12..18].copy_from_slice(&pubkey.0.x.0);
        hashable[18..24].copy_from_slice(&pubkey.0.y.0);
        hashable[24..].copy_from_slice(&msg.0);
        let challenge = ubig_to_biguint(&trunc_g_order(&hash_varlen(&mut hashable)));
        let signature = (nonce + (&challenge * secret_key)) % ubig_to_biguint(&G_ORDER);
        SchnorrSignature {
            chal: biguint_to_belts(&challenge),
            sig: biguint_to_belts(&signature),
        }
    }

    fn biguint_to_belts<const N: usize>(value: &BigUint) -> [Belt; N] {
        let radix = BigUint::from(1u64 << 32);
        let mut remaining = value.clone();
        std::array::from_fn(|_| {
            let rem = &remaining % &radix;
            let belt =
                Belt(u64::try_from(rem).expect("challenge remainder should fit into a belt"));
            remaining /= &radix;
            belt
        })
    }

    fn biguint_to_ubig(value: &BigUint) -> ibig::UBig {
        ibig::UBig::from_be_bytes(&value.to_bytes_be())
    }

    fn ubig_to_biguint(value: &ibig::UBig) -> BigUint {
        BigUint::from_bytes_be(&value.to_be_bytes())
    }

    fn set_new_signature_chal(
        base: &nockchain_types::v1::Transaction,
        signed: &mut nockchain_types::v1::Transaction,
        signer_hash: &Tip5Hash,
        new_chal: Belt,
    ) {
        let (
            nockchain_types::v1::Transaction::V1(base_tx),
            nockchain_types::v1::Transaction::V1(signed_tx),
        ) = (base, signed);
        let nockchain_types::v1::WitnessData::Witnesses(base_witness_map) = &base_tx.witness_data
        else {
            panic!("base transaction must use witness data");
        };
        let nockchain_types::v1::WitnessData::Witnesses(signed_witness_map) =
            &mut signed_tx.witness_data
        else {
            panic!("signed transaction must use witness data");
        };

        for (name, signed_witness) in &mut signed_witness_map.0 {
            let Some(base_witness) = base_witness_map
                .0
                .iter()
                .find(|(base_name, _)| base_name == name)
                .map(|(_, witness)| witness)
            else {
                continue;
            };
            if let Some(entry) = signed_witness.pkh_signature.0.iter_mut().find(|entry| {
                entry.pkh == *signer_hash
                    && base_witness
                        .pkh_signature
                        .0
                        .iter()
                        .all(|base_entry| base_entry.pkh != entry.pkh)
            }) {
                entry.signature.chal[0] = new_chal;
                return;
            }
        }

        panic!("signed transaction missing new signer contribution");
    }

    fn mutate_existing_signature(
        base: &nockchain_types::v1::Transaction,
        signed: &mut nockchain_types::v1::Transaction,
        new_chal: Belt,
    ) {
        let (
            nockchain_types::v1::Transaction::V1(base_tx),
            nockchain_types::v1::Transaction::V1(signed_tx),
        ) = (base, signed);
        let nockchain_types::v1::WitnessData::Witnesses(base_witness_map) = &base_tx.witness_data
        else {
            panic!("base transaction must use witness data");
        };
        let nockchain_types::v1::WitnessData::Witnesses(signed_witness_map) =
            &mut signed_tx.witness_data
        else {
            panic!("signed transaction must use witness data");
        };

        for (name, signed_witness) in &mut signed_witness_map.0 {
            let Some(base_witness) = base_witness_map
                .0
                .iter()
                .find(|(base_name, _)| base_name == name)
                .map(|(_, witness)| witness)
            else {
                continue;
            };
            if let Some(base_entry) = base_witness.pkh_signature.0.first() {
                let entry = signed_witness
                    .pkh_signature
                    .0
                    .iter_mut()
                    .find(|entry| entry.pkh == base_entry.pkh)
                    .expect("signed witness should preserve existing signer");
                entry.signature.chal[0] = new_chal;
                return;
            }
        }

        panic!("base transaction missing existing signer");
    }

    #[test]
    fn withdrawal_proposal_proto_roundtrip_preserves_envelope() {
        let proposal = sample_proposal(0);
        let envelope = proposal_to_proto(&proposal).expect("proposal to proto");
        let msg = WithdrawalProposalBroadcast {
            sender_node_id: 7,
            proposal: Some(envelope),
            proposal_hash: proposal.proposal_hash().expect("proposal hash"),
            timestamp: 123,
        };

        let encoded = msg.encode_to_vec();
        let decoded = WithdrawalProposalBroadcast::decode(encoded.as_slice())
            .expect("decode withdrawal proposal broadcast");
        let proposal = proposal_from_proto(decoded.proposal.as_ref().expect("proposal"))
            .expect("proposal from proto");
        assert_eq!(proposal, sample_proposal(0));
    }

    #[tokio::test]
    async fn broadcast_to_peer_persists_and_canonicalizes_proposal() {
        let request = sample_request();
        let proposal = sample_proposal(0);
        let node_pkhs = sample_node_pkhs();
        let broadcaster_node_id =
            scheduled_assembler_node_id(&proposal.id, proposal.epoch, &node_pkhs)
                .expect("scheduled assembler");
        let peer_node_id = (broadcaster_node_id + 1) % (node_pkhs.len() as u64);
        let (transport_a, state_store_a, runtime_task_a, _runtime_a, _dir_a) = open_transport(
            broadcaster_node_id,
            2,
            node_pkhs.clone(),
            WithdrawalFallbackPolicy::default(),
        )
        .await;
        let (transport_b, state_store_b, runtime_task_b, _runtime_b, _dir_b) = open_transport(
            peer_node_id,
            2,
            node_pkhs,
            WithdrawalFallbackPolicy::default(),
        )
        .await;
        transport_a
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track request on transport a");
        transport_b
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track request on transport b");
        register_request_ordering(&state_store_a, &request, 1).await;
        let fake_rpc = FakeWithdrawalPeerRpc {
            peers: Arc::new(HashMap::from([(peer_node_id, transport_b.clone())])),
        };

        let outcome = transport_a
            .broadcast_proposal_with_rpc(
                &proposal,
                &[PeerEndpoint {
                    node_id: peer_node_id,
                    address: "http://peer-2".to_string(),
                }],
                &fake_rpc,
            )
            .await
            .expect("broadcast proposal");

        let mut expected_node_ids = vec![broadcaster_node_id, peer_node_id];
        expected_node_ids.sort_unstable();
        assert_eq!(outcome.accepted_node_ids, expected_node_ids);
        assert!(outcome.canonicalized);

        let proposal_hash = proposal
            .proposal_hash()
            .expect("proposal hash after broadcast");
        let local = state_store_a
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("local sequenced row")
            .expect("local sequenced withdrawal");
        assert_eq!(local.proposal_hash, Some(proposal_hash.clone()));

        let remote = state_store_b
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("remote sequenced row")
            .expect("remote sequenced withdrawal");
        assert_eq!(remote.proposal_hash, Some(proposal_hash));

        runtime_task_a.abort();
        runtime_task_b.abort();
    }

    #[tokio::test]
    async fn broadcast_reaching_canonicalization_threshold_updates_broadcaster_sequencer_state() {
        let request = sample_request();
        let proposal = sample_proposal(0);
        let node_pkhs = sample_node_pkhs();
        let broadcaster_node_id =
            scheduled_assembler_node_id(&proposal.id, proposal.epoch, &node_pkhs)
                .expect("scheduled assembler");
        let peer_node_id = (broadcaster_node_id + 1) % (node_pkhs.len() as u64);
        let (transport_a, state_store_a, runtime_task_a, _runtime_a, _dir_a) = open_transport(
            broadcaster_node_id,
            2,
            node_pkhs.clone(),
            WithdrawalFallbackPolicy::default(),
        )
        .await;
        let (transport_b, _state_store_b, runtime_task_b, _runtime_b, _dir_b) = open_transport(
            peer_node_id,
            2,
            node_pkhs,
            WithdrawalFallbackPolicy::default(),
        )
        .await;
        transport_a
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track request on transport a");
        transport_b
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track request on transport b");
        register_request_ordering(&state_store_a, &request, 1).await;
        let fake_rpc = FakeWithdrawalPeerRpc {
            peers: Arc::new(HashMap::from([(peer_node_id, transport_b.clone())])),
        };

        let outcome = transport_a
            .broadcast_proposal_with_rpc(
                &proposal,
                &[PeerEndpoint {
                    node_id: peer_node_id,
                    address: "http://peer-2".to_string(),
                }],
                &fake_rpc,
            )
            .await
            .expect("broadcast proposal");

        let mut expected_node_ids = vec![broadcaster_node_id, peer_node_id];
        expected_node_ids.sort_unstable();
        assert_eq!(outcome.accepted_node_ids, expected_node_ids);
        assert!(outcome.canonicalized);
        let sequenced = state_store_a
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal after canonicalization")
            .expect("sequenced withdrawal should remain");
        assert_eq!(sequenced.state, WithdrawalState::PeerCanonical);
        assert_eq!(
            sequenced.proposal_hash.as_deref(),
            Some(proposal.proposal_hash().expect("proposal hash").as_str())
        );
        assert_eq!(
            state_store_a
                .list_reserved_input_names()
                .await
                .expect("list reserved inputs after canonicalization"),
            proposal.selected_inputs
        );

        runtime_task_a.abort();
        runtime_task_b.abort();
    }

    #[tokio::test]
    async fn broadcaster_expires_prepared_attempt_when_local_reserved_input_precheck_fails() {
        let request = sample_request();
        let proposal = sample_proposal(0);
        let mut blocking_request = sample_request();
        blocking_request.base_event_id = sample_base_event_id(0x88);
        blocking_request.as_of = Tip5Hash([Belt(61), Belt(62), Belt(63), Belt(64), Belt(65)]);
        let mut blocking_proposal = sample_proposal(0);
        blocking_proposal.id = blocking_request.withdrawal_id();
        blocking_proposal.selected_inputs = proposal.selected_inputs.clone();

        let node_pkhs = sample_node_pkhs();
        let broadcaster_node_id =
            scheduled_assembler_node_id(&proposal.id, proposal.epoch, &node_pkhs)
                .expect("scheduled assembler");
        let (transport, withdrawal_state_store, runtime_task, _runtime, _dir) = open_transport(
            broadcaster_node_id,
            1,
            node_pkhs,
            WithdrawalFallbackPolicy::default(),
        )
        .await;

        transport
            .registry()
            .track_withdrawal_request(&blocking_request)
            .await
            .expect("track blocking request");
        transport
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track local request");
        transport
            .registry()
            .validate_and_cache_prepared(&proposal)
            .await
            .expect("persist local proposal");
        transport
            .registry()
            .mark_proposal_prepared(&proposal)
            .await
            .expect("mark local proposal prepared");

        register_proposal_ordering(&withdrawal_state_store, &blocking_proposal, 1).await;
        register_proposal_ordering(&withdrawal_state_store, &proposal, 2).await;
        withdrawal_state_store
            .record_proposal_canonicalized(&blocking_proposal, 100)
            .await
            .expect("record blocking canonicalization");
        withdrawal_state_store
            .record_proposal_authorized(&blocking_proposal)
            .await
            .expect("record blocking authorization");
        withdrawal_state_store
            .record_submit_outcome(
                &blocking_proposal,
                WithdrawalState::MempoolAccepted,
                1,
                111,
                None,
            )
            .await
            .expect("record blocking mempool acceptance");

        let err = transport
            .broadcast_proposal_to_peers(&proposal, &[])
            .await
            .expect_err("local reserved-input precheck should fail");
        assert!(err.to_string().contains("before sequencer submission"));
        let live = transport
            .registry()
            .fetch_live_withdrawal(&proposal.id)
            .await
            .expect("fetch local live row");
        assert_eq!(live, None);
        let sequenced = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch local sequenced row")
            .expect("local sequenced row exists");
        assert_eq!(sequenced.state, WithdrawalState::Pending);
        assert_eq!(sequenced.proposal_hash, None);

        runtime_task.abort();
    }

    #[tokio::test]
    async fn broadcast_proposal_to_peers_skips_future_nonce_above_frontier() {
        // Regossip is active work. A locally prepared proposal above the
        // sequencer frontier must remain passive until lower nonces release.
        let request = sample_request();
        let mut future_request = sample_request();
        future_request.base_event_id = sample_base_event_id(0xcc);
        future_request.as_of = Tip5Hash([Belt(161), Belt(162), Belt(163), Belt(164), Belt(165)]);
        let proposal = sample_proposal_for_request(&future_request, 0);

        let node_pkhs = sample_node_pkhs();
        let broadcaster_node_id =
            scheduled_assembler_node_id(&proposal.id, proposal.epoch, &node_pkhs)
                .expect("scheduled assembler");
        let (transport, withdrawal_state_store, runtime_task, _runtime, _dir) = open_transport(
            broadcaster_node_id,
            1,
            node_pkhs,
            WithdrawalFallbackPolicy::default(),
        )
        .await;

        for tracked in [&request, &future_request] {
            transport
                .registry()
                .track_withdrawal_request(tracked)
                .await
                .expect("track request");
        }
        transport
            .registry()
            .validate_and_cache_prepared(&proposal)
            .await
            .expect("persist future proposal");
        transport
            .registry()
            .mark_proposal_prepared(&proposal)
            .await
            .expect("mark future proposal prepared");
        register_request_ordering(&withdrawal_state_store, &request, 1).await;
        register_request_ordering(&withdrawal_state_store, &future_request, 2).await;

        let outcome = transport
            .broadcast_proposal_to_peers(&proposal, &[])
            .await
            .expect("future nonce proposal should be passive");

        assert_eq!(
            outcome.proposal_hash,
            proposal.proposal_hash().expect("proposal hash")
        );
        assert!(outcome.accepted_node_ids.is_empty());
        assert!(!outcome.canonicalized);
        runtime_task.abort();
    }

    #[tokio::test]
    async fn broadcast_proposal_to_peers_skips_stale_nonce_below_frontier() {
        // A stale local prepared proposal below the sequencer frontier must not
        // regossip after its nonce has been released at the sequencer.
        let stale_request = sample_request();
        let mut frontier_request = sample_request();
        frontier_request.base_event_id = sample_base_event_id(0xcc);
        frontier_request.as_of = Tip5Hash([Belt(171), Belt(172), Belt(173), Belt(174), Belt(175)]);
        let stale_proposal = sample_proposal_for_request(&stale_request, 0);

        let node_pkhs = sample_node_pkhs();
        let broadcaster_node_id =
            scheduled_assembler_node_id(&stale_proposal.id, stale_proposal.epoch, &node_pkhs)
                .expect("scheduled assembler");
        let (transport, withdrawal_state_store, runtime_task, _runtime, _dir) = open_transport(
            broadcaster_node_id,
            1,
            node_pkhs,
            WithdrawalFallbackPolicy::default(),
        )
        .await;

        for tracked in [&stale_request, &frontier_request] {
            transport
                .registry()
                .track_withdrawal_request(tracked)
                .await
                .expect("track request");
        }
        transport
            .registry()
            .validate_and_cache_prepared(&stale_proposal)
            .await
            .expect("persist stale proposal");
        transport
            .registry()
            .mark_proposal_prepared(&stale_proposal)
            .await
            .expect("mark stale proposal prepared");
        register_request_ordering(&withdrawal_state_store, &stale_request, 1).await;
        register_request_ordering(&withdrawal_state_store, &frontier_request, 2).await;
        withdrawal_state_store
            .record_proposal_canonicalized(&stale_proposal, 100)
            .await
            .expect("record stale canonicalization");
        withdrawal_state_store
            .record_proposal_authorized(&stale_proposal)
            .await
            .expect("record stale authorization");
        withdrawal_state_store
            .record_submit_outcome(
                &stale_proposal,
                WithdrawalState::MempoolAccepted,
                1,
                111,
                None,
            )
            .await
            .expect("record stale mempool acceptance");

        let outcome = transport
            .broadcast_proposal_to_peers(&stale_proposal, &[])
            .await
            .expect("stale nonce proposal should be passive");

        assert_eq!(
            outcome.proposal_hash,
            stale_proposal.proposal_hash().expect("proposal hash")
        );
        assert!(outcome.accepted_node_ids.is_empty());
        assert!(!outcome.canonicalized);
        runtime_task.abort();
    }

    #[tokio::test]
    async fn broadcast_proposal_to_peers_skips_when_assembler_handoff_advances() {
        let request = sample_request();
        let proposal = sample_proposal(0);
        let node_pkhs = sample_node_pkhs();
        let broadcaster_node_id =
            scheduled_assembler_node_id(&proposal.id, proposal.epoch, &node_pkhs)
                .expect("scheduled assembler");
        let next_handoff_index = (1..=(node_pkhs.len() as u64 * 2))
            .find(|handoff_index| {
                scheduled_assembler_turn_node_id(
                    &proposal.id, proposal.epoch, *handoff_index, &node_pkhs,
                )
                .expect("scheduled handoff assembler")
                    != broadcaster_node_id
            })
            .expect("handoff to another assembler");
        let (transport, withdrawal_state_store, runtime_task, _runtime, _dir) = open_transport(
            broadcaster_node_id,
            1,
            node_pkhs,
            WithdrawalFallbackPolicy::default(),
        )
        .await;

        transport
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        transport
            .registry()
            .validate_and_cache_prepared(&proposal)
            .await
            .expect("persist proposal");
        transport
            .registry()
            .mark_proposal_prepared(&proposal)
            .await
            .expect("mark proposal prepared");
        register_request_ordering(&withdrawal_state_store, &request, 1).await;
        withdrawal_state_store
            .record_precanonical_handoff_for_id(
                &proposal.id, proposal.epoch, next_handoff_index, 10,
            )
            .await
            .expect("advance pre-canonical handoff");

        let outcome = transport
            .broadcast_proposal_to_peers(&proposal, &[])
            .await
            .expect("stale assembler should be passive");

        assert_eq!(
            outcome.proposal_hash,
            proposal.proposal_hash().expect("proposal hash")
        );
        assert!(outcome.accepted_node_ids.is_empty());
        assert!(!outcome.canonicalized);
        runtime_task.abort();
    }

    #[tokio::test]
    async fn broadcaster_treats_stale_frontier_canonical_rejection_as_passive() {
        let request = sample_request();
        let proposal = sample_proposal(0);
        let node_pkhs = sample_node_pkhs();
        let broadcaster_node_id =
            scheduled_assembler_node_id(&proposal.id, proposal.epoch, &node_pkhs)
                .expect("scheduled assembler");
        let (transport, runtime_task, _runtime, _dir) = open_transport_with_custom_sequencer(
            broadcaster_node_id,
            1,
            node_pkhs,
            WithdrawalFallbackPolicy::default(),
            Arc::new(StaleFrontierSequencerPort),
        )
        .await;

        transport
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        transport
            .registry()
            .validate_and_cache_prepared(&proposal)
            .await
            .expect("persist proposal");
        transport
            .registry()
            .mark_proposal_prepared(&proposal)
            .await
            .expect("mark proposal prepared");

        let outcome = transport
            .broadcast_proposal_to_peers(&proposal, &[])
            .await
            .expect("stale sequencer frontier should be passive");

        assert_eq!(
            outcome.proposal_hash,
            proposal.proposal_hash().expect("proposal hash")
        );
        assert_eq!(outcome.accepted_node_ids, vec![broadcaster_node_id]);
        assert!(!outcome.canonicalized);
        let live = transport
            .registry()
            .fetch_live_withdrawal(&proposal.id)
            .await
            .expect("fetch live withdrawal")
            .expect("prepared attempt remains live");
        assert_eq!(live.state, WithdrawalState::Prepared);

        runtime_task.abort();
    }

    #[tokio::test]
    async fn broadcaster_expires_prepared_attempt_when_sequencer_rejects_canonical_record() {
        let request = sample_request();
        let proposal = sample_proposal(0);
        let node_pkhs = sample_node_pkhs();
        let broadcaster_node_id =
            scheduled_assembler_node_id(&proposal.id, proposal.epoch, &node_pkhs)
                .expect("scheduled assembler");
        let (transport, runtime_task, _runtime, _dir) = open_transport_with_custom_sequencer(
            broadcaster_node_id,
            1,
            node_pkhs,
            WithdrawalFallbackPolicy::default(),
            Arc::new(RejectingCanonicalSequencerPort),
        )
        .await;

        transport
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        transport
            .registry()
            .validate_and_cache_prepared(&proposal)
            .await
            .expect("persist proposal");
        transport
            .registry()
            .mark_proposal_prepared(&proposal)
            .await
            .expect("mark proposal prepared");

        let err = transport
            .broadcast_proposal_to_peers(&proposal, &[])
            .await
            .expect_err("sequencer rejection should fail broadcast");
        assert!(err.to_string().contains("forced canonical rejection"));
        let live = transport
            .registry()
            .fetch_live_withdrawal(&proposal.id)
            .await
            .expect("fetch live withdrawal");
        assert_eq!(live, None);

        runtime_task.abort();
    }

    #[tokio::test]
    async fn record_signed_progress_treats_stale_frontier_rejection_as_passive() {
        let request = sample_request();
        let proposal = sample_proposal(0);
        let (transport, runtime_task, _runtime, _dir) = open_transport_with_custom_sequencer(
            0,
            1,
            sample_node_pkhs(),
            WithdrawalFallbackPolicy::default(),
            Arc::new(StaleFrontierSequencerPort),
        )
        .await;

        transport
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        transport
            .registry()
            .validate_and_cache_prepared(&proposal)
            .await
            .expect("persist proposal");
        transport
            .registry()
            .mark_proposal_prepared(&proposal)
            .await
            .expect("mark proposal prepared");
        transport
            .registry()
            .mark_proposal_canonical(&proposal)
            .await
            .expect("mark proposal canonical");

        transport
            .record_signed_progress_at_sequencer(&proposal, 0)
            .await
            .expect("stale sequencer frontier should be passive");

        runtime_task.abort();
    }

    #[tokio::test]
    async fn ingest_peer_signed_proposal_rejects_post_canonical_input_replacement() {
        let request = sample_request();
        let base_proposal = sample_proposal(0);
        let mut replaced_inputs = base_proposal.clone();
        replaced_inputs.selected_inputs = vec![Name::new(
            Tip5Hash([Belt(901), Belt(902), Belt(903), Belt(904), Belt(905)]),
            Tip5Hash([Belt(911), Belt(912), Belt(913), Belt(914), Belt(915)]),
        )];

        let (transport, _withdrawal_state_store, runtime_task, _runtime, _dir) = open_transport(
            0,
            1,
            sample_node_pkhs(),
            WithdrawalFallbackPolicy::default(),
        )
        .await;
        transport
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        transport
            .registry()
            .validate_and_cache_prepared(&base_proposal)
            .await
            .expect("persist base proposal");
        transport
            .registry()
            .mark_proposal_prepared(&base_proposal)
            .await
            .expect("mark base proposal prepared");
        transport
            .registry()
            .mark_proposal_canonical(&base_proposal)
            .await
            .expect("mark base proposal canonical");

        let response = transport
            .ingest_peer_signed_proposal(1, &replaced_inputs)
            .await
            .expect("ingest signed proposal with replaced inputs");

        assert!(!response.accepted);
        assert_eq!(response.status, "proposal_mismatch");
        assert!(response
            .error
            .contains("diverged from persisted base proposal"));
        let live = transport
            .registry()
            .fetch_live_withdrawal(&base_proposal.id)
            .await
            .expect("fetch live withdrawal after mismatch")
            .expect("canonical proposal remains live");
        assert_eq!(live.state, WithdrawalState::PeerCanonical);
        assert_eq!(
            live.proposal_hash.as_deref(),
            Some(
                base_proposal
                    .proposal_hash()
                    .expect("base proposal hash")
                    .as_str()
            )
        );

        runtime_task.abort();
    }

    #[tokio::test]
    async fn ingest_peer_signed_proposal_accepts_valid_sender_witness_contribution() {
        let request = sample_request();
        let mut base_proposal = sample_proposal(0);
        let mut signed_proposal = sample_proposal(0);
        let (base_transaction, target_name, removed_hash) = partially_signed_transaction_gap();
        base_proposal.transaction = base_transaction;
        base_proposal.selected_inputs = base_proposal.transaction.normalized_input_names();
        let signer_hash = install_valid_sender_contribution(
            &mut base_proposal.transaction, &mut signed_proposal.transaction, &target_name,
            &removed_hash,
        );
        signed_proposal.selected_inputs = signed_proposal.transaction.normalized_input_names();

        let node_pkhs = vec![
            Tip5Hash([Belt(41), Belt(42), Belt(43), Belt(44), Belt(45)]),
            signer_hash,
            Tip5Hash([Belt(61), Belt(62), Belt(63), Belt(64), Belt(65)]),
        ];
        let sender_node_id = 1;
        let (transport, _withdrawal_state_store, runtime_task, _runtime, _dir) =
            open_transport(0, 1, node_pkhs, WithdrawalFallbackPolicy::default()).await;
        transport
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        transport
            .registry()
            .validate_and_cache_prepared(&base_proposal)
            .await
            .expect("persist base proposal");
        transport
            .registry()
            .mark_proposal_prepared(&base_proposal)
            .await
            .expect("mark base proposal prepared");
        transport
            .registry()
            .mark_proposal_canonical(&base_proposal)
            .await
            .expect("mark base proposal canonical");

        let response = transport
            .ingest_peer_signed_proposal(sender_node_id, &signed_proposal)
            .await
            .expect("ingest valid signed proposal");

        assert!(response.accepted);
        assert_eq!(response.status, "inserted");
        assert!(transport
            .registry()
            .has_signed_proposal_from_signer(
                &base_proposal.id,
                base_proposal.epoch,
                &base_proposal.proposal_hash().expect("proposal hash"),
                sender_node_id,
            )
            .await
            .expect("load signed contribution replay marker"));

        runtime_task.abort();
    }

    #[tokio::test]
    async fn ingest_peer_signed_proposal_rejects_missing_new_witness_contribution() {
        let request = sample_request();
        let mut base_proposal = sample_proposal(0);
        let (base_transaction, signer_hash) = partially_signed_transaction();
        base_proposal.transaction = base_transaction;
        base_proposal.selected_inputs = base_proposal.transaction.normalized_input_names();
        let signed_proposal = base_proposal.clone();

        let node_pkhs =
            vec![Tip5Hash([Belt(41), Belt(42), Belt(43), Belt(44), Belt(45)]), signer_hash];
        let (transport, _withdrawal_state_store, runtime_task, _runtime, _dir) =
            open_transport(0, 1, node_pkhs, WithdrawalFallbackPolicy::default()).await;
        transport
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        transport
            .registry()
            .validate_and_cache_prepared(&base_proposal)
            .await
            .expect("persist base proposal");
        transport
            .registry()
            .mark_proposal_prepared(&base_proposal)
            .await
            .expect("mark base proposal prepared");
        transport
            .registry()
            .mark_proposal_canonical(&base_proposal)
            .await
            .expect("mark base proposal canonical");

        let response = transport
            .ingest_peer_signed_proposal(1, &signed_proposal)
            .await
            .expect("ingest signed proposal with no new delta");

        assert!(!response.accepted);
        assert_eq!(response.status, "invalid_signer_delta");
        assert!(response.error.contains("no new witness contribution"));

        runtime_task.abort();
    }

    #[tokio::test]
    async fn ingest_peer_signed_proposal_rejects_wrong_signer_delta() {
        let request = sample_request();
        let mut base_proposal = sample_proposal(0);
        let mut signed_proposal = sample_proposal(0);
        let (base_transaction, target_name, removed_hash) = partially_signed_transaction_gap();
        base_proposal.transaction = base_transaction;
        base_proposal.selected_inputs = base_proposal.transaction.normalized_input_names();
        let signer_hash = install_valid_sender_contribution(
            &mut base_proposal.transaction, &mut signed_proposal.transaction, &target_name,
            &removed_hash,
        );
        signed_proposal.selected_inputs = signed_proposal.transaction.normalized_input_names();

        let node_pkhs = vec![
            Tip5Hash([Belt(41), Belt(42), Belt(43), Belt(44), Belt(45)]),
            Tip5Hash([Belt(51), Belt(52), Belt(53), Belt(54), Belt(55)]),
            signer_hash,
        ];
        let (transport, _withdrawal_state_store, runtime_task, _runtime, _dir) =
            open_transport(0, 1, node_pkhs, WithdrawalFallbackPolicy::default()).await;
        transport
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        transport
            .registry()
            .validate_and_cache_prepared(&base_proposal)
            .await
            .expect("persist base proposal");
        transport
            .registry()
            .mark_proposal_prepared(&base_proposal)
            .await
            .expect("mark base proposal prepared");
        transport
            .registry()
            .mark_proposal_canonical(&base_proposal)
            .await
            .expect("mark base proposal canonical");

        let response = transport
            .ingest_peer_signed_proposal(1, &signed_proposal)
            .await
            .expect("ingest wrong signer contribution");

        assert!(!response.accepted);
        assert_eq!(response.status, "invalid_signer_delta");
        assert!(response.error.contains("expected"));

        runtime_task.abort();
    }

    #[tokio::test]
    async fn ingest_peer_signed_proposal_rejects_over_signed_pkh_witness() {
        let request = sample_request();
        let mut base_proposal = sample_proposal(0);
        let mut signed_proposal = sample_proposal(0);
        let base_transaction = sample_transaction();
        let target_name = threshold_satisfied_input_name(&base_transaction);
        base_proposal.transaction = base_transaction.clone();
        signed_proposal.transaction = base_transaction;
        base_proposal.selected_inputs = base_proposal.transaction.normalized_input_names();
        let signer_hash = install_extra_valid_sender_contribution(
            &mut base_proposal.transaction, &mut signed_proposal.transaction, &target_name,
        );
        signed_proposal.selected_inputs = signed_proposal.transaction.normalized_input_names();

        let node_pkhs =
            vec![Tip5Hash([Belt(41), Belt(42), Belt(43), Belt(44), Belt(45)]), signer_hash];
        let (transport, _withdrawal_state_store, runtime_task, _runtime, _dir) =
            open_transport(0, 1, node_pkhs, WithdrawalFallbackPolicy::default()).await;
        transport
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        transport
            .registry()
            .validate_and_cache_prepared(&base_proposal)
            .await
            .expect("persist base proposal");
        transport
            .registry()
            .mark_proposal_prepared(&base_proposal)
            .await
            .expect("mark base proposal prepared");
        transport
            .registry()
            .mark_proposal_canonical(&base_proposal)
            .await
            .expect("mark base proposal canonical");

        let response = transport
            .ingest_peer_signed_proposal(1, &signed_proposal)
            .await
            .expect("ingest over-signed proposal");

        assert!(!response.accepted);
        assert_eq!(response.status, "invalid_signer_delta");
        assert!(response.error.contains("exceeded PKH threshold"));

        runtime_task.abort();
    }

    #[tokio::test]
    async fn ingest_peer_signed_proposal_rejects_invalid_new_witness_signature() {
        let request = sample_request();
        let mut base_proposal = sample_proposal(0);
        let mut signed_proposal = sample_proposal(0);
        let (base_transaction, target_name, removed_hash) = partially_signed_transaction_gap();
        base_proposal.transaction = base_transaction;
        base_proposal.selected_inputs = base_proposal.transaction.normalized_input_names();
        let signer_hash = install_valid_sender_contribution(
            &mut base_proposal.transaction, &mut signed_proposal.transaction, &target_name,
            &removed_hash,
        );
        signed_proposal.selected_inputs = signed_proposal.transaction.normalized_input_names();
        set_new_signature_chal(
            &base_proposal.transaction,
            &mut signed_proposal.transaction,
            &signer_hash,
            Belt(1),
        );

        let node_pkhs =
            vec![Tip5Hash([Belt(41), Belt(42), Belt(43), Belt(44), Belt(45)]), signer_hash];
        let (transport, _withdrawal_state_store, runtime_task, _runtime, _dir) =
            open_transport(0, 1, node_pkhs, WithdrawalFallbackPolicy::default()).await;
        transport
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        transport
            .registry()
            .validate_and_cache_prepared(&base_proposal)
            .await
            .expect("persist base proposal");
        transport
            .registry()
            .mark_proposal_prepared(&base_proposal)
            .await
            .expect("mark base proposal prepared");
        transport
            .registry()
            .mark_proposal_canonical(&base_proposal)
            .await
            .expect("mark base proposal canonical");

        let response = transport
            .ingest_peer_signed_proposal(1, &signed_proposal)
            .await
            .expect("ingest signed proposal with invalid signature");

        assert!(!response.accepted);
        assert_eq!(response.status, "invalid_witness_signature");

        runtime_task.abort();
    }

    #[tokio::test]
    async fn ingest_peer_signed_proposal_rejects_mutated_existing_witness_entry() {
        let request = sample_request();
        let mut base_proposal = sample_proposal(0);
        let mut signed_proposal = sample_proposal(0);
        let (base_transaction, target_name, removed_hash) = partially_signed_transaction_gap();
        base_proposal.transaction = base_transaction;
        base_proposal.selected_inputs = base_proposal.transaction.normalized_input_names();
        let signer_hash = install_valid_sender_contribution(
            &mut base_proposal.transaction, &mut signed_proposal.transaction, &target_name,
            &removed_hash,
        );
        signed_proposal.selected_inputs = signed_proposal.transaction.normalized_input_names();
        mutate_existing_signature(
            &base_proposal.transaction,
            &mut signed_proposal.transaction,
            Belt(1),
        );

        let node_pkhs =
            vec![Tip5Hash([Belt(41), Belt(42), Belt(43), Belt(44), Belt(45)]), signer_hash];
        let (transport, _withdrawal_state_store, runtime_task, _runtime, _dir) =
            open_transport(0, 1, node_pkhs, WithdrawalFallbackPolicy::default()).await;
        transport
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        transport
            .registry()
            .validate_and_cache_prepared(&base_proposal)
            .await
            .expect("persist base proposal");
        transport
            .registry()
            .mark_proposal_prepared(&base_proposal)
            .await
            .expect("mark base proposal prepared");
        transport
            .registry()
            .mark_proposal_canonical(&base_proposal)
            .await
            .expect("mark base proposal canonical");

        let response = transport
            .ingest_peer_signed_proposal(1, &signed_proposal)
            .await
            .expect("ingest signed proposal with mutated existing witness");

        assert!(!response.accepted);
        assert_eq!(response.status, "invalid_signer_delta");
        assert!(response.error.contains("changed existing signer"));

        runtime_task.abort();
    }

    #[tokio::test]
    async fn ingress_routes_withdrawal_proposal_directly_to_transport() {
        let request = sample_request();
        let proposal = sample_proposal(0);
        let node_pkhs = sample_node_pkhs();
        let sender_node_id = scheduled_assembler_node_id(&proposal.id, proposal.epoch, &node_pkhs)
            .expect("scheduled assembler");
        let (transport, _state_store, runtime_task, runtime, _dir) =
            open_transport(1, 1, node_pkhs, WithdrawalFallbackPolicy::default()).await;
        transport
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track request");

        let service = IngressService::new(
            1,
            runtime,
            test_signer(),
            Arc::new(ProposalCache::new()),
            test_bridge_status(),
            HashMap::new(),
            StopController::new().0,
            Vec::new(),
        )
        .with_withdrawal_transport(transport.clone());

        let proposal_hash = proposal.proposal_hash().expect("proposal hash");
        let response = service
            .broadcast_withdrawal_proposal(Request::new(WithdrawalProposalBroadcast {
                sender_node_id,
                proposal: Some(proposal_to_proto(&proposal).expect("proposal proto")),
                proposal_hash: proposal_hash.clone(),
                timestamp: 555,
            }))
            .await
            .expect("ingress response")
            .into_inner();

        assert!(response.accepted);
        assert_eq!(response.proposal_hash, proposal_hash);
        assert_eq!(response.responder_node_id, 1);
        assert!(transport
            .registry()
            .fetch_cached_proposal(proposal.id.clone(), proposal.epoch)
            .await
            .expect("fetch cached proposal")
            .is_some());

        runtime_task.abort();
    }

    #[tokio::test]
    async fn rejects_epoch_advance_while_live_peer_canonical_attempt_exists() {
        let request = sample_request();
        let proposal_epoch_0 = sample_proposal(0);
        let mut proposal_epoch_1 = sample_proposal(1);
        proposal_epoch_1.transaction = proposal_epoch_0.transaction.clone();
        let sender_epoch_1 = scheduled_assembler_node_id(
            &proposal_epoch_1.id,
            proposal_epoch_1.epoch,
            &sample_node_pkhs(),
        )
        .expect("scheduled epoch 1 assembler");

        let (transport, withdrawal_state_store, _runtime_task, _runtime, _dir) = open_transport(
            1,
            1,
            sample_node_pkhs(),
            WithdrawalFallbackPolicy::default(),
        )
        .await;
        transport
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        transport
            .registry()
            .validate_and_cache_prepared(&proposal_epoch_0)
            .await
            .expect("persist epoch 0");
        transport
            .registry()
            .mark_proposal_canonical(&proposal_epoch_0)
            .await
            .expect("mark epoch 0 canonical");
        register_request_ordering(&withdrawal_state_store, &request, 1).await;
        withdrawal_state_store
            .record_proposal_canonicalized(&proposal_epoch_0, 100)
            .await
            .expect("canonicalize epoch 0");

        let response = transport
            .ingest_peer_proposal(sender_epoch_1, &proposal_epoch_1)
            .await
            .expect("ingest epoch 1");

        assert!(!response.accepted);
        assert_eq!(response.status, "live_attempt_exists");
        assert!(response.error.contains("live"));
        assert!(transport
            .registry()
            .fetch_cached_proposal(proposal_epoch_1.id.clone(), proposal_epoch_1.epoch)
            .await
            .expect("fetch epoch 1")
            .is_none());
    }

    #[tokio::test]
    async fn rejects_epoch_advance_even_when_live_attempt_is_stale() {
        let request = sample_request();
        let proposal_epoch_0 = sample_proposal(0);
        let mut proposal_epoch_1 = sample_proposal(1);
        proposal_epoch_1.transaction = proposal_epoch_0.transaction.clone();
        let node_pkhs = sample_node_pkhs();
        let sender_epoch_1 =
            scheduled_assembler_node_id(&proposal_epoch_1.id, proposal_epoch_1.epoch, &node_pkhs)
                .expect("scheduled epoch 1 assembler");

        let (transport, withdrawal_state_store, _runtime_task, _runtime, _dir) = open_transport(
            1,
            1,
            node_pkhs,
            WithdrawalFallbackPolicy {
                assembly_timeout_blocks: 0,
                submission_timeout_blocks: 30,
            },
        )
        .await;
        transport
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        transport
            .registry()
            .validate_and_cache_prepared(&proposal_epoch_0)
            .await
            .expect("persist epoch 0");
        transport
            .registry()
            .mark_proposal_canonical(&proposal_epoch_0)
            .await
            .expect("mark epoch 0 canonical");
        register_request_ordering(&withdrawal_state_store, &request, 1).await;
        withdrawal_state_store
            .record_proposal_canonicalized(&proposal_epoch_0, 100)
            .await
            .expect("canonicalize epoch 0");

        let response = transport
            .ingest_peer_proposal(sender_epoch_1, &proposal_epoch_1)
            .await
            .expect("ingest epoch 1");

        assert!(!response.accepted);
        assert_eq!(response.status, "live_attempt_exists");
        assert!(response.error.contains("live"));
        assert!(transport
            .registry()
            .fetch_cached_proposal(proposal_epoch_1.id.clone(), proposal_epoch_1.epoch)
            .await
            .expect("fetch epoch 1")
            .is_none());
        let live = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal_epoch_1.id)
            .await
            .expect("fetch live withdrawal")
            .expect("epoch 0 attempt remains live");
        assert_eq!(live.current_epoch, 0);
    }

    #[tokio::test]
    async fn rejects_proposal_from_wrong_epoch_leader() {
        let request = sample_request();
        let proposal = sample_proposal(0);
        let node_pkhs = sample_node_pkhs();
        let expected_sender = scheduled_assembler_node_id(&proposal.id, proposal.epoch, &node_pkhs)
            .expect("scheduled assembler");
        let wrong_sender = (expected_sender + 1) % (node_pkhs.len() as u64);

        let (transport, _withdrawal_state_store, _runtime_task, _runtime, _dir) =
            open_transport(0, 1, node_pkhs, WithdrawalFallbackPolicy::default()).await;
        transport
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track request");

        let response = transport
            .ingest_peer_proposal(wrong_sender, &proposal)
            .await
            .expect("ingest wrong leader proposal");

        assert!(!response.accepted);
        assert_eq!(response.status, "wrong_assembler");
    }

    #[tokio::test]
    async fn accepts_proposal_from_current_sequencer_handoff_leader() {
        let request = sample_request();
        let proposal = sample_proposal(0);
        let node_pkhs = sample_node_pkhs();
        let original_sender = scheduled_assembler_node_id(&proposal.id, proposal.epoch, &node_pkhs)
            .expect("initial assembler");
        let (handoff_index, handoff_sender) =
            handoff_sender_distinct_from(&proposal, &node_pkhs, original_sender);
        let local_node_id = (handoff_sender + 1) % (node_pkhs.len() as u64);

        let (transport, withdrawal_state_store, _runtime_task, _runtime, _dir) = open_transport(
            local_node_id,
            1,
            node_pkhs,
            WithdrawalFallbackPolicy::default(),
        )
        .await;
        transport
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        register_request_ordering(&withdrawal_state_store, &request, 1).await;
        withdrawal_state_store
            .record_precanonical_handoff_for_id(&proposal.id, proposal.epoch, handoff_index, 100)
            .await
            .expect("record pre-canonical handoff");

        let response = transport
            .ingest_peer_proposal(handoff_sender, &proposal)
            .await
            .expect("ingest handoff leader proposal");

        assert!(response.accepted, "response: {response:?}");
        assert_ne!(response.status, "wrong_assembler");
    }

    #[tokio::test]
    async fn rejects_original_epoch_leader_after_sequencer_handoff() {
        let request = sample_request();
        let proposal = sample_proposal(0);
        let node_pkhs = sample_node_pkhs();
        let original_sender = scheduled_assembler_node_id(&proposal.id, proposal.epoch, &node_pkhs)
            .expect("initial assembler");
        let (handoff_index, handoff_sender) =
            handoff_sender_distinct_from(&proposal, &node_pkhs, original_sender);
        let local_node_id = (handoff_sender + 1) % (node_pkhs.len() as u64);

        let (transport, withdrawal_state_store, _runtime_task, _runtime, _dir) = open_transport(
            local_node_id,
            1,
            node_pkhs,
            WithdrawalFallbackPolicy::default(),
        )
        .await;
        transport
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        register_request_ordering(&withdrawal_state_store, &request, 1).await;
        withdrawal_state_store
            .record_precanonical_handoff_for_id(&proposal.id, proposal.epoch, handoff_index, 100)
            .await
            .expect("record pre-canonical handoff");

        let response = transport
            .ingest_peer_proposal(original_sender, &proposal)
            .await
            .expect("ingest original leader proposal after handoff");

        assert!(!response.accepted, "response: {response:?}");
        assert_eq!(response.status, "wrong_assembler");
    }

    #[tokio::test]
    async fn rejects_future_handoff_leader_before_sequencer_handoff() {
        let request = sample_request();
        let proposal = sample_proposal(0);
        let node_pkhs = sample_node_pkhs();
        let original_sender = scheduled_assembler_node_id(&proposal.id, proposal.epoch, &node_pkhs)
            .expect("initial assembler");
        let (_handoff_index, future_handoff_sender) =
            handoff_sender_distinct_from(&proposal, &node_pkhs, original_sender);
        let local_node_id = (future_handoff_sender + 1) % (node_pkhs.len() as u64);

        let (transport, withdrawal_state_store, _runtime_task, _runtime, _dir) = open_transport(
            local_node_id,
            1,
            node_pkhs,
            WithdrawalFallbackPolicy::default(),
        )
        .await;
        transport
            .registry()
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        register_request_ordering(&withdrawal_state_store, &request, 1).await;

        let response = transport
            .ingest_peer_proposal(future_handoff_sender, &proposal)
            .await
            .expect("ingest future handoff leader proposal");

        assert!(!response.accepted, "response: {response:?}");
        assert_eq!(response.status, "wrong_assembler");
    }

    fn handoff_sender_distinct_from(
        proposal: &WithdrawalProposalData,
        node_pkhs: &[nockchain_types::tx_engine::common::Hash],
        excluded_sender: u64,
    ) -> (u64, u64) {
        (1..=node_pkhs.len() as u64)
            .find_map(|handoff_index| {
                let handoff_sender = scheduled_assembler_turn_node_id(
                    &proposal.id, proposal.epoch, handoff_index, node_pkhs,
                )
                .ok()?;
                (handoff_sender != excluded_sender).then_some((handoff_index, handoff_sender))
            })
            .expect("handoff assembler")
    }
}
