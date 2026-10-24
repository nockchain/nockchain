# B+ Tree page directory for New Mars Persistent Memory Arena

> Historical note. This is exploratory PMA design documentation, not current protocol or runtime authority.

## Problem

We need to be able to dynamically allocate intervals of memory which will be consistently and durably persisted to disk. This requires mapping memory pages to regions of a disk-backed file. Modifying a page requires that the data first be *copied* to a new region so that the original data is not corrupted if the process exits with the new data in an inconsistent state or not fully persisted to disk.

This requires a durable mapping of virtual memory page addresses to offsets in the backing file. One solution is to store a metadata table indexed by the base pointer of the new table. This is simple to implement but has some downsides: it must itself be durable, so there is some complexity in ensuring the information for later pages of the table are stored in earlier pages. The table must grow to accomodate the highest virtual memory address used, and cannot efficiently encode contiguous multi-page mappings.

Further, it is unclear whether mainstream OS kernels can handle the sheer quantity of mappings necessary to keep the entire persistent memory arena always mapped. It may be necessary to limit the number of mappings, using a marking eviction strategy to unmap infrequently-used mappings, and mapping pages on demand by reference to the index. In such a case, we would want to take advantage of contiguous mappings, which are likely to contain related nouns, and map them as one mapping. 

The alternative is to use a B+ tree, *indexed by page base pointers*, and storing disk offsets as values. With such a structure, the page directory is stored in the arena alongside data. There is no need to separately implement dirtying and synchronization for the page directory, it is done by the same implementation as dirtying and synchronizing data. B+ trees naturally encode intervals of keys, which provides a means to store contiguous mappings.

Only tree nodes relative to a traversal must be mapped. This provides an efficient way to handle on-demand mapping of page regions. Since the structure naturally stores contiguous page mappings, such mappings can be supplied to `mmap()`, further reducing pressure on the virtual memory system.

The downside of this approach is added complexity of the datastructure, and a small asymptotic penalty (constants become logarithmic dependencies) in most operations.

## Implementation details

The page directory maps pages of virtual memory (identified by their base pointers) to page-sized intervals (hereafter PSIs) in a file. It must be persisted in the file itself for durability of the data. Corruption of the page directory renders the data unrecoverable, and so it is subject to the same durability constraints as the actual data.

Nodes of a B+ tree contain an array of N entries of values in the keyspace, and N+1 leaves or references to child nodes. 

A reference to a child page must contain an offset into the file where the page is stored. Modern processors and operating systems allow, in practice, 48-bit addresses, though pointers are represented as 64-bit integers. Pages are generally 4 kilobytes or 2^12 bytes in size. So a byte-addressed offset (pointer or file offset) truncated to address the base of a page is 48-12=36 bits, or 4.5 bytes. We expand this to 5 bytes for ease of addressing and to use the extra bits as flags. Thus, a 4KB page can store 409 indices and 410 child references. This requires a maximum tree depth of 6 to store entries for every page in an approximately 85TB arena. Each leaf page entry occupies 5 bytes, for the disk offset. Node pages are mapped into the memory at the start of the interval they represent, thus the leftmost interval is advanced by 1 page for the leftmost child of a node.

There are of course internal pages, but since each page shares its parent pointers with 455 others the amortized contribution is negligible. So the overhead of the page table is a fraction of a percent of the total storage space of the arena.

Since a B+ tree naturally stores *intervals*, a leaf with a file offset denotes a run of pages contiguous in both virtual memory and on-disk storage. An unmapped interval is denoted by a 0 file offset, since the first two PSIs in the file are the double-buffered entry point into the file.


Modifying a page requires that the page first be copied to a new disk location (dirtied) as would be done for a copy-on-write of a data page. This will involve altering its parent node, which will need to be dirtied, and so on up to a node that is already dirty, or to the root, which is copied over whichever of the first two PSIs on disk is not the most current.

When syncing a snapshot to disk, the B+ tree is traversed (and a stack maintained), descending only into nodes for which the dirty bit is set. Dirty pages are `msync()`ed first and the dirty bits of their table entries cleared on a successful synchronization.

Once dirty pages referenced by a node are synced, the node itself can be synced, and so on back up to the root node. Once the root node itself is successfully synced the snapshot can be considered durable.

## Allocation

Nouns which are to be stored in the PMA come from the 2stackz arena. The copier first scans the noun to determine the memory size necessary in the PMA, and then calls `pma_malloc()` with this size. The noun is then recursively copied into the allocated buffer.

Allocation must find both a suitably sized region of virtual memory, and a suitably sized free region in the backing block store. The allocator should maintain an ephemeral cache of free space on the disk, which can be inferred on process start from the on-disk B+ tree. If no suitable region is available, then the file is expanded. Freeing regions at the end of the file should contract the file.

**TODO** description of how allocation works, in particular less-than-page and multipage allocations.

## Collection

A collection of roots are stored in the PMA's metadata. Before snapshotting, these roots are supplied to a mark-and-sweep garbage collector, which frees all allocations not marked in a trace from the root. Freed pages have their associated PSIs on disk returned to the free set of PSIs.
