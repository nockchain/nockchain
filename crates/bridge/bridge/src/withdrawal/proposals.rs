use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use deadpool_diesel::sqlite::{Manager, Pool};
use deadpool_diesel::Runtime;
use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use diesel::OptionalExtension;
use prost::Message;

use crate::observability::metrics;
use crate::shared::errors::BridgeError;
use crate::shared::ingress::proto::WithdrawalCommitCertificate;
use crate::shared::kernel_projection::{
    ensure_kernel_projection_cursor_schema, load_kernel_projection_cursor,
    upsert_kernel_projection_cursor, KernelProjectionCursor,
};
use crate::shared::types::Tip5Hash;
use crate::withdrawal::schema::withdrawals;
use crate::withdrawal::state::{
    AcquireWithdrawalAssemblyOutcome, LiveWithdrawalView, SignedWithdrawalTransactionRecord,
    WithdrawalState,
};
use crate::withdrawal::types::{
    NockWithdrawalRequestKernelData, WithdrawalId, WithdrawalProposalData,
    WithdrawalSequencerProposalArtifacts,
};
use crate::withdrawal::validation::WithdrawalTransactionBodyValidator;

const SQLITE_BUSY_TIMEOUT_MS: u64 = 2_000;

/// Operator-side withdrawal request/state persistence.
///
/// The durable local database intentionally stores only kernel-derived
/// withdrawal requests and the local lifecycle projection. Proposal bodies and
/// signer contributions are process-local cache entries; canonical proposal
/// artifacts are owned by the sequencer journal/projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedWithdrawalRequest {
    pub id: WithdrawalId,
    pub recipient: Tip5Hash,
    pub amount: u64,
    pub base_batch_end: u64,
    pub withdrawal_nonce: u64,
}

#[derive(Debug, Clone, PartialEq)]
struct StoredSignedProposalContribution {
    signer_node_id: u64,
    created_at: u64,
    transaction: nockchain_types::v1::Transaction,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProposalCacheKey {
    id: WithdrawalId,
    epoch: u64,
}

#[derive(Default)]
struct WithdrawalProposalCache {
    proposals: Mutex<HashMap<ProposalCacheKey, WithdrawalProposalData>>,
    signatures: Mutex<HashMap<ProposalCacheKey, Vec<StoredSignedProposalContribution>>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WithdrawalProposalCacheSummary {
    pub proposal_count: u64,
    pub signature_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithdrawalTuiRow {
    pub id: WithdrawalId,
    pub recipient: Option<Tip5Hash>,
    pub amount: Option<u64>,
    pub base_batch_end: Option<u64>,
    pub withdrawal_nonce: u64,
    pub current_epoch: u64,
    pub proposal_hash: Option<String>,
    pub has_commit_certificate: bool,
    pub has_authorized_transaction: bool,
    pub has_submitted_transaction: bool,
    pub turn_started_base_height: Option<u64>,
    pub state: WithdrawalState,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WithdrawalTuiCounts {
    pub total_count: u64,
    pub live_count: u64,
    pub ordering_blocking_count: u64,
    pub pending_count: u64,
    pub assembling_count: u64,
    pub prepared_count: u64,
    pub peer_canonical_count: u64,
    pub authorized_count: u64,
    pub mempool_accepted_count: u64,
    pub confirmed_count: u64,
    pub below_frontier_count: u64,
    pub above_frontier_count: u64,
}

impl TrackedWithdrawalRequest {
    fn new(
        id: WithdrawalId,
        recipient: Tip5Hash,
        amount: u64,
        base_batch_end: u64,
        withdrawal_nonce: u64,
    ) -> Self {
        Self {
            id,
            recipient,
            amount,
            base_batch_end,
            withdrawal_nonce,
        }
    }

    pub(crate) fn from_live_withdrawal(row: &LiveWithdrawalView) -> Result<Self, BridgeError> {
        let withdrawal_nonce = row.withdrawal_nonce.ok_or_else(|| {
            BridgeError::Runtime(format!(
                "live withdrawal {:?} is missing withdrawal nonce",
                row.id
            ))
        })?;
        let recipient = row.recipient.clone().ok_or_else(|| {
            BridgeError::Runtime(format!("live withdrawal {:?} is missing recipient", row.id))
        })?;
        let amount = row.gross_burned_amount.ok_or_else(|| {
            BridgeError::Runtime(format!(
                "live withdrawal {:?} is missing gross burned amount",
                row.id
            ))
        })?;
        let base_batch_end = row.base_batch_end.ok_or_else(|| {
            BridgeError::Runtime(format!(
                "live withdrawal {:?} is missing base batch end",
                row.id
            ))
        })?;
        Ok(Self {
            id: row.id.clone(),
            recipient,
            amount,
            base_batch_end,
            withdrawal_nonce,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WithdrawalProposalValidationOutcome {
    Inserted,
    Replay,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum WithdrawalProposalValidationError {
    #[error("unknown withdrawal {id:?}")]
    UnknownWithdrawal { id: WithdrawalId },

    #[error("proposal does not match tracked withdrawal for {id:?}: {field}")]
    WithdrawalMismatch {
        id: WithdrawalId,
        field: &'static str,
    },

    #[error("same-epoch equivocation for withdrawal {id:?} at epoch {epoch}")]
    SameEpochEquivocation { id: WithdrawalId, epoch: u64 },

    #[error("invalid withdrawal transaction body for {id:?} epoch {epoch}: {reason}")]
    InvalidTransactionBody {
        id: WithdrawalId,
        epoch: u64,
        reason: String,
    },

    #[error(
        "selected inputs for withdrawal {id:?} epoch {epoch} are not spendable at the local safe tip: {reason}"
    )]
    SelectedInputsNotSafe {
        id: WithdrawalId,
        epoch: u64,
        reason: String,
    },

    #[error(
        "non-contiguous epoch for withdrawal {id:?}: expected {expected_epoch}, got {received_epoch}"
    )]
    NonContiguousEpoch {
        id: WithdrawalId,
        expected_epoch: u64,
        received_epoch: u64,
    },

    #[error(
        "epoch advancement for withdrawal {id:?} is blocked by live {live_state:?} attempt at epoch {current_epoch}: got {received_epoch}"
    )]
    LiveAttemptExists {
        id: WithdrawalId,
        current_epoch: u64,
        live_state: WithdrawalState,
        received_epoch: u64,
    },

    #[error(
        "wrong assembler for withdrawal {id:?} epoch {epoch}: expected node {expected_node_id}, got node {received_node_id}"
    )]
    WrongAssembler {
        id: WithdrawalId,
        epoch: u64,
        expected_node_id: u64,
        received_node_id: u64,
    },

    #[error("withdrawal projection store failure: {0}")]
    Store(String),
}

impl From<BridgeError> for WithdrawalProposalValidationError {
    fn from(err: BridgeError) -> Self {
        Self::Store(err.to_string())
    }
}

impl From<diesel::result::Error> for WithdrawalProposalValidationError {
    fn from(err: diesel::result::Error) -> Self {
        Self::Store(format!("withdrawal projection transaction failed: {err}"))
    }
}

#[derive(Insertable)]
#[diesel(table_name = withdrawals)]
struct NewWithdrawalRow {
    base_as_of: Vec<u8>,
    base_event_id: Vec<u8>,
    recipient: Vec<u8>,
    gross_burned_amount: i64,
    base_batch_end: i64,
    withdrawal_nonce: i64,
    current_epoch: i64,
    proposal_hash: Option<String>,
    peer_commit_certificate: Option<Vec<u8>>,
    state: String,
    turn_started_base_height: Option<i64>,
    submitted_tx_name: Option<String>,
    submitted_tx_hash: Option<String>,
    submitted_at: Option<i64>,
    confirmed_height: Option<i64>,
    confirmed_block_id: Option<Vec<u8>>,
    created_at: i64,
    updated_at: i64,
}

#[derive(Clone, Queryable, Identifiable)]
#[diesel(table_name = withdrawals)]
struct WithdrawalRow {
    id: i64,
    base_as_of: Vec<u8>,
    base_event_id: Vec<u8>,
    recipient: Vec<u8>,
    gross_burned_amount: i64,
    base_batch_end: i64,
    withdrawal_nonce: i64,
    current_epoch: i64,
    proposal_hash: Option<String>,
    peer_commit_certificate: Option<Vec<u8>>,
    state: String,
    turn_started_base_height: Option<i64>,
    submitted_tx_name: Option<String>,
    submitted_tx_hash: Option<String>,
    _submitted_at: Option<i64>,
    _confirmed_height: Option<i64>,
    _confirmed_block_id: Option<Vec<u8>>,
    created_at: i64,
    updated_at: i64,
}

pub struct WithdrawalProjectionStore {
    pool: Pool,
}

impl WithdrawalProjectionStore {
    /// Opens the withdrawal request/state store and ensures the SQLite schema.
    pub async fn open(path: PathBuf) -> Result<Self, BridgeError> {
        let pool = sqlite_pool(&path)?;
        let store = Self { pool };
        store.ensure_schema().await?;
        Ok(store)
    }

    async fn with_conn<T, F>(&self, f: F) -> Result<T, BridgeError>
    where
        F: FnOnce(&mut SqliteConnection) -> Result<T, BridgeError> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.pool.get().await.map_err(|err| {
            BridgeError::Runtime(format!("withdrawal projection pool failed: {err}"))
        })?;
        conn.interact(move |conn| {
            conn.batch_execute(&format!(
                "PRAGMA busy_timeout = {};",
                SQLITE_BUSY_TIMEOUT_MS
            ))
            .map_err(|err| {
                BridgeError::Runtime(format!("withdrawal projection pragma failed: {err}"))
            })?;
            f(conn)
        })
        .await
        .map_err(|err| {
            BridgeError::Runtime(format!("withdrawal projection interact failed: {err}"))
        })?
    }

    async fn persist_requests(
        &self,
        requests: Vec<NockWithdrawalRequestKernelData>,
    ) -> Result<(), BridgeError> {
        self.with_conn(move |conn| persist_withdrawal_requests(conn, &requests))
            .await
    }

    async fn replay_withdrawal_request_projection(
        &self,
        requests: Vec<NockWithdrawalRequestKernelData>,
        next_cursor: KernelProjectionCursor,
    ) -> Result<(), BridgeError> {
        let updated_at = now_unix_secs()?;
        self.with_conn(move |conn| {
            conn.immediate_transaction::<_, anyhow::Error, _>(move |conn| {
                persist_withdrawal_requests_in_transaction(conn, &requests)?;
                recover_startup_assembly_locks(conn, updated_at)?;
                upsert_kernel_projection_cursor(conn, &next_cursor)?;
                Ok(())
            })
            .map_err(BridgeError::from)
        })
        .await
    }

    async fn has_withdrawal_projection_rows(&self) -> Result<bool, BridgeError> {
        self.with_conn(has_withdrawal_projection_rows).await
    }

    async fn load_kernel_projection_cursor(
        &self,
    ) -> Result<Option<KernelProjectionCursor>, BridgeError> {
        self.with_conn(load_kernel_projection_cursor).await
    }

    async fn upsert_kernel_projection_cursor(
        &self,
        cursor: KernelProjectionCursor,
    ) -> Result<(), BridgeError> {
        self.with_conn(move |conn| upsert_kernel_projection_cursor(conn, &cursor))
            .await
    }

    pub async fn max_persisted_base_batch_end(&self) -> Result<Option<u64>, BridgeError> {
        self.with_conn(max_persisted_base_batch_end).await
    }

    pub async fn persist_request(
        &self,
        request: &NockWithdrawalRequestKernelData,
    ) -> Result<TrackedWithdrawalRequest, BridgeError> {
        self.persist_requests(vec![request.clone()]).await?;
        self.fetch_request(request.withdrawal_id())
            .await?
            .ok_or_else(|| {
                BridgeError::Runtime(format!(
                    "missing persisted withdrawal request for {:?}",
                    request.withdrawal_id()
                ))
            })
    }

    pub async fn load_sorted_tracked_requests(
        &self,
    ) -> Result<Vec<TrackedWithdrawalRequest>, BridgeError> {
        self.with_conn(load_sorted_withdrawal_requests).await
    }

    pub async fn recover_startup_assembly_locks(&self) -> Result<u64, BridgeError> {
        let updated_at = now_unix_secs()?;
        self.with_conn(move |conn| recover_startup_assembly_locks(conn, updated_at))
            .await
    }

    pub async fn load_live_withdrawals(&self) -> Result<Vec<LiveWithdrawalView>, BridgeError> {
        self.with_conn(load_live_withdrawals).await
    }

    pub async fn load_live_withdrawals_in_state(
        &self,
        state: WithdrawalState,
    ) -> Result<Vec<LiveWithdrawalView>, BridgeError> {
        self.with_conn(move |conn| load_live_withdrawals_filtered(conn, Some(state)))
            .await
    }

    pub async fn fetch_request(
        &self,
        id: WithdrawalId,
    ) -> Result<Option<TrackedWithdrawalRequest>, BridgeError> {
        self.with_conn(move |conn| {
            fetch_withdrawal_request(conn, &id).map(|entry| entry.map(|(tracked, _state)| tracked))
        })
        .await
    }

    pub async fn current_epoch(&self, id: WithdrawalId) -> Result<Option<u64>, BridgeError> {
        self.with_conn(move |conn| fetch_withdrawal_current_epoch(conn, &id))
            .await
    }

    pub async fn fetch_live_withdrawal(
        &self,
        id: &WithdrawalId,
    ) -> Result<Option<LiveWithdrawalView>, BridgeError> {
        let id = id.clone();
        self.with_conn(move |conn| fetch_live_withdrawal_view(conn, &id))
            .await
    }

    pub async fn fetch_live_withdrawal_by_nonce(
        &self,
        withdrawal_nonce: u64,
    ) -> Result<Option<LiveWithdrawalView>, BridgeError> {
        self.with_conn(move |conn| fetch_live_withdrawal_view_by_nonce(conn, withdrawal_nonce))
            .await
    }

    pub async fn fetch_tui_row_by_nonce(
        &self,
        withdrawal_nonce: u64,
    ) -> Result<Option<WithdrawalTuiRow>, BridgeError> {
        self.with_conn(move |conn| fetch_withdrawal_tui_row_by_nonce(conn, withdrawal_nonce))
            .await
    }

    pub async fn load_tui_rows_around_nonce(
        &self,
        center_nonce: Option<u64>,
        limit: usize,
    ) -> Result<Vec<WithdrawalTuiRow>, BridgeError> {
        self.with_conn(move |conn| load_withdrawal_tui_rows(conn, center_nonce, limit))
            .await
    }

    pub async fn load_tui_counts(
        &self,
        frontier_nonce: Option<u64>,
    ) -> Result<WithdrawalTuiCounts, BridgeError> {
        self.with_conn(move |conn| load_withdrawal_tui_counts(conn, frontier_nonce))
            .await
    }

    pub async fn acquire_withdrawal_assembly(
        &self,
        id: &WithdrawalId,
        epoch: u64,
        turn_started_base_height: u64,
    ) -> Result<AcquireWithdrawalAssemblyOutcome, BridgeError> {
        let id = id.clone();
        let updated_at = now_unix_secs()?;
        self.with_conn(move |conn| {
            acquire_withdrawal_assembly(conn, &id, epoch, updated_at, turn_started_base_height)
        })
        .await
    }

    pub async fn release_assembly_lock(&self, id: &WithdrawalId) -> Result<(), BridgeError> {
        let id = id.clone();
        let updated_at = now_unix_secs()?;
        self.with_conn(move |conn| release_assembly_lock(conn, &id, updated_at))
            .await
    }

    pub async fn release_stale_assembly_lock(
        &self,
        id: &WithdrawalId,
        epoch: u64,
    ) -> Result<bool, BridgeError> {
        let id = id.clone();
        let updated_at = now_unix_secs()?;
        self.with_conn(move |conn| release_stale_assembly_lock(conn, &id, epoch, updated_at))
            .await
    }

    pub async fn reconcile_pending_epoch(
        &self,
        id: &WithdrawalId,
        sequencer_epoch: u64,
    ) -> Result<bool, BridgeError> {
        let id = id.clone();
        let updated_at = now_unix_secs()?;
        self.with_conn(move |conn| reconcile_pending_epoch(conn, &id, sequencer_epoch, updated_at))
            .await
    }

    pub async fn reconcile_prepared_with_pending_sequencer(
        &self,
        id: &WithdrawalId,
        sequencer_epoch: u64,
        sequencer_handoff_index: u64,
        sequencer_turn_started_base_height: Option<u64>,
    ) -> Result<bool, BridgeError> {
        let id = id.clone();
        let updated_at = now_unix_secs()?;
        self.with_conn(move |conn| {
            reconcile_prepared_with_pending_sequencer(
                conn, &id, sequencer_epoch, sequencer_handoff_index,
                sequencer_turn_started_base_height, updated_at,
            )
        })
        .await
    }

    async fn validate_and_stage_prepared(
        &self,
        proposal: WithdrawalProposalData,
    ) -> Result<WithdrawalProposalValidationOutcome, WithdrawalProposalValidationError> {
        let conn = self.pool.get().await.map_err(|err| {
            WithdrawalProposalValidationError::Store(format!(
                "withdrawal projection pool failed: {err}"
            ))
        })?;
        conn.interact(move |conn| {
            conn.batch_execute(&format!(
                "PRAGMA busy_timeout = {};",
                SQLITE_BUSY_TIMEOUT_MS
            ))
            .map_err(|err| {
                WithdrawalProposalValidationError::Store(format!(
                    "withdrawal projection pragma failed: {err}"
                ))
            })?;
            conn.immediate_transaction::<_, WithdrawalProposalValidationError, _>(|conn| {
                validate_and_stage_prepared(conn, &proposal)
            })
        })
        .await
        .map_err(|err| {
            WithdrawalProposalValidationError::Store(format!(
                "withdrawal projection interact failed: {err}"
            ))
        })?
    }

    pub async fn mark_proposal_prepared(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<(), BridgeError> {
        let proposal = proposal.clone();
        self.with_conn(move |conn| mark_proposal_prepared(conn, &proposal))
            .await
    }

    pub async fn mark_proposal_canonical(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<(), BridgeError> {
        let proposal = proposal.clone();
        self.with_conn(move |conn| mark_proposal_canonical(conn, &proposal, None))
            .await
    }

    pub async fn mark_proposal_canonical_with_certificate(
        &self,
        proposal: &WithdrawalProposalData,
        commit_certificate: &WithdrawalCommitCertificate,
    ) -> Result<(), BridgeError> {
        let proposal = proposal.clone();
        let commit_certificate = commit_certificate.clone();
        self.with_conn(move |conn| {
            mark_proposal_canonical(conn, &proposal, Some(commit_certificate.encode_to_vec()))
        })
        .await
    }

    pub async fn mark_proposal_authorized(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<(), BridgeError> {
        let proposal = proposal.clone();
        self.with_conn(move |conn| mark_proposal_authorized(conn, &proposal))
            .await
    }

    pub async fn mark_proposal_mempool_accepted(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<(), BridgeError> {
        let proposal = proposal.clone();
        self.with_conn(move |conn| mark_proposal_mempool_accepted(conn, &proposal))
            .await
    }

    pub async fn mark_proposal_confirmed(
        &self,
        proposal: &WithdrawalProposalData,
        confirmed_height: u64,
        confirmed_block_id: Tip5Hash,
    ) -> Result<(), BridgeError> {
        let proposal = proposal.clone();
        self.with_conn(move |conn| {
            mark_proposal_confirmed(conn, &proposal, confirmed_height, &confirmed_block_id)
        })
        .await
    }

    pub async fn mark_proposal_expired(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<(), BridgeError> {
        let proposal = proposal.clone();
        self.with_conn(move |conn| mark_proposal_expired(conn, &proposal))
            .await
    }

    async fn ensure_schema(&self) -> Result<(), BridgeError> {
        self.with_conn(|conn| {
            conn.batch_execute(
                r#"
            PRAGMA journal_mode=WAL;
            PRAGMA synchronous=FULL;

            CREATE TABLE IF NOT EXISTS withdrawals (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                base_as_of BLOB NOT NULL CHECK(length(base_as_of) = 40),
                base_event_id BLOB NOT NULL CHECK(length(base_event_id) = 32),
                recipient BLOB NOT NULL CHECK(length(recipient) = 40),
                gross_burned_amount INTEGER NOT NULL,
                base_batch_end INTEGER NOT NULL,
                withdrawal_nonce INTEGER NOT NULL UNIQUE,
                current_epoch INTEGER NOT NULL,
                proposal_hash TEXT NULL,
                peer_commit_certificate BLOB NULL,
                state TEXT NOT NULL,
                turn_started_base_height INTEGER NULL,
                submitted_tx_name TEXT NULL,
                submitted_tx_hash TEXT NULL,
                submitted_at INTEGER NULL,
                confirmed_height INTEGER NULL,
                confirmed_block_id BLOB NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                UNIQUE(base_as_of, base_event_id)
            );

            CREATE INDEX IF NOT EXISTS withdrawals_by_ordering
              ON withdrawals(base_batch_end, base_event_id);
            CREATE INDEX IF NOT EXISTS withdrawals_by_nonce
              ON withdrawals(withdrawal_nonce);
            CREATE INDEX IF NOT EXISTS withdrawals_by_state
              ON withdrawals(state);
            "#,
            )
            .map_err(|err| {
                BridgeError::Runtime(format!("withdrawal projection schema failed: {err}"))
            })?;
            ensure_kernel_projection_cursor_schema(conn)?;
            ensure_table_column(conn, "withdrawals", "turn_started_base_height", "INTEGER")?;
            ensure_table_column(conn, "withdrawals", "proposal_hash", "TEXT")?;
            ensure_table_column(conn, "withdrawals", "peer_commit_certificate", "BLOB")?;
            if !sqlite_table_has_column(conn, "withdrawals", "withdrawal_nonce")? {
                return Err(BridgeError::Runtime(
                    "legacy withdrawals schema without withdrawal_nonce is unsupported; recreate the undeployed bridge DB"
                        .into(),
                ));
            }
            Ok(())
        })
        .await
    }
}

#[derive(Clone)]
pub struct WithdrawalProposalRegistry {
    store: Arc<WithdrawalProjectionStore>,
    cache: Arc<WithdrawalProposalCache>,
    transaction_body_validation: WithdrawalTransactionBodyValidation,
}

#[derive(Clone)]
enum WithdrawalTransactionBodyValidation {
    Enforced(WithdrawalTransactionBodyValidator),
    #[cfg(test)]
    DisabledForTests,
}

impl WithdrawalProposalRegistry {
    pub fn new(
        store: Arc<WithdrawalProjectionStore>,
        transaction_body_validator: WithdrawalTransactionBodyValidator,
    ) -> Self {
        Self {
            store,
            cache: Arc::new(WithdrawalProposalCache::default()),
            transaction_body_validation: WithdrawalTransactionBodyValidation::Enforced(
                transaction_body_validator,
            ),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_without_transaction_body_validator_for_tests(
        store: Arc<WithdrawalProjectionStore>,
    ) -> Self {
        Self {
            store,
            cache: Arc::new(WithdrawalProposalCache::default()),
            transaction_body_validation: WithdrawalTransactionBodyValidation::DisabledForTests,
        }
    }

    pub async fn track_withdrawal_request(
        &self,
        request: &NockWithdrawalRequestKernelData,
    ) -> Result<TrackedWithdrawalRequest, BridgeError> {
        self.track_withdrawal_requests(std::slice::from_ref(request))
            .await?;
        self.store
            .fetch_request(request.withdrawal_id())
            .await?
            .ok_or_else(|| {
                BridgeError::Runtime(format!(
                    "missing tracked withdrawal request for {:?}",
                    request.withdrawal_id()
                ))
            })
    }

    pub async fn track_withdrawal_requests(
        &self,
        requests: &[NockWithdrawalRequestKernelData],
    ) -> Result<u64, BridgeError> {
        self.store.persist_requests(requests.to_vec()).await?;
        Ok(self.store.load_sorted_tracked_requests().await?.len() as u64)
    }

    pub async fn restore_tracked_withdrawal_requests(&self) -> Result<u64, BridgeError> {
        self.store.recover_startup_assembly_locks().await?;
        self.clear_cache();
        Ok(self.store.load_sorted_tracked_requests().await?.len() as u64)
    }

    pub async fn load_kernel_projection_cursor(
        &self,
    ) -> Result<Option<KernelProjectionCursor>, BridgeError> {
        self.store.load_kernel_projection_cursor().await
    }

    pub async fn has_kernel_projection_rows(&self) -> Result<bool, BridgeError> {
        self.store.has_withdrawal_projection_rows().await
    }

    pub async fn set_kernel_projection_cursor(
        &self,
        cursor: KernelProjectionCursor,
    ) -> Result<(), BridgeError> {
        self.store
            .upsert_kernel_projection_cursor(cursor.clone())
            .await?;
        record_withdrawal_projection_cursor_metrics(&cursor);
        Ok(())
    }

    pub async fn replay_withdrawal_request_projection(
        &self,
        requests: Vec<NockWithdrawalRequestKernelData>,
        next_cursor: KernelProjectionCursor,
    ) -> Result<u64, BridgeError> {
        let cursor_metrics = next_cursor.clone();
        if let Err(err) = self
            .store
            .replay_withdrawal_request_projection(requests, next_cursor)
            .await
        {
            metrics::init_metrics()
                .withdrawal_projection_replay_error
                .increment();
            return Err(err);
        }
        self.clear_cache();
        let rows = self.store.load_sorted_tracked_requests().await?.len() as u64;
        let metrics = metrics::init_metrics();
        metrics.withdrawal_projection_replay_rows.swap(rows as f64);
        record_withdrawal_projection_cursor_metrics(&cursor_metrics);
        Ok(rows)
    }

    pub async fn load_sorted_tracked_withdrawal_requests(
        &self,
    ) -> Result<Vec<TrackedWithdrawalRequest>, BridgeError> {
        self.store.load_sorted_tracked_requests().await
    }

    pub async fn next_expected_epoch(&self, id: &WithdrawalId) -> Result<u64, BridgeError> {
        Ok(self.store.current_epoch(id.clone()).await?.unwrap_or(0))
    }

    pub async fn withdrawal_nonce(&self, id: &WithdrawalId) -> Result<Option<u64>, BridgeError> {
        Ok(self
            .fetch_live_withdrawal(id)
            .await?
            .and_then(|row| row.withdrawal_nonce))
    }

    pub async fn max_persisted_base_batch_end(&self) -> Result<Option<u64>, BridgeError> {
        self.store.max_persisted_base_batch_end().await
    }

    pub async fn fetch_cached_proposal(
        &self,
        id: WithdrawalId,
        epoch: u64,
    ) -> Result<Option<WithdrawalProposalData>, BridgeError> {
        let proposal = self
            .cache
            .proposals
            .lock()
            .map_err(|_| {
                metrics::init_metrics()
                    .withdrawal_proposal_cache_poisoned
                    .increment();
                BridgeError::Runtime("withdrawal proposal cache poisoned".into())
            })?
            .get(&ProposalCacheKey { id, epoch })
            .cloned();
        if proposal.is_none() {
            metrics::init_metrics()
                .withdrawal_proposal_cache_cache_miss
                .increment();
        }
        Ok(proposal)
    }

    pub async fn cache_reconstructed_proposal(
        &self,
        proposal: WithdrawalProposalData,
    ) -> Result<(), BridgeError> {
        self.cache_proposal(proposal)?;
        metrics::init_metrics()
            .withdrawal_proposal_cache_hydrated
            .increment();
        Ok(())
    }

    pub async fn list_live_withdrawals(&self) -> Result<Vec<LiveWithdrawalView>, BridgeError> {
        self.store.load_live_withdrawals().await
    }

    pub async fn list_live_withdrawals_in_state(
        &self,
        state: WithdrawalState,
    ) -> Result<Vec<LiveWithdrawalView>, BridgeError> {
        self.store.load_live_withdrawals_in_state(state).await
    }

    pub async fn fetch_live_withdrawal(
        &self,
        id: &WithdrawalId,
    ) -> Result<Option<LiveWithdrawalView>, BridgeError> {
        self.store.fetch_live_withdrawal(id).await
    }

    pub async fn fetch_live_withdrawal_by_nonce(
        &self,
        withdrawal_nonce: u64,
    ) -> Result<Option<LiveWithdrawalView>, BridgeError> {
        self.store
            .fetch_live_withdrawal_by_nonce(withdrawal_nonce)
            .await
    }

    pub async fn fetch_tui_row_by_nonce(
        &self,
        withdrawal_nonce: u64,
    ) -> Result<Option<WithdrawalTuiRow>, BridgeError> {
        self.store.fetch_tui_row_by_nonce(withdrawal_nonce).await
    }

    pub async fn load_tui_rows_around_nonce(
        &self,
        center_nonce: Option<u64>,
        limit: usize,
    ) -> Result<Vec<WithdrawalTuiRow>, BridgeError> {
        self.store
            .load_tui_rows_around_nonce(center_nonce, limit)
            .await
    }

    pub async fn load_tui_counts(
        &self,
        frontier_nonce: Option<u64>,
    ) -> Result<WithdrawalTuiCounts, BridgeError> {
        self.store.load_tui_counts(frontier_nonce).await
    }

    pub fn cache_summary(&self) -> Result<WithdrawalProposalCacheSummary, BridgeError> {
        let proposal_count = self
            .cache
            .proposals
            .lock()
            .map_err(|_| BridgeError::Runtime("withdrawal proposal cache poisoned".into()))?
            .len();
        let signature_count: usize = self
            .cache
            .signatures
            .lock()
            .map_err(|_| BridgeError::Runtime("withdrawal signature cache poisoned".into()))?
            .values()
            .map(Vec::len)
            .sum();
        Ok(WithdrawalProposalCacheSummary {
            proposal_count: u64::try_from(proposal_count).map_err(|err| {
                BridgeError::ValueConversion(format!("proposal cache count overflow: {err}"))
            })?,
            signature_count: u64::try_from(signature_count).map_err(|err| {
                BridgeError::ValueConversion(format!("signature cache count overflow: {err}"))
            })?,
        })
    }

    pub async fn acquire_withdrawal_assembly(
        &self,
        id: &WithdrawalId,
        epoch: u64,
        turn_started_base_height: u64,
    ) -> Result<AcquireWithdrawalAssemblyOutcome, BridgeError> {
        self.store
            .acquire_withdrawal_assembly(id, epoch, turn_started_base_height)
            .await
    }

    pub async fn release_assembly_lock(&self, id: &WithdrawalId) -> Result<(), BridgeError> {
        self.store.release_assembly_lock(id).await
    }

    pub async fn release_stale_assembly_lock(
        &self,
        id: &WithdrawalId,
        epoch: u64,
    ) -> Result<bool, BridgeError> {
        self.store.release_stale_assembly_lock(id, epoch).await
    }

    pub async fn reconcile_pending_epoch(
        &self,
        id: &WithdrawalId,
        sequencer_epoch: u64,
    ) -> Result<bool, BridgeError> {
        let changed = self
            .store
            .reconcile_pending_epoch(id, sequencer_epoch)
            .await?;
        if changed {
            self.clear_cache();
        }
        Ok(changed)
    }

    pub async fn reconcile_prepared_with_pending_sequencer(
        &self,
        id: &WithdrawalId,
        sequencer_epoch: u64,
        sequencer_handoff_index: u64,
        sequencer_turn_started_base_height: Option<u64>,
    ) -> Result<bool, BridgeError> {
        let changed = self
            .store
            .reconcile_prepared_with_pending_sequencer(
                id, sequencer_epoch, sequencer_handoff_index, sequencer_turn_started_base_height,
            )
            .await?;
        if changed {
            self.clear_cache();
        }
        Ok(changed)
    }

    pub async fn mark_proposal_prepared(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<(), BridgeError> {
        self.cache_proposal(proposal.clone())?;
        self.store.mark_proposal_prepared(proposal).await
    }

    pub async fn mark_proposal_canonical(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<(), BridgeError> {
        self.cache_proposal(proposal.clone())?;
        self.store.mark_proposal_canonical(proposal).await
    }

    pub async fn mark_proposal_canonical_with_certificate(
        &self,
        proposal: &WithdrawalProposalData,
        commit_certificate: &WithdrawalCommitCertificate,
    ) -> Result<(), BridgeError> {
        self.cache_proposal(proposal.clone())?;
        self.store
            .mark_proposal_canonical_with_certificate(proposal, commit_certificate)
            .await
    }

    pub async fn mark_proposal_authorized(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<(), BridgeError> {
        self.cache_proposal(proposal.clone())?;
        self.store.mark_proposal_authorized(proposal).await
    }

    pub async fn mark_proposal_mempool_accepted(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<(), BridgeError> {
        self.cache_proposal(proposal.clone())?;
        self.store.mark_proposal_mempool_accepted(proposal).await
    }

    pub async fn mark_proposal_confirmed(
        &self,
        proposal: &WithdrawalProposalData,
        confirmed_height: u64,
        confirmed_block_id: Tip5Hash,
    ) -> Result<(), BridgeError> {
        self.store
            .mark_proposal_confirmed(proposal, confirmed_height, confirmed_block_id)
            .await?;
        self.evict_proposal(&proposal.id, proposal.epoch)
    }

    pub async fn mark_proposal_expired(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<(), BridgeError> {
        self.evict_proposal(&proposal.id, proposal.epoch)?;
        self.store.mark_proposal_expired(proposal).await
    }

    pub async fn record_proposal_signed(
        &self,
        proposal: &WithdrawalProposalData,
        signer_node_id: u64,
    ) -> Result<(), BridgeError> {
        let key = ProposalCacheKey {
            id: proposal.id.clone(),
            epoch: proposal.epoch,
        };
        let proposal_hash = proposal.proposal_hash()?;
        let cached = self
            .fetch_cached_proposal(proposal.id.clone(), proposal.epoch)
            .await?
            .ok_or_else(|| {
                BridgeError::Runtime(format!(
                    "missing cached proposal for signed withdrawal {:?} epoch {}",
                    proposal.id, proposal.epoch
                ))
            })?;
        if cached.proposal_hash()? != proposal_hash {
            return Err(BridgeError::Runtime(format!(
                "signed proposal hash {} does not match cached proposal hash {}",
                proposal_hash,
                cached.proposal_hash()?
            )));
        }

        let mut guard = self
            .cache
            .signatures
            .lock()
            .map_err(|_| BridgeError::Runtime("withdrawal signature cache poisoned".into()))?;
        let contributions = guard.entry(key).or_default();
        if let Some(existing) = contributions
            .iter()
            .find(|entry| entry.signer_node_id == signer_node_id)
        {
            if existing.transaction == proposal.transaction {
                return Ok(());
            }
            return Err(BridgeError::Runtime(format!(
                "conflicting signed contribution for signer {} on withdrawal {:?} epoch {}",
                signer_node_id, proposal.id, proposal.epoch
            )));
        }
        contributions.push(StoredSignedProposalContribution {
            signer_node_id,
            created_at: u64::try_from(now_unix_secs()?).map_err(|err| {
                BridgeError::ValueConversion(format!("created_at overflow: {err}"))
            })?,
            transaction: proposal.transaction.clone(),
        });
        Ok(())
    }

    pub async fn has_signed_proposal_from_signer(
        &self,
        id: &WithdrawalId,
        epoch: u64,
        _proposal_hash: &str,
        signer_node_id: u64,
    ) -> Result<bool, BridgeError> {
        Ok(self
            .cache
            .signatures
            .lock()
            .map_err(|_| BridgeError::Runtime("withdrawal signature cache poisoned".into()))?
            .get(&ProposalCacheKey {
                id: id.clone(),
                epoch,
            })
            .map(|entries| {
                entries
                    .iter()
                    .any(|entry| entry.signer_node_id == signer_node_id)
            })
            .unwrap_or(false))
    }

    pub async fn load_signed_transactions(
        &self,
        id: &WithdrawalId,
        epoch: u64,
        _proposal_hash: &str,
    ) -> Result<Vec<SignedWithdrawalTransactionRecord>, BridgeError> {
        let mut records = self
            .cache
            .signatures
            .lock()
            .map_err(|_| BridgeError::Runtime("withdrawal signature cache poisoned".into()))?
            .get(&ProposalCacheKey {
                id: id.clone(),
                epoch,
            })
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|entry| {
                Ok(SignedWithdrawalTransactionRecord {
                    signer_node_id: entry.signer_node_id,
                    created_at: i64::try_from(entry.created_at).map_err(|err| {
                        BridgeError::ValueConversion(format!("created_at overflow: {err}"))
                    })?,
                    transaction: entry.transaction,
                })
            })
            .collect::<Result<Vec<_>, BridgeError>>()?;
        records.sort_by_key(|record| (record.signer_node_id, record.created_at));
        Ok(records)
    }

    pub async fn validate_and_cache_prepared(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<WithdrawalProposalValidationOutcome, WithdrawalProposalValidationError> {
        let key = ProposalCacheKey {
            id: proposal.id.clone(),
            epoch: proposal.epoch,
        };
        if let Some(existing) = self
            .cache
            .proposals
            .lock()
            .map_err(|_| {
                WithdrawalProposalValidationError::Store(
                    "withdrawal proposal cache poisoned".into(),
                )
            })?
            .get(&key)
            .cloned()
        {
            return if existing == *proposal {
                Ok(WithdrawalProposalValidationOutcome::Replay)
            } else {
                Err(WithdrawalProposalValidationError::SameEpochEquivocation {
                    id: proposal.id.clone(),
                    epoch: proposal.epoch,
                })
            };
        }

        self.validate_transaction_body(proposal).map_err(|err| {
            WithdrawalProposalValidationError::InvalidTransactionBody {
                id: proposal.id.clone(),
                epoch: proposal.epoch,
                reason: err.to_string(),
            }
        })?;
        let outcome = self
            .store
            .validate_and_stage_prepared(proposal.clone())
            .await?;
        self.cache_proposal(proposal.clone())?;
        Ok(outcome)
    }

    fn validate_transaction_body(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<(), BridgeError> {
        match &self.transaction_body_validation {
            WithdrawalTransactionBodyValidation::Enforced(validator) => {
                validator.validate(proposal)
            }
            #[cfg(test)]
            WithdrawalTransactionBodyValidation::DisabledForTests => Ok(()),
        }
    }

    fn cache_proposal(&self, proposal: WithdrawalProposalData) -> Result<(), BridgeError> {
        self.validate_transaction_body(&proposal)?;
        self.cache
            .proposals
            .lock()
            .map_err(|_| {
                metrics::init_metrics()
                    .withdrawal_proposal_cache_poisoned
                    .increment();
                BridgeError::Runtime("withdrawal proposal cache poisoned".into())
            })?
            .insert(
                ProposalCacheKey {
                    id: proposal.id.clone(),
                    epoch: proposal.epoch,
                },
                proposal,
            );
        Ok(())
    }

    fn evict_proposal(&self, id: &WithdrawalId, epoch: u64) -> Result<(), BridgeError> {
        let key = ProposalCacheKey {
            id: id.clone(),
            epoch,
        };
        self.cache
            .proposals
            .lock()
            .map_err(|_| {
                metrics::init_metrics()
                    .withdrawal_proposal_cache_poisoned
                    .increment();
                BridgeError::Runtime("withdrawal proposal cache poisoned".into())
            })?
            .remove(&key);
        self.cache
            .signatures
            .lock()
            .map_err(|_| {
                metrics::init_metrics()
                    .withdrawal_proposal_cache_poisoned
                    .increment();
                BridgeError::Runtime("withdrawal signature cache poisoned".into())
            })?
            .remove(&key);
        metrics::init_metrics()
            .withdrawal_proposal_cache_evicted
            .increment();
        Ok(())
    }

    fn clear_cache(&self) {
        if let Ok(mut proposals) = self.cache.proposals.lock() {
            proposals.clear();
        }
        if let Ok(mut signatures) = self.cache.signatures.lock() {
            signatures.clear();
        }
    }
}

fn record_withdrawal_projection_cursor_metrics(cursor: &KernelProjectionCursor) {
    let metrics = metrics::init_metrics();
    metrics
        .withdrawal_projection_cursor_base_next_height
        .swap(cursor.base_next_height as f64);
    metrics
        .withdrawal_projection_cursor_nock_next_height
        .swap(cursor.nock_next_height as f64);
}

impl WithdrawalRow {
    fn withdrawal_id(&self) -> Result<WithdrawalId, BridgeError> {
        Ok(WithdrawalId {
            as_of: Tip5Hash::from_be_limb_bytes(&self.base_as_of)
                .map_err(|err| BridgeError::Runtime(format!("invalid withdrawal as_of: {err}")))?,
            base_event_id: self.base_event_id.clone().into(),
        })
    }

    fn try_into_tracked_entry(
        self,
    ) -> Result<(TrackedWithdrawalRequest, WithdrawalState), BridgeError> {
        let state = parse_state_tag(&self.state)?;
        let tracked = self.try_into_tracked_request()?;
        Ok((tracked, state))
    }

    fn try_into_tracked_request(self) -> Result<TrackedWithdrawalRequest, BridgeError> {
        let recipient = Tip5Hash::from_be_limb_bytes(&self.recipient)
            .map_err(|err| BridgeError::Runtime(format!("invalid withdrawal recipient: {err}")))?;
        let amount = u64::try_from(self.gross_burned_amount)
            .map_err(|err| BridgeError::ValueConversion(format!("amount overflow: {err}")))?;
        let base_batch_end = u64::try_from(self.base_batch_end).map_err(|err| {
            BridgeError::ValueConversion(format!("base_batch_end overflow: {err}"))
        })?;
        let withdrawal_nonce = stored_withdrawal_nonce(&self)?;
        Ok(TrackedWithdrawalRequest::new(
            self.withdrawal_id()?,
            recipient,
            amount,
            base_batch_end,
            withdrawal_nonce,
        ))
    }

    fn matches_request(
        &self,
        request: &NockWithdrawalRequestKernelData,
    ) -> Result<bool, BridgeError> {
        let recipient = Tip5Hash::from_be_limb_bytes(&self.recipient)
            .map_err(|err| BridgeError::Runtime(format!("invalid withdrawal recipient: {err}")))?;
        let amount = u64::try_from(self.gross_burned_amount)
            .map_err(|err| BridgeError::ValueConversion(format!("amount overflow: {err}")))?;
        let base_batch_end = u64::try_from(self.base_batch_end).map_err(|err| {
            BridgeError::ValueConversion(format!("base_batch_end overflow: {err}"))
        })?;
        Ok(recipient == request.recipient
            && amount == request.amount
            && base_batch_end == request.base_batch_end)
    }
}

fn canonical_withdrawal_ordering_cmp(
    left_base_batch_end: u64,
    left_base_event_id: &[u8],
    right_base_batch_end: u64,
    right_base_event_id: &[u8],
) -> std::cmp::Ordering {
    left_base_batch_end
        .cmp(&right_base_batch_end)
        .then_with(|| left_base_event_id.cmp(right_base_event_id))
}

fn stored_withdrawal_nonce(row: &WithdrawalRow) -> Result<u64, BridgeError> {
    u64::try_from(row.withdrawal_nonce)
        .map_err(|err| BridgeError::ValueConversion(format!("withdrawal nonce overflow: {err}")))
}

fn max_persisted_base_batch_end(conn: &mut SqliteConnection) -> Result<Option<u64>, BridgeError> {
    use crate::withdrawal::schema::withdrawals::dsl as withdrawal_dsl;

    withdrawals::table
        .select(diesel::dsl::max(withdrawal_dsl::base_batch_end))
        .first::<Option<i64>>(conn)
        .map_err(|err| {
            BridgeError::Runtime(format!("withdrawal max base_batch_end failed: {err}"))
        })?
        .map(|value| {
            u64::try_from(value).map_err(|err| {
                BridgeError::ValueConversion(format!("base_batch_end overflow: {err}"))
            })
        })
        .transpose()
}

fn has_withdrawal_projection_rows(conn: &mut SqliteConnection) -> Result<bool, BridgeError> {
    let count = withdrawals::table
        .select(diesel::dsl::count_star())
        .first::<i64>(conn)
        .map_err(|err| {
            BridgeError::Runtime(format!("withdrawal projection row count failed: {err}"))
        })?;
    Ok(count > 0)
}

fn fetch_withdrawal_request(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
) -> Result<Option<(TrackedWithdrawalRequest, WithdrawalState)>, BridgeError> {
    let Some(row) = find_withdrawal_row(conn, id)? else {
        return Ok(None);
    };
    row.try_into_tracked_entry().map(Some)
}

fn fetch_withdrawal_current_epoch(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
) -> Result<Option<u64>, BridgeError> {
    let Some(row) = find_withdrawal_row(conn, id)? else {
        return Ok(None);
    };
    u64::try_from(row.current_epoch)
        .map(Some)
        .map_err(|err| BridgeError::ValueConversion(format!("current_epoch overflow: {err}")))
}

fn persist_withdrawal_requests(
    conn: &mut SqliteConnection,
    requests: &[NockWithdrawalRequestKernelData],
) -> Result<(), BridgeError> {
    conn.immediate_transaction::<_, anyhow::Error, _>(move |conn| {
        persist_withdrawal_requests_in_transaction(conn, requests)?;
        Ok(())
    })
    .map_err(BridgeError::from)
}

fn persist_withdrawal_requests_in_transaction(
    conn: &mut SqliteConnection,
    requests: &[NockWithdrawalRequestKernelData],
) -> Result<(), BridgeError> {
    use crate::withdrawal::schema::withdrawals::dsl as withdrawal_dsl;

    let requests = requests.to_vec();
    let last_persisted = withdrawals::table
        .select((
            withdrawal_dsl::base_batch_end,
            withdrawal_dsl::base_event_id,
            withdrawal_dsl::withdrawal_nonce,
        ))
        .order_by((
            withdrawal_dsl::base_batch_end.desc(),
            withdrawal_dsl::base_event_id.desc(),
        ))
        .first::<(i64, Vec<u8>, i64)>(conn)
        .optional()
        .map_err(|err| BridgeError::Runtime(format!("withdrawal head lookup failed: {err}")))?;
    let mut max_existing_key = None::<(u64, Vec<u8>)>;
    let mut next_withdrawal_nonce = 1_u64;
    if let Some((base_batch_end, base_event_id, withdrawal_nonce)) = last_persisted {
        max_existing_key = Some((
            u64::try_from(base_batch_end).map_err(|err| {
                BridgeError::ValueConversion(format!("base_batch_end overflow: {err}"))
            })?,
            base_event_id,
        ));
        next_withdrawal_nonce = u64::try_from(withdrawal_nonce)
            .map_err(|err| {
                BridgeError::ValueConversion(format!("withdrawal nonce overflow: {err}"))
            })?
            .checked_add(1)
            .ok_or_else(|| BridgeError::Runtime("withdrawal nonce overflow".into()))?;
    }

    let mut new_requests = Vec::new();
    let mut seen_new = HashMap::<(Vec<u8>, Vec<u8>), NockWithdrawalRequestKernelData>::new();
    for request in requests {
        let key = (
            request.as_of.to_be_limb_bytes().to_vec(),
            request.base_event_id.0.clone(),
        );
        if let Some(prior) = seen_new.get(&key) {
            if prior.recipient != request.recipient
                || prior.amount != request.amount
                || prior.base_batch_end != request.base_batch_end
            {
                metrics::init_metrics()
                    .withdrawal_projection_immutable_mismatch
                    .increment();
                return Err(BridgeError::Runtime(format!(
                    "duplicate withdrawal request batch entry does not match for {:?}",
                    request.withdrawal_id()
                )));
            }
            continue;
        }

        if let Some(existing) = find_withdrawal_row(conn, &request.withdrawal_id())? {
            if !existing.matches_request(&request)? {
                metrics::init_metrics()
                    .withdrawal_projection_immutable_mismatch
                    .increment();
                return Err(BridgeError::Runtime(format!(
                    "stored withdrawal request does not match kernel request for {:?}",
                    request.withdrawal_id()
                )));
            }
            continue;
        }

        if let Some((max_base_batch_end, max_base_event_id)) = &max_existing_key {
            if canonical_withdrawal_ordering_cmp(
                request.base_batch_end, &request.base_event_id.0, *max_base_batch_end,
                max_base_event_id,
            )
            .is_lt()
            {
                return Err(BridgeError::Runtime(format!(
                    "new withdrawal request {:?} would sort before already persisted request history",
                    request.withdrawal_id()
                )));
            }
        }

        seen_new.insert(key, request.clone());
        new_requests.push(request.clone());
    }

    new_requests.sort_by(|left, right| {
        canonical_withdrawal_ordering_cmp(
            left.base_batch_end, &left.base_event_id.0, right.base_batch_end,
            &right.base_event_id.0,
        )
    });

    for request in new_requests {
        let created_at = now_unix_secs()?;
        let row = NewWithdrawalRow {
            base_as_of: request.as_of.to_be_limb_bytes().to_vec(),
            base_event_id: request.base_event_id.0.clone(),
            recipient: request.recipient.to_be_limb_bytes().to_vec(),
            gross_burned_amount: i64::try_from(request.amount)
                .map_err(|err| BridgeError::ValueConversion(format!("amount overflow: {err}")))?,
            base_batch_end: i64::try_from(request.base_batch_end).map_err(|err| {
                BridgeError::ValueConversion(format!("base_batch_end overflow: {err}"))
            })?,
            withdrawal_nonce: i64::try_from(next_withdrawal_nonce).map_err(|err| {
                BridgeError::ValueConversion(format!("withdrawal nonce overflow: {err}"))
            })?,
            current_epoch: 0,
            proposal_hash: None,
            peer_commit_certificate: None,
            state: state_tag(WithdrawalState::Pending).to_string(),
            turn_started_base_height: None,
            submitted_tx_name: None,
            submitted_tx_hash: None,
            submitted_at: None,
            confirmed_height: None,
            confirmed_block_id: None,
            created_at,
            updated_at: created_at,
        };
        diesel::insert_into(withdrawals::table)
            .values(&row)
            .execute(conn)
            .map_err(|err| {
                BridgeError::Runtime(format!("withdrawal request insert failed: {err}"))
            })?;
        next_withdrawal_nonce = next_withdrawal_nonce
            .checked_add(1)
            .ok_or_else(|| BridgeError::Runtime("withdrawal nonce overflow".into()))?;
    }
    Ok(())
}

fn load_sorted_withdrawal_requests(
    conn: &mut SqliteConnection,
) -> Result<Vec<TrackedWithdrawalRequest>, BridgeError> {
    use crate::withdrawal::schema::withdrawals::dsl as withdrawal_dsl;

    withdrawals::table
        .filter(withdrawal_dsl::state.ne(state_tag(WithdrawalState::Confirmed)))
        .order_by(withdrawal_dsl::withdrawal_nonce.asc())
        .load::<WithdrawalRow>(conn)
        .map_err(|err| BridgeError::Runtime(format!("withdrawal row load failed: {err}")))?
        .into_iter()
        .map(WithdrawalRow::try_into_tracked_request)
        .collect()
}

fn validate_and_stage_prepared(
    conn: &mut SqliteConnection,
    proposal: &WithdrawalProposalData,
) -> Result<WithdrawalProposalValidationOutcome, WithdrawalProposalValidationError> {
    let Some(withdrawal) = find_withdrawal_row(conn, &proposal.id)? else {
        return Err(WithdrawalProposalValidationError::UnknownWithdrawal {
            id: proposal.id.clone(),
        });
    };
    let state = parse_state_tag(&withdrawal.state)?;
    if state == WithdrawalState::Confirmed {
        return Err(WithdrawalProposalValidationError::UnknownWithdrawal {
            id: proposal.id.clone(),
        });
    }

    let tracked = withdrawal.clone().try_into_tracked_request()?;
    if tracked.recipient != proposal.recipient {
        return Err(WithdrawalProposalValidationError::WithdrawalMismatch {
            id: proposal.id.clone(),
            field: "recipient",
        });
    }
    if tracked.amount != proposal.burned_amount {
        return Err(WithdrawalProposalValidationError::WithdrawalMismatch {
            id: proposal.id.clone(),
            field: "burned_amount",
        });
    }
    if tracked.base_batch_end != proposal.base_batch_end {
        return Err(WithdrawalProposalValidationError::WithdrawalMismatch {
            id: proposal.id.clone(),
            field: "base_batch_end",
        });
    }

    let current_epoch = u64::try_from(withdrawal.current_epoch)
        .map_err(|err| BridgeError::ValueConversion(format!("current_epoch overflow: {err}")))?;
    if proposal.epoch > current_epoch && state != WithdrawalState::Pending {
        return Err(WithdrawalProposalValidationError::LiveAttemptExists {
            id: proposal.id.clone(),
            current_epoch,
            live_state: state,
            received_epoch: proposal.epoch,
        });
    }
    if proposal.epoch != current_epoch {
        return Err(WithdrawalProposalValidationError::NonContiguousEpoch {
            id: proposal.id.clone(),
            expected_epoch: current_epoch,
            received_epoch: proposal.epoch,
        });
    }

    if let Some(existing_hash) = withdrawal.proposal_hash.as_deref() {
        let proposal_hash = proposal.proposal_hash()?;
        if matches!(
            state,
            WithdrawalState::Prepared
                | WithdrawalState::PeerCanonical
                | WithdrawalState::Authorized
                | WithdrawalState::MempoolAccepted
        ) && existing_hash != proposal_hash
        {
            return Err(WithdrawalProposalValidationError::SameEpochEquivocation {
                id: proposal.id.clone(),
                epoch: proposal.epoch,
            });
        }
        if existing_hash == proposal_hash && state != WithdrawalState::Prepared {
            return Ok(WithdrawalProposalValidationOutcome::Replay);
        }
    }

    mark_proposal_prepared(conn, proposal)?;
    Ok(WithdrawalProposalValidationOutcome::Inserted)
}

fn mark_proposal_prepared(
    conn: &mut SqliteConnection,
    proposal: &WithdrawalProposalData,
) -> Result<(), BridgeError> {
    update_withdrawal_state(
        conn,
        proposal,
        WithdrawalState::Prepared,
        Some(proposal.proposal_hash()?),
        None,
        None,
        None,
    )
}

fn mark_proposal_canonical(
    conn: &mut SqliteConnection,
    proposal: &WithdrawalProposalData,
    commit_certificate: Option<Vec<u8>>,
) -> Result<(), BridgeError> {
    update_withdrawal_state(
        conn,
        proposal,
        WithdrawalState::PeerCanonical,
        Some(proposal.proposal_hash()?),
        commit_certificate,
        None,
        None,
    )
}

fn mark_proposal_authorized(
    conn: &mut SqliteConnection,
    proposal: &WithdrawalProposalData,
) -> Result<(), BridgeError> {
    update_withdrawal_state(
        conn,
        proposal,
        WithdrawalState::Authorized,
        Some(proposal.proposal_hash()?),
        None,
        Some(transaction_name(&proposal.transaction).to_string()),
        None,
    )
}

fn mark_proposal_mempool_accepted(
    conn: &mut SqliteConnection,
    proposal: &WithdrawalProposalData,
) -> Result<(), BridgeError> {
    update_withdrawal_state(
        conn,
        proposal,
        WithdrawalState::MempoolAccepted,
        Some(proposal.proposal_hash()?),
        None,
        Some(transaction_name(&proposal.transaction).to_string()),
        Some(proposal.proposal_hash()?),
    )
}

fn mark_proposal_confirmed(
    conn: &mut SqliteConnection,
    proposal: &WithdrawalProposalData,
    confirmed_height: u64,
    confirmed_block_id: &Tip5Hash,
) -> Result<(), BridgeError> {
    let Some(withdrawal) = find_withdrawal_row(conn, &proposal.id)? else {
        return Err(BridgeError::Runtime(format!(
            "missing tracked withdrawal row for proposal {:?}",
            proposal.id
        )));
    };
    let updated_at = now_unix_secs()?;
    diesel::update(withdrawals::table.find(withdrawal.id))
        .set((
            withdrawals::current_epoch.eq(i64::try_from(proposal.epoch)
                .map_err(|err| BridgeError::ValueConversion(format!("epoch too large: {err}")))?),
            withdrawals::proposal_hash.eq(Some(proposal.proposal_hash()?)),
            withdrawals::state.eq(state_tag(WithdrawalState::Confirmed).to_string()),
            withdrawals::turn_started_base_height.eq::<Option<i64>>(None),
            withdrawals::submitted_tx_name
                .eq(Some(transaction_name(&proposal.transaction).to_string())),
            withdrawals::submitted_tx_hash.eq(Some(proposal.proposal_hash()?)),
            withdrawals::submitted_at.eq(Some(updated_at)),
            withdrawals::confirmed_height.eq(Some(i64::try_from(confirmed_height).map_err(
                |err| BridgeError::ValueConversion(format!("confirmed height overflow: {err}")),
            )?)),
            withdrawals::confirmed_block_id
                .eq(Some(confirmed_block_id.to_be_limb_bytes().to_vec())),
            withdrawals::updated_at.eq(updated_at),
        ))
        .execute(conn)
        .map_err(|err| {
            BridgeError::Runtime(format!("withdrawal confirmed update failed: {err}"))
        })?;
    Ok(())
}

fn mark_proposal_expired(
    conn: &mut SqliteConnection,
    proposal: &WithdrawalProposalData,
) -> Result<(), BridgeError> {
    let Some(withdrawal) = find_withdrawal_row(conn, &proposal.id)? else {
        return Ok(());
    };
    let state = parse_state_tag(&withdrawal.state)?;
    if state != WithdrawalState::Prepared {
        return Err(BridgeError::Runtime(format!(
            "cannot expire withdrawal {:?} from non-pre-canonical state {}",
            proposal.id,
            state.as_str()
        )));
    }
    let updated_at = now_unix_secs()?;
    expire_prepared_row(conn, withdrawal.id, proposal.epoch, updated_at)?;
    Ok(())
}

fn acquire_withdrawal_assembly(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
    epoch: u64,
    updated_at: i64,
    turn_started_base_height: u64,
) -> Result<AcquireWithdrawalAssemblyOutcome, BridgeError> {
    conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
        if let Some(active) = fetch_active_assembly_withdrawal(conn)? {
            let active_id = active.withdrawal_id()?;
            if active_id != *id {
                return Ok(AcquireWithdrawalAssemblyOutcome::Busy { active: active_id });
            }
        }

        let Some(existing) = find_withdrawal_row(conn, id)? else {
            return Err(anyhow::Error::from(BridgeError::Runtime(format!(
                "missing tracked withdrawal row for assembly {:?}",
                id
            ))));
        };
        let state = parse_state_tag(&existing.state)?;
        if state != WithdrawalState::Pending {
            return Ok(AcquireWithdrawalAssemblyOutcome::AlreadyTracked {
                id: id.clone(),
                state,
            });
        }

        diesel::update(withdrawals::table.find(existing.id))
            .set((
                withdrawals::current_epoch.eq(i64::try_from(epoch).map_err(|err| {
                    BridgeError::ValueConversion(format!("epoch too large: {err}"))
                })?),
                withdrawals::proposal_hash.eq::<Option<String>>(None),
                withdrawals::peer_commit_certificate.eq::<Option<Vec<u8>>>(None),
                withdrawals::state.eq(state_tag(WithdrawalState::Assembling).to_string()),
                withdrawals::turn_started_base_height.eq(Some(
                    i64::try_from(turn_started_base_height).map_err(|err| {
                        BridgeError::ValueConversion(format!(
                            "turn_started_base_height too large: {err}"
                        ))
                    })?,
                )),
                withdrawals::submitted_tx_name.eq::<Option<String>>(None),
                withdrawals::submitted_tx_hash.eq::<Option<String>>(None),
                withdrawals::submitted_at.eq::<Option<i64>>(None),
                withdrawals::confirmed_height.eq::<Option<i64>>(None),
                withdrawals::confirmed_block_id.eq::<Option<Vec<u8>>>(None),
                withdrawals::updated_at.eq(updated_at),
            ))
            .execute(conn)
            .map_err(|err| {
                BridgeError::Runtime(format!("assembly lock claim update failed: {err}"))
            })?;

        Ok(AcquireWithdrawalAssemblyOutcome::Acquired)
    })
    .map_err(BridgeError::from)
}

fn release_assembly_lock(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
    updated_at: i64,
) -> Result<(), BridgeError> {
    let Some(existing) = find_withdrawal_row(conn, id)? else {
        return Ok(());
    };
    if parse_state_tag(&existing.state)? != WithdrawalState::Assembling {
        return Ok(());
    }

    diesel::update(withdrawals::table.find(existing.id))
        .set((
            withdrawals::state.eq(state_tag(WithdrawalState::Pending).to_string()),
            withdrawals::turn_started_base_height.eq::<Option<i64>>(None),
            withdrawals::updated_at.eq(updated_at),
        ))
        .execute(conn)
        .map_err(|err| BridgeError::Runtime(format!("assembly lock release failed: {err}")))?;
    Ok(())
}

fn recover_startup_assembly_locks(
    conn: &mut SqliteConnection,
    updated_at: i64,
) -> Result<u64, BridgeError> {
    let released = diesel::update(withdrawals::table.filter(withdrawals::state.eq_any([
        state_tag(WithdrawalState::Assembling).to_string(),
        state_tag(WithdrawalState::Prepared).to_string(),
    ])))
    .set((
        withdrawals::proposal_hash.eq::<Option<String>>(None),
        withdrawals::peer_commit_certificate.eq::<Option<Vec<u8>>>(None),
        withdrawals::state.eq(state_tag(WithdrawalState::Pending).to_string()),
        withdrawals::turn_started_base_height.eq::<Option<i64>>(None),
        withdrawals::updated_at.eq(updated_at),
    ))
    .execute(conn)
    .map_err(|err| BridgeError::Runtime(format!("startup assembly lock recovery failed: {err}")))?;

    u64::try_from(released)
        .map_err(|err| BridgeError::ValueConversion(format!("released row count overflow: {err}")))
}

fn release_stale_assembly_lock(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
    epoch: u64,
    updated_at: i64,
) -> Result<bool, BridgeError> {
    let Some(existing) = find_withdrawal_row(conn, id)? else {
        return Ok(false);
    };
    if parse_state_tag(&existing.state)? != WithdrawalState::Assembling
        || u64::try_from(existing.current_epoch)
            .map_err(|err| BridgeError::ValueConversion(format!("current_epoch overflow: {err}")))?
            != epoch
    {
        return Ok(false);
    }

    diesel::update(withdrawals::table.find(existing.id))
        .set((
            withdrawals::state.eq(state_tag(WithdrawalState::Pending).to_string()),
            withdrawals::turn_started_base_height.eq::<Option<i64>>(None),
            withdrawals::updated_at.eq(updated_at),
        ))
        .execute(conn)
        .map_err(|err| {
            BridgeError::Runtime(format!("stale assembly lock release failed: {err}"))
        })?;
    Ok(true)
}

fn reconcile_pending_epoch(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
    sequencer_epoch: u64,
    updated_at: i64,
) -> Result<bool, BridgeError> {
    let Some(existing) = find_withdrawal_row(conn, id)? else {
        return Ok(false);
    };
    if parse_state_tag(&existing.state)? != WithdrawalState::Pending {
        return Ok(false);
    }
    let current_epoch = u64::try_from(existing.current_epoch)
        .map_err(|err| BridgeError::ValueConversion(format!("current_epoch overflow: {err}")))?;
    if current_epoch == sequencer_epoch {
        return Ok(false);
    }

    diesel::update(withdrawals::table.find(existing.id))
        .set((
            withdrawals::current_epoch.eq(i64::try_from(sequencer_epoch)
                .map_err(|err| BridgeError::ValueConversion(format!("epoch too large: {err}")))?),
            withdrawals::proposal_hash.eq::<Option<String>>(None),
            withdrawals::peer_commit_certificate.eq::<Option<Vec<u8>>>(None),
            withdrawals::turn_started_base_height.eq::<Option<i64>>(None),
            withdrawals::submitted_tx_name.eq::<Option<String>>(None),
            withdrawals::submitted_tx_hash.eq::<Option<String>>(None),
            withdrawals::submitted_at.eq::<Option<i64>>(None),
            withdrawals::confirmed_height.eq::<Option<i64>>(None),
            withdrawals::confirmed_block_id.eq::<Option<Vec<u8>>>(None),
            withdrawals::updated_at.eq(updated_at),
        ))
        .execute(conn)
        .map_err(|err| {
            BridgeError::Runtime(format!(
                "pending withdrawal epoch reconciliation failed: {err}"
            ))
        })?;
    Ok(true)
}

fn reconcile_prepared_with_pending_sequencer(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
    sequencer_epoch: u64,
    sequencer_handoff_index: u64,
    sequencer_turn_started_base_height: Option<u64>,
    updated_at: i64,
) -> Result<bool, BridgeError> {
    let Some(existing) = find_withdrawal_row(conn, id)? else {
        return Ok(false);
    };
    if parse_state_tag(&existing.state)? != WithdrawalState::Prepared {
        return Ok(false);
    }
    let current_epoch = u64::try_from(existing.current_epoch)
        .map_err(|err| BridgeError::ValueConversion(format!("current_epoch overflow: {err}")))?;
    if current_epoch != sequencer_epoch {
        expire_prepared_row(conn, existing.id, sequencer_epoch, updated_at)?;
        return Ok(true);
    }
    let Some(sequencer_turn_started_base_height) = sequencer_turn_started_base_height else {
        return Ok(false);
    };
    let handoff_advanced = match existing.turn_started_base_height {
        Some(local_turn_started_base_height) => {
            let local_turn_started_base_height = u64::try_from(local_turn_started_base_height)
                .map_err(|err| {
                    BridgeError::ValueConversion(format!(
                        "turn_started_base_height overflow: {err}"
                    ))
                })?;
            sequencer_turn_started_base_height > local_turn_started_base_height
        }
        None => sequencer_handoff_index > 0,
    };
    if !handoff_advanced {
        return Ok(false);
    }

    expire_prepared_row(conn, existing.id, sequencer_epoch, updated_at)?;
    Ok(true)
}

fn expire_prepared_row(
    conn: &mut SqliteConnection,
    row_id: i64,
    epoch: u64,
    updated_at: i64,
) -> Result<(), BridgeError> {
    diesel::update(withdrawals::table.find(row_id))
        .set((
            withdrawals::current_epoch.eq(i64::try_from(epoch)
                .map_err(|err| BridgeError::ValueConversion(format!("epoch too large: {err}")))?),
            withdrawals::proposal_hash.eq::<Option<String>>(None),
            withdrawals::peer_commit_certificate.eq::<Option<Vec<u8>>>(None),
            withdrawals::state.eq(state_tag(WithdrawalState::Pending).to_string()),
            withdrawals::turn_started_base_height.eq::<Option<i64>>(None),
            withdrawals::submitted_tx_name.eq::<Option<String>>(None),
            withdrawals::submitted_tx_hash.eq::<Option<String>>(None),
            withdrawals::submitted_at.eq::<Option<i64>>(None),
            withdrawals::confirmed_height.eq::<Option<i64>>(None),
            withdrawals::confirmed_block_id.eq::<Option<Vec<u8>>>(None),
            withdrawals::updated_at.eq(updated_at),
        ))
        .execute(conn)
        .map_err(|err| BridgeError::Runtime(format!("withdrawal expired update failed: {err}")))?;
    Ok(())
}

fn update_withdrawal_state(
    conn: &mut SqliteConnection,
    proposal: &WithdrawalProposalData,
    state: WithdrawalState,
    proposal_hash: Option<String>,
    commit_certificate: Option<Vec<u8>>,
    submitted_tx_name: Option<String>,
    submitted_tx_hash: Option<String>,
) -> Result<(), BridgeError> {
    let Some(withdrawal) = find_withdrawal_row(conn, &proposal.id)? else {
        return Err(BridgeError::Runtime(format!(
            "missing tracked withdrawal row for proposal {:?}",
            proposal.id
        )));
    };
    let turn_started_base_height = if state == WithdrawalState::Prepared {
        withdrawal.turn_started_base_height
    } else {
        None
    };
    let peer_commit_certificate = commit_certificate.or(withdrawal.peer_commit_certificate);
    let updated_at = now_unix_secs()?;
    diesel::update(withdrawals::table.find(withdrawal.id))
        .set((
            withdrawals::current_epoch.eq(i64::try_from(proposal.epoch)
                .map_err(|err| BridgeError::ValueConversion(format!("epoch too large: {err}")))?),
            withdrawals::proposal_hash.eq(proposal_hash),
            withdrawals::peer_commit_certificate.eq(peer_commit_certificate),
            withdrawals::state.eq(state_tag(state).to_string()),
            withdrawals::turn_started_base_height.eq(turn_started_base_height),
            withdrawals::submitted_tx_name.eq(submitted_tx_name),
            withdrawals::submitted_tx_hash.eq(submitted_tx_hash),
            withdrawals::submitted_at.eq(None::<i64>),
            withdrawals::confirmed_height.eq(None::<i64>),
            withdrawals::confirmed_block_id.eq(None::<Vec<u8>>),
            withdrawals::updated_at.eq(updated_at),
        ))
        .execute(conn)
        .map_err(|err| BridgeError::Runtime(format!("withdrawal state update failed: {err}")))?;
    Ok(())
}

fn find_withdrawal_row(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
) -> Result<Option<WithdrawalRow>, BridgeError> {
    use crate::withdrawal::schema::withdrawals::dsl as withdrawal_dsl;

    withdrawals::table
        .filter(withdrawal_dsl::base_as_of.eq(id.as_of.to_be_limb_bytes().to_vec()))
        .filter(withdrawal_dsl::base_event_id.eq(id.base_event_id.0.clone()))
        .first::<WithdrawalRow>(conn)
        .optional()
        .map_err(|err| BridgeError::Runtime(format!("withdrawal row lookup failed: {err}")))
}

fn find_withdrawal_row_by_nonce(
    conn: &mut SqliteConnection,
    withdrawal_nonce: u64,
) -> Result<Option<WithdrawalRow>, BridgeError> {
    use crate::withdrawal::schema::withdrawals::dsl as withdrawal_dsl;

    withdrawals::table
        .filter(
            withdrawal_dsl::withdrawal_nonce.eq(i64::try_from(withdrawal_nonce).map_err(
                |err| BridgeError::ValueConversion(format!("withdrawal nonce overflow: {err}")),
            )?),
        )
        .first::<WithdrawalRow>(conn)
        .optional()
        .map_err(|err| BridgeError::Runtime(format!("withdrawal nonce lookup failed: {err}")))
}

fn fetch_active_assembly_withdrawal(
    conn: &mut SqliteConnection,
) -> Result<Option<WithdrawalRow>, BridgeError> {
    use crate::withdrawal::schema::withdrawals::dsl as withdrawal_dsl;

    withdrawals::table
        .filter(withdrawal_dsl::state.eq(state_tag(WithdrawalState::Assembling)))
        .order_by(withdrawal_dsl::updated_at.asc())
        .first::<WithdrawalRow>(conn)
        .optional()
        .map_err(|err| {
            BridgeError::Runtime(format!("active assembly withdrawal lookup failed: {err}"))
        })
}

fn load_live_withdrawals(
    conn: &mut SqliteConnection,
) -> Result<Vec<LiveWithdrawalView>, BridgeError> {
    load_live_withdrawals_filtered(conn, None)
}

fn load_live_withdrawals_filtered(
    conn: &mut SqliteConnection,
    state_filter: Option<WithdrawalState>,
) -> Result<Vec<LiveWithdrawalView>, BridgeError> {
    use crate::withdrawal::schema::withdrawals::dsl as withdrawal_dsl;

    let mut query = withdrawals::table.into_boxed::<diesel::sqlite::Sqlite>();
    if let Some(state) = state_filter {
        query = query.filter(withdrawal_dsl::state.eq(state_tag(state).to_string()));
    }
    let rows = query
        .order_by((
            withdrawal_dsl::base_batch_end.asc(),
            withdrawal_dsl::base_event_id.asc(),
        ))
        .load::<WithdrawalRow>(conn)
        .map_err(|err| BridgeError::Runtime(format!("live withdrawal load failed: {err}")))?;

    rows.into_iter()
        .filter_map(|row| {
            match stored_withdrawal_nonce(&row)
                .and_then(|withdrawal_nonce| build_live_withdrawal_view(row, withdrawal_nonce))
            {
                Ok(Some(row)) => Some(Ok(row)),
                Ok(None) => None,
                Err(err) => Some(Err(err)),
            }
        })
        .collect()
}

fn fetch_live_withdrawal_view(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
) -> Result<Option<LiveWithdrawalView>, BridgeError> {
    let Some(row) = find_withdrawal_row(conn, id)? else {
        return Ok(None);
    };
    let withdrawal_nonce = stored_withdrawal_nonce(&row)?;
    build_live_withdrawal_view(row, withdrawal_nonce)
}

fn fetch_live_withdrawal_view_by_nonce(
    conn: &mut SqliteConnection,
    withdrawal_nonce: u64,
) -> Result<Option<LiveWithdrawalView>, BridgeError> {
    let Some(row) = find_withdrawal_row_by_nonce(conn, withdrawal_nonce)? else {
        return Ok(None);
    };
    build_live_withdrawal_view(row, withdrawal_nonce)
}

fn fetch_withdrawal_tui_row_by_nonce(
    conn: &mut SqliteConnection,
    withdrawal_nonce: u64,
) -> Result<Option<WithdrawalTuiRow>, BridgeError> {
    let Some(row) = find_withdrawal_row_by_nonce(conn, withdrawal_nonce)? else {
        return Ok(None);
    };
    withdrawal_row_to_tui_row(row).map(Some)
}

fn load_withdrawal_tui_rows(
    conn: &mut SqliteConnection,
    center_nonce: Option<u64>,
    limit: usize,
) -> Result<Vec<WithdrawalTuiRow>, BridgeError> {
    use crate::withdrawal::schema::withdrawals::dsl as withdrawal_dsl;

    let bounded_limit = limit.clamp(1, 50);
    let mut query = withdrawals::table
        .filter(withdrawal_dsl::state.ne(state_tag(WithdrawalState::Confirmed)))
        .into_boxed::<diesel::sqlite::Sqlite>();
    if let Some(center_nonce) = center_nonce {
        let half_window = u64::try_from(bounded_limit / 2).unwrap_or_default();
        let lower_nonce = center_nonce.saturating_sub(half_window);
        query = query.filter(withdrawal_dsl::withdrawal_nonce.ge(
            i64::try_from(lower_nonce).map_err(|err| {
                BridgeError::ValueConversion(format!("withdrawal nonce overflow: {err}"))
            })?,
        ));
    }
    let rows = query
        .order(withdrawal_dsl::withdrawal_nonce.asc())
        .limit(i64::try_from(bounded_limit).map_err(|err| {
            BridgeError::ValueConversion(format!("withdrawal TUI limit overflow: {err}"))
        })?)
        .load::<WithdrawalRow>(conn)
        .map_err(|err| BridgeError::Runtime(format!("withdrawal TUI row load failed: {err}")))?;

    rows.into_iter().map(withdrawal_row_to_tui_row).collect()
}

fn load_withdrawal_tui_counts(
    conn: &mut SqliteConnection,
    frontier_nonce: Option<u64>,
) -> Result<WithdrawalTuiCounts, BridgeError> {
    Ok(WithdrawalTuiCounts {
        total_count: count_withdrawals(conn, None, false)?,
        live_count: count_withdrawals(conn, None, true)?,
        ordering_blocking_count: count_ordering_blocking_withdrawals(conn)?,
        pending_count: count_withdrawals(conn, Some(WithdrawalState::Pending), false)?,
        assembling_count: count_withdrawals(conn, Some(WithdrawalState::Assembling), false)?,
        prepared_count: count_withdrawals(conn, Some(WithdrawalState::Prepared), false)?,
        peer_canonical_count: count_withdrawals(conn, Some(WithdrawalState::PeerCanonical), false)?,
        authorized_count: count_withdrawals(conn, Some(WithdrawalState::Authorized), false)?,
        mempool_accepted_count: count_withdrawals(
            conn,
            Some(WithdrawalState::MempoolAccepted),
            false,
        )?,
        confirmed_count: count_withdrawals(conn, Some(WithdrawalState::Confirmed), false)?,
        below_frontier_count: count_relative_to_frontier(conn, frontier_nonce, true)?,
        above_frontier_count: count_relative_to_frontier(conn, frontier_nonce, false)?,
    })
}

fn count_withdrawals(
    conn: &mut SqliteConnection,
    state_filter: Option<WithdrawalState>,
    exclude_confirmed: bool,
) -> Result<u64, BridgeError> {
    use crate::withdrawal::schema::withdrawals::dsl as withdrawal_dsl;

    let mut query = withdrawals::table.into_boxed::<diesel::sqlite::Sqlite>();
    if let Some(state) = state_filter {
        query = query.filter(withdrawal_dsl::state.eq(state_tag(state).to_string()));
    }
    if exclude_confirmed {
        query = query.filter(withdrawal_dsl::state.ne(state_tag(WithdrawalState::Confirmed)));
    }
    let count = query
        .select(diesel::dsl::count_star())
        .first::<i64>(conn)
        .map_err(|err| BridgeError::Runtime(format!("withdrawal TUI count failed: {err}")))?;
    u64::try_from(count)
        .map_err(|err| BridgeError::ValueConversion(format!("withdrawal count overflow: {err}")))
}

fn count_ordering_blocking_withdrawals(conn: &mut SqliteConnection) -> Result<u64, BridgeError> {
    use crate::withdrawal::schema::withdrawals::dsl as withdrawal_dsl;

    let blocking_states = [
        state_tag(WithdrawalState::Pending).to_string(),
        state_tag(WithdrawalState::Assembling).to_string(),
        state_tag(WithdrawalState::Prepared).to_string(),
        state_tag(WithdrawalState::PeerCanonical).to_string(),
        state_tag(WithdrawalState::Authorized).to_string(),
    ];
    let count = withdrawals::table
        .filter(withdrawal_dsl::state.eq_any(blocking_states))
        .select(diesel::dsl::count_star())
        .first::<i64>(conn)
        .map_err(|err| BridgeError::Runtime(format!("withdrawal blocking count failed: {err}")))?;
    u64::try_from(count).map_err(|err| {
        BridgeError::ValueConversion(format!("withdrawal blocking count overflow: {err}"))
    })
}

fn count_relative_to_frontier(
    conn: &mut SqliteConnection,
    frontier_nonce: Option<u64>,
    below: bool,
) -> Result<u64, BridgeError> {
    use crate::withdrawal::schema::withdrawals::dsl as withdrawal_dsl;

    let Some(frontier_nonce) = frontier_nonce else {
        return Ok(0);
    };
    let frontier_nonce = i64::try_from(frontier_nonce)
        .map_err(|err| BridgeError::ValueConversion(format!("withdrawal nonce overflow: {err}")))?;
    let mut query = withdrawals::table
        .filter(withdrawal_dsl::state.ne(state_tag(WithdrawalState::Confirmed)))
        .into_boxed::<diesel::sqlite::Sqlite>();
    query = if below {
        query.filter(withdrawal_dsl::withdrawal_nonce.lt(frontier_nonce))
    } else {
        query.filter(withdrawal_dsl::withdrawal_nonce.gt(frontier_nonce))
    };
    let count = query
        .select(diesel::dsl::count_star())
        .first::<i64>(conn)
        .map_err(|err| {
            BridgeError::Runtime(format!("withdrawal frontier-relative count failed: {err}"))
        })?;
    u64::try_from(count).map_err(|err| {
        BridgeError::ValueConversion(format!(
            "withdrawal frontier-relative count overflow: {err}"
        ))
    })
}

fn withdrawal_row_to_tui_row(row: WithdrawalRow) -> Result<WithdrawalTuiRow, BridgeError> {
    let id = row.withdrawal_id()?;
    let state = parse_state_tag(&row.state)?;
    let recipient = Some(
        Tip5Hash::from_be_limb_bytes(&row.recipient)
            .map_err(|err| BridgeError::Runtime(format!("invalid withdrawal recipient: {err}")))?,
    );
    let amount = Some(
        u64::try_from(row.gross_burned_amount)
            .map_err(|err| BridgeError::ValueConversion(format!("amount overflow: {err}")))?,
    );
    let base_batch_end =
        Some(u64::try_from(row.base_batch_end).map_err(|err| {
            BridgeError::ValueConversion(format!("base_batch_end overflow: {err}"))
        })?);
    let current_epoch = u64::try_from(row.current_epoch)
        .map_err(|err| BridgeError::ValueConversion(format!("current_epoch overflow: {err}")))?;
    let turn_started_base_height = row
        .turn_started_base_height
        .map(|height| {
            u64::try_from(height).map_err(|err| {
                BridgeError::ValueConversion(format!("turn_started_base_height overflow: {err}"))
            })
        })
        .transpose()?;
    Ok(WithdrawalTuiRow {
        id,
        recipient,
        amount,
        base_batch_end,
        withdrawal_nonce: stored_withdrawal_nonce(&row)?,
        current_epoch,
        proposal_hash: row.proposal_hash,
        has_commit_certificate: row.peer_commit_certificate.is_some(),
        has_authorized_transaction: row.submitted_tx_name.is_some(),
        has_submitted_transaction: row.submitted_tx_hash.is_some(),
        turn_started_base_height,
        state,
        updated_at: row.updated_at,
    })
}

fn build_live_withdrawal_view(
    row: WithdrawalRow,
    withdrawal_nonce: u64,
) -> Result<Option<LiveWithdrawalView>, BridgeError> {
    let id = row.withdrawal_id()?;
    let state = parse_state_tag(&row.state)?;
    if !has_active_operator_attempt(state) {
        return Ok(None);
    }
    Ok(Some(LiveWithdrawalView {
        id,
        recipient: Some(
            Tip5Hash::from_be_limb_bytes(&row.recipient).map_err(|err| {
                BridgeError::Runtime(format!("invalid withdrawal recipient: {err}"))
            })?,
        ),
        gross_burned_amount: Some(
            u64::try_from(row.gross_burned_amount)
                .map_err(|err| BridgeError::ValueConversion(format!("amount overflow: {err}")))?,
        ),
        base_batch_end: Some(u64::try_from(row.base_batch_end).map_err(|err| {
            BridgeError::ValueConversion(format!("base_batch_end overflow: {err}"))
        })?),
        withdrawal_nonce: Some(withdrawal_nonce),
        current_epoch: u64::try_from(row.current_epoch).map_err(|err| {
            BridgeError::ValueConversion(format!("current_epoch overflow: {err}"))
        })?,
        proposal_hash: row.proposal_hash,
        peer_commit_certificate: row.peer_commit_certificate,
        authorized_transaction_name: row.submitted_tx_name,
        handoff_index: 0,
        turn_started_base_height: row
            .turn_started_base_height
            .map(|height| {
                u64::try_from(height).map_err(|err| {
                    BridgeError::ValueConversion(format!(
                        "turn_started_base_height overflow: {err}"
                    ))
                })
            })
            .transpose()?,
        submit_attempt_count: 0,
        last_submit_attempt_base_height: None,
        last_submit_error: None,
        state,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}

fn has_active_operator_attempt(state: WithdrawalState) -> bool {
    // "Live" is intentionally narrower than "tracked": a Pending row is a
    // durable Base burn fact waiting for assembly, while a Confirmed row is
    // terminal. Live rows are only active operator attempts:
    // Assembling, Prepared, PeerCanonical, Authorized, or MempoolAccepted.
    !matches!(state, WithdrawalState::Pending | WithdrawalState::Confirmed)
}

fn now_unix_secs() -> Result<i64, BridgeError> {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .map_err(|err| BridgeError::ValueConversion(format!("created_at overflow: {err}")))
}

fn state_tag(state: WithdrawalState) -> &'static str {
    state.as_str()
}

fn parse_state_tag(value: &str) -> Result<WithdrawalState, BridgeError> {
    WithdrawalState::parse(value)
}

fn transaction_name(transaction: &nockchain_types::v1::Transaction) -> &str {
    match transaction {
        nockchain_types::v1::Transaction::V1(tx) => &tx.name,
    }
}

pub(crate) fn reconstruct_withdrawal_proposal(
    tracked: &TrackedWithdrawalRequest,
    artifacts: WithdrawalSequencerProposalArtifacts,
) -> Result<WithdrawalProposalData, BridgeError> {
    if tracked.id.base_event_id != artifacts.id.base_event_id {
        return Err(BridgeError::Runtime(format!(
            "sequencer artifacts base_event_id {:?} does not match tracked withdrawal {:?}",
            artifacts.id, tracked.id
        )));
    }
    if tracked.base_batch_end != artifacts.base_batch_end {
        return Err(BridgeError::Runtime(format!(
            "sequencer artifacts base_batch_end {} does not match tracked withdrawal {:?} base_batch_end {}",
            artifacts.base_batch_end, tracked.id, tracked.base_batch_end
        )));
    }
    let proposal = WithdrawalProposalData {
        id: tracked.id.clone(),
        recipient: tracked.recipient.clone(),
        amount: artifacts.amount,
        burned_amount: tracked.amount,
        base_batch_end: tracked.base_batch_end,
        epoch: artifacts.epoch,
        snapshot: artifacts.snapshot,
        selected_inputs: artifacts.selected_inputs,
        transaction: artifacts.transaction,
    };
    let computed_hash = proposal.proposal_hash()?;
    if computed_hash != artifacts.proposal_hash {
        return Err(BridgeError::Runtime(format!(
            "reconstructed withdrawal proposal hash mismatch for {:?} epoch {}: sequencer {} reconstructed {}",
            proposal.id, proposal.epoch, artifacts.proposal_hash, computed_hash
        )));
    }
    Ok(proposal)
}

fn sqlite_pool(path: &Path) -> Result<Pool, BridgeError> {
    let path_str = path.to_string_lossy();
    let manager = Manager::new(path_str.to_string(), Runtime::Tokio1);
    Pool::builder(manager).build().map_err(|err| {
        BridgeError::Runtime(format!("withdrawal projection pool build failed: {err}"))
    })
}

fn ensure_table_column(
    conn: &mut SqliteConnection,
    table_name: &str,
    column_name: &str,
    column_sql: &str,
) -> Result<(), BridgeError> {
    if sqlite_table_has_column(conn, table_name, column_name)? {
        return Ok(());
    }
    conn.batch_execute(&format!(
        "ALTER TABLE {table_name} ADD COLUMN {column_name} {column_sql};"
    ))
    .map_err(|err| {
        BridgeError::Runtime(format!(
            "failed to add missing column {table_name}.{column_name}: {err}"
        ))
    })?;
    Ok(())
}

fn sqlite_table_has_column(
    conn: &mut SqliteConnection,
    table_name: &str,
    column_name: &str,
) -> Result<bool, BridgeError> {
    #[derive(QueryableByName)]
    struct TableInfoRow {
        #[diesel(sql_type = diesel::sql_types::Text)]
        name: String,
    }

    let pragma = format!("PRAGMA table_info({table_name})");
    let columns = diesel::sql_query(pragma)
        .load::<TableInfoRow>(conn)
        .map_err(|err| BridgeError::Runtime(format!("table_info({table_name}) failed: {err}")))?;
    Ok(columns.iter().any(|row| row.name == column_name))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nockapp::noun::slab::{NockJammer, NounSlab};
    use nockchain_math::belt::Belt;
    use noun_serde::NounDecode;
    use tempfile::tempdir;

    use super::*;
    use crate::withdrawal::types::WithdrawalSnapshot;

    async fn open_registry() -> (tempfile::TempDir, WithdrawalProposalRegistry) {
        let dir = tempdir().expect("tempdir");
        let store = Arc::new(
            WithdrawalProjectionStore::open(dir.path().join("withdrawals.sqlite"))
                .await
                .expect("store"),
        );
        (
            dir,
            WithdrawalProposalRegistry::new_without_transaction_body_validator_for_tests(store),
        )
    }

    fn sample_base_event_id(start: u8) -> crate::shared::types::BaseEventId {
        crate::shared::types::BaseEventId(
            (0..32).map(|offset| start.wrapping_add(offset)).collect(),
        )
    }

    fn sample_request() -> NockWithdrawalRequestKernelData {
        NockWithdrawalRequestKernelData {
            base_event_id: sample_base_event_id(0xaa),
            recipient: Tip5Hash([Belt(101), Belt(102), Belt(103), Belt(104), Belt(105)]),
            amount: 123_456,
            base_batch_end: 777,
            as_of: Tip5Hash([Belt(11), Belt(22), Belt(33), Belt(44), Belt(55)]),
        }
    }

    fn sample_request_with_order(
        base_batch_end: u64,
        event_start: u8,
    ) -> NockWithdrawalRequestKernelData {
        NockWithdrawalRequestKernelData {
            base_event_id: sample_base_event_id(event_start),
            recipient: Tip5Hash([Belt(101), Belt(102), Belt(103), Belt(104), Belt(105)]),
            amount: 123_456 + u64::from(event_start),
            base_batch_end,
            as_of: Tip5Hash([Belt(11), Belt(22), Belt(33), Belt(44), Belt(55)]),
        }
    }

    fn sample_transaction() -> nockchain_types::v1::Transaction {
        const TRANSACTION_JAM: &[u8] = include_bytes!(
            "../../test-fixtures/transactions/9MpGym52AumtwyBxYPyVsWHvcamUYwZkc1Nq7w3cFGF28u8ceVDwt3e.tx"
        );

        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = slab
            .cue_into(TRANSACTION_JAM.to_vec().into())
            .expect("cue transaction fixture");
        let space = nockapp::NounAllocator::noun_space(&slab);
        nockchain_types::v1::Transaction::from_noun(&noun, &space)
            .expect("decode transaction fixture")
    }

    fn sample_proposal(epoch: u64) -> WithdrawalProposalData {
        let request = sample_request();
        sample_proposal_for_request(&request, epoch)
    }

    fn sample_proposal_for_request(
        request: &NockWithdrawalRequestKernelData,
        epoch: u64,
    ) -> WithdrawalProposalData {
        let transaction = sample_transaction();
        WithdrawalProposalData {
            id: request.withdrawal_id(),
            recipient: request.recipient.clone(),
            amount: request.amount.saturating_sub(222),
            burned_amount: request.amount,
            base_batch_end: request.base_batch_end,
            epoch,
            snapshot: WithdrawalSnapshot {
                height: 42 + epoch,
                block_id: Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            },
            selected_inputs: transaction.normalized_input_names(),
            transaction,
        }
    }

    #[tokio::test]
    async fn proposals_are_cached_not_durable() {
        let (dir, registry) = open_registry().await;
        let request = sample_request();
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track");
        let proposal = sample_proposal(0);
        assert_eq!(
            registry
                .validate_and_cache_prepared(&proposal)
                .await
                .expect("validate"),
            WithdrawalProposalValidationOutcome::Inserted
        );
        assert_eq!(
            registry
                .fetch_cached_proposal(proposal.id.clone(), proposal.epoch)
                .await
                .expect("fetch"),
            Some(proposal.clone())
        );

        let reopened = Arc::new(
            WithdrawalProjectionStore::open(dir.path().join("withdrawals.sqlite"))
                .await
                .expect("reopen"),
        );
        let reopened =
            WithdrawalProposalRegistry::new_without_transaction_body_validator_for_tests(reopened);
        assert_eq!(
            reopened
                .fetch_cached_proposal(proposal.id.clone(), proposal.epoch)
                .await
                .expect("fetch reopened"),
            None
        );
    }

    #[tokio::test]
    async fn startup_recovery_releases_prepared_cache_state() {
        let (_dir, registry) = open_registry().await;
        let request = sample_request();
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track");
        let proposal = sample_proposal(0);
        registry
            .validate_and_cache_prepared(&proposal)
            .await
            .expect("validate");
        assert_eq!(
            registry
                .fetch_live_withdrawal(&proposal.id)
                .await
                .expect("live")
                .expect("row")
                .state,
            WithdrawalState::Prepared
        );

        registry
            .restore_tracked_withdrawal_requests()
            .await
            .expect("restore");
        assert!(registry
            .fetch_live_withdrawal(&proposal.id)
            .await
            .expect("live")
            .is_none());
    }

    #[tokio::test]
    async fn signed_contributions_are_memory_only() {
        let (_dir, registry) = open_registry().await;
        let request = sample_request();
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track");
        let proposal = sample_proposal(0);
        registry
            .validate_and_cache_prepared(&proposal)
            .await
            .expect("validate");
        registry
            .record_proposal_signed(&proposal, 7)
            .await
            .expect("record signed");
        let records = registry
            .load_signed_transactions(
                &proposal.id,
                proposal.epoch,
                &proposal.proposal_hash().unwrap(),
            )
            .await
            .expect("records");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].signer_node_id, 7);
    }

    #[tokio::test]
    async fn confirmed_withdrawal_evicts_cached_proposal_and_signatures() {
        let (_dir, registry) = open_registry().await;
        let request = sample_request();
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track");
        let proposal = sample_proposal(0);
        registry
            .validate_and_cache_prepared(&proposal)
            .await
            .expect("validate");
        registry
            .record_proposal_signed(&proposal, 7)
            .await
            .expect("record signed");

        registry
            .mark_proposal_confirmed(
                &proposal,
                123,
                Tip5Hash([Belt(51), Belt(52), Belt(53), Belt(54), Belt(55)]),
            )
            .await
            .expect("mark confirmed");

        assert_eq!(
            registry
                .fetch_cached_proposal(proposal.id.clone(), proposal.epoch)
                .await
                .expect("fetch cached"),
            None
        );
        assert!(registry
            .load_signed_transactions(
                &proposal.id,
                proposal.epoch,
                &proposal.proposal_hash().unwrap(),
            )
            .await
            .expect("records")
            .is_empty());
    }

    #[tokio::test]
    async fn live_row_can_be_loaded_by_nonce() {
        let (_dir, registry) = open_registry().await;
        let request = sample_request();
        let tracked = registry
            .track_withdrawal_request(&request)
            .await
            .expect("track");
        registry
            .acquire_withdrawal_assembly(&tracked.id, 0, 10)
            .await
            .expect("acquire");
        let row = registry
            .fetch_live_withdrawal_by_nonce(tracked.withdrawal_nonce)
            .await
            .expect("fetch")
            .expect("row");
        assert_eq!(row.id, tracked.id);
        assert_eq!(row.state, WithdrawalState::Assembling);
    }

    #[tokio::test]
    async fn prepared_hash_replacement_requires_sequencer_handoff_clear_after_restart() {
        let (dir, registry) = open_registry().await;
        let request = sample_request();
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track");
        let proposal = sample_proposal(0);
        let mut replacement = proposal.clone();
        replacement.amount = replacement.amount.saturating_sub(1);
        assert_ne!(
            proposal.proposal_hash().expect("proposal hash"),
            replacement.proposal_hash().expect("replacement hash")
        );
        registry
            .acquire_withdrawal_assembly(&proposal.id, proposal.epoch, 10)
            .await
            .expect("acquire assembly");
        registry
            .validate_and_cache_prepared(&proposal)
            .await
            .expect("stage prepared proposal");

        let reopened = Arc::new(
            WithdrawalProjectionStore::open(dir.path().join("withdrawals.sqlite"))
                .await
                .expect("reopen"),
        );
        let reopened =
            WithdrawalProposalRegistry::new_without_transaction_body_validator_for_tests(reopened);
        let err = reopened
            .validate_and_cache_prepared(&replacement)
            .await
            .expect_err("same-epoch replacement should be rejected while prepared is live");
        assert!(matches!(
            err,
            WithdrawalProposalValidationError::SameEpochEquivocation { .. }
        ));

        assert!(reopened
            .reconcile_prepared_with_pending_sequencer(&proposal.id, proposal.epoch, 1, Some(20))
            .await
            .expect("expire stale prepared handoff"));
        assert_eq!(
            reopened
                .validate_and_cache_prepared(&replacement)
                .await
                .expect("stage replacement after handoff clear"),
            WithdrawalProposalValidationOutcome::Inserted
        );
    }

    #[tokio::test]
    async fn prepared_epoch_mismatch_reconciles_to_pending_sequencer_epoch_after_restart() {
        let (dir, registry) = open_registry().await;
        let request = sample_request();
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track");
        let drifted = sample_proposal(1);
        registry
            .acquire_withdrawal_assembly(&drifted.id, drifted.epoch, 10)
            .await
            .expect("acquire drifted assembly");
        registry
            .validate_and_cache_prepared(&drifted)
            .await
            .expect("stage drifted prepared proposal");

        let reopened = Arc::new(
            WithdrawalProjectionStore::open(dir.path().join("withdrawals.sqlite"))
                .await
                .expect("reopen"),
        );
        let reopened =
            WithdrawalProposalRegistry::new_without_transaction_body_validator_for_tests(reopened);
        assert!(reopened
            .reconcile_prepared_with_pending_sequencer(&drifted.id, 0, 0, None)
            .await
            .expect("reconcile prepared epoch mismatch"));
        assert_eq!(
            reopened
                .next_expected_epoch(&drifted.id)
                .await
                .expect("next expected epoch"),
            0
        );
        assert_eq!(
            reopened
                .validate_and_cache_prepared(&sample_proposal(0))
                .await
                .expect("stage sequencer epoch replacement"),
            WithdrawalProposalValidationOutcome::Inserted
        );
    }

    #[tokio::test]
    async fn tui_queue_rows_are_nonce_ordered_and_bounded() {
        let (_dir, registry) = open_registry().await;
        let requests = vec![
            sample_request_with_order(30, 0x30),
            sample_request_with_order(10, 0x10),
            sample_request_with_order(40, 0x40),
            sample_request_with_order(20, 0x20),
        ];
        registry
            .track_withdrawal_requests(&requests)
            .await
            .expect("track");

        let all_rows = registry
            .load_tui_rows_around_nonce(None, 3)
            .await
            .expect("rows");
        assert_eq!(
            all_rows
                .iter()
                .map(|row| row.withdrawal_nonce)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(
            all_rows
                .iter()
                .map(|row| row.base_batch_end)
                .collect::<Vec<_>>(),
            vec![Some(10), Some(20), Some(30)]
        );

        let centered_rows = registry
            .load_tui_rows_around_nonce(Some(3), 2)
            .await
            .expect("centered rows");
        assert_eq!(
            centered_rows
                .iter()
                .map(|row| row.withdrawal_nonce)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[tokio::test]
    async fn tui_artifact_presence_uses_metadata_only() {
        let (_dir, registry) = open_registry().await;
        let request = sample_request();
        let tracked = registry
            .track_withdrawal_request(&request)
            .await
            .expect("track");
        let proposal = sample_proposal_for_request(&request, 0);
        registry
            .validate_and_cache_prepared(&proposal)
            .await
            .expect("validate");
        let proposal_hash = proposal.proposal_hash().expect("proposal hash");
        let certificate = WithdrawalCommitCertificate {
            withdrawal_id: None,
            epoch: proposal.epoch,
            proposal_hash,
            signatures: Vec::new(),
        };

        registry
            .mark_proposal_canonical_with_certificate(&proposal, &certificate)
            .await
            .expect("canonical");
        let canonical = registry
            .fetch_tui_row_by_nonce(tracked.withdrawal_nonce)
            .await
            .expect("fetch")
            .expect("canonical row");
        assert!(canonical.has_commit_certificate);
        assert!(!canonical.has_authorized_transaction);
        assert!(!canonical.has_submitted_transaction);

        registry
            .mark_proposal_authorized(&proposal)
            .await
            .expect("authorized");
        let authorized = registry
            .fetch_tui_row_by_nonce(tracked.withdrawal_nonce)
            .await
            .expect("fetch")
            .expect("authorized row");
        assert!(authorized.has_commit_certificate);
        assert!(authorized.has_authorized_transaction);
        assert!(!authorized.has_submitted_transaction);

        registry
            .mark_proposal_mempool_accepted(&proposal)
            .await
            .expect("mempool accepted");
        let submitted = registry
            .fetch_tui_row_by_nonce(tracked.withdrawal_nonce)
            .await
            .expect("fetch")
            .expect("submitted row");
        assert!(submitted.has_commit_certificate);
        assert!(submitted.has_authorized_transaction);
        assert!(submitted.has_submitted_transaction);
    }

    #[tokio::test]
    async fn tui_counts_track_blocking_mempool_and_confirmed_rows() {
        let (_dir, registry) = open_registry().await;
        let requests = vec![
            sample_request_with_order(10, 0x10),
            sample_request_with_order(20, 0x20),
            sample_request_with_order(30, 0x30),
            sample_request_with_order(40, 0x40),
        ];
        registry
            .track_withdrawal_requests(&requests)
            .await
            .expect("track");
        let tracked = registry
            .load_sorted_tracked_withdrawal_requests()
            .await
            .expect("tracked");
        assert_eq!(
            tracked
                .iter()
                .map(|row| row.withdrawal_nonce)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );

        let authorized = sample_proposal_for_request(&requests[1], 0);
        registry
            .validate_and_cache_prepared(&authorized)
            .await
            .expect("prepare authorized");
        registry
            .mark_proposal_authorized(&authorized)
            .await
            .expect("authorized");

        let mempool_accepted = sample_proposal_for_request(&requests[2], 0);
        registry
            .validate_and_cache_prepared(&mempool_accepted)
            .await
            .expect("prepare mempool accepted");
        registry
            .mark_proposal_mempool_accepted(&mempool_accepted)
            .await
            .expect("mempool accepted");

        let confirmed = sample_proposal_for_request(&requests[3], 0);
        registry
            .validate_and_cache_prepared(&confirmed)
            .await
            .expect("prepare confirmed");
        registry
            .mark_proposal_confirmed(
                &confirmed,
                123,
                Tip5Hash([Belt(51), Belt(52), Belt(53), Belt(54), Belt(55)]),
            )
            .await
            .expect("confirmed");

        let counts = registry.load_tui_counts(Some(2)).await.expect("counts");
        assert_eq!(counts.total_count, 4);
        assert_eq!(counts.live_count, 3);
        assert_eq!(counts.ordering_blocking_count, 2);
        assert_eq!(counts.pending_count, 1);
        assert_eq!(counts.authorized_count, 1);
        assert_eq!(counts.mempool_accepted_count, 1);
        assert_eq!(counts.confirmed_count, 1);
        assert_eq!(counts.below_frontier_count, 1);
        assert_eq!(counts.above_frontier_count, 1);
    }
}
