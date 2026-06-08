use ibig::UBig;
use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::mem::NockStack;
use nockvm::noun::Noun;
use noun_serde::{NounDecode, NounEncode};

use crate::form::belt::*;
use crate::form::crypto::cheetah::*;
use crate::form::tip5;

#[inline(always)]
pub fn ch_scal_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let n_atom = slot(sam, 2, &space)?.in_space(&space).as_atom()?;

    let p = slot(sam, 3, &space)?;
    let a_pt = CheetahPoint::from_noun(&p, &space).map_err(|_| BAIL_FAIL)?;

    let res = if let Ok(n) = n_atom.as_u64() {
        ch_scal(n, &a_pt)?
    } else {
        // Convert to UBig
        let n_big = n_atom.as_ubig(&mut context.stack);
        ch_scal_big(&n_big, &a_pt)?
    };

    let res_noun = res.to_noun(&mut context.stack);
    Ok(res_noun)
}

pub fn verify_affine_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let pubkey = slot(sam, 2, &space)?;
    let m = slot(sam, 6, &space)?;
    let chal = slot(sam, 14, &space)?
        .in_space(&space)
        .as_atom()?
        .as_ubig(&mut context.stack);
    let sig = slot(sam, 15, &space)?
        .in_space(&space)
        .as_atom()?
        .as_ubig(&mut context.stack);

    let pubkey: CheetahPoint = CheetahPoint::from_noun(&pubkey, &space).map_err(|_| BAIL_FAIL)?;
    let m = <[Belt; 5]>::from_noun(&m, &space).map_err(|_| BAIL_FAIL)?;

    let res = verify_affine(&pubkey, &m, &chal, &sig)?;
    Ok(res.to_noun(&mut context.stack))
}

pub(crate) struct ValidateArgs {
    pub pubkey: CheetahPoint,
    pub m: [Belt; 5],
    pub chal: UBig,
    pub sig: UBig,
}

//  TODO: Implement NounDecode for UBig, requires NounAllocator in NounDecode from_noun
//impl NounDecode for ValidateArgs {
//    fn from_noun<A: NounAllocator>(stack: &mut A, noun: &Noun) -> Result<Self, NounDecodeError> {
//        let pubkey = CheetahPoint::from_noun(&noun.slot(2)?)?;
//        let m = Vec::<Belt>::from_noun(&noun.slot(6)?)?;
//        let chal = noun.slot(14)?.as_atom()?.as_ubig(stack);
//        let sig = noun.slot(15)?.as_atom()?.as_ubig(stack);
//
//        Ok(ValidateArgs {
//            pubkey,
//            m,
//            chal,
//            sig,
//        })
//    }
//}

pub fn batch_verify_affine_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let list = slot(subject, 6, &space)?;
    //  `batch-verify:affine:schnorr` = `(levy batch verify)`. Here `chal`/`sig`
    //  are raw `@ux` scalars (the schnorr arm, not the belt-schnorr t8 wrapper).
    //  Any element this jet cannot decode as the expected types Punts, so the
    //  runtime re-runs the authoritative Hoon rather than diverging on a
    //  malformed input the Hoon would still process.
    let args = list
        .in_space(&space)
        .list_iter()
        .map(|arg| {
            let pubkey =
                CheetahPoint::from_noun(&arg.slot(2)?.noun(), &space).map_err(|_| JetErr::Punt)?;
            let m =
                <[Belt; 5]>::from_noun(&arg.slot(6)?.noun(), &space).map_err(|_| JetErr::Punt)?;
            let chal = arg
                .slot(14)?
                .as_atom()
                .map_err(|_| JetErr::Punt)?
                .as_ubig(&mut context.stack);
            let sig = arg
                .slot(15)?
                .as_atom()
                .map_err(|_| JetErr::Punt)?
                .as_ubig(&mut context.stack);
            Ok(ValidateArgs {
                pubkey,
                m,
                chal,
                sig,
            })
        })
        .collect::<Result<Vec<ValidateArgs>, JetErr>>()?;

    levy_verify_affine(&args, &mut context.stack)
}

/// Resolve a batch of decoded signature args exactly as Hoon `(levy batch verify)`:
/// verify each element in order and short-circuit at the first `%.n`. An empty
/// batch is `%.y`. A verification that errors (a non-curve point driving
/// `f6-div` by zero, where the Hoon `verify` crashes) propagates as a
/// deterministic `JetErr` via `?` — reached only when every earlier element
/// verified, exactly as `levy` reaches that element before any `%.n`.
fn levy_verify_affine(args: &[ValidateArgs], stack: &mut NockStack) -> Result<Noun, JetErr> {
    for arg in args {
        if !verify_affine(&arg.pubkey, &arg.m, &arg.chal, &arg.sig)? {
            return Ok(false.to_noun(stack));
        }
    }
    Ok(true.to_noun(stack))
}

#[inline(always)]
pub fn verify_affine(
    pubkey: &CheetahPoint,
    m: &[Belt],
    chal: &UBig,
    sig: &UBig,
) -> Result<bool, JetErr> {
    //  Match the Hoon `+verify:affine:schnorr` scalar-range guards
    //  (open/hoon/common/ztd/three.hoon): both the challenge and response must
    //  satisfy `0 < scalar < g-order`. `Ok(false)` mirrors the Hoon `?&`
    //  short-circuit to `%.n` before any scalar multiplication.
    let zero = UBig::from(0u32);
    if chal == &zero || sig == &zero || chal >= &*G_ORDER || sig >= &*G_ORDER {
        return Ok(false);
    }
    let left = ch_scal_big(sig, &A_GEN)?;
    let right = ch_neg(&ch_scal_big(chal, pubkey)?);
    let sum = ch_add(&left, &right)?;
    //  NB: the Hoon `+verify:affine:schnorr` has `?< =(scalar f6-zero)`, which
    //  compares the whole `a-pt` to an `f6lt` and is therefore always false — a
    //  no-op. Deployed Hoon proceeds to hash `sum` even when `sum.x` is zero
    //  (e.g. the identity, where `sum.y == F6_ONE`), so to stay bit-identical we
    //  proceed here too rather than returning early. (If the Hoon is ever changed
    //  to actually reject an x-zero sum, this jet must change in lockstep.)
    let mut hashable = vec![Belt(0); 6 * 4 + 5];
    hashable[0..6].copy_from_slice(&sum.x.0);
    hashable[6..12].copy_from_slice(&sum.y.0);
    hashable[12..18].copy_from_slice(&pubkey.x.0);
    hashable[18..24].copy_from_slice(&pubkey.y.0);
    hashable[24..].copy_from_slice(m);

    let hash = tip5::hash::hash_varlen(&mut hashable);
    let truncated_hash = trunc_g_order(&hash);

    Ok(truncated_hash == *chal)
}

/// Jet for `batch-verify:affine:belt-schnorr:cheetah`. The Hoon is
/// `(levy batch verify)` where `verify` converts each `t8` challenge/response to
/// an atom via `t8-to-atom` (`rap 5`) and calls `verify:affine:schnorr`. This is
/// the v1 transaction signature-verification path.
pub fn belt_schnorr_batch_verify_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let list = slot(subject, 6, &space)?;
    //  Decode each `[pk m chal=t8 sig=t8]`. A decode failure means an input this
    //  jet does not model (e.g. a non-field limb `>= prime`, which the Hoon arm
    //  still processes via `rap 5`); Punt so the runtime re-runs the authoritative
    //  Hoon instead of diverging.
    let args = list
        .in_space(&space)
        .list_iter()
        .map(|arg| {
            let pubkey =
                CheetahPoint::from_noun(&arg.slot(2)?.noun(), &space).map_err(|_| JetErr::Punt)?;
            let m =
                <[Belt; 5]>::from_noun(&arg.slot(6)?.noun(), &space).map_err(|_| JetErr::Punt)?;
            let chal_t8 =
                <[Belt; 8]>::from_noun(&arg.slot(14)?.noun(), &space).map_err(|_| JetErr::Punt)?;
            let sig_t8 =
                <[Belt; 8]>::from_noun(&arg.slot(15)?.noun(), &space).map_err(|_| JetErr::Punt)?;
            Ok(ValidateArgs {
                pubkey,
                m,
                chal: belt_schnorr_t8_to_ubig(&chal_t8),
                sig: belt_schnorr_t8_to_ubig(&sig_t8),
            })
        })
        .collect::<Result<Vec<ValidateArgs>, JetErr>>()?;

    levy_verify_affine(&args, &mut context.stack)
}

#[cfg(test)]
mod tests {
    use ibig::UBig;
    use nockvm::jets::util::test::{assert_jet, init_context, A};
    use nockvm::noun::{Atom, D, NO, T, YES};
    use noun_serde::NounEncode;

    use super::*;

    const F6_TEST: F6lt = F6lt([
        Belt(13724052584687643294),
        Belt(6944593306454870014),
        Belt(10082672435494154603),
        Belt(6450272673873704561),
        Belt(2898784811200916299),
        Belt(15463938240345685194),
    ]);

    #[test]
    fn test_b58_roundtrip() {
        for x in ["32KVTmv3ofSyACq9nC1Hgnk4Jt8rs2hj1cvDZWC1EQuiYFMDg8MaLtF3ntafJbEUH5XPV1pK3K4xkxfjRPAWprBb7LYCVv4HF7817Bwh9M9xAdmgrPt77j4xejihNFd9h5Eo",
            "2Xu6FtvopCS69Ko2YnC99B9SVVZ7PLoVn7WvEdDpJKRxW1pmj51uBQdYfADEbRUFYwG55Wi2Qwa3f6Y6WTev5jLcvfJFDEr2Wwt8rViQeLsz1XwEPah5pxtwHTm2nmecjJNW"] {
                let point = CheetahPoint::from_base58(x).unwrap();
                let x_round = point.into_base58().unwrap();
                assert_eq!(x, x_round)
            }
    }

    #[test]
    fn test_cheetah_point_from_b58() {
        let expected_point = A_GEN;
        // Create a known CheetahPoint with specific x and y coordinates
        // Encode the bytes to base58
        let b58_str = expected_point.into_base58().unwrap();

        // Now test decoding
        let decoded_point =
            CheetahPoint::from_base58(&b58_str).expect("Failed to decode valid base58 string");

        // Check if the decoded point matches our expected point
        assert_eq!(decoded_point.x.0, expected_point.x.0);
        assert_eq!(decoded_point.y.0, expected_point.y.0);
        assert_eq!(decoded_point.inf, expected_point.inf);

        // Test error cases

        // 1. Invalid base58 string
        let result = CheetahPoint::from_base58("invalid!base58");
        assert!(result.is_err());

        // 2. Too short base58 string (not enough bytes for 12 Belts)
        let short_bytes = [1u8, 2, 3, 4];
        let short_b58 = bs58::encode(&short_bytes).into_string();
        let result = CheetahPoint::from_base58(&short_b58);
        assert!(result.is_err());

        // 3. Valid base58 but not length 96
        let odd_bytes = vec![1u8; 95]; // Not divisible by 8
        let odd_b58 = bs58::encode(&odd_bytes).into_string();
        let result = CheetahPoint::from_base58(&odd_b58);
        assert!(result.is_err());
    }

    #[test]
    fn test_f6mul() {
        let f0 = F6_ZERO;
        let f1 = F6_ONE;
        let f2 = F6lt([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5), Belt(6)]);

        assert_eq!(f6_mul(&f1, &f2), f2);
        assert_eq!(f6_mul(&f2, &f1), f2);
        assert_eq!(f6_mul(&f0, &f2), f0);
        assert_eq!(f6_mul(&f2, &f0), f0);
    }

    #[test]
    fn test_f6inv() -> Result<(), JetErr> {
        let f = F6_ONE;
        let f_inv = f6_inv(&f)?;
        assert_eq!(f_inv, f);

        let f = F6_ZERO;
        let f_inv = f6_inv(&f);
        assert!(f_inv.is_err());

        let f = F6lt([Belt(1), Belt(1), Belt(1), Belt(1), Belt(1), Belt(1)]);
        let f_inv = f6_inv(&f)?;
        assert_eq!(
            f_inv,
            F6lt([
                Belt(3074457344902430720),
                Belt(15372286724512153601),
                Belt(0),
                Belt(0),
                Belt(0),
                Belt(0)
            ])
        );

        let f = F6_TEST;
        let f_inv = f6_inv(&f)?;
        assert_eq!(
            f_inv,
            F6lt([
                Belt(129083178215983407),
                Belt(16804250925345184998),
                Belt(6447171951354165736),
                Belt(16181730381532049633),
                Belt(9179768094922373417),
                Belt(8139613426717722210)
            ])
        );

        Ok(())
    }

    #[test]
    fn test_f6_div() -> Result<(), JetErr> {
        let f1 = F6_TEST;
        let f2 = F6lt([Belt(0xdeadbeef), Belt(0xdead0001), Belt(0), Belt(0), Belt(0), Belt(0)]);
        let res = f6_div(&f1, &f2)?;
        assert_eq!(
            res,
            F6lt([
                Belt(7542375812088865094),
                Belt(15664235984267184732),
                Belt(2705725317242016633),
                Belt(4831474931498658260),
                Belt(4259601222882849719),
                Belt(5901377836576087143)
            ])
        );
        Ok(())
    }

    #[test]
    fn test_ch_scal() -> Result<(), JetErr> {
        let n = 3;

        let exp_pt = CheetahPoint {
            x: F6lt([
                Belt(12461929372724418873),
                Belt(16567359094004701986),
                Belt(18139376982535661051),
                Belt(3904128592858427998),
                Belt(1409597492055585669),
                Belt(10004445677131924957),
            ]),
            y: F6lt([
                Belt(11902197035441682466),
                Belt(5072010750673887563),
                Belt(16590571040514665822),
                Belt(11686652568553538253),
                Belt(9569866106958470758),
                Belt(6839548852764696901),
            ]),
            inf: false,
        };

        let res = ch_scal(n, &A_GEN)?;

        assert_eq!(res, exp_pt);
        Ok(())
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_ch_scal_jet() {
        let mut context = init_context();

        let a_gen_noun = A_GEN.to_noun(&mut context.stack);

        let n = 3;
        let sample = T(&mut context.stack, &[D(n), a_gen_noun]);

        // [%gen-cubed x=[a0=12.461.929.372.724.418.873 a1=16.567.359.094.004.701.986 a2=18.139.376.982.535.661.051 a3=3.904.128.592.858.427.998 a4=1.409.597.492.055.585.669 a5=10.004.445.677.131.924.957] y=[a0=11.902.197.035.441.682.466 a1=5.072.010.750.673.887.563 a2=16.590.571.040.514.665.822 a3=11.686.652.568.553.538.253 a4=9.569.866.106.958.470.758 a5=6.839.548.852.764.696.901] inf=%.n]
        let exp_pt = CheetahPoint {
            x: F6lt([
                Belt(12461929372724418873),
                Belt(16567359094004701986),
                Belt(18139376982535661051),
                Belt(3904128592858427998),
                Belt(1409597492055585669),
                Belt(10004445677131924957),
            ]),
            y: F6lt([
                Belt(11902197035441682466),
                Belt(5072010750673887563),
                Belt(16590571040514665822),
                Belt(11686652568553538253),
                Belt(9569866106958470758),
                Belt(6839548852764696901),
            ]),
            inf: false,
        };

        let exp_noun = exp_pt.to_noun(&mut context.stack);

        assert_jet(&mut context, ch_scal_jet, sample, exp_noun);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_ch_scal_jet_ubig() {
        let mut context = init_context();

        let a_gen_noun = A_GEN.to_noun(&mut context.stack);

        let n = A(&mut context.stack, &G_ORDER);
        let sample = T(&mut context.stack, &[n, a_gen_noun]);

        let exp_noun = A_ID.to_noun(&mut context.stack);

        assert_jet(&mut context, ch_scal_jet, sample, exp_noun);
    }
    #[test]
    fn test_verify_affine_sparse_seckey() -> Result<(), Box<dyn std::error::Error>> {
        // chal and sig are values taken from an example signature
        // secret_key: 0x8
        // message (hash): [0 1 2 3 4]
        let chal = UBig::from_str_radix(
            "6ed772faeda592c3d5c570169acb19e5e979ea9975409bfa28d874a88c34fba", 16,
        )?;
        let sig = UBig::from_str_radix(
            "64483168448a47664e22ba6c4a571eb0dd64dc5ee95b550c66b5227791278589", 16,
        )?;
        // pubkey
        let pubkey = CheetahPoint {
            x: F6lt([
                Belt(5226170347725594598),
                Belt(10326968723909427995),
                Belt(9909287574944299757),
                Belt(3389312162809687369),
                Belt(6741939401364684801),
                Belt(1215336833048603318),
            ]),
            y: F6lt([
                Belt(4761860904395420101),
                Belt(8266056389007434480),
                Belt(9911285737560359492),
                Belt(14968168698225451681),
                Belt(5907552010793110532),
                Belt(781863599964220501),
            ]),
            inf: false,
        };

        let m = [Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)];
        assert!(verify_affine(&pubkey, &m, &chal, &sig)?);
        Ok(())
    }

    #[test]
    fn test_verify_affine_dense_seckey() -> Result<(), Box<dyn std::error::Error>> {
        // chal and sig are values taken from an example signature
        // secret_key: g-order - 1
        // message (hash): [8 9 10 11 12]
        let chal = UBig::from_str_radix(
            "6f3cd43cd8709f4368aed04cd84292ab1c380cb645aaa7d010669d70375cbe88", 16,
        )?;
        let sig = UBig::from_str_radix(
            "5197ab182e307a350b5cf3606d6e99a6f35b0d382c8330dde6e51fb6ef8ebb8c", 16,
        )?;
        let pubkey = CheetahPoint {
            x: F6lt([
                Belt(2754611494552410273),
                Belt(8599518745794843693),
                Belt(10526511002404673680),
                Belt(4830863958577994148),
                Belt(375185138577093320),
                Belt(12938930721685970739),
            ]),
            y: F6lt([
                Belt(3062714866612034253),
                Belt(15671931273416742386),
                Belt(4071440668668521568),
                Belt(7738250649524482367),
                Belt(5259065445844042557),
                Belt(8456011930642078370),
            ]),
            inf: false,
        };
        let m = [Belt(8), Belt(9), Belt(10), Belt(11), Belt(12)];
        assert!(verify_affine(&pubkey, &m, &chal, &sig)?);
        Ok(())
    }

    #[test]
    fn test_verify_affine_scalar_range_checks() -> Result<(), Box<dyn std::error::Error>> {
        // Use the dense valid signature as the baseline, then confirm the jet
        // rejects every out-of-range scalar exactly as Hoon `+verify` does.
        let chal = UBig::from_str_radix(
            "6f3cd43cd8709f4368aed04cd84292ab1c380cb645aaa7d010669d70375cbe88", 16,
        )?;
        let sig = UBig::from_str_radix(
            "5197ab182e307a350b5cf3606d6e99a6f35b0d382c8330dde6e51fb6ef8ebb8c", 16,
        )?;
        let pubkey = CheetahPoint {
            x: F6lt([
                Belt(2754611494552410273),
                Belt(8599518745794843693),
                Belt(10526511002404673680),
                Belt(4830863958577994148),
                Belt(375185138577093320),
                Belt(12938930721685970739),
            ]),
            y: F6lt([
                Belt(3062714866612034253),
                Belt(15671931273416742386),
                Belt(4071440668668521568),
                Belt(7738250649524482367),
                Belt(5259065445844042557),
                Belt(8456011930642078370),
            ]),
            inf: false,
        };
        let m = [Belt(8), Belt(9), Belt(10), Belt(11), Belt(12)];

        // Baseline: the canonical signature verifies.
        assert!(verify_affine(&pubkey, &m, &chal, &sig)?);

        let zero = UBig::from(0u32);
        // chal == 0 and sig == 0 are rejected.
        assert!(!verify_affine(&pubkey, &m, &zero, &sig)?);
        assert!(!verify_affine(&pubkey, &m, &chal, &zero)?);
        // scalar == g-order is rejected (Hoon requires `lth scalar g-order`).
        assert!(!verify_affine(&pubkey, &m, &G_ORDER, &sig)?);
        assert!(!verify_affine(&pubkey, &m, &chal, &G_ORDER)?);
        // scalar > g-order is rejected. `sig + g-order` reduces to the same
        // [sig]G group element but is out of range, so it must be rejected to
        // match Hoon.
        let sig_plus = &sig + &*G_ORDER;
        assert!(!verify_affine(&pubkey, &m, &chal, &sig_plus)?);
        let chal_plus = &chal + &*G_ORDER;
        assert!(!verify_affine(&pubkey, &m, &chal_plus, &sig)?);
        Ok(())
    }

    #[test]
    fn test_batch_verify_affine() -> Result<(), Box<dyn std::error::Error>> {
        let mut context = init_context();
        let chal = UBig::from_str_radix(
            "6f3cd43cd8709f4368aed04cd84292ab1c380cb645aaa7d010669d70375cbe88", 16,
        )?;
        let sig = UBig::from_str_radix(
            "5197ab182e307a350b5cf3606d6e99a6f35b0d382c8330dde6e51fb6ef8ebb8c", 16,
        )?;
        let pubkey = CheetahPoint {
            x: F6lt([
                Belt(2754611494552410273),
                Belt(8599518745794843693),
                Belt(10526511002404673680),
                Belt(4830863958577994148),
                Belt(375185138577093320),
                Belt(12938930721685970739),
            ]),
            y: F6lt([
                Belt(3062714866612034253),
                Belt(15671931273416742386),
                Belt(4071440668668521568),
                Belt(7738250649524482367),
                Belt(5259065445844042557),
                Belt(8456011930642078370),
            ]),
            inf: false,
        };
        let m = [Belt(8), Belt(9), Belt(10), Belt(11), Belt(12)];

        let pubkey = pubkey.to_noun(&mut context.stack);
        let chal = Atom::from_ubig(&mut context.stack, &chal).as_noun();
        let sig = Atom::from_ubig(&mut context.stack, &sig).as_noun();
        let m = m.to_noun(&mut context.stack);
        let arg = T(&mut context.stack, &[pubkey, m, chal, sig]);
        let sample = T(&mut context.stack, &[arg, arg, arg, arg, arg, arg, D(0)]);
        assert_jet(&mut context, batch_verify_affine_jet, sample, YES);
        Ok(())
    }

    // Hand-computed `rap 5` (cat/met) reference for the faithful reconstruction.
    #[test]
    fn test_t8_to_scalar_rap5() {
        let two32 = UBig::from(1u64 << 32);
        // [1,0,...]: trailing zeros collapse -> 1
        assert_eq!(
            belt_schnorr_t8_to_ubig(&[1, 0, 0, 0, 0, 0, 0, 0].map(Belt)),
            UBig::from(1u64)
        );
        // [0,1,0,...]: leading zero has met=0, so l1 lands at position 0 -> 1
        assert_eq!(
            belt_schnorr_t8_to_ubig(&[0, 1, 0, 0, 0, 0, 0, 0].map(Belt)),
            UBig::from(1u64)
        );
        // [5,7,0,...]: both nonzero (met=1) -> 5 + 7*2^32
        assert_eq!(
            belt_schnorr_t8_to_ubig(&[5, 7, 0, 0, 0, 0, 0, 0].map(Belt)),
            UBig::from(5u64) + UBig::from(7u64) * &two32
        );
        // [5,0,7,0,...]: interior zero does not advance, so l2 lands at 2^32
        assert_eq!(
            belt_schnorr_t8_to_ubig(&[5, 0, 7, 0, 0, 0, 0, 0].map(Belt)),
            UBig::from(5u64) + UBig::from(7u64) * &two32
        );
        // a two-block limb (>= 2^32) has met=2, shifting the next limb by 64 bits
        assert_eq!(
            belt_schnorr_t8_to_ubig(&[1u64 << 33, 1, 0, 0, 0, 0, 0, 0].map(Belt)),
            UBig::from(1u64 << 33) + (UBig::from(1u64) << 64)
        );
    }

    // The deployed Hoon `?< =(scalar f6-zero)` is a no-op, so an identity sum
    // (sum.x == 0, from pubkey=A_GEN, chal==sig) must NOT error — it proceeds and
    // returns a boolean (false here), matching Hoon.
    #[test]
    fn test_verify_affine_identity_sum() -> Result<(), Box<dyn std::error::Error>> {
        let k = UBig::from(12_345u64);
        let m = [Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)];
        // sum = k*A_GEN + (-(k*A_GEN)) = identity.
        let res = verify_affine(&A_GEN, &m, &k, &k)?;
        assert!(!res);
        Ok(())
    }

    fn ubig_to_t8(scalar: &UBig) -> [Belt; 8] {
        let radix = UBig::from(1u64 << 32);
        let mut limbs = [Belt(0); 8];
        let mut x = scalar.clone();
        for limb in limbs.iter_mut() {
            let r = &x % &radix;
            *limb = Belt(u64::try_from(&r).unwrap());
            x /= &radix;
        }
        limbs
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_belt_schnorr_batch_verify_jet() -> Result<(), Box<dyn std::error::Error>> {
        let mut context = init_context();
        let chal = UBig::from_str_radix(
            "6f3cd43cd8709f4368aed04cd84292ab1c380cb645aaa7d010669d70375cbe88", 16,
        )?;
        let sig = UBig::from_str_radix(
            "5197ab182e307a350b5cf3606d6e99a6f35b0d382c8330dde6e51fb6ef8ebb8c", 16,
        )?;
        let pubkey = CheetahPoint {
            x: F6lt([
                Belt(2754611494552410273),
                Belt(8599518745794843693),
                Belt(10526511002404673680),
                Belt(4830863958577994148),
                Belt(375185138577093320),
                Belt(12938930721685970739),
            ]),
            y: F6lt([
                Belt(3062714866612034253),
                Belt(15671931273416742386),
                Belt(4071440668668521568),
                Belt(7738250649524482367),
                Belt(5259065445844042557),
                Belt(8456011930642078370),
            ]),
            inf: false,
        };
        let m = [Belt(8), Belt(9), Belt(10), Belt(11), Belt(12)];

        let chal_t8 = ubig_to_t8(&chal);
        let sig_t8 = ubig_to_t8(&sig);
        // this vector has no zero 32-bit limb, so the faithful reconstruction
        // round-trips back to the original scalar.
        assert_eq!(belt_schnorr_t8_to_ubig(&chal_t8), chal);
        assert_eq!(belt_schnorr_t8_to_ubig(&sig_t8), sig);

        let pk_n = pubkey.to_noun(&mut context.stack);
        let m_n = m.to_noun(&mut context.stack);
        let chal_n = chal_t8.to_noun(&mut context.stack);
        let sig_n = sig_t8.to_noun(&mut context.stack);
        let elem = T(&mut context.stack, &[pk_n, m_n, chal_n, sig_n]);
        // a batch of two copies of a valid signature verifies.
        let sample = T(&mut context.stack, &[elem, elem, D(0)]);
        assert_jet(&mut context, belt_schnorr_batch_verify_jet, sample, YES);

        // empty batch -> levy of empty is %.y
        let empty = D(0);
        assert_jet(&mut context, belt_schnorr_batch_verify_jet, empty, YES);

        // corrupt the challenge -> the whole batch fails.
        let mut bad_chal_t8 = chal_t8;
        bad_chal_t8[0] = Belt(bad_chal_t8[0].0 ^ 1);
        let bad_chal_n = bad_chal_t8.to_noun(&mut context.stack);
        let bad_elem = T(&mut context.stack, &[pk_n, m_n, bad_chal_n, sig_n]);
        let bad_sample = T(&mut context.stack, &[elem, bad_elem, D(0)]);
        assert_jet(&mut context, belt_schnorr_batch_verify_jet, bad_sample, NO);
        Ok(())
    }
}
