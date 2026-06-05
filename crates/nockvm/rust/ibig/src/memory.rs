//! Memory allocation.

use alloc::alloc::Layout;
use core::marker::PhantomData;
use core::{mem, slice};

/// Chunk of memory directly allocated from the global allocator.
pub(crate) struct MemoryAllocation {
    layout: Layout,
    start: *mut u8,
}

pub trait Stack: Sized {
    // no-std bites me in the keister again
    unsafe fn alloc_layout(&mut self, layout: Layout) -> *mut u64;
}

/// Chunk of memory.
pub(crate) struct Memory<'a> {
    /// Start pointer.
    start: *mut u8,
    /// End pointer.
    end: *mut u8,
    /// Logically, Memory contains a reference to some data with lifetime 'a.
    phantom_data: PhantomData<&'a mut ()>,
}

impl MemoryAllocation {
    pub(crate) fn new_stack<S: Stack>(stack: &mut S, layout: Layout) -> MemoryAllocation {
        let start = if layout.size() == 0 {
            // We should use layout.dangling(), but that is unstable.
            layout.align() as *mut u8
        } else if layout.size() > isize::MAX as usize {
            panic_out_of_memory()
        } else {
            // Safe because size is non-zero.
            let ptr = unsafe {
                let ep = stack.alloc_layout(layout);
                ep as *mut u8
            };
            if ptr.is_null() {
                panic_out_of_memory();
            }
            ptr
        };

        MemoryAllocation { layout, start }
    }

    /// Allocate memory.
    pub(crate) fn new(layout: Layout) -> MemoryAllocation {
        let start = if layout.size() == 0 {
            // We should use layout.dangling(), but that is unstable.
            layout.align() as *mut u8
        } else if layout.size() > isize::MAX as usize {
            panic_out_of_memory()
        } else {
            // Safe because size is non-zero.
            let ptr = unsafe { alloc::alloc::alloc(layout) };
            if ptr.is_null() {
                panic_out_of_memory();
            }
            ptr
        };

        MemoryAllocation { layout, start }
    }

    /// Get memory.
    #[inline]
    pub(crate) fn memory(&mut self) -> Memory {
        Memory {
            start: self.start,
            end: self.start.wrapping_add(self.layout.size()),
            phantom_data: PhantomData,
        }
    }
}

impl Drop for MemoryAllocation {
    fn drop(&mut self) {
        if self.layout.size() != 0 {
            // Safe because the memory was allocated with the same layout.
            unsafe { alloc::alloc::dealloc(self.start, self.layout) };
        }
    }
}

impl Memory<'_> {
    /// Allocate a slice with a given value.
    ///
    /// Returns the remaining chunk of memory.
    ///
    /// The original memory is not usable until both the new memory and the slice are dropped.
    ///
    /// The elements of the slice never get dropped!
    pub(crate) fn allocate_slice_fill<T: Copy>(&mut self, n: usize, val: T) -> (&mut [T], Memory) {
        self.allocate_slice_initialize::<T, _>(n, |ptr| {
            for i in 0..n {
                // Safe because ptr is properly aligned and has enough space.
                unsafe {
                    ptr.add(i).write(val);
                };
            }
        })
    }

    /// Allocate a slice by copying another slice.
    ///
    /// Returns the remaining chunk of memory.
    ///
    /// The original memory is not usable until both the new memory and the slice are dropped.
    ///
    /// The elements of the slice never get dropped!
    pub(crate) fn allocate_slice_copy<T: Copy>(&mut self, source: &[T]) -> (&mut [T], Memory) {
        self.allocate_slice_initialize::<T, _>(source.len(), |ptr| {
            for (i, v) in source.iter().enumerate() {
                // Safe because ptr is properly aligned and has enough space.
                unsafe {
                    ptr.add(i).write(*v);
                };
            }
        })
    }

    /// Allocate a slice by copying a smaller slice and filling the remainder with a value.
    ///
    /// Returns the remaining chunk of memory.
    ///
    /// The original memory is not usable until both the new memory and the slice are dropped.
    ///
    /// The elements of the slice never get dropped!
    pub(crate) fn allocate_slice_copy_fill<T: Copy>(
        &mut self,
        n: usize,
        source: &[T],
        val: T,
    ) -> (&mut [T], Memory) {
        assert!(n >= source.len());

        self.allocate_slice_initialize::<T, _>(n, |ptr| {
            for (i, v) in source.iter().enumerate() {
                // Safe because ptr is properly aligned and has enough space.
                unsafe {
                    ptr.add(i).write(*v);
                };
            }
            for i in source.len()..n {
                // Safe because ptr is properly aligned and has enough space.
                unsafe {
                    ptr.add(i).write(val);
                };
            }
        })
    }

    fn allocate_slice_initialize<T, F>(&mut self, n: usize, init: F) -> (&mut [T], Memory)
    where
        F: FnOnce(*mut T),
    {
        #[allow(clippy::redundant_closure)]
        let (ptr, slice_end) = self
            .try_find_memory_for_slice::<T>(n)
            .unwrap_or_else(|| panic_allocated_too_little());

        init(ptr);

        // Safe because ptr is properly sized and aligned and has been initialized.
        let slice = unsafe { slice::from_raw_parts_mut(ptr, n) };
        let new_memory = Memory {
            start: slice_end,
            end: self.end,
            phantom_data: PhantomData,
        };

        (slice, new_memory)
    }

    fn try_find_memory_for_slice<T>(&self, n: usize) -> Option<(*mut T, *mut u8)> {
        let start = self.start as usize;
        let end = self.end as usize;

        let padding = start.wrapping_neg() & (mem::align_of::<T>() - 1);
        let slice_start = start.checked_add(padding)?;
        let size = n.checked_mul(mem::size_of::<T>())?;
        let slice_end = slice_start.checked_add(size)?;
        if slice_end <= end {
            Some((slice_start as *mut T, slice_end as *mut u8))
        } else {
            None
        }
    }
}

#[inline]
pub(crate) fn zero_layout() -> Layout {
    Layout::from_size_align(0, 1).unwrap_or_else(|err| {
        panic!(
            "Panicked with {err:?} at {}:{} (git sha: {:?})",
            file!(),
            line!(),
            option_env!("GIT_SHA")
        )
    })
}

pub(crate) fn array_layout<T>(n: usize) -> Layout {
    Layout::array::<T>(n).unwrap_or_else(|_| panic_out_of_memory())
}

pub(crate) fn add_layout(a: Layout, b: Layout) -> Layout {
    let (layout, _padding) = a.extend(b).unwrap_or_else(|_| panic_out_of_memory());
    layout
}

pub(crate) fn max_layout(a: Layout, b: Layout) -> Layout {
    Layout::from_size_align(a.size().max(b.size()), a.align().max(b.align()))
        .unwrap_or_else(|_| panic_out_of_memory())
}

pub(crate) fn panic_out_of_memory() -> ! {
    panic!("out of memory")
}

fn panic_allocated_too_little() -> ! {
    panic!("internal error: not enough memory allocated")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory() {
        let mut scratchpad =
            MemoryAllocation::new(Layout::from_size_align(8, 4).unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            }));
        let mut memory = scratchpad.memory();
        let (a, mut new_memory) = memory.allocate_slice_fill::<u32>(1, 3);
        assert_eq!(a, &[3]);
        // Neither of these should compile:
        // let _ = scratchpad.memory();
        // let _ = memory.allocate_slice::<u32>(1, 3);
        let (b, _) = new_memory.allocate_slice_fill::<u32>(1, 4);
        assert_eq!(b, &[4]);
        // Now we can reuse the memory.
        let (c, _) = memory.allocate_slice_copy::<u32>(&[4, 5]);
        assert_eq!(c, &[4, 5]);
        // Reuse the memory again.
        let (c, _) = memory.allocate_slice_copy_fill::<u32>(2, &[4], 7);
        assert_eq!(c, &[4, 7]);
    }

    #[test]
    #[should_panic]
    fn test_memory_ran_out() {
        let mut scratchpad =
            MemoryAllocation::new(Layout::from_size_align(8, 4).unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            }));
        let mut memory = scratchpad.memory();
        let (a, mut new_memory) = memory.allocate_slice_fill::<u32>(1, 3);
        assert_eq!(a, &[3]);
        let _ = new_memory.allocate_slice_fill::<u32>(2, 4);
    }

    #[test]
    fn test_add_layout() {
        let layout = add_layout(
            Layout::from_size_align(1, 1).unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            }),
            Layout::from_size_align(8, 4).unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            }),
        );
        assert_eq!(layout.size(), 12);
        assert_eq!(layout.align(), 4);
    }

    #[test]
    fn test_max_layout() {
        let layout = max_layout(
            Layout::from_size_align(100, 1).unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            }),
            Layout::from_size_align(8, 4).unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            }),
        );
        assert_eq!(layout.size(), 100);
        assert_eq!(layout.align(), 4);
    }
}
