use either::{Left, Right};

/** Parsing jets
 */
use crate::interpreter::Context;
use crate::jets::bits::util::met;
use crate::jets::math::util::{gte_b, lte_b, lth_b};
use crate::jets::util::{kick, slam, slam_with_space, slot, BAIL_FAIL};
use crate::jets::Result;
use crate::noun::{Cell, CellHandle, Noun, D, T};

crate::gdb!();

//
//  Text conversion
//
pub fn jet_trip(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.fast_noun_space();
    let sam = slot(subject, 6, &space)?.as_atom()?;
    let chars = met(3, sam, &space);
    if chars == 0 {
        return Ok(D(0));
    };

    let sam_handle = sam.in_space(&space);
    let bytes = &sam_handle.as_ne_bytes()[0..chars];

    let mut result = D(0);
    let mut dest = &mut result as *mut Noun;

    for byte in bytes {
        unsafe {
            let (it, it_mem) = Cell::new_raw_mut(&mut context.stack);

            // safe because a byte can't overflow a direct atom
            (*it_mem).head = D((*byte) as u64);

            *dest = it.as_noun();
            dest = &mut (*it_mem).tail as *mut Noun;
        }
    }
    unsafe { *dest = D(0) };
    Ok(result)
}

//
//  Tracing
//

pub fn jet_last(_context: &mut Context, subject: Noun) -> Result {
    let space = _context.stack.fast_noun_space();
    let sam = slot(subject, 6, &space)?;
    let zyc = slot(sam, 2, &space)?;
    let naz = slot(sam, 3, &space)?;

    util::last(zyc, naz, &space)
}

//
//  Combinators
//

pub fn jet_bend(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.fast_noun_space();
    let sam = slot(subject, 6, &space)?;
    let vex = slot(sam, 2, &space)?.in_space(&space).as_cell()?;
    let sab = slot(sam, 3, &space)?;
    let van = slot(subject, 7, &space)?;
    let raq = slot(van, 6, &space)?;

    let p_vex = vex.head().noun();
    let q_vex = vex.tail().noun();

    if unsafe { q_vex.raw_equals(&D(0)) } {
        return Ok(vex.cell().as_noun());
    }

    let uq_vex = q_vex.in_space(&space).as_cell()?.tail().as_cell()?;
    let puq_vex = uq_vex.head().noun();
    let quq_vex = uq_vex.tail().noun();

    let yit = slam_with_space(context, sab, quq_vex, &space)?
        .in_space(&space)
        .as_cell()?;
    let p_yit = yit.head().noun();
    let q_yit = yit.tail().noun();

    let yur = util::last(p_vex, p_yit, &space)?;

    if unsafe { q_yit.raw_equals(&D(0)) } {
        Ok(T(&mut context.stack, &[yur, q_vex]))
    } else {
        let uq_yit = q_yit.in_space(&space).as_cell()?.tail().as_cell()?;
        let puq_yit = uq_yit.head().noun();
        let quq_yit = uq_yit.tail().noun();

        let arg = T(&mut context.stack, &[puq_vex, puq_yit]);
        let vux = slam_with_space(context, raq, arg, &space)?;

        if unsafe { vux.raw_equals(&D(0)) } {
            Ok(T(&mut context.stack, &[yur, q_vex]))
        } else {
            let q_vux = vux.in_space(&space).as_cell()?.tail().noun();
            Ok(T(&mut context.stack, &[yur, D(0), q_vux, quq_yit]))
        }
    }
}

pub fn jet_comp(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.fast_noun_space();
    let sam = slot(subject, 6, &space)?;
    let vex = slot(sam, 2, &space)?.in_space(&space).as_cell()?;
    let sab = slot(sam, 3, &space)?;
    let van = slot(subject, 7, &space)?;
    let raq = slot(van, 6, &space)?;

    let p_vex = vex.head().noun();
    let q_vex = vex.tail().noun();

    if unsafe { q_vex.raw_equals(&D(0)) } {
        return Ok(vex.cell().as_noun());
    }

    let uq_vex = q_vex.in_space(&space).as_cell()?.tail().as_cell()?;
    let puq_vex = uq_vex.head().noun();
    let quq_vex = uq_vex.tail().noun();

    let yit = slam_with_space(context, sab, quq_vex, &space)?
        .in_space(&space)
        .as_cell()?;
    let p_yit = yit.head().noun();
    let q_yit = yit.tail().noun();

    let yur = util::last(p_vex, p_yit, &space)?;

    if unsafe { q_yit.raw_equals(&D(0)) } {
        Ok(T(&mut context.stack, &[yur, D(0)]))
    } else {
        let uq_yit = q_yit.in_space(&space).as_cell()?.tail().as_cell()?;
        let puq_yit = uq_yit.head().noun();
        let quq_yit = uq_yit.tail().noun();

        let arg = T(&mut context.stack, &[puq_vex, puq_yit]);
        let vux = slam_with_space(context, raq, arg, &space)?;
        Ok(T(&mut context.stack, &[yur, D(0), vux, quq_yit]))
    }
}

pub fn jet_glue(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.fast_noun_space();
    let sam = slot(subject, 6, &space)?;
    let vex = slot(sam, 2, &space)?.in_space(&space).as_cell()?;
    let sab = slot(sam, 3, &space)?;
    let van = slot(subject, 7, &space)?;
    let bus = slot(van, 6, &space)?;

    let p_vex = vex.head().noun();
    let q_vex = vex.tail().noun();

    if unsafe { q_vex.raw_equals(&D(0)) } {
        return Ok(vex.cell().as_noun());
    }

    let uq_vex = q_vex.in_space(&space).as_cell()?.tail().as_cell()?;
    let puq_vex = uq_vex.head().noun();
    let quq_vex = uq_vex.tail().noun();

    let yit = slam_with_space(context, bus, quq_vex, &space)?
        .in_space(&space)
        .as_cell()?;
    let p_yit = yit.head().noun();
    let q_yit = yit.tail().noun();

    let yur = util::last(p_vex, p_yit, &space)?;

    if unsafe { q_yit.raw_equals(&D(0)) } {
        Ok(T(&mut context.stack, &[yur, D(0)]))
    } else {
        let uq_yit = q_yit.in_space(&space).as_cell()?.tail().as_cell()?;
        let quq_yit = uq_yit.tail().noun();

        let wam = slam_with_space(context, sab, quq_yit, &space)?
            .in_space(&space)
            .as_cell()?;
        let p_wam = wam.head().noun();
        let q_wam = wam.tail().noun();

        let goy = util::last(yur, p_wam, &space)?;

        if unsafe { q_wam.raw_equals(&D(0)) } {
            Ok(T(&mut context.stack, &[goy, D(0)]))
        } else {
            let uq_wam = q_wam.in_space(&space).as_cell()?.tail().as_cell()?;
            let puq_wam = uq_wam.head().noun();
            let quq_wam = uq_wam.tail().noun();

            let puq_arg = T(&mut context.stack, &[puq_vex, puq_wam]);
            Ok(T(&mut context.stack, &[goy, D(0x0), puq_arg, quq_wam]))
        }
    }
}

pub fn jet_pfix(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.fast_noun_space();
    let sam = slot(subject, 6, &space)?;
    let vex = slot(sam, 2, &space)?.in_space(&space).as_cell()?;
    let sab = slot(sam, 3, &space)?;

    let p_vex = vex.head().noun();
    let q_vex = vex.tail().noun();

    if unsafe { q_vex.raw_equals(&D(0)) } {
        return Ok(vex.cell().as_noun());
    }

    let uq_vex = q_vex.in_space(&space).as_cell()?.tail().as_cell()?;
    let quq_vex = uq_vex.tail().noun();

    let yit = slam_with_space(context, sab, quq_vex, &space)?
        .in_space(&space)
        .as_cell()?;

    let p_yit = yit.head().noun();
    let q_yit = yit.tail().noun();

    //  XX: Why don't we just return yit? When would p_vex ever be the later of the two?
    let arg = util::last(p_vex, p_yit, &space)?;
    Ok(T(&mut context.stack, &[arg, q_yit]))
}

pub fn jet_plug(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.fast_noun_space();
    let vex = slot(subject, 12, &space)?.in_space(&space).as_cell()?;
    let sab = slot(subject, 13, &space)?;
    let p_vex = vex.head().noun();
    let q_vex = vex.tail().noun();

    if unsafe { q_vex.raw_equals(&D(0)) } {
        Ok(vex.cell().as_noun())
    } else {
        let uq_vex = q_vex.in_space(&space).as_cell()?.tail().as_cell()?;
        let puq_vex = uq_vex.head().noun();
        let quq_vex = uq_vex.tail().noun();

        let yit = slam_with_space(context, sab, quq_vex, &space)?
            .in_space(&space)
            .as_cell()?;
        let p_yit = yit.head().noun();
        let q_yit = yit.tail().noun();

        let yur = util::last(p_vex, p_yit, &space)?;

        if unsafe { q_yit.raw_equals(&D(0)) } {
            Ok(T(&mut context.stack, &[yur, D(0)]))
        } else {
            let uq_yit = q_yit.in_space(&space).as_cell()?.tail().as_cell()?;
            let puq_yit = uq_yit.head().noun();
            let quq_yit = uq_yit.tail().noun();

            let inner = T(&mut context.stack, &[puq_vex, puq_yit]);
            Ok(T(&mut context.stack, &[yur, D(0), inner, quq_yit]))
        }
    }
}

pub fn jet_pose(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.fast_noun_space();
    let vex = slot(subject, 12, &space)?.in_space(&space).as_cell()?;
    let sab = slot(subject, 13, &space)?;

    let p_vex = vex.head().noun();
    let q_vex = vex.tail().noun();

    if unsafe { !q_vex.raw_equals(&D(0)) } {
        return Ok(vex.cell().as_noun());
    }

    let roq = kick(context, sab, D(2))?.in_space(&space).as_cell()?;
    let yur = util::last(p_vex, roq.head().noun(), &space)?;
    Ok(T(&mut context.stack, &[yur, roq.tail().noun()]))
}

pub fn jet_sfix(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.fast_noun_space();
    let sam = slot(subject, 6, &space)?;
    let vex = slot(sam, 2, &space)?.in_space(&space).as_cell()?;
    let sab = slot(sam, 3, &space)?;

    let p_vex = vex.head().noun();
    let q_vex = vex.tail().noun();

    if unsafe { q_vex.raw_equals(&D(0)) } {
        return Ok(vex.cell().as_noun());
    }

    let uq_vex = q_vex.in_space(&space).as_cell()?.tail().as_cell()?;
    let puq_vex = uq_vex.head().noun();
    let quq_vex = uq_vex.tail().noun();

    let yit = slam_with_space(context, sab, quq_vex, &space)?
        .in_space(&space)
        .as_cell()?;

    let p_yit = yit.head().noun();
    let q_yit = yit.tail().noun();
    let yur = util::last(p_vex, p_yit, &space)?;

    if unsafe { q_yit.raw_equals(&D(0)) } {
        Ok(T(&mut context.stack, &[yur, D(0)]))
    } else {
        let uq_yit = q_yit.in_space(&space).as_cell()?.tail().as_cell()?;
        let quq_yit = uq_yit.tail().noun();

        Ok(T(&mut context.stack, &[yur, D(0), puq_vex, quq_yit]))
    }
}

//
//  Rule Builders
//

pub fn jet_cold(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.fast_noun_space();
    let tub = slot(subject, 6, &space)?;
    let van = slot(subject, 7, &space)?;
    let cus = slot(van, 12, &space)?;
    let sef = slot(van, 13, &space)?;

    let vex = slam_with_space(context, sef, tub, &space)?
        .in_space(&space)
        .as_cell()?;
    let p_vex = vex.head().noun();
    let q_vex = vex.tail().noun();

    if unsafe { q_vex.raw_equals(&D(0)) } {
        Ok(vex.cell().as_noun())
    } else {
        let quq_vex = q_vex
            .in_space(&space)
            .as_cell()?
            .tail()
            .as_cell()?
            .tail()
            .noun();

        Ok(T(&mut context.stack, &[p_vex, D(0), cus, quq_vex]))
    }
}

pub fn jet_cook(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.fast_noun_space();
    let tub = slot(subject, 6, &space)?;
    let van = slot(subject, 7, &space)?;
    let poq = slot(van, 12, &space)?;
    let sef = slot(van, 13, &space)?;

    let vex = slam_with_space(context, sef, tub, &space)?
        .in_space(&space)
        .as_cell()?;
    let p_vex = vex.head().noun();
    let q_vex = vex.tail().noun();

    if unsafe { q_vex.raw_equals(&D(0)) } {
        Ok(vex.cell().as_noun())
    } else {
        let uq_vex = q_vex.in_space(&space).as_cell()?.tail().as_cell()?;
        let puq_vex = uq_vex.head().noun();
        let quq_vex = uq_vex.tail().noun();

        let wag = slam_with_space(context, poq, puq_vex, &space)?;
        Ok(T(&mut context.stack, &[p_vex, D(0), wag, quq_vex]))
    }
}

pub fn jet_easy(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.fast_noun_space();
    let tub = slot(subject, 6, &space)?;
    let van = slot(subject, 7, &space)?;
    let huf = slot(van, 6, &space)?;

    Ok(T(
        &mut context.stack,
        &[tub.in_space(&space).as_cell()?.head().noun(), D(0), huf, tub],
    ))
}

pub fn jet_here(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.fast_noun_space();
    let tub = slot(subject, 6, &space)?;
    let van = slot(subject, 7, &space)?;
    let hez = slot(van, 12, &space)?;
    let sef = slot(van, 13, &space)?;

    let p_tub = tub.in_space(&space).as_cell()?.head().noun();

    let vex = slam_with_space(context, sef, tub, &space)?
        .in_space(&space)
        .as_cell()?;
    let p_vex = vex.head().noun();
    let q_vex = vex.tail().noun();

    // XX fixes Vere's jet mismatch with Hoon 139.
    if unsafe { q_vex.raw_equals(&D(0)) } {
        return Ok(vex.cell().as_noun());
    }

    let uq_vex = q_vex.in_space(&space).as_cell()?.tail().as_cell()?;
    let puq_vex = uq_vex.head().noun();
    let quq_vex = uq_vex.tail().noun();
    let pquq_vex = quq_vex.in_space(&space).as_cell()?.head().noun();

    let inner_gud = T(&mut context.stack, &[p_tub, pquq_vex]);
    let gud = T(&mut context.stack, &[inner_gud, puq_vex]);
    let wag = slam_with_space(context, hez, gud, &space)?;

    Ok(T(&mut context.stack, &[p_vex, D(0), wag, quq_vex]))
}

pub fn jet_just(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.fast_noun_space();
    let tub = slot(subject, 6, &space)?;
    let van = slot(subject, 7, &space)?;
    let daf = slot(van, 6, &space)?;

    let tub_cell = tub.in_space(&space).as_cell()?;
    let p_tub = tub_cell.head().noun();
    let q_tub = tub_cell.tail().noun();

    if unsafe {
        q_tub.raw_equals(&D(0)) || !daf.raw_equals(&q_tub.in_space(&space).as_cell()?.head().noun())
    } {
        util::fail(context, p_tub)
    } else {
        util::next(context, tub, &space)
    }
}

pub fn jet_mask(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.fast_noun_space();
    let tub = slot(subject, 6, &space)?;
    let van = slot(subject, 7, &space)?;
    let mut bud = slot(van, 6, &space)?;

    let tub_cell = tub.in_space(&space).as_cell()?;
    let p_tub = tub_cell.head().noun();
    let q_tub = tub_cell.tail().noun();

    if unsafe { q_tub.raw_equals(&D(0)) } {
        return util::fail(context, p_tub);
    }

    let iq_tub = q_tub.in_space(&space).as_cell()?.head().noun();
    while unsafe { !bud.raw_equals(&D(0)) } {
        let cell = bud.in_space(&space).as_cell()?;
        if unsafe { cell.head().noun().raw_equals(&iq_tub) } {
            return util::next(context, tub, &space);
        }
        bud = cell.tail().noun();
    }
    util::fail(context, p_tub)
}

pub fn jet_shim(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.fast_noun_space();
    let tub = slot(subject, 6, &space)?.in_space(&space).as_cell()?;
    let van = slot(subject, 7, &space)?;
    let zep = slot(van, 6, &space)?.in_space(&space).as_cell()?;

    let p_tub = tub.head().noun();
    let q_tub = tub.tail().noun();

    if unsafe { q_tub.raw_equals(&D(0)) } {
        util::fail(context, p_tub)
    } else {
        let p_zep = zep.head().noun();
        let q_zep = zep.tail().noun();
        let iq_tub = q_tub.in_space(&space).as_cell()?.head().noun();

        if let (Some(p_zep_d), Some(q_zep_d), Some(iq_tub_d)) =
            (p_zep.direct(), q_zep.direct(), iq_tub.direct())
        {
            if (iq_tub_d.data() >= p_zep_d.data()) && (iq_tub_d.data() <= q_zep_d.data()) {
                util::next(context, tub.cell().as_noun(), &space)
            } else {
                util::fail(context, p_tub)
            }
        } else {
            Err(BAIL_FAIL)
        }
    }
}

pub fn jet_stag(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.fast_noun_space();
    let tub = slot(subject, 6, &space)?;
    let van = slot(subject, 7, &space)?;
    let gob = slot(van, 12, &space)?;
    let sef = slot(van, 13, &space)?;

    let vex = slam_with_space(context, sef, tub, &space)?
        .in_space(&space)
        .as_cell()?;
    let p_vex = vex.head().noun();
    let q_vex = vex.tail().noun();

    if unsafe { q_vex.raw_equals(&D(0)) } {
        Ok(vex.cell().as_noun())
    } else {
        let uq_vex = q_vex.in_space(&space).as_cell()?.tail().as_cell()?;
        let puq_vex = uq_vex.head().noun();
        let quq_vex = uq_vex.tail().noun();

        let wag = T(&mut context.stack, &[gob, puq_vex]);
        Ok(T(&mut context.stack, &[p_vex, D(0), wag, quq_vex]))
    }
}

pub fn jet_stew(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.fast_noun_space();
    let tub = slot(subject, 6, &space)?.in_space(&space).as_cell()?;
    let con = slot(subject, 7, &space)?;
    let mut hel = slot(con, 2, &space)?;

    let p_tub = tub.head().noun();
    let q_tub = tub.tail().noun();
    if unsafe { q_tub.raw_equals(&D(0)) } {
        return util::fail(context, p_tub);
    }

    let iq_tub = q_tub.in_space(&space).as_cell()?.head().as_atom()?.atom();
    if !iq_tub.is_direct() {
        // Character cannot be encoded using 8 bytes = computibilty error
        return Err(BAIL_FAIL);
    }

    loop {
        if unsafe { hel.raw_equals(&D(0)) } {
            return util::fail(context, p_tub);
        } else {
            let n_hel = slot(hel, 2, &space)?.in_space(&space).as_cell()?;
            let l_hel = slot(hel, 6, &space)?;
            let r_hel = slot(hel, 7, &space)?;
            let pn_hel = n_hel.head().noun();
            let qn_hel = n_hel.tail().noun();

            let bit = match pn_hel.as_either_atom_cell() {
                Left(atom) => match atom.as_either() {
                    Left(direct) => iq_tub.as_direct()?.data() == direct.data(),
                    Right(_) => {
                        // Character cannot be encoded using 8 bytes = computibilty error
                        return Err(BAIL_FAIL);
                    }
                },
                Right(cell) => {
                    let cell_handle = CellHandle::new(cell, &space);
                    let hpn_hel = cell_handle.head().as_atom()?.atom();
                    let tpn_hel = cell_handle.tail().as_atom()?.atom();

                    match (hpn_hel.as_either(), tpn_hel.as_either()) {
                        (Left(_), Left(_)) => {
                            gte_b(&mut context.stack, iq_tub, hpn_hel, &space)
                                && lte_b(&mut context.stack, iq_tub, tpn_hel, &space)
                        }
                        _ => {
                            // XX: Fixes jet mismatch in Vere
                            // Character cannot be encoded using 8 bytes = computibilty error
                            return Err(BAIL_FAIL);
                        }
                    }
                }
            };

            if bit {
                return slam(context, qn_hel, tub.cell().as_noun());
            } else {
                let wor = match pn_hel.as_either_atom_cell() {
                    Left(atom) => atom,
                    Right(cell) => CellHandle::new(cell, &space).head().as_atom()?.atom(),
                };

                if lth_b(&mut context.stack, iq_tub, wor, &space) {
                    hel = l_hel;
                } else {
                    hel = r_hel;
                }
            }
        }
    }
}

// +$  edge  [p=hair q=(unit [p=* q=nail])]
#[derive(Copy, Clone)]
struct StirPair {
    pub har: Noun, // p.edge
    pub res: Noun, // p.u.q.edge
}

pub fn jet_stir(context: &mut Context, subject: Noun) -> Result {
    let space = context.stack.fast_noun_space();
    unsafe {
        context.with_stack_frame(0, |context| {
            let mut tub = slot(subject, 6, &space)?;
            let van = slot(subject, 7, &space)?;
            let rud = slot(van, 12, &space)?;
            let raq = slot(van, 26, &space)?;
            let fel = slot(van, 27, &space)?;

            // initial accumulator (deconstructed)
            let mut p_wag: Noun;
            let mut puq_wag: Noun;
            let quq_wag: Noun;

            // push incremental, succesful [fel] parse results onto stack
            {
                let vex = slam_with_space(context, fel, tub, &space)?
                    .in_space(&space)
                    .as_cell()?;
                let mut p_vex = vex.head().noun();
                let mut q_vex = vex.tail().noun();
                while !q_vex.raw_equals(&D(0)) {
                    let puq_vex = slot(q_vex, 6, &space)?;
                    let quq_vex = slot(q_vex, 7, &space)?;

                    *(context.stack.push::<StirPair>()) = StirPair {
                        har: p_vex,
                        res: puq_vex,
                    };

                    tub = quq_vex;

                    let vex = slam_with_space(context, fel, tub, &space)?
                        .in_space(&space)
                        .as_cell()?;
                    p_vex = vex.head().noun();
                    q_vex = vex.tail().noun();
                }

                p_wag = p_vex;
                puq_wag = rud;
                quq_wag = tub;
            }

            // unwind the stack, folding parse results into [wag] by way of [raq]
            while !context.stack.stack_is_empty() {
                let par_u = *(context.stack.top::<StirPair>());
                p_wag = util::last(par_u.har, p_wag, &space)?;
                let sam = T(&mut context.stack, &[par_u.res, puq_wag]);
                puq_wag = slam_with_space(context, raq, sam, &space)?;
                context.stack.pop::<StirPair>();
            }

            let res = T(&mut context.stack, &[p_wag, D(0), puq_wag, quq_wag]);
            Ok(res)
        })
    }
}

pub mod util {
    use std::cmp::Ordering;

    use crate::interpreter::{inc, Context};
    use crate::jets::Result;
    use crate::noun::{Noun, NounSpace, D, T};

    pub fn last(zyc: Noun, naz: Noun, space: &NounSpace) -> Result {
        let zyl = zyc.in_space(space).as_cell()?;
        let nal = naz.in_space(space).as_cell()?;

        let p_zyc = zyl.head().noun().as_direct()?.data();
        let q_zyc = zyl.tail().noun().as_direct()?.data();
        let p_naz = nal.head().noun().as_direct()?.data();
        let q_naz = nal.tail().noun().as_direct()?.data();

        match p_zyc.cmp(&p_naz) {
            Ordering::Equal => {
                if q_zyc > q_naz {
                    Ok(zyc)
                } else {
                    Ok(naz)
                }
            }
            Ordering::Greater => Ok(zyc),
            Ordering::Less => Ok(naz),
        }
    }

    // Passing Noun and doing Cell check inside next is best to keep jet semantics in sync w/ Hoon.
    pub fn next(context: &mut Context, tub: Noun, space: &NounSpace) -> Result {
        let tub_cell = tub.in_space(space).as_cell()?;
        let p_tub = tub_cell.head().noun();
        let q_tub = tub_cell.tail().noun();

        if unsafe { q_tub.raw_equals(&D(0)) } {
            return fail(context, p_tub);
        }

        let q_tub_cell = q_tub.in_space(space).as_cell()?;
        let iq_tub = q_tub_cell.head().noun();
        let tq_tub = q_tub_cell.tail().noun();

        let zac = lust(context, iq_tub, p_tub, space)?;
        Ok(T(&mut context.stack, &[zac, D(0), iq_tub, zac, tq_tub]))
    }

    // Passing Noun and doing Cell check inside next is best to keep jet semantics in sync w/ Hoon.
    pub fn lust(context: &mut Context, weq: Noun, naz: Noun, space: &NounSpace) -> Result {
        let naz_cell = naz.in_space(space).as_cell()?;
        let p_naz = naz_cell.head().as_atom()?.atom();
        let q_naz = naz_cell.tail().as_atom()?.atom();

        if unsafe { weq.raw_equals(&D(10)) } {
            let arg = inc(&mut context.stack, p_naz).as_noun();
            Ok(T(&mut context.stack, &[arg, D(1)]))
        } else {
            let arg = inc(&mut context.stack, q_naz).as_noun();
            Ok(T(&mut context.stack, &[p_naz.as_noun(), arg]))
        }
    }

    pub fn fail(context: &mut Context, hair: Noun) -> Result {
        Ok(T(&mut context.stack, &[hair, D(0)]))
    }
}

#[cfg(test)]
mod tests {
    use ibig::ubig;

    use super::*;
    use crate::jets::util::test::*;
    use crate::noun::{D, T};
    use crate::serialization::cue;

    //  XX: need unit tests for:
    //      +last
    //      +bend
    //      +comp
    //      +glue
    //      +pfix
    //      +pose
    //      +sfix
    //      +here
    //      +just
    //      +mask
    //      +stag

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_easy() {
        let c = &mut init_context();

        // ((easy 'a') [[1 1] "abc"])
        //  [[1 1] "abc"]
        let sam_jam = A(&mut c.stack, &ubig!(3205468216717221061))
            .as_atom()
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
        let sam = cue(&mut c.stack, sam_jam);
        //  [p=[p=1 q=1] q=[~ [p='a' q=[p=[p=1 q=1] q="abc"]]]]
        let ans_jam = A(&mut c.stack, &ubig!(1720922644868600060465749189))
            .as_atom()
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
        let ans = cue(&mut c.stack, ans_jam).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        let ctx = T(&mut c.stack, &[D(0), D(97), D(0)]);
        assert_jet_door(
            c,
            jet_easy,
            sam.unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            }),
            ctx,
            ans,
        );

        // ((easy %foo) [[1 1] "abc"])
        //  [[1 1] "abc"]
        let sam_jam = A(&mut c.stack, &ubig!(3205468216717221061))
            .as_atom()
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
        let sam = cue(&mut c.stack, sam_jam).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        //  [p=[p=1 q=1] q=[~ [p=%foo q=[p=[p=1 q=1] q="abc"]]]]
        let ans_jam = A(&mut c.stack, &ubig!(3609036366588910247778413036281029))
            .as_atom()
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
        let ans = cue(&mut c.stack, ans_jam).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        let ctx = T(&mut c.stack, &[D(0), D(0x6f6f66), D(0)]);
        assert_jet_door(c, jet_easy, sam, ctx, ans);
    }
}
