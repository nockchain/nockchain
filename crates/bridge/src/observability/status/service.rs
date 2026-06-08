use std::sync::{Arc, RwLock};

use hex::encode as hex_encode;
use tonic::{Request, Response, Status};
use tracing::warn;

use super::proto::bridge_status_server::BridgeStatus;
use super::proto::{
    Base58Hash, EthAddress as EthAddressProto, GetStatusRequest, GetStatusResponse, LastDeposit,
    RunningState, SuccessfulDeposit,
};
use super::state::BridgeStatus as BridgeStatusCache;
use crate::deposit::log::{DepositLog, DepositLogEntry};
use crate::deposit::types::NockDepositRequestData;
use crate::observability::health::NodeHealthStatus;
use crate::shared::config::NonceEpochConfig;
use crate::shared::stop::StopHandle;

#[derive(Clone, Debug)]
pub struct LastSubmittedDeposit {
    pub deposit: NockDepositRequestData,
    pub base_tx_hash: String,
    pub base_block_number: u64,
}

async fn load_last_successful_deposit(
    status_nonce: Option<u64>,
    log_nonce: Option<u64>,
    deposit_log: &DepositLog,
    nonce_epoch: &NonceEpochConfig,
) -> Option<SuccessfulDeposit> {
    for nonce in last_successful_deposit_nonce_candidates(status_nonce, log_nonce) {
        match deposit_log.get_by_nonce(nonce, nonce_epoch).await {
            Ok(Some(entry)) => return Some(successful_deposit_from_log_entry(nonce, entry)),
            Ok(None) => {
                warn!(
                    target: "bridge.status",
                    nonce,
                    "last successful deposit nonce was not present in local log"
                );
            }
            Err(err) => {
                warn!(
                    target: "bridge.status",
                    error=%err,
                    nonce,
                    "failed to load last successful deposit from log"
                );
            }
        }
    }
    None
}

fn last_successful_deposit_nonce_candidates(
    status_nonce: Option<u64>,
    log_nonce: Option<u64>,
) -> Vec<u64> {
    let mut candidates = Vec::with_capacity(2);
    if let Some(nonce) = status_nonce {
        candidates.push(nonce);
    }
    if let Some(nonce) = log_nonce {
        candidates.push(nonce);
    }
    candidates.sort_unstable_by(|a, b| b.cmp(a));
    candidates.dedup();
    candidates
}

fn successful_deposit_from_log_entry(nonce: u64, entry: DepositLogEntry) -> SuccessfulDeposit {
    SuccessfulDeposit {
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
    }
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
    stop_handle: StopHandle,
}

impl StatusService {
    pub fn new(
        state: BridgeStatusState,
        deposit_log: Arc<DepositLog>,
        nonce_epoch: NonceEpochConfig,
        bridge_status: BridgeStatusCache,
        stop_handle: StopHandle,
    ) -> Self {
        Self {
            state,
            deposit_log,
            nonce_epoch,
            bridge_status,
            stop_handle,
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
        let health = self.bridge_status.health_snapshots();
        let peer_unhealthy_count = health
            .iter()
            .filter(|snapshot| !matches!(snapshot.status, NodeHealthStatus::Healthy))
            .count() as u32;
        let healthy_nodes = (health.len() + 1).saturating_sub(peer_unhealthy_count as usize);
        let degradation_warning = if healthy_nodes < 4 {
            Some(format!(
                "bridge is degraded: {healthy_nodes}/5 nodes healthy"
            ))
        } else {
            None
        };
        let nockchain_api_connected = matches!(
            network.nockchain_api_status,
            crate::observability::tui::types::NockchainApiStatus::Connected { .. }
        );
        let nockchain_api_last_error = network
            .nockchain_api_status
            .last_error()
            .map(str::to_string);
        let last_stop_reason = self.stop_handle.info().map(|info| info.reason);
        let last_error_summary = self
            .bridge_status
            .alerts()
            .alerts
            .iter()
            .find(|alert| alert.severity >= crate::observability::tui::types::AlertSeverity::Error)
            .map(|alert| format!("{}: {}", alert.title, alert.message));

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

        let log_last_deposit_nonce =
            match self.deposit_log.max_nonce_in_epoch(&self.nonce_epoch).await {
                Ok(nonce) => nonce,
                Err(err) => {
                    warn!(
                        target: "bridge.status",
                        error=%err,
                        "failed to load last successful deposit nonce from log"
                    );
                    None
                }
            };
        let last_successful_deposit = load_last_successful_deposit(
            self.bridge_status.last_deposit_nonce(),
            log_last_deposit_nonce,
            &self.deposit_log,
            &self.nonce_epoch,
        )
        .await;

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
            pending_deposits: network.pending_deposits,
            pending_withdrawals: network.pending_withdrawals,
            unsettled_deposit_count: network.unsettled_deposit_count,
            unsettled_withdrawal_count: network.unsettled_withdrawal_count,
            degradation_warning,
            nockchain_api_connected,
            nockchain_api_last_error,
            peer_unhealthy_count,
            last_stop_reason,
            last_error_summary,
        }))
    }
}
