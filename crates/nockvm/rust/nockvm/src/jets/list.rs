/** Text processing jets
 */
use crate::interpreter::Context;
use crate::jets::util::{slot, BAIL_FAIL};
use crate::jets::Result;
use crate::noun::{Cell, Noun, D, T};
use crate::site::{site_slam, Site};

crate::gdb!();

pub fn jet_weld(context: &mut Context, subject: Noun) -> Result {
    let sam = slot(subject, 6)?;
    let a = slot(sam, 2)?;
    let b = slot(sam, 3)?;
    util::weld(&mut context.stack, a, b)
}

pub fn jet_flop(context: &mut Context, subject: Noun) -> Result {
    let sam = slot(subject, 6)?;
    util::flop(&mut context.stack, sam)
}

pub fn jet_lent(_context: &mut Context, subject: Noun) -> Result {
    let list = slot(subject, 6)?;
    util::lent(list).map(|x| D(x as u64))
}

pub fn jet_roll(context: &mut Context, subject: Noun) -> Result {
    let sample = slot(subject, 6)?;
    let mut list = slot(sample, 2)?;
    let mut gate = slot(sample, 3)?;
    let mut prod = slot(gate, 13)?;

    let site = Site::new(context, &mut gate);
    loop {
        if let Ok(list_cell) = list.as_cell() {
            list = list_cell.tail();
            let sam = T(&mut context.stack, &[list_cell.head(), prod]);
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
    let sam = slot(subject, 6)?;
    let index = slot(sam, 2)?;
    let list = slot(sam, 3)?;

    util::snag(list, index)
}

pub fn jet_snip(context: &mut Context, subject: Noun) -> Result {
    let list = slot(subject, 6)?;
    util::snip(&mut context.stack, list)
}

pub fn jet_turn(context: &mut Context, subject: Noun) -> Result {
    let sample = slot(subject, 6)?;
    let mut list = slot(sample, 2)?;
    let mut gate = slot(sample, 3)?;
    let mut res = D(0);
    let mut dest: *mut Noun = &mut res; // Mutable pointer because we cannot guarantee initialized

    // Since the gate doesn't change, we can do a single jet check and use that through the whole
    // loop
    let site = Site::new(context, &mut gate);
    loop {
        if let Ok(list_cell) = list.as_cell() {
            list = list_cell.tail();
            unsafe {
                let (new_cell, new_mem) = Cell::new_raw_mut(&mut context.stack);
                (*new_mem).head = site_slam(context, &site, list_cell.head())?;
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
    let list = slot(subject, 6)?;
    let stack = &mut context.stack;

    util::zing(stack, list)
}

pub fn jet_reap(context: &mut Context, subject: Noun) -> Result {
    let sam = slot(subject, 6)?;
    let a_noun = slot(sam, 2)?;
    let b_noun = slot(sam, 3)?;

    let a = a_noun.as_atom()?.as_u64()?;
    util::reap(&mut context.stack, a, b_noun)
}

pub fn jet_levy(context: &mut Context, subject: Noun) -> Result {
    let sam = slot(subject, 6)?;
    let a_noun = slot(sam, 2)?;
    let b_noun = slot(sam, 3)?;

    util::levy(context, a_noun, b_noun)
}

pub fn jet_find(context: &mut Context, subject: Noun) -> Result {
    let sam = slot(subject, 6)?;
    let nedl = slot(sam, 2)?;
    let hstk = slot(sam, 3)?;

    util::find(context, nedl, hstk)
}

pub fn jet_scag(context: &mut Context, subject: Noun) -> Result {
    let sam = slot(subject, 6)?;
    let a = sam.as_cell()?.head().as_atom()?;
    let b = sam.as_cell()?.tail();

    util::scag(context, a, b)
}

pub mod util {
    use std::result;

    use crate::interpreter::Context;
    use crate::jets::util::BAIL_EXIT;
    use crate::jets::{JetErr, Result};
    use crate::mem::NockStack;
    use crate::noun::{Atom, Cell, Noun, NounAllocator, D, NO, T, YES};
    use crate::site::{site_slam, Site};

    /// Reverse order of list
    pub fn flop<T: NounAllocator>(alloc: &mut T, noun: Noun) -> Result {
        let mut list = noun;
        let mut tsil = D(0);
        loop {
            if unsafe { list.raw_equals(&D(0)) } {
                break;
            }

            let cell = list.as_cell()?;
            tsil = T(alloc, &[cell.head(), tsil]);
            list = cell.tail();
        }

        Ok(tsil)
    }

    pub fn weld<A: NounAllocator>(alloc: &mut A, a: Noun, b: Noun) -> Result {
        let mut res = D(0);
        let mut cur = a;
        loop {
            if unsafe { cur.raw_equals(&D(0)) } {
                break;
            }
            let cell = cur.as_cell()?;
            res = T(alloc, &[cell.head(), res]);
            cur = cell.tail();
        }
        cur = b;
        loop {
            if unsafe { cur.raw_equals(&D(0)) } {
                break;
            }
            let cell = cur.as_cell()?;
            res = T(alloc, &[cell.head(), res]);
            cur = cell.tail();
        }
        flop(alloc, res)
    }

    pub fn lent(tape: Noun) -> result::Result<usize, JetErr> {
        let mut len = 0usize;
        let mut list = tape;
        loop {
            if let Some(atom) = list.atom() {
                if atom.as_bitslice().first_one().is_none() {
                    break;
                } else {
                    return Err(BAIL_EXIT);
                }
            }
            let cell = list.as_cell()?;
            // don't need checked_add or indirect atom result: 2^63-1 atoms would be 64 ebibytes
            len += 1;
            list = cell.tail();
        }
        Ok(len)
    }

    pub fn snag(tape: Noun, index: Noun) -> Result {
        let mut list = tape;
        let mut idx = index.as_atom()?.as_u64()? as usize;
        loop {
            if unsafe { list.raw_equals(&D(0)) } {
                return Err(BAIL_EXIT);
            }
            let cell = list.as_cell()?;
            if idx == 0 {
                return Ok(cell.head());
            }
            idx -= 1;
            list = cell.tail();
        }
    }

    pub fn snip(stack: &mut NockStack, tape: Noun) -> Result {
        let mut ret = D(0);
        let mut dest = &mut ret as *mut Noun;
        let mut list = tape;

        if let Some(atom) = list.atom() {
            if atom.as_bitslice().first_one().is_none() {
                return Ok(D(0));
            }
        }

        loop {
            let cell = list.as_cell()?;
            if let Some(atom) = cell.tail().atom() {
                if atom.as_bitslice().first_one().is_none() {
                    break;
                } else {
                    return Err(BAIL_EXIT);
                }
            }
            unsafe {
                let (new_cell, new_mem) = Cell::new_raw_mut(stack);
                (*new_mem).head = cell.head();
                *dest = new_cell.as_noun();
                dest = &mut (*new_mem).tail;
            }
            list = cell.tail();
        }
        unsafe { *dest = D(0) };
        Ok(ret)
    }

    pub fn zing(stack: &mut NockStack, mut list: Noun) -> Result {
        unsafe {
            let mut res: Noun = D(0);
            let mut dest = &mut res as *mut Noun;

            while !list.raw_equals(&D(0)) {
                let pair = list.as_cell()?;
                let mut sublist = pair.head();
                list = pair.tail();

                while !sublist.raw_equals(&D(0)) {
                    let it = sublist.as_cell()?;
                    let i = it.head();
                    sublist = it.tail();

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

        loop {
            if unsafe { list.raw_equals(&D(0)) } {
                return Ok(YES);
            }

            let cell = list.as_cell()?;
            let b_res = site_slam(context, &site, cell.head())?;
            if unsafe { b_res.raw_equals(&NO) } {
                return Ok(NO);
            }
            list = cell.tail();
        }
    }

    pub fn find(context: &mut Context, nedl: Noun, hstk: Noun) -> Result {
        let mut hstk = hstk;
        let mut i = 0;
        loop {
            let mut n = nedl;
            let mut h = hstk;
            loop {
                if unsafe { n.raw_equals(&D(0)) || h.raw_equals(&D(0)) } {
                    // not found
                    return Ok(D(0)); // (unit @ud)  ~
                }

                if unsafe { n.as_cell()?.head().raw_equals(&h.as_cell()?.head()) } {
                    if unsafe { n.as_cell()?.tail().raw_equals(&D(0)) } {
                        // match found
                        return Ok(T(&mut context.stack, &[D(0), D(i)])); // (unit @ud)  i
                    }

                    n = n.as_cell()?.tail();
                    h = h.as_cell()?.tail();
                    continue;
                }

                // try next position
                hstk = hstk.as_cell()?.tail();
                i += 1;
                break;
            }
        }
    }

    pub fn scag(context: &mut Context, a: Atom, b: Noun) -> Result {
        // Accepts an atom a and list b, producing the first a elements of the front of the list.
        let a = a.as_u64()?;
        let mut res: Vec<Noun> = vec![];
        let mut list = b;
        let mut pos = 0;
        loop {
            if unsafe { list.raw_equals(&D(0)) } {
                break;
            }
            let current_cell = list.as_cell()?;
            if pos >= a {
                break;
            }
            res.push(current_cell.head());
            list = current_cell.tail();
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
