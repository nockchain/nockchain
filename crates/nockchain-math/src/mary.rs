use nockvm::noun::{IndirectAtom, Noun, NounAllocator};
use noun_serde::{NounDecode, NounDecodeError, NounEncode};

use crate::belt::Belt;
use crate::felt::Felt;
use crate::handle::{finalize_mary, new_handle_mut_mary};

#[derive(Clone, PartialEq)]
pub struct Mary {
    pub step: u32,
    pub len: u32,
    pub dat: Vec<u64>,
}

#[derive(Clone, Copy)]
pub struct MarySlice<'a> {
    pub step: u32,
    pub len: u32,
    pub dat: &'a [u64],
}

pub struct MarySliceMut<'a> {
    pub step: u32,
    pub len: u32,
    pub dat: &'a mut [u64],
}

pub struct Table<'a> {
    pub num_cols: u32,
    pub mary: MarySlice<'a>,
}

impl Mary {
    pub fn as_slice(&self) -> MarySlice<'_> {
        MarySlice {
            step: self.step,
            len: self.len,
            dat: self.dat.as_slice(),
        }
    }
    pub fn as_mut_slice(&mut self) -> MarySliceMut<'_> {
        MarySliceMut {
            step: self.step,
            len: self.len,
            dat: self.dat.as_mut_slice(),
        }
    }
}

impl TryFrom<MarySlice<'_>> for &[Felt] {
    type Error = ();

    #[inline(always)]
    fn try_from(m: MarySlice) -> std::result::Result<Self, Self::Error> {
        assert_eq!(m.step, 3);

        let dat_slice: &[Felt] =
            unsafe { std::slice::from_raw_parts(m.dat.as_ptr() as *const Felt, m.len as usize) };
        Ok(dat_slice)
    }
}

impl TryFrom<MarySliceMut<'_>> for &[Felt] {
    type Error = ();

    #[inline(always)]
    fn try_from(m: MarySliceMut) -> std::result::Result<Self, Self::Error> {
        assert_eq!(m.step, 3);

        let dat_slice: &[Felt] = unsafe {
            std::slice::from_raw_parts(<[u64]>::as_ptr(&m.dat[0..3]) as *const Felt, m.len as usize)
        };
        Ok(dat_slice)
    }
}

impl TryFrom<MarySliceMut<'_>> for &mut [Felt] {
    type Error = ();

    #[inline(always)]
    fn try_from(m: MarySliceMut) -> std::result::Result<Self, Self::Error> {
        assert_eq!(m.step, 3);

        let dat_slice: &mut [Felt] = unsafe {
            std::slice::from_raw_parts_mut(
                <[u64]>::as_mut_ptr(&mut m.dat[0..3]) as *mut Felt,
                m.len as usize,
            )
        };
        Ok(dat_slice)
    }
}

impl NounDecode for Mary {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        Mary::try_from(*noun).map_err(|_| NounDecodeError::MaryDecodeError)
    }
}

impl NounEncode for Mary {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let (res, res_poly): (IndirectAtom, MarySliceMut) =
            new_handle_mut_mary(allocator, self.step as usize, self.len as usize);

        res_poly.dat.copy_from_slice(&self.dat[..]);

        finalize_mary(allocator, self.step as usize, self.len as usize, res)
    }
}

impl std::fmt::Debug for Mary {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "Mary: (step={}, len={}, dat={:?})\r",
            self.step, self.len, self.dat
        )
    }
}

impl std::fmt::Debug for MarySlice<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "MarySlice: (step={}, len={}, dat={:?})\r",
            self.step, self.len, self.dat
        )
    }
}

impl std::fmt::Debug for MarySliceMut<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "MarySliceMut: (step={}, len={}, dat={:?})\r",
            self.step, self.len, self.dat
        )
    }
}

#[inline(always)]
pub fn mary_weld(a: MarySlice, b: MarySlice, res: MarySliceMut) {
    assert_eq!(a.step, b.step);
    assert_eq!(res.len, a.len + b.len);
    let a_len = a.len as usize;
    let res_len = res.len as usize;
    let step = res.step as usize;
    res.dat[0..a_len * step].copy_from_slice(a.dat);
    res.dat[a_len * step..res_len * step].copy_from_slice(b.dat);
}

#[inline(always)]
pub fn mary_transpose(fpolys: MarySlice, offset: usize, res: &mut MarySliceMut) {
    let step = fpolys.step as usize;
    let len = fpolys.len as usize;

    let num_cols = step / offset;
    let num_rows = len;

    for i in 0..num_cols {
        for j in 0..num_rows {
            for k in 0..offset {
                res.dat[offset * (i * num_rows + j) + k] =
                    fpolys.dat[offset * (j * num_cols + i) + k];
            }
        }
    }
}

#[inline(always)]
pub fn snag_as_bpoly(a: MarySlice<'_>, i: usize) -> &[Belt] {
    let step = a.step as usize;
    to_belts(&a.dat[step * i..(step * (i + 1))])
}

#[inline(always)]
pub fn to_belts(sli: &[u64]) -> &[Belt] {
    unsafe {
        let ptr = sli.as_ptr() as *const Belt;
        std::slice::from_raw_parts(ptr, sli.len())
    }
}
