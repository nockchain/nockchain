# Zero-cost Memory Moves in the New Mars PMA

> Historical note. This document captures exploratory PMA memory-move ideas, not current protocol or runtime authority.

The New Mars Persistent Memory Arena (PMA) maintains a file-backed memory arena where pages to be mutated are copied to new blocks in the file prior to mutation. Both reads and writes are memory reads and writes: data is written to memory in the standard way and is only written to disk as part of a page eviction or when `msync()` is called to ensure the page is durable.

One possible use of this mechanism is to operate a "2stackz" allocation and control stack within an otherwised unused region of virtual memory within the PMA. Stack push, stack pop, and allocate operations use the lower level operations of the PMA to allocate disk regions for pages, map these regions, and store the mapping in a persistent page directory. None of these operations entails any synchronous write to disk. 

The allocator design repurposed for the permanent heap area of the PMA, phkmalloc, allocates a new page or pages for allocation requests of more than one half of the size of a page. The 2stackz allocation algorithm could easily be extended either to page-align such allocations, or to note them during copying and copy them to a page-aligned boundary in the stack frame. A page-aligned object can be moved in virtual memory with no copying overhead simply by re-mapping the disk region to new pages of virtual memory, and updating the page index to correspond.

Pages are unmapped when the stack is popped across their boundary, and deleted from the page directory when unmapped. Since snapshots are taken between event processing, not during, ephemeral pages for the stack will not be synchronously written to disk. In fact, they will only be placed on disk at all if the virtual memory system must eject the page, and the disk region will be repurposed for another page since the mapping will be deleted from the page directory and the disk region added to the cache of free disk pages.

(An optimization is to continue to map pages when the extent of the stack requires it, but to unmap stack pages as a batch when the computation on the stack has completed.)

If the 2stackz copier can take advantage of this system to move large objects in virtual memory space without needing to write or copy them to disk, or to copy them in physical memory, it will remove a great deal of the overhead associated with memory management. Large atoms could be handled without repeated copying overheads, regardless of size. Further, moving large atoms to the permanent heap would also involve only remapping disk (and indirectly, physical memory) to the proper heap location. Snapshots will then synchronize the large atom data to disk as with any other noun.
