use either::{Left, Right};
use nockvm::jets::util::BAIL_FAIL;
use nockvm::jets::JetErr;
use nockvm::noun::{Atom, Cell, Error, IndirectAtom, Noun, Result, D};
use noun_serde::{NounDecode, NounEncode};

use crate::belt::*;
use crate::felt::*;
use crate::handle::{finalize_poly, new_handle_mut_slice};
use crate::mary::*;
use crate::noun_ext::{AtomMathExt, NounMathExt};
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

    fn as_belt(&self) -> Result<Belt> {
        if let Ok(x) = self.as_u64() {
            Ok(Belt(x))
        } else {
            Err(Error::NotRepresentable)
        }
    }

    fn as_felt<'a>(&self) -> Result<&'a Felt> {
        if let Ok(atom) = self.as_indirect() {
            if atom.size() == 4 {
                let buf_ptr = atom.data_pointer();
                unsafe {
                    assert!(*(buf_ptr.add(3)) == 0x1);
                }
                let felt_ref: &Felt = unsafe { &*(buf_ptr as *const Felt) };
                Ok(felt_ref)
            } else {
                Err(Error::NotRepresentable)
            }
        } else {
            Err(Error::NotRepresentable)
        }
    }

    fn as_mut_felt<'a>(&self) -> Result<&'a mut Felt> {
        if let Ok(mut atom) = self.as_indirect() {
            if atom.size() == 4 {
                let buf_ptr = atom.data_pointer_mut();
                unsafe {
                    assert!(*(buf_ptr.add(3)) == 0x1);
                }
                let felt_ref: &mut Felt = unsafe { &mut *(buf_ptr as *mut Felt) };
                Ok(felt_ref)
            } else {
                Err(Error::NotRepresentable)
            }
        } else {
            Err(Error::NotRepresentable)
        }
    }
}

impl NounMathExt for Noun {
    fn as_belt(&self) -> Result<Belt> {
        if let Ok(atom) = self.as_atom() {
            atom.as_belt()
        } else {
            Err(Error::NotRepresentable)
        }
    }

    fn as_felt<'a>(&self) -> Result<&'a Felt> {
        if let Ok(atom) = self.as_atom() {
            atom.as_felt()
        } else {
            Err(Error::NotRepresentable)
        }
    }

    fn as_mut_felt<'a>(&self) -> Result<&'a mut Felt> {
        if let Ok(atom) = self.as_atom() {
            atom.as_mut_felt()
        } else {
            Err(Error::NotRepresentable)
        }
    }

    fn uncell<const N: usize>(&self) -> Result<[Self; N]> {
        let mut inp = *self;
        let mut cnt = 0;
        let mut ret = [(); N].map(|_| {
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
            let n = core::mem::replace(e, Ok(D(0)));
            return Err(n.unwrap_err());
        }
        Ok(ret.map(|v| v.unwrap()))
    }
}

impl TryFrom<Noun> for MarySlice<'_> {
    type Error = ();

    fn try_from(n: Noun) -> std::result::Result<Self, Self::Error> {
        if n.is_atom() {
            Err(())
        } else {
            MarySlice::try_from(n.as_cell()?)
        }
    }
}

impl TryFrom<Noun> for Mary {
    type Error = ();

    fn try_from(n: Noun) -> std::result::Result<Self, Self::Error> {
        if n.is_atom() {
            Err(())
        } else {
            let slice = MarySlice::try_from(n.as_cell()?)?;
            Ok(Mary {
                step: slice.step,
                len: slice.len,
                dat: slice.dat.to_vec(),
            })
        }
    }
}

impl TryFrom<Cell> for MarySlice<'_> {
    type Error = ();

    #[inline(always)]
    fn try_from(c: Cell) -> std::result::Result<Self, Self::Error> {
        let step = c.head().as_atom()?.as_u32()?;
        let len = c.tail().as_cell()?.head().as_atom()?.as_u32()?;
        let cell: Cell = c.tail().as_cell()?;
        let dat_noun: Atom = c.tail().as_cell()?.tail().as_atom()?;
        let dat_slice: &[u64] = match dat_noun.as_either() {
            Left(_direct) => unsafe {
                let tail_ptr2 = &(*(cell.to_raw_pointer())).tail as *const Noun;
                std::slice::from_raw_parts(tail_ptr2 as *const u64, (len * step) as usize)
            },
            Right(indirect) => unsafe {
                std::slice::from_raw_parts(
                    indirect.data_pointer() as *mut u64,
                    (len * step) as usize,
                )
            },
        };
        Ok(MarySlice {
            step,
            len,
            dat: dat_slice,
        })
    }
}

impl TryFrom<Noun> for Table<'_> {
    type Error = ();

    fn try_from(n: Noun) -> std::result::Result<Self, Self::Error> {
        if n.is_atom() {
            Err(())
        } else {
            Table::try_from(n.as_cell()?)
        }
    }
}

impl TryFrom<Cell> for Table<'_> {
    type Error = ();

    #[inline(always)]
    fn try_from(c: Cell) -> std::result::Result<Self, Self::Error> {
        let full_width = c.head().as_atom()?.as_u32()?;
        let mary_cell = c.tail().as_cell()?;
        let mary = MarySlice::try_from(mary_cell)?;

        Ok(Table {
            num_cols: full_width,
            mary,
        })
    }
}

// TODO: use Ares::noun::Result or Error somehow for the methods that
// convert our structs from nouns
impl TryFrom<Noun> for BPolySlice<'_> {
    type Error = JetErr;

    #[inline(always)]
    fn try_from(n: Noun) -> std::result::Result<Self, Self::Error> {
        if n.is_atom() {
            Err(BAIL_FAIL)
        } else {
            BPolySlice::try_from(n.as_cell()?)
        }
    }
}

impl TryFrom<Noun> for FPolySlice<'_> {
    type Error = JetErr;

    #[inline(always)]
    fn try_from(n: Noun) -> std::result::Result<Self, Self::Error> {
        if n.is_atom() {
            Err(BAIL_FAIL)
        } else {
            FPolySlice::try_from(n.as_cell()?)
        }
    }
}

impl TryFrom<&Noun> for FPolySlice<'_> {
    type Error = JetErr;

    #[inline(always)]
    fn try_from(n: &Noun) -> std::result::Result<Self, Self::Error> {
        if n.is_atom() {
            Err(BAIL_FAIL)
        } else {
            FPolySlice::try_from(n.as_cell()?)
        }
    }
}

impl TryFrom<Cell> for BPolySlice<'_> {
    type Error = JetErr;

    #[inline(always)]
    fn try_from(c: Cell) -> std::result::Result<Self, Self::Error> {
        let head = c.head().as_atom();
        let tail = c.tail().as_atom();
        if let (Ok(head), Ok(tail)) = (head, tail) {
            let len32 = head.as_u32()?;
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

impl TryFrom<Cell> for FPolySlice<'_> {
    type Error = JetErr;

    #[inline(always)]
    fn try_from(c: Cell) -> std::result::Result<Self, Self::Error> {
        let head = c.head().as_atom();
        let tail = c.tail().as_atom();
        if let (Ok(head), Ok(tail)) = (head, tail) {
            let len32 = head.as_u32()?;
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

impl TryFrom<Cell> for FPolyVec {
    type Error = JetErr;

    #[inline(always)]
    fn try_from(c: Cell) -> std::result::Result<Self, Self::Error> {
        let head = c.head().as_atom();
        let tail = c.tail().as_atom();
        if let (Ok(head), Ok(tail)) = (head, tail) {
            let len32 = head.as_u32()?;
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

impl TryFrom<Cell> for BPolyVec {
    type Error = JetErr;

    #[inline(always)]
    fn try_from(c: Cell) -> std::result::Result<Self, Self::Error> {
        let head = c.head().as_atom();
        let tail = c.tail().as_atom();
        if let (Ok(head), Ok(tail)) = (head, tail) {
            let len32 = head.as_u32()?;
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
    ) -> std::result::Result<Self, noun_serde::NounDecodeError> {
        FPolyVec::try_from(noun.as_cell().expect("not a cell"))
            .map_err(|_| noun_serde::NounDecodeError::FPolyDecodeError)
    }
}

impl NounEncode for FPolyVec {
    fn to_noun<A: nockvm::noun::NounAllocator>(&self, allocator: &mut A) -> nockvm::noun::Noun {
        let (res, res_poly): (IndirectAtom, &mut [Felt]) =
            new_handle_mut_slice(allocator, Some(self.0.len() as usize));
        res_poly.copy_from_slice(&self.0);
        finalize_poly(allocator, Some(self.0.len() as usize), res)
    }
}

impl NounDecode for BPolyVec {
    fn from_noun(
        noun: &nockvm::noun::Noun,
    ) -> std::result::Result<Self, noun_serde::NounDecodeError> {
        BPolyVec::try_from(noun.as_cell().expect("not a cell"))
            .map_err(|_| noun_serde::NounDecodeError::FPolyDecodeError)
    }
}

impl NounEncode for BPolyVec {
    fn to_noun<A: nockvm::noun::NounAllocator>(&self, allocator: &mut A) -> nockvm::noun::Noun {
        let (res, res_poly): (IndirectAtom, &mut [Belt]) =
            new_handle_mut_slice(allocator, Some(self.0.len() as usize));
        res_poly.copy_from_slice(&self.0);
        finalize_poly(allocator, Some(self.0.len() as usize), res)
    }
}
