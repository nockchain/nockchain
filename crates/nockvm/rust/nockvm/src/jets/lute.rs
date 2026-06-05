use nockvm_macros::tas;

/** ++ut jets (compiler backend and pretty-printer)
 */
use crate::interpreter::{interpret, Context};
use crate::jets::util::*;
use crate::jets::Result;
use crate::noun::{Noun, D, NO, NONE, T, YES};

crate::gdb!();

pub fn jet_ut_crop(context: &mut Context, subject: Noun) -> Result {
    let rff = slot(subject, 6)?;
    let van = slot(subject, 7)?;

    let bat = slot(van, 2)?;
    let sut = slot(van, 6)?;

    let flag = if let Ok(noun) = slot(van, 59) {
        if unsafe { noun.raw_equals(&D(0)) } {
            0u64
        } else {
            1u64
        }
    } else {
        1
    };
    let fun = 141 + tas!(b"crop") + (flag << 8);
    let mut key = T(&mut context.stack, &[D(fun), sut, rff, bat]);

    match context.cache.lookup(&mut context.stack, &mut key) {
        Some(pro) => Ok(pro),
        None => {
            let pro = interpret(context, subject, slot(subject, 2)?)?;
            context.cache = context.cache.insert(&mut context.stack, &mut key, pro);
            Ok(pro)
        }
    }
}

pub fn jet_ut_fish(context: &mut Context, subject: Noun) -> Result {
    //  axe must be Atom, though we use it as Noun
    let axe = slot(subject, 6)?.as_atom()?;
    let van = slot(subject, 7)?;

    let bat = slot(van, 2)?;
    let sut = slot(van, 6)?;

    let flag = if let Ok(noun) = slot(van, 59) {
        if unsafe { noun.raw_equals(&D(0)) } {
            0u64
        } else {
            1u64
        }
    } else {
        1
    };
    let fun = 141 + tas!(b"fish") + (flag << 8);
    let mut key = T(&mut context.stack, &[D(fun), sut, axe.as_noun(), bat]);

    match context.cache.lookup(&mut context.stack, &mut key) {
        Some(pro) => Ok(pro),
        None => {
            let pro = interpret(context, subject, slot(subject, 2)?)?;
            context.cache = context.cache.insert(&mut context.stack, &mut key, pro);
            Ok(pro)
        }
    }
}

pub fn jet_ut_fuse(context: &mut Context, subject: Noun) -> Result {
    let rff = slot(subject, 6)?;
    let van = slot(subject, 7)?;

    let bat = slot(van, 2)?;
    let sut = slot(van, 6)?;

    let flag = if let Ok(noun) = slot(van, 59) {
        if unsafe { noun.raw_equals(&D(0)) } {
            0u64
        } else {
            1u64
        }
    } else {
        1
    };
    let fun = 141 + tas!(b"fuse") + (flag << 8);
    let mut key = T(&mut context.stack, &[D(fun), sut, rff, bat]);

    match context.cache.lookup(&mut context.stack, &mut key) {
        Some(pro) => Ok(pro),
        None => {
            let pro = interpret(context, subject, slot(subject, 2)?)?;
            context.cache = context.cache.insert(&mut context.stack, &mut key, pro);
            Ok(pro)
        }
    }
}

pub fn jet_ut_mint(context: &mut Context, subject: Noun) -> Result {
    let gol = slot(subject, 12)?;
    let gen = slot(subject, 13)?;
    let van = slot(subject, 7)?;

    let bat = slot(van, 2)?;
    let sut = slot(van, 6)?;

    let fun = 141 + tas!(b"mint");
    let vet = slot(van, 59).map_or(NONE, |x| x);
    let mut key = T(&mut context.stack, &[D(fun), vet, sut, gol, gen, bat]);

    match context.cache.lookup(&mut context.stack, &mut key) {
        Some(pro) => Ok(pro),
        None => {
            let pro = interpret(context, subject, slot(subject, 2)?)?;
            context.cache = context.cache.insert(&mut context.stack, &mut key, pro);
            Ok(pro)
        }
    }
}

pub fn jet_ut_mull(context: &mut Context, subject: Noun) -> Result {
    let gol = slot(subject, 12)?;
    let dox = slot(subject, 26)?;
    let gen = slot(subject, 27)?;
    let van = slot(subject, 7)?;

    let bat = slot(van, 2)?;
    let sut = slot(van, 6)?;

    let flag = if let Ok(noun) = slot(van, 59) {
        if unsafe { noun.raw_equals(&D(0)) } {
            0u64
        } else {
            1u64
        }
    } else {
        1
    };
    let fun = 141 + tas!(b"mull") + (flag << 8);
    let mut key = T(&mut context.stack, &[D(fun), sut, gol, dox, gen, bat]);

    match context.cache.lookup(&mut context.stack, &mut key) {
        Some(pro) => Ok(pro),
        None => {
            let pro = interpret(context, subject, slot(subject, 2)?)?;
            context.cache = context.cache.insert(&mut context.stack, &mut key, pro);
            Ok(pro)
        }
    }
}

pub fn jet_ut_nest_dext(context: &mut Context, subject: Noun) -> Result {
    let nest_in_core = slot(subject, 3)?;

    let seg = slot(nest_in_core, 12)?;
    let reg = slot(nest_in_core, 26)?;
    let nest_core = slot(nest_in_core, 7)?;

    let rff = slot(nest_core, 13)?;
    let van = slot(nest_core, 7)?;

    let bat = slot(van, 2)?;
    let sut = slot(van, 6)?;

    let flag = if let Ok(noun) = slot(van, 59) {
        if unsafe { noun.raw_equals(&D(0)) } {
            0u64
        } else {
            1u64
        }
    } else {
        1
    };
    let fun = (141 + tas!(b"dext")) + (flag << 8);
    let mut key = T(&mut context.stack, &[D(fun), sut, rff, bat]);

    match context.cache.lookup(&mut context.stack, &mut key) {
        Some(pro) => Ok(pro),
        None => {
            let pro = interpret(context, subject, slot(subject, 2)?)?;
            if unsafe { pro.raw_equals(&YES) && reg.raw_equals(&D(0)) }
                || unsafe { pro.raw_equals(&NO) && seg.raw_equals(&D(0)) }
            {
                context.cache = context.cache.insert(&mut context.stack, &mut key, pro);
            }
            Ok(pro)
        }
    }
}

pub fn jet_ut_redo(context: &mut Context, subject: Noun) -> Result {
    let rff = slot(subject, 6)?;
    let van = slot(subject, 7)?;
    let bat = slot(van, 2)?;
    let sut = slot(van, 6)?;

    let flag = if let Ok(noun) = slot(van, 59) {
        if unsafe { noun.raw_equals(&D(0)) } {
            0u64
        } else {
            1u64
        }
    } else {
        1
    };
    let fun = 141 + tas!(b"redo") + (flag << 8);
    let mut key = T(&mut context.stack, &[D(fun), sut, rff, bat]);

    match context.cache.lookup(&mut context.stack, &mut key) {
        Some(pro) => Ok(pro),
        None => {
            let pro = interpret(context, subject, slot(subject, 2)?)?;
            context.cache = context.cache.insert(&mut context.stack, &mut key, pro);
            Ok(pro)
        }
    }
}

pub fn jet_ut_rest(context: &mut Context, subject: Noun) -> Result {
    let leg = slot(subject, 6)?;
    let van = slot(subject, 7)?;

    let bat = slot(van, 2)?;
    let sut = slot(van, 6)?;

    let flag = if let Ok(noun) = slot(van, 59) {
        if unsafe { noun.raw_equals(&D(0)) } {
            0u64
        } else {
            1u64
        }
    } else {
        1
    };
    let fun = 141 + tas!(b"rest") + (flag << 8);
    let mut key = T(&mut context.stack, &[D(fun), sut, leg, bat]);

    match context.cache.lookup(&mut context.stack, &mut key) {
        Some(pro) => Ok(pro),
        None => {
            let pro = interpret(context, subject, slot(subject, 2)?)?;
            context.cache = context.cache.insert(&mut context.stack, &mut key, pro);
            Ok(pro)
        }
    }
}
