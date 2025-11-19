use either::{Left, Right};
use ibig::ops::DivRem;
use ibig::UBig;

/** Math jets
 *
 * We use ibig for math operations.  This is a pure rust library, and it is very convenient to use.
 * If they're noticeably, fater, we may want to use gmp or a library wrapping it, such as rug.
 *
 * In any case, it's important to ensure that the library only allocates on the nock stack.  Gmp
 * has mp_set_memory_functions.  I don't know if rug does any allocation on top of that.  ibig does
 * not appear to support custom allocation functionc, but we could probably patch it.  If we're
 * patching it, we might even be able to avoid copying the input and output at all, which might
 * give a greater performance advantage than using gmp anyway.
 *
 * Another approach is use a global custom allocator.  This is fairly involved, but it would allow
 * us to use any library without worrying whether it allocates.
 */
use crate::interpreter::Context;
use crate::jets::util::*;
use crate::jets::Result;
use crate::noun::{Atom, DirectAtom, IndirectAtom, Noun, D, DIRECT_MAX, T};

crate::gdb!();

pub fn jet_add(context: &mut Context, subject: Noun) -> Result {
    let arg = slot(subject, 6)?;
    let a = slot(arg, 2)?.as_atom()?;
    let b = slot(arg, 3)?.as_atom()?;
    Ok(util::add(&mut context.stack, a, b).as_noun())
}

pub fn jet_dec(context: &mut Context, subject: Noun) -> Result {
    let arg = slot(subject, 6)?;
    if let Ok(atom) = arg.as_atom() {
        match atom.as_either() {
            Left(direct) => {
                if direct.data() == 0 {
                    Err(BAIL_EXIT)
                } else {
                    Ok(unsafe { DirectAtom::new_unchecked(direct.data() - 1) }.as_noun())
                }
            }
            Right(indirect) => {
                let indirect_slice = indirect.as_bitslice();
                match indirect_slice.first_one() {
                    None => {
                        panic!("Decrementing 0 stored as an indirect atom");
                    }
                    Some(first_one) => {
                        let (mut new_indirect, new_slice) = unsafe {
                            IndirectAtom::new_raw_mut_bitslice(&mut context.stack, indirect.size())
                        };
                        if first_one > 0 {
                            new_slice[..first_one].fill(true);
                        }
                        new_slice.set(first_one, false);
                        new_slice[first_one + 1..]
                            .copy_from_bitslice(&indirect_slice[first_one + 1..]);
                        let res = unsafe { new_indirect.normalize_as_atom() };
                        Ok(res.as_noun())
                    }
                }
            }
        }
    } else {
        Err(BAIL_EXIT)
    }
}

pub fn jet_div(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let arg = slot(subject, 6)?;
    let a = slot(arg, 2)?.as_atom()?;
    let b = slot(arg, 3)?.as_atom()?;

    if unsafe { b.as_noun().raw_equals(&D(0)) } {
        Err(BAIL_EXIT)
    } else if let (Ok(a), Ok(b)) = (a.as_direct(), b.as_direct()) {
        Ok(unsafe { DirectAtom::new_unchecked(a.data() / b.data()) }.as_noun())
    } else {
        let a_big = a.as_ubig(stack);
        let b_big = b.as_ubig(stack);
        let res = UBig::div_stack(stack, a_big, b_big);
        Ok(Atom::from_ubig(stack, &res).as_noun())
    }
}

pub fn jet_dvr(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let arg = slot(subject, 6)?;
    let a = slot(arg, 2)?.as_atom()?;
    let b = slot(arg, 3)?.as_atom()?;

    if unsafe { b.as_noun().raw_equals(&D(0)) } {
        Err(BAIL_EXIT)
    } else {
        let (div, rem) = if let (Ok(a), Ok(b)) = (a.as_direct(), b.as_direct()) {
            let (div, rem) = (a.data() / b.data(), a.data() % b.data());
            unsafe {
                (
                    DirectAtom::new_unchecked(div).as_noun(),
                    DirectAtom::new_unchecked(rem).as_noun(),
                )
            }
        } else {
            let (div, rem) = a.as_ubig(stack).div_rem(b.as_ubig(stack));
            (
                Atom::from_ubig(stack, &div).as_noun(),
                Atom::from_ubig(stack, &rem).as_noun(),
            )
        };

        Ok(T(stack, &[div, rem]))
    }
}

pub fn jet_gte(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let arg = slot(subject, 6)?;
    let a = slot(arg, 2)?.as_atom()?;
    let b = slot(arg, 3)?.as_atom()?;

    Ok(util::gte(stack, a, b))
}

pub fn jet_gth(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let arg = slot(subject, 6)?;
    let a = slot(arg, 2)?.as_atom()?;
    let b = slot(arg, 3)?.as_atom()?;

    Ok(util::gth(stack, a, b))
}

pub fn jet_lte(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let arg = slot(subject, 6)?;
    let a = slot(arg, 2)?.as_atom()?;
    let b = slot(arg, 3)?.as_atom()?;

    Ok(util::lte(stack, a, b))
}

pub fn jet_lth(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let arg = slot(subject, 6)?;
    let a = slot(arg, 2)?.as_atom()?;
    let b = slot(arg, 3)?.as_atom()?;

    Ok(util::lth(stack, a, b))
}

pub fn jet_max(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let arg = slot(subject, 6)?;
    let a = slot(arg, 2)?.as_atom()?;
    let b = slot(arg, 3)?.as_atom()?;

    Ok(if let (Ok(a), Ok(b)) = (a.as_direct(), b.as_direct()) {
        if a.data() >= b.data() {
            a.as_noun()
        } else {
            b.as_noun()
        }
    } else if a.bit_size() > b.bit_size() {
        a.as_noun()
    } else if a.bit_size() < b.bit_size() {
        b.as_noun()
    } else if a.as_ubig(stack) >= b.as_ubig(stack) {
        a.as_noun()
    } else {
        b.as_noun()
    })
}

pub fn jet_min(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let arg = slot(subject, 6)?;
    let a = slot(arg, 2)?.as_atom()?;
    let b = slot(arg, 3)?.as_atom()?;

    Ok(if let (Ok(a), Ok(b)) = (a.as_direct(), b.as_direct()) {
        if a.data() <= b.data() {
            a.as_noun()
        } else {
            b.as_noun()
        }
    } else if a.bit_size() < b.bit_size() {
        a.as_noun()
    } else if a.bit_size() > b.bit_size() {
        b.as_noun()
    } else if a.as_ubig(stack) <= b.as_ubig(stack) {
        a.as_noun()
    } else {
        b.as_noun()
    })
}

pub fn jet_mod(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let arg = slot(subject, 6)?;
    let a = slot(arg, 2)?.as_atom()?;
    let b = slot(arg, 3)?.as_atom()?;

    if unsafe { b.as_noun().raw_equals(&D(0)) } {
        Err(BAIL_EXIT)
    } else if let (Ok(a), Ok(b)) = (a.as_direct(), b.as_direct()) {
        Ok(unsafe { DirectAtom::new_unchecked(a.data() % b.data()) }.as_noun())
    } else {
        let res = a.as_ubig(stack) % b.as_ubig(stack);
        Ok(Atom::from_ubig(stack, &res).as_noun())
    }
}

pub fn jet_mul(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let arg = slot(subject, 6)?;
    let a = slot(arg, 2)?.as_atom()?;
    let b = slot(arg, 3)?.as_atom()?;

    if let (Ok(a), Ok(b)) = (a.as_direct(), b.as_direct()) {
        let res = a.data() as u128 * b.data() as u128;
        if res < DIRECT_MAX as u128 {
            Ok(Atom::new(stack, res as u64).as_noun())
        } else {
            Ok(unsafe {
                IndirectAtom::new_raw_bytes(
                    stack,
                    if res < u64::MAX as u128 { 8 } else { 16 },
                    &res as *const u128 as *const u8,
                )
            }
            .as_noun())
        }
    } else {
        let a_big = a.as_ubig(stack);
        let b_big = b.as_ubig(stack);
        let res = UBig::mul_stack(stack, a_big, b_big);
        Ok(Atom::from_ubig(stack, &res).as_noun())
    }
}

pub fn jet_sub(context: &mut Context, subject: Noun) -> Result {
    let arg = slot(subject, 6)?;
    let a = slot(arg, 2)?.as_atom()?;
    let b = slot(arg, 3)?.as_atom()?;

    Ok(util::sub(&mut context.stack, a, b)?.as_noun())
}

pub mod util {
    use ibig::UBig;

    use crate::mem::NockStack;
    use crate::noun::{Atom, Error, Noun, NounAllocator, Result, NO, YES};

    /// Addition
    pub fn add(stack: &mut NockStack, a: Atom, b: Atom) -> Atom {
        if let (Ok(a), Ok(b)) = (a.as_direct(), b.as_direct()) {
            Atom::new(stack, a.data() + b.data())
        } else {
            let a_big = a.as_ubig(stack);
            let b_big = b.as_ubig(stack);
            let res = UBig::add_stack(stack, a_big, b_big);
            Atom::from_ubig(stack, &res)
        }
    }

    /// Greater than or equal to (boolean)
    pub fn gte_b(stack: &mut NockStack, a: Atom, b: Atom) -> bool {
        if let (Ok(a), Ok(b)) = (a.as_direct(), b.as_direct()) {
            a.data() >= b.data()
        } else if a.bit_size() > b.bit_size() {
            true
        } else if a.bit_size() < b.bit_size() {
            false
        } else {
            a.as_ubig(stack) >= b.as_ubig(stack)
        }
    }

    /// Greater than or equal to
    pub fn gte(stack: &mut NockStack, a: Atom, b: Atom) -> Noun {
        if gte_b(stack, a, b) {
            YES
        } else {
            NO
        }
    }

    /// Greater than (boolean)
    pub fn gth_b(stack: &mut NockStack, a: Atom, b: Atom) -> bool {
        if let (Ok(a), Ok(b)) = (a.as_direct(), b.as_direct()) {
            a.data() > b.data()
        } else if a.bit_size() > b.bit_size() {
            true
        } else if a.bit_size() < b.bit_size() {
            false
        } else {
            a.as_ubig(stack) > b.as_ubig(stack)
        }
    }

    /// Greater than
    pub fn gth(stack: &mut NockStack, a: Atom, b: Atom) -> Noun {
        if gth_b(stack, a, b) {
            YES
        } else {
            NO
        }
    }

    /// Less than or equal to (boolean)
    pub fn lte_b(stack: &mut NockStack, a: Atom, b: Atom) -> bool {
        if let (Ok(a), Ok(b)) = (a.as_direct(), b.as_direct()) {
            a.data() <= b.data()
        } else if a.bit_size() < b.bit_size() {
            true
        } else if a.bit_size() > b.bit_size() {
            false
        } else {
            a.as_ubig(stack) <= b.as_ubig(stack)
        }
    }

    /// Less than or equal to
    pub fn lte(stack: &mut NockStack, a: Atom, b: Atom) -> Noun {
        if lte_b(stack, a, b) {
            YES
        } else {
            NO
        }
    }

    /// Less than (boolean)
    pub fn lth_b<A: NounAllocator>(stack: &mut A, a: Atom, b: Atom) -> bool {
        if let (Ok(a), Ok(b)) = (a.as_direct(), b.as_direct()) {
            a.data() < b.data()
        } else if a.bit_size() > b.bit_size() {
            false
        } else if a.bit_size() < b.bit_size() {
            true
        } else {
            a.as_ubig(stack) < b.as_ubig(stack)
        }
    }

    /// Less than
    pub fn lth<A: NounAllocator>(stack: &mut A, a: Atom, b: Atom) -> Noun {
        if lth_b(stack, a, b) {
            YES
        } else {
            NO
        }
    }

    /// Subtraction
    pub fn sub(stack: &mut NockStack, a: Atom, b: Atom) -> Result<Atom> {
        if let (Ok(a), Ok(b)) = (a.as_direct(), b.as_direct()) {
            let a_small = a.data();
            let b_small = b.data();

            if a_small < b_small {
                Err(Error::NotRepresentable)
            } else {
                Ok(Atom::new(stack, a_small - b_small))
            }
        } else {
            let a_big = a.as_ubig(stack);
            let b_big = b.as_ubig(stack);

            if a_big < b_big {
                Err(Error::NotRepresentable)
            } else {
                let a_big = a.as_ubig(stack);
                let b_big = b.as_ubig(stack);
                let res = UBig::sub_stack(stack, a_big, b_big);
                Ok(Atom::from_ubig(stack, &res))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ibig::ubig;

    use super::*;
    use crate::jets::util::test::*;
    use crate::mem::NockStack;
    use crate::noun::{Noun, D, NO, T, YES};

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

    fn atom_128_b(stack: &mut NockStack) -> Noun {
        A(stack, &ubig!(0xdeadbeef12345678fedcba9876540000))
    }

    fn atom_264(stack: &mut NockStack) -> Noun {
        A(
            stack,
            &ubig!(_0xdeadbeef12345678fedcba9876540000deadbeef12345678fedcba9876540000ff),
        )
    }

    fn atom_528(stack: &mut NockStack) -> Noun {
        A(stack, &ubig!(_0xdeadbeef12345678fedcba9876540000deadbeef12345678fedcba9876540000ffdeadbeef12345678fedcba9876540000deadbeef12345678fedcba9876540001ff))
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_add() {
        let c = &mut init_context();

        assert_common_jet(
            c,
            jet_add,
            &[atom_128, atom_96],
            ubig!(0xdeadbef00d03068514bb685765666666),
        );
        assert_common_jet(
            c,
            jet_add,
            &[atom_63, atom_96],
            ubig!(0xfaceb00c95deadbeef123455),
        );
        assert_common_jet(c, jet_add, &[atom_63, atom_63], ubig!(0xfffffffffffffffe));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_dec() {
        let c = &mut init_context();
        let s = &mut c.stack;

        let (a0, _a24, a63, _a96, a128) = atoms(s);
        assert_jet_ubig(c, jet_dec, a128, ubig!(0xdeadbeef12345678fedcba987654320f));
        assert_jet(c, jet_dec, a63, D(0x7ffffffffffffffe));
        assert_jet_err(c, jet_dec, a0, BAIL_EXIT);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_div() {
        let c = &mut init_context();

        assert_common_jet(c, jet_div, &[atom_128, atom_96], ubig!(0xe349f8f0));
        assert_common_jet(c, jet_div, &[atom_96, atom_63], ubig!(0x1f59d6018));
        assert_common_jet(c, jet_div, &[atom_63, atom_96], ubig!(0));
        assert_common_jet(c, jet_div, &[atom_63, atom_63], ubig!(1));
        assert_common_jet(c, jet_div, &[atom_63, atom_24], ubig!(0xf2044dacfe));
        assert_common_jet(
            c,
            jet_div,
            &[atom_128, atom_24],
            ubig!(0x1a507f98b6fa8605ea3a79e97bf),
        );
        let res = ubig!(
            _0x00000000000001000000000000000000000000000000000000000000000000000000000000000001
        );
        assert_common_jet(c, jet_div, &[atom_528, atom_264], res);
        assert_common_jet_err(c, jet_div, &[atom_63, atom_0], BAIL_EXIT);
        assert_common_jet_err(c, jet_div, &[atom_0, atom_0], BAIL_EXIT);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_dvr() {
        let c = &mut init_context();

        let (a0, a24, a63, a96, a128) = atoms(&mut c.stack);
        let a264 = atom_264(&mut c.stack);
        let a528 = atom_528(&mut c.stack);

        let sam = T(&mut c.stack, &[a128, a96]);
        let res_a = A(&mut c.stack, &ubig!(0xe349f8f0));
        let res_b = A(&mut c.stack, &ubig!(0xcb0ce564ec598f658409d170));
        let res = T(&mut c.stack, &[res_a, res_b]);
        assert_jet(c, jet_dvr, sam, res);

        let sam = T(&mut c.stack, &[a128, a24]);
        let res_a = A(&mut c.stack, &ubig!(0x1a507f98b6fa8605ea3a79e97bf));
        let res_b = A(&mut c.stack, &ubig!(0x3b2013));
        let res = T(&mut c.stack, &[res_a, res_b]);
        assert_jet(c, jet_dvr, sam, res);

        let sam = T(&mut c.stack, &[a63, a63]);
        let res_a = A(&mut c.stack, &ubig!(1));
        let res_b = A(&mut c.stack, &ubig!(0));
        let res = T(&mut c.stack, &[res_a, res_b]);
        assert_jet(c, jet_dvr, sam, res);

        let sam = T(&mut c.stack, &[a0, a24]);
        let res_a = A(&mut c.stack, &ubig!(0));
        let res_b = A(&mut c.stack, &ubig!(0));
        let res = T(&mut c.stack, &[res_a, res_b]);
        assert_jet(c, jet_dvr, sam, res);

        let sam = T(&mut c.stack, &[a528, a264]);
        let res_a = A(
            &mut c.stack,
            &ubig!(
                _0x00000000000001000000000000000000000000000000000000000000000000000000000000000001
            ),
        );
        let res_b = A(&mut c.stack, &ubig!(0x100));
        let res = T(&mut c.stack, &[res_a, res_b]);
        assert_jet(c, jet_dvr, sam, res);

        assert_common_jet_err(c, jet_dvr, &[atom_63, atom_0], BAIL_EXIT);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_gte() {
        let c = &mut init_context();

        assert_common_jet_noun(c, jet_gte, &[atom_128, atom_96], YES);
        assert_common_jet_noun(c, jet_gte, &[atom_96, atom_63], YES);
        assert_common_jet_noun(c, jet_gte, &[atom_63, atom_96], NO);
        assert_common_jet_noun(c, jet_gte, &[atom_63, atom_63], YES);
        assert_common_jet_noun(c, jet_gte, &[atom_63, atom_24], YES);
        assert_common_jet_noun(c, jet_gte, &[atom_128, atom_24], YES);
        assert_common_jet_noun(c, jet_gte, &[atom_128, atom_128_b], YES);
        assert_common_jet_noun(c, jet_gte, &[atom_128_b, atom_128], NO);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_gth() {
        let c = &mut init_context();

        assert_common_jet_noun(c, jet_gth, &[atom_128, atom_96], YES);
        assert_common_jet_noun(c, jet_gth, &[atom_96, atom_63], YES);
        assert_common_jet_noun(c, jet_gth, &[atom_63, atom_96], NO);
        assert_common_jet_noun(c, jet_gth, &[atom_63, atom_63], NO);
        assert_common_jet_noun(c, jet_gth, &[atom_63, atom_24], YES);
        assert_common_jet_noun(c, jet_gth, &[atom_128, atom_24], YES);
        assert_common_jet_noun(c, jet_gth, &[atom_128, atom_128_b], YES);
        assert_common_jet_noun(c, jet_gth, &[atom_128_b, atom_128], NO);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_lte() {
        let c = &mut init_context();

        assert_common_jet_noun(c, jet_lte, &[atom_128, atom_96], NO);
        assert_common_jet_noun(c, jet_lte, &[atom_96, atom_63], NO);
        assert_common_jet_noun(c, jet_lte, &[atom_63, atom_96], YES);
        assert_common_jet_noun(c, jet_lte, &[atom_63, atom_63], YES);
        assert_common_jet_noun(c, jet_lte, &[atom_63, atom_24], NO);
        assert_common_jet_noun(c, jet_lte, &[atom_128, atom_24], NO);
        assert_common_jet_noun(c, jet_lte, &[atom_128, atom_128_b], NO);
        assert_common_jet_noun(c, jet_lte, &[atom_128_b, atom_128], YES);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_lth() {
        let c = &mut init_context();

        assert_common_jet_noun(c, jet_lth, &[atom_128, atom_96], NO);
        assert_common_jet_noun(c, jet_lth, &[atom_96, atom_63], NO);
        assert_common_jet_noun(c, jet_lth, &[atom_63, atom_96], YES);
        assert_common_jet_noun(c, jet_lth, &[atom_63, atom_63], NO);
        assert_common_jet_noun(c, jet_lth, &[atom_63, atom_24], NO);
        assert_common_jet_noun(c, jet_lth, &[atom_128, atom_24], NO);
        assert_common_jet_noun(c, jet_lth, &[atom_128, atom_128_b], NO);
        assert_common_jet_noun(c, jet_lth, &[atom_128_b, atom_128], YES);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_max() {
        let c = &mut init_context();

        assert_common_jet(
            c,
            jet_max,
            &[atom_128, atom_96],
            ubig!(0xdeadbeef12345678fedcba9876543210),
        );
        assert_common_jet(
            c,
            jet_max,
            &[atom_96, atom_63],
            ubig!(0xfaceb00c15deadbeef123456),
        );
        assert_common_jet(
            c,
            jet_max,
            &[atom_63, atom_96],
            ubig!(0xfaceb00c15deadbeef123456),
        );
        assert_common_jet(c, jet_max, &[atom_63, atom_63], ubig!(0x7fffffffffffffff));
        assert_common_jet(c, jet_max, &[atom_63, atom_24], ubig!(0x7fffffffffffffff));
        assert_common_jet(
            c,
            jet_max,
            &[atom_128, atom_24],
            ubig!(0xdeadbeef12345678fedcba9876543210),
        );
        assert_common_jet(
            c,
            jet_max,
            &[atom_128, atom_128_b],
            ubig!(0xdeadbeef12345678fedcba9876543210),
        );
        assert_common_jet(
            c,
            jet_max,
            &[atom_128_b, atom_128],
            ubig!(0xdeadbeef12345678fedcba9876543210),
        );
        assert_common_jet(c, jet_max, &[atom_528, atom_264], ubig!(_0xdeadbeef12345678fedcba9876540000deadbeef12345678fedcba9876540000ffdeadbeef12345678fedcba9876540000deadbeef12345678fedcba9876540001ff));
        assert_common_jet(c, jet_max, &[atom_264, atom_528], ubig!(_0xdeadbeef12345678fedcba9876540000deadbeef12345678fedcba9876540000ffdeadbeef12345678fedcba9876540000deadbeef12345678fedcba9876540001ff));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_min() {
        let c = &mut init_context();

        assert_common_jet(
            c,
            jet_min,
            &[atom_128, atom_96],
            ubig!(0xfaceb00c15deadbeef123456),
        );
        assert_common_jet(c, jet_min, &[atom_96, atom_63], ubig!(0x7fffffffffffffff));
        assert_common_jet(c, jet_min, &[atom_63, atom_96], ubig!(0x7fffffffffffffff));
        assert_common_jet(c, jet_min, &[atom_63, atom_63], ubig!(0x7fffffffffffffff));
        assert_common_jet(c, jet_min, &[atom_63, atom_24], ubig!(0x876543));
        assert_common_jet(c, jet_min, &[atom_128, atom_24], ubig!(0x876543));
        assert_common_jet(
            c,
            jet_min,
            &[atom_128, atom_128_b],
            ubig!(0xdeadbeef12345678fedcba9876540000),
        );
        assert_common_jet(
            c,
            jet_min,
            &[atom_128_b, atom_128],
            ubig!(0xdeadbeef12345678fedcba9876540000),
        );
        assert_common_jet(
            c,
            jet_min,
            &[atom_528, atom_264],
            ubig!(_0xdeadbeef12345678fedcba9876540000deadbeef12345678fedcba9876540000ff),
        );
        assert_common_jet(
            c,
            jet_min,
            &[atom_264, atom_528],
            ubig!(_0xdeadbeef12345678fedcba9876540000deadbeef12345678fedcba9876540000ff),
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_mod() {
        let c = &mut init_context();

        assert_common_jet(
            c,
            jet_mod,
            &[atom_128, atom_96],
            ubig!(0xcb0ce564ec598f658409d170),
        );
        assert_common_jet(c, jet_mod, &[atom_96, atom_63], ubig!(0x15deadc0e4af946e));
        assert_common_jet(c, jet_mod, &[atom_63, atom_96], ubig!(0x7fffffffffffffff));
        assert_common_jet(c, jet_mod, &[atom_63, atom_63], ubig!(0));
        assert_common_jet(c, jet_mod, &[atom_63, atom_24], ubig!(0x798385));
        assert_common_jet(c, jet_mod, &[atom_128, atom_24], ubig!(0x3b2013));
        assert_common_jet(c, jet_mod, &[atom_528, atom_264], ubig!(0x100));
        assert_common_jet_err(c, jet_mod, &[atom_63, atom_0], BAIL_EXIT);
        assert_common_jet_err(c, jet_mod, &[atom_0, atom_0], BAIL_EXIT);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_mul() {
        let c = &mut init_context();

        assert_common_jet(
            c,
            jet_mul,
            &[atom_128, atom_96],
            ubig!(_0xda297567129704bf42e744f13ff0ea4fc4ac01215b708bc94f941160),
        );
        assert_common_jet(
            c,
            jet_mul,
            &[atom_63, atom_96],
            ubig!(_0x7d6758060aef56de7cba6a1eea21524110edcbaa),
        );
        assert_common_jet(
            c,
            jet_mul,
            &[atom_63, atom_63],
            ubig!(0x3fffffffffffffff0000000000000001),
        );
        assert_common_jet(c, jet_mul, &[atom_24, atom_24], ubig!(0x479bf4b7ef89));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_sub() {
        let c = &mut init_context();

        assert_common_jet(
            c,
            jet_sub,
            &[atom_128, atom_96],
            ubig!(0xdeadbeee1765a66ce8fe0cd98741fdba),
        );
        assert_common_jet(
            c,
            jet_sub,
            &[atom_96, atom_63],
            ubig!(0xfaceb00b95deadbeef123457),
        );
        assert_common_jet(c, jet_sub, &[atom_63, atom_63], ubig!(0));
        assert_common_jet(c, jet_sub, &[atom_128, atom_128], ubig!(0));
        assert_common_jet_err(c, jet_sub, &[atom_63, atom_96], BAIL_EXIT);
    }
}
