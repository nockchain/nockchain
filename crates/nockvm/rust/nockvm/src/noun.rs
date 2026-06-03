#![allow(clippy::missing_safety_doc)]

use std::slice::{from_raw_parts, from_raw_parts_mut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::{error, fmt, ptr, str};

use bitvec::prelude::{BitSlice, Lsb0};
use either::{Either, Left, Right};
use ibig::{Stack, UBig};
use intmap::IntMap;
use nockvm_macros::tas;
use static_assertions::assert_cfg;

use crate::mem::{word_size_of, Arena, NockStack};
use crate::offset::{PmaOffsetWords, WordOffset};
use crate::pma::Pma;

crate::gdb!();

assert_cfg!(
    target_endian = "little",
    "nockvm will not execute correctly on non-little-endian systems"
);

/** Tag for a direct atom. */
pub(crate) const DIRECT_TAG: u64 = 0x0;

/** Tag mask for a direct atom. */
pub(crate) const DIRECT_MASK: u64 = !(u64::MAX >> 1);

/** Maximum value of a direct atom. Values higher than this must be represented by indirect atoms. */
pub const DIRECT_MAX: u64 = u64::MAX >> 1;

/** Tag for an indirect atom. */
pub(crate) const INDIRECT_TAG: u64 = DIRECT_MASK;

/** Tag mask for an indirect atom. */
pub(crate) const INDIRECT_MASK: u64 = !(u64::MAX >> 2);

/** Tag for a cell. */
pub(crate) const CELL_TAG: u64 = INDIRECT_MASK;

/** Tag mask for a cell. */
pub(crate) const CELL_MASK: u64 = !(u64::MAX >> 3);

pub(crate) const LOCATION_BIT: u64 = 1 << 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PtrLocation {
    Stack,
    Offset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocLocation {
    Stack,
    PmaPtr,
    PmaOffset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NounRepr {
    Direct,
    Indirect(AllocLocation),
    Cell(AllocLocation),
    Forwarding(AllocLocation),
}

impl AllocLocation {
    pub fn is_stack(self) -> bool {
        matches!(self, AllocLocation::Stack)
    }

    pub fn is_pma(self) -> bool {
        matches!(self, AllocLocation::PmaPtr | AllocLocation::PmaOffset)
    }

    pub fn is_offset(self) -> bool {
        matches!(self, AllocLocation::PmaOffset)
    }
}

impl NounRepr {
    pub fn is_allocated(self) -> bool {
        !matches!(self, NounRepr::Direct)
    }

    pub fn location(self) -> Option<AllocLocation> {
        match self {
            NounRepr::Direct => None,
            NounRepr::Indirect(loc) => Some(loc),
            NounRepr::Cell(loc) => Some(loc),
            NounRepr::Forwarding(loc) => Some(loc),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TaggedPtr(u64);

impl TaggedPtr {
    #[inline(always)]
    fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    #[inline(always)]
    unsafe fn from_stack_ptr(ptr: *const u8, tag: u64) -> Self {
        debug_assert!(
            (ptr as usize) & 0x7 == 0,
            "Stack pointer {:p} not 8-byte aligned",
            ptr
        );
        Self(((ptr as u64) >> 3) | tag)
    }

    #[inline(always)]
    fn from_offset<O: WordOffset>(words: O, tag: u64) -> Self {
        assert!(
            words.words() < LOCATION_BIT,
            "offset {} exceeds payload capacity",
            words.words()
        );
        Self(words.words() | LOCATION_BIT | tag)
    }

    #[inline(always)]
    fn location(self) -> PtrLocation {
        if self.0 & LOCATION_BIT == 0 {
            PtrLocation::Stack
        } else {
            PtrLocation::Offset
        }
    }

    #[inline(always)]
    fn payload(self, mask: u64) -> u64 {
        self.0 & !(mask | LOCATION_BIT)
    }

    #[inline(always)]
    fn resolve_const_trusted(self, mask: u64, space: &NounSpace) -> *const u8 {
        let payload = self.0 & !(mask | LOCATION_BIT);
        if self.0 & LOCATION_BIT == 0 {
            let payload = usize::try_from(
                payload
                    .checked_shl(3)
                    .expect("stack pointer payload exceeds addressable range"),
            )
            .expect("stack pointer payload exceeds usize addressable range");
            payload as *const u8
        } else {
            let base = unsafe { space.pma_base_unchecked() };
            let offset = PmaOffsetWords::from_words(payload)
                .checked_bytes_usize()
                .expect("PMA offset exceeds addressable range");
            base.checked_add(offset)
                .expect("PMA offset exceeds addressable range") as *const u8
        }
    }

    fn resolve_const(self, mask: u64, space: &NounSpace) -> *const u8 {
        match self.location() {
            PtrLocation::Stack => space.resolve_stack_ptr(self.payload(mask)),
            PtrLocation::Offset => {
                space.resolve_pma_ptr(PmaOffsetWords::from_words(self.payload(mask)))
            }
        }
    }

    #[inline(always)]
    fn resolve_mut(self, mask: u64, space: &NounSpace) -> *mut u8 {
        self.resolve_const(mask, space) as *mut u8
    }

    #[inline(always)]
    fn raw(self) -> u64 {
        self.0
    }
}

pub struct NounSpace {
    stack: Option<Arc<Arena>>,
    pma: Option<Arc<Arena>>,
    stack_epoch: Option<Arc<AtomicU64>>,
    stack_epoch_ptr: Option<*const AtomicU64>,
    stack_epoch_snapshot: Option<u64>,
    stack_base: Option<usize>,
    stack_end: Option<usize>,
    pma_base: Option<usize>,
    pma_end: Option<usize>,
    pma_words: Option<usize>,
    extra_ptr_ranges: Vec<(usize, usize)>,
}

impl NounSpace {
    #[inline]
    fn arena_cache(arena: Option<&Arena>) -> (Option<usize>, Option<usize>, Option<usize>) {
        if let Some(arena) = arena {
            let base = arena.base_ptr() as usize;
            let end = base
                .checked_add(arena.len_bytes())
                .expect("arena bounds exceed usize address space");
            (Some(base), Some(end), Some(arena.words()))
        } else {
            (None, None, None)
        }
    }

    #[inline]
    fn pma_arena_cache(arena: Option<&Arena>) -> (Option<usize>, Option<usize>, Option<usize>) {
        if let Some(arena) = arena {
            let base = arena.base_ptr() as usize;
            let end = base
                .checked_add(arena.reserved_len_bytes())
                .expect("PMA reserved bounds exceed usize address space");
            (Some(base), Some(end), Some(arena.words()))
        } else {
            (None, None, None)
        }
    }

    pub fn from_arenas(stack: Option<Arc<Arena>>, pma: Option<Arc<Arena>>) -> Self {
        let (stack_base, stack_end, _) = Self::arena_cache(stack.as_deref());
        let (pma_base, pma_end, pma_words) = Self::pma_arena_cache(pma.as_deref());
        Self {
            stack,
            pma,
            stack_epoch: None,
            stack_epoch_ptr: None,
            stack_epoch_snapshot: None,
            stack_base,
            stack_end,
            pma_base,
            pma_end,
            pma_words,
            extra_ptr_ranges: Vec::new(),
        }
    }

    pub fn from_stack(stack: &NockStack, pma: Option<Arc<Arena>>) -> Self {
        let stack_arena = Arc::clone(stack.arena());
        let stack_epoch = stack.stack_epoch();
        let (stack_base, stack_end, _) = Self::arena_cache(Some(stack.arena_ref()));
        let (pma_base, pma_end, pma_words) = Self::pma_arena_cache(pma.as_deref());
        Self {
            stack: Some(stack_arena),
            pma,
            stack_epoch: Some(stack_epoch),
            stack_epoch_ptr: Some(stack.stack_epoch_ref() as *const AtomicU64),
            stack_epoch_snapshot: Some(stack.stack_epoch_snapshot()),
            stack_base,
            stack_end,
            pma_base,
            pma_end,
            pma_words,
            extra_ptr_ranges: Vec::new(),
        }
    }

    pub(crate) fn from_stack_ephemeral(stack: &NockStack) -> Self {
        let (stack_base, stack_end, _) = Self::arena_cache(Some(stack.arena_ref()));
        let (pma_base, pma_end, pma_words) = Self::pma_arena_cache(stack.pma_ref());
        Self {
            stack: None,
            pma: None,
            stack_epoch: None,
            stack_epoch_ptr: Some(stack.stack_epoch_ref() as *const AtomicU64),
            stack_epoch_snapshot: Some(stack.stack_epoch_snapshot()),
            stack_base,
            stack_end,
            pma_base,
            pma_end,
            pma_words,
            extra_ptr_ranges: Vec::new(),
        }
    }

    #[inline(always)]
    unsafe fn pma_base_unchecked(&self) -> usize {
        debug_assert!(
            self.pma_base.is_some(),
            "PMA arena is required to resolve offset nouns"
        );
        self.pma_base.unwrap_unchecked()
    }

    pub fn new(stack: &NockStack, pma: &Pma) -> Self {
        Self::from_stack(stack, Some(Arc::clone(pma.arena())))
    }

    pub fn stack_only(stack: &NockStack) -> Self {
        Self::from_stack(stack, None)
    }

    pub fn pma_only(pma: &Pma) -> Self {
        Self::from_arenas(None, Some(Arc::clone(pma.arena())))
    }

    pub fn empty() -> Self {
        Self::from_arenas(None, None)
    }

    pub fn with_extra_ptr_ranges(mut self, ranges: Vec<(usize, usize)>) -> Self {
        self.extra_ptr_ranges = ranges;
        self
    }

    pub fn handle<'a>(&'a self, noun: Noun) -> NounHandle<'a> {
        NounHandle::new(noun, self)
    }

    /// Introduce a fresh generative brand tied to this exact `NounSpace` for the duration of `f`.
    ///
    /// This is an experimental proof-of-concept layer that sits alongside the existing unbranded
    /// `Noun`/`NounHandle` APIs. Code inside the closure can only combine branded handles that came
    /// from the same branded `NounSpace`.
    ///
    /// Valid code continues to type-check within one branded scope:
    ///
    /// ```
    /// use nockvm::mem::{NockStack, NOCK_STACK_SIZE_TINY};
    /// use nockvm::noun::{Cell, D, NounSpace};
    ///
    /// let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
    /// let noun = Cell::new(&mut stack, D(1), D(2)).as_noun();
    /// let space = NounSpace::stack_only(&stack);
    ///
    /// space.with_brand(|space| {
    ///     let cell = space.handle(noun).as_cell().unwrap();
    ///     assert_eq!(cell.head().as_atom().unwrap().as_u64().unwrap(), 1);
    ///     assert_eq!(cell.tail().as_atom().unwrap().as_u64().unwrap(), 2);
    /// });
    /// ```
    ///
    /// Handles from different branded spaces do not type-check together:
    ///
    /// ```compile_fail
    /// use nockvm::mem::{NockStack, NOCK_STACK_SIZE_TINY};
    /// use nockvm::noun::{BrandedNounHandle, BrandedNounSpace, Cell, D, NounSpace};
    ///
    /// fn same_space<'space, 'id>(
    ///     _space: BrandedNounSpace<'space, 'id>,
    ///     _noun: BrandedNounHandle<'space, 'id>,
    /// ) {
    /// }
    ///
    /// let mut stack_a = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
    /// let mut stack_b = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
    /// let noun_a = Cell::new(&mut stack_a, D(1), D(2)).as_noun();
    /// let space_a = NounSpace::stack_only(&stack_a);
    /// let space_b = NounSpace::stack_only(&stack_b);
    ///
    /// space_a.with_brand(|space_a| {
    ///     space_b.with_brand(|space_b| {
    ///         same_space(space_b, space_a.handle(noun_a));
    ///     });
    /// });
    /// ```
    ///
    /// Branded handles also cannot escape the generative scope:
    ///
    /// ```compile_fail
    /// use nockvm::mem::{NockStack, NOCK_STACK_SIZE_TINY};
    /// use nockvm::noun::{Cell, D, NounSpace};
    ///
    /// let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
    /// let noun = Cell::new(&mut stack, D(1), D(2)).as_noun();
    /// let space = NounSpace::stack_only(&stack);
    ///
    /// let _escaped = space.with_brand(|space| space.handle(noun));
    /// ```
    pub fn with_brand<R>(&self, f: impl for<'id> FnOnce(BrandedNounSpace<'_, 'id>) -> R) -> R {
        f(BrandedNounSpace::new(self))
    }

    fn assert_stack_epoch(&self) {
        let Some(epoch_ptr) = self.stack_epoch_ptr else {
            return;
        };
        let snapshot = self
            .stack_epoch_snapshot
            .expect("stack epoch snapshot missing");
        let current = unsafe { (*epoch_ptr).load(Ordering::Relaxed) };
        assert!(
            current == snapshot,
            "NounSpace used after NockStack reset/flip (current epoch {current}, snapshot {snapshot})"
        );
    }

    fn resolve_stack_ptr(&self, payload: u64) -> *const u8 {
        let ptr = usize::try_from(
            payload
                .checked_shl(3)
                .expect("stack pointer payload exceeds addressable range"),
        )
        .expect("stack pointer payload exceeds usize addressable range")
            as *const u8;
        let addr = ptr as usize;
        if let (Some(base), Some(end)) = (self.stack_base, self.stack_end) {
            if addr >= base && addr < end {
                self.assert_stack_epoch();
                return ptr;
            }
        }
        if let (Some(base), Some(end)) = (self.pma_base, self.pma_end) {
            if addr >= base && addr < end {
                return ptr;
            }
        }
        for (base, end) in &self.extra_ptr_ranges {
            if addr >= *base && addr < *end {
                return ptr;
            }
        }
        panic!(
            "pointer-form noun {:p} is not within stack or PMA arenas",
            ptr
        );
    }

    fn classify_ptr(&self, ptr: *const u8) -> AllocLocation {
        let addr = ptr as usize;
        if let (Some(base), Some(end)) = (self.stack_base, self.stack_end) {
            if addr >= base && addr < end {
                self.assert_stack_epoch();
                return AllocLocation::Stack;
            }
        }
        if let (Some(base), Some(end)) = (self.pma_base, self.pma_end) {
            if addr >= base && addr < end {
                return AllocLocation::PmaPtr;
            }
        }
        for (base, end) in &self.extra_ptr_ranges {
            if addr >= *base && addr < *end {
                return AllocLocation::Stack;
            }
        }
        panic!(
            "pointer-form noun {:p} is not within stack or PMA arenas",
            ptr
        );
    }

    fn resolve_pma_ptr(&self, offset: PmaOffsetWords) -> *const u8 {
        let offset = offset
            .try_into_usize()
            .expect("PMA offset exceeds addressable range");
        let arena_words = self
            .pma
            .as_ref()
            .map(|arena| arena.words())
            .or(self.pma_words)
            .expect("PMA arena is required to resolve offset nouns");
        assert!(
            offset < arena_words,
            "PMA offset {} out of bounds (size words {})",
            offset,
            arena_words
        );
        let base = self
            .pma_base
            .expect("PMA arena is required to resolve offset nouns");
        let offset_bytes = offset
            .checked_mul(8)
            .expect("PMA offset exceeds addressable range");
        let ptr = base
            .checked_add(offset_bytes)
            .expect("PMA offset exceeds addressable range") as *const u8;
        assert!(
            {
                let end = self
                    .pma_end
                    .expect("PMA arena is required to resolve offset nouns");
                let addr = ptr as usize;
                addr >= base && addr < end
            },
            "PMA offset {} resolves outside the PMA arena",
            offset
        );
        ptr
    }
}

mod generative_brand {
    pub enum Id {}
}

type Brand<'id> =
    std::marker::PhantomData<fn(&'id generative_brand::Id) -> &'id generative_brand::Id>;

#[doc(hidden)]
#[derive(Copy, Clone)]
pub struct BrandedNounSpace<'space, 'id> {
    space: &'space NounSpace,
    _brand: Brand<'id>,
}

impl<'space, 'id> BrandedNounSpace<'space, 'id> {
    fn new(space: &'space NounSpace) -> Self {
        Self {
            space,
            _brand: std::marker::PhantomData,
        }
    }

    pub fn handle(self, noun: Noun) -> BrandedNounHandle<'space, 'id> {
        BrandedNounHandle::from_unbranded(NounHandle::new(noun, self.space))
    }
}

#[derive(Copy, Clone)]
pub struct NounHandle<'a> {
    noun: Noun,
    space: &'a NounSpace,
}

impl<'a> NounHandle<'a> {
    pub fn new(noun: Noun, space: &'a NounSpace) -> Self {
        Self { noun, space }
    }

    pub fn noun(self) -> Noun {
        self.noun
    }

    pub fn space(self) -> &'a NounSpace {
        self.space
    }

    pub fn repr(self) -> NounRepr {
        self.noun.repr(self.space)
    }

    pub fn allocated_location(self) -> Option<AllocLocation> {
        self.noun.allocated_location(self.space)
    }

    pub fn is_direct(self) -> bool {
        self.noun.is_direct()
    }

    pub fn is_atom(self) -> bool {
        self.noun.is_atom()
    }

    pub fn is_cell(self) -> bool {
        self.noun.is_cell()
    }

    pub fn is_allocated(self) -> bool {
        self.noun.is_allocated()
    }

    pub fn as_atom(self) -> Result<AtomHandle<'a>> {
        self.noun
            .as_atom()
            .map(|atom| AtomHandle::new(atom, self.space))
    }

    pub fn as_cell(self) -> Result<CellHandle<'a>> {
        self.noun
            .as_cell()
            .map(|cell| CellHandle::new(cell, self.space))
    }

    pub fn atom(self) -> Option<AtomHandle<'a>> {
        self.noun
            .atom()
            .map(|atom| AtomHandle::new(atom, self.space))
    }

    pub fn cell(self) -> Option<CellHandle<'a>> {
        self.noun
            .cell()
            .map(|cell| CellHandle::new(cell, self.space))
    }

    pub fn as_either_atom_cell(self) -> Either<AtomHandle<'a>, CellHandle<'a>> {
        match self.noun.as_either_atom_cell() {
            Left(atom) => Left(AtomHandle::new(atom, self.space)),
            Right(cell) => Right(CellHandle::new(cell, self.space)),
        }
    }

    pub fn slot(self, axis: u64) -> Result<NounHandle<'a>> {
        self.noun
            .slot(axis, self.space)
            .map(|noun| NounHandle::new(noun, self.space))
    }

    pub fn slot_atom(self, atom: Atom) -> Result<NounHandle<'a>> {
        self.noun
            .slot_atom(atom, self.space)
            .map(|noun| NounHandle::new(noun, self.space))
    }

    pub fn list_iter(self) -> NounHandleListIterator<'a> {
        NounHandleListIterator { noun: self }
    }

    pub fn eq_bytes(self, bytes: impl AsRef<[u8]>) -> bool {
        if let Ok(atom) = self.noun.as_atom() {
            AtomHandle::new(atom, self.space).eq_bytes(bytes)
        } else {
            false
        }
    }

    pub fn mass(self) -> usize {
        self.noun.mass(self.space)
    }

    pub fn mass_frame(self, stack: &NockStack) -> usize {
        self.noun.mass_frame(stack, self.space)
    }

    pub unsafe fn forwarding_pointer(self) -> Option<NounHandle<'a>> {
        let allocated = self.noun.as_allocated().ok()?;
        allocated
            .forwarding_pointer(self.space)
            .map(|forwarded| NounHandle::new(forwarded.as_noun(), self.space))
    }
}

#[doc(hidden)]
#[derive(Copy, Clone)]
pub struct BrandedNounHandle<'space, 'id> {
    handle: NounHandle<'space>,
    _brand: Brand<'id>,
}

impl<'space, 'id> BrandedNounHandle<'space, 'id> {
    fn from_unbranded(handle: NounHandle<'space>) -> Self {
        Self {
            handle,
            _brand: std::marker::PhantomData,
        }
    }

    pub fn repr(self) -> NounRepr {
        self.handle.repr()
    }

    pub fn as_atom(self) -> Result<BrandedAtomHandle<'space, 'id>> {
        self.handle.as_atom().map(BrandedAtomHandle::from_unbranded)
    }

    pub fn as_cell(self) -> Result<BrandedCellHandle<'space, 'id>> {
        self.handle.as_cell().map(BrandedCellHandle::from_unbranded)
    }

    pub fn slot(self, axis: u64) -> Result<Self> {
        self.handle.slot(axis).map(Self::from_unbranded)
    }
}

#[derive(Copy, Clone)]
pub struct AtomHandle<'a> {
    atom: Atom,
    space: &'a NounSpace,
}

impl<'a> AtomHandle<'a> {
    pub fn new(atom: Atom, space: &'a NounSpace) -> Self {
        Self { atom, space }
    }

    pub fn atom(self) -> Atom {
        self.atom
    }

    pub fn space(self) -> &'a NounSpace {
        self.space
    }

    pub fn as_noun(self) -> NounHandle<'a> {
        NounHandle::new(self.atom.as_noun(), self.space)
    }

    pub fn is_direct(self) -> bool {
        self.atom.is_direct()
    }

    pub fn is_indirect(self) -> bool {
        self.atom.is_indirect()
    }

    pub fn is_normalized(&self) -> bool {
        self.atom.is_normalized(self.space)
    }

    pub fn as_ne_bytes(&self) -> &[u8] {
        self.atom.as_ne_bytes(self.space)
    }

    pub fn eq_bytes<B: AsRef<[u8]>>(&self, bytes: B) -> bool {
        let bytes_ref = bytes.as_ref();
        let atom_bytes = self.as_ne_bytes();
        if bytes_ref.len() > atom_bytes.len() {
            return false;
        }
        if bytes_ref.len() == atom_bytes.len() {
            return atom_bytes == bytes_ref;
        }
        if atom_bytes[bytes_ref.len()..].iter().any(|b| *b != 0) {
            return false;
        }
        &atom_bytes[0..bytes_ref.len()] == bytes_ref
    }

    pub fn to_bytes_until_nul(&self) -> std::result::Result<Vec<u8>, str::Utf8Error> {
        str::from_utf8(self.as_ne_bytes())
            .map(|bytes| bytes.trim_end_matches('\0').as_bytes().to_vec())
    }

    pub fn into_string(self) -> std::result::Result<String, str::Utf8Error> {
        str::from_utf8(self.as_ne_bytes()).map(|string| string.trim_end_matches('\0').to_string())
    }

    pub fn to_ne_bytes(self) -> Vec<u8> {
        self.atom.to_ne_bytes(self.space)
    }

    pub fn to_be_bytes(self) -> Vec<u8> {
        self.atom.to_be_bytes(self.space)
    }

    pub fn to_le_bytes(self) -> Vec<u8> {
        self.atom.to_le_bytes(self.space)
    }

    pub fn as_u64(self) -> Result<u64> {
        self.atom.as_u64(self.space)
    }

    pub fn as_u64_pair(self) -> Result<[u64; 2]> {
        unsafe { self.atom.as_u64_pair(self.space) }
    }

    pub fn as_bitslice(&self) -> &BitSlice<u64, Lsb0> {
        self.atom.as_bitslice(self.space)
    }

    pub fn as_bitslice_mut(&mut self) -> &mut BitSlice<u64, Lsb0> {
        self.atom.as_bitslice_mut(self.space)
    }

    pub fn as_ubig<S: Stack>(self, stack: &mut S) -> UBig {
        self.atom.as_ubig(stack, self.space)
    }

    pub fn size(self) -> usize {
        self.atom.size(self.space)
    }

    pub fn bit_size(self) -> usize {
        self.atom.bit_size(self.space)
    }

    pub fn data_pointer(&self) -> *const u64 {
        self.atom.data_pointer(self.space)
    }

    pub fn raw_size(self) -> usize {
        match self.atom.as_either() {
            Left(_direct) => 1,
            Right(indirect) => indirect.raw_size(self.space),
        }
    }

    pub unsafe fn raw_pointer(self) -> *const u64 {
        let indirect = self
            .atom
            .as_indirect()
            .expect("expected indirect atom for raw_pointer");
        indirect.to_raw_pointer(self.space)
    }

    pub unsafe fn raw_pointer_mut(self) -> *mut u64 {
        let mut indirect = self
            .atom
            .as_indirect()
            .expect("expected indirect atom for raw_pointer_mut");
        indirect.to_raw_pointer_mut(self.space)
    }

    pub unsafe fn set_forwarding_pointer(self, new_me: *const u64) {
        let mut indirect = self
            .atom
            .as_indirect()
            .expect("expected indirect atom for set_forwarding_pointer");
        indirect.set_forwarding_pointer(new_me, self.space);
    }

    pub unsafe fn normalize(self) -> AtomHandle<'a> {
        let mut atom = self.atom;
        let normalized = atom.normalize(self.space);
        AtomHandle::new(normalized, self.space)
    }
    pub fn as_direct(&self) -> Result<DirectAtom> {
        if self.is_direct() {
            unsafe { Ok(self.atom.direct) }
        } else {
            Err(Error::NotDirectAtom)
        }
    }

    pub fn as_indirect(&self) -> Result<IndirectAtom> {
        if self.is_indirect() {
            unsafe { Ok(self.atom.indirect) }
        } else {
            Err(Error::NotIndirectAtom)
        }
    }

    pub fn as_either(&self) -> Either<DirectAtom, IndirectAtom> {
        if self.is_indirect() {
            unsafe { Right(self.atom.indirect) }
        } else {
            unsafe { Left(self.atom.direct) }
        }
    }
}

#[doc(hidden)]
#[derive(Copy, Clone)]
pub struct BrandedAtomHandle<'space, 'id> {
    handle: AtomHandle<'space>,
    _brand: Brand<'id>,
}

impl<'space, 'id> BrandedAtomHandle<'space, 'id> {
    fn from_unbranded(handle: AtomHandle<'space>) -> Self {
        Self {
            handle,
            _brand: std::marker::PhantomData,
        }
    }

    pub fn as_noun(self) -> BrandedNounHandle<'space, 'id> {
        BrandedNounHandle::from_unbranded(self.handle.as_noun())
    }

    pub fn as_u64(self) -> Result<u64> {
        self.handle.as_u64()
    }
}

#[derive(Copy, Clone)]
pub struct CellHandle<'a> {
    cell: Cell,
    space: &'a NounSpace,
}

impl<'a> CellHandle<'a> {
    pub fn new(cell: Cell, space: &'a NounSpace) -> Self {
        Self { cell, space }
    }

    pub fn cell(self) -> Cell {
        self.cell
    }

    pub fn space(self) -> &'a NounSpace {
        self.space
    }

    pub fn as_noun(self) -> NounHandle<'a> {
        NounHandle::new(self.cell.as_noun(), self.space)
    }

    pub fn head(self) -> NounHandle<'a> {
        NounHandle::new(self.cell.head(self.space), self.space)
    }

    pub fn tail(self) -> NounHandle<'a> {
        NounHandle::new(self.cell.tail(self.space), self.space)
    }

    pub unsafe fn raw_pointer(self) -> *const CellMemory {
        self.cell.to_raw_pointer(self.space)
    }

    pub unsafe fn raw_pointer_mut(self) -> *mut CellMemory {
        let mut cell = self.cell;
        cell.to_raw_pointer_mut(self.space)
    }

    pub unsafe fn set_forwarding_pointer(self, new_me: *const CellMemory) {
        let mut cell = self.cell;
        cell.set_forwarding_pointer(new_me, self.space);
    }
}

#[doc(hidden)]
#[derive(Copy, Clone)]
pub struct BrandedCellHandle<'space, 'id> {
    handle: CellHandle<'space>,
    _brand: Brand<'id>,
}

impl<'space, 'id> BrandedCellHandle<'space, 'id> {
    fn from_unbranded(handle: CellHandle<'space>) -> Self {
        Self {
            handle,
            _brand: std::marker::PhantomData,
        }
    }

    pub fn as_noun(self) -> BrandedNounHandle<'space, 'id> {
        BrandedNounHandle::from_unbranded(self.handle.as_noun())
    }

    pub fn head(self) -> BrandedNounHandle<'space, 'id> {
        BrandedNounHandle::from_unbranded(self.handle.head())
    }

    pub fn tail(self) -> BrandedNounHandle<'space, 'id> {
        BrandedNounHandle::from_unbranded(self.handle.tail())
    }
}

pub struct NounHandleListIterator<'a> {
    noun: NounHandle<'a>,
}

impl<'a> Iterator for NounHandleListIterator<'a> {
    type Item = NounHandle<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Ok(cell) = self.noun.as_cell() {
            let head = cell.head();
            self.noun = cell.tail();
            Some(head)
        } else if unsafe { self.noun.noun().raw_equals(&D(0)) } {
            None
        } else {
            panic!("Improper list terminator: {:?}", self.noun.noun());
        }
    }
}

/*  A note on forwarding pointers:
 *
 *  Forwarding pointers are only used temporarily during copies between NockStack frames and between
 *  the NockStack and the PMA. Since unifying equality checks can create structural sharing between
 *  Noun objects, forwarding pointers act as a signal that a Noun has already been copied to the
 *  "to" space. The old Noun object in the "from" space is given a forwarding pointer so that any
 *  future refernces to the same structure know that it has already been copied and that they should
 *  retain the structural sharing relationship by referencing the new copy in the "to" copy space.
 *
 *  The Nouns in the "from" space marked with forwarding pointers are dangling pointers after a copy
 *  operation. No code outside of the copying code checks for forwarding pointers. This invariant
 *  must be enforced in two ways:
 *      1. The current frame must be immediately popped after preserving data, when
 *          copying from a junior NockStack frame to a senior NockStack frame.
 *      2. All persistent derived state (e.g. Hot state, Warm state) must be preserved
 *          and the NockStack reset after saving data to the PMA.
 */

/** Tag for a forwarding pointer */
const FORWARDING_TAG: u64 = CELL_MASK;

/** Tag mask for a forwarding pointer */
const FORWARDING_MASK: u64 = CELL_MASK;

/** Shorthand for 0's that actually are ~ **/
pub const SIG: Noun = D(0);

/** Loobeans */
pub const YES: Noun = D(0);
pub const NO: Noun = D(1);
pub const NONE: Noun = unsafe { DirectAtom::new_unchecked(tas!(b"MORMAGIC")).as_noun() };

#[cfg(feature = "check_acyclic")]
#[macro_export]
macro_rules! assert_acyclic {
    ( $space:expr, $x:expr ) => {
        assert_no_alloc::permit_alloc(|| {
            assert!(crate::noun::acyclic_noun(($x).in_space($space)));
        })
    };
}

#[cfg(not(feature = "check_acyclic"))]
#[macro_export]
macro_rules! assert_acyclic {
    ( $space:expr, $x:expr ) => {};
}

pub(crate) fn acyclic_noun(noun: NounHandle) -> bool {
    let mut seen = IntMap::new();
    acyclic_noun_go(noun.noun(), &mut seen, noun.space())
}

fn acyclic_noun_go(noun: Noun, seen: &mut IntMap<u64, ()>, space: &NounSpace) -> bool {
    match noun.as_either_atom_cell() {
        Left(_atom) => true,
        Right(cell) => {
            if seen.get(cell.0).is_some() {
                false
            } else {
                seen.insert(cell.0, ());
                let cell_handle = cell.in_space(space);
                if acyclic_noun_go(cell_handle.head().noun(), seen, space) {
                    if acyclic_noun_go(cell_handle.tail().noun(), seen, space) {
                        seen.remove(cell.0);
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
        }
    }
}

#[cfg(feature = "check_forwarding")]
#[macro_export]
macro_rules! assert_no_forwarding_pointers {
    ( $space:expr, $x:expr ) => {
        assert_no_alloc::permit_alloc(|| {
            assert!(crate::noun::no_forwarding_pointers(($x).in_space($space)));
        })
    };
}

#[cfg(not(feature = "check_forwarding"))]
#[macro_export]
macro_rules! assert_no_forwarding_pointers {
    ( $space:expr, $x:expr ) => {};
}

pub(crate) fn no_forwarding_pointers(noun: NounHandle) -> bool {
    let mut dbg_stack = Vec::new();
    let space = noun.space();
    dbg_stack.push(noun.noun());

    while !dbg_stack.is_empty() {
        if let Some(noun) = dbg_stack.pop() {
            if unsafe { noun.raw & FORWARDING_MASK == FORWARDING_TAG } {
                return false;
            } else if let Ok(cell) = noun.in_space(space).as_cell() {
                dbg_stack.push(cell.tail().noun());
                dbg_stack.push(cell.head().noun());
            }
        } else {
            break;
        }
    }

    true
}

/** Test if a noun is a direct atom. */
fn is_direct_atom(noun: u64) -> bool {
    noun & DIRECT_MASK == DIRECT_TAG
}

/** Test if a noun is an indirect atom. */
fn is_indirect_atom(noun: u64) -> bool {
    noun & INDIRECT_MASK == INDIRECT_TAG
}

/** Test if a noun is a cell. */
fn is_cell(noun: u64) -> bool {
    noun & CELL_MASK == CELL_TAG
}

#[inline(always)]
unsafe fn noun_from_raw(raw: u64) -> Noun {
    Noun { raw }
}

#[inline(always)]
unsafe fn cell_ptr_from_raw_trusted(raw: u64, space: &NounSpace) -> *const CellMemory {
    TaggedPtr::from_raw(raw).resolve_const_trusted(CELL_MASK, space) as *const CellMemory
}

/** A noun-related error. */
#[derive(Debug, PartialEq)]
pub enum Error {
    /** Expected type [`Allocated`]. */
    NotAllocated,
    /** Expected type [`Atom`]. */
    NotAtom,
    /** Expected type [`Cell`]. */
    NotCell,
    /** Expected type [`DirectAtom`]. */
    NotDirectAtom,
    /** Expected type [`IndirectAtom`]. */
    NotIndirectAtom,
    /** The value can't be represented by the given type. */
    NotRepresentable,
}

impl error::Error for Error {}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotAllocated => f.write_str("not an allocated noun"),
            Error::NotAtom => f.write_str("not an atom"),
            Error::NotCell => f.write_str("not a cell"),
            Error::NotDirectAtom => f.write_str("not a direct atom"),
            Error::NotIndirectAtom => f.write_str("not an indirect atom"),
            Error::NotRepresentable => f.write_str("unrepresentable value"),
        }
    }
}

impl From<Error> for () {
    fn from(_: Error) -> Self {}
}

/** A [`Result`] that returns an [`Error`] on error. */
pub type Result<T> = std::result::Result<T, Error>;

/** A direct atom.
 *
 * Direct atoms represent an atom up to and including DIRECT_MAX as a machine word.
 */
#[derive(Copy, Clone)]
#[repr(C)]
#[repr(packed(8))]
pub struct DirectAtom(u64);

impl DirectAtom {
    /** Create a new direct atom, or panic if the value is greater than DIRECT_MAX */
    pub const fn new_panic(value: u64) -> Self {
        if value > DIRECT_MAX {
            panic!("Number is greater than DIRECT_MAX")
        } else {
            DirectAtom(value)
        }
    }

    /** Create a new direct atom, or return Err if the value is greater than DIRECT_MAX */
    pub const fn new(value: u64) -> Result<Self> {
        if value > DIRECT_MAX {
            Err(Error::NotRepresentable)
        } else {
            Ok(DirectAtom(value))
        }
    }

    /** Create a new direct atom. This is unsafe because the value is not checked.
     *
     * Attempting to create a direct atom with a value greater than DIRECT_MAX will
     * result in this value being interpreted by the runtime as a cell or indirect atom,
     * with corresponding memory accesses. Thus, this function is marked as unsafe.
     */
    pub const unsafe fn new_unchecked(value: u64) -> Self {
        DirectAtom(value)
    }

    pub fn bit_size(self) -> usize {
        (64 - self.0.leading_zeros()) as usize
    }

    pub fn as_atom(self) -> Atom {
        Atom { direct: self }
    }

    pub fn as_ubig<S: Stack>(self, _stack: &mut S) -> UBig {
        UBig::from(self.0)
    }

    pub const fn as_noun(self) -> Noun {
        Noun { direct: self }
    }

    pub fn data(self) -> u64 {
        self.0
    }

    pub fn as_bitslice(&self) -> &BitSlice<u64, Lsb0> {
        BitSlice::from_element(&self.0)
    }

    pub fn as_bitslice_mut(&mut self) -> &mut BitSlice<u64, Lsb0> {
        BitSlice::from_element_mut(&mut self.0)
    }

    pub fn as_ne_bytes(&self) -> &[u8] {
        let bytes: &[u8; 8] = unsafe { std::mem::transmute(&self.0) };
        &bytes[..]
    }

    /// Returns Vec<u8> under native-endian of the machine
    pub fn to_ne_bytes(&self) -> Vec<u8> {
        self.as_ne_bytes().to_vec()
    }

    pub fn to_be_bytes(&self) -> Vec<u8> {
        self.0.to_be_bytes().to_vec()
    }

    pub fn to_le_bytes(&self) -> Vec<u8> {
        self.0.to_le_bytes().to_vec()
    }
}

impl fmt::Debug for DirectAtom {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.0 == 0 {
            return write!(f, "0");
        }

        let mut null = false;
        let mut n = 0;
        let bytes = self.0.to_le_bytes();
        for byte in bytes.iter() {
            if *byte == 0 {
                null = true;
                continue;
            }
            if (null && *byte != 0) || *byte < 33 || *byte > 126 {
                return write!(f, "{}", self.0);
            }
            n += 1;
        }
        if n > 1 {
            write!(f, "%{}", unsafe {
                std::str::from_utf8_unchecked(&bytes[..n])
            })
        } else {
            write!(f, "{}", self.0)
        }
    }
}

#[allow(non_snake_case)]
pub const fn D(n: u64) -> Noun {
    DirectAtom::new_panic(n).as_noun()
}

#[allow(non_snake_case)]
pub fn T<A: NounAllocator>(allocator: &mut A, tup: &[Noun]) -> Noun {
    Cell::new_tuple(allocator, tup).as_noun()
}

/// Create $tape Noun from ASCII string
pub fn tape<A: NounAllocator>(allocator: &mut A, text: &str) -> Noun {
    //  XX: Needs unit tests
    let mut res = D(0);
    for c in text.bytes().rev() {
        res = T(allocator, &[D(c as u64), res])
    }
    res
}

/** An indirect atom.
 *
 *  Indirect atoms represent atoms above DIRECT_MAX as a tagged pointer to a memory buffer
 *  structured as:
 *
 *  - first word: metadata
 *  - second word: size in 64-bit words
 *  - remaining words: data
 *
 *  Indirect atoms are always stored in little-endian byte order
 */
#[derive(Copy, Clone)]
#[repr(C)]
#[repr(packed(8))]
pub struct IndirectAtom(u64);

impl IndirectAtom {
    /** Tag the pointer and type it as an indirect atom. */
    pub unsafe fn from_raw_pointer(ptr: *const u64) -> Self {
        IndirectAtom(TaggedPtr::from_stack_ptr(ptr as *const u8, INDIRECT_TAG).raw())
    }

    pub fn from_offset_words<O: WordOffset>(words: O) -> Self {
        IndirectAtom(TaggedPtr::from_offset(words, INDIRECT_TAG).raw())
    }

    /** Strip the tag from an indirect atom and return it as a mutable pointer to its memory buffer. */
    unsafe fn to_raw_pointer_mut(&mut self, space: &NounSpace) -> *mut u64 {
        TaggedPtr::from_raw(self.0).resolve_mut(INDIRECT_MASK, space) as *mut u64
    }

    /** Strip the tag from an indirect atom and return it as a pointer to its memory buffer. */
    #[allow(clippy::wrong_self_convention)]
    pub(crate) unsafe fn to_raw_pointer(&self, space: &NounSpace) -> *const u64 {
        TaggedPtr::from_raw(self.0).resolve_const(INDIRECT_MASK, space) as *const u64
    }

    /** Strip the tag with no location/epoch checks. Trusted local fast path only. */
    #[inline(always)]
    #[allow(clippy::wrong_self_convention)]
    pub(crate) unsafe fn to_raw_pointer_trusted(&self, space: &NounSpace) -> *const u64 {
        TaggedPtr::from_raw(self.0).resolve_const_trusted(INDIRECT_MASK, space) as *const u64
    }

    /// Get raw pointer for stack-pointer form atoms only
    pub unsafe fn to_raw_pointer_stack(&self) -> *const u64 {
        let tagged = TaggedPtr::from_raw(self.0);
        if tagged.location() == PtrLocation::Stack {
            ((tagged.payload(INDIRECT_MASK)) << 3) as *const u64
        } else {
            panic!("expected stack-pointer Noun, got offset instead");
        }
    }

    /// Get mutable raw pointer for stack-pointer form atoms only
    pub fn to_raw_pointer_mut_stack(&mut self) -> *mut u64 {
        let tagged = TaggedPtr::from_raw(self.0);
        if tagged.location() == PtrLocation::Stack {
            ((tagged.payload(INDIRECT_MASK)) << 3) as *mut u64
        } else {
            panic!("expected stack-pointer Noun, got offset instead");
        }
    }

    pub unsafe fn set_forwarding_pointer(&mut self, new_me: *const u64, space: &NounSpace) {
        // This is OK because the size is stored as 64 bit words, not bytes.
        // Thus, a true size value will never be larger than U64::MAX >> 3, and so
        // any of the high bits set as an MSB
        *self.to_raw_pointer_mut(space).add(1) =
            TaggedPtr::from_stack_ptr(new_me as *const u8, FORWARDING_TAG).raw();
    }

    pub(crate) unsafe fn forwarding_pointer(&self, space: &NounSpace) -> Option<IndirectAtom> {
        let size_raw = *self.to_raw_pointer(space).add(1);
        if size_raw & FORWARDING_MASK == FORWARDING_TAG {
            let ptr =
                TaggedPtr::from_raw(size_raw).resolve_const(FORWARDING_MASK, space) as *const u64;
            Some(Self::from_raw_pointer(ptr))
        } else {
            None
        }
    }

    /** Make an indirect atom by copying from other memory.
     *
     *  Note: size is in 64-bit words, not bytes.
     */
    pub unsafe fn new_raw<A: NounAllocator>(
        allocator: &mut A,
        size: usize,
        data: *const u64,
    ) -> Self {
        let (mut indirect, buffer) = Self::new_raw_mut(allocator, size);
        ptr::copy_nonoverlapping(data, buffer, size);
        // Use normalize_stack since new_raw_mut creates stack-pointer form atoms
        *(indirect.normalize_stack())
    }

    /** Make an indirect atom by copying from other memory.
     *
     *  Note: size is bytes, not words
     */
    pub unsafe fn new_raw_bytes<A: NounAllocator>(
        allocator: &mut A,
        size: usize,
        data: *const u8,
    ) -> Self {
        let (mut indirect, buffer) = Self::new_raw_mut_bytes(allocator, size);
        ptr::copy_nonoverlapping(data, buffer.as_mut_ptr(), size);
        // Use normalize_stack since new_raw_mut_bytes creates stack-pointer form atoms
        *(indirect.normalize_stack())
    }

    pub unsafe fn new_raw_bytes_ref<A: NounAllocator>(allocator: &mut A, data: &[u8]) -> Self {
        IndirectAtom::new_raw_bytes(allocator, data.len(), data.as_ptr())
    }

    /** Make an indirect atom that can be written into. Return the atom (which should not be used
     * until it is written and normalized) and a mutable pointer which is the data buffer for the
     * indirect atom, to be written into.
     */
    pub unsafe fn new_raw_mut<A: NounAllocator>(
        allocator: &mut A,
        size: usize,
    ) -> (Self, *mut u64) {
        debug_assert!(size > 0);
        let buffer = allocator.alloc_indirect(size);
        *buffer = 0;
        *buffer.add(1) = size as u64;
        (Self::from_raw_pointer(buffer), buffer.add(2))
    }

    /** Make an indirect atom that can be written into, and zero the whole data buffer.
     * Return the atom (which should not be used until it is written and normalized) and a mutable
     * pointer which is the data buffer for the indirect atom, to be written into.
     */
    pub unsafe fn new_raw_mut_zeroed<A: NounAllocator>(
        allocator: &mut A,
        size: usize,
    ) -> (Self, *mut u64) {
        let allocation = Self::new_raw_mut(allocator, size);
        ptr::write_bytes(allocation.1, 0, size);
        allocation
    }

    /** Make an indirect atom that can be written into as a bitslice. The constraints of
     * [new_raw_mut_zeroed] also apply here
     */
    pub unsafe fn new_raw_mut_bitslice<'a, A: NounAllocator>(
        allocator: &mut A,
        size: usize,
    ) -> (Self, &'a mut BitSlice<u64, Lsb0>) {
        let (noun, ptr) = Self::new_raw_mut_zeroed(allocator, size);
        (
            noun,
            BitSlice::from_slice_mut(from_raw_parts_mut(ptr, size)),
        )
    }

    /** Make an indirect atom that can be written into as a slice of bytes. The constraints of
     * [new_raw_mut_zeroed] also apply here
     *
     * Note: size is bytes, not words
     */
    pub unsafe fn new_raw_mut_bytes<'a, A: NounAllocator>(
        allocator: &mut A,
        size: usize,
    ) -> (Self, &'a mut [u8]) {
        let word_size = (size + 7) >> 3;
        let (noun, ptr) = Self::new_raw_mut_zeroed(allocator, word_size);
        (noun, from_raw_parts_mut(ptr as *mut u8, size))
    }

    /// Create an indirect atom backed by a fixed-size array
    pub unsafe fn new_raw_mut_bytearray<'a, const N: usize, A: NounAllocator>(
        allocator: &mut A,
    ) -> (Self, &'a mut [u8; N]) {
        let word_size = (std::mem::size_of::<[u8; N]>() + 7) >> 3;
        let (noun, ptr) = Self::new_raw_mut_zeroed(allocator, word_size);
        (noun, &mut *(ptr as *mut [u8; N]))
    }

    /** Size of an indirect atom in 64-bit words */
    pub(crate) fn size(&self, space: &NounSpace) -> usize {
        unsafe { *(self.to_raw_pointer(space).add(1)) as usize }
    }

    /** Memory size of an indirect atom (including size + metadata fields) in 64-bit words */
    pub(crate) fn raw_size(&self, space: &NounSpace) -> usize {
        self.size(space) + 2
    }

    pub(crate) fn bit_size(&self, space: &NounSpace) -> usize {
        unsafe {
            ((self.size(space) - 1) << 6) + 64
                - (*(self.to_raw_pointer(space).add(2 + self.size(space) - 1))).leading_zeros()
                    as usize
        }
    }

    /** Pointer to data for indirect atom */
    pub(crate) fn data_pointer(&self, space: &NounSpace) -> *const u64 {
        unsafe { self.to_raw_pointer(space).add(2) }
    }

    pub(crate) fn data_pointer_mut(&mut self, space: &NounSpace) -> *mut u64 {
        unsafe { self.to_raw_pointer_mut(space).add(2) }
    }

    pub fn data_pointer_stack(&self) -> Option<*const u64> {
        let tagged = TaggedPtr::from_raw(self.0);
        if tagged.location() == PtrLocation::Stack {
            Some(((tagged.payload(INDIRECT_MASK)) << 3) as *const u64)
        } else {
            None
        }
    }

    pub(crate) fn as_slice(&self, space: &NounSpace) -> &[u64] {
        unsafe { from_raw_parts(self.data_pointer(space), self.size(space)) }
    }

    #[inline(always)]
    pub(crate) fn size_trusted(&self, space: &NounSpace) -> usize {
        unsafe { *(self.to_raw_pointer_trusted(space).add(1)) as usize }
    }

    #[inline(always)]
    pub(crate) fn data_pointer_trusted(&self, space: &NounSpace) -> *const u64 {
        unsafe { self.to_raw_pointer_trusted(space).add(2) }
    }

    #[inline(always)]
    pub(crate) fn as_slice_trusted(&self, space: &NounSpace) -> &[u64] {
        unsafe { from_raw_parts(self.data_pointer_trusted(space), self.size_trusted(space)) }
    }

    pub(crate) fn as_mut_slice(&mut self, space: &NounSpace) -> &mut [u64] {
        unsafe { from_raw_parts_mut(self.data_pointer_mut(space), self.size(space)) }
    }

    pub(crate) fn as_ne_bytes(&self, space: &NounSpace) -> &[u8] {
        unsafe { from_raw_parts(self.data_pointer(space) as *const u8, self.size(space) << 3) }
    }

    #[allow(clippy::wrong_self_convention)]
    pub(crate) fn to_ne_bytes(&self, space: &NounSpace) -> Vec<u8> {
        self.as_ne_bytes(space).to_vec()
    }

    #[allow(unused)]
    #[allow(clippy::wrong_self_convention)]
    pub(crate) fn to_be_bytes(&self, space: &NounSpace) -> Vec<u8> {
        if self.size(space) == 1 {
            let num = unsafe { *(self.data_pointer(space)) };
            num.to_be_bytes().to_vec()
        } else {
            let mut bytes_ne = self.to_ne_bytes(space);
            #[cfg(target_endian = "little")]
            {
                bytes_ne.reverse()
            }
            bytes_ne
        }
    }

    #[allow(unused)]
    #[allow(clippy::wrong_self_convention)]
    pub(crate) fn to_le_bytes(&self, space: &NounSpace) -> Vec<u8> {
        if self.size(space) == 1 {
            let num = unsafe { *(self.data_pointer(space)) };
            num.to_le_bytes().to_vec()
        } else {
            let mut bytes_ne = self.to_ne_bytes(space);
            #[cfg(target_endian = "big")]
            {
                bytes_ne.reverse()
            }

            bytes_ne
        }
    }

    #[allow(unused)]
    /** BitSlice view on an indirect atom, with lifetime tied to reference to indirect atom. */
    pub(crate) fn as_bitslice(&self, space: &NounSpace) -> &BitSlice<u64, Lsb0> {
        BitSlice::from_slice(self.as_slice(space))
    }

    pub(crate) fn as_bitslice_mut(&mut self, space: &NounSpace) -> &mut BitSlice<u64, Lsb0> {
        BitSlice::from_slice_mut(self.as_mut_slice(space))
    }

    pub(crate) fn as_ubig<S: Stack>(&self, stack: &mut S, space: &NounSpace) -> UBig {
        let bytes_mem_repr = self.as_ne_bytes(space);

        #[cfg(target_endian = "little")]
        {
            UBig::from_le_bytes_stack(stack, bytes_mem_repr)
        }
        #[cfg(not(target_endian = "little"))]
        {
            UBig::from_be_bytes_stack(stack, bytes_mem_repr)
        }
    }

    pub(crate) unsafe fn as_u64(self, space: &NounSpace) -> Result<u64> {
        if self.size(space) == 1 {
            Ok(*(self.data_pointer(space)))
        } else {
            Err(Error::NotRepresentable)
        }
    }

    /** Produce a SoftFloat-compatible ordered pair of 64-bit words */
    pub(crate) fn as_u64_pair(self, space: &NounSpace) -> Result<[u64; 2]> {
        if self.size(space) <= 2 {
            let u128_array = &mut [0u64; 2];
            u128_array.copy_from_slice(&(self.as_slice(space)[0..2]));
            Ok(*u128_array)
        } else {
            Err(Error::NotRepresentable)
        }
    }

    /** Ensure that the size does not contain any trailing 0 words */
    pub(crate) unsafe fn normalize(&mut self, space: &NounSpace) -> &Self {
        let mut index = self.size(space) - 1;
        let data = self.data_pointer(space);
        loop {
            if index == 0 || *(data.add(index)) != 0 {
                break;
            }
            index -= 1;
        }
        *(self.to_raw_pointer_mut(space).add(1)) = (index + 1) as u64;
        self
    }

    /// Normalize a stack-pointer form indirect atom (no arena needed).
    /// Panics if the atom is in offset form.
    pub unsafe fn normalize_stack(&mut self) -> &Self {
        let ptr = self.to_raw_pointer_mut_stack();
        let mut index = (*(ptr.add(1)) as usize) - 1; // size is at offset 1
        let data = ptr.add(2); // data starts at offset 2
        loop {
            if index == 0 || *(data.add(index)) != 0 {
                break;
            }
            index -= 1;
        }
        *(ptr.add(1)) = (index + 1) as u64;
        self
    }

    /** Normalize, but convert to direct atom if it will fit */
    pub unsafe fn normalize_as_atom(&mut self, space: &NounSpace) -> Atom {
        self.normalize(space);
        if self.size(space) == 1 && *(self.data_pointer(space)) <= DIRECT_MAX {
            Atom {
                direct: DirectAtom(*(self.data_pointer(space))),
            }
        } else {
            Atom { indirect: *self }
        }
    }

    /// Normalize a stack-pointer form atom, converting to direct if it fits.
    /// Panics if the atom is in offset form.
    pub unsafe fn normalize_as_atom_stack(&mut self) -> Atom {
        self.normalize_stack();
        let ptr = self.to_raw_pointer_stack();
        let size = *(ptr.add(1)) as usize;
        let data = ptr.add(2);
        if size == 1 && *data <= DIRECT_MAX {
            Atom {
                direct: DirectAtom(*data),
            }
        } else {
            Atom { indirect: *self }
        }
    }

    pub fn as_atom(self) -> Atom {
        Atom { indirect: self }
    }

    pub fn as_allocated(self) -> Allocated {
        Allocated { indirect: self }
    }

    pub fn as_noun(self) -> Noun {
        Noun { indirect: self }
    }
}

// XX: Need a version that either:
//      a) allocates on the NockStack directly for creating a tape (or even a string?)
//      b) disables no-allocation, creates a string, utilitzes it (eprintf or generate tape), and then deallocates
impl fmt::Debug for IndirectAtom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tagged = TaggedPtr::from_raw(self.0);
        match tagged.location() {
            PtrLocation::Stack => {
                let ptr = ((tagged.payload(INDIRECT_MASK)) << 3) as *const u8;
                write!(f, "IndirectAtom(StackPtr={ptr:p})")
            }
            PtrLocation::Offset => {
                let offset = tagged.payload(INDIRECT_MASK);
                write!(f, "IndirectAtom(PmaOffset={offset})")
            }
        }
    }
}

/**
 * A cell.
 *
 * A cell is represented by a tagged pointer to a memory buffer with metadata, a word describing
 * the noun which is the cell's head, and a word describing a noun which is the cell's tail, each
 * at a fixed offset.
 */
#[derive(Copy, Clone)]
#[repr(C)]
#[repr(packed(8))]
pub struct Cell(u64);

impl Cell {
    pub unsafe fn from_raw_pointer(ptr: *const CellMemory) -> Self {
        Cell(TaggedPtr::from_stack_ptr(ptr as *const u8, CELL_TAG).raw())
    }

    pub fn from_offset_words<O: WordOffset>(words: O) -> Self {
        Cell(TaggedPtr::from_offset(words, CELL_TAG).raw())
    }

    #[allow(clippy::wrong_self_convention)]
    pub(crate) unsafe fn to_raw_pointer(&self, space: &NounSpace) -> *const CellMemory {
        TaggedPtr::from_raw(self.0).resolve_const(CELL_MASK, space) as *const CellMemory
    }

    #[inline(always)]
    #[allow(clippy::wrong_self_convention)]
    pub(crate) unsafe fn to_raw_pointer_trusted(&self, space: &NounSpace) -> *const CellMemory {
        TaggedPtr::from_raw(self.0).resolve_const_trusted(CELL_MASK, space) as *const CellMemory
    }

    pub(crate) unsafe fn to_raw_pointer_mut(&mut self, space: &NounSpace) -> *mut CellMemory {
        TaggedPtr::from_raw(self.0).resolve_mut(CELL_MASK, space) as *mut CellMemory
    }

    #[inline(always)]
    pub fn stack_memory_pointer(&self) -> Option<*const CellMemory> {
        let tagged = TaggedPtr::from_raw(self.0);
        if tagged.location() == PtrLocation::Stack {
            Some(((tagged.payload(CELL_MASK)) << 3) as *const CellMemory)
        } else {
            None
        }
    }

    pub(crate) unsafe fn head_as_mut(mut self, space: &NounSpace) -> *mut Noun {
        &mut (*self.to_raw_pointer_mut(space)).head as *mut Noun
    }

    pub(crate) unsafe fn tail_as_mut(mut self, space: &NounSpace) -> *mut Noun {
        &mut (*self.to_raw_pointer_mut(space)).tail as *mut Noun
    }

    pub unsafe fn set_forwarding_pointer(&mut self, new_me: *const CellMemory, space: &NounSpace) {
        (*self.to_raw_pointer_mut(space)).head = Noun {
            raw: TaggedPtr::from_stack_ptr(new_me as *const u8, FORWARDING_TAG).raw(),
        }
    }

    pub(crate) unsafe fn forwarding_pointer(&self, space: &NounSpace) -> Option<Cell> {
        let head_raw = (*self.to_raw_pointer(space)).head.raw;
        if head_raw & FORWARDING_MASK == FORWARDING_TAG {
            let ptr = TaggedPtr::from_raw(head_raw).resolve_const(FORWARDING_MASK, space)
                as *const CellMemory;
            Some(Self::from_raw_pointer(ptr))
        } else {
            None
        }
    }

    pub fn new<T: NounAllocator>(allocator: &mut T, head: Noun, tail: Noun) -> Cell {
        unsafe {
            let (cell, memory) = Self::new_raw_mut(allocator);
            (*memory).head = head;
            (*memory).tail = tail;
            cell
        }
    }

    pub fn new_tuple<A: NounAllocator>(allocator: &mut A, tup: &[Noun]) -> Cell {
        if tup.len() < 2 {
            panic!("Cannot create tuple with fewer than 2 elements");
        }

        let len = tup.len();
        let mut cell = Cell::new(allocator, tup[len - 2], tup[len - 1]);
        for i in (0..len - 2).rev() {
            cell = Cell::new(allocator, tup[i], cell.as_noun());
        }
        cell
    }

    pub unsafe fn new_raw_mut<A: NounAllocator>(allocator: &mut A) -> (Cell, *mut CellMemory) {
        let memory = allocator.alloc_cell();
        assert!(
            (memory as usize).is_multiple_of(std::mem::align_of::<CellMemory>()),
            "Memory is not aligned, {} {}",
            memory as usize,
            std::mem::align_of::<CellMemory>()
        );
        (*memory).metadata = 0;
        (Self::from_raw_pointer(memory), memory)
    }

    // TODO: idk about making these owned independently of their parent
    pub(crate) fn head(&self, space: &NounSpace) -> Noun {
        unsafe { (*(self.to_raw_pointer(space))).head }
    }

    // TODO: Ditto, etc.
    pub(crate) fn tail(&self, space: &NounSpace) -> Noun {
        unsafe { (*(self.to_raw_pointer(space))).tail }
    }

    #[inline(always)]
    pub(crate) fn head_tail(&self, space: &NounSpace) -> (Noun, Noun) {
        unsafe {
            let cell = self.to_raw_pointer(space);
            ((*cell).head, (*cell).tail)
        }
    }

    #[inline(always)]
    pub(crate) fn head_tail_trusted(&self, space: &NounSpace) -> (Noun, Noun) {
        unsafe {
            let cell = self.to_raw_pointer_trusted(space);
            ((*cell).head, (*cell).tail)
        }
    }

    pub(crate) fn head_ref<'a>(&'a self, space: &'a NounSpace) -> &'a Noun {
        unsafe {
            self.to_raw_pointer(space)
                .as_ref()
                .map(|cell| &cell.head)
                .unwrap_or_else(|| panic!("head_ref: invalid pointer"))
        }
    }

    // TODO: Ditto, etc.
    pub(crate) fn tail_ref<'a>(&'a self, space: &'a NounSpace) -> &'a Noun {
        unsafe {
            self.to_raw_pointer(space)
                .as_ref()
                .map(|cell| &cell.tail)
                .unwrap_or_else(|| panic!("head_ref: invalid pointer"))
        }
    }

    pub fn as_allocated(&self) -> Allocated {
        Allocated { cell: *self }
    }

    pub fn as_noun(&self) -> Noun {
        Noun { cell: *self }
    }

    pub fn in_space<'a>(self, space: &'a NounSpace) -> CellHandle<'a> {
        CellHandle::new(self, space)
    }
}

impl fmt::Debug for Cell {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let tagged = TaggedPtr::from_raw(self.0);
        match tagged.location() {
            PtrLocation::Stack => {
                let ptr = ((tagged.payload(CELL_MASK)) << 3) as *const u8;
                write!(f, "Cell(StackPtr={ptr:p})")
            }
            PtrLocation::Offset => {
                let offset = tagged.payload(CELL_MASK);
                write!(f, "Cell(PmaOffset={offset})")
            }
        }
    }
}

pub struct FullDebugCell<'a, 'b> {
    pub cell: &'a Cell,
    pub space: &'b NounSpace,
}

impl fmt::Debug for FullDebugCell<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fn do_fmt(
            cell: &Cell,
            space: &NounSpace,
            brackets: bool,
            f: &mut fmt::Formatter,
        ) -> fmt::Result {
            if brackets {
                write!(f, "[")?;
            }
            let cell_handle = (*cell).in_space(space);
            match cell_handle.head().as_cell() {
                Ok(head_cell) => {
                    do_fmt(&head_cell.cell(), space, true, f)?;
                    write!(f, " ")?;
                }
                Err(_) => {
                    write!(f, "{:?} ", cell_handle.head().noun())?;
                }
            }
            match cell_handle.tail().as_cell() {
                Ok(next_cell) => {
                    do_fmt(&next_cell.cell(), space, false, f)?;
                }
                Err(_) => {
                    write!(f, "{:?}", cell_handle.tail().noun())?;
                }
            }
            if brackets {
                write!(f, "]")?;
            }
            Ok(())
        }

        do_fmt(self.cell, self.space, true, f)?;
        Ok(())
    }
}

// Render a path which is a linked-list of cells of of atoms (direct and indirect strings)
pub struct DebugPath<'a, 'b> {
    pub cell: &'a Cell,
    pub space: &'b NounSpace,
}

impl fmt::Debug for DebugPath<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "[")?;
        let mut cell = *self.cell;
        loop {
            let head = cell.head(self.space).as_atom();
            match head {
                Ok(atom) => {
                    if atom.is_direct() {
                        write!(f, "{:?}", atom.as_direct())?;
                    } else if atom.is_indirect() {
                        write!(f, "{:?}", atom.as_indirect())?;
                    } else {
                        write!(f, "{atom:?}")?;
                    }
                }
                Err(_) => {
                    write!(f, "ERR, not atom")?;
                }
            }
            match cell.tail(self.space).as_cell() {
                Ok(next_cell) => {
                    write!(f, " ")?;
                    cell = next_cell;
                }
                Err(_) => {
                    write!(f, " {:?}]", cell.tail(self.space))?;
                    break;
                }
            }
        }
        Ok(())
    }
}

// Axis iteration helpers for direct axes (u64)
pub struct DirectAxisIterator {
    axis: u64,
    cursor: usize,
}

impl DirectAxisIterator {
    #[inline(always)]
    pub fn new(axis: u64) -> Option<Self> {
        if axis == 0 {
            None
        } else {
            let cursor = if axis == 1 {
                0
            } else {
                63 - axis.leading_zeros() as usize
            };
            Some(DirectAxisIterator { axis, cursor })
        }
    }

    #[inline(always)]
    pub fn next(&mut self) -> Option<bool> {
        if self.cursor == 0 {
            None
        } else {
            self.cursor -= 1;
            Some(((self.axis >> self.cursor) & 1) != 0)
        }
    }
}

// Axis iteration helpers for indirect axes (slice of u64)
pub struct IndirectAxisIterator<'a> {
    words: &'a [u64],
    cursor: usize,
}

impl<'a> IndirectAxisIterator<'a> {
    #[inline(always)]
    pub fn new(words: &'a [u64]) -> Option<Self> {
        if words.is_empty() {
            return None;
        }

        // Find highest bit in the axis
        let mut highest_word_idx = words.len() - 1;
        while highest_word_idx > 0 && words[highest_word_idx] == 0 {
            highest_word_idx -= 1;
        }

        let highest_word = words[highest_word_idx];
        if highest_word == 0 {
            return None;
        }

        let highest_bit_in_word = 63 - highest_word.leading_zeros() as usize;
        let cursor = (highest_word_idx << 6) + highest_bit_in_word;

        Some(IndirectAxisIterator { words, cursor })
    }

    #[inline(always)]
    pub fn next(&mut self) -> Option<bool> {
        if self.cursor == 0 {
            None
        } else {
            self.cursor -= 1;
            let word_idx = self.cursor >> 6;
            let bit_idx = self.cursor & 63;
            Some(((self.words[word_idx] >> bit_idx) & 1) != 0)
        }
    }
}

// Direct axis traversal without bitvec - for u64 axes
#[inline(always)]
fn slot_direct(cell: &Cell, axis: u64, space: &NounSpace) -> Result<Noun> {
    if axis == 0 {
        return Err(Error::NotRepresentable);
    }
    if axis == 1 {
        return Ok(cell.as_noun());
    }

    let highest = 63 - axis.leading_zeros() as usize;
    let mut current = *cell;
    let mut noun = current.as_noun();

    for idx in (0..highest).rev() {
        let descend_tail = ((axis >> idx) & 1) != 0;
        let memory = unsafe { current.to_raw_pointer(space) };
        noun = unsafe {
            if descend_tail {
                (*memory).tail
            } else {
                (*memory).head
            }
        };

        if idx != 0 {
            if noun.is_cell() {
                current = unsafe { noun.cell };
            } else {
                return Err(Error::NotRepresentable);
            }
        }
    }

    Ok(noun)
}

// Trusted local axis traversal for the interpreter hot path.
// This skips location and epoch checks and assumes all traversed nouns are
// stack-local or PMA-local.
#[inline(always)]
fn slot_direct_trusted(cell: &Cell, axis: u64, space: &NounSpace) -> Result<Noun> {
    if axis == 0 {
        return Err(Error::NotRepresentable);
    }
    if axis == 1 {
        return Ok(cell.as_noun());
    }

    let highest = 63 - axis.leading_zeros() as usize;
    let mut current_raw = cell.0;
    let mut noun_raw = current_raw;

    for idx in (0..highest).rev() {
        let descend_tail = ((axis >> idx) & 1) != 0;
        let memory = unsafe { cell_ptr_from_raw_trusted(current_raw, space) };
        noun_raw = unsafe {
            if descend_tail {
                (*memory).tail.raw
            } else {
                (*memory).head.raw
            }
        };

        if idx != 0 {
            if is_cell(noun_raw) {
                current_raw = noun_raw;
            } else {
                return Err(Error::NotRepresentable);
            }
        }
    }

    Ok(unsafe { noun_from_raw(noun_raw) })
}

impl Slots for Cell {}

// Indirect axis traversal - for large axes stored in word slices
#[inline(always)]
fn slot_indirect(cell: &Cell, words: &[u64], space: &NounSpace) -> Result<Noun> {
    if words.is_empty() {
        return Err(Error::NotRepresentable);
    }

    // Find highest bit in the axis
    let mut highest_word_idx = words.len() - 1;
    while highest_word_idx > 0 && words[highest_word_idx] == 0 {
        highest_word_idx -= 1;
    }

    let highest_word = words[highest_word_idx];
    if highest_word == 0 {
        return Err(Error::NotRepresentable);
    }

    let highest_bit_in_word = 63 - highest_word.leading_zeros() as usize;
    let highest = (highest_word_idx << 6) + highest_bit_in_word;

    if highest == 0 {
        return Ok(cell.as_noun());
    }

    let mut current = *cell;
    let mut noun = current.as_noun();
    let mut idx = highest;

    while idx != 0 {
        idx -= 1;
        let word_idx = idx >> 6;
        let bit_idx = idx & 63;
        let descend_tail = ((words[word_idx] >> bit_idx) & 1) != 0;

        let memory = unsafe { current.to_raw_pointer(space) };
        noun = unsafe {
            if descend_tail {
                (*memory).tail
            } else {
                (*memory).head
            }
        };

        if idx != 0 {
            if noun.is_cell() {
                current = unsafe { noun.cell };
            } else {
                return Err(Error::NotRepresentable);
            }
        }
    }

    Ok(noun)
}

#[inline(always)]
fn slot_indirect_trusted(cell: &Cell, words: &[u64], space: &NounSpace) -> Result<Noun> {
    if words.is_empty() {
        return Err(Error::NotRepresentable);
    }

    let mut highest_word_idx = words.len() - 1;
    while highest_word_idx > 0 && words[highest_word_idx] == 0 {
        highest_word_idx -= 1;
    }

    let highest_word = words[highest_word_idx];
    if highest_word == 0 {
        return Err(Error::NotRepresentable);
    }

    let highest_bit_in_word = 63 - highest_word.leading_zeros() as usize;
    let highest = (highest_word_idx << 6) + highest_bit_in_word;

    if highest == 0 {
        return Ok(cell.as_noun());
    }

    let mut current_raw = cell.0;
    let mut noun_raw = current_raw;
    let mut idx = highest;

    while idx != 0 {
        idx -= 1;
        let word_idx = idx >> 6;
        let bit_idx = idx & 63;
        let descend_tail = ((words[word_idx] >> bit_idx) & 1) != 0;

        let memory = unsafe { cell_ptr_from_raw_trusted(current_raw, space) };
        noun_raw = unsafe {
            if descend_tail {
                (*memory).tail.raw
            } else {
                (*memory).head.raw
            }
        };

        if idx != 0 {
            if is_cell(noun_raw) {
                current_raw = noun_raw;
            } else {
                return Err(Error::NotRepresentable);
            }
        }
    }

    Ok(unsafe { noun_from_raw(noun_raw) })
}

impl private::RawSlots for Cell {
    #[inline(always)]
    fn raw_slot_direct(&self, axis: u64, space: &NounSpace) -> Result<Noun> {
        slot_direct(self, axis, space)
    }

    #[inline(always)]
    fn raw_slot_indirect(&self, axis: &[u64], space: &NounSpace) -> Result<Noun> {
        slot_indirect(self, axis, space)
    }
}

/**
 * Memory representation of the contents of a cell
 */
#[derive(Copy, Clone)]
#[repr(C)]
#[repr(packed(8))]
pub struct CellMemory {
    pub metadata: u64,
    pub head: Noun,
    pub tail: Noun,
}

#[derive(Copy, Clone)]
#[repr(C)]
#[repr(packed(8))]
pub union Atom {
    pub(crate) raw: u64,
    direct: DirectAtom,
    indirect: IndirectAtom,
}

impl Atom {
    pub fn new<A: NounAllocator>(allocator: &mut A, value: u64) -> Atom {
        if value <= DIRECT_MAX {
            unsafe { DirectAtom::new_unchecked(value).as_atom() }
        } else {
            unsafe { IndirectAtom::new_raw(allocator, 1, &value).as_atom() }
        }
    }

    // to_le_bytes and new_raw are copies.  We should be able to do this completely without copies
    // if we integrate with ibig properly.
    pub fn from_ubig<A: NounAllocator>(allocator: &mut A, big: &UBig) -> Atom {
        let bit_size = big.bit_len();
        let buffer = big.to_le_bytes_stack();
        if bit_size < 64 {
            let mut value = 0u64;
            for i in (0..bit_size).step_by(8) {
                value |= (buffer[i / 8] as u64) << i;
            }
            unsafe { DirectAtom::new_unchecked(value).as_atom() }
        } else {
            let byte_size = (big.bit_len() + 7) >> 3;
            unsafe { IndirectAtom::new_raw_bytes(allocator, byte_size, buffer.as_ptr()).as_atom() }
        }
    }

    pub fn is_direct(&self) -> bool {
        unsafe { is_direct_atom(self.raw) }
    }

    pub fn is_indirect(&self) -> bool {
        unsafe { is_indirect_atom(self.raw) }
    }

    pub(crate) fn is_normalized(&self, space: &NounSpace) -> bool {
        unsafe {
            if let Some(indirect) = self.indirect() {
                if (indirect.size(space) == 1 && *indirect.data_pointer(space) <= DIRECT_MAX)
                    || *indirect.data_pointer(space).add(indirect.size(space) - 1) == 0
                {
                    return false;
                }
            } // nothing to do for direct atom
        };

        true
    }

    pub fn as_direct(&self) -> Result<DirectAtom> {
        if self.is_direct() {
            unsafe { Ok(self.direct) }
        } else {
            Err(Error::NotDirectAtom)
        }
    }

    pub fn as_indirect(&self) -> Result<IndirectAtom> {
        if self.is_indirect() {
            unsafe { Ok(self.indirect) }
        } else {
            Err(Error::NotIndirectAtom)
        }
    }

    pub fn as_either(&self) -> Either<DirectAtom, IndirectAtom> {
        if self.is_indirect() {
            unsafe { Right(self.indirect) }
        } else {
            unsafe { Left(self.direct) }
        }
    }

    pub fn as_noun(self) -> Noun {
        Noun { atom: self }
    }

    pub fn in_space<'a>(self, space: &'a NounSpace) -> AtomHandle<'a> {
        AtomHandle::new(self, space)
    }

    /// Returns a slice of bytes in native-endian order. Currently, Sword only supports
    /// little-endian machines, so this will return little-endian.
    pub(crate) fn as_ne_bytes(&self, space: &NounSpace) -> &[u8] {
        if self.is_direct() {
            unsafe { self.direct.as_ne_bytes() }
        } else {
            unsafe { self.indirect.as_ne_bytes(space) }
        }
    }

    /// Returns Vec<u8> in native-endian order
    #[allow(clippy::wrong_self_convention)]
    pub(crate) fn to_ne_bytes(&self, space: &NounSpace) -> Vec<u8> {
        if self.is_direct() {
            unsafe { self.direct.to_ne_bytes() }
        } else {
            unsafe { self.indirect.to_ne_bytes(space) }
        }
    }

    /// Returns Vec<u8> in big-endian order
    pub(crate) fn to_be_bytes(self, space: &NounSpace) -> Vec<u8> {
        if self.is_direct() {
            unsafe { self.direct.to_be_bytes() }
        } else {
            unsafe { self.indirect.to_be_bytes(space) }
        }
    }

    /// Returns Vec<u8> in little-endian order
    pub(crate) fn to_le_bytes(self, space: &NounSpace) -> Vec<u8> {
        if self.is_direct() {
            unsafe { self.direct.to_le_bytes() }
        } else {
            unsafe { self.indirect.to_le_bytes(space) }
        }
    }

    pub(crate) fn as_u64(self, space: &NounSpace) -> Result<u64> {
        if self.is_direct() {
            Ok(unsafe { self.direct.data() })
        } else {
            unsafe { self.indirect.as_u64(space) }
        }
    }

    pub fn as_bool(self) -> Result<bool> {
        if self.is_direct() {
            Ok(unsafe { self.direct.data() == 0 })
        } else {
            Err(Error::NotRepresentable)
        }
    }

    /** Produce a SoftFloat-compatible ordered pair of 64-bit words */
    pub(crate) unsafe fn as_u64_pair(self, space: &NounSpace) -> Result<[u64; 2]> {
        if self.is_direct() {
            let u128_array = &mut [0u64; 2];
            u128_array[0] = self.as_direct()?.data();
            u128_array[1] = 0x0_u64;
            Ok(*u128_array)
        } else {
            unsafe { self.indirect.as_u64_pair(space) }
        }
    }

    pub(crate) fn as_bitslice(&self, space: &NounSpace) -> &BitSlice<u64, Lsb0> {
        if self.is_indirect() {
            unsafe { self.indirect.as_bitslice(space) }
        } else {
            unsafe { self.direct.as_bitslice() }
        }
    }

    pub(crate) fn as_bitslice_mut(&mut self, space: &NounSpace) -> &mut BitSlice<u64, Lsb0> {
        if self.is_indirect() {
            unsafe { self.indirect.as_bitslice_mut(space) }
        } else {
            unsafe { self.direct.as_bitslice_mut() }
        }
    }

    pub(crate) fn as_ubig<S: Stack>(self, stack: &mut S, space: &NounSpace) -> UBig {
        if self.is_indirect() {
            unsafe { self.indirect.as_ubig(stack, space) }
        } else {
            unsafe { self.direct.as_ubig(stack) }
        }
    }

    pub fn direct(&self) -> Option<DirectAtom> {
        if self.is_direct() {
            unsafe { Some(self.direct) }
        } else {
            None
        }
    }

    pub fn indirect(&self) -> Option<IndirectAtom> {
        if self.is_indirect() {
            unsafe { Some(self.indirect) }
        } else {
            None
        }
    }

    pub(crate) fn size(&self, space: &NounSpace) -> usize {
        match self.as_either() {
            Left(_direct) => 1,
            Right(indirect) => indirect.size(space),
        }
    }

    pub(crate) fn bit_size(&self, space: &NounSpace) -> usize {
        match self.as_either() {
            Left(direct) => direct.bit_size(),
            Right(indirect) => indirect.bit_size(space),
        }
    }

    pub(crate) fn data_pointer(&self, space: &NounSpace) -> *const u64 {
        match self.as_either() {
            Left(_direct) => (self as *const Atom) as *const u64,
            Right(indirect) => indirect.data_pointer(space),
        }
    }

    pub(crate) unsafe fn normalize(&mut self, space: &NounSpace) -> Atom {
        if self.is_indirect() {
            self.indirect.normalize_as_atom(space)
        } else {
            *self
        }
    }

    /** Make an atom from a raw u64
     *
     * # Safety
     *
     * Note that the [u64] parameter is *not*, in general, the value of the atom!
     *
     * In particular, anything with the high bit set will be treated as a tagged pointer.
     * This method is only to be used to restore an atom from the raw [u64] representation
     * returned by [Noun::as_raw], and should only be used if we are sure the restored noun is in
     * fact an atom.
     */
    pub unsafe fn from_raw(raw: u64) -> Atom {
        Atom { raw }
    }
}

impl fmt::Debug for Atom {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.as_noun().fmt(f)
    }
}

#[derive(Copy, Clone)]
#[repr(C)]
#[repr(packed(8))]
pub union Allocated {
    raw: u64,
    indirect: IndirectAtom,
    cell: Cell,
}

impl Allocated {
    pub fn is_indirect(&self) -> bool {
        unsafe { is_indirect_atom(self.raw) }
    }

    pub fn is_cell(&self) -> bool {
        unsafe { is_cell(self.raw) }
    }

    #[allow(clippy::wrong_self_convention)]
    pub(crate) unsafe fn to_raw_pointer(&self, space: &NounSpace) -> *const u64 {
        let tagged = TaggedPtr::from_raw(self.raw);
        if self.is_indirect() {
            tagged.resolve_const(INDIRECT_MASK, space) as *const u64
        } else {
            tagged.resolve_const(CELL_MASK, space) as *const u64
        }
    }

    pub(crate) unsafe fn to_raw_pointer_mut(&mut self, space: &NounSpace) -> *mut u64 {
        let tagged = TaggedPtr::from_raw(self.raw);
        if self.is_indirect() {
            tagged.resolve_mut(INDIRECT_MASK, space) as *mut u64
        } else {
            tagged.resolve_mut(CELL_MASK, space) as *mut u64
        }
    }

    pub(crate) unsafe fn const_to_raw_pointer_mut(self, space: &NounSpace) -> *mut u64 {
        let tagged = TaggedPtr::from_raw(self.raw);
        if self.is_indirect() {
            tagged.resolve_mut(INDIRECT_MASK, space) as *mut u64
        } else {
            tagged.resolve_mut(CELL_MASK, space) as *mut u64
        }
    }

    pub(crate) unsafe fn forwarding_pointer(&self, space: &NounSpace) -> Option<Allocated> {
        match self.as_either() {
            Left(indirect) => indirect.forwarding_pointer(space).map(|i| i.as_allocated()),
            Right(cell) => cell.forwarding_pointer(space).map(|c| c.as_allocated()),
        }
    }

    pub(crate) unsafe fn get_metadata(&self, space: &NounSpace) -> u64 {
        *(self.to_raw_pointer(space))
    }

    pub(crate) unsafe fn set_metadata(&mut self, metadata: u64, space: &NounSpace) {
        *(self.const_to_raw_pointer_mut(space)) = metadata;
    }

    pub fn as_either(&self) -> Either<IndirectAtom, Cell> {
        if self.is_indirect() {
            unsafe { Left(self.indirect) }
        } else {
            unsafe { Right(self.cell) }
        }
    }

    pub fn as_ref_either(&self) -> Either<&IndirectAtom, &Cell> {
        if self.is_indirect() {
            unsafe { Left(&self.indirect) }
        } else {
            unsafe { Right(&self.cell) }
        }
    }

    pub fn cell(&self) -> Option<Cell> {
        if self.is_cell() {
            unsafe { Some(self.cell) }
        } else {
            None
        }
    }

    pub fn as_noun(&self) -> Noun {
        Noun { allocated: *self }
    }

    pub(crate) fn get_cached_mug(self: Allocated, space: &NounSpace) -> Option<u32> {
        unsafe {
            let bottom_metadata = self.get_metadata(space) as u32 & 0x7FFFFFFF; // magic number: LS 31 bits
            if bottom_metadata > 0 {
                Some(bottom_metadata)
            } else {
                None
            }
        }
    }
}

impl fmt::Debug for Allocated {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.as_noun().fmt(f)
    }
}

#[derive(Copy, Clone)]
#[repr(C)]
#[repr(packed(8))]
pub union Noun {
    pub(crate) raw: u64,
    direct: DirectAtom,
    indirect: IndirectAtom,
    atom: Atom,
    cell: Cell,
    allocated: Allocated,
}

impl Noun {
    pub fn is_none(self) -> bool {
        unsafe { self.raw == u64::MAX }
    }

    pub fn in_space<'a>(self, space: &'a NounSpace) -> NounHandle<'a> {
        NounHandle::new(self, space)
    }

    pub fn is_direct(&self) -> bool {
        unsafe { is_direct_atom(self.raw) }
    }

    pub fn is_indirect(&self) -> bool {
        unsafe { is_indirect_atom(self.raw) }
    }

    pub fn is_atom(&self) -> bool {
        self.is_direct() || self.is_indirect()
    }

    pub fn is_allocated(&self) -> bool {
        self.is_indirect() || self.is_cell()
    }

    pub(crate) fn repr(&self, space: &NounSpace) -> NounRepr {
        let raw = unsafe { self.as_raw() };
        if is_direct_atom(raw) {
            return NounRepr::Direct;
        }

        enum AllocKind {
            Indirect,
            Cell,
            Forwarding,
        }

        let (mask, kind) = if is_indirect_atom(raw) {
            (INDIRECT_MASK, AllocKind::Indirect)
        } else if is_cell(raw) {
            (CELL_MASK, AllocKind::Cell)
        } else if raw & FORWARDING_MASK == FORWARDING_TAG {
            (FORWARDING_MASK, AllocKind::Forwarding)
        } else {
            unreachable!("unknown noun tag for raw {:#x}", raw);
        };

        let tagged = TaggedPtr::from_raw(raw);
        let location = match tagged.location() {
            PtrLocation::Offset => {
                if matches!(kind, AllocKind::Forwarding) {
                    panic!("forwarding pointers cannot be offset-form");
                }
                AllocLocation::PmaOffset
            }
            PtrLocation::Stack => {
                let ptr = (tagged.payload(mask) << 3) as *const u8;
                space.classify_ptr(ptr)
            }
        };

        match kind {
            AllocKind::Indirect => NounRepr::Indirect(location),
            AllocKind::Cell => NounRepr::Cell(location),
            AllocKind::Forwarding => NounRepr::Forwarding(location),
        }
    }

    pub(crate) fn allocated_location(&self, space: &NounSpace) -> Option<AllocLocation> {
        self.repr(space).location()
    }

    #[inline]
    pub(crate) fn is_stack_allocated(&self, space: &NounSpace) -> bool {
        matches!(self.allocated_location(space), Some(AllocLocation::Stack))
    }

    pub fn is_cell(&self) -> bool {
        unsafe { is_cell(self.raw) }
    }

    pub fn as_direct(&self) -> Result<DirectAtom> {
        if self.is_direct() {
            unsafe { Ok(self.direct) }
        } else {
            Err(Error::NotDirectAtom)
        }
    }

    pub fn as_indirect(&self) -> Result<IndirectAtom> {
        if self.is_indirect() {
            unsafe { Ok(self.indirect) }
        } else {
            Err(Error::NotIndirectAtom)
        }
    }

    pub fn as_cell(&self) -> Result<Cell> {
        if self.is_cell() {
            unsafe { Ok(self.cell) }
        } else {
            Err(Error::NotCell)
        }
    }

    pub fn as_atom(&self) -> Result<Atom> {
        if self.is_atom() {
            unsafe { Ok(self.atom) }
        } else {
            Err(Error::NotAtom)
        }
    }

    pub fn as_allocated(&self) -> Result<Allocated> {
        if self.is_allocated() {
            unsafe { Ok(self.allocated) }
        } else {
            Err(Error::NotAllocated)
        }
    }

    pub fn as_either_atom_cell(&self) -> Either<Atom, Cell> {
        if self.is_cell() {
            unsafe { Right(self.cell) }
        } else {
            unsafe { Left(self.atom) }
        }
    }

    pub fn as_either_direct_allocated(self) -> Either<DirectAtom, Allocated> {
        if self.is_direct() {
            unsafe { Left(self.direct) }
        } else {
            unsafe { Right(self.allocated) }
        }
    }

    pub fn as_ref_either_direct_allocated(&self) -> Either<&DirectAtom, &Allocated> {
        if self.is_direct() {
            unsafe { Left(&self.direct) }
        } else {
            unsafe { Right(&self.allocated) }
        }
    }

    pub fn as_ref_mut_either_direct_allocated(
        &mut self,
    ) -> Either<&mut DirectAtom, &mut Allocated> {
        if self.is_direct() {
            unsafe { Left(&mut self.direct) }
        } else {
            unsafe { Right(&mut self.allocated) }
        }
    }

    pub fn atom(&self) -> Option<Atom> {
        if self.is_atom() {
            unsafe { Some(self.atom) }
        } else {
            None
        }
    }

    pub fn cell(&self) -> Option<Cell> {
        if self.is_cell() {
            unsafe { Some(self.cell) }
        } else {
            None
        }
    }

    pub fn direct(&self) -> Option<DirectAtom> {
        if self.is_direct() {
            unsafe { Some(self.direct) }
        } else {
            None
        }
    }

    pub fn indirect(&self) -> Option<IndirectAtom> {
        if self.is_indirect() {
            unsafe { Some(self.indirect) }
        } else {
            None
        }
    }

    pub fn allocated(&self) -> Option<Allocated> {
        if self.is_allocated() {
            unsafe { Some(self.allocated) }
        } else {
            None
        }
    }

    /** Are these the same noun */
    pub unsafe fn raw_equals(&self, other: &Noun) -> bool {
        self.raw == other.raw
    }

    pub unsafe fn as_raw(&self) -> u64 {
        self.raw
    }

    pub unsafe fn from_raw(raw: u64) -> Noun {
        Noun { raw }
    }

    /** Produce the total size of a noun, in words
     *
     * This counts the total size, see mass_frame() to count the size in the current frame.
     */
    pub(crate) fn mass(self, space: &NounSpace) -> usize {
        unsafe {
            let res = self.mass_wind(space, &|_| true);
            self.mass_unwind(space, &|_| true);
            res
        }
    }

    /** Produce the size of a noun in the current frame, in words */
    pub(crate) fn mass_frame(self, stack: &NockStack, space: &NounSpace) -> usize {
        unsafe {
            let res = self.mass_wind(space, &|p| stack.is_in_frame(p));
            self.mass_unwind(space, &|p| stack.is_in_frame(p));
            res
        }
    }

    /** Produce the total size of a noun, in words
     *
     * `inside` determines whether a pointer should be counted.  If it returns false, we also do
     * not recurse into that noun if it is a cell.  See mass_frame() for an example.
     *
     * This "winds up" the mass calculation, which includes setting the 32nd bit of the metadata to
     * mark nouns that have already been counted.
     *
     * This is unsafe because you *must* call mass_unwind() with the same `inside` function to
     * unmark the noun.  This is exposed so that you can count several the "mass difference" of a
     * series of nouns.  If you call this twice consecutively, the first result will be the mass of
     * the first noun, and the second will be the mass of the second noun minus the overlap with
     * the first noun.
     */
    pub(crate) unsafe fn mass_wind(
        self,
        space: &NounSpace,
        inside: &impl Fn(*const u64) -> bool,
    ) -> usize {
        if let Ok(mut allocated) = self.as_allocated() {
            if inside(allocated.to_raw_pointer(space)) {
                if allocated.get_metadata(space) & (1 << 32) == 0 {
                    allocated.set_metadata(allocated.get_metadata(space) | (1 << 32), space);
                    match allocated.as_either() {
                        Left(indirect) => indirect.size(space) + 2,
                        Right(cell) => {
                            let cell_handle = cell.in_space(space);
                            word_size_of::<CellMemory>()
                                + cell_handle.head().noun().mass_wind(space, inside)
                                + cell_handle.tail().noun().mass_wind(space, inside)
                        }
                    }
                } else {
                    0
                }
            } else {
                0
            }
        } else {
            0
        }
    }

    /** See mass_wind() */
    pub(crate) unsafe fn mass_unwind(
        self,
        space: &NounSpace,
        inside: &impl Fn(*const u64) -> bool,
    ) {
        if let Ok(mut allocated) = self.as_allocated() {
            if inside(allocated.to_raw_pointer(space)) {
                allocated.set_metadata(allocated.get_metadata(space) & !(1 << 32), space);
                if let Right(cell) = allocated.as_either() {
                    let cell_handle = cell.in_space(space);
                    cell_handle.head().noun().mass_unwind(space, inside);
                    cell_handle.tail().noun().mass_unwind(space, inside);
                }
            }
        }
    }
}

impl fmt::Debug for Noun {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        unsafe {
            if self.is_direct() {
                write!(f, "{:?}", self.direct)
            } else if self.is_indirect() {
                write!(f, "{:?}", self.indirect)
            } else if self.is_cell() {
                write!(f, "{:?}", self.cell)
            } else {
                write!(f, "Noun::Unknown({:x})", self.raw)
            }
        }
    }
}

impl Slots for Noun {}
impl private::RawSlots for Noun {
    #[inline(always)]
    fn raw_slot_direct(&self, axis: u64, space: &NounSpace) -> Result<Noun> {
        match self.as_either_atom_cell() {
            Right(cell) => cell.raw_slot_direct(axis, space),
            Left(_atom) => {
                if axis == 1 {
                    Ok(*self)
                } else {
                    // Axis tried to descend through atom
                    Err(Error::NotCell)
                }
            }
        }
    }

    #[inline(always)]
    fn raw_slot_indirect(&self, axis: &[u64], space: &NounSpace) -> Result<Noun> {
        match self.as_either_atom_cell() {
            Right(cell) => cell.raw_slot_indirect(axis, space),
            Left(_atom) => {
                // Check if axis is 1 (all words are 0 except word[0] & 1 == 1)
                if axis.len() == 1 && axis[0] == 1 {
                    Ok(*self)
                } else if axis.is_empty() || (axis.len() == 1 && axis[0] == 0) {
                    Err(Error::NotRepresentable)
                } else {
                    // Axis tried to descend through atom
                    Err(Error::NotCell)
                }
            }
        }
    }
}

impl Noun {
    #[inline(always)]
    fn raw_slot_direct_trusted(self, axis: u64, space: &NounSpace) -> Result<Noun> {
        match self.as_either_atom_cell() {
            Right(cell) => slot_direct_trusted(&cell, axis, space),
            Left(_atom) => {
                if axis == 1 {
                    Ok(self)
                } else if axis == 0 {
                    Err(Error::NotRepresentable)
                } else {
                    Err(Error::NotCell)
                }
            }
        }
    }

    #[inline(always)]
    fn raw_slot_indirect_trusted(self, axis: &[u64], space: &NounSpace) -> Result<Noun> {
        match self.as_either_atom_cell() {
            Right(cell) => slot_indirect_trusted(&cell, axis, space),
            Left(_atom) => {
                if axis.len() == 1 && axis[0] == 1 {
                    Ok(self)
                } else if axis.is_empty() || (axis.len() == 1 && axis[0] == 0) {
                    Err(Error::NotRepresentable)
                } else {
                    Err(Error::NotCell)
                }
            }
        }
    }

    #[inline(always)]
    pub(crate) fn slot_direct_trusted_or_checked(
        self,
        axis: u64,
        space: &NounSpace,
    ) -> Result<Noun> {
        self.raw_slot_direct_trusted(axis, space)
    }

    #[inline(always)]
    pub(crate) fn slot_indirect_trusted_or_checked(
        self,
        axis: &[u64],
        space: &NounSpace,
    ) -> Result<Noun> {
        self.raw_slot_indirect_trusted(axis, space)
    }

    #[inline(always)]
    pub(crate) fn slot_atom_trusted_or_checked(
        self,
        atom: Atom,
        space: &NounSpace,
    ) -> Result<Noun> {
        match atom.as_either() {
            Left(direct) => self.slot_direct_trusted_or_checked(direct.data(), space),
            Right(indirect) => {
                self.slot_indirect_trusted_or_checked(indirect.as_slice_trusted(space), space)
            }
        }
    }
}

/**
 * An allocation object (probably a mem::NockStack) which can allocate a memory buffer sized to
 * a certain number of nouns
 */
pub trait NounAllocator: Sized + Stack {
    /** Allocate memory for some multiple of the size of a noun
     *
     * This should allocate *two more* `u64`s than `words` to make space for the size and metadata
     */
    unsafe fn alloc_indirect(&mut self, words: usize) -> *mut u64;

    /** Allocate memory for a cell */
    unsafe fn alloc_cell(&mut self) -> *mut CellMemory;

    /** Allocate space for a struct in a stack frame */
    unsafe fn alloc_struct<T>(&mut self, count: usize) -> *mut T;

    /** Check if two allocated nouns are equal **/
    unsafe fn equals(&mut self, a: *mut Noun, b: *mut Noun) -> bool;

    fn noun_space(&self) -> NounSpace;
}

/**
 * Implementing types allow component Nouns to be retreived by numeric axis
 */
pub(crate) trait Slots: private::RawSlots {
    /**
     * Retrieve component Noun at given axis, or fail with descriptive error
     */
    fn slot(&self, axis: u64, space: &NounSpace) -> Result<Noun> {
        self.raw_slot_direct(axis, space)
    }

    /**
     * Retrieve component Noun at axis given as Atom, or fail with descriptive error
     */
    fn slot_atom(&self, atom: Atom, space: &NounSpace) -> Result<Noun> {
        match atom.as_either() {
            Left(direct) => self.raw_slot_direct(direct.data(), space),
            Right(indirect) => self.raw_slot_indirect(indirect.as_slice(space), space),
        }
    }
}

/**
 * Implementation methods that should not be made available to derived crates
 */
mod private {
    use crate::noun::{Noun, NounSpace, Result};

    /**
     * Implementation of the Slots trait
     */
    pub trait RawSlots {
        /**
         * Actual logic of retreiving Noun object at some axis (direct)
         */
        fn raw_slot_direct(&self, axis: u64, space: &NounSpace) -> Result<Noun>;

        /**
         * Actual logic of retreiving Noun object at some axis (indirect)
         */
        fn raw_slot_indirect(&self, axis: &[u64], space: &NounSpace) -> Result<Noun>;
    }
}

#[cfg(test)]
mod tests {
    use crate::jets::util::test::init_context;
    use crate::noun::{Cell, NounSpace, Slots, D};

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_slot_direct_simple() {
        let mut context = init_context();
        let space = NounSpace::stack_only(&context.stack);
        let cell = Cell::new(&mut context.stack, D(1), D(2));

        // axis 1 returns the whole cell
        assert!(unsafe {
            cell.slot(1, &space)
                .expect("axis 1 should resolve to the whole cell")
                .raw_equals(&cell.as_noun())
        });

        // axis 2 returns head
        assert!(unsafe {
            cell.slot(2, &space)
                .expect("axis 2 should resolve to the head")
                .raw_equals(&D(1))
        });

        // axis 3 returns tail
        assert!(unsafe {
            cell.slot(3, &space)
                .expect("axis 3 should resolve to the tail")
                .raw_equals(&D(2))
        });
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_slot_direct_nested() {
        let mut context = init_context();
        let space = NounSpace::stack_only(&context.stack);
        let inner = Cell::new(&mut context.stack, D(3), D(4));
        // cell = [1 [3 4]]
        let cell = Cell::new(&mut context.stack, D(1), inner.as_noun());

        // axis 6 = 110 binary = tail then head = head of tail = 3
        assert!(unsafe {
            cell.slot(6, &space)
                .expect("axis 6 should resolve to tail/head")
                .raw_equals(&D(3))
        });

        // axis 7 = 111 binary = tail then tail = tail of tail = 4
        assert!(unsafe {
            cell.slot(7, &space)
                .expect("axis 7 should resolve to tail/tail")
                .raw_equals(&D(4))
        });

        // axis 4 = 100 binary = head then stop = should fail (head is atom)
        assert!(cell.slot(4, &space).is_err());

        // cell2 = [[3 4] 2]
        let cell2 = Cell::new(&mut context.stack, inner.as_noun(), D(2));
        // axis 5 = 101 binary = head then tail = tail of head = 4
        assert!(unsafe {
            cell2
                .slot(5, &space)
                .expect("axis 5 should resolve to head/tail")
                .raw_equals(&D(4))
        });
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_slot_zero_axis() {
        let mut context = init_context();
        let space = NounSpace::stack_only(&context.stack);
        let cell = Cell::new(&mut context.stack, D(1), D(2));

        // axis 0 should fail
        assert!(cell.slot(0, &space).is_err());
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_with_brand_valid_usage() {
        let mut context = init_context();
        let space = NounSpace::stack_only(&context.stack);
        let cell = Cell::new(&mut context.stack, D(10), D(20)).as_noun();

        space.with_brand(|space| {
            let cell = space.handle(cell).as_cell().expect("cell");
            assert_eq!(
                cell.head()
                    .as_atom()
                    .expect("head atom")
                    .as_u64()
                    .expect("head atom should fit in u64"),
                10
            );
            assert_eq!(
                cell.tail()
                    .as_atom()
                    .expect("tail atom")
                    .as_u64()
                    .expect("tail atom should fit in u64"),
                20
            );
            assert!(matches!(
                cell.as_noun().repr(),
                crate::noun::NounRepr::Cell(_)
            ));
        });
    }
}

#[cfg(test)]
mod test {
    use ibig::ubig;

    use crate::jets::util::test::init_context;
    use crate::noun::Atom;

    #[test]
    //  APOLOGIA: ibig/ubig ManuallyDrops Vec, we are aware, we plan on purging it
    #[cfg_attr(miri, ignore)]
    fn test_to_ne_bytes_direct() {
        let mut context = init_context();
        let space = context.stack.noun_space();
        let big = ubig!(0x1234567890abcdefa0);
        let atom = Atom::from_ubig(&mut context.stack, &big);
        let bytes = atom.in_space(&space).to_ne_bytes();
        #[cfg(target_endian = "little")]
        {
            assert_eq!(
                bytes,
                vec![
                    0xa0, 0xef, 0xcd, 0xab, 0x90, 0x78, 0x56, 0x34, 0x12, 0x00, 0x00, 0x00, 0x00,
                    0x00, 0x00, 0x00
                ]
            );
        }
        #[cfg(target_endian = "big")]
        {
            assert_eq!(
                bytes,
                vec![
                    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x12, 0x34, 0x56, 0x78, 0x90, 0xab,
                    0xcd, 0xef, 0xa0
                ]
            );
        }
    }

    #[test]
    //  APOLOGIA: ibig/ubig ManuallyDrops Vec, we are aware, we plan on purging it
    #[cfg_attr(miri, ignore)]
    fn test_to_ne_bytes_indirect() {
        let mut context = init_context();
        let space = context.stack.noun_space();
        let atom = Atom::new(&mut context.stack, 0x1234);
        let bytes = atom.in_space(&space).to_ne_bytes();
        #[cfg(target_endian = "little")]
        {
            assert_eq!(bytes, vec![0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        }
        #[cfg(target_endian = "big")]
        {
            assert_eq!(bytes, vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x12, 0x34]);
        }
    }

    #[test]
    //  APOLOGIA: ibig/ubig ManuallyDrops Vec, we are aware, we plan on purging it
    #[cfg_attr(miri, ignore)]
    fn test_to_x_bytes_direct() {
        let mut context = init_context();
        let space = context.stack.noun_space();
        let atom = Atom::new(&mut context.stack, 0x1234);
        let bytes_le = atom.in_space(&space).to_le_bytes();
        assert_eq!(
            bytes_le,
            vec![0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        );

        let bytes_be = atom.in_space(&space).to_be_bytes();
        assert_eq!(
            bytes_be,
            vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x12, 0x34]
        );
    }

    #[test]
    //  APOLOGIA: ibig/ubig ManuallyDrops Vec, we are aware, we plan on purging it
    #[cfg_attr(miri, ignore)]
    fn test_to_le_bytes_indirect() {
        let mut context = init_context();
        let space = context.stack.noun_space();
        let big = ubig!(0x1234567890abcd);
        let atom = Atom::from_ubig(&mut context.stack, &big);
        let bytes = atom.in_space(&space).to_le_bytes();
        assert_eq!(bytes, vec![0xcd, 0xab, 0x90, 0x78, 0x56, 0x34, 0x12, 0x00]);
        //
        let big = ubig!(0x1234567890abcdefa0);
        let atom = Atom::from_ubig(&mut context.stack, &big);
        let bytes = atom.in_space(&space).to_le_bytes();
        assert_eq!(
            bytes,
            vec![
                0xa0, 0xef, 0xcd, 0xab, 0x90, 0x78, 0x56, 0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00
            ],
        );
    }

    #[test]
    //  APOLOGIA: ibig/ubig ManuallyDrops Vec, we are aware, we plan on purging it
    #[cfg_attr(miri, ignore)]
    fn test_to_be_bytes_indirect() {
        let mut context = init_context();
        let space = context.stack.noun_space();
        let big = ubig!(0x34567890abcdef);
        let atom = Atom::from_ubig(&mut context.stack, &big);
        let bytes = atom.in_space(&space).to_be_bytes();
        assert_eq!(bytes, vec![0x00, 0x34, 0x56, 0x78, 0x90, 0xab, 0xcd, 0xef]);
        //
        let big = ubig!(0x1234567890abcdefa0);
        let atom = Atom::from_ubig(&mut context.stack, &big);
        let bytes = atom.in_space(&space).to_be_bytes();
        assert_eq!(
            bytes,
            vec![
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x12, 0x34, 0x56, 0x78, 0x90, 0xab, 0xcd,
                0xef, 0xa0
            ]
        );
    }
}
