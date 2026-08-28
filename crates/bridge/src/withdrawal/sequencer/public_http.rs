use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::Address;
use axum::extract::rejection::QueryRejection;
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tonic::Request;
use tracing::{info, warn};

use crate::shared::errors::BridgeError;
use crate::shared::ingress::proto;
use crate::shared::ingress::proto::withdrawal_public_query_server::WithdrawalPublicQuery;
use crate::shared::types::Tip5Hash;
use crate::withdrawal::sequencer::base_incidents::RejectedBaseWithdrawalBurn;
use crate::withdrawal::sequencer::public_rpc::PublicWithdrawalQueryService;

pub const WITHDRAWAL_PUBLIC_HTTP_PATH: &str = "/withdrawal-status";
const RATE_WINDOW_SECS: u64 = 60;
const MAX_RATE_CLIENTS: usize = 10_000;

#[derive(Debug, Clone)]
pub struct PublicWithdrawalHttpConfig {
    pub listen_addr: SocketAddr,
    pub allowed_origins: HashSet<String>,
    pub requests_per_minute: u32,
    pub message_inbox_address: Address,
    pub bridge_signer_pkhs: Vec<String>,
    pub bridge_threshold: u64,
    pub iris_sdk_version: String,
}

impl PublicWithdrawalHttpConfig {
    pub fn validate(&self) -> Result<(), BridgeError> {
        let bridge_signer_count =
            u64::try_from(self.bridge_signer_pkhs.len()).map_err(|error| {
                BridgeError::Config(format!("public withdrawal signer count overflow: {error}"))
            })?;
        let unique_bridge_signer_count =
            self.bridge_signer_pkhs.iter().collect::<HashSet<_>>().len();
        if self.allowed_origins.is_empty()
            || self.allowed_origins.contains("*")
            || self.requests_per_minute == 0
            || self.message_inbox_address == Address::ZERO
            || self.bridge_signer_pkhs.is_empty()
            || self.bridge_threshold == 0
            || self.bridge_threshold > bridge_signer_count
            || self
                .bridge_signer_pkhs
                .iter()
                .any(|signer| signer.trim().is_empty())
            || unique_bridge_signer_count != self.bridge_signer_pkhs.len()
            || self.iris_sdk_version.trim().is_empty()
        {
            return Err(BridgeError::Config(
                "public withdrawal HTTP identity, CORS, or rate-limit config is invalid".into(),
            ));
        }
        for origin in &self.allowed_origins {
            let header = origin.parse::<HeaderValue>().map_err(|_| {
                BridgeError::Config(format!("invalid public withdrawal CORS origin: {origin}"))
            })?;
            let url = Url::parse(origin).map_err(|_| {
                BridgeError::Config(format!("invalid public withdrawal CORS origin: {origin}"))
            })?;
            let host_is_loopback = match url.host_str().unwrap_or_default().trim_matches(['[', ']'])
            {
                "localhost" | "127.0.0.1" | "::1" => true,
                host => host
                    .parse::<IpAddr>()
                    .is_ok_and(|address| address.is_loopback()),
            };
            let secure = url.scheme() == "https";
            let explicit_loopback_http = url.scheme() == "http" && host_is_loopback;
            if header.as_bytes() != origin.as_bytes()
                || url.origin().ascii_serialization() != *origin
                || !(secure || explicit_loopback_http)
            {
                return Err(BridgeError::Config(format!(
                    "public withdrawal CORS origin must be a canonical HTTPS origin or explicit loopback HTTP origin: {origin}"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
struct HttpState {
    config: Arc<PublicWithdrawalHttpConfig>,
    service: PublicWithdrawalQueryService,
    limiter: Arc<RateLimiter>,
}

#[derive(Debug, Clone, Copy)]
struct RateWindow {
    started_at: u64,
    requests: u32,
}

struct RateLimiter {
    requests_per_minute: u32,
    clients: Mutex<HashMap<IpAddr, RateWindow>>,
}

impl RateLimiter {
    fn new(requests_per_minute: u32) -> Self {
        Self {
            requests_per_minute,
            clients: Mutex::new(HashMap::new()),
        }
    }

    fn check(&self, client: IpAddr, now: u64) -> Result<(), HttpError> {
        let mut clients = self
            .clients
            .lock()
            .map_err(|_| HttpError::Internal("rate limiter lock is poisoned"))?;
        if !clients.contains_key(&client) && clients.len() >= MAX_RATE_CLIENTS {
            clients.retain(|_, window| now.saturating_sub(window.started_at) < RATE_WINDOW_SECS);
            if clients.len() >= MAX_RATE_CLIENTS {
                return Err(HttpError::RateLimited);
            }
        }
        let window = clients.entry(client).or_insert(RateWindow {
            started_at: now,
            requests: 0,
        });
        if now.saturating_sub(window.started_at) >= RATE_WINDOW_SECS {
            *window = RateWindow {
                started_at: now,
                requests: 0,
            };
        }
        if window.requests >= self.requests_per_minute {
            return Err(HttpError::RateLimited);
        }
        window.requests = window.requests.saturating_add(1);
        Ok(())
    }
}

#[derive(Debug, Default, Deserialize)]
struct PublicQuery {
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
struct BrowserReadiness {
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
struct BrowserWithdrawalStatus {
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
struct BrowserWithdrawalHistory {
    schema_version: u64,
    revision: String,
    records: Vec<BrowserWithdrawalStatus>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserWithdrawalQuote {
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

pub async fn serve_public_withdrawal_http(
    config: PublicWithdrawalHttpConfig,
    service: PublicWithdrawalQueryService,
) -> Result<(), BridgeError> {
    config.validate()?;
    let listener = tokio::net::TcpListener::bind(config.listen_addr)
        .await
        .map_err(|error| {
            BridgeError::Runtime(format!("public withdrawal HTTP bind failed: {error}"))
        })?;
    info!(
        target: "bridge.withdrawal.public_http",
        listen_addr = %config.listen_addr,
        allowed_origin_count = config.allowed_origins.len(),
        requests_per_minute = config.requests_per_minute,
        "public withdrawal HTTP adapter listening"
    );
    let state = HttpState {
        limiter: Arc::new(RateLimiter::new(config.requests_per_minute)),
        config: Arc::new(config),
        service,
    };
    let app = Router::new()
        .route(
            WITHDRAWAL_PUBLIC_HTTP_PATH,
            get(public_handler).options(options_handler),
        )
        .with_state(state);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .map_err(|error| BridgeError::Runtime(format!("public withdrawal HTTP server failed: {error}")))
}

async fn public_handler(
    State(state): State<HttpState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    query: Result<Query<PublicQuery>, QueryRejection>,
) -> Response {
    let origin = match allowed_origin(&state.config, &headers) {
        Ok(origin) => origin,
        Err(error) => return error_response(error, None),
    };
    if let Err(error) = state.limiter.check(peer.ip(), unix_secs()) {
        return error_response(error, origin.as_ref());
    }
    let query = match query {
        Ok(Query(query)) => query,
        Err(_) => {
            return error_response(
                HttpError::BadRequest("query parameters are invalid"),
                origin.as_ref(),
            );
        }
    };
    let result = if truthy(query.quote.as_deref()) {
        quote_response(&state, query).await
    } else if truthy(query.history.as_deref()) {
        history_response(&state, query).await
    } else if let Some(base_event_id) = query.base_event_id.as_deref() {
        status_response(&state, base_event_id, query.account.as_deref()).await
    } else {
        readiness_response(&state).await
    };
    match result {
        Ok(response) => with_headers(response, origin.as_ref()),
        Err(error) => error_response(error, origin.as_ref()),
    }
}

async fn options_handler(
    State(state): State<HttpState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let origin = match allowed_origin(&state.config, &headers) {
        Ok(origin) => origin,
        Err(error) => return error_response(error, None),
    };
    if let Err(error) = state.limiter.check(peer.ip(), unix_secs()) {
        return error_response(error, origin.as_ref());
    }
    with_headers(StatusCode::NO_CONTENT.into_response(), origin.as_ref())
}

async fn readiness_response(state: &HttpState) -> Result<Response, HttpError> {
    let response = state
        .service
        .get_withdrawal_readiness(Request::new(proto::GetPublicWithdrawalReadinessRequest {
            deployment: Some(state.service.deployment()),
        }))
        .await
        .map_err(HttpError::Upstream)?
        .into_inner();
    let readiness = proto::PublicWithdrawalReadiness::try_from(response.readiness)
        .unwrap_or(proto::PublicWithdrawalReadiness::Unspecified);
    let ready =
        readiness == proto::PublicWithdrawalReadiness::Ready && response.accepting_new_withdrawals;
    let reason = if ready {
        None
    } else {
        nonempty(response.support_hint.clone()).or_else(|| {
            let labels = response
                .reasons
                .iter()
                .filter_map(|value| proto::PublicWithdrawalReadinessReason::try_from(*value).ok())
                .map(|reason| reason.as_str_name().to_owned())
                .collect::<Vec<_>>()
                .join(", ");
            nonempty(labels)
        })
    };
    let deployment = state.service.deployment();
    Ok(json_response(
        StatusCode::OK,
        &BrowserReadiness {
            schema_version: 1,
            observed_at: nonnegative_ms(response.updated_at_unix_ms),
            base_observed_at: response.base_observed_at_unix_ms.map(nonnegative_ms),
            ready,
            chain_id: deployment.base_chain_id,
            nock_token_address: format!("0x{}", hex::encode(deployment.nock_contract_address)),
            message_inbox_address: format!("{:#x}", state.config.message_inbox_address),
            bridge_signer_pkhs: state.config.bridge_signer_pkhs.clone(),
            bridge_threshold: state.config.bridge_threshold,
            withdrawals_enabled: response.contract_gate_enabled == Some(true),
            operator_admission_enabled: response.operator_admission_enabled,
            contract_gate_enabled: response.contract_gate_enabled == Some(true),
            minimum_gross_nocks: response.minimum_gross_nocks,
            minimum_gross_nicks: response.minimum_gross_nicks,
            minimum_gross_base_units: response.minimum_gross_base_units,
            base_units_per_nock: response.base_units_per_nock,
            nicks_per_nock: response.nicks_per_nock,
            base_units_per_nick: response.base_units_per_nick,
            bridge_fee_nicks_per_started_nock: response.bridge_fee_nicks_per_started_nock,
            maximum_nicks: response.maximum_nicks,
            withdrawal_wire_protocol: response.protocol_id,
            withdrawal_policy_id: response.policy_id,
            iris_sdk_version: state.config.iris_sdk_version.clone(),
            reason,
        },
    ))
}

async fn status_response(
    state: &HttpState,
    base_event_id: &str,
    account: Option<&str>,
) -> Result<Response, HttpError> {
    let event_id = decode_hex32(base_event_id)?;
    let account = parse_account(account)?;
    let response = state
        .service
        .get_withdrawal(Request::new(proto::GetPublicWithdrawalRequest {
            lookup: Some(proto::PublicWithdrawalLookupKey {
                deployment: Some(state.service.deployment()),
                key: Some(proto::public_withdrawal_lookup_key::Key::BaseEventId(
                    event_id.to_vec(),
                )),
            }),
        }))
        .await
        .map_err(HttpError::Upstream)?
        .into_inner();
    let status = if response.found {
        let record = response.withdrawal.ok_or(HttpError::InvalidUpstream(
            "found response omitted withdrawal",
        ))?;
        status_from_record(state, &record, account)?
    } else {
        synthetic_status(
            &event_id, response.revision, response.resolution, response.support_hint,
        )?
    };
    Ok(json_response(StatusCode::OK, &status))
}

async fn history_response(state: &HttpState, query: PublicQuery) -> Result<Response, HttpError> {
    let burner = parse_account(query.account.as_deref())?.ok_or(HttpError::BadRequest(
        "account is required for withdrawal history",
    ))?;
    let limit = query.limit.unwrap_or(50).clamp(1, 50);
    let response = state
        .service
        .list_withdrawals_by_burner(Request::new(proto::ListPublicWithdrawalsByBurnerRequest {
            deployment: Some(state.service.deployment()),
            burner: burner.as_slice().to_vec(),
            page_size: limit,
            page_token: String::new(),
        }))
        .await
        .map_err(HttpError::Upstream)?
        .into_inner();
    let mut records = response
        .withdrawals
        .iter()
        .map(|record| status_from_record(state, record, Some(burner)))
        .collect::<Result<Vec<_>, _>>()?;
    let rejected = state
        .service
        .rejected_burns_by_burner(burner, limit)
        .await
        .map_err(HttpError::Bridge)?;
    for incident in rejected {
        records.push(rejected_status(&incident, response.snapshot_revision));
    }
    records.sort_by(|left, right| {
        right
            .observed_at
            .cmp(&left.observed_at)
            .then_with(|| right.base_event_id.cmp(&left.base_event_id))
    });
    let mut seen_base_events = HashSet::new();
    records.retain(|record| seen_base_events.insert(record.base_event_id.clone()));
    records.truncate(limit as usize);
    Ok(json_response(
        StatusCode::OK,
        &BrowserWithdrawalHistory {
            schema_version: 1,
            revision: response.snapshot_revision.to_string(),
            records,
        },
    ))
}

async fn quote_response(state: &HttpState, query: PublicQuery) -> Result<Response, HttpError> {
    let gross_amount_nicks = query
        .gross_amount_nicks
        .ok_or(HttpError::BadRequest("gross_amount_nicks is required"))?;
    let destination = query
        .destination_lock_root
        .ok_or(HttpError::BadRequest("destination_lock_root is required"))?;
    let lock_root = Tip5Hash::from_base58(&destination)
        .map_err(|_| HttpError::BadRequest("destination_lock_root is invalid"))?;
    let response = state
        .service
        .get_withdrawal_quote(Request::new(proto::GetPublicWithdrawalQuoteRequest {
            deployment: Some(state.service.deployment()),
            gross_amount_nicks,
            destination_lock_root: lock_root.to_be_limb_bytes().to_vec(),
        }))
        .await
        .map_err(HttpError::Upstream)?
        .into_inner();
    let snapshot_block_id = response
        .snapshot_block_id
        .as_deref()
        .map(decode_nock_hash)
        .transpose()?;
    Ok(json_response(
        StatusCode::OK,
        &BrowserWithdrawalQuote {
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
    state: &HttpState,
    record: &proto::PublicWithdrawalRecord,
    account: Option<Address>,
) -> Result<BrowserWithdrawalStatus, HttpError> {
    validate_record(state, record, account)?;
    let status = proto::PublicWithdrawalStatus::try_from(record.status)
        .unwrap_or(proto::PublicWithdrawalStatus::Unspecified);
    let resolution = proto::PublicWithdrawalResolution::try_from(record.resolution)
        .unwrap_or(proto::PublicWithdrawalResolution::Unspecified);
    let nock_block_id = record
        .nock_confirmed_block_id
        .as_deref()
        .map(decode_nock_hash)
        .transpose()?;
    let terminal = status == proto::PublicWithdrawalStatus::Confirmed
        && resolution == proto::PublicWithdrawalResolution::Found
        && record.canonical_base_event
        && record
            .nock_transaction_name
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        && nock_block_id.is_some()
        && record
            .net_amount_nicks
            .as_deref()
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|value| value > 0);
    let browser_status = if resolution == proto::PublicWithdrawalResolution::Reorged {
        "reorg_hold"
    } else {
        match status {
            proto::PublicWithdrawalStatus::Confirmed if terminal => "terminal",
            proto::PublicWithdrawalStatus::Confirmed => "sequencer_confirmed",
            proto::PublicWithdrawalStatus::Failure => "failed",
            proto::PublicWithdrawalStatus::WithdrawalPending
            | proto::PublicWithdrawalStatus::Delayed => "submitted",
            proto::PublicWithdrawalStatus::Draft
            | proto::PublicWithdrawalStatus::AwaitingBase
            | proto::PublicWithdrawalStatus::Unspecified => "pending",
        }
    };
    Ok(BrowserWithdrawalStatus {
        schema_version: 2,
        withdrawal_id: format!("withdrawal:{}", hex::encode(&record.base_event_id)),
        base_event_id: format!("0x{}", hex::encode(&record.base_event_id)),
        status: browser_status,
        resolution: resolution_label(resolution),
        revision: record.revision.to_string(),
        recovery_generation: record.recovery_generation.unwrap_or_default(),
        terminal_proof: terminal,
        nock_transaction_id: terminal
            .then(|| record.nock_transaction_name.clone())
            .flatten(),
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
) -> Result<BrowserWithdrawalStatus, HttpError> {
    let resolution = proto::PublicWithdrawalResolution::try_from(resolution)
        .unwrap_or(proto::PublicWithdrawalResolution::Unspecified);
    if !matches!(
        resolution,
        proto::PublicWithdrawalResolution::MalformedBurn
            | proto::PublicWithdrawalResolution::Compensated
    ) {
        return Err(HttpError::NotFound);
    }
    Ok(BrowserWithdrawalStatus {
        schema_version: 2,
        withdrawal_id: format!("withdrawal:{}", hex::encode(event_id)),
        base_event_id: format!("0x{}", hex::encode(event_id)),
        status: "failed",
        resolution: resolution_label(resolution),
        revision: revision.to_string(),
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

fn rejected_status(
    incident: &RejectedBaseWithdrawalBurn,
    revision: u64,
) -> BrowserWithdrawalStatus {
    BrowserWithdrawalStatus {
        schema_version: 2,
        withdrawal_id: format!("withdrawal:{}", hex::encode(&incident.base_event_id.0)),
        base_event_id: format!("0x{}", hex::encode(&incident.base_event_id.0)),
        status: "failed",
        resolution: "malformed_burn",
        revision: revision.to_string(),
        recovery_generation: 0,
        terminal_proof: false,
        nock_transaction_id: None,
        nock_block_id: None,
        actual_payout_nicks: None,
        invalidated_block_number: None,
        invalidated_block_hash: None,
        prior_status: None,
        recovery_reason: None,
        observed_at: incident
            .observed_at_unix_secs
            .and_then(|value| value.checked_mul(1_000))
            .unwrap_or_else(unix_ms),
        reason: Some(format!(
            "Unsupported withdrawal burn ({}). Contact support; do not burn again.",
            incident.rejection_code
        )),
    }
}

fn validate_record(
    state: &HttpState,
    record: &proto::PublicWithdrawalRecord,
    account: Option<Address>,
) -> Result<(), HttpError> {
    if record.schema_version != 1 || record.base_event_id.len() != 32 {
        return Err(HttpError::InvalidUpstream("withdrawal identity is invalid"));
    }
    if record.deployment.as_ref() != Some(&state.service.deployment()) {
        return Err(HttpError::InvalidUpstream("withdrawal deployment diverged"));
    }
    if record.burner.len() != 20 {
        return Err(HttpError::InvalidUpstream("withdrawal burner is malformed"));
    }
    if account.is_some_and(|expected| record.burner.as_slice() != expected.as_slice()) {
        return Err(HttpError::BadRequest("account does not own the withdrawal"));
    }
    Ok(())
}

fn resolution_label(resolution: proto::PublicWithdrawalResolution) -> &'static str {
    match resolution {
        proto::PublicWithdrawalResolution::Found => "found",
        proto::PublicWithdrawalResolution::NotObserved => "not_observed",
        proto::PublicWithdrawalResolution::AmbiguousLog => "ambiguous_log",
        proto::PublicWithdrawalResolution::MalformedBurn => "malformed_burn",
        proto::PublicWithdrawalResolution::BelowPolicy => "below_policy",
        proto::PublicWithdrawalResolution::Reorged => "reorged",
        proto::PublicWithdrawalResolution::Inconsistent => "inconsistent",
        proto::PublicWithdrawalResolution::Compensated => "compensated",
        proto::PublicWithdrawalResolution::Unspecified => "inconsistent",
    }
}

fn allowed_origin(
    config: &PublicWithdrawalHttpConfig,
    headers: &HeaderMap,
) -> Result<Option<HeaderValue>, HttpError> {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return Ok(None);
    };
    let origin_text = origin.to_str().map_err(|_| HttpError::ForbiddenOrigin)?;
    if !config.allowed_origins.contains(origin_text) {
        return Err(HttpError::ForbiddenOrigin);
    }
    Ok(Some(origin.clone()))
}

fn with_headers(mut response: Response, origin: Option<&HeaderValue>) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::VARY, HeaderValue::from_static("Origin"));
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, OPTIONS"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type"),
    );
    if let Some(origin) = origin {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin.clone());
    }
    response
}

fn error_response(error: HttpError, origin: Option<&HeaderValue>) -> Response {
    let status = match &error {
        HttpError::BadRequest(_) => StatusCode::BAD_REQUEST,
        HttpError::ForbiddenOrigin => StatusCode::FORBIDDEN,
        HttpError::NotFound => StatusCode::NOT_FOUND,
        HttpError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        HttpError::Upstream(_) | HttpError::Bridge(_) => StatusCode::BAD_GATEWAY,
        HttpError::InvalidUpstream(_) | HttpError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let message = public_error_message(&error);
    if matches!(
        &error,
        HttpError::Upstream(_)
            | HttpError::Bridge(_)
            | HttpError::InvalidUpstream(_)
            | HttpError::Internal(_)
    ) {
        warn!(
            target: "bridge.withdrawal.public_http",
            error = %error,
            "public withdrawal HTTP request failed"
        );
    }
    with_headers(json_response(status, &json!({ "error": message })), origin)
}

fn public_error_message(error: &HttpError) -> String {
    match error {
        HttpError::BadRequest(_)
        | HttpError::ForbiddenOrigin
        | HttpError::NotFound
        | HttpError::RateLimited => error.to_string(),
        HttpError::Upstream(_) | HttpError::Bridge(_) => {
            "public withdrawal service is temporarily unavailable".to_owned()
        }
        HttpError::InvalidUpstream(_) | HttpError::Internal(_) => {
            "public withdrawal service encountered invalid state".to_owned()
        }
    }
}

fn json_response<T: Serialize>(status: StatusCode, value: &T) -> Response {
    (status, Json(value)).into_response()
}

fn parse_account(value: Option<&str>) -> Result<Option<Address>, HttpError> {
    value
        .map(|value| {
            value
                .parse::<Address>()
                .map_err(|_| HttpError::BadRequest("account is not an EVM address"))
        })
        .transpose()
}

fn decode_hex32(value: &str) -> Result<[u8; 32], HttpError> {
    let raw = value.strip_prefix("0x").unwrap_or(value);
    hex::decode(raw)
        .map_err(|_| HttpError::BadRequest("base_event_id is not hex"))?
        .try_into()
        .map_err(|_| HttpError::BadRequest("base_event_id must be 32 bytes"))
}

fn decode_nock_hash(bytes: &[u8]) -> Result<String, HttpError> {
    Tip5Hash::from_be_limb_bytes(bytes)
        .map(|hash| hash.to_base58())
        .map_err(|_| HttpError::InvalidUpstream("Nockchain block id is malformed"))
}

fn truthy(value: Option<&str>) -> bool {
    matches!(value, Some("1" | "true"))
}

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn nonnegative_ms(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

fn nonnegative_optional_ms(value: Option<i64>) -> u64 {
    value.map(nonnegative_ms).unwrap_or_else(unix_ms)
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or_default()
}

#[derive(Debug, thiserror::Error)]
enum HttpError {
    #[error("bad public withdrawal request: {0}")]
    BadRequest(&'static str),
    #[error("request Origin is not allowed")]
    ForbiddenOrigin,
    #[error("withdrawal was not observed")]
    NotFound,
    #[error("public withdrawal request rate exceeded")]
    RateLimited,
    #[error("public withdrawal upstream failed: {0}")]
    Upstream(tonic::Status),
    #[error("public withdrawal storage failed: {0}")]
    Bridge(BridgeError),
    #[error("public withdrawal upstream returned invalid facts: {0}")]
    InvalidUpstream(&'static str),
    #[error("public withdrawal HTTP internal error: {0}")]
    Internal(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_config() -> PublicWithdrawalHttpConfig {
        PublicWithdrawalHttpConfig {
            listen_addr: "127.0.0.1:8080".parse().expect("listen address"),
            allowed_origins: HashSet::from(["https://swap.example".to_owned()]),
            requests_per_minute: 2,
            message_inbox_address: Address::from([0x11; 20]),
            bridge_signer_pkhs: vec!["signer-0".to_owned(), "signer-1".to_owned()],
            bridge_threshold: 2,
            iris_sdk_version: "0.3.3".to_owned(),
        }
    }

    #[test]
    fn config_rejects_unsafe_origins_and_signer_rosters() {
        let mut config = valid_config();
        config.allowed_origins = HashSet::from(["*".to_owned()]);
        assert!(config.validate().is_err());

        config.allowed_origins = HashSet::from(["http://swap.example".to_owned()]);
        assert!(config.validate().is_err());

        config.allowed_origins = HashSet::from(["https://swap.example/path".to_owned()]);
        assert!(config.validate().is_err());

        config.allowed_origins = HashSet::from(["http://127.0.0.1:3000".to_owned()]);
        assert!(config.validate().is_ok());

        config.bridge_signer_pkhs = vec!["same-signer".to_owned(), "same-signer".to_owned()];
        assert!(config.validate().is_err());
    }

    #[test]
    fn internal_error_details_are_not_exposed_to_public_clients() {
        let internal = HttpError::Internal("private invariant detail");
        let invalid_upstream = HttpError::InvalidUpstream("private upstream detail");

        assert_eq!(
            public_error_message(&internal),
            "public withdrawal service encountered invalid state"
        );
        assert_eq!(
            public_error_message(&invalid_upstream),
            "public withdrawal service encountered invalid state"
        );
        assert!(!public_error_message(&internal).contains("private"));
        assert!(!public_error_message(&invalid_upstream).contains("private"));
    }

    #[test]
    fn rate_limit_is_per_client_and_resets_at_the_next_window() {
        let limiter = RateLimiter::new(2);
        let first: IpAddr = "127.0.0.1".parse().expect("first client");
        let second: IpAddr = "127.0.0.2".parse().expect("second client");
        assert!(limiter.check(first, 100).is_ok());
        assert!(limiter.check(first, 101).is_ok());
        assert!(matches!(
            limiter.check(first, 102),
            Err(HttpError::RateLimited)
        ));
        assert!(limiter.check(second, 102).is_ok());
        assert!(limiter.check(first, 160).is_ok());
    }

    #[test]
    fn rate_limiter_sweeps_expired_clients_only_when_capacity_is_reached() {
        let limiter = RateLimiter::new(2);
        {
            let mut clients = limiter.clients.lock().expect("rate limiter clients");
            for index in 0..MAX_RATE_CLIENTS {
                clients.insert(
                    IpAddr::V6(std::net::Ipv6Addr::from(index as u128)),
                    RateWindow {
                        started_at: 0,
                        requests: 1,
                    },
                );
            }
        }

        let new_client = IpAddr::V6(std::net::Ipv6Addr::from(u128::MAX));
        assert!(limiter.check(new_client, RATE_WINDOW_SECS).is_ok());
        assert_eq!(
            limiter.clients.lock().expect("rate limiter clients").len(),
            1
        );
    }

    #[test]
    fn cors_requires_an_allowlisted_exact_origin_and_disables_caching() {
        let config = valid_config();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://swap.example"),
        );
        let origin = allowed_origin(&config, &headers)
            .expect("allowed origin")
            .expect("origin present");
        let response = with_headers(StatusCode::OK.into_response(), Some(&origin));
        assert_eq!(
            response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&origin)
        );
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL),
            Some(&HeaderValue::from_static("no-store"))
        );

        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://attacker.example"),
        );
        assert!(matches!(
            allowed_origin(&config, &headers),
            Err(HttpError::ForbiddenOrigin)
        ));
    }

    #[test]
    fn unresolved_malformed_and_compensated_burns_are_explicit_failures() {
        let event_id = [0x44; 32];
        for (resolution, expected) in [
            (
                proto::PublicWithdrawalResolution::MalformedBurn,
                "malformed_burn",
            ),
            (
                proto::PublicWithdrawalResolution::Compensated,
                "compensated",
            ),
        ] {
            let status = synthetic_status(
                &event_id,
                9,
                resolution as i32,
                "contact support".to_owned(),
            )
            .expect("synthetic terminal rejection");
            assert_eq!(status.schema_version, 2);
            assert_eq!(status.status, "failed");
            assert_eq!(status.resolution, expected);
            assert_eq!(status.revision, "9");
            assert!(!status.terminal_proof);
        }
    }
}
