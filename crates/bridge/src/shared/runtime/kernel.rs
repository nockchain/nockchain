use std::collections::VecDeque;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use hex::encode;
use nockapp::driver::{make_driver, NockAppHandle, PokeResult};
use nockapp::nockapp::wire::WireRepr;
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::one_punch::OnePunchWire;
use nockapp::wire::Wire;
use nockapp::{Bytes, NounAllocator};
use noun_serde::{NounDecode, NounEncode};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, info, warn};

use crate::core::ports::KernelStatePort;
use crate::deposit::types::{BaseDepositSettlementEntry, NockDepositRequestKernelData};
use crate::observability::metrics;
use crate::shared::errors::BridgeError;
use crate::shared::types::{
    keccak256, BaseBlockCommitAck, BaseBlockRef, BaseEvent, BoolPeek, BridgeCause,
    BridgeCauseVariant, BridgeState, CountPeek, HeightPeek, HoldInfo, HoldPeek,
    NockDepositRequestsPeek, NockWithdrawalRequestsPeek, NockchainTxsMap, PendingBaseBlockCommit,
    PendingBaseBlockCommitPeek, RawBaseBlockEntry, RawBaseBlocks, StopInfoPeek, StopLastBlocks,
    Tip5Hash, Tx,
};
use crate::withdrawal::types::{BaseWithdrawalEntry, NockWithdrawalRequestKernelData};

const MAX_PENDING_EVENTS: usize = 1024;
const BASE_BLOCK_COMMIT_ACK_TIMEOUT: Duration = Duration::from_secs(30);

fn since_height_path_slab(tag: &str, start_height: u64) -> NounSlab<NockJammer> {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let path = vec![tag.to_string(), start_height.to_string()];
    let path_noun = path.to_noun(&mut slab);
    slab.set_root(path_noun);
    slab
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventId {
    pub kind: BridgeEventKind,
    pub timestamp_ms: u128,
    pub digest: [u8; 32],
}

impl EventId {
    pub fn digest_excerpt(&self) -> String {
        encode(&self.digest[..4])
    }
}

pub struct EventEnvelope<T> {
    pub id: EventId,
    pub payload: T,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeEventKind {
    ChainBase,
    ChainNock,
}

impl BridgeEventKind {
    fn as_str(&self) -> &'static str {
        match self {
            BridgeEventKind::ChainBase => "chain-base",
            BridgeEventKind::ChainNock => "chain-nock",
        }
    }
}

#[derive(Clone, Debug)]
pub enum BridgeEvent {
    Chain(Box<ChainEvent>),
}

impl BridgeEvent {
    fn kind(&self) -> BridgeEventKind {
        match self {
            BridgeEvent::Chain(ref chain) => match chain.as_ref() {
                ChainEvent::Base(_) => BridgeEventKind::ChainBase,
                ChainEvent::Nock(_) => BridgeEventKind::ChainNock,
            },
        }
    }

    fn identity_material(&self) -> Vec<u8> {
        match self {
            BridgeEvent::Chain(ref chain) => match chain.as_ref() {
                ChainEvent::Base(batch) => batch.identity_material(),
                ChainEvent::Nock(block) => block.identity_material(),
            },
        }
    }
}

#[derive(Clone, Debug)]
pub enum ChainEvent {
    Base(BaseBlockBatch),
    Nock(NockBlockEvent),
}

#[derive(Clone, Debug)]
pub struct BaseBlockBatch {
    pub version: u64,
    pub first_height: u64,
    pub last_height: u64,
    pub blocks: Vec<BaseBlockRef>,
    pub withdrawals: Vec<BaseWithdrawalEntry>,
    pub deposit_settlements: Vec<BaseDepositSettlementEntry>,
    /// Events per block height for conversion to RawBaseBlocks
    pub block_events: std::collections::HashMap<u64, Vec<BaseEvent>>,
    pub prev: Tip5Hash,
}

impl BaseBlockBatch {
    pub(crate) fn identity_material(&self) -> Vec<u8> {
        let mut material = Vec::new();
        material.extend_from_slice(&self.version.to_be_bytes());
        material.extend_from_slice(&self.first_height.to_be_bytes());
        material.extend_from_slice(&self.last_height.to_be_bytes());
        material.extend_from_slice(&self.prev.to_be_bytes());
        for block in &self.blocks {
            material.extend_from_slice(&block.height.to_be_bytes());
            material.extend_from_slice(&block.block_id.0);
        }
        for entry in &self.withdrawals {
            material.extend_from_slice(&entry.base_tx_id.0);
            material.extend_from_slice(&entry.withdrawal.raw_amount.to_be_bytes());
            if let Some(dest) = &entry.withdrawal.dest {
                material.extend_from_slice(&dest.to_be_bytes());
            }
        }
        for entry in &self.deposit_settlements {
            material.extend_from_slice(&entry.base_tx_id.0);
            material.extend_from_slice(&entry.settlement.data.counterpart.to_be_bytes());
            material.extend_from_slice(&entry.settlement.data.as_of.to_be_bytes());
            material.extend_from_slice(&entry.settlement.data.dest.0);
            material.extend_from_slice(&entry.settlement.data.settled_amount.to_be_bytes());
            material.extend_from_slice(&entry.settlement.data.bridge_fee.to_be_bytes());
            for fee in &entry.settlement.data.fees {
                material.extend_from_slice(&fee.address.0);
                material.extend_from_slice(&fee.amount.to_be_bytes());
            }
        }
        material
    }
}

impl From<BaseBlockBatch> for RawBaseBlocks {
    fn from(batch: BaseBlockBatch) -> Self {
        batch
            .blocks
            .into_iter()
            .map(|block_ref| RawBaseBlockEntry {
                height: block_ref.height,
                block_id: block_ref.block_id,
                parent_block_id: block_ref.parent_block_id,
                txs: batch
                    .block_events
                    .get(&block_ref.height)
                    .cloned()
                    .unwrap_or_default(),
            })
            .collect()
    }
}

#[derive(Clone)]
pub struct NockBlockEvent {
    pub block: nockchain_types::tx_engine::common::Page,
    pub page_slab: nockapp::noun::slab::NounSlab<nockapp::noun::slab::NockJammer>,
    pub page_noun: nockapp::Noun,
    pub txs: Vec<(nockchain_types::tx_engine::common::TxId, Tx)>,
}

impl std::fmt::Debug for NockBlockEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NockBlockEvent")
            .field("block", &self.block)
            .field("txs", &self.txs)
            .finish()
    }
}

impl NockBlockEvent {
    fn identity_material(&self) -> Vec<u8> {
        let mut material = Vec::new();
        for limb in self.block.digest.0.iter() {
            material.extend_from_slice(&limb.0.to_be_bytes());
        }
        for limb in self.block.parent.0.iter() {
            material.extend_from_slice(&limb.0.to_be_bytes());
        }
        material.extend_from_slice(&self.block.height.to_be_bytes());
        for (tx_id, _raw_tx) in &self.txs {
            for limb in tx_id.0.iter() {
                material.extend_from_slice(&limb.0.to_be_bytes());
            }
        }
        material
    }

    pub fn height(&self) -> u64 {
        self.block.height
    }

    pub fn block_hash(&self) -> [u8; 32] {
        let mut raw = [0u8; 40];
        for (idx, limb) in self.block.digest.0.iter().enumerate() {
            raw[idx * 8..(idx + 1) * 8].copy_from_slice(&limb.0.to_be_bytes());
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(&raw[8..]);
        out
    }

    pub fn parent_hash(&self) -> [u8; 32] {
        let mut raw = [0u8; 40];
        for (idx, limb) in self.block.parent.0.iter().enumerate() {
            raw[idx * 8..(idx + 1) * 8].copy_from_slice(&limb.0.to_be_bytes());
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(&raw[8..]);
        out
    }
}

#[derive(Clone)]
struct BridgeRuntimeHandleChannels {
    inbound_tx: Sender<EventEnvelope<BridgeEvent>>,
    peek_tx: Sender<PeekRequest>,
    poke_tx: Sender<BridgePoke>,
}

#[derive(Clone)]
struct BridgeRuntimeHandleState {
    base_tip_hash: Arc<RwLock<Option<String>>>,
}

#[derive(Clone)]
pub struct BridgeRuntimeHandle {
    channels: BridgeRuntimeHandleChannels,
    state: BridgeRuntimeHandleState,
}

impl BridgeRuntimeHandle {
    pub fn set_base_tip_hash(&self, tip_hash: String) {
        if tip_hash.is_empty() {
            return;
        }
        if let Ok(mut guard) = self.state.base_tip_hash.write() {
            *guard = Some(tip_hash);
        }
    }

    pub fn get_base_tip_hash(&self) -> Option<String> {
        self.state
            .base_tip_hash
            .read()
            .ok()
            .and_then(|guard| guard.clone())
    }

    pub async fn send_event(&self, event: BridgeEvent) -> Result<EventId, BridgeError> {
        let id = make_event_id(event.kind(), &event.identity_material());
        let envelope = EventEnvelope { id, payload: event };
        self.channels
            .inbound_tx
            .send(envelope)
            .await
            .map_err(|e| BridgeError::Runtime(format!("inbound channel closed: {}", e)))?;
        Ok(id)
    }

    /// Typed helper for harnesses/tests to inject a Base batch event.
    pub async fn inject_base_batch(&self, batch: BaseBlockBatch) -> Result<EventId, BridgeError> {
        self.send_event(BridgeEvent::Chain(Box::new(ChainEvent::Base(batch))))
            .await
    }

    /// Typed helper for harnesses/tests to inject a nock block event.
    pub async fn inject_nock_block(&self, block: NockBlockEvent) -> Result<EventId, BridgeError> {
        self.send_event(BridgeEvent::Chain(Box::new(ChainEvent::Nock(block))))
            .await
    }

    pub async fn peek_base_next_height(&self) -> Result<Option<u64>, BridgeError> {
        let path = vec!["base-hashchain-next-height".to_string()];
        self.peek_height_path(path).await
    }

    pub async fn peek_pending_base_block_commit(
        &self,
    ) -> Result<Option<PendingBaseBlockCommit>, BridgeError> {
        let path = vec!["pending-base-block-commit".to_string()];
        let peek = self
            .peek_typed_path::<PendingBaseBlockCommitPeek>(path)
            .await?;
        Ok(peek.and_then(|p| p.inner.flatten()))
    }

    pub async fn peek_nock_next_height(&self) -> Result<Option<u64>, BridgeError> {
        let path = vec!["nock-hashchain-next-height".to_string()];
        self.peek_height_path(path).await
    }

    /// Peek the current nock hashchain tip height derived from next height.
    pub async fn nock_hashchain_tip(&self) -> Result<Option<u64>, BridgeError> {
        Ok(self
            .peek_nock_next_height()
            .await?
            .map(|height| height.saturating_sub(1)))
    }

    pub async fn peek_nock_last_deposit_height(&self) -> Result<Option<u64>, BridgeError> {
        let path = vec!["nock-last-deposit-height".to_string()];
        self.peek_height_path(path).await
    }

    /// Peek the count of unsettled deposits (awaiting settlement on Base).
    pub async fn peek_unsettled_deposit_count(&self) -> Result<u64, BridgeError> {
        let path = vec!["unsettled-deposit-count".to_string()];
        self.peek_count_path(path).await
    }

    /// Peek all unsettled deposits as a list of nonce-free nock deposit requests.
    pub async fn peek_unsettled_deposits(
        &self,
    ) -> Result<Vec<NockDepositRequestKernelData>, BridgeError> {
        let path = vec!["unsettled-deposits".to_string()];
        let peek = self
            .peek_typed_path::<NockDepositRequestsPeek>(path)
            .await?;
        Ok(peek.and_then(|p| p.inner.flatten()).unwrap_or_default())
    }

    /// Peek all deposits in the nock hashchain as a list of nonce-free nock deposit requests.
    ///
    /// This is intended for deterministic backfill of the runtime deposit log during
    /// nonce epoch activation.
    pub async fn peek_nock_hashchain_deposits(
        &self,
    ) -> Result<Vec<NockDepositRequestKernelData>, BridgeError> {
        let path = vec!["nock-hashchain-deposits".to_string()];
        let peek = self
            .peek_typed_path::<NockDepositRequestsPeek>(path)
            .await?;
        Ok(peek.and_then(|p| p.inner.flatten()).unwrap_or_default())
    }

    /// Peek deposits in the nock hashchain with `block_height >= start_height`.
    ///
    /// This is intended for incremental backfill of the runtime deposit log.
    pub async fn peek_nock_hashchain_deposits_since_height(
        &self,
        start_height: u64,
    ) -> Result<Vec<NockDepositRequestKernelData>, BridgeError> {
        let peek = self
            .peek_typed_slab::<NockDepositRequestsPeek>(since_height_path_slab(
                "nock-hashchain-deposits-since-height", start_height,
            ))
            .await?
            .ok_or_else(|| {
                BridgeError::Runtime(format!(
                    "kernel nock hashchain deposits since-height peek returned no response: start_height={start_height}"
                ))
            })?;
        let records = peek.inner.flatten().unwrap_or_default();
        let tx_ids: Vec<String> = records
            .iter()
            .map(|req| {
                let hex = encode(req.tx_id.to_be_limb_bytes());
                format!("{} ({})", req.tx_id.to_base58(), hex)
            })
            .collect();
        info!(
            target: "bridge.peek",
            start_height,
            count = records.len(),
            tx_ids = ?tx_ids,
            "peeked nock hashchain deposits since height"
        );
        Ok(records)
    }

    /// Peek the count of unsettled withdrawals (awaiting settlement on Nockchain).
    pub async fn peek_unsettled_withdrawal_count(&self) -> Result<u64, BridgeError> {
        let path = vec!["unsettled-withdrawal-count".to_string()];
        self.peek_count_path(path).await
    }

    /// Peek all unsettled withdrawals as a list of withdrawal requests.
    pub async fn peek_unsettled_withdrawals(
        &self,
    ) -> Result<Vec<NockWithdrawalRequestKernelData>, BridgeError> {
        let path = vec!["unsettled-withdrawals".to_string()];
        let peek = self
            .peek_typed_path::<NockWithdrawalRequestsPeek>(path)
            .await?;
        Ok(peek.and_then(|p| p.inner.flatten()).unwrap_or_default())
    }

    /// Peek Base withdrawal burns with `base_batch_end >= start_height`.
    ///
    /// This is intended for incremental boot restore of the operator-side
    /// withdrawal request log.
    pub async fn peek_base_hashchain_withdrawals_since_height(
        &self,
        start_height: u64,
    ) -> Result<Vec<NockWithdrawalRequestKernelData>, BridgeError> {
        let peek = self
            .peek_typed_slab::<NockWithdrawalRequestsPeek>(since_height_path_slab(
                "base-hashchain-withdrawals-since-height", start_height,
            ))
            .await?
            .ok_or_else(|| {
                BridgeError::Runtime(format!(
                    "kernel base hashchain withdrawals since-height peek returned no response: start_height={start_height}"
                ))
            })?;
        let requests = peek.inner.flatten().unwrap_or_default();
        info!(
            target: "bridge.peek",
            start_height,
            count = requests.len(),
            "peeked base hashchain withdrawals since height"
        );
        Ok(requests)
    }

    /// Peek whether base chain processing is held waiting for nock.
    pub async fn peek_base_hold(&self) -> Result<bool, BridgeError> {
        Ok(self.peek_base_hold_info().await?.is_some())
    }

    /// Peek the base hold info (hash + height), if present.
    pub async fn peek_base_hold_info(&self) -> Result<Option<HoldInfo>, BridgeError> {
        let path = vec!["base-hold".to_string()];
        self.peek_hold_path(path).await
    }

    /// Peek the nock height that releases a base hold.
    pub async fn peek_base_hold_height(&self) -> Result<Option<u64>, BridgeError> {
        Ok(self.peek_base_hold_info().await?.map(|hold| hold.height))
    }

    /// Peek whether nock chain processing is held waiting for base.
    pub async fn peek_nock_hold(&self) -> Result<bool, BridgeError> {
        Ok(self.peek_nock_hold_info().await?.is_some())
    }

    /// Peek the nock hold info (hash + height), if present.
    pub async fn peek_nock_hold_info(&self) -> Result<Option<HoldInfo>, BridgeError> {
        let path = vec!["nock-hold".to_string()];
        self.peek_hold_path(path).await
    }

    /// Peek whether the kernel has latched a stop state.
    pub async fn peek_stop_state(&self) -> Result<bool, BridgeError> {
        let path = vec!["stop-state".to_string()];
        self.peek_bool_path(path).await
    }

    /// Peek the base height that releases a nock hold.
    pub async fn peek_nock_hold_height(&self) -> Result<Option<u64>, BridgeError> {
        Ok(self.peek_nock_hold_info().await?.map(|hold| hold.height))
    }

    /// Peek whether the bridge is running in fakenet mode.
    ///
    /// The Hoon kernel returns `true` if constants are NOT equal to the default
    /// mainnet constants, meaning the bridge is in fakenet mode (constants were
    /// overridden). Returns `false` for mainnet mode (using default constants).
    pub async fn peek_is_fakenet(&self) -> Result<bool, BridgeError> {
        let path = vec!["fakenet".to_string()];
        self.peek_bool_path(path).await
    }

    /// Peek the kernel's computed `stop-info` (last known good tips + heights).
    pub async fn peek_stop_info(&self) -> Result<Option<StopLastBlocks>, BridgeError> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let path = vec!["stop-info".to_string()];
        let path_noun = path.to_noun(&mut slab);
        slab.set_root(path_noun);

        let bytes_opt = self.peek_slab(slab).await?;
        let Some(bytes) = bytes_opt else {
            return Ok(None);
        };

        let slab = cue_bytes(bytes)?;
        let noun = unsafe { slab.root() };
        let space = slab.noun_space();
        let peek = StopInfoPeek::from_noun(noun, &space).map_err(|err| {
            BridgeError::Runtime(format!("failed to decode peek stop-info: {}", err))
        })?;
        Ok(peek.inner.flatten())
    }

    /// Fetch all kernel state counts in a single batch for TUI display.
    /// Returns defaults (0/false) for any failed peeks rather than failing entirely.
    pub async fn update_bridge_state(&self) -> BridgeState {
        let metrics = metrics::init_metrics();
        let total_started = Instant::now();

        let base_hold_info = {
            let started = Instant::now();
            let info = self.peek_base_hold_info().await.ok().flatten();
            metrics
                .bridge_state_peek_base_hold_info_time
                .add_timing(&started.elapsed());
            info
        };
        let nock_hold_info = {
            let started = Instant::now();
            let info = self.peek_nock_hold_info().await.ok().flatten();
            metrics
                .bridge_state_peek_nock_hold_info_time
                .add_timing(&started.elapsed());
            info
        };
        let unsettled_deposits = {
            let started = Instant::now();
            let count = self.peek_unsettled_deposit_count().await.unwrap_or(0);
            metrics
                .bridge_state_peek_unsettled_deposits_time
                .add_timing(&started.elapsed());
            count
        };
        let unsettled_withdrawals = {
            let started = Instant::now();
            let count = self.peek_unsettled_withdrawal_count().await.unwrap_or(0);
            metrics
                .bridge_state_peek_unsettled_withdrawals_time
                .add_timing(&started.elapsed());
            count
        };
        let base_next_height = {
            let started = Instant::now();
            let height = self.peek_base_next_height().await.ok().flatten();
            metrics
                .bridge_state_peek_base_next_height_time
                .add_timing(&started.elapsed());
            height
        };
        let nock_next_height = {
            let started = Instant::now();
            let height = self.peek_nock_next_height().await.ok().flatten();
            metrics
                .bridge_state_peek_nock_next_height_time
                .add_timing(&started.elapsed());
            height
        };
        let kernel_stopped = {
            let started = Instant::now();
            let stopped = self.peek_stop_state().await.unwrap_or(false);
            metrics
                .bridge_state_peek_stop_state_time
                .add_timing(&started.elapsed());
            stopped
        };
        let is_fakenet = {
            let started = Instant::now();
            let value = self.peek_is_fakenet().await.ok();
            metrics
                .bridge_state_peek_is_fakenet_time
                .add_timing(&started.elapsed());
            value
        };

        let state = BridgeState {
            unsettled_deposits,
            unsettled_withdrawals,
            base_tip_hash: self.get_base_tip_hash(),
            base_next_height,
            nock_next_height,
            base_hold: base_hold_info.is_some(),
            nock_hold: nock_hold_info.is_some(),
            kernel_stopped,
            is_fakenet,
            base_hold_height: base_hold_info.as_ref().map(|hold| hold.height),
            nock_hold_height: nock_hold_info.as_ref().map(|hold| hold.height),
        };

        metrics
            .bridge_state_snapshot_time
            .add_timing(&total_started.elapsed());
        state
    }

    async fn peek_count_path(&self, path: Vec<String>) -> Result<u64, BridgeError> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let path_noun = path.to_noun(&mut slab);
        slab.set_root(path_noun);

        let bytes_opt = self.peek_slab(slab).await?;
        let Some(bytes) = bytes_opt else {
            return Ok(0); // absent = 0 count
        };
        let slab = cue_bytes(bytes)?;
        let noun = unsafe { slab.root() };
        let space = slab.noun_space();
        let peek = CountPeek::from_noun(noun, &space)
            .map_err(|err| BridgeError::Runtime(format!("failed to decode peek count: {}", err)))?;
        Ok(peek.inner.flatten().unwrap_or(0))
    }

    async fn peek_bool_path(&self, path: Vec<String>) -> Result<bool, BridgeError> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let path_noun = path.to_noun(&mut slab);
        slab.set_root(path_noun);

        let bytes_opt = self.peek_slab(slab).await?;
        let Some(bytes) = bytes_opt else {
            return Ok(false); // absent = false
        };
        let slab = cue_bytes(bytes)?;
        let noun = unsafe { slab.root() };
        let space = slab.noun_space();
        let peek = BoolPeek::from_noun(noun, &space)
            .map_err(|err| BridgeError::Runtime(format!("failed to decode peek bool: {}", err)))?;
        Ok(peek.inner.flatten().unwrap_or(false))
    }

    async fn peek_height_path(&self, path: Vec<String>) -> Result<Option<u64>, BridgeError> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let path_noun = path.to_noun(&mut slab);
        slab.set_root(path_noun);

        let bytes_opt = self.peek_slab(slab).await?;
        let Some(bytes) = bytes_opt else {
            return Ok(None);
        };
        let slab = cue_bytes(bytes)?;
        let noun = unsafe { slab.root() };
        let space = slab.noun_space();
        let peek = HeightPeek::from_noun(noun, &space).map_err(|err| {
            BridgeError::Runtime(format!("failed to decode peek height: {}", err))
        })?;
        match peek.inner {
            Some(Some(height)) => Ok(Some(height)),
            _ => Ok(None),
        }
    }

    async fn peek_hold_path(&self, path: Vec<String>) -> Result<Option<HoldInfo>, BridgeError> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let path_noun = path.to_noun(&mut slab);
        slab.set_root(path_noun);

        let bytes_opt = self.peek_slab(slab).await?;
        let Some(bytes) = bytes_opt else {
            return Ok(None);
        };
        let slab = cue_bytes(bytes)?;
        let noun = unsafe { slab.root() };
        let space = slab.noun_space();
        let peek = HoldPeek::from_noun(noun, &space)
            .map_err(|err| BridgeError::Runtime(format!("failed to decode peek hold: {}", err)))?;
        Ok(peek.inner.flatten())
    }

    async fn peek_typed_path<T: NounDecode>(
        &self,
        path: Vec<String>,
    ) -> Result<Option<T>, BridgeError> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let path_noun = path.to_noun(&mut slab);
        slab.set_root(path_noun);
        self.peek_typed_slab(slab).await
    }

    async fn peek_typed_slab<T: NounDecode>(
        &self,
        path_slab: NounSlab<NockJammer>,
    ) -> Result<Option<T>, BridgeError> {
        let bytes_opt = self.peek_slab(path_slab).await?;
        let Some(bytes) = bytes_opt else {
            return Ok(None);
        };

        let slab = cue_bytes(bytes)?;
        let noun = unsafe { slab.root() };
        let space = slab.noun_space();
        let decoded = T::from_noun(noun, &space).map_err(|err| {
            BridgeError::Runtime(format!("failed to decode typed peek response: {}", err))
        })?;
        Ok(Some(decoded))
    }

    async fn peek_slab(
        &self,
        path_slab: NounSlab<NockJammer>,
    ) -> Result<Option<Vec<u8>>, BridgeError> {
        let (respond_to, response) = oneshot::channel();
        self.channels
            .peek_tx
            .send(PeekRequest {
                path_slab,
                respond_to,
            })
            .await
            .map_err(|e| BridgeError::Runtime(format!("peek channel closed: {}", e)))?;
        response
            .await
            .map_err(|e| BridgeError::Runtime(format!("peek response dropped: {}", e)))?
    }

    /// Send a poke directly to the kernel.
    /// This is used by the ingress service to forward validated peer proposals
    /// to the kernel.
    pub async fn send_poke(&self, poke: BridgePoke) -> Result<(), BridgeError> {
        self.channels
            .poke_tx
            .send(poke)
            .await
            .map_err(|e| BridgeError::Runtime(format!("poke channel closed: {}", e)))
    }

    pub async fn poke_blocking_timeout(
        &self,
        wire: WireRepr,
        slab: NounSlab<NockJammer>,
        timeout: Duration,
    ) -> Result<(), BridgeError> {
        let (respond_to, response) = oneshot::channel();
        tokio::time::timeout(
            timeout,
            self.channels
                .poke_tx
                .send(BridgePoke::blocking(wire, slab, timeout, respond_to)),
        )
        .await
        .map_err(|_| BridgeError::Runtime("timed out enqueueing bridge kernel poke".into()))?
        .map_err(|e| BridgeError::Runtime(format!("poke channel closed: {}", e)))?;

        tokio::time::timeout(timeout, response)
            .await
            .map_err(|_| {
                BridgeError::Runtime("timed out waiting for bridge kernel poke ack".into())
            })?
            .map_err(|e| {
                BridgeError::Runtime(format!("bridge kernel poke response dropped: {}", e))
            })??;
        Ok(())
    }

    pub async fn send_stop(&self, last: StopLastBlocks) -> Result<(), BridgeError> {
        let cause = BridgeCause::stop(last);
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = cause.to_noun(&mut slab);
        slab.set_root(noun);
        let wire = OnePunchWire::Poke.to_wire();
        self.send_poke(BridgePoke::new(wire, slab)).await
    }

    pub async fn send_start(&self) -> Result<(), BridgeError> {
        let cause = BridgeCause::start();
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = cause.to_noun(&mut slab);
        slab.set_root(noun);
        let wire = OnePunchWire::Poke.to_wire();
        self.send_poke(BridgePoke::new(wire, slab)).await
    }

    pub async fn send_base_block_withdrawals_committed(
        &self,
        ack: BaseBlockCommitAck,
    ) -> Result<(), BridgeError> {
        let cause = BridgeCause::base_block_withdrawals_committed(ack);
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = cause.to_noun(&mut slab);
        slab.set_root(noun);
        let wire = OnePunchWire::Poke.to_wire();
        self.poke_blocking_timeout(wire, slab, BASE_BLOCK_COMMIT_ACK_TIMEOUT)
            .await
    }
}

#[async_trait]
impl KernelStatePort for BridgeRuntimeHandle {
    async fn peek_base_hold(&self) -> Result<bool, BridgeError> {
        BridgeRuntimeHandle::peek_base_hold(self).await
    }

    async fn peek_base_pending_commit(&self) -> Result<bool, BridgeError> {
        Ok(BridgeRuntimeHandle::peek_pending_base_block_commit(self)
            .await?
            .is_some())
    }

    async fn peek_base_next_height(&self) -> Result<Option<u64>, BridgeError> {
        BridgeRuntimeHandle::peek_base_next_height(self).await
    }

    async fn peek_nock_next_height(&self) -> Result<Option<u64>, BridgeError> {
        BridgeRuntimeHandle::peek_nock_next_height(self).await
    }

    async fn emit_chain_event(&self, event: ChainEvent) -> Result<EventId, BridgeError> {
        self.send_event(BridgeEvent::Chain(Box::new(event))).await
    }

    fn set_base_tip_hash(&self, tip_hash: String) {
        BridgeRuntimeHandle::set_base_tip_hash(self, tip_hash);
    }
}

pub trait CauseBuilder: Send + Sync {
    fn build_poke(
        &self,
        event: &EventEnvelope<BridgeEvent>,
    ) -> Result<CauseBuildOutcome, BridgeError>;
}

pub enum CauseBuildOutcome {
    Emit(BridgePoke),
    Deferred(String),
    Ignored(String),
}

#[derive(Default)]
pub struct KernelCauseBuilder;

impl CauseBuilder for KernelCauseBuilder {
    fn build_poke(
        &self,
        event: &EventEnvelope<BridgeEvent>,
    ) -> Result<CauseBuildOutcome, BridgeError> {
        let BridgeEvent::Chain(ref chain) = &event.payload;
        match chain.as_ref() {
            ChainEvent::Base(batch) => {
                debug!(
                    target: "bridge.runtime.cause",
                    first_height=%batch.first_height,
                    last_height=%batch.last_height,
                    blocks_count=%batch.blocks.len(),
                    withdrawals_count=%batch.withdrawals.len(),
                    "building base-blocks cause from batch"
                );
                let raw_base_blocks: RawBaseBlocks = batch.clone().into();
                debug!(
                    target: "bridge.runtime.cause",
                    entries_count=%raw_base_blocks.len(),
                    "RawBaseBlocks after conversion"
                );
                let cause = BridgeCause(0, BridgeCauseVariant::BaseBlocks(raw_base_blocks));
                let mut slab: NounSlab<NockJammer> = NounSlab::new();
                let noun = cause.to_noun(&mut slab);
                debug!(
                    target: "bridge.runtime.cause",
                    noun_is_cell=%noun.is_cell(),
                    "encoded BridgeCause to noun"
                );
                slab.set_root(noun);
                let wire = OnePunchWire::Poke.to_wire();
                Ok(CauseBuildOutcome::Emit(BridgePoke::new(wire, slab)))
            }
            ChainEvent::Nock(nock_block) => {
                debug!(
                    target: "bridge.runtime.cause",
                    height=%nock_block.height(),
                    digest_b58=%nock_block.block.digest.to_base58(),
                    parent_b58=%nock_block.block.parent.to_base58(),
                    txs_count=%nock_block.txs.len(),
                    "building nockchain-block cause from block"
                );
                let mut poke_slab = NounSlab::new();
                let page_space = nock_block.page_slab.noun_space();
                let page_noun = poke_slab.copy_into(nock_block.page_noun, &page_space);
                let tag = String::from("nockchain-block").to_noun(&mut poke_slab);
                let txs = NockchainTxsMap(nock_block.txs.clone()).to_noun(&mut poke_slab);
                let cause =
                    nockvm::noun::T(&mut poke_slab, &[nockvm::noun::D(0), tag, page_noun, txs]);
                debug!(
                    target: "bridge.runtime.cause",
                    noun_is_cell=%cause.is_cell(),
                    "encoded NockchainBlock BridgeCause to noun"
                );
                poke_slab.set_root(cause);
                let wire = OnePunchWire::Poke.to_wire();
                Ok(CauseBuildOutcome::Emit(BridgePoke::new(wire, poke_slab)))
            }
        }
    }
}

pub struct BridgePoke {
    pub wire: WireRepr,
    pub slab: NounSlab<NockJammer>,
    respond_to: Option<oneshot::Sender<Result<(), BridgeError>>>,
    timeout: Option<Duration>,
}

impl BridgePoke {
    pub fn new(wire: WireRepr, slab: NounSlab<NockJammer>) -> Self {
        Self {
            wire,
            slab,
            respond_to: None,
            timeout: None,
        }
    }

    fn blocking(
        wire: WireRepr,
        slab: NounSlab<NockJammer>,
        timeout: Duration,
        respond_to: oneshot::Sender<Result<(), BridgeError>>,
    ) -> Self {
        Self {
            wire,
            slab,
            respond_to: Some(respond_to),
            timeout: Some(timeout),
        }
    }
}

struct PeekRequest {
    /// Pre-built noun slab containing the path to peek
    path_slab: NounSlab<NockJammer>,
    respond_to: oneshot::Sender<Result<Option<Vec<u8>>, BridgeError>>,
}

struct BridgeRuntimeDeps {
    cause_builder: Arc<dyn CauseBuilder>,
}

struct BridgeRuntimeChannels {
    inbound_rx: Receiver<EventEnvelope<BridgeEvent>>,
    poke_tx: Sender<BridgePoke>,
    poke_rx: Option<Receiver<BridgePoke>>,
    peek_rx: Option<Receiver<PeekRequest>>,
}

#[derive(Default)]
struct BridgeRuntimeState {
    pending_events: VecDeque<EventEnvelope<BridgeEvent>>,
}

pub struct BridgeRuntime {
    deps: BridgeRuntimeDeps,
    channels: BridgeRuntimeChannels,
    state: BridgeRuntimeState,
}

impl BridgeRuntime {
    pub fn new(cause_builder: Arc<dyn CauseBuilder>) -> (Self, BridgeRuntimeHandle) {
        let (inbound_tx, inbound_rx) = mpsc::channel(256);
        let (poke_tx, poke_rx) = mpsc::channel(128);
        let (peek_tx, peek_rx) = mpsc::channel(128);
        let base_tip_hash = Arc::new(RwLock::new(None));
        let handle_poke_tx = poke_tx.clone();
        let runtime = BridgeRuntime {
            deps: BridgeRuntimeDeps { cause_builder },
            channels: BridgeRuntimeChannels {
                inbound_rx,
                poke_tx,
                poke_rx: Some(poke_rx),
                peek_rx: Some(peek_rx),
            },
            state: BridgeRuntimeState::default(),
        };
        let handle = BridgeRuntimeHandle {
            channels: BridgeRuntimeHandleChannels {
                inbound_tx,
                peek_tx,
                poke_tx: handle_poke_tx,
            },
            state: BridgeRuntimeHandleState { base_tip_hash },
        };
        (runtime, handle)
    }

    pub async fn install_driver(
        &mut self,
        app: &mut nockapp::NockApp<NockJammer>,
    ) -> Result<(), BridgeError> {
        let poke_rx = self
            .channels
            .poke_rx
            .take()
            .ok_or_else(|| BridgeError::Runtime("driver already installed".into()))?;
        let peek_rx = self
            .channels
            .peek_rx
            .take()
            .ok_or_else(|| BridgeError::Runtime("driver already installed".into()))?;
        let driver = make_driver(move |handle: NockAppHandle| {
            let mut poke_rx = poke_rx;
            let mut peek_rx = peek_rx;
            async move {
                loop {
                    tokio::select! {
                        Some(poke) = poke_rx.recv() => {
                            let BridgePoke {
                                wire,
                                slab,
                                respond_to,
                                timeout,
                            } = poke;
                            let result = match timeout {
                                Some(timeout) => handle.poke_timeout(wire, slab, timeout).await,
                                None => handle.poke(wire, slab).await,
                            };
                            let result = match result {
                                Ok(PokeResult::Ack) => Ok(()),
                                Ok(PokeResult::Nack) => Err(BridgeError::Runtime(
                                    "bridge kernel poke returned nack".into(),
                                )),
                                Err(err) => Err(BridgeError::Runtime(format!(
                                    "bridge kernel poke failed: {err}"
                                ))),
                            };
                            match respond_to {
                                Some(respond_to) => {
                                    let _ = respond_to.send(result);
                                }
                                None => {
                                    if let Err(err) = result {
                                        error!(
                                            target: "bridge.runtime.driver",
                                            error=%err,
                                            "failed to poke kernel from runtime driver"
                                        );
                                    }
                                }
                            }
                        }
                        Some(peek) = peek_rx.recv() => {
                            let result = handle
                                .peek(peek.path_slab)
                                .await
                                .map(|opt| opt.map(|s| s.jam().to_vec()))
                                .map_err(|e| BridgeError::Runtime(e.to_string()));
                            let _ = peek.respond_to.send(result);
                        }
                        else => break,
                    }
                }
                Ok(())
            }
        });
        app.add_io_driver(driver).await;
        Ok(())
    }

    pub async fn run(mut self) -> Result<(), BridgeError> {
        loop {
            tokio::select! {
                // Use biased to prioritize channel messages over timer
                biased;

                event = self.channels.inbound_rx.recv() => {
                    match event {
                        Some(e) => self.process_event(e).await?,
                        None => break, // Channel closed, shutdown
                    }
                }
            }
        }
        Ok(())
    }

    async fn process_event(
        &mut self,
        event: EventEnvelope<BridgeEvent>,
    ) -> Result<(), BridgeError> {
        let outcome = self.deps.cause_builder.build_poke(&event)?;
        match outcome {
            CauseBuildOutcome::Emit(poke) => {
                self.channels
                    .poke_tx
                    .send(poke)
                    .await
                    .map_err(|e| BridgeError::Runtime(format!("failed to enqueue poke: {}", e)))?;
            }
            CauseBuildOutcome::Deferred(reason) => {
                let kind = event.id.kind.as_str().to_string();
                let digest = event.id.digest_excerpt();
                self.enqueue_pending(event);
                debug!(
                    target: "bridge.runtime",
                    kind=%kind,
                    digest=%digest,
                    reason=%reason,
                    pending=self.state.pending_events.len(),
                    "event deferred"
                );
            }
            CauseBuildOutcome::Ignored(reason) => {
                debug!(
                    target: "bridge.runtime",
                    kind=%event.id.kind.as_str(),
                    digest=%event.id.digest_excerpt(),
                    reason=%reason,
                    "event ignored"
                );
            }
        }
        Ok(())
    }

    fn enqueue_pending(&mut self, event: EventEnvelope<BridgeEvent>) {
        if self.state.pending_events.len() >= MAX_PENDING_EVENTS {
            if let Some(oldest) = self.state.pending_events.pop_front() {
                warn!(
                    target: "bridge.runtime",
                    kind=%oldest.id.kind.as_str(),
                    digest=%oldest.id.digest_excerpt(),
                    "dropping oldest pending event"
                );
            }
        }
        self.state.pending_events.push_back(event);
    }
}

fn make_event_id(kind: BridgeEventKind, material: &[u8]) -> EventId {
    let mut payload = Vec::new();
    payload.extend_from_slice(kind.as_str().as_bytes());
    payload.extend_from_slice(material);
    let digest = keccak256(&payload);
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    EventId {
        kind,
        timestamp_ms,
        digest,
    }
}

fn cue_bytes(bytes: Vec<u8>) -> Result<NounSlab<NockJammer>, BridgeError> {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let noun = slab
        .cue_into(Bytes::from(bytes))
        .map_err(|err| BridgeError::Runtime(err.to_string()))?;
    slab.set_root(noun);
    Ok(slab)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, Once};

    use kernels_open_bridge::KERNEL;
    use nockapp::kernel::form::Kernel;
    use nockapp::NockApp;
    use nockchain_math::belt::Belt;
    use tempfile::TempDir;
    use tokio::time::{sleep, Duration};

    use super::*;
    use crate::shared::types::{
        zero_tip5_hash, AtomBytes, BaseEventContent, BridgeConstants, EthAddress,
    };
    use crate::withdrawal::types::{
        CreateWithdrawalTxData, SelectedWithdrawalNoteData, Withdrawal, WithdrawalId,
        WithdrawalSnapshot,
    };

    const NICKS_PER_NOCK: u64 = 65_536;

    struct RecordingEventBuilder {
        events: Arc<Mutex<Vec<BridgeEvent>>>,
    }

    impl CauseBuilder for RecordingEventBuilder {
        fn build_poke(
            &self,
            event: &EventEnvelope<BridgeEvent>,
        ) -> Result<CauseBuildOutcome, BridgeError> {
            self.events
                .lock()
                .expect("recording event builder mutex poisoned")
                .push(event.payload.clone());
            Ok(CauseBuildOutcome::Deferred("test".into()))
        }
    }

    struct RecordingBuilder {
        events: Arc<Mutex<Vec<EventId>>>,
    }

    fn init_kernel_black_box_test_logging() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let _ = tracing_subscriber::fmt()
                .with_test_writer()
                .with_max_level(tracing::Level::INFO)
                .try_init();
        });
    }

    impl CauseBuilder for RecordingBuilder {
        fn build_poke(
            &self,
            event: &EventEnvelope<BridgeEvent>,
        ) -> Result<CauseBuildOutcome, BridgeError> {
            self.events
                .lock()
                .expect("Mutex poisoned in test - this should not happen")
                .push(event.id);
            Ok(CauseBuildOutcome::Deferred("test".into()))
        }
    }

    fn sample_base_batch() -> BaseBlockBatch {
        BaseBlockBatch {
            version: 0,
            first_height: 7,
            last_height: 7,
            blocks: vec![BaseBlockRef {
                height: 7,
                block_id: AtomBytes(vec![0x01, 0x02]),
                parent_block_id: AtomBytes(vec![0x00, 0x01]),
            }],
            withdrawals: Vec::new(),
            deposit_settlements: Vec::new(),
            block_events: HashMap::new(),
            prev: zero_tip5_hash(),
        }
    }

    #[tokio::test]
    async fn runtime_records_chain_events_via_cause_builder() -> Result<(), BridgeError> {
        let records = Arc::new(Mutex::new(Vec::new()));
        let builder = Arc::new(RecordingBuilder {
            events: records.clone(),
        });
        let (runtime, handle) = BridgeRuntime::new(builder);
        let runtime_task = tokio::spawn(runtime.run());

        let id = handle
            .send_event(BridgeEvent::Chain(Box::new(ChainEvent::Base(
                sample_base_batch(),
            ))))
            .await?;
        assert!(matches!(id.kind, BridgeEventKind::ChainBase));

        sleep(Duration::from_millis(20)).await;
        drop(handle);
        runtime_task
            .await
            .expect("Runtime task should complete successfully")?;

        let events = records
            .lock()
            .expect("Mutex poisoned in test - this should not happen");
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].kind, BridgeEventKind::ChainBase));
        Ok(())
    }

    #[tokio::test]
    async fn runtime_records_withdrawal_events() -> Result<(), BridgeError> {
        let base_event_id = AtomBytes((0..32).map(|offset| offset + 1).collect());
        let events = Arc::new(Mutex::new(Vec::new()));
        let builder = Arc::new(RecordingEventBuilder {
            events: events.clone(),
        });
        let (runtime, handle) = BridgeRuntime::new(builder);
        let runtime_task = tokio::spawn(runtime.run());

        let withdrawal = BaseWithdrawalEntry {
            base_tx_id: AtomBytes(vec![0x01]),
            withdrawal: Withdrawal {
                base_tx_id: AtomBytes(vec![0x01]),
                dest: None,
                raw_amount: 5,
            },
        };
        let mut block_events = HashMap::new();
        block_events.insert(
            10,
            vec![BaseEvent {
                base_event_id: base_event_id.clone(),
                content: BaseEventContent::BurnForWithdrawal {
                    burner: EthAddress([0xde; 20]),
                    amount: 5,
                    lock_root: zero_tip5_hash(),
                },
            }],
        );
        let batch = BaseBlockBatch {
            version: 0,
            first_height: 10,
            last_height: 10,
            blocks: vec![BaseBlockRef {
                height: 10,
                block_id: AtomBytes(vec![0x06]),
                parent_block_id: AtomBytes(vec![0x05]),
            }],
            withdrawals: vec![withdrawal.clone()],
            deposit_settlements: Vec::new(),
            block_events,
            prev: zero_tip5_hash(),
        };

        handle
            .send_event(BridgeEvent::Chain(Box::new(ChainEvent::Base(batch))))
            .await?;

        sleep(Duration::from_millis(20)).await;
        drop(handle);
        runtime_task
            .await
            .expect("Runtime task should complete successfully")?;

        let recorded = events.lock().expect("recording events mutex poisoned");
        assert_eq!(recorded.len(), 1);
        match &recorded[0] {
            BridgeEvent::Chain(ref chain) => {
                if let ChainEvent::Base(recorded_batch) = chain.as_ref() {
                    assert_eq!(recorded_batch.withdrawals.len(), 1);
                    assert_eq!(
                        recorded_batch.withdrawals[0].withdrawal.raw_amount,
                        withdrawal.withdrawal.raw_amount
                    );
                } else {
                    panic!("expected base chain event");
                }
            }
        }

        Ok(())
    }

    #[test]
    fn kernel_builder_emits_base_poke() -> Result<(), BridgeError> {
        let builder = KernelCauseBuilder;
        let event = EventEnvelope {
            id: make_event_id(BridgeEventKind::ChainBase, &[]),
            payload: BridgeEvent::Chain(Box::new(ChainEvent::Base(sample_base_batch()))),
        };
        let outcome = builder.build_poke(&event)?;
        assert!(matches!(outcome, CauseBuildOutcome::Emit(_)));
        Ok(())
    }

    fn jam_height_peek(peek: HeightPeek) -> Vec<u8> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = peek.to_noun(&mut slab);
        slab.set_root(noun);
        slab.jam().to_vec()
    }

    #[tokio::test]
    async fn peek_base_height_returns_value() -> Result<(), BridgeError> {
        let builder = Arc::new(RecordingBuilder {
            events: Arc::new(Mutex::new(Vec::new())),
        });
        let (mut runtime, handle) = BridgeRuntime::new(builder);
        let mut peek_rx = runtime
            .channels
            .peek_rx
            .take()
            .expect("peek receiver missing");

        let responder = tokio::spawn(async move {
            if let Some(request) = peek_rx.recv().await {
                // Note: path is now in path_slab as a NounSlab, not a Vec<String>
                let bytes = jam_height_peek(HeightPeek {
                    inner: Some(Some(42)),
                });
                let _ = request.respond_to.send(Ok(Some(bytes)));
            }
        });

        let height = handle.peek_base_next_height().await?;
        assert_eq!(height, Some(42));

        responder.await.expect("responder task failed");
        Ok(())
    }

    #[tokio::test]
    async fn peek_nock_height_handles_absent() -> Result<(), BridgeError> {
        let builder = Arc::new(RecordingBuilder {
            events: Arc::new(Mutex::new(Vec::new())),
        });
        let (mut runtime, handle) = BridgeRuntime::new(builder);
        let mut peek_rx = runtime
            .channels
            .peek_rx
            .take()
            .expect("peek receiver missing");

        let responder = tokio::spawn(async move {
            if let Some(request) = peek_rx.recv().await {
                // Note: path is now in path_slab as a NounSlab, not a Vec<String>
                let bytes = jam_height_peek(HeightPeek { inner: Some(None) });
                let _ = request.respond_to.send(Ok(Some(bytes)));
            }
        });

        let height = handle.peek_nock_next_height().await?;
        assert!(height.is_none());

        responder.await.expect("responder task failed");
        Ok(())
    }

    fn jam_count_peek(peek: CountPeek) -> Vec<u8> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = peek.to_noun(&mut slab);
        slab.set_root(noun);
        slab.jam().to_vec()
    }

    fn jam_hold_peek(peek: HoldPeek) -> Vec<u8> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = peek.to_noun(&mut slab);
        slab.set_root(noun);
        slab.jam().to_vec()
    }

    fn jam_deposit_requests_peek(peek: NockDepositRequestsPeek) -> Vec<u8> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = peek.to_noun(&mut slab);
        slab.set_root(noun);
        slab.jam().to_vec()
    }

    fn jam_withdrawal_requests_peek(peek: NockWithdrawalRequestsPeek) -> Vec<u8> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = peek.to_noun(&mut slab);
        slab.set_root(noun);
        slab.jam().to_vec()
    }

    fn jam_pending_base_block_commit_peek(peek: PendingBaseBlockCommitPeek) -> Vec<u8> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = peek.to_noun(&mut slab);
        slab.set_root(noun);
        slab.jam().to_vec()
    }

    fn decode_path_string_slab(path_slab: &NounSlab<NockJammer>) -> Vec<String> {
        let noun = unsafe { path_slab.root() };
        let space = path_slab.noun_space();
        Vec::<String>::from_noun(noun, &space).expect("decode string peek path")
    }

    fn decode_bridge_cause_slab(slab: &NounSlab<NockJammer>) -> BridgeCause {
        let noun = unsafe { slab.root() };
        let space = slab.noun_space();
        BridgeCause::from_noun(noun, &space).expect("decode bridge cause")
    }

    async fn setup_bridge_nockapp() -> Result<(TempDir, NockApp), BridgeError> {
        let temp_dir = TempDir::new()
            .map_err(|err| BridgeError::Runtime(format!("temp dir creation failed: {err}")))?;
        let kernel_bytes = KERNEL.to_vec();
        let kernel_f = move |_| async move {
            Kernel::load(&kernel_bytes, None, vec![], Default::default(), None).await
        };
        let app = NockApp::new(kernel_f)
            .await
            .map_err(|err| BridgeError::Runtime(format!("bridge nockapp setup failed: {err}")))?;
        Ok((temp_dir, app))
    }

    fn poke_cause_sync(app: &mut NockApp, cause: BridgeCause) -> Result<(), BridgeError> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = cause.to_noun(&mut slab);
        slab.set_root(noun);
        app.poke_sync(OnePunchWire::Poke.to_wire(), slab)
            .map(|_| ())
            .map_err(|err| BridgeError::Runtime(format!("bridge kernel poke failed: {err}")))
    }

    async fn poke_cause_async(app: &mut NockApp, cause: BridgeCause) -> Result<(), BridgeError> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = cause.to_noun(&mut slab);
        slab.set_root(noun);
        app.poke(OnePunchWire::Poke.to_wire(), slab)
            .await
            .map(|_| ())
            .map_err(|err| BridgeError::Runtime(format!("bridge kernel poke failed: {err}")))
    }

    async fn peek_base_hashchain_withdrawals_since_height_black_box(
        app: &mut NockApp,
        start_height: u64,
    ) -> Result<Vec<NockWithdrawalRequestKernelData>, BridgeError> {
        let slab = since_height_path_slab("base-hashchain-withdrawals-since-height", start_height);
        let noun = app
            .peek_handle(slab)
            .await
            .map_err(|err| BridgeError::Runtime(format!("bridge kernel peek failed: {err}")))?
            .ok_or_else(|| BridgeError::Runtime("bridge kernel peek unexpectedly absent".into()))?;
        let root = unsafe { noun.root() };
        let space = noun.noun_space();
        Vec::<NockWithdrawalRequestKernelData>::from_noun(root, &space).map_err(|err| {
            BridgeError::Runtime(format!(
                "failed to decode withdrawal requests peek: {err:?}"
            ))
        })
    }

    async fn peek_base_next_height_black_box(
        app: &mut NockApp,
    ) -> Result<Option<u64>, BridgeError> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let path = vec!["base-hashchain-next-height".to_string()];
        let noun = path.to_noun(&mut slab);
        slab.set_root(noun);

        let noun = app
            .peek(slab)
            .await
            .map_err(|err| BridgeError::Runtime(format!("bridge kernel peek failed: {err}")))?;
        let root = unsafe { noun.root() };
        let space = noun.noun_space();
        let peek = HeightPeek::from_noun(root, &space).map_err(|err| {
            BridgeError::Runtime(format!("failed to decode height peek: {err:?}"))
        })?;
        Ok(match peek.inner {
            Some(Some(height)) => Some(height),
            _ => None,
        })
    }

    async fn peek_pending_base_block_commit_black_box(
        app: &mut NockApp,
    ) -> Result<Option<PendingBaseBlockCommit>, BridgeError> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let path = vec!["pending-base-block-commit".to_string()];
        let noun = path.to_noun(&mut slab);
        slab.set_root(noun);

        let noun = app
            .peek(slab)
            .await
            .map_err(|err| BridgeError::Runtime(format!("bridge kernel peek failed: {err}")))?;
        let root = unsafe { noun.root() };
        let space = noun.noun_space();
        let peek = PendingBaseBlockCommitPeek::from_noun(root, &space).map_err(|err| {
            BridgeError::Runtime(format!(
                "failed to decode pending base block commit peek: {err:?}"
            ))
        })?;
        Ok(peek.inner.flatten())
    }

    async fn ack_pending_base_block_commit_black_box(
        app: &mut NockApp,
    ) -> Result<PendingBaseBlockCommit, BridgeError> {
        let pending = peek_pending_base_block_commit_black_box(app)
            .await?
            .ok_or_else(|| BridgeError::Runtime("expected pending base block commit".into()))?;
        poke_cause_async(
            app,
            BridgeCause::base_block_withdrawals_committed(pending.ack()),
        )
        .await?;
        Ok(pending)
    }

    fn sample_bridge_constants(base_start_height: u64) -> BridgeConstants {
        BridgeConstants {
            base_blocks_chunk: 1,
            base_start_height,
            nockchain_start_height: 0,
            minimum_event_nocks: 1,
            ..BridgeConstants::default()
        }
    }

    fn sample_create_withdrawal_tx_request() -> CreateWithdrawalTxData {
        let name = nockchain_types::tx_engine::common::Name::new(
            Tip5Hash([Belt(101), Belt(102), Belt(103), Belt(104), Belt(105)]),
            Tip5Hash([Belt(201), Belt(202), Belt(203), Belt(204), Belt(205)]),
        );
        let note = nockchain_types::v1::Note::V1(nockchain_types::v1::NoteV1::new(
            nockchain_types::tx_engine::common::BlockHeight(Belt(7)),
            name.clone(),
            nockchain_types::v1::NoteData::new(vec![
                nockchain_types::v1::NoteDataEntry::bridge_deposit([111, 222, 333]),
            ]),
            nockchain_types::tx_engine::common::Nicks(7_000_000_000usize),
        ));

        CreateWithdrawalTxData {
            id: WithdrawalId {
                as_of: Tip5Hash([Belt(11), Belt(12), Belt(13), Belt(14), Belt(15)]),
                base_event_id: AtomBytes((0..32).map(|offset| offset + 1).collect()),
            },
            recipient: Tip5Hash([Belt(21), Belt(22), Belt(23), Belt(24), Belt(25)]),
            amount: 6_534_088_928,
            burned_amount: 6_553_600_000,
            base_batch_end: 11_072,
            epoch: 0,
            snapshot: WithdrawalSnapshot {
                height: 19,
                block_id: Tip5Hash([Belt(31), Belt(32), Belt(33), Belt(34), Belt(35)]),
            },
            fee: 40_467_925,
            selected_notes: vec![SelectedWithdrawalNoteData { name, note }],
        }
    }

    fn sample_base_burn_batch(
        height: u64,
        block_id: Vec<u8>,
        parent_block_id: Vec<u8>,
        events: Vec<BaseEvent>,
    ) -> BaseBlockBatch {
        let mut block_events = HashMap::new();
        block_events.insert(height, events);
        BaseBlockBatch {
            version: 0,
            first_height: height,
            last_height: height,
            blocks: vec![BaseBlockRef {
                height,
                block_id: AtomBytes(block_id),
                parent_block_id: AtomBytes(parent_block_id),
            }],
            withdrawals: Vec::new(),
            deposit_settlements: Vec::new(),
            block_events,
            prev: zero_tip5_hash(),
        }
    }

    #[tokio::test]
    async fn peek_unsettled_deposit_count_returns_value() -> Result<(), BridgeError> {
        let builder = Arc::new(RecordingBuilder {
            events: Arc::new(Mutex::new(Vec::new())),
        });
        let (mut runtime, handle) = BridgeRuntime::new(builder);
        let mut peek_rx = runtime
            .channels
            .peek_rx
            .take()
            .expect("peek receiver missing");

        let responder = tokio::spawn(async move {
            if let Some(request) = peek_rx.recv().await {
                let bytes = jam_count_peek(CountPeek {
                    inner: Some(Some(5)),
                });
                let _ = request.respond_to.send(Ok(Some(bytes)));
            }
        });

        let count = handle.peek_unsettled_deposit_count().await?;
        assert_eq!(count, 5);

        responder.await.expect("responder task failed");
        Ok(())
    }

    #[tokio::test]
    async fn peek_count_returns_zero_on_absent() -> Result<(), BridgeError> {
        let builder = Arc::new(RecordingBuilder {
            events: Arc::new(Mutex::new(Vec::new())),
        });
        let (mut runtime, handle) = BridgeRuntime::new(builder);
        let mut peek_rx = runtime
            .channels
            .peek_rx
            .take()
            .expect("peek receiver missing");

        let responder = tokio::spawn(async move {
            if let Some(request) = peek_rx.recv().await {
                // Return None to simulate absent data
                let _ = request.respond_to.send(Ok(None));
            }
        });

        let count = handle.peek_unsettled_withdrawal_count().await?;
        assert_eq!(count, 0);

        responder.await.expect("responder task failed");
        Ok(())
    }

    #[tokio::test]
    async fn peek_base_hashchain_withdrawals_since_height_sends_expected_path_and_decodes_requests(
    ) -> Result<(), BridgeError> {
        let start_height = 79_561;
        let builder = Arc::new(RecordingBuilder {
            events: Arc::new(Mutex::new(Vec::new())),
        });
        let (mut runtime, handle) = BridgeRuntime::new(builder);
        let mut peek_rx = runtime
            .channels
            .peek_rx
            .take()
            .expect("peek receiver missing");

        let responder = tokio::spawn(async move {
            if let Some(request) = peek_rx.recv().await {
                assert_eq!(
                    decode_path_string_slab(&request.path_slab),
                    vec![
                        "base-hashchain-withdrawals-since-height".to_string(),
                        start_height.to_string(),
                    ]
                );
                let bytes = jam_withdrawal_requests_peek(NockWithdrawalRequestsPeek {
                    inner: Some(Some(vec![NockWithdrawalRequestKernelData {
                        base_event_id: AtomBytes((0..32).map(|offset| offset + 1).collect()),
                        recipient: Tip5Hash([Belt(11), Belt(12), Belt(13), Belt(14), Belt(15)]),
                        amount: 5,
                        base_batch_end: 42,
                        as_of: Tip5Hash([Belt(21), Belt(22), Belt(23), Belt(24), Belt(25)]),
                    }])),
                });
                let _ = request.respond_to.send(Ok(Some(bytes)));
            }
        });

        let requests = handle
            .peek_base_hashchain_withdrawals_since_height(start_height)
            .await?;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].amount, 5);
        assert_eq!(requests[0].base_batch_end, 42);

        responder.await.expect("responder task failed");
        Ok(())
    }

    #[tokio::test]
    async fn peek_base_hashchain_withdrawals_since_height_errors_on_absent_response(
    ) -> Result<(), BridgeError> {
        let builder = Arc::new(RecordingBuilder {
            events: Arc::new(Mutex::new(Vec::new())),
        });
        let (mut runtime, handle) = BridgeRuntime::new(builder);
        let mut peek_rx = runtime
            .channels
            .peek_rx
            .take()
            .expect("peek receiver missing");

        let responder = tokio::spawn(async move {
            if let Some(request) = peek_rx.recv().await {
                let _ = request.respond_to.send(Ok(None));
            }
        });

        let err = handle
            .peek_base_hashchain_withdrawals_since_height(40)
            .await
            .expect_err("absent projection peek should fail closed");
        assert!(
            err.to_string().contains("base hashchain withdrawals"),
            "unexpected error: {err}"
        );

        responder.await.expect("responder task failed");
        Ok(())
    }

    #[tokio::test]
    async fn peek_nock_hashchain_deposits_since_height_sends_expected_path_and_decodes_requests(
    ) -> Result<(), BridgeError> {
        let start_height = 13_507;
        let builder = Arc::new(RecordingBuilder {
            events: Arc::new(Mutex::new(Vec::new())),
        });
        let (mut runtime, handle) = BridgeRuntime::new(builder);
        let mut peek_rx = runtime
            .channels
            .peek_rx
            .take()
            .expect("peek receiver missing");

        let responder = tokio::spawn(async move {
            if let Some(request) = peek_rx.recv().await {
                assert_eq!(
                    decode_path_string_slab(&request.path_slab),
                    vec![
                        "nock-hashchain-deposits-since-height".to_string(),
                        start_height.to_string(),
                    ]
                );
                let bytes = jam_deposit_requests_peek(NockDepositRequestsPeek {
                    inner: Some(Some(vec![NockDepositRequestKernelData {
                        tx_id: Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
                        name: nockchain_types::tx_engine::common::Name::new(
                            Tip5Hash([Belt(11), Belt(12), Belt(13), Belt(14), Belt(15)]),
                            Tip5Hash([Belt(21), Belt(22), Belt(23), Belt(24), Belt(25)]),
                        ),
                        recipient: EthAddress([0xaa; 20]),
                        amount: 7,
                        block_height: 51,
                        as_of: Tip5Hash([Belt(31), Belt(32), Belt(33), Belt(34), Belt(35)]),
                    }])),
                });
                let _ = request.respond_to.send(Ok(Some(bytes)));
            }
        });

        let requests = handle
            .peek_nock_hashchain_deposits_since_height(start_height)
            .await?;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].amount, 7);
        assert_eq!(requests[0].block_height, 51);

        responder.await.expect("responder task failed");
        Ok(())
    }

    #[tokio::test]
    async fn peek_nock_hashchain_deposits_since_height_errors_on_absent_response(
    ) -> Result<(), BridgeError> {
        let builder = Arc::new(RecordingBuilder {
            events: Arc::new(Mutex::new(Vec::new())),
        });
        let (mut runtime, handle) = BridgeRuntime::new(builder);
        let mut peek_rx = runtime
            .channels
            .peek_rx
            .take()
            .expect("peek receiver missing");

        let responder = tokio::spawn(async move {
            if let Some(request) = peek_rx.recv().await {
                let _ = request.respond_to.send(Ok(None));
            }
        });

        let err = handle
            .peek_nock_hashchain_deposits_since_height(50)
            .await
            .expect_err("absent projection peek should fail closed");
        assert!(
            err.to_string().contains("nock hashchain deposits"),
            "unexpected error: {err}"
        );

        responder.await.expect("responder task failed");
        Ok(())
    }

    #[tokio::test]
    async fn peek_pending_base_block_commit_sends_expected_path_and_decodes_pending(
    ) -> Result<(), BridgeError> {
        let builder = Arc::new(RecordingBuilder {
            events: Arc::new(Mutex::new(Vec::new())),
        });
        let (mut runtime, handle) = BridgeRuntime::new(builder);
        let mut peek_rx = runtime
            .channels
            .peek_rx
            .take()
            .expect("peek receiver missing");

        let pending = PendingBaseBlockCommit {
            blocks_hash: Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            first_height: 40,
            last_height: 41,
            withdrawals: vec![NockWithdrawalRequestKernelData {
                base_event_id: AtomBytes((0..32).map(|offset| offset + 7).collect()),
                recipient: Tip5Hash([Belt(11), Belt(12), Belt(13), Belt(14), Belt(15)]),
                amount: 5,
                base_batch_end: 41,
                as_of: Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            }],
        };
        let expected = pending.clone();

        let responder = tokio::spawn(async move {
            if let Some(request) = peek_rx.recv().await {
                assert_eq!(
                    decode_path_string_slab(&request.path_slab),
                    vec!["pending-base-block-commit".to_string()]
                );
                let bytes = jam_pending_base_block_commit_peek(PendingBaseBlockCommitPeek {
                    inner: Some(Some(pending)),
                });
                let _ = request.respond_to.send(Ok(Some(bytes)));
            }
        });

        let decoded = handle
            .peek_pending_base_block_commit()
            .await?
            .expect("pending base commit");
        assert_eq!(decoded, expected);

        responder.await.expect("responder task failed");
        Ok(())
    }

    #[tokio::test]
    async fn send_base_block_withdrawals_committed_waits_for_blocking_ack(
    ) -> Result<(), BridgeError> {
        let builder = Arc::new(RecordingBuilder {
            events: Arc::new(Mutex::new(Vec::new())),
        });
        let (mut runtime, handle) = BridgeRuntime::new(builder);
        let mut poke_rx = runtime
            .channels
            .poke_rx
            .take()
            .expect("poke receiver missing");
        let ack = BaseBlockCommitAck {
            blocks_hash: Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            first_height: 40,
            last_height: 41,
        };
        let expected_ack = ack.clone();

        let responder = tokio::spawn(async move {
            let poke = poke_rx.recv().await.expect("blocking poke");
            assert_eq!(poke.timeout, Some(BASE_BLOCK_COMMIT_ACK_TIMEOUT));
            let cause = decode_bridge_cause_slab(&poke.slab);
            match cause.1 {
                BridgeCauseVariant::BaseBlockWithdrawalsCommitted(decoded) => {
                    assert_eq!(decoded, expected_ack);
                }
                _ => panic!("expected base block withdrawals committed cause"),
            }
            poke.respond_to
                .expect("blocking poke response channel")
                .send(Ok(()))
                .expect("send blocking poke ack");
        });

        handle.send_base_block_withdrawals_committed(ack).await?;
        responder.await.expect("responder task failed");
        Ok(())
    }

    #[tokio::test]
    async fn blocking_poke_returns_error_when_response_is_dropped() {
        let builder = Arc::new(RecordingBuilder {
            events: Arc::new(Mutex::new(Vec::new())),
        });
        let (mut runtime, handle) = BridgeRuntime::new(builder);
        let mut poke_rx = runtime
            .channels
            .poke_rx
            .take()
            .expect("poke receiver missing");

        let responder = tokio::spawn(async move {
            let _poke = poke_rx.recv().await.expect("blocking poke");
        });

        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        slab.set_root(nockvm::noun::D(0));
        let err = handle
            .poke_blocking_timeout(OnePunchWire::Poke.to_wire(), slab, Duration::from_secs(1))
            .await
            .expect_err("dropped response should fail");
        assert!(
            err.to_string()
                .contains("bridge kernel poke response dropped"),
            "unexpected error: {err}"
        );
        responder.await.expect("responder task failed");
    }

    #[tokio::test]
    async fn blocking_poke_returns_kernel_nack_error() {
        let builder = Arc::new(RecordingBuilder {
            events: Arc::new(Mutex::new(Vec::new())),
        });
        let (mut runtime, handle) = BridgeRuntime::new(builder);
        let mut poke_rx = runtime
            .channels
            .poke_rx
            .take()
            .expect("poke receiver missing");

        let responder = tokio::spawn(async move {
            let poke = poke_rx.recv().await.expect("blocking poke");
            poke.respond_to
                .expect("blocking poke response channel")
                .send(Err(BridgeError::Runtime(
                    "bridge kernel poke returned nack".into(),
                )))
                .expect("send blocking poke nack");
        });

        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        slab.set_root(nockvm::noun::D(0));
        let err = handle
            .poke_blocking_timeout(OnePunchWire::Poke.to_wire(), slab, Duration::from_secs(1))
            .await
            .expect_err("nack response should fail");
        assert!(
            err.to_string().contains("bridge kernel poke returned nack"),
            "unexpected error: {err}"
        );
        responder.await.expect("responder task failed");
    }

    #[tokio::test]
    async fn blocking_poke_returns_error_when_poke_channel_is_closed() {
        let builder = Arc::new(RecordingBuilder {
            events: Arc::new(Mutex::new(Vec::new())),
        });
        let (runtime, handle) = BridgeRuntime::new(builder);
        drop(runtime);

        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        slab.set_root(nockvm::noun::D(0));
        let err = handle
            .poke_blocking_timeout(OnePunchWire::Poke.to_wire(), slab, Duration::from_secs(1))
            .await
            .expect_err("closed poke channel should fail");
        assert!(
            err.to_string().contains("poke channel closed"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn blocking_poke_times_out_waiting_for_ack() {
        let builder = Arc::new(RecordingBuilder {
            events: Arc::new(Mutex::new(Vec::new())),
        });
        let (_runtime, handle) = BridgeRuntime::new(builder);

        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        slab.set_root(nockvm::noun::D(0));
        let err = handle
            .poke_blocking_timeout(OnePunchWire::Poke.to_wire(), slab, Duration::from_millis(1))
            .await
            .expect_err("missing ack should time out");
        assert!(
            err.to_string()
                .contains("timed out waiting for bridge kernel poke ack"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn bridge_kernel_black_box_peek_base_hashchain_withdrawals_since_height(
    ) -> Result<(), BridgeError> {
        let (_temp, mut app) = setup_bridge_nockapp().await?;
        poke_cause_async(
            &mut app,
            BridgeCause::set_constants(sample_bridge_constants(40)),
        )
        .await?;

        let older_event = BaseEvent {
            base_event_id: AtomBytes((0..32).map(|offset| offset + 1).collect()),
            content: BaseEventContent::BurnForWithdrawal {
                burner: EthAddress([0xaa; 20]),
                amount: NICKS_PER_NOCK + 5,
                lock_root: Tip5Hash([Belt(11), Belt(12), Belt(13), Belt(14), Belt(15)]),
            },
        };
        let newer_event_1 = BaseEvent {
            base_event_id: AtomBytes((0..32).map(|offset| offset + 41).collect()),
            content: BaseEventContent::BurnForWithdrawal {
                burner: EthAddress([0xbb; 20]),
                amount: NICKS_PER_NOCK + 7,
                lock_root: Tip5Hash([Belt(21), Belt(22), Belt(23), Belt(24), Belt(25)]),
            },
        };
        let newer_event_2 = BaseEvent {
            base_event_id: AtomBytes((0..32).map(|offset| offset + 81).collect()),
            content: BaseEventContent::BurnForWithdrawal {
                burner: EthAddress([0xcc; 20]),
                amount: NICKS_PER_NOCK + 9,
                lock_root: Tip5Hash([Belt(31), Belt(32), Belt(33), Belt(34), Belt(35)]),
            },
        };

        let older_batch = sample_base_burn_batch(40, vec![0x40], vec![0x00], vec![older_event]);
        let newer_batch = sample_base_burn_batch(
            41,
            vec![0x41],
            vec![0x40],
            vec![newer_event_1.clone(), newer_event_2.clone()],
        );

        poke_cause_async(
            &mut app,
            BridgeCause(0, BridgeCauseVariant::BaseBlocks(older_batch.into())),
        )
        .await?;
        let older_pending = ack_pending_base_block_commit_black_box(&mut app).await?;
        assert_eq!(older_pending.last_height, 40);
        poke_cause_async(
            &mut app,
            BridgeCause(0, BridgeCauseVariant::BaseBlocks(newer_batch.into())),
        )
        .await?;
        let newer_pending = ack_pending_base_block_commit_black_box(&mut app).await?;
        assert_eq!(newer_pending.last_height, 41);

        let mut since_forty =
            peek_base_hashchain_withdrawals_since_height_black_box(&mut app, 40).await?;
        since_forty.sort_by(|a, b| a.base_event_id.as_slice().cmp(b.base_event_id.as_slice()));
        assert_eq!(since_forty.len(), 3);

        let mut since_forty_one =
            peek_base_hashchain_withdrawals_since_height_black_box(&mut app, 41).await?;
        since_forty_one.sort_by(|a, b| a.base_event_id.as_slice().cmp(b.base_event_id.as_slice()));
        assert_eq!(since_forty_one.len(), 2);
        assert_eq!(since_forty_one[0].amount, NICKS_PER_NOCK + 7);
        assert_eq!(since_forty_one[0].base_batch_end, 41);
        assert_eq!(
            since_forty_one[0].recipient,
            Tip5Hash([Belt(21), Belt(22), Belt(23), Belt(24), Belt(25)])
        );
        assert_eq!(since_forty_one[1].amount, NICKS_PER_NOCK + 9);
        assert_eq!(since_forty_one[1].base_batch_end, 41);
        assert_eq!(
            since_forty_one[1].recipient,
            Tip5Hash([Belt(31), Belt(32), Belt(33), Belt(34), Belt(35)])
        );
        assert!(since_forty_one[0].as_of == since_forty_one[1].as_of);
        assert!(since_forty_one
            .iter()
            .all(|request| request.base_batch_end == 41));

        Ok(())
    }

    #[tokio::test]
    async fn bridge_kernel_black_box_peek_nock_hashchain_deposits_since_height_accepts_string_height(
    ) -> Result<(), BridgeError> {
        let start_height = 79_561;
        let (_temp, mut app) = setup_bridge_nockapp().await?;
        poke_cause_async(
            &mut app,
            BridgeCause::set_constants(BridgeConstants {
                nockchain_start_height: start_height,
                ..sample_bridge_constants(40)
            }),
        )
        .await?;

        let slab = since_height_path_slab("nock-hashchain-deposits-since-height", start_height);
        let response = app
            .peek_handle(slab)
            .await
            .map_err(|err| BridgeError::Runtime(format!("bridge kernel peek failed: {err}")))?;

        assert!(
            response.is_none(),
            "empty nock hashchain should return missing data after matching string height path"
        );

        Ok(())
    }

    #[tokio::test]
    async fn bridge_kernel_black_box_peek_base_hashchain_withdrawals_since_height_uses_string_height(
    ) -> Result<(), BridgeError> {
        let start_height = 79_561;
        let (_temp, mut app) = setup_bridge_nockapp().await?;
        poke_cause_async(
            &mut app,
            BridgeCause::set_constants(sample_bridge_constants(start_height)),
        )
        .await?;

        let event = BaseEvent {
            base_event_id: AtomBytes((0..32).map(|offset| offset + 1).collect()),
            content: BaseEventContent::BurnForWithdrawal {
                burner: EthAddress([0xdd; 20]),
                amount: NICKS_PER_NOCK + 11,
                lock_root: Tip5Hash([Belt(41), Belt(42), Belt(43), Belt(44), Belt(45)]),
            },
        };
        let batch = sample_base_burn_batch(start_height, vec![0x61], vec![0x60], vec![event]);

        poke_cause_async(
            &mut app,
            BridgeCause(0, BridgeCauseVariant::BaseBlocks(batch.into())),
        )
        .await?;
        let pending = ack_pending_base_block_commit_black_box(&mut app).await?;
        assert_eq!(pending.last_height, start_height);

        let withdrawals =
            peek_base_hashchain_withdrawals_since_height_black_box(&mut app, start_height).await?;
        assert_eq!(withdrawals.len(), 1);
        assert_eq!(withdrawals[0].amount, NICKS_PER_NOCK + 11);
        assert_eq!(withdrawals[0].base_batch_end, start_height);

        Ok(())
    }

    #[tokio::test]
    async fn bridge_kernel_black_box_filters_withdrawal_burns_at_nick_floor(
    ) -> Result<(), BridgeError> {
        let (_temp, mut app) = setup_bridge_nockapp().await?;
        poke_cause_async(
            &mut app,
            BridgeCause::set_constants(sample_bridge_constants(40)),
        )
        .await?;

        let below_floor = BaseEvent {
            base_event_id: AtomBytes((0..32).map(|offset| offset + 1).collect()),
            content: BaseEventContent::BurnForWithdrawal {
                burner: EthAddress([0xaa; 20]),
                amount: 1,
                lock_root: Tip5Hash([Belt(11), Belt(12), Belt(13), Belt(14), Belt(15)]),
            },
        };
        let exact_floor = BaseEvent {
            base_event_id: AtomBytes((0..32).map(|offset| offset + 41).collect()),
            content: BaseEventContent::BurnForWithdrawal {
                burner: EthAddress([0xbb; 20]),
                amount: NICKS_PER_NOCK,
                lock_root: Tip5Hash([Belt(21), Belt(22), Belt(23), Belt(24), Belt(25)]),
            },
        };
        let above_floor = BaseEvent {
            base_event_id: AtomBytes((0..32).map(|offset| offset + 81).collect()),
            content: BaseEventContent::BurnForWithdrawal {
                burner: EthAddress([0xcc; 20]),
                amount: NICKS_PER_NOCK + 1,
                lock_root: Tip5Hash([Belt(31), Belt(32), Belt(33), Belt(34), Belt(35)]),
            },
        };

        let batch = sample_base_burn_batch(
            40,
            vec![0x40],
            vec![0x00],
            vec![below_floor, exact_floor, above_floor],
        );
        poke_cause_async(
            &mut app,
            BridgeCause(0, BridgeCauseVariant::BaseBlocks(batch.into())),
        )
        .await?;
        let pending = ack_pending_base_block_commit_black_box(&mut app).await?;

        assert_eq!(pending.withdrawals.len(), 1);
        assert_eq!(pending.withdrawals[0].amount, NICKS_PER_NOCK + 1);
        assert_eq!(
            pending.withdrawals[0].recipient,
            Tip5Hash([Belt(31), Belt(32), Belt(33), Belt(34), Belt(35)])
        );

        Ok(())
    }

    #[tokio::test]
    async fn bridge_kernel_black_box_advances_base_next_height_after_pending_ack(
    ) -> Result<(), BridgeError> {
        let (_temp, mut app) = setup_bridge_nockapp().await?;
        poke_cause_async(
            &mut app,
            BridgeCause::set_constants(sample_bridge_constants(40)),
        )
        .await?;

        assert_eq!(peek_base_next_height_black_box(&mut app).await?, Some(40));

        let first_batch = sample_base_burn_batch(40, vec![0x40], vec![0x00], Vec::new());
        poke_cause_async(
            &mut app,
            BridgeCause(0, BridgeCauseVariant::BaseBlocks(first_batch.into())),
        )
        .await?;
        assert_eq!(peek_base_next_height_black_box(&mut app).await?, Some(40));
        let first_pending = peek_pending_base_block_commit_black_box(&mut app)
            .await?
            .expect("pending first base commit");
        assert_eq!(first_pending.first_height, 40);
        assert_eq!(first_pending.last_height, 40);
        assert!(first_pending.withdrawals.is_empty());
        ack_pending_base_block_commit_black_box(&mut app).await?;
        assert_eq!(peek_base_next_height_black_box(&mut app).await?, Some(41));

        let second_batch = sample_base_burn_batch(41, vec![0x41], vec![0x40], Vec::new());
        poke_cause_async(
            &mut app,
            BridgeCause(0, BridgeCauseVariant::BaseBlocks(second_batch.into())),
        )
        .await?;
        assert_eq!(peek_base_next_height_black_box(&mut app).await?, Some(41));
        ack_pending_base_block_commit_black_box(&mut app).await?;
        assert_eq!(peek_base_next_height_black_box(&mut app).await?, Some(42));

        Ok(())
    }

    #[test]
    fn bridge_kernel_black_box_accepts_create_withdrawal_tx_poke_without_constants(
    ) -> Result<(), BridgeError> {
        init_kernel_black_box_test_logging();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| BridgeError::Runtime(format!("test runtime build failed: {err}")))?;
        let (_temp, mut app) = runtime.block_on(setup_bridge_nockapp())?;
        poke_cause_sync(
            &mut app,
            BridgeCause(
                0,
                BridgeCauseVariant::CreateWithdrawalTx(sample_create_withdrawal_tx_request()),
            ),
        )?;
        Ok(())
    }

    #[test]
    fn bridge_kernel_black_box_poke_sync_masks_invalid_cause_via_crud() -> Result<(), BridgeError> {
        init_kernel_black_box_test_logging();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| BridgeError::Runtime(format!("test runtime build failed: {err}")))?;
        let (_temp, mut app) = runtime.block_on(setup_bridge_nockapp())?;

        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        slab.set_root(nockvm::noun::D(0));

        let effects = app
            .poke_sync(OnePunchWire::Poke.to_wire(), slab)
            .map_err(|err| BridgeError::Runtime(format!("bridge kernel poke failed: {err}")))?;
        assert!(effects.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn peek_base_hold_returns_true() -> Result<(), BridgeError> {
        let builder = Arc::new(RecordingBuilder {
            events: Arc::new(Mutex::new(Vec::new())),
        });
        let (mut runtime, handle) = BridgeRuntime::new(builder);
        let mut peek_rx = runtime
            .channels
            .peek_rx
            .take()
            .expect("peek receiver missing");

        let responder = tokio::spawn(async move {
            if let Some(request) = peek_rx.recv().await {
                let bytes = jam_hold_peek(HoldPeek {
                    inner: Some(Some(HoldInfo {
                        hash: crate::shared::types::zero_tip5_hash(),
                        height: 42,
                    })),
                });
                let _ = request.respond_to.send(Ok(Some(bytes)));
            }
        });

        let hold = handle.peek_base_hold().await?;
        assert!(hold);

        responder.await.expect("responder task failed");
        Ok(())
    }

    #[tokio::test]
    async fn peek_hold_returns_none_on_absent() -> Result<(), BridgeError> {
        let builder = Arc::new(RecordingBuilder {
            events: Arc::new(Mutex::new(Vec::new())),
        });
        let (mut runtime, handle) = BridgeRuntime::new(builder);
        let mut peek_rx = runtime
            .channels
            .peek_rx
            .take()
            .expect("peek receiver missing");

        let responder = tokio::spawn(async move {
            if let Some(request) = peek_rx.recv().await {
                // Return None to simulate absent data
                let _ = request.respond_to.send(Ok(None));
            }
        });

        let hold = handle.peek_nock_hold().await?;
        assert!(!hold);

        responder.await.expect("responder task failed");
        Ok(())
    }
}
