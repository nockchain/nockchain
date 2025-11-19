use bytes::Bytes;
use nockchain_math::crypto::argon2::Byts;
use nockvm::ext::AtomExt;
use nockvm::interpreter::Context;
use nockvm::jets::cold::Nounable;
use nockvm::jets::util::{slot, BAIL_EXIT};
use nockvm::jets::JetErr;
use nockvm::noun::{Atom, Noun};

use crate::form::crypto::argon2::{argon2_hook, Argon2Args};

pub fn argon2_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let parent_core = slot(subject, 7)?;
    let params = slot(parent_core, 6)?;

    // prepare parameters
    let args = Argon2Args::from_noun(&mut context.stack, &params)?;

    let sam = slot(subject, 6)?;
    let msg: Byts = Byts::from_noun(&mut context.stack, &slot(sam, 2)?)?;
    let sat: Byts = Byts::from_noun(&mut context.stack, &slot(sam, 3)?)?;

    let mut res = vec![0; args.out];
    argon2_hook(args, &msg.0, &sat.0, &mut res).map_err(|_| BAIL_EXIT)?;

    // create Bytes type from res
    let res_bytes = Bytes::from(res);
    let res_atom = Atom::from_bytes(&mut context.stack, &res_bytes);
    let res_noun = res_atom.as_noun();
    Ok(res_noun)
}

#[cfg(test)]
pub mod test {
    use hex_literal::hex;
    use ibig::UBig;
    use nockvm::ext::make_tas;
    use nockvm::jets::util::test::{assert_jet_door, init_context};
    use nockvm::noun::{D, T};

    use super::*;

    /// Test the argon2d jet with test vector from RFC 9106
    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_argon2d_v0x13_jet() {
        let mut context = init_context();
        let out = D(32);
        let algorithm = make_tas(&mut context.stack, "d").as_noun();
        let version = D(0x13);
        let threads = D(4);
        let mem_cost = D(32);
        let time_cost = D(3);
        let secret_byts = Byts([0x03; 8].to_vec());
        let secret = secret_byts.into_noun(&mut context.stack);
        let extra_byts = Byts([0x04; 12].to_vec());
        let extra = extra_byts.into_noun(&mut context.stack);

        let params = T(
            &mut context.stack,
            &[out, algorithm, version, threads, mem_cost, time_cost, secret, extra],
        );
        let password_byts = Byts([0x01; 32].to_vec());
        let password = password_byts.into_noun(&mut context.stack);
        let salt_byts = Byts([0x02; 16].to_vec()).into_noun(&mut context.stack);
        let salt = salt_byts.into_noun(&mut context.stack);

        let inner = T(&mut context.stack, &[password, salt]);

        let expected_tag = hex!(
            "
            51 2b 39 1b 6f 11 62 97
            53 71 d3 09 19 73 42 94
            f8 68 e3 be 39 84 f3 c1
            a1 3a 4d b9 fa be 4a cb
            "
        );

        let expected_bytes =
            Atom::from_ubig(&mut context.stack, &UBig::from_le_bytes(&expected_tag)).as_noun();

        let parent_context = T(&mut context.stack, &[D(0), params, D(0)]);
        assert_jet_door(
            &mut context, argon2_jet, inner, parent_context, expected_bytes,
        );
    }

    /// Test if argon2d jet matches reference with input where endian-ness matters
    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_argon2d_v0x13_endian_jet() {
        let mut context = init_context();
        let out = D(32);
        let algorithm = make_tas(&mut context.stack, "d").as_noun();
        let version = D(0x13);
        let threads = D(4);
        let mem_cost = D(32);
        let time_cost = D(3);
        let secret_byts = Byts([0x03; 8].to_vec());
        let secret = secret_byts.into_noun(&mut context.stack);
        let extra_byts = Byts([0x04; 12].to_vec());
        let extra = extra_byts.into_noun(&mut context.stack);

        let params = T(
            &mut context.stack,
            &[out, algorithm, version, threads, mem_cost, time_cost, secret, extra],
        );

        let mut password_byts = Byts([0x01; 32].to_vec());
        password_byts.0[0] = 0xaa;
        let password = password_byts.clone().into_noun(&mut context.stack);
        let salt_byts = Byts([0x02; 16].to_vec());
        let salt = salt_byts.clone().into_noun(&mut context.stack);
        let inner = T(&mut context.stack, &[password, salt]);

        let args = Argon2Args::from_noun(&mut context.stack, &params).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        let expected_tag = &mut vec![0u8; args.out];

        argon2_hook(args, &password_byts.0, &salt_byts.0, expected_tag).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });

        let expected_bytes =
            Atom::from_ubig(&mut context.stack, &UBig::from_le_bytes(expected_tag)).as_noun();

        let parent_context = T(&mut context.stack, &[D(0), params, D(0)]);
        assert_jet_door(
            &mut context, argon2_jet, inner, parent_context, expected_bytes,
        );
    }
}
