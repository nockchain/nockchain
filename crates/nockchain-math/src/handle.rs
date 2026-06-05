use nockvm::mem::NockStack;
use nockvm::noun::*;

use crate::felt::Felt;
use crate::mary::*;
use crate::noun_ext::*;
use crate::poly::*;

// The idea behind this trait is that you would like to create a noun and
// a corresponding data type (like a BPoly) from that Noun, sharing memory
// in such a way that modifications to one creates an analogous modification
// in the other.
//
// Theoretically, you could implement Handle for either the noun or the other
// type. We've chosen to implement them for nouns, but this whole system
// is provisional and subject to change.
pub trait Handle<A> {
    fn new_zeroed_handle_mut(stack: &mut NockStack, len: Option<usize>) -> (Self, A)
    where
        Self: Sized;
}

pub fn new_handle_mut_slice<'a, T, B: NounAllocator>(
    allocator: &mut B,
    len: Option<usize>,
) -> (IndirectAtom, &'a mut [T])
where
    T: Element,
{
    let handle_len = len.unwrap_or_else(|| {
        panic!(
            "Panicked at {}:{} (git sha: {:?})",
            file!(),
            line!(),
            option_env!("GIT_SHA")
        )
    }) * T::len();
    let (tail, dat_ptr) = unsafe { IndirectAtom::new_raw_mut_words(allocator, handle_len + 1) };
    *(dat_ptr.last_mut().unwrap_or_else(|| {
        panic!(
            "Panicked at {}:{} (git sha: {:?})",
            file!(),
            line!(),
            option_env!("GIT_SHA")
        )
    })) = 0x1;
    let res_poly = dat_ptr.as_mut_ptr() as *mut T;
    let sli_ref = unsafe {
        std::slice::from_raw_parts_mut(
            res_poly,
            len.unwrap_or_else(|| {
                panic!(
                    "Panicked at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            }),
        )
    };
    (tail, sli_ref)
}

pub fn new_handle_mut_felt<'a, T: NounAllocator>(alloc: &mut T) -> (IndirectAtom, &'a mut Felt) {
    let (felt_atom, dat_ptr) = unsafe { IndirectAtom::new_raw_mut_words(alloc, 4) };
    dat_ptr[3] = 0x1;
    (
        felt_atom,
        felt_atom.as_atom().as_mut_felt().unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        }),
    )
}

pub fn new_handle_mut_mary<'a, B: NounAllocator>(
    allocator: &mut B,
    step: usize,
    len: usize,
) -> (IndirectAtom, MarySliceMut<'a>) {
    let (tail, dat_ptr) = unsafe { IndirectAtom::new_raw_mut_words(allocator, (step * len) + 1) };
    *(dat_ptr.last_mut().unwrap_or_else(|| {
        panic!(
            "Panicked at {}:{} (git sha: {:?})",
            file!(),
            line!(),
            option_env!("GIT_SHA")
        )
    })) = 0x1;

    let res_dat = dat_ptr.as_mut_ptr();
    let sli_ref = unsafe { std::slice::from_raw_parts_mut(res_dat, step * len) };

    let res_mary = MarySliceMut {
        step: step as u32,
        len: len as u32,
        dat: sli_ref,
    };
    (tail, res_mary)
}

pub fn finalize_mary<A: NounAllocator>(
    allocator: &mut A,
    step: usize,
    len: usize,
    mut res: IndirectAtom,
) -> Noun {
    unsafe {
        res.normalize();
    }
    let array = T(allocator, &[D(len as u64), res.as_noun()]);

    T(allocator, &[D(step as u64), array])
}

pub fn finalize_poly<A: NounAllocator>(
    allocator: &mut A,
    len: Option<usize>,
    mut res: IndirectAtom,
) -> Noun {
    unsafe {
        res.normalize();
    }
    let head = Atom::new(
        allocator,
        len.unwrap_or_else(|| {
            panic!(
                "Panicked at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        }) as u64,
    )
    .as_noun();
    let res_cell = Cell::new(allocator, head, res.as_noun());
    res_cell.as_noun()
}
