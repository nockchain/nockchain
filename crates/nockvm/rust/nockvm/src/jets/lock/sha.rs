use nockvm_crypto::sha::{ac_sha1, ac_shal, ac_shas, ac_shay};

use crate::interpreter::Context;
use crate::jets::bits::util::met;
use crate::jets::util::{slot, BAIL_FAIL};
use crate::jets::Result;
use crate::noun::{IndirectAtom, Noun};

crate::gdb!();

pub fn jet_shas(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let sam = slot(subject, 6)?;
    let sal = slot(sam, 2)?.as_atom()?;
    let ruz = slot(sam, 3)?.as_atom()?;

    unsafe {
        let (mut out_ida, out) = IndirectAtom::new_raw_mut_bytes(stack, 32);

        let sal_bytes = &(sal.as_ne_bytes())[0..met(3, sal)]; // drop trailing zeros
        let (mut _salt_ida, salt) = IndirectAtom::new_raw_mut_bytes(stack, sal_bytes.len());
        salt.copy_from_slice(sal_bytes);

        let msg_len = met(3, ruz);
        if msg_len > 0 {
            let msg_bytes = &(ruz.as_ne_bytes())[0..msg_len];
            let (_msg_ida, msg) = IndirectAtom::new_raw_mut_bytes(stack, msg_bytes.len());
            msg.copy_from_slice(msg_bytes);
            ac_shas(msg, salt, out);
        } else {
            ac_shas(&mut [], salt, out);
        }

        Ok(out_ida.normalize_as_atom().as_noun())
    }
}

pub fn jet_shax(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let sam = slot(subject, 6)?;
    let ruz = sam.as_atom()?;
    let msg_len = met(3, ruz);

    unsafe {
        let (mut ida, out) = IndirectAtom::new_raw_mut_bytes(stack, 32);

        if msg_len > 0 {
            let msg_bytes = &(ruz.as_ne_bytes())[0..msg_len];
            let (_msg_ida, msg) = IndirectAtom::new_raw_mut_bytes(stack, msg_bytes.len());
            msg.copy_from_slice(msg_bytes);
            ac_shay(msg, out);
        } else {
            ac_shay(&mut [], out);
        }

        Ok(ida.normalize_as_atom().as_noun())
    }
}

pub fn jet_shay(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let sam = slot(subject, 6)?;
    let len = slot(sam, 2)?.as_atom()?;
    let ruz = slot(sam, 3)?.as_atom()?;

    let length = match len.as_direct() {
        Ok(direct) => direct.data() as usize,
        Err(_) => return Err(BAIL_FAIL),
    };
    let msg_len = met(3, ruz);

    unsafe {
        let (mut out_ida, out) = IndirectAtom::new_raw_mut_bytes(stack, 32);
        if length == 0 {
            ac_shay(&mut [], out);
        } else if msg_len >= length {
            let (mut _msg_ida, msg) = IndirectAtom::new_raw_mut_bytes(stack, length);
            msg.copy_from_slice(&(ruz.as_ne_bytes())[0..length]);
            ac_shay(msg, out);
        } else {
            let msg_bytes = &(ruz.as_ne_bytes())[0..msg_len];
            let (mut _msg_ida, msg) = IndirectAtom::new_raw_mut_bytes(stack, length);
            msg[0..msg_len].copy_from_slice(msg_bytes);
            ac_shay(msg, out);
        }

        Ok(out_ida.normalize_as_atom().as_noun())
    }
}

pub fn jet_shal(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let sam = slot(subject, 6)?;
    let len = slot(sam, 2)?.as_atom()?;
    let ruz = slot(sam, 3)?.as_atom()?;

    let length = match len.as_direct() {
        Ok(direct) => direct.data() as usize,
        Err(_) => return Err(BAIL_FAIL),
    };
    let msg_len = met(3, ruz);

    unsafe {
        let (mut out_ida, out) = IndirectAtom::new_raw_mut_bytes(stack, 64);
        if length == 0 {
            ac_shal(&mut [], out);
        } else if msg_len >= length {
            let (mut _msg_ida, msg) = IndirectAtom::new_raw_mut_bytes(stack, length);
            msg.copy_from_slice(&(ruz.as_ne_bytes())[0..length]);
            ac_shal(msg, out);
        } else {
            let msg_bytes = &(ruz.as_ne_bytes())[0..msg_len];
            let (mut _msg_ida, msg) = IndirectAtom::new_raw_mut_bytes(stack, length);
            msg[0..msg_len].copy_from_slice(msg_bytes);
            ac_shal(msg, out);
        }

        Ok(out_ida.normalize_as_atom().as_noun())
    }
}

pub fn jet_sha1(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let sam = slot(subject, 6)?;
    let len = slot(sam, 2)?.as_atom()?;
    let ruz = slot(sam, 3)?.as_atom()?;

    let length = match len.as_direct() {
        Ok(direct) => direct.data() as usize,
        Err(_) => return Err(BAIL_FAIL),
    };
    let msg_len = met(3, ruz);

    unsafe {
        let (mut out_ida, out) = IndirectAtom::new_raw_mut_bytes(stack, 20);
        if length == 0 {
            ac_sha1(&mut [], out);
        } else if msg_len >= length {
            let (mut _msg_ida, msg) = IndirectAtom::new_raw_mut_bytes(stack, length);
            msg.copy_from_slice(&(ruz.as_ne_bytes())[0..length]);
            ac_sha1(msg, out);
        } else {
            let msg_bytes = &(ruz.as_ne_bytes())[0..msg_len];
            let (mut _msg_ida, msg) = IndirectAtom::new_raw_mut_bytes(stack, length);
            msg[0..msg_len].copy_from_slice(msg_bytes);
            ac_sha1(msg, out);
        }

        Ok(out_ida.normalize_as_atom().as_noun())
    }
}

#[cfg(test)]
mod tests {
    use ibig::ubig;

    use super::*;
    use crate::jets::util::test::{assert_jet, assert_jet_err, assert_jet_ubig, init_context, A};
    use crate::noun::{D, DIRECT_MAX, T};

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_shas() {
        let c = &mut init_context();

        let sam = T(&mut c.stack, &[D(1), D(0)]);
        assert_jet_ubig(
            c,
            jet_shas,
            sam,
            ubig!(_0x4abac214e1e95fe0c60df79d09cbd05454a4cb958683e02318aa147f2a5e6d60),
        );

        let sam = T(&mut c.stack, &[D(1), D(1)]);
        assert_jet_ubig(
            c,
            jet_shas,
            sam,
            ubig!(_0x547da92584bc986e5784edb746c29504bfd6b34572c83b7b96440ca77d35cdfc),
        );

        let sam = T(&mut c.stack, &[D(2), D(2)]);
        assert_jet_ubig(
            c,
            jet_shas,
            sam,
            ubig!(_0x4cf01fe7cc56ef70d17735322488de0d31857afcfe451e199abe6295f78f5328),
        );

        let a = A(
            &mut c.stack,
            &ubig!(_0xa1d6eb6ef33f233ae6980ca7c4fc65f90fe1bdee11c730d41607b4747c83de72),
        );
        let b = A(
            &mut c.stack,
            &ubig!(_0xa1d6eb6ef33f233ae6980ca7c4fc65f90fe1bdee11c730d41607b4747c83de73),
        );
        let sam = T(&mut c.stack, &[a, b]);
        assert_jet_ubig(
            c,
            jet_shas,
            sam,
            ubig!(_0xf7569a89650553ef13f9a8f0bb751fd42b70a4821be6bc1cbe197af33ce4843c),
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_shax() {
        let c = &mut init_context();

        assert_jet_ubig(
            c,
            jet_shax,
            D(0), // ''
            ubig!(_0x55b852781b9995a44c939b64e441ae2724b96f99c8f4fb9a141cfc9842c4b0e3),
        );

        assert_jet_ubig(
            c,
            jet_shax,
            D(7303014), // 'foo'
            ubig!(_0xaee76662885e8af9a0bf8364702d42133441301d3c459bf98fc6ff686bb4262c),
        );

        let a = A(
            &mut c.stack,
            &ubig!(_0xaee76662885e8af9a0bf8364702d42133441301d3c459bf98fc6ff686bb4262c),
        );
        assert_jet_ubig(
            c,
            jet_shax,
            a,
            ubig!(_0x9ee26e46c2028aa4a9c463aa722b82ed8bf6e185c3e5a5a69814a2c78fe8adc7),
        );

        assert_jet_ubig(
            c,
            jet_shax,
            D(123456789),
            ubig!(_0xa1d6eb6ef33f233ae6980ca7c4fc65f90fe1bdee11c730d41607b4747c83de72),
        );

        let a = A(
            &mut c.stack,
            &ubig!(_0xa1d6eb6ef33f233ae6980ca7c4fc65f90fe1bdee11c730d41607b4747c83de72),
        );
        assert_jet_ubig(
            c,
            jet_shax,
            a,
            ubig!(_0xf90f3184d7347a20cfdd2d5f7ac5c82eb9ab7af54c9419fbc18832c5a33360c9),
        )
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_shay() {
        let c = &mut init_context();

        let sam = T(&mut c.stack, &[D(0), D(0)]);
        let ret = A(
            &mut c.stack,
            &ubig!(_0x55b852781b9995a44c939b64e441ae2724b96f99c8f4fb9a141cfc9842c4b0e3),
        );
        assert_jet(c, jet_shay, sam, ret);

        let sam = T(&mut c.stack, &[D(1), D(0)]);
        let ret = A(
            &mut c.stack,
            &ubig!(_0x1da0af1706a31185763837b33f1d90782c0a78bbe644a59c987ab3ff9c0b346e),
        );
        assert_jet(c, jet_shay, sam, ret);

        let sam = T(&mut c.stack, &[D(0), D(1)]);
        let ret = A(
            &mut c.stack,
            &ubig!(_0x55b852781b9995a44c939b64e441ae2724b96f99c8f4fb9a141cfc9842c4b0e3),
        );
        assert_jet(c, jet_shay, sam, ret);

        let sam = T(&mut c.stack, &[D(1), D(478560413032)]); // [1 'hello']
        let ret = A(
            &mut c.stack,
            &ubig!(_0x23b14de6713b28aadf8f95026636eb6ab63e99c952bceb401fa4f1642640a9aa),
        );
        assert_jet(c, jet_shay, sam, ret);

        let sam = T(&mut c.stack, &[D(2), D(478560413032)]); // [2 'hello']
        let ret = A(
            &mut c.stack,
            &ubig!(_0xde1e7ee30cecf453f1d77c08a125fdc6a4bbac72c01dd7a1e21cd0d22f7e2f37),
        );
        assert_jet(c, jet_shay, sam, ret);

        let big = DIRECT_MAX + 1;
        let ida = unsafe {
            IndirectAtom::new_raw_bytes(&mut c.stack, 8, &big as *const u64 as *const u8)
        };
        let sam = T(&mut c.stack, &[ida.as_noun(), D(478560413032)]);
        assert_jet_err(c, jet_shay, sam, BAIL_FAIL);

        let big: u128 = (DIRECT_MAX as u128) << 64;
        let ida = unsafe {
            IndirectAtom::new_raw_bytes(&mut c.stack, 8, &big as *const u128 as *const u8)
        };
        let sam = T(&mut c.stack, &[ida.as_noun(), D(478560413032)]);
        assert_jet_err(c, jet_shay, sam, BAIL_FAIL);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_shal() {
        let c = &mut init_context();

        let sam = T(&mut c.stack, &[D(0), D(0)]);
        assert_jet_ubig(
            c,
            jet_shal,
            sam,
            ubig!(_0x3eda27f97a3238a5817a4147bd31b9632fec7e87d21883ffb0f2855d3cd1d047cee96cd321a9f483dc15570b05e420d607806dd6502854f1bdb8ef7e35e183cf)
        );

        let sam = T(&mut c.stack, &[D(1), D(0)]);
        assert_jet_ubig(
            c,
            jet_shal,
            sam,
            ubig!(_0xee1069e3f03884c3e5d457253423844a323c29eb4cde70630b58c3712a804a70221d35d9506e242c9414ff192e283dd6caa4eff86a457baf93d68189024d24b8)
        );

        let sam = T(&mut c.stack, &[D(0), D(1)]);
        assert_jet_ubig(
            c,
            jet_shal,
            sam,
            ubig!(_0x3eda27f97a3238a5817a4147bd31b9632fec7e87d21883ffb0f2855d3cd1d047cee96cd321a9f483dc15570b05e420d607806dd6502854f1bdb8ef7e35e183cf)
        );

        let wid = A(
            &mut c.stack,
            &ubig!(_0xa1d6eb6ef33f233ae6980ca7c4fc65f90fe1bdee11c730d41607b4747c83de72),
        );
        let dat = A(
            &mut c.stack,
            &ubig!(_0xa1d6eb6ef33f233ae6980ca7c4fc65f90fe1bdee11c730d41607b4747c83de73),
        );
        let sam = T(&mut c.stack, &[wid, dat]);
        assert_jet_err(c, jet_shal, sam, BAIL_FAIL);

        let wid = A(
            &mut c.stack,
            &ubig!(_0xa1d6eb6ef33f233ae6980ca7c4fc65f90fe1bdee11c730d41607b4747c83de72),
        );
        let sam = T(&mut c.stack, &[wid, D(1)]);
        assert_jet_err(c, jet_shal, sam, BAIL_FAIL);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_sha1() {
        let c = &mut init_context();

        let sam = T(&mut c.stack, &[D(0), D(0)]);
        assert_jet_ubig(
            c,
            jet_sha1,
            sam,
            ubig!(_0xda39a3ee5e6b4b0d3255bfef95601890afd80709),
        );

        let sam = T(&mut c.stack, &[D(1), D(0)]);
        assert_jet_ubig(
            c,
            jet_sha1,
            sam,
            ubig!(_0x5ba93c9db0cff93f52b521d7420e43f6eda2784f),
        );

        let sam = T(&mut c.stack, &[D(0), D(1)]);
        assert_jet_ubig(
            c,
            jet_sha1,
            sam,
            ubig!(_0xda39a3ee5e6b4b0d3255bfef95601890afd80709),
        );

        let wid = A(
            &mut c.stack,
            &ubig!(_0xa1d6eb6ef33f233ae6980ca7c4fc65f90fe1bdee11c730d41607b4747c83de72),
        );
        let dat = A(
            &mut c.stack,
            &ubig!(_0xa1d6eb6ef33f233ae6980ca7c4fc65f90fe1bdee11c730d41607b4747c83de73),
        );
        let sam = T(&mut c.stack, &[wid, dat]);
        assert_jet_err(c, jet_sha1, sam, BAIL_FAIL);

        let wid = A(
            &mut c.stack,
            &ubig!(_0xa1d6eb6ef33f233ae6980ca7c4fc65f90fe1bdee11c730d41607b4747c83de72),
        );
        let sam = T(&mut c.stack, &[wid, D(1)]);
        assert_jet_err(c, jet_sha1, sam, BAIL_FAIL);
    }
}
