#![allow(clippy::len_without_is_empty)]

use std::ops::{Add, Div, Mul, Neg, Sub};

use nockvm::jets::util::BAIL_EXIT;
use nockvm::jets::JetErr;
use nockvm::noun::Noun;
use noun_serde::{NounDecode, NounEncode};
use num_traits::Pow;
use rkyv::{Archive, Deserialize, Serialize};
use serde::de::Error as SerdeError;
use serde::{Deserialize as SerdeDeserialize, Serialize as SerdeSerialize};
use tracing::debug;

// Base field arithmetic functions.
pub const PRIME: u64 = 18446744069414584321;
pub const PRIME_PRIME: u64 = PRIME - 2;
pub const PRIME_128: u128 = 18446744069414584321;
const RP: u128 = 340282366841710300967557013911933812736;
pub const R2: u128 = 18446744065119617025;
// const R_MOD_P: u64 = 0xFFFF_FFFF;
pub const H: u64 = 20033703337;
pub const ORDER: u64 = 2_u64.pow(32);

#[derive(
    Archive,
    Deserialize,
    Serialize,
    Copy,
    Clone,
    Debug,
    Eq,
    PartialEq,
    PartialOrd,
    Ord,
    Hash,
    Default,
)]
#[repr(transparent)]
pub struct Belt(pub u64);

impl SerdeSerialize for Belt {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u64(self.0)
    }
}

impl<'de> SerdeDeserialize<'de> for Belt {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        if !based_check(value) {
            return Err(SerdeError::custom("Belt value not based"));
        }
        Ok(Belt(value))
    }
}

pub fn based_check(a: u64) -> bool {
    a < PRIME
}

#[macro_export]
macro_rules! based {
    ( $( $x:expr ),* ) => {
      {
          $(
              debug_assert!($crate::belt::based_check($x), "element must be inside the field\r");
          )*
      }
    };
}
// Manual implementation for Belt to encode/decode as atom
impl NounEncode for Belt {
    fn to_noun<A: nockvm::noun::NounAllocator>(&self, allocator: &mut A) -> nockvm::noun::Noun {
        nockvm::noun::Atom::new(allocator, self.0).as_noun()
    }
}

impl NounDecode for Belt {
    fn from_noun(noun: &nockvm::noun::Noun) -> Result<Self, noun_serde::NounDecodeError> {
        let atom = noun
            .as_atom()
            .map_err(|_| noun_serde::NounDecodeError::ExpectedAtom)?;
        let value = atom
            .as_u64()
            .map_err(|_| noun_serde::NounDecodeError::Custom("Belt value too large".to_string()))?;
        if !based_check(value) {
            return Err(noun_serde::NounDecodeError::Custom(
                "Belt value not based".to_string(),
            ));
        }
        Ok(Belt(value))
    }
}

const ROOTS: &[u64] = &[
    0x0000000000000001, 0xffffffff00000000, 0x0001000000000000, 0xfffffffeff000001,
    0xefffffff00000001, 0x00003fffffffc000, 0x0000008000000000, 0xf80007ff08000001,
    0xbf79143ce60ca966, 0x1905d02a5c411f4e, 0x9d8f2ad78bfed972, 0x0653b4801da1c8cf,
    0xf2c35199959dfcb6, 0x1544ef2335d17997, 0xe0ee099310bba1e2, 0xf6b2cffe2306baac,
    0x54df9630bf79450e, 0xabd0a6e8aa3d8a0e, 0x81281a7b05f9beac, 0xfbd41c6b8caa3302,
    0x30ba2ecd5e93e76d, 0xf502aef532322654, 0x4b2a18ade67246b5, 0xea9d5a1336fbc98b,
    0x86cdcc31c307e171, 0x4bbaf5976ecfefd8, 0xed41d05b78d6e286, 0x10d78dd8915a171d,
    0x59049500004a4485, 0xdfa8c93ba46d2666, 0x7e9bd009b86a0845, 0x400a7f755588e659,
    0x185629dcda58878c,
];

impl Belt {
    #[inline(always)]
    pub fn zero() -> Self {
        Belt(Default::default())
    }

    #[inline(always)]
    pub fn one() -> Self {
        Belt(1)
    }

    #[inline(always)]
    pub fn is_zero(&self) -> bool {
        self.0 == 0
    }

    #[inline(always)]
    pub fn is_one(&self) -> bool {
        self.0 == 1
    }

    #[inline(always)]
    pub fn ordered_root(&self) -> Result<Self, FieldError> {
        // Belt(bpow(H, ORDER / self.0))
        let log_of_self = self.0.ilog2();
        if (log_of_self as usize) >= ROOTS.len() {
            debug!("ordered_root: out of bounds");
            return Err(FieldError::OrderedRootError);
        }
        // assert that it was an even power of two
        if self.0 != 1 << log_of_self {
            debug!("ordered_root: not power of two");
            return Err(FieldError::OrderedRootError);
        }
        Ok(ROOTS[log_of_self as usize].into())
    }

    #[inline(always)]
    pub fn inv(&self) -> Self {
        Belt(binv(self.0))
    }
}

impl From<FieldError> for JetErr {
    fn from(err: FieldError) -> Self {
        match err {
            FieldError::OrderedRootError => BAIL_EXIT,
        }
    }
}

impl Add for Belt {
    type Output = Self;

    #[inline(always)]
    fn add(self, rhs: Self) -> Self::Output {
        let a = self.0;
        let b = rhs.0;
        Belt(badd(a, b))
    }
}

impl Sub for Belt {
    type Output = Self;

    #[inline(always)]
    fn sub(self, rhs: Self) -> Self::Output {
        let a = self.0;
        let b = rhs.0;
        Belt(bsub(a, b))
    }
}

impl Neg for Belt {
    type Output = Self;

    #[inline(always)]
    fn neg(self) -> Self::Output {
        let a = self.0;
        Belt(bneg(a))
    }
}

impl Mul for Belt {
    type Output = Self;

    #[inline(always)]
    fn mul(self, rhs: Self) -> Self::Output {
        let a = self.0;
        let b = rhs.0;
        Belt(bmul(a, b))
    }
}

impl Pow<usize> for Belt {
    type Output = Self;

    #[inline(always)]
    fn pow(self, rhs: usize) -> Self::Output {
        Belt(bpow(self.0, rhs as u64))
    }
}

impl Div for Belt {
    type Output = Self;

    // No need to check for based since mul and inv already do so
    #[inline(always)]
    #[allow(clippy::suspicious_arithmetic_impl)]
    fn div(self, rhs: Self) -> Self::Output {
        self * rhs.inv()
    }
}

impl PartialEq<u64> for Belt {
    #[inline(always)]
    fn eq(&self, other: &u64) -> bool {
        self.0 == *other
    }
}

impl PartialEq<Belt> for u64 {
    #[inline(always)]
    fn eq(&self, other: &Belt) -> bool {
        *self == other.0
    }
}

impl AsRef<u64> for Belt {
    #[inline(always)]
    fn as_ref(&self) -> &u64 {
        &self.0
    }
}

impl TryFrom<&u64> for Belt {
    type Error = ();

    #[inline(always)]
    fn try_from(f: &u64) -> Result<Self, Self::Error> {
        based!(*f);
        Ok(Belt(*f))
    }
}

impl TryFrom<Noun> for Belt {
    type Error = ();

    #[inline(always)]
    fn try_from(n: Noun) -> std::result::Result<Self, Self::Error> {
        if !n.is_atom() {
            Err(())
        } else {
            Belt::try_from(&n.as_atom()?.as_u64()?)
        }
    }
}

impl From<u64> for Belt {
    #[inline(always)]
    fn from(f: u64) -> Self {
        Belt(f)
    }
}

impl From<Belt> for u64 {
    #[inline(always)]
    fn from(b: Belt) -> Self {
        b.0
    }
}

impl From<u32> for Belt {
    #[inline(always)]
    fn from(f: u32) -> Self {
        Belt(f as u64)
    }
}

impl From<Belt> for u32 {
    #[inline(always)]
    fn from(b: Belt) -> Self {
        b.0 as u32
    }
}

impl quickcheck::Arbitrary for Belt {
    fn arbitrary(g: &mut quickcheck::Gen) -> Self {
        Belt(u64::arbitrary(g) % PRIME)
    }
}

#[derive(Debug)]
pub enum FieldError {
    OrderedRootError,
}

#[inline(always)]
pub fn mont_reduction(a: u128) -> u64 {
    debug_assert!(a < RP, "element must be inside the field\r");
    let x1: u128 = (a >> 32) & 0xffffffff;
    let x2: u128 = a >> 64;
    let c: u128 = {
        let x0: u128 = a & 0xffffffff;
        (x0 + x1) << 32
    };
    let f: u128 = c >> 64;
    let d: u128 = c - (x1 + (f * PRIME_128));
    if x2 >= d {
        (x2 - d) as u64
    } else {
        (x2 + PRIME_128 - d) as u64
    }
}

#[inline(always)]
pub fn montiply(a: u64, b: u64) -> u64 {
    based!(a);
    based!(b);

    mont_reduction((a as u128) * (b as u128))
}

#[inline(always)]
pub fn montify(a: u64) -> u64 {
    based!(a);

    mont_reduction((a as u128) * R2)
}

#[inline(always)]
pub fn badd(a: u64, b: u64) -> u64 {
    based!(a);
    based!(b);

    let b = PRIME.wrapping_sub(b);
    let (r, c) = a.overflowing_sub(b);
    let adj = 0u32.wrapping_sub(c as u32);
    r.wrapping_sub(adj as u64)
}

#[inline(always)]
pub fn bneg(a: u64) -> u64 {
    based!(a);
    if a != 0 {
        PRIME - a
    } else {
        0
    }
}

#[inline(always)]
pub fn bsub(a: u64, b: u64) -> u64 {
    based!(a);
    based!(b);

    let (r, c) = a.overflowing_sub(b);
    let adj = 0u32.wrapping_sub(c as u32);
    r.wrapping_sub(adj as u64)
}

/// Reduce a 128 bit number
#[inline(always)]
pub fn reduce(n: u128) -> u64 {
    reduce_159(n as u64, (n >> 64) as u32, (n >> 96) as u64)
}

/// Reduce a 159 bit number
/// See <https://cp4space.hatsya.com/2021/09/01/an-efficient-prime-for-number-theoretic-transforms/>
/// See <https://github.com/mir-protocol/plonky2/blob/3a6d693f3ffe5aa1636e0066a4ea4885a10b5cdf/field/src/goldilocks_field.rs#L340-L356>
/// Removing both branch_hints can cause misleading changes to performance. bmul and especially
/// bpow in their micro-benchmarks will appear to be faster but higher-level stuff like bp_fft
/// will be slower. Make sure you validate your changes across the whole benchmark suite.
/// We have wrapping benchmarks (bpow(PRIME - 1, 5)) that are meant to be sensitive to the edge
/// cases but they seem to get faster anyway.
#[inline(always)]
pub fn reduce_159(low: u64, mid: u32, high: u64) -> u64 {
    let (mut low2, carry) = low.overflowing_sub(high);
    if carry {
        low2 = low2.wrapping_add(PRIME);
    }

    let mut product = (mid as u64) << 32;
    product -= product >> 32;

    let (mut result, carry) = product.overflowing_add(low2);
    if carry {
        // This branch is likely to happen. It should compile to a use
        // branchless conditional operations. This seems counter-intuitive,
        // but we get better performance out of bpow from this branch_hint.
        result = result.wrapping_sub(PRIME);
    }

    if result >= PRIME {
        // TODO: 2025-04-26: Chris A: I'm not sure that it's actually guaranteed,
        // when I unified the two branches, it caused an error.
        // This branch is unlikely to happen. It is guaranteed not to be taken
        // if the above branch was taken. (But merging the two branches is
        // slower.)
        // TODO: 2025-04-26: Chris A: +20% improvement to roswell prove_block pow/128 vs. branch_hint
        core::hint::cold_path();
        result -= PRIME;
    }
    result
}

#[inline(always)]
pub fn bmul(a: u64, b: u64) -> u64 {
    based!(a);
    based!(b);
    reduce((a as u128) * (b as u128))
}

#[inline(always)]
pub fn binv(a: u64) -> u64 {
    based!(a);
    let y = montify(a);
    let y2 = montiply(y, montiply(y, y));
    let y3 = montiply(y, montiply(y2, y2));
    let y5 = montiply(y2, montwopow(y3, 2));
    let y10 = montiply(y5, montwopow(y5, 5));
    let y20 = montiply(y10, montwopow(y10, 10));
    let y30 = montiply(y10, montwopow(y20, 10));
    let y31 = montiply(y, montiply(y30, y30));
    let dup = montiply(montwopow(y31, 32), y31);

    mont_reduction(montiply(y, montiply(dup, dup)).into())
}

#[inline(always)]
pub fn montwopow(a: u64, b: u32) -> u64 {
    based!(a);
    // if b == 0 {
    //     return a;
    // }

    let mut res = a;
    for _ in 0..b {
        res = montiply(res, res);
    }
    res
}

#[inline(always)]
pub fn bpow(mut a: u64, mut b: u64) -> u64 {
    based!(a);
    based!(b);

    let mut c: u64 = 1;
    if b == 0 {
        return c;
    }

    while b > 1 {
        if b & 1 == 0 {
            a = reduce((a as u128) * (a as u128));
            b /= 2;
        } else {
            c = reduce((c as u128) * (a as u128));
            a = reduce((a as u128) * (a as u128));
            b = (b - 1) / 2;
        }
    }
    reduce((c as u128) * (a as u128))
}
