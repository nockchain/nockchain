use nockvm::jets::JetErr;
use nockvm::noun::{Noun, NounAllocator, D, T};

use super::common::*;
use crate::noun_ext::NounMathExt;

pub fn z_map_put<A: NounAllocator, H: TipHasher>(
    stack: &mut A,
    a: &Noun,
    b: &mut Noun,
    c: &mut Noun,
    hasher: &H,
) -> Result<Noun, JetErr> {
    if unsafe { a.raw_equals(&D(0)) } {
        let kv = T(stack, &[*b, *c]);
        Ok(T(stack, &[kv, D(0), D(0)]))
    } else {
        let [mut an, mut al, mut ar] = a.uncell()?;
        let [mut anp, mut anq] = an.uncell()?;
        if unsafe { stack.equals(b, &mut anp) } {
            if unsafe { stack.equals(c, &mut anq) } {
                return Ok(*a);
            } else {
                an = T(stack, &[*b, *c]);
                let anbc = T(stack, &[an, al, ar]);
                return Ok(anbc);
            }
        } else if gor_tip(stack, b, &mut anp, hasher)? {
            let d = z_map_put(stack, &mut al, b, c, hasher)?;
            let [dn, dl, dr] = d.uncell()?;
            let [mut dnp, _dnq] = dn.uncell()?;
            if mor_tip(stack, &mut anp, &mut dnp, hasher)? {
                Ok(T(stack, &[an, d, ar]))
            } else {
                let new_a = T(stack, &[an, dr, ar]);
                Ok(T(stack, &[dn, dl, new_a]))
            }
        } else {
            let d = z_map_put(stack, &mut ar, b, c, hasher)?;
            let [dn, dl, dr] = d.uncell()?;
            let [mut dnp, _dnq] = dn.uncell()?;
            if mor_tip(stack, &mut anp, &mut dnp, hasher)? {
                Ok(T(stack, &[an, al, d]))
            } else {
                let new_a = T(stack, &[an, al, dl]);
                Ok(T(stack, &[dn, new_a, dr]))
            }
        }
    }
}
