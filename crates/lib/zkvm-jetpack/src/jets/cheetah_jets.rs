use ibig::UBig;
use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::mem::NockStack;
use nockvm::noun::{Atom, Noun, T};
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

/// Jet for `+sign:affine:schnorr:cheetah` (open/hoon/common/ztd/three.hoon).
/// This is the deterministic Schnorr signing arm on the per-input v1
/// transaction signing hot path. The Hoon:
///
/// ```hoon
/// =/  m-list  (leaf-sequence:shape m)
/// ?>  (levy sk-as-32-bit-belts |=(n=@ (lth n (bex 32))))
/// =/  sk      (rep 5 sk-as-32-bit-belts)
/// ?<  =(sk 0)
/// ?>  (lth sk g-order:curve)
/// =/  pubkey  (ch-scal:affine:curve sk a-gen:curve)
/// =/  nonce
///   (trunc-g-order (hash-varlen:tip5 (zing [x.pubkey y.pubkey m-list sk-belts ~])))
/// ?<  =(nonce 0)
/// =/  scalar  (ch-scal:affine:curve nonce a-gen:curve)
/// =/  chal
///   (trunc-g-order (hash-varlen:tip5 (zing [x.scalar y.scalar x.pubkey y.pubkey m-list])))
/// ?<  =(chal 0)
/// =/  sig  (mod (add nonce (mul chal sk)) g-order:curve)
/// ?<  =(sig 0)
/// [chal sig]
/// ```
///
/// `rep 5` packs each limb into a fixed 2^32-wide block, so a limb `>= 2^32`
/// would change the reconstruction; the Hoon `?>` rejects that, and every
/// `?>`/`?<` guard the Hoon would crash or short-circuit on (limb out of range,
/// degenerate `sk`/`nonce`/`chal`/`sig`) is surfaced here as `None` so the jet
/// Punts and the authoritative Hoon runs rather than the jet diverging.
/// `Some((chal, sig))` is the canonical, byte-identical signature.
pub fn sign_affine(sk_belts: &[u64], m: &[Belt; 5]) -> Result<Option<(UBig, UBig)>, JetErr> {
    let two_32: u64 = 1 << 32;
    //  ?>  (levy sk-as-32-bit-belts |=(n=@ (lth n (bex 32))))
    if sk_belts.iter().any(|&limb| limb >= two_32) {
        return Ok(None);
    }
    //  sk = (rep 5 sk-as-32-bit-belts): little-endian base-2^32 reconstruction
    //  with fixed-width 32-bit blocks (NOT `rap 5`; limbs are guaranteed < 2^32).
    let mut sk = UBig::from(0u32);
    for (index, &limb) in sk_belts.iter().enumerate() {
        sk += UBig::from(limb) << (32 * index);
    }
    let zero = UBig::from(0u32);
    //  ?<  =(sk 0)  /  ?>  (lth sk g-order:curve)
    if sk == zero || sk >= *G_ORDER {
        return Ok(None);
    }
    //  pubkey = sk * G
    let pubkey = ch_scal_big(&sk, &A_GEN)?;

    //  nonce = trunc-g-order(hash-varlen(zing [x.pubkey y.pubkey m-list sk-belts]))
    let mut nonce_pre: Vec<Belt> = Vec::with_capacity(6 + 6 + 5 + sk_belts.len());
    nonce_pre.extend_from_slice(&pubkey.x.0);
    nonce_pre.extend_from_slice(&pubkey.y.0);
    nonce_pre.extend_from_slice(m);
    nonce_pre.extend(sk_belts.iter().map(|&limb| Belt(limb)));
    let nonce = trunc_g_order(&tip5::hash::hash_varlen(&mut nonce_pre));
    //  ?<  =(nonce 0)
    if nonce == zero {
        return Ok(None);
    }
    //  scalar = nonce * G
    let scalar = ch_scal_big(&nonce, &A_GEN)?;

    //  chal = trunc-g-order(hash-varlen(zing [x.scalar y.scalar x.pubkey y.pubkey m-list]))
    let mut chal_pre: Vec<Belt> = Vec::with_capacity(6 * 4 + 5);
    chal_pre.extend_from_slice(&scalar.x.0);
    chal_pre.extend_from_slice(&scalar.y.0);
    chal_pre.extend_from_slice(&pubkey.x.0);
    chal_pre.extend_from_slice(&pubkey.y.0);
    chal_pre.extend_from_slice(m);
    let chal = trunc_g_order(&tip5::hash::hash_varlen(&mut chal_pre));
    //  ?<  =(chal 0)
    if chal == zero {
        return Ok(None);
    }
    //  sig = (nonce + chal*sk) mod g-order
    let product = &chal * &sk;
    let sum = &nonce + &product;
    let sig = sum % &*G_ORDER;
    //  ?<  =(sig 0)
    if sig == zero {
        return Ok(None);
    }
    Ok(Some((chal, sig)))
}

/// Jet wrapper for `+sign:affine:schnorr:cheetah`. Sample is
/// `[sk-as-32-bit-belts=(list belt) m=noun-digest:tip5]`; result is `[c=@ux s=@ux]`.
/// Any input the jet does not model (a non-atom limb, an `m` that is not a
/// 5-tuple of belts, or any `?>`/`?<` guard the Hoon would crash on) Punts so
/// the runtime re-runs the authoritative Hoon.
pub fn sign_affine_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let sk_list = slot(sam, 2, &space)?;
    let m_noun = slot(sam, 3, &space)?;

    let m = <[Belt; 5]>::from_noun(&m_noun, &space).map_err(|_| JetErr::Punt)?;
    //  Decode the `(list belt)` of 32-bit limbs. A limb that is not an atom, or
    //  is too wide to be a `@` that `rep 5` would treat as a single block we
    //  model, Punts to the Hoon.
    let sk_belts = sk_list
        .in_space(&space)
        .list_iter()
        .map(|limb| {
            limb.as_atom()
                .map_err(|_| JetErr::Punt)?
                .as_u64()
                .map_err(|_| JetErr::Punt)
        })
        .collect::<Result<Vec<u64>, JetErr>>()?;

    match sign_affine(&sk_belts, &m)? {
        Some((chal, sig)) => {
            let chal_noun = Atom::from_ubig(&mut context.stack, &chal).as_noun();
            let sig_noun = Atom::from_ubig(&mut context.stack, &sig).as_noun();
            Ok(T(&mut context.stack, &[chal_noun, sig_noun]))
        }
        None => Err(JetErr::Punt),
    }
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

    /// Decompose a scalar into little-endian fixed-width 2^32 blocks, i.e. the
    /// `sk-as-32-bit-belts` list whose `(rep 5 _)` reconstructs the scalar.
    fn ubig_to_rep5_limbs(n: &UBig) -> Vec<u64> {
        let radix = UBig::from(1u64 << 32);
        let zero = UBig::from(0u32);
        let mut limbs = Vec::new();
        let mut x = n.clone();
        while x > zero {
            limbs.push(u64::try_from(&(&x % &radix)).unwrap());
            x /= &radix;
        }
        if limbs.is_empty() {
            limbs.push(0);
        }
        limbs
    }

    fn hex(s: &str) -> UBig {
        UBig::from_str_radix(s, 16).expect("valid hex")
    }

    //  The four signature vectors `+test-cheetah-sign` (hoon/tests/crypto/mod/
    //  cheetah.hoon) pins against the pure Hoon `+sign:affine:schnorr`. This is
    //  the byte-identical KAT for `sign_affine`: same inputs, same `[chal sig]`.
    fn sign_kats() -> Vec<(UBig, [Belt; 5], UBig, UBig)> {
        vec![
            (
                UBig::from(1u32),
                [Belt(0); 5],
                hex("4a2511e6552729502867c400116d40bc6c59b7e77eac956f7e521ad983220c32"),
                hex("4395849073d2f7e838f74abd41661f7ab09ef5f6356ffe27b3266e24f8b96ce6"),
            ),
            (
                UBig::from(8u32),
                [Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)],
                hex("30fa095225112a75f24c26afd1d32c6bd655eab4272d5a9b83f973c45b41193"),
                hex("77fc1604219900af3330a21dd388a638f7880ad498ef6e3dd4421aeeb994ee4"),
            ),
            (
                &*G_ORDER - UBig::from(1u32),
                [Belt(8), Belt(9), Belt(10), Belt(11), Belt(12)],
                hex("3d177aeef7321eba9896b14417b1ed17ea0280e2c82fc0f719be77399be336f9"),
                hex("31d5dee5feb4ca7df85051d7f56b2a121bf8097c2bc668804d246fea50656402"),
            ),
            (
                hex("123456789abcdef0fedcba9876543210abcdef12"),
                [Belt(100), Belt(200), Belt(300), Belt(400), Belt(500)],
                hex("9eb985344b4baebed3b6bd3ef8c0b3e5036c36b34dc9e1c4d49b769b074585"),
                hex("5cbee97145fab06e427a971e0190f7d83ac4dab6af4aad8b687d8eea03a40771"),
            ),
        ]
    }

    #[test]
    fn test_sign_affine_matches_hoon_vectors() {
        for (sk, m, exp_chal, exp_sig) in sign_kats() {
            let limbs = ubig_to_rep5_limbs(&sk);
            let (chal, sig) = sign_affine(&limbs, &m)
                .expect("no jet error")
                .expect("non-degenerate signature");
            assert_eq!(chal, exp_chal, "chal mismatch for sk={sk}");
            assert_eq!(sig, exp_sig, "sig mismatch for sk={sk}");
        }
    }

    //  Every produced signature must verify under the byte-identical
    //  `verify_affine` — independent corroboration that the deterministic output
    //  is a valid signature, across small/dense/multi-limb keys and varied msgs.
    #[test]
    fn test_sign_then_verify_roundtrip() -> Result<(), JetErr> {
        let g_minus = &*G_ORDER - UBig::from(1u32);
        let scalars = [
            UBig::from(1u32),
            UBig::from(2u32),
            UBig::from(8u32),
            UBig::from(0xdead_beef_u64),
            UBig::from(1u64 << 40),
            g_minus.clone(),
            &g_minus / UBig::from(2u32),
        ];
        for (i, sk) in scalars.iter().enumerate() {
            let limbs = ubig_to_rep5_limbs(sk);
            let m = [
                Belt(i as u64),
                Belt(i as u64 + 7),
                Belt(0xffff_ffff_ffff_ffff - i as u64),
                Belt(42),
                Belt(i as u64 * 1000),
            ];
            let (chal, sig) = sign_affine(&limbs, &m)?.expect("non-degenerate");
            let pubkey = ch_scal_big(sk, &A_GEN)?;
            assert!(
                verify_affine(&pubkey, &m, &chal, &sig)?,
                "signature for sk={sk} did not verify"
            );
        }
        Ok(())
    }

    //  Guards the Hoon `?>`/`?<` arms surface as `None` (the jet Punts to Hoon).
    #[test]
    fn test_sign_affine_punts_on_guard_violations() -> Result<(), JetErr> {
        let m = [Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)];
        // a limb >= 2^32 is rejected by `?> (levy ... (lth n (bex 32)))`
        assert!(sign_affine(&[1u64 << 32], &m)?.is_none());
        assert!(sign_affine(&[5, 1u64 << 33, 7], &m)?.is_none());
        // sk == 0 is rejected by `?< =(sk 0)`
        assert!(sign_affine(&[0], &m)?.is_none());
        assert!(sign_affine(&[0, 0], &m)?.is_none());
        assert!(sign_affine(&[], &m)?.is_none());
        // sk >= g-order is rejected by `?> (lth sk g-order)`
        let g_limbs = ubig_to_rep5_limbs(&G_ORDER);
        assert!(sign_affine(&g_limbs, &m)?.is_none());
        Ok(())
    }

    //  The jet wrapper end-to-end: decode `[sk-list m]`, produce `[chal sig]`.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_sign_affine_jet() {
        let mut context = init_context();
        // sk = 8 -> sk-list ~[8]; m = [1 2 3 4 5]
        let sk_list = T(&mut context.stack, &[D(8), D(0)]);
        let m = [Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)];
        let m_noun = m.to_noun(&mut context.stack);
        let sample = T(&mut context.stack, &[sk_list, m_noun]);

        let chal = Atom::from_ubig(
            &mut context.stack,
            &hex("30fa095225112a75f24c26afd1d32c6bd655eab4272d5a9b83f973c45b41193"),
        )
        .as_noun();
        let sig = Atom::from_ubig(
            &mut context.stack,
            &hex("77fc1604219900af3330a21dd388a638f7880ad498ef6e3dd4421aeeb994ee4"),
        )
        .as_noun();
        let expected = T(&mut context.stack, &[chal, sig]);
        assert_jet(&mut context, sign_affine_jet, sample, expected);
    }
}
