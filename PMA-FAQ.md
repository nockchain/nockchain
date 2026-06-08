# PMA FAQ

This FAQ is for node operators running a PMA-enabled Nockchain release.

PMA means Persistent Memory Arena. It changes how Nockchain stores and reloads the NockApp kernel state. It is not a protocol feature, not a git artifact, and not a replacement for your keys.

## What changes for operators?

In a PMA-enabled release, Nockchain stores the live kernel state in a file-backed persistent memory arena under the node data directory. The main operator-facing result is lower steady-state resident memory for the NockApp/Serf state and faster persistence after accepted events.

You should not need to change the command or systemd unit that runs `nockchain`. On first boot, the PMA version should discover the existing data directory and migrate from the latest checkpoint if PMA state is not present yet.

## What happens the first time I run the PMA version?

If the data directory has legacy checkpoints but no usable PMA or PMA event log, boot uses the latest checkpoint as a bootstrap boundary.

The checkpoint jam is treated like a state jam:

- The kernel state is imported.
- The checkpoint cold jet cache is ignored.
- A fresh runtime cold cache is created.
- PMA state and metadata are written.
- An epoch snapshot is created.
- The SQLite event log starts recording accepted events after the checkpoint boundary.

That last point matters. The node is not replaying every historical event into SQLite. The checkpoint establishes the trusted starting state, and the PMA event log records new accepted events after that point. During this bootstrap window, an empty event log plus PMA metadata at the checkpoint event is expected.

## Will first boot use more memory than normal PMA boot?

Yes. First-time migration from a checkpoint still has to cue and hydrate the checkpoint state before it can copy that state into PMA. Do not shrink a server's memory allocation before its first successful PMA boot.

After the PMA state has been created, later boots should use the PMA fast path and avoid the large checkpoint hydration peak. Let the node boot, build PMA state, and run for a bit before lowering memory limits.

## What should I see in the logs?

On first migration from a checkpoint, look for a line like:

```text
Boot source: checkpoint ... empty_event_log_bootstrap=true checkpoint_cold_state=empty
```

On later PMA boots, look for a line like:

```text
Boot source: PMA ...
```

During normal event processing, PMA timing logs include fields such as `event_eval_ms`, `preserve_ms`, `durable_append_ms`, and `cleanup_total_ms`. These split event execution from PMA/event-log durability work.

## Can I revert to the previous non-PMA version?

The PMA migration does not delete existing checkpoint jams. If you start a PMA release and hit a problem, you should be able to stop it, revert to the previous non-PMA binary, and continue from the existing checkpoint data while reporting the PMA issue.

Treat PMA as a forward migration path once you are satisfied it is healthy. The rollback story is for operational escape hatches, not for bouncing back and forth indefinitely.

## Is PMA a cache?

No, not in the disposable sense. PMA is the node's durable local kernel-state store after migration.

Some runtime structures are still caches. PMA does not persist or restore derived runtime caches such as `hot`, `warm`, `cold`, jet-test HAMTs, or memo tables. Those are rebuilt on boot. The durable PMA state is the kernel state itself.

## Why does PMA rebuild cold jet state on boot?

The cold jet state is an optimization cache built out of HAMTs and linked runtime structures containing process-local pointers. Those pointers are not a stable disk format.

PMA stores durable Nouns in offset form so they can be reloaded after the file is mapped again. The runtime cold cache is rebuilt from empty state and repopulated as the kernel runs. This is why checkpoint bootstrap imports checkpoint jams as state-only data and ignores serialized checkpoint cold state.

## Can I copy the PMA directory from somebody else?

No. Do not import or copy raw PMA files from third parties.

Raw PMA files are local runtime artifacts. They are not consensus artifacts, release artifacts, or safe community bootstrap files. If you need to bootstrap a node from somebody else, use a trusted state jam or other supported bootstrap artifact instead.

## Can I copy PMA files between my own machines?

Only if you control both machines and know exactly what you are doing. Even then, the safer approach is to copy recovery artifacts rather than the live operative PMA.

Prefer copying:

- The SQLite event log and its sidecars.
- Verified snapshots and their manifests.
- Checkpoints, if you are still using a pre-PMA bootstrap path.

Avoid copying just:

- `pma/*.pma`
- `pma/*.meta`

The operative PMA fast path trusts sidecar/trailer consistency and event-number agreement. Snapshots have stronger integrity metadata and are a better transfer boundary.

## Should PMA files be checked into git?

No. PMA files, checkpoints, snapshots, and event logs are large node-state artifacts. They do not belong in the git repository.

The source tree contains code and small fixtures. Chain state belongs in the node data directory.

## What files are in the PMA data directory?

The exact directory depends on your `--data-dir`, but the important pieces are:

- `pma/`: operative PMA slabs and PMA metadata sidecars.
- `event-log.sqlite3`: SQLite accepted-event log for events after the current recovery boundary.
- `event-log.sqlite3-wal` and `event-log.sqlite3-shm`: SQLite WAL sidecars when WAL mode is active.
- `checkpoints/`: legacy checkpoint jams, useful for first PMA bootstrap and rollback.
- snapshot PMA files and manifests under the PMA-managed snapshot paths.

Do not manually edit these files. If you are troubleshooting, stop the node first and preserve copies before deleting anything.

## What happens after a crash or power loss?

PMA commits accepted events in a strict order:

1. The accepted event is committed to SQLite.
2. The new kernel-state frontier is copied into PMA.
3. The PMA file is synced.
4. The PMA `.meta` sidecar is written last.

On boot, Nockchain checks whether PMA and SQLite agree on the event boundary. If they agree, PMA can use the fast path. The first checkpoint bootstrap into an empty event log is the special case: PMA may be at the checkpoint event while SQLite has not recorded any post-checkpoint events yet. Outside that bootstrap case, if PMA is missing, invalid, behind SQLite, or ahead of SQLite, boot enters recovery and uses verified snapshots, checkpoints, and event-log replay as appropriate.

## Does PMA make every workload use less memory?

No. PMA reduces the memory pressure from the NockApp/Serf kernel state. It does not automatically reduce memory used by unrelated process caches.

For example, public gRPC block explorer caches can still use substantial memory. PMA helps the node's kernel-state storage, but separate API caches need their own storage strategy.

## Does PMA make syncing faster?

It can, especially for workloads that spend a lot of time preserving or checkpointing large state. PMA avoids repeatedly jamming the whole subject for ordinary durability and avoids retaining the whole durable subject in anonymous heap memory during normal operation.

It is not a magic protocol shortcut. Network, peer quality, block validation, disk speed, and API workload still matter.

## Can I disable PMA?

In a PMA-enabled Nockchain release, PMA is the normal operating mode for NockApps and Nockchain peers. It is not intended to be an optional cache layer that operators toggle on and off.

Development and testing tools may expose lower-level knobs, but production operators should treat PMA as the persistence mode for that release.

## Is it safe to delete checkpoints after PMA bootstraps?

Do not rush to delete them. Existing checkpoints are useful as a rollback and recovery aid while you are validating a PMA upgrade.

Once you are confident the node has booted from PMA repeatedly and has healthy snapshots/event logs, checkpoint retention becomes an operator storage-policy decision. If in doubt, keep the latest known-good checkpoint until the PMA release has aged on your machine.

## Where are the deeper PMA docs?

See [`docs/pma/`](./docs/pma/) for PMA durability, design, memory-attribution, and noun-provenance docs.
