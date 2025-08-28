//! Word buffer.

use alloc::vec::Vec;
use core::iter;
use core::mem::ManuallyDrop;
use core::ops::{Deref, DerefMut};

use crate::arch::word::Word;
use crate::memory;
use crate::memory::Stack;
use crate::ubig::UBig;

/// Buffer for Words.
///
/// UBig operations are usually performed by creating a Buffer with appropriate capacity, filling it
/// in with Words, and then converting to UBig.
///
/// If its capacity is exceeded, the `Buffer` will panic.
///
/// ManuallyDrop so that we don't try to deallocate the buffer when it goes out of scope.
#[derive(Debug, Eq, Hash, PartialEq)]
pub(crate) struct Buffer(ManuallyDrop<Vec<Word>>);

impl Buffer {
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    pub(crate) fn allocate_stack<S: Stack>(stack: &mut S, num_words: usize) -> Buffer {
        if num_words > Buffer::MAX_CAPACITY {
            UBig::panic_number_too_large();
        }
        let capacity = Buffer::default_capacity(num_words);
        unsafe {
            let ptr = stack.alloc_layout(memory::array_layout::<Word>(capacity));
            Buffer(ManuallyDrop::new(Vec::from_raw_parts(ptr, 0, capacity)))
        }
    }

    /// Creates a `Buffer` with at least specified capacity.
    ///
    /// It leaves some extra space for future growth.
    pub(crate) fn allocate(num_words: usize) -> Buffer {
        if num_words > Buffer::MAX_CAPACITY {
            UBig::panic_number_too_large();
        }
        Buffer(ManuallyDrop::new(Vec::with_capacity(
            Buffer::default_capacity(num_words),
        )))
    }

    pub(crate) fn ensure_capacity_stack<S: Stack>(&mut self, stack: &mut S, num_words: usize) {
        if num_words > self.capacity() {
            self.reallocate_stack(stack, num_words)
        }
    }

    /// Ensure there is enough capacity in the buffer for `num_words`. Will reallocate if there is
    /// not enough.
    #[inline]
    pub(crate) fn ensure_capacity(&mut self, num_words: usize) {
        if num_words > self.capacity() {
            self.reallocate(num_words);
        }
    }

    /// Makes sure that the capacity is compact.
    #[inline]
    pub(crate) fn shrink(&mut self) {
        // We don't want to shrink anything allocated with _stack
        //
        // if self.capacity() > Buffer::max_compact_capacity(self.len()) {
        //     self.reallocate(self.len());
        // }
    }

    fn reallocate_stack<S: Stack>(&mut self, stack: &mut S, num_words: usize) {
        assert!(num_words >= self.len());
        let mut new_buffer = Buffer::allocate_stack(stack, num_words);
        new_buffer.0.extend_from_slice(self);

        *self = new_buffer;
    }

    /// Change capacity to store `num_words` plus some extra space for future growth.
    ///
    /// # Panics
    ///
    /// Panics if `num_words < len()`.
    fn reallocate(&mut self, num_words: usize) {
        assert!(num_words >= self.len());
        let mut new_buffer = Buffer::allocate(num_words);
        new_buffer.0.extend_from_slice(self);
        *self = new_buffer
    }

    /// Return buffer capacity.
    #[inline]
    pub(crate) fn capacity(&self) -> usize {
        self.0.capacity()
    }

    /// Append a Word to the buffer.
    ///
    /// # Panics
    ///
    /// Panics if there is not enough capacity.
    #[inline]
    pub(crate) fn push(&mut self, word: Word) {
        assert!(self.len() < self.capacity());
        self.0.push(word);
    }

    #[inline]
    pub(crate) fn push_may_reallocate_stack<S: Stack>(&mut self, stack: &mut S, word: Word) {
        self.ensure_capacity_stack(stack, self.len() + 1);
        self.push(word);
    }

    /// Append a Word and reallocate if necessary.
    #[inline]
    pub(crate) fn push_may_reallocate(&mut self, word: Word) {
        self.ensure_capacity(self.len() + 1);
        self.push(word);
    }

    /// Append `n` zeros.
    ///
    /// # Panics
    ///
    /// Panics if there is not enough capacity.
    pub(crate) fn push_zeros(&mut self, n: usize) {
        assert!(n <= self.capacity() - self.len());
        self.0.extend(iter::repeat(0).take(n));
    }

    /// Insert `n` zeros in front.
    ///
    /// # Panics
    ///
    /// Panics if there is not enough capacity.
    pub(crate) fn push_zeros_front(&mut self, n: usize) {
        assert!(n <= self.capacity() - self.len());
        self.0.splice(..0, iter::repeat(0).take(n));
    }

    /// Pop the most significant `Word`.
    #[inline]
    pub(crate) fn pop(&mut self) -> Option<Word> {
        self.0.pop()
    }

    /// Pop leading zero words.
    #[inline]
    pub(crate) fn pop_leading_zeros(&mut self) {
        while let Some(0) = self.last() {
            self.pop();
        }
    }

    #[inline]
    /// Truncate length to `len`.
    pub(crate) fn truncate(&mut self, len: usize) {
        assert!(self.len() >= len);

        self.0.truncate(len);
    }

    /// Erase first n elements.
    pub(crate) fn erase_front(&mut self, n: usize) {
        assert!(self.len() >= n);

        self.0.drain(..n);
    }

    /// Clone from `other` and resize if necessary.
    ///
    /// Equivalent to, but more efficient than:
    ///
    /// ```ignore
    /// buffer.ensure_capacity(source.len());
    /// buffer.clone_from(source);
    /// buffer.shrink();
    /// ```
    pub(crate) fn resizing_clone_from(&mut self, source: &Buffer) {
        let cap = self.capacity();
        let n = source.len();
        if cap >= n && cap <= Buffer::max_compact_capacity(n) {
            self.clone_from(source);
        } else {
            *self = source.clone();
        }
    }

    /// Maximum number of `Word`s.
    ///
    /// We allow 4 extra words beyond `UBig::MAX_LEN` to allow temporary space in operations.
    pub(crate) const MAX_CAPACITY: usize = UBig::MAX_LEN + 4;

    /// Default capacity for a given number of `Word`s.
    /// It should be between `num_words` and `max_capacity(num_words).
    ///
    /// Requires that `num_words <= MAX_CAPACITY`.
    ///
    /// Provides `2 + 0.125 * num_words` extra space.
    #[inline]
    fn default_capacity(num_words: usize) -> usize {
        debug_assert!(num_words <= Buffer::MAX_CAPACITY);
        (num_words + num_words / 8 + 2).min(Buffer::MAX_CAPACITY)
    }

    /// Maximum compact capacity for a given number of `Word`s.
    ///
    /// Requires that `num_words <= Buffer::MAX_CAPACITY`.
    ///
    /// Allows `4 + 0.25 * num_words` overhead.
    #[inline]
    fn max_compact_capacity(num_words: usize) -> usize {
        debug_assert!(num_words <= Buffer::MAX_CAPACITY);
        (num_words + num_words / 4 + 4).min(Buffer::MAX_CAPACITY)
    }
}

impl Clone for Buffer {
    /// New buffer will be sized as `Buffer::allocate(self.len())`.
    fn clone(&self) -> Buffer {
        let mut new_buffer = Buffer::allocate(self.len());
        new_buffer.clone_from(self);
        new_buffer
    }

    /// If capacity is exceeded, panic.
    #[inline]
    fn clone_from(&mut self, source: &Buffer) {
        assert!(self.capacity() >= source.len());
        self.0.clone_from(&source.0);
    }
}

impl Deref for Buffer {
    type Target = [Word];

    #[inline]
    fn deref(&self) -> &[Word] {
        &self.0
    }
}

impl DerefMut for Buffer {
    #[inline]
    fn deref_mut(&mut self) -> &mut [Word] {
        &mut self.0
    }
}

impl Extend<Word> for Buffer {
    fn extend<T>(&mut self, iter: T)
    where
        T: IntoIterator<Item = Word>,
    {
        for word in iter {
            self.push(word);
        }
    }
}

impl<'a> Extend<&'a Word> for Buffer {
    fn extend<T>(&mut self, iter: T)
    where
        T: IntoIterator<Item = &'a Word>,
    {
        for word in iter {
            self.push(*word);
        }
    }
}

// FIXME: ibig makes invalid assumptions about how Vec works, we need to audit this.
#[cfg(test)]
mod tests {
    use alloc::alloc;
    use core::alloc::Layout;

    use super::*;
    use crate::memory::Stack;

    // Simple stack implementation for testing
    struct TestStack;

    impl TestStack {
        fn new() -> Self {
            TestStack
        }
    }

    impl Stack for TestStack {
        unsafe fn alloc_layout(&mut self, layout: Layout) -> *mut u64 {
            if layout.size() == 0 {
                layout.dangling().as_ptr()
            } else {
                let ptr = alloc::alloc(layout);
                ptr as *mut u64
            }
        }
    }

    //     #[test]
    //     fn test_default_capacity() {
    //         assert_eq!(Buffer::default_capacity(2), 4);
    //         assert_eq!(Buffer::default_capacity(1000), 1127);
    //     }

    //     #[test]
    //     fn test_max_compact_capacity() {
    //         assert_eq!(Buffer::max_compact_capacity(2), 6);
    //         assert_eq!(Buffer::max_compact_capacity(1000), 1254);
    //     }

    //     #[test]
    //     fn test_allocate() {
    //         let buffer = Buffer::allocate(1000);
    //         assert_eq!(buffer.len(), 0);
    //         assert_eq!(buffer.capacity(), Buffer::default_capacity(1000));
    //     }

    //     #[test]
    //     #[should_panic]
    //     fn test_allocate_too_large() {
    //         let _ = Buffer::allocate(Buffer::MAX_CAPACITY + 1);
    //     }

    //     #[test]
    //     fn test_ensure_capacity() {
    //         let mut buffer = Buffer::allocate(2);
    //         buffer.push(7);
    //         assert_eq!(buffer.capacity(), 4);
    //         buffer.ensure_capacity(4);
    //         assert_eq!(buffer.capacity(), 4);
    //         buffer.ensure_capacity(5);
    //         assert_eq!(buffer.capacity(), 7);
    //         assert_eq!(&buffer[..], [7]);
    //     }

    //     #[test]
    //     fn test_shrink() {
    //         let mut buffer = Buffer::allocate(100);
    //         buffer.push(7);
    //         buffer.push(8);
    //         buffer.shrink();
    //         assert_eq!(buffer.capacity(), Buffer::default_capacity(2));
    //         assert_eq!(&buffer[..], [7, 8]);
    //     }

    //     #[test]
    //     fn test_push_pop() {
    //         let mut buffer = Buffer::allocate(5);
    //         buffer.push(1);
    //         buffer.push(2);
    //         assert_eq!(&buffer[..], [1, 2]);
    //         assert_eq!(buffer.pop(), Some(2));
    //         assert_eq!(buffer.pop(), Some(1));
    //         assert_eq!(buffer.pop(), None);
    //     }

    //     #[test]
    //     fn test_pop_leading_zeros() {
    //         let mut buffer = Buffer::allocate(5);
    //         buffer.push(1);
    //         buffer.push(2);
    //         buffer.push(0);
    //         buffer.push(0);
    //         buffer.pop_leading_zeros();
    //         assert_eq!(&buffer[..], [1, 2]);
    //     }

    //     #[test]
    //     fn test_extend() {
    //         let mut buffer = Buffer::allocate(5);
    //         buffer.push(1);
    //         let list: [Word; 2] = [2, 3];
    //         buffer.extend(&list);
    //         assert_eq!(&buffer[..], [1, 2, 3]);
    //     }

    //     #[test]
    //     fn test_push_zeros() {
    //         let mut buffer = Buffer::allocate(5);
    //         buffer.push(1);
    //         buffer.push_zeros(2);
    //         assert_eq!(&buffer[..], [1, 0, 0]);
    //     }

    //     #[test]
    //     fn test_push_zeros_front() {
    //         let mut buffer = Buffer::allocate(5);
    //         buffer.push(1);
    //         buffer.push_zeros_front(2);
    //         assert_eq!(&buffer[..], [0, 0, 1]);
    //     }

    //     #[test]
    //     fn test_truncate() {
    //         let mut buffer = Buffer::allocate(5);
    //         buffer.push(1);
    //         buffer.push(2);
    //         buffer.push(3);
    //         buffer.truncate(1);
    //         assert_eq!(&buffer[..], [1]);
    //     }

    //     #[test]
    //     fn test_erase_front() {
    //         let mut buffer = Buffer::allocate(5);
    //         buffer.push(1);
    //         buffer.push(2);
    //         buffer.push(3);
    //         buffer.erase_front(2);
    //         assert_eq!(&buffer[..], [3]);
    //     }

    //     #[test]
    //     #[should_panic]
    //     fn test_push_failed() {
    //         let mut buffer = Buffer::allocate(2);
    //         for _ in 0..10 {
    //             buffer.push(7);
    //         }
    //     }

    // This is a regression test for a bug where push_may_reallocate did not
    // increase the capacity of the buffer
    #[test]
    fn test_push_may_reallocate() {
        let mut buffer = Buffer::allocate(2);
        for _ in 0..10 {
            buffer.push_may_reallocate(7);
        }
        assert_eq!(buffer.len(), 10);
    }

    // This is a regression test for a bug where push_may_reallocate_stack did not
    // increase the capacity of the buffer
    #[test]
    fn test_push_may_reallocate_stack() {
        // Create a test stack for testing
        let mut stack = TestStack::new();

        // Create a buffer with small initial capacity
        let mut buffer = Buffer::allocate_stack(&mut stack, 2);

        // Push more elements than the initial capacity
        for i in 0..10 {
            buffer.push_may_reallocate_stack(&mut stack, i as Word);
        }
    }

    //     #[test]
    //     fn test_clone() {
    //         let mut buffer = Buffer::allocate(100);
    //         buffer.push(7);
    //         buffer.push(8);
    //         let buffer2 = buffer.clone();
    //         assert_eq!(buffer, buffer2);
    //         assert_eq!(buffer2.capacity(), Buffer::default_capacity(2));
    //     }

    //     #[test]
    //     fn test_clone_from() {
    //         let mut buffer = Buffer::allocate(100);
    //         buffer.push(7);
    //         buffer.push(8);
    //         let mut buffer2 = Buffer::allocate(50);
    //         buffer2.clone_from(&buffer);
    //         assert_eq!(buffer, buffer2);
    //         assert_eq!(buffer2.capacity(), Buffer::default_capacity(50));
    //     }

    //     #[test]
    //     fn test_resizing_clone_from() {
    //         let mut buf = Buffer::allocate(5);
    //         assert_eq!(buf.capacity(), 7);

    //         let mut buf2 = Buffer::allocate(4);
    //         assert_eq!(buf2.capacity(), 6);
    //         for i in 0..4 {
    //             buf2.push(i);
    //         }
    //         buf.resizing_clone_from(&buf2);
    //         assert_eq!(buf.capacity(), 7);
    //         assert_eq!(&buf[..], [0, 1, 2, 3]);

    //         let mut buf3 = Buffer::allocate(100);
    //         for i in 0..100 {
    //             buf3.push(i);
    //         }
    //         buf.resizing_clone_from(&buf3);
    //         assert_eq!(buf.capacity(), Buffer::default_capacity(100));
    //         assert_eq!(buf.len(), 100);

    //         buf.resizing_clone_from(&buf2);
    //         assert_eq!(buf.capacity(), 6);
    //         assert_eq!(&buf[..], [0, 1, 2, 3]);
    //     }
}
