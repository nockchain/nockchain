use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_EXIT, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::noun::{Atom, IndirectAtom, Noun, D, T};
use nockvm_macros::tas;
use tracing::debug;

use crate::form::belt::*;
use crate::form::felt::*;
use crate::form::gen_trace::{build_tree_data, TreeData};
use crate::form::handle::{finalize_mary, new_handle_mut_mary};
use crate::form::mary::*;
use crate::form::structs::HoonList;
use crate::jets::table_utils::*;

pub fn compute_v2_mega_extend_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let sam = slot(subject, 6)?;
    let table_mary = slot(sam, 2)?;
    let all_chals = slot(sam, 6)?;
    let _fock_ret = slot(sam, 7)?;

    let chals: MegaExtChals = init_mega_ext_chals(all_chals)?;
    let z2: Felt = fmul_(&chals.z, &chals.z);
    let z3: Felt = fmul_(&z2, &chals.z);
    let z_inv: Felt = finv_(&chals.z);

    let table_noun = slot(table_mary, 3)?;
    let Ok(table) = MarySlice::try_from(table_noun) else {
        debug!("cannot convert mary arg to mary");
        return Err(BAIL_FAIL);
    };

    let (res, mut res_mary): (IndirectAtom, MarySliceMut) = new_handle_mut_mary(
        &mut context.stack, NUM_MEGA_EXT_COLS as usize, table.len as usize,
    );

    let mut state: StateData = StateData::new();

    let first_row = get_row(&table, 0);
    let top_subject = Ion {
        size: grab_pelt(first_row, S_SIZE_IDX),
        leaf: grab_pelt(first_row, S_LEAF_IDX),
        dyck: grab_pelt(first_row, S_DYCK_IDX),
    };
    let top_formula = Ion {
        size: grab_pelt(first_row, F_SIZE_IDX),
        leaf: grab_pelt(first_row, F_LEAF_IDX),
        dyck: grab_pelt(first_row, F_DYCK_IDX),
    };
    let top_product = Ion {
        size: grab_pelt(first_row, E_SIZE_IDX),
        leaf: grab_pelt(first_row, E_LEAF_IDX),
        dyck: grab_pelt(first_row, E_DYCK_IDX),
    };

    state.ln = chals.z;
    state.opc = chals.z;
    state.stack_kv = fmul_(
        &chals.z,
        &compress_noun(&top_subject, &top_formula, &top_product, &chals),
    );

    let mut row_count = 0;
    for i in 0..table.len {
        row_count = i;
        let row: &[u64] = get_row(&table, i);
        if grab_belt(row, PAD_IDX) == 1 {
            break;
        }

        let f = Ion {
            size: grab_pelt(row, F_SIZE_IDX),
            leaf: grab_pelt(row, F_LEAF_IDX),
            dyck: grab_pelt(row, F_DYCK_IDX),
        };
        let f_h = Ion {
            size: grab_pelt(row, F_H_SIZE_IDX),
            leaf: grab_pelt(row, F_H_LEAF_IDX),
            dyck: grab_pelt(row, F_H_DYCK_IDX),
        };
        let f_t = Ion {
            size: grab_pelt(row, F_T_SIZE_IDX),
            leaf: grab_pelt(row, F_T_LEAF_IDX),
            dyck: grab_pelt(row, F_T_DYCK_IDX),
        };

        let (new_opc, new_decoder, new_op0_mset) = match get_opcode(row)? {
            0 => {
                let s = Ion {
                    size: grab_pelt(row, S_SIZE_IDX),
                    leaf: grab_pelt(row, S_LEAF_IDX),
                    dyck: grab_pelt(row, S_DYCK_IDX),
                };
                let e = Ion {
                    size: grab_pelt(row, E_SIZE_IDX),
                    leaf: grab_pelt(row, E_LEAF_IDX),
                    dyck: grab_pelt(row, E_DYCK_IDX),
                };
                let axis: Belt = f_t.leaf[0];
                let new_decoder: Felt = update_decoder(&chals, &state.decode_mset, &f, &f_h, &f_t);
                let new_op0_mset: Felt = if axis.0 == 1 {
                    // for axis=1 don't use memory table
                    state.op0_mset
                } else {
                    update_mset(&chals, &state.op0_mset, &s, &f_t, &e)
                };
                (state.opc, new_decoder, new_op0_mset)
            }
            1 => {
                let new_decoder = update_decoder(&chals, &state.decode_mset, &f, &f_h, &f_t);
                (state.opc, new_decoder, state.op0_mset)
            }
            2 => {
                let f_th = Ion {
                    size: grab_pelt(row, F_TH_SIZE_IDX),
                    leaf: grab_pelt(row, F_TH_LEAF_IDX),
                    dyck: grab_pelt(row, F_TH_DYCK_IDX),
                };
                let f_tt = Ion {
                    size: grab_pelt(row, F_TT_SIZE_IDX),
                    leaf: grab_pelt(row, F_TT_LEAF_IDX),
                    dyck: grab_pelt(row, F_TT_DYCK_IDX),
                };
                let new_opc = fmul_(&state.opc, &z3);
                let decode1 = update_decoder(&chals, &state.decode_mset, &f, &f_h, &f_t);
                let new_decoder = update_decoder(&chals, &decode1, &f_t, &f_th, &f_tt);
                (new_opc, new_decoder, state.op0_mset)
            }
            3 => {
                let new_opc = fmul_(&state.opc, &chals.z);
                let new_decoder = update_decoder(&chals, &state.decode_mset, &f, &f_h, &f_t);
                (new_opc, new_decoder, state.op0_mset)
            }
            4 => {
                let new_opc = fmul_(&state.opc, &chals.z);
                let new_decoder = update_decoder(&chals, &state.decode_mset, &f, &f_h, &f_t);
                (new_opc, new_decoder, state.op0_mset)
            }
            5 => {
                let f_th = Ion {
                    size: grab_pelt(row, F_TH_SIZE_IDX),
                    leaf: grab_pelt(row, F_TH_LEAF_IDX),
                    dyck: grab_pelt(row, F_TH_DYCK_IDX),
                };
                let f_tt = Ion {
                    size: grab_pelt(row, F_TT_SIZE_IDX),
                    leaf: grab_pelt(row, F_TT_LEAF_IDX),
                    dyck: grab_pelt(row, F_TT_DYCK_IDX),
                };
                let new_opc = fmul_(&state.opc, &z2);
                let decode1 = update_decoder(&chals, &state.decode_mset, &f, &f_h, &f_t);
                let new_decoder = update_decoder(&chals, &decode1, &f_t, &f_th, &f_tt);
                (new_opc, new_decoder, state.op0_mset)
            }
            6 => {
                let f_th = Ion {
                    size: grab_pelt(row, F_TH_SIZE_IDX),
                    leaf: grab_pelt(row, F_TH_LEAF_IDX),
                    dyck: grab_pelt(row, F_TH_DYCK_IDX),
                };
                let f_tt = Ion {
                    size: grab_pelt(row, F_TT_SIZE_IDX),
                    leaf: grab_pelt(row, F_TT_LEAF_IDX),
                    dyck: grab_pelt(row, F_TT_DYCK_IDX),
                };
                let f_tth = Ion {
                    size: grab_pelt(row, F_TTH_SIZE_IDX),
                    leaf: grab_pelt(row, F_TTH_LEAF_IDX),
                    dyck: grab_pelt(row, F_TTH_DYCK_IDX),
                };
                let f_ttt = Ion {
                    size: grab_pelt(row, F_TTT_SIZE_IDX),
                    leaf: grab_pelt(row, F_TTT_LEAF_IDX),
                    dyck: grab_pelt(row, F_TTT_DYCK_IDX),
                };
                let new_opc = fmul_(&state.opc, &z2);
                let decode1 = update_decoder(&chals, &state.decode_mset, &f, &f_h, &f_t);
                let decode2 = update_decoder(&chals, &decode1, &f_t, &f_th, &f_tt);
                let new_decoder = update_decoder(&chals, &decode2, &f_tt, &f_tth, &f_ttt);
                (new_opc, new_decoder, state.op0_mset)
            }
            7 => {
                let f_th = Ion {
                    size: grab_pelt(row, F_TH_SIZE_IDX),
                    leaf: grab_pelt(row, F_TH_LEAF_IDX),
                    dyck: grab_pelt(row, F_TH_DYCK_IDX),
                };
                let f_tt = Ion {
                    size: grab_pelt(row, F_TT_SIZE_IDX),
                    leaf: grab_pelt(row, F_TT_LEAF_IDX),
                    dyck: grab_pelt(row, F_TT_DYCK_IDX),
                };
                let new_opc = fmul_(&state.opc, &z2);
                let decode1 = update_decoder(&chals, &state.decode_mset, &f, &f_h, &f_t);
                let new_decoder = update_decoder(&chals, &decode1, &f_t, &f_th, &f_tt);
                (new_opc, new_decoder, state.op0_mset)
            }
            8 => {
                let f_th = Ion {
                    size: grab_pelt(row, F_TH_SIZE_IDX),
                    leaf: grab_pelt(row, F_TH_LEAF_IDX),
                    dyck: grab_pelt(row, F_TH_DYCK_IDX),
                };
                let f_tt = Ion {
                    size: grab_pelt(row, F_TT_SIZE_IDX),
                    leaf: grab_pelt(row, F_TT_LEAF_IDX),
                    dyck: grab_pelt(row, F_TT_DYCK_IDX),
                };
                let new_opc = fmul_(&state.opc, &z2);
                let decode1 = update_decoder(&chals, &state.decode_mset, &f, &f_h, &f_t);
                let new_decoder = update_decoder(&chals, &decode1, &f_t, &f_th, &f_tt);
                (new_opc, new_decoder, state.op0_mset)
            }
            9 => {
                let new_opc = fmul_(&state.opc, &z2);
                let new_decoder = update_decoder(&chals, &state.decode_mset, &f, &f_h, &f_t);
                (new_opc, new_decoder, state.op0_mset)
            }
            _ => {
                debug!("invalid opcode");
                return Err(BAIL_EXIT);
            }
        };
        state.sfcons_inv = compute_sfcons_inv(&state, row, &chals)?;
        let new_stack_kv: Felt = update_stack(&state, row, &chals, z2, z3)?;

        write_mega_ext_row_data(&mut res_mary, &Row(i as usize), &state);

        state.stack_kv = new_stack_kv;
        state.opc = new_opc;
        state.decode_mset = new_decoder;
        state.op0_mset = new_op0_mset;
        state.ln = fmul_(&chals.z, &state.ln);
    }

    // write a final pad row with the final line number
    if row_count < table.len {
        let last_row = get_row(&table, row_count);
        state.sfcons_inv = compute_sfcons_inv(&state, last_row, &chals)?;
        write_mega_ext_row_data(&mut res_mary, &Row(row_count as usize), &state);
    }

    // decrement the line number during padding but keep the kvs and msets the same
    for i in (row_count + 1)..table.len {
        let row: &[u64] = get_row(&table, i);

        state.ln = fmul_(&state.ln, &z_inv);
        state.sfcons_inv = compute_sfcons_inv(&state, row, &chals)?;
        write_mega_ext_row_data(&mut res_mary, &Row(i as usize), &state);
    }

    let res_cell = finalize_mary(
        &mut context.stack, NUM_MEGA_EXT_COLS as usize, table.len as usize, res,
    );
    let header = header(context);
    Ok(T(&mut context.stack, &[header, res_cell]))
}

fn compute_sfcons_inv(
    state: &StateData,
    row: &[u64],
    chals: &MegaExtChals,
) -> Result<Felt, JetErr> {
    if grab_belt(row, PAD_IDX).0 == 1 {
        Ok(finv_(&fsub_(&chals.z, &state.ln)))
    } else {
        match get_opcode(row)? {
            0 => Ok(make_invs(&fsub_(
                &grab_pelt(row, F_T_LEAF_IDX),
                &Felt::one(),
            ))),
            3 => Ok(make_invs(&fsub_(
                &chals.alf,
                &grab_pelt(row, SF1_E_SIZE_IDX),
            ))),
            5 => Ok(finv_(&fadd_all(vec![
                fmul_(
                    &chals.a,
                    &fsub_(
                        &grab_pelt(row, SF1_E_SIZE_IDX),
                        &grab_pelt(row, SF2_E_SIZE_IDX),
                    ),
                ),
                fmul_(
                    &chals.b,
                    &fsub_(
                        &grab_pelt(row, SF1_E_DYCK_IDX),
                        &grab_pelt(row, SF2_E_DYCK_IDX),
                    ),
                ),
                fmul_(
                    &chals.c,
                    &fsub_(
                        &grab_pelt(row, SF1_E_LEAF_IDX),
                        &grab_pelt(row, SF2_E_LEAF_IDX),
                    ),
                ),
                fsub_(&Felt::one(), &grab_pelt(row, E_LEAF_IDX)),
            ]))),
            8 => Ok(finv_(&fmul_(
                &grab_pelt(row, S_SIZE_IDX),
                &grab_pelt(row, SF2_E_SIZE_IDX),
            ))),
            9 => Ok(finv_(&fmul_(
                &grab_pelt(row, SF1_E_SIZE_IDX),
                &grab_pelt(row, SF2_E_SIZE_IDX),
            ))),
            _ => Ok(Felt::one()),
        }
    }
}

fn make_invs(f: &Felt) -> Felt {
    if f.is_zero() {
        Felt::zero()
    } else {
        finv_(f)
    }
}

fn update_stack(
    state: &StateData,
    row: &[u64],
    chals: &MegaExtChals,
    z2: Felt,
    z3: Felt,
) -> Result<Felt, JetErr> {
    let s = Ion {
        size: grab_pelt(row, S_SIZE_IDX),
        leaf: grab_pelt(row, S_LEAF_IDX),
        dyck: grab_pelt(row, S_DYCK_IDX),
    };
    let f = Ion {
        size: grab_pelt(row, F_SIZE_IDX),
        leaf: grab_pelt(row, F_LEAF_IDX),
        dyck: grab_pelt(row, F_DYCK_IDX),
    };
    let e = Ion {
        size: grab_pelt(row, E_SIZE_IDX),
        leaf: grab_pelt(row, E_LEAF_IDX),
        dyck: grab_pelt(row, E_DYCK_IDX),
    };

    let sf1_s = Ion {
        size: grab_pelt(row, SF1_S_SIZE_IDX),
        leaf: grab_pelt(row, SF1_S_LEAF_IDX),
        dyck: grab_pelt(row, SF1_S_DYCK_IDX),
    };
    let sf1_f = Ion {
        size: grab_pelt(row, SF1_F_SIZE_IDX),
        leaf: grab_pelt(row, SF1_F_LEAF_IDX),
        dyck: grab_pelt(row, SF1_F_DYCK_IDX),
    };
    let sf1_e = Ion {
        size: grab_pelt(row, SF1_E_SIZE_IDX),
        leaf: grab_pelt(row, SF1_E_LEAF_IDX),
        dyck: grab_pelt(row, SF1_E_DYCK_IDX),
    };

    let sf2_s = Ion {
        size: grab_pelt(row, SF2_S_SIZE_IDX),
        leaf: grab_pelt(row, SF2_S_LEAF_IDX),
        dyck: grab_pelt(row, SF2_S_DYCK_IDX),
    };
    let sf2_f = Ion {
        size: grab_pelt(row, SF2_F_SIZE_IDX),
        leaf: grab_pelt(row, SF2_F_LEAF_IDX),
        dyck: grab_pelt(row, SF2_F_DYCK_IDX),
    };
    let sf2_e = Ion {
        size: grab_pelt(row, SF2_E_SIZE_IDX),
        leaf: grab_pelt(row, SF2_E_LEAF_IDX),
        dyck: grab_pelt(row, SF2_E_DYCK_IDX),
    };

    let sf3_s = Ion {
        size: grab_pelt(row, SF3_S_SIZE_IDX),
        leaf: grab_pelt(row, SF3_S_LEAF_IDX),
        dyck: grab_pelt(row, SF3_S_DYCK_IDX),
    };
    let sf3_f = Ion {
        size: grab_pelt(row, SF3_F_SIZE_IDX),
        leaf: grab_pelt(row, SF3_F_LEAF_IDX),
        dyck: grab_pelt(row, SF3_F_DYCK_IDX),
    };
    let sf3_e = Ion {
        size: grab_pelt(row, SF3_E_SIZE_IDX),
        leaf: grab_pelt(row, SF3_E_LEAF_IDX),
        dyck: grab_pelt(row, SF3_E_DYCK_IDX),
    };

    let program: Felt = compress_noun(&s, &f, &e, chals);
    let sp1: Felt = compress_noun(&sf1_s, &sf1_f, &sf1_e, chals);
    let sp2: Felt = compress_noun(&sf2_s, &sf2_f, &sf2_e, chals);
    let sp3: Felt = compress_noun(&sf3_s, &sf3_f, &sf3_e, chals);

    let op: u64 = get_opcode(row)?;

    let term1: Felt = if matches!(op, 0 | 1) {
        Felt::zero()
    } else {
        fmul_(&sp1, &fmul_(&state.opc, &chals.z))
    };

    let term2: Felt = if matches!(op, 0 | 1 | 3 | 4) {
        Felt::zero()
    } else {
        fmul_(&sp2, &fmul_(&state.opc, &z2))
    };

    let term3: Felt = if op != 2 {
        Felt::zero()
    } else {
        fmul_(&sp3, &fmul_(&state.opc, &z3))
    };

    Ok(fadd_all(vec![
        state.stack_kv,
        term1,
        term2,
        term3,
        fneg_(&fmul_(&program, &state.ln)),
    ]))
}

fn get_opcode(row: &[u64]) -> Result<u64, JetErr> {
    if grab_belt(row, OP0_IDX).0 == 1 {
        Ok(0)
    } else if grab_belt(row, OP1_IDX).0 == 1 {
        Ok(1)
    } else if grab_belt(row, OP2_IDX).0 == 1 {
        Ok(2)
    } else if grab_belt(row, OP3_IDX).0 == 1 {
        Ok(3)
    } else if grab_belt(row, OP4_IDX).0 == 1 {
        Ok(4)
    } else if grab_belt(row, OP5_IDX).0 == 1 {
        Ok(5)
    } else if grab_belt(row, OP6_IDX).0 == 1 {
        Ok(6)
    } else if grab_belt(row, OP7_IDX).0 == 1 {
        Ok(7)
    } else if grab_belt(row, OP8_IDX).0 == 1 {
        Ok(8)
    } else if grab_belt(row, OP9_IDX).0 == 1 {
        Ok(9)
    } else {
        return Err(BAIL_EXIT);
    }
}

fn update_decoder(chals: &MegaExtChals, mset: &Felt, s: &Ion, h: &Ion, t: &Ion) -> Felt {
    let trip: Felt = fadd_all(vec![
        fmul_(&chals.j, &s.size),
        fmul_(&chals.k, &s.dyck),
        fmul_(&chals.l, &s.leaf),
        fmul_(&chals.m, &h.size),
        fmul_(&chals.n, &h.dyck),
        fmul_(&chals.o, &h.leaf),
        fmul_(&chals.w, &t.size),
        fmul_(&chals.x, &t.dyck),
        fmul_(&chals.y, &t.leaf),
    ]);
    fadd_(mset, &finv_(&fsub_(&chals.gam, &trip)))
}

fn update_mset(chals: &MegaExtChals, mset: &Felt, s: &Ion, axis: &Ion, e: &Ion) -> Felt {
    let mroot: Felt = fadd_(
        &fmul_(&chals.a, &s.size),
        &fadd_(&fmul_(&chals.b, &s.dyck), &fmul_(&chals.c, &s.leaf)),
    );

    let maxis: Felt = fmul_(&chals.m, &axis.leaf);
    let mval: Felt = fadd_(
        &fmul_(&chals.j, &e.size),
        &fadd_(&fmul_(&chals.k, &e.dyck), &fmul_(&chals.l, &e.leaf)),
    );
    let mvar: Felt = fadd_(&mroot, &fadd_(&maxis, &mval));
    fadd_(mset, &finv_(&fsub_(&chals.bet, &mvar)))
}

fn compress_noun(s: &Ion, f: &Ion, e: &Ion, chals: &MegaExtChals) -> Felt {
    fadd_(
        &fadd_(
            &fmul_(&chals.m, &compress_ion(s, &chals.j, &chals.k, &chals.l)),
            &fmul_(&chals.n, &compress_ion(f, &chals.j, &chals.k, &chals.l)),
        ),
        &fmul_(&chals.o, &compress_ion(e, &chals.j, &chals.k, &chals.l)),
    )
}

fn compress_ion(ion: &Ion, a: &Felt, b: &Felt, c: &Felt) -> Felt {
    fadd_(
        &fadd_(&fmul_(a, &ion.size), &fmul_(b, &ion.dyck)),
        &fmul_(c, &ion.leaf),
    )
}

pub fn compute_v2_extend_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let sam = slot(subject, 6)?;
    let table_mary = slot(sam, 2)?;
    let chals_rd1 = slot(sam, 6)?;
    let fock_ret = slot(sam, 7)?;
    let queue = slot(fock_ret, 2)?;

    let chals: ExtChals = init_ext_chals(chals_rd1)?;

    let table_noun = slot(table_mary, 3)?;
    let Ok(table) = MarySlice::try_from(table_noun) else {
        debug!("cannot convert mary arg to mary");
        return Err(BAIL_FAIL);
    };

    let (res, mut res_mary): (IndirectAtom, MarySliceMut) = new_handle_mut_mary(
        &mut context.stack, NUM_EXT_COLS as usize, table.len as usize,
    );

    let stack: Vec<TreeData> = build_compute_queue(queue, &chals.alf)?;
    let mut stack_idx: usize = 0;
    let mut row_idx: usize = 0;

    while stack_idx < stack.len() {
        let mut row = ExtRowData::new();

        row.s = stack[stack_idx];
        row.f = stack[stack_idx + 1];
        row.e = stack[stack_idx + 2];
        stack_idx += 3;

        row.f_h = build_tree_data(row.f.n.as_cell()?.head(), &chals.alf)?;
        row.f_t = build_tree_data(row.f.n.as_cell()?.tail(), &chals.alf)?;

        let op: u64 = if row.f.n.as_cell()?.head().is_atom() {
            row.f.n.as_cell()?.head().as_atom()?.as_u64()?
        } else {
            9
        };

        if matches!(op, 2 | 5 | 6 | 7 | 8) {
            row.f_th = build_tree_data(row.f_t.n.as_cell()?.head(), &chals.alf)?;
        }

        if matches!(op, 2 | 5 | 6 | 7 | 8) {
            row.f_tt = build_tree_data(row.f_t.n.as_cell()?.tail(), &chals.alf)?;
        }

        if op == 6 {
            row.f_tth = build_tree_data(row.f_tt.n.as_cell()?.head(), &chals.alf)?;
            row.f_ttt = build_tree_data(row.f_tt.n.as_cell()?.tail(), &chals.alf)?;
        }

        match op {
            0 => {}
            1 => {}
            2 => {
                let sf1_e = stack[stack_idx];
                let sf2_e = stack[stack_idx + 1];
                stack_idx += 2;

                row.sf1_s = row.s;
                row.sf1_f = row.f_th;
                row.sf1_e = sf1_e;
                row.sf2_s = row.s;
                row.sf2_f = row.f_tt;
                row.sf2_e = sf2_e;
                row.sf3_s = sf1_e;
                row.sf3_f = sf2_e;
                row.sf3_e = row.e;
            }
            3 => {
                let sf1_e = stack[stack_idx];
                stack_idx += 1;

                row.sf1_s = row.s;
                row.sf1_f = row.f_t;
                row.sf1_e = sf1_e;
            }
            4 => {
                let sf1_e = stack[stack_idx];
                stack_idx += 1;

                row.sf1_s = row.s;
                row.sf1_f = row.f_t;
                row.sf1_e = sf1_e;
            }
            5 => {
                let sf1_e = stack[stack_idx];
                let sf2_e = stack[stack_idx + 1];
                stack_idx += 2;

                row.sf1_s = row.s;
                row.sf1_f = row.f_th;
                row.sf1_e = sf1_e;
                row.sf2_s = row.s;
                row.sf2_f = row.f_tt;
                row.sf2_e = sf2_e;
            }
            6 => {
                let sf1_f = stack[stack_idx];
                let sf1_e = stack[stack_idx + 1];
                let sf2_e = stack[stack_idx + 2];
                stack_idx += 3;

                row.sf1_s = row.s;
                row.sf1_f = sf1_f;
                row.sf1_e = sf1_e;
                row.sf2_s = row.s;
                row.sf2_f = row.f_th;
                row.sf2_e = sf2_e;
            }
            7 => {
                let sf2_e = stack[stack_idx];
                stack_idx += 1;

                row.sf1_s = sf2_e;
                row.sf1_f = row.f_tt;
                row.sf1_e = row.e;
                row.sf2_s = row.s;
                row.sf2_f = row.f_th;
                row.sf2_e = sf2_e;
            }
            8 => {
                let sf1_s = stack[stack_idx];
                let sf2_e = stack[stack_idx + 1];
                stack_idx += 2;

                row.sf1_s = sf1_s;
                row.sf1_f = row.f_tt;
                row.sf1_e = row.e;
                row.sf2_s = row.s;
                row.sf2_f = row.f_th;
                row.sf2_e = sf2_e;
            }
            9 => {
                // cons is %9
                let left_e = stack[stack_idx];
                let right_e = stack[stack_idx + 1];
                stack_idx += 2;

                row.sf1_s = row.s;
                row.sf1_f = row.f_h;
                row.sf1_e = left_e;
                row.sf2_s = row.s;
                row.sf2_f = row.f_t;
                row.sf2_e = right_e;
            }
            _ => {
                debug!("invalid opcode");
                return Err(BAIL_EXIT);
            }
        }

        row.fcons_inv = compute_fcons_inv(&row);

        write_ext_row_data(&mut res_mary, &Row(row_idx), &row);
        row_idx += 1;
    }

    let res_cell = finalize_mary(
        &mut context.stack, NUM_EXT_COLS as usize, table.len as usize, res,
    );
    let header = header(context);
    Ok(T(&mut context.stack, &[header, res_cell]))
}

fn compute_fcons_inv(row: &ExtRowData) -> Felt {
    finv_(&fmul_(
        &row.f_h.size,
        &fmul_(&row.f_th.size, &row.f_tt.size),
    ))
}

struct StateData {
    ln: Felt,
    sfcons_inv: Felt,
    opc: Felt,
    stack_kv: Felt,
    decode_mset: Felt,
    op0_mset: Felt,
}

impl StateData {
    fn new() -> Self {
        StateData {
            ln: Felt::zero(),
            sfcons_inv: Felt::zero(),
            opc: Felt::zero(),
            stack_kv: Felt::zero(),
            decode_mset: Felt::zero(),
            op0_mset: Felt::zero(),
        }
    }
}

fn write_mega_ext_row_data(table: &mut MarySliceMut, row: &Row, data: &StateData) {
    write_pelt(table, &data.ln, row, &Col(mega_idx(LN_IDX)));
    write_pelt(table, &data.sfcons_inv, row, &Col(mega_idx(SFCONS_INV_IDX)));
    write_pelt(table, &data.opc, row, &Col(mega_idx(OPC_IDX)));
    write_pelt(table, &data.stack_kv, row, &Col(mega_idx(STACK_KV_IDX)));
    write_pelt(
        table,
        &data.decode_mset,
        row,
        &Col(mega_idx(DECODE_MSET_IDX)),
    );
    write_pelt(table, &data.op0_mset, row, &Col(mega_idx(OP0_MSET_IDX)));
}

#[derive(Copy, Clone)]
struct ExtRowData {
    s: TreeData,
    f: TreeData,
    e: TreeData,
    //
    sf1_s: TreeData,
    sf1_f: TreeData,
    sf1_e: TreeData,
    //
    sf2_s: TreeData,
    sf2_f: TreeData,
    sf2_e: TreeData,
    //
    sf3_s: TreeData,
    sf3_f: TreeData,
    sf3_e: TreeData,
    //
    f_h: TreeData,
    f_t: TreeData,
    f_th: TreeData,
    f_tt: TreeData,
    f_tth: TreeData,
    f_ttt: TreeData,
    //
    fcons_inv: Felt,
}

impl ExtRowData {
    fn new() -> Self {
        ExtRowData {
            s: TreeData::new(),
            f: TreeData::new(),
            e: TreeData::new(),

            sf1_s: TreeData::new(),
            sf1_f: TreeData::new(),
            sf1_e: TreeData::new(),

            sf2_s: TreeData::new(),
            sf2_f: TreeData::new(),
            sf2_e: TreeData::new(),

            sf3_s: TreeData::new(),
            sf3_f: TreeData::new(),
            sf3_e: TreeData::new(),

            f_h: TreeData::new(),
            f_t: TreeData::new(),
            f_th: TreeData::new(),
            f_tt: TreeData::new(),
            f_tth: TreeData::new(),
            f_ttt: TreeData::new(),

            fcons_inv: Felt::zero(),
        }
    }
}

fn write_ext_row_data(table: &mut MarySliceMut, row: &Row, data: &ExtRowData) {
    write_pelt(table, &data.s.size, row, &Col(ext_idx(S_SIZE_IDX)));
    write_pelt(table, &data.s.leaf, row, &Col(ext_idx(S_LEAF_IDX)));
    write_pelt(table, &data.s.dyck, row, &Col(ext_idx(S_DYCK_IDX)));

    write_pelt(table, &data.f.size, row, &Col(ext_idx(F_SIZE_IDX)));
    write_pelt(table, &data.f.leaf, row, &Col(ext_idx(F_LEAF_IDX)));
    write_pelt(table, &data.f.dyck, row, &Col(ext_idx(F_DYCK_IDX)));

    write_pelt(table, &data.e.size, row, &Col(ext_idx(E_SIZE_IDX)));
    write_pelt(table, &data.e.leaf, row, &Col(ext_idx(E_LEAF_IDX)));
    write_pelt(table, &data.e.dyck, row, &Col(ext_idx(E_DYCK_IDX)));

    write_pelt(table, &data.sf1_s.size, row, &Col(ext_idx(SF1_S_SIZE_IDX)));
    write_pelt(table, &data.sf1_s.leaf, row, &Col(ext_idx(SF1_S_LEAF_IDX)));
    write_pelt(table, &data.sf1_s.dyck, row, &Col(ext_idx(SF1_S_DYCK_IDX)));

    write_pelt(table, &data.sf1_f.size, row, &Col(ext_idx(SF1_F_SIZE_IDX)));
    write_pelt(table, &data.sf1_f.leaf, row, &Col(ext_idx(SF1_F_LEAF_IDX)));
    write_pelt(table, &data.sf1_f.dyck, row, &Col(ext_idx(SF1_F_DYCK_IDX)));

    write_pelt(table, &data.sf1_e.size, row, &Col(ext_idx(SF1_E_SIZE_IDX)));
    write_pelt(table, &data.sf1_e.leaf, row, &Col(ext_idx(SF1_E_LEAF_IDX)));
    write_pelt(table, &data.sf1_e.dyck, row, &Col(ext_idx(SF1_E_DYCK_IDX)));

    write_pelt(table, &data.sf2_s.size, row, &Col(ext_idx(SF2_S_SIZE_IDX)));
    write_pelt(table, &data.sf2_s.leaf, row, &Col(ext_idx(SF2_S_LEAF_IDX)));
    write_pelt(table, &data.sf2_s.dyck, row, &Col(ext_idx(SF2_S_DYCK_IDX)));

    write_pelt(table, &data.sf2_f.size, row, &Col(ext_idx(SF2_F_SIZE_IDX)));
    write_pelt(table, &data.sf2_f.leaf, row, &Col(ext_idx(SF2_F_LEAF_IDX)));
    write_pelt(table, &data.sf2_f.dyck, row, &Col(ext_idx(SF2_F_DYCK_IDX)));

    write_pelt(table, &data.sf2_e.size, row, &Col(ext_idx(SF2_E_SIZE_IDX)));
    write_pelt(table, &data.sf2_e.leaf, row, &Col(ext_idx(SF2_E_LEAF_IDX)));
    write_pelt(table, &data.sf2_e.dyck, row, &Col(ext_idx(SF2_E_DYCK_IDX)));

    write_pelt(table, &data.sf3_s.size, row, &Col(ext_idx(SF3_S_SIZE_IDX)));
    write_pelt(table, &data.sf3_s.leaf, row, &Col(ext_idx(SF3_S_LEAF_IDX)));
    write_pelt(table, &data.sf3_s.dyck, row, &Col(ext_idx(SF3_S_DYCK_IDX)));

    write_pelt(table, &data.sf3_f.size, row, &Col(ext_idx(SF3_F_SIZE_IDX)));
    write_pelt(table, &data.sf3_f.leaf, row, &Col(ext_idx(SF3_F_LEAF_IDX)));
    write_pelt(table, &data.sf3_f.dyck, row, &Col(ext_idx(SF3_F_DYCK_IDX)));

    write_pelt(table, &data.sf3_e.size, row, &Col(ext_idx(SF3_E_SIZE_IDX)));
    write_pelt(table, &data.sf3_e.leaf, row, &Col(ext_idx(SF3_E_LEAF_IDX)));
    write_pelt(table, &data.sf3_e.dyck, row, &Col(ext_idx(SF3_E_DYCK_IDX)));

    write_pelt(table, &data.f_h.size, row, &Col(ext_idx(F_H_SIZE_IDX)));
    write_pelt(table, &data.f_h.leaf, row, &Col(ext_idx(F_H_LEAF_IDX)));
    write_pelt(table, &data.f_h.dyck, row, &Col(ext_idx(F_H_DYCK_IDX)));

    write_pelt(table, &data.f_t.size, row, &Col(ext_idx(F_T_SIZE_IDX)));
    write_pelt(table, &data.f_t.leaf, row, &Col(ext_idx(F_T_LEAF_IDX)));
    write_pelt(table, &data.f_t.dyck, row, &Col(ext_idx(F_T_DYCK_IDX)));

    write_pelt(table, &data.f_th.size, row, &Col(ext_idx(F_TH_SIZE_IDX)));
    write_pelt(table, &data.f_th.leaf, row, &Col(ext_idx(F_TH_LEAF_IDX)));
    write_pelt(table, &data.f_th.dyck, row, &Col(ext_idx(F_TH_DYCK_IDX)));

    write_pelt(table, &data.f_tt.size, row, &Col(ext_idx(F_TT_SIZE_IDX)));
    write_pelt(table, &data.f_tt.leaf, row, &Col(ext_idx(F_TT_LEAF_IDX)));
    write_pelt(table, &data.f_tt.dyck, row, &Col(ext_idx(F_TT_DYCK_IDX)));

    write_pelt(table, &data.f_tth.size, row, &Col(ext_idx(F_TTH_SIZE_IDX)));
    write_pelt(table, &data.f_tth.leaf, row, &Col(ext_idx(F_TTH_LEAF_IDX)));
    write_pelt(table, &data.f_tth.dyck, row, &Col(ext_idx(F_TTH_DYCK_IDX)));

    write_pelt(table, &data.f_ttt.size, row, &Col(ext_idx(F_TTT_SIZE_IDX)));
    write_pelt(table, &data.f_ttt.leaf, row, &Col(ext_idx(F_TTT_LEAF_IDX)));
    write_pelt(table, &data.f_ttt.dyck, row, &Col(ext_idx(F_TTT_DYCK_IDX)));

    write_pelt(table, &data.fcons_inv, row, &Col(ext_idx(FCONS_INV_IDX)));
}

fn build_compute_queue(list: Noun, alf: &Felt) -> Result<Vec<TreeData>, JetErr> {
    let mut res: Vec<TreeData> = Vec::<TreeData>::new();

    for n in HoonList::try_from(list)?.into_iter() {
        let tree_data = build_tree_data(n, alf)?;
        res.push(tree_data)
    }
    Ok(res)
}

fn header(context: &mut Context) -> Noun {
    let prime: Noun = Atom::new(&mut context.stack, PRIME).as_noun();
    let header: Noun = T(
        &mut context.stack,
        &[
            D(TABLE_NAME),
            prime,
            D(NUM_BASIC_COLS),
            D(NUM_EXT_COLS),
            D(NUM_MEGA_EXT_COLS),
            D(NUM_BASIC_COLS + NUM_EXT_COLS + NUM_MEGA_EXT_COLS),
            D(1), // num-randomizers
        ],
    );
    header
}

fn mega_idx(idx: usize) -> usize {
    idx - ((NUM_BASIC_COLS + NUM_EXT_COLS) as usize)
}

fn ext_idx(idx: usize) -> usize {
    idx - (NUM_BASIC_COLS as usize)
}

const TABLE_NAME: u64 = tas!(b"compute");
const NUM_BASIC_COLS: u64 = 11;
const NUM_EXT_COLS: u64 = 165;
const NUM_MEGA_EXT_COLS: u64 = 18;

// column indices
// base columns (belts)
const PAD_IDX: usize = 0;
const OP0_IDX: usize = 1;
const OP1_IDX: usize = 2;
const OP2_IDX: usize = 3;
const OP3_IDX: usize = 4;
const OP4_IDX: usize = 5;
const OP5_IDX: usize = 6;
const OP6_IDX: usize = 7;
const OP7_IDX: usize = 8;
const OP8_IDX: usize = 9;
const OP9_IDX: usize = 10;

// extension columns (pelts)
const S_SIZE_IDX: usize = 11;
const S_LEAF_IDX: usize = 14;
const S_DYCK_IDX: usize = 17;
//
const F_SIZE_IDX: usize = 20;
const F_LEAF_IDX: usize = 23;
const F_DYCK_IDX: usize = 26;
//
const E_SIZE_IDX: usize = 29;
const E_LEAF_IDX: usize = 32;
const E_DYCK_IDX: usize = 35;
//
const SF1_S_SIZE_IDX: usize = 38;
const SF1_S_LEAF_IDX: usize = 41;
const SF1_S_DYCK_IDX: usize = 44;
//
const SF1_F_SIZE_IDX: usize = 47;
const SF1_F_LEAF_IDX: usize = 50;
const SF1_F_DYCK_IDX: usize = 53;
//
const SF1_E_SIZE_IDX: usize = 56;
const SF1_E_LEAF_IDX: usize = 59;
const SF1_E_DYCK_IDX: usize = 62;
//
const SF2_S_SIZE_IDX: usize = 65;
const SF2_S_LEAF_IDX: usize = 68;
const SF2_S_DYCK_IDX: usize = 71;
//
const SF2_F_SIZE_IDX: usize = 74;
const SF2_F_LEAF_IDX: usize = 77;
const SF2_F_DYCK_IDX: usize = 80;
//
const SF2_E_SIZE_IDX: usize = 83;
const SF2_E_LEAF_IDX: usize = 86;
const SF2_E_DYCK_IDX: usize = 89;
//
const SF3_S_SIZE_IDX: usize = 92;
const SF3_S_LEAF_IDX: usize = 95;
const SF3_S_DYCK_IDX: usize = 98;
//
const SF3_F_SIZE_IDX: usize = 101;
const SF3_F_LEAF_IDX: usize = 104;
const SF3_F_DYCK_IDX: usize = 107;
//
const SF3_E_SIZE_IDX: usize = 110;
const SF3_E_LEAF_IDX: usize = 113;
const SF3_E_DYCK_IDX: usize = 116;
//
const F_H_SIZE_IDX: usize = 119;
const F_H_LEAF_IDX: usize = 122;
const F_H_DYCK_IDX: usize = 125;
//
const F_T_SIZE_IDX: usize = 128;
const F_T_LEAF_IDX: usize = 131;
const F_T_DYCK_IDX: usize = 134;
//
const F_TH_SIZE_IDX: usize = 137;
const F_TH_LEAF_IDX: usize = 140;
const F_TH_DYCK_IDX: usize = 143;
//
const F_TT_SIZE_IDX: usize = 146;
const F_TT_LEAF_IDX: usize = 149;
const F_TT_DYCK_IDX: usize = 152;
//
const F_TTH_SIZE_IDX: usize = 155;
const F_TTH_LEAF_IDX: usize = 158;
const F_TTH_DYCK_IDX: usize = 161;
//
const F_TTT_SIZE_IDX: usize = 164;
const F_TTT_LEAF_IDX: usize = 167;
const F_TTT_DYCK_IDX: usize = 170;
//
const FCONS_INV_IDX: usize = 173;

// mega-extension columns (pelts)
const LN_IDX: usize = 176;
const SFCONS_INV_IDX: usize = 179;
const OPC_IDX: usize = 182;
const STACK_KV_IDX: usize = 185;
const DECODE_MSET_IDX: usize = 188;
const OP0_MSET_IDX: usize = 191;
