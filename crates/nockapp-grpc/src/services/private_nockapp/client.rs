use std::time::Duration;

use bytes::Bytes;
use futures::{Stream, StreamExt};
use nockapp::noun::slab::NounSlab;
use tonic::transport::{Channel, Endpoint};

use crate::error::{NockAppGrpcError, Result};
use crate::pb::common::v1::{ErrorStatus, Wire};
use crate::pb::private::v1::nock_app_service_client::NockAppServiceClient as PrivateNockAppClient;
use crate::pb::private::v1::*;

#[derive(Clone)]
pub struct PrivateNockAppGrpcClient {
    client: PrivateNockAppClient<Channel>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PokeResponseResult {
    Acknowledged(bool),
    Error(ErrorStatus),
}

const TCP_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
const HTTP2_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(20);
const HTTP2_KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(10);

impl PrivateNockAppGrpcClient {
    pub async fn connect<T: AsRef<str>>(address: T) -> Result<Self> {
        let client = PrivateNockAppClient::connect(address.as_ref().to_string()).await?;
        Ok(Self { client })
    }

    /// Connect with a bounded handshake and transport keepalives suitable for
    /// long-lived effect streams. The original [`Self::connect`] remains for
    /// callers that need tonic's default transport policy.
    pub async fn connect_with_timeout<T: AsRef<str>>(
        address: T,
        connect_timeout: Duration,
    ) -> Result<Self> {
        let endpoint = Endpoint::from_shared(address.as_ref().to_string())?
            .connect_timeout(connect_timeout)
            .tcp_keepalive(Some(TCP_KEEPALIVE_INTERVAL))
            .http2_keep_alive_interval(HTTP2_KEEPALIVE_INTERVAL)
            .keep_alive_timeout(HTTP2_KEEPALIVE_TIMEOUT)
            .keep_alive_while_idle(true);
        let channel = endpoint.connect().await?;
        Ok(Self {
            client: PrivateNockAppClient::new(channel),
        })
    }

    // Monitoring ping is handled in MonitoringService, not here.

    pub async fn peek(&mut self, pid: i32, path: Vec<u8>) -> Result<Vec<u8>> {
        let request = PeekRequest { pid, path };

        let response = self.client.peek(request).await?;
        let response = response.into_inner();

        match response.result {
            Some(peek_response::Result::Data(data)) => Ok(data),
            Some(peek_response::Result::Error(error)) => {
                Err(NockAppGrpcError::Internal(error.message))
            }
            None => Err(NockAppGrpcError::Internal("Empty response".to_string())),
        }
    }

    // pub async fn peek_vase(&mut self, pid: i32, path: Vec<String>) -> Result<Vec<u8>> {
    //     let request = PeekVaseRequest { pid, path };

    //     let response = self.client.peek_vase(request).await?;
    //     let response = response.into_inner();

    //     match response.result {
    //         Some(peek_vase_response::Result::Vase(vase)) => Ok(vase),
    //         Some(peek_vase_response::Result::Error(error)) => {
    //             Err(NockAppGrpcError::Internal(error.message))
    //         }
    //         None => Err(NockAppGrpcError::Internal("Empty response".to_string())),
    //     }
    // }

    /// Subscribe to the node's effect bus over the `WatchEffects` streaming
    /// RPC. `head_filter` is a list of byte strings (e.g., `vec![b"mine".to_vec()]`);
    /// the server forwards only effects whose head atom byte-equals one of
    /// these. Empty filter ⇒ all effects.
    ///
    /// Each yielded item is a `NounSlab` containing the decoded effect noun.
    pub async fn watch_effects(
        &mut self,
        pid: i32,
        head_filter: Vec<Vec<u8>>,
    ) -> Result<impl Stream<Item = Result<NounSlab>>> {
        let request = WatchEffectsRequest { pid, head_filter };
        let response = self.client.watch_effects(request).await?;
        let inner = response.into_inner();
        let stream = inner.map(|item| match item {
            Ok(msg) => {
                let mut slab = NounSlab::new();
                match slab.cue_into(Bytes::from(msg.effect)) {
                    Ok(_) => Ok(slab),
                    Err(e) => Err(NockAppGrpcError::Serialization(format!(
                        "JAM decoding of WatchEffects payload failed: {e:?}"
                    ))),
                }
            }
            Err(status) => Err(NockAppGrpcError::Internal(format!(
                "WatchEffects stream error: {status}"
            ))),
        });
        Ok(stream)
    }

    pub async fn poke_response(
        &mut self,
        pid: i32,
        wire: Wire,
        payload: Vec<u8>,
    ) -> Result<PokeResponseResult> {
        let request = PokeRequest {
            pid,
            wire: Some(wire),
            payload,
        };

        let response = self.client.poke(request).await?;
        let response = response.into_inner();

        match response.result {
            Some(poke_response::Result::Acknowledged(ack)) => {
                Ok(PokeResponseResult::Acknowledged(ack))
            }
            Some(poke_response::Result::Error(error)) => Ok(PokeResponseResult::Error(error)),
            None => Err(NockAppGrpcError::Internal("Empty response".to_string())),
        }
    }

    pub async fn poke(&mut self, pid: i32, wire: Wire, payload: Vec<u8>) -> Result<bool> {
        match self.poke_response(pid, wire, payload).await? {
            PokeResponseResult::Acknowledged(ack) => Ok(ack),
            PokeResponseResult::Error(error) => Err(NockAppGrpcError::Internal(error.message)),
        }
    }
}
