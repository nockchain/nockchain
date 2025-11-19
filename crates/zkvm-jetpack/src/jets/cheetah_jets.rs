use ibig::UBig;
use nockvm::ext::NounExt;
use nockvm::interpreter::Context;
use nockvm::jets::util::BAIL_FAIL;
use nockvm::jets::JetErr;
use nockvm::noun::{Noun, Slots};
use noun_serde::{NounDecode, NounEncode};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

use crate::form::belt::*;
use crate::form::crypto::cheetah::*;
use crate::form::tip5;

#[inline(always)]
pub fn ch_scal_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let sam = subject.slot(6)?;
    let n_atom = sam.slot(2)?.as_atom()?;

    let p = sam.slot(3)?;
    let a_pt = CheetahPoint::from_noun(&p).map_err(|_| BAIL_FAIL)?;

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
    let sam = subject.slot(6)?;
    let pubkey = sam.slot(2)?;
    let m = sam.slot(6)?;
    let chal = sam.slot(14)?.as_atom()?.as_ubig(&mut context.stack);
    let sig = sam.slot(15)?.as_atom()?.as_ubig(&mut context.stack);

    let pubkey: CheetahPoint = CheetahPoint::from_noun(&pubkey).map_err(|_| BAIL_FAIL)?;
    let m = <[Belt; 5]>::from_noun(&m).map_err(|_| BAIL_FAIL)?;

    let res = verify_affine(pubkey, &m, &chal, &sig)?;
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
    let list = subject.slot(6)?;
    let args = list
        .list_iter()
        .map(|arg| {
            let pubkey = CheetahPoint::from_noun(&arg.slot(2)?).map_err(|_| BAIL_FAIL)?;
            let m = <[Belt; 5]>::from_noun(&arg.slot(6)?).map_err(|_| BAIL_FAIL)?;
            let chal = arg.slot(14)?.as_atom()?.as_ubig(&mut context.stack);
            let sig = arg.slot(15)?.as_atom()?.as_ubig(&mut context.stack);
            Ok(ValidateArgs {
                pubkey,
                m,
                chal,
                sig,
            })
        })
        .collect::<Result<Vec<ValidateArgs>, JetErr>>()?;

    let all_signatures_valid = !args
        .par_iter()
        .map(|arg| {
            let ValidateArgs {
                pubkey,
                m,
                chal,
                sig,
            } = arg;
            verify_affine(*pubkey, m, chal, sig).unwrap()
        })
        //  check if any result is invalid and try to short-circuit as soon as an
        //  invalid result is found
        .any(|result| !result);
    Ok(all_signatures_valid.to_noun(&mut context.stack))
}

#[inline(always)]
pub fn verify_affine(
    pubkey: CheetahPoint,
    m: &[Belt],
    chal: &UBig,
    sig: &UBig,
) -> Result<bool, JetErr> {
    let left = ch_scal_big(&sig, &A_GEN)?;
    let right = ch_neg(&ch_scal_big(&chal, &pubkey)?);
    let sum = ch_add(&left, &right)?;
    if sum.x == F6_ZERO {
        return Err(BAIL_FAIL);
    }

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

#[cfg(test)]
mod tests {
    use ibig::UBig;
    use nockvm::jets::util::test::{assert_jet, init_context, A};
    use nockvm::noun::{Atom, D, T, YES};
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
                let point = CheetahPoint::from_base58(&x).unwrap();
                let x_round = point.into_base58().unwrap();
                assert_eq!(x, x_round)
            }
    }

    #[test]
    fn test_cheetah_point_from_b58() {
        for expected_point in [A_GEN] {
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
        }

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

        let n = A(&mut context.stack, &*G_ORDER);
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
        assert!(verify_affine(pubkey, &m, &chal, &sig)?);
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
        assert!(verify_affine(pubkey, &m, &chal, &sig)?);
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
}
