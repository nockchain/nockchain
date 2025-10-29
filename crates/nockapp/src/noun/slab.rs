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
use nockvm::mem::NockStack;
use nockvm::mug::{calc_atom_mug_u32, calc_cell_mug_u32, get_mug, set_mug};
use nockvm::noun::{Atom, Cell, CellMemory, DirectAtom, IndirectAtom, Noun, NounAllocator, D};
use nockvm::serialization::{met0_u64_to_usize, met0_usize};
use thiserror::Error;

use crate::noun::NounExt;

const CELL_MEM_WORD_SIZE: usize = (size_of::<CellMemory>() + 7) >> 3;

/// A (mostly*) self-contained arena for allocating nouns.
///
/// *Nouns may contain references to the PMA, but not other allocation arenas.
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
        self.root
            .list_iter()
            .map(|n| {
                let mut slab = Self::new();
                slab.copy_into(n);
                slab
            })
            .collect()
    }

    pub fn modify<F: FnOnce(Noun) -> Vec<Noun>>(&mut self, f: F) {
        let new_root_base = f(self.root);
        let new_root = nockvm::noun::T(self, &new_root_base);
        self.set_root(new_root);
    }

    pub fn modify_noun<F: FnOnce(Noun) -> Noun>(&mut self, f: F) {
        let new_root = f(self.root);
        self.set_root(new_root);
    }

    pub fn modify_with_imports3<F: FnOnce((Noun, Noun, Noun), Noun) -> Vec<Noun>>(
        &mut self,
        f: F,
        imports: (Noun, Noun, Noun),
    ) {
        self.copy_into(imports.0);
        self.copy_into(imports.1);
        self.copy_into(imports.2);
        let new_root_base = f(imports, self.root);
        let new_root = nockvm::noun::T(self, &new_root_base);
        self.set_root(new_root);
    }
}

impl<J> Clone for NounSlab<J> {
    fn clone(&self) -> Self {
        let mut slab = Self::new();
        slab.copy_into(self.root);
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
        slab_noun_equality(a, b)
    }
}

/// # Safety: no noun in this slab references a noun outside the slab, except in the PMA
unsafe impl Send for NounSlab {}

impl<J> Default for NounSlab<J> {
    fn default() -> Self {
        Self::new()
    }
}

impl<J> From<Noun> for NounSlab<J> {
    fn from(noun: Noun) -> Self {
        let mut slab = Self::new();
        slab.copy_into(noun);
        slab
    }
}

impl<const N: usize, J> From<[Noun; N]> for NounSlab<J> {
    fn from(nouns: [Noun; N]) -> Self {
        let mut slab = Self::new();
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
        self.copy_into(other.root);
    }

    /// Copy a noun into this slab, only leaving references into the PMA. Set that noun as the root
    /// noun.
    pub fn copy_into(&mut self, copy_root: Noun) -> Noun {
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
                        let indirect_ptr = unsafe { indirect.to_raw_pointer() };
                        let indirect_mem_size = indirect.raw_size();
                        if let Some(copied_noun) = copied.get(indirect_ptr as u64) {
                            unsafe { *dest = *copied_noun };
                            continue;
                        }
                        let indirect_new_mem = unsafe { self.alloc_indirect(indirect.size()) };
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
                        let cell_ptr = unsafe { cell.to_raw_pointer() };
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
                            copy_stack
                                .push((cell.tail(), std::ptr::addr_of_mut!((*cell_new_mem).tail)));
                            copy_stack
                                .push((cell.head(), std::ptr::addr_of_mut!((*cell_new_mem).head)));
                        }
                    }
                },
            }
        }
        self.root
    }

    /// Copy the root noun from this slab into the given NockStack, only leaving references into the PMA
    ///
    /// Note that this consumes the slab, the slab will be freed after and the root noun returned
    /// referencing the stack. Nouns referencing the slab should not be used past this point.
    #[tracing::instrument(skip(self, stack), level = "trace")]
    pub fn copy_to_stack(self, stack: &mut NockStack) -> Noun {
        let mut res = D(0);
        let mut copy_stack = vec![(self.root, &mut res as *mut Noun)];
        while let Some((noun, dest)) = copy_stack.pop() {
            if let Ok(allocated) = noun.as_allocated() {
                if let Some(forward) = unsafe { allocated.forwarding_pointer() } {
                    unsafe { *dest = forward.as_noun() };
                } else {
                    match allocated.as_either() {
                        Either::Left(mut indirect) => {
                            let raw_pointer = unsafe { indirect.to_raw_pointer() };
                            let raw_size = indirect.raw_size();
                            unsafe {
                                let indirect_mem = stack.alloc_indirect(indirect.size());
                                std::ptr::copy_nonoverlapping(raw_pointer, indirect_mem, raw_size);
                                indirect.set_forwarding_pointer(indirect_mem);
                                *dest = IndirectAtom::from_raw_pointer(indirect_mem)
                                    .as_atom()
                                    .as_noun();
                            }
                        }
                        Either::Right(mut cell) => {
                            let raw_pointer = unsafe { cell.to_raw_pointer() };
                            unsafe {
                                let cell_mem = stack.alloc_cell();
                                copy_nonoverlapping(raw_pointer, cell_mem, 1);
                                copy_stack.push((cell.tail(), &mut (*cell_mem).tail as *mut Noun));
                                copy_stack.push((cell.head(), &mut (*cell_mem).head as *mut Noun));
                                cell.set_forwarding_pointer(cell_mem);
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

    /// Set the root of the noun slab.
    ///
    /// Panics if the given root is not in the noun slab or PMA.
    pub fn set_root(&mut self, root: Noun) {
        if let Ok(allocated) = root.as_allocated() {
            match allocated.as_either() {
                Either::Left(indirect) => {
                    let ptr = unsafe { indirect.to_raw_pointer() };
                    let u8_ptr = ptr as *const u8;
                    for slab in &self.slabs {
                        if unsafe { u8_ptr >= slab.0 && u8_ptr < slab.0.add(slab.1.size()) } {
                            self.root = root;
                            return;
                        }
                    }
                    panic!("Set root of NounSlab to noun from outside slab");
                }
                Either::Right(cell) => {
                    let ptr = unsafe { cell.to_raw_pointer() };
                    let u8_ptr = ptr as *const u8;
                    for slab in &self.slabs {
                        if unsafe { u8_ptr >= slab.0 && u8_ptr < slab.0.add(slab.1.size()) } {
                            self.root = root;
                            return;
                        }
                    }
                    panic!("Set root of NounSlab to noun from outside slab");
                }
            }
        }
        self.root = root;
    }

    /// Get the root noun
    ///
    /// # Safety: The noun must not be used past the lifetime of the slab.
    pub unsafe fn root(&self) -> &Noun {
        &self.root
    }

    /// Get the root noun
    ///
    /// # Safety: The noun must not be used past the lifetime of the slab.
    pub unsafe fn root_mut(&mut self) -> &mut Noun {
        &mut self.root
    }
}

impl<J: Jammer> NounSlab<J> {
    pub fn jam(&self) -> Bytes {
        J::jam(unsafe { *self.root() })
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
    pub fn insert(&mut self, key: Noun, value: V) {
        let key_mug = slab_mug(key) as u64;
        if let Some(vec) = self.0.get_mut(key_mug) {
            let mut chain_iter = vec[..].iter_mut();
            if let Some(entry) = chain_iter.find(|entry| slab_noun_equality(&key, &entry.0)) {
                entry.1 = value;
            } else {
                vec.push((key, value))
            }
        } else {
            self.0.insert(key_mug, vec![(key, value)]);
        }
    }

    pub fn get(&self, key: Noun) -> Option<&V> {
        let key_mug = slab_mug(key) as u64;
        if let Some(vec) = self.0.get(key_mug) {
            let mut chain_iter = vec[..].iter();
            if let Some(entry) =
                chain_iter.find(|entry| slab_noun_equality(&(key as Noun), &entry.0))
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

pub fn slab_equality(a: &NounSlab, b: &NounSlab) -> bool {
    slab_noun_equality(&a.root, &b.root)
}

// Does not unify: slabs are collected all-at-once so there's no point.
pub fn slab_noun_equality(a: &Noun, b: &Noun) -> bool {
    let mut already_equal: IntMap<u128, ()> = IntMap::new();

    fn ae_keys(a: Noun, b: Noun) -> (u128, u128) {
        let a_raw = unsafe { a.as_raw() } as u128;
        let b_raw = unsafe { b.as_raw() } as u128;
        (a_raw << 64 | b_raw, b_raw << 64 | a_raw)
    }

    fn check_ae(ae: &IntMap<u128, ()>, a: Noun, b: Noun) -> bool {
        let (key1, key2) = ae_keys(a, b);
        ae.contains_key(key1) | ae.contains_key(key2)
    }

    fn set_ae(ae: &mut IntMap<u128, ()>, a: Noun, b: Noun) {
        let (key1, _key2) = ae_keys(a, b);
        ae.insert(key1, ());
    }

    enum StackEntry {
        Nouns(Noun, Noun),
        Cells(Noun, Noun),
    }

    let mut stack = vec![StackEntry::Nouns(*a, *b)];
    loop {
        if let Some(entry) = stack.pop() {
            match entry {
                StackEntry::Cells(a, b) => {
                    set_ae(&mut already_equal, a, b);
                }
                StackEntry::Nouns(a, b) => {
                    if unsafe { a.raw_equals(&b) } {
                        continue;
                    }

                    if check_ae(&already_equal, a, b) {
                        continue;
                    }

                    match (
                        a.as_ref_either_direct_allocated(),
                        b.as_ref_either_direct_allocated(),
                    ) {
                        (Either::Right(a_allocated), Either::Right(b_allocated)) => {
                            if let Some(a_mug) = a_allocated.get_cached_mug() {
                                if let Some(b_mug) = b_allocated.get_cached_mug() {
                                    if a_mug != b_mug {
                                        break false;
                                    }
                                }
                            };

                            match (a_allocated.as_ref_either(), b_allocated.as_ref_either()) {
                                (Either::Left(a_indirect), Either::Left(b_indirect)) => {
                                    if a_indirect.as_slice() != b_indirect.as_slice() {
                                        break false;
                                    }
                                    set_ae(&mut already_equal, a, b);
                                    continue;
                                }
                                (Either::Right(a_cell), Either::Right(b_cell)) => {
                                    stack.push(StackEntry::Cells(a, b));
                                    stack.push(StackEntry::Nouns(a_cell.tail(), b_cell.tail()));
                                    stack.push(StackEntry::Nouns(a_cell.head(), b_cell.head()));
                                    continue;
                                }
                                _ => {
                                    break false;
                                }
                            }
                        }
                        _ => {
                            break false;
                        }
                    }
                }
            }
        } else {
            break true;
        }
    }
}

fn slab_mug(a: Noun) -> u32 {
    let mut stack = vec![a];
    while let Some(noun) = stack.pop() {
        if let Ok(mut allocated) = noun.as_allocated() {
            if allocated.get_cached_mug().is_none() {
                match allocated.as_either() {
                    Either::Left(indirect) => unsafe {
                        set_mug(&mut allocated, calc_atom_mug_u32(indirect.as_atom()));
                    },
                    Either::Right(cell) => match (get_mug(cell.head()), get_mug(cell.tail())) {
                        (Some(head_mug), Some(tail_mug)) => unsafe {
                            set_mug(&mut allocated, calc_cell_mug_u32(head_mug, tail_mug));
                        },
                        _ => {
                            stack.push(noun);
                            stack.push(cell.tail());
                            stack.push(cell.head());
                        }
                    },
                }
            }
        }
    }
    get_mug(a).expect("Noun should have a mug once mugged.")
}

enum CueStackEntry {
    DestinationPointer(*mut Noun),
    BackRef(u64, *const Noun),
}

// gonna use this like an ML module
/// This makes us modular over different implementations of jam and cue
pub trait Jammer: Sized {
    fn jam(noun: Noun) -> Bytes;
    fn cue(slab: &mut NounSlab<Self>, bytes: Bytes) -> Result<Noun, CueError>;
}

pub struct NockJammer;

impl Jammer for NockJammer {
    fn jam(noun: Noun) -> Bytes {
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

        fn mat_atom(buffer: &mut BitVec<u8, Lsb0>, atom: Atom) {
            if unsafe { atom.as_noun().raw_equals(&D(0)) } {
                buffer.extend_from_bitslice(bits![u8, Lsb0; 0, 1]);
                return;
            }
            let atom_sz = met0_usize(atom);
            let atom_sz_sz = met0_u64_to_usize(atom_sz as u64);
            buffer.push(false); // atom tag
            let buffer_len = buffer.len();
            buffer.resize(buffer_len + atom_sz_sz, false);
            buffer.push(true);
            buffer.extend_from_bitslice(
                &BitSlice::<_, Lsb0>::from_element(&atom_sz)[0..atom_sz_sz - 1],
            );
            buffer.extend_from_bitslice(&atom.as_bitslice()[0..atom_sz]);
        }
        let mut backref_map = NounMap::<usize>::new();
        let mut stack = vec![noun];
        let mut buffer = bitvec![u8, Lsb0; 0; 0];
        while let Some(noun) = stack.pop() {
            if let Some(backref) = backref_map.get(noun) {
                if let Ok(atom) = noun.as_atom() {
                    if met0_u64_to_usize(*backref as u64) < met0_usize(atom) {
                        mat_backref(&mut buffer, *backref);
                    } else {
                        mat_atom(&mut buffer, atom)
                    }
                } else {
                    mat_backref(&mut buffer, *backref);
                }
            } else {
                backref_map.insert(noun, buffer.len());
                match noun.as_either_atom_cell() {
                    Either::Left(atom) => {
                        mat_atom(&mut buffer, atom);
                    }
                    Either::Right(cell) => {
                        buffer.extend_from_bitslice(bits![u8, Lsb0; 1, 0]); // cell tag
                        stack.push(cell.tail());
                        stack.push(cell.head());
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
                        let (mut indirect, slice) =
                            unsafe { IndirectAtom::new_raw_mut_bitslice(slab, indirect_words) };
                        slice[0..sz].clone_from_bitslice(&buffer[*cursor..*cursor + sz]);
                        *cursor += sz;
                        Ok(unsafe { indirect.normalize_as_atom() })
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
                    if bitslice[cursor] {
                        // 1
                        cursor += 1;
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
    use bitvec::prelude::*;
    use ibig::ubig;
    use nockvm::noun::{D, T};
    use nockvm_macros::tas;

    use super::*;
    use crate::AtomExt;
    #[test]
    fn test_ubig_alloc() {
        let mut slab: NounSlab = NounSlab::new();
        let big_exp = ubig!(0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF);
        let atom = Atom::from_ubig(&mut slab, &big_exp);
        let big = atom.as_ubig(&mut slab);
        assert_eq!(big, big_exp);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_jam() {
        let mut slab: NounSlab = NounSlab::new();
        let test_noun = T(
            &mut slab,
            &[D(tas!(b"request")), D(tas!(b"block")), D(tas!(b"by-id")), D(0)],
        );
        slab.set_root(test_noun);
        let jammed: Vec<u8> = slab.jam().to_vec();
        println!("jammed: {:?}", jammed);

        let mut stack = NockStack::new(1000, 0);
        let mut nockvm_jammed: Vec<u8> = nockvm::serialization::jam(&mut stack, test_noun)
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
        assert!(
            slab_noun_equality(unsafe { original_slab.root() }, &cued_noun),
            "Original and cued nouns should be equal"
        );
    }

    #[test]
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

        assert!(
            slab_noun_equality(unsafe { slab.root() }, &cued_noun),
            "Complex nouns should be equal after jam/cue roundtrip"
        );
    }

    #[test]
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

        assert!(
            slab_noun_equality(&noun_with_indirect, &cued_noun),
            "Nouns with indirect atoms should be equal after jam/cue roundtrip"
        );
    }

    #[test]
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

        assert!(
            slab_noun_equality(unsafe { slab.root() }, &cued_noun),
            "Nouns with tas! macros should be equal after jam/cue roundtrip"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
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
            assert!(
                slab_noun_equality(&cued_noun, &expected_noun),
                "Cued noun should equal [1 0]"
            );
        }
    }

    #[test]
    fn test_cell_construction_for_noun_slab() {
        let mut slab: NounSlab = NounSlab::new();
        let (cell, cell_mem_ptr) = unsafe { Cell::new_raw_mut(&mut slab) };
        unsafe { assert!(cell_mem_ptr as *const CellMemory == cell.to_raw_pointer()) };
    }

    #[test]
    fn test_noun_slab_copy_into() {
        let mut slab: NounSlab = NounSlab::new();
        let test_noun = T(&mut slab, &[D(5), D(23)]);
        slab.set_root(test_noun);
        let mut copy_slab: NounSlab = NounSlab::new();
        copy_slab.copy_into(test_noun);
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
    fn test_nounslab_modify() {
        let mut slab: NounSlab = NounSlab::new();
        slab.modify(|root| vec![D(0), D(tas!(b"bind")), root]);
        let mut test_slab: NounSlab = NounSlab::new();
        slab_noun_equality(
            &slab.root,
            &T(&mut test_slab, &[D(0), D(tas!(b"bind")), D(0)]),
        );
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
