/** Text processing jets
 */
use crate::interpreter::Context;
use crate::jets::util::{slot, BAIL_FAIL};
use crate::jets::Result;
use crate::noun::{Cell, Noun, D, T};
use crate::site::{site_slam, Site};

crate::gdb!();

pub fn jet_weld(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let a = slot(sam, 2, &space)?;
    let b = slot(sam, 3, &space)?;
    util::weld(&mut context.stack, a, b, &space)
}

pub fn jet_flop(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    util::flop(&mut context.stack, sam, &space)
}

pub fn jet_lent(_context: &mut Context, subject: Noun) -> Result {
    let space = _context.stack.noun_space();
    let list = slot(subject, 6, &space)?;
    util::lent(list, &space).map(|x| D(x as u64))
}

pub fn jet_roll(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let sample = slot(subject, 6, &space)?;
    let mut list = slot(sample, 2, &space)?;
    let mut gate = slot(sample, 3, &space)?;
    let mut prod = slot(gate, 13, &space)?;

    let site = Site::new(context, &mut gate);
    loop {
        if let Ok(list_cell) = list.in_space(&space).as_cell() {
            list = list_cell.tail().noun();
            let sam = T(&mut context.stack, &[list_cell.head().noun(), prod]);
            prod = site_slam(context, &site, sam)?;
        } else {
            if unsafe { !list.raw_equals(&D(0)) } {
                return Err(BAIL_FAIL);
            }
            return Ok(prod);
        }
    }
}

pub fn jet_snag(_context: &mut Context, subject: Noun) -> Result {
    let space = _context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let index = slot(sam, 2, &space)?;
    let list = slot(sam, 3, &space)?;
    util::snag(list, index, &space)
}

pub fn jet_snip(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let list = slot(subject, 6, &space)?;
    util::snip(&mut context.stack, list)
}

pub fn jet_turn(context: &mut Context, subject: Noun) -> Result {
    let mut res = D(0);
    let mut dest: *mut Noun = &mut res; // Mutable pointer because we cannot guarantee initialized
    let space = context.stack.noun_space();
    let sample = slot(subject, 6, &space)?;
    let mut list = slot(sample, 2, &space)?;
    let mut gate = slot(sample, 3, &space)?;

    // Since the gate doesn't change, we can do a single jet check and use that through the whole
    // loop
    let site = Site::new(context, &mut gate);
    loop {
        if let Ok(list_cell) = list.in_space(&space).as_cell() {
            list = list_cell.tail().noun();
            unsafe {
                let (new_cell, new_mem) = Cell::new_raw_mut(&mut context.stack);
                (*new_mem).head = site_slam(context, &site, list_cell.head().noun())?;
                *dest = new_cell.as_noun();
                dest = &mut (*new_mem).tail;
            }
        } else {
            if unsafe { !list.raw_equals(&D(0)) } {
                return Err(BAIL_FAIL);
            }
            unsafe {
                *dest = D(0);
            };
            return Ok(res);
        }
    }
}

pub fn jet_zing(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let list = slot(subject, 6, &space)?;
    let stack = &mut context.stack;

    util::zing(stack, list)
}

pub fn jet_reap(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let a_noun = slot(sam, 2, &space)?;
    let b_noun = slot(sam, 3, &space)?;
    let a = a_noun.in_space(&space).as_atom()?.as_u64()?;
    util::reap(&mut context.stack, a, b_noun)
}

pub fn jet_levy(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let a_noun = slot(sam, 2, &space)?;
    let b_noun = slot(sam, 3, &space)?;

    util::levy(context, a_noun, b_noun)
}

pub fn jet_find(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let nedl = slot(sam, 2, &space)?;
    let hstk = slot(sam, 3, &space)?;

    util::find(context, nedl, hstk)
}

pub fn jet_scag(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let sam_cell = sam.in_space(&space).as_cell()?;
    let a = sam_cell.head().as_atom()?.atom();
    let b = sam_cell.tail().noun();

    util::scag(context, a, b)
}

pub mod util {
    use std::result;

    use crate::interpreter::Context;
    use crate::jets::util::BAIL_EXIT;
    use crate::jets::{JetErr, Result};
    use crate::mem::NockStack;
    use crate::noun::{Atom, Cell, Noun, NounAllocator, NounSpace, D, NO, T, YES};
    use crate::site::{site_slam, Site};

    /// Reverse order of list
    pub fn flop<T: NounAllocator>(alloc: &mut T, noun: Noun, space: &NounSpace) -> Result {
        let mut list = noun;
        let mut tsil = D(0);
        loop {
            if unsafe { list.raw_equals(&D(0)) } {
                break;
            }

            let cell = list.in_space(space).as_cell()?;
            tsil = T(alloc, &[cell.head().noun(), tsil]);
            list = cell.tail().noun();
        }

        Ok(tsil)
    }

    pub fn weld<A: NounAllocator>(alloc: &mut A, a: Noun, b: Noun, space: &NounSpace) -> Result {
        let mut res = D(0);
        let mut cur = a;
        loop {
            if unsafe { cur.raw_equals(&D(0)) } {
                break;
            }
            let cell = cur.in_space(space).as_cell()?;
            res = T(alloc, &[cell.head().noun(), res]);
            cur = cell.tail().noun();
        }
        cur = b;
        loop {
            if unsafe { cur.raw_equals(&D(0)) } {
                break;
            }
            let cell = cur.in_space(space).as_cell()?;
            res = T(alloc, &[cell.head().noun(), res]);
            cur = cell.tail().noun();
        }
        let out_space = alloc.noun_space();
        flop(alloc, res, &out_space)
    }

    pub fn lent(tape: Noun, space: &NounSpace) -> result::Result<usize, JetErr> {
        let mut len = 0usize;
        let mut list = tape;
        loop {
            if let Some(atom) = list.in_space(space).atom() {
                if atom.as_bitslice().first_one().is_none() {
                    break;
                } else {
                    return Err(BAIL_EXIT);
                }
            }
            let cell = list.in_space(space).as_cell()?;
            // don't need checked_add or indirect atom result: 2^63-1 atoms would be 64 ebibytes
            len += 1;
            list = cell.tail().noun();
        }
        Ok(len)
    }

    pub fn snag(tape: Noun, index: Noun, space: &NounSpace) -> Result {
        let mut list = tape;
        let mut idx = index.in_space(space).as_atom()?.as_u64()? as usize;
        loop {
            if unsafe { list.raw_equals(&D(0)) } {
                return Err(BAIL_EXIT);
            }
            let cell = list.in_space(space).as_cell()?;
            if idx == 0 {
                return Ok(cell.head().noun());
            }
            idx -= 1;
            list = cell.tail().noun();
        }
    }

    pub fn snip(stack: &mut NockStack, tape: Noun) -> Result {
        let mut ret = D(0);
        let mut dest = &mut ret as *mut Noun;
        let mut list = tape;
        let space = stack.noun_space();

        if let Some(atom) = list.in_space(&space).atom() {
            if atom.as_bitslice().first_one().is_none() {
                return Ok(D(0));
            }
        }

        loop {
            let cell = list.in_space(&space).as_cell()?;
            if let Some(atom) = cell.tail().atom() {
                if atom.as_bitslice().first_one().is_none() {
                    break;
                } else {
                    return Err(BAIL_EXIT);
                }
            }
            unsafe {
                let (new_cell, new_mem) = Cell::new_raw_mut(stack);
                (*new_mem).head = cell.head().noun();
                *dest = new_cell.as_noun();
                dest = &mut (*new_mem).tail;
            }
            list = cell.tail().noun();
        }
        unsafe { *dest = D(0) };
        Ok(ret)
    }

    pub fn zing(stack: &mut NockStack, mut list: Noun) -> Result {
        unsafe {
            let mut res: Noun = D(0);
            let mut dest = &mut res as *mut Noun;
            let space = stack.noun_space();

            while !list.raw_equals(&D(0)) {
                let pair = list.in_space(&space).as_cell()?;
                let mut sublist = pair.head().noun();
                list = pair.tail().noun();

                while !sublist.raw_equals(&D(0)) {
                    let it = sublist.in_space(&space).as_cell()?;
                    let i = it.head().noun();
                    sublist = it.tail().noun();

                    let (new_cell, new_memory) = Cell::new_raw_mut(stack);
                    (*new_memory).head = i;
                    *dest = new_cell.as_noun();
                    dest = &mut (*new_memory).tail;
                }
            }

            *dest = D(0);
            Ok(res)
        }
    }

    pub fn reap(stack: &mut NockStack, a: u64, b_noun: Noun) -> Result {
        let mut tsil = D(0);
        let mut a_mut = a;
        loop {
            if a_mut == 0 {
                break;
            }
            tsil = T(stack, &[b_noun, tsil]);
            a_mut -= 1;
        }
        Ok(tsil)
    }
    pub fn levy(context: &mut Context, a_noun: Noun, mut b_noun: Noun) -> Result {
        let site = Site::new(context, &mut b_noun);
        let mut list = a_noun;
        let space = context.stack.noun_space();

        loop {
            if unsafe { list.raw_equals(&D(0)) } {
                return Ok(YES);
            }

            let cell = list.in_space(&space).as_cell()?;
            let b_res = site_slam(context, &site, cell.head().noun())?;
            if unsafe { b_res.raw_equals(&NO) } {
                return Ok(NO);
            }
            list = cell.tail().noun();
        }
    }

    pub fn find(context: &mut Context, nedl: Noun, hstk: Noun) -> Result {
        let mut hstk = hstk;
        let mut i = 0;
        let space = context.stack.noun_space();
        loop {
            let mut n = nedl;
            let mut h = hstk;
            loop {
                if unsafe { n.raw_equals(&D(0)) || h.raw_equals(&D(0)) } {
                    // not found
                    return Ok(D(0)); // (unit @ud)  ~
                }

                if unsafe {
                    n.in_space(&space)
                        .as_cell()?
                        .head()
                        .noun()
                        .raw_equals(&h.in_space(&space).as_cell()?.head().noun())
                } {
                    if unsafe {
                        n.in_space(&space)
                            .as_cell()?
                            .tail()
                            .noun()
                            .raw_equals(&D(0))
                    } {
                        // match found
                        return Ok(T(&mut context.stack, &[D(0), D(i)])); // (unit @ud)  i
                    }

                    n = n.in_space(&space).as_cell()?.tail().noun();
                    h = h.in_space(&space).as_cell()?.tail().noun();
                    continue;
                }

                // try next position
                hstk = hstk.in_space(&space).as_cell()?.tail().noun();
                i += 1;
                break;
            }
        }
    }

    pub fn scag(context: &mut Context, a: Atom, b: Noun) -> Result {
        // Accepts an atom a and list b, producing the first a elements of the front of the list.
        let space = context.stack.noun_space();
        let a = a.in_space(&space).as_u64()?;
        let mut res: Vec<Noun> = vec![];
        let mut list = b;
        let mut pos = 0;
        loop {
            if unsafe { list.raw_equals(&D(0)) } {
                break;
            }
            let current_cell = list.in_space(&space).as_cell()?;
            if pos >= a {
                break;
            }
            res.push(current_cell.head().noun());
            list = current_cell.tail().noun();
            pos += 1;
        }

        let mut res_cell = D(0);
        while let Some(n) = res.pop() {
            res_cell = T(&mut context.stack, &[n, res_cell]);
        }
        Ok(res_cell)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jets::util::test::{assert_jet, assert_jet_err, init_context};
    use crate::jets::util::BAIL_EXIT;
    use crate::noun::{D, T};

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_flop() {
        let c = &mut init_context();

        let sam = T(&mut c.stack, &[D(1), D(2), D(3), D(0)]);
        let res = T(&mut c.stack, &[D(3), D(2), D(1), D(0)]);
        assert_jet(c, jet_flop, sam, res);

        #[rustfmt::skip]
        let sam = T(
            &mut c.stack,
            &[
                D(0xd), D(0xe), D(0xa), D(0xd), D(0xb), D(0xe), D(0xe), D(0xf),
                D(0x1), D(0x2), D(0x3), D(0x4), D(0x5), D(0x6), D(0x7), D(0x8),
                D(0xf), D(0xe), D(0xd), D(0xc), D(0xb), D(0xa), D(0x9), D(0x8),
                D(0x7), D(0x6), D(0x5), D(0x4), D(0x3), D(0x2), D(0x1), D(0x0),
                D(0x0),
            ],
        );
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
        assert_jet(c, jet_flop, sam, res);

        assert_jet_err(c, jet_flop, D(1), BAIL_EXIT);
        let sam = T(&mut c.stack, &[D(1), D(2), D(3)]);
        assert_jet_err(c, jet_flop, sam, BAIL_EXIT);
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_lent() {
        let c = &mut init_context();

        assert_jet(c, jet_lent, D(0), D(0));
        let sam = T(&mut c.stack, &[D(1), D(2), D(3), D(0)]);
        assert_jet(c, jet_lent, sam, D(3));
        let sam = T(&mut c.stack, &[D(3), D(2), D(1), D(0)]);
        assert_jet(c, jet_lent, sam, D(3));
        assert_jet_err(c, jet_lent, D(1), BAIL_EXIT);
        let sam = T(&mut c.stack, &[D(3), D(2), D(1)]);
        assert_jet_err(c, jet_lent, sam, BAIL_EXIT);
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_snag() {
        let c = &mut init_context();
        let list1 = T(&mut c.stack, &[D(1), D(2), D(3), D(0)]);
        let sam = T(&mut c.stack, &[D(1), list1]);
        assert_jet(c, jet_snag, sam, D(2));

        let list2 = T(&mut c.stack, &[D(1), D(0)]);
        let sam = T(&mut c.stack, &[D(0), list2]);
        assert_jet(c, jet_snag, sam, D(1));

        let sam = T(&mut c.stack, &[D(3), list1]);
        assert_jet_err(c, jet_snag, sam, BAIL_EXIT);

        let sam = T(&mut c.stack, &[D(0), D(0)]);
        assert_jet_err(c, jet_snag, sam, BAIL_EXIT);
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_snip() {
        let c = &mut init_context();

        let sam = T(&mut c.stack, &[D(1), D(0)]);
        assert_jet(c, jet_snip, sam, D(0));

        let sam = T(&mut c.stack, &[D(1), D(2), D(0)]);
        let res = T(&mut c.stack, &[D(1), D(0)]);
        assert_jet(c, jet_snip, sam, res);

        let sam = T(&mut c.stack, &[D(1), D(2), D(3), D(0)]);
        let res = T(&mut c.stack, &[D(1), D(2), D(0)]);
        assert_jet(c, jet_snip, sam, res);

        let pair = T(&mut c.stack, &[D(1), D(2)]);
        let sam = T(&mut c.stack, &[pair, pair, pair, D(0)]);
        let res = T(&mut c.stack, &[pair, pair, D(0)]);
        assert_jet(c, jet_snip, sam, res);

        let sam = T(&mut c.stack, &[D(1), D(2), D(3)]);
        assert_jet_err(c, jet_snip, sam, BAIL_EXIT);

        assert_jet(c, jet_snip, D(0), D(0));
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_zing() {
        let c = &mut init_context();

        let list_0 = T(&mut c.stack, &[D(0), D(0), D(0), D(0)]);
        let list_1 = T(&mut c.stack, &[D(1), D(2), D(3), D(0)]);
        let list_2 = T(&mut c.stack, &[D(4), D(5), D(6), D(0)]);
        let list_3 = T(&mut c.stack, &[D(1), D(2), D(3), D(4), D(5), D(6), D(0)]);

        assert_jet(c, jet_zing, D(0), D(0));
        assert_jet(c, jet_zing, list_0, D(0));
        let sam = T(&mut c.stack, &[list_0, D(0)]);
        assert_jet(c, jet_zing, sam, list_0);
        let sam = T(&mut c.stack, &[list_1, list_2, D(0)]);
        assert_jet(c, jet_zing, sam, list_3);
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_weld() {
        let c = &mut init_context();
        let list_1 = T(&mut c.stack, &[D(1), D(2), D(3), D(0)]);
        let list_2 = T(&mut c.stack, &[D(4), D(5), D(6), D(0)]);
        let list_3 = T(&mut c.stack, &[D(1), D(2), D(3), D(4), D(5), D(6), D(0)]);

        let sam1 = T(&mut c.stack, &[D(0), D(0)]);
        assert_jet(c, jet_weld, sam1, D(0));

        let sam2 = T(&mut c.stack, &[D(0), list_1]);
        assert_jet(c, jet_weld, sam2, list_1);

        let sam3 = T(&mut c.stack, &[list_1, D(0)]);
        assert_jet(c, jet_weld, sam3, list_1);

        let sam4 = T(&mut c.stack, &[list_1, list_2]);
        assert_jet(c, jet_weld, sam4, list_3);
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_reap() {
        let c = &mut init_context();

        assert_jet_err(c, jet_reap, D(0), BAIL_EXIT);

        let sam = T(&mut c.stack, &[D(0), D(3)]);
        assert_jet(c, jet_reap, sam, D(0));

        let sam = T(&mut c.stack, &[D(1), D(3)]);
        let res = T(&mut c.stack, &[D(3), D(0)]);
        assert_jet(c, jet_reap, sam, res);

        let sam = T(&mut c.stack, &[D(2), D(3)]);
        let res = T(&mut c.stack, &[D(3), D(3), D(0)]);
        assert_jet(c, jet_reap, sam, res);

        let c34 = T(&mut c.stack, &[D(3), D(4)]);
        let sam = T(&mut c.stack, &[D(2), c34]);
        let res = T(&mut c.stack, &[c34, c34, D(0)]);
        assert_jet(c, jet_reap, sam, res);
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_find() {
        let c = &mut init_context();

        let c3 = T(&mut c.stack, &[D(3), D(0)]);
        let c33 = T(&mut c.stack, &[D(3), D(3), D(0)]);
        let c41 = T(&mut c.stack, &[D(4), D(1), D(0)]);
        let c123 = T(&mut c.stack, &[D(1), D(2), D(3), D(0)]);
        let c13413 = T(&mut c.stack, &[D(1), D(3), D(4), D(1), D(3), D(0)]);
        let c13313 = T(&mut c.stack, &[D(1), D(3), D(3), D(1), D(3), D(0)]);
        let c1341342 = T(
            &mut c.stack,
            &[D(1), D(3), D(4), D(1), D(3), D(4), D(2), D(0)],
        );

        let sam = T(&mut c.stack, &[D(0), D(0)]);
        let res = D(0);
        assert_jet(c, jet_find, sam, res);

        let sam = T(&mut c.stack, &[D(0), c123]);
        let res = D(0);
        assert_jet(c, jet_find, sam, res);

        let sam = T(&mut c.stack, &[c123, D(0)]);
        let res = D(0);
        assert_jet(c, jet_find, sam, res);

        let sam = T(&mut c.stack, &[c3, c123]);
        let res = T(&mut c.stack, &[D(0), D(2)]);
        assert_jet(c, jet_find, sam, res);

        let sam = T(&mut c.stack, &[c3, c33]);
        let res = T(&mut c.stack, &[D(0), D(0)]);
        assert_jet(c, jet_find, sam, res);

        let sam = T(&mut c.stack, &[c3, c13413]);
        let res = T(&mut c.stack, &[D(0), D(1)]);
        assert_jet(c, jet_find, sam, res);

        let sam = T(&mut c.stack, &[c3, c13313]);
        let res = T(&mut c.stack, &[D(0), D(1)]);
        assert_jet(c, jet_find, sam, res);

        let sam = T(&mut c.stack, &[c33, c13313]);
        let res = T(&mut c.stack, &[D(0), D(1)]);
        assert_jet(c, jet_find, sam, res);

        let sam = T(&mut c.stack, &[c41, c1341342]);
        let res = T(&mut c.stack, &[D(0), D(2)]);
        assert_jet(c, jet_find, sam, res);
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_scag() {
        let c = &mut init_context();

        // let ab00 = T(&mut c.stack, &[D(0), D(0)]);
        // let ab01 = T(&mut c.stack, &[D(0), D(1)]);
        // let ab02 = T(&mut c.stack, &[D(0), D(2)]);
        // let ab21 = T(&mut c.stack, &[D(2), D(1)]);
        // let ab32 = T(&mut c.stack, &[D(3), D(2)]);
        let c1341342 = T(
            &mut c.stack,
            &[D(1), D(3), D(4), D(1), D(3), D(4), D(2), D(0)],
        );

        let sam = T(&mut c.stack, &[D(0), c1341342]);
        let res = D(0);
        assert_jet(c, jet_scag, sam, res);

        let sam = T(&mut c.stack, &[D(1), c1341342]);
        let res = T(&mut c.stack, &[D(1), D(0)]);
        assert_jet(c, jet_scag, sam, res);

        let sam = T(&mut c.stack, &[D(2), c1341342]);
        let res = T(&mut c.stack, &[D(1), D(3), D(0)]);
        assert_jet(c, jet_scag, sam, res);

        let sam = T(&mut c.stack, &[D(3), c1341342]);
        let res = T(&mut c.stack, &[D(1), D(3), D(4), D(0)]);
        assert_jet(c, jet_scag, sam, res);

        let sam = T(&mut c.stack, &[D(99), c1341342]);
        let res = c1341342;
        assert_jet(c, jet_scag, sam, res);
    }
}
