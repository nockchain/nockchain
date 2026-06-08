use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use diesel::connection::SimpleConnection;
use diesel::dsl::{max, sql};
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Integer, Text};
use diesel::sqlite::SqliteConnection;
use diesel::{sql_query, OptionalExtension};
use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};
use nockvm::offset::PmaOffsetWords;
use thiserror::Error;

use crate::utils::durability;

const ACTIVE_SNAPSHOT_ID_KEY: &str = "active_snapshot_id";
const EVENT_LOG_MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

diesel::table! {
    events (id) {
        id -> BigInt,
        event_num -> BigInt,
        job_jam -> Binary,
        wire_source -> Text,
        wire_version -> BigInt,
        wire_tags_json -> Text,
        cause_hash -> Binary,
        job_hash -> Binary,
        event_processing_duration_us -> BigInt,
        created_at_ms -> BigInt,
    }
}

diesel::table! {
    snapshots (snapshot_id) {
        snapshot_id -> BigInt,
        kind -> Text,
        state -> Text,
        event_num -> BigInt,
        pma_path -> Text,
        manifest_path -> Text,
        alloc_words -> BigInt,
        kernel_root_raw -> BigInt,
        cold_offset -> BigInt,
        used_blake3 -> Binary,
        structure_blake3 -> Nullable<Binary>,
        created_at_ms -> BigInt,
        activated_at_ms -> Nullable<BigInt>,
        base_snapshot_id -> Nullable<BigInt>,
        timestamp_tag -> Text,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct EventLogConfig {
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct EventLogEntry {
    pub event_num: u64,
    pub job_jam: Vec<u8>,
    pub wire_source: String,
    pub wire_version: i64,
    pub wire_tags_json: String,
    pub cause_hash: Vec<u8>,
    pub job_hash: Vec<u8>,
    pub event_processing_duration: Duration,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct ReadySnapshotRecord {
    pub snapshot_id: i64,
    pub kind: String,
    pub event_num: u64,
    pub pma_path: String,
    pub manifest_path: String,
    pub alloc_words: u64,
    pub kernel_root_raw: u64,
    pub cold_offset: PmaOffsetWords,
    pub used_blake3: Vec<u8>,
    pub structure_blake3: Option<Vec<u8>>,
    pub created_at_ms: i64,
    pub activated_at_ms: Option<i64>,
    pub base_snapshot_id: Option<i64>,
    pub timestamp_tag: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ReplayLogEntry {
    pub event_num: u64,
    pub job_jam: Vec<u8>,
}

#[derive(Debug, Error)]
pub(crate) enum EventLogError {
    #[error("event log io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("event log sqlite connection error: {0}")]
    Connection(#[from] diesel::ConnectionError),
    #[error("event log sqlite query error: {0}")]
    Query(#[from] diesel::result::Error),
    #[error("event log path is not valid UTF-8: {0}")]
    NonUtf8Path(PathBuf),
    #[error("event log migration error: {0}")]
    Migration(String),
    #[error("invalid event number {0}")]
    InvalidEventNum(i64),
    #[error("sqlite INTEGER field {field} has out-of-range value {value}")]
    FieldOutOfRange { field: &'static str, value: i64 },
    #[error("integer field {field} out of range for sqlite INTEGER: {value}")]
    IntegerOutOfRange { field: &'static str, value: u64 },
    #[error("duration field {field} out of range for sqlite INTEGER microseconds: {micros}")]
    DurationOutOfRange { field: &'static str, micros: u128 },
    #[error("event log quick_check failed: {0}")]
    QuickCheck(String),
    #[error("event sequence gap detected: expected event_num {expected}, found {found}")]
    EventSequenceGap { expected: u64, found: u64 },
}

pub(crate) struct EventLog {
    path: PathBuf,
    conn: SqliteConnection,
}

#[derive(Insertable)]
#[diesel(table_name = events)]
struct NewEventRow<'a> {
    event_num: i64,
    job_jam: &'a [u8],
    wire_source: &'a str,
    wire_version: i64,
    wire_tags_json: &'a str,
    cause_hash: &'a [u8],
    job_hash: &'a [u8],
    event_processing_duration_us: i64,
    created_at_ms: i64,
}

impl<'a> NewEventRow<'a> {
    fn try_from_entry(event: &'a EventLogEntry) -> Result<Self, EventLogError> {
        Ok(Self {
            event_num: i64::try_from(event.event_num).map_err(|_| {
                EventLogError::InvalidEventNum(event.event_num.try_into().unwrap_or(i64::MAX))
            })?,
            job_jam: &event.job_jam,
            wire_source: &event.wire_source,
            wire_version: event.wire_version,
            wire_tags_json: &event.wire_tags_json,
            cause_hash: &event.cause_hash,
            job_hash: &event.job_hash,
            event_processing_duration_us: sqlite_duration_micros(
                "event_processing_duration_us", event.event_processing_duration,
            )?,
            created_at_ms: event.created_at_ms,
        })
    }
}

#[derive(Insertable)]
#[diesel(table_name = snapshots)]
struct NewSnapshotRow<'a> {
    kind: &'a str,
    state: &'a str,
    event_num: i64,
    pma_path: &'a str,
    manifest_path: &'a str,
    alloc_words: i64,
    kernel_root_raw: i64,
    cold_offset: i64,
    used_blake3: &'a [u8],
    structure_blake3: Option<&'a [u8]>,
    created_at_ms: i64,
    activated_at_ms: Option<i64>,
    base_snapshot_id: Option<i64>,
    timestamp_tag: &'a str,
}

impl<'a> NewSnapshotRow<'a> {
    fn try_from_record(snapshot: &'a ReadySnapshotRecord) -> Result<Self, EventLogError> {
        Ok(Self {
            kind: &snapshot.kind,
            state: "ready",
            event_num: sqlite_i64("event_num", snapshot.event_num)?,
            pma_path: &snapshot.pma_path,
            manifest_path: &snapshot.manifest_path,
            alloc_words: sqlite_i64("alloc_words", snapshot.alloc_words)?,
            kernel_root_raw: sqlite_bitcast_i64(snapshot.kernel_root_raw),
            cold_offset: i64::try_from(snapshot.cold_offset.words()).map_err(|_| {
                EventLogError::IntegerOutOfRange {
                    field: "cold_offset",
                    value: snapshot.cold_offset.words(),
                }
            })?,
            used_blake3: &snapshot.used_blake3,
            structure_blake3: snapshot.structure_blake3.as_deref(),
            created_at_ms: snapshot.created_at_ms,
            activated_at_ms: snapshot.activated_at_ms,
            base_snapshot_id: snapshot.base_snapshot_id,
            timestamp_tag: &snapshot.timestamp_tag,
        })
    }
}

#[derive(Queryable)]
struct SnapshotRow {
    snapshot_id: i64,
    kind: String,
    _state: String,
    event_num: i64,
    pma_path: String,
    manifest_path: String,
    alloc_words: i64,
    kernel_root_raw: i64,
    cold_offset: i64,
    used_blake3: Vec<u8>,
    structure_blake3: Option<Vec<u8>>,
    created_at_ms: i64,
    activated_at_ms: Option<i64>,
    base_snapshot_id: Option<i64>,
    timestamp_tag: String,
}

impl SnapshotRow {
    fn try_into_record(self) -> Result<ReadySnapshotRecord, EventLogError> {
        Ok(ReadySnapshotRecord {
            snapshot_id: self.snapshot_id,
            kind: self.kind,
            event_num: event_num_from_sqlite(self.event_num)?,
            pma_path: self.pma_path,
            manifest_path: self.manifest_path,
            alloc_words: u64_from_sqlite("alloc_words", self.alloc_words)?,
            kernel_root_raw: u64::from_ne_bytes(self.kernel_root_raw.to_ne_bytes()),
            cold_offset: PmaOffsetWords::from_words(u64_from_sqlite(
                "cold_offset", self.cold_offset,
            )?),
            used_blake3: self.used_blake3,
            structure_blake3: self.structure_blake3,
            created_at_ms: self.created_at_ms,
            activated_at_ms: self.activated_at_ms,
            base_snapshot_id: self.base_snapshot_id,
            timestamp_tag: self.timestamp_tag,
        })
    }
}

#[derive(QueryableByName)]
struct I64ValueRow {
    #[diesel(sql_type = BigInt)]
    value: i64,
}

#[derive(QueryableByName)]
struct QuickCheckRow {
    #[diesel(sql_type = Text)]
    quick_check: String,
}

impl EventLog {
    pub(crate) fn open(config: EventLogConfig) -> Result<Self, EventLogError> {
        if let Some(parent) = config.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut conn = establish_connection(&config.path)?;
        Self::configure(&mut conn)?;
        Self::migrate(&mut conn)?;
        Ok(Self {
            path: config.path,
            conn,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn append_event(&mut self, event: &EventLogEntry) -> Result<(), EventLogError> {
        let row = NewEventRow::try_from_entry(event)?;
        self.conn.immediate_transaction(|conn| {
            diesel::insert_into(events::table)
                .values(&row)
                .execute(conn)?;
            Ok(())
        })
    }

    pub(crate) fn has_ready_snapshot(&mut self) -> Result<bool, EventLogError> {
        let count = snapshots::table
            .filter(snapshots::state.eq("ready"))
            .count()
            .get_result::<i64>(&mut self.conn)?;
        Ok(count > 0)
    }

    pub(crate) fn insert_ready_snapshot(
        &mut self,
        snapshot: &ReadySnapshotRecord,
    ) -> Result<i64, EventLogError> {
        let row = NewSnapshotRow::try_from_record(snapshot)?;
        self.conn.immediate_transaction(|conn| {
            diesel::insert_into(snapshots::table)
                .values(&row)
                .execute(conn)?;
            let snapshot_id = last_insert_rowid(conn)?;
            store_meta_i64(conn, ACTIVE_SNAPSHOT_ID_KEY, snapshot_id)?;
            Ok(snapshot_id)
        })
    }

    pub(crate) fn quick_check(&mut self) -> Result<(), EventLogError> {
        let result = sql_query("SELECT quick_check FROM pragma_quick_check")
            .get_result::<QuickCheckRow>(&mut self.conn)?
            .quick_check;
        if result.eq_ignore_ascii_case("ok") {
            Ok(())
        } else {
            Err(EventLogError::QuickCheck(result))
        }
    }

    pub(crate) fn list_ready_snapshots(
        &mut self,
    ) -> Result<Vec<ReadySnapshotRecord>, EventLogError> {
        snapshots::table
            .filter(snapshots::state.eq("ready"))
            .order((
                sql::<Integer>("CASE kind WHEN 'rotating' THEN 0 ELSE 1 END").asc(),
                snapshots::timestamp_tag.desc(),
                snapshots::snapshot_id.desc(),
            ))
            .load::<SnapshotRow>(&mut self.conn)?
            .into_iter()
            .map(SnapshotRow::try_into_record)
            .collect()
    }

    pub(crate) fn latest_ready_snapshot_event_num(&mut self) -> Result<Option<u64>, EventLogError> {
        let latest_event_num = snapshots::table
            .filter(snapshots::state.eq("ready"))
            .select(max(snapshots::event_num))
            .first::<Option<i64>>(&mut self.conn)?;
        latest_event_num.map(event_num_from_sqlite).transpose()
    }

    pub(crate) fn mark_snapshot_failed(&mut self, snapshot_id: i64) -> Result<(), EventLogError> {
        diesel::update(snapshots::table.filter(snapshots::snapshot_id.eq(snapshot_id)))
            .set(snapshots::state.eq("failed"))
            .execute(&mut self.conn)?;
        self.refresh_active_snapshot_after_state_change(snapshot_id)
    }

    pub(crate) fn active_snapshot_id(&mut self) -> Result<Option<i64>, EventLogError> {
        load_meta_i64(&mut self.conn, ACTIVE_SNAPSHOT_ID_KEY)
    }

    pub(crate) fn set_active_snapshot_id(&mut self, snapshot_id: i64) -> Result<(), EventLogError> {
        store_meta_i64(&mut self.conn, ACTIVE_SNAPSHOT_ID_KEY, snapshot_id)
    }

    pub(crate) fn ready_rotating_snapshots(
        &mut self,
    ) -> Result<Vec<ReadySnapshotRecord>, EventLogError> {
        snapshots::table
            .filter(
                snapshots::state
                    .eq("ready")
                    .and(snapshots::kind.eq("rotating")),
            )
            .order((
                snapshots::timestamp_tag.desc(),
                snapshots::snapshot_id.desc(),
            ))
            .load::<SnapshotRow>(&mut self.conn)?
            .into_iter()
            .map(SnapshotRow::try_into_record)
            .collect()
    }

    pub(crate) fn retire_snapshot(&mut self, snapshot_id: i64) -> Result<(), EventLogError> {
        diesel::update(snapshots::table.filter(snapshots::snapshot_id.eq(snapshot_id)))
            .set(snapshots::state.eq("retired"))
            .execute(&mut self.conn)?;
        self.refresh_active_snapshot_after_state_change(snapshot_id)
    }

    pub(crate) fn replay_events_after(
        &mut self,
        event_num: u64,
    ) -> Result<Vec<ReplayLogEntry>, EventLogError> {
        let rows = events::table
            .select((events::event_num, events::job_jam))
            .filter(events::event_num.gt(sqlite_i64("event_num", event_num)?))
            .order(events::event_num.asc())
            .load::<(i64, Vec<u8>)>(&mut self.conn)?;
        let entries = rows
            .into_iter()
            .map(|(event_num, job_jam)| {
                Ok(ReplayLogEntry {
                    event_num: event_num_from_sqlite(event_num)?,
                    job_jam,
                })
            })
            .collect::<Result<Vec<_>, EventLogError>>()?;
        let mut expected = event_num.saturating_add(1);
        for entry in &entries {
            if entry.event_num != expected {
                return Err(EventLogError::EventSequenceGap {
                    expected,
                    found: entry.event_num,
                });
            }
            expected = expected.saturating_add(1);
        }
        Ok(entries)
    }

    pub(crate) fn event_processing_time_after(
        &mut self,
        event_num: u64,
    ) -> Result<Duration, EventLogError> {
        let total_micros = sql_query(
            "SELECT COALESCE(SUM(event_processing_duration_us), 0) AS value FROM events WHERE event_num > ?",
        )
        .bind::<BigInt, _>(sqlite_i64("event_num", event_num)?)
        .get_result::<I64ValueRow>(&mut self.conn)?
        .value;
        duration_from_sqlite("event_processing_duration_us", total_micros)
    }

    #[allow(dead_code)]
    pub(crate) fn max_event_num(&mut self) -> Result<Option<u64>, EventLogError> {
        let max_event_num = events::table
            .select(max(events::event_num))
            .first::<Option<i64>>(&mut self.conn)?;
        max_event_num.map(event_num_from_sqlite).transpose()
    }

    fn configure(conn: &mut SqliteConnection) -> Result<(), EventLogError> {
        conn.batch_execute(
            r#"
PRAGMA journal_mode = WAL;
PRAGMA temp_store = MEMORY;
PRAGMA foreign_keys = 1;
"#,
        )?;
        let sync_mode = if durability::fsync_disabled() {
            "OFF"
        } else {
            "FULL"
        };
        conn.batch_execute(&format!("PRAGMA synchronous = {sync_mode};"))?;
        Ok(())
    }

    fn migrate(conn: &mut SqliteConnection) -> Result<(), EventLogError> {
        conn.run_pending_migrations(EVENT_LOG_MIGRATIONS)
            .map(|_| ())
            .map_err(|err| EventLogError::Migration(err.to_string()))
    }

    fn refresh_active_snapshot_after_state_change(
        &mut self,
        changed_snapshot_id: i64,
    ) -> Result<(), EventLogError> {
        let active_snapshot_id = self.active_snapshot_id()?;
        if active_snapshot_id != Some(changed_snapshot_id) {
            return Ok(());
        }
        let replacement = snapshots::table
            .select(snapshots::snapshot_id)
            .filter(snapshots::state.eq("ready"))
            .order((
                sql::<Integer>("CASE kind WHEN 'rotating' THEN 0 ELSE 1 END").asc(),
                snapshots::timestamp_tag.desc(),
                snapshots::snapshot_id.desc(),
            ))
            .first::<i64>(&mut self.conn)
            .optional()?;
        if let Some(replacement) = replacement {
            self.set_active_snapshot_id(replacement)?;
        } else {
            delete_meta_key(&mut self.conn, ACTIVE_SNAPSHOT_ID_KEY)?;
        }
        Ok(())
    }
}

fn establish_connection(path: &Path) -> Result<SqliteConnection, EventLogError> {
    let path_str = path
        .to_str()
        .ok_or_else(|| EventLogError::NonUtf8Path(path.to_path_buf()))?;
    Ok(SqliteConnection::establish(path_str)?)
}

fn load_meta_i64(conn: &mut SqliteConnection, key: &str) -> Result<Option<i64>, EventLogError> {
    sql_query("SELECT CAST(value AS INTEGER) AS value FROM meta WHERE key = ?")
        .bind::<Text, _>(key)
        .get_result::<I64ValueRow>(conn)
        .optional()
        .map(|row| row.map(|row| row.value))
        .map_err(Into::into)
}

fn store_meta_i64(conn: &mut SqliteConnection, key: &str, value: i64) -> Result<(), EventLogError> {
    sql_query(
        r#"
INSERT INTO meta (key, value)
VALUES (?, ?)
ON CONFLICT(key) DO UPDATE SET value = excluded.value
"#,
    )
    .bind::<Text, _>(key)
    .bind::<BigInt, _>(value)
    .execute(conn)?;
    Ok(())
}

fn delete_meta_key(conn: &mut SqliteConnection, key: &str) -> Result<(), EventLogError> {
    sql_query("DELETE FROM meta WHERE key = ?")
        .bind::<Text, _>(key)
        .execute(conn)?;
    Ok(())
}

fn last_insert_rowid(conn: &mut SqliteConnection) -> Result<i64, EventLogError> {
    Ok(sql_query("SELECT last_insert_rowid() AS value")
        .get_result::<I64ValueRow>(conn)?
        .value)
}

fn sqlite_i64(field: &'static str, value: u64) -> Result<i64, EventLogError> {
    i64::try_from(value).map_err(|_| EventLogError::IntegerOutOfRange { field, value })
}

fn sqlite_bitcast_i64(value: u64) -> i64 {
    i64::from_ne_bytes(value.to_ne_bytes())
}

fn event_num_from_sqlite(value: i64) -> Result<u64, EventLogError> {
    u64::try_from(value).map_err(|_| EventLogError::InvalidEventNum(value))
}

fn u64_from_sqlite(field: &'static str, value: i64) -> Result<u64, EventLogError> {
    u64::try_from(value).map_err(|_| EventLogError::FieldOutOfRange { field, value })
}

fn sqlite_duration_micros(field: &'static str, value: Duration) -> Result<i64, EventLogError> {
    let micros = value.as_micros();
    if micros > i64::MAX as u128 {
        return Err(EventLogError::DurationOutOfRange { field, micros });
    }
    Ok(micros as i64)
}

fn duration_from_sqlite(field: &'static str, value: i64) -> Result<Duration, EventLogError> {
    let micros = u64_from_sqlite(field, value)?;
    Ok(Duration::from_micros(micros))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tempfile::TempDir;

    use super::*;

    fn sample_entry(event_num: u64) -> EventLogEntry {
        EventLogEntry {
            event_num,
            job_jam: vec![0, 1, 2, event_num as u8],
            wire_source: "sys".to_string(),
            wire_version: 1,
            wire_tags_json: "[]".to_string(),
            cause_hash: vec![3; 32],
            job_hash: vec![4; 32],
            event_processing_duration: Duration::from_micros(event_num),
            created_at_ms: 42,
        }
    }

    #[test]
    fn initializes_schema_and_appends_events() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("event-log.sqlite3");
        let mut log = EventLog::open(EventLogConfig { path }).expect("open event log");
        assert_eq!(log.max_event_num().expect("max event num"), None);

        log.append_event(&sample_entry(1)).expect("append event 1");
        log.append_event(&sample_entry(2)).expect("append event 2");

        assert_eq!(log.max_event_num().expect("max event num"), Some(2));
        assert_eq!(
            log.event_processing_time_after(0)
                .expect("event processing time after 0"),
            Duration::from_micros(3)
        );
    }

    #[test]
    fn inserts_ready_snapshot_rows() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("event-log.sqlite3");
        let mut log = EventLog::open(EventLogConfig { path }).expect("open event log");
        assert!(!log.has_ready_snapshot().expect("ready snapshot count"));

        log.insert_ready_snapshot(&ReadySnapshotRecord {
            snapshot_id: 0,
            kind: "epoch".to_string(),
            event_num: 7,
            pma_path: "epoch.pma".to_string(),
            manifest_path: "epoch.manifest".to_string(),
            alloc_words: 128,
            kernel_root_raw: u64::MAX,
            cold_offset: PmaOffsetWords::from_words(3),
            used_blake3: vec![5; 32],
            structure_blake3: None,
            created_at_ms: 99,
            activated_at_ms: Some(99),
            base_snapshot_id: None,
            timestamp_tag: "epoch".to_string(),
        })
        .expect("insert ready snapshot");

        assert!(log.has_ready_snapshot().expect("ready snapshot count"));
        let ready = log.list_ready_snapshots().expect("ready snapshots");
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].event_num, 7);
        assert_eq!(
            log.latest_ready_snapshot_event_num()
                .expect("latest ready snapshot event num"),
            Some(7)
        );
    }

    #[test]
    fn replay_events_detects_gaps() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("event-log.sqlite3");
        let mut log = EventLog::open(EventLogConfig { path }).expect("open event log");
        log.append_event(&sample_entry(1)).expect("append event 1");
        log.append_event(&sample_entry(3)).expect("append event 3");

        let err = log
            .replay_events_after(0)
            .expect_err("gap should be detected");
        assert!(matches!(
            err,
            EventLogError::EventSequenceGap {
                expected: 2,
                found: 3
            }
        ));
    }
}
