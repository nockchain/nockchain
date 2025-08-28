use bs58;
use ibig::UBig;
use nockvm::interpreter::Context;
use nockvm::jets::util::BAIL_FAIL;
use nockvm::jets::JetErr;
use nockvm::noun::{Atom, Noun, NounAllocator, Slots, NO, T, YES};
use noun_serde::{NounDecode, NounDecodeError, NounEncode};
use once_cell::sync::Lazy;

use crate::form::base::bneg;
use crate::form::bpoly::{bpegcd, bpscal};
use crate::form::{Belt, PRIME};
use crate::jets::tip5_jets::hash_varlen;
use crate::noun::noun_ext::AtomExt;

// TODO: move this into a nockchain-types crate
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheetahPoint {
    pub x: F6lt,
    pub y: F6lt,
    pub inf: bool,
}

static G_ORDER: Lazy<UBig> = Lazy::new(|| {
    UBig::from_str_radix(
        "7af2599b3b3f22d0563fbf0f990a37b5327aa72330157722d443623eaed4accf", 16,
    )
    .unwrap()
});

static P_BIG: Lazy<UBig> = Lazy::new(|| UBig::from(PRIME));
static P_BIG_2: Lazy<UBig> = Lazy::new(|| &*P_BIG * &*P_BIG);
static P_BIG_3: Lazy<UBig> = Lazy::new(|| &*P_BIG_2 * &*P_BIG);

pub const A_GEN: CheetahPoint = CheetahPoint {
    x: F6lt([
        Belt(2754611494552410273),
        Belt(8599518745794843693),
        Belt(10526511002404673680),
        Belt(4830863958577994148),
        Belt(375185138577093320),
        Belt(12938930721685970739),
    ]),
    y: F6lt([
        Belt(15384029202802550068),
        Belt(2774812795997841935),
        Belt(14375303400746062753),
        Belt(10708493419890101954),
        Belt(13187678623570541764),
        Belt(9990732138772505951),
    ]),
    inf: false,
};

impl CheetahPoint {
    pub fn into_base58(&self) -> Result<String, Box<dyn std::error::Error>> {
        if self.inf {
            return Err("CheetahPoint: point is at infinity, we will not encode it".into());
        }
        // Convert the Belt values to u64 bytes
        let mut bytes = Vec::new();
        bytes.push(0x1);
        for belt in self.y.0.iter().rev().chain(self.x.0.iter().rev()) {
            bytes.extend_from_slice(&belt.0.to_be_bytes());
        }
        Ok(bs58::encode(bytes).into_string())
    }

    pub fn from_base58(b58: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let v = bs58::decode(b58).into_vec()?;
        // (6 + 6) * 8 + 1 leading byte
        if v.len() != 97 {
            return Err("CheetahPoint: invalid base58 string length".into());
        }

        let mut v64 = v[1..]
            .chunks_exact(8)
            .map(|a| {
                let arr = <[u8; 8]>::try_from(a)
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
                Ok(Belt(u64::from_be_bytes(arr)))
            })
            .collect::<Result<Vec<Belt>, Box<dyn std::error::Error>>>()?;

        v64.reverse();

        let c_pt = CheetahPoint {
            x: F6lt {
                0: <[Belt; 6]>::try_from(&v64[..6])?,
            },
            y: F6lt {
                0: <[Belt; 6]>::try_from(&v64[6..])?,
            },
            inf: false,
        };

        if c_pt.in_curve() {
            Ok(c_pt)
        } else {
            Err("CheetahPoint: point is not on the curve".into())
        }
    }

    pub fn in_curve(&self) -> bool {
        if *self == A_ID {
            return true;
        }
        let scaled = ch_scal_big(&G_ORDER, self).unwrap();
        scaled == A_ID
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct F6lt(pub [Belt; 6]);

#[inline(always)]
pub(crate) fn make_n_belt<A: NounAllocator>(stack: &mut A, arr: &[Belt]) -> Noun {
    assert!(!arr.is_empty());
    let n = arr.len();
    let mut res_cell = Atom::new(stack, arr[n - 1].0).as_noun();
    for i in (0..n - 1).rev() {
        let b = Atom::new(stack, arr[i].0).as_noun();
        res_cell = T(stack, &[b, res_cell]);
    }
    res_cell
}

impl NounEncode for F6lt {
    fn to_noun<A: NounAllocator>(&self, stack: &mut A) -> Noun {
        make_n_belt(stack, &self.0)
    }
}

impl NounDecode for F6lt {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let mut x = *noun;
        // convert f6lts to vecs
        let mut f6lt = [Belt(0); 6];
        for i in 0..5 {
            let cell = x.as_cell()?;
            f6lt[i] = cell.head().as_atom()?.as_belt()?;
            x = cell.tail();
        }
        f6lt[5] = x.as_atom()?.as_belt()?;

        Ok(F6lt(f6lt))
    }
}

impl NounDecode for CheetahPoint {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let x = noun.slot(2)?;
        let y = noun.slot(6)?;
        let inf = noun.slot(7)?;

        // convert f6lts to vecs
        let y_f6lt = F6lt::from_noun(&x)?;
        let x_f6lt = F6lt::from_noun(&y)?;

        Ok(CheetahPoint {
            x: y_f6lt,
            y: x_f6lt,
            inf: inf.as_atom()?.as_bool()?,
        })
    }
}

impl NounEncode for CheetahPoint {
    fn to_noun<A: NounAllocator>(&self, stack: &mut A) -> Noun {
        let x_noun = make_n_belt(stack, &self.x.0);
        let y_noun = make_n_belt(stack, &self.y.0);
        let inf_noun = if self.inf { YES } else { NO };
        T(stack, &[x_noun, y_noun, inf_noun])
    }
}

#[inline(always)]
pub(crate) fn f6_div(f1: &F6lt, f2: &F6lt) -> Result<F6lt, JetErr> {
    let f2_inv = f6_inv(f2)?;
    Ok(f6_mul(f1, &f2_inv))
}

#[inline(always)]
fn karat3(a: &[Belt; 3], b: &[Belt; 3]) -> [Belt; 5] {
    let m = [a[0] * b[0], a[1] * b[1], a[2] * b[2]];
    [
        m[0],
        (a[0] + a[1]) * (b[0] + b[1]) - (m[0] + m[1]),
        (a[0] + a[2]) * (b[0] + b[2]) - (m[0] + m[2]) + m[1],
        (a[1] + a[2]) * (b[1] + b[2]) - (m[1] + m[2]),
        m[2],
    ]
}

#[inline(always)]
fn f6_mul(f: &F6lt, g: &F6lt) -> F6lt {
    let f0g0 = karat3(&[f.0[0], f.0[1], f.0[2]], &[g.0[0], g.0[1], g.0[2]]);
    let f1g1 = karat3(&[f.0[3], f.0[4], f.0[5]], &[g.0[3], g.0[4], g.0[5]]);

    let foil = karat3(
        &[f.0[0] + f.0[3], f.0[1] + f.0[4], f.0[2] + f.0[5]],
        &[g.0[0] + g.0[3], g.0[1] + g.0[4], g.0[2] + g.0[5]],
    );

    let cross = [
        foil[0] - (f0g0[0] + f1g1[0]),
        foil[1] - (f0g0[1] + f1g1[1]),
        foil[2] - (f0g0[2] + f1g1[2]),
        foil[3] - (f0g0[3] + f1g1[3]),
        foil[4] - (f0g0[4] + f1g1[4]),
    ];
    F6lt([
        f0g0[0] + Belt(7) * (cross[3] + f1g1[0]),
        f0g0[1] + Belt(7) * (cross[4] + f1g1[1]),
        f0g0[2] + Belt(7) * f1g1[2],
        f0g0[3] + cross[0] + Belt(7) * f1g1[3],
        f0g0[4] + cross[1] + Belt(7) * f1g1[4],
        cross[2],
    ])
}

#[inline(always)]
fn f6_inv(f: &F6lt) -> Result<F6lt, JetErr> {
    if f == &F6_ZERO {
        return Err(BAIL_FAIL);
    }
    let mut res = [Belt(0); 6];
    // length of d is at most min(6, 7) + 1
    let mut d = [Belt(0); 7];
    // length of u is at most deg(b) = 7
    let mut u = [Belt(0); 7];
    // length of u is at most deg(a) = 6
    let mut v = [Belt(0); 6];
    bpegcd(
        &f.0,
        &[Belt(bneg(7)), Belt(0), Belt(0), Belt(0), Belt(0), Belt(0), Belt(1)],
        &mut d,
        &mut u,
        &mut v,
    );
    let inv = d[0].inv();
    bpscal(inv, &u, &mut res);
    Ok(F6lt(res))
}

#[inline(always)]
fn f6_add(f1: &F6lt, f2: &F6lt) -> F6lt {
    F6lt([
        f1.0[0] + f2.0[0],
        f1.0[1] + f2.0[1],
        f1.0[2] + f2.0[2],
        f1.0[3] + f2.0[3],
        f1.0[4] + f2.0[4],
        f1.0[5] + f2.0[5],
    ])
}

fn f6_scal(s: Belt, f: &F6lt) -> F6lt {
    F6lt([f.0[0] * s, f.0[1] * s, f.0[2] * s, f.0[3] * s, f.0[4] * s, f.0[5] * s])
}

// TODO: Try karat3-square if performance is an issue
#[inline(always)]
fn f6_square(f: &F6lt) -> F6lt {
    f6_mul(f, f)
}

#[inline(always)]
fn f6_neg(f: &F6lt) -> F6lt {
    F6lt([-f.0[0], -f.0[1], -f.0[2], -f.0[3], -f.0[4], -f.0[5]])
}

#[inline(always)]
fn f6_sub(f1: &F6lt, f2: &F6lt) -> F6lt {
    f6_add(f1, &f6_neg(f2))
}

#[inline(always)]
fn ch_double_unsafe(x: &F6lt, y: &F6lt) -> Result<CheetahPoint, JetErr> {
    let slope = f6_div(
        &f6_add(&f6_scal(Belt(3), &f6_square(x)), &F6_ONE),
        &f6_scal(Belt(2), y),
    )?;
    let x_out = f6_sub(&f6_square(&slope), &f6_scal(Belt(2), x));
    let y_out = f6_sub(&f6_mul(&slope, &f6_sub(x, &x_out)), y);
    Ok(CheetahPoint {
        x: x_out,
        y: y_out,
        inf: false,
    })
}

pub(crate) const A_ID: CheetahPoint = CheetahPoint {
    x: F6_ZERO,
    y: F6_ONE,
    inf: true,
};
pub(crate) const F6_ZERO: F6lt = F6lt([Belt(0); 6]);
pub(crate) const F6_ONE: F6lt = F6lt([Belt(1), Belt(0), Belt(0), Belt(0), Belt(0), Belt(0)]);

#[inline(always)]
fn ch_double(p: CheetahPoint) -> Result<CheetahPoint, JetErr> {
    if p.inf {
        return Ok(A_ID);
    }
    if p.y == F6_ZERO {
        return Ok(A_ID);
    }
    ch_double_unsafe(&p.x, &p.y)
}

#[inline(always)]
fn ch_add_unsafe(p: CheetahPoint, q: CheetahPoint) -> Result<CheetahPoint, JetErr> {
    let slope = f6_div(&f6_sub(&p.y, &q.y), &f6_sub(&p.x, &q.x))?;
    let x_out = f6_sub(&f6_square(&slope), &f6_add(&p.x, &q.x));
    let y_out = f6_sub(&f6_mul(&slope, &f6_sub(&p.x, &x_out)), &p.y);
    Ok(CheetahPoint {
        x: x_out,
        y: y_out,
        inf: false,
    })
}

#[inline(always)]
fn ch_neg(p: &CheetahPoint) -> CheetahPoint {
    CheetahPoint {
        x: p.x,
        y: f6_neg(&p.y),
        inf: p.inf,
    }
}

#[inline(always)]
fn ch_add(p: &CheetahPoint, q: &CheetahPoint) -> Result<CheetahPoint, JetErr> {
    if p.inf {
        return Ok(*q);
    }
    if q.inf {
        return Ok(*p);
    }
    if *p == ch_neg(q) {
        return Ok(A_ID);
    }
    if p == q {
        return ch_double(*p);
    }
    ch_add_unsafe(*p, *q)
}

#[inline(always)]
pub(crate) fn ch_scal(n: u64, p: &CheetahPoint) -> Result<CheetahPoint, JetErr> {
    let mut n = n;
    let mut p_copy = *p;
    let mut acc = A_ID;
    while n > 0 {
        if n & 1 == 1 {
            acc = ch_add(&acc, &p_copy)?;
        }
        p_copy = ch_double(p_copy)?;
        n >>= 1;
    }
    Ok(acc)
}

#[inline(always)]
pub(crate) fn ch_scal_big(n: &UBig, p: &CheetahPoint) -> Result<CheetahPoint, JetErr> {
    let mut n_copy = n.clone();
    let zero = UBig::from(0u64);
    let mut p_copy = *p;
    let mut acc = A_ID;

    while n_copy > zero {
        // Check if least significant bit is set
        if n_copy.bit(0) {
            acc = ch_add(&acc, &p_copy)?;
        }
        p_copy = ch_double(p_copy)?;
        n_copy >>= 1; // Right shift by 1 bit
    }
    Ok(acc)
}

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

    let res = verify_affine(pubkey, &m, chal, sig)?;
    Ok(res.to_noun(&mut context.stack))
}

#[inline(always)]
pub fn verify_affine(
    pubkey: CheetahPoint,
    m: &[Belt; 5],
    chal: UBig,
    sig: UBig,
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

    let hash = hash_varlen(&mut hashable);
    let truncated_hash = trunc_g_order(&hash);

    Ok(truncated_hash == chal)
}

fn trunc_g_order(a: &[u64]) -> UBig {
    let mut result = UBig::from(a[0]);
    result += &*P_BIG * UBig::from(a[1]);
    result += &*P_BIG_2 * UBig::from(a[2]);
    result += &*P_BIG_3 * UBig::from(a[3]);

    result % &*G_ORDER
}

#[cfg(test)]
mod tests {
    use ibig::UBig;
    use nockvm::jets::util::test::{assert_jet, init_context, A};
    use nockvm::noun::{D, T};
    use noun_serde::NounEncode;

    use super::*;
    use crate::form::Belt;

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
        let x = "32KVTmv3ofSyACq9nC1Hgnk4Jt8rs2hj1cvDZWC1EQuiYFMDg8MaLtF3ntafJbEUH5XPV1pK3K4xkxfjRPAWprBb7LYCVv4HF7817Bwh9M9xAdmgrPt77j4xejihNFd9h5Eo";
        let point = CheetahPoint::from_base58(&x).unwrap();
        let x_round = point.into_base58().unwrap();
        assert_eq!(x, x_round)
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
        let mut context = init_context();
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
        let pubkey = pubkey.to_noun(&mut context.stack);
        let chal = Atom::from_ubig(&mut context.stack, &chal).as_noun();
        let sig = Atom::from_ubig(&mut context.stack, &sig).as_noun();
        let m = m.to_noun(&mut context.stack);
        let sample = T(&mut context.stack, &[pubkey, m, chal, sig]);
        assert_jet(&mut context, verify_affine_jet, sample, YES);
        Ok(())
    }

    #[test]
    fn test_verify_affine_dense_seckey() -> Result<(), Box<dyn std::error::Error>> {
        let mut context = init_context();
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

        let pubkey = pubkey.to_noun(&mut context.stack);
        let chal = Atom::from_ubig(&mut context.stack, &chal).as_noun();
        let sig = Atom::from_ubig(&mut context.stack, &sig).as_noun();
        let m = m.to_noun(&mut context.stack);
        let sample = T(&mut context.stack, &[pubkey, m, chal, sig]);
        assert_jet(&mut context, verify_affine_jet, sample, YES);
        Ok(())
    }
}
