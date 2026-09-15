use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::Address;
use async_trait::async_trait;
use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use bridge::shared::ingress::proto as ingress_proto;
use nockchain_types::common::Hash as NockHash;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const STATUS_PATH: &str = "/withdrawal-status";

#[derive(Debug, Clone)]
pub struct BrowserStatusAdapterConfig {
    pub upstream_endpoint: String,
    pub run_dir: PathBuf,
    pub terminal_proof_path: PathBuf,
    pub run_id: String,
    pub base_chain_id: u64,
    pub nock: Address,
    pub message_inbox: Address,
    pub bridge_signer_pkhs: Vec<String>,
    pub bridge_threshold: u64,
    pub policy_id: String,
    pub protocol_id: String,
    pub iris_sdk_version: String,
}

impl BrowserStatusAdapterConfig {
    fn validate(&self) -> Result<(), BrowserStatusAdapterError> {
        require_loopback(&self.upstream_endpoint)?;
        if self.run_id.trim().is_empty()
            || self.nock == Address::ZERO
            || self.message_inbox == Address::ZERO
            || self.bridge_signer_pkhs.is_empty()
            || self.bridge_threshold == 0
            || self.bridge_threshold as usize > self.bridge_signer_pkhs.len()
            || self
                .bridge_signer_pkhs
                .iter()
                .any(|pkh| pkh.trim().is_empty())
            || self.policy_id.trim().is_empty()
            || self.protocol_id.trim().is_empty()
            || self.iris_sdk_version.trim().is_empty()
        {
            return Err(BrowserStatusAdapterError::InvalidConfig(
                "identity, deployment, signer, or protocol fields are invalid",
            ));
        }
        if !self.terminal_proof_path.starts_with(&self.run_dir) {
            return Err(BrowserStatusAdapterError::PathOutsideRun(
                self.terminal_proof_path.clone(),
            ));
        }
        Ok(())
    }

    fn deployment(&self) -> ingress_proto::PublicWithdrawalDeployment {
        ingress_proto::PublicWithdrawalDeployment {
            base_chain_id: self.base_chain_id,
            nock_contract_address: self.nock.as_slice().to_vec(),
            policy_id: self.policy_id.clone(),
            protocol_id: self.protocol_id.clone(),
        }
    }
}

pub struct BrowserStatusAdapter {
    endpoint: String,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<Result<(), String>>,
}

impl BrowserStatusAdapter {
    pub async fn start(
        config: BrowserStatusAdapterConfig,
    ) -> Result<Self, BrowserStatusAdapterError> {
        let source = Arc::new(GrpcBrowserStatusSource {
            endpoint: config.upstream_endpoint.clone(),
        });
        Self::start_with_source(config, source).await
    }

    pub async fn start_with_source(
        config: BrowserStatusAdapterConfig,
        source: Arc<dyn BrowserStatusSource>,
    ) -> Result<Self, BrowserStatusAdapterError> {
        config.validate()?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(BrowserStatusAdapterError::Bind)?;
        let address = listener
            .local_addr()
            .map_err(BrowserStatusAdapterError::Bind)?;
        let endpoint = format!("http://{address}{STATUS_PATH}");
        let state = AdapterState {
            config: Arc::new(config),
            source,
        };
        let app = Router::new()
            .route(STATUS_PATH, get(status_handler).options(options_handler))
            .layer(middleware::from_fn(local_http_headers))
            .with_state(state);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await
                .map_err(|error| error.to_string())
        });
        Ok(Self {
            endpoint,
            shutdown: Some(shutdown_tx),
            task,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn shutdown(mut self) -> Result<(), BrowserStatusAdapterError> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.task
            .await
            .map_err(BrowserStatusAdapterError::Join)?
            .map_err(BrowserStatusAdapterError::Server)
    }
}

#[async_trait]
pub trait BrowserStatusSource: Send + Sync {
    async fn get_readiness(
        &self,
        request: ingress_proto::GetPublicWithdrawalReadinessRequest,
    ) -> Result<ingress_proto::GetPublicWithdrawalReadinessResponse, String>;

    async fn get_withdrawal(
        &self,
        request: ingress_proto::GetPublicWithdrawalRequest,
    ) -> Result<ingress_proto::GetPublicWithdrawalResponse, String>;
    async fn list_withdrawals_by_burner(
        &self,
        request: ingress_proto::ListPublicWithdrawalsByBurnerRequest,
    ) -> Result<ingress_proto::ListPublicWithdrawalsByBurnerResponse, String>;

    async fn get_withdrawal_quote(
        &self,
        request: ingress_proto::GetPublicWithdrawalQuoteRequest,
    ) -> Result<ingress_proto::GetPublicWithdrawalQuoteResponse, String>;
}

#[derive(Debug, Clone)]
struct GrpcBrowserStatusSource {
    endpoint: String,
}

#[async_trait]
impl BrowserStatusSource for GrpcBrowserStatusSource {
    async fn get_readiness(
        &self,
        request: ingress_proto::GetPublicWithdrawalReadinessRequest,
    ) -> Result<ingress_proto::GetPublicWithdrawalReadinessResponse, String> {
        let mut client =
            ingress_proto::withdrawal_public_query_client::WithdrawalPublicQueryClient::connect(
                self.endpoint.clone(),
            )
            .await
            .map_err(|error| error.to_string())?;
        client
            .get_withdrawal_readiness(request)
            .await
            .map(|response| response.into_inner())
            .map_err(|error| error.to_string())
    }

    async fn get_withdrawal(
        &self,
        request: ingress_proto::GetPublicWithdrawalRequest,
    ) -> Result<ingress_proto::GetPublicWithdrawalResponse, String> {
        let mut client =
            ingress_proto::withdrawal_public_query_client::WithdrawalPublicQueryClient::connect(
                self.endpoint.clone(),
            )
            .await
            .map_err(|error| error.to_string())?;
        client
            .get_withdrawal(request)
            .await
            .map(|response| response.into_inner())
            .map_err(|error| error.to_string())
    }

    async fn list_withdrawals_by_burner(
        &self,
        request: ingress_proto::ListPublicWithdrawalsByBurnerRequest,
    ) -> Result<ingress_proto::ListPublicWithdrawalsByBurnerResponse, String> {
        let mut client =
            ingress_proto::withdrawal_public_query_client::WithdrawalPublicQueryClient::connect(
                self.endpoint.clone(),
            )
            .await
            .map_err(|error| error.to_string())?;
        client
            .list_withdrawals_by_burner(request)
            .await
            .map(|response| response.into_inner())
            .map_err(|error| error.to_string())
    }

    async fn get_withdrawal_quote(
        &self,
        request: ingress_proto::GetPublicWithdrawalQuoteRequest,
    ) -> Result<ingress_proto::GetPublicWithdrawalQuoteResponse, String> {
        let mut client =
            ingress_proto::withdrawal_public_query_client::WithdrawalPublicQueryClient::connect(
                self.endpoint.clone(),
            )
            .await
            .map_err(|error| error.to_string())?;
        client
            .get_withdrawal_quote(request)
            .await
            .map(|response| response.into_inner())
            .map_err(|error| error.to_string())
    }
}

#[derive(Clone)]
struct AdapterState {
    config: Arc<BrowserStatusAdapterConfig>,
    source: Arc<dyn BrowserStatusSource>,
}

#[derive(Debug, Default, Deserialize)]
struct StatusQuery {
    base_event_id: Option<String>,
    account: Option<String>,
    history: Option<String>,
    quote: Option<String>,
    limit: Option<u32>,
    gross_amount_nicks: Option<String>,
    destination_lock_root: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserReadinessResponse {
    schema_version: u64,
    observed_at: u64,
    base_observed_at: Option<u64>,
    ready: bool,
    chain_id: u64,
    nock_token_address: String,
    message_inbox_address: String,
    bridge_signer_pkhs: Vec<String>,
    bridge_threshold: u64,
    withdrawals_enabled: bool,
    operator_admission_enabled: bool,
    contract_gate_enabled: bool,
    minimum_gross_nocks: String,
    minimum_gross_nicks: String,
    minimum_gross_base_units: String,
    base_units_per_nock: String,
    nicks_per_nock: String,
    base_units_per_nick: String,
    bridge_fee_nicks_per_started_nock: String,
    maximum_nicks: String,
    withdrawal_wire_protocol: String,
    withdrawal_policy_id: String,
    iris_sdk_version: String,
    reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserWithdrawalStatusResponse {
    schema_version: u64,
    withdrawal_id: String,
    base_event_id: String,
    status: &'static str,
    resolution: &'static str,
    revision: String,
    recovery_generation: u64,
    terminal_proof: bool,
    nock_transaction_id: Option<String>,
    nock_block_id: Option<String>,
    actual_payout_nicks: Option<String>,
    invalidated_block_number: Option<String>,
    invalidated_block_hash: Option<String>,
    prior_status: Option<String>,
    recovery_reason: Option<String>,
    observed_at: u64,
    reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserWithdrawalHistoryResponse {
    schema_version: u64,
    revision: String,
    records: Vec<BrowserWithdrawalStatusResponse>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserWithdrawalQuoteResponse {
    schema_version: u64,
    available: bool,
    gross_amount_nicks: String,
    bridge_fee_nicks: String,
    transaction_fee_nicks: String,
    net_payout_nicks: String,
    snapshot_height: Option<u64>,
    snapshot_block_id: Option<String>,
    observed_at: u64,
    revision: String,
    reason: Option<String>,
}

async fn status_handler(
    State(state): State<AdapterState>,
    Query(query): Query<StatusQuery>,
) -> Response {
    let result = if truthy(query.quote.as_deref()) {
        quote_response(&state, &query).await
    } else if truthy(query.history.as_deref()) {
        history_response(&state, &query).await
    } else if let Some(base_event_id) = query.base_event_id.as_deref() {
        withdrawal_response(&state, base_event_id, query.account.as_deref()).await
    } else {
        readiness_response(&state).await
    };
    match result {
        Ok(response) => response,
        Err(error) => error_response(error),
    }
}

async fn options_handler() -> Response {
    with_no_store(StatusCode::NO_CONTENT.into_response())
}

async fn readiness_response(state: &AdapterState) -> Result<Response, HttpStatusError> {
    let readiness = state
        .source
        .get_readiness(ingress_proto::GetPublicWithdrawalReadinessRequest {
            deployment: Some(state.config.deployment()),
        })
        .await
        .map_err(HttpStatusError::Upstream)?;
    if readiness.policy_id != state.config.policy_id
        || readiness.protocol_id != state.config.protocol_id
    {
        return Err(HttpStatusError::InvalidUpstream(
            "withdrawal readiness protocol identity diverged",
        ));
    }
    let state_value = ingress_proto::PublicWithdrawalReadiness::try_from(readiness.readiness)
        .unwrap_or(ingress_proto::PublicWithdrawalReadiness::Unspecified);
    let ready = state_value == ingress_proto::PublicWithdrawalReadiness::Ready
        && readiness.accepting_new_withdrawals;
    let reason_labels = readiness
        .reasons
        .iter()
        .filter_map(|reason| {
            ingress_proto::PublicWithdrawalReadinessReason::try_from(*reason)
                .ok()
                .map(|reason| reason.as_str_name().to_owned())
        })
        .collect::<Vec<_>>()
        .join(", ");
    let reason = if ready {
        None
    } else {
        Some(
            [nonempty(readiness.support_hint), nonempty(reason_labels)]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" "),
        )
    };
    Ok(json_response(
        StatusCode::OK,
        &BrowserReadinessResponse {
            schema_version: 1,
            observed_at: nonnegative_ms(readiness.updated_at_unix_ms),
            base_observed_at: readiness.base_observed_at_unix_ms.map(nonnegative_ms),
            ready,
            chain_id: state.config.base_chain_id,
            nock_token_address: format!("{:#x}", state.config.nock),
            message_inbox_address: format!("{:#x}", state.config.message_inbox),
            bridge_signer_pkhs: state.config.bridge_signer_pkhs.clone(),
            bridge_threshold: state.config.bridge_threshold,
            withdrawals_enabled: readiness.contract_gate_enabled == Some(true),
            operator_admission_enabled: readiness.operator_admission_enabled,
            contract_gate_enabled: readiness.contract_gate_enabled == Some(true),
            minimum_gross_nocks: readiness.minimum_gross_nocks,
            minimum_gross_nicks: readiness.minimum_gross_nicks,
            minimum_gross_base_units: readiness.minimum_gross_base_units,
            base_units_per_nock: readiness.base_units_per_nock,
            nicks_per_nock: readiness.nicks_per_nock,
            base_units_per_nick: readiness.base_units_per_nick,
            bridge_fee_nicks_per_started_nock: readiness.bridge_fee_nicks_per_started_nock,
            maximum_nicks: readiness.maximum_nicks,
            withdrawal_wire_protocol: state.config.protocol_id.clone(),
            withdrawal_policy_id: state.config.policy_id.clone(),
            iris_sdk_version: state.config.iris_sdk_version.clone(),
            reason,
        },
    ))
}

async fn withdrawal_response(
    state: &AdapterState,
    base_event_id: &str,
    account: Option<&str>,
) -> Result<Response, HttpStatusError> {
    let event_id = decode_hex32(base_event_id)?;
    let account = parse_account(account)?;
    let response = state
        .source
        .get_withdrawal(ingress_proto::GetPublicWithdrawalRequest {
            lookup: Some(ingress_proto::PublicWithdrawalLookupKey {
                deployment: Some(state.config.deployment()),
                key: Some(
                    ingress_proto::public_withdrawal_lookup_key::Key::BaseEventId(
                        event_id.to_vec(),
                    ),
                ),
            }),
        })
        .await
        .map_err(HttpStatusError::Upstream)?;
    let status = if response.found {
        let record = response.withdrawal.ok_or(HttpStatusError::InvalidUpstream(
            "found response omitted withdrawal",
        ))?;
        status_from_record(state, &record, account, true)?
    } else {
        synthetic_status(
            &event_id, response.revision, response.resolution, response.support_hint,
        )?
    };
    Ok(json_response(StatusCode::OK, &status))
}

async fn history_response(
    state: &AdapterState,
    query: &StatusQuery,
) -> Result<Response, HttpStatusError> {
    let account = parse_account(query.account.as_deref())?.ok_or(HttpStatusError::BadRequest(
        "account is required for withdrawal history",
    ))?;
    let limit = query.limit.unwrap_or(50).clamp(1, 50);
    let response = state
        .source
        .list_withdrawals_by_burner(ingress_proto::ListPublicWithdrawalsByBurnerRequest {
            deployment: Some(state.config.deployment()),
            burner: account.as_slice().to_vec(),
            page_size: limit,
            page_token: String::new(),
        })
        .await
        .map_err(HttpStatusError::Upstream)?;
    let mut records = response
        .withdrawals
        .iter()
        .map(|record| status_from_record(state, record, Some(account), false))
        .collect::<Result<Vec<_>, _>>()?;
    records.sort_by(|left, right| {
        right
            .observed_at
            .cmp(&left.observed_at)
            .then_with(|| right.base_event_id.cmp(&left.base_event_id))
    });
    let mut seen_base_events = std::collections::HashSet::new();
    records.retain(|record| seen_base_events.insert(record.base_event_id.clone()));
    records.truncate(limit as usize);
    Ok(json_response(
        StatusCode::OK,
        &BrowserWithdrawalHistoryResponse {
            schema_version: 1,
            revision: browser_revision(response.snapshot_revision, false),
            records,
        },
    ))
}

async fn quote_response(
    state: &AdapterState,
    query: &StatusQuery,
) -> Result<Response, HttpStatusError> {
    let gross_amount_nicks =
        query
            .gross_amount_nicks
            .clone()
            .ok_or(HttpStatusError::BadRequest(
                "gross_amount_nicks is required",
            ))?;
    let destination = query
        .destination_lock_root
        .as_deref()
        .ok_or(HttpStatusError::BadRequest(
            "destination_lock_root is required",
        ))?;
    let lock_root = NockHash::from_base58(destination)
        .map_err(|_| HttpStatusError::BadRequest("destination_lock_root is invalid"))?;
    let response = state
        .source
        .get_withdrawal_quote(ingress_proto::GetPublicWithdrawalQuoteRequest {
            deployment: Some(state.config.deployment()),
            gross_amount_nicks,
            destination_lock_root: lock_root.to_be_limb_bytes().to_vec(),
        })
        .await
        .map_err(HttpStatusError::Upstream)?;
    let snapshot_block_id = response
        .snapshot_block_id
        .as_deref()
        .map(decode_nock_hash)
        .transpose()?;
    Ok(json_response(
        StatusCode::OK,
        &BrowserWithdrawalQuoteResponse {
            schema_version: 1,
            available: response.available,
            gross_amount_nicks: response.gross_amount_nicks,
            bridge_fee_nicks: response.bridge_fee_nicks,
            transaction_fee_nicks: response.transaction_fee_nicks,
            net_payout_nicks: response.net_payout_nicks,
            snapshot_height: response.snapshot_height,
            snapshot_block_id,
            observed_at: nonnegative_ms(response.observed_at_unix_ms),
            revision: response.revision.to_string(),
            reason: nonempty(response.reason),
        },
    ))
}

fn status_from_record(
    state: &AdapterState,
    record: &ingress_proto::PublicWithdrawalRecord,
    account: Option<Address>,
    terminal_proof_must_match: bool,
) -> Result<BrowserWithdrawalStatusResponse, HttpStatusError> {
    let event_id: [u8; 32] = record
        .base_event_id
        .as_slice()
        .try_into()
        .map_err(|_| HttpStatusError::InvalidUpstream("withdrawal identity is invalid"))?;
    validate_record(state, record, &event_id, account)?;
    let status = ingress_proto::PublicWithdrawalStatus::try_from(record.status)
        .unwrap_or(ingress_proto::PublicWithdrawalStatus::Unspecified);
    let resolution = ingress_proto::PublicWithdrawalResolution::try_from(record.resolution)
        .unwrap_or(ingress_proto::PublicWithdrawalResolution::Unspecified);
    let transaction_id = record.nock_transaction_name.clone();
    let nock_block_id = record
        .nock_confirmed_block_id
        .as_deref()
        .map(decode_nock_hash)
        .transpose()?;
    let terminal = if status == ingress_proto::PublicWithdrawalStatus::Confirmed
        && resolution == ingress_proto::PublicWithdrawalResolution::Found
        && record.canonical_base_event
    {
        terminal_gate(
            state,
            record,
            transaction_id.as_deref(),
            nock_block_id.as_deref(),
            terminal_proof_must_match,
        )?
    } else {
        false
    };
    let browser_status = if resolution == ingress_proto::PublicWithdrawalResolution::Reorged {
        "reorg_hold"
    } else {
        match status {
            ingress_proto::PublicWithdrawalStatus::Confirmed if terminal => "terminal",
            ingress_proto::PublicWithdrawalStatus::Confirmed => "sequencer_confirmed",
            ingress_proto::PublicWithdrawalStatus::Failure => "failed",
            ingress_proto::PublicWithdrawalStatus::WithdrawalPending
            | ingress_proto::PublicWithdrawalStatus::Delayed => "submitted",
            ingress_proto::PublicWithdrawalStatus::Draft
            | ingress_proto::PublicWithdrawalStatus::AwaitingBase
            | ingress_proto::PublicWithdrawalStatus::Unspecified => "pending",
        }
    };
    Ok(BrowserWithdrawalStatusResponse {
        schema_version: 2,
        withdrawal_id: format!("withdrawal:{}", hex::encode(event_id)),
        base_event_id: format!("0x{}", hex::encode(event_id)),
        status: browser_status,
        resolution: resolution_label(resolution),
        revision: browser_revision(record.revision, terminal),
        recovery_generation: record.recovery_generation.unwrap_or_default(),
        terminal_proof: terminal,
        nock_transaction_id: terminal.then_some(transaction_id).flatten(),
        nock_block_id: terminal.then_some(nock_block_id).flatten(),
        actual_payout_nicks: terminal.then(|| record.net_amount_nicks.clone()).flatten(),
        invalidated_block_number: record
            .invalidated_block_height
            .map(|value| value.to_string()),
        invalidated_block_hash: record
            .invalidated_block_id
            .as_ref()
            .map(|value| format!("0x{}", hex::encode(value))),
        prior_status: nonempty(record.prior_status.clone()),
        recovery_reason: nonempty(record.recovery_reason.clone()),
        observed_at: nonnegative_optional_ms(
            record.updated_at_unix_ms.or(record.observed_at_unix_ms),
        ),
        reason: nonempty(record.support_hint.clone()),
    })
}

fn synthetic_status(
    event_id: &[u8; 32],
    revision: u64,
    resolution: i32,
    support_hint: String,
) -> Result<BrowserWithdrawalStatusResponse, HttpStatusError> {
    let resolution = ingress_proto::PublicWithdrawalResolution::try_from(resolution)
        .unwrap_or(ingress_proto::PublicWithdrawalResolution::Unspecified);
    if !matches!(
        resolution,
        ingress_proto::PublicWithdrawalResolution::MalformedBurn
            | ingress_proto::PublicWithdrawalResolution::Compensated
    ) {
        return Err(HttpStatusError::NotFound);
    }
    Ok(BrowserWithdrawalStatusResponse {
        schema_version: 2,
        withdrawal_id: format!("withdrawal:{}", hex::encode(event_id)),
        base_event_id: format!("0x{}", hex::encode(event_id)),
        status: "failed",
        resolution: resolution_label(resolution),
        revision: browser_revision(revision, false),
        recovery_generation: 0,
        terminal_proof: false,
        nock_transaction_id: None,
        nock_block_id: None,
        actual_payout_nicks: None,
        invalidated_block_number: None,
        invalidated_block_hash: None,
        prior_status: None,
        recovery_reason: None,
        observed_at: unix_ms(),
        reason: nonempty(support_hint),
    })
}
fn validate_record(
    state: &AdapterState,
    record: &ingress_proto::PublicWithdrawalRecord,
    event_id: &[u8; 32],
    account: Option<Address>,
) -> Result<(), HttpStatusError> {
    if record.schema_version != 1 || record.base_event_id.as_slice() != event_id {
        return Err(HttpStatusError::InvalidUpstream(
            "withdrawal identity is invalid",
        ));
    }
    let deployment = record
        .deployment
        .as_ref()
        .ok_or(HttpStatusError::InvalidUpstream(
            "withdrawal deployment is missing",
        ))?;
    if deployment != &state.config.deployment() {
        return Err(HttpStatusError::InvalidUpstream(
            "withdrawal deployment diverged",
        ));
    }
    if record.burner.len() != 20 {
        return Err(HttpStatusError::InvalidUpstream(
            "withdrawal burner is malformed",
        ));
    }
    if account.is_some_and(|expected| record.burner.as_slice() != expected.as_slice()) {
        return Err(HttpStatusError::BadRequest(
            "account does not own the withdrawal",
        ));
    }
    Ok(())
}

fn terminal_gate(
    state: &AdapterState,
    record: &ingress_proto::PublicWithdrawalRecord,
    transaction_id: Option<&str>,
    block_id: Option<&str>,
    must_match: bool,
) -> Result<bool, HttpStatusError> {
    if !state.config.terminal_proof_path.is_file() {
        return Ok(false);
    }
    let bytes = match fs::read(&state.config.terminal_proof_path) {
        Ok(bytes) => bytes,
        Err(_) if !must_match => return Ok(false),
        Err(_) => {
            return Err(HttpStatusError::InvalidTerminalProof(
                "terminal proof cannot be read",
            ))
        }
    };
    let proof: BrowserTerminalGate = match serde_json::from_slice(&bytes) {
        Ok(proof) => proof,
        Err(_) if !must_match => return Ok(false),
        Err(_) => {
            return Err(HttpStatusError::InvalidTerminalProof(
                "terminal proof is malformed",
            ))
        }
    };
    if proof.schema_version != 1
        || proof.run_id != state.config.run_id
        || !proof.terminal
        || proof.burn_count != 1
        || proof.payout_count != 1
        || transaction_id != Some(proof.nock_transaction_id.as_str())
        || block_id != Some(proof.nock_block_id.as_str())
        || record.net_amount_nicks.as_deref() != Some(proof.payout_nicks.as_str())
        || !record.canonical_base_event
    {
        if !must_match {
            return Ok(false);
        }
        return Err(HttpStatusError::InvalidTerminalProof(
            "terminal proof does not match the public withdrawal",
        ));
    }
    Ok(true)
}

#[derive(Debug, Deserialize)]
struct BrowserTerminalGate {
    schema_version: u64,
    run_id: String,
    terminal: bool,
    nock_transaction_id: String,
    nock_block_id: String,
    burn_count: u64,
    payout_count: u64,
    payout_nicks: String,
}
fn resolution_label(resolution: ingress_proto::PublicWithdrawalResolution) -> &'static str {
    match resolution {
        ingress_proto::PublicWithdrawalResolution::Found => "found",
        ingress_proto::PublicWithdrawalResolution::NotObserved => "not_observed",
        ingress_proto::PublicWithdrawalResolution::AmbiguousLog => "ambiguous_log",
        ingress_proto::PublicWithdrawalResolution::MalformedBurn => "malformed_burn",
        ingress_proto::PublicWithdrawalResolution::BelowPolicy => "below_policy",
        ingress_proto::PublicWithdrawalResolution::Reorged => "reorged",
        ingress_proto::PublicWithdrawalResolution::Inconsistent => "inconsistent",
        ingress_proto::PublicWithdrawalResolution::Compensated => "compensated",
        ingress_proto::PublicWithdrawalResolution::Unspecified => "inconsistent",
    }
}

fn parse_account(value: Option<&str>) -> Result<Option<Address>, HttpStatusError> {
    value
        .map(|account| {
            account
                .parse::<Address>()
                .map_err(|_| HttpStatusError::BadRequest("account is not an EVM address"))
        })
        .transpose()
}

fn truthy(value: Option<&str>) -> bool {
    matches!(value, Some("1" | "true"))
}

fn decode_hex32(value: &str) -> Result<[u8; 32], HttpStatusError> {
    let raw = value.strip_prefix("0x").unwrap_or(value);
    let bytes =
        hex::decode(raw).map_err(|_| HttpStatusError::BadRequest("base_event_id is not hex"))?;
    bytes
        .try_into()
        .map_err(|_| HttpStatusError::BadRequest("base_event_id must be 32 bytes"))
}

fn decode_nock_hash(bytes: &[u8]) -> Result<String, HttpStatusError> {
    NockHash::from_be_limb_bytes(bytes)
        .map(|hash| hash.to_base58())
        .map_err(|_| HttpStatusError::InvalidUpstream("Nockchain block id is malformed"))
}
fn browser_revision(upstream_revision: u64, terminal_proof: bool) -> String {
    (u128::from(upstream_revision) * 2 + u128::from(terminal_proof)).to_string()
}

fn nonnegative_ms(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn nonnegative_optional_ms(value: Option<i64>) -> u64 {
    value.map(nonnegative_ms).unwrap_or_else(unix_ms)
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}
fn json_response<T: Serialize>(status: StatusCode, value: &T) -> Response {
    with_no_store((status, Json(value)).into_response())
}

fn error_response(error: HttpStatusError) -> Response {
    let status = match error {
        HttpStatusError::BadRequest(_) => StatusCode::BAD_REQUEST,
        HttpStatusError::NotFound => StatusCode::NOT_FOUND,
        HttpStatusError::ForbiddenOrigin => StatusCode::FORBIDDEN,
        HttpStatusError::Upstream(_) => StatusCode::BAD_GATEWAY,
        HttpStatusError::InvalidUpstream(_) | HttpStatusError::InvalidTerminalProof(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    json_response(status, &json!({ "error": error.to_string() }))
}

fn with_no_store(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
    response
}

async fn local_http_headers(request: Request, next: Next) -> Response {
    let origin = match request.headers().get(header::ORIGIN) {
        Some(value) => match allowed_loopback_origin(value) {
            Ok(origin) => Some(origin),
            Err(error) => return error_response(error),
        },
        None => None,
    };
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
    headers.insert(header::VARY, HeaderValue::from_static("Origin"));
    if let Some(origin) = origin {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, OPTIONS"),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("content-type"),
        );
    }
    response
}

fn allowed_loopback_origin(origin: &HeaderValue) -> Result<HeaderValue, HttpStatusError> {
    let text = origin
        .to_str()
        .map_err(|_| HttpStatusError::ForbiddenOrigin)?;
    let url = Url::parse(text).map_err(|_| HttpStatusError::ForbiddenOrigin)?;
    let loopback = match url.host_str().unwrap_or_default().trim_matches(['[', ']']) {
        "localhost" | "127.0.0.1" | "::1" => true,
        host => host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback()),
    };
    if !matches!(url.scheme(), "http" | "https")
        || !loopback
        || url.origin().ascii_serialization() != text
    {
        return Err(HttpStatusError::ForbiddenOrigin);
    }
    Ok(origin.clone())
}

fn require_loopback(value: &str) -> Result<(), BrowserStatusAdapterError> {
    let url = Url::parse(value)
        .map_err(|_| BrowserStatusAdapterError::InvalidConfig("upstream URL is invalid"))?;
    let loopback = match url.host_str().unwrap_or_default().trim_matches(['[', ']']) {
        "localhost" | "127.0.0.1" | "::1" => true,
        host => host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback()),
    };
    if !matches!(url.scheme(), "http" | "https") || !loopback {
        return Err(BrowserStatusAdapterError::InvalidConfig(
            "upstream URL must be loopback HTTP(S)",
        ));
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum BrowserStatusAdapterError {
    #[error("invalid browser status adapter config: {0}")]
    InvalidConfig(&'static str),
    #[error("browser status path is outside the run directory: {0}")]
    PathOutsideRun(PathBuf),
    #[error("failed to bind browser status adapter: {0}")]
    Bind(std::io::Error),
    #[error("browser status adapter task failed: {0}")]
    Join(tokio::task::JoinError),
    #[error("browser status adapter server failed: {0}")]
    Server(String),
}

#[derive(Debug, Error)]
enum HttpStatusError {
    #[error("bad status request: {0}")]
    BadRequest(&'static str),
    #[error("withdrawal was not observed")]
    NotFound,
    #[error("browser origin is not an explicit loopback origin")]
    ForbiddenOrigin,
    #[error("public withdrawal upstream failed: {0}")]
    Upstream(String),
    #[error("public withdrawal upstream returned invalid facts: {0}")]
    InvalidUpstream(&'static str),
    #[error("terminal proof is invalid: {0}")]
    InvalidTerminalProof(&'static str),
}
