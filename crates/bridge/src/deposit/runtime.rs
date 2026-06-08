use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::Address;
use async_trait::async_trait;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::deposit::cache::ProposalCache;
use crate::deposit::log::{validate_deposit_log_against_chain_nonce_prefix, DepositLog};
use crate::deposit::ports::BaseContractPort;
use crate::deposit::posting::run_posting_loop;
use crate::deposit::signing::run_signing_cursor_loop;
use crate::deposit::types::NockDepositRequestKernelData;
use crate::observability::health::PeerEndpoint;
use crate::observability::status::{BridgeStatus, BridgeStatusState};
use crate::shared::config::NonceEpochConfig;
use crate::shared::errors::BridgeError;
use crate::shared::kernel_projection::{
    plan_kernel_projection_boot, KernelProjectionBootPlan, KernelProjectionCursor,
    KernelProjectionPosition,
};
use crate::shared::runtime::BridgeRuntimeHandle;
use crate::shared::signing::BridgeSigner;
use crate::shared::stop::{StopController, StopHandle};
use crate::shared::types::NodeConfig;

const DEPOSIT_PROJECTION_REPLAY_OVERLAP: u64 = 1;

#[async_trait]
pub trait DepositKernelProjectionPort: Send + Sync {
    async fn peek_base_next_height(&self) -> Result<Option<u64>, BridgeError>;

    async fn peek_nock_next_height(&self) -> Result<Option<u64>, BridgeError>;

    async fn peek_nock_hashchain_deposits_since_height(
        &self,
        start_height: u64,
    ) -> Result<Vec<NockDepositRequestKernelData>, BridgeError>;
}

#[async_trait]
impl DepositKernelProjectionPort for BridgeRuntimeHandle {
    async fn peek_base_next_height(&self) -> Result<Option<u64>, BridgeError> {
        BridgeRuntimeHandle::peek_base_next_height(self).await
    }

    async fn peek_nock_next_height(&self) -> Result<Option<u64>, BridgeError> {
        BridgeRuntimeHandle::peek_nock_next_height(self).await
    }

    async fn peek_nock_hashchain_deposits_since_height(
        &self,
        start_height: u64,
    ) -> Result<Vec<NockDepositRequestKernelData>, BridgeError> {
        BridgeRuntimeHandle::peek_nock_hashchain_deposits_since_height(self, start_height).await
    }
}

pub struct DepositRuntimeContext<B> {
    pub runtime: Arc<BridgeRuntimeHandle>,
    pub base_bridge: Arc<B>,
    pub deposit_log: Arc<DepositLog>,
    pub nonce_epoch: NonceEpochConfig,
    pub proposal_cache: Arc<ProposalCache>,
    pub signer: Arc<BridgeSigner>,
    pub valid_addresses: HashSet<Address>,
    pub peers: Vec<PeerEndpoint>,
    pub self_node_id: u64,
    pub bridge_status: BridgeStatus,
    pub address_to_node_id: HashMap<Address, u64>,
    pub stop_controller: StopController,
    pub stop: StopHandle,
    pub status_state: BridgeStatusState,
    pub node_config: NodeConfig,
}

pub struct DepositRuntimeHandles {
    pub signing_cursor: JoinHandle<()>,
    pub posting: JoinHandle<()>,
}

pub async fn bootstrap_runtime<B: BaseContractPort>(
    context: &DepositRuntimeContext<B>,
) -> Result<(), BridgeError> {
    restore_deposit_log_from_kernel_projection(
        context.runtime.as_ref(),
        context.deposit_log.as_ref(),
        &context.nonce_epoch,
        Duration::from_secs(2),
    )
    .await?;

    let tip_height = context.runtime.nock_hashchain_tip().await?.unwrap_or(0);
    if tip_height >= context.nonce_epoch.start_height {
        validate_deposit_log_against_chain_nonce_prefix(
            context.base_bridge.clone(),
            context.deposit_log.clone(),
            context.nonce_epoch.clone(),
        )
        .await?;
    } else {
        info!(
            target: "bridge.deposit_log",
            tip_height,
            nonce_epoch_start_height = context.nonce_epoch.start_height,
            "skipping deposit log validation until hashchain reaches epoch start height"
        );
    }

    Ok(())
}

pub async fn restore_deposit_log_from_kernel_projection<K: DepositKernelProjectionPort>(
    kernel: &K,
    deposit_log: &DepositLog,
    nonce_epoch: &NonceEpochConfig,
    initial_delay: Duration,
) -> Result<u64, BridgeError> {
    if !initial_delay.is_zero() {
        tokio::time::sleep(initial_delay).await;
    }

    let current_position = peek_kernel_projection_position(kernel).await?;
    let existing_cursor = deposit_log.load_kernel_projection_cursor().await?;
    let has_projection_rows = deposit_log.has_kernel_projection_rows().await?;
    let boot_plan = if existing_cursor.is_none() && has_projection_rows {
        let legacy_position =
            legacy_deposit_projection_initial_position(deposit_log, nonce_epoch, &current_position)
                .await?;
        warn!(
            target: "bridge.deposit_log",
            base_next_height = legacy_position.base_next_height,
            nock_next_height = legacy_position.nock_next_height,
            current_base_next_height = current_position.base_next_height,
            current_nock_next_height = current_position.nock_next_height,
            "initializing missing deposit kernel projection cursor for legacy deposit log"
        );
        KernelProjectionBootPlan::Initialize(KernelProjectionCursor::from_position(legacy_position))
    } else {
        plan_kernel_projection_boot(
            existing_cursor,
            has_projection_rows,
            &current_position,
            current_position.clone(),
        )?
    };

    match boot_plan {
        KernelProjectionBootPlan::UseExisting(cursor) => {
            replay_deposit_projection_gap(
                kernel, deposit_log, nonce_epoch, cursor, current_position,
            )
            .await
        }
        KernelProjectionBootPlan::Initialize(cursor) => {
            if cursor.base_next_height == current_position.base_next_height
                && cursor.nock_next_height == current_position.nock_next_height
            {
                deposit_log.set_kernel_projection_cursor(cursor).await?;
                info!(
                    target: "bridge.deposit_log",
                    base_next_height = current_position.base_next_height,
                    nock_next_height = current_position.nock_next_height,
                    "initialized deposit kernel projection cursor"
                );
                return Ok(0);
            }

            replay_deposit_projection_gap(
                kernel, deposit_log, nonce_epoch, cursor, current_position,
            )
            .await
        }
    }
}

async fn legacy_deposit_projection_initial_position(
    deposit_log: &DepositLog,
    nonce_epoch: &NonceEpochConfig,
    current_position: &KernelProjectionPosition,
) -> Result<KernelProjectionPosition, BridgeError> {
    let nock_next_height = deposit_log
        .max_block_height(nonce_epoch)
        .await?
        .map(|height| {
            height
                .saturating_add(1)
                .min(current_position.nock_next_height)
        })
        .unwrap_or(current_position.nock_next_height);
    Ok(KernelProjectionPosition {
        base_next_height: current_position.base_next_height,
        base_tip_hash: None,
        nock_next_height,
        nock_tip_hash: None,
    })
}

async fn replay_deposit_projection_gap<K: DepositKernelProjectionPort>(
    kernel: &K,
    deposit_log: &DepositLog,
    nonce_epoch: &NonceEpochConfig,
    cursor: KernelProjectionCursor,
    current_position: KernelProjectionPosition,
) -> Result<u64, BridgeError> {
    if cursor.base_next_height == current_position.base_next_height
        && cursor.nock_next_height == current_position.nock_next_height
    {
        return Ok(0);
    }

    let replay_start = cursor
        .nock_next_height
        .saturating_sub(DEPOSIT_PROJECTION_REPLAY_OVERLAP);
    let records = kernel
        .peek_nock_hashchain_deposits_since_height(replay_start)
        .await?;
    if let Some(record) = records
        .iter()
        .find(|record| record.block_height >= current_position.nock_next_height)
    {
        return Err(BridgeError::Runtime(format!(
            "kernel returned deposit beyond observed Nock hashchain tip: deposit_block_height={} kernel_nock_next_height={}",
            record.block_height, current_position.nock_next_height
        )));
    }

    let inserted = deposit_log
        .replay_deposit_projection(
            records,
            KernelProjectionCursor::from_position(current_position.clone()),
            nonce_epoch,
        )
        .await?;
    info!(
        target: "bridge.deposit_log",
        inserted,
        replay_start,
        base_next_height = current_position.base_next_height,
        nock_next_height = current_position.nock_next_height,
        "deposit log replay from kernel projection cursor complete"
    );
    Ok(inserted)
}

async fn peek_kernel_projection_position<K: DepositKernelProjectionPort>(
    kernel: &K,
) -> Result<KernelProjectionPosition, BridgeError> {
    let base_next_height = kernel.peek_base_next_height().await?.ok_or_else(|| {
        BridgeError::Runtime("kernel Base hashchain next height is unavailable".into())
    })?;
    let nock_next_height = kernel.peek_nock_next_height().await?.ok_or_else(|| {
        BridgeError::Runtime("kernel Nock hashchain next height is unavailable".into())
    })?;
    Ok(KernelProjectionPosition {
        base_next_height,
        base_tip_hash: None,
        nock_next_height,
        nock_tip_hash: None,
    })
}

pub fn spawn_runtime_loops<B>(context: DepositRuntimeContext<B>) -> DepositRuntimeHandles
where
    B: BaseContractPort + 'static,
{
    let DepositRuntimeContext {
        runtime,
        base_bridge,
        deposit_log,
        nonce_epoch,
        proposal_cache,
        signer,
        valid_addresses,
        peers,
        self_node_id,
        bridge_status,
        address_to_node_id,
        stop_controller,
        stop,
        status_state,
        node_config,
    } = context;
    let signing_base_bridge = base_bridge.clone();
    let signing_proposal_cache = proposal_cache.clone();
    let signing_bridge_status = bridge_status.clone();
    let signing_stop = stop.clone();

    let signing_cursor = tokio::spawn(async move {
        run_signing_cursor_loop(
            runtime, signing_base_bridge, deposit_log, &nonce_epoch, signing_proposal_cache,
            signer, valid_addresses, peers, self_node_id, signing_bridge_status,
            address_to_node_id, stop_controller, signing_stop,
        )
        .await;
    });

    let posting = tokio::spawn(async move {
        run_posting_loop(
            proposal_cache, base_bridge, node_config, bridge_status, stop, status_state,
        )
        .await;
    });

    DepositRuntimeHandles {
        signing_cursor,
        posting,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use async_trait::async_trait;
    use nockchain_math::belt::Belt;
    use nockchain_types::v1::Name;
    use tempfile::TempDir;

    use super::*;
    use crate::deposit::log::{DepositLog, DepositLogEntry};
    use crate::shared::config::NonceEpochConfig;
    use crate::shared::kernel_projection::{KernelProjectionCursor, KernelProjectionPosition};
    use crate::shared::types::{EthAddress, Tip5Hash};

    #[derive(Clone)]
    struct FakeKernel {
        base_next_height: u64,
        nock_next_height: u64,
        deposits: Vec<NockDepositRequestKernelData>,
        replay_starts: Arc<Mutex<Vec<u64>>>,
    }

    #[async_trait]
    impl DepositKernelProjectionPort for FakeKernel {
        async fn peek_base_next_height(&self) -> Result<Option<u64>, BridgeError> {
            Ok(Some(self.base_next_height))
        }

        async fn peek_nock_next_height(&self) -> Result<Option<u64>, BridgeError> {
            Ok(Some(self.nock_next_height))
        }

        async fn peek_nock_hashchain_deposits_since_height(
            &self,
            start_height: u64,
        ) -> Result<Vec<NockDepositRequestKernelData>, BridgeError> {
            self.replay_starts.lock().unwrap().push(start_height);
            Ok(self
                .deposits
                .iter()
                .filter(|deposit| deposit.block_height >= start_height)
                .cloned()
                .collect())
        }
    }

    fn tip5(a: u64, b: u64, c: u64, d: u64, e: u64) -> Tip5Hash {
        Tip5Hash([Belt(a), Belt(b), Belt(c), Belt(d), Belt(e)])
    }

    fn position(base_next_height: u64, nock_next_height: u64) -> KernelProjectionPosition {
        KernelProjectionPosition {
            base_next_height,
            base_tip_hash: None,
            nock_next_height,
            nock_tip_hash: None,
        }
    }

    fn cursor(base_next_height: u64, nock_next_height: u64) -> KernelProjectionCursor {
        KernelProjectionCursor::from_position(position(base_next_height, nock_next_height))
    }

    fn epoch() -> NonceEpochConfig {
        NonceEpochConfig {
            base: 0,
            start_height: 1,
            start_tx_id: None,
        }
    }

    async fn open_log() -> (TempDir, DepositLog) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("deposit-log.sqlite");
        let log = DepositLog::open(path).await.unwrap();
        (dir, log)
    }

    async fn assert_cursor_position(
        log: &DepositLog,
        base_next_height: u64,
        nock_next_height: u64,
    ) {
        let cursor = log
            .load_kernel_projection_cursor()
            .await
            .unwrap()
            .expect("cursor exists");
        assert_eq!(cursor.base_next_height, base_next_height);
        assert_eq!(cursor.nock_next_height, nock_next_height);
    }

    fn kernel(
        base_next_height: u64,
        nock_next_height: u64,
        deposits: Vec<NockDepositRequestKernelData>,
    ) -> FakeKernel {
        FakeKernel {
            base_next_height,
            nock_next_height,
            deposits,
            replay_starts: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn request(block_height: u64, tx_id: Tip5Hash, amount: u64) -> NockDepositRequestKernelData {
        NockDepositRequestKernelData {
            block_height,
            tx_id,
            as_of: tip5(9, block_height, 0, 0, 0),
            name: Name::new(
                tip5(10, block_height, 0, 0, 0),
                tip5(11, block_height, 0, 0, 0),
            ),
            recipient: EthAddress([block_height as u8; 20]),
            amount,
        }
    }

    #[tokio::test]
    async fn missing_cursor_initializes_at_current_tip() {
        let (_dir, log) = open_log().await;
        let kernel = kernel(10, 20, vec![request(19, tip5(1, 0, 0, 0, 0), 10)]);

        let inserted =
            restore_deposit_log_from_kernel_projection(&kernel, &log, &epoch(), Duration::ZERO)
                .await
                .unwrap();

        assert_eq!(inserted, 0);
        assert_cursor_position(&log, 10, 20).await;
        assert!(kernel.replay_starts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn existing_cursor_catches_up_from_overlap() {
        let (_dir, log) = open_log().await;
        log.set_kernel_projection_cursor(cursor(10, 15))
            .await
            .unwrap();
        let kernel = kernel(
            12,
            18,
            vec![request(14, tip5(14, 0, 0, 0, 0), 14), request(16, tip5(16, 0, 0, 0, 0), 16)],
        );

        let inserted =
            restore_deposit_log_from_kernel_projection(&kernel, &log, &epoch(), Duration::ZERO)
                .await
                .unwrap();

        assert_eq!(inserted, 2);
        assert_eq!(*kernel.replay_starts.lock().unwrap(), vec![14]);
        assert_cursor_position(&log, 12, 18).await;
    }

    #[tokio::test]
    async fn legacy_deposit_rows_without_cursor_replay_from_last_deposit_height() {
        let (_dir, log) = open_log().await;
        let req = request(4, tip5(4, 0, 0, 0, 0), 4);
        let later = request(8, tip5(8, 0, 0, 0, 0), 8);
        let seed = req.clone();
        log.insert_entry(&DepositLogEntry {
            block_height: seed.block_height,
            tx_id: seed.tx_id,
            as_of: seed.as_of,
            name: seed.name,
            recipient: seed.recipient,
            amount_to_mint: seed.amount,
        })
        .await
        .unwrap();
        let kernel = kernel(10, 20, vec![req, later]);

        let inserted =
            restore_deposit_log_from_kernel_projection(&kernel, &log, &epoch(), Duration::ZERO)
                .await
                .unwrap();

        assert_eq!(inserted, 1);
        assert_eq!(*kernel.replay_starts.lock().unwrap(), vec![4]);
        assert_cursor_position(&log, 10, 20).await;
    }

    #[tokio::test]
    async fn cursor_ahead_of_kernel_fails() {
        let (_dir, log) = open_log().await;
        log.set_kernel_projection_cursor(cursor(11, 20))
            .await
            .unwrap();
        let kernel = kernel(10, 20, Vec::new());

        let err =
            restore_deposit_log_from_kernel_projection(&kernel, &log, &epoch(), Duration::ZERO)
                .await
                .unwrap_err();

        assert!(
            err.to_string().contains("ahead of kernel Base hashchain"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn existing_cursor_is_used_as_projection_frontier() {
        let (_dir, log) = open_log().await;
        log.set_kernel_projection_cursor(cursor(10, 20))
            .await
            .unwrap();
        let kernel = kernel(10, 20, Vec::new());

        let inserted =
            restore_deposit_log_from_kernel_projection(&kernel, &log, &epoch(), Duration::ZERO)
                .await
                .unwrap();

        assert_eq!(inserted, 0);
        assert!(kernel.replay_starts.lock().unwrap().is_empty());
        assert_cursor_position(&log, 10, 20).await;
    }
}
