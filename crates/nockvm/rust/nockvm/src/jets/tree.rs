/** Tree jets
 */
use crate::interpreter::Context;
use crate::jets::bits::util::*;
use crate::jets::util::*;
use crate::jets::Result;
use crate::noun::{IndirectAtom, Noun, D};

crate::gdb!();

pub fn jet_cap(_context: &mut Context, subject: Noun) -> Result {
    let arg = slot(subject, 6)?;
    let tom = arg.as_atom()?;
    let met = met(0, tom);

    unsafe {
        if met < 2 {
            Err(BAIL_EXIT)
        } else if *(tom.as_bitslice().get_unchecked(met - 2)) {
            Ok(D(3))
        } else {
            Ok(D(2))
        }
    }
}

pub fn jet_mas(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let tom = slot(subject, 6)?.as_atom()?;
    let met = met(0, tom);

    if met < 2 {
        Err(BAIL_EXIT)
    } else {
        let out_bits = met - 1;
        let out_words = (out_bits + 63) >> 6;
        let (mut indirect_out, out_bs) =
            unsafe { IndirectAtom::new_raw_mut_bitslice(stack, out_words) };
        out_bs.set(met - 2, true); // Set MSB
        if met > 2 {
            out_bs[0..(met - 2)].copy_from_bitslice(&tom.as_bitslice()[0..(met - 2)]);
        };
        unsafe { Ok(indirect_out.normalize_as_atom().as_noun()) }
    }
}

pub fn jet_peg(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let arg = slot(subject, 6)?;
    let a = slot(arg, 2)?.as_atom()?;
    let b = slot(arg, 3)?.as_atom()?;

    unsafe {
        if a.as_noun().raw_equals(&D(0)) {
            return Err(BAIL_EXIT);
        };

        if b.as_noun().raw_equals(&D(0)) {
            return Err(BAIL_EXIT);
        };
    }

    let a_bits = met(0, a);
    let b_bits = met(0, b);
    let out_bits = a_bits + b_bits - 1;

    let out_words = (out_bits + 63) >> 6; // bits to 8-byte words

    let (mut indirect_out, out_bs) =
        unsafe { IndirectAtom::new_raw_mut_bitslice(stack, out_words) };

    out_bs[0..b_bits - 1].copy_from_bitslice(&b.as_bitslice()[0..b_bits - 1]);
    out_bs[b_bits - 1..out_bits].copy_from_bitslice(&a.as_bitslice()[0..a_bits]);

    unsafe { Ok(indirect_out.normalize_as_atom().as_noun()) }
}

#[cfg(test)]
mod tests {
    use ibig::ubig;

    use super::*;
    use crate::jets::util::test::*;
    use crate::mem::NockStack;
    use crate::noun::{Noun, D, DIRECT_MAX};

    fn atom_0(_stack: &mut NockStack) -> Noun {
        D(0x0)
    }

    fn atom_1(_stack: &mut NockStack) -> Noun {
        D(0x1)
    }

    fn atom_62(_stack: &mut NockStack) -> Noun {
        D(0x3fffffffffffffff)
    }

    fn atom_63(_stack: &mut NockStack) -> Noun {
        D(0x4000000000000000)
    }

    fn atom_64(stack: &mut NockStack) -> Noun {
        A(stack, &ubig!(_0x8000000000000000))
    }

    fn atom_65(stack: &mut NockStack) -> Noun {
        A(stack, &ubig!(_0x10000000000000000))
    }

    fn atom_66(stack: &mut NockStack) -> Noun {
        A(stack, &ubig!(_0x20000000000000000))
    }

    fn pos_2(_stack: &mut NockStack) -> Noun {
        D(2)
    }

    fn pos_3(_stack: &mut NockStack) -> Noun {
        D(3)
    }

    fn pos_4(_stack: &mut NockStack) -> Noun {
        D(4)
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_cap() {
        let c = &mut init_context();

        assert_jet_err(c, jet_cap, D(0), BAIL_EXIT);
        assert_jet_err(c, jet_cap, D(1), BAIL_EXIT);

        assert_jet(c, jet_cap, D(2), D(2));
        assert_jet(c, jet_cap, D(3), D(3));
        assert_jet(c, jet_cap, D(4), D(2));
        assert_jet(c, jet_cap, D(5), D(2));
        assert_jet(c, jet_cap, D(6), D(3));
        assert_jet(c, jet_cap, D(7), D(3));
        assert_jet(c, jet_cap, D(8), D(2));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_mas() {
        let c = &mut init_context();
        let a63 = atom_63(&mut c.stack);
        let a64 = atom_64(&mut c.stack);
        let a65 = atom_65(&mut c.stack);
        let a66 = atom_66(&mut c.stack);

        // Test invalid input
        assert_jet_err(c, jet_mas, D(0), BAIL_EXIT);
        assert_jet_err(c, jet_mas, D(1), BAIL_EXIT);

        // Test direct
        assert_jet(c, jet_mas, D(2), D(1));
        assert_jet(c, jet_mas, D(3), D(1));
        assert_jet(c, jet_mas, D(4), D(2));
        assert_jet(c, jet_mas, D(5), D(3));
        assert_jet(c, jet_mas, D(6), D(2));
        assert_jet(c, jet_mas, D(7), D(3));
        assert_jet(c, jet_mas, D(8), D(4));

        // Test indirect
        assert_jet(c, jet_mas, a64, a63);
        assert_jet(c, jet_mas, a65, a64);
        assert_jet(c, jet_mas, a66, a65);

        // Test allocation
        assert_jet_size(c, jet_mas, D(2), 1_usize);
        assert_jet_size(c, jet_mas, a65, 1_usize);
        assert_jet_size(c, jet_mas, a66, 2_usize);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_peg() {
        let c = &mut init_context();

        assert_common_jet_err(c, jet_peg, &[atom_0, atom_1], BAIL_EXIT);
        assert_common_jet_err(c, jet_peg, &[atom_1, atom_0], BAIL_EXIT);

        // Test direct
        assert_common_jet_noun(c, jet_peg, &[pos_2, pos_3], D(5));
        assert_common_jet_noun(c, jet_peg, &[pos_4, pos_4], D(16));

        // Test direct with overflow.
        assert_common_jet_noun(c, jet_peg, &[atom_62, pos_3], D(DIRECT_MAX));
        assert_common_jet(c, jet_peg, &[pos_2, atom_63], ubig!(0x8000000000000000));

        // Test indirect.
        assert_common_jet(c, jet_peg, &[atom_65, atom_1], ubig!(_0x10000000000000000));
        assert_common_jet(c, jet_peg, &[atom_1, atom_65], ubig!(_0x10000000000000000));
        assert_common_jet(
            c,
            jet_peg,
            &[atom_65, atom_65],
            ubig!(_0x100000000000000000000000000000000),
        );

        // Test allocation
        assert_common_jet_size(c, jet_peg, &[atom_65, atom_1], 2_usize);
        assert_common_jet_size(c, jet_peg, &[atom_1, atom_65], 2_usize);
        assert_common_jet_size(c, jet_peg, &[atom_65, atom_65], 3_usize);
    }
}
