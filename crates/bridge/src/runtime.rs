use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use alloy::primitives::Address;
use async_trait::async_trait;
use hex::encode;
use nockapp::driver::{make_driver, NockAppHandle};
use nockapp::nockapp::wire::WireRepr;
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::one_punch::OnePunchWire;
use nockapp::wire::Wire;
use nockapp::Bytes;
use noun_serde::{NounDecode, NounEncode};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, info, warn};

use crate::bridge_status::BridgeStatus;
use crate::config::NonceEpochConfig;
use crate::core::posting::{
    PostingCandidateDecision, PostingPlanner, PostingReadyProposal, PostingTickPlanInput,
};
use crate::core::signing::{
    SigningCandidatePrecheckDecision, SigningCandidatePrecheckInput, SigningEpochBoundsDecision,
    SigningEpochBoundsDecisionInput, SigningPlanner, SigningProcessedDecision,
    SigningProcessedDecisionInput, SigningTickPlanAction, SigningTickPlanInput,
};
use crate::errors::BridgeError;
use crate::health::PeerEndpoint;
use crate::loop_policy::{PostingLoopPolicy, SigningLoopPolicy};
use crate::metrics;
use crate::ports::{BaseContractPort, KernelStatePort};
use crate::proposal_cache::ProposalCache;
use crate::signing::BridgeSigner;
use crate::types::{
    keccak256, BaseBlockRef, BaseDepositSettlementEntry, BaseEvent, BaseWithdrawalEntry, BoolPeek,
    BridgeCause, BridgeCauseVariant, BridgeState, CountPeek, HeightPeek, HoldInfo, HoldPeek,
    NockDepositRequestKernelData, NockDepositRequestsPeek, NockchainTxsMap, NodeConfig,
    RawBaseBlockEntry, RawBaseBlocks, StopInfoPeek, StopLastBlocks, Tip5Hash, Tx,
};

const MAX_PENDING_EVENTS: usize = 1024;
const SUBMIT_DEPOSIT_TIMEOUT_SECS: u64 = 60; // prevent hung RPC from stalling queue

fn format_ud_for_cord(value: u64) -> String {
    let raw = value.to_string();
    let mut out = String::with_capacity(raw.len() + raw.len() / 3);
    for (idx, ch) in raw.chars().rev().enumerate() {
        if idx > 0 && idx.is_multiple_of(3) {
            out.push('.');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

fn format_source_tx_id(tx_id: &Tip5Hash) -> String {
    tx_id.to_base58()
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeEffectKind {
    BaseContractCall,
    NockchainTx,
}

impl BridgeEffectKind {
    fn as_str(&self) -> &'static str {
        match self {
            BridgeEffectKind::BaseContractCall => "base-contract-call",
            BridgeEffectKind::NockchainTx => "nockchain-tx",
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
pub enum BridgeEffect {
    BaseContractCall(BaseContractCallEffect),
    NockchainTx(NockchainTxEffect),
}

impl BridgeEffect {
    fn kind(&self) -> BridgeEffectKind {
        match self {
            BridgeEffect::BaseContractCall(_) => BridgeEffectKind::BaseContractCall,
            BridgeEffect::NockchainTx(_) => BridgeEffectKind::NockchainTx,
        }
    }

    fn identity_material(&self) -> Vec<u8> {
        match self {
            BridgeEffect::BaseContractCall(effect) => effect.identity_material(),
            BridgeEffect::NockchainTx(effect) => effect.identity_material(),
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

#[derive(Clone, Debug)]
pub struct BaseContractCallEffect {
    pub submission: Vec<u8>,
}

impl BaseContractCallEffect {
    fn identity_material(&self) -> Vec<u8> {
        self.submission.clone()
    }
}

#[derive(Clone, Debug)]
pub struct NockchainTxEffect {
    pub transaction: Vec<u8>,
}

impl NockchainTxEffect {
    fn identity_material(&self) -> Vec<u8> {
        self.transaction.clone()
    }
}

#[derive(Clone)]
struct BridgeRuntimeHandleChannels {
    inbound_tx: Sender<EventEnvelope<BridgeEvent>>,
    effect_tx: Sender<EventEnvelope<BridgeEffect>>,
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

    pub async fn send_effect(&self, effect: BridgeEffect) -> Result<EventId, BridgeError> {
        let id = make_effect_id(effect.kind(), &effect.identity_material());
        let envelope = EventEnvelope {
            id,
            payload: effect,
        };
        self.channels
            .effect_tx
            .send(envelope)
            .await
            .map_err(|e| BridgeError::Runtime(format!("effect channel closed: {}", e)))?;
        Ok(id)
    }

    pub async fn peek_base_next_height(&self) -> Result<Option<u64>, BridgeError> {
        let path = vec!["base-hashchain-next-height".to_string()];
        self.peek_height_path(path).await
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
        let path = vec![
            "nock-hashchain-deposits-since-height".to_string(),
            format_ud_for_cord(start_height),
        ];
        let peek = self
            .peek_typed_path::<NockDepositRequestsPeek>(path)
            .await?;
        let records = peek.and_then(|p| p.inner.flatten()).unwrap_or_default();
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
        let peek = StopInfoPeek::from_noun(noun).map_err(|err| {
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
        let peek = CountPeek::from_noun(noun)
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
        let peek = BoolPeek::from_noun(noun)
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
        let peek = HeightPeek::from_noun(noun).map_err(|err| {
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
        let peek = HoldPeek::from_noun(noun)
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

        let bytes_opt = self.peek_slab(slab).await?;
        let Some(bytes) = bytes_opt else {
            return Ok(None);
        };

        let slab = cue_bytes(bytes)?;
        let noun = unsafe { slab.root() };
        let decoded = T::from_noun(noun).map_err(|err| {
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
    /// This is used by the ingress service to poke the kernel with proposed-base-call
    /// when validating incoming proposals from peers.
    pub async fn send_poke(&self, poke: BridgePoke) -> Result<(), BridgeError> {
        self.channels
            .poke_tx
            .send(poke)
            .await
            .map_err(|e| BridgeError::Runtime(format!("poke channel closed: {}", e)))
    }

    pub async fn send_stop(&self, last: StopLastBlocks) -> Result<(), BridgeError> {
        let cause = BridgeCause::stop(last);
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = cause.to_noun(&mut slab);
        slab.set_root(noun);
        let wire = OnePunchWire::Poke.to_wire();
        self.send_poke(BridgePoke { wire, slab }).await
    }

    pub async fn send_start(&self) -> Result<(), BridgeError> {
        let cause = BridgeCause::start();
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = cause.to_noun(&mut slab);
        slab.set_root(noun);
        let wire = OnePunchWire::Poke.to_wire();
        self.send_poke(BridgePoke { wire, slab }).await
    }
}

#[async_trait]
impl KernelStatePort for BridgeRuntimeHandle {
    async fn peek_base_hold(&self) -> Result<bool, BridgeError> {
        BridgeRuntimeHandle::peek_base_hold(self).await
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
                Ok(CauseBuildOutcome::Emit(BridgePoke { wire, slab }))
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
                let page_noun = poke_slab.copy_into(nock_block.page_noun);
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
                Ok(CauseBuildOutcome::Emit(BridgePoke {
                    wire,
                    slab: poke_slab,
                }))
            }
        }
    }
}

#[derive(Clone)]
pub struct BridgePoke {
    pub wire: WireRepr,
    pub slab: NounSlab<NockJammer>,
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
    effect_rx: Receiver<EventEnvelope<BridgeEffect>>,
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
        let (effect_tx, effect_rx) = mpsc::channel(256);
        let (poke_tx, poke_rx) = mpsc::channel(128);
        let (peek_tx, peek_rx) = mpsc::channel(128);
        let base_tip_hash = Arc::new(RwLock::new(None));
        let handle_poke_tx = poke_tx.clone();
        let runtime = BridgeRuntime {
            deps: BridgeRuntimeDeps { cause_builder },
            channels: BridgeRuntimeChannels {
                inbound_rx,
                effect_rx,
                poke_tx,
                poke_rx: Some(poke_rx),
                peek_rx: Some(peek_rx),
            },
            state: BridgeRuntimeState::default(),
        };
        let handle = BridgeRuntimeHandle {
            channels: BridgeRuntimeHandleChannels {
                inbound_tx,
                effect_tx,
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
                            if let Err(err) = handle.poke(poke.wire.clone(), poke.slab).await {
                                error!(
                                    target: "bridge.runtime.driver",
                                    error=%err,
                                    "failed to poke kernel from runtime driver"
                                );
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
                effect = self.channels.effect_rx.recv() => {
                    match effect {
                        Some(e) => self.process_effect(e).await?,
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

    async fn process_effect(
        &mut self,
        effect: EventEnvelope<BridgeEffect>,
    ) -> Result<(), BridgeError> {
        let detail = match &effect.payload {
            BridgeEffect::BaseContractCall(data) => {
                format!("submission_bytes={}", data.submission.len())
            }
            BridgeEffect::NockchainTx(data) => format!("tx_bytes={}", data.transaction.len()),
        };
        info!(
            target: "bridge.runtime.effects",
            kind=%effect.id.kind.as_str(),
            digest=%effect.id.digest_excerpt(),
            detail=%detail,
            "queued effect awaiting transport"
        );
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

fn make_effect_id(kind: BridgeEffectKind, material: &[u8]) -> EventId {
    let mut payload = Vec::new();
    payload.extend_from_slice(kind.as_str().as_bytes());
    payload.extend_from_slice(material);
    let digest = keccak256(&payload);
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    EventId {
        kind: match kind {
            BridgeEffectKind::BaseContractCall => BridgeEventKind::ChainBase,
            BridgeEffectKind::NockchainTx => BridgeEventKind::ChainNock,
        },
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

#[derive(Clone, Copy, Debug)]
enum SignatureBroadcastReason {
    Initial,
    Regossip,
}

impl SignatureBroadcastReason {
    fn as_str(self) -> &'static str {
        match self {
            SignatureBroadcastReason::Initial => "initial",
            SignatureBroadcastReason::Regossip => "regossip",
        }
    }
}

fn system_time_secs(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn spawn_signature_broadcast(
    peers: &[PeerEndpoint],
    msg: &crate::ingress::proto::SignatureBroadcast,
    prop_id: &str,
    reason: SignatureBroadcastReason,
) {
    use tracing::{debug, warn};

    use crate::ingress::proto::bridge_ingress_client::BridgeIngressClient;

    for peer in peers {
        let msg = msg.clone();
        let addr = peer.address.clone();
        let peer_id = peer.node_id;
        let prop_id = prop_id.to_string();

        tokio::spawn(async move {
            match BridgeIngressClient::connect(addr.clone()).await {
                Ok(mut client) => match client.broadcast_signature(msg).await {
                    Ok(_) => {
                        debug!(
                            target: "bridge.cursor",
                            peer_node_id=peer_id,
                            proposal_hash=%prop_id,
                            reason=reason.as_str(),
                            "broadcast signature to peer"
                        );
                    }
                    Err(e) => {
                        warn!(
                            target: "bridge.cursor",
                            peer_node_id=peer_id,
                            error=%e,
                            reason=reason.as_str(),
                            "failed to broadcast signature to peer"
                        );
                    }
                },
                Err(e) => {
                    warn!(
                        target: "bridge.cursor",
                        peer_node_id=peer_id,
                        peer_address=%addr,
                        error=%e,
                        reason=reason.as_str(),
                        "failed to connect to peer for signature broadcast"
                    );
                }
            }
        });
    }
}

#[derive(Clone, Debug)]
pub struct SigningTickState {
    pub logged_epoch_ready: bool,
    pub last_regossip_at: SystemTime,
}

impl SigningTickState {
    pub fn new(now: SystemTime) -> Self {
        Self {
            logged_epoch_ready: false,
            last_regossip_at: now,
        }
    }
}

impl Default for SigningTickState {
    fn default() -> Self {
        Self::new(UNIX_EPOCH)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SigningTickInput {
    pub now: SystemTime,
    pub tip_height: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SigningTickOutcome {
    pub regossip_broadcasts: usize,
    pub initial_broadcasts: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SigningCandidateExecutionResult {
    Broadcasted,
    SignFailed,
    DuplicateSignature,
    ProposalStale,
    InvalidOwnSignature,
    CacheUpdateFailed,
}

#[derive(Clone)]
pub struct SigningTickPorts<B: BaseContractPort> {
    runtime: Arc<BridgeRuntimeHandle>,
    base_bridge: Arc<B>,
    deposit_log: Arc<crate::deposit_log::DepositLog>,
    proposal_cache: Arc<ProposalCache>,
}

impl<B: BaseContractPort> SigningTickPorts<B> {
    pub fn new(
        runtime: Arc<BridgeRuntimeHandle>,
        base_bridge: Arc<B>,
        deposit_log: Arc<crate::deposit_log::DepositLog>,
        proposal_cache: Arc<ProposalCache>,
    ) -> Self {
        Self {
            runtime,
            base_bridge,
            deposit_log,
            proposal_cache,
        }
    }
}

#[derive(Clone)]
pub struct SigningTickNodeState {
    signer: Arc<BridgeSigner>,
    valid_addresses: Arc<HashSet<Address>>,
    peers: Arc<Vec<PeerEndpoint>>,
    self_node_id: u64,
    address_to_node_id: Arc<std::collections::HashMap<Address, u64>>,
}

impl SigningTickNodeState {
    pub fn new(
        signer: Arc<BridgeSigner>,
        valid_addresses: HashSet<Address>,
        peers: Vec<PeerEndpoint>,
        self_node_id: u64,
        address_to_node_id: std::collections::HashMap<Address, u64>,
    ) -> Self {
        Self {
            signer,
            valid_addresses: Arc::new(valid_addresses),
            peers: Arc::new(peers),
            self_node_id,
            address_to_node_id: Arc::new(address_to_node_id),
        }
    }
}

#[derive(Clone)]
pub struct SigningTickControl {
    bridge_status: BridgeStatus,
    stop_controller: crate::stop::StopController,
    stop: crate::stop::StopHandle,
    local_stop_mode: SigningLocalStopMode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SigningLocalStopMode {
    #[default]
    RuntimeProbeAndBroadcast,
    LocalTriggerOnly,
}

impl SigningTickControl {
    pub fn new(
        bridge_status: BridgeStatus,
        stop_controller: crate::stop::StopController,
        stop: crate::stop::StopHandle,
    ) -> Self {
        Self {
            bridge_status,
            stop_controller,
            stop,
            local_stop_mode: SigningLocalStopMode::RuntimeProbeAndBroadcast,
        }
    }

    pub fn with_local_stop_mode(mut self, local_stop_mode: SigningLocalStopMode) -> Self {
        self.local_stop_mode = local_stop_mode;
        self
    }
}

#[derive(Clone)]
pub struct SigningTickConfig {
    nonce_epoch: NonceEpochConfig,
    policy: SigningLoopPolicy,
}

impl SigningTickConfig {
    pub fn new(nonce_epoch: &NonceEpochConfig, policy: SigningLoopPolicy) -> Self {
        Self {
            nonce_epoch: nonce_epoch.clone(),
            policy,
        }
    }
}

#[derive(Clone)]
pub struct SigningTickContext<B: BaseContractPort> {
    ports: SigningTickPorts<B>,
    node: SigningTickNodeState,
    control: SigningTickControl,
    config: SigningTickConfig,
}

impl<B: BaseContractPort> SigningTickContext<B> {
    pub fn new(
        ports: SigningTickPorts<B>,
        node: SigningTickNodeState,
        control: SigningTickControl,
        config: SigningTickConfig,
    ) -> Self {
        Self {
            ports,
            node,
            control,
            config,
        }
    }

    async fn execute_signing_candidate(
        &self,
        req: &crate::types::NockDepositRequestData,
        deposit_id: &crate::types::DepositId,
        now: SystemTime,
        now_secs: u64,
    ) -> SigningCandidateExecutionResult {
        use tracing::{debug, error, info, warn};

        use crate::ingress::proto::SignatureBroadcast;
        use crate::signing::verify_bridge_signature;

        let my_eth_address = self.node.signer.address();
        let proposal_hash = req.compute_proposal_hash();
        let proposal_id = hex::encode(proposal_hash);

        // Ensure the proposal is visible in the TUI even if no kernel `%commit-nock-deposits`
        // effect fires on this node (e.g. after restart).
        self.control
            .bridge_status
            .update_proposal(crate::tui::types::Proposal {
                id: proposal_id.clone(),
                proposal_type: "deposit".to_string(),
                description: format!(
                    "Deposit {} wei to {} (nonce {})",
                    req.amount,
                    hex::encode(req.recipient.0),
                    req.nonce
                ),
                signatures_collected: 0,
                signatures_required: crate::proposal_cache::SIGNATURE_THRESHOLD as u8,
                signers: vec![],
                created_at: now,
                status: crate::tui::types::ProposalStatus::Pending,
                data_hash: proposal_id.clone(),
                submitted_at_block: None,
                submitted_at: None,
                tx_hash: None,
                time_to_submit_ms: None,
                executed_at_block: None,
                source_block: Some(req.block_height),
                amount: Some(req.amount as u128),
                recipient: Some(format!("0x{}", hex::encode(req.recipient.0))),
                nonce: Some(req.nonce),
                source_tx_id: Some(format_source_tx_id(&req.tx_id)),
                current_proposer: None,
                is_my_turn: false,
                time_until_takeover: None,
            });

        // Step 1: Sign the proposal locally.
        let signature = match self.node.signer.sign_hash(&proposal_hash).await {
            Ok(sig) => sig.as_bytes().to_vec(),
            Err(e) => {
                error!(
                    target: "bridge.cursor",
                    error=%e,
                    proposal_hash=%proposal_id,
                    "failed to sign proposal"
                );
                return SigningCandidateExecutionResult::SignFailed;
            }
        };

        // Step 2: Add own signature to cache.
        let add_result = self.ports.proposal_cache.add_signature(
            deposit_id,
            crate::proposal_cache::SignatureData {
                signer_address: my_eth_address,
                signature: signature.clone(),
                proposal_hash,
                is_mine: true,
            },
            Some(req.clone()),
            |hash, sig| verify_bridge_signature(hash, sig, &self.node.valid_addresses),
        );

        if let Ok(report) = self
            .ports
            .proposal_cache
            .apply_pending_signatures(deposit_id, |hash, sig| {
                verify_bridge_signature(hash, sig, &self.node.valid_addresses)
            })
        {
            if report.applied > 0 {
                debug!(
                    target: "bridge.cursor",
                    proposal_hash=%proposal_id,
                    applied_count=report.applied,
                    "applied pending signatures from peers"
                );
            }
            if let Some(first) = report.mismatched.first() {
                let deposit_id_hex = hex::encode(deposit_id.to_bytes());
                let expected_hex = hex::encode(first.expected_hash);
                let received_hex = hex::encode(first.received_hash);
                warn!(
                    target: "bridge.cursor",
                    deposit_id=%deposit_id_hex,
                    expected_hash=%expected_hex,
                    received_hash=%received_hex,
                    signer=%first.signer_address,
                    mismatch_count=report.mismatched.len(),
                    "peer signature proposal hash mismatch, possible nonce divergence"
                );
                self.control.bridge_status.push_alert(
                    crate::tui::types::AlertSeverity::Error,
                    "Nonce Divergence Suspected".to_string(),
                    format!(
                        "Deposit {} has {} peer signature(s) for a different proposal hash. expected={}, received={}, signer={}",
                        deposit_id_hex,
                        report.mismatched.len(),
                        expected_hex,
                        received_hex,
                        first.signer_address
                    ),
                    "nonce-divergence".to_string(),
                );
            }
        }

        if let Ok(Some(proposal_state)) = self.ports.proposal_cache.get_state(deposit_id) {
            self.control
                .bridge_status
                .sync_proposal_signatures_from_cache(
                    &proposal_id, &proposal_state, &self.node.address_to_node_id,
                    self.node.self_node_id,
                );
        }

        match add_result {
            Ok(crate::proposal_cache::SignatureAddResult::Added)
            | Ok(crate::proposal_cache::SignatureAddResult::ThresholdReached) => {}
            Ok(crate::proposal_cache::SignatureAddResult::Duplicate) => {
                debug!(
                    target: "bridge.cursor",
                    proposal_hash=%proposal_id,
                    "duplicate signature, skipping broadcast"
                );
                return SigningCandidateExecutionResult::DuplicateSignature;
            }
            Ok(crate::proposal_cache::SignatureAddResult::Stale) => {
                info!(
                    target: "bridge.cursor",
                    proposal_hash=%proposal_id,
                    "proposal already confirmed, skipping broadcast"
                );
                return SigningCandidateExecutionResult::ProposalStale;
            }
            Ok(crate::proposal_cache::SignatureAddResult::Invalid(msg)) => {
                warn!(
                    target: "bridge.cursor",
                    proposal_hash=%proposal_id,
                    error=%msg,
                    "own signature invalid, skipping broadcast"
                );
                return SigningCandidateExecutionResult::InvalidOwnSignature;
            }
            Err(e) => {
                warn!(
                    target: "bridge.cursor",
                    proposal_hash=%proposal_id,
                    error=%e,
                    "failed to add own signature to cache, skipping broadcast"
                );
                return SigningCandidateExecutionResult::CacheUpdateFailed;
            }
        }

        // Step 3: Broadcast signature to all peers (fire-and-forget, concurrent).
        let broadcast_msg = SignatureBroadcast {
            deposit_id: deposit_id.to_bytes(),
            proposal_hash: proposal_hash.to_vec(),
            signature,
            signer_address: my_eth_address.as_slice().to_vec(),
            timestamp: now_secs,
        };

        spawn_signature_broadcast(
            self.node.peers.as_ref(),
            &broadcast_msg,
            &proposal_id,
            SignatureBroadcastReason::Initial,
        );
        SigningCandidateExecutionResult::Broadcasted
    }

    fn execute_regossip(&self, now_secs: u64) -> usize {
        use tracing::{debug, warn};

        use crate::ingress::proto::SignatureBroadcast;

        let my_eth_address = self.node.signer.address();
        let mut broadcasts = 0;
        match self.ports.proposal_cache.collecting_with_my_sig() {
            Ok(pending) => {
                for (deposit_id, proposal_state) in pending {
                    let Some(sig) = proposal_state.my_signature.clone() else {
                        continue;
                    };

                    let broadcast_msg = SignatureBroadcast {
                        deposit_id: deposit_id.to_bytes(),
                        proposal_hash: proposal_state.proposal_hash.to_vec(),
                        signature: sig,
                        signer_address: my_eth_address.as_slice().to_vec(),
                        timestamp: now_secs,
                    };

                    let prop_id = hex::encode(proposal_state.proposal_hash);
                    spawn_signature_broadcast(
                        self.node.peers.as_ref(),
                        &broadcast_msg,
                        &prop_id,
                        SignatureBroadcastReason::Regossip,
                    );
                    broadcasts += 1;
                }
            }
            Err(err) => {
                warn!(
                    target: "bridge.cursor",
                    error=%err,
                    "failed to gather proposals for signature re-gossip"
                );
            }
        }
        if broadcasts > 0 {
            debug!(
                target: "bridge.cursor",
                regossip_broadcasts=broadcasts,
                "re-gossiped collecting signatures"
            );
        }
        broadcasts
    }
}

impl<B: BaseContractPort> SigningTickContext<B> {
    async fn trigger_local_stop(&self, reason: String) {
        use std::time::SystemTime;

        use tracing::info;

        use crate::stop::{trigger_local_stop, StopInfo, StopSource};
        use crate::tui::types::AlertSeverity;

        info!(
            target: "bridge.cursor",
            mode = ?self.control.local_stop_mode,
            reason=%reason,
            "signing requested local stop"
        );

        match self.control.local_stop_mode {
            SigningLocalStopMode::RuntimeProbeAndBroadcast => {
                trigger_local_stop(
                    self.ports.runtime.clone(),
                    self.control.stop_controller.clone(),
                    self.control.bridge_status.clone(),
                    reason,
                )
                .await;
            }
            SigningLocalStopMode::LocalTriggerOnly => {
                let metrics = crate::metrics::init_metrics();
                metrics.stop_local_requests.increment();

                let info = StopInfo {
                    reason: reason.clone(),
                    last: None,
                    source: StopSource::Local,
                    at: SystemTime::now(),
                };
                if !self.control.stop_controller.trigger(info) {
                    metrics.stop_local_duplicate.increment();
                    info!(
                        target: "bridge.cursor",
                        reason=%reason,
                        "local stop already active, skipping duplicate local-only trigger"
                    );
                    return;
                }
                metrics.stop_local_triggered.increment();
                info!(
                    target: "bridge.cursor",
                    reason=%reason,
                    "local-only stop activated"
                );
                self.control.bridge_status.push_alert(
                    AlertSeverity::Error,
                    "Bridge Stopped".to_string(),
                    reason,
                    "local-stop".to_string(),
                );
            }
        }
    }

    pub async fn tick_once(
        &self,
        state: &mut SigningTickState,
        input: SigningTickInput,
    ) -> SigningTickOutcome {
        use tracing::{debug, info, warn};

        use crate::types::{DepositId, NockDepositRequestData};

        let context = self;
        let mut outcome = SigningTickOutcome::default();
        let now = input.now;
        let now_secs = system_time_secs(now);

        // Periodically re-gossip our own signatures for deposits still collecting.
        if now
            .duration_since(state.last_regossip_at)
            .unwrap_or_default()
            >= context.config.policy.regossip_interval
        {
            outcome.regossip_broadcasts += context.execute_regossip(now_secs);
            state.last_regossip_at = now;
        }

        // Always poll chain nonce for health/TUI visibility, even before tip/epoch gates.
        let last_chain_nonce = match context.ports.base_bridge.get_last_deposit_nonce().await {
            Ok(nonce) => {
                context
                    .control
                    .bridge_status
                    .update_last_deposit_nonce(nonce);
                Some(nonce)
            }
            Err(e) => {
                warn!(
                    target: "bridge.cursor",
                    error=%e,
                    "failed to query lastDepositNonce from chain"
                );
                None
            }
        };

        let nonce_epoch_base = context.config.nonce_epoch.base;
        let first_epoch_nonce = context.config.nonce_epoch.first_epoch_nonce();
        let mut epoch_ready_logged = false;
        let mut maybe_mark_epoch_ready = |tip_height: u64, state: &mut SigningTickState| {
            if epoch_ready_logged {
                return;
            }
            state.logged_epoch_ready = true;
            epoch_ready_logged = true;
            info!(
                target: "bridge.cursor",
                tip_height,
                nonce_epoch_start_height = context.config.nonce_epoch.start_height,
                "hashchain reached nonce epoch start height, signing enabled"
            );
        };
        let mut plan = SigningPlanner::plan_tick(SigningTickPlanInput {
            tip_height: input.tip_height,
            nonce_epoch_start_height: context.config.nonce_epoch.start_height,
            logged_epoch_ready: state.logged_epoch_ready,
            last_chain_nonce,
            nonce_epoch_base,
            first_epoch_nonce,
            log_len: None,
        });
        let next_nonce = loop {
            match plan {
                SigningTickPlanAction::WaitForTip => {
                    debug!(
                        target: "bridge.cursor",
                        "no nock hashchain tip yet, waiting before signing"
                    );
                    return outcome;
                }
                SigningTickPlanAction::WaitForEpochStart { tip_height } => {
                    debug!(
                        target: "bridge.cursor",
                        tip_height,
                        nonce_epoch_start_height = context.config.nonce_epoch.start_height,
                        "hashchain behind nonce epoch start height, waiting to sign"
                    );
                    return outcome;
                }
                SigningTickPlanAction::NeedLastChainNonce {
                    tip_height,
                    reached_epoch_start,
                } => {
                    if reached_epoch_start {
                        maybe_mark_epoch_ready(tip_height, state);
                    }
                    return outcome;
                }
                SigningTickPlanAction::StopNonceEpochMismatch {
                    tip_height,
                    reached_epoch_start,
                    last_chain_nonce,
                    nonce_epoch_base,
                } => {
                    if reached_epoch_start {
                        maybe_mark_epoch_ready(tip_height, state);
                    }
                    let reason = format!(
                        "nonce epoch mismatch: nonce_epoch_base ({nonce_epoch_base}) is greater than on-chain lastDepositNonce ({last_chain_nonce}); check config"
                    );
                    context.trigger_local_stop(reason).await;
                    return outcome;
                }
                SigningTickPlanAction::NeedLogLen {
                    tip_height,
                    reached_epoch_start,
                    last_chain_nonce,
                } => {
                    if reached_epoch_start {
                        maybe_mark_epoch_ready(tip_height, state);
                    }

                    let log_len = match context
                        .ports
                        .deposit_log
                        .number_of_deposits_in_epoch(&context.config.nonce_epoch)
                        .await
                    {
                        Ok(v) => v,
                        Err(err) => {
                            warn!(
                                target: "bridge.cursor",
                                error=%err,
                                "failed to count deposits in sqlite"
                            );
                            let reason = format!("failed to count deposits in sqlite: {err}");
                            context.trigger_local_stop(reason).await;
                            return outcome;
                        }
                    };

                    plan = SigningPlanner::plan_tick(SigningTickPlanInput {
                        tip_height: input.tip_height,
                        nonce_epoch_start_height: context.config.nonce_epoch.start_height,
                        logged_epoch_ready: state.logged_epoch_ready,
                        last_chain_nonce: Some(last_chain_nonce),
                        nonce_epoch_base,
                        first_epoch_nonce,
                        log_len: Some(log_len),
                    });
                }
                SigningTickPlanAction::WaitForLogCatchup {
                    tip_height,
                    reached_epoch_start,
                    nonce_epoch_base,
                    log_len,
                    spent_epoch_nonces,
                    ..
                } => {
                    if reached_epoch_start {
                        maybe_mark_epoch_ready(tip_height, state);
                    }
                    debug!(
                        target: "bridge.cursor",
                        log_len,
                        spent_epoch_nonces,
                        nonce_epoch_base,
                        "deposit log behind chain prefix, waiting for log to catch up"
                    );
                    return outcome;
                }
                SigningTickPlanAction::Continue {
                    tip_height,
                    reached_epoch_start,
                    next_nonce,
                    ..
                } => {
                    if reached_epoch_start {
                        maybe_mark_epoch_ready(tip_height, state);
                    }
                    break next_nonce;
                }
            }
        };

        let candidates = match context
            .ports
            .deposit_log
            .records_from_nonce(
                next_nonce, context.config.policy.pipeline_depth, &context.config.nonce_epoch,
            )
            .await
        {
            Ok(v) => v,
            Err(err) => {
                warn!(
                    target: "bridge.cursor",
                    error=%err,
                    "failed to query candidate deposits from sqlite"
                );
                return outcome;
            }
        };

        if candidates.is_empty() {
            return outcome;
        }

        // Sign and gossip signatures for the tip candidate (and optional pipeline).
        for (nonce, record) in candidates {
            if context.control.stop.is_stopped() {
                break;
            }

            if matches!(
                SigningPlanner::plan_epoch_bounds(SigningEpochBoundsDecisionInput {
                    is_before_start_key: context
                        .config
                        .nonce_epoch
                        .is_before_start_key(record.block_height, &record.tx_id),
                }),
                SigningEpochBoundsDecision::StopRecordBeforeStart
            ) {
                let reason = format!(
                    "signing candidate is before nonce_epoch start key (record_height={}, start_height={}); candidate should have been filtered before signing",
                    record.block_height, context.config.nonce_epoch.start_height
                );
                context.trigger_local_stop(reason).await;
                break;
            }

            let req = NockDepositRequestData {
                tx_id: record.tx_id.clone(),
                name: record.name.clone(),
                recipient: record.recipient,
                amount: record.amount_to_mint,
                block_height: record.block_height,
                as_of: record.as_of.clone(),
                nonce,
            };

            let deposit_id = DepositId::from_effect_payload(&req);

            let existing_state = context
                .ports
                .proposal_cache
                .get_state(&deposit_id)
                .ok()
                .flatten();
            let precheck = SigningPlanner::plan_candidate_precheck(SigningCandidatePrecheckInput {
                is_confirmed: existing_state
                    .as_ref()
                    .map(|proposal_state| {
                        proposal_state.status == crate::proposal_cache::ProposalStatus::Confirmed
                    })
                    .unwrap_or(false),
                has_my_signature: existing_state
                    .as_ref()
                    .and_then(|proposal_state| proposal_state.my_signature.as_ref())
                    .is_some(),
            });
            match precheck {
                SigningCandidatePrecheckDecision::SkipConfirmed
                | SigningCandidatePrecheckDecision::SkipAlreadySigned => {
                    continue;
                }
                SigningCandidatePrecheckDecision::CheckProcessedOnChain => {}
            }

            // Optimization: skip signing if deposit is already processed on-chain.
            // Do not block signing on transient Base RPC errors.
            match context
                .ports
                .base_bridge
                .is_deposit_processed(&req.tx_id)
                .await
            {
                Ok(processed_on_chain) => {
                    match SigningPlanner::plan_processed(SigningProcessedDecisionInput {
                        processed_on_chain,
                    }) {
                        SigningProcessedDecision::SkipProcessed => {
                            debug!(
                                target: "bridge.cursor",
                                nonce,
                                "deposit already processed on-chain, skipping signature"
                            );
                            continue;
                        }
                        SigningProcessedDecision::ContinueSign => {}
                    }
                }
                Err(e) => {
                    warn!(
                        target: "bridge.cursor",
                        nonce=req.nonce,
                        error=%e,
                        "failed to query processedDeposits, proceeding to sign anyway"
                    );
                }
            }

            if matches!(
                context
                    .execute_signing_candidate(&req, &deposit_id, now, now_secs)
                    .await,
                SigningCandidateExecutionResult::Broadcasted
            ) {
                outcome.initial_broadcasts += 1;
            }
        }

        outcome
    }
}

pub async fn signing_tick_once<B: BaseContractPort>(
    context: &SigningTickContext<B>,
    state: &mut SigningTickState,
    input: SigningTickInput,
) -> SigningTickOutcome {
    context.tick_once(state, input).await
}

/// Background loop that deterministically selects deposits to sign from shared history + chain tip.
///
/// This decouples signing from `%commit-nock-deposits` effects so nodes can restart at different
/// nock heights and still converge on the same `lastDepositNonce + 1` deposit for signing.
#[allow(clippy::too_many_arguments)]
pub async fn run_signing_cursor_loop<B: BaseContractPort>(
    runtime: Arc<BridgeRuntimeHandle>,
    base_bridge: Arc<B>,
    deposit_log: Arc<crate::deposit_log::DepositLog>,
    nonce_epoch: &NonceEpochConfig,
    proposal_cache: Arc<ProposalCache>,
    signer: Arc<BridgeSigner>,
    valid_addresses: HashSet<Address>,
    peers: Vec<PeerEndpoint>,
    self_node_id: u64,
    bridge_status: BridgeStatus,
    address_to_node_id: std::collections::HashMap<Address, u64>,
    stop_controller: crate::stop::StopController,
    stop: crate::stop::StopHandle,
) {
    run_signing_cursor_loop_with_policy(
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
        SigningLoopPolicy::default(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_signing_cursor_loop_with_policy<B: BaseContractPort>(
    runtime: Arc<BridgeRuntimeHandle>,
    base_bridge: Arc<B>,
    deposit_log: Arc<crate::deposit_log::DepositLog>,
    nonce_epoch: &NonceEpochConfig,
    proposal_cache: Arc<ProposalCache>,
    signer: Arc<BridgeSigner>,
    valid_addresses: HashSet<Address>,
    peers: Vec<PeerEndpoint>,
    self_node_id: u64,
    bridge_status: BridgeStatus,
    address_to_node_id: std::collections::HashMap<Address, u64>,
    stop_controller: crate::stop::StopController,
    stop: crate::stop::StopHandle,
    policy: SigningLoopPolicy,
) {
    use tokio::time::{interval, MissedTickBehavior};
    use tracing::{info, warn};

    info!(
        target: "bridge.cursor",
        poll_interval_secs=policy.poll_interval.as_secs(),
        pipeline_depth=policy.pipeline_depth,
        regossip_interval_secs=policy.regossip_interval.as_secs(),
        "starting signing cursor loop"
    );

    let context = SigningTickContext::new(
        SigningTickPorts::new(runtime.clone(), base_bridge, deposit_log, proposal_cache),
        SigningTickNodeState::new(
            signer, valid_addresses, peers, self_node_id, address_to_node_id,
        ),
        SigningTickControl::new(bridge_status, stop_controller, stop.clone()),
        SigningTickConfig::new(nonce_epoch, policy),
    );

    let mut ticker = interval(policy.poll_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut state = SigningTickState::new(SystemTime::now());

    loop {
        ticker.tick().await;

        if stop.is_stopped() {
            continue;
        }

        let tip_height = match runtime.nock_hashchain_tip().await {
            Ok(height) => height,
            Err(err) => {
                warn!(
                    target: "bridge.cursor",
                    error=%err,
                    "failed to peek nock hashchain tip height"
                );
                continue;
            }
        };
        let _ = signing_tick_once(
            &context,
            &mut state,
            SigningTickInput {
                now: SystemTime::now(),
                tip_height,
            },
        )
        .await;
    }
}

fn update_proposal_cache_metrics(proposal_cache: &ProposalCache) {
    let metrics = metrics::init_metrics();
    let snapshot = match proposal_cache.metrics_snapshot() {
        Ok(snapshot) => snapshot,
        Err(_) => {
            metrics.proposal_cache_metrics_update_error.increment();
            return;
        }
    };

    metrics
        .proposal_cache_total
        .swap(snapshot.proposal_total as f64);
    metrics
        .proposal_cache_collecting
        .swap(snapshot.collecting as f64);
    metrics.proposal_cache_ready.swap(snapshot.ready as f64);
    metrics.proposal_cache_posting.swap(snapshot.posting as f64);
    metrics
        .proposal_cache_confirmed
        .swap(snapshot.confirmed as f64);
    metrics.proposal_cache_failed.swap(snapshot.failed as f64);
    metrics
        .proposal_cache_total_peer_signatures
        .swap(snapshot.total_peer_signatures as f64);
    metrics
        .proposal_cache_max_peer_signatures_per_proposal
        .swap(snapshot.max_peer_signatures_per_proposal as f64);
    metrics
        .proposal_cache_proposals_with_my_signature
        .swap(snapshot.proposals_with_my_signature as f64);
    metrics
        .proposal_cache_pending_signature_deposit_count
        .swap(snapshot.pending_signature_deposit_count as f64);
    metrics
        .proposal_cache_pending_signature_total
        .swap(snapshot.pending_signature_total as f64);
    metrics
        .proposal_cache_oldest_age_secs
        .swap(snapshot.oldest_age_secs as f64);
    metrics
        .proposal_cache_oldest_confirmed_age_secs
        .swap(snapshot.oldest_confirmed_age_secs as f64);
    metrics
        .proposal_cache_oldest_failed_age_secs
        .swap(snapshot.oldest_failed_age_secs as f64);
    metrics
        .proposal_cache_pending_oldest_age_secs
        .swap(snapshot.pending_oldest_age_secs as f64);
    metrics
        .proposal_cache_approx_state_bytes
        .swap(snapshot.approx_state_bytes as f64);
    metrics
        .proposal_cache_approx_peer_signature_bytes
        .swap(snapshot.approx_peer_signature_bytes as f64);
    metrics
        .proposal_cache_approx_my_signature_bytes
        .swap(snapshot.approx_my_signature_bytes as f64);
    metrics
        .proposal_cache_approx_pending_signature_bytes
        .swap(snapshot.approx_pending_signature_bytes as f64);
    metrics
        .proposal_cache_approx_total_bytes
        .swap(snapshot.approx_total_bytes as f64);
}

/// Background loop that checks ProposalCache for ready proposals and posts to BASE.
///
/// Runs continuously, checking every second for proposals with threshold signatures.
/// Posts to Base when:
/// 1. Threshold reached (status=Ready)
/// 2. I'm the proposer OR backoff expired (failover logic)
///
fn spawn_confirmation_broadcast(
    peers: &[(u64, String)],
    msg: &crate::ingress::proto::ConfirmationBroadcast,
    proposal_id: &str,
) {
    use tracing::{info, warn};

    use crate::ingress::proto::bridge_ingress_client::BridgeIngressClient;

    for (peer_node_id, peer_address) in peers {
        let msg = msg.clone();
        let addr = peer_address.clone();
        let peer_id = *peer_node_id;
        let prop_id = proposal_id.to_string();

        tokio::spawn(async move {
            match BridgeIngressClient::connect(addr.clone()).await {
                Ok(mut client) => match client.broadcast_confirmation(msg).await {
                    Ok(_) => {
                        info!(
                            target: "bridge.posting",
                            peer_node_id=peer_id,
                            proposal_hash=%prop_id,
                            "broadcast confirmation to peer"
                        );
                    }
                    Err(e) => {
                        warn!(
                            target: "bridge.posting",
                            peer_node_id=peer_id,
                            error=%e,
                            "failed to broadcast confirmation to peer"
                        );
                    }
                },
                Err(e) => {
                    warn!(
                        target: "bridge.posting",
                        peer_node_id=peer_id,
                        peer_address=%addr,
                        error=%e,
                        "failed to connect to peer for confirmation broadcast"
                    );
                }
            }
        });
    }
}

#[derive(Clone, Debug, Default)]
pub struct PostingTickState {
    pub ticks_executed: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct PostingTickInput {
    pub now: SystemTime,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PostingTickOutcome {
    pub ready_proposals: usize,
    pub submitted: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PostingSubmissionExecutionResult {
    Submitted,
    ProposalNoLongerReady,
    SignatureFetchFailed,
    MarkPostingFailed,
    SubmitFailed,
    SubmitTimedOut,
}

#[derive(Clone)]
pub struct PostingTickPorts<B: BaseContractPort> {
    proposal_cache: Arc<ProposalCache>,
    base_bridge: Arc<B>,
}

impl<B: BaseContractPort> PostingTickPorts<B> {
    pub fn new(proposal_cache: Arc<ProposalCache>, base_bridge: Arc<B>) -> Self {
        Self {
            proposal_cache,
            base_bridge,
        }
    }
}

#[derive(Clone)]
pub struct PostingTickNodeState {
    node_config: NodeConfig,
    peers: Arc<Vec<(u64, String)>>,
    my_node_id: usize,
}

impl PostingTickNodeState {
    pub fn new(node_config: NodeConfig) -> Self {
        let my_node_id = node_config.node_id as usize;
        let peers: Vec<(u64, String)> = node_config
            .nodes
            .iter()
            .enumerate()
            .filter(|(idx, _)| *idx != my_node_id)
            .map(|(idx, node)| (idx as u64, crate::health::normalize_endpoint(&node.ip)))
            .collect();
        Self {
            node_config,
            peers: Arc::new(peers),
            my_node_id,
        }
    }
}

#[derive(Clone)]
pub struct PostingTickControl {
    bridge_status: BridgeStatus,
    status_state: crate::status::BridgeStatusState,
}

impl PostingTickControl {
    pub fn new(
        bridge_status: BridgeStatus,
        status_state: crate::status::BridgeStatusState,
    ) -> Self {
        Self {
            bridge_status,
            status_state,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PostingTickConfig {
    failover_backoff_secs: u64,
}

impl PostingTickConfig {
    pub fn new(failover_backoff_secs: u64) -> Self {
        Self {
            failover_backoff_secs,
        }
    }
}

#[derive(Clone)]
pub struct PostingTickContext<B: BaseContractPort> {
    ports: PostingTickPorts<B>,
    node: PostingTickNodeState,
    control: PostingTickControl,
    config: PostingTickConfig,
}

impl<B: BaseContractPort> PostingTickContext<B> {
    pub fn new(
        ports: PostingTickPorts<B>,
        node: PostingTickNodeState,
        control: PostingTickControl,
        config: PostingTickConfig,
    ) -> Self {
        Self {
            ports,
            node,
            control,
            config,
        }
    }

    async fn execute_submission(
        &self,
        deposit_id: &crate::types::DepositId,
        proposal_state: &crate::proposal_cache::ProposalState,
        proposal_hash: [u8; 32],
        proposal_id: &str,
        now: SystemTime,
        now_secs: u64,
    ) -> PostingSubmissionExecutionResult {
        use serde_bytes::ByteBuf;
        use tracing::{error, info, warn};

        use crate::ingress::proto::ConfirmationBroadcast;
        use crate::status::LastSubmittedDeposit;
        use crate::tui::types::{AlertSeverity, BatchStatus, ProposalStatus};
        use crate::types::DepositSubmission;

        // Get signatures for posting BEFORE marking as posting
        // (get_signatures_for_posting requires status == Ready).
        let signatures = match self
            .ports
            .proposal_cache
            .get_signatures_for_posting(deposit_id)
        {
            Ok(Some(sigs)) => sigs,
            Ok(None) => {
                warn!(
                    target: "bridge.posting",
                    proposal_hash=%proposal_id,
                    "proposal no longer ready for posting"
                );
                return PostingSubmissionExecutionResult::ProposalNoLongerReady;
            }
            Err(e) => {
                error!(
                    target: "bridge.posting",
                    error=%e,
                    proposal_hash=%proposal_id,
                    "failed to get signatures for posting"
                );
                let _ = self.ports.proposal_cache.mark_failed(deposit_id);
                return PostingSubmissionExecutionResult::SignatureFetchFailed;
            }
        };

        // Mark as posting to prevent duplicate submissions.
        if let Err(e) = self.ports.proposal_cache.mark_posting(deposit_id) {
            error!(
                target: "bridge.posting",
                error=%e,
                proposal_hash=%proposal_id,
                "failed to mark proposal as posting"
            );
            return PostingSubmissionExecutionResult::MarkPostingFailed;
        }

        // Update batch status to Submitting.
        self.control
            .bridge_status
            .update_batch_status(BatchStatus::Submitting {
                batch_id: proposal_state.proposal.nonce,
            });

        info!(
            target: "bridge.posting",
            proposal_hash=%proposal_id,
            "posting proposal to BASE"
        );

        // Prepare deposit submission.
        let req = &proposal_state.proposal;
        let mut recipient_bytes = [0u8; 20];
        recipient_bytes.copy_from_slice(&req.recipient.0);

        let submission = DepositSubmission {
            tx_id: req.tx_id.clone(),
            name_first: req.name.first.clone(),
            name_last: req.name.last.clone(),
            recipient: recipient_bytes,
            amount: req.amount as u128,
            block_height: req.block_height,
            as_of: req.as_of.clone(),
            nonce: req.nonce,
            signatures: crate::types::SignatureSet {
                eth_signatures: signatures.into_iter().map(ByteBuf::from).collect(),
                nock_signatures: vec![], // Not used for Base deposits
            },
        };

        // Update TUI to Submitted status with timestamp.
        if let Some(mut proposal) = self.control.bridge_status.find_proposal(proposal_id) {
            proposal.status = ProposalStatus::Submitted;
            proposal.submitted_at = Some(now);
            if let Ok(duration) = now.duration_since(proposal.created_at) {
                proposal.time_to_submit_ms = Some(duration.as_millis() as u64);
            }
            self.control.bridge_status.update_proposal(proposal);
        }

        // Submit to BASE with a timeout so a hung RPC can't stall the queue.
        match tokio::time::timeout(
            Duration::from_secs(SUBMIT_DEPOSIT_TIMEOUT_SECS),
            self.ports.base_bridge.submit_deposit(submission),
        )
        .await
        {
            Ok(Ok(result)) => {
                info!(
                    target: "bridge.posting",
                    proposal_hash=%proposal_id,
                    tx_hash=%result.tx_hash,
                    block_number=%result.block_number,
                    "successfully posted deposit to BASE"
                );

                self.control
                    .status_state
                    .update_last_submitted_deposit(LastSubmittedDeposit {
                        deposit: proposal_state.proposal.clone(),
                        base_tx_hash: result.tx_hash.clone(),
                        base_block_number: result.block_number,
                    });

                // Mark confirmed in cache.
                let _ = self.ports.proposal_cache.mark_confirmed(deposit_id);

                // Broadcast confirmation to all peers so they stop waiting.
                let confirmation_msg = ConfirmationBroadcast {
                    deposit_id: deposit_id.to_bytes(),
                    proposal_hash: proposal_hash.to_vec(),
                    tx_hash: result.tx_hash.as_bytes().to_vec(),
                    block_number: result.block_number,
                    timestamp: now_secs,
                };
                spawn_confirmation_broadcast(
                    self.node.peers.as_ref(),
                    &confirmation_msg,
                    proposal_id,
                );

                // Update TUI to Executed status.
                if let Some(mut proposal) = self.control.bridge_status.find_proposal(proposal_id) {
                    proposal.status = ProposalStatus::Executed;
                    proposal.tx_hash = Some(result.tx_hash);
                    proposal.submitted_at_block = Some(result.block_number);
                    proposal.executed_at_block = Some(result.block_number);
                    self.control.bridge_status.update_proposal(proposal);
                }

                // Update batch status back to Idle after successful submission.
                self.control
                    .bridge_status
                    .update_batch_status(BatchStatus::Idle);
                PostingSubmissionExecutionResult::Submitted
            }
            Ok(Err(e)) => {
                error!(
                    target: "bridge.posting",
                    error=%e,
                    proposal_hash=%proposal_id,
                    "failed to post deposit to BASE"
                );

                // Mark failed in cache.
                let _ = self.ports.proposal_cache.mark_failed(deposit_id);

                // Update TUI to Failed status.
                if let Some(mut proposal) = self.control.bridge_status.find_proposal(proposal_id) {
                    proposal.status = ProposalStatus::Failed {
                        reason: format!("BASE submission failed: {}", e),
                    };
                    self.control.bridge_status.update_proposal(proposal);
                }

                // Push alert for failure.
                self.control.bridge_status.push_alert(
                    AlertSeverity::Error,
                    "Proposal Failed".to_string(),
                    format!("Failed to post deposit {}: {}", proposal_id, e),
                    "posting-loop".to_string(),
                );

                // Update batch status back to Idle after failure.
                self.control
                    .bridge_status
                    .update_batch_status(BatchStatus::Idle);
                PostingSubmissionExecutionResult::SubmitFailed
            }
            Err(_) => {
                error!(
                    target: "bridge.posting",
                    proposal_hash=%proposal_id,
                    timeout_secs=SUBMIT_DEPOSIT_TIMEOUT_SECS,
                    "posting to BASE timed out"
                );

                // Mark failed in cache.
                let _ = self.ports.proposal_cache.mark_failed(deposit_id);

                // Update TUI to Failed status.
                if let Some(mut proposal) = self.control.bridge_status.find_proposal(proposal_id) {
                    proposal.status = ProposalStatus::Failed {
                        reason: format!(
                            "BASE submission timed out after {}s",
                            SUBMIT_DEPOSIT_TIMEOUT_SECS
                        ),
                    };
                    self.control.bridge_status.update_proposal(proposal);
                }

                // Push alert for failure.
                self.control.bridge_status.push_alert(
                    AlertSeverity::Error,
                    "Proposal Failed".to_string(),
                    format!(
                        "Failed to post deposit {}: timed out after {}s",
                        proposal_id, SUBMIT_DEPOSIT_TIMEOUT_SECS
                    ),
                    "posting-loop".to_string(),
                );

                // Update batch status back to Idle after timeout.
                self.control
                    .bridge_status
                    .update_batch_status(BatchStatus::Idle);
                PostingSubmissionExecutionResult::SubmitTimedOut
            }
        }
    }
}

impl<B: BaseContractPort> PostingTickContext<B> {
    pub async fn tick_once(
        &self,
        state: &mut PostingTickState,
        input: PostingTickInput,
    ) -> PostingTickOutcome {
        use tracing::{debug, error, info};

        use crate::proposer::hoon_proposer;

        let context = self;
        state.ticks_executed = state.ticks_executed.saturating_add(1);
        let mut outcome = PostingTickOutcome::default();

        // Get all ready proposals
        let ready_proposals = match context.ports.proposal_cache.ready_proposals() {
            Ok(proposals) => proposals,
            Err(e) => {
                error!(target: "bridge.posting", error=%e, "failed to fetch ready proposals");
                return outcome;
            }
        };

        if ready_proposals.is_empty() {
            return outcome;
        }
        outcome.ready_proposals = ready_proposals.len();

        // Query the chain for the last confirmed deposit nonce.
        // This is the source of truth - we only submit lastDepositNonce + 1.
        let last_chain_nonce = match context.ports.base_bridge.get_last_deposit_nonce().await {
            Ok(n) => n,
            Err(e) => {
                error!(target: "bridge.posting", error=%e, "failed to query lastDepositNonce from chain");
                return outcome;
            }
        };
        context
            .control
            .bridge_status
            .update_last_deposit_nonce(last_chain_nonce);
        let next_nonce = last_chain_nonce + 1;

        debug!(
            target: "bridge.posting",
            last_chain_nonce=last_chain_nonce,
            next_nonce=next_nonce,
            "queried chain for deposit nonce"
        );

        // NOTE: do not "skip" a stuck nonce. Under runtime-assigned epoch nonces, skipping
        // strands deposits permanently because subsequent nonces cannot be posted until the
        // contract advances. We only ever submit `lastDepositNonce + 1`.
        let now_secs = system_time_secs(input.now);

        // Get current nockchain height for proposer calculation.
        // Use the block_height from the first proposal as a proxy
        // (all proposals in a batch should be from the same height).
        let current_height = ready_proposals
            .first()
            .map(|(_, proposal_state)| proposal_state.proposal.block_height)
            .unwrap_or(0);
        let node_pkhs: Vec<_> = context
            .node
            .node_config
            .nodes
            .iter()
            .map(|node| node.nock_pkh.clone())
            .collect();
        let num_nodes = node_pkhs.len();
        let current_proposer = hoon_proposer(current_height, &node_pkhs);

        debug!(
            target: "bridge.posting",
            ready_count=ready_proposals.len(),
            current_height=current_height,
            current_proposer=current_proposer,
            my_node_id=context.node.my_node_id,
            "checking ready proposals"
        );

        let decisions = PostingPlanner::plan_tick(
            PostingTickPlanInput {
                next_nonce,
                my_node_id: context.node.my_node_id,
                current_proposer,
                num_nodes,
                now_secs,
                failover_backoff_secs: context.config.failover_backoff_secs,
            },
            &ready_proposals
                .iter()
                .map(|(_, proposal_state)| PostingReadyProposal {
                    nonce: proposal_state.proposal.nonce,
                    ready_at: proposal_state.ready_at,
                })
                .collect::<Vec<_>>(),
        );

        for ((deposit_id, proposal_state), decision) in ready_proposals.into_iter().zip(decisions) {
            let proposal_hash = proposal_state.proposal_hash;
            let proposal_id = hex::encode(proposal_hash);

            let is_proposer = match decision {
                PostingCandidateDecision::MarkConfirmedOnChain => {
                    debug!(
                            target: "bridge.posting",
                            proposal_hash=%proposal_id,
                        nonce=proposal_state.proposal.nonce,
                        last_chain_nonce=last_chain_nonce,
                        "proposal already confirmed on chain, marking confirmed"
                    );
                    let _ = context.ports.proposal_cache.mark_confirmed(&deposit_id);
                    continue;
                }
                PostingCandidateDecision::WaitForEarlierNonce => {
                    debug!(
                        target: "bridge.posting",
                        proposal_hash=%proposal_id,
                        nonce=proposal_state.proposal.nonce,
                        next_nonce=next_nonce,
                        "waiting for nonce {} to be ready before posting {}",
                        next_nonce,
                        proposal_state.proposal.nonce
                    );
                    continue;
                }
                PostingCandidateDecision::NotMyTurn => {
                    debug!(
                        target: "bridge.posting",
                        proposal_hash=%proposal_id,
                        current_proposer=current_proposer,
                        my_node_id=context.node.my_node_id,
                        "not my turn to post, waiting for proposer or failover"
                    );
                    continue;
                }
                PostingCandidateDecision::Submit { is_proposer } => is_proposer,
            };

            info!(
                target: "bridge.posting",
                proposal_hash=%proposal_id,
                current_proposer=current_proposer,
                my_node_id=context.node.my_node_id,
                is_proposer=is_proposer,
                "posting proposal to BASE"
            );

            if matches!(
                context
                    .execute_submission(
                        &deposit_id, &proposal_state, proposal_hash, &proposal_id, input.now,
                        now_secs,
                    )
                    .await,
                PostingSubmissionExecutionResult::Submitted
            ) {
                outcome.submitted += 1;
            }
        }

        outcome
    }
}

pub async fn posting_tick_once<B: BaseContractPort>(
    context: &PostingTickContext<B>,
    state: &mut PostingTickState,
    input: PostingTickInput,
) -> PostingTickOutcome {
    context.tick_once(state, input).await
}

pub async fn run_posting_loop<B: BaseContractPort>(
    proposal_cache: Arc<ProposalCache>,
    base_bridge: Arc<B>,
    node_config: NodeConfig,
    bridge_status: BridgeStatus,
    stop: crate::stop::StopHandle,
    status_state: crate::status::BridgeStatusState,
) {
    run_posting_loop_with_policy(
        proposal_cache,
        base_bridge,
        node_config,
        bridge_status,
        stop,
        status_state,
        PostingLoopPolicy::default(),
    )
    .await
}

pub async fn run_posting_loop_with_policy<B: BaseContractPort>(
    proposal_cache: Arc<ProposalCache>,
    base_bridge: Arc<B>,
    node_config: NodeConfig,
    bridge_status: BridgeStatus,
    stop: crate::stop::StopHandle,
    status_state: crate::status::BridgeStatusState,
    policy: PostingLoopPolicy,
) {
    use tracing::info;

    info!("Starting proposal posting loop");
    let context = PostingTickContext::new(
        PostingTickPorts::new(proposal_cache, base_bridge),
        PostingTickNodeState::new(node_config),
        PostingTickControl::new(bridge_status, status_state),
        PostingTickConfig::new(policy.failover_backoff_secs),
    );
    let mut state = PostingTickState::default();

    loop {
        tokio::time::sleep(policy.tick_interval).await;
        update_proposal_cache_metrics(&context.ports.proposal_cache);
        if stop.is_stopped() {
            continue;
        }

        let _ = posting_tick_once(
            &context,
            &mut state,
            PostingTickInput {
                now: SystemTime::now(),
            },
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use tokio::time::{sleep, Duration};

    use super::*;
    use crate::types::{
        zero_tip5_hash, AtomBytes, BaseEventContent, EthAddress, Tip5Hash, Withdrawal,
    };

    #[test]
    fn format_ud_for_cord_inserts_dots() {
        assert_eq!(format_ud_for_cord(0), "0");
        assert_eq!(format_ud_for_cord(12), "12");
        assert_eq!(format_ud_for_cord(999), "999");
        assert_eq!(format_ud_for_cord(1_000), "1.000");
        assert_eq!(format_ud_for_cord(50_000), "50.000");
        assert_eq!(format_ud_for_cord(1_234_567), "1.234.567");
        assert_eq!(format_ud_for_cord(1_234_567_890), "1.234.567.890");
    }

    #[test]
    fn format_source_tx_id_uses_base58() {
        let tx_id = Tip5Hash::from_limbs(&[1, 2, 3, 4, 5]);
        let expected = tx_id.to_base58();
        assert_eq!(format_source_tx_id(&tx_id), expected);
        assert_ne!(format!("{:?}", tx_id), expected);
    }

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
                base_event_id: AtomBytes(vec![0x01]),
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
                        hash: crate::types::zero_tip5_hash(),
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
