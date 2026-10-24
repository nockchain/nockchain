pub mod hash;

use std::ops::{Add, Mul, Sub};

use arrayref::array_ref;

use crate::belt::*;

pub const DIGEST_LENGTH: usize = 5;
pub const STATE_SIZE: usize = 16;
pub const NUM_SPLIT_AND_LOOKUP: usize = 4;
pub const LOG2_STATE_SIZE: usize = 4;
pub const CAPACITY: usize = 6;
pub const RATE: usize = 10;
pub const NUM_ROUNDS: usize = 7;
pub const R: u128 = 18446744073709551616;
pub const R2: u64 = 0xfffffffe00000001;
pub const R_MOD_P: u64 = 4294967295;
pub const RP: u128 = 0xffffffff000000010000000000000000;
pub const P: u64 = 0xffffffff00000001;

const LOOKUP_TABLE: [u8; 256] = [
    0, 7, 26, 63, 124, 215, 85, 254, 214, 228, 45, 185, 140, 173, 33, 240, 29, 177, 176, 32, 8,
    110, 87, 202, 204, 99, 150, 106, 230, 14, 235, 128, 213, 239, 212, 138, 23, 130, 208, 6, 44,
    71, 93, 116, 146, 189, 251, 81, 199, 97, 38, 28, 73, 179, 95, 84, 152, 48, 35, 119, 49, 88,
    242, 3, 148, 169, 72, 120, 62, 161, 166, 83, 175, 191, 137, 19, 100, 129, 112, 55, 221, 102,
    218, 61, 151, 237, 68, 164, 17, 147, 46, 234, 203, 216, 22, 141, 65, 57, 123, 12, 244, 54, 219,
    231, 96, 77, 180, 154, 5, 253, 133, 165, 98, 195, 205, 134, 245, 30, 9, 188, 59, 142, 186, 197,
    181, 144, 92, 31, 224, 163, 111, 74, 58, 69, 113, 196, 67, 246, 225, 10, 121, 50, 60, 157, 90,
    122, 2, 250, 101, 75, 178, 159, 24, 36, 201, 11, 243, 132, 198, 190, 114, 233, 39, 52, 21, 209,
    108, 238, 91, 187, 18, 104, 194, 37, 153, 34, 200, 143, 126, 155, 236, 118, 64, 80, 172, 89,
    94, 193, 135, 183, 86, 107, 252, 13, 167, 206, 136, 220, 207, 103, 171, 160, 76, 182, 227, 217,
    158, 56, 174, 4, 66, 109, 139, 162, 184, 211, 249, 47, 125, 232, 117, 43, 16, 42, 127, 20, 241,
    25, 149, 105, 156, 51, 53, 168, 145, 247, 223, 79, 78, 226, 15, 222, 82, 115, 70, 210, 27, 41,
    1, 170, 40, 131, 192, 229, 248, 255,
];

pub const MONT_ONE: u64 = R_MOD_P;

const ROUND_CONSTANTS_MONT_7: [u64; NUM_ROUNDS * STATE_SIZE] = [
    6813007285744613222, 9538108458283805344, 8266796718228611711, 10279833152781686635,
    16136178252164695712, 11678500968092896548, 18177224533631314584, 8519208882353197867,
    15278933395031186751, 5605030382266121712, 3266079902019342405, 16977155338689078860,
    575533378161618286, 14008024146379968822, 16952256074650551489, 10699818153468018415,
    16274322239097854776, 16277174423203830480, 15551543598572731978, 17734447432847017984,
    10634644696612177250, 14223629804877666866, 9951585614956111842, 13410522825507264153,
    15504271780310363158, 15788426030062030790, 7247426745733321025, 15545848059337170693,
    7257327654080199927, 2632620606461733813, 5468949404670321892, 3408181798280532022,
    6407521478186447124, 10532483258532500040, 9962573180077511189, 14997058441937336819,
    8347291381276979462, 1834710304424753372, 8919127106750279878, 17952692726686580444,
    10425759383842794244, 3571063091305112274, 11196674225031209104, 17831978239644188755,
    7386054759687923415, 49233562557975441, 15763370708992892484, 7042466268943341941,
    14925546578125121441, 5737865664192390903, 6112071640890712275, 7093386846491465789,
    12933769084390453308, 840431699266909703, 2593502341286518015, 805532971224672190,
    662776811092263083, 1082592850076858062, 2260713232066289719, 18161814497919979745,
    11436170062534698819, 9156670326168191466, 13690674722453603930, 16450526946025880915,
    3443037901035637703, 13512956751884108002, 12765464435334038877, 16857582347068713433,
    4403818324750733470, 16327824648413653612, 9624633671524957693, 11798148227002487001,
    4806282851616964758, 13789375745913111929, 8048230392833675591, 15394445679479006170,
    5381819560221452561, 4546720664034456941, 17286163612312122987, 16936562784938244714,
    11067749825657848638, 5556080822347806028, 3866118074743041663, 2201009632364155631,
    10808969316669713964, 9312983943061336112, 17369380183573126906, 12953586427039891533,
    16564382082196301935, 6117018641086235131, 2379948990303454544, 9900641007991965131,
    14289331750432136160, 12105488135916678431, 14113550218116986428, 13441194625000926086,
    8346758232352358445, 13109503806329090541, 16233458644157342064, 3717000905522992223,
    4028024080310608291, 16928904978228437531, 486523272751840851, 17746229827600458028,
    4231774801891550196, 11401341037617516726, 12004481761165906799, 1880237553532135241,
    7506757868197934780, 1656439004520781315, 7739084580576441604, 17945328382079677663,
];

const MDS_MATRIX_FIRST_COLUMN_I64: [i64; STATE_SIZE] = [
    61402, 1108, 28750, 33823, 7454, 43244, 53865, 12034, 56951, 27521, 41351, 40901, 12021, 59689,
    26798, 17845,
];

pub fn permute(sponge: &mut [u64; 16]) {
    for i in 0..NUM_ROUNDS {
        let a = sbox_layer(array_ref![sponge, 0, STATE_SIZE]);
        let b = mds_cyclomul(&a);

        for j in 0..STATE_SIZE {
            sponge[j] = badd(ROUND_CONSTANTS_MONT_7[i * STATE_SIZE + j], b[j]);
        }
    }
}

/// Round count for the **paper-spec 5-round Tip5 variant** (Tip5 paper IACR
/// ePrint 2023/107 §2.4, N=5). Distinct from the canonical [`NUM_ROUNDS`] (= 7).
/// Used ONLY by the ai-pow-zk recursive-certificate proving stack; the canonical
/// Nockchain hash remains the 7-round [`permute`].
pub const NUM_ROUNDS_5ROUND: usize = 5;

/// **5-round Tip5 variant for ai-pow-zk recursive-certificate proving** —
/// identical to [`permute`] (same S-box, same cyclomul MDS, same round-constant
/// schedule) but iterating only the first 5 rounds
/// (`ROUND_CONSTANTS_MONT_7[0..5*STATE_SIZE]`). It must match the in-circuit
/// Tip5 adapter the recursion uses.
///
/// **Do not use this for canonical Nockchain hashing** — use 7-round [`permute`].
pub fn permute_5round(sponge: &mut [u64; 16]) {
    for i in 0..NUM_ROUNDS_5ROUND {
        let a = sbox_layer(array_ref![sponge, 0, STATE_SIZE]);
        let b = mds_cyclomul(&a);

        for j in 0..STATE_SIZE {
            sponge[j] = badd(ROUND_CONSTANTS_MONT_7[i * STATE_SIZE + j], b[j]);
        }
    }
}

fn sbox_layer(state: &[u64; STATE_SIZE]) -> [u64; STATE_SIZE] {
    let mut res: [u64; STATE_SIZE] = [0; STATE_SIZE];

    for i in 0..NUM_SPLIT_AND_LOOKUP {
        let mut bytes = state[i].to_le_bytes();
        for i in 0..8 {
            bytes[i] = LOOKUP_TABLE[bytes[i] as usize];
        }
        res[i] = u64::from_le_bytes(bytes);
    }

    for j in NUM_SPLIT_AND_LOOKUP..STATE_SIZE {
        let x = state[j];
        let x2 = montiply(x, x);
        let x4 = montiply(x2, x2);
        let x3 = montiply(x, x2);
        res[j] = montiply(x3, x4);
    }
    res
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
#[repr(transparent)]
struct Complex([i64; 2]);

#[inline(always)]
fn cadd(a: &Complex, b: &Complex) -> Complex {
    Complex([a.0[0] + b.0[0], a.0[1] + b.0[1]])
}

#[inline(always)]
fn csub(a: &Complex, b: &Complex) -> Complex {
    Complex([a.0[0] - b.0[0], a.0[1] - b.0[1]])
}

#[inline(always)]
fn csub3(a: &Complex, b: &Complex, c: &Complex) -> Complex {
    Complex([a.0[0] - b.0[0] - c.0[0], a.0[1] - b.0[1] - c.0[1]])
}

#[inline(always)]
fn cmul(f: &Complex, g: &Complex) -> Complex {
    let a = f.0[0] * g.0[0];
    let b = f.0[1] * g.0[1];
    let c = (f.0[0] + f.0[1]) * (g.0[0] + g.0[1]);
    Complex([a - b, c - a - b])
}

impl Add for Complex {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        cadd(&self, &other)
    }
}

impl Sub for Complex {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        csub(&self, &other)
    }
}

impl Mul for Complex {
    type Output = Self;

    fn mul(self, other: Self) -> Self {
        cmul(&self, &other)
    }
}

#[inline(always)]
fn cpoly_add<const N: usize>(f: &[Complex; N], g: &[Complex; N]) -> [Complex; N] {
    let mut res: [Complex; N] = [Complex([0, 0]); N];
    for i in 0..N {
        res[i] = cadd(&f[i], &g[i]);
    }
    res
}

#[inline(always)]
fn cpoly_sub3<const N: usize>(
    f: &[Complex; N],
    g: &[Complex; N],
    h: &[Complex; N],
) -> [Complex; N] {
    let mut res: [Complex; N] = [Complex([0, 0]); N];
    for i in 0..N {
        res[i] = csub3(&f[i], &g[i], &h[i]);
    }
    res
}

#[inline(always)]
fn complex_karatsuba_1(f: &[Complex; 2], g: &[Complex; 2]) -> [Complex; 3] {
    let a = cmul(&f[0], &g[0]);
    let c = cmul(&f[1], &g[1]);
    let b = csub3(&cmul(&cadd(&f[0], &f[1]), &cadd(&g[0], &g[1])), &a, &c);
    [a, b, c]
}

#[inline(always)]
fn complex_karatsuba_3(f: &[Complex; 4], g: &[Complex; 4]) -> [Complex; 7] {
    let a0: &[Complex; 2] = array_ref!(f, 2, 2);
    let a1: &[Complex; 2] = array_ref!(f, 0, 2);
    let b0: &[Complex; 2] = array_ref!(g, 2, 2);
    let b1: &[Complex; 2] = array_ref!(g, 0, 2);

    let m0: [Complex; 3] = complex_karatsuba_1(a0, b0);
    let m2: [Complex; 3] = complex_karatsuba_1(a1, b1);
    let mid: [Complex; 3] = complex_karatsuba_1(&cpoly_add(a0, a1), &cpoly_add(b0, b1));
    let m1: [Complex; 3] = cpoly_sub3(&mid, &m0, &m2);
    [m2[0], m2[1], cadd(&m2[2], &m1[0]), m1[1], cadd(&m1[2], &m0[0]), m0[1], m0[2]]
}

#[inline(always)]
fn zpoly_add<const N: usize>(f: &[i64; N], g: &[i64; N]) -> [i64; N] {
    let mut res: [i64; N] = [0; N];
    for i in 0..N {
        res[i] = f[i] + g[i];
    }
    res
}

#[inline(always)]
fn zpoly_sub<const N: usize>(f: &[i64; N], g: &[i64; N]) -> [i64; N] {
    let mut res: [i64; N] = [0; N];
    for i in 0..N {
        res[i] = f[i] - g[i];
    }
    res
}

#[inline(always)]
fn zpoly_sub3<const N: usize>(f: &[i64; N], g: &[i64; N], h: &[i64; N]) -> [i64; N] {
    let mut res: [i64; N] = [0; N];
    for i in 0..N {
        res[i] = f[i] - g[i] - h[i];
    }
    res
}

#[inline(always)]
fn integer_karatsuba_1(f: &[i64; 2], g: &[i64; 2]) -> [i64; 3] {
    let a = f[0] * g[0];
    let c = f[1] * g[1];
    let b = ((f[0] + f[1]) * (g[0] + g[1])) - a - c;
    [a, b, c]
}

#[inline(always)]
fn integer_karatsuba_3(f: &[i64; 4], g: &[i64; 4]) -> [i64; 7] {
    let a0: &[i64; 2] = array_ref!(f, 2, 2);
    let a1: &[i64; 2] = array_ref!(f, 0, 2);
    let b0: &[i64; 2] = array_ref!(g, 2, 2);
    let b1: &[i64; 2] = array_ref!(g, 0, 2);

    let m0: [i64; 3] = integer_karatsuba_1(a0, b0);
    let m2: [i64; 3] = integer_karatsuba_1(a1, b1);
    let m1: [i64; 3] = zpoly_sub3(
        &integer_karatsuba_1(&zpoly_add(a0, a1), &zpoly_add(b0, b1)),
        &m0,
        &m2,
    );
    [m2[0], m2[1], m2[2] + m1[0], m1[1], m1[2] + m0[0], m0[1], m0[2]]
}

#[inline(always)]
fn poly_mul_mod_x4_plus_1(f: &[i64; 4], g: &[i64; 4]) -> [i64; 4] {
    let prod: [i64; 7] = integer_karatsuba_3(f, g);
    [prod[0] - prod[4], prod[1] - prod[5], prod[2] - prod[6], prod[3]]
}

#[inline(always)]
fn poly_mul_mod_x4_minus_1(f: &[i64; 4], g: &[i64; 4]) -> [i64; 4] {
    let prod: [i64; 7] = integer_karatsuba_3(f, g);
    [prod[0] + prod[4], prod[1] + prod[5], prod[2] + prod[6], prod[3]]
}

#[inline(always)]
fn poly_mul_mod_x8_minus_1(f: &[i64; 8], g: &[i64; 8]) -> [i64; 8] {
    let f0: &[i64; 4] = array_ref!(f, 0, 4);
    let f1: &[i64; 4] = array_ref!(f, 4, 4);
    let g0: &[i64; 4] = array_ref!(g, 0, 4);
    let g1: &[i64; 4] = array_ref!(g, 4, 4);

    let p0: [i64; 4] = poly_mul_mod_x4_plus_1(&zpoly_sub(f0, f1), &zpoly_sub(g0, g1));
    let p1: [i64; 4] = poly_mul_mod_x4_minus_1(&zpoly_add(f0, f1), &zpoly_add(g0, g1));
    [
        (p0[0] + p1[0]) >> 1,
        (p0[1] + p1[1]) >> 1,
        (p0[2] + p1[2]) >> 1,
        (p0[3] + p1[3]) >> 1,
        (-p0[0] + p1[0]) >> 1,
        (-p0[1] + p1[1]) >> 1,
        (-p0[2] + p1[2]) >> 1,
        (-p0[3] + p1[3]) >> 1,
    ]
}

#[inline(always)]
fn poly_mul_mod_x8_plus_1(f: &[i64; 8], g: &[i64; 8]) -> [i64; 8] {
    let f0: &[i64; 4] = array_ref!(f, 0, 4);
    let f1: &[i64; 4] = array_ref!(f, 4, 4);
    let g0: &[i64; 4] = array_ref!(g, 0, 4);
    let g1: &[i64; 4] = array_ref!(g, 4, 4);

    let cf: [Complex; 4] = [
        Complex([f0[0], -f1[0]]),
        Complex([f0[1], -f1[1]]),
        Complex([f0[2], -f1[2]]),
        Complex([f0[3], -f1[3]]),
    ];
    let cg: [Complex; 4] = [
        Complex([g0[0], -g1[0]]),
        Complex([g0[1], -g1[1]]),
        Complex([g0[2], -g1[2]]),
        Complex([g0[3], -g1[3]]),
    ];
    let p: [Complex; 7] = complex_karatsuba_3(&cf, &cg);
    [
        p[0].0[0] + p[4].0[1],
        p[1].0[0] + p[5].0[1],
        p[2].0[0] + p[6].0[1],
        p[3].0[0],
        p[4].0[0] - p[0].0[1],
        p[5].0[0] - p[1].0[1],
        p[6].0[0] - p[2].0[1],
        -p[3].0[1],
    ]
}

#[inline(always)]
fn poly_mul_mod_x16_minus_1(f: &[i64; 16], g: &[i64; 16]) -> [i64; 16] {
    let f0: &[i64; 8] = array_ref!(f, 0, 8);
    let f1: &[i64; 8] = array_ref!(f, 8, 8);
    let g0: &[i64; 8] = array_ref!(g, 0, 8);
    let g1: &[i64; 8] = array_ref!(g, 8, 8);

    let p0: [i64; 8] = poly_mul_mod_x8_minus_1(&zpoly_add(f0, f1), &zpoly_add(g0, g1));
    let p1: [i64; 8] = poly_mul_mod_x8_plus_1(&zpoly_sub(f0, f1), &zpoly_sub(g0, g1));

    [
        (p0[0] + p1[0]) >> 1,
        (p0[1] + p1[1]) >> 1,
        (p0[2] + p1[2]) >> 1,
        (p0[3] + p1[3]) >> 1,
        (p0[4] + p1[4]) >> 1,
        (p0[5] + p1[5]) >> 1,
        (p0[6] + p1[6]) >> 1,
        (p0[7] + p1[7]) >> 1,
        (p0[0] - p1[0]) >> 1,
        (p0[1] - p1[1]) >> 1,
        (p0[2] - p1[2]) >> 1,
        (p0[3] - p1[3]) >> 1,
        (p0[4] - p1[4]) >> 1,
        (p0[5] - p1[5]) >> 1,
        (p0[6] - p1[6]) >> 1,
        (p0[7] - p1[7]) >> 1,
    ]
}

const LO_MASK: u64 = 0x00000000ffffffff;

#[inline(always)]
fn mds_cyclomul(state: &[u64; 16]) -> [u64; 16] {
    let hi: [i64; 16] = [
        (state[0] >> 32) as i64,
        (state[1] >> 32) as i64,
        (state[2] >> 32) as i64,
        (state[3] >> 32) as i64,
        (state[4] >> 32) as i64,
        (state[5] >> 32) as i64,
        (state[6] >> 32) as i64,
        (state[7] >> 32) as i64,
        (state[8] >> 32) as i64,
        (state[9] >> 32) as i64,
        (state[10] >> 32) as i64,
        (state[11] >> 32) as i64,
        (state[12] >> 32) as i64,
        (state[13] >> 32) as i64,
        (state[14] >> 32) as i64,
        (state[15] >> 32) as i64,
    ];
    let lo: [i64; 16] = [
        (state[0] & LO_MASK) as i64,
        (state[1] & LO_MASK) as i64,
        (state[2] & LO_MASK) as i64,
        (state[3] & LO_MASK) as i64,
        (state[4] & LO_MASK) as i64,
        (state[5] & LO_MASK) as i64,
        (state[6] & LO_MASK) as i64,
        (state[7] & LO_MASK) as i64,
        (state[8] & LO_MASK) as i64,
        (state[9] & LO_MASK) as i64,
        (state[10] & LO_MASK) as i64,
        (state[11] & LO_MASK) as i64,
        (state[12] & LO_MASK) as i64,
        (state[13] & LO_MASK) as i64,
        (state[14] & LO_MASK) as i64,
        (state[15] & LO_MASK) as i64,
    ];
    let hi_res: [i64; 16] = poly_mul_mod_x16_minus_1(&MDS_MATRIX_FIRST_COLUMN_I64, &hi);
    let lo_res: [i64; 16] = poly_mul_mod_x16_minus_1(&MDS_MATRIX_FIRST_COLUMN_I64, &lo);
    [
        reduce(((hi_res[0] as u128) << 32) + (lo_res[0] as u128)),
        reduce(((hi_res[1] as u128) << 32) + (lo_res[1] as u128)),
        reduce(((hi_res[2] as u128) << 32) + (lo_res[2] as u128)),
        reduce(((hi_res[3] as u128) << 32) + (lo_res[3] as u128)),
        reduce(((hi_res[4] as u128) << 32) + (lo_res[4] as u128)),
        reduce(((hi_res[5] as u128) << 32) + (lo_res[5] as u128)),
        reduce(((hi_res[6] as u128) << 32) + (lo_res[6] as u128)),
        reduce(((hi_res[7] as u128) << 32) + (lo_res[7] as u128)),
        reduce(((hi_res[8] as u128) << 32) + (lo_res[8] as u128)),
        reduce(((hi_res[9] as u128) << 32) + (lo_res[9] as u128)),
        reduce(((hi_res[10] as u128) << 32) + (lo_res[10] as u128)),
        reduce(((hi_res[11] as u128) << 32) + (lo_res[11] as u128)),
        reduce(((hi_res[12] as u128) << 32) + (lo_res[12] as u128)),
        reduce(((hi_res[13] as u128) << 32) + (lo_res[13] as u128)),
        reduce(((hi_res[14] as u128) << 32) + (lo_res[14] as u128)),
        reduce(((hi_res[15] as u128) << 32) + (lo_res[15] as u128)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::belt::{badd, bmul, montify};

    fn dense_mds_reference(state: &[u64; STATE_SIZE]) -> [u64; STATE_SIZE] {
        let mut result = [0; STATE_SIZE];
        for i in 0..STATE_SIZE {
            for j in 0..STATE_SIZE {
                let coeff = MDS_MATRIX_FIRST_COLUMN_I64[(i + STATE_SIZE - j) % STATE_SIZE] as u64;
                result[i] = badd(result[i], bmul(coeff, state[j]));
            }
        }
        result
    }

    #[test]
    fn tip5_mds_cyclomul_matches_dense_reference() {
        let mut state = [0; STATE_SIZE];
        for (i, item) in state.iter_mut().enumerate() {
            *item = montify(((i as u64 + 1) * 0x1_0001) % P);
        }

        assert_eq!(mds_cyclomul(&state), dense_mds_reference(&state));
    }

    fn parse_fixture_state(line: &str, prefix: &str, vector_index: usize) -> [u64; STATE_SIZE] {
        let Some(rest) = line.strip_prefix(prefix) else {
            panic!("vector {vector_index}: expected {prefix:?} line, got {line:?}");
        };
        let mut state = [0u64; STATE_SIZE];
        let mut fields = rest.split_whitespace();
        for (i, slot) in state.iter_mut().enumerate() {
            *slot = fields
                .next()
                .unwrap_or_else(|| panic!("vector {vector_index}: missing {prefix} field {i}"))
                .parse()
                .unwrap_or_else(|err| {
                    panic!("vector {vector_index}: invalid {prefix} field {i}: {err}")
                });
        }
        assert!(
            fields.next().is_none(),
            "vector {vector_index}: too many {prefix} fields"
        );
        state
    }

    #[test]
    fn tip5_5round_matches_ai_pow_zk_golden_fixture() {
        let fixture = include_str!("../../../ai-pow-zk/tests/fixtures/tip5_5round_golden_kat.txt");
        let mut lines = fixture
            .lines()
            .filter(|line| line.starts_with("IN ") || line.starts_with("OUT "));
        let mut vector_index = 0usize;

        while let Some(input_line) = lines.next() {
            let expected_line = lines
                .next()
                .unwrap_or_else(|| panic!("vector {vector_index}: missing OUT line"));
            let mut state = parse_fixture_state(input_line, "IN ", vector_index);
            let expected = parse_fixture_state(expected_line, "OUT ", vector_index);

            permute_5round(&mut state);
            assert_eq!(state, expected, "vector {vector_index}");
            vector_index += 1;
        }

        assert_eq!(vector_index, 315, "fixture vector count changed");
    }
}
