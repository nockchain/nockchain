use bs58;
use ibig::UBig;
use nockvm::jets::util::BAIL_FAIL;
use nockvm::jets::JetErr;
use noun_serde::{NounDecode, NounEncode};
use once_cell::sync::Lazy;

use crate::belt::{bneg, Belt, PRIME};
use crate::bpoly::{bpegcd, bpscal};

pub static G_ORDER: Lazy<UBig> = Lazy::new(|| {
    UBig::from_str_radix(
        "7af2599b3b3f22d0563fbf0f990a37b5327aa72330157722d443623eaed4accf", 16,
    )
    .unwrap()
});

pub static P_BIG: Lazy<UBig> = Lazy::new(|| UBig::from(PRIME));
pub static P_BIG_2: Lazy<UBig> = Lazy::new(|| &*P_BIG * &*P_BIG);
pub static P_BIG_3: Lazy<UBig> = Lazy::new(|| &*P_BIG_2 * &*P_BIG);

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

#[derive(Debug, thiserror::Error)]
pub enum CheetahError {
    #[error("base58 decode error: {0}")]
    Base58(#[from] bs58::decode::Error),

    #[error("used zpub import key instead of address")]
    ZPubUsed,

    #[error("invalid base58 string length, got {0}")]
    InvalidLength(usize),

    #[error("array conversion failed")]
    ArrayConversion,

    #[error("point is not on the curve")]
    NotOnCurve,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, NounDecode, NounEncode)]
pub struct CheetahPoint {
    pub x: F6lt,
    pub y: F6lt,
    pub inf: bool,
}

impl CheetahPoint {
    ///  A pubkey consists of a leading 1 byte and 12 base field elements that are 8 bytes each. (12*8) + 1 = 97.
    const BYTES: usize = 97;
    pub fn into_base58(&self) -> Result<String, CheetahError> {
        if self.inf {
            return Err(CheetahError::NotOnCurve);
        }
        // Convert the Belt values to u64 bytes
        let mut bytes = Vec::new();
        bytes.push(0x1);
        for belt in self.y.0.iter().rev().chain(self.x.0.iter().rev()) {
            bytes.extend_from_slice(&belt.0.to_be_bytes());
        }
        Ok(bs58::encode(bytes).into_string())
    }
    pub fn from_base58(b58: &str) -> Result<Self, CheetahError> {
        let v = bs58::decode(b58).into_vec()?;
        if v.len() != Self::BYTES {
            if b58.starts_with("zpub") {
                return Err(CheetahError::ZPubUsed);
            }
            return Err(CheetahError::InvalidLength(v.len()));
        }

        let mut v64 = v[1..]
            .chunks_exact(8)
            .map(|a| {
                let arr = <[u8; 8]>::try_from(a).map_err(|_| CheetahError::ArrayConversion)?;
                Ok(Belt(u64::from_be_bytes(arr)))
            })
            .collect::<Result<Vec<Belt>, CheetahError>>()?;

        v64.reverse();

        let c_pt = CheetahPoint {
            x: F6lt {
                0: <[Belt; 6]>::try_from(&v64[..6]).map_err(|_| CheetahError::ArrayConversion)?,
            },
            y: F6lt {
                0: <[Belt; 6]>::try_from(&v64[6..]).map_err(|_| CheetahError::ArrayConversion)?,
            },
            inf: false,
        };

        if c_pt.in_curve() {
            Ok(c_pt)
        } else {
            Err(CheetahError::NotOnCurve)
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

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, NounDecode, NounEncode)]
pub struct F6lt(pub [Belt; 6]);

#[inline(always)]
pub fn f6_div(f1: &F6lt, f2: &F6lt) -> Result<F6lt, JetErr> {
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
pub fn f6_mul(f: &F6lt, g: &F6lt) -> F6lt {
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
pub fn f6_inv(f: &F6lt) -> Result<F6lt, JetErr> {
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
pub fn ch_double_unsafe(x: &F6lt, y: &F6lt) -> Result<CheetahPoint, JetErr> {
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

pub const A_ID: CheetahPoint = CheetahPoint {
    x: F6_ZERO,
    y: F6_ONE,
    inf: true,
};
pub const F6_ZERO: F6lt = F6lt([Belt(0); 6]);
pub const F6_ONE: F6lt = F6lt([Belt(1), Belt(0), Belt(0), Belt(0), Belt(0), Belt(0)]);

#[inline(always)]
pub fn ch_double(p: CheetahPoint) -> Result<CheetahPoint, JetErr> {
    if p.inf {
        return Ok(A_ID);
    }
    if p.y == F6_ZERO {
        return Ok(A_ID);
    }
    ch_double_unsafe(&p.x, &p.y)
}

#[inline(always)]
pub fn ch_add_unsafe(p: CheetahPoint, q: CheetahPoint) -> Result<CheetahPoint, JetErr> {
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
pub fn ch_neg(p: &CheetahPoint) -> CheetahPoint {
    CheetahPoint {
        x: p.x,
        y: f6_neg(&p.y),
        inf: p.inf,
    }
}

#[inline(always)]
pub fn ch_add(p: &CheetahPoint, q: &CheetahPoint) -> Result<CheetahPoint, JetErr> {
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
pub fn ch_scal(n: u64, p: &CheetahPoint) -> Result<CheetahPoint, JetErr> {
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
pub fn ch_scal_big(n: &UBig, p: &CheetahPoint) -> Result<CheetahPoint, JetErr> {
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

pub fn trunc_g_order(a: &[u64]) -> UBig {
    let mut result = UBig::from(a[0]);
    result += &*P_BIG * UBig::from(a[1]);
    result += &*P_BIG_2 * UBig::from(a[2]);
    result += &*P_BIG_3 * UBig::from(a[3]);

    result % &*G_ORDER
}
#[cfg(test)]
mod test {
    use super::*;
    use crate::belt::Belt;

    const F6_TEST: F6lt = F6lt([
        Belt(13724052584687643294),
        Belt(6944593306454870014),
        Belt(10082672435494154603),
        Belt(6450272673873704561),
        Belt(2898784811200916299),
        Belt(15463938240345685194),
    ]);

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
}
