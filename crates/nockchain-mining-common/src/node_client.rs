//! High-level gRPC client for the node's private `NockAppService`.
//!
//! Wraps [`nockapp_grpc::private_nockapp::client::PrivateNockAppGrpcClient`]
//! with mining-specific helpers: `set_mining_key`, `enable_mining`,
//! `watch_candidates`, and `submit_mined_block`. These are the four
//! operations every external miner binary (ZK-PoW + AI-PoW) needs.
//!
//! Each typed helper takes a [`WireRepr`] argument so the caller can
//! supply its own crate-specific wire vocabulary (e.g.
//! `zk_pow_miner::ZkPowMinerWire` with `SOURCE = "zk-pow-miner"`, or
//! `ai_pow_miner::wire::AiPowMinerWire` with `SOURCE = "ai-pow-miner"`).
//! The shared substrate stays wire-agnostic; the kernel-side dispatcher
//! in `hoon/apps/dumbnet/inner.hoon` routes by source.
//!
//! The candidate watcher decodes `%mine` effects from the server's
//! `WatchEffects` stream into [`MiningCandidate`]. The pokes mirror
//! the noun shapes the kernel-side Hoon expects — historically built
//! in-process in `crates/nockchain/src/mining.rs` (now removed).

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use futures::{Stream, StreamExt};
use nockapp::nockapp::wire::WireRepr;
use nockapp::noun::slab::NounSlab;
use nockapp::noun::AtomExt;
use nockapp_grpc::pb::common::v1::{ErrorCode, Wire as GrpcWire};
use nockapp_grpc::private_nockapp::client::{PokeResponseResult, PrivateNockAppGrpcClient};
use nockapp_grpc::wire_conversion::nockapp_wire_to_grpc;
use nockvm::noun::{Atom, D, NO, T, YES};
use nockvm_macros::tas;
use thiserror::Error;
use tracing::debug;

use crate::candidate::{CandidateDecodeError, MiningCandidate};
use crate::key_config::{MiningKeyConfig, MiningPkhConfig};

#[derive(Debug, Error)]
pub enum NodeClientError {
    #[error("gRPC transport error: {0}")]
    Grpc(#[from] nockapp_grpc::error::NockAppGrpcError),
    #[error("kernel rejected poke: code={code} message={message}")]
    PokeRejected { code: i32, message: String },
    #[error("poke deadline expired before send")]
    PokeDeadlineBeforeSend,
    #[error("poke acknowledgement not observed before deadline")]
    PokeAckTimedOut,
    #[error("poke acknowledgement not observed before shutdown")]
    PokeAckCancelled,
    #[error("decoding candidate effect: {0}")]
    Decode(#[from] CandidateDecodeError),
}

/// Default `pid` tag sent on Poke / WatchEffects. The node uses it only
/// for tracking / logging; any nonzero int is fine.
const DEFAULT_PID: i32 = 1;

/// Stream of decoded mining candidates yielded by [`NodeClient::watch_candidates`].
///
/// Boxed + pinned so callers can hold it as a single owned type without
/// naming the underlying combinator stack.
pub type CandidateStream =
    Pin<Box<dyn Stream<Item = Result<MiningCandidate, NodeClientError>> + Send>>;

#[derive(Debug)]
pub struct PreparedPoke {
    pid: i32,
    wire: GrpcWire,
    payload: Vec<u8>,
}

impl PreparedPoke {
    pub fn wire(&self) -> &GrpcWire {
        &self.wire
    }

    pub fn payload_len(&self) -> usize {
        self.payload.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PokeTransportStatus {
    Ack,
    Nack,
    FailureBeforeSend,
    AckUnknown,
}

#[derive(Debug)]
pub enum PokeTransportOutcome {
    Ack {
        elapsed: Duration,
    },
    Nack {
        elapsed: Duration,
        code: i32,
        message: String,
    },
    FailureBeforeSend {
        elapsed: Duration,
        error: NodeClientError,
    },
    AckUnknown {
        elapsed: Duration,
        error: NodeClientError,
    },
}

impl PokeTransportOutcome {
    pub fn status(&self) -> PokeTransportStatus {
        match self {
            Self::Ack { .. } => PokeTransportStatus::Ack,
            Self::Nack { .. } => PokeTransportStatus::Nack,
            Self::FailureBeforeSend { .. } => PokeTransportStatus::FailureBeforeSend,
            Self::AckUnknown { .. } => PokeTransportStatus::AckUnknown,
        }
    }

    pub fn send_started(&self) -> bool {
        !matches!(self, Self::FailureBeforeSend { .. })
    }

    pub fn elapsed(&self) -> Duration {
        match self {
            Self::Ack { elapsed }
            | Self::Nack { elapsed, .. }
            | Self::FailureBeforeSend { elapsed, .. }
            | Self::AckUnknown { elapsed, .. } => *elapsed,
        }
    }

    pub fn into_result(self) -> Result<(), NodeClientError> {
        match self {
            Self::Ack { .. } => Ok(()),
            Self::Nack { code, message, .. } => {
                Err(NodeClientError::PokeRejected { code, message })
            }
            Self::FailureBeforeSend { error, .. } | Self::AckUnknown { error, .. } => Err(error),
        }
    }
}

pub struct NodeClient {
    client: PrivateNockAppGrpcClient,
}

impl NodeClient {
    /// Connect to a node's private gRPC service. `address` is a URL
    /// like `http://127.0.0.1:5555`.
    pub async fn connect<T: AsRef<str>>(address: T) -> Result<Self, NodeClientError> {
        let client = PrivateNockAppGrpcClient::connect(address).await?;
        Ok(Self { client })
    }

    /// Push the mining-reward configuration to the node's kernel.
    /// Same noun shape as the old in-process
    /// `set_mining_key_advanced(...)`: `[%command %set-mining-key-advanced
    /// configs-list pkh-configs-list]`. The caller supplies the wire
    /// (typically `XPowMinerWire::SetPubKey.to_wire()` for their crate).
    pub async fn set_mining_key(
        &mut self,
        wire: WireRepr,
        configs: Vec<MiningKeyConfig>,
        pkh_configs: Vec<MiningPkhConfig>,
    ) -> Result<(), NodeClientError> {
        let mut slab = NounSlab::new();
        let set_mining_key_adv =
            <Atom as AtomExt>::from_value(&mut slab, "set-mining-key-advanced")
                .expect("set-mining-key-advanced atom");

        // v0 (pubkey) configs list — cons-cell list, terminated by 0.
        let mut configs_list = D(0);
        for config in configs {
            let mut keys_noun = D(0);
            for key in config.keys {
                let key_atom = <Atom as AtomExt>::from_value(&mut slab, key).expect("key atom");
                keys_noun = T(&mut slab, &[key_atom.as_noun(), keys_noun]);
            }
            let config_tuple = T(&mut slab, &[D(config.share), D(config.m), keys_noun]);
            configs_list = T(&mut slab, &[config_tuple, configs_list]);
        }

        // v1 (pubkey-hash) configs list — cons-cell list, terminated by 0.
        let mut pkh_configs_list = D(0);
        for config in pkh_configs {
            let pkh_noun = <Atom as AtomExt>::from_value(&mut slab, config.pkh)
                .expect("pkh atom")
                .as_noun();
            let config_tuple = T(&mut slab, &[D(config.share), pkh_noun]);
            pkh_configs_list = T(&mut slab, &[config_tuple, pkh_configs_list]);
        }

        let poke = T(
            &mut slab,
            &[
                D(tas!(b"command")),
                set_mining_key_adv.as_noun(),
                configs_list,
                pkh_configs_list,
            ],
        );
        slab.set_root(poke);

        self.poke_wire(wire, slab).await
    }

    /// Toggle mining on / off. Same noun shape as the old in-process
    /// `enable_mining(...)`: `[%command %enable-mining flag]`. The
    /// caller supplies the wire (typically
    /// `XPowMinerWire::Enable.to_wire()` for their crate).
    pub async fn enable_mining(
        &mut self,
        wire: WireRepr,
        enable: bool,
    ) -> Result<(), NodeClientError> {
        let mut slab = NounSlab::new();
        let enable_mining_atom =
            <Atom as AtomExt>::from_value(&mut slab, "enable-mining").expect("enable-mining atom");
        let poke = T(
            &mut slab,
            &[D(tas!(b"command")), enable_mining_atom.as_noun(), if enable { YES } else { NO }],
        );
        slab.set_root(poke);
        self.poke_wire(wire, slab).await
    }

    /// Submit a mined block. The caller produces the same poke payload
    /// the in-process driver historically poked the main kernel with
    /// (the `poke` tail of a `%mine-result` success effect from the
    /// miner kernel) — this just wraps the gRPC round-trip. The caller
    /// supplies the wire (typically `XPowMinerWire::Mined.to_wire()`
    /// for their crate).
    pub async fn submit_mined_block(
        &mut self,
        wire: WireRepr,
        slab: NounSlab,
    ) -> Result<(), NodeClientError> {
        self.poke_wire(wire, slab).await
    }

    /// Subscribe to the node's mining-candidate effects, decoded into
    /// [`MiningCandidate`]s. The `head_filter` is passed through to the
    /// `WatchEffects` RPC so non-matching traffic is dropped server-side.
    ///
    /// Typical callers:
    /// - ZK-PoW miner: `vec![b"mine-zk".to_vec()]`
    /// - AI-PoW miner: `vec![b"mine-ai".to_vec()]`
    ///
    /// The returned stream is owned; callers can drop it to end the
    /// subscription. Server-side disconnect surfaces as a `None` from
    /// `Stream::next`.
    pub async fn watch_candidates(
        &mut self,
        head_filter: Vec<Vec<u8>>,
    ) -> Result<CandidateStream, NodeClientError> {
        let raw = self.client.watch_effects(DEFAULT_PID, head_filter).await?;
        let mapped = raw.filter_map(|item| async move {
            match item {
                Err(e) => Some(Err(NodeClientError::Grpc(e))),
                Ok(slab) => match MiningCandidate::from_effect_slab(slab) {
                    Ok(Some(c)) => Some(Ok(c)),
                    Ok(None) => {
                        // server-side head_filter should mean this never fires,
                        // but the check is defensive — keep the stream alive.
                        debug!("watch_candidates: dropped non-mine-zk/mine-ai effect");
                        None
                    }
                    Err(e) => Some(Err(NodeClientError::Decode(e))),
                },
            }
        });
        Ok(Box::pin(mapped) as CandidateStream)
    }

    /// Materialize an immutable gRPC poke payload. Timing callers use this
    /// boundary to separate noun construction/JAM from transport.
    pub fn prepare_poke_wire(wire: WireRepr, slab: NounSlab) -> PreparedPoke {
        PreparedPoke {
            pid: DEFAULT_PID,
            wire: nockapp_wire_to_grpc(&wire),
            payload: slab.jam().to_vec(),
        }
    }

    /// Send a prepared poke and retain the delivery classification.
    pub async fn send_prepared_poke(&mut self, prepared: PreparedPoke) -> PokeTransportOutcome {
        let started_at = Instant::now();
        let response = self
            .client
            .poke_response(prepared.pid, prepared.wire, prepared.payload)
            .await;
        Self::classify_poke_response(started_at, response)
    }

    /// Send a prepared poke with a client-side acknowledgement deadline.
    pub async fn send_prepared_poke_with_timeout(
        &mut self,
        prepared: PreparedPoke,
        timeout: Duration,
    ) -> PokeTransportOutcome {
        let started_at = Instant::now();
        if timeout.is_zero() {
            return PokeTransportOutcome::FailureBeforeSend {
                elapsed: started_at.elapsed(),
                error: NodeClientError::PokeDeadlineBeforeSend,
            };
        }

        let response = tokio::time::timeout(
            timeout,
            self.client
                .poke_response(prepared.pid, prepared.wire, prepared.payload),
        )
        .await;
        match response {
            Ok(result) => Self::classify_poke_response(started_at, result),
            Err(_) => PokeTransportOutcome::AckUnknown {
                elapsed: started_at.elapsed(),
                error: NodeClientError::PokeAckTimedOut,
            },
        }
    }

    /// Send a prepared poke with acknowledgement deadline and shutdown cancellation.
    pub async fn send_prepared_poke_with_timeout_or_cancel<C>(
        &mut self,
        prepared: PreparedPoke,
        timeout: Duration,
        cancelled: C,
    ) -> PokeTransportOutcome
    where
        C: Future<Output = ()>,
    {
        let started_at = Instant::now();
        if timeout.is_zero() {
            return PokeTransportOutcome::FailureBeforeSend {
                elapsed: started_at.elapsed(),
                error: NodeClientError::PokeDeadlineBeforeSend,
            };
        }

        let response = self
            .client
            .poke_response(prepared.pid, prepared.wire, prepared.payload);
        let timeout = tokio::time::sleep(timeout);
        tokio::pin!(response);
        tokio::pin!(timeout);
        tokio::pin!(cancelled);

        tokio::select! {
            result = &mut response => Self::classify_poke_response(started_at, result),
            _ = &mut timeout => PokeTransportOutcome::AckUnknown {
                elapsed: started_at.elapsed(),
                error: NodeClientError::PokeAckTimedOut,
            },
            _ = &mut cancelled => PokeTransportOutcome::AckUnknown {
                elapsed: started_at.elapsed(),
                error: NodeClientError::PokeAckCancelled,
            },
        }
    }

    fn classify_poke_response(
        started_at: Instant,
        response: std::result::Result<PokeResponseResult, nockapp_grpc::error::NockAppGrpcError>,
    ) -> PokeTransportOutcome {
        let elapsed = started_at.elapsed();
        match response {
            Ok(PokeResponseResult::Acknowledged(true)) => PokeTransportOutcome::Ack { elapsed },
            Ok(PokeResponseResult::Acknowledged(false)) => PokeTransportOutcome::Nack {
                elapsed,
                code: ErrorCode::PokeFailed as i32,
                message: "kernel did not acknowledge poke".to_string(),
            },
            Ok(PokeResponseResult::Error(error)) => PokeTransportOutcome::Nack {
                elapsed,
                code: error.code,
                message: error.message,
            },
            Err(error) => PokeTransportOutcome::AckUnknown {
                elapsed,
                error: NodeClientError::Grpc(error),
            },
        }
    }

    /// Send a poke on an arbitrary [`WireRepr`]. This is the underlying
    /// gRPC operation; the typed helpers
    /// ([`set_mining_key`](Self::set_mining_key),
    /// [`enable_mining`](Self::enable_mining),
    /// [`submit_mined_block`](Self::submit_mined_block)) all delegate
    /// here.
    pub async fn poke_wire(
        &mut self,
        wire: WireRepr,
        slab: NounSlab,
    ) -> Result<(), NodeClientError> {
        let prepared = Self::prepare_poke_wire(wire, slab);
        self.send_prepared_poke(prepared).await.into_result()
    }
}

#[cfg(test)]
mod tests {
    use nockapp_grpc::error::NockAppGrpcError;
    use nockapp_grpc::pb::common::v1::ErrorStatus;

    use super::*;

    #[test]
    fn node_client_poke_prepare_materializes_wire_and_jam() {
        let mut slab = NounSlab::new();
        slab.set_root(D(42));

        let prepared = NodeClient::prepare_poke_wire(WireRepr::no_tags("test-miner", 7), slab);

        assert_eq!(prepared.wire().source, "test-miner");
        assert_eq!(prepared.wire().version, 7);
        assert!(prepared.payload_len() > 0);
    }

    #[test]
    fn node_client_poke_classifies_ack_nack_and_unknown() {
        let ack = NodeClient::classify_poke_response(
            Instant::now(),
            Ok(PokeResponseResult::Acknowledged(true)),
        );
        assert_eq!(ack.status(), PokeTransportStatus::Ack);
        assert!(ack.send_started());
        assert!(ack.into_result().is_ok());

        let bool_nack = NodeClient::classify_poke_response(
            Instant::now(),
            Ok(PokeResponseResult::Acknowledged(false)),
        );
        match bool_nack {
            PokeTransportOutcome::Nack { code, message, .. } => {
                assert_eq!(code, ErrorCode::PokeFailed as i32);
                assert!(message.contains("kernel did not acknowledge"));
            }
            other => panic!("expected bool nack, got {other:?}"),
        }

        let error_nack = NodeClient::classify_poke_response(
            Instant::now(),
            Ok(PokeResponseResult::Error(ErrorStatus {
                code: ErrorCode::PokeFailed as i32,
                message: "Poke operation failed".to_string(),
                details: None,
            })),
        );
        match error_nack {
            PokeTransportOutcome::Nack { code, message, .. } => {
                assert_eq!(code, ErrorCode::PokeFailed as i32);
                assert_eq!(message, "Poke operation failed");
            }
            other => panic!("expected error-status nack, got {other:?}"),
        }

        let unknown = NodeClient::classify_poke_response(
            Instant::now(),
            Err(NockAppGrpcError::Internal("connection lost".to_string())),
        );
        assert_eq!(unknown.status(), PokeTransportStatus::AckUnknown);
        assert!(unknown.send_started());
        match unknown.into_result() {
            Err(NodeClientError::Grpc(err)) => {
                assert!(err.to_string().contains("connection lost"));
            }
            other => panic!("expected ack-unknown grpc error, got {other:?}"),
        }
    }
}
