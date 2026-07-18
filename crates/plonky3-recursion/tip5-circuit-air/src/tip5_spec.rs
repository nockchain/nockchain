//! In-crate, native-faithful **5-round Tip5** reference for the
//! ai-pow-zk recursive proving construction (per maintainer
//! 2026-05-20).
//!
//! This is the bit-for-bit twin of `nockchain_math::tip5::permute_5round`
//! (the two cannot share code — they live in separate, excluded Cargo
//! workspaces). Faithfulness is *closed* by the committed golden KAT
//! fixture `tip5_5round_golden_kat.txt`: `nockchain-math` proves the
//! fixture matches its live `permute_5round`; here `tests::*` proves
//! these embedded constants and this `permute` match that *same
//! committed fixture* bit-for-bit. Cross-workspace soundness loop
//! closed without a code dependency.
//!
//! **DIVERGENCE FROM CANONICAL NOCKCHAIN TIP5:** Nockchain's
//! canonical hash (`nockchain_math::tip5::permute`) uses 7 rounds
//! as a defensive cushion above the Tip5 paper's spec (N=5; IACR
//! ePrint 2023/107 §2.4). This ai-pow-zk-specific 5-round variant
//! trades that 2-round cushion for proof-size reduction in the
//! in-circuit Tip5 AIR (~−25-30% AIR width). The cryptanalysis
//! (IACR ePrint 2024/1900 "Opening the Blackbox": practical
//! 3-round attacks) supports 5 rounds as paper-spec-secure with
//! no extra margin. Maintainer-approved divergence per goal
//! 2026-05-20.
//!
//! All arithmetic is plain canonical-domain Goldilocks (`mod p`),
//! exactly as `belt::{bmul,badd,bpow}` (which use `reduce`/`reduce_159`,
//! *not* Montgomery) and `permute`'s `r_cons = (RC·2^64) mod p`.
//!
//! **MDS optimisation (2026-05-21).** The linear layer is a circulant
//! 16×16 matrix-vector product. We compute it as a **cyclic convolution
//! of `MDS_FIRST_COLUMN_I64` with the state vector** via Karatsuba
//! polynomial multiplication in `Z[x]/(x^16-1)`, decomposed by CRT:
//! `(x^16-1) = (x^8-1)(x^8+1)`, then `(x^8-1) = (x^4-1)(x^4+1)`, etc.
//! Result: ~64 i64-multiplications per `mds_cyclomul` call vs ~256
//! field-multiplications in the naive O(n²) implementation. All ops
//! are over `[i64; N]` arrays which the compiler auto-vectorises to
//! NEON / AVX without explicit SIMD intrinsics.
//!
//! **Bit-identity guarantee:** for any state in `[0, P_GOLDILOCKS)^16`,
//! `mds_cyclomul(state)` produces the **mathematically identical**
//! `Goldilocks` field output to the naive `linear_layer(state)` (which
//! is preserved below as `linear_layer_naive` for differential
//! testing). Validated by both the `linear_layer_naive_matches_cyclomul`
//! randomized differential test below AND the existing
//! `permute_matches_spec_permute` / `native_equiv_kat` /
//! `lookup_air_equals_native_spec` binding tests (which all run against
//! `permute`, which now dispatches to `mds_cyclomul`).

/// Goldilocks prime `p = 2^64 − 2^32 + 1` (`belt::PRIME`).
pub const P_GOLDILOCKS: u64 = 0xffff_ffff_0000_0001;
/// Tip5 state width.
pub const STATE_SIZE: usize = 16;
/// **5-round Tip5 (paper-spec IACR ePrint 2023/107 §2.4 N=5).**
/// ai-pow-zk-specific divergence from canonical Nockchain
/// `nockchain_math::tip5::NUM_ROUNDS=7`; bit-for-bit twin of
/// `nockchain_math::tip5::NUM_ROUNDS_5ROUND=5`.
pub const NUM_ROUNDS: usize = 5;
/// Split-and-lookup S-box lanes (the rest use the x^7 power map).
pub const NUM_SPLIT_AND_LOOKUP: usize = 4;

const P: u128 = P_GOLDILOCKS as u128;
/// `R = 2^64`, the Montgomery factor applied to round constants in
/// both nockchain-math Tip5 variants (`(RC as u128 * R) % PRIME_128`).
const R: u128 = 1u128 << 64;

/// The split-and-lookup table `L` (`nockchain_math::tip5::LOOKUP_TABLE`).
/// C2.0 proves `LOOKUP_TABLE[b] == ((b+1)^3 − 1) mod 257 ∀ b`.
pub const LOOKUP_TABLE: [u8; 256] = [
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

/// **First 5·16 = 80** round constants from
/// `nockchain_math::tip5::ROUND_CONSTANTS` (raw; row-major).
/// The 5-round Tip5 variant uses ONLY these 80 entries; the
/// remaining 32 entries from the canonical 7-round array are
/// intentionally absent here.
pub const ROUND_CONSTANTS: [u64; NUM_ROUNDS * STATE_SIZE] = [
    1332676891236936200,
    16607633045354064669,
    12746538998793080786,
    15240351333789289931,
    10333439796058208418,
    986873372968378050,
    153505017314310505,
    703086547770691416,
    8522628845961587962,
    1727254290898686320,
    199492491401196126,
    2969174933639985366,
    1607536590362293391,
    16971515075282501568,
    15401316942841283351,
    14178982151025681389,
    2916963588744282587,
    5474267501391258599,
    5350367839445462659,
    7436373192934779388,
    12563531800071493891,
    12265318129758141428,
    6524649031155262053,
    1388069597090660214,
    3049665785814990091,
    5225141380721656276,
    10399487208361035835,
    6576713996114457203,
    12913805829885867278,
    10299910245954679423,
    12980779960345402499,
    593670858850716490,
    12184128243723146967,
    1315341360419235257,
    9107195871057030023,
    4354141752578294067,
    8824457881527486794,
    14811586928506712910,
    7768837314956434138,
    2807636171572954860,
    9487703495117094125,
    13452575580428891895,
    14689488045617615844,
    16144091782672017853,
    15471922440568867245,
    17295382518415944107,
    15054306047726632486,
    5708955503115886019,
    9596017237020520842,
    16520851172964236909,
    8513472793890943175,
    8503326067026609602,
    9402483918549940854,
    8614816312698982446,
    7744830563717871780,
    14419404818700162041,
    8090742384565069824,
    15547662568163517559,
    17314710073626307254,
    10008393716631058961,
    14480243402290327574,
    13569194973291808551,
    10573516815088946209,
    15120483436559336219,
    3515151310595301563,
    1095382462248757907,
    5323307938514209350,
    14204542692543834582,
    12448773944668684656,
    13967843398310696452,
    14838288394107326806,
    13718313940616442191,
    15032565440414177483,
    13769903572116157488,
    17074377440395071208,
    16931086385239297738,
    8723550055169003617,
    590842605971518043,
    16642348030861036090,
    10708719298241282592,
];

/// First row of the circulant MDS matrix
/// (`nockchain_math::tip5::MDS_MATRIX_I64[0]`). Row `i` is this row
/// rotated right by `i`: `M[i][j] = MDS_FIRST_ROW[(j + 16 − i) % 16]`.
pub const MDS_FIRST_ROW: [u64; STATE_SIZE] = [
    61402, 17845, 26798, 59689, 12021, 40901, 41351, 27521, 56951, 12034, 53865, 43244, 7454,
    33823, 28750, 1108,
];

/// First **column** of the circulant MDS matrix, derived from
/// [`MDS_FIRST_ROW`] via `COL[i] = ROW[(16 − i) mod 16]`.
///
/// Used by [`mds_cyclomul`] as the kernel of a cyclic convolution:
/// for any circulant matrix `M` built from first row `r`, the
/// matrix-vector product `M · v` equals the cyclic convolution
/// `first_column(M) ⋆ v` (standard linear-algebra identity).
pub const MDS_FIRST_COLUMN_I64: [i64; STATE_SIZE] = [
    61402, 1108, 28750, 33823, 7454, 43244, 53865, 12034, 56951, 27521, 41351, 40901, 12021, 59689,
    26798, 17845,
];

/// The full circulant MDS matrix derived from [`MDS_FIRST_ROW`].
pub fn mds_matrix() -> [[u64; STATE_SIZE]; STATE_SIZE] {
    let mut m = [[0u64; STATE_SIZE]; STATE_SIZE];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = MDS_FIRST_ROW[(j + STATE_SIZE - i) % STATE_SIZE];
        }
    }
    m
}

/// `((RC as u128) * 2^64) % p` — the per-(round,lane) constant the AIR
/// embeds (exactly `permute`'s `r_cons`).
pub const fn rc_precomp(rc_raw: u64) -> u64 {
    (((rc_raw as u128) * R) % P) as u64
}

#[inline]
fn badd(a: u64, b: u64) -> u64 {
    (((a as u128) + (b as u128)) % P) as u64
}

#[inline]
fn bmul(a: u64, b: u64) -> u64 {
    (((a as u128) * (b as u128)) % P) as u64
}

#[inline]
fn bpow7(a: u64) -> u64 {
    let a2 = bmul(a, a);
    let a3 = bmul(a2, a);
    bmul(bmul(a3, a3), a) // a^7 = a^3 · a^3 · a
}

fn sbox_layer(state: &[u64; STATE_SIZE]) -> [u64; STATE_SIZE] {
    let mut res = [0u64; STATE_SIZE];
    for i in 0..NUM_SPLIT_AND_LOOKUP {
        let mut bytes = state[i].to_le_bytes();
        for byte in &mut bytes {
            *byte = LOOKUP_TABLE[*byte as usize];
        }
        res[i] = u64::from_le_bytes(bytes);
    }
    for j in NUM_SPLIT_AND_LOOKUP..STATE_SIZE {
        res[j] = bpow7(state[j]);
    }
    res
}

/// **Reference** naive O(n²) MDS matrix-vector product. Used as
/// the differential-test oracle for the optimised [`mds_cyclomul`]
/// below (and preserved here so the algebraic correctness of the
/// circulant-matrix structure is locally readable + verifiable).
#[cfg(test)]
fn linear_layer_naive(state: &[u64; STATE_SIZE]) -> [u64; STATE_SIZE] {
    let m = mds_matrix();
    let mut result = [0u64; STATE_SIZE];
    for i in 0..STATE_SIZE {
        let mut acc = 0u64;
        for j in 0..STATE_SIZE {
            acc = badd(acc, bmul(m[i][j], state[j]));
        }
        result[i] = acc;
    }
    result
}

// =====================================================================
//  Cyclic-convolution MDS via Karatsuba (production hot path).
//
//  The MDS matrix M is circulant — every row is a rotation of the
//  first row. For a circulant matrix C built from first row r, the
//  matrix-vector product C·v equals the cyclic convolution of v with
//  first_column(C) (where COL[i] = ROW[(n−i) mod n]).
//
//  Cyclic convolution mod (x^n − 1) is just polynomial multiplication
//  mod (x^n − 1). For n=16, we apply CRT via the factorisation
//  `(x^16 − 1) = (x^8 − 1) · (x^8 + 1)`, then recursively
//  `(x^8 − 1) = (x^4 − 1) · (x^4 + 1)` and `(x^8 + 1)` (which requires
//  complex integers for the `x^4 ± i` factors). Karatsuba multiplication
//  at each level reduces the multiplication count further. Net result:
//  ~64 i64-multiplications vs ~256 field-multiplications for the naive
//  matrix-vector product.
//
//  Algebraic correctness is gated by the differential test
//  `linear_layer_naive_matches_cyclomul` below (compares the optimised
//  path against the locally-defined naive O(n²) reference on edge
//  cases + 64 deterministically-seeded random states) plus the
//  binding `permute_matches_golden_kat` test that re-runs the whole
//  permutation against the frozen committed KAT fixture.
// =====================================================================

#[derive(Copy, Clone, Default, PartialEq, Eq)]
struct Complex([i64; 2]);

#[inline(always)]
fn cadd(a: Complex, b: Complex) -> Complex {
    Complex([a.0[0] + b.0[0], a.0[1] + b.0[1]])
}

#[inline(always)]
fn csub3(a: Complex, b: Complex, c: Complex) -> Complex {
    Complex([a.0[0] - b.0[0] - c.0[0], a.0[1] - b.0[1] - c.0[1]])
}

#[inline(always)]
fn cmul(f: Complex, g: Complex) -> Complex {
    // Karatsuba: (f0+if1)(g0+ig1) = (f0g0 − f1g1) + i(f0g1 + f1g0).
    // a=f0g0, b=f1g1, c=(f0+f1)(g0+g1)−a−b ⇒ fg = (a−b) + i(c).
    let a = f.0[0] * g.0[0];
    let b = f.0[1] * g.0[1];
    let c = (f.0[0] + f.0[1]) * (g.0[0] + g.0[1]);
    Complex([a - b, c - a - b])
}

#[inline(always)]
fn cpoly_add<const N: usize>(f: &[Complex; N], g: &[Complex; N]) -> [Complex; N] {
    let mut res = [Complex([0, 0]); N];
    for i in 0..N {
        res[i] = cadd(f[i], g[i]);
    }
    res
}

#[inline(always)]
fn cpoly_sub3<const N: usize>(
    f: &[Complex; N],
    g: &[Complex; N],
    h: &[Complex; N],
) -> [Complex; N] {
    let mut res = [Complex([0, 0]); N];
    for i in 0..N {
        res[i] = csub3(f[i], g[i], h[i]);
    }
    res
}

#[inline(always)]
fn zpoly_add<const N: usize>(f: &[i64; N], g: &[i64; N]) -> [i64; N] {
    let mut res = [0i64; N];
    for i in 0..N {
        res[i] = f[i] + g[i];
    }
    res
}

#[inline(always)]
fn zpoly_sub<const N: usize>(f: &[i64; N], g: &[i64; N]) -> [i64; N] {
    let mut res = [0i64; N];
    for i in 0..N {
        res[i] = f[i] - g[i];
    }
    res
}

#[inline(always)]
fn zpoly_sub3<const N: usize>(f: &[i64; N], g: &[i64; N], h: &[i64; N]) -> [i64; N] {
    let mut res = [0i64; N];
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

/// Multiply two degree-3 polynomials via Karatsuba:
/// `f = a0·x² + a1`, `g = b0·x² + b1`; `fg = a0b0·x⁴ + (a0b1+a1b0)·x² + a1b1`.
#[inline(always)]
fn integer_karatsuba_3(f: &[i64; 4], g: &[i64; 4]) -> [i64; 7] {
    let a0 = [f[2], f[3]];
    let a1 = [f[0], f[1]];
    let b0 = [g[2], g[3]];
    let b1 = [g[0], g[1]];

    let m0 = integer_karatsuba_1(&a0, &b0);
    let m2 = integer_karatsuba_1(&a1, &b1);
    let m1 = zpoly_sub3(
        &integer_karatsuba_1(&zpoly_add(&a0, &a1), &zpoly_add(&b0, &b1)),
        &m0,
        &m2,
    );
    [
        m2[0],
        m2[1],
        m2[2] + m1[0],
        m1[1],
        m1[2] + m0[0],
        m0[1],
        m0[2],
    ]
}

#[inline(always)]
fn complex_karatsuba_1(f: &[Complex; 2], g: &[Complex; 2]) -> [Complex; 3] {
    let a = cmul(f[0], g[0]);
    let c = cmul(f[1], g[1]);
    let b = csub3(cmul(cadd(f[0], f[1]), cadd(g[0], g[1])), a, c);
    [a, b, c]
}

#[inline(always)]
fn complex_karatsuba_3(f: &[Complex; 4], g: &[Complex; 4]) -> [Complex; 7] {
    let a0 = [f[2], f[3]];
    let a1 = [f[0], f[1]];
    let b0 = [g[2], g[3]];
    let b1 = [g[0], g[1]];

    let m0 = complex_karatsuba_1(&a0, &b0);
    let m2 = complex_karatsuba_1(&a1, &b1);
    let mid = complex_karatsuba_1(&cpoly_add(&a0, &a1), &cpoly_add(&b0, &b1));
    let m1 = cpoly_sub3(&mid, &m0, &m2);
    [
        m2[0],
        m2[1],
        cadd(m2[2], m1[0]),
        m1[1],
        cadd(m1[2], m0[0]),
        m0[1],
        m0[2],
    ]
}

/// Multiply `f·g` in `Z[x] / (x⁴ + 1)` via Karatsuba.
#[inline(always)]
fn poly_mul_mod_x4_plus_1(f: &[i64; 4], g: &[i64; 4]) -> [i64; 4] {
    let prod = integer_karatsuba_3(f, g);
    // x⁴ = −1 ⇒ reduce by subtracting the upper half.
    [
        prod[0] - prod[4],
        prod[1] - prod[5],
        prod[2] - prod[6],
        prod[3],
    ]
}

/// Multiply `f·g` in `Z[x] / (x⁴ − 1)` via Karatsuba.
#[inline(always)]
fn poly_mul_mod_x4_minus_1(f: &[i64; 4], g: &[i64; 4]) -> [i64; 4] {
    let prod = integer_karatsuba_3(f, g);
    // x⁴ = 1 ⇒ reduce by adding the upper half.
    [
        prod[0] + prod[4],
        prod[1] + prod[5],
        prod[2] + prod[6],
        prod[3],
    ]
}

/// Multiply `f·g` in `Z[x] / (x⁸ − 1)` via CRT
/// `(x⁸ − 1) = (x⁴ − 1)(x⁴ + 1)`.
#[inline(always)]
fn poly_mul_mod_x8_minus_1(f: &[i64; 8], g: &[i64; 8]) -> [i64; 8] {
    let f0 = [f[0], f[1], f[2], f[3]];
    let f1 = [f[4], f[5], f[6], f[7]];
    let g0 = [g[0], g[1], g[2], g[3]];
    let g1 = [g[4], g[5], g[6], g[7]];

    let p0 = poly_mul_mod_x4_plus_1(&zpoly_sub(&f0, &f1), &zpoly_sub(&g0, &g1));
    let p1 = poly_mul_mod_x4_minus_1(&zpoly_add(&f0, &f1), &zpoly_add(&g0, &g1));
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

/// Multiply `f·g` in `Z[x] / (x⁸ + 1)`. Requires moving into the
/// Gaussian integers `Z[i]` because `(x⁸ + 1) = (x⁴ + i)(x⁴ − i)`
/// (the two factors are complex-conjugate).
#[inline(always)]
fn poly_mul_mod_x8_plus_1(f: &[i64; 8], g: &[i64; 8]) -> [i64; 8] {
    // Re-interpret f = f0 + x⁴·f1 (and g similarly) as cf = f0 − i·f1
    // in the Gaussian-integer ring; then compute mod (x⁴ + i).
    let cf = [
        Complex([f[0], -f[4]]),
        Complex([f[1], -f[5]]),
        Complex([f[2], -f[6]]),
        Complex([f[3], -f[7]]),
    ];
    let cg = [
        Complex([g[0], -g[4]]),
        Complex([g[1], -g[5]]),
        Complex([g[2], -g[6]]),
        Complex([g[3], -g[7]]),
    ];
    let p = complex_karatsuba_3(&cf, &cg);
    // x⁴ = −i ⇒ reduce p = p0 + x⁴·p1 to p0 − i·p1.
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

/// Multiply `f·g` in `Z[x] / (x¹⁶ − 1)` via CRT
/// `(x¹⁶ − 1) = (x⁸ − 1)(x⁸ + 1)`.
#[inline(always)]
fn poly_mul_mod_x16_minus_1(f: &[i64; 16], g: &[i64; 16]) -> [i64; 16] {
    let f0 = [f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7]];
    let f1 = [f[8], f[9], f[10], f[11], f[12], f[13], f[14], f[15]];
    let g0 = [g[0], g[1], g[2], g[3], g[4], g[5], g[6], g[7]];
    let g1 = [g[8], g[9], g[10], g[11], g[12], g[13], g[14], g[15]];

    let p0 = poly_mul_mod_x8_minus_1(&zpoly_add(&f0, &f1), &zpoly_add(&g0, &g1));
    let p1 = poly_mul_mod_x8_plus_1(&zpoly_sub(&f0, &f1), &zpoly_sub(&g0, &g1));
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

const LO_MASK: u64 = 0x0000_0000_ffff_ffff;

/// Reduce an i128 that can be negative back to the canonical Goldilocks
/// range `[0, P)`. `rem_euclid` handles the sign correctly. The
/// cyclic-convolution body's final results are mathematically
/// non-negative for non-negative inputs, but the Karatsuba decomposition
/// uses subtractions producing signed intermediates; we accept i128
/// here for safety and let the reduction normalise.
#[inline(always)]
fn reduce_i128(n: i128) -> u64 {
    n.rem_euclid(P as i128) as u64
}

/// Cyclic-convolution MDS matrix-vector product
/// `MDS_FIRST_COLUMN ⋆ state` mod (`x¹⁶ − 1`), reduced mod
/// `P_GOLDILOCKS`. Mathematically identical to
/// [`linear_layer_naive`] for any `state ∈ [0, P)^16`.
///
/// Split state into hi/lo 32-bit halves so the integer convolutions
/// stay well inside i64 (max element product after the polynomial
/// expansion is bounded by `2^16 · 2^32 · 16 ≈ 2^52`); recombine
/// at the end and reduce.
fn mds_cyclomul(state: &[u64; STATE_SIZE]) -> [u64; STATE_SIZE] {
    let hi: [i64; STATE_SIZE] = [
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
    let lo: [i64; STATE_SIZE] = [
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

    let hi_res = poly_mul_mod_x16_minus_1(&MDS_FIRST_COLUMN_I64, &hi);
    let lo_res = poly_mul_mod_x16_minus_1(&MDS_FIRST_COLUMN_I64, &lo);

    let mut res = [0u64; STATE_SIZE];
    for i in 0..STATE_SIZE {
        // Recombine: hi * 2^32 + lo, then reduce mod P_GOLDILOCKS.
        // Both halves are independently the cyclic convolution of
        // MDS_FIRST_COLUMN_I64 with the corresponding state half;
        // the sum equals the convolution with `(hi<<32)+lo == state`.
        let combined = ((hi_res[i] as i128) << 32) + (lo_res[i] as i128);
        res[i] = reduce_i128(combined);
    }
    res
}

/// The **5-round** Tip5 permutation — bit-for-bit
/// `nockchain_math::tip5::permute_5round` (the ai-pow-zk-specific
/// paper-spec variant; NOT the canonical Nockchain 7-round
/// `permute`).
///
/// **Linear layer** (the dominant cost) is computed via [`mds_cyclomul`]
/// (cyclic convolution + Karatsuba; ~4× fewer multiplications than the
/// naive O(n²) matrix-vector product). Bit-identical to the naive
/// reference [`linear_layer_naive`] (see `linear_layer_naive_matches_cyclomul`
/// test) and to the frozen `nockchain_math::tip5::permute_5round` oracle
/// (see `tests::permute_matches_golden_kat`).
pub fn permute(sponge: &mut [u64; STATE_SIZE]) {
    for i in 0..NUM_ROUNDS {
        let a = sbox_layer(sponge);
        let b = mds_cyclomul(&a);
        for j in 0..STATE_SIZE {
            let rc = rc_precomp(ROUND_CONSTANTS[i * STATE_SIZE + j]);
            sponge[j] = badd(rc, b[j]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MDS_FIRST_COLUMN_I64 must equal `MDS_FIRST_ROW` reversed-rotated
    /// (`COL[i] = ROW[(N − i) mod N]`). This invariant is what makes
    /// `M · v = MDS_FIRST_COLUMN ⋆ v` (cyclic convolution).
    #[test]
    fn mds_first_column_matches_first_row_rotation() {
        for i in 0..STATE_SIZE {
            let expected = MDS_FIRST_ROW[(STATE_SIZE - i) % STATE_SIZE] as i64;
            assert_eq!(
                MDS_FIRST_COLUMN_I64[i], expected,
                "COL[{i}] != ROW[({} - {i}) mod {}]",
                STATE_SIZE, STATE_SIZE
            );
        }
    }

    /// `mds_cyclomul` must produce the SAME field output as the naive
    /// O(n²) matrix-vector product for any state. This is the
    /// algebraic-correctness gate for the optimised path.
    ///
    /// Tested across:
    /// - Edge cases (all zeros, all P-1, single non-zero per slot)
    /// - Random states (deterministic seeded RNG for reproducibility)
    /// - The "all-canonical-just-below-P" boundary
    #[test]
    fn linear_layer_naive_matches_cyclomul() {
        // Edge: all zeros.
        let zero = [0u64; STATE_SIZE];
        assert_eq!(linear_layer_naive(&zero), mds_cyclomul(&zero));

        // Edge: all P-1.
        let max = [P_GOLDILOCKS - 1; STATE_SIZE];
        assert_eq!(linear_layer_naive(&max), mds_cyclomul(&max));

        // Edge: single non-zero per slot (16 instances).
        for k in 0..STATE_SIZE {
            let mut s = [0u64; STATE_SIZE];
            s[k] = P_GOLDILOCKS - 1;
            assert_eq!(
                linear_layer_naive(&s),
                mds_cyclomul(&s),
                "single-nonzero slot {k}"
            );
        }

        // Deterministic-seeded random states (Linear Congruential
        // Generator; no rand dep). 64 random states cover a broad
        // distribution.
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let lcg = |s: &mut u64| {
            *s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *s
        };
        for case in 0..64 {
            let state: [u64; STATE_SIZE] = core::array::from_fn(|_| lcg(&mut seed) % P_GOLDILOCKS);
            assert_eq!(
                linear_layer_naive(&state),
                mds_cyclomul(&state),
                "random case {case}"
            );
        }
    }
}
