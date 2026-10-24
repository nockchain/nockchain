# New Mars: Memory layout and representation

Status: Historical

*Trust posture: this document is useful implementation context, but not canonical architecture policy.*

This document captures a 2022-era model of New Mars memory layout and noun representation.

## Noun representation

Nouns are represented as machine words with the MSBs as tag bits.
By treating a 0 MSB as the tag for a direct atom, we can compute directly with direct atoms without masking the tag.


| MSBs | Noun                  |
| ---- | --------------------- |
| 0    | Direct Atom           |
| 10   | Indirect Atom Pointer |
| 110  | Cell Pointer          |


A direct atom is an atom which fits in a machine word, less one bit for the tag. It is stored directly.

An indirect atom is an atom which is too big to be a direct atom. It is thus represented as a tagged
pointer. The memory referenced is 64-bit aligned. The first 64 bits are metadata, followed by the 
size in 64-bit words, then the actual value of the atom, whose number of words
is equal to the size in little-endian order. The metadata field is primarily used for
the mug. The first three bits of the size are reserved for a possible forwarding pointer tag.

A cell is represented as a tagged pointer. The memory referenced is adjacent machine words.
The machine word at the pointer is metadata. The machine word directly following (higher in memory)
is the noun representation of the head, followed by the noun representation of the tail. The metadata
field is primarily used for the mug.

During collection the memory for indirect atoms and cells may be rewritten to indicate a _forwarding pointer_.
When an indirect atom is copied from one stack frame to another, its size is replaced with a
forwarding pointer. When a cell is copied from one stack frame to another, its
head is replaced with a forwarding pointer.

The tag for a forwarding pointer is 111.

## Memory Layout

Computation proceeds in a memory arena, using two stacks growing towards each other from opposite ends.
The stack growing from lower memory to higher is termed the "west" stack and lower memory is directionally "west."
The stack growing from higher memory to lower is termed the "east" stack and, similarly, higher memory is directionally "east."

Computational frames are always pushed onto the opposing stack from the current frame.
Thus the two stacks logically form one single computational stack.
The stack on which the current frame is located is termed the "polarity".
Thus, the current stack frame may be said to have "west" or "east" polarity.

Three pointers are tracked:
the frame pointer, stack pointer, and allocation pointer.

The "frame pointer" is changed when pushing/popping a stack frame.
When the current polarity is west, it points to the westernmost byte in the current frame.
Slots relative to the frame (for register saves or return addresses) are addressed by offsetting eastward.
When the current polarity is east, it points to the easternmost byte _west of_ the current frame.
Slots relative to the frame are in this case addressed by offsetting westward.
Note that we do not reverse which "end" of a machine word we address by the polarity.

The "stack pointer" is changed when pushing a new stack frame and when using the lightweight stack.
When the current polarity is west, it points to the westernmost byte east of the current frame.
When the current polarity is east, it points to the westernmost byte *of* the current frame.

The "allocation pointer" is changed to allocate memory inside the current frame.
When pushing a new frame, polarity reverses and the frame reserves three words for previous frame/stack/allocation pointers (see `RESERVED = 3` in `rust/nockvm/src/mem.rs`).
This provides both a return mechanism (restore frame, stack, and allocation pointers) and polarity-swapping frame push behavior.

Allocation is achieved by moving the allocation pointer toward the stack pointer and returning the allocation start (west/east behavior is mirrored by polarity).

When popping a frame, the result register is examined to provide a collection root.
Pointers which fall within the current frame are recursively chased and copied to the parent frames allocation area, bumping the saved stack pointer as this progresses. This ensures that data allocated in this frame (or a child frame and then copied here) which is live from the returned result is not lost when the frame is popped.

The collector makes use of memory just beyond the returning frame as an ephemeral stack, containing noun words to
be copied and a destination to copy them to.
Copying a cell results in a new cell allocation on the parent memory, and pushing both constituent nouns onto the
stack with the head and tail of the cell as destinations.
Copying a direct atom is just a matter of writing it to the destination.
Copying an indirect atom involves an allocation, copying of the atom's memory, and writing the new allocation to the saved destination.

Frames to which a call may return should reserve the first local frame slot (after the three reserved frame/stack/allocation pointer words) for the return address.
_Note that this makes it the ***caller's*** responsibility to push a new frame for a non-tail call._

