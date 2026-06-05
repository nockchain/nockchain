use nockvm_crypto::ed25519::{ac_ed_puck, ac_ed_shar, ac_ed_sign, ac_ed_veri};

use crate::interpreter::Context;
use crate::jets::bits::util::met;
use crate::jets::util::{slot, BAIL_EXIT};
use crate::jets::{JetErr, Result};
use crate::mem::NockStack;
use crate::noun::{IndirectAtom, Noun, NO, YES};

crate::gdb!();

pub fn jet_puck(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let sed = slot(subject, 6)?.as_atom()?;

    let sed_len = met(3, sed);
    if sed_len > 32 {
        return Err(BAIL_EXIT);
    }

    unsafe {
        let sed_bytes = &mut [0u8; 32];
        sed_bytes[0..sed_len].copy_from_slice(&(sed.as_ne_bytes())[0..sed_len]);

        let (mut pub_ida, pub_key) = IndirectAtom::new_raw_mut_bytearray::<32, NockStack>(stack);
        ac_ed_puck(sed_bytes, pub_key);

        Ok(pub_ida.normalize_as_atom().as_noun())
    }
}

pub fn jet_shar(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let pub_key = slot(subject, 12)?.as_atom()?;
    let sec_key = slot(subject, 13)?.as_atom()?;

    if met(3, sec_key) > 32 {
        // sek is size checked by +puck via +suck
        return Err(BAIL_EXIT);
    }
    if met(3, pub_key) > 32 {
        // pub is not size checked in Hoon, but it must be 32 bytes or less for
        // ucrypt. Therefore, punt on larger values.
        return Err(JetErr::Punt);
    }

    unsafe {
        let public = &mut [0u8; 32];
        let secret = &mut [0u8; 32];

        let pub_bytes = pub_key.as_ne_bytes();
        let sec_bytes = sec_key.as_ne_bytes();

        public[0..pub_bytes.len()].copy_from_slice(pub_bytes);
        secret[0..sec_bytes.len()].copy_from_slice(sec_bytes);

        let (mut shar_ida, shar) = IndirectAtom::new_raw_mut_bytearray::<32, NockStack>(stack);
        ac_ed_shar(public, secret, shar);

        Ok(shar_ida.normalize_as_atom().as_noun())
    }
}

pub fn jet_sign(context: &mut Context, subject: Noun) -> Result {
    let stack = &mut context.stack;
    let msg = slot(subject, 12)?.as_atom()?;
    let sed = slot(subject, 13)?.as_atom()?;

    unsafe {
        let sed_bytes = sed.as_ne_bytes();
        let sed_len = sed_bytes.len();
        if sed_len > 32 {
            return Err(BAIL_EXIT);
        };
        let seed = &mut [0u8; 32];
        seed[0..sed_len].copy_from_slice(sed_bytes);

        let (mut sig_ida, sig) = IndirectAtom::new_raw_mut_bytearray::<64, NockStack>(stack);

        let msg_len = met(3, msg);
        if msg_len > 0 {
            let (_msg_ida, message) = IndirectAtom::new_raw_mut_bytes(stack, msg_len);
            message.copy_from_slice(&msg.as_ne_bytes()[0..msg_len]);
            ac_ed_sign(message, seed, sig);
        } else {
            ac_ed_sign(&[0u8; 0], seed, sig);
        }

        sig.reverse();
        Ok(sig_ida.normalize_as_atom().as_noun())
    }
}

pub fn jet_veri(_context: &mut Context, subject: Noun) -> Result {
    let sig = slot(subject, 12)?.as_atom()?;
    let msg = slot(subject, 26)?.as_atom()?;
    let puk = slot(subject, 27)?.as_atom()?;

    // Both are size checked by Hoon, but without crashing
    let sig_bytes = sig.as_ne_bytes();
    if sig_bytes.len() > 64 {
        return Ok(NO);
    };
    let signature = &mut [0u8; 64];
    signature[0..sig_bytes.len()].copy_from_slice(sig_bytes);

    let pub_bytes = puk.as_ne_bytes();
    if pub_bytes.len() > 32 {
        return Ok(NO);
    };
    let public_key = &mut [0u8; 32];
    public_key[0..pub_bytes.len()].copy_from_slice(pub_bytes);

    let message = &(msg.as_ne_bytes())[0..met(3, msg)]; // drop trailing zeros

    let valid = ac_ed_veri(message, public_key, signature);

    Ok(if valid { YES } else { NO })
}

#[cfg(test)]
mod tests {
    use ibig::ubig;

    use super::*;
    use crate::jets::util::test::{assert_jet, assert_jet_err, init_context, A};
    use crate::noun::{D, T};

    //  XX: Should use the test vectors from Section 7.1 of RFC 8032:
    //      https://tools.ietf.org/html/rfc8032#section-7.1

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_puck() {
        let c = &mut init_context();

        let sam = A(&mut c.stack, &ubig!(_0x0));
        let ret = A(
            &mut c.stack,
            &ubig!(_0x29da598ba148c03aa643e21d77153265730d6f2ad0a8a3622da4b6cebc276a3b),
        );
        assert_jet(c, jet_puck, sam, ret);

        let sam = A(
            &mut c.stack,
            &ubig!(_0x607fae1c03ac3b701969327b69c54944c42cec92f44a84ba605afdef9db1619d),
        );
        let ret = A(
            &mut c.stack,
            &ubig!(_0x1a5107f7681a02af2523a6daf372e10e3a0764c9d3fe4bd5b70ab18201985ad7),
        );
        assert_jet(c, jet_puck, sam, ret);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_shar() {
        let c = &mut init_context();

        let sam = T(&mut c.stack, &[D(0), D(0)]);
        let ret = A(&mut c.stack, &ubig!(_0x0));
        assert_jet(c, jet_shar, sam, ret);

        let sam = T(&mut c.stack, &[D(234), D(234)]);
        let ret = A(
            &mut c.stack,
            &ubig!(_0x6ecd5779a47841207a2cd0c9d085796aa646842885a332adac540027d768c1c5),
        );
        assert_jet(c, jet_shar, sam, ret);

        let sam = A(
            &mut c.stack,
            &ubig!(_0xfb099b0acc4d1ce37f9982a2ed331245e0cdfdf6979364b7676a142b8233e53b),
        );
        assert_jet_err(c, jet_shar, sam, BAIL_EXIT);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_sign() {
        let c = &mut init_context();

        unsafe {
            let sam = T(&mut c.stack, &[D(0), D(0)]);
            let ret = A(&mut c.stack, &ubig!(_0x8f895b3cafe2c9506039d0e2a66382568004674fe8d237785092e40d6aaf483e4fc60168705f31f101596138ce21aa357c0d32a064f423dc3ee4aa3abf53f803));
            assert_jet(c, jet_sign, sam, ret);

            let message = D(0x72);

            let sed_ubig =
                ubig!(_0x4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb);
            let sed_bytes = sed_ubig.to_be_bytes();
            let seed =
                IndirectAtom::new_raw_bytes(&mut c.stack, sed_bytes.len(), sed_bytes.as_ptr())
                    .as_noun();

            let sam = T(&mut c.stack, &[message, seed]);
            let ret = A(&mut c.stack, &ubig!(_0x92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00));
            assert_jet(c, jet_sign, sam, ret);

            let msg_ubig = ubig!(_0xaf82);
            let msg_bytes = msg_ubig.to_be_bytes();
            let message =
                IndirectAtom::new_raw_bytes(&mut c.stack, msg_bytes.len(), msg_bytes.as_ptr())
                    .as_noun();

            let sed_ubig =
                ubig!(_0xc5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7);
            let sed_bytes = sed_ubig.to_be_bytes();
            let seed =
                IndirectAtom::new_raw_bytes(&mut c.stack, sed_bytes.len(), sed_bytes.as_ptr())
                    .as_noun();

            let sam = T(&mut c.stack, &[message, seed]);
            let ret = A(&mut c.stack, &ubig!(_0x6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a));
            assert_jet(c, jet_sign, sam, ret);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_veri() {
        let c = &mut init_context();

        unsafe {
            let sam = T(&mut c.stack, &[D(0), D(0), D(0)]);
            assert_jet(c, jet_veri, sam, NO);

            let sig_ubig = ubig!(_0x92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00);
            let sig_bytes = sig_ubig.to_be_bytes();
            let signature =
                IndirectAtom::new_raw_bytes(&mut c.stack, sig_bytes.len(), sig_bytes.as_ptr())
                    .as_noun();

            let message = D(0x72);

            let pub_ubig =
                ubig!(_0x3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c);
            let pub_bytes = pub_ubig.to_be_bytes();
            let public_key =
                IndirectAtom::new_raw_bytes(&mut c.stack, pub_bytes.len(), pub_bytes.as_ptr())
                    .as_noun();

            let sam = T(&mut c.stack, &[signature, message, public_key]);
            assert_jet(c, jet_veri, sam, YES);

            let sig_ubig = ubig!(_0x6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a);
            let sig_bytes = sig_ubig.to_be_bytes();
            let signature =
                IndirectAtom::new_raw_bytes(&mut c.stack, sig_bytes.len(), sig_bytes.as_ptr())
                    .as_noun();

            let msg_ubig = ubig!(0xaf82);
            let msg_bytes = msg_ubig.to_be_bytes();
            let message =
                IndirectAtom::new_raw_bytes(&mut c.stack, msg_bytes.len(), msg_bytes.as_ptr())
                    .as_noun();

            let pub_ubig =
                ubig!(_0xfc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025);
            let pub_bytes = pub_ubig.to_be_bytes();
            let public_key =
                IndirectAtom::new_raw_bytes(&mut c.stack, pub_bytes.len(), pub_bytes.as_ptr())
                    .as_noun();

            let sam = T(&mut c.stack, &[signature, message, public_key]);
            assert_jet(c, jet_veri, sam, YES);
        }
    }
}
