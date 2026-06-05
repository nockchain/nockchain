/** Formatting jets
 */
use crate::interpreter::Context;
use crate::jets::util::slot;
use crate::jets::Result;
use crate::noun::Noun;

crate::gdb!();

pub fn jet_scow(context: &mut Context, subject: Noun) -> Result {
    let aura = slot(subject, 12)?.as_direct()?;
    let atom = slot(subject, 13)?.as_atom()?;
    util::scow(&mut context.stack, aura, atom)
}

pub mod util {
    use nockvm_macros::tas;
    use num_traits::identities::Zero;

    use crate::jets;
    use crate::jets::JetErr;
    use crate::mem::NockStack;
    use crate::noun::{Atom, Cell, DirectAtom, D, T};

    pub fn scow(
        stack: &mut NockStack,
        aura: DirectAtom, // XX: technically this should be Atom?
        atom: Atom,
    ) -> jets::Result {
        match aura.data() {
            tas!(b"ud") => {
                if atom.as_bitslice().first_one().is_none() {
                    return Ok(T(stack, &[D(b'0' as u64), D(0)]));
                }

                let mut root = D(0);
                let mut lent = 0;
                if atom.direct().is_some() {
                    let mut n = atom.as_direct()?.data();

                    while n != 0 {
                        root = T(stack, &[D(b'0' as u64 + (n % 10)), root]);
                        n /= 10;
                        lent += 1;
                    }
                } else {
                    let mut n = atom.as_indirect()?.as_ubig(stack);

                    while !n.is_zero() {
                        root = T(stack, &[D(b'0' as u64 + (&n % 10u64)), root]);
                        n /= 10u64;
                        lent += 1;
                    }
                }

                unsafe {
                    let mut list = root.as_cell()?;
                    lent -= 1;

                    while lent > 2 {
                        if lent % 3 == 0 {
                            let (cell, memory) = Cell::new_raw_mut(stack);
                            (*memory).head = D(b'.' as u64);
                            (*memory).tail = list.tail();
                            (*(list.to_raw_pointer_mut())).tail = cell.as_noun();
                            list = list.tail().as_cell()?;
                        }
                        list = list.tail().as_cell()?;
                        lent -= 1;
                    }

                    Ok(root)
                }
            }
            _ => Err(JetErr::Punt),
        }
    }
}

#[cfg(test)]
mod tests {
    use ibig::ubig;
    use nockvm_macros::tas;

    use super::*;
    use crate::jets::util::test::{assert_jet, assert_jet_err, init_context, A};
    use crate::jets::JetErr;
    use crate::noun::{Noun, D, T};

    // Rust can't handle implicit conversions from u8 to u64
    #[allow(non_snake_case)]
    fn B(b: u8) -> Noun {
        D(b as u64)
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_scow() {
        let c = &mut init_context();

        let aura = D(tas!(b"ud"));
        let sam = T(&mut c.stack, &[aura, D(0)]);
        let res = T(&mut c.stack, &[B(b'0'), D(0)]);
        assert_jet(c, jet_scow, sam, res);
        let sam = T(&mut c.stack, &[aura, D(100)]);
        let res = T(&mut c.stack, &[B(b'1'), B(b'0'), B(b'0'), D(0)]);
        assert_jet(c, jet_scow, sam, res);
        let big = A(&mut c.stack, &ubig!(100));
        let sam = T(&mut c.stack, &[aura, big]);
        let res = T(&mut c.stack, &[B(b'1'), B(b'0'), B(b'0'), D(0)]);
        assert_jet(c, jet_scow, sam, res);
        let sam = T(&mut c.stack, &[aura, D(1000)]);
        let res = T(
            &mut c.stack,
            &[B(b'1'), B(b'.'), B(b'0'), B(b'0'), B(b'0'), D(0)],
        );
        assert_jet(c, jet_scow, sam, res);
        let big = A(&mut c.stack, &ubig!(1000));
        let sam = T(&mut c.stack, &[aura, big]);
        let res = T(
            &mut c.stack,
            &[B(b'1'), B(b'.'), B(b'0'), B(b'0'), B(b'0'), D(0)],
        );
        assert_jet(c, jet_scow, sam, res);
        let sam = T(&mut c.stack, &[aura, D(9876543210)]);
        let res = T(
            &mut c.stack,
            &[
                B(b'9'),
                B(b'.'),
                B(b'8'),
                B(b'7'),
                B(b'6'),
                B(b'.'),
                B(b'5'),
                B(b'4'),
                B(b'3'),
                B(b'.'),
                B(b'2'),
                B(b'1'),
                B(b'0'),
                D(0),
            ],
        );
        assert_jet(c, jet_scow, sam, res);
        let bad_aura = D(tas!(b"ux"));
        let sam = T(&mut c.stack, &[bad_aura, D(0)]);
        assert_jet_err(c, jet_scow, sam, JetErr::Punt);
    }
}
