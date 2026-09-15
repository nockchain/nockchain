use alloy::primitives::{Address, B256};
use deadpool_diesel::sqlite::Pool;
use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Binary, Nullable, Text};
use diesel::sqlite::SqliteConnection;

use crate::shared::base::compute_base_event_id;
use crate::shared::errors::BridgeError;
use crate::shared::types::BaseEventId;
use crate::withdrawal::sequencer::schema::sequencer_compensated_withdrawals;

const SQLITE_BUSY_TIMEOUT_MS: u64 = 2_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedBaseWithdrawalBurn {
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
    pub burner: Option<Address>,
    pub amount_base_units: Option<String>,
    pub commitment: Option<B256>,
    pub calldata: Vec<u8>,
    pub rejection_code: String,
    pub rejection_detail: String,
    pub first_observed_at: i64,
    pub last_observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompensatedBaseWithdrawal {
    pub chain_id: u64,
    pub nock_contract_address: Address,
    pub base_event_id: BaseEventId,
    pub tx_hash: B256,
    pub log_index: u64,
    pub reason: String,
    pub evidence_reference: String,
    pub recorded_at: i64,
}

#[derive(Clone)]
pub struct BaseIncidentStore {
    pool: Pool,
}

impl BaseIncidentStore {
    pub(crate) fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn record_rejected_burns(
        &self,
        records: Vec<RejectedBaseWithdrawalBurn>,
    ) -> Result<u64, BridgeError> {
        self.with_conn(move |conn| {
            conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                let mut inserted = 0_u64;
                for record in &records {
                    if insert_rejected_burn_checked(conn, record)? {
                        inserted = inserted.saturating_add(1);
                    }
                }
                Ok(inserted)
            })
            .map_err(|error| {
                BridgeError::Runtime(format!(
                    "Base withdrawal rejection transaction failed: {error}"
                ))
            })
        })
        .await
    }

    pub async fn load_rejected_burn(
        &self,
        chain_id: u64,
        nock_contract_address: Address,
        base_event_id: &BaseEventId,
    ) -> Result<Option<RejectedBaseWithdrawalBurn>, BridgeError> {
        let base_event_id = base_event_id.clone();
        self.with_conn(move |conn| {
            load_rejected_burn_row(conn, chain_id, nock_contract_address, &base_event_id)
        })
        .await
    }

    pub async fn load_rejected_burn_by_tx_log(
        &self,
        chain_id: u64,
        nock_contract_address: Address,
        tx_hash: B256,
        log_index: u64,
    ) -> Result<Option<RejectedBaseWithdrawalBurn>, BridgeError> {
        self.with_conn(move |conn| {
            load_rejected_burn_by_tx_log_row(
                conn, chain_id, nock_contract_address, tx_hash, log_index,
            )
        })
        .await
    }
    pub async fn list_rejected_burns_by_tx_hash(
        &self,
        chain_id: u64,
        nock_contract_address: Address,
        tx_hash: B256,
    ) -> Result<Vec<RejectedBaseWithdrawalBurn>, BridgeError> {
        self.with_conn(move |conn| {
            load_rejected_burn_rows_by_tx_hash(conn, chain_id, nock_contract_address, tx_hash)
        })
        .await
    }
    pub async fn list_rejected_burns_by_burner(
        &self,
        chain_id: u64,
        nock_contract_address: Address,
        burner: Address,
        limit: u32,
    ) -> Result<Vec<RejectedBaseWithdrawalBurn>, BridgeError> {
        self.with_conn(move |conn| {
            load_rejected_burn_rows_by_burner(conn, chain_id, nock_contract_address, burner, limit)
        })
        .await
    }

    pub async fn record_compensated_withdrawals(
        &self,
        records: Vec<CompensatedBaseWithdrawal>,
    ) -> Result<u64, BridgeError> {
        self.with_conn(move |conn| {
            conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                let mut inserted = 0_u64;
                for record in &records {
                    if insert_compensated_withdrawal_checked(conn, record)? {
                        inserted = inserted.saturating_add(1);
                    }
                }
                Ok(inserted)
            })
            .map_err(|error| {
                BridgeError::Runtime(format!(
                    "compensated withdrawal transaction failed: {error}"
                ))
            })
        })
        .await
    }

    pub async fn load_compensated_withdrawal(
        &self,
        chain_id: u64,
        nock_contract_address: Address,
        base_event_id: &BaseEventId,
    ) -> Result<Option<CompensatedBaseWithdrawal>, BridgeError> {
        let base_event_id = base_event_id.clone();
        self.with_conn(move |conn| {
            load_compensated_withdrawal_row(conn, chain_id, nock_contract_address, &base_event_id)
        })
        .await
    }
    pub async fn load_compensated_withdrawal_by_tx_log(
        &self,
        chain_id: u64,
        nock_contract_address: Address,
        tx_hash: B256,
        log_index: u64,
    ) -> Result<Option<CompensatedBaseWithdrawal>, BridgeError> {
        self.with_conn(move |conn| {
            load_compensated_withdrawal_by_tx_log_row(
                conn, chain_id, nock_contract_address, tx_hash, log_index,
            )
        })
        .await
    }
    pub async fn list_compensated_withdrawals_by_tx_hash(
        &self,
        chain_id: u64,
        nock_contract_address: Address,
        tx_hash: B256,
    ) -> Result<Vec<CompensatedBaseWithdrawal>, BridgeError> {
        self.with_conn(move |conn| {
            load_compensated_withdrawal_rows_by_tx_hash(
                conn, chain_id, nock_contract_address, tx_hash,
            )
        })
        .await
    }

    pub async fn list_compensated_withdrawals(
        &self,
        chain_id: u64,
        nock_contract_address: Address,
    ) -> Result<Vec<CompensatedBaseWithdrawal>, BridgeError> {
        self.with_conn(move |conn| {
            load_compensated_withdrawal_rows(conn, chain_id, nock_contract_address)
        })
        .await
    }
    pub async fn list_compensated_withdrawals_for_events(
        &self,
        chain_id: u64,
        nock_contract_address: Address,
        base_event_ids: Vec<BaseEventId>,
    ) -> Result<Vec<CompensatedBaseWithdrawal>, BridgeError> {
        self.with_conn(move |conn| {
            load_compensated_withdrawal_rows_for_events(
                conn, chain_id, nock_contract_address, &base_event_ids,
            )
        })
        .await
    }

    async fn with_conn<T, F>(&self, operation: F) -> Result<T, BridgeError>
    where
        F: FnOnce(&mut SqliteConnection) -> Result<T, BridgeError> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.pool.get().await.map_err(|error| {
            BridgeError::Runtime(format!("Base incident store pool failed: {error}"))
        })?;
        conn.interact(move |conn| {
            conn.batch_execute(&format!(
                "PRAGMA busy_timeout = {SQLITE_BUSY_TIMEOUT_MS}; PRAGMA foreign_keys = ON;"
            ))
            .map_err(|error| {
                BridgeError::Runtime(format!("Base incident store pragma failed: {error}"))
            })?;
            operation(conn)
        })
        .await
        .map_err(|error| {
            BridgeError::Runtime(format!("Base incident store interact failed: {error}"))
        })?
    }
}

pub(crate) fn ensure_base_incident_schema(conn: &mut SqliteConnection) -> Result<(), BridgeError> {
    conn.batch_execute(
        r#"
        CREATE TABLE IF NOT EXISTS sequencer_base_burn_rejections (
            chain_id INTEGER NOT NULL,
            nock_contract_address BLOB NOT NULL CHECK(length(nock_contract_address) = 20),
            base_event_id BLOB NOT NULL CHECK(length(base_event_id) = 32),
            block_number INTEGER NOT NULL,
            block_hash BLOB NOT NULL CHECK(length(block_hash) = 32),
            parent_hash BLOB NOT NULL CHECK(length(parent_hash) = 32),
            observed_at_unix_secs INTEGER,
            tx_hash BLOB NOT NULL CHECK(length(tx_hash) = 32),
            tx_index INTEGER NOT NULL,
            log_index INTEGER NOT NULL,
            burner BLOB CHECK(burner IS NULL OR length(burner) = 20),
            amount_base_units TEXT,
            commitment BLOB CHECK(commitment IS NULL OR length(commitment) = 32),
            calldata BLOB NOT NULL,
            rejection_code TEXT NOT NULL,
            rejection_detail TEXT NOT NULL,
            canonical INTEGER NOT NULL DEFAULT 1 CHECK(canonical IN (0, 1)),
            first_observed_at INTEGER NOT NULL,
            last_observed_at INTEGER NOT NULL,
            PRIMARY KEY (chain_id, nock_contract_address, base_event_id)
        );

        CREATE UNIQUE INDEX IF NOT EXISTS sequencer_base_burn_rejections_by_log
          ON sequencer_base_burn_rejections(
            chain_id, nock_contract_address, tx_hash, log_index
          );
        CREATE INDEX IF NOT EXISTS sequencer_base_burn_rejections_by_burner
          ON sequencer_base_burn_rejections(
            chain_id, nock_contract_address, burner,
            block_number DESC, log_index DESC, base_event_id DESC
          );

        CREATE TABLE IF NOT EXISTS sequencer_compensated_withdrawals (
            chain_id INTEGER NOT NULL,
            nock_contract_address BLOB NOT NULL CHECK(length(nock_contract_address) = 20),
            base_event_id BLOB NOT NULL CHECK(length(base_event_id) = 32),
            tx_hash BLOB NOT NULL CHECK(length(tx_hash) = 32),
            log_index INTEGER NOT NULL,
            reason TEXT NOT NULL CHECK(length(reason) > 0),
            evidence_reference TEXT NOT NULL CHECK(length(evidence_reference) > 0),
            recorded_at INTEGER NOT NULL,
            PRIMARY KEY (chain_id, nock_contract_address, base_event_id),
            UNIQUE (chain_id, nock_contract_address, tx_hash, log_index)
        );
        "#,
    )
    .map_err(|error| BridgeError::Runtime(format!("Base incident schema failed: {error}")))
}

fn insert_rejected_burn_checked(
    conn: &mut SqliteConnection,
    record: &RejectedBaseWithdrawalBurn,
) -> Result<bool, BridgeError> {
    validate_rejected_burn(record)?;
    let inserted = diesel::sql_query(
        r#"
        INSERT OR IGNORE INTO sequencer_base_burn_rejections (
            chain_id, nock_contract_address, base_event_id,
            block_number, block_hash, parent_hash, observed_at_unix_secs,
            tx_hash, tx_index, log_index, burner, amount_base_units,
            commitment, calldata, rejection_code, rejection_detail,
            first_observed_at, last_observed_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
    .bind::<Nullable<Binary>, _>(record.burner.map(|address| address.as_slice().to_vec()))
    .bind::<Nullable<Text>, _>(record.amount_base_units.clone())
    .bind::<Nullable<Binary>, _>(
        record
            .commitment
            .map(|commitment| commitment.as_slice().to_vec()),
    )
    .bind::<Binary, _>(record.calldata.clone())
    .bind::<Text, _>(record.rejection_code.clone())
    .bind::<Text, _>(record.rejection_detail.clone())
    .bind::<BigInt, _>(record.first_observed_at)
    .bind::<BigInt, _>(record.last_observed_at)
    .execute(conn)
    .map_err(|error| BridgeError::Runtime(format!("Base rejection insert failed: {error}")))?;

    let stored = load_rejected_burn_row(
        conn, record.chain_id, record.nock_contract_address, &record.base_event_id,
    )?
    .ok_or_else(|| {
        BridgeError::Runtime(format!(
            "Base rejection insert did not produce {:?}",
            record.base_event_id
        ))
    })?;
    let mut expected = record.clone();
    expected.first_observed_at = stored.first_observed_at;
    expected.last_observed_at = stored.last_observed_at;
    if stored != expected {
        return Err(BridgeError::Runtime(format!(
            "conflicting immutable Base rejection facts for {:?}",
            record.base_event_id
        )));
    }
    diesel::sql_query(
        r#"
        UPDATE sequencer_base_burn_rejections
        SET last_observed_at = MAX(last_observed_at, ?)
        WHERE chain_id = ? AND nock_contract_address = ? AND base_event_id = ?
        "#,
    )
    .bind::<BigInt, _>(record.last_observed_at)
    .bind::<BigInt, _>(u64_to_i64(record.chain_id, "chain_id")?)
    .bind::<Binary, _>(record.nock_contract_address.as_slice().to_vec())
    .bind::<Binary, _>(record.base_event_id.0.clone())
    .execute(conn)
    .map_err(|error| {
        BridgeError::Runtime(format!("Base rejection observation update failed: {error}"))
    })?;
    Ok(inserted == 1)
}

fn validate_rejected_burn(record: &RejectedBaseWithdrawalBurn) -> Result<(), BridgeError> {
    if record.base_event_id.0.len() != 32 {
        return Err(BridgeError::ValueConversion(
            "rejected Base event id must be 32 bytes".into(),
        ));
    }
    if record.rejection_code.trim().is_empty() || record.rejection_detail.trim().is_empty() {
        return Err(BridgeError::ValueConversion(
            "rejected Base burn requires code and detail".into(),
        ));
    }
    if record.first_observed_at > record.last_observed_at {
        return Err(BridgeError::ValueConversion(
            "rejected Base burn observation times are reversed".into(),
        ));
    }
    let expected_base_event_id = compute_base_event_id(&record.tx_hash, Some(record.log_index));
    if expected_base_event_id != record.base_event_id {
        return Err(BridgeError::ValueConversion(
            "rejected Base event id does not match transaction coordinate".into(),
        ));
    }
    Ok(())
}

fn insert_compensated_withdrawal_checked(
    conn: &mut SqliteConnection,
    record: &CompensatedBaseWithdrawal,
) -> Result<bool, BridgeError> {
    if record.base_event_id.0.len() != 32
        || record.reason.trim().is_empty()
        || record.evidence_reference.trim().is_empty()
    {
        return Err(BridgeError::ValueConversion(
            "compensated withdrawal requires exact identity, reason, and evidence".into(),
        ));
    }
    let expected_base_event_id = compute_base_event_id(&record.tx_hash, Some(record.log_index));
    if expected_base_event_id != record.base_event_id {
        return Err(BridgeError::ValueConversion(
            "compensated withdrawal base event id does not match transaction coordinate".into(),
        ));
    }
    let inserted = diesel::sql_query(
        r#"
        INSERT OR IGNORE INTO sequencer_compensated_withdrawals (
            chain_id, nock_contract_address, base_event_id,
            tx_hash, log_index, reason, evidence_reference, recorded_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(record.chain_id, "chain_id")?)
    .bind::<Binary, _>(record.nock_contract_address.as_slice().to_vec())
    .bind::<Binary, _>(record.base_event_id.0.clone())
    .bind::<Binary, _>(record.tx_hash.as_slice().to_vec())
    .bind::<BigInt, _>(u64_to_i64(record.log_index, "log_index")?)
    .bind::<Text, _>(record.reason.clone())
    .bind::<Text, _>(record.evidence_reference.clone())
    .bind::<BigInt, _>(record.recorded_at)
    .execute(conn)
    .map_err(|error| {
        BridgeError::Runtime(format!("compensated withdrawal insert failed: {error}"))
    })?;
    let stored = load_compensated_withdrawal_row(
        conn, record.chain_id, record.nock_contract_address, &record.base_event_id,
    )?
    .ok_or_else(|| {
        BridgeError::Runtime(format!(
            "compensated withdrawal insert did not produce {:?}",
            record.base_event_id
        ))
    })?;
    if stored != *record {
        return Err(BridgeError::Runtime(format!(
            "conflicting compensated withdrawal facts for {:?}",
            record.base_event_id
        )));
    }
    Ok(inserted == 1)
}

#[derive(QueryableByName)]
struct RejectedBurnSqlRow {
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
    #[diesel(sql_type = Nullable<BigInt>)]
    observed_at_unix_secs: Option<i64>,
    #[diesel(sql_type = Binary)]
    tx_hash: Vec<u8>,
    #[diesel(sql_type = BigInt)]
    tx_index: i64,
    #[diesel(sql_type = BigInt)]
    log_index: i64,
    #[diesel(sql_type = Nullable<Binary>)]
    burner: Option<Vec<u8>>,
    #[diesel(sql_type = Nullable<Text>)]
    amount_base_units: Option<String>,
    #[diesel(sql_type = Nullable<Binary>)]
    commitment: Option<Vec<u8>>,
    #[diesel(sql_type = Binary)]
    calldata: Vec<u8>,
    #[diesel(sql_type = Text)]
    rejection_code: String,
    #[diesel(sql_type = Text)]
    rejection_detail: String,
    #[diesel(sql_type = BigInt)]
    first_observed_at: i64,
    #[diesel(sql_type = BigInt)]
    last_observed_at: i64,
}

impl TryFrom<RejectedBurnSqlRow> for RejectedBaseWithdrawalBurn {
    type Error = BridgeError;

    fn try_from(row: RejectedBurnSqlRow) -> Result<Self, Self::Error> {
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
            burner: row
                .burner
                .map(|value| address_from_bytes(&value, "burner"))
                .transpose()?,
            amount_base_units: row.amount_base_units,
            commitment: row
                .commitment
                .map(|value| fixed_bytes::<32>(&value, "commitment").map(B256::from))
                .transpose()?,
            calldata: row.calldata,
            rejection_code: row.rejection_code,
            rejection_detail: row.rejection_detail,
            first_observed_at: row.first_observed_at,
            last_observed_at: row.last_observed_at,
        })
    }
}

fn load_rejected_burn_row(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
    base_event_id: &BaseEventId,
) -> Result<Option<RejectedBaseWithdrawalBurn>, BridgeError> {
    rejected_burn_query(
        conn,
        "chain_id = ? AND nock_contract_address = ? AND base_event_id = ? AND canonical = 1",
        chain_id,
        nock_contract_address,
        base_event_id.0.clone(),
        None,
    )
}

fn load_rejected_burn_by_tx_log_row(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
    tx_hash: B256,
    log_index: u64,
) -> Result<Option<RejectedBaseWithdrawalBurn>, BridgeError> {
    rejected_burn_query(
        conn,
        "chain_id = ? AND nock_contract_address = ? AND tx_hash = ? AND log_index = ? AND canonical = 1",
        chain_id,
        nock_contract_address,
        tx_hash.as_slice().to_vec(),
        Some(log_index),
    )
}
fn load_rejected_burn_rows_by_tx_hash(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
    tx_hash: B256,
) -> Result<Vec<RejectedBaseWithdrawalBurn>, BridgeError> {
    diesel::sql_query(
        r#"
        SELECT chain_id, nock_contract_address, base_event_id,
               block_number, block_hash, parent_hash, observed_at_unix_secs,
               tx_hash, tx_index, log_index, burner, amount_base_units,
               commitment, calldata, rejection_code, rejection_detail,
               first_observed_at, last_observed_at
        FROM sequencer_base_burn_rejections
        WHERE chain_id = ? AND nock_contract_address = ? AND tx_hash = ?
          AND canonical = 1
        ORDER BY log_index ASC
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(chain_id, "chain_id")?)
    .bind::<Binary, _>(nock_contract_address.as_slice().to_vec())
    .bind::<Binary, _>(tx_hash.as_slice().to_vec())
    .load::<RejectedBurnSqlRow>(conn)
    .map_err(|error| BridgeError::Runtime(format!("Base rejection list failed: {error}")))?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}
fn load_rejected_burn_rows_by_burner(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
    burner: Address,
    limit: u32,
) -> Result<Vec<RejectedBaseWithdrawalBurn>, BridgeError> {
    diesel::sql_query(
        r#"
        SELECT chain_id, nock_contract_address, base_event_id,
               block_number, block_hash, parent_hash, observed_at_unix_secs,
               tx_hash, tx_index, log_index, burner, amount_base_units,
               commitment, calldata, rejection_code, rejection_detail,
               first_observed_at, last_observed_at
        FROM sequencer_base_burn_rejections
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
    .load::<RejectedBurnSqlRow>(conn)
    .map_err(|error| BridgeError::Runtime(format!("Base rejection history failed: {error}")))?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

#[derive(QueryableByName)]
struct CountRow {
    #[diesel(sql_type = BigInt)]
    value: i64,
}

pub(crate) fn compensated_withdrawal_exists(
    conn: &mut SqliteConnection,
    base_event_id: &BaseEventId,
) -> Result<bool, BridgeError> {
    let count = diesel::sql_query(
        r#"
        SELECT COUNT(*) AS value
        FROM sequencer_compensated_withdrawals
        WHERE base_event_id = ?
        "#,
    )
    .bind::<Binary, _>(base_event_id.0.clone())
    .get_result::<CountRow>(conn)
    .map_err(|error| {
        BridgeError::Runtime(format!(
            "compensated withdrawal existence check failed: {error}"
        ))
    })?
    .value;
    Ok(count > 0)
}

fn rejected_burn_query(
    conn: &mut SqliteConnection,
    predicate: &str,
    chain_id: u64,
    nock_contract_address: Address,
    identity: Vec<u8>,
    log_index: Option<u64>,
) -> Result<Option<RejectedBaseWithdrawalBurn>, BridgeError> {
    let sql = format!(
        r#"
        SELECT chain_id, nock_contract_address, base_event_id,
               block_number, block_hash, parent_hash, observed_at_unix_secs,
               tx_hash, tx_index, log_index, burner, amount_base_units,
               commitment, calldata, rejection_code, rejection_detail,
               first_observed_at, last_observed_at
        FROM sequencer_base_burn_rejections
        WHERE {predicate}
        "#
    );
    let query = diesel::sql_query(sql)
        .bind::<BigInt, _>(u64_to_i64(chain_id, "chain_id")?)
        .bind::<Binary, _>(nock_contract_address.as_slice().to_vec())
        .bind::<Binary, _>(identity);
    let row = if let Some(log_index) = log_index {
        query
            .bind::<BigInt, _>(u64_to_i64(log_index, "log_index")?)
            .get_result::<RejectedBurnSqlRow>(conn)
            .optional()
    } else {
        query.get_result::<RejectedBurnSqlRow>(conn).optional()
    }
    .map_err(|error| BridgeError::Runtime(format!("Base rejection load failed: {error}")))?;
    row.map(TryInto::try_into).transpose()
}

#[derive(Queryable, QueryableByName)]
struct CompensatedWithdrawalSqlRow {
    #[diesel(sql_type = BigInt)]
    chain_id: i64,
    #[diesel(sql_type = Binary)]
    nock_contract_address: Vec<u8>,
    #[diesel(sql_type = Binary)]
    base_event_id: Vec<u8>,
    #[diesel(sql_type = Binary)]
    tx_hash: Vec<u8>,
    #[diesel(sql_type = BigInt)]
    log_index: i64,
    #[diesel(sql_type = Text)]
    reason: String,
    #[diesel(sql_type = Text)]
    evidence_reference: String,
    #[diesel(sql_type = BigInt)]
    recorded_at: i64,
}

impl TryFrom<CompensatedWithdrawalSqlRow> for CompensatedBaseWithdrawal {
    type Error = BridgeError;

    fn try_from(row: CompensatedWithdrawalSqlRow) -> Result<Self, Self::Error> {
        Ok(Self {
            chain_id: i64_to_u64(row.chain_id, "chain_id")?,
            nock_contract_address: address_from_bytes(
                &row.nock_contract_address, "nock_contract_address",
            )?,
            base_event_id: BaseEventId(
                fixed_bytes::<32>(&row.base_event_id, "base_event_id")?.to_vec(),
            ),
            tx_hash: B256::from(fixed_bytes::<32>(&row.tx_hash, "tx_hash")?),
            log_index: i64_to_u64(row.log_index, "log_index")?,
            reason: row.reason,
            evidence_reference: row.evidence_reference,
            recorded_at: row.recorded_at,
        })
    }
}

fn load_compensated_withdrawal_row(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
    base_event_id: &BaseEventId,
) -> Result<Option<CompensatedBaseWithdrawal>, BridgeError> {
    diesel::sql_query(
        r#"
        SELECT chain_id, nock_contract_address, base_event_id,
               tx_hash, log_index, reason, evidence_reference, recorded_at
        FROM sequencer_compensated_withdrawals
        WHERE chain_id = ? AND nock_contract_address = ? AND base_event_id = ?
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(chain_id, "chain_id")?)
    .bind::<Binary, _>(nock_contract_address.as_slice().to_vec())
    .bind::<Binary, _>(base_event_id.0.clone())
    .get_result::<CompensatedWithdrawalSqlRow>(conn)
    .optional()
    .map_err(|error| BridgeError::Runtime(format!("compensated withdrawal load failed: {error}")))?
    .map(TryInto::try_into)
    .transpose()
}

fn load_compensated_withdrawal_by_tx_log_row(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
    tx_hash: B256,
    log_index: u64,
) -> Result<Option<CompensatedBaseWithdrawal>, BridgeError> {
    diesel::sql_query(
        r#"
        SELECT chain_id, nock_contract_address, base_event_id,
               tx_hash, log_index, reason, evidence_reference, recorded_at
        FROM sequencer_compensated_withdrawals
        WHERE chain_id = ? AND nock_contract_address = ?
          AND tx_hash = ? AND log_index = ?
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(chain_id, "chain_id")?)
    .bind::<Binary, _>(nock_contract_address.as_slice().to_vec())
    .bind::<Binary, _>(tx_hash.as_slice().to_vec())
    .bind::<BigInt, _>(u64_to_i64(log_index, "log_index")?)
    .get_result::<CompensatedWithdrawalSqlRow>(conn)
    .optional()
    .map_err(|error| {
        BridgeError::Runtime(format!(
            "compensated withdrawal tx/log lookup failed: {error}"
        ))
    })?
    .map(TryInto::try_into)
    .transpose()
}

fn load_compensated_withdrawal_rows_by_tx_hash(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
    tx_hash: B256,
) -> Result<Vec<CompensatedBaseWithdrawal>, BridgeError> {
    diesel::sql_query(
        r#"
        SELECT chain_id, nock_contract_address, base_event_id,
               tx_hash, log_index, reason, evidence_reference, recorded_at
        FROM sequencer_compensated_withdrawals
        WHERE chain_id = ? AND nock_contract_address = ? AND tx_hash = ?
        ORDER BY log_index ASC
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(chain_id, "chain_id")?)
    .bind::<Binary, _>(nock_contract_address.as_slice().to_vec())
    .bind::<Binary, _>(tx_hash.as_slice().to_vec())
    .load::<CompensatedWithdrawalSqlRow>(conn)
    .map_err(|error| {
        BridgeError::Runtime(format!("compensated withdrawal tx list failed: {error}"))
    })?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

fn load_compensated_withdrawal_rows(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
) -> Result<Vec<CompensatedBaseWithdrawal>, BridgeError> {
    diesel::sql_query(
        r#"
        SELECT chain_id, nock_contract_address, base_event_id,
               tx_hash, log_index, reason, evidence_reference, recorded_at
        FROM sequencer_compensated_withdrawals
        WHERE chain_id = ? AND nock_contract_address = ?
        ORDER BY recorded_at ASC, base_event_id ASC
        "#,
    )
    .bind::<BigInt, _>(u64_to_i64(chain_id, "chain_id")?)
    .bind::<Binary, _>(nock_contract_address.as_slice().to_vec())
    .load::<CompensatedWithdrawalSqlRow>(conn)
    .map_err(|error| BridgeError::Runtime(format!("compensated withdrawal list failed: {error}")))?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}
fn load_compensated_withdrawal_rows_for_events(
    conn: &mut SqliteConnection,
    chain_id: u64,
    nock_contract_address: Address,
    base_event_ids: &[BaseEventId],
) -> Result<Vec<CompensatedBaseWithdrawal>, BridgeError> {
    use crate::withdrawal::sequencer::schema::sequencer_compensated_withdrawals::dsl as compensated;

    if base_event_ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids = base_event_ids
        .iter()
        .map(|id| id.0.clone())
        .collect::<Vec<_>>();
    sequencer_compensated_withdrawals::table
        .filter(compensated::chain_id.eq(u64_to_i64(chain_id, "chain_id")?))
        .filter(compensated::nock_contract_address.eq(nock_contract_address.as_slice().to_vec()))
        .filter(compensated::base_event_id.eq_any(ids))
        .order((
            compensated::recorded_at.asc(),
            compensated::base_event_id.asc(),
        ))
        .load::<CompensatedWithdrawalSqlRow>(conn)
        .map_err(|error| {
            BridgeError::Runtime(format!("compensated withdrawal batch list failed: {error}"))
        })?
        .into_iter()
        .map(TryInto::try_into)
        .collect()
}

fn u64_to_i64(value: u64, field: &str) -> Result<i64, BridgeError> {
    i64::try_from(value)
        .map_err(|error| BridgeError::ValueConversion(format!("{field} overflow: {error}")))
}

fn i64_to_u64(value: i64, field: &str) -> Result<u64, BridgeError> {
    u64::try_from(value)
        .map_err(|error| BridgeError::ValueConversion(format!("{field} overflow: {error}")))
}

fn fixed_bytes<const N: usize>(value: &[u8], field: &str) -> Result<[u8; N], BridgeError> {
    value.try_into().map_err(|_| {
        BridgeError::ValueConversion(format!("{field} has {} bytes, expected {N}", value.len()))
    })
}

fn address_from_bytes(value: &[u8], field: &str) -> Result<Address, BridgeError> {
    Ok(Address::from(fixed_bytes::<20>(value, field)?))
}

#[cfg(test)]
mod tests {
    use alloy::primitives::{Address, B256};
    use tempfile::tempdir;

    use super::{CompensatedBaseWithdrawal, RejectedBaseWithdrawalBurn};
    use crate::shared::base::compute_base_event_id;
    use crate::withdrawal::sequencer::store::WithdrawalSequencerStore;

    fn rejected() -> RejectedBaseWithdrawalBurn {
        let tx_hash = B256::repeat_byte(0x55);
        let log_index = 4;
        RejectedBaseWithdrawalBurn {
            chain_id: 8_453,
            nock_contract_address: Address::repeat_byte(0x11),
            base_event_id: compute_base_event_id(&tx_hash, Some(log_index)),
            block_number: 100,
            block_hash: B256::repeat_byte(0x33),
            parent_hash: B256::repeat_byte(0x44),
            observed_at_unix_secs: Some(1_700_000_000),
            tx_hash,
            tx_index: 3,
            log_index,
            burner: Some(Address::repeat_byte(0x66)),
            amount_base_units: Some("10000000000000000".to_string()),
            commitment: Some(B256::repeat_byte(0x77)),
            calldata: vec![0x88; 68],
            rejection_code: "missing_calldata_trailer".to_string(),
            rejection_detail: "missing withdrawal trailer".to_string(),
            first_observed_at: 10,
            last_observed_at: 10,
        }
    }

    #[tokio::test]
    async fn rejection_is_durable_idempotent_and_conflict_checked() {
        let directory = tempdir().expect("temporary directory");
        let store = WithdrawalSequencerStore::open(directory.path().join("state.sqlite"))
            .await
            .expect("open store");
        let incidents = store.base_activity_store().incident_store();
        let record = rejected();

        assert_eq!(
            incidents
                .record_rejected_burns(vec![record.clone()])
                .await
                .expect("record rejection"),
            1
        );
        let mut replay = record.clone();
        replay.first_observed_at = 20;
        replay.last_observed_at = 20;
        assert_eq!(
            incidents
                .record_rejected_burns(vec![replay])
                .await
                .expect("replay rejection"),
            0
        );
        let stored = incidents
            .load_rejected_burn(
                record.chain_id, record.nock_contract_address, &record.base_event_id,
            )
            .await
            .expect("load rejection")
            .expect("rejection exists");
        assert_eq!(stored.first_observed_at, 10);
        assert_eq!(stored.last_observed_at, 20);

        let mut conflict = record;
        conflict.rejection_code = "malformed_calldata".to_string();
        assert!(incidents
            .record_rejected_burns(vec![conflict])
            .await
            .expect_err("conflicting rejection must fail")
            .to_string()
            .contains("conflicting immutable"));
    }

    #[tokio::test]
    async fn compensation_is_durable_idempotent_and_immutable() {
        let directory = tempdir().expect("temporary directory");
        let store = WithdrawalSequencerStore::open(directory.path().join("state.sqlite"))
            .await
            .expect("open store");
        let incidents = store.base_activity_store().incident_store();
        let tx_hash = B256::repeat_byte(0xaa);
        let log_index = 7;
        let record = CompensatedBaseWithdrawal {
            chain_id: 8_453,
            nock_contract_address: Address::repeat_byte(0x11),
            base_event_id: compute_base_event_id(&tx_hash, Some(log_index)),
            tx_hash,
            log_index,
            reason: "governance-approved compensation".to_string(),
            evidence_reference: "incident-123".to_string(),
            recorded_at: 42,
        };

        assert_eq!(
            incidents
                .record_compensated_withdrawals(vec![record.clone()])
                .await
                .expect("record compensation"),
            1
        );
        assert_eq!(
            incidents
                .record_compensated_withdrawals(vec![record.clone()])
                .await
                .expect("replay compensation"),
            0
        );
        assert!(store
            .is_compensated_withdrawal(&record.base_event_id)
            .await
            .expect("check compensation"));

        let mut conflict = record;
        conflict.reason = "different reason".to_string();
        assert!(incidents
            .record_compensated_withdrawals(vec![conflict])
            .await
            .expect_err("conflicting compensation must fail")
            .to_string()
            .contains("conflicting compensated"));
    }
}
