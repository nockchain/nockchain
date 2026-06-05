#!/usr/bin/env bash
set -euo pipefail

DB="${1:-.data.nockchain-sync-fsync-on/event-log.sqlite3}"

if [ ! -f "$DB" ]; then
  echo "Event log not found: $DB"
  exit 1
fi

cat <<'SQL' | sqlite3 -readonly "$DB"
.mode column
.headers on

SELECT '=== Last 100 heard-block Pokes ===' AS '';
SELECT
  event_num,
  wire_tags_json AS tags,
  printf('%.1f ms', event_processing_duration_us / 1000.0) AS duration,
  datetime(created_at_ms / 1000, 'unixepoch', 'localtime') AS created_at
FROM events
WHERE wire_source = 'libp2p'
  AND wire_tags_json LIKE '%heard-block%'
ORDER BY event_num DESC
LIMIT 100;

SELECT '=== Duration Stats: heard-block (last 100) ===' AS '';
SELECT
  printf('%.1f ms', min(event_processing_duration_us) / 1000.0) AS min_dur,
  printf('%.1f ms', avg(event_processing_duration_us) / 1000.0) AS avg_dur,
  printf('%.1f ms', max(event_processing_duration_us) / 1000.0) AS max_dur,
  printf('%.1f ms', sum(event_processing_duration_us) / 1000.0) AS total_dur,
  count(*) AS count
FROM (
  SELECT event_processing_duration_us FROM events
  WHERE wire_source = 'libp2p'
    AND wire_tags_json LIKE '%heard-block%'
  ORDER BY event_num DESC
  LIMIT 100
);

SELECT '=== Top 20 Slowest heard-block Pokes ===' AS '';
SELECT
  event_num,
  wire_tags_json AS tags,
  printf('%.1f ms', event_processing_duration_us / 1000.0) AS duration,
  datetime(created_at_ms / 1000, 'unixepoch', 'localtime') AS created_at
FROM events
WHERE wire_source = 'libp2p'
  AND wire_tags_json LIKE '%heard-block%'
ORDER BY event_processing_duration_us DESC
LIMIT 20;
SQL
