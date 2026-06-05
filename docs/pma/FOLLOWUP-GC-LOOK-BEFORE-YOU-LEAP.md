# PMA GC Follow-Up: Look Before You Leap

## Context

PMA GC currently compacts by copying reachable state from the active from-space PMA slab into the inactive to-space PMA slab, then switching the active slab metadata to point at the compacted PMA. The important detail is that the current copy algorithm uses forwarding pointers in from-space to preserve sharing and avoid duplicate copies.

Those forwarding pointers intentionally mutate the source noun graph. Because the PMA is file-backed with a shared writable mapping, those mutations can become durable independently of the higher-level GC commit point.

## Why the old active `.meta` cannot simply stay in place

The `.meta` file is the boot-time authority that says a PMA slab is a valid root for the current kernel and event number. If the active from-space `.meta` remains present while GC is rewriting from-space with forwarding pointers, a crash or kill during GC can leave a bootable-looking PMA whose contents are no longer valid durable state.

Forwarding pointers are process-local raw pointers into the to-space mapping, not stable PMA offsets that can be interpreted safely after restart. Keeping `from.meta` until `to.meta` is durable would only be safe if the compaction algorithm did not mutate from-space.

## Current integrity-first ordering

The current safe state machine is integrity-first rather than availability-first:

1. Start with `from.meta` present and `to.meta` absent, so boot selects from-space.
2. Remove any stale inactive `to.meta` and sync the parent directory.
3. Create or truncate to-space.
4. Remove active `from.meta` and sync the parent directory before the first source mutation.
5. Copy reachable state from from-space to to-space, mutating from-space with forwarding pointers.
6. Persist and sync to-space data and trailer.
7. Atomically write `to.meta` and sync the parent directory.
8. Mark the to-space slab as active in memory.

If the process dies between steps 4 and 7, there may be no valid active PMA and boot must recover from a ready snapshot plus event-log replay. This is expected for the current algorithm.

## What LAX1 taught us

The LAX1 incident should not be interpreted as proof that early `from.meta` deletion is wrong. Early deletion is the integrity-preserving choice for a mutating from-space collector. The practical failure was that snapshot fallback recovery was undermined by snapshot cleanup incorrectly treating tracked snapshot artifacts as orphaned and moving them into `corrupted_pma`; that bug has been fixed separately.

LAX1 also had pathological storage behavior, which increased the probability of being killed inside expensive fsync/fdatasync windows. That made the GC kill window more visible, but it did not change the correctness constraint around forwarding pointers.

## Recommended smaller follow-ups before changing GC ordering

1. Strengthen the GC recovery-anchor check before deleting `from.meta`: do not accept only “there is a ready snapshot row”; cheaply verify that the active ready snapshot's PMA and manifest paths exist and that manifest/PMA metadata are readable and internally consistent.
2. Add comments around the `from.meta` deletion explaining that the deletion is deliberately before `copy_from_pma()` because the copy mutates from-space with non-durable forwarding pointers.
3. Add focused tests for the GC-in-progress state: after `from.meta` is removed and before `to.meta` is written, boot should reject both operative PMAs and fall back to a ready snapshot plus event-log replay.
4. Consider operational hardening separately: make the systemd wrapper `exec` the nockchain process and increase `TimeoutStopSec` so normal shutdown has enough time to complete PMA/snapshot sync work.
5. If we want atomic PMA availability across GC later, redesign compaction to avoid mutating from-space, for example with an external relocation table, a private/source scratch mapping, or another non-mutating copy strategy. That is a larger design change and should not be approximated by leaving `from.meta` in place with the current forwarding-pointer collector.
