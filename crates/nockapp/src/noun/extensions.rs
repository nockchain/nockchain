use std::ptr::copy_nonoverlapping;

use bytes::Bytes;
use either::Either;
use nockvm::ext::AtomExt as CoreAtomExt;
pub use nockvm::ext::{IndirectAtomExt, JammedNoun, NounExt};
use nockvm::noun::{Atom, Cell, IndirectAtom, NounAllocator, D};

use crate::noun::slab::NounSlab;
use crate::{Noun, Result, ToBytes, ToBytesExt};

// TODO: This exists largely because nockapp doesn't own the [`Atom`] type from [`nockvm`].
// TODO: The next step for this should be to lower the methods on this trait to a concrete `impl` stanza for [`Atom`] in [`nockvm`].
// TODO: In the course of doing so, we should split out a serialization trait that has only the [`AtomExt::from_value`] method as a public API in [`nockvm`].
// The goal would be to canonicalize the Atom representations of various Rust types. When it needs to be specialized, users can make a newtype.
pub trait AtomExt: CoreAtomExt {
    fn from_bytes<A: NounAllocator>(allocator: &mut A, bytes: &Bytes) -> Atom;
    fn from_value<A: NounAllocator, T: ToBytes>(allocator: &mut A, value: T) -> Result<Atom>;
    fn eq_bytes(self, bytes: impl AsRef<[u8]>) -> bool;
    fn to_bytes_until_nul(self) -> Result<Vec<u8>>;
    fn into_string(self) -> Result<String>;
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

    /** Test for byte equality, ignoring trailing 0s in the Atom representation
        beyond the length of the bytes compared to
    */
    fn eq_bytes(self, bytes: impl AsRef<[u8]>) -> bool {
        CoreAtomExt::eq_bytes(&self, bytes)
    }

    fn to_bytes_until_nul(self) -> Result<Vec<u8>> {
        CoreAtomExt::to_bytes_until_nul(&self).map_err(Into::into)
    }

    fn into_string(self) -> Result<String> {
        CoreAtomExt::into_string(self).map_err(Into::into)
    }
}

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
    fn from_atom(atom: Atom) -> Self {
        atom.as_u64().unwrap_or_else(|err| {
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
impl IntoNoun for &str {
    fn into_noun(self) -> Noun {
        let mut slab: NounSlab = NounSlab::new();
        let bytes = self.to_bytes().unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        let contents_atom =
            <IndirectAtom as IndirectAtomExt>::from_bytes(&mut slab, bytes.as_slice());
        Noun::from_atom(contents_atom)
    }
}

pub trait AsSlabVec {
    fn as_slab_vec(&self) -> Vec<NounSlab>;
}

impl AsSlabVec for Noun {
    fn as_slab_vec(&self) -> Vec<NounSlab> {
        let noun_list = *self;
        let mut slab_vec = Vec::new();
        for noun in noun_list.list_iter() {
            let mut new_slab = NounSlab::new();
            new_slab.copy_into(noun);
            slab_vec.push(new_slab);
        }
        slab_vec
    }
}

impl AsSlabVec for NounSlab {
    fn as_slab_vec(&self) -> Vec<NounSlab> {
        let noun_list = unsafe { self.root() };
        noun_list.as_slab_vec()
    }
}

pub trait FromAtom {
    fn from_atom(atom: Atom) -> Self;
}
impl FromAtom for Noun {
    fn from_atom(atom: Atom) -> Self {
        atom.as_noun()
    }
}

pub trait IntoSlab {
    fn into_slab(self) -> NounSlab;
}

impl IntoSlab for &str {
    fn into_slab(self) -> NounSlab {
        let mut slab = NounSlab::new();
        let noun = self.into_noun();
        slab.set_root(noun);
        slab
    }
}

pub trait NounAllocatorExt {
    fn copy_into(&mut self, noun: Noun) -> Noun;
}

impl<A: NounAllocator> NounAllocatorExt for A {
    fn copy_into(&mut self, noun: Noun) -> Noun {
        let mut stack = Vec::with_capacity(32);
        let mut res = D(0);
        stack.push((noun, &mut res as *mut Noun));
        while let Some((noun, dest)) = stack.pop() {
            match noun.as_either_direct_allocated() {
                Either::Left(d) => unsafe {
                    *dest = d.as_noun();
                },
                Either::Right(a) => match a.as_either() {
                    Either::Left(i) => unsafe {
                        let word_size = i.size();
                        let ia = self.alloc_indirect(word_size);
                        copy_nonoverlapping(i.to_raw_pointer(), ia, word_size + 2);
                        *dest = IndirectAtom::from_raw_pointer(ia).as_noun();
                    },
                    Either::Right(c) => unsafe {
                        let cm = self.alloc_cell();
                        *dest = Cell::from_raw_pointer(cm).as_noun();
                        stack.push((c.tail(), &mut (*cm).tail));
                        stack.push((c.head(), &mut (*cm).head));
                    },
                },
            }
        }
        res
    }
}
