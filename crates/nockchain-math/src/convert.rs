use nockvm::jets::util::BAIL_FAIL;
use nockvm::jets::JetErr;
use nockvm::noun::{
    Atom, AtomHandle, CellHandle, Error, IndirectAtom, Noun, NounHandle, NounSpace, Result, D,
};
use noun_serde::{NounDecode, NounEncode};

use crate::belt::*;
use crate::felt::*;
use crate::handle::{finalize_poly, new_handle_mut_slice};
use crate::mary::*;
use crate::noun_ext::{AtomMathExt, AtomMathExtHandle, NounMathExt, NounMathExtHandle};
use crate::poly::*;

impl AtomMathExt for Atom {
    fn as_u32(&self) -> Result<u32> {
        if let Ok(a) = self.as_direct() {
            if a.bit_size() > 32 {
                Err(Error::NotRepresentable)
            } else {
                Ok(a.data() as u32)
            }
        } else {
            Err(Error::NotRepresentable)
        }
    }

    fn as_belt(&self, space: &NounSpace) -> Result<Belt> {
        if let Ok(x) = self.in_space(space).as_u64() {
            Ok(Belt(x))
        } else {
            Err(Error::NotRepresentable)
        }
    }

    fn as_felt<'a>(&self, space: &NounSpace) -> Result<&'a Felt> {
        let handle = self.in_space(space);
        if handle.is_direct() {
            return Err(Error::NotRepresentable);
        }
        if handle.size() != 4 {
            return Err(Error::NotRepresentable);
        }
        let buf_ptr = handle.data_pointer();
        unsafe {
            if *(buf_ptr.add(3)) != 0x1 {
                return Err(Error::NotRepresentable);
            }
            Ok(&*(buf_ptr as *const Felt))
        }
    }

    fn as_mut_felt<'a>(&self, space: &NounSpace) -> Result<&'a mut Felt> {
        let handle = self.in_space(space);
        if handle.is_direct() {
            return Err(Error::NotRepresentable);
        }
        if handle.size() != 4 {
            return Err(Error::NotRepresentable);
        }
        unsafe {
            let buf_ptr = handle.raw_pointer_mut().add(2);
            if *(buf_ptr.add(3)) != 0x1 {
                return Err(Error::NotRepresentable);
            }
            Ok(&mut *(buf_ptr as *mut Felt))
        }
    }
}

impl<'a> AtomMathExtHandle for AtomHandle<'a> {
    fn as_u32(&self) -> Result<u32> {
        if let Ok(a) = self.as_direct() {
            if a.bit_size() > 32 {
                Err(Error::NotRepresentable)
            } else {
                Ok(a.data() as u32)
            }
        } else {
            Err(Error::NotRepresentable)
        }
    }

    fn as_belt(&self) -> Result<Belt> {
        if let Ok(x) = self.as_u64() {
            Ok(Belt(x))
        } else {
            Err(Error::NotRepresentable)
        }
    }

    fn as_felt<'b>(&self) -> Result<&'b Felt> {
        if self.is_direct() {
            return Err(Error::NotRepresentable);
        }
        if self.size() != 4 {
            return Err(Error::NotRepresentable);
        }
        let buf_ptr = self.data_pointer();
        unsafe {
            if *(buf_ptr.add(3)) != 0x1 {
                return Err(Error::NotRepresentable);
            }
            Ok(&*(buf_ptr as *const Felt))
        }
    }

    fn as_mut_felt<'b>(&self) -> Result<&'b mut Felt> {
        if self.is_direct() {
            return Err(Error::NotRepresentable);
        }
        if self.size() != 4 {
            return Err(Error::NotRepresentable);
        }
        unsafe {
            let buf_ptr = self.raw_pointer_mut().add(2);
            if *(buf_ptr.add(3)) != 0x1 {
                return Err(Error::NotRepresentable);
            }
            Ok(&mut *(buf_ptr as *mut Felt))
        }
    }
}

impl NounMathExt for Noun {
    fn as_belt(&self, space: &NounSpace) -> Result<Belt> {
        if let Ok(atom) = self.in_space(space).as_atom() {
            atom.atom().as_belt(space)
        } else {
            Err(Error::NotRepresentable)
        }
    }

    fn as_felt<'a>(&self, space: &NounSpace) -> Result<&'a Felt> {
        if let Ok(atom) = self.in_space(space).as_atom() {
            atom.atom().as_felt(space)
        } else {
            Err(Error::NotRepresentable)
        }
    }

    fn as_mut_felt<'a>(&self, space: &NounSpace) -> Result<&'a mut Felt> {
        if let Ok(atom) = self.in_space(space).as_atom() {
            atom.atom().as_mut_felt(space)
        } else {
            Err(Error::NotRepresentable)
        }
    }

    fn uncell<const N: usize>(&self, space: &NounSpace) -> Result<[Self; N]> {
        let mut inp = *self;
        let mut cnt = 0;
        let mut ret: [Result<Self>; N] = [(); N].map(|_| {
            cnt += 1;
            if cnt == N {
                Ok(inp)
            } else {
                let c = inp.in_space(space).as_cell()?;
                inp = c.tail().noun();
                Ok(c.head().noun())
            }
        });
        if let Some(e) = ret.iter_mut().find(|v| v.is_err()) {
            let n = core::mem::replace(e, Ok(D(0)));
            return Err(n.expect_err("checked is_err above"));
        }
        Ok(ret.map(|v| v.expect("all results are Ok after filtering errors")))
    }
}

impl<'a> NounMathExtHandle for NounHandle<'a> {
    fn as_belt(&self) -> Result<Belt> {
        if let Ok(atom) = self.as_atom() {
            atom.as_belt()
        } else {
            Err(Error::NotRepresentable)
        }
    }

    fn as_felt<'b>(&self) -> Result<&'b Felt> {
        if let Ok(atom) = self.as_atom() {
            atom.as_felt()
        } else {
            Err(Error::NotRepresentable)
        }
    }

    fn as_mut_felt<'b>(&self) -> Result<&'b mut Felt> {
        if let Ok(atom) = self.as_atom() {
            atom.as_mut_felt()
        } else {
            Err(Error::NotRepresentable)
        }
    }

    fn uncell<const N: usize>(&self) -> Result<[Self; N]> {
        let mut inp = *self;
        let mut cnt = 0;
        let mut ret: [Result<Self>; N] = [(); N].map(|_| {
            cnt += 1;
            if cnt == N {
                Ok(inp)
            } else {
                let c = inp.as_cell()?;
                inp = c.tail();
                Ok(c.head())
            }
        });
        if let Some(e) = ret.iter_mut().find(|v| v.is_err()) {
            let n = core::mem::replace(e, Ok(D(0).in_space(self.space())));
            return match n {
                Ok(_) => unreachable!("checked is_err above"),
                Err(err) => Err(err),
            };
        }
        Ok(ret.map(|v| v.expect("all results are Ok after filtering errors")))
    }
}

impl MarySlice<'_> {
    #[allow(clippy::result_unit_err)]
    pub fn try_from(noun: Noun, space: &NounSpace) -> std::result::Result<Self, ()> {
        if noun.is_atom() {
            Err(())
        } else {
            MarySlice::try_from_cell(noun.in_space(space).as_cell()?, space)
        }
    }

    #[inline(always)]
    #[allow(clippy::result_unit_err)]
    pub fn try_from_cell(c: CellHandle<'_>, _space: &NounSpace) -> std::result::Result<Self, ()> {
        let step = c.head().as_atom()?.atom().as_u32()?;
        let len = c.tail().as_cell()?.head().as_atom()?.atom().as_u32()?;
        let cell = c.tail().as_cell()?;
        let dat_atom = cell.tail().as_atom()?;
        let dat_slice: &[u64] = if dat_atom.is_direct() {
            unsafe {
                let cell_ptr = cell.raw_pointer();
                let tail_ptr = &(*cell_ptr).tail as *const Noun;
                std::slice::from_raw_parts(tail_ptr as *const u64, (len * step) as usize)
            }
        } else {
            unsafe { std::slice::from_raw_parts(dat_atom.data_pointer(), (len * step) as usize) }
        };
        Ok(MarySlice {
            step,
            len,
            dat: dat_slice,
        })
    }
}

impl Mary {
    #[allow(clippy::result_unit_err)]
    pub fn try_from(noun: Noun, space: &NounSpace) -> std::result::Result<Self, ()> {
        if noun.is_atom() {
            Err(())
        } else {
            let slice = MarySlice::try_from_cell(noun.in_space(space).as_cell()?, space)?;
            Ok(Mary {
                step: slice.step,
                len: slice.len,
                dat: slice.dat.to_vec(),
            })
        }
    }
}

impl Table<'_> {
    #[allow(clippy::result_unit_err)]
    pub fn try_from(noun: Noun, space: &NounSpace) -> std::result::Result<Self, ()> {
        if noun.is_atom() {
            Err(())
        } else {
            Table::try_from_cell(noun.in_space(space).as_cell()?, space)
        }
    }

    #[inline(always)]
    #[allow(clippy::result_unit_err)]
    pub fn try_from_cell(c: CellHandle<'_>, space: &NounSpace) -> std::result::Result<Self, ()> {
        let full_width = c.head().as_atom()?.atom().as_u32()?;
        let mary_cell = c.tail().as_cell()?;
        let mary = MarySlice::try_from_cell(mary_cell, space)?;

        Ok(Table {
            num_cols: full_width,
            mary,
        })
    }
}

// TODO: use Ares::noun::Result or Error somehow for the methods that
// convert our structs from nouns
impl BPolySlice<'_> {
    #[inline(always)]
    pub fn try_from(noun: Noun, space: &NounSpace) -> std::result::Result<Self, JetErr> {
        if noun.is_atom() {
            Err(BAIL_FAIL)
        } else {
            BPolySlice::try_from_cell(noun.in_space(space).as_cell()?, space)
        }
    }

    #[inline(always)]
    pub fn try_from_cell(
        c: CellHandle<'_>,
        _space: &NounSpace,
    ) -> std::result::Result<Self, JetErr> {
        let head = c.head().as_atom();
        let tail = c.tail().as_atom();
        if let (Ok(head), Ok(tail)) = (head, tail) {
            let len32 = head.atom().as_u32()?;
            if tail.is_direct() {
                return Err(BAIL_FAIL);
            }
            let dat_slice: BPolySlice = unsafe {
                PolySlice(std::slice::from_raw_parts(
                    tail.data_pointer() as *const Belt,
                    len32 as usize,
                ))
            };
            Ok(dat_slice)
        } else {
            Err(BAIL_FAIL)
        }
    }
}

impl FPolySlice<'_> {
    #[inline(always)]
    pub fn try_from(noun: Noun, space: &NounSpace) -> std::result::Result<Self, JetErr> {
        if noun.is_atom() {
            Err(BAIL_FAIL)
        } else {
            FPolySlice::try_from_cell(noun.in_space(space).as_cell()?, space)
        }
    }

    #[inline(always)]
    pub fn try_from_cell(
        c: CellHandle<'_>,
        _space: &NounSpace,
    ) -> std::result::Result<Self, JetErr> {
        let head = c.head().as_atom();
        let tail = c.tail().as_atom();
        if let (Ok(head), Ok(tail)) = (head, tail) {
            let len32 = head.atom().as_u32()?;
            if tail.is_direct() {
                return Err(BAIL_FAIL);
            }
            let dat_slice: FPolySlice = unsafe {
                PolySlice(std::slice::from_raw_parts(
                    tail.data_pointer() as *const Felt,
                    len32 as usize,
                ))
            };
            Ok(dat_slice)
        } else {
            Err(BAIL_FAIL)
        }
    }
}

impl FPolyVec {
    #[inline(always)]
    pub fn try_from(noun: Noun, space: &NounSpace) -> std::result::Result<Self, JetErr> {
        if noun.is_atom() {
            Err(BAIL_FAIL)
        } else {
            FPolyVec::try_from_cell(noun.in_space(space).as_cell()?, space)
        }
    }

    #[inline(always)]
    pub fn try_from_cell(
        c: CellHandle<'_>,
        _space: &NounSpace,
    ) -> std::result::Result<Self, JetErr> {
        let head = c.head().as_atom();
        let tail = c.tail().as_atom();
        if let (Ok(head), Ok(tail)) = (head, tail) {
            let len32 = head.atom().as_u32()?;
            if tail.is_direct() {
                return Err(BAIL_FAIL);
            }
            let dat_vec: FPolyVec = unsafe {
                PolyVec(
                    std::slice::from_raw_parts(tail.data_pointer() as *const Felt, len32 as usize)
                        .to_vec(),
                )
            };
            Ok(dat_vec)
        } else {
            Err(BAIL_FAIL)
        }
    }
}

impl BPolyVec {
    #[inline(always)]
    pub fn try_from(noun: Noun, space: &NounSpace) -> std::result::Result<Self, JetErr> {
        if noun.is_atom() {
            Err(BAIL_FAIL)
        } else {
            BPolyVec::try_from_cell(noun.in_space(space).as_cell()?, space)
        }
    }

    #[inline(always)]
    pub fn try_from_cell(
        c: CellHandle<'_>,
        _space: &NounSpace,
    ) -> std::result::Result<Self, JetErr> {
        let head = c.head().as_atom();
        let tail = c.tail().as_atom();
        if let (Ok(head), Ok(tail)) = (head, tail) {
            let len32 = head.atom().as_u32()?;
            if tail.is_direct() {
                return Err(BAIL_FAIL);
            }
            let dat_vec: BPolyVec = unsafe {
                PolyVec(
                    std::slice::from_raw_parts(tail.data_pointer() as *const Belt, len32 as usize)
                        .to_vec(),
                )
            };
            Ok(dat_vec)
        } else {
            Err(BAIL_FAIL)
        }
    }
}

impl NounDecode for FPolyVec {
    fn from_noun(
        noun: &nockvm::noun::Noun,
        space: &NounSpace,
    ) -> std::result::Result<Self, noun_serde::NounDecodeError> {
        FPolyVec::try_from(*noun, space).map_err(|_| noun_serde::NounDecodeError::FPolyDecodeError)
    }
}

impl NounEncode for FPolyVec {
    fn to_noun<A: nockvm::noun::NounAllocator>(&self, allocator: &mut A) -> nockvm::noun::Noun {
        let (res, res_poly): (IndirectAtom, &mut [Felt]) =
            new_handle_mut_slice(allocator, Some(self.0.len()));
        res_poly.copy_from_slice(&self.0);
        finalize_poly(allocator, Some(self.0.len()), res)
    }
}

impl NounDecode for BPolyVec {
    fn from_noun(
        noun: &nockvm::noun::Noun,
        space: &NounSpace,
    ) -> std::result::Result<Self, noun_serde::NounDecodeError> {
        BPolyVec::try_from(*noun, space).map_err(|_| noun_serde::NounDecodeError::FPolyDecodeError)
    }
}

impl NounEncode for BPolyVec {
    fn to_noun<A: nockvm::noun::NounAllocator>(&self, allocator: &mut A) -> nockvm::noun::Noun {
        let (res, res_poly): (IndirectAtom, &mut [Belt]) =
            new_handle_mut_slice(allocator, Some(self.0.len()));
        res_poly.copy_from_slice(&self.0);
        finalize_poly(allocator, Some(self.0.len()), res)
    }
}
