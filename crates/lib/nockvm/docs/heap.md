# Heaps and persistence

> Historical note. This is archived heap/persistence design context, not current protocol or runtime authority.

However, there are several reasons why we may wish to copy a noun off the stack and reference it during a computation.
Copying a large noun of the stack and maintaining a reference instead prevents repeated copying up the stack.
Fixing a location for a noun enables pointer equality to short-circuit equality checks.
Snapshotting and paging requires a fixed location in memory for a noun, unperturbed by computation.
Finally, the ability to attach metadata (hints, compiled code, reference counts, etc) to nouns is extremely helpful for runtime implementation, but costly if provided for every noun.
Instead, we can store nouns in a coarsely-allocated heap which attaches metadata to the allocation entry for each noun, rather than for each cell.

## Memory arenas
### Large objects
To prevent repeated copying of large objects, a temporary older-generation heap may suffice.
This is a heap used alongside a Nock stack (see [stack.md](stack.md)) to which a noun may be copied instead of copying it up to the calling frame.

Each allocation entry records the location of the most senior pointer to the entry, and if that frame is popped or object freed without a copy of the pointer, the object can be freed.
Thus, we can use a traditional alloc/free approach with a free list for allocations, while not losing automatic and predictable memory management.

#### Aside: unifying equality
The current vere (u3) implements unifying equality, meaning that when two nouns are discovered to be equal, a reference to one is replaced with a reference to the other.
This is not required by the Nock specification, which demands only structural equality, but is an obvious and nearly costless optimization to make to structural equality.

However, we must be careful not to undo invariants that our memory system depends on. For the stack, this means that when replacing a reference to a noun with a structurally equal one,
we must take care to replace the reference to more junior (more recently pushed) stack frames with references to more senior stack frames. We must also take care not to introduce
references from any heap back to the stack, thus any heap must be treated as more senior (and thus chosen for unifying equality) than any stack frame.

### Persistence and paging
The canonical persistence layer for any Urbit ship is the event log.
However, exclusive reliance on the event log requires us to permit the event log to grow without bound and to replay a ship's entire history every time it is restarted.
Fast restarts and event log pruning both rely on snapshots, which are persistent copies of the current Arvo state.

Further, all data stored on an Urbit ship and accessible to it is present as nouns in the Arvo state.
Unless we wish to limit Urbit forever to persisting only as much data as can reasonably be stored in RAM, we need some mechanism for storing large parts of the Arvo state on disk, and loading them into memory only when needed.

Nouns which need to be persisted, or which require metadata, are "canonicalized."
That is, they are assigned a memory address which is permanent for the lifetime of the noun.
Once this is done, the noun can be persisted on disk, and the physical memory freed for other uses.
The *virtual memory* space remains reserved for the noun at least as long as the noun is still reachable from the set of roots (see below).

The persistent heap maintains a queue of which persisted nouns were recently touched, and evicts the pages for least recently used nouns (likely using `madvise()` with the `MADV_DONTNEED` advice on Linux) when the resident set size is too large.
User-space page fault handling (using `userfaultfd` on Linux or equivalent page fault trapping on other systems) is used to detect subsequent reads in a noun's virtual memory space and page the noun back in.

The persistent heap also maintains a metadata record adjacent to each noun, which allows for:
- Reference counting, to ensure we can predictably drop nouns which are no longer referenced by roots.
- Code caching, for nouns representing Nock formulas
- Any other data helpful to the implementation.

## Snapshotting
Snapshotting makes use of the current arena, storing a root for each snapshot. The snapshotting process ensures all nouns reachable from the snapshot synced to disk.

## Event logging
TODO: Can we also use persistent noun storage for the event log?

(TODO: strategy for maintaining use times which doesn't require faulting on every access)

## Roots
### Always
- Any undiscarded snapshot
- Any unpruned event log entry (if using noun persistence for event loggin)

### During running computations
- Any result or subject register
- Any slot in a Nock stack
