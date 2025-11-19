use nockvm::jets::JetErr;
use nockvm::noun::{Noun, NounAllocator, D, T};

use super::common::*;
use crate::noun_ext::NounMathExt;

pub fn z_set_put<A: NounAllocator, H: TipHasher>(
    stack: &mut A,
    a: &Noun,
    b: &mut Noun,
    hasher: &H,
) -> Result<Noun, JetErr> {
    if unsafe { a.raw_equals(&D(0)) } {
        Ok(T(stack, &[*b, D(0), D(0)]))
    } else {
        let [mut an, al, ar] = a.uncell()?;
        if unsafe { stack.equals(b, &mut an) } {
            Ok(*a)
        } else if gor_tip(stack, b, &mut an, hasher)? {
            let c = z_set_put(stack, &al, b, hasher)?;
            let [mut cn, cl, cr] = c.uncell()?;
            if mor_tip(stack, &mut an, &mut cn, hasher)? {
                Ok(T(stack, &[an, c, ar]))
            } else {
                let new_a = T(stack, &[an, cr, ar]);
                Ok(T(stack, &[cn, cl, new_a]))
            }
        } else {
            let c = z_set_put(stack, &ar, b, hasher)?;
            let [mut cn, cl, cr] = c.uncell()?;
            if mor_tip(stack, &mut an, &mut cn, hasher)? {
                Ok(T(stack, &[an, al, c]))
            } else {
                let new_a = T(stack, &[an, al, cl]);
                Ok(T(stack, &[cn, new_a, cr]))
            }
        }
    }
}

pub fn z_set_bif<A: NounAllocator, H: TipHasher>(
    stack: &mut A,
    a: &mut Noun,
    b: &mut Noun,
    hasher: &H,
) -> Result<Noun, JetErr> {
    fn do_bif<A: NounAllocator, H: TipHasher>(
        stack: &mut A,
        a: &mut Noun,
        b: &mut Noun,
        hasher: &H,
    ) -> Result<Noun, JetErr> {
        if unsafe { a.raw_equals(&D(0)) } {
            Ok(T(stack, &[*b, D(0), D(0)]))
        } else {
            let [mut n, mut l, mut r] = a.uncell()?;
            if unsafe { stack.equals(b, &mut n) } {
                Ok(*a)
            } else if gor_tip(stack, b, &mut n, hasher)? {
                // could also parameterize Hasher if needed
                let c = do_bif(stack, &mut l, b, hasher)?;
                let [cn, cl, cr] = c.uncell()?;
                let new_a = T(stack, &[n, cr, r]);
                Ok(T(stack, &[cn, cl, new_a]))
            } else {
                let c = do_bif(stack, &mut r, b, hasher)?;
                let [cn, cl, cr] = c.uncell()?;
                let new_a = T(stack, &[n, l, cl]);
                Ok(T(stack, &[cn, new_a, cr]))
            }
        }
    }
    let res = do_bif(stack, a, b, hasher)?;
    Ok(res.as_cell()?.tail())
}

pub fn z_set_dif<A: NounAllocator, H: TipHasher>(
    stack: &mut A,
    a: &mut Noun,
    b: &mut Noun,
    hasher: &H,
) -> Result<Noun, JetErr> {
    fn dif_helper<A: NounAllocator, H: TipHasher>(
        stack: &mut A,
        d: &mut Noun,
        e: &mut Noun,
        hasher: &H,
    ) -> Result<Noun, JetErr> {
        if unsafe { d.raw_equals(&D(0)) } {
            Ok(*e)
        } else if unsafe { e.raw_equals(&D(0)) } {
            Ok(*d)
        } else {
            let [mut dn, dl, mut dr] = d.uncell()?;
            let [mut en, mut el, er] = e.uncell()?;
            if mor_tip(stack, &mut dn, &mut en, hasher)? {
                let df = dif_helper(stack, &mut dr, e, hasher)?;
                Ok(T(stack, &[dn, dl, df]))
            } else {
                let df = dif_helper(stack, d, &mut el, hasher)?;
                Ok(T(stack, &[en, df, er]))
            }
        }
    }

    if unsafe { b.raw_equals(&D(0)) } {
        Ok(*a)
    } else {
        let [mut bn, mut bl, mut br] = b.uncell()?;
        let c = z_set_bif(stack, a, &mut bn, hasher)?; // could also be generic if needed
        let [mut cl, mut cr] = c.uncell()?;
        let mut d = z_set_dif(stack, &mut cl, &mut bl, hasher)?;
        let mut e = z_set_dif(stack, &mut cr, &mut br, hasher)?;
        dif_helper(stack, &mut d, &mut e, hasher)
    }
}
