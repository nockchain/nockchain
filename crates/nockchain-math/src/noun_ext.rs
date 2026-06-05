use nockvm::noun::{IndirectAtom, NounAllocator, Result};

use crate::belt::Belt;
use crate::felt::Felt;

// Note: since these are methods for converting to other types,
// their implementations are found in hand/convert
pub trait AtomMathExt {
    fn as_u32(&self) -> Result<u32>;
    fn as_belt(&self) -> Result<Belt>;
    fn as_felt<'a>(&self) -> Result<&'a Felt>;
    fn as_mut_felt<'a>(&self) -> Result<&'a mut Felt>;
}

// Note: since these are methods for converting to other types,
// their implementations are found in hand/convert
pub trait NounMathExt: Sized {
    fn as_belt(&self) -> Result<Belt>;
    fn as_felt<'a>(&self) -> Result<&'a Felt>;
    fn as_mut_felt<'a>(&self) -> Result<&'a mut Felt>;
    fn uncell<const N: usize>(&self) -> Result<[Self; N]>;
}

pub trait IndirectAtomExt {
    /// # Safety
    /// The caller must ensure that the size is a multiple of 8 and that the
    /// resulting memory is properly aligned for 64-bit access.
    /// The caller must also ensure that the memory is not accessed after the
    /// allocator is dropped.
    unsafe fn new_raw_mut_words<'a, A: NounAllocator>(
        allocator: &mut A,
        size: usize,
    ) -> (Self, &'a mut [u64])
    where
        Self: Sized;
}

impl IndirectAtomExt for IndirectAtom {
    /** Make an indirect atom that can be written into as a slice of machine
     * words. The constraints of [new_raw_mut_zeroed] also apply here
     *
     * Note: size is 64 bit machine words
     */
    unsafe fn new_raw_mut_words<'a, A: NounAllocator>(
        allocator: &mut A,
        size: usize,
    ) -> (Self, &'a mut [u64]) {
        let (noun, ptr) = Self::new_raw_mut_zeroed(allocator, size);
        (noun, std::slice::from_raw_parts_mut(ptr, size))
    }
}
