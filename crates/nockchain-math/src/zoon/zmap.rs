use nockvm::interpreter::Context;
use nockvm::jets::util::slot;
use nockvm::jets::JetErr;
use nockvm::noun::{Noun, NounAllocator, D, T};
use nockvm::site::{site_slam, Site};

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

/// Reduce a z-map using the gate's cached `Site`, mirroring Hoon `++rep`.
pub fn z_map_rep(context: &mut Context, map: &Noun, gate: &mut Noun) -> Result<Noun, JetErr> {
    let prod = slot(*gate, 13)?;
    let site = Site::new(context, gate);
    let mut reducer = |node: Noun, acc: Noun| -> Result<Noun, JetErr> {
        let sam = T(&mut context.stack, &[node, acc]);
        site_slam(context, &site, sam)
    };
    rep_fold(*map, prod, &mut reducer)
}

fn rep_fold<F>(tree: Noun, acc: Noun, reducer: &mut F) -> Result<Noun, JetErr>
where
    F: FnMut(Noun, Noun) -> Result<Noun, JetErr>,
{
    if unsafe { tree.raw_equals(&D(0)) } {
        return Ok(acc);
    }

    let [entry, left, right] = tree.uncell()?;
    let acc = reducer(entry, acc)?;
    let acc = rep_fold(left, acc, reducer)?;
    rep_fold(right, acc, reducer)
}
