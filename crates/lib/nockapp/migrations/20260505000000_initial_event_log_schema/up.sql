-- IF NOT EXISTS lets pre-Diesel PMA event logs adopt Diesel migration tracking.
CREATE TABLE IF NOT EXISTS meta (
  key TEXT PRIMARY KEY,
  value BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  event_num INTEGER NOT NULL UNIQUE,
  job_jam BLOB NOT NULL,
  wire_source TEXT NOT NULL,
  wire_version INTEGER NOT NULL,
  wire_tags_json TEXT NOT NULL,
  cause_hash BLOB NOT NULL,
  job_hash BLOB NOT NULL,
  event_processing_duration_us INTEGER NOT NULL DEFAULT 0,
  created_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS events_event_num_idx ON events(event_num);

CREATE TABLE IF NOT EXISTS snapshots (
  snapshot_id INTEGER PRIMARY KEY AUTOINCREMENT,
  kind TEXT NOT NULL CHECK(kind IN ('epoch','rotating')),
  state TEXT NOT NULL CHECK(state IN ('writing','ready','failed','retired')),
  event_num INTEGER NOT NULL,
  pma_path TEXT NOT NULL,
  manifest_path TEXT NOT NULL,
  alloc_words INTEGER NOT NULL,
  kernel_root_raw INTEGER NOT NULL,
  cold_offset INTEGER NOT NULL,
  used_blake3 BLOB NOT NULL,
  structure_blake3 BLOB,
  created_at_ms INTEGER NOT NULL,
  activated_at_ms INTEGER,
  base_snapshot_id INTEGER,
  timestamp_tag TEXT NOT NULL,
  UNIQUE(kind, timestamp_tag)
);

CREATE INDEX IF NOT EXISTS snapshots_kind_ts_idx ON snapshots(kind, timestamp_tag DESC);
CREATE INDEX IF NOT EXISTS snapshots_event_idx ON snapshots(event_num DESC);
