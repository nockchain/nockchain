use std::collections::VecDeque;

use nockvm::hamt::MutHamt;
use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::mem::NockStack;
use nockvm::noun::{Atom, IndirectAtom, Noun, D, T};
use nockvm_macros::tas;
use tracing::debug;

use crate::form::belt::*;
use crate::form::felt::*;
use crate::form::gen_trace::{build_tree_data, TreeData};
use crate::form::handle::{finalize_mary, new_handle_mut_mary};
use crate::form::mary::*;
use crate::jets::table_utils::*;

pub fn memory_v2_extend_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let sam = slot(subject, 6)?;
    let table_mary = slot(sam, 2)?;
    let chals_rd1 = slot(sam, 6)?;
    let fock_ret = slot(sam, 7)?;
    let sf = slot(fock_ret, 15)?;
    let subject = slot(sf, 2)?;
    let formula = slot(sf, 3)?;

    let chals: ExtChals = init_ext_chals(chals_rd1)?;

    let table_noun = slot(table_mary, 3)?;
    let Ok(table) = MarySlice::try_from(table_noun) else {
        debug!("cannot convert mary arg to mary");
        return Err(BAIL_FAIL);
    };

    let (res, mut res_mary): (IndirectAtom, MarySliceMut) = new_handle_mut_mary(
        &mut context.stack, NUM_EXT_COLS as usize, table.len as usize,
    );

    let build_and_bft = add_ions(
        &mut context.stack,
        &mut rna_bfta(vec![(subject, true), (formula, false)])?,
        &chals,
    )?;

    build_and_bft.iter().enumerate().for_each(|(i, mb)| {
        let row_idx = Row(i);

        write_pelt(
            &mut res_mary,
            &mb.parent.size,
            &row_idx,
            &Col(ext_idx(PARENT_SIZE_IDX)),
        );
        write_pelt(
            &mut res_mary,
            &mb.parent.dyck,
            &row_idx,
            &Col(ext_idx(PARENT_DYCK_IDX)),
        );
        write_pelt(
            &mut res_mary,
            &mb.parent.leaf,
            &row_idx,
            &Col(ext_idx(PARENT_LEAF_IDX)),
        );

        write_pelt(
            &mut res_mary,
            &mb.left.size,
            &row_idx,
            &Col(ext_idx(LC_SIZE_IDX)),
        );
        write_pelt(
            &mut res_mary,
            &mb.left.dyck,
            &row_idx,
            &Col(ext_idx(LC_DYCK_IDX)),
        );
        write_pelt(
            &mut res_mary,
            &mb.left.leaf,
            &row_idx,
            &Col(ext_idx(LC_LEAF_IDX)),
        );

        write_pelt(
            &mut res_mary,
            &mb.right.size,
            &row_idx,
            &Col(ext_idx(RC_SIZE_IDX)),
        );
        write_pelt(
            &mut res_mary,
            &mb.right.dyck,
            &row_idx,
            &Col(ext_idx(RC_DYCK_IDX)),
        );
        write_pelt(
            &mut res_mary,
            &mb.right.leaf,
            &row_idx,
            &Col(ext_idx(RC_LEAF_IDX)),
        );

        let inv: Felt = finv_(&fmul_(
            &fsub_(&mb.parent.size, &Felt::one()),
            &fmul_(
                &fsub_(&mb.left.size, &Felt::one()),
                &fsub_(&mb.right.size, &Felt::one()),
            ),
        ));

        write_pelt(&mut res_mary, &inv, &row_idx, &Col(ext_idx(INV_IDX)));
    });

    // padded columns are all 0 except for %inv which is -1
    let neg_one: Felt = fsub_(&Felt::zero(), &Felt::one());
    for i in build_and_bft.len()..(table.len as usize) {
        write_pelt(&mut res_mary, &neg_one, &Row(i), &Col(ext_idx(INV_IDX)));
    }

    let res_cell = finalize_mary(
        &mut context.stack, NUM_EXT_COLS as usize, table.len as usize, res,
    );
    let header = header(context);
    Ok(T(&mut context.stack, &[header, res_cell]))
}

fn rna_bfta(tres: Vec<(Noun, bool)>) -> Result<Vec<Noun>, JetErr> {
    let mut queue: VecDeque<(Noun, Belt)> = tres
        .iter()
        .filter(|(n, _b)| n.is_cell())
        .map(|(n, b)| (*n, if *b { Belt::one() } else { Belt::zero() }))
        .collect();

    let mut res: Vec<Noun> = Vec::<Noun>::new();

    while !queue.is_empty() {
        let (noun, ax) = queue.pop_front().unwrap_or_else(|| {
            panic!(
                "Panicked at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        let head = noun.as_cell()?.head();
        let tail = noun.as_cell()?.tail();

        match (head.is_atom(), tail.is_atom()) {
            (true, true) => {}
            (true, false) => {
                queue.push_back((tail, go_right(&ax)));
            }
            (false, true) => {
                queue.push_back((head, go_left(&ax)));
            }
            (false, false) => {
                queue.push_back((head, go_left(&ax)));
                queue.push_back((tail, go_right(&ax)));
            }
        };
        res.push(noun);
    }

    res.reverse();

    Ok(res)
}

fn add_ions(
    stack: &mut NockStack,
    lst: &mut Vec<Noun>,
    chals: &ExtChals,
) -> Result<Vec<MemoryBankEx>, JetErr> {
    let mut res = Vec::<MemoryBankEx>::new();

    let cache = MutHamt::<Ion>::new(stack);

    for noun in lst {
        let mut head = noun.as_cell()?.head();
        let mut tail = noun.as_cell()?.tail();

        let left: Ion = if head.is_atom() {
            atom_ion(head.as_atom()?, &chals.alf)?
        } else {
            cache.lookup(stack, &mut head).unwrap_or_else(|| {
                panic!(
                    "Panicked at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            })
        };

        let right: Ion = if tail.is_atom() {
            atom_ion(tail.as_atom()?, &chals.alf)?
        } else {
            cache.lookup(stack, &mut tail).unwrap_or_else(|| {
                panic!(
                    "Panicked at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            })
        };

        let parent: Ion = cons_ion(&chals.alf, &left, &right);
        cache.insert(stack, noun, parent);

        let mbe = MemoryBankEx {
            parent,
            left,
            right,
        };
        res.push(mbe);
    }
    res.reverse();
    Ok(res)
}

fn cons_ion(alf: &Felt, left: &Ion, right: &Ion) -> Ion {
    let alfinv = finv_(alf);
    let size = fmul_(&left.size, &right.size);
    let dyck = fadd_all(vec![
        fmul_all(vec![right.size, right.size, alfinv, left.dyck]),
        fmul_all(vec![right.size, right.size, alfinv, alfinv]),
        right.dyck,
    ]);
    let leaf = fadd_(&fmul_(&right.size, &left.leaf), &right.leaf);
    Ion { size, dyck, leaf }
}

fn atom_ion(atom: Atom, alf: &Felt) -> Result<Ion, JetErr> {
    Ok(Ion {
        size: *alf,
        dyck: Felt::zero(),
        leaf: Felt::lift(Belt(atom.as_u64()?)),
    })
}

struct MemoryBankEx {
    parent: Ion,
    left: Ion,
    right: Ion,
}

pub fn memory_v2_mega_extend_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let sam = slot(subject, 6)?;
    let table_mary = slot(sam, 2)?;
    let all_chals = slot(sam, 6)?;
    let fock_ret = slot(sam, 7)?;
    let sf = slot(fock_ret, 15)?;
    let subject = slot(sf, 2)?;
    let subject_is_atom: bool = subject.is_atom();

    let chals: MegaExtChals = init_mega_ext_chals(all_chals)?;
    let z2: Felt = fmul_(&chals.z, &chals.z);

    let table_noun = slot(table_mary, 3)?;
    let Ok(table) = MarySlice::try_from(table_noun) else {
        debug!("cannot convert mary arg to mary");
        return Err(BAIL_FAIL);
    };

    let (res, mut res_mary): (IndirectAtom, MarySliceMut) = new_handle_mut_mary(
        &mut context.stack, NUM_MEGA_EXT_COLS as usize, table.len as usize,
    );

    let first_row = get_row(&table, 0);
    let second_row = get_row(&table, 1);

    let subj_fp_triple: TreeData = build_tree_data(subject, &chals.alf)?;
    let input: Felt = ifp_compress(
        &Ion {
            size: subj_fp_triple.size,
            dyck: subj_fp_triple.dyck,
            leaf: subj_fp_triple.leaf,
        },
        &chals.a,
        &chals.b,
        &chals.c,
    );

    let first_row_ax: Belt = grab_belt(first_row, AXIS_IDX);
    let first_row_fp: Felt = ifp_compress(
        &Ion {
            size: grab_pelt(first_row, PARENT_SIZE_IDX),
            dyck: grab_pelt(first_row, PARENT_DYCK_IDX),
            leaf: grab_pelt(first_row, PARENT_LEAF_IDX),
        },
        &chals.j,
        &chals.k,
        &chals.l,
    );

    let second_row_ax: Belt = grab_belt(second_row, AXIS_IDX);
    let second_row_fp: Felt = ifp_compress(
        &Ion {
            size: grab_pelt(second_row, PARENT_SIZE_IDX),
            dyck: grab_pelt(second_row, PARENT_DYCK_IDX),
            leaf: grab_pelt(second_row, PARENT_LEAF_IDX),
        },
        &chals.j,
        &chals.k,
        &chals.l,
    );
    let subj_info_ax: Belt = if subject_is_atom {
        Belt::zero()
    } else {
        first_row_ax
    };

    let subj_info_fp: Felt = if subject_is_atom {
        Felt::zero()
    } else {
        first_row_fp
    };

    let form_info_ax: Belt = if subject_is_atom {
        first_row_ax
    } else {
        second_row_ax
    };

    let form_info_fp: Felt = if subject_is_atom {
        first_row_fp
    } else {
        second_row_fp
    };

    let input_subj_fp: Felt = fadd_(&subj_info_fp, &fscal_(&subj_info_ax, &chals.m));
    let input_form_fp: Felt = fadd_(&form_info_fp, &fscal_(&form_info_ax, &chals.m));

    let mut line_ct: Felt = chals.z;
    let mut node_ct: Felt = if subject_is_atom { chals.z } else { z2 };
    let mut decode_mset: Felt = Felt::zero();
    let mut op0_mset: Felt = Felt::zero();
    let mut kvs: Felt = if subject_is_atom {
        fmul_(&chals.z, &input_form_fp)
    } else {
        fadd_(
            &fmul_(&chals.z, &input_subj_fp),
            &fmul_(&z2, &input_form_fp),
        )
    };

    for i in 0..table.len {
        let row: &[u64] = get_row(&table, i);

        let parent: Ion = Ion {
            size: grab_pelt(row, PARENT_SIZE_IDX),
            dyck: grab_pelt(row, PARENT_DYCK_IDX),
            leaf: grab_pelt(row, PARENT_LEAF_IDX),
        };
        let left: Ion = Ion {
            size: grab_pelt(row, LC_SIZE_IDX),
            dyck: grab_pelt(row, LC_DYCK_IDX),
            leaf: grab_pelt(row, LC_LEAF_IDX),
        };
        let right: Ion = Ion {
            size: grab_pelt(row, RC_SIZE_IDX),
            dyck: grab_pelt(row, RC_DYCK_IDX),
            leaf: grab_pelt(row, RC_LEAF_IDX),
        };

        let left_is_atom: bool = grab_belt(row, OP_L_IDX).is_zero();
        let right_is_atom: bool = grab_belt(row, OP_R_IDX).is_zero();
        let ax: Belt = grab_belt(row, AXIS_IDX);

        let par: Felt = ifp_compress(&parent, &chals.j, &chals.k, &chals.l);
        let wt_pax: Felt = fscal_(&ax, &chals.m);

        let lc: Felt = ifp_compress(&left, &chals.j, &chals.k, &chals.l);
        let wt_lax: Felt = fscal_(&go_left(&ax), &chals.m);

        let rc: Felt = ifp_compress(&right, &chals.j, &chals.k, &chals.l);
        let wt_rax: Felt = fscal_(&go_right(&ax), &chals.m);

        let new_line_ct = fmul_(&line_ct, &chals.z);
        let new_node_ct = if left_is_atom {
            if right_is_atom {
                node_ct
            } else {
                fmul_(&node_ct, &chals.z)
            }
        } else if right_is_atom {
            fmul_(&node_ct, &chals.z)
        } else {
            fmul_(&node_ct, &z2)
        };
        let new_kvs: Felt = fsub_(
            &fadd_(
                &kvs,
                &fadd_(
                    &if left_is_atom {
                        Felt::zero()
                    } else {
                        fmul_(&chals.z, &fmul_(&node_ct, &fadd_(&lc, &wt_lax)))
                    },
                    &fadd_(
                        &if !left_is_atom || right_is_atom {
                            Felt::zero()
                        } else {
                            fmul_(&chals.z, &fmul_(&node_ct, &fadd_(&rc, &wt_rax)))
                        },
                        &if left_is_atom || right_is_atom {
                            Felt::zero()
                        } else {
                            fmul_(&z2, &fmul_(&node_ct, &fadd_(&rc, &wt_rax)))
                        },
                    ),
                ),
            ),
            &fmul_(&fadd_(&par, &wt_pax), &line_ct),
        );

        let new_decode_mset: Felt = {
            let dat: Felt = fadd_all(vec![
                fmul_(&chals.j, &grab_pelt(row, PARENT_SIZE_IDX)),
                fmul_(&chals.k, &grab_pelt(row, PARENT_DYCK_IDX)),
                fmul_(&chals.l, &grab_pelt(row, PARENT_LEAF_IDX)),
                fmul_(&chals.m, &grab_pelt(row, LC_SIZE_IDX)),
                fmul_(&chals.n, &grab_pelt(row, LC_DYCK_IDX)),
                fmul_(&chals.o, &grab_pelt(row, LC_LEAF_IDX)),
                fmul_(&chals.w, &grab_pelt(row, RC_SIZE_IDX)),
                fmul_(&chals.x, &grab_pelt(row, RC_DYCK_IDX)),
                fmul_(&chals.y, &grab_pelt(row, RC_LEAF_IDX)),
            ]);
            let mult: Belt = grab_belt(row, DMULT_IDX);
            ld_add(&chals.gam, &decode_mset, &dat, mult)
        };

        let new_op0_mset: Felt = {
            let mset1: Felt = ld_add(
                &chals.bet,
                &op0_mset,
                &fadd_(&input, &fadd_(&wt_pax, &par)),
                grab_belt(row, MULT_IDX),
            );
            let mset2: Felt = if !left_is_atom {
                mset1
            } else {
                ld_add(
                    &chals.bet,
                    &mset1,
                    &fadd_(&input, &fadd_(&wt_lax, &lc)),
                    grab_belt(row, MULT_LC_IDX),
                )
            };
            let mset3: Felt = if !right_is_atom {
                mset2
            } else {
                ld_add(
                    &chals.bet,
                    &mset2,
                    &fadd_(&input, &fadd_(&wt_rax, &rc)),
                    grab_belt(row, MULT_RC_IDX),
                )
            };
            mset3
        };

        let kvs_pioz: Felt = pioz(&kvs);

        let data_k: Felt = {
            let p1 = fadd_all(vec![
                fmul_(&chals.j, &line_ct),
                fmul_(&chals.k, &node_ct),
                fmul_(&chals.l, &kvs),
                fmul_(&chals.m, &kvs_pioz),
            ]);
            let p2 = fadd_all(vec![
                fmul_(&chals.n, &line_ct),
                fmul_(&chals.o, &node_ct),
                fmul_(&chals.w, &kvs),
                fmul_(&chals.x, &kvs_pioz),
            ]);
            fmul_all(vec![p1, p2, fadd_(&p1, &p2), kvs_pioz])
        };

        let row_idx: Row = Row(i as usize);
        write_pelt(&mut res_mary, &line_ct, &row_idx, &Col(mega_idx(LN_IDX)));
        write_pelt(&mut res_mary, &node_ct, &row_idx, &Col(mega_idx(NC_IDX)));
        write_pelt(&mut res_mary, &kvs, &row_idx, &Col(mega_idx(KVS_IDX)));
        write_pelt(
            &mut res_mary,
            &kvs_pioz,
            &row_idx,
            &Col(mega_idx(KVS_IOZ_IDX)),
        );
        write_pelt(
            &mut res_mary,
            &fmul_(&kvs, &kvs_pioz),
            &row_idx,
            &Col(mega_idx(KVSF_IDX)),
        );
        write_pelt(
            &mut res_mary,
            &decode_mset,
            &row_idx,
            &Col(mega_idx(DECODE_MSET_IDX)),
        );
        write_pelt(
            &mut res_mary,
            &op0_mset,
            &row_idx,
            &Col(mega_idx(OP0_MSET_IDX)),
        );
        write_pelt(&mut res_mary, &data_k, &row_idx, &Col(mega_idx(DATA_K_IDX)));

        line_ct = new_line_ct;
        node_ct = new_node_ct;
        decode_mset = new_decode_mset;
        op0_mset = new_op0_mset;
        kvs = new_kvs;
    }

    let res_cell = finalize_mary(
        &mut context.stack, NUM_MEGA_EXT_COLS as usize, table.len as usize, res,
    );
    let header = header(context);
    Ok(T(&mut context.stack, &[header, res_cell]))
}

fn mega_idx(idx: usize) -> usize {
    idx - ((NUM_BASIC_COLS + NUM_EXT_COLS) as usize)
}

fn ext_idx(idx: usize) -> usize {
    idx - (NUM_BASIC_COLS as usize)
}

fn pioz(f: &Felt) -> Felt {
    if f.is_zero() {
        Felt::zero()
    } else {
        finv_(f)
    }
}

fn ld_add(chal: &Felt, old_val: &Felt, f: &Felt, n: Belt) -> Felt {
    fadd_(old_val, &fscal_(&n, &finv_(&fsub_(chal, f))))
}

fn go_left(ax: &Belt) -> Belt {
    *ax * Belt(2)
}

fn go_right(ax: &Belt) -> Belt {
    if ax.is_zero() {
        Belt::zero()
    } else {
        (*ax * Belt(2)) + Belt::one()
    }
}

fn ifp_compress(ion: &Ion, a: &Felt, b: &Felt, c: &Felt) -> Felt {
    fadd_(
        &fadd_(&fmul_(a, &ion.size), &fmul_(b, &ion.dyck)),
        &fmul_(c, &ion.leaf),
    )
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

const TABLE_NAME: u64 = tas!(b"memory");
const NUM_BASIC_COLS: u64 = 14;
const NUM_EXT_COLS: u64 = 30;
const NUM_MEGA_EXT_COLS: u64 = 24;

// column indices
// base columns (belts)
const _PAD_IDX: usize = 0;
const AXIS_IDX: usize = 1;
const _AXIS_IOZ_IDX: usize = 2;
const _AXIS_FLAG_IDX: usize = 3;
const _LEAF_L_IDX: usize = 4;
const _LEAF_R_IDX: usize = 5;
const OP_L_IDX: usize = 6;
const OP_R_IDX: usize = 7;
const _COUNT_IDX: usize = 8;
const _COUNT_INV_IDX: usize = 9;
const DMULT_IDX: usize = 10;
const MULT_IDX: usize = 11;
const MULT_LC_IDX: usize = 12;
const MULT_RC_IDX: usize = 13;

// extension columns (pelts)
const PARENT_SIZE_IDX: usize = 14;
const PARENT_DYCK_IDX: usize = 17;
const PARENT_LEAF_IDX: usize = 20;
const LC_SIZE_IDX: usize = 23;
const LC_DYCK_IDX: usize = 26;
const LC_LEAF_IDX: usize = 29;
const RC_SIZE_IDX: usize = 32;
const RC_DYCK_IDX: usize = 35;
const RC_LEAF_IDX: usize = 38;
const INV_IDX: usize = 41;

// mega-extension columns (pelts)
const LN_IDX: usize = 44;
const NC_IDX: usize = 47;
const KVS_IDX: usize = 50;
const KVS_IOZ_IDX: usize = 53;
const KVSF_IDX: usize = 56;
const DECODE_MSET_IDX: usize = 59;
const OP0_MSET_IDX: usize = 62;
const DATA_K_IDX: usize = 65;
