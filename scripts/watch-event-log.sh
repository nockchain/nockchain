#!/usr/bin/env bash
set -euo pipefail

DB="${1:-.data.nockchain-sync-fsync-on/event-log.sqlite3}"

if [ ! -f "$DB" ]; then
  echo "Event log not found: $DB"
  exit 1
fi

Q=$(cat <<'SQL'
.mode column
.headers on

SELECT '=== Event Summary ===' AS '';
SELECT
  count(*) AS total_events,
  min(event_num) AS first_event,
  max(event_num) AS latest_event
FROM events;

SELECT '=== Recent Events ===' AS '';
SELECT
  event_num,
  wire_source,
  printf('%.1f ms', event_processing_duration_us / 1000.0) AS duration,
  datetime(created_at_ms / 1000, 'unixepoch', 'localtime') AS created_at
FROM events
ORDER BY event_num DESC
LIMIT 10;

SELECT '=== Snapshots ===' AS '';
SELECT
  snapshot_id,
  kind,
  state,
  event_num,
  datetime(created_at_ms / 1000, 'unixepoch', 'localtime') AS created_at,
  timestamp_tag
FROM snapshots
ORDER BY snapshot_id DESC
LIMIT 10;
SQL
)

echo "$Q" | sqlite3 -readonly "$DB"
