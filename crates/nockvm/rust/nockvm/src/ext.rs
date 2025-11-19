use std::str;

use bincode::{Decode, Encode};
use bytes::Bytes;

use crate::interpreter::Error;
use crate::mem::NockStack;
use crate::noun::{Atom, IndirectAtom, Noun, NounAllocator, D};
use crate::serialization::{cue, jam};

/// Convenience helpers for working with `Atom`.
pub trait AtomExt {
    fn from_bytes<A: NounAllocator>(allocator: &mut A, bytes: &[u8]) -> Atom;
    fn eq_bytes<B: AsRef<[u8]>>(&self, bytes: B) -> bool;
    fn to_bytes_until_nul(&self) -> std::result::Result<Vec<u8>, str::Utf8Error>;
    fn into_string(self) -> std::result::Result<String, str::Utf8Error>;
}

impl AtomExt for Atom {
    fn from_bytes<A: NounAllocator>(allocator: &mut A, bytes: &[u8]) -> Atom {
        <IndirectAtom as IndirectAtomExt>::from_bytes(allocator, bytes)
    }

    fn eq_bytes<B: AsRef<[u8]>>(&self, bytes: B) -> bool {
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

    fn to_bytes_until_nul(&self) -> std::result::Result<Vec<u8>, str::Utf8Error> {
        str::from_utf8(self.as_ne_bytes())
            .map(|bytes| bytes.trim_end_matches('\0').as_bytes().to_vec())
    }

    fn into_string(self) -> std::result::Result<String, str::Utf8Error> {
        str::from_utf8(self.as_ne_bytes()).map(|string| string.trim_end_matches('\0').to_string())
    }
}

/// Extension helpers for safely constructing indirect atoms.
pub trait IndirectAtomExt {
    fn from_bytes<A: NounAllocator>(allocator: &mut A, bytes: &[u8]) -> Atom;

    unsafe fn from_raw_parts<A: NounAllocator>(
        allocator: &mut A,
        size: usize,
        data: *const u8,
    ) -> Atom;
}

impl IndirectAtomExt for IndirectAtom {
    fn from_bytes<A: NounAllocator>(allocator: &mut A, bytes: &[u8]) -> Atom {
        unsafe { Self::from_raw_parts(allocator, bytes.len(), bytes.as_ptr()) }
    }

    unsafe fn from_raw_parts<A: NounAllocator>(
        allocator: &mut A,
        size: usize,
        data: *const u8,
    ) -> Atom {
        Self::new_raw_bytes(allocator, size, data).normalize_as_atom()
    }
}

/// Helpers for working with nouns directly.
pub trait NounExt {
    fn cue_bytes(stack: &mut NockStack, bytes: &Bytes) -> std::result::Result<Noun, Error>;
    fn cue_bytes_slice(stack: &mut NockStack, bytes: &[u8]) -> std::result::Result<Noun, Error>;
    fn jam_self(self, stack: &mut NockStack) -> JammedNoun;
    fn list_iter(self) -> NounListIterator;
    fn eq_bytes(self, bytes: impl AsRef<[u8]>) -> bool;
}

impl NounExt for Noun {
    fn cue_bytes(stack: &mut NockStack, bytes: &Bytes) -> std::result::Result<Noun, Error> {
        let atom = <Atom as AtomExt>::from_bytes(stack, bytes.as_ref());
        cue(stack, atom)
    }

    fn cue_bytes_slice(stack: &mut NockStack, bytes: &[u8]) -> std::result::Result<Noun, Error> {
        let atom = <IndirectAtom as IndirectAtomExt>::from_bytes(stack, bytes);
        cue(stack, atom)
    }

    fn jam_self(self, stack: &mut NockStack) -> JammedNoun {
        JammedNoun::from_noun(stack, self)
    }

    fn list_iter(self) -> NounListIterator {
        NounListIterator(self)
    }

    fn eq_bytes(self, bytes: impl AsRef<[u8]>) -> bool {
        if let Ok(atom) = self.as_atom() {
            atom.eq_bytes(bytes)
        } else {
            false
        }
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

    pub fn cue_self(&self, stack: &mut NockStack) -> std::result::Result<Noun, Error> {
        let atom = <IndirectAtom as IndirectAtomExt>::from_bytes(stack, self.0.as_ref());
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
        if let Ok(cell) = self.0.as_cell() {
            self.0 = cell.tail();
            Some(cell.head())
        } else if unsafe { self.0.raw_equals(&D(0)) } {
            None
        } else {
            panic!("Improper list terminator: {:?}", self.0);
        }
    }
}

pub fn make_tas<A: NounAllocator>(allocator: &mut A, tas: &str) -> Atom {
    <Atom as AtomExt>::from_bytes(allocator, tas.as_bytes())
}
