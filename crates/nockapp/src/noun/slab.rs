#![allow(clippy::missing_safety_doc)]

use std::alloc::Layout;
use std::fmt::Debug;
use std::mem::size_of;
use std::ptr::copy_nonoverlapping;

use bitvec::prelude::{BitSlice, BitVec, Lsb0};
use bitvec::view::BitView;
use bitvec::{bits, bitvec};
use bytes::Bytes;
use either::Either;
use ibig::Stack;
use intmap::IntMap;
use nockvm::ext::noun_equality;
use nockvm::mem::NockStack;
use nockvm::mug::{calc_atom_mug_u32, calc_cell_mug_u32, get_mug, set_mug};
use nockvm::noun::{
    Atom, Cell, CellMemory, DirectAtom, IndirectAtom, Noun, NounAllocator, NounSpace, D, DIRECT_MAX,
};
use nockvm::serialization::{met0_u64_to_usize, met0_usize};
use thiserror::Error;

const CELL_MEM_WORD_SIZE: usize = (size_of::<CellMemory>() + 7) >> 3;

/// A self-contained arena for allocating nouns.
pub struct NounSlab<J = NockJammer> {
    root: Noun,
    slabs: Vec<(*mut u8, Layout)>,
    allocation_start: *mut u64,
    allocation_stop: *mut u64,
    _phantom: std::marker::PhantomData<J>,
}

impl<J> Debug for NounSlab<J> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NounSlab")
            .field("root", &self.root)
            .field("slabs", &self.slabs)
            .field("allocation_start", &self.allocation_start)
            .field("allocation_stop", &self.allocation_stop)
            .finish()
    }
}

impl<J> NounSlab<J> {
    fn contains_ptr(&self, ptr: *const u8) -> bool {
        self.slabs.iter().any(|(base, layout)| unsafe {
            !base.is_null() && ptr >= *base as *const u8 && ptr < base.add(layout.size())
        })
    }

    fn rehome_noun(&mut self, noun: Noun) -> Noun {
        let mut copied = IntMap::new();
        self.rehome_noun_inner(noun, &mut copied)
    }

    fn rehome_noun_inner(&mut self, noun: Noun, copied: &mut IntMap<u64, Noun>) -> Noun {
        match noun.as_either_direct_allocated() {
            Either::Left(direct) => direct.as_noun(),
            Either::Right(allocated) => match allocated.as_either() {
                Either::Left(indirect) => {
                    let Some(data_ptr) = indirect.data_pointer_stack() else {
                        panic!(
                            "Cannot splice offset-form noun into NounSlab without a source NounSpace"
                        );
                    };
                    if self.contains_ptr(data_ptr as *const u8) {
                        return noun;
                    }

                    let src_ptr = unsafe { indirect.to_raw_pointer_stack() };
                    let src_key = src_ptr as u64;
                    if let Some(copied_noun) = copied.get(src_key) {
                        return *copied_noun;
                    }

                    let size = unsafe { *src_ptr.add(1) as usize };
                    let new_mem = unsafe { self.alloc_indirect(size) };
                    unsafe {
                        copy_nonoverlapping(src_ptr, new_mem, size + 2);
                    }
                    let copied_noun =
                        unsafe { IndirectAtom::from_raw_pointer(new_mem).as_atom().as_noun() };
                    copied.insert(src_key, copied_noun);
                    copied_noun
                }
                Either::Right(cell) => {
                    let Some(cell_ptr) = cell.stack_memory_pointer() else {
                        panic!(
                            "Cannot splice offset-form noun into NounSlab without a source NounSpace"
                        );
                    };
                    let src_key = cell_ptr as u64;
                    if let Some(copied_noun) = copied.get(src_key) {
                        return *copied_noun;
                    }

                    let source_head = unsafe { (*cell_ptr).head };
                    let source_tail = unsafe { (*cell_ptr).tail };
                    let rehomed_head = self.rehome_noun_inner(source_head, copied);
                    let rehomed_tail = self.rehome_noun_inner(source_tail, copied);

                    if self.contains_ptr(cell_ptr as *const u8)
                        && unsafe { rehomed_head.raw_equals(&source_head) }
                        && unsafe { rehomed_tail.raw_equals(&source_tail) }
                    {
                        copied.insert(src_key, noun);
                        return noun;
                    }

                    let copied_noun = Cell::new(self, rehomed_head, rehomed_tail).as_noun();
                    copied.insert(src_key, copied_noun);
                    copied_noun
                }
            },
        }
    }

    pub fn coerce_jammer<I>(mut self) -> NounSlab<I> {
        let slabs = std::mem::take(&mut self.slabs);
        NounSlab {
            root: self.root,
            slabs,
            allocation_start: self.allocation_start,
            allocation_stop: self.allocation_stop,
            _phantom: std::marker::PhantomData,
        }
    }

    unsafe fn raw_alloc(new_layout: Layout) -> *mut u8 {
        if new_layout.size() == 0 {
            std::alloc::handle_alloc_error(new_layout);
        }
        assert!(new_layout.align().is_power_of_two(), "Invalid alignment");
        let slab = std::alloc::alloc(new_layout);
        if slab.is_null() {
            std::alloc::handle_alloc_error(new_layout);
        } else {
            slab
        }
    }

    pub fn to_vec(&self) -> Vec<Self> {
        let space = self.noun_space();
        self.root
            .in_space(&space)
            .list_iter()
            .map(|n| {
                let mut slab = Self::new();
                slab.copy_into(n.noun(), &space);
                slab
            })
            .collect()
    }

    pub fn modify<F: FnOnce(Noun) -> Vec<Noun>>(&mut self, f: F) {
        let new_root_base = f(self.root);
        let rehomed_root_base: Vec<Noun> = new_root_base
            .into_iter()
            .map(|noun| self.rehome_noun(noun))
            .collect();
        let new_root = nockvm::noun::T(self, &rehomed_root_base);
        self.set_root(new_root);
    }

    pub fn modify_noun<F: FnOnce(Noun) -> Noun>(&mut self, f: F) {
        let new_root = self.rehome_noun(f(self.root));
        self.set_root(new_root);
    }

    pub fn modify_with_imports3<F: FnOnce((Noun, Noun, Noun), Noun) -> Vec<Noun>>(
        &mut self,
        f: F,
        imports: (Noun, Noun, Noun),
        space: &NounSpace,
    ) {
        let old_root = self.root;
        let imported = (
            self.copy_into(imports.0, space),
            self.copy_into(imports.1, space),
            self.copy_into(imports.2, space),
        );
        let new_root_base = f(imported, old_root);
        let rehomed_root_base: Vec<Noun> = new_root_base
            .into_iter()
            .map(|noun| self.rehome_noun(noun))
            .collect();
        let new_root = nockvm::noun::T(self, &rehomed_root_base);
        self.set_root(new_root);
    }
}

impl<J> Clone for NounSlab<J> {
    fn clone(&self) -> Self {
        let mut slab = Self::new();
        let space = self.noun_space();
        slab.copy_into(self.root, &space);
        slab
    }
}

impl<J> NounAllocator for NounSlab<J> {
    unsafe fn alloc_indirect(&mut self, words: usize) -> *mut u64 {
        let raw_size = words + 2;

        // Make sure we have enough space
        if self.allocation_start.is_null()
            || self.allocation_start.add(raw_size) > self.allocation_stop
        {
            let next_idx = std::cmp::max(self.slabs.len(), min_idx_for_size(raw_size));
            self.slabs
                .resize(next_idx + 1, (std::ptr::null_mut(), Layout::new::<u8>()));
            let new_size = idx_to_size(next_idx);
            let new_layout = Layout::array::<u64>(new_size).unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            let new_slab = Self::raw_alloc(new_layout);
            let new_slab_u64 = new_slab as *mut u64;
            self.slabs[next_idx] = (new_slab, new_layout);
            self.allocation_start = new_slab_u64;
            self.allocation_stop = new_slab_u64.add(new_size);
        }

        let new_indirect_ptr = self.allocation_start;
        self.allocation_start = self.allocation_start.add(raw_size);
        new_indirect_ptr
    }
    unsafe fn alloc_cell(&mut self) -> *mut CellMemory {
        if self.allocation_start.is_null()
            || self.allocation_start.add(CELL_MEM_WORD_SIZE) > self.allocation_stop
        // || (self.allocation_start as usize) + CELL_MEM_WORD_SIZE > (self.allocation_stop as usize)
        // || (self.allocation_start.expose_provenance()) + CELL_MEM_WORD_SIZE > (self.allocation_stop.expose_provenance())
        // || (self.allocation_start as usize) + (CELL_MEM_WORD_SIZE * std::mem::size_of::<u64>()) > (self.allocation_stop as usize)
        {
            let next_idx = std::cmp::max(self.slabs.len(), min_idx_for_size(CELL_MEM_WORD_SIZE));
            self.slabs
                .resize(next_idx + 1, (std::ptr::null_mut(), Layout::new::<u8>()));
            let new_size = idx_to_size(next_idx);
            let new_layout = Layout::array::<u64>(new_size).unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            let new_slab = Self::raw_alloc(new_layout);
            let new_slab_u64 = new_slab as *mut u64;
            self.slabs[next_idx] = (new_slab, new_layout);
            self.allocation_start = new_slab_u64;
            self.allocation_stop = new_slab_u64.add(new_size);
        }
        let new_cell_ptr = self.allocation_start as *mut CellMemory;
        // self.allocation_start = ((self.allocation_start.expose_provenance()) + CELL_MEM_WORD_SIZE) as *mut u64;
        self.allocation_start = std::ptr::with_exposed_provenance_mut(
            self.allocation_start.expose_provenance()
                + (CELL_MEM_WORD_SIZE * std::mem::size_of::<u64>()),
        );
        new_cell_ptr
    }

    unsafe fn alloc_struct<T>(&mut self, count: usize) -> *mut T {
        let layout = Layout::array::<T>(count).expect("Bad layout in alloc_struct");
        let word_size = (layout.size() + 7) >> 3;
        assert!(layout.align() <= std::mem::size_of::<u64>());
        if self.allocation_start.is_null()
            || self.allocation_start.add(word_size) > self.allocation_stop
        {
            let next_idx = std::cmp::max(self.slabs.len(), min_idx_for_size(word_size));
            self.slabs
                .resize(next_idx + 1, (std::ptr::null_mut(), Layout::new::<u8>()));
            let new_size = idx_to_size(next_idx);
            let new_layout = Layout::array::<u64>(new_size).unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            let new_slab = Self::raw_alloc(new_layout);
            let new_slab_u64 = new_slab as *mut u64;
            self.slabs[next_idx] = (new_slab, new_layout);
            self.allocation_start = new_slab_u64;
            self.allocation_stop = new_slab_u64.add(new_size);
        }
        let new_struct_ptr = self.allocation_start as *mut T;
        self.allocation_start = self.allocation_start.add(word_size);
        new_struct_ptr
    }

    unsafe fn equals(&mut self, a: *mut Noun, b: *mut Noun) -> bool {
        let a = unsafe { &mut *a };
        let b = unsafe { &mut *b };
        let space = self.noun_space();
        noun_equality((*a).in_space(&space), (*b).in_space(&space))
    }

    fn noun_space(&self) -> NounSpace {
        NounSpace::empty().with_extra_ptr_ranges(self.ptr_ranges())
    }
}

/// # Safety: sending a slab across threads relies on the invariant that every noun reachable from
/// its root is allocated inside the slab.
unsafe impl Send for NounSlab {}

impl<J> Default for NounSlab<J> {
    fn default() -> Self {
        Self::new()
    }
}

impl<J> NounSlab<J> {
    pub fn from_noun(noun: Noun, space: &NounSpace) -> Self {
        let mut slab = Self::new();
        slab.copy_into(noun, space);
        slab
    }
}

impl<const N: usize, J> From<[Noun; N]> for NounSlab<J> {
    fn from(nouns: [Noun; N]) -> Self {
        let mut slab = Self::new();
        let nouns = nouns.map(|noun| slab.rehome_noun(noun));
        let new_root = nockvm::noun::T(&mut slab, &nouns);
        slab.set_root(new_root);
        slab
    }
}

impl<J> NounSlab<J> {
    /// Make a new noun slab with D(0) as the root
    #[tracing::instrument]
    pub fn new() -> Self {
        let slabs = Vec::new();
        let allocation_start: *mut u64 = std::ptr::null_mut();
        let allocation_stop: *mut u64 = std::ptr::null_mut();
        let root: Noun = D(0);
        NounSlab {
            root,
            slabs,
            allocation_start,
            allocation_stop,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Copy the root from another slab into this slab, set this slab's root to the copied root
    pub fn copy_from_slab(&mut self, other: &NounSlab) {
        let space = other.noun_space();
        self.copy_into(other.root, &space);
    }

    /// Copy a noun into this slab and set that noun as the root noun.
    pub fn copy_into(&mut self, copy_root: Noun, space: &NounSpace) -> Noun {
        let mut copied: IntMap<u64, Noun> = IntMap::new();
        // let mut copy_stack = vec![(copy_root, &mut self.root as *mut Noun)];
        let mut copy_stack = vec![(copy_root, std::ptr::addr_of_mut!(self.root))];
        while let Some((noun, dest)) = copy_stack.pop() {
            match noun.as_either_direct_allocated() {
                Either::Left(_direct) => {
                    unsafe { *dest = noun };
                }
                Either::Right(allocated) => match allocated.as_either() {
                    Either::Left(indirect) => {
                        let indirect_ptr =
                            unsafe { indirect.as_atom().in_space(space).raw_pointer() };
                        let indirect_mem_size = indirect.as_atom().in_space(space).raw_size();
                        if let Some(copied_noun) = copied.get(indirect_ptr as u64) {
                            unsafe { *dest = *copied_noun };
                            continue;
                        }
                        let indirect_new_mem = unsafe {
                            self.alloc_indirect(indirect.as_atom().in_space(space).size())
                        };
                        unsafe {
                            copy_nonoverlapping(indirect_ptr, indirect_new_mem, indirect_mem_size)
                        };
                        let copied_noun = unsafe {
                            IndirectAtom::from_raw_pointer(indirect_new_mem)
                                .as_atom()
                                .as_noun()
                        };
                        copied.insert(indirect_ptr as u64, copied_noun);
                        unsafe { *dest = copied_noun };
                    }
                    Either::Right(cell) => {
                        let cell_ptr = unsafe { cell.in_space(space).raw_pointer() };
                        if let Some(copied_noun) = copied.get(cell_ptr as u64) {
                            unsafe { *dest = *copied_noun };
                            continue;
                        }
                        let cell_new_mem = unsafe { self.alloc_cell() };
                        unsafe { copy_nonoverlapping(cell_ptr, cell_new_mem, 1) };
                        let copied_noun = unsafe { Cell::from_raw_pointer(cell_new_mem).as_noun() };
                        copied.insert(cell_ptr as u64, copied_noun);
                        unsafe { *dest = copied_noun };
                        unsafe {
                            // copy_stack
                            //     .push((cell.tail(), &mut (*cell_new_mem).tail as *mut Noun));
                            // copy_stack
                            //     .push((cell.head(), &mut (*cell_new_mem).head as *mut Noun));
                            copy_stack.push((
                                cell.in_space(space).tail().noun(),
                                std::ptr::addr_of_mut!((*cell_new_mem).tail),
                            ));
                            copy_stack.push((
                                cell.in_space(space).head().noun(),
                                std::ptr::addr_of_mut!((*cell_new_mem).head),
                            ));
                        }
                    }
                },
            }
        }
        self.root
    }

    /// Copy the root noun from this slab into the given NockStack.
    ///
    /// Note that this consumes the slab, the slab will be freed after and the root noun returned
    /// referencing the stack. Nouns referencing the slab should not be used past this point.
    #[tracing::instrument(skip(self, stack), level = "trace")]
    pub fn copy_to_stack(self, stack: &mut NockStack) -> Noun {
        let space = stack.noun_space().with_extra_ptr_ranges(self.ptr_ranges());
        let mut res = D(0);
        let mut copy_stack = vec![(self.root, &mut res as *mut Noun)];
        while let Some((noun, dest)) = copy_stack.pop() {
            if let Ok(allocated) = noun.as_allocated() {
                let noun_handle = noun.in_space(&space);
                if let Some(forward) = unsafe { noun_handle.forwarding_pointer() } {
                    unsafe { *dest = forward.noun() };
                } else {
                    match allocated.as_either() {
                        Either::Left(indirect) => {
                            let raw_pointer =
                                unsafe { indirect.as_atom().in_space(&space).raw_pointer() };
                            let raw_size = indirect.as_atom().in_space(&space).raw_size();
                            unsafe {
                                let indirect_mem = stack
                                    .alloc_indirect(indirect.as_atom().in_space(&space).size());
                                std::ptr::copy_nonoverlapping(raw_pointer, indirect_mem, raw_size);
                                indirect
                                    .as_atom()
                                    .in_space(&space)
                                    .set_forwarding_pointer(indirect_mem);
                                *dest = IndirectAtom::from_raw_pointer(indirect_mem)
                                    .as_atom()
                                    .as_noun();
                            }
                        }
                        Either::Right(cell) => {
                            let raw_pointer = unsafe { cell.in_space(&space).raw_pointer() };
                            unsafe {
                                let cell_mem = stack.alloc_cell();
                                copy_nonoverlapping(raw_pointer, cell_mem, 1);
                                copy_stack.push((
                                    cell.in_space(&space).tail().noun(),
                                    &mut (*cell_mem).tail as *mut Noun,
                                ));
                                copy_stack.push((
                                    cell.in_space(&space).head().noun(),
                                    &mut (*cell_mem).head as *mut Noun,
                                ));
                                cell.in_space(&space).set_forwarding_pointer(cell_mem);
                                *dest = Cell::from_raw_pointer(cell_mem).as_noun()
                            }
                        }
                    }
                }
            } else {
                unsafe {
                    *dest = noun;
                } // Direct atom
            }
        }
        res
    }

    fn ptr_ranges(&self) -> Vec<(usize, usize)> {
        let mut ranges = Vec::with_capacity(self.slabs.len());
        for (base, layout) in &self.slabs {
            if base.is_null() || layout.size() == 0 {
                continue;
            }
            let start = *base as usize;
            let end = start + layout.size();
            ranges.push((start, end));
        }
        ranges
    }

    /// Set the root of the noun slab.
    ///
    /// Panics if the given root is not in the noun slab.
    pub fn set_root(&mut self, root: Noun) {
        if let Ok(allocated) = root.as_allocated() {
            match allocated.as_either() {
                Either::Left(indirect) => {
                    let Some(ptr) = indirect.data_pointer_stack() else {
                        panic!("Set root of NounSlab to noun from outside slab");
                    };
                    let u8_ptr = ptr as *const u8;
                    if self.contains_ptr(u8_ptr) {
                        self.root = root;
                        return;
                    }
                    panic!("Set root of NounSlab to noun from outside slab");
                }
                Either::Right(cell) => {
                    let Some(ptr) = cell.stack_memory_pointer() else {
                        panic!("Set root of NounSlab to noun from outside slab");
                    };
                    let u8_ptr = ptr as *const u8;
                    if self.contains_ptr(u8_ptr) {
                        self.root = root;
                        return;
                    }
                    panic!("Set root of NounSlab to noun from outside slab");
                }
            }
        }
        self.root = root;
    }

    /// Get the root noun
    ///
    /// # Safety
    ///
    /// The noun must not be used past the lifetime of the slab.
    pub unsafe fn root(&self) -> &Noun {
        &self.root
    }

    /// Get the root noun
    ///
    /// # Safety
    ///
    /// The noun must not be used past the lifetime of the slab.
    pub unsafe fn root_mut(&mut self) -> &mut Noun {
        &mut self.root
    }
}

impl<J: Jammer> NounSlab<J> {
    pub fn jam(&self) -> Bytes {
        let space = self.noun_space();
        J::jam(unsafe { *self.root() }, &space)
    }

    pub fn cue_into(&mut self, jammed: Bytes) -> Result<Noun, CueError> {
        J::cue(self, jammed)
    }
}

impl<J> Stack for NounSlab<J> {
    unsafe fn alloc_layout(&mut self, layout: Layout) -> *mut u64 {
        let word_size = (layout.size() + 7) >> 3;

        // Ensure we have enough space
        if self.allocation_start.is_null()
            || self.allocation_start.add(word_size) > self.allocation_stop
        {
            let next_idx = std::cmp::max(self.slabs.len(), min_idx_for_size(word_size));
            self.slabs
                .resize(next_idx + 1, (std::ptr::null_mut(), Layout::new::<u8>()));
            let new_size = idx_to_size(next_idx);
            let new_layout = Layout::array::<u64>(new_size).unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            let new_slab = Self::raw_alloc(new_layout);
            let new_slab_u64 = new_slab as *mut u64;
            self.slabs[next_idx] = (new_slab, new_layout);
            self.allocation_start = new_slab_u64;
            self.allocation_stop = new_slab_u64.add(new_size);
        }

        let ptr = self.allocation_start;
        self.allocation_start = self.allocation_start.add(word_size);
        ptr
    }
}

impl<J> Drop for NounSlab<J> {
    fn drop(&mut self) {
        for slab in self.slabs.drain(..) {
            if !slab.0.is_null() {
                unsafe { std::alloc::dealloc(slab.0, slab.1) };
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum CueError {
    #[error("cue: Bad backref")]
    BadBackref,
    #[error("cue: backref too big")]
    BackrefTooBig,
    #[error("cue: truncated buffer")]
    TruncatedBuffer,
}

/// Slab size from vector index, in 8-byte words
fn idx_to_size(idx: usize) -> usize {
    1 << (2 * idx + 9)
}

/// Inverse of idx_to_size
fn min_idx_for_size(sz: usize) -> usize {
    let mut log2sz = sz.ilog2() as usize;
    let round_sz = 1 << log2sz;
    if round_sz != sz {
        log2sz += 1;
    };
    if log2sz <= 9 {
        0
    } else {
        (log2sz - 9).div_ceil(2)
    }
}

pub struct NounMap<V>(IntMap<u64, Vec<(Noun, V)>>);

impl<V> Default for NounMap<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V> NounMap<V> {
    pub fn new() -> Self {
        NounMap(IntMap::new())
    }
    pub fn insert(&mut self, key: Noun, value: V, space: &NounSpace) {
        let key_mug = slab_mug(key, space) as u64;
        if let Some(vec) = self.0.get_mut(key_mug) {
            let mut chain_iter = vec[..].iter_mut();
            if let Some(entry) =
                chain_iter.find(|entry| noun_equality(key.in_space(space), entry.0.in_space(space)))
            {
                entry.1 = value;
            } else {
                vec.push((key, value))
            }
        } else {
            self.0.insert(key_mug, vec![(key, value)]);
        }
    }

    pub fn get(&self, key: Noun, space: &NounSpace) -> Option<&V> {
        let key_mug = slab_mug(key, space) as u64;
        if let Some(vec) = self.0.get(key_mug) {
            let mut chain_iter = vec[..].iter();
            if let Some(entry) = chain_iter
                .find(|entry| noun_equality((key as Noun).in_space(space), entry.0.in_space(space)))
            {
                Some(&entry.1)
            } else {
                None
            }
        } else {
            None
        }
    }
}

pub fn slab_equality<J, K>(a: &NounSlab<J>, b: &NounSlab<K>) -> bool {
    let mut ranges = a.ptr_ranges();
    ranges.extend(b.ptr_ranges());
    let space = NounSpace::empty().with_extra_ptr_ranges(ranges);
    noun_equality(a.root.in_space(&space), b.root.in_space(&space))
}

fn slab_mug(a: Noun, space: &NounSpace) -> u32 {
    let mut stack = vec![a];
    while let Some(noun) = stack.pop() {
        if let Ok(mut allocated) = noun.as_allocated() {
            if get_mug(noun, space).is_none() {
                match allocated.as_either() {
                    Either::Left(indirect) => unsafe {
                        set_mug(
                            &mut allocated,
                            calc_atom_mug_u32(indirect.as_atom(), space),
                            space,
                        );
                    },
                    Either::Right(cell) => match (
                        get_mug(cell.in_space(space).head().noun(), space),
                        get_mug(cell.in_space(space).tail().noun(), space),
                    ) {
                        (Some(head_mug), Some(tail_mug)) => unsafe {
                            set_mug(
                                &mut allocated,
                                calc_cell_mug_u32(head_mug, tail_mug, space),
                                space,
                            );
                        },
                        _ => {
                            stack.push(noun);
                            stack.push(cell.in_space(space).tail().noun());
                            stack.push(cell.in_space(space).head().noun());
                        }
                    },
                }
            }
        }
    }
    get_mug(a, space).expect("Noun should have a mug once mugged.")
}

enum CueStackEntry {
    DestinationPointer(*mut Noun),
    BackRef(u64, *const Noun),
}

// gonna use this like an ML module
/// This makes us modular over different implementations of jam and cue
pub trait Jammer: Sized {
    fn jam(noun: Noun, space: &NounSpace) -> Bytes;
    fn cue(slab: &mut NounSlab<Self>, bytes: Bytes) -> Result<Noun, CueError>;
}

pub struct NockJammer;

impl Jammer for NockJammer {
    fn jam(noun: Noun, space: &NounSpace) -> Bytes {
        fn mat_backref(buffer: &mut BitVec<u8, Lsb0>, backref: usize) {
            if backref == 0 {
                buffer.extend_from_bitslice(bits![u8, Lsb0; 1, 1, 1]);
                return;
            }
            let backref_sz = met0_u64_to_usize(backref as u64);
            let backref_sz_sz = met0_u64_to_usize(backref_sz as u64);
            buffer.extend_from_bitslice(bits![u8, Lsb0; 1, 1]); // backref tag
            let buffer_len = buffer.len();
            buffer.resize(buffer_len + backref_sz_sz, false);
            buffer.push(true);
            buffer.extend_from_bitslice(
                &BitSlice::<_, Lsb0>::from_element(&backref_sz)[0..backref_sz_sz - 1],
            );
            buffer
                .extend_from_bitslice(&BitSlice::<_, Lsb0>::from_element(&backref)[0..backref_sz]);
        }

        fn mat_atom(buffer: &mut BitVec<u8, Lsb0>, atom: Atom, space: &NounSpace) {
            if unsafe { atom.as_noun().raw_equals(&D(0)) } {
                buffer.extend_from_bitslice(bits![u8, Lsb0; 0, 1]);
                return;
            }
            let atom_sz = met0_usize(atom, space);
            let atom_sz_sz = met0_u64_to_usize(atom_sz as u64);
            buffer.push(false); // atom tag
            let buffer_len = buffer.len();
            buffer.resize(buffer_len + atom_sz_sz, false);
            buffer.push(true);
            buffer.extend_from_bitslice(
                &BitSlice::<_, Lsb0>::from_element(&atom_sz)[0..atom_sz_sz - 1],
            );
            buffer.extend_from_bitslice(&atom.in_space(space).as_bitslice()[0..atom_sz]);
        }
        let mut backref_map = NounMap::<usize>::new();
        let mut stack = vec![noun];
        let mut buffer = bitvec![u8, Lsb0; 0; 0];
        while let Some(noun) = stack.pop() {
            if let Some(backref) = backref_map.get(noun, space) {
                if let Ok(atom) = noun.as_atom() {
                    if met0_u64_to_usize(*backref as u64) < met0_usize(atom, space) {
                        mat_backref(&mut buffer, *backref);
                    } else {
                        mat_atom(&mut buffer, atom, space)
                    }
                } else {
                    mat_backref(&mut buffer, *backref);
                }
            } else {
                backref_map.insert(noun, buffer.len(), space);
                match noun.as_either_atom_cell() {
                    Either::Left(atom) => {
                        mat_atom(&mut buffer, atom, space);
                    }
                    Either::Right(cell) => {
                        buffer.extend_from_bitslice(bits![u8, Lsb0; 1, 0]); // cell tag
                        let cell = cell.in_space(space);
                        stack.push(cell.tail().noun());
                        stack.push(cell.head().noun());
                    }
                }
            }
        }

        Bytes::copy_from_slice(buffer.as_raw_slice())
    }

    fn cue(slab: &mut NounSlab, jammed: Bytes) -> Result<Noun, CueError> {
        fn rub_backref(cursor: &mut usize, buffer: &BitSlice<u8, Lsb0>) -> Result<usize, CueError> {
            if let Some(idx) = buffer[*cursor..].first_one() {
                if idx == 0 {
                    *cursor += 1;
                    Ok(0)
                } else {
                    *cursor += idx + 1;
                    let mut sz = 0usize;
                    let sz_slice = BitSlice::<_, Lsb0>::from_element_mut(&mut sz);
                    if buffer.len() < *cursor + idx - 1 {
                        Err(CueError::TruncatedBuffer)?;
                    };
                    sz_slice[0..idx - 1].clone_from_bitslice(&buffer[*cursor..*cursor + idx - 1]);
                    sz_slice.set(idx - 1, true);
                    *cursor += idx - 1;
                    if sz > size_of::<usize>() << 3 {
                        Err(CueError::BackrefTooBig)?;
                    }
                    if buffer.len() < *cursor + sz {
                        Err(CueError::TruncatedBuffer)?;
                    }
                    let mut backref = 0usize;
                    let backref_slice = BitSlice::<_, Lsb0>::from_element_mut(&mut backref);
                    backref_slice[0..sz].clone_from_bitslice(&buffer[*cursor..*cursor + sz]);
                    *cursor += sz;
                    Ok(backref)
                }
            } else {
                Err(CueError::TruncatedBuffer)
            }
        }

        fn rub_atom(
            slab: &mut NounSlab,
            cursor: &mut usize,
            buffer: &BitSlice<u8, Lsb0>,
        ) -> Result<Atom, CueError> {
            if let Some(idx) = buffer[*cursor..].first_one() {
                if idx == 0 {
                    *cursor += 1;
                    unsafe { Ok(DirectAtom::new_unchecked(0).as_atom()) }
                } else {
                    *cursor += idx + 1;
                    let mut sz = 0usize;
                    let sz_slice = BitSlice::<_, Lsb0>::from_element_mut(&mut sz);
                    if buffer.len() < *cursor + idx - 1 {
                        Err(CueError::TruncatedBuffer)?;
                    }
                    sz_slice[0..idx - 1].clone_from_bitslice(&buffer[*cursor..*cursor + idx - 1]);
                    sz_slice.set(idx - 1, true);
                    *cursor += idx - 1;
                    if buffer.len() < *cursor + sz {
                        Err(CueError::TruncatedBuffer)?;
                    }
                    if sz < 64 {
                        // Direct atom: less than 64 bits
                        let mut data = 0u64;
                        let atom_slice = BitSlice::<_, Lsb0>::from_element_mut(&mut data);
                        atom_slice[0..sz].clone_from_bitslice(&buffer[*cursor..*cursor + sz]);
                        *cursor += sz;
                        Ok(unsafe { DirectAtom::new_unchecked(data).as_atom() })
                    } else {
                        // Indirect atom
                        let indirect_words = (sz + 63) >> 6; // fast round to 64-bit words
                        let (indirect, data_ptr) =
                            unsafe { IndirectAtom::new_raw_mut_zeroed(slab, indirect_words) };
                        let slice = unsafe {
                            BitSlice::<u64, Lsb0>::from_slice_mut(std::slice::from_raw_parts_mut(
                                data_ptr, indirect_words,
                            ))
                        };
                        slice[0..sz].clone_from_bitslice(&buffer[*cursor..*cursor + sz]);
                        *cursor += sz;
                        let words =
                            unsafe { std::slice::from_raw_parts_mut(data_ptr, indirect_words) };
                        let mut used_words = words.len();
                        while used_words > 1 && words[used_words - 1] == 0 {
                            used_words -= 1;
                        }
                        if used_words == 0 {
                            return Ok(unsafe { DirectAtom::new_unchecked(0).as_atom() });
                        }
                        if used_words == 1 {
                            let value = words[0];
                            if value <= DIRECT_MAX {
                                return Ok(unsafe { DirectAtom::new_unchecked(value).as_atom() });
                            }
                        }
                        unsafe {
                            let meta_ptr = words.as_mut_ptr().sub(2);
                            *meta_ptr.add(1) = used_words as u64;
                        }
                        Ok(indirect.as_atom())
                    }
                }
            } else {
                Err(CueError::TruncatedBuffer)
            }
        }

        let mut backref_map = IntMap::new();
        let bitslice = jammed.view_bits::<Lsb0>();
        let mut cursor = 0usize;
        let mut res = D(0);
        let mut stack = vec![CueStackEntry::DestinationPointer(&mut res)];
        let mut noun_counter = 0;
        loop {
            match stack.pop() {
                Some(CueStackEntry::DestinationPointer(dest)) => {
                    let backref = cursor as u64;
                    if cursor >= bitslice.len() {
                        return Err(CueError::TruncatedBuffer);
                    }
                    if bitslice[cursor] {
                        // 1
                        cursor += 1;
                        if cursor >= bitslice.len() {
                            return Err(CueError::TruncatedBuffer);
                        }
                        if bitslice[cursor] {
                            // 1 - backref
                            cursor += 1;
                            let backref = rub_backref(&mut cursor, bitslice)?;
                            if let Some(noun) = backref_map.get(backref as u64) {
                                unsafe {
                                    *dest = *noun;
                                }
                            } else {
                                Err(CueError::BadBackref)?
                            }
                        } else {
                            // 0 - cell
                            cursor += 1;
                            let (cell, cell_mem) = unsafe { Cell::new_raw_mut(slab) };
                            unsafe {
                                *dest = cell.as_noun();
                            }
                            unsafe {
                                stack.push(CueStackEntry::BackRef(backref, dest as *const Noun));
                                stack
                                    .push(CueStackEntry::DestinationPointer(&mut (*cell_mem).tail));
                                stack
                                    .push(CueStackEntry::DestinationPointer(&mut (*cell_mem).head));
                            }
                        }
                    } else {
                        // 0 - atom
                        cursor += 1;
                        unsafe { *dest = rub_atom(slab, &mut cursor, bitslice)?.as_noun() };
                        backref_map.insert(backref, unsafe { *dest });
                    }
                }
                Some(CueStackEntry::BackRef(backref, noun_ptr)) => {
                    backref_map.insert(backref, unsafe { *noun_ptr });
                }
                None => {
                    break;
                }
            }
            noun_counter += 1;
        }
        tracing::trace!("cue_into: noun_counter {}", noun_counter);
        slab.set_root(res);
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    use bitvec::prelude::*;
    use ibig::ubig;
    use nockvm::mem::{NockStack, NOCK_STACK_SIZE_TINY};
    use nockvm::noun::{AllocLocation, NounRepr, D, T};
    use nockvm::pma::{Pma, PmaCopy};
    use nockvm_macros::tas;
    use tempfile::TempDir;

    use super::*;
    use crate::AtomExt;

    fn assert_set_root_rejects(root: Noun) {
        let mut slab: NounSlab = NounSlab::new();
        let res = catch_unwind(AssertUnwindSafe(|| slab.set_root(root)));
        assert!(
            res.is_err(),
            "set_root should reject roots outside the slab"
        );
    }
    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_ubig_alloc() {
        let mut slab: NounSlab = NounSlab::new();
        let big_exp = ubig!(0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF);
        let atom = Atom::from_ubig(&mut slab, &big_exp);
        let space = slab.noun_space();
        let big = atom.in_space(&space).as_ubig(&mut slab);
        assert_eq!(big, big_exp);
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_jam() {
        let mut slab: NounSlab = NounSlab::new();
        let slab_noun = T(
            &mut slab,
            &[D(tas!(b"request")), D(tas!(b"block")), D(tas!(b"by-id")), D(0)],
        );
        slab.set_root(slab_noun);
        let jammed: Vec<u8> = slab.jam().to_vec();
        println!("jammed: {:?}", jammed);

        let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let stack_noun = slab.copy_to_stack(&mut stack);
        let space = stack.noun_space();
        let mut nockvm_jammed: Vec<u8> = nockvm::serialization::jam(&mut stack, stack_noun)
            .in_space(&space)
            .as_ne_bytes()
            .to_vec();
        let nockvm_suffix: Vec<u8> = nockvm_jammed.split_off(jammed.len());
        println!("nockvm_jammed: {:?}", nockvm_jammed);

        assert_eq!(jammed, nockvm_jammed, "Jammed results should be identical");
        assert!(
            nockvm_suffix.iter().all(|b| { *b == 0 }),
            "Extra bytes in nockvm jam should all be 0"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_jam_cue_roundtrip() {
        let mut original_slab: NounSlab = NounSlab::new();
        let original_noun = T(&mut original_slab, &[D(5), D(23)]);
        println!("original_noun: {:?}", original_noun);
        original_slab.set_root(original_noun);

        // Jam the original noun
        let jammed: Vec<u8> = original_slab.jam().to_vec();

        // Cue the jammed data into a new slab
        let mut cued_slab: NounSlab = NounSlab::new();
        let cued_noun = cued_slab
            .cue_into(jammed.into())
            .expect("Cue should succeed");

        println!("cued_noun: {:?}", cued_noun);

        // Compare the original and cued nouns
        let mut ranges = original_slab.ptr_ranges();
        ranges.extend(cued_slab.ptr_ranges());
        let space = NounSpace::empty().with_extra_ptr_ranges(ranges);
        assert!(
            noun_equality(
                unsafe { original_slab.root() }.in_space(&space),
                cued_noun.in_space(&space),
            ),
            "Original and cued nouns should be equal"
        );
    }

    #[test]
    fn test_cue_into_rejects_truncated_atom_without_panicking() {
        let mut slab: NounSlab = NounSlab::new();

        let result = catch_unwind(AssertUnwindSafe(|| {
            slab.cue_into(Bytes::copy_from_slice(&[0; 13]))
        }));

        assert!(result.is_ok(), "truncated jam input must not panic");
        assert!(matches!(
            result.expect("catch_unwind result"),
            Err(CueError::TruncatedBuffer)
        ));
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_complex_noun() {
        let mut slab: NounSlab = NounSlab::new();
        let complex_noun = T(
            &mut slab,
            &[D(tas!(b"request")), D(tas!(b"block")), D(tas!(b"by-id")), D(0)],
        );
        slab.set_root(complex_noun);

        let jammed = slab.jam();
        let mut cued_slab: NounSlab = NounSlab::new();
        let cued_noun = cued_slab.cue_into(jammed).expect("Cue should succeed");

        let mut ranges = slab.ptr_ranges();
        ranges.extend(cued_slab.ptr_ranges());
        let space = NounSpace::empty().with_extra_ptr_ranges(ranges);
        assert!(
            noun_equality(
                unsafe { slab.root() }.in_space(&space),
                cued_noun.in_space(&space),
            ),
            "Complex nouns should be equal after jam/cue roundtrip"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_indirect_atoms() {
        let mut slab: NounSlab = NounSlab::new();
        let large_number = u64::MAX as u128 + 1;
        let large_number_bytes = Bytes::from(large_number.to_le_bytes().to_vec());
        let indirect_atom = Atom::from_bytes(&mut slab, &large_number_bytes);
        let noun_with_indirect = T(&mut slab, &[D(1), indirect_atom.as_noun(), D(2)]);
        println!("noun_with_indirect: {:?}", noun_with_indirect);
        slab.set_root(noun_with_indirect);

        let jammed = slab.jam();
        let mut cued_slab: NounSlab = NounSlab::new();
        let cued_noun = cued_slab.cue_into(jammed).expect("Cue should succeed");
        println!("cued_noun: {:?}", cued_noun);

        let mut ranges = slab.ptr_ranges();
        ranges.extend(cued_slab.ptr_ranges());
        let space = NounSpace::empty().with_extra_ptr_ranges(ranges);
        assert!(
            noun_equality(
                noun_with_indirect.in_space(&space),
                cued_noun.in_space(&space),
            ),
            "Nouns with indirect atoms should be equal after jam/cue roundtrip"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_tas_macro() {
        let mut slab: NounSlab = NounSlab::new();
        let tas_noun = T(
            &mut slab,
            &[D(tas!(b"foo")), D(tas!(b"bar")), D(tas!(b"baz"))],
        );
        slab.set_root(tas_noun);

        let jammed = slab.jam();
        let mut cued_slab: NounSlab = NounSlab::new();
        let cued_noun = cued_slab.cue_into(jammed).expect("Cue should succeed");

        let mut ranges = slab.ptr_ranges();
        ranges.extend(cued_slab.ptr_ranges());
        let space = NounSpace::empty().with_extra_ptr_ranges(ranges);
        assert!(
            noun_equality(
                unsafe { slab.root() }.in_space(&space),
                cued_noun.in_space(&space),
            ),
            "Nouns with tas! macros should be equal after jam/cue roundtrip"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_cue_from_file() {
        use std::fs::File;
        use std::io::Read;
        use std::path::Path;

        use bytes::Bytes;

        // Path to the test file
        // For Bazel builds, we use the test-jams directory from the environment
        #[cfg(feature = "bazel_build")]
        let file_path = match std::env::var("TEST_JAMS_DIR") {
            Ok(dir) => format!("{}/cue-test.jam", dir),
            Err(_) => String::from("test-jams/cue-test.jam"),
        };

        // For Cargo builds, we use the regular path
        #[cfg(not(feature = "bazel_build"))]
        let file_path = "test-jams/cue-test.jam";

        // Check if the file exists
        if !Path::new(&file_path).exists() {
            println!("Test file not found at {}, skipping test", file_path);
            return; // Skip the test if the file doesn't exist
        }

        // Read the jammed data from the file
        let mut file = File::open(file_path).expect("Failed to open file");
        let mut jammed_data = Vec::new();
        file.read_to_end(&mut jammed_data)
            .expect("Failed to read file");
        let jammed = Bytes::from(jammed_data);

        // Create a new NounSlab and attempt to cue the data
        let mut slab: NounSlab = NounSlab::new();
        let result = slab.cue_into(jammed);

        // Assert that cue_into does not return an error
        assert!(
            result.is_ok(),
            "cue_into returned an error: {:?}",
            result.err()
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_cyclic_structure() {
        let mut slab: NounSlab = NounSlab::new();

        // Create a jammed representation of a cyclic structure
        // [0 *] where * refers back to the entire cell, i.e. 0b11110001
        let mut jammed = BitVec::<u8, Lsb0>::new();
        jammed.extend_from_bitslice(bits![u8, Lsb0; 1, 1, 1]); //Backref to the entire structure
        jammed.extend_from_bitslice(bits![u8, Lsb0; 1, 0 ,0]); // Atom 0
        jammed.extend_from_bitslice(bits![u8, Lsb0; 0, 1]); // Cell

        let jammed_bytes = Bytes::from(jammed.into_vec());

        let result = slab.cue_into(jammed_bytes);
        assert!(
            result.is_err(),
            "Expected error due to cyclic structure, but cue_into completed successfully"
        );
        if let Err(e) = result {
            println!("Error type: {:?}", e);
            assert!(
                matches!(e, CueError::BadBackref),
                "Expected CueError::BadBackref, but got a different error"
            );
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_cue_simple_cell() {
        let mut slab: NounSlab = NounSlab::new();

        // Create a jammed representation of [1 0] by hand
        let mut jammed = BitVec::<u8, Lsb0>::new();
        jammed.extend_from_bitslice(bits![u8, Lsb0; 1, 0, 0, 0, 1, 1, 0, 1]); // 0b10110001

        let jammed_bytes = Bytes::from(jammed.into_vec());

        let result = slab.cue_into(jammed_bytes);
        assert!(result.is_ok(), "cue_into should succeed");
        if let Ok(cued_noun) = result {
            let expected_noun = T(&mut slab, &[D(1), D(0)]);
            let space = slab.noun_space();
            assert!(
                noun_equality(cued_noun.in_space(&space), expected_noun.in_space(&space),),
                "Cued noun should equal [1 0]"
            );
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_cell_construction_for_noun_slab() {
        let mut slab: NounSlab = NounSlab::new();
        let (cell, cell_mem_ptr) = unsafe { Cell::new_raw_mut(&mut slab) };
        let space = slab.noun_space();
        let cell_mem_ptr = cell_mem_ptr as *const CellMemory;
        let cell_ptr = cell
            .in_space(&space)
            .cell()
            .stack_memory_pointer()
            .expect("cell not in stack");
        assert!(cell_mem_ptr == cell_ptr);
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_noun_slab_copy_into() {
        let mut slab: NounSlab = NounSlab::new();
        let test_noun = T(&mut slab, &[D(5), D(23)]);
        slab.set_root(test_noun);
        let mut copy_slab: NounSlab = NounSlab::new();
        let space = slab.noun_space();
        copy_slab.copy_into(test_noun, &space);
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_modify_with_imports3_uses_imported_nouns_and_preserves_root() {
        let mut local_slab: NounSlab = NounSlab::new();
        let local_root = T(&mut local_slab, &[D(42), D(43)]);
        local_slab.set_root(local_root);

        let mut foreign_slab: NounSlab = NounSlab::new();
        let foreign_a = T(&mut foreign_slab, &[D(1), D(2)]);
        let foreign_b = T(&mut foreign_slab, &[D(3), D(4)]);
        let foreign_c = T(&mut foreign_slab, &[D(5), D(6)]);
        foreign_slab.set_root(foreign_c);
        let foreign_space = foreign_slab.noun_space();

        local_slab.modify_with_imports3(
            |(import_a, import_b, import_c), root| vec![root, import_a, import_b, import_c, D(0)],
            (foreign_a, foreign_b, foreign_c),
            &foreign_space,
        );

        let local_space = local_slab.noun_space();
        let elems: Vec<Noun> = unsafe { *local_slab.root() }
            .in_space(&local_space)
            .list_iter()
            .map(|handle| handle.noun())
            .collect();

        assert_eq!(
            elems.len(),
            4,
            "modified slab should contain root plus 3 imports"
        );
        assert!(
            unsafe { elems[0].raw_equals(&local_root) },
            "closure should receive the original slab root"
        );
        let imported_a_slab: NounSlab = NounSlab::from_noun(elems[1], &local_space);
        let imported_b_slab: NounSlab = NounSlab::from_noun(elems[2], &local_space);
        let imported_c_slab: NounSlab = NounSlab::from_noun(elems[3], &local_space);
        let expected_a_slab: NounSlab = NounSlab::from_noun(foreign_a, &foreign_space);
        let expected_b_slab: NounSlab = NounSlab::from_noun(foreign_b, &foreign_space);
        let expected_c_slab: NounSlab = NounSlab::from_noun(foreign_c, &foreign_space);
        assert!(
            slab_equality(&imported_a_slab, &expected_a_slab),
            "first imported noun should match structurally"
        );
        assert!(
            slab_equality(&imported_b_slab, &expected_b_slab),
            "second imported noun should match structurally"
        );
        assert!(
            slab_equality(&imported_c_slab, &expected_c_slab),
            "third imported noun should match structurally"
        );
        assert!(
            !unsafe { elems[1].raw_equals(&foreign_a) },
            "first imported noun should be copied into the destination slab"
        );
        assert!(
            !unsafe { elems[2].raw_equals(&foreign_b) },
            "second imported noun should be copied into the destination slab"
        );
        assert!(
            !unsafe { elems[3].raw_equals(&foreign_c) },
            "third imported noun should be copied into the destination slab"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_modify_rehomes_foreign_children_into_destination_slab() {
        let mut local_slab: NounSlab = NounSlab::new();
        let local_root = T(&mut local_slab, &[D(42), D(43)]);
        local_slab.set_root(local_root);

        let mut foreign_slab: NounSlab = NounSlab::new();
        let foreign_root = T(&mut foreign_slab, &[D(1), D(2)]);
        foreign_slab.set_root(foreign_root);
        let foreign_space = foreign_slab.noun_space();

        local_slab.modify(|root| vec![root, foreign_root, D(0)]);

        let local_space = local_slab.noun_space();
        let elems: Vec<Noun> = unsafe { *local_slab.root() }
            .in_space(&local_space)
            .list_iter()
            .map(|handle| handle.noun())
            .collect();

        assert_eq!(
            elems.len(),
            2,
            "modified slab should contain 2 list elements"
        );
        assert!(
            unsafe { elems[0].raw_equals(&local_root) },
            "closure should receive the original slab root"
        );
        let imported_slab: NounSlab = NounSlab::from_noun(elems[1], &local_space);
        let expected_slab: NounSlab = NounSlab::from_noun(foreign_root, &foreign_space);
        assert!(
            slab_equality(&imported_slab, &expected_slab),
            "foreign child should be copied structurally into the destination slab"
        );
        assert!(
            !unsafe { elems[1].raw_equals(&foreign_root) },
            "foreign child should not retain the foreign slab pointer"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_modify_noun_rehomes_mixed_local_root() {
        let mut local_slab: NounSlab = NounSlab::new();
        let local_root = T(&mut local_slab, &[D(42), D(43)]);
        local_slab.set_root(local_root);

        let mut foreign_slab: NounSlab = NounSlab::new();
        let foreign_root = T(&mut foreign_slab, &[D(1), D(2)]);
        foreign_slab.set_root(foreign_root);
        let foreign_space = foreign_slab.noun_space();

        let mixed_root = T(&mut local_slab, &[local_root, foreign_root, D(0)]);
        local_slab.modify_noun(|_| mixed_root);

        let local_space = local_slab.noun_space();
        let elems: Vec<Noun> = unsafe { *local_slab.root() }
            .in_space(&local_space)
            .list_iter()
            .map(|handle| handle.noun())
            .collect();

        assert_eq!(
            elems.len(),
            2,
            "modified slab should contain 2 list elements"
        );
        assert!(
            unsafe { elems[0].raw_equals(&local_root) },
            "local root should be preserved"
        );
        let imported_slab: NounSlab = NounSlab::from_noun(elems[1], &local_space);
        let expected_slab: NounSlab = NounSlab::from_noun(foreign_root, &foreign_space);
        assert!(
            slab_equality(&imported_slab, &expected_slab),
            "modify_noun should re-home foreign descendants before setting the root"
        );
        assert!(
            !unsafe { elems[1].raw_equals(&foreign_root) },
            "foreign child should not retain the foreign slab pointer"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_from_array_rehomes_foreign_children_into_destination_slab() {
        let mut foreign_slab: NounSlab = NounSlab::new();
        let foreign_root = T(&mut foreign_slab, &[D(1), D(2)]);
        foreign_slab.set_root(foreign_root);
        let foreign_space = foreign_slab.noun_space();

        let slab: NounSlab = [D(7), foreign_root, D(0)].into();
        let local_space = slab.noun_space();
        let elems: Vec<Noun> = unsafe { *slab.root() }
            .in_space(&local_space)
            .list_iter()
            .map(|handle| handle.noun())
            .collect();

        assert_eq!(elems.len(), 2, "slab root should be a 2-element list");
        assert!(
            unsafe { elems[0].raw_equals(&D(7)) },
            "first list element should remain direct"
        );
        let imported_slab: NounSlab = NounSlab::from_noun(elems[1], &local_space);
        let expected_slab: NounSlab = NounSlab::from_noun(foreign_root, &foreign_space);
        assert!(
            slab_equality(&imported_slab, &expected_slab),
            "foreign child should be copied structurally into the destination slab"
        );
        assert!(
            !unsafe { elems[1].raw_equals(&foreign_root) },
            "foreign child should not retain the foreign slab pointer"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_set_root_rejects_roots_outside_slab() {
        let mut local_slab: NounSlab = NounSlab::new();
        let local_root = T(&mut local_slab, &[D(1), D(2)]);
        local_slab.set_root(local_root);
        assert!(unsafe { local_slab.root().raw_equals(&local_root) });

        let mut foreign_slab: NounSlab = NounSlab::new();
        let foreign_root = T(&mut foreign_slab, &[D(3), D(4)]);
        assert_set_root_rejects(foreign_root);

        let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let stack_root = Cell::new(&mut stack, D(5), D(6)).as_noun();
        assert_set_root_rejects(stack_root);

        let tempdir = TempDir::new().expect("create temp dir for PMA");
        let pma_path = tempdir.path().join("set-root-test.pma");
        let mut pma = Pma::new(1 << 10, pma_path).expect("create test PMA");
        let mut pma_root = Cell::new(&mut stack, D(7), D(8)).as_noun();
        unsafe {
            pma_root.copy_to_pma(&stack, &mut pma);
        }
        let pma_space = NounSpace::pma_only(&pma);
        assert!(matches!(
            pma_root.in_space(&pma_space).repr(),
            NounRepr::Cell(AllocLocation::PmaOffset)
        ));
        assert_set_root_rejects(pma_root);
    }

    // Fails in Miri
    // #[test]
    // fn test_alloc_cell_for_noun_slab_uninit() {
    //     let mut slab = NounSlab::new();
    //     let cell_ptr = unsafe { slab.alloc_cell() };
    //     let cell: Cell = unsafe { Cell::from_raw_pointer(cell_ptr) };
    //     unsafe { assert_eq!(cell.head().as_raw(), 0) };
    // }

    #[test]
    fn test_alloc_cell_for_noun_slab_set_value() {
        let mut slab: NounSlab = NounSlab::new();
        let mut i = 0;
        while i < 100 {
            let cell_ptr = unsafe { slab.alloc_cell() };
            let cell_memory = CellMemory {
                metadata: 0,
                head: D(i),
                tail: D(i + 1),
            };
            unsafe { (*cell_ptr) = cell_memory };
            i += 1;
            println!("allocation_start: {:?}", slab.allocation_start);
        }
        // let cell_ptr = unsafe { slab.alloc_cell() };
        // // Set the cell_ptr to a value
        // let cell_memory = CellMemory { metadata: 0, head: D(5), tail: D(23) };
        // unsafe { (*cell_ptr) = cell_memory };
        // let cell: Cell = unsafe { Cell::from_raw_pointer(cell_ptr) };
        // unsafe { assert_eq!(cell.head().as_raw(), 5) };
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_nounslab_modify() {
        let mut slab: NounSlab = NounSlab::new();
        slab.modify(|root| vec![D(0), D(tas!(b"bind")), root]);
        let mut test_slab: NounSlab = NounSlab::new();
        let test_noun = T(&mut test_slab, &[D(0), D(tas!(b"bind")), D(0)]);
        let mut ranges = slab.ptr_ranges();
        ranges.extend(test_slab.ptr_ranges());
        let space = NounSpace::empty().with_extra_ptr_ranges(ranges);
        noun_equality(slab.root.in_space(&space), test_noun.in_space(&space));
        // let peek_res = unsafe { bind_slab.root_owned() };
        // let bind_noun = T(&mut bind_slab, &[D(pid), D(tas!(b"bind")), peek_res]);
    }
    // // This test _should_ fail under Miri
    // #[test]
    // #[should_panic(expected = "error: Undefined Behavior: using uninitialized data, but this operation requires initialized memory")]
    // fn test_raw_alloc() {
    //     let layout = Layout::array::<u64>(512).unwrap_or_else(|| panic!("Panicked at {}:{} (git sha: {:?})", file!(), line!(), option_env!("GIT_SHA")));
    //     let slab = unsafe { NounSlab::raw_alloc(layout) };
    //     assert!(!slab.is_null());
    //     // cast doesn't hide it from Miri
    //     let new_slab_u64 = slab as *mut u64;
    //     let _huh = unsafe { *new_slab_u64 };
    //     unsafe { std::alloc::dealloc(slab, layout) };
    // }
}
