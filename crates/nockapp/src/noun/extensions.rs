use core::str;
use std::iter::Iterator;
use std::ptr::copy_nonoverlapping;

use bincode::{Decode, Encode};
use bytes::Bytes;
use either::Either;
use nockvm::interpreter::Error;
use nockvm::mem::NockStack;
use nockvm::noun::{Atom, Cell, IndirectAtom, NounAllocator, D};
use nockvm::serialization::{cue, jam};

use crate::noun::slab::NounSlab;
use crate::{Noun, Result, ToBytes, ToBytesExt};

pub trait NounExt {
    fn cue_bytes(stack: &mut NockStack, bytes: &Bytes) -> Result<Noun, Error>;
    fn cue_bytes_slice(stack: &mut NockStack, bytes: &[u8]) -> Result<Noun, Error>;
    fn jam_self(self, stack: &mut NockStack) -> JammedNoun;
    fn list_iter(self) -> impl Iterator<Item = Noun>;
    fn eq_bytes(self, bytes: impl AsRef<[u8]>) -> bool;
}

impl NounExt for Noun {
    fn cue_bytes(stack: &mut NockStack, bytes: &Bytes) -> Result<Noun, Error> {
        let atom = Atom::from_bytes(stack, bytes);
        cue(stack, atom)
    }

    // generally, we should be using `cue_bytes`, but if we're not going to be passing it around
    // its OK to just cue a byte slice to avoid copying.
    fn cue_bytes_slice(stack: &mut NockStack, bytes: &[u8]) -> Result<Noun, Error> {
        let atom = unsafe {
            IndirectAtom::new_raw_bytes(stack, bytes.len(), bytes.as_ptr()).normalize_as_atom()
        };
        cue(stack, atom)
    }

    fn jam_self(self, stack: &mut NockStack) -> JammedNoun {
        JammedNoun::from_noun(stack, self)
    }

    fn list_iter(self) -> impl Iterator<Item = Noun> {
        NounListIterator(self)
    }

    fn eq_bytes(self, bytes: impl AsRef<[u8]>) -> bool {
        if let Ok(a) = self.as_atom() {
            a.eq_bytes(bytes)
        } else {
            false
        }
    }
}

// TODO: This exists largely because nockapp doesn't own the [`Atom`] type from [`nockvm`].
// TODO: The next step for this should be to lower the methods on this trait to a concrete `impl` stanza for [`Atom`] in [`nockvm`].
// TODO: In the course of doing so, we should split out a serialization trait that has only the [`AtomExt::from_value`] method as a public API in [`nockvm`].
// The goal would be to canonicalize the Atom representations of various Rust types. When it needs to be specialized, users can make a newtype.
pub trait AtomExt {
    fn from_bytes<A: NounAllocator>(allocator: &mut A, bytes: &Bytes) -> Atom;
    fn from_value<A: NounAllocator, T: ToBytes>(allocator: &mut A, value: T) -> Result<Atom>;
    fn eq_bytes(self, bytes: impl AsRef<[u8]>) -> bool;
    fn to_bytes_until_nul(self) -> Result<Vec<u8>>;
    fn into_string(self) -> Result<String>;
}

impl AtomExt for Atom {
    // TODO: This is iffy. What byte representation is it expecting and why?
    fn from_bytes<A: NounAllocator>(allocator: &mut A, bytes: &Bytes) -> Atom {
        unsafe {
            IndirectAtom::new_raw_bytes(allocator, bytes.len(), bytes.as_ptr()).normalize_as_atom()
        }
    }

    // TODO: This is worth making into a public/supported part of [`nockvm`]'s API.
    fn from_value<A: NounAllocator, T: ToBytes>(allocator: &mut A, value: T) -> Result<Atom> {
        unsafe {
            let data: Bytes = value.as_bytes()?;
            Ok(
                IndirectAtom::new_raw_bytes(allocator, data.len(), data.as_ptr())
                    .normalize_as_atom(),
            )
        }
    }

    /** Test for byte equality, ignoring trailing 0s in the Atom representation
        beyond the length of the bytes compared to
    */
    fn eq_bytes(self, bytes: impl AsRef<[u8]>) -> bool {
        let bytes_ref = bytes.as_ref();
        let atom_bytes = self.as_ne_bytes();
        // TODO: Turn this into a match on a cmp?
        #[allow(clippy::comparison_chain)]
        if bytes_ref.len() > atom_bytes.len() {
            false
        } else if bytes_ref.len() == atom_bytes.len() {
            atom_bytes == bytes_ref
        } else {
            // check for nul bytes beyond comparing bytestring
            for b in &atom_bytes[bytes_ref.len()..] {
                if *b != 0u8 {
                    return false;
                }
            }
            &atom_bytes[0..bytes_ref.len()] == bytes_ref
        }
    }

    fn to_bytes_until_nul(self) -> Result<Vec<u8>> {
        let bytes = str::from_utf8(self.as_ne_bytes())?;
        Ok(bytes.trim_end_matches('\0').as_bytes().to_vec())
    }

    fn into_string(self) -> Result<String> {
        let str = str::from_utf8(self.as_ne_bytes())?;
        Ok(str.trim_end_matches('\0').to_string())
    }
}

#[derive(Clone, PartialEq, Debug, Encode, Decode)]
pub struct JammedNoun(#[bincode(with_serde)] pub Bytes);

impl JammedNoun {
    pub fn new(bytes: Bytes) -> Self {
        Self(bytes)
    }

    pub fn from_noun(stack: &mut NockStack, noun: Noun) -> Self {
        let jammed_atom = jam(stack, noun);
        JammedNoun(Bytes::copy_from_slice(jammed_atom.as_ne_bytes()))
    }

    pub fn cue_self(&self, stack: &mut NockStack) -> Result<Noun, Error> {
        let atom = unsafe {
            IndirectAtom::new_raw_bytes(stack, self.0.len(), self.0.as_ptr()).normalize_as_atom()
        };
        cue(stack, atom)
    }
}

impl From<&[u8]> for JammedNoun {
    fn from(bytes: &[u8]) -> Self {
        JammedNoun::new(Bytes::copy_from_slice(bytes))
    }
}

impl From<Vec<u8>> for JammedNoun {
    fn from(byte_vec: Vec<u8>) -> Self {
        JammedNoun::new(Bytes::from(byte_vec))
    }
}

impl AsRef<Bytes> for JammedNoun {
    fn as_ref(&self) -> &Bytes {
        &self.0
    }
}

impl AsRef<[u8]> for JammedNoun {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl Default for JammedNoun {
    fn default() -> Self {
        JammedNoun::new(Bytes::new())
    }
}

pub struct NounListIterator(Noun);

impl Iterator for NounListIterator {
    type Item = Noun;
    fn next(&mut self) -> Option<Self::Item> {
        if let Ok(it) = self.0.as_cell() {
            self.0 = it.tail();
            Some(it.head())
        } else if unsafe { self.0.raw_equals(&D(0)) } {
            None
        } else {
            panic!("Improper list terminator: {:?}", self.0)
        }
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
        let contents_atom = unsafe {
            let bytes = self.to_bytes().unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            IndirectAtom::new_raw_bytes_ref(&mut slab, bytes.as_slice()).normalize_as_atom()
        };
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
