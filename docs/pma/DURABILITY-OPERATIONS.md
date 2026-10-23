# PMA Durability Operations

This document is the operator-facing guide for the PMA durability path implemented in `nockapp`.

## What Exists Now

The current runtime has:

1. PMA durability enabled by default for normal NockApp boots.
2. SQLite event durability for accepted events.
3. An epoch snapshot advanced by compaction into immutable generations.
4. Rotating snapshots with retention of two ready non-epoch snapshots.
5. Verified snapshot restore with replay from SQLite `job_jam`.
6. Fallback across snapshot candidates, then checkpoint/state-jam bootstrap.
7. Orphan snapshot artifact cleanup into `pma/corrupted_pma/`.
8. PMA GC on an interval after normal event durability has completed.
9. Event-log compaction on a cumulative accepted event-processing time threshold, bounded by the older retained rotating snapshot.

## Relevant CLI Flags

These are the main boot flags operators and production testers should know about:

1. `--ephemeral`
   Runs with in-memory NockStack state only. This disables PMA durability, event logs, snapshots, and PMA GC. Production nodes should not use this except for explicit testing.

2. `--data-dir`
   Overrides the root durability directory.

3. `--event-log-path`
   Overrides the SQLite event log path. Default: `data_dir/event-log.sqlite3`.

4. `--rotating-snapshot-interval-event-time`
   Controls how much cumulative accepted event-processing time must elapse before a new rotating snapshot is attempted. Use `none` or `0` to disable rotating snapshots. Default: `900` seconds.

5. `--epoch-compaction-interval-event-time`
   Controls cumulative accepted event-processing time between epoch compactions, using the same compute-time semantics as snapshot rotation. Default: `1800` seconds. An enabled threshold must be strictly greater than the rotating snapshot threshold (`900` seconds by default); invalid configurations fail before boot creates files. Use `none` or `0` to disable compaction. Disabling rotating snapshots or selecting `--ephemeral` also disables compaction.

6. `--gc-interval`
   Controls PMA GC cadence in wall-clock seconds. Use `none` or `0` to disable PMA GC. Default: `3600` seconds.

7. `--bootstrap-from-chkjam`
   Copies a jammed checkpoint into the data directory as a bootstrap source. This is a migration/bootstrap path, not the normal steady-state recovery path.

8. `--disable-fsync`
   Disables filesystem sync calls, including SQLite full-sync durability. This is for benchmarks or local testing only, not production durability testing.

## Relevant Environment Variables

1. `NOCK_STACK_FREE_GAP_TRIM`
   Controls anonymous NockStack free-gap `madvise(MADV_DONTNEED)` after top-frame flips. This is enabled by default with a 512 MiB free-gap threshold. Set to `0`, `false`, `off`, `no`, `disable`, or `disabled` to opt out.

## On-Disk Layout

Under `data_dir`:

1. `event-log.sqlite3`
2. `event-log.sqlite3-wal` and `event-log.sqlite3-shm` when SQLite WAL mode is active
3. `pma/0.pma`, `pma/1.pma`, `pma/0.meta`, and `pma/1.meta` for operative runtime slabs
4. Initial `pma/epoch.pma` / `pma/epoch.manifest`, followed by immutable `pma/epoch-${TAG}.pma` / `pma/epoch-${TAG}.manifest` generations
5. `pma/snap-${TIMESTAMP}.pma` and `pma/snap-${TIMESTAMP}.manifest`
6. `pma/corrupted_pma/`
7. `checkpoints/` for legacy checkpoint jams used during bootstrap or rollback

Notes:

1. `epoch` and `snap-*` files are snapshot artifacts.
2. `0.pma` / `1.pma` are the operative runtime slabs. GC can switch which slab is active.
3. `corrupted_pma/` is where orphan snapshot artifacts or crash leftovers are moved for later inspection.
4. Compaction prunes accepted events through the epoch boundary after publishing and fully verifying the new epoch. The event suffix needed by both retained rotating snapshots remains in SQLite.
5. Do not manually edit any of these files while the node is running.

## Boot Order

Normal boot first inspects existing operative PMA artifacts and opens the SQLite event log. If PMA artifacts exist and the event log cannot be opened, boot fails closed.

The recovery decision order is:

1. Valid operative PMA fast path when the PMA event number equals SQLite max event number.
2. Special first-migration bootstrap: valid PMA with a nonzero event number and an empty event log.
3. Ready snapshots from SQLite, ordered with the active snapshot first and then remaining ready candidates.
4. Checkpoint/state-only bootstrap, if it is at or after the compacted replay floor, not ahead of SQLite, and replay can reach SQLite max.
5. Fresh kernel plus event-log replay from event 0, only when no prefix has been compacted, SQLite has events, and replay continuity is intact.
6. Fresh kernel with no replay only when SQLite has no committed events.

If PMA is ahead of, behind, missing from, or invalid relative to the event log, it is not treated as authoritative. Boot attempts verified snapshot restore and event-log replay instead.

Snapshots and checkpoints below the compacted replay floor are skipped. Once compaction has removed a prefix, boot requires a recovery base at or after that floor; fresh state cannot replay the retained suffix alone. The operative PMA fast path still uses SQLite's maximum retained event number, because compaction always retains the accepted head.

If continuity is broken in the event log for the chosen boot base, boot fails rather than silently falling back to stale state.

## Snapshot Cleanup Behavior

On boot:

1. Snapshot artifacts without corresponding ready SQLite rows are moved into `pma/corrupted_pma/`.
2. If a snapshot candidate fails verification, it is marked `failed` in SQLite and boot continues to the next candidate.
3. The active snapshot id is updated when a snapshot is successfully restored.

## Epoch Compaction Behavior

Both rotation and compaction count accepted event evaluation time, excluding idle time, rejected events, snapshot copying, and storage maintenance. Their independent counters survive restart through the durations recorded in SQLite. A successful compaction records the accepted head at which it ran, so replay history retained before that head does not count again toward the next compaction.

After an accepted event and any due rotation, compaction checks its compute threshold. It requires two ready rotating snapshots and uses the older one as the new epoch boundary. If fewer than two are available, or the boundary cannot advance, compaction waits. This keeps the epoch at or behind both retained rotations even when an individual event crosses multiple thresholds or snapshot creation falls behind.

A failed compaction attempt waits another full compaction interval of accepted event-processing time before retrying. Restart reconstructs the successful-compaction cadence and may retry immediately if it is already due.

The runtime copies the older rotation into an independent epoch generation, preserves sparse storage, and fully verifies it. A single SQLite transaction publishes the new epoch, retires superseded snapshots, advances the replay floor, and deletes events through that floor. Only after the transaction commits are retired snapshot files removed. A crash before publication leaves the previous epoch and event history available; a crash afterward leaves the new epoch and retained replay suffix available. Boot cleanup handles leftover unpublished or retired files.

Compaction then runs SQLite `VACUUM` and a truncating WAL checkpoint to return freed database and WAL space to the filesystem. Failed physical reclamation remains pending for a later compaction or boot cleanup; it does not undo the durable epoch publication. These operations perform storage IO on the serialized runtime thread and need temporary headroom for the deletion WAL and database rewrite. Compute-time thresholds bound replay work rather than disk bytes, so database size depends on the workload.

## PMA GC Behavior

PMA GC is a discrete phase after accepted-event durability, not a replacement for it.

The normal event path first commits the accepted event to SQLite, copies the durable kernel state into PMA, syncs the PMA state, and publishes PMA metadata. Only after that may GC compact into the inactive slab.

If there is no ready snapshot recovery anchor, GC skips rather than invalidating the only operative PMA recovery source.

## Metrics

The following `nockapp` metrics are relevant to durability and recovery:

1. `nockapp.event_log.append`
2. `nockapp.event_log.commit_failures`
3. `nockapp.snapshot.build`
4. `nockapp.snapshot.build_failures`
5. `nockapp.snapshot.verify`
6. `nockapp.snapshot.verify_failures`
7. `nockapp.snapshot.cleanup`
8. `nockapp.snapshot.cleanup_failures`
9. `nockapp.replay.apply`
10. `nockapp.replay.failures`
11. `nockapp.replay.events`

## Suggested Dashboards

Recommended dashboard panels:

1. Event log append latency: p50 / p95 / p99 of `nockapp.event_log.append`.
2. Snapshot build latency: `nockapp.snapshot.build`.
3. Snapshot verify latency: `nockapp.snapshot.verify`.
4. Snapshot cleanup latency and failure count.
5. Replay duration and replayed event count per boot.
6. Event log commit failure count.
7. Snapshot verify failure count.
8. Replay failure count.

## Suggested Alerts

Recommended alerts:

1. Any non-zero `nockapp.event_log.commit_failures`.
2. Any non-zero `nockapp.snapshot.verify_failures`.
3. Any non-zero `nockapp.replay.failures`.
4. Sustained `nockapp.snapshot.build_failures`.
5. Sustained `nockapp.snapshot.cleanup_failures`.

## Operator Guidance

If boot leaves files in `pma/corrupted_pma/`:

1. Do not delete them immediately.
2. Check the SQLite `snapshots` table and recent logs.
3. Confirm whether the moved files correspond to a crash during snapshot write or a deferred cleanup case.

If boot fails on event-log continuity:

1. Treat it as a real durability problem, not a transient startup failure.
2. Inspect the `events` table for missing `event_num` values.
3. Do not assume checkpoint fallback is safe if accepted events may be missing from the log.
