#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use bridge::core::ports::{BaseSourcePort, KernelStatePort, NockSourcePort, NockTipInfo};
use bridge::deposit::base::DepositSubmissionResult;
use bridge::deposit::ports::BaseContractPort;
use bridge::deposit::types::DepositSubmission;
use bridge::shared::errors::BridgeError;
use bridge::shared::runtime::{BaseBlockBatch, ChainEvent, EventId, NockBlockEvent};
use bridge::shared::types::Tip5Hash;
use tokio::sync::Mutex;

type SharedHeightResultQueue = Arc<Mutex<VecDeque<Result<Option<u64>, BridgeError>>>>;
type SharedHoldResultQueue = Arc<Mutex<VecDeque<Result<bool, BridgeError>>>>;

#[derive(Clone, Default)]
pub struct InMemoryBaseContract {
    pub last_nonce: Arc<Mutex<u64>>,
    pub processed: Arc<Mutex<Vec<Tip5Hash>>>,
    pub submissions: Arc<Mutex<Vec<DepositSubmission>>>,
}

#[async_trait]
impl BaseContractPort for InMemoryBaseContract {
    async fn submit_deposit(
        &self,
        submission: DepositSubmission,
    ) -> Result<DepositSubmissionResult, BridgeError> {
        self.submissions.lock().await.push(submission);
        let mut nonce = self.last_nonce.lock().await;
        *nonce = nonce.saturating_add(1);
        Ok(DepositSubmissionResult {
            tx_hash: format!("in-memory-{:x}", *nonce),
            block_number: *nonce,
        })
    }

    async fn get_last_deposit_nonce(&self) -> Result<u64, BridgeError> {
        Ok(*self.last_nonce.lock().await)
    }

    async fn is_deposit_processed(&self, tx_id: &Tip5Hash) -> Result<bool, BridgeError> {
        Ok(self.processed.lock().await.iter().any(|seen| seen == tx_id))
    }
}

#[derive(Clone, Default)]
pub struct InMemoryBaseSource {
    pub chain_tip: Arc<Mutex<u64>>,
    pub batches: Arc<Mutex<VecDeque<BaseBlockBatch>>>,
    pub chain_tip_responses: Arc<Mutex<VecDeque<Result<u64, BridgeError>>>>,
    pub batch_responses: Arc<Mutex<VecDeque<Result<BaseBlockBatch, BridgeError>>>>,
}

#[async_trait]
impl BaseSourcePort for InMemoryBaseSource {
    async fn chain_tip_height(&self) -> Result<u64, BridgeError> {
        if let Some(response) = self.chain_tip_responses.lock().await.pop_front() {
            return response;
        }
        Ok(*self.chain_tip.lock().await)
    }

    async fn fetch_batch(&self, _start: u64, _end: u64) -> Result<BaseBlockBatch, BridgeError> {
        if let Some(response) = self.batch_responses.lock().await.pop_front() {
            return response;
        }
        self.batches
            .lock()
            .await
            .pop_front()
            .ok_or_else(|| BridgeError::Runtime("in-memory batch queue empty".into()))
    }
}

#[derive(Default)]
pub struct InMemoryNockSource {
    pub tips: VecDeque<NockTipInfo>,
    pub blocks: VecDeque<NockBlockEvent>,
    pub tip_results: VecDeque<Result<Option<NockTipInfo>, BridgeError>>,
    pub block_results: VecDeque<Result<Option<NockBlockEvent>, BridgeError>>,
}

#[async_trait]
impl NockSourcePort for InMemoryNockSource {
    async fn tip_info(&mut self) -> Result<Option<NockTipInfo>, BridgeError> {
        if let Some(response) = self.tip_results.pop_front() {
            return response;
        }
        Ok(self.tips.pop_front())
    }

    async fn fetch_block_at_height(
        &mut self,
        _height: u64,
    ) -> Result<Option<NockBlockEvent>, BridgeError> {
        if let Some(response) = self.block_results.pop_front() {
            return response;
        }
        Ok(self.blocks.pop_front())
    }
}

#[derive(Clone, Default)]
pub struct InMemoryKernelState {
    pub base_hold_active: Arc<Mutex<bool>>,
    pub base_pending_commit_active: Arc<Mutex<bool>>,
    pub base_next_height: Arc<Mutex<Option<u64>>>,
    pub nock_next_height: Arc<Mutex<Option<u64>>>,
    pub events: Arc<Mutex<Vec<ChainEvent>>>,
    pub base_tip_hash: Arc<Mutex<Option<String>>>,
    pub base_hold_results: SharedHoldResultQueue,
    pub base_next_height_results: SharedHeightResultQueue,
    pub nock_next_height_results: SharedHeightResultQueue,
    pub emit_results: Arc<Mutex<VecDeque<Result<EventId, BridgeError>>>>,
}

#[async_trait]
impl KernelStatePort for InMemoryKernelState {
    async fn peek_base_hold(&self) -> Result<bool, BridgeError> {
        if let Some(response) = self.base_hold_results.lock().await.pop_front() {
            return response;
        }
        Ok(*self.base_hold_active.lock().await)
    }

    async fn peek_base_pending_commit(&self) -> Result<bool, BridgeError> {
        Ok(*self.base_pending_commit_active.lock().await)
    }

    async fn peek_base_next_height(&self) -> Result<Option<u64>, BridgeError> {
        if let Some(response) = self.base_next_height_results.lock().await.pop_front() {
            return response;
        }
        Ok(*self.base_next_height.lock().await)
    }

    async fn peek_nock_next_height(&self) -> Result<Option<u64>, BridgeError> {
        if let Some(response) = self.nock_next_height_results.lock().await.pop_front() {
            return response;
        }
        Ok(*self.nock_next_height.lock().await)
    }

    async fn emit_chain_event(&self, event: ChainEvent) -> Result<EventId, BridgeError> {
        if let Some(response) = self.emit_results.lock().await.pop_front() {
            if response.is_ok() {
                self.events.lock().await.push(event);
            }
            return response;
        }
        self.events.lock().await.push(event);
        Ok(EventId {
            kind: bridge::shared::runtime::BridgeEventKind::ChainBase,
            timestamp_ms: 0,
            digest: [0u8; 32],
        })
    }

    fn set_base_tip_hash(&self, tip_hash: String) {
        if let Ok(mut guard) = self.base_tip_hash.try_lock() {
            *guard = Some(tip_hash);
        }
    }
}
