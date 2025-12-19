// TODO: fix stack push in PC
use std::alloc::Layout;
use std::ops::{Deref, DerefMut};
use std::panic::panic_any;
use std::ptr::copy_nonoverlapping;
use std::vec::Vec;
use std::{mem, ptr};

use either::Either::{self, Left, Right};
use ibig::Stack;
use memmap2::MmapMut;
use thiserror::Error;

use crate::noun::{Atom, Cell, CellMemory, IndirectAtom, Noun, NounAllocator};
use crate::{assert_acyclic, assert_no_forwarding_pointers, assert_no_junior_pointers};

crate::gdb!();

/** Number of reserved slots for alloc_pointer and frame_pointer in each frame */
pub(crate) const RESERVED: usize = 3;

/** Word offsets for alloc and frame pointers  */
pub(crate) const FRAME: usize = 0;
pub(crate) const STACK: usize = 1;
pub(crate) const ALLOC: usize = 2;

/**  Utility function to get size in words */
#[inline]
pub(crate) const fn word_size_of<T>() -> usize {
    (mem::size_of::<T>() + 7) >> 3
}

/** Utility function to compute the raw memory usage of an [IndirectAtom] */
fn indirect_raw_size(atom: IndirectAtom) -> usize {
    debug_assert!(atom.size() > 0);
    atom.size() + 2
}

#[derive(Debug, Clone)]
pub struct MemoryState {
    pub intended_alloc_words: Option<usize>,
    pub frame_offset: usize,
    pub stack_offset: usize,
    pub alloc_offset: usize,
    pub prev_stack_pointer: usize,
    // pub(crate) prev_frame_pointer: usize,
    pub prev_alloc_pointer: usize,
    pub pc: bool,
}

/// Error type for when a potential allocation would cause an OOM error
#[derive(Debug, Clone)]
pub struct OutOfMemoryError(pub MemoryState, pub Allocation);

/// Error type for allocation errors in [NockStack]
#[derive(Debug, Clone, Error)]
pub enum AllocationError {
    #[error("Out of memory: {0:?}")]
    OutOfMemory(OutOfMemoryError),
    #[error("Cannot allocate in copy phase: {0:?}")]
    CannotAllocateInPreCopy(MemoryState),
    // No slots being available is always a programming error, just panic.
    // #[error("No slots available")]
    // NoSlotsAvailable,
}

impl From<AllocationError> for std::io::Error {
    fn from(_e: AllocationError) -> std::io::Error {
        std::io::ErrorKind::OutOfMemory.into()
    }
}

#[derive(Debug, Error)]
pub enum NewStackError {
    #[error("stack too small")]
    StackTooSmall,
    #[error("Failed to map memory for stack: {0}")]
    MmapFailed(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy)]
pub enum ArenaOrientation {
    /// stack_pointer < alloc_pointer
    /// stack_pointer increases on push
    /// frame_pointer increases on push
    /// alloc_pointer decreases on alloc
    West,
    /// stack_pointer > alloc_pointer
    /// stack_pointer decreases on push
    /// frame_pointer decreases on push
    /// alloc_pointer increases on alloc
    East,
}

#[derive(Debug, Clone, Copy)]
pub enum AllocationType {
    /// alloc pointer moves
    Alloc,
    /// stack pointer moves
    Push,
    /// On a frame push, the frame pointer becomes the current_alloc_pointer (+/- words),
    /// the stack pointer is set to the value of the new frame pointer, and the alloc pointer
    /// is set to the pre-frame-push stack pointer.
    FramePush,
    /// To check for a valid slot_pointer you need to check the space between frame pointer
    /// and previous alloc pointer and then subtract RESERVED
    SlotPointer,
    /// Allocate in the previous stack frame
    AllocPreviousFrame,
    /// Flip top frame
    FlipTopFrame,
}

impl AllocationType {
    pub(crate) fn is_alloc_previous_frame(&self) -> bool {
        matches!(self, AllocationType::AllocPreviousFrame)
    }

    pub(crate) fn is_push(&self) -> bool {
        matches!(self, AllocationType::Push)
    }

    pub(crate) fn is_flip_top_frame(&self) -> bool {
        matches!(self, AllocationType::FlipTopFrame)
    }

    pub(crate) fn allowed_when_pc(&self) -> bool {
        self.is_alloc_previous_frame() || self.is_push() || self.is_flip_top_frame()
    }
}

// unsafe {
//     self.frame_pointer = if self.is_west() {
//         current_alloc_pointer.sub(words)
//     } else {
//         current_alloc_pointer.add(words)
//     };
//     self.alloc_pointer = current_stack_pointer;
//     self.stack_pointer = self.frame_pointer;
//     *(self.slot_pointer(FRAME)) = current_frame_pointer as u64;
//     *(self.slot_pointer(STACK)) = current_stack_pointer as u64;
//     *(self.slot_pointer(ALLOC)) = current_alloc_pointer as u64;

/// Non-size parameters for validating an allocation
#[derive(Debug, Clone)]
pub struct Allocation {
    pub orientation: ArenaOrientation,
    pub alloc_type: AllocationType,
    pub pc: bool,
}

#[derive(Debug, Clone)]
pub enum Direction {
    Increasing,
    Decreasing,
    IncreasingDeref,
}

pub enum AllocType {
    Mmap,
    Malloc,
}

pub enum Memory {
    Mmap(MmapMut),
    Malloc(*mut u8, usize),
}

impl Deref for Memory {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        match self {
            Memory::Mmap(mmap) => mmap.deref(),
            Memory::Malloc(ptr, size) => unsafe { core::slice::from_raw_parts(*ptr, *size) },
        }
    }
}

impl DerefMut for Memory {
    #[inline]
    fn deref_mut(&mut self) -> &mut [u8] {
        match self {
            Memory::Mmap(mmap) => mmap.deref_mut(),
            Memory::Malloc(ptr, size) => unsafe { core::slice::from_raw_parts_mut(*ptr, *size) },
        }
    }
}

impl AsRef<[u8]> for Memory {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.deref()
    }
}

impl AsMut<[u8]> for Memory {
    #[inline]
    fn as_mut(&mut self) -> &mut [u8] {
        self.deref_mut()
    }
}

impl Memory {
    /// Layout and MmapMut::map_anon take their sizes/lengths in bytes but we speak in terms
    /// of machine words which are u64 for our purposes so we're 8x'ing them with a cutesy shift.
    pub(crate) fn allocate(alloc_type: AllocType, size: usize) -> Result<Self, NewStackError> {
        let memory = match alloc_type {
            AllocType::Mmap => {
                let mmap_mut = MmapMut::map_anon(size << 3)?;
                Self::Mmap(mmap_mut)
            }
            AllocType::Malloc => {
                // Align is in terms of bytes so I'm aligning it to 64-bits / 8 bytes, word size.
                let layout = Layout::from_size_align(size << 3, std::mem::size_of::<u64>())
                    .expect("Invalid layout");
                let alloc = unsafe { std::alloc::alloc(layout) };
                if alloc.is_null() {
                    // std promises that std::alloc::handle_alloc_error will diverge
                    std::alloc::handle_alloc_error(layout);
                }
                Self::Malloc(alloc, size)
            }
        };
        Ok(memory)
    }
}

/// A stack for Nock computation, which supports stack allocation and delimited copying collection
/// for returned nouns
#[allow(dead_code)] // We need the memory field to keep our memory from being unmapped
pub struct NockStack {
    /// The base pointer from the original allocation
    start: *const u64,
    /// The size of the memory region in words
    size: usize,
    /// Offset from base for the current stack frame (in words)
    frame_offset: usize,
    /// Offset from base for the current stack pointer (in words)
    stack_offset: usize,
    /// Offset from base for the current allocation pointer (in words)
    alloc_offset: usize,
    /// The least amount of space between the stack and alloc pointers since last reset
    least_space: usize,
    /// The underlying memory allocation which must be kept alive
    memory: Memory,
    /// Whether or not [`Self::pre_copy()`] has been called on the current stack frame.
    pc: bool,
}

impl NockStack {
    // Helper method to derive a pointer from the base + offset
    #[inline(always)]
    unsafe fn derive_ptr(&self, offset: usize) -> *mut u64 {
        // FIXME: This assert is not valid in the general case, since the offset can be larger than
        // size in certain cases like alloc_pointer, so we need to lift this out of derive_ptr.
        // debug_assert!(
        //     offset < self.size,
        //     "Offset {} out of bounds for size {}",
        //     offset,
        //     self.size
        // );

        // // change the if condition to a debug_assert!
        // debug_assert!(
        //     offset < isize::MAX as usize,
        //     "Offset too large for pointer arithmetic: {} > {}",
        //     offset,
        //     isize::MAX
        // );

        // Safe pointer arithmetic using the strict-provenance API
        (self.start as *mut u64).add(offset)
    }

    // Helper method to get a frame pointer from the current frame offset
    #[inline(always)]
    unsafe fn frame_pointer(&self) -> *mut u64 {
        self.derive_ptr(self.frame_offset)
    }

    // Helper method to get a stack pointer from the current stack offset
    #[inline(always)]
    unsafe fn stack_pointer(&self) -> *mut u64 {
        self.derive_ptr(self.stack_offset)
    }

    // Helper method to get an alloc pointer from the current alloc offset
    #[inline(always)]
    unsafe fn alloc_pointer(&self) -> *mut u64 {
        self.derive_ptr(self.alloc_offset)
    }

    /**  Initialization:
     * The initial frame is a west frame. When the stack is initialized, a number of slots is given.
     * We add three extra slots to store the “previous” frame, stack, and allocation pointer. For the
     * initial frame, the previous allocation pointer is set to the beginning (low boundary) of the
     * arena, the previous frame pointer is set to NULL, and the previous stack pointer is set to NULL
     * size is in 64-bit (i.e. 8-byte) words.
     * top_slots is how many slots to allocate to the top stack frame.
     */
    pub fn new(size: usize, top_slots: usize) -> NockStack {
        let result = Self::new_(size, top_slots);
        match result {
            Ok((stack, _)) => stack,
            Err(e) => std::panic::panic_any(e),
        }
    }

    pub fn new_(size: usize, top_slots: usize) -> Result<(NockStack, usize), NewStackError> {
        if top_slots + RESERVED > size {
            return Err(NewStackError::StackTooSmall);
        }
        let free = size - (top_slots + RESERVED);
        #[cfg(feature = "mmap")]
        let mut memory = Memory::allocate(AllocType::Mmap, size)?;
        #[cfg(feature = "malloc")]
        let mut memory = Memory::allocate(AllocType::Malloc, size)?;
        let start = memory.as_mut_ptr() as *mut u64;

        // Here, frame_offset < alloc_offset, so the initial frame is West
        let frame_offset = RESERVED + top_slots;
        let stack_offset = frame_offset;
        // FIXME: This was alloc_offset = size; why?
        let alloc_offset = size;
        let least_space = alloc_offset
            .checked_sub(stack_offset)
            .expect("Stack too small to create");

        unsafe {
            // Store previous frame/stack/alloc info in reserved slots
            let prev_frame_slot = frame_offset - (FRAME + 1);
            let prev_stack_slot = frame_offset - (STACK + 1);
            let prev_alloc_slot = frame_offset - (ALLOC + 1);

            *(start.add(prev_frame_slot)) = ptr::null::<u64>() as u64; // "frame pointer" from "previous" frame
            *(start.add(prev_stack_slot)) = ptr::null::<u64>() as u64; // "stack pointer" from "previous" frame
            *(start.add(prev_alloc_slot)) = start as u64; // "alloc pointer" from "previous" frame
        };

        assert_eq!(alloc_offset - stack_offset, free);
        Ok((
            NockStack {
                start: start as *const u64,
                size,
                frame_offset,
                stack_offset,
                alloc_offset,
                least_space,
                memory,
                pc: false,
            },
            free,
        ))
    }

    fn memory_state(&self, words: Option<usize>) -> MemoryState {
        unsafe {
            MemoryState {
                intended_alloc_words: words,
                frame_offset: self.frame_offset,
                stack_offset: self.stack_offset,
                alloc_offset: self.alloc_offset,
                prev_stack_pointer: *self.prev_stack_pointer_pointer() as usize,
                // prev_frame_pointer: *self.prev_frame_pointer_pointer() as usize,
                prev_alloc_pointer: *self.prev_alloc_pointer_pointer() as usize,
                pc: self.pc,
            }
        }
    }

    fn cannot_alloc_in_pc(&self, size: Option<usize>) -> AllocationError {
        AllocationError::CannotAllocateInPreCopy(self.memory_state(size))
    }

    fn out_of_memory(&self, alloc: Allocation, words: Option<usize>) -> AllocationError {
        AllocationError::OutOfMemory(OutOfMemoryError(self.memory_state(words), alloc))
    }

    pub(crate) fn get_alloc_config(&self, alloc_type: AllocationType) -> Allocation {
        Allocation {
            orientation: if self.is_west() {
                ArenaOrientation::West
            } else {
                ArenaOrientation::East
            },
            alloc_type,
            pc: self.pc,
        }
    }

    // When frame_pointer < alloc_pointer, the frame is West
    // West frame layout:
    // - start
    // - *prev_alloc_ptr
    // - frame_pointer
    // - stack_pointer
    // - (middle)
    // - alloc_pointer
    // - *prev_stack_ptr
    // - *prev_frame_ptr
    // - end
    // East frame layout:
    // - start
    // - *prev_frame_ptr
    // - *prev_stack_ptr
    // - alloc_pointer
    // - (middle)
    // - stack_pointer
    // - frame_pointer
    // - *prev_alloc_ptr
    // - end
    // sometimes the stack pointer is moving, sometimes the alloc pointer is moving
    // if you're allocating you're just bumping the alloc pointer
    // pushing a frame is more complicated
    // it's fine to cross the middle of the stack, it's not fine for them to cross each other
    // push vs. frame_push
    // push_east/push_west use prev_alloc_pointer_pointer instead of alloc_pointer when self.pc is true
    // Species of allocation: alloc, push, frame_push
    // Size modifiers: raw, indirect, struct, layout
    // Directionality parameters: (East/West), (Stack/Alloc), (pc: true/false)
    // Types of size: word (words: usize)
    /// Check if an allocation or pointer retrieval indicates an invalid request or an invalid state
    pub(crate) fn alloc_would_oom_(&self, alloc: Allocation, words: usize) {
        // When the fast path is enabled, make the parameters count as used to avoid
        // unused-variable warnings while still short-circuiting.
        #[cfg(feature = "no_check_oom")]
        {
            let _ = (&alloc, words);
        }

        if cfg!(feature = "no_check_oom") {
            return;
        }
        let _memory_state = self.memory_state(Some(words));
        if self.pc && !alloc.alloc_type.allowed_when_pc() {
            panic_any(self.cannot_alloc_in_pc(Some(words)));
        }

        // Convert words to byte count (for compatibility with old code)
        let _bytes = words * 8;

        // Check space availability based on offsets
        let (target_offset, limit_offset, direction) = match (alloc.alloc_type, alloc.orientation) {
            // West + Alloc, alloc is decreasing
            (AllocationType::Alloc, ArenaOrientation::West) => {
                let start_offset = self.alloc_offset;
                let limit_offset = self.stack_offset;
                let target_offset = if start_offset >= words {
                    start_offset - words
                } else {
                    panic!("Alloc would underflow in West+Alloc");
                };
                (target_offset, limit_offset, Direction::Decreasing)
            }
            // East + Alloc, alloc is increasing
            (AllocationType::Alloc, ArenaOrientation::East) => {
                let start_offset = self.alloc_offset;
                let limit_offset = self.stack_offset;
                let target_offset = start_offset + words;
                (target_offset, limit_offset, Direction::Increasing)
            }
            // West + Push, stack is increasing
            (AllocationType::Push, ArenaOrientation::West) => {
                let start_offset = self.stack_offset;
                let limit_offset = if self.pc {
                    unsafe { self.prev_alloc_offset() }
                } else {
                    self.alloc_offset
                };
                let target_offset = start_offset + words;
                (target_offset, limit_offset, Direction::Increasing)
            }
            // East + Push, stack is decreasing
            (AllocationType::Push, ArenaOrientation::East) => {
                let start_offset = self.stack_offset;
                let limit_offset = if self.pc {
                    unsafe { self.prev_alloc_offset() }
                } else {
                    self.alloc_offset
                };
                let target_offset = if start_offset >= words {
                    start_offset - words
                } else {
                    panic!("Push would underflow in East+Push");
                };
                (target_offset, limit_offset, Direction::Decreasing)
            }
            // West + FramePush, alloc is decreasing
            (AllocationType::FramePush, ArenaOrientation::West) => {
                let start_offset = self.alloc_offset;
                let limit_offset = self.stack_offset;
                let target_offset = if start_offset >= words {
                    start_offset - words
                } else {
                    panic!("FramePush would underflow in West+FramePush");
                };
                (target_offset, limit_offset, Direction::Decreasing)
            }
            // East + FramePush, alloc is increasing
            (AllocationType::FramePush, ArenaOrientation::East) => {
                let start_offset = self.alloc_offset;
                let limit_offset = self.stack_offset;
                let target_offset = start_offset + words;
                (target_offset, limit_offset, Direction::Increasing)
            }
            // West + SlotPointer, polarity is reversed because we're getting the prev pointer
            (AllocationType::SlotPointer, ArenaOrientation::West) => {
                let _slots_available = unsafe {
                    self.slots_available()
                        .expect("No slots available on slot_pointer alloc check")
                };
                let start_offset = self.frame_offset;
                let limit_offset = unsafe { self.prev_alloc_offset() };
                let target_offset = if start_offset > words + 1 {
                    start_offset - words - 1
                } else {
                    panic!("SlotPointer would underflow in West+SlotPointer");
                };
                (target_offset, limit_offset, Direction::Decreasing)
            }
            // East + SlotPointer, polarity is reversed because we're getting the prev pointer
            (AllocationType::SlotPointer, ArenaOrientation::East) => {
                let _slots_available = unsafe {
                    self.slots_available()
                        .expect("No slots available on slot_pointer alloc check")
                };
                let start_offset = self.frame_offset;
                let limit_offset = unsafe { self.prev_alloc_offset() };
                let target_offset = start_offset + words;
                (target_offset, limit_offset, Direction::IncreasingDeref)
            }
            // The alloc previous frame stuff is like doing a normal alloc but start offset is prev alloc and limit offset is stack offset
            // polarity is reversed because we're getting the prev pointer
            (AllocationType::AllocPreviousFrame, ArenaOrientation::West) => {
                let start_offset = unsafe { self.prev_alloc_offset() };
                let limit_offset = self.stack_offset;
                let target_offset = start_offset + words;
                (target_offset, limit_offset, Direction::Increasing)
            }
            // polarity is reversed because we're getting the prev pointer
            (AllocationType::AllocPreviousFrame, ArenaOrientation::East) => {
                let start_offset = unsafe { self.prev_alloc_offset() };
                let limit_offset = self.stack_offset;
                let target_offset = if start_offset >= words {
                    start_offset - words
                } else {
                    panic!("AllocPreviousFrame would underflow in East+AllocPreviousFrame");
                };
                (target_offset, limit_offset, Direction::Decreasing)
            }
            (AllocationType::FlipTopFrame, ArenaOrientation::West) => {
                let start_offset = self.size; // End of the memory region
                let limit_offset = unsafe { self.prev_alloc_offset() };
                let target_offset = if start_offset >= words {
                    start_offset - words
                } else {
                    panic!("FlipTopFrame would underflow in West+FlipTopFrame");
                };
                (target_offset, limit_offset, Direction::Decreasing)
            }
            (AllocationType::FlipTopFrame, ArenaOrientation::East) => {
                let start_offset = 0; // Beginning of the memory region
                let limit_offset = unsafe { self.prev_alloc_offset() };
                let target_offset = start_offset + words;
                (target_offset, limit_offset, Direction::Increasing)
            }
        };
        match direction {
            Direction::Increasing => {
                if target_offset > limit_offset {
                    panic_any(self.out_of_memory(alloc, Some(words)))
                }
            }
            Direction::Decreasing => {
                if target_offset < limit_offset {
                    panic_any(self.out_of_memory(alloc, Some(words)))
                }
            }
            // TODO this check is imprecise and should take into account the size of the pointer!
            Direction::IncreasingDeref => {
                if target_offset >= limit_offset {
                    panic_any(self.out_of_memory(alloc, Some(words)))
                }
            }
        }
    }
    pub(crate) fn alloc_would_oom(&self, alloc_type: AllocationType, words: usize) {
        let alloc = self.get_alloc_config(alloc_type);
        self.alloc_would_oom_(alloc, words)
    }

    /** Resets the NockStack but flipping the top-frame polarity and unsetting PC. Sets the alloc
     * offset to the "previous" alloc offset stored in the top frame to keep things "preserved"
     * from the top frame. This allows us to do a copying GC on the top frame without erroneously
     * "popping" the top frame.
     */
    // Pop analogue, doesn't need OOM check.
    pub unsafe fn flip_top_frame(&mut self, top_slots: usize) {
        // Assert that we are at the top
        assert!((*self.prev_frame_pointer_pointer()).is_null());
        assert!((*self.prev_stack_pointer_pointer()).is_null());

        // Get the previous alloc offset to use for the new frame
        let prev_alloc_ptr = *(self.prev_alloc_pointer_pointer());
        let new_alloc_offset = (prev_alloc_ptr as usize - self.start as usize) / 8;

        if self.is_west() {
            let size = RESERVED + top_slots;
            self.alloc_would_oom_(
                Allocation {
                    orientation: ArenaOrientation::West,
                    alloc_type: AllocationType::FlipTopFrame,
                    pc: self.pc,
                },
                size,
            );

            // new top frame will be east
            let new_frame_offset = self.size - size;
            let new_frame_ptr = self.derive_ptr(new_frame_offset);

            // Set up the pointers for the new frame
            *(new_frame_ptr.add(FRAME)) = ptr::null::<u64>() as u64;
            *(new_frame_ptr.add(STACK)) = ptr::null::<u64>() as u64;
            *(new_frame_ptr.add(ALLOC)) = (self.start as *mut u64).add(self.size) as u64;

            // Update offsets
            self.frame_offset = new_frame_offset;
            self.stack_offset = new_frame_offset;
            self.alloc_offset = new_alloc_offset;
            self.least_space = match new_frame_offset.checked_sub(new_alloc_offset) {
                Some(space) => space,
                None => panic_any(self.out_of_memory(
                    Allocation {
                        orientation: ArenaOrientation::West,
                        alloc_type: AllocationType::FlipTopFrame,
                        pc: self.pc,
                    },
                    Some(size),
                )),
            };
            self.pc = false;

            assert!(!self.is_west());
        } else {
            // new top frame will be west
            let size = RESERVED + top_slots;
            self.alloc_would_oom_(
                Allocation {
                    orientation: ArenaOrientation::East,
                    alloc_type: AllocationType::FlipTopFrame,
                    pc: self.pc,
                },
                size,
            );

            // Set up the new frame offset at the beginning of memory + size
            let new_frame_offset = size;
            let new_frame_ptr = self.derive_ptr(new_frame_offset);

            // Set up the pointers for the new frame (at the west side)
            *(new_frame_ptr.sub(FRAME + 1)) = ptr::null::<u64>() as u64;
            *(new_frame_ptr.sub(STACK + 1)) = ptr::null::<u64>() as u64;
            *(new_frame_ptr.sub(ALLOC + 1)) = self.start as u64;

            // Update offsets
            self.frame_offset = new_frame_offset;
            self.stack_offset = new_frame_offset;
            self.alloc_offset = new_alloc_offset;
            self.least_space = match new_alloc_offset.checked_sub(new_frame_offset) {
                Some(space) => space,
                None => panic_any(self.out_of_memory(
                    Allocation {
                        orientation: ArenaOrientation::East,
                        alloc_type: AllocationType::FlipTopFrame,
                        pc: self.pc,
                    },
                    Some(size),
                )),
            };
            self.pc = false;

            assert!(self.is_west());
        };
    }

    /// Resets the NockStack. The top frame is west as in the initial creation of the NockStack.
    // Doesn't need an OOM check, pop analogue
    pub(crate) fn reset(&mut self, top_slots: usize) {
        // Set offsets for west frame layout
        self.frame_offset = RESERVED + top_slots;
        self.stack_offset = self.frame_offset;
        self.alloc_offset = self.size;
        self.least_space = self
            .alloc_offset
            .checked_sub(self.frame_offset)
            .expect("Resetting a stack too small (should never happen)");
        self.pc = false;

        unsafe {
            // Calculate slot offsets for previous pointers
            let prev_frame_slot = self.frame_offset - (FRAME + 1);
            let prev_stack_slot = self.frame_offset - (STACK + 1);
            let prev_alloc_slot = self.frame_offset - (ALLOC + 1);

            // Store null pointers for previous frame/stack and base pointer for previous alloc
            *(self.derive_ptr(prev_frame_slot)) = ptr::null::<u64>() as u64; // "frame pointer" from "previous" frame
            *(self.derive_ptr(prev_stack_slot)) = ptr::null::<u64>() as u64; // "stack pointer" from "previous" frame
            *(self.derive_ptr(prev_alloc_slot)) = self.start as u64; // "alloc pointer" from "previous" frame

            assert!(self.is_west());
        };
    }

    pub(crate) fn copying(&self) -> bool {
        self.pc
    }

    /** Current frame pointer of this NockStack */
    pub(crate) fn get_frame_pointer(&self) -> *const u64 {
        unsafe { self.frame_pointer() }
    }

    /** Current stack pointer of this NockStack */
    pub(crate) fn get_stack_pointer(&self) -> *const u64 {
        unsafe { self.stack_pointer() }
    }

    /** Current alloc pointer of this NockStack */
    pub(crate) fn get_alloc_pointer(&self) -> *const u64 {
        unsafe { self.alloc_pointer() }
    }

    /** Current frame offset of this NockStack */
    pub(crate) fn get_frame_offset(&self) -> usize {
        self.frame_offset
    }

    /** Current stack offset of this NockStack */
    pub(crate) fn get_stack_offset(&self) -> usize {
        self.stack_offset
    }

    /** Current alloc offset of this NockStack */
    pub(crate) fn get_alloc_offset(&self) -> usize {
        self.alloc_offset
    }

    /** Start of the memory range for this NockStack */
    pub(crate) fn get_start(&self) -> *const u64 {
        self.start
    }

    /** End of the memory range for this NockStack */
    pub(crate) fn get_size(&self) -> usize {
        self.size
    }

    /** Checks if the current stack frame has West polarity */
    #[inline]
    pub(crate) fn is_west(&self) -> bool {
        self.stack_offset < self.alloc_offset
    }

    /** Size **in 64-bit words** of this NockStack */
    pub(crate) fn size(&self) -> usize {
        self.size
    }

    /** Get the low-water-mark for space in this nockstack */
    pub fn least_space(&self) -> usize {
        self.least_space
    }

    /** Check to see if an allocation is in frame */
    #[inline]
    pub(crate) unsafe fn is_in_frame<T>(&self, ptr: *const T) -> bool {
        // Check if the pointer is null
        if ptr.is_null() {
            return false;
        }
        // Calculate the pointer offset from the base in words
        let ptr_u64 = ptr as *const u64;
        // We need to permit alloc here for panic reasons
        debug_assert!(
            ptr_u64 >= self.start,
            "is_in_frame: {} >= {}",
            ptr_u64 as usize,
            self.start as usize,
        );
        debug_assert!(
            ptr_u64 < self.start.add(self.size),
            "is_in_frame: {} < {}",
            ptr_u64 as usize,
            self.start.add(self.size) as usize,
        );

        let ptr_offset = (ptr_u64 as usize - self.start as usize) / 8;

        // Get the previous stack pointer
        let prev_ptr = *self.prev_stack_pointer_pointer();
        let prev_stack_offset = if prev_ptr.is_null() {
            if self.is_west() {
                // For top/west frame with null stack pointer, use the end of memory
                self.size
            } else {
                // For top/east frame with null stack pointer, use the start of memory (offset 0)
                0
            }
        } else {
            // Calculate the offset of the previous stack pointer
            (prev_ptr as usize - self.start as usize) / 8
        };

        // Check if the pointer is within the current frame's allocation arena
        if self.is_west() {
            // For west orientation: alloc_offset <= ptr_offset < prev_stack_offset
            ptr_offset >= self.alloc_offset && ptr_offset < prev_stack_offset
        } else {
            // For east orientation: prev_stack_offset <= ptr_offset < alloc_offset
            ptr_offset >= prev_stack_offset && ptr_offset < self.alloc_offset
        }
    }

    pub(crate) fn div_rem_nonzero(a: usize, b: std::num::NonZeroUsize) -> (usize, usize) {
        (a / b, a % b)
    }

    fn divide_evenly(divisor: usize, quotient: usize) -> usize {
        let non_zero_quotient = std::num::NonZeroUsize::new(quotient)
            .expect("Quotient cannot be zero, cannot divide by zero");
        let (div, rem) = Self::div_rem_nonzero(divisor, non_zero_quotient);
        assert!(rem == 0);
        div
    }

    unsafe fn slots_available(&self) -> Option<usize> {
        let prev_alloc_offset = self.prev_alloc_offset();

        // For slot pointer we have to add 1 to reserved, but frame_push is just reserved.
        let reserved_words = RESERVED;

        let (left, right) = if self.is_west() {
            (self.frame_offset, prev_alloc_offset)
        } else {
            (prev_alloc_offset, self.frame_offset)
        };

        left.checked_sub(right)
            .and_then(|v| v.checked_sub(reserved_words))
    }

    // Get the offset of the previous alloc pointer
    unsafe fn prev_alloc_offset(&self) -> usize {
        // let prev_alloc_ptr = *self.prev_alloc_pointer_pointer();
        // if prev_alloc_ptr == self.start as *mut u64 {
        //     0
        // } else {
        //     // Calculate offset from base pointer in words
        //     (prev_alloc_ptr as usize - self.start as usize) / 8
        // }
        // seems to be ~5x faster
        let ptr = *self.prev_alloc_pointer_pointer() as usize;
        // ptr == start  ⇒  diff==0
        ((ptr).wrapping_sub(self.start as usize)) >> 3
    }

    /** Mutable pointer to a slot in a stack frame: east stack */
    // TODO: slot_pointer_east_: Needs a simple bounds check
    #[cfg(test)]
    unsafe fn slot_pointer_east_(&self, slot: usize) -> *mut u64 {
        self.alloc_would_oom_(
            Allocation {
                orientation: ArenaOrientation::East,
                alloc_type: AllocationType::SlotPointer,
                pc: self.pc,
            },
            slot,
        );
        self.derive_ptr(self.frame_offset + slot)
    }

    /** Mutable pointer to a slot in a stack frame: west stack */
    // TODO: slot_pointer_west_: Needs a simple bounds check
    #[cfg(test)]
    unsafe fn slot_pointer_west_(&self, slot: usize) -> *mut u64 {
        self.alloc_would_oom_(
            Allocation {
                orientation: ArenaOrientation::West,
                alloc_type: AllocationType::SlotPointer,
                pc: self.pc,
            },
            slot,
        );
        // Ensure we don't underflow if frame_offset is too small
        debug_assert!(self.frame_offset > slot, "Not enough space for slot");
        self.derive_ptr(self.frame_offset - (slot + 1))
    }

    /** Mutable pointer to a slot in a stack frame: east stack */
    // TODO: slot_pointer_east: Needs a simple bounds check
    unsafe fn slot_pointer_east(&self, slot: usize) -> *mut u64 {
        self.derive_ptr(self.frame_offset + slot)
    }

    unsafe fn slot_offset_east(&self, slot: usize) -> usize {
        self.frame_offset + slot
    }

    /** Mutable pointer to a slot in a stack frame: west stack */
    // TODO: slot_pointer_west: Needs a simple bounds check
    unsafe fn slot_pointer_west(&self, slot: usize) -> *mut u64 {
        // Ensure we don't underflow if frame_offset is too small
        debug_assert!(self.frame_offset > slot, "Not enough space for slot");
        self.derive_ptr(self.frame_offset - (slot + 1))
    }

    unsafe fn slot_offset_west(&self, slot: usize) -> usize {
        // Ensure we don't underflow if frame_offset is too small
        debug_assert!(self.frame_offset > slot, "Not enough space for slot");
        self.frame_offset - (slot + 1)
    }

    /// Mutable pointer to a slot in a stack frame
    /// Panics on out-of-bounds conditions
    #[cfg(test)]
    unsafe fn slot_pointer_(&self, slot: usize) -> *mut u64 {
        if self.is_west() {
            self.slot_pointer_west_(slot)
        } else {
            self.slot_pointer_east_(slot)
        }
    }

    /// Mutable pointer to a slot in a stack frame
    /// Panics on out-of-bounds conditions
    // TODO: slot_pointer: Needs a simple bounds check
    unsafe fn slot_pointer(&self, slot: usize) -> *mut u64 {
        if self.is_west() {
            self.slot_pointer_west(slot)
        } else {
            self.slot_pointer_east(slot)
        }
    }

    unsafe fn slot_offset(&self, slot: usize) -> usize {
        if self.is_west() {
            self.slot_offset_west(slot)
        } else {
            self.slot_offset_east(slot)
        }
    }

    /** Mutable pointer into a slot in free space east of allocation pointer */
    unsafe fn free_slot_east(&self, slot: usize) -> *mut u64 {
        self.derive_ptr(self.free_slot_east_offset(slot))
    }

    #[inline]
    unsafe fn free_slot_east_offset(&self, slot: usize) -> usize {
        // Ensure we don't overflow if alloc_offset is too large
        debug_assert!(
            self.alloc_offset + slot < self.size,
            "Not enough space for slot"
        );
        self.alloc_offset + slot
    }

    /** Mutable pointer into a slot in free space west of allocation pointer */
    unsafe fn free_slot_west(&self, slot: usize) -> *mut u64 {
        self.derive_ptr(self.free_slot_west_offset(slot))
    }

    #[inline]
    unsafe fn free_slot_west_offset(&self, slot: usize) -> usize {
        // Ensure we don't underflow if alloc_offset is too small
        debug_assert!(self.alloc_offset > slot, "Not enough space for slot");
        self.alloc_offset - (slot + 1)
    }

    unsafe fn free_slot(&self, slot: usize) -> *mut u64 {
        self.derive_ptr(self.free_slot_offset(slot))
    }

    #[inline]
    unsafe fn free_slot_offset(&self, slot: usize) -> usize {
        if self.is_west() {
            self.free_slot_west_offset(slot)
        } else {
            self.free_slot_east_offset(slot)
        }
    }

    /** Pointer to a local slot typed as Noun */
    pub(crate) unsafe fn local_noun_pointer(&mut self, local: usize) -> *mut Noun {
        let res = self.slot_pointer(local + RESERVED);
        res as *mut Noun
    }

    /** Pointer to where the previous frame pointer is saved in a frame */
    unsafe fn prev_frame_pointer_pointer(&self) -> *mut *mut u64 {
        let res = if !self.pc {
            self.slot_pointer(FRAME)
        } else {
            self.free_slot(FRAME)
        };
        res as *mut *mut u64
    }

    /** Pointer to where the previous stack pointer is saved in a frame */
    pub(crate) unsafe fn prev_stack_pointer_pointer(&self) -> *mut *mut u64 {
        let res = if !self.pc {
            self.slot_pointer(STACK)
        } else {
            self.free_slot(STACK)
        };
        res as *mut *mut u64
    }

    // Removed prev_alloc_offset_offset - it was using undefined functions

    /** Pointer to where the previous alloc pointer is saved in a frame */
    unsafe fn prev_alloc_pointer_pointer(&self) -> *mut *mut u64 {
        let res = if !self.pc {
            self.slot_pointer(ALLOC)
        } else {
            self.free_slot(ALLOC)
        };
        res as *mut *mut u64
    }

    /**  Allocation
     * In a west frame, the allocation pointer is higher than the frame pointer, and so the allocation
     * size is subtracted from the allocation pointer, and then the allocation pointer is returned as
     * the pointer to the newly allocated memory.
     *
     * In an east frame, the allocation pointer is lower than the frame pointer, and so the allocation
     * pointer is saved in a temporary, then the allocation size added to it, and finally the original
     * allocation pointer is returned as the pointer to the newly allocated memory.
     * */
    // Bump the alloc pointer for a west frame to make space for an allocation
    unsafe fn raw_alloc_west(&mut self, words: usize) -> *mut u64 {
        self.alloc_would_oom(AllocationType::Alloc, words);
        if self.pc {
            panic!("Allocation during cleanup phase is prohibited.");
        }

        // Calculate new offset with safe subtraction
        let new_alloc_offset = match self.alloc_offset.checked_sub(words) {
            Some(offset) => offset,
            None => panic!("Alloc offset underflow in West frame"),
        };

        // Update the space low-water-mark
        let new_space = match new_alloc_offset.checked_sub(self.stack_offset) {
            Some(space) => space,
            None => panic_any(
                self.out_of_memory(self.get_alloc_config(AllocationType::Alloc), Some(words)),
            ),
        };
        self.least_space = new_space.min(self.least_space);

        // Derive pointer from the new offset
        let alloc_ptr = self.derive_ptr(new_alloc_offset);

        // Update the alloc offset
        self.alloc_offset = new_alloc_offset;
        debug_assert!(self.alloc_offset <= self.size, "Alloc offset out of bounds");

        // Return the pointer to the allocated space
        alloc_ptr
    }

    /** Bump the alloc pointer for an east frame to make space for an allocation */
    unsafe fn raw_alloc_east(&mut self, words: usize) -> *mut u64 {
        self.alloc_would_oom(AllocationType::Alloc, words);
        if self.pc {
            panic!("Allocation during cleanup phase is prohibited.");
        }

        // Get the pointer for the current allocation
        let alloc_ptr = self.derive_ptr(self.alloc_offset);

        // Calculate new offset with safe addition
        let new_alloc_offset = match self.alloc_offset.checked_add(words) {
            Some(offset) => offset,
            None => panic!("Alloc offset overflow in East frame"),
        };

        let new_space = match self.stack_offset.checked_sub(new_alloc_offset) {
            Some(space) => space,
            None => panic_any(
                self.out_of_memory(self.get_alloc_config(AllocationType::Alloc), Some(words)),
            ),
        };
        self.least_space = new_space.min(self.least_space);

        // Check that the new offset is within bounds
        if new_alloc_offset > self.size {
            panic!(
                "New allocation offset out of bounds: {} > {}",
                new_alloc_offset, self.size
            );
        }

        // Update the alloc offset
        self.alloc_offset = new_alloc_offset;

        // Return the pointer to the allocated space
        alloc_ptr
    }

    /** Allocate space for an indirect pointer in a west frame */
    unsafe fn indirect_alloc_west(&mut self, words: usize) -> *mut u64 {
        self.raw_alloc_west(words + 2)
    }

    /** Allocate space for an indirect pointer in an east frame */
    unsafe fn indirect_alloc_east(&mut self, words: usize) -> *mut u64 {
        self.raw_alloc_east(words + 2)
    }

    /** Allocate space for an indirect pointer in a stack frame */
    unsafe fn indirect_alloc(&mut self, words: usize) -> *mut u64 {
        if self.is_west() {
            self.indirect_alloc_west(words)
        } else {
            self.indirect_alloc_east(words)
        }
    }

    /** Allocate space for a struct in a west frame */
    unsafe fn struct_alloc_west<T>(&mut self, count: usize) -> *mut T {
        let eigen_pointer = self.raw_alloc_west(word_size_of::<T>() * count);
        eigen_pointer as *mut T
    }

    /** Allocate space for a struct in an east frame */
    unsafe fn struct_alloc_east<T>(&mut self, count: usize) -> *mut T {
        let eigen_pointer = self.raw_alloc_east(word_size_of::<T>() * count);
        eigen_pointer as *mut T
    }

    /** Allocate space for a struct in a stack frame */
    pub unsafe fn struct_alloc<T>(&mut self, count: usize) -> *mut T {
        if self.is_west() {
            self.struct_alloc_west::<T>(count)
        } else {
            self.struct_alloc_east::<T>(count)
        }
    }

    unsafe fn raw_alloc_in_previous_frame_west(&mut self, words: usize) -> *mut u64 {
        self.alloc_would_oom_(
            Allocation {
                orientation: ArenaOrientation::West,
                alloc_type: AllocationType::AllocPreviousFrame,
                pc: self.pc,
            },
            words,
        );
        // Note that the allocation is on the east frame, thus resembles raw_alloc_east
        // Get the prev_alloc_offset
        let prev_alloc_offset = self.prev_alloc_offset();

        // Store the current pointer to return
        let alloc_ptr = self.derive_ptr(prev_alloc_offset);

        // Calculate new offset with safe addition
        let new_prev_alloc_offset = match prev_alloc_offset.checked_add(words) {
            Some(offset) => offset,
            None => panic!("Previous frame alloc offset overflow in West orientation"),
        };

        // Check that the new offset is within bounds
        if new_prev_alloc_offset >= self.size {
            panic!(
                "New allocation offset out of bounds: {} >= {}",
                new_prev_alloc_offset, self.size
            );
        }

        // Create the new pointer and update it in the previous frame
        let new_prev_alloc_ptr = self.derive_ptr(new_prev_alloc_offset);
        *(self.prev_alloc_pointer_pointer()) = new_prev_alloc_ptr;

        // Return the original pointer
        alloc_ptr
    }

    unsafe fn raw_alloc_in_previous_frame_east(&mut self, words: usize) -> *mut u64 {
        self.alloc_would_oom_(
            Allocation {
                orientation: ArenaOrientation::East,
                alloc_type: AllocationType::AllocPreviousFrame,
                pc: self.pc,
            },
            words,
        );
        // Note that the allocation is on the west frame, thus resembles raw_alloc_west
        // Get the prev_alloc_offset
        let prev_alloc_offset = self.prev_alloc_offset();

        // Calculate new offset with safe subtraction
        let new_prev_alloc_offset = match prev_alloc_offset.checked_sub(words) {
            Some(offset) => offset,
            None => panic!("Previous frame alloc offset underflow in East orientation"),
        };

        // Check that the new offset is within bounds
        if new_prev_alloc_offset >= self.size {
            panic!(
                "New allocation offset out of bounds: {} >= {}",
                new_prev_alloc_offset, self.size
            );
        }

        // Create the new pointer and update it in the previous frame
        let new_prev_alloc_ptr = self.derive_ptr(new_prev_alloc_offset);
        *(self.prev_alloc_pointer_pointer()) = new_prev_alloc_ptr;

        // Return the new pointer (this matches the old behavior)
        new_prev_alloc_ptr
    }

    /** Allocate space in the previous stack frame. This calls pre_copy() first to ensure that the
     * stack frame is in cleanup phase, which is the only time we should be allocating in a previous
     * frame. */
    unsafe fn raw_alloc_in_previous_frame(&mut self, words: usize) -> *mut u64 {
        self.pre_copy();
        if self.is_west() {
            self.raw_alloc_in_previous_frame_west(words)
        } else {
            self.raw_alloc_in_previous_frame_east(words)
        }
    }

    /** Allocates space in the previous frame for some number of T's. */
    pub unsafe fn struct_alloc_in_previous_frame<T>(&mut self, count: usize) -> *mut T {
        let res = self.raw_alloc_in_previous_frame(word_size_of::<T>() * count);
        res as *mut T
    }

    /** Allocate space for an indirect atom in the previous stack frame. */
    unsafe fn indirect_alloc_in_previous_frame(&mut self, words: usize) -> *mut u64 {
        self.raw_alloc_in_previous_frame(words + 2)
    }

    /** Allocate space for an alloc::Layout in a stack frame */
    unsafe fn layout_alloc(&mut self, layout: Layout) -> *mut u64 {
        assert!(layout.align() <= 64, "layout alignment must be <= 64");
        if self.is_west() {
            self.raw_alloc_west((layout.size() + 7) >> 3)
        } else {
            self.raw_alloc_east((layout.size() + 7) >> 3)
        }
    }

    /**  Copying and Popping
     * Prior to any copying step, the saved frame, stack, and allocation pointers must
     * be moved out of the frame. A three-word allocation is made to hold the saved
     * frame, stack, and allocation pointers. After this they will be accessed by reference
     * to the allocation pointer, so no more allocations must be made between now
     * and restoration.
     *
     * Copying can then proceed by updating the saved allocation pointer for each
     * copied object. This will almost immediately clobber the frame, so return by
     * writing to a slot in the previous frame or in a register is necessary.
     *
     * Finally, the frame, stack, and allocation pointers are restored from the saved
     * location.
     *
     * Copies reserved pointers to free space adjacent to the allocation arena, and
     * moves the lightweight stack to the free space adjacent to that.
     *
     * Once this function is called a on stack frame, we say that it is now in the "cleanup
     * phase". At this point, no more allocations can be made, and all that is left to
     * do is figure out what data in this frame needs to be preserved and thus copied to
     * the parent frame.
     *
     * This might be the most confusing part of the split stack system. But we've tried
     * to make it so that the programmer doesn't need to think about it at all. The
     * interface for using the reserved pointers (prev_xyz_pointer_pointer()) and
     * lightweight stack (push(), pop(), top()) are the same regardless of whether
     * or not pre_copy() has been called.
     * */
    unsafe fn pre_copy(&mut self) {
        // pre_copy is intended to be idempotent, so we don't need to do anything if it's already been called
        if !self.pc {
            let is_west = self.is_west();
            let words = if is_west { RESERVED + 1 } else { RESERVED };
            // TODO: pre_copy: Treating pre_copy like a FramePush for OOM checking purposes
            // Is this correct?
            let () = self.alloc_would_oom_(self.get_alloc_config(AllocationType::FramePush), words);

            // Copy the previous frame/stack/alloc pointers to free slots
            *(self.free_slot(FRAME)) = *(self.slot_pointer(FRAME));
            *(self.free_slot(STACK)) = *(self.slot_pointer(STACK));
            *(self.free_slot(ALLOC)) = *(self.slot_pointer(ALLOC));

            self.pc = true;

            // Change polarity of lightweight stack by updating the stack offset
            if is_west {
                self.stack_offset = self.alloc_offset - words;
            } else {
                self.stack_offset = self.alloc_offset + words;
            }
        }
    }

    // Doesn't need an OOM check, just an assertion. We expect it to panic.
    pub(crate) unsafe fn assert_struct_is_in<T>(&self, ptr: *const T, count: usize) {
        // Get the appropriate offsets based on pre-copy status
        let alloc_offset = if self.pc {
            self.prev_alloc_offset()
        } else {
            self.alloc_offset
        };

        let stack_offset = if self.pc {
            // Get previous stack offset
            let prev_ptr = *self.prev_stack_pointer_pointer();
            (prev_ptr as usize - self.start as usize) / 8
        } else {
            self.stack_offset
        };

        // Calculate the pointer offset from the base in words
        let ptr_start_offset = (ptr as usize - self.start as usize) / 8;
        let ptr_end_offset =
            ((ptr as usize) + count * std::mem::size_of::<T>() - self.start as usize) / 8;

        // Determine the valid memory range
        let (low_offset, high_offset) = if alloc_offset > stack_offset {
            (stack_offset, alloc_offset)
        } else {
            (alloc_offset, stack_offset)
        };

        // Convert offsets to byte addresses for error reporting
        let low = self.start as usize + (low_offset * 8);
        let hi = self.start as usize + (high_offset * 8);

        // Check if pointer is outside the valid range
        if (ptr_start_offset < low_offset && ptr_end_offset <= low_offset)
            || (ptr_start_offset >= high_offset && ptr_end_offset > high_offset)
        {
            // The pointer is outside the allocation range, which is valid
            return;
        }

        // If we got here, there's a use-after-free problem
        panic!(
            "Use after free: allocation from {:#x} to {:#x}, free space from {:#x} to {:#x}",
            ptr as usize,
            (ptr as usize) + count * std::mem::size_of::<T>(),
            low,
            hi
        );
    }

    // Doesn't need an OOM check, just an assertion. We expect it to panic.
    unsafe fn assert_noun_in(&self, noun: Noun) {
        let mut dbg_stack = Vec::new();
        dbg_stack.push(noun);

        // Get the appropriate offsets based on pre-copy status
        let alloc_offset = if self.pc {
            self.prev_alloc_offset()
        } else {
            self.alloc_offset
        };

        let stack_offset = if self.pc {
            // Get previous stack offset
            let prev_ptr = *self.prev_stack_pointer_pointer();
            (prev_ptr as usize - self.start as usize) / 8
        } else {
            self.stack_offset
        };

        // Determine the valid memory range (in words)
        let (low_offset, high_offset) = if alloc_offset > stack_offset {
            (stack_offset, alloc_offset)
        } else {
            (alloc_offset, stack_offset)
        };

        // Convert offsets to byte addresses for checking and reporting
        let low = self.start as usize + (low_offset * 8);
        let hi = self.start as usize + (high_offset * 8);

        loop {
            if let Some(subnoun) = dbg_stack.pop() {
                if let Ok(a) = subnoun.as_allocated() {
                    // Get the pointer address
                    let np = a.to_raw_pointer() as usize;

                    // Check if the noun is in the free space (which would be an error)
                    if np >= low && np < hi {
                        panic!("noun not in {:?}: {:?}", (low, hi), subnoun);
                    }

                    // If it's a cell, check its head and tail too
                    if let Right(c) = a.as_either() {
                        dbg_stack.push(c.tail());
                        dbg_stack.push(c.head());
                    }
                }
            } else {
                return;
            }
        }
    }

    // Note re: #684: We don't need OOM checks on de-alloc
    pub(crate) unsafe fn frame_pop(&mut self) {
        let prev_frame_ptr = *self.prev_frame_pointer_pointer();
        let prev_stack_ptr = *self.prev_stack_pointer_pointer();
        let prev_alloc_ptr = *self.prev_alloc_pointer_pointer();

        // Check for null pointers before calculating offsets
        if prev_frame_ptr.is_null() || prev_stack_ptr.is_null() || prev_alloc_ptr.is_null() {
            panic!(
                "serf: frame_pop: null NockStack pointer f={prev_frame_ptr:p} s={prev_stack_ptr:p} a={prev_alloc_ptr:p}",
            );
        }

        // Calculate the offsets from base pointer
        self.frame_offset = (prev_frame_ptr as usize - self.start as usize) / 8;
        self.stack_offset = (prev_stack_ptr as usize - self.start as usize) / 8;
        self.alloc_offset = (prev_alloc_ptr as usize - self.start as usize) / 8;

        self.pc = false;
    }

    pub unsafe fn preserve<T: Preserve>(&mut self, x: &mut T) {
        x.preserve(self)
    }

    /**  Pushing
     *  When pushing, we swap the stack and alloc pointers, set the frame pointer to be the stack
     *  pointer, move both frame and stack pointer by number of locals (eastward for west frames,
     *  westward for east frame), and then save the old stack/frame/alloc pointers in slots
     *  adjacent to the frame pointer.
     * Push a frame onto the stack with 0 or more local variable slots. */
    /// This computation for num_locals is done in the east/west variants, but roughly speaking it's the input n words + 3 for prev frame alloc/stack/frame pointers
    pub fn frame_push(&mut self, num_locals: usize) {
        if self.pc {
            panic!("frame_push during cleanup phase is prohibited.");
        }
        let words = num_locals + RESERVED;
        self.alloc_would_oom(AllocationType::FramePush, words);

        // Save current offsets
        let current_frame_offset = self.frame_offset;
        let current_stack_offset = self.stack_offset;
        let current_alloc_offset = self.alloc_offset;

        unsafe {
            // Calculate new offsets
            if self.is_west() {
                self.frame_offset = self.alloc_offset - words;
            } else {
                self.frame_offset = self.alloc_offset + words;
            }

            // Update stack and alloc offsets
            self.alloc_offset = self.stack_offset;
            self.stack_offset = self.frame_offset;

            // Store pointers to previous frame in reserved slots
            let current_frame_ptr = self.derive_ptr(current_frame_offset);
            let current_stack_ptr = self.derive_ptr(current_stack_offset);
            let current_alloc_ptr = self.derive_ptr(current_alloc_offset);

            *(self.slot_pointer(FRAME)) = current_frame_ptr as u64;
            *(self.slot_pointer(STACK)) = current_stack_ptr as u64;
            *(self.slot_pointer(ALLOC)) = current_alloc_ptr as u64;
        }
    }

    /** Run a closure inside a frame, popping regardless of the value returned by the closure.
     * This is useful for writing fallible computations with the `?` operator.
     *
     * Note that results allocated on the stack *must* be `preserve()`d by the closure.
     */
    pub(crate) unsafe fn with_frame<F, O>(&mut self, num_locals: usize, f: F) -> O
    where
        F: FnOnce(&mut NockStack) -> O,
        O: Preserve,
    {
        self.frame_push(num_locals);
        let mut ret = f(self);
        ret.preserve(self);
        self.frame_pop();
        ret
    }

    /** Lightweight stack.
     * The lightweight stack is a stack data structure present in each stack
     * frame, often used for noun traversal. During normal operation (self.pc
     * == false),a west frame has a "west-oriented" lightweight stack, which
     * means that it sits immediately eastward of the frame pointer, pushing
     * moves the stack pointer eastward, and popping moves the frame pointer
     * westward. The stack is empty when stack_pointer == frame_pointer. The
     * east frame situation is the same, swapping west for east.
     *
     * Once a stack frame is preparing to be popped, pre_copy() is called (pc == true)
     * and this reverses the orientation of the lightweight stack. For a west frame,
     * that means it starts at the eastward most free byte west of the allocation
     * arena (which is now more words west than the allocation pointer, to account
     * for slots containing the previous frame's pointers), pushing moves the
     * stack pointer westward, and popping moves it eastward. Again, the east
     * frame situation is the same, swapping west for east.
     *
     * When pc == true, the lightweight stack is used for copying from the current
     * frame's allocation arena to the previous frames.
     *
     * Push onto the lightweight stack, moving the stack_pointer. Note that
     * this violates the _east/_west naming convention somewhat, since e.g.
     * a west frame when pc == false has a west-oriented lightweight stack,
     * but when pc == true it becomes east-oriented.*/
    pub(crate) unsafe fn push<T>(&mut self) -> *mut T {
        if self.is_west() && !self.pc || !self.is_west() && self.pc {
            self.push_west::<T>()
        } else {
            self.push_east::<T>()
        }
    }

    /// Push onto a west-oriented lightweight stack, moving the stack_pointer.
    unsafe fn push_west<T>(&mut self) -> *mut T {
        let words = word_size_of::<T>();
        self.alloc_would_oom_(
            Allocation {
                orientation: ArenaOrientation::West,
                alloc_type: AllocationType::Push,
                pc: self.pc,
            },
            words,
        );

        // Get the appropriate limit offset
        let limit_offset = if self.pc {
            self.prev_alloc_offset()
        } else {
            self.alloc_offset
        };

        // Get the current pointer at stack_offset (before we move it)
        let alloc_ptr = self.derive_ptr(self.stack_offset);

        // Calculate the new stack offset
        let new_stack_offset = self.stack_offset + words;

        // Check if we've gone past the limit
        if new_stack_offset > limit_offset {
            panic!(
                "Out of memory, alloc_would_oom didn't catch it. memory_state: {:#?}",
                self.memory_state(Some(words))
            );
        } else {
            // Update stack offset and return the original pointer
            self.stack_offset = new_stack_offset;
            alloc_ptr as *mut T
        }
    }

    /// Push onto an east-oriented ligthweight stack, moving the stack_pointer
    unsafe fn push_east<T>(&mut self) -> *mut T {
        let words = word_size_of::<T>();
        self.alloc_would_oom_(
            Allocation {
                orientation: ArenaOrientation::East,
                alloc_type: AllocationType::Push,
                pc: self.pc,
            },
            words,
        );

        // Get the appropriate limit offset
        let limit_offset = if self.pc {
            self.prev_alloc_offset()
        } else {
            self.alloc_offset
        };

        // Calculate the new stack offset
        if self.stack_offset < words {
            panic!("Stack offset underflow during push_east");
        }
        let new_stack_offset = self.stack_offset - words;

        // Check if we've gone below the limit
        if new_stack_offset < limit_offset {
            panic!(
                "Out of memory, alloc_would_oom didn't catch it. memory_state: {:#?}",
                self.memory_state(Some(words))
            );
        } else {
            // Get the pointer at the new offset
            let alloc_ptr = self.derive_ptr(new_stack_offset);

            // Update stack offset
            self.stack_offset = new_stack_offset;

            // Return the pointer at the new offset
            alloc_ptr as *mut T
        }
    }

    /** Pop a west-oriented lightweight stack, moving the stack pointer. */
    unsafe fn pop_west<T>(&mut self) {
        let words = word_size_of::<T>();
        if self.stack_offset < words {
            panic!("Stack underflow during pop_west");
        }
        self.stack_offset -= words;
    }

    /** Pop an east-oriented lightweight stack, moving the stack pointer. */
    unsafe fn pop_east<T>(&mut self) {
        let words = word_size_of::<T>();
        self.stack_offset += words;
        if self.stack_offset > self.size {
            panic!("Stack overflow during pop_east");
        }
    }

    /** Pop the lightweight stack, moving the stack_pointer. Note that
     * this violates the _east/_west naming convention somewhat, since e.g.
     * a west frame when pc == false has a west-oriented lightweight stack,
     * but when pc == true it becomes east-oriented.*/
    // Re: #684: We don't need OOM checks on pop
    #[inline]
    pub(crate) unsafe fn pop<T>(&mut self) {
        if self.is_west() && !self.pc || !self.is_west() && self.pc {
            self.pop_west::<T>();
        } else {
            self.pop_east::<T>();
        };
    }

    /** Peek the top of the lightweight stack. Note that
     * this violates the _east/_west naming convention somewhat, since e.g.
     * a west frame when pc == false has a west-oriented lightweight stack,
     * but when pc == true it becomes east-oriented.*/
    #[inline]
    pub(crate) unsafe fn top<T>(&mut self) -> *mut T {
        if self.is_west() && !self.pc || !self.is_west() && self.pc {
            self.top_west()
        } else {
            self.top_east()
        }
    }

    /** Peek the top of a west-oriented lightweight stack. */
    #[inline]
    unsafe fn top_west<T>(&mut self) -> *mut T {
        let words = word_size_of::<T>();
        if self.stack_offset < words {
            panic!("Stack underflow during top_west");
        }
        self.derive_ptr(self.stack_offset - words) as *mut T
    }

    /** Peek the top of an east-oriented lightweight stack. */
    #[inline]
    unsafe fn top_east<T>(&mut self) -> *mut T {
        self.derive_ptr(self.stack_offset) as *mut T
    }

    /** Checks to see if the lightweight stack is empty. Note that this doesn't work
     * when the stack pointer has been moved to be close to the allocation arena, such
     * as in copy_west(). */
    pub(crate) fn stack_is_empty(&self) -> bool {
        if !self.pc {
            self.stack_offset == self.frame_offset
        } else if self.is_west() {
            // Check if we've moved the stack to the beginning of free space
            let expected_offset = self.alloc_offset - (RESERVED + 1);
            self.stack_offset == expected_offset
        } else {
            // Check if we've moved the stack to the beginning of free space
            let expected_offset = self.alloc_offset + RESERVED;
            self.stack_offset == expected_offset
        }
    }

    pub(crate) fn no_junior_pointers(&self, noun: Noun) -> bool {
        unsafe {
            if let Ok(c) = noun.as_cell() {
                let mut dbg_stack = Vec::new();

                // Start with the current frame's offsets
                // No need to track the initial frame orientation
                let mut stack_offset = self.stack_offset;
                let mut alloc_offset = self.alloc_offset;

                // Get the previous frame's pointers
                let mut prev_frame_ptr = *self.prev_frame_pointer_pointer();
                let mut prev_stack_ptr = *self.prev_stack_pointer_pointer();
                let mut prev_alloc_ptr = *self.prev_alloc_pointer_pointer();

                let mut prev_stack_offset = if !prev_stack_ptr.is_null() {
                    (prev_stack_ptr as usize - self.start as usize) / 8
                } else {
                    self.size // Use end of memory if null (for top frame)
                };

                let mut prev_alloc_offset = if !prev_alloc_ptr.is_null() {
                    (prev_alloc_ptr as usize - self.start as usize) / 8
                } else {
                    0 // Not used if null
                };

                // Determine the cell pointer offset
                let cell_ptr_offset = (c.to_raw_pointer() as usize - self.start as usize) / 8;

                // Determine range for the cell's frame
                let (range_lo_offset, range_hi_offset) = loop {
                    // Handle null stack pointer for top frame
                    if prev_stack_ptr.is_null() {
                        prev_stack_offset = self.size; // Use end of memory
                    }

                    // Determine current frame's allocation range based on orientation
                    let (lo_offset, hi_offset) = if stack_offset < alloc_offset {
                        // West frame
                        (alloc_offset, prev_stack_offset)
                    } else {
                        // East frame
                        (prev_stack_offset, alloc_offset)
                    };

                    // Check if the cell is in this frame's range
                    if cell_ptr_offset >= lo_offset && cell_ptr_offset < hi_offset {
                        // Use frame boundary for range calculation
                        break if stack_offset < alloc_offset {
                            (stack_offset, alloc_offset)
                        } else {
                            (alloc_offset, stack_offset)
                        };
                    } else {
                        // Move to previous frame
                        stack_offset = prev_stack_offset;
                        alloc_offset = prev_alloc_offset;

                        // Calculate orientation for previous frame
                        let is_west = stack_offset < alloc_offset;

                        // Retrieve next previous frame's pointers
                        // Instead of calculating from frame_offset (which we no longer track),
                        // we'll use the previous frame pointer directly to access slots
                        if !prev_frame_ptr.is_null() {
                            if is_west {
                                // For west frames, previous pointers are at [prev_frame_ptr - (SLOT + 1)]
                                prev_frame_ptr = *(prev_frame_ptr.sub(FRAME + 1) as *mut *mut u64);
                                prev_stack_ptr = *(prev_frame_ptr.sub(STACK + 1) as *mut *mut u64);
                                prev_alloc_ptr = *(prev_frame_ptr.sub(ALLOC + 1) as *mut *mut u64);
                            } else {
                                // For east frames, previous pointers are at [prev_frame_ptr + SLOT]
                                prev_frame_ptr = *(prev_frame_ptr.add(FRAME) as *mut *mut u64);
                                prev_stack_ptr = *(prev_frame_ptr.add(STACK) as *mut *mut u64);
                                prev_alloc_ptr = *(prev_frame_ptr.add(ALLOC) as *mut *mut u64);
                            }
                        }

                        // We no longer need to track prev_frame_offset since we're not using frame_offset

                        prev_stack_offset = if !prev_stack_ptr.is_null() {
                            (prev_stack_ptr as usize - self.start as usize) / 8
                        } else {
                            self.size // Use end of memory if null (for top frame)
                        };

                        prev_alloc_offset = if !prev_alloc_ptr.is_null() {
                            (prev_alloc_ptr as usize - self.start as usize) / 8
                        } else {
                            0 // Not used if null
                        };
                    }
                };

                // Convert offsets to pointers for error reporting
                let range_lo_ptr = self.derive_ptr(range_lo_offset);
                let range_hi_ptr = self.derive_ptr(range_hi_offset);

                // Check all nouns in the tree
                dbg_stack.push(c.head());
                dbg_stack.push(c.tail());
                while let Some(n) = dbg_stack.pop() {
                    if let Ok(a) = n.as_allocated() {
                        let ptr = a.to_raw_pointer();
                        // Calculate pointer offset
                        let ptr_offset = (ptr as usize - self.start as usize) / 8;

                        // Check if the pointer is within the invalid range
                        if ptr_offset >= range_lo_offset && ptr_offset < range_hi_offset {
                            eprintln!(
                                "\rserf: Noun {:x} has Noun {:x} in junior of range {:p}-{:p}",
                                (noun.raw << 3),
                                (n.raw << 3),
                                range_lo_ptr,
                                range_hi_ptr
                            );
                            return false;
                        }

                        // Continue traversing if it's a cell
                        if let Some(c) = a.cell() {
                            dbg_stack.push(c.tail());
                            dbg_stack.push(c.head());
                        }
                    }
                }

                true
            } else {
                true
            }
        }
    }

    /**
     * Debugging
     *
     * The below functions are useful for debugging NockStack issues.
     *
     * Walk down the NockStack, printing frames. Absolutely no safety checks are peformed, as the
     * purpose is to discover garbage data; just print pointers until the bottom of the NockStack
     * (i.e. a null frame pointer) is encountered. Possible to crash, if a frame pointer gets
     * written over.
     */
    pub(crate) fn print_frames(&mut self) {
        let mut fp = unsafe { self.frame_pointer() };
        let mut sp = unsafe { self.stack_pointer() };
        let mut ap = unsafe { self.alloc_pointer() };
        let mut c = 0u64;

        eprintln!("\r start = {:p}", self.start);

        loop {
            c += 1;

            eprintln!("\r {c}:");
            eprintln!("\r frame_pointer = {fp:p}");
            eprintln!("\r stack_pointer = {sp:p}");
            eprintln!("\r alloc_pointer = {ap:p}");

            if fp.is_null() {
                break;
            }

            unsafe {
                if fp < ap {
                    sp = *(fp.sub(STACK + 1) as *mut *mut u64);
                    ap = *(fp.sub(ALLOC + 1) as *mut *mut u64);
                    fp = *(fp.sub(FRAME + 1) as *mut *mut u64);
                } else {
                    sp = *(fp.add(STACK) as *mut *mut u64);
                    ap = *(fp.add(ALLOC) as *mut *mut u64);
                    fp = *(fp.add(FRAME) as *mut *mut u64);
                }
            }
        }
    }

    /**
     * Sanity check every frame of the NockStack. Most useful paired with a gdb session set to
     * catch rust_panic.
     */
    // #684: Don't need OOM checks here
    pub(crate) fn assert_sane(&mut self) {
        let start = self.start;
        let limit = unsafe { self.start.add(self.size) };
        let mut fp = unsafe { self.frame_pointer() };
        let mut sp = unsafe { self.stack_pointer() };
        let mut ap = unsafe { self.alloc_pointer() };
        let mut ought_west: bool = fp < ap;

        loop {
            // fp is null iff sp is null
            assert!(!(fp.is_null() ^ sp.is_null()));

            // ap should never be null
            assert!(!ap.is_null());

            if fp.is_null() {
                break;
            }

            // all pointers must be between start and size
            assert!(fp as *const u64 >= start);
            assert!(fp as *const u64 <= limit);
            assert!(sp as *const u64 >= start);
            assert!(sp as *const u64 <= limit);
            assert!(ap as *const u64 >= start);
            assert!(ap as *const u64 <= limit);

            // frames should flip between east-west correctly
            assert!((fp < ap) == ought_west);

            // sp should be between fp and ap
            if ought_west {
                assert!(sp >= fp);
                assert!(sp < ap);
            } else {
                assert!(sp <= fp);
                assert!(sp > ap);
            }

            unsafe {
                if ought_west {
                    sp = *(fp.sub(STACK + 1) as *mut *mut u64);
                    ap = *(fp.sub(ALLOC + 1) as *mut *mut u64);
                    fp = *(fp.sub(FRAME + 1) as *mut *mut u64);
                } else {
                    sp = *(fp.add(STACK) as *mut *mut u64);
                    ap = *(fp.add(ALLOC) as *mut *mut u64);
                    fp = *(fp.add(FRAME) as *mut *mut u64);
                }
            }
            ought_west = !ought_west;
        }
    }
}

impl NounAllocator for NockStack {
    unsafe fn alloc_indirect(&mut self, words: usize) -> *mut u64 {
        self.indirect_alloc(words)
    }

    unsafe fn alloc_cell(&mut self) -> *mut CellMemory {
        self.struct_alloc::<CellMemory>(1)
    }

    unsafe fn alloc_struct<T>(&mut self, count: usize) -> *mut T {
        self.struct_alloc::<T>(count)
    }

    unsafe fn equals(&mut self, a: *mut Noun, b: *mut Noun) -> bool {
        crate::unifying_equality::unifying_equality(self, a, b)
    }
}

/// Immutable, acyclic objects which may be copied up the stack
pub trait Preserve {
    /// Ensure an object will not be invalidated by popping the NockStack
    unsafe fn preserve(&mut self, stack: &mut NockStack);
    unsafe fn assert_in_stack(&self, stack: &NockStack);
}

impl Preserve for () {
    unsafe fn preserve(&mut self, _stack: &mut NockStack) {}

    unsafe fn assert_in_stack(&self, _stack: &NockStack) {}
}

impl Preserve for IndirectAtom {
    unsafe fn preserve(&mut self, stack: &mut NockStack) {
        let size = indirect_raw_size(*self);
        let buf = stack.struct_alloc_in_previous_frame::<u64>(size);
        copy_nonoverlapping(self.to_raw_pointer(), buf, size);
        *self = IndirectAtom::from_raw_pointer(buf);
    }
    unsafe fn assert_in_stack(&self, stack: &NockStack) {
        stack.assert_noun_in(self.as_atom().as_noun());
    }
}

impl Preserve for Atom {
    unsafe fn preserve(&mut self, stack: &mut NockStack) {
        match self.as_either() {
            Left(_direct) => {}
            Right(mut indirect) => {
                indirect.preserve(stack);
                *self = indirect.as_atom();
            }
        }
    }
    unsafe fn assert_in_stack(&self, stack: &NockStack) {
        stack.assert_noun_in(self.as_noun());
    }
}

impl Preserve for Noun {
    unsafe fn preserve(&mut self, stack: &mut NockStack) {
        noun_preserve(stack, self)
    }
    unsafe fn assert_in_stack(&self, stack: &NockStack) {
        stack.assert_noun_in(*self);
    }
}

/// Used to be stack.copy, but it was only used as a Preserve impl for Noun.
/// This version tries to bail earlier than the old one and we're using a Vec
/// for the worklist.
unsafe fn noun_preserve(stack: &mut NockStack, noun: &mut Noun) {
    assert_acyclic!(*noun);
    assert_no_forwarding_pointers!(*noun);
    assert_no_junior_pointers!(stack, *noun);

    let root_allocated = match noun.as_either_direct_allocated() {
        Either::Left(_direct) => return,
        Either::Right(allocated) => allocated,
    };

    if let Some(new_allocated) = root_allocated.forwarding_pointer() {
        *noun = new_allocated.as_noun();
        return;
    }

    if !stack.is_in_frame(root_allocated.to_raw_pointer()) {
        return;
    }

    // TODO: Try making this buffer part of NockStack
    let mut work: Vec<(Noun, *mut Noun)> = Vec::with_capacity(32);
    work.push((*noun, noun as *mut Noun));

    while let Some((value, dest_ptr)) = work.pop() {
        match value.as_either_direct_allocated() {
            Either::Left(_direct) => unsafe {
                *dest_ptr = value;
            },
            Either::Right(allocated) => unsafe {
                if let Some(new_allocated) = allocated.forwarding_pointer() {
                    *dest_ptr = new_allocated.as_noun();
                    continue;
                }

                if !stack.is_in_frame(allocated.to_raw_pointer()) {
                    *dest_ptr = value;
                    continue;
                }

                match allocated.as_either() {
                    Either::Left(mut indirect) => {
                        let alloc = stack.indirect_alloc_in_previous_frame(indirect.size());
                        copy_nonoverlapping(
                            indirect.to_raw_pointer(),
                            alloc,
                            indirect_raw_size(indirect),
                        );
                        indirect.set_forwarding_pointer(alloc);
                        *dest_ptr = IndirectAtom::from_raw_pointer(alloc).as_noun();
                    }
                    Either::Right(mut cell) => {
                        let alloc = stack.struct_alloc_in_previous_frame::<CellMemory>(1);
                        (*alloc).metadata = (*cell.to_raw_pointer()).metadata;

                        let tail = cell.tail();
                        let head = cell.head();

                        cell.set_forwarding_pointer(alloc);

                        work.push((tail, &mut (*alloc).tail));
                        work.push((head, &mut (*alloc).head));

                        *dest_ptr = Cell::from_raw_pointer(alloc).as_noun();
                    }
                }
            },
        }
    }

    assert_acyclic!(*noun);
    assert_no_forwarding_pointers!(*noun);
    assert_no_junior_pointers!(stack, *noun);
}

impl Stack for NockStack {
    unsafe fn alloc_layout(&mut self, layout: Layout) -> *mut u64 {
        self.layout_alloc(layout)
    }
}

impl<T: Preserve, E: Preserve> Preserve for Result<T, E> {
    unsafe fn preserve(&mut self, stack: &mut NockStack) {
        match self.as_mut() {
            Ok(t_ref) => t_ref.preserve(stack),
            Err(e_ref) => e_ref.preserve(stack),
        }
    }

    unsafe fn assert_in_stack(&self, stack: &NockStack) {
        match self.as_ref() {
            Ok(t_ref) => t_ref.assert_in_stack(stack),
            Err(e_ref) => e_ref.assert_in_stack(stack),
        }
    }
}

impl Preserve for bool {
    unsafe fn preserve(&mut self, _: &mut NockStack) {}

    unsafe fn assert_in_stack(&self, _: &NockStack) {}
}

impl Preserve for u32 {
    unsafe fn preserve(&mut self, _: &mut NockStack) {}

    unsafe fn assert_in_stack(&self, _: &NockStack) {}
}

impl Preserve for usize {
    unsafe fn preserve(&mut self, _: &mut NockStack) {}

    unsafe fn assert_in_stack(&self, _: &NockStack) {}
}

impl Preserve for AllocationError {
    unsafe fn preserve(&mut self, _: &mut NockStack) {}

    unsafe fn assert_in_stack(&self, _: &NockStack) {}
}

#[cfg(test)]
mod test {
    use std::iter::FromIterator;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    use super::*;
    use crate::jets::cold::test::{make_noun_list, make_test_stack};
    use crate::jets::cold::{NounList, Nounable};
    use crate::mem::NockStack;
    use crate::noun::D;

    fn test_noun_list_alloc_fn(
        stack_size: usize,
        item_count: u64,
    ) -> crate::jets::cold::NounableResult<()> {
        // fails at 512, works at 1024
        // const STACK_SIZE: usize = 1;
        // println!("TEST_SIZE: {}", STACK_SIZE);
        let mut stack = make_test_stack(stack_size);
        // Stack size 1 works until 15 elements, 14 passes, 15 fails.
        // const ITEM_COUNT: u64 = 15;
        let vec = Vec::from_iter(0..item_count);
        let items = vec.iter().map(|&x| D(x)).collect::<Vec<Noun>>();
        let slice = vec.as_slice();
        let noun_list = make_noun_list(&mut stack, slice);
        assert!(!noun_list.0.is_null());
        let noun = noun_list.into_noun(&mut stack);
        let new_noun_list: NounList =
            <NounList as Nounable>::from_noun::<NockStack>(&mut stack, &noun)?;
        let mut tracking_item_count = 0;
        println!("items: {:?}", items);
        for (a, b) in new_noun_list.zip(items.iter()) {
            // TODO: Maybe replace this with: https://doc.rust-lang.org/std/primitive.pointer.html#method.as_ref-1
            let a_val = unsafe { *a };
            println!("a: {:?}, b: {:?}", a_val, b);
            assert!(
                unsafe { (*a).raw_equals(b) },
                "Items don't match: {:?} {:?}",
                unsafe { *a },
                b
            );
            tracking_item_count += 1;
        }
        assert_eq!(tracking_item_count, item_count as usize);
        Ok(())
    }

    // cargo test -p nockvm test_noun_list_alloc -- --nocapture
    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_noun_list_alloc() {
        const PASSES: u64 = 72;
        const FAILS: u64 = 73;
        const STACK_SIZE: usize = 512;

        let should_fail_to_alloc = catch_unwind(|| test_noun_list_alloc_fn(STACK_SIZE, FAILS));
        assert!(should_fail_to_alloc
            .map_err(|err| err.is::<AllocationError>())
            .expect_err("Expected alloc error"));
        let should_succeed = test_noun_list_alloc_fn(STACK_SIZE, PASSES);
        assert!(should_succeed.is_ok());
    }

    // cargo test -p nockvm test_frame_push -- --nocapture
    #[test]
    fn test_frame_push() {
        // fails at 100, passes at 99, top_slots default to 100?
        const PASSES: usize = 503;
        const FAILS: usize = 504;
        const STACK_SIZE: usize = 512;
        let mut stack = make_test_stack(STACK_SIZE);
        let frame_push_res = catch_unwind(AssertUnwindSafe(|| stack.frame_push(FAILS)));
        assert!(frame_push_res
            .map_err(|err| err.is::<AllocationError>())
            .expect_err("Expected alloc error"));
        let mut stack = make_test_stack(STACK_SIZE);
        let frame_push_res = catch_unwind(AssertUnwindSafe(|| stack.frame_push(PASSES)));
        assert!(frame_push_res.is_ok());
    }

    // cargo test -p nockvm test_stack_push -- --nocapture
    #[test]
    fn test_stack_push() {
        const PASSES: usize = 506;
        const STACK_SIZE: usize = 512;
        let mut stack = make_test_stack(STACK_SIZE);
        let mut counter = 0;
        // Fails at 102, probably because top_slots is 100?
        while counter < PASSES {
            let push_res = catch_unwind(AssertUnwindSafe(|| unsafe { stack.push::<u64>() }));
            assert!(push_res.is_ok(), "Failed to push, counter: {}", counter);
            counter += 1;
        }
        let push_res = catch_unwind(AssertUnwindSafe(|| unsafe { stack.push::<u64>() }));
        assert!(push_res
            .map_err(|err| err.is::<AllocationError>())
            .expect_err("Expected alloc error"));
    }

    // cargo test -p nockvm test_frame_and_stack_push -- --nocapture
    #[test]
    fn test_frame_and_stack_push() {
        const STACK_SIZE: usize = 514; // to make sure of an odd space for the stack push
        const SUCCESS_PUSHES: usize = 101;
        let mut stack = make_test_stack(STACK_SIZE);
        let mut counter = 0;
        while counter < SUCCESS_PUSHES {
            let frame_push_res = catch_unwind(AssertUnwindSafe(|| stack.frame_push(1)));
            assert!(
                frame_push_res.is_ok(),
                "Failed to frame_push, counter: {}",
                counter
            );
            let push_res = catch_unwind(AssertUnwindSafe(|| unsafe { stack.push::<u64>() }));
            assert!(push_res.is_ok(), "Failed to push, counter: {}", counter);
            counter += 1;
        }
        let frame_push_res = catch_unwind(AssertUnwindSafe(|| stack.frame_push(1)));
        assert!(frame_push_res
            .map_err(|err| err.is::<AllocationError>())
            .expect_err("Expected alloc error"));
        // a single stack u64 push won't cause an error but a frame push will
        let push_res = catch_unwind(AssertUnwindSafe(|| unsafe { stack.push::<u64>() }));
        assert!(push_res.is_ok());
        // pushing an array of 1 u64 will NOT cause an error
        let push_res = catch_unwind(AssertUnwindSafe(|| unsafe { stack.push::<[u64; 1]>() }));
        assert!(push_res.is_ok());
        // pushing an array of 2 u64s WILL cause an error
        let push_res = catch_unwind(AssertUnwindSafe(|| unsafe { stack.push::<[u64; 2]>() }));
        assert!(push_res
            .map_err(|err| err.is::<AllocationError>())
            .expect_err("Expected alloc error"),);
    }

    // cargo test -p nockvm test_slot_pointer -- --nocapture
    // Test the slot_pointer checking by pushing frames and slots until we run out of space
    #[test]
    fn test_slot_pointer() {
        const STACK_SIZE: usize = 512;
        const SLOT_POINTERS: usize = 32;
        let mut stack = make_test_stack(STACK_SIZE);
        // let push_res: Result<*mut u64, AllocationError> = unsafe { stack.push::<u64>() };
        // let frame_push_res = catch_unwind(AssertUnwindSafe(|| stack.frame_push(SLOT_POINTERS)));
        // assert!(frame_push_res.is_ok());
        stack.frame_push(SLOT_POINTERS);
        let mut counter = 0;
        while counter < SLOT_POINTERS + RESERVED {
            println!("counter: {counter}");
            let slot_pointer_res =
                catch_unwind(AssertUnwindSafe(|| unsafe { stack.slot_pointer_(counter) }));
            assert!(
                slot_pointer_res.is_ok(),
                "Failed to slot_pointer, counter: {}",
                counter
            );
            counter += 1;
        }
        let slot_pointer_res =
            catch_unwind(AssertUnwindSafe(|| unsafe { stack.slot_pointer_(counter) }));
        assert!(slot_pointer_res
            .map_err(|err| err.is::<AllocationError>())
            .expect_err("Expected alloc error"),);
    }

    // cargo test -p nockvm test_prev_alloc -- --nocapture
    // Test the alloc in previous frame checking by pushing a frame and then allocating in the previous frame until we run out of space
    #[test]
    fn test_prev_alloc() {
        const STACK_SIZE: usize = 512;
        const SUCCESS_ALLOCS: usize = 503;
        let mut stack = make_test_stack(STACK_SIZE);
        println!("\n############## frame push \n");
        let frame_push_res = catch_unwind(AssertUnwindSafe(|| stack.frame_push(0)));
        assert!(frame_push_res.is_ok());
        let pre_copy_res = catch_unwind(AssertUnwindSafe(|| unsafe { stack.pre_copy() }));
        assert!(pre_copy_res.is_ok());
        let mut counter = 0;

        while counter < SUCCESS_ALLOCS {
            println!("counter: {counter}");
            let prev_alloc_res = catch_unwind(AssertUnwindSafe(|| unsafe {
                stack.raw_alloc_in_previous_frame(1)
            }));
            assert!(
                prev_alloc_res.is_ok(),
                "Failed to prev_alloc, counter: {}",
                counter
            );
            counter += 1;
        }
        println!("### This next raw_alloc_in_previous_frame should fail ###\n");
        let prev_alloc_res = catch_unwind(AssertUnwindSafe(|| unsafe {
            stack.raw_alloc_in_previous_frame(1)
        }));
        assert!(
            prev_alloc_res
                .map_err(|err| err.is::<AllocationError>())
                .expect_err("Expected alloc error"),
            "Didn't get expected alloc error",
        );
    }
}
