use std::net::SocketAddr;

use nockapp::driver::{make_driver, IODriverFn, NockAppHandle};
use nockchain_types::tx_engine::v1;
use nockvm::ext::NounExt;
use nockvm_macros::tas;
use noun_serde::{NounDecode, NounDecodeError};
use tracing::{error, info, warn};

use super::client::PublicNockchainGrpcClient;
use super::server::PublicNockchainGrpcServer;
use crate::pb::public::v2::wallet_send_transaction_response;

pub enum PublicNockchainEffect {
    SendTx { raw_tx: v1::RawTx },
}

impl NounDecode for PublicNockchainEffect {
    fn from_noun(effect: &nockapp::Noun) -> Result<Self, NounDecodeError> {
        let effect_cell = effect.as_cell()?;
        if !effect_cell.head().eq_bytes(b"nockchain-grpc") {
            return Err(NounDecodeError::InvalidTag);
        }

        let payload_cell = effect_cell.tail().as_cell()?;
        let tag_atom = payload_cell.head().as_atom()?;
        let tag = tag_atom
            .as_direct()
            .map_err(|_| NounDecodeError::InvalidTag)?
            .data();

        match tag {
            t if t == tas!(b"send-tx") => {
                let raw_tx = v1::RawTx::from_noun(&payload_cell.tail())?;
                Ok(PublicNockchainEffect::SendTx { raw_tx })
            }
            _ => Err(NounDecodeError::InvalidTag),
        }
    }
}

/// Create a public gRPC server driver for NockApp (read-only/public API)
pub fn grpc_server_driver(addr: SocketAddr) -> IODriverFn {
    make_driver(move |handle: NockAppHandle| async move {
        let server = PublicNockchainGrpcServer::new(handle);
        match server.serve(addr).await {
            Ok(_) => {
                info!("Public gRPC server shutting down gracefully");
                Ok(())
            }
            Err(e) => {
                error!("Public gRPC server error: {}", e);
                Err(nockapp::NockAppError::IoError(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Public gRPC server failed: {}", e),
                )))
            }
        }
    })
}

/// Connect to the public gRPC server and provide a client to the app if needed
pub fn grpc_listener_driver(addr: String) -> IODriverFn {
    make_driver(move |handle: NockAppHandle| async move {
        tracing::debug!("Starting public grpc listener driver");
        let mut client = PublicNockchainGrpcClient::connect(addr.to_string())
            .await
            .map_err(|e| {
                nockapp::NockAppError::OtherError(format!(
                    "Public gRPC client failed to connect: {}",
                    e
                ))
            })?;

        loop {
            let effect = match handle.next_effect().await {
                Ok(effect) => effect,
                Err(_) => continue,
            };

            let effect = match PublicNockchainEffect::from_noun(unsafe { effect.root() }) {
                Ok(effect) => effect,
                Err(NounDecodeError::InvalidTag) => continue,
                Err(err) => {
                    warn!("Failed to decode nockchain-grpc effect: {}", err);
                    continue;
                }
            };

            match effect {
                PublicNockchainEffect::SendTx { raw_tx } => {
                    match client.wallet_send_transaction(raw_tx).await {
                        Ok(resp) => match resp.result {
                            Some(wallet_send_transaction_response::Result::Ack(_)) => {
                                info!("wallet_send_transaction acknowledged: true");
                            }
                            Some(wallet_send_transaction_response::Result::Error(ref err)) => {
                                error!("wallet_send_transaction returned error: {}", err.message);
                            }
                            None => {
                                warn!("wallet_send_transaction response missing result");
                            }
                        },
                        Err(err) => {
                            error!("wallet_send_transaction failed: {}", err);
                        }
                    }
                }
            }
        }
    })
}
