use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use deadpool_diesel::sqlite::{Manager, Pool};
use deadpool_diesel::Runtime;
use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::{Bytes, NounAllocator};
use noun_serde::{NounDecode, NounEncode};
use prost::Message;
use thiserror::Error;

use crate::observability::metrics;
use crate::shared::errors::BridgeError;
use crate::shared::ingress::proto::WithdrawalCommitCertificate;
use crate::shared::types::{AtomBytes, Tip5Hash};
use crate::withdrawal::proposals::TrackedWithdrawalRequest;
use crate::withdrawal::raw_tx as withdrawal_raw_tx;
use crate::withdrawal::sequencer::journal::{
    SequencerJournal, SequencerJournalBaseContext, SequencerJournalConfirmationContext,
    SequencerJournalCursor, SequencerJournalEventType, SequencerJournalHandle,
    SequencerJournalInputName, SequencerJournalNockchainContext, SequencerJournalObjectRef,
    SequencerJournalProposalContext, SequencerJournalRecord, SequencerJournalSubmissionContext,
    SequencerJournalWithdrawal, GENESIS_EVENT_ID,
};
use crate::withdrawal::sequencer::schema::{
    sequencer_journal_cursor, sequencer_withdrawals, withdrawal_reserved_inputs,
    withdrawal_submission_events,
};
use crate::withdrawal::state::{
    LiveWithdrawalView, SignedWithdrawalTransactionRecord, WithdrawalState,
};
use crate::withdrawal::types::{
    normalized_note_names, WithdrawalId, WithdrawalProposalData,
    WithdrawalSequencerProposalArtifacts, WithdrawalSnapshot,
};

const SQLITE_BUSY_TIMEOUT_MS: u64 = 2_000;

/// Sequencer-owned withdrawal coordination state store.
///
/// This store is used by the API-node sequencer process. It owns the durable
/// sequencer tables:
/// - `withdrawal_submission_events`
/// - `sequencer_withdrawals`
/// - `withdrawal_reserved_inputs`

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawalSubmissionEventType {
    WithdrawalOrdered,
    ProposalPrepared,
    ProposalSigned,
    ProposalCanonicalized,
    PrecanonicalHandoff,
    ProposerTurnExpired,
    ProposalAuthorized,
    ProposalRejected,
    ProposalExpired,
    ProposalSuperseded,
    TxSubmitted,
    TxSeenMempoolAccepted,
    MempoolRetryAttempted,
    TxConfirmed,
}

impl WithdrawalSubmissionEventType {
    /// Returns the stable string tag persisted for this submission event type.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::WithdrawalOrdered => "withdrawal_ordered",
            Self::ProposalPrepared => "proposal_prepared",
            Self::ProposalSigned => "proposal_signed",
            Self::ProposalCanonicalized => "proposal_canonicalized",
            Self::PrecanonicalHandoff => "precanonical_handoff",
            Self::ProposerTurnExpired => "proposer_turn_expired",
            Self::ProposalAuthorized => "proposal_authorized",
            Self::ProposalRejected => "proposal_rejected",
            Self::ProposalExpired => "proposal_expired",
            Self::ProposalSuperseded => "proposal_superseded",
            Self::TxSubmitted => "tx_submitted",
            Self::TxSeenMempoolAccepted => "tx_seen_mempool_accepted",
            Self::MempoolRetryAttempted => "mempool_retry_attempted",
            Self::TxConfirmed => "tx_confirmed",
        }
    }

    /// Parses a persisted submission event type tag from SQLite.
    fn parse(value: &str) -> Result<Self, BridgeError> {
        match value {
            "withdrawal_ordered" => Ok(Self::WithdrawalOrdered),
            "proposal_prepared" => Ok(Self::ProposalPrepared),
            "proposal_signed" => Ok(Self::ProposalSigned),
            "proposal_canonicalized" => Ok(Self::ProposalCanonicalized),
            "precanonical_handoff" => Ok(Self::PrecanonicalHandoff),
            "proposer_turn_expired" => Ok(Self::ProposerTurnExpired),
            "proposal_authorized" => Ok(Self::ProposalAuthorized),
            "proposal_rejected" => Ok(Self::ProposalRejected),
            "proposal_expired" => Ok(Self::ProposalExpired),
            "proposal_superseded" => Ok(Self::ProposalSuperseded),
            "tx_submitted" => Ok(Self::TxSubmitted),
            "tx_seen_mempool_accepted" => Ok(Self::TxSeenMempoolAccepted),
            "mempool_retry_attempted" => Ok(Self::MempoolRetryAttempted),
            "tx_confirmed" => Ok(Self::TxConfirmed),
            other => Err(BridgeError::Runtime(format!(
                "unknown withdrawal submission event type: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithdrawalSubmissionEventRecord {
    pub event_id: i64,
    pub created_at: i64,
    pub id: WithdrawalId,
    pub epoch: u64,
    pub proposal_hash: String,
    pub transaction_name: String,
    pub event_type: WithdrawalSubmissionEventType,
    pub signer_node_id: Option<u64>,
    pub commit_certificate: Option<Vec<u8>>,
    pub snapshot: Option<WithdrawalSnapshot>,
    pub confirmed_height: Option<u64>,
    pub confirmed_block_id: Option<Tip5Hash>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SequencerDecision {
    Allowed,
    Rejected(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WithdrawalFrontierCheck {
    pub registered: bool,
    pub is_frontier: bool,
}

impl WithdrawalFrontierCheck {
    pub fn allowed(self) -> bool {
        self.registered && self.is_frontier
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuthorizedRetryPayload {
    pub id: WithdrawalId,
    pub epoch: u64,
    pub proposal_hash: String,
    pub submitted_raw_tx_id: String,
    pub raw_tx_bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedTransactionExport {
    pub submitted_raw_tx_id: String,
    pub transaction_jam: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequencerJournalRecoveryReport {
    pub journal_id: String,
    pub start_sequence: u64,
    pub start_event_id: String,
    pub last_sequence: u64,
    pub last_event_id: String,
    pub replayed_count: u64,
    pub max_replayed_base_height: Option<u64>,
    pub max_replayed_nockchain_height: Option<u64>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WithdrawalSequencerStoreError {
    #[error(
        "withdrawal {requested:?} cannot be authorized while {active:?} remains {active_state:?}"
    )]
    SingleFlightViolation {
        active: Box<WithdrawalId>,
        active_state: WithdrawalState,
        requested: Box<WithdrawalId>,
    },

    #[error("withdrawal {id:?} proposal {proposal_hash} is not peer-canonical in the withdrawal state store")]
    NotPeerCanonical {
        id: Box<WithdrawalId>,
        proposal_hash: Box<str>,
    },

    #[error(
        "withdrawal {id:?} proposal {proposal_hash} is not authorized for submission/confirmation"
    )]
    NotAuthorized {
        id: Box<WithdrawalId>,
        proposal_hash: Box<str>,
    },

    #[error("withdrawal state store failure: {0}")]
    Store(String),
}

impl From<BridgeError> for WithdrawalSequencerStoreError {
    fn from(err: BridgeError) -> Self {
        Self::Store(err.to_string())
    }
}

#[derive(Insertable)]
#[diesel(table_name = withdrawal_submission_events)]
struct NewWithdrawalSubmissionEventRow {
    created_at: i64,
    withdrawal_id_as_of: Vec<u8>,
    withdrawal_id_base_event_id: Vec<u8>,
    epoch: i64,
    proposal_hash: String,
    transaction_name: String,
    event_type: String,
    signer_node_id: Option<i64>,
    commit_certificate: Option<Vec<u8>>,
    transaction_jam: Option<Vec<u8>>,
    snapshot_height: Option<i64>,
    snapshot_block_id: Option<Vec<u8>>,
    confirmed_height: Option<i64>,
    confirmed_block_id: Option<Vec<u8>>,
}

#[derive(Queryable)]
struct WithdrawalSubmissionEventRow {
    event_id: i64,
    created_at: i64,
    withdrawal_id_as_of: Vec<u8>,
    withdrawal_id_base_event_id: Vec<u8>,
    epoch: i64,
    proposal_hash: String,
    transaction_name: String,
    event_type: String,
    signer_node_id: Option<i64>,
    commit_certificate: Option<Vec<u8>>,
    _transaction_jam: Option<Vec<u8>>,
    snapshot_height: Option<i64>,
    snapshot_block_id: Option<Vec<u8>>,
    confirmed_height: Option<i64>,
    confirmed_block_id: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Queryable)]
struct SequencerJournalCursorRow {
    _journal_id: String,
    last_sequence: i64,
    last_event_id: String,
    _updated_at: i64,
}

impl TryFrom<SequencerJournalCursorRow> for SequencerJournalCursor {
    type Error = BridgeError;

    fn try_from(row: SequencerJournalCursorRow) -> Result<Self, Self::Error> {
        let last_sequence = u64::try_from(row.last_sequence).map_err(|err| {
            BridgeError::Runtime(format!(
                "sequencer journal cursor sequence is negative or invalid: {err}"
            ))
        })?;
        Ok(Self {
            journal_id: row._journal_id,
            last_sequence,
            last_event_id: row.last_event_id,
        })
    }
}

#[derive(Insertable, AsChangeset)]
#[diesel(table_name = sequencer_journal_cursor)]
struct NewSequencerJournalCursorRow {
    journal_id: String,
    last_sequence: i64,
    last_event_id: String,
    updated_at: i64,
}

#[derive(Insertable, AsChangeset)]
#[diesel(table_name = sequencer_withdrawals)]
struct SequencerWithdrawalRow {
    withdrawal_id_as_of: Vec<u8>,
    withdrawal_id_base_event_id: Vec<u8>,
    withdrawal_nonce: i64,
    current_epoch: i64,
    proposal_hash: Option<String>,
    request_recipient: Option<Vec<u8>>,
    request_burned_amount: Option<i64>,
    request_base_batch_end: Option<i64>,
    canonical_amount: Option<i64>,
    canonical_base_batch_end: Option<i64>,
    canonical_transaction_jam: Option<Vec<u8>>,
    canonical_selected_inputs_jam: Option<Vec<u8>>,
    canonical_snapshot_height: Option<i64>,
    canonical_snapshot_block_id: Option<Vec<u8>>,
    peer_commit_certificate: Option<Vec<u8>>,
    authorized_transaction_name: Option<String>,
    authorized_transaction_jam: Option<Vec<u8>>,
    authorized_raw_tx: Option<Vec<u8>>,
    handoff_index: i64,
    turn_started_base_height: Option<i64>,
    submit_attempt_count: i64,
    last_submit_attempt_base_height: Option<i64>,
    last_submit_error: Option<String>,
    state: String,
    created_at: i64,
    updated_at: i64,
}

#[derive(Queryable)]
struct SequencerWithdrawalStoredRow {
    withdrawal_id_as_of: Vec<u8>,
    withdrawal_id_base_event_id: Vec<u8>,
    withdrawal_nonce: i64,
    current_epoch: i64,
    proposal_hash: Option<String>,
    request_recipient: Option<Vec<u8>>,
    request_burned_amount: Option<i64>,
    request_base_batch_end: Option<i64>,
    canonical_amount: Option<i64>,
    canonical_base_batch_end: Option<i64>,
    canonical_transaction_jam: Option<Vec<u8>>,
    canonical_selected_inputs_jam: Option<Vec<u8>>,
    canonical_snapshot_height: Option<i64>,
    canonical_snapshot_block_id: Option<Vec<u8>>,
    peer_commit_certificate: Option<Vec<u8>>,
    authorized_transaction_name: Option<String>,
    authorized_transaction_jam: Option<Vec<u8>>,
    authorized_raw_tx: Option<Vec<u8>>,
    handoff_index: i64,
    turn_started_base_height: Option<i64>,
    submit_attempt_count: i64,
    last_submit_attempt_base_height: Option<i64>,
    last_submit_error: Option<String>,
    state: String,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SequencerWithdrawalRequestFacts {
    recipient: Tip5Hash,
    burned_amount: u64,
    base_batch_end: u64,
}

impl SequencerWithdrawalRequestFacts {
    fn from_tracked(tracked: &TrackedWithdrawalRequest) -> Self {
        Self {
            recipient: tracked.recipient.clone(),
            burned_amount: tracked.amount,
            base_batch_end: tracked.base_batch_end,
        }
    }

    fn from_proposal(proposal: &WithdrawalProposalData) -> Self {
        Self {
            recipient: proposal.recipient.clone(),
            burned_amount: proposal.burned_amount,
            base_batch_end: proposal.base_batch_end,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SequencedWithdrawalView {
    id: WithdrawalId,
    withdrawal_nonce: Option<u64>,
    current_epoch: u64,
    proposal_hash: Option<String>,
    request_facts: Option<SequencerWithdrawalRequestFacts>,
    canonical_amount: Option<u64>,
    canonical_base_batch_end: Option<u64>,
    canonical_transaction_jam: Option<Vec<u8>>,
    canonical_selected_inputs: Option<Vec<nockchain_types::v1::Name>>,
    canonical_snapshot: Option<WithdrawalSnapshot>,
    peer_commit_certificate: Option<Vec<u8>>,
    authorized_transaction_name: Option<String>,
    authorized_transaction_jam: Option<Vec<u8>>,
    authorized_raw_tx: Option<Vec<u8>>,
    handoff_index: u64,
    turn_started_base_height: Option<u64>,
    submit_attempt_count: u64,
    last_submit_attempt_base_height: Option<u64>,
    last_submit_error: Option<String>,
    state: WithdrawalState,
    created_at: i64,
    updated_at: i64,
}

impl SequencedWithdrawalView {
    fn into_live_withdrawal_view(self) -> LiveWithdrawalView {
        let (recipient, gross_burned_amount, base_batch_end) =
            if let Some(request_facts) = self.request_facts {
                (
                    Some(request_facts.recipient),
                    Some(request_facts.burned_amount),
                    Some(request_facts.base_batch_end),
                )
            } else {
                (None, None, None)
            };
        LiveWithdrawalView {
            id: self.id,
            recipient,
            gross_burned_amount,
            base_batch_end,
            withdrawal_nonce: self.withdrawal_nonce,
            current_epoch: self.current_epoch,
            proposal_hash: self.proposal_hash,
            peer_commit_certificate: self.peer_commit_certificate,
            authorized_transaction_name: self.authorized_transaction_name,
            handoff_index: self.handoff_index,
            turn_started_base_height: self.turn_started_base_height,
            submit_attempt_count: self.submit_attempt_count,
            last_submit_attempt_base_height: self.last_submit_attempt_base_height,
            last_submit_error: self.last_submit_error,
            state: self.state,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

#[derive(QueryableByName)]
struct LastInsertRowId {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    value: i64,
}

struct SequencerReservedInputRow {
    id: WithdrawalId,
    epoch: u64,
    input: nockchain_types::v1::Name,
    created_at: i64,
    updated_at: i64,
}

type ReservedInputSqlRow = (Vec<u8>, Vec<u8>, i64, Vec<u8>, Vec<u8>, i64, i64);

pub struct WithdrawalSequencerStore {
    pool: Pool,
    journal: SequencerJournalHandle,
}

impl WithdrawalSequencerStore {
    /// Opens the withdrawal state store and ensures the schema is present before
    /// any runtime loop starts using it.
    pub async fn open(path: PathBuf) -> Result<Self, BridgeError> {
        let pool = sqlite_pool(&path)?;
        let service = Self {
            pool,
            journal: SequencerJournalHandle::disabled(),
        };
        service.ensure_schema().await?;
        Ok(service)
    }

    pub fn with_journal(mut self, journal: SequencerJournalHandle) -> Self {
        self.journal = journal;
        self
    }

    /// Reconciles the local SQLite projection with the durable sequencer journal
    /// before the sequencer starts serving RPCs.
    ///
    /// The journal is the source of truth for sequencer-only decisions. SQLite is
    /// a projection. Startup therefore classifies the local cursor, verifies that
    /// it is a valid prefix of the remote journal, then replays every successor
    /// event. The local cursor advances only after the replay projector commits,
    /// so a crash between remote append and local projection leaves a recoverable
    /// cursor gap.
    pub async fn recover_from_journal_on_startup(
        &self,
    ) -> Result<Option<SequencerJournalRecoveryReport>, BridgeError> {
        let Some(journal_id) = self.journal.journal_id() else {
            return Ok(None);
        };
        let mut cursor = self.prepare_journal_recovery_cursor(&journal_id).await?;
        let start_sequence = cursor.last_sequence;
        let start_event_id = cursor.last_event_id.clone();
        let cursor_object = self.verify_journal_cursor_object(&journal_id, &cursor)?;
        let mut current_ref = match cursor_object {
            Some((cursor_ref, cursor_record)) => {
                self.verify_journal_cursor_event_applied(cursor_record)
                    .await?;
                Some(cursor_ref)
            }
            None => None,
        };
        let mut replayed_count = 0_u64;
        let mut max_replayed_base_height = None;
        let mut max_replayed_nockchain_height = None;

        loop {
            let Some(next_ref) = self.journal.first_after(current_ref.as_ref())? else {
                break;
            };
            let record = self.journal.get(&next_ref)?;
            verify_journal_successor(&journal_id, &cursor, &next_ref, &record)?;
            set_journal_recovery_bounds(
                &record, &mut max_replayed_base_height, &mut max_replayed_nockchain_height,
            );
            self.project_replayed_journal_record(record.clone()).await?;
            cursor.last_sequence = record.sequence;
            cursor.last_event_id = record.event_id.clone();
            current_ref = Some(next_ref);
            replayed_count = replayed_count.saturating_add(1);
        }

        Ok(Some(SequencerJournalRecoveryReport {
            journal_id,
            start_sequence,
            start_event_id,
            last_sequence: cursor.last_sequence,
            last_event_id: cursor.last_event_id,
            replayed_count,
            max_replayed_base_height,
            max_replayed_nockchain_height,
        }))
    }

    /// Loads and classifies the local journal cursor against current projection
    /// contents. This intentionally fails before touching remote storage when the
    /// local cursor and local SQLite projection disagree.
    async fn prepare_journal_recovery_cursor(
        &self,
        journal_id: &str,
    ) -> Result<SequencerJournalCursor, BridgeError> {
        let journal_id = journal_id.to_string();
        self.with_conn(move |conn| {
            let explicit_cursor = load_journal_cursor_optional(conn, &journal_id)?;
            let projection_has_rows = sequencer_projection_has_rows(conn)?;
            check_journal_recovery_cursor(&journal_id, explicit_cursor, projection_has_rows)
        })
        .await
    }

    /// Verifies that a non-genesis cursor names a real remote journal object.
    ///
    /// Genesis has no object by construction, so the first real object is
    /// validated later as the sequence-1 successor of genesis.
    fn verify_journal_cursor_object(
        &self,
        journal_id: &str,
        cursor: &SequencerJournalCursor,
    ) -> Result<Option<(SequencerJournalObjectRef, SequencerJournalRecord)>, BridgeError> {
        if cursor.last_sequence == 0 {
            return Ok(None);
        }
        let cursor_ref = self.journal.object_ref_for_cursor(cursor)?;
        let cursor_record = self.journal.get(&cursor_ref).map_err(|err| {
            BridgeError::Runtime(format!(
                "sequencer journal cursor object is missing or unreadable; local cursor may be ahead of remote or out of sync: sequence {}, event {}, object {}: {err}",
                cursor.last_sequence, cursor.last_event_id, cursor_ref.key
            ))
        })?;
        verify_journal_cursor_record(journal_id, cursor, &cursor_ref, &cursor_record)?;
        Ok(Some((cursor_ref, cursor_record)))
    }

    /// Verifies that the cursor event is already reflected in SQLite before
    /// replay starts from its successor.
    ///
    /// Cursor/hash continuity proves the local cursor names a real remote event;
    /// this check proves SQLite has not been manually corrupted into claiming
    /// that event without carrying its projection effects. The check is
    /// read-only: recovery either starts from a trustworthy cursor or fails
    /// closed.
    async fn verify_journal_cursor_event_applied(
        &self,
        record: SequencerJournalRecord,
    ) -> Result<(), BridgeError> {
        self.with_conn(move |conn| verify_journal_cursor_event_applied(conn, &record))
            .await
    }

    /// Applies one replayed event and advances the cursor in the same SQLite
    /// transaction. This is the core recovery invariant: replay is never marked
    /// complete for an event unless the projection for that event committed.
    async fn project_replayed_journal_record(
        &self,
        record: SequencerJournalRecord,
    ) -> Result<(), BridgeError> {
        self.with_write_tx(move |conn, _journal| {
            apply_journal_event(conn, &record, SequencerJournalApplyMode::Replay)?;
            upsert_journal_cursor(
                conn, &record.journal_id, record.sequence, &record.event_id,
                record.created_at_unix_ms,
            )
        })
        .await
    }

    pub async fn load_journal_cursor(
        &self,
        journal_id: &str,
    ) -> Result<SequencerJournalCursor, BridgeError> {
        let journal_id = journal_id.to_string();
        self.with_conn(move |conn| load_journal_cursor(conn, &journal_id)?.try_into())
            .await
    }

    /// Runs a store operation against a pooled SQLite connection with the
    /// store's standard pragmas applied.
    async fn with_conn<T, E, F>(&self, f: F) -> Result<T, E>
    where
        F: FnOnce(&mut SqliteConnection) -> Result<T, E> + Send + 'static,
        T: Send + 'static,
        E: From<BridgeError> + std::error::Error + Send + Sync + 'static,
    {
        let conn = self.pool.get().await.map_err(|err| {
            E::from(BridgeError::Runtime(format!(
                "withdrawal state store pool failed: {err}"
            )))
        })?;
        let result = conn
            .interact(move |conn| {
                conn.batch_execute(&format!(
                    "PRAGMA busy_timeout = {}; PRAGMA foreign_keys = ON;",
                    SQLITE_BUSY_TIMEOUT_MS
                ))
                .map_err(|err| {
                    E::from(BridgeError::Runtime(format!(
                        "withdrawal state store pragma failed: {err}"
                    )))
                })?;
                f(conn)
            })
            .await
            .map_err(|err| {
                E::from(BridgeError::Runtime(format!(
                    "withdrawal state store interact failed: {err}"
                )))
            })?;
        result
    }

    /// Runs a store operation inside a write transaction, rolling back on
    /// errors and committing only on success.
    async fn with_write_tx<T, E, F>(&self, f: F) -> Result<T, E>
    where
        F: FnOnce(&mut SqliteConnection, &SequencerJournalHandle) -> Result<T, E> + Send + 'static,
        T: Send + 'static,
        E: From<BridgeError> + std::error::Error + Send + Sync + 'static,
    {
        let journal = self.journal.clone();
        self.with_conn(move |conn| {
            conn.immediate_transaction::<_, anyhow::Error, _>(|conn| Ok(f(conn, &journal)?))
                .map_err(|err| {
                    E::from(BridgeError::Runtime(format!(
                        "withdrawal state store transaction failed: {err}"
                    )))
                })
        })
        .await
    }

    /// Records that a pending proposal became the peer-canonical proposal for
    /// its withdrawal epoch.
    pub async fn record_proposal_canonicalized(
        &self,
        proposal: &WithdrawalProposalData,
        turn_started_base_height: u64,
    ) -> Result<(), WithdrawalSequencerStoreError> {
        self.record_proposal_canonicalized_with_certificate(
            proposal, None, turn_started_base_height,
        )
        .await
    }

    /// Records that a pending proposal became the peer-canonical proposal for
    /// its withdrawal epoch.
    pub async fn record_proposal_canonicalized_with_certificate(
        &self,
        proposal: &WithdrawalProposalData,
        commit_certificate: Option<&WithdrawalCommitCertificate>,
        turn_started_base_height: u64,
    ) -> Result<(), WithdrawalSequencerStoreError> {
        self.record_peer_canonical_proposal(proposal, commit_certificate, turn_started_base_height)
            .await
    }

    /// Records the peer-canonical proposal and its commit certificate on the
    /// sequencer row before authorization begins.
    pub async fn record_peer_canonical_proposal(
        &self,
        proposal: &WithdrawalProposalData,
        commit_certificate: Option<&WithdrawalCommitCertificate>,
        turn_started_base_height: u64,
    ) -> Result<(), WithdrawalSequencerStoreError> {
        let proposal = proposal.clone();
        let commit_certificate = commit_certificate.cloned();
        let created_at = now_unix_secs()?;
        self.with_write_tx(move |conn, journal| {
            record_peer_canonical_proposal_tx(
                conn,
                journal,
                &proposal,
                commit_certificate.as_ref(),
                turn_started_base_height,
                created_at,
            )
        })
        .await
    }

    /// Records one signer's witness contribution for a proposal.
    pub async fn record_proposal_signed(
        &self,
        proposal: &WithdrawalProposalData,
        signer_node_id: u64,
        observed_base_height: u64,
    ) -> Result<(), WithdrawalSequencerStoreError> {
        let proposal = proposal.clone();
        let created_at = now_unix_secs()?;
        self.with_write_tx(move |conn, journal| {
            let Some(existing) = fetch_sequenced_withdrawal(conn, &proposal.id.base_event_id)? else {
                return Ok(());
            };
            if existing.current_epoch != proposal.epoch {
                return Ok(());
            }
            let withdrawal_nonce = existing.withdrawal_nonce.ok_or_else(|| {
                WithdrawalSequencerStoreError::Store(format!(
                    "sequenced withdrawal {:?} is missing nonce while recording signed proposal",
                    proposal.id
                ))
            })?;
            ensure_withdrawal_is_current_frontier(
                conn,
                &proposal.id,
                withdrawal_nonce,
                "record signed proposal",
            )?;

            let proposal_hash = proposal.proposal_hash()?;
            if signed_proposal_exists(
                conn,
                &proposal.id,
                proposal.epoch,
                signer_node_id,
                &proposal_hash,
            )? {
                return Ok(());
            }

            let live_hash_matches =
                existing.proposal_hash.as_deref() == Some(proposal_hash.as_str());
            if matches!(
                existing.state,
                WithdrawalState::PeerCanonical | WithdrawalState::Authorized
            ) && !live_hash_matches
            {
                return Err(WithdrawalSequencerStoreError::Store(format!(
                    "cannot record signed proposal for withdrawal {:?} epoch {} with mismatched live canonical hash",
                    proposal.id, proposal.epoch
                )));
            }
            let next_turn_started_base_height = if matches!(
                existing.state,
                WithdrawalState::PeerCanonical
            ) {
                Some(observed_base_height)
            } else {
                existing.turn_started_base_height
            };

            apply_sequencer_mutation(
                conn,
                journal,
                SequencerMutation::ProposalSigned {
                    proposal,
                    existing,
                    signer_node_id,
                    next_turn_started_base_height,
                    created_at,
                },
            )?;
            Ok(())
        })
        .await
    }

    /// Advances a post-canonical proposer turn by withdrawal id once the
    /// sequencer-owned Base-height handoff window has elapsed.
    pub async fn record_proposer_turn_expired_for_id(
        &self,
        id: &WithdrawalId,
        epoch: u64,
        next_handoff_index: u64,
        next_turn_started_base_height: u64,
    ) -> Result<bool, WithdrawalSequencerStoreError> {
        let id = id.clone();
        let created_at = now_unix_secs()?;
        self.with_write_tx(move |conn, journal| {
            record_proposer_turn_expired_tx_for_id(
                conn, journal, &id, epoch, next_handoff_index, next_turn_started_base_height,
                created_at,
            )
        })
        .await
    }

    /// Advances the shared pre-canonical handoff index for a pending
    /// withdrawal epoch without creating a replacement tx attempt.
    pub async fn record_precanonical_handoff_for_id(
        &self,
        id: &WithdrawalId,
        epoch: u64,
        next_handoff_index: u64,
        turn_started_base_height: u64,
    ) -> Result<(), WithdrawalSequencerStoreError> {
        let id = id.clone();
        let created_at = now_unix_secs()?;
        self.with_write_tx(move |conn, journal| {
            record_precanonical_handoff_tx_for_id(
                conn, journal, &id, epoch, next_handoff_index, turn_started_base_height, created_at,
            )
        })
        .await
    }

    /// Records that the current proposer turn expired for a canonicalized
    /// withdrawal without clearing the fixed transaction body.
    pub async fn record_proposer_turn_expired(
        &self,
        proposal: &WithdrawalProposalData,
        next_handoff_index: u64,
        next_turn_started_base_height: u64,
    ) -> Result<bool, WithdrawalSequencerStoreError> {
        let proposal = proposal.clone();
        let created_at = now_unix_secs()?;
        self.with_write_tx(move |conn, journal| {
            record_proposer_turn_expired_tx(
                conn, journal, &proposal, next_handoff_index, next_turn_started_base_height,
                created_at,
            )
        })
        .await
    }

    /// Records that the peer-canonical proposal has been authorized for
    /// submission and becomes the single-flight active sequencer withdrawal.
    pub async fn record_proposal_authorized(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<(), WithdrawalSequencerStoreError> {
        let proposal = proposal.clone();
        let created_at = now_unix_secs()?;
        self.with_write_tx(move |conn, journal| {
            record_proposal_authorized_tx(conn, journal, &proposal, None, created_at)
        })
        .await
    }

    /// Records a bounded-submit result for an authorized proposal. Failed
    /// submit loops keep the row authorized so the nonce frontier remains
    /// blocked without adding a separate stalled lifecycle state.
    pub async fn record_submit_outcome(
        &self,
        proposal: &WithdrawalProposalData,
        final_state: WithdrawalState,
        submit_attempt_count: u64,
        last_submit_attempt_base_height: u64,
        last_submit_error: Option<String>,
    ) -> Result<(), WithdrawalSequencerStoreError> {
        let proposal = proposal.clone();
        let created_at = now_unix_secs()?;
        self.with_write_tx(move |conn, journal| {
            record_submit_outcome_tx(
                conn,
                journal,
                &proposal,
                final_state,
                submit_attempt_count,
                last_submit_attempt_base_height,
                last_submit_error.clone(),
                created_at,
            )
        })
        .await
    }

    /// Records that an authorized transaction was observed in the mempool
    /// without rebroadcasting it during this request.
    pub async fn record_authorized_mempool_accepted(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<(), WithdrawalSequencerStoreError> {
        let proposal = proposal.clone();
        let created_at = now_unix_secs()?;
        self.with_write_tx(move |conn, journal| {
            record_authorized_mempool_accepted_tx(conn, journal, &proposal, created_at)
        })
        .await
    }

    /// Refreshes submit-attempt metadata for an already mempool-accepted
    /// withdrawal and records the retry observation in append-only history.
    pub async fn record_mempool_retry_attempt(
        &self,
        id: &WithdrawalId,
        expected_epoch: u64,
        expected_proposal_hash: &str,
        attempt_base_height: u64,
        error: Option<String>,
    ) -> Result<(), WithdrawalSequencerStoreError> {
        let id = id.clone();
        let expected_proposal_hash = expected_proposal_hash.to_string();
        let updated_at = now_unix_secs()?;
        self.with_write_tx(move |conn, journal| {
            record_mempool_retry_attempt_tx(
                conn,
                journal,
                &id,
                expected_epoch,
                &expected_proposal_hash,
                attempt_base_height,
                error.clone(),
                updated_at,
            )
        })
        .await
    }

    /// Records a confirmed block for a mempool-accepted proposal and
    /// transitions the sequencer row to `Confirmed`.
    pub async fn record_tx_confirmed(
        &self,
        proposal: &WithdrawalProposalData,
        confirmed_height: u64,
        confirmed_block_id: Tip5Hash,
    ) -> Result<(), WithdrawalSequencerStoreError> {
        let proposal = proposal.clone();
        let created_at = now_unix_secs()?;
        self.with_write_tx(move |conn, journal| {
            record_tx_confirmed_tx(
                conn,
                journal,
                &proposal,
                confirmed_height,
                confirmed_block_id.clone(),
                created_at,
            )
        })
        .await
    }

    /// Records confirmation using only the sequencer withdrawal id.
    pub async fn record_tx_confirmed_by_id(
        &self,
        id: &WithdrawalId,
        confirmed_height: u64,
        confirmed_block_id: Tip5Hash,
    ) -> Result<bool, BridgeError> {
        let id = id.clone();
        let created_at = now_unix_secs()?;
        self.with_write_tx(move |conn, journal| {
            let Some(existing) = fetch_sequenced_withdrawal(conn, &id.base_event_id)? else {
                return Ok(false);
            };
            if existing.state == WithdrawalState::Confirmed {
                return Ok(false);
            }
            if existing.state != WithdrawalState::MempoolAccepted {
                return Ok(false);
            }
            let proposal_hash = existing.proposal_hash.clone().ok_or_else(|| {
                BridgeError::Runtime(format!(
                    "sequenced withdrawal {:?} is missing authorized proposal hash",
                    id
                ))
            })?;
            let transaction_name =
                existing
                    .authorized_transaction_name
                    .clone()
                    .ok_or_else(|| {
                        BridgeError::Runtime(format!(
                            "sequenced withdrawal {:?} is missing authorized transaction name",
                            id
                        ))
                    })?;
            let withdrawal_nonce = existing.withdrawal_nonce.ok_or_else(|| {
                BridgeError::Runtime(format!(
                    "sequenced withdrawal {:?} is missing nonce during confirmation",
                    id
                ))
            })?;
            apply_sequencer_mutation(
                conn,
                journal,
                SequencerMutation::TxConfirmedById {
                    id,
                    existing,
                    withdrawal_nonce,
                    proposal_hash,
                    transaction_name,
                    confirmed_height,
                    confirmed_block_id: confirmed_block_id.clone(),
                    created_at,
                },
            )?;
            Ok(true)
        })
        .await
    }

    /// Records that the submitted transaction was observed as mempool-accepted
    /// by a Nockchain node without yet marking it confirmed.
    pub async fn record_tx_seen_mempool_accepted(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<(), WithdrawalSequencerStoreError> {
        let proposal = proposal.clone();
        let created_at = now_unix_secs()?;
        self.with_write_tx(move |conn, journal| {
            record_tx_seen_mempool_accepted_tx(conn, journal, &proposal, created_at)
        })
        .await
    }

    pub async fn ensure_tracked_withdrawal_ordering(
        &self,
        tracked: &TrackedWithdrawalRequest,
    ) -> Result<(), BridgeError> {
        self.ensure_tracked_withdrawal_ordering_with_turn_start(tracked, None)
            .await
    }

    pub async fn ensure_tracked_withdrawal_ordering_at_base_height(
        &self,
        tracked: &TrackedWithdrawalRequest,
        turn_started_base_height: u64,
    ) -> Result<(), BridgeError> {
        self.ensure_tracked_withdrawal_ordering_with_turn_start(
            tracked,
            Some(turn_started_base_height),
        )
        .await
    }

    async fn ensure_tracked_withdrawal_ordering_with_turn_start(
        &self,
        tracked: &TrackedWithdrawalRequest,
        turn_started_base_height: Option<u64>,
    ) -> Result<(), BridgeError> {
        let tracked = tracked.clone();
        let request_facts = SequencerWithdrawalRequestFacts::from_tracked(&tracked);
        let created_at = now_unix_secs()?;
        self.with_write_tx(move |conn, journal| {
            ensure_tracked_withdrawal_ordering_tx(
                conn, journal, &tracked.id, tracked.withdrawal_nonce, request_facts,
                turn_started_base_height, created_at,
            )
        })
        .await
    }

    pub async fn ensure_registered_proposal_ordering(
        &self,
        proposal: &WithdrawalProposalData,
        withdrawal_nonce: u64,
    ) -> Result<(), BridgeError> {
        let proposal = proposal.clone();
        self.with_conn(move |conn| {
            let request_facts = SequencerWithdrawalRequestFacts::from_proposal(&proposal);
            ensure_registered_withdrawal_ordering(
                conn,
                &proposal.id,
                withdrawal_nonce,
                Some(&request_facts),
            )
        })
        .await
    }

    /// Loads the currently reserved note names across all canonical-or-later
    /// sequencer-owned withdrawals.
    pub async fn list_reserved_input_names(
        &self,
    ) -> Result<Vec<nockchain_types::v1::Name>, BridgeError> {
        self.with_conn(list_reserved_input_names).await
    }

    #[cfg(test)]
    async fn reserved_input_names_for(
        &self,
        id: &WithdrawalId,
    ) -> Result<Vec<nockchain_types::v1::Name>, BridgeError> {
        let id = id.clone();
        self.with_conn(move |conn| load_reserved_input_names_for_withdrawal(conn, &id))
            .await
    }

    /// Atomically validates ordering and records the sequencer's authorized
    /// current state for a proposal.
    pub async fn sequencer_authorize_proposal(
        &self,
        proposal: &WithdrawalProposalData,
        withdrawal_nonce: u64,
        commit_certificate: &WithdrawalCommitCertificate,
        turn_started_base_height: u64,
    ) -> Result<SequencerDecision, BridgeError> {
        let proposal = proposal.clone();
        let commit_certificate = commit_certificate.clone();
        let created_at = now_unix_secs()?;
        self.with_write_tx(move |conn, journal| {
            let request_facts = SequencerWithdrawalRequestFacts::from_proposal(&proposal);
            ensure_registered_withdrawal_ordering(
                conn,
                &proposal.id,
                withdrawal_nonce,
                Some(&request_facts),
            )?;
            let Some((next_id, next_nonce)) = next_pending_withdrawal_ordering(conn)? else {
                return Ok(SequencerDecision::Rejected(
                    "no pending withdrawals available for authorization".to_string(),
                ));
            };
            if !same_base_event_id(&next_id, &proposal.id) || next_nonce != withdrawal_nonce {
                return Ok(SequencerDecision::Rejected(format!(
                    "withdrawal {:?} nonce {} is blocked by next pending withdrawal {:?} nonce {}",
                    proposal.id, withdrawal_nonce, next_id, next_nonce
                )));
            }

            let proposal_hash = proposal.proposal_hash()?;
            if let Some(existing) = fetch_sequenced_withdrawal(conn, &proposal.id.base_event_id)? {
                if existing.current_epoch == proposal.epoch
                    && existing.proposal_hash.as_deref() == Some(proposal_hash.as_str())
                    && matches!(
                        existing.state,
                        WithdrawalState::Authorized
                            | WithdrawalState::MempoolAccepted
                            | WithdrawalState::Confirmed
                    )
                {
                    return Ok(SequencerDecision::Allowed);
                }
            }

            record_peer_canonical_proposal_tx(
                conn,
                journal,
                &proposal,
                Some(&commit_certificate),
                turn_started_base_height,
                created_at,
            )
            .map_err(|err| BridgeError::Runtime(err.to_string()))?;
            match record_proposal_authorized_tx(
                conn,
                journal,
                &proposal,
                Some(turn_started_base_height),
                created_at,
            ) {
                Ok(()) => Ok(SequencerDecision::Allowed),
                Err(err) => Ok(SequencerDecision::Rejected(err.to_string())),
            }
        })
        .await
    }

    /// Validates whether a proposal is currently eligible for sequencer-side
    /// submission without mutating the durable current-state row.
    pub async fn sequencer_can_submit_proposal(
        &self,
        proposal: &WithdrawalProposalData,
        withdrawal_nonce: u64,
    ) -> Result<SequencerDecision, BridgeError> {
        let proposal = proposal.clone();
        self.with_conn(move |conn| {
            check_sequencer_submit_preconditions(conn, &proposal, withdrawal_nonce)
        })
        .await
    }

    /// Returns the accepted withdrawal nonce for a known withdrawal.
    pub async fn withdrawal_nonce_for(
        &self,
        id: &WithdrawalId,
    ) -> Result<Option<u64>, BridgeError> {
        let base_event_id = id.base_event_id.clone();
        self.with_conn(move |conn| fetch_withdrawal_nonce(conn, &base_event_id))
            .await
    }

    /// Returns the next withdrawal nonce whose sequencer state has not yet
    /// released ordering.
    pub async fn next_pending_withdrawal_ordering(
        &self,
    ) -> Result<Option<(WithdrawalId, u64)>, BridgeError> {
        self.with_conn(next_pending_withdrawal_ordering).await
    }

    /// Returns the lowest withdrawal nonce that is live at the sequencer and
    /// not yet released from ordering.
    pub async fn current_live_withdrawal_nonce(&self) -> Result<Option<u64>, BridgeError> {
        self.with_conn(current_live_withdrawal_nonce).await
    }

    /// Returns whether a withdrawal is registered and is the sequencer's current
    /// live frontier.
    pub async fn frontier_allows_withdrawal(
        &self,
        id: &WithdrawalId,
    ) -> Result<WithdrawalFrontierCheck, BridgeError> {
        let id = id.clone();
        self.with_conn(move |conn| frontier_allows_withdrawal(conn, &id))
            .await
    }

    /// Loads the full append-only submission event history.
    pub async fn list_submission_events(
        &self,
    ) -> Result<Vec<WithdrawalSubmissionEventRecord>, BridgeError> {
        self.with_conn(load_events).await
    }

    /// Returns whether this signer has already contributed a signed proposal
    /// record for the given `(withdrawal_id, epoch, proposal_hash)`.
    pub async fn has_signed_proposal_from_signer(
        &self,
        id: &WithdrawalId,
        epoch: u64,
        proposal_hash: &str,
        signer_node_id: u64,
    ) -> Result<bool, BridgeError> {
        let id = id.clone();
        let proposal_hash = proposal_hash.to_string();
        self.with_conn(move |conn| {
            signed_proposal_exists(conn, &id, epoch, signer_node_id, &proposal_hash)
        })
        .await
    }

    /// Loads the distinct signed transaction contributions recorded for a
    /// proposal hash.
    pub async fn load_signed_transactions(
        &self,
        id: &WithdrawalId,
        epoch: u64,
        proposal_hash: &str,
    ) -> Result<Vec<SignedWithdrawalTransactionRecord>, BridgeError> {
        let id = id.clone();
        let proposal_hash = proposal_hash.to_string();
        self.with_conn(move |conn| {
            load_signed_transaction_records(conn, &id, epoch, &proposal_hash)
        })
        .await
    }

    /// Loads the exact authorized transaction for orphan retry of one
    /// mempool-accepted withdrawal.
    pub async fn load_authorized_transaction_for_retry(
        &self,
        id: &WithdrawalId,
    ) -> Result<Option<AuthorizedRetryPayload>, BridgeError> {
        let id = id.clone();
        self.with_conn(move |conn| load_authorized_transaction_for_retry(conn, &id))
            .await
    }

    /// Loads the stored authorized transaction envelope for operator export.
    pub async fn load_authorized_transaction_export_by_tx_id(
        &self,
        tx_id: &str,
    ) -> Result<Option<AuthorizedTransactionExport>, BridgeError> {
        let tx_id = tx_id.to_string();
        self.with_conn(move |conn| load_authorized_transaction_export_by_tx_id(conn, &tx_id))
            .await
    }

    /// Loads the sequencer-owned current-state row for one withdrawal.
    pub async fn fetch_sequenced_withdrawal(
        &self,
        id: &WithdrawalId,
    ) -> Result<Option<LiveWithdrawalView>, BridgeError> {
        let base_event_id = id.base_event_id.clone();
        self.with_conn(move |conn| {
            fetch_sequenced_withdrawal(conn, &base_event_id)
                .map(|row| row.map(SequencedWithdrawalView::into_live_withdrawal_view))
        })
        .await
    }

    /// Loads the canonical proposal artifacts projected from the sequencer
    /// journal for one withdrawal.
    pub async fn load_canonical_proposal_artifacts(
        &self,
        id: &WithdrawalId,
    ) -> Result<Option<WithdrawalSequencerProposalArtifacts>, BridgeError> {
        let base_event_id = id.base_event_id.clone();
        self.with_conn(move |conn| load_canonical_proposal_artifacts(conn, &base_event_id))
            .await
    }

    /// Returns all sequencer-owned current-state rows ordered by oldest update
    /// first.
    pub async fn list_sequenced_withdrawals(&self) -> Result<Vec<LiveWithdrawalView>, BridgeError> {
        self.with_conn(fetch_all_sequenced_withdrawals).await
    }

    /// Creates or upgrades the SQLite schema used by withdrawal sequencing.
    async fn ensure_schema(&self) -> Result<(), BridgeError> {
        self.with_conn(|conn| {
            rename_sqlite_table_if_needed(conn, "live_withdrawals", "sequencer_withdrawals")?;
            conn.batch_execute(
                r#"
            PRAGMA journal_mode=WAL;
            PRAGMA synchronous=FULL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS withdrawal_submission_events (
                event_id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at INTEGER NOT NULL,
                withdrawal_id_as_of BLOB NOT NULL CHECK(length(withdrawal_id_as_of) = 40),
                withdrawal_id_base_event_id BLOB NOT NULL,
                epoch INTEGER NOT NULL,
                proposal_hash TEXT NOT NULL,
                transaction_name TEXT NOT NULL,
                event_type TEXT NOT NULL,
                signer_node_id INTEGER,
                commit_certificate BLOB,
                transaction_jam BLOB,
                snapshot_height INTEGER,
                snapshot_block_id BLOB,
                confirmed_height INTEGER,
                confirmed_block_id BLOB
            );

            CREATE INDEX IF NOT EXISTS withdrawal_submission_events_lookup
              ON withdrawal_submission_events(
                withdrawal_id_as_of,
                withdrawal_id_base_event_id,
                epoch,
                proposal_hash,
                event_id
              );
            CREATE INDEX IF NOT EXISTS withdrawal_submission_events_by_base_event
              ON withdrawal_submission_events(
                withdrawal_id_base_event_id,
                epoch,
                proposal_hash,
                event_id
              );

            CREATE TABLE IF NOT EXISTS sequencer_journal_cursor (
                journal_id TEXT PRIMARY KEY,
                last_sequence INTEGER NOT NULL,
                last_event_id TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS sequencer_withdrawals (
                withdrawal_id_as_of BLOB NOT NULL CHECK(length(withdrawal_id_as_of) = 40),
                withdrawal_id_base_event_id BLOB NOT NULL,
                withdrawal_nonce INTEGER NOT NULL UNIQUE,
                current_epoch INTEGER NOT NULL,
                proposal_hash TEXT,
                request_recipient BLOB,
                request_burned_amount INTEGER,
                request_base_batch_end INTEGER,
                canonical_amount INTEGER,
                canonical_base_batch_end INTEGER,
                canonical_transaction_jam BLOB,
                canonical_selected_inputs_jam BLOB,
                canonical_snapshot_height INTEGER,
                canonical_snapshot_block_id BLOB,
                peer_commit_certificate BLOB,
                authorized_transaction_name TEXT,
                authorized_transaction_jam BLOB,
                authorized_raw_tx BLOB,
                handoff_index INTEGER NOT NULL DEFAULT 0,
                turn_started_base_height INTEGER,
                submit_attempt_count INTEGER NOT NULL DEFAULT 0,
                last_submit_attempt_base_height INTEGER,
                last_submit_error TEXT,
                state TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (withdrawal_id_base_event_id)
            );

            DROP INDEX IF EXISTS live_withdrawals_by_state;

            CREATE INDEX IF NOT EXISTS sequencer_withdrawals_by_state
              ON sequencer_withdrawals(state, updated_at);
            CREATE UNIQUE INDEX IF NOT EXISTS sequencer_withdrawals_by_nonce
              ON sequencer_withdrawals(withdrawal_nonce);
            CREATE UNIQUE INDEX IF NOT EXISTS sequencer_withdrawals_by_base_event_id
              ON sequencer_withdrawals(withdrawal_id_base_event_id);

            CREATE TABLE IF NOT EXISTS withdrawal_reserved_inputs (
                withdrawal_id_as_of BLOB NOT NULL CHECK(length(withdrawal_id_as_of) = 40),
                withdrawal_id_base_event_id BLOB NOT NULL,
                epoch INTEGER NOT NULL,
                input_first BLOB NOT NULL CHECK(length(input_first) = 40),
                input_last BLOB NOT NULL CHECK(length(input_last) = 40),
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (
                    withdrawal_id_as_of,
                    withdrawal_id_base_event_id,
                    input_first,
                    input_last
                ),
                FOREIGN KEY (withdrawal_id_base_event_id)
                  REFERENCES sequencer_withdrawals(withdrawal_id_base_event_id)
            );

            CREATE INDEX IF NOT EXISTS withdrawal_reserved_inputs_by_withdrawal
              ON withdrawal_reserved_inputs(
                withdrawal_id_as_of,
                withdrawal_id_base_event_id,
                epoch
              );
            CREATE INDEX IF NOT EXISTS withdrawal_reserved_inputs_by_base_event
              ON withdrawal_reserved_inputs(withdrawal_id_base_event_id, epoch);
            CREATE UNIQUE INDEX IF NOT EXISTS withdrawal_reserved_inputs_by_name
              ON withdrawal_reserved_inputs(input_first, input_last);

            DROP TABLE IF EXISTS current_reserved_inputs;
            DROP TABLE IF EXISTS withdrawal_submission_event_inputs;
            "#,
            )
            .map_err(|err| {
                BridgeError::Runtime(format!("withdrawal state store schema failed: {err}"))
            })?;
            ensure_sqlite_column_exists(
                conn, "sequencer_withdrawals", "withdrawal_nonce", "INTEGER",
            )?;
            ensure_sqlite_column_exists(
                conn, "sequencer_withdrawals", "request_recipient", "BLOB",
            )?;
            ensure_sqlite_column_exists(
                conn, "sequencer_withdrawals", "request_burned_amount", "INTEGER",
            )?;
            ensure_sqlite_column_exists(
                conn, "sequencer_withdrawals", "request_base_batch_end", "INTEGER",
            )?;
            ensure_sqlite_column_exists(
                conn, "sequencer_withdrawals", "canonical_amount", "INTEGER",
            )?;
            ensure_sqlite_column_exists(
                conn, "sequencer_withdrawals", "canonical_base_batch_end", "INTEGER",
            )?;
            ensure_sqlite_column_exists(
                conn, "sequencer_withdrawals", "canonical_transaction_jam", "BLOB",
            )?;
            ensure_sqlite_column_exists(
                conn, "sequencer_withdrawals", "canonical_selected_inputs_jam", "BLOB",
            )?;
            ensure_sqlite_column_exists(
                conn, "sequencer_withdrawals", "canonical_snapshot_height", "INTEGER",
            )?;
            ensure_sqlite_column_exists(
                conn, "sequencer_withdrawals", "canonical_snapshot_block_id", "BLOB",
            )?;
            ensure_sqlite_column_exists(
                conn, "sequencer_withdrawals", "peer_commit_certificate", "BLOB",
            )?;
            ensure_sqlite_column_exists(
                conn, "sequencer_withdrawals", "submit_attempt_count", "INTEGER NOT NULL DEFAULT 0",
            )?;
            ensure_sqlite_column_exists(
                conn, "sequencer_withdrawals", "handoff_index", "INTEGER NOT NULL DEFAULT 0",
            )?;
            ensure_sqlite_column_exists(
                conn, "sequencer_withdrawals", "turn_started_base_height", "INTEGER",
            )?;
            ensure_sqlite_column_exists(
                conn, "sequencer_withdrawals", "last_submit_attempt_base_height", "INTEGER",
            )?;
            ensure_sqlite_column_exists(
                conn, "sequencer_withdrawals", "last_submit_error", "TEXT",
            )?;
            ensure_sqlite_column_exists(
                conn, "sequencer_withdrawals", "authorized_transaction_jam", "BLOB",
            )?;
            ensure_sqlite_column_exists(
                conn, "sequencer_withdrawals", "authorized_raw_tx", "BLOB",
            )?;
            conn.batch_execute(
                r#"
            CREATE INDEX IF NOT EXISTS sequencer_withdrawals_by_request_ordering
              ON sequencer_withdrawals(
                request_base_batch_end DESC,
                withdrawal_id_base_event_id DESC
              );
            "#,
            )
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "sequencer withdrawal request ordering index failed: {err}"
                ))
            })?;
            ensure_sqlite_column_exists(
                conn, "withdrawal_submission_events", "commit_certificate", "BLOB",
            )?;
            Ok(())
        })
        .await
    }
}

#[derive(Debug, Clone)]
struct SequencerWithdrawalUpdate {
    id: WithdrawalId,
    withdrawal_nonce: u64,
    current_epoch: u64,
    proposal_hash: Option<String>,
    request_facts: Option<SequencerWithdrawalRequestFacts>,
    canonical_amount: Option<u64>,
    canonical_base_batch_end: Option<u64>,
    canonical_transaction_jam: Option<Vec<u8>>,
    canonical_selected_inputs: Option<Vec<nockchain_types::v1::Name>>,
    canonical_snapshot: Option<WithdrawalSnapshot>,
    peer_commit_certificate: Option<Vec<u8>>,
    authorized_transaction_name: Option<String>,
    authorized_transaction_jam: Option<Vec<u8>>,
    authorized_raw_tx: Option<Vec<u8>>,
    handoff_index: u64,
    turn_started_base_height: Option<u64>,
    submit_attempt_count: u64,
    last_submit_attempt_base_height: Option<u64>,
    last_submit_error: Option<String>,
    state: WithdrawalState,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SequencerJournalApplyMode {
    Runtime,
    Replay,
}

enum SequencerMutation {
    WithdrawalOrdered {
        id: WithdrawalId,
        withdrawal_nonce: u64,
        request_facts: SequencerWithdrawalRequestFacts,
        turn_started_base_height: Option<u64>,
        created_at: i64,
    },
    ProposalSigned {
        proposal: WithdrawalProposalData,
        existing: SequencedWithdrawalView,
        signer_node_id: u64,
        next_turn_started_base_height: Option<u64>,
        created_at: i64,
    },
    ProposalCanonicalized {
        proposal: WithdrawalProposalData,
        existing: SequencedWithdrawalView,
        withdrawal_nonce: u64,
        proposal_hash: String,
        commit_certificate: Option<WithdrawalCommitCertificate>,
        turn_started_base_height: u64,
        created_at: i64,
    },
    ProposerTurnExpiredForProposal {
        proposal: WithdrawalProposalData,
        existing: SequencedWithdrawalView,
        next_handoff_index: u64,
        next_turn_started_base_height: u64,
        created_at: i64,
    },
    ProposerTurnExpiredForRow {
        existing: SequencedWithdrawalView,
        next_handoff_index: u64,
        next_turn_started_base_height: u64,
        created_at: i64,
    },
    PrecanonicalHandoff {
        existing: SequencedWithdrawalView,
        next_handoff_index: u64,
        turn_started_base_height: u64,
        created_at: i64,
    },
    ProposalAuthorized {
        proposal: WithdrawalProposalData,
        existing: SequencedWithdrawalView,
        proposal_hash: String,
        authorized_transaction: StoredAuthorizedTransaction,
        turn_started_base_height: Option<u64>,
        created_at: i64,
    },
    SubmitOutcome {
        proposal: WithdrawalProposalData,
        existing: SequencedWithdrawalView,
        final_state: WithdrawalState,
        authorized_transaction: StoredAuthorizedTransaction,
        submit_attempt_count: u64,
        last_submit_attempt_base_height: u64,
        last_submit_error: Option<String>,
        created_at: i64,
    },
    AuthorizedMempoolAccepted {
        proposal: WithdrawalProposalData,
        existing: SequencedWithdrawalView,
        created_at: i64,
    },
    MempoolRetryAttempted {
        existing: SequencedWithdrawalView,
        attempt_base_height: u64,
        error: Option<String>,
        updated_at: i64,
    },
    TxConfirmed {
        proposal: WithdrawalProposalData,
        existing: SequencedWithdrawalView,
        withdrawal_nonce: u64,
        proposal_hash: String,
        authorized_transaction: StoredAuthorizedTransaction,
        confirmed_height: u64,
        confirmed_block_id: Tip5Hash,
        created_at: i64,
    },
    TxConfirmedById {
        id: WithdrawalId,
        existing: SequencedWithdrawalView,
        withdrawal_nonce: u64,
        proposal_hash: String,
        transaction_name: String,
        confirmed_height: u64,
        confirmed_block_id: Tip5Hash,
        created_at: i64,
    },
    TxSeenMempoolAccepted {
        proposal: WithdrawalProposalData,
        existing: SequencedWithdrawalView,
        withdrawal_nonce: u64,
        proposal_hash: String,
        authorized_transaction: StoredAuthorizedTransaction,
        created_at: i64,
    },
}

/// Builds an unordered journal record from stable event facts.
///
/// Object-store sequencing is assigned later by `append_and_project_journal_records`.
#[allow(clippy::too_many_arguments)]
fn sequencer_journal_record(
    created_at: i64,
    event_type: SequencerJournalEventType,
    id: &WithdrawalId,
    epoch: u64,
    withdrawal_nonce: Option<u64>,
    base: Option<SequencerJournalBaseContext>,
    nockchain: Option<SequencerJournalNockchainContext>,
    proposal: Option<SequencerJournalProposalContext>,
    submission: Option<SequencerJournalSubmissionContext>,
    confirmation: Option<SequencerJournalConfirmationContext>,
) -> Result<SequencerJournalRecord, BridgeError> {
    sequencer_journal_record_with_request_facts(
        created_at, event_type, id, epoch, withdrawal_nonce, None, base, nockchain, proposal,
        submission, confirmation,
    )
}

#[allow(clippy::too_many_arguments)]
fn sequencer_journal_record_with_request_facts(
    created_at: i64,
    event_type: SequencerJournalEventType,
    id: &WithdrawalId,
    epoch: u64,
    withdrawal_nonce: Option<u64>,
    request_facts: Option<&SequencerWithdrawalRequestFacts>,
    base: Option<SequencerJournalBaseContext>,
    nockchain: Option<SequencerJournalNockchainContext>,
    proposal: Option<SequencerJournalProposalContext>,
    submission: Option<SequencerJournalSubmissionContext>,
    confirmation: Option<SequencerJournalConfirmationContext>,
) -> Result<SequencerJournalRecord, BridgeError> {
    SequencerJournalRecord::new_unsequenced(
        created_at.saturating_mul(1_000),
        event_type,
        SequencerJournalWithdrawal {
            as_of: hex::encode(tip5_to_bytes(&id.as_of)),
            base_event_id: hex::encode(&id.base_event_id.0),
            withdrawal_nonce,
            recipient: request_facts.map(|facts| hex::encode(tip5_to_bytes(&facts.recipient))),
            burned_amount: request_facts.map(|facts| facts.burned_amount),
            base_batch_end: request_facts.map(|facts| facts.base_batch_end),
            epoch,
        },
        base,
        nockchain,
        proposal,
        submission,
        confirmation,
    )
}

/// Builds Base-chain context and omits the whole block when all fields are absent.
fn journal_base_context(
    base_batch_end: Option<u64>,
    turn_started_base_height: Option<u64>,
    last_submit_attempt_base_height: Option<u64>,
) -> Option<SequencerJournalBaseContext> {
    (base_batch_end.is_some()
        || turn_started_base_height.is_some()
        || last_submit_attempt_base_height.is_some())
    .then_some(SequencerJournalBaseContext {
        base_batch_end,
        turn_started_base_height,
        last_submit_attempt_base_height,
    })
}

/// Builds the Nockchain context stored on proposal-owned journal events.
fn proposal_nockchain_context(
    proposal: &WithdrawalProposalData,
) -> Option<SequencerJournalNockchainContext> {
    Some(SequencerJournalNockchainContext {
        snapshot_height: Some(proposal.snapshot.height),
        snapshot_block_id: Some(hex::encode(tip5_to_bytes(&proposal.snapshot.block_id))),
        safe_tip_height_observed_by_writer: None,
    })
}

/// Builds proposal context from the full proposal object for newly authored events.
fn journal_proposal_context_from_proposal(
    proposal: &WithdrawalProposalData,
    proposal_hash: Option<&str>,
    commit_certificate: Option<&WithdrawalCommitCertificate>,
    signer_node_id: Option<u64>,
) -> Result<SequencerJournalProposalContext, BridgeError> {
    Ok(SequencerJournalProposalContext {
        proposal_hash: match proposal_hash {
            Some(proposal_hash) => proposal_hash.to_string(),
            None => proposal.proposal_hash()?,
        },
        amount: Some(proposal.amount),
        transaction_name: Some(withdrawal_raw_tx::submitted_raw_tx_id_base58(
            &proposal.transaction,
        )?),
        transaction_jam: Some(hex::encode(jam_transaction(&proposal.transaction)?)),
        selected_inputs: journal_input_names(&proposal.selected_inputs),
        commit_certificate: commit_certificate
            .map(encode_commit_certificate)
            .transpose()?
            .map(hex::encode),
        signer_node_id,
    })
}

/// Builds proposal context from the current sequencer projection when replaying row-owned events.
fn journal_proposal_context_from_existing(
    existing: &SequencedWithdrawalView,
) -> Result<SequencerJournalProposalContext, BridgeError> {
    let proposal_hash = existing.proposal_hash.clone().ok_or_else(|| {
        BridgeError::Runtime(format!(
            "missing canonical proposal hash for withdrawal {:?}",
            existing.id
        ))
    })?;
    Ok(journal_proposal_context_from_existing_with_hash(
        existing,
        &proposal_hash,
        existing.authorized_transaction_name.clone(),
    ))
}

/// Builds proposal context from a sequencer row when the caller already chose the hash/id.
fn journal_proposal_context_from_existing_with_hash(
    existing: &SequencedWithdrawalView,
    proposal_hash: &str,
    transaction_name: Option<String>,
) -> SequencerJournalProposalContext {
    SequencerJournalProposalContext {
        proposal_hash: proposal_hash.to_string(),
        amount: existing.canonical_amount,
        transaction_name,
        transaction_jam: existing.canonical_transaction_jam.as_ref().map(hex::encode),
        selected_inputs: existing
            .canonical_selected_inputs
            .as_deref()
            .map(journal_input_names)
            .unwrap_or_default(),
        commit_certificate: existing.peer_commit_certificate.as_ref().map(hex::encode),
        signer_node_id: None,
    }
}

/// Encodes selected input note names into stable journal hex fields.
fn journal_input_names(inputs: &[nockchain_types::v1::Name]) -> Vec<SequencerJournalInputName> {
    normalized_note_names(inputs)
        .into_iter()
        .map(|input| SequencerJournalInputName {
            first: hex::encode(tip5_to_bytes(&input.first)),
            last: hex::encode(tip5_to_bytes(&input.last)),
        })
        .collect()
}

/// Builds submission context for durable events that carry submit/retry artifacts.
fn journal_submission_context(
    submitted_raw_tx_id: Option<String>,
    authorized_raw_tx: Option<&[u8]>,
    submit_attempt_count: Option<u64>,
    last_submit_error: Option<&str>,
) -> SequencerJournalSubmissionContext {
    SequencerJournalSubmissionContext {
        submitted_raw_tx_id,
        authorized_raw_tx: authorized_raw_tx.map(hex::encode),
        submit_attempt_count,
        last_submit_error: last_submit_error.map(str::to_string),
    }
}

/// Builds submission context from the current sequencer row for observation-only events.
fn journal_submission_context_from_existing(
    existing: &SequencedWithdrawalView,
    submit_attempt_count: Option<u64>,
    last_submit_error: Option<&str>,
) -> SequencerJournalSubmissionContext {
    journal_submission_context(
        existing.authorized_transaction_name.clone(),
        existing.authorized_raw_tx.as_deref(),
        submit_attempt_count,
        last_submit_error,
    )
}

/// Builds confirmation context for a finalized withdrawal observation.
fn journal_confirmation_context(
    confirmed_height: u64,
    confirmed_block_id: &Tip5Hash,
) -> SequencerJournalConfirmationContext {
    SequencerJournalConfirmationContext {
        included_height: None,
        included_block_id: None,
        confirmed_height: Some(confirmed_height),
        confirmed_block_id: Some(hex::encode(tip5_to_bytes(confirmed_block_id))),
    }
}

/// Decoded identity fields shared by every journal projector.
#[derive(Debug, Clone)]
struct DecodedJournalEvent {
    id: WithdrawalId,
    epoch: u64,
    withdrawal_nonce: Option<u64>,
    request_facts: Option<SequencerWithdrawalRequestFacts>,
    created_at: i64,
}

/// Decodes journal identity fields once before dispatching to an event-specific projector.
fn decode_journal_event_identity(
    event: &SequencerJournalRecord,
) -> Result<DecodedJournalEvent, BridgeError> {
    let as_of = decode_journal_tip5_hex("withdrawal.as_of", &event.withdrawal.as_of)?;
    let base_event_id =
        decode_journal_hex("withdrawal.base_event_id", &event.withdrawal.base_event_id)?;
    let request_facts = match (
        event.withdrawal.recipient.as_deref(),
        event.withdrawal.burned_amount,
        event.withdrawal.base_batch_end,
    ) {
        (Some(recipient), Some(burned_amount), Some(base_batch_end)) => {
            Some(SequencerWithdrawalRequestFacts {
                recipient: decode_journal_tip5_hex("withdrawal.recipient", recipient)?,
                burned_amount,
                base_batch_end,
            })
        }
        (None, None, None) => None,
        _ => {
            return Err(BridgeError::Runtime(format!(
                "sequencer journal event {:?} has incomplete withdrawal request facts",
                event.event_type
            )));
        }
    };
    Ok(DecodedJournalEvent {
        id: WithdrawalId {
            as_of,
            base_event_id: AtomBytes(base_event_id),
        },
        epoch: event.withdrawal.epoch,
        withdrawal_nonce: event.withdrawal.withdrawal_nonce,
        request_facts,
        created_at: event.created_at_unix_ms.div_euclid(1_000),
    })
}

/// Decodes one hex field from the journal and attaches the field name to errors.
fn decode_journal_hex(field: &str, value: &str) -> Result<Vec<u8>, BridgeError> {
    hex::decode(value).map_err(|err| {
        BridgeError::Runtime(format!("failed to decode journal hex field {field}: {err}"))
    })
}

/// Decodes an optional hex field while preserving `None` for omitted context fields.
fn decode_optional_journal_hex(
    field: &str,
    value: Option<&str>,
) -> Result<Option<Vec<u8>>, BridgeError> {
    value
        .map(|value| decode_journal_hex(field, value))
        .transpose()
}

/// Decodes a journal Tip5 hash from its hex byte representation.
fn decode_journal_tip5_hex(field: &str, value: &str) -> Result<Tip5Hash, BridgeError> {
    tip5_from_bytes(&decode_journal_hex(field, value)?)
}

/// Requires the event to carry a withdrawal nonce before projecting nonce-bound state.
fn required_journal_nonce(
    event: &SequencerJournalRecord,
    decoded: &DecodedJournalEvent,
) -> Result<u64, BridgeError> {
    decoded.withdrawal_nonce.ok_or_else(|| {
        BridgeError::Runtime(format!(
            "sequencer journal event {:?} is missing withdrawal nonce",
            event.event_type
        ))
    })
}

/// Requires proposal context for journal events that identify a canonical proposal.
fn require_journal_proposal(
    event: &SequencerJournalRecord,
) -> Result<&SequencerJournalProposalContext, BridgeError> {
    event.proposal.as_ref().ok_or_else(|| {
        BridgeError::Runtime(format!(
            "sequencer journal event {:?} is missing proposal context",
            event.event_type
        ))
    })
}

/// Requires submission context for journal events that update submit metadata.
fn require_journal_submission(
    event: &SequencerJournalRecord,
) -> Result<&SequencerJournalSubmissionContext, BridgeError> {
    event.submission.as_ref().ok_or_else(|| {
        BridgeError::Runtime(format!(
            "sequencer journal event {:?} is missing submission context",
            event.event_type
        ))
    })
}

/// Requires confirmation context for journal events that mark withdrawals confirmed.
fn require_journal_confirmation(
    event: &SequencerJournalRecord,
) -> Result<&SequencerJournalConfirmationContext, BridgeError> {
    event.confirmation.as_ref().ok_or_else(|| {
        BridgeError::Runtime(format!(
            "sequencer journal event {:?} is missing confirmation context",
            event.event_type
        ))
    })
}

/// Reads the Base height associated with a submit attempt from an event.
fn journal_last_submit_attempt_base_height(
    event: &SequencerJournalRecord,
) -> Result<u64, BridgeError> {
    event
        .base
        .as_ref()
        .and_then(|base| base.last_submit_attempt_base_height)
        .ok_or_else(|| {
            BridgeError::Runtime(format!(
                "sequencer journal event {:?} is missing last_submit_attempt_base_height",
                event.event_type
            ))
        })
}

/// Reads the proposer-turn start height if the event carries handoff context.
fn journal_turn_started_base_height(event: &SequencerJournalRecord) -> Option<u64> {
    event
        .base
        .as_ref()
        .and_then(|base| base.turn_started_base_height)
}

/// Decodes selected input names from proposal context for reserved-input replay.
fn decode_journal_inputs(
    proposal: &SequencerJournalProposalContext,
) -> Result<Vec<nockchain_types::v1::Name>, BridgeError> {
    proposal
        .selected_inputs
        .iter()
        .map(|input| {
            Ok(nockchain_types::v1::Name::new(
                decode_journal_tip5_hex("proposal.selected_inputs.first", &input.first)?,
                decode_journal_tip5_hex("proposal.selected_inputs.last", &input.last)?,
            ))
        })
        .collect()
}

/// Returns the submitted raw tx id from submission context, falling back to proposal context.
fn journal_submitted_raw_tx_id(event: &SequencerJournalRecord) -> Option<String> {
    event
        .submission
        .as_ref()
        .and_then(|submission| submission.submitted_raw_tx_id.clone())
        .or_else(|| {
            event
                .proposal
                .as_ref()
                .and_then(|proposal| proposal.transaction_name.clone())
        })
}

/// Reconstructs stored authorized artifacts when an event carries both typed and raw tx bytes.
fn journal_authorized_transaction(
    event: &SequencerJournalRecord,
) -> Result<Option<StoredAuthorizedTransaction>, BridgeError> {
    let Some(submitted_raw_tx_id) = journal_submitted_raw_tx_id(event) else {
        return Ok(None);
    };
    let transaction_jam = decode_optional_journal_hex(
        "proposal.transaction_jam",
        event
            .proposal
            .as_ref()
            .and_then(|proposal| proposal.transaction_jam.as_deref()),
    )?;
    let raw_tx_bytes = decode_optional_journal_hex(
        "submission.authorized_raw_tx",
        event
            .submission
            .as_ref()
            .and_then(|submission| submission.authorized_raw_tx.as_deref()),
    )?;
    Ok(match (transaction_jam, raw_tx_bytes) {
        (Some(transaction_jam), Some(raw_tx_bytes)) => Some(StoredAuthorizedTransaction {
            submitted_raw_tx_id,
            transaction_jam,
            raw_tx_bytes,
        }),
        _ => None,
    })
}

fn require_journal_transaction_jam(
    event: &SequencerJournalRecord,
    proposal: &SequencerJournalProposalContext,
) -> Result<Vec<u8>, BridgeError> {
    decode_optional_journal_hex(
        "proposal.transaction_jam",
        proposal.transaction_jam.as_deref(),
    )?
    .ok_or_else(|| {
        BridgeError::Runtime(format!(
            "sequencer journal event {:?} is missing proposal.transaction_jam",
            event.event_type
        ))
    })
}

fn require_journal_base_batch_end(event: &SequencerJournalRecord) -> Result<u64, BridgeError> {
    event
        .base
        .as_ref()
        .and_then(|base| base.base_batch_end)
        .ok_or_else(|| {
            BridgeError::Runtime(format!(
                "sequencer journal event {:?} is missing base.base_batch_end",
                event.event_type
            ))
        })
}

fn require_journal_snapshot(
    event: &SequencerJournalRecord,
) -> Result<WithdrawalSnapshot, BridgeError> {
    let nockchain = event.nockchain.as_ref().ok_or_else(|| {
        BridgeError::Runtime(format!(
            "sequencer journal event {:?} is missing nockchain context",
            event.event_type
        ))
    })?;
    let height = nockchain.snapshot_height.ok_or_else(|| {
        BridgeError::Runtime(format!(
            "sequencer journal event {:?} is missing nockchain.snapshot_height",
            event.event_type
        ))
    })?;
    let block_id = nockchain
        .snapshot_block_id
        .as_deref()
        .ok_or_else(|| {
            BridgeError::Runtime(format!(
                "sequencer journal event {:?} is missing nockchain.snapshot_block_id",
                event.event_type
            ))
        })
        .and_then(|value| decode_journal_tip5_hex("nockchain.snapshot_block_id", value))?;
    Ok(WithdrawalSnapshot { height, block_id })
}

/// Requires authorized transaction artifacts for events that must be retry-resumable.
fn require_journal_authorized_transaction(
    event: &SequencerJournalRecord,
) -> Result<StoredAuthorizedTransaction, BridgeError> {
    journal_authorized_transaction(event)?.ok_or_else(|| {
        BridgeError::Runtime(format!(
            "sequencer journal event {:?} is missing authorized transaction artifacts",
            event.event_type
        ))
    })
}

/// Ranks states within one Base-burn withdrawal's forward lifecycle.
///
/// This rank is not globally meaningful across different withdrawals; callers
/// must only compare a replay event against the row for the same
/// `base_event_id`. The stored `as_of` is kernel context, not sequencer
/// identity.
fn withdrawal_state_rank(state: WithdrawalState) -> u8 {
    match state {
        WithdrawalState::Pending => 0,
        WithdrawalState::Assembling | WithdrawalState::Prepared => 1,
        WithdrawalState::PeerCanonical => 2,
        WithdrawalState::Authorized => 3,
        WithdrawalState::MempoolAccepted => 4,
        WithdrawalState::Confirmed => 5,
    }
}

/// Fails replay if the projection is already past the event being applied.
///
/// Startup recovery first verifies that the local cursor is the exact SQLite
/// projection frontier. Replaying a cursor successor should therefore never see
/// a row that has already advanced past that successor's target state.
fn ensure_replay_not_past_target_state(
    mode: SequencerJournalApplyMode,
    event_id: &WithdrawalId,
    existing: &SequencedWithdrawalView,
    target_state: WithdrawalState,
    proposal_hash: Option<&str>,
) -> Result<(), BridgeError> {
    if mode != SequencerJournalApplyMode::Replay {
        return Ok(());
    }
    if !same_base_event_id(&existing.id, event_id) {
        return Err(BridgeError::Runtime(format!(
            "journal replay tried to compare state for withdrawal {:?} against event {:?}",
            existing.id, event_id
        )));
    }
    if withdrawal_state_rank(existing.state) > withdrawal_state_rank(target_state) {
        return Err(BridgeError::Runtime(format!(
            "journal replay found withdrawal {:?} state {} ahead of event state {} for proposal {:?}",
            existing.id,
            existing.state.as_str(),
            target_state.as_str(),
            proposal_hash
        )));
    }
    Ok(())
}

/// Verifies that replayed nonce-bound events match the row already materialized locally.
fn ensure_existing_journal_nonce(
    existing: &SequencedWithdrawalView,
    expected_nonce: u64,
) -> Result<(), BridgeError> {
    if existing.withdrawal_nonce != Some(expected_nonce) {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal nonce mismatch for withdrawal {:?}: event {}, existing {:?}",
            existing.id, expected_nonce, existing.withdrawal_nonce
        )));
    }
    Ok(())
}

/// Read-only startup check that the local projection really contains the cursor event.
///
/// This complements the write-time invariant where projection and cursor update
/// happen in one SQLite transaction. It catches out-of-band corruption or manual
/// cursor edits where the cursor points at a valid remote journal object but the
/// sequencer rows do not reflect that event.
fn verify_journal_cursor_event_applied(
    conn: &mut SqliteConnection,
    event: &SequencerJournalRecord,
) -> Result<(), BridgeError> {
    let decoded = decode_journal_event_identity(event)?;
    match event.event_type {
        SequencerJournalEventType::WithdrawalOrdered => {
            verify_cursor_withdrawal_ordered(conn, event, &decoded)
        }
        SequencerJournalEventType::ProposalCanonicalized => {
            verify_cursor_proposal_canonicalized(conn, event, &decoded)
        }
        SequencerJournalEventType::ProposalAuthorized => {
            verify_cursor_proposal_authorized(conn, event, &decoded)
        }
        SequencerJournalEventType::TxSubmitted => verify_cursor_tx_submitted(conn, event, &decoded),
        SequencerJournalEventType::TxSeenMempoolAccepted => {
            verify_cursor_tx_seen_mempool_accepted(conn, event, &decoded)
        }
        SequencerJournalEventType::MempoolRetryAttempted => {
            verify_cursor_mempool_retry_attempted(conn, event, &decoded)
        }
        SequencerJournalEventType::TxConfirmed => verify_cursor_tx_confirmed(conn, event, &decoded),
    }
}

/// Verifies that the cursor's `withdrawal_ordered` event created the nonce-bound
/// sequencer row. No proposal fields should exist yet at this lifecycle point.
fn verify_cursor_withdrawal_ordered(
    conn: &mut SqliteConnection,
    event: &SequencerJournalRecord,
    decoded: &DecodedJournalEvent,
) -> Result<(), BridgeError> {
    verify_cursor_projection(
        conn,
        event,
        decoded,
        CursorProjectionCheck::at(WithdrawalState::Pending)
            .no_proposal_state()
            .no_authorized_artifacts()
            .no_submit_metadata()
            .reservations(CursorReservationCheck::Cleared),
    )
}

/// Verifies that `proposal_canonicalized` advanced the row to peer-canonical,
/// stored the canonical proposal hash/certificate, and reserved its inputs.
fn verify_cursor_proposal_canonicalized(
    conn: &mut SqliteConnection,
    event: &SequencerJournalRecord,
    decoded: &DecodedJournalEvent,
) -> Result<(), BridgeError> {
    verify_cursor_projection(
        conn,
        event,
        decoded,
        CursorProjectionCheck::at(WithdrawalState::PeerCanonical)
            .proposal_hash()
            .commit_certificate()
            .no_authorized_artifacts()
            .no_submit_metadata()
            .reservations(CursorReservationCheck::MatchProposal),
    )
}

/// Verifies that `proposal_authorized` stored the same canonical proposal hash
/// plus the authorized transaction artifacts needed for submit/retry.
fn verify_cursor_proposal_authorized(
    conn: &mut SqliteConnection,
    event: &SequencerJournalRecord,
    decoded: &DecodedJournalEvent,
) -> Result<(), BridgeError> {
    verify_cursor_projection(
        conn,
        event,
        decoded,
        CursorProjectionCheck::at(WithdrawalState::Authorized)
            .proposal_hash()
            .authorized_artifacts()
            .submit_metadata()
            .reservations(CursorReservationCheck::MatchProposal),
    )
}

/// Verifies that `tx_submitted` recorded submit metadata without requiring a
/// lifecycle advance beyond `Authorized`.
fn verify_cursor_tx_submitted(
    conn: &mut SqliteConnection,
    event: &SequencerJournalRecord,
    decoded: &DecodedJournalEvent,
) -> Result<(), BridgeError> {
    verify_cursor_projection(
        conn,
        event,
        decoded,
        CursorProjectionCheck::at(WithdrawalState::Authorized)
            .proposal_hash()
            .authorized_artifacts()
            .submit_metadata()
            .reservations(CursorReservationCheck::MatchProposal),
    )
}

/// Verifies that `tx_seen_mempool_accepted` advanced the row to
/// `MempoolAccepted` while preserving proposal, tx, submit, and reservations.
fn verify_cursor_tx_seen_mempool_accepted(
    conn: &mut SqliteConnection,
    event: &SequencerJournalRecord,
    decoded: &DecodedJournalEvent,
) -> Result<(), BridgeError> {
    verify_cursor_projection(
        conn,
        event,
        decoded,
        CursorProjectionCheck::at(WithdrawalState::MempoolAccepted)
            .proposal_hash()
            .authorized_artifacts()
            .submit_metadata()
            .reservations(CursorReservationCheck::MatchProposal),
    )
}

/// Verifies that `mempool_retry_attempted` only refreshed retry/submit metadata
/// for the already mempool-accepted transaction.
fn verify_cursor_mempool_retry_attempted(
    conn: &mut SqliteConnection,
    event: &SequencerJournalRecord,
    decoded: &DecodedJournalEvent,
) -> Result<(), BridgeError> {
    verify_cursor_projection(
        conn,
        event,
        decoded,
        CursorProjectionCheck::at(WithdrawalState::MempoolAccepted)
            .proposal_hash()
            .authorized_artifacts()
            .submit_metadata(),
    )
}

/// Verifies that `tx_confirmed` finalized the row and cleared live reservations
/// while leaving proposal and authorized tx artifacts available for audit.
fn verify_cursor_tx_confirmed(
    conn: &mut SqliteConnection,
    event: &SequencerJournalRecord,
    decoded: &DecodedJournalEvent,
) -> Result<(), BridgeError> {
    verify_cursor_projection(
        conn,
        event,
        decoded,
        CursorProjectionCheck::at(WithdrawalState::Confirmed)
            .proposal_hash()
            .authorized_artifacts()
            .submit_metadata()
            .reservations(CursorReservationCheck::Cleared),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CursorReservationCheck {
    None,
    MatchProposal,
    Cleared,
}

#[derive(Debug, Clone, Copy)]
struct CursorProjectionCheck {
    target_state: Option<WithdrawalState>,
    verify_epoch: bool,
    proposal_hash: bool,
    commit_certificate: bool,
    authorized_artifacts: bool,
    no_proposal_state: bool,
    no_authorized_artifacts: bool,
    submit_metadata: bool,
    no_submit_metadata: bool,
    reservations: CursorReservationCheck,
}

impl CursorProjectionCheck {
    fn at(target_state: WithdrawalState) -> Self {
        Self {
            target_state: Some(target_state),
            verify_epoch: true,
            proposal_hash: false,
            commit_certificate: false,
            authorized_artifacts: false,
            no_proposal_state: false,
            no_authorized_artifacts: false,
            submit_metadata: false,
            no_submit_metadata: false,
            reservations: CursorReservationCheck::None,
        }
    }

    fn proposal_hash(mut self) -> Self {
        self.proposal_hash = true;
        self
    }

    fn commit_certificate(mut self) -> Self {
        self.commit_certificate = true;
        self
    }

    fn authorized_artifacts(mut self) -> Self {
        self.authorized_artifacts = true;
        self
    }

    fn no_proposal_state(mut self) -> Self {
        self.no_proposal_state = true;
        self
    }

    fn no_authorized_artifacts(mut self) -> Self {
        self.no_authorized_artifacts = true;
        self
    }

    fn submit_metadata(mut self) -> Self {
        self.submit_metadata = true;
        self
    }

    fn no_submit_metadata(mut self) -> Self {
        self.no_submit_metadata = true;
        self
    }

    fn reservations(mut self, reservations: CursorReservationCheck) -> Self {
        self.reservations = reservations;
        self
    }

    fn needs_proposal(self) -> bool {
        self.proposal_hash
            || self.commit_certificate
            || self.reservations == CursorReservationCheck::MatchProposal
    }
}

/// Runs the common cursor-event projection checks shared by durable event types.
///
/// The event-specific wrappers above only declare what facts the event should
/// have projected. This helper performs the common order: load the target row,
/// verify nonce/epoch/state, verify proposal identity, then check any stored tx
/// artifacts, submit metadata, and reserved-input projection.
fn verify_cursor_projection(
    conn: &mut SqliteConnection,
    event: &SequencerJournalRecord,
    decoded: &DecodedJournalEvent,
    check: CursorProjectionCheck,
) -> Result<(), BridgeError> {
    let proposal = check
        .needs_proposal()
        .then(|| require_journal_proposal(event))
        .transpose()?;
    let existing = require_cursor_projection_row(conn, event, decoded)?;
    let withdrawal_nonce = required_journal_nonce(event, decoded)?;
    ensure_existing_journal_nonce(&existing, withdrawal_nonce)?;
    if check.verify_epoch || check.target_state.is_some() {
        verify_cursor_epoch_and_state(event, decoded, &existing, check.target_state)?;
    }
    if check.proposal_hash {
        let proposal = proposal.expect("proposal hash checks require proposal context");
        verify_cursor_proposal_hash(event, &existing, &proposal.proposal_hash)?;
    }
    if check.commit_certificate {
        let proposal = proposal.expect("commit certificate checks require proposal context");
        verify_cursor_commit_certificate(event, &existing, proposal)?;
    }
    if check.no_proposal_state {
        verify_cursor_no_proposal_state(event, &existing)?;
    }
    if check.authorized_artifacts {
        verify_cursor_authorized_artifacts(event, &existing)?;
    }
    if check.no_authorized_artifacts {
        verify_cursor_no_authorized_artifacts(event, &existing)?;
    }
    if check.submit_metadata {
        verify_cursor_submit_metadata(event, &existing)?;
    }
    if check.no_submit_metadata {
        verify_cursor_no_submit_metadata(event, &existing)?;
    }
    match check.reservations {
        CursorReservationCheck::None => Ok(()),
        CursorReservationCheck::MatchProposal => {
            let proposal = proposal.expect("reservation checks require proposal context");
            verify_cursor_reserved_inputs(conn, event, decoded, &existing, proposal)
        }
        CursorReservationCheck::Cleared => {
            verify_cursor_reserved_inputs_cleared(conn, event, decoded)
        }
    }
}

/// Loads the row that should contain the cursor event's projected state.
fn require_cursor_projection_row(
    conn: &mut SqliteConnection,
    event: &SequencerJournalRecord,
    decoded: &DecodedJournalEvent,
) -> Result<SequencedWithdrawalView, BridgeError> {
    fetch_sequenced_withdrawal(conn, &decoded.id.base_event_id)?.ok_or_else(|| {
        cursor_projection_mismatch(event, "missing sequenced withdrawal row for cursor event")
    })
}

/// Checks that the row is for the cursor event's epoch and exact lifecycle
/// state. The startup cursor is the last applied event, so a row that has moved
/// past this state means the projection and cursor are out of sync.
fn verify_cursor_epoch_and_state(
    event: &SequencerJournalRecord,
    decoded: &DecodedJournalEvent,
    existing: &SequencedWithdrawalView,
    target_state: Option<WithdrawalState>,
) -> Result<(), BridgeError> {
    if existing.current_epoch != decoded.epoch {
        return Err(cursor_projection_mismatch(
            event,
            &format!(
                "epoch mismatch: event {}, row {}",
                decoded.epoch, existing.current_epoch
            ),
        ));
    }
    let Some(target_state) = target_state else {
        return Ok(());
    };
    if existing.state != target_state {
        return Err(cursor_projection_mismatch(
            event,
            &format!(
                "state {} does not match cursor state {}",
                existing.state.as_str(),
                target_state.as_str()
            ),
        ));
    }
    Ok(())
}

/// Checks that the single stored proposal identity matches the journal event.
fn verify_cursor_proposal_hash(
    event: &SequencerJournalRecord,
    existing: &SequencedWithdrawalView,
    proposal_hash: &str,
) -> Result<(), BridgeError> {
    if existing.proposal_hash.as_deref() != Some(proposal_hash) {
        return Err(cursor_projection_mismatch(
            event,
            &format!(
                "proposal hash mismatch: event {}, row {:?}",
                proposal_hash, existing.proposal_hash
            ),
        ));
    }
    Ok(())
}

/// Checks that the peer commit certificate bytes match the canonicalized event.
fn verify_cursor_commit_certificate(
    event: &SequencerJournalRecord,
    existing: &SequencedWithdrawalView,
    proposal: &SequencerJournalProposalContext,
) -> Result<(), BridgeError> {
    let expected = decode_optional_journal_hex(
        "proposal.commit_certificate",
        proposal.commit_certificate.as_deref(),
    )?;
    if existing.peer_commit_certificate != expected {
        return Err(cursor_projection_mismatch(
            event, "commit certificate mismatch for cursor event",
        ));
    }
    Ok(())
}

/// Checks that pre-canonical cursor events did not accidentally materialize a
/// proposal hash or commit certificate.
fn verify_cursor_no_proposal_state(
    event: &SequencerJournalRecord,
    existing: &SequencedWithdrawalView,
) -> Result<(), BridgeError> {
    if existing.proposal_hash.is_some() || existing.peer_commit_certificate.is_some() {
        return Err(cursor_projection_mismatch(
            event, "cursor event unexpectedly has proposal state",
        ));
    }
    Ok(())
}

/// Checks that the authorized tx id, structured transaction jam, and raw tx
/// bytes match the journal event. These are the artifacts needed for exact
/// retry/recovery, so a cursor that claims this event must already have them.
fn verify_cursor_authorized_artifacts(
    event: &SequencerJournalRecord,
    existing: &SequencedWithdrawalView,
) -> Result<(), BridgeError> {
    if let Some(expected_raw_tx_id) = journal_submitted_raw_tx_id(event) {
        if existing.authorized_transaction_name.as_deref() != Some(expected_raw_tx_id.as_str()) {
            return Err(cursor_projection_mismatch(
                event,
                &format!(
                    "submitted raw tx id mismatch: event {}, row {:?}",
                    expected_raw_tx_id, existing.authorized_transaction_name
                ),
            ));
        }
    }
    let expected = journal_authorized_transaction(event)?.ok_or_else(|| {
        cursor_projection_mismatch(
            event, "cursor event is missing authorized transaction artifacts",
        )
    })?;
    if existing.authorized_transaction_jam.as_deref() != Some(expected.transaction_jam.as_slice()) {
        return Err(cursor_projection_mismatch(
            event, "authorized transaction jam mismatch for cursor event",
        ));
    }
    if existing.authorized_raw_tx.as_deref() != Some(expected.raw_tx_bytes.as_slice()) {
        return Err(cursor_projection_mismatch(
            event, "authorized raw tx mismatch for cursor event",
        ));
    }
    Ok(())
}

/// Checks that events before authorization did not materialize retryable tx
/// artifacts.
fn verify_cursor_no_authorized_artifacts(
    event: &SequencerJournalRecord,
    existing: &SequencedWithdrawalView,
) -> Result<(), BridgeError> {
    if existing.authorized_transaction_name.is_some()
        || existing.authorized_transaction_jam.is_some()
        || existing.authorized_raw_tx.is_some()
    {
        return Err(cursor_projection_mismatch(
            event, "cursor event unexpectedly has authorized transaction artifacts",
        ));
    }
    Ok(())
}

/// Checks submit/retry metadata exactly. A cursor that names a submit/retry
/// event must point at the same attempt count, Base height, and error currently
/// materialized in SQLite.
fn verify_cursor_submit_metadata(
    event: &SequencerJournalRecord,
    existing: &SequencedWithdrawalView,
) -> Result<(), BridgeError> {
    let Some(submission) = event.submission.as_ref() else {
        return Ok(());
    };
    let Some(expected_count) = submission.submit_attempt_count else {
        return Ok(());
    };
    if existing.submit_attempt_count != expected_count {
        return Err(cursor_projection_mismatch(
            event,
            &format!(
                "submit attempt count mismatch: event {}, row {}",
                expected_count, existing.submit_attempt_count
            ),
        ));
    }
    let expected_base_height = event
        .base
        .as_ref()
        .and_then(|base| base.last_submit_attempt_base_height);
    if existing.last_submit_attempt_base_height != expected_base_height {
        return Err(cursor_projection_mismatch(
            event,
            &format!(
                "last submit Base height mismatch: event {:?}, row {:?}",
                expected_base_height, existing.last_submit_attempt_base_height
            ),
        ));
    }
    if existing.last_submit_error != submission.last_submit_error {
        return Err(cursor_projection_mismatch(
            event, "last submit error mismatch for cursor event",
        ));
    }
    Ok(())
}

/// Checks that events before submit did not materialize submit/retry metadata.
fn verify_cursor_no_submit_metadata(
    event: &SequencerJournalRecord,
    existing: &SequencedWithdrawalView,
) -> Result<(), BridgeError> {
    if existing.submit_attempt_count != 0
        || existing.last_submit_attempt_base_height.is_some()
        || existing.last_submit_error.is_some()
    {
        return Err(cursor_projection_mismatch(
            event, "cursor event unexpectedly has submit metadata",
        ));
    }
    Ok(())
}

/// Checks that a cursor event has no live input reservations.
fn verify_cursor_reserved_inputs_cleared(
    conn: &mut SqliteConnection,
    event: &SequencerJournalRecord,
    decoded: &DecodedJournalEvent,
) -> Result<(), BridgeError> {
    if !load_reserved_input_rows_for_withdrawal(conn, &decoded.id)?.is_empty() {
        return Err(cursor_projection_mismatch(
            event, "cursor event has unexpected reserved inputs",
        ));
    }
    Ok(())
}

/// Checks that live reserved inputs match the canonical proposal unless the row
/// has already been confirmed, in which case reservations must be gone.
fn verify_cursor_reserved_inputs(
    conn: &mut SqliteConnection,
    event: &SequencerJournalRecord,
    decoded: &DecodedJournalEvent,
    existing: &SequencedWithdrawalView,
    proposal: &SequencerJournalProposalContext,
) -> Result<(), BridgeError> {
    let expected = decode_journal_inputs(proposal)?;
    if expected.is_empty() {
        return Ok(());
    }
    let mut actual = load_reserved_input_rows_for_withdrawal(conn, &decoded.id)?
        .into_iter()
        .map(|row| row.input)
        .collect::<Vec<_>>();
    actual.sort_by_key(note_name_sort_key);
    if existing.state == WithdrawalState::Confirmed {
        if actual.is_empty() {
            return Ok(());
        }
        return Err(cursor_projection_mismatch(
            event, "confirmed cursor projection still has reserved inputs",
        ));
    }
    if actual != normalized_note_names(&expected) {
        return Err(cursor_projection_mismatch(
            event, "reserved inputs do not match cursor event",
        ));
    }
    Ok(())
}

/// Builds the shared fail-closed error for cursor/projection mismatches.
fn cursor_projection_mismatch(event: &SequencerJournalRecord, detail: &str) -> BridgeError {
    metrics::init_metrics()
        .sequencer_withdrawal_journal_projection_mismatch
        .increment();
    BridgeError::Runtime(format!(
        "sequencer journal cursor event {:?} sequence {} is not reflected in SQLite projection: {detail}",
        event.event_type, event.sequence
    ))
}

impl SequencerMutation {
    /// Returns the durable journal records that must be appended before this mutation applies.
    ///
    /// Empty means the mutation is local diagnostic/coordination state only and
    /// does not participate in exact remote journal replay.
    fn journal_records(&self) -> Result<Vec<SequencerJournalRecord>, BridgeError> {
        match self {
            Self::WithdrawalOrdered {
                id,
                withdrawal_nonce,
                request_facts,
                turn_started_base_height,
                created_at,
            } => Ok(vec![sequencer_journal_record_with_request_facts(
                *created_at,
                SequencerJournalEventType::WithdrawalOrdered,
                id,
                0,
                Some(*withdrawal_nonce),
                Some(request_facts),
                journal_base_context(None, *turn_started_base_height, None),
                None,
                None,
                None,
                None,
            )?]),
            Self::ProposalSigned { .. }
            | Self::ProposerTurnExpiredForProposal { .. }
            | Self::ProposerTurnExpiredForRow { .. }
            | Self::PrecanonicalHandoff { .. } => Ok(Vec::new()),
            Self::ProposalCanonicalized {
                proposal,
                withdrawal_nonce,
                proposal_hash,
                commit_certificate,
                turn_started_base_height,
                created_at,
                ..
            } => Ok(vec![sequencer_journal_record(
                *created_at,
                SequencerJournalEventType::ProposalCanonicalized,
                &proposal.id,
                proposal.epoch,
                Some(*withdrawal_nonce),
                journal_base_context(
                    Some(proposal.base_batch_end),
                    Some(*turn_started_base_height),
                    None,
                ),
                proposal_nockchain_context(proposal),
                Some(journal_proposal_context_from_proposal(
                    proposal,
                    Some(proposal_hash),
                    commit_certificate.as_ref(),
                    None,
                )?),
                None,
                None,
            )?]),
            Self::ProposalAuthorized {
                proposal,
                existing,
                proposal_hash,
                authorized_transaction,
                turn_started_base_height,
                created_at,
            } => Ok(vec![sequencer_journal_record(
                *created_at,
                SequencerJournalEventType::ProposalAuthorized,
                &proposal.id,
                proposal.epoch,
                existing.withdrawal_nonce,
                journal_base_context(
                    Some(proposal.base_batch_end),
                    turn_started_base_height.or(existing.turn_started_base_height),
                    None,
                ),
                proposal_nockchain_context(proposal),
                Some(journal_proposal_context_from_proposal(
                    proposal,
                    Some(proposal_hash),
                    None,
                    None,
                )?),
                Some(journal_submission_context(
                    Some(authorized_transaction.submitted_raw_tx_id.clone()),
                    Some(&authorized_transaction.raw_tx_bytes),
                    Some(existing.submit_attempt_count),
                    existing.last_submit_error.as_deref(),
                )),
                None,
            )?]),
            Self::SubmitOutcome {
                proposal,
                existing,
                final_state,
                authorized_transaction,
                submit_attempt_count,
                last_submit_attempt_base_height,
                last_submit_error,
                created_at,
            } => {
                let mut records = vec![sequencer_journal_record(
                    *created_at,
                    SequencerJournalEventType::TxSubmitted,
                    &proposal.id,
                    proposal.epoch,
                    existing.withdrawal_nonce,
                    journal_base_context(
                        Some(proposal.base_batch_end),
                        existing.turn_started_base_height,
                        Some(*last_submit_attempt_base_height),
                    ),
                    proposal_nockchain_context(proposal),
                    Some(journal_proposal_context_from_proposal(
                        proposal, None, None, None,
                    )?),
                    Some(journal_submission_context(
                        Some(authorized_transaction.submitted_raw_tx_id.clone()),
                        Some(&authorized_transaction.raw_tx_bytes),
                        Some(*submit_attempt_count),
                        last_submit_error.as_deref(),
                    )),
                    None,
                )?];
                match final_state {
                    WithdrawalState::MempoolAccepted => {
                        records.push(sequencer_journal_record(
                            *created_at,
                            SequencerJournalEventType::TxSeenMempoolAccepted,
                            &proposal.id,
                            proposal.epoch,
                            existing.withdrawal_nonce,
                            journal_base_context(
                                Some(proposal.base_batch_end),
                                existing.turn_started_base_height,
                                Some(*last_submit_attempt_base_height),
                            ),
                            proposal_nockchain_context(proposal),
                            Some(journal_proposal_context_from_proposal(
                                proposal, None, None, None,
                            )?),
                            Some(journal_submission_context(
                                Some(authorized_transaction.submitted_raw_tx_id.clone()),
                                Some(&authorized_transaction.raw_tx_bytes),
                                Some(*submit_attempt_count),
                                last_submit_error.as_deref(),
                            )),
                            None,
                        )?);
                    }
                    WithdrawalState::Authorized => {}
                    other => {
                        return Err(BridgeError::Runtime(format!(
                            "invalid final submit state {}",
                            other.as_str()
                        )));
                    }
                }
                Ok(records)
            }
            Self::AuthorizedMempoolAccepted {
                proposal,
                existing,
                created_at,
            } => Ok(vec![sequencer_journal_record(
                *created_at,
                SequencerJournalEventType::TxSeenMempoolAccepted,
                &proposal.id,
                proposal.epoch,
                existing.withdrawal_nonce,
                journal_base_context(
                    Some(proposal.base_batch_end),
                    existing.turn_started_base_height,
                    existing.last_submit_attempt_base_height,
                ),
                proposal_nockchain_context(proposal),
                Some(journal_proposal_context_from_proposal(
                    proposal, None, None, None,
                )?),
                Some(journal_submission_context_from_existing(
                    existing,
                    Some(existing.submit_attempt_count),
                    None,
                )),
                None,
            )?]),
            Self::MempoolRetryAttempted {
                existing,
                attempt_base_height,
                error,
                updated_at,
            } => Ok(vec![sequencer_journal_record(
                *updated_at,
                SequencerJournalEventType::MempoolRetryAttempted,
                &existing.id,
                existing.current_epoch,
                existing.withdrawal_nonce,
                journal_base_context(
                    None,
                    existing.turn_started_base_height,
                    Some(*attempt_base_height),
                ),
                None,
                Some(journal_proposal_context_from_existing(existing)?),
                Some(journal_submission_context_from_existing(
                    existing,
                    Some(existing.submit_attempt_count.saturating_add(1)),
                    error.as_deref(),
                )),
                None,
            )?]),
            Self::TxConfirmed {
                proposal,
                existing,
                withdrawal_nonce,
                proposal_hash,
                authorized_transaction,
                confirmed_height,
                confirmed_block_id,
                created_at,
            } => Ok(vec![sequencer_journal_record(
                *created_at,
                SequencerJournalEventType::TxConfirmed,
                &proposal.id,
                proposal.epoch,
                Some(*withdrawal_nonce),
                journal_base_context(
                    Some(proposal.base_batch_end),
                    existing.turn_started_base_height,
                    existing.last_submit_attempt_base_height,
                ),
                proposal_nockchain_context(proposal),
                Some(journal_proposal_context_from_proposal(
                    proposal,
                    Some(proposal_hash),
                    None,
                    None,
                )?),
                Some(journal_submission_context(
                    Some(authorized_transaction.submitted_raw_tx_id.clone()),
                    Some(&authorized_transaction.raw_tx_bytes),
                    Some(existing.submit_attempt_count),
                    existing.last_submit_error.as_deref(),
                )),
                Some(journal_confirmation_context(
                    *confirmed_height, confirmed_block_id,
                )),
            )?]),
            Self::TxConfirmedById {
                id,
                existing,
                withdrawal_nonce,
                proposal_hash,
                transaction_name,
                confirmed_height,
                confirmed_block_id,
                created_at,
            } => Ok(vec![sequencer_journal_record(
                *created_at,
                SequencerJournalEventType::TxConfirmed,
                id,
                existing.current_epoch,
                Some(*withdrawal_nonce),
                journal_base_context(
                    None, existing.turn_started_base_height,
                    existing.last_submit_attempt_base_height,
                ),
                None,
                Some(journal_proposal_context_from_existing_with_hash(
                    existing,
                    proposal_hash,
                    Some(transaction_name.clone()),
                )),
                Some(journal_submission_context_from_existing(
                    existing,
                    Some(existing.submit_attempt_count),
                    existing.last_submit_error.as_deref(),
                )),
                Some(journal_confirmation_context(
                    *confirmed_height, confirmed_block_id,
                )),
            )?]),
            Self::TxSeenMempoolAccepted {
                proposal,
                existing,
                withdrawal_nonce,
                proposal_hash,
                authorized_transaction,
                created_at,
            } => Ok(vec![sequencer_journal_record(
                *created_at,
                SequencerJournalEventType::TxSeenMempoolAccepted,
                &proposal.id,
                proposal.epoch,
                Some(*withdrawal_nonce),
                journal_base_context(
                    Some(proposal.base_batch_end),
                    existing.turn_started_base_height,
                    existing.last_submit_attempt_base_height,
                ),
                proposal_nockchain_context(proposal),
                Some(journal_proposal_context_from_proposal(
                    proposal,
                    Some(proposal_hash),
                    None,
                    None,
                )?),
                Some(journal_submission_context(
                    Some(authorized_transaction.submitted_raw_tx_id.clone()),
                    Some(&authorized_transaction.raw_tx_bytes),
                    Some(existing.submit_attempt_count),
                    existing.last_submit_error.as_deref(),
                )),
                None,
            )?]),
        }
    }

    /// Returns local append-only debug rows for operator diagnostics.
    ///
    /// These rows are useful locally but are not the remote recovery source of truth.
    fn debug_rows(&self) -> Result<Vec<NewWithdrawalSubmissionEventRow>, BridgeError> {
        match self {
            Self::WithdrawalOrdered { id, created_at, .. } => {
                Ok(vec![NewWithdrawalSubmissionEventRow::from_withdrawal_id(
                    id,
                    0,
                    WithdrawalSubmissionEventType::WithdrawalOrdered,
                    *created_at,
                )?])
            }
            Self::ProposalSigned {
                proposal,
                signer_node_id,
                created_at,
                ..
            } => Ok(vec![NewWithdrawalSubmissionEventRow::from_proposal(
                proposal,
                WithdrawalSubmissionEventType::ProposalSigned,
                *created_at,
                Some(*signer_node_id),
                None,
                None,
            )?]),
            Self::ProposalCanonicalized {
                proposal,
                commit_certificate,
                created_at,
                ..
            } => Ok(vec![NewWithdrawalSubmissionEventRow::from_proposal(
                proposal,
                WithdrawalSubmissionEventType::ProposalCanonicalized,
                *created_at,
                None,
                commit_certificate.as_ref(),
                None,
            )?]),
            Self::ProposerTurnExpiredForProposal {
                proposal,
                created_at,
                ..
            } => Ok(vec![NewWithdrawalSubmissionEventRow::from_proposal(
                proposal,
                WithdrawalSubmissionEventType::ProposerTurnExpired,
                *created_at,
                None,
                None,
                None,
            )?]),
            Self::ProposerTurnExpiredForRow {
                existing,
                created_at,
                ..
            } => Ok(vec![
                NewWithdrawalSubmissionEventRow::from_existing_sequencer_row(
                    existing,
                    WithdrawalSubmissionEventType::ProposerTurnExpired,
                    *created_at,
                )?,
            ]),
            Self::PrecanonicalHandoff {
                existing,
                created_at,
                ..
            } => Ok(vec![NewWithdrawalSubmissionEventRow::from_withdrawal_id(
                &existing.id,
                existing.current_epoch,
                WithdrawalSubmissionEventType::PrecanonicalHandoff,
                *created_at,
            )?]),
            Self::ProposalAuthorized {
                proposal,
                created_at,
                ..
            } => Ok(vec![NewWithdrawalSubmissionEventRow::from_proposal(
                proposal,
                WithdrawalSubmissionEventType::ProposalAuthorized,
                *created_at,
                None,
                None,
                None,
            )?]),
            Self::SubmitOutcome {
                proposal,
                final_state,
                created_at,
                ..
            } => {
                let submit_event = NewWithdrawalSubmissionEventRow::from_proposal(
                    proposal,
                    WithdrawalSubmissionEventType::TxSubmitted,
                    *created_at,
                    None,
                    None,
                    None,
                )?;
                let outcome_event_type = match final_state {
                    WithdrawalState::MempoolAccepted => {
                        Some(WithdrawalSubmissionEventType::TxSeenMempoolAccepted)
                    }
                    WithdrawalState::Authorized => None,
                    other => {
                        return Err(BridgeError::Runtime(format!(
                            "invalid final submit state {}",
                            other.as_str()
                        )));
                    }
                };
                let mut events = vec![submit_event];
                if let Some(outcome_event_type) = outcome_event_type {
                    events.push(NewWithdrawalSubmissionEventRow::from_proposal(
                        proposal, outcome_event_type, *created_at, None, None, None,
                    )?);
                }
                Ok(events)
            }
            Self::AuthorizedMempoolAccepted {
                proposal,
                created_at,
                ..
            }
            | Self::TxSeenMempoolAccepted {
                proposal,
                created_at,
                ..
            } => Ok(vec![NewWithdrawalSubmissionEventRow::from_proposal(
                proposal,
                WithdrawalSubmissionEventType::TxSeenMempoolAccepted,
                *created_at,
                None,
                None,
                None,
            )?]),
            Self::MempoolRetryAttempted {
                existing,
                updated_at,
                ..
            } => Ok(vec![
                NewWithdrawalSubmissionEventRow::from_existing_sequencer_row(
                    existing,
                    WithdrawalSubmissionEventType::MempoolRetryAttempted,
                    *updated_at,
                )?,
            ]),
            Self::TxConfirmed {
                proposal,
                confirmed_height,
                confirmed_block_id,
                created_at,
                ..
            } => Ok(vec![NewWithdrawalSubmissionEventRow::from_proposal(
                proposal,
                WithdrawalSubmissionEventType::TxConfirmed,
                *created_at,
                None,
                None,
                Some((*confirmed_height, confirmed_block_id.clone())),
            )?]),
            Self::TxConfirmedById {
                id,
                existing,
                proposal_hash,
                transaction_name,
                confirmed_height,
                confirmed_block_id,
                created_at,
                ..
            } => Ok(vec![NewWithdrawalSubmissionEventRow {
                created_at: *created_at,
                withdrawal_id_as_of: tip5_to_bytes(&id.as_of),
                withdrawal_id_base_event_id: id.base_event_id.0.clone(),
                epoch: i64::try_from(existing.current_epoch).map_err(|err| {
                    BridgeError::ValueConversion(format!("epoch too large: {err}"))
                })?,
                proposal_hash: proposal_hash.clone(),
                transaction_name: transaction_name.clone(),
                event_type: WithdrawalSubmissionEventType::TxConfirmed
                    .as_str()
                    .to_string(),
                signer_node_id: None,
                commit_certificate: None,
                transaction_jam: None,
                snapshot_height: None,
                snapshot_block_id: None,
                confirmed_height: Some(i64::try_from(*confirmed_height).map_err(|err| {
                    BridgeError::ValueConversion(format!("confirmed height too large: {err}"))
                })?),
                confirmed_block_id: Some(tip5_to_bytes(confirmed_block_id)),
            }]),
        }
    }

    /// Applies the typed SQLite mutation after any required journaling has already completed.
    ///
    /// Durable events normally project through `apply_journal_event`; this path
    /// is still used for local-only mutations and as the structured apply logic
    /// for mutation variants that have no remote journal record.
    fn apply(self, conn: &mut SqliteConnection) -> Result<(), BridgeError> {
        match self {
            Self::WithdrawalOrdered {
                id,
                withdrawal_nonce,
                request_facts,
                turn_started_base_height,
                created_at,
            } => create_ordered_sequencer_row(
                conn, &id, withdrawal_nonce, request_facts, turn_started_base_height, created_at,
            ),
            Self::ProposalSigned {
                proposal,
                existing,
                next_turn_started_base_height,
                created_at,
                ..
            } => upsert_sequenced_withdrawal(
                conn,
                SequencerWithdrawalUpdate {
                    id: proposal.id.clone(),
                    withdrawal_nonce: existing
                        .withdrawal_nonce
                        .expect("sequenced withdrawal rows must carry a nonce"),
                    current_epoch: existing.current_epoch,
                    proposal_hash: existing.proposal_hash,
                    request_facts: existing.request_facts,
                    canonical_amount: existing.canonical_amount,
                    canonical_base_batch_end: existing.canonical_base_batch_end,
                    canonical_transaction_jam: existing.canonical_transaction_jam,
                    canonical_selected_inputs: existing.canonical_selected_inputs,
                    canonical_snapshot: existing.canonical_snapshot,
                    peer_commit_certificate: existing.peer_commit_certificate,
                    authorized_transaction_name: existing.authorized_transaction_name,
                    authorized_transaction_jam: existing.authorized_transaction_jam,
                    authorized_raw_tx: existing.authorized_raw_tx,
                    handoff_index: existing.handoff_index,
                    turn_started_base_height: next_turn_started_base_height,
                    submit_attempt_count: existing.submit_attempt_count,
                    last_submit_attempt_base_height: existing.last_submit_attempt_base_height,
                    last_submit_error: existing.last_submit_error,
                    state: existing.state,
                    created_at: existing.created_at,
                    updated_at: created_at,
                },
            ),
            Self::ProposalCanonicalized {
                proposal,
                existing,
                withdrawal_nonce,
                proposal_hash,
                commit_certificate,
                turn_started_base_height,
                created_at,
            } => {
                upsert_sequenced_withdrawal(
                    conn,
                    SequencerWithdrawalUpdate {
                        id: proposal.id.clone(),
                        withdrawal_nonce,
                        current_epoch: proposal.epoch,
                        proposal_hash: Some(proposal_hash),
                        request_facts: existing.request_facts.or_else(|| {
                            Some(SequencerWithdrawalRequestFacts::from_proposal(&proposal))
                        }),
                        canonical_amount: canonical_amount_from_proposal(&proposal),
                        canonical_base_batch_end: canonical_base_batch_end_from_proposal(&proposal),
                        canonical_transaction_jam: canonical_transaction_jam_from_proposal(
                            &proposal,
                        )?,
                        canonical_selected_inputs: canonical_selected_inputs_from_proposal(
                            &proposal,
                        ),
                        canonical_snapshot: canonical_snapshot_from_proposal(&proposal),
                        peer_commit_certificate: commit_certificate
                            .as_ref()
                            .map(encode_commit_certificate)
                            .transpose()?,
                        authorized_transaction_name: None,
                        authorized_transaction_jam: None,
                        authorized_raw_tx: None,
                        handoff_index: existing.handoff_index,
                        turn_started_base_height: Some(turn_started_base_height),
                        submit_attempt_count: 0,
                        last_submit_attempt_base_height: None,
                        last_submit_error: None,
                        state: WithdrawalState::PeerCanonical,
                        created_at: existing.created_at,
                        updated_at: created_at,
                    },
                )?;
                insert_reserved_inputs_for_proposal(conn, &proposal, created_at)
            }
            Self::ProposerTurnExpiredForProposal {
                existing,
                next_handoff_index,
                next_turn_started_base_height,
                created_at,
                ..
            }
            | Self::ProposerTurnExpiredForRow {
                existing,
                next_handoff_index,
                next_turn_started_base_height,
                created_at,
            } => upsert_sequenced_withdrawal(
                conn,
                SequencerWithdrawalUpdate {
                    id: existing.id,
                    withdrawal_nonce: existing
                        .withdrawal_nonce
                        .expect("sequenced withdrawal rows must carry a nonce"),
                    current_epoch: existing.current_epoch,
                    proposal_hash: existing.proposal_hash,
                    request_facts: existing.request_facts,
                    canonical_amount: existing.canonical_amount,
                    canonical_base_batch_end: existing.canonical_base_batch_end,
                    canonical_transaction_jam: existing.canonical_transaction_jam,
                    canonical_selected_inputs: existing.canonical_selected_inputs,
                    canonical_snapshot: existing.canonical_snapshot,
                    peer_commit_certificate: existing.peer_commit_certificate,
                    authorized_transaction_name: existing.authorized_transaction_name,
                    authorized_transaction_jam: existing.authorized_transaction_jam,
                    authorized_raw_tx: existing.authorized_raw_tx,
                    handoff_index: next_handoff_index,
                    turn_started_base_height: Some(next_turn_started_base_height),
                    submit_attempt_count: existing.submit_attempt_count,
                    last_submit_attempt_base_height: existing.last_submit_attempt_base_height,
                    last_submit_error: existing.last_submit_error,
                    state: existing.state,
                    created_at: existing.created_at,
                    updated_at: created_at,
                },
            ),
            Self::PrecanonicalHandoff {
                existing,
                next_handoff_index,
                turn_started_base_height,
                created_at,
            } => upsert_sequenced_withdrawal(
                conn,
                SequencerWithdrawalUpdate {
                    id: existing.id,
                    withdrawal_nonce: existing
                        .withdrawal_nonce
                        .expect("sequenced withdrawal rows must carry a nonce"),
                    current_epoch: existing.current_epoch,
                    proposal_hash: existing.proposal_hash,
                    request_facts: existing.request_facts,
                    canonical_amount: existing.canonical_amount,
                    canonical_base_batch_end: existing.canonical_base_batch_end,
                    canonical_transaction_jam: existing.canonical_transaction_jam,
                    canonical_selected_inputs: existing.canonical_selected_inputs,
                    canonical_snapshot: existing.canonical_snapshot,
                    peer_commit_certificate: existing.peer_commit_certificate,
                    authorized_transaction_name: existing.authorized_transaction_name,
                    authorized_transaction_jam: existing.authorized_transaction_jam,
                    authorized_raw_tx: existing.authorized_raw_tx,
                    handoff_index: next_handoff_index,
                    turn_started_base_height: Some(turn_started_base_height),
                    submit_attempt_count: existing.submit_attempt_count,
                    last_submit_attempt_base_height: existing.last_submit_attempt_base_height,
                    last_submit_error: existing.last_submit_error,
                    state: existing.state,
                    created_at: existing.created_at,
                    updated_at: created_at,
                },
            ),
            Self::ProposalAuthorized {
                proposal,
                existing,
                proposal_hash,
                authorized_transaction,
                turn_started_base_height,
                created_at,
            } => upsert_sequenced_withdrawal(
                conn,
                SequencerWithdrawalUpdate {
                    id: proposal.id.clone(),
                    withdrawal_nonce: existing
                        .withdrawal_nonce
                        .expect("sequenced withdrawal rows must carry a nonce"),
                    current_epoch: proposal.epoch,
                    peer_commit_certificate: existing.peer_commit_certificate,
                    proposal_hash: Some(proposal_hash),
                    request_facts: existing.request_facts.or_else(|| {
                        Some(SequencerWithdrawalRequestFacts::from_proposal(&proposal))
                    }),
                    canonical_amount: existing
                        .canonical_amount
                        .or_else(|| canonical_amount_from_proposal(&proposal)),
                    canonical_base_batch_end: existing
                        .canonical_base_batch_end
                        .or_else(|| canonical_base_batch_end_from_proposal(&proposal)),
                    canonical_transaction_jam: existing
                        .canonical_transaction_jam
                        .or(canonical_transaction_jam_from_proposal(&proposal)?),
                    canonical_selected_inputs: existing
                        .canonical_selected_inputs
                        .or_else(|| canonical_selected_inputs_from_proposal(&proposal)),
                    canonical_snapshot: existing
                        .canonical_snapshot
                        .or_else(|| canonical_snapshot_from_proposal(&proposal)),
                    authorized_transaction_name: Some(authorized_transaction.submitted_raw_tx_id),
                    authorized_transaction_jam: Some(authorized_transaction.transaction_jam),
                    authorized_raw_tx: Some(authorized_transaction.raw_tx_bytes),
                    handoff_index: existing.handoff_index,
                    turn_started_base_height: turn_started_base_height
                        .or(existing.turn_started_base_height),
                    submit_attempt_count: existing.submit_attempt_count,
                    last_submit_attempt_base_height: existing.last_submit_attempt_base_height,
                    last_submit_error: existing.last_submit_error,
                    state: WithdrawalState::Authorized,
                    created_at,
                    updated_at: created_at,
                },
            ),
            Self::SubmitOutcome {
                existing,
                final_state,
                authorized_transaction,
                submit_attempt_count,
                last_submit_attempt_base_height,
                last_submit_error,
                created_at,
                ..
            } => update_sequencer_submission_state_tx(
                conn,
                &existing,
                SequencerSubmissionStateUpdate {
                    next_state: final_state,
                    authorized_transaction: Some(authorized_transaction),
                    submit_attempt_count,
                    last_submit_attempt_base_height,
                    last_submit_error,
                    updated_at: created_at,
                    action: "submit outcome recording",
                },
            ),
            Self::AuthorizedMempoolAccepted {
                existing,
                created_at,
                ..
            } => update_sequencer_submission_state_tx(
                conn,
                &existing,
                SequencerSubmissionStateUpdate {
                    next_state: WithdrawalState::MempoolAccepted,
                    authorized_transaction: None,
                    submit_attempt_count: existing.submit_attempt_count,
                    last_submit_attempt_base_height: existing
                        .last_submit_attempt_base_height
                        .unwrap_or_default(),
                    last_submit_error: None,
                    updated_at: created_at,
                    action: "authorized mempool-accepted observation",
                },
            ),
            Self::MempoolRetryAttempted {
                existing,
                attempt_base_height,
                error,
                updated_at,
            } => update_sequencer_submission_state_tx(
                conn,
                &existing,
                SequencerSubmissionStateUpdate {
                    next_state: WithdrawalState::MempoolAccepted,
                    authorized_transaction: None,
                    submit_attempt_count: existing.submit_attempt_count.saturating_add(1),
                    last_submit_attempt_base_height: attempt_base_height,
                    last_submit_error: error,
                    updated_at,
                    action: "orphan retry recording",
                },
            ),
            Self::TxConfirmed {
                proposal,
                existing,
                withdrawal_nonce,
                proposal_hash,
                authorized_transaction,
                created_at,
                ..
            } => {
                upsert_sequenced_withdrawal(
                    conn,
                    SequencerWithdrawalUpdate {
                        id: proposal.id.clone(),
                        withdrawal_nonce,
                        current_epoch: proposal.epoch,
                        peer_commit_certificate: existing.peer_commit_certificate,
                        proposal_hash: Some(proposal_hash),
                        request_facts: existing.request_facts.or_else(|| {
                            Some(SequencerWithdrawalRequestFacts::from_proposal(&proposal))
                        }),
                        canonical_amount: existing
                            .canonical_amount
                            .or_else(|| canonical_amount_from_proposal(&proposal)),
                        canonical_base_batch_end: existing
                            .canonical_base_batch_end
                            .or_else(|| canonical_base_batch_end_from_proposal(&proposal)),
                        canonical_transaction_jam: existing
                            .canonical_transaction_jam
                            .or(canonical_transaction_jam_from_proposal(&proposal)?),
                        canonical_selected_inputs: existing
                            .canonical_selected_inputs
                            .or_else(|| canonical_selected_inputs_from_proposal(&proposal)),
                        canonical_snapshot: existing
                            .canonical_snapshot
                            .or_else(|| canonical_snapshot_from_proposal(&proposal)),
                        authorized_transaction_name: Some(
                            authorized_transaction.submitted_raw_tx_id,
                        ),
                        authorized_transaction_jam: Some(authorized_transaction.transaction_jam),
                        authorized_raw_tx: Some(authorized_transaction.raw_tx_bytes),
                        handoff_index: existing.handoff_index,
                        turn_started_base_height: existing.turn_started_base_height,
                        submit_attempt_count: existing.submit_attempt_count,
                        last_submit_attempt_base_height: existing.last_submit_attempt_base_height,
                        last_submit_error: existing.last_submit_error,
                        state: WithdrawalState::Confirmed,
                        created_at,
                        updated_at: created_at,
                    },
                )?;
                clear_reserved_inputs_for_withdrawal(conn, &proposal.id)
            }
            Self::TxConfirmedById {
                id,
                existing,
                withdrawal_nonce,
                proposal_hash,
                transaction_name,
                created_at,
                ..
            } => {
                upsert_sequenced_withdrawal(
                    conn,
                    SequencerWithdrawalUpdate {
                        id,
                        withdrawal_nonce,
                        current_epoch: existing.current_epoch,
                        peer_commit_certificate: existing.peer_commit_certificate,
                        proposal_hash: Some(proposal_hash),
                        request_facts: existing.request_facts,
                        canonical_amount: existing.canonical_amount,
                        canonical_base_batch_end: existing.canonical_base_batch_end,
                        canonical_transaction_jam: existing.canonical_transaction_jam,
                        canonical_selected_inputs: existing.canonical_selected_inputs,
                        canonical_snapshot: existing.canonical_snapshot,
                        authorized_transaction_name: Some(transaction_name),
                        authorized_transaction_jam: existing.authorized_transaction_jam,
                        authorized_raw_tx: existing.authorized_raw_tx,
                        handoff_index: existing.handoff_index,
                        turn_started_base_height: existing.turn_started_base_height,
                        submit_attempt_count: existing.submit_attempt_count,
                        last_submit_attempt_base_height: existing.last_submit_attempt_base_height,
                        last_submit_error: existing.last_submit_error,
                        state: WithdrawalState::Confirmed,
                        created_at,
                        updated_at: created_at,
                    },
                )?;
                clear_reserved_inputs_for_withdrawal(conn, &existing.id)
            }
            Self::TxSeenMempoolAccepted {
                proposal,
                existing,
                withdrawal_nonce,
                proposal_hash,
                authorized_transaction,
                created_at,
            } => upsert_sequenced_withdrawal(
                conn,
                SequencerWithdrawalUpdate {
                    id: proposal.id.clone(),
                    withdrawal_nonce,
                    current_epoch: proposal.epoch,
                    peer_commit_certificate: existing.peer_commit_certificate,
                    proposal_hash: Some(proposal_hash),
                    request_facts: existing.request_facts.or_else(|| {
                        Some(SequencerWithdrawalRequestFacts::from_proposal(&proposal))
                    }),
                    canonical_amount: existing
                        .canonical_amount
                        .or_else(|| canonical_amount_from_proposal(&proposal)),
                    canonical_base_batch_end: existing
                        .canonical_base_batch_end
                        .or_else(|| canonical_base_batch_end_from_proposal(&proposal)),
                    canonical_transaction_jam: existing
                        .canonical_transaction_jam
                        .or(canonical_transaction_jam_from_proposal(&proposal)?),
                    canonical_selected_inputs: existing
                        .canonical_selected_inputs
                        .or_else(|| canonical_selected_inputs_from_proposal(&proposal)),
                    canonical_snapshot: existing
                        .canonical_snapshot
                        .or_else(|| canonical_snapshot_from_proposal(&proposal)),
                    authorized_transaction_name: Some(authorized_transaction.submitted_raw_tx_id),
                    authorized_transaction_jam: Some(authorized_transaction.transaction_jam),
                    authorized_raw_tx: Some(authorized_transaction.raw_tx_bytes),
                    handoff_index: existing.handoff_index,
                    turn_started_base_height: existing.turn_started_base_height,
                    submit_attempt_count: existing.submit_attempt_count,
                    last_submit_attempt_base_height: existing.last_submit_attempt_base_height,
                    last_submit_error: existing.last_submit_error,
                    state: WithdrawalState::MempoolAccepted,
                    created_at,
                    updated_at: created_at,
                },
            ),
        }
    }
}

/// Applies one durable journal event to the local SQLite projection.
///
/// Runtime mode is used immediately after append. Replay mode is used for
/// startup/catch-up and fails closed if the local projection is already past
/// the replayed event.
fn apply_journal_event(
    conn: &mut SqliteConnection,
    event: &SequencerJournalRecord,
    mode: SequencerJournalApplyMode,
) -> Result<(), BridgeError> {
    let decoded = decode_journal_event_identity(event)?;
    match event.event_type {
        SequencerJournalEventType::WithdrawalOrdered => {
            apply_journal_withdrawal_ordered(conn, event, &decoded, mode)
        }
        SequencerJournalEventType::ProposalCanonicalized => {
            apply_journal_proposal_canonicalized(conn, event, &decoded, mode)
        }
        SequencerJournalEventType::ProposalAuthorized => {
            apply_journal_proposal_authorized(conn, event, &decoded, mode)
        }
        SequencerJournalEventType::TxSubmitted => {
            apply_journal_tx_submitted(conn, event, &decoded, mode)
        }
        SequencerJournalEventType::TxSeenMempoolAccepted => {
            apply_journal_tx_seen_mempool_accepted(conn, event, &decoded, mode)
        }
        SequencerJournalEventType::MempoolRetryAttempted => {
            apply_journal_mempool_retry_attempted(conn, event, &decoded, mode)
        }
        SequencerJournalEventType::TxConfirmed => {
            apply_journal_tx_confirmed(conn, event, &decoded, mode)
        }
    }
}

/// Projects `withdrawal_ordered` by materializing the sequencer row and nonce.
fn apply_journal_withdrawal_ordered(
    conn: &mut SqliteConnection,
    event: &SequencerJournalRecord,
    decoded: &DecodedJournalEvent,
    mode: SequencerJournalApplyMode,
) -> Result<(), BridgeError> {
    let withdrawal_nonce = decoded.withdrawal_nonce.ok_or_else(|| {
        BridgeError::Runtime("withdrawal_ordered event is missing withdrawal nonce".to_string())
    })?;
    let request_facts = decoded.request_facts.clone().ok_or_else(|| {
        BridgeError::Runtime("withdrawal_ordered event is missing request facts".to_string())
    })?;
    let turn_started_base_height = journal_turn_started_base_height(event);
    if let Some(existing) = fetch_sequenced_withdrawal(conn, &decoded.id.base_event_id)? {
        ensure_existing_journal_nonce(&existing, withdrawal_nonce)?;
        if existing.request_facts.as_ref() != Some(&request_facts) {
            return Err(BridgeError::Runtime(format!(
                "withdrawal_ordered journal event {} request facts do not match existing projection for {:?}",
                event.event_id, decoded.id
            )));
        }
        if turn_started_base_height.is_some()
            && existing.turn_started_base_height != turn_started_base_height
        {
            return Err(BridgeError::Runtime(format!(
                "withdrawal_ordered journal event {} turn_started_base_height does not match existing projection for {:?}",
                event.event_id, decoded.id
            )));
        }
        ensure_replay_not_past_target_state(
            mode,
            &decoded.id,
            &existing,
            WithdrawalState::Pending,
            None,
        )
        .map_err(|err| {
            BridgeError::Runtime(format!(
                "withdrawal_ordered journal event {} cannot replay over existing projection: {err}",
                event.event_id
            ))
        })?;
        return Ok(());
    }
    ensure_new_request_follows_canonical_order(conn, &decoded.id, &request_facts)?;
    create_ordered_sequencer_row(
        conn, &decoded.id, withdrawal_nonce, request_facts, turn_started_base_height,
        decoded.created_at,
    )
}

/// Projects `proposal_canonicalized` and restores selected-input reservations.
fn apply_journal_proposal_canonicalized(
    conn: &mut SqliteConnection,
    event: &SequencerJournalRecord,
    decoded: &DecodedJournalEvent,
    mode: SequencerJournalApplyMode,
) -> Result<(), BridgeError> {
    let withdrawal_nonce = required_journal_nonce(event, decoded)?;
    let proposal = require_journal_proposal(event)?;
    let existing =
        fetch_sequenced_withdrawal(conn, &decoded.id.base_event_id)?.ok_or_else(|| {
            BridgeError::Runtime(format!(
            "missing sequenced withdrawal row for {:?} during proposal_canonicalized projection",
            decoded.id
        ))
        })?;
    ensure_existing_journal_nonce(&existing, withdrawal_nonce)?;
    ensure_replay_not_past_target_state(
        mode,
        &decoded.id,
        &existing,
        WithdrawalState::PeerCanonical,
        Some(&proposal.proposal_hash),
    )?;
    let commit_certificate = decode_optional_journal_hex(
        "proposal.commit_certificate",
        proposal.commit_certificate.as_deref(),
    )?;
    let canonical_amount = proposal.amount.ok_or_else(|| {
        BridgeError::Runtime(format!(
            "sequencer journal event {:?} is missing proposal.amount",
            event.event_type
        ))
    })?;
    let canonical_base_batch_end = require_journal_base_batch_end(event)?;
    let canonical_transaction_jam = require_journal_transaction_jam(event, proposal)?;
    let canonical_selected_inputs = decode_journal_inputs(proposal)?;
    let canonical_snapshot = require_journal_snapshot(event)?;
    upsert_sequenced_withdrawal(
        conn,
        SequencerWithdrawalUpdate {
            id: decoded.id.clone(),
            withdrawal_nonce,
            current_epoch: decoded.epoch,
            proposal_hash: Some(proposal.proposal_hash.clone()),
            request_facts: existing.request_facts,
            canonical_amount: Some(canonical_amount),
            canonical_base_batch_end: Some(canonical_base_batch_end),
            canonical_transaction_jam: Some(canonical_transaction_jam),
            canonical_selected_inputs: Some(canonical_selected_inputs.clone()),
            canonical_snapshot: Some(canonical_snapshot),
            peer_commit_certificate: commit_certificate,
            authorized_transaction_name: None,
            authorized_transaction_jam: None,
            authorized_raw_tx: None,
            handoff_index: existing.handoff_index,
            turn_started_base_height: journal_turn_started_base_height(event),
            submit_attempt_count: 0,
            last_submit_attempt_base_height: None,
            last_submit_error: None,
            state: WithdrawalState::PeerCanonical,
            created_at: existing.created_at,
            updated_at: decoded.created_at,
        },
    )?;
    insert_reserved_inputs_for_journal_names(
        conn, &decoded.id, decoded.epoch, &canonical_selected_inputs, decoded.created_at, mode,
    )
}

/// Projects `proposal_authorized` and restores the fully authorized tx artifacts.
fn apply_journal_proposal_authorized(
    conn: &mut SqliteConnection,
    event: &SequencerJournalRecord,
    decoded: &DecodedJournalEvent,
    mode: SequencerJournalApplyMode,
) -> Result<(), BridgeError> {
    let withdrawal_nonce = required_journal_nonce(event, decoded)?;
    let proposal = require_journal_proposal(event)?;
    let authorized_transaction = require_journal_authorized_transaction(event)?;
    let existing =
        fetch_sequenced_withdrawal(conn, &decoded.id.base_event_id)?.ok_or_else(|| {
            BridgeError::Runtime(format!(
                "missing sequenced withdrawal row for {:?} during proposal_authorized projection",
                decoded.id
            ))
        })?;
    ensure_existing_journal_nonce(&existing, withdrawal_nonce)?;
    ensure_replay_not_past_target_state(
        mode,
        &decoded.id,
        &existing,
        WithdrawalState::Authorized,
        Some(&proposal.proposal_hash),
    )?;
    upsert_sequenced_withdrawal(
        conn,
        SequencerWithdrawalUpdate {
            id: decoded.id.clone(),
            withdrawal_nonce,
            current_epoch: decoded.epoch,
            peer_commit_certificate: existing.peer_commit_certificate,
            proposal_hash: Some(proposal.proposal_hash.clone()),
            request_facts: existing.request_facts,
            canonical_amount: existing.canonical_amount.or(proposal.amount),
            canonical_base_batch_end: existing
                .canonical_base_batch_end
                .or(event.base.as_ref().and_then(|base| base.base_batch_end)),
            canonical_transaction_jam: existing.canonical_transaction_jam.or(
                decode_optional_journal_hex(
                    "proposal.transaction_jam",
                    proposal.transaction_jam.as_deref(),
                )?,
            ),
            canonical_selected_inputs: existing.canonical_selected_inputs.or_else(|| {
                let inputs = decode_journal_inputs(proposal).ok()?;
                (!inputs.is_empty()).then_some(inputs)
            }),
            canonical_snapshot: existing
                .canonical_snapshot
                .or_else(|| require_journal_snapshot(event).ok()),
            authorized_transaction_name: Some(authorized_transaction.submitted_raw_tx_id),
            authorized_transaction_jam: Some(authorized_transaction.transaction_jam),
            authorized_raw_tx: Some(authorized_transaction.raw_tx_bytes),
            handoff_index: existing.handoff_index,
            turn_started_base_height: journal_turn_started_base_height(event)
                .or(existing.turn_started_base_height),
            submit_attempt_count: existing.submit_attempt_count,
            last_submit_attempt_base_height: existing.last_submit_attempt_base_height,
            last_submit_error: existing.last_submit_error,
            state: WithdrawalState::Authorized,
            created_at: decoded.created_at,
            updated_at: decoded.created_at,
        },
    )
}

/// Projects `tx_submitted` without advancing past `Authorized`.
fn apply_journal_tx_submitted(
    conn: &mut SqliteConnection,
    event: &SequencerJournalRecord,
    decoded: &DecodedJournalEvent,
    mode: SequencerJournalApplyMode,
) -> Result<(), BridgeError> {
    let proposal_hash = require_journal_proposal(event)?.proposal_hash.clone();
    let existing =
        fetch_sequenced_withdrawal(conn, &decoded.id.base_event_id)?.ok_or_else(|| {
            BridgeError::Runtime(format!(
                "missing sequenced withdrawal row for {:?} during tx_submitted projection",
                decoded.id
            ))
        })?;
    ensure_replay_not_past_target_state(
        mode,
        &decoded.id,
        &existing,
        WithdrawalState::Authorized,
        Some(&proposal_hash),
    )?;
    let submission = require_journal_submission(event)?;
    update_sequencer_submission_state_tx(
        conn,
        &existing,
        SequencerSubmissionStateUpdate {
            next_state: WithdrawalState::Authorized,
            authorized_transaction: journal_authorized_transaction(event)?,
            submit_attempt_count: submission
                .submit_attempt_count
                .unwrap_or(existing.submit_attempt_count),
            last_submit_attempt_base_height: journal_last_submit_attempt_base_height(event)?,
            last_submit_error: submission.last_submit_error.clone(),
            updated_at: decoded.created_at,
            action: "journal tx_submitted projection",
        },
    )
}

/// Projects a mempool-accepted observation and advances the row to `MempoolAccepted`.
fn apply_journal_tx_seen_mempool_accepted(
    conn: &mut SqliteConnection,
    event: &SequencerJournalRecord,
    decoded: &DecodedJournalEvent,
    mode: SequencerJournalApplyMode,
) -> Result<(), BridgeError> {
    let proposal_hash = require_journal_proposal(event)?.proposal_hash.clone();
    let existing =
        fetch_sequenced_withdrawal(conn, &decoded.id.base_event_id)?.ok_or_else(|| {
            BridgeError::Runtime(format!(
            "missing sequenced withdrawal row for {:?} during tx_seen_mempool_accepted projection",
            decoded.id
        ))
        })?;
    ensure_replay_not_past_target_state(
        mode,
        &decoded.id,
        &existing,
        WithdrawalState::MempoolAccepted,
        Some(&proposal_hash),
    )?;
    let submission = event.submission.as_ref();
    update_sequencer_submission_state_tx(
        conn,
        &existing,
        SequencerSubmissionStateUpdate {
            next_state: WithdrawalState::MempoolAccepted,
            authorized_transaction: journal_authorized_transaction(event)?,
            submit_attempt_count: submission
                .and_then(|submission| submission.submit_attempt_count)
                .unwrap_or(existing.submit_attempt_count),
            last_submit_attempt_base_height: event
                .base
                .as_ref()
                .and_then(|base| base.last_submit_attempt_base_height)
                .or(existing.last_submit_attempt_base_height)
                .unwrap_or_default(),
            last_submit_error: submission
                .and_then(|submission| submission.last_submit_error.clone()),
            updated_at: decoded.created_at,
            action: "journal tx_seen_mempool_accepted projection",
        },
    )
}

/// Projects orphan-retry metadata while leaving the row in `MempoolAccepted`.
fn apply_journal_mempool_retry_attempted(
    conn: &mut SqliteConnection,
    event: &SequencerJournalRecord,
    decoded: &DecodedJournalEvent,
    mode: SequencerJournalApplyMode,
) -> Result<(), BridgeError> {
    let proposal_hash = require_journal_proposal(event)?.proposal_hash.clone();
    let existing =
        fetch_sequenced_withdrawal(conn, &decoded.id.base_event_id)?.ok_or_else(|| {
            BridgeError::Runtime(format!(
            "missing sequenced withdrawal row for {:?} during mempool_retry_attempted projection",
            decoded.id
        ))
        })?;
    ensure_replay_not_past_target_state(
        mode,
        &decoded.id,
        &existing,
        WithdrawalState::MempoolAccepted,
        Some(&proposal_hash),
    )?;
    let submission = require_journal_submission(event)?;
    update_sequencer_submission_state_tx(
        conn,
        &existing,
        SequencerSubmissionStateUpdate {
            next_state: WithdrawalState::MempoolAccepted,
            authorized_transaction: None,
            submit_attempt_count: submission
                .submit_attempt_count
                .unwrap_or_else(|| existing.submit_attempt_count.saturating_add(1)),
            last_submit_attempt_base_height: journal_last_submit_attempt_base_height(event)?,
            last_submit_error: submission.last_submit_error.clone(),
            updated_at: decoded.created_at,
            action: "journal mempool_retry_attempted projection",
        },
    )
}

/// Projects `tx_confirmed` and clears live reserved inputs for the withdrawal.
fn apply_journal_tx_confirmed(
    conn: &mut SqliteConnection,
    event: &SequencerJournalRecord,
    decoded: &DecodedJournalEvent,
    mode: SequencerJournalApplyMode,
) -> Result<(), BridgeError> {
    let withdrawal_nonce = required_journal_nonce(event, decoded)?;
    let proposal = require_journal_proposal(event)?;
    let confirmation = require_journal_confirmation(event)?;
    let existing =
        fetch_sequenced_withdrawal(conn, &decoded.id.base_event_id)?.ok_or_else(|| {
            BridgeError::Runtime(format!(
                "missing sequenced withdrawal row for {:?} during tx_confirmed projection",
                decoded.id
            ))
        })?;
    ensure_existing_journal_nonce(&existing, withdrawal_nonce)?;
    ensure_replay_not_past_target_state(
        mode,
        &decoded.id,
        &existing,
        WithdrawalState::Confirmed,
        Some(&proposal.proposal_hash),
    )?;
    // `sequencer_withdrawals` does not currently persist confirmation block
    // metadata; validate it here so malformed replay records still fail closed.
    let _confirmed_height = confirmation.confirmed_height.ok_or_else(|| {
        BridgeError::Runtime("tx_confirmed journal event is missing confirmed_height".to_string())
    })?;
    let _confirmed_block_id = confirmation
        .confirmed_block_id
        .as_deref()
        .ok_or_else(|| {
            BridgeError::Runtime(
                "tx_confirmed journal event is missing confirmed_block_id".to_string(),
            )
        })
        .and_then(|value| decode_journal_tip5_hex("confirmation.confirmed_block_id", value))?;
    let authorized_transaction = journal_authorized_transaction(event)?;
    upsert_sequenced_withdrawal(
        conn,
        SequencerWithdrawalUpdate {
            id: decoded.id.clone(),
            withdrawal_nonce,
            current_epoch: decoded.epoch,
            peer_commit_certificate: existing.peer_commit_certificate,
            proposal_hash: Some(proposal.proposal_hash.clone()),
            request_facts: existing.request_facts,
            canonical_amount: existing.canonical_amount.or(proposal.amount),
            canonical_base_batch_end: existing
                .canonical_base_batch_end
                .or(event.base.as_ref().and_then(|base| base.base_batch_end)),
            canonical_transaction_jam: existing.canonical_transaction_jam.or(
                decode_optional_journal_hex(
                    "proposal.transaction_jam",
                    proposal.transaction_jam.as_deref(),
                )?,
            ),
            canonical_selected_inputs: existing.canonical_selected_inputs.or_else(|| {
                let inputs = decode_journal_inputs(proposal).ok()?;
                (!inputs.is_empty()).then_some(inputs)
            }),
            canonical_snapshot: existing
                .canonical_snapshot
                .or_else(|| require_journal_snapshot(event).ok()),
            authorized_transaction_name: authorized_transaction
                .as_ref()
                .map(|transaction| transaction.submitted_raw_tx_id.clone())
                .or_else(|| journal_submitted_raw_tx_id(event))
                .or(existing.authorized_transaction_name),
            authorized_transaction_jam: authorized_transaction
                .as_ref()
                .map(|transaction| transaction.transaction_jam.clone())
                .or(existing.authorized_transaction_jam),
            authorized_raw_tx: authorized_transaction
                .as_ref()
                .map(|transaction| transaction.raw_tx_bytes.clone())
                .or(existing.authorized_raw_tx),
            handoff_index: existing.handoff_index,
            turn_started_base_height: existing.turn_started_base_height,
            submit_attempt_count: event
                .submission
                .as_ref()
                .and_then(|submission| submission.submit_attempt_count)
                .unwrap_or(existing.submit_attempt_count),
            last_submit_attempt_base_height: event
                .base
                .as_ref()
                .and_then(|base| base.last_submit_attempt_base_height)
                .or(existing.last_submit_attempt_base_height),
            last_submit_error: event
                .submission
                .as_ref()
                .and_then(|submission| submission.last_submit_error.clone())
                .or(existing.last_submit_error),
            state: WithdrawalState::Confirmed,
            created_at: decoded.created_at,
            updated_at: decoded.created_at,
        },
    )?;
    clear_reserved_inputs_for_withdrawal(conn, &decoded.id)
}

impl NewWithdrawalSubmissionEventRow {
    /// Builds the append-only event row persisted for a proposal lifecycle
    /// transition.
    fn from_proposal(
        proposal: &WithdrawalProposalData,
        event_type: WithdrawalSubmissionEventType,
        created_at: i64,
        signer_node_id: Option<u64>,
        commit_certificate: Option<&WithdrawalCommitCertificate>,
        confirmed: Option<(u64, Tip5Hash)>,
    ) -> Result<Self, BridgeError> {
        let epoch = i64::try_from(proposal.epoch)
            .map_err(|err| BridgeError::ValueConversion(format!("epoch too large: {err}")))?;
        let snapshot_height = Some(i64::try_from(proposal.snapshot.height).map_err(|err| {
            BridgeError::ValueConversion(format!("snapshot height too large: {err}"))
        })?);
        let confirmed_height = confirmed
            .as_ref()
            .map(|(height, _)| {
                i64::try_from(*height).map_err(|err| {
                    BridgeError::ValueConversion(format!("confirmed height too large: {err}"))
                })
            })
            .transpose()?;
        Ok(Self {
            created_at,
            withdrawal_id_as_of: tip5_to_bytes(&proposal.id.as_of),
            withdrawal_id_base_event_id: proposal.id.base_event_id.0.clone(),
            epoch,
            proposal_hash: proposal.proposal_hash()?,
            transaction_name: withdrawal_raw_tx::submitted_raw_tx_id_base58(&proposal.transaction)?,
            event_type: event_type.as_str().to_string(),
            signer_node_id: signer_node_id
                .map(|node_id| {
                    i64::try_from(node_id).map_err(|err| {
                        BridgeError::ValueConversion(format!(
                            "signer_node_id too large for event row: {err}"
                        ))
                    })
                })
                .transpose()?,
            commit_certificate: commit_certificate
                .map(encode_commit_certificate)
                .transpose()?,
            transaction_jam: Some(jam_transaction(&proposal.transaction)?),
            snapshot_height,
            snapshot_block_id: Some(tip5_to_bytes(&proposal.snapshot.block_id)),
            confirmed_height,
            confirmed_block_id: confirmed.map(|(_, block_id)| tip5_to_bytes(&block_id)),
        })
    }

    fn from_existing_sequencer_row(
        existing: &SequencedWithdrawalView,
        event_type: WithdrawalSubmissionEventType,
        created_at: i64,
    ) -> Result<Self, BridgeError> {
        let epoch = i64::try_from(existing.current_epoch)
            .map_err(|err| BridgeError::ValueConversion(format!("epoch too large: {err}")))?;
        let proposal_hash = existing
            .proposal_hash
            .clone()
            .or(existing.proposal_hash.clone())
            .ok_or_else(|| {
                BridgeError::Runtime(format!(
                    "missing canonical proposal hash for withdrawal {:?}",
                    existing.id
                ))
            })?;
        Ok(Self {
            created_at,
            withdrawal_id_as_of: tip5_to_bytes(&existing.id.as_of),
            withdrawal_id_base_event_id: existing.id.base_event_id.0.clone(),
            epoch,
            proposal_hash,
            transaction_name: existing
                .authorized_transaction_name
                .clone()
                .unwrap_or_default(),
            event_type: event_type.as_str().to_string(),
            signer_node_id: None,
            commit_certificate: None,
            transaction_jam: None,
            snapshot_height: None,
            snapshot_block_id: None,
            confirmed_height: None,
            confirmed_block_id: None,
        })
    }

    fn from_withdrawal_id(
        id: &WithdrawalId,
        epoch: u64,
        event_type: WithdrawalSubmissionEventType,
        created_at: i64,
    ) -> Result<Self, BridgeError> {
        let epoch = i64::try_from(epoch)
            .map_err(|err| BridgeError::ValueConversion(format!("epoch too large: {err}")))?;
        Ok(Self {
            created_at,
            withdrawal_id_as_of: tip5_to_bytes(&id.as_of),
            withdrawal_id_base_event_id: id.base_event_id.0.clone(),
            epoch,
            proposal_hash: String::new(),
            transaction_name: String::new(),
            event_type: event_type.as_str().to_string(),
            signer_node_id: None,
            commit_certificate: None,
            transaction_jam: None,
            snapshot_height: None,
            snapshot_block_id: None,
            confirmed_height: None,
            confirmed_block_id: None,
        })
    }
}

fn insert_debug_event(
    conn: &mut SqliteConnection,
    row: &NewWithdrawalSubmissionEventRow,
) -> Result<i64, BridgeError> {
    diesel::insert_into(withdrawal_submission_events::table)
        .values(row)
        .execute(conn)
        .map_err(|err| BridgeError::Runtime(format!("withdrawal event insert failed: {err}")))?;
    last_insert_rowid(conn)
}

/// Loads the persisted local cursor row without applying a default.
///
/// Startup recovery needs to distinguish "no cursor exists yet" from "there is
/// an explicit genesis cursor". That distinction is security-relevant: either
/// form is fine only when the replay-owned SQLite projection is empty.
fn load_journal_cursor_optional(
    conn: &mut SqliteConnection,
    journal_id: &str,
) -> Result<Option<SequencerJournalCursorRow>, BridgeError> {
    use crate::withdrawal::sequencer::schema::sequencer_journal_cursor::dsl as cursor;

    sequencer_journal_cursor::table
        .filter(cursor::journal_id.eq(journal_id))
        .first::<SequencerJournalCursorRow>(conn)
        .optional()
        .map_err(|err| BridgeError::Runtime(format!("sequencer journal cursor load failed: {err}")))
}

/// Loads the local cursor for this journal, defaulting to genesis for first boot.
fn load_journal_cursor(
    conn: &mut SqliteConnection,
    journal_id: &str,
) -> Result<SequencerJournalCursorRow, BridgeError> {
    Ok(
        load_journal_cursor_optional(conn, journal_id)?.unwrap_or_else(|| {
            SequencerJournalCursorRow {
                _journal_id: journal_id.to_string(),
                last_sequence: 0,
                last_event_id: GENESIS_EVENT_ID.to_string(),
                _updated_at: 0,
            }
        }),
    )
}

/// Returns whether the local replay-owned projection has durable sequencer rows.
///
/// Schema metadata and the cursor table do not count. Local debug history does
/// not count either: replay does not rebuild `withdrawal_submission_events`, so
/// debug rows alone are not proof that the sequencer projection has advanced.
fn sequencer_projection_has_rows(conn: &mut SqliteConnection) -> Result<bool, BridgeError> {
    let withdrawal_count = sequencer_withdrawals::table
        .count()
        .get_result::<i64>(conn)
        .map_err(|err| {
            BridgeError::Runtime(format!(
                "sequencer withdrawal projection count failed: {err}"
            ))
        })?;
    if withdrawal_count > 0 {
        return Ok(true);
    }
    let reserved_count = withdrawal_reserved_inputs::table
        .count()
        .get_result::<i64>(conn)
        .map_err(|err| {
            BridgeError::Runtime(format!("reserved input projection count failed: {err}"))
        })?;
    Ok(reserved_count > 0)
}

/// Classifies local startup state before remote replay begins.
///
/// The fail-closed cases here prevent silently treating an already-mutated local
/// projection as if it were a fresh empty database. If the cursor is missing or
/// genesis, the projection must be empty because replay will start from remote
/// sequence 1. If the cursor is non-genesis, the projection must be non-empty
/// because the cursor claims prior events were already applied.
fn check_journal_recovery_cursor(
    journal_id: &str,
    cursor_row: Option<SequencerJournalCursorRow>,
    projection_has_rows: bool,
) -> Result<SequencerJournalCursor, BridgeError> {
    let explicit_cursor = cursor_row.is_some();
    let cursor = match cursor_row {
        Some(row) => SequencerJournalCursor::try_from(row)?,
        None => SequencerJournalCursor::genesis(journal_id),
    };
    if cursor.journal_id != journal_id {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal cursor journal mismatch: local {}, configured {}",
            cursor.journal_id, journal_id
        )));
    }
    if cursor.last_sequence == 0 {
        if cursor.last_event_id != GENESIS_EVENT_ID {
            return Err(BridgeError::Runtime(format!(
                "sequencer journal genesis cursor has invalid event id: expected {}, found {}",
                GENESIS_EVENT_ID, cursor.last_event_id
            )));
        }
        if projection_has_rows {
            let cursor_source = if explicit_cursor {
                "explicit genesis cursor"
            } else {
                "missing cursor treated as genesis"
            };
            return Err(BridgeError::Runtime(format!(
                "sequencer journal recovery refused {cursor_source} with non-empty SQLite projection"
            )));
        }
        return Ok(cursor);
    }
    if !projection_has_rows {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal recovery refused non-genesis cursor sequence {} with empty SQLite projection",
            cursor.last_sequence
        )));
    }
    if cursor.last_event_id.trim().is_empty() {
        return Err(BridgeError::Runtime(
            "sequencer journal non-genesis cursor has empty event id".to_string(),
        ));
    }
    Ok(cursor)
}

/// Verifies that the local non-genesis cursor names the exact remote object.
///
/// A missing or mismatched cursor object means the local database may be ahead
/// of the remote journal, pointed at a different journal, or restored from an
/// incompatible backup. There is no safe automatic replay path from that state.
fn verify_journal_cursor_record(
    journal_id: &str,
    cursor: &SequencerJournalCursor,
    object_ref: &SequencerJournalObjectRef,
    record: &SequencerJournalRecord,
) -> Result<(), BridgeError> {
    if object_ref.sequence != cursor.last_sequence {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal cursor object {} sequence mismatch: cursor has {}, key has {}",
            object_ref.key, cursor.last_sequence, object_ref.sequence
        )));
    }
    verify_journal_object_payload(journal_id, object_ref, record)?;
    if record.sequence != cursor.last_sequence || record.event_id != cursor.last_event_id {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal cursor object {} payload does not match local cursor sequence/event",
            object_ref.key
        )));
    }
    Ok(())
}

/// Verifies that a replay candidate is the exact next object after the cursor.
///
/// The remote object store is append-only by convention, not a database
/// transaction log. These checks are the software contract that turns object
/// sequence keys plus `previous_event_id` into a strict hash-linked sequence.
fn verify_journal_successor(
    journal_id: &str,
    cursor: &SequencerJournalCursor,
    object_ref: &SequencerJournalObjectRef,
    record: &SequencerJournalRecord,
) -> Result<(), BridgeError> {
    let expected_sequence = cursor.last_sequence.checked_add(1).ok_or_else(|| {
        BridgeError::Runtime("sequencer journal cursor sequence overflow".to_string())
    })?;
    if object_ref.sequence != expected_sequence {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal successor sequence mismatch after cursor {}: expected {}, found {} at {}",
            cursor.last_sequence, expected_sequence, object_ref.sequence, object_ref.key
        )));
    }
    verify_journal_object_payload(journal_id, object_ref, record)?;
    if record.previous_event_id != cursor.last_event_id {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal previous_event_id mismatch at sequence {}: expected {}, found {}",
            record.sequence, cursor.last_event_id, record.previous_event_id
        )));
    }
    Ok(())
}

/// Verifies that an object key and decoded payload agree on journal identity.
fn verify_journal_object_payload(
    journal_id: &str,
    object_ref: &SequencerJournalObjectRef,
    record: &SequencerJournalRecord,
) -> Result<(), BridgeError> {
    if record.journal_id != journal_id {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal object {} belongs to journal {}, not {}",
            object_ref.key, record.journal_id, journal_id
        )));
    }
    if record.sequence != object_ref.sequence {
        return Err(BridgeError::Runtime(format!(
            "sequencer journal object {} sequence mismatch: key has {}, record has {}",
            object_ref.key, object_ref.sequence, record.sequence
        )));
    }
    crate::withdrawal::sequencer::journal::verify_journal_record_hashes(record)?;
    Ok(())
}

/// Tracks the highest chain heights mentioned by records replayed this startup.
///
/// This is deliberately a startup lower bound: the sequencer waits for its Base
/// height watcher to reach these replayed facts before serving RPC. Discovery of
/// unsequenced Base requests remains bridge/kernel projection work, and
/// Nockchain inclusion/retry catch-up is handled by the sequencer loops.
fn set_journal_recovery_bounds(
    record: &SequencerJournalRecord,
    max_base_height: &mut Option<u64>,
    max_nockchain_height: &mut Option<u64>,
) {
    if let Some(base) = &record.base {
        observe_height(max_base_height, base.base_batch_end);
        observe_height(max_base_height, base.turn_started_base_height);
        observe_height(max_base_height, base.last_submit_attempt_base_height);
    }
    if let Some(nockchain) = &record.nockchain {
        observe_height(max_nockchain_height, nockchain.snapshot_height);
        observe_height(
            max_nockchain_height, nockchain.safe_tip_height_observed_by_writer,
        );
    }
    if let Some(confirmation) = &record.confirmation {
        observe_height(max_nockchain_height, confirmation.included_height);
        observe_height(max_nockchain_height, confirmation.confirmed_height);
    }
}

fn observe_height(max_height: &mut Option<u64>, height: Option<u64>) {
    if let Some(height) = height {
        *max_height = Some(
            max_height
                .map(|current| current.max(height))
                .unwrap_or(height),
        );
    }
}

/// Persists the last journal record that was durably appended and locally projected.
fn upsert_journal_cursor(
    conn: &mut SqliteConnection,
    journal_id: &str,
    last_sequence: u64,
    last_event_id: &str,
    updated_at: i64,
) -> Result<(), BridgeError> {
    let last_sequence = i64::try_from(last_sequence).map_err(|err| {
        BridgeError::Runtime(format!(
            "sequencer journal sequence does not fit SQLite integer: {err}"
        ))
    })?;
    let row = NewSequencerJournalCursorRow {
        journal_id: journal_id.to_string(),
        last_sequence,
        last_event_id: last_event_id.to_string(),
        updated_at,
    };
    diesel::insert_into(sequencer_journal_cursor::table)
        .values(&row)
        .on_conflict(sequencer_journal_cursor::journal_id)
        .do_update()
        .set(&row)
        .execute(conn)
        .map_err(|err| {
            BridgeError::Runtime(format!("sequencer journal cursor update failed: {err}"))
        })?;
    Ok(())
}

/// Appends durable records in order, projects each one, then advances the local cursor.
///
/// Cursor advancement happens after projection so a crash cannot claim that a
/// record was applied locally before the SQLite mutation completed.
fn append_and_project_journal_records(
    conn: &mut SqliteConnection,
    journal: &SequencerJournalHandle,
    journal_records: &[SequencerJournalRecord],
) -> Result<(), BridgeError> {
    let Some(journal_id) = journal.journal_id() else {
        for record in journal_records {
            journal.append(record)?;
            apply_journal_event(conn, record, SequencerJournalApplyMode::Runtime)?;
        }
        return Ok(());
    };

    let mut cursor = load_journal_cursor(conn, &journal_id)?;
    let mut next_sequence = u64::try_from(cursor.last_sequence).map_err(|err| {
        BridgeError::Runtime(format!(
            "sequencer journal cursor sequence is negative or invalid: {err}"
        ))
    })?;
    for record in journal_records {
        next_sequence = next_sequence.saturating_add(1);
        let ordered = record.clone().into_ordered(
            journal_id.clone(),
            next_sequence,
            cursor.last_event_id.clone(),
        )?;
        journal.append(&ordered)?;
        apply_journal_event(conn, &ordered, SequencerJournalApplyMode::Runtime)?;
        upsert_journal_cursor(
            conn, &journal_id, ordered.sequence, &ordered.event_id, ordered.created_at_unix_ms,
        )?;
        cursor.last_sequence = i64::try_from(ordered.sequence).map_err(|err| {
            BridgeError::Runtime(format!(
                "sequencer journal sequence does not fit SQLite integer: {err}"
            ))
        })?;
        cursor.last_event_id = ordered.event_id;
    }
    Ok(())
}

/// Inserts local diagnostic submission rows and returns their SQLite ids.
fn insert_debug_rows(
    conn: &mut SqliteConnection,
    debug_rows: &[NewWithdrawalSubmissionEventRow],
) -> Result<Vec<i64>, BridgeError> {
    let mut event_ids = Vec::with_capacity(debug_rows.len());
    for row in debug_rows {
        event_ids.push(insert_debug_event(conn, row)?);
    }
    Ok(event_ids)
}

/// Applies a local-only sequencer mutation after its diagnostic rows are
/// recorded. Durable journal events use `apply_journal_event` instead.
fn apply_after_local_debug_mutation<T, E, F>(
    conn: &mut SqliteConnection,
    debug_rows: &[NewWithdrawalSubmissionEventRow],
    apply: F,
) -> Result<T, E>
where
    E: From<BridgeError>,
    F: FnOnce(&mut SqliteConnection, &[i64]) -> Result<T, E>,
{
    let event_ids = insert_debug_rows(conn, debug_rows).map_err(E::from)?;
    apply(conn, &event_ids)
}

/// Applies a typed sequencer mutation through the journal-first write contract.
///
/// Mutations with durable journal records append and project those records
/// before local debug rows are inserted. Local-only/debug mutations still write
/// their diagnostic rows before applying their SQLite changes.
fn apply_sequencer_mutation(
    conn: &mut SqliteConnection,
    journal: &SequencerJournalHandle,
    mutation: SequencerMutation,
) -> Result<(), BridgeError> {
    let journal_records = mutation.journal_records()?;
    let debug_rows = mutation.debug_rows()?;
    if journal_records.is_empty() {
        return apply_after_local_debug_mutation(conn, &debug_rows, |conn, _event_ids| {
            mutation.apply(conn)
        });
    }
    append_and_project_journal_records(conn, journal, &journal_records)?;
    insert_debug_rows(conn, &debug_rows)?;
    Ok(())
}

fn require_withdrawal_nonce(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
) -> Result<u64, WithdrawalSequencerStoreError> {
    fetch_withdrawal_nonce(conn, &id.base_event_id)?.ok_or_else(|| {
        WithdrawalSequencerStoreError::Store(format!(
            "missing sequencer withdrawal nonce for {:?}",
            id
        ))
    })
}

fn record_peer_canonical_proposal_tx(
    conn: &mut SqliteConnection,
    journal: &SequencerJournalHandle,
    proposal: &WithdrawalProposalData,
    commit_certificate: Option<&WithdrawalCommitCertificate>,
    turn_started_base_height: u64,
    created_at: i64,
) -> Result<(), WithdrawalSequencerStoreError> {
    validate_canonical_proposal_tx_inputs(proposal)?;
    let proposal_hash = proposal.proposal_hash()?;
    let withdrawal_nonce = require_withdrawal_nonce(conn, &proposal.id)?;
    let sequenced_row =
        fetch_sequenced_withdrawal(conn, &proposal.id.base_event_id)?.ok_or_else(|| {
            WithdrawalSequencerStoreError::Store(format!(
                "missing sequencer withdrawal row for {:?} while recording canonical proposal",
                proposal.id
            ))
        })?;
    ensure_withdrawal_is_current_frontier(
        conn, &proposal.id, withdrawal_nonce, "record canonical proposal",
    )?;
    let proposal_already_pinned = sequenced_row.current_epoch == proposal.epoch
        && sequenced_row.proposal_hash.as_deref() == Some(proposal_hash.as_str());
    if proposal_already_pinned {
        let next_commit_certificate = commit_certificate
            .map(encode_commit_certificate)
            .transpose()?
            .or(sequenced_row.peer_commit_certificate.clone());
        let next_turn_started_base_height = sequenced_row
            .turn_started_base_height
            .or(Some(turn_started_base_height));
        if next_commit_certificate != sequenced_row.peer_commit_certificate
            || next_turn_started_base_height != sequenced_row.turn_started_base_height
        {
            upsert_sequenced_withdrawal(
                conn,
                SequencerWithdrawalUpdate {
                    id: proposal.id.clone(),
                    withdrawal_nonce,
                    current_epoch: sequenced_row.current_epoch,
                    proposal_hash: Some(proposal_hash),
                    request_facts: sequenced_row
                        .request_facts
                        .clone()
                        .or_else(|| Some(SequencerWithdrawalRequestFacts::from_proposal(proposal))),
                    canonical_amount: sequenced_row
                        .canonical_amount
                        .or_else(|| canonical_amount_from_proposal(proposal)),
                    canonical_base_batch_end: sequenced_row
                        .canonical_base_batch_end
                        .or_else(|| canonical_base_batch_end_from_proposal(proposal)),
                    canonical_transaction_jam: sequenced_row
                        .canonical_transaction_jam
                        .clone()
                        .or(canonical_transaction_jam_from_proposal(proposal)?),
                    canonical_selected_inputs: sequenced_row
                        .canonical_selected_inputs
                        .clone()
                        .or_else(|| canonical_selected_inputs_from_proposal(proposal)),
                    canonical_snapshot: sequenced_row
                        .canonical_snapshot
                        .clone()
                        .or_else(|| canonical_snapshot_from_proposal(proposal)),
                    peer_commit_certificate: next_commit_certificate,
                    authorized_transaction_name: sequenced_row.authorized_transaction_name.clone(),
                    authorized_transaction_jam: sequenced_row.authorized_transaction_jam.clone(),
                    authorized_raw_tx: sequenced_row.authorized_raw_tx.clone(),
                    handoff_index: sequenced_row.handoff_index,
                    turn_started_base_height: next_turn_started_base_height,
                    submit_attempt_count: sequenced_row.submit_attempt_count,
                    last_submit_attempt_base_height: sequenced_row.last_submit_attempt_base_height,
                    last_submit_error: sequenced_row.last_submit_error.clone(),
                    state: sequenced_row.state,
                    created_at: sequenced_row.created_at,
                    updated_at: created_at,
                },
            )?;
        }
        ensure_reserved_inputs_match_pinned_proposal(conn, proposal, created_at)?;
        return Ok(());
    }
    if let Some(existing_hash) = sequenced_row.proposal_hash.as_deref() {
        return Err(WithdrawalSequencerStoreError::Store(format!(
            "withdrawal {:?} already has canonical proposal hash {} at epoch {}; refusing conflicting proposal hash {} at epoch {}",
            proposal.id, existing_hash, sequenced_row.current_epoch, proposal_hash, proposal.epoch
        )));
    }
    if sequenced_row.state != WithdrawalState::Pending {
        return Err(WithdrawalSequencerStoreError::Store(format!(
            "withdrawal {:?} is already in sequencer state {}; refusing new canonical proposal hash {} at epoch {}",
            proposal.id,
            sequenced_row.state.as_str(),
            proposal_hash,
            proposal.epoch
        )));
    }

    ensure_canonical_proposal_inputs_are_unreserved(conn, proposal)?;

    apply_sequencer_mutation(
        conn,
        journal,
        SequencerMutation::ProposalCanonicalized {
            proposal: proposal.clone(),
            existing: sequenced_row,
            withdrawal_nonce,
            proposal_hash,
            commit_certificate: commit_certificate.cloned(),
            turn_started_base_height,
            created_at,
        },
    )?;
    Ok(())
}

fn record_proposer_turn_expired_tx(
    conn: &mut SqliteConnection,
    journal: &SequencerJournalHandle,
    proposal: &WithdrawalProposalData,
    next_handoff_index: u64,
    next_turn_started_base_height: u64,
    created_at: i64,
) -> Result<bool, WithdrawalSequencerStoreError> {
    let proposal_hash = proposal.proposal_hash()?;
    let Some(existing) = fetch_sequenced_withdrawal(conn, &proposal.id.base_event_id)? else {
        return Ok(false);
    };
    if existing.current_epoch != proposal.epoch {
        return Ok(false);
    }
    if !matches!(
        existing.state,
        WithdrawalState::PeerCanonical | WithdrawalState::Authorized
    ) {
        return Ok(false);
    }
    if existing.proposal_hash.as_deref() != Some(proposal_hash.as_str()) {
        return Err(WithdrawalSequencerStoreError::Store(format!(
            "cannot advance proposer turn for withdrawal {:?} epoch {} with mismatched canonical hash",
            proposal.id, proposal.epoch
        )));
    }
    if next_handoff_index <= existing.handoff_index {
        return Ok(false);
    }

    apply_sequencer_mutation(
        conn,
        journal,
        SequencerMutation::ProposerTurnExpiredForProposal {
            proposal: proposal.clone(),
            existing,
            next_handoff_index,
            next_turn_started_base_height,
            created_at,
        },
    )?;
    Ok(true)
}

fn record_proposer_turn_expired_tx_for_id(
    conn: &mut SqliteConnection,
    journal: &SequencerJournalHandle,
    id: &WithdrawalId,
    epoch: u64,
    next_handoff_index: u64,
    next_turn_started_base_height: u64,
    created_at: i64,
) -> Result<bool, WithdrawalSequencerStoreError> {
    let Some(existing) = fetch_sequenced_withdrawal(conn, &id.base_event_id)? else {
        return Ok(false);
    };
    if existing.current_epoch != epoch {
        return Ok(false);
    }
    if !matches!(
        existing.state,
        WithdrawalState::PeerCanonical | WithdrawalState::Authorized
    ) {
        return Ok(false);
    }
    if next_handoff_index <= existing.handoff_index {
        return Ok(false);
    }

    apply_sequencer_mutation(
        conn,
        journal,
        SequencerMutation::ProposerTurnExpiredForRow {
            existing,
            next_handoff_index,
            next_turn_started_base_height,
            created_at,
        },
    )?;
    Ok(true)
}

fn record_precanonical_handoff_tx_for_id(
    conn: &mut SqliteConnection,
    journal: &SequencerJournalHandle,
    id: &WithdrawalId,
    epoch: u64,
    next_handoff_index: u64,
    turn_started_base_height: u64,
    created_at: i64,
) -> Result<(), WithdrawalSequencerStoreError> {
    let Some(existing) = fetch_sequenced_withdrawal(conn, &id.base_event_id)? else {
        return Ok(());
    };
    if existing.current_epoch != epoch {
        return Ok(());
    }
    if existing.state != WithdrawalState::Pending {
        return Ok(());
    }
    if next_handoff_index <= existing.handoff_index {
        return Ok(());
    }

    apply_sequencer_mutation(
        conn,
        journal,
        SequencerMutation::PrecanonicalHandoff {
            existing,
            next_handoff_index,
            turn_started_base_height,
            created_at,
        },
    )?;
    Ok(())
}

fn record_proposal_authorized_tx(
    conn: &mut SqliteConnection,
    journal: &SequencerJournalHandle,
    proposal: &WithdrawalProposalData,
    turn_started_base_height: Option<u64>,
    created_at: i64,
) -> Result<(), WithdrawalSequencerStoreError> {
    let proposal_hash = proposal.proposal_hash()?;
    let authorized_transaction = stored_authorized_transaction(&proposal.transaction)?;
    let Some(existing) = fetch_sequenced_withdrawal(conn, &proposal.id.base_event_id)? else {
        return Err(WithdrawalSequencerStoreError::NotPeerCanonical {
            id: Box::new(proposal.id.clone()),
            proposal_hash: proposal_hash.into_boxed_str(),
        });
    };
    if existing.proposal_hash.as_deref() != Some(proposal_hash.as_str()) {
        return Err(WithdrawalSequencerStoreError::NotPeerCanonical {
            id: Box::new(proposal.id.clone()),
            proposal_hash: proposal_hash.into_boxed_str(),
        });
    }

    if let Some(active) = fetch_active_in_flight(conn)? {
        if !same_base_event_id(&active.id, &proposal.id) {
            return Err(WithdrawalSequencerStoreError::SingleFlightViolation {
                active: Box::new(active.id),
                active_state: active.state,
                requested: Box::new(proposal.id.clone()),
            });
        }
    }

    apply_sequencer_mutation(
        conn,
        journal,
        SequencerMutation::ProposalAuthorized {
            proposal: proposal.clone(),
            existing,
            proposal_hash,
            authorized_transaction,
            turn_started_base_height,
            created_at,
        },
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn record_submit_outcome_tx(
    conn: &mut SqliteConnection,
    journal: &SequencerJournalHandle,
    proposal: &WithdrawalProposalData,
    final_state: WithdrawalState,
    submit_attempt_count: u64,
    last_submit_attempt_base_height: u64,
    last_submit_error: Option<String>,
    created_at: i64,
) -> Result<(), WithdrawalSequencerStoreError> {
    let proposal_hash = proposal.proposal_hash()?;
    let authorized_transaction = stored_authorized_transaction(&proposal.transaction)?;
    require_authorized(conn, &proposal.id, &proposal_hash)?;
    let Some(existing) = fetch_sequenced_withdrawal(conn, &proposal.id.base_event_id)? else {
        return Ok(());
    };
    if existing.state == WithdrawalState::Confirmed {
        return Ok(());
    }

    apply_sequencer_mutation(
        conn,
        journal,
        SequencerMutation::SubmitOutcome {
            proposal: proposal.clone(),
            existing,
            final_state,
            authorized_transaction,
            submit_attempt_count,
            last_submit_attempt_base_height,
            last_submit_error,
            created_at,
        },
    )?;
    Ok(())
}

fn record_authorized_mempool_accepted_tx(
    conn: &mut SqliteConnection,
    journal: &SequencerJournalHandle,
    proposal: &WithdrawalProposalData,
    created_at: i64,
) -> Result<(), WithdrawalSequencerStoreError> {
    let proposal_hash = proposal.proposal_hash()?;
    require_authorized(conn, &proposal.id, &proposal_hash)?;
    let Some(existing) = fetch_sequenced_withdrawal(conn, &proposal.id.base_event_id)? else {
        return Ok(());
    };
    if existing.state == WithdrawalState::Confirmed
        || existing.state == WithdrawalState::MempoolAccepted
    {
        return Ok(());
    }
    if existing.state != WithdrawalState::Authorized {
        return Err(WithdrawalSequencerStoreError::Store(format!(
            "withdrawal {:?} epoch {} is in state {} instead of authorized during mempool-accepted observation",
            proposal.id,
            proposal.epoch,
            existing.state.as_str()
        )));
    }

    apply_sequencer_mutation(
        conn,
        journal,
        SequencerMutation::AuthorizedMempoolAccepted {
            proposal: proposal.clone(),
            existing,
            created_at,
        },
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn record_mempool_retry_attempt_tx(
    conn: &mut SqliteConnection,
    journal: &SequencerJournalHandle,
    id: &WithdrawalId,
    expected_epoch: u64,
    expected_proposal_hash: &str,
    attempt_base_height: u64,
    error: Option<String>,
    updated_at: i64,
) -> Result<(), WithdrawalSequencerStoreError> {
    let existing = fetch_sequenced_withdrawal(conn, &id.base_event_id)?.ok_or_else(|| {
        WithdrawalSequencerStoreError::Store(format!(
            "missing sequenced withdrawal row for {:?} while recording orphan retry attempt",
            id
        ))
    })?;
    if existing.state != WithdrawalState::MempoolAccepted {
        return Err(WithdrawalSequencerStoreError::Store(format!(
            "withdrawal {:?} epoch {} is in state {} instead of mempool_accepted during orphan retry recording",
            id,
            existing.current_epoch,
            existing.state.as_str()
        )));
    }
    if existing.current_epoch != expected_epoch {
        return Err(WithdrawalSequencerStoreError::Store(format!(
            "withdrawal {:?} orphan retry expected epoch {} but current epoch is {}",
            id, expected_epoch, existing.current_epoch
        )));
    }
    if existing.proposal_hash.as_deref() != Some(expected_proposal_hash) {
        return Err(WithdrawalSequencerStoreError::Store(format!(
            "withdrawal {:?} epoch {} proposal hash {} is not the authorized sequencer proposal during orphan retry recording",
            id, expected_epoch, expected_proposal_hash
        )));
    }
    if existing.authorized_transaction_name.is_none() {
        return Err(WithdrawalSequencerStoreError::Store(format!(
            "withdrawal {:?} epoch {} is missing authorized transaction name during orphan retry recording",
            id, expected_epoch
        )));
    }

    apply_sequencer_mutation(
        conn,
        journal,
        SequencerMutation::MempoolRetryAttempted {
            existing,
            attempt_base_height,
            error,
            updated_at,
        },
    )?;
    Ok(())
}

struct SequencerSubmissionStateUpdate {
    next_state: WithdrawalState,
    authorized_transaction: Option<StoredAuthorizedTransaction>,
    submit_attempt_count: u64,
    last_submit_attempt_base_height: u64,
    last_submit_error: Option<String>,
    updated_at: i64,
    action: &'static str,
}

fn update_sequencer_submission_state_tx(
    conn: &mut SqliteConnection,
    existing: &SequencedWithdrawalView,
    update: SequencerSubmissionStateUpdate,
) -> Result<(), BridgeError> {
    let withdrawal_nonce = existing.withdrawal_nonce.ok_or_else(|| {
        BridgeError::Runtime(format!(
            "sequenced withdrawal {:?} is missing nonce during {}",
            existing.id, update.action
        ))
    })?;
    let authorized_transaction_name = update
        .authorized_transaction
        .as_ref()
        .map(|value| value.submitted_raw_tx_id.clone())
        .or_else(|| existing.authorized_transaction_name.clone());
    let authorized_transaction_jam = update
        .authorized_transaction
        .as_ref()
        .map(|value| value.transaction_jam.clone())
        .or_else(|| existing.authorized_transaction_jam.clone());
    let authorized_raw_tx = update
        .authorized_transaction
        .as_ref()
        .map(|value| value.raw_tx_bytes.clone())
        .or_else(|| existing.authorized_raw_tx.clone());
    upsert_sequenced_withdrawal(
        conn,
        SequencerWithdrawalUpdate {
            id: existing.id.clone(),
            withdrawal_nonce,
            current_epoch: existing.current_epoch,
            proposal_hash: existing.proposal_hash.clone(),
            request_facts: existing.request_facts.clone(),
            canonical_amount: existing.canonical_amount,
            canonical_base_batch_end: existing.canonical_base_batch_end,
            canonical_transaction_jam: existing.canonical_transaction_jam.clone(),
            canonical_selected_inputs: existing.canonical_selected_inputs.clone(),
            canonical_snapshot: existing.canonical_snapshot.clone(),
            peer_commit_certificate: existing.peer_commit_certificate.clone(),
            authorized_transaction_name,
            authorized_transaction_jam,
            authorized_raw_tx,
            handoff_index: existing.handoff_index,
            turn_started_base_height: existing.turn_started_base_height,
            submit_attempt_count: update.submit_attempt_count,
            last_submit_attempt_base_height: Some(update.last_submit_attempt_base_height),
            last_submit_error: update.last_submit_error,
            state: update.next_state,
            created_at: existing.created_at,
            updated_at: update.updated_at,
        },
    )?;
    Ok(())
}

fn check_sequencer_submit_preconditions(
    conn: &mut SqliteConnection,
    proposal: &WithdrawalProposalData,
    withdrawal_nonce: u64,
) -> Result<SequencerDecision, BridgeError> {
    let request_facts = SequencerWithdrawalRequestFacts::from_proposal(proposal);
    ensure_registered_withdrawal_ordering(
        conn,
        &proposal.id,
        withdrawal_nonce,
        Some(&request_facts),
    )?;

    let proposal_hash = proposal.proposal_hash()?;
    let Some((next_id, next_nonce)) = next_pending_withdrawal_ordering(conn)? else {
        return Ok(SequencerDecision::Rejected(
            "no pending withdrawals available for submission".to_string(),
        ));
    };
    if !same_base_event_id(&next_id, &proposal.id) || next_nonce != withdrawal_nonce {
        return Ok(SequencerDecision::Rejected(format!(
            "withdrawal {:?} nonce {} is blocked by next pending withdrawal {:?} nonce {}",
            proposal.id, withdrawal_nonce, next_id, next_nonce
        )));
    }

    let Some(existing) = fetch_sequenced_withdrawal(conn, &proposal.id.base_event_id)? else {
        return Ok(SequencerDecision::Rejected(format!(
            "withdrawal {:?} epoch {} is not currently authorized for submission",
            proposal.id, proposal.epoch
        )));
    };

    if existing.current_epoch != proposal.epoch {
        return Ok(SequencerDecision::Rejected(format!(
            "withdrawal {:?} epoch {} is not the current authorized epoch",
            proposal.id, proposal.epoch
        )));
    }

    if !matches!(existing.state, WithdrawalState::Authorized) {
        if matches!(
            existing.state,
            WithdrawalState::MempoolAccepted | WithdrawalState::Confirmed
        ) && existing.proposal_hash.as_deref() == Some(proposal_hash.as_str())
        {
            return Ok(SequencerDecision::Allowed);
        }
        return Ok(SequencerDecision::Rejected(format!(
            "withdrawal {:?} epoch {} is in state {} instead of authorized",
            proposal.id,
            proposal.epoch,
            existing.state.as_str()
        )));
    }
    if existing.proposal_hash.as_deref() != Some(proposal_hash.as_str()) {
        return Ok(SequencerDecision::Rejected(format!(
            "withdrawal {:?} epoch {} proposal hash {} is not the authorized sequencer proposal",
            proposal.id, proposal.epoch, proposal_hash
        )));
    }

    Ok(SequencerDecision::Allowed)
}

fn record_tx_confirmed_tx(
    conn: &mut SqliteConnection,
    journal: &SequencerJournalHandle,
    proposal: &WithdrawalProposalData,
    confirmed_height: u64,
    confirmed_block_id: Tip5Hash,
    created_at: i64,
) -> Result<(), WithdrawalSequencerStoreError> {
    let proposal_hash = proposal.proposal_hash()?;
    let authorized_transaction = stored_authorized_transaction(&proposal.transaction)?;
    require_authorized(conn, &proposal.id, &proposal_hash)?;
    let Some(existing) = fetch_sequenced_withdrawal(conn, &proposal.id.base_event_id)? else {
        return Ok(());
    };
    if existing.state == WithdrawalState::Confirmed {
        return Ok(());
    }
    if existing.state != WithdrawalState::MempoolAccepted {
        return Ok(());
    }
    let withdrawal_nonce = existing.withdrawal_nonce.ok_or_else(|| {
        WithdrawalSequencerStoreError::Store(format!(
            "sequenced withdrawal {:?} is missing nonce during confirmation",
            proposal.id
        ))
    })?;

    apply_sequencer_mutation(
        conn,
        journal,
        SequencerMutation::TxConfirmed {
            proposal: proposal.clone(),
            existing,
            withdrawal_nonce,
            proposal_hash,
            authorized_transaction,
            confirmed_height,
            confirmed_block_id,
            created_at,
        },
    )?;
    Ok(())
}

fn record_tx_seen_mempool_accepted_tx(
    conn: &mut SqliteConnection,
    journal: &SequencerJournalHandle,
    proposal: &WithdrawalProposalData,
    created_at: i64,
) -> Result<(), WithdrawalSequencerStoreError> {
    let proposal_hash = proposal.proposal_hash()?;
    let authorized_transaction = stored_authorized_transaction(&proposal.transaction)?;
    require_authorized(conn, &proposal.id, &proposal_hash)?;
    let Some(existing) = fetch_sequenced_withdrawal(conn, &proposal.id.base_event_id)? else {
        return Ok(());
    };
    if existing.state == WithdrawalState::Confirmed {
        return Ok(());
    }
    let withdrawal_nonce = existing.withdrawal_nonce.ok_or_else(|| {
        WithdrawalSequencerStoreError::Store(format!(
            "sequenced withdrawal {:?} is missing nonce during mempool acceptance",
            proposal.id
        ))
    })?;
    apply_sequencer_mutation(
        conn,
        journal,
        SequencerMutation::TxSeenMempoolAccepted {
            proposal: proposal.clone(),
            existing,
            withdrawal_nonce,
            proposal_hash,
            authorized_transaction,
            created_at,
        },
    )?;
    Ok(())
}

/// Upserts the sequencer-owned current-state row.
fn upsert_sequenced_withdrawal(
    conn: &mut SqliteConnection,
    update: SequencerWithdrawalUpdate,
) -> Result<(), BridgeError> {
    use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

    let current_epoch = i64::try_from(update.current_epoch)
        .map_err(|err| BridgeError::ValueConversion(format!("epoch too large: {err}")))?;
    let row = SequencerWithdrawalRow {
        withdrawal_id_as_of: tip5_to_bytes(&update.id.as_of),
        withdrawal_id_base_event_id: update.id.base_event_id.0,
        withdrawal_nonce: i64::try_from(update.withdrawal_nonce).map_err(|err| {
            BridgeError::ValueConversion(format!("withdrawal nonce too large: {err}"))
        })?,
        current_epoch,
        proposal_hash: update.proposal_hash,
        request_recipient: update
            .request_facts
            .as_ref()
            .map(|facts| tip5_to_bytes(&facts.recipient)),
        request_burned_amount: update
            .request_facts
            .as_ref()
            .map(|facts| {
                i64::try_from(facts.burned_amount).map_err(|err| {
                    BridgeError::ValueConversion(format!("request burned amount too large: {err}"))
                })
            })
            .transpose()?,
        request_base_batch_end: update
            .request_facts
            .as_ref()
            .map(|facts| {
                i64::try_from(facts.base_batch_end).map_err(|err| {
                    BridgeError::ValueConversion(format!("request base_batch_end too large: {err}"))
                })
            })
            .transpose()?,
        canonical_amount: update
            .canonical_amount
            .map(|amount| {
                i64::try_from(amount).map_err(|err| {
                    BridgeError::ValueConversion(format!("canonical amount too large: {err}"))
                })
            })
            .transpose()?,
        canonical_base_batch_end: update
            .canonical_base_batch_end
            .map(|height| {
                i64::try_from(height).map_err(|err| {
                    BridgeError::ValueConversion(format!(
                        "canonical base_batch_end too large: {err}"
                    ))
                })
            })
            .transpose()?,
        canonical_transaction_jam: update.canonical_transaction_jam,
        canonical_selected_inputs_jam: update
            .canonical_selected_inputs
            .as_deref()
            .map(jam_selected_inputs)
            .transpose()?,
        canonical_snapshot_height: update
            .canonical_snapshot
            .as_ref()
            .map(|snapshot| {
                i64::try_from(snapshot.height).map_err(|err| {
                    BridgeError::ValueConversion(format!(
                        "canonical snapshot height too large: {err}"
                    ))
                })
            })
            .transpose()?,
        canonical_snapshot_block_id: update
            .canonical_snapshot
            .as_ref()
            .map(|snapshot| tip5_to_bytes(&snapshot.block_id)),
        peer_commit_certificate: update.peer_commit_certificate,
        authorized_transaction_name: update.authorized_transaction_name,
        authorized_transaction_jam: update.authorized_transaction_jam,
        authorized_raw_tx: update.authorized_raw_tx,
        handoff_index: i64::try_from(update.handoff_index).map_err(|err| {
            BridgeError::ValueConversion(format!("handoff index too large: {err}"))
        })?,
        turn_started_base_height: update
            .turn_started_base_height
            .map(|height| {
                i64::try_from(height).map_err(|err| {
                    BridgeError::ValueConversion(format!(
                        "turn_started_base_height too large: {err}"
                    ))
                })
            })
            .transpose()?,
        submit_attempt_count: i64::try_from(update.submit_attempt_count).map_err(|err| {
            BridgeError::ValueConversion(format!("submit attempt count too large: {err}"))
        })?,
        last_submit_attempt_base_height: update
            .last_submit_attempt_base_height
            .map(|height| {
                i64::try_from(height).map_err(|err| {
                    BridgeError::ValueConversion(format!(
                        "last_submit_attempt_base_height too large: {err}"
                    ))
                })
            })
            .transpose()?,
        last_submit_error: update.last_submit_error,
        state: update.state.as_str().to_string(),
        created_at: update.created_at,
        updated_at: update.updated_at,
    };

    diesel::insert_into(sequencer_withdrawals::table)
        .values(&row)
        .on_conflict(sequenced::withdrawal_id_base_event_id)
        .do_update()
        .set((
            sequenced::withdrawal_id_as_of.eq(&row.withdrawal_id_as_of),
            sequenced::withdrawal_nonce.eq(row.withdrawal_nonce),
            sequenced::current_epoch.eq(row.current_epoch),
            sequenced::proposal_hash.eq(&row.proposal_hash),
            sequenced::request_recipient.eq(&row.request_recipient),
            sequenced::request_burned_amount.eq(row.request_burned_amount),
            sequenced::request_base_batch_end.eq(row.request_base_batch_end),
            sequenced::canonical_amount.eq(row.canonical_amount),
            sequenced::canonical_base_batch_end.eq(row.canonical_base_batch_end),
            sequenced::canonical_transaction_jam.eq(&row.canonical_transaction_jam),
            sequenced::canonical_selected_inputs_jam.eq(&row.canonical_selected_inputs_jam),
            sequenced::canonical_snapshot_height.eq(row.canonical_snapshot_height),
            sequenced::canonical_snapshot_block_id.eq(&row.canonical_snapshot_block_id),
            sequenced::peer_commit_certificate.eq(&row.peer_commit_certificate),
            sequenced::authorized_transaction_name.eq(&row.authorized_transaction_name),
            sequenced::authorized_transaction_jam.eq(&row.authorized_transaction_jam),
            sequenced::authorized_raw_tx.eq(&row.authorized_raw_tx),
            sequenced::handoff_index.eq(row.handoff_index),
            sequenced::turn_started_base_height.eq(row.turn_started_base_height),
            sequenced::submit_attempt_count.eq(row.submit_attempt_count),
            sequenced::last_submit_attempt_base_height.eq(row.last_submit_attempt_base_height),
            sequenced::last_submit_error.eq(&row.last_submit_error),
            sequenced::state.eq(&row.state),
            sequenced::updated_at.eq(row.updated_at),
        ))
        .execute(conn)
        .map_err(|err| {
            BridgeError::Runtime(format!("sequencer withdrawal upsert failed: {err}"))
        })?;
    Ok(())
}

/// Materializes the fresh `Pending` sequencer row for a newly ordered
/// withdrawal.
fn create_ordered_sequencer_row(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
    withdrawal_nonce: u64,
    request_facts: SequencerWithdrawalRequestFacts,
    turn_started_base_height: Option<u64>,
    created_at: i64,
) -> Result<(), BridgeError> {
    upsert_sequenced_withdrawal(
        conn,
        SequencerWithdrawalUpdate {
            id: id.clone(),
            withdrawal_nonce,
            current_epoch: 0,
            proposal_hash: None,
            request_facts: Some(request_facts),
            canonical_amount: None,
            canonical_base_batch_end: None,
            canonical_transaction_jam: None,
            canonical_selected_inputs: None,
            canonical_snapshot: None,
            peer_commit_certificate: None,
            authorized_transaction_name: None,
            authorized_transaction_jam: None,
            authorized_raw_tx: None,
            handoff_index: 0,
            turn_started_base_height,
            submit_attempt_count: 0,
            last_submit_attempt_base_height: None,
            last_submit_error: None,
            state: WithdrawalState::Pending,
            created_at,
            updated_at: created_at,
        },
    )
}

/// Persists the accepted tracked request directly on the sequencer row and
/// rejects nonce gaps, mismatches, or canonical ordering regressions.
fn ensure_tracked_withdrawal_ordering_tx(
    conn: &mut SqliteConnection,
    journal: &SequencerJournalHandle,
    id: &WithdrawalId,
    withdrawal_nonce: u64,
    request_facts: SequencerWithdrawalRequestFacts,
    turn_started_base_height: Option<u64>,
    created_at: i64,
) -> Result<(), BridgeError> {
    use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

    let withdrawal_nonce_i64 = i64::try_from(withdrawal_nonce).map_err(|err| {
        BridgeError::ValueConversion(format!("withdrawal nonce too large: {err}"))
    })?;
    let existing_by_id = sequencer_withdrawals::table
        .filter(sequenced::withdrawal_id_base_event_id.eq(id.base_event_id.0.clone()))
        .first::<SequencerWithdrawalStoredRow>(conn)
        .optional()
        .map_err(|err| {
            BridgeError::Runtime(format!("sequencer withdrawal fetch by id failed: {err}"))
        })?;
    if let Some(existing) = existing_by_id {
        if existing.withdrawal_nonce != withdrawal_nonce_i64 {
            return Err(BridgeError::Runtime(format!(
                "sequencer withdrawal nonce {} does not match presented nonce {} for {:?}",
                existing.withdrawal_nonce, withdrawal_nonce, id
            )));
        }
        ensure_existing_request_facts_match(id, &existing, &request_facts)?;
        return Ok(());
    }

    let existing_by_nonce = sequencer_withdrawals::table
        .filter(sequenced::withdrawal_nonce.eq(withdrawal_nonce_i64))
        .first::<SequencerWithdrawalStoredRow>(conn)
        .optional()
        .map_err(|err| {
            BridgeError::Runtime(format!("sequencer withdrawal fetch by nonce failed: {err}"))
        })?;
    if let Some(existing) = existing_by_nonce {
        return Err(BridgeError::Runtime(format!(
            "withdrawal nonce {} is already assigned to {:?}",
            withdrawal_nonce,
            WithdrawalId {
                as_of: Tip5Hash::from_be_limb_bytes(&existing.withdrawal_id_as_of).map_err(
                    |err| BridgeError::Runtime(format!("invalid stored sequencer as_of: {err}")),
                )?,
                base_event_id: existing.withdrawal_id_base_event_id.into(),
            }
        )));
    }

    let expected_next_nonce = sequencer_withdrawals::table
        .select(diesel::dsl::max(sequenced::withdrawal_nonce))
        .first::<Option<i64>>(conn)
        .map_err(|err| {
            BridgeError::Runtime(format!("sequencer withdrawal max nonce failed: {err}"))
        })?
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| BridgeError::Runtime("sequencer withdrawal nonce overflow".into()))?;
    if expected_next_nonce != withdrawal_nonce_i64 {
        return Err(BridgeError::Runtime(format!(
            "expected next withdrawal nonce {}, got {} for {:?}",
            expected_next_nonce, withdrawal_nonce, id
        )));
    }
    ensure_new_request_follows_canonical_order(conn, id, &request_facts)?;

    apply_sequencer_mutation(
        conn,
        journal,
        SequencerMutation::WithdrawalOrdered {
            id: id.clone(),
            withdrawal_nonce,
            request_facts,
            turn_started_base_height,
            created_at,
        },
    )
}

fn ensure_registered_withdrawal_ordering(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
    withdrawal_nonce: u64,
    request_facts: Option<&SequencerWithdrawalRequestFacts>,
) -> Result<(), BridgeError> {
    use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

    let withdrawal_nonce_i64 = i64::try_from(withdrawal_nonce).map_err(|err| {
        BridgeError::ValueConversion(format!("withdrawal nonce too large: {err}"))
    })?;
    let existing = sequencer_withdrawals::table
        .filter(sequenced::withdrawal_id_base_event_id.eq(id.base_event_id.0.clone()))
        .first::<SequencerWithdrawalStoredRow>(conn)
        .optional()
        .map_err(|err| {
            BridgeError::Runtime(format!("sequencer withdrawal fetch by id failed: {err}"))
        })?
        .ok_or_else(|| {
            BridgeError::Runtime(format!(
                "sequencer withdrawal {:?} must be registered before this operation",
                id
            ))
        })?;
    if existing.withdrawal_nonce != withdrawal_nonce_i64 {
        return Err(BridgeError::Runtime(format!(
            "sequencer withdrawal nonce {} does not match presented nonce {} for {:?}",
            existing.withdrawal_nonce, withdrawal_nonce, id
        )));
    }
    if let Some(request_facts) = request_facts {
        ensure_existing_request_facts_match(id, &existing, request_facts)?;
    }
    Ok(())
}

fn stored_request_facts(
    row: &SequencerWithdrawalStoredRow,
) -> Result<Option<SequencerWithdrawalRequestFacts>, BridgeError> {
    match (
        row.request_recipient.as_deref(),
        row.request_burned_amount,
        row.request_base_batch_end,
    ) {
        (Some(recipient), Some(burned_amount), Some(base_batch_end)) => {
            Ok(Some(SequencerWithdrawalRequestFacts {
                recipient: tip5_from_bytes(recipient)?,
                burned_amount: u64::try_from(burned_amount).map_err(|err| {
                    BridgeError::ValueConversion(format!("request burned amount overflow: {err}"))
                })?,
                base_batch_end: u64::try_from(base_batch_end).map_err(|err| {
                    BridgeError::ValueConversion(format!("request base_batch_end overflow: {err}"))
                })?,
            }))
        }
        (None, None, None) => Ok(None),
        _ => Err(BridgeError::Runtime(
            "sequencer withdrawal request fact columns must be all null or all present".into(),
        )),
    }
}

fn ensure_existing_request_facts_match(
    id: &WithdrawalId,
    row: &SequencerWithdrawalStoredRow,
    request_facts: &SequencerWithdrawalRequestFacts,
) -> Result<(), BridgeError> {
    let Some(existing) = stored_request_facts(row)? else {
        return Err(BridgeError::Runtime(format!(
            "sequencer withdrawal {:?} is missing request facts needed to validate registration",
            id
        )));
    };
    if &existing != request_facts {
        return Err(BridgeError::Runtime(format!(
            "sequencer withdrawal {:?} request facts do not match presented registration facts",
            id
        )));
    }
    Ok(())
}

fn ensure_new_request_follows_canonical_order(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
    request_facts: &SequencerWithdrawalRequestFacts,
) -> Result<(), BridgeError> {
    use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

    let missing_request_facts = sequencer_withdrawals::table
        .filter(sequenced::request_base_batch_end.is_null())
        .select(sequenced::withdrawal_nonce)
        .limit(1)
        .get_result::<i64>(conn)
        .optional()
        .map_err(|err| {
            BridgeError::Runtime(format!(
                "sequencer withdrawal missing request fact lookup failed: {err}"
            ))
        })?;
    if missing_request_facts.is_some() {
        return Err(BridgeError::Runtime(format!(
            "cannot validate canonical registration for {:?}: sequencer has rows without request facts",
            id
        )));
    }

    // This is an index-head lookup over sequencer_withdrawals_by_request_ordering;
    // keep the LIMIT explicit so the projection never materializes more than one row.
    let max_existing_key = sequencer_withdrawals::table
        .filter(sequenced::request_base_batch_end.is_not_null())
        .select((
            sequenced::request_base_batch_end,
            sequenced::withdrawal_id_base_event_id,
        ))
        .order_by((
            sequenced::request_base_batch_end.desc(),
            sequenced::withdrawal_id_base_event_id.desc(),
        ))
        .limit(1)
        .get_result::<(Option<i64>, Vec<u8>)>(conn)
        .optional()
        .map_err(|err| {
            BridgeError::Runtime(format!(
                "sequencer withdrawal canonical request head lookup failed: {err}"
            ))
        })?;
    let Some((Some(max_base_batch_end), max_base_event_id)) = max_existing_key else {
        return Ok(());
    };
    let max_base_batch_end = u64::try_from(max_base_batch_end).map_err(|err| {
        BridgeError::ValueConversion(format!("request base_batch_end overflow: {err}"))
    })?;
    if canonical_withdrawal_ordering_cmp(
        request_facts.base_batch_end, &id.base_event_id.0, max_base_batch_end, &max_base_event_id,
    )
    .is_lt()
    {
        return Err(BridgeError::Runtime(format!(
            "new sequencer withdrawal {:?} would sort before already registered withdrawal history",
            id
        )));
    }
    Ok(())
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

fn same_base_event_id(left: &WithdrawalId, right: &WithdrawalId) -> bool {
    left.base_event_id == right.base_event_id
}

/// Loads the accepted nonce for a withdrawal from the durable sequencer row.
fn fetch_withdrawal_nonce(
    conn: &mut SqliteConnection,
    base_event_id: &AtomBytes,
) -> Result<Option<u64>, BridgeError> {
    use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

    let row = sequencer_withdrawals::table
        .filter(sequenced::withdrawal_id_base_event_id.eq(base_event_id.0.clone()))
        .select(sequenced::withdrawal_nonce)
        .first::<i64>(conn)
        .optional()
        .map_err(|err| {
            BridgeError::Runtime(format!("sequencer withdrawal nonce fetch failed: {err}"))
        })?;
    row.map(|nonce| {
        u64::try_from(nonce).map_err(|err| {
            BridgeError::ValueConversion(format!("withdrawal nonce overflow: {err}"))
        })
    })
    .transpose()
}

/// Returns the lowest sequenced withdrawal row that is still live from the
/// sequencer's ordering perspective.
fn current_live_withdrawal_frontier(
    conn: &mut SqliteConnection,
) -> Result<Option<(WithdrawalId, u64)>, BridgeError> {
    use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

    let unreleased_states = [
        WithdrawalState::Pending.as_str(),
        WithdrawalState::PeerCanonical.as_str(),
        WithdrawalState::Authorized.as_str(),
    ];
    let row = sequencer_withdrawals::table
        .filter(sequenced::state.eq_any(unreleased_states))
        .order(sequenced::withdrawal_nonce.asc())
        .first::<SequencerWithdrawalStoredRow>(conn)
        .optional()
        .map_err(|err| {
            BridgeError::Runtime(format!(
                "next pending sequencer withdrawal fetch failed: {err}"
            ))
        })?;

    row.map(try_into_sequenced_withdrawal_view)
        .transpose()?
        .map(|state| {
            let nonce = state
                .withdrawal_nonce
                .ok_or_else(|| BridgeError::Runtime("sequenced withdrawal missing nonce".into()))?;
            Ok((state.id, nonce))
        })
        .transpose()
}

/// Returns the lowest sequenced withdrawal nonce that is not yet released from
/// ordering. This keeps the legacy id+nonce query aligned with the nonce-only
/// sequencer frontier API.
fn next_pending_withdrawal_ordering(
    conn: &mut SqliteConnection,
) -> Result<Option<(WithdrawalId, u64)>, BridgeError> {
    current_live_withdrawal_frontier(conn)
}

/// Returns the nonce-only current live withdrawal frontier.
fn current_live_withdrawal_nonce(conn: &mut SqliteConnection) -> Result<Option<u64>, BridgeError> {
    current_live_withdrawal_frontier(conn).map(|row| row.map(|(_, nonce)| nonce))
}

fn ensure_withdrawal_is_current_frontier(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
    withdrawal_nonce: u64,
    action: &str,
) -> Result<(), BridgeError> {
    let Some((frontier_id, frontier_nonce)) = current_live_withdrawal_frontier(conn)? else {
        return Err(BridgeError::Runtime(format!(
            "cannot {action} for withdrawal {:?} nonce {} because the sequencer has no current frontier",
            id, withdrawal_nonce
        )));
    };
    if !same_base_event_id(&frontier_id, id) || frontier_nonce != withdrawal_nonce {
        return Err(BridgeError::Runtime(format!(
            "cannot {action} for withdrawal {:?} nonce {} while sequencer frontier is {:?} nonce {}",
            id, withdrawal_nonce, frontier_id, frontier_nonce
        )));
    }
    Ok(())
}

/// Checks the sequencer authority directly: the id must be registered and be
/// the current frontier row.
fn frontier_allows_withdrawal(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
) -> Result<WithdrawalFrontierCheck, BridgeError> {
    let registered = fetch_sequenced_withdrawal(conn, &id.base_event_id)?.is_some();
    let is_frontier = current_live_withdrawal_frontier(conn)?
        .map(|(frontier_id, _)| same_base_event_id(&frontier_id, id))
        .unwrap_or(false);
    Ok(WithdrawalFrontierCheck {
        registered,
        is_frontier,
    })
}

/// Returns the single-flight authorized withdrawal, if one exists.
fn fetch_active_in_flight(
    conn: &mut SqliteConnection,
) -> Result<Option<SequencedWithdrawalView>, BridgeError> {
    use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

    let row = sequencer_withdrawals::table
        .filter(sequenced::state.eq(WithdrawalState::Authorized.as_str()))
        .order(sequenced::updated_at.asc())
        .first::<SequencerWithdrawalStoredRow>(conn)
        .optional()
        .map_err(|err| {
            BridgeError::Runtime(format!("active sequencer withdrawal fetch failed: {err}"))
        })?;
    row.map(try_into_sequenced_withdrawal_view).transpose()
}

/// Ensures that the given proposal hash is the currently authorized proposal
/// for this withdrawal.
fn require_authorized(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
    proposal_hash: &str,
) -> Result<(), WithdrawalSequencerStoreError> {
    let Some(existing) = fetch_sequenced_withdrawal(conn, &id.base_event_id)? else {
        return Err(WithdrawalSequencerStoreError::NotAuthorized {
            id: Box::new(id.clone()),
            proposal_hash: proposal_hash.to_string().into_boxed_str(),
        });
    };
    if existing.proposal_hash.as_deref() != Some(proposal_hash) {
        return Err(WithdrawalSequencerStoreError::NotAuthorized {
            id: Box::new(id.clone()),
            proposal_hash: proposal_hash.to_string().into_boxed_str(),
        });
    }
    Ok(())
}

/// Loads one sequencer-owned current-state row by Base burn event id.
fn fetch_sequenced_withdrawal(
    conn: &mut SqliteConnection,
    base_event_id: &AtomBytes,
) -> Result<Option<SequencedWithdrawalView>, BridgeError> {
    use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

    let row = sequencer_withdrawals::table
        .filter(sequenced::withdrawal_id_base_event_id.eq(base_event_id.0.clone()))
        .first::<SequencerWithdrawalStoredRow>(conn)
        .optional()
        .map_err(|err| BridgeError::Runtime(format!("sequencer withdrawal fetch failed: {err}")))?;
    row.map(try_into_sequenced_withdrawal_view).transpose()
}

fn load_canonical_proposal_artifacts(
    conn: &mut SqliteConnection,
    base_event_id: &AtomBytes,
) -> Result<Option<WithdrawalSequencerProposalArtifacts>, BridgeError> {
    let Some(row) = fetch_sequenced_withdrawal(conn, base_event_id)? else {
        return Ok(None);
    };
    let proposal_hash = row.proposal_hash.clone().ok_or_else(|| {
        BridgeError::Runtime(format!(
            "sequenced withdrawal {:?} epoch {} is missing canonical proposal hash",
            row.id, row.current_epoch
        ))
    })?;
    let amount = row.canonical_amount.ok_or_else(|| {
        BridgeError::Runtime(format!(
            "sequenced withdrawal {:?} epoch {} is missing canonical amount",
            row.id, row.current_epoch
        ))
    })?;
    let base_batch_end = row.canonical_base_batch_end.ok_or_else(|| {
        BridgeError::Runtime(format!(
            "sequenced withdrawal {:?} epoch {} is missing canonical base_batch_end",
            row.id, row.current_epoch
        ))
    })?;
    let snapshot = row.canonical_snapshot.clone().ok_or_else(|| {
        BridgeError::Runtime(format!(
            "sequenced withdrawal {:?} epoch {} is missing canonical snapshot",
            row.id, row.current_epoch
        ))
    })?;
    let selected_inputs = row.canonical_selected_inputs.clone().ok_or_else(|| {
        BridgeError::Runtime(format!(
            "sequenced withdrawal {:?} epoch {} is missing canonical selected inputs",
            row.id, row.current_epoch
        ))
    })?;
    let transaction_jam = row.canonical_transaction_jam.clone().ok_or_else(|| {
        BridgeError::Runtime(format!(
            "sequenced withdrawal {:?} epoch {} is missing canonical transaction jam",
            row.id, row.current_epoch
        ))
    })?;
    let transaction = cue_transaction(transaction_jam)?;

    Ok(Some(WithdrawalSequencerProposalArtifacts {
        id: row.id,
        epoch: row.current_epoch,
        proposal_hash,
        amount,
        base_batch_end,
        snapshot,
        selected_inputs,
        transaction,
        commit_certificate: row.peer_commit_certificate,
        authorized_transaction_name: row.authorized_transaction_name,
        authorized_transaction_jam: row.authorized_transaction_jam,
        authorized_raw_tx: row.authorized_raw_tx,
    }))
}

/// Loads all sequencer-owned current-state rows.
fn fetch_all_sequenced_withdrawals(
    conn: &mut SqliteConnection,
) -> Result<Vec<LiveWithdrawalView>, BridgeError> {
    use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

    let rows = sequencer_withdrawals::table
        .order((
            sequenced::updated_at.asc(),
            sequenced::withdrawal_id_as_of.asc(),
        ))
        .load::<SequencerWithdrawalStoredRow>(conn)
        .map_err(|err| BridgeError::Runtime(format!("sequencer withdrawal list failed: {err}")))?;
    rows.into_iter()
        .map(try_into_sequenced_withdrawal_view)
        .map(|row| row.map(SequencedWithdrawalView::into_live_withdrawal_view))
        .collect()
}

fn note_name_sort_key(name: &nockchain_types::v1::Name) -> ([u8; 40], [u8; 40]) {
    (name.first.to_be_limb_bytes(), name.last.to_be_limb_bytes())
}

fn canonical_amount_from_proposal(proposal: &WithdrawalProposalData) -> Option<u64> {
    Some(proposal.amount)
}

fn canonical_base_batch_end_from_proposal(proposal: &WithdrawalProposalData) -> Option<u64> {
    Some(proposal.base_batch_end)
}

fn canonical_transaction_jam_from_proposal(
    proposal: &WithdrawalProposalData,
) -> Result<Option<Vec<u8>>, BridgeError> {
    Ok(Some(jam_transaction(&proposal.transaction)?))
}

fn canonical_selected_inputs_from_proposal(
    proposal: &WithdrawalProposalData,
) -> Option<Vec<nockchain_types::v1::Name>> {
    Some(normalized_note_names(&proposal.selected_inputs))
}

fn canonical_snapshot_from_proposal(
    proposal: &WithdrawalProposalData,
) -> Option<WithdrawalSnapshot> {
    Some(proposal.snapshot.clone())
}

pub(crate) fn validate_canonical_proposal_tx_inputs(
    proposal: &WithdrawalProposalData,
) -> Result<(), BridgeError> {
    let actual_inputs = proposal.transaction.normalized_input_names();
    let claimed_inputs = normalized_note_names(&proposal.selected_inputs);
    if actual_inputs != claimed_inputs {
        return Err(BridgeError::Runtime(format!(
            "withdrawal {:?} epoch {} transaction inputs do not match proposal selected_inputs",
            proposal.id, proposal.epoch
        )));
    }
    Ok(())
}

fn insert_reserved_input(
    conn: &mut SqliteConnection,
    row: &SequencerReservedInputRow,
) -> Result<(), BridgeError> {
    use crate::withdrawal::sequencer::schema::withdrawal_reserved_inputs::dsl as reserved;

    let epoch = i64::try_from(row.epoch)
        .map_err(|err| BridgeError::ValueConversion(format!("epoch too large: {err}")))?;
    diesel::insert_into(withdrawal_reserved_inputs::table)
        .values((
            reserved::withdrawal_id_as_of.eq(tip5_to_bytes(&row.id.as_of)),
            reserved::withdrawal_id_base_event_id.eq(row.id.base_event_id.0.clone()),
            reserved::epoch.eq(epoch),
            reserved::input_first.eq(tip5_to_bytes(&row.input.first)),
            reserved::input_last.eq(tip5_to_bytes(&row.input.last)),
            reserved::created_at.eq(row.created_at),
            reserved::updated_at.eq(row.updated_at),
        ))
        .execute(conn)
        .map_err(|err| BridgeError::Runtime(format!("reserved input insert failed: {err}")))?;
    Ok(())
}

fn try_into_reserved_input_row(
    row: ReservedInputSqlRow,
) -> Result<SequencerReservedInputRow, BridgeError> {
    let (
        withdrawal_id_as_of,
        withdrawal_id_base_event_id,
        epoch,
        input_first,
        input_last,
        created_at,
        updated_at,
    ) = row;
    Ok(SequencerReservedInputRow {
        id: WithdrawalId {
            as_of: tip5_from_bytes(&withdrawal_id_as_of)?,
            base_event_id: AtomBytes(withdrawal_id_base_event_id),
        },
        epoch: u64::try_from(epoch)
            .map_err(|err| BridgeError::ValueConversion(format!("epoch overflow: {err}")))?,
        input: nockchain_types::v1::Name::new(
            tip5_from_bytes(&input_first)?,
            tip5_from_bytes(&input_last)?,
        ),
        created_at,
        updated_at,
    })
}

fn load_reserved_input_rows(
    conn: &mut SqliteConnection,
) -> Result<Vec<SequencerReservedInputRow>, BridgeError> {
    use crate::withdrawal::sequencer::schema::withdrawal_reserved_inputs::dsl as reserved;

    let rows = withdrawal_reserved_inputs::table
        .select((
            reserved::withdrawal_id_as_of,
            reserved::withdrawal_id_base_event_id,
            reserved::epoch,
            reserved::input_first,
            reserved::input_last,
            reserved::created_at,
            reserved::updated_at,
        ))
        .order((
            reserved::withdrawal_id_as_of.asc(),
            reserved::withdrawal_id_base_event_id.asc(),
            reserved::input_first.asc(),
            reserved::input_last.asc(),
        ))
        .load::<ReservedInputSqlRow>(conn)
        .map_err(|err| BridgeError::Runtime(format!("reserved input load failed: {err}")))?;
    rows.into_iter().map(try_into_reserved_input_row).collect()
}

fn load_reserved_input_rows_for_withdrawal(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
) -> Result<Vec<SequencerReservedInputRow>, BridgeError> {
    use crate::withdrawal::sequencer::schema::withdrawal_reserved_inputs::dsl as reserved;

    let rows = withdrawal_reserved_inputs::table
        .filter(reserved::withdrawal_id_base_event_id.eq(id.base_event_id.0.clone()))
        .select((
            reserved::withdrawal_id_as_of,
            reserved::withdrawal_id_base_event_id,
            reserved::epoch,
            reserved::input_first,
            reserved::input_last,
            reserved::created_at,
            reserved::updated_at,
        ))
        .order((reserved::input_first.asc(), reserved::input_last.asc()))
        .load::<ReservedInputSqlRow>(conn)
        .map_err(|err| {
            BridgeError::Runtime(format!("withdrawal reserved input load failed: {err}"))
        })?;
    rows.into_iter().map(try_into_reserved_input_row).collect()
}

#[cfg(test)]
fn load_reserved_input_names_for_withdrawal(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
) -> Result<Vec<nockchain_types::v1::Name>, BridgeError> {
    let mut names = load_reserved_input_rows_for_withdrawal(conn, id)?
        .into_iter()
        .map(|row| row.input)
        .collect::<Vec<_>>();
    names.sort_by_key(note_name_sort_key);
    Ok(names)
}

fn list_reserved_input_names(
    conn: &mut SqliteConnection,
) -> Result<Vec<nockchain_types::v1::Name>, BridgeError> {
    let mut names = load_reserved_input_rows(conn)?
        .into_iter()
        .map(|row| row.input)
        .collect::<Vec<_>>();
    names.sort_by_key(note_name_sort_key);
    Ok(names)
}

fn clear_reserved_inputs_for_withdrawal(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
) -> Result<(), BridgeError> {
    use crate::withdrawal::sequencer::schema::withdrawal_reserved_inputs::dsl as reserved;

    diesel::delete(
        withdrawal_reserved_inputs::table
            .filter(reserved::withdrawal_id_base_event_id.eq(id.base_event_id.0.clone())),
    )
    .execute(conn)
    .map_err(|err| BridgeError::Runtime(format!("reserved input delete failed: {err}")))?;
    Ok(())
}

fn ensure_canonical_proposal_inputs_are_unreserved(
    conn: &mut SqliteConnection,
    proposal: &WithdrawalProposalData,
) -> Result<(), BridgeError> {
    let desired = normalized_note_names(&proposal.selected_inputs);
    for existing in load_reserved_input_rows(conn)? {
        if same_base_event_id(&existing.id, &proposal.id) {
            continue;
        }
        if desired.contains(&existing.input) {
            return Err(BridgeError::Runtime(format!(
                "cannot record canonical proposal for withdrawal {:?} epoch {} because input {:?} is already reserved by withdrawal {:?} epoch {}",
                proposal.id, proposal.epoch, existing.input, existing.id, existing.epoch
            )));
        }
    }
    Ok(())
}

fn insert_reserved_inputs_for_proposal(
    conn: &mut SqliteConnection,
    proposal: &WithdrawalProposalData,
    created_at: i64,
) -> Result<(), BridgeError> {
    insert_reserved_inputs_for_names(
        conn,
        &proposal.id,
        proposal.epoch,
        &proposal.selected_inputs,
        created_at,
        SequencerJournalApplyMode::Runtime,
    )
}

fn insert_reserved_inputs_for_journal_names(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
    epoch: u64,
    inputs: &[nockchain_types::v1::Name],
    created_at: i64,
    mode: SequencerJournalApplyMode,
) -> Result<(), BridgeError> {
    insert_reserved_inputs_for_names(conn, id, epoch, inputs, created_at, mode)
}

fn insert_reserved_inputs_for_names(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
    epoch: u64,
    inputs: &[nockchain_types::v1::Name],
    created_at: i64,
    mode: SequencerJournalApplyMode,
) -> Result<(), BridgeError> {
    let desired = normalized_note_names(inputs);
    for existing in load_reserved_input_rows(conn)? {
        if same_base_event_id(&existing.id, id) {
            continue;
        }
        if desired.contains(&existing.input) {
            return Err(BridgeError::Runtime(format!(
                "cannot reserve journal input {:?} for withdrawal {:?} epoch {} because it is already reserved by withdrawal {:?} epoch {}",
                existing.input, id, epoch, existing.id, existing.epoch
            )));
        }
    }

    let existing_rows = load_reserved_input_rows_for_withdrawal(conn, id)?;
    let existing_epoch = existing_rows.first().map(|row| row.epoch);
    let mut existing_inputs = existing_rows
        .iter()
        .map(|row| row.input.clone())
        .collect::<Vec<_>>();
    existing_inputs.sort_by_key(note_name_sort_key);

    if existing_epoch == Some(epoch) && existing_inputs == desired {
        return Ok(());
    }
    if mode == SequencerJournalApplyMode::Replay && !existing_rows.is_empty() {
        return Err(BridgeError::Runtime(format!(
            "journal replay reserved inputs for withdrawal {:?} epoch {} do not match existing reservation",
            id, epoch
        )));
    }

    clear_reserved_inputs_for_withdrawal(conn, id)?;
    for input in desired {
        insert_reserved_input(
            conn,
            &SequencerReservedInputRow {
                id: id.clone(),
                epoch,
                input,
                created_at,
                updated_at: created_at,
            },
        )?;
    }
    Ok(())
}

fn ensure_reserved_inputs_match_pinned_proposal(
    conn: &mut SqliteConnection,
    proposal: &WithdrawalProposalData,
    created_at: i64,
) -> Result<(), BridgeError> {
    let desired = normalized_note_names(&proposal.selected_inputs);

    let existing_rows = load_reserved_input_rows_for_withdrawal(conn, &proposal.id)?;
    if existing_rows.is_empty() {
        return insert_reserved_inputs_for_proposal(conn, proposal, created_at);
    }

    let existing_epoch = existing_rows.first().map(|row| row.epoch);
    let mut existing_inputs = existing_rows
        .iter()
        .map(|row| row.input.clone())
        .collect::<Vec<_>>();
    existing_inputs.sort_by_key(note_name_sort_key);

    if existing_epoch == Some(proposal.epoch) && existing_inputs == desired {
        return Ok(());
    }

    Err(BridgeError::Runtime(format!(
        "reserved inputs for withdrawal {:?} do not match pinned canonical proposal at epoch {}",
        proposal.id, proposal.epoch
    )))
}

/// Returns whether a signer-specific `proposal_signed` event already exists for
/// this proposal hash.
fn signed_proposal_exists(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
    epoch: u64,
    signer_node_id: u64,
    proposal_hash: &str,
) -> Result<bool, BridgeError> {
    use crate::withdrawal::sequencer::schema::withdrawal_submission_events::dsl as events;

    let epoch = i64::try_from(epoch)
        .map_err(|err| BridgeError::ValueConversion(format!("epoch too large: {err}")))?;
    let signer_node_id = i64::try_from(signer_node_id)
        .map_err(|err| BridgeError::ValueConversion(format!("signer node id too large: {err}")))?;
    let count = withdrawal_submission_events::table
        .filter(events::withdrawal_id_base_event_id.eq(id.base_event_id.0.clone()))
        .filter(events::epoch.eq(epoch))
        .filter(events::proposal_hash.eq(proposal_hash.to_string()))
        .filter(events::event_type.eq(WithdrawalSubmissionEventType::ProposalSigned.as_str()))
        .filter(events::signer_node_id.eq(Some(signer_node_id)))
        .count()
        .get_result::<i64>(conn)
        .map_err(|err| {
            BridgeError::Runtime(format!("signed proposal existence query failed: {err}"))
        })?;
    Ok(count > 0)
}

/// Loads the latest signed transaction contribution from each signer for a
/// proposal hash.
fn load_signed_transaction_records(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
    epoch: u64,
    proposal_hash: &str,
) -> Result<Vec<SignedWithdrawalTransactionRecord>, BridgeError> {
    use crate::withdrawal::sequencer::schema::withdrawal_submission_events::dsl as events;

    let epoch = i64::try_from(epoch)
        .map_err(|err| BridgeError::ValueConversion(format!("epoch too large: {err}")))?;
    let rows = withdrawal_submission_events::table
        .filter(events::withdrawal_id_base_event_id.eq(id.base_event_id.0.clone()))
        .filter(events::epoch.eq(epoch))
        .filter(events::proposal_hash.eq(proposal_hash.to_string()))
        .filter(events::event_type.eq(WithdrawalSubmissionEventType::ProposalSigned.as_str()))
        .order((events::created_at.asc(), events::event_id.asc()))
        .load::<WithdrawalSubmissionEventRow>(conn)
        .map_err(|err| BridgeError::Runtime(format!("signed proposal load failed: {err}")))?;

    let mut deduped = std::collections::BTreeMap::<u64, SignedWithdrawalTransactionRecord>::new();
    for row in rows {
        let Some(signer_node_id) = row.signer_node_id else {
            return Err(BridgeError::Runtime(
                "proposal_signed event missing signer_node_id".into(),
            ));
        };
        let signer_node_id = u64::try_from(signer_node_id).map_err(|err| {
            BridgeError::ValueConversion(format!("signed proposal signer overflow: {err}"))
        })?;
        let Some(transaction_jam) = row._transaction_jam else {
            return Err(BridgeError::Runtime(
                "proposal_signed event missing transaction_jam".into(),
            ));
        };
        deduped.insert(
            signer_node_id,
            SignedWithdrawalTransactionRecord {
                signer_node_id,
                created_at: row.created_at,
                transaction: cue_transaction(transaction_jam)?,
            },
        );
    }
    Ok(deduped.into_values().collect())
}

fn load_authorized_transaction_for_retry(
    conn: &mut SqliteConnection,
    id: &WithdrawalId,
) -> Result<Option<AuthorizedRetryPayload>, BridgeError> {
    let Some(existing) = fetch_sequenced_withdrawal(conn, &id.base_event_id)? else {
        return Ok(None);
    };
    if existing.state != WithdrawalState::MempoolAccepted {
        return Err(BridgeError::Runtime(format!(
            "withdrawal {:?} epoch {} is in state {} instead of mempool_accepted for orphan retry",
            id,
            existing.current_epoch,
            existing.state.as_str()
        )));
    }
    let proposal_hash = existing.proposal_hash.clone().ok_or_else(|| {
        BridgeError::Runtime(format!(
            "withdrawal {:?} epoch {} is missing authorized proposal hash for orphan retry",
            id, existing.current_epoch
        ))
    })?;
    let authorized_raw_tx_id = existing
        .authorized_transaction_name
        .clone()
        .ok_or_else(|| {
            BridgeError::Runtime(format!(
                "withdrawal {:?} epoch {} is missing authorized raw tx id for orphan retry",
                id, existing.current_epoch
            ))
        })?;
    let raw_tx_bytes = match existing.authorized_raw_tx {
        Some(raw_tx_bytes) => raw_tx_bytes,
        None => {
            let transaction_jam = existing.authorized_transaction_jam.ok_or_else(|| {
                BridgeError::Runtime(format!(
                    "withdrawal {:?} epoch {} proposal {} is missing both authorized_raw_tx and authorized_transaction_jam for orphan retry",
                    id, existing.current_epoch, proposal_hash
                ))
            })?;
            let transaction = cue_transaction(transaction_jam)?;
            withdrawal_raw_tx::persisted_raw_tx_from_transaction(&transaction)?.raw_tx_bytes
        }
    };

    Ok(Some(AuthorizedRetryPayload {
        id: existing.id,
        epoch: existing.current_epoch,
        proposal_hash,
        submitted_raw_tx_id: authorized_raw_tx_id,
        raw_tx_bytes,
    }))
}

fn load_authorized_transaction_export_by_tx_id(
    conn: &mut SqliteConnection,
    tx_id: &str,
) -> Result<Option<AuthorizedTransactionExport>, BridgeError> {
    use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

    let row = sequencer_withdrawals::table
        .filter(sequenced::authorized_transaction_name.eq(Some(tx_id.to_string())))
        .first::<SequencerWithdrawalStoredRow>(conn)
        .optional()
        .map_err(|err| {
            BridgeError::Runtime(format!("authorized transaction export query failed: {err}"))
        })?;
    let Some(row) = row else {
        return Ok(None);
    };
    let row = try_into_sequenced_withdrawal_view(row)?;
    let submitted_raw_tx_id = row.authorized_transaction_name.ok_or_else(|| {
        BridgeError::Runtime(format!(
            "withdrawal {:?} epoch {} is missing authorized transaction id for export",
            row.id, row.current_epoch
        ))
    })?;
    let transaction_jam = row.authorized_transaction_jam.ok_or_else(|| {
        BridgeError::Runtime(format!(
            "withdrawal {:?} epoch {} authorized transaction {} is missing transaction jam for export",
            row.id, row.current_epoch, submitted_raw_tx_id
        ))
    })?;
    Ok(Some(AuthorizedTransactionExport {
        submitted_raw_tx_id,
        transaction_jam,
    }))
}

/// Loads the append-only submission history in event-id order.
fn load_events(
    conn: &mut SqliteConnection,
) -> Result<Vec<WithdrawalSubmissionEventRecord>, BridgeError> {
    use crate::withdrawal::sequencer::schema::withdrawal_submission_events::dsl as events;

    let rows = withdrawal_submission_events::table
        .order(events::event_id.asc())
        .load::<WithdrawalSubmissionEventRow>(conn)
        .map_err(|err| BridgeError::Runtime(format!("withdrawal events load failed: {err}")))?;

    rows.into_iter()
        .map(try_into_submission_event_record)
        .collect()
}

/// Converts a raw SQLite event row into the typed debug record.
fn try_into_submission_event_record(
    row: WithdrawalSubmissionEventRow,
) -> Result<WithdrawalSubmissionEventRecord, BridgeError> {
    let id = WithdrawalId {
        as_of: tip5_from_bytes(&row.withdrawal_id_as_of)?,
        base_event_id: AtomBytes(row.withdrawal_id_base_event_id),
    };
    let epoch = u64::try_from(row.epoch)
        .map_err(|err| BridgeError::ValueConversion(format!("event epoch overflow: {err}")))?;
    let snapshot = match (row.snapshot_height, row.snapshot_block_id) {
        (Some(height), Some(block_id)) => Some(WithdrawalSnapshot {
            height: u64::try_from(height).map_err(|err| {
                BridgeError::ValueConversion(format!("snapshot height overflow: {err}"))
            })?,
            block_id: tip5_from_bytes(&block_id)?,
        }),
        (None, None) => None,
        _ => {
            return Err(BridgeError::Runtime(
                "event snapshot columns must be both null or both present".into(),
            ))
        }
    };
    let confirmed_height = row
        .confirmed_height
        .map(|height| {
            u64::try_from(height).map_err(|err| {
                BridgeError::ValueConversion(format!("confirmed height overflow: {err}"))
            })
        })
        .transpose()?;
    let confirmed_block_id = row
        .confirmed_block_id
        .as_ref()
        .map(|bytes| tip5_from_bytes(bytes))
        .transpose()?;
    let signer_node_id = row
        .signer_node_id
        .map(|node_id| {
            u64::try_from(node_id).map_err(|err| {
                BridgeError::ValueConversion(format!("event signer node id overflow: {err}"))
            })
        })
        .transpose()?;
    Ok(WithdrawalSubmissionEventRecord {
        event_id: row.event_id,
        created_at: row.created_at,
        id,
        epoch,
        proposal_hash: row.proposal_hash,
        transaction_name: row.transaction_name,
        event_type: WithdrawalSubmissionEventType::parse(&row.event_type)?,
        signer_node_id,
        commit_certificate: row.commit_certificate,
        snapshot,
        confirmed_height,
        confirmed_block_id,
    })
}

/// Converts a stored sequencer-withdrawal row into the typed view model.
fn try_into_sequenced_withdrawal_view(
    row: SequencerWithdrawalStoredRow,
) -> Result<SequencedWithdrawalView, BridgeError> {
    let request_facts = stored_request_facts(&row)?;
    Ok(SequencedWithdrawalView {
        id: WithdrawalId {
            as_of: tip5_from_bytes(&row.withdrawal_id_as_of)?,
            base_event_id: AtomBytes(row.withdrawal_id_base_event_id),
        },
        withdrawal_nonce: Some(u64::try_from(row.withdrawal_nonce).map_err(|err| {
            BridgeError::ValueConversion(format!("sequenced withdrawal nonce overflow: {err}"))
        })?),
        current_epoch: u64::try_from(row.current_epoch).map_err(|err| {
            BridgeError::ValueConversion(format!("sequenced epoch overflow: {err}"))
        })?,
        proposal_hash: row.proposal_hash,
        request_facts,
        canonical_amount: row
            .canonical_amount
            .map(|value| {
                u64::try_from(value).map_err(|err| {
                    BridgeError::ValueConversion(format!("canonical amount overflow: {err}"))
                })
            })
            .transpose()?,
        canonical_base_batch_end: row
            .canonical_base_batch_end
            .map(|value| {
                u64::try_from(value).map_err(|err| {
                    BridgeError::ValueConversion(format!(
                        "canonical base_batch_end overflow: {err}"
                    ))
                })
            })
            .transpose()?,
        canonical_transaction_jam: row.canonical_transaction_jam,
        canonical_selected_inputs: row
            .canonical_selected_inputs_jam
            .map(cue_selected_inputs)
            .transpose()?,
        canonical_snapshot: match (
            row.canonical_snapshot_height, row.canonical_snapshot_block_id,
        ) {
            (Some(height), Some(block_id)) => Some(WithdrawalSnapshot {
                height: u64::try_from(height).map_err(|err| {
                    BridgeError::ValueConversion(format!(
                        "canonical snapshot height overflow: {err}"
                    ))
                })?,
                block_id: tip5_from_bytes(&block_id)?,
            }),
            (None, None) => None,
            _ => {
                return Err(BridgeError::Runtime(
                    "canonical snapshot projection columns must be both null or both present"
                        .into(),
                ))
            }
        },
        peer_commit_certificate: row.peer_commit_certificate,
        authorized_transaction_name: row.authorized_transaction_name,
        authorized_transaction_jam: row.authorized_transaction_jam,
        authorized_raw_tx: row.authorized_raw_tx,
        handoff_index: u64::try_from(row.handoff_index).map_err(|err| {
            BridgeError::ValueConversion(format!("handoff index overflow: {err}"))
        })?,
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
        submit_attempt_count: u64::try_from(row.submit_attempt_count).map_err(|err| {
            BridgeError::ValueConversion(format!("submit attempt count overflow: {err}"))
        })?,
        last_submit_attempt_base_height: row
            .last_submit_attempt_base_height
            .map(|height| {
                u64::try_from(height).map_err(|err| {
                    BridgeError::ValueConversion(format!(
                        "last_submit_attempt_base_height overflow: {err}"
                    ))
                })
            })
            .transpose()?,
        last_submit_error: row.last_submit_error,
        state: WithdrawalState::parse(&row.state)?,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

/// Adds a column to an existing SQLite table if an older local schema does not
/// already contain it.
fn ensure_sqlite_column_exists(
    conn: &mut SqliteConnection,
    table_name: &str,
    column_name: &str,
    column_sql: &str,
) -> Result<(), BridgeError> {
    if sqlite_column_exists(conn, table_name, column_name)? {
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

/// Returns whether a SQLite table contains the named column.
fn sqlite_column_exists(
    conn: &mut SqliteConnection,
    table_name: &str,
    column_name: &str,
) -> Result<bool, BridgeError> {
    #[derive(diesel::QueryableByName)]
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

/// Returns whether a SQLite table exists in the current database.
fn sqlite_table_exists(conn: &mut SqliteConnection, table_name: &str) -> Result<bool, BridgeError> {
    #[derive(diesel::QueryableByName)]
    struct SqliteMasterCountRow {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        count: i64,
    }

    let row = diesel::sql_query(
        "SELECT COUNT(*) AS count FROM sqlite_master WHERE type = 'table' AND name = ?",
    )
    .bind::<diesel::sql_types::Text, _>(table_name)
    .get_result::<SqliteMasterCountRow>(conn)
    .map_err(|err| {
        BridgeError::Runtime(format!(
            "sqlite_master table lookup failed for {table_name}: {err}"
        ))
    })?;
    Ok(row.count > 0)
}

/// Renames a legacy SQLite table when the replacement table does not already
/// exist.
fn rename_sqlite_table_if_needed(
    conn: &mut SqliteConnection,
    old_name: &str,
    new_name: &str,
) -> Result<(), BridgeError> {
    if !sqlite_table_exists(conn, old_name)? || sqlite_table_exists(conn, new_name)? {
        return Ok(());
    }

    conn.batch_execute(&format!("ALTER TABLE {old_name} RENAME TO {new_name};"))
        .map_err(|err| {
            BridgeError::Runtime(format!(
                "failed to rename SQLite table {old_name} to {new_name}: {err}"
            ))
        })?;
    Ok(())
}

#[derive(Debug, Clone)]
struct StoredAuthorizedTransaction {
    submitted_raw_tx_id: String,
    transaction_jam: Vec<u8>,
    raw_tx_bytes: Vec<u8>,
}

/// Jams a transaction into the durable noun encoding stored in submission
/// history.
pub(crate) fn jam_transaction(
    transaction: &nockchain_types::v1::Transaction,
) -> Result<Vec<u8>, BridgeError> {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let noun = transaction.to_noun(&mut slab);
    slab.set_root(noun);
    Ok(slab.jam().to_vec())
}

/// Cues a jammed transaction payload back into the typed transaction wrapper.
pub(crate) fn cue_transaction(
    bytes: Vec<u8>,
) -> Result<nockchain_types::v1::Transaction, BridgeError> {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let noun = slab
        .cue_into(Bytes::from(bytes))
        .map_err(|err| BridgeError::Runtime(format!("failed to cue transaction jam: {err}")))?;
    let space = slab.noun_space();
    nockchain_types::v1::Transaction::from_noun(&noun, &space)
        .map_err(|err| BridgeError::Runtime(format!("failed to decode transaction noun: {err}")))
}

fn jam_selected_inputs(inputs: &[nockchain_types::v1::Name]) -> Result<Vec<u8>, BridgeError> {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let noun = normalized_note_names(inputs).to_noun(&mut slab);
    slab.set_root(noun);
    Ok(slab.jam().to_vec())
}

fn cue_selected_inputs(bytes: Vec<u8>) -> Result<Vec<nockchain_types::v1::Name>, BridgeError> {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let noun = slab
        .cue_into(Bytes::from(bytes))
        .map_err(|err| BridgeError::Runtime(format!("failed to cue selected inputs jam: {err}")))?;
    let space = slab.noun_space();
    Vec::<nockchain_types::v1::Name>::from_noun(&noun, &space)
        .map(|inputs| normalized_note_names(&inputs))
        .map_err(|err| {
            BridgeError::Runtime(format!("failed to decode selected inputs noun: {err}"))
        })
}

fn stored_authorized_transaction(
    transaction: &nockchain_types::v1::Transaction,
) -> Result<StoredAuthorizedTransaction, BridgeError> {
    let raw_tx = withdrawal_raw_tx::persisted_raw_tx_from_transaction(transaction)?;
    Ok(StoredAuthorizedTransaction {
        submitted_raw_tx_id: raw_tx.tx_id_base58,
        transaction_jam: jam_transaction(transaction)?,
        raw_tx_bytes: raw_tx.raw_tx_bytes,
    })
}

/// Encodes a withdrawal commit certificate into the protobuf bytes stored in
/// SQLite history and sequencer current-state rows.
fn encode_commit_certificate(
    certificate: &WithdrawalCommitCertificate,
) -> Result<Vec<u8>, BridgeError> {
    Ok(certificate.encode_to_vec())
}

/// Returns SQLite's last inserted row id for the current connection.
fn last_insert_rowid(conn: &mut SqliteConnection) -> Result<i64, BridgeError> {
    let row = diesel::sql_query("SELECT last_insert_rowid() AS value")
        .get_result::<LastInsertRowId>(conn)
        .map_err(|err| BridgeError::Runtime(format!("last_insert_rowid failed: {err}")))?;
    Ok(row.value)
}

/// Returns the current Unix time in seconds, checked for i64 overflow.
fn now_unix_secs() -> Result<i64, BridgeError> {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .map_err(|err| BridgeError::ValueConversion(format!("unix seconds overflow: {err}")))
}

/// Converts a Tip5 hash into the byte encoding stored in SQLite.
fn tip5_to_bytes(hash: &Tip5Hash) -> Vec<u8> {
    hash.to_be_limb_bytes().to_vec()
}

/// Converts a stored byte slice back into a Tip5 hash.
fn tip5_from_bytes(bytes: &[u8]) -> Result<Tip5Hash, BridgeError> {
    Tip5Hash::from_be_limb_bytes(bytes)
        .map_err(|err| BridgeError::Runtime(format!("failed to decode Tip5Hash: {err}")))
}

/// Builds the SQLite pool used by the withdrawal state store.
fn sqlite_pool(path: &Path) -> Result<Pool, BridgeError> {
    let path_str = path.to_string_lossy();
    let manager = Manager::new(path_str.to_string(), Runtime::Tokio1);
    Pool::builder(manager).build().map_err(|err| {
        BridgeError::Runtime(format!("withdrawal state store pool build failed: {err}"))
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use nockchain_math::belt::Belt;
    use tempfile::tempdir;

    use super::*;
    use crate::withdrawal::sequencer::journal::{
        r2_test_support as r2, verify_remote_journal, ObjectStoreSequencerJournal,
        RecordingSequencerJournal,
    };
    use crate::withdrawal::transport::withdrawal_id_to_proto;

    fn sample_base_event_id(start: u8) -> AtomBytes {
        AtomBytes((0..32).map(|offset| start.wrapping_add(offset)).collect())
    }

    fn fixture_transaction() -> nockchain_types::v1::Transaction {
        const TRANSACTION_JAM: &[u8] = include_bytes!(
            "../../../test-fixtures/transactions/9MpGym52AumtwyBxYPyVsWHvcamUYwZkc1Nq7w3cFGF28u8ceVDwt3e.tx"
        );

        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = slab
            .cue_into(TRANSACTION_JAM.to_vec().into())
            .expect("cue transaction fixture");
        let space = nockapp::NounAllocator::noun_space(&slab);
        nockchain_types::v1::Transaction::from_noun(&noun, &space)
            .expect("decode transaction fixture")
    }

    fn shifted_hash(hash: &Tip5Hash, delta: u64) -> Tip5Hash {
        Tip5Hash(hash.0.map(|belt| Belt(belt.0.wrapping_add(delta))))
    }

    fn shifted_name(name: &nockchain_types::v1::Name, delta: u64) -> nockchain_types::v1::Name {
        nockchain_types::v1::Name::new(
            shifted_hash(&name.first, delta),
            shifted_hash(&name.last, delta.wrapping_add(10_000)),
        )
    }

    fn sample_transaction(seed: u64) -> nockchain_types::v1::Transaction {
        let mut transaction = fixture_transaction();
        let nockchain_types::v1::Transaction::V1(tx) = &mut transaction;
        let delta = seed.saturating_mul(1_000);
        tx.name = format!("{}-{seed}", tx.name);
        for (name, _) in &mut tx.spends.0 {
            *name = shifted_name(name, delta);
        }
        match &mut tx.metadata.inputs {
            nockchain_types::v1::InputMetadata::LegacySignatures(map) => {
                for (name, _) in &mut map.0 {
                    *name = shifted_name(name, delta);
                }
            }
            nockchain_types::v1::InputMetadata::SpendConditions(map) => {
                for (name, _) in &mut map.0 {
                    *name = shifted_name(name, delta);
                }
            }
        }
        match &mut tx.witness_data {
            nockchain_types::v1::WitnessData::Signatures(map) => {
                for (name, _) in &mut map.0 {
                    *name = shifted_name(name, delta);
                }
            }
            nockchain_types::v1::WitnessData::Witnesses(map) => {
                for (name, _) in &mut map.0 {
                    *name = shifted_name(name, delta);
                }
            }
        }
        transaction
    }

    fn sample_withdrawal_id(seed: u64) -> WithdrawalId {
        WithdrawalId {
            as_of: Tip5Hash([
                Belt(seed + 1),
                Belt(seed + 2),
                Belt(seed + 3),
                Belt(seed + 4),
                Belt(seed + 5),
            ]),
            base_event_id: sample_base_event_id(seed as u8),
        }
    }

    fn sample_name(seed: u64) -> nockchain_types::v1::Name {
        nockchain_types::v1::Name::new(
            Tip5Hash([
                Belt(seed + 101),
                Belt(seed + 102),
                Belt(seed + 103),
                Belt(seed + 104),
                Belt(seed + 105),
            ]),
            Tip5Hash([
                Belt(seed + 201),
                Belt(seed + 202),
                Belt(seed + 203),
                Belt(seed + 204),
                Belt(seed + 205),
            ]),
        )
    }

    fn blob_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn sample_proposal(seed: u64, epoch: u64) -> WithdrawalProposalData {
        let transaction = sample_transaction(seed);
        WithdrawalProposalData {
            id: sample_withdrawal_id(seed),
            recipient: Tip5Hash([
                Belt(seed + 301),
                Belt(seed + 302),
                Belt(seed + 303),
                Belt(seed + 304),
                Belt(seed + 305),
            ]),
            amount: 9_000 + seed,
            burned_amount: 10_000 + seed,
            base_batch_end: 80 + seed,
            epoch,
            snapshot: WithdrawalSnapshot {
                height: 500 + epoch,
                block_id: Tip5Hash([
                    Belt(seed + 401),
                    Belt(seed + 402),
                    Belt(seed + 403),
                    Belt(seed + 404),
                    Belt(seed + 405),
                ]),
            },
            selected_inputs: transaction.normalized_input_names(),
            transaction,
        }
    }

    fn tracked_from_proposal(
        proposal: &WithdrawalProposalData,
        withdrawal_nonce: u64,
    ) -> TrackedWithdrawalRequest {
        TrackedWithdrawalRequest {
            id: proposal.id.clone(),
            recipient: proposal.recipient.clone(),
            amount: proposal.burned_amount,
            base_batch_end: proposal.base_batch_end,
            withdrawal_nonce,
        }
    }

    #[test]
    fn proposal_hash_is_stable_for_same_value() {
        let proposal = sample_proposal(99, 3);
        let hash_a = proposal.proposal_hash().expect("first proposal hash");
        let hash_b = proposal.proposal_hash().expect("second proposal hash");
        assert_eq!(hash_a, hash_b);
    }

    #[tokio::test]
    async fn canonicalization_rejects_transaction_selected_input_mismatch() {
        let (_dir, service) = open_service().await;
        let mut proposal = sample_proposal(29, 0);
        proposal.selected_inputs = vec![sample_name(88_888)];

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        let err = service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect_err("mismatched transaction inputs should fail");
        assert!(matches!(
            err,
            WithdrawalSequencerStoreError::Store(message)
            if message.contains("transaction inputs do not match proposal selected_inputs")
        ));
    }

    #[tokio::test]
    async fn canonicalization_rejects_conflicting_hash_for_existing_withdrawal() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(61, 0);
        let proposal_hash = proposal.proposal_hash().expect("proposal hash");
        let mut conflicting = sample_proposal(62, 0);
        conflicting.id = proposal.id.clone();
        conflicting.recipient = proposal.recipient.clone();
        conflicting.burned_amount = proposal.burned_amount;
        conflicting.base_batch_end = proposal.base_batch_end;
        conflicting.epoch = proposal.epoch;
        let conflicting_hash = conflicting
            .proposal_hash()
            .expect("conflicting proposal hash");
        assert_ne!(conflicting_hash, proposal_hash);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonical proposal");
        service
            .record_proposal_authorized(&proposal)
            .await
            .expect("record authorized proposal");
        let before = service
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch authorized withdrawal")
            .expect("authorized withdrawal exists");

        let err = service
            .record_proposal_canonicalized(&conflicting, 200)
            .await
            .expect_err("conflicting canonical proposal should fail");
        assert!(matches!(
            err,
            WithdrawalSequencerStoreError::Store(ref message)
                if message.contains("already has canonical proposal hash")
        ));

        let after = service
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch withdrawal after conflict")
            .expect("withdrawal remains sequenced");
        assert_eq!(after.state, WithdrawalState::Authorized);
        assert_eq!(after.proposal_hash.as_deref(), Some(proposal_hash.as_str()));
        assert_eq!(
            after.authorized_transaction_name,
            before.authorized_transaction_name
        );
        assert_eq!(
            service
                .reserved_input_names_for(&proposal.id)
                .await
                .expect("load reservations after conflict"),
            proposal.selected_inputs
        );
    }

    #[test]
    fn proposer_turn_expired_event_type_roundtrips() {
        assert_eq!(
            WithdrawalSubmissionEventType::parse("proposer_turn_expired")
                .expect("parse proposer_turn_expired"),
            WithdrawalSubmissionEventType::ProposerTurnExpired
        );
    }

    #[tokio::test]
    async fn durable_journal_receives_lifecycle_event() {
        let journal = RecordingSequencerJournal::default();
        let (_dir, service) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(30, 0);
        let proposal_hash = proposal.proposal_hash().expect("proposal hash");

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonical proposal");

        let records = journal.records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].journal_id, "recording");
        assert_eq!(records[0].sequence, 1);
        assert_eq!(records[0].previous_event_id, GENESIS_EVENT_ID);
        assert_eq!(records[1].journal_id, "recording");
        assert_eq!(records[1].sequence, 2);
        assert_eq!(records[1].previous_event_id, records[0].event_id);
        assert_eq!(
            records[0].event_type,
            SequencerJournalEventType::WithdrawalOrdered
        );
        assert_eq!(
            records[1].event_type,
            SequencerJournalEventType::ProposalCanonicalized
        );
        assert_eq!(
            records[1]
                .proposal
                .as_ref()
                .expect("proposal context")
                .proposal_hash,
            proposal_hash
        );
        assert_eq!(
            records[1]
                .nockchain
                .as_ref()
                .expect("nockchain context")
                .snapshot_height,
            Some(500)
        );

        let cursor = load_journal_cursor_for(&service, "recording")
            .await
            .expect("journal cursor exists");
        assert_eq!(cursor.last_sequence, 2);
        assert_eq!(cursor.last_event_id, records[1].event_id);
    }

    #[tokio::test]
    async fn durable_journal_authorization_records_submitted_raw_tx() {
        let journal = RecordingSequencerJournal::default();
        let (_dir, service) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(32, 0);
        let expected_raw_tx =
            withdrawal_raw_tx::persisted_raw_tx_from_transaction(&proposal.transaction)
                .expect("expected raw tx");

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonical proposal");
        service
            .record_proposal_authorized(&proposal)
            .await
            .expect("record authorized proposal");

        let expected_raw_tx_hex = hex::encode(expected_raw_tx.raw_tx_bytes);
        let records = journal.records();
        assert_eq!(records.len(), 3);
        assert_eq!(
            records[2].event_type,
            SequencerJournalEventType::ProposalAuthorized
        );
        assert_eq!(
            records[2]
                .proposal
                .as_ref()
                .expect("proposal context")
                .transaction_name
                .as_deref(),
            Some(expected_raw_tx.tx_id_base58.as_str())
        );
        assert_eq!(
            records[2]
                .submission
                .as_ref()
                .expect("submission context")
                .submitted_raw_tx_id
                .as_deref(),
            Some(expected_raw_tx.tx_id_base58.as_str())
        );
        assert_eq!(
            records[2]
                .submission
                .as_ref()
                .expect("submission context")
                .authorized_raw_tx
                .as_deref(),
            Some(expected_raw_tx_hex.as_str())
        );
    }

    #[tokio::test]
    async fn journal_projector_replay_rebuilds_authorized_state() {
        let journal = RecordingSequencerJournal::default();
        let (_source_dir, source) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(36, 0);
        let proposal_hash = proposal.proposal_hash().expect("proposal hash");
        let expected_authorized =
            stored_authorized_transaction(&proposal.transaction).expect("authorized artifacts");

        source
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        source
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonical proposal");
        source
            .record_proposal_authorized(&proposal)
            .await
            .expect("record authorized proposal");

        let records = journal.records();
        let (_replay_dir, replay) = open_service().await;
        for record in records.clone() {
            replay
                .with_conn(move |conn| {
                    apply_journal_event(conn, &record, SequencerJournalApplyMode::Replay)
                })
                .await
                .expect("replay journal event");
        }

        let id = proposal.id.clone();
        let replayed = replay
            .with_conn(move |conn| {
                fetch_sequenced_withdrawal(conn, &id.base_event_id)?
                    .ok_or_else(|| BridgeError::Runtime("missing replayed row".to_string()))
            })
            .await
            .expect("fetch replayed row");
        assert_eq!(replayed.state, WithdrawalState::Authorized);
        assert_eq!(replayed.proposal_hash, Some(proposal_hash));
        assert_eq!(
            replayed.authorized_transaction_name,
            Some(expected_authorized.submitted_raw_tx_id)
        );
        assert_eq!(
            replayed.authorized_transaction_jam,
            Some(expected_authorized.transaction_jam)
        );
        assert_eq!(
            replayed.authorized_raw_tx,
            Some(expected_authorized.raw_tx_bytes)
        );

        let id = proposal.id.clone();
        let reserved = replay
            .with_conn(move |conn| load_reserved_input_rows_for_withdrawal(conn, &id))
            .await
            .expect("load replayed reserved inputs");
        assert_eq!(
            reserved.len(),
            normalized_note_names(&proposal.selected_inputs).len()
        );
    }

    #[tokio::test]
    async fn journal_projector_replay_uses_base_event_identity_when_ordered_as_of_differs() {
        let journal = RecordingSequencerJournal::default();
        let (_source_dir, source) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(136, 0);
        let mut tracked = tracked_from_proposal(&proposal, 1);
        tracked.id.as_of = sample_withdrawal_id(137).as_of;

        source
            .ensure_tracked_withdrawal_ordering(&tracked)
            .await
            .expect("record ordering with non-canonical as_of");
        source
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonical proposal");

        let records = journal.records();
        assert_eq!(
            records
                .iter()
                .map(|record| record.event_type)
                .collect::<Vec<_>>(),
            vec![
                SequencerJournalEventType::WithdrawalOrdered,
                SequencerJournalEventType::ProposalCanonicalized,
            ]
        );
        assert_ne!(records[0].withdrawal.as_of, records[1].withdrawal.as_of);
        assert_eq!(
            records[0].withdrawal.base_event_id,
            records[1].withdrawal.base_event_id
        );

        let (_replay_dir, replay) = open_service().await;
        replay_records(&replay, &records)
            .await
            .expect("replay as_of correction by base event id");
        let replayed = replay
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch replayed withdrawal")
            .expect("replayed withdrawal exists");
        assert_eq!(replayed.id, proposal.id);
        assert_eq!(replayed.state, WithdrawalState::PeerCanonical);
    }

    #[tokio::test]
    async fn journal_projector_replay_applies_every_durable_event_type() {
        let (records, proposal, _confirmed_block_id) =
            full_submission_lifecycle_journal_records(37).await;
        assert_eq!(
            records
                .iter()
                .map(|record| record.event_type)
                .collect::<Vec<_>>(),
            vec![
                SequencerJournalEventType::WithdrawalOrdered,
                SequencerJournalEventType::ProposalCanonicalized,
                SequencerJournalEventType::ProposalAuthorized,
                SequencerJournalEventType::TxSubmitted,
                SequencerJournalEventType::TxSubmitted,
                SequencerJournalEventType::TxSeenMempoolAccepted,
                SequencerJournalEventType::MempoolRetryAttempted,
                SequencerJournalEventType::TxConfirmed,
            ]
        );
        let expected_authorized =
            stored_authorized_transaction(&proposal.transaction).expect("authorized artifacts");

        let (_replay_dir, replay) = open_service().await;

        replay_record(&replay, records[0].clone())
            .await
            .expect("replay withdrawal_ordered");
        let sequenced = replay
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch pending withdrawal")
            .expect("pending withdrawal exists");
        assert_eq!(sequenced.withdrawal_nonce, Some(1));
        assert_eq!(sequenced.state, WithdrawalState::Pending);

        replay_record(&replay, records[1].clone())
            .await
            .expect("replay proposal_canonicalized");
        let sequenced = replay
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch peer-canonical withdrawal")
            .expect("peer-canonical withdrawal exists");
        assert_eq!(sequenced.state, WithdrawalState::PeerCanonical);
        assert_eq!(
            replay
                .reserved_input_names_for(&proposal.id)
                .await
                .expect("load reserved inputs after canonical replay"),
            proposal.selected_inputs
        );

        replay_record(&replay, records[2].clone())
            .await
            .expect("replay proposal_authorized");
        let sequenced = replay
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch authorized withdrawal")
            .expect("authorized withdrawal exists");
        assert_eq!(sequenced.state, WithdrawalState::Authorized);
        assert_eq!(
            sequenced.authorized_transaction_name,
            Some(expected_authorized.submitted_raw_tx_id.clone())
        );
        let stored = load_stored_row(&replay, &proposal.id).await;
        assert_eq!(
            stored.authorized_transaction_jam,
            Some(expected_authorized.transaction_jam.clone())
        );
        assert_eq!(
            stored.authorized_raw_tx,
            Some(expected_authorized.raw_tx_bytes.clone())
        );

        replay_record(&replay, records[3].clone())
            .await
            .expect("replay failed tx_submitted");
        let sequenced = replay
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch authorized withdrawal after failed submit")
            .expect("authorized withdrawal exists after failed submit");
        assert_eq!(sequenced.state, WithdrawalState::Authorized);
        assert_eq!(sequenced.submit_attempt_count, 2);
        assert_eq!(sequenced.last_submit_attempt_base_height, Some(120));
        assert_eq!(
            sequenced.last_submit_error.as_deref(),
            Some("submit failed")
        );

        replay_record(&replay, records[4].clone())
            .await
            .expect("replay successful tx_submitted");
        let sequenced = replay
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch authorized withdrawal after successful submit")
            .expect("authorized withdrawal exists after successful submit");
        assert_eq!(sequenced.state, WithdrawalState::Authorized);
        assert_eq!(sequenced.submit_attempt_count, 3);
        assert_eq!(sequenced.last_submit_attempt_base_height, Some(140));
        assert_eq!(sequenced.last_submit_error, None);

        replay_record(&replay, records[5].clone())
            .await
            .expect("replay tx_seen_mempool_accepted");
        let sequenced = replay
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch mempool-accepted withdrawal")
            .expect("mempool-accepted withdrawal exists");
        assert_eq!(sequenced.state, WithdrawalState::MempoolAccepted);
        assert_eq!(sequenced.submit_attempt_count, 3);
        assert_eq!(sequenced.last_submit_attempt_base_height, Some(140));

        replay_record(&replay, records[6].clone())
            .await
            .expect("replay mempool_retry_attempted");
        let sequenced = replay
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch retried withdrawal")
            .expect("retried withdrawal exists");
        assert_eq!(sequenced.state, WithdrawalState::MempoolAccepted);
        assert_eq!(sequenced.submit_attempt_count, 4);
        assert_eq!(sequenced.last_submit_attempt_base_height, Some(160));
        assert_eq!(sequenced.last_submit_error.as_deref(), Some("orphan retry"));

        replay_record(&replay, records[7].clone())
            .await
            .expect("replay tx_confirmed");
        let sequenced = replay
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch confirmed withdrawal")
            .expect("confirmed withdrawal exists");
        assert_eq!(sequenced.state, WithdrawalState::Confirmed);
        assert_eq!(
            sequenced.authorized_transaction_name,
            Some(expected_authorized.submitted_raw_tx_id)
        );
        assert!(replay
            .reserved_input_names_for(&proposal.id)
            .await
            .expect("load reserved inputs after confirmation replay")
            .is_empty());
    }

    #[tokio::test]
    async fn journal_projector_replay_rejects_full_stream_replay_over_projection() {
        let (records, proposal, _confirmed_block_id) =
            full_submission_lifecycle_journal_records(38).await;
        let (_replay_dir, replay) = open_service().await;

        replay_records(&replay, &records)
            .await
            .expect("first full replay");
        let err = replay_records(&replay, &records)
            .await
            .expect_err("second full replay should fail under exact-frontier replay");
        assert!(err.to_string().contains("ahead of event state"));

        let sequenced = replay
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch replayed withdrawal")
            .expect("replayed withdrawal exists");
        assert_eq!(sequenced.state, WithdrawalState::Confirmed);
        assert_eq!(sequenced.submit_attempt_count, 4);
        assert_eq!(sequenced.last_submit_attempt_base_height, Some(160));
        assert_eq!(sequenced.last_submit_error.as_deref(), Some("orphan retry"));
        assert!(replay
            .reserved_input_names_for(&proposal.id)
            .await
            .expect("load reserved inputs after exact-frontier replay")
            .is_empty());
    }

    #[tokio::test]
    async fn journal_projector_replay_rejects_stale_event_with_mismatched_hash() {
        let (records_a, proposal_a, _confirmed_block_id) =
            full_submission_lifecycle_journal_records(39).await;
        let mut proposal_b = sample_proposal(40, 0);
        proposal_b.id = proposal_a.id.clone();
        let journal_b = RecordingSequencerJournal::default();
        let (_source_dir_b, source_b) = open_service_with_journal(journal_b.handle()).await;
        source_b
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal_b, 1))
            .await
            .expect("record ordering for mismatched proposal");
        source_b
            .record_proposal_canonicalized(&proposal_b, 100)
            .await
            .expect("record mismatched canonical proposal");
        let mismatched_canonicalized = journal_b.records()[1].clone();

        let (_replay_dir, replay) = open_service().await;
        replay_records(&replay, &records_a[..3])
            .await
            .expect("replay proposal A through authorization");

        let err = replay_record(&replay, mismatched_canonicalized)
            .await
            .expect_err("stale mismatched event should fail closed");
        assert!(err.to_string().contains("ahead of event state"));
    }

    #[tokio::test]
    async fn journal_projector_replay_rejects_cross_withdrawal_rank_comparison() {
        let (_dir, service) = open_service().await;
        let proposal_a = sample_proposal(41, 0);
        let proposal_b = sample_proposal(42, 0);

        authorize_with_handoffs(&service, &proposal_a).await;
        let proposal_a_id = proposal_a.id.clone();
        let existing = service
            .with_conn(move |conn| {
                fetch_sequenced_withdrawal(conn, &proposal_a_id.base_event_id)?
                    .ok_or_else(|| BridgeError::Runtime("missing authorized row".to_string()))
            })
            .await
            .expect("fetch internal authorized withdrawal");
        let err = ensure_replay_not_past_target_state(
            SequencerJournalApplyMode::Replay,
            &proposal_b.id,
            &existing,
            WithdrawalState::PeerCanonical,
            existing.proposal_hash.as_deref(),
        )
        .expect_err("cross-withdrawal rank comparison should fail");
        assert!(err.to_string().contains("tried to compare state"));
    }

    #[tokio::test]
    async fn journal_projector_replay_rejects_mismatched_ordering_nonce() {
        let (_dir, replay) = open_service().await;
        let proposal = sample_proposal(43, 0);
        let request_facts = SequencerWithdrawalRequestFacts::from_proposal(&proposal);
        let first_ordering = sequencer_journal_record_with_request_facts(
            1,
            SequencerJournalEventType::WithdrawalOrdered,
            &proposal.id,
            0,
            Some(1),
            Some(&request_facts),
            None,
            None,
            None,
            None,
            None,
        )
        .expect("first ordering record");
        let mismatched_ordering = sequencer_journal_record_with_request_facts(
            2,
            SequencerJournalEventType::WithdrawalOrdered,
            &proposal.id,
            0,
            Some(2),
            Some(&request_facts),
            None,
            None,
            None,
            None,
            None,
        )
        .expect("mismatched ordering record");

        replay_record(&replay, first_ordering)
            .await
            .expect("replay first ordering");
        let err = replay_record(&replay, mismatched_ordering)
            .await
            .expect_err("mismatched ordering nonce should fail");
        assert!(err.to_string().contains("sequencer journal nonce mismatch"));
    }

    #[tokio::test]
    async fn journal_projector_replay_rejects_mismatched_ordering_turn_start() {
        let (_dir, replay) = open_service().await;
        let proposal = sample_proposal(44, 0);
        let request_facts = SequencerWithdrawalRequestFacts::from_proposal(&proposal);
        let first_ordering = sequencer_journal_record_with_request_facts(
            1,
            SequencerJournalEventType::WithdrawalOrdered,
            &proposal.id,
            0,
            Some(1),
            Some(&request_facts),
            journal_base_context(None, Some(100), None),
            None,
            None,
            None,
            None,
        )
        .expect("first ordering record");
        let mismatched_ordering = sequencer_journal_record_with_request_facts(
            2,
            SequencerJournalEventType::WithdrawalOrdered,
            &proposal.id,
            0,
            Some(1),
            Some(&request_facts),
            journal_base_context(None, Some(200), None),
            None,
            None,
            None,
            None,
        )
        .expect("mismatched turn-start ordering record");

        replay_record(&replay, first_ordering)
            .await
            .expect("replay first ordering");
        let err = replay_record(&replay, mismatched_ordering)
            .await
            .expect_err("mismatched ordering turn start should fail");
        assert!(err
            .to_string()
            .contains("turn_started_base_height does not match existing projection"));
    }

    #[tokio::test]
    async fn durable_journal_skips_proposal_signed_debug_event() {
        let journal = RecordingSequencerJournal::default();
        let (_dir, service) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(33, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonical proposal");

        let records_before = journal.records();
        let cursor_before = load_journal_cursor_for(&service, "recording")
            .await
            .expect("journal cursor exists before debug-only event");
        journal.set_fail(true);
        service
            .record_proposal_signed(&proposal, 4, 125)
            .await
            .expect("proposal_signed should remain local debug history");
        assert_eq!(journal.records(), records_before);
        let cursor_after = load_journal_cursor_for(&service, "recording")
            .await
            .expect("journal cursor exists after debug-only event");
        assert_eq!(cursor_after.last_sequence, cursor_before.last_sequence);
        assert_eq!(cursor_after.last_event_id, cursor_before.last_event_id);

        let events = service
            .list_submission_events()
            .await
            .expect("list debug events after signed proposal");
        assert_eq!(
            events
                .last()
                .expect("proposal signed debug event")
                .event_type,
            WithdrawalSubmissionEventType::ProposalSigned
        );
    }

    #[tokio::test]
    async fn journal_startup_recovery_passes_with_empty_remote_and_genesis_cursor() {
        let journal = RecordingSequencerJournal::default();
        let (_dir, service) = open_service_with_journal(journal.handle()).await;

        let recovery = service
            .recover_from_journal_on_startup()
            .await
            .expect("empty remote startup journal recovery")
            .expect("journal enabled");

        assert_eq!(
            recovery,
            SequencerJournalRecoveryReport {
                journal_id: "recording".to_string(),
                start_sequence: 0,
                start_event_id: GENESIS_EVENT_ID.to_string(),
                last_sequence: 0,
                last_event_id: GENESIS_EVENT_ID.to_string(),
                replayed_count: 0,
                max_replayed_base_height: None,
                max_replayed_nockchain_height: None,
            }
        );
    }

    #[tokio::test]
    async fn journal_startup_recovery_passes_when_cursor_matches_remote() {
        let journal = RecordingSequencerJournal::default();
        let (_dir, service) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(34, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonical proposal");

        let recovery = service
            .recover_from_journal_on_startup()
            .await
            .expect("startup journal recovery")
            .expect("journal enabled");
        let records = journal.records();
        assert_eq!(
            recovery,
            SequencerJournalRecoveryReport {
                journal_id: "recording".to_string(),
                start_sequence: 2,
                start_event_id: records[1].event_id.clone(),
                last_sequence: 2,
                last_event_id: records[1].event_id.clone(),
                replayed_count: 0,
                max_replayed_base_height: None,
                max_replayed_nockchain_height: None,
            }
        );
    }

    #[tokio::test]
    async fn journal_startup_recovery_fails_when_cursor_event_was_not_projected() {
        let journal = RecordingSequencerJournal::default();
        let (_source_dir, source) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(51, 0);

        source
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        source
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonical proposal");
        let records = journal.records();
        let ordered = records[0].clone();
        let canonicalized = records[1].clone();

        let (_replay_dir, replay) = open_service_with_journal(journal.handle()).await;
        replay
            .with_conn(move |conn| {
                // Simulate a corrupted local DB: the first event is projected,
                // but the cursor falsely claims the second event was also
                // applied. Recovery must not trust cursor/hash continuity alone.
                apply_journal_event(conn, &ordered, SequencerJournalApplyMode::Replay)?;
                upsert_journal_cursor(
                    conn, "recording", canonicalized.sequence, &canonicalized.event_id,
                    canonicalized.created_at_unix_ms,
                )
            })
            .await
            .expect("seed mismatched projection/cursor");

        let err = replay
            .recover_from_journal_on_startup()
            .await
            .expect_err("cursor event missing from projection should fail closed");

        assert!(err
            .to_string()
            .contains("is not reflected in SQLite projection"));
        assert!(err
            .to_string()
            .contains("state pending does not match cursor state peer_canonical"));
    }

    #[tokio::test]
    async fn journal_startup_recovery_fails_when_cursor_artifacts_are_missing() {
        let journal = RecordingSequencerJournal::default();
        let (_dir, service) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(52, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonical proposal");
        service
            .record_proposal_authorized(&proposal)
            .await
            .expect("record authorized proposal");

        service
            .with_conn(move |conn| {
                use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

                diesel::update(
                    sequencer_withdrawals::table
                        .filter(
                            sequenced::withdrawal_id_as_of.eq(tip5_to_bytes(&proposal.id.as_of)),
                        )
                        .filter(
                            sequenced::withdrawal_id_base_event_id.eq(proposal
                                .id
                                .base_event_id
                                .0
                                .clone()),
                        ),
                )
                .set(sequenced::authorized_raw_tx.eq(Option::<Vec<u8>>::None))
                .execute(conn)
                .map_err(|err| {
                    BridgeError::Runtime(format!("failed to corrupt authorized_raw_tx: {err}"))
                })?;
                Ok::<(), BridgeError>(())
            })
            .await
            .expect("corrupt authorized raw tx");

        let err = service
            .recover_from_journal_on_startup()
            .await
            .expect_err("cursor artifact mismatch should fail closed");

        assert!(err
            .to_string()
            .contains("authorized raw tx mismatch for cursor event"));
    }

    #[tokio::test]
    async fn journal_startup_recovery_fails_when_cursor_raw_tx_bit_is_flipped() {
        let journal = RecordingSequencerJournal::default();
        let (_dir, service) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(60, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonical proposal");
        service
            .record_proposal_authorized(&proposal)
            .await
            .expect("record authorized proposal");
        service
            .with_conn(move |conn| {
                use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

                let mut row = fetch_sequenced_withdrawal(conn, &proposal.id.base_event_id)?
                    .ok_or_else(|| BridgeError::Runtime("missing authorized row".to_string()))?;
                let raw_tx = row.authorized_raw_tx.as_mut().ok_or_else(|| {
                    BridgeError::Runtime("missing authorized raw tx before bit flip".to_string())
                })?;
                let first_byte = raw_tx.first_mut().ok_or_else(|| {
                    BridgeError::Runtime("authorized raw tx is empty before bit flip".to_string())
                })?;
                *first_byte ^= 0x01;

                diesel::update(
                    sequencer_withdrawals::table
                        .filter(
                            sequenced::withdrawal_id_as_of.eq(tip5_to_bytes(&proposal.id.as_of)),
                        )
                        .filter(
                            sequenced::withdrawal_id_base_event_id.eq(proposal
                                .id
                                .base_event_id
                                .0
                                .clone()),
                        ),
                )
                .set(sequenced::authorized_raw_tx.eq(row.authorized_raw_tx))
                .execute(conn)
                .map_err(|err| {
                    BridgeError::Runtime(format!("failed to corrupt authorized_raw_tx bit: {err}"))
                })?;
                Ok::<(), BridgeError>(())
            })
            .await
            .expect("flip one raw tx bit");

        let err = service
            .recover_from_journal_on_startup()
            .await
            .expect_err("raw tx bit flip should fail closed");

        assert!(err
            .to_string()
            .contains("authorized raw tx mismatch for cursor event"));
    }

    #[tokio::test]
    async fn journal_startup_recovery_fails_when_cursor_nonce_is_malleated() {
        let journal = RecordingSequencerJournal::default();
        let (_dir, service) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(53, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .with_conn(move |conn| {
                use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

                diesel::update(
                    sequencer_withdrawals::table
                        .filter(
                            sequenced::withdrawal_id_as_of.eq(tip5_to_bytes(&proposal.id.as_of)),
                        )
                        .filter(
                            sequenced::withdrawal_id_base_event_id.eq(proposal
                                .id
                                .base_event_id
                                .0
                                .clone()),
                        ),
                )
                .set(sequenced::withdrawal_nonce.eq(2_i64))
                .execute(conn)
                .map_err(|err| {
                    BridgeError::Runtime(format!("failed to corrupt withdrawal_nonce: {err}"))
                })?;
                Ok::<(), BridgeError>(())
            })
            .await
            .expect("corrupt nonce");

        let err = service
            .recover_from_journal_on_startup()
            .await
            .expect_err("nonce mismatch should fail closed");

        assert!(err.to_string().contains("sequencer journal nonce mismatch"));
    }

    #[tokio::test]
    async fn journal_startup_recovery_fails_when_cursor_target_row_is_malleated() {
        let journal = RecordingSequencerJournal::default();
        let (_dir, service) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(59, 0);
        let replacement_id = sample_withdrawal_id(590);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .with_conn(move |conn| {
                use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

                diesel::update(
                    sequencer_withdrawals::table
                        .filter(
                            sequenced::withdrawal_id_as_of.eq(tip5_to_bytes(&proposal.id.as_of)),
                        )
                        .filter(
                            sequenced::withdrawal_id_base_event_id.eq(proposal
                                .id
                                .base_event_id
                                .0
                                .clone()),
                        ),
                )
                .set((
                    sequenced::withdrawal_id_as_of.eq(tip5_to_bytes(&replacement_id.as_of)),
                    sequenced::withdrawal_id_base_event_id
                        .eq(replacement_id.base_event_id.0.clone()),
                ))
                .execute(conn)
                .map_err(|err| {
                    BridgeError::Runtime(format!("failed to corrupt withdrawal row id: {err}"))
                })?;
                Ok::<(), BridgeError>(())
            })
            .await
            .expect("corrupt target row id");

        let err = service
            .recover_from_journal_on_startup()
            .await
            .expect_err("target row identity mismatch should fail closed");

        assert!(err
            .to_string()
            .contains("missing sequenced withdrawal row for cursor event"));
    }

    #[tokio::test]
    async fn journal_startup_recovery_fails_when_cursor_proposal_hash_is_malleated() {
        let journal = RecordingSequencerJournal::default();
        let (_dir, service) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(54, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonical proposal");
        service
            .with_conn(move |conn| {
                use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

                diesel::update(
                    sequencer_withdrawals::table
                        .filter(
                            sequenced::withdrawal_id_as_of.eq(tip5_to_bytes(&proposal.id.as_of)),
                        )
                        .filter(
                            sequenced::withdrawal_id_base_event_id.eq(proposal
                                .id
                                .base_event_id
                                .0
                                .clone()),
                        ),
                )
                .set(sequenced::proposal_hash.eq(Some("malleated-hash")))
                .execute(conn)
                .map_err(|err| {
                    BridgeError::Runtime(format!("failed to corrupt proposal_hash: {err}"))
                })?;
                Ok::<(), BridgeError>(())
            })
            .await
            .expect("corrupt proposal hash");

        let err = service
            .recover_from_journal_on_startup()
            .await
            .expect_err("proposal hash mismatch should fail closed");

        assert!(err.to_string().contains("proposal hash mismatch"));
    }

    #[tokio::test]
    async fn journal_startup_recovery_fails_when_cursor_tx_id_is_malleated() {
        let journal = RecordingSequencerJournal::default();
        let (_dir, service) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(55, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonical proposal");
        service
            .record_proposal_authorized(&proposal)
            .await
            .expect("record authorized proposal");
        service
            .with_conn(move |conn| {
                use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

                diesel::update(
                    sequencer_withdrawals::table
                        .filter(
                            sequenced::withdrawal_id_as_of.eq(tip5_to_bytes(&proposal.id.as_of)),
                        )
                        .filter(
                            sequenced::withdrawal_id_base_event_id.eq(proposal
                                .id
                                .base_event_id
                                .0
                                .clone()),
                        ),
                )
                .set(sequenced::authorized_transaction_name.eq(Some("malleated-tx-id")))
                .execute(conn)
                .map_err(|err| {
                    BridgeError::Runtime(format!(
                        "failed to corrupt authorized_transaction_name: {err}"
                    ))
                })?;
                Ok::<(), BridgeError>(())
            })
            .await
            .expect("corrupt tx id");

        let err = service
            .recover_from_journal_on_startup()
            .await
            .expect_err("tx id mismatch should fail closed");

        assert!(err.to_string().contains("submitted raw tx id mismatch"));
    }

    #[tokio::test]
    async fn journal_startup_recovery_fails_when_cursor_tx_jam_is_malleated() {
        let journal = RecordingSequencerJournal::default();
        let (_dir, service) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(56, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonical proposal");
        service
            .record_proposal_authorized(&proposal)
            .await
            .expect("record authorized proposal");
        service
            .with_conn(move |conn| {
                use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

                diesel::update(
                    sequencer_withdrawals::table
                        .filter(
                            sequenced::withdrawal_id_as_of.eq(tip5_to_bytes(&proposal.id.as_of)),
                        )
                        .filter(
                            sequenced::withdrawal_id_base_event_id.eq(proposal
                                .id
                                .base_event_id
                                .0
                                .clone()),
                        ),
                )
                .set(sequenced::authorized_transaction_jam.eq(Some(vec![0xde, 0xad, 0xbe, 0xef])))
                .execute(conn)
                .map_err(|err| {
                    BridgeError::Runtime(format!(
                        "failed to corrupt authorized_transaction_jam: {err}"
                    ))
                })?;
                Ok::<(), BridgeError>(())
            })
            .await
            .expect("corrupt tx jam");

        let err = service
            .recover_from_journal_on_startup()
            .await
            .expect_err("tx jam mismatch should fail closed");

        assert!(err
            .to_string()
            .contains("authorized transaction jam mismatch"));
    }

    #[tokio::test]
    async fn journal_startup_recovery_fails_when_cursor_submit_metadata_is_malleated() {
        let journal = RecordingSequencerJournal::default();
        let (_dir, service) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(57, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonical proposal");
        service
            .record_proposal_authorized(&proposal)
            .await
            .expect("record authorized proposal");
        service
            .record_submit_outcome(
                &proposal,
                WithdrawalState::Authorized,
                2,
                120,
                Some("submit failed".to_string()),
            )
            .await
            .expect("record submit outcome");
        service
            .with_conn(move |conn| {
                use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

                diesel::update(
                    sequencer_withdrawals::table
                        .filter(
                            sequenced::withdrawal_id_as_of.eq(tip5_to_bytes(&proposal.id.as_of)),
                        )
                        .filter(
                            sequenced::withdrawal_id_base_event_id.eq(proposal
                                .id
                                .base_event_id
                                .0
                                .clone()),
                        ),
                )
                .set(sequenced::last_submit_attempt_base_height.eq(Some(999_i64)))
                .execute(conn)
                .map_err(|err| {
                    BridgeError::Runtime(format!(
                        "failed to corrupt last_submit_attempt_base_height: {err}"
                    ))
                })?;
                Ok::<(), BridgeError>(())
            })
            .await
            .expect("corrupt submit metadata");

        let err = service
            .recover_from_journal_on_startup()
            .await
            .expect_err("submit metadata mismatch should fail closed");

        assert!(err.to_string().contains("last submit Base height mismatch"));
    }

    #[tokio::test]
    async fn journal_startup_recovery_fails_when_cursor_reserved_inputs_are_malleated() {
        let journal = RecordingSequencerJournal::default();
        let (_dir, service) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(58, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonical proposal");
        let id = proposal.id.clone();
        service
            .with_conn(move |conn| clear_reserved_inputs_for_withdrawal(conn, &id))
            .await
            .expect("corrupt reserved inputs");

        let err = service
            .recover_from_journal_on_startup()
            .await
            .expect_err("reserved input mismatch should fail closed");

        assert!(err
            .to_string()
            .contains("reserved inputs do not match cursor event"));
    }

    #[tokio::test]
    async fn journal_startup_recovery_replays_empty_sqlite_from_genesis() {
        let journal = RecordingSequencerJournal::default();
        let (_source_dir, source) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(45, 0);

        source
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        source
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonical proposal");

        let (_replay_dir, replay) = open_service_with_journal(journal.handle()).await;
        let recovery = replay
            .recover_from_journal_on_startup()
            .await
            .expect("startup recovery from genesis")
            .expect("journal enabled");

        assert_eq!(recovery.start_sequence, 0);
        assert_eq!(recovery.replayed_count, 2);
        assert_eq!(recovery.last_sequence, 2);
        assert_eq!(recovery.max_replayed_base_height, Some(125));
        assert_eq!(recovery.max_replayed_nockchain_height, Some(500));
        let sequenced = replay
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch replayed withdrawal")
            .expect("replayed withdrawal exists");
        assert_eq!(sequenced.state, WithdrawalState::PeerCanonical);
        assert_eq!(
            replay
                .reserved_input_names_for(&proposal.id)
                .await
                .expect("load replayed reservations"),
            proposal.selected_inputs
        );
    }

    #[tokio::test]
    async fn journal_startup_recovery_rejects_tampered_signed_remote_record() {
        let journal = RecordingSequencerJournal::default();
        let (_source_dir, source) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(61, 0);

        source
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        let mut records = journal.records();
        records[0].withdrawal.epoch = records[0].withdrawal.epoch.saturating_add(1);
        journal.replace_records(records);

        let (_replay_dir, replay) = open_service_with_journal(journal.handle()).await;
        let err = replay
            .recover_from_journal_on_startup()
            .await
            .expect_err("tampered signed remote record should fail closed");

        assert!(err.to_string().contains("event_id mismatch"));
        assert!(load_journal_cursor_for(&replay, "recording")
            .await
            .is_none());
    }

    #[tokio::test]
    async fn journal_startup_recovery_rejects_missing_remote_signature() {
        let journal = RecordingSequencerJournal::default();
        let (_source_dir, source) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(62, 0);

        source
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        let mut records = journal.records();
        records[0].signature = None;
        journal.replace_records(records);

        let (_replay_dir, replay) = open_service_with_journal(journal.handle()).await;
        let err = replay
            .recover_from_journal_on_startup()
            .await
            .expect_err("unsigned remote record should fail closed");

        assert!(err.to_string().contains("signature is missing"));
        assert!(load_journal_cursor_for(&replay, "recording")
            .await
            .is_none());
    }

    #[tokio::test]
    async fn journal_startup_recovery_fails_when_genesis_cursor_has_projection_rows() {
        let journal = RecordingSequencerJournal::default();
        let (_dir, service) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(46, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .with_conn(move |conn| upsert_journal_cursor(conn, "recording", 0, GENESIS_EVENT_ID, 0))
            .await
            .expect("force cursor back to genesis");

        let err = service
            .recover_from_journal_on_startup()
            .await
            .expect_err("genesis cursor with non-empty projection should fail closed");

        assert!(err.to_string().contains("non-empty SQLite projection"));
    }

    #[tokio::test]
    async fn journal_startup_recovery_fails_when_missing_cursor_has_projection_rows() {
        let journal = RecordingSequencerJournal::default();
        let (_dir, service) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(47, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .with_conn(move |conn| {
                diesel::delete(sequencer_journal_cursor::table)
                    .execute(conn)
                    .map_err(|err| {
                        BridgeError::Runtime(format!("failed to delete test cursor: {err}"))
                    })?;
                Ok::<(), BridgeError>(())
            })
            .await
            .expect("delete cursor");

        let err = service
            .recover_from_journal_on_startup()
            .await
            .expect_err("missing cursor with non-empty projection should fail closed");

        assert!(err.to_string().contains("non-empty SQLite projection"));
    }

    #[tokio::test]
    async fn journal_startup_recovery_fails_when_non_genesis_cursor_has_empty_projection() {
        let journal = RecordingSequencerJournal::default();
        let (_dir, service) = open_service_with_journal(journal.handle()).await;

        service
            .with_conn(|conn| upsert_journal_cursor(conn, "recording", 1, "missing", 0))
            .await
            .expect("seed non-genesis cursor without projection rows");

        let err = service
            .recover_from_journal_on_startup()
            .await
            .expect_err("non-genesis cursor with empty projection should fail closed");

        assert!(err
            .to_string()
            .contains("non-genesis cursor sequence 1 with empty SQLite projection"));
    }

    #[tokio::test]
    async fn journal_startup_recovery_rejects_bad_first_successor_link() {
        let journal = RecordingSequencerJournal::default();
        let proposal = sample_proposal(49, 0);
        let request_facts = SequencerWithdrawalRequestFacts::from_proposal(&proposal);
        let bad_first_event = sequencer_journal_record_with_request_facts(
            1,
            SequencerJournalEventType::WithdrawalOrdered,
            &proposal.id,
            0,
            Some(1),
            Some(&request_facts),
            None,
            None,
            None,
            None,
            None,
        )
        .expect("build bad first event")
        .into_ordered("recording".to_string(), 1, "not-genesis".to_string())
        .expect("order bad first event");
        journal
            .append(&bad_first_event)
            .expect("seed malformed remote event");
        let (_dir, service) = open_service_with_journal(journal.handle()).await;

        let err = service
            .recover_from_journal_on_startup()
            .await
            .expect_err("bad previous_event_id should fail closed");

        assert!(err.to_string().contains("previous_event_id mismatch"));
        assert!(load_journal_cursor_for(&service, "recording")
            .await
            .is_none());
    }

    #[tokio::test]
    async fn journal_startup_recovery_projection_failure_does_not_advance_cursor() {
        let (records, _proposal, _confirmed_block_id) =
            full_submission_lifecycle_journal_records(50).await;
        let bad_authorized_record = records[2]
            .clone()
            .into_ordered("recording".to_string(), 1, GENESIS_EVENT_ID.to_string())
            .expect("reorder bad authorized event");
        let journal = RecordingSequencerJournal::default();
        journal
            .append(&bad_authorized_record)
            .expect("seed bad remote event");
        let (_dir, service) = open_service_with_journal(journal.handle()).await;

        let err = service
            .recover_from_journal_on_startup()
            .await
            .expect_err("projection failure should fail startup");

        assert!(err.to_string().contains("missing sequenced withdrawal row"));
        assert!(load_journal_cursor_for(&service, "recording")
            .await
            .is_none());
    }

    #[tokio::test]
    async fn journal_startup_recovery_fails_when_local_cursor_is_ahead() {
        let journal = RecordingSequencerJournal::default();
        let (_dir, service) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(48, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("seed projection row");
        service
            .with_conn(move |conn| upsert_journal_cursor(conn, "recording", 2, "missing", 0))
            .await
            .expect("seed ahead cursor");

        let err = service
            .recover_from_journal_on_startup()
            .await
            .expect_err("local cursor ahead should fail");

        assert!(err
            .to_string()
            .contains("cursor object is missing or unreadable"));
    }

    #[tokio::test]
    async fn journal_startup_recovery_replays_when_local_cursor_is_behind() {
        let journal = RecordingSequencerJournal::default();
        let (_source_dir, source) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(35, 0);

        source
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        source
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonical proposal");
        let records = journal.records();
        let ordered = records[0].clone();

        let (_replay_dir, replay) = open_service_with_journal(journal.handle()).await;
        replay
            .with_conn(move |conn| {
                apply_journal_event(conn, &ordered, SequencerJournalApplyMode::Replay)?;
                upsert_journal_cursor(
                    conn, "recording", ordered.sequence, &ordered.event_id,
                    ordered.created_at_unix_ms,
                )
            })
            .await
            .expect("seed local projection at first event");

        let recovery = replay
            .recover_from_journal_on_startup()
            .await
            .expect("local cursor behind should replay successor")
            .expect("journal enabled");

        assert_eq!(recovery.start_sequence, 1);
        assert_eq!(recovery.last_sequence, 2);
        assert_eq!(recovery.replayed_count, 1);
        let cursor = load_journal_cursor_for(&replay, "recording")
            .await
            .expect("cursor advanced after replay");
        assert_eq!(cursor.last_sequence, 2);
        assert_eq!(cursor.last_event_id, records[1].event_id);
    }

    #[tokio::test]
    async fn journal_startup_recovery_replays_multiple_lifecycles_into_empty_sqlite() {
        let journal = RecordingSequencerJournal::default();
        let (_source_dir, source) = open_service_with_journal(journal.handle()).await;
        let (confirmed_a, _confirmed_block_a) =
            record_full_submission_lifecycle_with_nonce(&source, 80, 1).await;
        let (confirmed_b, _confirmed_block_b) =
            record_full_submission_lifecycle_with_nonce(&source, 81, 2).await;
        let authorized_c = record_authorized_lifecycle_with_nonce(&source, 82, 3).await;

        let records = journal.records();
        assert_eq!(records.len(), 19);
        let source_cursor = load_journal_cursor_for(&source, "recording")
            .await
            .expect("source journal cursor exists");
        assert_eq!(source_cursor.last_sequence, 19);
        assert_eq!(source_cursor.last_event_id, records[18].event_id);

        let (_replay_dir, replay) = open_service_with_journal(journal.handle()).await;
        let recovery = replay
            .recover_from_journal_on_startup()
            .await
            .expect("startup recovery from multi-lifecycle journal")
            .expect("journal enabled");

        assert_eq!(recovery.start_sequence, 0);
        assert_eq!(recovery.start_event_id, GENESIS_EVENT_ID);
        assert_eq!(recovery.replayed_count, 19);
        assert_eq!(recovery.last_sequence, 19);
        assert_eq!(recovery.last_event_id, source_cursor.last_event_id);
        let expected_base_bound = confirmed_a
            .base_batch_end
            .max(confirmed_b.base_batch_end)
            .max(authorized_c.base_batch_end)
            .max(160);
        assert_eq!(recovery.max_replayed_base_height, Some(expected_base_bound));
        assert_eq!(recovery.max_replayed_nockchain_height, Some(777));

        assert_recovered_confirmed_lifecycle(&replay, &confirmed_a, 1).await;
        assert_recovered_confirmed_lifecycle(&replay, &confirmed_b, 2).await;
        assert_recovered_authorized_lifecycle(&replay, &authorized_c, 3).await;
        assert_eq!(
            replay
                .current_live_withdrawal_nonce()
                .await
                .expect("load live frontier"),
            Some(3)
        );

        let cursor = load_journal_cursor_for(&replay, "recording")
            .await
            .expect("replay cursor advanced");
        assert_eq!(cursor.last_sequence, source_cursor.last_sequence);
        assert_eq!(cursor.last_event_id, source_cursor.last_event_id);
    }

    #[tokio::test]
    #[ignore = "requires BRIDGE_R2_RUN_E2E=1 and R2 S3-compatible credentials"]
    async fn r2_journal_startup_recovery_replays_full_lifecycle_into_empty_sqlite() {
        if !r2::enabled() {
            eprintln!(
                "skipping R2 recovery E2E test; set {}=1 to run it",
                r2::R2_E2E_ENABLE_ENV
            );
            return;
        }
        let _r2_guard = r2::async_serial_guard().await;
        let (handle, journal, _cleanup, journal_id) =
            r2_journal_bundle("store-full-lifecycle-recovery");
        let (_source_dir, source) = open_service_with_journal(handle).await;
        let (proposal, _confirmed_block_id) = record_full_submission_lifecycle(&source, 70).await;
        let expected_authorized =
            stored_authorized_transaction(&proposal.transaction).expect("authorized artifacts");

        let source_cursor = load_journal_cursor_for(&source, &journal_id)
            .await
            .expect("source journal cursor exists");
        assert_eq!(source_cursor.last_sequence, 8);
        let remote_tail = r2::expect("verify remote journal", verify_remote_journal(&journal));
        assert_eq!(
            remote_tail.last_sequence,
            u64::try_from(source_cursor.last_sequence).expect("cursor sequence is non-negative")
        );
        assert_eq!(remote_tail.last_event_id, source_cursor.last_event_id);

        let (_replay_dir, replay) = open_service_with_journal(SequencerJournalHandle::ObjectStore(
            Box::new(journal.clone()),
        ))
        .await;
        let recovery = replay
            .recover_from_journal_on_startup()
            .await
            .expect("startup recovery from R2 journal")
            .expect("journal enabled");

        assert_eq!(recovery.start_sequence, 0);
        assert_eq!(recovery.start_event_id, GENESIS_EVENT_ID);
        assert_eq!(recovery.replayed_count, 8);
        assert_eq!(
            recovery.last_sequence,
            u64::try_from(source_cursor.last_sequence).expect("cursor sequence is non-negative")
        );
        assert_eq!(recovery.last_event_id, source_cursor.last_event_id);
        assert_eq!(recovery.max_replayed_base_height, Some(160));
        assert_eq!(recovery.max_replayed_nockchain_height, Some(777));

        let proposal_id = proposal.id.clone();
        let sequenced = replay
            .with_conn(move |conn| {
                fetch_sequenced_withdrawal(conn, &proposal_id.base_event_id)?
                    .ok_or_else(|| BridgeError::Runtime("missing recovered withdrawal".to_string()))
            })
            .await
            .expect("fetch recovered withdrawal");
        assert_eq!(sequenced.state, WithdrawalState::Confirmed);
        assert_eq!(sequenced.withdrawal_nonce, Some(1));
        assert_eq!(
            sequenced.proposal_hash,
            Some(proposal.proposal_hash().expect("proposal hash"))
        );
        assert_eq!(sequenced.canonical_amount, Some(proposal.amount));
        assert_eq!(
            sequenced.canonical_base_batch_end,
            Some(proposal.base_batch_end)
        );
        assert_eq!(
            sequenced.canonical_selected_inputs,
            Some(proposal.selected_inputs.clone())
        );
        assert_eq!(
            sequenced.canonical_snapshot,
            Some(proposal.snapshot.clone())
        );
        assert_eq!(
            sequenced.authorized_transaction_name,
            Some(expected_authorized.submitted_raw_tx_id.clone())
        );
        assert_eq!(
            sequenced.authorized_transaction_jam,
            Some(expected_authorized.transaction_jam.clone())
        );
        assert_eq!(
            sequenced.authorized_raw_tx,
            Some(expected_authorized.raw_tx_bytes.clone())
        );
        assert_eq!(sequenced.submit_attempt_count, 4);
        assert_eq!(sequenced.last_submit_attempt_base_height, Some(160));
        assert_eq!(sequenced.last_submit_error.as_deref(), Some("orphan retry"));
        assert!(replay
            .reserved_input_names_for(&proposal.id)
            .await
            .expect("load recovered reserved inputs")
            .is_empty());
        assert_eq!(
            replay
                .current_live_withdrawal_nonce()
                .await
                .expect("load live frontier"),
            None
        );

        let cursor = load_journal_cursor_for(&replay, &journal_id)
            .await
            .expect("replay cursor advanced");
        assert_eq!(cursor.last_sequence, source_cursor.last_sequence);
        assert_eq!(cursor.last_event_id, source_cursor.last_event_id);
    }

    #[tokio::test]
    #[ignore = "requires BRIDGE_R2_RUN_E2E=1 and R2 S3-compatible credentials"]
    async fn r2_journal_startup_recovery_replays_multiple_lifecycles_into_empty_sqlite() {
        if !r2::enabled() {
            eprintln!(
                "skipping R2 recovery E2E test; set {}=1 to run it",
                r2::R2_E2E_ENABLE_ENV
            );
            return;
        }
        let _r2_guard = r2::async_serial_guard().await;
        let (handle, journal, _cleanup, journal_id) =
            r2_journal_bundle("store-multi-lifecycle-recovery");
        let (_source_dir, source) = open_service_with_journal(handle).await;
        let (confirmed_a, _confirmed_block_a) =
            record_full_submission_lifecycle_with_nonce(&source, 83, 1).await;
        let (confirmed_b, _confirmed_block_b) =
            record_full_submission_lifecycle_with_nonce(&source, 84, 2).await;
        let authorized_c = record_authorized_lifecycle_with_nonce(&source, 85, 3).await;

        let source_cursor = load_journal_cursor_for(&source, &journal_id)
            .await
            .expect("source journal cursor exists");
        assert_eq!(source_cursor.last_sequence, 19);
        let remote_tail = r2::expect("verify remote journal", verify_remote_journal(&journal));
        assert_eq!(remote_tail.last_sequence, 19);
        assert_eq!(remote_tail.last_event_id, source_cursor.last_event_id);

        let (_replay_dir, replay) = open_service_with_journal(SequencerJournalHandle::ObjectStore(
            Box::new(journal.clone()),
        ))
        .await;
        let recovery = replay
            .recover_from_journal_on_startup()
            .await
            .expect("startup recovery from R2 multi-lifecycle journal")
            .expect("journal enabled");

        assert_eq!(recovery.start_sequence, 0);
        assert_eq!(recovery.start_event_id, GENESIS_EVENT_ID);
        assert_eq!(recovery.replayed_count, 19);
        assert_eq!(recovery.last_sequence, 19);
        assert_eq!(recovery.last_event_id, source_cursor.last_event_id);
        let expected_base_bound = confirmed_a
            .base_batch_end
            .max(confirmed_b.base_batch_end)
            .max(authorized_c.base_batch_end)
            .max(160);
        assert_eq!(recovery.max_replayed_base_height, Some(expected_base_bound));
        assert_eq!(recovery.max_replayed_nockchain_height, Some(777));

        assert_recovered_confirmed_lifecycle(&replay, &confirmed_a, 1).await;
        assert_recovered_confirmed_lifecycle(&replay, &confirmed_b, 2).await;
        assert_recovered_authorized_lifecycle(&replay, &authorized_c, 3).await;
        assert_eq!(
            replay
                .current_live_withdrawal_nonce()
                .await
                .expect("load live frontier"),
            Some(3)
        );

        let cursor = load_journal_cursor_for(&replay, &journal_id)
            .await
            .expect("cursor advanced after R2 replay");
        assert_eq!(cursor.last_sequence, source_cursor.last_sequence);
        assert_eq!(cursor.last_event_id, source_cursor.last_event_id);
    }

    #[tokio::test]
    #[ignore = "requires BRIDGE_R2_RUN_E2E=1 and R2 S3-compatible credentials"]
    async fn r2_journal_startup_recovery_replays_successors_from_local_cursor() {
        if !r2::enabled() {
            eprintln!(
                "skipping R2 recovery E2E test; set {}=1 to run it",
                r2::R2_E2E_ENABLE_ENV
            );
            return;
        }
        let _r2_guard = r2::async_serial_guard().await;
        let (handle, journal, _cleanup, journal_id) = r2_journal_bundle("store-cursor-recovery");
        let (_source_dir, source) = open_service_with_journal(handle).await;
        let (proposal, _confirmed_block_id) = record_full_submission_lifecycle(&source, 71).await;
        let object_refs = r2::expect("list remote journal", journal.list());
        assert_eq!(object_refs.len(), 8);
        let ordered = r2::expect("load first remote event", journal.get(&object_refs[0]));

        let (_replay_dir, replay) = open_service_with_journal(SequencerJournalHandle::ObjectStore(
            Box::new(journal.clone()),
        ))
        .await;
        let journal_id_for_cursor = journal_id.clone();
        replay
            .with_conn(move |conn| {
                apply_journal_event(conn, &ordered, SequencerJournalApplyMode::Replay)?;
                upsert_journal_cursor(
                    conn, &journal_id_for_cursor, ordered.sequence, &ordered.event_id,
                    ordered.created_at_unix_ms,
                )
            })
            .await
            .expect("seed replay projection at first event");

        let recovery = replay
            .recover_from_journal_on_startup()
            .await
            .expect("startup recovery from R2 cursor")
            .expect("journal enabled");

        assert_eq!(recovery.start_sequence, 1);
        assert_eq!(recovery.replayed_count, 7);
        assert_eq!(recovery.last_sequence, 8);
        let sequenced = replay
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch recovered withdrawal")
            .expect("recovered withdrawal exists");
        assert_eq!(sequenced.state, WithdrawalState::Confirmed);
        let cursor = load_journal_cursor_for(&replay, &journal_id)
            .await
            .expect("cursor advanced after R2 replay");
        assert_eq!(cursor.last_sequence, 8);
        assert_eq!(cursor.last_event_id, recovery.last_event_id);
    }

    #[tokio::test]
    #[ignore = "requires BRIDGE_R2_RUN_E2E=1 and R2 S3-compatible credentials"]
    async fn r2_journal_startup_recovery_fails_when_local_cursor_is_ahead_of_remote() {
        if !r2::enabled() {
            eprintln!(
                "skipping R2 recovery E2E test; set {}=1 to run it",
                r2::R2_E2E_ENABLE_ENV
            );
            return;
        }
        let _r2_guard = r2::async_serial_guard().await;
        let (handle, _journal, _cleanup, journal_id) = r2_journal_bundle("store-cursor-ahead");
        let (_dir, service) = open_service_with_journal(handle).await;
        let proposal = sample_proposal(72, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("seed one remote event and local projection");
        service
            .with_conn(move |conn| upsert_journal_cursor(conn, &journal_id, 999, "missing", 0))
            .await
            .expect("seed ahead cursor");

        let err = service
            .recover_from_journal_on_startup()
            .await
            .expect_err("cursor ahead of R2 remote journal should fail closed");

        assert!(err
            .to_string()
            .contains("cursor object is missing or unreadable"));
    }

    #[tokio::test]
    #[ignore = "requires BRIDGE_R2_RUN_E2E=1 and R2 S3-compatible credentials"]
    async fn r2_journal_startup_recovery_rejects_remote_sequence_gap() {
        if !r2::enabled() {
            eprintln!(
                "skipping R2 recovery E2E test; set {}=1 to run it",
                r2::R2_E2E_ENABLE_ENV
            );
            return;
        }
        let _r2_guard = r2::async_serial_guard().await;
        let (_handle, journal, _cleanup, journal_id) = r2_journal_bundle("store-sequence-gap");
        let proposal = sample_proposal(73, 0);
        let request_facts = SequencerWithdrawalRequestFacts::from_proposal(&proposal);
        let bad_first_event = sequencer_journal_record_with_request_facts(
            1,
            SequencerJournalEventType::WithdrawalOrdered,
            &proposal.id,
            0,
            Some(1),
            Some(&request_facts),
            None,
            None,
            None,
            None,
            None,
        )
        .expect("build gap event")
        .into_ordered(journal_id.clone(), 2, GENESIS_EVENT_ID.to_string())
        .expect("order gap event");
        r2::expect("append gap event", journal.append(&bad_first_event));

        let (_dir, service) = open_service_with_journal(SequencerJournalHandle::ObjectStore(
            Box::new(journal.clone()),
        ))
        .await;
        let err = service
            .recover_from_journal_on_startup()
            .await
            .expect_err("remote sequence gap should fail closed");

        assert!(err.to_string().contains("successor sequence mismatch"));
        assert!(load_journal_cursor_for(&service, &journal_id)
            .await
            .is_none());
    }

    #[tokio::test]
    async fn durable_journal_failure_aborts_projection_update() {
        let journal = RecordingSequencerJournal::default();
        let (_dir, service) = open_service_with_journal(journal.handle()).await;
        let proposal = sample_proposal(31, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        journal.set_fail(true);
        let err = service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect_err("remote journal failure should abort canonicalization");
        assert!(err
            .to_string()
            .contains("remote sequencer journal unavailable"));

        let row = service
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal after failed journal append")
            .expect("ordering row remains");
        assert_eq!(row.proposal_hash, None);
        assert_eq!(
            service
                .reserved_input_names_for(&proposal.id)
                .await
                .expect("reserved inputs after failed journal append"),
            Vec::<nockchain_types::v1::Name>::new()
        );
        let cursor = load_journal_cursor_for(&service, "recording")
            .await
            .expect("cursor remains at successful ordering event");
        assert_eq!(cursor.last_sequence, 1);
        assert_eq!(journal.records().len(), 1);
    }

    #[tokio::test]
    async fn durable_journal_projection_failure_does_not_advance_cursor() {
        let (records, _proposal, _confirmed_block_id) =
            full_submission_lifecycle_journal_records(44).await;
        let bad_authorized_record = records[2].clone();
        let journal = RecordingSequencerJournal::default();
        let (_dir, service) = open_service_with_journal(journal.handle()).await;
        service
            .with_conn(move |conn| upsert_journal_cursor(conn, "recording", 7, "stable-cursor", 11))
            .await
            .expect("seed existing cursor");

        let err = service
            .with_write_tx(move |conn, journal| {
                append_and_project_journal_records(conn, journal, &[bad_authorized_record])
            })
            .await
            .expect_err("projection should fail because prerequisite rows are missing");
        assert!(err.to_string().contains("missing sequenced withdrawal row"));

        // The remote append happened, but local projection failed before the
        // cursor update. Model A keeps the old cursor as the exact frontier so
        // startup recovery can replay this remote successor later.
        assert_eq!(journal.records().len(), 1);
        let cursor = load_journal_cursor_for(&service, "recording")
            .await
            .expect("existing cursor remains present");
        assert_eq!(cursor.last_sequence, 7);
        assert_eq!(cursor.last_event_id, "stable-cursor");
    }

    async fn open_service() -> (tempfile::TempDir, WithdrawalSequencerStore) {
        let dir = tempdir().expect("tempdir");
        let path: PathBuf = dir.path().join("withdrawal-state-store.sqlite");
        let service = WithdrawalSequencerStore::open(path)
            .await
            .expect("open withdrawal state store");
        (dir, service)
    }

    async fn open_service_with_journal(
        journal: SequencerJournalHandle,
    ) -> (tempfile::TempDir, WithdrawalSequencerStore) {
        let dir = tempdir().expect("tempdir");
        let path: PathBuf = dir.path().join("withdrawal-state-store.sqlite");
        let service = WithdrawalSequencerStore::open(path)
            .await
            .expect("open withdrawal state store")
            .with_journal(journal);
        (dir, service)
    }

    fn r2_journal_bundle(
        test_name: &str,
    ) -> (
        SequencerJournalHandle,
        ObjectStoreSequencerJournal,
        r2::Cleanup,
        String,
    ) {
        let config = r2::object_store_config(test_name);
        let journal_id = config.journal_id.clone();
        let journal =
            ObjectStoreSequencerJournal::new(config.clone()).expect("construct R2 cleanup journal");
        let cleanup = r2::Cleanup::new(journal.clone());
        let handle =
            SequencerJournalHandle::object_store(config).expect("construct R2 sequencer journal");
        (handle, journal, cleanup, journal_id)
    }

    async fn load_journal_cursor_for(
        service: &WithdrawalSequencerStore,
        journal_id: &str,
    ) -> Option<SequencerJournalCursorRow> {
        let journal_id = journal_id.to_string();
        service
            .with_conn(move |conn| {
                use crate::withdrawal::sequencer::schema::sequencer_journal_cursor::dsl as cursor;

                sequencer_journal_cursor::table
                    .filter(cursor::journal_id.eq(journal_id))
                    .first::<SequencerJournalCursorRow>(conn)
                    .optional()
                    .map_err(|err| {
                        BridgeError::Runtime(format!(
                            "sequencer journal cursor test row fetch failed: {err}"
                        ))
                    })
            })
            .await
            .expect("load journal cursor")
    }

    async fn load_stored_row(
        service: &WithdrawalSequencerStore,
        id: &WithdrawalId,
    ) -> SequencerWithdrawalStoredRow {
        let id = id.clone();
        service
            .with_conn(move |conn| {
                use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

                sequencer_withdrawals::table
                    .filter(sequenced::withdrawal_id_as_of.eq(tip5_to_bytes(&id.as_of)))
                    .filter(sequenced::withdrawal_id_base_event_id.eq(id.base_event_id.0.clone()))
                    .first::<SequencerWithdrawalStoredRow>(conn)
                    .map_err(|err| {
                        BridgeError::Runtime(format!(
                            "sequencer withdrawal test row fetch failed: {err}"
                        ))
                    })
            })
            .await
            .expect("load stored sequencer row")
    }

    fn sample_commit_certificate(proposal: &WithdrawalProposalData) -> WithdrawalCommitCertificate {
        WithdrawalCommitCertificate {
            withdrawal_id: Some(withdrawal_id_to_proto(&proposal.id)),
            epoch: proposal.epoch,
            proposal_hash: proposal.proposal_hash().expect("proposal hash"),
            signatures: Vec::new(),
        }
    }

    async fn full_submission_lifecycle_journal_records(
        seed: u64,
    ) -> (
        Vec<SequencerJournalRecord>,
        WithdrawalProposalData,
        Tip5Hash,
    ) {
        let journal = RecordingSequencerJournal::default();
        let (_dir, source) = open_service_with_journal(journal.handle()).await;
        let (proposal, confirmed_block_id) = record_full_submission_lifecycle(&source, seed).await;

        (journal.records(), proposal, confirmed_block_id)
    }

    async fn record_full_submission_lifecycle(
        source: &WithdrawalSequencerStore,
        seed: u64,
    ) -> (WithdrawalProposalData, Tip5Hash) {
        record_full_submission_lifecycle_with_nonce(source, seed, 1).await
    }

    async fn record_full_submission_lifecycle_with_nonce(
        source: &WithdrawalSequencerStore,
        seed: u64,
        withdrawal_nonce: u64,
    ) -> (WithdrawalProposalData, Tip5Hash) {
        let proposal = sample_proposal(seed, 0);
        let proposal_hash = proposal.proposal_hash().expect("proposal hash");
        let confirmed_block_id = Tip5Hash([
            Belt(seed + 900),
            Belt(seed + 901),
            Belt(seed + 902),
            Belt(seed + 903),
            Belt(seed + 904),
        ]);

        source
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, withdrawal_nonce))
            .await
            .expect("record ordering");
        source
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonical proposal");
        source
            .record_proposal_authorized(&proposal)
            .await
            .expect("record authorized proposal");
        source
            .record_submit_outcome(
                &proposal,
                WithdrawalState::Authorized,
                2,
                120,
                Some("submit failed".to_string()),
            )
            .await
            .expect("record failed submit attempt");
        source
            .record_submit_outcome(&proposal, WithdrawalState::MempoolAccepted, 3, 140, None)
            .await
            .expect("record mempool-accepted submit attempt");
        source
            .record_mempool_retry_attempt(
                &proposal.id,
                proposal.epoch,
                &proposal_hash,
                160,
                Some("orphan retry".to_string()),
            )
            .await
            .expect("record mempool retry attempt");
        source
            .record_tx_confirmed(&proposal, 777, confirmed_block_id.clone())
            .await
            .expect("record confirmed withdrawal");

        (proposal, confirmed_block_id)
    }

    async fn record_authorized_lifecycle_with_nonce(
        source: &WithdrawalSequencerStore,
        seed: u64,
        withdrawal_nonce: u64,
    ) -> WithdrawalProposalData {
        let proposal = sample_proposal(seed, 0);
        source
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, withdrawal_nonce))
            .await
            .expect("record ordering");
        source
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonical proposal");
        source
            .record_proposal_authorized(&proposal)
            .await
            .expect("record authorized proposal");
        proposal
    }

    async fn assert_recovered_confirmed_lifecycle(
        replay: &WithdrawalSequencerStore,
        proposal: &WithdrawalProposalData,
        withdrawal_nonce: u64,
    ) {
        let expected_authorized =
            stored_authorized_transaction(&proposal.transaction).expect("authorized artifacts");
        let id = proposal.id.clone();
        let sequenced = replay
            .with_conn(move |conn| {
                fetch_sequenced_withdrawal(conn, &id.base_event_id)?
                    .ok_or_else(|| BridgeError::Runtime("missing recovered row".to_string()))
            })
            .await
            .expect("fetch recovered withdrawal");
        assert_eq!(sequenced.state, WithdrawalState::Confirmed);
        assert_eq!(sequenced.withdrawal_nonce, Some(withdrawal_nonce));
        assert_eq!(
            sequenced.proposal_hash,
            Some(proposal.proposal_hash().expect("proposal hash"))
        );
        assert_eq!(sequenced.canonical_amount, Some(proposal.amount));
        assert_eq!(
            sequenced.canonical_base_batch_end,
            Some(proposal.base_batch_end)
        );
        assert_eq!(
            sequenced.canonical_selected_inputs,
            Some(proposal.selected_inputs.clone())
        );
        assert_eq!(
            sequenced.canonical_snapshot,
            Some(proposal.snapshot.clone())
        );
        assert_eq!(
            sequenced.authorized_transaction_name,
            Some(expected_authorized.submitted_raw_tx_id)
        );
        assert_eq!(
            sequenced.authorized_transaction_jam,
            Some(expected_authorized.transaction_jam)
        );
        assert_eq!(
            sequenced.authorized_raw_tx,
            Some(expected_authorized.raw_tx_bytes)
        );
        assert_eq!(sequenced.submit_attempt_count, 4);
        assert_eq!(sequenced.last_submit_attempt_base_height, Some(160));
        assert_eq!(sequenced.last_submit_error.as_deref(), Some("orphan retry"));
        assert!(replay
            .reserved_input_names_for(&proposal.id)
            .await
            .expect("load recovered reserved inputs")
            .is_empty());
    }

    async fn assert_recovered_authorized_lifecycle(
        replay: &WithdrawalSequencerStore,
        proposal: &WithdrawalProposalData,
        withdrawal_nonce: u64,
    ) {
        let expected_authorized =
            stored_authorized_transaction(&proposal.transaction).expect("authorized artifacts");
        let id = proposal.id.clone();
        let sequenced = replay
            .with_conn(move |conn| {
                fetch_sequenced_withdrawal(conn, &id.base_event_id)?.ok_or_else(|| {
                    BridgeError::Runtime("missing recovered authorized row".to_string())
                })
            })
            .await
            .expect("fetch recovered authorized withdrawal");
        assert_eq!(sequenced.state, WithdrawalState::Authorized);
        assert_eq!(sequenced.withdrawal_nonce, Some(withdrawal_nonce));
        assert_eq!(
            sequenced.proposal_hash,
            Some(proposal.proposal_hash().expect("proposal hash"))
        );
        assert_eq!(
            sequenced.authorized_transaction_name,
            Some(expected_authorized.submitted_raw_tx_id)
        );
        assert_eq!(
            sequenced.authorized_transaction_jam,
            Some(expected_authorized.transaction_jam)
        );
        assert_eq!(
            sequenced.authorized_raw_tx,
            Some(expected_authorized.raw_tx_bytes)
        );
        assert_eq!(
            replay
                .reserved_input_names_for(&proposal.id)
                .await
                .expect("load recovered authorized reservations"),
            proposal.selected_inputs
        );
    }

    async fn replay_record(
        service: &WithdrawalSequencerStore,
        record: SequencerJournalRecord,
    ) -> Result<(), BridgeError> {
        service
            .with_conn(move |conn| {
                apply_journal_event(conn, &record, SequencerJournalApplyMode::Replay)
            })
            .await
    }

    async fn replay_records(
        service: &WithdrawalSequencerStore,
        records: &[SequencerJournalRecord],
    ) -> Result<(), BridgeError> {
        for record in records {
            replay_record(service, record.clone()).await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn ensure_schema_renames_live_withdrawals_table_to_sequencer_withdrawals() {
        let dir = tempdir().expect("tempdir");
        let path: PathBuf = dir.path().join("withdrawal-state-store.sqlite");
        let id = sample_withdrawal_id(77);
        let as_of_hex = blob_hex(&tip5_to_bytes(&id.as_of));
        let base_event_hex = blob_hex(&id.base_event_id.0);

        {
            let path_str = path.to_str().expect("sqlite path should be valid unicode");
            let mut conn =
                SqliteConnection::establish(path_str).expect("open legacy sqlite connection");
            conn.batch_execute(&format!(
                r#"
                CREATE TABLE live_withdrawals (
                    withdrawal_id_as_of BLOB NOT NULL CHECK(length(withdrawal_id_as_of) = 40),
                    withdrawal_id_base_event_id BLOB NOT NULL,
                    withdrawal_nonce INTEGER,
                    current_epoch INTEGER NOT NULL,
                    proposal_hash TEXT,
                    peer_commit_certificate BLOB,
                    authorized_transaction_name TEXT,
                    state TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    PRIMARY KEY (withdrawal_id_as_of, withdrawal_id_base_event_id)
                );

                CREATE INDEX live_withdrawals_by_state
                  ON live_withdrawals(state, updated_at);

                INSERT INTO live_withdrawals (
                    withdrawal_id_as_of,
                    withdrawal_id_base_event_id,
                    withdrawal_nonce,
                    current_epoch,
                    proposal_hash,
                    peer_commit_certificate,
                    proposal_hash,
                    authorized_transaction_name,
                    state,
                    created_at,
                    updated_at
                ) VALUES (
                    X'{as_of_hex}',
                    X'{base_event_hex}',
                    77,
                    3,
                    'peer-hash',
                    NULL,
                    'authorized-hash',
                    'tx-name',
                    'mempool_accepted',
                    111,
                    222
                );
                "#
            ))
            .expect("seed legacy live_withdrawals table");
        }

        let service = WithdrawalSequencerStore::open(path.clone())
            .await
            .expect("open withdrawal state store");
        let migrated = service
            .fetch_sequenced_withdrawal(&id)
            .await
            .expect("fetch migrated sequenced withdrawal")
            .expect("migrated sequenced withdrawal exists");
        assert_eq!(migrated.withdrawal_nonce, Some(77));
        assert_eq!(migrated.current_epoch, 3);
        assert_eq!(
            migrated.authorized_transaction_name.as_deref(),
            Some("tx-name")
        );
        assert_eq!(migrated.state, WithdrawalState::MempoolAccepted);

        let path_str = path.to_str().expect("sqlite path should be valid unicode");
        let mut conn = SqliteConnection::establish(path_str).expect("reopen sqlite connection");
        assert!(!sqlite_table_exists(&mut conn, "live_withdrawals")
            .expect("check legacy live_withdrawals absence"),);
        assert!(sqlite_table_exists(&mut conn, "sequencer_withdrawals")
            .expect("check sequencer_withdrawals presence"),);
        assert!(sqlite_table_exists(&mut conn, "sequencer_journal_cursor")
            .expect("check sequencer_journal_cursor presence"),);
        assert!(sqlite_column_exists(
            &mut conn, "sequencer_withdrawals", "authorized_transaction_jam",
        )
        .expect("check authorized_transaction_jam presence"),);
        assert!(
            sqlite_column_exists(&mut conn, "sequencer_withdrawals", "authorized_raw_tx",)
                .expect("check authorized_raw_tx presence"),
        );
    }

    #[tokio::test]
    async fn ensure_schema_keeps_reserved_inputs_table_without_snapshot_columns() {
        let (_dir, service) = open_service().await;

        service
            .with_conn(move |conn| -> Result<(), BridgeError> {
                assert!(sqlite_table_exists(conn, "withdrawal_reserved_inputs")?);
                assert!(!sqlite_column_exists(
                    conn, "sequencer_withdrawals", "reservation_snapshot_height",
                )?);
                assert!(!sqlite_column_exists(
                    conn, "sequencer_withdrawals", "reservation_snapshot_block_id",
                )?);
                Ok(())
            })
            .await
            .expect("verify reservation schema");
    }

    #[tokio::test]
    async fn canonicalization_preserves_handoff_state() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(87, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("register withdrawal ordering");
        service
            .record_precanonical_handoff_for_id(&proposal.id, proposal.epoch, 2, 77)
            .await
            .expect("record pre-canonical handoff");
        service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonicalized");

        let sequenced = service
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .expect("sequenced withdrawal exists");
        assert_eq!(sequenced.state, WithdrawalState::PeerCanonical);
        assert_eq!(sequenced.handoff_index, 2);
        assert_eq!(sequenced.turn_started_base_height, Some(100));
        assert_eq!(
            service
                .reserved_input_names_for(&proposal.id)
                .await
                .expect("load reserved inputs for canonical withdrawal"),
            proposal.selected_inputs
        );
    }

    async fn authorize_with_handoffs(
        service: &WithdrawalSequencerStore,
        proposal: &WithdrawalProposalData,
    ) {
        // Seed post-canonical handoff state before later tests mutate submit
        // metadata, so they can catch any whole-row upsert that accidentally
        // resets sequencer coordination fields like handoff_index or
        // turn_started_base_height.
        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(proposal, 1))
            .await
            .expect("register withdrawal ordering");
        service
            .record_proposal_canonicalized(proposal, 100)
            .await
            .expect("record canonicalized");
        service
            .record_proposer_turn_expired_for_id(&proposal.id, proposal.epoch, 1, 130)
            .await
            .expect("record peer-canonical handoff");
        service
            .record_proposal_authorized(proposal)
            .await
            .expect("record authorized");
        service
            .record_proposer_turn_expired_for_id(&proposal.id, proposal.epoch, 2, 160)
            .await
            .expect("record authorized handoff");
    }

    async fn mempool_accept_with_handoffs(
        service: &WithdrawalSequencerStore,
        proposal: &WithdrawalProposalData,
        submit_attempt_count: u64,
        last_submit_attempt_base_height: u64,
        last_submit_error: Option<String>,
    ) {
        authorize_with_handoffs(service, proposal).await;
        service
            .record_submit_outcome(
                proposal,
                WithdrawalState::MempoolAccepted,
                submit_attempt_count,
                last_submit_attempt_base_height,
                last_submit_error,
            )
            .await
            .expect("record mempool accepted");
    }

    #[tokio::test]
    async fn authorized_handoffs_update_current_row() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(88, 0);

        authorize_with_handoffs(&service, &proposal).await;

        let sequenced = service
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch withdrawal after authorization")
            .expect("authorized withdrawal remains sequenced");
        assert_eq!(sequenced.state, WithdrawalState::Authorized);
        assert_eq!(sequenced.handoff_index, 2);
        assert_eq!(sequenced.turn_started_base_height, Some(160));
        assert_eq!(
            service
                .reserved_input_names_for(&proposal.id)
                .await
                .expect("load reserved inputs after authorization"),
            proposal.selected_inputs
        );
    }

    #[tokio::test]
    async fn record_proposal_authorized_persists_retry_artifacts() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(88_001, 0);

        authorize_with_handoffs(&service, &proposal).await;

        let stored = load_stored_row(&service, &proposal.id).await;
        let expected =
            stored_authorized_transaction(&proposal.transaction).expect("stored authorized tx");
        assert_eq!(
            stored.authorized_transaction_name,
            Some(expected.submitted_raw_tx_id)
        );
        assert_eq!(
            stored.authorized_transaction_jam,
            Some(expected.transaction_jam)
        );
        assert_eq!(stored.authorized_raw_tx, Some(expected.raw_tx_bytes));
    }

    #[tokio::test]
    async fn load_authorized_transaction_export_by_tx_id_returns_transaction_jam() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(88_002, 0);

        authorize_with_handoffs(&service, &proposal).await;

        let expected =
            stored_authorized_transaction(&proposal.transaction).expect("stored authorized tx");
        let export = service
            .load_authorized_transaction_export_by_tx_id(&expected.submitted_raw_tx_id)
            .await
            .expect("load transaction export")
            .expect("transaction export exists");
        assert_eq!(export.submitted_raw_tx_id, expected.submitted_raw_tx_id);
        assert_eq!(export.transaction_jam, expected.transaction_jam);
    }

    #[tokio::test]
    async fn authorized_handoff_state_persists_through_submit_failure() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(88, 0);

        authorize_with_handoffs(&service, &proposal).await;

        service
            .record_submit_outcome(
                &proposal,
                WithdrawalState::Authorized,
                1,
                200,
                Some("submit failed".to_string()),
            )
            .await
            .expect("record submit failure");
        let sequenced = service
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch authorized withdrawal")
            .expect("authorized withdrawal remains sequenced");
        assert_eq!(sequenced.state, WithdrawalState::Authorized);
        assert_eq!(sequenced.handoff_index, 2);
        assert_eq!(sequenced.turn_started_base_height, Some(160));
        assert_eq!(
            service
                .reserved_input_names_for(&proposal.id)
                .await
                .expect("load reserved inputs after submit failure"),
            proposal.selected_inputs
        );
        assert_eq!(
            service
                .list_reserved_input_names()
                .await
                .expect("list reserved inputs after submit failure"),
            proposal.selected_inputs
        );
    }

    #[tokio::test]
    async fn authorized_handoff_state_persists_through_mempool_acceptance() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(89, 0);

        authorize_with_handoffs(&service, &proposal).await;

        service
            .record_submit_outcome(&proposal, WithdrawalState::MempoolAccepted, 1, 111, None)
            .await
            .expect("record mempool accepted");
        let sequenced = service
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch mempool-accepted withdrawal")
            .expect("mempool-accepted withdrawal remains sequenced");
        assert_eq!(sequenced.state, WithdrawalState::MempoolAccepted);
        assert_eq!(sequenced.handoff_index, 2);
        assert_eq!(sequenced.turn_started_base_height, Some(160));
        assert_eq!(
            service
                .reserved_input_names_for(&proposal.id)
                .await
                .expect("load reserved inputs after mempool acceptance"),
            proposal.selected_inputs
        );
    }

    #[tokio::test]
    async fn load_authorized_transaction_for_retry_returns_exact_persisted_raw_tx() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(90, 0);

        mempool_accept_with_handoffs(&service, &proposal, 1, 200, None).await;
        let expected_raw_tx =
            withdrawal_raw_tx::persisted_raw_tx_from_transaction(&proposal.transaction)
                .expect("persisted raw tx");

        let payload = service
            .load_authorized_transaction_for_retry(&proposal.id)
            .await
            .expect("load authorized retry payload")
            .expect("mempool-accepted retry payload exists");
        assert_eq!(payload.id, proposal.id);
        assert_eq!(payload.epoch, proposal.epoch);
        assert_eq!(
            payload.proposal_hash,
            proposal.proposal_hash().expect("proposal hash")
        );
        assert_eq!(
            payload.submitted_raw_tx_id,
            withdrawal_raw_tx::submitted_raw_tx_id_base58(&proposal.transaction)
                .expect("submitted tx id")
        );
        assert_eq!(payload.raw_tx_bytes, expected_raw_tx.raw_tx_bytes);

        let stored = load_stored_row(&service, &proposal.id).await;
        assert_eq!(
            stored.authorized_transaction_name,
            Some(expected_raw_tx.tx_id_base58)
        );
        assert!(stored.authorized_transaction_jam.is_some());
        assert_eq!(stored.authorized_raw_tx, Some(payload.raw_tx_bytes));
    }

    #[tokio::test]
    async fn load_authorized_transaction_for_retry_uses_raw_tx_when_transaction_jam_missing() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(95, 0);

        mempool_accept_with_handoffs(&service, &proposal, 1, 200, None).await;
        let id = proposal.id.clone();
        let expected_raw_tx =
            withdrawal_raw_tx::persisted_raw_tx_from_transaction(&proposal.transaction)
                .expect("persisted raw tx");

        service
            .with_conn(move |conn| -> Result<(), BridgeError> {
                use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

                diesel::update(
                    sequencer_withdrawals::table
                        .filter(sequenced::withdrawal_id_as_of.eq(tip5_to_bytes(&id.as_of)))
                        .filter(
                            sequenced::withdrawal_id_base_event_id.eq(id.base_event_id.0.clone()),
                        ),
                )
                .set(sequenced::authorized_transaction_jam.eq(Option::<Vec<u8>>::None))
                .execute(conn)
                .map_err(|err| {
                    BridgeError::Runtime(format!(
                        "clear authorized_transaction_jam for retry test failed: {err}"
                    ))
                })?;
                Ok(())
            })
            .await
            .expect("clear authorized_transaction_jam");

        let payload = service
            .load_authorized_transaction_for_retry(&proposal.id)
            .await
            .expect("load authorized retry payload")
            .expect("mempool-accepted retry payload exists");
        assert_eq!(payload.raw_tx_bytes, expected_raw_tx.raw_tx_bytes);
    }

    #[tokio::test]
    async fn load_authorized_transaction_for_retry_returns_none_for_missing_withdrawal() {
        let (_dir, service) = open_service().await;

        assert!(service
            .load_authorized_transaction_for_retry(&sample_withdrawal_id(9_999))
            .await
            .expect("missing withdrawal retry load should succeed")
            .is_none());
    }

    #[tokio::test]
    async fn load_authorized_transaction_for_retry_rejects_non_mempool_accepted_state() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(91, 0);

        authorize_with_handoffs(&service, &proposal).await;

        let err = service
            .load_authorized_transaction_for_retry(&proposal.id)
            .await
            .expect_err("authorized-but-not-mempool-accepted retry load should fail");
        assert!(err
            .to_string()
            .contains("instead of mempool_accepted for orphan retry"));
    }

    #[tokio::test]
    async fn load_authorized_transaction_for_retry_rejects_missing_authorized_raw_tx_id() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(94, 0);

        mempool_accept_with_handoffs(&service, &proposal, 1, 200, None).await;

        let id = proposal.id.clone();
        service
            .with_conn(move |conn| -> Result<(), BridgeError> {
                use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

                diesel::update(
                    sequencer_withdrawals::table
                        .filter(sequenced::withdrawal_id_as_of.eq(tip5_to_bytes(&id.as_of)))
                        .filter(
                            sequenced::withdrawal_id_base_event_id.eq(id.base_event_id.0.clone()),
                        ),
                )
                .set(sequenced::authorized_transaction_name.eq(Option::<String>::None))
                .execute(conn)
                .map_err(|err| {
                    BridgeError::Runtime(format!(
                        "clear authorized raw tx id for retry test failed: {err}"
                    ))
                })?;
                Ok(())
            })
            .await
            .expect("clear authorized raw tx id");

        let err = service
            .load_authorized_transaction_for_retry(&proposal.id)
            .await
            .expect_err("missing authorized raw tx id should fail");
        assert!(err
            .to_string()
            .contains("missing authorized raw tx id for orphan retry"));
    }

    #[tokio::test]
    async fn load_authorized_transaction_for_retry_falls_back_to_transaction_jam_when_raw_tx_missing(
    ) {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(96, 0);

        mempool_accept_with_handoffs(&service, &proposal, 1, 200, None).await;
        let expected_raw_tx =
            withdrawal_raw_tx::persisted_raw_tx_from_transaction(&proposal.transaction)
                .expect("persisted raw tx");

        let id = proposal.id.clone();
        service
            .with_conn(move |conn| -> Result<(), BridgeError> {
                use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

                diesel::update(
                    sequencer_withdrawals::table
                        .filter(sequenced::withdrawal_id_as_of.eq(tip5_to_bytes(&id.as_of)))
                        .filter(
                            sequenced::withdrawal_id_base_event_id.eq(id.base_event_id.0.clone()),
                        ),
                )
                .set(sequenced::authorized_raw_tx.eq(Option::<Vec<u8>>::None))
                .execute(conn)
                .map_err(|err| {
                    BridgeError::Runtime(format!(
                        "clear authorized_raw_tx for retry test failed: {err}"
                    ))
                })?;
                Ok(())
            })
            .await
            .expect("clear authorized_raw_tx");

        let payload = service
            .load_authorized_transaction_for_retry(&proposal.id)
            .await
            .expect("load authorized retry payload")
            .expect("mempool-accepted retry payload exists");
        assert_eq!(payload.raw_tx_bytes, expected_raw_tx.raw_tx_bytes);
    }

    #[tokio::test]
    async fn load_authorized_transaction_for_retry_rejects_missing_both_raw_tx_and_transaction_jam()
    {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(97, 0);

        mempool_accept_with_handoffs(&service, &proposal, 1, 200, None).await;

        let id = proposal.id.clone();
        service
            .with_conn(move |conn| -> Result<(), BridgeError> {
                use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

                diesel::update(
                    sequencer_withdrawals::table
                        .filter(sequenced::withdrawal_id_as_of.eq(tip5_to_bytes(&id.as_of)))
                        .filter(
                            sequenced::withdrawal_id_base_event_id.eq(id.base_event_id.0.clone()),
                        ),
                )
                .set((
                    sequenced::authorized_raw_tx.eq(Option::<Vec<u8>>::None),
                    sequenced::authorized_transaction_jam.eq(Option::<Vec<u8>>::None),
                ))
                .execute(conn)
                .map_err(|err| {
                    BridgeError::Runtime(format!(
                        "clear authorized retry artifacts for retry test failed: {err}"
                    ))
                })?;
                Ok(())
            })
            .await
            .expect("clear authorized retry artifacts");

        let err = service
            .load_authorized_transaction_for_retry(&proposal.id)
            .await
            .expect_err("missing both retry artifacts should fail");
        assert!(err.to_string().contains(
            "missing both authorized_raw_tx and authorized_transaction_jam for orphan retry"
        ));
    }

    #[tokio::test]
    async fn record_mempool_retry_attempt_updates_metadata_with_journal_event() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(92, 0);

        mempool_accept_with_handoffs(&service, &proposal, 1, 200, None).await;

        let before = service
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch mempool-accepted withdrawal before retry")
            .expect("mempool-accepted withdrawal exists");
        let event_count_before = service
            .list_submission_events()
            .await
            .expect("list events before retry")
            .len();
        let reserved_before = service
            .reserved_input_names_for(&proposal.id)
            .await
            .expect("load reserved inputs before retry");
        let proposal_hash = before
            .proposal_hash
            .clone()
            .expect("authorized proposal hash");

        service
            .record_mempool_retry_attempt(
                &proposal.id,
                proposal.epoch,
                &proposal_hash,
                222,
                Some("orphan retry".to_string()),
            )
            .await
            .expect("record orphan retry metadata");

        let after = service
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch mempool-accepted withdrawal after retry")
            .expect("mempool-accepted withdrawal exists");
        let event_count_after = service
            .list_submission_events()
            .await
            .expect("list events after retry")
            .len();
        let reserved_after = service
            .reserved_input_names_for(&proposal.id)
            .await
            .expect("load reserved inputs after retry");
        assert_eq!(after.state, WithdrawalState::MempoolAccepted);
        assert_eq!(after.submit_attempt_count, before.submit_attempt_count + 1);
        assert_eq!(after.last_submit_attempt_base_height, Some(222));
        assert_eq!(after.last_submit_error.as_deref(), Some("orphan retry"));
        assert_eq!(after.proposal_hash, before.proposal_hash);
        assert_eq!(
            after.authorized_transaction_name,
            before.authorized_transaction_name
        );
        assert_eq!(after.handoff_index, before.handoff_index);
        assert_eq!(
            after.turn_started_base_height,
            before.turn_started_base_height
        );
        assert_eq!(event_count_after, event_count_before + 1);
        assert_eq!(reserved_after, reserved_before);
    }

    #[tokio::test]
    async fn record_mempool_retry_attempt_rejects_non_mempool_accepted_state() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(96, 0);

        authorize_with_handoffs(&service, &proposal).await;

        let err = service
            .record_mempool_retry_attempt(
                &proposal.id,
                proposal.epoch,
                &proposal.proposal_hash().expect("proposal hash"),
                333,
                Some("orphan retry".to_string()),
            )
            .await
            .expect_err("non-mempool-accepted retry metadata update should fail");
        assert!(err
            .to_string()
            .contains("instead of mempool_accepted during orphan retry recording"));
    }

    #[tokio::test]
    async fn record_mempool_retry_attempt_rejects_mismatched_authorized_hash() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(93, 0);

        mempool_accept_with_handoffs(&service, &proposal, 1, 200, None).await;

        let event_count_before = service
            .list_submission_events()
            .await
            .expect("list events before rejected retry")
            .len();
        let err = service
            .record_mempool_retry_attempt(
                &proposal.id,
                proposal.epoch,
                "wrong-proposal-hash",
                333,
                Some("orphan retry".to_string()),
            )
            .await
            .expect_err("mismatched authorized hash should reject retry metadata update");
        assert!(err
            .to_string()
            .contains("is not the authorized sequencer proposal during orphan retry recording"));

        let after = service
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch mempool-accepted withdrawal after rejected retry")
            .expect("mempool-accepted withdrawal exists");
        assert_eq!(after.state, WithdrawalState::MempoolAccepted);
        assert_eq!(after.submit_attempt_count, 1);
        assert_eq!(after.last_submit_attempt_base_height, Some(200));
        assert_eq!(after.last_submit_error, None);
        assert_eq!(
            service
                .list_submission_events()
                .await
                .expect("list events after rejected retry")
                .len(),
            event_count_before
        );
    }

    #[tokio::test]
    async fn canonicalized_event_drives_sequencer_state() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(1, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonicalized");

        let proposal_hash = proposal.proposal_hash().expect("proposal hash");
        let events = service.list_submission_events().await.expect("list events");
        assert_eq!(events.len(), 2);
        assert_eq!(
            events
                .iter()
                .map(|event| event.proposal_hash.as_str())
                .collect::<Vec<_>>(),
            vec!["", proposal_hash.as_str()]
        );
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_type)
                .collect::<Vec<_>>(),
            vec![
                WithdrawalSubmissionEventType::WithdrawalOrdered,
                WithdrawalSubmissionEventType::ProposalCanonicalized
            ]
        );

        let sequenced = service
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .expect("sequenced withdrawal exists");
        assert_eq!(sequenced.state, WithdrawalState::PeerCanonical);
        assert_eq!(
            sequenced.proposal_hash.as_deref(),
            Some(proposal_hash.as_str())
        );
    }

    #[tokio::test]
    async fn canonicalization_uses_base_event_identity_after_as_of_only_registration() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(101, 0);
        let mut tracked = tracked_from_proposal(&proposal, 1);
        tracked.id.as_of = sample_withdrawal_id(102).as_of;

        service
            .ensure_tracked_withdrawal_ordering(&tracked)
            .await
            .expect("record ordering with non-canonical as_of");
        let pending = service
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch by base_event_id")
            .expect("pending withdrawal exists");
        assert_eq!(pending.state, WithdrawalState::Pending);
        assert_eq!(pending.id.as_of, tracked.id.as_of);

        service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("canonicalize same Base event with proposal as_of");

        let proposal_hash = proposal.proposal_hash().expect("proposal hash");
        let sequenced = service
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch canonical withdrawal")
            .expect("sequenced withdrawal exists");
        assert_eq!(sequenced.id, proposal.id);
        assert_eq!(sequenced.state, WithdrawalState::PeerCanonical);
        assert_eq!(
            sequenced.proposal_hash.as_deref(),
            Some(proposal_hash.as_str())
        );
    }

    #[tokio::test]
    async fn peer_canonical_record_initializes_handoff_fields() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(2, 0);
        let commit_certificate = sample_commit_certificate(&proposal);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_peer_canonical_proposal(&proposal, Some(&commit_certificate), 777)
            .await
            .expect("record peer canonical proposal");

        let row = load_stored_row(&service, &proposal.id).await;
        assert_eq!(row.handoff_index, 0);
        assert_eq!(row.turn_started_base_height, Some(777));
        assert_eq!(
            row.proposal_hash.as_deref(),
            Some(proposal.proposal_hash().expect("proposal hash").as_str())
        );
        assert_eq!(
            row.peer_commit_certificate,
            Some(encode_commit_certificate(&commit_certificate).expect("encode cert"))
        );
    }

    #[tokio::test]
    async fn peer_canonical_record_preserves_authorized_state() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(3, 0);
        let commit_certificate = sample_commit_certificate(&proposal);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_peer_canonical_proposal(&proposal, Some(&commit_certificate), 555)
            .await
            .expect("record peer canonical proposal");
        service
            .record_proposal_authorized(&proposal)
            .await
            .expect("record authorized");
        service
            .record_peer_canonical_proposal(&proposal, Some(&commit_certificate), 999)
            .await
            .expect("repeat peer canonical proposal");

        let row = load_stored_row(&service, &proposal.id).await;
        assert_eq!(row.state, WithdrawalState::Authorized.as_str());
        assert_eq!(row.handoff_index, 0);
        assert_eq!(row.turn_started_base_height, Some(555));
        assert_eq!(
            row.proposal_hash.as_deref(),
            Some(proposal.proposal_hash().expect("proposal hash").as_str())
        );
    }

    #[tokio::test]
    async fn peer_canonical_replay_backfills_missing_reserved_inputs() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(30, 0);
        let commit_certificate = sample_commit_certificate(&proposal);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_peer_canonical_proposal(&proposal, Some(&commit_certificate), 555)
            .await
            .expect("record peer canonical proposal");

        let id = proposal.id.clone();
        service
            .with_conn(move |conn| -> Result<(), BridgeError> {
                clear_reserved_inputs_for_withdrawal(conn, &id)?;
                Ok(())
            })
            .await
            .expect("clear reserved inputs");

        assert!(service
            .reserved_input_names_for(&proposal.id)
            .await
            .expect("load reserved inputs after manual clear")
            .is_empty());

        service
            .record_peer_canonical_proposal(&proposal, Some(&commit_certificate), 999)
            .await
            .expect("replay peer canonical proposal");

        assert_eq!(
            service
                .reserved_input_names_for(&proposal.id)
                .await
                .expect("load reserved inputs after replay"),
            proposal.selected_inputs
        );
        let row = load_stored_row(&service, &proposal.id).await;
        assert_eq!(row.turn_started_base_height, Some(555));
    }

    #[tokio::test]
    async fn peer_canonical_replay_with_matching_reserved_inputs_is_idempotent() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(32, 0);
        let commit_certificate = sample_commit_certificate(&proposal);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_peer_canonical_proposal(&proposal, Some(&commit_certificate), 555)
            .await
            .expect("record peer canonical proposal");

        let initial_events = service.list_submission_events().await.expect("list events");
        assert_eq!(initial_events.len(), 2);
        assert_eq!(
            initial_events[0].event_type,
            WithdrawalSubmissionEventType::WithdrawalOrdered
        );
        assert_eq!(
            initial_events[1].event_type,
            WithdrawalSubmissionEventType::ProposalCanonicalized
        );
        assert_eq!(
            service
                .reserved_input_names_for(&proposal.id)
                .await
                .expect("load reserved inputs after first canonicalization"),
            proposal.selected_inputs
        );

        service
            .record_peer_canonical_proposal(&proposal, Some(&commit_certificate), 555)
            .await
            .expect("replay identical peer canonical proposal");

        assert_eq!(
            service
                .reserved_input_names_for(&proposal.id)
                .await
                .expect("load reserved inputs after identical replay"),
            proposal.selected_inputs
        );
        let replay_events = service
            .list_submission_events()
            .await
            .expect("list events after replay");
        assert_eq!(replay_events.len(), 2);
        assert_eq!(
            replay_events[0].event_type,
            WithdrawalSubmissionEventType::WithdrawalOrdered
        );
        assert_eq!(
            replay_events[1].event_type,
            WithdrawalSubmissionEventType::ProposalCanonicalized
        );
        let row = load_stored_row(&service, &proposal.id).await;
        assert_eq!(row.state, WithdrawalState::PeerCanonical.as_str());
        assert_eq!(row.turn_started_base_height, Some(555));
    }

    #[tokio::test]
    async fn peer_canonical_replay_rejects_mismatched_reserved_inputs() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(31, 0);
        let commit_certificate = sample_commit_certificate(&proposal);
        let wrong_input = sample_name(9_999);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_peer_canonical_proposal(&proposal, Some(&commit_certificate), 555)
            .await
            .expect("record peer canonical proposal");

        let id = proposal.id.clone();
        let epoch = proposal.epoch;
        let wrong_input_for_insert = wrong_input.clone();
        service
            .with_conn(move |conn| -> Result<(), BridgeError> {
                clear_reserved_inputs_for_withdrawal(conn, &id)?;
                insert_reserved_input(
                    conn,
                    &SequencerReservedInputRow {
                        id,
                        epoch,
                        input: wrong_input_for_insert,
                        created_at: 1,
                        updated_at: 1,
                    },
                )?;
                Ok(())
            })
            .await
            .expect("insert mismatched reserved input");

        let err = service
            .record_peer_canonical_proposal(&proposal, Some(&commit_certificate), 999)
            .await
            .expect_err("mismatched reserved inputs should fail");
        assert!(matches!(
            err,
            WithdrawalSequencerStoreError::Store(message)
            if message.contains("do not match pinned canonical proposal")
        ));
        assert_eq!(
            service
                .reserved_input_names_for(&proposal.id)
                .await
                .expect("load reserved inputs after mismatch"),
            vec![wrong_input]
        );
        let row = load_stored_row(&service, &proposal.id).await;
        assert_eq!(row.turn_started_base_height, Some(555));
    }

    #[tokio::test]
    async fn proposer_turn_expired_preserves_canonical_state() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(4, 0);
        let commit_certificate = sample_commit_certificate(&proposal);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_peer_canonical_proposal(&proposal, Some(&commit_certificate), 555)
            .await
            .expect("record peer canonical proposal");
        service
            .record_proposer_turn_expired(&proposal, 2, 777)
            .await
            .expect("record proposer turn expired");

        let row = load_stored_row(&service, &proposal.id).await;
        assert_eq!(row.state, WithdrawalState::PeerCanonical.as_str());
        assert_eq!(row.handoff_index, 2);
        assert_eq!(row.turn_started_base_height, Some(777));
        assert_eq!(
            row.proposal_hash.as_deref(),
            Some(proposal.proposal_hash().expect("proposal hash").as_str())
        );
        assert_eq!(
            service
                .reserved_input_names_for(&proposal.id)
                .await
                .expect("load reserved inputs after proposer handoff"),
            proposal.selected_inputs
        );
        assert_eq!(
            service
                .list_reserved_input_names()
                .await
                .expect("list reserved inputs after proposer handoff"),
            proposal.selected_inputs
        );

        let events = service.list_submission_events().await.expect("list events");
        assert_eq!(
            events
                .last()
                .expect("proposer turn expired event")
                .event_type,
            WithdrawalSubmissionEventType::ProposerTurnExpired
        );
    }

    #[tokio::test]
    async fn authorization_blocks_later_nonce_until_prior_released() {
        // A later nonce cannot even become peer-canonical while an earlier
        // nonce remains the unreleased sequencer frontier. Once the earlier
        // nonce reaches mempool acceptance, the frontier can advance.
        let (_dir, service) = open_service().await;
        let proposal_a = sample_proposal(10, 0);
        let proposal_b = sample_proposal(20, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal_a, 1))
            .await
            .expect("record first ordering");
        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal_b, 2))
            .await
            .expect("record second ordering");
        service
            .record_proposal_canonicalized(&proposal_a, 100)
            .await
            .expect("record first canonicalized");
        service
            .record_proposal_authorized(&proposal_a)
            .await
            .expect("authorize first proposal");

        let err = service
            .record_proposal_canonicalized(&proposal_b, 101)
            .await
            .expect_err("second canonicalization should wait for frontier advancement");
        assert!(matches!(
            err,
            WithdrawalSequencerStoreError::Store(ref error)
                if error.contains("record canonical proposal")
                    && error.contains("while sequencer frontier")
        ));

        service
            .record_submit_outcome(&proposal_a, WithdrawalState::MempoolAccepted, 1, 111, None)
            .await
            .expect("release first nonce from ordering");
        service
            .record_proposal_canonicalized(&proposal_b, 102)
            .await
            .expect("record second canonicalized after release");
        service
            .record_proposal_authorized(&proposal_b)
            .await
            .expect("authorize second proposal after release");
    }

    #[tokio::test]
    async fn ensure_tracked_withdrawal_ordering_materializes_pending_sequencer_row() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(40, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("repeat ordering registration");

        let sequenced = service
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .expect("sequenced withdrawal exists");
        assert_eq!(sequenced.withdrawal_nonce, Some(1));
        assert_eq!(sequenced.current_epoch, 0);
        assert_eq!(sequenced.state, WithdrawalState::Pending);
    }

    #[tokio::test]
    async fn tracked_registration_is_idempotent_for_same_base_event_with_different_as_of() {
        let (_dir, service) = open_service().await;
        let original = sample_proposal(40, 0);
        let mut same_event = tracked_from_proposal(&original, 1);
        same_event.id.as_of = sample_withdrawal_id(41).as_of;

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&original, 1))
            .await
            .expect("record original ordering");
        service
            .ensure_tracked_withdrawal_ordering(&same_event)
            .await
            .expect("same Base burn event and request facts are idempotent");

        let sequenced = service
            .fetch_sequenced_withdrawal(&same_event.id)
            .await
            .expect("fetch by same base event")
            .expect("sequenced withdrawal exists");
        assert_eq!(sequenced.withdrawal_nonce, Some(1));
        assert_eq!(sequenced.id.base_event_id, original.id.base_event_id);
    }

    #[tokio::test]
    async fn tracked_registration_rejects_same_base_event_with_different_request_facts() {
        let (_dir, service) = open_service().await;
        let original = sample_proposal(40, 0);
        let mut conflicting = tracked_from_proposal(&original, 1);
        conflicting.id.as_of = sample_withdrawal_id(41).as_of;
        conflicting.amount = conflicting.amount.saturating_add(1);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&original, 1))
            .await
            .expect("record original ordering");
        let err = service
            .ensure_tracked_withdrawal_ordering(&conflicting)
            .await
            .expect_err("same Base burn event with different facts should be rejected");
        let message = err.to_string();
        assert!(message.contains("request facts do not match"), "{message}");
    }

    #[tokio::test]
    async fn tracked_registration_rejects_request_that_sorts_before_existing_history() {
        let (_dir, service) = open_service().await;
        let later_request = tracked_from_proposal(&sample_proposal(40, 0), 1);
        let earlier_request = tracked_from_proposal(&sample_proposal(30, 0), 2);

        service
            .ensure_tracked_withdrawal_ordering(&later_request)
            .await
            .expect("record later canonical request first");
        let err = service
            .ensure_tracked_withdrawal_ordering(&earlier_request)
            .await
            .expect_err("canonical order regression should be rejected");
        let message = err.to_string();
        assert!(
            message.contains("would sort before already registered withdrawal history"),
            "{message}"
        );

        assert!(service
            .fetch_sequenced_withdrawal(&earlier_request.id)
            .await
            .expect("fetch rejected withdrawal")
            .is_none());
    }

    #[tokio::test]
    async fn canonicalized_proposal_requires_ordering() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(50, 3);

        let err = service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect_err("canonicalized proposal without ordering should fail");
        assert!(matches!(err, WithdrawalSequencerStoreError::Store(_)));
    }

    #[tokio::test]
    async fn authorized_submit_failure_keeps_next_pending_ordering_blocked() {
        let (_dir, service) = open_service().await;
        let proposal_a = sample_proposal(10, 0);
        let proposal_b = sample_proposal(20, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal_a, 1))
            .await
            .expect("record first ordering");
        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal_b, 2))
            .await
            .expect("record second ordering");
        service
            .record_proposal_canonicalized(&proposal_a, 100)
            .await
            .expect("record canonicalized");
        service
            .record_proposal_authorized(&proposal_a)
            .await
            .expect("record authorized");
        service
            .record_submit_outcome(
                &proposal_a,
                WithdrawalState::Authorized,
                2,
                222,
                Some("submit failed".to_string()),
            )
            .await
            .expect("record submit failure");

        let next = service
            .next_pending_withdrawal_ordering()
            .await
            .expect("next pending ordering")
            .expect("pending ordering exists");
        assert_eq!(next.0, proposal_a.id);
        assert_eq!(next.1, 1);
    }

    #[tokio::test]
    async fn mempool_accepted_releases_next_pending_ordering() {
        let (_dir, service) = open_service().await;
        let proposal_a = sample_proposal(10, 0);
        let proposal_b = sample_proposal(20, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal_a, 1))
            .await
            .expect("record first ordering");
        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal_b, 2))
            .await
            .expect("record second ordering");
        service
            .record_proposal_canonicalized(&proposal_a, 100)
            .await
            .expect("record canonicalized");
        service
            .record_proposal_authorized(&proposal_a)
            .await
            .expect("record authorized");
        service
            .record_submit_outcome(&proposal_a, WithdrawalState::MempoolAccepted, 1, 111, None)
            .await
            .expect("record mempool accepted");

        let next = service
            .next_pending_withdrawal_ordering()
            .await
            .expect("next pending ordering")
            .expect("pending ordering exists");
        assert_eq!(next.0, proposal_b.id);
        assert_eq!(next.1, 2);
        assert_eq!(
            service
                .list_reserved_input_names()
                .await
                .expect("list reserved inputs after mempool acceptance"),
            proposal_a.selected_inputs
        );
    }

    #[tokio::test]
    async fn current_live_withdrawal_nonce_empty_sequencer_returns_none() {
        // An empty sequencer has no unreleased withdrawal nonce to expose as
        // the ordering frontier.
        let (_dir, service) = open_service().await;

        let frontier = service
            .current_live_withdrawal_nonce()
            .await
            .expect("current live withdrawal nonce");

        assert_eq!(frontier, None);
    }

    #[tokio::test]
    async fn current_live_withdrawal_nonce_returns_pending_nonce() {
        // A newly registered Pending row is unreleased, so it is the frontier.
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(21, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record pending ordering");

        assert_eq!(
            service
                .current_live_withdrawal_nonce()
                .await
                .expect("current live withdrawal nonce"),
            Some(1)
        );
    }

    #[tokio::test]
    async fn current_live_withdrawal_nonce_returns_peer_canonical_nonce() {
        // PeerCanonical rows still block ordering, so the frontier stays on
        // that nonce until submission reaches a released state.
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(22, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonicalized");

        assert_eq!(
            service
                .current_live_withdrawal_nonce()
                .await
                .expect("current live withdrawal nonce"),
            Some(1)
        );
    }

    #[tokio::test]
    async fn current_live_withdrawal_nonce_returns_authorized_nonce() {
        // Authorized rows remain unreleased because the sequencer has not yet
        // observed mempool acceptance or confirmation.
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(23, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonicalized");
        service
            .record_proposal_authorized(&proposal)
            .await
            .expect("record authorized");

        assert_eq!(
            service
                .current_live_withdrawal_nonce()
                .await
                .expect("current live withdrawal nonce"),
            Some(1)
        );
    }

    #[tokio::test]
    async fn current_live_withdrawal_nonce_skips_mempool_accepted_nonce() {
        // MempoolAccepted releases its nonce under the default ordering rule,
        // so the frontier advances to the next unreleased row.
        let (_dir, service) = open_service().await;
        let proposal_a = sample_proposal(24, 0);
        let proposal_b = sample_proposal(25, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal_a, 1))
            .await
            .expect("record first ordering");
        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal_b, 2))
            .await
            .expect("record second ordering");
        service
            .record_proposal_canonicalized(&proposal_a, 100)
            .await
            .expect("record canonicalized");
        service
            .record_proposal_authorized(&proposal_a)
            .await
            .expect("record authorized");
        service
            .record_submit_outcome(&proposal_a, WithdrawalState::MempoolAccepted, 1, 111, None)
            .await
            .expect("record mempool accepted");

        assert_eq!(
            service
                .current_live_withdrawal_nonce()
                .await
                .expect("current live withdrawal nonce"),
            Some(2)
        );
    }

    #[tokio::test]
    async fn current_live_withdrawal_nonce_skips_confirmed_nonce() {
        // Confirmed rows are released permanently and do not become the
        // current live frontier.
        let (_dir, service) = open_service().await;
        let proposal_a = sample_proposal(26, 0);
        let proposal_b = sample_proposal(27, 0);
        let confirmed_block_id = Tip5Hash([Belt(901), Belt(902), Belt(903), Belt(904), Belt(905)]);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal_a, 1))
            .await
            .expect("record first ordering");
        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal_b, 2))
            .await
            .expect("record second ordering");
        service
            .record_proposal_canonicalized(&proposal_a, 100)
            .await
            .expect("record canonicalized");
        service
            .record_proposal_authorized(&proposal_a)
            .await
            .expect("record authorized");
        service
            .record_submit_outcome(&proposal_a, WithdrawalState::MempoolAccepted, 1, 111, None)
            .await
            .expect("record mempool accepted");
        service
            .record_tx_confirmed(&proposal_a, 777, confirmed_block_id)
            .await
            .expect("record confirmed");

        assert_eq!(
            service
                .current_live_withdrawal_nonce()
                .await
                .expect("current live withdrawal nonce"),
            Some(2)
        );
    }

    #[tokio::test]
    async fn current_live_withdrawal_nonce_lowest_unreleased_nonce_wins() {
        // Released lower rows are skipped, but among unreleased rows the
        // smallest nonce is the single sequencer frontier.
        let (_dir, service) = open_service().await;
        let proposal_a = sample_proposal(28, 0);
        let proposal_b = sample_proposal(29, 0);
        let proposal_c = sample_proposal(30, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal_a, 1))
            .await
            .expect("record first ordering");
        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal_b, 2))
            .await
            .expect("record second ordering");
        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal_c, 3))
            .await
            .expect("record third ordering");
        service
            .record_proposal_canonicalized(&proposal_a, 100)
            .await
            .expect("record first canonicalized");
        service
            .record_proposal_authorized(&proposal_a)
            .await
            .expect("record first authorized");
        service
            .record_submit_outcome(&proposal_a, WithdrawalState::MempoolAccepted, 1, 111, None)
            .await
            .expect("record first mempool accepted");
        service
            .record_proposal_canonicalized(&proposal_b, 101)
            .await
            .expect("record second canonicalized");
        service
            .record_proposal_authorized(&proposal_b)
            .await
            .expect("record second authorized");

        assert_eq!(
            service
                .current_live_withdrawal_nonce()
                .await
                .expect("current live withdrawal nonce"),
            Some(2)
        );
    }

    #[tokio::test]
    async fn current_live_withdrawal_nonce_does_not_return_rows_above_lowest_unreleased() {
        // A future unreleased row is passive while a lower unreleased nonce is
        // still live at the sequencer.
        let (_dir, service) = open_service().await;
        let proposal_a = sample_proposal(31, 0);
        let proposal_b = sample_proposal(32, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal_a, 1))
            .await
            .expect("record first ordering");
        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal_b, 2))
            .await
            .expect("record second ordering");

        assert_eq!(
            service
                .current_live_withdrawal_nonce()
                .await
                .expect("current live withdrawal nonce"),
            Some(1)
        );
    }

    #[tokio::test]
    async fn canonicalization_rejects_non_frontier_with_lower_pending_nonce() {
        // The sequencer may register higher nonce rows as Pending, but it must
        // not let them advance to PeerCanonical until every lower unreleased
        // nonce has been released from ordering.
        let (_dir, service) = open_service().await;
        let proposal_a = sample_proposal(32, 0);
        let proposal_b = sample_proposal(33, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal_a, 1))
            .await
            .expect("record first ordering");
        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal_b, 2))
            .await
            .expect("record second ordering");

        let err = service
            .record_proposal_canonicalized(&proposal_b, 100)
            .await
            .expect_err("future nonce canonicalization should be rejected");
        assert!(matches!(
            err,
            WithdrawalSequencerStoreError::Store(ref message)
                if message.contains("record canonical proposal")
                    && message.contains("while sequencer frontier")
        ));

        let future = service
            .fetch_sequenced_withdrawal(&proposal_b.id)
            .await
            .expect("fetch future withdrawal")
            .expect("future withdrawal remains registered");
        assert_eq!(future.state, WithdrawalState::Pending);
        assert_eq!(future.proposal_hash, None);
    }

    #[tokio::test]
    async fn frontier_allows_withdrawal_requires_registered_frontier_id() {
        // The direct frontier check succeeds only when the requested id is both
        // registered at the sequencer and the current unreleased frontier row.
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(33, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");

        let check = service
            .frontier_allows_withdrawal(&proposal.id)
            .await
            .expect("frontier check");

        assert!(check.registered);
        assert!(check.is_frontier);
        assert!(check.allowed());
    }

    #[tokio::test]
    async fn frontier_allows_withdrawal_rejects_unregistered_id() {
        // An unregistered withdrawal id is never active, even when the
        // sequencer has no current frontier.
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(34, 0);

        let check = service
            .frontier_allows_withdrawal(&proposal.id)
            .await
            .expect("frontier check");

        assert!(!check.registered);
        assert!(!check.is_frontier);
        assert!(!check.allowed());
    }

    #[tokio::test]
    async fn frontier_allows_withdrawal_rejects_registered_non_frontier_id() {
        // Registered future rows remain passive until lower unreleased nonces
        // leave the frontier.
        let (_dir, service) = open_service().await;
        let proposal_a = sample_proposal(35, 0);
        let proposal_b = sample_proposal(36, 0);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal_a, 1))
            .await
            .expect("record first ordering");
        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal_b, 2))
            .await
            .expect("record second ordering");

        let check = service
            .frontier_allows_withdrawal(&proposal_b.id)
            .await
            .expect("frontier check");

        assert!(check.registered);
        assert!(!check.is_frontier);
        assert!(!check.allowed());
    }

    #[tokio::test]
    async fn canonicalization_rejects_inputs_reserved_by_other_withdrawal() {
        let (_dir, service) = open_service().await;
        let proposal_a = sample_proposal(60, 0);
        let mut proposal_b = sample_proposal(61, 0);
        proposal_b.transaction = proposal_a.transaction.clone();
        proposal_b.selected_inputs = proposal_a.selected_inputs.clone();

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal_a, 1))
            .await
            .expect("record first ordering");
        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal_b, 2))
            .await
            .expect("record second ordering");
        service
            .record_proposal_canonicalized(&proposal_a, 100)
            .await
            .expect("record first canonicalized proposal");
        service
            .record_proposal_authorized(&proposal_a)
            .await
            .expect("record first authorized proposal");
        service
            .record_submit_outcome(&proposal_a, WithdrawalState::MempoolAccepted, 1, 111, None)
            .await
            .expect("record first mempool acceptance");

        let err = service
            .record_proposal_canonicalized(&proposal_b, 200)
            .await
            .expect_err("canonicalization should reject conflicting reserved inputs");
        assert!(matches!(
            err,
            WithdrawalSequencerStoreError::Store(ref message)
                if message.contains("already reserved by withdrawal")
        ));

        let blocked = service
            .fetch_sequenced_withdrawal(&proposal_b.id)
            .await
            .expect("fetch blocked withdrawal")
            .expect("blocked withdrawal remains sequenced");
        assert_eq!(blocked.state, WithdrawalState::Pending);
        assert_eq!(blocked.current_epoch, 0);
        assert_eq!(blocked.proposal_hash, None);
        assert_eq!(
            service
                .list_reserved_input_names()
                .await
                .expect("list reserved inputs after canonical conflict"),
            proposal_a.selected_inputs
        );
    }

    #[tokio::test]
    async fn later_nonce_may_confirm_before_earlier_nonce_after_mempool_acceptance() {
        let (_dir, service) = open_service().await;
        let proposal_a = sample_proposal(10, 0);
        let proposal_b = sample_proposal(20, 0);
        let block_a = Tip5Hash([Belt(900), Belt(901), Belt(902), Belt(903), Belt(904)]);
        let block_b = Tip5Hash([Belt(910), Belt(911), Belt(912), Belt(913), Belt(914)]);

        for (proposal, nonce) in [(&proposal_a, 1_u64), (&proposal_b, 2_u64)] {
            service
                .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(proposal, nonce))
                .await
                .expect("record ordering");
            service
                .record_proposal_canonicalized(proposal, 100)
                .await
                .expect("record canonicalized");
            service
                .record_proposal_authorized(proposal)
                .await
                .expect("record authorized");
            service
                .record_submit_outcome(proposal, WithdrawalState::MempoolAccepted, 1, 111, None)
                .await
                .expect("record mempool accepted");
        }

        service
            .record_tx_confirmed(&proposal_b, 777, block_b.clone())
            .await
            .expect("confirm later nonce first");

        let earlier = service
            .fetch_sequenced_withdrawal(&proposal_a.id)
            .await
            .expect("fetch earlier withdrawal")
            .expect("earlier withdrawal exists");
        let later = service
            .fetch_sequenced_withdrawal(&proposal_b.id)
            .await
            .expect("fetch later withdrawal")
            .expect("later withdrawal exists");
        assert_eq!(earlier.state, WithdrawalState::MempoolAccepted);
        assert_eq!(later.state, WithdrawalState::Confirmed);

        service
            .record_tx_confirmed(&proposal_a, 778, block_a.clone())
            .await
            .expect("confirm earlier nonce second");

        let earlier = service
            .fetch_sequenced_withdrawal(&proposal_a.id)
            .await
            .expect("fetch earlier withdrawal after confirm")
            .expect("earlier withdrawal exists");
        assert_eq!(earlier.state, WithdrawalState::Confirmed);
    }

    #[tokio::test]
    async fn confirmed_withdrawal_clears_reserved_inputs_and_keeps_sequencer_state() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(30, 1);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonicalized");
        service
            .record_proposal_authorized(&proposal)
            .await
            .expect("record authorized");
        service
            .record_submit_outcome(&proposal, WithdrawalState::MempoolAccepted, 1, 111, None)
            .await
            .expect("record mempool-accepted submit outcome");
        let confirmed_block_id = Tip5Hash([Belt(900), Belt(901), Belt(902), Belt(903), Belt(904)]);
        service
            .record_tx_confirmed(&proposal, 777, confirmed_block_id.clone())
            .await
            .expect("record confirmed");

        let sequenced = service
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .expect("sequenced withdrawal exists");
        assert_eq!(sequenced.state, WithdrawalState::Confirmed);

        let event_types = service
            .list_submission_events()
            .await
            .expect("list events")
            .into_iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>();
        assert_eq!(
            event_types,
            vec![
                WithdrawalSubmissionEventType::WithdrawalOrdered,
                WithdrawalSubmissionEventType::ProposalCanonicalized,
                WithdrawalSubmissionEventType::ProposalAuthorized,
                WithdrawalSubmissionEventType::TxSubmitted,
                WithdrawalSubmissionEventType::TxSeenMempoolAccepted,
                WithdrawalSubmissionEventType::TxConfirmed,
            ]
        );
        let sequenced = service
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal after confirmation")
            .expect("sequenced withdrawal remains after confirmation");
        assert_eq!(sequenced.state, WithdrawalState::Confirmed);
        assert!(service
            .reserved_input_names_for(&proposal.id)
            .await
            .expect("load reserved inputs after confirmation")
            .is_empty());
        assert!(service
            .list_reserved_input_names()
            .await
            .expect("list reserved inputs after confirmation")
            .is_empty());
    }

    #[tokio::test]
    async fn mempool_accepted_updates_sequencer_state_before_confirmation() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(35, 1);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonicalized");
        service
            .record_proposal_authorized(&proposal)
            .await
            .expect("record authorized");
        service
            .record_submit_outcome(&proposal, WithdrawalState::MempoolAccepted, 2, 222, None)
            .await
            .expect("record mempool accepted");

        let sequenced = service
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .expect("sequenced withdrawal exists");
        assert_eq!(sequenced.state, WithdrawalState::MempoolAccepted);

        let event_types = service
            .list_submission_events()
            .await
            .expect("list events")
            .into_iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>();
        assert_eq!(
            event_types,
            vec![
                WithdrawalSubmissionEventType::WithdrawalOrdered,
                WithdrawalSubmissionEventType::ProposalCanonicalized,
                WithdrawalSubmissionEventType::ProposalAuthorized,
                WithdrawalSubmissionEventType::TxSubmitted,
                WithdrawalSubmissionEventType::TxSeenMempoolAccepted,
            ]
        );
    }

    #[tokio::test]
    async fn confirmed_sequencer_state_persists_without_rebuild() {
        let (_dir, service) = open_service().await;
        let proposal = sample_proposal(60, 2);
        let confirmed_block_id = Tip5Hash([Belt(990), Belt(991), Belt(992), Belt(993), Belt(994)]);

        service
            .ensure_tracked_withdrawal_ordering(&tracked_from_proposal(&proposal, 1))
            .await
            .expect("record ordering");
        service
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("record canonicalized");
        service
            .record_proposal_authorized(&proposal)
            .await
            .expect("record authorized");
        service
            .record_submit_outcome(&proposal, WithdrawalState::MempoolAccepted, 1, 333, None)
            .await
            .expect("record mempool-accepted submit outcome");
        service
            .record_tx_confirmed(&proposal, 888, confirmed_block_id.clone())
            .await
            .expect("record confirmed");

        let sequenced = service
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .expect("confirmed sequenced withdrawal exists");
        let proposal_hash = proposal.proposal_hash().expect("proposal hash");
        assert_eq!(sequenced.state, WithdrawalState::Confirmed);
        assert_eq!(
            sequenced.proposal_hash.as_deref(),
            Some(proposal_hash.as_str())
        );
    }
}
