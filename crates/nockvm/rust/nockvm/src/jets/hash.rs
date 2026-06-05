/** Hash jets
 */
use crate::interpreter::Context;
use crate::jets::util::*;
use crate::jets::Result;
use crate::mug::mug;
use crate::noun::Noun;

crate::gdb!();

pub fn jet_mug(context: &mut Context, subject: Noun) -> Result {
    let arg = slot(subject, 6)?;
    Ok(mug(&mut context.stack, arg).as_noun())
}

#[cfg(test)]
mod tests {
    use ibig::ubig;

    use super::*;
    use crate::jets::util::test::{assert_jet, init_context, A};
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

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_mug() {
        let c = &mut init_context();
        let (a0, a24, a63, a96, a128) = atoms(&mut c.stack);

        assert_jet(c, jet_mug, a0, D(0x79ff04e8));
        assert_jet(c, jet_mug, a24, D(0x69d59d90));
        assert_jet(c, jet_mug, a63, D(0x7a9f252e));
        assert_jet(c, jet_mug, a96, D(0x2aa4c8fb));
        assert_jet(c, jet_mug, a128, D(0x44fb2c0c));
        let sam = T(&mut c.stack, &[a128, a128]);
        assert_jet(c, jet_mug, sam, D(0x61c0ea5c));
        let sam = T(&mut c.stack, &[a96, a128]);
        assert_jet(c, jet_mug, sam, D(0x20fb143f));
        let sam = T(&mut c.stack, &[a0, a0]);
        assert_jet(c, jet_mug, sam, D(0x192f5588));
        let sam = T(&mut c.stack, &[a0, a24, a63, a96, a128]);
        let sam = T(&mut c.stack, &[sam, a0, a24, a63, a96, a128]);
        let sam = T(&mut c.stack, &[sam, a0, a24, a63, a96, a128]);
        assert_jet(c, jet_mug, sam, D(0x7543cac7));
    }
}
