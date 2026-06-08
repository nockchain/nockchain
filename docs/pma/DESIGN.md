# NockVM's Persistent Memory Arena

Date: 2026-04-29
Author: @bitemyapp

This is a near-end-of-implementation-effort summary of the design intent for NockVM's PMA (persistent memory arena). This is intended to help both humans and LLMs understand what the PMA is expected to do and why at a high-level and what considerations and trade-offs went into those decisions.

## History

NockVM is a general purpose runtime for Nock, something it shares in common with Urbit's Vere. NockVM started as Ares, a next-generation runtime for Vere designed to overcome some design infelicities that were limiting runtime performance. The principal designer and author was Edward Amsden. I picked up and started working on `nockvm` along with some parts of the Nockchain platform a couple years prior to finishing the PMA.

NockVM is still slower in a straight line for the runtime execution of Nock than Vere, but the implementation has proved itself in the areas where it's closest to the performance frontier: garbage collection built on copying compaction. NockVM's `NockStack` uses East/West alternating stack frames for runtime data. Not having the overhead of reference-counting and a more efficient garbage collection implementation has enabled NockVM to generally be faster on Linux than on macOS. The opposite is true for Vere, which tends to be faster on Apple Silicon, largely because of the burden reference counting imposes on memory bandwidth and latency.

The main thing preventing NockVM from exceeding Vere's runtime efficiency at present is that it's still got a relatively naive mutually recursive interpreter. I have taken the current design of NockVM about as far as I can take it in terms of efficiency. I used `hoonc` (compiling Hoon using the Hoon-compiler-in-Hoon running in the Nock runtime) to benchmark and compare NockVM and Vere because it is close to a worst-case benchmark for the current interpreter. My last major nockvm efficiency pass got the runtime performance delta down to 2.5x from 5x. I believe when I started working on NockVM the delta was closer to 10x.

I believe the next major leap will necessarily come from replacing the recursive interpreter with a bytecode VM. There are some things about how Nock (and Hoon) work that makes this a little trickier than you'd normally anticipate, principally because Nock can generate new formulas to evaluate at runtime. I'm assuming that some blend of AoT and JIT bytecode compilation and caching will be required but I haven't sunk my teeth into this problem yet.

The more efficient GC design in NockVM has helped tremendously with making Nockchain viable as new blocks get mined and the chain continues to grow. In the pre-PMA design, we handled persistence by checkpointing the subject (all of the state in the VM, "Arvo" in Urbit parlance) to disk at a fixed time interval. This backup-to-disk checkpointing process was a source of some heartache because it required jamming the subject which meant that the time required to construct and persist these checkpoint jams was increasing linearly with the size of the subject. The problem is that the subject itself grows as the chain state grows. For the ZKVM and blockchain state machine to be sound, all of the chain state needs to be addressable as first-class "Nouns" (values in Hoon or Nock) within the VM.

We didn't notice this until I was testing and comparing PMA performance against the baseline implementation, but we were experiencing parasitic loss from the NockStack having to compact the entire subject in the final flip-top-frame of a `poke` operation. The PMA spares us this and the extra durability work is a few orders of magnitude faster than that was.

So we needed some kind of persistent memory arena or virtual memory based solution for paging out unused parts of the subject so that the RAM requirements of a Nockchain peer didn't keep increasing linearly with chain history. The PMA in the branch carrying this document appears to have accomplished this.

## What

- Works well on macOS and Linux alike.
- Don't require retaining the entire subject in memory simultaneously during ordinary operations
- Durability story should be strictly better from a promptness perspective than baseline's checkpointing intervals
- PMA should recover safely from incomplete writes / slab corruption due to SIGKILL or power loss
- Snapshotting replaces checkpointing, takes ~5 seconds instead of 35-140 seconds as checkpointing requires.
- The PMA should, as much as possible while respecting runtime efficiency and memory constraints, be kind to solid state drives and not burn them out quickly. The NockStack is still being used, it's just ephemeral and the write-back fsyncs to the PMA happen after the event is finished executing and leftovers (persistent updates to the subject) are getting rolled up.
- Runtime execution efficiency is not worse than baseline by more than 10-20%. `hoonc` is close to a worst-case scenario and after some optimization work I got the delta between PMA and baseline down from 4x to 1.13x.
- At 50k+ blocks the PMA peers sit at about 7-8 GiB of steady-state RSS. Baseline is ~18-19 GiB with spikes to ~35-39 GiB while checkpointing, which is much of the time. The default recommendation at time of writing for baseline was to use 64 GiB servers.

## How

The PMA is intentionally very conventional and boring, or at least as much as I could make it given the _slightly_ exotic structure of Nock Nouns. In classic PHK/Varnish style, it leans hard on the virtual memory subsystem to handle paging data in and out of RAM as required. 

### Virtual memory

The existing slab allocation in NockStack was already using anonymous mmaps. It could use `malloc` and I added that as an opt-in to NockStack in the past, but a multi-gigabyte slab allocation requested via `malloc` is presumptively going to turn into an anonymous mmap anyway. Hard to imagine libc deciding to `sbrk` 16 GiB. Instead of an anonymous memory map, the PMA is a shared mmap. "Shared" here refers to the fact that it actually lives as a persistent file on disk and hypothetically other processes could map the same segment into RAM without duplication. Don't do that though, we didn't design the PMA to be multi-process or multi-threading safe. A sufficiently intrepid hacker could probably prototype multi-threaded Nock execution using a shared PMA. Just be forewarned that the PMA effort was pretty gnarly, adding multi-threading to that could be difficult to make 100% correct.

With a shared mmap we are relying on the kernel's virtual memory subsystem to flush memory mutated by the application to disk, to page out unused (cold) data, and to page-fault in cold data that got read or written to again.

### Durability

Much of what's to be said about durability here concerns two main areas of the platform, booting a NockApp and wrapping up `poke` events. Our risk model principally concerned itself with scenarios like power loss, `SIGKILL`, and the like. I explicitly did not attempt to address cosmic bit-flips, nefarious processes with write permissions to the slab, or anything else similarly out-of-pocket. The blockchain is a distributed and replicated computer system, this isn't a computer in a probe going past the asteroid belt.

Most important parts of the durability story:

- PMA slab + PMA .meta
- `sqlite3` event log
- Snapshots (lightly augmented copies of the PMA slab)

The basic order of things persistence-wise is this:

- NockVM executes `poke`
- If the event is accepted, capture the exact accepted job for the event log. A failed live poke can still synthesize a `%crud` replacement through the ordinary live `poke_swap` path; if that replacement is accepted, the replacement event is what gets logged.
- Commit the accepted event to the `sqlite3` event log in an immediate transaction. Event-log append failure is fatal because the PMA must not be advanced past the SQLite accepted-event boundary.
- Bump allocate the new persistent Arvo frontier into the PMA.
- Rewrite the new root/spine slots being copied so they point at either newly allocated PMA offsets or existing PMA offsets. Existing PMA subtrees are not rewritten during ordinary event commit.
- Write the PMA file trailer. The trailer records PMA file shape, including the allocation offset. It does _not_ record the accepted event number.
- `fdatasync` the PMA file. Failure is fatal on the durability-critical path.
- Write the PMA `.meta` sidecar last. The sidecar records the kernel hash, accepted event number, root pointer for the kernel state, and a checksum. Sidecar write failure is fatal on the durability-critical path.

The idea here is straightforward. We can't go to the same lengths guarding against corruption in the operative PMA slab that we can for snapshots. But if we're always committing to the `sqlite3` event log before we touch anything in the PMA _and_ the sidecar `.meta` event id (monotonic number for events executed) is the last thing written and fsync'd, then we can detect incomplete write sequences because the PMA's sidecar event id will be behind the latest event id in the event log. In practice, we treat any inequality of the event ids between the PMA and event log on boot as a DR (disaster recovery) trigger.

This does not make the operative PMA slab a fully checksummed data structure. The fast path validates the sidecar checksum/kernel hash, checks that the PMA opens and that trailer/file metadata are coherent, and requires the PMA sidecar event number to equal SQLite's max event number. It does not hash or traverse the operative slab the way snapshot recovery does. The operative PMA durability model is primarily about detecting torn or incomplete accepted-event persistence, not arbitrary same-boundary PMA data corruption.

### Event write-back

After an accepted event, the kernel installs a single new Arvo root. That root is a complete Arvo value, not a separate delta object, but in normal operation it is a mixed graph: newly allocated NockStack cells for the paths rebuilt by the event, novel durable subtrees created by the event, direct atoms, and references to old PMA-resident subtrees. The graph itself is the dirty-path information.

PMA write-back starts at that new root and walks forward. Stack-allocated cells and indirect atoms are copied into the PMA and rewritten to offset form. Direct atoms are embedded directly. Existing PMA offsets are treated as terminals: the copier writes the offset into the new parent slot and does not descend through the old subtree. Forwarding pointers are used only inside the source copy space to preserve sharing among newly copied objects, so if the new graph references the same novel subtree twice it is copied once and both destination slots reuse the same PMA offset.

The practical append size for one ordinary event is therefore approximately the new Arvo root plus the rebuilt spines to changed regions plus the novel durable subtrees themselves. Unchanged old subtrees are included by reference, not copied. In units of the implementation, each copied cell is three 64-bit words (`metadata`, `head`, `tail`), each copied indirect atom is two header words plus its data words, and direct atoms do not allocate separate PMA storage. `NOCK_PMA_TIMING_DETAIL` records the appended word count for the Arvo copy as `detail.arvo.alloc_words`; multiplying by eight gives the byte count of PMA data newly appended for that event.

Ordinary event write-back should not dirty old PMA pages that contain unchanged Arvo subtrees. It dirties the newly appended bump-allocation range, possibly the previously partially filled trailing page, the PMA trailer containing the allocation offset, the `.meta` sidecar, and the SQLite/WAL pages. This is why event commit remains fast when the kernel preserves structural sharing. If kernel code reconstructs a large list or map instead of sharing it, PMA persistence will append that newly reconstructed structure; the PMA cannot infer sharing that the returned Arvo graph did not preserve.

### Runtime caches and checkpoint bootstrap

The durable PMA state is only the Arvo/kernel state. PMA boot does not persist or restore derived runtime caches such as `hot`, `warm`, `cold`, jet-test HAMTs, or the memo cache. In particular, the NockVM cold jet state is implemented as HAMTs and linked runtime structures containing native process pointers. Those pointers are appropriate for the current mapped process and stack/PMA arenas, but they are not a stable restart-persistent format. The PMA uses offset-form Nouns for durable state; the cold runtime cache is rebuilt from empty state on boot and repopulated as the kernel runs.

This is also why first-time PMA bootstrap from legacy checkpoints treats checkpoint jams like state jams. Checkpoint files may contain both kernel state and serialized cold jet state, but PMA migration only imports the kernel state portion and initializes cold state as empty. The checkpoint cold state is an optimization cache, not chain state. Hydrating it during PMA migration increases transient memory pressure substantially and is not required for correctness. After the checkpoint state has been copied into PMA and PMA metadata has been published, later boots use the PMA fast path and rebuild runtime caches in the same way as ordinary PMA restarts.

### Event log canonicity

SQLite is the accepted-event authority. The event log uses WAL mode and `synchronous=FULL` unless durability syncing has explicitly been disabled. Boot runs `pragma_quick_check` before trusting the event log for recovery. The `events.event_num` column is unique, and replay reads events in ascending `event_num` order while checking for sequence gaps.

This matters because PMA-like files are only materializations of state at some accepted event number. The SQLite event log decides what history exists. Operative PMA, snapshots, and checkpoints are boot accelerators or recovery anchors at a particular event boundary.

The `--disable-fsync` option disables these durability claims. It disables application fsync/fdatasync calls and causes SQLite to use `synchronous=OFF`. That mode can be useful for benchmarking or controlled rebuilds, but it is not the crash-recovery mode described here.

### Disaster recovery

On boot, the NockApp checks that the operative PMA sidecar event id matches the latest event id in the `sqlite3` event log database. If they are equal, the operative PMA is eligible for the fast path. If the PMA is ahead of SQLite, behind SQLite, invalid, or missing, boot enters recovery.

Recovery tries sources in order:

- Ready snapshots from SQLite. The snapshot is verified, copied back over the operative PMA slab, and SQLite events after the snapshot event number are replayed.
- Legacy checkpoints. A checkpoint is only usable if it is not ahead of SQLite and replay from the checkpoint event number reaches the SQLite max event number.
- Fresh boot plus event-log replay from zero, if the event log is continuous from event 1.

If no boot base can replay to SQLite's accepted-event boundary, boot fails closed. Replay applies the exact logged job and fails if that job is rejected; replay does not use the live `poke_swap` path to synthesize a different event.

### Snapshots

I had a clever idea while working on snapshotting and it pleased me so much that I am compelled to explain it here. I did not use a fixed duration interval (like GC or checkpointing) or count of events processed. Instead, snapshotting uses _event execution time_ as the trigger for initiating snapshot construction. It's a threshold that represents the sum of time spent executing events since the last snapshot. The appeal of this design is that it enables operators to make a very precise trade-off between time spent snapshotting during ordinary operations against the maximum amount of time you want to spend replaying events from the event log in a DR scenario.

There was some extra effort put into detecting corruption in the snapshots because it's work that doesn't have to get done over-and-over in the course of executing events and because the snapshots are a critical part of our DR (disaster recovery) story for NockVM and NockApps. The `SnapshotManifest` protects itself. `SnapshotManifest` stores magic/version plus PMA shape, event number, root pointers, `used_blake3`, optional `structure_blake3`, and a checksum. The checksum is BLAKE3 over the manifest payload excluding the checksum field, and decode validates it before the manifest is trusted. See `crates/nockapp/src/snapshot.rs:185`.

The snapshot PMA file is protected by `used_blake3`. When creating a snapshot, the code syncs the source PMA, copies it, then hashes the allocated PMA prefix: `pma.alloc_offset()` * 8 bytes. That hash is stored in the manifest and SQLite ready-snapshot row. See `crates/nockapp/src/snapshot.rs:572`. Verification recomputes that same used-range hash before accepting the snapshot. It also checks manifest `pma_words / alloc_words` against the PMA trailer metadata. If the file is truncated inside the used range, read_exact fails; if any byte in the used prefix changed, `UsedHashMismatch` fails verification. See `crates/nockapp/src/snapshot.rs:313` and `crates/nockapp/src/snapshot.rs:819`.

Snapshot creation runs a fast verification pass before inserting the snapshot as ready in SQLite. Snapshot restore runs full verification before copying the snapshot back over the operative PMA. Full verification validates the saved root pointer, validates the cold offset, and traverses the noun graph. The optional `structure_blake3` field can pin a structural hash, but current production snapshot creation stores `None` there; the normal cryptographic integrity check is the used-prefix BLAKE3 hash, with full traversal providing structural sanity checks.

Snapshot kernel-hash semantics intentionally differ from the operative PMA fast path. An operative PMA sidecar with a kernel hash mismatch is rejected. A verified snapshot or checkpoint with an old kernel hash may be loaded into the current kernel with a warning, matching the migration/import semantics already used for checkpoint state.

### PMA GC

PMA GC is enabled by default for application boots, with `--gc-interval` controlling the cadence in seconds. The default application interval is one hour (`3600` seconds). Tests and specialized tooling can still disable it with `--gc-interval none`, `--gc-interval 0`, or by constructing `PmaConfig` with no GC interval.

GC is a discrete phase after the normal event durability path, not a replacement for it. The accepted event is first committed to SQLite, copied into the active PMA, synced, and published in PMA metadata. Only after that durable persistence phase may GC attempt to compact into the inactive slab. If a rotating snapshot is due, it runs after this PMA persistence/GC sequence.

The GC path treats the SQLite event log plus ready snapshots as the recovery authority. It only runs when a ready snapshot recovery anchor exists. Before touching the inactive slab, it removes and syncs the inactive sidecar metadata. After creating the inactive slab, it removes and syncs the active sidecar metadata before destructively reading/copying from the active slab. That deliberate invalidation means a crash during GC should not leave boot believing the active operative PMA is authoritative. Boot must instead recover from a verified snapshot and replay the SQLite event log to the SQLite boundary.

GC copies the durable Arvo root into the alternate slab, installs the alternate PMA arena, then publishes the alternate sidecar metadata last. Derived runtime caches (`hot`, `warm`, `cold`, jet-test HAMT, and the memo cache) are rebuilt on the current stack after the slab switch. They are not copied as durable state, and they are not allowed to keep pointers into the old active slab after GC completes.

GC has a different page-dirtying profile from ordinary event commit. It copies the reachable Arvo DAG from the active slab into the inactive slab. To preserve sharing without an external relocation map, the copier writes forwarding pointers into source objects in the old active slab after copying them. Those in-source forwarding pointers answer "has this source object already been copied, and where did it land?" for later references to the same object. This saves the memory and lookup overhead of a `source offset -> destination offset` relocation table, but it dirties old source pages and makes the source slab invalid as an operative PMA once GC begins. A non-destructive GC would need such an external relocation map or would have to duplicate shared subgraphs.

## Caveats and risks

- It is _not_ safe to load or import raw PMA slabs from third parties. Do _not_ do this unless it's between computers you have control over. Use state jams instead!

- The operative PMA fast path is not snapshot verification. A PMA with sidecar event number equal to SQLite max is accepted after sidecar/trailer/open checks, not after a BLAKE3 scan or noun-graph traversal of the slab. If the threat model includes arbitrary data corruption inside a fully written operative slab, use snapshots/state jams/distributed validation as the recovery boundary.

- Snapshot restore verifies the manifest/PMA pair, but the recovery path currently uses the SQLite snapshot row to choose the replay boundary before restore. The duplicated row fields and manifest fields should stay identical because they are written together during snapshot creation, but this is not a substitute for SQLite integrity. If SQLite itself is corrupted beyond what `quick_check` and schema constraints catch, recovery decisions can be wrong.

- PMA GC depends on having a ready snapshot as the fallback recovery anchor. If no ready snapshot exists when the interval fires, GC is skipped rather than invalidating the only operative PMA. This trades space reclamation for crash-recovery clarity.

- Any operation that touches the whole chain state, such as a new state migration the NockVM kernel `load` poke, could re-hot all of the pages of the entire chain state and cause an unusual RSS spike. At time of writing, we haven't tested this scenario under memory pressure yet.

- Similarly, a fresh peer syncing the chain state from the genesis block could send requests that lead to a fully-synchronized peer temporarily re-hotting cold pages from early chain history, causing a temporary RSS spike.

- The PMA should be able to operate in environments with less than the nominal amount of memory required to address the whole subject, but you should expect that memory pressure will cause more page-churn and thus make events execute more slowly in proportion to the degree of memory pressure it is subjected to.

## Alternative design considered

You can dig it up in one of the old branches on the GitHub repository but I did implement two different PMAs. The storage design that "won" the contest was the bump allocating PMA. The competing design was a b-tree PMA with mark-and-sweep garbage collection. The relevant decision here is that the PMA's storage model is bump allocation plus periodic copy-style compaction into an alternate slab rather than a mutable b-tree heap. There are a few factors that led to choosing the bump PMA:

- Copying GC took ~5 seconds to compact the entire 50k+ block subject, mark-and-sweep GC in the b-tree PMA took closer to 35 seconds.

- b-tree PMA used a little less RAM steady-state by default (~5.8 GiB vs. 7.5 GiB) but it was somewhat more complicated and slightly slower than the bump PMA in terms of executing events. My belief is that the b-tree had lower RSS because it was explicitly attempting to respect page boundaries. The bump PMA doesn't really concern itself with that at all. During copy-style PMA GC, reachable parts of the subject are rewritten to the alternate slab, so surplus page fragmentation/deadweight disappears when the compacted slab becomes operative.

- Making the mark-and-sweep GC faster is definitely possible but would've added a greater drag coefficient to runtime efficiency. The design attempted to harvest low-utilization pages and compact sub-trees into a smaller number of higher utilization pages. This seemed to work fine but 35 seconds is just intolerable.

- The Bump PMA's design and simplicity coheres better with the design principles that informed NockVM and NockStack originally, especially NockStack's implementation of garbage collection.

- I didn't have the wherewithal to deal with large (exceeding the capacity of a single 4K page) atoms in a more intelligent manner in the b-tree PMA. We don't have many examples of this in the NockVM/Nockchain ecosystem but it's my understanding that this is a much more common thing in Urbit applications.

- Copying compaction is extremely fast even for 10 GiB+ subjects and is a lot closer to the "speed of light" than the mark-and-sweep implementation was. It is harder to get it wrong or make it slow, and keeping it as a discrete post-persistence phase gives it a clearer durability boundary than trying to interleave compaction with event execution.

So I went with the bump PMA as the storage model.

## Potential next steps

- Restore slab high watermark metrics for the slab
- Automatically upgrade slab sizes when we hit a configuration utilization threshold, still doubling
- Add the ability to export a state jam in the background safely without having to shut the Serf down first.
- Rationalize PMA GC triggering so it can safely fire faster than the current hourly default when the workload actually needs it.
  - Track PMA append churn since the last GC using the already-measured `pma_arvo_copy` appended word count. This is cheap because event commit already computes the PMA allocation delta. Treat it as an estimate of garbage pressure, not exact truth.
  - Track raw active-slab allocation growth since the last completed GC. This is also cheap and gives a hard upper bound on how much compaction could reclaim, but it cannot distinguish live growth from garbage by itself.
  - Log exact reclaim efficiency after each GC as `from_alloc_words - to_alloc_words`, then use that history to tune churn/growth thresholds. If GC repeatedly reclaims little, back off even if the timer fires.
  - Add a memory-pressure override based on cgroup-aware memory usage. Under Docker or Kubernetes, read cgroup memory current/max rather than host RAM. Use this as a safety signal to page out PMA mappings first and only accelerate GC when PMA churn/growth also indicates real dead space.
  - Keep a minimum wall-clock or event-count spacing between GCs. Copy-style GC is fast, but it dirties source pages, writes a full compacted slab, and temporarily maps both slabs, so it should not chase noisy short-term signals.
- Make a tool and in-system hook for tracing leftover write-backs during event commit. The idea is to record appended PMA word counts, approximate dirty-page spans, and where the new Arvo frontier is allocated across the slab. Goal would be to dump a data representation (JSON?) after executing some number of events and a separate binary that generates a static web page visualizing pages dirtied by writes vs. pages only referenced by PMA offsets.
  - Stretch goal: ditto but cover reads in the NockStack during event execution as well.
  - Stretch goal: ditto but cover GC.
- Add another write-back epicycle that spares write-through to disk without compromising integrity. Basically you write to the PMA without doing any write-back to the PMA .meta's event id for N number of events. If there's a SIGKILL or power cut before the next time you bump it, the event id will get recognized as stale and trigger a DR on boot. You could do this either with maybe `mlock` or by having an in-memory proxy for the event id that doesn't get written to the .meta on each event.
- CDC, building on event log. You'll probably want to figure out delta patch traces for the PMA write routines so that replication doesn't require re-executing the `poke` event though.
- Multi-threaded Nock execution with shared memory (PMA)
- Bytecode VM for NockVM to close the runtime efficiency gap with Vere
