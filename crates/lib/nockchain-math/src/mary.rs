use nockvm::noun::{IndirectAtom, Noun, NounAllocator, NounSpace};
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
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        Mary::try_from(*noun, space).map_err(|_| NounDecodeError::MaryDecodeError)
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
pub fn mary_snag(a: MarySlice, i: usize, res: &mut [u64]) {
    let step = a.step as usize;
    res.copy_from_slice(&a.dat[i * step..(i + 1) * step]);
}

#[inline(always)]
pub fn mary_weld_step(a: MarySlice, b: MarySlice, res: MarySliceMut) {
    assert_eq!(a.len, b.len);
    assert_eq!(res.step, a.step + b.step);

    let a_step = a.step as usize;
    let b_step = b.step as usize;
    let mut curr = 0;
    for i in 0..res.len as usize {
        res.dat[curr..curr + a_step].copy_from_slice(&a.dat[i * a_step..(i + 1) * a_step]);
        curr += a_step;

        res.dat[curr..curr + b_step].copy_from_slice(&b.dat[i * b_step..(i + 1) * b_step]);
        curr += b_step;
    }
}

#[inline(always)]
pub fn mary_zero_extend(a: MarySlice, res: MarySliceMut) {
    assert_eq!(a.step, res.step);

    let a_len = a.len as usize;
    let res_len = res.len as usize;
    let step = res.step as usize;
    res.dat[0..a_len * step].copy_from_slice(a.dat);
    res.dat[a_len * step..res_len * step].fill(0);
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
pub fn mary_transpose_(fpolys: MarySlice, offset: usize) -> Mary {
    let step = fpolys.step as usize;
    let len = fpolys.len as usize;
    let mut res = Mary {
        step: len as u32 * offset as u32,
        len: step as u32 / offset as u32,
        dat: vec![0; step * len],
    };

    let mut res_slice = res.as_mut_slice();
    mary_transpose(fpolys, offset, &mut res_slice);
    res
}

#[inline(always)]
pub fn zing_fpoly(a: Vec<&[u64]>, res: MarySliceMut) {
    let step = res.step as usize;
    for (i, item) in a.iter().enumerate() {
        res.dat[step * i..(step * (i + 1))].copy_from_slice(item)
    }
}

#[inline(always)]
pub fn zing_vecs(a: Vec<Vec<Belt>>, res: MarySliceMut) {
    let step = res.step as usize;
    for (i, item) in a.iter().enumerate() {
        res.dat[step * i..(step * (i + 1))].copy_from_slice(belts_to_u64s(item.as_slice()));
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

#[inline(always)]
pub fn to_belts_mut(sli: &mut [u64]) -> &mut [Belt] {
    unsafe {
        let ptr = sli.as_mut_ptr() as *mut Belt;
        std::slice::from_raw_parts_mut(ptr, sli.len())
    }
}

#[inline(always)]
pub fn to_felts(sli: &[u64]) -> &[Felt] {
    assert!(sli.len().is_multiple_of(3));
    unsafe {
        let ptr = sli.as_ptr() as *const Felt;
        std::slice::from_raw_parts(ptr, sli.len() / 3)
    }
}

#[inline(always)]
pub fn to_felts_mut(sli: &mut [u64]) -> &mut [Felt] {
    assert!(sli.len().is_multiple_of(3));
    unsafe {
        let ptr = sli.as_mut_ptr() as *mut Felt;
        std::slice::from_raw_parts_mut(ptr, sli.len() / 3)
    }
}

#[inline(always)]
pub fn belts_to_u64s(sli: &[Belt]) -> &[u64] {
    unsafe {
        let ptr = sli.as_ptr() as *const u64;
        std::slice::from_raw_parts(ptr, sli.len())
    }
}

pub fn felt_to_u64s(sli: &[Felt]) -> &[u64] {
    unsafe {
        let ptr = sli.as_ptr() as *const u64;
        std::slice::from_raw_parts(ptr, sli.len() * 3)
    }
}

/// Get next power of two of mary length.
pub fn table_height(table: &Table) -> usize {
    let n = table.mary.len as u64;
    if n == 1 {
        return 1;
    }
    let padded_len = 2u64.pow((n - 1).ilog2() + 1);
    padded_len as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mary_transpose_alloc_matches_buffered_transpose() {
        let mary = Mary {
            step: 4,
            len: 3,
            dat: (1..=12).collect(),
        };
        let mut expected = Mary {
            step: 6,
            len: 2,
            dat: vec![0; 12],
        };
        let mut expected_slice = expected.as_mut_slice();

        mary_transpose(mary.as_slice(), 2, &mut expected_slice);

        assert_eq!(mary_transpose_(mary.as_slice(), 2), expected);
    }

    #[test]
    fn mary_slice_conversion_helpers_preserve_layout() {
        let belts = [Belt(1), Belt(2), Belt(3)];
        assert_eq!(belts_to_u64s(&belts), &[1, 2, 3]);
        assert_eq!(to_belts(&[1, 2, 3]), belts);

        let mut belt_words = [4, 5];
        to_belts_mut(&mut belt_words)[1] = Belt(9);
        assert_eq!(belt_words, [4, 9]);

        let felts = [Felt([Belt(10), Belt(11), Belt(12)])];
        assert_eq!(felt_to_u64s(&felts), &[10, 11, 12]);
        assert_eq!(to_felts(&[10, 11, 12]), felts);

        let mut felt_words = [1, 2, 3];
        to_felts_mut(&mut felt_words)[0] = Felt([Belt(6), Belt(7), Belt(8)]);
        assert_eq!(felt_words, [6, 7, 8]);
    }
}
