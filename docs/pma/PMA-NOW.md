# PMA-Ready NockVM Plan (Option 1)

> Historical implementation-plan/status note. For current public-release guidance, prefer [`DESIGN.md`](./DESIGN.md) and [`DURABILITY-OPERATIONS.md`](./DURABILITY-OPERATIONS.md).

Goal: convert every allocated noun to carry a position‑independent payload (stack-pointer vs. PMA offset bit), so the Serf slab can be mmap-cloned into read-only CoW replicas without corrupting pointers. We must preserve today’s interpreter hot-path throughput; the only acceptable overhead is a couple of masks/shifts per dereference. We now also want the full nockchain-api stack to run on the memfd-backed PMA so we can measure resident usage under real workloads.

## Tagged commits

### retag-perf-hoonc-builds
This tag improves the performance of retag_noun_tree
such that `hoonc` is capable of building the Nockchain kernel in
under 10 minutes. The `hoonc_hotspots` benchmark also builds and runs, with
the following performance regression from `nockchain/master`:
   
   ```
  | Benchmark                  | Master   | Current Branch | Regression |
  |----------------------------|----------|----------------|------------|
  | hamt_symbol_table_hot_path | 56.4 µs  | 97.1 µs        | +72%       |
  | noun_preserve_deep_core    | 77.5 µs  | 122.5 µs       | +58%       |
  | unifying_equality_canopy   | 32.3 µs  | 47.4 µs        | +47%       |
  | interpret_hint_stack       | 5.86 µs  | 12.5 µs        | +113%      |
  | warm_jet_lookup            | 5.57 µs  | 12.5 µs        | +124%      |
  | context_cache_churn        | 69.3 µs  | 98.5 µs        | +42%       |
  | cue_jam_roundtrip          | 486.0 µs | 595.6 µs       | +23%       |
   ```
   
Prior to this, it never finished parsing the first file. However, `hoonc` will
crash when it tries to write the file to disk, due to issues with the
`NockStack`/`NounSlab` interface. Basically, there are methods called by the
file and exit driver that want a `NockStack` with `install_arena()` called,
despite the fact that the effects they are processing are all pointer-allocated.
There is a workaround in `98ba6ea` that simply creates emphemeral `NockStack`s
just to satisfy the API requirement, but this is obviously not a real solution.

## Phase 0 – Preconditions

- ✅ Branch is frozen around this document; we treat the plan as source of truth.
- ✅ We have a clean baseline after selectively opting out of fmt/clippy per user guidance.
- ✅ Perf baselines captured (`cargo test --release -p nockvm`, `criterion` microbenches); use them when validating further steps.

## Phase 1 – Arena metadata & pointer math (`open/crates/nockvm/rust/nockvm/src/mem.rs`)

✅ **Done**

1. `Arena` now always owns a memfd-backed slab (`rustix::memfd_create` + `ftruncate` + `MmapMut`). It exposes `ptr_from_offset`, `offset_from_ptr`, `map_copy_read_only`.
2. `NockStack` stores an `Arc<Arena>` and provides `arena()`, `install_arena()`, `ptr_from_offset()`, `offset_from_ptr()`.
3. Allocators (`struct_alloc`, `alloc_cell`, preserve paths, slab copies) tag offsets immediately; forwarding pointers also store offsets.
4. Every stack consumer installs the arena via TLS guards; unit tests, doctests, integration tests now all use small helpers/guards so we no longer see `Arena::with_current` panics.

## Phase 2 – Tagged pointer representation (`open/crates/nockvm/rust/nockvm/src/noun.rs`)

✅ **Done**

1. `TaggedPtr` wraps tag/location/payload and backs every `IndirectAtom`/`Cell` constructor and dereference path.
2. Every stack consumer (`stack.install_arena()` in HAMT, jets, serialization, noun slabs, benches, context builders) ensures the TLS slot holds the active arena before tagged pointers are resolved; `Arena::with_current` now just validates that invariant.
3. Constructors that load from slabs/checkpoints/jam all tag offsets immediately via `stack.offset_from_ptr`.
4. Jets/HAMT/serialization are arena-aware through the install hook, keeping the interpreter hot path unchanged and ready for replicas.

## Phase 3 – Nursery→permanent retagging (still in `mem.rs` / `noun.rs`)

✅ **Done**

- Added `NockStack::retag_noun_tree` plus a lightweight `Retag` trait so `Serf::retag_survivors` rewrites every preserved root (Arvo, scry stack, cache, hot/warm/cold, and the test-jet HAMTs) into offset form immediately after each `preserve_event_update_leftovers`.
- `Hamt`, `WarmEntry`, `Hot`, `Cold`, `Batteries*`, and `NounList` now participate in the retag walker, meaning the caches no longer rely on another preserve pass to shed stack pointers.
- Regression coverage: `hamt_retag_converts_entries_to_offsets` asserts HAMT keys/values are PMA-safe, and the existing debug sweep stays enabled under `debug_assertions` for extra safety when running integration tests.

## Phase 4 – Interpreter/context plumbing (`open/crates/nockvm/rust/nockvm/src/interpreter.rs` and friends)

✅/⚠️

- ✅ `Context` now stores an `Arc<Arena>`; `create_context`, HAMT, serialization, jets, benches, noun slabs call `stack.install_arena()` before touching tagged nouns.
- ⚠️ We still rely on TLS for dereferences. To make replicas safe under heavy load we eventually need to pass `&Arena` explicitly to the few hot helpers (slotting, jets) and delete the fallback from `TaggedPtr`. That change is invasive, so it comes **after** we shake out nockchain-api testing with TLS still enabled.

## Phase 5 – CoW replica scaffolding (nockapp integration + nockchain-api)

⚠️ **Not started yet** – this is the critical path toward running `nockchain-api` atop the new PMA.

Tasks needed:
- `Arena::clone_read_only()` exists, but we also need `Arena::map_copy_private_stack()` or an equivalent overlay so replicas get private write space for the nursery/stack without faulting the permanent slab.
- SerfReplica pool with:
  - deterministic poke replay stream (leader keeps feeding changes),
  - read-only peek scheduling, load-balancing, and back-pressure,
  - metrics for replica lag and residency.
- End-to-end integration tests (e.g., `nockapp_grpc` peeking through replicas).
- CLI glue so `nockchain-api` can enable the replica pool behind a feature flag.

Until this phase is complete, running `nockchain-api` will exercise the memfd PMA but still serialize peeks through the leader arena.

## Testing & Verification Plan

1. **Unit / Debug assertions:**
   - `noun.rs`: round-trip `as_raw()`/`from_raw()` for each kind/location combination; ensure offsets resolve to the correct addresses via mock arenas.
   - `mem.rs`: property tests (`proptest`) verifying `ptr_from_offset(offset_from_ptr(ptr)) == ptr` for various alignments/orientations.
   - Preservation tests: craft nouns containing forwarding pointers, run `noun_preserve`, and assert location bits remain valid.
   - The serf now runs a debug-only sweep (`debug_assert_offsets`) after each `preserve_event_update_leftovers` that panics if any noun still carries a stack pointer. Running the existing integration tests with `debug_assertions` enabled exercises this automatically.

2. **Integration / system tests:**
   - Existing `nockapp` integration tests (`open/crates/nockapp/tests/integration.rs`) should be run under both `mmap` and `malloc` features to catch regressions.
   - Add a new test that imports a checkpoint, clones the arena into a fake replica, and runs a peek to validate that offsets stay stable after remapping.
   - For OS-level validation that paging behaves correctly with the memfd-backed slab:
     1. Launch a nockapp instance with a very large snapshot so the slab consumes multiple gigabytes.
     2. Record the PID and inspect `/proc/$PID/smaps` (or `pmap -x`) to capture the `nockstack` VMA’s RSS/Swap.
     3. Trigger a read-only workload (peeks via the upcoming replica pool) so replicas fault pages lazily.
     4. Use `echo 3 | sudo tee /proc/sys/vm/drop_caches`, `mincore`, or `perf mem` to ensure the kernel can drop cold pages and fault them back in for replicas.
     5. Compare before/after RSS to confirm the shared memfd enables CoW instead of duplicating the slab.

3. **Performance guardrails:**
   - Extend the `criterion` benches (e.g., `benches/hoonc_hotspots.rs`) to measure axis traversal and jets with the new tagging.
   - Compare against the Phase 0 baseline; permissible regression is ≤1 % on median latency. If larger, profile using `perf` to locate hot spots (tag decoding, arena rebasing, etc.) and optimize.

4. **Miri & sanitizers:**
   - Run `cargo miri test -p nockvm` for pointer-heavy modules (`noun`, `mem`, `interpreter`). We have already ported every unit test/ doctest to install arenas so Miri can run the lightweight subset without TLS failures.
   - Address Sanitizer via `RUSTFLAGS="-Zsanitizer=address"` for the nockvm crate to catch misuse of the new pointer math.

5. **Documentation & rollout:**
   - Update `NOCK-PMA.md` with actual code snippets once implemented.
   - Document the new invariants in `open/crates/nockvm/DEVELOPERS.md` (how the arena/offset dance works, expectations for jets).
   - Provide a migration note in `CHANGELOG.md` describing the new noun representation.

## Sequencing & Code Review Checklist

1. Phase 1 + 2 landed; keep the codepath always-on (no flag).
2. Phase 3 retagging is live; next up is Phase 5 (replicas / peek scaling) while maintaining the existing perf guardrails.
3. Every PR must include:
   - Updated tests/benchmarks.
   - Proof (metrics or benchmark output) that the interpreter hot path did not regress beyond the tolerance window.

## Driving Toward nockchain-api & PMA Memory Measurements

To run `nockchain-api` on the memfd-backed PMA and collect resident memory data we need:

1. ✅ **Phase 3 retagging** is in place, so snapshots/peeks never see stack pointers and the leader slab is PMA-safe.
2. **Replica overlay (Phase 5)** remains pending for multi-replica peek scaling; it is not required for the immediate single-serf RSS study but must land before we farm peeks to read-only stacks.
3. The `nockapp::kernel::boot::Cli` now takes `--data-dir` (full path to the checkpoints directory) and `nockchain-cli` exposes `--identity-path`, so `cargo run -p nockchain-api -- --data-dir test-api/.data.nockchain --identity-path test-api/.nockchain_identity ...` boots directly on the canned 2.5 GiB snapshot. See `open/crates/nockchain-api/README.md` for the exact command and notes on the `nockvm.pma.*` metrics to watch.
3. **Operational checklist** for perf/memory studies:
   - Start `nockchain-api` with `NOCKVM_MEMFD_STACK=1` (or a dedicated env) and the replica flag.
   - Use `gnort` metrics (`nockvm.pma.*`) to monitor residency, touched pages, and post-peek ratios.
   - Record `/proc/$PID/smaps` while issuing peeks via gRPC (simulate pokes concurrently) to ensure the kernel is paging unused chunks.
   - Compare RSS vs. the old anonymous mmap builds.
4. **Docs/tests**: update `PMA-NOW.md` (this file) whenever steps shift, and add a “replica smoke test” under `open/crates/nockapp/tests` once Phase 5 lands.

This plan gets us to position-independent nouns with measured, reviewable steps, unlocking the mmap CoW replicas without jeopardizing interpreter performance.
