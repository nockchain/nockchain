use std::sync::{Arc, RwLock};

use hex::encode as hex_encode;
use tonic::{Request, Response, Status};
use tracing::warn;

use crate::bridge_status::BridgeStatus as BridgeStatusCache;
use crate::config::NonceEpochConfig;
use crate::deposit_log::DepositLog;
use crate::types::NockDepositRequestData;

pub mod proto {
    tonic::include_proto!("bridge.status.v1");
}

use proto::bridge_status_server::BridgeStatus;
use proto::{
    Base58Hash, EthAddress as EthAddressProto, GetStatusRequest, GetStatusResponse, LastDeposit,
    RunningState, SuccessfulDeposit,
};

#[derive(Clone, Debug)]
pub struct LastSubmittedDeposit {
    pub deposit: NockDepositRequestData,
    pub base_tx_hash: String,
    pub base_block_number: u64,
}

#[derive(Clone, Debug, Default)]
pub struct BridgeStatusState {
    last_submitted_deposit: Arc<RwLock<Option<LastSubmittedDeposit>>>,
}

impl BridgeStatusState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update_last_submitted_deposit(&self, deposit: LastSubmittedDeposit) {
        if let Ok(mut guard) = self.last_submitted_deposit.write() {
            *guard = Some(deposit);
        }
    }

    pub fn last_submitted_deposit(&self) -> Option<LastSubmittedDeposit> {
        self.last_submitted_deposit
            .read()
            .ok()
            .and_then(|guard| guard.clone())
    }
}

#[derive(Clone)]
pub struct StatusService {
    state: BridgeStatusState,
    deposit_log: Arc<DepositLog>,
    nonce_epoch: NonceEpochConfig,
    bridge_status: BridgeStatusCache,
}

impl StatusService {
    pub fn new(
        state: BridgeStatusState,
        deposit_log: Arc<DepositLog>,
        nonce_epoch: NonceEpochConfig,
        bridge_status: BridgeStatusCache,
    ) -> Self {
        Self {
            state,
            deposit_log,
            nonce_epoch,
            bridge_status,
        }
    }
}

#[tonic::async_trait]
impl BridgeStatus for StatusService {
    async fn get_status(
        &self,
        _request: Request<GetStatusRequest>,
    ) -> Result<Response<GetStatusResponse>, Status> {
        let network = self.bridge_status.network();
        let base_height = if network.base.last_updated.is_some() {
            Some(network.base.height)
        } else {
            None
        };
        let nock_height = if network.nockchain.last_updated.is_some() {
            Some(network.nockchain.height)
        } else {
            None
        };

        let running_state = if network.kernel_stopped {
            RunningState::Stopped
        } else {
            RunningState::Running
        };

        let last_submitted_deposit = self
            .state
            .last_submitted_deposit()
            .map(|entry| LastDeposit {
                tx_id: Some(Base58Hash {
                    value: entry.deposit.tx_id.to_base58(),
                }),
                name_first: Some(Base58Hash {
                    value: entry.deposit.name.first.to_base58(),
                }),
                name_last: Some(Base58Hash {
                    value: entry.deposit.name.last.to_base58(),
                }),
                recipient: Some(EthAddressProto {
                    value: format!("0x{}", hex_encode(entry.deposit.recipient.0)),
                }),
                amount: entry.deposit.amount,
                block_height: entry.deposit.block_height,
                as_of: Some(Base58Hash {
                    value: entry.deposit.as_of.to_base58(),
                }),
                nonce: entry.deposit.nonce,
                base_tx_hash: entry.base_tx_hash,
                base_block_number: entry.base_block_number,
            });

        let last_deposit_nonce = self.bridge_status.last_deposit_nonce();
        let last_successful_deposit = match last_deposit_nonce {
            Some(nonce) => match self
                .deposit_log
                .get_by_nonce(nonce, &self.nonce_epoch)
                .await
            {
                Ok(Some(entry)) => Some(SuccessfulDeposit {
                    tx_id: Some(Base58Hash {
                        value: entry.tx_id.to_base58(),
                    }),
                    name_first: Some(Base58Hash {
                        value: entry.name.first.to_base58(),
                    }),
                    name_last: Some(Base58Hash {
                        value: entry.name.last.to_base58(),
                    }),
                    recipient: Some(EthAddressProto {
                        value: format!("0x{}", hex_encode(entry.recipient.0)),
                    }),
                    amount: entry.amount_to_mint,
                    block_height: entry.block_height,
                    as_of: Some(Base58Hash {
                        value: entry.as_of.to_base58(),
                    }),
                    nonce,
                }),
                Ok(None) => None,
                Err(err) => {
                    warn!(
                        target: "bridge.status",
                        error=%err,
                        nonce,
                        "failed to load last successful deposit from log"
                    );
                    None
                }
            },
            None => None,
        };

        Ok(Response::new(GetStatusResponse {
            running_state: running_state as i32,
            nock_hold: network.nock_hold,
            base_hold: network.base_hold,
            nock_hold_height: if network.nock_hold {
                network.nock_hold_height
            } else {
                None
            },
            base_hold_height: if network.base_hold {
                network.base_hold_height
            } else {
                None
            },
            nock_height,
            base_height,
            last_submitted_deposit,
            last_successful_deposit,
        }))
    }
}
