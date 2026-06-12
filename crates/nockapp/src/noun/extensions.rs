use std::ptr::copy_nonoverlapping;

use bytes::Bytes;
use either::Either;
use intmap::IntMap;
use nockvm::ext::AtomExt as CoreAtomExt;
pub use nockvm::ext::{IndirectAtomExt, JammedNoun, NounExt};
use nockvm::noun::{Atom, Cell, IndirectAtom, NounAllocator, NounSpace, D};
use noun_serde::NounEncode;

use crate::noun::slab::{NockJammer, NounSlab};
use crate::{Noun, Result, ToBytes, ToBytesExt};

// TODO: This exists largely because nockapp doesn't own the [`Atom`] type from [`nockvm`].
// TODO: The next step for this should be to lower the methods on this trait to a concrete `impl` stanza for [`Atom`] in [`nockvm`].
// TODO: In the course of doing so, we should split out a serialization trait that has only the [`AtomExt::from_value`] method as a public API in [`nockvm`].
// The goal would be to canonicalize the Atom representations of various Rust types. When it needs to be specialized, users can make a newtype.
pub trait AtomExt: CoreAtomExt {
    fn from_bytes<A: NounAllocator>(allocator: &mut A, bytes: &Bytes) -> Atom;
    fn from_value<A: NounAllocator, T: ToBytes>(allocator: &mut A, value: T) -> Result<Atom>;
}

impl AtomExt for Atom {
    // TODO: This is iffy. What byte representation is it expecting and why?
    fn from_bytes<A: NounAllocator>(allocator: &mut A, bytes: &Bytes) -> Atom {
        <Self as CoreAtomExt>::from_bytes(allocator, bytes.as_ref())
    }

    // TODO: This is worth making into a public/supported part of [`nockvm`]'s API.
    fn from_value<A: NounAllocator, T: ToBytes>(allocator: &mut A, value: T) -> Result<Atom> {
        let data: Bytes = value.as_bytes()?;
        Ok(<Self as CoreAtomExt>::from_bytes(allocator, data.as_ref()))
    }
}

#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot implement `IntoNoun` safely",
    label = "use `IntoSlab` or allocate into a caller-owned allocator"
)]
pub trait IntoNoun {
    fn into_noun(self) -> Noun;
}

impl IntoNoun for Atom {
    fn into_noun(self) -> Noun {
        self.as_noun()
    }
}
impl IntoNoun for u64 {
    fn into_noun(self) -> Noun {
        unsafe { Atom::from_raw(self).into_noun() }
    }
}

impl FromAtom for u64 {
    fn from_atom(atom: Atom, space: &NounSpace) -> Self {
        atom.in_space(space).as_u64().unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        })
    }
}

impl IntoNoun for Noun {
    fn into_noun(self) -> Noun {
        self
    }
}
impl !IntoNoun for &str {}

pub trait AsSlabVec {
    fn as_slab_vec(&self, space: &NounSpace) -> Vec<NounSlab>;
}

impl AsSlabVec for Noun {
    fn as_slab_vec(&self, space: &NounSpace) -> Vec<NounSlab> {
        let noun_list = *self;
        let mut slab_vec = Vec::new();
        for noun in noun_list.in_space(space).list_iter() {
            let mut new_slab = NounSlab::new();
            new_slab.copy_into(noun.noun(), space);
            slab_vec.push(new_slab);
        }
        slab_vec
    }
}

impl AsSlabVec for NounSlab {
    fn as_slab_vec(&self, _space: &NounSpace) -> Vec<NounSlab> {
        let noun_list = unsafe { self.root() };
        let space = self.noun_space();
        noun_list.as_slab_vec(&space)
    }
}

pub trait FromAtom {
    fn from_atom(atom: Atom, space: &NounSpace) -> Self;
}
impl FromAtom for Noun {
    fn from_atom(atom: Atom, _space: &NounSpace) -> Self {
        atom.as_noun()
    }
}

pub trait IntoSlab {
    fn into_slab(self) -> NounSlab;
}

impl IntoSlab for &str {
    fn into_slab(self) -> NounSlab {
        let mut slab = NounSlab::new();
        let bytes = self.to_bytes().unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        let noun =
            <IndirectAtom as IndirectAtomExt>::from_bytes(&mut slab, bytes.as_slice()).as_noun();
        slab.set_root(noun);
        slab
    }
}

const CACHED_MUG_METADATA_MASK: u64 = 0x7fff_ffff;

pub trait NounAllocatorExt {
    fn copy_into(&mut self, noun: Noun, space: &NounSpace) -> Noun;
}

impl<A: NounAllocator> NounAllocatorExt for A {
    fn copy_into(&mut self, noun: Noun, space: &NounSpace) -> Noun {
        let mut copied: IntMap<u64, Noun> = IntMap::new();
        let mut stack = Vec::with_capacity(32);
        let mut res = D(0);
        stack.push((noun, &mut res as *mut Noun));
        while let Some((noun, dest)) = stack.pop() {
            match noun.as_either_direct_allocated() {
                Either::Left(direct) => unsafe {
                    *dest = direct.as_noun();
                },
                Either::Right(allocated) => match allocated.as_either() {
                    Either::Left(indirect) => {
                        let indirect_handle = indirect.as_atom().in_space(space);
                        let source_key = unsafe { indirect_handle.raw_pointer() as u64 };
                        if let Some(copied_noun) = copied.get(source_key) {
                            unsafe { *dest = *copied_noun };
                            continue;
                        }
                        let copied_mem = unsafe { self.alloc_indirect(indirect_handle.size()) };
                        unsafe {
                            copy_nonoverlapping(
                                indirect_handle.raw_pointer(),
                                copied_mem,
                                indirect_handle.raw_size(),
                            );
                            *copied_mem &= CACHED_MUG_METADATA_MASK;
                        }
                        let copied_noun = unsafe {
                            IndirectAtom::from_raw_pointer(copied_mem)
                                .as_atom()
                                .as_noun()
                        };
                        copied.insert(source_key, copied_noun);
                        unsafe { *dest = copied_noun };
                    }
                    Either::Right(cell) => {
                        let cell_handle = cell.in_space(space);
                        let source_key = unsafe { cell_handle.raw_pointer() as u64 };
                        if let Some(copied_noun) = copied.get(source_key) {
                            unsafe { *dest = *copied_noun };
                            continue;
                        }
                        let copied_mem = unsafe { self.alloc_cell() };
                        unsafe {
                            copy_nonoverlapping(cell_handle.raw_pointer(), copied_mem, 1);
                            (*copied_mem).metadata &= CACHED_MUG_METADATA_MASK;
                        }
                        let copied_noun = unsafe { Cell::from_raw_pointer(copied_mem).as_noun() };
                        copied.insert(source_key, copied_noun);
                        unsafe { *dest = copied_noun };
                        unsafe {
                            stack.push((cell_handle.tail().noun(), &mut (*copied_mem).tail));
                            stack.push((cell_handle.head().noun(), &mut (*copied_mem).head));
                        }
                    }
                },
            }
        }
        res
    }
}

pub trait NounJamExt {
    fn jam_bytes(self, space: &NounSpace) -> Bytes;
}

impl NounJamExt for Noun {
    fn jam_bytes(self, space: &NounSpace) -> Bytes {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        slab.copy_into(self, space);
        slab.jam()
    }
}

pub trait NounEncodeJamExt: NounEncode {
    fn jam_bytes(&self) -> Bytes {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = self.to_noun(&mut slab);
        slab.set_root(noun);
        slab.jam()
    }
}

impl<T: NounEncode> NounEncodeJamExt for T {}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use nockvm::mem::{NockStack, NOCK_STACK_SIZE_TINY};
    use nockvm::mug::{calc_atom_mug_u32, calc_cell_mug_u32, get_mug, set_mug};
    use nockvm::noun::{Atom, NounAllocator, D, T};

    use super::{AtomExt, IntoSlab, NounAllocatorExt};
    use crate::noun::slab::{NockJammer, NounSlab};

    #[test]
    fn str_into_slab_allocates_in_destination_slab() {
        let slab = "hello".into_slab();
        let root = unsafe { *slab.root() };
        let space = slab.noun_space();
        let atom = root
            .in_space(&space)
            .as_atom()
            .expect("root should be an atom");
        let text = atom
            .into_string()
            .expect("root atom should decode to utf-8");
        assert_eq!(text, "hello");
    }

    #[test]
    fn copy_into_preserves_shared_cells() {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let shared = T(&mut slab, &[D(1), D(2)]);
        let root = T(&mut slab, &[shared, shared]);
        let source_space = slab.noun_space();

        let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let copied = stack.copy_into(root, &source_space);
        let copied_space = stack.noun_space();
        let copied_cell = copied
            .in_space(&copied_space)
            .as_cell()
            .expect("copied root should be a cell");
        let left = copied_cell.head().noun();
        let right = copied_cell.tail().noun();

        assert!(
            unsafe { left.raw_equals(&right) },
            "copy_into should preserve repeated references instead of duplicating subtrees"
        );
        assert!(
            !unsafe { left.raw_equals(&shared) },
            "copy_into should allocate shared subtrees in the destination"
        );
    }

    #[test]
    fn allocator_copy_into_preserves_source_cached_mugs() {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let cell = T(&mut slab, &[D(5), D(23)]);
        let atom = Atom::from_bytes(&mut slab, &Bytes::from_static(b"large-indirect-atom"));
        let atom_noun = atom.as_noun();
        let source_space = slab.noun_space();
        let atom_mug = calc_atom_mug_u32(atom, &source_space);
        let cell_mug = {
            let cell_handle = cell
                .in_space(&source_space)
                .as_cell()
                .expect("source cell should be allocated");
            let head_mug = get_mug(cell_handle.head().noun(), &source_space).expect("head mug");
            let tail_mug = get_mug(cell_handle.tail().noun(), &source_space).expect("tail mug");
            unsafe { calc_cell_mug_u32(head_mug, tail_mug, &source_space) }
        };

        unsafe {
            let mut cell_allocated = cell.as_allocated().expect("cell should be allocated");
            set_mug(&mut cell_allocated, cell_mug, &source_space);
            let mut atom_allocated = atom_noun
                .as_allocated()
                .expect("indirect atom should be allocated");
            set_mug(&mut atom_allocated, atom_mug, &source_space);
        }

        let root = T(&mut slab, &[cell, atom_noun]);
        let source_space = slab.noun_space();
        let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let copied = stack.copy_into(root, &source_space);
        let copied_space = stack.noun_space();
        let copied_cell = copied
            .in_space(&copied_space)
            .as_cell()
            .expect("copied root should be a cell");

        assert_eq!(
            get_mug(copied_cell.head().noun(), &copied_space),
            Some(cell_mug)
        );
        assert_eq!(
            get_mug(copied_cell.tail().noun(), &copied_space),
            Some(atom_mug)
        );
    }
}
