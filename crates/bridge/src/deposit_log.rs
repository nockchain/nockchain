use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use deadpool_diesel::sqlite::{Manager, Pool};
use deadpool_diesel::Runtime;
use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use diesel::OptionalExtension;
use nockchain_types::v1::Name;
use tracing::{info, warn};

use crate::bridge_status::BridgeStatus;
use crate::config::NonceEpochConfig;
use crate::errors::BridgeError;
use crate::ethereum::BaseBridge;
use crate::metrics;
use crate::schema::deposit_log;
use crate::stop::StopHandle;
use crate::tui::state::TuiStatus;
use crate::tui::types::{DepositLogSnapshot, DepositLogView, DEPOSIT_LOG_PAGE_SIZE};
use crate::types::{DepositId, EthAddress, Tip5Hash};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepositLogEntry {
    pub block_height: u64,
    pub tx_id: Tip5Hash,
    pub as_of: Tip5Hash,
    pub name: Name,
    pub recipient: EthAddress,
    pub amount_to_mint: u64,
}

#[derive(Insertable)]
#[diesel(table_name = deposit_log)]
struct NewDepositLogRow {
    tx_id: Vec<u8>,
    block_height: i64,
    as_of: Vec<u8>,
    name_first: Vec<u8>,
    name_last: Vec<u8>,
    recipient: Vec<u8>,
    amount_to_mint: i64,
}

/// Insert outcomes for idempotent inserts. Conflicting rows return an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepositLogInsertOutcome {
    Inserted,
    ExistingMatch,
    ExistingMismatch,
}

#[derive(Queryable)]
struct DepositLogRow {
    tx_id: Vec<u8>,
    block_height: i64,
    as_of: Vec<u8>,
    name_first: Vec<u8>,
    name_last: Vec<u8>,
    recipient: Vec<u8>,
    amount_to_mint: i64,
}

pub struct DepositLog {
    pool: Pool,
    read_only_pool: Pool,
}

impl DepositLog {
    /// Open a deposit log at the given path, creating schema/indexes if missing.
    /// This does not load any rows into memory and only validates that the DB
    /// is reachable and compatible with the expected schema.
    pub async fn open(path: PathBuf) -> Result<Self, BridgeError> {
        let pool = sqlite_pool(&path, SqlitePoolMode::ReadWrite)?;
        let read_only_pool = sqlite_pool(&path, SqlitePoolMode::ReadOnly)?;
        let log = Self {
            pool,
            read_only_pool,
        };
        log.ensure_schema().await?;
        Ok(log)
    }

    /// Open the default per-node deposit log path under the system data directory.
    /// This is the standard location used by the bridge runtime.
    pub async fn open_default() -> Result<Self, BridgeError> {
        Self::open(default_deposit_log_path()?).await
    }

    async fn with_conn<T, F>(&self, f: F) -> Result<T, BridgeError>
    where
        F: FnOnce(&mut SqliteConnection) -> Result<T, BridgeError> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|err| BridgeError::Runtime(format!("deposit log pool failed: {err}")))?;
        let result = conn
            .interact(move |conn| {
                conn.batch_execute(&format!(
                    "PRAGMA busy_timeout = {};",
                    SQLITE_BUSY_TIMEOUT_MS
                ))
                .map_err(|err| BridgeError::Runtime(format!("deposit log pragma failed: {err}")))?;
                f(conn)
            })
            .await
            .map_err(|err| BridgeError::Runtime(format!("deposit log interact failed: {err}")))?;
        result
    }

    /// Insert a deposit log entry if the tx_id is not already present.
    /// Returns whether the insert happened, or if an existing row matches.
    /// Conflicting tx_id rows are treated as fatal errors.
    /// This is the only write path used by CDC and backfill.
    pub async fn insert_entry(
        &self,
        entry: &DepositLogEntry,
    ) -> Result<DepositLogInsertOutcome, BridgeError> {
        let row = NewDepositLogRow::try_from_entry(entry)?;
        let tx_id_hex = hex::encode(&row.tx_id);
        let inserted = self
            .with_conn(move |conn| {
                diesel::insert_into(deposit_log::table)
                    .values(&row)
                    .on_conflict_do_nothing()
                    .execute(conn)
                    .map_err(|err| {
                        BridgeError::Runtime(format!("deposit log insert failed: {err}"))
                    })
            })
            .await?;
        if inserted > 0 {
            return Ok(DepositLogInsertOutcome::Inserted);
        }

        let existing = self.fetch_entry(&entry.tx_id).await?;
        let Some(existing) = existing else {
            return Err(BridgeError::Runtime(
                "deposit log insert conflict without existing row".into(),
            ));
        };

        if &existing == entry {
            warn!(
                tx_id_bytes = %tx_id_hex,
                block_height = entry.block_height,
                "deposit log already has tx_id, skipping duplicate"
            );
            return Ok(DepositLogInsertOutcome::ExistingMatch);
        }

        Err(BridgeError::InvalidDepositLogEntry(format!(
            "deposit log tx_id conflict for {tx_id_hex}: existing={existing:?} incoming={entry:?}"
        )))
    }

    pub async fn compute_nonce(
        &self,
        entry: &DepositLogEntry,
        epoch: &NonceEpochConfig,
    ) -> Result<Option<u64>, BridgeError> {
        // Only compute nonces for entries at/after the epoch start key.
        if epoch.is_before_start_key(entry.block_height, &entry.tx_id) {
            return Ok(None);
        }
        // Count how many deposits in the epoch sort order come before this entry.
        let height_i64 = i64::try_from(entry.block_height).map_err(|err| {
            BridgeError::ValueConversion(format!("block_height too large for sqlite: {err}"))
        })?;
        let epoch_start = i64::try_from(epoch.start_height).map_err(|err| {
            BridgeError::ValueConversion(format!(
                "nonce_epoch_start_height too large for sqlite: {err}"
            ))
        })?;
        let tx_id_bytes = entry.tx_id.to_be_limb_bytes().to_vec();
        let start_tx_id_bytes = epoch
            .start_tx_id
            .as_ref()
            .map(Tip5Hash::to_be_limb_bytes)
            .map(Vec::from);
        let count: i64 = self
            .with_conn(move |conn| {
                use crate::schema::deposit_log::dsl::{
                    block_height, deposit_log as deposit_log_table, tx_id,
                };
                // Apply epoch start key filtering, then count rows strictly before this entry.
                let mut query = deposit_log_table.into_boxed();
                query = if let Some(start_tx_id_bytes) = start_tx_id_bytes {
                    // Inclusive start key in lexicographic (block_height, tx_id) order:
                    // (block_height, tx_id) >= (epoch_start, start_tx_id).
                    query.filter(
                        block_height.gt(epoch_start).or(block_height
                            .eq(epoch_start)
                            .and(tx_id.ge(start_tx_id_bytes))),
                    )
                } else {
                    query.filter(block_height.ge(epoch_start))
                };
                query
                    .filter(
                        block_height
                            .lt(height_i64)
                            .or(block_height.eq(height_i64).and(tx_id.lt(tx_id_bytes))),
                    )
                    .count()
                    .get_result(conn)
                    .map_err(|err| BridgeError::Runtime(format!("deposit log count failed: {err}")))
            })
            .await?;
        let index = u64::try_from(count).map_err(|err| {
            BridgeError::ValueConversion(format!("deposit log count overflow: {err}"))
        })?;
        // Nonce is epoch base + index (or base+1 if no start tx-id is defined).
        Ok(Some(epoch.first_epoch_nonce().saturating_add(index)))
    }

    /// Count the number of deposits at/after the epoch start key.
    /// This is used to validate local history against the chain nonce prefix.
    pub async fn number_of_deposits_in_epoch(
        &self,
        epoch: &NonceEpochConfig,
    ) -> Result<u64, BridgeError> {
        let epoch_start = i64::try_from(epoch.start_height).map_err(|err| {
            BridgeError::ValueConversion(format!(
                "nonce_epoch_start_height too large for sqlite: {err}"
            ))
        })?;
        let start_tx_id_bytes = epoch
            .start_tx_id
            .as_ref()
            .map(Tip5Hash::to_be_limb_bytes)
            .map(Vec::from);
        let count: i64 = self
            .with_conn(move |conn| {
                use crate::schema::deposit_log::dsl::{
                    block_height, deposit_log as deposit_log_table,
                };
                let mut query = deposit_log_table.into_boxed();
                if let Some(start_tx_id_bytes) = start_tx_id_bytes {
                    use crate::schema::deposit_log::dsl::tx_id;
                    query = query.filter(
                        block_height.gt(epoch_start).or(block_height
                            .eq(epoch_start)
                            .and(tx_id.ge(start_tx_id_bytes))),
                    );
                } else {
                    query = query.filter(block_height.ge(epoch_start));
                }
                query
                    .count()
                    .get_result(conn)
                    .map_err(|err| BridgeError::Runtime(format!("deposit log count failed: {err}")))
            })
            .await?;
        u64::try_from(count).map_err(|err| {
            BridgeError::ValueConversion(format!("deposit log count overflow: {err}"))
        })
    }

    /// Fetch a single entry by nonce, if it exists in the epoch window.
    /// Returns None if the nonce is before the epoch or beyond the log length.
    pub async fn get_by_nonce(
        &self,
        nonce: u64,
        epoch: &NonceEpochConfig,
    ) -> Result<Option<DepositLogEntry>, BridgeError> {
        let mut rows = self.records_from_nonce(nonce, 1, epoch).await?;
        Ok(rows.pop().map(|(_, entry)| entry))
    }

    /// Fetch a contiguous range of entries starting at `start_nonce`, in nonce order.
    /// The output includes the nonce alongside each entry for display and signing.
    pub async fn records_from_nonce(
        &self,
        start_nonce: u64,
        limit: usize,
        epoch: &NonceEpochConfig,
    ) -> Result<Vec<(u64, DepositLogEntry)>, BridgeError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let first_epoch_nonce = epoch.first_epoch_nonce();
        if start_nonce < first_epoch_nonce {
            return Ok(Vec::new());
        }
        let start_index = start_nonce - first_epoch_nonce;
        let Ok(start_index) = i64::try_from(start_index) else {
            return Ok(Vec::new());
        };

        // Query rows in canonical order (block_height, tx_id), then offset into the epoch.
        let epoch_start = i64::try_from(epoch.start_height).map_err(|err| {
            BridgeError::ValueConversion(format!(
                "nonce_epoch_start_height too large for sqlite: {err}"
            ))
        })?;

        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
        let start_tx_id_bytes = epoch
            .start_tx_id
            .as_ref()
            .map(Tip5Hash::to_be_limb_bytes)
            .map(Vec::from);
        let rows: Vec<DepositLogRow> = self
            .with_conn(move |conn| {
                use crate::schema::deposit_log::dsl::{
                    block_height, deposit_log as deposit_log_table, tx_id,
                };
                let mut query = deposit_log_table.into_boxed();
                query = if let Some(start_tx_id_bytes) = start_tx_id_bytes {
                    query.filter(
                        block_height.gt(epoch_start).or(block_height
                            .eq(epoch_start)
                            .and(tx_id.ge(start_tx_id_bytes))),
                    )
                } else {
                    query.filter(block_height.ge(epoch_start))
                };
                query
                    .order((block_height.asc(), tx_id.asc()))
                    .offset(start_index)
                    .limit(limit_i64)
                    .load(conn)
                    .map_err(|err| BridgeError::Runtime(format!("deposit log range failed: {err}")))
            })
            .await?;

        let mut out = Vec::with_capacity(rows.len());
        for (offset, row) in rows.into_iter().enumerate() {
            let nonce = start_nonce.saturating_add(offset as u64);
            out.push((nonce, DepositLogEntry::try_from_row(row)?));
        }
        Ok(out)
    }

    pub async fn snapshot(
        &self,
        nonce_epoch: &NonceEpochConfig,
        view: DepositLogView,
    ) -> Result<DepositLogSnapshot, BridgeError> {
        let conn = self
            .read_only_pool
            .get()
            .await
            .map_err(|err| BridgeError::Runtime(format!("deposit log pool failed: {err}")))?;
        let nonce_epoch = nonce_epoch.clone();
        conn.interact(move |conn| {
            conn.batch_execute(&format!(
                "PRAGMA busy_timeout = {};",
                SQLITE_BUSY_TIMEOUT_MS
            ))
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to configure read-only deposit log connection: {err}"
                ))
            })?;
            fetch_deposit_log_snapshot(conn, &nonce_epoch, view)
        })
        .await
        .map_err(|err| BridgeError::Runtime(format!("deposit log interact failed: {err}")))?
    }

    /// Return the maximum block height present at/after the epoch start key.
    /// Used for incremental backfill scans.
    pub async fn max_block_height(
        &self,
        epoch: &NonceEpochConfig,
    ) -> Result<Option<u64>, BridgeError> {
        let epoch_start = i64::try_from(epoch.start_height).map_err(|err| {
            BridgeError::ValueConversion(format!(
                "nonce_epoch_start_height too large for sqlite: {err}"
            ))
        })?;
        let start_tx_id_bytes = epoch
            .start_tx_id
            .as_ref()
            .map(Tip5Hash::to_be_limb_bytes)
            .map(Vec::from);
        let max: Option<i64> = self
            .with_conn(move |conn| {
                use crate::schema::deposit_log::dsl::{
                    block_height, deposit_log as deposit_log_table,
                };
                let mut query = deposit_log_table.into_boxed();
                if let Some(start_tx_id_bytes) = start_tx_id_bytes {
                    use crate::schema::deposit_log::dsl::tx_id;
                    query = query.filter(
                        block_height.gt(epoch_start).or(block_height
                            .eq(epoch_start)
                            .and(tx_id.ge(start_tx_id_bytes))),
                    );
                } else {
                    query = query.filter(block_height.ge(epoch_start));
                }
                query
                    .select(block_height)
                    .order(block_height.desc())
                    .first(conn)
                    .optional()
                    .map_err(|err| BridgeError::Runtime(format!("deposit log max failed: {err}")))
            })
            .await?;
        let Some(max) = max else {
            return Ok(None);
        };
        let max_u64 = u64::try_from(max).map_err(|err| {
            BridgeError::ValueConversion(format!("deposit log max overflow: {err}"))
        })?;
        Ok(Some(max_u64))
    }

    /// Fetch a single entry by tx_id, if present.
    /// This is used to resolve insert conflicts and to probe for the epoch start tx-id.
    async fn fetch_entry(&self, tx_id: &Tip5Hash) -> Result<Option<DepositLogEntry>, BridgeError> {
        let tx_id_bytes = tx_id.to_be_limb_bytes().to_vec();
        let row = self
            .with_conn(move |conn| {
                use crate::schema::deposit_log::dsl::{
                    deposit_log as deposit_log_table, tx_id as tx_id_col,
                };
                deposit_log_table
                    .filter(tx_id_col.eq(tx_id_bytes))
                    .first::<DepositLogRow>(conn)
                    .optional()
                    .map_err(|err| BridgeError::Runtime(format!("deposit log fetch failed: {err}")))
            })
            .await?;
        row.map(DepositLogEntry::try_from_row).transpose()
    }

    /// Fetch a single entry by deposit_id (as_of + name), if present.
    async fn fetch_entry_by_deposit_id(
        &self,
        deposit_id: &DepositId,
    ) -> Result<Option<DepositLogEntry>, BridgeError> {
        let as_of_bytes = deposit_id.as_of.to_be_limb_bytes().to_vec();
        let name_first_bytes = deposit_id.name.first.to_be_limb_bytes().to_vec();
        let name_last_bytes = deposit_id.name.last.to_be_limb_bytes().to_vec();
        let row = self
            .with_conn(move |conn| {
                use crate::schema::deposit_log::dsl::{
                    as_of, deposit_log as deposit_log_table, name_first, name_last,
                };
                deposit_log_table
                    .filter(as_of.eq(as_of_bytes))
                    .filter(name_first.eq(name_first_bytes))
                    .filter(name_last.eq(name_last_bytes))
                    .first::<DepositLogRow>(conn)
                    .optional()
                    .map_err(|err| {
                        BridgeError::Runtime(format!(
                            "deposit log fetch by deposit_id failed: {err}"
                        ))
                    })
            })
            .await?;
        row.map(DepositLogEntry::try_from_row).transpose()
    }

    /// Check whether the log contains a given tx_id.
    /// This is a lightweight wrapper used by backfill logic.
    pub async fn contains_tx_id(&self, tx_id: &Tip5Hash) -> Result<bool, BridgeError> {
        Ok(self.fetch_entry(tx_id).await?.is_some())
    }

    /// Check whether the log contains a given deposit_id.
    pub async fn contains_deposit_id(&self, deposit_id: &DepositId) -> Result<bool, BridgeError> {
        Ok(self.fetch_entry_by_deposit_id(deposit_id).await?.is_some())
    }

    /// Ensure the SQLite schema exists and is configured with WAL + index.
    /// This runs on open and is idempotent.
    async fn ensure_schema(&self) -> Result<(), BridgeError> {
        self.with_conn(|conn| {
            conn.batch_execute(
                r#"
            PRAGMA journal_mode=WAL;
            PRAGMA synchronous=FULL;

            CREATE TABLE IF NOT EXISTS deposit_log (
                tx_id BLOB PRIMARY KEY NOT NULL CHECK(length(tx_id) = 40),
                block_height INTEGER NOT NULL,
                as_of BLOB NOT NULL CHECK(length(as_of) = 40),
                name_first BLOB NOT NULL CHECK(length(name_first) = 40),
                name_last BLOB NOT NULL CHECK(length(name_last) = 40),
                recipient BLOB NOT NULL CHECK(length(recipient) = 20),
                amount_to_mint INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS deposit_log_by_height ON deposit_log(block_height, tx_id);
            "#,
            )
            .map_err(|err| BridgeError::Runtime(format!("deposit log schema failed: {err}")))?;
            Ok(())
        })
        .await
    }
}

impl DepositLogEntry {
    /// Convert a DB row into a strongly-typed deposit log entry.
    /// Validates byte lengths and integer bounds.
    fn try_from_row(row: DepositLogRow) -> Result<Self, BridgeError> {
        let recipient: [u8; 20] = row
            .recipient
            .as_slice()
            .try_into()
            .map_err(|_| BridgeError::Runtime("invalid recipient length".into()))?;
        let block_height = u64::try_from(row.block_height).map_err(|err| {
            BridgeError::ValueConversion(format!("deposit log block_height overflow: {err}"))
        })?;
        let amount_to_mint = u64::try_from(row.amount_to_mint).map_err(|err| {
            BridgeError::ValueConversion(format!("deposit log amount overflow: {err}"))
        })?;
        Ok(Self {
            block_height,
            tx_id: Tip5Hash::from_be_limb_bytes(&row.tx_id)
                .map_err(|e| BridgeError::Runtime(format!("invalid tx_id bytes: {e}")))?,
            as_of: Tip5Hash::from_be_limb_bytes(&row.as_of)
                .map_err(|e| BridgeError::Runtime(format!("invalid as_of bytes: {e}")))?,
            name: Name::new(
                Tip5Hash::from_be_limb_bytes(&row.name_first)
                    .map_err(|e| BridgeError::Runtime(format!("invalid name_first bytes: {e}")))?,
                Tip5Hash::from_be_limb_bytes(&row.name_last)
                    .map_err(|e| BridgeError::Runtime(format!("invalid name_last bytes: {e}")))?,
            ),
            recipient: EthAddress(recipient),
            amount_to_mint,
        })
    }
}

impl NewDepositLogRow {
    /// Convert an in-memory entry into an insertable DB row.
    /// Performs integer range checks for SQLite.
    fn try_from_entry(entry: &DepositLogEntry) -> Result<Self, BridgeError> {
        let block_height = i64::try_from(entry.block_height).map_err(|err| {
            BridgeError::ValueConversion(format!("block_height too large for sqlite: {err}"))
        })?;
        let amount_to_mint = i64::try_from(entry.amount_to_mint).map_err(|err| {
            BridgeError::ValueConversion(format!("amount_to_mint too large for sqlite: {err}"))
        })?;
        Ok(Self {
            tx_id: entry.tx_id.to_be_limb_bytes().to_vec(),
            block_height,
            as_of: entry.as_of.to_be_limb_bytes().to_vec(),
            name_first: entry.name.first.to_be_limb_bytes().to_vec(),
            name_last: entry.name.last.to_be_limb_bytes().to_vec(),
            recipient: entry.recipient.0.to_vec(),
            amount_to_mint,
        })
    }
}

/// Compute the default on-disk location of the per-node deposit log database.
fn default_deposit_log_path() -> Result<PathBuf, BridgeError> {
    Ok(nockapp::system_data_dir()
        .join("bridge")
        .join("deposit-queue.sqlite"))
}

const SQLITE_BUSY_TIMEOUT_MS: u64 = 2_000;

#[derive(Clone, Copy, Debug)]
enum SqlitePoolMode {
    ReadOnly,
    ReadWrite,
}

fn sqlite_pool(path: &Path, mode: SqlitePoolMode) -> Result<Pool, BridgeError> {
    let path_str = path.to_string_lossy();
    let url = match mode {
        SqlitePoolMode::ReadOnly => format!("file:{path_str}?mode=ro"),
        SqlitePoolMode::ReadWrite => path_str.to_string(),
    };
    let manager = Manager::new(url, Runtime::Tokio1);
    Pool::builder(manager)
        .build()
        .map_err(|err| BridgeError::Runtime(format!("deposit log pool build failed: {err}")))
}

pub async fn sync_deposit_log_from_hashchain(
    runtime: Arc<crate::runtime::BridgeRuntimeHandle>,
    deposit_log: Arc<DepositLog>,
    nonce_epoch: &NonceEpochConfig,
    initial_delay: Duration,
) -> Result<u64, BridgeError> {
    // Allow the kernel a moment to boot before peeking (used during startup).
    if !initial_delay.is_zero() {
        tokio::time::sleep(initial_delay).await;
    }

    let max_height = deposit_log.max_block_height(nonce_epoch).await?;
    let start_tx_present = if let Some(start_tx_id) = nonce_epoch.start_tx_id.as_ref() {
        deposit_log.contains_tx_id(start_tx_id).await?
    } else {
        true
    };

    // Determine the latest nockchain height that contained a deposit.
    // This peek is expected to always return a value (0 when no deposits exist yet).
    // If it returns None, treat it as a runtime contract violation.
    let last_deposit_height = runtime
        .peek_nock_last_deposit_height()
        .await?
        .ok_or_else(|| BridgeError::Runtime("Failed to peek last deposit height".to_string()))?
        .max(nonce_epoch.start_height);

    // Determine the first block height to scan next (incremental sync).
    // - If the log is empty, scan from the epoch start height (bootstrap case).
    // - If the log has advanced past the epoch start but the start tx-id is missing,
    //   the log is inconsistent and we hard-stop.
    // - Otherwise, scan from the smaller of the chain tip and the current log max.
    let scan_start_height = if let Some(max_height) = max_height {
        if max_height >= nonce_epoch.start_height && !start_tx_present {
            return Err(BridgeError::InvalidDepositLogBase(
                "Epoch base starting tx-id not present in deposit log, even though deposit log has advanced past epoch start height".to_string(),
            ));
        } else {
            std::cmp::min(last_deposit_height, max_height)
        }
    } else {
        nonce_epoch.start_height
    };

    let all = runtime
        .peek_nock_hashchain_deposits_since_height(scan_start_height)
        .await?;

    let records: Vec<DepositLogEntry> = all
        .into_iter()
        .filter(|r| !nonce_epoch.is_before_start_key(r.block_height, &r.tx_id))
        .map(|req| DepositLogEntry {
            block_height: req.block_height,
            tx_id: req.tx_id,
            name: req.name,
            recipient: req.recipient,
            amount_to_mint: req.amount,
            as_of: req.as_of,
        })
        .collect();

    let mut inserted = 0u64;
    for record in &records {
        if matches!(
            deposit_log.insert_entry(record).await?,
            DepositLogInsertOutcome::Inserted
        ) {
            inserted += 1;
        }
    }

    info!(
        target: "bridge.deposit_log",
        inserted,
        scan_start_height,
        last_deposit_height,
        nonce_epoch_start_height = nonce_epoch.start_height,
        "deposit log sync from nock hashchain complete"
    );

    Ok(inserted)
}

pub async fn validate_deposit_log_against_chain_nonce_prefix(
    base_bridge: Arc<BaseBridge>,
    deposit_log: Arc<DepositLog>,
    nonce_epoch: NonceEpochConfig,
) -> Result<(), BridgeError> {
    let last_chain_nonce = base_bridge.get_last_deposit_nonce().await?;
    check_deposit_log_against_chain_nonce_prefix_with_last_nonce(
        last_chain_nonce, deposit_log, nonce_epoch,
    )
    .await
}

// This should be called after syncing/backfilling the deposit log so the local
// history is complete before comparing against on-chain nonce state.
async fn check_deposit_log_against_chain_nonce_prefix_with_last_nonce(
    last_chain_nonce: u64,
    deposit_log: Arc<DepositLog>,
    nonce_epoch: NonceEpochConfig,
) -> Result<(), BridgeError> {
    let first_epoch_nonce = nonce_epoch.first_epoch_nonce();

    let nonces_spent_this_epoch = compute_spent_epoch_nonces(last_chain_nonce, first_epoch_nonce);
    let next_nonce = last_chain_nonce + 1;

    // Compute log_len for progress logging. Do not fail when the log is behind.
    let snapshot = build_log_validation_snapshot(&deposit_log, &nonce_epoch, next_nonce).await?;

    // Log behind is expected when the kernel lags; the log will catch up.
    if snapshot.log_len < nonces_spent_this_epoch {
        warn!(
            target: "bridge.deposit_log",
            log_len = snapshot.log_len,
            nonces_spent_this_epoch,
            "deposit log behind chain prefix, waiting for log to catch up"
        );
    }

    if let Some(next_rec) = snapshot.next_entry {
        warn!(
            target: "bridge.deposit_log",
            next_nonce,
            tx_id=%hex::encode(next_rec.tx_id.to_be_limb_bytes()),
            block_height=next_rec.block_height,
            "next epoch nonce already exists in local deposit log (waiting for signatures)"
        );
    } else {
        info!(
            target: "bridge.deposit_log",
            next_nonce,
            nonces_spent_this_epoch,
            log_len = snapshot.log_len,
            "no deposit yet for next epoch nonce in local log"
        );
    }

    info!(
        target: "bridge.deposit_log",
        nonce_epoch_base = nonce_epoch.base,
        last_chain_nonce,
        nonces_spent_this_epoch,
        log_len = snapshot.log_len,
        "deposit log nonce validation complete"
    );

    Ok(())
}

fn compute_spent_epoch_nonces(last_chain_nonce: u64, first_epoch_nonce: u64) -> u64 {
    if last_chain_nonce < first_epoch_nonce {
        0
    } else {
        last_chain_nonce - first_epoch_nonce + 1
    }
}

// Immutable view of the log state needed for validation without holding the mutex.
#[derive(Debug, Clone)]
struct LogValidationSnapshot {
    log_len: u64,
    next_entry: Option<DepositLogEntry>,
}

async fn build_log_validation_snapshot(
    log: &DepositLog,
    nonce_epoch: &NonceEpochConfig,
    next_nonce: u64,
) -> Result<LogValidationSnapshot, BridgeError> {
    let log_len = log.number_of_deposits_in_epoch(nonce_epoch).await?;
    let next_entry = log.get_by_nonce(next_nonce, nonce_epoch).await?;

    Ok(LogValidationSnapshot {
        log_len,
        next_entry,
    })
}

pub async fn persist_commit_nock_deposits_requests(
    mut requests: Vec<crate::types::NockDepositRequestKernelData>,
    deposit_log: &DepositLog,
    nonce_epoch: &NonceEpochConfig,
) -> Result<u64, BridgeError> {
    requests.retain(|req| {
        if nonce_epoch.is_before_start_key(req.block_height, &req.tx_id) {
            tracing::warn!(
                target: "bridge.propose",
                block_height=req.block_height,
                nonce_epoch_start_height = nonce_epoch.start_height,
                "deposit block is before nonce epoch start key, skipping CDC insert"
            );
            return false;
        }
        true
    });
    requests.sort_by(|a, b| {
        let height_cmp = a.block_height.cmp(&b.block_height);
        if height_cmp != std::cmp::Ordering::Equal {
            return height_cmp;
        }
        a.tx_id.to_be_limb_bytes().cmp(&b.tx_id.to_be_limb_bytes())
    });

    let mut inserted = 0u64;
    for req in requests {
        let entry = DepositLogEntry {
            block_height: req.block_height,
            tx_id: req.tx_id,
            name: req.name,
            recipient: req.recipient,
            amount_to_mint: req.amount,
            as_of: req.as_of,
        };
        if matches!(
            deposit_log.insert_entry(&entry).await?,
            DepositLogInsertOutcome::Inserted
        ) {
            inserted += 1;
        }
    }

    Ok(inserted)
}

/// CDC driver for commit-nock-deposits effects.
/// Persists effect payloads to the deposit log, no signing or gossip.
pub fn create_commit_nock_deposits_driver(
    runtime: Arc<crate::runtime::BridgeRuntimeHandle>,
    stop_controller: crate::stop::StopController,
    bridge_status: BridgeStatus,
    tui_status: Option<TuiStatus>,
    stop: crate::stop::StopHandle,
    deposit_log: Arc<DepositLog>,
    nonce_epoch: NonceEpochConfig,
) -> nockapp::driver::IODriverFn {
    use nockapp::driver::{make_driver, NockAppHandle};
    use noun_serde::NounDecode;
    use tracing::{error, info, warn};

    use crate::stop::trigger_local_stop;
    use crate::types::{BridgeEffect, BridgeEffectVariant};

    make_driver(move |handle: NockAppHandle| {
        let runtime = runtime.clone();
        let stop_controller = stop_controller.clone();
        let bridge_status = bridge_status.clone();
        let tui_status = tui_status.clone();
        let stop = stop.clone();
        let deposit_log = deposit_log.clone();
        let nonce_epoch = nonce_epoch;

        async move {
            loop {
                if stop.is_stopped() {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
                let effect = match handle.next_effect().await {
                    Ok(effect) => effect,
                    Err(_) => continue,
                };

                let root = unsafe { effect.root() };
                let bridge_effect = match BridgeEffect::from_noun(root) {
                    Ok(effect) => effect,
                    Err(err) => {
                        warn!("Failed to decode effect: {}", err);
                        continue;
                    }
                };

                if let BridgeEffectVariant::CommitNockDeposits(requests) = bridge_effect.variant {
                    info!(
                        target: "bridge.propose",
                        request_count=requests.len(),
                        "processing commit-nock-deposits effect (CDC mode)"
                    );
                    let inserted = match persist_commit_nock_deposits_requests(
                        requests,
                        deposit_log.as_ref(),
                        &nonce_epoch,
                    )
                    .await
                    {
                        Ok(inserted) => inserted,
                        Err(err) => {
                            error!(
                                target: "bridge.propose",
                                error=%err,
                                "deposit log persistence failed, triggering local stop"
                            );
                            let reason = format!(
                                "deposit log conflict while persisting commit-nock-deposits: {err}"
                            );
                            trigger_local_stop(
                                runtime.clone(),
                                stop_controller.clone(),
                                bridge_status.clone(),
                                reason,
                            )
                            .await;
                            continue;
                        }
                    };
                    info!(
                        target: "bridge.propose",
                        inserted,
                        "persisted commit-nock-deposits entries to deposit log"
                    );
                    if inserted > 0 {
                        if let Some(ref status) = tui_status {
                            status.notify_deposit_log_refresh();
                        }
                    }
                }
            }
        }
    })
}

pub async fn run_deposit_log_tui_poller(
    deposit_log_path: PathBuf,
    nonce_epoch: NonceEpochConfig,
    bridge_status: TuiStatus,
    stop: StopHandle,
) {
    use tokio::time::{interval, MissedTickBehavior};
    use tracing::warn;

    const POLL_INTERVAL: Duration = Duration::from_secs(30);
    const REFRESH_INTERVAL: Duration = Duration::from_secs(10);

    let pool = match sqlite_pool(&deposit_log_path, SqlitePoolMode::ReadOnly) {
        Ok(pool) => pool,
        Err(err) => {
            warn!(
                target: "bridge.tui.deposit_log",
                error=%err,
                "failed to create read-only deposit log pool"
            );
            return;
        }
    };

    let mut ticker = interval(POLL_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let notifier = bridge_status.deposit_log_notifier();
    let mut last_refresh = std::time::Instant::now()
        .checked_sub(REFRESH_INTERVAL)
        .unwrap_or_else(std::time::Instant::now);
    let mut last_view = DepositLogView::default();

    loop {
        let mut force_refresh = false;
        tokio::select! {
            _ = ticker.tick() => {}
            _ = notifier.notified() => {
                force_refresh = true;
            }
        }
        if stop.is_stopped() {
            continue;
        }
        if !bridge_status.deposit_log_active() {
            continue;
        }

        let view = bridge_status.deposit_log_view();
        if view.limit == 0 {
            continue;
        }
        let view_changed = view != last_view;
        if !force_refresh && !view_changed && last_refresh.elapsed() < REFRESH_INTERVAL {
            continue;
        }

        let conn = match pool.get().await {
            Ok(conn) => conn,
            Err(err) => {
                warn!(
                    target: "bridge.tui.deposit_log",
                    error=%err,
                    "failed to fetch deposit log connection from pool"
                );
                continue;
            }
        };

        let nonce_epoch = nonce_epoch.clone();
        let snapshot = match conn
            .interact(move |conn| {
                conn.batch_execute(&format!(
                    "PRAGMA query_only = ON; PRAGMA busy_timeout = {};",
                    SQLITE_BUSY_TIMEOUT_MS
                ))
                .map_err(|err| {
                    BridgeError::Runtime(format!(
                        "failed to configure read-only deposit log connection: {err}"
                    ))
                })?;
                fetch_deposit_log_snapshot(conn, &nonce_epoch, view)
            })
            .await
        {
            Ok(Ok(snapshot)) => snapshot,
            Ok(Err(err)) => {
                warn!(
                    target: "bridge.tui.deposit_log",
                    error=%err,
                    "failed to load deposit log snapshot"
                );
                continue;
            }
            Err(err) => {
                warn!(
                    target: "bridge.tui.deposit_log",
                    error=%err,
                    "failed to run deposit log snapshot query"
                );
                continue;
            }
        };

        bridge_status.update_deposit_log_snapshot(snapshot);
        last_refresh = std::time::Instant::now();
        last_view = view;
    }
}

pub(crate) fn fetch_deposit_log_snapshot(
    conn: &mut SqliteConnection,
    nonce_epoch: &NonceEpochConfig,
    view: DepositLogView,
) -> Result<DepositLogSnapshot, BridgeError> {
    let started = Instant::now();
    let metrics = metrics::init_metrics();
    metrics
        .tui_deposit_log_limit_requested
        .swap(view.limit as f64);
    metrics
        .tui_deposit_log_offset_requested
        .swap(view.offset as f64);
    if view.limit > DEPOSIT_LOG_PAGE_SIZE {
        metrics.tui_deposit_log_limit_over_cache.increment();
    }
    if view.limit > 10_000 {
        metrics.tui_deposit_log_limit_over_10000.increment();
    }
    use crate::schema::deposit_log::dsl::{
        amount_to_mint, block_height, deposit_log as deposit_log_table, recipient, tx_id,
    };

    let epoch_start = i64::try_from(nonce_epoch.start_height).map_err(|err| {
        BridgeError::ValueConversion(format!(
            "nonce_epoch_start_height too large for sqlite: {err}"
        ))
    })?;

    let start_tx_id_bytes = nonce_epoch
        .start_tx_id
        .as_ref()
        .map(Tip5Hash::to_be_limb_bytes)
        .map(Vec::from);

    let mut count_query = deposit_log_table.into_boxed();
    if let Some(start_tx_id_bytes) = start_tx_id_bytes.as_ref() {
        let start_tx_id_hex = hex::encode(start_tx_id_bytes);
        let start_filter = diesel::dsl::sql::<diesel::sql_types::Bool>(&format!(
            "(block_height, tx_id) >= ({epoch_start}, X'{start_tx_id_hex}')"
        ));
        count_query = count_query.filter(start_filter);
    } else {
        count_query = count_query.filter(block_height.ge(epoch_start));
    }

    let count_started = Instant::now();
    let total_count: i64 = count_query.count().get_result(conn).map_err(|err| {
        metrics
            .deposit_log_count_time
            .add_timing(&count_started.elapsed());
        BridgeError::Runtime(format!("deposit log count failed: {err}"))
    })?;
    metrics
        .deposit_log_count_time
        .add_timing(&count_started.elapsed());
    let total_count_u64 = u64::try_from(total_count).map_err(|err| {
        BridgeError::ValueConversion(format!("deposit log count overflow: {err}"))
    })?;
    let first_epoch_nonce = nonce_epoch.first_epoch_nonce();

    let result = if total_count_u64 == 0 {
        metrics.tui_deposit_log_rows_returned.swap(0.0);
        Ok(DepositLogSnapshot {
            total_count: 0,
            first_epoch_nonce,
            rows: Vec::new(),
        })
    } else {
        let mut rows_query = deposit_log_table.into_boxed();
        if let Some(start_tx_id_bytes) = start_tx_id_bytes.as_ref() {
            let start_tx_id_hex = hex::encode(start_tx_id_bytes);
            let start_filter = diesel::dsl::sql::<diesel::sql_types::Bool>(&format!(
                "(block_height, tx_id) >= ({epoch_start}, X'{start_tx_id_hex}')"
            ));
            rows_query = rows_query.filter(start_filter);
        } else {
            rows_query = rows_query.filter(block_height.ge(epoch_start));
        }

        let limit_i64 = i64::try_from(view.limit).unwrap_or(i64::MAX);
        let offset_i64 = i64::try_from(view.offset).unwrap_or(i64::MAX);
        let page_started = Instant::now();
        let rows: Vec<(Vec<u8>, i64, Vec<u8>, i64)> = rows_query
            .select((tx_id, block_height, recipient, amount_to_mint))
            .order((block_height.desc(), tx_id.desc()))
            .offset(offset_i64)
            .limit(limit_i64)
            .load(conn)
            .map_err(|err| {
                metrics
                    .deposit_log_page_time
                    .add_timing(&page_started.elapsed());
                BridgeError::Runtime(format!("deposit log query failed: {err}"))
            })?;
        metrics
            .deposit_log_page_time
            .add_timing(&page_started.elapsed());
        metrics
            .tui_deposit_log_rows_returned
            .swap(rows.len() as f64);

        let mut out = Vec::with_capacity(rows.len());
        for (idx, (tx_id_bytes, height, recipient_bytes, amount)) in rows.into_iter().enumerate() {
            let block_height_u64 = u64::try_from(height).map_err(|err| {
                BridgeError::ValueConversion(format!("deposit log block_height overflow: {err}"))
            })?;
            let amount_u64 = u64::try_from(amount).map_err(|err| {
                BridgeError::ValueConversion(format!("deposit log amount overflow: {err}"))
            })?;
            let offset_u64 = u64::try_from(view.offset).unwrap_or(u64::MAX);
            let descending_index = offset_u64.saturating_add(idx as u64);
            let ascending_index = total_count_u64
                .saturating_sub(1)
                .saturating_sub(descending_index);
            let nonce = first_epoch_nonce.saturating_add(ascending_index);

            let tx_hash = Tip5Hash::from_be_limb_bytes(&tx_id_bytes)
                .map_err(|err| BridgeError::Runtime(format!("invalid tx_id bytes: {err}")))?;
            out.push(crate::tui::types::DepositLogRow {
                nonce,
                block_height: block_height_u64,
                tx_id_base58: tx_hash.to_base58(),
                recipient_hex: format!("0x{}", hex::encode(recipient_bytes)),
                amount: amount_u64,
            });
        }

        Ok(DepositLogSnapshot {
            total_count: total_count_u64,
            first_epoch_nonce,
            rows: out,
        })
    };

    metrics
        .deposit_log_snapshot_time
        .add_timing(&started.elapsed());

    result
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nockchain_math::belt::Belt;
    use tempfile::TempDir;

    use super::*;

    fn tip5(a: u64, b: u64, c: u64, d: u64, e: u64) -> Tip5Hash {
        // Helper for creating deterministic Tip5 hashes in tests.
        Tip5Hash([Belt(a), Belt(b), Belt(c), Belt(d), Belt(e)])
    }

    fn addr(byte: u8) -> EthAddress {
        // Helper for creating deterministic 20-byte addresses in tests.
        EthAddress([byte; 20])
    }

    #[tokio::test]
    async fn insert_is_idempotent_by_tx_id() {
        // Ensure duplicate inserts on tx_id are treated as no-ops.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("deposit-log.sqlite");
        let log = DepositLog::open(path).await.unwrap();
        let epoch = NonceEpochConfig {
            base: 100,
            start_height: 10,
            start_tx_id: None,
        };
        let entry = DepositLogEntry {
            block_height: 10,
            tx_id: tip5(1, 2, 3, 4, 5),
            as_of: tip5(9, 9, 9, 9, 9),
            name: Name::new(tip5(10, 0, 0, 0, 0), tip5(11, 0, 0, 0, 0)),
            recipient: addr(0x11),
            amount_to_mint: 123,
        };

        let out1 = log.insert_entry(&entry).await.unwrap();
        let out2 = log.insert_entry(&entry).await.unwrap();
        assert_eq!(out1, DepositLogInsertOutcome::Inserted);
        assert_eq!(out2, DepositLogInsertOutcome::ExistingMatch);
        assert_eq!(log.compute_nonce(&entry, &epoch).await.unwrap(), Some(101));
    }

    #[tokio::test]
    async fn insert_mismatch_is_fatal() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("deposit-log.sqlite");
        let log = DepositLog::open(path).await.unwrap();
        let entry = DepositLogEntry {
            block_height: 10,
            tx_id: tip5(1, 2, 3, 4, 5),
            as_of: tip5(9, 9, 9, 9, 9),
            name: Name::new(tip5(10, 0, 0, 0, 0), tip5(11, 0, 0, 0, 0)),
            recipient: addr(0x11),
            amount_to_mint: 123,
        };

        log.insert_entry(&entry).await.unwrap();

        let mut conflicting = entry.clone();
        conflicting.amount_to_mint = 999;

        let err = log.insert_entry(&conflicting).await.unwrap_err();
        assert!(matches!(err, BridgeError::InvalidDepositLogEntry(_)));
    }

    #[tokio::test]
    async fn compute_nonce_orders_by_height_then_tx_id() {
        // Verify nonce ordering matches (block_height, tx_id) ascending.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("deposit-log.sqlite");
        let log = DepositLog::open(path).await.unwrap();
        let epoch = NonceEpochConfig {
            base: 100,
            start_height: 10,
            start_tx_id: None,
        };

        let entry_a = DepositLogEntry {
            block_height: 10,
            tx_id: tip5(1, 0, 0, 0, 0),
            as_of: tip5(9, 9, 9, 9, 9),
            name: Name::new(tip5(10, 0, 0, 0, 0), tip5(11, 0, 0, 0, 0)),
            recipient: addr(0x11),
            amount_to_mint: 1,
        };
        let entry_b = DepositLogEntry {
            block_height: 11,
            tx_id: tip5(1, 0, 0, 0, 1),
            as_of: tip5(8, 8, 8, 8, 8),
            name: Name::new(tip5(12, 0, 0, 0, 0), tip5(13, 0, 0, 0, 0)),
            recipient: addr(0x22),
            amount_to_mint: 2,
        };
        let entry_c = DepositLogEntry {
            block_height: 11,
            tx_id: tip5(2, 0, 0, 0, 0),
            as_of: tip5(7, 7, 7, 7, 7),
            name: Name::new(tip5(14, 0, 0, 0, 0), tip5(15, 0, 0, 0, 0)),
            recipient: addr(0x33),
            amount_to_mint: 3,
        };

        log.insert_entry(&entry_a).await.unwrap();
        log.insert_entry(&entry_b).await.unwrap();
        log.insert_entry(&entry_c).await.unwrap();

        assert_eq!(
            log.compute_nonce(&entry_a, &epoch).await.unwrap(),
            Some(101)
        );
        assert_eq!(
            log.compute_nonce(&entry_b, &epoch).await.unwrap(),
            Some(102)
        );
        assert_eq!(
            log.compute_nonce(&entry_c, &epoch).await.unwrap(),
            Some(103)
        );
    }

    #[tokio::test]
    async fn anchor_nonce_mapping_and_records() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("deposit-log.sqlite");
        let log = DepositLog::open(path).await.unwrap();

        let anchor_tx_id = tip5(2, 0, 0, 0, 0);
        let entry_anchor = DepositLogEntry {
            block_height: 10,
            tx_id: anchor_tx_id.clone(),
            as_of: tip5(9, 9, 9, 9, 9),
            name: Name::new(tip5(10, 0, 0, 0, 0), tip5(11, 0, 0, 0, 0)),
            recipient: addr(0x11),
            amount_to_mint: 1,
        };
        let entry_next = DepositLogEntry {
            block_height: 10,
            tx_id: tip5(3, 0, 0, 0, 0),
            as_of: tip5(8, 8, 8, 8, 8),
            name: Name::new(tip5(12, 0, 0, 0, 0), tip5(13, 0, 0, 0, 0)),
            recipient: addr(0x22),
            amount_to_mint: 2,
        };

        log.insert_entry(&entry_anchor).await.unwrap();
        log.insert_entry(&entry_next).await.unwrap();

        let epoch = NonceEpochConfig {
            base: 50,
            start_height: 10,
            start_tx_id: Some(anchor_tx_id),
        };

        assert_eq!(
            log.compute_nonce(&entry_anchor, &epoch).await.unwrap(),
            Some(50)
        );
        assert_eq!(
            log.compute_nonce(&entry_next, &epoch).await.unwrap(),
            Some(51)
        );

        let by_base = log
            .get_by_nonce(50, &epoch)
            .await
            .unwrap()
            .expect("anchor exists");
        let by_next = log
            .get_by_nonce(51, &epoch)
            .await
            .unwrap()
            .expect("next exists");
        assert_eq!(by_base.tx_id, entry_anchor.tx_id);
        assert_eq!(by_next.tx_id, entry_next.tx_id);

        let rows = log.records_from_nonce(50, 2, &epoch).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, 50);
        assert_eq!(rows[0].1.tx_id, entry_anchor.tx_id);
        assert_eq!(rows[1].0, 51);
        assert_eq!(rows[1].1.tx_id, entry_next.tx_id);
    }

    #[tokio::test]
    async fn start_key_filters_pre_epoch_entries() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("deposit-log.sqlite");
        let log = DepositLog::open(path).await.unwrap();

        let pre_epoch = DepositLogEntry {
            block_height: 10,
            tx_id: tip5(1, 0, 0, 0, 0),
            as_of: tip5(7, 7, 7, 7, 7),
            name: Name::new(tip5(10, 0, 0, 0, 0), tip5(11, 0, 0, 0, 0)),
            recipient: addr(0x10),
            amount_to_mint: 1,
        };
        let anchor_tx_id = tip5(2, 0, 0, 0, 0);
        let anchor = DepositLogEntry {
            block_height: 10,
            tx_id: anchor_tx_id.clone(),
            as_of: tip5(8, 8, 8, 8, 8),
            name: Name::new(tip5(12, 0, 0, 0, 0), tip5(13, 0, 0, 0, 0)),
            recipient: addr(0x11),
            amount_to_mint: 2,
        };
        let next = DepositLogEntry {
            block_height: 11,
            tx_id: tip5(3, 0, 0, 0, 0),
            as_of: tip5(9, 9, 9, 9, 9),
            name: Name::new(tip5(14, 0, 0, 0, 0), tip5(15, 0, 0, 0, 0)),
            recipient: addr(0x12),
            amount_to_mint: 3,
        };

        log.insert_entry(&pre_epoch).await.unwrap();
        log.insert_entry(&anchor).await.unwrap();
        log.insert_entry(&next).await.unwrap();

        let epoch = NonceEpochConfig {
            base: 50,
            start_height: 10,
            start_tx_id: Some(anchor_tx_id),
        };

        assert_eq!(log.number_of_deposits_in_epoch(&epoch).await.unwrap(), 2);
        assert_eq!(log.max_block_height(&epoch).await.unwrap(), Some(11));

        let rows = log.records_from_nonce(50, 10, &epoch).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].1.tx_id, anchor.tx_id);
        assert_eq!(rows[1].1.tx_id, next.tx_id);
    }

    #[tokio::test]
    async fn records_from_nonce_below_epoch_returns_empty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("deposit-log.sqlite");
        let log = DepositLog::open(path).await.unwrap();

        let anchor_tx_id = tip5(2, 0, 0, 0, 0);
        let anchor = DepositLogEntry {
            block_height: 10,
            tx_id: anchor_tx_id.clone(),
            as_of: tip5(8, 8, 8, 8, 8),
            name: Name::new(tip5(12, 0, 0, 0, 0), tip5(13, 0, 0, 0, 0)),
            recipient: addr(0x11),
            amount_to_mint: 2,
        };
        log.insert_entry(&anchor).await.unwrap();

        let epoch = NonceEpochConfig {
            base: 50,
            start_height: 10,
            start_tx_id: Some(anchor_tx_id),
        };

        let rows = log.records_from_nonce(49, 2, &epoch).await.unwrap();
        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn snapshot_applies_offset_and_limit() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("deposit-log.sqlite");
        let log = DepositLog::open(path).await.unwrap();
        let epoch = NonceEpochConfig {
            base: 100,
            start_height: 10,
            start_tx_id: None,
        };

        let entry_a = DepositLogEntry {
            block_height: 10,
            tx_id: tip5(1, 0, 0, 0, 0),
            as_of: tip5(9, 9, 9, 9, 9),
            name: Name::new(tip5(10, 0, 0, 0, 0), tip5(11, 0, 0, 0, 0)),
            recipient: addr(0x11),
            amount_to_mint: 1,
        };
        let entry_b = DepositLogEntry {
            block_height: 11,
            tx_id: tip5(2, 0, 0, 0, 0),
            as_of: tip5(8, 8, 8, 8, 8),
            name: Name::new(tip5(12, 0, 0, 0, 0), tip5(13, 0, 0, 0, 0)),
            recipient: addr(0x22),
            amount_to_mint: 2,
        };
        let entry_c = DepositLogEntry {
            block_height: 12,
            tx_id: tip5(3, 0, 0, 0, 0),
            as_of: tip5(7, 7, 7, 7, 7),
            name: Name::new(tip5(14, 0, 0, 0, 0), tip5(15, 0, 0, 0, 0)),
            recipient: addr(0x33),
            amount_to_mint: 3,
        };

        log.insert_entry(&entry_a).await.unwrap();
        log.insert_entry(&entry_b).await.unwrap();
        log.insert_entry(&entry_c).await.unwrap();

        let snapshot = log
            .snapshot(
                &epoch,
                DepositLogView {
                    offset: 0,
                    limit: 2,
                },
            )
            .await
            .unwrap();
        assert_eq!(snapshot.total_count, 3);
        assert_eq!(snapshot.first_epoch_nonce, 101);
        assert_eq!(snapshot.rows.len(), 2);
        assert_eq!(snapshot.rows[0].nonce, 103);
        assert_eq!(snapshot.rows[0].block_height, 12);
        assert_eq!(snapshot.rows[0].tx_id_base58, entry_c.tx_id.to_base58());
        assert_eq!(snapshot.rows[1].nonce, 102);
        assert_eq!(snapshot.rows[1].block_height, 11);
        assert_eq!(snapshot.rows[1].tx_id_base58, entry_b.tx_id.to_base58());

        let snapshot_offset = log
            .snapshot(
                &epoch,
                DepositLogView {
                    offset: 1,
                    limit: 1,
                },
            )
            .await
            .unwrap();
        assert_eq!(snapshot_offset.total_count, 3);
        assert_eq!(snapshot_offset.rows.len(), 1);
        assert_eq!(snapshot_offset.rows[0].nonce, 102);
        assert_eq!(snapshot_offset.rows[0].block_height, 11);
        assert_eq!(
            snapshot_offset.rows[0].tx_id_base58,
            entry_b.tx_id.to_base58()
        );
    }

    #[tokio::test]
    async fn validation_succeeds_when_log_is_behind_chain_nonce() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("deposit-log.sqlite");
        let log = DepositLog::open(path).await.unwrap();

        let entry = DepositLogEntry {
            block_height: 10,
            tx_id: tip5(1, 0, 0, 0, 0),
            as_of: tip5(9, 9, 9, 9, 9),
            name: Name::new(tip5(10, 0, 0, 0, 0), tip5(11, 0, 0, 0, 0)),
            recipient: addr(0x11),
            amount_to_mint: 1,
        };
        log.insert_entry(&entry).await.unwrap();

        let epoch = NonceEpochConfig {
            base: 0,
            start_height: 10,
            start_tx_id: None,
        };

        let deposit_log = Arc::new(log);
        let result =
            check_deposit_log_against_chain_nonce_prefix_with_last_nonce(5, deposit_log, epoch)
                .await;
        assert!(result.is_ok());
    }
}
