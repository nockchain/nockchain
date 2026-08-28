use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, B256};
use deadpool_diesel::sqlite::Pool;
use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Binary, Nullable, Text};
use diesel::sqlite::SqliteConnection;

use crate::shared::errors::BridgeError;
use crate::shared::types::{BaseEventId, Tip5Hash};
use crate::withdrawal::sequencer::base_incidents::{
    ensure_base_incident_schema, BaseIncidentStore,
};

const SQLITE_BUSY_TIMEOUT_MS: u64 = 2_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedBaseWithdrawalBurn {
    pub chain_id: u64,
    pub nock_contract_address: Address,
    pub base_event_id: BaseEventId,
    pub block_number: u64,
    pub block_hash: B256,
    pub parent_hash: B256,
    pub observed_at_unix_secs: Option<u64>,
    pub tx_hash: B256,
    pub tx_index: u64,
    pub log_index: u64,
    pub burner: Address,
    pub amount_base_units: String,
    pub amount_nicks: u64,
    pub lock_root: Tip5Hash,
    pub calldata: Vec<u8>,
    pub base_batch_end: u64,
    pub withdrawal_nonce: Option<u64>,
    pub verified_at: i64,
    pub policy_id: Option<String>,
    pub protocol_id: Option<String>,
}

impl VerifiedBaseWithdrawalBurn {
    pub fn identity(&self) -> (u64, Address, &BaseEventId) {
        (
            self.chain_id, self.nock_contract_address, &self.base_event_id,
        )
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicBaseWithdrawalBurn {
    pub burn: VerifiedBaseWithdrawalBurn,
    pub canonical: bool,
    pub invalidated_at: Option<i64>,
    pub invalidation_generation: Option<u64>,
    pub invalidation_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseActivityCursor {
    pub chain_id: u64,
    pub nock_contract_address: Address,
    pub last_verified_block: u64,
    pub last_verified_block_hash: B256,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseActivityHeaderCheckpoint {
    pub chain_id: u64,
    pub nock_contract_address: Address,
    pub block_number: u64,
    pub block_hash: B256,
    pub parent_hash: B256,
    pub block_timestamp: u64,
    pub verified_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseActivityReorgPlan {
    pub chain_id: u64,
    pub nock_contract_address: Address,
    pub old_cursor: BaseActivityCursor,
    pub common_ancestor: BaseActivityHeaderCheckpoint,
    pub canonical_cursor_header: BaseActivityHeaderCheckpoint,
    pub rewind_depth: u64,
    pub detected_at: i64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseActivityPageCursor {
    pub snapshot_revision: u64,
    pub snapshot_rowid: u64,
    pub last_block_number: u64,
    pub last_log_index: u64,
    pub last_base_event_id: BaseEventId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseActivityBurnPage {
    pub burns: Vec<PublicBaseWithdrawalBurn>,
    pub snapshot_rowid: u64,
}

#[derive(Clone)]
pub struct BaseActivityStore {
    pool: Pool,
}

impl BaseActivityStore {
    pub(crate) fn new(pool: Pool) -> Self {
        Self { pool }
    }
    pub fn incident_store(&self) -> BaseIncidentStore {
        BaseIncidentStore::new(self.pool.clone())
    }

    pub async fn insert_verified_burn(
        &self,
        record: VerifiedBaseWithdrawalBurn,
    ) -> Result<bool, BridgeError> {
        self.with_conn(move |conn| {
            conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                Ok(insert_verified_burn_checked(conn, &record)?)
            })
            .map_err(|error| {
                BridgeError::Runtime(format!("Base activity insert transaction failed: {error}"))
            })
        })
        .await
    }

    /// Atomically persists one fully verified block chunk and advances the
    /// canonical cursor. Overlap chunks may retain a newer existing cursor.
    pub async fn apply_verified_chunk(
        &self,
        records: Vec<VerifiedBaseWithdrawalBurn>,
        cursor: BaseActivityCursor,
    ) -> Result<u64, BridgeError> {
        self.apply_verified_chunk_with_headers(records, Vec::new(), cursor, 0)
            .await
    }

    pub async fn apply_verified_chunk_with_headers(
        &self,
        records: Vec<VerifiedBaseWithdrawalBurn>,
        headers: Vec<BaseActivityHeaderCheckpoint>,
        cursor: BaseActivityCursor,
        retained_from_block: u64,
    ) -> Result<u64, BridgeError> {
        self.with_conn(move |conn| {
            conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                let existing =
                    load_cursor_row(conn, cursor.chain_id, cursor.nock_contract_address)?;
                if let Some(existing) = &existing {
                    if cursor.last_verified_block == existing.last_verified_block
                        && cursor.last_verified_block_hash != existing.last_verified_block_hash
                    {
                        return Err(BridgeError::Runtime(format!(
                            "Base activity cursor hash mismatch at block {}",
                            cursor.last_verified_block
                        ))
                        .into());
                    }
                }
                let effective_ceiling = existing
                    .as_ref()
                    .map(|existing| existing.last_verified_block)
                    .unwrap_or_default()
                    .max(cursor.last_verified_block);
                for header in &headers {
                    if header.chain_id != cursor.chain_id
                        || header.nock_contract_address != cursor.nock_contract_address
                    {
                        return Err(BridgeError::Runtime(format!(
                            "Base header checkpoint {} does not belong to cursor chain/deployment",
                            header.block_number
                        ))
                        .into());
                    }
                    if header.block_number > effective_ceiling {
                        return Err(BridgeError::Runtime(format!(
                            "Base header checkpoint {} exceeds verified cursor ceiling {}",
                            header.block_number, effective_ceiling
                        ))
                        .into());
                    }
                    insert_header_checkpoint_checked(conn, header)?;
                }
                if !headers.is_empty() {
                    prune_header_checkpoints_before(
                        conn, cursor.chain_id, cursor.nock_contract_address, retained_from_block,
                    )?;
                }
                let mut inserted = 0u64;
                for record in &records {
                    if record.chain_id != cursor.chain_id
                        || record.nock_contract_address != cursor.nock_contract_address
                    {
                        return Err(BridgeError::Runtime(format!(
                            "Base burn {:?} does not belong to cursor chain/deployment",
                            record.base_event_id
                        ))
                        .into());
                    }
                    if record.block_number > effective_ceiling {
                        return Err(BridgeError::Runtime(format!(
                            "Base burn {:?} at block {} is beyond verified cursor ceiling {}",
                            record.base_event_id, record.block_number, effective_ceiling
                        ))
                        .into());
                    }
                    if insert_verified_burn_checked(conn, record)? {
                        inserted = inserted.saturating_add(1);
                    }
                }
                if existing.as_ref().is_none_or(|existing| {
                    cursor.last_verified_block > existing.last_verified_block
                }) {
                    upsert_cursor_row(conn, &cursor)?;
                }
                Ok(inserted)
            })
            .map_err(|error| {
                BridgeError::Runtime(format!("Base activity chunk transaction failed: {error}"))
            })
        })
        .await
    }

    pub async fn load_verified_burn(
        &self,
        chain_id: u64,
        nock_contract_address: Address,
        base_event_id: &BaseEventId,
    ) -> Result<Option<VerifiedBaseWithdrawalBurn>, BridgeError> {
        let base_event_id = base_event_id.clone();
        self.with_conn(move |conn| {
            load_verified_burn_row(conn, chain_id, nock_contract_address, &base_event_id)
        })
        .await
    }
    pub async fn load_public_burn(
        &self,
        chain_id: u64,
        nock_contract_address: Address,
        base_event_id: &BaseEventId,
    ) -> Result<Option<PublicBaseWithdrawalBurn>, BridgeError> {
        let base_event_id = base_event_id.clone();
        self.with_conn(move |conn| {
            load_public_burn_row(conn, chain_id, nock_contract_address, &base_event_id)
        })
        .await
    }

    pub async fn load_verified_burn_by_tx_log(
        &self,
        chain_id: u64,
        nock_contract_address: Address,
        tx_hash: B256,
        log_index: u64,
    ) -> Result<Option<VerifiedBaseWithdrawalBurn>, BridgeError> {
        self.with_conn(move |conn| {
            load_verified_burn_by_tx_log_row(
                conn, chain_id, nock_contract_address, tx_hash, log_index,
            )
        })
        .await
    }
    pub async fn load_public_burn_by_tx_log(
        &self,
        chain_id: u64,
        nock_contract_address: Address,
        tx_hash: B256,
        log_index: u64,
    ) -> Result<Option<PublicBaseWithdrawalBurn>, BridgeError> {
        self.with_conn(move |conn| {
            load_public_burn_by_tx_log_row(
                conn, chain_id, nock_contract_address, tx_hash, log_index,
            )
        })
        .await
    }

    pub async fn list_verified_burns_by_tx_hash(
        &self,
        chain_id: u64,
        nock_contract_address: Address,
        tx_hash: B256,
    ) -> Result<Vec<VerifiedBaseWithdrawalBurn>, BridgeError> {
        self.with_conn(move |conn| {
            load_verified_burn_rows_by_tx_hash(conn, chain_id, nock_contract_address, tx_hash)
        })
        .await
    }

    pub async fn list_verified_burns_by_burner(
        &self,
        chain_id: u64,
        nock_contract_address: Address,
        burner: Address,
        limit: u32,
    ) -> Result<Vec<VerifiedBaseWithdrawalBurn>, BridgeError> {
        self.with_conn(move |conn| {
            load_verified_burn_rows_by_burner(conn, chain_id, nock_contract_address, burner, limit)
        })
        .await
    }

    pub async fn list_public_burns_by_tx_hash(
        &self,
        chain_id: u64,
        nock_contract_address: Address,
        tx_hash: B256,
    ) -> Result<Vec<PublicBaseWithdrawalBurn>, BridgeError> {
        self.with_conn(move |conn| {
            load_public_burn_rows_by_tx_hash(conn, chain_id, nock_contract_address, tx_hash)
        })
        .await
    }

    pub async fn list_public_burns_by_burner_page(
        &self,
        chain_id: u64,
        nock_contract_address: Address,
        burner: Address,
        cursor: Option<BaseActivityPageCursor>,
        limit: u32,
    ) -> Result<BaseActivityBurnPage, BridgeError> {
        self.with_conn(move |conn| {
            load_verified_burn_page_by_burner(
                conn,
                chain_id,
                nock_contract_address,
                burner,
                cursor.as_ref(),
                limit,
            )
        })
        .await
    }
    pub async fn list_unmapped_verified_burns(
        &self,
        chain_id: u64,
        nock_contract_address: Address,
        minimum_block: u64,
    ) -> Result<Vec<VerifiedBaseWithdrawalBurn>, BridgeError> {
        self.with_conn(move |conn| {
            load_unmapped_verified_burn_rows(conn, chain_id, nock_contract_address, minimum_block)
        })
        .await
    }
    pub async fn load_cursor(
        &self,
        chain_id: u64,
        nock_contract_address: Address,
    ) -> Result<Option<BaseActivityCursor>, BridgeError> {
        self.with_conn(move |conn| load_cursor_row(conn, chain_id, nock_contract_address))
            .await
    }

    pub async fn load_header_checkpoints(
        &self,
        chain_id: u64,
        nock_contract_address: Address,
        start_block: u64,
        end_block: u64,
    ) -> Result<Vec<BaseActivityHeaderCheckpoint>, BridgeError> {
        self.with_conn(move |conn| {
            load_header_checkpoint_rows(
                conn, chain_id, nock_contract_address, start_block, end_block,
            )
        })
        .await
    }

    pub async fn advance_cursor(&self, cursor: BaseActivityCursor) -> Result<(), BridgeError> {
        self.with_conn(move |conn| {
            conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                if let Some(existing) =
                    load_cursor_row(conn, cursor.chain_id, cursor.nock_contract_address)?
                {
                    if cursor.last_verified_block < existing.last_verified_block {
                        return Err(BridgeError::Runtime(format!(
                            "Base activity cursor cannot rewind from {} to {}",
                            existing.last_verified_block, cursor.last_verified_block
                        ))
                        .into());
                    }
                    if cursor.last_verified_block == existing.last_verified_block
                        && cursor.last_verified_block_hash != existing.last_verified_block_hash
                    {
                        return Err(BridgeError::Runtime(format!(
                            "Base activity cursor hash mismatch at block {}",
                            cursor.last_verified_block
                        ))
                        .into());
                    }
                }
                upsert_cursor_row(conn, &cursor)?;
                Ok(())
            })
            .map_err(|error| {
                BridgeError::Runtime(format!("Base activity cursor transaction failed: {error}"))
            })
        })
        .await
    }

    async fn with_conn<T, F>(&self, operation: F) -> Result<T, BridgeError>
    where
        F: FnOnce(&mut SqliteConnection) -> Result<T, BridgeError> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.pool.get().await.map_err(|error| {
            BridgeError::Runtime(format!("Base activity store pool failed: {error}"))
        })?;
        conn.interact(move |conn| {
            conn.batch_execute(&format!(
                "PRAGMA busy_timeout = {SQLITE_BUSY_TIMEOUT_MS}; PRAGMA foreign_keys = ON;"
            ))
            .map_err(|error| {
                BridgeError::Runtime(format!("Base activity store pragma failed: {error}"))
            })?;
            operation(conn)
        })
        .await
        .map_err(|error| {
            BridgeError::Runtime(format!("Base activity store interact failed: {error}"))
        })?
    }
}

pub(crate) fn ensure_base_activity_schema(conn: &mut SqliteConnection) -> Result<(), BridgeError> {
    conn.batch_execute(
        r#"
        CREATE TABLE IF NOT EXISTS sequencer_base_burns (
            chain_id INTEGER NOT NULL,
            nock_contract_address BLOB NOT NULL CHECK(length(nock_contract_address) = 20),
            base_event_id BLOB NOT NULL CHECK(length(base_event_id) = 32),
            block_number INTEGER NOT NULL,
            block_hash BLOB NOT NULL CHECK(length(block_hash) = 32),
            parent_hash BLOB NOT NULL CHECK(length(parent_hash) = 32),
            tx_hash BLOB NOT NULL CHECK(length(tx_hash) = 32),
            tx_index INTEGER NOT NULL,
            log_index INTEGER NOT NULL,
            burner BLOB NOT NULL CHECK(length(burner) = 20),
            amount_base_units TEXT NOT NULL,
            amount_nicks TEXT NOT NULL,
            lock_root BLOB NOT NULL CHECK(length(lock_root) = 40),
            calldata BLOB NOT NULL,
            base_batch_end INTEGER NOT NULL,
            withdrawal_nonce INTEGER,
            verified_at INTEGER NOT NULL,
            observed_at_unix_secs INTEGER,
            policy_id TEXT,
            protocol_id TEXT,
            canonical INTEGER NOT NULL DEFAULT 1,
            invalidated_at INTEGER,
            invalidation_generation INTEGER,
            invalidation_reason TEXT,
            PRIMARY KEY (chain_id, nock_contract_address, base_event_id)
        );

        CREATE UNIQUE INDEX IF NOT EXISTS sequencer_base_burns_by_log
          ON sequencer_base_burns(chain_id, nock_contract_address, tx_hash, log_index);
        CREATE INDEX IF NOT EXISTS sequencer_base_burns_by_ordering
          ON sequencer_base_burns(chain_id, nock_contract_address, base_batch_end, base_event_id);
        CREATE INDEX IF NOT EXISTS sequencer_base_burns_by_nonce
          ON sequencer_base_burns(chain_id, nock_contract_address, withdrawal_nonce);
        CREATE INDEX IF NOT EXISTS sequencer_base_burns_by_burner
          ON sequencer_base_burns(
            chain_id, nock_contract_address, burner,
            block_number DESC, log_index DESC, base_event_id DESC
          );


        CREATE TABLE IF NOT EXISTS sequencer_base_activity_cursor (
            chain_id INTEGER NOT NULL,
            nock_contract_address BLOB NOT NULL CHECK(length(nock_contract_address) = 20),
            last_verified_block INTEGER NOT NULL,
            last_verified_block_hash BLOB NOT NULL CHECK(length(last_verified_block_hash) = 32),
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (chain_id, nock_contract_address)
        );

        CREATE TABLE IF NOT EXISTS sequencer_base_header_checkpoints (
            chain_id INTEGER NOT NULL,
            nock_contract_address BLOB NOT NULL CHECK(length(nock_contract_address) = 20),
            block_number INTEGER NOT NULL,
            block_hash BLOB NOT NULL CHECK(length(block_hash) = 32),
            parent_hash BLOB NOT NULL CHECK(length(parent_hash) = 32),
            block_timestamp INTEGER NOT NULL,
            verified_at INTEGER NOT NULL,
            PRIMARY KEY (chain_id, nock_contract_address, block_number)
        );

        CREATE TABLE IF NOT EXISTS sequencer_base_reorg_events (
            event_id INTEGER PRIMARY KEY AUTOINCREMENT,
            chain_id INTEGER NOT NULL,
            nock_contract_address BLOB NOT NULL CHECK(length(nock_contract_address) = 20),
            generation INTEGER NOT NULL,
            detected_at INTEGER NOT NULL,
            old_cursor_block INTEGER NOT NULL,
            old_cursor_hash BLOB NOT NULL CHECK(length(old_cursor_hash) = 32),
            common_ancestor_block INTEGER,
            common_ancestor_hash BLOB,
            canonical_tip_block INTEGER NOT NULL,
            canonical_tip_hash BLOB NOT NULL CHECK(length(canonical_tip_hash) = 32),
            rewind_depth INTEGER,
            outcome TEXT NOT NULL,
            reason TEXT NOT NULL,
            UNIQUE (chain_id, nock_contract_address, generation)
        );

        CREATE TABLE IF NOT EXISTS sequencer_base_burn_invalidations (
            event_id INTEGER PRIMARY KEY AUTOINCREMENT,
            chain_id INTEGER NOT NULL,
            nock_contract_address BLOB NOT NULL CHECK(length(nock_contract_address) = 20),
            generation INTEGER NOT NULL,
            base_event_id BLOB NOT NULL CHECK(length(base_event_id) = 32),
            old_block_number INTEGER NOT NULL,
            old_block_hash BLOB NOT NULL CHECK(length(old_block_hash) = 32),
            lifecycle_state TEXT,
            invalidated_at INTEGER NOT NULL,
            reason TEXT NOT NULL,
            UNIQUE (
                chain_id, nock_contract_address, generation, base_event_id
            )
        );

        CREATE TABLE IF NOT EXISTS sequencer_base_reorg_guard (
            chain_id INTEGER NOT NULL,
            nock_contract_address BLOB NOT NULL CHECK(length(nock_contract_address) = 20),
            generation INTEGER NOT NULL,
            reason TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (chain_id, nock_contract_address)
        );

        CREATE TABLE IF NOT EXISTS sequencer_base_reconciliation_cursor (
            chain_id INTEGER NOT NULL,
            nock_contract_address BLOB NOT NULL CHECK(length(nock_contract_address) = 20),
            journal_id TEXT NOT NULL,
            last_journal_sequence INTEGER NOT NULL,
            last_journal_event_id TEXT NOT NULL,
            last_base_block INTEGER NOT NULL,
            last_base_block_hash BLOB NOT NULL CHECK(length(last_base_block_hash) = 32),
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (chain_id, nock_contract_address, journal_id)
        );
        "#,
    )
    .map_err(|error| BridgeError::Runtime(format!("Base activity store schema failed: {error}")))?;
    ensure_base_activity_column(conn, "observed_at_unix_secs", "INTEGER")?;
    ensure_base_activity_column(conn, "policy_id", "TEXT")?;
    ensure_base_activity_column(conn, "protocol_id", "TEXT")?;
    ensure_base_incident_schema(conn)?;
    ensure_base_activity_column(conn, "canonical", "INTEGER NOT NULL DEFAULT 1")?;
    ensure_base_activity_column(conn, "invalidated_at", "INTEGER")?;
    ensure_base_activity_column(conn, "invalidation_generation", "INTEGER")?;
    ensure_base_activity_column(conn, "invalidation_reason", "TEXT")?;
    conn.batch_execute(
        r#"
        CREATE INDEX IF NOT EXISTS sequencer_base_burns_by_canonical_block
          ON sequencer_base_burns(
            chain_id, nock_contract_address, canonical, block_number
          );
        "#,
    )
    .map_err(|error| {
        BridgeError::Runtime(format!(
            "Base activity canonical index migration failed: {error}"
        ))
    })?;
    Ok(())
}

fn ensure_base_activity_column(
    conn: &mut SqliteConnection,
    column_name: &str,
    column_sql: &str,
) -> Result<(), BridgeError> {
    #[derive(QueryableByName)]
    struct TableInfoRow {
        #[diesel(sql_type = Text)]
        name: String,
    }

    let columns = diesel::sql_query("PRAGMA table_info(sequencer_base_burns)")
        .load::<TableInfoRow>(conn)
        .map_err(|error| {
            BridgeError::Runtime(format!("Base activity table_info failed: {error}"))
        })?;
    if columns.iter().any(|column| column.name == column_name) {
        return Ok(());
    }
    conn.batch_execute(&format!(
        "ALTER TABLE sequencer_base_burns ADD COLUMN {column_name} {column_sql};"
    ))
    .map_err(|error| {
        BridgeError::Runtime(format!(
            "Base activity column migration failed for {column_name}: {error}"
        ))
    })
}

fn insert_verified_burn_checked(
    conn: &mut SqliteConnection,
    record: &VerifiedBaseWithdrawalBurn,
) -> Result<bool, BridgeError> {
    let reactivated = diesel::sql_query(
        r#"
        DELETE FROM sequencer_base_burns
        WHERE chain_id = ? AND nock_contract_address = ?
          AND base_event_id = ? AND canonical = 0
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(record.chain_id, "chain_id")?)
    .bind::<Binary, _>(record.nock_contract_address.as_slice().to_vec())
    .bind::<Binary, _>(record.base_event_id.0.clone())
    .execute(conn)
    .map_err(|error| {
        BridgeError::Runtime(format!(
            "invalidated Base burn projection cleanup failed: {error}"
        ))
    })?;
    let inserted = insert_verified_burn_row(conn, record)?;
    backfill_verified_burn_public_metadata(conn, record)?;
    let mut stored = load_verified_burn_row(
        conn, record.chain_id, record.nock_contract_address, &record.base_event_id,
    )?
    .ok_or_else(|| {
        BridgeError::Runtime(format!(
            "Base activity insert did not produce a row for {:?}",
            record.base_event_id
        ))
    })?;
    stored.withdrawal_nonce = record.withdrawal_nonce;
    stored.verified_at = record.verified_at;
    if stored != *record {
        return Err(BridgeError::Runtime(format!(
            "conflicting immutable Base burn facts for {:?}",
            record.base_event_id
        )));
    }
    Ok(inserted || reactivated == 1)
}

fn backfill_verified_burn_public_metadata(
    conn: &mut SqliteConnection,
    record: &VerifiedBaseWithdrawalBurn,
) -> Result<(), BridgeError> {
    diesel::sql_query(
        r#"
        UPDATE sequencer_base_burns
        SET observed_at_unix_secs = COALESCE(observed_at_unix_secs, ?),
            policy_id = COALESCE(policy_id, ?),
            protocol_id = COALESCE(protocol_id, ?)
        WHERE chain_id = ? AND nock_contract_address = ? AND base_event_id = ?
        "#,
    )
    .bind::<Nullable<BigInt>, _>(
        record
            .observed_at_unix_secs
            .map(|value| u64_to_i64(value, "observed_at_unix_secs"))
            .transpose()?,
    )
    .bind::<Nullable<Text>, _>(record.policy_id.clone())
    .bind::<Nullable<Text>, _>(record.protocol_id.clone())
    .bind::<BigInt, _>(u64_to_i64(record.chain_id, "chain_id")?)
    .bind::<Binary, _>(record.nock_contract_address.as_slice().to_vec())
    .bind::<Binary, _>(record.base_event_id.0.clone())
    .execute(conn)
    .map_err(|error| {
        BridgeError::Runtime(format!("Base activity metadata backfill failed: {error}"))
    })?;
    Ok(())
}

#[derive(QueryableByName)]
struct BaseBurnSqlRow {
    #[diesel(sql_type = BigInt)]
    chain_id: i64,
    #[diesel(sql_type = Binary)]
    nock_contract_address: Vec<u8>,
    #[diesel(sql_type = Binary)]
    base_event_id: Vec<u8>,
    #[diesel(sql_type = BigInt)]
    block_number: i64,
    #[diesel(sql_type = Binary)]
    block_hash: Vec<u8>,
    #[diesel(sql_type = Binary)]
    parent_hash: Vec<u8>,
    #[diesel(sql_type = Binary)]
    tx_hash: Vec<u8>,
    #[diesel(sql_type = BigInt)]
    tx_index: i64,
    #[diesel(sql_type = BigInt)]
    log_index: i64,
    #[diesel(sql_type = Binary)]
    burner: Vec<u8>,
    #[diesel(sql_type = Text)]
    amount_base_units: String,
    #[diesel(sql_type = Text)]
    amount_nicks: String,
    #[diesel(sql_type = Binary)]
    lock_root: Vec<u8>,
    #[diesel(sql_type = Binary)]
    calldata: Vec<u8>,
    #[diesel(sql_type = BigInt)]
    base_batch_end: i64,
    #[diesel(sql_type = Nullable<BigInt>)]
    withdrawal_nonce: Option<i64>,
    #[diesel(sql_type = BigInt)]
    verified_at: i64,
    #[diesel(sql_type = Nullable<BigInt>)]
    observed_at_unix_secs: Option<i64>,
    #[diesel(sql_type = Nullable<Text>)]
    policy_id: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    protocol_id: Option<String>,
    #[diesel(sql_type = BigInt)]
    canonical: i64,
    #[diesel(sql_type = Nullable<BigInt>)]
    invalidated_at: Option<i64>,
    #[diesel(sql_type = Nullable<BigInt>)]
    invalidation_generation: Option<i64>,
    #[diesel(sql_type = Nullable<Text>)]
    invalidation_reason: Option<String>,
}

impl TryFrom<BaseBurnSqlRow> for VerifiedBaseWithdrawalBurn {
    type Error = BridgeError;

    fn try_from(row: BaseBurnSqlRow) -> Result<Self, Self::Error> {
        Ok(Self {
            chain_id: i64_to_u64(row.chain_id, "chain_id")?,
            nock_contract_address: address_from_bytes(
                &row.nock_contract_address, "nock_contract_address",
            )?,
            base_event_id: BaseEventId(
                fixed_bytes::<32>(&row.base_event_id, "base_event_id")?.to_vec(),
            ),
            block_number: i64_to_u64(row.block_number, "block_number")?,
            block_hash: B256::from(fixed_bytes::<32>(&row.block_hash, "block_hash")?),
            parent_hash: B256::from(fixed_bytes::<32>(&row.parent_hash, "parent_hash")?),
            observed_at_unix_secs: row
                .observed_at_unix_secs
                .map(|value| i64_to_u64(value, "observed_at_unix_secs"))
                .transpose()?,
            tx_hash: B256::from(fixed_bytes::<32>(&row.tx_hash, "tx_hash")?),
            tx_index: i64_to_u64(row.tx_index, "tx_index")?,
            log_index: i64_to_u64(row.log_index, "log_index")?,
            burner: address_from_bytes(&row.burner, "burner")?,
            amount_base_units: row.amount_base_units,
            amount_nicks: row.amount_nicks.parse::<u64>().map_err(|error| {
                BridgeError::ValueConversion(format!(
                    "invalid amount_nicks in Base activity: {error}"
                ))
            })?,
            lock_root: Tip5Hash::from_be_limb_bytes(&row.lock_root).map_err(|error| {
                BridgeError::ValueConversion(format!("invalid lock_root in Base activity: {error}"))
            })?,
            calldata: row.calldata,
            base_batch_end: i64_to_u64(row.base_batch_end, "base_batch_end")?,
            withdrawal_nonce: row
                .withdrawal_nonce
                .map(|value| i64_to_u64(value, "withdrawal_nonce"))
                .transpose()?,
            verified_at: row.verified_at,
            policy_id: row.policy_id,
            protocol_id: row.protocol_id,
        })
    }
}
impl BaseBurnSqlRow {
    fn into_public(self) -> Result<PublicBaseWithdrawalBurn, BridgeError> {
        let canonical = match self.canonical {
            0 => false,
            1 => true,
            value => {
                return Err(BridgeError::Runtime(format!(
                    "invalid Base activity canonical flag: {value}"
                )));
            }
        };
        let invalidated_at = self.invalidated_at;
        let invalidation_generation = self
            .invalidation_generation
            .map(|value| i64_to_u64(value, "invalidation_generation"))
            .transpose()?;
        let invalidation_reason = self.invalidation_reason.clone();
        Ok(PublicBaseWithdrawalBurn {
            burn: self.try_into()?,
            canonical,
            invalidated_at,
            invalidation_generation,
            invalidation_reason,
        })
    }
}

#[derive(QueryableByName)]
struct BaseActivityCursorSqlRow {
    #[diesel(sql_type = BigInt)]
    chain_id: i64,
    #[diesel(sql_type = Binary)]
    nock_contract_address: Vec<u8>,
    #[diesel(sql_type = BigInt)]
    last_verified_block: i64,
    #[diesel(sql_type = Binary)]
    last_verified_block_hash: Vec<u8>,
    #[diesel(sql_type = BigInt)]
    updated_at: i64,
}

impl TryFrom<BaseActivityCursorSqlRow> for BaseActivityCursor {
    type Error = BridgeError;

    fn try_from(row: BaseActivityCursorSqlRow) -> Result<Self, Self::Error> {
        Ok(Self {
            chain_id: i64_to_u64(row.chain_id, "chain_id")?,
            nock_contract_address: address_from_bytes(
                &row.nock_contract_address, "nock_contract_address",
            )?,
            last_verified_block: i64_to_u64(row.last_verified_block, "last_verified_block")?,
            last_verified_block_hash: B256::from(fixed_bytes::<32>(
                &row.last_verified_block_hash, "last_verified_block_hash",
            )?),
            updated_at: row.updated_at,
        })
    }
}

#[derive(QueryableByName)]
struct BaseActivityHeaderCheckpointSqlRow {
    #[diesel(sql_type = BigInt)]
    chain_id: i64,
    #[diesel(sql_type = Binary)]
    nock_contract_address: Vec<u8>,
    #[diesel(sql_type = BigInt)]
    block_number: i64,
    #[diesel(sql_type = Binary)]
    block_hash: Vec<u8>,
    #[diesel(sql_type = Binary)]
    parent_hash: Vec<u8>,
    #[diesel(sql_type = BigInt)]
    block_timestamp: i64,
    #[diesel(sql_type = BigInt)]
    verified_at: i64,
}

impl TryFrom<BaseActivityHeaderCheckpointSqlRow> for BaseActivityHeaderCheckpoint {
    type Error = BridgeError;

    fn try_from(row: BaseActivityHeaderCheckpointSqlRow) -> Result<Self, Self::Error> {
        Ok(Self {
            chain_id: i64_to_u64(row.chain_id, "chain_id")?,
            nock_contract_address: address_from_bytes(
                &row.nock_contract_address, "nock_contract_address",
            )?,
            block_number: i64_to_u64(row.block_number, "block_number")?,
            block_hash: B256::from(fixed_bytes::<32>(&row.block_hash, "block_hash")?),
            parent_hash: B256::from(fixed_bytes::<32>(&row.parent_hash, "parent_hash")?),
            block_timestamp: i64_to_u64(row.block_timestamp, "block_timestamp")?,
            verified_at: row.verified_at,
        })
    }
}

fn insert_verified_burn_row(
    conn: &mut SqliteConnection,
    record: &VerifiedBaseWithdrawalBurn,
) -> Result<bool, BridgeError> {
    let inserted = diesel::sql_query(
        r#"
        INSERT OR IGNORE INTO sequencer_base_burns (
            chain_id, nock_contract_address, base_event_id, block_number,
            block_hash, parent_hash, observed_at_unix_secs,
            tx_hash, tx_index, log_index, burner,
            amount_base_units, amount_nicks, lock_root, calldata,
            base_batch_end, withdrawal_nonce, verified_at, policy_id, protocol_id
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(record.chain_id, "chain_id")?)
    .bind::<Binary, _>(record.nock_contract_address.as_slice().to_vec())
    .bind::<Binary, _>(record.base_event_id.0.clone())
    .bind::<BigInt, _>(u64_to_i64(record.block_number, "block_number")?)
    .bind::<Binary, _>(record.block_hash.as_slice().to_vec())
    .bind::<Binary, _>(record.parent_hash.as_slice().to_vec())
    .bind::<Nullable<BigInt>, _>(
        record
            .observed_at_unix_secs
            .map(|value| u64_to_i64(value, "observed_at_unix_secs"))
            .transpose()?,
    )
    .bind::<Binary, _>(record.tx_hash.as_slice().to_vec())
    .bind::<BigInt, _>(u64_to_i64(record.tx_index, "tx_index")?)
    .bind::<BigInt, _>(u64_to_i64(record.log_index, "log_index")?)
    .bind::<Binary, _>(record.burner.as_slice().to_vec())
    .bind::<Text, _>(record.amount_base_units.clone())
    .bind::<Text, _>(record.amount_nicks.to_string())
    .bind::<Binary, _>(record.lock_root.to_be_limb_bytes().to_vec())
    .bind::<Binary, _>(record.calldata.clone())
    .bind::<BigInt, _>(u64_to_i64(record.base_batch_end, "base_batch_end")?)
    .bind::<Nullable<BigInt>, _>(
        record
            .withdrawal_nonce
            .map(|value| u64_to_i64(value, "withdrawal_nonce"))
            .transpose()?,
    )
    .bind::<BigInt, _>(record.verified_at)
    .bind::<Nullable<Text>, _>(record.policy_id.clone())
    .bind::<Nullable<Text>, _>(record.protocol_id.clone())
    .execute(conn)
    .map_err(|error| BridgeError::Runtime(format!("Base activity burn insert failed: {error}")))?;
    Ok(inserted == 1)
}

fn load_verified_burn_row(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
    base_event_id: &BaseEventId,
) -> Result<Option<VerifiedBaseWithdrawalBurn>, BridgeError> {
    diesel::sql_query(
        r#"
        SELECT chain_id, nock_contract_address, base_event_id, block_number,
               block_hash, parent_hash, observed_at_unix_secs,
               tx_hash, tx_index, log_index, burner,
               amount_base_units, amount_nicks, lock_root, calldata,
               base_batch_end, withdrawal_nonce, verified_at, policy_id, protocol_id,
               canonical, invalidated_at, invalidation_generation, invalidation_reason
        FROM sequencer_base_burns
        WHERE chain_id = ? AND nock_contract_address = ? AND base_event_id = ?
          AND canonical = 1
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(chain_id, "chain_id")?)
    .bind::<Binary, _>(nock_contract_address.as_slice().to_vec())
    .bind::<Binary, _>(base_event_id.0.clone())
    .get_result::<BaseBurnSqlRow>(conn)
    .optional()
    .map_err(|error| BridgeError::Runtime(format!("Base activity burn load failed: {error}")))?
    .map(TryInto::try_into)
    .transpose()
}

fn load_public_burn_row(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
    base_event_id: &BaseEventId,
) -> Result<Option<PublicBaseWithdrawalBurn>, BridgeError> {
    diesel::sql_query(
        r#"
        SELECT chain_id, nock_contract_address, base_event_id, block_number,
               block_hash, parent_hash, observed_at_unix_secs,
               tx_hash, tx_index, log_index, burner,
               amount_base_units, amount_nicks, lock_root, calldata,
               base_batch_end, withdrawal_nonce, verified_at, policy_id, protocol_id,
               canonical, invalidated_at, invalidation_generation, invalidation_reason
        FROM sequencer_base_burns
        WHERE chain_id = ? AND nock_contract_address = ? AND base_event_id = ?
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(chain_id, "chain_id")?)
    .bind::<Binary, _>(nock_contract_address.as_slice().to_vec())
    .bind::<Binary, _>(base_event_id.0.clone())
    .get_result::<BaseBurnSqlRow>(conn)
    .optional()
    .map_err(|error| BridgeError::Runtime(format!("public Base burn load failed: {error}")))?
    .map(BaseBurnSqlRow::into_public)
    .transpose()
}

fn load_verified_burn_by_tx_log_row(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
    tx_hash: B256,
    log_index: u64,
) -> Result<Option<VerifiedBaseWithdrawalBurn>, BridgeError> {
    diesel::sql_query(
        r#"
        SELECT chain_id, nock_contract_address, base_event_id, block_number,
               block_hash, parent_hash, observed_at_unix_secs,
               tx_hash, tx_index, log_index, burner,
               amount_base_units, amount_nicks, lock_root, calldata,
               base_batch_end, withdrawal_nonce, verified_at, policy_id, protocol_id,
               canonical, invalidated_at, invalidation_generation, invalidation_reason
        FROM sequencer_base_burns
        WHERE chain_id = ? AND nock_contract_address = ?
          AND tx_hash = ? AND log_index = ? AND canonical = 1
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(chain_id, "chain_id")?)
    .bind::<Binary, _>(nock_contract_address.as_slice().to_vec())
    .bind::<Binary, _>(tx_hash.as_slice().to_vec())
    .bind::<BigInt, _>(u64_to_i64(log_index, "log_index")?)
    .get_result::<BaseBurnSqlRow>(conn)
    .optional()
    .map_err(|error| BridgeError::Runtime(format!("Base activity tx/log lookup failed: {error}")))?
    .map(TryInto::try_into)
    .transpose()
}

fn load_public_burn_by_tx_log_row(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
    tx_hash: B256,
    log_index: u64,
) -> Result<Option<PublicBaseWithdrawalBurn>, BridgeError> {
    diesel::sql_query(
        r#"
        SELECT chain_id, nock_contract_address, base_event_id, block_number,
               block_hash, parent_hash, observed_at_unix_secs,
               tx_hash, tx_index, log_index, burner,
               amount_base_units, amount_nicks, lock_root, calldata,
               base_batch_end, withdrawal_nonce, verified_at, policy_id, protocol_id,
               canonical, invalidated_at, invalidation_generation, invalidation_reason
        FROM sequencer_base_burns
        WHERE chain_id = ? AND nock_contract_address = ?
          AND tx_hash = ? AND log_index = ?
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(chain_id, "chain_id")?)
    .bind::<Binary, _>(nock_contract_address.as_slice().to_vec())
    .bind::<Binary, _>(tx_hash.as_slice().to_vec())
    .bind::<BigInt, _>(u64_to_i64(log_index, "log_index")?)
    .get_result::<BaseBurnSqlRow>(conn)
    .optional()
    .map_err(|error| BridgeError::Runtime(format!("public Base tx/log lookup failed: {error}")))?
    .map(BaseBurnSqlRow::into_public)
    .transpose()
}

fn load_verified_burn_rows_by_tx_hash(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
    tx_hash: B256,
) -> Result<Vec<VerifiedBaseWithdrawalBurn>, BridgeError> {
    diesel::sql_query(
        r#"
        SELECT chain_id, nock_contract_address, base_event_id, block_number,
               block_hash, parent_hash, observed_at_unix_secs,
               tx_hash, tx_index, log_index, burner,
               amount_base_units, amount_nicks, lock_root, calldata,
               base_batch_end, withdrawal_nonce, verified_at, policy_id, protocol_id,
               canonical, invalidated_at, invalidation_generation, invalidation_reason
        FROM sequencer_base_burns
        WHERE chain_id = ? AND nock_contract_address = ? AND tx_hash = ?
          AND canonical = 1
        ORDER BY log_index ASC
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(chain_id, "chain_id")?)
    .bind::<Binary, _>(nock_contract_address.as_slice().to_vec())
    .bind::<Binary, _>(tx_hash.as_slice().to_vec())
    .load::<BaseBurnSqlRow>(conn)
    .map_err(|error| BridgeError::Runtime(format!("Base activity tx list failed: {error}")))?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}
fn load_public_burn_rows_by_tx_hash(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
    tx_hash: B256,
) -> Result<Vec<PublicBaseWithdrawalBurn>, BridgeError> {
    diesel::sql_query(
        r#"
        SELECT chain_id, nock_contract_address, base_event_id, block_number,
               block_hash, parent_hash, observed_at_unix_secs,
               tx_hash, tx_index, log_index, burner,
               amount_base_units, amount_nicks, lock_root, calldata,
               base_batch_end, withdrawal_nonce, verified_at, policy_id, protocol_id,
               canonical, invalidated_at, invalidation_generation, invalidation_reason
        FROM sequencer_base_burns
        WHERE chain_id = ? AND nock_contract_address = ? AND tx_hash = ?
        ORDER BY log_index ASC
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(chain_id, "chain_id")?)
    .bind::<Binary, _>(nock_contract_address.as_slice().to_vec())
    .bind::<Binary, _>(tx_hash.as_slice().to_vec())
    .load::<BaseBurnSqlRow>(conn)
    .map_err(|error| BridgeError::Runtime(format!("public Base tx list failed: {error}")))?
    .into_iter()
    .map(BaseBurnSqlRow::into_public)
    .collect()
}

fn load_verified_burn_rows_by_burner(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
    burner: Address,
    limit: u32,
) -> Result<Vec<VerifiedBaseWithdrawalBurn>, BridgeError> {
    diesel::sql_query(
        r#"
        SELECT chain_id, nock_contract_address, base_event_id, block_number,
               block_hash, parent_hash, observed_at_unix_secs,
               tx_hash, tx_index, log_index, burner,
               amount_base_units, amount_nicks, lock_root, calldata,
               base_batch_end, withdrawal_nonce, verified_at, policy_id, protocol_id,
               canonical, invalidated_at, invalidation_generation, invalidation_reason
        FROM sequencer_base_burns
        WHERE chain_id = ? AND nock_contract_address = ? AND burner = ?
          AND canonical = 1
        ORDER BY block_number DESC, log_index DESC, base_event_id DESC
        LIMIT ?
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(chain_id, "chain_id")?)
    .bind::<Binary, _>(nock_contract_address.as_slice().to_vec())
    .bind::<Binary, _>(burner.as_slice().to_vec())
    .bind::<BigInt, _>(i64::from(limit))
    .load::<BaseBurnSqlRow>(conn)
    .map_err(|error| BridgeError::Runtime(format!("Base activity burner history failed: {error}")))?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

#[derive(QueryableByName)]
struct MaxBaseBurnRowId {
    #[diesel(sql_type = Nullable<BigInt>)]
    value: Option<i64>,
}

fn load_verified_burn_page_by_burner(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
    burner: Address,
    cursor: Option<&BaseActivityPageCursor>,
    limit: u32,
) -> Result<BaseActivityBurnPage, BridgeError> {
    let snapshot_rowid = match cursor {
        Some(cursor) => {
            fixed_bytes::<32>(&cursor.last_base_event_id.0, "page cursor base_event_id")?;
            cursor.snapshot_rowid
        }
        None => diesel::sql_query(
            r#"
            SELECT MAX(rowid) AS value
            FROM sequencer_base_burns
            WHERE chain_id = ? AND nock_contract_address = ? AND burner = ?
            "#,
        )
        .bind::<BigInt, _>(u64_to_i64(chain_id, "chain_id")?)
        .bind::<Binary, _>(nock_contract_address.as_slice().to_vec())
        .bind::<Binary, _>(burner.as_slice().to_vec())
        .get_result::<MaxBaseBurnRowId>(conn)
        .map_err(|error| {
            BridgeError::Runtime(format!(
                "Base activity burner snapshot lookup failed: {error}"
            ))
        })?
        .value
        .map(|value| i64_to_u64(value, "snapshot_rowid"))
        .transpose()?
        .unwrap_or_default(),
    };
    let snapshot_rowid_i64 = u64_to_i64(snapshot_rowid, "snapshot_rowid")?;
    let rows = match cursor {
        Some(cursor) => diesel::sql_query(
            r#"
            SELECT chain_id, nock_contract_address, base_event_id, block_number,
                   block_hash, parent_hash, observed_at_unix_secs,
                   tx_hash, tx_index, log_index, burner,
                   amount_base_units, amount_nicks, lock_root, calldata,
                   base_batch_end, withdrawal_nonce, verified_at, policy_id, protocol_id,
                   canonical, invalidated_at, invalidation_generation, invalidation_reason
            FROM sequencer_base_burns
            WHERE chain_id = ? AND nock_contract_address = ? AND burner = ?
              AND rowid <= ?
              AND (
                block_number < ?
                OR (block_number = ? AND log_index < ?)
                OR (block_number = ? AND log_index = ? AND base_event_id < ?)
              )
            ORDER BY block_number DESC, log_index DESC, base_event_id DESC
            LIMIT ?
            "#,
        )
        .bind::<BigInt, _>(u64_to_i64(chain_id, "chain_id")?)
        .bind::<Binary, _>(nock_contract_address.as_slice().to_vec())
        .bind::<Binary, _>(burner.as_slice().to_vec())
        .bind::<BigInt, _>(snapshot_rowid_i64)
        .bind::<BigInt, _>(u64_to_i64(cursor.last_block_number, "last_block_number")?)
        .bind::<BigInt, _>(u64_to_i64(cursor.last_block_number, "last_block_number")?)
        .bind::<BigInt, _>(u64_to_i64(cursor.last_log_index, "last_log_index")?)
        .bind::<BigInt, _>(u64_to_i64(cursor.last_block_number, "last_block_number")?)
        .bind::<BigInt, _>(u64_to_i64(cursor.last_log_index, "last_log_index")?)
        .bind::<Binary, _>(cursor.last_base_event_id.0.clone())
        .bind::<BigInt, _>(i64::from(limit))
        .load::<BaseBurnSqlRow>(conn),
        None => diesel::sql_query(
            r#"
            SELECT chain_id, nock_contract_address, base_event_id, block_number,
                   block_hash, parent_hash, observed_at_unix_secs,
                   tx_hash, tx_index, log_index, burner,
                   amount_base_units, amount_nicks, lock_root, calldata,
                   base_batch_end, withdrawal_nonce, verified_at, policy_id, protocol_id,
                   canonical, invalidated_at, invalidation_generation, invalidation_reason
            FROM sequencer_base_burns
            WHERE chain_id = ? AND nock_contract_address = ? AND burner = ?
              AND rowid <= ?
            ORDER BY block_number DESC, log_index DESC, base_event_id DESC
            LIMIT ?
            "#,
        )
        .bind::<BigInt, _>(u64_to_i64(chain_id, "chain_id")?)
        .bind::<Binary, _>(nock_contract_address.as_slice().to_vec())
        .bind::<Binary, _>(burner.as_slice().to_vec())
        .bind::<BigInt, _>(snapshot_rowid_i64)
        .bind::<BigInt, _>(i64::from(limit))
        .load::<BaseBurnSqlRow>(conn),
    }
    .map_err(|error| {
        BridgeError::Runtime(format!("Base activity burner page query failed: {error}"))
    })?;
    Ok(BaseActivityBurnPage {
        burns: rows
            .into_iter()
            .map(BaseBurnSqlRow::into_public)
            .collect::<Result<Vec<_>, BridgeError>>()?,
        snapshot_rowid,
    })
}

fn load_unmapped_verified_burn_rows(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
    minimum_block: u64,
) -> Result<Vec<VerifiedBaseWithdrawalBurn>, BridgeError> {
    diesel::sql_query(
        r#"
        SELECT chain_id, nock_contract_address, base_event_id, block_number,
               block_hash, parent_hash, observed_at_unix_secs,
               tx_hash, tx_index, log_index, burner,
               amount_base_units, amount_nicks, lock_root, calldata,
               base_batch_end, withdrawal_nonce, verified_at, policy_id, protocol_id,
               canonical, invalidated_at, invalidation_generation, invalidation_reason
        FROM sequencer_base_burns
        WHERE chain_id = ?
          AND nock_contract_address = ?
          AND block_number >= ?
          AND canonical = 1
          AND withdrawal_nonce IS NULL
        ORDER BY base_batch_end ASC, base_event_id ASC
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(chain_id, "chain_id")?)
    .bind::<Binary, _>(nock_contract_address.as_slice().to_vec())
    .bind::<BigInt, _>(u64_to_i64(minimum_block, "minimum_block")?)
    .load::<BaseBurnSqlRow>(conn)
    .map_err(|error| {
        BridgeError::Runtime(format!("unmapped Base activity burn list failed: {error}"))
    })?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) fn assign_withdrawal_nonce(
    conn: &mut SqliteConnection,
    record: &VerifiedBaseWithdrawalBurn,
    withdrawal_nonce: u64,
) -> Result<(), BridgeError> {
    let stored = load_verified_burn_row(
        conn, record.chain_id, record.nock_contract_address, &record.base_event_id,
    )?
    .ok_or_else(|| {
        BridgeError::Runtime(format!(
            "cannot assign withdrawal nonce to missing Base burn {:?}",
            record.base_event_id
        ))
    })?;
    if stored
        .withdrawal_nonce
        .is_some_and(|nonce| nonce != withdrawal_nonce)
    {
        return Err(BridgeError::Runtime(format!(
            "Base burn {:?} is already assigned withdrawal nonce {:?}, cannot assign {}",
            record.base_event_id, stored.withdrawal_nonce, withdrawal_nonce
        )));
    }
    diesel::sql_query(
        r#"
        UPDATE sequencer_base_burns
        SET withdrawal_nonce = ?
        WHERE chain_id = ? AND nock_contract_address = ? AND base_event_id = ?
          AND canonical = 1
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(withdrawal_nonce, "withdrawal_nonce")?)
    .bind::<BigInt, _>(u64_to_i64(record.chain_id, "chain_id")?)
    .bind::<Binary, _>(record.nock_contract_address.as_slice().to_vec())
    .bind::<Binary, _>(record.base_event_id.0.clone())
    .execute(conn)
    .map_err(|error| {
        BridgeError::Runtime(format!("Base activity nonce assignment failed: {error}"))
    })?;
    Ok(())
}

pub(crate) fn load_verified_burn_for_reconciliation(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
    base_event_id: &BaseEventId,
) -> Result<Option<VerifiedBaseWithdrawalBurn>, BridgeError> {
    load_verified_burn_row(conn, chain_id, nock_contract_address, base_event_id)
}

pub(crate) fn load_activity_cursor_for_reconciliation(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
) -> Result<Option<BaseActivityCursor>, BridgeError> {
    load_cursor_row(conn, chain_id, nock_contract_address)
}

fn insert_header_checkpoint_checked(
    conn: &mut SqliteConnection,
    checkpoint: &BaseActivityHeaderCheckpoint,
) -> Result<(), BridgeError> {
    diesel::sql_query(
        r#"
        INSERT OR IGNORE INTO sequencer_base_header_checkpoints (
            chain_id, nock_contract_address, block_number, block_hash,
            parent_hash, block_timestamp, verified_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(checkpoint.chain_id, "chain_id")?)
    .bind::<Binary, _>(checkpoint.nock_contract_address.as_slice().to_vec())
    .bind::<BigInt, _>(u64_to_i64(checkpoint.block_number, "block_number")?)
    .bind::<Binary, _>(checkpoint.block_hash.as_slice().to_vec())
    .bind::<Binary, _>(checkpoint.parent_hash.as_slice().to_vec())
    .bind::<BigInt, _>(u64_to_i64(checkpoint.block_timestamp, "block_timestamp")?)
    .bind::<BigInt, _>(checkpoint.verified_at)
    .execute(conn)
    .map_err(|error| {
        BridgeError::Runtime(format!("Base header checkpoint insert failed: {error}"))
    })?;
    let stored: BaseActivityHeaderCheckpoint = diesel::sql_query(
        r#"
        SELECT chain_id, nock_contract_address, block_number, block_hash,
               parent_hash, block_timestamp, verified_at
        FROM sequencer_base_header_checkpoints
        WHERE chain_id = ? AND nock_contract_address = ? AND block_number = ?
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(checkpoint.chain_id, "chain_id")?)
    .bind::<Binary, _>(checkpoint.nock_contract_address.as_slice().to_vec())
    .bind::<BigInt, _>(u64_to_i64(checkpoint.block_number, "block_number")?)
    .get_result::<BaseActivityHeaderCheckpointSqlRow>(conn)
    .map_err(|error| {
        BridgeError::Runtime(format!("Base header checkpoint reload failed: {error}"))
    })?
    .try_into()?;
    if stored.block_hash != checkpoint.block_hash
        || stored.parent_hash != checkpoint.parent_hash
        || stored.block_timestamp != checkpoint.block_timestamp
    {
        return Err(BridgeError::Runtime(format!(
            "conflicting Base header checkpoint at block {}: stored {:?}, canonical {:?}",
            checkpoint.block_number, stored.block_hash, checkpoint.block_hash
        )));
    }
    Ok(())
}

fn load_header_checkpoint_rows(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
    start_block: u64,
    end_block: u64,
) -> Result<Vec<BaseActivityHeaderCheckpoint>, BridgeError> {
    if end_block < start_block {
        return Ok(Vec::new());
    }
    diesel::sql_query(
        r#"
        SELECT chain_id, nock_contract_address, block_number, block_hash,
               parent_hash, block_timestamp, verified_at
        FROM sequencer_base_header_checkpoints
        WHERE chain_id = ? AND nock_contract_address = ?
          AND block_number BETWEEN ? AND ?
        ORDER BY block_number ASC
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(chain_id, "chain_id")?)
    .bind::<Binary, _>(nock_contract_address.as_slice().to_vec())
    .bind::<BigInt, _>(u64_to_i64(start_block, "start_block")?)
    .bind::<BigInt, _>(u64_to_i64(end_block, "end_block")?)
    .load::<BaseActivityHeaderCheckpointSqlRow>(conn)
    .map_err(|error| BridgeError::Runtime(format!("Base header checkpoint load failed: {error}")))?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

fn prune_header_checkpoints_before(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
    retained_from_block: u64,
) -> Result<(), BridgeError> {
    diesel::sql_query(
        r#"
        DELETE FROM sequencer_base_header_checkpoints
        WHERE chain_id = ? AND nock_contract_address = ? AND block_number < ?
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(chain_id, "chain_id")?)
    .bind::<Binary, _>(nock_contract_address.as_slice().to_vec())
    .bind::<BigInt, _>(u64_to_i64(retained_from_block, "retained_from_block")?)
    .execute(conn)
    .map_err(|error| {
        BridgeError::Runtime(format!("Base header checkpoint pruning failed: {error}"))
    })?;
    Ok(())
}

fn load_cursor_row(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
) -> Result<Option<BaseActivityCursor>, BridgeError> {
    diesel::sql_query(
        r#"
        SELECT chain_id, nock_contract_address, last_verified_block,
               last_verified_block_hash, updated_at
        FROM sequencer_base_activity_cursor
        WHERE chain_id = ? AND nock_contract_address = ?
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(chain_id, "chain_id")?)
    .bind::<Binary, _>(nock_contract_address.as_slice().to_vec())
    .get_result::<BaseActivityCursorSqlRow>(conn)
    .optional()
    .map_err(|error| BridgeError::Runtime(format!("Base activity cursor load failed: {error}")))?
    .map(TryInto::try_into)
    .transpose()
}

fn upsert_cursor_row(
    conn: &mut SqliteConnection,
    cursor: &BaseActivityCursor,
) -> Result<(), BridgeError> {
    diesel::sql_query(
        r#"
        INSERT INTO sequencer_base_activity_cursor (
            chain_id, nock_contract_address, last_verified_block,
            last_verified_block_hash, updated_at
        ) VALUES (?, ?, ?, ?, ?)
        ON CONFLICT(chain_id, nock_contract_address) DO UPDATE SET
            last_verified_block = excluded.last_verified_block,
            last_verified_block_hash = excluded.last_verified_block_hash,
            updated_at = excluded.updated_at
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(cursor.chain_id, "chain_id")?)
    .bind::<Binary, _>(cursor.nock_contract_address.as_slice().to_vec())
    .bind::<BigInt, _>(u64_to_i64(
        cursor.last_verified_block, "last_verified_block",
    )?)
    .bind::<Binary, _>(cursor.last_verified_block_hash.as_slice().to_vec())
    .bind::<BigInt, _>(cursor.updated_at)
    .execute(conn)
    .map_err(|error| {
        BridgeError::Runtime(format!("Base activity cursor upsert failed: {error}"))
    })?;
    Ok(())
}

fn u64_to_i64(value: u64, field: &str) -> Result<i64, BridgeError> {
    i64::try_from(value).map_err(|error| {
        BridgeError::ValueConversion(format!("{field} does not fit SQLite INTEGER: {error}"))
    })
}

fn i64_to_u64(value: i64, field: &str) -> Result<u64, BridgeError> {
    u64::try_from(value).map_err(|error| {
        BridgeError::ValueConversion(format!("{field} is negative or invalid: {error}"))
    })
}

fn fixed_bytes<const N: usize>(value: &[u8], field: &str) -> Result<[u8; N], BridgeError> {
    value.try_into().map_err(|_| {
        BridgeError::ValueConversion(format!("{field} has {} bytes, expected {N}", value.len()))
    })
}

fn address_from_bytes(value: &[u8], field: &str) -> Result<Address, BridgeError> {
    Ok(Address::from(fixed_bytes::<20>(value, field)?))
}

pub fn current_unix_timestamp_secs() -> Result<i64, BridgeError> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| BridgeError::Runtime(format!("system clock before unix epoch: {error}")))?
        .as_secs();
    i64::try_from(seconds).map_err(|error| {
        BridgeError::ValueConversion(format!("unix timestamp does not fit i64: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::withdrawal::sequencer::store::WithdrawalSequencerStore;

    async fn open_store() -> (tempfile::TempDir, BaseActivityStore) {
        let directory = tempfile::TempDir::new().expect("temporary directory");
        let store = WithdrawalSequencerStore::open(directory.path().join("sequencer.sqlite"))
            .await
            .expect("open withdrawal store");
        let base_activity = store.base_activity_store();
        (directory, base_activity)
    }

    fn sample_burn() -> VerifiedBaseWithdrawalBurn {
        VerifiedBaseWithdrawalBurn {
            chain_id: 8_453,
            nock_contract_address: Address::from([0x11; 20]),
            base_event_id: BaseEventId(vec![0x22; 32]),
            block_number: 100,
            block_hash: B256::from([0x33; 32]),
            parent_hash: B256::from([0x44; 32]),
            observed_at_unix_secs: Some(1_699_999_999),
            tx_hash: B256::from([0x55; 32]),
            tx_index: 6,
            log_index: 7,
            burner: Address::from([0x66; 20]),
            amount_base_units: "2814749767106559999847412109375".to_string(),
            amount_nicks: u64::MAX,
            lock_root: Tip5Hash::from_limbs(&[1, 2, 3, 4, 5]),
            calldata: vec![0x77; 116],
            base_batch_end: 199,
            withdrawal_nonce: Some(8),
            verified_at: 1_700_000_000,
            policy_id: Some(crate::shared::types::WITHDRAWAL_POLICY_V1_ID.to_string()),
            protocol_id: Some(crate::shared::types::WITHDRAWAL_WIRE_V1_ID.to_string()),
        }
    }

    #[tokio::test]
    async fn verified_burn_insert_is_idempotent_and_rejects_conflicts() {
        let (_directory, store) = open_store().await;
        let burn = sample_burn();

        assert!(store
            .insert_verified_burn(burn.clone())
            .await
            .expect("insert burn"));
        assert!(!store
            .insert_verified_burn(burn.clone())
            .await
            .expect("idempotent insert"));
        assert_eq!(
            store
                .load_verified_burn(burn.chain_id, burn.nock_contract_address, &burn.base_event_id,)
                .await
                .expect("load burn"),
            Some(burn.clone())
        );

        let mut conflict = burn;
        conflict.amount_base_units = "2000000000000000000000".to_string();
        assert!(store
            .insert_verified_burn(conflict)
            .await
            .expect_err("conflicting burn must fail")
            .to_string()
            .contains("conflicting immutable Base burn facts"));
    }

    #[tokio::test]
    async fn cursor_advances_monotonically_and_rejects_hash_conflicts() {
        let (_directory, store) = open_store().await;
        let cursor = BaseActivityCursor {
            chain_id: 8_453,
            nock_contract_address: Address::from([0x11; 20]),
            last_verified_block: 100,
            last_verified_block_hash: B256::from([0x33; 32]),
            updated_at: 1_700_000_000,
        };
        store
            .advance_cursor(cursor.clone())
            .await
            .expect("advance cursor");
        assert_eq!(
            store
                .load_cursor(cursor.chain_id, cursor.nock_contract_address)
                .await
                .expect("load cursor"),
            Some(cursor.clone())
        );
        store
            .advance_cursor(cursor.clone())
            .await
            .expect("idempotent cursor");

        let mut hash_conflict = cursor.clone();
        hash_conflict.last_verified_block_hash = B256::from([0x99; 32]);
        assert!(store
            .advance_cursor(hash_conflict)
            .await
            .expect_err("same-height hash conflict")
            .to_string()
            .contains("cursor hash mismatch"));

        let mut rewind = cursor;
        rewind.last_verified_block = 99;
        assert!(store
            .advance_cursor(rewind)
            .await
            .expect_err("cursor rewind")
            .to_string()
            .contains("cannot rewind"));
    }

    #[tokio::test]
    async fn verified_chunk_is_atomic_and_recovers_overlap_burns() {
        let (_directory, store) = open_store().await;
        let burn = sample_burn();
        let cursor = BaseActivityCursor {
            chain_id: burn.chain_id,
            nock_contract_address: burn.nock_contract_address,
            last_verified_block: burn.block_number,
            last_verified_block_hash: burn.block_hash,
            updated_at: burn.verified_at,
        };
        assert_eq!(
            store
                .apply_verified_chunk(vec![burn.clone()], cursor.clone())
                .await
                .expect("apply first verified chunk"),
            1
        );

        let mut overlap_burn = burn.clone();
        overlap_burn.base_event_id = BaseEventId(vec![0x88; 32]);
        overlap_burn.block_number = 99;
        overlap_burn.block_hash = B256::from([0x89; 32]);
        overlap_burn.tx_hash = B256::from([0x8a; 32]);
        overlap_burn.log_index = 9;
        assert_eq!(
            store
                .apply_verified_chunk(vec![overlap_burn.clone()], cursor.clone())
                .await
                .expect("recover overlap burn"),
            1
        );
        assert_eq!(
            store
                .load_cursor(cursor.chain_id, cursor.nock_contract_address)
                .await
                .expect("load retained cursor"),
            Some(cursor.clone())
        );

        let mut first = burn.clone();
        first.base_event_id = BaseEventId(vec![0x91; 32]);
        first.tx_hash = B256::from([0x92; 32]);
        first.log_index = 10;
        let mut conflict = burn.clone();
        conflict.amount_nicks = 1;
        let error = store
            .apply_verified_chunk(vec![first.clone(), conflict], cursor.clone())
            .await
            .expect_err("conflicting chunk must roll back");
        assert!(error
            .to_string()
            .contains("conflicting immutable Base burn facts"));
        assert_eq!(
            store
                .load_verified_burn(
                    first.chain_id, first.nock_contract_address, &first.base_event_id,
                )
                .await
                .expect("load rolled-back burn"),
            None
        );
    }

    #[tokio::test]
    async fn legacy_public_metadata_stays_absent_until_verified_rescan_backfills_it() {
        let directory = tempfile::TempDir::new().expect("temporary directory");
        let path = directory.path().join("legacy-sequencer.sqlite");
        {
            let mut conn = SqliteConnection::establish(path.to_string_lossy().as_ref())
                .expect("open legacy SQLite");
            conn.batch_execute(
                r#"
                CREATE TABLE sequencer_base_burns (
                    chain_id INTEGER NOT NULL,
                    nock_contract_address BLOB NOT NULL,
                    base_event_id BLOB NOT NULL,
                    block_number INTEGER NOT NULL,
                    block_hash BLOB NOT NULL,
                    parent_hash BLOB NOT NULL,
                    tx_hash BLOB NOT NULL,
                    tx_index INTEGER NOT NULL,
                    log_index INTEGER NOT NULL,
                    burner BLOB NOT NULL,
                    amount_base_units TEXT NOT NULL,
                    amount_nicks TEXT NOT NULL,
                    lock_root BLOB NOT NULL,
                    calldata BLOB NOT NULL,
                    base_batch_end INTEGER NOT NULL,
                    withdrawal_nonce INTEGER,
                    verified_at INTEGER NOT NULL,
                    PRIMARY KEY (chain_id, nock_contract_address, base_event_id)
                );
                "#,
            )
            .expect("create legacy Base activity table");
        }
        let withdrawal_store = WithdrawalSequencerStore::open(path)
            .await
            .expect("migrate withdrawal store");
        let activity = withdrawal_store.base_activity_store();
        let mut legacy = sample_burn();
        legacy.observed_at_unix_secs = None;
        legacy.policy_id = None;
        legacy.protocol_id = None;
        activity
            .insert_verified_burn(legacy.clone())
            .await
            .expect("insert legacy metadata row");
        assert_eq!(
            activity
                .load_verified_burn(
                    legacy.chain_id, legacy.nock_contract_address, &legacy.base_event_id,
                )
                .await
                .expect("load legacy metadata"),
            Some(legacy.clone())
        );

        let mut enriched = legacy;
        enriched.observed_at_unix_secs = Some(1_699_999_999);
        enriched.policy_id = Some(crate::shared::types::WITHDRAWAL_POLICY_V1_ID.to_string());
        enriched.protocol_id = Some(crate::shared::types::WITHDRAWAL_WIRE_V1_ID.to_string());
        assert!(!activity
            .insert_verified_burn(enriched.clone())
            .await
            .expect("backfill metadata from verified rescan"));
        assert_eq!(
            activity
                .load_verified_burn(
                    enriched.chain_id, enriched.nock_contract_address, &enriched.base_event_id,
                )
                .await
                .expect("load backfilled metadata"),
            Some(enriched)
        );
    }

    #[tokio::test]
    async fn public_metadata_queries_round_trip_by_event_transaction_and_burner() {
        let directory = tempfile::TempDir::new().expect("temporary directory");
        let path = directory.path().join("public-metadata.sqlite");
        let withdrawal_store = WithdrawalSequencerStore::open(path.clone())
            .await
            .expect("open withdrawal store");
        let activity = withdrawal_store.base_activity_store();
        let first = sample_burn();
        let mut second = first.clone();
        second.base_event_id = BaseEventId(vec![0x23; 32]);
        second.log_index = 8;
        let mut newest = first.clone();
        newest.base_event_id = BaseEventId(vec![0x24; 32]);
        newest.block_number = 101;
        newest.block_hash = B256::from([0x34; 32]);
        newest.parent_hash = first.block_hash;
        newest.observed_at_unix_secs = Some(1_700_000_001);
        newest.tx_hash = B256::from([0x56; 32]);
        newest.log_index = 1;
        for burn in [&first, &second, &newest] {
            activity
                .insert_verified_burn(burn.clone())
                .await
                .expect("insert public metadata burn");
        }

        assert_eq!(
            activity
                .load_verified_burn_by_tx_log(
                    first.chain_id, first.nock_contract_address, first.tx_hash, second.log_index,
                )
                .await
                .expect("lookup by tx/log"),
            Some(second.clone())
        );
        assert_eq!(
            activity
                .list_verified_burns_by_tx_hash(
                    first.chain_id, first.nock_contract_address, first.tx_hash,
                )
                .await
                .expect("list transaction burns"),
            vec![first.clone(), second.clone()]
        );
        assert_eq!(
            activity
                .list_verified_burns_by_burner(
                    first.chain_id, first.nock_contract_address, first.burner, 2,
                )
                .await
                .expect("list burner history"),
            vec![newest.clone(), second]
        );

        let reopened = WithdrawalSequencerStore::open(path)
            .await
            .expect("reopen withdrawal store");
        assert_eq!(
            reopened
                .base_activity_store()
                .load_verified_burn(
                    newest.chain_id, newest.nock_contract_address, &newest.base_event_id,
                )
                .await
                .expect("load metadata after restart"),
            Some(newest)
        );
    }
}
