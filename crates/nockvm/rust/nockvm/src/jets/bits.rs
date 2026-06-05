use std::cmp;
use std::cmp::{max, min};

/** Bit arithmetic & logic jets
 */
use crate::interpreter::Context;
use crate::jets::util::*;
use crate::jets::Result;
use crate::mem::NockStack;
use crate::noun::{IndirectAtom, Noun, D};

crate::gdb!();

/*
 * Bit arithmetic
 */

pub fn jet_bex(context: &mut Context, subject: Noun) -> Result {
    let arg = slot(subject, 6)?.as_direct()?.data() as usize;
    Ok(util::bex(&mut context.stack, arg).as_noun())
}

pub fn jet_can(context: &mut Context, subject: Noun) -> Result {
    let arg = slot(subject, 6)?;
    let bloq = bloq(slot(arg, 2)?)?;
    let original_list = slot(arg, 3)?;

    util::can(&mut context.stack, bloq, original_list)
}

pub fn jet_cat(context: &mut Context, subject: Noun) -> Result {
    let arg = slot(subject, 6)?;
    let bloq = bloq(slot(arg, 2)?)?;
    let a = slot(arg, 6)?.as_atom()?;
    let b = slot(arg, 7)?.as_atom()?;

    let len_a = util::met(bloq, a);
    let len_b = util::met(bloq, b);
    let new_len = bite_to_word(bloq, checked_add(len_a, len_b)?)?;
    if new_len == 0 {
        Ok(a.as_noun())
    } else {
        unsafe {
            let (mut new_indirect, new_slice) =
                IndirectAtom::new_raw_mut_bitslice(&mut context.stack, new_len);
            chop(bloq, 0, len_a, 0, new_slice, a.as_bitslice())?;
            chop(bloq, 0, len_b, len_a, new_slice, b.as_bitslice())?;
            Ok(new_indirect.normalize_as_atom().as_noun())
        }
    }
}

pub fn jet_cut(context: &mut Context, subject: Noun) -> Result {
    let arg = slot(subject, 6)?;
    let bloq = bloq(slot(arg, 2)?)?;
    let start = slot(arg, 12)?.as_direct()?.data() as usize;
    let run = slot(arg, 13)?.as_direct()?.data() as usize;
    let atom = slot(arg, 7)?.as_atom()?;

    if run == 0 {
        return Ok(D(0));
    }

    let new_indirect = unsafe {
        let (mut new_indirect, new_slice) =
            IndirectAtom::new_raw_mut_bitslice(&mut context.stack, bite_to_word(bloq, run)?);
        chop(bloq, start, run, 0, new_slice, atom.as_bitslice())?;
        new_indirect.normalize_as_atom()
    };
    Ok(new_indirect.as_noun())
}

pub fn jet_sew(context: &mut Context, subject: Noun) -> Result {
    let sam = slot(subject, 6)?;
    let bloq = bloq(slot(sam, 2)?)?;
    let e = slot(sam, 7)?.as_atom()?;

    let bcd = slot(sam, 6)?;
    let offset = slot(bcd, 2)?.as_atom()?.as_u64()? as usize;
    let step = slot(bcd, 6)?.as_atom()?.as_u64()? as usize;

    if step == 0 {
        return Ok(e.as_noun());
    }

    let donor = slot(bcd, 7)?.as_atom()?;

    let len_d = util::met(bloq, donor);
    let len_e = util::met(bloq, e);
    let new_len = max(checked_add(step, offset)?, len_e);

    unsafe {
        let (mut dest_indirect, dest) =
            IndirectAtom::new_raw_mut_bitslice(&mut context.stack, bite_to_word(bloq, new_len)?);

        chop(bloq, 0, len_e, 0, dest, e.as_bitslice())?;

        let (_, lead) =
            IndirectAtom::new_raw_mut_bitslice(&mut context.stack, bite_to_word(bloq, step)?);

        chop(bloq, 0, min(step, len_d), 0, lead, donor.as_bitslice())?;

        chop(bloq, 0, step, offset, dest, lead)?;
        Ok(dest_indirect.normalize_as_atom().as_noun())
    }
}

pub fn jet_end(context: &mut Context, subject: Noun) -> Result {
    let arg = slot(subject, 6)?;
    let (bloq, step) = bite(slot(arg, 2)?)?;
    let a = slot(arg, 3)?.as_atom()?;

    if step == 0 {
        Ok(D(0))
    } else if step >= util::met(bloq, a) {
        Ok(a.as_noun())
    } else {
        unsafe {
            let (mut new_indirect, new_slice) =
                IndirectAtom::new_raw_mut_bitslice(&mut context.stack, bite_to_word(bloq, step)?);
            chop(bloq, 0, step, 0, new_slice, a.as_bitslice())?;
            Ok(new_indirect.normalize_as_atom().as_noun())
        }
    }
}

pub fn jet_lsh(context: &mut Context, subject: Noun) -> Result {
    let arg = slot(subject, 6)?;
    let (bloq, step) = bite(slot(arg, 2)?)?;
    let a = slot(arg, 3)?.as_atom()?;

    util::lsh(&mut context.stack, bloq, step, a)
}

pub fn jet_met(_context: &mut Context, subject: Noun) -> Result {
    let arg = slot(subject, 6)?;
    let bloq = bloq(slot(arg, 2)?)?;
    let a = slot(arg, 3)?.as_atom()?;

    Ok(D(util::met(bloq, a) as u64))
}

pub fn jet_rap(context: &mut Context, subject: Noun) -> Result {
    let arg = slot(subject, 6)?;
    let bloq = bloq(slot(arg, 2)?)?;
    let original_list = slot(arg, 3)?;
    Ok(util::rap(&mut context.stack, bloq, original_list)?.as_noun())
}

pub fn jet_rep(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;

    let arg = slot(subject, 6)?;
    let arg2 = slot(arg, 2)?;
    let arg3 = slot(arg, 3)?;

    rep(stack, arg2, arg3)
}

pub fn rep(stack: &mut NockStack, a: Noun, b: Noun) -> Result {
    let (bloq, step) = bite(a)?;
    let original_list = b;

    let mut len = 0usize;
    let mut list = original_list;
    loop {
        if unsafe { list.raw_equals(&D(0)) } {
            break;
        }

        let cell = list.as_cell()?;

        len = checked_add(len, step)?;
        list = cell.tail();
    }

    if len == 0 {
        Ok(D(0))
    } else {
        unsafe {
            let (mut new_indirect, new_slice) =
                IndirectAtom::new_raw_mut_bitslice(stack, bite_to_word(bloq, len)?);
            let mut pos = 0;
            let mut list = original_list;
            loop {
                if list.raw_equals(&D(0)) {
                    break;
                }

                let cell = list.as_cell()?;
                let atom = cell.head().as_atom()?;
                chop(bloq, 0, step, pos, new_slice, atom.as_bitslice())?;

                pos += step;
                list = cell.tail();
            }
            Ok(new_indirect.normalize_as_atom().as_noun())
        }
    }
}

pub fn jet_rev(context: &mut Context, subject: Noun) -> Result {
    let arg = slot(subject, 6)?;
    let boz = slot(arg, 2)?.as_atom()?.as_direct()?.data();

    if boz >= 64 {
        return Err(BAIL_EXIT);
    }

    let boz = boz as usize;
    let len = slot(arg, 6)?.as_atom()?.as_direct()?.data();
    let dat = slot(arg, 7)?.as_atom()?;
    let bits = len << boz;

    let src = dat.as_bitslice();
    let (mut output, dest) =
        unsafe { IndirectAtom::new_raw_mut_bitslice(&mut context.stack, bits as usize) };

    let len = len as usize;
    let total_len = len << boz;

    for (start, end) in (0..len)
        .map(|b| (b << boz, (b + 1) << boz))
        .filter(|(start, _)| (total_len - start) <= src.len())
    {
        dest[start..end].copy_from_bitslice(&src[(total_len - end)..(total_len - start)]);
    }

    Ok(unsafe { output.normalize_as_atom() }.as_noun())
}

pub fn jet_rip(context: &mut Context, subject: Noun) -> Result {
    let arg = slot(subject, 6)?;
    let (bloq, step) = bite(slot(arg, 2)?)?;
    let atom = slot(arg, 3)?.as_atom()?;
    util::rip(&mut context.stack, bloq, step, atom)
}

pub fn jet_rsh(context: &mut Context, subject: Noun) -> Result {
    let arg = slot(subject, 6)?;
    let (bloq, step) = bite(slot(arg, 2)?)?;
    let a = slot(arg, 3)?.as_atom()?;

    let len = util::met(bloq, a);
    if step >= len {
        return Ok(D(0));
    }

    let new_size = bits_to_word(checked_sub(a.bit_size(), checked_left_shift(bloq, step)?)?)?;
    unsafe {
        let (mut atom, dest) = IndirectAtom::new_raw_mut_bitslice(&mut context.stack, new_size);
        chop(bloq, step, len - step, 0, dest, a.as_bitslice())?;
        Ok(atom.normalize_as_atom().as_noun())
    }
}

pub fn jet_xeb(_context: &mut Context, subject: Noun) -> Result {
    let sam = slot(subject, 6)?;
    let a = slot(sam, 1)?.as_atom()?;
    Ok(D(util::met(0, a) as u64))
}

/*
 * Bit logic
 */

pub fn jet_con(context: &mut Context, subject: Noun) -> Result {
    let arg = slot(subject, 6)?;
    let a = slot(arg, 2)?.as_atom()?;
    let b = slot(arg, 3)?.as_atom()?;

    Ok(util::con(&mut context.stack, a, b).as_noun())
}

pub fn jet_dis(context: &mut Context, subject: Noun) -> Result {
    let arg = slot(subject, 6)?;
    let a = slot(arg, 2)?.as_atom()?;
    let b = slot(arg, 3)?.as_atom()?;

    let new_size = cmp::max(a.size(), b.size());

    unsafe {
        let (mut atom, dest) = IndirectAtom::new_raw_mut_bitslice(&mut context.stack, new_size);
        let a_bit = a.as_bitslice();
        dest[..a_bit.len()].copy_from_bitslice(a_bit);
        *dest &= b.as_bitslice();
        Ok(atom.normalize_as_atom().as_noun())
    }
}

pub fn jet_mix(context: &mut Context, subject: Noun) -> Result {
    let arg = slot(subject, 6)?;
    let a = slot(arg, 2)?.as_atom()?;
    let b = slot(arg, 3)?.as_atom()?;

    let new_size = cmp::max(a.size(), b.size());

    unsafe {
        let (mut atom, dest) = IndirectAtom::new_raw_mut_bitslice(&mut context.stack, new_size);
        let a_bit = a.as_bitslice();
        dest[..a_bit.len()].copy_from_bitslice(a_bit);
        *dest ^= b.as_bitslice();
        Ok(atom.normalize_as_atom().as_noun())
    }
}

pub mod util {
    use std::{cmp, result};

    use crate::jets::util::*;
    use crate::jets::{JetErr, Result};
    use crate::mem::NockStack;
    use crate::noun::{Atom, Cell, DirectAtom, IndirectAtom, Noun, D};

    /// Binary exponent
    pub fn bex(stack: &mut NockStack, arg: usize) -> Atom {
        unsafe {
            if arg < 63 {
                DirectAtom::new_unchecked(1 << arg).as_atom()
            } else {
                let (mut atom, dest) = IndirectAtom::new_raw_mut_bitslice(stack, (arg + 7) >> 3);
                dest.set(arg, true);
                atom.normalize_as_atom()
            }
        }
    }

    pub fn lsh(stack: &mut NockStack, bloq: usize, step: usize, a: Atom) -> Result {
        let len = met(bloq, a);
        if len == 0 {
            return Ok(D(0));
        }

        let new_size = bits_to_word(checked_add(a.bit_size(), checked_left_shift(bloq, step)?)?)?;
        unsafe {
            let (mut atom, dest) = IndirectAtom::new_raw_mut_bitslice(stack, new_size);
            chop(bloq, 0, len, step, dest, a.as_bitslice())?;
            Ok(atom.normalize_as_atom().as_noun())
        }
    }

    pub fn can(stack: &mut NockStack, bloq: usize, original_list: Noun) -> Result {
        let mut len = 0usize;
        let mut list = original_list;
        loop {
            if unsafe { list.raw_equals(&D(0)) } {
                break;
            }

            let cell = list.as_cell()?;
            let item = cell.head().as_cell()?;
            let step = item.head().as_direct()?.data() as usize;

            len = checked_add(len, step)?;
            list = cell.tail();
        }

        if len == 0 {
            Ok(D(0))
        } else {
            unsafe {
                let (mut new_indirect, new_slice) =
                    IndirectAtom::new_raw_mut_bitslice(stack, bite_to_word(bloq, len)?);
                let mut pos = 0;
                let mut list = original_list;
                loop {
                    if list.raw_equals(&D(0)) {
                        break;
                    }

                    let cell = list.as_cell()?;
                    let item = cell.head().as_cell()?;
                    let step = item.head().as_direct()?.data() as usize;
                    let atom = item.tail().as_atom()?;
                    chop(bloq, 0, step, pos, new_slice, atom.as_bitslice())?;

                    pos += step;
                    list = cell.tail();
                }
                Ok(new_indirect.normalize_as_atom().as_noun())
            }
        }
    }

    /// Measure the number of bloqs in an atom
    pub fn met(bloq: usize, a: Atom) -> usize {
        if unsafe { a.as_noun().raw_equals(&D(0)) } {
            0
        } else if bloq < 6 {
            (a.bit_size() + ((1 << bloq) - 1)) >> bloq
        } else {
            let bloq_word = bloq - 6;
            (a.size() + ((1 << bloq_word) - 1)) >> bloq_word
        }
    }

    pub fn rip(stack: &mut NockStack, bloq: usize, step: usize, atom: Atom) -> Result {
        let len = met(bloq, atom).div_ceil(step);
        let mut list = D(0);
        for i in (0..len).rev() {
            let new_atom = unsafe {
                let (mut new_indirect, new_slice) =
                    IndirectAtom::new_raw_mut_bitslice(stack, step << bloq);
                chop(bloq, i * step, step, 0, new_slice, atom.as_bitslice())?;
                new_indirect.normalize_as_atom()
            };
            list = Cell::new(stack, new_atom.as_noun(), list).as_noun();
        }

        Ok(list)
    }

    /// Binary OR
    pub fn con(stack: &mut NockStack, a: Atom, b: Atom) -> Atom {
        let new_size = cmp::max(a.size(), b.size());

        unsafe {
            let (mut atom, dest) = IndirectAtom::new_raw_mut_bitslice(stack, new_size);
            let a_bit = a.as_bitslice();
            dest[..a_bit.len()].copy_from_bitslice(a_bit);
            *dest |= b.as_bitslice();
            atom.normalize_as_atom()
        }
    }

    pub fn rap(
        stack: &mut NockStack,
        bloq: usize,
        original_list: Noun,
    ) -> result::Result<Atom, JetErr> {
        let mut len = 0usize;
        let mut list = original_list;
        loop {
            if unsafe { list.raw_equals(&D(0)) } {
                break;
            }

            let cell = list.as_cell()?;

            len = checked_add(len, met(bloq, cell.head().as_atom()?))?;
            list = cell.tail();
        }

        if len == 0 {
            Ok(Atom::new(stack, 0))
        } else {
            unsafe {
                let (mut new_indirect, new_slice) =
                    IndirectAtom::new_raw_mut_bitslice(stack, bite_to_word(bloq, len)?);
                let mut pos = 0;
                let mut list = original_list;

                loop {
                    if list.raw_equals(&D(0)) {
                        break;
                    }

                    let cell = list.as_cell()?;
                    let atom = cell.head().as_atom()?;
                    let step = met(bloq, atom);
                    chop(bloq, 0, step, pos, new_slice, atom.as_bitslice())?;

                    pos += step;
                    list = cell.tail();
                }

                Ok(new_indirect.normalize_as_atom())
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use ibig::ubig;

        use super::*;
        use crate::jets::util::test::A;
        use crate::noun::D;

        fn init_stack() -> NockStack {
            NockStack::new(8 << 10 << 10, 0)
        }

        #[test]
        #[cfg_attr(miri, ignore)]
        fn test_met() {
            let s = &mut init_stack();

            let a = A(s, &ubig!(0xdeadbeef12345678fedcba9876543210))
                .as_atom()
                .unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
            assert_eq!(met(0, a), 128);
            assert_eq!(met(1, a), 64);
            assert_eq!(met(2, a), 32);
            assert_eq!(met(3, a), 16);
            assert_eq!(met(4, a), 8);
            assert_eq!(met(5, a), 4);
            assert_eq!(met(6, a), 2);
            assert_eq!(met(7, a), 1);
            assert_eq!(met(8, a), 1);

            let a = D(0x7fffffffffffffff).as_atom().unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            assert_eq!(met(0, a), 63);
            assert_eq!(met(1, a), 32);
            assert_eq!(met(2, a), 16);
            assert_eq!(met(3, a), 8);
            assert_eq!(met(4, a), 4);
            assert_eq!(met(5, a), 2);
            assert_eq!(met(6, a), 1);
            assert_eq!(met(7, a), 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use ibig::ubig;

    use super::*;
    use crate::jets::util::test::*;
    use crate::mem::NockStack;
    use crate::noun::{Noun, D, T};

    fn atoms(s: &mut NockStack) -> (Noun, Noun, Noun, Noun, Noun) {
        (atom_0(s), atom_24(s), atom_63(s), atom_96(s), atom_128(s))
    }

    fn atom_0(_stack: &mut NockStack) -> Noun {
        D(0)
    }

    fn atom_24(_stack: &mut NockStack) -> Noun {
        D(0x876543)
    }

    fn atom_63(_stack: &mut NockStack) -> Noun {
        D(0x7fffffffffffffff)
    }

    fn atom_96(stack: &mut NockStack) -> Noun {
        A(stack, &ubig!(0xfaceb00c15deadbeef123456))
    }

    fn atom_128(stack: &mut NockStack) -> Noun {
        A(stack, &ubig!(0xdeadbeef12345678fedcba9876543210))
    }

    /*
     * Bit arithmetic
     */

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_bex() {
        let c = &mut init_context();

        assert_jet(c, jet_bex, D(0), D(1));
        assert_jet(c, jet_bex, D(5), D(32));
        assert_jet(c, jet_bex, D(62), D(0x4000000000000000));
        assert_jet_ubig(
            c,
            jet_bex,
            D(256),
            ubig!(_0x10000000000000000000000000000000000000000000000000000000000000000),
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_can() {
        let c = &mut init_context();

        let (a0, _a24, _a63, _a96, a128) = atoms(&mut c.stack);
        let bloq0 = D(0);
        let bloq3 = D(3);
        let bloq4 = D(4);
        let sam = T(&mut c.stack, &[bloq0, D(0)]);
        assert_jet(c, jet_can, sam, D(0));
        let sam = T(&mut c.stack, &[bloq3, D(0)]);
        assert_jet(c, jet_can, sam, D(0));
        let run1 = T(&mut c.stack, &[D(0), a0]);
        let run2 = T(&mut c.stack, &[D(1), a0]);
        let run3 = T(&mut c.stack, &[D(2), a0]);
        let sam = T(&mut c.stack, &[bloq0, run1, run2, run3, D(0)]);
        assert_jet(c, jet_can, sam, D(0));
        let sam = T(&mut c.stack, &[bloq3, run1, run2, run3, D(0)]);
        assert_jet(c, jet_can, sam, D(0));
        let run1 = T(&mut c.stack, &[D(1), a128]);
        let run2 = T(&mut c.stack, &[D(3), a0]);
        let sam = T(&mut c.stack, &[bloq3, run1, run2, D(0)]);
        assert_jet(c, jet_can, sam, D(0x10));
        let run1 = T(&mut c.stack, &[D(3), a0]);
        let run2 = T(&mut c.stack, &[D(1), a128]);
        let sam = T(&mut c.stack, &[bloq3, run1, run2, D(0)]);
        assert_jet(c, jet_can, sam, D(0x10000000));
        let run1 = T(&mut c.stack, &[D(8), D(0xfe)]);
        let run2 = T(&mut c.stack, &[D(4), D(0xa)]);
        let run3 = T(&mut c.stack, &[D(0), D(0xbbbb)]);
        let run4 = T(&mut c.stack, &[D(1), D(0)]);
        let run5 = T(&mut c.stack, &[D(1), D(0)]);
        let run6 = T(&mut c.stack, &[D(1), D(1)]);
        let run7 = T(&mut c.stack, &[D(1), D(1)]);
        let sam = T(
            &mut c.stack,
            &[bloq0, run1, run2, run3, run4, run5, run6, run7, D(0)],
        );
        assert_jet(c, jet_can, sam, D(0xcafe));
        let run1 = T(&mut c.stack, &[D(1), D(0xfe)]);
        let run2 = T(&mut c.stack, &[D(1), D(0xca)]);
        let sam = T(&mut c.stack, &[bloq4, run1, run2, D(0)]);
        assert_jet(c, jet_can, sam, D(0xca00fe));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_cat() {
        let c = &mut init_context();

        let (a0, a24, _a63, _a96, a128) = atoms(&mut c.stack);
        let bloq0 = D(0);
        let bloq3 = D(3);
        let bloq4 = D(4);
        let sam = T(&mut c.stack, &[bloq0, a0, a0]);
        assert_jet(c, jet_cat, sam, D(0));
        let sam = T(&mut c.stack, &[bloq3, a0, a0]);
        assert_jet(c, jet_cat, sam, D(0));
        let sam = T(&mut c.stack, &[bloq0, a24, a128]);
        let res = A(
            &mut c.stack,
            &ubig!(_0xdeadbeef12345678fedcba9876543210876543),
        );
        assert_jet(c, jet_cat, sam, res);
        let sam = T(&mut c.stack, &[bloq3, a24, a128]);
        let res = A(
            &mut c.stack,
            &ubig!(_0xdeadbeef12345678fedcba9876543210876543),
        );
        assert_jet(c, jet_cat, sam, res);
        let sam = T(&mut c.stack, &[bloq4, a24, a128]);
        let res = A(
            &mut c.stack,
            &ubig!(_0xdeadbeef12345678fedcba987654321000876543),
        );
        assert_jet(c, jet_cat, sam, res);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_cut() {
        let c = &mut init_context();

        let (_a0, a24, _a63, a96, a128) = atoms(&mut c.stack);
        let run = T(&mut c.stack, &[D(0), D(0)]);
        let sam = T(&mut c.stack, &[D(0), run, a24]);
        assert_jet(c, jet_cut, sam, D(0));
        let run = T(&mut c.stack, &[D(0), D(5)]);
        let sam = T(&mut c.stack, &[D(0), run, a24]);
        assert_jet(c, jet_cut, sam, D(0x3));
        let run = T(&mut c.stack, &[D(4), D(6)]);
        let sam = T(&mut c.stack, &[D(3), run, a96]);
        assert_jet(c, jet_cut, sam, D(0xb00c15deadbe));
        let run = T(&mut c.stack, &[D(4), D(1)]);
        let sam = T(&mut c.stack, &[D(4), run, a24]);
        assert_jet(c, jet_cut, sam, D(0));
        let run = T(&mut c.stack, &[D(2), D(10)]);
        let sam = T(&mut c.stack, &[D(4), run, a128]);
        let res = A(&mut c.stack, &ubig!(0xdeadbeef12345678fedcba98));
        assert_jet(c, jet_cut, sam, res);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_end() {
        let c = &mut init_context();

        let (a0, a24, _a63, a96, a128) = atoms(&mut c.stack);
        let sam = T(&mut c.stack, &[a0, a24]);
        assert_jet(c, jet_end, sam, D(0x1));
        let sam = T(&mut c.stack, &[D(3), a24]);
        assert_jet(c, jet_end, sam, D(0x43));
        let sam = T(&mut c.stack, &[D(7), a24]);
        assert_jet(c, jet_end, sam, a24);
        let sam = T(&mut c.stack, &[D(6), a128]);
        let res = A(&mut c.stack, &ubig!(0xfedcba9876543210));
        assert_jet(c, jet_end, sam, res);

        let bit = T(&mut c.stack, &[D(0), D(5)]);
        let sam = T(&mut c.stack, &[bit, a24]);
        assert_jet(c, jet_end, sam, D(0x3));
        let bit = T(&mut c.stack, &[D(4), D(6)]);
        let sam = T(&mut c.stack, &[bit, a96]);
        assert_jet(c, jet_end, sam, a96);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_lsh() {
        let c = &mut init_context();

        let (_, a24, _a63, a96, a128) = atoms(&mut c.stack);
        assert_common_jet_noun(c, jet_lsh, &[atom_0, atom_24], D(0x10eca86));
        let sam = T(&mut c.stack, &[D(3), a24]);
        assert_jet(c, jet_lsh, sam, D(0x87654300));
        let sam = T(&mut c.stack, &[D(7), a24]);
        let res = A(
            &mut c.stack,
            &ubig!(_0x87654300000000000000000000000000000000),
        );
        assert_jet(c, jet_lsh, sam, res);
        let sam = T(&mut c.stack, &[D(6), a128]);
        let res = A(
            &mut c.stack,
            &ubig!(_0xdeadbeef12345678fedcba98765432100000000000000000),
        );
        assert_jet(c, jet_lsh, sam, res);

        let bit = T(&mut c.stack, &[D(0), D(5)]);
        let sam = T(&mut c.stack, &[bit, a24]);
        assert_jet(c, jet_lsh, sam, D(0x10eca860));
        let bit = T(&mut c.stack, &[D(4), D(6)]);
        let sam = T(&mut c.stack, &[bit, a96]);
        let res = A(
            &mut c.stack,
            &ubig!(_0xfaceb00c15deadbeef123456000000000000000000000000),
        );
        assert_jet(c, jet_lsh, sam, res);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_met() {
        let c = &mut init_context();

        let (a0, a24, _a63, _a96, a128) = atoms(&mut c.stack);
        let sam = T(&mut c.stack, &[a0, a0]);
        assert_jet(c, jet_met, sam, D(0));
        let sam = T(&mut c.stack, &[a0, a24]);
        assert_jet(c, jet_met, sam, D(24));
        let sam = T(&mut c.stack, &[D(3), a24]);
        assert_jet(c, jet_met, sam, D(3));
        let sam = T(&mut c.stack, &[D(1), a128]);
        assert_jet(c, jet_met, sam, D(64));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_rap() {
        let c = &mut init_context();

        let bloq0 = D(0);
        let bloq2 = D(2);
        let bloq3 = D(3);
        let empty_list = D(0);
        let zero_list = T(&mut c.stack, &[D(0), D(0), D(0)]);
        let test_list = T(&mut c.stack, &[D(0xe), D(0xf), D(0xa), D(0xc), D(0)]);
        let wide_list = T(&mut c.stack, &[D(0xafe), D(0xc), D(0)]);
        let sam = T(&mut c.stack, &[bloq0, empty_list]);
        assert_jet(c, jet_rap, sam, D(0));
        let sam = T(&mut c.stack, &[bloq0, zero_list]);
        assert_jet(c, jet_rap, sam, D(0));
        let sam = T(&mut c.stack, &[bloq3, zero_list]);
        assert_jet(c, jet_rap, sam, D(0));
        let sam = T(&mut c.stack, &[bloq0, test_list]);
        assert_jet(c, jet_rap, sam, D(0xcafe));
        let sam = T(&mut c.stack, &[bloq2, test_list]);
        assert_jet(c, jet_rap, sam, D(0xcafe));
        let sam = T(&mut c.stack, &[bloq2, wide_list]);
        assert_jet(c, jet_rap, sam, D(0xcafe));
        let sam = T(&mut c.stack, &[bloq3, test_list]);
        let res = A(&mut c.stack, &ubig!(0xc0a0f0e));
        assert_jet(c, jet_rap, sam, res);
        let sam = T(&mut c.stack, &[bloq3, wide_list]);
        assert_jet(c, jet_rap, sam, D(0xc0afe));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_rep() {
        let c = &mut init_context();

        let (a0, a24, a63, a96, a128) = atoms(&mut c.stack);
        let sam = T(&mut c.stack, &[D(0), D(0)]);
        assert_jet(c, jet_rep, sam, D(0));
        let bit = T(&mut c.stack, &[D(3), D(2)]);
        let sam = T(&mut c.stack, &[bit, a0, a24, a63, a96, a128, D(0)]);
        let res = A(&mut c.stack, &ubig!(0x32103456ffff65430000));
        assert_jet(c, jet_rep, sam, res);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_rev() {
        let c = &mut init_context();

        let (_a0, a24, _a63, a96, _a128) = atoms(&mut c.stack);
        let sam = T(&mut c.stack, &[D(0), D(60), a24]);
        assert_jet(c, jet_rev, sam, D(0xc2a6e1000000000));
        let test = 0x1234567890123u64;
        let sam = T(&mut c.stack, &[D(3), D(7), D(test)]);
        assert_jet(c, jet_rev, sam, D(test.swap_bytes() >> 8));
        let sam = T(&mut c.stack, &[D(3), D(12), a96]);
        let res = A(&mut c.stack, &ubig!(0x563412efbeadde150cb0cefa));
        assert_jet(c, jet_rev, sam, res);
        let sam = T(&mut c.stack, &[D(3), D(4), D(0)]);
        assert_jet(c, jet_rev, sam, D(0));
        let sam = T(&mut c.stack, &[D(3), D(5), D(0x2)]);
        assert_jet(c, jet_rev, sam, D(0x200000000));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_rip() {
        let c = &mut init_context();

        let (_a0, _a24, _a63, _a96, a128) = atoms(&mut c.stack);
        let sam = T(&mut c.stack, &[D(0), D(0)]);
        assert_jet(c, jet_rip, sam, D(0));
        let bit = T(&mut c.stack, &[D(1), D(2)]);
        let sam = T(&mut c.stack, &[bit, a128]);
        #[rustfmt::skip]
        let res = T(
            &mut c.stack,
            &[
                D(0x0), D(0x1), D(0x2), D(0x3), D(0x4), D(0x5), D(0x6), D(0x7),
                D(0x8), D(0x9), D(0xa), D(0xb), D(0xc), D(0xd), D(0xe), D(0xf),
                D(0x8), D(0x7), D(0x6), D(0x5), D(0x4), D(0x3), D(0x2), D(0x1),
                D(0xf), D(0xe), D(0xe), D(0xb), D(0xd), D(0xa), D(0xe), D(0xd),
                D(0x0),
            ],
        );
        assert_jet(c, jet_rip, sam, res);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_rsh() {
        let c = &mut init_context();

        let (a0, a24, _a63, a96, a128) = atoms(&mut c.stack);
        let sam = T(&mut c.stack, &[a0, a24]);
        assert_jet(c, jet_rsh, sam, D(0x43b2a1));
        let sam = T(&mut c.stack, &[D(3), a24]);
        assert_jet(c, jet_rsh, sam, D(0x8765));
        let sam = T(&mut c.stack, &[D(7), a24]);
        assert_jet(c, jet_rsh, sam, D(0));
        let sam = T(&mut c.stack, &[D(2), a128]);
        let res = A(&mut c.stack, &ubig!(0xdeadbeef12345678fedcba987654321));
        assert_jet(c, jet_rsh, sam, res);
        let sam = T(&mut c.stack, &[D(6), a128]);
        let res = A(&mut c.stack, &ubig!(0xdeadbeef12345678));
        assert_jet(c, jet_rsh, sam, res);

        let bit = T(&mut c.stack, &[D(0), D(5)]);
        let sam = T(&mut c.stack, &[bit, a24]);
        assert_jet(c, jet_rsh, sam, D(0x43b2a));
        let bit = T(&mut c.stack, &[D(4), D(6)]);
        let sam = T(&mut c.stack, &[bit, a96]);
        assert_jet(c, jet_rsh, sam, D(0));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_sew() {
        let c = &mut init_context();
        let (a0, a24, a63, a96, a128) = atoms(&mut c.stack);

        // 0xfaceb00c15deadbeef123456
        let bcd = T(&mut c.stack, &[D(7), D(2), D(0)]);
        let sam = T(&mut c.stack, &[D(2), bcd, a96]);
        let res = A(&mut c.stack, &ubig!(0xfaceb00c15deadb00f123456));
        assert_jet(c, jet_sew, sam, res);

        //D(0x876543)
        let bcd = T(&mut c.stack, &[D(6), D(3), D(0xbad)]);
        let sam = T(&mut c.stack, &[D(2), bcd, a24]);
        let res = A(&mut c.stack, &ubig!(0xbad876543));
        assert_jet(c, jet_sew, sam, res);

        // D(0xdeadbeef12345678fedcba9876543210)
        let bcd = T(&mut c.stack, &[D(1), D(1), D(0xffff)]);
        let sam = T(&mut c.stack, &[D(4), bcd, a128]);
        let res = A(&mut c.stack, &ubig!(0xdeadbeef12345678fedcba98ffff3210));
        assert_jet(c, jet_sew, sam, res);

        //D(0x7fffffffffffffff)
        let bcd = T(&mut c.stack, &[D(9), D(3), D(0xeeee)]);
        let sam = T(&mut c.stack, &[D(3), bcd, a63]);
        let res = A(&mut c.stack, &ubig!(0xeeee007fffffffffffffff));
        assert_jet(c, jet_sew, sam, res);

        //D(0x876543)
        let bcd = T(&mut c.stack, &[D(7), D(0), D(0xbad)]);
        let sam = T(&mut c.stack, &[D(2), bcd, a24]);
        assert_jet(c, jet_sew, sam, a24);

        // D(0x0)
        let bcd = T(&mut c.stack, &[D(7), D(3), D(0xbad)]);
        let sam = T(&mut c.stack, &[D(2), bcd, a0]);
        let res = A(&mut c.stack, &ubig!(0xbad0000000));
        assert_jet(c, jet_sew, sam, res);
    }

    /*
     * Bit logic
     */

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_con() {
        let c = &mut init_context();

        let (a0, a24, a63, a96, a128) = atoms(&mut c.stack);
        let sam = T(&mut c.stack, &[a0, a0]);
        assert_jet(c, jet_con, sam, D(0));
        let sam = T(&mut c.stack, &[a24, a96]);
        let res = A(&mut c.stack, &ubig!(0xfaceb00c15deadbeef977557));
        assert_jet(c, jet_con, sam, res);
        let sam = T(&mut c.stack, &[a96, a128]);
        let res = A(&mut c.stack, &ubig!(0xdeadbeeffafef67cffdebfbeff563656));
        assert_jet(c, jet_con, sam, res);
        let sam = T(&mut c.stack, &[a24, a63]);
        assert_jet(c, jet_con, sam, a63);
        let sam = T(&mut c.stack, &[a0, a128]);
        assert_jet(c, jet_con, sam, a128);
        let sam = T(&mut c.stack, &[a128, a0]);
        assert_jet(c, jet_con, sam, a128);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_dis() {
        let c = &mut init_context();

        let (a0, a24, a63, a96, a128) = atoms(&mut c.stack);
        let sam = T(&mut c.stack, &[a0, a0]);
        assert_jet(c, jet_dis, sam, D(0));
        let sam = T(&mut c.stack, &[a24, a96]);
        assert_jet(c, jet_dis, sam, D(0x22442));
        let sam = T(&mut c.stack, &[a96, a128]);
        let res = A(&mut c.stack, &ubig!(0x1204100814dca89866103010));
        assert_jet(c, jet_dis, sam, res);
        let sam = T(&mut c.stack, &[a24, a63]);
        assert_jet(c, jet_dis, sam, a24);
        let sam = T(&mut c.stack, &[a0, a128]);
        assert_jet(c, jet_dis, sam, a0);
        let sam = T(&mut c.stack, &[a128, a0]);
        assert_jet(c, jet_dis, sam, a0);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_mix() {
        let c = &mut init_context();

        let (a0, a24, a63, a96, a128) = atoms(&mut c.stack);
        let sam = T(&mut c.stack, &[a0, a0]);
        assert_jet(c, jet_mix, sam, D(0));
        let sam = T(&mut c.stack, &[a24, a96]);
        let res = A(&mut c.stack, &ubig!(0xfaceb00c15deadbeef955115));
        assert_jet(c, jet_mix, sam, res);
        let sam = T(&mut c.stack, &[a96, a128]);
        let res = A(&mut c.stack, &ubig!(0xdeadbeefe8fae674eb02172699460646));
        assert_jet(c, jet_mix, sam, res);
        let sam = T(&mut c.stack, &[a24, a63]);
        let res = A(&mut c.stack, &ubig!(0x7fffffffff789abc));
        assert_jet(c, jet_mix, sam, res);
        let sam = T(&mut c.stack, &[a0, a128]);
        assert_jet(c, jet_mix, sam, a128);
        let sam = T(&mut c.stack, &[a128, a0]);
        assert_jet(c, jet_mix, sam, a128);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_xeb() {
        let c = &mut init_context();

        let (a0, a24, a63, a96, a128) = atoms(&mut c.stack);
        assert_jet(c, jet_xeb, a0, D(0));
        assert_jet(c, jet_xeb, a24, D(24));
        assert_jet(c, jet_xeb, a63, D(63));
        assert_jet(c, jet_xeb, a96, D(96));
        assert_jet(c, jet_xeb, a128, D(128));
    }
}
