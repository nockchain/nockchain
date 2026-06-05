use crate::interpreter::Context;
use crate::jets::util::*;
use crate::jets::Result;
use crate::noun::Noun;
use crate::serialization::{cue, jam};

crate::gdb!();

pub fn jet_cue(context: &mut Context, subject: Noun) -> Result {
    Ok(cue(&mut context.stack, slot(subject, 6)?.as_atom()?)?)
}

pub fn jet_jam(context: &mut Context, subject: Noun) -> Result {
    Ok(jam(&mut context.stack, slot(subject, 6)?).as_noun())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jets::util::test::*;
    use crate::noun::{D, T};

    #[test]
    fn test_jam() {
        let c = &mut init_context();

        assert_jet(c, jet_jam, D(0x0), D(0x2));
        assert_jet(c, jet_jam, D(0x1), D(0xc));
        let sam = T(&mut c.stack, &[D(0x0), D(0x0)]);
        assert_jet(c, jet_jam, sam, D(0x29));
        let sam = T(&mut c.stack, &[D(0x1), D(0x2), D(0x3), D(0x0)]);
        assert_jet(c, jet_jam, sam, D(0x2d0c871));
    }

    #[test]
    fn test_cue() {
        let c = &mut init_context();

        assert_jet(c, jet_cue, D(0x2), D(0x0));
        assert_jet(c, jet_cue, D(0xc), D(0x1));
        let res = T(&mut c.stack, &[D(0x0), D(0x0)]);
        assert_jet(c, jet_cue, D(0x29), res);
        let res = T(&mut c.stack, &[D(0x1), D(0x2), D(0x3), D(0x0)]);
        assert_jet(c, jet_cue, D(0x2d0c871), res);
    }
}
