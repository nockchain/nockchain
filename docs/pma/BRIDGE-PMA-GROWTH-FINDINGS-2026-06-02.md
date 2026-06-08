# Bridge PMA growth findings: June 2026

This note records findings from investigating rapid PMA disk growth on bridge
node A (`bridge-node-a-lsp4`) compared with bridge node B
(`bridge-node-b-vlvj`) around June 1, 2026.

## Summary

The strongest current hypothesis is that node A's rapid PMA growth was not
caused directly by failing peeks. Failed peeks should allocate on the NockStack
and unwind without advancing PMA allocation.

Instead, the code points to repeated PMA boot/load/preserve attempts under a
kernel-hash mismatch and bridge `+load` migration loop. Those boot attempts can
append to the PMA at the same event boundary, and PMA allocation metadata can be
advanced by allocation paths that are not themselves new accepted-event
frontiers.

There is also a suspicious implementation/design mismatch: the docs say runtime
caches are not restored or persisted as durable PMA state, but the current
`preserve_event_update_leftovers` path copies runtime state into PMA after
publishing the durable kernel state. Because PMA raw allocation writes the PMA
trailer allocation offset, those runtime copies can still advance the operative
PMA allocation pointer.

## Observed timeline

All local times below are America/Los_Angeles.

- Node A repeatedly logged PMA/kernel hash mismatch and bridge `+load` state
  upgrade messages. Node B did not.
- Node A's live compacted state remained about 1.6-1.7 GiB after normal GC.
- On June 1 at 16:45:54 PDT, node A compacted to `223,167,620` PMA words.
- On June 1 at 17:52:19 PDT, node A was already at `1,063,735,599` PMA
  allocated words and crossed the preflight reserve threshold, causing growth
  from 8 GiB to 16 GiB.
- The growth preflight itself was only short by a small reserve margin. The
  large allocation increase had already accumulated before the growth trigger.

This makes the preflight growth a symptom of prior PMA allocation churn, not the
root cause of the multi-GiB increase.

## What failed peeks do

The peek action path does not preserve PMA state:

- `SerfAction::Peek` copies the request onto the stack, calls `serf.peek`, and
  copies the result into a `NounSlab`.
- It does not call `preserve_event_update_leftovers`.
- It does not append an event log entry.
- It does not publish PMA metadata.

Relevant code:

- `open/crates/nockapp/src/kernel/form.rs`: `SerfAction::Peek`
- `open/crates/nockapp/src/kernel/form.rs`: `Serf::peek`

The interpreter also snapshots and restores runtime context on errors:

- `open/crates/nockvm/rust/nockvm/src/interpreter.rs`: `Context::save`
- `open/crates/nockvm/rust/nockvm/src/interpreter.rs`: `Context::restore`
- `open/crates/nockvm/rust/nockvm/src/interpreter.rs`: `exit`

So failed peeks should not explain persistent PMA allocation growth by
themselves.

## What accepted pokes do

Accepted pokes do advance durable state:

- The poke action calls `ensure_pma_capacity_before_event`.
- If the poke is accepted, `do_poke` installs the new Arvo root with
  `event_update`.
- The accepted job may be captured for the SQLite event log.
- The cleanup path calls `preserve_event_update_leftovers_with_pre_persist`.

Failed live pokes can still become durable if `poke_swap` synthesizes and
accepts a `%crud` replacement. Peeks do not use this path.

Relevant code:

- `open/crates/nockapp/src/kernel/form.rs`: `SerfAction::Poke`
- `open/crates/nockapp/src/kernel/form.rs`: `do_poke`
- `open/crates/nockapp/src/kernel/form.rs`: `poke_swap`
- `open/crates/nockapp/src/kernel/form.rs`: `capture_accepted_event`

However, accepted startup pokes alone do not fully explain growth at the same
event boundary during repeated boot/load loops.

## Boot-time PMA preserve path

On PMA boot, existing PMA state is loaded by sidecar metadata. A kernel-hash
mismatch does not reject the operative PMA. Instead, the persisted state noun is
loaded into the current kernel, which runs Hoon-level `+load` migration.

Relevant code:

- `open/crates/nockapp/src/kernel/form.rs`: `inspect_existing_pma`
- `open/crates/nockapp/src/kernel/boot.rs`: PMA boot selection
- `open/crates/nockapp/src/kernel/form.rs`: `Serf::new`

In `Serf::new`, after loading the old kernel state through the current kernel,
the code calls:

```rust
serf.event_update(event_num_raw, arvo);
let _ = serf.preserve_event_update_leftovers();
```

This happens during initialization and can happen at the same `event_num` that
was already stored in PMA metadata. If the node repeatedly restarts through this
path, it can repeatedly copy/preserve loaded state without a new accepted event
number.

For the bridge app, `+load` logs `bridge: +load state upgrade required` when it
upgrades old versioned bridge states.

Relevant code:

- `open/hoon/apps/bridge/bridge.hoon`: `++load`

## PMA allocation metadata behavior

PMA allocation is bump-only until GC. More importantly for this incident,
`raw_alloc` calls `persist_metadata` after each allocation, which updates the PMA
trailer allocation offset.

Relevant code:

- `open/crates/nockvm/rust/nockvm/src/pma.rs`: `Pma::raw_alloc`
- `open/crates/nockvm/rust/nockvm/src/pma.rs`: `Pma::persist_metadata`
- `open/crates/nockvm/rust/nockvm/src/pma.rs`: `Pma::open_with_min_inner`

On reopen, the PMA allocation offset is restored from the trailer. This means an
interrupted or repeated boot-time copy can leave a larger PMA allocation offset
even if the sidecar `.meta` continues to point at the old event/root.

That creates a plausible loop:

1. Boot selects PMA because sidecar event number matches SQLite.
2. Kernel hash mismatch causes state to load into the current kernel.
3. Bridge `+load` migration runs.
4. Initialization preserve copies state into PMA.
5. PMA raw allocations advance the trailer allocation offset.
6. The node exits or restarts before reaching stable operation/GC.
7. The next boot resumes from the larger trailer offset and repeats.

## Runtime-cache copy mismatch

`preserve_event_update_leftovers_with_pre_persist` does the following when PMA
is active:

1. Copy durable `arvo` state into PMA.
2. Run a pre-persist hook.
3. Persist PMA state and sidecar metadata.
4. Copy runtime state into PMA: `warm`, `test_jets`, `hot`, `cache`, `cold`.
5. Maybe run PMA GC.
6. Flip the NockStack frame.

Relevant code:

- `open/crates/nockapp/src/kernel/form.rs`: `copy_durable_state_to_pma`
- `open/crates/nockapp/src/kernel/form.rs`: `copy_runtime_state_to_pma`
- `open/crates/nockapp/src/kernel/form.rs`: `preserve_event_update_leftovers_with_pre_persist`

This is suspicious because PMA docs say runtime caches are not persisted or
restored as durable state:

- `open/PMA-FAQ.md`: "PMA does not persist or restore derived runtime caches"
- `open/docs/pma/DESIGN.md`: "The durable PMA state is only the Arvo/kernel
  state"

The roots for those runtime structures may not be restored on boot, but copying
them into the operative PMA still advances the PMA allocation offset. Because
`raw_alloc` publishes the trailer offset, these copies can contribute to file
growth even though they are not durable kernel state.

## Current conclusion

The rapid node A growth is best explained as PMA allocation churn during a
repeated PMA boot/load/preserve loop, not as failed peek allocation being
committed.

The most suspicious code paths are:

1. Boot-time `preserve_event_update_leftovers` after loading PMA state into a
   mismatched/current kernel.
2. PMA `raw_alloc` publishing the trailer allocation offset on every allocation.
3. Runtime-cache copies into PMA after durable state has already been published.
4. Lack of GC while the process repeatedly restarts before stable operation.

The bridge deposit replay peek failure is still relevant as the likely reason
the process does not stay up, but it is probably not the direct PMA growth
mechanism.

## Follow-up checks

Useful follow-ups:

- Add or enable per-segment PMA allocation logging during boot, especially for
  `pma_arvo_copy`, `pma_warm_copy`, `pma_hot_copy`, `pma_cache_copy`, and
  `pma_cold_copy`.
- Confirm whether boot-time preserve should be allowed to advance PMA at the
  same event number.
- Decide whether runtime cache copying should advance the operative PMA trailer
  allocation offset.
- Consider a test that simulates repeated PMA kernel-hash mismatch boot with
  `+load` migration at the same event number and asserts bounded PMA allocation
  growth.
- Consider a failure-injection test that interrupts boot-time preserve and
  verifies PMA trailer/sidecar consistency on the next boot.
