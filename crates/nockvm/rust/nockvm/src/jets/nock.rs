/** Virtualization jets
 */
use crate::interpreter::Context;
use crate::jets::util::slot;
use crate::jets::{JetErr, Result};
use crate::noun::{CellHandle, Noun, D, NO, T};

crate::gdb!();

pub fn jet_mink(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let arg = slot(subject, 6, &space)?;
    // mink sample = [nock scry_namespace]
    //             = [[subject formula] scry_namespace]
    let v_subject = slot(arg, 4, &space)?;
    let v_formula = slot(arg, 5, &space)?;
    let scry_handler = slot(arg, 3, &space)?;

    // Implicit error conversion
    Ok(util::mink(context, v_subject, v_formula, scry_handler)?)
}

pub fn jet_mole(context: &mut Context, subject: Noun) -> Result {
    jet_mure(context, subject)
}

pub fn jet_mule(context: &mut Context, subject: Noun) -> Result {
    jet_mute(context, subject)
}

pub fn jet_mure(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let tap = slot(subject, 6, &space)?;
    let fol = util::slam_gate_fol(&mut context.stack);
    let scry = util::pass_thru_scry(&mut context.stack);

    match util::mink(context, tap, fol, scry) {
        Ok(tone) => {
            let tone_cell = tone.in_space(&space).as_cell()?;
            if unsafe { tone_cell.head().noun().raw_equals(&D(0)) } {
                Ok(tone)
            } else {
                Ok(D(0))
            }
        }
        Err(err) => Err(JetErr::Fail(err)),
    }
}

pub fn jet_mute(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.noun_space();
    let tap = slot(subject, 6, &space)?;
    let fol = util::slam_gate_fol(&mut context.stack);
    let scry = util::pass_thru_scry(&mut context.stack);

    let tone = util::mink(context, tap, fol, scry);

    match util::mook(context, tone?.as_cell()?, false) {
        Ok(toon) => {
            let toon_handle = CellHandle::new(toon, &space);
            match toon_handle.head().noun() {
                x if unsafe { x.raw_equals(&D(0)) } => Ok(toon.as_noun()),
                x if unsafe { x.raw_equals(&D(1)) } => {
                    //  XX: Need to check that result is actually of type path
                    //      return [[%leaf "mute.hunk"] ~] if not
                    let bon = util::smyt(&mut context.stack, toon_handle.tail().noun(), &space)?;
                    Ok(T(&mut context.stack, &[NO, bon, D(0)]))
                }
                x if unsafe { x.raw_equals(&D(2)) } => {
                    Ok(T(&mut context.stack, &[NO, toon_handle.tail().noun()]))
                }
                _ => panic!("serf: mook: invalid toon"),
            }
        }
        Err(err) => Err(JetErr::Fail(err)),
    }
}

pub mod util {
    use std::result;

    use either::{Left, Right};
    use nockvm_macros::tas;

    use crate::hamt::Hamt;
    use crate::interpreter::{interpret, Context, Error, Mote};
    use crate::jets;
    use crate::jets::bits::util::rip;
    use crate::jets::form::util::scow;
    use crate::mem::NockStack;
    use crate::noun::{tape, Cell, CellHandle, Noun, NounSpace, D, T};

    pub const LEAF: Noun = D(tas!(b"leaf"));
    pub const ROSE: Noun = D(tas!(b"rose"));

    /// The classic "slam gate" formula.
    pub fn slam_gate_fol(stack: &mut NockStack) -> Noun {
        T(stack, &[D(9), D(2), D(0), D(1)])
    }

    /// The classic "pass-through" scry handler.
    pub fn pass_thru_scry(stack: &mut NockStack) -> Noun {
        // .*  0  !=  =>  ~  |=(a=^ ``.*(a [%12 [%0 2] %0 3]))
        // [[[1 0] [1 0] 2 [0 6] 1 12 [0 2] 0 3] [0 0] 0]
        let sig = T(stack, &[D(1), D(0)]);
        let sam = T(stack, &[D(0), D(6)]);
        let hed = T(stack, &[D(0), D(2)]);
        let tel = T(stack, &[D(0), D(3)]);
        let zap = T(stack, &[D(0), D(0)]);

        let cry = T(stack, &[D(12), hed, tel]);
        let fol = T(stack, &[D(1), cry]);
        let res = T(stack, &[D(2), sam, fol]);
        let uno = T(stack, &[sig, res]);
        let dos = T(stack, &[sig, uno]);

        let gat = T(stack, &[zap, D(0)]);

        T(stack, &[dos, gat])
    }

    /// The "always-fail" scry
    pub fn null_scry(stack: &mut NockStack) -> Noun {
        // .*  0  !=  =>  ~  |=(^ ~)
        // [[1 0] [0 0] 0]
        let sig = T(stack, &[D(1), D(0)]);
        let zap = T(stack, &[D(0), D(0)]);

        T(stack, &[sig, zap, D(0)])
    }

    // Deterministic scry crashes should have the following behaviour:
    //
    //  root                <-- bail, %crud
    //      mink            <-- return Deterministic
    //  scry                <-- return ScryCrashed
    //
    //  root                <-- bail, %crud
    //      mink            <-- return Deterministic
    //          mink        <-- return ScryCrashed
    //      scry            <-- return ScryCrashed
    //  scry                <-- return ScryCrashed
    //
    //  root
    //      mink            <-- return Tone
    //          mink        <-- return Deterministic
    //              mink    <-- return ScryCrashed
    //          scry        <-- return ScryCrashed
    //      scry            <-- return ScryCrashed
    //  scry
    //
    pub fn mink(
        context: &mut Context,
        subject: Noun,
        formula: Noun,
        scry: Noun,
    ) -> Result<Noun, Error> {
        let cache_snapshot = context.cache;
        let scry_snapshot = context.scry_stack;

        context.cache = Hamt::<Noun>::new(&mut context.stack);
        context.scry_stack = T(&mut context.stack, &[scry, context.scry_stack]);

        match interpret(context, subject, formula) {
            Ok(res) => {
                context.cache = cache_snapshot;
                context.scry_stack = scry_snapshot;
                Ok(T(&mut context.stack, &[D(0), res]))
            }
            Err(err) => match err {
                Error::ScryBlocked(path) => {
                    context.cache = cache_snapshot;
                    context.scry_stack = scry_snapshot;
                    Ok(T(&mut context.stack, &[D(1), path]))
                }
                Error::Deterministic(_, trace) => {
                    context.cache = cache_snapshot;
                    context.scry_stack = scry_snapshot;
                    Ok(T(&mut context.stack, &[D(2), trace]))
                }
                Error::ScryCrashed(trace) => {
                    context.cache = cache_snapshot;
                    // When we enter a +mink call, we record the state of the scry handler stack at the
                    // time (i.e. the Noun representing (list scry)). Each scry will pop the head off of
                    // this scry handler stack and calls interpret(), using the rest of the scry handler
                    // stack in the event that it scries again recursively. When a scry succeeds, it
                    // replaces the scry handler that it used by pushing it back onto the top of the
                    // scry handler stack. However, it never does so when it fails. Therefore, we can
                    // tell which particular virtualization instance failed by comparing the scry
                    // handler stack at the time of failure (i.e. the scry handler stack in the context
                    // after a failed scry) with the scry handler stack at the time of the virtualization
                    // call. Thus, whenever a virtualized interpret() call fails with a
                    // Error::ScryCrashed, jet_mink() compnockvm the two scry handler stack Nouns> If they
                    // are identical, jet_mink() bails with Error::Deterministic. Otherwise, it forwards
                    // the Error::ScryCrashed to the senior virtualization call.
                    if unsafe { context.scry_stack.raw_equals(&scry_snapshot) } {
                        Err(Error::Deterministic(Mote::Exit, trace))
                    } else {
                        Err(err)
                    }
                }
                Error::NonDeterministic(_, _) => {
                    // We choose to restore the cache and scry stack even on NonDeterministic errors
                    // to keep the logic all in one place (as opposed to having the serf reset them
                    // manually ONLY for NonDeterministic errors).
                    context.cache = cache_snapshot;
                    context.scry_stack = scry_snapshot;
                    Err(err)
                }
            },
        }
    }

    /** Consume $tone, produce $toon
     */
    //  XX: should write a jet_mook wrapper for this function
    pub fn mook(context: &mut Context, tone: Cell, flop: bool) -> result::Result<Cell, Error> {
        let space = context.stack.noun_space();
        let tone_handle = CellHandle::new(tone, &space);
        let tag = tone_handle.head().noun().as_direct()?;
        let original_list = tone_handle.tail().noun();

        if (tag.data() != 2) | unsafe { original_list.raw_equals(&D(0)) } {
            return Ok(tone);
        } else if original_list.atom().is_some() {
            return Err(Error::Deterministic(Mote::Exit, D(0)));
        }

        // XX: trim traces longer than 1024 frames

        unsafe {
            let mut res = D(0);
            let mut dest = &mut res as *mut Noun;

            let mut list = original_list;
            while !list.raw_equals(&D(0)) {
                let cell = list.in_space(&space).as_cell()?;
                let trace = cell.head().as_cell()?;
                let tag = trace.head().noun().as_direct()?;
                let dat = trace.tail().noun();

                let tank: Noun = match tag.data() {
                    tas!(b"hunk") => match dat.as_either_atom_cell() {
                        Left(_) => {
                            let stack = &mut context.stack;
                            let tape = tape(stack, "mook.hunk");
                            T(stack, &[LEAF, tape])
                        }
                        Right(cell) => {
                            //  XX: need to check that this is actually a path
                            //      return leaf+"mook.hunk" if not
                            let path = CellHandle::new(cell, &space).tail().noun();
                            smyt(&mut context.stack, path, &space)?
                        }
                    },
                    tas!(b"mean") => match dat.as_either_atom_cell() {
                        Left(atom) => {
                            let stack = &mut context.stack;
                            let tape = rip(stack, 3, 1, atom, &space)?;
                            T(stack, &[LEAF, tape])
                        }
                        Right(cell) => {
                            'tank: {
                                let scry = null_scry(&mut context.stack);
                                // if +mink didn't crash...
                                let cell_handle = CellHandle::new(cell, &space);
                                if let Ok(tone) =
                                    mink(context, dat, cell_handle.head().noun(), scry)
                                {
                                    if let Some(tonc) = tone.cell() {
                                        let tonc_handle = CellHandle::new(tonc, &space);
                                        // ...and +mink didn't fail or block...
                                        if tonc_handle.head().noun().raw_equals(&D(0)) {
                                            // ...return $tank from $tone
                                            //  XX: need to check that this is
                                            //      actually a tank;
                                            //      return leaf+"mook.mean" if not
                                            break 'tank tonc_handle.tail().noun();
                                        }
                                    } else {
                                        panic!("+mink in +mook somehow returned atom {tone:?}")
                                    }
                                }

                                // This code only called when the break statement
                                // above doesn't trigger
                                let stack = &mut context.stack;
                                let tape = tape(stack, "####");
                                T(stack, &[LEAF, tape])
                            }
                        }
                    },
                    tas!(b"spot") => {
                        let stack = &mut context.stack;

                        let spot = dat.in_space(&space).as_cell()?;
                        let pint = spot.tail().as_cell()?;
                        let pstr = pint.head().as_cell()?;
                        let pend = pint.tail().as_cell()?;

                        let colo = T(stack, &[D(b':' as u64), D(0)]);
                        let trel = T(stack, &[colo, D(0), D(0)]);

                        let smyt = smyt(stack, spot.head().noun(), &space)?;

                        let aura = D(tas!(b"ud")).as_direct()?;
                        let str_lin = scow(stack, aura, pstr.head().as_atom()?.atom(), &space)?;
                        let str_col = scow(stack, aura, pstr.tail().as_atom()?.atom(), &space)?;
                        let end_lin = scow(stack, aura, pend.head().as_atom()?.atom(), &space)?;
                        let end_col = scow(stack, aura, pend.tail().as_atom()?.atom(), &space)?;

                        let mut list = end_col.as_cell()?;
                        loop {
                            let list_handle = CellHandle::new(list, &space);
                            if list_handle.tail().atom().is_some() {
                                break;
                            }
                            list = list_handle.tail().noun().as_cell()?;
                        }
                        // "{end_col}]>"
                        let p4 = T(stack, &[D(b']' as u64), D(b'>' as u64), D(0)]);
                        (*list.tail_as_mut(&space)) = p4;

                        list = end_lin.as_cell()?;
                        loop {
                            let list_handle = CellHandle::new(list, &space);
                            if list_handle.tail().atom().is_some() {
                                break;
                            }
                            list = list_handle.tail().noun().as_cell()?;
                        }
                        // "{end_lin} {end_col}]>"
                        let p3 = T(stack, &[D(b' ' as u64), end_col]);
                        (*list.tail_as_mut(&space)) = p3;

                        list = str_col.as_cell()?;
                        loop {
                            let list_handle = CellHandle::new(list, &space);
                            if list_handle.tail().atom().is_some() {
                                break;
                            }
                            list = list_handle.tail().noun().as_cell()?;
                        }
                        // "{str_col}].[{end_lin} {end_col}]>"
                        let p2 = T(
                            stack,
                            &[D(b']' as u64), D(b'.' as u64), D(b'[' as u64), end_lin],
                        );
                        (*list.tail_as_mut(&space)) = p2;

                        list = str_lin.as_cell()?;
                        loop {
                            let list_handle = CellHandle::new(list, &space);
                            if list_handle.tail().atom().is_some() {
                                break;
                            }
                            list = list_handle.tail().noun().as_cell()?;
                        }
                        // "{str_lin} {str_col}].[{end_lin} {end_col}]>"
                        let p1 = T(stack, &[D(b' ' as u64), str_col]);
                        (*list.tail_as_mut(&space)) = p1;

                        // "<[{str_lin} {str_col}].[{end_lin} {end_col}]>"
                        let tape = T(stack, &[D(b'<' as u64), D(b'[' as u64), str_lin]);
                        let finn = T(stack, &[LEAF, tape]);

                        T(stack, &[ROSE, trel, smyt, finn, D(0)])
                    }
                    _ => {
                        let stack = &mut context.stack;
                        let tape = rip(stack, 3, 1, tag.as_atom(), &space)?;
                        T(
                            stack,
                            &[
                                D(tas!(b"m")),
                                D(tas!(b"o")),
                                D(tas!(b"o")),
                                D(tas!(b"k")),
                                D(tas!(b".")),
                                tape,
                            ],
                        )
                    } // XX: TODO
                      //  %hand
                      //  %lose
                };

                if flop {
                    res = T(&mut context.stack, &[tank, res]);
                } else {
                    let (new_cell, new_memory) = Cell::new_raw_mut(&mut context.stack);
                    (*new_memory).head = tank;
                    *dest = new_cell.as_noun();
                    dest = &mut (*new_memory).tail;
                }

                list = cell.tail().noun();
            }

            *dest = D(0);
            let toon = Cell::new(&mut context.stack, D(2), res);
            Ok(toon)
        }
    }

    pub fn smyt(stack: &mut NockStack, path: Noun, space: &NounSpace) -> jets::Result {
        let lash = D(tas!(b"/"));
        let zero = D(0);
        let sep = T(stack, &[lash, zero]);

        let trel = T(stack, &[sep, sep, zero]);
        let tank = smyt_help(stack, path, space)?;

        Ok(T(stack, &[ROSE, trel, tank]))
    }

    fn smyt_help(stack: &mut NockStack, path: Noun, space: &NounSpace) -> jets::Result {
        //  XX: switch to using Cell:new_raw_mut
        if unsafe { path.raw_equals(&D(0)) } {
            return Ok(D(0));
        }

        let cell = path.in_space(space).as_cell()?;
        let tail = smyt_help(stack, cell.tail().noun(), space)?;
        let trip = rip(stack, 3, 1, cell.head().as_atom()?.atom(), space)?;
        let head = T(stack, &[LEAF, trip]);

        Ok(T(stack, &[head, tail]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jets::util::test::{assert_jet, init_context};
    use crate::mem::NockStack;
    use crate::noun::{D, T};

    #[test]
    fn init() {
        // This needs to be done because TERMINATOR is lazy allocated, and if you don't
        // do it before you call the unit tests it'll get allocated on the Rust heap
        // inside an assert_no_alloc block.
        //
        // Also Rust has no primitive for pre-test setup / post-test teardown, so we
        // do it in a test that we rely on being called before any other in this file,
        // since we're already using single-threaded test mode to avoid race conditions
        // (because Rust doesn't support test order dependencies either).
    }

    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_mink_success() {
        let context = &mut init_context();
        let stack = &mut context.stack;

        let subj = D(0);
        let form = T(stack, &[D(1), D(53)]);
        let nock = T(stack, &[subj, form]);
        let scry = D(0);
        let samp = T(stack, &[nock, scry]);
        let rest = T(stack, &[D(0), D(53)]);
        assert_jet(context, jet_mink, samp, rest);
    }

    // FIXME: This test is failing because (we believe that) the stack traces are formatted differently.
    #[test]
    #[ignore]
    fn test_mink_zapzap() {
        let context = &mut init_context();
        let stack = &mut context.stack;

        let subj = D(0);
        let form = T(stack, &[D(0), D(0)]);
        let nock = T(stack, &[subj, form]);
        let scry = D(0);
        let samp = T(stack, &[nock, scry]);
        let rest = T(stack, &[D(2), D(0), D(0)]);
        assert_jet(context, jet_mink, samp, rest);
    }

    // FIXME: This test is failing because (we believe that) the stack traces are formatted differently.
    #[test]
    #[ignore]
    fn test_mink_trace() {
        let context = &mut init_context();
        let stack = &mut context.stack;
        let subj = D(0);
        let scry = D(0);

        //  !=  !:  ?~  0  !!  53
        //  [ 11
        //    [1.953.460.339 1 [1.953.719.668 0] [1 9] [1 22]]
        //    [ 8
        //      [11 [1.953.460.339 1 [1.953.719.668 0] [1 13] [1 14]] [1 0]]
        //      [ 6
        //        [5 [1 0] [0 2]]
        //        [7 [0 3] [11 [1.953.460.339 1 [1.953.719.668 0] [1 16] [1 18]] [0 0]]]
        //        [7 [0 3] [11 [1.953.460.339 1 [1.953.719.668 0] [1 20] [1 22]] [1 53]]]
        //      ]
        //    ]
        //  ]
        //
        //  1.953.719.668 = 'test'
        //  1.953.460.339 = 'spot'
        //
        //  All of this below is because of "two-phase borrow checks"
        //      https://stackoverflow.com/questions/60686259/mutable-borrow-in-function-argument

        let hint_spot = D(1953460339);
        let hint_path = T(stack, &[D(1953719668), D(0)]);
        let hint_dyn = D(1);
        let hint_row = D(1);

        let make_hint = |stack: &mut NockStack, col_start: u64, col_end: u64| {
            let start = T(stack, &[hint_row, D(col_start)]);
            let end = T(stack, &[hint_row, D(col_end)]);

            T(stack, &[hint_spot, hint_dyn, hint_path, start, end])
        };

        let sss3s1 = T(stack, &[D(0), D(3)]);
        let sss3s2s1 = make_hint(stack, 20, 22);
        let sss3s2s2 = T(stack, &[D(1), D(53)]);
        let sss3s2 = T(stack, &[D(11), sss3s2s1, sss3s2s2]);
        let sss3 = T(stack, &[D(7), sss3s1, sss3s2]);

        let sss2s1 = sss3s1;
        let sss2s2s1 = make_hint(stack, 16, 18);
        let sss2s2s2 = T(stack, &[D(0), D(0)]);
        let sss2s2 = T(stack, &[D(11), sss2s2s1, sss2s2s2]);
        let sss2 = T(stack, &[D(7), sss2s1, sss2s2]);

        let sss1s1 = T(stack, &[D(1), D(0)]);
        let sss1s2 = T(stack, &[D(0), D(2)]);
        let sss1 = T(stack, &[D(5), sss1s1, sss1s2]);

        let ss2 = T(stack, &[D(6), sss1, sss2, sss3]);

        let ss1s1 = make_hint(stack, 13, 14);
        let ss1s2 = sss1s1;
        let ss1 = T(stack, &[D(11), ss1s1, ss1s2]);

        let s2 = T(stack, &[D(8), ss1, ss2]);
        let s1 = make_hint(stack, 9, 22);
        let form = T(stack, &[D(11), s1, s2]);

        let nock = T(stack, &[subj, form]);
        let samp = T(stack, &[nock, scry]);

        //  trace
        //  [%2 trace=~[[~.spot [[1.953.719.668 0] [1 16] 1 18]] [~.spot [[1.953.719.668 0] [1 9] 1 22]]]]
        let ttt2t1 = T(stack, &[D(1), D(9)]);
        let ttt2t2 = T(stack, &[D(1), D(22)]);
        let ttt2 = T(stack, &[hint_path, ttt2t1, ttt2t2]);

        let ttt1t1 = T(stack, &[D(1), D(16)]);
        let ttt1t2 = T(stack, &[D(1), D(18)]);
        let ttt1 = T(stack, &[hint_path, ttt1t1, ttt1t2]);

        let tt2 = T(stack, &[hint_spot, ttt2]);
        let tt1 = T(stack, &[hint_spot, ttt1]);

        let t1 = T(stack, &[tt1, tt2, D(0)]);

        let rest = T(stack, &[D(2), t1, D(0)]);

        assert_jet(context, jet_mink, samp, rest);
    }
}
