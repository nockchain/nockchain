use nockvm::jets::JetErr;
use nockvm::noun::Noun;

use crate::form::belt::Belt;
use crate::form::felt::*;
use crate::form::mary::*;
use crate::form::structs::HoonList;

pub struct ExtChals {
    pub a: Felt,
    pub b: Felt,
    pub c: Felt,
    pub _d: Felt,
    pub _e: Felt,
    pub _f: Felt,
    pub _g: Felt,
    pub _p: Felt,
    pub _q: Felt,
    pub _r: Felt,
    pub _s: Felt,
    pub _t: Felt,
    pub _u: Felt,
    pub alf: Felt,
}

#[derive(Debug)]
pub struct MegaExtChals {
    pub a: Felt,
    pub b: Felt,
    pub c: Felt,
    pub d: Felt,
    pub e: Felt,
    pub f: Felt,
    pub g: Felt,
    pub p: Felt,
    pub q: Felt,
    pub r: Felt,
    pub s: Felt,
    pub t: Felt,
    pub u: Felt,
    pub alf: Felt,
    pub j: Felt,
    pub k: Felt,
    pub l: Felt,
    pub m: Felt,
    pub n: Felt,
    pub o: Felt,
    pub w: Felt,
    pub x: Felt,
    pub y: Felt,
    pub z: Felt,
    pub bet: Felt,
    pub gam: Felt,
}

pub fn init_ext_chals(chals: Noun) -> Result<ExtChals, JetErr> {
    let mut belts: Vec<u64> = Vec::<u64>::with_capacity(100);
    for b in HoonList::try_from(chals)?.into_iter() {
        belts.push(b.as_atom()?.as_u64()?);
    }
    let mut felts: Vec<Felt> = Vec::<Felt>::with_capacity(30);
    for trip in belts.chunks(3) {
        let felt: Felt = Felt::try_from(trip).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        felts.push(felt);
    }

    Ok(ExtChals {
        a: felts[0],
        b: felts[1],
        c: felts[2],
        _d: felts[3],
        _e: felts[4],
        _f: felts[5],
        _g: felts[6],
        _p: felts[7],
        _q: felts[8],
        _r: felts[9],
        _s: felts[10],
        _t: felts[11],
        _u: felts[12],
        alf: felts[13],
    })
}

pub fn init_mega_ext_chals(chals: Noun) -> Result<MegaExtChals, JetErr> {
    let mut belts: Vec<u64> = Vec::<u64>::with_capacity(100);
    for b in HoonList::try_from(chals)?.into_iter() {
        belts.push(b.as_atom()?.as_u64()?);
    }
    let mut felts: Vec<Felt> = Vec::<Felt>::with_capacity(30);
    for trip in belts.chunks(3) {
        let felt: Felt = Felt::try_from(trip).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        felts.push(felt);
    }

    Ok(MegaExtChals {
        a: felts[0],
        b: felts[1],
        c: felts[2],
        d: felts[3],
        e: felts[4],
        f: felts[5],
        g: felts[6],
        p: felts[7],
        q: felts[8],
        r: felts[9],
        s: felts[10],
        t: felts[11],
        u: felts[12],
        alf: felts[13],
        j: felts[14],
        k: felts[15],
        l: felts[16],
        m: felts[17],
        n: felts[18],
        o: felts[19],
        w: felts[20],
        x: felts[21],
        y: felts[22],
        z: felts[23],
        bet: felts[24],
        gam: felts[25],
    })
}

pub struct Row(pub usize);
pub struct Col(pub usize);

pub fn _write_belt(table: &mut MarySliceMut, b: Belt, row: &Row, col: &Col) {
    table.dat[(row.0 * (table.step as usize)) + col.0] = b.0
}

pub fn write_pelt(table: &mut MarySliceMut, f: &Felt, row: &Row, col: &Col) {
    table.dat[(row.0 * (table.step as usize)) + col.0] = f.0[0].0;
    table.dat[(row.0 * (table.step as usize)) + col.0 + 1] = f.0[1].0;
    table.dat[(row.0 * (table.step as usize)) + col.0 + 2] = f.0[2].0;
}

pub fn grab_pelt(row: &[u64], idx: usize) -> Felt {
    //  TODO: see if we can/should remove the copy
    let mut ret: Felt = Felt::zero();
    ret.0[0] = Belt(row[idx]);
    ret.0[1] = Belt(row[idx + 1]);
    ret.0[2] = Belt(row[idx + 2]);
    ret
}

pub fn grab_belt(row: &[u64], idx: usize) -> Belt {
    Belt(row[idx])
}

pub fn get_row<'a>(table: &'a MarySlice<'a>, num_u32: u32) -> &'a [u64] {
    let num: usize = num_u32 as usize;
    let step = table.step as usize;

    &table.dat[(step * num)..(step * (num + 1))]
}

#[derive(Copy, Clone)]
pub struct Ion {
    pub size: Felt,
    pub leaf: Felt,
    pub dyck: Felt,
}

pub fn fadd_all(felts: Vec<Felt>) -> Felt {
    let mut res: Felt = Felt::zero();
    for f in felts {
        fadd_self(&f, &mut res)
    }
    res
}

pub fn fmul_all(felts: Vec<Felt>) -> Felt {
    let mut res: Felt = Felt::one();
    for f in felts {
        res = fmul_(&f, &res);
    }
    res
}
