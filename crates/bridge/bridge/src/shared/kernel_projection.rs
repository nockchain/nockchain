use std::time::{SystemTime, UNIX_EPOCH};

use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use diesel::OptionalExtension;

use crate::shared::errors::BridgeError;

pub const KERNEL_PROJECTION_CURSOR_ID: &str = "bridge_kernel";
pub const KERNEL_PROJECTION_SCHEMA_VERSION: u64 = 1;

diesel::table! {
    kernel_projection_cursor (id) {
        id -> Text,
        base_next_height -> BigInt,
        base_tip_hash -> Nullable<Text>,
        nock_next_height -> BigInt,
        nock_tip_hash -> Nullable<Text>,
        schema_version -> BigInt,
        updated_at -> BigInt,
    }
}

/// Kernel-wide cursor position for SQLite projections derived from bridge
/// kernel Base/Nock hashchain state.
///
/// Today deposit and withdrawal projections live in separate SQLite files, so
/// each DB gets the same table shape. The cursor still represents the whole
/// kernel position for that projection DB; moving every kernel-derived
/// projection into one DB later would let us make the "cursor advances with
/// all projection writes" invariant physically atomic across deposits and
/// withdrawals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelProjectionCursor {
    pub base_next_height: u64,
    pub base_tip_hash: Option<String>,
    pub nock_next_height: u64,
    pub nock_tip_hash: Option<String>,
    pub schema_version: u64,
    pub updated_at: i64,
}

/// Current kernel position observed from peeks at boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelProjectionPosition {
    pub base_next_height: u64,
    pub base_tip_hash: Option<String>,
    pub nock_next_height: u64,
    pub nock_tip_hash: Option<String>,
}

/// The boot decision after checking cursor presence, local rows, and current
/// kernel position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelProjectionBootPlan {
    UseExisting(KernelProjectionCursor),
    Initialize(KernelProjectionCursor),
}

#[derive(Queryable)]
struct KernelProjectionCursorRow {
    id: String,
    base_next_height: i64,
    base_tip_hash: Option<String>,
    nock_next_height: i64,
    nock_tip_hash: Option<String>,
    schema_version: i64,
    updated_at: i64,
}

#[derive(Insertable)]
#[diesel(table_name = kernel_projection_cursor)]
struct NewKernelProjectionCursorRow {
    id: String,
    base_next_height: i64,
    base_tip_hash: Option<String>,
    nock_next_height: i64,
    nock_tip_hash: Option<String>,
    schema_version: i64,
    updated_at: i64,
}

/// Creates the shared kernel projection cursor table in the supplied SQLite
/// database.
pub fn ensure_kernel_projection_cursor_schema(
    conn: &mut SqliteConnection,
) -> Result<(), BridgeError> {
    conn.batch_execute(
        r#"
        CREATE TABLE IF NOT EXISTS kernel_projection_cursor (
            id TEXT PRIMARY KEY NOT NULL,
            base_next_height INTEGER NOT NULL CHECK(base_next_height >= 0),
            base_tip_hash TEXT NULL,
            nock_next_height INTEGER NOT NULL CHECK(nock_next_height >= 0),
            nock_tip_hash TEXT NULL,
            schema_version INTEGER NOT NULL CHECK(schema_version >= 0),
            updated_at INTEGER NOT NULL
        );
        "#,
    )
    .map_err(|err| BridgeError::Runtime(format!("kernel projection cursor schema failed: {err}")))
}

/// Loads the bridge-kernel cursor row, if this projection DB has been
/// initialized.
pub fn load_kernel_projection_cursor(
    conn: &mut SqliteConnection,
) -> Result<Option<KernelProjectionCursor>, BridgeError> {
    kernel_projection_cursor::table
        .filter(kernel_projection_cursor::id.eq(KERNEL_PROJECTION_CURSOR_ID))
        .first::<KernelProjectionCursorRow>(conn)
        .optional()
        .map_err(|err| {
            BridgeError::Runtime(format!("kernel projection cursor load failed: {err}"))
        })?
        .map(KernelProjectionCursor::try_from_row)
        .transpose()
}

/// Upserts the bridge-kernel cursor row. Callers should run this in the same
/// SQLite transaction as the projection writes covered by the cursor.
pub fn upsert_kernel_projection_cursor(
    conn: &mut SqliteConnection,
    cursor: &KernelProjectionCursor,
) -> Result<(), BridgeError> {
    let row = NewKernelProjectionCursorRow::try_from_cursor(cursor)?;

    diesel::insert_into(kernel_projection_cursor::table)
        .values(&row)
        .on_conflict(kernel_projection_cursor::id)
        .do_update()
        .set((
            kernel_projection_cursor::base_next_height.eq(row.base_next_height),
            kernel_projection_cursor::base_tip_hash.eq(row.base_tip_hash.clone()),
            kernel_projection_cursor::nock_next_height.eq(row.nock_next_height),
            kernel_projection_cursor::nock_tip_hash.eq(row.nock_tip_hash.clone()),
            kernel_projection_cursor::schema_version.eq(row.schema_version),
            kernel_projection_cursor::updated_at.eq(row.updated_at),
        ))
        .execute(conn)
        .map_err(|err| {
            BridgeError::Runtime(format!("kernel projection cursor update failed: {err}"))
        })?;
    Ok(())
}

/// Plans boot behavior for a projection DB from an explicit initialization
/// position.
pub fn plan_kernel_projection_boot(
    cursor: Option<KernelProjectionCursor>,
    has_kernel_projection_rows: bool,
    current_position: &KernelProjectionPosition,
    initial_position: KernelProjectionPosition,
) -> Result<KernelProjectionBootPlan, BridgeError> {
    if let Some(cursor) = cursor {
        ensure_cursor_not_ahead(&cursor, current_position)?;
        return Ok(KernelProjectionBootPlan::UseExisting(cursor));
    }

    if has_kernel_projection_rows {
        return Err(BridgeError::Runtime(
            "kernel projection cursor is missing but kernel-derived projection rows exist".into(),
        ));
    }

    let cursor = KernelProjectionCursor::from_position(initial_position);
    ensure_cursor_not_ahead(&cursor, current_position)?;

    Ok(KernelProjectionBootPlan::Initialize(cursor))
}

impl KernelProjectionCursor {
    pub fn from_position(position: KernelProjectionPosition) -> Self {
        Self {
            base_next_height: position.base_next_height,
            base_tip_hash: position.base_tip_hash,
            nock_next_height: position.nock_next_height,
            nock_tip_hash: position.nock_tip_hash,
            schema_version: KERNEL_PROJECTION_SCHEMA_VERSION,
            updated_at: current_unix_timestamp_secs(),
        }
    }

    fn try_from_row(row: KernelProjectionCursorRow) -> Result<Self, BridgeError> {
        if row.id != KERNEL_PROJECTION_CURSOR_ID {
            return Err(BridgeError::Runtime(format!(
                "unexpected kernel projection cursor id: {}",
                row.id
            )));
        }
        Ok(Self {
            base_next_height: i64_to_u64(row.base_next_height, "base_next_height")?,
            base_tip_hash: row.base_tip_hash,
            nock_next_height: i64_to_u64(row.nock_next_height, "nock_next_height")?,
            nock_tip_hash: row.nock_tip_hash,
            schema_version: i64_to_u64(row.schema_version, "schema_version")?,
            updated_at: row.updated_at,
        })
    }
}

impl NewKernelProjectionCursorRow {
    fn try_from_cursor(cursor: &KernelProjectionCursor) -> Result<Self, BridgeError> {
        Ok(Self {
            id: KERNEL_PROJECTION_CURSOR_ID.to_string(),
            base_next_height: u64_to_i64(cursor.base_next_height, "base_next_height")?,
            base_tip_hash: cursor.base_tip_hash.clone(),
            nock_next_height: u64_to_i64(cursor.nock_next_height, "nock_next_height")?,
            nock_tip_hash: cursor.nock_tip_hash.clone(),
            schema_version: u64_to_i64(cursor.schema_version, "schema_version")?,
            updated_at: cursor.updated_at,
        })
    }
}

fn ensure_cursor_not_ahead(
    cursor: &KernelProjectionCursor,
    current: &KernelProjectionPosition,
) -> Result<(), BridgeError> {
    if cursor.base_next_height > current.base_next_height {
        return Err(BridgeError::Runtime(format!(
            "kernel projection cursor is ahead of kernel Base hashchain: cursor_base_next_height={} kernel_base_next_height={}",
            cursor.base_next_height, current.base_next_height
        )));
    }
    if cursor.nock_next_height > current.nock_next_height {
        return Err(BridgeError::Runtime(format!(
            "kernel projection cursor is ahead of kernel Nock hashchain: cursor_nock_next_height={} kernel_nock_next_height={}",
            cursor.nock_next_height, current.nock_next_height
        )));
    }
    Ok(())
}

fn current_unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

fn u64_to_i64(value: u64, field: &'static str) -> Result<i64, BridgeError> {
    i64::try_from(value).map_err(|err| {
        BridgeError::ValueConversion(format!("kernel projection cursor {field} overflow: {err}"))
    })
}

fn i64_to_u64(value: i64, field: &'static str) -> Result<u64, BridgeError> {
    u64::try_from(value).map_err(|err| {
        BridgeError::ValueConversion(format!("kernel projection cursor {field} invalid: {err}"))
    })
}

#[cfg(test)]
mod tests {
    use diesel::Connection;
    use tempfile::tempdir;

    use super::*;

    fn position(base_next_height: u64, nock_next_height: u64) -> KernelProjectionPosition {
        KernelProjectionPosition {
            base_next_height,
            base_tip_hash: Some(format!("base-{base_next_height}")),
            nock_next_height,
            nock_tip_hash: Some(format!("nock-{nock_next_height}")),
        }
    }

    fn open_sqlite() -> (tempfile::TempDir, SqliteConnection) {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("projection.sqlite");
        let conn =
            SqliteConnection::establish(path.to_str().expect("sqlite path")).expect("open sqlite");
        (dir, conn)
    }

    #[test]
    fn kernel_projection_cursor_schema_roundtrips() {
        let (_dir, mut conn) = open_sqlite();
        ensure_kernel_projection_cursor_schema(&mut conn).expect("ensure schema");
        assert!(load_kernel_projection_cursor(&mut conn)
            .expect("load empty cursor")
            .is_none());

        let cursor = KernelProjectionCursor::from_position(position(42, 7));
        upsert_kernel_projection_cursor(&mut conn, &cursor).expect("upsert cursor");
        assert_eq!(
            load_kernel_projection_cursor(&mut conn).expect("load cursor"),
            Some(cursor)
        );
    }

    #[test]
    fn kernel_projection_boot_rejects_missing_cursor_with_rows() {
        let err = plan_kernel_projection_boot(None, true, &position(10, 20), position(10, 20))
            .expect_err("missing cursor with rows should fail");
        assert!(
            err.to_string().contains("projection rows exist"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn kernel_projection_boot_initializes_at_explicit_position() {
        let current = position(10, 20);
        let plan =
            plan_kernel_projection_boot(None, false, &current, position(7, 12)).expect("plan boot");
        assert!(matches!(
            plan,
            KernelProjectionBootPlan::Initialize(KernelProjectionCursor {
                base_next_height: 7,
                nock_next_height: 12,
                ..
            })
        ));
    }

    #[test]
    fn kernel_projection_boot_rejects_cursor_ahead_of_kernel() {
        let cursor = KernelProjectionCursor::from_position(position(11, 20));
        let err =
            plan_kernel_projection_boot(Some(cursor), true, &position(10, 20), position(10, 20))
                .expect_err("cursor ahead should fail");
        assert!(
            err.to_string().contains("ahead of kernel Base hashchain"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn kernel_projection_boot_uses_existing_cursor_when_not_ahead() {
        let cursor = KernelProjectionCursor::from_position(position(9, 20));
        let plan = plan_kernel_projection_boot(
            Some(cursor.clone()),
            true,
            &position(10, 20),
            position(99, 99),
        )
        .expect("plan boot");
        assert_eq!(plan, KernelProjectionBootPlan::UseExisting(cursor));
    }

    #[test]
    fn kernel_projection_boot_rejects_initial_position_ahead_of_kernel() {
        let err = plan_kernel_projection_boot(None, false, &position(10, 20), position(11, 20))
            .expect_err("initial position ahead should fail");
        assert!(
            err.to_string().contains("ahead of kernel Base hashchain"),
            "unexpected error: {err}"
        );
    }
}
