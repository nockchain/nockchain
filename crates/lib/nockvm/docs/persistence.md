# New Mars persistence

Status: Historical

*Trust posture: this document is preserved for design context. It is not a statement of current nockvm runtime behavior.*

## Current Reality Check (2026-02-19)

- The current `rust/nockvm` runtime memory allocator is in-process stack memory (`MmapMut::map_anon` / malloc paths in `rust/nockvm/src/mem.rs`), not a file-backed PMA.
- The file-backed PMA path described below is not present in the current crate; code comments note PMA removal in favor of checkpointing (`rust/nockvm/src/hamt.rs`).
- The remainder of this document uses historical present tense and should be read as a proposal archive.

## Historical Rationale

New Mars requires a persistence layer. Nouns which are allocated in the top frame of a 2stackz memory manager can only be freed by resetting the stack. This loses all nouns. Therefore, for nouns which persist between computations, another arena is needed.

These are also the nouns which should be included in a snapshot. Therefore, this arena ought to exist in memory mapped to persistent storage. This also represents a solution to the loom size limit: since memory is block-storage-backed, we can grow the Arvo state much larger than physical RAM or system swap space permit, and rely on virtual memory to make pages resident when we read them.

Since this memory will never be used to directly allocate a noun, we do not need the speed of bump-allocating. We do require stable addressing along with fragmentation resistance. Therefore, we will take design cues from `malloc` implementations, and maintain information about free spaces in our memory arena. For nouns, freeing will be triggered by tracing or reference counting from a set of roots.

This implies that any noun used as an input to a computation run in a stack arena must be maintained as a root until that computation completes. For instance: an Arvo core stored in persistent memory is used as input to an event computation. This core is maintained as a root in the persistent arena until the computation has completed.

## Copy-on-Write File-Backed Memory Arena (proposal)

As the [Varnish](https://varnish-cache.org/docs/trunk/phk/notes.html) notes point out: a userspace program ought not to go to great lengths to juggle disk vs memory residency for persistent data. This is something the (terran) operating system already puts large amounts of work into. It is not necessary to work very hard to manage eviction and restoration of immutable data. At the greatest extent of effort, judicious use of `madvise` calls can reduce block storage wait times.

Every page in our memory-backed arena is mapped to a region of a file. This ensures that the operating system can evict the page at will, without need for swap space. The file contains an index which is read when the process starts, describing which regions should be mapped to which virtual memory addresses. This permits us to persist data without serialization or deserialization. The file stores a memory image of the persistent arena.

What is more difficult is to ensure that data *written* to the arena is fully synchronized and that the persistent index between file and memory locations do not point to corrupted data. This is addressed with a "copy-on-write" strategy, similar but not identical to that of [LMDB](http://www.lmdb.tech/media/20120829-LinuxCon-MDB-txt.pdf).

The copy-on-write strategy ensures that commmitted data is not corrupted. Since it is not possible to "atomically" write data to disk, committed data is not overwritten, but rather copied and modified. Committing then progressively ensures that this new data is durably written to disk, and that the index of disk regions to memory regions is durably updated, before it will permit the previous copy to be overwritten.

### Handling writes to the arena

There are several ways in which we can handle writes without overwriting, and possibly corrupting, data already committed to our store.

By default, pages in the persistent memory arena are protected read-only (`PROT_READ` flag to `mmap()` or `mprotect()`). This means that an attempt to write through a pointer into such a page will trigger a protection fault, raising a SIGSEGV to our process.

#### Immediate copying

The most likely successful approach involves immediately copying the page to a new page, mapped to a new region of the file, and then mapping this page back over the same virtual memory space.

To handle the SIGSEGV signal and resolve the fault, our handler will:

- move the page to the copy-on-write set, so other threads faulting will not also attempt to copy the page.
- `write()` the contents of the page to an unused region of the backing block storage. Note that this will not block on writing through to the disk, though it may write through if the OS decides.
- `mmap()` the new region to the address of the original page, being sure to set `MAP_SHARED`, `PROT_READ`, and `PROT_WRITE`
- Move the page from the copy-on-write set to the dirty set.

The net effect of this is that the file region to which the read-protected page was mapped will remain unmodified, but our process continues with identical contents in a virtual memory page mapped to a new region of the file. The process continues from the faulting instruction and writes to memory. Index metadata which maps regions of the file to regions of memory remains untouched at this juncture.

To facilitate restoring the persistent memory arena from durable block storage, we reserve two pages at the beginning of the file for metadata storage. This does not limit us to a page for metadata, we may commit metadata objects alongside our stored data, and reference it from a page. A metadata page contains a counter and a checksum of the page's contents. The page with a valid checksum and the more recent counter is used to load the data.

To atomically commit writes since the last commit to durable storage, we simply `msync()` each dirty page. If all syncs succeed, we know that data in these pages is durably persisted to disk. We may now safely write a new metadata page to whichever metadata page is not in use. Once this too successfully `msync`s, our writes are durably committed.

#### Lazy copying

Another possible approach which leverages kernel support for in-memory copy-on-write, at the expense of using a boundable amount of swap, is to make writable mappings `MAP_PRIVATE`. This instructs the kernel to copy the page in the page cache on the first write. Unfortunately, this is now an anonymous mapping, meaning it is not backed by file storage.
Thus, if the kernel elects to evict it, it will be written to swap space on disk. It is still necessary to map pages as `PROT_READ` and use a signal handler, because we must track which pages are dirtied. It is almost certainly necessary to set a configurable limit on the number of dirty pages before a snapshot will be committed, since the number of dirty pages is limited by the available RAM and swap.

To commit, we select regions in the file from the free list, and `write()` each dirty page to a free region.
We must then achieve a successful `fsync()` to know that the file has been committed.

### When to commit

Committing is required to know that an event is durably persisted in the log. It is also required to save a snapshot to disk.

### Memory allocation

Allocation of memory space for persisted nouns, and for helper structures for the runtime, follows largely the algorithm from [phkmalloc](https://papers.freebsd.org/1998/phk-malloc.files/phk-malloc-paper.pdf).
Briefly:

- Pages are tracked as free, start, continue, or partitioned
- Small allocations seek space in a partitioned page, and only partition a page if no suitable space is found
- Large allocations (> 1/2 a page) are allocated sufficient contiguous free pages

One difference is that since all of our persistent memory is file-backed, the acquisition of new memory consists in mapping new, initially-dirty pages, backed by unused regions of block storage, instead of using the `brk` syscall.

### Block region allocation

As well as allocating memory to noun (fragments) which need to be copied into the persistent set, we must also allocate regions of block storage to memory pages. Fortunately this is much easier. All pages are the same size. When committing writes, we can take note of file regions which are not mapped by the new metadata, and prepopulate a free list of file regions with these regions. The list becomes usable once the commit succeeds. We simply pop regions off of the list, starting with those nearest the beginning of the file. until it is exhausted, at which point we must extend the file.

It's not clear whether it would be necessary to shrink the backing file, but if so, this could initially be done manually with a separate program, or a heuristic could be applied. Perhaps when a certain ratio of free-to-used pages are run, a defragmentation step is run, followed by shrinking the file.

### Snapshots

After each event is processed, the working state is copied to the snapshot persistent arena to be used as input for the next event. Creating a durable snapshot consists of committing, and recording a pair of event number and noun pointer in a durable metadata record. Metadata is referenced from the root page of a commit, and includes the event number of the snapshot and a pointer to the snapshotted noun.

### Event log

The event log should live in a separate arena from Arvo snapshot nouns. These arenas should be of incomparable seniority: that is, references are not permitted between either of them. This precludes synchronization issues between snapshot and event log commits, and ensures that old epochs of the event log can be freed without corrupting a snapshot.

This requires logic in the copier to ensure that a reference is only preserved as a reference if the seniority of the source is ≤ the seniority of the destination.

Epochs in the event log can be stored as linked lists in the event log persistent arena. In order to safely release effects computed from incoming events, the events must be durably written to disk.

Allocations in this arena may be made by bumping a pointer within pages. Dirtying a page does not necessitate copying as long as the pointer to the most recent event can be stored consistently, and is updated *after* an event is made durable on disk. This eliminates repeated copies of a page to re-dirty it after a commit. The only time when a free block of storage must be allocated to a page is when a new page is mapped. If each epoch has its own page set, then when an epoch is discarded, pages may simply be unmapped together and their blocks re-added to the free list.

The separate arena need not be maintained in an entirely separate memory region. Rather, on disk, two indexes each with two root pages (which are alternated for commits) can be maintained, and the allocator can track which pages belong to which arena, and inform the persistence subsystem of this when requesting free pages, so that the persistence subsystem knows which index should track a newly dirtied page.

### Concurrency

Because nouns are immutable, and because the copy-on-write approach ensures that a new, dirty page is a copy of an existing clean page prior to mapping it, readers should never see an inconsistent state of an object so long as that object is traceable from a root.

It is not necessary to support multiple writers to the arena. Event (`+poke`) computation is linearized: we cannot start computing the next event until the result of the previous event is copied to the arena. Computation of scries (`+peek`) may run concurrently, but will only read from the arena unless new code generation is necessary.

Thus, it should suffice to maintain a single, global writer lock for an arena.
Contention for this lock is not likely to be high, and this will eliminate a great deal of complexity which would obtain if page dirtying and commits required more granular synchronization.

## Tuning

Commit frequency of the Arvo snapshot arena is a potentially tunable value, with performance implications.
Committing more often incurs more disk IO, saving nouns that will potentially soon be discarded (Ames queues, for instance). However, under memory pressure, it makes paging more performant, as the proportion of dirty pages which must be *written* to disk to evict is smaller. Already-committed pages can simply be evicted from memory and re-read from disk when a read fault occurs. Committing less often requires writes as part of eviction, but reduces disk IO. It is desirable as long as memory pressure as low, and highly desirable for block storage where write-wearing is a concern.
