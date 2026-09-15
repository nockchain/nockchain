use std::collections::{BTreeSet, HashMap};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, B256};
use tonic::{async_trait, Request, Response, Status};
use tracing::{info, warn};

use crate::shared::errors::BridgeError;
use crate::shared::ingress::proto;
use crate::shared::ingress::proto::withdrawal_public_query_server::{
    WithdrawalPublicQuery, WithdrawalPublicQueryServer,
};
use crate::withdrawal::quote::WithdrawalQuotePort;
use crate::withdrawal::sequencer::base_activity::{
    BaseActivityPageCursor, BaseActivityStore, PublicBaseWithdrawalBurn,
};
use crate::withdrawal::sequencer::base_height::SequencerBaseHeightTracker;
use crate::withdrawal::sequencer::base_incidents::{
    BaseIncidentStore, CompensatedBaseWithdrawal, RejectedBaseWithdrawalBurn,
};
use crate::withdrawal::sequencer::store::{
    PublicWithdrawalRecoveryProjection, WithdrawalSequencerStore, WithdrawalSubmissionEventRecord,
};
use crate::withdrawal::state::{LiveWithdrawalView, WithdrawalState};
use crate::withdrawal::submission::WithdrawalSubmitPort;
use crate::withdrawal::transport::{withdrawal_id_from_proto, withdrawal_id_to_proto};
#[cfg(test)]
use crate::withdrawal::types::WithdrawalId as DomainWithdrawalId;
const PUBLIC_WITHDRAWAL_SCHEMA_VERSION: u32 = 1;
const PUBLIC_WITHDRAWAL_DEFAULT_PAGE_SIZE: u32 = 20;
const PUBLIC_WITHDRAWAL_MAX_PAGE_SIZE: u32 = 100;
const PUBLIC_WITHDRAWAL_PAGE_TOKEN_PAYLOAD_LEN: usize = 177;
const PUBLIC_WITHDRAWAL_PAGE_TOKEN_LEN: usize = PUBLIC_WITHDRAWAL_PAGE_TOKEN_PAYLOAD_LEN + 32;

#[derive(Clone)]
pub struct PublicWithdrawalQueryConfig {
    pub base_chain_id: u64,
    pub nock_contract_address: Address,
    pub policy_id: String,
    pub protocol_id: String,
    pub page_token_key: [u8; 32],
    pub delayed_after: Duration,
    pub base_stale_after: Duration,
    pub admission_enabled: bool,
}

#[derive(Clone)]
pub struct PublicWithdrawalQueryService {
    store: Arc<WithdrawalSequencerStore>,
    activity: BaseActivityStore,
    incidents: BaseIncidentStore,
    base_height_tracker: Arc<SequencerBaseHeightTracker>,
    quote: Arc<dyn WithdrawalQuotePort>,
    nockchain: Arc<dyn WithdrawalSubmitPort>,
    config: PublicWithdrawalQueryConfig,
}

impl PublicWithdrawalQueryService {
    pub fn new(
        store: Arc<WithdrawalSequencerStore>,
        base_height_tracker: Arc<SequencerBaseHeightTracker>,
        nockchain: Arc<dyn WithdrawalSubmitPort>,
        quote: Arc<dyn WithdrawalQuotePort>,
        config: PublicWithdrawalQueryConfig,
    ) -> Self {
        let activity = store.base_activity_store();
        let incidents = activity.incident_store();
        Self {
            store,
            activity,
            incidents,
            base_height_tracker,
            quote,
            nockchain,
            config,
        }
    }

    fn deployment_proto(&self) -> proto::PublicWithdrawalDeployment {
        proto::PublicWithdrawalDeployment {
            base_chain_id: self.config.base_chain_id,
            nock_contract_address: self.config.nock_contract_address.as_slice().to_vec(),
            policy_id: self.config.policy_id.clone(),
            protocol_id: self.config.protocol_id.clone(),
        }
    }

    fn validate_deployment(
        &self,
        deployment: Option<&proto::PublicWithdrawalDeployment>,
    ) -> Result<(), Status> {
        let deployment =
            deployment.ok_or_else(|| Status::invalid_argument("deployment is required"))?;
        if deployment.nock_contract_address.len() != 20 {
            return Err(Status::invalid_argument(
                "deployment Nock contract address must be 20 bytes",
            ));
        }
        if deployment.base_chain_id != self.config.base_chain_id
            || deployment.nock_contract_address != self.config.nock_contract_address.as_slice()
            || deployment.policy_id != self.config.policy_id
            || deployment.protocol_id != self.config.protocol_id
        {
            return Err(Status::failed_precondition(
                "withdrawal deployment, policy, or protocol does not match this endpoint",
            ));
        }
        Ok(())
    }

    fn parse_tx_hash(bytes: &[u8]) -> Result<B256, Status> {
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| Status::invalid_argument("Base transaction hash must be 32 bytes"))?;
        Ok(B256::from(bytes))
    }

    fn parse_base_event_id(bytes: &[u8]) -> Result<crate::shared::types::BaseEventId, Status> {
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| Status::invalid_argument("base_event_id must be 32 bytes"))?;
        Ok(crate::shared::types::BaseEventId(bytes.to_vec()))
    }

    fn parse_burner(bytes: &[u8]) -> Result<Address, Status> {
        let bytes: [u8; 20] = bytes
            .try_into()
            .map_err(|_| Status::invalid_argument("burner must be 20 bytes"))?;
        Ok(Address::from(bytes))
    }

    async fn burn_from_locator(
        &self,
        locator: &proto::PublicBaseWithdrawalLocator,
        require_unique: bool,
    ) -> Result<Option<PublicBaseWithdrawalBurn>, Status> {
        let tx_hash = Self::parse_tx_hash(&locator.transaction_hash)?;
        if let Some(log_index) = locator.log_index {
            return self
                .activity
                .load_public_burn_by_tx_log(
                    self.config.base_chain_id, self.config.nock_contract_address, tx_hash,
                    log_index,
                )
                .await
                .map_err(|err| self.internal_status("Base tx/log lookup", err));
        }
        let burns = self
            .activity
            .list_public_burns_by_tx_hash(
                self.config.base_chain_id, self.config.nock_contract_address, tx_hash,
            )
            .await
            .map_err(|err| self.internal_status("Base transaction lookup", err))?;
        match burns.len() {
            0 => Ok(None),
            1 => Ok(burns.into_iter().next()),
            _ if require_unique => Err(Status::failed_precondition(
                "Base transaction contains multiple withdrawal logs; log_index is required",
            )),
            _ => Ok(None),
        }
    }
    async fn rejected_from_locator(
        &self,
        locator: &proto::PublicBaseWithdrawalLocator,
        require_unique: bool,
    ) -> Result<Option<RejectedBaseWithdrawalBurn>, Status> {
        let tx_hash = Self::parse_tx_hash(&locator.transaction_hash)?;
        if let Some(log_index) = locator.log_index {
            return self
                .incidents
                .load_rejected_burn_by_tx_log(
                    self.config.base_chain_id, self.config.nock_contract_address, tx_hash,
                    log_index,
                )
                .await
                .map_err(|err| self.internal_status("rejected Base tx/log lookup", err));
        }
        let burns = self
            .incidents
            .list_rejected_burns_by_tx_hash(
                self.config.base_chain_id, self.config.nock_contract_address, tx_hash,
            )
            .await
            .map_err(|err| self.internal_status("rejected Base transaction lookup", err))?;
        match burns.len() {
            0 => Ok(None),
            1 => Ok(burns.into_iter().next()),
            _ if require_unique => Err(Status::failed_precondition(
                "Base transaction contains multiple rejected withdrawal logs; log_index is required",
            )),
            _ => Ok(None),
        }
    }

    async fn compensation_from_locator(
        &self,
        locator: &proto::PublicBaseWithdrawalLocator,
    ) -> Result<Option<CompensatedBaseWithdrawal>, Status> {
        let Some(log_index) = locator.log_index else {
            return Ok(None);
        };
        let tx_hash = Self::parse_tx_hash(&locator.transaction_hash)?;
        self.incidents
            .load_compensated_withdrawal_by_tx_log(
                self.config.base_chain_id, self.config.nock_contract_address, tx_hash, log_index,
            )
            .await
            .map_err(|err| self.internal_status("compensated Base tx/log lookup", err))
    }

    fn internal_status(&self, operation: &str, error: BridgeError) -> Status {
        warn!(
            target: "bridge.withdrawal.public",
            operation,
            error = %error,
            "public withdrawal query failed"
        );
        Status::unavailable("withdrawal status is temporarily unavailable")
    }

    async fn single_record(
        &self,
        burn: PublicBaseWithdrawalBurn,
    ) -> Result<proto::PublicWithdrawalRecord, Status> {
        let base_event_id = burn.burn.base_event_id.clone();
        let mut projections = self
            .store
            .public_lifecycle_projections(vec![base_event_id.clone()])
            .await
            .map_err(|err| self.internal_status("public lifecycle lookup", err))?;
        let projection = projections.remove(&base_event_id);
        let lifecycle = projection.as_ref().map(|projection| &projection.lifecycle);
        let net_amount = projection
            .as_ref()
            .and_then(|projection| projection.net_amount);
        let confirmation = projection
            .as_ref()
            .and_then(|projection| projection.confirmation.as_ref());
        let recovery = projection
            .as_ref()
            .and_then(|projection| projection.recovery.as_ref());
        let revision = self
            .store
            .public_projection_revision()
            .await
            .map_err(|err| self.internal_status("revision lookup", err))?;
        let compensated = self
            .incidents
            .load_compensated_withdrawal(
                self.config.base_chain_id, self.config.nock_contract_address, &base_event_id,
            )
            .await
            .map_err(|err| self.internal_status("compensation lookup", err))?;
        self.build_record(
            burn,
            compensated.as_ref(),
            lifecycle,
            net_amount,
            confirmation,
            recovery,
            revision,
            unix_now_secs().map_err(|err| self.internal_status("system time", err))?,
        )
        .map_err(|err| self.internal_status("record projection", err))
    }

    fn build_record(
        &self,
        public_burn: PublicBaseWithdrawalBurn,
        compensated: Option<&CompensatedBaseWithdrawal>,
        lifecycle: Option<&LiveWithdrawalView>,
        net_amount: Option<u64>,
        confirmation: Option<&WithdrawalSubmissionEventRecord>,
        recovery: Option<&PublicWithdrawalRecoveryProjection>,
        projection_revision: u64,
        now_secs: u64,
    ) -> Result<proto::PublicWithdrawalRecord, BridgeError> {
        let canonical_base_event = public_burn.canonical;
        let fallback_generation = public_burn.invalidation_generation;
        let fallback_reason = public_burn.invalidation_reason.clone();
        let fallback_invalidated_at = public_burn.invalidated_at;
        let burn = public_burn.burn;
        let policy_matches = burn.policy_id.as_deref() == Some(self.config.policy_id.as_str())
            && burn.protocol_id.as_deref() == Some(self.config.protocol_id.as_str());
        let minimum_nicks = crate::shared::types::WithdrawalPolicy::v1()
            .minimum_gross_nocks
            .checked_mul(crate::shared::types::WithdrawalPolicy::v1().nicks_per_nock)
            .ok_or_else(|| {
                BridgeError::ValueConversion("public withdrawal minimum overflow".into())
            })?;
        let below_policy = burn.amount_nicks < minimum_nicks;
        let observed_secs = burn.observed_at_unix_secs;
        let delayed = observed_secs
            .and_then(|observed| now_secs.checked_sub(observed))
            .is_some_and(|elapsed| elapsed >= self.config.delayed_after.as_secs());
        let recovery = recovery.filter(|recovery| {
            !matches!(
                (lifecycle, confirmation),
                (Some(row), Some(event))
                    if row.state == WithdrawalState::Confirmed
                        && event.created_at > recovery.observed_at
            )
        });
        let active_confirmation = confirmation.filter(|event| {
            lifecycle.is_some_and(|row| row.state == WithdrawalState::Confirmed)
                && recovery.is_none_or(|recovery| event.created_at > recovery.observed_at)
        });
        let reorged = !canonical_base_event
            || recovery.is_some()
            || lifecycle.is_some_and(|row| {
                matches!(
                    row.state,
                    WithdrawalState::Invalidated | WithdrawalState::ReorgHold
                )
            });
        let (status, resolution, support_hint) = if compensated.is_some() {
            (
                proto::PublicWithdrawalStatus::Failure,
                proto::PublicWithdrawalResolution::Compensated,
                "This burn was resolved through governance compensation and cannot produce another payout.",
            )
        } else if !policy_matches {
            (
                proto::PublicWithdrawalStatus::Failure,
                proto::PublicWithdrawalResolution::Inconsistent,
                "Withdrawal metadata is inconsistent with this deployment. Contact support.",
            )
        } else if below_policy {
            (
                proto::PublicWithdrawalStatus::Failure,
                proto::PublicWithdrawalResolution::BelowPolicy,
                "This burn is below the active withdrawal policy. Contact support; do not burn again.",
            )
        } else if reorged {
            (
                if !canonical_base_event
                    || lifecycle.is_some_and(|row| {
                        matches!(
                            row.state,
                            WithdrawalState::Invalidated | WithdrawalState::ReorgHold
                        )
                    })
                {
                    proto::PublicWithdrawalStatus::Failure
                } else {
                    proto::PublicWithdrawalStatus::WithdrawalPending
                },
                proto::PublicWithdrawalResolution::Reorged,
                "Authoritative chain recovery invalidated prior settlement facts. Do not submit another burn.",
            )
        } else if active_confirmation.is_some() {
            (
                proto::PublicWithdrawalStatus::Confirmed,
                proto::PublicWithdrawalResolution::Found,
                "",
            )
        } else if delayed {
            (
                proto::PublicWithdrawalStatus::Delayed,
                proto::PublicWithdrawalResolution::Found,
                "This withdrawal is delayed. Contact support; do not submit another burn.",
            )
        } else {
            (
                proto::PublicWithdrawalStatus::WithdrawalPending,
                proto::PublicWithdrawalResolution::Found,
                "Withdrawal processing is in progress. Do not submit another burn.",
            )
        };
        let withdrawal_id = lifecycle
            .filter(|row| row.id.as_of != crate::shared::types::zero_tip5_hash())
            .map(|row| withdrawal_id_to_proto(&row.id));
        let observed_at_unix_ms = observed_secs.map(seconds_u64_to_millis_i64).transpose()?;
        let lifecycle_updated_at = lifecycle.map(|row| row.updated_at);
        let recovery_updated_at = recovery
            .map(|recovery| recovery.observed_at)
            .or(fallback_invalidated_at);
        let updated_at_unix_ms = [lifecycle_updated_at, recovery_updated_at]
            .into_iter()
            .flatten()
            .max()
            .map(seconds_i64_to_millis)
            .transpose()?
            .or(observed_at_unix_ms);
        let confirmed_at_unix_ms = active_confirmation
            .map(|event| seconds_i64_to_millis(event.created_at))
            .transpose()?;
        let revision = compose_public_revision(burn.verified_at, projection_revision)?;
        Ok(proto::PublicWithdrawalRecord {
            schema_version: PUBLIC_WITHDRAWAL_SCHEMA_VERSION,
            deployment: Some(self.deployment_proto()),
            base: Some(proto::PublicBaseWithdrawalLocator {
                transaction_hash: burn.tx_hash.as_slice().to_vec(),
                log_index: Some(burn.log_index),
            }),
            base_event_id: burn.base_event_id.0,
            withdrawal_id,
            burner: burn.burner.as_slice().to_vec(),
            gross_amount_base_units: burn.amount_base_units,
            gross_amount_nicks: burn.amount_nicks.to_string(),
            net_amount_nicks: net_amount.map(|amount| amount.to_string()),
            destination_lock_root: burn.lock_root.to_be_limb_bytes().to_vec(),
            status: status as i32,
            resolution: resolution as i32,
            revision,
            canonical_base_event,
            base_block_number: Some(burn.block_number),
            base_block_hash: Some(burn.block_hash.as_slice().to_vec()),
            observed_at_unix_ms,
            updated_at_unix_ms,
            nock_transaction_name: lifecycle
                .and_then(|row| row.authorized_transaction_name.clone()),
            nock_confirmed_height: active_confirmation.and_then(|event| event.confirmed_height),
            nock_confirmed_block_id: active_confirmation
                .and_then(|event| event.confirmed_block_id.as_ref())
                .map(|block_id| block_id.to_be_limb_bytes().to_vec()),
            confirmed_at_unix_ms,
            support_hint: support_hint.to_string(),
            recovery_generation: recovery
                .map(|recovery| recovery.generation)
                .or(fallback_generation),
            invalidated_block_height: recovery
                .map(|recovery| recovery.invalidated_block_height)
                .or_else(|| (!canonical_base_event).then_some(burn.block_number)),
            invalidated_block_id: recovery
                .map(|recovery| recovery.invalidated_block_id.clone())
                .or_else(|| (!canonical_base_event).then(|| burn.block_hash.as_slice().to_vec())),
            prior_status: recovery
                .map(|recovery| recovery.prior_status.clone())
                .unwrap_or_else(|| {
                    if canonical_base_event {
                        String::new()
                    } else {
                        "pending".to_string()
                    }
                }),
            recovery_reason: recovery
                .map(|recovery| recovery.reason.clone())
                .or(fallback_reason)
                .unwrap_or_default(),
        })
    }

    fn decode_page_token(
        &self,
        token: &str,
        burner: Address,
        current_revision: u64,
    ) -> Result<BaseActivityPageCursor, Status> {
        let bytes =
            hex::decode(token).map_err(|_| Status::invalid_argument("page_token is malformed"))?;
        if bytes.len() != PUBLIC_WITHDRAWAL_PAGE_TOKEN_LEN {
            return Err(Status::invalid_argument("page_token has invalid length"));
        }
        let (payload, supplied_tag) = bytes.split_at(PUBLIC_WITHDRAWAL_PAGE_TOKEN_PAYLOAD_LEN);
        let expected_tag = blake3::keyed_hash(&self.config.page_token_key, payload);
        let mismatch = supplied_tag
            .iter()
            .zip(expected_tag.as_bytes())
            .fold(0u8, |acc, (left, right)| acc | (left ^ right));
        if mismatch != 0 {
            return Err(Status::invalid_argument("page_token authentication failed"));
        }
        if payload[0] != 2 {
            return Err(Status::invalid_argument(
                "page_token version is unsupported",
            ));
        }
        let chain_id = u64::from_be_bytes(
            payload[1..9]
                .try_into()
                .map_err(|_| Status::invalid_argument("page_token chain id is malformed"))?,
        );
        let token_contract = &payload[9..29];
        let token_burner = &payload[29..49];
        let policy_hash = &payload[49..81];
        let protocol_hash = &payload[81..113];
        if chain_id != self.config.base_chain_id
            || token_contract != self.config.nock_contract_address.as_slice()
            || token_burner != burner.as_slice()
            || policy_hash != blake3::hash(self.config.policy_id.as_bytes()).as_bytes()
            || protocol_hash != blake3::hash(self.config.protocol_id.as_bytes()).as_bytes()
        {
            return Err(Status::invalid_argument(
                "page_token belongs to another deployment, policy, protocol, or burner",
            ));
        }
        let snapshot_revision = u64::from_be_bytes(
            payload[113..121]
                .try_into()
                .map_err(|_| Status::invalid_argument("page_token revision is malformed"))?,
        );
        if snapshot_revision != current_revision {
            return Err(Status::failed_precondition(
                "page_token snapshot was superseded by a lifecycle or reorg revision; restart history pagination",
            ));
        }
        Ok(BaseActivityPageCursor {
            snapshot_revision,
            snapshot_rowid: u64::from_be_bytes(
                payload[121..129]
                    .try_into()
                    .map_err(|_| Status::invalid_argument("page_token snapshot is malformed"))?,
            ),
            last_block_number: u64::from_be_bytes(
                payload[129..137]
                    .try_into()
                    .map_err(|_| Status::invalid_argument("page_token block is malformed"))?,
            ),
            last_log_index: u64::from_be_bytes(
                payload[137..145]
                    .try_into()
                    .map_err(|_| Status::invalid_argument("page_token log is malformed"))?,
            ),
            last_base_event_id: crate::shared::types::BaseEventId(payload[145..177].to_vec()),
        })
    }

    fn encode_page_token(
        &self,
        burner: Address,
        snapshot_revision: u64,
        snapshot_rowid: u64,
        last: &PublicBaseWithdrawalBurn,
    ) -> String {
        let mut payload = Vec::with_capacity(PUBLIC_WITHDRAWAL_PAGE_TOKEN_PAYLOAD_LEN);
        payload.push(2);
        payload.extend_from_slice(&self.config.base_chain_id.to_be_bytes());
        payload.extend_from_slice(self.config.nock_contract_address.as_slice());
        payload.extend_from_slice(burner.as_slice());
        payload.extend_from_slice(blake3::hash(self.config.policy_id.as_bytes()).as_bytes());
        payload.extend_from_slice(blake3::hash(self.config.protocol_id.as_bytes()).as_bytes());
        payload.extend_from_slice(&snapshot_revision.to_be_bytes());
        payload.extend_from_slice(&snapshot_rowid.to_be_bytes());
        payload.extend_from_slice(&last.burn.block_number.to_be_bytes());
        payload.extend_from_slice(&last.burn.log_index.to_be_bytes());
        payload.extend_from_slice(&last.burn.base_event_id.0);
        let tag = blake3::keyed_hash(&self.config.page_token_key, &payload);
        payload.extend_from_slice(tag.as_bytes());
        hex::encode(payload)
    }
}

#[async_trait]
impl WithdrawalPublicQuery for PublicWithdrawalQueryService {
    async fn resolve_base_withdrawal(
        &self,
        request: Request<proto::ResolveBaseWithdrawalRequest>,
    ) -> Result<Response<proto::ResolveBaseWithdrawalResponse>, Status> {
        let request = request.into_inner();
        self.validate_deployment(request.deployment.as_ref())?;
        let locator = request
            .base
            .as_ref()
            .ok_or_else(|| Status::invalid_argument("Base locator is required"))?;
        let tx_hash = Self::parse_tx_hash(&locator.transaction_hash)?;
        if locator.log_index.is_none() {
            let burns = self
                .activity
                .list_public_burns_by_tx_hash(
                    self.config.base_chain_id, self.config.nock_contract_address, tx_hash,
                )
                .await
                .map_err(|err| self.internal_status("Base transaction resolution", err))?;
            let rejected = self
                .incidents
                .list_rejected_burns_by_tx_hash(
                    self.config.base_chain_id, self.config.nock_contract_address, tx_hash,
                )
                .await
                .map_err(|err| self.internal_status("rejected Base transaction resolution", err))?;
            let compensated = self
                .incidents
                .list_compensated_withdrawals_by_tx_hash(
                    self.config.base_chain_id, self.config.nock_contract_address, tx_hash,
                )
                .await
                .map_err(|err| {
                    self.internal_status("compensated Base transaction resolution", err)
                })?;
            let candidate_log_indices = burns
                .iter()
                .map(|burn| burn.burn.log_index)
                .chain(rejected.iter().map(|burn| burn.log_index))
                .chain(compensated.iter().map(|record| record.log_index))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            if candidate_log_indices.len() > 1 {
                return Ok(Response::new(proto::ResolveBaseWithdrawalResponse {
                    resolution: proto::PublicWithdrawalResolution::AmbiguousLog as i32,
                    status: proto::PublicWithdrawalStatus::AwaitingBase as i32,
                    withdrawal: None,
                    candidate_log_indices,
                    support_hint:
                        "Select the withdrawal log index from the Base transaction receipt."
                            .to_string(),
                }));
            }
            if let Some(burn) = burns.into_iter().next() {
                let record = self.single_record(burn).await?;
                return Ok(Response::new(proto::ResolveBaseWithdrawalResponse {
                    resolution: record.resolution,
                    status: record.status,
                    withdrawal: Some(record),
                    candidate_log_indices: Vec::new(),
                    support_hint: String::new(),
                }));
            }
            if compensated.into_iter().next().is_some() {
                return Ok(Response::new(proto::ResolveBaseWithdrawalResponse {
                    resolution: proto::PublicWithdrawalResolution::Compensated as i32,
                    status: proto::PublicWithdrawalStatus::Failure as i32,
                    withdrawal: None,
                    candidate_log_indices: Vec::new(),
                    support_hint:
                        "This burn was resolved through governance compensation; no second payout is permitted."
                            .to_string(),
                }));
            }
            if let Some(rejected) = rejected.into_iter().next() {
                return Ok(Response::new(proto::ResolveBaseWithdrawalResponse {
                    resolution: proto::PublicWithdrawalResolution::MalformedBurn as i32,
                    status: proto::PublicWithdrawalStatus::Failure as i32,
                    withdrawal: None,
                    candidate_log_indices: Vec::new(),
                    support_hint: format!(
                        "Unsupported withdrawal burn ({}). Contact support; do not burn again.",
                        rejected.rejection_code
                    ),
                }));
            }
        } else {
            if let Some(burn) = self.burn_from_locator(locator, true).await? {
                let record = self.single_record(burn).await?;
                return Ok(Response::new(proto::ResolveBaseWithdrawalResponse {
                    resolution: record.resolution,
                    status: record.status,
                    withdrawal: Some(record),
                    candidate_log_indices: Vec::new(),
                    support_hint: String::new(),
                }));
            }
            if self.compensation_from_locator(locator).await?.is_some() {
                return Ok(Response::new(proto::ResolveBaseWithdrawalResponse {
                    resolution: proto::PublicWithdrawalResolution::Compensated as i32,
                    status: proto::PublicWithdrawalStatus::Failure as i32,
                    withdrawal: None,
                    candidate_log_indices: Vec::new(),
                    support_hint:
                        "This burn was resolved through governance compensation; no second payout is permitted."
                            .to_string(),
                }));
            }
            if let Some(rejected) = self.rejected_from_locator(locator, true).await? {
                return Ok(Response::new(proto::ResolveBaseWithdrawalResponse {
                    resolution: proto::PublicWithdrawalResolution::MalformedBurn as i32,
                    status: proto::PublicWithdrawalStatus::Failure as i32,
                    withdrawal: None,
                    candidate_log_indices: Vec::new(),
                    support_hint: format!(
                        "Unsupported withdrawal burn ({}). Contact support; do not burn again.",
                        rejected.rejection_code
                    ),
                }));
            }
        }
        Ok(Response::new(proto::ResolveBaseWithdrawalResponse {
            resolution: proto::PublicWithdrawalResolution::NotObserved as i32,
            status: proto::PublicWithdrawalStatus::AwaitingBase as i32,
            withdrawal: None,
            candidate_log_indices: Vec::new(),
            support_hint: "Wait for Base confirmation and indexing; do not submit another burn."
                .to_string(),
        }))
    }

    async fn get_withdrawal(
        &self,
        request: Request<proto::GetPublicWithdrawalRequest>,
    ) -> Result<Response<proto::GetPublicWithdrawalResponse>, Status> {
        let lookup = request
            .into_inner()
            .lookup
            .ok_or_else(|| Status::invalid_argument("lookup is required"))?;
        self.validate_deployment(lookup.deployment.as_ref())?;
        let mut exact_withdrawal_id = None;
        let mut base_event_id_hint = None;
        let mut locator_hint = None;
        let burn = match lookup
            .key
            .ok_or_else(|| Status::invalid_argument("lookup key is required"))?
        {
            proto::public_withdrawal_lookup_key::Key::Base(locator) => {
                locator_hint = Some(locator.clone());
                self.burn_from_locator(&locator, true).await?
            }
            proto::public_withdrawal_lookup_key::Key::BaseEventId(base_event_id) => {
                let base_event_id = Self::parse_base_event_id(&base_event_id)?;
                base_event_id_hint = Some(base_event_id.clone());
                self.activity
                    .load_public_burn(
                        self.config.base_chain_id, self.config.nock_contract_address,
                        &base_event_id,
                    )
                    .await
                    .map_err(|err| self.internal_status("Base event lookup", err))?
            }
            proto::public_withdrawal_lookup_key::Key::WithdrawalId(withdrawal_id) => {
                let withdrawal_id = withdrawal_id_from_proto(&withdrawal_id)
                    .map_err(|_| Status::invalid_argument("withdrawal_id is malformed"))?;
                base_event_id_hint = Some(withdrawal_id.base_event_id.clone());
                let burn = self
                    .activity
                    .load_public_burn(
                        self.config.base_chain_id, self.config.nock_contract_address,
                        &withdrawal_id.base_event_id,
                    )
                    .await
                    .map_err(|err| self.internal_status("withdrawal id lookup", err))?;
                exact_withdrawal_id = Some(withdrawal_id);
                burn
            }
        };
        let revision = self
            .store
            .public_projection_revision()
            .await
            .map_err(|err| self.internal_status("public lookup revision", err))?;
        let Some(burn) = burn else {
            let compensated = if let Some(base_event_id) = base_event_id_hint.as_ref() {
                self.incidents
                    .load_compensated_withdrawal(
                        self.config.base_chain_id, self.config.nock_contract_address, base_event_id,
                    )
                    .await
                    .map_err(|err| self.internal_status("compensated Base event lookup", err))?
                    .is_some()
            } else if let Some(locator) = locator_hint.as_ref() {
                self.compensation_from_locator(locator).await?.is_some()
            } else {
                false
            };
            if compensated {
                return Ok(Response::new(proto::GetPublicWithdrawalResponse {
                    found: false,
                    withdrawal: None,
                    resolution: proto::PublicWithdrawalResolution::Compensated as i32,
                    support_hint:
                        "This burn was resolved through governance compensation; no second payout is permitted."
                            .to_string(),
                    revision,
                }));
            }
            let rejected = if let Some(base_event_id) = base_event_id_hint.as_ref() {
                self.incidents
                    .load_rejected_burn(
                        self.config.base_chain_id, self.config.nock_contract_address, base_event_id,
                    )
                    .await
                    .map_err(|err| self.internal_status("rejected Base event lookup", err))?
            } else if let Some(locator) = locator_hint.as_ref() {
                self.rejected_from_locator(locator, true).await?
            } else {
                None
            };
            if let Some(rejected) = rejected {
                return Ok(Response::new(proto::GetPublicWithdrawalResponse {
                    found: false,
                    withdrawal: None,
                    resolution: proto::PublicWithdrawalResolution::MalformedBurn as i32,
                    support_hint: format!(
                        "Unsupported withdrawal burn ({}). Contact support; do not burn again.",
                        rejected.rejection_code
                    ),
                    revision,
                }));
            }
            return Ok(Response::new(proto::GetPublicWithdrawalResponse {
                found: false,
                withdrawal: None,
                resolution: proto::PublicWithdrawalResolution::NotObserved as i32,
                support_hint:
                    "Wait for Base confirmation and indexing; do not submit another burn."
                        .to_string(),
                revision,
            }));
        };
        if let Some(expected) = exact_withdrawal_id {
            let lifecycle = self
                .store
                .fetch_sequenced_withdrawal(&expected)
                .await
                .map_err(|err| self.internal_status("withdrawal identity check", err))?;
            if lifecycle.as_ref().is_none_or(|row| row.id != expected) {
                return Ok(Response::new(proto::GetPublicWithdrawalResponse {
                    found: false,
                    withdrawal: None,
                    resolution: proto::PublicWithdrawalResolution::NotObserved as i32,
                    support_hint: "Withdrawal identity is not authoritative yet.".to_string(),
                    revision,
                }));
            }
        }
        let record = self.single_record(burn).await?;
        Ok(Response::new(proto::GetPublicWithdrawalResponse {
            found: true,
            resolution: record.resolution,
            support_hint: record.support_hint.clone(),
            revision: record.revision,
            withdrawal: Some(record),
        }))
    }

    async fn list_withdrawals_by_burner(
        &self,
        request: Request<proto::ListPublicWithdrawalsByBurnerRequest>,
    ) -> Result<Response<proto::ListPublicWithdrawalsByBurnerResponse>, Status> {
        let request = request.into_inner();
        self.validate_deployment(request.deployment.as_ref())?;
        let burner = Self::parse_burner(&request.burner)?;
        let revision = self
            .store
            .public_projection_revision()
            .await
            .map_err(|err| self.internal_status("history revision lookup", err))?;
        let page_size = match request.page_size {
            0 => PUBLIC_WITHDRAWAL_DEFAULT_PAGE_SIZE,
            size if size <= PUBLIC_WITHDRAWAL_MAX_PAGE_SIZE => size,
            _ => {
                return Err(Status::invalid_argument(
                    "page_size exceeds the maximum of 100",
                ))
            }
        };
        let cursor = if request.page_token.is_empty() {
            None
        } else {
            Some(self.decode_page_token(&request.page_token, burner, revision)?)
        };
        let fetch_limit = page_size
            .checked_add(1)
            .ok_or_else(|| Status::invalid_argument("page_size overflow"))?;
        let mut page = self
            .activity
            .list_public_burns_by_burner_page(
                self.config.base_chain_id, self.config.nock_contract_address, burner, cursor,
                fetch_limit,
            )
            .await
            .map_err(|err| self.internal_status("burner history", err))?;
        let has_more = page.burns.len() > page_size as usize;
        if has_more {
            page.burns.truncate(page_size as usize);
        }
        let next_page_token = if has_more {
            page.burns
                .last()
                .map(|last| self.encode_page_token(burner, revision, page.snapshot_rowid, last))
                .unwrap_or_default()
        } else {
            String::new()
        };
        let base_event_ids = page
            .burns
            .iter()
            .map(|burn| burn.burn.base_event_id.clone())
            .collect::<Vec<_>>();
        let projections = self
            .store
            .public_lifecycle_projections(base_event_ids.clone())
            .await
            .map_err(|err| self.internal_status("history lifecycle lookup", err))?;
        let compensated = self
            .incidents
            .list_compensated_withdrawals_for_events(
                self.config.base_chain_id, self.config.nock_contract_address, base_event_ids,
            )
            .await
            .map_err(|err| self.internal_status("history compensation lookup", err))?
            .into_iter()
            .map(|record| (record.base_event_id.clone(), record))
            .collect::<HashMap<_, _>>();
        let now_secs = unix_now_secs().map_err(|err| self.internal_status("system time", err))?;
        let mut withdrawals = Vec::with_capacity(page.burns.len());
        for burn in page.burns {
            let projection = projections.get(&burn.burn.base_event_id);
            let lifecycle = projection.map(|projection| &projection.lifecycle);
            let compensation = compensated.get(&burn.burn.base_event_id);
            let net_amount = projection.and_then(|projection| projection.net_amount);
            let confirmation = projection.and_then(|projection| projection.confirmation.as_ref());
            let recovery = projection.and_then(|projection| projection.recovery.as_ref());
            withdrawals.push(
                self.build_record(
                    burn, compensation, lifecycle, net_amount, confirmation, recovery, revision,
                    now_secs,
                )
                .map_err(|err| self.internal_status("history record projection", err))?,
            );
        }
        Ok(Response::new(
            proto::ListPublicWithdrawalsByBurnerResponse {
                withdrawals,
                next_page_token,
                snapshot_revision: revision,
            },
        ))
    }

    async fn get_withdrawal_readiness(
        &self,
        request: Request<proto::GetPublicWithdrawalReadinessRequest>,
    ) -> Result<Response<proto::GetPublicWithdrawalReadinessResponse>, Status> {
        let request = request.into_inner();
        self.validate_deployment(request.deployment.as_ref())?;
        let now_unix_ms = seconds_u64_to_millis_i64(
            unix_now_secs().map_err(|err| self.internal_status("system time", err))?,
        )
        .map_err(|err| self.internal_status("readiness timestamp", err))?;
        let now_unix_ms_u64 = u64::try_from(now_unix_ms)
            .map_err(|_| Status::internal("readiness clock is negative"))?;
        let base_observation = self.base_height_tracker.latest_confirmed_base_observation();
        let confirmed_base_height = base_observation.map(|(height, _)| height);
        let base_observed_at_unix_ms = base_observation
            .map(|(_, observed_at)| {
                i64::try_from(observed_at).map_err(|err| {
                    Status::internal(format!("Base observation timestamp overflow: {err}"))
                })
            })
            .transpose()?;
        let stale_after_ms = u64::try_from(self.config.base_stale_after.as_millis())
            .map_err(|_| Status::internal("Base stale interval overflow"))?;
        let base_fresh = base_observation.is_some_and(|(_, observed_at)| {
            now_unix_ms_u64.saturating_sub(observed_at) <= stale_after_ms
        });
        let activity_cursor = self
            .activity
            .load_cursor(self.config.base_chain_id, self.config.nock_contract_address)
            .await
            .map_err(|err| self.internal_status("Base cursor readiness", err))?;
        let reconciliation = self
            .store
            .load_public_base_reconciliation_frontier(
                self.config.base_chain_id, self.config.nock_contract_address,
            )
            .await
            .map_err(|err| self.internal_status("reconciliation readiness", err))?;
        let projection_revision = self
            .store
            .public_projection_revision()
            .await
            .map_err(|err| self.internal_status("projection readiness", err))?;
        let reorg_hold = self.store.ensure_reorg_ready().await.is_err();
        let observed_nockchain_height = self
            .nockchain
            .current_nockchain_tip_height()
            .await
            .unwrap_or_default();
        let mut reasons = Vec::new();
        let indexed_base_height = activity_cursor
            .as_ref()
            .map(|cursor| cursor.last_verified_block);
        if !base_fresh
            || confirmed_base_height.is_none()
            || indexed_base_height.is_none()
            || indexed_base_height < confirmed_base_height
        {
            reasons.push(proto::PublicWithdrawalReadinessReason::BaseScannerBehind as i32);
        }
        if reconciliation.as_ref().is_none_or(|frontier| {
            Some(frontier.base_block) != indexed_base_height
                || frontier.journal_sequence != projection_revision
        }) {
            reasons.push(proto::PublicWithdrawalReadinessReason::JournalReconciling as i32);
        }
        if reorg_hold
            && !reasons
                .contains(&(proto::PublicWithdrawalReadinessReason::JournalReconciling as i32))
        {
            reasons.push(proto::PublicWithdrawalReadinessReason::JournalReconciling as i32);
        }
        if observed_nockchain_height.is_none() {
            reasons.push(proto::PublicWithdrawalReadinessReason::NockchainUnavailable as i32);
        }
        if !self.config.admission_enabled
            || self.base_height_tracker.withdrawals_enabled() != Some(true)
        {
            reasons.push(proto::PublicWithdrawalReadinessReason::WithdrawalsPaused as i32);
        }
        if reasons.is_empty() {
            reasons.push(proto::PublicWithdrawalReadinessReason::Healthy as i32);
        }
        let ready = reasons.as_slice() == [proto::PublicWithdrawalReadinessReason::Healthy as i32];
        let readiness = if ready {
            proto::PublicWithdrawalReadiness::Ready
        } else if activity_cursor.is_some() && reconciliation.is_some() {
            proto::PublicWithdrawalReadiness::Degraded
        } else {
            proto::PublicWithdrawalReadiness::Unavailable
        };
        let policy = crate::shared::types::WithdrawalPolicy::v1();
        let minimum_gross_nicks = policy
            .minimum_gross_nocks
            .checked_mul(policy.nicks_per_nock)
            .ok_or_else(|| Status::internal("withdrawal policy minimum overflow"))?;
        let minimum_gross_base_units = u128::from(minimum_gross_nicks)
            .checked_mul(policy.base_units_per_nick)
            .ok_or_else(|| Status::internal("withdrawal policy Base minimum overflow"))?;
        Ok(Response::new(proto::GetPublicWithdrawalReadinessResponse {
            readiness: readiness as i32,
            accepting_new_withdrawals: ready,
            reasons,
            policy_id: self.config.policy_id.clone(),
            protocol_id: self.config.protocol_id.clone(),
            confirmed_base_height,
            indexed_base_height,
            reconciled_journal_sequence: reconciliation.map(|frontier| frontier.journal_sequence),
            observed_nockchain_height,
            updated_at_unix_ms: base_observed_at_unix_ms.unwrap_or_default(),
            support_hint: if ready {
                String::new()
            } else {
                "Withdrawals are temporarily unavailable or delayed. Do not submit a burn."
                    .to_string()
            },
            base_observed_at_unix_ms,
            minimum_gross_nocks: policy.minimum_gross_nocks.to_string(),
            minimum_gross_nicks: minimum_gross_nicks.to_string(),
            minimum_gross_base_units: minimum_gross_base_units.to_string(),
            base_units_per_nock: policy.base_units_per_nock.to_string(),
            nicks_per_nock: policy.nicks_per_nock.to_string(),
            base_units_per_nick: policy.base_units_per_nick.to_string(),
            maximum_nicks: policy.maximum_nicks.to_string(),
            bridge_fee_nicks_per_started_nock: policy.bridge_fee_nicks_per_started_nock.to_string(),
        }))
    }
    async fn get_withdrawal_quote(
        &self,
        request: Request<proto::GetPublicWithdrawalQuoteRequest>,
    ) -> Result<Response<proto::GetPublicWithdrawalQuoteResponse>, Status> {
        let request = request.into_inner();
        self.validate_deployment(request.deployment.as_ref())?;
        let gross_amount_nicks = request
            .gross_amount_nicks
            .parse::<u64>()
            .map_err(|_| Status::invalid_argument("gross_amount_nicks must be a u64 decimal"))?;
        let destination_lock_root =
            crate::shared::types::Tip5Hash::from_be_limb_bytes(&request.destination_lock_root)
                .map_err(|error| {
                    Status::invalid_argument(format!("destination_lock_root is invalid: {error}"))
                })?;
        let policy = crate::shared::types::WithdrawalPolicy::v1();
        let minimum_gross_nicks = policy
            .minimum_gross_nocks
            .checked_mul(policy.nicks_per_nock)
            .ok_or_else(|| Status::internal("withdrawal policy minimum overflow"))?;
        if gross_amount_nicks < minimum_gross_nicks {
            return Err(Status::invalid_argument(format!(
                "gross_amount_nicks is below the inclusive minimum {minimum_gross_nicks}"
            )));
        }
        let revision = self
            .store
            .public_projection_revision()
            .await
            .map_err(|error| self.internal_status("quote revision", error))?;
        let observed_at_unix_ms = seconds_u64_to_millis_i64(
            unix_now_secs().map_err(|error| self.internal_status("quote time", error))?,
        )
        .map_err(|error| self.internal_status("quote timestamp", error))?;
        let bridge_fee_nicks = wallet_tx_builder::fee::compute_bridge_fee(
            gross_amount_nicks, policy.bridge_fee_nicks_per_started_nock,
        );
        if !self.config.admission_enabled
            || self.base_height_tracker.withdrawals_enabled() != Some(true)
        {
            return Ok(Response::new(proto::GetPublicWithdrawalQuoteResponse {
                available: false,
                gross_amount_nicks: gross_amount_nicks.to_string(),
                bridge_fee_nicks: bridge_fee_nicks.to_string(),
                transaction_fee_nicks: String::new(),
                net_payout_nicks: String::new(),
                snapshot_height: None,
                snapshot_block_id: None,
                reserved_input_count: 0,
                observed_at_unix_ms,
                revision,
                reason: "Withdrawal admission is paused; do not submit a burn.".to_string(),
            }));
        }
        let reserved_inputs = self
            .store
            .list_reserved_input_names()
            .await
            .map_err(|error| self.internal_status("quote reservations", error))?;
        match self
            .quote
            .quote(gross_amount_nicks, destination_lock_root, &reserved_inputs)
            .await
        {
            Ok(quote) => Ok(Response::new(proto::GetPublicWithdrawalQuoteResponse {
                available: quote.net_payout_nicks > 0,
                gross_amount_nicks: quote.gross_amount_nicks.to_string(),
                bridge_fee_nicks: quote.bridge_fee_nicks.to_string(),
                transaction_fee_nicks: quote.transaction_fee_nicks.to_string(),
                net_payout_nicks: quote.net_payout_nicks.to_string(),
                snapshot_height: Some(quote.snapshot_height),
                snapshot_block_id: Some(quote.snapshot_block_id.to_be_limb_bytes().to_vec()),
                reserved_input_count: quote.reserved_input_count,
                observed_at_unix_ms: quote.observed_at_unix_ms,
                revision,
                reason: String::new(),
            })),
            Err(error) => {
                warn!(
                    target: "bridge.withdrawal.public",
                    error = %error,
                    gross_amount_nicks,
                    "authoritative withdrawal quote unavailable"
                );
                Ok(Response::new(proto::GetPublicWithdrawalQuoteResponse {
                    available: false,
                    gross_amount_nicks: gross_amount_nicks.to_string(),
                    bridge_fee_nicks: bridge_fee_nicks.to_string(),
                    transaction_fee_nicks: String::new(),
                    net_payout_nicks: String::new(),
                    snapshot_height: None,
                    snapshot_block_id: None,
                    reserved_input_count: u64::try_from(reserved_inputs.len()).map_err(|error| {
                        Status::internal(format!("reserved input count overflow: {error}"))
                    })?,
                    observed_at_unix_ms,
                    revision,
                    reason:
                        "Authoritative quote has no safe unreserved liquidity; do not submit a burn."
                            .to_string(),
                }))
            }
        }
    }
}

fn unix_now_secs() -> Result<u64, BridgeError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| BridgeError::Runtime(format!("system clock before unix epoch: {err}")))
        .map(|duration| duration.as_secs())
}

fn seconds_i64_to_millis(seconds: i64) -> Result<i64, BridgeError> {
    seconds.checked_mul(1_000).ok_or_else(|| {
        BridgeError::ValueConversion("public timestamp milliseconds overflow".into())
    })
}

fn seconds_u64_to_millis_i64(seconds: u64) -> Result<i64, BridgeError> {
    let millis = seconds.checked_mul(1_000).ok_or_else(|| {
        BridgeError::ValueConversion("public timestamp milliseconds overflow".into())
    })?;
    i64::try_from(millis).map_err(|err| {
        BridgeError::ValueConversion(format!("public timestamp does not fit i64: {err}"))
    })
}

fn compose_public_revision(verified_at: i64, projection_revision: u64) -> Result<u64, BridgeError> {
    let verified_at = u64::try_from(verified_at).map_err(|err| {
        BridgeError::ValueConversion(format!("public verified_at is invalid: {err}"))
    })?;
    let base = verified_at
        .checked_mul(1_u64 << 32)
        .ok_or_else(|| BridgeError::ValueConversion("public revision base overflow".into()))?;
    Ok(base.saturating_add(projection_revision.min(u64::from(u32::MAX))))
}

pub async fn serve_public_withdrawal_query(
    addr: SocketAddr,
    service: PublicWithdrawalQueryService,
) -> Result<(), BridgeError> {
    info!(
        target: "bridge.withdrawal.public",
        %addr,
        "starting read-only public withdrawal query server"
    );
    let service = WithdrawalPublicQueryServer::new(service)
        .max_decoding_message_size(64 * 1024)
        .max_encoding_message_size(1024 * 1024);
    tonic::transport::Server::builder()
        .concurrency_limit_per_connection(32)
        .load_shed(true)
        .timeout(Duration::from_secs(10))
        .max_concurrent_streams(64)
        .add_service(service)
        .serve(addr)
        .await
        .map_err(|err| {
            BridgeError::Runtime(format!("public withdrawal query server failed: {err}"))
        })
}

#[cfg(test)]
async fn serve_public_withdrawal_query_with_shutdown(
    addr: SocketAddr,
    service: PublicWithdrawalQueryService,
    shutdown: tokio::sync::oneshot::Receiver<()>,
) -> Result<(), BridgeError> {
    let service = WithdrawalPublicQueryServer::new(service)
        .max_decoding_message_size(64 * 1024)
        .max_encoding_message_size(1024 * 1024);
    tonic::transport::Server::builder()
        .concurrency_limit_per_connection(32)
        .load_shed(true)
        .timeout(Duration::from_secs(10))
        .max_concurrent_streams(64)
        .add_service(service)
        .serve_with_shutdown(addr, async move {
            let _ = shutdown.await;
        })
        .await
        .map_err(|err| {
            BridgeError::Runtime(format!("public withdrawal query test server failed: {err}"))
        })
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::time::Duration;

    use nockapp::noun::slab::{NockJammer, NounSlab};
    use noun_serde::NounDecode;
    use tempfile::TempDir;
    use tokio::time::sleep;
    use tonic::Request;

    use super::*;
    use crate::shared::types::{BaseEventId, Tip5Hash, WithdrawalPolicy};
    use crate::withdrawal::sequencer::base_activity::{
        BaseActivityCursor, VerifiedBaseWithdrawalBurn,
    };
    use crate::withdrawal::submission::{
        WithdrawalNetworkSubmitStatus, WithdrawalSubmitAttemptStatus,
    };
    use crate::withdrawal::types::{WithdrawalProposalData, WithdrawalSnapshot};
    struct PublicQueryNockchain {
        tip: Option<u64>,
    }

    #[async_trait]
    impl WithdrawalSubmitPort for PublicQueryNockchain {
        async fn submit_withdrawal(
            &self,
            _proposal: &WithdrawalProposalData,
        ) -> Result<WithdrawalSubmitAttemptStatus, BridgeError> {
            Err(BridgeError::Runtime(
                "public query test does not submit withdrawals".into(),
            ))
        }

        async fn resubmit_raw_tx(
            &self,
            _raw_tx: &nockchain_types::v1::RawTx,
        ) -> Result<WithdrawalNetworkSubmitStatus, BridgeError> {
            Err(BridgeError::Runtime(
                "public query test does not resubmit withdrawals".into(),
            ))
        }

        async fn current_nockchain_tip_height(&self) -> Result<Option<u64>, BridgeError> {
            Ok(self.tip)
        }
    }
    struct PublicQueryQuote;

    #[async_trait]
    impl crate::withdrawal::quote::WithdrawalQuotePort for PublicQueryQuote {
        async fn quote(
            &self,
            gross_amount_nicks: u64,
            _destination_lock_root: Tip5Hash,
            reserved_inputs: &[nockchain_types::v1::Name],
        ) -> Result<crate::withdrawal::quote::PublicWithdrawalQuote, BridgeError> {
            let bridge_fee_nicks = wallet_tx_builder::fee::compute_bridge_fee(
                gross_amount_nicks,
                WithdrawalPolicy::v1().bridge_fee_nicks_per_started_nock,
            );
            let transaction_fee_nicks = 256;
            let net_payout_nicks = gross_amount_nicks
                .checked_sub(bridge_fee_nicks)
                .and_then(|amount| amount.checked_sub(transaction_fee_nicks))
                .ok_or_else(|| BridgeError::Runtime("test quote fee exceeds gross".into()))?;
            Ok(crate::withdrawal::quote::PublicWithdrawalQuote {
                gross_amount_nicks,
                bridge_fee_nicks,
                transaction_fee_nicks,
                net_payout_nicks,
                snapshot_height: 700,
                snapshot_block_id: Tip5Hash::from_limbs(&[71, 72, 73, 74, 75]),
                reserved_input_count: u64::try_from(reserved_inputs.len())
                    .expect("reserved input count"),
                observed_at_unix_ms: 1_700_000_000_000,
            })
        }
    }

    fn public_query_transaction() -> nockchain_types::v1::Transaction {
        const TRANSACTION_JAM: &[u8] = include_bytes!(
            "../../../test-fixtures/transactions/9MpGym52AumtwyBxYPyVsWHvcamUYwZkc1Nq7w3cFGF28u8ceVDwt3e.tx"
        );
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = slab
            .cue_into(TRANSACTION_JAM.to_vec().into())
            .expect("cue public query transaction");
        let space = nockapp::NounAllocator::noun_space(&slab);
        nockchain_types::v1::Transaction::from_noun(&noun, &space)
            .expect("decode public query transaction")
    }

    fn public_query_burn(
        seed: u8,
        tx_hash: B256,
        block_number: u64,
        log_index: u64,
        burner: Address,
    ) -> VerifiedBaseWithdrawalBurn {
        let policy = WithdrawalPolicy::v1();
        let amount_nicks = policy
            .minimum_gross_nocks
            .checked_mul(policy.nicks_per_nock)
            .and_then(|minimum| minimum.checked_add(u64::from(seed)))
            .expect("public query amount");
        VerifiedBaseWithdrawalBurn {
            chain_id: 8_453,
            nock_contract_address: Address::from([0x11; 20]),
            base_event_id: BaseEventId(vec![seed; 32]),
            block_number,
            block_hash: B256::from([seed.wrapping_add(1); 32]),
            parent_hash: B256::from([seed; 32]),
            observed_at_unix_secs: Some(1_700_000_000 + block_number),
            tx_hash,
            tx_index: u64::from(seed),
            log_index,
            burner,
            amount_base_units: u128::from(amount_nicks)
                .checked_mul(policy.base_units_per_nick)
                .expect("public query Base amount")
                .to_string(),
            amount_nicks,
            lock_root: Tip5Hash::from_limbs(&[
                u64::from(seed),
                u64::from(seed) + 1,
                u64::from(seed) + 2,
                u64::from(seed) + 3,
                u64::from(seed) + 4,
            ]),
            calldata: vec![seed; 116],
            base_batch_end: 199,
            withdrawal_nonce: None,
            verified_at: 1_700_000_100 + i64::from(seed),
            policy_id: Some(policy.id.to_string()),
            protocol_id: Some(policy.wire_format.to_string()),
        }
    }
    fn canonical_public_burn(burn: VerifiedBaseWithdrawalBurn) -> PublicBaseWithdrawalBurn {
        PublicBaseWithdrawalBurn {
            burn,
            canonical: true,
            invalidated_at: None,
            invalidation_generation: None,
            invalidation_reason: None,
        }
    }

    struct PublicQueryFixture {
        _directory: TempDir,
        store: Arc<WithdrawalSequencerStore>,
        service: PublicWithdrawalQueryService,
        burns: Vec<VerifiedBaseWithdrawalBurn>,
        confirmed_id: DomainWithdrawalId,
        burner: Address,
    }

    impl PublicQueryFixture {
        async fn new() -> Self {
            let directory = TempDir::new().expect("public query tempdir");
            let store = Arc::new(
                WithdrawalSequencerStore::open(directory.path().join("public-query.sqlite"))
                    .await
                    .expect("open public query store"),
            );
            let burner = Address::from([0x44; 20]);
            let shared_tx_hash = B256::from([0x55; 32]);
            let burns = vec![
                public_query_burn(0x10, shared_tx_hash, 100, 1, burner),
                public_query_burn(0x20, shared_tx_hash, 100, 2, burner),
                public_query_burn(0x30, B256::from([0x56; 32]), 101, 1, burner),
            ];
            let activity = store.base_activity_store();
            for burn in &burns {
                activity
                    .insert_verified_burn(burn.clone())
                    .await
                    .expect("insert public query burn");
            }
            activity
                .advance_cursor(BaseActivityCursor {
                    chain_id: 8_453,
                    nock_contract_address: Address::from([0x11; 20]),
                    last_verified_block: 199,
                    last_verified_block_hash: B256::from([0x57; 32]),
                    updated_at: 1_700_000_200,
                })
                .await
                .expect("advance public query Base cursor");
            store
                .recover_unmatched_base_burns(8_453, Address::from([0x11; 20]), 100, 199)
                .await
                .expect("recover public query burns");

            let first = &burns[0];
            let transaction = public_query_transaction();
            let proposal = WithdrawalProposalData {
                id: DomainWithdrawalId {
                    as_of: Tip5Hash::from_limbs(&[91, 92, 93, 94, 95]),
                    base_event_id: first.base_event_id.clone(),
                },
                recipient: first.lock_root.clone(),
                amount: first.amount_nicks - 1,
                burned_amount: first.amount_nicks,
                base_batch_end: first.base_batch_end,
                epoch: 0,
                snapshot: WithdrawalSnapshot {
                    height: 500,
                    block_id: Tip5Hash::from_limbs(&[81, 82, 83, 84, 85]),
                },
                selected_inputs: transaction.normalized_input_names(),
                transaction,
            };
            store
                .record_proposal_canonicalized(&proposal, 199)
                .await
                .expect("record public query canonical proposal");
            store
                .record_proposal_authorized(&proposal)
                .await
                .expect("record public query authorized proposal");
            store
                .record_submit_outcome(&proposal, WithdrawalState::MempoolAccepted, 1, 199, None)
                .await
                .expect("record public query mempool acceptance");
            store
                .record_tx_confirmed(&proposal, 700, Tip5Hash::from_limbs(&[71, 72, 73, 74, 75]))
                .await
                .expect("record public query confirmation");
            assert!(matches!(
                store
                    .reconcile_journal_with_base(8_453, Address::from([0x11; 20]), 100,)
                    .await
                    .expect("reconcile public query fixture"),
                crate::withdrawal::sequencer::store::BaseJournalReconciliationOutcome::Ready(_)
            ));
            let tracker = Arc::new(SequencerBaseHeightTracker::default());
            tracker.record_confirmed_base_height(199);
            tracker.record_withdrawals_enabled(Some(true));
            let service = PublicWithdrawalQueryService::new(
                store.clone(),
                tracker,
                Arc::new(PublicQueryNockchain { tip: Some(800) }),
                Arc::new(PublicQueryQuote),
                PublicWithdrawalQueryConfig {
                    base_chain_id: 8_453,
                    nock_contract_address: Address::from([0x11; 20]),
                    policy_id: WithdrawalPolicy::v1().id.to_string(),
                    protocol_id: WithdrawalPolicy::v1().wire_format.to_string(),
                    page_token_key: [0x77; 32],
                    delayed_after: Duration::from_secs(u64::MAX),
                    base_stale_after: Duration::from_secs(60),
                    admission_enabled: true,
                },
            );
            Self {
                _directory: directory,
                store,
                service,
                burns,
                confirmed_id: proposal.id,
                burner,
            }
        }

        fn deployment(&self) -> proto::PublicWithdrawalDeployment {
            self.service.deployment_proto()
        }
    }

    #[tokio::test]
    async fn public_withdrawal_query_resolves_safe_states_and_readiness() {
        let fixture = PublicQueryFixture::new().await;
        let shared_tx_hash = fixture.burns[0].tx_hash.as_slice().to_vec();

        let ambiguous = fixture
            .service
            .resolve_base_withdrawal(Request::new(proto::ResolveBaseWithdrawalRequest {
                deployment: Some(fixture.deployment()),
                base: Some(proto::PublicBaseWithdrawalLocator {
                    transaction_hash: shared_tx_hash.clone(),
                    log_index: None,
                }),
            }))
            .await
            .expect("resolve ambiguous transaction")
            .into_inner();
        assert_eq!(
            ambiguous.resolution,
            proto::PublicWithdrawalResolution::AmbiguousLog as i32
        );
        assert_eq!(ambiguous.candidate_log_indices, vec![1, 2]);
        assert!(ambiguous.withdrawal.is_none());

        let confirmed = fixture
            .service
            .resolve_base_withdrawal(Request::new(proto::ResolveBaseWithdrawalRequest {
                deployment: Some(fixture.deployment()),
                base: Some(proto::PublicBaseWithdrawalLocator {
                    transaction_hash: shared_tx_hash,
                    log_index: Some(1),
                }),
            }))
            .await
            .expect("resolve confirmed withdrawal")
            .into_inner()
            .withdrawal
            .expect("confirmed public withdrawal");
        assert_eq!(
            confirmed.status,
            proto::PublicWithdrawalStatus::Confirmed as i32
        );
        assert_eq!(
            confirmed.resolution,
            proto::PublicWithdrawalResolution::Found as i32
        );
        assert_eq!(confirmed.nock_confirmed_height, Some(700));
        assert!(confirmed.nock_confirmed_block_id.is_some());
        assert!(confirmed.net_amount_nicks.is_some());
        assert!(confirmed.withdrawal_id.is_some());

        let pending = fixture
            .service
            .get_withdrawal(Request::new(proto::GetPublicWithdrawalRequest {
                lookup: Some(proto::PublicWithdrawalLookupKey {
                    deployment: Some(fixture.deployment()),
                    key: Some(proto::public_withdrawal_lookup_key::Key::BaseEventId(
                        fixture.burns[1].base_event_id.0.clone(),
                    )),
                }),
            }))
            .await
            .expect("get pending withdrawal")
            .into_inner()
            .withdrawal
            .expect("pending public withdrawal");
        assert_eq!(
            pending.status,
            proto::PublicWithdrawalStatus::WithdrawalPending as i32
        );
        assert!(pending.withdrawal_id.is_none());
        assert!(pending.net_amount_nicks.is_none());

        let by_internal_id = fixture
            .service
            .get_withdrawal(Request::new(proto::GetPublicWithdrawalRequest {
                lookup: Some(proto::PublicWithdrawalLookupKey {
                    deployment: Some(fixture.deployment()),
                    key: Some(proto::public_withdrawal_lookup_key::Key::WithdrawalId(
                        withdrawal_id_to_proto(&fixture.confirmed_id),
                    )),
                }),
            }))
            .await
            .expect("get by internal withdrawal id")
            .into_inner();
        assert!(by_internal_id.found);
        assert_eq!(
            by_internal_id
                .withdrawal
                .expect("internal id withdrawal")
                .status,
            proto::PublicWithdrawalStatus::Confirmed as i32
        );

        let unknown = fixture
            .service
            .resolve_base_withdrawal(Request::new(proto::ResolveBaseWithdrawalRequest {
                deployment: Some(fixture.deployment()),
                base: Some(proto::PublicBaseWithdrawalLocator {
                    transaction_hash: vec![0x99; 32],
                    log_index: Some(0),
                }),
            }))
            .await
            .expect("resolve unknown withdrawal")
            .into_inner();
        assert_eq!(
            unknown.resolution,
            proto::PublicWithdrawalResolution::NotObserved as i32
        );
        assert_eq!(
            unknown.status,
            proto::PublicWithdrawalStatus::AwaitingBase as i32
        );

        let readiness = fixture
            .service
            .get_withdrawal_readiness(Request::new(proto::GetPublicWithdrawalReadinessRequest {
                deployment: Some(fixture.deployment()),
            }))
            .await
            .expect("get public readiness")
            .into_inner();
        assert_eq!(
            readiness.readiness,
            proto::PublicWithdrawalReadiness::Ready as i32
        );
        assert!(readiness.accepting_new_withdrawals);
        assert_eq!(
            readiness.reasons,
            vec![proto::PublicWithdrawalReadinessReason::Healthy as i32]
        );
        assert_eq!(readiness.minimum_gross_nocks, "100000");
        assert_eq!(readiness.nicks_per_nock, "65536");
        assert_eq!(readiness.bridge_fee_nicks_per_started_nock, "195");
        assert!(readiness.base_observed_at_unix_ms.is_some());
        let quote = fixture
            .service
            .get_withdrawal_quote(Request::new(proto::GetPublicWithdrawalQuoteRequest {
                deployment: Some(fixture.deployment()),
                gross_amount_nicks: fixture.burns[1].amount_nicks.to_string(),
                destination_lock_root: fixture.burns[1].lock_root.to_be_limb_bytes().to_vec(),
            }))
            .await
            .expect("get authoritative public quote")
            .into_inner();
        assert!(quote.available);
        assert_eq!(quote.transaction_fee_nicks, "256");
        assert!(quote.net_payout_nicks.parse::<u64>().expect("net quote") > 0);
        assert_eq!(quote.snapshot_height, Some(700));
    }

    #[tokio::test]
    async fn public_withdrawal_regresses_confirmed_facts_on_nock_reorg() {
        let fixture = PublicQueryFixture::new().await;
        let before = fixture
            .service
            .get_withdrawal(Request::new(proto::GetPublicWithdrawalRequest {
                lookup: Some(proto::PublicWithdrawalLookupKey {
                    deployment: Some(fixture.deployment()),
                    key: Some(proto::public_withdrawal_lookup_key::Key::WithdrawalId(
                        withdrawal_id_to_proto(&fixture.confirmed_id),
                    )),
                }),
            }))
            .await
            .expect("get confirmed withdrawal")
            .into_inner()
            .withdrawal
            .expect("confirmed withdrawal");
        let page_before = fixture
            .service
            .list_withdrawals_by_burner(Request::new(proto::ListPublicWithdrawalsByBurnerRequest {
                deployment: Some(fixture.deployment()),
                burner: fixture.burner.as_slice().to_vec(),
                page_size: 1,
                page_token: String::new(),
            }))
            .await
            .expect("history page before reorg")
            .into_inner();
        assert!(!page_before.next_page_token.is_empty());
        let invalidated_block_id = Tip5Hash::from_limbs(&[71, 72, 73, 74, 75]);
        fixture
            .store
            .record_nockchain_inclusion_invalidated(
                &fixture.confirmed_id,
                700,
                invalidated_block_id.clone(),
                WithdrawalState::ReorgHold,
                "Nockchain confirmation was orphaned".to_string(),
            )
            .await
            .expect("record Nockchain reorg");
        let stale_page = fixture
            .service
            .list_withdrawals_by_burner(Request::new(proto::ListPublicWithdrawalsByBurnerRequest {
                deployment: Some(fixture.deployment()),
                burner: fixture.burner.as_slice().to_vec(),
                page_size: 1,
                page_token: page_before.next_page_token,
            }))
            .await
            .expect_err("pre-reorg page token must be superseded");
        assert_eq!(stale_page.code(), tonic::Code::FailedPrecondition);

        let after = fixture
            .service
            .get_withdrawal(Request::new(proto::GetPublicWithdrawalRequest {
                lookup: Some(proto::PublicWithdrawalLookupKey {
                    deployment: Some(fixture.deployment()),
                    key: Some(proto::public_withdrawal_lookup_key::Key::WithdrawalId(
                        withdrawal_id_to_proto(&fixture.confirmed_id),
                    )),
                }),
            }))
            .await
            .expect("get reorged withdrawal")
            .into_inner()
            .withdrawal
            .expect("reorged withdrawal");
        assert!(after.revision > before.revision);
        assert_eq!(
            after.resolution,
            proto::PublicWithdrawalResolution::Reorged as i32
        );
        assert_eq!(after.status, proto::PublicWithdrawalStatus::Failure as i32);
        assert_eq!(after.recovery_generation, Some(1));
        assert_eq!(after.invalidated_block_height, Some(700));
        assert_eq!(
            after.invalidated_block_id,
            Some(invalidated_block_id.to_be_limb_bytes().to_vec())
        );
        assert_eq!(after.prior_status, "confirmed");
        assert!(after.recovery_reason.contains("orphaned"));
        assert_eq!(after.nock_confirmed_height, None);
        assert_eq!(after.nock_confirmed_block_id, None);
        assert_eq!(after.confirmed_at_unix_ms, None);
    }

    #[tokio::test]
    async fn public_readiness_fails_closed_while_base_reorg_guard_is_active() {
        let fixture = PublicQueryFixture::new().await;
        fixture
            .store
            .activate_base_reorg_guard(
                8_453,
                Address::from([0x11; 20]),
                "proof fixture: kernel hashchain has not been recovered".to_string(),
            )
            .await
            .expect("activate Base reorg guard");

        let readiness = fixture
            .service
            .get_withdrawal_readiness(Request::new(proto::GetPublicWithdrawalReadinessRequest {
                deployment: Some(fixture.deployment()),
            }))
            .await
            .expect("reorg-held readiness")
            .into_inner();

        assert!(!readiness.accepting_new_withdrawals);
        assert!(readiness
            .reasons
            .contains(&(proto::PublicWithdrawalReadinessReason::JournalReconciling as i32)));
    }

    #[tokio::test]
    async fn public_readiness_fails_closed_for_stale_or_paused_admission() {
        let fixture = PublicQueryFixture::new().await;

        let mut operator_paused = fixture.service.clone();
        operator_paused.config.admission_enabled = false;
        let paused = operator_paused
            .get_withdrawal_readiness(Request::new(proto::GetPublicWithdrawalReadinessRequest {
                deployment: Some(fixture.deployment()),
            }))
            .await
            .expect("operator-paused readiness")
            .into_inner();
        assert!(!paused.accepting_new_withdrawals);
        assert!(paused
            .reasons
            .contains(&(proto::PublicWithdrawalReadinessReason::WithdrawalsPaused as i32)));

        let stale_tracker = Arc::new(SequencerBaseHeightTracker::default());
        assert!(stale_tracker.record_confirmed_base_observation(199, 1));
        stale_tracker.record_withdrawals_enabled(Some(true));
        let mut stale = fixture.service.clone();
        stale.base_height_tracker = stale_tracker;
        let stale = stale
            .get_withdrawal_readiness(Request::new(proto::GetPublicWithdrawalReadinessRequest {
                deployment: Some(fixture.deployment()),
            }))
            .await
            .expect("stale readiness")
            .into_inner();
        assert!(!stale.accepting_new_withdrawals);
        assert!(stale
            .reasons
            .contains(&(proto::PublicWithdrawalReadinessReason::BaseScannerBehind as i32)));
        assert_eq!(stale.base_observed_at_unix_ms, Some(1));

        let contract_paused_tracker = Arc::new(SequencerBaseHeightTracker::default());
        contract_paused_tracker.record_confirmed_base_height(199);
        contract_paused_tracker.record_withdrawals_enabled(Some(false));
        let mut contract_paused = fixture.service.clone();
        contract_paused.base_height_tracker = contract_paused_tracker;
        let contract_paused = contract_paused
            .get_withdrawal_readiness(Request::new(proto::GetPublicWithdrawalReadinessRequest {
                deployment: Some(fixture.deployment()),
            }))
            .await
            .expect("contract-paused readiness")
            .into_inner();
        assert!(!contract_paused.accepting_new_withdrawals);
        assert!(contract_paused
            .reasons
            .contains(&(proto::PublicWithdrawalReadinessReason::WithdrawalsPaused as i32)));
    }

    #[tokio::test]
    async fn public_withdrawal_query_exposes_malformed_and_compensated_resolutions() {
        let fixture = PublicQueryFixture::new().await;
        let incident_store = fixture.store.base_activity_store().incident_store();

        let rejected_tx_hash = B256::from([0x71; 32]);
        let rejected_log_index = 7;
        let rejected_id =
            crate::shared::base::compute_base_event_id(&rejected_tx_hash, Some(rejected_log_index));
        incident_store
            .record_rejected_burns(vec![
                crate::withdrawal::sequencer::base_incidents::RejectedBaseWithdrawalBurn {
                    chain_id: 8_453,
                    nock_contract_address: Address::from([0x11; 20]),
                    base_event_id: rejected_id.clone(),
                    block_number: 102,
                    block_hash: B256::from([0x72; 32]),
                    parent_hash: B256::from([0x73; 32]),
                    observed_at_unix_secs: Some(1_700_000_102),
                    tx_hash: rejected_tx_hash,
                    tx_index: 4,
                    log_index: rejected_log_index,
                    burner: Some(fixture.burner),
                    amount_base_units: Some("1".to_string()),
                    commitment: Some(B256::from([0x74; 32])),
                    calldata: vec![0; 68],
                    rejection_code: "missing_calldata_trailer".to_string(),
                    rejection_detail: "missing withdrawal trailer".to_string(),
                    first_observed_at: 1_700_000_102,
                    last_observed_at: 1_700_000_102,
                },
            ])
            .await
            .expect("record rejected public burn");

        let malformed = fixture
            .service
            .get_withdrawal(Request::new(proto::GetPublicWithdrawalRequest {
                lookup: Some(proto::PublicWithdrawalLookupKey {
                    deployment: Some(fixture.deployment()),
                    key: Some(proto::public_withdrawal_lookup_key::Key::BaseEventId(
                        rejected_id.0,
                    )),
                }),
            }))
            .await
            .expect("get rejected public burn")
            .into_inner();
        assert!(!malformed.found);
        assert_eq!(
            malformed.resolution,
            proto::PublicWithdrawalResolution::MalformedBurn as i32
        );
        assert!(malformed.support_hint.contains("missing_calldata_trailer"));

        let compensated_tx_hash = B256::from([0x75; 32]);
        let compensated_log_index = 8;
        let compensated_id = crate::shared::base::compute_base_event_id(
            &compensated_tx_hash,
            Some(compensated_log_index),
        );
        let mut compensated_burn = public_query_burn(
            0x76, compensated_tx_hash, 103, compensated_log_index, fixture.burner,
        );
        compensated_burn.base_event_id = compensated_id.clone();
        fixture
            .store
            .base_activity_store()
            .insert_verified_burn(compensated_burn)
            .await
            .expect("insert compensated public burn");
        incident_store
            .record_compensated_withdrawals(vec![
                crate::withdrawal::sequencer::base_incidents::CompensatedBaseWithdrawal {
                    chain_id: 8_453,
                    nock_contract_address: Address::from([0x11; 20]),
                    base_event_id: compensated_id.clone(),
                    tx_hash: compensated_tx_hash,
                    log_index: compensated_log_index,
                    reason: "governance-approved compensation".to_string(),
                    evidence_reference: "incident:public-compensation".to_string(),
                    recorded_at: 1_700_000_103,
                },
            ])
            .await
            .expect("record public compensation");

        let compensated = fixture
            .service
            .get_withdrawal(Request::new(proto::GetPublicWithdrawalRequest {
                lookup: Some(proto::PublicWithdrawalLookupKey {
                    deployment: Some(fixture.deployment()),
                    key: Some(proto::public_withdrawal_lookup_key::Key::BaseEventId(
                        compensated_id.0,
                    )),
                }),
            }))
            .await
            .expect("get compensated public burn")
            .into_inner();
        assert!(compensated.found);
        assert_eq!(
            compensated.resolution,
            proto::PublicWithdrawalResolution::Compensated as i32
        );
        assert_eq!(
            compensated.withdrawal.expect("compensated record").status,
            proto::PublicWithdrawalStatus::Failure as i32
        );
    }

    #[tokio::test]
    async fn public_withdrawal_history_is_snapshot_stable_bounded_and_tamper_evident() {
        let fixture = PublicQueryFixture::new().await;
        let first_page = fixture
            .service
            .list_withdrawals_by_burner(Request::new(proto::ListPublicWithdrawalsByBurnerRequest {
                deployment: Some(fixture.deployment()),
                burner: fixture.burner.as_slice().to_vec(),
                page_size: 2,
                page_token: String::new(),
            }))
            .await
            .expect("list first public history page")
            .into_inner();
        assert_eq!(first_page.withdrawals.len(), 2);
        assert_eq!(
            first_page.withdrawals[0].base_event_id,
            fixture.burns[2].base_event_id.0
        );
        assert_eq!(
            first_page.withdrawals[1].base_event_id,
            fixture.burns[1].base_event_id.0
        );
        assert!(!first_page.next_page_token.is_empty());

        let new_burn = public_query_burn(0x40, B256::from([0x58; 32]), 200, 1, fixture.burner);
        fixture
            .store
            .base_activity_store()
            .insert_verified_burn(new_burn.clone())
            .await
            .expect("insert post-snapshot burn");
        let second_page = fixture
            .service
            .list_withdrawals_by_burner(Request::new(proto::ListPublicWithdrawalsByBurnerRequest {
                deployment: Some(fixture.deployment()),
                burner: fixture.burner.as_slice().to_vec(),
                page_size: 2,
                page_token: first_page.next_page_token.clone(),
            }))
            .await
            .expect("list second public history page")
            .into_inner();
        assert_eq!(second_page.withdrawals.len(), 1);
        assert_eq!(
            second_page.withdrawals[0].base_event_id,
            fixture.burns[0].base_event_id.0
        );
        assert!(second_page
            .withdrawals
            .iter()
            .all(|withdrawal| withdrawal.base_event_id != new_burn.base_event_id.0));
        assert_eq!(second_page.snapshot_revision, first_page.snapshot_revision);

        let fresh_page = fixture
            .service
            .list_withdrawals_by_burner(Request::new(proto::ListPublicWithdrawalsByBurnerRequest {
                deployment: Some(fixture.deployment()),
                burner: fixture.burner.as_slice().to_vec(),
                page_size: 1,
                page_token: String::new(),
            }))
            .await
            .expect("list fresh history snapshot")
            .into_inner();
        assert_eq!(
            fresh_page.withdrawals[0].base_event_id,
            new_burn.base_event_id.0
        );

        let mut tampered = first_page.next_page_token.into_bytes();
        let last = tampered.last_mut().expect("nonempty page token");
        *last = if *last == b'0' { b'1' } else { b'0' };
        let tampered_error = fixture
            .service
            .list_withdrawals_by_burner(Request::new(proto::ListPublicWithdrawalsByBurnerRequest {
                deployment: Some(fixture.deployment()),
                burner: fixture.burner.as_slice().to_vec(),
                page_size: 2,
                page_token: String::from_utf8(tampered).expect("hex token"),
            }))
            .await
            .expect_err("tampered page token must fail");
        assert_eq!(tampered_error.code(), tonic::Code::InvalidArgument);

        let oversized_error = fixture
            .service
            .list_withdrawals_by_burner(Request::new(proto::ListPublicWithdrawalsByBurnerRequest {
                deployment: Some(fixture.deployment()),
                burner: fixture.burner.as_slice().to_vec(),
                page_size: 101,
                page_token: String::new(),
            }))
            .await
            .expect_err("oversized public history page must fail");
        assert_eq!(oversized_error.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn public_withdrawal_server_routes_no_privileged_sequencer_methods() {
        let fixture = PublicQueryFixture::new().await;
        let listener = TcpListener::bind("127.0.0.1:0").expect("reserve public query port");
        let addr = listener.local_addr().expect("public query address");
        drop(listener);
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(serve_public_withdrawal_query_with_shutdown(
            addr,
            fixture.service.clone(),
            shutdown_rx,
        ));
        let endpoint = format!("http://{addr}");
        let mut public_client = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match proto::withdrawal_public_query_client::WithdrawalPublicQueryClient::connect(
                    endpoint.clone(),
                )
                .await
                {
                    Ok(client) => break client,
                    Err(_) => sleep(Duration::from_millis(10)).await,
                }
            }
        })
        .await
        .expect("public query server readiness timeout");
        let readiness = public_client
            .get_withdrawal_readiness(proto::GetPublicWithdrawalReadinessRequest {
                deployment: Some(fixture.deployment()),
            })
            .await
            .expect("public readiness over gRPC")
            .into_inner();
        assert_eq!(
            readiness.readiness,
            proto::PublicWithdrawalReadiness::Ready as i32
        );

        let mut private_client =
            proto::withdrawal_sequencer_client::WithdrawalSequencerClient::connect(endpoint)
                .await
                .expect("connect private client to public listener");
        let error = private_client
            .get_current_live_withdrawal_nonce(proto::CurrentLiveWithdrawalNonceRequest {})
            .await
            .expect_err("private sequencer RPC must not be routed publicly");
        assert_eq!(error.code(), tonic::Code::Unimplemented);

        shutdown_tx.send(()).expect("signal public query shutdown");
        server
            .await
            .expect("public query server task")
            .expect("public query server shutdown");
    }

    #[tokio::test]
    async fn public_withdrawal_status_normalization_never_exposes_internal_states() {
        let fixture = PublicQueryFixture::new().await;
        let burn = fixture.burns[1].clone();
        let id = DomainWithdrawalId {
            as_of: crate::shared::types::zero_tip5_hash(),
            base_event_id: burn.base_event_id.clone(),
        };
        let mut lifecycle = fixture
            .store
            .fetch_sequenced_withdrawal(&id)
            .await
            .expect("fetch pending lifecycle")
            .expect("pending lifecycle");
        for state in [
            WithdrawalState::Pending,
            WithdrawalState::Assembling,
            WithdrawalState::Prepared,
            WithdrawalState::PeerCanonical,
            WithdrawalState::Authorized,
            WithdrawalState::MempoolAccepted,
        ] {
            lifecycle.state = state;
            let record = fixture
                .service
                .build_record(
                    canonical_public_burn(burn.clone()),
                    None,
                    Some(&lifecycle),
                    None,
                    None,
                    None,
                    10,
                    burn.observed_at_unix_secs.expect("observed time"),
                )
                .expect("normalize internal lifecycle");
            assert_eq!(
                record.status,
                proto::PublicWithdrawalStatus::WithdrawalPending as i32
            );
        }

        let mut delayed_service = fixture.service.clone();
        delayed_service.config.delayed_after = Duration::from_secs(1);
        let delayed = delayed_service
            .build_record(
                canonical_public_burn(burn.clone()),
                None,
                Some(&lifecycle),
                None,
                None,
                None,
                11,
                burn.observed_at_unix_secs.expect("observed time") + 2,
            )
            .expect("normalize delayed lifecycle");
        assert_eq!(
            delayed.status,
            proto::PublicWithdrawalStatus::Delayed as i32
        );

        let mut below_policy = burn.clone();
        below_policy.amount_nicks = 1;
        let failure = fixture
            .service
            .build_record(
                canonical_public_burn(below_policy),
                None,
                None,
                None,
                None,
                None,
                12,
                1_700_000_000,
            )
            .expect("normalize below-policy burn");
        assert_eq!(
            failure.status,
            proto::PublicWithdrawalStatus::Failure as i32
        );
        assert_eq!(
            failure.resolution,
            proto::PublicWithdrawalResolution::BelowPolicy as i32
        );

        let mut unknown_policy = burn;
        unknown_policy.policy_id = None;
        let inconsistent = fixture
            .service
            .build_record(
                canonical_public_burn(unknown_policy),
                None,
                None,
                None,
                None,
                None,
                13,
                1_700_000_000,
            )
            .expect("normalize unknown policy");
        assert_eq!(
            inconsistent.status,
            proto::PublicWithdrawalStatus::Failure as i32
        );
        assert_eq!(
            inconsistent.resolution,
            proto::PublicWithdrawalResolution::Inconsistent as i32
        );
    }
}
