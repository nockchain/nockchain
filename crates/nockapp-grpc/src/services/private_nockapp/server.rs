use std::net::SocketAddr;
use std::pin::Pin;

use futures::Stream;
use nockapp::driver::{NockAppHandle, PokeResult};
use nockapp::noun::slab::NounSlab;
use nockvm::noun::NounAllocator;
use tokio::sync::broadcast::error::RecvError;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Server;
use tonic::{Request, Response, Status};
use tonic_health::server::health_reporter;
use tonic_reflection::server::Builder as ReflectionBuilder;
use tracing::{debug, error, info, warn};

use crate::error::{NockAppGrpcError, Result};
use crate::pb::common::v1::{wire_tag, ErrorCode, ErrorStatus, Wire};
use crate::pb::private::v1::nock_app_service_server::{
    NockAppService as PrivateNockApp, NockAppServiceServer as PrivateNockAppServer,
};
use crate::pb::private::v1::*;
use crate::wire_conversion::grpc_wire_to_nockapp;

pub const PRIVATE_GRPC_AUTH_TOKEN_ENV: &str = "NOCKCHAIN_PRIVATE_GRPC_AUTH_TOKEN";
pub const PRIVATE_GRPC_MINER_TOKEN_ENV: &str = "NOCKCHAIN_PRIVATE_GRPC_MINER_TOKEN";
/// Explicit opt-out from bearer authentication. Without an operator token the
/// server refuses to start unless this is set (or the equivalent node CLI flag
/// `--allow-unauthenticated-private-grpc` is passed); every caller is then an
/// operator, which is only acceptable for a single-user loopback deployment.
pub const PRIVATE_GRPC_ALLOW_UNAUTHENTICATED_ENV: &str =
    "NOCKCHAIN_ALLOW_UNAUTHENTICATED_PRIVATE_GRPC";
pub const PRIVATE_GRPC_AUTH_HEADER: &str = "x-nockchain-private-token";
const PRIVATE_GRPC_MIN_TOKEN_LEN: usize = 32;

/// Validated startup snapshot. Credentials are intentionally not `Debug` and
/// cannot change between node initialization and the server task starting.
pub struct PrivateGrpcServerConfig {
    addr: SocketAddr,
    operator: Option<String>,
    miner: Option<String>,
    allow_unauthenticated: bool,
}

impl PrivateGrpcServerConfig {
    pub fn new(
        addr: SocketAddr,
        operator: Option<String>,
        miner: Option<String>,
        allow_unauthenticated: bool,
    ) -> Result<Self> {
        let config = Self {
            addr,
            operator: operator.filter(|token| !token.is_empty()),
            miner: miner.filter(|token| !token.is_empty()),
            allow_unauthenticated,
        };
        config.validate()?;
        Ok(config)
    }

    /// Resolve environment policy once, before starting the kernel or drivers.
    pub fn from_env(addr: SocketAddr, allow_unauthenticated: bool) -> Result<Self> {
        Self::from_env_reader(addr, allow_unauthenticated, |name| {
            match std::env::var(name) {
                Ok(value) => Ok(Some(value)),
                Err(std::env::VarError::NotPresent) => Ok(None),
                Err(std::env::VarError::NotUnicode(_)) => Err(NockAppGrpcError::InvalidRequest(
                    format!("{name} must contain valid UTF-8"),
                )),
            }
        })
    }

    fn from_env_reader(
        addr: SocketAddr,
        allow_unauthenticated: bool,
        mut read: impl FnMut(&str) -> Result<Option<String>>,
    ) -> Result<Self> {
        let operator = read(PRIVATE_GRPC_AUTH_TOKEN_ENV)?;
        let miner = read(PRIVATE_GRPC_MINER_TOKEN_ENV)?;
        let policy = read(PRIVATE_GRPC_ALLOW_UNAUTHENTICATED_ENV)?;
        Self::new(
            addr,
            operator,
            miner,
            allow_unauthenticated || allow_unauthenticated_flag_enabled(policy.as_deref()),
        )
    }

    pub fn validate(&self) -> Result<()> {
        validate_private_server_config(
            self.addr,
            self.operator.as_deref(),
            self.miner.as_deref(),
            self.allow_unauthenticated,
        )
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrivateRole {
    Operator,
    Miner,
}

fn token_matches(expected: &[u8], supplied: &[u8]) -> bool {
    if expected.len() != supplied.len() {
        return false;
    }
    expected
        .iter()
        .zip(supplied)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

fn operator_token_valid(operator: &str) -> bool {
    operator.len() >= PRIVATE_GRPC_MIN_TOKEN_LEN
}

fn miner_role_tokens_valid(operator: Option<&str>, miner: &str) -> bool {
    miner.len() >= PRIVATE_GRPC_MIN_TOKEN_LEN
        && operator.is_some_and(|operator| operator_token_valid(operator) && operator != miner)
}

/// Parse the `NOCKCHAIN_ALLOW_UNAUTHENTICATED_PRIVATE_GRPC` opt-out. Only an
/// explicit affirmative value enables it; an empty or unrelated value does not.
pub fn allow_unauthenticated_flag_enabled(value: Option<&str>) -> bool {
    matches!(
        value.map(str::trim),
        Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("YES")
    )
}

fn validate_private_server_config(
    addr: SocketAddr,
    operator: Option<&str>,
    miner: Option<&str>,
    allow_unauthenticated: bool,
) -> Result<()> {
    if let Some(operator) = operator {
        if !operator_token_valid(operator) {
            return Err(NockAppGrpcError::InvalidRequest(format!(
                "{PRIVATE_GRPC_AUTH_TOKEN_ENV} must be at least {PRIVATE_GRPC_MIN_TOKEN_LEN} characters"
            )));
        }
    }
    if let Some(miner) = miner {
        if !miner_role_tokens_valid(operator, miner) {
            return Err(NockAppGrpcError::InvalidRequest(format!(
                "{PRIVATE_GRPC_MINER_TOKEN_ENV} requires distinct operator and miner tokens of at least {PRIVATE_GRPC_MIN_TOKEN_LEN} characters each"
            )));
        }
    }
    if operator.is_none() && !allow_unauthenticated {
        return Err(NockAppGrpcError::InvalidRequest(format!(
            "private gRPC requires {PRIVATE_GRPC_AUTH_TOKEN_ENV}; set \
             --allow-unauthenticated-private-grpc or {PRIVATE_GRPC_ALLOW_UNAUTHENTICATED_ENV}=1 \
             to serve a loopback-only node without bearer authentication"
        )));
    }
    if !addr.ip().is_loopback() {
        return Err(NockAppGrpcError::InvalidRequest(format!(
            "private gRPC must bind loopback, not {addr}; use an encrypted tunnel for remote clients"
        )));
    }
    Ok(())
}

fn role_for_token(
    operator: Option<&str>,
    miner: Option<&str>,
    allow_unauthenticated: bool,
    supplied: &[u8],
) -> Option<PrivateRole> {
    if operator.is_none() && miner.is_none() {
        return allow_unauthenticated.then_some(PrivateRole::Operator);
    }
    if operator.is_some_and(|expected| token_matches(expected.as_bytes(), supplied)) {
        return Some(PrivateRole::Operator);
    }
    if miner.is_some_and(|expected| token_matches(expected.as_bytes(), supplied)) {
        return Some(PrivateRole::Miner);
    }
    None
}

fn miner_wire_allowed(pid: i32, wire: Option<&Wire>) -> bool {
    let Some(wire) = wire else { return false };
    pid == 1
        && wire.version == 1
        && matches!(wire.source.as_str(), "ai-pow-miner" | "zk-pow-miner")
        && wire.tags.len() == 1
        && matches!(wire.tags[0].value.as_ref(), Some(wire_tag::Value::Text(tag)) if tag == "mined")
}

fn miner_payload_allowed(slab: &NounSlab, source: &str) -> bool {
    let root = unsafe { *slab.root() };
    let space = slab.noun_space();
    let Ok(command) = root.in_space(&space).as_cell() else {
        return false;
    };
    let Ok(pow) = command.tail().as_cell() else {
        return false;
    };
    let Ok(kind) = pow.tail().as_cell() else {
        return false;
    };
    command.head().eq_bytes("command")
        && pow.head().eq_bytes("pow")
        && match source {
            "ai-pow-miner" => kind.head().eq_bytes("ai-pow"),
            "zk-pow-miner" => kind.head().eq_bytes("dumb-zkpow"),
            _ => false,
        }
}

pub struct PrivateNockAppGrpcServer {
    handle: NockAppHandle,
    auth_token: Option<String>,
    miner_token: Option<String>,
    allow_unauthenticated: bool,
}

impl PrivateNockAppGrpcServer {
    pub fn new(handle: NockAppHandle) -> Self {
        Self {
            handle,
            auth_token: None,
            miner_token: None,
            allow_unauthenticated: false,
        }
    }

    /// Use the immutable configuration already checked by node startup.
    pub(crate) fn with_config(mut self, config: PrivateGrpcServerConfig) -> Self {
        self.auth_token = config.operator;
        self.miner_token = config.miner;
        self.allow_unauthenticated = config.allow_unauthenticated;
        self
    }

    /// Opt out of bearer authentication. `serve` refuses to start without an
    /// operator token unless this is set; tokens, when configured, still apply.
    pub fn with_allow_unauthenticated(mut self, allow_unauthenticated: bool) -> Self {
        self.allow_unauthenticated = allow_unauthenticated;
        self
    }

    pub fn with_auth_token(mut self, auth_token: Option<String>) -> Self {
        self.auth_token = auth_token.filter(|token| !token.is_empty());
        self
    }

    pub fn with_miner_token(mut self, miner_token: Option<String>) -> Self {
        self.miner_token = miner_token.filter(|token| !token.is_empty());
        self
    }

    fn authorize<T>(&self, request: &Request<T>) -> std::result::Result<PrivateRole, Status> {
        let supplied = request
            .metadata()
            .get(PRIVATE_GRPC_AUTH_HEADER)
            .map(|value| value.as_bytes())
            .unwrap_or_default();
        role_for_token(
            self.auth_token.as_deref(),
            self.miner_token.as_deref(),
            self.allow_unauthenticated,
            supplied,
        )
        .ok_or_else(|| Status::unauthenticated("private gRPC token required"))
    }

    pub async fn serve(self, addr: SocketAddr) -> Result<()> {
        validate_private_server_config(
            addr,
            self.auth_token.as_deref(),
            self.miner_token.as_deref(),
            self.allow_unauthenticated,
        )?;
        if self.auth_token.is_none() {
            warn!(
                "private gRPC server on {addr} runs without bearer authentication; \
                 every loopback client holds operator authority"
            );
        }
        info!("Starting private gRPC server on {}", addr);

        let (health_reporter, health_service) = health_reporter();
        health_reporter
            .set_serving::<PrivateNockAppServer<PrivateNockAppGrpcServer>>()
            .await;
        let reflection_service_v1 = ReflectionBuilder::configure()
            .register_encoded_file_descriptor_set(nockapp_grpc_proto::pb::FILE_DESCRIPTOR_SET)
            .register_encoded_file_descriptor_set(tonic_health::pb::FILE_DESCRIPTOR_SET)
            .build_v1()
            .map_err(|e| {
                NockAppGrpcError::Internal(format!("Failed to build v1 reflection service: {}", e))
            })?;
        // Explicit message-size limits on the unauthenticated command
        // channel: receive matches the kernel's 4 MiB artifact-jam cap (the
        // tonic 4 MiB default, pinned so a tonic upgrade cannot silently
        // widen it); send is bounded at 16 MiB for WatchEffects payloads.
        let service = PrivateNockAppServer::new(self)
            .max_decoding_message_size(4 * 1024 * 1024)
            .max_encoding_message_size(16 * 1024 * 1024);

        Server::builder()
            .add_service(health_service)
            .add_service(reflection_service_v1)
            .add_service(service)
            .serve(addr)
            .await
            .map_err(NockAppGrpcError::Transport)?;

        Ok(())
    }

    /// Build error response with proper error status
    fn build_error_response<T>(&self, error: NockAppGrpcError) -> T
    where
        T: From<ErrorStatus>,
    {
        let error_status = ErrorStatus {
            code: match &error {
                NockAppGrpcError::PeekFailed => ErrorCode::PeekFailed as i32,
                NockAppGrpcError::PokeFailed => ErrorCode::PokeFailed as i32,
                NockAppGrpcError::Timeout => ErrorCode::Timeout as i32,
                NockAppGrpcError::InvalidRequest(_) => ErrorCode::InvalidRequest as i32,
                _ => ErrorCode::InternalError as i32,
            },
            message: error.to_string(),
            details: None,
        };
        T::from(error_status)
    }
}

#[tonic::async_trait]
impl PrivateNockApp for PrivateNockAppGrpcServer {
    async fn peek(
        &self,
        request: Request<PeekRequest>,
    ) -> std::result::Result<Response<PeekResponse>, Status> {
        if self.authorize(&request)? != PrivateRole::Operator {
            return Err(Status::permission_denied("miner token cannot Peek"));
        }
        let req = request.into_inner();
        debug!("CorePeek request: pid={}, path={:?}", req.pid, req.path);
        let mut slab = NounSlab::new();
        let _path = match slab.cue_into(bytes::Bytes::from(req.path)) {
            Ok(noun) => noun,
            Err(e) => {
                warn!("Failed to decode JAM payload: {:?}", e);
                let response = PeekResponse {
                    result: Some(peek_response::Result::Error(self.build_error_response(
                        NockAppGrpcError::Serialization(format!(
                            "JAM decoding for path failed: {:?}",
                            e
                        )),
                    ))),
                };
                return Ok(Response::new(response));
            }
        };

        match self.handle.peek(slab).await {
            Ok(Some(result_slab)) => {
                // Convert result to JAM-encoded bytes
                let jam_bytes = result_slab.jam();

                let response = PeekResponse {
                    result: Some(peek_response::Result::Data(jam_bytes.to_vec())),
                };
                Ok(Response::new(response))
            }
            Ok(None) => {
                debug!("Peek returned no result");
                let response = PeekResponse {
                    result: Some(peek_response::Result::Error(
                        self.build_error_response(NockAppGrpcError::PeekFailed),
                    )),
                };
                Ok(Response::new(response))
            }
            Err(e) => {
                error!("Peek operation failed: {}", e);
                let response = PeekResponse {
                    result: Some(peek_response::Result::Error(
                        self.build_error_response(NockAppGrpcError::NockApp(e)),
                    )),
                };
                Ok(Response::new(response))
            }
        }
    }

    type WatchEffectsStream =
        Pin<Box<dyn Stream<Item = std::result::Result<EffectMessage, Status>> + Send + 'static>>;

    async fn watch_effects(
        &self,
        request: Request<WatchEffectsRequest>,
    ) -> std::result::Result<Response<Self::WatchEffectsStream>, Status> {
        let role = self.authorize(&request)?;
        let req = request.into_inner();
        if role == PrivateRole::Miner
            && (req.pid != 1
                || req.head_filter.len() != 1
                || !matches!(req.head_filter[0].as_slice(), b"mine-ai" | b"mine-zk"))
        {
            return Err(Status::permission_denied(
                "miner token can only watch one mining-candidate effect",
            ));
        }
        debug!(
            "WatchEffects request: pid={} filters={}",
            req.pid,
            req.head_filter.len()
        );

        // One fresh broadcast subscriber per client. The bus drops effects
        // for slow consumers; a lagging subscriber gets a terminal stream error
        // so stateful miners reconnect and invalidate stale work.
        let mut receiver = self.handle.effect_sender.subscribe();
        let head_filter = req.head_filter;
        let (tx, rx) = tokio::sync::mpsc::channel::<std::result::Result<EffectMessage, Status>>(64);

        tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(slab) => {
                        let effect_noun = unsafe { *slab.root() };
                        // Filter on the head atom of the effect cell. Empty
                        // filter forwards everything. Use the slab noun space
                        // when reading the effect head.
                        let head_matches = if head_filter.is_empty() {
                            true
                        } else {
                            let space = slab.noun_space();
                            match effect_noun.in_space(&space).as_cell() {
                                Ok(cell) => {
                                    let head = cell.head();
                                    head_filter.iter().any(|f| head.eq_bytes(f.as_slice()))
                                }
                                Err(_) => false,
                            }
                        };
                        if !head_matches {
                            continue;
                        }
                        let jam_bytes = slab.jam();
                        let msg = EffectMessage {
                            effect: jam_bytes.to_vec(),
                        };
                        if tx.send(Ok(msg)).await.is_err() {
                            // Client disconnected.
                            break;
                        }
                    }
                    Err(RecvError::Lagged(n)) => {
                        warn!("WatchEffects client lagged by {n} effects; ending stream");
                        let _ = tx
                            .send(Err(Status::data_loss(format!(
                                "WatchEffects client lagged by {n} effects; reconnect required"
                            ))))
                            .await;
                        break;
                    }
                    Err(RecvError::Closed) => {
                        debug!("WatchEffects: effect broadcast closed; ending stream");
                        break;
                    }
                }
            }
        });

        let stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(stream) as Self::WatchEffectsStream))
    }

    async fn poke(
        &self,
        request: Request<PokeRequest>,
    ) -> std::result::Result<Response<PokeResponse>, Status> {
        let role = self.authorize(&request)?;
        let req = request.into_inner();
        if role == PrivateRole::Miner && !miner_wire_allowed(req.pid, req.wire.as_ref()) {
            return Err(Status::permission_denied(
                "miner token can only submit a mined proof",
            ));
        }
        debug!("Poke request: pid={}", req.pid);

        let wire = match req.wire {
            Some(wire) => match grpc_wire_to_nockapp(&wire) {
                Ok(w) => w,
                Err(e) => {
                    warn!("Invalid wire in Poke: {}", e);
                    let response = PokeResponse {
                        result: Some(poke_response::Result::Error(self.build_error_response(e))),
                    };
                    return Ok(Response::new(response));
                }
            },
            None => {
                warn!("Missing wire in Poke request");
                let response = PokeResponse {
                    result: Some(poke_response::Result::Error(self.build_error_response(
                        NockAppGrpcError::InvalidRequest("Wire is required".to_string()),
                    ))),
                };
                return Ok(Response::new(response));
            }
        };

        // Decode JAM payload
        let mut payload_slab = NounSlab::new();
        let _payload_noun = match payload_slab.cue_into(bytes::Bytes::from(req.payload)) {
            Ok(noun) => noun,
            Err(e) => {
                warn!("Failed to decode JAM payload: {:?}", e);
                let response = PokeResponse {
                    result: Some(poke_response::Result::Error(self.build_error_response(
                        NockAppGrpcError::Serialization(format!("JAM decoding failed: {:?}", e)),
                    ))),
                };
                return Ok(Response::new(response));
            }
        };

        if role == PrivateRole::Miner && !miner_payload_allowed(&payload_slab, &wire.source) {
            return Err(Status::permission_denied("invalid mined-proof payload"));
        }

        match self.handle.poke(wire, payload_slab).await {
            Ok(PokeResult::Ack) => {
                debug!("Poke operation acknowledged");
                let response = PokeResponse {
                    result: Some(poke_response::Result::Acknowledged(true)),
                };
                Ok(Response::new(response))
            }
            Ok(PokeResult::Nack) => {
                debug!("Poke operation nacked");
                let response = PokeResponse {
                    result: Some(poke_response::Result::Error(
                        self.build_error_response(NockAppGrpcError::PokeFailed),
                    )),
                };
                Ok(Response::new(response))
            }
            Err(e) => {
                error!("Poke operation failed: {}", e);
                let response = PokeResponse {
                    result: Some(poke_response::Result::Error(
                        self.build_error_response(NockAppGrpcError::NockApp(e)),
                    )),
                };
                Ok(Response::new(response))
            }
        }
    }
}

#[cfg(test)]
mod auth_tests {
    use nockapp::noun::slab::NounSlab;
    use nockvm::noun::{D, T};
    use nockvm_macros::tas;

    use super::{
        allow_unauthenticated_flag_enabled, miner_payload_allowed, miner_role_tokens_valid,
        miner_wire_allowed, role_for_token, token_matches, validate_private_server_config,
        PrivateGrpcServerConfig, PrivateRole,
    };
    use crate::pb::common::v1::{wire_tag, Wire, WireTag};

    const LOOPBACK: &str = "127.0.0.1:5555";

    #[test]
    fn effective_config_rejects_invalid_roles_before_startup() {
        let operator = "0123456789abcdef0123456789abcdef";
        let miner = "abcdef0123456789abcdef0123456789";
        let addr = LOOPBACK.parse().unwrap();
        for opt_out in [false, true] {
            for (operator, miner) in [
                (Some("short"), None),
                (Some(operator), Some("short")),
                (Some(operator), Some(operator)),
                (None, Some(miner)),
            ] {
                assert!(PrivateGrpcServerConfig::new(
                    addr,
                    operator.map(str::to_owned),
                    miner.map(str::to_owned),
                    opt_out,
                )
                .is_err());
            }
        }
        let config = PrivateGrpcServerConfig::new(
            addr,
            Some(operator.to_owned()),
            Some(miner.to_owned()),
            false,
        )
        .unwrap();
        assert_eq!(
            role_for_token(
                config.operator.as_deref(),
                config.miner.as_deref(),
                config.allow_unauthenticated,
                miner.as_bytes(),
            ),
            Some(PrivateRole::Miner),
        );
    }

    #[test]
    fn effective_environment_config_is_a_fixed_policy_snapshot() {
        use super::{
            PRIVATE_GRPC_ALLOW_UNAUTHENTICATED_ENV, PRIVATE_GRPC_AUTH_TOKEN_ENV,
            PRIVATE_GRPC_MINER_TOKEN_ENV,
        };

        let addr = LOOPBACK.parse().unwrap();
        assert!(PrivateGrpcServerConfig::from_env_reader(addr, false, |_| Ok(None)).is_err());
        assert!(PrivateGrpcServerConfig::from_env_reader(addr, true, |_| Ok(None)).is_ok());
        for policy in ["", "0", "false", "no", "maybe"] {
            assert!(
                PrivateGrpcServerConfig::from_env_reader(addr, false, |name| {
                    Ok((name == PRIVATE_GRPC_ALLOW_UNAUTHENTICATED_ENV).then(|| policy.to_owned()))
                })
                .is_err()
            );
        }
        for policy in ["1", "true", "TRUE", "yes", "YES", " 1 "] {
            let config = PrivateGrpcServerConfig::from_env_reader(addr, false, |name| {
                Ok((name == PRIVATE_GRPC_ALLOW_UNAUTHENTICATED_ENV).then(|| policy.to_owned()))
            })
            .unwrap();
            assert!(config.allow_unauthenticated);
            assert!(config.validate().is_ok());
        }
        let mut reads = Vec::new();
        let config = PrivateGrpcServerConfig::from_env_reader(addr, true, |name| {
            reads.push(name.to_owned());
            Ok(match name {
                PRIVATE_GRPC_AUTH_TOKEN_ENV => Some("0123456789abcdef0123456789abcdef".to_owned()),
                PRIVATE_GRPC_MINER_TOKEN_ENV => Some("abcdef0123456789abcdef0123456789".to_owned()),
                _ => None,
            })
        })
        .unwrap();
        assert_eq!(reads.len(), 3);
        assert_eq!(
            role_for_token(
                config.operator.as_deref(),
                config.miner.as_deref(),
                config.allow_unauthenticated,
                b"",
            ),
            None,
            "an opt-out cannot bypass credentials in the startup snapshot",
        );
        assert!(config.validate().is_ok());
        assert_eq!(reads.len(), 3, "validation must not reread the environment");
        assert!(PrivateGrpcServerConfig::from_env_reader(
            "0.0.0.0:5555".parse().unwrap(),
            true,
            |_| Ok(None),
        )
        .is_err());
    }

    #[test]
    fn private_token_comparison_rejects_missing_and_wrong_values() {
        let token = b"0123456789abcdef0123456789abcdef";
        assert!(token_matches(token, token));
        assert!(!token_matches(token, b""));
        assert!(!token_matches(token, b"0123456789abcdef0123456789abcdee"));
    }

    #[test]
    fn miner_token_has_no_operator_authority() {
        let operator = "0123456789abcdef0123456789abcdef";
        let miner = "abcdef0123456789abcdef0123456789";
        assert_eq!(
            role_for_token(Some(operator), Some(miner), false, operator.as_bytes()),
            Some(PrivateRole::Operator)
        );
        assert_eq!(
            role_for_token(Some(operator), Some(miner), false, miner.as_bytes()),
            Some(PrivateRole::Miner)
        );
        assert_eq!(
            role_for_token(Some(operator), Some(miner), false, b""),
            None
        );
        // The opt-out never weakens a configured token.
        assert_eq!(role_for_token(Some(operator), Some(miner), true, b""), None);
        assert_eq!(role_for_token(Some(operator), None, true, b""), None);
        assert!(miner_role_tokens_valid(Some(operator), miner));
        assert!(!miner_role_tokens_valid(None, miner));
        assert!(!miner_role_tokens_valid(Some("short"), miner));
        assert!(!miner_role_tokens_valid(Some(operator), operator));
        assert!(!miner_role_tokens_valid(Some(operator), "short"));
    }

    #[test]
    fn unauthenticated_mode_requires_explicit_opt_out() {
        // No tokens at all: only the explicit opt-out grants operator access.
        assert_eq!(role_for_token(None, None, false, b""), None);
        assert_eq!(role_for_token(None, None, false, b"anything"), None);
        assert_eq!(
            role_for_token(None, None, true, b""),
            Some(PrivateRole::Operator)
        );

        let addr = LOOPBACK.parse().unwrap();
        let err = validate_private_server_config(addr, None, None, false)
            .expect_err("no operator token and no opt-out must refuse to start");
        let message = err.to_string();
        assert!(
            message.contains("NOCKCHAIN_PRIVATE_GRPC_AUTH_TOKEN"),
            "{message}"
        );
        assert!(
            message.contains("--allow-unauthenticated-private-grpc"),
            "{message}"
        );
        assert!(validate_private_server_config(addr, None, None, true).is_ok());
        // The opt-out does not relax the loopback requirement.
        assert!(
            validate_private_server_config("0.0.0.0:5555".parse().unwrap(), None, None, true)
                .is_err()
        );
    }

    #[test]
    fn operator_token_minimum_length_applies_without_miner_token() {
        let addr = LOOPBACK.parse().unwrap();
        let strong = "0123456789abcdef0123456789abcdef";
        assert!(validate_private_server_config(addr, Some(strong), None, false).is_ok());
        for short in ["short", "0123456789abcdef0123456789abcde"] {
            let err = validate_private_server_config(addr, Some(short), None, false)
                .expect_err("short operator token must be rejected");
            assert!(err.to_string().contains("at least 32 characters"));
            // The unauthenticated opt-out is not a way around a weak token.
            assert!(validate_private_server_config(addr, Some(short), None, true).is_err());
        }
    }

    #[test]
    fn allow_unauthenticated_env_parses_only_affirmative_values() {
        for value in ["1", "true", "TRUE", "yes", " 1 "] {
            assert!(allow_unauthenticated_flag_enabled(Some(value)), "{value:?}");
        }
        for value in ["", "0", "false", "no", "maybe"] {
            assert!(
                !allow_unauthenticated_flag_enabled(Some(value)),
                "{value:?}"
            );
        }
        assert!(!allow_unauthenticated_flag_enabled(None));
    }

    #[test]
    fn private_server_requires_loopback_even_with_strong_tokens() {
        let operator = "0123456789abcdef0123456789abcdef";
        let miner = "abcdef0123456789abcdef0123456789";
        for addr in ["0.0.0.0:5555", "192.0.2.1:5555", "[::]:5555"] {
            assert!(validate_private_server_config(
                addr.parse().unwrap(),
                Some(operator),
                Some(miner),
                false,
            )
            .is_err());
        }
        for addr in ["127.0.0.1:5555", "[::1]:5555"] {
            assert!(validate_private_server_config(
                addr.parse().unwrap(),
                Some(operator),
                Some(miner),
                false,
            )
            .is_ok());
        }
    }

    #[test]
    fn miner_token_only_submits_mined_pow_payload() {
        let wire = Wire {
            source: "ai-pow-miner".to_owned(),
            version: 1,
            tags: vec![WireTag {
                value: Some(wire_tag::Value::Text("mined".to_owned())),
            }],
        };
        assert!(miner_wire_allowed(1, Some(&wire)));
        assert!(!miner_wire_allowed(2, Some(&wire)));
        let mut privileged = wire.clone();
        privileged.tags[0].value = Some(wire_tag::Value::Text("setpubkey".to_owned()));
        assert!(!miner_wire_allowed(1, Some(&privileged)));

        let mut slab = NounSlab::new();
        let mined = T(
            &mut slab,
            &[D(tas!(b"command")), D(tas!(b"pow")), D(tas!(b"ai-pow")), D(0)],
        );
        slab.set_root(mined);
        assert!(miner_payload_allowed(&slab, "ai-pow-miner"));
        assert!(!miner_payload_allowed(&slab, "zk-pow-miner"));

        let mut slab = NounSlab::new();
        let operator_command = T(&mut slab, &[D(tas!(b"command")), D(tas!(b"admin")), D(0)]);
        slab.set_root(operator_command);
        assert!(!miner_payload_allowed(&slab, "ai-pow-miner"));
    }
}
